use super::*;
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    home: File,
    account: CaptureAccount,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryogami-assets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = File::open(&path).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        assert_ne!(uid, 0, "asset fixtures require an ordinary account");
        let account = CaptureAccount {
            uid,
            gid,
            principal: AccountPrincipal::from_account(
                uid,
                gid,
                "asset-fixture",
                path.to_str().unwrap(),
            )
            .unwrap(),
        };
        Self { path, home, account }
    }
    fn write(&self, path: &AssetPath, bytes: &[u8]) {
        let path = self.path.join(path.home_relative().unwrap().as_str());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
fn path(namespace: AssetNamespace, relative: &str) -> AssetPath {
    AssetPath::new(namespace, RelativePath::new(relative).unwrap()).unwrap()
}
fn limits() -> CaptureLimits {
    CaptureLimits { max_paths: 20, max_file_bytes: 1024, max_total_bytes: 10240 }
}

#[test]
fn exact_assets_capture_metadata_absence_and_account_without_unrelated_home_data() {
    let fixture = Fixture::new();
    let model = path(AssetNamespace::SemanticModels, "example/semantic-pack.json");
    let absent = path(AssetNamespace::ProviderContent, &"a".repeat(64));
    fixture.write(&model, b"deterministic model fixture");
    std::fs::write(fixture.path.join("private-sentinel"), b"never-enrolled secret").unwrap();
    let captured =
        capture_assets(&fixture.home, &fixture.account, &[model.clone(), absent.clone()], limits())
            .unwrap();
    assert_eq!(captured.inventory.entries().len(), 2);
    assert!(
        captured
            .inventory
            .entries()
            .iter()
            .any(|e| e.path == absent && e.kind == EntryKind::Absent)
    );
    let actual =
        std::fs::metadata(fixture.path.join(model.home_relative().unwrap().as_str())).unwrap();
    let EntryKind::RegularFile { metadata, .. } =
        &captured.inventory.entries().iter().find(|e| e.path == model).unwrap().kind
    else {
        panic!("missing model record")
    };
    assert_eq!(
        (metadata.mode, metadata.uid, metadata.gid),
        (actual.mode() & 0o7777, actual.uid(), actual.gid())
    );
    assert_eq!(
        metadata.mtime_unix_ns,
        i128::from(actual.mtime()) * 1_000_000_000 + i128::from(actual.mtime_nsec())
    );
    let encoded = serde_json::to_string(&captured.inventory).unwrap();
    assert!(!encoded.contains("private-sentinel") && !encoded.contains("never-enrolled secret"));
    assert_eq!(
        std::fs::read(fixture.path.join("private-sentinel")).unwrap(),
        b"never-enrolled secret"
    );
    assert!(captured.inventory.fingerprint().is_ok());
}

#[test]
fn every_fixed_asset_namespace_preserves_exact_location() {
    let fixture = Fixture::new();
    let declarations = [
        path(AssetNamespace::ProviderContent, &"b".repeat(64)),
        path(AssetNamespace::SemanticModels, "example/model.onnx"),
        path(AssetNamespace::RemotePreviews, "wallhaven/item.png"),
        path(AssetNamespace::RemoteThumbnails, "pexels/item.jpg"),
        path(AssetNamespace::WorkshopContent, "123/project.json"),
    ];
    for declaration in &declarations {
        fixture.write(declaration, b"local fixture");
    }
    let captured =
        capture_assets(&fixture.home, &fixture.account, &declarations, limits()).unwrap();
    assert_eq!(captured.inventory.entries().len(), declarations.len());
    for declared in declarations {
        assert!(captured.inventory.entries().iter().any(|entry| entry.path == declared));
    }
}

#[test]
fn large_asset_streaming_capture_does_not_expand_legacy_preference_limits() {
    let fixture = Fixture::new();
    let asset = path(AssetNamespace::SemanticModels, "fixture/large.onnx");
    fixture.write(&asset, b"");
    let size = 16 * 1024 * 1024 + 1;
    File::options()
        .write(true)
        .open(fixture.path.join(asset.home_relative().unwrap().as_str()))
        .unwrap()
        .set_len(size)
        .unwrap();
    let large = CaptureLimits { max_paths: 1, max_file_bytes: size, max_total_bytes: size + 65536 };
    let captured =
        capture_assets(&fixture.home, &fixture.account, std::slice::from_ref(&asset), large)
            .unwrap();
    assert!(
        matches!(captured.inventory.entries()[0].kind, EntryKind::RegularFile { size: actual, .. } if actual == size)
    );
    assert!(matches!(
        crate::capture::capture_declared(
            &fixture.home,
            StateRoot::StateGeneration,
            &[asset.home_relative().unwrap()],
            large
        ),
        Err(CaptureError::InvalidLimits)
    ));
}

#[test]
fn path_traversal_untyped_provider_names_and_duplicates_refuse() {
    for value in ["../private", "/etc/shadow", "a/../../secret"] {
        assert!(RelativePath::new(value).is_err());
    }
    assert!(
        AssetPath::new(
            AssetNamespace::ProviderContent,
            RelativePath::new("credentials.json").unwrap()
        )
        .is_err()
    );
    assert!(
        AssetPath::new(
            AssetNamespace::SemanticModels,
            RelativePath::new(vec!["x"; 17].join("/")).unwrap()
        )
        .is_err()
    );
    let fixture = Fixture::new();
    let asset = path(AssetNamespace::SemanticModels, "model.onnx");
    assert!(
        capture_assets(&fixture.home, &fixture.account, &[asset.clone(), asset], limits()).is_err()
    );
}

#[test]
fn wrong_account_unsafe_mode_symlink_and_hardlink_never_enroll() {
    let fixture = Fixture::new();
    let asset = path(AssetNamespace::SemanticModels, "model.onnx");
    fixture.write(&asset, b"owned");
    let mut wrong = fixture.account.clone();
    wrong.uid += 1;
    assert!(matches!(
        capture_assets(&fixture.home, &wrong, std::slice::from_ref(&asset), limits()),
        Err(AssetError::Ownership)
    ));
    let file = fixture.path.join(asset.home_relative().unwrap().as_str());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        capture_assets(&fixture.home, &fixture.account, std::slice::from_ref(&asset), limits())
            .is_err()
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::write(fixture.path.join("secret"), b"private").unwrap();
    std::os::unix::fs::symlink(fixture.path.join("secret"), &file).unwrap();
    assert!(
        capture_assets(&fixture.home, &fixture.account, std::slice::from_ref(&asset), limits())
            .is_err()
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::hard_link(fixture.path.join("secret"), &file).unwrap();
    assert!(capture_assets(&fixture.home, &fixture.account, &[asset], limits()).is_err());
    assert_eq!(std::fs::read(fixture.path.join("secret")).unwrap(), b"private");
}

#[test]
fn existing_capture_bounds_and_total_quota_fail_without_writes() {
    let fixture = Fixture::new();
    let asset = path(AssetNamespace::SemanticModels, "model.onnx");
    fixture.write(&asset, b"too big");
    let quota = CaptureLimits { max_paths: 1, max_file_bytes: 3, max_total_bytes: 3 };
    assert!(
        capture_assets(&fixture.home, &fixture.account, std::slice::from_ref(&asset), quota)
            .is_err()
    );
    assert_eq!(
        std::fs::read(fixture.path.join(asset.home_relative().unwrap().as_str())).unwrap(),
        b"too big"
    );
}

#[test]
fn metadata_identity_and_absence_changes_affect_asset_fingerprint() {
    let fixture = Fixture::new();
    let asset = path(AssetNamespace::SemanticModels, "model.onnx");
    let first =
        capture_assets(&fixture.home, &fixture.account, std::slice::from_ref(&asset), limits())
            .unwrap();
    fixture.write(&asset, b"new bytes");
    let second = capture_assets(&fixture.home, &fixture.account, &[asset], limits()).unwrap();
    assert_ne!(first.inventory.fingerprint().unwrap(), second.inventory.fingerprint().unwrap());
    let mut other = fixture.account.clone();
    other.principal =
        AccountPrincipal::from_account(other.uid, other.gid, "other-user", "/home/other-user")
            .unwrap();
    assert!(second.inventory.require_account(&other).is_err());
}

#[test]
fn reviewed_asset_scope_binds_paths_limits_content_absence_and_preferences() {
    let fixture = Fixture::new();
    let model = path(AssetNamespace::SemanticModels, "fixture.onnx");
    let absent = path(AssetNamespace::SemanticModels, "missing.onnx");
    fixture.write(&model, b"local fixture");
    let scope = AssetScope::new(vec![model.clone(), absent.clone()], 1024, 4096).unwrap();
    let captured = scope.capture(&fixture.home, &fixture.account).unwrap();
    let preferences = Sha256::new("a".repeat(64)).unwrap();
    let original = scope.bind_fingerprint(&preferences, captured.inventory()).unwrap();
    assert_eq!(
        AssetScope::new(vec![absent.clone(), model.clone()], 1024, 4096)
            .unwrap()
            .bind_fingerprint(&preferences, captured.inventory())
            .unwrap(),
        original
    );
    let different_limits = AssetScope::new(scope.paths().to_vec(), 2048, 4096).unwrap();
    assert_ne!(
        different_limits.bind_fingerprint(&preferences, captured.inventory()).unwrap(),
        original
    );
    assert_ne!(
        scope
            .bind_fingerprint(&Sha256::new("b".repeat(64)).unwrap(), captured.inventory())
            .unwrap(),
        original
    );
    let missing_path = AssetScope::new(vec![model.clone()], 1024, 4096).unwrap();
    assert!(missing_path.bind_fingerprint(&preferences, captured.inventory()).is_err());
    fixture.write(&absent, b"appeared");
    assert_ne!(
        scope
            .bind_fingerprint(
                &preferences,
                scope.capture(&fixture.home, &fixture.account).unwrap().inventory()
            )
            .unwrap(),
        original
    );
    fixture.write(&model, b"changed");
    assert_ne!(
        scope
            .bind_fingerprint(
                &preferences,
                scope.capture(&fixture.home, &fixture.account).unwrap().inventory()
            )
            .unwrap(),
        original
    );
}

#[test]
fn reviewed_asset_scope_refuses_invalid_bounds_duplicate_and_oversized_current_state() {
    let fixture = Fixture::new();
    let model = path(AssetNamespace::SemanticModels, "fixture.onnx");
    for (paths, file, total) in [
        (vec![], 1, 1),
        (vec![model.clone(), model.clone()], 1, 1),
        (vec![model.clone()], 0, 1),
        (vec![model.clone()], 2, 1),
        (
            vec![model.clone()],
            MAX_CAPTURE_LIMITS.max_file_bytes + 1,
            MAX_CAPTURE_LIMITS.max_total_bytes,
        ),
        (vec![model.clone()], 1, MAX_CAPTURE_LIMITS.max_total_bytes + 1),
    ] {
        assert!(AssetScope::new(paths, file, total).is_err());
    }
    fixture.write(&model, b"larger than quota");
    let scope = AssetScope::new(vec![model], 1, 1).unwrap();
    assert!(scope.capture(&fixture.home, &fixture.account).is_err());
    let mut wire = serde_json::to_value(&scope).unwrap();
    wire["max_total_bytes"] = 0.into();
    let parsed: AssetScope = serde_json::from_value(wire).unwrap();
    assert!(parsed.validate().is_err());
    assert!(parsed.capture(&fixture.home, &fixture.account).is_err());
}

#[test]
fn library_binding_rejects_credential_roots_and_duplicate_physical_paths() {
    let fixture = Fixture::new();
    for root in [".ssh", ".gnupg/keys", ".config", ".local/state", ".aws", ".codex"] {
        assert!(
            AssetPath::for_library(
                RelativePath::new(root).unwrap(),
                RelativePath::new("file.png").unwrap()
            )
            .is_err()
        );
    }
    assert!(
        AssetPath::new(AssetNamespace::ConfiguredLibrary, RelativePath::new("file.png").unwrap())
            .is_err()
    );
    let path = AssetPath::for_library(
        RelativePath::new("Pictures/Wallpapers").unwrap(),
        RelativePath::new("file.png").unwrap(),
    )
    .unwrap();
    let alias = AssetPath::for_library(
        RelativePath::new("Pictures").unwrap(),
        RelativePath::new("Wallpapers/file.png").unwrap(),
    )
    .unwrap();
    fixture.write(&path, b"local fixture");
    assert!(
        capture_assets(&fixture.home, &fixture.account, &[path, alias], MAX_CAPTURE_LIMITS)
            .is_err()
    );
}

#[test]
fn historical_fixed_asset_encoding_is_unchanged_and_rejects_root_injection() {
    let path = path(AssetNamespace::SemanticModels, "model.onnx");
    let original = serde_json::to_string(&path).unwrap();
    assert_eq!(original, r#"{"namespace":"semantic_models","relative":"model.onnx"}"#);
    let mut forged = serde_json::to_value(&path).unwrap();
    forged["library_root"] = "Pictures".into();
    let parsed: AssetPath = serde_json::from_value(forged).unwrap();
    assert!(parsed.home_relative().is_err());
    assert_eq!(serde_json::from_str::<AssetPath>(&original).unwrap(), path);
}

fn output(
    path: &str,
    kind: crate::declared_outputs::OutputKind,
) -> crate::declared_outputs::OutputEntry {
    crate::declared_outputs::OutputEntry { path: RelativePath::new(path).unwrap(), kind }
}
fn output_declaration(
    fixture: &Fixture,
    entries: Vec<crate::declared_outputs::OutputEntry>,
) -> crate::declared_outputs::OutputDeclaration {
    crate::declared_outputs::OutputDeclaration::new(
        &fixture.account,
        Sha256::new("a".repeat(64)).unwrap(),
        entries,
    )
    .unwrap()
}
#[test]
fn declared_outputs_capture_missing_parents_and_independent_directory_effects_without_siblings() {
    use crate::declared_outputs::OutputKind;
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.path.join(".config/app")).unwrap();
    std::fs::write(fixture.path.join(".config/app/credentials"), b"unrelated-private-sentinel")
        .unwrap();
    std::fs::write(fixture.path.join(".config/app/theme.css"), b"old-theme").unwrap();
    let declaration = output_declaration(
        &fixture,
        vec![
            output(".config/app/theme.css", OutputKind::RegularFile),
            output(".config/new/colors/theme.css", OutputKind::RegularFile),
            output(".config/disabled/themes", OutputKind::Directory),
        ],
    );
    let scope = declaration.scope(1024, 8192).unwrap();
    let before = scope.capture(&fixture.home, &fixture.account).unwrap();
    assert_eq!(before.inventory.entries().len(), 8);
    assert!(
        before
            .inventory
            .entries()
            .iter()
            .any(|r| r.path.relative().as_str() == ".config/new" && r.kind == EntryKind::Absent)
    );
    let encoded = serde_json::to_string(&before.inventory).unwrap();
    assert!(!encoded.contains("credentials") && !encoded.contains("unrelated-private-sentinel"));
    assert_eq!(serde_json::from_str::<AssetInventory>(&encoded).unwrap(), before.inventory);
    std::fs::create_dir_all(fixture.path.join(".config/new/colors")).unwrap();
    std::fs::create_dir_all(fixture.path.join(".config/disabled/themes")).unwrap();
    std::fs::write(fixture.path.join(".config/new/colors/theme.css"), b"new-theme").unwrap();
    let after = scope.capture(&fixture.home, &fixture.account).unwrap();
    assert!(
        after
            .inventory
            .entries()
            .iter()
            .any(|r| r.path.relative().as_str() == ".config/disabled/themes"
                && matches!(r.kind, EntryKind::Directory { .. }))
    );
    assert_ne!(before.inventory.fingerprint().unwrap(), after.inventory.fingerprint().unwrap());
    assert!(before.revalidate(&fixture.home).is_err());
    assert_eq!(
        std::fs::read(fixture.path.join(".config/app/credentials")).unwrap(),
        b"unrelated-private-sentinel"
    );
}
#[test]
fn declared_outputs_bind_account_policy_exact_set_and_kind_and_refuse_legacy_enrollment() {
    use crate::declared_outputs::OutputKind;
    let fixture = Fixture::new();
    let declaration = output_declaration(
        &fixture,
        vec![output(".config/app/theme.css", OutputKind::RegularFile)],
    );
    let scope = declaration.scope(1024, 8192).unwrap();
    let captured = scope.capture(&fixture.home, &fixture.account).unwrap();
    let mut account = fixture.account.clone();
    account.gid += 1;
    assert!(scope.capture(&fixture.home, &account).is_err());
    let other = crate::declared_outputs::OutputDeclaration::new(
        &fixture.account,
        Sha256::new("b".repeat(64)).unwrap(),
        vec![output(".config/app/theme.css", OutputKind::RegularFile)],
    )
    .unwrap();
    assert_ne!(declaration.fingerprint().unwrap(), other.fingerprint().unwrap());
    assert!(
        other
            .scope(1024, 8192)
            .unwrap()
            .bind_fingerprint(&Sha256::new("c".repeat(64)).unwrap(), &captured.inventory)
            .is_err()
    );
    assert!(AssetScope::new(scope.paths.clone(), 1024, 8192).is_err());
    assert!(capture_assets(&fixture.home, &fixture.account, &scope.paths, scope.limits()).is_err());
    let value = serde_json::to_value(&captured.inventory).unwrap();
    for field in ["policy_sha256", "uid"] {
        let mut forged = value.clone();
        forged["outputs"][field] = if field == "uid" {
            serde_json::json!(fixture.account.uid + 1)
        } else {
            serde_json::json!("b".repeat(64))
        };
        assert!(serde_json::from_value::<AssetInventory>(forged).unwrap().validate().is_err());
    }
    let mut downgraded = value.clone();
    downgraded["schema_version"] = 1.into();
    assert!(serde_json::from_value::<AssetInventory>(downgraded).unwrap().validate().is_err());
    let mut forged = value;
    forged.as_object_mut().unwrap().remove("outputs");
    assert!(serde_json::from_value::<AssetInventory>(forged).unwrap().validate().is_err());
    assert!(
        crate::declared_outputs::OutputDeclaration::new(
            &fixture.account,
            Sha256::new("a".repeat(64)).unwrap(),
            vec![output(".config/ryoku/ryogami.json", OutputKind::RegularFile)]
        )
        .is_err()
    );
    assert!(
        crate::declared_outputs::OutputDeclaration::new(
            &fixture.account,
            Sha256::new("a".repeat(64)).unwrap(),
            vec![
                output(".config/app", OutputKind::RegularFile),
                output(".config/app/theme.css", OutputKind::RegularFile)
            ]
        )
        .is_err()
    );
}
#[test]
fn declared_outputs_allow_only_exact_internal_link_text_and_do_not_read_link_targets() {
    use crate::declared_outputs::OutputKind;
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.path.join(".cache/app")).unwrap();
    std::fs::write(fixture.path.join(".cache/app/theme.css"), b"target-not-enrolled").unwrap();
    std::os::unix::fs::symlink("theme.css", fixture.path.join(".cache/app/current")).unwrap();
    let declaration = output_declaration(
        &fixture,
        vec![output(
            ".cache/app/current",
            OutputKind::InternalSymlink { targets: vec![RelativePath::new("theme.css").unwrap()] },
        )],
    );
    let scope = declaration.scope(1024, 8192).unwrap();
    let captured = scope.capture(&fixture.home, &fixture.account).unwrap();
    assert!(
        captured
            .inventory
            .entries()
            .iter()
            .any(|r| matches!(&r.kind,EntryKind::Symlink {target,..} if target=="theme.css"))
    );
    assert!(captured.open_payloads(&fixture.home).unwrap().is_empty());
    assert!(!serde_json::to_string(&captured.inventory).unwrap().contains("target-not-enrolled"));
    for target in ["other.css", "../escape", "/tmp/escape"] {
        std::fs::remove_file(fixture.path.join(".cache/app/current")).unwrap();
        std::os::unix::fs::symlink(target, fixture.path.join(".cache/app/current")).unwrap();
        assert!(scope.capture(&fixture.home, &fixture.account).is_err());
    }
    std::fs::remove_file(fixture.path.join(".cache/app/current")).unwrap();
    std::fs::write(fixture.path.join(".cache/app/current"), b"wrong kind").unwrap();
    assert!(scope.capture(&fixture.home, &fixture.account).is_err());
}
#[test]
fn declared_outputs_refuse_shared_parents_oversized_content_and_aliases_without_writes() {
    use crate::declared_outputs::OutputKind;
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.path.join(".config/app")).unwrap();
    std::fs::write(fixture.path.join(".config/app/theme.css"), b"more-than-bound").unwrap();
    let declaration = output_declaration(
        &fixture,
        vec![output(".config/app/theme.css", OutputKind::RegularFile)],
    );
    assert!(declaration.scope(4, 16).unwrap().capture(&fixture.home, &fixture.account).is_err());
    let scope = declaration.scope(1024, 8192).unwrap();
    std::fs::set_permissions(
        fixture.path.join(".config/app"),
        std::fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(scope.capture(&fixture.home, &fixture.account).is_err());
    std::fs::set_permissions(
        fixture.path.join(".config/app"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::rename(fixture.path.join(".config/app"), fixture.path.join(".config/other")).unwrap();
    std::os::unix::fs::symlink("other", fixture.path.join(".config/app")).unwrap();
    assert!(scope.capture(&fixture.home, &fixture.account).is_err());
    assert_eq!(
        std::fs::read(fixture.path.join(".config/other/theme.css")).unwrap(),
        b"more-than-bound"
    );
}

#[test]
fn declared_outputs_enforce_declaration_bound_and_preserve_legacy_inventory_encoding() {
    use crate::declared_outputs::{OutputDeclaration, OutputKind};
    let fixture = Fixture::new();
    let entries = (0..MAX_CAPTURE_LIMITS.max_paths)
        .map(|index| output(&format!(".config/app/file-{index}"), OutputKind::RegularFile))
        .collect();
    assert!(
        OutputDeclaration::new(&fixture.account, Sha256::new("a".repeat(64)).unwrap(), entries)
            .is_err()
    );
    let legacy =
        AssetScope::new(vec![path(AssetNamespace::SemanticModels, "model.onnx")], 1024, 8192)
            .unwrap();
    assert!(!serde_json::to_string(&legacy).unwrap().contains("outputs"));
    let captured = legacy.capture(&fixture.home, &fixture.account).unwrap();
    let encoded = serde_json::to_value(captured.inventory()).unwrap();
    assert_eq!(encoded["schema_version"], 1);
    assert!(encoded.get("outputs").is_none());
    assert!(captured.inventory().output_declaration().is_none());
}
