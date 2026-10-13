//! Fail-closed sequencing contract for supported mutations.
//!
//! This module does not capture recovery state or execute provider operations.
//! Integrations must provide a trusted checkpoint verifier and plan revalidator.
//! The lock must be an open file in a service-owned, non-user-writable directory;
//! all cooperating writers must use the same inode. `flock` coordinates processes
//! on one host and is advisory, so it cannot constrain uncooperative providers.

use std::fs::File;
use std::io;

use rustix::fs::{FlockOperation, flock};

use crate::{
    JournalError, ManagedActionKind, OperationId, OperationJournal, OperationPhase,
    OperationStatus, ServiceError,
};

/// Identity pair for the system snapshot and Ryoku/configuration generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedGeneration {
    pub system_snapshot: String,
    pub ryoku_generation: String,
}

/// Evidence reported by the recovery integration after it has captured and
/// read-back verified every component required by the operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointEvidence {
    pub linked: LinkedGeneration,
    pub system_component_verified: bool,
    pub ryoku_component_verified: bool,
    pub contents_and_metadata_verified: bool,
}

/// Typed proof accepted by the mutation gate. Fields are private so callers
/// cannot construct a proof without passing the validation constructor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedLinkedGeneration(LinkedGeneration);

impl VerifiedLinkedGeneration {
    pub fn validate(evidence: CheckpointEvidence) -> Result<Self, GateError> {
        if evidence.linked.system_snapshot.trim().is_empty()
            || evidence.linked.ryoku_generation.trim().is_empty()
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

/// A request fingerprint that must still match after checkpoint verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedChange {
    pub operation_key: String,
    pub plan_fingerprint: String,
    pub affected_state_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateError {
    Lock(io::ErrorKind),
    Journal(JournalError),
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
}

impl<'a> JournaledOperation<'a> {
    pub fn new(
        journal: &'a mut OperationJournal,
        operation_id: OperationId,
        action: ManagedActionKind,
    ) -> Self {
        Self { journal, operation_id, action }
    }

    fn record(
        &mut self,
        phase: OperationPhase,
        error: Option<ServiceError>,
    ) -> Result<(), GateError> {
        self.journal
            .record(OperationStatus {
                operation_id: self.operation_id.clone(),
                action: self.action,
                phase,
                progress: None,
                error,
            })
            .map_err(GateError::Journal)
    }

    fn fail(&mut self, category: ServiceError, cause: GateError) -> GateError {
        match self.record(OperationPhase::Failed, Some(category)) {
            Ok(()) => cause,
            Err(journal_error) => journal_error,
        }
    }
}

/// Persist pre-change failures using only error codes already defined by
/// protocol v1. This keeps old clients able to deserialize operation status.
fn pre_change_error(cause: &GateError) -> ServiceError {
    match cause {
        GateError::Lock(_) => ServiceError::Busy,
        GateError::PlanChanged => ServiceError::StaleRequest,
        _ => ServiceError::RecoveryUnavailable,
    }
}

/// Holds an exclusive interprocess writer lock until dropped.
pub struct WriterGuard {
    _file: File,
}

impl WriterGuard {
    /// Blocks until the advisory exclusive lock is acquired.
    pub fn acquire(service_owned_lock_file: File) -> Result<Self, GateError> {
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

/// Sequence a mutation under a cross-process writer lock. `apply` is called
/// only after acquiring the lock, obtaining a verified linked generation, and
/// revalidating the plan. The callback is deliberately generic: this module
/// does not choose or execute a provider operation.
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

/// Execute one mutation through durably recorded orchestration phases. The
/// caller must supply trusted checkpoint, revalidation, apply, and post-change
/// verification integrations. Each phase is synchronized before the next
/// callback begins. A crash leaves the last reached phase for explicit
/// recovery; this function does not infer or replay an interrupted operation.
pub fn with_journaled_verified_mutation<T>(
    lock_file: File,
    planned: &PlannedChange,
    operation: &mut JournaledOperation<'_>,
    checkpoint: &mut impl CheckpointVerifier,
    revalidator: &mut impl PlanRevalidator,
    apply: impl FnOnce(&VerifiedLinkedGeneration) -> Result<T, GateError>,
    verify_post_change: impl FnOnce(&T, &VerifiedLinkedGeneration) -> Result<(), GateError>,
) -> Result<T, GateError> {
    operation.record(OperationPhase::Accepted, None)?;
    let _writer = match WriterGuard::acquire(lock_file) {
        Ok(guard) => guard,
        Err(error) => {
            let category = pre_change_error(&error);
            return Err(operation.fail(category, error));
        }
    };

    operation.record(OperationPhase::Capturing, None)?;
    let evidence = match checkpoint.create_and_verify() {
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
    let still_matches = match revalidator.still_matches(planned) {
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

    // If this durable write fails, the mutation callback is never entered.
    operation.record(OperationPhase::Applying, None)?;
    let result = match apply(&proof) {
        Ok(result) => result,
        Err(error) => return Err(operation.fail(ServiceError::Failed, error)),
    };

    operation.record(OperationPhase::Verifying, None)?;
    if let Err(error) = verify_post_change(&result, &proof) {
        // `failed` is an existing protocol-v1 code; unlike pre-change
        // checkpoint failures (`recovery_unavailable`), it marks failure
        // after the mutation entered the durable Verifying phase.
        return Err(operation.fail(ServiceError::Failed, error));
    }
    operation.record(OperationPhase::Succeeded, None)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
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

    fn evidence() -> CheckpointEvidence {
        CheckpointEvidence {
            linked: LinkedGeneration {
                system_snapshot: "snap-1".into(),
                ryoku_generation: "generation-1".into(),
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
            plan_fingerprint: "plan-a".into(),
            affected_state_fingerprint: "scope-a".into(),
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
                assert_eq!(proof.linked().system_snapshot, "snap-1");
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
        let mut checkpoint =
            FakeCheckpoint { evidence: Ok(evidence()), trace: Arc::new(Mutex::new(Vec::new())) };
        let mut revalidator =
            FakeRevalidator { matches: true, trace: Arc::new(Mutex::new(Vec::new())) };
        let apply_path = path.clone();
        let apply_id = id.clone();
        let verify_path = path.clone();
        let verify_id = id.clone();
        let result = with_journaled_verified_mutation(
            lock_file("durable-success"),
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
            let trace = Arc::new(Mutex::new(Vec::new()));
            let mut checkpoint =
                FakeCheckpoint { evidence: checkpoint_result, trace: trace.clone() };
            let mut revalidator = FakeRevalidator { matches: plan_matches, trace };
            let result = with_journaled_verified_mutation(
                lock_file(tag),
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
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint = FakeCheckpoint { evidence: Ok(evidence()), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace };
        let result = with_journaled_verified_mutation(
            lock_file("post-change"),
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
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut checkpoint = FakeCheckpoint { evidence: Ok(evidence()), trace: trace.clone() };
        let mut revalidator = FakeRevalidator { matches: true, trace };
        let result = with_journaled_verified_mutation(
            lock_file("apply-failure"),
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
    fn separate_descriptors_for_same_inode_serialize_writers() {
        use std::sync::mpsc;
        use std::time::Duration;
        let path =
            std::env::temp_dir().join(format!("ryoku-mutation-serialize-{}", std::process::id()));
        let first = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let second = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let guard = WriterGuard::acquire(first).unwrap();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _second_guard = WriterGuard::acquire(second).unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(75)).is_err());
        drop(guard);
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
