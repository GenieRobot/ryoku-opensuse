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
    CheckpointUnverified,
    PlanChanged,
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

#[cfg(test)]
mod tests {
    use super::*;
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
