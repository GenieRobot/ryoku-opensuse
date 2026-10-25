//! Prowl's fixed OpenAI-compatible routes over one authenticated owner stream.
//! Connection/token are captured from private account configuration, not a URL
//! or model instruction. All writes follow post-connect peer UID verification.
use super::{
    Connection,
    transport::{response_to, verify_peer},
};
use crate::{
    Error, Result,
    provider::{Completion, Message},
};
use std::{
    io::{BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
const WIRE_LIMIT: u64 = 1024 * 1024 + 32 * 1024;
fn remaining(deadline: Instant, cancelled: &AtomicBool) -> Result<Duration> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(Error::ProviderUnavailable)
}
fn write(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<()> {
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline, cancelled)?.min(Duration::from_millis(50))))
            .map_err(|_| Error::ProviderUnavailable)?;
        match stream.write(bytes) {
            Ok(0) => return Err(Error::ProviderUnavailable),
            Ok(count) => bytes = &bytes[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(Error::ProviderUnavailable),
        }
    }
    Ok(())
}
struct Reader<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
    cancelled: &'a AtomicBool,
}
impl Read for Reader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let remaining = remaining(self.deadline, self.cancelled)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::ConnectionAborted))?;
            self.stream.set_read_timeout(Some(remaining.min(Duration::from_millis(50))))?;
            match self.stream.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                result => return result,
            }
        }
    }
}
fn request(
    connection: &Connection,
    body: Option<&[u8]>,
    cancelled: &AtomicBool,
    emit: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    request_until(connection, body, cancelled, Instant::now() + Duration::from_secs(45), emit)
}
pub(super) fn request_until(
    connection: &Connection,
    body: Option<&[u8]>,
    cancelled: &AtomicBool,
    deadline: Instant,
    emit: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let endpoint = if body.is_some() { Endpoint::Completions } else { Endpoint::Models };
    request_endpoint(connection, endpoint, body, cancelled, deadline, emit)
}
pub(super) enum Endpoint<'a> {
    Completions,
    Models,
    CodeSearch { repo: &'a str, query: &'a str },
}
pub(super) fn request_endpoint(
    connection: &Connection,
    endpoint: Endpoint<'_>,
    body: Option<&[u8]>,
    cancelled: &AtomicBool,
    deadline: Instant,
    emit: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    remaining(deadline, cancelled)?;
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, connection.port));
    let mut stream = TcpStream::connect_timeout(
        &address,
        remaining(deadline, cancelled)?.min(Duration::from_secs(1)),
    )
    .map_err(|_| Error::ProviderUnavailable)?;
    // Ownership proof uses a short independent bound; no request bytes before it.
    verify_peer(&stream, deadline.min(Instant::now() + Duration::from_secs(3)))?;
    remaining(deadline, cancelled)?;
    let (method, path) = match endpoint {
        Endpoint::Completions => ("POST", "/v1/chat/completions".to_owned()),
        Endpoint::Models => ("GET", "/v1/models".to_owned()),
        Endpoint::CodeSearch { repo, query } => ("GET", super::code_search::path(repo, query)?),
    };
    let header = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nAccept-Encoding: identity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        connection.port,
        connection.token,
        body.map_or(0, <[u8]>::len)
    );
    write(&mut stream, header.as_bytes(), deadline, cancelled)?;
    if let Some(body) = body {
        write(&mut stream, body, deadline, cancelled)?;
    }
    let mut reader =
        BufReader::new(Reader { stream: &mut stream, deadline, cancelled }).take(WIRE_LIMIT);
    let result = response_to(&mut reader, emit);
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    match result? {
        200 => Ok(()),
        401 | 403 => Err(Error::Unauthorized),
        404 => Err(Error::NotFound),
        500..=599 => Err(Error::ProviderUnavailable),
        _ => Err(Error::ProviderFailed),
    }
}
pub(super) fn complete(
    connection: &Connection,
    model: &str,
    messages: &[Message],
    streaming: bool,
    cancelled: &AtomicBool,
    emit: &mut impl FnMut(&str) -> Result<()>,
) -> Result<Completion> {
    let mut payload = serde_json::json!({"model":model,"stream":streaming,"messages":messages});
    if streaming {
        payload["stream_options"] = serde_json::json!({"include_usage":true});
    }
    let payload = serde_json::to_vec(&payload).map_err(|_| Error::InvalidRequest)?;
    if payload.len() > crate::api::MAX_REQUEST_BYTES {
        return Err(Error::Limit);
    }
    if streaming {
        let mut decoder = crate::provider::stream::Decoder::default();
        request(connection, Some(&payload), cancelled, &mut |bytes| decoder.feed(bytes, emit))?;
        decoder.finish(emit)
    } else {
        let mut bytes = Vec::new();
        request(connection, Some(&payload), cancelled, &mut |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })?;
        crate::provider::parse_json_completion(&bytes)
    }
}
pub(super) fn models(connection: &Connection, cancelled: &AtomicBool) -> Result<Vec<String>> {
    let mut bytes = Vec::new();
    request(connection, None, cancelled, &mut |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    crate::provider::parse_catalog(&bytes)
}
