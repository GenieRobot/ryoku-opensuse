//! Preparation through a verified current-state generation and durable disk
//! stage. The caller must authorize recovery, resolve the complete approved
//! inventory, validate persistent mounts outside rollback scope, and keep all
//! writers frozen from snapshot creation through handoff to the offline writer.
//! Snapper availability/compatibility observations are trusted caller inputs;
//! this module neither creates native system snapshots nor stops services.

use crate::{
    EntryKind, Identifier, OperationReason, RecoveryManifest, StateRoot,
    capture::{CaptureError, CaptureLimits, capture_inventory},
    configuration_apply::{ConfigurationApplyError, ConfigurationDestination, apply_with_owner},
    configuration_stage::{StageError, StagedConfiguration, stage_configuration},
    disk_stage::{DiskStageError, DiskStageStore, VerifiedDiskStage},
    generation::{GenerationError, GenerationStore, VerifiedGeneration},
    generation_capture::{
        CaptureProvenance, DeclaredRoot, GenerationCaptureError, GenerationCaptureRequest,
        capture_publish_verify_checked,
    },
    restore::{
        ArtifactStatus, CompatibilityStatus, JournalError, PreflightError, PreflightEvidence,
        RestoreJournal, RestoreJournalStore, RestorePhase, RestoreScope, RestoreSourceEvidence,
        SystemSnapshotStatus, plan_restore, plan_restore_source,
    },
};
use rustix::io::Errno;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct RestoreCompatibility {
    pub source_snapshot: SystemSnapshotStatus,
    pub system: CompatibilityStatus,
    pub configuration: CompatibilityStatus,
}

pub struct PreparationStores<'a> {
    pub generations: &'a GenerationStore,
    pub journals: &'a RestoreJournalStore,
    pub stages: &'a DiskStageStore,
}

fn rollback_confirmation_transition(scope: RestoreScope, phase: RestorePhase) -> Option<bool> {
    if !matches!(scope, RestoreScope::SystemOnly | RestoreScope::CompleteLinked) {
        return None;
    }
    match phase {
        RestorePhase::WritersStopped => Some(true),
        RestorePhase::RootRollbackComplete
        | RestorePhase::ConfigurationRestored
        | RestorePhase::HealthChecked
        | RestorePhase::Complete => Some(false),
        _ => None,
    }
}

pub struct ConfigurationPreparationRequest<'a> {
    pub restore_id: Identifier,
    pub source: &'a VerifiedGeneration,
    pub scope: RestoreScope,
    pub compatibility: RestoreCompatibility,
    /// Fresh independently verified Snapper link plus the complete approved
    /// current-state inventory/provenance. Every affected target must be covered.
    pub checkpoint: GenerationCaptureRequest<'a>,
}

/// Inputs for a system-only restore. The verified source supplies the exact
/// native snapshot identity; the fresh capture retains Ryoku state that may
/// need to be recovered after a root rollback.
pub struct SystemPreparationRequest<'a> {
    pub restore_id: Identifier,
    pub source: &'a VerifiedGeneration,
    pub compatibility: RestoreCompatibility,
    pub checkpoint: GenerationCaptureRequest<'a>,
}

pub struct PreparedConfigurationRestore {
    journal: RestoreJournal,
    checkpoint: VerifiedGeneration,
    source: StagedConfiguration,
    stage: VerifiedDiskStage,
}
impl PreparedConfigurationRestore {
    pub fn journal(&self) -> &RestoreJournal {
        &self.journal
    }
    pub fn checkpoint(&self) -> &VerifiedGeneration {
        &self.checkpoint
    }
    pub fn source(&self) -> &StagedConfiguration {
        &self.source
    }
    pub fn stage(&self) -> &VerifiedDiskStage {
        &self.stage
    }
}

pub struct PreparedSystemRestore {
    journal: RestoreJournal,
    checkpoint: VerifiedGeneration,
}
impl PreparedSystemRestore {
    pub fn journal(&self) -> &RestoreJournal {
        &self.journal
    }
    pub fn checkpoint(&self) -> &VerifiedGeneration {
        &self.checkpoint
    }
}

/// The exclusive writer guard and durable rollback receipt that a caller must
/// retain while handing the machine to reboot. Dropping the guard before the
/// restart can allow another supported mutation to race with the selected
/// root target.
pub struct RootRollbackHandoff<G> {
    journal: RestoreJournal,
    receipt: ryoku_snapper::RootRollbackReceipt,
    writer_guard: G,
}
impl<G> RootRollbackHandoff<G> {
    pub fn journal(&self) -> &RestoreJournal {
        &self.journal
    }
    pub fn receipt(&self) -> &ryoku_snapper::RootRollbackReceipt {
        &self.receipt
    }
    pub fn writer_guard(&self) -> &G {
        &self.writer_guard
    }
}

/// Narrow native operation used only after restore preparation and writer
/// quiescence. Implementations must reconcile by attempt ID after uncertain
/// outcomes and must never retry a rollback automatically.
pub trait RootRollbackBackend {
    fn rollback_to(
        &mut self,
        request: &ryoku_snapper::RootRollbackRequest,
    ) -> Result<ryoku_snapper::RootRollbackReceipt, ryoku_snapper::SnapperCliError>;
}
impl RootRollbackBackend for ryoku_snapper::SnapperRollbackBackend {
    fn rollback_to(
        &mut self,
        request: &ryoku_snapper::RootRollbackRequest,
    ) -> Result<ryoku_snapper::RootRollbackReceipt, ryoku_snapper::SnapperCliError> {
        ryoku_snapper::SnapperRollbackBackend::rollback_to(self, request)
    }
}

#[derive(Debug)]
pub enum RestoreExecutionError {
    WrongScopeOrPhase,
    WriterQuiescence(String),
    HealthCheck(String),
    Apply(ConfigurationApplyError),
    Journal(JournalError),
    NativeRollback(ryoku_snapper::SnapperCliError),
    NativeVerification(crate::native_checkpoint::NativeCheckpointError),
    Generation(GenerationError),
}
impl std::fmt::Display for RestoreExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "restore execution failed: {self:?}")
    }
}
impl std::error::Error for RestoreExecutionError {}

#[derive(Debug)]
pub enum PreparationError {
    UnsupportedScope,
    InvalidSource,
    InvalidCheckpointIdentity,
    MissingCheckpointCoverage,
    CheckpointSnapshotUnavailable,
    UnexpectedPhase,
    LiveStateChanged,
    LiveCapture(CaptureError),
    Generation(GenerationError),
    Capture(GenerationCaptureError),
    Journal(JournalError),
    Preflight(PreflightError),
    StageSource(StageError),
    DiskStage(DiskStageError),
}
impl std::fmt::Display for PreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "restore preparation failed: {self:?}")
    }
}
impl std::error::Error for PreparationError {}
macro_rules! conversion {
    ($type:ty, $variant:ident) => {
        impl From<$type> for PreparationError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}
conversion!(GenerationError, Generation);
conversion!(GenerationCaptureError, Capture);
conversion!(JournalError, Journal);
conversion!(PreflightError, Preflight);
conversion!(StageError, StageSource);
conversion!(DiskStageError, DiskStage);
conversion!(CaptureError, LiveCapture);

impl PreparationStores<'_> {
    /// Remove only an incomplete disk stage for a preparation that has not
    /// advanced beyond checkpoint verification. The caller must hold the
    /// exclusive recovery mutation lock and run from an offline recovery
    /// environment. The journal and verified source determine the exact stage
    /// contents; complete or unexpected stages are refused.
    pub fn discard_incomplete_stage(
        &self,
        restore_id: &Identifier,
    ) -> Result<(), PreparationError> {
        let journal = self.journals.load(restore_id)?;
        if !matches!(journal.phase, RestorePhase::Planned | RestorePhase::CheckpointVerified) {
            return Err(PreparationError::UnexpectedPhase);
        }
        if !matches!(
            journal.plan.scope,
            RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked
        ) {
            return Err(PreparationError::UnsupportedScope);
        }
        let source = self.generations.open_verified_transaction(&journal.plan.transaction_id)?;
        if source.manifest().generation_id != journal.plan.generation_id
            || source.manifest().manifest_sha256.as_ref()
                != Some(&journal.plan.source_manifest_sha256)
            || source.manifest().snapper_config != journal.plan.snapper_config
            || source.manifest().snapper_snapshot_id != journal.plan.snapshot_id
            || source.manifest().filesystem_uuid != journal.plan.filesystem_uuid
        {
            return Err(PreparationError::InvalidSource);
        }
        let staged = stage_configuration(&source, &journal.plan)?;
        self.stages.discard_incomplete(&staged)?;
        Ok(())
    }

    /// Confirm the exact booted native target before advancing a system or
    /// complete-linked attempt beyond `WritersStopped`. Later phases stay intact.
    pub fn confirm_booted_root_rollback(
        &self,
        restore_id: &Identifier,
        trusted_root: &std::fs::File,
    ) -> Result<RestoreJournal, RestoreExecutionError> {
        let mut journal = self.journals.load(restore_id).map_err(RestoreExecutionError::Journal)?;
        let newly_confirming = rollback_confirmation_transition(journal.plan.scope, journal.phase)
            .ok_or(RestoreExecutionError::WrongScopeOrPhase)?;
        let source = self
            .generations
            .open_verified_transaction(&journal.plan.transaction_id)
            .map_err(RestoreExecutionError::Generation)?;
        let receipt = source
            .manifest()
            .root_snapshot_receipt()
            .ok_or(RestoreExecutionError::WrongScopeOrPhase)?;
        let evidence =
            crate::native_checkpoint::verify_running_root_rollback(trusted_root, &journal, receipt)
                .map_err(RestoreExecutionError::NativeVerification)?;
        if newly_confirming {
            self.journals
                .confirm_root_rollback_booted(&mut journal, &evidence)
                .map_err(RestoreExecutionError::Journal)?;
        }
        Ok(journal)
    }

    /// Finish a system-only rollback after caller-supplied boot and service
    /// health checks succeed. Until this durably reaches `Complete`, the
    /// recovery gate must continue to block Ryoku session startup.
    pub fn complete_system_restore(
        &self,
        journal: &mut RestoreJournal,
        health_check: impl FnOnce(&RestoreJournal) -> Result<(), String>,
    ) -> Result<RestoreJournal, RestoreExecutionError> {
        if journal.plan.scope != RestoreScope::SystemOnly
            || !matches!(
                journal.phase,
                RestorePhase::RootRollbackComplete
                    | RestorePhase::HealthChecked
                    | RestorePhase::Complete
            )
        {
            return Err(RestoreExecutionError::WrongScopeOrPhase);
        }
        if journal.phase == RestorePhase::Complete {
            return Ok(journal.clone());
        }
        if journal.phase == RestorePhase::RootRollbackComplete {
            health_check(journal).map_err(RestoreExecutionError::HealthCheck)?;
            self.journals
                .advance(journal, RestorePhase::HealthChecked)
                .map_err(RestoreExecutionError::Journal)?;
        }
        self.journals
            .advance(journal, RestorePhase::Complete)
            .map_err(RestoreExecutionError::Journal)?;
        Ok(journal.clone())
    }

    /// Stop writers, durably enter the native rollback phase, perform exactly
    /// one attempt-tagged root rollback, and persist Snapper's verified receipt.
    /// The returned handoff owns the writer guard; retain it until the caller
    /// has requested a reboot. If Snapper returns an error, the journal stays
    /// pending in `WritersStopped` and the attempt must be inspected manually.
    pub fn execute_system_root_rollback<G>(
        &self,
        prepared: &mut PreparedSystemRestore,
        stop_writers: impl FnOnce() -> Result<G, String>,
        backend: &mut impl RootRollbackBackend,
    ) -> Result<RootRollbackHandoff<G>, RestoreExecutionError> {
        self.execute_root_rollback(
            &mut prepared.journal,
            RestoreScope::SystemOnly,
            stop_writers,
            backend,
        )
    }

    /// Complete-linked counterpart to `execute_system_root_rollback`. The
    /// configuration stage is already verified and remains on persistent
    /// storage while the operating-system root is rolled back.
    pub fn execute_complete_linked_root_rollback<G>(
        &self,
        prepared: &mut PreparedConfigurationRestore,
        stop_writers: impl FnOnce() -> Result<G, String>,
        backend: &mut impl RootRollbackBackend,
    ) -> Result<RootRollbackHandoff<G>, RestoreExecutionError> {
        self.execute_root_rollback(
            &mut prepared.journal,
            RestoreScope::CompleteLinked,
            stop_writers,
            backend,
        )
    }

    fn execute_root_rollback<G>(
        &self,
        journal: &mut RestoreJournal,
        scope: RestoreScope,
        stop_writers: impl FnOnce() -> Result<G, String>,
        backend: &mut impl RootRollbackBackend,
    ) -> Result<RootRollbackHandoff<G>, RestoreExecutionError> {
        let expected_phase = match scope {
            RestoreScope::SystemOnly => RestorePhase::CheckpointVerified,
            RestoreScope::CompleteLinked => RestorePhase::ContentStaged,
            RestoreScope::ConfigurationOnly => {
                return Err(RestoreExecutionError::WrongScopeOrPhase);
            }
        };
        if journal.plan.scope != scope
            || journal.phase != expected_phase
            || journal.plan.snapper_config.as_str() != "root"
        {
            return Err(RestoreExecutionError::WrongScopeOrPhase);
        }
        let writer_guard = stop_writers().map_err(RestoreExecutionError::WriterQuiescence)?;
        self.journals
            .advance(journal, RestorePhase::WritersStopped)
            .map_err(RestoreExecutionError::Journal)?;
        let request = ryoku_snapper::RootRollbackRequest {
            config: ryoku_snapper::ConfigName::new(journal.plan.snapper_config.as_str())
                .map_err(|_| RestoreExecutionError::WrongScopeOrPhase)?,
            source_snapshot: ryoku_snapper::SnapshotId::new(journal.plan.snapshot_id.clone())
                .map_err(|_| RestoreExecutionError::WrongScopeOrPhase)?,
            transaction_id: crate::restore::root_rollback_tag(&journal.restore_id)
                .map_err(RestoreExecutionError::Journal)?,
        };
        let receipt =
            backend.rollback_to(&request).map_err(RestoreExecutionError::NativeRollback)?;
        self.journals
            .record_root_rollback_result(journal, &receipt)
            .map_err(RestoreExecutionError::Journal)?;
        Ok(RootRollbackHandoff { journal: journal.clone(), receipt, writer_guard })
    }

    /// Apply the selected configuration after the root target has been
    /// independently verified as the running root for a complete-linked
    /// restore, or apply a configuration-only restore while its enrolled
    /// writers are stopped.
    /// The caller-owned guard remains alive through application and health
    /// checks; the caller authenticates the operator and implements writer stop.
    pub fn execute_configuration_restore<G>(
        &self,
        prepared: &mut PreparedConfigurationRestore,
        destinations: &[ConfigurationDestination<'_>],
        stop_writers: impl FnOnce() -> Result<G, String>,
        health_check: impl FnOnce(&RestoreJournal) -> Result<(), String>,
    ) -> Result<RestoreJournal, RestoreExecutionError> {
        let phase_ready = match prepared.journal.plan.scope {
            RestoreScope::ConfigurationOnly => matches!(
                prepared.journal.phase,
                RestorePhase::ContentStaged
                    | RestorePhase::WritersStopped
                    | RestorePhase::ConfigurationRestored
                    | RestorePhase::HealthChecked
                    | RestorePhase::Complete
            ),
            RestoreScope::CompleteLinked => matches!(
                prepared.journal.phase,
                RestorePhase::RootRollbackComplete
                    | RestorePhase::ConfigurationRestored
                    | RestorePhase::HealthChecked
                    | RestorePhase::Complete
            ),
            RestoreScope::SystemOnly => false,
        };
        if !phase_ready {
            return Err(RestoreExecutionError::WrongScopeOrPhase);
        }
        if prepared.journal.phase == RestorePhase::Complete {
            return Ok(prepared.journal.clone());
        }

        let _writer_guard = stop_writers().map_err(RestoreExecutionError::WriterQuiescence)?;
        if prepared.journal.plan.scope == RestoreScope::ConfigurationOnly
            && prepared.journal.phase == RestorePhase::ContentStaged
        {
            self.journals
                .advance(&mut prepared.journal, RestorePhase::WritersStopped)
                .map_err(RestoreExecutionError::Journal)?;
        }
        if matches!(
            prepared.journal.phase,
            RestorePhase::RootRollbackComplete
                | RestorePhase::WritersStopped
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
        ) {
            prepared.journal = apply_with_owner(
                &prepared.source,
                self.stages,
                self.journals,
                &prepared.journal.restore_id,
                destinations,
                |_| std::ops::ControlFlow::Continue(()),
                rustix::process::geteuid().as_raw(),
            )
            .map_err(RestoreExecutionError::Apply)?;
        }
        if matches!(
            prepared.journal.phase,
            RestorePhase::ConfigurationRestored | RestorePhase::HealthChecked
        ) {
            health_check(&prepared.journal).map_err(RestoreExecutionError::HealthCheck)?;
            if prepared.journal.phase == RestorePhase::ConfigurationRestored {
                self.journals
                    .advance(&mut prepared.journal, RestorePhase::HealthChecked)
                    .map_err(RestoreExecutionError::Journal)?;
            }
        }
        if prepared.journal.phase == RestorePhase::HealthChecked {
            self.journals
                .advance(&mut prepared.journal, RestorePhase::Complete)
                .map_err(RestoreExecutionError::Journal)?;
        }
        Ok(prepared.journal.clone())
    }

    /// Capture and verify a current-state generation before allowing a
    /// system-only restore to enter the writer-stop phase. This prepares and
    /// journals the operation; it does not stop writers or roll back Snapper.
    pub fn begin_system(
        &self,
        request: SystemPreparationRequest<'_>,
    ) -> Result<PreparedSystemRestore, PreparationError> {
        match self.journals.load(&request.restore_id) {
            Err(JournalError::Filesystem(Errno::NOENT)) => {}
            Ok(_) => return Err(JournalError::AlreadyExists.into()),
            Err(error) => return Err(error.into()),
        }
        let source = self
            .generations
            .open_verified_transaction(&request.source.manifest().transaction_id)?;
        if source.manifest() != request.source.manifest() {
            return Err(PreparationError::InvalidSource);
        }
        if request.checkpoint.transaction_id != request.restore_id
            || request.restore_id == source.manifest().transaction_id
            || request.checkpoint.generation_id == source.manifest().generation_id
            || request.checkpoint.reason
                != OperationReason::Other(Identifier::new("system-restore").unwrap())
        {
            return Err(PreparationError::InvalidCheckpointIdentity);
        }

        // Reject unavailable or incompatible source snapshots before creating
        // the current-state checkpoint. This source-only plan does not claim
        // that the required current-state checkpoint already exists.
        let source_plan = plan_restore_source(
            source.manifest(),
            RestoreScope::SystemOnly,
            &source_facts(&request.compatibility),
        )?;
        let current_roots = request.checkpoint.roots;
        let current_snapshot = request.checkpoint.snapshot.link().clone();
        let checkpoint_manifest =
            capture_current_state(self.generations, request.checkpoint, source.manifest())?;
        if checkpoint_manifest.snapper_config != current_snapshot.snapper_config
            || checkpoint_manifest.snapper_snapshot_id != current_snapshot.snapper_snapshot_id
            || checkpoint_manifest.filesystem_uuid != current_snapshot.filesystem_uuid
            || checkpoint_manifest.snapper_config != source.manifest().snapper_config
            || checkpoint_manifest.filesystem_uuid != source.manifest().filesystem_uuid
            || checkpoint_manifest.snapper_snapshot_id == source.manifest().snapper_snapshot_id
        {
            return Err(PreparationError::InvalidCheckpointIdentity);
        }
        let checkpoint =
            self.generations.open_verified_transaction(&checkpoint_manifest.transaction_id)?;
        let plan = plan_restore(
            source.manifest(),
            RestoreScope::SystemOnly,
            &facts(&request.compatibility),
        )?;
        if !source_plan.matches(&plan) {
            return Err(PreparationError::InvalidSource);
        }
        verify_live_checkpoint(checkpoint.manifest(), current_roots)?;
        let mut journal = RestoreJournal::new(
            request.restore_id,
            plan,
            checkpoint.manifest().generation_id.clone(),
            checkpoint
                .manifest()
                .manifest_sha256
                .clone()
                .ok_or(PreparationError::InvalidCheckpointIdentity)?,
        );
        self.journals.create(&journal)?;
        self.journals.advance(&mut journal, RestorePhase::CheckpointVerified)?;
        Ok(PreparedSystemRestore { journal, checkpoint })
    }

    /// Reopen a prepared system-only restore after process interruption. It
    /// rechecks both source compatibility and the retained current-state
    /// generation and snapshot before returning the operation to its caller.
    pub fn resume_system(
        &self,
        restore_id: &Identifier,
        compatibility: &RestoreCompatibility,
        checkpoint_snapshot: &SystemSnapshotStatus,
        current_roots: &[DeclaredRoot<'_>],
    ) -> Result<PreparedSystemRestore, PreparationError> {
        let mut journal = self.journals.load(restore_id)?;
        if journal.plan.scope != RestoreScope::SystemOnly
            || !matches!(journal.phase, RestorePhase::Planned | RestorePhase::CheckpointVerified)
        {
            return Err(PreparationError::UnexpectedPhase);
        }
        let source = self.generations.open_verified_transaction(&journal.plan.transaction_id)?;
        let source_plan = plan_restore_source(
            source.manifest(),
            RestoreScope::SystemOnly,
            &source_facts(compatibility),
        )?;
        if !source_plan.matches(&journal.plan) {
            return Err(PreparationError::InvalidSource);
        }
        let checkpoint_manifest = self.generations.verify(&journal.current_state_generation)?;
        let checkpoint =
            self.generations.open_verified_transaction(&checkpoint_manifest.transaction_id)?;
        if checkpoint.manifest().generation_id != journal.current_state_generation
            || checkpoint.manifest().transaction_id != journal.restore_id
            || checkpoint.manifest().manifest_sha256.as_ref()
                != Some(&journal.current_state_manifest_sha256)
            || checkpoint.manifest().reason
                != OperationReason::Other(Identifier::new("system-restore").unwrap())
            || checkpoint.manifest().snapper_config != source.manifest().snapper_config
            || checkpoint.manifest().filesystem_uuid != source.manifest().filesystem_uuid
            || checkpoint.manifest().snapper_snapshot_id == source.manifest().snapper_snapshot_id
        {
            return Err(PreparationError::InvalidCheckpointIdentity);
        }
        match checkpoint_snapshot {
            SystemSnapshotStatus::Available { snapper_config, snapshot_id, filesystem_uuid }
                if snapper_config == &checkpoint.manifest().snapper_config
                    && snapshot_id == &checkpoint.manifest().snapper_snapshot_id
                    && filesystem_uuid == &checkpoint.manifest().filesystem_uuid => {}
            _ => return Err(PreparationError::CheckpointSnapshotUnavailable),
        }
        if plan_restore(source.manifest(), RestoreScope::SystemOnly, &facts(compatibility))?
            != journal.plan
        {
            return Err(PreparationError::InvalidSource);
        }
        verify_live_checkpoint(checkpoint.manifest(), current_roots)?;
        if journal.phase == RestorePhase::Planned {
            self.journals.advance(&mut journal, RestorePhase::CheckpointVerified)?;
        }
        Ok(PreparedSystemRestore { journal, checkpoint })
    }

    /// Capture and verify the actual current configuration, persist its identity
    /// in a new journal, and realize/verify the selected restore stage. Failure
    /// never deletes the checkpoint or overwrites an earlier restore attempt.
    pub fn begin(
        &self,
        request: ConfigurationPreparationRequest<'_>,
    ) -> Result<PreparedConfigurationRestore, PreparationError> {
        if request.scope == RestoreScope::SystemOnly {
            return Err(PreparationError::UnsupportedScope);
        }
        match self.journals.load(&request.restore_id) {
            Err(JournalError::Filesystem(Errno::NOENT)) => {}
            Ok(_) => return Err(JournalError::AlreadyExists.into()),
            Err(error) => return Err(error.into()),
        }
        let source = self
            .generations
            .open_verified_transaction(&request.source.manifest().transaction_id)?;
        if source.manifest() != request.source.manifest() {
            return Err(PreparationError::InvalidSource);
        }
        let checkpoint_link = request.checkpoint.snapshot.link();
        if request.checkpoint.transaction_id != request.restore_id
            || request.restore_id == source.manifest().transaction_id
            || request.checkpoint.generation_id == source.manifest().generation_id
            || request.checkpoint.reason != OperationReason::ConfigurationRestore
            || (checkpoint_link.snapper_config == source.manifest().snapper_config
                && checkpoint_link.filesystem_uuid == source.manifest().filesystem_uuid
                && checkpoint_link.snapper_snapshot_id == source.manifest().snapper_snapshot_id)
            || (request.scope == RestoreScope::CompleteLinked
                && (checkpoint_link.filesystem_uuid != source.manifest().filesystem_uuid
                    || checkpoint_link.snapper_config != source.manifest().snapper_config))
        {
            return Err(PreparationError::InvalidCheckpointIdentity);
        }
        for entry in source
            .manifest()
            .files()
            .iter()
            .filter(|entry| entry.root != StateRoot::StateGeneration)
        {
            if !request
                .checkpoint
                .roots
                .iter()
                .any(|root| root.root == entry.root && root.paths.contains(&entry.path))
            {
                return Err(PreparationError::MissingCheckpointCoverage);
            }
        }
        let checkpoint_snapshot = SystemSnapshotStatus::Available {
            snapper_config: checkpoint_link.snapper_config.clone(),
            snapshot_id: checkpoint_link.snapper_snapshot_id.clone(),
            filesystem_uuid: checkpoint_link.filesystem_uuid.clone(),
        };
        let current_roots = request.checkpoint.roots;
        let checkpoint =
            capture_current_state(self.generations, request.checkpoint, source.manifest())?;
        // These verified artifact statuses now derive from actual store/capture
        // verification. Compatibility and native Snapper state remain external.
        let plan = plan_restore(source.manifest(), request.scope, &facts(&request.compatibility))?;
        let journal = RestoreJournal::new(
            request.restore_id.clone(),
            plan,
            checkpoint.generation_id,
            checkpoint.manifest_sha256.ok_or(PreparationError::InvalidCheckpointIdentity)?,
        );
        self.journals.create(&journal)?;
        self.resume(
            &request.restore_id,
            &request.compatibility,
            &checkpoint_snapshot,
            current_roots,
        )
    }

    /// Reconstruct preparation from durable references. The caller must requery
    /// native Snapper and compatibility before passing the observations here.
    /// Later destructive phases belong to the offline recovery executor, not
    /// this entry point. A missing recorded-complete stage is never recreated.
    pub fn resume(
        &self,
        restore_id: &Identifier,
        compatibility: &RestoreCompatibility,
        checkpoint_snapshot: &SystemSnapshotStatus,
        current_roots: &[DeclaredRoot<'_>],
    ) -> Result<PreparedConfigurationRestore, PreparationError> {
        let mut journal = self.journals.load(restore_id)?;
        let pre_apply_phase = matches!(
            journal.phase,
            RestorePhase::Planned | RestorePhase::CheckpointVerified | RestorePhase::ContentStaged
        );
        let configuration_replay_phase = (journal.plan.scope == RestoreScope::ConfigurationOnly
            && matches!(
                journal.phase,
                RestorePhase::WritersStopped
                    | RestorePhase::ConfigurationRestored
                    | RestorePhase::HealthChecked
                    | RestorePhase::Complete
            ))
            || (journal.plan.scope == RestoreScope::CompleteLinked
                && matches!(
                    journal.phase,
                    RestorePhase::RootRollbackComplete
                        | RestorePhase::ConfigurationRestored
                        | RestorePhase::HealthChecked
                        | RestorePhase::Complete
                ));
        if !pre_apply_phase && !configuration_replay_phase {
            return Err(PreparationError::UnexpectedPhase);
        }
        let source = self.generations.open_verified_transaction(&journal.plan.transaction_id)?;
        let checkpoint_manifest = self.generations.verify(&journal.current_state_generation)?;
        let checkpoint =
            self.generations.open_verified_transaction(&checkpoint_manifest.transaction_id)?;
        let checkpoint_manifest = checkpoint.manifest();
        if checkpoint_manifest.generation_id != journal.current_state_generation
            || checkpoint_manifest.generation_id == source.manifest().generation_id
            || checkpoint_manifest.transaction_id != journal.restore_id
            || checkpoint_manifest.manifest_sha256.as_ref()
                != Some(&journal.current_state_manifest_sha256)
            || checkpoint_manifest.reason != OperationReason::ConfigurationRestore
            || (checkpoint_manifest.snapper_config == source.manifest().snapper_config
                && checkpoint_manifest.filesystem_uuid == source.manifest().filesystem_uuid
                && checkpoint_manifest.snapper_snapshot_id == source.manifest().snapper_snapshot_id)
            || (journal.plan.scope == RestoreScope::CompleteLinked
                && (checkpoint_manifest.filesystem_uuid != source.manifest().filesystem_uuid
                    || checkpoint_manifest.snapper_config != source.manifest().snapper_config))
        {
            return Err(PreparationError::InvalidCheckpointIdentity);
        }
        match checkpoint_snapshot {
            SystemSnapshotStatus::Available { snapper_config, snapshot_id, filesystem_uuid }
                if *snapper_config == checkpoint_manifest.snapper_config
                    && *snapshot_id == checkpoint_manifest.snapper_snapshot_id
                    && *filesystem_uuid == checkpoint_manifest.filesystem_uuid => {}
            _ => return Err(PreparationError::CheckpointSnapshotUnavailable),
        }
        if plan_restore(source.manifest(), journal.plan.scope, &facts(compatibility))?
            != journal.plan
        {
            return Err(PreparationError::InvalidSource);
        }
        for entry in source
            .manifest()
            .files()
            .iter()
            .filter(|entry| entry.root != StateRoot::StateGeneration)
        {
            if !checkpoint_manifest
                .files()
                .iter()
                .any(|saved| saved.root == entry.root && saved.path == entry.path)
            {
                return Err(PreparationError::MissingCheckpointCoverage);
            }
        }
        if pre_apply_phase {
            verify_live_checkpoint(checkpoint_manifest, current_roots)?;
        }
        let source = stage_configuration(&source, &journal.plan)?;
        if journal.phase == RestorePhase::Planned {
            self.journals.advance(&mut journal, RestorePhase::CheckpointVerified)?;
        }
        let stage = if journal.phase == RestorePhase::CheckpointVerified {
            match self.stages.create(&source) {
                Ok(_) | Err(DiskStageError::Filesystem(Errno::EXIST)) => {}
                Err(error) => return Err(error.into()),
            }
            let stage = self.stages.open_verified(&source)?;
            if !configuration_replay_phase {
                verify_live_checkpoint(checkpoint.manifest(), current_roots)?;
            }
            self.journals.advance(&mut journal, RestorePhase::ContentStaged)?;
            stage
        } else {
            let stage = self.stages.open_verified(&source)?;
            if journal.phase == RestorePhase::ContentStaged {
                verify_live_checkpoint(checkpoint.manifest(), current_roots)?;
            }
            stage
        };
        Ok(PreparedConfigurationRestore { journal, checkpoint, source, stage })
    }
}

fn capture_current_state(
    store: &GenerationStore,
    request: GenerationCaptureRequest<'_>,
    source: &RecoveryManifest,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    if let Some(principal) = source.account_principal() {
        // The console resolves this descriptor from the bound local account.
        // Reuse the installed complete inventory and profile provenance rather
        // than producing an unbound ad-hoc checkpoint from saved path names.
        if request.roots.len() != 1 || request.roots[0].root != StateRoot::UserHome {
            return Err(GenerationCaptureError::Inventory(
                crate::installed_capture::InventoryError::IncompleteInventory,
            ));
        }
        let home = request.roots[0].descriptor;
        let home_metadata = rustix::fs::fstat(home)?;
        let inventory = crate::installed_capture::InstalledInventory::for_home(home)?;
        if inventory.paths_for_root(StateRoot::UserHome) != Some(request.roots[0].paths) {
            return Err(GenerationCaptureError::Inventory(
                crate::installed_capture::InventoryError::IncompleteInventory,
            ));
        }
        let bundle =
            crate::installed_profiles::InstalledProfileBundle::load_packaged_defaults(None, None)
                .map_err(GenerationCaptureError::ProfileStore)?;
        if !bundle.matches_packages(&request.package_revisions) {
            return Err(GenerationCaptureError::Inventory(
                crate::installed_capture::InventoryError::ProfileIdentityMismatch,
            ));
        }
        let profiles = crate::installed_profiles::ProfileBundleStore::open_state_mount()
            .map_err(GenerationCaptureError::ProfileStore)?;
        let bundle = profiles
            .publish_candidate(&request.transaction_id, &bundle)
            .map_err(GenerationCaptureError::ProfileStore)?;
        return crate::installed_capture::capture_with_installed_profile_bundle(
            store,
            &inventory,
            crate::installed_capture::InstalledRoots {
                user_configuration: home,
                account_home: Some(home),
                system_configuration: None,
                account: crate::installed_capture::CaptureAccount {
                    uid: home_metadata.st_uid,
                    gid: home_metadata.st_gid,
                    principal: principal.clone(),
                },
            },
            crate::installed_capture::InstalledCaptureRequest {
                transaction_id: request.transaction_id,
                generation_id: request.generation_id,
                parent_generation_id: request.parent_generation_id,
                snapshot: request.snapshot,
                reason: request.reason,
                created_utc: request.created_utc,
                package_revisions: request.package_revisions,
                provider_revision: request.provider_revision,
                limits: request.limits,
            },
            &bundle,
        );
    }
    capture_publish_verify_checked(
        store,
        request,
        capture_inventory,
        |store, bytes, files| store.ensure_available_space(bytes, files),
        |_, _, profile_revision| {
            Ok(CaptureProvenance {
                profile_revision,
                account_principal: source.account_principal().cloned(),
            })
        },
        || Ok(()),
    )
}

fn verify_live_checkpoint(
    checkpoint: &RecoveryManifest,
    roots: &[DeclaredRoot<'_>],
) -> Result<(), PreparationError> {
    let expected_keys: BTreeSet<_> =
        checkpoint.files().iter().map(|entry| (entry.root, entry.path.clone())).collect();
    let observed_keys: BTreeSet<_> = roots
        .iter()
        .flat_map(|root| root.paths.iter().map(|path| (root.root, path.clone())))
        .collect();
    let unique_roots: BTreeSet<_> = roots.iter().map(|root| root.root).collect();
    if expected_keys != observed_keys || unique_roots.len() != roots.len() {
        return Err(PreparationError::MissingCheckpointCoverage);
    }
    let mut records = BTreeMap::new();
    let mut bytes = 0_u64;
    for root in roots {
        let captured =
            capture_inventory(root.descriptor, root.root, root.paths, CaptureLimits::default())?;
        bytes = bytes.saturating_add(captured.source_bytes);
        if bytes > CaptureLimits::default().max_total_bytes {
            return Err(PreparationError::LiveStateChanged);
        }
        for record in captured.records {
            records.insert((record.root, record.path.clone()), record);
        }
    }
    let mut groups = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    for expected in checkpoint.files() {
        let mut actual = records
            .remove(&(expected.root, expected.path.clone()))
            .ok_or(PreparationError::MissingCheckpointCoverage)?;
        if let (
            EntryKind::RegularFile { hard_link_group: expected_group, .. },
            EntryKind::RegularFile { hard_link_group: actual_group, .. },
        ) = (&expected.kind, &mut actual.kind)
        {
            match (expected_group, actual_group.as_ref()) {
                (Some(expected), Some(actual)) => {
                    if groups
                        .insert(expected.clone(), actual.clone())
                        .is_some_and(|old| old != *actual)
                        || reverse
                            .insert(actual.clone(), expected.clone())
                            .is_some_and(|old| old != *expected)
                    {
                        return Err(PreparationError::LiveStateChanged);
                    }
                }
                (None, None) => {}
                _ => return Err(PreparationError::LiveStateChanged),
            }
            *actual_group = expected_group.clone();
        }
        if actual.kind != expected.kind {
            return Err(PreparationError::LiveStateChanged);
        }
    }
    Ok(())
}

fn facts(compatibility: &RestoreCompatibility) -> PreflightEvidence {
    PreflightEvidence {
        generation: ArtifactStatus::Verified,
        archive: ArtifactStatus::Verified,
        current_state_checkpoint: ArtifactStatus::Verified,
        system_snapshot: compatibility.source_snapshot.clone(),
        system_compatibility: compatibility.system,
        configuration_compatibility: compatibility.configuration,
    }
}

fn source_facts(compatibility: &RestoreCompatibility) -> RestoreSourceEvidence {
    RestoreSourceEvidence {
        generation: ArtifactStatus::Verified,
        archive: ArtifactStatus::Verified,
        system_snapshot: compatibility.source_snapshot.clone(),
        system_compatibility: compatibility.system,
        configuration_compatibility: compatibility.configuration,
    }
}

#[cfg(test)]
mod rollback_confirmation_tests {
    use super::*;

    #[test]
    fn root_rollback_confirmation_distinguishes_first_write_from_safe_retries() {
        for scope in [RestoreScope::SystemOnly, RestoreScope::CompleteLinked] {
            assert_eq!(
                rollback_confirmation_transition(scope, RestorePhase::WritersStopped),
                Some(true)
            );
            for phase in [
                RestorePhase::RootRollbackComplete,
                RestorePhase::ConfigurationRestored,
                RestorePhase::HealthChecked,
                RestorePhase::Complete,
            ] {
                assert_eq!(rollback_confirmation_transition(scope, phase), Some(false));
            }
            for phase in [
                RestorePhase::Planned,
                RestorePhase::CheckpointVerified,
                RestorePhase::ContentStaged,
            ] {
                assert_eq!(rollback_confirmation_transition(scope, phase), None);
            }
        }
        assert_eq!(
            rollback_confirmation_transition(
                RestoreScope::ConfigurationOnly,
                RestorePhase::RootRollbackComplete
            ),
            None
        );
    }
}
