//! Exact provider-owned asset enrollment beside the fixed preference inventory.
//!
//! No wire request chooses a home or destination root. The native helper must
//! resolve CaptureAccount/home from its authenticated peer and hold its writer
//! lease. This module checks that binding, captures only compiled namespaces
//! and exact declarations, and preserves absence and ACL/xattr references.
//! It does not add asset paths to historical schema-1/2 preference records.
use crate::{
    AccountPrincipal, EntryKind, FileRecord, RelativePath, Sha256, StateRoot,
    capture::{CaptureError, CaptureLimits, capture_ryogami_assets},
    generation::Payloads,
    installed_capture::CaptureAccount,
};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Hash};
use std::{collections::BTreeSet, fs::File};

pub const MAX_CAPTURE_LIMITS: CaptureLimits = CaptureLimits {
    max_paths: 4096,
    max_file_bytes: 8 * 1024 * 1024 * 1024,
    max_total_bytes: 32 * 1024 * 1024 * 1024,
};

/// Fixed existing/private Ryogami namespaces. Configured image/video libraries
/// are deliberately not inferred from an arbitrary client path; their trusted
/// settings/catalog enrollment is a separate integration boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetNamespace {
    ProviderContent,
    SemanticModels,
    RemotePreviews,
    RemoteThumbnails,
    WorkshopContent,
}
impl AssetNamespace {
    pub fn home_prefix(self) -> &'static str {
        match self {
            Self::ProviderContent => ".local/share/ryogami/providers",
            Self::SemanticModels => ".local/share/ryogami/lens/models",
            Self::RemotePreviews => ".cache/ryogami/remote-preview",
            Self::RemoteThumbnails => ".cache/ryogami/remote",
            Self::WorkshopContent => ".local/share/ryogami/workshop",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetPath {
    namespace: AssetNamespace,
    relative: RelativePath,
}
impl AssetPath {
    pub fn new(namespace: AssetNamespace, relative: RelativePath) -> Result<Self, AssetError> {
        let path = Self { namespace, relative };
        path.validate()?;
        Ok(path)
    }
    fn validate(&self) -> Result<(), AssetError> {
        if self.relative.as_str().len() > 1024 || self.relative.as_str().split('/').count() > 16 {
            return Err(AssetError::Path);
        }
        if self.namespace == AssetNamespace::ProviderContent {
            let name = self.relative.as_str();
            let digest = [".part-", ".trash-", ".receipt-"]
                .into_iter()
                .find_map(|prefix| name.strip_prefix(prefix))
                .unwrap_or(name);
            Sha256::new(digest).map_err(|_| AssetError::Path)?;
        }
        Ok(())
    }
    pub fn namespace(&self) -> AssetNamespace {
        self.namespace
    }
    pub fn relative(&self) -> &RelativePath {
        &self.relative
    }
    pub fn home_relative(&self) -> Result<RelativePath, AssetError> {
        self.validate()?;
        RelativePath::new(format!("{}/{}", self.namespace.home_prefix(), self.relative.as_str()))
            .map_err(|_| AssetError::Path)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetRecord {
    pub path: AssetPath,
    pub kind: EntryKind,
}

/// A complete exact declaration for one provider transaction. All entries,
/// including absent destinations and private staging names, participate in the
/// digest. Successful parsing is byte-integrity evidence, never authorization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetInventory {
    schema_version: u16,
    principal: AccountPrincipal,
    uid: u32,
    gid: u32,
    entries: Vec<AssetRecord>,
}
impl AssetInventory {
    pub fn principal(&self) -> &AccountPrincipal {
        &self.principal
    }
    pub fn entries(&self) -> &[AssetRecord] {
        &self.entries
    }
    pub fn validate(&self) -> Result<(), AssetError> {
        if self.schema_version != 1
            || self.uid == 0
            || self.uid == u32::MAX
            || self.gid == u32::MAX
            || self.entries.is_empty()
            || self.entries.len() > MAX_CAPTURE_LIMITS.max_paths
        {
            return Err(AssetError::Bounds);
        }
        let mut paths = BTreeSet::new();
        let mut total = 0u64;
        for record in &self.entries {
            record.path.validate()?;
            if !paths.insert(record.path.clone()) {
                return Err(AssetError::Path);
            }
            let metadata = match &record.kind {
                EntryKind::Absent => continue,
                EntryKind::Directory { metadata } => metadata,
                EntryKind::RegularFile { metadata, size, hard_link_group, .. } => {
                    if *size > MAX_CAPTURE_LIMITS.max_file_bytes || hard_link_group.is_some() {
                        return Err(AssetError::Bounds);
                    }
                    total = total.checked_add(*size).ok_or(AssetError::Bounds)?;
                    metadata
                }
                EntryKind::Symlink { .. } => return Err(AssetError::Ownership),
            };
            if metadata.uid != self.uid
                || metadata.gid != self.gid
                || metadata.mode & !0o777 != 0
                || metadata.mode & 0o022 != 0
            {
                return Err(AssetError::Ownership);
            }
            if matches!(record.kind, EntryKind::RegularFile { .. }) && metadata.mode & 0o111 != 0 {
                return Err(AssetError::Ownership);
            }
        }
        if total > MAX_CAPTURE_LIMITS.max_total_bytes
            || self.entries.windows(2).any(|e| e[0].path >= e[1].path)
        {
            return Err(AssetError::Bounds);
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<Sha256, AssetError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| AssetError::Manifest)?;
        Sha256::new(format!("{:x}", Hash::digest(bytes))).map_err(|_| AssetError::Manifest)
    }
    pub fn require_account(&self, account: &CaptureAccount) -> Result<(), AssetError> {
        self.validate()?;
        if self.principal != account.principal || self.uid != account.uid || self.gid != account.gid
        {
            return Err(AssetError::Ownership);
        }
        Ok(())
    }
}

pub struct CapturedAssets {
    inventory: AssetInventory,
    metadata: Payloads,
}
impl CapturedAssets {
    pub fn inventory(&self) -> &AssetInventory {
        &self.inventory
    }
    pub fn metadata_payloads(&self) -> &Payloads {
        &self.metadata
    }

    pub(crate) fn revalidate(&self, home: &File) -> Result<(), AssetError> {
        let account = CaptureAccount {
            uid: self.inventory.uid,
            gid: self.inventory.gid,
            principal: self.inventory.principal.clone(),
        };
        let paths =
            self.inventory.entries.iter().map(|entry| entry.path.clone()).collect::<Vec<_>>();
        let current = capture_assets(home, &account, &paths, MAX_CAPTURE_LIMITS)?;
        if current.inventory != self.inventory || current.metadata != self.metadata {
            return Err(AssetError::Changed);
        }
        Ok(())
    }

    pub(crate) fn open_payloads(
        &self,
        home: &File,
    ) -> Result<std::collections::BTreeMap<Sha256, File>, AssetError> {
        self.revalidate(home)?;
        let mut payloads = std::collections::BTreeMap::new();
        for record in &self.inventory.entries {
            let EntryKind::RegularFile { metadata, size, sha256, .. } = &record.kind else {
                continue;
            };
            let path = record.path.home_relative()?;
            let file = File::from(fs::openat2(
                home,
                path.as_str(),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                ResolveFlags::BENEATH
                    | ResolveFlags::NO_SYMLINKS
                    | ResolveFlags::NO_MAGICLINKS
                    | ResolveFlags::NO_XDEV,
            )?);
            let stat = fs::fstat(&file)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
                || stat.st_nlink != 1
                || stat.st_uid != metadata.uid
                || stat.st_gid != metadata.gid
                || stat.st_mode & 0o7777 != metadata.mode
                || stat.st_size < 0
                || stat.st_size as u64 != *size
            {
                return Err(AssetError::Changed);
            }
            payloads.entry(sha256.clone()).or_insert(file);
        }
        Ok(payloads)
    }
}

#[derive(Debug)]
pub enum AssetError {
    Path,
    Bounds,
    Ownership,
    Changed,
    Manifest,
    Capture(CaptureError),
    Filesystem(rustix::io::Errno),
}
impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Path => "asset path is outside its declared provider namespace",
            Self::Bounds => "asset inventory exceeds its bound",
            Self::Ownership => "asset account or ownership mismatch",
            Self::Changed => "asset inventory changed during capture",
            Self::Manifest => "invalid asset inventory",
            Self::Capture(_) => "asset content or metadata capture failed",
            Self::Filesystem(_) => "asset filesystem check failed",
        })
    }
}
impl std::error::Error for AssetError {}
impl From<CaptureError> for AssetError {
    fn from(e: CaptureError) -> Self {
        Self::Capture(e)
    }
}
impl From<rustix::io::Errno> for AssetError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Filesystem(e)
    }
}

/// Two exact streaming captures bracket the metadata/content identity. No
/// directory traversal, payload buffering, directory creation or writes occur.
/// The native coordinator must keep its account-bound writer lease held.
pub fn capture_assets(
    home: &File,
    account: &CaptureAccount,
    declarations: &[AssetPath],
    limits: CaptureLimits,
) -> Result<CapturedAssets, AssetError> {
    check_home(home, account)?;
    if declarations.is_empty() || declarations.len() > MAX_CAPTURE_LIMITS.max_paths {
        return Err(AssetError::Bounds);
    }
    let mut sorted = declarations.to_vec();
    sorted.sort();
    if sorted.windows(2).any(|paths| paths[0] == paths[1]) {
        return Err(AssetError::Path);
    }
    let exact = sorted.iter().map(AssetPath::home_relative).collect::<Result<Vec<_>, _>>()?;
    let before = capture_ryogami_assets(home, &exact, limits)?;
    let after = capture_ryogami_assets(home, &exact, limits)?;
    check_home(home, account)?;
    if before.records != after.records || before.metadata_payloads != after.metadata_payloads {
        return Err(AssetError::Changed);
    }
    let mut entries = Vec::with_capacity(sorted.len());
    for path in sorted {
        let home_path = path.home_relative()?;
        let FileRecord { root, kind, .. } = before
            .records
            .iter()
            .find(|record| record.path == home_path)
            .ok_or(AssetError::Manifest)?
            .clone();
        if root != StateRoot::StateGeneration {
            return Err(AssetError::Manifest);
        }
        entries.push(AssetRecord { path, kind });
    }
    let inventory = AssetInventory {
        schema_version: 1,
        principal: account.principal.clone(),
        uid: account.uid,
        gid: account.gid,
        entries,
    };
    inventory.require_account(account)?;
    Ok(CapturedAssets { inventory, metadata: before.metadata_payloads })
}
fn check_home(home: &File, account: &CaptureAccount) -> Result<(), AssetError> {
    let stat = fs::fstat(home)?;
    if account.uid == 0
        || stat.st_uid != account.uid
        || stat.st_gid != account.gid
        || FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_mode & 0o022 != 0
    {
        return Err(AssetError::Ownership);
    }
    // Opening the already bound root with openat2 makes the no-symlink/no-cross-
    // mount policy explicit without choosing any path from a client.
    let checked = fs::openat2(
        home,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?;
    let checked = fs::fstat(checked)?;
    if (checked.st_dev, checked.st_ino) != (stat.st_dev, stat.st_ino) {
        return Err(AssetError::Changed);
    }
    Ok(())
}

#[cfg(test)]
#[path = "ryogami_assets_tests.rs"]
mod tests;
