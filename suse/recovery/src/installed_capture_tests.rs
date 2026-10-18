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
            path: Cow::Borrowed(*path),
            writer: "fixture-profile-writer",
            owner: Owner::Account,
            kind: Kind::Regular,
            modes: &[0o600, 0o640, 0o644],
            max_bytes: 64 * 1024,
            absence_allowed: true,
        })
        .chain([Enrollment {
            root: USER,
            path: Cow::Borrowed("missing.json"),
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
    home: File,
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
        for name in ["source", "home", "store"] {
            fs::create_dir(path.join(name)).unwrap();
            fs::set_permissions(path.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let source = File::open(path.join("source")).unwrap();
        let home = File::open(path.join("home")).unwrap();
        let store =
            GenerationStore::open_for_test(&File::open(path.join("store")).unwrap()).unwrap();
        let stat = fstat(&source).unwrap();
        let result = Self {
            path,
            source,
            home,
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
            user_home: &self.home,
            user_configuration: &self.source,
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
    fn capture_with_bundle(
        &self,
        plan: &InstalledInventory,
        id: &str,
        bundle: &crate::installed_profiles::VerifiedProfileBundle,
    ) -> Result<RecoveryManifest, GenerationCaptureError> {
        let mut capture_request = request(id);
        let package = crate::installed_profiles::tests::package();
        capture_request.package_revisions = vec![crate::PackageRevision {
            package_name: package.name,
            before_nevra: Some(package.nevra),
            after_nevra: None,
            upstream_revision: package.upstream_revision,
        }];
        capture_installed_configuration_using_profile(
            &self.store,
            plan,
            self.roots(),
            capture_request,
            collect,
            Some(bundle),
        )
    }
    fn no_publication(&self, id: &str) {
        assert!(self.store.verify_transaction(&Identifier::new(id).unwrap()).is_err());
        assert_eq!(fs::read_dir(self.path.join("store/generations")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(self.path.join("store/catalog")).unwrap().count(), 0);
    }
}

#[test]
fn session_preference_inventory_captures_only_declared_preferences() {
    let protected = crate::installed_profiles::tests::Fixture::new();
    let bundle = protected
        .store
        .publish_candidate_for_test(
            &Identifier::new("session-profile-bundle").unwrap(),
            &crate::installed_profiles::tests::bundle(),
        )
        .unwrap();
    let fixture = Fixture::new();
    let inventory = InstalledInventory::session_preferences().unwrap();
    let missing_bundle = fixture.capture(&inventory, "session-preferences-no-bundle").unwrap_err();
    assert!(
        matches!(
            &missing_bundle,
            GenerationCaptureError::Inventory(InventoryError::MissingProfileDefaults)
        ),
        "unexpected missing-bundle result: {missing_bundle:?}"
    );
    fixture.no_publication("session-preferences-no-bundle");
    let first =
        fixture.capture_with_bundle(&inventory, "session-preferences-absent", &bundle).unwrap();
    assert_eq!(first.files().len(), 32);
    for (root, path) in [
        (StateRoot::UserRyokuState, ".local/state/ryoku/weather-loc.json"),
        (StateRoot::UserRyokuState, ".local/state/ryoku/weather-cache.json"),
        (StateRoot::UserRyokuState, ".local/state/ryoku/events.json"),
        (StateRoot::UserRyokuState, ".local/state/ryoku/screentime.json"),
        (StateRoot::UserRyokuState, ".local/state/ryoku/stage/progress"),
        (StateRoot::UserRyokuConfig, "plugins.json"),
        (StateRoot::UserRyokuConfig, "desktop.json"),
    ] {
        assert!(
            first.files().iter().all(|record| record.root != root || record.path.as_str() != path),
            "sensitive or unreviewed Ryoku path entered the session checkpoint: {path}"
        );
    }
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuState
            && record.path.as_str() == ".local/state/ryoku/flags.json"
            && matches!(record.kind, EntryKind::Absent)
    }));
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuState
            && record.path.as_str() == ".local/state/ryoku/capture.json"
            && matches!(record.kind, EntryKind::Absent)
    }));
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuState
            && record.path.as_str() == ".local/state/ryoku/record.json"
            && matches!(record.kind, EntryKind::Absent)
    }));
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuState
            && record.path.as_str() == ".local/state/ryoku/nvibrant-value"
            && matches!(record.kind, EntryKind::Absent)
    }));
    for path in [
        ".local/state/ryoku/backlight",
        ".local/state/ryoku/touchpad.disabled",
        ".local/state/ryoku-nightlight",
        ".local/state/ryoku-nightlight-enabled",
        ".local/state/ryoku-nightlight-schedule.json",
        ".local/state/ryoku/stage-walls.json",
    ] {
        assert!(first.files().iter().any(|record| {
            record.root == StateRoot::UserRyokuState
                && record.path.as_str() == path
                && matches!(record.kind, EntryKind::Absent)
        }));
    }
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuConfig
            && record.path.as_str() == "shell.json"
            && matches!(record.kind, EntryKind::Absent)
    }));
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuConfig
            && record.path.as_str() == "matugen.json"
            && matches!(record.kind, EntryKind::Absent)
    }));
    assert!(first.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuConfig
            && record.path.as_str() == "profile.json"
            && matches!(record.kind, EntryKind::Absent)
    }));
    for path in [
        "theme.json",
        "brand.json",
        "launcher.json",
        "widgets.json",
        "performance.json",
        "recording.json",
        "visualizer.json",
        "stage.json",
        "camera.json",
        "ryoshot-beautify.json",
        "ryoshot.json",
        "equalizer.json",
        "power.json",
        "power-profile.json",
        "keypresses.json",
    ] {
        assert!(first.files().iter().any(|record| {
            record.root == StateRoot::UserRyokuConfig
                && record.path.as_str() == path
                && matches!(record.kind, EntryKind::Absent)
        }));
    }

    let flags = fixture.path.join("home/.local/state/ryoku");
    fs::create_dir_all(&flags).unwrap();
    fs::write(flags.join("desktop-notes.txt"), "a private note that is not in the recovery scope")
        .unwrap();
    fs::set_permissions(flags.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(
        flags.parent().unwrap().parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::write(
        flags.join("flags.json"),
        br#"{"dnd":true,"keepAwake":false,"keepAwakeSince":0,"gameMode":false,"gameDndPrev":false}"#,
    )
    .unwrap();
    fs::set_permissions(flags.join("flags.json"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(flags.join("record.json"), br#"{"desktopAudio":true,"mic":false,"edit":false}"#)
        .unwrap();
    fs::set_permissions(flags.join("record.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let vibrance = b"72\n";
    fs::write(flags.join("nvibrant-value"), vibrance).unwrap();
    fs::set_permissions(flags.join("nvibrant-value"), fs::Permissions::from_mode(0o600)).unwrap();
    let device_preferences =
        [("backlight", b"1200".as_slice()), ("touchpad.disabled", b"".as_slice())];
    for (name, payload) in device_preferences {
        fs::write(flags.join(name), payload).unwrap();
        fs::set_permissions(flags.join(name), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let nightlight_preferences = [
        ("ryoku-nightlight", b"4200".as_slice()),
        ("ryoku-nightlight-enabled", b"".as_slice()),
        ("ryoku-nightlight-schedule.json", br#"{"mode":"sun","marginMin":45}"#.as_slice()),
    ];
    for (name, payload) in nightlight_preferences {
        fs::write(fixture.path.join("home/.local/state").join(name), payload).unwrap();
        fs::set_permissions(
            fixture.path.join("home/.local/state").join(name),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let stage_walls = br#"{"current":"/home/alice/Pictures/wall.jpg","walls":{"/home/alice/Pictures/wall.jpg":{"effect":{"enabled":true,"depth":0.4},"layers":[]}}}"#;
    fs::write(flags.join("stage-walls.json"), stage_walls).unwrap();
    fs::set_permissions(flags.join("stage-walls.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let shell_settings = br#"{"theme":{"theme":"Tokyo Night"},"wallpaper":{"enabled":false}}"#;
    fs::write(fixture.path.join("source/shell.json"), shell_settings).unwrap();
    fs::set_permissions(fixture.path.join("source/shell.json"), fs::Permissions::from_mode(0o644))
        .unwrap();
    let matugen_settings = br#"{"mode":"dark","contrast":0.0,"appTheme":false}"#;
    fs::write(fixture.path.join("source/matugen.json"), matugen_settings).unwrap();
    fs::set_permissions(
        fixture.path.join("source/matugen.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let upstream_profile = br#"{"profile":"default","hero":{"kind":"default","source":""}}"#;
    fs::write(fixture.path.join("source/profile.json"), upstream_profile).unwrap();
    fs::set_permissions(
        fixture.path.join("source/profile.json"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let ryoshot_settings = br##"{"blurRadius":48,"mosaicBlock":16,"zoomFactor":1.75,"copyOnSave":true,"saveDir":"/home/alice/Pictures","toolStyle":{"arrow":{"color":"#ffffff","width":3,"filled":false}}}"##;
    fs::write(fixture.path.join("source/ryoshot.json"), ryoshot_settings).unwrap();
    fs::set_permissions(
        fixture.path.join("source/ryoshot.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let power_settings =
        br#"{"profiles":{"balanced":{"governor":"powersave"}},"applyDelayMs":400}"#;
    fs::write(fixture.path.join("source/power.json"), power_settings).unwrap();
    fs::set_permissions(fixture.path.join("source/power.json"), fs::Permissions::from_mode(0o600))
        .unwrap();
    let selected_power_profile = br#"{"profile":"balanced"}"#;
    fs::write(fixture.path.join("source/power-profile.json"), selected_power_profile).unwrap();
    fs::set_permissions(
        fixture.path.join("source/power-profile.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let second =
        fixture.capture_with_bundle(&inventory, "session-preferences-present", &bundle).unwrap();
    let record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuState
                && record.path.as_str() == ".local/state/ryoku/flags.json"
        })
        .unwrap();
    assert!(matches!(record.kind, EntryKind::RegularFile { .. }));
    assert_eq!(second.files().len(), 32);
    let recording_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuState
                && record.path.as_str() == ".local/state/ryoku/record.json"
        })
        .unwrap();
    assert!(matches!(recording_record.kind, EntryKind::RegularFile { .. }));
    let vibrance_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuState
                && record.path.as_str() == ".local/state/ryoku/nvibrant-value"
        })
        .unwrap();
    let EntryKind::RegularFile { metadata, sha256, size, .. } = &vibrance_record.kind else {
        panic!("device vibrance preference was not captured as a regular file");
    };
    assert_eq!(*size, vibrance.len() as u64);
    assert_eq!(metadata.mode & 0o7777, 0o600);
    assert_eq!(metadata.uid, fixture.account.uid);
    assert_eq!(metadata.gid, fixture.account.gid);
    assert_eq!(
        fixture
            .store
            .open_verified_transaction(&second.transaction_id)
            .unwrap()
            .read_payload(sha256)
            .unwrap(),
        vibrance
    );
    let shell_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuConfig && record.path.as_str() == "shell.json"
        })
        .unwrap();
    assert!(
        matches!(shell_record.kind, EntryKind::RegularFile { size, .. } if size == shell_settings.len() as u64)
    );
    let matugen_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuConfig && record.path.as_str() == "matugen.json"
        })
        .unwrap();
    assert!(
        matches!(matugen_record.kind, EntryKind::RegularFile { size, .. } if size == matugen_settings.len() as u64)
    );
    let upstream_profile_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuConfig && record.path.as_str() == "profile.json"
        })
        .unwrap();
    let EntryKind::RegularFile { sha256, size, .. } = &upstream_profile_record.kind else {
        panic!("upstream profile selection was not captured as an opaque file");
    };
    assert_eq!(*size, upstream_profile.len() as u64);
    assert_eq!(
        fixture
            .store
            .open_verified_transaction(&second.transaction_id)
            .unwrap()
            .read_payload(sha256)
            .unwrap(),
        upstream_profile
    );
    let ryoshot_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuConfig && record.path.as_str() == "ryoshot.json"
        })
        .unwrap();
    let EntryKind::RegularFile { metadata, sha256, size, .. } = &ryoshot_record.kind else {
        panic!("Ryoshot preferences were not captured as a regular file");
    };
    assert_eq!(*size, ryoshot_settings.len() as u64);
    assert_eq!(metadata.mode & 0o7777, 0o600);
    assert_eq!(metadata.uid, fixture.account.uid);
    assert_eq!(metadata.gid, fixture.account.gid);
    assert_eq!(
        fixture
            .store
            .open_verified_transaction(&second.transaction_id)
            .unwrap()
            .read_payload(sha256)
            .unwrap(),
        ryoshot_settings
    );
    for (path, expected) in [
        ("power.json", power_settings.as_slice()),
        ("power-profile.json", selected_power_profile.as_slice()),
    ] {
        let record = second
            .files()
            .iter()
            .find(|record| {
                record.root == StateRoot::UserRyokuConfig && record.path.as_str() == path
            })
            .unwrap();
        let EntryKind::RegularFile { metadata, sha256, size, .. } = &record.kind else {
            panic!("power preference was not captured as a regular file: {path}");
        };
        assert_eq!(*size, expected.len() as u64);
        assert_eq!(metadata.mode & 0o7777, 0o600);
        assert_eq!(metadata.uid, fixture.account.uid);
        assert_eq!(metadata.gid, fixture.account.gid);
        assert_eq!(
            fixture
                .store
                .open_verified_transaction(&second.transaction_id)
                .unwrap()
                .read_payload(sha256)
                .unwrap(),
            expected
        );
    }
    let stage_walls_record = second
        .files()
        .iter()
        .find(|record| {
            record.root == StateRoot::UserRyokuState
                && record.path.as_str() == ".local/state/ryoku/stage-walls.json"
        })
        .unwrap();
    let EntryKind::RegularFile { metadata, sha256, size, .. } = &stage_walls_record.kind else {
        panic!("Stage wall layout was not captured as a regular file");
    };
    assert_eq!(*size, stage_walls.len() as u64);
    assert_eq!(metadata.mode & 0o7777, 0o600);
    assert_eq!(metadata.uid, fixture.account.uid);
    assert_eq!(metadata.gid, fixture.account.gid);
    assert_eq!(
        fixture
            .store
            .open_verified_transaction(&second.transaction_id)
            .unwrap()
            .read_payload(sha256)
            .unwrap(),
        stage_walls
    );
    for (path, expected) in [
        (".local/state/ryoku/backlight", b"1200".as_slice()),
        (".local/state/ryoku/touchpad.disabled", b"".as_slice()),
        (".local/state/ryoku-nightlight", b"4200".as_slice()),
        (".local/state/ryoku-nightlight-enabled", b"".as_slice()),
        (
            ".local/state/ryoku-nightlight-schedule.json",
            br#"{"mode":"sun","marginMin":45}"#.as_slice(),
        ),
    ] {
        let record = second
            .files()
            .iter()
            .find(|record| record.root == StateRoot::UserRyokuState && record.path.as_str() == path)
            .unwrap();
        let EntryKind::RegularFile { metadata, sha256, size, .. } = &record.kind else {
            panic!("device preference was not captured as a regular file: {path}");
        };
        assert_eq!(*size, expected.len() as u64);
        assert_eq!(metadata.mode & 0o7777, 0o600);
        assert_eq!(metadata.uid, fixture.account.uid);
        assert_eq!(metadata.gid, fixture.account.gid);
        assert_eq!(
            fixture
                .store
                .open_verified_transaction(&second.transaction_id)
                .unwrap()
                .read_payload(sha256)
                .unwrap(),
            expected
        );
    }
    assert!(second.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuState
            && record.path.as_str() == ".local/state/ryoku"
            && matches!(record.kind, EntryKind::Directory { .. })
    }));
    assert!(!second.files().iter().any(|record| record.path.as_str().contains("clipboard")));
    assert!(!second.files().iter().any(|record| {
        record.root == StateRoot::UserRyokuState
            && record.path.as_str() == ".local/state/ryoku/desktop-notes.txt"
    }));

    let unsafe_fixture = Fixture::new();
    let outside = unsafe_fixture.path.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, unsafe_fixture.path.join("home/.local")).unwrap();
    let error = unsafe_fixture
        .capture_with_bundle(&inventory, "session-preferences-symlink", &bundle)
        .unwrap_err();
    assert!(
        matches!(error, GenerationCaptureError::Capture(CaptureError::UnsupportedEntryType)),
        "unexpected symlink result: {error:?}"
    );
    unsafe_fixture.no_publication("session-preferences-symlink");
}

#[test]
fn manual_session_checkpoint_seals_fixed_inventory_and_preserves_raw_profile_bytes() {
    let fixture = Fixture::new();
    let profile_fixture = crate::installed_profiles::tests::Fixture::new();
    let profile_package = crate::installed_profiles::tests::package();
    let bundle = profile_fixture
        .store
        .publish_candidate_for_test(
            &Identifier::new("manual-session-profile-bundle").unwrap(),
            &crate::installed_profiles::tests::bundle(),
        )
        .unwrap();
    assert!(fixture.store.open_snapper_catalog_store().is_err());
    fixture.store.initialize_snapper_catalog().unwrap();
    assert!(
        fixture
            .store
            .open_snapper_catalog_store()
            .unwrap()
            .load()
            .unwrap()
            .entries()
            .next()
            .is_none()
    );
    fixture.write(ryoku_profiles::LOCAL_PROFILE_FILE, b"{invalid current profile", 0o600);
    fixture.write("profile.json", br#"{"hero":{"kind":"custom","source":"hero.png"}}"#, 0o644);
    let hero_dir = fixture.path.join("source/profile");
    fs::create_dir(&hero_dir).unwrap();
    fs::set_permissions(&hero_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let hero_bytes = b"bounded custom hero fixture";
    fs::write(hero_dir.join("hero.png"), hero_bytes).unwrap();
    fs::set_permissions(hero_dir.join("hero.png"), fs::Permissions::from_mode(0o600)).unwrap();
    let manifest = capture_session_preference_checkpoint(
        &fixture.store,
        fixture.roots(),
        SessionPreferenceCheckpointRequest {
            transaction_id: Identifier::new("manual-session-checkpoint").unwrap(),
            generation_id: Identifier::new("manual-session-generation").unwrap(),
            snapshot: crate::generation_capture::VerifiedSnapshotLink::from_preverified_link(
                crate::SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            ),
            created_utc: "2026-10-18T00:00:00Z".into(),
            package_revisions: vec![crate::PackageRevision {
                package_name: profile_package.name,
                before_nevra: Some(profile_package.nevra),
                after_nevra: None,
                upstream_revision: profile_package.upstream_revision,
            }],
            reason: crate::OperationReason::Other(Identifier::new("manual-checkpoint").unwrap()),
        },
        &bundle,
    )
    .unwrap();

    assert_eq!(manifest.account_principal(), Some(&fixture.account.principal));
    assert_eq!(manifest.files().len(), 34);
    let generation = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
    let profile = manifest
        .files()
        .iter()
        .find(|record| record.path.as_str() == ryoku_profiles::LOCAL_PROFILE_FILE)
        .unwrap();
    let sha256 = match &profile.kind {
        EntryKind::RegularFile { sha256, .. } => sha256,
        _ => panic!("profile file should be captured as raw bytes"),
    };
    assert_eq!(generation.read_payload(sha256).unwrap(), b"{invalid current profile");
    let hero =
        manifest.files().iter().find(|record| record.path.as_str() == "profile/hero.png").unwrap();
    let EntryKind::RegularFile { sha256, size, .. } = &hero.kind else {
        panic!("selected custom hero was not captured as a file");
    };
    assert_eq!(*size, hero_bytes.len() as u64);
    assert_eq!(generation.read_payload(sha256).unwrap(), hero_bytes);
    assert!(manifest.files().iter().any(|record| {
        record.path.as_str() == "profile" && matches!(record.kind, EntryKind::Directory { .. })
    }));

    let unsafe_fixture = Fixture::new();
    unsafe_fixture.write(
        "profile.json",
        br#"{"hero":{"kind":"custom","source":"hero.png/../../outside"}}"#,
        0o644,
    );
    let package = crate::installed_profiles::tests::package();
    let error = capture_session_preference_checkpoint(
        &unsafe_fixture.store,
        unsafe_fixture.roots(),
        SessionPreferenceCheckpointRequest {
            transaction_id: Identifier::new("manual-session-unsafe-hero").unwrap(),
            generation_id: Identifier::new("manual-session-unsafe-hero-generation").unwrap(),
            snapshot: VerifiedSnapshotLink::from_preverified_link(
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            ),
            created_utc: "2026-10-18T00:00:00Z".into(),
            package_revisions: vec![crate::PackageRevision {
                package_name: package.name,
                before_nevra: Some(package.nevra),
                after_nevra: None,
                upstream_revision: package.upstream_revision,
            }],
            reason: crate::OperationReason::Other(Identifier::new("unsafe-hero").unwrap()),
        },
        &bundle,
    )
    .unwrap_err();
    assert!(matches!(error, GenerationCaptureError::Inventory(InventoryError::InvalidProfile)));
    unsafe_fixture.no_publication("manual-session-unsafe-hero");

    unsafe_fixture.write(
        "profile.json",
        br#"{"hero":{"kind":"custom","source":"hero.png"}}"#,
        0o644,
    );
    let package = crate::installed_profiles::tests::package();
    let error = capture_session_preference_checkpoint(
        &unsafe_fixture.store,
        unsafe_fixture.roots(),
        SessionPreferenceCheckpointRequest {
            transaction_id: Identifier::new("manual-session-missing-hero").unwrap(),
            generation_id: Identifier::new("manual-session-missing-hero-generation").unwrap(),
            snapshot: VerifiedSnapshotLink::from_preverified_link(
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            ),
            created_utc: "2026-10-18T00:00:00Z".into(),
            package_revisions: vec![crate::PackageRevision {
                package_name: package.name,
                before_nevra: Some(package.nevra),
                after_nevra: None,
                upstream_revision: package.upstream_revision,
            }],
            reason: crate::OperationReason::Other(Identifier::new("missing-hero").unwrap()),
        },
        &bundle,
    )
    .unwrap_err();
    assert!(matches!(error, GenerationCaptureError::Inventory(InventoryError::EntryPolicy)));
    unsafe_fixture.no_publication("manual-session-missing-hero");
}

#[test]
fn oversized_pinned_settings_fail_before_generation_publication() {
    let protected = crate::installed_profiles::tests::Fixture::new();
    let bundle = protected
        .store
        .publish_candidate_for_test(
            &Identifier::new("oversize-settings-profile-bundle").unwrap(),
            &crate::installed_profiles::tests::bundle(),
        )
        .unwrap();
    let fixture = Fixture::new();
    let settings = fixture.path.join("source/shell.json");
    fs::write(&settings, vec![b' '; 1024 * 1024 + 1]).unwrap();
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        fixture.capture_with_bundle(
            &InstalledInventory::session_preferences().unwrap(),
            "oversized-shell-settings",
            &bundle,
        ),
        Err(GenerationCaptureError::Inventory(InventoryError::EntryPolicy))
    ));
    fixture.no_publication("oversized-shell-settings");

    let profile = fixture.path.join("source/profile.json");
    fs::write(&profile, vec![b' '; 1024 * 1024 + 1]).unwrap();
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        fixture.capture_with_bundle(
            &InstalledInventory::session_preferences().unwrap(),
            "oversized-upstream-profile",
            &bundle,
        ),
        Err(GenerationCaptureError::Inventory(InventoryError::EntryPolicy))
    ));
    fixture.no_publication("oversized-upstream-profile");
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
    assert!(matches!(InstalledInventory::current(), Err(InventoryError::NoAuditedWriters)));
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
        candidates[0].path = Cow::Owned(invalid.to_owned());
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
    candidates.push(candidates[0].clone());
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
            path: Cow::Borrowed("directory"),
            writer: "fixture-settings-writer",
            owner: Owner::Account,
            kind: Kind::Directory,
            modes: &[0o700],
            max_bytes: 1,
            absence_allowed: false,
        },
        Enrollment {
            root: USER,
            path: Cow::Borrowed("link"),
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
        path: Cow::Borrowed("fixture-system.json"),
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
        .publish_candidate_for_test(
            &Identifier::new("import-bundle").unwrap(),
            &profile_test::bundle(),
        )
        .unwrap();
    fixture.write(
        ryoku_profiles::LOCAL_PROFILE_FILE,
        br#"{"schema_version":1,"id":"generic-desktop","settings":{"density":"compact"}}"#,
        0o600,
    );
    let entry = Enrollment {
        path: Cow::Borrowed(ryoku_profiles::LOCAL_PROFILE_FILE),
        ..entries()[0].clone()
    };
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
    assert_eq!(manifest.files()[0].path.as_str(), ryoku_profiles::LOCAL_PROFILE_FILE);
    assert!(manifest.files().iter().all(|file| file.root == USER));
    let provenance = manifest.profile_revision.as_ref().unwrap();
    assert!(provenance.profile_revision.as_str().starts_with("bundle-sha256-"));
    let reopened = protected.store.load_revision(&provenance.profile_revision).unwrap();
    let personal =
        fs::read(fixture.path.join("source").join(ryoku_profiles::LOCAL_PROFILE_FILE)).unwrap();
    assert_eq!(reopened.bundle().resolve(Some(&personal)).unwrap().provenance, *provenance);
    assert!(reopened.bundle().matches_packages(&manifest.package_revisions));
    let generation = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
    fs::remove_file(fixture.path.join("source").join(ryoku_profiles::LOCAL_PROFILE_FILE)).unwrap();
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
        .publish_candidate_for_test(
            &Identifier::new("import-bundle").unwrap(),
            &profile_test::bundle(),
        )
        .unwrap();
    let entry = Enrollment {
        path: Cow::Borrowed(ryoku_profiles::LOCAL_PROFILE_FILE),
        ..entries()[0].clone()
    };
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
fn absent_personal_profile_still_seals_the_original_account_principal() {
    use crate::installed_profiles::tests as profile_test;
    let fixture = Fixture::new();
    let protected = profile_test::Fixture::new();
    let bundle = protected
        .store
        .publish_candidate_for_test(
            &Identifier::new("import-principal").unwrap(),
            &profile_test::bundle(),
        )
        .unwrap();
    let entry = Enrollment {
        path: Cow::Borrowed(ryoku_profiles::LOCAL_PROFILE_FILE),
        ..entries()[0].clone()
    };
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
fn personal_writer_scope_covers_absence_and_guard_failure_prevents_publication() {
    use crate::installed_profiles::tests as profile_test;
    let fixture = Fixture::new();
    let protected = profile_test::Fixture::new();
    let verified = protected
        .store
        .publish_candidate_for_test(
            &Identifier::new("scoped-import").unwrap(),
            &profile_test::bundle(),
        )
        .unwrap();
    let plan = InstalledInventory::personal_profile_writer().unwrap();
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].writer, ryoku_profiles::PERSONAL_PROFILE_WRITER_ID);
    assert_eq!(plan.entries[0].path, ryoku_profiles::LOCAL_PROFILE_FILE);
    assert!(matches!(InstalledInventory::current(), Err(InventoryError::NoAuditedWriters)));
    let mut request = request("scoped-refused");
    let package = profile_test::package();
    request.package_revisions = vec![crate::PackageRevision {
        package_name: package.name,
        before_nevra: Some(package.nevra),
        after_nevra: None,
        upstream_revision: package.upstream_revision,
    }];
    let calls = std::cell::Cell::new(0);
    let captured = std::cell::Cell::new(0);
    let result = capture_installed_configuration_guarded(
        &fixture.store,
        &plan,
        fixture.roots(),
        request,
        |root, state, paths, limits| {
            captured.set(captured.get() + 1);
            assert_eq!(paths, [RelativePath::new(ryoku_profiles::LOCAL_PROFILE_FILE).unwrap()]);
            let inventory = collect(root, state, paths, limits)?;
            assert!(matches!(inventory.records[0].kind, EntryKind::Absent));
            Ok(inventory)
        },
        Some(&verified),
        || {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                assert_eq!(captured.get(), 2);
                Err(GenerationCaptureError::VerificationMismatch)
            } else {
                Ok(())
            }
        },
    );
    assert!(matches!(result, Err(GenerationCaptureError::VerificationMismatch)));
    assert_eq!(calls.get(), 2);
    fixture.no_publication("scoped-refused");
}
