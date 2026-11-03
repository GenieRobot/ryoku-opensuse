//! Bounded OpenAI-compatible SSE decoding. Transport supplies only bytes from
//! the explicitly configured TLS endpoint or verified owner gateway. No event selects a URL or tool.
use super::{Completion, MAX_RESPONSE};
use crate::{Error, Result, text};
use serde_json::Value;

#[derive(Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
    data: Vec<u8>,
    answer: String,
    input: Option<u64>,
    output: Option<u64>,
    stop_reason: Option<String>,
    actual_model: Option<String>,
    total: u64,
    done: bool,
}
impl Decoder {
    pub(crate) fn feed(
        &mut self,
        bytes: &[u8],
        emit: &mut impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        self.total = self.total.checked_add(bytes.len() as u64).ok_or(Error::Limit)?;
        if self.total > MAX_RESPONSE {
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
        if line.starts_with(b":") {
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            if self.done {
                return Err(Error::ProviderFailed);
            }
            if !self.data.is_empty() {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
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
            self.done = true;
            return Ok(());
        }
        let value: Value = serde_json::from_slice(&data).map_err(|_| Error::ProviderFailed)?;
        if value.get("error").is_some_and(|error| !error.is_null()) {
            return Err(Error::ProviderFailed);
        }
        if let Some(model) = value["model"].as_str() {
            super::model_name(model).map_err(|_| Error::ProviderFailed)?;
            self.actual_model = Some(model.to_owned());
        }
        let choices = value["choices"].as_array().ok_or(Error::ProviderFailed)?;
        if choices.len() > 1 {
            return Err(Error::ProviderFailed);
        }
        for choice in choices {
            if choice["index"].as_u64().is_some_and(|index| index != 0) {
                return Err(Error::ProviderFailed);
            }
            let delta = &choice["delta"];
            if delta["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty())
                || delta.get("function_call").is_some_and(|call| !call.is_null())
            {
                return Err(Error::ProviderFailed);
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                if !["stop", "length", "content_filter"].contains(&reason) {
                    return Err(Error::ProviderFailed);
                }
                self.stop_reason = Some(reason.into());
            }
            if let Some(content) = delta.get("content").filter(|content| !content.is_null()) {
                let content = content.as_str().ok_or(Error::ProviderFailed)?;
                if self.answer.len() + content.len() > 128 * 1024 {
                    return Err(Error::Limit);
                }
                self.answer.push_str(content);
                if !content.is_empty() {
                    emit(content)?;
                }
            }
        }
        for (key, total) in
            [("prompt_tokens", &mut self.input), ("completion_tokens", &mut self.output)]
        {
            if let Some(count) = value["usage"].get(key).filter(|count| !count.is_null()) {
                *total = Some(count.as_u64().ok_or(Error::ProviderFailed)?);
            }
        }
        Ok(())
    }
    pub(crate) fn finish(self, emit: &mut impl FnMut(&str) -> Result<()>) -> Result<Completion> {
        self.finish_allow_empty(emit, false)
    }
    pub(crate) fn finish_allow_empty(
        mut self,
        emit: &mut impl FnMut(&str) -> Result<()>,
        allow_empty: bool,
    ) -> Result<Completion> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line, emit)?;
        }
        self.event(emit)?;
        if !self.done {
            return Err(Error::ProviderFailed);
        }
        if !(allow_empty && self.answer.is_empty()) {
            text(&self.answer, 128 * 1024).map_err(|_| Error::ProviderFailed)?;
        }
        Ok(Completion {
            handoff_reason: None,
            text: self.answer,
            input_tokens: self.input.unwrap_or_default(),
            output_tokens: self.output.unwrap_or_default(),
            estimated: self.input.is_none() || self.output.is_none(),
            stop_reason: self.stop_reason,
            actual_model: self.actual_model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_utf8_crlf_comments_multiline_data_and_usage_stream_incrementally() {
        let bytes = concat!(
            ": ping\r\n\r\ndata: {\"model\":\"concrete-fixture\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"γειά\"}}]}\r\n\r\n",
            "data: {\"choices\":\ndata: [{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"length\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":7}}\n\ndata: [DONE]\n\n"
        );
        let mut decoder = Decoder::default();
        let mut chunks = Vec::new();
        let mut emit = |text: &str| {
            chunks.push(text.to_owned());
            Ok(())
        };
        for byte in bytes.as_bytes().chunks(1) {
            decoder.feed(byte, &mut emit).unwrap();
        }
        let result = decoder.finish(&mut emit).unwrap();
        assert_eq!(result.text, "γειά world");
        assert_eq!(chunks, vec!["γειά", " world"]);
        assert_eq!((result.input_tokens, result.output_tokens), (5, 7));
        assert!(!result.estimated);
        assert_eq!(result.stop_reason.as_deref(), Some("length"));
        assert_eq!(result.actual_model.as_deref(), Some("concrete-fixture"));
    }
    #[test]
    fn missing_terminal_malformed_error_tool_and_post_terminal_data_are_refused() {
        for bytes in [
            b"data: {\"choices\":[{\"delta\":{\"content\":\"incomplete\"}}]}\n\n".as_slice(),
            b"data: malformed\n\n",
            b"data: {\"error\":{\"message\":\"PRIVATE_PROVIDER_BODY\"}}\n\n",
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{}]}}]}\n\n",
            b"data: [DONE]\n\ndata: {\"choices\":[]}\n\n",
        ] {
            let mut decoder = Decoder::default();
            let mut emit = |_: &str| Ok(());
            let result = decoder.feed(bytes, &mut emit).and_then(|()| decoder.finish(&mut emit));
            assert!(matches!(result, Err(Error::ProviderFailed)));
        }
    }
    #[test]
    fn callback_cancel_and_frame_response_limits_stop_decoding() {
        let mut decoder = Decoder::default();
        assert!(matches!(
            decoder.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n", &mut |_| {
                Err(Error::Cancelled)
            }),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            Decoder::default().feed(&vec![b'x'; 65537], &mut |_| Ok(())),
            Err(Error::Limit)
        ));
        assert!(matches!(
            Decoder::default().feed(&vec![b'x'; MAX_RESPONSE as usize + 1], &mut |_| Ok(())),
            Err(Error::Limit)
        ));
    }
    #[test]
    fn absent_usage_is_estimated_and_terminal_without_trailing_blank_line_is_supported() {
        let mut decoder = Decoder::default();
        let mut emit = |_: &str| Ok(());
        decoder.feed(b"data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]",&mut emit).unwrap();
        let completion = decoder.finish(&mut emit).unwrap();
        assert!(completion.estimated);
        assert_eq!((completion.input_tokens, completion.output_tokens), (0, 0));
        assert_eq!(completion.stop_reason.as_deref(), Some("stop"));
        assert!(completion.actual_model.is_none());
    }
}
