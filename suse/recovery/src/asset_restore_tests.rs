use super::*;
use crate::{
    FileRecord, OperationReason, RecoveryManifest, SnapshotLink, StateRoot, VerificationStatus,
    generation::{GenerationStore, Payloads},
    ryogami_assets::AssetNamespace,
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
        let assets = capture_assets(&self.home, &self.account, paths, MAX_CAPTURE_LIMITS).unwrap();
        let link = SnapshotLink::new(
            Identifier::new("root").unwrap(),
            (40 + number).to_string(),
            "11111111-1111-1111-1111-111111111111",
        )
        .unwrap();
        let receipt = serde_json::from_value(serde_json::json!({"schema":1,"link":link,"subvolume_id":300+number,
            "subvolume_uuid": format!("22222222-2222-2222-2222-{:012}",number),"origin_root_uuid":"33333333-3333-3333-3333-333333333333"})).unwrap();
        let mut manifest = RecoveryManifest::new(
            Identifier::new(format!("tx-{number}")).unwrap(),
            Identifier::new(format!("generation-{number}")).unwrap(),
            None,
            link,
            OperationReason::ProviderChange,
            "2026-10-25T13:00:00Z",
        )
        .unwrap();
        manifest.attach_root_snapshot_receipt(receipt).unwrap();
        manifest.attach_account_principal(self.account.principal.clone());
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserHome,
                    RelativePath::new(".local/state/ryoku/flags.json").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        manifest.seal().unwrap();
        let manifest = self
            .store
            .publish_with_assets(manifest, &Payloads::new(), &assets, &self.home, || true)
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
