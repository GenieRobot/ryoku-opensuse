//! Durable configuration staging outside rollback scope. Objects have fixed
//! flat slot names; manifest target paths and symlink targets never resolve
//! beneath this store. The privileged coordinator must validate the persistent
//! mount, authorize restore, retain the source/checkpoint, and hold exclusive
//! writer ownership. Nothing here publishes live targets or advances a journal.

use crate::{
    EntryKind, FileRecord, RelativePath, StateRoot,
    capture::{CaptureError, CaptureLimits, capture_declared},
    configuration_stage::StagedConfiguration,
    inode_metadata::{InodeMetadataError, prepare_inode_metadata, prepare_symlink_metadata},
    restore::RestorePlan,
};
use rustix::{
    fs::{
        AtFlags, FileType, Mode, OFlags, RenameFlags, fsync, linkat, mkdirat, openat,
        renameat_with, statat, symlinkat, syncfs, unlinkat,
    },
    io::Errno,
};
use serde::Serialize;
use sha2::Digest;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

const MAX_RECEIPT_BYTES: u64 = crate::MAX_MANIFEST_BYTES as u64 + 16 * 1024;
const RECEIPT: &str = "complete.json";

#[derive(Debug)]
pub enum DiskStageError {
    InvalidStore,
    IncompleteOrChanged,
    ReceiptTooLarge,
    Capture(CaptureError),
    Metadata(InodeMetadataError),
    Filesystem(Errno),
    Io(std::io::Error),
    Encoding(serde_json::Error),
}
impl std::fmt::Display for DiskStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "configuration disk staging failed: {self:?}")
    }
}
impl std::error::Error for DiskStageError {}
impl From<Errno> for DiskStageError {
    fn from(e: Errno) -> Self {
        Self::Filesystem(e)
    }
}
impl From<std::io::Error> for DiskStageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<CaptureError> for DiskStageError {
    fn from(e: CaptureError) -> Self {
        Self::Capture(e)
    }
}
impl From<InodeMetadataError> for DiskStageError {
    fn from(e: InodeMetadataError) -> Self {
        Self::Metadata(e)
    }
}
impl From<serde_json::Error> for DiskStageError {
    fn from(e: serde_json::Error) -> Self {
        Self::Encoding(e)
    }
}

#[derive(Serialize)]
struct Receipt<'a> {
    schema_version: u16,
    plan: &'a RestorePlan,
    entries: Vec<&'a FileRecord>,
}

/// A pinned stage verified against the exact immutable source bundle. This is
/// an observation, not a writer lock: the coordinator must keep exclusive
/// ownership and reverify after process restart before using any objects.
pub struct VerifiedDiskStage {
    directory: File,
    plan: RestorePlan,
}
impl VerifiedDiskStage {
    pub fn directory(&self) -> &File {
        &self.directory
    }
    pub fn plan(&self) -> &RestorePlan {
        &self.plan
    }
}

pub struct DiskStageStore {
    root: File,
    owner: u32,
}

impl DiskStageStore {
    /// Requires a caller-validated persistent root-owned mode-0700 directory.
    pub fn open(root: &File) -> Result<Self, DiskStageError> {
        Self::checked_open(root, 0)
    }

    #[cfg(test)]
    pub(crate) fn open_for_test(root: &File) -> Result<Self, DiskStageError> {
        Self::checked_open(root, root.metadata()?.uid())
    }

    fn checked_open(root: &File, owner: u32) -> Result<Self, DiskStageError> {
        check_directory(root, owner)?;
        Ok(Self { root: File::from(rustix::io::dup(root)?), owner })
    }

    /// Create once. An existing or interrupted stage is never overwritten or
    /// credited as complete. On failure retain recovery mode and the checkpoint;
    /// the coordinator must explicitly discard/rebuild an incomplete stage.
    pub fn create(
        &self,
        source: &StagedConfiguration,
    ) -> Result<VerifiedDiskStage, DiskStageError> {
        self.create_with_observer(source, |_| {})
    }

    #[cfg(test)]
    pub(crate) fn create_with_hook(
        &self,
        source: &StagedConfiguration,
        after_durable_boundary: impl FnMut(usize),
    ) -> Result<VerifiedDiskStage, DiskStageError> {
        self.create_with_observer(source, after_durable_boundary)
    }

    fn create_with_observer(
        &self,
        source: &StagedConfiguration,
        mut after_durable_boundary: impl FnMut(usize),
    ) -> Result<VerifiedDiskStage, DiskStageError> {
        let receipt = receipt_bytes(source)?;
        let name = stage_name(source)?;
        mkdirat(&self.root, name.as_str(), Mode::from_raw_mode(0o700))?;
        fsync(&self.root)?;
        after_durable_boundary(1);
        let directory = self.open_directory(&name)?;
        let mut links = BTreeMap::new();
        for (index, entry) in source.entries().iter().enumerate() {
            let slot = object_name(index);
            match &entry.record().kind {
                EntryKind::Absent => {}
                EntryKind::Directory { .. } => {
                    mkdirat(&directory, slot.as_str(), Mode::from_raw_mode(0o700))?;
                    let file = File::from(openat(
                        &directory,
                        slot.as_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )?);
                    prepare_inode_metadata(&file, entry)?;
                }
                EntryKind::RegularFile { hard_link_group, .. } => {
                    if let Some(previous) =
                        hard_link_group.as_ref().and_then(|group| links.get(group))
                    {
                        linkat(&directory, previous, &directory, slot.as_str(), AtFlags::empty())?;
                        continue;
                    }
                    let mut file = File::from(openat(
                        &directory,
                        slot.as_str(),
                        OFlags::RDWR
                            | OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::NOFOLLOW
                            | OFlags::CLOEXEC,
                        Mode::from_raw_mode(0o600),
                    )?);
                    file.write_all(entry.content().ok_or(DiskStageError::IncompleteOrChanged)?)?;
                    prepare_inode_metadata(&file, entry)?;
                    if let Some(group) = hard_link_group {
                        links.insert(group.clone(), slot);
                    }
                }
                EntryKind::Symlink { target, .. } => {
                    symlinkat(target.as_str(), &directory, slot.as_str())?;
                    prepare_symlink_metadata(
                        &directory,
                        &RelativePath::new(slot).expect("fixed slot"),
                        entry,
                    )?;
                }
            }
        }
        fsync(&directory)?;
        // Include link-count and symlink-inode metadata in the durable flush.
        syncfs(&directory)?;
        verify_objects(&directory, source, false)?;
        after_durable_boundary(2);
        let mut file = File::from(openat(
            &directory,
            ".receipt.tmp",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        file.write_all(&receipt)?;
        fsync(&file)?;
        fsync(&directory)?;
        after_durable_boundary(3);
        renameat_with(&directory, ".receipt.tmp", &directory, RECEIPT, RenameFlags::NOREPLACE)?;
        fsync(&directory)?;
        fsync(&self.root)?;
        after_durable_boundary(4);
        Ok(VerifiedDiskStage { directory, plan: source.plan().clone() })
    }

    /// Reopen only after reconstructing the exact source bundle from its
    /// verified durable generation and persisted digest-bound restore plan.
    pub fn open_verified(
        &self,
        source: &StagedConfiguration,
    ) -> Result<VerifiedDiskStage, DiskStageError> {
        let directory = self.open_directory(&stage_name(source)?)?;
        let receipt = File::from(openat(
            &directory,
            RECEIPT,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let metadata = receipt.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != self.owner
            || metadata.mode() & 0o7777 != 0o600
            || metadata.nlink() != 1
        {
            return Err(DiskStageError::IncompleteOrChanged);
        }
        let mut bytes = Vec::new();
        receipt.take(MAX_RECEIPT_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes != receipt_bytes(source)? {
            return Err(DiskStageError::IncompleteOrChanged);
        }
        verify_objects(&directory, source, true)?;
        Ok(VerifiedDiskStage { directory, plan: source.plan().clone() })
    }

    /// Remove only an incomplete stage for the exact verified source bundle.
    /// The caller must hold the exclusive recovery lock and prove that the
    /// journal has not advanced to `ContentStaged`. Complete receipts, unknown
    /// entries, wrong inode types/owners/modes, external hard links and
    /// populated staged directories are retained for explicit review.
    pub fn discard_incomplete(&self, source: &StagedConfiguration) -> Result<(), DiskStageError> {
        let name = stage_name(source)?;
        let directory = self.open_directory(&name)?;
        let receipt = statat(&directory, RECEIPT, AtFlags::SYMLINK_NOFOLLOW);
        if !matches!(receipt, Err(Errno::NOENT)) {
            return Err(DiskStageError::IncompleteOrChanged);
        }

        let mut expected = BTreeMap::new();
        for (index, entry) in source.entries().iter().enumerate() {
            if !matches!(entry.record().kind, EntryKind::Absent) {
                expected.insert(object_name(index), entry);
            }
        }
        let actual_names = fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))?
            .map(|entry| entry.map(|entry| entry.file_name().into_string().unwrap_or_default()))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if actual_names
            .iter()
            .any(|actual| actual != ".receipt.tmp" && !expected.contains_key(actual.as_str()))
        {
            return Err(DiskStageError::IncompleteOrChanged);
        }
        if actual_names.len()
            != actual_names.iter().filter(|name| *name == ".receipt.tmp").count()
                + actual_names.iter().filter(|name| expected.contains_key(name.as_str())).count()
        {
            return Err(DiskStageError::IncompleteOrChanged);
        }

        let store_stat = rustix::fs::fstat(&directory)?;
        for actual in &actual_names {
            let stat = statat(&directory, actual.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
            let kind = FileType::from_raw_mode(stat.st_mode);
            if actual == ".receipt.tmp" {
                if kind != FileType::RegularFile
                    || stat.st_uid != store_stat.st_uid
                    || stat.st_gid != store_stat.st_gid
                    || stat.st_mode & 0o7777 != 0o600
                    || stat.st_nlink != 1
                {
                    return Err(DiskStageError::IncompleteOrChanged);
                }
                continue;
            }
            let entry = expected.get(actual.as_str()).ok_or(DiskStageError::IncompleteOrChanged)?;
            let (expected_type, metadata) = match &entry.record().kind {
                EntryKind::Directory { metadata } => (FileType::Directory, metadata),
                EntryKind::RegularFile { metadata, .. } => (FileType::RegularFile, metadata),
                EntryKind::Symlink { metadata, .. } => (FileType::Symlink, metadata),
                EntryKind::Absent => return Err(DiskStageError::IncompleteOrChanged),
            };
            if kind != expected_type
                || stat.st_uid != metadata.uid
                || stat.st_gid != metadata.gid
                || stat.st_mode & 0o7777 != metadata.mode
            {
                return Err(DiskStageError::IncompleteOrChanged);
            }
            if let EntryKind::RegularFile { hard_link_group, .. } = &entry.record().kind {
                let expected_links = match hard_link_group {
                    Some(group) => expected
                        .iter()
                        .filter(|(slot, candidate)| {
                            actual_names.contains(slot.as_str())
                                && matches!(
                                    &candidate.record().kind,
                                    EntryKind::RegularFile {
                                        hard_link_group: Some(candidate_group),
                                        ..
                                    } if candidate_group == group
                                )
                        })
                        .count() as u64,
                    None => 1,
                };
                if stat.st_nlink != expected_links {
                    return Err(DiskStageError::IncompleteOrChanged);
                }
            }
            if expected_type == FileType::Directory
                && fs::read_dir(format!("/proc/self/fd/{}/{}", directory.as_raw_fd(), actual))?
                    .next()
                    .is_some()
            {
                return Err(DiskStageError::IncompleteOrChanged);
            }
        }

        for actual in actual_names {
            let flags = if expected
                .get(&actual)
                .is_some_and(|entry| matches!(entry.record().kind, EntryKind::Directory { .. }))
            {
                AtFlags::REMOVEDIR
            } else {
                AtFlags::empty()
            };
            unlinkat(&directory, actual.as_str(), flags)?;
        }
        fsync(&directory)?;
        unlinkat(&self.root, name.as_str(), AtFlags::REMOVEDIR)?;
        fsync(&self.root)?;
        Ok(())
    }

    fn open_directory(&self, name: &str) -> Result<File, DiskStageError> {
        let directory = File::from(openat(
            &self.root,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        check_directory(&directory, self.owner)?;
        if directory.metadata()?.dev() != self.root.metadata()?.dev() {
            return Err(DiskStageError::InvalidStore);
        }
        Ok(directory)
    }
}

fn check_directory(file: &File, owner: u32) -> Result<(), DiskStageError> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o7777 != 0o700 {
        return Err(DiskStageError::InvalidStore);
    }
    Ok(())
}
pub(crate) fn stage_name(source: &StagedConfiguration) -> Result<String, DiskStageError> {
    // Bind every receipt input, including scope. Independent restore plans
    // for the same source must not reuse an incompatible completed directory.
    Ok(format!("stage-{:x}", sha2::Sha256::digest(receipt_bytes(source)?)))
}
fn object_name(index: usize) -> String {
    format!("object-{index:03}")
}
fn receipt_bytes(source: &StagedConfiguration) -> Result<Vec<u8>, DiskStageError> {
    // Apply the same expanded-path budget as generation capture, rather than
    // counting a reused payload digest only once and writing an unbounded set.
    let limits = CaptureLimits::default();
    if source.entries().len() > limits.max_paths {
        return Err(CaptureError::TooManyPaths.into());
    }
    let mut total = 0_u64;
    for entry in source.entries() {
        let content = entry.content().map_or(0, |bytes| bytes.len() as u64);
        if content > limits.max_file_bytes {
            return Err(CaptureError::FileTooLarge.into());
        }
        let target = match &entry.record().kind {
            EntryKind::Symlink { target, .. } => target.len() as u64,
            _ => 0,
        };
        let acl = entry.acl().map_or(0, |acl| {
            acl.access.as_ref().map_or(0, |value| value.len() as u64)
                + acl.default.as_ref().map_or(0, |value| value.len() as u64)
        });
        let labels: u64 = entry.xattrs().map(|(_, bytes)| bytes.len() as u64).sum();
        total = total
            .saturating_add(content)
            .saturating_add(target)
            .saturating_add(acl)
            .saturating_add(labels);
        if total > limits.max_total_bytes {
            return Err(CaptureError::TotalCaptureTooLarge.into());
        }
    }
    let receipt = serde_json::to_vec(&Receipt {
        schema_version: 1,
        plan: source.plan(),
        entries: source.entries().iter().map(|entry| entry.record()).collect(),
    })?;
    if receipt.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(DiskStageError::ReceiptTooLarge);
    }
    Ok(receipt)
}

fn verify_objects(
    directory: &File,
    source: &StagedConfiguration,
    complete: bool,
) -> Result<(), DiskStageError> {
    let slots: Vec<_> = (0..source.entries().len())
        .map(|index| RelativePath::new(object_name(index)).expect("fixed slot"))
        .collect();
    let mut names: BTreeSet<_> = source
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, entry)| !matches!(entry.record().kind, EntryKind::Absent))
        .map(|(index, _)| object_name(index))
        .collect();
    if complete {
        names.insert(RECEIPT.into());
    }
    let actual_names = fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))?
        .map(|entry| entry.map(|entry| entry.file_name().into_string().unwrap_or_default()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if actual_names != names {
        return Err(DiskStageError::IncompleteOrChanged);
    }
    let observed =
        capture_declared(directory, StateRoot::EtcRyoku, &slots, CaptureLimits::default())?;
    let mut link_groups = BTreeMap::new();
    let mut reverse_groups = BTreeMap::new();
    for (entry, mut actual) in source.entries().iter().zip(observed) {
        if let (
            EntryKind::RegularFile { hard_link_group: expected, .. },
            EntryKind::RegularFile { hard_link_group: found, .. },
        ) = (&entry.record().kind, &mut actual.kind)
        {
            match (expected, found.as_ref()) {
                (Some(expected), Some(found)) => {
                    if link_groups
                        .insert(expected.clone(), found.clone())
                        .is_some_and(|old| old != *found)
                        || reverse_groups
                            .insert(found.clone(), expected.clone())
                            .is_some_and(|old| old != *expected)
                    {
                        return Err(DiskStageError::IncompleteOrChanged);
                    }
                }
                (None, None) => {}
                _ => return Err(DiskStageError::IncompleteOrChanged),
            }
            *found = expected.clone();
        }
        if entry.record().kind != actual.kind {
            return Err(DiskStageError::IncompleteOrChanged);
        }
    }
    Ok(())
}
