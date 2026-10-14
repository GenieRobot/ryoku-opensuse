//! Typed, local profile layers. File loading requires an explicit directory
//! descriptor and uses one fixed, no-follow file name; the crate has no
//! ambient path or network access.
//!
//! Callers load and validate each layer at an outer boundary, then resolve
//! them with [`ProfileLayers::resolve`]. A companion layer is optional local
//! input; resolving a profile never fetches it or requires credentials.
//!
//! A synthetic local resolution requires only the layer values supplied by
//! the caller:
//!
//! ```
//! use ryoku_profiles::{Desktop, Density, ProfileLayers, SettingsLayer, Theme};
//!
//! let resolved = ProfileLayers {
//!     defaults: SettingsLayer {
//!         desktop: Some(Desktop::Hyprland),
//!         theme: Some(Theme::RyokuDark),
//!         density: Some(Density::Comfortable),
//!     },
//!     system_overrides: SettingsLayer {
//!         desktop: Some(Desktop::Kde),
//!         ..SettingsLayer::default()
//!     },
//!     companion: Some(SettingsLayer {
//!         theme: Some(Theme::System),
//!         ..SettingsLayer::default()
//!     }),
//!     personal: SettingsLayer {
//!         density: Some(Density::Compact),
//!         ..SettingsLayer::default()
//!     },
//! }
//! .resolve();
//!
//! assert_eq!(resolved.desktop, Some(Desktop::Kde));
//! assert_eq!(resolved.theme, Some(Theme::System));
//! assert_eq!(resolved.density, Some(Density::Compact));
//! ```

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

mod local;
pub use local::{LOCAL_PROFILE_FILE, ProfileLoadError, ProfileOwner, load_local_profile};

/// The only profile document schema currently accepted by this crate.
pub const PROFILE_SCHEMA_VERSION: u16 = 1;
/// Bound local profile input before parsing it.
pub const MAX_PROFILE_DOCUMENT_BYTES: usize = 64 * 1024;

/// Stable identifier for a locally installed profile.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    /// Accepts lowercase ASCII letters/digits separated by single hyphens.
    pub fn new(value: impl Into<String>) -> Result<Self, ProfileIdError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && value.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !value.contains("--");
        if valid { Ok(Self(value)) } else { Err(ProfileIdError) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileIdError;

impl std::fmt::Display for ProfileIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid profile identifier")
    }
}

impl std::error::Error for ProfileIdError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Desktop {
    Hyprland,
    Kde,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    RyokuDark,
    RyokuLight,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Compact,
    Comfortable,
}

/// A partial layer. `None` means the layer makes no statement about that key.
/// Values are enums so profile data cannot inject paths, commands, or scripts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsLayer {
    pub desktop: Option<Desktop>,
    pub theme: Option<Theme>,
    pub density: Option<Density>,
}

/// On-disk local profile format. It contains declarative values only; it has
/// no paths, commands, scripts, or remote references.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDocument {
    pub schema_version: u16,
    pub id: ProfileId,
    #[serde(default)]
    pub settings: SettingsLayer,
}

impl ProfileDocument {
    pub fn new(id: ProfileId, settings: SettingsLayer) -> Self {
        Self { schema_version: PROFILE_SCHEMA_VERSION, id, settings }
    }

    /// Parse a bounded local document. Older schema versions are accepted
    /// only after an explicit migration is implemented; unknown versions fail
    /// closed rather than being interpreted using today's field meanings.
    pub fn parse(bytes: &[u8]) -> Result<Self, ProfileDocumentError> {
        if bytes.len() > MAX_PROFILE_DOCUMENT_BYTES {
            return Err(ProfileDocumentError::InputTooLarge);
        }
        let document: Self = serde_json::from_slice(bytes)
            .map_err(|error| ProfileDocumentError::Serialization(error.to_string()))?;
        if document.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileDocumentError::UnsupportedSchemaVersion(document.schema_version));
        }
        Ok(document)
    }

    /// Serialize in stable struct-field order with no nondeterministic maps.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProfileDocumentError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileDocumentError::UnsupportedSchemaVersion(self.schema_version));
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ProfileDocumentError::Serialization(error.to_string()))?;
        if bytes.len() > MAX_PROFILE_DOCUMENT_BYTES {
            return Err(ProfileDocumentError::InputTooLarge);
        }
        Ok(bytes)
    }

    pub fn layer(&self) -> SettingsLayer {
        self.settings
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileDocumentError {
    InputTooLarge,
    UnsupportedSchemaVersion(u16),
    Serialization(String),
}

impl std::fmt::Display for ProfileDocumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge => write!(formatter, "profile document exceeds size limit"),
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "unsupported profile schema version {version}")
            }
            Self::Serialization(message) => {
                write!(formatter, "invalid profile document: {message}")
            }
        }
    }
}

impl std::error::Error for ProfileDocumentError {}

/// Inputs are applied in the fixed order declared by the public contract:
/// profile defaults, system overrides, optional companion, personal overrides.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProfileLayers {
    pub defaults: SettingsLayer,
    pub system_overrides: SettingsLayer,
    pub companion: Option<SettingsLayer>,
    pub personal: SettingsLayer,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResolvedSettings {
    pub desktop: Option<Desktop>,
    pub theme: Option<Theme>,
    pub density: Option<Density>,
}

impl ProfileLayers {
    /// Resolve each key independently; a missing higher-layer key inherits
    /// the most recent lower-layer value.
    pub fn resolve(self) -> ResolvedSettings {
        let mut result = ResolvedSettings::default();
        result.apply(self.defaults);
        result.apply(self.system_overrides);
        if let Some(companion) = self.companion {
            result.apply(companion);
        }
        result.apply(self.personal);
        result
    }

    /// Return stable content revisions for the exact local layers used by a
    /// resolution. Revisions are SHA-256 of each layer's canonical typed JSON;
    /// absent optional layers remain absent. This records provenance without
    /// embedding profile payloads or consulting a remote source.
    pub fn content_revisions(self) -> ProfileLayerRevisions {
        ProfileLayerRevisions {
            defaults: layer_revision(self.defaults),
            system_overrides: Some(layer_revision(self.system_overrides)),
            companion: self.companion.map(layer_revision),
            personal_overrides: Some(layer_revision(self.personal)),
        }
    }
}

/// Content-derived revisions for the four profile resolution layers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileLayerRevisions {
    pub defaults: String,
    pub system_overrides: Option<String>,
    pub companion: Option<String>,
    pub personal_overrides: Option<String>,
}

fn layer_revision(layer: SettingsLayer) -> String {
    // SettingsLayer consists only of closed enums and options; serde's struct
    // field order is fixed, so this encoding is canonical and map-free.
    let canonical = serde_json::to_vec(&layer).expect("typed settings serialize");
    let digest = Sha256::digest(canonical);
    let mut revision = String::with_capacity(67);
    revision.push_str("sha256-");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut revision, "{byte:02x}").expect("writing to String cannot fail");
    }
    revision
}

impl ResolvedSettings {
    fn apply(&mut self, layer: SettingsLayer) {
        if layer.desktop.is_some() {
            self.desktop = layer.desktop;
        }
        if layer.theme.is_some() {
            self.theme = layer.theme;
        }
        if layer.density.is_some() {
            self.density = layer.density;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_layers_override_per_key_and_companion_is_optional() {
        let layers = ProfileLayers {
            defaults: SettingsLayer {
                desktop: Some(Desktop::Hyprland),
                theme: Some(Theme::RyokuDark),
                density: Some(Density::Comfortable),
            },
            system_overrides: SettingsLayer {
                desktop: Some(Desktop::Kde),
                ..SettingsLayer::default()
            },
            companion: Some(SettingsLayer {
                theme: Some(Theme::System),
                ..SettingsLayer::default()
            }),
            personal: SettingsLayer { density: Some(Density::Compact), ..SettingsLayer::default() },
        };
        assert_eq!(
            layers.resolve(),
            ResolvedSettings {
                desktop: Some(Desktop::Kde),
                theme: Some(Theme::System),
                density: Some(Density::Compact),
            }
        );

        let without_companion = ProfileLayers { companion: None, ..layers };
        assert_eq!(without_companion.resolve().theme, Some(Theme::RyokuDark));
    }

    #[test]
    fn layer_content_revisions_are_stable_and_track_effective_inputs() {
        let base = ProfileLayers {
            defaults: SettingsLayer { theme: Some(Theme::RyokuDark), ..SettingsLayer::default() },
            ..ProfileLayers::default()
        };
        let first = base.content_revisions();
        let repeated = base.content_revisions();
        assert_eq!(first, repeated);
        assert_eq!(first.defaults.len(), 71);
        assert!(first.defaults.starts_with("sha256-"));
        assert_eq!(first.companion, None);

        let changed = ProfileLayers {
            defaults: SettingsLayer { theme: Some(Theme::RyokuLight), ..SettingsLayer::default() },
            ..ProfileLayers::default()
        }
        .content_revisions();
        assert_ne!(first.defaults, changed.defaults);

        let installed_companion =
            ProfileLayers { companion: Some(SettingsLayer::default()), ..ProfileLayers::default() }
                .content_revisions();
        assert!(installed_companion.companion.is_some());
    }

    #[test]
    fn profile_id_rejects_path_like_and_noncanonical_values() {
        assert_eq!(ProfileId::new("desktop-default").unwrap().as_str(), "desktop-default");
        assert!(ProfileId::new("../outside").is_err());
        assert!(ProfileId::new("Uppercase").is_err());
        assert!(ProfileId::new("double--dash").is_err());
    }

    #[test]
    fn versioned_document_round_trips_with_stable_bytes() {
        let document = ProfileDocument::new(
            ProfileId::new("personal-desktop").unwrap(),
            SettingsLayer {
                desktop: Some(Desktop::Kde),
                theme: Some(Theme::RyokuDark),
                density: None,
            },
        );
        let bytes = document.canonical_bytes().unwrap();
        assert_eq!(
            bytes,
            br#"{"schema_version":1,"id":"personal-desktop","settings":{"desktop":"kde","theme":"ryoku_dark","density":null}}"#
        );
        let decoded = ProfileDocument::parse(&bytes).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(decoded.layer(), document.settings);
    }

    #[test]
    fn document_rejects_unknown_versions_fields_and_unsafe_ids() {
        assert_eq!(
            ProfileDocument::parse(br#"{"schema_version":2,"id":"x","settings":{}}"#),
            Err(ProfileDocumentError::UnsupportedSchemaVersion(2))
        );
        assert_eq!(
            ProfileDocument::parse(br#"{"schema_version":0,"id":"x","settings":{}}"#),
            Err(ProfileDocumentError::UnsupportedSchemaVersion(0))
        );
        let mut unsupported =
            ProfileDocument::new(ProfileId::new("x").unwrap(), SettingsLayer::default());
        unsupported.schema_version = 2;
        assert_eq!(
            unsupported.canonical_bytes(),
            Err(ProfileDocumentError::UnsupportedSchemaVersion(2))
        );
        assert!(
            ProfileDocument::parse(br#"{"schema_version":1,"id":"x","settings":{"command":"sh"}}"#)
                .is_err()
        );
        assert!(
            ProfileDocument::parse(br#"{"schema_version":1,"id":"../outside","settings":{}}"#)
                .is_err()
        );
        assert_eq!(
            ProfileDocument::parse(&vec![b' '; MAX_PROFILE_DOCUMENT_BYTES + 1]),
            Err(ProfileDocumentError::InputTooLarge)
        );
    }
}
