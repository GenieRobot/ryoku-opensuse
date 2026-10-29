use super::*;
use crate::tree_restore::{TreeObservation, TreeRestoreIntent, TreeRestorePlan};

fn root() -> AssetPath {
    AssetPath::new(AssetNamespace::SemanticModels, RelativePath::new("tree-fixture").unwrap())
        .unwrap()
}
fn child(root: &AssetPath, tail: &str) -> AssetPath {
    root.with_relative(RelativePath::new(format!("{}/{}", root.relative().as_str(), tail)).unwrap())
        .unwrap()
}
fn source(f: &Fixture, present: bool) -> VerifiedGeneration {
    std::fs::create_dir_all(f.file(&root()).parent().unwrap()).unwrap();
    if present {
        f.write(&child(&root(), "nested/model.onnx"), &[7; 192 * 1024]);
        std::fs::set_permissions(f.file(&root()), std::fs::Permissions::from_mode(0o750)).unwrap();
    }
    let scope = crate::ryogami_trees::inspect_scope(
        &f.home,
        &f.account,
        root(),
        vec![],
        MAX_CAPTURE_LIMITS.max_file_bytes,
        MAX_CAPTURE_LIMITS.max_total_bytes,
        || true,
    )
    .unwrap();
    f.capture_scope(1, &scope)
}
fn prepare(
    f: &Fixture,
    source: &VerifiedGeneration,
) -> (TreeRestoreIntent, VerifiedGeneration, TreeRestorePlan) {
    let intent = TreeRestoreIntent::new(
        Identifier::new("tree-operation").unwrap(),
        source,
        &root(),
        &f.home,
        &f.account,
    )
    .unwrap();
    let checkpoint = f.capture_scope(2, intent.scope());
    let plan = TreeRestorePlan::prepare(&intent, source, &checkpoint, &f.home, &f.account).unwrap();
    (intent, checkpoint, plan)
}
#[test]
fn replacement_preserves_original_tree_and_metadata_and_reopens() {
    let f = Fixture::new();
    let source = source(&f, true);
    f.write(&child(&root(), "nested/model.onnx"), b"current");
    f.write(&child(&root(), "obsolete"), b"retain me");
    let sentinel = f.file(&root()).with_file_name("private-sibling");
    std::fs::write(&sentinel, b"private").unwrap();
    let (intent, checkpoint, plan) = prepare(&f, &source);
    let receipt =
        plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
    assert_eq!(
        std::fs::read(f.file(&child(&root(), "nested/model.onnx"))).unwrap(),
        vec![7; 192 * 1024]
    );
    assert!(!f.file(&child(&root(), "obsolete")).exists());
    assert_eq!(
        std::fs::read(f.file(&child(intent.trash(), "nested/model.onnx"))).unwrap(),
        b"current"
    );
    assert_eq!(std::fs::read(f.file(&child(intent.trash(), "obsolete"))).unwrap(), b"retain me");
    assert_eq!(std::fs::read(sentinel).unwrap(), b"private");
    assert!(!f.file(intent.staging()).exists());
    let reopened =
        TreeRestorePlan::reopen(&receipt, &source, &checkpoint, &f.home, &f.account, &f.journal)
            .unwrap();
    assert_eq!(reopened.observe(&f.home, &f.account).unwrap(), TreeObservation::Restored);
    assert_eq!(
        reopened.resume(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap(),
        receipt
    );
    assert!(
        reopened.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).is_err()
    );
}
#[test]
fn absent_tree_restore_and_removal_are_recoverable() {
    for wanted in [false, true] {
        let f = Fixture::new();
        let source = source(&f, wanted);
        if wanted {
            std::fs::remove_dir_all(f.file(&root())).unwrap();
        } else {
            f.write(&child(&root(), "model.onnx"), b"retain installed");
        }
        let (intent, checkpoint, plan) = prepare(&f, &source);
        plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        assert_eq!(f.file(&root()).exists(), wanted);
        assert_eq!(f.file(intent.trash()).exists(), !wanted);
        assert_eq!(plan.observe(&f.home, &f.account).unwrap(), TreeObservation::Restored);
    }
}
#[test]
fn every_interruption_reopens_and_partial_tree_needs_fresh_cleanup() {
    let mut observed = std::collections::BTreeSet::new();
    for stop in 2..24 {
        let f = Fixture::new();
        let source = source(&f, true);
        f.write(&child(&root(), "nested/model.onnx"), b"current");
        let (intent, checkpoint, plan) = prepare(&f, &source);
        let receipt = plan.receipt_identity().unwrap();
        let mut calls = 0;
        let result = plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || {
            calls += 1;
            calls != stop
        });
        if result.is_ok() {
            break;
        }
        let reopened = TreeRestorePlan::reopen(
            &receipt,
            &source,
            &checkpoint,
            &f.home,
            &f.account,
            &f.journal,
        )
        .unwrap();
        let state = reopened.observe(&f.home, &f.account).unwrap();
        observed.insert(format!("{state:?}"));
        if state == TreeObservation::Staged {
            assert!(
                reopened
                    .resume(&source, &checkpoint, &f.home, &f.account, &f.journal, || true)
                    .is_err()
            );
            let cleanup = TreeRestoreIntent::new(
                Identifier::new("cleanup-partial").unwrap(),
                &checkpoint,
                intent.staging(),
                &f.home,
                &f.account,
            )
            .unwrap();
            let fresh = f.capture_scope(3, cleanup.scope());
            let cleanup_plan =
                TreeRestorePlan::prepare(&cleanup, &checkpoint, &fresh, &f.home, &f.account)
                    .unwrap();
            cleanup_plan
                .apply(&checkpoint, &fresh, &f.home, &f.account, &f.journal, || true)
                .unwrap();
            assert!(!f.file(intent.staging()).exists());
            assert!(f.file(cleanup.trash()).is_dir());
        }
        reopened.resume(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        assert_eq!(reopened.observe(&f.home, &f.account).unwrap(), TreeObservation::Restored);
    }
    assert_eq!(
        observed,
        ["Original", "Staged", "Swapped", "Restored"].into_iter().map(str::to_string).collect()
    );
}
#[test]
fn changed_parent_or_membership_refuses_before_journal() {
    for parent_change in [false, true] {
        let f = Fixture::new();
        let source = source(&f, true);
        f.write(&child(&root(), "nested/model.onnx"), b"current");
        let (_, checkpoint, plan) = prepare(&f, &source);
        if parent_change {
            let parent = f.file(&root()).parent().unwrap().to_path_buf();
            let moved = parent.with_file_name("old-parent");
            std::fs::rename(&parent, &moved).unwrap();
            std::fs::create_dir(&parent).unwrap();
            std::fs::rename(moved.join("tree-fixture"), parent.join("tree-fixture")).unwrap();
        } else {
            f.write(&child(&root(), "unexpected"), b"do not delete");
        }
        assert!(
            plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).is_err()
        );
        assert_eq!(std::fs::read_dir(f.path.join("journal")).unwrap().count(), 0);
    }
}

#[test]
fn protected_record_partner_and_account_binding_survive_restart() {
    let f = Fixture::new();
    let source = source(&f, true);
    f.write(&child(&root(), "nested/model.onnx"), b"current");
    let (_, checkpoint, plan) = prepare(&f, &source);
    let receipt = plan.receipt_identity().unwrap();
    let mut calls = 0;
    assert!(
        plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || {
            calls += 1;
            calls < 2
        })
        .is_err()
    );
    assert!(
        TreeRestorePlan::reopen(&receipt, &checkpoint, &source, &f.home, &f.account, &f.journal)
            .is_err()
    );
    let mut other = f.account.clone();
    other.principal =
        AccountPrincipal::from_account(other.uid, other.gid, "other-account", "/home/other")
            .unwrap();
    assert!(
        TreeRestorePlan::reopen(&receipt, &source, &checkpoint, &f.home, &other, &f.journal)
            .is_err()
    );
    let record = f.path.join("journal/tree-restore-tree-operation.json");
    let bytes = std::fs::read(&record).unwrap();
    assert!(!bytes.windows(b"current".len()).any(|window| window == b"current"));
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&record, b"{}").unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        TreeRestorePlan::reopen(&receipt, &source, &checkpoint, &f.home, &f.account, &f.journal)
            .is_err()
    );
}
#[test]
fn unchanged_tree_and_absence_are_verified_noops() {
    for present in [false, true] {
        let f = Fixture::new();
        let source = source(&f, present);
        let (intent, checkpoint, plan) = prepare(&f, &source);
        let receipt =
            plan.apply(&source, &checkpoint, &f.home, &f.account, &f.journal, || true).unwrap();
        assert!(!f.file(intent.staging()).exists());
        assert!(!f.file(intent.trash()).exists());
        TreeRestorePlan::reopen(&receipt, &source, &checkpoint, &f.home, &f.account, &f.journal)
            .unwrap();
    }
}
#[test]
fn staging_inodes_have_account_ownership_at_creation_and_do_not_change_caller() {
    use crate::account_inode::{Kind, create};
    let f = Fixture::new();
    let before = (rustix::process::geteuid(), rustix::process::getegid());
    for (leaf, kind, mode) in [("file", Kind::File, 0o600), ("directory", Kind::Directory, 0o700)] {
        let inode = create(&f.home, leaf, &f.account, kind).unwrap();
        let stat = rustix::fs::fstat(inode).unwrap();
        assert_eq!(
            (stat.st_uid, stat.st_gid, stat.st_mode & 0o7777),
            (f.account.uid, f.account.gid, mode)
        );
        assert!(create(&f.home, leaf, &f.account, kind).is_err());
    }
    for leaf in ["", ".", "..", "nested/file", "null\0leaf"] {
        assert!(create(&f.home, leaf, &f.account, Kind::File).is_err());
    }
    std::os::unix::fs::symlink("file", f.path.join("home/link")).unwrap();
    assert!(create(&f.home, "link", &f.account, Kind::File).is_err());
    assert_eq!((rustix::process::geteuid(), rustix::process::getegid()), before);
    std::fs::set_permissions(f.path.join("home"), std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(create(&f.home, "unsafe-parent", &f.account, Kind::File).is_err());
}
