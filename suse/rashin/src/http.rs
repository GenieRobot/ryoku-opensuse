//! Loopback browser/API boundary. The browser uses a private credential and
//! origin-scoped HTTP authentication; the CLI uses a bearer credential. A localhost
//! Origin or X-Rashin-Client header alone never grants access.
mod console;
use crate::{
    Error, Result,
    api::{Document, MAX_REQUEST_BYTES, Request, Response},
    authorization::Access,
    digest,
    runtime::Engine,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};

const HEADER_LIMIT: usize = 16 * 1024;
pub struct Gateway {
    engine: Arc<Engine>,
    token: String,
    origin: String,
    browser_authorization: String,
}

impl Gateway {
    pub fn new(engine: Arc<Engine>, token: String, origin: String) -> Result<Self> {
        if token.len() != 64
            || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !origin.starts_with("http://127.0.0.1:")
        {
            return Err(Error::InvalidRequest);
        }
        // Validate the full origin using the socket address parser, without DNS.
        let address = origin.strip_prefix("http://").ok_or(Error::InvalidRequest)?;
        let address: std::net::SocketAddr = address.parse().map_err(|_| Error::InvalidRequest)?;
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(Error::InvalidRequest);
        }
        let browser_authorization =
            format!("Basic {}", base64(format!("owner:{token}").as_bytes()));
        Ok(Self { engine, token, origin, browser_authorization })
    }

    pub fn serve(self, listener: TcpListener) -> Result<()> {
        if format!("http://{}", listener.local_addr().map_err(|_| Error::Storage)?) != self.origin {
            return Err(Error::InvalidRequest);
        }
        for stream in listener.incoming() {
            let mut stream = stream.map_err(|_| Error::Storage)?;
            let _ = self.connection(&mut stream);
        }
        Ok(())
    }

    pub fn connection(&self, stream: &mut TcpStream) -> Result<()> {
        if !stream.peer_addr().map_err(|_| Error::Unauthorized)?.ip().is_loopback() {
            return Err(Error::Unauthorized);
        }
        stream.set_read_timeout(Some(Duration::from_secs(3))).map_err(|_| Error::Storage)?;
        stream.set_write_timeout(Some(Duration::from_secs(3))).map_err(|_| Error::Storage)?;
        self.handle(stream)
    }

    /// Also used by byte-stream integration tests; production must call
    /// connection(), which checks the loopback peer and enforces I/O deadlines.
    pub fn handle(&self, stream: &mut (impl Read + Write)) -> Result<()> {
        let request = match read_request(stream) {
            Ok(request) => request,
            Err(_) => {
                return send(
                    stream,
                    400,
                    "application/json",
                    br#"{"status":"error","error":"invalid_request"}"#,
                    None,
                );
            }
        };
        if request.headers.get("host").map(String::as_str) != self.origin.strip_prefix("http://") {
            return denied(stream);
        }
        let authorization = request.headers.get("authorization");
        let explicit_bearer = authorization.is_some_and(|value| value.starts_with("Bearer "));
        let same_origin =
            request.headers.get("origin").is_some_and(|origin| origin == &self.origin);
        let wrong_origin =
            request.headers.get("origin").is_some_and(|origin| origin != &self.origin);
        if wrong_origin {
            return denied(stream);
        }
        // Cookies never grant authority: browser cookies cross local ports.
        // Transport assigns the role; JSON never selects it. Only the exact
        // canonical Basic credential is accepted, avoiding decoding ambiguity.
        let access = if let Some(header) = authorization {
            if let Some(token) = header.strip_prefix("Bearer ") {
                if equal(token, &self.token) {
                    Some(Access::Owner)
                } else if self.engine.authenticate_ask_client(token)? {
                    Some(Access::AskClient)
                } else {
                    None
                }
            } else if equal(header, &self.browser_authorization) {
                Some(Access::Owner)
            } else {
                None
            }
        } else {
            None
        };
        let owner = matches!(access, Some(Access::Owner));
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => send(stream, 200, "text/html; charset=utf-8", LOGIN.as_bytes(), None),
            ("GET", "/login") if owner => redirect(stream, "/console"),
            ("GET", "/login" | "/console") if access.is_none() && !explicit_bearer => {
                self.challenge(stream)
            }
            ("GET", path)
                if access.is_none()
                    && !explicit_bearer
                    && (path.starts_with("/session/")
                        || path.starts_with("/job/")
                        || path == "/prowl"
                        || path.starts_with("/prowl/")) =>
            {
                self.challenge(stream)
            }
            ("POST", "/api/action") => {
                let Some(access) = access.filter(|_| explicit_bearer || same_origin) else {
                    return denied(stream);
                };
                if !request
                    .headers
                    .get("content-type")
                    .is_some_and(|value| value.split(';').next() == Some("application/json"))
                {
                    return send(
                        stream,
                        415,
                        "application/json",
                        br#"{"status":"error","error":"invalid_request"}"#,
                        None,
                    );
                }
                let result = serde_json::from_slice::<Request>(&request.body)
                    .map_err(|_| Error::InvalidRequest)
                    .and_then(|request| self.engine.dispatch_as(access, request));
                let status = match &result {
                    Ok(_) => 200,
                    Err(Error::Unauthorized) => 403,
                    Err(_) => 400,
                };
                let bytes =
                    serde_json::to_vec(&Response::from(result)).map_err(|_| Error::Storage)?;
                send(stream, status, "application/json", &bytes, None)
            }
            ("GET", "/console") if owner => {
                send(stream, 200, "text/html; charset=utf-8", self.console()?.as_bytes(), None)
            }
            ("POST", "/ask") if owner && same_origin => {
                let fields = form(&request.body)?;
                let id = fields.get("id").ok_or(Error::InvalidRequest)?.to_owned();
                let prompt = fields.get("prompt").ok_or(Error::InvalidRequest)?.to_owned();
                let response = self.engine.dispatch(Request::Ask { id, prompt });
                send(
                    stream,
                    200,
                    "text/html; charset=utf-8",
                    page(
                        "Ask",
                        &format!(
                            "<pre>{}</pre><a href=\"/console\">Refresh conversations</a>",
                            escape(
                                &serde_json::to_string_pretty(&Response::from(response))
                                    .map_err(|_| Error::Storage)?
                            )
                        ),
                    )
                    .as_bytes(),
                    None,
                )
            }
            ("POST", "/index") if owner && same_origin => {
                let fields = form(&request.body)?;
                let name = fields.get("name").ok_or(Error::InvalidRequest)?.to_owned();
                let content = fields.get("content").ok_or(Error::InvalidRequest)?.to_owned();
                let sha256 = digest(content.as_bytes());
                let response = self.engine.dispatch(Request::Index {
                    documents: vec![Document { name, content, sha256 }],
                });
                send(
                    stream,
                    200,
                    "text/html; charset=utf-8",
                    page(
                        "Index",
                        &format!(
                            "<pre>{}</pre><a href=\"/console\">Back to Rashin</a>",
                            escape(
                                &serde_json::to_string_pretty(&Response::from(response))
                                    .map_err(|_| Error::Storage)?
                            )
                        ),
                    )
                    .as_bytes(),
                    None,
                )
            }
            ("GET", path) if owner && (path == "/prowl" || path.starts_with("/prowl/")) => {
                send_ui(stream, console::prowl(&self.engine, path))
            }
            ("GET", path) if owner && path.starts_with("/session/") => {
                send_ui(stream, console::session(&self.engine, &path[9..]))
            }
            ("GET", path) if owner && path.starts_with("/job/") => {
                send_ui(stream, console::job(&self.engine, &path[5..]))
            }
            ("POST", "/console/action") if owner && same_origin => {
                match console::submit(&self.engine, &request.body) {
                    Ok(path) => redirect(stream, &path),
                    Err(error) => send_ui(stream, Err(error)),
                }
            }
            ("POST", "/console/action" | "/ask" | "/index")
                if !owner && !explicit_bearer && same_origin =>
            {
                self.challenge(stream)
            }
            ("POST", "/console/action" | "/ask" | "/index" | "/login") => denied(stream),
            _ if access.is_none() || !owner => denied(stream),
            _ => send(stream, 404, "text/plain", b"Not found", None),
        }
    }

    fn challenge(&self, stream: &mut impl Write) -> Result<()> {
        send(
            stream,
            401,
            "text/html; charset=utf-8",
            LOGIN.as_bytes(),
            Some(&format!("Basic realm=\"Rashin {}\", charset=\"UTF-8\"", self.origin)),
        )
    }

    fn console(&self) -> Result<String> {
        console::home(&self.engine)
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
fn read_request(stream: &mut impl Read) -> Result<HttpRequest> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= HEADER_LIMIT {
            return Err(Error::Limit);
        }
        let mut byte = [0];
        stream.read_exact(&mut byte).map_err(|_| Error::InvalidRequest)?;
        bytes.push(byte[0]);
    }
    let header = std::str::from_utf8(&bytes).map_err(|_| Error::InvalidRequest)?;
    let mut lines = header.split("\r\n");
    let first: Vec<_> = lines.next().ok_or(Error::InvalidRequest)?.split(' ').collect();
    if first.len() != 3
        || first[2] != "HTTP/1.1"
        || !["GET", "POST"].contains(&first[0])
        || !first[1].starts_with('/')
        || first[1].contains(['\r', '\n'])
    {
        return Err(Error::InvalidRequest);
    }
    let mut headers = BTreeMap::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(Error::InvalidRequest)?;
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b.is_ascii_control())
        {
            return Err(Error::InvalidRequest);
        }
        if headers.insert(name.to_ascii_lowercase(), value.trim().to_owned()).is_some() {
            return Err(Error::InvalidRequest);
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(Error::InvalidRequest);
    }
    let length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| Error::InvalidRequest)?
        .unwrap_or(0);
    if length > MAX_REQUEST_BYTES || (first[0] == "POST" && !headers.contains_key("content-length"))
    {
        return Err(Error::Limit);
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).map_err(|_| Error::InvalidRequest)?;
    Ok(HttpRequest { method: first[0].into(), path: first[1].into(), headers, body })
}

fn send(
    stream: &mut impl Write,
    status: u16,
    content_type: &str,
    body: &[u8],
    challenge: Option<&str>,
) -> Result<()> {
    let challenge =
        challenge.map(|value| format!("WWW-Authenticate: {value}\r\n")).unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'\r\n{challenge}\r\n",
        if status == 200 { "OK" } else { "Error" },
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|_| Error::Storage)
}
fn send_ui(stream: &mut impl Write, result: Result<String>) -> Result<()> {
    match result {
        Ok(html) => send(stream, 200, "text/html; charset=utf-8", html.as_bytes(), None),
        Err(error) => send(
            stream,
            400,
            "text/html; charset=utf-8",
            page(
                "Rashin action",
                &format!(
                    "<p>{}</p><a href=\"/console\">Back to workspace</a>",
                    escape(&error.to_string())
                ),
            )
            .as_bytes(),
            None,
        ),
    }
}
fn redirect(stream: &mut impl Write, path: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(Error::InvalidRequest);
    }
    write!(stream,"HTTP/1.1 303 See Other\r\nLocation: {path}\r\nContent-Length: 0\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'\r\n\r\n").map_err(|_|Error::Storage)
}
fn denied(stream: &mut impl Write) -> Result<()> {
    send(stream, 403, "application/json", br#"{"status":"error","error":"unauthorized"}"#, None)
}
fn equal(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left.bytes().zip(right.bytes()).fold(0u8, |difference, (a, b)| difference | (a ^ b)) == 0
}
fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><main><h1>{title}</h1>{body}</main></html>"
    )
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn form(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let raw = std::str::from_utf8(bytes).map_err(|_| Error::InvalidRequest)?;
    let mut fields = BTreeMap::new();
    for field in raw.split('&') {
        let (key, value) = field.split_once('=').ok_or(Error::InvalidRequest)?;
        if fields.insert(decode(key)?, decode(value)?).is_some() {
            return Err(Error::InvalidRequest);
        }
    }
    Ok(fields)
}
fn decode(raw: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut source = raw.bytes();
    while let Some(byte) = source.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let a = source
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or(Error::InvalidRequest)?;
                let b = source
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or(Error::InvalidRequest)?;
                bytes.push((a * 16 + b) as u8);
            }
            byte => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).map_err(|_| Error::InvalidRequest)
}
// Encoding only: authentication compares the complete canonical header in
// constant time. No permissive decoder, alternate username, or cookie fallback.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            encoded.push(if index > chunk.len() {
                '='
            } else {
                ALPHABET[((word >> (18 - index * 6)) & 63) as usize] as char
            });
        }
    }
    encoded
}
const LOGIN: &str = "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Rashin sign in</title><main><h1>Rashin</h1><p>Open the workspace and use username <strong>owner</strong> with the value of browser.token in your private Rashin state directory as the password. Your browser authenticates only to this origin. Close the browser session to clear its cached authentication.</p><a href=\"/console\">Open private workspace</a></main></html>";

#[cfg(test)]
mod tests {
    #[test]
    fn canonical_basic_encoding_matches_standard_vectors() {
        for (raw, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            (
                "owner:1111111111111111111111111111111111111111111111111111111111111111",
                "b3duZXI6MTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMQ==",
            ),
        ] {
            assert_eq!(super::base64(raw.as_bytes()), expected);
        }
    }
}
