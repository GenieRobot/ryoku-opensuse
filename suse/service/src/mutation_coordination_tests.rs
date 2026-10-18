//! Instrumented synthetic integration: proves ordering/history, not native capture.
use super::*;
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const STAGES: [MutationStage; 6] = [
    MutationStage::Preflight,
    MutationStage::Capturing,
    MutationStage::Revalidating,
    MutationStage::Applying,
    MutationStage::HealthChecking,
    MutationStage::PostChangeCapture,
];
static NEXT: AtomicU64 = AtomicU64::new(0);

fn plan() -> PlannedChange {
    PlannedChange {
        operation_key: "install:synthetic".into(),
        plan_fingerprint: "11".repeat(32),
        affected_state_fingerprint: "22".repeat(32),
    }
}
fn linked() -> LinkedGeneration {
    LinkedGeneration {
        transaction_id: Identifier::new("txn-synthetic").unwrap(),
        generation_id: Identifier::new("generation-synthetic").unwrap(),
        manifest_sha256: Sha256::new("aa".repeat(32)).unwrap(),
        snapshot: SnapshotLink::new(
            Identifier::new("root").unwrap(),
            "42",
            "11111111-1111-1111-1111-111111111111",
        )
        .unwrap(),
    }
}
fn post_linked() -> LinkedGeneration {
    let mut post = linked();
    post.transaction_id = Identifier::new("txn-post").unwrap();
    post.generation_id = Identifier::new("generation-post").unwrap();
    post.manifest_sha256 = Sha256::new("bb".repeat(32)).unwrap();
    post.snapshot.snapper_snapshot_id = "43".into();
    post
}
fn verified_post(before: &VerifiedLinkedGeneration) -> VerifiedPostChangeGeneration {
    VerifiedPostChangeGeneration::validate(
        CheckpointEvidence {
            linked: post_linked(),
            system_component_verified: true,
            ryoku_component_verified: true,
            contents_and_metadata_verified: true,
        },
        before,
    )
    .unwrap()
}
fn id() -> OperationId {
    OperationId::new("synthetic-operation").unwrap()
}
fn journal(path: &Path) -> OperationJournal {
    OperationJournal::open(OpenOptions::new().read(true).append(true).open(path).unwrap()).unwrap()
}

struct Scenario {
    directory: PathBuf,
    path: PathBuf,
    lock: MutationLock,
}
impl Scenario {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "ryoku-coordination-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("operations.log");
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        Self::open(directory)
    }
    fn open(directory: PathBuf) -> Self {
        let lock = MutationLock::open(
            File::open(&directory).unwrap(),
            rustix::process::geteuid().as_raw(),
        )
        .unwrap();
        Self { path: directory.join("operations.log"), directory, lock }
    }
    fn integration(&self) -> Instrumented {
        Instrumented {
            path: self.path.clone(),
            operation_id: id(),
            stages: Vec::new(),
            fail: None,
            cancel_after: None,
            cancelled: false,
            crash: None,
            capture_block: None,
            post_override: None,
            post_preimage_override: None,
        }
    }
    fn run(&self, integration: &mut Instrumented) -> Result<(), GateError> {
        let mut journal = journal(&self.path);
        let mut operation =
            JournaledOperation::new(&mut journal, id(), ManagedActionKind::InstallPackage);
        coordinate_mutation(&self.lock, &plan(), &mut operation, integration)
    }
}
impl Drop for Scenario {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct Instrumented {
    path: PathBuf,
    operation_id: OperationId,
    stages: Vec<MutationStage>,
    fail: Option<MutationStage>,
    cancel_after: Option<MutationStage>,
    cancelled: bool,
    crash: Option<MutationStage>,
    capture_block: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
    post_override: Option<CheckpointEvidence>,
    post_preimage_override: Option<LinkedGeneration>,
}
impl Instrumented {
    fn enter(&mut self, stage: MutationStage) {
        // Independently replay disk before each callback's effects. Applying
        // observes the full checkpoint binding, not an in-memory callback value.
        let persisted = journal(&self.path);
        let context = persisted.mutation_context(&self.operation_id).unwrap();
        assert_eq!(context.plan, plan());
        assert_eq!(context.stage, stage);
        assert_eq!(context.outcome, None);
        assert_eq!(context.post_checkpoint, None);
        assert_eq!(persisted.get(&self.operation_id).unwrap().phase, stage.phase());
        assert_eq!(
            context.checkpoint,
            if matches!(stage, MutationStage::Preflight | MutationStage::Capturing) {
                None
            } else {
                Some(linked())
            }
        );
        self.stages.push(stage);
    }
    fn finish(&mut self, stage: MutationStage, error: GateError) -> Result<(), GateError> {
        if self.crash == Some(stage) {
            std::process::exit(73);
        }
        if self.cancel_after == Some(stage) {
            self.cancelled = true;
        }
        if self.fail == Some(stage) { Err(error) } else { Ok(()) }
    }
}
impl MutationIntegration for Instrumented {
    type Output = ();
    fn preflight(
        &mut self,
        _: &PlannedChange,
        _: &mut dyn FnMut(crate::Progress) -> Result<(), GateError>,
    ) -> Result<(), GateError> {
        self.enter(MutationStage::Preflight);
        self.finish(MutationStage::Preflight, GateError::InsufficientSpace)
    }
    fn capture(
        &mut self,
        _: &PlannedChange,
        report_progress: &mut dyn FnMut(crate::Progress) -> Result<(), GateError>,
    ) -> Result<CheckpointEvidence, GateError> {
        self.enter(MutationStage::Capturing);
        report_progress(crate::Progress { completed: 0, total: Some(2) })?;
        report_progress(crate::Progress { completed: 1, total: Some(2) })?;
        if let Some((ready, release)) = self.capture_block.take() {
            ready.send(()).unwrap();
            release.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        }
        self.finish(MutationStage::Capturing, GateError::CheckpointUnverified)?;
        Ok(CheckpointEvidence {
            linked: linked(),
            system_component_verified: true,
            ryoku_component_verified: true,
            contents_and_metadata_verified: true,
        })
    }
    fn revalidate(
        &mut self,
        _: &PlannedChange,
        _: &VerifiedLinkedGeneration,
        _: &mut dyn FnMut(crate::Progress) -> Result<(), GateError>,
    ) -> Result<bool, GateError> {
        self.enter(MutationStage::Revalidating);
        self.finish(MutationStage::Revalidating, GateError::PlanChanged)?;
        Ok(true)
    }
    fn apply(
        &mut self,
        proof: &VerifiedLinkedGeneration,
        report_progress: &mut dyn FnMut(crate::Progress) -> Result<(), GateError>,
    ) -> Result<(), GateError> {
        self.enter(MutationStage::Applying);
        assert_eq!(proof.linked(), &linked());
        report_progress(crate::Progress { completed: 1, total: Some(3) })?;
        report_progress(crate::Progress { completed: 2, total: Some(3) })?;
        fs::write(self.path.with_file_name("provider-state"), b"changed").unwrap();
        self.finish(MutationStage::Applying, GateError::ApplyFailed)
    }
    fn health_check(
        &mut self,
        _: &(),
        _: &VerifiedLinkedGeneration,
        _: &mut dyn FnMut(crate::Progress) -> Result<(), GateError>,
    ) -> Result<(), GateError> {
        self.enter(MutationStage::HealthChecking);
        self.finish(MutationStage::HealthChecking, GateError::ApplyFailed)
    }
    fn capture_post_change(
        &mut self,
        _: &(),
        before: &VerifiedLinkedGeneration,
        _: &mut dyn FnMut(crate::Progress) -> Result<(), GateError>,
    ) -> Result<VerifiedPostChangeGeneration, GateError> {
        self.enter(MutationStage::PostChangeCapture);
        self.finish(MutationStage::PostChangeCapture, GateError::PostChangeFailed)?;
        let other_before;
        let before = if let Some(linked) = self.post_preimage_override.take() {
            other_before = VerifiedLinkedGeneration::validate(CheckpointEvidence {
                linked,
                system_component_verified: true,
                ryoku_component_verified: true,
                contents_and_metadata_verified: true,
            })
            .unwrap();
            &other_before
        } else {
            before
        };
        if let Some(evidence) = self.post_override.take() {
            VerifiedPostChangeGeneration::validate(evidence, before)
        } else {
            Ok(verified_post(before))
        }
    }
    fn cancellation_requested(&self) -> bool {
        self.cancelled
    }
}

#[test]
fn exact_plan_and_link_are_durable_before_first_mutation() {
    let scenario = Scenario::new();
    let mut integration = scenario.integration();
    scenario.run(&mut integration).unwrap();
    assert_eq!(integration.stages, STAGES);
    let progress: Vec<_> = std::fs::read_to_string(&scenario.path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter_map(|record| record.get("status")?.get("progress")?.get("completed")?.as_u64())
        .collect();
    assert_eq!(progress, [0, 1, 1, 2]);
    let reopened = journal(&scenario.path);
    let context = reopened.mutation_context(&id()).unwrap();
    assert_eq!(context.plan, plan());
    assert_eq!(context.checkpoint, Some(linked()));
    assert_eq!(context.post_checkpoint, Some(post_linked()));
    assert_eq!(context.stage, MutationStage::Complete);
    assert!(reopened.recovery_required().is_empty());
}

#[test]
fn post_capture_requires_fresh_verified_reference_bound_to_this_preimage() {
    let valid = CheckpointEvidence {
        linked: post_linked(),
        system_component_verified: true,
        ryoku_component_verified: true,
        contents_and_metadata_verified: true,
    };
    let before = linked();
    let mut reused_transaction = valid.clone();
    reused_transaction.linked.transaction_id = before.transaction_id.clone();
    let mut reused_generation = valid.clone();
    reused_generation.linked.generation_id = before.generation_id.clone();
    let mut reused_digest = valid.clone();
    reused_digest.linked.manifest_sha256 = before.manifest_sha256.clone();
    let mut reused_snapshot = valid.clone();
    reused_snapshot.linked.snapshot = before.snapshot.clone();
    for evidence in [
        CheckpointEvidence { linked: before, ..valid.clone() },
        CheckpointEvidence { system_component_verified: false, ..valid.clone() },
        CheckpointEvidence { ryoku_component_verified: false, ..valid.clone() },
        CheckpointEvidence { contents_and_metadata_verified: false, ..valid.clone() },
        reused_transaction,
        reused_generation,
        reused_digest,
        reused_snapshot,
    ] {
        let scenario = Scenario::new();
        let mut integration = scenario.integration();
        integration.post_override = Some(evidence);
        assert_eq!(scenario.run(&mut integration), Err(GateError::PostChangeFailed));
        let persisted = journal(&scenario.path);
        let context = persisted.mutation_context(&id()).unwrap();
        assert_eq!(context.stage, MutationStage::PostChangeCapture);
        assert_eq!(context.outcome, Some(MutationOutcome::PostChangeCaptureFailed));
        assert_eq!(context.checkpoint, Some(linked()));
        assert_eq!(context.post_checkpoint, None);
        assert_eq!(persisted.recovery_required().len(), 1);
    }
    let scenario = Scenario::new();
    let mut integration = scenario.integration();
    let mut wrong_before = linked();
    wrong_before.snapshot.snapper_snapshot_id = "100".into();
    integration.post_preimage_override = Some(wrong_before);
    assert_eq!(scenario.run(&mut integration), Err(GateError::PostChangeFailed));
    assert_eq!(journal(&scenario.path).mutation_context(&id()).unwrap().post_checkpoint, None);
}

#[test]
fn failures_retain_distinct_prechange_unknown_and_postcapture_outcomes() {
    for stage in STAGES {
        let scenario = Scenario::new();
        let mut integration = scenario.integration();
        integration.fail = Some(stage);
        let result = scenario.run(&mut integration);
        if stage == MutationStage::Preflight {
            assert_eq!(result, Err(GateError::InsufficientSpace));
        } else {
            assert!(result.is_err());
        }
        let reopened = journal(&scenario.path);
        let context = reopened.mutation_context(&id()).unwrap();
        assert_eq!(context.stage, stage);
        let outcome = if stage.before_change() {
            MutationOutcome::FailedBeforeChange
        } else if stage == MutationStage::PostChangeCapture {
            MutationOutcome::PostChangeCaptureFailed
        } else {
            MutationOutcome::MayHaveMutated
        };
        assert_eq!(context.outcome, Some(outcome));
        assert_eq!(scenario.directory.join("provider-state").exists(), !stage.before_change());
        assert_eq!(reopened.recovery_required().is_empty(), stage.before_change());
    }
}

#[test]
fn invalid_plans_and_reused_operation_ids_never_enter_integration_hooks() {
    let scenario = Scenario::new();
    let mut journal = journal(&scenario.path);
    let mut invalid = plan();
    invalid.affected_state_fingerprint = "unbounded-unverified-plan".into();
    let mut operation =
        JournaledOperation::new(&mut journal, id(), ManagedActionKind::InstallPackage);
    let mut integration = scenario.integration();
    assert_eq!(
        coordinate_mutation(&scenario.lock, &invalid, &mut operation, &mut integration),
        Err(GateError::InvalidPlan)
    );
    assert!(integration.stages.is_empty());
    assert!(fs::read(&scenario.path).unwrap().is_empty());
    scenario.run(&mut scenario.integration()).unwrap();
    let original = fs::read(&scenario.path).unwrap();
    assert_eq!(
        scenario.run(&mut integration),
        Err(GateError::Journal(JournalError::InvalidTransition))
    );
    assert!(integration.stages.is_empty());
    assert_eq!(fs::read(&scenario.path).unwrap(), original);
}

#[test]
fn cancellation_is_honored_only_before_first_mutation() {
    for cancel_after in [
        None,
        Some(MutationStage::Preflight),
        Some(MutationStage::Capturing),
        Some(MutationStage::Revalidating),
    ] {
        let scenario = Scenario::new();
        let mut integration = scenario.integration();
        integration.cancel_after = cancel_after;
        integration.cancelled = cancel_after.is_none();
        assert_eq!(scenario.run(&mut integration), Err(GateError::CancelledBeforeChange));
        let reopened = journal(&scenario.path);
        assert_eq!(
            reopened.mutation_context(&id()).unwrap().outcome,
            Some(MutationOutcome::CancelledBeforeChange)
        );
        assert!(reopened.recovery_required().is_empty());
        assert!(!scenario.directory.join("provider-state").exists());
    }
    for cancel_after in STAGES.into_iter().filter(|stage| !stage.before_change()) {
        let scenario = Scenario::new();
        let mut integration = scenario.integration();
        integration.cancel_after = Some(cancel_after);
        scenario.run(&mut integration).unwrap();
        assert_eq!(integration.stages, STAGES);
    }
}

#[test]
fn process_death_preserves_each_phase_and_restart_refuses_reapply() {
    const DIRECTORY: &str = "RYOKU_TEST_COORDINATOR_CRASH_DIRECTORY";
    const STAGE: &str = "RYOKU_TEST_COORDINATOR_CRASH_STAGE";
    if let Some(directory) = std::env::var_os(DIRECTORY) {
        let scenario = Scenario::open(directory.into());
        let mut integration = scenario.integration();
        integration.crash = Some(serde_json::from_str(&std::env::var(STAGE).unwrap()).unwrap());
        let _ = scenario.run(&mut integration);
        panic!("requested crash stage was not reached");
    }
    for stage in STAGES {
        let scenario = Scenario::new();
        // This handle intentionally predates the child; refresh under the
        // common lock must observe the child's durable state before accepting.
        let mut stale = journal(&scenario.path);
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact").arg("mutation_gate::coordination_tests::process_death_preserves_each_phase_and_restart_refuses_reapply")
            .env(DIRECTORY, &scenario.directory).env(STAGE, serde_json::to_string(&stage).unwrap())
            .status().unwrap();
        assert_eq!(child.code(), Some(73));
        let reopened = journal(&scenario.path);
        assert_eq!(reopened.mutation_context(&id()).unwrap().stage, stage);
        assert_eq!(reopened.recovery_required().len(), 1);
        let bytes = fs::read(&scenario.path).unwrap();
        let mut operation = JournaledOperation::new(
            &mut stale,
            OperationId::new("fresh-request").unwrap(),
            ManagedActionKind::InstallPackage,
        );
        let mut integration = scenario.integration();
        assert_eq!(
            coordinate_mutation(&scenario.lock, &plan(), &mut operation, &mut integration),
            Err(GateError::RecoveryRequired)
        );
        assert!(integration.stages.is_empty());
        assert_eq!(fs::read(&scenario.path).unwrap(), bytes);
        assert_eq!(scenario.directory.join("provider-state").exists(), !stage.before_change());
    }
}

#[test]
fn common_lock_serializes_whole_operations_using_independent_stale_journals() {
    use std::{sync::mpsc, time::Duration};
    let scenario = Scenario::new();
    let mut first_journal = journal(&scenario.path);
    let mut second_journal = journal(&scenario.path);
    let mut first = scenario.integration();
    let mut second = scenario.integration();
    first.operation_id = OperationId::new("operation-first").unwrap();
    second.operation_id = OperationId::new("operation-second").unwrap();
    let second_lock = MutationLock::open(
        File::open(&scenario.directory).unwrap(),
        rustix::process::geteuid().as_raw(),
    )
    .unwrap();
    let (ready, entered_capture) = mpsc::channel();
    let (release, may_finish) = mpsc::channel();
    first.capture_block = Some((ready, may_finish));
    let (finished, second_finished) = mpsc::channel();
    let (started, second_started) = mpsc::channel();
    std::thread::scope(|scope| {
        let first_writer = scope.spawn(|| {
            let mut operation = JournaledOperation::new(
                &mut first_journal,
                first.operation_id.clone(),
                ManagedActionKind::InstallPackage,
            );
            coordinate_mutation(&scenario.lock, &plan(), &mut operation, &mut first).unwrap();
        });
        entered_capture.recv_timeout(Duration::from_secs(3)).unwrap();
        let second_writer = scope.spawn(move || {
            let mut operation = JournaledOperation::new(
                &mut second_journal,
                second.operation_id.clone(),
                ManagedActionKind::InstallPackage,
            );
            started.send(()).unwrap();
            coordinate_mutation(&second_lock, &plan(), &mut operation, &mut second).unwrap();
            finished.send(()).unwrap();
        });
        second_started.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(second_finished.recv_timeout(Duration::from_millis(75)).is_err());
        release.send(()).unwrap();
        first_writer.join().unwrap();
        second_finished.recv_timeout(Duration::from_secs(3)).unwrap();
        second_writer.join().unwrap();
    });
    let records: Vec<crate::JournalRecord> = fs::read_to_string(&scenario.path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 22);
    assert!(
        records[..11].iter().all(|record| record.status.operation_id.as_str() == "operation-first")
    );
    assert!(
        records[11..]
            .iter()
            .all(|record| record.status.operation_id.as_str() == "operation-second")
    );
    assert!(journal(&scenario.path).recovery_required().is_empty());
}

#[test]
fn legacy_interruption_has_no_invented_checkpoint_and_blocks_new_work() {
    let scenario = Scenario::new();
    let mut old = journal(&scenario.path);
    for phase in [
        OperationPhase::Accepted,
        OperationPhase::Capturing,
        OperationPhase::Applying,
        OperationPhase::Failed,
    ] {
        old.record(OperationStatus {
            operation_id: id(),
            action: ManagedActionKind::InstallPackage,
            phase,
            progress: None,
            error: if phase == OperationPhase::Failed { Some(ServiceError::Failed) } else { None },
        })
        .unwrap();
    }
    let reopened = journal(&scenario.path);
    assert!(reopened.mutation_context(&id()).is_none());
    assert_eq!(reopened.recovery_required().len(), 1);
    let bytes = fs::read(&scenario.path).unwrap();
    let mut integration = scenario.integration();
    assert_eq!(scenario.run(&mut integration), Err(GateError::RecoveryRequired));
    assert!(integration.stages.is_empty());
    assert_eq!(fs::read(&scenario.path).unwrap(), bytes);
}

#[test]
fn journal_replay_rejects_changed_bindings_actions_missing_proof_and_bad_shapes() {
    use serde_json::{Value, json};
    let scenario = Scenario::new();
    scenario.run(&mut scenario.integration()).unwrap();
    let original: Vec<Value> = fs::read_to_string(&scenario.path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let applying =
        original.iter().position(|record| record["mutation"]["stage"] == "applying").unwrap();
    for (pointer, value) in [
        ("/mutation/plan/operation_key", json!("install:different")),
        ("/mutation/plan/plan_fingerprint", json!("bb".repeat(32))),
        ("/mutation/plan/affected_state_fingerprint", json!("cc".repeat(32))),
        ("/mutation/checkpoint/transaction_id", json!("different-transaction")),
        ("/mutation/checkpoint/generation_id", json!("different-generation")),
        ("/mutation/checkpoint/manifest_sha256", json!("dd".repeat(32))),
        ("/mutation/checkpoint/snapshot/snapper_config", json!("different-config")),
        ("/mutation/checkpoint/snapshot/snapper_snapshot_id", json!("43")),
        (
            "/mutation/checkpoint/snapshot/filesystem_uuid",
            json!("22222222-2222-2222-2222-222222222222"),
        ),
        ("/mutation/checkpoint", Value::Null),
        ("/mutation", Value::Null),
        ("/status/action", json!("remove_package")),
        ("/mutation/stage", json!("complete")),
    ] {
        let mut records = original.clone();
        *records[applying].pointer_mut(pointer).unwrap() = value;
        let bytes = records.iter().map(|record| format!("{record}\n")).collect::<String>();
        fs::write(&scenario.path, &bytes).unwrap();
        assert!(
            OperationJournal::open(
                OpenOptions::new().read(true).append(true).open(&scenario.path).unwrap()
            )
            .is_err(),
            "accepted changed {pointer}"
        );
        assert_eq!(fs::read(&scenario.path).unwrap(), bytes.as_bytes());
    }
    for (pointer, value) in [
        ("/mutation/plan/operation_key", json!("x".repeat(257))),
        ("/mutation/plan/plan_fingerprint", json!("invalid-digest")),
        ("/mutation/checkpoint", serde_json::to_value(linked()).unwrap()),
    ] {
        let mut records = original.clone();
        *records[0].pointer_mut(pointer).unwrap() = value;
        let bytes = records.iter().map(|record| format!("{record}\n")).collect::<String>();
        fs::write(&scenario.path, &bytes).unwrap();
        assert!(
            OperationJournal::open(
                OpenOptions::new().read(true).append(true).open(&scenario.path).unwrap()
            )
            .is_err()
        );
    }
    for value in [Value::Null, serde_json::to_value(linked()).unwrap()] {
        let mut records = original.clone();
        records.last_mut().unwrap()["mutation"]["post_checkpoint"] = value;
        let bytes = records.iter().map(|record| format!("{record}\n")).collect::<String>();
        fs::write(&scenario.path, &bytes).unwrap();
        assert!(
            OperationJournal::open(
                OpenOptions::new().read(true).append(true).open(&scenario.path).unwrap()
            )
            .is_err()
        );
    }
    let mut records = original.clone();
    let mut changed_terminal = original.last().unwrap().clone();
    changed_terminal["mutation"]["post_checkpoint"]["manifest_sha256"] = json!("ee".repeat(32));
    records.push(changed_terminal);
    fs::write(
        &scenario.path,
        records.iter().map(|record| format!("{record}\n")).collect::<String>(),
    )
    .unwrap();
    assert!(
        OperationJournal::open(
            OpenOptions::new().read(true).append(true).open(&scenario.path).unwrap()
        )
        .is_err()
    );
    let oversized = vec![b'x'; crate::MAX_JOURNAL_LINE_BYTES + 1];
    fs::write(&scenario.path, &oversized).unwrap();
    assert!(matches!(
        OperationJournal::open(
            OpenOptions::new().read(true).append(true).open(&scenario.path).unwrap()
        ),
        Err(JournalError::Corrupt)
    ));
    assert_eq!(fs::read(&scenario.path).unwrap(), oversized);
}
