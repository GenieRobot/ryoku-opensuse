//! Fixed provider protocols. Credentials travel over stdin to the distro curl
//! client, never command arguments, diagnostics, or assistant responses.
mod stream;
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
    /// Explicit, bounded acceptance provider. It performs no network I/O and
    /// is labeled as a fixture in every provider description and usage record.
    Fixture { delay_millis: u16, outcome: FixtureOutcome },
    OpenAiCompatible {
        endpoint: String,
        api_key: String,
        models: Vec<String>,
        #[serde(default)]
        streaming: bool,
    },
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
}

impl Provider {
    pub fn validate(&self) -> Result<()> {
        if let Self::Fixture { delay_millis, outcome } = self {
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
                || models.is_empty()
                || models.len() > 128
            {
                return Err(Error::InvalidRequest);
            }
            for model in models {
                model_name(model)?;
            }
        }
        Ok(())
    }

    pub fn models(&self) -> Vec<String> {
        match self {
            Self::Local {} => vec!["local-retrieval-v1".into()],
            Self::Fixture { .. } => vec!["fixture-v1".into()],
            Self::OpenAiCompatible { models, .. } => models.clone(),
        }
    }

    pub fn describe(&self, id: &str) -> serde_json::Value {
        match self {
            Self::Local {} => {
                json!({"id":id,"kind":"local","models":self.models(),"requires_credentials":false})
            }
            Self::Fixture { .. } => {
                json!({"id":id,"kind":"fixture","models":self.models(),"requires_credentials":false,"test_only":true})
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
            Self::Fixture { delay_millis, outcome } => {
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
                })
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
                    let value: serde_json::Value =
                        serde_json::from_slice(&bytes).map_err(|_| Error::ProviderFailed)?;
                    let answer = value
                        .pointer("/choices/0/message/content")
                        .and_then(|v| v.as_str())
                        .ok_or(Error::ProviderFailed)?
                        .to_owned();
                    text(&answer, 128 * 1024).map_err(|_| Error::ProviderFailed)?;
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
                                    && reason
                                        .bytes()
                                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                            })
                            .map(str::to_owned),
                        actual_model,
                    })
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

pub fn model_name(model: &str) -> Result<()> {
    text(model, 128)?;
    if !model.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_.:-".contains(&b)) {
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
}
