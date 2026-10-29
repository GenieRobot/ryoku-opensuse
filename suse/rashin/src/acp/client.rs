//! One adapter connection, serialized ACP sessions, bounded synchronous RPC.
//! I/O never blocks cancellation: both pipes are nonblocking and the manager
//! owns the complete PID namespace until close/timeout/drop.
use super::{
    Config,
    protocol::{self, Capabilities, Frame, Id, Session},
    sandbox::Process,
};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
    process::{ChildStdin, ChildStdout},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Transport assigns this handler from the accepted lane/grants, never from an
/// adapter-supplied role. No raw RPC error or unknown field enters diagnostics.
pub trait Handler {
    fn capabilities(&self) -> Value;
    fn request(&mut self, method: &str, params: &Value) -> Result<Value>;
    fn update(&mut self, params: &Value) -> Result<()>;
}
pub struct Client {
    _process: Process,
    input: ChildStdin,
    output: ChildStdout,
    incoming: Vec<u8>,
    total: usize,
    next_id: i64,
    pub capabilities: Capabilities,
    pub session: Option<Session>,
    deferred: Vec<Value>,
    eof: bool,
}
impl Client {
    /// Supervisor identity for process health; not an adapter-selected target.
    pub fn process_id(&self) -> u32 {
        self._process.id()
    }

    pub fn connect(
        config: &Config,
        runner: &Path,
        deadline: Instant,
        cancelled: &AtomicBool,
        handler: &mut impl Handler,
    ) -> Result<Self> {
        if cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Error::ProviderUnavailable);
        }
        let mut process = Process::spawn(config, runner)?;
        let input = process.stdin()?;
        let output = process.stdout()?;
        rustix::fs::fcntl_setfl(&input, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::ProviderUnavailable)?;
        rustix::fs::fcntl_setfl(&output, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::ProviderUnavailable)?;
        let mut client = Self {
            _process: process,
            input,
            output,
            incoming: Vec::new(),
            total: 0,
            next_id: 0,
            capabilities: Capabilities::default(),
            session: None,
            deferred: Vec::new(),
            eof: false,
        };
        let result=client.call("initialize",json!({"protocolVersion":1,"clientCapabilities":handler.capabilities(),"clientInfo":{"name":"ryoku-rashin","title":"Ryoku Rashin","version":"1"}}),deadline,cancelled,handler)?;
        client.capabilities = Capabilities::initialize(&result)?;
        Ok(client)
    }
    fn write(
        &mut self,
        value: &Value,
        deadline: Instant,
        cancelled: Option<&AtomicBool>,
    ) -> Result<()> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidRequest)?;
        if bytes.len() > protocol::FRAME_BYTES {
            return Err(Error::Limit);
        }
        bytes.push(b'\n');
        let mut offset = 0;
        while offset < bytes.len() {
            if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::ProviderUnavailable);
            }
            match self.input.write(&bytes[offset..]) {
                Ok(0) => return Err(Error::ProviderUnavailable),
                Ok(written) => offset += written,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Error::ProviderUnavailable),
            }
        }
        Ok(())
    }
    fn read(&mut self) -> Result<Option<Frame>> {
        if let Some(end) = self.incoming.iter().position(|byte| *byte == b'\n') {
            let mut line = self.incoming.drain(..=end).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                return Err(Error::ProviderFailed);
            }
            return protocol::decode(&line).map(Some);
        }
        if self.eof {
            return Err(Error::ProviderUnavailable);
        }
        let mut bytes = [0u8; 8192];
        match self.output.read(&mut bytes) {
            Ok(0) => self.eof = true,
            Ok(length) => {
                self.total = self.total.checked_add(length).ok_or(Error::Limit)?;
                if self.total > 4 * 1024 * 1024 {
                    return Err(Error::Limit);
                }
                self.incoming.extend_from_slice(&bytes[..length]);
                if self.incoming.len() > protocol::FRAME_BYTES
                    && !self.incoming[..=protocol::FRAME_BYTES].contains(&b'\n')
                {
                    return Err(Error::Limit);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(Error::ProviderUnavailable),
        }
        Ok(None)
    }
    fn check_session(&self, params: &Value) -> Result<()> {
        let session = self.session.as_ref().ok_or(Error::ProviderFailed)?;
        if params["sessionId"] != session.id {
            return Err(Error::ProviderFailed);
        }
        Ok(())
    }
    fn call(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
        cancelled: &AtomicBool,
        handler: &mut impl Handler,
    ) -> Result<Value> {
        self.next_id = self.next_id.checked_add(1).ok_or(Error::Limit)?;
        let id = self.next_id;
        self.write(&protocol::request(id, method, params), deadline, Some(cancelled))?;
        let mut frames = 0usize;
        loop {
            if cancelled.load(Ordering::Acquire) {
                if let Some(session) = &self.session {
                    let notice =
                        protocol::notify("session/cancel", json!({"sessionId":session.id}));
                    let _ = self.write(&notice, Instant::now() + Duration::from_millis(100), None);
                }
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::ProviderUnavailable);
            }
            let Some(frame) = self.read()? else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            frames += 1;
            if frames > 1024 {
                return Err(Error::Limit);
            }
            match frame {
                Frame::Response { id: received, result } => {
                    if received != Id::Number(id) {
                        return Err(Error::ProviderFailed);
                    }
                    return result.map_err(|_| Error::ProviderFailed);
                }
                Frame::Notification { method, params } => {
                    if method != "session/update" {
                        return Err(Error::ProviderFailed);
                    }
                    if self.session.is_none() && matches!(method.as_str(), "session/update") {
                        if self.deferred.len() >= 32 {
                            return Err(Error::Limit);
                        }
                        self.deferred.push(params);
                    } else {
                        self.check_session(&params)?;
                        if params["update"]["sessionUpdate"] == "config_option_update" {
                            self.session
                                .as_mut()
                                .ok_or(Error::Integrity)?
                                .update_config(&params["update"])?;
                        }
                        handler.update(&params)?;
                    }
                }
                Frame::Request { id, method, params } => {
                    let response = if self.check_session(&params).is_err() {
                        protocol::reject(id, -32602)
                    } else {
                        match handler.request(&method, &params) {
                            Ok(result) => protocol::respond(id, result),
                            Err(Error::NotFound) => protocol::reject(id, -32601),
                            Err(Error::Cancelled) => return Err(Error::Cancelled),
                            Err(_) => protocol::reject(id, -32001),
                        }
                    };
                    self.write(&response, deadline, Some(cancelled))?;
                }
            }
        }
    }
    pub fn new_session(
        &mut self,
        deadline: Instant,
        cancelled: &AtomicBool,
        handler: &mut impl Handler,
    ) -> Result<Session> {
        self.session = None;
        let result = self.call(
            "session/new",
            json!({"cwd":"/home/adapter","mcpServers":[]}),
            deadline,
            cancelled,
            handler,
        )?;
        let session = Session::decode(&result, None)?;
        self.session = Some(session.clone());
        for params in std::mem::take(&mut self.deferred) {
            self.check_session(&params)?;
            if params["update"]["sessionUpdate"] == "config_option_update" {
                self.session.as_mut().ok_or(Error::Integrity)?.update_config(&params["update"])?;
            }
            handler.update(&params)?;
        }
        self.session.clone().ok_or(Error::Integrity)
    }
    pub fn load_session(
        &mut self,
        id: &str,
        deadline: Instant,
        cancelled: &AtomicBool,
        handler: &mut impl Handler,
    ) -> Result<Session> {
        if !self.capabilities.load_session {
            return Err(Error::ProviderUnavailable);
        }
        protocol::opaque(&json!(id))?;
        self.session = None;
        let result = self.call(
            "session/load",
            json!({"sessionId":id,"cwd":"/home/adapter","mcpServers":[]}),
            deadline,
            cancelled,
            handler,
        )?;
        let session = Session::decode(&result, Some(id))?;
        self.session = Some(session.clone());
        for params in std::mem::take(&mut self.deferred) {
            self.check_session(&params)?;
            if params["update"]["sessionUpdate"] == "config_option_update" {
                self.session.as_mut().ok_or(Error::Integrity)?.update_config(&params["update"])?;
            }
            handler.update(&params)?;
        }
        self.session.clone().ok_or(Error::Integrity)
    }
    pub fn set_model(
        &mut self,
        model: &str,
        deadline: Instant,
        cancelled: &AtomicBool,
        handler: &mut impl Handler,
    ) -> Result<()> {
        let session = self.session.as_ref().ok_or(Error::InvalidRequest)?;
        if model == "adapter-default" {
            return Ok(());
        }
        if !session.models.iter().any(|offered| offered.id == model) {
            return Err(Error::ProviderUnavailable);
        }
        let (method, params) = if let Some(option) = &session.model_option {
            (
                "session/set_config_option",
                json!({"sessionId":session.id,"configId":option,"value":model}),
            )
        } else {
            ("session/set_model", json!({"sessionId":session.id,"modelId":model}))
        };
        self.call(method, params, deadline, cancelled, handler)?;
        self.session.as_mut().ok_or(Error::Integrity)?.current_model = Some(model.into());
        Ok(())
    }
    pub fn prompt(
        &mut self,
        text: &str,
        deadline: Instant,
        cancelled: &AtomicBool,
        handler: &mut impl Handler,
    ) -> Result<String> {
        crate::text(text, 64 * 1024)?;
        let session = self.session.as_ref().ok_or(Error::InvalidRequest)?;
        let result = self.call(
            "session/prompt",
            json!({"sessionId":session.id,"prompt":[{"type":"text","text":text}]}),
            deadline,
            cancelled,
            handler,
        )?;
        let stop = result["stopReason"]
            .as_str()
            .filter(|reason| {
                ["end_turn", "max_tokens", "max_turn_requests", "refusal", "cancelled"]
                    .contains(reason)
            })
            .ok_or(Error::ProviderFailed)?;
        if stop == "cancelled" {
            return Err(Error::Cancelled);
        }
        Ok(stop.into())
    }
}
