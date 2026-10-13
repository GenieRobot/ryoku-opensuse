//! Typed, local profile layers. This crate has no filesystem or network access.
//!
//! Callers load and validate each layer at an outer boundary, then resolve
//! them with [`ProfileLayers::resolve`]. A companion layer is optional local
//! input; resolving a profile never fetches it or requires credentials.

/// Stable identifier for a locally installed profile.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileIdError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Desktop {
    Hyprland,
    Kde,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Theme {
    RyokuDark,
    RyokuLight,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Density {
    Compact,
    Comfortable,
}

/// A partial layer. `None` means the layer makes no statement about that key.
/// Values are enums so profile data cannot inject paths, commands, or scripts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SettingsLayer {
    pub desktop: Option<Desktop>,
    pub theme: Option<Theme>,
    pub density: Option<Density>,
}

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
    fn profile_id_rejects_path_like_and_noncanonical_values() {
        assert_eq!(ProfileId::new("desktop-default").unwrap().as_str(), "desktop-default");
        assert!(ProfileId::new("../outside").is_err());
        assert!(ProfileId::new("Uppercase").is_err());
        assert!(ProfileId::new("double--dash").is_err());
    }
}
