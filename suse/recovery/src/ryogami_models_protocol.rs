//! Closed model-reference intents and prepared native actor commits for #175.
//! Prepared commits are helper-only data, never accepted as public intents.
use crate::{
    Sha256,
    wallpaper_protocol::{MAX_REQUEST_BYTES, WallpaperProtocolError, bounded_line},
};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, WallpaperProtocolError>;
pub const MAX_MODELS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelReferenceIntent {
    Add { path: Option<String> },
    Forget { index: u32 },
}
impl ModelReferenceIntent {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Add { path: Some(path) } => validate_manifest_path(path, false),
            Self::Add { path: None } => Ok(()),
            Self::Forget { index } if (*index as usize) < MAX_MODELS => Ok(()),
            _ => Err(WallpaperProtocolError::InvalidFrame),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReference {
    pub name: String,
    pub manifest: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}
impl ModelReference {
    pub fn validate(&self) -> Result<()> {
        if self.name.len() > 256
            || self.name.chars().any(char::is_control)
            || self.id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            })
            || self.version.as_ref().is_some_and(|value| {
                value.is_empty() || value.len() > 128 || value.chars().any(char::is_control)
            })
            || self.dimensions.is_some_and(|dimensions| dimensions == 0 || dimensions > 65_536)
            || self.bytes.is_some_and(|bytes| bytes > 8 * 1024 * 1024 * 1024)
            || (self.managed && self.id.is_none())
        {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        validate_manifest_path(&self.manifest, true)
    }
}

/// Exact helper-prepared model list with an unchanged selection. Other settings and credentials
/// are preserved by the actor's expected-file-digest checked transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReferenceCommit {
    pub expected_config: Option<Sha256>,
    pub staging_token: Sha256,
    pub models: Vec<ModelReference>,
    pub expected_selected: String,
}
impl ModelReferenceCommit {
    pub fn validate(&self) -> Result<()> {
        if self.models.len() > MAX_MODELS {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        for model in &self.models {
            model.validate()?;
        }
        validate_manifest_path(&self.expected_selected, true)?;
        Ok(())
    }
    pub fn staging_name(&self) -> String {
        format!(".ryoku-model-{}.part", self.staging_token.as_str())
    }
    pub fn staging_asset(
        &self,
    ) -> std::result::Result<crate::ryogami_assets::AssetPath, crate::ryogami_assets::AssetError>
    {
        crate::ryogami_assets::AssetPath::new(
            crate::ryogami_assets::AssetNamespace::ModelReferenceStage,
            crate::RelativePath::new(self.staging_name())
                .map_err(|_| crate::ryogami_assets::AssetError::Path)?,
        )
    }
    pub fn frame(&self, id: i64) -> Result<Vec<u8>> {
        if id < 0 {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        self.validate()?;
        let mut bytes = serde_json::to_vec(&CommitFrame {
            method: "broker.model-references".into(),
            params: self.clone(),
            id,
        })
        .map_err(|_| WallpaperProtocolError::InvalidFrame)?;
        bytes.push(b'\n');
        bounded_line(&bytes, MAX_REQUEST_BYTES)?;
        Ok(bytes)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitFrame {
    method: String,
    params: ModelReferenceCommit,
    id: i64,
}
pub fn parse_commit(bytes: &[u8]) -> Result<(i64, ModelReferenceCommit)> {
    bounded_line(bytes, MAX_REQUEST_BYTES)?;
    let frame: CommitFrame =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if frame.method != "broker.model-references" || frame.id < 0 {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    frame.params.validate()?;
    Ok((frame.id, frame.params))
}

/// Decode only the existing picker action envelope. No prepared list, root,
/// temporary name, arbitrary settings key or checkpoint field is accepted.
pub fn parse_public_intent(bytes: &[u8]) -> Result<(i64, ModelReferenceIntent)> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Frame {
        method: String,
        params: Params,
        id: i64,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        id: String,
        #[serde(default)]
        args: serde_json::Value,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Add {
        #[serde(default)]
        path: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Forget {
        index: u32,
    }
    bounded_line(bytes, MAX_REQUEST_BYTES)?;
    let frame: Frame =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if frame.method != "settings.action" || frame.id < 0 {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    let args = if frame.params.args.is_null() { serde_json::json!({}) } else { frame.params.args };
    let intent = match frame.params.id.as_str() {
        "AddSemanticModel" => {
            let args: Add =
                serde_json::from_value(args).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
            ModelReferenceIntent::Add {
                path: args.path.and_then(|path| {
                    let path = path.trim();
                    (!path.is_empty()).then(|| path.to_owned())
                }),
            }
        }
        "RemoveSemanticModel" => {
            let args: Forget =
                serde_json::from_value(args).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
            ModelReferenceIntent::Forget { index: args.index }
        }
        _ => return Err(WallpaperProtocolError::InvalidFrame),
    };
    intent.validate()?;
    Ok((frame.id, intent))
}
fn validate_manifest_path(path: &str, empty_allowed: bool) -> Result<()> {
    if empty_allowed && path.is_empty() {
        return Ok(());
    }
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || !(path.starts_with('/') || path.starts_with("~/"))
        || path.contains('\\')
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_add_and_forget_intents_are_strict_and_do_not_accept_commits() {
        assert_eq!(parse_public_intent(b"{\"method\":\"settings.action\",\"params\":{\"id\":\"AddSemanticModel\",\"args\":{}},\"id\":4}\n").unwrap(),
            (4, ModelReferenceIntent::Add { path: None }));
        assert_eq!(parse_public_intent(b"{\"method\":\"settings.action\",\"params\":{\"id\":\"RemoveSemanticModel\",\"args\":{\"index\":1}},\"id\":5}\n").unwrap(),
            (5, ModelReferenceIntent::Forget { index: 1 }));
        for extra in ["command", "models", "root", "staging_token", "checkpoint", "credentials"] {
            let frame = serde_json::json!({"method":"settings.action","params":{"id":"AddSemanticModel","args":{extra:"injected"}},"id":1});
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            assert!(parse_public_intent(&bytes).is_err());
        }
    }
    #[test]
    fn prepared_commit_roundtrip_binds_exact_fields_but_is_never_a_public_intent() {
        let commit = ModelReferenceCommit {
            expected_config: None,
            staging_token: Sha256::new("a".repeat(64)).unwrap(),
            models: vec![ModelReference {
                name: "Fixture".into(),
                manifest: "~/Models/fixture.json".into(),
                managed: false,
                id: None,
                version: None,
                dimensions: None,
                bytes: None,
            }],
            expected_selected: String::new(),
        };
        let frame = commit.frame(7).unwrap();
        assert_eq!(parse_commit(&frame).unwrap(), (7, commit.clone()));
        assert!(parse_public_intent(&frame).is_err());
        assert_eq!(commit.staging_name(), format!(".ryoku-model-{}.part", "a".repeat(64)));
        let mut bad = commit.clone();
        bad.models[0].manifest = "/home/user/../secret".into();
        assert!(bad.frame(1).is_err());
        let mut large = commit;
        large.models = vec![large.models[0].clone(); MAX_MODELS + 1];
        assert!(large.frame(1).is_err());
    }

    #[test]
    fn actor_permit_binds_model_frame_home_and_actor_without_wallpaper_player_authority() {
        use crate::wallpaper_protocol::{
            ActorCall, ActorIdentity, NativeChange, digest, parse_change, parse_native_change,
            parse_public,
        };
        let commit = ModelReferenceCommit {
            expected_config: Some(Sha256::new("b".repeat(64)).unwrap()),
            staging_token: Sha256::new("a".repeat(64)).unwrap(),
            models: vec![],
            expected_selected: String::new(),
        };
        let frame = commit.frame(7).unwrap();
        assert!(parse_public(&frame).is_err());
        assert!(parse_change(&frame).is_err());
        assert_eq!(
            parse_native_change(&frame).unwrap(),
            (7, NativeChange::ModelReferences(Box::new(commit)))
        );
        let actor = ActorIdentity { pid: 22, start_time: 33 };
        let mut call = ActorCall {
            actor: actor.clone(),
            account_home_sha256: digest(b"/home/fixture"),
            plan_sha256: digest(b"verified-linked-plan"),
            frame_sha256: digest(&frame),
            frame,
            player_pins: vec![],
            inherit_manual_pause: false,
        };
        assert!(call.validate_for_home(&actor, "/home/fixture").is_ok());
        assert!(call.validate_for_home(&actor, "/home/unrelated").is_err());
        assert!(call.validate(&ActorIdentity { pid: 22, start_time: 34 }).is_err());
        call.inherit_manual_pause = true;
        assert!(call.validate(&actor).is_err());
        call.inherit_manual_pause = false;
        call.player_pins.push(ActorIdentity { pid: 44, start_time: 55 });
        assert!(call.validate(&actor).is_err());
        call.player_pins.clear();
        call.frame[0] = b' ';
        assert!(call.validate(&actor).is_err());
    }
}
