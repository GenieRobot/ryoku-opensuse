use super::*;
use crate::{
    configuration_apply::ConfigurationDestination,
    generation_capture::{DeclaredRoot, GenerationCaptureRequest, VerifiedSnapshotLink},
    preparation::{
        ConfigurationPreparationRequest, PreparationError, PreparationStores,
        PreparedConfigurationRestore, PreparedSystemRestore, RestoreCompatibility,
        SystemPreparationRequest,
    },
    restore::{CompatibilityStatus, RestoreJournalStore, RestorePhase, SystemSnapshotStatus},
};

fn journals(fixture: &Fixture) -> RestoreJournalStore {
    let path = fixture.path.join("journals");
    if !path.exists() {
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    RestoreJournalStore::open(&File::open(path).unwrap()).unwrap()
}
fn compatibility() -> RestoreCompatibility {
    RestoreCompatibility {
        source_snapshot: SystemSnapshotStatus::Missing,
        system: CompatibilityStatus::Unknown,
        configuration: CompatibilityStatus::Compatible,
    }
}
fn complete_linked_compatibility() -> RestoreCompatibility {
    RestoreCompatibility {
        source_snapshot: SystemSnapshotStatus::Available {
            snapper_config: Identifier::new("root").unwrap(),
            snapshot_id: "42".into(),
            filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
        },
        system: CompatibilityStatus::Compatible,
        configuration: CompatibilityStatus::Compatible,
    }
}
fn checkpoint_snapshot() -> SystemSnapshotStatus {
    SystemSnapshotStatus::Available {
        snapper_config: Identifier::new("root").unwrap(),
        snapshot_id: "43".into(),
        filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
    }
}
fn begin(
    fixture: &Fixture,
    attempt: &str,
    omit_path: bool,
    facts: RestoreCompatibility,
) -> Result<PreparedConfigurationRestore, PreparationError> {
    begin_with(fixture, attempt, omit_path, facts, |_| {})
}

fn begin_with(
    fixture: &Fixture,
    attempt: &str,
    omit_path: bool,
    facts: RestoreCompatibility,
    adjust: impl FnOnce(&mut ConfigurationPreparationRequest<'_>),
) -> Result<PreparedConfigurationRestore, PreparationError> {
    let source = fixture
        .generation
        .open_verified_transaction(&fixture.source.plan().transaction_id)
        .unwrap();
    let root = File::open(fixture.path.join("source")).unwrap();
    let mut paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    if omit_path {
        paths.pop();
    }
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let journals = journals(fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    };
    let mut request = ConfigurationPreparationRequest {
        restore_id: Identifier::new(attempt).unwrap(),
        source: &source,
        scope: RestoreScope::ConfigurationOnly,
        compatibility: facts,
        checkpoint: GenerationCaptureRequest {
            transaction_id: Identifier::new(attempt).unwrap(),
            generation_id: Identifier::new(format!("checkpoint-{attempt}")).unwrap(),
            parent_generation_id: Some(source.manifest().generation_id.clone()),
            snapshot: VerifiedSnapshotLink::from_preverified_link(
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "43",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            ),
            reason: OperationReason::ConfigurationRestore,
            created_utc: "2026-10-15T01:00:00Z".into(),
            package_revisions: vec![],
            provider_revision: None,
            profile_revision: None,
            roots: &roots,
            limits: CaptureLimits::default(),
        },
    };
    adjust(&mut request);
    stores.begin(request)
}
fn current_bytes(prepared: &PreparedConfigurationRestore) -> Vec<u8> {
    let record = prepared
        .checkpoint()
        .manifest()
        .files()
        .iter()
        .find(|entry| entry.path.as_str() == "a")
        .unwrap();
    let EntryKind::RegularFile { sha256, .. } = &record.kind else { panic!("expected file") };
    prepared.checkpoint().read_payload(sha256).unwrap()
}

fn resume(
    fixture: &Fixture,
    restore_id: &Identifier,
    snapshot: &SystemSnapshotStatus,
) -> Result<PreparedConfigurationRestore, PreparationError> {
    let root = File::open(fixture.path.join("source")).unwrap();
    let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let journal = journals(fixture);
    let stage = fixture.store();
    PreparationStores { generations: &fixture.generation, journals: &journal, stages: &stage }
        .resume(restore_id, &compatibility(), snapshot, &roots)
}

fn begin_system(fixture: &Fixture) -> Result<PreparedSystemRestore, PreparationError> {
    begin_system_with_compatibility(fixture, CompatibilityStatus::Compatible)
}

fn begin_system_with_compatibility(
    fixture: &Fixture,
    system_compatibility: CompatibilityStatus,
) -> Result<PreparedSystemRestore, PreparationError> {
    let source = fixture
        .generation
        .open_verified_transaction(&fixture.source.plan().transaction_id)
        .unwrap();
    let root = File::open(fixture.path.join("source")).unwrap();
    let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let restore_id = Identifier::new("system-attempt").unwrap();
    let journals = journals(fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    };
    stores.begin_system(SystemPreparationRequest {
        restore_id: restore_id.clone(),
        source: &source,
        compatibility: RestoreCompatibility {
            source_snapshot: SystemSnapshotStatus::Available {
                snapper_config: Identifier::new("root").unwrap(),
                snapshot_id: "42".into(),
                filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
            },
            system: system_compatibility,
            configuration: CompatibilityStatus::Unknown,
        },
        checkpoint: GenerationCaptureRequest {
            transaction_id: restore_id,
            generation_id: Identifier::new("checkpoint-system-attempt").unwrap(),
            parent_generation_id: Some(source.manifest().generation_id.clone()),
            snapshot: VerifiedSnapshotLink::from_preverified_link(
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "43",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            ),
            reason: OperationReason::Other(Identifier::new("system-restore").unwrap()),
            created_utc: "2026-10-15T01:00:00Z".into(),
            package_revisions: vec![],
            provider_revision: None,
            profile_revision: None,
            roots: &roots,
            limits: CaptureLimits::default(),
        },
    })
}

#[test]
fn incompatible_system_source_fails_before_current_state_capture() {
    let fixture = Fixture::new();
    assert!(matches!(
        begin_system_with_compatibility(&fixture, CompatibilityStatus::Incompatible),
        Err(PreparationError::Preflight(crate::restore::PreflightError::SystemIncompatible))
    ));
    assert!(
        fixture
            .generation
            .open_verified_transaction(&Identifier::new("system-attempt").unwrap())
            .is_err()
    );
    assert!(journals(&fixture).incomplete_attempts().unwrap().is_empty());
}

#[test]
fn system_only_preparation_captures_checkpoint_without_configuration_staging() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("source/a"), b"current system state").unwrap();
    let prepared = begin_system(&fixture).unwrap();
    assert_eq!(prepared.journal().plan.scope, RestoreScope::SystemOnly);
    assert_eq!(prepared.journal().phase, RestorePhase::CheckpointVerified);
    assert_eq!(current_system_checkpoint_bytes(&prepared), b"current system state");
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"current system state");

    let root = File::open(fixture.path.join("source")).unwrap();
    let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let journals = journals(&fixture);
    let stages = fixture.store();
    let reopened = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    }
    .resume_system(
        &prepared.journal().restore_id,
        &RestoreCompatibility {
            source_snapshot: SystemSnapshotStatus::Available {
                snapper_config: Identifier::new("root").unwrap(),
                snapshot_id: "42".into(),
                filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
            },
            system: CompatibilityStatus::Compatible,
            configuration: CompatibilityStatus::Unknown,
        },
        &checkpoint_snapshot(),
        &roots,
    )
    .unwrap();
    assert_eq!(reopened.journal().phase, RestorePhase::CheckpointVerified);
    assert_eq!(current_system_checkpoint_bytes(&reopened), b"current system state");
}

#[test]
fn configuration_only_executor_fails_closed_and_replays_an_interrupted_apply() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("source/a"), b"current before restore").unwrap();
    let mut prepared = begin(&fixture, "restore-execute", false, compatibility()).unwrap();
    let root = File::open(fixture.path.join("source")).unwrap();
    let destination = [ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &root }];
    let journal_store = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journal_store,
        stages: &stages,
    };

    assert!(matches!(
        stores.execute_configuration_only(
            &mut prepared,
            &destination,
            || Err::<(), String>("writer service unavailable".into()),
            |_| Ok(()),
        ),
        Err(crate::preparation::RestoreExecutionError::WriterQuiescence(_))
    ));
    assert_eq!(prepared.journal().phase, RestorePhase::ContentStaged);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"current before restore");

    // Model a process interruption immediately after the durable stop phase
    // and an apply that changed only part of the enrolled tree.
    let mut interrupted = prepared.journal().clone();
    journal_store.advance(&mut interrupted, RestorePhase::WritersStopped).unwrap();
    fs::write(fixture.path.join("source/a"), b"partially restored").unwrap();
    drop(prepared);

    let mut resumed =
        resume(&fixture, &Identifier::new("restore-execute").unwrap(), &checkpoint_snapshot())
            .unwrap();
    assert_eq!(resumed.journal().phase, RestorePhase::WritersStopped);
    assert!(matches!(
        stores.execute_configuration_only(
            &mut resumed,
            &destination,
            || Ok(()),
            |_| Err("configuration health check failed".into()),
        ),
        Err(crate::preparation::RestoreExecutionError::HealthCheck(_))
    ));
    assert_eq!(resumed.journal().phase, RestorePhase::ConfigurationRestored);

    // A crash after the durable health phase must revalidate live state before
    // completing; state may have changed after the previous writer guard died.
    let mut health_checked = resumed.journal().clone();
    journal_store.advance(&mut health_checked, RestorePhase::HealthChecked).unwrap();
    fs::write(fixture.path.join("source/a"), b"changed after health check").unwrap();
    drop(resumed);
    let mut resumed =
        resume(&fixture, &Identifier::new("restore-execute").unwrap(), &checkpoint_snapshot())
            .unwrap();
    assert_eq!(resumed.journal().phase, RestorePhase::HealthChecked);
    let completed = stores
        .execute_configuration_only(
            &mut resumed,
            &destination,
            || Ok(()),
            |journal| {
                assert_eq!(journal.phase, RestorePhase::HealthChecked);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(completed.phase, RestorePhase::Complete);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
}

#[test]
fn complete_linked_restore_resumes_only_after_recorded_target_is_confirmed_booted() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("source/a"), b"current before linked restore").unwrap();
    let prepared =
        begin_with(&fixture, "restore-linked", false, complete_linked_compatibility(), |request| {
            request.scope = RestoreScope::CompleteLinked
        })
        .unwrap();
    assert_eq!(prepared.journal().phase, RestorePhase::ContentStaged);
    let journals = journals(&fixture);
    let mut journal = prepared.journal().clone();
    journals.advance(&mut journal, RestorePhase::WritersStopped).unwrap();
    let receipt = ryoku_snapper::RootRollbackReceipt {
        config: ryoku_snapper::ConfigName::new("root").unwrap(),
        source_snapshot: ryoku_snapper::SnapshotId::new("42").unwrap(),
        previous_default_snapshot: ryoku_snapper::SnapshotId::new("44").unwrap(),
        backup_snapshot: ryoku_snapper::SnapshotId::new("45").unwrap(),
        rollback_target_snapshot: ryoku_snapper::SnapshotId::new("46").unwrap(),
        transaction_id: ryoku_snapper::TransactionId::new("restore-linked:rollback").unwrap(),
    };
    journals.record_root_rollback_result(&mut journal, &receipt).unwrap();
    assert_eq!(journal.phase, RestorePhase::WritersStopped);
    assert_eq!(journal.root_rollback.as_ref().unwrap().target_snapshot(), "46");
    let evidence = crate::native_checkpoint::test_running_rollback_target(
        journal.restore_id.clone(),
        "46".into(),
        journal.plan.filesystem_uuid.clone(),
    );
    journals.confirm_root_rollback_booted(&mut journal, &evidence).unwrap();
    assert_eq!(journal.phase, RestorePhase::RootRollbackComplete);
    assert!(!journal.has_recorded_root_rollback_handoff(RestoreScope::CompleteLinked));
    fs::write(fixture.path.join("source/a"), b"rolled back root contents").unwrap();
    drop(prepared);

    let root = File::open(fixture.path.join("source")).unwrap();
    let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let stages = fixture.store();
    let mut resumed = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    }
    .resume(&journal.restore_id, &complete_linked_compatibility(), &checkpoint_snapshot(), &roots)
    .unwrap();
    assert_eq!(resumed.journal().phase, RestorePhase::RootRollbackComplete);
    let destination = [ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &root }];
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    };
    assert!(matches!(
        stores.execute_configuration_restore(
            &mut resumed,
            &destination,
            || Ok(()),
            |_| { Err("systemd recovery gate is unhealthy".into()) }
        ),
        Err(crate::preparation::RestoreExecutionError::HealthCheck(_))
    ));
    assert_eq!(resumed.journal().phase, RestorePhase::ConfigurationRestored);
    assert_eq!(
        journals.load(&journal.restore_id).unwrap().phase,
        RestorePhase::ConfigurationRestored
    );

    let completed = stores
        .execute_configuration_restore(&mut resumed, &destination, || Ok(()), |_| Ok(()))
        .unwrap();
    assert_eq!(completed.phase, RestorePhase::Complete);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
}

struct RollbackBackendFixture {
    calls: usize,
    result: Result<ryoku_snapper::RootRollbackReceipt, ryoku_snapper::SnapperCliError>,
}
impl crate::preparation::RootRollbackBackend for RollbackBackendFixture {
    fn rollback_to(
        &mut self,
        request: &ryoku_snapper::RootRollbackRequest,
    ) -> Result<ryoku_snapper::RootRollbackReceipt, ryoku_snapper::SnapperCliError> {
        self.calls += 1;
        assert_eq!(request.config.as_str(), "root");
        assert_eq!(request.source_snapshot.as_str(), "42");
        assert!(matches!(
            request.transaction_id.as_str(),
            "rollback-attempt:rollback" | "system-attempt:rollback"
        ));
        self.result.clone()
    }
}

fn rollback_receipt() -> ryoku_snapper::RootRollbackReceipt {
    ryoku_snapper::RootRollbackReceipt {
        config: ryoku_snapper::ConfigName::new("root").unwrap(),
        source_snapshot: ryoku_snapper::SnapshotId::new("42").unwrap(),
        previous_default_snapshot: ryoku_snapper::SnapshotId::new("44").unwrap(),
        backup_snapshot: ryoku_snapper::SnapshotId::new("45").unwrap(),
        rollback_target_snapshot: ryoku_snapper::SnapshotId::new("46").unwrap(),
        transaction_id: ryoku_snapper::TransactionId::new("rollback-attempt:rollback").unwrap(),
    }
}

#[test]
fn system_only_restore_requires_boot_verification_and_health_before_completion() {
    let fixture = Fixture::new();
    let mut prepared = begin_system(&fixture).unwrap();
    let journals = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    };
    let mut receipt = rollback_receipt();
    receipt.transaction_id = ryoku_snapper::TransactionId::new("system-attempt:rollback").unwrap();
    let mut backend = RollbackBackendFixture { calls: 0, result: Ok(receipt) };
    let handoff =
        stores.execute_system_root_rollback(&mut prepared, || Ok(()), &mut backend).unwrap();
    assert_eq!(handoff.journal().phase, RestorePhase::WritersStopped);
    let mut journal = handoff.journal().clone();
    let evidence = crate::native_checkpoint::test_running_rollback_target(
        journal.restore_id.clone(),
        "46".into(),
        journal.plan.filesystem_uuid.clone(),
    );
    journals.confirm_root_rollback_booted(&mut journal, &evidence).unwrap();
    assert_eq!(journal.phase, RestorePhase::RootRollbackComplete);

    assert!(matches!(
        stores.complete_system_restore(&mut journal, |_| Err("health failed".into())),
        Err(crate::preparation::RestoreExecutionError::HealthCheck(_))
    ));
    assert_eq!(journal.phase, RestorePhase::RootRollbackComplete);
    let complete = stores.complete_system_restore(&mut journal, |_| Ok(())).unwrap();
    assert_eq!(complete.phase, RestorePhase::Complete);
}

#[test]
fn complete_linked_native_rollback_persists_receipt_and_returns_writer_guard() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let fixture = Fixture::new();
    let mut prepared = begin_with(
        &fixture,
        "rollback-attempt",
        false,
        complete_linked_compatibility(),
        |request| request.scope = RestoreScope::CompleteLinked,
    )
    .unwrap();
    let journals = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    };
    let dropped = Arc::new(AtomicBool::new(false));
    let mut backend = RollbackBackendFixture { calls: 0, result: Ok(rollback_receipt()) };
    let handoff = stores
        .execute_complete_linked_root_rollback(
            &mut prepared,
            || Ok(Guard(dropped.clone())),
            &mut backend,
        )
        .unwrap();

    assert_eq!(backend.calls, 1);
    assert_eq!(handoff.journal().phase, RestorePhase::WritersStopped);
    assert!(handoff.journal().has_recorded_root_rollback_handoff(RestoreScope::CompleteLinked));
    assert_eq!(handoff.journal().root_rollback.as_ref().unwrap().target_snapshot(), "46");
    assert_eq!(handoff.receipt().transaction_id.as_str(), "rollback-attempt:rollback");
    assert!(!dropped.load(Ordering::SeqCst));
    drop(handoff);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn uncertain_native_rollback_stays_pending_and_is_never_retried_automatically() {
    let fixture = Fixture::new();
    let mut prepared = begin_with(
        &fixture,
        "rollback-attempt",
        false,
        complete_linked_compatibility(),
        |request| request.scope = RestoreScope::CompleteLinked,
    )
    .unwrap();
    let journals = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journals,
        stages: &stages,
    };
    let mut backend =
        RollbackBackendFixture { calls: 0, result: Err(ryoku_snapper::SnapperCliError::TimedOut) };
    assert!(matches!(
        stores.execute_complete_linked_root_rollback(&mut prepared, || Ok(()), &mut backend),
        Err(crate::preparation::RestoreExecutionError::NativeRollback(
            ryoku_snapper::SnapperCliError::TimedOut
        ))
    ));
    assert_eq!(backend.calls, 1);
    assert_eq!(prepared.journal().phase, RestorePhase::WritersStopped);
    assert!(prepared.journal().root_rollback.is_none());
    assert!(matches!(
        stores.execute_complete_linked_root_rollback(&mut prepared, || Ok(()), &mut backend),
        Err(crate::preparation::RestoreExecutionError::WrongScopeOrPhase)
    ));
    assert_eq!(backend.calls, 1);
}

fn current_system_checkpoint_bytes(prepared: &PreparedSystemRestore) -> Vec<u8> {
    let record = prepared
        .checkpoint()
        .manifest()
        .files()
        .iter()
        .find(|entry| entry.path.as_str() == "a")
        .unwrap();
    let EntryKind::RegularFile { sha256, .. } = &record.kind else { panic!("expected file") };
    prepared.checkpoint().read_payload(sha256).unwrap()
}

#[test]
fn preparation_captures_actual_current_bytes_and_resumes_without_recapture() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("source/a"), b"current before restore").unwrap();
    let prepared = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    assert_eq!(prepared.journal().phase, RestorePhase::ContentStaged);
    assert_eq!(prepared.journal().restore_id.as_str(), "restore-a");
    assert_eq!(current_bytes(&prepared), b"current before restore");
    assert_eq!(prepared.source().entries()[0].content().unwrap(), b"saved configuration");
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"current before restore");
    drop(prepared);
    let resumed =
        resume(&fixture, &Identifier::new("restore-a").unwrap(), &checkpoint_snapshot()).unwrap();
    assert_eq!(current_bytes(&resumed), b"current before restore");
    assert_eq!(resumed.stage().plan(), resumed.source().plan());
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
}

#[test]
fn repeated_restores_of_one_source_have_distinct_attempts_and_checkpoints() {
    let fixture = Fixture::new();
    let first = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    fs::write(fixture.path.join("source/a"), b"second current state").unwrap();
    let second = begin(&fixture, "restore-b", false, compatibility()).unwrap();
    assert_eq!(first.source().plan(), second.source().plan());
    assert_ne!(first.journal().restore_id, second.journal().restore_id);
    assert_ne!(first.journal().current_state_generation, second.journal().current_state_generation);
    assert_eq!(current_bytes(&first), b"saved configuration");
    assert_eq!(current_bytes(&second), b"second current state");
    assert!(begin(&fixture, "restore-a", false, compatibility()).is_err());
}

#[test]
fn missing_affected_path_fails_before_checkpoint_or_journal_creation() {
    let fixture = Fixture::new();
    assert!(matches!(
        begin(&fixture, "restore-a", true, compatibility()),
        Err(PreparationError::MissingCheckpointCoverage)
    ));
    assert!(fixture.generation.verify(&Identifier::new("checkpoint-restore-a").unwrap()).is_err());
    assert!(!fixture.stage().exists());
    assert!(!fixture.path.join("journals/restore-a.json").exists());
}

#[test]
fn incompatible_source_retains_the_verified_current_generation_without_staging() {
    let fixture = Fixture::new();
    let mut facts = compatibility();
    facts.configuration = CompatibilityStatus::Incompatible;
    assert!(matches!(
        begin(&fixture, "restore-a", false, facts),
        Err(PreparationError::Preflight(_))
    ));
    assert!(fixture.generation.verify(&Identifier::new("checkpoint-restore-a").unwrap()).is_ok());
    assert!(!fixture.stage().exists());
    assert!(!fixture.path.join("journals/restore-a.json").exists());
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
}

#[test]
fn configuration_restored_journal_reopens_after_health_failure_and_completes() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("source/a"), b"current before restore").unwrap();
    let restore_id = Identifier::new("restore-health-reopen").unwrap();
    let mut prepared = begin(&fixture, restore_id.as_str(), false, compatibility()).unwrap();
    let root = File::open(fixture.path.join("source")).unwrap();
    let destination = [ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &root }];
    let journal_store = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journal_store,
        stages: &stages,
    };

    assert!(matches!(
        stores.execute_configuration_only(
            &mut prepared,
            &destination,
            || Ok(()),
            |_| Err("simulated health interruption".into()),
        ),
        Err(crate::preparation::RestoreExecutionError::HealthCheck(reason))
            if reason == "simulated health interruption"
    ));
    assert_eq!(prepared.journal().phase, RestorePhase::ConfigurationRestored);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
    drop(prepared);

    let mut reopened = resume(&fixture, &restore_id, &checkpoint_snapshot()).unwrap();
    assert_eq!(reopened.journal().phase, RestorePhase::ConfigurationRestored);
    let completed = stores
        .execute_configuration_only(
            &mut reopened,
            &destination,
            || Ok(()),
            |journal| {
                assert_eq!(journal.phase, RestorePhase::ConfigurationRestored);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(completed.phase, RestorePhase::Complete);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
}

#[test]
fn configuration_restore_process_exit_reopens_durable_restored_phase() {
    const CHILD_TEST: &str = "configuration_stage::disk_stage_tests::preparation_tests::configuration_restore_process_exit_child";

    let fixture = Fixture::new();
    fs::write(fixture.path.join("source/a"), b"current before restore").unwrap();
    let restore_id = Identifier::new("restore-process-reopen").unwrap();
    let prepared = begin(&fixture, restore_id.as_str(), false, compatibility()).unwrap();
    assert_eq!(prepared.journal().phase, RestorePhase::ContentStaged);
    drop(prepared);

    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
        .env("RYOKU_RESTORE_CHILD_ROOT", &fixture.path)
        .env("RYOKU_RESTORE_CHILD_ID", restore_id.as_str())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(77), "child did not exit at the durable restore boundary");

    let root = File::open(fixture.path.join("source")).unwrap();
    let destination = [ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &root }];
    let journal_store = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journal_store,
        stages: &stages,
    };
    let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let mut reopened =
        stores.resume(&restore_id, &compatibility(), &checkpoint_snapshot(), &roots).unwrap();
    assert_eq!(reopened.journal().phase, RestorePhase::ConfigurationRestored);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
    let completed = stores
        .execute_configuration_only(&mut reopened, &destination, || Ok(()), |_| Ok(()))
        .unwrap();
    assert_eq!(completed.phase, RestorePhase::Complete);
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"saved configuration");
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
}

#[test]
#[ignore = "spawned after a durable configuration restore phase"]
fn configuration_restore_process_exit_child() {
    let (Ok(root), Ok(restore_id)) =
        (std::env::var("RYOKU_RESTORE_CHILD_ROOT"), std::env::var("RYOKU_RESTORE_CHILD_ID"))
    else {
        return;
    };
    let fixture = Fixture::reopen(PathBuf::from(root));
    let restore_id = Identifier::new(restore_id).unwrap();
    let root = File::open(fixture.path.join("source")).unwrap();
    let destinations = [ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &root }];
    let journal_store = journals(&fixture);
    let stages = fixture.store();
    let stores = PreparationStores {
        generations: &fixture.generation,
        journals: &journal_store,
        stages: &stages,
    };
    let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
        .into_iter()
        .map(|path| crate::RelativePath::new(path).unwrap())
        .collect();
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &root, paths: &paths }];
    let mut prepared =
        stores.resume(&restore_id, &compatibility(), &checkpoint_snapshot(), &roots).unwrap();
    let _ = stores.execute_configuration_only(
        &mut prepared,
        &destinations,
        || Ok(()),
        |_| std::process::exit(77),
    );
    panic!("restore did not reach its health-check boundary");
}

#[test]
fn incomplete_stage_retains_checkpoint_and_resumes_only_after_explicit_discard() {
    let fixture = Fixture::new();
    let stage_store = fixture.store();
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = stage_store.create_with_hook(&fixture.source, |boundary| {
            if boundary == 2 {
                panic!("simulated exit after staged objects are durable but before the receipt");
            }
        });
    }));
    assert!(interrupted.is_err());
    assert!(stage_store.open_verified(&fixture.source).is_err());
    assert!(fixture.stage().join("object-000").exists());
    assert!(begin(&fixture, "restore-a", false, compatibility()).is_err());
    let journal = journals(&fixture);
    let state = journal.load(&Identifier::new("restore-a").unwrap()).unwrap();
    assert_eq!(state.phase, RestorePhase::CheckpointVerified);
    assert!(fixture.generation.verify(&state.current_state_generation).is_ok());
    let stage = fixture.store();
    PreparationStores { generations: &fixture.generation, journals: &journal, stages: &stage }
        .discard_incomplete_stage(&state.restore_id)
        .unwrap();
    let resumed = resume(&fixture, &state.restore_id, &checkpoint_snapshot()).unwrap();
    assert_eq!(resumed.journal().phase, RestorePhase::ContentStaged);
    assert_eq!(
        resumed.journal().current_state_manifest_sha256,
        state.current_state_manifest_sha256
    );
}

#[test]
fn incomplete_stage_discard_refuses_after_content_staged() {
    let fixture = Fixture::new();
    let prepared = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    let journal = journals(&fixture);
    let stage = fixture.store();
    let stores =
        PreparationStores { generations: &fixture.generation, journals: &journal, stages: &stage };
    assert!(matches!(
        stores.discard_incomplete_stage(&prepared.journal().restore_id),
        Err(PreparationError::UnexpectedPhase)
    ));
    assert!(fixture.stage().join("complete.json").exists());
}

#[test]
fn missing_native_checkpoint_partner_and_missing_completed_stage_do_not_advance() {
    let fixture = Fixture::new();
    let prepared = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    let journal = journals(&fixture);
    assert!(matches!(
        resume(&fixture, &prepared.journal().restore_id, &SystemSnapshotStatus::Missing),
        Err(PreparationError::CheckpointSnapshotUnavailable)
    ));
    fs::remove_file(fixture.stage().join("complete.json")).unwrap();
    assert!(resume(&fixture, &prepared.journal().restore_id, &checkpoint_snapshot()).is_err());
    assert_eq!(journal.load(&prepared.journal().restore_id).unwrap(), *prepared.journal());
    assert!(!fixture.stage().join("complete.json").exists());
}

#[test]
fn checkpoint_payload_corruption_blocks_preparation_resume() {
    let fixture = Fixture::new();
    let prepared = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    let generation_path = fixture
        .path
        .join("generation/generations")
        .join(prepared.journal().current_state_generation.as_str());
    // Mutate the committed manifest; reopening must fail instead of accepting
    // a caller's previously successful checkpoint status.
    fs::set_permissions(&generation_path, fs::Permissions::from_mode(0o700)).unwrap();
    let manifest_path = generation_path.join("manifest.json");
    fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&manifest_path, b"corrupted checkpoint").unwrap();
    let journal = journals(&fixture);
    assert!(resume(&fixture, &prepared.journal().restore_id, &checkpoint_snapshot()).is_err());
    assert_eq!(
        journal.load(&prepared.journal().restore_id).unwrap().phase,
        RestorePhase::ContentStaged
    );
}

#[test]
fn changed_live_state_requires_a_new_checkpoint_instead_of_silently_recapturing() {
    let fixture = Fixture::new();
    let prepared = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    fs::write(fixture.path.join("source/a"), b"changed after interruption").unwrap();
    assert!(matches!(
        resume(&fixture, &prepared.journal().restore_id, &checkpoint_snapshot()),
        Err(PreparationError::LiveStateChanged)
    ));
    assert_eq!(current_bytes(&prepared), b"saved configuration");
    assert_eq!(
        journals(&fixture).load(&prepared.journal().restore_id).unwrap(),
        *prepared.journal()
    );
    assert_eq!(fs::read(fixture.path.join("source/a")).unwrap(), b"changed after interruption");
}

#[test]
fn reused_source_snapshot_and_unsupported_scope_fail_before_new_checkpoint() {
    let fixture = Fixture::new();
    assert!(matches!(
        begin_with(&fixture, "restore-a", false, compatibility(), |request| {
            request.checkpoint.snapshot = VerifiedSnapshotLink::from_preverified_link(
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            );
        }),
        Err(PreparationError::InvalidCheckpointIdentity)
    ));
    assert!(matches!(
        begin_with(&fixture, "restore-a", false, compatibility(), |request| {
            request.scope = RestoreScope::SystemOnly;
        }),
        Err(PreparationError::UnsupportedScope)
    ));
    assert!(fixture.generation.verify(&Identifier::new("checkpoint-restore-a").unwrap()).is_err());
    assert!(!fixture.path.join("journals/restore-a.json").exists());
}

#[test]
fn complete_linked_preparation_checks_source_partner_without_claiming_rollback() {
    let fixture = Fixture::new();
    assert!(matches!(
        begin_with(&fixture, "restore-a", false, compatibility(), |request| {
            request.scope = RestoreScope::CompleteLinked;
        }),
        Err(PreparationError::Preflight(_))
    ));
    assert!(fixture.generation.verify(&Identifier::new("checkpoint-restore-a").unwrap()).is_ok());
    let mut facts = compatibility();
    facts.source_snapshot = SystemSnapshotStatus::Available {
        snapper_config: Identifier::new("root").unwrap(),
        snapshot_id: "42".into(),
        filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
    };
    facts.system = CompatibilityStatus::Compatible;
    let ready = begin_with(&fixture, "restore-b", false, facts, |request| {
        request.scope = RestoreScope::CompleteLinked;
    })
    .unwrap();
    assert_eq!(ready.journal().phase, RestorePhase::ContentStaged);
}

#[test]
fn configuration_resume_preserves_a_later_recovery_phase() {
    let fixture = Fixture::new();
    let ready = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    let journal = journals(&fixture);
    let mut state = ready.journal().clone();
    journal.advance(&mut state, RestorePhase::WritersStopped).unwrap();
    let resumed = resume(&fixture, &state.restore_id, &checkpoint_snapshot()).unwrap();
    assert_eq!(resumed.journal().phase, RestorePhase::WritersStopped);
    assert_eq!(journal.load(&state.restore_id).unwrap(), state);
}
