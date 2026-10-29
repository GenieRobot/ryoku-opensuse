//! Bounded Linux capture for an administrator-declared exact file list.
//!
//! The caller supplies an already-open directory descriptor for one approved
//! root. Relative components are opened one at a time with `O_NOFOLLOW`; the
//! collector never walks a directory tree or follows a final symlink.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    os::fd::{AsFd, AsRawFd},
};

use rustix::{
    fs::{
        AtFlags, FileType, Mode, OFlags, ResolveFlags, fgetxattr, fstat, lgetxattr, openat2,
        readlinkat, statat,
    },
    io::{Errno, dup},
};
use sha2::{Digest, Sha256 as Sha256Digest};

use crate::{
    EntryKind, EntryMetadata, ExtendedMetadata, FileRecord, Identifier, RelativePath, Sha256,
    StateRoot, ValidationError,
    metadata::{AclPayload, MAX_METADATA_VALUE_BYTES},
};

/// Hard upper bounds for one exact-file capture call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureLimits {
    pub max_paths: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self { max_paths: 256, max_file_bytes: 16 * 1024 * 1024, max_total_bytes: 64 * 1024 * 1024 }
    }
}

/// Capture failure. Every error aborts the whole declared set; callers must
/// not publish a partial result.
#[derive(Debug)]
pub enum CaptureError {
    InvalidRoot,
    InvalidLimits,
    TooManyPaths,
    DuplicatePath,
    UnsupportedEntryType,
    UnsupportedMetadata(&'static str),
    MetadataTooLarge,
    FileTooLarge,
    TotalCaptureTooLarge,
    HardLinkOutsideInventory,
    ConcurrentModification,
    Filesystem(Errno),
    ContentRead(std::io::Error),
    Validation(ValidationError),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot => write!(formatter, "capture root is not an open directory"),
            Self::InvalidLimits => write!(formatter, "capture limits must be nonzero"),
            Self::TooManyPaths => write!(formatter, "declared path count exceeds capture limit"),
            Self::DuplicatePath => write!(formatter, "declared path is duplicated"),
            Self::UnsupportedEntryType => {
                write!(formatter, "entry type is not supported for capture")
            }
            Self::UnsupportedMetadata(name) => {
                write!(formatter, "required filesystem metadata is unsupported: {name}")
            }
            Self::MetadataTooLarge => write!(formatter, "extended metadata exceeds capture limit"),
            Self::FileTooLarge => write!(formatter, "file exceeds per-file capture limit"),
            Self::TotalCaptureTooLarge => write!(formatter, "capture exceeds total byte limit"),
            Self::HardLinkOutsideInventory => {
                write!(formatter, "hard-link aliases are not completely declared")
            }
            Self::ConcurrentModification => {
                write!(formatter, "entry changed while it was being captured")
            }
            Self::Filesystem(error) => write!(formatter, "filesystem capture failed: {error}"),
            Self::ContentRead(error) => write!(formatter, "file content read failed: {error}"),
            Self::Validation(error) => write!(formatter, "captured record is invalid: {error:?}"),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<ValidationError> for CaptureError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

struct Captured {
    record: FileRecord,
    device: u64,
    inode: u64,
    links: u64,
}

/// Raw metadata accompanies records only inside the protected capture path.
pub(crate) struct CapturedInventory {
    pub records: Vec<FileRecord>,
    pub metadata_payloads: BTreeMap<Sha256, Vec<u8>>,
    pub source_bytes: u64,
}

// The large-asset path raises content bounds, never metadata allocation bounds.
// This also preserves every historical 256-path metadata combination.
const MAX_CAPTURE_METADATA_BYTES: usize = 64 * 1024 * 1024;
fn extend_metadata(
    target: &mut BTreeMap<Sha256, Vec<u8>>,
    source: BTreeMap<Sha256, Vec<u8>>,
) -> Result<(), CaptureError> {
    target.extend(source);
    if target.values().map(Vec::len).sum::<usize>() > MAX_CAPTURE_METADATA_BYTES {
        return Err(CaptureError::TotalCaptureTooLarge);
    }
    Ok(())
}

pub(crate) fn capture_inventory(
    root_fd: &File,
    root: StateRoot,
    declared: &[RelativePath],
    limits: CaptureLimits,
) -> Result<CapturedInventory, CaptureError> {
    capture_inventory_with_xattrs(root_fd, root, declared, limits, &KernelXattrs)
}

#[cfg(test)]
pub(crate) fn capture_inventory_without_xattrs_for_test(
    root_fd: &File,
    root: StateRoot,
    declared: &[RelativePath],
    limits: CaptureLimits,
) -> Result<CapturedInventory, CaptureError> {
    capture_inventory_with_xattrs(root_fd, root, declared, limits, &TestNoXattrs)
}

/// Captures exactly `declared` beneath the caller-opened `root_fd`.
///
/// Missing final entries and missing intermediate directories produce absent
/// records. An intermediate symlink, special file, incomplete
/// hard-link set, unsupported ACL/xattr, or concurrent change aborts capture.
/// The root descriptor itself must already refer to the approved fixed root;
/// its path is never taken from manifest data.
pub fn capture_declared(
    root_fd: &File,
    root: StateRoot,
    declared: &[RelativePath],
    limits: CaptureLimits,
) -> Result<Vec<FileRecord>, CaptureError> {
    capture_declared_with_xattrs(root_fd, root, declared, limits, &KernelXattrs)
}

#[cfg(test)]
struct TestNoXattrs;

#[cfg(test)]
impl XattrReader for TestNoXattrs {
    fn read(&self, _fd: &File, _name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
        Ok(None)
    }

    fn read_symlink(
        &self,
        _parent: &File,
        _leaf: &str,
        _name: &'static str,
    ) -> Result<Option<Vec<u8>>, CaptureError> {
        Ok(None)
    }
}

fn capture_declared_with_xattrs(
    root_fd: &File,
    root: StateRoot,
    declared: &[RelativePath],
    limits: CaptureLimits,
    xattrs: &impl XattrReader,
) -> Result<Vec<FileRecord>, CaptureError> {
    capture_inventory_with_xattrs(root_fd, root, declared, limits, xattrs)
        .map(|inventory| inventory.records)
}

pub(crate) fn capture_inventory_with_xattrs(
    root_fd: &File,
    root: StateRoot,
    declared: &[RelativePath],
    limits: CaptureLimits,
    xattrs: &impl XattrReader,
) -> Result<CapturedInventory, CaptureError> {
    capture_inventory_bounded(
        root_fd,
        root,
        declared,
        limits,
        xattrs,
        CaptureLimits {
            max_paths: HARD_MAX_PATHS,
            max_file_bytes: HARD_MAX_FILE_BYTES,
            max_total_bytes: HARD_MAX_TOTAL_BYTES,
        },
    )
}

/// Only the code-owned Ryogami asset enrollment calls this larger streaming
/// collector. Public exact-file/preferences capture retains its original caps.
pub(crate) fn capture_ryogami_assets(
    home: &File,
    declared: &[RelativePath],
    limits: CaptureLimits,
) -> Result<CapturedInventory, CaptureError> {
    capture_inventory_bounded(
        home,
        StateRoot::StateGeneration,
        declared,
        limits,
        &KernelXattrs,
        crate::ryogami_assets::MAX_CAPTURE_LIMITS,
    )
}

fn capture_inventory_bounded(
    root_fd: &File,
    root: StateRoot,
    declared: &[RelativePath],
    limits: CaptureLimits,
    xattrs: &impl XattrReader,
    maximum: CaptureLimits,
) -> Result<CapturedInventory, CaptureError> {
    if limits.max_paths > maximum.max_paths
        || limits.max_file_bytes > maximum.max_file_bytes
        || limits.max_total_bytes > maximum.max_total_bytes
    {
        return Err(CaptureError::InvalidLimits);
    }
    if limits.max_paths == 0 || limits.max_file_bytes == 0 || limits.max_total_bytes == 0 {
        return Err(CaptureError::InvalidLimits);
    }
    if declared.len() > limits.max_paths {
        return Err(CaptureError::TooManyPaths);
    }
    let root_stat = fstat(root_fd).map_err(CaptureError::Filesystem)?;
    if FileType::from_raw_mode(root_stat.st_mode) != FileType::Directory {
        return Err(CaptureError::InvalidRoot);
    }

    let mut unique = BTreeSet::new();
    if declared.iter().any(|path| !unique.insert(path.as_str())) {
        return Err(CaptureError::DuplicatePath);
    }

    let mut ordered = declared.to_vec();
    ordered.sort();
    let mut captured = Vec::with_capacity(ordered.len());
    let mut total_bytes = 0_u64;
    let mut metadata_payloads = BTreeMap::new();

    for path in ordered {
        let Some((parent, leaf)) = open_parent(root_fd, path.as_str())? else {
            captured.push(Captured {
                record: FileRecord::new(root, path, EntryKind::Absent)?,
                device: 0,
                inode: 0,
                links: 0,
            });
            continue;
        };

        let before = match statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) if error == Errno::NOENT => {
                captured.push(Captured {
                    record: FileRecord::new(root, path, EntryKind::Absent)?,
                    device: 0,
                    inode: 0,
                    links: 0,
                });
                continue;
            }
            Err(error) => return Err(CaptureError::Filesystem(error)),
        };

        let kind = FileType::from_raw_mode(before.st_mode);
        if kind == FileType::Symlink {
            let target_bytes = readlinkat(&parent, leaf, Vec::new())
                .map_err(CaptureError::Filesystem)?
                .into_bytes();
            if target_bytes.is_empty() || target_bytes.len() > 4096 {
                return Err(CaptureError::UnsupportedEntryType);
            }
            let target = String::from_utf8(target_bytes.clone())
                .map_err(|_| CaptureError::UnsupportedEntryType)?;
            if target.starts_with('/') || target.split('/').any(|component| component == "..") {
                return Err(CaptureError::UnsupportedEntryType);
            }
            let acl_access = xattrs.read_symlink(
                &File::from(dup(parent.as_fd()).map_err(CaptureError::Filesystem)?),
                leaf,
                "system.posix_acl_access",
            )?;
            let acl_default = xattrs.read_symlink(
                &File::from(dup(parent.as_fd()).map_err(CaptureError::Filesystem)?),
                leaf,
                "system.posix_acl_default",
            )?;
            let selinux = xattrs.read_symlink(
                &File::from(dup(parent.as_fd()).map_err(CaptureError::Filesystem)?),
                leaf,
                "security.selinux",
            )?;
            let metadata = metadata_from_xattrs(&before, acl_access, acl_default, selinux)?;
            let after = statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(CaptureError::Filesystem)?;
            let target_after = readlinkat(&parent, leaf, Vec::new())
                .map_err(CaptureError::Filesystem)?
                .into_bytes();
            if !same_fingerprint(&before, &after) || target_bytes != target_after {
                return Err(CaptureError::ConcurrentModification);
            }
            total_bytes = total_bytes
                .saturating_add(target_bytes.len() as u64)
                .saturating_add(metadata.source_bytes);
            if total_bytes > limits.max_total_bytes {
                return Err(CaptureError::TotalCaptureTooLarge);
            }
            extend_metadata(&mut metadata_payloads, metadata.payloads)?;
            captured.push(Captured {
                record: FileRecord::new(
                    root,
                    path,
                    EntryKind::Symlink { metadata: metadata.entry, target },
                )?,
                device: before.st_dev,
                inode: before.st_ino,
                links: before.st_nlink,
            });
            continue;
        }
        if !matches!(kind, FileType::RegularFile | FileType::Directory) {
            return Err(CaptureError::UnsupportedEntryType);
        }

        let mut flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        if kind == FileType::Directory {
            flags |= OFlags::DIRECTORY;
        }
        let fd = openat2(&parent, leaf, flags, Mode::empty(), RESOLVE_CAPTURE_PATH)
            .map_err(CaptureError::Filesystem)?;
        let opened = fstat(&fd).map_err(CaptureError::Filesystem)?;
        if !same_identity(&before, &opened) || FileType::from_raw_mode(opened.st_mode) != kind {
            return Err(CaptureError::ConcurrentModification);
        }
        let mut file = File::from(fd);

        let metadata_capture = capture_metadata(&file, &opened, xattrs)?;
        total_bytes = total_bytes.saturating_add(metadata_capture.source_bytes);
        if total_bytes > limits.max_total_bytes {
            return Err(CaptureError::TotalCaptureTooLarge);
        }
        let metadata_before = metadata_capture.entry;
        extend_metadata(&mut metadata_payloads, metadata_capture.payloads)?;
        let entry_kind = match kind {
            FileType::Directory => EntryKind::Directory { metadata: metadata_before.clone() },
            FileType::RegularFile => {
                let size = u64::try_from(opened.st_size).map_err(|_| CaptureError::FileTooLarge)?;
                if size > limits.max_file_bytes
                    || total_bytes.saturating_add(size) > limits.max_total_bytes
                {
                    return Err(if size > limits.max_file_bytes {
                        CaptureError::FileTooLarge
                    } else {
                        CaptureError::TotalCaptureTooLarge
                    });
                }

                let mut hasher = Sha256Digest::new();
                let mut read_total = 0_u64;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    let read = file.read(&mut buffer).map_err(CaptureError::ContentRead)?;
                    if read == 0 {
                        break;
                    }
                    read_total = read_total.saturating_add(read as u64);
                    if read_total > limits.max_file_bytes
                        || total_bytes.saturating_add(read_total) > limits.max_total_bytes
                    {
                        return Err(if read_total > limits.max_file_bytes {
                            CaptureError::FileTooLarge
                        } else {
                            CaptureError::TotalCaptureTooLarge
                        });
                    }
                    hasher.update(&buffer[..read]);
                }
                let after = fstat(&file).map_err(CaptureError::Filesystem)?;
                let path_after = statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(CaptureError::Filesystem)?;
                let metadata_after = capture_metadata(&file, &after, xattrs)?.entry;
                if !same_fingerprint(&opened, &after)
                    || !same_identity(&after, &path_after)
                    || metadata_before != metadata_after
                    || read_total != size
                {
                    return Err(CaptureError::ConcurrentModification);
                }
                total_bytes = total_bytes.saturating_add(read_total);
                let digest =
                    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
                let sha256 = Sha256::new(digest)?;
                EntryKind::RegularFile {
                    metadata: metadata_before.clone(),
                    size: read_total,
                    sha256,
                    hard_link_group: None,
                }
            }
            _ => unreachable!("file type was restricted above"),
        };

        if kind == FileType::Directory {
            let after = fstat(file.as_fd()).map_err(CaptureError::Filesystem)?;
            let path_after = statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(CaptureError::Filesystem)?;
            let metadata_after = capture_metadata(&file, &after, xattrs)?.entry;
            if !same_fingerprint(&opened, &after)
                || !same_identity(&after, &path_after)
                || metadata_before != metadata_after
            {
                return Err(CaptureError::ConcurrentModification);
            }
        }

        let links = opened.st_nlink;
        captured.push(Captured {
            record: FileRecord::new(root, path, entry_kind)?,
            device: opened.st_dev,
            inode: opened.st_ino,
            links,
        });
    }

    assign_hard_link_groups(&mut captured)?;
    Ok(CapturedInventory {
        records: captured.into_iter().map(|entry| entry.record).collect(),
        metadata_payloads,
        source_bytes: total_bytes,
    })
}

fn assign_hard_link_groups(captured: &mut [Captured]) -> Result<(), CaptureError> {
    let mut aliases = BTreeMap::<(u64, u64), usize>::new();
    for entry in captured.iter() {
        if matches!(entry.record.kind, EntryKind::RegularFile { .. }) && entry.links > 1 {
            *aliases.entry((entry.device, entry.inode)).or_default() += 1;
        }
    }
    let mut proofs = BTreeMap::new();
    for entry in captured.iter_mut() {
        if entry.links <= 1 || !matches!(entry.record.kind, EntryKind::RegularFile { .. }) {
            continue;
        }
        if aliases.get(&(entry.device, entry.inode)).copied() != usize::try_from(entry.links).ok() {
            return Err(CaptureError::HardLinkOutsideInventory);
        }
        let group = Identifier::new(format!("hardlink-{:x}-{:x}", entry.device, entry.inode))?;
        if let EntryKind::RegularFile { metadata, size, sha256, hard_link_group } =
            &mut entry.record.kind
        {
            let proof = (metadata.clone(), *size, sha256.clone());
            if let Some(previous) = proofs.get(&(entry.device, entry.inode)) {
                if *previous != proof {
                    return Err(CaptureError::ConcurrentModification);
                }
            } else {
                proofs.insert((entry.device, entry.inode), proof);
            }
            *hard_link_group = Some(group);
        }
    }

    Ok(())
}

fn open_parent<'a>(
    root_fd: &File,
    relative: &'a str,
) -> Result<Option<(rustix::fd::OwnedFd, &'a str)>, CaptureError> {
    let mut components = relative.split('/').peekable();
    let mut parent = dup(root_fd.as_fd()).map_err(CaptureError::Filesystem)?;
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            return Ok(Some((parent, component)));
        }
        parent = match openat2(
            &parent,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
            RESOLVE_CAPTURE_PATH,
        ) {
            Ok(next) => next,
            Err(error) if error == Errno::NOENT => return Ok(None),
            Err(error) => return Err(CaptureError::Filesystem(error)),
        };
    }
    Err(CaptureError::Validation(ValidationError::InvalidRelativePath))
}

pub(crate) trait XattrReader {
    fn read(&self, fd: &File, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError>;
    fn read_symlink(
        &self,
        parent: &File,
        leaf: &str,
        name: &'static str,
    ) -> Result<Option<Vec<u8>>, CaptureError>;
}

struct KernelXattrs;

impl XattrReader for KernelXattrs {
    fn read(&self, fd: &File, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
        get_xattr(fd.as_fd(), name)
    }

    fn read_symlink(
        &self,
        parent: &File,
        leaf: &str,
        name: &'static str,
    ) -> Result<Option<Vec<u8>>, CaptureError> {
        // `parent` is pinned and `leaf` is a single validated component.
        // lgetxattr does not follow the final symlink.
        let path = format!("/proc/self/fd/{}/{leaf}", parent.as_raw_fd());
        let mut buffer = vec![0_u8; MAX_XATTR_BYTES];
        match lgetxattr(path, name, &mut buffer) {
            Ok(length) => {
                buffer.truncate(length);
                Ok(Some(buffer))
            }
            Err(error) if error == Errno::NODATA => Ok(None),
            // Linux symlink inodes do not carry POSIX access/default ACLs.
            // Their ACL lookup can report EOPNOTSUPP even on an ACL-capable
            // filesystem; this is absence, not a missing capture provider.
            Err(error)
                if error == Errno::OPNOTSUPP
                    && matches!(name, "system.posix_acl_access" | "system.posix_acl_default") =>
            {
                Ok(None)
            }
            Err(error) if error == Errno::OPNOTSUPP || error == Errno::NOSYS => {
                Err(CaptureError::UnsupportedMetadata(name))
            }
            Err(error) if error == Errno::RANGE => Err(CaptureError::MetadataTooLarge),
            Err(error) => Err(CaptureError::Filesystem(error)),
        }
    }
}

struct MetadataSnapshot {
    entry: EntryMetadata,
    source_bytes: u64,
    payloads: BTreeMap<Sha256, Vec<u8>>,
}

fn capture_metadata(
    fd: &File,
    stat: &rustix::fs::Stat,
    xattrs: &impl XattrReader,
) -> Result<MetadataSnapshot, CaptureError> {
    let acl_access = xattrs.read(fd, "system.posix_acl_access")?;
    let acl_default = xattrs.read(fd, "system.posix_acl_default")?;
    let selinux = xattrs.read(fd, "security.selinux")?;
    metadata_from_xattrs(stat, acl_access, acl_default, selinux)
}

fn metadata_from_xattrs(
    stat: &rustix::fs::Stat,
    acl_access: Option<Vec<u8>>,
    acl_default: Option<Vec<u8>>,
    selinux: Option<Vec<u8>>,
) -> Result<MetadataSnapshot, CaptureError> {
    let acl_access_len = acl_access.as_ref().map_or(0, |value| value.len() as u64);
    let acl_default_len = acl_default.as_ref().map_or(0, |value| value.len() as u64);
    let mut payloads = BTreeMap::new();
    let acl_bytes = AclPayload { access: acl_access, default: acl_default }
        .encode()
        .map_err(|_| CaptureError::MetadataTooLarge)?;
    let acl_sha256 = acl_bytes.map(|bytes| {
        let digest = hash(Sha256Digest::digest(&bytes));
        payloads.insert(digest.clone(), bytes);
        digest
    });
    let mut extended = ExtendedMetadata::default();
    let selinux_len = selinux.as_ref().map_or(0, |value| value.len() as u64);
    if let Some(selinux) = selinux {
        if selinux.len() > MAX_METADATA_VALUE_BYTES {
            return Err(CaptureError::MetadataTooLarge);
        }
        let digest = hash(Sha256Digest::digest(&selinux));
        extended.values.insert(crate::XattrName::SelinuxLabel, digest.clone());
        payloads.insert(digest, selinux);
    }
    let source_bytes = acl_access_len.saturating_add(acl_default_len).saturating_add(selinux_len);
    let mtime_ns = i128::from(stat.st_mtime)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i128::from(stat.st_mtime_nsec)))
        .ok_or(CaptureError::UnsupportedMetadata("mtime"))?;
    Ok(MetadataSnapshot {
        entry: EntryMetadata {
            mode: stat.st_mode & 0o7777,
            uid: stat.st_uid,
            gid: stat.st_gid,
            mtime_unix_ns: mtime_ns,
            acl_sha256,
            extended,
        },
        source_bytes,
        payloads,
    })
}

fn get_xattr<Fd: AsFd>(fd: Fd, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
    let mut buffer = vec![0_u8; MAX_XATTR_BYTES];
    match fgetxattr(fd, name, &mut buffer) {
        Ok(length) => {
            buffer.truncate(length);
            Ok(Some(buffer))
        }
        Err(error) if error == Errno::NODATA => Ok(None),
        Err(error) if error == Errno::OPNOTSUPP || error == Errno::NOSYS => {
            Err(CaptureError::UnsupportedMetadata(name))
        }
        Err(error) if error == Errno::RANGE => Err(CaptureError::MetadataTooLarge),
        Err(error) => Err(CaptureError::Filesystem(error)),
    }
}

const MAX_XATTR_BYTES: usize = MAX_METADATA_VALUE_BYTES;
const HARD_MAX_PATHS: usize = 256;
const HARD_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const HARD_MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const RESOLVE_CAPTURE_PATH: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

fn hash(digest: impl AsRef<[u8]>) -> Sha256 {
    let value = digest.as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    // SHA-256's lowercase hexadecimal output is always accepted by this constructor.
    Sha256::new(value).expect("SHA-256 encoder emits a valid digest")
}

fn same_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

fn same_fingerprint(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    same_identity(left, right)
        && left.st_mode == right.st_mode
        && left.st_uid == right.st_uid
        && left.st_gid == right.st_gid
        && left.st_nlink == right.st_nlink
        && left.st_size == right.st_size
        && left.st_mtime == right.st_mtime
        && left.st_mtime_nsec == right.st_mtime_nsec
        && left.st_ctime == right.st_ctime
        && left.st_ctime_nsec == right.st_ctime_nsec
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        os::unix::fs::symlink,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{CaptureError, CaptureLimits, capture_declared};
    use crate::{EntryKind, RelativePath, Sha256, StateRoot};
    use rustix::fs::{Mode, mkfifoat};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path =
                std::env::temp_dir().join(format!("ryoku-capture-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn path(value: &str) -> RelativePath {
        RelativePath::new(value).unwrap()
    }

    fn root_file(path: &std::path::Path) -> File {
        File::open(path).unwrap()
    }

    struct NoXattrs;

    impl super::XattrReader for NoXattrs {
        fn read(&self, _fd: &File, _name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
            Ok(None)
        }
        fn read_symlink(
            &self,
            _parent: &File,
            _leaf: &str,
            _name: &'static str,
        ) -> Result<Option<Vec<u8>>, CaptureError> {
            Ok(None)
        }
    }

    struct EmptyAccessAcl;

    impl super::XattrReader for EmptyAccessAcl {
        fn read(&self, _fd: &File, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
            Ok((name == "system.posix_acl_access").then(Vec::new))
        }
        fn read_symlink(
            &self,
            _parent: &File,
            _leaf: &str,
            name: &'static str,
        ) -> Result<Option<Vec<u8>>, CaptureError> {
            Ok((name == "system.posix_acl_access").then(Vec::new))
        }
    }

    struct SmallXattrs;

    impl super::XattrReader for SmallXattrs {
        fn read(&self, _fd: &File, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
            Ok((name == "security.selinux").then(|| vec![b'x'; 2]))
        }
        fn read_symlink(
            &self,
            _parent: &File,
            _leaf: &str,
            name: &'static str,
        ) -> Result<Option<Vec<u8>>, CaptureError> {
            Ok((name == "security.selinux").then(|| vec![b'x'; 2]))
        }
    }

    struct UnsupportedXattrs;

    impl super::XattrReader for UnsupportedXattrs {
        fn read(&self, _fd: &File, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
            Err(CaptureError::UnsupportedMetadata(name))
        }
        fn read_symlink(
            &self,
            _parent: &File,
            _leaf: &str,
            name: &'static str,
        ) -> Result<Option<Vec<u8>>, CaptureError> {
            Err(CaptureError::UnsupportedMetadata(name))
        }
    }

    fn capture_without_xattrs(
        root: &File,
        declared: &[RelativePath],
        limits: CaptureLimits,
    ) -> Result<Vec<crate::FileRecord>, CaptureError> {
        super::capture_declared_with_xattrs(root, StateRoot::EtcRyoku, declared, limits, &NoXattrs)
    }

    #[test]
    fn captures_only_declared_files_and_absences_without_recursion() {
        let temp = TempDir::new();
        fs::create_dir(temp.0.join("nested")).unwrap();
        fs::write(temp.0.join("nested/config.toml"), b"safe = true\n").unwrap();
        fs::write(temp.0.join("unlisted"), b"ignore me").unwrap();
        let root = root_file(&temp.0);

        let records = capture_without_xattrs(
            &root,
            &[path("nested"), path("nested/config.toml"), path("missing/file.toml")],
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(records.len(), 3);
        assert!(records.iter().any(|record| {
            record.path.as_str() == "nested" && matches!(record.kind, EntryKind::Directory { .. })
        }));
        assert!(records.iter().any(|record| {
            record.path.as_str() == "nested/config.toml"
                && matches!(record.kind, EntryKind::RegularFile { size: 12, .. })
        }));
        assert!(records.iter().any(|record| {
            record.path.as_str() == "missing/file.toml" && matches!(record.kind, EntryKind::Absent)
        }));
    }

    #[test]
    fn refuses_symlink_parent_and_leaf_without_touching_target() {
        let temp = TempDir::new();
        let outside = TempDir::new();
        fs::write(outside.0.join("secret"), b"untouched").unwrap();
        symlink(&outside.0, temp.0.join("parent-link")).unwrap();
        symlink(outside.0.join("secret"), temp.0.join("leaf-link")).unwrap();
        let root = root_file(&temp.0);

        let parent =
            capture_without_xattrs(&root, &[path("parent-link/secret")], CaptureLimits::default());
        assert!(matches!(parent, Err(CaptureError::Filesystem(_))));

        let leaf = capture_without_xattrs(&root, &[path("leaf-link")], CaptureLimits::default());
        assert!(matches!(leaf, Err(CaptureError::UnsupportedEntryType)));
        assert_eq!(fs::read(outside.0.join("secret")).unwrap(), b"untouched");
    }

    #[test]
    fn captures_safe_symlink_target_as_text_without_following_it() {
        let temp = TempDir::new();
        fs::write(temp.0.join("theme.toml"), b"theme = 'blue'\n").unwrap();
        symlink("theme.toml", temp.0.join("theme-current")).unwrap();
        let root = root_file(&temp.0);

        let records =
            capture_without_xattrs(&root, &[path("theme-current")], CaptureLimits::default())
                .unwrap();

        assert!(matches!(
            &records[0].kind,
            EntryKind::Symlink { target, .. } if target == "theme.toml"
        ));
        assert_eq!(
            fs::read_link(temp.0.join("theme-current")).unwrap(),
            std::path::Path::new("theme.toml")
        );
    }

    #[test]
    fn rejects_special_files_limits_duplicates_and_unlisted_hard_links() {
        let temp = TempDir::new();
        fs::write(temp.0.join("large"), vec![b'x'; 16]).unwrap();
        fs::hard_link(temp.0.join("large"), temp.0.join("alias")).unwrap();
        let root = root_file(&temp.0);
        mkfifoat(&root, "pipe", Mode::from_raw_mode(0o600)).unwrap();
        assert!(matches!(
            capture_without_xattrs(&root, &[path("pipe")], CaptureLimits::default()),
            Err(CaptureError::UnsupportedEntryType)
        ));
        assert!(matches!(
            capture_without_xattrs(&root, &[path("large")], CaptureLimits::default()),
            Err(CaptureError::HardLinkOutsideInventory)
        ));
        let complete = capture_without_xattrs(
            &root,
            &[path("large"), path("alias")],
            CaptureLimits::default(),
        )
        .unwrap();
        let groups = complete
            .iter()
            .filter_map(|record| match &record.kind {
                EntryKind::RegularFile { hard_link_group, .. } => hard_link_group.as_ref(),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], groups[1]);
        let mut inconsistent = vec![
            super::Captured { record: complete[0].clone(), device: 9, inode: 17, links: 2 },
            super::Captured { record: complete[1].clone(), device: 9, inode: 17, links: 2 },
        ];
        if let EntryKind::RegularFile { sha256, .. } = &mut inconsistent[1].record.kind {
            *sha256 = Sha256::new("f".repeat(64)).unwrap();
        }
        assert!(matches!(
            super::assign_hard_link_groups(&mut inconsistent),
            Err(CaptureError::ConcurrentModification)
        ));
        assert!(matches!(
            capture_without_xattrs(
                &root,
                &[path("large"), path("large")],
                CaptureLimits::default()
            ),
            Err(CaptureError::DuplicatePath)
        ));
        let limits = CaptureLimits { max_paths: 1, max_file_bytes: 1, max_total_bytes: 1 };
        fs::write(temp.0.join("single"), vec![b'x'; 16]).unwrap();
        assert!(matches!(
            capture_without_xattrs(&root, &[path("single")], limits),
            Err(CaptureError::FileTooLarge)
        ));
    }

    #[test]
    fn captures_or_refuses_when_host_metadata_support_varies() {
        let temp = TempDir::new();
        fs::write(temp.0.join("config"), b"value").unwrap();
        let root = root_file(&temp.0);
        match capture_declared(
            &root,
            StateRoot::EtcRyoku,
            &[path("config")],
            CaptureLimits::default(),
        ) {
            Err(CaptureError::UnsupportedMetadata(_)) => {}
            Ok(_) => {}
            Err(error) => panic!("unexpected capture error: {error}"),
        }
        assert!(matches!(
            super::capture_declared_with_xattrs(
                &root,
                StateRoot::EtcRyoku,
                &[path("config")],
                CaptureLimits::default(),
                &UnsupportedXattrs,
            ),
            Err(CaptureError::UnsupportedMetadata("system.posix_acl_access"))
        ));
    }

    #[test]
    fn acl_digest_distinguishes_absence_from_empty_and_counts_metadata_bytes() {
        let temp = TempDir::new();
        fs::write(temp.0.join("config"), b"value").unwrap();
        let root = root_file(&temp.0);
        let fd = rustix::fs::openat2(
            &root,
            "config",
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
        )
        .unwrap();
        let file = File::from(fd);
        let stat = rustix::fs::fstat(&file).unwrap();
        let absent = super::capture_metadata(&file, &stat, &NoXattrs).unwrap();
        let empty_acl = super::capture_metadata(&file, &stat, &EmptyAccessAcl).unwrap();
        let labeled = super::capture_metadata(&file, &stat, &SmallXattrs).unwrap();
        assert_eq!(absent.entry.acl_sha256, None);
        assert_ne!(empty_acl.entry.acl_sha256, None);
        assert_eq!(empty_acl.source_bytes, 0);
        assert_eq!(labeled.source_bytes, 2);
        assert!(labeled.entry.extended.values.contains_key(&crate::XattrName::SelinuxLabel));

        assert!(matches!(
            super::capture_declared_with_xattrs(
                &root,
                StateRoot::EtcRyoku,
                &[path("config")],
                CaptureLimits { max_paths: 1, max_file_bytes: 16, max_total_bytes: 6 },
                &SmallXattrs,
            ),
            Err(CaptureError::TotalCaptureTooLarge)
        ));
    }
}
