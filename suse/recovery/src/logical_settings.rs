//! Separately versioned key-level settings recovery data. These records grant no execution authority.
use crate::{AccountPrincipal, Sha256};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};
#[cfg(test)]
#[path = "logical_settings_tests.rs"]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFile {
    pub path: String,
    pub sha256: Sha256,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionWitness {
    pub device: u64,
    pub inode: u64,
    pub uid: u32,
    pub process: crate::wallpaper_protocol::ActorIdentity,
    pub executable_sha256: Sha256,
}
impl SessionWitness {
    pub fn digest(&self) -> io::Result<Sha256> {
        Sha256::new(crate::wallpaper_protocol::digest(&serde_json::to_vec(self)?))
            .map_err(|_| io::Error::other("invalid session witness digest"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeSettingKey {
    ColorScheme,
    AccentColor,
    GtkTheme,
    IconTheme,
}
impl ThemeSettingKey {
    pub fn name(self) -> &'static str {
        match self {
            Self::ColorScheme => "color-scheme",
            Self::AccentColor => "accent-color",
            Self::GtkTheme => "gtk-theme",
            Self::IconTheme => "icon-theme",
        }
    }
    pub const SCHEMA: &str = "org.gnome.desktop.interface";
}

/// The user override must remain distinct from the effective/default value.
/// Restoring a formerly unset key requires Reset, even if Get returned a value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSettingState {
    pub user_value: Option<String>,
    pub effective_value: String,
    pub default_value: String,
    pub writable: bool,
}
impl ThemeSettingState {
    pub fn validate(&self) -> bool {
        self.user_value.as_deref().is_none_or(valid_string)
            && valid_string(&self.effective_value)
            && valid_string(&self.default_value)
    }
}
fn valid_string(value: &str) -> bool {
    value.len() <= 4096 && !value.contains('\0')
}

/// Identity of the reviewed actor, schema bundle and exact session service.
/// Each actor operation must recheck its OS-backed identity/lifetime bindings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSettingsBinding {
    pub account: AccountPrincipal,
    pub uid: u32,
    pub executable_sha256: Sha256,
    pub schema_sha256: Sha256,
    pub session_service_sha256: Sha256,
}
impl ThemeSettingsBinding {
    pub fn validate(&self) -> bool {
        self.uid != 0 && self.uid != u32::MAX
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThemeSettingChange {
    Set(String),
    Reset,
    NudgeGtk(String),
}
impl ThemeSettingChange {
    pub fn validate(&self, key: ThemeSettingKey) -> bool {
        match self {
            Self::Reset => true,
            Self::Set(value) => valid_string(value),
            Self::NudgeGtk(value) => {
                key == ThemeSettingKey::GtkTheme && !value.is_empty() && valid_string(value)
            }
        }
    }
    pub fn target<'a>(&'a self, before: &'a ThemeSettingState) -> (Option<&'a str>, &'a str) {
        match self {
            Self::Set(value) | Self::NudgeGtk(value) => (Some(value), value),
            Self::Reset => (None, &before.default_value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSettingsCapsule {
    pub version: u16,
    /// Exact native pre-checkpoint associated with this key image. This is
    /// structural linkage; protected provenance and kernel availability are
    /// independently verified by the mutation coordinator.
    pub native_receipt: crate::native_checkpoint::RootSnapshotReceipt,
    pub binding: ThemeSettingsBinding,
    pub approved_policy_sha256: Sha256,
    pub plan_sha256: Sha256,
    pub changes_sha256: Sha256,
    #[serde(deserialize_with = "deserialize_key_states")]
    pub keys: BTreeMap<ThemeSettingKey, ThemeSettingState>,
}
fn deserialize_key_states<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<ThemeSettingKey, ThemeSettingState>, D::Error> {
    deserialize_key_map(deserializer)
}

/// Preserve the exact-key invariant while decoding a private worker request;
/// ordinary BTreeMap decoding would silently replace duplicate declarations.
pub fn deserialize_setting_changes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<ThemeSettingKey, ThemeSettingChange>, D::Error> {
    deserialize_key_map(deserializer)
}

fn deserialize_key_map<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<BTreeMap<ThemeSettingKey, T>, D::Error> {
    struct UniqueKeys<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for UniqueKeys<T> {
        type Value = BTreeMap<ThemeSettingKey, T>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique exact theme-setting keys")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut keys = BTreeMap::new();
            while let Some((key, value)) = map.next_entry()? {
                if keys.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate logical settings key"));
                }
                if keys.len() > 4 {
                    return Err(serde::de::Error::custom("oversized logical settings key set"));
                }
            }
            Ok(keys)
        }
    }
    deserializer.deserialize_map(UniqueKeys(std::marker::PhantomData))
}

pub fn changes_digest(
    changes: &BTreeMap<ThemeSettingKey, ThemeSettingChange>,
) -> io::Result<Sha256> {
    if changes.is_empty()
        || changes.len() > 4
        || changes.iter().any(|(key, change)| !change.validate(*key))
    {
        return Err(io::Error::other("invalid exact desired logical settings"));
    }
    Sha256::new(crate::wallpaper_protocol::digest(&serde_json::to_vec(changes)?))
        .map_err(|_| io::Error::other("invalid desired settings digest"))
}
impl ThemeSettingsCapsule {
    pub fn validate(&self) -> bool {
        self.version == 1
            && self.native_receipt.validate().is_ok()
            && self.binding.validate()
            && !self.keys.is_empty()
            && self.keys.len() <= 4
            && self.keys.values().all(ThemeSettingState::validate)
    }
    pub fn digest(&self) -> io::Result<Sha256> {
        if !self.validate() {
            return Err(io::Error::other("invalid logical settings capsule"));
        }
        Sha256::new(crate::wallpaper_protocol::digest(&serde_json::to_vec(self)?))
            .map_err(|_| io::Error::other("invalid logical settings digest"))
    }
}
