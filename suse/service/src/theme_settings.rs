//! Key-level GSettings transaction semantics for the retained theme worker.
//!
//! These data and primitives grant no execution authority. The concrete actor
//! must be bounded/authenticated independently, and the caller must enroll the
//! capsule in its linked pre/post generation and retain all writer holds. An
//! observation returned here is not a durable completion receipt.
#[cfg(test)]
use ryoku_recovery::AccountPrincipal;
use ryoku_recovery::Sha256;
pub use ryoku_recovery::logical_settings::*;
use std::{collections::BTreeMap, io};

#[cfg(test)]
pub(crate) fn test_native_receipt() -> ryoku_recovery::native_checkpoint::RootSnapshotReceipt {
    ryoku_recovery::RecoveryManifest::parse_verified_json(include_bytes!(
        "../../recovery/tests/fixtures/issue176-logical/legacy-v2.json"
    ))
    .unwrap()
    .root_snapshot_receipt()
    .unwrap()
    .clone()
}

/// Implementations provide a fixed-key backend; no schema/path/command text
/// comes from the UI. A None value means reset the user override, not Set("").
pub trait ThemeSettingsActor {
    fn binding(&mut self) -> io::Result<ThemeSettingsBinding>;
    fn check_lifetime(&mut self) -> io::Result<()>;
    fn read(&mut self, key: ThemeSettingKey) -> io::Result<ThemeSettingState>;
    /// Validate every desired value (and GTK placeholder) against the pinned
    /// backend schema before any key in the batch is changed.
    fn validate_change(
        &mut self,
        key: ThemeSettingKey,
        change: &ThemeSettingChange,
    ) -> io::Result<()>;
    fn replace_user_value(&mut self, key: ThemeSettingKey, value: Option<&str>) -> io::Result<()>;
    /// Complete the backend's write acknowledgement within its enforced deadline.
    fn synchronize(&mut self) -> io::Result<()>;
}

#[derive(Debug)]
pub struct ThemeSettingsFailure {
    pub attempted: Vec<ThemeSettingKey>,
    pub reason: String,
}
impl ThemeSettingsFailure {
    fn before(reason: impl ToString) -> Self {
        Self { attempted: vec![], reason: reason.to_string() }
    }
}
impl std::fmt::Display for ThemeSettingsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "logical settings refused: {}; attempted={:?}; inspect linked recovery",
            self.reason, self.attempted
        )
    }
}
impl std::error::Error for ThemeSettingsFailure {}

#[derive(Debug)]
pub struct ThemeSettingsObservation {
    pub capsule_sha256: Sha256,
    pub attempted: Vec<ThemeSettingKey>,
    pub after: BTreeMap<ThemeSettingKey, ThemeSettingState>,
}

pub fn capture(
    actor: &mut (impl ThemeSettingsActor + ?Sized),
    expected: &ThemeSettingsBinding,
    native_receipt: &ryoku_recovery::native_checkpoint::RootSnapshotReceipt,
    approved_policy_sha256: Sha256,
    plan_sha256: Sha256,
    changes: &BTreeMap<ThemeSettingKey, ThemeSettingChange>,
) -> Result<ThemeSettingsCapsule, ThemeSettingsFailure> {
    native_receipt.validate().map_err(ThemeSettingsFailure::before)?;
    let changes_sha256 = changes_digest(changes).map_err(ThemeSettingsFailure::before)?;
    check_actor(actor, expected).map_err(ThemeSettingsFailure::before)?;
    let mut image = BTreeMap::new();
    for key in changes.keys() {
        actor.validate_change(*key, &changes[key]).map_err(ThemeSettingsFailure::before)?;
        let value = actor.read(*key).map_err(ThemeSettingsFailure::before)?;
        if !value.validate() || image.insert(*key, value).is_some() {
            return Err(ThemeSettingsFailure::before("invalid or duplicate logical key state"));
        }
    }
    // A second complete read detects a changed key while the image was built;
    // this does not replace the caller's writer isolation or linked capture.
    revalidate(actor, expected, &image).map_err(ThemeSettingsFailure::before)?;
    Ok(ThemeSettingsCapsule {
        version: 1,
        native_receipt: native_receipt.clone(),
        binding: expected.clone(),
        approved_policy_sha256,
        plan_sha256,
        changes_sha256,
        keys: image,
    })
}

fn check_actor(
    actor: &mut (impl ThemeSettingsActor + ?Sized),
    expected: &ThemeSettingsBinding,
) -> io::Result<()> {
    actor.check_lifetime()?;
    if !expected.validate() || actor.binding()? != *expected {
        return Err(io::Error::other("settings account/runtime/schema/session identity changed"));
    }
    Ok(())
}
fn revalidate(
    actor: &mut (impl ThemeSettingsActor + ?Sized),
    binding: &ThemeSettingsBinding,
    keys: &BTreeMap<ThemeSettingKey, ThemeSettingState>,
) -> io::Result<()> {
    check_actor(actor, binding)?;
    for (key, before) in keys {
        if actor.read(*key)? != *before {
            return Err(io::Error::other("settings preimage changed"));
        }
    }
    check_actor(actor, binding)
}

pub fn apply(
    actor: &mut (impl ThemeSettingsActor + ?Sized),
    before: &ThemeSettingsCapsule,
    expected: &ThemeSettingsBinding,
    approved_policy_sha256: &Sha256,
    plan_sha256: &Sha256,
    changes: &BTreeMap<ThemeSettingKey, ThemeSettingChange>,
) -> Result<ThemeSettingsObservation, ThemeSettingsFailure> {
    if !before.validate()
        || before.binding != *expected
        || before.approved_policy_sha256 != *approved_policy_sha256
        || before.plan_sha256 != *plan_sha256
        || changes_digest(changes).as_ref().ok() != Some(&before.changes_sha256)
        || before.keys.keys().ne(changes.keys())
        || changes.iter().any(|(key, change)| !change.validate(*key))
        || before.keys.values().any(|value| !value.writable)
    {
        return Err(ThemeSettingsFailure::before(
            "account/policy/plan/key scope or key writability mismatch",
        ));
    }
    revalidate(actor, expected, &before.keys).map_err(ThemeSettingsFailure::before)?;
    for (key, change) in changes {
        actor.validate_change(*key, change).map_err(ThemeSettingsFailure::before)?;
    }
    let digest = before.digest().map_err(ThemeSettingsFailure::before)?;
    let mut attempted = Vec::new();
    let mut run = || -> io::Result<BTreeMap<ThemeSettingKey, ThemeSettingState>> {
        for (key, change) in changes {
            check_actor(actor, expected)?;
            // Mark before issuing an operation whose failure can be ambiguous.
            attempted.push(*key);
            if let ThemeSettingChange::NudgeGtk(want) = change {
                let temporary = if want == "Adwaita" { "Adwaita-dark" } else { "Adwaita" };
                actor.replace_user_value(*key, Some(temporary))?;
                actor.synchronize()?;
                check_actor(actor, expected)?;
            }
            let (value, _) = change.target(&before.keys[key]);
            actor.replace_user_value(*key, value)?;
            actor.synchronize()?;
        }
        let mut after = BTreeMap::new();
        for (key, change) in changes {
            check_actor(actor, expected)?;
            let observed = actor.read(*key)?;
            let (user, effective) = change.target(&before.keys[key]);
            if !observed.validate()
                || observed.user_value.as_deref() != user
                || observed.effective_value != effective
                || observed.default_value != before.keys[key].default_value
            {
                return Err(io::Error::other(
                    "logical key read-back did not match the exact requested state",
                ));
            }
            after.insert(*key, observed);
        }
        check_actor(actor, expected)?;
        Ok(after)
    };
    match run() {
        Ok(after) => Ok(ThemeSettingsObservation { capsule_sha256: digest, attempted, after }),
        Err(error) => Err(ThemeSettingsFailure { attempted, reason: error.to_string() }),
    }
}

/// Restore only these exact original user overrides. The caller must first
/// checkpoint current keys and approve restore; the old capsule is not approval.
pub fn restore_changes(
    source: &ThemeSettingsCapsule,
    current_binding: &ThemeSettingsBinding,
    selected_keys: &[ThemeSettingKey],
) -> Result<BTreeMap<ThemeSettingKey, ThemeSettingChange>, ThemeSettingsFailure> {
    let selected = selected_keys.iter().copied().collect::<std::collections::BTreeSet<_>>();
    if !source.validate()
        || !current_binding.validate()
        || source.binding.account != current_binding.account
        || source.binding.uid != current_binding.uid
        || selected.len() != selected_keys.len()
        || selected.len() != source.keys.len()
        || source.keys.keys().copied().ne(selected)
    {
        return Err(ThemeSettingsFailure::before(
            "invalid source logical capsule/account/selected key scope",
        ));
    }
    Ok(source
        .keys
        .iter()
        .map(|(key, state)| {
            (
                *key,
                state.user_value.as_ref().map_or(ThemeSettingChange::Reset, |value| {
                    ThemeSettingChange::Set(value.clone())
                }),
            )
        })
        .collect())
}

#[cfg(test)]
#[path = "theme_settings_tests.rs"]
mod tests;
