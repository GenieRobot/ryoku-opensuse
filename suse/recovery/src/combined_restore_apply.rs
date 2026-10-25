//! Durable coordination for the private preference subset and exact streamed
//! asset plans. Public native authorization remains outside this storage layer.
use super::*;
use crate::{
    asset_restore::{FileRestoreReceipt, RestoreObservation},
    configuration_apply::{ApplyControl, ConfigurationDestination, apply_subset},
    disk_stage::DiskStageStore,
    generation_capture::DeclaredRoot,
    restore::{RestoreJournal, RestoreJournalStore, RestorePhase, RestorePlan},
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
    if record.schema != 1 || record.operation != *operation {
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
impl PreparedCombinedRestore {
    fn record(&self, checkpoint: &VerifiedGeneration) -> Result<Record> {
        if checkpoint.manifest().manifest_sha256.as_ref() != Some(&self.checkpoint_manifest) {
            return Err(CombinedError::Binding);
        }
        Ok(Record {
            schema: 1,
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
        let intent = CombinedRestoreIntent::new(operation, request.source)?;
        if record.account != request.account.principal
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
        Ok(Self { intent, checkpoint_manifest: record.checkpoint_manifest, preferences, assets })
    }

    /// Writes preparation records only. The native coordinator must establish
    /// writer shutdown and the verified root rollback before calling apply.
    pub fn begin(
        &self,
        stores: &CombinedStores<'_>,
        execution: &CombinedExecution<'_>,
    ) -> Result<RestoreJournal> {
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
        let record = self.record(execution.checkpoint)?;
        let mut journal = RestoreJournal::new(
            record.operation.clone(),
            record.plan.clone(),
            record.checkpoint_generation.clone(),
            record.checkpoint_manifest.clone(),
        );
        stores.journals.create(&journal).map_err(|_| CombinedError::Record)?;
        stores
            .journals
            .advance(&mut journal, RestorePhase::CheckpointVerified)
            .map_err(|_| CombinedError::Record)?;
        stores.stages.create(&self.preferences).map_err(|_| CombinedError::Record)?;
        write_record(stores.assets, &record)?;
        stores
            .journals
            .advance(&mut journal, RestorePhase::ContentStaged)
            .map_err(|_| CombinedError::Record)?;
        Ok(journal)
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
