//! These run real installed GIO/GSettings, with a private explicit memory
//! backend and fixture schema. They never contact the desktop bus or DConf.
use super::*;
use ryoku_recovery::theme_settings_actor::{apply, capture, restore_changes};
use ryoku_recovery::{AccountPrincipal, Sha256};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};
fn sha(byte: &str) -> Sha256 {
    Sha256::new(byte.repeat(32)).unwrap()
}
fn test_native_receipt() -> ryoku_recovery::native_checkpoint::RootSnapshotReceipt {
    ryoku_recovery::RecoveryManifest::parse_verified_json(include_bytes!(
        "../../recovery/tests/fixtures/issue176-logical/legacy-v2.json"
    ))
    .unwrap()
    .root_snapshot_receipt()
    .unwrap()
    .clone()
}
fn actor() -> GioThemeSettingsActor {
    let binding = ThemeSettingsBinding {
        account: AccountPrincipal::from_account(1000, 1000, "fixture", "/home/fixture").unwrap(),
        uid: 1000,
        executable_sha256: sha("ab"),
        schema_sha256: sha("cd"),
        session_service_sha256: sha("ef"),
    };
    GioThemeSettingsActor::memory(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/issue176-gsettings").as_path(),
        binding,
    )
    .unwrap()
}
fn image(
    actor: &mut GioThemeSettingsActor,
    changes: &BTreeMap<ThemeSettingKey, ThemeSettingChange>,
) -> ryoku_recovery::logical_settings::ThemeSettingsCapsule {
    capture(actor, &actor.expected.clone(), &test_native_receipt(), sha("11"), sha("22"), changes)
        .unwrap()
}

#[test]
fn real_gio_preserves_unset_vs_explicit_default_and_restores_exact_keys() {
    let mut actor = actor();
    assert_eq!(actor.read(ThemeSettingKey::GtkTheme).unwrap().user_value, None);
    actor.replace_user_value(ThemeSettingKey::GtkTheme, Some("Adwaita")).unwrap();
    actor.synchronize().unwrap();
    let changes = [
        (ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into())),
        (ThemeSettingKey::GtkTheme, ThemeSettingChange::NudgeGtk("User Theme".into())),
    ]
    .into();
    let before = image(&mut actor, &changes);
    assert_eq!(before.keys[&ThemeSettingKey::ColorScheme].user_value, None);
    assert_eq!(before.keys[&ThemeSettingKey::GtkTheme].user_value, Some("Adwaita".into()));
    let binding = actor.expected.clone();
    let observed = apply(&mut actor, &before, &binding, &sha("11"), &sha("22"), &changes).unwrap();
    assert_eq!(observed.after[&ThemeSettingKey::GtkTheme].user_value, Some("User Theme".into()));
    let restore = restore_changes(
        &before,
        &binding,
        &[ThemeSettingKey::ColorScheme, ThemeSettingKey::GtkTheme],
    )
    .unwrap();
    let current = image(&mut actor, &restore);
    apply(&mut actor, &current, &binding, &sha("11"), &sha("22"), &restore).unwrap();
    assert_eq!(actor.read(ThemeSettingKey::ColorScheme).unwrap().user_value, None);
    assert_eq!(actor.read(ThemeSettingKey::GtkTheme).unwrap().user_value, Some("Adwaita".into()));
    // The unrelated schema key is observed through the real API and remains
    // untouched; it has no variant in the actor's writable key enum.
    let font = actor
        .api
        .string(unsafe { (actor.api.get)(actor.settings.as_ptr(), c"font-name".as_ptr()) })
        .unwrap();
    assert_eq!(font, "User Font 11");
}

#[test]
fn real_gio_handles_literal_string_values_without_shell_or_variant_text_parsing() {
    let mut actor = actor();
    let text = "User's \"Blue\" \\theme\n next";
    let changes = [(ThemeSettingKey::GtkTheme, ThemeSettingChange::NudgeGtk(text.into()))].into();
    let before = image(&mut actor, &changes);
    let binding = actor.expected.clone();
    let observed = apply(&mut actor, &before, &binding, &sha("11"), &sha("22"), &changes).unwrap();
    assert_eq!(observed.after[&ThemeSettingKey::GtkTheme].user_value, Some(text.into()));
}

#[test]
fn real_gio_range_refusal_precedes_every_write_in_the_batch() {
    let mut actor = actor();
    let changes = [
        (ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("not-a-valid-scheme".into())),
        (ThemeSettingKey::GtkTheme, ThemeSettingChange::NudgeGtk("User Theme".into())),
    ]
    .into();
    let binding = actor.expected.clone();
    assert!(
        capture(&mut actor, &binding, &test_native_receipt(), sha("11"), sha("22"), &changes)
            .is_err()
    );
    assert_eq!(actor.read(ThemeSettingKey::ColorScheme).unwrap().user_value, None);
    assert_eq!(actor.read(ThemeSettingKey::GtkTheme).unwrap().user_value, None);
}

#[test]
fn real_gio_guard_loss_prevents_setting_or_restore_dispatch() {
    let mut actor = actor();
    let changes =
        [(ThemeSettingKey::GtkTheme, ThemeSettingChange::NudgeGtk("User Theme".into()))].into();
    let before = image(&mut actor, &changes);
    let binding = actor.expected.clone();
    let active = Rc::new(Cell::new(true));
    let guard_active = Rc::clone(&active);
    let expected = binding.clone();
    actor.guard = Box::new(move || {
        if guard_active.get() {
            Ok(expected.clone())
        } else {
            Err(io::Error::other("authenticated actor lease expired"))
        }
    });
    active.set(false);
    let error = apply(&mut actor, &before, &binding, &sha("11"), &sha("22"), &changes).unwrap_err();
    assert!(error.attempted.is_empty());
    active.set(true);
    assert_eq!(actor.read(ThemeSettingKey::GtkTheme).unwrap().user_value, None);
}

#[test]
fn missing_exact_schema_bundle_never_falls_back_to_process_defaults() {
    let binding = actor().expected.clone();
    let missing = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/issue176-gsettings/absent-schema-directory");
    let error = GioThemeSettingsActor::memory(&missing, binding).err().unwrap();
    assert!(error.to_string().contains("exact pinned schema bundle"));
}

#[test]
fn dummy_memory_backend_cannot_attest_native_dconf_state() {
    let api = Api::open().unwrap();
    let backend = unsafe { (api.memory_backend)() };
    assert!(!backend.is_null());
    let error = api.require_dconf_backend(backend).unwrap_err();
    assert!(error.to_string().contains("refusing memory/alternate backend fallback"));
    unsafe {
        (api.object_unref)(backend);
    }
    assert!(api.require_dconf_backend(std::ptr::null_mut()).is_err());
}

#[test]
fn actual_dconf_backend_is_identified_in_a_private_account_environment() {
    const ROLE: &str = "RYOKU_THEME_DCONF_TYPE_FIXTURE";
    const TEST: &str =
        "theme_gio::tests::actual_dconf_backend_is_identified_in_a_private_account_environment";
    if std::env::var_os(ROLE).is_some() {
        let api = Api::open().unwrap();
        let backend = unsafe { (api.default_backend)() };
        let verified = api.require_dconf_backend(backend);
        if !backend.is_null() {
            unsafe {
                (api.object_unref)(backend);
            }
        }
        verified.expect("the declared native DConf module must be installed");
        return;
    }
    use std::{
        os::unix::fs::DirBuilderExt,
        process::Command,
        time::{Duration, Instant},
    };
    let home = std::env::temp_dir().join(format!("ryoku-dconf-type-{}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
    for name in ["config", "run", "cache"] {
        std::fs::DirBuilder::new().mode(0o700).create(home.join(name)).unwrap();
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env_clear()
        .env(ROLE, "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_RUNTIME_DIR", home.join("run"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/run/absent-bus", home.display()))
        .env("GSETTINGS_BACKEND", "dconf")
        .env("LANG", "C")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            std::fs::remove_dir_all(&home).unwrap();
            panic!("private native-backend fixture exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    std::fs::remove_dir_all(home).unwrap();
    assert!(status.success());
}
