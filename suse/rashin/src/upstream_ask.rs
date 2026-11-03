//! Narrow Unix transport for the pinned launcher's Ask protocol.
use crate::{Error, Result, api::Request, runtime::Engine};
use serde_json::json;
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const LIMIT: usize = 16;
struct Lease(Arc<AtomicUsize>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn serve(listener: UnixListener, engine: Arc<Engine>) -> Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    let current = Arc::new(Mutex::new(None::<String>));
    for stream in listener.incoming() {
        let stream = stream.map_err(|_| Error::Storage)?;
        if active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < LIMIT).then_some(n + 1))
            .is_err()
        {
            continue;
        }
        let count = Arc::clone(&active);
        let engine = Arc::clone(&engine);
        let current = Arc::clone(&current);
        if std::thread::Builder::new()
            .name("rashin-upstream-ask".into())
            .spawn(move || {
                let _lease = Lease(count);
                let _ = connection(stream, &engine, &current);
            })
            .is_err()
        {
            active.fetch_sub(1, Ordering::AcqRel);
        }
    }
    Ok(())
}

fn connection(
    mut stream: UnixStream,
    engine: &Arc<Engine>,
    current: &Mutex<Option<String>>,
) -> Result<()> {
    let peer = rustix::net::sockopt::socket_peercred(&stream).map_err(|_| Error::Unauthorized)?;
    if peer.uid != rustix::process::geteuid() {
        return Err(Error::Unauthorized);
    }
    stream.set_read_timeout(Some(Duration::from_secs(3))).map_err(|_| Error::Storage)?;
    stream.set_write_timeout(Some(Duration::from_secs(3))).map_err(|_| Error::Storage)?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() == 16 * 1024 {
            return Err(Error::Limit);
        }
        let mut byte = [0];
        stream.read_exact(&mut byte).map_err(|_| Error::InvalidRequest)?;
        head.push(byte[0]);
    }
    let head = std::str::from_utf8(&head).map_err(|_| Error::InvalidRequest)?;
    let mut lines = head.split("\r\n");
    let first: Vec<_> = lines.next().ok_or(Error::InvalidRequest)?.split(' ').collect();
    if first.len() != 3 || first[2] != "HTTP/1.1" {
        return Err(Error::InvalidRequest);
    }
    let mut content_length = None;
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(Error::InvalidRequest)?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(Error::InvalidRequest);
        }
        if name.eq_ignore_ascii_case("content-length")
            && content_length.replace(value.trim()).is_some()
        {
            return Err(Error::InvalidRequest);
        }
    }
    if content_length.is_some_and(|value| value != "0") {
        return Err(Error::InvalidRequest);
    }
    match (first[0], first[1]) {
        ("GET", "/api/ping") => response(&mut stream, b"ok", "text/plain"),
        ("GET", "/api/ask/recent") => {
            let mut records = engine.dispatch(Request::Recent {})?["asks"]
                .as_array()
                .ok_or(Error::Integrity)?
                .clone();
            records.retain(|r| r["job"]["status"] == "completed");
            records.sort_by(|a, b| {
                b["job"]["started"]
                    .as_u64()
                    .cmp(&a["job"]["started"].as_u64())
                    .then_with(|| {
                        b["job"]["started_nanos"].as_u64().cmp(&a["job"]["started_nanos"].as_u64())
                    })
                    .then_with(|| b["id"].as_str().cmp(&a["id"].as_str()))
            });
            let records: Vec<_> = records.iter().take(12).map(|r| {
                let job = &r["job"];
                let at = job["started"].as_i64().and_then(|n| jiff::Timestamp::from_second(n).ok()).map(|t| t.to_string()).unwrap_or_default();
                json!({"id":r["id"],"at":at,"q":job["prompt"],"a":job["answer"],"via":job["model"],"needsAgent":job["needs_agent"],"needsAgentReason":job["needs_agent_reason"]})
            }).collect();
            response(
                &mut stream,
                &serde_json::to_vec(&records).map_err(|_| Error::Storage)?,
                "application/json",
            )
        }
        ("POST", "/api/ask/cancel") => {
            let id = current.lock().map_err(|_| Error::Storage)?.clone();
            if let Some(id) = id {
                let _ = engine.dispatch(Request::Cancel { id });
            }
            response(&mut stream, b"", "text/plain")
        }
        ("POST", path) if path.starts_with("/api/ask?") => {
            let mut fields = url::form_urlencoded::parse(&path.as_bytes()[9..]);
            let (key, question) = fields.next().ok_or(Error::InvalidRequest)?;
            if key != "q" || fields.next().is_some() {
                return Err(Error::InvalidRequest);
            }
            let mut random = [0u8; 16];
            std::fs::File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut random))
                .map_err(|_| Error::Storage)?;
            let id = format!("a{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>());
            {
                let mut slot = current.lock().map_err(|_| Error::Storage)?;
                if slot.is_some() {
                    return Err(Error::Busy);
                }
                engine.dispatch(Request::Ask { id: id.clone(), prompt: question.into_owned() })?;
                *slot = Some(id.clone());
            }
            let result = stream_ask(&mut stream, engine, &id);
            if result.is_err() {
                let _ = engine.dispatch(Request::Cancel { id: id.clone() });
            }
            let mut slot = current.lock().map_err(|_| Error::Storage)?;
            if slot.as_deref() == Some(&id) {
                *slot = None;
            }
            result
        }
        _ => Err(Error::Unauthorized),
    }
}
fn response(stream: &mut UnixStream, body: &[u8], kind: &str) -> Result<()> {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n", body.len()).and_then(|_| stream.write_all(body)).map_err(|_| Error::Storage)
}
fn stream_ask(stream: &mut UnixStream, engine: &Arc<Engine>, id: &str) -> Result<()> {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n@working thinking\n").map_err(|_| Error::Storage)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut last_heartbeat = Instant::now();
    let mut writing = false;
    loop {
        let result = engine.dispatch(Request::Job { id: id.into() })?;
        let job = &result["job"];
        match job["status"].as_str() {
            Some("completed") => {
                let answer = json!({"id":id,"text":job["answer"],"images":[],"actions":[],"via":job["model"],"needsAgent":job["needs_agent"],"needsAgentReason":job["needs_agent_reason"]});
                writeln!(
                    stream,
                    "@answer {}",
                    serde_json::to_string(&answer).map_err(|_| Error::Storage)?
                )
                .map_err(|_| Error::Storage)?;
                return Ok(());
            }
            Some("running") if Instant::now() < deadline => (),
            _ => {
                let _ = engine.dispatch(Request::Cancel { id: id.into() });
                writeln!(stream, "@error ask failed or cancelled").map_err(|_| Error::Storage)?;
                return Ok(());
            }
        }
        if !writing && result["progress"].as_str().is_some_and(|s| !s.is_empty()) {
            writeln!(stream, "@working writing").map_err(|_| Error::Storage)?;
            writing = true;
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(1) {
            writeln!(stream, "@working thinking").map_err(|_| Error::Storage)?;
            last_heartbeat = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}
