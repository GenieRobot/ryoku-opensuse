//! Metadata restoration on caller-owned private staging inodes.
//!
//! The caller must supply a writable, exclusively held inode in its protected
//! staging area, never a live configuration target. This primitive does not
//! authorize restore, checkpoint current state, publish names, advance a
//! journal, or freeze writers. Apply directory metadata after preparing children.
//! Failures can leave the staging inode partly modified; never publish it.

use std::{
    fs::File,
    os::unix::fs::{FileExt, MetadataExt},
};

use rustix::{
    fs::{
        Gid, Mode, Timespec, Timestamps, UTIME_OMIT, Uid, XattrFlags, fchmod, fchown, fgetxattr,
        fremovexattr, fsetxattr, fsync, futimens,
    },
    io::Errno,
};

use crate::{
    EntryKind, XattrName, configuration_stage::StagedEntry, metadata::MAX_METADATA_VALUE_BYTES,
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
    let acl = entry.acl();
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
    let attributes: Vec<_> = entry.xattrs().collect();
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
