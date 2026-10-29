use super::*;

fn sha(byte: &str) -> Sha256 {
    Sha256::new(byte.repeat(32)).unwrap()
}
struct Actor {
    identity: ThemeSettingsBinding,
    values: BTreeMap<ThemeSettingKey, ThemeSettingState>,
    unrelated: BTreeMap<String, String>,
    writes: Vec<(ThemeSettingKey, Option<String>)>,
    alive: bool,
    expire_after_write: bool,
    fail_sync: bool,
    lie_after_write: bool,
}
impl Actor {
    fn new() -> Self {
        Self {
            identity: ThemeSettingsBinding {
                account: AccountPrincipal::from_account(1000, 1000, "fixture", "/home/fixture")
                    .unwrap(),
                uid: 1000,
                executable_sha256: sha("ab"),
                schema_sha256: sha("cd"),
                session_service_sha256: sha("ef"),
            },
            values: [
                (ThemeSettingKey::ColorScheme, state(None, "default")),
                (ThemeSettingKey::GtkTheme, state(Some("Adwaita"), "Adwaita")),
                (ThemeSettingKey::AccentColor, state(Some("blue"), "blue")),
                (ThemeSettingKey::IconTheme, state(None, "Adwaita")),
            ]
            .into(),
            unrelated: [
                ("font-name".into(), "User Font 11".into()),
                ("unrelated-application".into(), "keep me".into()),
            ]
            .into(),
            writes: vec![],
            alive: true,
            expire_after_write: false,
            fail_sync: false,
            lie_after_write: false,
        }
    }
    fn capture(
        &mut self,
        changes: &BTreeMap<ThemeSettingKey, ThemeSettingChange>,
    ) -> ThemeSettingsCapsule {
        capture(self, &self.identity.clone(), &test_native_receipt(), sha("11"), sha("22"), changes)
            .unwrap()
    }
    fn apply(
        &mut self,
        capsule: &ThemeSettingsCapsule,
        changes: &BTreeMap<ThemeSettingKey, ThemeSettingChange>,
    ) -> Result<ThemeSettingsObservation, ThemeSettingsFailure> {
        apply(
            self,
            capsule,
            &capsule.binding,
            &capsule.approved_policy_sha256,
            &capsule.plan_sha256,
            changes,
        )
    }
}
fn state(user: Option<&str>, default: &str) -> ThemeSettingState {
    ThemeSettingState {
        user_value: user.map(str::to_string),
        effective_value: user.unwrap_or(default).into(),
        default_value: default.into(),
        writable: true,
    }
}
impl ThemeSettingsActor for Actor {
    fn binding(&mut self) -> io::Result<ThemeSettingsBinding> {
        Ok(self.identity.clone())
    }
    fn check_lifetime(&mut self) -> io::Result<()> {
        if self.alive {
            Ok(())
        } else {
            Err(io::Error::other("actor exited/cancelled or its writer hold failed"))
        }
    }
    fn read(&mut self, key: ThemeSettingKey) -> io::Result<ThemeSettingState> {
        Ok(self.values[&key].clone())
    }
    fn validate_change(
        &mut self,
        _key: ThemeSettingKey,
        _change: &ThemeSettingChange,
    ) -> io::Result<()> {
        Ok(())
    }
    fn replace_user_value(&mut self, key: ThemeSettingKey, value: Option<&str>) -> io::Result<()> {
        self.writes.push((key, value.map(str::to_string)));
        let entry = self.values.get_mut(&key).unwrap();
        entry.user_value = value.map(str::to_string);
        entry.effective_value = if self.lie_after_write {
            "wrong read-back".into()
        } else {
            value.unwrap_or(&entry.default_value).into()
        };
        if self.expire_after_write {
            self.alive = false;
        }
        Ok(())
    }
    fn synchronize(&mut self) -> io::Result<()> {
        if self.fail_sync {
            Err(io::Error::other("backend acknowledgement missing"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn unset_and_explicit_default_restore_differ_and_unrelated_keys_survive() {
    let mut actor = Actor::new();
    let unrelated = actor.unrelated.clone();
    let keys = [ThemeSettingKey::ColorScheme, ThemeSettingKey::GtkTheme];
    let changes = [
        (ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into())),
        (ThemeSettingKey::GtkTheme, ThemeSettingChange::Set("User Theme".into())),
    ]
    .into();
    let original = actor.capture(&changes);
    assert_eq!(original.keys[&ThemeSettingKey::ColorScheme].user_value, None);
    assert_eq!(original.keys[&ThemeSettingKey::GtkTheme].user_value, Some("Adwaita".into()));
    let observation = actor.apply(&original, &changes).unwrap();
    assert_eq!(observation.after.len(), 2);
    let restore = restore_changes(&original, &actor.identity, &keys).unwrap();
    let current = actor.capture(&restore);
    assert_eq!(restore[&ThemeSettingKey::ColorScheme], ThemeSettingChange::Reset);
    assert_eq!(restore[&ThemeSettingKey::GtkTheme], ThemeSettingChange::Set("Adwaita".into()));
    actor.apply(&current, &restore).unwrap();
    assert_eq!(
        actor.values[&ThemeSettingKey::ColorScheme],
        original.keys[&ThemeSettingKey::ColorScheme]
    );
    assert_eq!(actor.values[&ThemeSettingKey::GtkTheme], original.keys[&ThemeSettingKey::GtkTheme]);
    assert_eq!(actor.unrelated, unrelated);
}

#[test]
fn gtk_nudge_uses_the_original_real_placeholder_then_exact_target() {
    for target in ["Adwaita", "User Theme"] {
        let mut actor = Actor::new();
        let changes =
            [(ThemeSettingKey::GtkTheme, ThemeSettingChange::NudgeGtk(target.into()))].into();
        let before = actor.capture(&changes);
        actor.apply(&before, &changes).unwrap();
        assert_eq!(
            actor.writes,
            vec![
                (
                    ThemeSettingKey::GtkTheme,
                    Some(if target == "Adwaita" { "Adwaita-dark" } else { "Adwaita" }.into())
                ),
                (ThemeSettingKey::GtkTheme, Some(target.into()))
            ]
        );
    }
}

#[test]
fn changed_account_runtime_schema_service_policy_plan_or_preimage_never_writes() {
    for field in 0..8 {
        let mut actor = Actor::new();
        let changes =
            [(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into()))].into();
        let before = actor.capture(&changes);
        let mut expected = before.binding.clone();
        let mut policy = before.approved_policy_sha256.clone();
        let mut plan = before.plan_sha256.clone();
        match field {
            0 => expected.uid += 1,
            1 => {
                expected.account =
                    AccountPrincipal::from_account(1001, 1001, "other", "/home/other").unwrap()
            }
            2 => actor.identity.executable_sha256 = sha("01"),
            3 => actor.identity.schema_sha256 = sha("02"),
            4 => actor.identity.session_service_sha256 = sha("03"),
            5 => policy = sha("04"),
            6 => plan = sha("05"),
            _ => {
                actor.values.get_mut(&ThemeSettingKey::ColorScheme).unwrap().user_value =
                    Some("concurrent".into())
            }
        }
        let error = apply(
            &mut actor,
            &before,
            &expected,
            &policy,
            &plan,
            &[(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into()))]
                .into(),
        )
        .unwrap_err();
        assert!(error.attempted.is_empty());
        assert!(actor.writes.is_empty());
    }
}

#[test]
fn unknown_keys_or_ambiguous_scope_cannot_reach_an_actor_write() {
    assert!(serde_json::from_str::<ThemeSettingKey>("\"font-name\"").is_err());
    let mut actor = Actor::new();
    let approved =
        [(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into()))].into();
    let before = actor.capture(&approved);
    for changes in [
        BTreeMap::new(),
        [(ThemeSettingKey::GtkTheme, ThemeSettingChange::Set("Adwaita".into()))].into(),
        [(ThemeSettingKey::ColorScheme, ThemeSettingChange::NudgeGtk("Adwaita".into()))].into(),
    ] {
        assert!(actor.apply(&before, &changes).unwrap_err().attempted.is_empty());
    }
    let duplicate = serde_json::to_string(&before).unwrap().replace(
        "\"keys\":{",
        &format!(
            "\"keys\":{{\"color-scheme\":{},",
            serde_json::to_string(&before.keys[&ThemeSettingKey::ColorScheme]).unwrap()
        ),
    );
    assert!(serde_json::from_str::<ThemeSettingsCapsule>(&duplicate).is_err());
    assert!(actor.writes.is_empty());
}

#[test]
fn read_only_key_refuses_whole_batch_before_first_write() {
    let mut actor = Actor::new();
    actor.values.get_mut(&ThemeSettingKey::GtkTheme).unwrap().writable = false;
    let changes = [
        (ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into())),
        (ThemeSettingKey::GtkTheme, ThemeSettingChange::Set("User Theme".into())),
    ]
    .into();
    let before = actor.capture(&changes);
    assert!(actor.apply(&before, &changes).unwrap_err().attempted.is_empty());
    assert!(actor.writes.is_empty());
}

#[test]
fn missing_ack_bad_readback_and_actor_loss_never_attest_completion() {
    for fault in 0..3 {
        let mut actor = Actor::new();
        let changes =
            [(ThemeSettingKey::GtkTheme, ThemeSettingChange::NudgeGtk("User Theme".into()))].into();
        let before = actor.capture(&changes);
        match fault {
            0 => actor.fail_sync = true,
            1 => actor.lie_after_write = true,
            _ => actor.expire_after_write = true,
        }
        let error = actor.apply(&before, &changes).unwrap_err();
        assert_eq!(error.attempted, vec![ThemeSettingKey::GtkTheme]);
        assert!(!actor.writes.is_empty());
        assert!(error.to_string().contains("linked recovery"));
        if fault != 1 {
            assert_eq!(actor.writes.len(), 1);
        }
    }
}

#[test]
fn key_capsule_is_bound_to_unset_state_and_value_limits() {
    let mut actor = Actor::new();
    let changes =
        [(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into()))].into();
    let mut before = actor.capture(&changes);
    let digest = before.digest().unwrap();
    before.keys.get_mut(&ThemeSettingKey::ColorScheme).unwrap().user_value = Some("default".into());
    assert_ne!(digest, before.digest().unwrap());
    before.keys.get_mut(&ThemeSettingKey::ColorScheme).unwrap().effective_value = "x".repeat(4097);
    assert!(!before.validate());
    assert!(restore_changes(&before, &actor.identity, &[ThemeSettingKey::ColorScheme]).is_err());
}

#[test]
fn changed_desired_values_and_foreign_or_wider_restore_scope_are_refused() {
    let mut actor = Actor::new();
    let approved =
        [(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into()))].into();
    let before = actor.capture(&approved);
    let changed =
        [(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-dark".into()))].into();
    assert!(actor.apply(&before, &changed).unwrap_err().attempted.is_empty());
    let mut foreign = actor.identity.clone();
    foreign.uid = 1001;
    foreign.account = AccountPrincipal::from_account(1001, 1001, "other", "/home/other").unwrap();
    assert!(restore_changes(&before, &foreign, &[ThemeSettingKey::ColorScheme]).is_err());
    assert!(
        restore_changes(
            &before,
            &actor.identity,
            &[ThemeSettingKey::ColorScheme, ThemeSettingKey::GtkTheme]
        )
        .is_err()
    );
    assert!(
        restore_changes(
            &before,
            &actor.identity,
            &[ThemeSettingKey::ColorScheme, ThemeSettingKey::ColorScheme]
        )
        .is_err()
    );
    assert!(actor.writes.is_empty());
}
