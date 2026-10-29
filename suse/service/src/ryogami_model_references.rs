//! Bounded preparation for retained model-list actions. The caller supplies
//! settings read from the authenticated account under its writer lease. This
//! module performs no writes and grants no actor or checkpoint authority.
use ryoku_recovery::{
    Sha256,
    ryogami_models_protocol::{
        MAX_MODELS, ModelReference, ModelReferenceCommit, ModelReferenceIntent,
    },
    wallpaper_protocol::digest,
};

const MAX_SETTINGS: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceError {
    Configuration,
    Capacity,
    MissingReference,
    SelectionCallbackRequired,
    InvalidIntent,
}
impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Configuration => "model reference configuration is invalid",
            Self::Capacity => "model reference capacity exceeded",
            Self::MissingReference => "model reference no longer exists",
            Self::SelectionCallbackRequired => {
                "selected model removal requires the linked semantic callback transaction"
            }
            Self::InvalidIntent => "model reference intent is invalid",
        })
    }
}
impl std::error::Error for ReferenceError {}

/// `display_name` is resolved from the explicitly selected manifest by the
/// trusted provider, never from unrelated account files. Existing model
/// metadata is validated and retained exactly; unknown fields are refused.
pub fn prepare(
    settings: Option<&[u8]>,
    intent: &ModelReferenceIntent,
    display_name: Option<&str>,
    staging_token: Sha256,
) -> Result<ModelReferenceCommit, ReferenceError> {
    intent.validate().map_err(|_| ReferenceError::InvalidIntent)?;
    if settings.is_some_and(|bytes| bytes.len() > MAX_SETTINGS) {
        return Err(ReferenceError::Capacity);
    }
    let config: serde_json::Value = settings
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| ReferenceError::Configuration)?
        .unwrap_or_else(|| serde_json::json!({}));
    if !config.is_object() {
        return Err(ReferenceError::Configuration);
    }
    let semantic = config.get("semantic").filter(|value| !value.is_null());
    if semantic.is_some_and(|value| !value.is_object()) {
        return Err(ReferenceError::Configuration);
    }
    let mut models: Vec<ModelReference> = match semantic.and_then(|value| value.get("models")) {
        None | Some(serde_json::Value::Null) => vec![],
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|_| ReferenceError::Configuration)?
        }
    };
    if models.len() > MAX_MODELS {
        return Err(ReferenceError::Capacity);
    }
    for model in &models {
        model.validate().map_err(|_| ReferenceError::Configuration)?;
    }
    let selected = match semantic.and_then(|value| value.get("manifest")) {
        None | Some(serde_json::Value::Null) => "",
        Some(serde_json::Value::String(value)) => value,
        _ => return Err(ReferenceError::Configuration),
    };
    match intent {
        ModelReferenceIntent::Add { path } => {
            if models.len() == MAX_MODELS {
                return Err(ReferenceError::Capacity);
            }
            let name = match path {
                Some(_) => display_name.ok_or(ReferenceError::InvalidIntent)?,
                None if display_name.is_none() => "",
                None => return Err(ReferenceError::InvalidIntent),
            };
            models.push(ModelReference {
                name: name.into(),
                manifest: path.clone().unwrap_or_default(),
                managed: false,
                id: None,
                version: None,
                dimensions: None,
                bytes: None,
            });
        }
        ModelReferenceIntent::Forget { index } => {
            if display_name.is_some() {
                return Err(ReferenceError::InvalidIntent);
            }
            if *index as usize >= models.len() {
                return Err(ReferenceError::MissingReference);
            }
            models.remove(*index as usize);
            // Match upstream reconcileSelectedModel exactly: even forgetting
            // an inactive row clears an already orphaned nonempty selection.
            // That callback stops/reindexes the semantic service (#176).
            let selection = selected.trim();
            if !selection.is_empty() && !models.iter().any(|model| model.manifest == selection) {
                return Err(ReferenceError::SelectionCallbackRequired);
            }
        }
    }
    let commit = ModelReferenceCommit {
        expected_config: settings.map(|bytes| Sha256::new(digest(bytes)).expect("SHA-256")),
        staging_token,
        models,
        expected_selected: selected.into(),
    };
    commit.frame(0).map_err(|_| ReferenceError::Capacity)?;
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token() -> Sha256 {
        Sha256::new("a".repeat(64)).unwrap()
    }
    fn config() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "provider":{"secret":"fixture-secret-never-in-permit"},
            "semantic":{"enabled":true,"manifest":"/models/a/manifest.json","models":[
                {"name":"A","manifest":"/models/a/manifest.json","managed":true,"id":"a","version":"1","dimensions":256,"bytes":42},
                {"name":"B","manifest":"/models/b/manifest.json"}
            ]}
        })).unwrap()
    }
    #[test]
    fn add_retains_selection_and_model_metadata_without_forwarding_credentials() {
        let original = config();
        let commit = prepare(
            Some(&original),
            &ModelReferenceIntent::Add { path: Some("~/models/c/manifest.json".into()) },
            Some("C"),
            token(),
        )
        .unwrap();
        assert_eq!(commit.models.len(), 3);
        assert_eq!(commit.models[0].bytes, Some(42));
        assert_eq!(commit.models[0].dimensions, Some(256));
        assert_eq!(commit.expected_selected, "/models/a/manifest.json");
        assert_eq!(commit.expected_config.unwrap().as_str(), digest(&original));
        let frame =
            prepare(Some(&original), &ModelReferenceIntent::Add { path: None }, None, token())
                .unwrap()
                .frame(3)
                .unwrap();
        assert!(!String::from_utf8(frame).unwrap().contains("fixture-secret"));
        assert_eq!(original, config());
    }
    #[test]
    fn forget_preserves_live_selection_or_reports_exact_callback_dependency() {
        let original = config();
        let commit =
            prepare(Some(&original), &ModelReferenceIntent::Forget { index: 1 }, None, token())
                .unwrap();
        assert_eq!(commit.models.len(), 1);
        assert_eq!(commit.expected_selected, "/models/a/manifest.json");
        assert_eq!(
            prepare(Some(&original), &ModelReferenceIntent::Forget { index: 0 }, None, token()),
            Err(ReferenceError::SelectionCallbackRequired)
        );
        assert_eq!(
            prepare(Some(&original), &ModelReferenceIntent::Forget { index: 5 }, None, token()),
            Err(ReferenceError::MissingReference)
        );
        let mut orphan: serde_json::Value = serde_json::from_slice(&original).unwrap();
        orphan["semantic"]["manifest"] = "/models/orphan/manifest.json".into();
        assert_eq!(
            prepare(
                Some(&serde_json::to_vec(&orphan).unwrap()),
                &ModelReferenceIntent::Forget { index: 1 },
                None,
                token()
            ),
            Err(ReferenceError::SelectionCallbackRequired)
        );
        let mut duplicate: serde_json::Value = serde_json::from_slice(&original).unwrap();
        duplicate["semantic"]["models"][1] = duplicate["semantic"]["models"][0].clone();
        assert!(
            prepare(
                Some(&serde_json::to_vec(&duplicate).unwrap()),
                &ModelReferenceIntent::Forget { index: 0 },
                None,
                token()
            )
            .is_ok()
        );
    }
    #[test]
    fn blank_add_works_and_malformed_legacy_or_oversized_lists_refuse_losslessly() {
        let blank =
            prepare(None, &ModelReferenceIntent::Add { path: None }, None, token()).unwrap();
        assert_eq!(blank.models[0].name, "");
        assert_eq!(blank.models[0].manifest, "");
        assert_eq!(blank.expected_config, None);
        for bytes in [
            b"[]".as_slice(),
            br#"{"semantic":{"models":[{"name":"x","manifest":"","unknown":"keep-me"}]}}"#,
            br#"{"semantic":{"models":"wrong"}}"#,
        ] {
            assert_eq!(
                prepare(Some(bytes), &ModelReferenceIntent::Add { path: None }, None, token()),
                Err(ReferenceError::Configuration)
            );
        }
        let full = serde_json::to_vec(
            &serde_json::json!({"semantic":{"models":vec![blank.models[0].clone();MAX_MODELS]}}),
        )
        .unwrap();
        assert_eq!(
            prepare(Some(&full), &ModelReferenceIntent::Add { path: None }, None, token()),
            Err(ReferenceError::Capacity)
        );
        assert_eq!(
            prepare(
                Some(&vec![b' '; MAX_SETTINGS + 1]),
                &ModelReferenceIntent::Add { path: None },
                None,
                token()
            ),
            Err(ReferenceError::Capacity)
        );
    }
}
