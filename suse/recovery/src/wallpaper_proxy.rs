//! Per-frame public Ryogami proxy. Native descriptors stay inside Rust.
use crate::{
    wallpaper_channel::NativeActor,
    wallpaper_protocol::{self as wire, PublicRequest, WallpaperChange},
};
use rustix::net::sockopt::socket_peercred;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const FRAME_TIME: Duration = Duration::from_secs(5);
const MAX_CLIENTS: usize = 16;
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(0);

pub(crate) fn run_client(arguments: &[std::ffi::OsString]) -> io::Result<i32> {
    let arguments = arguments
        .iter()
        .map(|value| {
            value
                .to_str()
                .filter(|value| !value.chars().any(char::is_control))
                .ok_or_else(|| io::Error::other("wallpaper arguments must be bounded UTF-8"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let mut frame = arguments.join(" ").into_bytes();
    frame.push(b'\n');
    wire::bounded_line(&frame, wire::MAX_REQUEST_BYTES).map_err(io::Error::other)?;
    let uid = rustix::process::geteuid().as_raw();
    let mut stream = UnixStream::connect(format!("/run/user/{uid}/ryogami.sock"))?;
    if socket_peercred(&stream)?.uid.as_raw() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unexpected wallpaper broker account",
        ));
    }
    stream.set_write_timeout(Some(FRAME_TIME))?;
    stream.write_all(&frame)?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    let mut reader = BufReader::new(stream);
    let mut reply = Vec::new();
    reader.by_ref().take(wire::MAX_ACTOR_REPLY_BYTES as u64 + 1).read_until(b'\n', &mut reply)?;
    wire::bounded_line(&reply, wire::MAX_ACTOR_REPLY_BYTES).map_err(io::Error::other)?;
    let failed = reply.starts_with(b"err ")
        || serde_json::from_slice::<serde_json::Value>(&reply)
            .is_ok_and(|value| value.get("error").is_some());
    if reply != b"ok\n" {
        if failed {
            std::io::stderr().write_all(&reply)?;
        } else {
            std::io::stdout().write_all(&reply)?;
        }
    }
    Ok(i32::from(failed))
}

pub(crate) struct ProxyServer {
    actor: Arc<NativeActor>,
    closed: Arc<AtomicBool>,
    owner: Option<std::thread::JoinHandle<io::Result<()>>>,
}

impl ProxyServer {
    pub(crate) fn start(actor: Arc<NativeActor>, uid: u32, gid: u32) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let root = std::fs::File::open("/run/user")?;
        let runtime = std::fs::File::from(rustix::fs::openat2(
            &root,
            uid.to_string(),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )?);
        let metadata = runtime.metadata()?;
        if metadata.uid() != uid || metadata.gid() != gid || metadata.mode() & 0o7777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid fixed wallpaper runtime directory",
            ));
        }
        let path = format!("/proc/self/fd/{}/ryogami.sock", runtime.as_raw_fd());
        // Never steal a named endpoint or unlink an entry selected by a peer.
        // An interrupted broker's stale endpoint needs explicit reconciliation.
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let socket = std::fs::symlink_metadata(&path)?;
        listener.set_nonblocking(true)?;
        let closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&closed);
        let native = Arc::clone(&actor);
        let active = Arc::new(AtomicUsize::new(0));
        let owner = std::thread::Builder::new().name("ryogami-proxy".into()).spawn(move || {
            let result = (|| {
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((client, _)) => {
                            if active
                                .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                                    (count < MAX_CLIENTS).then_some(count + 1)
                                })
                                .is_err()
                            {
                                continue;
                            }
                            let native = Arc::clone(&native);
                            let active = Arc::clone(&active);
                            std::thread::Builder::new().name("ryogami-client".into()).spawn(
                                move || {
                                    struct ClientCount(Arc<AtomicUsize>);
                                    impl Drop for ClientCount {
                                        fn drop(&mut self) {
                                            self.0.fetch_sub(1, Ordering::AcqRel);
                                        }
                                    }
                                    let _count = ClientCount(active);
                                    let _ =
                                        relay_connection(client, uid, &native, apply_via_helper);
                                },
                            )?;
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            })();
            if let Ok(current) = std::fs::symlink_metadata(&path)
                && (current.dev(), current.ino()) == (socket.dev(), socket.ino())
            {
                let _ =
                    rustix::fs::unlinkat(&runtime, "ryogami.sock", rustix::fs::AtFlags::empty());
            }
            result
        })?;
        Ok(Self { actor, closed, owner: Some(owner) })
    }
}

impl Drop for ProxyServer {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.actor.shutdown();
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

fn apply_via_helper(id: i64, action: &WallpaperChange) -> io::Result<Vec<u8>> {
    action.validate().map_err(io::Error::other)?;
    let mut stream = UnixStream::connect("/run/ryoku/helper.sock")?;
    if socket_peercred(&stream)?.uid.as_raw() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wallpaper helper peer is not root",
        ));
    }
    let request_id = format!(
        "wallpaper-{}-{}",
        std::process::id(),
        NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
    );
    let (operation, expected_outcome, params) = match action {
        WallpaperChange::Apply(action) => {
            ("apply_wallpaper", "wallpaper_applied", serde_json::to_value(action))
        }
        WallpaperChange::Audio(action) => {
            ("set_wallpaper_audio", "wallpaper_audio_set", serde_json::to_value(action))
        }
    };
    let params = params.map_err(io::Error::other)?;
    let request = serde_json::json!({"version":1,"request_id":request_id,"request":{"operation":operation,"action":params,"rpc_id":id}});
    let mut bytes = serde_json::to_vec(&request).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > wire::MAX_REQUEST_BYTES {
        return Err(io::Error::other("wallpaper helper request exceeds bound"));
    }
    stream.set_write_timeout(Some(FRAME_TIME))?;
    stream.write_all(&bytes)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut reader = BufReader::new(stream);
    let mut reply = Vec::new();
    loop {
        reader.get_ref().set_read_timeout(Some(
            deadline.checked_duration_since(Instant::now()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "wallpaper helper deadline exceeded")
            })?,
        ))?;
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete wallpaper helper response",
            ));
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(bytes.len(), |index| index + 1);
        if reply.len().saturating_add(count) > wire::MAX_ACTOR_REPLY_BYTES * 4 + 4096 {
            return Err(io::Error::other("wallpaper helper response exceeds bound"));
        }
        reply.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    let response: serde_json::Value = serde_json::from_slice(&reply).map_err(io::Error::other)?;
    if response.get("version").and_then(serde_json::Value::as_u64) != Some(1)
        || response.get("request_id").and_then(serde_json::Value::as_str)
            != Some(request_id.as_str())
    {
        return Err(io::Error::other("wallpaper helper response identity mismatch"));
    }
    let outcome = response
        .get("outcome")
        .ok_or_else(|| io::Error::other("missing wallpaper helper outcome"))?;
    if outcome.get("result").and_then(serde_json::Value::as_str) != Some(expected_outcome) {
        let kind = match outcome
            .get("error")
            .and_then(|error| error.get("detail"))
            .and_then(serde_json::Value::as_str)
        {
            Some("wallpaper_state_capacity") => Some(io::ErrorKind::StorageFull),
            Some("wallpaper_source_capacity") => Some(io::ErrorKind::FileTooLarge),
            Some("wallpaper_source_unavailable") => Some(io::ErrorKind::InvalidInput),
            Some("wallpaper_state_location" | "wallpaper_actor_layout_changed") => {
                Some(io::ErrorKind::Unsupported)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            return Err(io::Error::new(kind, "wallpaper input preflight refused"));
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wallpaper helper refused action",
        ));
    }
    let reply: Vec<u8> = serde_json::from_value(
        outcome
            .get("reply")
            .cloned()
            .ok_or_else(|| io::Error::other("missing native wallpaper reply"))?,
    )
    .map_err(io::Error::other)?;
    validate_reply(&reply, id)?;
    Ok(reply)
}

/// Only the root helper result can supply a mutating response. In particular,
/// no public bytes, channel label or client plan hash construct an actor call.
pub(crate) fn relay_connection(
    client: UnixStream,
    uid: u32,
    native: &NativeActor,
    mut apply: impl FnMut(i64, &WallpaperChange) -> io::Result<Vec<u8>>,
) -> io::Result<()> {
    let peer = socket_peercred(&client)?;
    if uid == 0 || peer.uid.as_raw() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wallpaper peer is not the session account",
        ));
    }
    let native = native.query_connection()?;
    relay_authenticated(client, native, &mut apply)
}

fn relay_authenticated(
    client: UnixStream,
    mut native: UnixStream,
    apply: &mut impl FnMut(i64, &WallpaperChange) -> io::Result<Vec<u8>>,
) -> io::Result<()> {
    let output = Mutex::new(client.try_clone()?);
    let closed = AtomicBool::new(false);
    let native_reader = native.try_clone()?;
    std::thread::scope(|scope| {
        let receiver = scope.spawn(|| {
            let result = relay_native(native_reader, &output, &closed);
            closed.store(true, Ordering::Release);
            let _ = output.lock().map(|stream| stream.shutdown(std::net::Shutdown::Both));
            result
        });
        let result = (|| {
            let mut client = BufReader::new(client);
            loop {
                let Some(frame) = read_frame(&mut client, wire::MAX_REQUEST_BYTES, &closed)? else {
                    return Ok(());
                };
                match wire::parse_public(&frame) {
                    Ok(PublicRequest::Query { frame, .. }) => {
                        // Every later frame on the same subscription/query
                        // connection is classified again before native sees it.
                        native.set_write_timeout(Some(FRAME_TIME))?;
                        native.write_all(&frame)?;
                    }
                    Ok(PublicRequest::Apply { id, action, plain }) => {
                        let reply = mutation_reply(id, apply(id, &WallpaperChange::Apply(action)))?;
                        let reply = if plain {
                            let value: serde_json::Value =
                                serde_json::from_slice(&reply).map_err(io::Error::other)?;
                            if value.get("error").is_some() {
                                b"err wallpaper action failed; inspect the linked operation history\n".to_vec()
                            } else {
                                b"ok\n".to_vec()
                            }
                        } else {
                            reply
                        };
                        write_public(&output, &reply)?;
                    }
                    Ok(PublicRequest::Audio { id, action }) => {
                        let reply = mutation_reply(id, apply(id, &WallpaperChange::Audio(action)))?;
                        write_public(&output, &reply)?;
                    }
                    Ok(PublicRequest::Handoff { id, .. }) => {
                        write_public(
                            &output,
                            &error_reply(
                                id,
                                "automatic wallpaper action is retained and awaits issue176 constrained authorization and complete recovery envelope",
                            ),
                        )?;
                    }
                    Err(error) => {
                        // Correlation is display-only and confers no authority.
                        let id = serde_json::from_slice::<serde_json::Value>(&frame)
                            .ok()
                            .and_then(|value| value.get("id").and_then(serde_json::Value::as_i64))
                            .filter(|id| *id >= 0)
                            .unwrap_or(0);
                        let message = match error {
                            wire::WallpaperProtocolError::UnownedAction => {
                                "this retained Ryogami action needs its typed authorization and complete recovery write envelope"
                            }
                            _ => "invalid bounded wallpaper request",
                        };
                        write_public(&output, &error_reply(id, message))?;
                    }
                }
            }
        })();
        closed.store(true, Ordering::Release);
        let _ = native.shutdown(std::net::Shutdown::Both);
        let received =
            receiver.join().map_err(|_| io::Error::other("native query reader failed"))?;
        result.and(received)
    })
}

fn relay_native(
    native: UnixStream,
    output: &Mutex<UnixStream>,
    closed: &AtomicBool,
) -> io::Result<()> {
    let mut native = BufReader::new(native);
    while let Some(frame) = read_frame(&mut native, wire::MAX_QUERY_REPLY_BYTES, closed)? {
        if frame == b"ok\n" || frame.starts_with(b"err ") {
            write_public(output, &frame)?;
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&frame).map_err(io::Error::other)?;
        // Plain wallpaper-topic subscribers receive the original frame object
        // without an id/event wrapper. JSON RPC replies/events keep theirs.
        value.as_object().ok_or_else(|| io::Error::other("native query reply is not an object"))?;
        write_public(output, &frame)?;
    }
    Ok(())
}

fn write_public(output: &Mutex<UnixStream>, frame: &[u8]) -> io::Result<()> {
    let mut output = output.lock().map_err(|_| io::Error::other("wallpaper output poisoned"))?;
    output.set_write_timeout(Some(FRAME_TIME))?;
    output.write_all(frame)
}

/// Idle subscriptions are allowed; a started frame has one absolute deadline.
/// Buffered framing keeps later requests available for separate classification,
/// including query-then-write reuse without exposing a native descriptor.
fn read_frame(
    stream: &mut BufReader<UnixStream>,
    maximum: usize,
    closed: &AtomicBool,
) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    let mut deadline = None;
    loop {
        if closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "wallpaper frame deadline exceeded",
            ));
        }
        stream.get_ref().set_read_timeout(Some(Duration::from_millis(100)))?;
        match stream.fill_buf() {
            Ok([]) if frame.is_empty() => return Ok(None),
            Ok([]) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete wallpaper frame",
                ));
            }
            Ok(bytes) => {
                deadline.get_or_insert_with(|| Instant::now() + FRAME_TIME);
                let newline = bytes.iter().position(|byte| *byte == b'\n');
                let count = newline.map_or(bytes.len(), |index| index + 1);
                frame.extend_from_slice(&bytes[..count]);
                stream.consume(count);
                if frame.len() > maximum {
                    return Err(io::Error::other("wallpaper frame exceeds bound"));
                }
                if newline.is_some() {
                    return Ok(Some(frame));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
}

fn mutation_reply(id: i64, result: io::Result<Vec<u8>>) -> io::Result<Vec<u8>> {
    Ok(match result {
        Ok(reply) => {
            validate_reply(&reply, id)?;
            reply
        }
        Err(error) if error.kind() == io::ErrorKind::StorageFull => error_reply(
            id,
            "wallpaper recovery state exceeds the verified 16 MiB per-file capacity; increase verified recovery capacity before applying changes",
        ),
        Err(error) if error.kind() == io::ErrorKind::FileTooLarge => error_reply(
            id,
            "wallpaper input exceeds the verified bound; choose a static file at most 64 MiB or a video at most 8 GiB",
        ),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => error_reply(
            id,
            "choose a readable regular wallpaper owned by your account or a trusted installed file under /usr/share; links and shared writable files cannot be applied",
        ),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => error_reply(
            id,
            "wallpaper cache settings differ from restore-point coverage; restore an enrolled cache setting and restart the wallpaper service before retrying. Custom locations need their declared-state recovery provider",
        ),
        Err(_) => error_reply(
            id,
            "wallpaper authorization or linked checkpoint is unavailable; review operator membership and protected history, and reinstall the reviewed profile if its bundle or snapshot is missing",
        ),
    })
}

fn validate_reply(reply: &[u8], id: i64) -> io::Result<()> {
    wire::bounded_line(reply, wire::MAX_ACTOR_REPLY_BYTES).map_err(io::Error::other)?;
    let value: serde_json::Value = serde_json::from_slice(reply).map_err(io::Error::other)?;
    if value.get("id").and_then(serde_json::Value::as_i64) != Some(id)
        || value.get("result").is_some() == value.get("error").is_some()
    {
        return Err(io::Error::other("helper wallpaper response does not match request"));
    }
    Ok(())
}

fn error_reply(id: i64, message: &str) -> Vec<u8> {
    let mut reply =
        serde_json::to_vec(&serde_json::json!({"id":id,"error":{"code":-32001,"message":message}}))
            .expect("fixed error response serializes");
    reply.push(b'\n');
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallpaper_protocol::WallpaperApply;
    use std::{collections::BTreeMap, io::Read, sync::atomic::AtomicUsize};

    #[test]
    fn direct_audio_frames_require_the_typed_helper_and_never_reach_query_actor() {
        let (mut client, proxy) = UnixStream::pair().unwrap();
        let (native, mut backend) = UnixStream::pair().unwrap();
        std::thread::scope(|scope| {
            let worker=scope.spawn(|| relay_authenticated(proxy,native,&mut |id, change| {
                let WallpaperChange::Audio(action)=change else { panic!("wrong action") };
                assert!(action.outputs.is_empty()); assert_eq!(action.volume,Some(25));
                if id==2 { return Err(io::Error::new(io::ErrorKind::PermissionDenied,"denied")); }
                Ok(format!("{{\"id\":{id},\"result\":{{\"audio_written\":[{{\"state\":\"outputs\",\"key\":\"*\"}}]}}}}\n").into_bytes())
            }));
            let mut reader = BufReader::new(client.try_clone().unwrap());
            for id in [1, 2] {
                let action = wire::WallpaperAudio {
                    outputs: vec!["*".into()],
                    mute: None,
                    volume: Some(25),
                };
                client.write_all(&action.frame(id).unwrap()).unwrap();
                let mut reply = String::new();
                reader.read_line(&mut reply).unwrap();
                let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
                assert_eq!(value["id"], id);
                assert_eq!(value.get("result").is_some(), id == 1);
            }
            backend.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            assert!(backend.read(&mut [0]).is_err());
            client.shutdown(std::net::Shutdown::Both).unwrap();
            backend.shutdown(std::net::Shutdown::Both).unwrap();
            worker.join().unwrap().unwrap();
        });
    }

    #[test]
    fn direct_socket_layout_refusal_is_actionable_and_never_reaches_native_query_channel() {
        let (mut client, proxy) = UnixStream::pair().unwrap();
        let (native, mut backend) = UnixStream::pair().unwrap();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                relay_authenticated(proxy, native, &mut |_, _| {
                    Err(io::Error::new(io::ErrorKind::Unsupported, "layout preflight refused"))
                })
            });
            let action = WallpaperApply {
                kind: wire::WallpaperKind::Static,
                path: "/home/fixture/a.png".into(),
                outputs: vec![],
                mute: BTreeMap::new(),
                volume: BTreeMap::new(),
            };
            client.write_all(&action.frame(9).unwrap()).unwrap();
            let mut reply = String::new();
            BufReader::new(client.try_clone().unwrap()).read_line(&mut reply).unwrap();
            assert!(
                reply.contains("restart")
                    && reply.contains("cache")
                    && reply.contains("recovery provider")
            );
            assert_eq!(serde_json::from_str::<serde_json::Value>(&reply).unwrap()["id"], 9);
            backend.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            assert!(backend.read(&mut [0]).is_err());
            client.shutdown(std::net::Shutdown::Both).unwrap();
            backend.shutdown(std::net::Shutdown::Both).unwrap();
            worker.join().unwrap().unwrap();
        });
    }

    #[test]
    fn query_apply_and_theme_handoff_use_distinct_typed_boundaries() {
        let (mut client, proxy) = UnixStream::pair().unwrap();
        let (native, mut backend) = UnixStream::pair().unwrap();
        let calls = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                relay_authenticated(proxy, native, &mut |id, change| {
                    let WallpaperChange::Apply(action) = change else {
                        panic!("unexpected audio fixture call")
                    };
                    assert!(matches!(
                        action.kind,
                        wire::WallpaperKind::Static | wire::WallpaperKind::Video
                    ));
                    assert_eq!(action.outputs, ["DP-1"]);
                    calls.fetch_add(1, Ordering::Relaxed);
                    Ok(format!("{{\"id\":{id},\"result\":{{\"applied\":true}}}}\n").into_bytes())
                })
            });
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let query = b"{\"id\":1,\"method\":\"wall.outputs\",\"params\":{}}\n";
            client.write_all(query).unwrap();
            let mut received = vec![0; query.len()];
            backend.read_exact(&mut received).unwrap();
            assert_eq!(received, query);
            backend.write_all(b"{\"id\":1,\"result\":{\"outputs\":[\"DP-1\"]}}\n").unwrap();
            let mut reply = String::new();
            reader.read_line(&mut reply).unwrap();
            assert!(reply.contains("DP-1"));
            for (id, kind) in [(2, wire::WallpaperKind::Static), (3, wire::WallpaperKind::Video)] {
                let action = WallpaperApply {
                    kind,
                    path: "/home/fixture/Pictures/wall".into(),
                    outputs: vec!["DP-1".into()],
                    mute: BTreeMap::new(),
                    volume: BTreeMap::new(),
                };
                client.write_all(&action.frame(id).unwrap()).unwrap();
                reply.clear();
                reader.read_line(&mut reply).unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&reply).unwrap(),
                    serde_json::json!({"id":id,"result":{"applied":true}})
                );
            }
            // The shell's retained scheduleTheme route is a separate typed
            // handoff. The apply approval never authorizes its write set.
            client.write_all(b"{\"id\":4,\"method\":\"broker.automatic\",\"params\":{\"action\":\"shell.wallpaper.theme\"}}\n").unwrap();
            reply.clear();
            reader.read_line(&mut reply).unwrap();
            assert!(reply.contains("issue176"));
            assert_eq!(calls.load(Ordering::Relaxed), 2);
            backend.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            assert!(backend.read(&mut [0]).is_err(), "mutator or theme frame reached query actor");
            client.shutdown(std::net::Shutdown::Both).unwrap();
            backend.shutdown(std::net::Shutdown::Both).unwrap();
            worker.join().unwrap().unwrap();
        });
    }

    #[test]
    fn direct_socket_query_then_write_reuse_never_reaches_native_without_helper() {
        let (mut client, proxy) = UnixStream::pair().unwrap();
        let (native, mut backend) = UnixStream::pair().unwrap();
        let calls = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                relay_authenticated(proxy, native, &mut |_, _| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "fixture authorization refused",
                    ))
                })
            });
            let query = b"{\"method\":\"wall.list\",\"params\":{},\"id\":1}\n";
            client.write_all(query).unwrap();
            let mut received = vec![0; query.len()];
            backend.read_exact(&mut received).unwrap();
            assert_eq!(received, query);
            backend.write_all(b"{\"id\":1,\"result\":{\"wallpapers\":[]}}\n").unwrap();
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let mut reply = String::new();
            reader.read_line(&mut reply).unwrap();
            assert!(reply.contains("wallpapers"));
            let action = WallpaperApply {
                kind: wire::WallpaperKind::Static,
                path: "/home/fixture/Pictures/a.jpg".into(),
                outputs: vec![],
                mute: BTreeMap::new(),
                volume: BTreeMap::new(),
            };
            client.write_all(&action.frame(2).unwrap()).unwrap();
            reply.clear();
            reader.read_line(&mut reply).unwrap();
            assert!(reply.contains("error"));
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            client.write_all(b"{\"method\":\"state.set\",\"params\":{\"key\":\"x\",\"value\":\"y\"},\"id\":3}\n").unwrap();
            reply.clear();
            reader.read_line(&mut reply).unwrap();
            assert!(reply.contains("recovery write envelope"));
            backend.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            assert!(backend.read(&mut [0]).is_err());
            assert_eq!(calls.load(Ordering::Relaxed), 1);
            client.shutdown(std::net::Shutdown::Both).unwrap();
            backend.shutdown(std::net::Shutdown::Both).unwrap();
            worker.join().unwrap().unwrap();
        });
    }
}
