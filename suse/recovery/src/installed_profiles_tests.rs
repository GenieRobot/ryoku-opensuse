use super::*;
use ryoku_profiles::{Density, Desktop, Theme};
use std::{
    fs as stdfs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) const DEFAULTS: &[u8] = br#"{ "schema_version":1, "id":"generic-desktop", "settings":{"desktop":"hyprland","theme":"ryoku_dark","density":"comfortable"} }"#;
const SYSTEM: &[u8] =
    br#"{"schema_version":1,"id":"generic-desktop","settings":{"desktop":"kde"}}"#;
const COMPANION: &[u8] =
    br#"{"schema_version":1,"id":"generic-desktop","settings":{"theme":"system"}}"#;
const PERSONAL: &[u8] =
    br#"{"schema_version":1,"id":"generic-desktop","settings":{"density":"compact"}}"#;
pub(crate) fn package() -> DefaultsPackage {
    DefaultsPackage {
        name: PackageName::new("ryoku-profiles").unwrap(),
        nevra: Revision::new("ryoku-profiles-0:0.1-1.x86_64").unwrap(),
        upstream_revision: None,
    }
}
pub(crate) fn bundle() -> InstalledProfileBundle {
    InstalledProfileBundle::from_local_documents(package(), DEFAULTS, Some(SYSTEM), Some(COMPANION))
        .unwrap()
}
pub(crate) struct Fixture {
    pub path: PathBuf,
    pub store: ProfileBundleStore,
}
impl Fixture {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-profile-bundle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for dir in [&path, &path.join("profiles"), &path.join("profiles/.staging")] {
            stdfs::create_dir(dir).unwrap();
            stdfs::set_permissions(dir, stdfs::Permissions::from_mode(0o700)).unwrap();
        }
        let root = File::open(&path).unwrap();
        let stat = fs::fstat(&root).unwrap();
        let store =
            ProfileBundleStore::open_existing(&root, stat.st_uid, stat.st_gid, false).unwrap();
        Self { path, store }
    }
    fn published(&self, id: &Sha256) -> PathBuf {
        self.path.join("profiles").join(id.as_str())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            let Ok(entries) = stdfs::read_dir(path) else { return };
            stdfs::set_permissions(path, stdfs::Permissions::from_mode(0o700)).unwrap();
            for entry in entries.flatten() {
                if entry.file_type().unwrap().is_dir() {
                    writable(&entry.path());
                }
            }
        }
        writable(&self.path);
        let _ = stdfs::remove_dir_all(&self.path);
    }
}
fn transaction() -> Identifier {
    Identifier::new("profile-import").unwrap()
}

#[test]
fn immutable_local_bundle_roundtrip_retains_sources_and_all_layers_without_remote_inputs() {
    let fixture = Fixture::new();
    let bundle = bundle();
    let verified = fixture.store.publish_candidate(&transaction(), &bundle).unwrap();
    assert_eq!(verified.bundle().defaults_source_bytes(), DEFAULTS);
    assert_eq!(verified.bundle().defaults_source_sha256(), &digest(DEFAULTS));
    let resolved = verified.bundle().resolve(Some(PERSONAL)).unwrap();
    assert_eq!(
        resolved.settings,
        ResolvedSettings {
            desktop: Some(Desktop::Kde),
            theme: Some(Theme::System),
            density: Some(Density::Compact)
        }
    );
    let reopened = fixture.store.load_revision(&resolved.provenance.profile_revision).unwrap();
    assert_eq!(reopened.id(), verified.id());
    assert_eq!(reopened.bundle().canonical_bytes().unwrap(), bundle.canonical_bytes().unwrap());
    assert_eq!(stdfs::read_dir(fixture.path.join("profiles/.staging")).unwrap().count(), 0);
    assert_eq!(
        stdfs::metadata(fixture.published(verified.id()).join("bundle.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o400
    );
    assert_eq!(
        stdfs::metadata(fixture.published(verified.id())).unwrap().permissions().mode() & 0o7777,
        0o500
    );
    assert_eq!(
        fixture.store.publish_candidate(&transaction(), &bundle).unwrap().id(),
        verified.id()
    );
    assert!(!fixture.path.join("profiles/active").exists());
}

#[test]
fn source_byte_digest_differs_from_semantic_layer_revision() {
    let plain =
        InstalledProfileBundle::from_local_documents(package(), DEFAULTS, None, None).unwrap();
    let formatted = format!("\n{}\n", std::str::from_utf8(DEFAULTS).unwrap());
    let changed =
        InstalledProfileBundle::from_local_documents(package(), formatted.as_bytes(), None, None)
            .unwrap();
    assert_ne!(plain.id().unwrap(), changed.id().unwrap());
    assert_ne!(plain.defaults_source_sha256(), changed.defaults_source_sha256());
    assert_eq!(
        plain.resolve(None).unwrap().provenance.defaults_revision,
        changed.resolve(None).unwrap().provenance.defaults_revision
    );
    let absent = plain.resolve(None).unwrap().provenance;
    assert!(
        absent.system_override_revision.is_none()
            && absent.companion_revision.is_none()
            && absent.personal_override_revision.is_none()
    );
    let empty = br#"{"schema_version":1,"id":"generic-desktop","settings":{}}"#;
    let present =
        InstalledProfileBundle::from_local_documents(package(), DEFAULTS, Some(empty), Some(empty))
            .unwrap();
    let present = present.resolve(Some(empty)).unwrap().provenance;
    assert!(
        present.system_override_revision.is_some()
            && present.companion_revision.is_some()
            && present.personal_override_revision.is_some()
    );
}

#[test]
fn bounded_schema_digest_and_identity_failures_do_not_expose_document_text() {
    let secret = br#"{"schema_version":1,"id":"generic-desktop","remote":"private-secret-marker"}"#;
    let error =
        InstalledProfileBundle::from_local_documents(package(), DEFAULTS, None, Some(secret))
            .unwrap_err();
    assert!(!error.to_string().contains("private-secret-marker"));
    let mut value = serde_json::to_value(bundle()).unwrap();
    value["schema_version"] = 2.into();
    assert!(matches!(
        InstalledProfileBundle::parse(&serde_json::to_vec(&value).unwrap()),
        Err(ProfileStoreError::UnsupportedSchema)
    ));
    value["schema_version"] = 1.into();
    value["defaults"]["source_sha256"] = "0".repeat(64).into();
    assert!(matches!(
        InstalledProfileBundle::parse(&serde_json::to_vec(&value).unwrap()),
        Err(ProfileStoreError::DigestMismatch)
    ));
    assert!(matches!(
        InstalledProfileBundle::parse(&vec![b' '; MAX_BUNDLE_BYTES + 1]),
        Err(ProfileStoreError::TooLarge)
    ));
    let other = br#"{"schema_version":1,"id":"other-desktop","settings":{}}"#;
    assert!(matches!(bundle().resolve(Some(other)), Err(ProfileStoreError::ProfileMismatch)));
}

#[test]
fn interrupted_or_corrupt_objects_are_not_adopted_repaired_or_replaced() {
    let fixture = Fixture::new();
    stdfs::create_dir(fixture.path.join("profiles/.staging/profile-import")).unwrap();
    assert!(matches!(
        fixture.store.publish_candidate(&transaction(), &bundle()),
        Err(ProfileStoreError::NeedsReconciliation)
    ));
    assert!(!fixture.published(&bundle().id().unwrap()).exists());
    let verified = fixture
        .store
        .publish_candidate(&Identifier::new("another-import").unwrap(), &bundle())
        .unwrap();
    let file = fixture.published(verified.id()).join("bundle.json");
    stdfs::set_permissions(&file, stdfs::Permissions::from_mode(0o600)).unwrap();
    stdfs::write(&file, b"corrupted").unwrap();
    stdfs::set_permissions(&file, stdfs::Permissions::from_mode(0o400)).unwrap();
    assert!(verified.revalidate().is_err());
    assert!(fixture.store.publish_candidate(&transaction(), &bundle()).is_err());
    assert_eq!(stdfs::read(&file).unwrap(), b"corrupted");
    assert!(
        fixture.store.load_revision(&Revision::new("opaque-legacy-revision").unwrap()).is_err()
    );
}

#[test]
fn symlinks_extra_entries_and_foreign_identity_are_refused() {
    let fixture = Fixture::new();
    let id = bundle().id().unwrap();
    symlink(".staging", fixture.published(&id)).unwrap();
    assert!(fixture.store.publish_candidate(&transaction(), &bundle()).is_err());
    stdfs::remove_file(fixture.published(&id)).unwrap();
    let verified = fixture.store.publish_candidate(&transaction(), &bundle()).unwrap();
    stdfs::set_permissions(fixture.published(&id), stdfs::Permissions::from_mode(0o700)).unwrap();
    stdfs::write(fixture.published(&id).join("unknown"), b"keep").unwrap();
    stdfs::set_permissions(fixture.published(&id), stdfs::Permissions::from_mode(0o500)).unwrap();
    assert!(verified.revalidate().is_err());
    let root = File::open(&fixture.path).unwrap();
    let stat = fs::fstat(&root).unwrap();
    assert!(ProfileBundleStore::open_existing(&root, stat.st_uid + 1, stat.st_gid, false).is_err());
    assert!(ProfileBundleStore::open_existing(&root, stat.st_uid, stat.st_gid + 1, false).is_err());
}

#[test]
fn package_loader_has_one_fixed_path_and_preserves_original_default_bytes() {
    let fixture = Fixture::new();
    let directory = fixture.path.join("usr/share/ryoku/config");
    stdfs::create_dir_all(&directory).unwrap();
    stdfs::set_permissions(&directory, stdfs::Permissions::from_mode(0o755)).unwrap();
    let source = directory.join("profile.json");
    stdfs::write(&source, DEFAULTS).unwrap();
    stdfs::set_permissions(&source, stdfs::Permissions::from_mode(0o644)).unwrap();
    let root = File::open(&fixture.path).unwrap();
    let stat = fs::fstat(&root).unwrap();
    assert_eq!(read_packaged_defaults(&root, stat.st_uid, stat.st_gid).unwrap(), DEFAULTS);
    stdfs::set_permissions(&source, stdfs::Permissions::from_mode(0o666)).unwrap();
    assert!(read_packaged_defaults(&root, stat.st_uid, stat.st_gid).is_err());
    stdfs::remove_file(&source).unwrap();
    symlink("../../../../outside", &source).unwrap();
    assert!(read_packaged_defaults(&root, stat.st_uid, stat.st_gid).is_err());
}

#[test]
fn pinned_bundle_must_remain_discoverable_at_its_exact_published_name() {
    let fixture = Fixture::new();
    let verified = fixture.store.publish_candidate(&transaction(), &bundle()).unwrap();
    let path = fixture.published(verified.id());
    let moved = path.with_file_name("retained-object");
    stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o700)).unwrap();
    stdfs::rename(&path, &moved).unwrap();
    stdfs::set_permissions(&moved, stdfs::Permissions::from_mode(0o500)).unwrap();
    assert!(verified.revalidate().is_err());
    // Even an independently published equal-content replacement cannot reuse
    // a token for the original inode and namespace association.
    fixture
        .store
        .publish_candidate(&Identifier::new("replace-import").unwrap(), &bundle())
        .unwrap();
    assert!(matches!(verified.revalidate(), Err(ProfileStoreError::ConcurrentModification)));
    assert!(fixture.store.load(verified.id()).is_ok());
}

#[test]
fn installer_absence_has_versioned_provenance_and_preserves_all_override_layers() {
    let absent =
        InstalledProfileBundle::without_installed_defaults(Some(SYSTEM), Some(COMPANION)).unwrap();
    assert!(absent.defaults_are_absent());
    let bytes = absent.canonical_bytes().unwrap();
    let parsed = InstalledProfileBundle::parse(&bytes).unwrap();
    assert_eq!(parsed.system_source_bytes(), Some(SYSTEM));
    assert_eq!(parsed.companion_source_bytes(), Some(COMPANION));
    let resolved = parsed.resolve(Some(PERSONAL)).unwrap();
    assert_eq!(resolved.settings.desktop, Some(Desktop::Kde));
    assert_eq!(resolved.settings.theme, Some(Theme::System));
    assert_eq!(resolved.settings.density, Some(Density::Compact));
    let empty = crate::installer_state::from_installed(&Default::default()).unwrap();
    assert!(parsed.matches_packages(&empty));
    assert!(!parsed.matches_packages(&[]));
    let installed = crate::installer_state::from_installed(&std::collections::BTreeMap::from([(
        "ryoku-profile-defaults".into(),
        "0:0.1.0-1.noarch".into(),
    )]))
    .unwrap();
    assert!(!parsed.matches_packages(&installed));
    let installed_bundle = InstalledProfileBundle::from_local_documents(
        DefaultsPackage {
            name: PackageName::new("ryoku-profile-defaults").unwrap(),
            nevra: Revision::new("ryoku-profile-defaults-0:0.1.0-1.noarch").unwrap(),
            upstream_revision: None,
        },
        DEFAULTS,
        Some(SYSTEM),
        Some(COMPANION),
    )
    .unwrap();
    assert!(!installed_bundle.matches_packages(&empty));
    assert!(installed_bundle.matches_packages(&installed));
    assert_ne!(installed_bundle.id().unwrap(), parsed.id().unwrap());
    assert_eq!(installed_bundle.system_source_bytes(), parsed.system_source_bytes());
    assert_eq!(installed_bundle.companion_source_bytes(), parsed.companion_source_bytes());
}
#[test]
fn absent_defaults_cannot_be_smuggled_into_schema_one_or_changed_to_real_defaults() {
    let absent = InstalledProfileBundle::without_installed_defaults(None, None).unwrap();
    for kind in 0..5 {
        let mut altered = absent.clone();
        match kind {
            0 => altered.schema_version = 1,
            1 => altered.defaults_absent = None,
            2 => altered.defaults_absent = Some(false),
            3 => altered.defaults = LocalLayer::new(DEFAULTS).unwrap(),
            4 => altered.package.nevra = Revision::new("real-package").unwrap(),
            _ => unreachable!(),
        }
        assert!(altered.canonical_bytes().is_err());
    }
    let normal = bundle();
    let mut mixed = normal.clone();
    mixed.defaults_absent = Some(true);
    assert!(mixed.canonical_bytes().is_err());
    assert!(
        !String::from_utf8(normal.canonical_bytes().unwrap()).unwrap().contains("defaults_absent")
    );
}
#[test]
fn standard_capture_and_restore_reject_absent_defaults_without_exact_installer_marker() {
    let absent = InstalledProfileBundle::without_installed_defaults(None, None).unwrap();
    let packages = crate::installer_state::from_installed(&Default::default()).unwrap();
    assert!(
        absent.validate_checkpoint_inputs(&crate::installer_state::reason(), &packages).is_ok()
    );
    for reason in [
        crate::OperationReason::PackageChange,
        crate::OperationReason::ConfigurationRestore,
        crate::OperationReason::ProfileApply,
        crate::OperationReason::Other(Identifier::new("installer-lifecycle-v2").unwrap()),
    ] {
        assert!(matches!(
            absent.validate_checkpoint_inputs(&reason, &packages),
            Err(ProfileStoreError::UnsupportedSchema)
        ));
    }
    let mut wrong = packages.clone();
    wrong[0].package_name = PackageName::new("ryoku-unrecognized").unwrap();
    assert!(absent.validate_checkpoint_inputs(&crate::installer_state::reason(), &wrong).is_err());
    let mut missing = packages;
    missing.pop();
    assert!(
        absent.validate_checkpoint_inputs(&crate::installer_state::reason(), &missing).is_err()
    );
    // A normal installed bundle cannot be paired with absence to imitate a
    // removal/clean boundary, even under the correct marker.
    assert!(
        bundle().validate_checkpoint_inputs(&crate::installer_state::reason(), &missing).is_err()
    );
}
