//! Preparation through a verified current-state generation and durable disk
//! stage. The caller must authorize recovery, resolve the complete approved
//! inventory, validate persistent mounts outside rollback scope, and keep all
//! writers frozen from snapshot creation through handoff to the offline writer.
//! Snapper availability/compatibility observations are trusted caller inputs;
//! this module neither creates native system snapshots nor stops services.

use crate::{
    EntryKind, Identifier, OperationReason, RecoveryManifest, StateRoot,
    capture::{CaptureError, CaptureLimits, capture_inventory},
    configuration_stage::{StageError, StagedConfiguration, stage_configuration},
    disk_stage::{DiskStageError, DiskStageStore, VerifiedDiskStage},
    generation::{GenerationError, GenerationStore, VerifiedGeneration},
    generation_capture::{
        DeclaredRoot, GenerationCaptureError, GenerationCaptureRequest, capture_publish_verify,
    },
    restore::{
        ArtifactStatus, CompatibilityStatus, JournalError, PreflightError, PreflightEvidence,
        RestoreJournal, RestoreJournalStore, RestorePhase, RestoreScope, SystemSnapshotStatus,
        plan_restore,
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

pub struct ConfigurationPreparationRequest<'a> {
    pub restore_id: Identifier,
    pub source: &'a VerifiedGeneration,
    pub scope: RestoreScope,
    pub compatibility: RestoreCompatibility,
    /// Fresh independently verified Snapper link plus the complete approved
    /// current-state inventory/provenance. Every affected target must be covered.
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
        let checkpoint = capture_publish_verify(self.generations, request.checkpoint)?;
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
        if !matches!(
            journal.phase,
            RestorePhase::Planned | RestorePhase::CheckpointVerified | RestorePhase::ContentStaged
        ) || journal.plan.scope == RestoreScope::SystemOnly
        {
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
        verify_live_checkpoint(checkpoint_manifest, current_roots)?;
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
            verify_live_checkpoint(checkpoint.manifest(), current_roots)?;
            self.journals.advance(&mut journal, RestorePhase::ContentStaged)?;
            stage
        } else {
            let stage = self.stages.open_verified(&source)?;
            verify_live_checkpoint(checkpoint.manifest(), current_roots)?;
            stage
        };
        Ok(PreparedConfigurationRestore { journal, checkpoint, source, stage })
    }
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
