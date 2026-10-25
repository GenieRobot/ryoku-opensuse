//! Durable coordination for the private preference subset and exact streamed
//! asset plans. Public native authorization remains outside this storage layer.
use super::*;
use crate::{
    asset_restore::{FileRestoreReceipt, RestoreObservation},
    configuration_apply::{ApplyControl, ConfigurationDestination, apply_subset},
    declared_output_restore::OutputRestoreIdentity,
    disk_stage::DiskStageStore,
    generation_capture::DeclaredRoot,
    restore::{RestoreJournal, RestoreJournalStore, RestorePhase, RestorePlan},
    tree_restore::{TreeObservation, TreeRestoreReceipt},
};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    ops::ControlFlow,
};
const MAX_RECORD: usize = 1024 * 1024;

pub struct CombinedStores<'a> {
    pub stages: &'a DiskStageStore,
    pub journals: &'a RestoreJournalStore,
    /// Protected coordinator directory outside all rollback scopes, never home.
    pub assets: &'a File,
}
pub struct CombinedExecution<'a> {
    pub source: &'a VerifiedGeneration,
    pub checkpoint: &'a VerifiedGeneration,
    pub home: &'a File,
    pub account: &'a CaptureAccount,
    pub destinations: &'a [ConfigurationDestination<'a>],
    pub checkpoint_roots: &'a [DeclaredRoot<'a>],
}
#[derive(Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u16,
    operation: Identifier,
    account: AccountPrincipal,
    plan: RestorePlan,
    checkpoint_generation: Identifier,
    checkpoint_manifest: Sha256,
    assets: Vec<FileRestoreReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trees: Vec<TreeRestoreReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outputs: Option<OutputRestoreIdentity>,
}
fn record_name(operation: &Identifier) -> String {
    format!("combined-restore-{}.json", operation.as_str())
}
fn private_directory(file: &File) -> Result<()> {
    let st = fs::fstat(file).map_err(|_| CombinedError::Record)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::Directory
        || st.st_uid != rustix::process::geteuid().as_raw()
        || st.st_mode & 0o7777 != 0o700
    {
        return Err(CombinedError::Record);
    }
    Ok(())
}
fn read_record(directory: &File, operation: &Identifier) -> Result<Record> {
    private_directory(directory)?;
    let mut file = File::from(
        fs::openat2(
            directory,
            record_name(operation).as_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            ResolveFlags::BENEATH
                | ResolveFlags::NO_SYMLINKS
                | ResolveFlags::NO_MAGICLINKS
                | ResolveFlags::NO_XDEV,
        )
        .map_err(|_| CombinedError::Record)?,
    );
    let st = fs::fstat(&file).map_err(|_| CombinedError::Record)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile
        || st.st_nlink != 1
        || st.st_uid != rustix::process::geteuid().as_raw()
        || st.st_mode & 0o7777 != 0o400
        || st.st_size < 0
        || st.st_size as usize > MAX_RECORD
    {
        return Err(CombinedError::Record);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_RECORD + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CombinedError::Record)?;
    if bytes.len() > MAX_RECORD || bytes.len() != st.st_size as usize {
        return Err(CombinedError::Record);
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| CombinedError::Record)?;
    if record.schema
        != if record.outputs.is_some() {
            3
        } else if record.trees.is_empty() {
            1
        } else {
            2
        }
        || record.operation != *operation
    {
        return Err(CombinedError::Record);
    }
    Ok(record)
}
fn write_record(directory: &File, record: &Record) -> Result<()> {
    private_directory(directory)?;
    let bytes = serde_json::to_vec(record).map_err(|_| CombinedError::Record)?;
    if bytes.len() > MAX_RECORD {
        return Err(CombinedError::Record);
    }
    let mut file = File::from(
        fs::openat(
            directory,
            record_name(&record.operation).as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| CombinedError::Record)?,
    );
    file.write_all(&bytes).map_err(|_| CombinedError::Record)?;
    fs::fchmod(&file, Mode::from_raw_mode(0o400)).map_err(|_| CombinedError::Record)?;
    file.sync_all().map_err(|_| CombinedError::Record)?;
    fs::fsync(directory).map_err(|_| CombinedError::Record)?;
    if read_record(directory, &record.operation)? != *record {
        return Err(CombinedError::Record);
    }
    Ok(())
}
fn asset_record_exists(directory: &File, receipt: &FileRestoreReceipt) -> Result<bool> {
    private_directory(directory)?;
    let name = format!("asset-restore-{}.json", receipt.operation.as_str());
    match fs::statat(directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(_) => Err(CombinedError::Record),
    }
}
fn tree_record_exists(directory: &File, receipt: &TreeRestoreReceipt) -> Result<bool> {
    private_directory(directory)?;
    let name = format!("tree-restore-{}.json", receipt.operation.as_str());
    match fs::statat(directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(_) => Err(CombinedError::Record),
    }
}
impl PreparedCombinedRestore {
    fn record(&self, checkpoint: &VerifiedGeneration) -> Result<Record> {
        if checkpoint.manifest().manifest_sha256.as_ref() != Some(&self.checkpoint_manifest) {
            return Err(CombinedError::Binding);
        }
        Ok(Record {
            schema: if self.outputs.is_some() {
                3
            } else if self.trees.is_empty() {
                1
            } else {
                2
            },
            operation: self.intent.operation.clone(),
            account: self.intent.account.clone(),
            plan: self.preferences.plan().clone(),
            checkpoint_generation: checkpoint.manifest().generation_id.clone(),
            checkpoint_manifest: self.checkpoint_manifest.clone(),
            assets: self
                .assets
                .iter()
                .map(FileRestorePlan::receipt_identity)
                .collect::<std::result::Result<_, _>>()
                .map_err(CombinedError::Restore)?,
            outputs: self
                .outputs
                .as_ref()
                .map(OutputRestorePlan::receipt_identity)
                .transpose()
                .map_err(CombinedError::Restore)?,
            trees: self
                .trees
                .iter()
                .map(TreeRestorePlan::receipt_identity)
                .collect::<std::result::Result<_, _>>()
                .map_err(CombinedError::Restore)?,
        })
    }
    fn check_execution(&self, execution: &CombinedExecution<'_>) -> Result<()> {
        if execution.account.principal != self.intent.account
            || execution.source.manifest().manifest_sha256.as_ref()
                != Some(&self.intent.source_manifest)
            || execution.checkpoint.manifest().manifest_sha256.as_ref()
                != Some(&self.checkpoint_manifest)
        {
            return Err(CombinedError::Binding);
        }
        Ok(())
    }
    /// Reconstruct from the protected combined record, verified source/current
    /// generations and each exact write-ahead record. No destination is written
    /// and a changed receipt/parent/stage cannot be credited as completed.
    pub fn reopen(
        operation: Identifier,
        stores: &CombinedStores<'_>,
        request: CombinedPreparation<'_>,
    ) -> Result<Self> {
        let record = read_record(stores.assets, &operation)?;
        let intent = CombinedRestoreIntent::from_checkpoint(
            operation,
            request.source,
            request.checkpoint,
            request.account,
        )?;
        if record.operation != request.checkpoint.manifest().transaction_id
            || record.account != request.account.principal
            || record.account != intent.account
            || record.checkpoint_manifest
                != request
                    .checkpoint
                    .manifest()
                    .manifest_sha256
                    .clone()
                    .ok_or(CombinedError::Binding)?
            || record.checkpoint_generation != request.checkpoint.manifest().generation_id
            || record.assets.len() != intent.assets.len()
            || record.trees.len() != intent.trees.len()
            || record.outputs.is_some() != intent.outputs.is_some()
            || request.scope == RestoreScope::SystemOnly
            || request.evidence.current_state_checkpoint != ArtifactStatus::Verified
        {
            return Err(CombinedError::Binding);
        }
        let facts = request.evidence;
        let plan = build_preference_subset_plan(
            request.source.manifest(),
            request.scope,
            facts.generation,
            facts.archive,
            &facts.system_snapshot,
            facts.system_compatibility,
            facts.configuration_compatibility,
        )
        .map_err(CombinedError::Preflight)?;
        if plan != record.plan {
            return Err(CombinedError::Binding);
        }
        let journal = stores.journals.load(&record.operation).map_err(|_| CombinedError::Record)?;
        if journal.plan != record.plan
            || journal.current_state_generation != record.checkpoint_generation
            || journal.current_state_manifest_sha256 != record.checkpoint_manifest
        {
            return Err(CombinedError::Record);
        }
        let preferences =
            stage_preference_subset(request.source, &plan).map_err(CombinedError::Stage)?;
        stores.stages.open_verified(&preferences).map_err(|_| CombinedError::Record)?;
        let assets = intent
            .assets
            .iter()
            .zip(&record.assets)
            .map(|(item, receipt)| {
                let plan = if asset_record_exists(stores.assets, receipt)? {
                    FileRestorePlan::reopen(
                        receipt,
                        request.source,
                        request.checkpoint,
                        request.home,
                        request.account,
                        stores.assets,
                    )
                } else {
                    FileRestorePlan::prepare(
                        item,
                        request.source,
                        request.checkpoint,
                        request.home,
                        request.account,
                    )
                }
                .map_err(CombinedError::Restore)?;
                if !plan.matches_intent(item)
                    || plan.receipt_identity().map_err(CombinedError::Restore)? != *receipt
                {
                    return Err(CombinedError::Record);
                }
                Ok(plan)
            })
            .collect::<Result<Vec<_>>>()?;
        let trees = intent
            .trees
            .iter()
            .zip(&record.trees)
            .map(|(item, receipt)| {
                let plan = if tree_record_exists(stores.assets, receipt)? {
                    TreeRestorePlan::reopen(
                        receipt,
                        request.source,
                        request.checkpoint,
                        request.home,
                        request.account,
                        stores.assets,
                    )
                } else {
                    TreeRestorePlan::prepare(
                        item,
                        request.source,
                        request.checkpoint,
                        request.home,
                        request.account,
                    )
                }
                .map_err(CombinedError::Restore)?;
                if !plan.matches_intent(item)
                    || plan.receipt_identity().map_err(CombinedError::Restore)? != *receipt
                {
                    return Err(CombinedError::Record);
                }
                Ok(plan)
            })
            .collect::<Result<Vec<_>>>()?;
        let outputs = match (&intent.outputs, &record.outputs) {
            (Some(item), Some(receipt)) => {
                let plan = if OutputRestorePlan::record_exists(stores.assets, receipt)
                    .map_err(CombinedError::Restore)?
                {
                    OutputRestorePlan::reopen(
                        &receipt.operation,
                        request.source,
                        request.checkpoint,
                        request.account,
                        stores.assets,
                    )
                } else {
                    OutputRestorePlan::prepare(
                        item,
                        request.source,
                        request.checkpoint,
                        request.home,
                        request.account,
                    )
                }
                .map_err(CombinedError::Restore)?;
                if !plan.matches_intent(item)
                    || plan.receipt_identity().map_err(CombinedError::Restore)? != *receipt
                {
                    return Err(CombinedError::Record);
                }
                Some(plan)
            }
            (None, None) => None,
            _ => return Err(CombinedError::Record),
        };
        Ok(Self {
            intent,
            checkpoint_manifest: record.checkpoint_manifest,
            preferences,
            assets,
            trees,
            outputs,
        })
    }

    /// Writes preparation records only. The native coordinator must establish
    /// writer shutdown and the verified root rollback before calling apply.
    pub fn begin(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
    ) -> Result<RestoreJournal> {
        self.begin_with_boundary(stores, execution, || true)
    }

    pub fn begin_with_boundary(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<RestoreJournal> {
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        self.check_execution(execution)?;
        crate::preparation::verify_live_checkpoint(
            execution.checkpoint.manifest(),
            execution.checkpoint_roots,
        )
        .map_err(|_| CombinedError::Coverage)?;
        for asset in &self.assets {
            if asset.observe(execution.home, execution.account).map_err(CombinedError::Restore)?
                != RestoreObservation::Original
            {
                return Err(CombinedError::Binding);
            }
        }
        for tree in &self.trees {
            if !tree.ready(execution.home, execution.account).map_err(CombinedError::Restore)? {
                return Err(CombinedError::Binding);
            }
        }
        if let Some(outputs) = &self.outputs {
            outputs
                .original_ready(execution.home, execution.account)
                .map_err(CombinedError::Restore)?;
        }
        let record = self.record(execution.checkpoint)?;
        let mut journal = RestoreJournal::new(
            record.operation.clone(),
            record.plan.clone(),
            record.checkpoint_generation.clone(),
            record.checkpoint_manifest.clone(),
        );
        stores.journals.create(&journal).map_err(|_| CombinedError::Record)?;
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        stores
            .journals
            .advance(&mut journal, RestorePhase::CheckpointVerified)
            .map_err(|_| CombinedError::Record)?;
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        stores.stages.create(&self.preferences).map_err(|_| CombinedError::Record)?;
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        write_record(stores.assets, &record)?;
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        stores
            .journals
            .advance(&mut journal, RestorePhase::ContentStaged)
            .map_err(|_| CombinedError::Record)?;
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        Ok(journal)
    }
    fn check_preparation(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
    ) -> Result<(Record, RestoreJournal)> {
        self.check_execution(execution)?;
        let record = self.record(execution.checkpoint)?;
        let journal = stores.journals.load(&record.operation).map_err(|_| CombinedError::Record)?;
        if journal.plan != record.plan
            || journal.current_state_generation != record.checkpoint_generation
            || journal.current_state_manifest_sha256 != record.checkpoint_manifest
            || !matches!(
                journal.phase,
                RestorePhase::Planned
                    | RestorePhase::CheckpointVerified
                    | RestorePhase::ContentStaged
            )
        {
            return Err(CombinedError::Binding);
        }
        crate::preparation::verify_live_checkpoint(
            execution.checkpoint.manifest(),
            execution.checkpoint_roots,
        )
        .map_err(|_| CombinedError::Coverage)?;
        for (asset, receipt) in self.assets.iter().zip(&record.assets) {
            if asset_record_exists(stores.assets, receipt)?
                || asset
                    .observe(execution.home, execution.account)
                    .map_err(CombinedError::Restore)?
                    != RestoreObservation::Original
            {
                return Err(CombinedError::Binding);
            }
        }
        for (tree, receipt) in self.trees.iter().zip(&record.trees) {
            if tree_record_exists(stores.assets, receipt)?
                || !tree.ready(execution.home, execution.account).map_err(CombinedError::Restore)?
            {
                return Err(CombinedError::Binding);
            }
        }
        if let Some(outputs) = &self.outputs {
            if OutputRestorePlan::record_exists(
                stores.assets,
                &outputs.receipt_identity().map_err(CombinedError::Restore)?,
            )
            .map_err(CombinedError::Restore)?
            {
                return Err(CombinedError::Binding);
            }
            outputs
                .original_ready(execution.home, execution.account)
                .map_err(CombinedError::Restore)?;
        }
        Ok((record, journal))
    }

    /// Resume only before any live apply. Complete immutable stages are reused;
    /// an interrupted or unknown stage is never overwritten. The explicit
    /// cleanup method below handles only verified incomplete private artifacts.
    pub fn resume_preparation(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
    ) -> Result<RestoreJournal> {
        let (record, mut journal) = self.check_preparation(stores, execution)?;
        if journal.phase == RestorePhase::Planned {
            stores
                .journals
                .advance(&mut journal, RestorePhase::CheckpointVerified)
                .map_err(|_| CombinedError::Record)?;
        }
        if stores.stages.open_verified(&self.preferences).is_err() {
            if journal.phase == RestorePhase::ContentStaged {
                return Err(CombinedError::Record);
            }
            stores.stages.create(&self.preferences).map_err(|_| CombinedError::Record)?;
        }
        private_directory(stores.assets)?;
        let exists = match fs::statat(
            stores.assets,
            record_name(&record.operation).as_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(_) => true,
            Err(rustix::io::Errno::NOENT) => false,
            Err(_) => return Err(CombinedError::Record),
        };
        if exists {
            if read_record(stores.assets, &record.operation)? != record {
                return Err(CombinedError::Record);
            }
        } else {
            if journal.phase == RestorePhase::ContentStaged {
                return Err(CombinedError::Record);
            }
            write_record(stores.assets, &record)?;
        }
        if journal.phase == RestorePhase::CheckpointVerified {
            stores
                .journals
                .advance(&mut journal, RestorePhase::ContentStaged)
                .map_err(|_| CombinedError::Record)?;
        }
        Ok(journal)
    }

    /// Remove only a known incomplete protected stage or a partial combined
    /// record matching the exact expected canonical prefix. A complete record,
    /// unknown bytes, links or any post-preparation phase refuse before cleanup.
    /// Live preferences/assets and the retained checkpoint are never deleted.
    pub fn discard_incomplete_preparation(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
    ) -> Result<()> {
        let (record, journal) = self.check_preparation(stores, execution)?;
        if journal.phase == RestorePhase::ContentStaged {
            return Err(CombinedError::Binding);
        }
        private_directory(stores.assets)?;
        let name = record_name(&record.operation);
        let partial = match fs::openat2(
            stores.assets,
            name.as_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            ResolveFlags::BENEATH
                | ResolveFlags::NO_SYMLINKS
                | ResolveFlags::NO_MAGICLINKS
                | ResolveFlags::NO_XDEV,
        ) {
            Ok(fd) => Some(File::from(fd)),
            Err(rustix::io::Errno::NOENT) => None,
            Err(_) => return Err(CombinedError::Record),
        };
        if let Some(mut file) = partial.as_ref() {
            let expected = serde_json::to_vec(&record).map_err(|_| CombinedError::Record)?;
            let stat = fs::fstat(file).map_err(|_| CombinedError::Record)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
                || stat.st_nlink != 1
                || stat.st_uid != rustix::process::geteuid().as_raw()
                || stat.st_mode & 0o7777 != 0o600
                || stat.st_size < 0
                || stat.st_size as usize > expected.len()
                || expected.len() > MAX_RECORD
            {
                return Err(CombinedError::Record);
            }
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take((MAX_RECORD + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| CombinedError::Record)?;
            if bytes.len() != stat.st_size as usize || !expected.starts_with(&bytes) {
                return Err(CombinedError::Record);
            }
        }
        let complete_stage = stores.stages.open_verified(&self.preferences).is_ok();
        if complete_stage && partial.is_none() {
            return Err(CombinedError::Record);
        }
        if !complete_stage {
            stores
                .stages
                .discard_incomplete(&self.preferences)
                .map_err(|_| CombinedError::Record)?;
        }
        if let Some(file) = partial {
            let before = fs::fstat(&file).map_err(|_| CombinedError::Record)?;
            let current = fs::statat(stores.assets, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|_| CombinedError::Record)?;
            if before.st_dev != current.st_dev
                || before.st_ino != current.st_ino
                || before.st_ctime != current.st_ctime
                || before.st_ctime_nsec != current.st_ctime_nsec
            {
                return Err(CombinedError::Record);
            }
            fs::unlinkat(stores.assets, name.as_str(), AtFlags::empty())
                .map_err(|_| CombinedError::Record)?;
            fs::fsync(stores.assets).map_err(|_| CombinedError::Record)?;
        }
        Ok(())
    }

    /// All asset writes remain behind the durable writer/root-rollback phases.
    /// An interrupted partial stage requires a fresh captured cleanup; it is
    /// never silently reused. Completed items are read back after restart.
    pub fn apply(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
        boundary: impl FnMut() -> bool,
    ) -> Result<RestoreJournal> {
        self.apply_owned(stores, execution, boundary, 0)
    }
    pub(crate) fn apply_owned(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
        mut boundary: impl FnMut() -> bool,
        owner: u32,
    ) -> Result<RestoreJournal> {
        self.check_execution(execution)?;
        let record = read_record(stores.assets, &self.intent.operation)?;
        if record != self.record(execution.checkpoint)? {
            return Err(CombinedError::Record);
        }
        let mut journal =
            stores.journals.load(&record.operation).map_err(|_| CombinedError::Record)?;
        let ready = match journal.plan.scope {
            RestoreScope::ConfigurationOnly => matches!(
                journal.phase,
                RestorePhase::WritersStopped
                    | RestorePhase::ConfigurationRestored
                    | RestorePhase::HealthChecked
            ),
            RestoreScope::CompleteLinked => matches!(
                journal.phase,
                RestorePhase::RootRollbackComplete
                    | RestorePhase::ConfigurationRestored
                    | RestorePhase::HealthChecked
            ),
            RestoreScope::SystemOnly => false,
        };
        if !ready
            || journal.plan != record.plan
            || journal.current_state_generation != record.checkpoint_generation
            || journal.current_state_manifest_sha256 != record.checkpoint_manifest
        {
            return Err(CombinedError::Binding);
        }
        stores.stages.open_verified(&self.preferences).map_err(|_| CombinedError::Record)?;
        for (asset, receipt) in self.assets.iter().zip(&record.assets) {
            if !boundary() {
                return Err(CombinedError::Interrupted);
            }
            let exists = asset_record_exists(stores.assets, receipt)?;
            let reopened;
            let asset = if exists {
                reopened = FileRestorePlan::reopen(
                    receipt,
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                )
                .map_err(CombinedError::Restore)?;
                &reopened
            } else {
                asset
            };
            if exists
                && asset
                    .restored(execution.home, execution.account)
                    .map_err(CombinedError::Restore)?
            {
                continue;
            }
            if matches!(
                journal.phase,
                RestorePhase::ConfigurationRestored | RestorePhase::HealthChecked
            ) {
                return Err(CombinedError::Binding);
            }
            let actual = if exists {
                asset.resume_original(
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                    &mut boundary,
                )
            } else {
                asset.apply(
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                    &mut boundary,
                )
            }
            .map_err(CombinedError::Restore)?;
            if actual != *receipt {
                return Err(CombinedError::Record);
            }
        }
        for (tree, receipt) in self.trees.iter().zip(&record.trees) {
            if !boundary() {
                return Err(CombinedError::Interrupted);
            }
            let exists = tree_record_exists(stores.assets, receipt)?;
            let reopened;
            let tree = if exists {
                reopened = TreeRestorePlan::reopen(
                    receipt,
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                )
                .map_err(CombinedError::Restore)?;
                &reopened
            } else {
                tree
            };
            if exists
                && tree
                    .observe(execution.home, execution.account)
                    .map_err(CombinedError::Restore)?
                    == TreeObservation::Restored
            {
                continue;
            }
            if matches!(
                journal.phase,
                RestorePhase::ConfigurationRestored | RestorePhase::HealthChecked
            ) {
                return Err(CombinedError::Binding);
            }
            let actual = if exists {
                tree.resume(
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                    &mut boundary,
                )
            } else {
                tree.apply(
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                    &mut boundary,
                )
            }
            .map_err(CombinedError::Restore)?;
            if actual != *receipt {
                return Err(CombinedError::Record);
            }
        }
        if let Some(outputs) = &self.outputs {
            if !boundary() {
                return Err(CombinedError::Interrupted);
            }
            if !outputs
                .completed(
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                )
                .map_err(CombinedError::Restore)?
            {
                if matches!(
                    journal.phase,
                    RestorePhase::ConfigurationRestored | RestorePhase::HealthChecked
                ) {
                    return Err(CombinedError::Binding);
                }
                outputs
                    .apply_or_resume(
                        execution.source,
                        execution.checkpoint,
                        execution.home,
                        execution.account,
                        stores.assets,
                        &mut boundary,
                    )
                    .map_err(CombinedError::Restore)?;
            }
        }
        apply_subset(
            &self.preferences,
            stores.stages,
            stores.journals,
            &record.operation,
            execution.destinations,
            |_| if boundary() { ControlFlow::Continue(()) } else { ControlFlow::Break(()) },
            ApplyControl { owner, advance: false },
        )
        .map_err(|_| CombinedError::Apply)?;
        for asset in &self.assets {
            if !asset.restored(execution.home, execution.account).map_err(CombinedError::Restore)? {
                return Err(CombinedError::Binding);
            }
        }
        for tree in &self.trees {
            if tree.observe(execution.home, execution.account).map_err(CombinedError::Restore)?
                != TreeObservation::Restored
            {
                return Err(CombinedError::Binding);
            }
        }
        if let Some(outputs) = &self.outputs
            && !outputs
                .completed(
                    execution.source,
                    execution.checkpoint,
                    execution.home,
                    execution.account,
                    stores.assets,
                )
                .map_err(CombinedError::Restore)?
        {
            return Err(CombinedError::Binding);
        }
        if !boundary() {
            return Err(CombinedError::Interrupted);
        }
        if matches!(
            journal.phase,
            RestorePhase::WritersStopped | RestorePhase::RootRollbackComplete
        ) {
            stores
                .journals
                .advance(&mut journal, RestorePhase::ConfigurationRestored)
                .map_err(|_| CombinedError::Record)?;
        }
        Ok(journal)
    }
}

/// Derived only from verified partial-stage observations. No arbitrary deletion
/// target or client path can be inserted into this cleanup scope.
pub struct CombinedCleanupIntent {
    operation: Identifier,
    restore_operation: Identifier,
    source_manifest: Sha256,
    account: AccountPrincipal,
    intents: Vec<FileRestoreIntent>,
    trees: Vec<TreeRestoreIntent>,
    outputs: Option<OutputRestoreIntent>,
    scope: AssetScope,
}
impl PreparedCombinedRestore {
    pub fn partial_cleanup(
        &self,
        operation: Identifier,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
    ) -> Result<CombinedCleanupIntent> {
        if checkpoint.manifest().manifest_sha256.as_ref() != Some(&self.checkpoint_manifest)
            || operation == self.intent.operation
            || account.principal != self.intent.account
        {
            return Err(CombinedError::Binding);
        }
        let mut intents = Vec::new();
        let mut trees = Vec::new();
        let mut roots = Vec::new();
        let mut paths = BTreeSet::new();
        for (index, (plan, original)) in self.assets.iter().zip(&self.intent.assets).enumerate() {
            if plan.observe(home, account).map_err(CombinedError::Restore)?
                == RestoreObservation::Staged
            {
                let id = Identifier::new(format!(
                    "cleanup-{}-{index}",
                    &crate::wallpaper_protocol::digest(operation.as_str().as_bytes())[..32]
                ))
                .map_err(|_| CombinedError::Binding)?;
                let intent = FileRestoreIntent::new(id, checkpoint, original.staging())
                    .map_err(CombinedError::Restore)?;
                paths.extend(
                    intent.scope().map_err(CombinedError::Restore)?.paths().iter().cloned(),
                );
                intents.push(intent);
            }
        }
        for (index, (plan, original)) in self.trees.iter().zip(&self.intent.trees).enumerate() {
            if plan.observe(home, account).map_err(CombinedError::Restore)?
                == TreeObservation::Staged
            {
                let id = Identifier::new(format!(
                    "cleanup-tree-{}-{index}",
                    &crate::wallpaper_protocol::digest(operation.as_str().as_bytes())[..32]
                ))
                .map_err(|_| CombinedError::Binding)?;
                let intent =
                    TreeRestoreIntent::new(id, checkpoint, original.staging(), home, account)
                        .map_err(CombinedError::Restore)?;
                paths.extend(intent.scope().paths().iter().cloned());
                roots.extend(intent.scope().trees().iter().cloned());
                trees.push(intent);
            }
        }
        let outputs = if let Some(plan) = &self.outputs {
            let id = Identifier::new(format!(
                "cleanup-outputs-{}",
                &crate::wallpaper_protocol::digest(operation.as_str().as_bytes())[..32]
            ))
            .map_err(|_| CombinedError::Binding)?;
            plan.partial_cleanup(id, checkpoint, home, account, journal)
                .map_err(CombinedError::Restore)?
        } else {
            None
        };
        if let Some(outputs) = &outputs {
            paths.extend(outputs.scope().paths().iter().cloned());
        }
        if intents.is_empty() && trees.is_empty() && outputs.is_none() {
            return Err(CombinedError::Binding);
        }
        let scope = AssetScope::new_with_outputs(
            paths.into_iter().collect(),
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
            outputs.as_ref().and_then(|output| output.scope().output_declaration().cloned()),
        )
        .and_then(|scope| scope.with_trees(roots))
        .map_err(CombinedError::Asset)?;
        Ok(CombinedCleanupIntent {
            operation,
            restore_operation: self.intent.operation.clone(),
            source_manifest: self.checkpoint_manifest.clone(),
            account: self.intent.account.clone(),
            intents,
            trees,
            outputs,
            scope,
        })
    }
}
impl CombinedCleanupIntent {
    pub fn operation(&self) -> &Identifier {
        &self.operation
    }
    pub fn scope(&self) -> &AssetScope {
        &self.scope
    }
    /// The authorized console verifies and pins the fresh native partner before
    /// calling this method. The original restore phase remains unchanged.
    pub fn apply(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<usize> {
        if source.manifest().manifest_sha256.as_ref() != Some(&self.source_manifest)
            || checkpoint.manifest().transaction_id != self.operation
            || account.principal != self.account
        {
            return Err(CombinedError::Binding);
        }
        let plans = self
            .intents
            .iter()
            .map(|intent| {
                FileRestorePlan::prepare(intent, source, checkpoint, home, account)
                    .map_err(CombinedError::Restore)
            })
            .collect::<Result<Vec<_>>>()?;
        let trees = self
            .trees
            .iter()
            .map(|intent| {
                TreeRestorePlan::prepare(intent, source, checkpoint, home, account)
                    .map_err(CombinedError::Restore)
            })
            .collect::<Result<Vec<_>>>()?;
        let outputs = self
            .outputs
            .as_ref()
            .map(|intent| {
                OutputRestorePlan::prepare(intent, source, checkpoint, home, account)
                    .map_err(CombinedError::Restore)
            })
            .transpose()?;
        #[derive(Serialize)]
        struct CleanupRecord<'a> {
            schema: u16,
            operation: &'a Identifier,
            restore_operation: &'a Identifier,
            source_manifest: &'a Sha256,
            checkpoint_transaction: &'a Identifier,
            checkpoint_manifest: &'a Sha256,
            account: &'a AccountPrincipal,
            receipts: Vec<FileRestoreReceipt>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            trees: Vec<TreeRestoreReceipt>,
            #[serde(skip_serializing_if = "Option::is_none")]
            outputs: Option<OutputRestoreIdentity>,
        }
        let record = CleanupRecord {
            schema: if outputs.is_some() {
                3
            } else if trees.is_empty() {
                1
            } else {
                2
            },
            operation: &self.operation,
            restore_operation: &self.restore_operation,
            source_manifest: &self.source_manifest,
            checkpoint_transaction: &checkpoint.manifest().transaction_id,
            checkpoint_manifest: checkpoint
                .manifest()
                .manifest_sha256
                .as_ref()
                .ok_or(CombinedError::Binding)?,
            account: &self.account,
            receipts: plans
                .iter()
                .map(FileRestorePlan::receipt_identity)
                .collect::<std::result::Result<_, _>>()
                .map_err(CombinedError::Restore)?,
            trees: trees
                .iter()
                .map(TreeRestorePlan::receipt_identity)
                .collect::<std::result::Result<_, _>>()
                .map_err(CombinedError::Restore)?,
            outputs: outputs
                .as_ref()
                .map(OutputRestorePlan::receipt_identity)
                .transpose()
                .map_err(CombinedError::Restore)?,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| CombinedError::Record)?;
        if bytes.len() > MAX_RECORD || !boundary() {
            return Err(CombinedError::Interrupted);
        }
        private_directory(journal)?;
        let name = format!("combined-cleanup-{}.json", self.operation.as_str());
        let mut file = File::from(
            fs::openat(
                journal,
                name.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|_| CombinedError::Record)?,
        );
        file.write_all(&bytes).map_err(|_| CombinedError::Record)?;
        fs::fchmod(&file, Mode::from_raw_mode(0o400)).map_err(|_| CombinedError::Record)?;
        file.sync_all().map_err(|_| CombinedError::Record)?;
        fs::fsync(journal).map_err(|_| CombinedError::Record)?;
        use std::os::unix::fs::FileExt;
        let mut check = vec![0; bytes.len()];
        file.read_exact_at(&mut check, 0).map_err(|_| CombinedError::Record)?;
        if check != bytes {
            return Err(CombinedError::Record);
        }
        for plan in &plans {
            plan.apply(source, checkpoint, home, account, journal, &mut boundary)
                .map_err(CombinedError::Restore)?;
        }
        for tree in &trees {
            tree.apply(source, checkpoint, home, account, journal, &mut boundary)
                .map_err(CombinedError::Restore)?;
        }
        if let Some(output) = &outputs {
            output
                .apply_or_resume(source, checkpoint, home, account, journal, &mut boundary)
                .map_err(CombinedError::Restore)?;
        }
        Ok(plans.len() + trees.len() + usize::from(outputs.is_some()))
    }
}
