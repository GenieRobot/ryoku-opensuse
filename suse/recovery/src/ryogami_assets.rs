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
    ConfiguredLibrary,
    ModelReferenceStage,
    TreeStage,
    MediaTrash,
    CatalogStage,
    WallpaperCache,
    DeclaredOutput,
    OutputStage,
}
impl AssetNamespace {
    pub fn home_prefix(self) -> &'static str {
        match self {
            Self::ProviderContent => ".local/share/ryogami/providers",
            Self::SemanticModels => ".local/share/ryogami/lens/models",
            Self::RemotePreviews => ".cache/ryogami/remote-preview",
            Self::RemoteThumbnails => ".cache/ryogami/remote",
            Self::WorkshopContent => ".local/share/ryogami/workshop",
            Self::ConfiguredLibrary
            | Self::ModelReferenceStage
            | Self::TreeStage
            | Self::MediaTrash
            | Self::CatalogStage
            | Self::DeclaredOutput
            | Self::OutputStage => "",
            Self::WallpaperCache => ".cache/ryogami/wallpaper",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetPath {
    namespace: AssetNamespace,
    relative: RelativePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    library_root: Option<RelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_binding: Option<Sha256>,
}
impl AssetPath {
    pub fn new(namespace: AssetNamespace, relative: RelativePath) -> Result<Self, AssetError> {
        let path = Self { namespace, relative, library_root: None, output_binding: None };
        path.validate()?;
        Ok(path)
    }
    /// Trusted provider construction only: root comes from the authenticated
    /// account's inspected settings/session, never from a request field. The
    /// provider must verify exact library membership before enrolling a file.
    pub fn for_library(root: RelativePath, relative: RelativePath) -> Result<Self, AssetError> {
        let path = Self {
            namespace: AssetNamespace::ConfiguredLibrary,
            relative,
            library_root: Some(root),
            output_binding: None,
        };
        path.validate()?;
        Ok(path)
    }
    pub(crate) fn for_declared_output(
        binding: Sha256,
        relative: RelativePath,
    ) -> Result<Self, AssetError> {
        let path = Self {
            namespace: AssetNamespace::DeclaredOutput,
            relative,
            library_root: None,
            output_binding: Some(binding),
        };
        path.validate()?;
        Ok(path)
    }
    pub(crate) fn output_binding(&self) -> Option<&Sha256> {
        self.output_binding.as_ref()
    }
    /// Preserve the verified namespace/root when deriving an exact staging name.
    pub fn with_relative(&self, relative: RelativePath) -> Result<Self, AssetError> {
        let path = Self { relative, ..self.clone() };
        path.validate()?;
        Ok(path)
    }
    fn validate(&self) -> Result<(), AssetError> {
        if (self.namespace == AssetNamespace::DeclaredOutput) != self.output_binding.is_some() {
            return Err(AssetError::Path);
        }
        match (self.namespace, &self.library_root) {
            (AssetNamespace::ConfiguredLibrary, Some(root))
                if library_root_allowed(root)
                    && root.as_str().len() <= 1024
                    && root.as_str().split('/').count() <= 16 => {}
            (AssetNamespace::ConfiguredLibrary, _) | (_, Some(_)) => return Err(AssetError::Path),
            (_, None) => {}
        }
        if self.relative.as_str().len() > 1024 || self.relative.as_str().split('/').count() > 16 {
            return Err(AssetError::Path);
        }
        if self.namespace == AssetNamespace::OutputStage {
            let (root, leaf) = self
                .relative
                .as_str()
                .split_once('/')
                .map_or((self.relative.as_str(), None), |(root, leaf)| (root, Some(leaf)));
            let token = root
                .strip_prefix(".ryoku-output-stage-")
                .or_else(|| root.strip_prefix(".ryoku-output-trash-"))
                .ok_or(AssetError::Path)?;
            Sha256::new(token).map_err(|_| AssetError::Path)?;
            if let Some(leaf) = leaf {
                let (kind, index) = leaf.split_once('-').ok_or(AssetError::Path)?;
                let number: usize = index.parse().map_err(|_| AssetError::Path)?;
                if !matches!(kind, "d" | "f")
                    || number >= MAX_CAPTURE_LIMITS.max_paths
                    || index != number.to_string()
                {
                    return Err(AssetError::Path);
                }
            }
        }
        if self.namespace == AssetNamespace::CatalogStage {
            let token = self
                .relative
                .as_str()
                .strip_prefix(".ryoku-catalog-")
                .and_then(|name| name.strip_suffix(".part"))
                .ok_or(AssetError::Path)?;
            Sha256::new(token).map_err(|_| AssetError::Path)?;
        }
        if self.namespace == AssetNamespace::MediaTrash {
            let (token, index) = self
                .relative
                .as_str()
                .strip_prefix(".ryoku-media-trash-")
                .and_then(|name| name.rsplit_once('-'))
                .ok_or(AssetError::Path)?;
            Sha256::new(token).map_err(|_| AssetError::Path)?;
            let index: u8 = index.parse().map_err(|_| AssetError::Path)?;
            if index > 3 || self.relative.as_str() != format!(".ryoku-media-trash-{token}-{index}")
            {
                return Err(AssetError::Path);
            }
        }
        if self.namespace == AssetNamespace::WallpaperCache {
            let (directory, leaf) =
                self.relative.as_str().split_once('/').ok_or(AssetError::Path)?;
            if !matches!(directory, "thumbs" | "thumbs-sm" | "video-thumbs" | "anim")
                || leaf.is_empty()
                || leaf.contains('/')
            {
                return Err(AssetError::Path);
            }
        }
        if self.namespace == AssetNamespace::TreeStage {
            let leaf = self.relative.as_str().split('/').next().ok_or(AssetError::Path)?;
            let token = leaf
                .strip_prefix(".ryoku-tree-stage-")
                .or_else(|| leaf.strip_prefix(".ryoku-tree-trash-"))
                .ok_or(AssetError::Path)?;
            Sha256::new(token).map_err(|_| AssetError::Path)?;
        }
        if self.namespace == AssetNamespace::ModelReferenceStage {
            let digest = self
                .relative
                .as_str()
                .strip_prefix(".ryoku-model-")
                .and_then(|name| name.strip_suffix(".part"))
                .ok_or(AssetError::Path)?;
            Sha256::new(digest).map_err(|_| AssetError::Path)?;
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
        let prefix = self
            .library_root
            .as_ref()
            .map(RelativePath::as_str)
            .unwrap_or_else(|| self.namespace.home_prefix());
        if prefix.is_empty() {
            return Ok(self.relative.clone());
        }
        RelativePath::new(format!("{}/{}", prefix, self.relative.as_str()))
            .map_err(|_| AssetError::Path)
    }
}

fn library_root_allowed(root: &RelativePath) -> bool {
    let value = root.as_str();
    if value == ".local/share/backgrounds"
        || value.starts_with(".local/share/backgrounds/")
        || value == ".local/share/wallpapers"
        || value.starts_with(".local/share/wallpapers/")
    {
        return true;
    }
    ![
        ".ssh",
        ".gnupg",
        ".aws",
        ".azure",
        ".config",
        ".cache",
        ".local",
        ".codex",
        ".mozilla",
        ".pki",
        ".password-store",
        ".kube",
        ".docker",
        ".npm",
        ".cargo",
        ".rustup",
        ".steam",
        ".var",
    ]
    .contains(&value.split('/').next().unwrap_or_default())
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trees: Vec<AssetPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outputs: Option<crate::declared_outputs::OutputDeclaration>,
}
impl AssetInventory {
    pub fn output_declaration(&self) -> Option<&crate::declared_outputs::OutputDeclaration> {
        self.outputs.as_ref()
    }
    pub fn trees(&self) -> &[AssetPath] {
        &self.trees
    }
    pub fn principal(&self) -> &AccountPrincipal {
        &self.principal
    }
    pub fn entries(&self) -> &[AssetRecord] {
        &self.entries
    }
    pub fn validate(&self) -> Result<(), AssetError> {
        if self.schema_version
            != if self.outputs.is_some() {
                3
            } else if self.trees.is_empty() {
                1
            } else {
                2
            }
            || self.uid == 0
            || self.uid == u32::MAX
            || self.gid == u32::MAX
            || self.entries.is_empty()
            || self.entries.len() > MAX_CAPTURE_LIMITS.max_paths
        {
            return Err(AssetError::Bounds);
        }
        crate::declared_outputs::validate_scope(
            self.outputs.as_ref(),
            &self.entries.iter().map(|record| record.path.clone()).collect::<Vec<_>>(),
        )?;
        if let Some(outputs) = &self.outputs {
            outputs.require_account(&CaptureAccount {
                uid: self.uid,
                gid: self.gid,
                principal: self.principal.clone(),
            })?;
        }
        let mut paths = BTreeSet::new();
        let mut home_paths = BTreeSet::new();
        let mut total = 0u64;
        for record in &self.entries {
            record.path.validate()?;
            if !paths.insert(record.path.clone())
                || !home_paths.insert(record.path.home_relative()?)
            {
                return Err(AssetError::Path);
            }
            if record.path.namespace() == AssetNamespace::DeclaredOutput {
                self.outputs.as_ref().ok_or(AssetError::Path)?.validate_record(record)?;
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
                EntryKind::Symlink { metadata, target }
                    if record.path.namespace() == AssetNamespace::DeclaredOutput
                        || (record.path.namespace() == AssetNamespace::OutputStage
                            && self
                                .outputs
                                .as_ref()
                                .is_some_and(|outputs| outputs.allows_link_target(target))) =>
                {
                    metadata
                }
                EntryKind::Symlink { .. } => return Err(AssetError::Ownership),
            };
            if metadata.uid != self.uid
                || metadata.gid != self.gid
                || metadata.mode & !0o777 != 0
                || (!matches!(record.kind, EntryKind::Symlink { .. }) && metadata.mode & 0o022 != 0)
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
        if let Some(outputs) = &self.outputs {
            outputs.validate_topology(&self.entries)?;
        }
        crate::ryogami_trees::validate_inventory(self)?;
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

/// Frozen server-reviewed scope and byte reservation for both pre/post captures.
/// It is not a wire authorization token. Exact paths and bounds participate in
/// the reviewed plan fingerprint so a provider cannot change scope after apply.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetScope {
    paths: Vec<AssetPath>,
    max_file_bytes: u64,
    max_total_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trees: Vec<AssetPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outputs: Option<crate::declared_outputs::OutputDeclaration>,
}
impl AssetScope {
    pub fn output_declaration(&self) -> Option<&crate::declared_outputs::OutputDeclaration> {
        self.outputs.as_ref()
    }
    pub fn new(
        paths: Vec<AssetPath>,
        max_file_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<Self, AssetError> {
        Self::new_with_outputs(paths, max_file_bytes, max_total_bytes, None)
    }
    pub(crate) fn new_with_outputs(
        mut paths: Vec<AssetPath>,
        max_file_bytes: u64,
        max_total_bytes: u64,
        outputs: Option<crate::declared_outputs::OutputDeclaration>,
    ) -> Result<Self, AssetError> {
        paths.sort();
        let scope = Self { paths, max_file_bytes, max_total_bytes, trees: vec![], outputs };
        scope.validate()?;
        Ok(scope)
    }
    pub fn validate(&self) -> Result<(), AssetError> {
        if self.paths.is_empty()
            || self.paths.len() > MAX_CAPTURE_LIMITS.max_paths
            || self.paths.windows(2).any(|pair| pair[0] >= pair[1])
            || self.max_file_bytes == 0
            || self.max_file_bytes > MAX_CAPTURE_LIMITS.max_file_bytes
            || self.max_total_bytes < self.max_file_bytes
            || self.max_total_bytes > MAX_CAPTURE_LIMITS.max_total_bytes
        {
            return Err(AssetError::Bounds);
        }
        for path in &self.paths {
            path.validate()?;
        }
        crate::declared_outputs::validate_scope(self.outputs.as_ref(), &self.paths)?;
        crate::ryogami_trees::validate_roots(&self.trees, &self.paths)?;
        Ok(())
    }
    pub fn trees(&self) -> &[AssetPath] {
        &self.trees
    }
    pub(crate) fn with_trees(mut self, mut trees: Vec<AssetPath>) -> Result<Self, AssetError> {
        trees.sort();
        self.trees = trees;
        self.validate()?;
        Ok(self)
    }
    pub(crate) fn from_inventory(inventory: &AssetInventory) -> Result<Self, AssetError> {
        inventory.validate()?;
        Self::new_with_outputs(
            inventory.entries().iter().map(|entry| entry.path.clone()).collect(),
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
            inventory.outputs.clone(),
        )?
        .with_trees(inventory.trees.clone())
    }
    pub fn paths(&self) -> &[AssetPath] {
        &self.paths
    }
    pub fn limits(&self) -> CaptureLimits {
        CaptureLimits {
            max_paths: self.paths.len(),
            max_file_bytes: self.max_file_bytes,
            max_total_bytes: self.max_total_bytes,
        }
    }
    pub fn capture(
        &self,
        home: &File,
        account: &CaptureAccount,
    ) -> Result<CapturedAssets, AssetError> {
        self.validate()?;
        crate::ryogami_trees::verify_declared(home, account, self)?;
        crate::declared_outputs::verify_stage_membership(home, account, &self.paths)?;
        let mut captured = capture_assets_with_outputs(
            home,
            account,
            &self.paths,
            self.limits(),
            self.outputs.clone(),
        )?;
        captured.inventory.trees = self.trees.clone();
        captured.inventory.schema_version = if self.outputs.is_some() {
            3
        } else if self.trees.is_empty() {
            1
        } else {
            2
        };
        captured.inventory.validate()?;
        crate::ryogami_trees::verify_inventory(home, account, &captured.inventory)?;
        crate::declared_outputs::verify_stage_membership(home, account, &self.paths)?;
        Ok(captured)
    }
    pub fn bind_fingerprint(
        &self,
        preferences: &Sha256,
        assets: &AssetInventory,
    ) -> Result<Sha256, AssetError> {
        self.validate()?;
        assets.validate()?;
        if assets.entries().iter().map(|entry| &entry.path).ne(self.paths.iter())
            || assets.trees() != self.trees()
            || assets.outputs != self.outputs
        {
            return Err(AssetError::Path);
        }
        let bytes =
            serde_json::to_vec(&(preferences, self, assets)).map_err(|_| AssetError::Manifest)?;
        Sha256::new(format!("{:x}", Hash::digest(bytes))).map_err(|_| AssetError::Manifest)
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
        let current = AssetScope::from_inventory(&self.inventory)?.capture(home, &account)?;
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
    Interrupted,
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
            Self::Interrupted => "asset tree enrollment interrupted",
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
    capture_assets_with_outputs(home, account, declarations, limits, None)
}
fn capture_assets_with_outputs(
    home: &File,
    account: &CaptureAccount,
    declarations: &[AssetPath],
    limits: CaptureLimits,
    outputs: Option<crate::declared_outputs::OutputDeclaration>,
) -> Result<CapturedAssets, AssetError> {
    crate::declared_outputs::validate_scope(outputs.as_ref(), declarations)?;
    if let Some(outputs) = &outputs {
        outputs.require_account(account)?;
    }
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
        schema_version: if outputs.is_some() { 3 } else { 1 },
        principal: account.principal.clone(),
        uid: account.uid,
        gid: account.gid,
        entries,
        trees: vec![],
        outputs,
    };
    inventory.require_account(account)?;
    Ok(CapturedAssets { inventory, metadata: before.metadata_payloads })
}
pub(crate) fn check_home(home: &File, account: &CaptureAccount) -> Result<(), AssetError> {
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
