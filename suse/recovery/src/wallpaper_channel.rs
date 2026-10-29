//! Anonymous native connections. The broker never publishes a backend path.
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::{
    io::{self, IoSlice, Read, Write},
    mem::MaybeUninit,
    os::{fd::AsFd, unix::net::UnixStream},
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::wallpaper_protocol::{
    ActorCall, ActorIdentity, MAX_ACTOR_REPLY_BYTES, NativeChange, bounded_line,
    parse_native_change,
};

pub(crate) struct NativeActor {
    control: Mutex<UnixStream>,
    identity: ActorIdentity,
    owner_pid: u32,
    uid: u32,
    executable: crate::writer_freeze::ExecutablePin,
    manual_paused: std::sync::atomic::AtomicBool,
    metadata_readers: std::sync::atomic::AtomicUsize,
}

impl NativeActor {
    pub(crate) fn shutdown(&self) {
        if let Ok(control) = self.control.lock() {
            let _ = control.shutdown(std::net::Shutdown::Both);
        }
    }
    pub(crate) fn new(control: UnixStream, child_pid: u32, uid: u32) -> io::Result<Self> {
        control.set_write_timeout(Some(Duration::from_secs(5)))?;
        let owner_pid = std::process::id();
        let identity = process_identity(child_pid, owner_pid, uid)?;
        let executable = crate::writer_freeze::ExecutablePin::open(std::path::Path::new(
            "/usr/libexec/ryoku/upstream/ryogami",
        ))
        .map_err(io::Error::other)?;
        if !executable.matches(child_pid as i32).map_err(io::Error::other)? {
            return Err(io::Error::other("native wallpaper image differs from installed actor"));
        }
        Ok(Self {
            control: Mutex::new(control),
            identity,
            owner_pid,
            uid,
            executable,
            manual_paused: std::sync::atomic::AtomicBool::new(false),
            metadata_readers: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub(crate) fn identity(&self) -> io::Result<ActorIdentity> {
        if process_identity(self.identity.pid, self.owner_pid, self.uid)? != self.identity
            || !self.executable.matches(self.identity.pid as i32).map_err(io::Error::other)?
        {
            return Err(io::Error::other("native wallpaper actor identity changed"));
        }
        Ok(self.identity.clone())
    }

    pub(crate) fn manual_paused(&self) -> io::Result<bool> {
        self.identity()?;
        Ok(self.manual_paused.load(std::sync::atomic::Ordering::Acquire))
    }
    pub(crate) fn record_manual_pause(&self, paused: bool) -> io::Result<()> {
        self.identity()?;
        self.manual_paused.store(paused, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub(crate) fn player_identities(&self) -> io::Result<Vec<ActorIdentity>> {
        self.identity()?;
        let image = crate::writer_freeze::ExecutablePin::open(std::path::Path::new(
            "/usr/bin/ryogami-live",
        ))
        .map_err(io::Error::other)?;
        let mut players = Vec::new();
        for pid in crate::writer_freeze::owned_child_processes(self.identity.pid as i32)
            .map_err(io::Error::other)?
        {
            if image.matches(pid).map_err(io::Error::other)? {
                players.push(process_identity(pid as u32, self.identity.pid, self.uid)?);
            }
        }
        if players.len() > 64 {
            return Err(io::Error::other("wallpaper player capacity"));
        }
        self.identity()?;
        Ok(players)
    }

    pub(crate) fn preserved_player_stops(
        &self,
    ) -> io::Result<Vec<crate::writer_freeze::PreservedPlayerStop>> {
        self.identity()?;
        let stops = crate::writer_freeze::stopped_actor_players(self.identity.pid as i32, self.uid)
            .map_err(io::Error::other)?;
        self.identity()?;
        Ok(stops)
    }

    pub(crate) fn execution(
        &self,
        call: &ActorCall,
    ) -> io::Result<crate::writer_freeze::ActorExecution> {
        self.identity()?;
        let (_, action) = parse_native_change(&call.frame).map_err(io::Error::other)?;
        let programs: &[&str] =
            if matches!(action, NativeChange::ModelReferences(_) | NativeChange::MediaCatalog(_)) {
                &[]
            } else {
                &["/usr/bin/ryogami-live", "/usr/bin/ffmpeg", "/usr/bin/ffprobe"]
            };
        let executables = programs
            .iter()
            .map(|path| {
                crate::writer_freeze::ExecutablePin::open(std::path::Path::new(path))
                    .map_err(io::Error::other)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let playback = if let NativeChange::Wallpaper(
            crate::wallpaper_protocol::WallpaperChange::Pause(action),
        ) = action
        {
            Some(crate::writer_freeze::PlaybackExecution {
                players: call.player_pins.clone(),
                requested_paused: action.paused,
            })
        } else {
            None
        };
        let pause_new_image = if call.inherit_manual_pause {
            Some(
                crate::writer_freeze::ExecutablePin::open(std::path::Path::new(
                    "/usr/bin/ryogami-live",
                ))
                .map_err(io::Error::other)?,
            )
        } else {
            None
        };
        Ok(crate::writer_freeze::ActorExecution {
            playback,
            pause_new_image,
            actor_pid: self.identity.pid as i32,
            executables,
        })
    }

    fn connection(&self, kind: ChannelKind) -> io::Result<UnixStream> {
        self.identity()?;
        let (broker, native) = UnixStream::pair()?;
        let control =
            self.control.lock().map_err(|_| io::Error::other("native control channel poisoned"))?;
        transfer_connection(&control, &native, kind)?;
        Ok(broker)
    }

    /// Retained by the Rust framed proxy, never transferred to a UI client.
    pub(crate) fn query_connection(&self) -> io::Result<UnixStream> {
        self.connection(ChannelKind::ReadOnly)
    }

    /// Root-authenticated source metadata only. This separate private channel
    /// cannot carry an ActorCall or any native mutation. The Go receiver uses
    /// an immutable settings snapshot and the HTTP-only source dispatcher.
    pub(crate) fn source_metadata(
        &self,
        call: &crate::ryogami_sources_protocol::SourceMetadataCall,
        home: &str,
    ) -> io::Result<crate::ryogami_sources_protocol::SourceMetadataResponse> {
        use std::sync::atomic::Ordering;
        const MAX_READERS: usize = 4;
        self.metadata_readers
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_READERS).then_some(count + 1)
            })
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "source metadata is busy"))?;
        struct Reader<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for Reader<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let _reader = Reader(&self.metadata_readers);
        call.validate(&self.identity()?, home).map_err(io::Error::other)?;
        let connection = self.connection(ChannelKind::SourceMetadata)?;
        let response = exchange_source_metadata(connection, call)?;
        self.identity()?;
        Ok(response)
    }

    /// Called only by the authenticated root writer-control handler after its
    /// one-call permit is validated and the selected actor has been resumed.
    pub(crate) fn apply(&self, call: &ActorCall) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        call.validate(&self.identity()?).map_err(io::Error::other)?;
        let (id, _) = parse_native_change(&call.frame).map_err(io::Error::other)?;
        let mut connection = self.connection(ChannelKind::Approved)?;
        connection.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut permit = serde_json::to_vec(call).map_err(io::Error::other)?;
        if permit.len() > 4 * crate::wallpaper_protocol::MAX_REQUEST_BYTES {
            return Err(io::Error::other("native wallpaper permit exceeds bound"));
        }
        permit.push(b'\n');
        connection.write_all(&permit)?;
        // A cannot be reused for a second action, including by a native error
        // path. The receiver validates the single frame followed by EOF.
        connection.shutdown(std::net::Shutdown::Write)?;
        let mut reply = Vec::new();
        loop {
            connection.set_read_timeout(Some(
                deadline.checked_duration_since(Instant::now()).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "native actor deadline exceeded")
                })?,
            ))?;
            let mut chunk = [0; 4096];
            let count = connection.read(&mut chunk)?;
            if count == 0 || reply.len().saturating_add(count) > MAX_ACTOR_REPLY_BYTES {
                return Err(io::Error::other("missing or oversized native wallpaper reply"));
            }
            let newline = chunk[..count].iter().position(|byte| *byte == b'\n');
            if newline.is_some_and(|index| index + 1 != count) {
                return Err(io::Error::other("native actor returned more than one frame"));
            }
            reply.extend_from_slice(&chunk[..count]);
            if newline.is_some() {
                break;
            }
        }
        bounded_line(&reply, MAX_ACTOR_REPLY_BYTES).map_err(io::Error::other)?;
        let value: serde_json::Value = serde_json::from_slice(&reply).map_err(io::Error::other)?;
        let object =
            value.as_object().ok_or_else(|| io::Error::other("native reply is not an object"))?;
        if object.get("id").and_then(serde_json::Value::as_i64) != Some(id)
            || object.keys().any(|key| !["id", "result", "error"].contains(&key.as_str()))
            || object.contains_key("result") == object.contains_key("error")
        {
            return Err(io::Error::other("native wallpaper reply does not match action"));
        }
        let mut trailing = [0];
        connection.set_read_timeout(Some(
            deadline.checked_duration_since(Instant::now()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "native actor deadline exceeded")
            })?,
        ))?;
        if (&connection).read(&mut trailing)? != 0 {
            return Err(io::Error::other("native wallpaper action channel was reused"));
        }
        self.identity()?;
        Ok(reply)
    }
}

fn exchange_source_metadata(
    mut connection: UnixStream,
    call: &crate::ryogami_sources_protocol::SourceMetadataCall,
) -> io::Result<crate::ryogami_sources_protocol::SourceMetadataResponse> {
    call.query.validate().map_err(io::Error::other)?;
    connection.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut request = serde_json::to_vec(call).map_err(io::Error::other)?;
    request.push(b'\n');
    bounded_line(&request, crate::ryogami_sources_protocol::MAX_FRAME_BYTES)
        .map_err(io::Error::other)?;
    connection.write_all(&request)?;
    connection.shutdown(std::net::Shutdown::Write)?;
    let maximum = crate::ryogami_sources_protocol::MAX_METADATA_RESULT_BYTES + 4096;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut reply = Vec::new();
    loop {
        connection.set_read_timeout(Some(
            deadline.checked_duration_since(Instant::now()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "source metadata deadline exceeded")
            })?,
        ))?;
        let mut chunk = [0; 4096];
        let count = connection.read(&mut chunk)?;
        if count == 0 || reply.len().saturating_add(count) > maximum {
            return Err(io::Error::other("missing or oversized source metadata reply"));
        }
        let newline = chunk[..count].iter().position(|byte| *byte == b'\n');
        if newline.is_some_and(|index| index + 1 != count) {
            return Err(io::Error::other("source metadata returned multiple frames"));
        }
        reply.extend_from_slice(&chunk[..count]);
        if newline.is_some() {
            break;
        }
    }
    bounded_line(&reply, maximum).map_err(io::Error::other)?;
    let response: crate::ryogami_sources_protocol::SourceMetadataResponse =
        serde_json::from_slice(&reply).map_err(io::Error::other)?;
    response.validate(call).map_err(io::Error::other)?;
    let mut trailing = [0];
    connection.set_read_timeout(Some(
        deadline.checked_duration_since(Instant::now()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::TimedOut, "source metadata deadline exceeded")
        })?,
    ))?;
    if connection.read(&mut trailing)? != 0 {
        return Err(io::Error::other("source metadata channel was reused"));
    }
    Ok(response)
}

/// Kernel process identity is checked independently of caller-provided hashes.
/// The only accepted actor is the fixed native child created by this broker.
fn process_identity(pid: u32, parent: u32, uid: u32) -> io::Result<ActorIdentity> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let actual_parent = status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:\t"))
        .and_then(|value| value.parse::<u32>().ok());
    let uids: Option<Vec<u32>> = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:\t"))
        .map(|values| values.split_whitespace().map(str::parse).collect::<Result<_, _>>())
        .transpose()
        .map_err(io::Error::other)?;
    if actual_parent != Some(parent) || uids.as_deref() != Some([uid; 4].as_slice()) {
        return Err(io::Error::other("native wallpaper actor is not the broker's account child"));
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, fields) =
        stat.rsplit_once(") ").ok_or_else(|| io::Error::other("invalid native actor stat"))?;
    let start_time = fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::other("missing native actor start identity"))?
        .parse()
        .map_err(io::Error::other)?;
    Ok(ActorIdentity { pid, start_time })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChannelKind {
    ReadOnly,
    Approved,
    SourceMetadata,
}

impl ChannelKind {
    fn byte(self) -> u8 {
        match self {
            Self::ReadOnly => b'R',
            Self::Approved => b'A',
            Self::SourceMetadata => b'S',
        }
    }
}

/// The caller serializes sends on the inherited control stream. One descriptor
/// and one kind byte create one independent existing-native-protocol connection.
pub(crate) fn transfer_connection(
    control: &UnixStream,
    connection: &UnixStream,
    kind: ChannelKind,
) -> io::Result<()> {
    let descriptors = [connection.as_fd()];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    if !ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)) {
        return Err(io::Error::other("native descriptor transfer buffer is too small"));
    }
    if sendmsg(control, &[IoSlice::new(&[kind.byte()])], &mut ancillary, SendFlags::NOSIGNAL)? != 1
    {
        return Err(io::Error::other("incomplete native descriptor transfer"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, recvmsg};
    use std::{
        io::{IoSliceMut, Read, Write},
        os::fd::OwnedFd,
    };

    fn receive(control: &UnixStream) -> (u8, UnixStream) {
        let mut byte = [0];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let result = recvmsg(
            control,
            &mut [IoSliceMut::new(&mut byte)],
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC,
        )
        .unwrap();
        assert_eq!(result.bytes, 1);
        assert!(!result.flags.contains(ReturnFlags::CTRUNC));
        let mut descriptors: Vec<OwnedFd> = vec![];
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(fds) = message {
                descriptors.extend(fds);
            } else {
                panic!("unexpected ancillary message");
            }
        }
        assert_eq!(descriptors.len(), 1);
        (byte[0], UnixStream::from(descriptors.pop().unwrap()))
    }

    #[test]
    fn native_connections_are_anonymous_independent_and_keep_request_ids_separate() {
        let (broker, daemon) = UnixStream::pair().unwrap();
        let (mut first, first_native) = UnixStream::pair().unwrap();
        let (mut second, second_native) = UnixStream::pair().unwrap();
        transfer_connection(&broker, &first_native, ChannelKind::ReadOnly).unwrap();
        transfer_connection(&broker, &second_native, ChannelKind::Approved).unwrap();
        drop(first_native);
        drop(second_native);
        let (kind, mut native_first) = receive(&daemon);
        assert_eq!(kind, b'R');
        let (kind, mut native_second) = receive(&daemon);
        assert_eq!(kind, b'A');
        first.write_all(b"first id=1\n").unwrap();
        second.write_all(b"second id=1\n").unwrap();
        let mut bytes = [0; 11];
        native_first.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"first id=1\n");
        let mut bytes = [0; 12];
        native_second.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"second id=1\n");
        assert!(native_first.local_addr().unwrap().as_pathname().is_none());
        assert!(native_second.local_addr().unwrap().as_pathname().is_none());
    }

    fn metadata_call() -> crate::ryogami_sources_protocol::SourceMetadataCall {
        use crate::ryogami_sources_protocol::*;
        SourceMetadataCall {
            actor: ActorIdentity { pid: 42, start_time: 77 },
            account_home_sha256: crate::Sha256::new("a".repeat(64)).unwrap(),
            request_id: crate::Identifier::new("source-channel-fixture").unwrap(),
            query: PreparedSourceMetadataQuery {
                expected_settings: None,
                query: SourceMetadataQuery {
                    source: Source::Bing,
                    search: Search {
                        query: "fixture".into(),
                        page: 1,
                        generation: 9,
                        filters: SearchFilters::Bing { market: "en-US".into() },
                    },
                },
            },
            rpc_id: 12,
        }
    }

    #[test]
    fn source_metadata_has_an_independent_one_use_descriptor_and_correlated_reply() {
        use crate::ryogami_sources_protocol::*;
        let (broker, daemon) = UnixStream::pair().unwrap();
        let (connection, native) = UnixStream::pair().unwrap();
        transfer_connection(&broker, &native, ChannelKind::SourceMetadata).unwrap();
        drop(native);
        let (kind, mut native) = receive(&daemon);
        assert_eq!(kind, b'S');
        let call = metadata_call();
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                let mut bytes = Vec::new();
                native.read_to_end(&mut bytes).unwrap();
                assert_eq!(serde_json::from_slice::<SourceMetadataCall>(&bytes).unwrap(), call);
                let response = SourceMetadataResponse {
                    id: call.rpc_id,
                    request_id: call.request_id.clone(),
                    error: None,
                    result: Some(SourceMetadataPage {
                        results: vec![],
                        last_page: 1,
                        current_page: 1,
                        next_cursor: String::new(),
                    }),
                };
                let mut bytes = serde_json::to_vec(&response).unwrap();
                bytes.push(b'\n');
                native.write_all(&bytes).unwrap();
                native.shutdown(std::net::Shutdown::Both).unwrap();
            });
            let response = exchange_source_metadata(connection, &call).unwrap();
            assert_eq!(response.id, 12);
            assert!(response.result.unwrap().results.is_empty());
            server.join().unwrap();
        });
    }

    #[test]
    fn source_metadata_refuses_mismatched_oversized_and_reused_responses() {
        let call = metadata_call();
        for fault in ["id", "nonce", "multiple", "reuse", "oversize", "truncated"] {
            let (connection, mut native) = UnixStream::pair().unwrap();
            std::thread::scope(|scope| {
                let server = scope.spawn(|| {
                    let mut request = Vec::new();
                    native.read_to_end(&mut request).unwrap();
                    let mut response = serde_json::json!({
                        "id":call.rpc_id,"request_id":call.request_id,
                        "error":"provider_unavailable"
                    });
                    if fault == "id" {
                        response["id"] = serde_json::json!(13);
                    }
                    if fault == "nonce" {
                        response["request_id"] = serde_json::json!("source-other");
                    }
                    let mut bytes = serde_json::to_vec(&response).unwrap();
                    if fault != "truncated" {
                        bytes.push(b'\n');
                    }
                    if fault == "multiple" {
                        bytes.extend_from_slice(b"{}\n");
                    }
                    if fault == "oversize" {
                        bytes = vec![
                            b' ';
                            crate::ryogami_sources_protocol::MAX_METADATA_RESULT_BYTES
                                + 4097
                        ];
                    }
                    let _ = native.write_all(&bytes);
                    if fault == "reuse" {
                        let _ = native.write_all(b"{}");
                    }
                    let _ = native.shutdown(std::net::Shutdown::Both);
                });
                assert!(exchange_source_metadata(connection, &call).is_err(), "{fault}");
                server.join().unwrap();
            });
        }
    }
}
