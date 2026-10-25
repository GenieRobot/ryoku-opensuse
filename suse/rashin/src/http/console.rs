//! Rust-rendered owner workflows. Every form maps to a typed runtime action;
//! credentials are never rendered back into a form or returned by inventory.
use super::{escape, form, page};
use crate::{
    Error, Result,
    api::{FixKind, HandoffRun, Lane, Request},
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
fn fix_kind(value: &str) -> Result<FixKind> {
    match value {
        "doctor" => Ok(FixKind::Doctor),
        "tip" => Ok(FixKind::Tip),
        "app" => Ok(FixKind::App),
        _ => Err(Error::InvalidRequest),
    }
}
fn tool(value: &str) -> Result<crate::usage::Tool> {
    match value {
        "claude" => Ok(crate::usage::Tool::Claude),
        "codex" => Ok(crate::usage::Tool::Codex),
        "opencode" => Ok(crate::usage::Tool::Opencode),
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
        "index_wiki" => {
            let id = value(&fields, "id")?;
            let replace = fields.get("replace_sha256").filter(|sha| !sha.is_empty()).cloned();
            if replace.is_some() && fields.get("confirm_replace").map(String::as_str) != Some("yes")
            {
                return Err(Error::InvalidRequest);
            }
            engine.dispatch(Request::IndexWiki {
                id: id.into(),
                sha256: value(&fields, "sha256")?.into(),
                replace_sha256: replace,
            })?;
            id_path("wiki", id)
        }
        "wire" => {
            engine
                .dispatch(Request::Wire { instructions: value(&fields, "instructions")?.into() })?;
            Ok("/console".into())
        }
        "unwire" => {
            engine.dispatch(Request::Unwire {})?;
            Ok("/console".into())
        }
        "fix" => {
            if value(&fields, "confirm")? != "yes" {
                return Err(Error::InvalidRequest);
            }
            let id = value(&fields, "id")?.to_owned();
            engine.dispatch(Request::Fix {
                id: id.clone(),
                kind: fix_kind(value(&fields, "kind")?)?,
                component: value(&fields, "component")?.into(),
                evidence: value(&fields, "evidence")?.into(),
                native_evidence_consent: Some(value(&fields, "consent")?.into()),
            })?;
            id_path("job", &id)
        }
        "manual_handoff" => {
            let run = match value(&fields, "mode")? {
                "stage" => None,
                "run" => Some(HandoffRun {
                    id: value(&fields, "id")?.into(),
                    session: value(&fields, "session")?.into(),
                }),
                _ => return Err(Error::InvalidRequest),
            };
            let path = run
                .as_ref()
                .map(|run| id_path("job", &run.id))
                .transpose()?
                .unwrap_or_else(|| "/console".into());
            engine.dispatch(Request::ManualHandoff {
                question: value(&fields, "question")?.into(),
                answer: value(&fields, "answer")?.into(),
                run,
            })?;
            Ok(path)
        }
        "handoff" => {
            engine.dispatch(Request::Handoff {
                ask_id: value(&fields, "ask_id")?.into(),
                run: None,
            })?;
            Ok("/console".into())
        }
        "handoff_run" => {
            let id = value(&fields, "id")?.to_owned();
            engine.dispatch(Request::Handoff {
                ask_id: value(&fields, "ask_id")?.into(),
                run: Some(HandoffRun {
                    id: id.clone(),
                    session: value(&fields, "session")?.into(),
                }),
            })?;
            id_path("job", &id)
        }
        "clear_handoff" => {
            engine.dispatch(Request::ClearHandoff {})?;
            Ok("/console".into())
        }
        "enroll_usage" => {
            let five = fields.get("five_hour_limit").filter(|value| !value.is_empty());
            let seven = fields.get("seven_day_limit").filter(|value| !value.is_empty());
            let soft_caps = match (five, seven) {
                (None, None) => None,
                (Some(five), Some(seven)) => Some(crate::usage::SoftCaps {
                    five_hour_limit: five.parse().map_err(|_| Error::InvalidRequest)?,
                    seven_day_limit: seven.parse().map_err(|_| Error::InvalidRequest)?,
                }),
                _ => return Err(Error::InvalidRequest),
            };
            engine.dispatch(Request::EnrollUsageSource {
                tool: tool(value(&fields, "tool")?)?,
                path: value(&fields, "path")?.into(),
                soft_caps,
            })?;
            Ok("/usage".into())
        }
        "collect_usage" => {
            engine.dispatch(Request::CollectUsage { tool: tool(value(&fields, "tool")?)? })?;
            Ok("/usage".into())
        }
        "disable_usage" => {
            engine
                .dispatch(Request::DisableUsageSource { tool: tool(value(&fields, "tool")?)? })?;
            Ok("/usage".into())
        }
        "set_public_web" => {
            engine.dispatch(Request::SetPublicWeb {
                provider: value(&fields, "provider")?.into(),
                enabled: fields.contains_key("enabled"),
            })?;
            Ok("/console".into())
        }
        "set_code_search" => {
            if fields.get("root_id").is_some_and(|id| !id.is_empty())
                && fields.get("confirm").map(String::as_str) != Some("yes")
            {
                return Err(Error::InvalidRequest);
            }
            engine.dispatch(Request::SetCodeSearch {
                provider: value(&fields, "provider")?.into(),
                root_id: fields.get("root_id").filter(|id| !id.is_empty()).cloned(),
            })?;
            Ok("/console".into())
        }
        "enroll_read_root" => {
            if fields.get("confirm").map(String::as_str) != Some("yes") {
                return Err(Error::InvalidRequest);
            }
            engine.dispatch(Request::EnrollReadRoot {
                provider: value(&fields, "provider")?.into(),
                id: value(&fields, "root_id")?.into(),
                path: value(&fields, "path")?.into(),
            })?;
            Ok("/console".into())
        }
        "disable_read_root" => {
            engine.dispatch(Request::DisableReadRoot {
                provider: value(&fields, "provider")?.into(),
                id: value(&fields, "root_id")?.into(),
            })?;
            Ok("/console".into())
        }
        "set_read_only_tools" => {
            engine.dispatch(Request::SetReadOnlyTools {
                provider: value(&fields, "provider")?.into(),
                enabled: fields.contains_key("enabled"),
            })?;
            Ok("/console".into())
        }
        "connect_prowl_provider" => {
            engine.dispatch(Request::ConnectProwlProvider {
                id: value(&fields, "provider")?.into(),
                models: models(value(&fields, "models")?),
                streaming: fields.contains_key("streaming"),
            })?;
            Ok("/console".into())
        }
        "update_prowl_provider" => {
            engine.dispatch(Request::UpdateProvider {
                id: value(&fields, "provider")?.into(),
                changes: ProviderEdit {
                    models: Some(models(value(&fields, "models")?)),
                    streaming: Some(fields.contains_key("streaming")),
                    ..ProviderEdit::default()
                },
            })?;
            Ok("/console".into())
        }
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
        "<p>Your private assistant workspace.</p><p><a href=\"/prowl\">Prowl gateway</a> · <a href=\"/usage\">Local tool usage</a> · <a href=\"/fix\">Fix investigation</a> · <a href=\"/wiki\">Offline Wiki</a></p><h2>Ask</h2>",
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
    body += "</ul>";
    if !sessions["pending_handoff"].is_null() {
        let pending = &sessions["pending_handoff"];
        body += &format!(
            "<h2>Pending context for next Agent message</h2><h3>Question</h3><pre>{}</pre><h3>Answer</h3><pre>{}</pre>",
            escape(text(pending, "question")),
            escape(text(pending, "answer"))
        );
        body += &format!("<p>Context source: {}</p>", escape(text(pending, "origin")));
        body += &begin("clear_handoff");
        body += "<button>Clear pending context</button></form>";
    }
    body += "<details><summary>Manual context for Agent</summary><p>Supplied context is labeled manual and is separate from recorded Ask history. Immediate Run cancels active Ask work.</p>";
    body += &begin("manual_handoff");
    body += &hidden("id", &nonce()?);
    body += "<label>Question<textarea name=\"question\" required maxlength=\"16384\"></textarea></label><label>Answer (may be empty)<textarea name=\"answer\" maxlength=\"16384\"></textarea></label><label>Continue<select name=\"mode\"><option value=\"stage\">Stage for next Agent message</option><option value=\"run\">Run now and cancel Ask</option></select></label><label>Agent conversation for immediate Run<select name=\"session\"><option value=\"\">Choose Agent conversation</option>";
    for session in sessions["sessions"]
        .as_array()
        .ok_or(Error::Integrity)?
        .iter()
        .filter(|session| session["lane"] == "agent")
    {
        let id = text(session, "id");
        body += &format!("<option value=\"{}\">{}</option>", escape(id), escape(id));
    }
    body += "</select></label><button>Continue with manual context</button></form></details>";
    body += &format!(
        "<h2>Built-in Agent instructions</h2><p>Instructions are {} for Agent messages. Ask and Chat are separate.</p>",
        if status["wired"] == true { "enabled" } else { "disabled" }
    );
    body += &begin("wire");
    body += "<label>Replace Agent instructions<textarea name=\"instructions\" required maxlength=\"16384\"></textarea></label><button>Save built-in Agent instructions</button></form>";
    if status["wired"] == true {
        body += &begin("unwire");
        body += "<button>Remove built-in Agent instructions</button></form>";
    }
    body += "<h2>Models</h2>";
    for name in ["ask", "agent", "chat"] {
        let saved = &status["routes"][name];
        let mut options = String::new();
        let mut available = false;
        for provider in providers["providers"].as_array().ok_or(Error::Integrity)? {
            if provider["available"] == false {
                continue;
            }
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
        if provider["kind"] == "prowl_gateway" {
            body += &format!(
                "<p>Gateway connection configured: {}. <a href=\"/prowl\">Manage private gateway connection</a></p>",
                provider["available"]
            );
            let roots = engine.dispatch(Request::ReadRoots { provider: id.into() })?;
            body += "<h4>Enrolled file read roots</h4>";
            for root in roots["roots"].as_array().ok_or(Error::Integrity)? {
                let root_id = root["id"].as_str().ok_or(Error::Integrity)?;
                body += &format!(
                    "<p>{}: {} · available {}</p>",
                    escape(root_id),
                    escape(root["path"].as_str().ok_or(Error::Integrity)?),
                    root["available"]
                );
                body += &begin("set_code_search");
                body += &hidden("provider", id);
                body += &hidden("root_id", root_id);
                body +=
                    &format!("<p>Current indexed source: {}</p>", roots["source_root"] == root_id);
                body += "<label><input name=\"confirm\" type=\"checkbox\" value=\"yes\" required>Allow this provider to search Prowl's index of this source root and receive matching file snippets.</label><button>Use as indexed source</button></form>";
                body += &begin("disable_read_root");
                body += &hidden("provider", id);
                body += &hidden("root_id", root_id);
                body += "<button>Disable file root</button></form>";
            }
            body += &begin("set_code_search");
            body += &hidden("provider", id);
            body += "<button>Disable source search</button></form>";
            body += &begin("enroll_read_root");
            body += &hidden("provider", id);
            body += "<label>Root ID<input name=\"root_id\" required maxlength=\"64\"></label><label>Absolute directory<input name=\"path\" required maxlength=\"4096\"></label><label><input name=\"confirm\" type=\"checkbox\" value=\"yes\" required>Allow this gateway and its configured provider to read/list ordinary text files in this directory. Secret and credential Store paths remain blocked.</label><button>Enroll file root</button></form>";
            body += &begin("set_public_web");
            body += &hidden("provider", id);
            body += &format!(
                "<label><input name=\"enabled\" type=\"checkbox\" value=\"true\"{}>Enable public web reads</label><p>Allows model-selected public HTTP/S GET requests and up to three public redirects. URL requests reach those sites and fetched text reaches this provider. No account credentials/cookies are sent. Private/local addresses are blocked. Fix keeps fetch disabled.</p><button>Save web permission</button></form>",
                if provider["public_web_enabled"] == true { " checked" } else { "" }
            );
            body += &begin("set_read_only_tools");
            body += &hidden("provider", id);
            body += &format!(
                "<label><input name=\"enabled\" type=\"checkbox\" value=\"true\"{}>Enable read-only system tools</label><p>Allows the model to read installed packages, cached updates, user service status, process names, disk, kernel, GPU and network state. Results go to this gateway and may reach its configured external provider. Fix keeps these tools disabled.</p><button>Save tool permission</button></form>",
                if provider["read_only_tools_enabled"] == true { " checked" } else { "" }
            );
            body += &begin("update_prowl_provider");
            body += &hidden("provider", id);
            let models = provider["models"]
                .as_array()
                .ok_or(Error::Integrity)?
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            body += &format!(
                "<label>Offered route/model IDs<textarea name=\"models\">{}</textarea></label><label><input name=\"streaming\" type=\"checkbox\" value=\"true\"{}>Stream responses</label><button>Save gateway provider settings</button></form>",
                escape(&models),
                if provider["streaming"] == true { " checked" } else { "" }
            );
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
    if let Some(records) = job["tool_activity"].as_array().filter(|rows| !rows.is_empty()) {
        body += "<h3>Tool activity</h3><table><thead><tr><th>Tool</th><th>Status</th><th>Error</th></tr></thead><tbody>";
        for record in records {
            body += &format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(text(record, "name")),
                escape(text(record, "phase")),
                escape(text(record, "error"))
            );
        }
        body += "</tbody></table>";
    }
    if job["tool_activity_truncated"] == true {
        body += "<p>Tool activity was omitted to preserve space for the final result.</p>";
    }
    if job["needs_agent"] == true {
        body += &format!(
            "<h3>Agent handoff required</h3><p>{}</p><p>Continue explicitly in Agent to review the requested change.</p>",
            escape(text(job, "needs_agent_reason"))
        );
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
    if job["lane"] == "ask" && (job["status"] == "completed" || running) {
        body += "<h2>Continue in Agent</h2><p>Use the recorded question and answer as context. Chat remains separate.</p>";
        if running {
            body += "<p>This Ask is unfinished. Staging captures partial context and leaves Ask running. Immediate Run cancels active Ask work; neither action marks the Ask completed.</p>";
        }
        body += &begin("handoff");
        body += &hidden("ask_id", id);
        body += if running {
            "<button>Stage current partial context for next Agent message</button></form>"
        } else {
            "<button>Stage context for next Agent message</button></form>"
        };
        let sessions = engine.dispatch(Request::Sessions {})?;
        let agents = sessions["sessions"]
            .as_array()
            .ok_or(Error::Integrity)?
            .iter()
            .filter(|session| session["lane"] == "agent")
            .collect::<Vec<_>>();
        if !agents.is_empty() {
            body += &begin("handoff_run");
            body += &hidden("ask_id", id);
            body += &hidden("id", &nonce()?);
            body += "<label>Agent conversation<select name=\"session\" required>";
            for session in agents {
                let id = text(session, "id");
                body += &format!("<option value=\"{}\">{}</option>", escape(id), escape(id));
            }
            body += "</select></label><button>Run in selected Agent conversation</button></form>";
        } else {
            body += "<p>Create an Agent conversation in the workspace to run this context immediately.</p>";
        }
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
            body += "<h2>Use Prowl for assistant completions</h2><p>Connect adds a provider choice. Choose its model explicitly in the workspace; saved lane picks stay unchanged. Prowl may route prompts to external providers you configured.</p>";
            body += &begin("connect_prowl_provider");
            body += "<label>Provider name<input name=\"provider\" value=\"prowl\" required maxlength=\"64\"></label><label>Offered route/model IDs, one per line<textarea name=\"models\" required>auto:fast</textarea></label><label><input name=\"streaming\" type=\"checkbox\" checked>Stream responses</label><button>Connect Prowl provider</button></form>";
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

pub(super) fn usage(engine: &Arc<Engine>) -> Result<String> {
    let usage = engine.dispatch(Request::Usage {})?;
    let mut body = String::from(
        "<p><a href=\"/console\">Workspace</a></p><p>Explicitly enroll a private local tool data directory. Collection retains aggregate activity and optional opencode soft caps. Transcripts and credentials stay out of the report; provider quota is separate.</p>",
    );
    body += &begin("enroll_usage");
    body += "<label>Tool<select name=\"tool\"><option value=\"claude\">Claude</option><option value=\"codex\">Codex</option><option value=\"opencode\">opencode</option></select></label><label>Absolute private data directory<input name=\"path\" required maxlength=\"4096\"></label><label>Optional opencode five-hour soft cap<input name=\"five_hour_limit\" type=\"number\" min=\"1\" max=\"1000000000000000\"></label><label>Optional opencode seven-day soft cap<input name=\"seven_day_limit\" type=\"number\" min=\"1\" max=\"1000000000000000\"></label><button>Enroll local source</button></form>";
    for (tool, source) in usage["sources"].as_object().ok_or(Error::Integrity)? {
        body += &format!(
            "<section><h2>{}</h2><p>Enrolled directory: {}</p>",
            escape(tool),
            escape(text(source, "path"))
        );
        let report = &usage["tools"][tool];
        if report.is_null() {
            body += "<p>Not yet collected.</p>";
        } else {
            body += &format!(
                "<p>Source status: {}</p><p>Five-hour local tokens: {} · seven-day local tokens: {}</p><p>Provider quota: {}</p><pre>{}</pre>",
                escape(text(report, "source_status")),
                report["five_hour_tokens"],
                report["seven_day_tokens"],
                escape(text(report, "quota_status")),
                escape(&serde_json::to_string_pretty(report).map_err(|_| Error::Storage)?)
            );
        }
        for (action, label) in [
            ("collect_usage", "Collect local activity now"),
            ("disable_usage", "Disable source and clear cached activity"),
        ] {
            body += &begin(action);
            body += &hidden("tool", tool);
            body += &format!("<button>{label}</button></form>");
        }
        body += "</section>";
    }
    body += "<p>The running daemon owns the persistent five-minute schedule. Enrollment does not enable login startup, lingering or another collector process.</p>";
    Ok(page("Local tool usage", &body))
}

pub(super) fn fix_form() -> String {
    page(
        "Fix investigation",
        "<p><a href=\"/console\">Workspace</a></p><p>Review a finding and bounded native observations before starting an Agent investigation. Proposed repairs still need their separately authorized Ryoku provider and linked recovery.</p><form method=\"post\" action=\"/console/fix-preview\"><label>Finding type<select name=\"kind\"><option value=\"doctor\">Doctor</option><option value=\"tip\">Tip</option><option value=\"app\">App incident</option></select></label><label>Component<input name=\"component\" required maxlength=\"128\"></label><label>Finding and evidence<textarea name=\"evidence\" required maxlength=\"16384\"></textarea></label><button>Review destination and native evidence</button></form>",
    )
}
pub(super) fn fix_preview(engine: &Arc<Engine>, body: &[u8]) -> Result<String> {
    let fields = form(body)?;
    let kind = value(&fields, "kind")?;
    let component = value(&fields, "component")?;
    let evidence = value(&fields, "evidence")?;
    let preview = engine.dispatch(Request::FixPreview {
        kind: fix_kind(kind)?,
        component: component.into(),
        evidence: evidence.into(),
    })?;
    let mut body = format!(
        "<p><a href=\"/fix\">Back to finding</a></p><p>{}</p><h2>Destination</h2><pre>{}</pre><h2>Finding</h2><pre>{}</pre><h2>Bounded native observations</h2><pre>{}</pre><p>This review expires in {} seconds.</p>",
        escape(text(&preview, "disclosure")),
        escape(&serde_json::to_string_pretty(&preview["destination"]).map_err(|_| Error::Storage)?),
        escape(evidence),
        escape(text(&preview, "native_evidence")),
        preview["expires_in_seconds"]
    );
    body += &begin("fix");
    let id = nonce()?;
    for (name, value) in [
        ("id", id.as_str()),
        ("kind", kind),
        ("component", component),
        ("evidence", evidence),
        ("consent", text(&preview, "native_evidence_consent")),
    ] {
        body += &hidden(name, value);
    }
    body += "<label><input name=\"confirm\" type=\"checkbox\" value=\"yes\" required>I reviewed the destination, finding and native observations.</label><button>Confirm and start investigation</button></form>";
    Ok(page("Review Fix investigation", &body))
}
