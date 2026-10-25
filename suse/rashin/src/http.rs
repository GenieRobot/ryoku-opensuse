//! Loopback browser/API boundary. The browser uses a private credential and
//! same-origin, HttpOnly cookies; the CLI uses a bearer credential. A localhost
//! Origin or X-Rashin-Client header alone never grants access.
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
        Ok(Self { engine, token, origin })
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
        let explicit_bearer = request.headers.get("authorization");
        let cookie = request.headers.get("cookie").is_some_and(|cookies| {
            cookies
                .split(';')
                .filter_map(|cookie| cookie.trim().split_once('='))
                .any(|(name, token)| name == "rashin" && equal(token, &self.token))
        });
        let same_origin =
            request.headers.get("origin").is_some_and(|origin| origin == &self.origin);
        let wrong_origin =
            request.headers.get("origin").is_some_and(|origin| origin != &self.origin);
        if wrong_origin {
            return denied(stream);
        }
        // An explicit restricted/invalid bearer cannot inherit a stronger
        // browser cookie. Transport assigns the role; JSON never selects it.
        let access = if let Some(header) = explicit_bearer {
            if let Some(token) = header.strip_prefix("Bearer ") {
                if equal(token, &self.token) {
                    Some(Access::Owner)
                } else if self.engine.authenticate_ask_client(token)? {
                    Some(Access::AskClient)
                } else {
                    None
                }
            } else {
                None
            }
        } else if cookie {
            Some(Access::Owner)
        } else {
            None
        };
        let owner = matches!(access, Some(Access::Owner));
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => send(stream, 200, "text/html; charset=utf-8", LOGIN.as_bytes(), None),
            ("POST", "/login") => {
                if !same_origin {
                    return denied(stream);
                }
                let fields = form(&request.body)?;
                if !fields.get("token").is_some_and(|token| equal(token, &self.token)) {
                    return denied(stream);
                }
                send(
                    stream,
                    200,
                    "text/html; charset=utf-8",
                    self.console()?.as_bytes(),
                    Some(&format!("rashin={}; HttpOnly; SameSite=Strict; Path=/", self.token)),
                )
            }
            ("POST", "/api/action") => {
                let Some(access) = access.filter(|_| explicit_bearer.is_some() || same_origin)
                else {
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
            ("POST", "/ask") if owner && cookie && same_origin => {
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
            ("POST", "/index") if owner && cookie && same_origin => {
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
            _ if access.is_none() || !owner => denied(stream),
            _ => send(stream, 404, "text/plain", b"Not found", None),
        }
    }

    fn console(&self) -> Result<String> {
        let recent = self.engine.dispatch(Request::Recent {})?;
        let usage = self.engine.dispatch(Request::Usage {})?;
        let status = self.engine.dispatch(Request::Status {})?;
        Ok(page(
            "Rashin",
            &format!(
                "<p>Private local assistant console. Provider configuration and Agent/Chat are also available through the typed API.</p><h2>Ask</h2><form method=\"post\" action=\"/ask\"><label>Unique request ID <input name=\"id\" required maxlength=\"64\"></label><label>Question <textarea name=\"prompt\" required maxlength=\"16384\"></textarea></label><button>Ask</button></form><h2>Index local evidence</h2><p>This replaces the current document index. Uploaded text stays in your private account state.</p><form method=\"post\" action=\"/index\"><label>Document name <input name=\"name\" required maxlength=\"128\"></label><label>Content <textarea name=\"content\" required maxlength=\"32768\"></textarea></label><button>Index</button></form><h2>Recent answers</h2><a href=\"/console\">Refresh</a><pre>{}</pre><h2>Usage</h2><pre>{}</pre><h2>Status</h2><pre>{}</pre>",
                escape(&serde_json::to_string_pretty(&recent).map_err(|_| Error::Storage)?),
                escape(&serde_json::to_string_pretty(&usage).map_err(|_| Error::Storage)?),
                escape(&serde_json::to_string_pretty(&status).map_err(|_| Error::Storage)?)
            ),
        ))
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
    cookie: Option<&str>,
) -> Result<()> {
    let cookie = cookie.map(|cookie| format!("Set-Cookie: {cookie}\r\n")).unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'\r\n{cookie}\r\n",
        if status == 200 { "OK" } else { "Error" },
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|_| Error::Storage)
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
const LOGIN: &str = "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Rashin sign in</title><main><h1>Rashin</h1><p>Read browser.token in your private Rashin state directory and enter its value to connect.</p><form method=\"post\" action=\"/login\"><label>Private browser token <input name=\"token\" type=\"password\" required autocomplete=\"off\" maxlength=\"64\"></label><button>Connect</button></form></main></html>";
