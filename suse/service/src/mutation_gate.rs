//! Fail-closed sequencing contract for supported mutations.
//!
//! This module does not capture recovery state or execute provider operations.
//! Integrations must provide a trusted checkpoint verifier and plan revalidator.
//! `MutationLock` pins a private service-state directory and opens one fixed lock
//! file afresh for each operation. `flock` coordinates cooperating Ryoku
//! processes on one host and releases on process exit; it cannot constrain
//! uncooperative providers.

use std::fs::File;
use std::io;

use rustix::fs::{FileType, FlockOperation, Mode, OFlags, flock, fstat, fsync, openat};
use ryoku_recovery::{Identifier, Sha256, SnapshotLink};
use serde::{Deserialize, Serialize};

use crate::{
    JournalError, ManagedActionKind, OperationId, OperationJournal, OperationPhase,
    OperationStatus, ServiceError,
};

/// Identity pair for the system snapshot and Ryoku/configuration generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedGeneration {
    pub transaction_id: Identifier,
    pub generation_id: Identifier,
    pub manifest_sha256: Sha256,
    pub snapshot: SnapshotLink,
}

impl LinkedGeneration {
    /// Copy the exact sealed identity from persistent generation read-back.
    /// This reference alone proves neither installed inventory completeness nor
    /// current native snapshot availability, which the integration must check.
    pub fn from_verified_generation(
        generation: &ryoku_recovery::generation::VerifiedGeneration,
    ) -> Result<Self, GateError> {
        let manifest = generation.manifest();
        let linked = Self {
            transaction_id: manifest.transaction_id.clone(),
            generation_id: manifest.generation_id.clone(),
            manifest_sha256: manifest
                .manifest_sha256
                .clone()
                .ok_or(GateError::CheckpointUnverified)?,
            snapshot: SnapshotLink::new(
                manifest.snapper_config.clone(),
                manifest.snapper_snapshot_id.clone(),
                manifest.filesystem_uuid.clone(),
            )
            .map_err(|_| GateError::CheckpointUnverified)?,
        };
        if !linked.valid() {
            return Err(GateError::CheckpointUnverified);
        }
        Ok(linked)
    }

    fn valid(&self) -> bool {
        if self.snapshot.snapper_snapshot_id.len() > 20 || self.snapshot.filesystem_uuid.len() != 36
        {
            return false;
        }
        let number = self.snapshot.snapper_snapshot_id.parse::<u64>();
        matches!(number, Ok(number) if number != 0 && number.to_string() == self.snapshot.snapper_snapshot_id)
            && SnapshotLink::new(
                self.snapshot.snapper_config.clone(),
                self.snapshot.snapper_snapshot_id.clone(),
                self.snapshot.filesystem_uuid.clone(),
            )
            .is_ok()
    }

    fn fresh_after(&self, before: &Self) -> bool {
        self.valid()
            && self.transaction_id != before.transaction_id
            && self.generation_id != before.generation_id
            && self.manifest_sha256 != before.manifest_sha256
            && self.snapshot.snapper_config == before.snapshot.snapper_config
            && self.snapshot.filesystem_uuid == before.snapshot.filesystem_uuid
            && self.snapshot.snapper_snapshot_id != before.snapshot.snapper_snapshot_id
    }
}

#[cfg(test)]
#[path = "mutation_coordination_tests.rs"]
mod coordination_tests;

/// Evidence reported by the recovery integration after it has captured and
/// read-back verified every component required by the operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointEvidence {
    pub linked: LinkedGeneration,
    pub system_component_verified: bool,
    pub ryoku_component_verified: bool,
    pub contents_and_metadata_verified: bool,
}

/// Validated trusted integration attestation. Its private fields ensure shape
/// checks, but these booleans do not independently prove native capture,
/// complete installed enrollment, writer quiescence, or authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedLinkedGeneration(LinkedGeneration);

impl VerifiedLinkedGeneration {
    pub fn validate(evidence: CheckpointEvidence) -> Result<Self, GateError> {
        if !evidence.linked.valid()
            || !evidence.system_component_verified
            || !evidence.ryoku_component_verified
            || !evidence.contents_and_metadata_verified
        {
            return Err(GateError::CheckpointUnverified);
        }
        Ok(Self(evidence.linked))
    }

    pub fn linked(&self) -> &LinkedGeneration {
        &self.0
    }
}

/// Trusted post-change attestation bound to this exact preimage. Validation
/// requires independently read-back complete state and a fresh immutable/native
/// reference; the booleans remain trusted assertions, not native verification.
pub struct VerifiedPostChangeGeneration {
    preimage: LinkedGeneration,
    linked: LinkedGeneration,
}
impl VerifiedPostChangeGeneration {
    pub fn validate(
        evidence: CheckpointEvidence,
        before: &VerifiedLinkedGeneration,
    ) -> Result<Self, GateError> {
        let after = VerifiedLinkedGeneration::validate(evidence)
            .map_err(|_| GateError::PostChangeFailed)?;
        if !after.linked().fresh_after(before.linked()) {
            return Err(GateError::PostChangeFailed);
        }
        Ok(Self { preimage: before.linked().clone(), linked: after.0 })
    }
    pub fn linked(&self) -> &LinkedGeneration {
        &self.linked
    }
}

/// A request fingerprint that must still match after checkpoint verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedChange {
    pub operation_key: String,
    pub plan_fingerprint: String,
    pub affected_state_fingerprint: String,
}

impl PlannedChange {
    fn valid(&self) -> bool {
        !self.operation_key.is_empty()
            && self.operation_key.len() <= 256
            && self.operation_key.bytes().all(|byte| byte.is_ascii_graphic())
            && Sha256::new(self.plan_fingerprint.clone()).is_ok()
            && Sha256::new(self.affected_state_fingerprint.clone()).is_ok()
    }
}

/// Detailed durable phase; the socket's coarse protocol-v1 phase is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationStage {
    Preflight,
    Capturing,
    Revalidating,
    Applying,
    HealthChecking,
    PostChangeCapture,
    Complete,
}

impl MutationStage {
    fn next(self) -> Option<Self> {
        match self {
            Self::Preflight => Some(Self::Capturing),
            Self::Capturing => Some(Self::Revalidating),
            Self::Revalidating => Some(Self::Applying),
            Self::Applying => Some(Self::HealthChecking),
            Self::HealthChecking => Some(Self::PostChangeCapture),
            Self::PostChangeCapture => Some(Self::Complete),
            Self::Complete => None,
        }
    }
    fn phase(self) -> OperationPhase {
        match self {
            Self::Preflight => OperationPhase::Accepted,
            Self::Capturing | Self::Revalidating => OperationPhase::Capturing,
            Self::Applying => OperationPhase::Applying,
            Self::HealthChecking | Self::PostChangeCapture => OperationPhase::Verifying,
            Self::Complete => OperationPhase::Succeeded,
        }
    }
    fn before_change(self) -> bool {
        matches!(self, Self::Preflight | Self::Capturing | Self::Revalidating)
    }
}

/// Failure classification survives reconnect independently of generic v1 errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOutcome {
    FailedBeforeChange,
    CancelledBeforeChange,
    MayHaveMutated,
    PostChangeCaptureFailed,
}

/// Exact plan and linked checkpoint association stored in the same synchronized
/// record as each phase. This is historical evidence, never a replay permit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationContext {
    pub plan: PlannedChange,
    pub checkpoint: Option<LinkedGeneration>,
    pub post_checkpoint: Option<LinkedGeneration>,
    pub stage: MutationStage,
    pub outcome: Option<MutationOutcome>,
}

impl MutationContext {
    pub(crate) fn valid_for(&self, status: &OperationStatus) -> bool {
        if !self.plan.valid() || status.progress.is_some() {
            return false;
        }
        let needs_checkpoint =
            !matches!(self.stage, MutationStage::Preflight | MutationStage::Capturing);
        if self.checkpoint.is_some() != needs_checkpoint
            || self.checkpoint.as_ref().is_some_and(|checkpoint| !checkpoint.valid())
        {
            return false;
        }
        if self.post_checkpoint.is_some() != (self.stage == MutationStage::Complete)
            || self.post_checkpoint.as_ref().is_some_and(|post| {
                !self.checkpoint.as_ref().is_some_and(|before| post.fresh_after(before))
            })
        {
            return false;
        }
        if let Some(outcome) = self.outcome {
            status.phase == OperationPhase::Failed
                && status.error.is_some()
                && match outcome {
                    MutationOutcome::FailedBeforeChange => {
                        self.stage.before_change()
                            && matches!(
                                status.error,
                                Some(
                                    ServiceError::RecoveryUnavailable
                                        | ServiceError::StaleRequest
                                        | ServiceError::Busy
                                )
                            )
                    }
                    MutationOutcome::CancelledBeforeChange => {
                        self.stage.before_change() && status.error == Some(ServiceError::Failed)
                    }
                    MutationOutcome::MayHaveMutated => {
                        matches!(
                            self.stage,
                            MutationStage::Applying | MutationStage::HealthChecking
                        ) && status.error == Some(ServiceError::Failed)
                    }
                    MutationOutcome::PostChangeCaptureFailed => {
                        self.stage == MutationStage::PostChangeCapture
                            && status.error == Some(ServiceError::Failed)
                    }
                }
        } else {
            status.phase == self.stage.phase() && status.error.is_none()
        }
    }

    pub(crate) fn follows(&self, previous: &Self) -> bool {
        if previous.outcome.is_some()
            || previous.stage == MutationStage::Complete
            || self.plan != previous.plan
        {
            return false;
        }
        let checkpoint_matches = if previous.stage == MutationStage::Capturing
            && self.stage == MutationStage::Revalidating
            && self.outcome.is_none()
        {
            previous.checkpoint.is_none() && self.checkpoint.is_some()
        } else {
            self.checkpoint == previous.checkpoint
        };
        let post_matches = if previous.stage == MutationStage::PostChangeCapture
            && self.stage == MutationStage::Complete
            && self.outcome.is_none()
        {
            previous.post_checkpoint.is_none() && self.post_checkpoint.is_some()
        } else {
            self.post_checkpoint == previous.post_checkpoint
        };
        checkpoint_matches
            && post_matches
            && if self.outcome.is_some() {
                self.stage == previous.stage
            } else {
                previous.stage.next() == Some(self.stage)
            }
    }

    pub(crate) fn initial(&self) -> bool {
        self.stage == MutationStage::Preflight
            && self.checkpoint.is_none()
            && self.post_checkpoint.is_none()
            && self.outcome.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateError {
    Lock(io::ErrorKind),
    Journal(JournalError),
    InvalidPlan,
    RecoveryRequired,
    PreflightFailed,
    InsufficientSpace,
    CancelledBeforeChange,
    CheckpointUnverified,
    PlanChanged,
    ApplyFailed,
    PostChangeFailed,
}

/// Durable operation identity and append-only journal for one guarded change.
pub struct JournaledOperation<'a> {
    journal: &'a mut OperationJournal,
    operation_id: OperationId,
    action: ManagedActionKind,
    context: Option<MutationContext>,
}

impl<'a> JournaledOperation<'a> {
    pub fn new(
        journal: &'a mut OperationJournal,
        operation_id: OperationId,
        action: ManagedActionKind,
    ) -> Self {
        Self { journal, operation_id, action, context: None }
    }

    fn record(
        &mut self,
        phase: OperationPhase,
        error: Option<ServiceError>,
    ) -> Result<(), GateError> {
        self.journal
            .record_mutation(
                OperationStatus {
                    operation_id: self.operation_id.clone(),
                    action: self.action,
                    phase,
                    progress: None,
                    error,
                },
                self.context.clone().ok_or(GateError::InvalidPlan)?,
            )
            .map_err(GateError::Journal)
    }

    fn fail(&mut self, category: ServiceError, cause: GateError) -> GateError {
        let context = self.context.as_mut().expect("accepted operation has context");
        context.outcome =
            Some(if cause == GateError::CancelledBeforeChange && context.stage.before_change() {
                MutationOutcome::CancelledBeforeChange
            } else if context.stage.before_change() {
                MutationOutcome::FailedBeforeChange
            } else if context.stage == MutationStage::PostChangeCapture {
                MutationOutcome::PostChangeCaptureFailed
            } else {
                MutationOutcome::MayHaveMutated
            });
        match self.record(OperationPhase::Failed, Some(category)) {
            Ok(()) => cause,
            Err(journal_error) => journal_error,
        }
    }

    fn advance(&mut self, stage: MutationStage) -> Result<(), GateError> {
        self.context.as_mut().expect("accepted operation has context").stage = stage;
        self.record(stage.phase(), None)
    }
}

/// Persist pre-change failures using only error codes already defined by
/// protocol v1. This keeps old clients able to deserialize operation status.
fn pre_change_error(cause: &GateError) -> ServiceError {
    match cause {
        GateError::Lock(_) => ServiceError::Busy,
        GateError::PlanChanged => ServiceError::StaleRequest,
        GateError::CancelledBeforeChange => ServiceError::Failed,
        _ => ServiceError::RecoveryUnavailable,
    }
}

const MUTATION_LOCK_NAME: &str = "mutation.lock";

/// Stable location for the coordinator lock used by cooperating Ryoku writers.
/// Construct this from a trusted, service-owned state-directory descriptor.
pub struct MutationLock {
    directory: File,
    expected_uid: u32,
}

impl MutationLock {
    /// Validate and pin a private state directory, creating the fixed lock file
    /// if needed. The directory must be owned by `expected_uid` and inaccessible
    /// to group and other users. Each `acquire` performs a fresh open so threads
    /// and processes contend using distinct open-file descriptions for the same
    /// lock inode.
    pub fn open(directory: File, expected_uid: u32) -> Result<Self, GateError> {
        validate_lock_directory(&directory, expected_uid)?;
        let lock = Self { directory, expected_uid };
        drop(lock.open_lock_file()?);
        fsync(&lock.directory).map_err(lock_error)?;
        Ok(lock)
    }

    /// Block until all cooperating Ryoku writers using this lock directory
    /// release the common advisory lock.
    pub fn acquire(&self) -> Result<WriterGuard, GateError> {
        WriterGuard::acquire(self.open_lock_file()?)
    }

    fn open_lock_file(&self) -> Result<File, GateError> {
        validate_lock_directory(&self.directory, self.expected_uid)?;
        let descriptor = openat(
            &self.directory,
            MUTATION_LOCK_NAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(lock_error)?;
        let file = File::from(descriptor);
        let metadata = fstat(&file).map_err(lock_error)?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
            || metadata.st_uid != self.expected_uid
            || metadata.st_mode & 0o7777 != 0o600
            || metadata.st_nlink != 1
        {
            return Err(GateError::Lock(io::ErrorKind::PermissionDenied));
        }
        Ok(file)
    }
}

fn validate_lock_directory(directory: &File, expected_uid: u32) -> Result<(), GateError> {
    let metadata = fstat(directory).map_err(lock_error)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
        || metadata.st_uid != expected_uid
        || metadata.st_mode & 0o077 != 0
    {
        return Err(GateError::Lock(io::ErrorKind::PermissionDenied));
    }
    Ok(())
}

fn lock_error(error: rustix::io::Errno) -> GateError {
    GateError::Lock(io::Error::from(error).kind())
}

/// Holds an exclusive interprocess writer lock until dropped or the process exits.
pub struct WriterGuard {
    _file: File,
}

impl WriterGuard {
    fn acquire(service_owned_lock_file: File) -> Result<Self, GateError> {
        flock(&service_owned_lock_file, FlockOperation::LockExclusive)
            .map_err(|error| GateError::Lock(error.kind()))?;
        Ok(Self { _file: service_owned_lock_file })
    }
}

/// Implemented by an integration that can create the linked pre-change state
/// and verify it from persisted state. A successful return alone is not enough:
/// the returned evidence must name both linked generations and assert all three
/// verification dimensions. This interface intentionally performs no writes.
pub trait CheckpointVerifier {
    fn create_and_verify(&mut self) -> Result<CheckpointEvidence, GateError>;
}

/// Rechecks the exact reviewed plan and its covered paths after capture. Any
/// mismatch rejects the operation before the apply callback is invoked.
pub trait PlanRevalidator {
    fn still_matches(&mut self, planned: &PlannedChange) -> Result<bool, GateError>;
}

/// Test-only unjournaled variant retained for focused gate unit tests.
/// Production callers must use [`with_journaled_verified_mutation`] so every
/// supported mutation has durable operation history before its callback runs.
#[cfg(test)]
pub fn with_verified_prechange<T>(
    lock_file: File,
    planned: &PlannedChange,
    checkpoint: &mut impl CheckpointVerifier,
    revalidator: &mut impl PlanRevalidator,
    apply: impl FnOnce(&VerifiedLinkedGeneration) -> T,
) -> Result<T, GateError> {
    let _writer = WriterGuard::acquire(lock_file)?;
    let proof = VerifiedLinkedGeneration::validate(checkpoint.create_and_verify()?)?;
    if !revalidator.still_matches(planned)? {
        return Err(GateError::PlanChanged);
    }
    Ok(apply(&proof))
}

/// Trusted integration hooks. No implementation is installed in this build.
/// Preflight must authorize the exact plan and check state/space; capture must
/// prove complete declared state and native linkage from persistent read-back.
/// Boolean attestations remain an integration boundary, not native evidence.
pub trait MutationIntegration {
    type Output;
    fn preflight(&mut self, plan: &PlannedChange) -> Result<(), GateError>;
    fn capture(&mut self, plan: &PlannedChange) -> Result<CheckpointEvidence, GateError>;
    fn revalidate(
        &mut self,
        plan: &PlannedChange,
        checkpoint: &VerifiedLinkedGeneration,
    ) -> Result<bool, GateError>;
    fn apply(&mut self, checkpoint: &VerifiedLinkedGeneration) -> Result<Self::Output, GateError>;
    fn health_check(
        &mut self,
        result: &Self::Output,
        checkpoint: &VerifiedLinkedGeneration,
    ) -> Result<(), GateError>;
    fn capture_post_change(
        &mut self,
        result: &Self::Output,
        checkpoint: &VerifiedLinkedGeneration,
    ) -> Result<VerifiedPostChangeGeneration, GateError>;
    fn cancellation_requested(&self) -> bool {
        false
    }
}

/// Synchronize every phase and its exact plan/checkpoint association before
/// entering its callback. All phases share the existing interprocess lock.
/// Reload under that lock and refuse unresolved prior work; status alone never
/// authorizes automatic resume or repeating an apply. The caller retains writer
/// quiescence across these hooks; this advisory lock cannot stop external tools.
pub fn coordinate_mutation<I: MutationIntegration>(
    lock: &MutationLock,
    planned: &PlannedChange,
    operation: &mut JournaledOperation<'_>,
    integration: &mut I,
) -> Result<I::Output, GateError> {
    if !planned.valid() {
        return Err(GateError::InvalidPlan);
    }
    // Acquire before the first journal append. Cooperating processes share this
    // lock across every phase, preventing records or provider work from
    // interleaving around one operation.
    let _writer = lock.acquire()?;
    operation.journal.refresh().map_err(GateError::Journal)?;
    if !operation.journal.recovery_required().is_empty() {
        return Err(GateError::RecoveryRequired);
    }
    operation.context = Some(MutationContext {
        plan: planned.clone(),
        checkpoint: None,
        post_checkpoint: None,
        stage: MutationStage::Preflight,
        outcome: None,
    });
    operation.record(OperationPhase::Accepted, None)?;
    if integration.cancellation_requested() {
        return Err(operation.fail(ServiceError::Failed, GateError::CancelledBeforeChange));
    }
    if let Err(error) = integration.preflight(planned) {
        return Err(operation.fail(pre_change_error(&error), error));
    }
    if integration.cancellation_requested() {
        return Err(operation.fail(ServiceError::Failed, GateError::CancelledBeforeChange));
    }
    operation.advance(MutationStage::Capturing)?;
    let evidence = match integration.capture(planned) {
        Ok(evidence) => evidence,
        Err(error) => {
            let category = pre_change_error(&error);
            return Err(operation.fail(category, error));
        }
    };
    let proof = match VerifiedLinkedGeneration::validate(evidence) {
        Ok(proof) => proof,
        Err(error) => {
            let category = pre_change_error(&error);
            return Err(operation.fail(category, error));
        }
    };
    operation.context.as_mut().expect("accepted operation has context").checkpoint =
        Some(proof.linked().clone());
    operation.advance(MutationStage::Revalidating)?;
    if integration.cancellation_requested() {
        return Err(operation.fail(ServiceError::Failed, GateError::CancelledBeforeChange));
    }
    let still_matches = match integration.revalidate(planned, &proof) {
        Ok(value) => value,
        Err(error) => {
            let category = pre_change_error(&error);
            return Err(operation.fail(category, error));
        }
    };
    if !still_matches {
        let error = GateError::PlanChanged;
        let category = pre_change_error(&error);
        return Err(operation.fail(category, error));
    }
    if integration.cancellation_requested() {
        return Err(operation.fail(ServiceError::Failed, GateError::CancelledBeforeChange));
    }

    // If this durable write fails, the mutation callback is never entered.
    operation.advance(MutationStage::Applying)?;
    let result = match integration.apply(&proof) {
        Ok(result) => result,
        Err(error) => return Err(operation.fail(ServiceError::Failed, error)),
    };

    // Cancellation after Applying never skips health or post-change capture:
    // providers may already have changed state and cannot be safely unwound by
    // a cancellation flag. Their errors are classified as recovery-required.
    operation.advance(MutationStage::HealthChecking)?;
    if let Err(error) = integration.health_check(&result, &proof) {
        return Err(operation.fail(ServiceError::Failed, error));
    }
    operation.advance(MutationStage::PostChangeCapture)?;
    let post = match integration.capture_post_change(&result, &proof) {
        Ok(post) if post.preimage == *proof.linked() => post,
        Ok(_) => return Err(operation.fail(ServiceError::Failed, GateError::PostChangeFailed)),
        Err(error) => return Err(operation.fail(ServiceError::Failed, error)),
    };
    operation.context.as_mut().expect("accepted operation has context").post_checkpoint =
        Some(post.linked);
    operation.advance(MutationStage::Complete)?;
    Ok(result)
}

/// Reduced callbacks are retained solely for the older sequencing fixtures.
/// Production callers must provide every `MutationIntegration` hook explicitly.
#[cfg(test)]
fn with_journaled_verified_mutation<T>(
    lock: &MutationLock,
    planned: &PlannedChange,
    operation: &mut JournaledOperation<'_>,
    checkpoint: &mut impl CheckpointVerifier,
    revalidator: &mut impl PlanRevalidator,
    apply: impl FnOnce(&VerifiedLinkedGeneration) -> Result<T, GateError>,
    post: impl FnOnce(&T, &VerifiedLinkedGeneration) -> Result<(), GateError>,
) -> Result<T, GateError> {
    struct Fixture<'a, C, R, A, V, T> {
        checkpoint: &'a mut C,
        revalidator: &'a mut R,
        apply: Option<A>,
        post: Option<V>,
        output: std::marker::PhantomData<T>,
    }
    impl<C, R, A, V, T> MutationIntegration for Fixture<'_, C, R, A, V, T>
    where
        C: CheckpointVerifier,
        R: PlanRevalidator,
        A: FnOnce(&VerifiedLinkedGeneration) -> Result<T, GateError>,
        V: FnOnce(&T, &VerifiedLinkedGeneration) -> Result<(), GateError>,
    {
        type Output = T;
        fn preflight(&mut self, _: &PlannedChange) -> Result<(), GateError> {
            Ok(())
        }
        fn capture(&mut self, _: &PlannedChange) -> Result<CheckpointEvidence, GateError> {
            self.checkpoint.create_and_verify()
        }
        fn revalidate(
            &mut self,
            plan: &PlannedChange,
            _: &VerifiedLinkedGeneration,
        ) -> Result<bool, GateError> {
            self.revalidator.still_matches(plan)
        }
        fn apply(&mut self, proof: &VerifiedLinkedGeneration) -> Result<T, GateError> {
            self.apply.take().unwrap()(proof)
        }
        fn health_check(&mut self, _: &T, _: &VerifiedLinkedGeneration) -> Result<(), GateError> {
            Ok(())
        }
        fn capture_post_change(
            &mut self,
            result: &T,
            proof: &VerifiedLinkedGeneration,
        ) -> Result<VerifiedPostChangeGeneration, GateError> {
            self.post.take().unwrap()(result, proof)?;
            let mut linked = proof.linked().clone();
            linked.transaction_id = Identifier::new("txn-post").unwrap();
            linked.generation_id = Identifier::new("generation-post").unwrap();
            linked.manifest_sha256 = Sha256::new("ff".repeat(32)).unwrap();
            linked.snapshot.snapper_snapshot_id = "43".into();
            VerifiedPostChangeGeneration::validate(
                CheckpointEvidence {
                    linked,
                    system_component_verified: true,
                    ryoku_component_verified: true,
                    contents_and_metadata_verified: true,
                },
                proof,
            )
        }
    }
    coordinate_mutation(
        lock,
        planned,
        operation,
        &mut Fixture {
            checkpoint,
            revalidator,
            apply: Some(apply),
            post: Some(post),
            output: std::marker::PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JournalRecord;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    fn lock_file(tag: &str) -> File {
        let path =
            std::env::temp_dir().join(format!("ryoku-mutation-{tag}-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(path).unwrap();
        file
    }

    struct TestMutationLock {
        path: PathBuf,
        lock: MutationLock,
    }

    impl Drop for TestMutationLock {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn mutation_lock(tag: &str) -> TestMutationLock {
        let path =
            std::env::temp_dir().join(format!("ryoku-mutation-lock-{tag}-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let lock = MutationLock::open(File::open(&path).unwrap(), uid).unwrap();
        TestMutationLock { path, lock }
    }

    fn evidence() -> CheckpointEvidence {
        CheckpointEvidence {
            linked: LinkedGeneration {
                transaction_id: Identifier::new("txn-1").unwrap(),
                generation_id: Identifier::new("generation-1").unwrap(),
                manifest_sha256: Sha256::new("aa".repeat(32)).unwrap(),
                snapshot: SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "11111111-1111-1111-1111-111111111111",
                )
                .unwrap(),
            },
            system_component_verified: true,
            ryoku_component_verified: true,
            contents_and_metadata_verified: true,
        }
    }

    struct FakeCheckpoint {
        evidence: Result<CheckpointEvidence, GateError>,
        trace: Arc<Mutex<Vec<&'static str>>>,
    }
    impl CheckpointVerifier for FakeCheckpoint {
        fn create_and_verify(&mut self) -> Result<CheckpointEvidence, GateError> {
            self.trace.lock().unwrap().push("checkpoint");
            self.evidence.clone()
        }
    }
    struct FakeRevalidator {
        matches: bool,
        trace: Arc<Mutex<Vec<&'static str>>>,
    }
    impl PlanRevalidator for FakeRevalidator {
        fn still_matches(&mut self, _: &PlannedChange) -> Result<bool, GateError> {
            self.trace.lock().unwrap().push("revalidate");
            Ok(self.matches)
        }
    }
    fn planned() -> PlannedChange {
        PlannedChange {
            operation_key: "install:sample".into(),
            plan_fingerprint: "11".repeat(32),
            affected_state_fingerprint: "22".repeat(32),
        }
    }

    fn operation_journal(tag: &str) -> (PathBuf, OperationJournal) {
        let path = std::env::temp_dir()
            .join(format!("ryoku-mutation-journal-{tag}-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        (path, OperationJournal::open(file).unwrap())
    }

    fn persisted_status(path: &Path, id: &OperationId) -> OperationStatus {
        let file = std::fs::OpenOptions::new().read(true).append(true).open(path).unwrap();
        OperationJournal::open(file).unwrap().get(id).cloned().unwrap()
    }

    fn persisted_phases(path: &Path, id: &OperationId) -> Vec<OperationPhase> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<JournalRecord>(line).unwrap())
            .filter(|record| record.status.operation_id == *id)
            .map(|record| record.status.phase)
            .collect()
    }

    fn operation_id(value: &str) -> OperationId {
        OperationId::new(value).unwrap()
    }

    fn action() -> ManagedActionKind {
        ManagedActionKind::InstallPackage
    }

    #[test]
    fn apply_occurs_only_after_verified_checkpoint_and_plan_revalidation() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint = FakeCheckpoint { evidence: Ok(evidence()), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace: trace.clone() };
        let result = with_verified_prechange(
            lock_file("ordered"),
            &planned(),
            &mut checkpoint,
            &mut revalidator,
            |proof| {
                assert_eq!(proof.linked().snapshot.snapper_snapshot_id, "42");
                trace.lock().unwrap().push("apply");
                42
            },
        )
        .unwrap();
        assert_eq!(result, 42);
        assert_eq!(*trace.lock().unwrap(), ["checkpoint", "revalidate", "apply"]);
    }

    #[test]
    fn missing_checkpoint_component_blocks_apply() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut invalid = evidence();
        invalid.ryoku_component_verified = false;
        let mut checkpoint = FakeCheckpoint { evidence: Ok(invalid), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace: trace.clone() };
        let result = with_verified_prechange(
            lock_file("missing"),
            &planned(),
            &mut checkpoint,
            &mut revalidator,
            |_| {
                trace.lock().unwrap().push("apply");
            },
        );
        assert_eq!(result, Err(GateError::CheckpointUnverified));
        assert_eq!(*trace.lock().unwrap(), ["checkpoint"]);
    }

    #[test]
    fn changed_plan_blocks_apply_after_checkpoint() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint = FakeCheckpoint { evidence: Ok(evidence()), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: false, trace: trace.clone() };
        let result = with_verified_prechange(
            lock_file("stale"),
            &planned(),
            &mut checkpoint,
            &mut revalidator,
            |_| {
                trace.lock().unwrap().push("apply");
            },
        );
        assert_eq!(result, Err(GateError::PlanChanged));
        assert_eq!(*trace.lock().unwrap(), ["checkpoint", "revalidate"]);
    }

    #[test]
    fn checkpoint_failure_blocks_revalidation_and_apply() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint =
            FakeCheckpoint { evidence: Err(GateError::CheckpointUnverified), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace: trace.clone() };
        let result = with_verified_prechange(
            lock_file("error"),
            &planned(),
            &mut checkpoint,
            &mut revalidator,
            |_| {
                trace.lock().unwrap().push("apply");
            },
        );
        assert_eq!(result, Err(GateError::CheckpointUnverified));
        assert_eq!(*trace.lock().unwrap(), ["checkpoint"]);
    }

    #[test]
    fn journaled_mutation_persists_each_phase_before_callbacks() {
        let (path, mut journal) = operation_journal("success");
        let id = operation_id("op-success");
        let mut operation = JournaledOperation::new(&mut journal, id.clone(), action());
        let lock = mutation_lock("durable-success");
        struct DurableCheckpoint {
            path: PathBuf,
            id: OperationId,
        }
        impl CheckpointVerifier for DurableCheckpoint {
            fn create_and_verify(&mut self) -> Result<CheckpointEvidence, GateError> {
                assert_eq!(persisted_status(&self.path, &self.id).phase, OperationPhase::Capturing);
                Ok(evidence())
            }
        }
        struct DurableRevalidator {
            path: PathBuf,
            id: OperationId,
        }
        impl PlanRevalidator for DurableRevalidator {
            fn still_matches(&mut self, _: &PlannedChange) -> Result<bool, GateError> {
                assert_eq!(persisted_status(&self.path, &self.id).phase, OperationPhase::Capturing);
                Ok(true)
            }
        }
        let mut checkpoint = DurableCheckpoint { path: path.clone(), id: id.clone() };
        let mut revalidator = DurableRevalidator { path: path.clone(), id: id.clone() };
        let apply_path = path.clone();
        let apply_id = id.clone();
        let verify_path = path.clone();
        let verify_id = id.clone();
        let result = with_journaled_verified_mutation(
            &lock.lock,
            &planned(),
            &mut operation,
            &mut checkpoint,
            &mut revalidator,
            move |_| {
                let status = persisted_status(&apply_path, &apply_id);
                assert_eq!(status.phase, OperationPhase::Applying);
                assert_eq!(status.error, None);
                Ok("applied")
            },
            move |result, _| {
                assert_eq!(*result, "applied");
                let status = persisted_status(&verify_path, &verify_id);
                assert_eq!(status.phase, OperationPhase::Verifying);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result, "applied");
        let status = persisted_status(&path, &id);
        assert_eq!(status.phase, OperationPhase::Succeeded);
        assert_eq!(status.error, None);
        assert_eq!(
            persisted_phases(&path, &id),
            [
                OperationPhase::Accepted,
                OperationPhase::Capturing,
                OperationPhase::Capturing,
                OperationPhase::Applying,
                OperationPhase::Verifying,
                OperationPhase::Verifying,
                OperationPhase::Succeeded,
            ]
        );
        drop(operation);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journaled_failed_checkpoint_and_revalidation_never_apply() {
        for (tag, checkpoint_result, plan_matches) in [
            ("capture", Err(GateError::CheckpointUnverified), true),
            ("revalidate", Ok(evidence()), false),
        ] {
            let (path, mut journal) = operation_journal(tag);
            let id = operation_id(if tag == "capture" { "op-capture" } else { "op-stale" });
            let mut operation = JournaledOperation::new(&mut journal, id.clone(), action());
            let lock = mutation_lock(tag);
            let trace = Arc::new(Mutex::new(Vec::new()));
            let mut checkpoint =
                FakeCheckpoint { evidence: checkpoint_result, trace: trace.clone() };
            let mut revalidator = FakeRevalidator { matches: plan_matches, trace };
            let result = with_journaled_verified_mutation(
                &lock.lock,
                &planned(),
                &mut operation,
                &mut checkpoint,
                &mut revalidator,
                |_| -> Result<(), GateError> {
                    panic!("apply must not follow failed pre-change gates")
                },
                |_: &(), _| Ok(()),
            );
            let expected = if tag == "capture" {
                (GateError::CheckpointUnverified, ServiceError::RecoveryUnavailable)
            } else {
                (GateError::PlanChanged, ServiceError::StaleRequest)
            };
            assert_eq!(result, Err(expected.0));
            let status = persisted_status(&path, &id);
            assert_eq!(status.phase, OperationPhase::Failed);
            assert_eq!(status.error, Some(expected.1));
            drop(operation);
            drop(journal);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn journaled_post_change_failure_is_durable_and_distinct() {
        let (path, mut journal) = operation_journal("post-change");
        let id = operation_id("op-post-change");
        let mut operation = JournaledOperation::new(&mut journal, id.clone(), action());
        let lock = mutation_lock("post-change");
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint = FakeCheckpoint { evidence: Ok(evidence()), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace };
        let result = with_journaled_verified_mutation(
            &lock.lock,
            &planned(),
            &mut operation,
            &mut checkpoint,
            &mut revalidator,
            |_| Ok(()),
            |_, _| Err(GateError::PostChangeFailed),
        );
        assert_eq!(result, Err(GateError::PostChangeFailed));
        let status = persisted_status(&path, &id);
        assert_eq!(status.phase, OperationPhase::Failed);
        assert_eq!(status.error, Some(ServiceError::Failed));
        drop(operation);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journaled_apply_failure_is_a_terminal_failed_record() {
        let (path, mut journal) = operation_journal("apply-failure");
        let id = operation_id("op-apply-failure");
        let mut operation = JournaledOperation::new(&mut journal, id.clone(), action());
        let lock = mutation_lock("apply-failure");
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint = FakeCheckpoint { evidence: Ok(evidence()), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace };
        let result = with_journaled_verified_mutation(
            &lock.lock,
            &planned(),
            &mut operation,
            &mut checkpoint,
            &mut revalidator,
            |_| Err(GateError::ApplyFailed),
            |_: &(), _| Ok(()),
        );
        assert_eq!(result, Err(GateError::ApplyFailed));
        let status = persisted_status(&path, &id);
        assert_eq!(status.phase, OperationPhase::Failed);
        assert_eq!(status.error, Some(ServiceError::Failed));
        drop(operation);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn independently_opened_coordinator_locks_serialize_writers() {
        use std::sync::mpsc;
        use std::time::Duration;
        let first = mutation_lock("serialize");
        let uid = rustix::process::geteuid().as_raw();
        let second = MutationLock::open(first.lock.directory.try_clone().unwrap(), uid).unwrap();
        let guard = first.lock.acquire().unwrap();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _second_guard = second.acquire().unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(75)).is_err());
        drop(guard);
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn coordinator_lock_rejects_nonprivate_state_directories() {
        let path =
            std::env::temp_dir().join(format!("ryoku-mutation-lock-public-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result =
            MutationLock::open(File::open(&path).unwrap(), rustix::process::geteuid().as_raw());
        assert!(matches!(result, Err(GateError::Lock(io::ErrorKind::PermissionDenied))));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir(path).unwrap();
    }

    #[test]
    fn coordinator_lock_rejects_aliased_lock_inode() {
        let owner = mutation_lock("aliased");
        std::fs::hard_link(owner.path.join(MUTATION_LOCK_NAME), owner.path.join("alias")).unwrap();
        assert!(matches!(
            owner.lock.acquire(),
            Err(GateError::Lock(io::ErrorKind::PermissionDenied))
        ));
        std::fs::remove_file(owner.path.join("alias")).unwrap();
    }

    #[test]
    fn lock_holder_process_exit_releases_the_coordinator_lock() {
        const DIRECTORY_ENV: &str = "RYOKU_TEST_MUTATION_LOCK_DIRECTORY";
        if let Some(path) = std::env::var_os(DIRECTORY_ENV) {
            let lock =
                MutationLock::open(File::open(path).unwrap(), rustix::process::geteuid().as_raw())
                    .unwrap();
            let _guard = lock.acquire().unwrap();
            // Exit without Rust destructors to model an interrupted process.
            std::process::exit(73);
        }

        let owner = mutation_lock("process-exit");
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("mutation_gate::tests::lock_holder_process_exit_releases_the_coordinator_lock")
            .env(DIRECTORY_ENV, &owner.path)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        let _guard = owner.lock.acquire().unwrap();
    }

    #[test]
    fn lock_holder_process_contention_child() {
        let Some(directory) = std::env::var_os("RYOKU_TEST_MUTATION_LOCK_DIRECTORY") else {
            return;
        };
        let ready = PathBuf::from(std::env::var_os("RYOKU_TEST_MUTATION_LOCK_READY").unwrap());
        let release = PathBuf::from(std::env::var_os("RYOKU_TEST_MUTATION_LOCK_RELEASE").unwrap());
        let lock =
            MutationLock::open(File::open(directory).unwrap(), rustix::process::geteuid().as_raw())
                .unwrap();
        let _guard = lock.acquire().unwrap();
        std::fs::write(ready, b"locked").unwrap();
        while !release.exists() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::process::exit(74);
    }

    #[test]
    fn independent_processes_contend_on_the_coordinator_lock() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let owner = mutation_lock("process-contention");
        let ready = owner.path.join("child.ready");
        let release = owner.path.join("child.release");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("mutation_gate::tests::lock_holder_process_contention_child")
            .env("RYOKU_TEST_MUTATION_LOCK_DIRECTORY", &owner.path)
            .env("RYOKU_TEST_MUTATION_LOCK_READY", &ready)
            .env("RYOKU_TEST_MUTATION_LOCK_RELEASE", &release)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "lock holder child did not acquire lock");
            assert!(child.try_wait().unwrap().is_none(), "lock holder child exited early");
            std::thread::sleep(Duration::from_millis(5));
        }

        let second = MutationLock::open(
            owner.lock.directory.try_clone().unwrap(),
            rustix::process::geteuid().as_raw(),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _guard = second.acquire().unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(75)).is_err());
        std::fs::write(&release, b"release").unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(74));
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        waiter.join().unwrap();
        std::fs::remove_file(ready).unwrap();
        std::fs::remove_file(release).unwrap();
    }
}
