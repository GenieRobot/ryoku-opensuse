use super::*;
use crate::{AccountPrincipal, ryogami_assets::capture_assets};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    home: File,
    account: CaptureAccount,
    root: AssetPath,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryogami-tree-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        let account = CaptureAccount {
            uid,
            gid,
            principal: AccountPrincipal::from_account(
                uid,
                gid,
                "tree-fixture",
                path.to_str().unwrap(),
            )
            .unwrap(),
        };
        let root =
            AssetPath::new(AssetNamespace::SemanticModels, RelativePath::new("fixture").unwrap())
                .unwrap();
        Self { home: File::open(&path).unwrap(), path, account, root }
    }
    fn file(&self, tail: &str) -> PathBuf {
        self.path.join(self.root.home_relative().unwrap().as_str()).join(tail)
    }
    fn write(&self, tail: &str, bytes: &[u8]) {
        let path = self.file(tail);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn scope(&self) -> AssetScope {
        inspect_scope(&self.home, &self.account, self.root.clone(), vec![], 1024, 4096, || true)
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
#[test]
fn complete_tree_capture_retains_directories_files_and_explicit_future_absence() {
    let f = Fixture::new();
    f.write("manifest.json", b"{}");
    f.write("nested/model.onnx", b"fixture");
    std::fs::create_dir(f.path.join(".ssh")).unwrap();
    std::fs::write(f.path.join(".ssh/unrelated-private"), b"not-enrolled").unwrap();
    let future = f.root.with_relative(RelativePath::new("fixture/future.json").unwrap()).unwrap();
    let scope =
        inspect_scope(&f.home, &f.account, f.root.clone(), vec![future], 1024, 4096, || true)
            .unwrap();
    assert_eq!(scope.paths().len(), 5);
    assert_eq!(scope.trees(), std::slice::from_ref(&f.root));
    let captured = scope.capture(&f.home, &f.account).unwrap();
    assert_eq!(captured.inventory().trees(), scope.trees());
    let bytes = serde_json::to_vec(captured.inventory()).unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["schema_version"], 2);
    assert!(!String::from_utf8(bytes).unwrap().contains("unrelated-private"));
    captured.revalidate(&f.home).unwrap();
    let hash = scope
        .bind_fingerprint(&crate::Sha256::new("a".repeat(64)).unwrap(), captured.inventory())
        .unwrap();
    f.write("nested/model.onnx", b"changed");
    assert!(captured.revalidate(&f.home).is_err());
    let new = scope.capture(&f.home, &f.account).unwrap();
    assert_ne!(
        hash,
        scope
            .bind_fingerprint(&crate::Sha256::new("a".repeat(64)).unwrap(), new.inventory())
            .unwrap()
    );
}
#[test]
fn complete_tree_detects_added_children_and_missing_directory_topology() {
    let f = Fixture::new();
    f.write("nested/model.onnx", b"model");
    let scope = f.scope();
    let captured = scope.capture(&f.home, &f.account).unwrap();
    f.write("new-unplanned-file", b"new");
    assert!(scope.capture(&f.home, &f.account).is_err());
    assert!(captured.revalidate(&f.home).is_err());
    std::fs::remove_file(f.file("new-unplanned-file")).unwrap();
    let mut value = serde_json::to_value(captured.inventory()).unwrap();
    value["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["path"]["relative"] != "fixture/nested");
    let bad: AssetInventory = serde_json::from_value(value).unwrap();
    assert!(bad.validate().is_err());
    let mut value = serde_json::to_value(captured.inventory()).unwrap();
    value["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["path"]["relative"] != "fixture/nested/model.onnx");
    let partial: AssetInventory = serde_json::from_value(value).unwrap();
    partial.validate().unwrap();
    assert!(verify_inventory(&f.home, &f.account, &partial).is_err());
}
#[test]
fn absent_tree_and_legacy_flat_asset_bytes_have_distinct_explicit_claims() {
    let f = Fixture::new();
    let scope = f.scope();
    assert_eq!(scope.paths(), std::slice::from_ref(&f.root));
    let captured = scope.capture(&f.home, &f.account).unwrap();
    assert!(matches!(captured.inventory().entries()[0].kind, EntryKind::Absent));
    let flat =
        capture_assets(&f.home, &f.account, std::slice::from_ref(&f.root), MAX_CAPTURE_LIMITS)
            .unwrap();
    let bytes = serde_json::to_vec(flat.inventory()).unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains("trees"));
    let mut malformed = serde_json::to_value(captured.inventory()).unwrap();
    malformed["schema_version"] = 1.into();
    assert!(serde_json::from_value::<AssetInventory>(malformed).unwrap().validate().is_err());
    f.write("new/model", b"new");
    assert!(captured.revalidate(&f.home).is_err());
}
#[test]
fn tree_enrollment_refuses_links_permissions_owner_and_size_without_writes() {
    for fault in ["symlink", "hardlink", "executable", "directory-mode", "size", "total", "owner"] {
        let f = Fixture::new();
        f.write("model", b"12345");
        match fault {
            "symlink" => std::os::unix::fs::symlink("/etc", f.file("link")).unwrap(),
            "hardlink" => std::fs::hard_link(f.file("model"), f.file("alias")).unwrap(),
            "executable" => {
                std::fs::set_permissions(f.file("model"), std::fs::Permissions::from_mode(0o700))
                    .unwrap()
            }
            "directory-mode" => {
                std::fs::set_permissions(f.file(""), std::fs::Permissions::from_mode(0o777))
                    .unwrap()
            }
            "total" => f.write("other", b"12345"),
            _ => (),
        }
        let mut account = f.account.clone();
        if fault == "owner" {
            account.uid += 1;
        }
        let (file, total) = match fault {
            "size" => (4, 20),
            "total" => (5, 9),
            _ => (1024, 4096),
        };
        assert!(
            inspect_scope(&f.home, &account, f.root.clone(), vec![], file, total, || true).is_err(),
            "{fault}"
        );
        assert_eq!(std::fs::read(f.file("model")).unwrap(), b"12345");
    }
}
#[test]
fn tree_cancellation_race_and_overlapping_root_claims_refuse() {
    let f = Fixture::new();
    f.write("nested/model", b"model");
    assert!(matches!(
        inspect_scope(&f.home, &f.account, f.root.clone(), vec![], 1024, 4096, || false),
        Err(AssetError::Interrupted)
    ));
    let mut calls = 0;
    assert!(
        inspect_scope(&f.home, &f.account, f.root.clone(), vec![], 1024, 4096, || {
            calls += 1;
            if calls == 3 {
                f.write("concurrent-child", b"new");
            }
            true
        })
        .is_err()
    );
    let scope = f.scope();
    let nested = f.root.with_relative(RelativePath::new("fixture/nested").unwrap()).unwrap();
    assert!(scope.with_trees(vec![f.root.clone(), nested]).is_err());
    let unsupported = AssetPath::new(
        AssetNamespace::ModelReferenceStage,
        RelativePath::new(format!(".ryoku-model-{}.part", "a".repeat(64))).unwrap(),
    )
    .unwrap();
    assert!(inspect_scope(&f.home, &f.account, unsupported, vec![], 1024, 4096, || true).is_err());
}

#[test]
fn complete_tree_count_and_depth_are_bounded_before_payload_capture() {
    let f = Fixture::new();
    for index in 0..MAX_CAPTURE_LIMITS.max_paths {
        f.write(&format!("node-{index}"), b"");
    }
    assert!(matches!(
        inspect_scope(&f.home, &f.account, f.root.clone(), vec![], 1024, 4096, || true),
        Err(AssetError::Bounds)
    ));
    let nested = Fixture::new();
    nested.write(&format!("{}/file", vec!["nested"; 16].join("/")), b"data");
    assert!(matches!(
        inspect_scope(
            &nested.home,
            &nested.account,
            nested.root.clone(),
            vec![],
            1024,
            4096,
            || true
        ),
        Err(AssetError::Path)
    ));
}
