//! Bounded OpenAI tool-call assembly; names/arguments are data, never commands.
use crate::{
    Error, Result,
    provider::{Completion, stream::Decoder},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
const CALLS: usize = 8;
const ARGUMENTS: usize = 16 * 1024;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Function {
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: Function,
}
impl ToolCall {
    fn bounded(&self) -> Result<()> {
        if self.id.len() > 128
            || self.r#type.len() > 16
            || self.function.name.len() > 64
            || self.function.arguments.len() > ARGUMENTS
        {
            return Err(Error::Limit);
        }
        if self.id.contains('\0') || self.function.arguments.contains('\0') {
            return Err(Error::ProviderFailed);
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        self.bounded()?;
        if self.id.is_empty()
            || self.id.bytes().any(|b| b.is_ascii_control())
            || self.r#type != "function"
            || self.function.name.is_empty()
            || !self.function.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::ProviderFailed);
        }
        Ok(())
    }
}
pub(super) struct Round {
    pub completion: Completion,
    pub calls: Vec<ToolCall>,
}
fn validate(calls: &[ToolCall]) -> Result<()> {
    if calls.len() > CALLS {
        return Err(Error::Limit);
    }
    let mut ids = std::collections::BTreeSet::new();
    for call in calls {
        call.validate()?;
        if !ids.insert(&call.id) {
            return Err(Error::ProviderFailed);
        }
    }
    Ok(())
}
pub(super) fn json(bytes: &[u8], tools: bool) -> Result<Round> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::ProviderFailed)?;
    let message = &value["choices"][0]["message"];
    if message.get("function_call").is_some_and(|v| !v.is_null()) {
        return Err(Error::ProviderFailed);
    }
    let calls = match message.get("tool_calls").filter(|v| !v.is_null()) {
        Some(value) => serde_json::from_value::<Vec<ToolCall>>(value.clone())
            .map_err(|_| Error::ProviderFailed)?,
        None => Vec::new(),
    };
    validate(&calls)?;
    if !tools && !calls.is_empty() {
        return Err(Error::ProviderFailed);
    }
    let completion = crate::provider::parse_json_completion_allow_empty(bytes, !calls.is_empty())?;
    Ok(Round { completion, calls })
}
#[derive(Default)]
pub(super) struct Stream {
    normal: Decoder,
    pending: Vec<u8>,
    data: Vec<u8>,
    calls: BTreeMap<u64, ToolCall>,
    total: usize,
}
impl Stream {
    pub fn feed(&mut self, bytes: &[u8], emit: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
        self.total = self.total.checked_add(bytes.len()).ok_or(Error::Limit)?;
        if self.total > 1024 * 1024 {
            return Err(Error::Limit);
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let mut line = self.pending.drain(..=end).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&line, emit)?;
        }
        if self.pending.len() > 64 * 1024 {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn line(&mut self, line: &[u8], emit: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
        if line.is_empty() {
            return self.event(emit);
        }
        if let Some(data) = line.strip_prefix(b"data:") {
            if !self.data.is_empty() {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
            if self.data.len() > 64 * 1024 {
                return Err(Error::Limit);
            }
        }
        Ok(())
    }
    fn event(&mut self, emit: &mut impl FnMut(&str) -> Result<()>) -> Result<()> {
        if self.data.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.data);
        if data == b"[DONE]" {
            return self.normal.feed(b"data: [DONE]\n\n", emit);
        }
        let mut value: Value = serde_json::from_slice(&data).map_err(|_| Error::ProviderFailed)?;
        let choices = value["choices"].as_array_mut().ok_or(Error::ProviderFailed)?;
        if choices.len() > 1 {
            return Err(Error::ProviderFailed);
        }
        for choice in choices {
            if choice["index"].as_u64().is_some_and(|i| i != 0) {
                return Err(Error::ProviderFailed);
            }
            if let Some(calls) = choice["delta"].get("tool_calls").filter(|v| !v.is_null()) {
                let calls = calls.as_array().ok_or(Error::ProviderFailed)?;
                if calls.len() > CALLS {
                    return Err(Error::Limit);
                }
                for delta in calls {
                    let index = delta["index"].as_u64().ok_or(Error::ProviderFailed)?;
                    if index >= CALLS as u64 {
                        return Err(Error::Limit);
                    }
                    let call = self.calls.entry(index).or_default();
                    for (key, target) in [("id", &mut call.id), ("type", &mut call.r#type)] {
                        if let Some(value) = delta.get(key) {
                            target.push_str(value.as_str().ok_or(Error::ProviderFailed)?);
                        }
                    }
                    if let Some(function) = delta.get("function") {
                        if !function.is_object() {
                            return Err(Error::ProviderFailed);
                        }
                        for (key, target) in [
                            ("name", &mut call.function.name),
                            ("arguments", &mut call.function.arguments),
                        ] {
                            if let Some(value) = function.get(key) {
                                target.push_str(value.as_str().ok_or(Error::ProviderFailed)?);
                            }
                        }
                    }
                    call.bounded()?;
                }
            }
            if let Some(delta) = choice["delta"].as_object_mut() {
                delta.remove("tool_calls");
            }
            if choice["finish_reason"] == "tool_calls" {
                choice["finish_reason"] = Value::String("stop".into());
            }
        }
        let frame = format!(
            "data: {}\n\n",
            serde_json::to_string(&value).map_err(|_| Error::ProviderFailed)?
        );
        self.normal.feed(frame.as_bytes(), emit)
    }
    pub fn finish(mut self, emit: &mut impl FnMut(&str) -> Result<()>) -> Result<Round> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line, emit)?;
        }
        self.event(emit)?;
        let calls = self.calls.into_values().collect::<Vec<_>>();
        validate(&calls)?;
        let completion = self.normal.finish_allow_empty(emit, !calls.is_empty())?;
        Ok(Round { completion, calls })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn frame(value: Value) -> String {
        format!("data: {value}\n\n")
    }
    #[test]
    fn fragmented_utf8_arguments_and_multiple_calls_preserve_ids_names_usage_and_text() {
        let wire=[
            frame(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call-α","type":"function","function":{"name":"system_","arguments":"{\"topic\":"}}]}}]})),
            frame(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"query","arguments":"\"kernel\"}"}},{"index":1,"id":"other","type":"function","function":{"name":"unknown_tool","arguments":"{}"}}]},"finish_reason":"tool_calls"}]})),
            frame(json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":5},"model":"actual/◇"})),
            "data: [DONE]\n\n".into(),
        ].concat();
        let mut decoder = Stream::default();
        let mut progress = String::new();
        for byte in wire.as_bytes().chunks(1) {
            decoder
                .feed(byte, &mut |s| {
                    progress.push_str(s);
                    Ok(())
                })
                .unwrap();
        }
        let result = decoder.finish(&mut |_| Ok(())).unwrap();
        assert_eq!(result.calls.len(), 2);
        assert_eq!(result.calls[0].id, "call-α");
        assert_eq!(result.calls[0].function.name, "system_query");
        assert_eq!(result.calls[0].function.arguments, r#"{"topic":"kernel"}"#);
        assert!(progress.is_empty());
        assert!(result.completion.text.is_empty());
        assert_eq!((result.completion.input_tokens, result.completion.output_tokens), (3, 5));
        assert_eq!(result.completion.actual_model.as_deref(), Some("actual/◇"));
    }
    #[test]
    fn malformed_missing_terminal_duplicate_ids_and_oversize_calls_are_refused() {
        for value in [
            json!({"choices":[{"delta":{"tool_calls":[{"index":8}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":"not-array"}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"x".repeat(ARGUMENTS+1)}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"same","type":"function","function":{"name":"system_query","arguments":"{}"}},{"index":1,"id":"same","type":"function","function":{"name":"system_query","arguments":"{}"}}]}}]}),
        ] {
            let mut decoder = Stream::default();
            let wire = frame(value) + "data: [DONE]\n\n";
            assert!(
                decoder
                    .feed(wire.as_bytes(), &mut |_| Ok(()))
                    .and_then(|_| decoder.finish(&mut |_| Ok(())))
                    .is_err()
            );
        }
        let mut decoder = Stream::default();
        decoder
            .feed(
                frame(json!({"choices":[{"delta":{"content":"unfinished"}}]})).as_bytes(),
                &mut |_| Ok(()),
            )
            .unwrap();
        assert!(decoder.finish(&mut |_| Ok(())).is_err());
        assert!(json(br#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"system_query","arguments":"{}"}}]}}]}"#,false).is_err());
        let calls=(0..9).map(|i|json!({"id":format!("c{i}"),"type":"function","function":{"name":"system_query","arguments":"{}"}})).collect::<Vec<_>>();
        assert!(matches!(
            json(
                serde_json::to_vec(
                    &json!({"choices":[{"message":{"content":null,"tool_calls":calls}}]})
                )
                .unwrap()
                .as_slice(),
                true
            ),
            Err(Error::Limit)
        ));
    }
}
