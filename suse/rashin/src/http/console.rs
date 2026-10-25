//! Rust-rendered owner workflows. Every form maps to a typed runtime action;
//! credentials are never rendered back into a form or returned by inventory.
use super::{escape, form, page};
use crate::{
    Error, Result,
    api::{Lane, Request},
    provider::{Provider, ProviderEdit},
    runtime::Engine,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Read, sync::Arc};

fn nonce() -> Result<String> {
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| Error::Storage)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn hidden(name: &str, value: &str) -> String {
    format!("<input type=\"hidden\" name=\"{name}\" value=\"{}\">", escape(value))
}
fn begin(action: &str) -> String {
    format!("<form method=\"post\" action=\"/console/action\">{}", hidden("action", action))
}
fn value<'a>(fields: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    fields.get(name).map(String::as_str).ok_or(Error::InvalidRequest)
}
fn lane(value: &str) -> Result<Lane> {
    match value {
        "ask" => Ok(Lane::Ask),
        "agent" => Ok(Lane::Agent),
        "chat" => Ok(Lane::Chat),
        _ => Err(Error::InvalidRequest),
    }
}
fn models(value: &str) -> Vec<String> {
    value.lines().filter(|line| !line.is_empty()).map(str::to_owned).collect()
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
fn id_path(prefix: &str, id: &str) -> Result<String> {
    crate::identifier(id)?;
    Ok(format!("/{prefix}/{id}"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pick {
    provider: String,
    model: String,
}

pub(super) fn submit(engine: &Arc<Engine>, body: &[u8]) -> Result<String> {
    let fields = form(body)?;
    match value(&fields, "action")? {
        "configure_prowl" => {
            engine.dispatch(Request::ConfigureProwl {
                port: value(&fields, "port")?.parse().map_err(|_| Error::InvalidRequest)?,
                token: fields.get("token").filter(|token| !token.is_empty()).cloned(),
            })?;
            Ok("/prowl".into())
        }
        "disable_prowl" => {
            engine.dispatch(Request::DisableProwl {})?;
            Ok("/prowl".into())
        }
        "ask" => {
            let id = value(&fields, "id")?.to_owned();
            engine.dispatch(Request::Ask {
                id: id.clone(),
                prompt: value(&fields, "prompt")?.into(),
            })?;
            id_path("job", &id)
        }
        "new_session" => {
            let id = value(&fields, "id")?.to_owned();
            engine.dispatch(Request::NewSession {
                id: id.clone(),
                lane: lane(value(&fields, "lane")?)?,
            })?;
            id_path("session", &id)
        }
        "prompt" => {
            let id = value(&fields, "id")?.to_owned();
            engine.dispatch(Request::Prompt {
                id: id.clone(),
                session: value(&fields, "session")?.into(),
                prompt: value(&fields, "prompt")?.into(),
            })?;
            id_path("job", &id)
        }
        "put_provider" => {
            engine.dispatch(Request::PutProvider {
                id: value(&fields, "provider")?.into(),
                provider: Provider::OpenAiCompatible {
                    endpoint: value(&fields, "endpoint")?.into(),
                    api_key: value(&fields, "api_key")?.into(),
                    models: models(value(&fields, "models")?),
                    streaming: fields.contains_key("streaming"),
                },
            })?;
            Ok("/console".into())
        }
        "update_provider" => {
            engine.dispatch(Request::UpdateProvider {
                id: value(&fields, "provider")?.into(),
                changes: ProviderEdit {
                    endpoint: Some(value(&fields, "endpoint")?.into()),
                    api_key: fields.get("api_key").filter(|key| !key.is_empty()).cloned(),
                    models: Some(models(value(&fields, "models")?)),
                    streaming: Some(fields.contains_key("streaming")),
                },
            })?;
            Ok("/console".into())
        }
        "set_route" => {
            let pick: Pick = serde_json::from_str(value(&fields, "choice")?)
                .map_err(|_| Error::InvalidRequest)?;
            engine.dispatch(Request::SetRoute {
                lane: lane(value(&fields, "lane")?)?,
                provider: pick.provider,
                model: pick.model,
            })?;
            Ok("/console".into())
        }
        "refresh_models" => {
            let id = value(&fields, "id")?.to_owned();
            engine.dispatch(Request::RefreshModels {
                id: id.clone(),
                provider: value(&fields, "provider")?.into(),
            })?;
            id_path("job", &id)
        }
        "remove_provider" => {
            engine.dispatch(Request::RemoveProvider { id: value(&fields, "provider")?.into() })?;
            Ok("/console".into())
        }
        "cancel" => {
            let id = value(&fields, "id")?;
            engine.dispatch(Request::Cancel { id: id.into() })?;
            id_path("job", id)
        }
        "forget" => {
            engine.dispatch(Request::ForgetJob { id: value(&fields, "id")?.into() })?;
            Ok("/console".into())
        }
        "delete_session" => {
            engine.dispatch(Request::DeleteSession { id: value(&fields, "session")?.into() })?;
            Ok("/console".into())
        }
        _ => Err(Error::InvalidRequest),
    }
}

pub(super) fn home(engine: &Arc<Engine>) -> Result<String> {
    let status = engine.dispatch(Request::Status {})?;
    let providers = engine.dispatch(Request::Providers {})?;
    let sessions = engine.dispatch(Request::Sessions {})?;
    let jobs = engine.dispatch(Request::Jobs {})?;
    let mut body = String::from(
        "<p>Your private assistant workspace.</p><p><a href=\"/prowl\">Prowl gateway</a></p><h2>Ask</h2>",
    );
    body += &begin("ask");
    body += &hidden("id", &nonce()?);
    body += "<label>Question<textarea name=\"prompt\" required maxlength=\"16384\"></textarea></label><button>Ask</button></form><h2>Conversations</h2>";
    body += &begin("new_session");
    body += &hidden("id", &nonce()?);
    body += "<label>Conversation type<select name=\"lane\"><option value=\"agent\">Agent</option><option value=\"chat\">Chat</option></select></label><button>New conversation</button></form><ul>";
    for session in sessions["sessions"].as_array().ok_or(Error::Integrity)? {
        let id = text(session, "id");
        let path = id_path("session", id)?;
        body += &format!(
            "<li><a href=\"{}\">{} conversation</a> · {} messages</li>",
            escape(&path),
            escape(text(session, "lane")),
            session["messages"]
        );
    }
    body += "</ul><h2>Models</h2>";
    for name in ["ask", "agent", "chat"] {
        let saved = &status["routes"][name];
        let mut options = String::new();
        let mut available = false;
        for provider in providers["providers"].as_array().ok_or(Error::Integrity)? {
            for model in provider["models"].as_array().ok_or(Error::Integrity)? {
                let model = model.as_str().ok_or(Error::Integrity)?;
                let provider_id = text(provider, "id");
                let selected = saved["provider"] == provider_id && saved["model"] == model;
                available |= selected;
                options += &format!(
                    "<option value=\"{}\"{}>{} · {}</option>",
                    escape(&json!({"provider":provider_id,"model":model}).to_string()),
                    if selected { " selected" } else { "" },
                    escape(provider_id),
                    escape(model)
                );
            }
        }
        body += &begin("set_route");
        body += &hidden("lane", name);
        body += &format!("<label>{} model<select name=\"choice\" required>", escape(name));
        if !available {
            body += &format!(
                "<option value=\"\" disabled selected>Saved pick unavailable: {} · {}</option>",
                escape(text(saved, "provider")),
                escape(text(saved, "model"))
            );
        }
        body += &options;
        body += "</select></label><button>Use selected model</button></form>";
    }
    body += "<h2>Providers</h2>";
    for provider in providers["providers"].as_array().ok_or(Error::Integrity)? {
        let id = text(provider, "id");
        body +=
            &format!("<section><h3>{}</h3><p>{}</p>", escape(id), escape(text(provider, "kind")));
        body += &begin("refresh_models");
        body += &hidden("id", &nonce()?);
        body += &hidden("provider", id);
        body += "<button>Refresh offered models</button></form>";
        if provider["kind"] == "open_ai_compatible" {
            body += &begin("update_provider");
            body += &hidden("provider", id);
            body += &provider_fields(provider, false);
            body += "<button>Save provider changes</button></form>";
        }
        if id != "local" {
            body += &begin("remove_provider");
            body += &hidden("provider", id);
            body += "<button>Remove provider</button></form>";
        }
        body += "</section>";
    }
    body += "<h3>Add provider</h3>";
    body += &begin("put_provider");
    body += "<label>Provider name<input name=\"provider\" required maxlength=\"64\" pattern=\"[A-Za-z0-9_-]+\"></label>";
    body += &provider_fields(&Value::Null, true);
    body += "<button>Add provider</button></form><h2>Recent work</h2><ul>";
    for job in jobs["jobs"].as_array().ok_or(Error::Integrity)? {
        body += &format!(
            "<li><a href=\"{}\">{} · {}</a> {}</li>",
            escape(&id_path("job", text(job, "id"))?),
            escape(text(job, "lane")),
            escape(text(job, "status")),
            escape(text(job, "summary"))
        );
    }
    body += "</ul><h2>Recent answers</h2>";
    let recent = engine.dispatch(Request::Recent {})?;
    for record in recent["asks"].as_array().ok_or(Error::Integrity)? {
        let job = &record["job"];
        body += &format!("<article><h3>Question</h3><pre>{}</pre>", escape(text(job, "prompt")));
        if let Some(answer) = job["answer"].as_str() {
            body += &format!("<h3>Answer</h3><pre>{}</pre>", escape(answer));
        }
        body += &format!(
            "<a href=\"{}\">{}</a></article>",
            escape(&id_path("job", text(record, "id"))?),
            escape(text(job, "status"))
        );
    }
    body += "<h2>Index local evidence</h2><p>Replace the current document index with uploaded text.</p><form method=\"post\" action=\"/index\"><label>Document name<input name=\"name\" required maxlength=\"128\"></label><label>Content<textarea name=\"content\" required maxlength=\"32768\"></textarea></label><button>Index evidence</button></form>";
    let usage = engine.dispatch(Request::Usage {})?;
    body += &format!(
        "<h2>Usage</h2><pre>{}</pre><h2>Status</h2><pre>{}</pre>",
        escape(&serde_json::to_string_pretty(&usage).map_err(|_| Error::Storage)?),
        escape(&serde_json::to_string_pretty(&status).map_err(|_| Error::Storage)?)
    );
    Ok(page("Rashin", &body))
}
fn provider_fields(provider: &Value, new: bool) -> String {
    let models = provider["models"]
        .as_array()
        .map(|rows| rows.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    format!(
        "<label>HTTPS endpoint<input name=\"endpoint\" type=\"url\" value=\"{}\" required maxlength=\"2048\"></label><label>{}<input name=\"api_key\" type=\"password\" autocomplete=\"off\" maxlength=\"4096\"{}></label><label>Offered model IDs, one per line<textarea name=\"models\">{}</textarea></label><label><input name=\"streaming\" type=\"checkbox\" value=\"true\"{}>Stream responses</label>",
        escape(text(provider, "endpoint")),
        if new { "Private API key" } else { "New API key (leave blank to retain current key)" },
        if new { " required" } else { "" },
        escape(&models),
        if provider["streaming"].as_bool().unwrap_or(false) { " checked" } else { "" }
    )
}
pub(super) fn session(engine: &Arc<Engine>, id: &str) -> Result<String> {
    crate::identifier(id)?;
    let value = engine.dispatch(Request::Session { id: id.into() })?;
    let session = &value["session"];
    let mut body = format!(
        "<p><a href=\"/console\">Workspace</a></p><h2>{} conversation</h2>",
        escape(text(session, "lane"))
    );
    for message in session["messages"].as_array().ok_or(Error::Integrity)? {
        body += &format!(
            "<article><h3>{}</h3><pre>{}</pre></article>",
            escape(text(message, "role")),
            escape(text(message, "content"))
        );
    }
    body += &begin("prompt");
    body += &hidden("id", &nonce()?);
    body += &hidden("session", id);
    body += "<label>Message<textarea name=\"prompt\" required maxlength=\"16384\"></textarea></label><button>Send message</button></form>";
    let jobs = engine.dispatch(Request::Jobs {})?;
    for job in
        jobs["jobs"].as_array().ok_or(Error::Integrity)?.iter().filter(|job| job["session"] == id)
    {
        body += &format!(
            "<p><a href=\"{}\">{} · {}</a></p>",
            escape(&id_path("job", text(job, "id"))?),
            escape(text(job, "status")),
            escape(text(job, "summary"))
        );
    }
    body += &begin("delete_session");
    body += &hidden("session", id);
    body += "<button>Delete conversation and its work records</button></form>";
    Ok(page("Rashin conversation", &body))
}
pub(super) fn job(engine: &Arc<Engine>, id: &str) -> Result<String> {
    crate::identifier(id)?;
    let value = engine.dispatch(Request::Job { id: id.into() })?;
    let job = &value["job"];
    let running = job["status"] == "running";
    let mut body = format!(
        "<p><a href=\"/console\">Workspace</a></p><h2>{}</h2><p>{} · {}</p>",
        escape(text(job, "status")),
        escape(text(job, "provider")),
        escape(text(job, "model"))
    );
    if job["operation"] == "catalog_refresh" {
        body += &format!("<p>Model catalog refresh · offered models: {}</p>", job["catalog_count"]);
    } else {
        body += &format!("<h3>Question</h3><pre>{}</pre>", escape(text(job, "prompt")));
    }
    if let Some(progress) = value["progress"].as_str().filter(|text| !text.is_empty()) {
        body += &format!("<h3>Receiving response</h3><pre>{}</pre>", escape(progress));
    }
    if let Some(answer) = job["answer"].as_str() {
        body += &format!("<h3>Answer</h3><pre>{}</pre>", escape(answer));
    }
    if !job["error"].is_null() {
        body += &format!("<p>{}</p>", escape(text(job, "error")));
    }
    if let Some(session) = job["session"].as_str() {
        body += &format!(
            "<p><a href=\"{}\">Conversation</a></p>",
            escape(&id_path("session", session)?)
        );
    }
    body += &begin(if running { "cancel" } else { "forget" });
    body += &hidden("id", id);
    body += if running {
        "<button>Cancel</button></form>"
    } else {
        "<button>Remove work record</button></form>"
    };
    let html = page("Rashin work", &body);
    Ok(if running {
        html.replacen("<main>", "<meta http-equiv=\"refresh\" content=\"1\"><main>", 1)
    } else {
        html
    })
}

pub(super) fn prowl(engine: &Arc<Engine>, path: &str) -> Result<String> {
    use crate::prowl::Query;
    let status = engine.dispatch(Request::ProwlStatus {})?;
    let mut body = String::from(
        "<p><a href=\"/console\">Workspace</a></p><p>Connect an already-running local Prowl gateway. This does not start or enable a service.</p>",
    );
    if path == "/prowl" {
        body += &begin("configure_prowl");
        body += &format!(
            "<label>Loopback gateway port<input name=\"port\" type=\"number\" min=\"1\" max=\"65535\" value=\"{}\" required></label>",
            status["port"].as_u64().unwrap_or(8788)
        );
        body += "<label>Management token (leave blank to retain saved credential)<input name=\"token\" type=\"password\" autocomplete=\"off\" maxlength=\"512\"></label><button>Save gateway connection</button></form>";
        if status["configured"] == true {
            body += "<p>Configured. Credentials remain private.</p><ul><li><a href=\"/prowl/profiles\">Profiles</a></li><li><a href=\"/prowl/active\">Active profile</a></li><li><a href=\"/prowl/presets\">Profile presets</a></li></ul>";
            body += &begin("disable_prowl");
            body += "<button>Disconnect gateway</button></form>";
        } else {
            body += "<p>No gateway configured.</p>";
        }
    } else {
        let query = match path {
            "/prowl/profiles" => Query::Profiles {},
            "/prowl/active" => Query::ActiveProfile {},
            "/prowl/presets" => Query::ProfilePresets {},
            value => Query::ProfileModels {
                id: value
                    .strip_prefix("/prowl/models/")
                    .ok_or(Error::InvalidRequest)?
                    .parse()
                    .map_err(|_| Error::InvalidRequest)?,
            },
        };
        let result = engine.dispatch(Request::ReadProwl { query })?;
        body += "<p><a href=\"/prowl\">Gateway settings</a></p>";
        if matches!(query, Query::Profiles {}) {
            for profile in result["result"].as_array().ok_or(Error::Integrity)? {
                let id = profile["id"].as_u64().ok_or(Error::Integrity)?;
                body += &format!(
                    "<p><a href=\"/prowl/models/{id}\">{} · {} models</a></p>",
                    escape(text(profile, "name")),
                    profile["modelCount"]
                );
            }
        }
        body += &format!(
            "<pre>{}</pre>",
            escape(&serde_json::to_string_pretty(&result["result"]).map_err(|_| Error::Storage)?)
        );
    }
    Ok(page("Prowl gateway", &body))
}
