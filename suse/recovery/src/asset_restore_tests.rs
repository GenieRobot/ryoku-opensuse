use super::*;
use crate::{
    OperationReason, RecoveryManifest, SnapshotLink, StateRoot, VerificationStatus,
    generation::GenerationStore, ryogami_assets::AssetNamespace,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    home: File,
    journal: File,
    account: CaptureAccount,
    store: GenerationStore,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryogami-restore-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        for directory in ["home", "store", "journal"] {
            std::fs::create_dir(path.join(directory)).unwrap();
            std::fs::set_permissions(path.join(directory), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        assert_ne!(uid, 0);
        let home = File::open(path.join("home")).unwrap();
        let journal = File::open(path.join("journal")).unwrap();
        let store =
            GenerationStore::open_for_test(&File::open(path.join("store")).unwrap()).unwrap();
        let account = CaptureAccount {
            uid,
            gid,
            principal: AccountPrincipal::from_account(
                uid,
                gid,
                "restore-fixture",
                path.join("home").to_str().unwrap(),
            )
            .unwrap(),
        };
        Self { path, home, journal, account, store }
    }
    fn file(&self, path: &AssetPath) -> PathBuf {
        self.path.join("home").join(path.home_relative().unwrap().as_str())
    }
    fn write(&self, path: &AssetPath, bytes: &[u8]) {
        let file = self.file(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn capture(&self, number: u64, paths: &[AssetPath]) -> VerifiedGeneration {
        self.capture_preferences(number, paths, true)
    }
    fn capture_preferences(
        &self,
        number: u64,
        paths: &[AssetPath],
        include_preferences: bool,
    ) -> VerifiedGeneration {
        self.capture_preferences_named(number, paths, include_preferences, &format!("tx-{number}"))
    }
    fn capture_preferences_named(
        &self,
        number: u64,
        paths: &[AssetPath],
        include_preferences: bool,
        transaction: &str,
    ) -> VerifiedGeneration {
        let assets = capture_assets(&self.home, &self.account, paths, MAX_CAPTURE_LIMITS).unwrap();
        self.publish_assets(number, assets, include_preferences, transaction)
    }
    fn capture_scope(&self, number: u64, scope: &AssetScope) -> VerifiedGeneration {
        self.publish_assets(
            number,
            scope.capture(&self.home, &self.account).unwrap(),
            true,
            &format!("tx-{number}"),
        )
    }
    fn publish_assets(
        &self,
        number: u64,
        assets: crate::ryogami_assets::CapturedAssets,
        include_preferences: bool,
        transaction: &str,
    ) -> VerifiedGeneration {
        let link = SnapshotLink::new(
            Identifier::new("root").unwrap(),
            (40 + number).to_string(),
            "11111111-1111-1111-1111-111111111111",
        )
        .unwrap();
        let receipt = serde_json::from_value(serde_json::json!({"schema":1,"link":link,"subvolume_id":300+number,
            "subvolume_uuid": format!("22222222-2222-2222-2222-{:012}",number),"origin_root_uuid":"33333333-3333-3333-3333-333333333333"})).unwrap();
        let mut manifest = RecoveryManifest::new(
            Identifier::new(transaction).unwrap(),
            Identifier::new(format!("generation-{number}")).unwrap(),
            None,
            link,
            OperationReason::ProviderChange,
            "2026-10-25T13:00:00Z",
        )
        .unwrap();
        manifest.attach_root_snapshot_receipt(receipt).unwrap();
        manifest.attach_account_principal(self.account.principal.clone());
        let preference = RelativePath::new(if include_preferences {
            ".config/ryoku/ryogami.json"
        } else {
            ".local/state/ryoku/flags.json"
        })
        .unwrap();
        let captured = crate::capture::capture_inventory(
            &self.home,
            StateRoot::UserHome,
            std::slice::from_ref(&preference),
            Default::default(),
        )
        .unwrap();
        let mut payloads = captured.metadata_payloads;
        for record in captured.records {
            if let EntryKind::RegularFile { sha256, .. } = &record.kind {
                payloads.insert(
                    sha256.clone(),
                    std::fs::read(self.path.join("home").join(record.path.as_str())).unwrap(),
                );
            }
            manifest.add_file(record).unwrap();
        }
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        manifest.seal().unwrap();
        let manifest = self
            .store
            .publish_with_assets(manifest, &payloads, &assets, &self.home, || true)
            .unwrap();
        self.store.open_verified_transaction(&manifest.transaction_id).unwrap()
    }
    fn setup(
        &self,
        original: &[u8],
    ) -> (AssetPath, VerifiedGeneration, FileRestoreIntent, VerifiedGeneration, FileRestorePlan)
    {
        let path = AssetPath::new(
            AssetNamespace::SemanticModels,
            RelativePath::new("fixture/model.onnx").unwrap(),
        )
        .unwrap();
        self.write(&path, original);
        let source = self.capture(1, std::slice::from_ref(&path));
        self.write(&path, b"current bytes");
        let intent =
            FileRestoreIntent::new(Identifier::new("restore-model").unwrap(), &source, &path)
                .unwrap();
        let checkpoint = self.capture(2, intent.scope().unwrap().paths());
        let plan =
            FileRestorePlan::prepare(&intent, &source, &checkpoint, &self.home, &self.account)
                .unwrap();
        (path, source, intent, checkpoint, plan)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if path.is_dir() {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
                for entry in std::fs::read_dir(path).unwrap() {
                    writable(&entry.unwrap().path());
                }
            }
        }
        writable(&self.path);
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn exact_restore_journals_before_replace_preserves_source_and_reopens_after_restart() {
    let fixture = Fixture::new();
    let (path, source, intent, checkpoint, plan) = fixture.setup(b"original fixture");
    let sentinel = fixture.file(&path).with_file_name("private-sentinel");
    std::fs::write(&sentinel, b"private").unwrap();
    let receipt = plan
        .apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || true)
        .unwrap();
    assert_eq!(std::fs::read(fixture.file(&path)).unwrap(), b"original fixture");
    assert!(!fixture.file(intent.staging()).exists());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"private");
    drop(plan);
    let reopened = FileRestorePlan::reopen(
        &receipt,
        &source,
        &checkpoint,
        &fixture.home,
        &fixture.account,
        &fixture.journal,
    )
    .unwrap();
    assert_eq!(
        reopened.observe(&fixture.home, &fixture.account).unwrap(),
        RestoreObservation::Restored
    );
    assert!(
        reopened
            .apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || true)
            .is_err()
    );
    assert!(fixture.store.verify_transaction(&checkpoint.manifest().transaction_id).is_ok());
}

#[test]
fn partial_stage_is_truthful_and_cleanup_and_restore_require_fresh_captured_operations() {
    let fixture = Fixture::new();
    let original = vec![7; 256 * 1024];
    let (path, source, intent, checkpoint, plan) = fixture.setup(&original);
    let receipt = plan.receipt_identity().unwrap();
    let mut calls = 0;
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || {
            calls += 1;
            calls < 5
        })
        .is_err()
    );
    assert_eq!(std::fs::read(fixture.file(&path)).unwrap(), b"current bytes");
    assert!(fixture.file(intent.staging()).metadata().unwrap().len() < original.len() as u64);
    let reopened = FileRestorePlan::reopen(
        &receipt,
        &source,
        &checkpoint,
        &fixture.home,
        &fixture.account,
        &fixture.journal,
    )
    .unwrap();
    assert_eq!(
        reopened.observe(&fixture.home, &fixture.account).unwrap(),
        RestoreObservation::Staged
    );
    assert!(
        reopened
            .apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || true)
            .is_err()
    );
    // Cleanup restores the original preimage's explicit absence, under another
    // captured operation. It never blindly unlinks a guessed partial filename.
    let cleanup = FileRestoreIntent::new(
        Identifier::new("cleanup-partial").unwrap(),
        &checkpoint,
        intent.staging(),
    )
    .unwrap();
    let fresh = fixture.capture(3, cleanup.scope().unwrap().paths());
    let cleanup_plan =
        FileRestorePlan::prepare(&cleanup, &checkpoint, &fresh, &fixture.home, &fixture.account)
            .unwrap();
    cleanup_plan
        .apply(&checkpoint, &fresh, &fixture.home, &fixture.account, &fixture.journal, || true)
        .unwrap();
    assert!(!fixture.file(intent.staging()).exists());
    let retry =
        FileRestoreIntent::new(Identifier::new("retry-model").unwrap(), &source, &path).unwrap();
    let fresh = fixture.capture(4, retry.scope().unwrap().paths());
    FileRestorePlan::prepare(&retry, &source, &fresh, &fixture.home, &fixture.account)
        .unwrap()
        .apply(&source, &fresh, &fixture.home, &fixture.account, &fixture.journal, || true)
        .unwrap();
    assert_eq!(std::fs::read(fixture.file(&path)).unwrap(), original);
}

#[test]
fn refusal_and_prewrite_cancellation_leave_home_and_journal_untouched() {
    let fixture = Fixture::new();
    let (path, source, intent, checkpoint, plan) = fixture.setup(b"original");
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || {
            false
        })
        .is_err()
    );
    assert_eq!(std::fs::read_dir(fixture.path.join("journal")).unwrap().count(), 0);
    assert!(
        FileRestorePlan::prepare(&intent, &source, &source, &fixture.home, &fixture.account)
            .is_err()
    );
    fixture.write(&path, b"unexpected new state");
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || {
            true
        })
        .is_err()
    );
    assert_eq!(std::fs::read_dir(fixture.path.join("journal")).unwrap().count(), 0);
    assert_eq!(std::fs::read(fixture.file(&path)).unwrap(), b"unexpected new state");
    assert!(!fixture.file(intent.staging()).exists());
}

#[test]
fn journal_only_and_post_rename_interruptions_have_distinct_restart_observations() {
    for after_rename in [false, true] {
        let fixture = Fixture::new();
        let (path, source, _, checkpoint, plan) = fixture.setup(b"original");
        let receipt = plan.receipt_identity().unwrap();
        let mut calls = 0;
        let result = plan.apply(
            &source,
            &checkpoint,
            &fixture.home,
            &fixture.account,
            &fixture.journal,
            || {
                calls += 1;
                if after_rename {
                    std::fs::read(fixture.file(&path)).unwrap() != b"original"
                } else {
                    calls < 2
                }
            },
        );
        assert!(matches!(result, Err(RestoreError::Interrupted)));
        let reopened = FileRestorePlan::reopen(
            &receipt,
            &source,
            &checkpoint,
            &fixture.home,
            &fixture.account,
            &fixture.journal,
        )
        .unwrap();
        assert_eq!(
            reopened.observe(&fixture.home, &fixture.account).unwrap(),
            if after_rename { RestoreObservation::Restored } else { RestoreObservation::Original }
        );
        assert!(fixture.store.verify_transaction(&checkpoint.manifest().transaction_id).is_ok());
    }
}

#[test]
fn missing_coverage_wrong_account_parent_change_and_stage_injection_cannot_start() {
    let fixture = Fixture::new();
    let (path, source, intent, checkpoint, plan) = fixture.setup(b"original");
    let missing = fixture.capture(3, std::slice::from_ref(&path));
    assert!(
        FileRestorePlan::prepare(&intent, &source, &missing, &fixture.home, &fixture.account)
            .is_err()
    );
    let mut wrong = fixture.account.clone();
    wrong.principal =
        AccountPrincipal::from_account(wrong.uid, wrong.gid, "other", "/home/other").unwrap();
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &wrong, &fixture.journal, || true).is_err()
    );
    let parent = fixture.file(&path).parent().unwrap().to_owned();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || {
            true
        })
        .is_err()
    );
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    fixture.write(intent.staging(), &vec![0; 4096]);
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || {
            true
        })
        .is_err()
    );
    std::fs::remove_file(fixture.file(intent.staging())).unwrap();
    std::os::unix::fs::symlink(fixture.file(&path), fixture.file(intent.staging())).unwrap();
    assert!(
        plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || {
            true
        })
        .is_err()
    );
    assert_eq!(std::fs::read_dir(fixture.path.join("journal")).unwrap().count(), 0);
    assert_eq!(std::fs::read(fixture.file(&path)).unwrap(), b"current bytes");
}

#[test]
fn corrupt_or_missing_protected_record_never_authorizes_restart() {
    let fixture = Fixture::new();
    let (_, source, _, checkpoint, plan) = fixture.setup(b"original");
    let receipt = plan.receipt_identity().unwrap();
    assert!(
        FileRestorePlan::reopen(
            &receipt,
            &source,
            &checkpoint,
            &fixture.home,
            &fixture.account,
            &fixture.journal
        )
        .is_err()
    );
    plan.apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || true)
        .unwrap();
    assert!(
        FileRestorePlan::reopen(
            &receipt,
            &checkpoint,
            &source,
            &fixture.home,
            &fixture.account,
            &fixture.journal
        )
        .is_err()
    );
    let record = fixture
        .path
        .join("journal")
        .join(format!("asset-restore-{}.json", receipt.operation.as_str()));
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&record, b"{}").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        FileRestorePlan::reopen(
            &receipt,
            &source,
            &checkpoint,
            &fixture.home,
            &fixture.account,
            &fixture.journal
        )
        .is_err()
    );
}

#[test]
fn configured_library_restore_keeps_its_exact_bound_root_across_staging_and_reopen() {
    let fixture = Fixture::new();
    let path = AssetPath::for_library(
        RelativePath::new("Pictures/Personal library").unwrap(),
        RelativePath::new("nested/photo.png").unwrap(),
    )
    .unwrap();
    fixture.write(&path, b"original image fixture");
    let source = fixture.capture(1, std::slice::from_ref(&path));
    fixture.write(&path, b"changed image fixture");
    let intent =
        FileRestoreIntent::new(Identifier::new("library-restore").unwrap(), &source, &path)
            .unwrap();
    assert!(
        intent
            .staging()
            .home_relative()
            .unwrap()
            .as_str()
            .starts_with("Pictures/Personal library/nested/.part-")
    );
    let checkpoint = fixture.capture(2, intent.scope().unwrap().paths());
    let plan =
        FileRestorePlan::prepare(&intent, &source, &checkpoint, &fixture.home, &fixture.account)
            .unwrap();
    let receipt = plan
        .apply(&source, &checkpoint, &fixture.home, &fixture.account, &fixture.journal, || true)
        .unwrap();
    assert_eq!(std::fs::read(fixture.file(&path)).unwrap(), b"original image fixture");
    assert_eq!(
        FileRestorePlan::reopen(
            &receipt,
            &source,
            &checkpoint,
            &fixture.home,
            &fixture.account,
            &fixture.journal
        )
        .unwrap()
        .observe(&fixture.home, &fixture.account)
        .unwrap(),
        RestoreObservation::Restored
    );
}

#[test]
fn interrupted_model_reference_stage_is_exactly_enrolled_and_recoverable_without_home_scan() {
    let fixture = Fixture::new();
    let path = AssetPath::new(
        AssetNamespace::ModelReferenceStage,
        RelativePath::new(format!(".ryoku-model-{}.part", "a".repeat(64))).unwrap(),
    )
    .unwrap();
    let source = fixture.capture(1, std::slice::from_ref(&path));
    fixture.write(&path, b"partial private model configuration");
    std::fs::write(fixture.path.join("home/unrelated"), b"private sentinel").unwrap();
    let intent =
        FileRestoreIntent::new(Identifier::new("cleanup-model-reference").unwrap(), &source, &path)
            .unwrap();
    assert_eq!(intent.staging().namespace(), AssetNamespace::ModelReferenceStage);
    let current = fixture.capture(2, intent.scope().unwrap().paths());
    let plan =
        FileRestorePlan::prepare(&intent, &source, &current, &fixture.home, &fixture.account)
            .unwrap();
    let receipt = plan
        .apply(&source, &current, &fixture.home, &fixture.account, &fixture.journal, || true)
        .unwrap();
    assert!(!fixture.file(&path).exists());
    assert_eq!(std::fs::read(fixture.path.join("home/unrelated")).unwrap(), b"private sentinel");
    let reopened = FileRestorePlan::reopen(
        &receipt,
        &source,
        &current,
        &fixture.home,
        &fixture.account,
        &fixture.journal,
    )
    .unwrap();
    assert_eq!(
        reopened.observe(&fixture.home, &fixture.account).unwrap(),
        RestoreObservation::Restored
    );
    for name in
        [".ssh/id_rsa", ".config/ryoku/ryogami.json", ".ryoku-model-private.part", "unrelated"]
    {
        assert!(
            AssetPath::new(AssetNamespace::ModelReferenceStage, RelativePath::new(name).unwrap())
                .is_err()
        );
    }
}

#[test]
fn combined_restore_preparation_keeps_preferences_and_every_asset_bound_to_fresh_checkpoint() {
    use crate::combined_restore::{
        CombinedPreparation, CombinedRestoreIntent, PreparedCombinedRestore,
    };
    use crate::restore::{
        ArtifactStatus, CompatibilityStatus, PreflightEvidence, RestoreScope, SystemSnapshotStatus,
    };
    let fixture = Fixture::new();
    let model = AssetPath::new(
        AssetNamespace::SemanticModels,
        RelativePath::new("fixture/model.onnx").unwrap(),
    )
    .unwrap();
    let stage = AssetPath::new(
        AssetNamespace::ModelReferenceStage,
        RelativePath::new(format!(".ryoku-model-{}.part", "b".repeat(64))).unwrap(),
    )
    .unwrap();
    fixture.write(&model, b"original model");
    let source = fixture.capture(1, &[model.clone(), stage.clone()]);
    let make_intent =
        || CombinedRestoreIntent::new(Identifier::new("tx-2").unwrap(), &source).unwrap();
    let intent = make_intent();
    assert_eq!(intent.scope().paths().len(), 4);
    fixture.write(&model, b"changed model");
    fixture.write(&stage, b"interrupted model settings");
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
    let prepare = |intent, checkpoint, scope, facts: &PreflightEvidence, account| {
        PreparedCombinedRestore::prepare(
            intent,
            CombinedPreparation {
                source: &source,
                checkpoint,
                home: &fixture.home,
                account,
                scope,
                evidence: facts,
            },
        )
    };
    let combined =
        prepare(intent, &checkpoint, RestoreScope::CompleteLinked, &facts, &fixture.account)
            .unwrap();
    assert_eq!(combined.preference_entries(), 1);
    assert_eq!(combined.asset_entries(), 2);
    assert_eq!(combined.source_manifest(), source.manifest().manifest_sha256.as_ref().unwrap());
    assert_eq!(
        combined.checkpoint_manifest(),
        checkpoint.manifest().manifest_sha256.as_ref().unwrap()
    );
    for scope in
        [RestoreScope::ConfigurationOnly, RestoreScope::CompleteLinked, RestoreScope::SystemOnly]
    {
        assert_eq!(
            crate::restore::plan_restore(source.manifest(), scope, &facts),
            Err(crate::restore::PreflightError::AssetRestoreRequired)
        );
    }
    assert!(
        prepare(make_intent(), &checkpoint, RestoreScope::SystemOnly, &facts, &fixture.account)
            .is_err()
    );
    let incomplete_assets = fixture.capture(3, &[model.clone(), stage.clone()]);
    assert!(matches!(
        prepare(
            CombinedRestoreIntent::new(Identifier::new("tx-3").unwrap(), &source).unwrap(),
            &incomplete_assets,
            RestoreScope::CompleteLinked,
            &facts,
            &fixture.account
        ),
        Err(crate::combined_restore::CombinedError::Coverage)
    ));
    let fourth = CombinedRestoreIntent::new(Identifier::new("tx-4").unwrap(), &source).unwrap();
    let incomplete_preferences = fixture.capture_preferences(4, fourth.scope().paths(), false);
    assert!(matches!(
        prepare(
            fourth,
            &incomplete_preferences,
            RestoreScope::CompleteLinked,
            &facts,
            &fixture.account
        ),
        Err(crate::combined_restore::CombinedError::Coverage)
    ));
    assert!(CombinedRestoreIntent::new(source.manifest().transaction_id.clone(), &source).is_err());
    assert!(matches!(
        prepare(
            CombinedRestoreIntent::new(Identifier::new("wrong-restore-id").unwrap(), &source)
                .unwrap(),
            &checkpoint,
            RestoreScope::CompleteLinked,
            &facts,
            &fixture.account
        ),
        Err(crate::combined_restore::CombinedError::Binding)
    ));
    let mut no_checkpoint = facts.clone();
    no_checkpoint.current_state_checkpoint = ArtifactStatus::Missing;
    assert!(
        prepare(
            make_intent(),
            &checkpoint,
            RestoreScope::CompleteLinked,
            &no_checkpoint,
            &fixture.account
        )
        .is_err()
    );
    let mut wrong = fixture.account.clone();
    wrong.principal =
        AccountPrincipal::from_account(wrong.uid, wrong.gid, "another", "/home/another").unwrap();
    assert!(
        prepare(make_intent(), &checkpoint, RestoreScope::CompleteLinked, &facts, &wrong).is_err()
    );
    fixture.write(&model, b"changed after fresh checkpoint");
    assert!(
        prepare(make_intent(), &checkpoint, RestoreScope::CompleteLinked, &facts, &fixture.account)
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(fixture.path.join("journal")).unwrap().count(),
        0,
        "preparation cannot write a journal or target"
    );
}

#[path = "combined_restore_tests.rs"]
mod combined;

#[path = "tree_restore_tests.rs"]
mod trees;

#[test]
fn media_trash_catalog_stage_and_exact_cache_files_restore_and_reopen() {
    for (namespace, relative) in [
        (AssetNamespace::MediaTrash, format!(".ryoku-media-trash-{}-1", "a".repeat(64))),
        (AssetNamespace::CatalogStage, format!(".ryoku-catalog-{}.part", "a".repeat(64))),
        (AssetNamespace::WallpaperCache, "thumbs/fixture.jpg".into()),
    ] {
        let f = Fixture::new();
        let path = AssetPath::new(namespace, RelativePath::new(relative).unwrap()).unwrap();
        f.write(&path, b"source bytes");
        let source = f.capture(1, std::slice::from_ref(&path));
        f.write(&path, b"changed bytes");
        let intent =
            FileRestoreIntent::new(Identifier::new("media-file-restore").unwrap(), &source, &path)
                .unwrap();
        let checkpoint = f.capture(2, intent.scope().unwrap().paths());
        let plan =
            FileRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account).unwrap();
        let receipt =
            plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        assert_eq!(std::fs::read(f.file(&path)).unwrap(), b"source bytes");
        let reopened = FileRestorePlan::reopen(
            &receipt,
            &source,
            &checkpoint,
            &f.home,
            &f.account,
            &f.journal,
        )
        .unwrap();
        assert!(reopened.restored(&f.home, &f.account).unwrap());
        assert!(!f.file(intent.staging()).exists());
    }
}

#[test]
fn declared_outputs_publish_and_reopen_linked_inventory_with_policy_and_no_legacy_restore_bypass() {
    use crate::declared_outputs::{OutputDeclaration, OutputEntry, OutputKind};
    let f = Fixture::new();
    std::fs::create_dir_all(f.path.join("home/.config/app")).unwrap();
    std::fs::write(f.path.join("home/.config/app/theme.css"), b"approved-theme-fixture").unwrap();
    std::fs::write(f.path.join("home/.config/app/credentials"), b"private-not-enrolled").unwrap();
    let outputs = OutputDeclaration::new(
        &f.account,
        Sha256::new("d".repeat(64)).unwrap(),
        vec![
            OutputEntry {
                path: RelativePath::new(".config/app/theme.css").unwrap(),
                kind: OutputKind::RegularFile,
            },
            OutputEntry {
                path: RelativePath::new(".config/disabled/themes").unwrap(),
                kind: OutputKind::Directory,
            },
        ],
    )
    .unwrap();
    let scope = outputs.scope(1024, 8192).unwrap();
    let source = f.capture_scope(80, &scope);
    assert_eq!(source.manifest().schema_version, 3);
    let reopened = f.store.open_verified_transaction(&source.manifest().transaction_id).unwrap();
    assert_eq!(reopened.manifest(), source.manifest());
    let inventory = reopened.manifest().assets().unwrap();
    assert_eq!(
        inventory.fingerprint().unwrap(),
        scope.capture(&f.home, &f.account).unwrap().inventory().fingerprint().unwrap()
    );
    let encoded = serde_json::to_string(reopened.manifest()).unwrap();
    assert!(!encoded.contains("credentials") && !encoded.contains("private-not-enrolled"));
    assert!(
        crate::combined_restore::CombinedRestoreIntent::new(
            Identifier::new("declared-unready").unwrap(),
            &reopened
        )
        .is_ok()
    );
    let file =
        scope.paths().iter().find(|p| p.relative().as_str() == ".config/app/theme.css").unwrap();
    let old_intent =
        FileRestoreIntent::new(Identifier::new("declared-file-unready").unwrap(), &reopened, file)
            .unwrap();
    assert!(old_intent.scope().is_err());
    assert!(std::fs::read_dir(f.path.join("journal")).unwrap().next().is_none());
    assert_eq!(
        std::fs::read(f.path.join("home/.config/app/credentials")).unwrap(),
        b"private-not-enrolled"
    );
}

fn declared_stage_fixture(f: &Fixture) -> AssetScope {
    use crate::declared_outputs::{OutputDeclaration, OutputEntry, OutputKind};
    for directory in [".config/app", ".config/new/colors"] {
        std::fs::create_dir_all(f.path.join("home").join(directory)).unwrap();
    }
    std::fs::write(f.path.join("home/.config/app/theme.css"), b"original-theme").unwrap();
    std::fs::write(f.path.join("home/.config/app/secret"), b"unrelated-credential").unwrap();
    std::fs::write(f.path.join("home/.config/new/colors/theme.css"), b"missing-parent-theme")
        .unwrap();
    std::os::unix::fs::symlink("theme.css", f.path.join("home/.config/app/current")).unwrap();
    OutputDeclaration::new(
        &f.account,
        Sha256::new("e".repeat(64)).unwrap(),
        vec![
            OutputEntry {
                path: RelativePath::new(".config/app/theme.css").unwrap(),
                kind: OutputKind::RegularFile,
            },
            OutputEntry {
                path: RelativePath::new(".config/new/colors/theme.css").unwrap(),
                kind: OutputKind::RegularFile,
            },
            OutputEntry {
                path: RelativePath::new(".config/app/current").unwrap(),
                kind: OutputKind::InternalSymlink {
                    targets: vec![
                        RelativePath::new("other.css").unwrap(),
                        RelativePath::new("theme.css").unwrap(),
                    ],
                },
            },
        ],
    )
    .unwrap()
    .scope(1024, 8192)
    .unwrap()
}
#[test]
fn declared_output_restore_stages_owned_files_links_and_missing_parents_then_reopens_inode_receipt()
{
    use crate::declared_output_restore::{
        OutputRestoreIntent, OutputRestorePlan, PreparedOutputRestore,
    };
    let f = Fixture::new();
    let scope = declared_stage_fixture(&f);
    let source = f.capture_scope(90, &scope);
    std::fs::write(f.path.join("home/.config/app/theme.css"), b"current-theme").unwrap();
    std::fs::remove_dir_all(f.path.join("home/.config/new")).unwrap();
    let operation = Identifier::new("declared-stage").unwrap();
    let intent = OutputRestoreIntent::new(operation.clone(), &source).unwrap();
    let checkpoint = f.capture_scope(91, intent.scope());
    let plan =
        OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account).unwrap();
    let prepared =
        plan.stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    prepared.verify_unpublished(&f.home, &f.account).unwrap();
    let reopened = PreparedOutputRestore::reopen(
        &source,
        &checkpoint,
        &f.account,
        &f.journal,
        prepared.receipt(),
    )
    .unwrap();
    reopened.verify_unpublished(&f.home, &f.account).unwrap();
    let recovered =
        OutputRestorePlan::reopen(&operation, &source, &checkpoint, &f.account, &f.journal)
            .unwrap()
            .reopen_staged(&source, &checkpoint, &f.account, &f.journal)
            .unwrap();
    assert_eq!(recovered.receipt(), prepared.receipt());
    assert_eq!(std::fs::read(f.path.join("home/.config/app/theme.css")).unwrap(), b"current-theme");
    assert!(!f.path.join("home/.config/new").exists());
    assert_eq!(
        std::fs::read(f.path.join("home/.config/app/secret")).unwrap(),
        b"unrelated-credential"
    );
    let stage = intent
        .scope()
        .paths()
        .iter()
        .find(|path| {
            path.namespace() == AssetNamespace::OutputStage
                && path.relative().as_str().starts_with(".ryoku-output-stage-")
                && !path.relative().as_str().contains('/')
        })
        .unwrap();
    std::fs::write(f.file(stage).join("injected"), b"unrelated-stage-file").unwrap();
    assert!(reopened.verify_unpublished(&f.home, &f.account).is_err());
    assert_eq!(std::fs::read(f.file(stage).join("injected")).unwrap(), b"unrelated-stage-file");
}
#[test]
fn declared_output_restore_stage_interruption_never_changes_live_outputs_or_adopts_partial_stage() {
    use crate::declared_output_restore::{OutputRestoreIntent, OutputRestorePlan};
    let mut interrupted = 0;
    for stop in 1..=24 {
        let f = Fixture::new();
        let scope = declared_stage_fixture(&f);
        let source = f.capture_scope(92, &scope);
        std::fs::write(f.path.join("home/.config/app/theme.css"), b"current-theme").unwrap();
        std::fs::remove_dir_all(f.path.join("home/.config/new")).unwrap();
        let operation = Identifier::new(format!("declared-stage-{stop}")).unwrap();
        let intent = OutputRestoreIntent::new(operation.clone(), &source).unwrap();
        let checkpoint = f.capture_scope(93, intent.scope());
        let plan =
            OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account).unwrap();
        let mut steps = 0;
        let result = plan.stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || {
            steps += 1;
            steps != stop
        });
        match result {
            Ok(prepared) => prepared.verify_unpublished(&f.home, &f.account).unwrap(),
            Err(_) => interrupted += 1,
        }
        assert_eq!(
            std::fs::read(f.path.join("home/.config/app/theme.css")).unwrap(),
            b"current-theme"
        );
        assert!(!f.path.join("home/.config/new").exists());
        assert_eq!(
            std::fs::read(f.path.join("home/.config/app/secret")).unwrap(),
            b"unrelated-credential"
        );
        if let Ok(reopened) =
            OutputRestorePlan::reopen(&operation, &source, &checkpoint, &f.account, &f.journal)
        {
            if let Ok(ready) = reopened.reopen_staged(&source, &checkpoint, &f.account, &f.journal)
            {
                ready.verify_unpublished(&f.home, &f.account).unwrap();
            }
            let has_stage = intent.scope().paths().iter().any(|path| {
                path.namespace() == AssetNamespace::OutputStage
                    && std::fs::symlink_metadata(f.file(path)).is_ok()
            });
            let retry =
                reopened.stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true);
            if has_stage {
                assert!(retry.is_err());
            } else {
                retry.unwrap().verify_unpublished(&f.home, &f.account).unwrap();
            }
        }
    }
    assert!(interrupted >= 12);
}

#[test]
fn declared_output_restore_live_apply_recreates_parents_replaces_links_and_retains_originals() {
    use crate::declared_output_restore::{
        OutputRestoreIntent, OutputRestorePlan, PreparedOutputRestore,
    };
    let f = Fixture::new();
    let scope = declared_stage_fixture(&f);
    let source = f.capture_scope(94, &scope);
    std::fs::write(f.path.join("home/.config/app/theme.css"), b"current-theme").unwrap();
    std::fs::remove_file(f.path.join("home/.config/app/current")).unwrap();
    std::os::unix::fs::symlink("other.css", f.path.join("home/.config/app/current")).unwrap();
    std::fs::remove_dir_all(f.path.join("home/.config/new")).unwrap();
    let intent =
        OutputRestoreIntent::new(Identifier::new("declared-live").unwrap(), &source).unwrap();
    let checkpoint = f.capture_scope(95, intent.scope());
    let plan =
        OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account).unwrap();
    let prepared =
        plan.stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    let receipt =
        prepared.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    let reopened =
        PreparedOutputRestore::reopen(&source, &checkpoint, &f.account, &f.journal, &receipt)
            .unwrap();
    assert!(reopened.restored(&f.home, &f.account).unwrap());
    reopened.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    assert_eq!(
        std::fs::read(f.path.join("home/.config/app/theme.css")).unwrap(),
        b"original-theme"
    );
    assert_eq!(
        std::fs::read(f.path.join("home/.config/new/colors/theme.css")).unwrap(),
        b"missing-parent-theme"
    );
    assert_eq!(
        std::fs::read_link(f.path.join("home/.config/app/current")).unwrap(),
        PathBuf::from("theme.css")
    );
    assert_eq!(
        std::fs::read(f.path.join("home/.config/app/secret")).unwrap(),
        b"unrelated-credential"
    );
    let retained = intent
        .scope()
        .paths()
        .iter()
        .filter(|path| {
            path.namespace() == AssetNamespace::OutputStage
                && path.relative().as_str().starts_with(".ryoku-output-trash-")
        })
        .filter_map(|path| std::fs::read(f.file(path)).ok())
        .collect::<Vec<_>>();
    assert!(retained.contains(&b"current-theme".to_vec()));
}
#[test]
fn declared_output_restore_live_interruptions_reopen_through_created_parents_and_exchanged_files() {
    use crate::declared_output_restore::{
        OutputRestoreIntent, OutputRestorePlan, PreparedOutputRestore,
    };
    let mut interrupted = 0;
    for stop in 1..=30 {
        let f = Fixture::new();
        let scope = declared_stage_fixture(&f);
        let source = f.capture_scope(96, &scope);
        std::fs::write(f.path.join("home/.config/app/theme.css"), b"current-theme").unwrap();
        std::fs::remove_dir_all(f.path.join("home/.config/new")).unwrap();
        let intent = OutputRestoreIntent::new(
            Identifier::new(format!("declared-live-{stop}")).unwrap(),
            &source,
        )
        .unwrap();
        let checkpoint = f.capture_scope(97, intent.scope());
        let prepared =
            OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account)
                .unwrap()
                .stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
                .unwrap();
        let mut steps = 0;
        if prepared
            .apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || {
                steps += 1;
                steps != stop
            })
            .is_err()
        {
            interrupted += 1;
        }
        let reopened = PreparedOutputRestore::reopen(
            &source,
            &checkpoint,
            &f.account,
            &f.journal,
            prepared.receipt(),
        )
        .unwrap();
        reopened.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        assert!(reopened.restored(&f.home, &f.account).unwrap());
        assert_eq!(
            std::fs::read(f.path.join("home/.config/app/secret")).unwrap(),
            b"unrelated-credential"
        );
    }
    assert!(interrupted >= 12);
}
#[test]
fn declared_output_restore_removes_only_declared_new_directories_and_refuses_unrelated_children() {
    use crate::declared_output_restore::{OutputRestoreIntent, OutputRestorePlan};
    for unexpected in [false, true] {
        let f = Fixture::new();
        let scope = declared_stage_fixture(&f);
        std::fs::remove_dir_all(f.path.join("home/.config/new")).unwrap();
        let source = f.capture_scope(98, &scope);
        std::fs::create_dir_all(f.path.join("home/.config/new/colors")).unwrap();
        std::fs::write(f.path.join("home/.config/new/colors/theme.css"), b"created-theme").unwrap();
        if unexpected {
            std::fs::write(f.path.join("home/.config/new/secret"), b"unrelated-new-sentinel")
                .unwrap();
        }
        let intent =
            OutputRestoreIntent::new(Identifier::new("declared-remove-parents").unwrap(), &source)
                .unwrap();
        let checkpoint = f.capture_scope(99, intent.scope());
        let plan = OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account);
        if unexpected {
            assert!(plan.is_err());
            assert!(std::fs::read_dir(f.path.join("journal")).unwrap().next().is_none());
            assert_eq!(
                std::fs::read(f.path.join("home/.config/new/secret")).unwrap(),
                b"unrelated-new-sentinel"
            );
            assert_eq!(
                std::fs::read(f.path.join("home/.config/new/colors/theme.css")).unwrap(),
                b"created-theme"
            );
        } else {
            let prepared = plan
                .unwrap()
                .stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
                .unwrap();
            prepared.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
            assert!(prepared.restored(&f.home, &f.account).unwrap());
            assert!(!f.path.join("home/.config/new").exists());
        }
    }
}

#[test]
fn declared_output_restore_changed_parent_or_omitted_ready_inode_refuses_without_live_changes() {
    use crate::declared_output_restore::{
        OutputRestoreIntent, OutputRestorePlan, PreparedOutputRestore,
    };
    for changed_parent in [false, true] {
        let f = Fixture::new();
        let scope = declared_stage_fixture(&f);
        let source = f.capture_scope(100, &scope);
        std::fs::write(f.path.join("home/.config/app/theme.css"), b"current-theme").unwrap();
        let operation = Identifier::new("declared-ready-refusal").unwrap();
        let intent = OutputRestoreIntent::new(operation.clone(), &source).unwrap();
        let checkpoint = f.capture_scope(101, intent.scope());
        let plan =
            OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account).unwrap();
        let prepared =
            plan.stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        let mut wrong = f.account.clone();
        wrong.gid += 1;
        assert!(
            PreparedOutputRestore::reopen(
                &source,
                &checkpoint,
                &wrong,
                &f.journal,
                prepared.receipt()
            )
            .is_err()
        );
        if changed_parent {
            std::fs::rename(
                f.path.join("home/.config/app"),
                f.path.join("home/.config/retained-app"),
            )
            .unwrap();
            std::fs::create_dir(f.path.join("home/.config/app")).unwrap();
            std::fs::write(
                f.path.join("home/.config/app/theme.css"),
                b"replacement-parent-sentinel",
            )
            .unwrap();
            assert!(
                prepared
                    .apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
                    .is_err()
            );
            assert_eq!(
                std::fs::read(f.path.join("home/.config/app/theme.css")).unwrap(),
                b"replacement-parent-sentinel"
            );
            assert_eq!(
                std::fs::read(f.path.join("home/.config/retained-app/theme.css")).unwrap(),
                b"current-theme"
            );
        } else {
            let path = f.path.join("journal/output-restore-declared-ready-refusal-ready.json");
            let mut ready: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            ready["staged"].as_array_mut().unwrap().pop();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::write(&path, serde_json::to_vec(&ready).unwrap()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            assert!(
                PreparedOutputRestore::reopen(
                    &source,
                    &checkpoint,
                    &f.account,
                    &f.journal,
                    prepared.receipt()
                )
                .is_err()
            );
            assert!(plan.reopen_staged(&source, &checkpoint, &f.account, &f.journal).is_err());
            assert!(
                prepared
                    .apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
                    .is_err()
            );
            assert_eq!(
                std::fs::read(f.path.join("home/.config/app/theme.css")).unwrap(),
                b"current-theme"
            );
        }
    }
}
#[test]
fn declared_output_restore_directory_removal_interruptions_reopen_without_unrelated_losses() {
    use crate::declared_output_restore::{
        OutputRestoreIntent, OutputRestorePlan, PreparedOutputRestore,
    };
    let mut interrupted = 0;
    for stop in 1..=24 {
        let f = Fixture::new();
        let scope = declared_stage_fixture(&f);
        std::fs::remove_dir_all(f.path.join("home/.config/new")).unwrap();
        let source = f.capture_scope(102, &scope);
        std::fs::create_dir_all(f.path.join("home/.config/new/colors")).unwrap();
        std::fs::write(f.path.join("home/.config/new/colors/theme.css"), b"created-theme").unwrap();
        let intent = OutputRestoreIntent::new(
            Identifier::new(format!("declared-remove-{stop}")).unwrap(),
            &source,
        )
        .unwrap();
        let checkpoint = f.capture_scope(103, intent.scope());
        let prepared =
            OutputRestorePlan::prepare(&intent, &source, &checkpoint, &f.home, &f.account)
                .unwrap()
                .stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
                .unwrap();
        let mut steps = 0;
        if prepared
            .apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || {
                steps += 1;
                steps != stop
            })
            .is_err()
        {
            interrupted += 1;
        }
        let reopened = PreparedOutputRestore::reopen(
            &source,
            &checkpoint,
            &f.account,
            &f.journal,
            prepared.receipt(),
        )
        .unwrap();
        reopened.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        assert!(reopened.restored(&f.home, &f.account).unwrap());
        assert!(!f.path.join("home/.config/new").exists());
        assert_eq!(
            std::fs::read(f.path.join("home/.config/app/secret")).unwrap(),
            b"unrelated-credential"
        );
    }
    assert!(interrupted >= 12);
}

#[test]
fn declared_output_restore_preserves_and_restores_prior_generated_stage_and_trash_entries() {
    use crate::declared_output_restore::{OutputRestoreIntent, OutputRestorePlan};
    let f = Fixture::new();
    let scope = declared_stage_fixture(&f);
    let source = f.capture_scope(104, &scope);
    std::fs::write(f.path.join("home/.config/app/theme.css"), b"retained-original").unwrap();
    let first =
        OutputRestoreIntent::new(Identifier::new("declared-first").unwrap(), &source).unwrap();
    let checkpoint = f.capture_scope(105, first.scope());
    let prepared = OutputRestorePlan::prepare(&first, &source, &checkpoint, &f.home, &f.account)
        .unwrap()
        .stage(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
        .unwrap();
    prepared.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    let post = f.capture_scope(106, first.scope());
    let retained = first
        .scope()
        .paths()
        .iter()
        .find(|path| {
            path.namespace() == AssetNamespace::OutputStage
                && path.relative().as_str().starts_with(".ryoku-output-trash-")
                && std::fs::read(f.file(path)).is_ok_and(|bytes| bytes == b"retained-original")
        })
        .unwrap();
    std::fs::write(f.file(retained), b"changed-retained").unwrap();
    std::fs::write(f.path.join("home/.config/app/theme.css"), b"changed-theme").unwrap();
    let second =
        OutputRestoreIntent::new(Identifier::new("declared-second").unwrap(), &post).unwrap();
    let checkpoint = f.capture_scope(107, second.scope());
    let prepared = OutputRestorePlan::prepare(&second, &post, &checkpoint, &f.home, &f.account)
        .unwrap()
        .stage(&post, &checkpoint, &f.home, &f.account, &f.journal, || true)
        .unwrap();
    prepared.apply(&post, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    assert!(prepared.restored(&f.home, &f.account).unwrap());
    assert_eq!(std::fs::read(f.file(retained)).unwrap(), b"retained-original");
    assert_eq!(
        std::fs::read(f.path.join("home/.config/app/theme.css")).unwrap(),
        b"original-theme"
    );
}
