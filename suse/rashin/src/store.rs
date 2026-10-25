//! Descriptor-relative, private, durable state. A process lifetime flock keeps
//! daemon restart/replay and command-line clients from losing each other's data.
use crate::{Error, Result};
use rustix::fs::{self, FileType, FlockOperation, Mode, OFlags, ResolveFlags};
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, net::UnixListener},
    },
    path::Path,
};

pub const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);

pub struct Store {
    directory: File,
    _lock: File,
    uid: u32,
}

impl Store {
    /// A fresh browser credential per daemon lifetime. Reading the file is an
    /// explicit owner action; it is never printed in a URL or server log.
    pub fn browser_token(&self) -> Result<String> {
        let mut random = [0; 32];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(|_| Error::Storage)?;
        let token = random.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let mut file = File::from(
            fs::openat2(
                &self.directory,
                "browser.token",
                OFlags::WRONLY | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o600),
                RESOLVE,
            )
            .map_err(|_| Error::UnsafeState)?,
        );
        validate(&file, self.uid)?;
        file.set_len(0).map_err(|_| Error::Storage)?;
        file.write_all(token.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Storage)?;
        self.directory.sync_all().map_err(|_| Error::Storage)?;
        Ok(token)
    }
    pub fn listener(&self) -> Result<UnixListener> {
        match fs::statat(&self.directory, "rashin.sock", fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat)
                if FileType::from_raw_mode(stat.st_mode) == FileType::Socket
                    && stat.st_uid == self.uid =>
            {
                fs::unlinkat(&self.directory, "rashin.sock", fs::AtFlags::empty())
                    .map_err(|_| Error::Storage)?;
            }
            Err(rustix::io::Errno::NOENT) => (),
            _ => return Err(Error::UnsafeState),
        }
        let path = format!("/proc/self/fd/{}/rashin.sock", self.directory.as_raw_fd());
        let listener = UnixListener::bind(&path).map_err(|_| Error::Storage)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::Storage)?;
        Ok(listener)
    }
    /// The directory must already exist with owner-only mode. Opening never
    /// follows a symlink, including ancestors, and never repairs unsafe modes.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::UnsafeState);
        }
        let root = File::open("/").map_err(|_| Error::Storage)?;
        let directory = File::from(
            fs::openat2(
                &root,
                path.strip_prefix("/").map_err(|_| Error::UnsafeState)?,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                RESOLVE,
            )
            .map_err(|_| Error::UnsafeState)?,
        );
        let uid = rustix::process::geteuid().as_raw();
        let stat = fs::fstat(&directory).map_err(|_| Error::Storage)?;
        if stat.st_uid != uid || stat.st_mode & 0o7777 != 0o700 {
            return Err(Error::UnsafeState);
        }
        let lock = File::from(
            fs::openat2(
                &directory,
                "daemon.lock",
                OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o600),
                RESOLVE,
            )
            .map_err(|_| Error::UnsafeState)?,
        );
        validate(&lock, uid)?;
        fs::flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(|_| Error::Busy)?;
        let store = Self { directory, _lock: lock, uid };
        // A staged file is never promoted after a crash. It is safe to discard
        // only under the exclusive daemon lease.
        match fs::unlinkat(&store.directory, "state.next", fs::AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => (),
            Err(_) => return Err(Error::Storage),
        }
        Ok(store)
    }

    pub(crate) fn identity(&self) -> Result<(u64, u64)> {
        let stat = fs::fstat(&self.directory).map_err(|_| Error::Storage)?;
        Ok((stat.st_dev, stat.st_ino))
    }

    pub fn read(&self) -> Result<Option<Vec<u8>>> {
        let file = match fs::openat2(
            &self.directory,
            "state.json",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            RESOLVE,
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(Error::UnsafeState),
        };
        validate(&file, self.uid)?;
        let mut bytes = Vec::new();
        file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes).map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(Error::Limit);
        }
        Ok(Some(bytes))
    }

    pub fn save(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err(Error::Limit);
        }
        // Validate the destination before replacement, including hard links.
        self.read()?;
        let mut next = File::from(
            fs::openat2(
                &self.directory,
                "state.next",
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
                RESOLVE,
            )
            .map_err(|_| Error::UnsafeState)?,
        );
        let result = (|| {
            validate(&next, self.uid)?;
            next.write_all(bytes).map_err(|_| Error::Storage)?;
            next.sync_all().map_err(|_| Error::Storage)?;
            fs::renameat(&self.directory, "state.next", &self.directory, "state.json")
                .map_err(|_| Error::Storage)?;
            self.directory.sync_all().map_err(|_| Error::Storage)
        })();
        if result.is_err() {
            let _ = fs::unlinkat(&self.directory, "state.next", fs::AtFlags::empty());
        }
        result
    }
}

fn validate(file: &File, uid: u32) -> Result<()> {
    let stat = fs::fstat(file).map_err(|_| Error::Storage)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != uid
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(Error::UnsafeState);
    }
    Ok(())
}
