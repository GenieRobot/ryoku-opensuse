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
        let source = fixture.capture(1, std::slice::from_ref(&model));
        fixture.write(&model, b"changed model");
        std::fs::write(&config, CURRENT).unwrap();
        let intent =
            CombinedRestoreIntent::new(Identifier::new("combined-operation").unwrap(), &source)
                .unwrap();
        let checkpoint = fixture.capture(2, intent.scope().paths());
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
        CombinedRestoreIntent::new(Identifier::new("combined-operation").unwrap(), &self.source)
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
    let cleanup = FileRestoreIntent::new(
        Identifier::new("fresh-combined-cleanup").unwrap(),
        &f.checkpoint,
        &stage,
    )
    .unwrap();
    let fresh = f.fixture.capture(3, cleanup.scope().unwrap().paths());
    let cleanup = FileRestorePlan::prepare(
        &cleanup,
        &f.checkpoint,
        &fresh,
        &f.fixture.home,
        &f.fixture.account,
    )
    .unwrap();
    cleanup
        .apply(
            &f.checkpoint,
            &fresh,
            &f.fixture.home,
            &f.fixture.account,
            &f.fixture.journal,
            || true,
        )
        .unwrap();
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
