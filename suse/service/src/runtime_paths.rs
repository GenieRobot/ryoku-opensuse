//! Fixed native daemon paths, pinned descriptors and a daemon-lifetime lock.
//! This lock prevents duplicate user daemons; it is not the shared mutation lock.

use crate::account_state::AccountHomeRoot;
use rustix::fs::{
    AtFlags, FileType, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, Stat, flock, fstat,
    fsync, mkdirat, openat2, renameat_with, statat, unlinkat,
};
use rustix::io::Errno;
use std::{
    fs::File,
    os::unix::net::UnixListener,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::PathBuf,
};

const SOCKET: &str = "ryoku-service.sock";
const LOCK: &str = "ryoku-service.lock";
const JOURNAL: &str = "operations.log";
const SERVICE_STATE: &str = "service";
const PRIVATE_MODE: u32 = 0o600;

pub(crate) struct RuntimePaths {
    root: File,
    home: AccountHomeRoot,
    runtime: File,
    state: StateDirectories,
    lock: File,
    journal: File,
    socket: File,
}

struct StateDirectories {
    local: File,
    state: File,
    app: File,
    service: File,
}
impl StateDirectories {
    fn open(home: &File, uid: u32) -> Result<Self, Errno> {
        let local = child_directory(home, ".local", uid, false)?;
        let state = child_directory(&local, "state", uid, false)?;
        // Upstream QML also owns this namespace and legitimately creates it
        // with mode 0755. Keep the native journal in its own private child.
        let app = child_directory(&state, "ryoku", uid, false)?;
        let service = child_directory(&app, SERVICE_STATE, uid, true)?;
        Ok(Self { local, state, app, service })
    }

    fn revalidate(&self, home: &File, uid: u32) -> Result<(), Errno> {
        let local = open_directory(home, ".local", true)?;
        let state = open_directory(&local, "state", true)?;
        let app = open_directory(&state, "ryoku", true)?;
        let service = open_directory(&app, SERVICE_STATE, true)?;
        for (pinned, current, private) in [
            (&self.local, &local, false),
            (&self.state, &state, false),
            (&self.app, &app, false),
            (&self.service, &service, true),
        ] {
            validate_directory(current, uid, private)?;
            same_inode(&fstat(pinned)?, &fstat(current)?)?;
        }
        Ok(())
    }

    // Caller holds the daemon lifetime lock. Never choose between two journals
    // or copy history: an atomic no-replace rename retains the legacy inode.
    fn open_journal(&self, home: &File, uid: u32) -> Result<File, Errno> {
        self.revalidate(home, uid)?;
        let legacy = named_entry_exists(&self.app, JOURNAL)?;
        let current = named_entry_exists(&self.service, JOURNAL)?;
        let journal = match (legacy, current) {
            (true, true) => return Err(Errno::EXIST),
            (true, false) => {
                let journal = open_regular(&self.app, JOURNAL, uid, false)?;
                fsync(&journal)?;
                self.revalidate(home, uid)?;
                require_named_inode(&self.app, JOURNAL, &journal)?;
                renameat_with(&self.app, JOURNAL, &self.service, JOURNAL, RenameFlags::NOREPLACE)?;
                validate_regular(&journal, uid)?;
                require_named_inode(&self.service, JOURNAL, &journal)?;
                journal
            }
            (false, true) => open_regular(&self.service, JOURNAL, uid, false)?,
            (false, false) => {
                open_regular_with_flags(&self.service, JOURNAL, uid, OFlags::CREATE | OFlags::EXCL)?
            }
        };
        // Synchronize both names, including on restart after an interrupted
        // migration, before replay can expose durable operation lookup.
        fsync(&self.service)?;
        fsync(&self.app)?;
        self.revalidate(home, uid)?;
        self.revalidate_journal(&journal, uid)?;
        Ok(journal)
    }

    fn revalidate_journal(&self, journal: &File, uid: u32) -> Result<(), Errno> {
        if named_entry_exists(&self.app, JOURNAL)? {
            return Err(Errno::EXIST);
        }
        validate_regular(journal, uid)?;
        require_named_inode(&self.service, JOURNAL, journal)
    }
}

fn named_entry_exists(parent: &File, name: &str) -> Result<bool, Errno> {
    match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(Errno::NOENT) => Ok(false),
        Err(error) => Err(error),
    }
}

impl RuntimePaths {
    pub(crate) fn open() -> Result<(Self, UnixListener, File), &'static str> {
        let home = AccountHomeRoot::for_current_user().map_err(|_| "account home unavailable")?;
        let uid = home.uid();
        let root = File::open("/").map_err(|_| "native root unavailable")?;
        let runtime = open_runtime(&root, uid).map_err(|_| "runtime directory unavailable")?;
        // Acquire before replay, journal opening or stale-socket cleanup. Keep
        // this descriptor alive for the whole service lifetime.
        let lock =
            open_regular(&runtime, LOCK, uid, true).map_err(|_| "daemon lock unavailable")?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| "daemon lock unavailable")?;
        require_named_inode(&runtime, LOCK, &lock).map_err(|_| "daemon lock changed")?;
        let state =
            StateDirectories::open(home.home(), uid).map_err(|_| "state directory unavailable")?;
        let journal =
            state.open_journal(home.home(), uid).map_err(|_| "operation journal unavailable")?;
        let (listener, socket) =
            bind_listener(&runtime, uid).map_err(|_| "service socket unavailable")?;
        let paths = Self { root, home, runtime, state, lock, journal, socket };
        // Reopen the fixed published namespace before journal replay. A pinned
        // descriptor alone does not prove its directory has not been renamed.
        paths.revalidate()?;
        let journal = paths.journal.try_clone().map_err(|_| "operation journal unavailable")?;
        Ok((paths, listener, journal))
    }

    pub(crate) fn uid(&self) -> u32 {
        self.home.uid()
    }

    pub(crate) fn revalidate(&self) -> Result<(), &'static str> {
        self.home.revalidate().map_err(|_| "account home changed")?;
        let uid = self.uid();
        let runtime = open_runtime(&self.root, uid).map_err(|_| "runtime directory changed")?;
        same_inode(
            &fstat(&self.runtime).map_err(|_| "runtime unavailable")?,
            &fstat(&runtime).map_err(|_| "runtime unavailable")?,
        )
        .map_err(|_| "runtime directory identity changed")?;
        self.state.revalidate(self.home.home(), uid).map_err(|_| "state directory changed")?;
        validate_regular(&self.lock, uid).map_err(|_| "private file metadata changed")?;
        require_named_inode(&self.runtime, LOCK, &self.lock)
            .map_err(|_| "private file identity changed")?;
        self.state
            .revalidate_journal(&self.journal, uid)
            .map_err(|_| "operation journal changed")?;
        validate_socket(&self.socket, uid).map_err(|_| "service socket metadata changed")?;
        require_named_inode(&self.runtime, SOCKET, &self.socket)
            .map_err(|_| "service socket identity changed")?;
        Ok(())
    }
}

fn open_runtime(root: &File, uid: u32) -> Result<File, Errno> {
    let run = open_directory(root, "run", false)?;
    validate_directory(&run, 0, false)?;
    // /run/user can itself be a native runtime mount, independently of both
    // /run and each /run/user/<UID> mount. Admit only these fixed boundaries;
    // all later opens below the UID runtime retain NO_XDEV.
    let users = open_directory(&run, "user", false)?;
    validate_directory(&users, 0, false)?;
    // The per-user directory may be its own native runtime mount.
    let runtime = open_directory(&users, &uid.to_string(), false)?;
    validate_directory(&runtime, uid, true)?;
    Ok(runtime)
}

fn open_directory(parent: &File, name: &str, same_mount: bool) -> Result<File, Errno> {
    let mut resolve =
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
    if same_mount {
        resolve |= ResolveFlags::NO_XDEV;
    }
    Ok(File::from(openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve,
    )?))
}

fn child_directory(parent: &File, name: &str, uid: u32, private: bool) -> Result<File, Errno> {
    match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) => fsync(parent)?,
        Err(Errno::EXIST) => {}
        Err(error) => return Err(error),
    }
    let directory = open_directory(parent, name, true)?;
    validate_directory(&directory, uid, private)?;
    Ok(directory)
}

fn validate_directory(file: &File, uid: u32, private: bool) -> Result<(), Errno> {
    let stat = fstat(file)?;
    let mode = stat.st_mode & 0o7777;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != uid
        || if private { mode != 0o700 } else { !matches!(mode, 0o700 | 0o750 | 0o755) }
    {
        return Err(Errno::PERM);
    }
    Ok(())
}

fn open_regular(parent: &File, name: &str, uid: u32, create: bool) -> Result<File, Errno> {
    open_regular_with_flags(
        parent,
        name,
        uid,
        if create { OFlags::CREATE } else { OFlags::empty() },
    )
}

fn open_regular_with_flags(
    parent: &File,
    name: &str,
    uid: u32,
    creation: OFlags,
) -> Result<File, Errno> {
    let flags = OFlags::RDWR
        | OFlags::APPEND
        | OFlags::CLOEXEC
        | OFlags::NOFOLLOW
        | OFlags::NONBLOCK
        | creation;
    let file = File::from(openat2(
        parent,
        name,
        flags,
        if creation.contains(OFlags::CREATE) {
            Mode::from_raw_mode(PRIVATE_MODE)
        } else {
            Mode::empty()
        },
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )?);
    validate_regular(&file, uid)?;
    require_named_inode(parent, name, &file)?;
    Ok(file)
}
fn validate_regular(file: &File, uid: u32) -> Result<(), Errno> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != uid
        || stat.st_mode & 0o7777 != PRIVATE_MODE
        || stat.st_nlink != 1
    {
        return Err(Errno::PERM);
    }
    Ok(())
}
fn same_inode(old: &Stat, current: &Stat) -> Result<(), Errno> {
    if old.st_dev != current.st_dev || old.st_ino != current.st_ino {
        return Err(Errno::PERM);
    }
    Ok(())
}
fn require_named_inode(parent: &File, name: &str, file: &File) -> Result<(), Errno> {
    same_inode(&fstat(file)?, &statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?)
}

fn fd_path(file: &File, leaf: Option<&str>) -> PathBuf {
    let path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    leaf.map_or(path.clone(), |leaf| path.join(leaf))
}

fn bind_listener(runtime: &File, uid: u32) -> Result<(UnixListener, File), Errno> {
    remove_stale_socket(runtime, uid)?;
    // Linux has no bindat. The kernel procfd reference anchors bind/connect to
    // the live held directory, without changing the process working directory.
    let listener = UnixListener::bind(fd_path(runtime, Some(SOCKET)))
        .map_err(|error| Errno::from_io_error(&error).unwrap_or(Errno::IO))?;
    let socket = File::from(openat2(
        runtime,
        SOCKET,
        OFlags::PATH | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )?);
    let stat = fstat(&socket)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || stat.st_uid != uid
        || stat.st_nlink != 1
    {
        return Err(Errno::PERM);
    }
    // chmod through the held O_PATH descriptor, never through a mutable leaf.
    std::fs::set_permissions(fd_path(&socket, None), std::fs::Permissions::from_mode(PRIVATE_MODE))
        .map_err(|error| Errno::from_io_error(&error).unwrap_or(Errno::IO))?;
    validate_socket(&socket, uid)?;
    require_named_inode(runtime, SOCKET, &socket)?;
    Ok((listener, socket))
}

fn validate_socket(file: &File, uid: u32) -> Result<(), Errno> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || stat.st_uid != uid
        || stat.st_mode & 0o7777 != PRIVATE_MODE
        || stat.st_nlink != 1
    {
        return Err(Errno::PERM);
    }
    Ok(())
}

fn remove_stale_socket(runtime: &File, uid: u32) -> Result<(), Errno> {
    let stat = match statat(runtime, SOCKET, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) => return Ok(()),
        result => result?,
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || stat.st_uid != uid
        || stat.st_nlink != 1
    {
        return Err(Errno::PERM);
    }
    let probe = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK,
        None,
    )?;
    let address = rustix::net::SocketAddrUnix::new(fd_path(runtime, Some(SOCKET)))?;
    match rustix::net::connect(&probe, &address) {
        Err(Errno::CONNREFUSED) => {}
        _ => return Err(Errno::BUSY),
    }
    same_inode(&stat, &statat(runtime, SOCKET, AtFlags::SYMLINK_NOFOLLOW)?)?;
    unlinkat(runtime, SOCKET, AtFlags::empty())
}

#[cfg(test)]
#[path = "runtime_paths_tests.rs"]
mod tests;
