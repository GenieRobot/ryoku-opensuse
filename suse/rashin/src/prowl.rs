//! Individually typed Prowl reads from the retained 1.0 gateway contract.
//! The upstream catch-all proxy and automatic service spawn are deliberately
//! replaced by explicit owner configuration and fixed local routes. Management
//! credentials stay in private state/stdin; no token path or URL is accepted.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod completion;
pub(crate) mod read_roots;
mod system_query;
mod tool_calls;
mod tools;
pub(crate) use tools::Context as ReadTools;
mod transport;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connection {
    pub port: u16,
    pub token: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Query {
    Profiles {},
    ActiveProfile {},
    ProfileModels { id: u64 },
    ProfilePresets {},
}
impl Query {
    fn path(self) -> Result<String> {
        Ok(match self {
            Self::Profiles {} => "/api/profiles".into(),
            Self::ActiveProfile {} => "/api/profiles/active".into(),
            Self::ProfileModels { id } => {
                valid_id(id)?;
                format!("/api/profiles/{id}/models")
            }
            Self::ProfilePresets {} => "/api/profiles/presets".into(),
        })
    }
}
impl Connection {
    pub fn validate(&self) -> Result<()> {
        if self.port == 0
            || self.token.len() < 16
            || self.token.len() > 512
            || !self.token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
    pub fn read(&self, query: Query) -> Result<Value> {
        self.validate()?;
        let path = query.path()?;
        let (status, body) = transport::get(self.port, &self.token, &path)?;
        match status {
            200 => (),
            401 | 403 => return Err(Error::Unauthorized),
            404 => return Err(Error::NotFound),
            500..=599 => return Err(Error::ProviderUnavailable),
            _ => return Err(Error::ProviderFailed),
        }
        // Never relay a raw gateway error or an accidental management-token
        // echo. Deserialize/serialize only the retained public response fields.
        if body.windows(self.token.len()).any(|window| window == self.token.as_bytes()) {
            return Err(Error::ProviderFailed);
        }
        parse(query, &body)
    }
}
fn valid_id(id: u64) -> Result<()> {
    if id == 0 || id > 9_007_199_254_740_991 {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}
fn bounded(value: &str, limit: usize) -> Result<()> {
    if value.len() > limit || value.contains('\0') {
        return Err(Error::ProviderFailed);
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Profile {
    id: u64,
    name: String,
    strategy: String,
    emoji: String,
    color: String,
    #[serde(rename = "type")]
    kind: String,
    is_favorite: u8,
    sort_order: i64,
    auto_sort: Option<String>,
    layout_config: Option<String>,
    auto_include_new_models: u8,
    #[serde(rename = "modelCount")]
    model_count: u64,
    created_at: String,
}
impl Profile {
    fn validate(&self) -> Result<()> {
        valid_id(self.id).map_err(|_| Error::ProviderFailed)?;
        if self.is_favorite > 1 || self.auto_include_new_models > 1 {
            return Err(Error::ProviderFailed);
        }
        for value in
            [&self.name, &self.strategy, &self.emoji, &self.color, &self.kind, &self.created_at]
        {
            bounded(value, 256)?;
        }
        if let Some(value) = &self.auto_sort {
            bounded(value, 128)?;
        }
        if let Some(value) = &self.layout_config {
            bounded(value, 64 * 1024)?;
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct ActiveProfile {
    #[serde(rename = "activeProfileId")]
    active_profile_id: Option<u64>,
}
#[derive(Serialize, Deserialize)]
struct ProfileModel {
    model_db_id: u64,
    priority: i64,
    enabled: bool,
    platform: String,
    model_id: String,
    display_name: String,
    intelligence_rank: i64,
    speed_rank: i64,
    size_label: String,
    rpm_limit: Option<u64>,
    rpd_limit: Option<u64>,
    tpm_limit: Option<u64>,
    tpd_limit: Option<u64>,
    monthly_token_budget: String,
}
#[derive(Serialize, Deserialize)]
struct Preset {
    id: String,
    name: String,
    description: String,
    group: String,
    requirements: Vec<String>,
    models: u64,
    strategy: String,
}
#[derive(Serialize, Deserialize)]
struct Presets {
    presets: Vec<Preset>,
}
fn parse(query: Query, body: &[u8]) -> Result<Value> {
    fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
        serde_json::from_slice(body).map_err(|_| Error::ProviderFailed)
    }
    fn encode(value: impl Serialize) -> Result<Value> {
        serde_json::to_value(value).map_err(|_| Error::ProviderFailed)
    }
    match query {
        Query::Profiles {} => {
            let rows: Vec<Profile> = decode(body)?;
            if rows.len() > 512 {
                return Err(Error::Limit);
            }
            let mut seen = std::collections::BTreeSet::new();
            for row in &rows {
                row.validate()?;
                if !seen.insert(row.id) {
                    return Err(Error::ProviderFailed);
                }
            }
            encode(rows)
        }
        Query::ActiveProfile {} => {
            let envelope: Value = decode(body)?;
            if envelope.get("activeProfileId").is_none() {
                return Err(Error::ProviderFailed);
            }
            let active: ActiveProfile = decode(body)?;
            if let Some(id) = active.active_profile_id {
                valid_id(id).map_err(|_| Error::ProviderFailed)?;
            }
            encode(active)
        }
        Query::ProfileModels { .. } => {
            let rows: Vec<ProfileModel> = decode(body)?;
            if rows.len() > 4096 {
                return Err(Error::Limit);
            }
            let mut seen = std::collections::BTreeSet::new();
            for row in &rows {
                valid_id(row.model_db_id).map_err(|_| Error::ProviderFailed)?;
                if !seen.insert(row.model_db_id) {
                    return Err(Error::ProviderFailed);
                }
                for value in
                    [&row.platform, &row.display_name, &row.size_label, &row.monthly_token_budget]
                {
                    bounded(value, 256)?;
                }
                crate::provider::model_name(&row.model_id).map_err(|_| Error::ProviderFailed)?;
            }
            encode(rows)
        }
        Query::ProfilePresets {} => {
            let rows: Presets = decode(body)?;
            if rows.presets.len() > 512 {
                return Err(Error::Limit);
            }
            for row in &rows.presets {
                for value in [&row.id, &row.name, &row.group, &row.strategy] {
                    bounded(value, 256)?;
                }
                bounded(&row.description, 4096)?;
                if row.requirements.len() > 64 {
                    return Err(Error::Limit);
                }
                for requirement in &row.requirements {
                    bounded(requirement, 256)?;
                }
            }
            encode(rows)
        }
    }
}

impl Connection {
    pub(crate) fn complete(
        &self,
        model: &str,
        messages: &[crate::provider::Message],
        streaming: bool,
        cancelled: &std::sync::atomic::AtomicBool,
        emit: &mut impl FnMut(&str) -> Result<()>,
    ) -> Result<crate::provider::Completion> {
        self.validate()?;
        completion::complete(self, model, messages, streaming, cancelled, emit)
    }
    pub(crate) fn complete_with_tools(
        &self,
        model: &str,
        messages: &[crate::provider::Message],
        streaming: bool,
        cancelled: &std::sync::atomic::AtomicBool,
        context: &ReadTools,
        emit: &mut impl FnMut(&str) -> Result<()>,
    ) -> Result<crate::provider::Completion> {
        self.validate()?;
        tools::complete(self, model, messages, streaming, cancelled, context, emit)
    }
    pub(crate) fn models(&self, cancelled: &std::sync::atomic::AtomicBool) -> Result<Vec<String>> {
        self.validate()?;
        completion::models(self, cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn routes_credentials_and_fixed_transport_have_no_generic_proxy_or_command_path() {
        assert_eq!(Query::Profiles {}.path().unwrap(), "/api/profiles");
        assert_eq!(Query::ActiveProfile {}.path().unwrap(), "/api/profiles/active");
        assert_eq!(Query::ProfileModels { id: 42 }.path().unwrap(), "/api/profiles/42/models");
        assert_eq!(Query::ProfilePresets {}.path().unwrap(), "/api/profiles/presets");
        for id in [0, u64::MAX] {
            assert_eq!(Query::ProfileModels { id }.path(), Err(Error::InvalidRequest));
        }
        for value in [
            json!({"operation":"profiles","path":"/api/keys"}),
            json!({"operation":"profile_models","id":"../../keys"}),
            json!({"operation":"profiles","role":"owner"}),
        ] {
            assert!(serde_json::from_value::<Query>(value).is_err());
        }
        for token in ["", "short", "credential\nnew-header", "credential token with spaces"] {
            assert_eq!(
                Connection { port: 8788, token: token.into() }.validate(),
                Err(Error::InvalidRequest)
            );
        }
        assert_eq!(
            Connection { port: 0, token: "private-management-token".into() }.validate(),
            Err(Error::InvalidRequest)
        );
    }
    #[test]
    fn retained_typed_response_fields_preserve_profile_models_and_drop_unknown_secrets() {
        let profile = json!({"id":7,"name":"Private <profile>","strategy":"round-robin","emoji":"🦊","color":"red","type":"custom","is_favorite":1,"sort_order":2,"auto_sort":null,"layout_config":"{}","auto_include_new_models":0,"modelCount":2,"created_at":"2026-10-25","unknown_secret":"do-not-relay"});
        let bytes = serde_json::to_vec(&vec![profile.clone()]).unwrap();
        let parsed = parse(Query::Profiles {}, &bytes).unwrap();
        assert_eq!(parsed[0]["name"], "Private <profile>");
        assert_eq!(parsed[0]["modelCount"], 2);
        assert!(parsed[0].get("unknown_secret").is_none());
        let oversized = serde_json::to_vec(&vec![profile.clone(); 513]).unwrap();
        let duplicate = serde_json::to_vec(&vec![profile.clone(), profile]).unwrap();
        assert_eq!(parse(Query::Profiles {}, &duplicate), Err(Error::ProviderFailed));
        assert_eq!(
            parse(Query::ActiveProfile {}, br#"{"activeProfileId":null}"#).unwrap(),
            json!({"activeProfileId":null})
        );
        for bad in [
            br#"{}"#.as_slice(),
            br#"{"error":{"code":"bad"}}"#,
            br#"{"activeProfileId":0}"#,
            br#"{"activeProfileId":"7"}"#,
        ] {
            assert_eq!(parse(Query::ActiveProfile {}, bad), Err(Error::ProviderFailed));
        }
        let model = json!({"model_db_id":9,"priority":1,"enabled":true,"platform":"custom","model_id":"org/model ◇ <opaque>","display_name":"Local","intelligence_rank":-1,"speed_rank":2,"size_label":"unknown","rpm_limit":null,"rpd_limit":10,"tpm_limit":null,"tpd_limit":null,"monthly_token_budget":"unlimited","api_key":"do-not-relay"});
        let parsed =
            parse(Query::ProfileModels { id: 7 }, &serde_json::to_vec(&vec![model]).unwrap())
                .unwrap();
        assert_eq!(parsed[0]["model_id"], "org/model ◇ <opaque>");
        assert_eq!(parsed[0]["rpd_limit"], 10);
        assert!(parsed[0].get("api_key").is_none());
        assert_eq!(
            parse(Query::ProfilePresets {}, br#"{"presets":[]}"#).unwrap(),
            json!({"presets":[]})
        );
        assert_eq!(parse(Query::Profiles {}, &oversized), Err(Error::Limit));
    }
}
