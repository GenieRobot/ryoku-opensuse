use super::*;
use crate::{
    Identifier, ManifestError, RecoveryManifest, ValidationError, VerificationStatus,
    generation::{GenerationStore, Payloads},
    restore::{
        self, ArtifactStatus, CompatibilityStatus, PreflightError, PreflightEvidence, RestoreScope,
        SystemSnapshotStatus,
    },
};
use std::{
    fs::File,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    sync::atomic::{AtomicU64, Ordering},
};
const LEGACY1: &[u8] = include_bytes!("../tests/fixtures/issue176-logical/legacy-v1.json");
const LEGACY2: &[u8] = include_bytes!("../tests/fixtures/issue176-logical/legacy-v2.json");
static NEXT: AtomicU64 = AtomicU64::new(0);
fn sha(byte: &str) -> Sha256 {
    Sha256::new(byte.repeat(32)).unwrap()
}
fn capsule() -> ThemeSettingsCapsule {
    let account = AccountPrincipal::from_account(1000, 1000, "fixture", "/home/fixture").unwrap();
    ThemeSettingsCapsule {
        version: 1,
        native_receipt: RecoveryManifest::parse_verified_json(LEGACY2)
            .unwrap()
            .root_snapshot_receipt()
            .unwrap()
            .clone(),
        binding: ThemeSettingsBinding {
            account,
            uid: 1000,
            executable_sha256: sha("11"),
            schema_sha256: sha("22"),
            session_service_sha256: sha("33"),
        },
        approved_policy_sha256: sha("44"),
        plan_sha256: sha("55"),
        changes_sha256: sha("66"),
        keys: [(
            ThemeSettingKey::ColorScheme,
            ThemeSettingState {
                user_value: None,
                effective_value: "default".into(),
                default_value: "default".into(),
                writable: true,
            },
        )]
        .into(),
    }
}
fn manifest() -> RecoveryManifest {
    let mut manifest = RecoveryManifest::parse_verified_json(LEGACY2).unwrap();
    manifest.attach_account_principal(capsule().binding.account);
    manifest.seal().unwrap();
    manifest
}

#[test]
fn legacy_manifest_bytes_and_digest_remain_exactly_compatible() {
    for original in [LEGACY1, LEGACY2] {
        let manifest = RecoveryManifest::parse_verified_json(original).unwrap();
        assert!(manifest.logical_settings().is_none());
        assert_eq!(manifest.canonical_bytes().unwrap(), original);
        assert!(manifest.verify_digest().unwrap());
        assert!(!std::str::from_utf8(original).unwrap().contains("logical_settings"));
    }
}

#[test]
fn private_worker_changes_reject_duplicate_or_unknown_keys() {
    #[derive(serde::Deserialize)]
    struct Request {
        #[serde(deserialize_with = "deserialize_setting_changes")]
        desired: BTreeMap<ThemeSettingKey, ThemeSettingChange>,
    }
    let request: Request =
        serde_json::from_str(r#"{"desired":{"color-scheme":{"operation":"reset"}}}"#).unwrap();
    assert_eq!(request.desired.len(), 1);
    for json in [
        r#"{"desired":{"color-scheme":{"operation":"reset"},"color-scheme":{"operation":"set","value":"prefer-dark"}}}"#,
        r#"{"desired":{"font-name":{"operation":"reset"}}}"#,
    ] {
        assert!(serde_json::from_str::<Request>(json).is_err());
    }
}

#[test]
fn schema_three_capsule_is_account_bound_and_entirely_sealed() {
    let mut manifest = manifest();
    manifest.attach_logical_settings(capsule()).unwrap();
    assert_eq!(manifest.schema_version, 3);
    manifest.seal().unwrap();
    let bytes = manifest.canonical_bytes().unwrap();
    let reopened = RecoveryManifest::parse_verified_json(&bytes).unwrap();
    assert_eq!(reopened.logical_settings(), Some(&capsule()));
    let tampered = String::from_utf8(bytes)
        .unwrap()
        .replace("\"effective_value\":\"default\"", "\"effective_value\":\"prefer-dark\"");
    assert!(matches!(
        RecoveryManifest::parse_verified_json(tampered.as_bytes()),
        Err(ManifestError::DigestMismatch)
    ));
    let mut foreign = capsule();
    foreign.binding.account =
        AccountPrincipal::from_account(1001, 1001, "other", "/home/other").unwrap();
    assert_eq!(
        manifest.attach_logical_settings(foreign),
        Err(ValidationError::InvalidLogicalSettings)
    );
}

#[test]
fn logical_family_cannot_be_smuggled_into_legacy_or_omitted_from_schema_three() {
    let mut legacy = RecoveryManifest::parse_verified_json(LEGACY1).unwrap();
    legacy.attach_account_principal(capsule().binding.account);
    assert_eq!(
        legacy.attach_logical_settings(capsule()),
        Err(ValidationError::InvalidLogicalSettings)
    );
    let mut modern = manifest();
    modern.schema_version = 3;
    assert_eq!(modern.validate(), Err(ValidationError::InvalidLogicalSettings));
    modern.schema_version = 2;
    modern.attach_logical_settings(capsule()).unwrap();
    for version in [1, 2, 4] {
        modern.schema_version = version;
        assert!(modern.validate().is_err());
    }
}

#[test]
fn logical_capsule_cannot_be_relinked_to_a_different_native_checkpoint() {
    let mut manifest = manifest();
    let mut wrong = capsule();
    let mut foreign = serde_json::to_value(&wrong.native_receipt).unwrap();
    foreign["subvolume_id"] = 301.into();
    wrong.native_receipt = serde_json::from_value(foreign).unwrap();
    assert_ne!(wrong.native_receipt, capsule().native_receipt);
    assert_eq!(
        manifest.attach_logical_settings(wrong),
        Err(ValidationError::InvalidLogicalSettings)
    );
    assert_eq!(manifest.schema_version, 2);
    manifest.attach_logical_settings(capsule()).unwrap();
    let mut changed = serde_json::to_value(capsule().native_receipt).unwrap();
    changed["subvolume_id"] = 301.into();
    let changed = serde_json::from_value(changed).unwrap();
    assert_eq!(
        manifest.attach_root_snapshot_receipt(changed),
        Err(ValidationError::InvalidLogicalSettings)
    );
}

#[test]
fn generation_readback_retains_the_distinct_logical_capsule() {
    let path = std::env::temp_dir().join(format!(
        "ryoku-logical-generation-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    let root = File::open(&path).unwrap();
    let store = GenerationStore::open_for_test(&root).unwrap();
    let mut manifest = manifest();
    manifest.attach_logical_settings(capsule()).unwrap();
    manifest.set_verification(
        VerificationStatus::Verified,
        VerificationStatus::Verified,
        VerificationStatus::Verified,
    );
    manifest.seal().unwrap();
    store.publish(&manifest, &Payloads::new()).unwrap();
    let reopened = store
        .open_verified_transaction(&Identifier::new("txn-legacy-key-fixture").unwrap())
        .unwrap();
    assert_eq!(
        reopened.manifest().logical_settings().unwrap().digest().unwrap(),
        capsule().digest().unwrap()
    );
    assert!(reopened.manifest().files().is_empty());
    drop(reopened);
    drop(store);
    drop(root);
    for directory in
        ["generations/gen-legacy-key-fixture", "generations/gen-legacy-key-fixture/payloads"]
    {
        std::fs::set_permissions(path.join(directory), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn legacy_configuration_restore_cannot_silently_complete_a_logical_family() {
    let mut manifest = manifest();
    manifest.attach_logical_settings(capsule()).unwrap();
    manifest.seal().unwrap();
    let evidence = PreflightEvidence {
        generation: ArtifactStatus::Verified,
        archive: ArtifactStatus::Verified,
        current_state_checkpoint: ArtifactStatus::Verified,
        system_snapshot: SystemSnapshotStatus::Available {
            snapper_config: manifest.snapper_config.clone(),
            snapshot_id: manifest.snapper_snapshot_id.clone(),
            filesystem_uuid: manifest.filesystem_uuid.clone(),
        },
        system_compatibility: CompatibilityStatus::Compatible,
        configuration_compatibility: CompatibilityStatus::Compatible,
    };
    for scope in [RestoreScope::ConfigurationOnly, RestoreScope::CompleteLinked] {
        assert_eq!(
            restore::plan_restore(&manifest, scope, &evidence),
            Err(PreflightError::LogicalSettingsActorRequired)
        );
    }
}
