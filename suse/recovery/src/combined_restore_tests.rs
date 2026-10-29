use super::*;
use crate::{
    combined_restore::{
        CombinedExecution, CombinedPreparation, CombinedRestoreIntent, CombinedStores,
        PreparedCombinedRestore,
    },
    configuration_apply::ConfigurationDestination,
    disk_stage::DiskStageStore,
    generation_capture::DeclaredRoot,
    restore::{
        ArtifactStatus, CompatibilityStatus, PreflightEvidence, RestoreJournalStore, RestorePhase,
        RestoreScope, SystemSnapshotStatus,
    },
};
const ORIGINAL: &[u8] = br#"{"semantic":{"models":[{"name":"original","manifest":"/fixture/model.json"}]},"secret":"fixture-private"}"#;
const CURRENT: &[u8] = br#"{"semantic":{"models":[]},"secret":"fixture-private"}"#;
struct CombinedFixture {
    fixture: Fixture,
    model: AssetPath,
    source: VerifiedGeneration,
    checkpoint: VerifiedGeneration,
    facts: PreflightEvidence,
    stages: DiskStageStore,
    phases: RestoreJournalStore,
    preference: Vec<RelativePath>,
}
impl CombinedFixture {
    fn new() -> Self {
        Self::with_tree(false)
    }
    fn with_tree(tree: bool) -> Self {
        Self::with_families(tree, false)
    }
    fn with_outputs() -> Self {
        Self::with_families(true, true)
    }
    fn with_families(tree: bool, outputs: bool) -> Self {
        let fixture = Fixture::new();
        let model = AssetPath::new(
            AssetNamespace::SemanticModels,
            RelativePath::new("fixture/model.onnx").unwrap(),
        )
        .unwrap();
        fixture.write(&model, b"original model");
        let config = fixture.path.join("home/.config/ryoku/ryogami.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, ORIGINAL).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut source_scope = if tree {
            let root = model.with_relative(RelativePath::new("fixture").unwrap()).unwrap();
            fixture.write(&cover(), b"original cover");
            crate::ryogami_trees::inspect_scope(
                &fixture.home,
                &fixture.account,
                root,
                vec![cover()],
                MAX_CAPTURE_LIMITS.max_file_bytes,
                MAX_CAPTURE_LIMITS.max_total_bytes,
                || true,
            )
            .unwrap()
        } else {
            AssetScope::new(
                vec![model.clone()],
                MAX_CAPTURE_LIMITS.max_file_bytes,
                MAX_CAPTURE_LIMITS.max_total_bytes,
            )
            .unwrap()
        };
        if outputs {
            let declarations = declared_stage_fixture(&fixture);
            let mut paths = source_scope.paths().to_vec();
            paths.extend(declarations.paths().iter().cloned());
            source_scope = AssetScope::new_with_outputs(
                paths,
                MAX_CAPTURE_LIMITS.max_file_bytes,
                MAX_CAPTURE_LIMITS.max_total_bytes,
                declarations.output_declaration().cloned(),
            )
            .unwrap()
            .with_trees(source_scope.trees().to_vec())
            .unwrap();
        }
        let source = fixture.capture_scope(1, &source_scope);
        fixture.write(&model, b"changed model");
        if tree {
            fixture.write(&cover(), b"changed cover");
        }
        if outputs {
            std::fs::write(fixture.path.join("home/.config/app/theme.css"), b"changed-theme")
                .unwrap();
            std::fs::remove_dir_all(fixture.path.join("home/.config/new")).unwrap();
        }
        std::fs::write(&config, CURRENT).unwrap();
        let intent = CombinedRestoreIntent::discover(
            Identifier::new("combined-operation").unwrap(),
            &source,
            &fixture.home,
            &fixture.account,
        )
        .unwrap();
        let checkpoint = fixture.publish_assets(
            2,
            intent.scope().capture(&fixture.home, &fixture.account).unwrap(),
            true,
            "combined-operation",
        );
        let facts = PreflightEvidence {
            generation: ArtifactStatus::Verified,
            archive: ArtifactStatus::Verified,
            current_state_checkpoint: ArtifactStatus::Verified,
            system_snapshot: SystemSnapshotStatus::Available {
                snapper_config: source.manifest().snapper_config.clone(),
                snapshot_id: source.manifest().snapper_snapshot_id.clone(),
                filesystem_uuid: source.manifest().filesystem_uuid.clone(),
            },
            system_compatibility: CompatibilityStatus::Compatible,
            configuration_compatibility: CompatibilityStatus::Compatible,
        };
        for name in ["stages", "phases"] {
            std::fs::create_dir(fixture.path.join(name)).unwrap();
            std::fs::set_permissions(
                fixture.path.join(name),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let stages =
            DiskStageStore::open_for_test(&File::open(fixture.path.join("stages")).unwrap())
                .unwrap();
        let phases =
            RestoreJournalStore::open(&File::open(fixture.path.join("phases")).unwrap()).unwrap();
        Self {
            fixture,
            model,
            source,
            checkpoint,
            facts,
            stages,
            phases,
            preference: vec![RelativePath::new(".config/ryoku/ryogami.json").unwrap()],
        }
    }
    fn intent(&self) -> CombinedRestoreIntent {
        CombinedRestoreIntent::from_checkpoint(
            Identifier::new("combined-operation").unwrap(),
            &self.source,
            &self.checkpoint,
            &self.fixture.account,
        )
        .unwrap()
    }
    fn request(&self) -> CombinedPreparation<'_> {
        CombinedPreparation {
            source: &self.source,
            checkpoint: &self.checkpoint,
            home: &self.fixture.home,
            account: &self.fixture.account,
            scope: RestoreScope::ConfigurationOnly,
            evidence: &self.facts,
        }
    }
    fn stores(&self) -> CombinedStores<'_> {
        CombinedStores {
            stages: &self.stages,
            journals: &self.phases,
            assets: &self.fixture.journal,
        }
    }
    fn config(&self) -> PathBuf {
        self.fixture.path.join("home/.config/ryoku/ryogami.json")
    }
    fn begin(&self) -> PreparedCombinedRestore {
        let prepared = PreparedCombinedRestore::prepare(self.intent(), self.request()).unwrap();
        let roots = [DeclaredRoot {
            root: StateRoot::UserHome,
            descriptor: &self.fixture.home,
            paths: &self.preference,
        }];
        let destinations =
            [ConfigurationDestination { root: StateRoot::UserHome, directory: &self.fixture.home }];
        let execution = CombinedExecution {
            source: &self.source,
            checkpoint: &self.checkpoint,
            home: &self.fixture.home,
            account: &self.fixture.account,
            destinations: &destinations,
            checkpoint_roots: &roots,
        };
        let mut journal = prepared.begin(&self.stores(), &execution).unwrap();
        assert_eq!(journal.phase, RestorePhase::ContentStaged);
        assert!(
            prepared
                .apply_owned(&self.stores(), &execution, || true, self.fixture.account.uid)
                .is_err()
        );
        self.phases.advance(&mut journal, RestorePhase::WritersStopped).unwrap();
        prepared
    }
    fn apply(
        &self,
        prepared: &PreparedCombinedRestore,
        boundary: impl FnMut() -> bool,
    ) -> std::result::Result<crate::restore::RestoreJournal, crate::combined_restore::CombinedError>
    {
        let roots = [DeclaredRoot {
            root: StateRoot::UserHome,
            descriptor: &self.fixture.home,
            paths: &self.preference,
        }];
        let destinations =
            [ConfigurationDestination { root: StateRoot::UserHome, directory: &self.fixture.home }];
        prepared.apply_owned(
            &self.stores(),
            &CombinedExecution {
                source: &self.source,
                checkpoint: &self.checkpoint,
                home: &self.fixture.home,
                account: &self.fixture.account,
                destinations: &destinations,
                checkpoint_roots: &roots,
            },
            boundary,
            self.fixture.account.uid,
        )
    }
    fn reopen(&self) -> PreparedCombinedRestore {
        PreparedCombinedRestore::reopen(
            Identifier::new("combined-operation").unwrap(),
            &self.stores(),
            self.request(),
        )
        .unwrap()
    }
    fn with_execution<T>(&self, run: impl FnOnce(&CombinedExecution<'_>) -> T) -> T {
        let roots = [DeclaredRoot {
            root: StateRoot::UserHome,
            descriptor: &self.fixture.home,
            paths: &self.preference,
        }];
        let destinations =
            [ConfigurationDestination { root: StateRoot::UserHome, directory: &self.fixture.home }];
        run(&CombinedExecution {
            source: &self.source,
            checkpoint: &self.checkpoint,
            home: &self.fixture.home,
            account: &self.fixture.account,
            destinations: &destinations,
            checkpoint_roots: &roots,
        })
    }
    fn phase(&self) -> RestorePhase {
        self.phases.load(&Identifier::new("combined-operation").unwrap()).unwrap().phase
    }
}
#[test]
fn combined_application_restores_real_configuration_and_model_before_phase_and_reopens() {
    let f = CombinedFixture::new();
    let prepared = f.begin();
    let sentinel = f.fixture.path.join("home/unrelated-secret");
    std::fs::write(&sentinel, b"unchanged").unwrap();
    let journal = f.apply(&prepared, || true).unwrap();
    assert_eq!(journal.phase, RestorePhase::ConfigurationRestored);
    assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
    assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"original model");
    assert_eq!(std::fs::read(sentinel).unwrap(), b"unchanged");
    drop(prepared);
    let reopened = f.reopen();
    assert_eq!(f.apply(&reopened, || true).unwrap().phase, RestorePhase::ConfigurationRestored);
}
#[test]
fn either_family_failure_never_advances_combined_restore_and_preference_retry_skips_verified_asset()
{
    for family in ["asset", "preference"] {
        let f = CombinedFixture::new();
        let prepared = f.begin();
        let parent = f.fixture.path.join("home/.config/ryoku");
        let backup = f.fixture.path.join("home/.config/saved");
        if family == "asset" {
            f.fixture.write(&f.model, b"unplanned bytes");
        } else {
            std::fs::rename(&parent, &backup).unwrap();
            std::os::unix::fs::symlink("/tmp", &parent).unwrap();
        }
        assert!(f.apply(&prepared, || true).is_err());
        assert_eq!(f.phase(), RestorePhase::WritersStopped);
        if family == "asset" {
            assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
            assert!(
                PreparedCombinedRestore::reopen(
                    Identifier::new("combined-operation").unwrap(),
                    &f.stores(),
                    f.request()
                )
                .is_err()
            );
        } else {
            assert_eq!(std::fs::read(backup.join("ryogami.json")).unwrap(), CURRENT);
            assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"original model");
            std::fs::remove_file(&parent).unwrap();
            std::fs::rename(backup, parent).unwrap();
            let reopened = f.reopen();
            assert_eq!(
                f.apply(&reopened, || true).unwrap().phase,
                RestorePhase::ConfigurationRestored
            );
            assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
        }
    }
}
#[test]
fn combined_partial_asset_requires_fresh_checkpointed_cleanup_then_reopens_original_record() {
    let f = CombinedFixture::new();
    let prepared = f.begin();
    let stage = f.intent().scope().paths().iter().find(|path| **path != f.model).unwrap().clone();
    assert!(f.apply(&prepared, || !f.fixture.file(&stage).exists()).is_err());
    assert_eq!(f.phase(), RestorePhase::WritersStopped);
    assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
    let reopened = f.reopen();
    assert!(f.apply(&reopened, || true).is_err());
    assert!(f.fixture.file(&stage).exists());
    let cleanup = reopened
        .partial_cleanup(
            Identifier::new("tx-3").unwrap(),
            &f.checkpoint,
            &f.fixture.home,
            &f.fixture.account,
            &f.fixture.journal,
        )
        .unwrap();
    assert_eq!(cleanup.scope().paths().len(), 2);
    assert!(cleanup.scope().paths().contains(&stage));
    assert!(!cleanup.scope().paths().contains(&f.model));
    let fresh = f.fixture.capture(3, cleanup.scope().paths());
    assert!(
        cleanup
            .apply(
                &f.source,
                &fresh,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
                || true
            )
            .is_err()
    );
    assert!(
        cleanup
            .apply(
                &f.checkpoint,
                &f.checkpoint,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
                || true
            )
            .is_err()
    );
    assert!(
        cleanup
            .apply(
                &f.checkpoint,
                &fresh,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
                || false
            )
            .is_err()
    );
    assert!(f.fixture.file(&stage).exists());
    assert_eq!(
        cleanup
            .apply(
                &f.checkpoint,
                &fresh,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
                || true
            )
            .unwrap(),
        1
    );
    assert_eq!(f.phase(), RestorePhase::WritersStopped);
    let record =
        std::fs::read_to_string(f.fixture.path.join("journal/combined-cleanup-tx-3.json")).unwrap();
    assert!(record.contains("combined-operation") && record.contains("tx-3"));
    assert!(!record.contains("fixture-private"));
    let reopened = f.reopen();
    assert_eq!(f.apply(&reopened, || true).unwrap().phase, RestorePhase::ConfigurationRestored);
    assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
}
#[test]
fn combined_post_asset_interruption_and_corrupt_protected_record_cannot_fake_completion() {
    let f = CombinedFixture::new();
    let prepared = f.begin();
    assert!(
        f.apply(&prepared, || std::fs::read(f.fixture.file(&f.model)).unwrap()
            != b"original model")
            .is_err()
    );
    assert_eq!(f.phase(), RestorePhase::WritersStopped);
    assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
    let reopened = f.reopen();
    assert_eq!(f.apply(&reopened, || true).unwrap().phase, RestorePhase::ConfigurationRestored);
    let record = f.fixture.path.join("journal/combined-restore-combined-operation.json");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&record, b"{}").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        PreparedCombinedRestore::reopen(
            Identifier::new("combined-operation").unwrap(),
            &f.stores(),
            f.request()
        )
        .is_err()
    );
}

#[test]
fn combined_preparation_resumes_each_durable_boundary_without_live_writes() {
    for stop in 2..=6 {
        let f = CombinedFixture::new();
        let prepared = PreparedCombinedRestore::prepare(f.intent(), f.request()).unwrap();
        let mut step = 0;
        assert!(matches!(
            f.with_execution(|execution| prepared.begin_with_boundary(
                &f.stores(),
                execution,
                || {
                    step += 1;
                    step != stop
                }
            )),
            Err(crate::combined_restore::CombinedError::Interrupted)
        ));
        let reconstructed = PreparedCombinedRestore::prepare(f.intent(), f.request()).unwrap();
        let journal = f
            .with_execution(|execution| reconstructed.resume_preparation(&f.stores(), execution))
            .unwrap();
        assert_eq!(journal.phase, RestorePhase::ContentStaged);
        assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
        assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"changed model");
        assert!(
            f.with_execution(
                |execution| reconstructed.discard_incomplete_preparation(&f.stores(), execution)
            )
            .is_err()
        );
    }
}
#[test]
fn combined_incomplete_stage_and_record_require_exact_explicit_cleanup_before_resume() {
    for artifact in ["stage", "record"] {
        let f = CombinedFixture::new();
        let prepared = PreparedCombinedRestore::prepare(f.intent(), f.request()).unwrap();
        let mut step = 0;
        let stop = if artifact == "stage" { 4 } else { 5 };
        assert!(
            f.with_execution(|execution| prepared.begin_with_boundary(
                &f.stores(),
                execution,
                || {
                    step += 1;
                    step != stop
                }
            ))
            .is_err()
        );
        assert_eq!(f.phase(), RestorePhase::CheckpointVerified);
        let stage = std::fs::read_dir(f.fixture.path.join("stages"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let record = f.fixture.path.join("journal/combined-restore-combined-operation.json");
        let prefix = if artifact == "stage" {
            std::fs::remove_file(stage.join("complete.json")).unwrap();
            std::fs::write(stage.join("unknown"), b"must retain").unwrap();
            None
        } else {
            // A complete record is not an incomplete artifact eligible for deletion.
            assert!(
                f.with_execution(
                    |execution| prepared.discard_incomplete_preparation(&f.stores(), execution)
                )
                .is_err()
            );
            let bytes = std::fs::read(&record).unwrap();
            std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::write(&record, b"unknown bytes").unwrap();
            Some(bytes[..bytes.len() / 2].to_vec())
        };
        assert!(
            f.with_execution(|execution| prepared.resume_preparation(&f.stores(), execution))
                .is_err()
        );
        assert!(
            f.with_execution(
                |execution| prepared.discard_incomplete_preparation(&f.stores(), execution)
            )
            .is_err()
        );
        assert!(stage.exists());
        if let Some(prefix) = prefix {
            std::fs::write(&record, prefix).unwrap();
        } else {
            std::fs::remove_file(stage.join("unknown")).unwrap();
        }
        f.with_execution(|execution| {
            prepared.discard_incomplete_preparation(&f.stores(), execution)
        })
        .unwrap();
        assert_eq!(f.phase(), RestorePhase::CheckpointVerified);
        let reconstructed = PreparedCombinedRestore::prepare(f.intent(), f.request()).unwrap();
        assert_eq!(
            f.with_execution(|execution| reconstructed.resume_preparation(&f.stores(), execution))
                .unwrap()
                .phase,
            RestorePhase::ContentStaged
        );
        assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
        assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"changed model");
        assert!(
            f.fixture
                .store
                .open_verified_transaction(&f.checkpoint.manifest().transaction_id)
                .is_ok()
        );
    }
}

#[test]
fn combined_native_handoff_binds_receipt_and_leaves_failed_or_unbooted_restore_pending() {
    struct Backend {
        fail: bool,
        calls: usize,
    }
    impl crate::preparation::RootRollbackBackend for Backend {
        fn rollback_to(
            &mut self,
            request: &ryoku_snapper::RootRollbackRequest,
        ) -> std::result::Result<ryoku_snapper::RootRollbackReceipt, ryoku_snapper::SnapperCliError>
        {
            self.calls += 1;
            assert_eq!(request.config.as_str(), "root");
            assert_eq!(request.source_snapshot.as_str(), "41");
            assert_eq!(request.transaction_id.as_str(), "combined-operation:rollback");
            if self.fail {
                return Err(ryoku_snapper::SnapperCliError::InvalidInventory);
            }
            Ok(ryoku_snapper::RootRollbackReceipt {
                config: request.config.clone(),
                source_snapshot: request.source_snapshot.clone(),
                previous_default_snapshot: ryoku_snapper::SnapshotId::new("48").unwrap(),
                backup_snapshot: ryoku_snapper::SnapshotId::new("49").unwrap(),
                rollback_target_snapshot: ryoku_snapper::SnapshotId::new("50").unwrap(),
                transaction_id: request.transaction_id.clone(),
            })
        }
    }
    for fail in [false, true] {
        let f = CombinedFixture::new();
        let mut request = f.request();
        request.scope = RestoreScope::CompleteLinked;
        let prepared = PreparedCombinedRestore::prepare(f.intent(), request).unwrap();
        f.with_execution(|execution| prepared.begin(&f.stores(), execution)).unwrap();
        let stores = crate::preparation::PreparationStores {
            generations: &f.fixture.store,
            journals: &f.phases,
            stages: &f.stages,
        };
        let mut backend = Backend { fail, calls: 0 };
        // Quiescence failure must precede durable phase or native changes.
        assert!(
            stores
                .execute_combined_root_rollback::<()>(
                    &prepared,
                    || Err("fixture writer stop".into()),
                    &mut backend
                )
                .is_err()
        );
        assert_eq!(backend.calls, 0);
        assert_eq!(f.phase(), RestorePhase::ContentStaged);
        let handoff =
            stores.execute_combined_root_rollback(&prepared, || Ok("guard"), &mut backend);
        assert_eq!(backend.calls, 1);
        assert_eq!(f.phase(), RestorePhase::WritersStopped);
        let journal = f.phases.load(prepared.operation()).unwrap();
        if fail {
            assert!(handoff.is_err());
            assert!(journal.root_rollback.is_none());
        } else {
            let handoff = handoff.unwrap();
            assert_eq!(handoff.writer_guard(), &"guard");
            assert_eq!(handoff.receipt().rollback_target_snapshot.as_str(), "50");
            assert_eq!(handoff.journal(), &journal);
            assert!(journal.root_rollback.is_some());
        }
        // Native return alone cannot authorize asset/config application or
        // another rollback; independent boot confirmation is still required.
        assert!(f.apply(&prepared, || true).is_err());
        assert!(stores.execute_combined_root_rollback(&prepared, || Ok(()), &mut backend).is_err());
        assert_eq!(backend.calls, 1);
        assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
        assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"changed model");
    }
    let f = CombinedFixture::new();
    let prepared = f.begin();
    let stores = crate::preparation::PreparationStores {
        generations: &f.fixture.store,
        journals: &f.phases,
        stages: &f.stages,
    };
    let mut backend = Backend { fail: false, calls: 0 };
    assert!(stores.execute_combined_root_rollback(&prepared, || Ok(()), &mut backend).is_err());
    assert_eq!(backend.calls, 0);
}

fn cover() -> AssetPath {
    AssetPath::new(AssetNamespace::RemotePreviews, RelativePath::new("fixture/cover.png").unwrap())
        .unwrap()
}
#[test]
fn combined_tree_restores_config_and_metadata_before_completion_and_reopens() {
    let f = CombinedFixture::with_tree(true);
    let prepared = f.begin();
    let journal = f.apply(&prepared, || true).unwrap();
    assert_eq!(journal.phase, RestorePhase::ConfigurationRestored);
    assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
    assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"original model");
    assert_eq!(f.apply(&f.reopen(), || true).unwrap().phase, RestorePhase::ConfigurationRestored);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.fixture.path.join("journal/combined-restore-combined-operation.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read(f.fixture.file(&cover())).unwrap(), b"original cover");
    assert_eq!(record["assets"].as_array().unwrap().len(), 1);
    assert_eq!(record["schema"], 2);
    assert_eq!(record["trees"].as_array().unwrap().len(), 1);
}
#[test]
fn combined_tree_interruption_cleanup_and_reopen_hold_completion_until_all_families_verify() {
    let mut partials = 0;
    for stop in 1..30 {
        let f = CombinedFixture::with_tree(true);
        let prepared = f.begin();
        let mut calls = 0;
        let result = f.apply(&prepared, || {
            calls += 1;
            calls != stop
        });
        if result.is_ok() {
            break;
        }
        assert_eq!(f.phase(), RestorePhase::WritersStopped);
        let reopened = f.reopen();
        if let Ok(cleanup) = reopened.partial_cleanup(
            Identifier::new("fresh-tree-cleanup").unwrap(),
            &f.checkpoint,
            &f.fixture.home,
            &f.fixture.account,
            &f.fixture.journal,
        ) {
            partials += 1;
            assert!(f.apply(&reopened, || true).is_err());
            let fresh = f.fixture.publish_assets(
                3,
                cleanup.scope().capture(&f.fixture.home, &f.fixture.account).unwrap(),
                true,
                "fresh-tree-cleanup",
            );
            assert_eq!(
                cleanup
                    .apply(
                        &f.checkpoint,
                        &fresh,
                        &f.fixture.home,
                        &f.fixture.account,
                        &f.fixture.journal,
                        || true
                    )
                    .unwrap(),
                1
            );
            assert_eq!(f.phase(), RestorePhase::WritersStopped);
        }
        assert_eq!(
            f.apply(&f.reopen(), || true).unwrap().phase,
            RestorePhase::ConfigurationRestored
        );
        assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
        assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"original model");
    }
    assert!(partials > 0);
}
#[test]
fn combined_tree_changed_membership_or_preference_failure_never_completes() {
    for tree_failure in [false, true] {
        let f = CombinedFixture::with_tree(true);
        let prepared = f.begin();
        let parent = f.config().parent().unwrap().to_path_buf();
        let backup = parent.with_file_name("saved");
        if tree_failure {
            f.fixture.write(
                &f.model.with_relative(RelativePath::new("fixture/unexpected").unwrap()).unwrap(),
                b"private",
            );
        } else {
            std::fs::rename(&parent, &backup).unwrap();
            std::os::unix::fs::symlink("/tmp", &parent).unwrap();
        }
        assert!(f.apply(&prepared, || true).is_err());
        assert_eq!(f.phase(), RestorePhase::WritersStopped);
        if !tree_failure {
            std::fs::remove_file(&parent).unwrap();
            std::fs::rename(backup, parent).unwrap();
            assert_eq!(
                f.apply(&f.reopen(), || true).unwrap().phase,
                RestorePhase::ConfigurationRestored
            );
        } else {
            assert!(
                PreparedCombinedRestore::reopen(
                    Identifier::new("combined-operation").unwrap(),
                    &f.stores(),
                    f.request()
                )
                .is_err()
            );
            assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
        }
    }
}

#[test]
fn combined_tree_preparation_replays_immutable_checkpoint_and_preserves_live_bytes() {
    for stop in 2..=6 {
        let f = CombinedFixture::with_tree(true);
        let prepared = PreparedCombinedRestore::prepare(f.intent(), f.request()).unwrap();
        let mut step = 0;
        assert!(
            f.with_execution(|execution| prepared.begin_with_boundary(
                &f.stores(),
                execution,
                || {
                    step += 1;
                    step != stop
                }
            ))
            .is_err()
        );
        let reconstructed = PreparedCombinedRestore::prepare(f.intent(), f.request()).unwrap();
        assert_eq!(
            f.with_execution(|execution| reconstructed.resume_preparation(&f.stores(), execution))
                .unwrap()
                .phase,
            RestorePhase::ContentStaged
        );
        assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
        assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"changed model");
        assert_eq!(std::fs::read(f.fixture.file(&cover())).unwrap(), b"changed cover");
        f.reopen();
    }
}
#[test]
fn combined_tree_receipt_downgrade_or_omission_cannot_authorize_replay() {
    for mutation in ["downgrade", "omit", "receipt"] {
        let f = CombinedFixture::with_tree(true);
        f.begin();
        let path = f.fixture.path.join("journal/combined-restore-combined-operation.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match mutation {
            "downgrade" => record["schema"] = 1.into(),
            "omit" => {
                record["schema"] = 1.into();
                record.as_object_mut().unwrap().remove("trees");
            }
            _ => record["trees"][0]["record_sha256"] = "0".repeat(64).into(),
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(
            PreparedCombinedRestore::reopen(
                Identifier::new("combined-operation").unwrap(),
                &f.stores(),
                f.request()
            )
            .is_err()
        );
        assert_eq!(f.phase(), RestorePhase::WritersStopped);
        assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
    }
    let f = CombinedFixture::new();
    f.begin();
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.fixture.path.join("journal/combined-restore-combined-operation.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record["schema"], 1);
    assert!(record.get("trees").is_none());
    f.reopen();
}

#[test]
fn combined_declared_outputs_restore_all_four_families_before_completion_and_reopen() {
    let f = CombinedFixture::with_outputs();
    let prepared = f.begin();
    assert_eq!(f.apply(&prepared, || true).unwrap().phase, RestorePhase::ConfigurationRestored);
    assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
    assert_eq!(std::fs::read(f.fixture.file(&f.model)).unwrap(), b"original model");
    assert_eq!(std::fs::read(f.fixture.file(&cover())).unwrap(), b"original cover");
    assert_eq!(
        std::fs::read(f.fixture.path.join("home/.config/app/theme.css")).unwrap(),
        b"original-theme"
    );
    assert_eq!(
        std::fs::read(f.fixture.path.join("home/.config/new/colors/theme.css")).unwrap(),
        b"missing-parent-theme"
    );
    assert_eq!(
        std::fs::read(f.fixture.path.join("home/.config/app/secret")).unwrap(),
        b"unrelated-credential"
    );
    assert_eq!(f.apply(&f.reopen(), || true).unwrap().phase, RestorePhase::ConfigurationRestored);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.fixture.path.join("journal/combined-restore-combined-operation.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record["schema"], 3);
    assert!(record["outputs"].is_object());
    assert_eq!(record["assets"].as_array().unwrap().len(), 1);
    assert_eq!(record["trees"].as_array().unwrap().len(), 1);
}
#[test]
fn combined_declared_outputs_interrupted_live_replay_withholds_completion_until_preferences_verify()
{
    let mut interrupted = 0;
    for stop in 1..=30 {
        let f = CombinedFixture::with_outputs();
        let prepared = f.begin();
        let mut calls = 0;
        let result = f.apply(&prepared, || {
            let ready =
                std::fs::read_dir(f.fixture.path.join("journal")).unwrap().flatten().any(|entry| {
                    entry.file_name().to_str().is_some_and(|name| {
                        name.starts_with("output-restore-") && name.ends_with("-ready.json")
                    })
                });
            if ready {
                calls += 1;
                calls != stop
            } else {
                true
            }
        });
        if result.is_err() {
            interrupted += 1;
            assert_eq!(f.phase(), RestorePhase::WritersStopped);
        }
        assert_eq!(
            f.apply(&f.reopen(), || true).unwrap().phase,
            RestorePhase::ConfigurationRestored
        );
        assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
        assert_eq!(
            std::fs::read(f.fixture.path.join("home/.config/app/secret")).unwrap(),
            b"unrelated-credential"
        );
    }
    assert!(interrupted >= 12);
}
#[test]
fn combined_declared_outputs_failure_or_receipt_omission_cannot_complete_configuration() {
    for failure in ["output", "preference", "receipt"] {
        let f = CombinedFixture::with_outputs();
        let prepared = f.begin();
        if failure == "output" {
            std::fs::write(f.fixture.path.join("home/.config/app/theme.css"), b"unexpected-output")
                .unwrap();
            assert!(f.apply(&prepared, || true).is_err());
            assert_eq!(f.phase(), RestorePhase::WritersStopped);
            assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
        } else if failure == "preference" {
            let bad = f.fixture.path.join("bad-home");
            std::fs::create_dir(&bad).unwrap();
            let bad = File::open(bad).unwrap();
            f.with_execution(|execution| {
                let destinations =
                    [ConfigurationDestination { root: StateRoot::UserHome, directory: &bad }];
                let bad_execution = CombinedExecution { destinations: &destinations, ..*execution };
                assert!(
                    prepared
                        .apply_owned(&f.stores(), &bad_execution, || true, f.fixture.account.uid)
                        .is_err()
                );
            });
            assert_eq!(f.phase(), RestorePhase::WritersStopped);
            assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
            assert_eq!(
                std::fs::read(f.fixture.path.join("home/.config/app/theme.css")).unwrap(),
                b"original-theme"
            );
            assert_eq!(
                f.apply(&f.reopen(), || true).unwrap().phase,
                RestorePhase::ConfigurationRestored
            );
        } else {
            let path = f.fixture.path.join("journal/combined-restore-combined-operation.json");
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            value.as_object_mut().unwrap().remove("outputs");
            value["schema"] = 2.into();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            assert!(
                PreparedCombinedRestore::reopen(
                    Identifier::new("combined-operation").unwrap(),
                    &f.stores(),
                    f.request()
                )
                .is_err()
            );
            assert!(f.apply(&prepared, || true).is_err());
            assert_eq!(f.phase(), RestorePhase::WritersStopped);
        }
    }
}

#[test]
fn combined_declared_outputs_partial_cleanup_requires_fresh_checkpoint_and_preserves_live_inodes() {
    use std::os::unix::fs::MetadataExt;
    let f = CombinedFixture::with_outputs();
    let prepared = f.begin();
    let stage = f
        .intent()
        .scope()
        .paths()
        .iter()
        .find(|path| {
            path.namespace() == AssetNamespace::OutputStage
                && path.relative().as_str().starts_with(".ryoku-output-stage-")
                && !path.relative().as_str().contains('/')
        })
        .unwrap()
        .clone();
    let live = f.fixture.path.join("home/.config/app/theme.css");
    let live_inode = std::fs::symlink_metadata(&live).unwrap().ino();
    let parent = live.parent().unwrap();
    let parent_inode = std::fs::symlink_metadata(parent).unwrap().ino();
    let partial_exists = || {
        std::fs::read_dir(f.fixture.file(&stage)).is_ok_and(|entries| {
            entries
                .filter_map(std::result::Result::ok)
                .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        })
    };
    assert!(f.apply(&prepared, || !partial_exists()).is_err());
    assert!(partial_exists());
    assert_eq!(f.phase(), RestorePhase::WritersStopped);
    let reopened = f.reopen();
    assert!(f.apply(&reopened, || true).is_err());
    let cleanup = reopened
        .partial_cleanup(
            Identifier::new("fresh-output-cleanup").unwrap(),
            &f.checkpoint,
            &f.fixture.home,
            &f.fixture.account,
            &f.fixture.journal,
        )
        .unwrap();
    let fresh = f.fixture.publish_assets(
        3,
        cleanup.scope().capture(&f.fixture.home, &f.fixture.account).unwrap(),
        true,
        "fresh-output-cleanup",
    );
    for (source, checkpoint) in [(&f.source, &fresh), (&f.checkpoint, &f.checkpoint)] {
        assert!(
            cleanup
                .apply(
                    source,
                    checkpoint,
                    &f.fixture.home,
                    &f.fixture.account,
                    &f.fixture.journal,
                    || true
                )
                .is_err()
        );
    }
    assert!(
        cleanup
            .apply(
                &f.checkpoint,
                &fresh,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
                || false
            )
            .is_err()
    );
    assert_eq!(
        cleanup
            .apply(
                &f.checkpoint,
                &fresh,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
                || true
            )
            .unwrap(),
        1
    );
    assert!(!f.fixture.file(&stage).exists());
    assert_eq!(std::fs::symlink_metadata(&live).unwrap().ino(), live_inode);
    assert_eq!(std::fs::symlink_metadata(parent).unwrap().ino(), parent_inode);
    assert_eq!(std::fs::read(&live).unwrap(), b"changed-theme");
    assert_eq!(std::fs::read(f.config()).unwrap(), CURRENT);
    assert_eq!(f.phase(), RestorePhase::WritersStopped);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.fixture.path.join("journal/combined-cleanup-fresh-output-cleanup.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record["schema"], 3);
    assert!(record["outputs"]["intent"].is_string());
    // The protected original intent survives cleanup, and the original restore
    // can safely create a new stage without replacing unchanged live entries.
    assert_eq!(f.apply(&f.reopen(), || true).unwrap().phase, RestorePhase::ConfigurationRestored);
    assert_eq!(std::fs::read(f.config()).unwrap(), ORIGINAL);
    assert_eq!(std::fs::read(live).unwrap(), b"original-theme");
}

#[test]
fn combined_declared_outputs_cleanup_interruption_retains_bytes_and_allows_fresh_retry() {
    use std::os::unix::fs::MetadataExt;
    let mut interruptions = 0;
    for stop in 1..35 {
        let f = CombinedFixture::with_outputs();
        let prepared = f.begin();
        let stage = f
            .intent()
            .scope()
            .paths()
            .iter()
            .find(|path| {
                path.namespace() == AssetNamespace::OutputStage
                    && path.relative().as_str().starts_with(".ryoku-output-stage-")
                    && !path.relative().as_str().contains('/')
            })
            .unwrap()
            .clone();
        assert!(
            f.apply(&prepared, || !std::fs::read_dir(f.fixture.file(&stage)).is_ok_and(
                |entries| {
                    entries
                        .filter_map(std::result::Result::ok)
                        .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                }
            ))
            .is_err()
        );
        let live = f.fixture.path.join("home/.config/app/theme.css");
        let inode = std::fs::symlink_metadata(&live).unwrap().ino();
        let cleanup = f
            .reopen()
            .partial_cleanup(
                Identifier::new("cleanup-interrupted").unwrap(),
                &f.checkpoint,
                &f.fixture.home,
                &f.fixture.account,
                &f.fixture.journal,
            )
            .unwrap();
        let fresh = f.fixture.publish_assets(
            3,
            cleanup.scope().capture(&f.fixture.home, &f.fixture.account).unwrap(),
            true,
            "cleanup-interrupted",
        );
        let mut steps = 0;
        let result = cleanup.apply(
            &f.checkpoint,
            &fresh,
            &f.fixture.home,
            &f.fixture.account,
            &f.fixture.journal,
            || {
                steps += 1;
                steps != stop
            },
        );
        if result.is_err() {
            interruptions += 1;
        }
        assert_eq!(f.phase(), RestorePhase::WritersStopped);
        assert_eq!(std::fs::symlink_metadata(&live).unwrap().ino(), inode);
        assert_eq!(std::fs::read(&live).unwrap(), b"changed-theme");
        if f.fixture.file(&stage).exists() {
            let retry = f
                .reopen()
                .partial_cleanup(
                    Identifier::new("cleanup-fresh-retry").unwrap(),
                    &f.checkpoint,
                    &f.fixture.home,
                    &f.fixture.account,
                    &f.fixture.journal,
                )
                .unwrap();
            let retry_checkpoint = f.fixture.publish_assets(
                4,
                retry.scope().capture(&f.fixture.home, &f.fixture.account).unwrap(),
                true,
                "cleanup-fresh-retry",
            );
            retry
                .apply(
                    &f.checkpoint,
                    &retry_checkpoint,
                    &f.fixture.home,
                    &f.fixture.account,
                    &f.fixture.journal,
                    || true,
                )
                .unwrap();
        }
        // Every captured partial remains in the verified fresh generation even
        // when its live inode has already moved to a private retained directory.
        let reopened_fresh =
            f.fixture.store.open_verified_transaction(&fresh.manifest().transaction_id).unwrap();
        assert_eq!(reopened_fresh.manifest(), fresh.manifest());
        assert_eq!(
            f.apply(&f.reopen(), || true).unwrap().phase,
            RestorePhase::ConfigurationRestored
        );
    }
    assert!(interruptions >= 12);
}

#[test]
fn combined_declared_outputs_cleanup_refuses_changed_parent_unknown_stage_child_and_ready_receipt()
{
    for case in ["parent", "child", "ready"] {
        let f = CombinedFixture::with_outputs();
        let prepared = f.begin();
        let stage = f
            .intent()
            .scope()
            .paths()
            .iter()
            .find(|path| {
                path.namespace() == AssetNamespace::OutputStage
                    && path.relative().as_str().starts_with(".ryoku-output-stage-")
                    && !path.relative().as_str().contains('/')
            })
            .unwrap()
            .clone();
        if case == "ready" {
            f.apply(&prepared, || true).unwrap();
        } else {
            assert!(f.apply(&prepared, || !f.fixture.file(&stage).exists()).is_err());
            if case == "parent" {
                let parent = f.fixture.path.join("home/.config/app");
                std::fs::rename(&parent, parent.with_file_name("kept-app")).unwrap();
                std::fs::create_dir(&parent).unwrap();
                std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
            } else {
                std::fs::write(f.fixture.file(&stage).join("unrelated"), b"private sibling")
                    .unwrap();
            }
        }
        assert!(
            f.reopen()
                .partial_cleanup(
                    Identifier::new("refused-output-cleanup").unwrap(),
                    &f.checkpoint,
                    &f.fixture.home,
                    &f.fixture.account,
                    &f.fixture.journal
                )
                .is_err()
        );
        assert!(
            !f.fixture.path.join("journal/combined-cleanup-refused-output-cleanup.json").exists()
        );
    }
}
