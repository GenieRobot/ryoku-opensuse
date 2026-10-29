//! Retained fast lane: at most four tool rounds, then one no-tools answer round.
//! One deadline covers all HTTP requests and fixed native probes.
use super::{
    Connection,
    activity::{Callbacks, Phase, Record},
    completion, read_roots, system_query, tool_calls,
};
use crate::{
    Error, Result,
    provider::{Completion, Message},
};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
#[derive(Default)]
pub(crate) struct Context {
    pub system_queries: bool,
    pub public_web: bool,
    pub roots: Vec<read_roots::Captured>,
    pub code_root: Option<String>,
}
impl Context {
    pub fn enabled(&self) -> bool {
        self.system_queries || self.public_web || !self.roots.is_empty()
    }
}
fn schema(context: &Context) -> Value {
    let mut tools = Vec::new();
    if context.system_queries {
        tools.extend(json!([{"type":"function","function":{"name":"system_query","description":"Read native system state. Enabled by the owner; results may be sent to the configured gateway/provider.","parameters":{"type":"object","additionalProperties":false,"properties":{"topic":{"type":"string","enum":["packages","updates","service","processes","disk","kernel","gpu","network"]},"arg":{"type":"string","description":"Optional installed package capability or user service name"}},"required":["topic"]}}}]).as_array().expect("fixed tool schema").clone());
    }
    if !context.roots.is_empty() {
        let roots = context.roots.iter().map(|root| root.describe()).collect::<Vec<_>>();
        for (name, description) in [
            ("read_file", "Read a text file, at most32KiB"),
            ("list_dir", "List at most200 directory entries"),
        ] {
            tools.push(json!({"type":"function","function":{"name":name,"description":format!("{description}. Only these owner-enrolled roots are authorized: {roots:?}. Use an absolute path or ROOT_ID:/relative/path. No symlink,secret or credential Store reads."),"parameters":{"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"}},"required":["path"]}}}));
        }
    }
    if let Some(root_id) = &context.code_root {
        tools.push(json!({"type":"function","function":{"name":"search_code","description":format!("Search the configured Prowl source index for enrolled root {root_id}. Returns bounded current-root source snippets."),"parameters":{"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"]}}}));
    }
    if context.public_web {
        tools.push(json!({"type":"function","function":{"name":"fetch_url","description":"Fetch a public HTTP(S) page as bounded text. No credentials/cookies. Every DNS result and redirect must stay public.","parameters":{"type":"object","additionalProperties":false,"properties":{"url":{"type":"string"}},"required":["url"]}}}));
    }
    Value::Array(tools)
}
struct RoundOptions {
    schema: Value,
    streaming: bool,
    tools: bool,
    deadline: Instant,
    max_payload: usize,
}
fn round(
    connection: &Connection,
    model: &str,
    messages: &[Value],
    options: RoundOptions,
    cancelled: &AtomicBool,
    emit: &mut impl FnMut(&str) -> Result<()>,
) -> Result<tool_calls::Round> {
    let RoundOptions { streaming, tools, deadline, schema, max_payload } = options;
    let mut payload = json!({"model":model,"messages":messages,"stream":streaming});
    if streaming {
        payload["stream_options"] = json!({"include_usage":true});
    }
    if tools {
        payload["tools"] = schema;
    }
    let payload = serde_json::to_vec(&payload).map_err(|_| Error::InvalidRequest)?;
    if payload.len() > max_payload {
        return Err(Error::Limit);
    }
    let result = if streaming {
        let mut decoder = tool_calls::Stream::default();
        completion::request_until(connection, Some(&payload), cancelled, deadline, &mut |bytes| {
            decoder.feed(bytes, emit)
        })?;
        decoder.finish(emit)?
    } else {
        let mut bytes = Vec::new();
        completion::request_until(connection, Some(&payload), cancelled, deadline, &mut |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })?;
        tool_calls::json(&bytes, tools)?
    };
    if !tools && !result.calls.is_empty() {
        return Err(Error::ProviderFailed);
    }
    Ok(result)
}
pub(super) fn complete(
    connection: &Connection,
    model: &str,
    messages: &[Message],
    streaming: bool,
    cancelled: &AtomicBool,
    context: &Context,
    callbacks: &mut Callbacks<'_>,
) -> Result<Completion> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let max_payload = if messages.iter().any(|message| !message.images.is_empty()) {
        crate::api::MAX_IMAGE_REQUEST_BYTES
    } else {
        crate::api::MAX_REQUEST_BYTES
    };
    let mut messages = crate::provider::wire_messages(messages)?;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut estimated = false;
    let mut sequence = 0u8;
    for index in 0..=4 {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::ProviderUnavailable);
        }
        let mut result = round(
            connection,
            model,
            &messages,
            RoundOptions {
                streaming,
                tools: index < 4,
                deadline,
                schema: schema(context),
                max_payload,
            },
            cancelled,
            &mut |text| (callbacks.text)(text),
        )?;
        input = input.checked_add(result.completion.input_tokens).ok_or(Error::Limit)?;
        output = output.checked_add(result.completion.output_tokens).ok_or(Error::Limit)?;
        estimated |= result.completion.estimated;
        if result.calls.is_empty() {
            result.completion.input_tokens = input;
            result.completion.output_tokens = output;
            result.completion.estimated = estimated;
            return Ok(result.completion);
        }
        messages.push(
            json!({"role":"assistant","content":result.completion.text,"tool_calls":result.calls}),
        );
        for call in result.calls {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if sequence >= 32 {
                return Err(Error::Limit);
            }
            let mut event = Record {
                ordinal: sequence,
                call_hash: crate::digest(call.id.as_bytes()),
                name: match call.function.name.as_str() {
                    "system_query" | "read_file" | "list_dir" | "search_code" | "fetch_url" => {
                        call.function.name.clone()
                    }
                    _ => "unknown_tool".into(),
                },
                phase: Phase::Running,
                error: None,
            };
            sequence += 1;
            (callbacks.tool)(event.clone())?;
            let output = if call.function.name == "system_query" && context.system_queries {
                Some(system_query::execute(&call.function.arguments, cancelled, deadline))
            } else if call.function.name == "fetch_url" && context.public_web {
                Some(super::public_web::execute(&call.function.arguments, cancelled, deadline))
            } else if call.function.name == "search_code" && context.code_root.is_some() {
                Some(
                    context
                        .roots
                        .iter()
                        .find(|root| Some(root.id()) == context.code_root.as_deref())
                        .ok_or(Error::Integrity)
                        .and_then(|root| {
                            super::code_search::execute(
                                connection,
                                root,
                                &call.function.arguments,
                                cancelled,
                                deadline,
                            )
                        }),
                )
            } else if matches!(call.function.name.as_str(), "read_file" | "list_dir")
                && !context.roots.is_empty()
            {
                Some(
                    read_roots::execute(
                        &context.roots,
                        &call.function.name,
                        &call.function.arguments,
                        cancelled,
                        deadline,
                    )
                    .map(|v| v.to_string()),
                )
            } else {
                None
            };
            let result = match output {
                Some(Ok(text)) => {
                    event.phase = Phase::Completed;
                    text
                }
                Some(Err(Error::Cancelled)) => {
                    event.phase = Phase::Cancelled;
                    event.error = Some(Error::Cancelled);
                    (callbacks.tool)(event)?;
                    return Err(Error::Cancelled);
                }
                Some(Err(error)) => {
                    event.phase = Phase::Failed;
                    event.error = Some(error);
                    json!({"error":format!("{error}")}).to_string()
                }
                None => {
                    event.phase = Phase::Failed;
                    event.error = Some(Error::InvalidRequest);
                    json!({"error":"unknown_tool"}).to_string()
                }
            };
            (callbacks.tool)(event)?;
            messages.push(json!({"role":"tool","tool_call_id":call.id,"content":result}));
        }
    }
    Err(Error::ProviderFailed)
}
