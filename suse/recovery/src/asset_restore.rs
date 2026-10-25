//! Exact-file asset restoration with an immutable write-ahead record.
//!
//! This storage engine is not an authorization or native-snapshot verifier.
//! The helper must hold its authenticated account writer lease, independently
//! verify both native linked partners, and record Applying before invoking it.
//! Its returned receipt is local readback only; success still needs the native
//! coordinator's complete post-checkpoint and protected operation result.
use crate::{
    AccountPrincipal, EntryKind, Identifier, RelativePath, Sha256,
    generation::{GenerationError, VerifiedGeneration},
    installed_capture::CaptureAccount,
    ryogami_assets::{AssetPath, AssetRecord, AssetScope, MAX_CAPTURE_LIMITS, capture_assets},
};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Hash};
use std::{
    fs::File,
    io::{Read, Write},
};

const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub enum RestoreError {
    InvalidBinding,
    Changed,
    Interrupted,
    Filesystem(rustix::io::Errno),
    Io(std::io::Error),
    Generation(GenerationError),
}
impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBinding => "asset restore binding is invalid",
            Self::Changed => "asset restore state changed",
            Self::Interrupted => {
                "asset restore interrupted; retained checkpoint and journal are required"
            }
            Self::Filesystem(_) | Self::Io(_) => "asset restore local storage failed",
            Self::Generation(_) => "asset restore generation verification failed",
        })
    }
}
impl std::error::Error for RestoreError {}
impl From<rustix::io::Errno> for RestoreError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Filesystem(e)
    }
}
impl From<std::io::Error> for RestoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<GenerationError> for RestoreError {
    fn from(e: GenerationError) -> Self {
        Self::Generation(e)
    }
}
type Result<T> = std::result::Result<T, RestoreError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    transaction: Identifier,
    generation: Identifier,
    manifest: Sha256,
}
impl Reference {
    fn from_generation(generation: &VerifiedGeneration) -> Result<Self> {
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

/// Constructed before capture, so the generated staging name is also enrolled
/// as absent in the fresh linked checkpoint. Only regular/absent file targets
/// are supported by this exact-file engine; directory operations are separate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRestoreIntent {
    operation: Identifier,
    source: Reference,
    target: AssetRecord,
    stage: AssetPath,
}
impl FileRestoreIntent {
    pub fn new(
        operation: Identifier,
        source: &VerifiedGeneration,
        path: &AssetPath,
    ) -> Result<Self> {
        let reference = Reference::from_generation(source)?;
        let target = source
            .manifest()
            .assets()
            .and_then(|assets| assets.entries().iter().find(|entry| entry.path == *path))
            .cloned()
            .ok_or(RestoreError::InvalidBinding)?;
        if !matches!(target.kind, EntryKind::RegularFile { .. } | EntryKind::Absent) {
            return Err(RestoreError::InvalidBinding);
        }
        let identity = serde_json::to_vec(&(&operation, &reference, path))
            .map_err(|_| RestoreError::InvalidBinding)?;
        let leaf = if path.namespace() == crate::ryogami_assets::AssetNamespace::ModelReferenceStage
        {
            format!(".ryoku-model-{:x}.part", Hash::digest(identity))
        } else {
            format!(".part-{:x}", Hash::digest(identity))
        };
        let relative = path
            .relative()
            .as_str()
            .rsplit_once('/')
            .map_or_else(|| leaf.clone(), |(parent, _)| format!("{parent}/{leaf}"));
        let stage = path
            .with_relative(RelativePath::new(relative).map_err(|_| RestoreError::InvalidBinding)?)
            .map_err(|_| RestoreError::InvalidBinding)?;
        if stage == *path {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(Self { operation, source: reference, target, stage })
    }
    pub fn scope(&self) -> Result<AssetScope> {
        AssetScope::new(
            vec![self.target.path.clone(), self.stage.clone()],
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_file_bytes * 2,
        )
        .map_err(|_| RestoreError::InvalidBinding)
    }
    pub fn target(&self) -> &AssetPath {
        &self.target.path
    }
    pub fn staging(&self) -> &AssetPath {
        &self.stage
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreRecord {
    schema: u16,
    operation: Identifier,
    principal: AccountPrincipal,
    source: Reference,
    checkpoint: Reference,
    target: AssetRecord,
    previous: AssetRecord,
    stage: AssetPath,
    parent: ParentPin,
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
fn parent_pin(file: &File) -> Result<ParentPin> {
    let stat = fs::fstat(file)?;
    Ok(ParentPin {
        device: stat.st_dev,
        inode: stat.st_ino,
        uid: stat.st_uid,
        gid: stat.st_gid,
        mode: stat.st_mode,
    })
}

/// Store this identity in the protected native operation result. A stage name
/// or a user-owned copy of the record never authorizes recovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRestoreReceipt {
    pub operation: Identifier,
    pub record_sha256: Sha256,
}

/// Truthful local observation; Restored is not native operation completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreObservation {
    Original,
    Staged,
    Restored,
}

pub struct FileRestorePlan {
    record: RestoreRecord,
}
impl FileRestorePlan {
    /// Read-only: prove original source, fresh checkpoint, exact target/stage
    /// coverage and ordinary-account ownership before any record or stage write.
    pub fn prepare(
        intent: &FileRestoreIntent,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
    ) -> Result<Self> {
        if Reference::from_generation(source)? != intent.source {
            return Err(RestoreError::InvalidBinding);
        }
        let before = checkpoint.manifest();
        let wanted = source.manifest();
        let reference = Reference::from_generation(checkpoint)?;
        if reference.transaction == intent.source.transaction
            || reference.generation == intent.source.generation
            || reference.manifest == intent.source.manifest
            || before.snapper_snapshot_id == wanted.snapper_snapshot_id
            || before.filesystem_uuid != wanted.filesystem_uuid
            || before.snapper_config != wanted.snapper_config
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
        let assets = before.assets().ok_or(RestoreError::InvalidBinding)?;
        let previous = assets
            .entries()
            .iter()
            .find(|entry| entry.path == intent.target.path)
            .cloned()
            .ok_or(RestoreError::InvalidBinding)?;
        if !matches!(previous.kind, EntryKind::RegularFile { .. } | EntryKind::Absent)
            || !assets
                .entries()
                .iter()
                .any(|entry| entry.path == intent.stage && entry.kind == EntryKind::Absent)
        {
            return Err(RestoreError::InvalidBinding);
        }
        let plan = Self {
            record: RestoreRecord {
                schema: 1,
                operation: intent.operation.clone(),
                principal: account.principal.clone(),
                source: intent.source.clone(),
                checkpoint: reference,
                target: intent.target.clone(),
                previous,
                stage: intent.stage.clone(),
                parent: parent_pin(&parent(home, &intent.target.path, account)?.0)?,
            },
        };
        if plan.observe(home, account)? != RestoreObservation::Original {
            return Err(RestoreError::Changed);
        }
        Ok(plan)
    }
    pub fn receipt_identity(&self) -> Result<FileRestoreReceipt> {
        let bytes = serde_json::to_vec(&self.record).map_err(|_| RestoreError::InvalidBinding)?;
        Ok(FileRestoreReceipt {
            operation: self.record.operation.clone(),
            record_sha256: digest(&bytes)?,
        })
    }

    /// Read-only restart inspection from the protected record. Neither a
    /// partially written stage nor Restored permits new writes without fresh
    /// native authorization/checkpointing. Recovery may select the original
    /// preimage's absent stage as a separately checkpointed cleanup operation.
    pub fn reopen(
        receipt: &FileRestoreReceipt,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<Self> {
        validate_journal(journal)?;
        let name = format!("asset-restore-{}.json", receipt.operation.as_str());
        let bytes = read_record(journal, &name)?;
        if digest(&bytes)? != receipt.record_sha256 {
            return Err(RestoreError::InvalidBinding);
        }
        let record: RestoreRecord =
            serde_json::from_slice(&bytes).map_err(|_| RestoreError::InvalidBinding)?;
        if record.schema != 1
            || record.operation != receipt.operation
            || record.principal != account.principal
            || Reference::from_generation(source)? != record.source
            || Reference::from_generation(checkpoint)? != record.checkpoint
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
        let intent = FileRestoreIntent::new(record.operation.clone(), source, &record.target.path)?;
        let before = checkpoint.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
        if intent.target != record.target
            || intent.stage != record.stage
            || !before.entries().contains(&record.previous)
            || record.previous.path != record.target.path
            || !before
                .entries()
                .iter()
                .any(|entry| entry.path == record.stage && entry.kind == EntryKind::Absent)
        {
            return Err(RestoreError::InvalidBinding);
        }
        let plan = Self { record };
        plan.observe(home, account)?;
        Ok(plan)
    }

    pub fn observe(&self, home: &File, account: &CaptureAccount) -> Result<RestoreObservation> {
        if self.record.principal != account.principal
            || parent_pin(&parent(home, &self.record.target.path, account)?.0)?
                != self.record.parent
        {
            return Err(RestoreError::InvalidBinding);
        }
        let actual = capture_assets(
            home,
            account,
            &[self.record.target.path.clone(), self.record.stage.clone()],
            MAX_CAPTURE_LIMITS,
        )
        .map_err(|_| RestoreError::Changed)?;
        let target = actual
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.path == self.record.target.path)
            .ok_or(RestoreError::Changed)?;
        let stage = actual
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.path == self.record.stage)
            .ok_or(RestoreError::Changed)?;
        if stage.kind == EntryKind::Absent {
            if *target == self.record.previous {
                return Ok(RestoreObservation::Original);
            }
            if *target == self.record.target {
                return Ok(RestoreObservation::Restored);
            }
        } else if *target == self.record.previous
            && let (
                EntryKind::RegularFile { size, .. },
                EntryKind::RegularFile { size: maximum, .. },
            ) = (&stage.kind, &self.record.target.kind)
            && size <= maximum
        {
            return Ok(RestoreObservation::Staged);
        }
        Err(RestoreError::Changed)
    }

    /// The caller has already durably entered Applying and retains a verified
    /// current-state checkpoint. The journal directory must be its protected
    /// coordinator directory. This method never deletes a partial stage on
    /// failure; its exact checkpointed name and immutable record remain for
    /// explicit authenticated recovery, without claiming automatic completion.
    pub fn apply(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<FileRestoreReceipt> {
        if !boundary() {
            return Err(RestoreError::Interrupted);
        }
        if Reference::from_generation(source)? != self.record.source
            || Reference::from_generation(checkpoint)? != self.record.checkpoint
            || self.observe(home, account)? != RestoreObservation::Original
        {
            return Err(RestoreError::Changed);
        }
        validate_journal(journal)?;
        let bytes = serde_json::to_vec(&self.record).map_err(|_| RestoreError::InvalidBinding)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(RestoreError::InvalidBinding);
        }
        let receipt = self.receipt_identity()?;
        let journal_name = format!("asset-restore-{}.json", self.record.operation.as_str());
        write_record(journal, &journal_name, &bytes)?;
        if !boundary() || self.observe(home, account)? != RestoreObservation::Original {
            return Err(RestoreError::Interrupted);
        }
        let (parent, leaf) = parent(home, &self.record.target.path, account)?;
        if parent_pin(&parent)? != self.record.parent {
            return Err(RestoreError::Changed);
        }
        let stage_leaf = self
            .record
            .stage
            .relative()
            .as_str()
            .rsplit('/')
            .next()
            .ok_or(RestoreError::InvalidBinding)?;
        match &self.record.target.kind {
            EntryKind::RegularFile { .. } => {
                let stage = File::from(fs::openat(
                    &parent,
                    stage_leaf,
                    OFlags::RDWR
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::from_raw_mode(0o600),
                )?);
                fs::fchmod(&stage, Mode::from_raw_mode(0o600))?;
                fs::fchown(
                    &stage,
                    Some(fs::Uid::from_raw(account.uid)),
                    Some(fs::Gid::from_raw(account.gid)),
                )?;
                source.prepare_asset_inode(
                    &self.record.target.path,
                    account,
                    &stage,
                    &mut boundary,
                )?;
                // The lease freezes live writers; repeat the full target proof
                // before atomic publication and never replace a directory/link.
                if !boundary() || self.observe(home, account)? != RestoreObservation::Staged {
                    return Err(RestoreError::Interrupted);
                }
                let staged = capture_assets(
                    home,
                    account,
                    std::slice::from_ref(&self.record.stage),
                    MAX_CAPTURE_LIMITS,
                )
                .map_err(|_| RestoreError::Changed)?;
                if staged.inventory().entries()[0].kind != self.record.target.kind {
                    return Err(RestoreError::Changed);
                }
                if self.record.previous.kind == EntryKind::Absent {
                    fs::renameat_with(
                        &parent,
                        stage_leaf,
                        &parent,
                        leaf.as_str(),
                        fs::RenameFlags::NOREPLACE,
                    )?;
                } else {
                    fs::renameat(&parent, stage_leaf, &parent, leaf.as_str())?;
                }
                fs::fsync(&parent)?;
            }
            EntryKind::Absent => {
                if self.record.previous.kind != EntryKind::Absent {
                    fs::unlinkat(&parent, leaf.as_str(), AtFlags::empty())?;
                }
                fs::fsync(&parent)?;
            }
            _ => return Err(RestoreError::InvalidBinding),
        }
        if !boundary() {
            return Err(RestoreError::Interrupted);
        }
        // Equal before/after bytes are also a successful verified no-op.
        let actual = capture_assets(
            home,
            account,
            &[self.record.target.path.clone(), self.record.stage.clone()],
            MAX_CAPTURE_LIMITS,
        )
        .map_err(|_| RestoreError::Changed)?;
        if !actual.inventory().entries().contains(&self.record.target)
            || !actual
                .inventory()
                .entries()
                .iter()
                .any(|entry| entry.path == self.record.stage && entry.kind == EntryKind::Absent)
        {
            return Err(RestoreError::Changed);
        }
        Ok(receipt)
    }
}

fn digest(bytes: &[u8]) -> Result<Sha256> {
    Sha256::new(format!("{:x}", Hash::digest(bytes))).map_err(|_| RestoreError::InvalidBinding)
}
fn validate_journal(directory: &File) -> Result<()> {
    let stat = fs::fstat(directory)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(RestoreError::InvalidBinding);
    }
    Ok(())
}
fn write_record(parent: &File, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = File::from(fs::openat(
        parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    file.write_all(bytes)?;
    fs::fchmod(&file, Mode::from_raw_mode(0o400))?;
    file.sync_all()?;
    fs::fsync(parent)?;
    if read_record(parent, name)? != bytes {
        return Err(RestoreError::Changed);
    }
    Ok(())
}
fn read_record(parent: &File, name: &str) -> Result<Vec<u8>> {
    let mut saved = File::from(fs::openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        NO_LINKS,
    )?);
    let stat = fs::fstat(&saved)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7777 != 0o400
        || stat.st_size < 0
        || stat.st_size as usize > MAX_RECORD_BYTES
    {
        return Err(RestoreError::Changed);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut saved).take((MAX_RECORD_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD_BYTES || bytes.len() != stat.st_size as usize {
        return Err(RestoreError::Changed);
    }
    Ok(bytes)
}

fn parent(home: &File, path: &AssetPath, account: &CaptureAccount) -> Result<(File, String)> {
    let relative = path.home_relative().map_err(|_| RestoreError::InvalidBinding)?;
    let (parents, leaf) = relative.as_str().rsplit_once('/').unwrap_or(("", relative.as_str()));
    let mut current = home.try_clone()?;
    let root = fs::fstat(&current)?;
    if FileType::from_raw_mode(root.st_mode) != FileType::Directory
        || root.st_uid != account.uid
        || root.st_gid != account.gid
        || root.st_mode & 0o7022 != 0
    {
        return Err(RestoreError::InvalidBinding);
    }
    for component in parents.split('/').filter(|component| !component.is_empty()) {
        current = File::from(fs::openat2(
            &current,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            NO_LINKS,
        )?);
        let stat = fs::fstat(&current)?;
        if stat.st_uid != account.uid || stat.st_gid != account.gid || stat.st_mode & 0o7022 != 0 {
            return Err(RestoreError::InvalidBinding);
        }
    }
    Ok((current, leaf.to_owned()))
}

#[cfg(test)]
#[path = "asset_restore_tests.rs"]
mod tests;
