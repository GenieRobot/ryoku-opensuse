//! Complete owned-tree restoration with protected intent and retained originals.
//! This engine does not authorize callers or verify native Snapper topology.
//! The coordinator supplies both independently verified partners, an account
//! writer lease, a durable applying phase and complete post-capture afterward.
use crate::{
    AccountPrincipal, EntryKind, Identifier, RelativePath, Sha256,
    asset_restore::{RestoreError, parent},
    generation::VerifiedGeneration,
    installed_capture::CaptureAccount,
    ryogami_assets::{
        AssetInventory, AssetNamespace, AssetPath, AssetRecord, AssetScope, MAX_CAPTURE_LIMITS,
    },
};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
};
type Result<T> = std::result::Result<T, RestoreError>;
const MAX_RECORD: usize = 8 * 1024 * 1024;
const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    transaction: Identifier,
    generation: Identifier,
    manifest: Sha256,
}
impl Reference {
    fn new(generation: &VerifiedGeneration) -> Result<Self> {
        let manifest = generation.manifest();
        if manifest.root_snapshot_receipt().is_none() || manifest.assets().is_none() {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(Self {
            transaction: manifest.transaction_id.clone(),
            generation: manifest.generation_id.clone(),
            manifest: manifest.manifest_sha256.clone().ok_or(RestoreError::InvalidBinding)?,
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentPin {
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}
fn pin(file: &File) -> Result<ParentPin> {
    let stat = fs::fstat(file)?;
    Ok(ParentPin {
        device: stat.st_dev,
        inode: stat.st_ino,
        uid: stat.st_uid,
        gid: stat.st_gid,
        mode: stat.st_mode,
    })
}
fn digest(bytes: &[u8]) -> Result<Sha256> {
    Sha256::new(crate::wallpaper_protocol::digest(bytes)).map_err(|_| RestoreError::InvalidBinding)
}
fn suffix<'a>(path: &'a AssetPath, root: &AssetPath) -> Result<Option<&'a str>> {
    if root.namespace() != path.namespace() {
        return Ok(None);
    }
    if root.with_relative(path.relative().clone()).map_err(|_| RestoreError::InvalidBinding)?
        != *path
    {
        return Ok(None);
    }
    if path == root {
        return Ok(Some(""));
    }
    Ok(path
        .relative()
        .as_str()
        .strip_prefix(root.relative().as_str())
        .and_then(|tail| tail.strip_prefix('/')))
}
fn mapped(path: &AssetPath, root: &AssetPath, destination: &AssetPath) -> Result<AssetPath> {
    let tail = suffix(path, root)?.ok_or(RestoreError::InvalidBinding)?;
    if tail.is_empty() {
        return Ok(destination.clone());
    }
    destination
        .with_relative(
            RelativePath::new(format!("{}/{}", destination.relative().as_str(), tail))
                .map_err(|_| RestoreError::InvalidBinding)?,
        )
        .map_err(|_| RestoreError::InvalidBinding)
}
fn records(inventory: &AssetInventory, root: &AssetPath) -> Result<Vec<AssetRecord>> {
    if !inventory.trees().contains(root) {
        return Err(RestoreError::InvalidBinding);
    }
    inventory
        .entries()
        .iter()
        .filter_map(|record| match suffix(&record.path, root) {
            Ok(Some(_)) => Some(Ok(record.clone())),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}
fn roots(
    operation: &Identifier,
    source: &Reference,
    root: &AssetPath,
) -> Result<(AssetPath, AssetPath)> {
    let hash = digest(
        &serde_json::to_vec(&(operation, source, root))
            .map_err(|_| RestoreError::InvalidBinding)?,
    )?;
    let path = |label| {
        AssetPath::new(
            AssetNamespace::TreeStage,
            RelativePath::new(format!(".ryoku-tree-{label}-{}", hash.as_str()))
                .map_err(|_| RestoreError::InvalidBinding)?,
        )
        .map_err(|_| RestoreError::InvalidBinding)
    };
    Ok((path("stage")?, path("trash")?))
}
fn scope(
    root: &AssetPath,
    stage: &AssetPath,
    trash: &AssetPath,
    wanted: &[AssetRecord],
    current: &[AssetPath],
) -> Result<AssetScope> {
    let mut targets: BTreeSet<_> = wanted.iter().map(|entry| entry.path.clone()).collect();
    targets.extend(current.iter().cloned());
    let mut paths = targets.clone();
    for path in &targets {
        paths.insert(mapped(path, root, stage)?);
    }
    for path in current {
        paths.insert(mapped(path, root, trash)?);
    }
    AssetScope::new(
        paths.into_iter().collect(),
        MAX_CAPTURE_LIMITS.max_file_bytes,
        MAX_CAPTURE_LIMITS.max_total_bytes,
    )
    .and_then(|scope| scope.with_trees(vec![root.clone(), stage.clone(), trash.clone()]))
    .map_err(|_| RestoreError::InvalidBinding)
}

#[derive(Clone)]
pub struct TreeRestoreIntent {
    operation: Identifier,
    source: Reference,
    root: AssetPath,
    stage: AssetPath,
    trash: AssetPath,
    scope: AssetScope,
}
impl TreeRestoreIntent {
    /// Read-only current-tree discovery. Every source/current path and both
    /// generated tree locations must be captured before plan preparation.
    pub fn new(
        operation: Identifier,
        source: &VerifiedGeneration,
        root: &AssetPath,
        home: &File,
        account: &CaptureAccount,
    ) -> Result<Self> {
        let reference = Reference::new(source)?;
        if reference.transaction == operation {
            return Err(RestoreError::InvalidBinding);
        }
        let inventory = source.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
        inventory.require_account(account).map_err(|_| RestoreError::InvalidBinding)?;
        let wanted = records(inventory, root)?;
        let current = crate::ryogami_trees::inspect_scope(
            home,
            account,
            root.clone(),
            vec![],
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
            || true,
        )
        .map_err(|_| RestoreError::Changed)?;
        let (stage, trash) = roots(&operation, &reference, root)?;
        let scope = scope(root, &stage, &trash, &wanted, current.paths())?;
        let captured = scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        for path in [&stage, &trash] {
            if !captured
                .inventory()
                .entries()
                .iter()
                .any(|entry| entry.path == *path && entry.kind == EntryKind::Absent)
            {
                return Err(RestoreError::Changed);
            }
        }
        Ok(Self { operation, source: reference, root: root.clone(), stage, trash, scope })
    }
    pub fn scope(&self) -> &AssetScope {
        &self.scope
    }
    pub fn target(&self) -> &AssetPath {
        &self.root
    }
    pub fn staging(&self) -> &AssetPath {
        &self.stage
    }
    pub fn trash(&self) -> &AssetPath {
        &self.trash
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u16,
    operation: Identifier,
    principal: AccountPrincipal,
    source: Reference,
    checkpoint: Reference,
    root: AssetPath,
    stage: AssetPath,
    trash: AssetPath,
    scope: AssetScope,
    parent: ParentPin,
    home: ParentPin,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeRestoreReceipt {
    pub operation: Identifier,
    pub record_sha256: Sha256,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeObservation {
    Original,
    Staged,
    Swapped,
    Restored,
}
pub struct TreeRestorePlan {
    record: Record,
    wanted: Vec<AssetRecord>,
    previous: Vec<AssetRecord>,
}
impl TreeRestorePlan {
    pub fn prepare(
        intent: &TreeRestoreIntent,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
    ) -> Result<Self> {
        let record = Record {
            schema: 1,
            operation: intent.operation.clone(),
            principal: account.principal.clone(),
            source: intent.source.clone(),
            checkpoint: Reference::new(checkpoint)?,
            root: intent.root.clone(),
            stage: intent.stage.clone(),
            trash: intent.trash.clone(),
            scope: intent.scope.clone(),
            parent: pin(&parent(home, &intent.root, account)?.0)?,
            home: pin(home)?,
        };
        let plan = Self::bound(record, source, checkpoint, account)?;
        match plan.observe(home, account)? {
            TreeObservation::Original => (),
            TreeObservation::Restored if plan.noop() => (),
            _ => return Err(RestoreError::Changed),
        }
        Ok(plan)
    }
    fn bound(
        record: Record,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        account: &CaptureAccount,
    ) -> Result<Self> {
        if record.schema != 1
            || record.principal != account.principal
            || record.source != Reference::new(source)?
            || record.checkpoint != Reference::new(checkpoint)?
            || record.operation == record.source.transaction
            || record.source.transaction == record.checkpoint.transaction
            || record.source.generation == record.checkpoint.generation
            || record.source.manifest == record.checkpoint.manifest
            || source.manifest().snapper_snapshot_id == checkpoint.manifest().snapper_snapshot_id
            || source.manifest().filesystem_uuid != checkpoint.manifest().filesystem_uuid
            || source.manifest().snapper_config != checkpoint.manifest().snapper_config
        {
            return Err(RestoreError::InvalidBinding);
        }
        for generation in [source, checkpoint] {
            generation
                .manifest()
                .assets()
                .ok_or(RestoreError::InvalidBinding)?
                .require_account(account)
                .map_err(|_| RestoreError::InvalidBinding)?;
        }
        let before = checkpoint.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
        let wanted =
            records(source.manifest().assets().ok_or(RestoreError::InvalidBinding)?, &record.root)?;
        let previous = records(before, &record.root)?;
        let (stage, trash) = roots(&record.operation, &record.source, &record.root)?;
        let current: Vec<_> = previous
            .iter()
            .filter(|entry| entry.path == record.root || entry.kind != EntryKind::Absent)
            .map(|entry| entry.path.clone())
            .collect();
        if stage != record.stage
            || trash != record.trash
            || scope(&record.root, &stage, &trash, &wanted, &current)? != record.scope
        {
            return Err(RestoreError::InvalidBinding);
        }
        for root in [&stage, &trash] {
            if records(before, root)?.iter().any(|entry| entry.kind != EntryKind::Absent) {
                return Err(RestoreError::InvalidBinding);
            }
        }
        if record
            .scope
            .paths()
            .iter()
            .any(|path| !before.entries().iter().any(|entry| entry.path == *path))
        {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(Self { record, wanted, previous })
    }
    fn noop(&self) -> bool {
        self.wanted
            .iter()
            .filter(|e| e.kind != EntryKind::Absent)
            .eq(self.previous.iter().filter(|e| e.kind != EntryKind::Absent))
    }
    pub fn receipt_identity(&self) -> Result<TreeRestoreReceipt> {
        Ok(TreeRestoreReceipt {
            operation: self.record.operation.clone(),
            record_sha256: digest(
                &serde_json::to_vec(&self.record).map_err(|_| RestoreError::InvalidBinding)?,
            )?,
        })
    }
    pub fn reopen(
        receipt: &TreeRestoreReceipt,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<Self> {
        let bytes = read_record(journal, &receipt.operation)?;
        if digest(&bytes)? != receipt.record_sha256 {
            return Err(RestoreError::InvalidBinding);
        }
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| RestoreError::InvalidBinding)?;
        if record.operation != receipt.operation {
            return Err(RestoreError::InvalidBinding);
        }
        let plan = Self::bound(record, source, checkpoint, account)?;
        plan.observe(home, account)?;
        Ok(plan)
    }
    fn parents(&self, home: &File, account: &CaptureAccount) -> Result<(File, String)> {
        let result = parent(home, &self.record.root, account)?;
        if pin(&result.0)? != self.record.parent || pin(home)? != self.record.home {
            return Err(RestoreError::Changed);
        }
        Ok(result)
    }
    fn matches(
        &self,
        actual: &[AssetRecord],
        location: &AssetPath,
        expected: &[AssetRecord],
    ) -> Result<bool> {
        for entry in actual {
            if suffix(&entry.path, location)?.is_none() {
                continue;
            }
            let original = mapped(&entry.path, location, &self.record.root)?;
            let kind = expected
                .iter()
                .find(|e| e.path == original)
                .map(|e| &e.kind)
                .unwrap_or(&EntryKind::Absent);
            if &entry.kind != kind {
                return Ok(false);
            }
        }
        for entry in expected.iter().filter(|entry| entry.kind != EntryKind::Absent) {
            let path = mapped(&entry.path, &self.record.root, location)?;
            if !actual
                .iter()
                .any(|candidate| candidate.path == path && candidate.kind == entry.kind)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn observe(&self, home: &File, account: &CaptureAccount) -> Result<TreeObservation> {
        self.parents(home, account)?;
        let capture =
            self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        let actual = capture.inventory().entries();
        let absent = Vec::new();
        let target_previous = self.matches(actual, &self.record.root, &self.previous)?;
        let target_wanted = self.matches(actual, &self.record.root, &self.wanted)?;
        let stage_absent = self.matches(actual, &self.record.stage, &absent)?;
        let trash_absent = self.matches(actual, &self.record.trash, &absent)?;
        if target_wanted
            && stage_absent
            && (self.matches(actual, &self.record.trash, &self.previous)?
                || (self.noop() && trash_absent))
        {
            return Ok(TreeObservation::Restored);
        }
        if target_previous && trash_absent {
            return Ok(if stage_absent {
                TreeObservation::Original
            } else {
                TreeObservation::Staged
            });
        }
        if target_wanted
            && trash_absent
            && self.matches(actual, &self.record.stage, &self.previous)?
        {
            return Ok(TreeObservation::Swapped);
        }
        Err(RestoreError::Changed)
    }
}
fn private_journal(directory: &File) -> Result<()> {
    let s = fs::fstat(directory)?;
    if FileType::from_raw_mode(s.st_mode) != FileType::Directory
        || s.st_uid != rustix::process::geteuid().as_raw()
        || s.st_mode & 0o7777 != 0o700
    {
        return Err(RestoreError::InvalidBinding);
    }
    Ok(())
}
fn record_name(operation: &Identifier) -> String {
    format!("tree-restore-{}.json", operation.as_str())
}
fn read_record(journal: &File, operation: &Identifier) -> Result<Vec<u8>> {
    private_journal(journal)?;
    let mut file = File::from(fs::openat2(
        journal,
        record_name(operation).as_str(),
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        NO_LINKS,
    )?);
    let stat = fs::fstat(&file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != 0o400
        || stat.st_size < 0
        || stat.st_size as usize > MAX_RECORD
    {
        return Err(RestoreError::Changed);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take((MAX_RECORD + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() != stat.st_size as usize || bytes.len() > MAX_RECORD {
        return Err(RestoreError::Changed);
    }
    Ok(bytes)
}
fn write_record(journal: &File, record: &Record) -> Result<()> {
    private_journal(journal)?;
    let bytes = serde_json::to_vec(record).map_err(|_| RestoreError::InvalidBinding)?;
    if bytes.len() > MAX_RECORD {
        return Err(RestoreError::InvalidBinding);
    }
    let mut file = File::from(fs::openat(
        journal,
        record_name(&record.operation).as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    file.write_all(&bytes)?;
    fs::fchmod(&file, Mode::from_raw_mode(0o400))?;
    file.sync_all()?;
    fs::fsync(journal)?;
    if read_record(journal, &record.operation)? != bytes {
        return Err(RestoreError::Changed);
    }
    Ok(())
}

impl TreeRestorePlan {
    /// Write the protected record before any destination creation. Completed
    /// local bytes still need the caller's verified native post-checkpoint.
    pub fn apply(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<TreeRestoreReceipt> {
        Self::bound(self.record.clone(), source, checkpoint, account)?;
        let observed = self.observe(home, account)?;
        if observed != TreeObservation::Original
            && !(self.noop() && observed == TreeObservation::Restored)
        {
            return Err(RestoreError::Changed);
        }
        if !boundary() {
            return Err(RestoreError::Interrupted);
        }
        self.check_space(home)?;
        write_record(journal, &self.record)?;
        if !boundary() {
            return Err(RestoreError::Interrupted);
        }
        self.finish(source, home, account, &mut boundary)
    }
    /// Continue only a journal-only start, an exact completed exchange, or a
    /// fully verified result. Partial stage reuse needs separately checkpointed
    /// cleanup; no unknown/incomplete tree is overwritten or recursively removed.
    pub fn resume(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<TreeRestoreReceipt> {
        Self::bound(self.record.clone(), source, checkpoint, account)?;
        if read_record(journal, &self.record.operation)?
            != serde_json::to_vec(&self.record).map_err(|_| RestoreError::InvalidBinding)?
        {
            return Err(RestoreError::Changed);
        }
        if self.observe(home, account)? == TreeObservation::Staged {
            return Err(RestoreError::Interrupted);
        }
        if !boundary() {
            return Err(RestoreError::Interrupted);
        }
        self.check_space(home)?;
        self.finish(source, home, account, &mut boundary)
    }
    fn check_space(&self, home: &File) -> Result<()> {
        let bytes = self
            .wanted
            .iter()
            .try_fold(16 * 1024 * 1024u64, |sum, entry| {
                let size = match entry.kind {
                    EntryKind::RegularFile { size, .. } => size,
                    _ => 0,
                };
                sum.checked_add(size)
            })
            .ok_or(RestoreError::InvalidBinding)?;
        let space = fs::fstatvfs(home)?;
        if space.f_bavail.saturating_mul(space.f_frsize) < bytes {
            return Err(rustix::io::Errno::NOSPC.into());
        }
        Ok(())
    }
    fn build(
        &self,
        source: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        boundary: &mut impl FnMut() -> bool,
    ) -> Result<()> {
        if !boundary() {
            return Err(RestoreError::Interrupted);
        }
        let stage = crate::account_inode::create(
            home,
            self.record.stage.relative().as_str(),
            account,
            crate::account_inode::Kind::Directory,
        )?;
        let mut entries: Vec<_> = self
            .wanted
            .iter()
            .filter(|entry| entry.path != self.record.root && entry.kind != EntryKind::Absent)
            .collect();
        entries.sort_by_key(|entry| {
            (entry.path.relative().as_str().split('/').count(), entry.path.clone())
        });
        for entry in &entries {
            if !boundary() {
                return Err(RestoreError::Interrupted);
            }
            let tail =
                suffix(&entry.path, &self.record.root)?.ok_or(RestoreError::InvalidBinding)?;
            let (parent_path, leaf) = tail.rsplit_once('/').unwrap_or(("", tail));
            let parent = if parent_path.is_empty() {
                stage.try_clone()?
            } else {
                File::from(fs::openat2(
                    &stage,
                    parent_path,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    NO_LINKS,
                )?)
            };
            match entry.kind {
                EntryKind::Directory { .. } => {
                    crate::account_inode::create(
                        &parent,
                        leaf,
                        account,
                        crate::account_inode::Kind::Directory,
                    )?;
                }
                EntryKind::RegularFile { .. } => {
                    let file = crate::account_inode::create(
                        &parent,
                        leaf,
                        account,
                        crate::account_inode::Kind::File,
                    )?;
                    source.prepare_asset_inode(&entry.path, account, &file, &mut *boundary)?;
                }
                _ => return Err(RestoreError::InvalidBinding),
            }
        }
        // Finalize directories bottom-up, after child names and contents are
        // durable. This preserves directory timestamps, ACLs and labels.
        for entry in entries
            .iter()
            .rev()
            .copied()
            .chain(self.wanted.iter().filter(|entry| entry.path == self.record.root))
        {
            if !matches!(entry.kind, EntryKind::Directory { .. }) {
                continue;
            }
            if !boundary() {
                return Err(RestoreError::Interrupted);
            }
            let tail =
                suffix(&entry.path, &self.record.root)?.ok_or(RestoreError::InvalidBinding)?;
            let directory = if tail.is_empty() {
                stage.try_clone()?
            } else {
                File::from(fs::openat2(
                    &stage,
                    tail,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    NO_LINKS,
                )?)
            };
            source.prepare_asset_directory_metadata(&entry.path, account, &directory)?;
        }
        fs::fsync(home)?;
        Ok(())
    }
    fn finish(
        &self,
        source: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        boundary: &mut impl FnMut() -> bool,
    ) -> Result<TreeRestoreReceipt> {
        let mut observed = self.observe(home, account)?;
        if observed == TreeObservation::Restored {
            return self.receipt_identity();
        }
        let (parent, leaf) = self.parents(home, account)?;
        let stage = self.record.stage.relative().as_str();
        let trash = self.record.trash.relative().as_str();
        if observed == TreeObservation::Original {
            let wanted_root = self
                .wanted
                .iter()
                .find(|entry| entry.path == self.record.root)
                .ok_or(RestoreError::InvalidBinding)?;
            if matches!(wanted_root.kind, EntryKind::Directory { .. }) {
                self.build(source, home, account, boundary)?;
                let captured =
                    self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
                if self.observe(home, account)? != TreeObservation::Staged
                    || !self.matches(
                        captured.inventory().entries(),
                        &self.record.stage,
                        &self.wanted,
                    )?
                {
                    return Err(RestoreError::Changed);
                }
                self.parents(home, account)?;
                if !boundary() {
                    return Err(RestoreError::Interrupted);
                }
                let previous_absent = self
                    .previous
                    .iter()
                    .any(|entry| entry.path == self.record.root && entry.kind == EntryKind::Absent);
                fs::renameat_with(
                    home,
                    stage,
                    &parent,
                    leaf.as_str(),
                    if previous_absent {
                        fs::RenameFlags::NOREPLACE
                    } else {
                        fs::RenameFlags::EXCHANGE
                    },
                )?;
                fs::fsync(home)?;
                fs::fsync(&parent)?;
            } else if wanted_root.kind == EntryKind::Absent {
                if !boundary() {
                    return Err(RestoreError::Interrupted);
                }
                fs::renameat_with(&parent, leaf.as_str(), home, trash, fs::RenameFlags::NOREPLACE)?;
                fs::fsync(&parent)?;
                fs::fsync(home)?;
            } else {
                return Err(RestoreError::InvalidBinding);
            }
            if !boundary() {
                return Err(RestoreError::Interrupted);
            }
            observed = self.observe(home, account)?;
        }
        if observed == TreeObservation::Swapped {
            self.parents(home, account)?;
            if !boundary() {
                return Err(RestoreError::Interrupted);
            }
            fs::renameat_with(home, stage, home, trash, fs::RenameFlags::NOREPLACE)?;
            fs::fsync(home)?;
            if !boundary() {
                return Err(RestoreError::Interrupted);
            }
        }
        if self.observe(home, account)? != TreeObservation::Restored {
            return Err(RestoreError::Changed);
        }
        self.receipt_identity()
    }
}
