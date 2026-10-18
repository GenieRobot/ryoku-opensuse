//! Versioned, typed boundary between the per-user service and root helper.
//! The installed helper verifies exact linked checkpoints read-only; package
//! mutation and restore requests remain unavailable.

use std::io::{BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::sockopt::socket_error,
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with},
};

use serde::{Deserialize, Serialize};

use crate::account_state::{AccountStateError, AccountStateRoot};
use crate::{
    HelperRequest, ManagedActionKind, PeerIdentity, ProtocolError, RequestId, ServiceError,
    authorize_helper_operator, authorize_helper_peer,
};

pub const HELPER_PROTOCOL_VERSION: u16 = 2;
pub const MAX_HELPER_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_HELPER_RESPONSE_BYTES: usize = 64 * 1024;
const HELPER_IO_TIMEOUT: Duration = Duration::from_secs(5);
pub const HELPER_SOCKET_PATH: &str = "/run/ryoku/helper.sock";

/// Prepare the authenticated operator's pinned configuration root for a
/// forthcoming capture executor. Authorization precedes NSS/filesystem work;
/// the caller cannot select another UID or supply a home/configuration path.
/// This preflight does not enroll files, journal acceptance, or authorize a
/// mutation. Installed request dispatch remains disabled until linked capture
/// and writer coordination are implemented.
pub fn prepare_operator_account(
    peer: &PeerIdentity,
    operator_gid: u32,
) -> Result<AccountStateRoot, ServiceError> {
    with_operator_account(peer, operator_gid, AccountStateRoot::for_peer)
}

fn with_operator_account<T>(
    peer: &PeerIdentity,
    operator_gid: u32,
    resolve: impl FnOnce(&PeerIdentity) -> Result<T, AccountStateError>,
) -> Result<T, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    resolve(peer).map_err(|error| match error {
        AccountStateError::LookupBusy => ServiceError::Busy,
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

/// The only success response is read-only evidence for a verified linked
/// checkpoint. Mutation requests remain rejected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperResponse {
    pub version: u16,
    pub request_id: RequestId,
    pub outcome: HelperOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
pub enum HelperOutcome {
    VerifiedLinkedCheckpoint(VerifiedLinkedCheckpoint),
    Rejected(ServiceError),
}

/// Read-only evidence that the exact cataloged generation and Snapper partner
/// were both reopened and verified by the protected helper.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedLinkedCheckpoint {
    pub transaction_id: String,
    pub generation_id: String,
    pub manifest_sha256: String,
    pub snapper_config: String,
    pub snapshot_id: String,
    pub filesystem_uuid: String,
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

/// Evaluate the user-service helper policy. This boundary never accepts work;
/// the root helper owns the authenticated read-only checkpoint verifier.
pub fn dispatch_helper_request(
    peer: &PeerIdentity,
    service_uid: u32,
    envelope: HelperEnvelope,
) -> Result<HelperResponse, ProtocolError> {
    envelope.validate()?;
    let outcome = if authorize_helper_peer(peer, service_uid).is_err() {
        HelperOutcome::Rejected(ServiceError::Unauthorized)
    } else {
        // No request variant is executable in this build. Never return an
        // accepted response or persist a mutation that was not performed.
        HelperOutcome::Rejected(ServiceError::Unsupported)
    };
    Ok(HelperResponse {
        version: HELPER_PROTOCOL_VERSION,
        request_id: envelope.request_id,
        outcome,
    })
}

/// Dispatch without a recovery reader. This fail-closed path supports policy
/// checks and tests; the installed socket uses the protected-state reader.
pub fn dispatch_operator_request(
    peer: PeerIdentity,
    operator_gid: u32,
    envelope: HelperEnvelope,
) -> Result<HelperResponse, ProtocolError> {
    dispatch_operator_request_with_verifier(peer, operator_gid, envelope, |_, _| {
        Err(ServiceError::Unsupported)
    })
}

fn dispatch_operator_request_with_verifier(
    peer: PeerIdentity,
    operator_gid: u32,
    envelope: HelperEnvelope,
    mut verify_linked: impl FnMut(&str, &str) -> Result<VerifiedLinkedCheckpoint, ServiceError>,
) -> Result<HelperResponse, ProtocolError> {
    envelope.validate()?;
    let outcome = if authorize_helper_operator(&peer, operator_gid).is_err() {
        HelperOutcome::Rejected(ServiceError::Unauthorized)
    } else {
        match &envelope.request {
            HelperRequest::VerifyLinkedCheckpoint { transaction_id, generation_id } => {
                match verify_linked(transaction_id, generation_id) {
                    Ok(verified) => HelperOutcome::VerifiedLinkedCheckpoint(verified),
                    Err(error) => HelperOutcome::Rejected(error),
                }
            }
            // Availability probes do not advertise unavailable operations;
            // package and restore execution remain gated on M1/M2 acceptance.
            _ => HelperOutcome::Rejected(ServiceError::Unsupported),
        }
    };
    Ok(HelperResponse {
        version: HELPER_PROTOCOL_VERSION,
        request_id: envelope.request_id,
        outcome,
    })
}

/// Handle one user-service socket connection. Peer credentials are obtained
/// before request dispatch; it never accepts or persists an operation.
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
    write_helper_response_until(&mut stream, response, deadline)
}

/// Serve one connection from the opt-in group-owned helper socket.
pub fn serve_operator_helper_connection(
    stream: UnixStream,
    operator_gid: u32,
) -> Result<(), ProtocolError> {
    serve_operator_helper_connection_with_verifier(stream, operator_gid, |_, _| {
        Err(ServiceError::Unsupported)
    })
}

/// Serve authenticated helper requests against the pinned read-only recovery
/// reader. Only linked-checkpoint verification is enabled.
pub fn serve_protected_operator_helper_connection(
    stream: UnixStream,
    operator_gid: u32,
    reader: &ryoku_recovery::generation::GenerationReader,
) -> Result<(), ProtocolError> {
    serve_operator_helper_connection_with_verifier(
        stream,
        operator_gid,
        |transaction, generation| verify_linked_checkpoint(reader, transaction, generation),
    )
}

fn serve_operator_helper_connection_with_verifier(
    mut stream: UnixStream,
    operator_gid: u32,
    verify_linked: impl FnMut(&str, &str) -> Result<VerifiedLinkedCheckpoint, ServiceError>,
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
    let response =
        dispatch_operator_request_with_verifier(peer, operator_gid, envelope, verify_linked)?;
    write_helper_response_until(&mut stream, response, deadline)
}

fn verify_linked_checkpoint(
    reader: &ryoku_recovery::generation::GenerationReader,
    transaction_id: &str,
    generation_id: &str,
) -> Result<VerifiedLinkedCheckpoint, ServiceError> {
    let transaction = ryoku_recovery::Identifier::new(transaction_id.to_owned())
        .map_err(|_| ServiceError::Malformed("invalid transaction ID".into()))?;
    let expected_generation = ryoku_recovery::Identifier::new(generation_id.to_owned())
        .map_err(|_| ServiceError::Malformed("invalid generation ID".into()))?;
    let root = std::fs::File::open("/").map_err(|_| ServiceError::RecoveryUnavailable)?;
    let linked = reader
        .open_cataloged_transaction(&root, &transaction)
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    let manifest = linked.generation().manifest();
    if manifest.generation_id != expected_generation {
        return Err(ServiceError::StaleRequest);
    }
    let digest = manifest.manifest_sha256.as_ref().ok_or(ServiceError::RecoveryUnavailable)?;
    let receipt = linked.snapshot().receipt();
    let link = receipt.link();
    Ok(VerifiedLinkedCheckpoint {
        transaction_id: manifest.transaction_id.as_str().to_owned(),
        generation_id: manifest.generation_id.as_str().to_owned(),
        manifest_sha256: digest.as_str().to_owned(),
        snapper_config: link.snapper_config.as_str().to_owned(),
        snapshot_id: link.snapper_snapshot_id.clone(),
        filesystem_uuid: link.filesystem_uuid.clone(),
    })
}

/// Ask the installed helper whether a named action is available. This probe
/// has no operation identifier, payload path, or mutation semantics. Until a
/// reviewed provider is installed the only valid response is Unsupported.
pub fn check_action_availability(action: ManagedActionKind) -> ServiceError {
    check_action_availability_at(Path::new(HELPER_SOCKET_PATH), action)
}

pub(crate) fn check_action_availability_at(
    socket_path: &Path,
    action: ManagedActionKind,
) -> ServiceError {
    check_action_availability_until(socket_path, action, Instant::now() + HELPER_IO_TIMEOUT)
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
            HelperOutcome::VerifiedLinkedCheckpoint(_) => {
                Err(ProtocolError::Malformed("unexpected helper verification response"))
            }
        }
    })();
    match result {
        Ok(error) => error,
        Err(_) => ServiceError::RecoveryUnavailable,
    }
}

#[cfg(test)]
pub(crate) fn verify_linked_checkpoint_at(
    socket_path: &Path,
    transaction_id: &str,
    generation_id: &str,
) -> Result<VerifiedLinkedCheckpoint, ServiceError> {
    verify_linked_checkpoint_until(
        socket_path,
        transaction_id,
        generation_id,
        Instant::now() + HELPER_IO_TIMEOUT,
    )
}

pub(crate) fn verify_linked_checkpoint_until(
    socket_path: &Path,
    transaction_id: &str,
    generation_id: &str,
    deadline: Instant,
) -> Result<VerifiedLinkedCheckpoint, ServiceError> {
    let mut stream = connect_helper_until(socket_path, deadline)
        .map_err(|error| helper_connect_error(error.kind()))?;
    let request = HelperEnvelope {
        version: HELPER_PROTOCOL_VERSION,
        request_id: RequestId::new("verify-linked").expect("static request ID is valid"),
        request: HelperRequest::VerifyLinkedCheckpoint {
            transaction_id: transaction_id.to_owned(),
            generation_id: generation_id.to_owned(),
        },
    };
    request.validate().map_err(|_| ServiceError::Malformed("invalid recovery reference".into()))?;
    let response = exchange_helper_until(&mut stream, &request, deadline)
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    if response.version != HELPER_PROTOCOL_VERSION || response.request_id != request.request_id {
        return Err(ServiceError::RecoveryUnavailable);
    }
    match response.outcome {
        HelperOutcome::Rejected(error) => Err(error),
        HelperOutcome::VerifiedLinkedCheckpoint(verified)
            if verified.transaction_id == transaction_id
                && verified.generation_id == generation_id
                && ryoku_recovery::Sha256::new(verified.manifest_sha256.clone()).is_ok()
                && valid_uuid(&verified.filesystem_uuid)
                && ryoku_recovery::Identifier::new(verified.snapper_config.clone()).is_ok()
                && valid_snapshot_id(&verified.snapshot_id) =>
        {
            Ok(verified)
        }
        HelperOutcome::VerifiedLinkedCheckpoint(_) => Err(ServiceError::RecoveryUnavailable),
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

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn valid_snapshot_id(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|number| number > 0 && number.to_string() == value)
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

fn connect_helper_until(path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
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
fn write_helper_response(
    stream: UnixStream,
    response: HelperResponse,
) -> Result<(), ProtocolError> {
    use std::io::BufWriter;
    let mut writer = BufWriter::new(stream);
    serde_json::to_writer(&mut writer, &response).map_err(|_| ProtocolError::Io)?;
    writer.write_all(b"\n").map_err(|_| ProtocolError::Io)?;
    writer.flush().map_err(|_| ProtocolError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckpointId, ManagedActionKind, PackageId, authorize_helper_operator};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    use std::thread;

    #[test]
    fn expired_helper_deadline_fails_before_socket_io() {
        let (_peer, mut stream) = crate::test_unix_pair_or_skip!();
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

    #[test]
    fn linked_checkpoint_client_correlates_and_validates_helper_evidence() {
        let path =
            std::env::temp_dir().join(format!("ryoku-verify-helper-{}.sock", std::process::id()));
        let listener = crate::test_unix_listener_or_skip!(&path);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_helper_envelope(&mut BufReader::new(&mut stream)).unwrap();
            assert_eq!(request.request_id.as_str(), "verify-linked");
            assert_eq!(
                request.request,
                HelperRequest::VerifyLinkedCheckpoint {
                    transaction_id: "txn-1".into(),
                    generation_id: "generation-1".into(),
                }
            );
            let response = HelperResponse {
                version: HELPER_PROTOCOL_VERSION,
                request_id: request.request_id,
                outcome: HelperOutcome::VerifiedLinkedCheckpoint(VerifiedLinkedCheckpoint {
                    transaction_id: "txn-1".into(),
                    generation_id: "generation-1".into(),
                    manifest_sha256: "a".repeat(64),
                    snapper_config: "root".into(),
                    snapshot_id: "42".into(),
                    filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
                }),
            };
            write_helper_response(stream, response).unwrap();
        });
        let verified = verify_linked_checkpoint_at(&path, "txn-1", "generation-1").unwrap();
        assert_eq!(verified.snapshot_id, "42");
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn account_preflight_authorizes_before_lookup_and_binds_exact_peer() {
        const OPERATOR_GID: u32 = 4242;
        let peer = PeerIdentity {
            uid: 1000,
            gid: 1000,
            supplementary_gids: vec![OPERATOR_GID],
            pid: None,
        };
        for denied in [
            PeerIdentity { uid: 0, ..peer.clone() },
            PeerIdentity { supplementary_gids: vec![], ..peer.clone() },
        ] {
            let result = with_operator_account(&denied, OPERATOR_GID, |_| -> Result<(), _> {
                panic!("unauthorized request reached account lookup");
            });
            assert_eq!(result, Err(ServiceError::Unauthorized));
        }
        assert_eq!(
            with_operator_account(&peer, OPERATOR_GID, |authenticated| {
                assert_eq!(authenticated, &peer);
                Ok(authenticated.uid())
            }),
            Ok(1000)
        );
        for error in [
            AccountStateError::LookupTimedOut,
            AccountStateError::AccountChanged,
            AccountStateError::UnsafeDirectory,
            AccountStateError::UnsupportedAccount,
        ] {
            assert_eq!(
                with_operator_account(&peer, OPERATOR_GID, |_| Err::<(), _>(error)),
                Err(ServiceError::RecoveryUnavailable)
            );
        }
        assert_eq!(
            with_operator_account(&peer, OPERATOR_GID, |_| Err::<(), _>(
                AccountStateError::LookupBusy
            )),
            Err(ServiceError::Busy)
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
            generation_id: "generation-1".into(),
            packages: vec![PackageId::new("firefox").unwrap()],
        });
        let mut json = serde_json::to_vec(&envelope).unwrap();
        json.push(b'\n');
        assert_eq!(read_helper_envelope(&mut json.as_slice()).unwrap(), envelope);

        let arbitrary_command = br#"{"version":2,"request_id":"r1","request":{"operation":"apply_package_transaction","transaction_id":"txn-1","generation_id":"generation-1","packages":["firefox"],"command":"sh -c id"}}
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
            generation_id: "generation-1".into(),
        });
        wrong_version.version = HELPER_PROTOCOL_VERSION + 1;
        assert_eq!(
            wrong_version.validate(),
            Err(ProtocolError::UnsupportedVersion(HELPER_PROTOCOL_VERSION + 1))
        );

        let malformed = valid_envelope(HelperRequest::RestoreCheckpoint {
            transaction_id: "../escape".into(),
            generation_id: "generation-1".into(),
            checkpoint_id: CheckpointId::new("snap-1").unwrap(),
        });
        assert!(matches!(malformed.validate(), Err(ProtocolError::Malformed(_))));

        let duplicate = valid_envelope(HelperRequest::ApplyPackageTransaction {
            transaction_id: "txn-1".into(),
            generation_id: "generation-1".into(),
            packages: vec![PackageId::new("firefox").unwrap(); 2],
        });
        assert!(matches!(duplicate.validate(), Err(ProtocolError::Malformed(_))));
    }

    #[test]
    fn authenticated_and_unauthorized_peers_receive_only_fail_closed_outcomes() {
        let (service_side, _peer_side) = crate::test_unix_pair_or_skip!();
        let peer = PeerIdentity::from_unix_socket(&service_side).unwrap();
        let envelope = valid_envelope(HelperRequest::RestoreCheckpoint {
            transaction_id: "txn-1".into(),
            generation_id: "generation-1".into(),
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
                generation_id: "generation-1".into(),
            },
            HelperRequest::ApplyPackageTransaction {
                transaction_id: "txn-1".into(),
                generation_id: "generation-1".into(),
                packages: vec![PackageId::new("firefox").unwrap()],
            },
            HelperRequest::RestoreCheckpoint {
                transaction_id: "txn-1".into(),
                generation_id: "generation-1".into(),
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
    fn linked_checkpoint_verification_runs_only_for_an_authenticated_operator() {
        const OPERATOR_GID: u32 = 4242;
        let operator = PeerIdentity {
            uid: 1000,
            gid: 1000,
            supplementary_gids: vec![OPERATOR_GID],
            pid: None,
        };
        let request = valid_envelope(HelperRequest::VerifyLinkedCheckpoint {
            transaction_id: "txn-1".into(),
            generation_id: "generation-1".into(),
        });
        let mut called = false;
        let response = dispatch_operator_request_with_verifier(
            operator.clone(),
            OPERATOR_GID,
            request.clone(),
            |transaction, generation| {
                called = true;
                assert_eq!(transaction, "txn-1");
                assert_eq!(generation, "generation-1");
                Ok(VerifiedLinkedCheckpoint {
                    transaction_id: transaction.to_owned(),
                    generation_id: generation.to_owned(),
                    manifest_sha256: "0".repeat(64),
                    snapper_config: "root".into(),
                    snapshot_id: "42".into(),
                    filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
                })
            },
        )
        .unwrap();
        assert!(called);
        assert!(matches!(response.outcome, HelperOutcome::VerifiedLinkedCheckpoint(_)));

        let denied = PeerIdentity { supplementary_gids: vec![], ..operator };
        let mut unauthorized_called = false;
        let response =
            dispatch_operator_request_with_verifier(denied, OPERATOR_GID, request, |_, _| {
                unauthorized_called = true;
                panic!("unauthorized peer reached linked verification");
            })
            .unwrap();
        assert!(!unauthorized_called);
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
    }

    #[test]
    fn linked_checkpoint_success_response_has_bounded_typed_wire_shape() {
        let response = HelperResponse {
            version: HELPER_PROTOCOL_VERSION,
            request_id: RequestId::new("verify-1").unwrap(),
            outcome: HelperOutcome::VerifiedLinkedCheckpoint(VerifiedLinkedCheckpoint {
                transaction_id: "txn-1".into(),
                generation_id: "generation-1".into(),
                manifest_sha256: "0".repeat(64),
                snapper_config: "root".into(),
                snapshot_id: "42".into(),
                filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            }),
        };
        let mut frame = serde_json::to_vec(&response).unwrap();
        frame.push(b'\n');
        let decoded = read_helper_response(&mut BufReader::new(frame.as_slice())).unwrap();
        assert_eq!(decoded, response);
        assert!(frame.len() <= MAX_HELPER_RESPONSE_BYTES + 1);
    }

    #[test]
    fn connected_peer_groups_match_kernel_process_group_snapshot() {
        let (server, client) = crate::test_unix_pair_or_skip!();
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
        let (server, mut client) = crate::test_unix_pair_or_skip!();
        client.write_all(b"not-json\n").unwrap();
        let operator_gid = rustix::process::getegid().as_raw();
        serve_operator_helper_connection(server, operator_gid).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert!(matches!(response.outcome, HelperOutcome::Rejected(ServiceError::Malformed(_))));
    }

    #[test]
    fn obsolete_protocol_version_gets_structured_unsupported_rejection() {
        let (server, mut client) = crate::test_unix_pair_or_skip!();
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
        let listener = crate::test_unix_listener_or_skip!(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(
            check_action_availability_at(&path, ManagedActionKind::InstallPackage),
            ServiceError::Unauthorized
        );
        drop(listener);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_helper_and_permission_denial_have_distinct_typed_errors() {
        assert_eq!(
            helper_connect_error(std::io::ErrorKind::NotFound),
            ServiceError::RecoveryUnavailable
        );
        assert_eq!(
            helper_connect_error(std::io::ErrorKind::ConnectionRefused),
            ServiceError::RecoveryUnavailable
        );
        assert_eq!(
            helper_connect_error(std::io::ErrorKind::PermissionDenied),
            ServiceError::Unauthorized
        );
    }

    #[test]
    fn helper_response_frame_is_bounded_and_requires_a_newline() {
        let oversized = vec![b'x'; MAX_HELPER_RESPONSE_BYTES + 2];
        assert_eq!(read_helper_response(&mut oversized.as_slice()), Err(ProtocolError::TooLarge));
        assert_eq!(
            read_helper_response(&mut b"{}".as_slice()),
            Err(ProtocolError::Malformed("incomplete helper response frame"))
        );
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
                generation_id: "generation-1".into(),
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
        let (server, mut client) = crate::test_unix_pair_or_skip!();
        client.write_all(b"not-json\n").unwrap();
        serve_helper_connection(server, u32::MAX).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));

        let (server, mut client) = crate::test_unix_pair_or_skip!();
        let peer = PeerIdentity::from_unix_socket(&client).unwrap();
        send_envelope(&mut client, &socket_request("known-req"));
        serve_helper_connection(server, peer.uid()).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "known-req");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unsupported));
    }

    #[test]
    fn operator_socket_rejects_before_parsing_untrusted_request() {
        let (server, mut client) = crate::test_unix_pair_or_skip!();
        client.write_all(b"not-json\n").unwrap();
        serve_operator_helper_connection(server, u32::MAX).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
    }

    #[test]
    fn socket_handler_returns_typed_framing_error_and_handles_peer_close() {
        let (server, mut client) = crate::test_unix_pair_or_skip!();
        client.write_all(b"not-json\n").unwrap();
        serve_helper_connection(server, rustix::process::geteuid().as_raw()).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert!(matches!(response.outcome, HelperOutcome::Rejected(ServiceError::Malformed(_))));

        let (server, mut client) = crate::test_unix_pair_or_skip!();
        client.write_all(b"{\"version\":1").unwrap();
        drop(client);
        assert_eq!(
            serve_helper_connection(server, rustix::process::geteuid().as_raw()),
            Err(ProtocolError::Malformed("incomplete helper request frame"))
        );
    }
}
