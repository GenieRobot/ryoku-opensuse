//! Versioned, typed boundary between the per-user service and root helper.
//! The installed operator boundary executes only the reviewed wallpaper runner;
//! all other mutation families retain explicit unsupported responses.

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::sockopt::socket_error,
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with},
};
use std::io::{BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{
    HelperRequest, ManagedActionKind, PeerIdentity, ProtocolError, RequestId, ServiceError,
    authorize_helper_operator, authorize_helper_peer,
};

pub const HELPER_PROTOCOL_VERSION: u16 = 1;
pub const MAX_HELPER_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_HELPER_RESPONSE_BYTES: usize = 64 * 1024;
const HELPER_IO_TIMEOUT: Duration = Duration::from_secs(5);
pub const HELPER_SOCKET_PATH: &str = "/run/ryoku/helper.sock";

/// Resolve only the authenticated operator's fixed configuration root.
/// This preflight grants no capture, mutation, or protected-history access.
pub fn prepare_operator_account(
    peer: &PeerIdentity,
    operator_gid: u32,
) -> Result<crate::account_state::AccountStateRoot, ServiceError> {
    with_operator_account(peer, operator_gid, crate::account_state::AccountStateRoot::for_peer)
}

fn with_operator_account<T>(
    peer: &PeerIdentity,
    operator_gid: u32,
    resolve: impl FnOnce(&PeerIdentity) -> Result<T, crate::account_state::AccountStateError>,
) -> Result<T, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    resolve(peer).map_err(|error| match error {
        crate::account_state::AccountStateError::LookupBusy => ServiceError::Busy,
        _ => ServiceError::RecoveryUnavailable,
    })
}

/// One helper request frame. The request language is the explicit
/// [`HelperRequest`] enum; unknown fields are rejected during decoding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperEnvelope {
    pub version: u16,
    pub request_id: RequestId,
    pub request: HelperRequest,
}

impl HelperEnvelope {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != HELPER_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        self.request.validate()
    }
}

/// A typed result. Wallpaper success is emitted only after the protected runner
/// has verified its post-checkpoint and completed the operation journal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperResponse {
    pub version: u16,
    pub request_id: RequestId,
    pub outcome: HelperOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "HelperOutcomeWire", into = "HelperOutcomeWire")]
pub enum HelperOutcome {
    Rejected(ServiceError),
    WallpaperApplied { operation_id: crate::OperationId, reply: Vec<u8> },
    WallpaperAudioSet { operation_id: crate::OperationId, reply: Vec<u8> },
    WorkspaceWallpaperChanged { operation_id: crate::OperationId, reply: Vec<u8> },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum HelperOutcomeWire {
    Rejected { error: ServiceError },
    WallpaperApplied { operation_id: crate::OperationId, reply: Vec<u8> },
    WallpaperAudioSet { operation_id: crate::OperationId, reply: Vec<u8> },
    WorkspaceWallpaperChanged { operation_id: crate::OperationId, reply: Vec<u8> },
}
impl From<HelperOutcome> for HelperOutcomeWire {
    fn from(value: HelperOutcome) -> Self {
        match value {
            HelperOutcome::Rejected(error) => Self::Rejected { error },
            HelperOutcome::WallpaperAudioSet { operation_id, reply } => {
                Self::WallpaperAudioSet { operation_id, reply }
            }
            HelperOutcome::WorkspaceWallpaperChanged { operation_id, reply } => {
                Self::WorkspaceWallpaperChanged { operation_id, reply }
            }
            HelperOutcome::WallpaperApplied { operation_id, reply } => {
                Self::WallpaperApplied { operation_id, reply }
            }
        }
    }
}
impl From<HelperOutcomeWire> for HelperOutcome {
    fn from(value: HelperOutcomeWire) -> Self {
        match value {
            HelperOutcomeWire::Rejected { error } => Self::Rejected(error),
            HelperOutcomeWire::WallpaperAudioSet { operation_id, reply } => {
                Self::WallpaperAudioSet { operation_id, reply }
            }
            HelperOutcomeWire::WorkspaceWallpaperChanged { operation_id, reply } => {
                Self::WorkspaceWallpaperChanged { operation_id, reply }
            }
            HelperOutcomeWire::WallpaperApplied { operation_id, reply } => {
                Self::WallpaperApplied { operation_id, reply }
            }
        }
    }
}

/// Read one bounded newline-delimited helper frame and validate both its
/// protocol version and typed references before policy sees it.
pub fn read_helper_envelope(reader: &mut impl BufRead) -> Result<HelperEnvelope, ProtocolError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut limited = reader.take((MAX_HELPER_REQUEST_BYTES + 2) as u64);
    limited.read_until(b'\n', &mut bytes).map_err(|_| ProtocolError::Io)?;
    if bytes.len() > MAX_HELPER_REQUEST_BYTES + 1 {
        return Err(ProtocolError::TooLarge);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(ProtocolError::Malformed("incomplete helper request frame"));
    }
    bytes.pop();
    let envelope: HelperEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| ProtocolError::Malformed("invalid helper request"))?;
    envelope.validate()?;
    Ok(envelope)
}

/// Evaluate the current helper policy. Even authenticated, valid operations
/// are rejected until linked checkpoint and provider evidence are integrated.
pub fn dispatch_helper_request(
    peer: &PeerIdentity,
    service_uid: u32,
    envelope: HelperEnvelope,
) -> Result<HelperResponse, ProtocolError> {
    envelope.validate()?;
    let outcome = if authorize_helper_peer(peer, service_uid).is_err() {
        HelperOutcome::Rejected(ServiceError::Unauthorized)
    } else {
        // This service-account boundary has no executable request variant.
        // Operator wallpaper dispatch uses its separate authenticated path.
        HelperOutcome::Rejected(ServiceError::Unsupported)
    };
    Ok(HelperResponse {
        version: HELPER_PROTOCOL_VERSION,
        request_id: envelope.request_id,
        outcome,
    })
}

/// Dispatch for the installed root helper. Socket mode gates membership at
/// connect time; kernel peer credentials independently validate the group and UID.
pub fn dispatch_operator_request(
    peer: PeerIdentity,
    operator_gid: u32,
    envelope: HelperEnvelope,
) -> Result<HelperResponse, ProtocolError> {
    envelope.validate()?;
    let outcome = if authorize_helper_operator(&peer, operator_gid).is_err() {
        HelperOutcome::Rejected(ServiceError::Unauthorized)
    } else {
        match envelope.request {
            HelperRequest::ApplyWallpaper { action, rpc_id } => {
                match crate::wallpaper::apply(&peer, operator_gid, action, rpc_id) {
                    Ok(completed) => HelperOutcome::WallpaperApplied {
                        operation_id: completed.operation_id,
                        reply: completed.reply,
                    },
                    Err(error) => HelperOutcome::Rejected(error),
                }
            }
            HelperRequest::SetWallpaperAudio { action, rpc_id } => {
                match crate::wallpaper::set_audio(&peer, operator_gid, action, rpc_id) {
                    Ok(completed) => HelperOutcome::WallpaperAudioSet {
                        operation_id: completed.operation_id,
                        reply: completed.reply,
                    },
                    Err(error) => HelperOutcome::Rejected(error),
                }
            }
            HelperRequest::ChangeWorkspaceWallpaper { action, rpc_id } => {
                match crate::wallpaper::workspace(&peer, operator_gid, action, rpc_id) {
                    Ok(completed) => HelperOutcome::WorkspaceWallpaperChanged {
                        operation_id: completed.operation_id,
                        reply: completed.reply,
                    },
                    Err(error) => HelperOutcome::Rejected(error),
                }
            }
            _ => HelperOutcome::Rejected(ServiceError::Unsupported),
        }
    };
    Ok(HelperResponse {
        version: HELPER_PROTOCOL_VERSION,
        request_id: envelope.request_id,
        outcome,
    })
}

/// Handle one helper socket connection. Peer credentials are obtained before
/// request dispatch, and valid requests can produce only typed rejection
/// outcomes. This function neither accepts nor persists an operation.
pub fn serve_helper_connection(
    mut stream: UnixStream,
    service_uid: u32,
) -> Result<(), ProtocolError> {
    let deadline = Instant::now() + HELPER_IO_TIMEOUT;
    stream.set_nonblocking(true).map_err(|_| ProtocolError::Io)?;

    // Read kernel credentials before parsing or dispatching a request. The
    // helper rejects unauthorized peers without parsing their request. Since
    // no request ID is trusted yet, it uses a fixed correlation ID.
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    if authorize_helper_peer(&peer, service_uid).is_err() {
        return write_helper_response_until(
            &mut stream,
            HelperResponse {
                version: HELPER_PROTOCOL_VERSION,
                request_id: RequestId::new("invalid").expect("static request ID is valid"),
                outcome: HelperOutcome::Rejected(ServiceError::Unauthorized),
            },
            deadline,
        );
    }
    let frame = match read_helper_frame_until(&mut stream, deadline) {
        Ok(frame) => frame,
        Err(error @ ProtocolError::Malformed("incomplete helper request frame")) => {
            return Err(error);
        }
        Err(ProtocolError::Io) => return Err(ProtocolError::Io),
        Err(error) => {
            let outcome = match error {
                ProtocolError::UnsupportedVersion(_) => {
                    HelperOutcome::Rejected(ServiceError::Unsupported)
                }
                ProtocolError::Malformed(_) | ProtocolError::TooLarge => HelperOutcome::Rejected(
                    ServiceError::Malformed("invalid helper request".into()),
                ),
                ProtocolError::Io => unreachable!(),
            };
            return write_helper_response_until(
                &mut stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("invalid").expect("static request ID is valid"),
                    outcome,
                },
                deadline,
            );
        }
    };
    let envelope = match read_helper_envelope(&mut std::io::Cursor::new(frame)) {
        Ok(envelope) => envelope,
        Err(ProtocolError::Malformed("incomplete helper request frame")) => {
            return Err(ProtocolError::Malformed("incomplete helper request frame"));
        }
        Err(ProtocolError::Io) => return Err(ProtocolError::Io),
        Err(error) => {
            let outcome = match error {
                ProtocolError::UnsupportedVersion(_) => {
                    HelperOutcome::Rejected(ServiceError::Unsupported)
                }
                ProtocolError::Malformed(_) | ProtocolError::TooLarge => HelperOutcome::Rejected(
                    ServiceError::Malformed("invalid helper request".into()),
                ),
                ProtocolError::Io => unreachable!("I/O errors returned above"),
            };
            return write_helper_response_until(
                &mut stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("invalid").expect("static request ID is valid"),
                    outcome,
                },
                deadline,
            );
        }
    };

    let response = dispatch_helper_request(&peer, service_uid, envelope)?;
    write_helper_response_until(&mut stream, response, Instant::now() + HELPER_IO_TIMEOUT)
}

/// Serve one connection from the opt-in group-owned helper socket.
pub fn serve_operator_helper_connection(
    mut stream: UnixStream,
    operator_gid: u32,
) -> Result<(), ProtocolError> {
    let deadline = Instant::now() + HELPER_IO_TIMEOUT;
    stream.set_nonblocking(true).map_err(|_| ProtocolError::Io)?;
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    if authorize_helper_operator(&peer, operator_gid).is_err() {
        return write_helper_response_until(
            &mut stream,
            HelperResponse {
                version: HELPER_PROTOCOL_VERSION,
                request_id: RequestId::new("invalid").expect("static request ID is valid"),
                outcome: HelperOutcome::Rejected(ServiceError::Unauthorized),
            },
            deadline,
        );
    }
    let frame = match read_helper_frame_until(&mut stream, deadline) {
        Ok(frame) => frame,
        Err(error @ ProtocolError::Malformed("incomplete helper request frame")) => {
            return Err(error);
        }
        Err(ProtocolError::Io) => return Err(ProtocolError::Io),
        Err(error) => {
            let outcome = match error {
                ProtocolError::UnsupportedVersion(_) => {
                    HelperOutcome::Rejected(ServiceError::Unsupported)
                }
                ProtocolError::Malformed(_) | ProtocolError::TooLarge => HelperOutcome::Rejected(
                    ServiceError::Malformed("invalid helper request".into()),
                ),
                ProtocolError::Io => unreachable!(),
            };
            return write_helper_response_until(
                &mut stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("invalid").expect("static request ID is valid"),
                    outcome,
                },
                deadline,
            );
        }
    };
    let envelope = match read_helper_envelope(&mut std::io::Cursor::new(frame)) {
        Ok(envelope) => envelope,
        Err(ProtocolError::Malformed("incomplete helper request frame")) => {
            return Err(ProtocolError::Malformed("incomplete helper request frame"));
        }
        Err(ProtocolError::Io) => return Err(ProtocolError::Io),
        Err(error) => {
            let outcome = match error {
                ProtocolError::UnsupportedVersion(_) => {
                    HelperOutcome::Rejected(ServiceError::Unsupported)
                }
                ProtocolError::Malformed(_) | ProtocolError::TooLarge => HelperOutcome::Rejected(
                    ServiceError::Malformed("invalid helper request".into()),
                ),
                ProtocolError::Io => unreachable!("I/O errors returned above"),
            };
            return write_helper_response_until(
                &mut stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("invalid").expect("static request ID is valid"),
                    outcome,
                },
                deadline,
            );
        }
    };
    let response = dispatch_operator_request(peer, operator_gid, envelope)?;
    write_helper_response_until(&mut stream, response, Instant::now() + HELPER_IO_TIMEOUT)
}

pub fn check_action_availability(action: ManagedActionKind) -> ServiceError {
    check_action_availability_at(Path::new(HELPER_SOCKET_PATH), action)
}

pub(crate) fn check_action_availability_at(
    socket_path: &Path,
    action: ManagedActionKind,
) -> ServiceError {
    let deadline = Instant::now() + HELPER_IO_TIMEOUT;
    check_action_availability_until(socket_path, action, deadline)
}

pub(crate) fn check_action_availability_until(
    socket_path: &Path,
    action: ManagedActionKind,
    deadline: Instant,
) -> ServiceError {
    let mut stream = match connect_helper_until(socket_path, deadline) {
        Ok(stream) => stream,
        Err(error) => return helper_connect_error(error.kind()),
    };
    let result = (|| -> Result<ServiceError, ProtocolError> {
        let request = HelperEnvelope {
            version: HELPER_PROTOCOL_VERSION,
            request_id: RequestId::new("availability").expect("static request ID is valid"),
            request: HelperRequest::CheckAvailability { action },
        };
        let response = exchange_helper_until(&mut stream, &request, deadline)?;
        if response.version != HELPER_PROTOCOL_VERSION || response.request_id != request.request_id
        {
            return Err(ProtocolError::Malformed("helper response correlation mismatch"));
        }
        match response.outcome {
            HelperOutcome::Rejected(error) => Ok(error),
            HelperOutcome::WallpaperApplied { .. }
            | HelperOutcome::WallpaperAudioSet { .. }
            | HelperOutcome::WorkspaceWallpaperChanged { .. } => {
                Err(ProtocolError::Malformed("unexpected mutation response to availability probe"))
            }
        }
    })();
    match result {
        Ok(error) => error,
        Err(_) => ServiceError::RecoveryUnavailable,
    }
}

fn exchange_helper_until(
    stream: &mut UnixStream,
    request: &HelperEnvelope,
    deadline: Instant,
) -> Result<HelperResponse, ProtocolError> {
    stream.set_nonblocking(true).map_err(|_| ProtocolError::Io)?;
    let mut frame = serde_json::to_vec(request).map_err(|_| ProtocolError::Io)?;
    frame.push(b'\n');
    let mut pending = frame.as_slice();
    while !pending.is_empty() {
        wait_helper_ready(stream, PollFlags::OUT, deadline)?;
        match stream.write(pending) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(count) => pending = &pending[count..],
            Err(error)
                if error.kind() == std::io::ErrorKind::Interrupted
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(_) => return Err(ProtocolError::Io),
        }
    }
    let bytes = read_helper_frame_until(stream, deadline)?;
    read_helper_response(&mut bytes.as_slice())
}

fn read_helper_response(reader: &mut impl BufRead) -> Result<HelperResponse, ProtocolError> {
    let mut bytes = Vec::with_capacity(1024);
    reader
        .take((MAX_HELPER_RESPONSE_BYTES + 2) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| ProtocolError::Io)?;
    if bytes.len() > MAX_HELPER_RESPONSE_BYTES + 1 {
        return Err(ProtocolError::TooLarge);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(ProtocolError::Malformed("incomplete helper response frame"));
    }
    bytes.pop();
    serde_json::from_slice(&bytes).map_err(|_| ProtocolError::Malformed("invalid helper response"))
}

fn remaining(deadline: Instant) -> Result<Duration, ProtocolError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(ProtocolError::Io)
}

fn wait_helper_ready(
    stream: &UnixStream,
    flags: PollFlags,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    loop {
        let timeout = Timespec::try_from(remaining(deadline)?).map_err(|_| ProtocolError::Io)?;
        let mut descriptors = [PollFd::new(stream, flags)];
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(_) => return Err(ProtocolError::Io),
        }
    }
}

pub(crate) fn connect_helper_until(path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    remaining(deadline).map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC.union(SocketFlags::NONBLOCK),
        None,
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    let address = SocketAddrUnix::new(path)
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    match connect(&socket, &address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS | Errno::AGAIN) => loop {
            let timeout = Timespec::try_from(
                remaining(deadline)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?,
            )
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
            let mut descriptors = [PollFd::new(&socket, PollFlags::OUT)];
            match poll(&mut descriptors, Some(&timeout)) {
                Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::TimedOut)),
                Ok(_) => match socket_error(&socket) {
                    Ok(Ok(())) => break,
                    Ok(Err(error)) => {
                        return Err(std::io::Error::from_raw_os_error(error.raw_os_error()));
                    }
                    Err(error) => {
                        return Err(std::io::Error::from_raw_os_error(error.raw_os_error()));
                    }
                },
                Err(Errno::INTR) => continue,
                Err(error) => return Err(std::io::Error::from_raw_os_error(error.raw_os_error())),
            }
        },
        Err(error) => return Err(std::io::Error::from_raw_os_error(error.raw_os_error())),
    }
    remaining(deadline).map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
    Ok(UnixStream::from(socket))
}

fn read_helper_frame_until(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        wait_helper_ready(stream, PollFlags::IN, deadline)?;
        let count = match stream.read(&mut buffer) {
            Ok(0) => return Err(ProtocolError::Malformed("incomplete helper request frame")),
            Ok(count) => count,
            Err(error)
                if error.kind() == std::io::ErrorKind::Interrupted
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(_) => return Err(ProtocolError::Io),
        };
        let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
        let end = newline.map_or(count, |index| index + 1);
        if newline.is_some_and(|index| index + 1 != count) {
            return Err(ProtocolError::Malformed("multiple helper request frames"));
        }
        if bytes.len() + end > MAX_HELPER_REQUEST_BYTES + 1 {
            return Err(ProtocolError::TooLarge);
        }
        bytes.extend_from_slice(&buffer[..end]);
        if newline.is_some() {
            return Ok(bytes);
        }
    }
}

fn write_helper_response_until(
    stream: &mut UnixStream,
    response: HelperResponse,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    let mut frame = serde_json::to_vec(&response).map_err(|_| ProtocolError::Io)?;
    if frame.len() + 1 > MAX_HELPER_RESPONSE_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    frame.push(b'\n');
    let mut pending = frame.as_slice();
    while !pending.is_empty() {
        wait_helper_ready(stream, PollFlags::OUT, deadline)?;
        match stream.write(pending) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(count) => pending = &pending[count..],
            Err(error)
                if error.kind() == std::io::ErrorKind::Interrupted
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(_) => return Err(ProtocolError::Io),
        }
    }
    Ok(())
}

fn helper_connect_error(kind: std::io::ErrorKind) -> ServiceError {
    if kind == std::io::ErrorKind::PermissionDenied {
        ServiceError::Unauthorized
    } else {
        ServiceError::RecoveryUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckpointId, ManagedActionKind, PackageId, authorize_helper_operator};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;

    #[test]
    fn manual_workspace_helper_has_no_client_preparation_or_authority_shortcut() {
        use ryoku_recovery::workspace_protocol::{
            WorkspaceOperation, WorkspaceRequest, WorkspaceSelector,
        };
        for operation in [WorkspaceOperation::Assign, WorkspaceOperation::Unassign] {
            let action = WorkspaceRequest {
                operation,
                workspace: WorkspaceSelector { output: "DP-1".into(), ..Default::default() },
                wallpaper: None,
            };
            let response = dispatch_operator_request(
                PeerIdentity { uid: 1000, gid: 1000, supplementary_gids: vec![], pid: None },
                4242,
                valid_envelope(HelperRequest::ChangeWorkspaceWallpaper { action, rpc_id: 4 }),
            )
            .unwrap();
            assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
        }
        let success = HelperOutcome::WorkspaceWallpaperChanged {
            operation_id: crate::OperationId::new("workspace-wire").unwrap(),
            reply: b"reply".to_vec(),
        };
        let wire = serde_json::to_value(&success).unwrap();
        assert_eq!(wire["result"], "workspace_wallpaper_changed");
        assert!(wire.get("reply").is_some());
        assert!(wire.get("error").is_none());
        assert_eq!(serde_json::from_value::<HelperOutcome>(wire).unwrap(), success);
        assert!(serde_json::from_value::<HelperRequest>(serde_json::json!({"operation":"change_workspace_wallpaper","rpc_id":4,"action":{"operation":"assign","workspace":{"output":"DP-1"},"wallpaper":null,"prepared":{"active":true}}})).is_err());
    }
    #[test]
    fn audio_wire_and_dispatch_keep_action_authority_separate() {
        let action = ryoku_recovery::wallpaper_protocol::WallpaperAudio {
            outputs: vec![],
            mute: Some(true),
            volume: Some(35),
        };
        let envelope =
            valid_envelope(HelperRequest::SetWallpaperAudio { action: action.clone(), rpc_id: 7 });
        envelope.validate().unwrap();
        let response = dispatch_operator_request(
            PeerIdentity { uid: 1000, gid: 1000, supplementary_gids: vec![], pid: None },
            4242,
            envelope,
        )
        .unwrap();
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
        assert!(
            valid_envelope(HelperRequest::SetWallpaperAudio { action, rpc_id: -1 })
                .validate()
                .is_err()
        );
        let success = HelperOutcome::WallpaperAudioSet {
            operation_id: crate::OperationId::new("audio-wire-1").unwrap(),
            reply: b"audio-reply".to_vec(),
        };
        let wire = serde_json::to_value(&success).unwrap();
        assert_eq!(wire["result"], "wallpaper_audio_set");
        assert!(wire.get("reply").is_some());
        assert!(wire.get("error").is_none());
        assert_eq!(serde_json::from_value::<HelperOutcome>(wire).unwrap(), success);
    }

    #[test]
    fn wallpaper_success_wire_matches_proxy_and_preserves_rejection_shape() {
        let operation_id = crate::OperationId::new("wallpaper-wire-1").unwrap();
        let reply = br#"{"id":7,"result":{"applied":true}}"#.to_vec();
        let success = HelperOutcome::WallpaperApplied { operation_id, reply: reply.clone() };
        let wire = serde_json::to_value(&success).unwrap();
        assert_eq!(wire["result"], "wallpaper_applied");
        assert_eq!(wire["reply"], serde_json::json!(reply));
        assert!(wire.get("error").is_none());
        assert_eq!(serde_json::from_value::<HelperOutcome>(wire).unwrap(), success);
        let rejection = HelperOutcome::Rejected(ServiceError::Unsupported);
        let wire = serde_json::to_value(&rejection).unwrap();
        assert_eq!(wire, serde_json::json!({"result":"rejected","error":{"code":"unsupported"}}));
        assert_eq!(serde_json::from_value::<HelperOutcome>(wire).unwrap(), rejection);
        assert!(
            serde_json::from_value::<HelperOutcome>(serde_json::json!({
                "result":"wallpaper_applied","operation_id":"wallpaper-wire-1",
                "error":{"reply":[]}
            }))
            .is_err()
        );
    }

    #[test]
    fn wallpaper_dispatch_rejects_unauthorized_before_provider_lookup() {
        let action = serde_json::from_value(serde_json::json!({
            "type":"static","path":"/not-accessed.png","outputs":[],
            "outputs_audio":{},"outputs_volume":{}
        }))
        .unwrap();
        let response = dispatch_operator_request(
            PeerIdentity { uid: 1000, gid: 1000, supplementary_gids: vec![], pid: None },
            4242,
            valid_envelope(HelperRequest::ApplyWallpaper { action, rpc_id: 7 }),
        )
        .unwrap();
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
    }

    #[test]
    fn account_resolution_occurs_only_after_operator_authorization() {
        let peer = PeerIdentity { uid: 1000, gid: 1000, supplementary_gids: vec![], pid: None };
        let result: Result<(), _> = with_operator_account(&peer, 4242, |_| {
            panic!("unauthorized caller reached account lookup")
        });
        assert_eq!(result, Err(ServiceError::Unauthorized));
        let peer = PeerIdentity { supplementary_gids: vec![4242], ..peer };
        assert_eq!(with_operator_account(&peer, 4242, |resolved| Ok(resolved.uid())), Ok(1000));
    }

    #[test]
    fn helper_response_frame_has_a_bounded_size_and_requires_newline() {
        let oversized = vec![b'x'; MAX_HELPER_RESPONSE_BYTES + 2];
        assert_eq!(read_helper_response(&mut oversized.as_slice()), Err(ProtocolError::TooLarge));
        assert!(matches!(read_helper_response(&mut &b"{}"[..]), Err(ProtocolError::Malformed(_))));
    }

    #[test]
    fn trickling_input_cannot_extend_the_absolute_helper_deadline() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..40 {
                if client.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let started = Instant::now();
        assert_eq!(
            read_helper_frame_until(&mut server, started + Duration::from_millis(50)),
            Err(ProtocolError::Io)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(server);
        writer.join().unwrap();
    }

    #[test]
    fn expired_helper_deadline_fails_before_socket_io() {
        let (_peer, mut stream) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let deadline = Instant::now() - Duration::from_millis(1);
        assert_eq!(read_helper_frame_until(&mut stream, deadline), Err(ProtocolError::Io));
        assert_eq!(
            write_helper_response_until(
                &mut stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("expired").unwrap(),
                    outcome: HelperOutcome::Rejected(ServiceError::Unsupported),
                },
                deadline,
            ),
            Err(ProtocolError::Io)
        );
    }

    fn valid_envelope(request: HelperRequest) -> HelperEnvelope {
        HelperEnvelope {
            version: HELPER_PROTOCOL_VERSION,
            request_id: RequestId::new("helper-req-1").unwrap(),
            request,
        }
    }

    #[test]
    fn versioned_typed_request_round_trips_and_rejects_unknown_command_fields() {
        let envelope = valid_envelope(HelperRequest::ApplyPackageTransaction {
            transaction_id: "txn-1".into(),
            manifest_id: "manifest-1".into(),
            packages: vec![PackageId::new("firefox").unwrap()],
        });
        let mut json = serde_json::to_vec(&envelope).unwrap();
        json.push(b'\n');
        assert_eq!(read_helper_envelope(&mut json.as_slice()).unwrap(), envelope);

        let arbitrary_command = br#"{"version":1,"request_id":"r1","request":{"operation":"apply_package_transaction","transaction_id":"txn-1","manifest_id":"m1","packages":["firefox"],"command":"sh -c id"}}
"#;
        assert!(matches!(
            read_helper_envelope(&mut arbitrary_command.as_slice()),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn malformed_versions_references_and_package_lists_fail_before_dispatch() {
        let mut wrong_version = valid_envelope(HelperRequest::VerifyLinkedCheckpoint {
            transaction_id: "txn-1".into(),
            manifest_id: "manifest-1".into(),
        });
        wrong_version.version = 2;
        assert_eq!(wrong_version.validate(), Err(ProtocolError::UnsupportedVersion(2)));

        let malformed = valid_envelope(HelperRequest::RestoreCheckpoint {
            transaction_id: "../escape".into(),
            manifest_id: "manifest-1".into(),
            checkpoint_id: CheckpointId::new("snap-1").unwrap(),
        });
        assert!(matches!(malformed.validate(), Err(ProtocolError::Malformed(_))));

        let duplicate = valid_envelope(HelperRequest::ApplyPackageTransaction {
            transaction_id: "txn-1".into(),
            manifest_id: "manifest-1".into(),
            packages: vec![PackageId::new("firefox").unwrap(); 2],
        });
        assert!(matches!(duplicate.validate(), Err(ProtocolError::Malformed(_))));
    }

    #[test]
    fn authenticated_and_unauthorized_peers_receive_only_fail_closed_outcomes() {
        let (service_side, _peer_side) = UnixStream::pair().unwrap();
        let peer = PeerIdentity::from_unix_socket(&service_side).unwrap();
        let envelope = valid_envelope(HelperRequest::RestoreCheckpoint {
            transaction_id: "txn-1".into(),
            manifest_id: "manifest-1".into(),
            checkpoint_id: CheckpointId::new("snap-1").unwrap(),
        });
        let allowed = dispatch_helper_request(&peer, peer.uid(), envelope.clone()).unwrap();
        assert_eq!(allowed.outcome, HelperOutcome::Rejected(ServiceError::Unsupported));
        let denied = dispatch_helper_request(&peer, u32::MAX, envelope).unwrap();
        assert_eq!(denied.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
    }

    #[test]
    fn operator_socket_requires_opt_in_group_and_keeps_valid_requests_denied() {
        const OPERATOR_GID: u32 = 4242;
        let request = valid_envelope(HelperRequest::CheckAvailability {
            action: ManagedActionKind::InstallPackage,
        });
        let root_peer =
            PeerIdentity { uid: 0, gid: 0, supplementary_gids: vec![OPERATOR_GID], pid: None };
        let ordinary_peer =
            PeerIdentity { uid: 1000, gid: 1000, supplementary_gids: vec![1001], pid: None };
        let operator_peer = PeerIdentity {
            uid: 1000,
            gid: 1000,
            supplementary_gids: vec![OPERATOR_GID],
            pid: None,
        };
        let primary_group_peer = PeerIdentity { gid: OPERATOR_GID, ..ordinary_peer.clone() };

        assert_eq!(
            authorize_helper_operator(&root_peer, OPERATOR_GID),
            Err(ServiceError::Unauthorized)
        );
        assert_eq!(
            authorize_helper_operator(&ordinary_peer, OPERATOR_GID),
            Err(ServiceError::Unauthorized)
        );
        assert_eq!(authorize_helper_operator(&operator_peer, OPERATOR_GID), Ok(()));
        assert_eq!(authorize_helper_operator(&primary_group_peer, OPERATOR_GID), Ok(()));

        assert_eq!(
            dispatch_operator_request(root_peer, OPERATOR_GID, request.clone()).unwrap().outcome,
            HelperOutcome::Rejected(ServiceError::Unauthorized)
        );
        assert_eq!(
            dispatch_operator_request(ordinary_peer, OPERATOR_GID, request.clone())
                .unwrap()
                .outcome,
            HelperOutcome::Rejected(ServiceError::Unauthorized)
        );
        assert_eq!(
            dispatch_operator_request(operator_peer.clone(), OPERATOR_GID, request.clone())
                .unwrap()
                .outcome,
            HelperOutcome::Rejected(ServiceError::Unsupported)
        );

        let mutation_requests = [
            HelperRequest::VerifyLinkedCheckpoint {
                transaction_id: "txn-1".into(),
                manifest_id: "manifest-1".into(),
            },
            HelperRequest::ApplyPackageTransaction {
                transaction_id: "txn-1".into(),
                manifest_id: "manifest-1".into(),
                packages: vec![PackageId::new("firefox").unwrap()],
            },
            HelperRequest::RestoreCheckpoint {
                transaction_id: "txn-1".into(),
                manifest_id: "manifest-1".into(),
                checkpoint_id: CheckpointId::new("snap-1").unwrap(),
            },
        ];
        for request in mutation_requests {
            assert_eq!(
                dispatch_operator_request(
                    operator_peer.clone(),
                    OPERATOR_GID,
                    HelperEnvelope {
                        request,
                        ..valid_envelope(HelperRequest::CheckAvailability {
                            action: ManagedActionKind::CreateCheckpoint,
                        })
                    },
                )
                .unwrap()
                .outcome,
                HelperOutcome::Rejected(ServiceError::Unsupported)
            );
        }
    }

    #[test]
    fn connected_peer_groups_match_kernel_process_group_snapshot() {
        let (server, client) = UnixStream::pair().unwrap();
        let peer = PeerIdentity::from_unix_socket(&server).unwrap();
        let expected = rustix::process::getgroups()
            .unwrap()
            .into_iter()
            .map(|gid| gid.as_raw())
            .collect::<Vec<_>>();
        assert_eq!(peer.supplementary_gids, expected);
        drop(client);
    }

    #[test]
    fn operator_group_database_requires_one_valid_local_entry() {
        assert_eq!(
            crate::parse_operator_group_gid("root:x:0:\nryoku-operators:x:4242:\n"),
            Ok(4242)
        );
        assert!(matches!(
            crate::parse_operator_group_gid("root:x:0:\n"),
            Err(ProtocolError::Malformed(_))
        ));
        assert!(matches!(
            crate::parse_operator_group_gid("ryoku-operators:x:bad:\n"),
            Err(ProtocolError::Malformed(_))
        ));
        assert!(matches!(
            crate::parse_operator_group_gid("ryoku-operators:x:4242:\nryoku-operators:x:4243:\n"),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn operator_socket_returns_malformed_rejection_without_dispatching() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"not-json\n").unwrap();
        let operator_gid = rustix::process::getegid().as_raw();
        serve_operator_helper_connection(server, operator_gid).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert!(matches!(response.outcome, HelperOutcome::Rejected(ServiceError::Malformed(_))));
    }

    #[test]
    fn obsolete_protocol_version_gets_structured_unsupported_rejection() {
        let (server, mut client) = UnixStream::pair().unwrap();
        let mut envelope = valid_envelope(HelperRequest::CheckAvailability {
            action: ManagedActionKind::InstallPackage,
        });
        envelope.version = HELPER_PROTOCOL_VERSION + 1;
        send_envelope(&mut client, &envelope);
        let operator_gid = rustix::process::getegid().as_raw();
        serve_operator_helper_connection(server, operator_gid).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unsupported));
    }

    #[test]
    fn availability_request_has_no_arbitrary_command_or_path_fields() {
        let envelope = valid_envelope(HelperRequest::CheckAvailability {
            action: ManagedActionKind::RestoreCheckpoint,
        });
        let mut bytes = serde_json::to_vec(&envelope).unwrap();
        bytes.push(b'\n');
        assert_eq!(read_helper_envelope(&mut bytes.as_slice()).unwrap(), envelope);
        let with_command = br#"{"version":1,"request_id":"r1","request":{"operation":"check_availability","action":"install_package","command":"id","path":"/etc"}}
"#;
        assert!(matches!(
            read_helper_envelope(&mut with_command.as_slice()),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn unavailable_operator_socket_permission_is_a_structured_unauthorized_error() {
        use std::os::unix::fs::PermissionsExt;

        if rustix::process::geteuid().as_raw() == 0 {
            return;
        }
        let path =
            std::env::temp_dir().join(format!("ryoku-helper-denied-{}.sock", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(
            check_action_availability_at(&path, ManagedActionKind::InstallPackage),
            ServiceError::Unauthorized
        );
        drop(listener);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn frame_limit_is_enforced() {
        let mut input = vec![b'x'; MAX_HELPER_REQUEST_BYTES + 2];
        *input.last_mut().unwrap() = b'\n';
        assert_eq!(
            read_helper_envelope(&mut BufReader::new(input.as_slice())),
            Err(ProtocolError::TooLarge)
        );
    }

    fn socket_request(request_id: &str) -> HelperEnvelope {
        HelperEnvelope {
            version: HELPER_PROTOCOL_VERSION,
            request_id: RequestId::new(request_id).unwrap(),
            request: HelperRequest::VerifyLinkedCheckpoint {
                transaction_id: "txn-1".into(),
                manifest_id: "manifest-1".into(),
            },
        }
    }

    fn send_envelope(stream: &mut UnixStream, envelope: &HelperEnvelope) {
        serde_json::to_writer(&mut *stream, envelope).unwrap();
        stream.write_all(b"\n").unwrap();
    }

    fn receive_response(stream: &mut UnixStream) -> HelperResponse {
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn socket_handler_authenticates_then_writes_only_typed_rejections() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"not-json\n").unwrap();
        serve_helper_connection(server, u32::MAX).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));

        let (server, mut client) = UnixStream::pair().unwrap();
        let peer = PeerIdentity::from_unix_socket(&client).unwrap();
        send_envelope(&mut client, &socket_request("known-req"));
        serve_helper_connection(server, peer.uid()).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "known-req");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unsupported));
    }

    #[test]
    fn operator_socket_rejects_before_parsing_untrusted_request() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"not-json\n").unwrap();
        serve_operator_helper_connection(server, u32::MAX).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
    }

    #[test]
    fn socket_handler_returns_typed_framing_error_and_handles_peer_close() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"not-json\n").unwrap();
        serve_helper_connection(server, rustix::process::geteuid().as_raw()).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert!(matches!(response.outcome, HelperOutcome::Rejected(ServiceError::Malformed(_))));

        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"{\"version\":1").unwrap();
        drop(client);
        assert_eq!(
            serve_helper_connection(server, rustix::process::geteuid().as_raw()),
            Err(ProtocolError::Malformed("incomplete helper request frame"))
        );
    }
}
