use super::*;
use crate::{
    generation_capture::{DeclaredRoot, GenerationCaptureRequest, VerifiedSnapshotLink},
    preparation::{
        ConfigurationPreparationRequest, PreparationError, PreparationStores,
        PreparedConfigurationRestore, RestoreCompatibility,
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
fn incomplete_stage_retains_checkpoint_and_resumes_only_after_explicit_discard() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.stage()).unwrap();
    fs::set_permissions(fixture.stage(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(begin(&fixture, "restore-a", false, compatibility()).is_err());
    let journal = journals(&fixture);
    let state = journal.load(&Identifier::new("restore-a").unwrap()).unwrap();
    assert_eq!(state.phase, RestorePhase::CheckpointVerified);
    assert!(fixture.generation.verify(&state.current_state_generation).is_ok());
    fs::remove_dir(fixture.stage()).unwrap();
    let resumed = resume(&fixture, &state.restore_id, &checkpoint_snapshot()).unwrap();
    assert_eq!(resumed.journal().phase, RestorePhase::ContentStaged);
    assert_eq!(
        resumed.journal().current_state_manifest_sha256,
        state.current_state_manifest_sha256
    );
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
fn preparation_does_not_rewind_a_later_recovery_phase() {
    let fixture = Fixture::new();
    let ready = begin(&fixture, "restore-a", false, compatibility()).unwrap();
    let journal = journals(&fixture);
    let mut state = ready.journal().clone();
    journal.advance(&mut state, RestorePhase::WritersStopped).unwrap();
    assert!(matches!(
        resume(&fixture, &state.restore_id, &checkpoint_snapshot()),
        Err(PreparationError::UnexpectedPhase)
    ));
    assert_eq!(journal.load(&state.restore_id).unwrap(), state);
}
