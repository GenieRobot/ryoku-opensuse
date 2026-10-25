//! Fixed provider protocols. Credentials travel over stdin to the distro curl
//! client, never command arguments, diagnostics, or assistant responses.
pub(crate) mod stream;
use crate::{Error, Result, text};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_RESPONSE: u64 = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Provider {
    /// A deterministic, network-free, retrieval provider for packaged acceptance.
    Local {},
    Acp {
        config: crate::acp::Config,
        models: Vec<String>,
    },
    /// Explicit, bounded acceptance provider. It performs no network I/O and
    /// is labeled as a fixture in every provider description and usage record.
    Fixture {
        delay_millis: u16,
        outcome: FixtureOutcome,
        #[serde(default)]
        catalog: Option<FixtureCatalog>,
    },
    OpenAiCompatible {
        endpoint: String,
        api_key: String,
        #[serde(default)]
        models: Vec<String>,
        #[serde(default)]
        streaming: bool,
    },
    /// References the separately enrolled private Prowl connection. No key is
    /// duplicated in provider configuration or exposed by provider inventory.
    ProwlGateway {
        #[serde(default)]
        models: Vec<String>,
        #[serde(default)]
        streaming: bool,
    },
    /// Captured worker-only credentials. Never accepted or stored through JSON.
    #[doc(hidden)]
    #[serde(skip)]
    ProwlResolved {
        port: u16,
        token: String,
        models: Vec<String>,
        streaming: bool,
    },
}

/// Explicit network-free catalog fixture; cached/offered sets model server
/// changes without sending a request or changing saved user picks implicitly.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureCatalog {
    pub cached: Vec<String>,
    pub offered: Vec<String>,
    pub fail: bool,
}

/// Optional owner edits preserve existing credential contents when no new key
/// is supplied. Descriptions never serialize the retained secret.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEdit {
    pub endpoint: Option<String>,
    pub api_key: Option<String>,
    pub models: Option<Vec<String>>,
    pub streaming: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "text", rename_all = "snake_case", deny_unknown_fields)]
pub enum FixtureOutcome {
    Answer(String),
    Failure,
    Stream { chunks: Vec<String>, fail: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated: bool,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub actual_model: Option<String>,
    #[serde(skip)]
    pub handoff_reason: Option<String>,
}

impl Provider {
    pub fn validate(&self) -> Result<()> {
        if let Self::Fixture { delay_millis, outcome, .. } = self {
            if *delay_millis > 1000 {
                return Err(Error::Limit);
            }
            if let FixtureOutcome::Stream { chunks, .. } = outcome {
                if chunks.is_empty()
                    || chunks.len() > 128
                    || chunks.iter().map(String::len).sum::<usize>() > 16 * 1024
                {
                    return Err(Error::Limit);
                }
                for chunk in chunks {
                    text(chunk, 16 * 1024)?;
                }
            }
            if let FixtureOutcome::Answer(answer) = outcome {
                text(answer, 16 * 1024)?;
            }
        }
        if let Self::Fixture { catalog: Some(catalog), .. } = self {
            for models in [&catalog.cached, &catalog.offered] {
                if models.len() > 128 {
                    return Err(Error::Limit);
                }
                for model in models {
                    model_name(model)?;
                }
            }
        }
        if let Self::OpenAiCompatible { endpoint, api_key, models, .. } = self {
            text(endpoint, 2048)?;
            text(api_key, 4096)?;
            // No URL credentials, fragments, queries, control bytes or proxy
            // overrides. Redirects are disabled and TLS verification is mandatory.
            if !endpoint.starts_with("https://")
                || endpoint.contains(['@', '?', '#'])
                || endpoint.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
                || endpoint
                    .trim_start_matches("https://")
                    .split('/')
                    .next()
                    .is_none_or(str::is_empty)
                || api_key.bytes().any(|b| b.is_ascii_control())
                || models.len() > 128
            {
                return Err(Error::InvalidRequest);
            }
            for model in models {
                model_name(model)?;
            }
        }
        if let Self::Acp { config, .. } = self {
            config.validate()?;
        }
        if let Self::ProwlGateway { models, .. }
        | Self::ProwlResolved { models, .. }
        | Self::Acp { models, .. } = self
        {
            if models.len() > 128 {
                return Err(Error::Limit);
            }
            for model in models {
                model_name(model)?;
            }
        }
        if let Self::ProwlResolved { port, token, .. } = self {
            crate::prowl::Connection { port: *port, token: token.clone() }.validate()?;
        }
        Ok(())
    }
    pub(crate) fn resolve(&self, connection: Option<&crate::prowl::Connection>) -> Result<Self> {
        if let Self::ProwlGateway { models, streaming } = self {
            let connection = connection.ok_or(Error::ProviderUnavailable)?;
            connection.validate()?;
            Ok(Self::ProwlResolved {
                port: connection.port,
                token: connection.token.clone(),
                models: models.clone(),
                streaming: *streaming,
            })
        } else {
            Ok(self.clone())
        }
    }
    pub(crate) fn is_prowl(&self) -> bool {
        matches!(self, Self::ProwlGateway { .. } | Self::ProwlResolved { .. })
    }

    pub(crate) fn is_tool_provider(&self) -> bool {
        self.is_prowl() || matches!(self, Self::Acp { .. })
    }

    pub fn edited(&self, edit: ProviderEdit) -> Result<Self> {
        if let Self::Acp { config, models } = self {
            if edit.endpoint.is_some()
                || edit.api_key.is_some()
                || edit.streaming.is_some()
                || edit.models.is_none()
            {
                return Err(Error::InvalidRequest);
            }
            let next = Self::Acp {
                config: config.clone(),
                models: edit.models.unwrap_or_else(|| models.clone()),
            };
            next.validate()?;
            return Ok(next);
        }
        if let Self::ProwlGateway { models, streaming } = self {
            if edit.endpoint.is_some()
                || edit.api_key.is_some()
                || (edit.models.is_none() && edit.streaming.is_none())
            {
                return Err(Error::InvalidRequest);
            }
            let next = Self::ProwlGateway {
                models: edit.models.unwrap_or_else(|| models.clone()),
                streaming: edit.streaming.unwrap_or(*streaming),
            };
            next.validate()?;
            return Ok(next);
        }
        let Self::OpenAiCompatible { endpoint, api_key, models, streaming } = self else {
            return Err(Error::InvalidRequest);
        };
        if edit.endpoint.is_none()
            && edit.api_key.is_none()
            && edit.models.is_none()
            && edit.streaming.is_none()
        {
            return Err(Error::InvalidRequest);
        }
        let next = Self::OpenAiCompatible {
            endpoint: edit.endpoint.unwrap_or_else(|| endpoint.clone()),
            api_key: edit.api_key.unwrap_or_else(|| api_key.clone()),
            models: edit.models.unwrap_or_else(|| models.clone()),
            streaming: edit.streaming.unwrap_or(*streaming),
        };
        next.validate()?;
        Ok(next)
    }

    pub fn models(&self) -> Vec<String> {
        match self {
            Self::Local {} => vec!["local-retrieval-v1".into()],
            Self::Fixture { catalog, .. } => catalog
                .as_ref()
                .map(|catalog| catalog.cached.clone())
                .unwrap_or_else(|| vec!["fixture-v1".into()]),
            Self::OpenAiCompatible { models, .. }
            | Self::ProwlGateway { models, .. }
            | Self::ProwlResolved { models, .. }
            | Self::Acp { models, .. } => models.clone(),
        }
    }

    pub(crate) fn with_catalog(mut self, models: Vec<String>) -> Result<Self> {
        match &mut self {
            Self::OpenAiCompatible { models: cached, .. }
            | Self::ProwlGateway { models: cached, .. }
            | Self::Acp { models: cached, .. } => *cached = models,
            Self::Fixture { catalog: Some(catalog), .. } => catalog.cached = models,
            Self::Local {} if models == ["local-retrieval-v1"] => {}
            Self::Fixture { catalog: None, .. } if models == ["fixture-v1"] => {}
            _ => return Err(Error::Integrity),
        }
        self.validate()?;
        Ok(self)
    }
    pub(crate) fn fetch_models(&self, cancelled: &AtomicBool) -> Result<Vec<String>> {
        self.validate()?;
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        match self {
            Self::Local {} => Ok(self.models()),
            Self::Acp { config, .. } => crate::acp::bridge::catalog(
                config,
                &std::env::current_exe().map_err(|_| Error::ProviderUnavailable)?,
                cancelled,
            ),
            Self::Fixture { delay_millis, catalog, .. } => {
                let until = std::time::Instant::now()
                    + std::time::Duration::from_millis(u64::from(*delay_millis));
                while std::time::Instant::now() < until {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(Error::Cancelled);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                if catalog.as_ref().is_some_and(|catalog| catalog.fail) {
                    return Err(Error::ProviderFailed);
                }
                Ok(catalog
                    .as_ref()
                    .map(|catalog| catalog.offered.clone())
                    .unwrap_or_else(|| self.models()))
            }
            Self::ProwlGateway { .. } => Err(Error::ProviderUnavailable),
            Self::ProwlResolved { port, token, .. } => {
                crate::prowl::Connection { port: *port, token: token.clone() }.models(cancelled)
            }
            Self::OpenAiCompatible { endpoint, api_key, .. } => {
                let config = format!(
                    "url = {}\nrequest = \"GET\"\nheader = {}\nheader = \"Accept: application/json\"\n",
                    quote(&format!("{}/models", endpoint.trim_end_matches('/'))),
                    quote(&format!("Authorization: Bearer {api_key}"))
                );
                parse_catalog(&catalog_request(&config, cancelled)?)
            }
        }
    }

    pub fn describe(&self, id: &str) -> serde_json::Value {
        match self {
            Self::Acp { config, .. } => {
                json!({"id":id,"kind":"acp","models":self.models(),"artifact_sha256":config.fingerprint(),"network":config.network_enabled(),"isolated":true})
            }
            Self::Local {} => {
                json!({"id":id,"kind":"local","models":self.models(),"requires_credentials":false})
            }
            Self::Fixture { .. } => {
                json!({"id":id,"kind":"fixture","models":self.models(),"requires_credentials":false,"test_only":true})
            }
            Self::ProwlGateway { streaming, .. } => {
                json!({"id":id,"kind":"prowl_gateway","models":self.models(),"streaming":streaming,"requires_credentials":true,"credential_reference":"private_prowl_connection"})
            }
            Self::ProwlResolved { port, streaming, .. } => {
                json!({"id":id,"kind":"prowl_gateway","models":self.models(),"streaming":streaming,"port":port,"requires_credentials":true,"credential_present":true,"available":true,"egress":"configured_gateway_routes_may_be_external"})
            }
            Self::OpenAiCompatible { endpoint, streaming, .. } => {
                json!({"id":id,"kind":"open_ai_compatible","endpoint":endpoint,"models":self.models(),"requires_credentials":true,"credential_present":true,"streaming":streaming})
            }
        }
    }

    pub fn complete(
        &self,
        model: &str,
        messages: &[Message],
        cancelled: &AtomicBool,
    ) -> Result<Completion> {
        self.complete_with_progress(model, messages, cancelled, &mut |_| Ok(()))
    }

    pub(crate) fn complete_with_read_tools(
        &self,
        model: &str,
        messages: &[Message],
        cancelled: &AtomicBool,
        context: &crate::prowl::ReadTools,
        callbacks: &mut crate::prowl::activity::Callbacks<'_>,
    ) -> Result<Completion> {
        match self {
            Self::ProwlResolved { port, token, streaming, .. } => {
                crate::prowl::Connection { port: *port, token: token.clone() }
                    .complete_with_tools(model, messages, *streaming, cancelled, context, callbacks)
            }
            _ => Err(Error::InvalidRequest),
        }
    }
    pub fn complete_with_progress(
        &self,
        model: &str,
        messages: &[Message],
        cancelled: &AtomicBool,
        emit: &mut impl FnMut(&str) -> Result<()>,
    ) -> Result<Completion> {
        self.validate()?;
        if !self.models().iter().any(|candidate| candidate == model) {
            return Err(Error::NotFound);
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        match self {
            Self::Acp { config, .. } => crate::acp::bridge::complete(
                config,
                model,
                messages,
                cancelled,
                &crate::prowl::ReadTools::default(),
                &mut crate::prowl::activity::Callbacks { text: emit, tool: &mut |_| Ok(()) },
                crate::acp::bridge::Options {
                    runner: &std::env::current_exe().map_err(|_| Error::ProviderUnavailable)?,
                    ask: true,
                },
            ),
            Self::Fixture { delay_millis, outcome, .. } => {
                if let FixtureOutcome::Stream { chunks, fail } = outcome {
                    let mut answer = String::new();
                    for chunk in chunks {
                        if cancelled.load(Ordering::Acquire) {
                            return Err(Error::Cancelled);
                        }
                        answer.push_str(chunk);
                        emit(chunk)?;
                        let deadline = std::time::Instant::now()
                            + std::time::Duration::from_millis(
                                u64::from(*delay_millis) / chunks.len() as u64,
                            );
                        while std::time::Instant::now() < deadline {
                            if cancelled.load(Ordering::Acquire) {
                                return Err(Error::Cancelled);
                            }
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                    }
                    if *fail {
                        return Err(Error::ProviderFailed);
                    }
                    return Ok(Completion {
                        text: answer,
                        input_tokens: messages
                            .iter()
                            .map(|message| message.content.len().div_ceil(4) as u64)
                            .sum(),
                        output_tokens: chunks
                            .iter()
                            .map(|chunk| chunk.len())
                            .sum::<usize>()
                            .div_ceil(4) as u64,
                        estimated: true,
                        stop_reason: None,
                        actual_model: Some(model.into()),
                        handoff_reason: None,
                    });
                }
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis((*delay_millis).into());
                while std::time::Instant::now() < deadline {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(Error::Cancelled);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if cancelled.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
                match outcome {
                    FixtureOutcome::Answer(answer) => Ok(Completion {
                        text: answer.clone(),
                        input_tokens: messages
                            .iter()
                            .map(|message| message.content.len().div_ceil(4) as u64)
                            .sum(),
                        output_tokens: answer.len().div_ceil(4) as u64,
                        estimated: true,
                        stop_reason: None,
                        actual_model: Some(model.into()),
                        handoff_reason: None,
                    }),
                    FixtureOutcome::Failure => Err(Error::ProviderFailed),
                    FixtureOutcome::Stream { .. } => {
                        unreachable!("stream fixture handled before delay")
                    }
                }
            }
            Self::Local {} => {
                let query = messages
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::User)
                    .ok_or(Error::InvalidRequest)?;
                let terms: Vec<_> = query
                    .content
                    .split_whitespace()
                    .map(|term| term.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
                    .filter(|term| term.len() > 2)
                    .collect();
                let evidence: Vec<_> = messages
                    .iter()
                    .filter(|m| m.role == Role::System)
                    .flat_map(|m| m.content.lines())
                    .filter(|line| {
                        line.starts_with("SOURCE ")
                            && terms.iter().any(|term| line.to_lowercase().contains(term))
                    })
                    .take(8)
                    .collect();
                let answer = if evidence.is_empty() {
                    "The local retrieval provider found no matching indexed evidence. Index relevant documents or select a configured external provider.".to_owned()
                } else {
                    format!("From your verified local index:\n{}", evidence.join("\n"))
                };
                Ok(Completion {
                    input_tokens: messages.iter().map(|m| m.content.len().div_ceil(4) as u64).sum(),
                    output_tokens: answer.len().div_ceil(4) as u64,
                    text: answer,
                    estimated: true,
                    stop_reason: None,
                    actual_model: Some(model.into()),
                    handoff_reason: None,
                })
            }
            Self::ProwlGateway { .. } => Err(Error::ProviderUnavailable),
            Self::ProwlResolved { port, token, streaming, .. } => {
                crate::prowl::Connection { port: *port, token: token.clone() }
                    .complete(model, messages, *streaming, cancelled, emit)
            }
            Self::OpenAiCompatible { endpoint, api_key, streaming, .. } => {
                let mut body = json!({"model":model,"messages":messages,"stream":streaming});
                if *streaming {
                    body["stream_options"] = json!({"include_usage":true});
                }
                let config = format!(
                    "url = {}\nheader = {}\nheader = \"Content-Type: application/json\"\ndata = {}\n",
                    quote(&format!("{}/chat/completions", endpoint.trim_end_matches('/'))),
                    quote(&format!("Authorization: Bearer {api_key}")),
                    quote(&body.to_string())
                );
                let mut child = client_command().spawn().map_err(|_| Error::ProviderUnavailable)?;
                let result = (|| {
                    child
                        .stdin
                        .take()
                        .ok_or(Error::ProviderFailed)?
                        .write_all(config.as_bytes())
                        .map_err(|_| Error::ProviderFailed)?;
                    let mut bytes = Vec::new();
                    let mut decoder = stream::Decoder::default();
                    let mut stdout = child.stdout.take().ok_or(Error::ProviderFailed)?;
                    rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
                        .map_err(|_| Error::ProviderFailed)?;
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(46);
                    loop {
                        if cancelled.load(Ordering::Acquire) {
                            return Err(Error::Cancelled);
                        }
                        if std::time::Instant::now() >= deadline {
                            return Err(Error::ProviderFailed);
                        }
                        let mut chunk = [0; 8192];
                        match stdout.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(count) => {
                                if *streaming {
                                    decoder.feed(&chunk[..count], emit)?;
                                } else {
                                    bytes.extend_from_slice(&chunk[..count]);
                                }
                                if bytes.len() as u64 > MAX_RESPONSE {
                                    return Err(Error::Limit);
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(std::time::Duration::from_millis(5))
                            }
                            Err(_) => return Err(Error::ProviderFailed),
                        }
                    }
                    loop {
                        if cancelled.load(Ordering::Acquire) {
                            return Err(Error::Cancelled);
                        }
                        if std::time::Instant::now() >= deadline {
                            return Err(Error::ProviderFailed);
                        }
                        if let Some(status) = child.try_wait().map_err(|_| Error::ProviderFailed)? {
                            if !status.success() {
                                return Err(Error::ProviderFailed);
                            }
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    if cancelled.load(Ordering::Acquire) {
                        return Err(Error::Cancelled);
                    }
                    if *streaming {
                        return decoder.finish(emit);
                    }
                    parse_json_completion(&bytes)
                })();
                if result.is_err() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                result
            }
        }
    }
}

pub(crate) fn parse_json_completion(bytes: &[u8]) -> Result<Completion> {
    parse_json_completion_allow_empty(bytes, false)
}
pub(crate) fn parse_json_completion_allow_empty(
    bytes: &[u8],
    allow_empty: bool,
) -> Result<Completion> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error::ProviderFailed)?;
    let content = value.pointer("/choices/0/message/content");
    let answer = if allow_empty && content.is_none_or(|v| v.is_null()) {
        String::new()
    } else {
        content.and_then(|v| v.as_str()).ok_or(Error::ProviderFailed)?.to_owned()
    };
    if !(allow_empty && answer.is_empty()) {
        text(&answer, 128 * 1024).map_err(|_| Error::ProviderFailed)?;
    }
    let input = value.pointer("/usage/prompt_tokens").and_then(|v| v.as_u64());
    let output = value.pointer("/usage/completion_tokens").and_then(|v| v.as_u64());
    let actual_model = value
        .get("model")
        .and_then(|value| value.as_str())
        .map(|name| {
            model_name(name).map_err(|_| Error::ProviderFailed)?;
            Ok(name.to_owned())
        })
        .transpose()?;
    Ok(Completion {
        text: answer,
        input_tokens: input.unwrap_or_default(),
        output_tokens: output.unwrap_or_default(),
        estimated: input.is_none() || output.is_none(),
        stop_reason: value
            .pointer("/choices/0/finish_reason")
            .and_then(|value| value.as_str())
            .filter(|reason| {
                reason.len() <= 64
                    && reason.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .map(str::to_owned),
        actual_model,
        handoff_reason: None,
    })
}

pub(crate) fn parse_catalog(bytes: &[u8]) -> Result<Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error::ProviderFailed)?;
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Err(Error::ProviderFailed);
    }
    let rows = value["data"].as_array().ok_or(Error::ProviderFailed)?;
    if rows.len() > 128 {
        return Err(Error::Limit);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut models = Vec::new();
    for row in rows {
        let id = row["id"].as_str().ok_or(Error::ProviderFailed)?;
        model_name(id).map_err(|_| Error::ProviderFailed)?;
        if seen.insert(id.to_owned()) {
            models.push(id.to_owned());
        }
    }
    Ok(models)
}
fn catalog_request(config: &str, cancelled: &AtomicBool) -> Result<Vec<u8>> {
    let mut child = client_command().spawn().map_err(|_| Error::ProviderUnavailable)?;
    let result = (|| {
        child
            .stdin
            .take()
            .ok_or(Error::ProviderFailed)?
            .write_all(config.as_bytes())
            .map_err(|_| Error::ProviderFailed)?;
        let mut stdout = child.stdout.take().ok_or(Error::ProviderFailed)?;
        rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::ProviderFailed)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(46);
        let mut bytes = Vec::new();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::ProviderFailed);
            }
            let mut chunk = [0; 8192];
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    bytes.extend_from_slice(&chunk[..count]);
                    if bytes.len() as u64 > MAX_RESPONSE {
                        return Err(Error::Limit);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(_) => return Err(Error::ProviderFailed),
            }
        }
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::ProviderFailed);
            }
            if let Some(status) = child.try_wait().map_err(|_| Error::ProviderFailed)? {
                if !status.success() {
                    return Err(Error::ProviderFailed);
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn model_name(model: &str) -> Result<()> {
    text(model, 128)?;
    // Model IDs are opaque JSON values chosen from the configured allowlist.
    // They never become a URL, command argument, path or executable syntax.
    if model.chars().any(char::is_control) {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r")
    )
}

const CURL_ARGS: &[&str] = &[
    "--disable",
    "--silent",
    "--no-buffer",
    "--fail",
    "--proto",
    "=https",
    "--max-redirs",
    "0",
    "--connect-timeout",
    "5",
    "--max-time",
    "45",
    "--max-filesize",
    "1048576",
    "--proxy",
    "",
    "--config",
    "-",
];

fn private_environment(command: &mut Command) {
    command.env_clear().env("LANG", "C");
}

fn client_command() -> Command {
    let mut command = Command::new("/usr/bin/curl");
    private_environment(&mut command);
    command.args(CURL_ARGS).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_client_has_fixed_no_shell_arguments_and_resource_limits() {
        let command = client_command();
        assert_eq!(command.get_program(), "/usr/bin/curl");
        assert_eq!(
            command.get_args().map(|arg| arg.to_str().unwrap()).collect::<Vec<_>>(),
            vec![
                "--disable",
                "--silent",
                "--no-buffer",
                "--fail",
                "--proto",
                "=https",
                "--max-redirs",
                "0",
                "--connect-timeout",
                "5",
                "--max-time",
                "45",
                "--max-filesize",
                "1048576",
                "--proxy",
                "",
                "--config",
                "-"
            ]
        );
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            vec![(std::ffi::OsStr::new("LANG"), Some(std::ffi::OsStr::new("C")))]
        );
        let debug = format!("{command:?}");
        assert!(!debug.contains("synthetic-private-key"));
        assert!(!debug.contains("synthetic-private-prompt"));
        assert!(!CURL_ARGS.iter().any(|argument| argument.contains("output")
            || argument.contains("insecure")
            || argument.contains("location")));
    }

    #[test]
    fn client_environment_contains_no_inherited_credentials_or_proxy_config() {
        let mut command = Command::new("/usr/bin/env");
        private_environment(&mut command);
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"LANG=C\n");
    }

    #[test]
    fn config_values_cannot_introduce_extra_curl_config_lines() {
        assert_eq!(quote("a\\b\"c\nd\r"), "\"a\\\\b\\\"c\\nd\\r\"");
        assert_eq!(quote("synthetic-private-key").lines().count(), 1);
        assert_eq!(quote("synthetic-private-prompt\nnext line").lines().count(), 1);
    }
    #[test]
    fn catalog_parser_preserves_opaque_ids_empty_sets_and_refuses_invalid_or_large_data() {
        assert_eq!(
            parse_catalog(br#"{"data":[{"id":"model@one#profile"},{"id":"model@one#profile"}]}"#)
                .unwrap(),
            vec!["model@one#profile"]
        );
        assert!(parse_catalog(br#"{"data":[]}"#).unwrap().is_empty());
        for bytes in [
            b"not JSON".as_slice(),
            br#"{"data":[{}]}"#,
            br#"{"data":[],"error":{"message":"PRIVATE_API_BODY"}}"#,
            br#"{"data":[{"id":"bad\nmodel"}]}"#,
        ] {
            assert!(matches!(parse_catalog(bytes), Err(Error::ProviderFailed)));
        }
        let bytes=serde_json::to_vec(&json!({"data":(0..129).map(|n|json!({"id":format!("model-{n}")})).collect::<Vec<_>>()})).unwrap();
        assert!(matches!(parse_catalog(&bytes), Err(Error::Limit)));
    }

    #[test]
    fn prowl_provider_keeps_credential_reference_and_resolved_context_cannot_enter_json_state() {
        let provider = Provider::ProwlGateway { models: vec!["auto:fast".into()], streaming: true };
        let connection =
            crate::prowl::Connection { port: 8788, token: "PRIVATE_RESOLVED_PROWL_KEY".into() };
        let resolved = provider.resolve(Some(&connection)).unwrap();
        assert!(serde_json::to_string(&resolved).is_err());
        assert!(serde_json::from_str::<Provider>(r#"{"kind":"prowl_resolved","port":8788,"token":"not-input","models":[],"streaming":true}"#).is_err());
        assert!(!serde_json::to_string(&provider).unwrap().contains("PRIVATE_RESOLVED_PROWL_KEY"));
        assert_eq!(provider.resolve(None).err(), Some(Error::ProviderUnavailable));
        assert!(
            provider
                .edited(ProviderEdit {
                    api_key: Some("must-use-private-connection".into()),
                    ..ProviderEdit::default()
                })
                .is_err()
        );
        let edited = provider
            .edited(ProviderEdit {
                models: Some(vec!["opaque/model ◇".into()]),
                streaming: Some(false),
                ..ProviderEdit::default()
            })
            .unwrap();
        assert_eq!(edited.models(), vec!["opaque/model ◇"]);
        assert!(edited.is_prowl());
    }
}
