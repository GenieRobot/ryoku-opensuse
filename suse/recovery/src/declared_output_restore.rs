//! Sparse output restoration: exact application entries, never a mixed tree.
//! Native authorization, writer leases and verified linked partners are caller
//! prerequisites. Private stages and immutable inode receipts precede live writes.
use crate::{
    AccountPrincipal, EntryKind, Identifier, RelativePath, Sha256,
    asset_restore::{RestoreError, parent},
    declared_outputs::{OutputDeclaration, OutputKind},
    generation::VerifiedGeneration,
    installed_capture::CaptureAccount,
    ryogami_assets::{AssetNamespace, AssetPath, AssetRecord, AssetScope, MAX_CAPTURE_LIMITS},
};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
};
type Result<T> = std::result::Result<T, RestoreError>;
const MAX_RECORD: usize = 16 * 1024 * 1024;
const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_XDEV)
    .union(ResolveFlags::NO_MAGICLINKS);
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    transaction: Identifier,
    generation: Identifier,
    manifest: Sha256,
}
impl Reference {
    fn new(source: &VerifiedGeneration) -> Result<Self> {
        let m = source.manifest();
        if m.root_snapshot_receipt().is_none()
            || m.assets().and_then(|a| a.output_declaration()).is_none()
        {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(Self {
            transaction: m.transaction_id.clone(),
            generation: m.generation_id.clone(),
            manifest: m.manifest_sha256.clone().ok_or(RestoreError::InvalidBinding)?,
        })
    }
}
fn hash(value: &impl Serialize) -> Result<Sha256> {
    Sha256::new(crate::wallpaper_protocol::digest(
        &serde_json::to_vec(value).map_err(|_| RestoreError::InvalidBinding)?,
    ))
    .map_err(|_| RestoreError::InvalidBinding)
}
fn outputs(source: &VerifiedGeneration) -> Result<(OutputDeclaration, Vec<AssetRecord>)> {
    let inventory = source.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
    inventory.validate().map_err(|_| RestoreError::InvalidBinding)?;
    let declaration = inventory.output_declaration().ok_or(RestoreError::InvalidBinding)?.clone();
    let entries = inventory
        .entries()
        .iter()
        .filter(|r| {
            matches!(
                r.path.namespace(),
                AssetNamespace::DeclaredOutput | AssetNamespace::OutputStage
            )
        })
        .cloned()
        .collect();
    Ok((declaration, entries))
}
fn directory_slot(declaration: &OutputDeclaration, target: &AssetRecord) -> Result<bool> {
    if target.path.namespace() == AssetNamespace::DeclaredOutput {
        Ok(matches!(
            declaration
                .entries()
                .iter()
                .find(|entry| entry.path == *target.path.relative())
                .ok_or(RestoreError::InvalidBinding)?
                .kind,
            OutputKind::Directory
        ))
    } else {
        let relative = target.path.relative().as_str();
        Ok(!relative.contains('/')
            || relative.rsplit('/').next().is_some_and(|leaf| leaf.starts_with("d-")))
    }
}
fn slot(root: &AssetPath, index: usize, directory: bool) -> Result<AssetPath> {
    let prefix = if directory { "d" } else { "f" };
    root.with_relative(
        RelativePath::new(format!("{}/{prefix}-{index}", root.relative().as_str()))
            .map_err(|_| RestoreError::InvalidBinding)?,
    )
    .map_err(|_| RestoreError::InvalidBinding)
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputRestoreIntent {
    operation: Identifier,
    source: Reference,
    declaration: OutputDeclaration,
    targets: Vec<AssetRecord>,
    stage: AssetPath,
    trash: AssetPath,
    scope: AssetScope,
}
impl OutputRestoreIntent {
    pub fn new(operation: Identifier, source: &VerifiedGeneration) -> Result<Self> {
        let reference = Reference::new(source)?;
        if operation == reference.transaction {
            return Err(RestoreError::InvalidBinding);
        }
        let (declaration, targets) = outputs(source)?;
        let token = hash(&(&operation, &reference, &declaration))?;
        let make = |prefix| {
            AssetPath::new(
                AssetNamespace::OutputStage,
                RelativePath::new(format!(".ryoku-output-{prefix}-{}", token.as_str()))
                    .map_err(|_| RestoreError::InvalidBinding)?,
            )
            .map_err(|_| RestoreError::InvalidBinding)
        };
        let stage = make("stage")?;
        let trash = make("trash")?;
        let mut paths = targets.iter().map(|record| record.path.clone()).collect::<Vec<_>>();
        paths.extend([stage.clone(), trash.clone()]);
        for (index, target) in targets.iter().enumerate() {
            let directory = directory_slot(&declaration, target)?;
            paths.push(slot(&stage, index, directory)?);
            paths.push(slot(&trash, index, directory)?);
        }
        let scope = AssetScope::new_with_outputs(
            paths,
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
            Some(declaration.clone()),
        )
        .map_err(|_| RestoreError::InvalidBinding)?;
        Ok(Self { operation, source: reference, declaration, targets, stage, trash, scope })
    }
    pub fn scope(&self) -> &AssetScope {
        &self.scope
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    path: Option<AssetPath>,
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}
fn pin(home: &File, path: Option<&AssetPath>, account: &CaptureAccount) -> Result<Pin> {
    let stat = if let Some(path) = path {
        let (parent, leaf) = parent(home, path, account)?;
        fs::statat(&parent, leaf.as_str(), AtFlags::SYMLINK_NOFOLLOW)?
    } else {
        fs::fstat(home)?
    };
    if stat.st_uid != account.uid || stat.st_gid != account.gid {
        return Err(RestoreError::InvalidBinding);
    }
    Ok(Pin {
        path: path.cloned(),
        device: stat.st_dev,
        inode: stat.st_ino,
        uid: stat.st_uid,
        gid: stat.st_gid,
        mode: stat.st_mode,
    })
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u16,
    operation: Identifier,
    source: Reference,
    checkpoint: Reference,
    account: AccountPrincipal,
    declaration: OutputDeclaration,
    targets: Vec<AssetRecord>,
    previous: Vec<AssetRecord>,
    scope: AssetScope,
    stage: AssetPath,
    trash: AssetPath,
    original_pins: Vec<Pin>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    version: u16,
    intent: Sha256,
    staged: Vec<Pin>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRestoreReceipt {
    operation: Identifier,
    intent: Sha256,
    ready: Sha256,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRestoreIdentity {
    pub operation: Identifier,
    pub intent: Sha256,
}
pub struct OutputRestorePlan {
    record: Record,
}
pub struct PreparedOutputRestore {
    record: Record,
    ready: Ready,
    receipt: OutputRestoreReceipt,
}
impl OutputRestorePlan {
    pub(crate) fn partial_cleanup(
        &self,
        operation: Identifier,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<Option<OutputRestoreIntent>> {
        if Reference::new(checkpoint)? != self.record.checkpoint
            || account.principal != self.record.account
        {
            return Err(RestoreError::InvalidBinding);
        }
        if record_exists(journal, &name(&self.record.operation, "ready"))? {
            return Ok(None);
        }
        let actual = self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        if !actual.inventory().entries().iter().any(|entry| {
            !self.record.targets.iter().any(|target| target.path == entry.path)
                && entry.kind != EntryKind::Absent
        }) {
            return Ok(None);
        }
        let saved: Record = read_record(journal, &name(&self.record.operation, "intent"))?;
        if saved != self.record {
            return Err(RestoreError::Changed);
        }
        for entry in &self.record.previous {
            if !actual.inventory().entries().contains(entry) {
                return Err(RestoreError::Changed);
            }
        }
        for original in &self.record.original_pins {
            if pin(home, original.path.as_ref(), account)? != *original {
                return Err(RestoreError::Changed);
            }
        }
        Ok(Some(OutputRestoreIntent::new(operation, checkpoint)?))
    }
    pub(crate) fn receipt_identity(&self) -> Result<OutputRestoreIdentity> {
        Ok(OutputRestoreIdentity {
            operation: self.record.operation.clone(),
            intent: hash(&self.record)?,
        })
    }
    pub(crate) fn matches_intent(&self, intent: &OutputRestoreIntent) -> bool {
        self.record.operation == intent.operation
            && self.record.source == intent.source
            && self.record.scope == intent.scope
            && self.record.targets == intent.targets
    }
    pub(crate) fn original_ready(&self, home: &File, account: &CaptureAccount) -> Result<()> {
        self.original(home, account)
    }
    pub(crate) fn record_exists(journal: &File, identity: &OutputRestoreIdentity) -> Result<bool> {
        record_exists(journal, &name(&identity.operation, "intent"))
    }
    pub(crate) fn completed(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<bool> {
        if !record_exists(journal, &name(&self.record.operation, "ready"))? {
            return Ok(false);
        }
        self.reopen_staged(source, checkpoint, account, journal)?.restored(home, account)
    }
    pub(crate) fn apply_or_resume(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<()> {
        let prepared = if record_exists(journal, &name(&self.record.operation, "ready"))? {
            self.reopen_staged(source, checkpoint, account, journal)?
        } else {
            self.stage(source, checkpoint, home, account, journal, &mut boundary)?
        };
        prepared.apply(source, checkpoint, home, account, journal, &mut boundary)?;
        Ok(())
    }
    pub fn reopen(
        operation: &Identifier,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<Self> {
        let record: Record = read_record(journal, &name(operation, "intent"))?;
        Self::bound(&record, source, checkpoint, account)?;
        if &record.operation != operation {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(Self { record })
    }
    pub fn reopen_staged(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<PreparedOutputRestore> {
        let ready: Ready = read_record(journal, &name(&self.record.operation, "ready"))?;
        let receipt = OutputRestoreReceipt {
            operation: self.record.operation.clone(),
            intent: hash(&self.record)?,
            ready: hash(&ready)?,
        };
        PreparedOutputRestore::reopen(source, checkpoint, account, journal, &receipt)
    }
    pub fn prepare(
        intent: &OutputRestoreIntent,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
    ) -> Result<Self> {
        if Reference::new(source)? != intent.source
            || Reference::new(source)? == Reference::new(checkpoint)?
        {
            return Err(RestoreError::InvalidBinding);
        }
        let source_inventory = source.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
        let current = checkpoint.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
        source_inventory.require_account(account).map_err(|_| RestoreError::InvalidBinding)?;
        current.require_account(account).map_err(|_| RestoreError::InvalidBinding)?;
        if current.output_declaration() != Some(&intent.declaration) {
            return Err(RestoreError::InvalidBinding);
        }
        let actual = intent.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        for entry in actual.inventory().entries() {
            if !current.entries().contains(entry)
                || (!intent.targets.iter().any(|target| target.path == entry.path)
                    && entry.kind != EntryKind::Absent)
            {
                return Err(RestoreError::Changed);
            }
        }
        let previous = intent
            .targets
            .iter()
            .map(|target| {
                actual
                    .inventory()
                    .entries()
                    .iter()
                    .find(|entry| entry.path == target.path)
                    .cloned()
                    .ok_or(RestoreError::InvalidBinding)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut original_pins = vec![pin(home, None, account)?];
        for entry in &previous {
            if entry.kind != EntryKind::Absent {
                original_pins.push(pin(home, Some(&entry.path), account)?);
            }
        }
        let result = Self {
            record: Record {
                version: 1,
                operation: intent.operation.clone(),
                source: intent.source.clone(),
                checkpoint: Reference::new(checkpoint)?,
                account: account.principal.clone(),
                declaration: intent.declaration.clone(),
                targets: intent.targets.clone(),
                previous,
                scope: intent.scope.clone(),
                stage: intent.stage.clone(),
                trash: intent.trash.clone(),
                original_pins,
            },
        };
        apply::validate_removals(&result.record, home, account)?;
        Ok(result)
    }
    /// Persist intent, prepare owned private inodes, then durably bind their
    /// identities. An interrupted incomplete stage is never silently adopted.
    pub fn stage(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<PreparedOutputRestore> {
        Self::bound(&self.record, source, checkpoint, account)?;
        self.original(home, account)?;
        check(&mut boundary)?;
        let intent_name = name(&self.record.operation, "intent");
        if record_exists(journal, &intent_name)? {
            let previous: Record = read_record(journal, &intent_name)?;
            if previous != self.record {
                return Err(RestoreError::Changed);
            }
        } else {
            write_record(journal, &intent_name, &self.record)?;
        }
        for root in [&self.record.stage, &self.record.trash] {
            check(&mut boundary)?;
            crate::account_inode::create(
                home,
                root.relative().as_str(),
                account,
                crate::account_inode::Kind::Directory,
            )?;
        }
        let stage_dir = open_directory(home, &self.record.stage)?;
        for (index, target) in self.record.targets.iter().enumerate() {
            check(&mut boundary)?;
            if target.kind == self.record.previous[index].kind {
                continue;
            }
            let stage = slot(
                &self.record.stage,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            let leaf =
                stage.relative().as_str().rsplit('/').next().ok_or(RestoreError::InvalidBinding)?;
            match &target.kind {
                EntryKind::Absent => {}
                EntryKind::RegularFile { .. } => {
                    let inode = crate::account_inode::create(
                        &stage_dir,
                        leaf,
                        account,
                        crate::account_inode::Kind::File,
                    )?;
                    source.prepare_asset_inode(&target.path, account, &inode, &mut boundary)?;
                }
                EntryKind::Directory { .. } => {
                    let inode = crate::account_inode::create(
                        &stage_dir,
                        leaf,
                        account,
                        crate::account_inode::Kind::Directory,
                    )?;
                    source.prepare_asset_directory_metadata(&target.path, account, &inode)?;
                }
                EntryKind::Symlink { target: link, .. } => {
                    crate::account_inode::create(
                        &stage_dir,
                        leaf,
                        account,
                        crate::account_inode::Kind::Symlink(link),
                    )?;
                    source.prepare_asset_symlink_metadata(
                        &target.path,
                        account,
                        &stage_dir,
                        &RelativePath::new(leaf).map_err(|_| RestoreError::InvalidBinding)?,
                    )?;
                }
            }
        }
        check(&mut boundary)?;
        let actual = self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        let mut staged = vec![
            pin(home, Some(&self.record.stage), account)?,
            pin(home, Some(&self.record.trash), account)?,
        ];
        for (index, target) in self.record.targets.iter().enumerate() {
            let stage = slot(
                &self.record.stage,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            let trash = slot(
                &self.record.trash,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            if !actual.inventory().entries().iter().any(|entry| {
                entry.path == stage
                    && entry.kind
                        == if target.kind == self.record.previous[index].kind {
                            EntryKind::Absent
                        } else {
                            target.kind.clone()
                        }
            }) || !actual
                .inventory()
                .entries()
                .iter()
                .any(|entry| entry.path == trash && entry.kind == EntryKind::Absent)
                || !actual.inventory().entries().contains(&self.record.previous[index])
            {
                return Err(RestoreError::Changed);
            }
            if target.kind != EntryKind::Absent && target.kind != self.record.previous[index].kind {
                staged.push(pin(home, Some(&stage), account)?);
            }
        }
        for original in &self.record.original_pins {
            if pin(home, original.path.as_ref(), account)? != *original {
                return Err(RestoreError::Changed);
            }
        }
        verify_private_children(home, &self.record, account)?;
        let ready = Ready { version: 1, intent: hash(&self.record)?, staged };
        write_record(journal, &name(&self.record.operation, "ready"), &ready)?;
        check(&mut boundary)?;
        let receipt = OutputRestoreReceipt {
            operation: self.record.operation.clone(),
            intent: ready.intent.clone(),
            ready: hash(&ready)?,
        };
        Ok(PreparedOutputRestore { record: self.record.clone(), ready, receipt })
    }
    fn bound(
        record: &Record,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        account: &CaptureAccount,
    ) -> Result<()> {
        if record.version != 1
            || record.account != account.principal
            || record.source != Reference::new(source)?
            || record.checkpoint != Reference::new(checkpoint)?
        {
            return Err(RestoreError::InvalidBinding);
        }
        let intent = OutputRestoreIntent::new(record.operation.clone(), source)?;
        if record.declaration != intent.declaration
            || record.targets != intent.targets
            || record.scope != intent.scope
            || record.stage != intent.stage
            || record.trash != intent.trash
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
        let inventory = checkpoint.manifest().assets().ok_or(RestoreError::InvalidBinding)?;
        let previous = record
            .targets
            .iter()
            .map(|target| {
                inventory
                    .entries()
                    .iter()
                    .find(|entry| entry.path == target.path)
                    .cloned()
                    .ok_or(RestoreError::InvalidBinding)
            })
            .collect::<Result<Vec<_>>>()?;
        if previous != record.previous
            || inventory.output_declaration() != Some(&record.declaration)
        {
            return Err(RestoreError::InvalidBinding);
        }
        let expected = std::iter::once(None)
            .chain(
                record
                    .previous
                    .iter()
                    .filter(|r| r.kind != EntryKind::Absent)
                    .map(|r| Some(&r.path)),
            )
            .collect::<Vec<_>>();
        if record.original_pins.iter().map(|p| p.path.as_ref()).ne(expected) {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(())
    }
    fn original(&self, home: &File, account: &CaptureAccount) -> Result<()> {
        let current =
            self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        if current.inventory().entries().iter().any(|entry| {
            if self.record.previous.iter().any(|previous| previous.path == entry.path) {
                !self.record.previous.contains(entry)
            } else {
                entry.kind != EntryKind::Absent
            }
        }) {
            return Err(RestoreError::Changed);
        }
        for original in &self.record.original_pins {
            if pin(home, original.path.as_ref(), account)? != *original {
                return Err(RestoreError::Changed);
            }
        }
        Ok(())
    }
}
impl PreparedOutputRestore {
    pub fn receipt(&self) -> &OutputRestoreReceipt {
        &self.receipt
    }
    pub fn reopen(
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        account: &CaptureAccount,
        journal: &File,
        receipt: &OutputRestoreReceipt,
    ) -> Result<Self> {
        let record: Record = read_record(journal, &name(&receipt.operation, "intent"))?;
        let ready: Ready = read_record(journal, &name(&receipt.operation, "ready"))?;
        OutputRestorePlan::bound(&record, source, checkpoint, account)?;
        if record.operation != receipt.operation
            || hash(&record)? != receipt.intent
            || hash(&ready)? != receipt.ready
            || ready.version != 1
            || ready.intent != receipt.intent
        {
            return Err(RestoreError::Changed);
        }
        let mut expected = vec![record.stage.clone(), record.trash.clone()];
        for (index, target) in record.targets.iter().enumerate() {
            if target.kind != EntryKind::Absent && target.kind != record.previous[index].kind {
                expected.push(slot(
                    &record.stage,
                    index,
                    directory_slot(&record.declaration, &record.targets[index])?,
                )?);
            }
        }
        if ready.staged.iter().map(|p| p.path.as_ref()).ne(expected.iter().map(Some)) {
            return Err(RestoreError::InvalidBinding);
        }
        Ok(Self { record, ready, receipt: receipt.clone() })
    }
    pub fn verify_unpublished(&self, home: &File, account: &CaptureAccount) -> Result<()> {
        let current =
            self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        for original in &self.record.original_pins {
            if pin(home, original.path.as_ref(), account)? != *original {
                return Err(RestoreError::Changed);
            }
        }
        for staged in &self.ready.staged {
            if pin(home, staged.path.as_ref(), account)? != *staged {
                return Err(RestoreError::Changed);
            }
        }
        for (index, target) in self.record.targets.iter().enumerate() {
            let stage = slot(
                &self.record.stage,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            let trash = slot(
                &self.record.trash,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            if !current
                .inventory()
                .entries()
                .iter()
                .any(|entry| entry.path == trash && entry.kind == EntryKind::Absent)
            {
                return Err(RestoreError::Changed);
            }
            if !current.inventory().entries().contains(&self.record.previous[index])
                || !current.inventory().entries().iter().any(|entry| {
                    entry.path == stage
                        && entry.kind
                            == if target.kind == self.record.previous[index].kind {
                                EntryKind::Absent
                            } else {
                                target.kind.clone()
                            }
                })
            {
                return Err(RestoreError::Changed);
            }
        }
        verify_private_children(home, &self.record, account)?;
        Ok(())
    }
}
fn verify_private_children(home: &File, record: &Record, account: &CaptureAccount) -> Result<()> {
    for root in [&record.stage, &record.trash] {
        let directory = open_directory(home, root)?;
        for entry in fs::Dir::read_from(&directory)? {
            let entry = entry?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            let leaf = std::str::from_utf8(bytes).map_err(|_| RestoreError::Changed)?;
            let path = root
                .with_relative(
                    RelativePath::new(format!("{}/{leaf}", root.relative().as_str()))
                        .map_err(|_| RestoreError::Changed)?,
                )
                .map_err(|_| RestoreError::Changed)?;
            if !record.scope.paths().contains(&path) {
                return Err(RestoreError::Changed);
            }
            let stat = fs::statat(&directory, leaf, AtFlags::SYMLINK_NOFOLLOW)?;
            if stat.st_uid != account.uid || stat.st_gid != account.gid {
                return Err(RestoreError::Changed);
            }
            if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                let child = open_directory(home, &path)?;
                for child_entry in fs::Dir::read_from(&child)? {
                    let child_entry = child_entry?;
                    if !matches!(child_entry.file_name().to_bytes(), b"." | b"..") {
                        return Err(RestoreError::Changed);
                    }
                }
            }
        }
    }
    Ok(())
}
fn check(boundary: &mut impl FnMut() -> bool) -> Result<()> {
    if boundary() { Ok(()) } else { Err(RestoreError::Interrupted) }
}
fn open_directory(home: &File, path: &AssetPath) -> Result<File> {
    Ok(File::from(fs::openat2(
        home,
        path.home_relative().map_err(|_| RestoreError::InvalidBinding)?.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        NO_LINKS,
    )?))
}
fn name(operation: &Identifier, phase: &str) -> String {
    format!("output-restore-{}-{phase}.json", operation.as_str())
}
fn private(journal: &File) -> Result<()> {
    let stat = fs::fstat(journal)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7777 != 0o700
    {
        Err(RestoreError::InvalidBinding)
    } else {
        Ok(())
    }
}
fn write_record(journal: &File, name: &str, value: &impl Serialize) -> Result<()> {
    private(journal)?;
    let bytes = serde_json::to_vec(value).map_err(|_| RestoreError::InvalidBinding)?;
    if bytes.len() > MAX_RECORD {
        return Err(RestoreError::InvalidBinding);
    }
    let mut file = File::from(fs::openat(
        journal,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    file.write_all(&bytes)?;
    fs::fchmod(&file, Mode::from_raw_mode(0o400))?;
    file.sync_all()?;
    fs::fsync(journal)?;
    if read_bytes(journal, name)? != bytes {
        return Err(RestoreError::Changed);
    }
    Ok(())
}
fn read_bytes(journal: &File, name: &str) -> Result<Vec<u8>> {
    private(journal)?;
    let file = File::from(fs::openat2(
        journal,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        NO_LINKS,
    )?);
    let stat = fs::fstat(&file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o7777 != 0o400
        || stat.st_nlink != 1
        || stat.st_size < 0
        || stat.st_size as usize > MAX_RECORD
    {
        return Err(RestoreError::Changed);
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() != stat.st_size as usize || bytes.len() > MAX_RECORD {
        return Err(RestoreError::Changed);
    }
    Ok(bytes)
}
fn read_record<T: serde::de::DeserializeOwned>(journal: &File, name: &str) -> Result<T> {
    serde_json::from_slice(&read_bytes(journal, name)?).map_err(|_| RestoreError::InvalidBinding)
}

#[path = "declared_output_restore_apply.rs"]
mod apply;

fn record_exists(journal: &File, name: &str) -> Result<bool> {
    private(journal)?;
    match fs::statat(journal, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
