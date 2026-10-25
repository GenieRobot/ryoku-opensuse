//! Account-owned writer leases for native Go and Quickshell processes.
//!
//! The root reader only connects to authenticated account sockets and locks a
//! read-only descriptor. Same-UID parent wrappers stop their own child trees.
//! Every supported entry point must launch through `ryoku-writer`; an upstream
//! binary launched outside the wrapper is an external, uncoordinated writer.

use crate::capture_session::{CaptureSessionError as Error, Result};
use rustix::{
    fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags, ResolveFlags},
    net::sockopt::socket_peercred,
};
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const LOCK: &str = "writers.lock";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_WRITERS: usize = 128;
static SHUTDOWN_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriterKind {
    Daemon,
    Provider,
    HubBackend,
    Ryogami,
    Ryoshot,
    Welcome,
    KeysHint,
}
impl WriterKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "daemon" => Ok(Self::Daemon),
            "provider" => Ok(Self::Provider),
            "hub-backend" => Ok(Self::HubBackend),
            "ryogami" => Ok(Self::Ryogami),
            "ryoshot" => Ok(Self::Ryoshot),
            "welcome" => Ok(Self::Welcome),
            "keys-hint" => Ok(Self::KeysHint),
            _ => Err(Error::Unsupported),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Provider => "provider",
            Self::HubBackend => "hub-backend",
            Self::Ryogami => "ryogami",
            Self::Ryoshot => "ryoshot",
            Self::Welcome => "welcome",
            Self::KeysHint => "keys-hint",
        }
    }
    fn command(self) -> Command {
        match self {
            Self::Daemon => Command::new("/usr/libexec/ryoku/upstream/ryoku-shell"),
            Self::Provider => Command::new("/usr/libexec/ryoku/upstream/ryoku-wm-hyprland"),
            Self::HubBackend => Command::new("/usr/libexec/ryoku/upstream/ryoku-hub"),
            Self::Ryogami => Command::new("/usr/libexec/ryoku/upstream/ryogami"),
            role => {
                let mut command = Command::new("/usr/libexec/ryoku/quickshell/qs");
                let path = match role {
                    Self::Ryoshot => "/usr/share/ryoku/shell/quickshell/ryoshot",
                    Self::Welcome => "/usr/share/ryoku/shell/quickshell/welcome",
                    Self::KeysHint => "/usr/share/ryoku/shell/quickshell/keys-hint",
                    _ => unreachable!(),
                };
                command.args(["-p", path]);
                command
            }
        }
    }
}

struct Namespace {
    root: File,
    uid: u32,
    gid: u32,
    native_mount: Option<u64>,
}
impl Namespace {
    fn native(uid: u32, gid: u32, create: bool) -> Result<Self> {
        let run = File::from(fs::open(
            "/run/user",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let stat = fs::fstat(&run)?;
        if stat.st_uid != 0 || stat.st_gid != 0 || stat.st_mode & 0o7022 != 0 {
            return Err(Error::UnsafeBoundary);
        }
        let runtime = File::from(fs::openat2(
            &run,
            uid.to_string(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE.difference(ResolveFlags::NO_XDEV),
        )?);
        directory(&runtime, uid, gid)?;
        let parent_mount = descriptor_mount(&run)?;
        let runtime_mount = descriptor_mount(&runtime)?;
        if parent_mount != runtime_mount {
            let mut info = String::new();
            File::open("/proc/self/mountinfo")?.take(1024 * 1024 + 1).read_to_string(&mut info)?;
            if info.len() > 1024 * 1024 || !runtime_mount_allowed(&info, runtime_mount, uid) {
                return Err(Error::UnsafeBoundary);
            }
        }
        if create {
            if uid != rustix::process::geteuid().as_raw() || uid == 0 {
                return Err(Error::Unsupported);
            }
            match fs::mkdirat(&runtime, "ryoku-writers", Mode::from_raw_mode(0o700)) {
                Ok(()) => {}
                Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let root = File::from(fs::openat2(
            &runtime,
            "ryoku-writers",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )?);
        directory(&root, uid, gid)?;
        let mount = descriptor_mount(&root)?;
        Ok(Self { root, uid, gid, native_mount: Some(mount) })
    }
    fn open_lock(&self, create: bool) -> Result<File> {
        directory(&self.root, self.uid, self.gid)?;
        let flags = if create { OFlags::RDWR | OFlags::CREATE } else { OFlags::RDONLY };
        let file = File::from(fs::openat2(
            &self.root,
            LOCK,
            flags | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            if create { Mode::from_raw_mode(0o600) } else { Mode::empty() },
            RESOLVE,
        )?);
        self.check_lock(&file)?;
        if create {
            fs::fsync(&self.root)?;
        }
        Ok(file)
    }
    fn check_lock(&self, file: &File) -> Result<()> {
        if let Some(mount) = self.native_mount {
            let current = Self::native(self.uid, self.gid, false)?;
            let original = fs::fstat(&self.root)?;
            let current_stat = fs::fstat(&current.root)?;
            if current.native_mount != Some(mount)
                || (original.st_dev, original.st_ino) != (current_stat.st_dev, current_stat.st_ino)
            {
                return Err(Error::UnsafeBoundary);
            }
        }
        directory(&self.root, self.uid, self.gid)?;
        let stat = fs::fstat(file)?;
        let current = fs::statat(&self.root, LOCK, AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_uid != self.uid
            || stat.st_gid != self.gid
            || stat.st_mode & 0o7777 != 0o600
            || stat.st_nlink != 1
            || (stat.st_dev, stat.st_ino) != (current.st_dev, current.st_ino)
        {
            return Err(Error::UnsafeBoundary);
        }
        Ok(())
    }
    fn path(&self, leaf: &str) -> String {
        format!("/proc/self/fd/{}/{leaf}", self.root.as_raw_fd())
    }
    fn sockets(&self) -> Result<Vec<(String, i32)>> {
        let mut result = Vec::new();
        for entry in fs::Dir::read_from(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_str().map_err(|_| Error::UnsafeBoundary)?;
            if matches!(name, "." | ".." | LOCK) {
                continue;
            }
            let (kind, pid) = name
                .strip_suffix(".sock")
                .and_then(|v| v.rsplit_once('-'))
                .ok_or(Error::UnsafeBoundary)?;
            WriterKind::parse(kind)?;
            let pid: i32 = pid.parse().map_err(|_| Error::UnsafeBoundary)?;
            if pid <= 0 || name != format!("{kind}-{pid}.sock") {
                return Err(Error::UnsafeBoundary);
            }
            let stat = fs::statat(&self.root, name, AtFlags::SYMLINK_NOFOLLOW)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
                || stat.st_uid != self.uid
                || stat.st_gid != self.gid
                || stat.st_mode & 0o7777 != 0o666
            {
                return Err(Error::UnsafeBoundary);
            }
            result.push((name.to_owned(), pid));
            if result.len() > MAX_WRITERS {
                return Err(Error::WriterUnavailable);
            }
        }
        result.sort();
        Ok(result)
    }
}
fn descriptor_mount(file: &File) -> Result<u64> {
    let stat = fs::statx(file, "", AtFlags::EMPTY_PATH, fs::StatxFlags::MNT_ID)?;
    if stat.stx_mask & fs::StatxFlags::MNT_ID.bits() == 0 || stat.stx_mnt_id == 0 {
        return Err(Error::UnsafeBoundary);
    }
    Ok(stat.stx_mnt_id)
}
// Only the exact systemd runtime tmpfs boundary can cross a mount here. No
// bind of a subtree, foreign mountpoint/filesystem, symlink or child mount is
// admitted. Descriptor mount IDs are pinned and rechecked by every lease check.
fn runtime_mount_allowed(info: &str, mount: u64, uid: u32) -> bool {
    let mut matching = info.lines().filter_map(|line| {
        let (before, after) = line.split_once(" - ")?;
        let before: Vec<_> = before.split_ascii_whitespace().collect();
        let after: Vec<_> = after.split_ascii_whitespace().collect();
        (before.first()?.parse::<u64>().ok()? == mount).then_some((before, after))
    });
    let Some((before, after)) = matching.next() else {
        return false;
    };
    matching.next().is_none()
        && before.len() >= 6
        && after.len() == 3
        && before[3] == "/"
        && before[4] == format!("/run/user/{uid}")
        && after[0] == "tmpfs"
        && after[1] == "tmpfs"
        && ["rw", "nosuid", "nodev"]
            .iter()
            .all(|required| before[5].split(',').any(|option| option == *required))
}
fn directory(file: &File, uid: u32, gid: u32) -> Result<()> {
    let stat = fs::fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != uid
        || stat.st_gid != gid
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(Error::UnsafeBoundary);
    }
    Ok(())
}

/// Initialize only this account's private coordination namespace. This does
/// not publish a recovery generation or accept a provider mutation.
pub fn initialize_writer_namespace() -> Result<()> {
    let namespace = Namespace::native(
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
        true,
    )?;
    drop(namespace.open_lock(true)?);
    Ok(())
}

pub(crate) struct CaptureWriterLease {
    namespace: Namespace,
    lock: Option<File>,
    writers: Vec<UnixStream>,
    names: Vec<(String, i32)>,
    actor_used: std::cell::Cell<bool>,
}
impl CaptureWriterLease {
    pub(crate) fn for_account(uid: u32, gid: u32, home: &File) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 || uid == 0 {
            return Err(Error::Unsupported);
        }
        let home = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))?;
        let home = home.to_str().ok_or(Error::Unsupported)?;
        if home.ends_with(" (deleted)") {
            return Err(Error::UnsafeBoundary);
        }
        Self::acquire(Namespace::native(uid, gid, false)?, home)
    }
    fn acquire(namespace: Namespace, home: &str) -> Result<Self> {
        let lock = namespace.open_lock(false)?;
        let names = namespace.sockets()?;
        let mut lease = Self {
            namespace,
            lock: Some(lock),
            writers: Vec::new(),
            names,
            actor_used: std::cell::Cell::new(false),
        };
        for (name, pid) in &lease.names {
            let mut stream = UnixStream::connect(lease.namespace.path(name))?;
            let peer = socket_peercred(&stream)?;
            if peer.uid.as_raw() != lease.namespace.uid || peer.pid.as_raw_pid() != *pid {
                return Err(Error::UnsafeBoundary);
            }
            stream.set_read_timeout(Some(TIMEOUT))?;
            stream.set_write_timeout(Some(TIMEOUT))?;
            stream.write_all(&hello(home))?;
            let mut ready = [0];
            stream.read_exact(&mut ready)?;
            if ready != *b"Q" {
                return Err(Error::WriterUnavailable);
            }
            lease.writers.push(stream);
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match fs::flock(
                lease.lock.as_ref().ok_or(Error::UnsafeBoundary)?,
                FlockOperation::NonBlockingLockExclusive,
            ) {
                Ok(()) => break,
                Err(rustix::io::Errno::WOULDBLOCK) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(error) => return Err(error.into()),
            }
        }
        lease.check()?;
        Ok(lease)
    }
    pub(crate) fn check(&self) -> Result<()> {
        self.namespace.check_lock(self.lock.as_ref().ok_or(Error::WriterUnavailable)?)?;
        if self.namespace.sockets()? != self.names {
            return Err(Error::WriterUnavailable);
        }
        for stream in &self.writers {
            let mut stream = stream;
            stream.write_all(b"P")?;
            let mut answer = [0];
            stream.read_exact(&mut answer)?;
            if answer != *b"V" {
                return Err(Error::WriterUnavailable);
            }
        }
        Ok(())
    }
    pub(crate) fn release(&mut self) -> Result<()> {
        self.lock.take();
        self.writers.clear();
        Ok(())
    }
    fn wallpaper_stream(&self) -> Result<UnixStream> {
        self.check()?;
        let actors = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| name.starts_with("ryogami-"))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [index] = actors.as_slice() else { return Err(Error::WriterUnavailable) };
        let stream = self.writers[*index].try_clone()?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        Ok(stream)
    }
    pub(crate) fn wallpaper_actor(&self) -> Result<crate::wallpaper_protocol::ActorIdentity> {
        let mut stream = self.wallpaper_stream()?;
        stream.write_all(b"I")?;
        let identity = read_actor_wire(&mut stream)?;
        self.check()?;
        Ok(identity)
    }
    pub(crate) fn wallpaper_players(
        &self,
    ) -> Result<Vec<crate::wallpaper_protocol::ActorIdentity>> {
        let mut stream = self.wallpaper_stream()?;
        stream.write_all(b"J")?;
        let players = read_actor_wire(&mut stream)?;
        self.check()?;
        Ok(players)
    }
    pub(crate) fn apply_wallpaper(
        &self,
        call: &crate::wallpaper_protocol::ActorCall,
    ) -> Result<Vec<u8>> {
        if self.actor_used.get() {
            return Err(Error::Unsupported);
        }
        let expected = self.wallpaper_actor()?;
        call.validate(&expected).map_err(|_| Error::Unsupported)?;
        let mut stream = self.wallpaper_stream()?;
        self.actor_used.set(true);
        stream.set_read_timeout(Some(Duration::from_secs(45)))?;
        stream.write_all(b"A")?;
        write_actor_wire(&mut stream, call)?;
        let reply = read_actor_wire(&mut stream)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        self.check()?;
        Ok(reply)
    }
}
impl Drop for CaptureWriterLease {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Own a fixed supported upstream process and all of its descendants. The
/// wrapper never runs as root and forwards no commands through the helper.
pub fn run_writer(kind: WriterKind, arguments: &[std::ffi::OsString]) -> Result<i32> {
    if kind == WriterKind::HubBackend && !crate::writer_command_policy::hub_query(arguments) {
        return Err(Error::Unsupported);
    }
    let uid = rustix::process::geteuid().as_raw();
    if uid == 0 {
        return Err(Error::Unsupported);
    }
    if kind == WriterKind::Ryogami
        && !(arguments.is_empty() || arguments == [std::ffi::OsString::from("daemon")])
    {
        // CLI is a broker client, never a second daemon/actor or an unreviewed
        // native worker. Its exact frame crosses the same picker boundary.
        return crate::wallpaper_proxy::run_client(arguments).map_err(Error::from);
    }
    let namespace = Namespace::native(uid, rustix::process::getegid().as_raw(), true)?;
    let mut command = kind.command();
    command.args(arguments);
    install_signal_forwarding()?;
    run_using(namespace, kind, command, 0)
}

struct Endpoint {
    namespace: Namespace,
    name: String,
    inode: (u64, u64),
    listener: UnixListener,
    clean: bool,
}
impl Endpoint {
    fn bind(namespace: Namespace, kind: WriterKind) -> Result<Self> {
        let name = format!("{}-{}.sock", kind.name(), std::process::id());
        let listener = UnixListener::bind(namespace.path(&name))?;
        fs::chmodat(&namespace.root, name.as_str(), Mode::from_raw_mode(0o666), AtFlags::empty())?;
        let stat = fs::statat(&namespace.root, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
        listener.set_nonblocking(true)?;
        Ok(Self { namespace, name, inode: (stat.st_dev, stat.st_ino), listener, clean: false })
    }
    fn serve(
        &self,
        mut stream: UnixStream,
        lock: &File,
        helper_uid: u32,
        actor: Option<&crate::wallpaper_channel::NativeActor>,
    ) -> Result<()> {
        let peer = socket_peercred(&stream)?;
        if peer.uid.as_raw() != helper_uid {
            return Err(Error::UnsafeBoundary);
        }
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let home = std::env::var("HOME").map_err(|_| Error::Unsupported)?;
        let expected = hello(&home);
        let mut request = vec![0; expected.len()];
        stream.read_exact(&mut request)?;
        if request != expected {
            return Err(Error::Unsupported);
        }
        self.namespace.check_lock(lock)?;
        // Nested wrappers coordinate their own subtrees and must remain able
        // to answer the root reader. Every registered wrapper still holds a
        // shared lock until its own subtree is stopped, so exclusive capture
        // cannot proceed by omitting a nested participant.
        let excluded = self
            .namespace
            .sockets()?
            .iter()
            .map(|(_, pid)| *pid)
            .filter(|pid| *pid != std::process::id() as i32)
            .collect();
        let stops =
            actor.map(|actor| actor.preserved_player_stops()).transpose()?.unwrap_or_default();
        let mut freeze = crate::writer_freeze::WriterFreeze::for_children(&home, excluded, stops)?;
        fs::flock(lock, FlockOperation::Unlock)?;
        // Even a failed acknowledgement must reacquire shared ownership before
        // releasing child stops, so a concurrent capture cannot be bypassed.
        let result = (|| {
            stream.write_all(b"Q")?;
            stream.set_read_timeout(Some(Duration::from_millis(100)))?;
            let mut request = [0];
            let mut actor_used = false;
            loop {
                if SHUTDOWN_SIGNAL.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                    return Err(Error::WriterUnavailable);
                }
                match stream.read(&mut request) {
                    Ok(0) => return Ok(()),
                    Ok(1) if request == *b"P" => {
                        freeze.check()?;
                        self.namespace.check_lock(lock)?;
                        stream.write_all(b"V")?;
                    }
                    Ok(1) if request == *b"I" => {
                        freeze.check()?;
                        let actor = actor.ok_or(Error::Unsupported)?;
                        write_actor_wire(&mut stream, &actor.identity()?)?;
                    }
                    Ok(1) if request == *b"J" => {
                        freeze.check()?;
                        let actor = actor.ok_or(Error::Unsupported)?;
                        write_actor_wire(&mut stream, &actor.player_identities()?)?;
                    }
                    Ok(1) if request == *b"A" => {
                        if actor_used {
                            return Err(Error::Unsupported);
                        }
                        let actor = actor.ok_or(Error::Unsupported)?;
                        stream.set_read_timeout(Some(TIMEOUT))?;
                        let call: crate::wallpaper_protocol::ActorCall =
                            read_actor_wire(&mut stream)?;
                        call.validate_for_home(&actor.identity()?, &home)
                            .map_err(|_| Error::Unsupported)?;
                        freeze.check()?;
                        self.namespace.check_lock(lock)?;
                        let (_, action) = crate::wallpaper_protocol::parse_change(&call.frame)
                            .map_err(|_| Error::Unsupported)?;
                        if matches!(action, crate::wallpaper_protocol::WallpaperChange::Pause(_))
                            && actor.player_identities()? != call.player_pins
                        {
                            return Err(Error::WriterUnavailable);
                        }
                        let execution = actor.execution(&call)?;
                        // This lease accepts exactly one approved native call;
                        // it cannot be replayed even if the application fails.
                        actor_used = true;
                        freeze.run_actor(execution)?;
                        let applied = actor.apply(&call);
                        // Always refreeze before interpreting an application
                        // error or acknowledging any response to the helper.
                        let stopped = freeze.refreeze();
                        stopped?;
                        freeze.check()?;
                        actor.identity()?;
                        self.namespace.check_lock(lock)?;
                        let applied = applied?;
                        if let crate::wallpaper_protocol::WallpaperChange::Pause(pause) = action {
                            if actor.player_identities()? != call.player_pins {
                                return Err(Error::WriterUnavailable);
                            }
                            let value: serde_json::Value =
                                serde_json::from_slice(&applied).map_err(|_| Error::Unsupported)?;
                            if value.get("error").is_none() {
                                let effective = value["result"]["effective_paused"]
                                    .as_bool()
                                    .ok_or(Error::Unsupported)?;
                                let pins: Vec<crate::wallpaper_protocol::ActorIdentity> =
                                    serde_json::from_value(value["result"]["player_pins"].clone())
                                        .map_err(|_| Error::Unsupported)?;
                                if pins != call.player_pins
                                    || value["result"]["paused"].as_bool() != Some(pause.paused)
                                {
                                    return Err(Error::Unsupported);
                                }
                                freeze.playback_disposition(pins, effective)?;
                            }
                        }
                        write_actor_wire(&mut stream, &applied)?;
                        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
                    }
                    Ok(_) => return Err(Error::Unsupported),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        })();
        // A shutdown/cancellation invalidates the reader before waiting for its
        // exclusive lock. Children remain stopped until that lock is released.
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let locked = fs::flock(lock, FlockOperation::LockShared).map_err(Error::from);
        let resumed = freeze.release();
        locked?;
        resumed?;
        result
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if self.clean
            && let Ok(stat) =
                fs::statat(&self.namespace.root, self.name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            && (stat.st_dev, stat.st_ino) == self.inode
        {
            let _ = fs::unlinkat(&self.namespace.root, self.name.as_str(), AtFlags::empty());
        }
    }
}

fn run_using(
    namespace: Namespace,
    kind: WriterKind,
    mut command: Command,
    helper_uid: u32,
) -> Result<i32> {
    become_subreaper()?;
    let lock = namespace.open_lock(true)?;
    fs::flock(&lock, FlockOperation::LockShared)?;
    let mut endpoint = Endpoint::bind(namespace, kind)?;
    let native_control = if kind == WriterKind::Ryogami {
        let (control, native) = UnixStream::pair()?;
        command
            .env("RYOKU_WALLPAPER_BROKER_FD", "0")
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(native)));
        Some(control)
    } else {
        None
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            endpoint.clean = true;
            return Err(error.into());
        }
    };
    let actor = if let Some(control) = native_control {
        match crate::wallpaper_channel::NativeActor::new(
            control,
            child.id(),
            endpoint.namespace.uid,
        ) {
            Ok(actor) => Some(Arc::new(actor)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    } else {
        None
    };
    let _proxy = if let Some(actor) = &actor {
        match crate::wallpaper_proxy::ProxyServer::start(
            Arc::clone(actor),
            endpoint.namespace.uid,
            endpoint.namespace.gid,
        ) {
            Ok(proxy) => Some(proxy),
            Err(error) => {
                actor.shutdown();
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    } else {
        None
    };
    let mut status = None;
    loop {
        let signal = SHUTDOWN_SIGNAL.swap(0, std::sync::atomic::Ordering::Relaxed);
        if signal != 0 {
            forward_signal(signal)?;
        }
        if status.is_none() {
            status = child.try_wait()?;
        }
        if let Some(status) = status {
            reap_orphans()?;
            if !has_children()? {
                endpoint.clean = true;
                return Ok(status.code().unwrap_or(1));
            }
        }
        match endpoint.listener.accept() {
            Ok((stream, _)) => {
                let result = endpoint.serve(stream, &lock, helper_uid, actor.as_deref());
                #[cfg(test)]
                if let Err(error) = &result {
                    eprintln!("fixture writer lease failed: {error}");
                }
                drop(result);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) => return Err(error.into()),
        }
        endpoint.namespace.check_lock(&lock)?;
    }
}

const MAX_ACTOR_WIRE: usize = crate::wallpaper_protocol::MAX_REQUEST_BYTES * 4 + 4096;
fn read_actor_wire<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_ACTOR_WIRE {
        return Err(Error::Unsupported);
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Unsupported)
}
fn write_actor_wire(stream: &mut UnixStream, value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Unsupported)?;
    if bytes.len() > MAX_ACTOR_WIRE {
        return Err(Error::Unsupported);
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    Ok(())
}
extern "C" fn remember_signal(signal: libc::c_int) {
    SHUTDOWN_SIGNAL.store(signal, std::sync::atomic::Ordering::Relaxed);
}
#[allow(unsafe_code)] // Process-local signal handlers only store a lock-free scalar.
fn install_signal_forwarding() -> Result<()> {
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: zero is a valid initial sigaction; the handler has the required ABI.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = remember_signal as *const () as usize;
        // SAFETY: action is initialized and lives for the entire call; no old pointer.
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}
#[allow(unsafe_code)] // Send only a received termination signal to this wrapper's children.
fn forward_signal(signal: i32) -> Result<()> {
    for pid in crate::writer_freeze::owned_child_processes(std::process::id() as i32)? {
        // SAFETY: positive PID discovered as our same-account descendant; scalar signal only.
        if unsafe { libc::kill(pid, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error.into());
            }
        }
    }
    Ok(())
}
fn has_children() -> Result<bool> {
    Ok(!crate::writer_freeze::owned_child_processes(std::process::id() as i32)?.is_empty())
}
fn hello(home: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let hash: String =
        Sha256::digest(home.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("RYOKU-CAPTURE-1 {hash}\n").into_bytes()
}
#[allow(unsafe_code)] // Reap only this wrapper's exited natural children, outside trace sessions.
fn reap_orphans() -> Result<()> {
    loop {
        let mut status = 0;
        // SAFETY: status is valid for waitpid; -1 selects only this process's children.
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid == 0 {
            return Ok(());
        }
        if pid < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                return Ok(());
            }
            return Err(error.into());
        }
    }
}
#[allow(unsafe_code)] // A process-local lifecycle flag; no identity or privilege change.
fn become_subreaper() -> Result<()> {
    // SAFETY: PR_SET_CHILD_SUBREAPER accepts one boolean scalar argument.
    let result = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "writer_protocol_tests.rs"]
mod tests;
