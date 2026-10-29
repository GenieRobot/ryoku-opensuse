//! Runtime ACP handler: host reads use the captured owner grants, mutations
//! require the native writer integration, and permissions never authorize shell
//! text. Adapter-internal processes remain inside the proved jail.
use super::{Client, Config, Handler};
use crate::{
    Error, Result,
    provider::{Completion, Message},
    prowl::{
        ReadTools,
        activity::{Callbacks, Phase, Record},
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
pub(crate) struct Options<'a> {
    pub runner: &'a Path,
    pub ask: bool,
    pub thought: &'a mut dyn FnMut(&str) -> Result<()>,
    pub usage: &'a mut dyn FnMut(ContextUsage) -> Result<()>,
    pub metadata: &'a mut dyn FnMut(MetadataUpdate) -> Result<()>,
}
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextUsage {
    pub size: u64,
    pub used: u64,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Command {
    pub name: String,
    pub description: String,
    pub hint: Option<String>,
}
pub(crate) enum MetadataUpdate {
    Title(String),
    Commands(Vec<Command>),
}
pub(crate) fn valid_metadata_text(text: &str, max: usize) -> bool {
    text.len() <= max && !text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
}
impl Command {
    pub(crate) fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && valid_metadata_text(&self.name, 64)
            && valid_metadata_text(&self.description, 512)
            && self.hint.as_ref().is_none_or(|hint| valid_metadata_text(hint, 256))
    }
}
fn metadata_text(value: &Value, max: usize) -> Result<String> {
    let text = value.as_str().ok_or(Error::ProviderFailed)?;
    if !valid_metadata_text(text, max) {
        return Err(Error::ProviderFailed);
    }
    Ok(text.into())
}
struct Surface<'a, 'b> {
    context: &'a ReadTools,
    callbacks: &'a mut Callbacks<'b>,
    cancelled: &'a AtomicBool,
    deadline: Instant,
    text: String,
    collecting: bool,
    tools: BTreeMap<String, (u8, bool)>,
    ask: bool,
    denied: bool,
    actual_model: Option<String>,
    thought: &'a mut dyn FnMut(&str) -> Result<()>,
    usage: &'a mut dyn FnMut(ContextUsage) -> Result<()>,
    metadata: &'a mut dyn FnMut(MetadataUpdate) -> Result<()>,
}
impl Surface<'_, '_> {
    fn read(&self, params: &Value) -> Result<String> {
        let path = params["path"].as_str().ok_or(Error::InvalidRequest)?;
        let integer = |name: &str| {
            params
                .get(name)
                .filter(|value| !value.is_null())
                .map(|value| value.as_u64().ok_or(Error::InvalidRequest))
                .transpose()
        };
        crate::prowl::read_roots::read_text_window(
            &self.context.roots,
            path,
            integer("line")?,
            integer("limit")?,
            self.cancelled,
            self.deadline,
        )
    }

    fn tool(&mut self, id: &str, phase: Phase, error: Option<Error>) -> Result<()> {
        if id.len() > 256 || id.is_empty() || id.chars().any(char::is_control) {
            return Err(Error::ProviderFailed);
        }
        if !self.tools.contains_key(id) {
            if self.tools.len() >= 32 {
                return Err(Error::Limit);
            }
            let ordinal = self.tools.len() as u8;
            (self.callbacks.tool)(Record {
                ordinal,
                call_hash: crate::digest(id.as_bytes()),
                name: "acp_tool".into(),
                phase: Phase::Running,
                error: None,
            })?;
            self.tools.insert(id.into(), (ordinal, false));
        }
        let (ordinal, terminal) = self.tools.get_mut(id).ok_or(Error::Integrity)?;
        if phase != Phase::Running && !*terminal {
            (self.callbacks.tool)(Record {
                ordinal: *ordinal,
                call_hash: crate::digest(id.as_bytes()),
                name: "acp_tool".into(),
                phase,
                error: match phase {
                    Phase::Failed => error.or(Some(Error::ProviderFailed)),
                    Phase::Cancelled => Some(Error::Cancelled),
                    _ => None,
                },
            })?;
            *terminal = true;
        }
        Ok(())
    }
}
impl Handler for Surface<'_, '_> {
    fn capabilities(&self) -> Value {
        json!({"fs":{"readTextFile":!self.context.roots.is_empty(),"writeTextFile":false},"terminal":false})
    }
    fn request(&mut self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "fs/read_text_file" => Ok(json!({"content":self.read(params)?})),
            "fs/write_text_file" => {
                self.denied |= self.ask;
                Err(Error::Unauthorized)
            }
            "session/request_permission" => {
                let call = &params["toolCall"];
                let kernel = call["kind"] == "execute"
                    && self.context.system_queries
                    && call["rawInput"]["command"] == "/usr/bin/uname"
                    && call["rawInput"]["args"] == json!(["-r"]);
                let read = call["kind"] == "read" && self.read(&call["rawInput"]).is_ok();
                let allowed = read || kernel;
                if !allowed {
                    self.denied |= self.ask;
                }
                let rows = params["options"]
                    .as_array()
                    .filter(|rows| rows.len() <= 16)
                    .ok_or(Error::ProviderFailed)?;
                let kind = if allowed { "allow_once" } else { "reject_once" };
                let choice = rows
                    .iter()
                    .find(|row| row["kind"] == kind)
                    .map(|row| super::protocol::opaque(&row["optionId"]))
                    .transpose()?;
                if let Some(id) = call["toolCallId"].as_str() {
                    self.tool(
                        id,
                        if allowed { Phase::Running } else { Phase::Failed },
                        (!allowed).then_some(Error::Unauthorized),
                    )?;
                }
                Ok(match choice {
                    Some(option) => json!({"outcome":{"outcome":"selected","optionId":option}}),
                    None => json!({"outcome":{"outcome":"cancelled"}}),
                })
            }
            _ => Err(Error::NotFound),
        }
    }
    fn update(&mut self, params: &Value) -> Result<()> {
        let update = &params["update"];
        match update["sessionUpdate"].as_str().ok_or(Error::ProviderFailed)? {
            "agent_message_chunk" => {
                if update["content"]["type"] != "text" {
                    return Err(Error::ProviderUnavailable);
                }
                let text = update["content"]["text"].as_str().ok_or(Error::ProviderFailed)?;
                if self.collecting {
                    if self.text.len() + text.len() > 128 * 1024 {
                        return Err(Error::Limit);
                    }
                    self.text.push_str(text);
                    (self.callbacks.text)(text)?;
                }
            }
            "tool_call" | "tool_call_update" => {
                let id = update["toolCallId"].as_str().ok_or(Error::ProviderFailed)?;
                let phase = match update["status"].as_str().unwrap_or("in_progress") {
                    "pending" | "in_progress" => Phase::Running,
                    "completed" => Phase::Completed,
                    "failed" => Phase::Failed,
                    "cancelled" => Phase::Cancelled,
                    _ => return Err(Error::ProviderFailed),
                };
                self.tool(id, phase, None)?;
            }
            "current_model_update" | "model_update" => {
                let model = update
                    .get("currentModelId")
                    .or_else(|| update.get("modelId"))
                    .ok_or(Error::ProviderFailed)?;
                self.actual_model = Some(super::protocol::opaque(model)?);
            }
            "agent_thought_chunk" => {
                if update["content"]["type"] != "text" {
                    return Err(Error::ProviderUnavailable);
                }
                let text = update["content"]["text"].as_str().ok_or(Error::ProviderFailed)?;
                if self.collecting {
                    (self.thought)(text)?;
                }
            }
            "usage_update" => {
                let size = update["size"].as_u64().ok_or(Error::ProviderFailed)?;
                let used = update["used"].as_u64().ok_or(Error::ProviderFailed)?;
                (self.usage)(ContextUsage { size, used })?;
            }
            "available_commands_update" => {
                let rows = update["availableCommands"]
                    .as_array()
                    .filter(|rows| rows.len() <= 64)
                    .ok_or(Error::ProviderFailed)?;
                let mut commands = Vec::with_capacity(rows.len());
                for row in rows {
                    let name = metadata_text(&row["name"], 64)?;
                    if name.trim().is_empty() {
                        return Err(Error::ProviderFailed);
                    }
                    let description = row
                        .get("description")
                        .filter(|value| !value.is_null())
                        .map(|value| metadata_text(value, 512))
                        .transpose()?
                        .unwrap_or_default();
                    let hint = match row.get("input").filter(|input| !input.is_null()) {
                        Some(input) => input
                            .as_object()
                            .ok_or(Error::ProviderFailed)?
                            .get("hint")
                            .filter(|value| !value.is_null())
                            .map(|value| metadata_text(value, 256))
                            .transpose()?,
                        None => None,
                    };
                    commands.push(Command { name, description, hint });
                }
                (self.metadata)(MetadataUpdate::Commands(commands))?;
            }
            "session_info_update" => {
                if let Some(title) = update.get("title").filter(|value| !value.is_null()) {
                    (self.metadata)(MetadataUpdate::Title(
                        metadata_text(title, 256)?.trim().into(),
                    ))?;
                }
            }
            "user_message_chunk" | "current_mode_update" | "config_option_update" | "plan" => {}
            _ => return Err(Error::ProviderUnavailable),
        }
        Ok(())
    }
}
pub(crate) fn complete(
    config: &Config,
    model: &str,
    messages: &[Message],
    cancelled: &AtomicBool,
    context: &ReadTools,
    callbacks: &mut Callbacks<'_>,
    options: Options<'_>,
) -> Result<Completion> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut surface = Surface {
        context,
        callbacks,
        cancelled,
        deadline,
        text: String::new(),
        collecting: false,
        tools: BTreeMap::new(),
        ask: options.ask,
        denied: false,
        actual_model: None,
        thought: options.thought,
        usage: options.usage,
        metadata: options.metadata,
    };
    let mut client = Client::connect(config, options.runner, deadline, cancelled, &mut surface)?;
    client.new_session(deadline, cancelled, &mut surface)?;
    client.set_model(model, deadline, cancelled, &mut surface)?;
    let prompt = serde_json::to_string(messages).map_err(|_| Error::InvalidRequest)?;
    surface.collecting = true;
    let stop_reason = client.prompt(&prompt, deadline, cancelled, &mut surface)?;
    if surface.text.trim().is_empty() {
        if surface.denied {
            surface.text = "The requested operation requires explicit Agent authorization.".into();
        } else {
            return Err(Error::ProviderFailed);
        }
    }
    Ok(Completion {
        input_tokens: prompt.len().div_ceil(4) as u64,
        output_tokens: surface.text.len().div_ceil(4) as u64,
        text: surface.text,
        estimated: true,
        stop_reason: Some(stop_reason),
        actual_model: surface
            .actual_model
            .or_else(|| client.session.as_ref().and_then(|session| session.current_model.clone())),
        handoff_reason: surface
            .denied
            .then(|| "The requested operation requires explicit Agent authorization.".into()),
    })
}
pub(crate) fn catalog(
    config: &Config,
    runner: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<String>> {
    struct Quiet;
    impl Handler for Quiet {
        fn capabilities(&self) -> Value {
            json!({"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false})
        }
        fn request(&mut self, _: &str, _: &Value) -> Result<Value> {
            Err(Error::Unauthorized)
        }
        fn update(&mut self, _: &Value) -> Result<()> {
            Ok(())
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut handler = Quiet;
    let mut client = Client::connect(config, runner, deadline, cancelled, &mut handler)?;
    let session = client.new_session(deadline, cancelled, &mut handler)?;
    let mut models = session.models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    if models.is_empty() {
        models.push("adapter-default".into());
    }
    Ok(models)
}
