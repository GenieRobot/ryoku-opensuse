//! Metadata restoration on caller-owned private staging inodes.
//!
//! The caller must supply a writable, exclusively held inode in its protected
//! staging area for regular files. The offline writer can finalize an enrolled
//! live directory after all child operations, while writers remain stopped;
//! failures must leave recovery active. This primitive does not
//! authorize restore, checkpoint current state, publish names, advance a
//! journal, or freeze writers. Apply directory metadata after preparing children.
//! Failures can leave the staging inode partly modified; never publish it.

use std::{
    fs::File,
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
};

use rustix::{
    fs::{
        AtFlags, FileType, Gid, Mode, Timespec, Timestamps, UTIME_OMIT, Uid, XattrFlags, chownat,
        fchmod, fchown, fgetxattr, fremovexattr, fsetxattr, fsync, futimens, lgetxattr,
        lremovexattr, lsetxattr, readlinkat, statat, utimensat,
    },
    io::Errno,
};

use crate::{
    EntryKind, EntryMetadata, RelativePath, XattrName,
    configuration_stage::StagedEntry,
    metadata::{AclPayload, MAX_METADATA_VALUE_BYTES},
};

#[derive(Debug)]
pub enum InodeMetadataError {
    UnsupportedKind,
    InodeMismatch,
    InvalidMetadata,
    ReadbackMismatch,
    Io(std::io::Error),
    Filesystem(Errno),
}

impl std::fmt::Display for InodeMetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedKind => {
                f.write_str("metadata preparation requires a regular file or directory")
            }
            Self::InodeMismatch => f.write_str("staging inode does not match the prepared entry"),
            Self::InvalidMetadata => {
                f.write_str("prepared inode metadata is invalid or unrepresentable")
            }
            Self::ReadbackMismatch => {
                f.write_str("restored inode metadata does not match on readback")
            }
            Self::Io(error) => write!(f, "staging inode I/O failed: {error}"),
            Self::Filesystem(error) => {
                write!(f, "staging inode metadata operation failed: {error}")
            }
        }
    }
}
impl std::error::Error for InodeMetadataError {}
impl From<std::io::Error> for InodeMetadataError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<Errno> for InodeMetadataError {
    fn from(error: Errno) -> Self {
        Self::Filesystem(error)
    }
}

/// Restore and sync exact metadata from an immutable verified stage. No paths
/// are opened. Regular files must still have only one link; create declared
/// hard-link aliases after this step, within the protected staging area.
/// The descriptor must support reads as well as metadata writes.
pub fn prepare_inode_metadata(file: &File, entry: &StagedEntry) -> Result<(), InodeMetadataError> {
    let before = file.metadata()?;
    let (metadata, directory) = match &entry.record().kind {
        EntryKind::RegularFile { metadata, size, .. } => {
            if !before.is_file() || before.nlink() != 1 || before.len() != *size {
                return Err(InodeMetadataError::InodeMismatch);
            }
            let expected = entry.content().ok_or(InodeMetadataError::InvalidMetadata)?;
            let mut buffer = [0_u8; 8192];
            let mut offset = 0;
            for chunk in expected.chunks(buffer.len()) {
                file.read_exact_at(&mut buffer[..chunk.len()], offset)?;
                if buffer[..chunk.len()] != *chunk {
                    return Err(InodeMetadataError::InodeMismatch);
                }
                offset += chunk.len() as u64;
            }
            if offset != *size {
                return Err(InodeMetadataError::InodeMismatch);
            }
            (metadata, false)
        }
        EntryKind::Directory { metadata } => {
            if !before.is_dir() {
                return Err(InodeMetadataError::InodeMismatch);
            }
            (metadata, true)
        }
        _ => return Err(InodeMetadataError::UnsupportedKind),
    };
    apply_verified_metadata(file, metadata, directory, entry.acl(), entry.xattrs().collect())
}

/// Apply metadata after the caller has independently verified the entire
/// staging inode's type and bytes. This is private to recovery implementations;
/// it neither chooses a destination nor publishes an inode.
pub(crate) fn apply_verified_metadata(
    file: &File,
    metadata: &EntryMetadata,
    directory: bool,
    acl: Option<&AclPayload>,
    attributes: Vec<(XattrName, &[u8])>,
) -> Result<(), InodeMetadataError> {
    let before = file.metadata()?;
    if before.is_dir() != directory || (!directory && (!before.is_file() || before.nlink() != 1)) {
        return Err(InodeMetadataError::InodeMismatch);
    }
    if metadata.mode & !0o7777 != 0
        || metadata.uid == u32::MAX
        || metadata.gid == u32::MAX
        || (!directory && acl.is_some_and(|acl| acl.default.is_some()))
    {
        return Err(InodeMetadataError::InvalidMetadata);
    }
    let seconds = metadata
        .mtime_unix_ns
        .div_euclid(1_000_000_000)
        .try_into()
        .map_err(|_| InodeMetadataError::InvalidMetadata)?;
    let nanos = metadata.mtime_unix_ns.rem_euclid(1_000_000_000) as _;
    let label = attributes
        .iter()
        .find_map(|(name, value)| (*name == XattrName::SelinuxLabel).then_some(*value));

    // chown may clear set-ID bits. chmod precedes ACL restoration because
    // chmod after setting an ACL would rewrite its mask. Readback catches
    // incompatible mode/ACL combinations instead of silently changing either.
    if before.uid() != metadata.uid || before.gid() != metadata.gid {
        fchown(file, Some(Uid::from_raw(metadata.uid)), Some(Gid::from_raw(metadata.gid)))?;
    }
    fchmod(file, Mode::from_bits_retain(metadata.mode))?;
    replace_attribute(file, "system.posix_acl_access", acl.and_then(|acl| acl.access.as_deref()))?;
    if directory {
        replace_attribute(
            file,
            "system.posix_acl_default",
            acl.and_then(|acl| acl.default.as_deref()),
        )?;
    }
    replace_attribute(file, XattrName::SelinuxLabel.as_str(), label)?;
    futimens(
        file,
        &Timestamps {
            last_access: Timespec { tv_sec: 0, tv_nsec: UTIME_OMIT },
            last_modification: Timespec { tv_sec: seconds, tv_nsec: nanos },
        },
    )?;
    fsync(file)?;
    let after = file.metadata()?;
    if after.uid() != metadata.uid
        || after.gid() != metadata.gid
        || after.mode() & 0o7777 != metadata.mode
        || i128::from(after.mtime()) * 1_000_000_000 + i128::from(after.mtime_nsec())
            != metadata.mtime_unix_ns
    {
        return Err(InodeMetadataError::ReadbackMismatch);
    }
    verify_attribute(file, "system.posix_acl_access", acl.and_then(|acl| acl.access.as_deref()))?;
    if directory {
        verify_attribute(
            file,
            "system.posix_acl_default",
            acl.and_then(|acl| acl.default.as_deref()),
        )?;
    }
    verify_attribute(file, XattrName::SelinuxLabel.as_str(), label)?;
    Ok(())
}

fn read_attribute(file: &File, name: &str) -> Result<Option<Vec<u8>>, InodeMetadataError> {
    let mut value = vec![0; MAX_METADATA_VALUE_BYTES];
    match fgetxattr(file, name, &mut value) {
        Ok(length) => {
            value.truncate(length);
            Ok(Some(value))
        }
        Err(Errno::NODATA | Errno::OPNOTSUPP) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn replace_attribute(
    file: &File,
    name: &str,
    value: Option<&[u8]>,
) -> Result<(), InodeMetadataError> {
    match value {
        Some(value) => fsetxattr(file, name, value, XattrFlags::empty())?,
        None => {
            // A missing attribute needs no privileged removal. In particular,
            // removing absent security.selinux can otherwise fail for nonroot.
            if read_attribute(file, name)?.is_some() {
                fremovexattr(file, name)?;
            }
        }
    }
    Ok(())
}

fn verify_attribute(
    file: &File,
    name: &str,
    expected: Option<&[u8]>,
) -> Result<(), InodeMetadataError> {
    if read_attribute(file, name)?.as_deref() != expected {
        return Err(InodeMetadataError::ReadbackMismatch);
    }
    Ok(())
}

/// Prepare a symlink in a caller-owned, exclusively held private staging
/// directory. `leaf` must be one component. Every operation treats the final
/// link itself as the target; even an absolute link target is never followed.
/// Live configuration parents and concurrent writers are unsupported.
pub fn prepare_symlink_metadata(
    parent: &File,
    leaf: &RelativePath,
    entry: &StagedEntry,
) -> Result<(), InodeMetadataError> {
    if leaf.as_str().contains('/') {
        return Err(InodeMetadataError::InvalidMetadata);
    }
    let EntryKind::Symlink { metadata, target } = &entry.record().kind else {
        return Err(InodeMetadataError::UnsupportedKind);
    };
    if metadata.mode != 0o777
        || metadata.uid == u32::MAX
        || metadata.gid == u32::MAX
        || entry.acl().is_some()
    {
        return Err(InodeMetadataError::InvalidMetadata);
    }
    let seconds = metadata
        .mtime_unix_ns
        .div_euclid(1_000_000_000)
        .try_into()
        .map_err(|_| InodeMetadataError::InvalidMetadata)?;
    let nanos = metadata.mtime_unix_ns.rem_euclid(1_000_000_000) as _;
    let before = statat(parent, leaf.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::Symlink
        || readlinkat(parent, leaf.as_str(), Vec::new())?.as_bytes() != target.as_bytes()
    {
        return Err(InodeMetadataError::InodeMismatch);
    }
    if before.st_uid != metadata.uid || before.st_gid != metadata.gid {
        chownat(
            parent,
            leaf.as_str(),
            Some(Uid::from_raw(metadata.uid)),
            Some(Gid::from_raw(metadata.gid)),
            AtFlags::SYMLINK_NOFOLLOW,
        )?;
    }
    // The parent descriptor is pinned and the single leaf is exclusively
    // owned by this staging operation. Linux l*xattr never follows the leaf.
    let path = format!("/proc/self/fd/{}/{}", parent.as_raw_fd(), leaf.as_str());
    let attributes: Vec<_> = entry.xattrs().collect();
    let label = attributes
        .iter()
        .find_map(|(name, value)| (*name == XattrName::SelinuxLabel).then_some(*value));
    match label {
        Some(value) => {
            lsetxattr(&path, XattrName::SelinuxLabel.as_str(), value, XattrFlags::empty())?
        }
        None => {
            if read_link_attribute(&path, XattrName::SelinuxLabel.as_str())?.is_some() {
                lremovexattr(&path, XattrName::SelinuxLabel.as_str())?;
            }
        }
    }
    utimensat(
        parent,
        leaf.as_str(),
        &Timestamps {
            last_access: Timespec { tv_sec: 0, tv_nsec: UTIME_OMIT },
            last_modification: Timespec { tv_sec: seconds, tv_nsec: nanos },
        },
        AtFlags::SYMLINK_NOFOLLOW,
    )?;
    fsync(parent)?;
    let after = statat(parent, leaf.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
    if after.st_dev != before.st_dev
        || after.st_ino != before.st_ino
        || after.st_uid != metadata.uid
        || after.st_gid != metadata.gid
        || after.st_mode & 0o7777 != metadata.mode
        || i128::from(after.st_mtime) * 1_000_000_000 + i128::from(after.st_mtime_nsec)
            != metadata.mtime_unix_ns
        || readlinkat(parent, leaf.as_str(), Vec::new())?.as_bytes() != target.as_bytes()
        || read_link_attribute(&path, XattrName::SelinuxLabel.as_str())?.as_deref() != label
        || read_link_attribute(&path, "system.posix_acl_access")?.is_some()
        || read_link_attribute(&path, "system.posix_acl_default")?.is_some()
    {
        return Err(InodeMetadataError::ReadbackMismatch);
    }
    Ok(())
}

fn read_link_attribute(path: &str, name: &str) -> Result<Option<Vec<u8>>, InodeMetadataError> {
    let mut value = vec![0; MAX_METADATA_VALUE_BYTES];
    match lgetxattr(path, name, &mut value) {
        Ok(length) => {
            value.truncate(length);
            Ok(Some(value))
        }
        Err(Errno::NODATA | Errno::OPNOTSUPP) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
