//! These run real installed GIO/GSettings, with a private explicit memory
//! backend and fixture schema. They never contact the desktop bus or DConf.
use super::*;
use crate::theme_settings::{apply, capture, restore_changes, test_native_receipt};
use ryoku_recovery::{AccountPrincipal, Sha256};
use std::{cell::Cell, collections::BTreeMap, rc::Rc};
fn sha(byte: &str) -> Sha256 {
    Sha256::new(byte.repeat(32)).unwrap()
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
) -> crate::theme_settings::ThemeSettingsCapsule {
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
