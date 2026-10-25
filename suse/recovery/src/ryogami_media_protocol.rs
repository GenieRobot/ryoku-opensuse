//! Private catalog commits for authenticated media trash/restore providers.
//! Catalog rows and staging identities are prepared by the helper; public
//! clients can name only a media key or an opaque protected trash receipt.
use crate::{
    Sha256,
    wallpaper_protocol::{MAX_REQUEST_BYTES, WallpaperProtocolError, bounded_line},
};
use serde::{Deserialize, Serialize};
type Result<T> = std::result::Result<T, WallpaperProtocolError>;
pub const MAX_ENTRY_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaIntent {
    Delete { key: String },
    Restore { receipt: String },
}
impl MediaIntent {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Delete { key } if valid_key(key) => Ok(()),
            Self::Restore { receipt }
                if receipt.split_once('.').is_some_and(|(token, sha)| {
                    Sha256::new(token).is_ok() && Sha256::new(sha).is_ok()
                }) =>
            {
                Ok(())
            }
            _ => Err(WallpaperProtocolError::InvalidFrame),
        }
    }
}
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 512 && !key.chars().any(char::is_control)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogMutation {
    Remove { key: String },
    Insert { key: String, entry: serde_json::Value },
}
impl CatalogMutation {
    pub fn key(&self) -> &str {
        match self {
            Self::Remove { key } | Self::Insert { key, .. } => key,
        }
    }
    fn validate(&self) -> Result<()> {
        if !valid_key(self.key()) {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        if let Self::Insert { key, entry } = self {
            let object = entry.as_object().ok_or(WallpaperProtocolError::InvalidFrame)?;
            if object.len() > 64
                || object.get("key").and_then(serde_json::Value::as_str) != Some(key.as_str())
                || !matches!(
                    object.get("type").and_then(serde_json::Value::as_str),
                    Some("static" | "video")
                )
                || serde_json::to_vec(entry)
                    .map_err(|_| WallpaperProtocolError::InvalidFrame)?
                    .len()
                    > MAX_ENTRY_BYTES
            {
                return Err(WallpaperProtocolError::InvalidFrame);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCommit {
    pub expected_catalog: Sha256,
    pub staging_token: Sha256,
    pub mutation: CatalogMutation,
}
impl CatalogCommit {
    pub fn validate(&self) -> Result<()> {
        self.mutation.validate()
    }
    pub fn staging_name(&self) -> String {
        format!(".ryoku-catalog-{}.part", self.staging_token.as_str())
    }
    pub fn staging_asset(
        &self,
    ) -> std::result::Result<crate::ryogami_assets::AssetPath, crate::ryogami_assets::AssetError>
    {
        crate::ryogami_assets::AssetPath::new(
            crate::ryogami_assets::AssetNamespace::CatalogStage,
            crate::RelativePath::new(self.staging_name())
                .map_err(|_| crate::ryogami_assets::AssetError::Path)?,
        )
    }
    pub fn frame(&self, id: i64) -> Result<Vec<u8>> {
        self.validate()?;
        if id < 0 {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        let mut bytes = serde_json::to_vec(&Frame {
            method: "broker.media-catalog".into(),
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
struct Frame {
    method: String,
    params: CatalogCommit,
    id: i64,
}
pub fn parse_commit(bytes: &[u8]) -> Result<(i64, CatalogCommit)> {
    bounded_line(bytes, MAX_REQUEST_BYTES)?;
    let frame: Frame =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if frame.method != "broker.media-catalog" || frame.id < 0 {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    frame.params.validate()?;
    Ok((frame.id, frame.params))
}
pub fn parse_public_intent(bytes: &[u8]) -> Result<(i64, MediaIntent)> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PublicFrame {
        method: String,
        params: serde_json::Value,
        id: i64,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Delete {
        key: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Restore {
        receipt: String,
    }
    bounded_line(bytes, MAX_REQUEST_BYTES)?;
    let frame: PublicFrame =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if frame.id < 0 {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    let intent = match frame.method.as_str() {
        "wall.delete" => {
            let params: Delete = serde_json::from_value(frame.params)
                .map_err(|_| WallpaperProtocolError::InvalidFrame)?;
            MediaIntent::Delete { key: params.key }
        }
        "wall.restore" => {
            let params: Restore = serde_json::from_value(frame.params)
                .map_err(|_| WallpaperProtocolError::InvalidFrame)?;
            MediaIntent::Restore { receipt: params.receipt }
        }
        _ => return Err(WallpaperProtocolError::InvalidFrame),
    };
    intent.validate()?;
    Ok((frame.id, intent))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_media_intents_cannot_supply_paths_catalog_or_recovery_authority() {
        fn line(value: &serde_json::Value) -> Vec<u8> {
            let mut bytes = serde_json::to_vec(value).unwrap();
            bytes.push(b'\n');
            bytes
        }
        let receipt = format!("{}.{}", "a".repeat(64), "b".repeat(64));
        for (method, params, expected) in [
            (
                "wall.delete",
                serde_json::json!({"key":"fixture"}),
                MediaIntent::Delete { key: "fixture".into() },
            ),
            (
                "wall.restore",
                serde_json::json!({"receipt":receipt}),
                MediaIntent::Restore { receipt: receipt.clone() },
            ),
        ] {
            let frame = serde_json::json!({"method":method,"params":params,"id":7});
            let bytes = line(&frame);
            assert_eq!(parse_public_intent(&bytes).unwrap(), (7, expected.clone()));
            assert!(
                matches!(crate::wallpaper_protocol::parse_public(&bytes).unwrap(), crate::wallpaper_protocol::PublicRequest::Media {id:7,action} if action == expected)
            );
            for field in ["path", "catalog", "checkpoint", "staging_token", "entry"] {
                let mut forged = frame.clone();
                forged["params"][field] = "client".into();
                assert!(parse_public_intent(&line(&forged)).is_err());
            }
            let mut forged = frame;
            forged["id"] = (-1).into();
            assert!(parse_public_intent(&line(&forged)).is_err());
        }
        for intent in [
            MediaIntent::Delete { key: String::new() },
            MediaIntent::Delete { key: "x".repeat(513) },
            MediaIntent::Restore { receipt: "../../receipt".into() },
            MediaIntent::Restore { receipt: format!("{receipt}.extra") },
        ] {
            assert!(intent.validate().is_err());
        }
    }

    #[test]
    fn private_catalog_commit_is_closed_bounded_and_never_a_public_wallpaper_change() {
        let commit = CatalogCommit {
            expected_catalog: Sha256::new("1".repeat(64)).unwrap(),
            staging_token: Sha256::new("2".repeat(64)).unwrap(),
            mutation: CatalogMutation::Remove { key: "fixture".into() },
        };
        let frame = commit.frame(3).unwrap();
        assert_eq!(parse_commit(&frame).unwrap(), (3, commit));
        assert!(crate::wallpaper_protocol::parse_change(&frame).is_err());
        assert!(crate::wallpaper_protocol::parse_public(&frame).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        value["params"]["command"] = "injected".into();
        assert!(parse_commit(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    #[test]
    fn restore_row_preserves_unknown_metadata_but_requires_exact_key_type_and_bound() {
        let row = serde_json::json!({"key":"fixture", "type":"static", "name":"fixture.png", "future_field": 9007199254740993u64});
        let mut commit = CatalogCommit {
            expected_catalog: Sha256::new("1".repeat(64)).unwrap(),
            staging_token: Sha256::new("2".repeat(64)).unwrap(),
            mutation: CatalogMutation::Insert { key: "fixture".into(), entry: row.clone() },
        };
        assert_eq!(parse_commit(&commit.frame(4).unwrap()).unwrap().1, commit);
        for entry in [
            serde_json::json!({"key":"other","type":"static"}),
            serde_json::json!({"key":"fixture","type":"we"}),
            serde_json::json!({"key":"fixture","type":"static","name":"x".repeat(MAX_ENTRY_BYTES)}),
        ] {
            commit.mutation = CatalogMutation::Insert { key: "fixture".into(), entry };
            assert!(commit.validate().is_err());
        }
    }
}
