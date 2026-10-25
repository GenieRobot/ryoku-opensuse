//! Bounded ACP1 JSON-RPC over NDJSON. Responses/updates are decoded as data;
//! optional capabilities never silently enable filesystem or process authority.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const FRAME_BYTES: usize = 128 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Id {
    Number(i64),
    Text(String),
}
impl Id {
    fn validate(&self) -> Result<()> {
        if let Self::Text(text) = self
            && (text.is_empty() || text.len() > 128 || text.chars().any(char::is_control))
        {
            return Err(Error::ProviderFailed);
        }
        Ok(())
    }
}
pub(super) enum Frame {
    Response { id: Id, result: std::result::Result<Value, ()> },
    Notification { method: String, params: Value },
    Request { id: Id, method: String, params: Value },
}
pub(super) fn decode(bytes: &[u8]) -> Result<Frame> {
    if bytes.len() > FRAME_BYTES {
        return Err(Error::Limit);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::ProviderFailed)?;
    if value["jsonrpc"] != "2.0" || !value.is_object() {
        return Err(Error::ProviderFailed);
    }
    let id = value
        .get("id")
        .map(|id| serde_json::from_value::<Id>(id.clone()).map_err(|_| Error::ProviderFailed))
        .transpose()?;
    if let Some(id) = &id {
        id.validate()?;
    }
    if let Some(method) = value.get("method") {
        if value.get("result").is_some() || value.get("error").is_some() {
            return Err(Error::ProviderFailed);
        }
        let method = method
            .as_str()
            .filter(|m| {
                !m.is_empty()
                    && m.len() <= 96
                    && m.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.'))
            })
            .ok_or(Error::ProviderFailed)?
            .to_owned();
        let params = value.get("params").cloned().unwrap_or(json!({}));
        if !params.is_object() {
            return Err(Error::ProviderFailed);
        }
        return Ok(match id {
            Some(id) => Frame::Request { id, method, params },
            None => Frame::Notification { method, params },
        });
    }
    let id = id.ok_or(Error::ProviderFailed)?;
    let result = match (value.get("result"), value.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) if error.is_object() && error["code"].as_i64().is_some() => Err(()),
        _ => return Err(Error::ProviderFailed),
    };
    Ok(Frame::Response { id, result })
}
pub(super) fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
pub(super) fn notify(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","method":method,"params":params})
}
pub(super) fn reject(id: Id, code: i64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":"Request unavailable under the current capability grant"}})
}
pub(super) fn respond(id: Id, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub load_session: bool,
    pub images: bool,
}
impl Capabilities {
    pub(super) fn initialize(value: &Value) -> Result<Self> {
        if value["protocolVersion"] != 1 {
            return Err(Error::ProviderUnavailable);
        }
        Ok(Self {
            load_session: value["agentCapabilities"]["loadSession"].as_bool().unwrap_or(false),
            images: value["agentCapabilities"]["promptCapabilities"]["image"]
                .as_bool()
                .unwrap_or(false),
        })
    }
}
pub(super) fn opaque(value: &Value) -> Result<String> {
    let id = value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .ok_or(Error::ProviderFailed)?;
    Ok(id.into())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub models: Vec<Model>,
    pub current_model: Option<String>,
}
impl Session {
    pub(super) fn decode(value: &Value, loaded: Option<&str>) -> Result<Self> {
        let id = match value.get("sessionId") {
            Some(id) => opaque(id)?,
            None => loaded.ok_or(Error::ProviderFailed)?.to_owned(),
        };
        let mut models = Vec::new();
        if let Some(rows) = value["models"].get("availableModels") {
            let rows = rows.as_array().ok_or(Error::ProviderFailed)?;
            if rows.len() > 128 {
                return Err(Error::Limit);
            }
            for row in rows {
                let id = opaque(&row["modelId"])?;
                if models.iter().any(|model: &Model| model.id == id) {
                    return Err(Error::ProviderFailed);
                }
                models.push(Model { id, name: opaque(&row["name"])? });
            }
        }
        let current_model = value["models"]
            .get("currentModelId")
            .filter(|id| !id.is_null())
            .map(opaque)
            .transpose()?;
        Ok(Self { id, models, current_model })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_rejects_ambiguous_rpc_and_preserves_opaque_request_ids_without_error_body_leak() {
        assert!(matches!(decode(r#"{"jsonrpc":"2.0","id":"opaque-λ","method":"session/request_permission","params":{}}"#.as_bytes()).unwrap(),Frame::Request{id:Id::Text(_),..}));
        for value in [
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1}}),
            json!({"jsonrpc":"1.0","id":1,"result":{}}),
            json!({"jsonrpc":"2.0","id":null,"result":{}}),
            json!({"jsonrpc":"2.0","method":"shell;evil"}),
        ] {
            assert!(decode(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        assert!(matches!(
            decode(
                br#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"PRIVATE_DIAGNOSTIC"}}"#
            )
            .unwrap(),
            Frame::Response { result: Err(()), .. }
        ));
        assert!(decode(&vec![b' '; FRAME_BYTES + 1]).is_err());
    }
    #[test]
    fn negotiated_capabilities_and_session_models_are_bounded_and_drop_unknown_fields() {
        assert_eq!(Capabilities::initialize(&json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true}}})).unwrap(),Capabilities{load_session:true,images:true});
        assert!(Capabilities::initialize(&json!({"protocolVersion":2})).is_err());
        let value = json!({"sessionId":"session/◇","models":{"currentModelId":"unavailable/saved","availableModels":[{"modelId":"model@opaque","name":"Model"}]},"private_key":"PRIVATE_SECRET"});
        let session = Session::decode(&value, None).unwrap();
        assert_eq!(session.current_model.as_deref(), Some("unavailable/saved"));
        assert!(!serde_json::to_string(&session).unwrap().contains("PRIVATE_SECRET"));
        assert_eq!(Session::decode(&json!({}), Some("loaded-id")).unwrap().id, "loaded-id");
    }
}
