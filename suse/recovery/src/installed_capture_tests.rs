use super::*;
use crate::{
    OperationReason, SnapshotLink,
    capture::{
        CaptureError, CaptureLimits, CapturedInventory, XattrReader, capture_inventory_with_xattrs,
    },
    generation_capture::VerifiedSnapshotLink,
};
use ryoku_profiles::{Density, Desktop, Theme};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const USER: StateRoot = StateRoot::UserRyokuConfig;
const PROFILE_PATHS: [&str; 4] =
    ["defaults.json", "system.json", "companion.json", "personal.json"];
const SOURCES: ProfileSources = ProfileSources {
    defaults: (USER, "defaults.json"),
    system: (USER, "system.json"),
    companion: (USER, "companion.json"),
    personal: (USER, "personal.json"),
};
// These paths are synthetic, private fixtures. No installed catalogue uses them.
fn entries() -> Vec<Enrollment> {
    PROFILE_PATHS
        .iter()
        .map(|path| Enrollment {
            root: USER,
            path,
            writer: "fixture-profile-writer",
            owner: Owner::Account,
            kind: Kind::Regular,
            modes: &[0o600, 0o640, 0o644],
            max_bytes: 64 * 1024,
            absence_allowed: true,
        })
        .chain([Enrollment {
            root: USER,
            path: "missing.json",
            writer: "fixture-settings-writer",
            owner: Owner::Account,
            kind: Kind::Regular,
            modes: &[0o600],
            max_bytes: 1024,
            absence_allowed: true,
        }])
        .collect()
}
fn plan() -> InstalledInventory {
    InstalledInventory::compile(
        &["fixture-profile-writer", "fixture-settings-writer"],
        &entries(),
        Some(SOURCES),
    )
    .unwrap()
}
struct Fixture {
    path: PathBuf,
    source: File,
    store: GenerationStore,
    account: CaptureAccount,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-installed-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        for name in ["source", "store"] {
            fs::create_dir(path.join(name)).unwrap();
            fs::set_permissions(path.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let source = File::open(path.join("source")).unwrap();
        let store =
            GenerationStore::open_for_test(&File::open(path.join("store")).unwrap()).unwrap();
        let stat = fstat(&source).unwrap();
        let result = Self {
            path,
            source,
            store,
            account: CaptureAccount {
                uid: stat.st_uid,
                gid: stat.st_gid,
                principal: crate::AccountPrincipal::from_account(
                    stat.st_uid,
                    stat.st_gid,
                    "fixture",
                    "/home/fixture",
                )
                .unwrap(),
            },
        };
        result.write("defaults.json", br#"{ "schema_version":1, "id":"fixture-desktop", "settings":{"desktop":"hyprland","theme":"ryoku_dark","density":"comfortable"} }"#, 0o640);
        result.write(
            "system.json",
            br#"{"schema_version":1,"id":"fixture-desktop","settings":{"desktop":"kde"}}"#,
            0o644,
        );
        result.write(
            "companion.json",
            br#"{"schema_version":1,"id":"fixture-desktop","settings":{"theme":"system"}}"#,
            0o600,
        );
        result.write(
            "personal.json",
            br#"{"schema_version":1,"id":"fixture-desktop","settings":{"density":"compact"}}"#,
            0o600,
        );
        result.write("unrelated-private", b"must not be archived", 0o600);
        result
    }
    fn write(&self, name: &str, bytes: &[u8], mode: u32) {
        fs::write(self.path.join("source").join(name), bytes).unwrap();
        fs::set_permissions(self.path.join("source").join(name), fs::Permissions::from_mode(mode))
            .unwrap();
    }
    fn roots(&self) -> InstalledRoots<'_> {
        InstalledRoots {
            user_configuration: &self.source,
            account_home: None,
            system_configuration: None,
            account: self.account.clone(),
        }
    }
    fn capture(
        &self,
        plan: &InstalledInventory,
        id: &str,
    ) -> Result<RecoveryManifest, GenerationCaptureError> {
        capture_installed_configuration_using(&self.store, plan, self.roots(), request(id), collect)
    }
    fn no_publication(&self, id: &str) {
        assert!(self.store.verify_transaction(&Identifier::new(id).unwrap()).is_err());
        assert_eq!(fs::read_dir(self.path.join("store/generations")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(self.path.join("store/catalog")).unwrap().count(), 0);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            let Ok(children) = fs::read_dir(path) else { return };
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            for entry in children.flatten() {
                if entry.file_type().unwrap().is_dir() {
                    writable(&entry.path());
                }
            }
        }
        writable(&self.path);
        let _ = fs::remove_dir_all(&self.path);
    }
}
struct Metadata;
const LABEL: &[u8] = b"fixture_u:object_r:fixture_t:s0\0";
impl XattrReader for Metadata {
    fn read(&self, _: &File, name: &'static str) -> Result<Option<Vec<u8>>, CaptureError> {
        Ok(match name {
            "system.posix_acl_access" => Some(vec![2, 0, 0, 0]),
            "security.selinux" => Some(LABEL.to_vec()),
            _ => None,
        })
    }
    fn read_symlink(
        &self,
        _: &File,
        _: &str,
        _: &'static str,
    ) -> Result<Option<Vec<u8>>, CaptureError> {
        Ok(None)
    }
}
fn collect(
    root: &File,
    state: StateRoot,
    paths: &[RelativePath],
    limits: CaptureLimits,
) -> Result<CapturedInventory, CaptureError> {
    capture_inventory_with_xattrs(root, state, paths, limits, &Metadata)
}
fn request(id: &str) -> InstalledCaptureRequest {
    InstalledCaptureRequest {
        transaction_id: Identifier::new(id).unwrap(),
        generation_id: Identifier::new(format!("generation-{id}")).unwrap(),
        parent_generation_id: None,
        snapshot: VerifiedSnapshotLink::from_preverified_link(
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
        ),
        reason: OperationReason::ProfileApply,
        created_utc: "2026-10-15T00:00:00Z".into(),
        package_revisions: vec![],
        provider_revision: None,
        limits: CaptureLimits::default(),
    }
}

#[test]
fn installed_candidates_remain_unavailable_and_enrollment_is_exact() {
    assert_eq!(
        InstalledInventory::current().unwrap().entries.len(),
        crate::installed_inventory::PREFERENCES.len()
    );
    let mut candidates = entries();
    assert!(matches!(
        InstalledInventory::compile(
            &["fixture-profile-writer", "fixture-settings-writer", "missing-writer"],
            &candidates,
            Some(SOURCES)
        ),
        Err(InventoryError::IncompleteWriterCoverage)
    ));
    for invalid in ["../outside", "/etc/passwd", "nested/../../outside"] {
        candidates[0].path = invalid;
        assert!(matches!(
            InstalledInventory::compile(
                &["fixture-profile-writer", "fixture-settings-writer"],
                &candidates,
                Some(SOURCES)
            ),
            Err(InventoryError::InvalidEnrollment)
        ));
    }
    for root in [StateRoot::StateGeneration, StateRoot::UserRyokuProfile] {
        let mut candidates = entries();
        candidates[0].root = root;
        assert!(matches!(
            InstalledInventory::compile(
                &["fixture-profile-writer", "fixture-settings-writer"],
                &candidates,
                Some(SOURCES)
            ),
            Err(InventoryError::InvalidEnrollment)
        ));
    }
    let mut candidates = entries();
    candidates.push(candidates[0]);
    assert!(matches!(
        InstalledInventory::compile(
            &["fixture-profile-writer", "fixture-settings-writer"],
            &candidates,
            Some(SOURCES)
        ),
        Err(InventoryError::InvalidEnrollment)
    ));
    let mut candidates = entries();
    candidates[0].modes = &[0o660];
    assert!(matches!(
        InstalledInventory::compile(
            &["fixture-profile-writer", "fixture-settings-writer"],
            &candidates,
            Some(SOURCES)
        ),
        Err(InventoryError::InvalidEnrollment)
    ));
}

#[test]
fn multilayer_capture_preserves_exact_bytes_metadata_absence_and_effective_revisions() {
    let fixture = Fixture::new();
    let plan = plan();
    let manifest = fixture.capture(&plan, "complete").unwrap();
    assert_eq!(manifest.files().len(), 5);
    assert!(manifest.is_mutation_ready());
    let verified = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
    let mut payloads = Payloads::new();
    for record in manifest.files() {
        if record.path.as_str() == "missing.json" {
            assert!(matches!(record.kind, EntryKind::Absent));
            continue;
        }
        let EntryKind::RegularFile { metadata, sha256, .. } = &record.kind else {
            panic!("regular profile")
        };
        let source = fixture.path.join("source").join(record.path.as_str());
        let original = fs::metadata(&source).unwrap();
        assert_eq!(
            (metadata.uid, metadata.gid, metadata.mode),
            (original.uid(), original.gid(), original.mode() & 0o7777)
        );
        assert_eq!(
            metadata.mtime_unix_ns,
            i128::from(original.mtime()) * 1_000_000_000 + i128::from(original.mtime_nsec())
        );
        assert!(metadata.acl_sha256.is_some());
        let label = metadata.extended.values.get(&crate::XattrName::SelinuxLabel).unwrap();
        assert_eq!(verified.read_payload(label).unwrap(), LABEL);
        let bytes = verified.read_payload(sha256).unwrap();
        assert_eq!(bytes, fs::read(source).unwrap());
        payloads.insert(sha256.clone(), bytes);
    }
    let profile = plan.profile(manifest.files(), &payloads).unwrap();
    assert_eq!(
        profile.effective,
        ResolvedSettings {
            desktop: Some(Desktop::Kde),
            theme: Some(Theme::System),
            density: Some(Density::Compact)
        }
    );
    assert_eq!(manifest.profile_revision, Some(profile.provenance));
    assert_eq!(
        fs::read(fixture.path.join("source/unrelated-private")).unwrap(),
        b"must not be archived"
    );
}

#[test]
fn missing_optional_layers_have_absent_records_and_null_revisions() {
    let fixture = Fixture::new();
    for name in ["system.json", "companion.json", "personal.json"] {
        fs::remove_file(fixture.path.join("source").join(name)).unwrap();
    }
    let absent = fixture.capture(&plan(), "absent").unwrap();
    let revision = absent.profile_revision.unwrap();
    assert_eq!(
        (
            revision.system_override_revision,
            revision.companion_revision.clone(),
            revision.personal_override_revision
        ),
        (None, None, None)
    );
    fixture.write(
        "companion.json",
        br#"{"schema_version":1,"id":"fixture-desktop","settings":{}}"#,
        0o600,
    );
    let present = fixture.capture(&plan(), "present-empty").unwrap().profile_revision.unwrap();
    assert!(present.companion_revision.is_some());
    assert_ne!(present.profile_revision, revision.profile_revision);
    for name in ["system.json", "personal.json"] {
        fixture.write(name, br#"{"schema_version":1,"id":"fixture-desktop","settings":{}}"#, 0o600);
    }
    let all_present = fixture.capture(&plan(), "all-empty").unwrap().profile_revision.unwrap();
    assert!(all_present.system_override_revision.is_some());
    assert!(all_present.personal_override_revision.is_some());
    assert_ne!(all_present.profile_revision, present.profile_revision);
}

#[test]
fn missing_defaults_and_invalid_or_mismatched_profiles_never_publish() {
    for (bytes, expected) in [
        (
            br#"{"schema_version":2,"id":"fixture-desktop","settings":{}}"#.as_slice(),
            InventoryError::InvalidProfile,
        ),
        (
            br#"{"schema_version":1,"id":"other-profile","settings":{}}"#.as_slice(),
            InventoryError::ProfileIdentityMismatch,
        ),
        (
            br#"{"schema_version":1,"id":"fixture-desktop","remote":"https://invalid.test"}"#
                .as_slice(),
            InventoryError::InvalidProfile,
        ),
    ] {
        let fixture = Fixture::new();
        fixture.write("companion.json", bytes, 0o600);
        assert!(
            matches!(fixture.capture(&plan(), "invalid"), Err(GenerationCaptureError::Inventory(error)) if error == expected)
        );
        fixture.no_publication("invalid");
    }
    let fixture = Fixture::new();
    fs::remove_file(fixture.path.join("source/defaults.json")).unwrap();
    assert!(matches!(
        fixture.capture(&plan(), "no-defaults"),
        Err(GenerationCaptureError::Inventory(InventoryError::MissingProfileDefaults))
    ));
    fixture.no_publication("no-defaults");
}

#[test]
fn inventory_rejects_omissions_substitutions_duplicates_and_wrong_owners_before_sealing() {
    let fixture = Fixture::new();
    let plan = plan();
    let original = collect(&fixture.source, USER, &plan.paths[&USER], CaptureLimits::default())
        .unwrap()
        .records;
    for change in 0..7 {
        let mut records = original.clone();
        match change {
            0 => {
                records.pop();
            }
            1 => records[0].path = RelativePath::new("unrelated-private").unwrap(),
            2 => records[0] = records[1].clone(),
            3 => records[0].root = StateRoot::EtcRyoku,
            4..=6 => {
                let EntryKind::RegularFile { metadata, .. } = &mut records[0].kind else {
                    panic!()
                };
                match change {
                    4 => metadata.uid += 1,
                    5 => metadata.gid += 1,
                    _ => metadata.mode = 0o4755,
                }
            }
            _ => unreachable!(),
        }
        let error = capture_installed_configuration_using(
            &fixture.store,
            &plan,
            fixture.roots(),
            request("bad-records"),
            |_, _, _, _| {
                Ok(CapturedInventory {
                    records: records.clone(),
                    metadata_payloads: Payloads::new(),
                    source_bytes: 0,
                })
            },
        );
        assert!(error.is_err());
        fixture.no_publication("bad-records");
        assert!(plan.validate_records(&records, &fixture.account).is_err());
    }
}

#[test]
fn rejected_modes_symlink_and_unsupported_metadata_leave_no_publication() {
    let fixture = Fixture::new();
    fixture.write("personal.json", b"{}", 0o666);
    assert!(matches!(
        fixture.capture(&plan(), "unsafe-mode"),
        Err(GenerationCaptureError::Inventory(InventoryError::EntryPolicy))
    ));
    fixture.no_publication("unsafe-mode");
    fs::remove_file(fixture.path.join("source/personal.json")).unwrap();
    symlink("unrelated-private", fixture.path.join("source/personal.json")).unwrap();
    assert!(matches!(
        fixture.capture(&plan(), "link"),
        Err(GenerationCaptureError::Inventory(InventoryError::EntryPolicy))
    ));
    fixture.no_publication("link");
    fs::remove_file(fixture.path.join("source/personal.json")).unwrap();
    symlink("../outside", fixture.path.join("source/personal.json")).unwrap();
    assert!(fixture.capture(&plan(), "escape").is_err());
    fixture.no_publication("escape");
    let result = capture_installed_configuration_using(
        &fixture.store,
        &plan(),
        fixture.roots(),
        request("metadata"),
        |_, _, _, _| Err(CaptureError::UnsupportedMetadata("security.selinux")),
    );
    assert!(matches!(
        result,
        Err(GenerationCaptureError::Capture(CaptureError::UnsupportedMetadata(_)))
    ));
    fixture.no_publication("metadata");
}

#[test]
fn changed_second_capture_cannot_publish() {
    let fixture = Fixture::new();
    let plan = plan();
    let count = std::cell::Cell::new(0);
    let result = capture_installed_configuration_using(
        &fixture.store,
        &plan,
        fixture.roots(),
        request("racing"),
        |root, state, paths, limits| {
            count.set(count.get() + 1);
            if count.get() == 2 {
                fixture.write("personal.json", b"{}", 0o600);
            }
            collect(root, state, paths, limits)
        },
    );
    assert!(matches!(result, Err(GenerationCaptureError::VerificationMismatch)));
    fixture.no_publication("racing");
}

#[test]
fn directory_and_symlink_require_explicit_enrollment_and_preserve_link_text() {
    let fixture = Fixture::new();
    let mut entries = entries();
    entries.extend([
        Enrollment {
            root: USER,
            path: "directory",
            writer: "fixture-settings-writer",
            owner: Owner::Account,
            kind: Kind::Directory,
            modes: &[0o700],
            max_bytes: 1,
            absence_allowed: false,
        },
        Enrollment {
            root: USER,
            path: "link",
            writer: "fixture-settings-writer",
            owner: Owner::Account,
            kind: Kind::Symlink,
            modes: &[0o777],
            max_bytes: 128,
            absence_allowed: false,
        },
    ]);
    fs::create_dir(fixture.path.join("source/directory")).unwrap();
    fs::set_permissions(fixture.path.join("source/directory"), fs::Permissions::from_mode(0o700))
        .unwrap();
    fixture.write("directory/unlisted", b"ignored", 0o600);
    symlink("personal.json", fixture.path.join("source/link")).unwrap();
    let plan = InstalledInventory::compile(
        &["fixture-profile-writer", "fixture-settings-writer"],
        &entries,
        Some(SOURCES),
    )
    .unwrap();
    let manifest = fixture.capture(&plan, "links").unwrap();
    assert_eq!(manifest.files().len(), 7);
    assert!(manifest.files().iter().any(|entry| matches!(&entry.kind, EntryKind::Symlink { target, .. } if target == "personal.json")));
}

#[test]
fn hardlinks_and_unproven_system_roots_fail_closed() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.path.join("source/personal.json")).unwrap();
    fs::hard_link(
        fixture.path.join("source/companion.json"),
        fixture.path.join("source/personal.json"),
    )
    .unwrap();
    assert!(matches!(
        fixture.capture(&plan(), "hardlinks"),
        Err(GenerationCaptureError::Inventory(InventoryError::EntryPolicy))
    ));
    fixture.no_publication("hardlinks");
    let mut entries = entries();
    entries.push(Enrollment {
        root: StateRoot::EtcRyoku,
        path: "fixture-system.json",
        writer: "fixture-settings-writer",
        owner: Owner::System,
        kind: Kind::Regular,
        modes: &[0o600],
        max_bytes: 1024,
        absence_allowed: true,
    });
    let plan = InstalledInventory::compile(
        &["fixture-profile-writer", "fixture-settings-writer"],
        &entries,
        Some(SOURCES),
    )
    .unwrap();
    assert!(matches!(
        fixture.capture(&plan, "no-root"),
        Err(GenerationCaptureError::Inventory(InventoryError::MissingRoot))
    ));
    fixture.no_publication("no-root");
    if fixture.account.uid != 0 {
        let roots =
            InstalledRoots { system_configuration: Some(&fixture.source), ..fixture.roots() };
        assert!(matches!(
            capture_installed_configuration_using(
                &fixture.store,
                &plan,
                roots,
                request("foreign-root"),
                collect
            ),
            Err(GenerationCaptureError::Inventory(InventoryError::UnsafeRoot))
        ));
        fixture.no_publication("foreign-root");
    }
}

#[test]
fn capture_binds_exact_protected_bundle_without_restoring_packaged_defaults() {
    use crate::installed_profiles::tests as profile_test;
    let fixture = Fixture::new();
    let protected = profile_test::Fixture::new();
    let verified = protected
        .store
        .publish_candidate(&Identifier::new("import-bundle").unwrap(), &profile_test::bundle())
        .unwrap();
    fixture.write(
        "opensuse-profile.json",
        br#"{"schema_version":1,"id":"generic-desktop","settings":{"density":"compact"}}"#,
        0o600,
    );
    let entry = Enrollment { path: "opensuse-profile.json", ..entries()[0] };
    let plan = InstalledInventory::compile(&["fixture-profile-writer"], &[entry], None).unwrap();
    let mut request = request("bundle-capture");
    let package = profile_test::package();
    request.package_revisions = vec![crate::PackageRevision {
        package_name: package.name,
        before_nevra: Some(package.nevra),
        after_nevra: None,
        upstream_revision: package.upstream_revision,
    }];
    let manifest = capture_installed_configuration_using_profile(
        &fixture.store,
        &plan,
        fixture.roots(),
        request,
        collect,
        Some(&verified),
    )
    .unwrap();
    assert_eq!(manifest.files().len(), 1);
    assert_eq!(manifest.files()[0].path.as_str(), "opensuse-profile.json");
    assert!(manifest.files().iter().all(|file| file.root == USER));
    let provenance = manifest.profile_revision.as_ref().unwrap();
    assert!(provenance.profile_revision.as_str().starts_with("bundle-sha256-"));
    let reopened = protected.store.load_revision(&provenance.profile_revision).unwrap();
    let personal = fs::read(fixture.path.join("source/opensuse-profile.json")).unwrap();
    assert_eq!(reopened.bundle().resolve(Some(&personal)).unwrap().provenance, *provenance);
    assert!(reopened.bundle().matches_packages(&manifest.package_revisions));
    let generation = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
    fs::remove_file(fixture.path.join("source/opensuse-profile.json")).unwrap();
    // Recovery reads archived personal bytes and the immutable local bundle;
    // neither the live profile nor an original companion source is needed.
    let resolved = protected.store.resolve_generation(&generation).unwrap();
    assert_eq!(resolved.provenance, *provenance);
    assert_eq!(resolved.settings.density, Some(Density::Compact));
    let bundle_path = protected.path.join("profiles").join(verified.id().as_str());
    fs::set_permissions(&bundle_path, fs::Permissions::from_mode(0o700)).unwrap();
    let retained = protected.path.join("profiles/retained-for-test");
    fs::rename(bundle_path, &retained).unwrap();
    fs::set_permissions(retained, fs::Permissions::from_mode(0o500)).unwrap();
    assert!(protected.store.resolve_generation(&generation).is_err());
}

#[test]
fn missing_bundle_namespace_or_mismatched_package_cannot_publish_capture() {
    use crate::installed_profiles::tests as profile_test;
    let fixture = Fixture::new();
    let protected = profile_test::Fixture::new();
    let verified = protected
        .store
        .publish_candidate(&Identifier::new("import-bundle").unwrap(), &profile_test::bundle())
        .unwrap();
    let entry = Enrollment { path: "opensuse-profile.json", ..entries()[0] };
    let plan = InstalledInventory::compile(&["fixture-profile-writer"], &[entry], None).unwrap();
    assert!(matches!(
        capture_installed_configuration_using_profile(
            &fixture.store,
            &plan,
            fixture.roots(),
            request("bad-package"),
            collect,
            Some(&verified)
        ),
        Err(GenerationCaptureError::ProfileStore(
            crate::installed_profiles::ProfileStoreError::PackageMismatch
        ))
    ));
    fixture.no_publication("bad-package");
    let mut request = request("missing-bundle");
    let package = profile_test::package();
    request.package_revisions = vec![crate::PackageRevision {
        package_name: package.name,
        before_nevra: Some(package.nevra),
        after_nevra: None,
        upstream_revision: package.upstream_revision,
    }];
    let count = std::cell::Cell::new(0);
    let result = capture_installed_configuration_using_profile(
        &fixture.store,
        &plan,
        fixture.roots(),
        request,
        |root, state, paths, limits| {
            count.set(count.get() + 1);
            if count.get() == 2 {
                let source = protected.path.join("profiles").join(verified.id().as_str());
                fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
                let target = protected.path.join("profiles/retained-object");
                fs::rename(&source, &target).unwrap();
                fs::set_permissions(target, fs::Permissions::from_mode(0o500)).unwrap();
            }
            collect(root, state, paths, limits)
        },
        Some(&verified),
    );
    assert!(matches!(result, Err(GenerationCaptureError::ProfileStore(_))));
    fixture.no_publication("missing-bundle");
}

#[test]
fn compiled_inventory_preserves_preferences_links_absence_metadata_and_offline_layers() {
    use crate::installed_profiles::tests as profile_test;
    let fixture = Fixture::new();
    let protected = profile_test::Fixture::new();
    let verified = protected
        .store
        .publish_candidate(&Identifier::new("compiled-inputs").unwrap(), &profile_test::bundle())
        .unwrap();
    let root = fixture.path.join("source");
    fs::create_dir_all(root.join(".config/ryoku")).unwrap();
    fs::create_dir_all(root.join(".local/state/ryoku")).unwrap();
    fs::create_dir_all(root.join(".cache/ryogami/wallpaper")).unwrap();
    fixture.write(
        crate::installed_inventory::PERSONAL_PROFILE,
        br#"{"schema_version":1,"id":"generic-desktop","settings":{"density":"compact"}}"#,
        0o600,
    );
    fixture.write(".config/ryoku/profile.json", b"opaque upstream Hub data", 0o640);
    fixture.write(".config/ryoku/theme.json", b"customized theme", 0o640);
    fixture.write(".local/state/ryoku/stage-walls.json", b"opaque wall settings", 0o600);
    fixture.write(
        ".cache/ryogami/wallpaper/index.json",
        br#"{"synthetic":{"apply_count":2}}"#,
        0o640,
    );
    fixture.write(".cache/ryogami/wallpaper/state.json", br#"{"selected":"synthetic"}"#, 0o600);
    symlink("theme.json", root.join(".config/ryoku/shell.json")).unwrap();
    let plan = InstalledInventory::current().unwrap();
    let mut req = request("compiled-capture");
    let package = profile_test::package();
    req.package_revisions = vec![crate::PackageRevision {
        package_name: package.name,
        before_nevra: Some(package.nevra),
        after_nevra: None,
        upstream_revision: package.upstream_revision,
    }];
    let mut roots = fixture.roots();
    roots.account_home = Some(&fixture.source);
    let manifest = capture_installed_configuration_using_profile(
        &fixture.store,
        &plan,
        roots,
        req,
        collect,
        Some(&verified),
    )
    .unwrap();
    assert_eq!(manifest.files().len(), crate::installed_inventory::PREFERENCES.len());
    assert!(manifest.files().iter().all(|r| r.root == StateRoot::UserHome));
    assert!(manifest.files().iter().any(|r| r.path.as_str() == ".config/ryoku/shell.json"
        && matches!(&r.kind, EntryKind::Symlink { target, .. } if target == "theme.json")));
    let theme =
        manifest.files().iter().find(|r| r.path.as_str() == ".config/ryoku/theme.json").unwrap();
    let EntryKind::RegularFile { metadata, sha256, .. } = &theme.kind else {
        panic!("regular preference required")
    };
    assert_eq!(
        (metadata.uid, metadata.gid, metadata.mode),
        (fixture.account.uid, fixture.account.gid, 0o640)
    );
    assert!(metadata.acl_sha256.is_some());
    assert!(manifest.files().iter().any(|r| matches!(r.kind, EntryKind::Absent)));
    let generation = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
    assert_eq!(generation.read_payload(sha256).unwrap(), b"customized theme");
    for (path, bytes, mode) in [
        (
            crate::installed_inventory::WALLPAPER_STORE[0],
            br#"{"synthetic":{"apply_count":2}}"#.as_slice(),
            0o640,
        ),
        (
            crate::installed_inventory::WALLPAPER_STORE[1],
            br#"{"selected":"synthetic"}"#.as_slice(),
            0o600,
        ),
    ] {
        let record = manifest.files().iter().find(|record| record.path.as_str() == path).unwrap();
        assert!(plan.accepts_record_kind(record));
        let EntryKind::RegularFile { metadata, sha256, .. } = &record.kind else {
            panic!("wallpaper store must be archived as regular files")
        };
        assert_eq!(
            (metadata.uid, metadata.gid, metadata.mode),
            (fixture.account.uid, fixture.account.gid, mode)
        );
        assert_eq!(generation.read_payload(sha256).unwrap(), bytes);
    }
    fs::remove_file(root.join(crate::installed_inventory::PERSONAL_PROFILE)).unwrap();
    let resolved = protected.store.resolve_generation(&generation).unwrap();
    assert_eq!(resolved.settings.density, Some(Density::Compact));
    assert_eq!(resolved.provenance, *manifest.profile_revision.as_ref().unwrap());
    assert_eq!(fs::read(root.join("unrelated-private")).unwrap(), b"must not be archived");
}

#[test]
fn compiled_inventory_rejects_external_links_unsafe_parents_and_typed_upstream_confusion() {
    let fixture = Fixture::new();
    let root = fixture.path.join("source");
    fs::create_dir_all(root.join(".config/ryoku")).unwrap();
    symlink("../../outside", root.join(".config/ryoku/theme.json")).unwrap();
    let plan = InstalledInventory::current().unwrap();
    let mut roots = fixture.roots();
    roots.account_home = Some(&fixture.source);
    assert!(
        capture_installed_configuration_using_profile(
            &fixture.store,
            &plan,
            roots,
            request("unsafe-compiled"),
            collect,
            None
        )
        .is_err()
    );
    fixture.no_publication("unsafe-compiled");
    fs::remove_file(root.join(".config/ryoku/theme.json")).unwrap();
    fs::remove_dir(root.join(".config/ryoku")).unwrap();
    symlink("../", root.join(".config/ryoku")).unwrap();
    let mut roots = fixture.roots();
    roots.account_home = Some(&fixture.source);
    assert!(
        capture_installed_configuration_using_profile(
            &fixture.store,
            &plan,
            roots,
            request("unsafe-parent"),
            collect,
            None
        )
        .is_err()
    );
    fixture.no_publication("unsafe-parent");
}

#[test]
fn lost_writer_lease_before_publication_cannot_create_a_mutation_ready_generation() {
    let fixture = Fixture::new();
    let checks = std::cell::Cell::new(0);
    let result = capture_installed_configuration_checked(
        &fixture.store,
        &plan(),
        fixture.roots(),
        request("lost-writer-lease"),
        collect,
        InstalledCaptureContext::default(),
        || {
            checks.set(checks.get() + 1);
            if checks.get() == 1 { Ok(()) } else { Err(GenerationCaptureError::CoordinationLost) }
        },
    );
    assert!(matches!(result, Err(GenerationCaptureError::CoordinationLost)));
    assert_eq!(checks.get(), 2);
    fixture.no_publication("lost-writer-lease");
}

fn logical_capsule(fixture: &Fixture) -> crate::logical_settings::ThemeSettingsCapsule {
    use crate::logical_settings::*;
    let sha = || crate::Sha256::new("11".repeat(32)).unwrap();
    ThemeSettingsCapsule {
        version: 1,
        native_receipt: crate::theme_settings_actor::test_native_receipt(),
        binding: ThemeSettingsBinding {
            account: fixture.account.principal.clone(),
            uid: fixture.account.uid,
            executable_sha256: sha(),
            schema_sha256: sha(),
            session_service_sha256: sha(),
        },
        approved_policy_sha256: sha(),
        plan_sha256: sha(),
        changes_sha256: sha(),
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

#[test]
fn logical_capture_is_sealed_with_the_file_image_before_generation_publication() {
    let fixture = Fixture::new();
    let capsule = logical_capsule(&fixture);
    let mut req = request("logical-file-image");
    req.snapshot = VerifiedSnapshotLink::from_native_receipt(capsule.native_receipt.clone());
    let calls = std::cell::Cell::new(0);
    let capture = || {
        calls.set(calls.get() + 1);
        Ok(capsule.clone())
    };
    let manifest = capture_installed_configuration_checked(
        &fixture.store,
        &plan(),
        fixture.roots(),
        req,
        collect,
        InstalledCaptureContext { bundle: None, logical_settings: Some(&capture) },
        || Ok(()),
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(manifest.schema_version, 3);
    assert!(!manifest.files().is_empty());
    let reopened = fixture
        .store
        .open_verified_transaction(&Identifier::new("logical-file-image").unwrap())
        .unwrap();
    assert_eq!(reopened.manifest().logical_settings(), Some(&capsule));
    assert_eq!(reopened.manifest().files(), manifest.files());
}

#[test]
fn logical_capture_failure_foreign_binding_or_lease_loss_never_publishes() {
    for fault in 0..4 {
        let fixture = Fixture::new();
        let mut capsule = logical_capsule(&fixture);
        let mut req = request("logical-refusal");
        req.snapshot = VerifiedSnapshotLink::from_native_receipt(capsule.native_receipt.clone());
        if fault == 1 {
            capsule.binding.account =
                crate::AccountPrincipal::from_account(1001, 1001, "other", "/home/other").unwrap();
        } else if fault == 2 {
            let mut receipt = serde_json::to_value(&capsule.native_receipt).unwrap();
            receipt["subvolume_id"] = 301.into();
            capsule.native_receipt = serde_json::from_value(receipt).unwrap();
        }
        let captured = std::cell::Cell::new(false);
        let capture = || {
            captured.set(true);
            if fault == 0 {
                Err(GenerationCaptureError::CoordinationLost)
            } else {
                Ok(capsule.clone())
            }
        };
        let result = capture_installed_configuration_checked(
            &fixture.store,
            &plan(),
            fixture.roots(),
            req,
            collect,
            InstalledCaptureContext { bundle: None, logical_settings: Some(&capture) },
            || {
                if fault == 3 && captured.get() {
                    Err(GenerationCaptureError::CoordinationLost)
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err(), "fault {fault}");
        assert!(captured.get());
        fixture.no_publication("logical-refusal");
    }
}

#[test]
fn lost_writer_lease_during_publication_retracts_generation_and_allows_retry() {
    for fail_at in 3..=6 {
        let fixture = Fixture::new();
        let checks = std::cell::Cell::new(0);
        let result = capture_installed_configuration_checked(
            &fixture.store,
            &plan(),
            fixture.roots(),
            request("publication-lease"),
            collect,
            InstalledCaptureContext::default(),
            || {
                checks.set(checks.get() + 1);
                if checks.get() == fail_at {
                    Err(GenerationCaptureError::CoordinationLost)
                } else {
                    Ok(())
                }
            },
        );
        assert!(
            matches!(result, Err(GenerationCaptureError::CoordinationLost)),
            "check {fail_at}: {result:?}"
        );
        assert_eq!(checks.get(), fail_at);
        fixture.no_publication("publication-lease");
        for directory in ["catalog", "generations", "staging"] {
            assert_eq!(
                fs::read_dir(fixture.path.join("store").join(directory)).unwrap().count(),
                0,
                "failed check {fail_at} left {directory} entries"
            );
        }
        let retried = capture_installed_configuration_using(
            &fixture.store,
            &plan(),
            fixture.roots(),
            request("publication-lease"),
            collect,
        )
        .unwrap();
        assert!(retried.is_mutation_ready());
    }
}

#[test]
fn managed_namespace_changes_after_capture_or_during_publication_are_rejected() {
    use crate::installed_profiles::tests as profile_test;
    for unknown in [".config/ryoku/new-preference.json", ".config/ryogami-wall/new-wall.json"] {
        // Lease checks bracket each namespace scan. Check 3 follows both exact
        // reads; checks 7, 9 and 11 precede publication-boundary namespace scans.
        for change_at in [3, 7, 9, 11] {
            let fixture = Fixture::new();
            let protected = profile_test::Fixture::new();
            let bundle = protected
                .store
                .publish_candidate(
                    &Identifier::new("namespace-inputs").unwrap(),
                    &profile_test::bundle(),
                )
                .unwrap();
            let plan = InstalledInventory::for_home(&fixture.source).unwrap();
            let mut req = request("changed-namespace");
            let package = profile_test::package();
            req.package_revisions = vec![crate::PackageRevision {
                package_name: package.name,
                before_nevra: Some(package.nevra),
                after_nevra: None,
                upstream_revision: package.upstream_revision,
            }];
            let mut roots = fixture.roots();
            roots.account_home = Some(&fixture.source);
            let checks = std::cell::Cell::new(0);
            let result = capture_installed_configuration_checked(
                &fixture.store,
                &plan,
                roots,
                req,
                collect,
                InstalledCaptureContext { bundle: Some(&bundle), logical_settings: None },
                || {
                    checks.set(checks.get() + 1);
                    if checks.get() == change_at {
                        fs::create_dir_all(
                            fixture.path.join("source").join(unknown).parent().unwrap(),
                        )
                        .unwrap();
                        fixture.write(unknown, b"created by an uncoordinated editor", 0o600);
                    }
                    Ok(())
                },
            );
            assert!(
                matches!(
                    result,
                    Err(GenerationCaptureError::Inventory(InventoryError::UnsupportedManagedState))
                ),
                "change at {change_at}: {result:?}"
            );
            assert_eq!(checks.get(), change_at);
            fixture.no_publication("changed-namespace");
            assert_eq!(fs::read_dir(fixture.path.join("store/generations")).unwrap().count(), 0);
        }
    }
}

#[test]
fn absent_personal_profile_still_seals_the_original_account_principal() {
    use crate::installed_profiles::tests as profile_test;
    let fixture = Fixture::new();
    let protected = profile_test::Fixture::new();
    let bundle = protected
        .store
        .publish_candidate(&Identifier::new("import-principal").unwrap(), &profile_test::bundle())
        .unwrap();
    let entry = Enrollment { path: "opensuse-profile.json", ..entries()[0] };
    let plan = InstalledInventory::compile(&["fixture-profile-writer"], &[entry], None).unwrap();
    let mut request = request("absent-principal");
    let package = profile_test::package();
    request.package_revisions = vec![crate::PackageRevision {
        package_name: package.name,
        before_nevra: Some(package.nevra),
        after_nevra: None,
        upstream_revision: package.upstream_revision,
    }];
    let manifest = capture_installed_configuration_using_profile(
        &fixture.store,
        &plan,
        fixture.roots(),
        request,
        collect,
        Some(&bundle),
    )
    .unwrap();
    assert!(matches!(manifest.files()[0].kind, EntryKind::Absent));
    assert_eq!(manifest.account_principal(), Some(&fixture.account.principal));
    let reopened = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
    assert_eq!(reopened.manifest().account_principal(), Some(&fixture.account.principal));
    let changed = crate::AccountPrincipal::from_account(
        fixture.account.uid,
        fixture.account.gid,
        "fixture",
        "/home/reassigned",
    )
    .unwrap();
    let mut untrusted = serde_json::to_value(&manifest).unwrap();
    untrusted["account_principal"] = serde_json::to_value(changed).unwrap();
    assert!(matches!(
        RecoveryManifest::parse_verified_json(&serde_json::to_vec(&untrusted).unwrap()),
        Err(crate::ManifestError::DigestMismatch)
    ));
}

#[test]
fn managed_namespace_and_wallpaper_state_locations_cannot_be_silently_omitted() {
    let fixture = Fixture::new();
    let root = fixture.path.join("source");
    fs::create_dir_all(root.join(".config/ryoku")).unwrap();
    fixture.write(".config/ryoku/ryogami.json", br#"{"paths":{"cache":""}}"#, 0o644);
    assert!(InstalledInventory::for_home(&fixture.source).is_ok());
    let custom_setting = br#"{"paths":{"cache":"/outside"}}"#;
    fixture.write(".config/ryoku/ryogami.json", custom_setting, 0o644);
    assert!(matches!(
        InstalledInventory::for_home(&fixture.source),
        Err(GenerationCaptureError::Inventory(InventoryError::WallpaperStateLocation))
    ));
    assert_eq!(fs::read(root.join(".config/ryoku/ryogami.json")).unwrap(), custom_setting);
    assert!(InventoryError::WallpaperStateLocation.to_string().contains("enrolled"));
    fixture.write(
        ".config/ryoku/ryogami.json",
        br#"{"paths":{"cache":"~/.cache/ryogami"}}"#,
        0o644,
    );
    assert!(InstalledInventory::for_home(&fixture.source).is_ok());
    fixture.write(".config/ryoku/new-preference.json", b"unknown managed preference", 0o644);
    assert!(matches!(
        InstalledInventory::for_home(&fixture.source),
        Err(GenerationCaptureError::Inventory(InventoryError::UnsupportedManagedState))
    ));
    fs::remove_file(root.join(".config/ryoku/new-preference.json")).unwrap();
    fs::create_dir_all(root.join(".config/ryoku/output-profiles")).unwrap();
    fixture.write(
        ".config/ryoku/output-profiles/docked.json",
        b"unsupported dynamic saved profile",
        0o644,
    );
    assert!(InstalledInventory::for_home(&fixture.source).is_err());
}

#[test]
fn plan_fingerprint_binds_complete_home_inventory_content_metadata_and_absence() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path.join("source/.config/ryoku")).unwrap();
    let inventory = InstalledInventory::current().unwrap();
    let absent = inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
    fixture.write(".config/ryoku/shell.json", b"{}", 0o600);
    let present = inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
    assert_ne!(absent, present);
    assert_eq!(present, inventory.fingerprint(&fixture.source, &fixture.account).unwrap());
    fixture.write(".config/ryoku/shell.json", b"{\"changed\":true}", 0o600);
    let changed = inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
    assert_ne!(present, changed);
    fs::set_permissions(
        fixture.path.join("source/.config/ryoku/shell.json"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    assert_ne!(changed, inventory.fingerprint(&fixture.source, &fixture.account).unwrap());
    fixture.write(".config/ryoku/unknown.json", b"{}", 0o600);
    assert!(inventory.fingerprint(&fixture.source, &fixture.account).is_err());
    fs::remove_file(fixture.path.join("source/.config/ryoku/unknown.json")).unwrap();
    fs::remove_file(fixture.path.join("source/.config/ryoku/shell.json")).unwrap();
    assert_eq!(absent, inventory.fingerprint(&fixture.source, &fixture.account).unwrap());
}

#[test]
fn v1_wallpaper_store_is_bound_by_bytes_metadata_and_absence_without_media_enrollment() {
    let fixture = Fixture::new();
    let root = fixture.path.join("source");
    fs::create_dir_all(root.join(".cache/ryogami/wallpaper")).unwrap();
    fs::create_dir_all(root.join("Pictures/Wallpapers")).unwrap();
    fixture.write("Pictures/Wallpapers/private.jpg", b"unrelated media sentinel", 0o600);
    let inventory = InstalledInventory::current().unwrap();
    let absent = inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
    let paths = inventory.paths_for_root(StateRoot::UserHome).unwrap();
    assert!(
        crate::installed_inventory::WALLPAPER_PERSISTENT_STATE
            .iter()
            .all(|path| { paths.iter().any(|enrolled| enrolled.as_str() == *path) })
    );
    assert!(!paths.iter().any(|path| path.as_str().starts_with("Pictures/")));
    for path in crate::installed_inventory::WALLPAPER_PERSISTENT_STATE {
        fixture.write(path, br#"{"selected":"synthetic"}"#, 0o600);
        let present = inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
        assert_ne!(present, absent);
        fixture.write(path, br#"{"selected":"changed"}"#, 0o600);
        let changed = inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
        assert_ne!(present, changed);
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(0o640)).unwrap();
        assert_ne!(changed, inventory.fingerprint(&fixture.source, &fixture.account).unwrap());
        fs::remove_file(root.join(path)).unwrap();
        assert_eq!(absent, inventory.fingerprint(&fixture.source, &fixture.account).unwrap());
        symlink("../../../Pictures/Wallpapers/private.jpg", root.join(path)).unwrap();
        assert!(inventory.fingerprint(&fixture.source, &fixture.account).is_err());
        fs::remove_file(root.join(path)).unwrap();
        fs::hard_link(root.join("Pictures/Wallpapers/private.jpg"), root.join(path)).unwrap();
        assert!(inventory.fingerprint(&fixture.source, &fixture.account).is_err());
        fs::remove_file(root.join(path)).unwrap();
    }
    // Derived caches and other action-owned state share the directory. Exact
    // enrollment neither recurses through nor rejects those independent files.
    fs::create_dir_all(root.join(".cache/ryogami/wallpaper/thumbs")).unwrap();
    fixture.write(".cache/ryogami/wallpaper/thumbs/synthetic.jpg", b"derived image", 0o600);
    fixture.write(".cache/ryogami/wallpaper/playlists.json", b"unrelated action sentinel", 0o600);
    assert!(InstalledInventory::for_home(&fixture.source).is_ok());
    assert_eq!(absent, inventory.fingerprint(&fixture.source, &fixture.account).unwrap());
    assert_eq!(
        fs::read(root.join("Pictures/Wallpapers/private.jpg")).unwrap(),
        b"unrelated media sentinel"
    );
}

#[test]
fn v1_wallpaper_store_accepts_large_catalogues_and_exact_capture_bound() {
    let fixture = Fixture::new();
    let root = fixture.path.join("source");
    fs::create_dir_all(root.join(".cache/ryogami/wallpaper")).unwrap();
    let inventory = InstalledInventory::current().unwrap();
    // The pinned Go Entry schema has twenty-one fields and no record-count cap.
    // This realistic large library deliberately exceeds the old4MiB limit.
    let mut catalogue = serde_json::Map::new();
    for number in 0..10_000 {
        let key = format!("synthetic-wallpaper-{number:05}");
        let path = format!(
            "~/Pictures/Wallpapers/collection-{number:05}/long-descriptive-wallpaper-name-3840x2160.jpg"
        );
        catalogue.insert(
            key.clone(),
            serde_json::json!({
                "key": key, "name": format!("Synthetic wallpaper {number}"), "type": "static",
                "path": path, "thumb": format!("~/.cache/ryogami/wallpaper/thumbs/{key}.jpg"),
                "thumb_sm": format!("~/.cache/ryogami/wallpaper/thumbs-sm/{key}.jpg"),
                "favourite": 0, "hue": 128, "sat": 100, "richness": 100,
                "video_file": "", "video_prev": "", "we_id": "", "we_type": "",
                "tags": "landscape,abstract,synthetic-fixture", "preview": "",
                "filesize": 1_048_576, "width": 3840, "height": 2160, "mtime": 1_700_000_000,
                "apply_count": 0,
            }),
        );
    }
    let bytes = serde_json::to_vec(&catalogue).unwrap();
    assert!(bytes.len() > 4 * 1024 * 1024);
    assert!(bytes.len() as u64 <= crate::installed_inventory::WALLPAPER_STORE_MAX_BYTES);
    fixture.write(crate::installed_inventory::WALLPAPER_STORE[0], &bytes, 0o600);
    inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
    drop(bytes);
    drop(catalogue);
    for path in crate::installed_inventory::WALLPAPER_PERSISTENT_STATE {
        let file = File::create(root.join(path)).unwrap();
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(0o600)).unwrap();
        file.set_len(crate::installed_inventory::WALLPAPER_STORE_MAX_BYTES).unwrap();
        inventory.fingerprint(&fixture.source, &fixture.account).unwrap();
        file.set_len(crate::installed_inventory::WALLPAPER_STORE_MAX_BYTES + 1).unwrap();
        assert!(matches!(inventory.fingerprint(&fixture.source, &fixture.account),
            Err(GenerationCaptureError::Inventory(InventoryError::WallpaperStoreCapacity { file, maximum_bytes }))
            if file == *path && maximum_bytes == crate::installed_inventory::WALLPAPER_STORE_MAX_BYTES
        ));
        file.set_len(0).unwrap();
    }
}
