//! Retained fast lane: at most four tool rounds, then one no-tools answer round.
//! One deadline covers all HTTP requests and fixed native probes.
use super::{Connection, completion, read_roots, system_query, tool_calls};
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
    pub roots: Vec<read_roots::Captured>,
}
impl Context {
    pub fn enabled(&self) -> bool {
        self.system_queries || !self.roots.is_empty()
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
    Value::Array(tools)
}
struct RoundOptions {
    schema: Value,
    streaming: bool,
    tools: bool,
    deadline: Instant,
}
fn round(
    connection: &Connection,
    model: &str,
    messages: &[Value],
    options: RoundOptions,
    cancelled: &AtomicBool,
    emit: &mut impl FnMut(&str) -> Result<()>,
) -> Result<tool_calls::Round> {
    let RoundOptions { streaming, tools, deadline, schema } = options;
    let mut payload = json!({"model":model,"messages":messages,"stream":streaming});
    if streaming {
        payload["stream_options"] = json!({"include_usage":true});
    }
    if tools {
        payload["tools"] = schema;
    }
    let payload = serde_json::to_vec(&payload).map_err(|_| Error::InvalidRequest)?;
    if payload.len() > crate::api::MAX_REQUEST_BYTES {
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
    emit: &mut impl FnMut(&str) -> Result<()>,
) -> Result<Completion> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut messages = messages
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::InvalidRequest)?;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut estimated = false;
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
            RoundOptions { streaming, tools: index < 4, deadline, schema: schema(context) },
            cancelled,
            emit,
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
            let result = if call.function.name == "system_query" && context.system_queries {
                match system_query::execute(&call.function.arguments, cancelled, deadline) {
                    Ok(value) => value,
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(error) => json!({"error":format!("{error}")}).to_string(),
                }
            } else if matches!(call.function.name.as_str(), "read_file" | "list_dir")
                && !context.roots.is_empty()
            {
                match read_roots::execute(
                    &context.roots,
                    &call.function.name,
                    &call.function.arguments,
                    cancelled,
                    deadline,
                ) {
                    Ok(value) => value.to_string(),
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(error) => json!({"error":format!("{error}")}).to_string(),
                }
            } else {
                json!({"error":"unknown_tool"}).to_string()
            };
            messages.push(json!({"role":"tool","tool_call_id":call.id,"content":result}));
        }
    }
    Err(Error::ProviderFailed)
}
