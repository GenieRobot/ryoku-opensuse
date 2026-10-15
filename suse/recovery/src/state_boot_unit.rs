//! Fixed-path, no-replace publication of the created state volume's boot unit.
use crate::state_provision::{STATE_MOUNT_UNIT, StateProvisionError as Error};
use rustix::fs::{
    FileType, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, fstat, fsync, openat2, renameat_with,
};
use rustix::io::Errno;
use std::{
    fs::File,
    io::{Read, Write},
};

pub(crate) fn install(root: &File, bytes: &[u8]) -> Result<(), Error> {
    let directory = unit_directory(root)?;
    publish(&directory, bytes, 0, 0)
}

fn unit_directory(root: &File) -> Result<File, Error> {
    let mut directory = root.try_clone()?;
    for component in ["etc", "systemd", "system"] {
        directory = File::from(openat2(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?);
        let stat = fstat(&directory)?;
        if stat.st_uid != 0 || stat.st_gid != 0 || stat.st_mode & 0o022 != 0 {
            return Err(Error::UnsafeBootUnit);
        }
    }
    Ok(directory)
}

/// Read-only check: neither publish a missing unit nor recreate enablement.
pub(crate) fn verify_enabled(root: &File, bytes: &[u8]) -> Result<(), Error> {
    let directory = unit_directory(root)?;
    verify_enabled_in(&directory, bytes, 0, 0)
}

fn verify_enabled_in(directory: &File, bytes: &[u8], uid: u32, gid: u32) -> Result<(), Error> {
    verify(&mut open(directory, STATE_MOUNT_UNIT)?, bytes, uid, gid)?;
    let wants = File::from(openat2(
        directory,
        "local-fs.target.wants",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let stat = fstat(&wants)?;
    if stat.st_uid != uid || stat.st_gid != gid || stat.st_mode & 0o022 != 0 {
        return Err(Error::UnsafeBootUnit);
    }
    let stat = rustix::fs::statat(&wants, STATE_MOUNT_UNIT, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Symlink
        || stat.st_uid != uid
        || stat.st_gid != gid
    {
        return Err(Error::UnsafeBootUnit);
    }
    let target = rustix::fs::readlinkat(&wants, STATE_MOUNT_UNIT, Vec::new())?;
    let relative = format!("../{STATE_MOUNT_UNIT}");
    let absolute = format!("/etc/systemd/system/{STATE_MOUNT_UNIT}");
    if target.as_bytes() != relative.as_bytes() && target.as_bytes() != absolute.as_bytes() {
        return Err(Error::ConflictingBootUnit);
    }
    Ok(())
}

fn open(directory: &File, name: &str) -> Result<File, Errno> {
    openat2(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
}

fn verify(file: &mut File, bytes: &[u8], uid: u32, gid: u32) -> Result<(), Error> {
    let stat = fstat(&*file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != uid
        || stat.st_gid != gid
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != 0o644
        || stat.st_size as usize != bytes.len()
    {
        return Err(Error::UnsafeBootUnit);
    }
    let mut actual = Vec::new();
    file.take(bytes.len() as u64 + 1).read_to_end(&mut actual)?;
    if actual != bytes {
        return Err(Error::ConflictingBootUnit);
    }
    Ok(())
}

fn publish(directory: &File, bytes: &[u8], uid: u32, gid: u32) -> Result<(), Error> {
    match open(directory, STATE_MOUNT_UNIT) {
        Ok(mut existing) => {
            verify(&mut existing, bytes, uid, gid)?;
            fsync(directory)?;
            return Ok(());
        }
        Err(Errno::NOENT) => (),
        Err(error) => return Err(error.into()),
    }
    let pending = ".ryoku-state-mount.pending";
    match openat2(
        directory,
        pending,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    ) {
        Ok(fd) => {
            fchmod(&fd, Mode::from_raw_mode(0o644))?;
            let mut file = File::from(fd);
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        Err(Errno::EXIST) => (),
        Err(error) => return Err(error.into()),
    }
    let mut pending_file = open(directory, pending)?;
    verify(&mut pending_file, bytes, uid, gid)?;
    pending_file.sync_all()?;
    renameat_with(directory, pending, directory, STATE_MOUNT_UNIT, RenameFlags::NOREPLACE)?;
    fsync(directory)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn boot_check_requires_exact_unit_and_owned_enablement_without_repair() {
        let path = std::env::temp_dir().join(format!("ryoku-boot-check-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = File::open(&path).unwrap();
        let stat = fstat(&directory).unwrap();
        let check = || verify_enabled_in(&directory, b"complete", stat.st_uid, stat.st_gid);
        assert!(check().is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        publish(&directory, b"complete", stat.st_uid, stat.st_gid).unwrap();
        assert!(check().is_err());
        let wants = path.join("local-fs.target.wants");
        fs::create_dir(&wants).unwrap();
        fs::set_permissions(&wants, fs::Permissions::from_mode(0o755)).unwrap();
        let link = wants.join(STATE_MOUNT_UNIT);
        for target in
            [format!("../{STATE_MOUNT_UNIT}"), format!("/etc/systemd/system/{STATE_MOUNT_UNIT}")]
        {
            std::os::unix::fs::symlink(target, &link).unwrap();
            check().unwrap();
            fs::remove_file(&link).unwrap();
        }
        std::os::unix::fs::symlink("../foreign.mount", &link).unwrap();
        assert!(matches!(check(), Err(Error::ConflictingBootUnit)));
        assert_eq!(fs::read_link(&link).unwrap(), std::path::Path::new("../foreign.mount"));
        fs::remove_file(&link).unwrap();
        fs::write(&link, b"foreign").unwrap();
        assert!(matches!(check(), Err(Error::UnsafeBootUnit)));
        assert_eq!(fs::read(&link).unwrap(), b"foreign");
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(format!("../{STATE_MOUNT_UNIT}"), &link).unwrap();
        fs::set_permissions(&wants, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(check(), Err(Error::UnsafeBootUnit)));
        fs::set_permissions(&wants, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(path.join(STATE_MOUNT_UNIT), b"changed!").unwrap();
        assert!(check().is_err());
        assert_eq!(fs::read(path.join(STATE_MOUNT_UNIT)).unwrap(), b"changed!");
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn publication_preserves_conflicts_and_resumes_only_complete_owned_pending_files() {
        let path = std::env::temp_dir().join(format!("ryoku-boot-unit-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = File::open(&path).unwrap();
        let stat = fstat(&directory).unwrap();
        let pending = path.join(".ryoku-state-mount.pending");
        fs::write(&pending, b"partial").unwrap();
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(publish(&directory, b"complete", stat.st_uid, stat.st_gid).is_err());
        assert_eq!(fs::read(&pending).unwrap(), b"partial");
        assert!(!path.join(STATE_MOUNT_UNIT).exists());
        fs::write(&pending, b"complete").unwrap();
        publish(&directory, b"complete", stat.st_uid, stat.st_gid).unwrap();
        publish(&directory, b"complete", stat.st_uid, stat.st_gid).unwrap();
        assert!(!pending.exists());
        assert!(publish(&directory, b"different", stat.st_uid, stat.st_gid).is_err());
        assert_eq!(fs::read(path.join(STATE_MOUNT_UNIT)).unwrap(), b"complete");
        fs::hard_link(path.join(STATE_MOUNT_UNIT), path.join("foreign-alias")).unwrap();
        assert!(matches!(
            publish(&directory, b"complete", stat.st_uid, stat.st_gid),
            Err(Error::UnsafeBootUnit)
        ));
        fs::remove_file(path.join(STATE_MOUNT_UNIT)).unwrap();
        std::os::unix::fs::symlink("foreign-alias", path.join(STATE_MOUNT_UNIT)).unwrap();
        assert!(publish(&directory, b"complete", stat.st_uid, stat.st_gid).is_err());
        assert_eq!(fs::read(path.join("foreign-alias")).unwrap(), b"complete");
        fs::remove_dir_all(path).unwrap();
    }
}
