//! Create exact destination-local staging inodes with account ownership at the
//! creation syscall, avoiding a root-owned crash window before later chown.
//! A dedicated short-lived Linux thread drops its own credentials permanently;
//! the parent and all other threads retain theirs. No caller closure runs there.
use crate::installed_capture::CaptureAccount;
use rustix::{
    fs::{self, FileType, Mode, OFlags, ResolveFlags},
    io::Errno,
};
use std::fs::File;
#[derive(Clone, Copy)]
pub(crate) enum Kind<'a> {
    File,
    Directory,
    Symlink(&'a str),
}
pub(crate) fn create(
    parent: &File,
    leaf: &str,
    account: &CaptureAccount,
    kind: Kind<'_>,
) -> Result<File, Errno> {
    if account.uid == 0
        || leaf.is_empty()
        || leaf == "."
        || leaf == ".."
        || leaf.contains('/')
        || leaf.as_bytes().contains(&0)
    {
        return Err(Errno::INVAL);
    }
    if let Kind::Symlink(target) = kind {
        crate::RelativePath::new(target).map_err(|_| Errno::INVAL)?;
    }
    let before = fs::fstat(parent)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::Directory
        || before.st_uid != account.uid
        || before.st_gid != account.gid
        || before.st_mode & 0o7022 != 0
    {
        return Err(Errno::PERM);
    }
    let caller_uid = rustix::process::geteuid();
    let caller_gid = rustix::process::getegid();
    if caller_uid.as_raw() != 0
        && (caller_uid.as_raw() != account.uid || caller_gid.as_raw() != account.gid)
    {
        return Err(Errno::PERM);
    }
    let created = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("ryoku-owned-inode".into())
            .spawn_scoped(scope, || {
                if rustix::process::geteuid().as_raw() == 0 {
                    rustix::thread::set_thread_groups(&[])?;
                    rustix::thread::set_thread_gid(fs::Gid::from_raw(account.gid))?;
                    rustix::thread::set_thread_uid(fs::Uid::from_raw(account.uid))?;
                }
                if rustix::process::geteuid().as_raw() != account.uid
                    || rustix::process::getegid().as_raw() != account.gid
                {
                    return Err(Errno::PERM);
                }
                let file = match kind {
                    Kind::File => File::from(fs::openat(
                        parent,
                        leaf,
                        OFlags::RDWR
                            | OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::NOFOLLOW
                            | OFlags::CLOEXEC,
                        Mode::from_raw_mode(0o600),
                    )?),
                    Kind::Symlink(target) => {
                        fs::symlinkat(target, parent, leaf)?;
                        File::from(fs::openat(
                            parent,
                            leaf,
                            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        )?)
                    }
                    Kind::Directory => {
                        fs::mkdirat(parent, leaf, Mode::from_raw_mode(0o700))?;
                        File::from(fs::openat2(
                            parent,
                            leaf,
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                            Mode::empty(),
                            ResolveFlags::BENEATH
                                | ResolveFlags::NO_SYMLINKS
                                | ResolveFlags::NO_XDEV,
                        )?)
                    }
                };
                let mode = match kind {
                    Kind::File => 0o600,
                    Kind::Directory => 0o700,
                    Kind::Symlink(_) => 0o777,
                };
                if !matches!(kind, Kind::Symlink(_)) {
                    fs::fchmod(&file, Mode::from_raw_mode(mode))?;
                }
                fs::fsync(parent)?;
                let actual = fs::fstat(&file)?;
                if actual.st_uid != account.uid
                    || actual.st_gid != account.gid
                    || actual.st_dev != before.st_dev
                    || actual.st_mode & 0o7777 != mode
                {
                    return Err(Errno::PERM);
                }
                Ok(file)
            })
            .map_err(|_| Errno::AGAIN)?
            .join()
            .map_err(|_| Errno::IO)
    })?;
    // Linux credential calls above are deliberately thread-specific. Refuse
    // any unexpected caller-identity change before returning a usable inode.
    if rustix::process::geteuid() != caller_uid || rustix::process::getegid() != caller_gid {
        return Err(Errno::PERM);
    }
    created
}
