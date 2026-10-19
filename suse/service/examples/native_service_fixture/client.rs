use super::{
    OBSERVER, OPERATOR,
    support::{Result, require},
};
use ryoku_service::{
    Envelope, HelperRequest, ManagedActionKind, OperationId, PROTOCOL_VERSION, PeerIdentity,
    Request, RequestId, Response, ResponseBody, ServiceError, discovery,
    helper_protocol::{
        HELPER_PROTOCOL_VERSION, HELPER_SOCKET_PATH, HelperEnvelope, HelperOutcome, HelperResponse,
    },
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const BOUND: usize = 64 * 1024;
fn connect(path: &str) -> std::io::Result<UnixStream> {
    let address = rustix::net::SocketAddrUnix::new(path)?;
    let socket = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK,
        None,
    )?;
    rustix::net::connect(&socket, &address)?;
    let stream = UnixStream::from(socket);
    stream.set_nonblocking(false)?;
    Ok(stream)
}
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "native client deadline expired".into())
}
fn exchange(path: &str, server_uid: u32, bytes: &[u8]) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = connect(path)?;
    require(
        PeerIdentity::from_unix_socket(&stream)?.uid() == server_uid,
        "server kernel UID differs",
    )?;
    let mut offset = 0;
    while offset < bytes.len() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err("socket closed during request".into()),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !response.contains(&b'\n') {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let limit = buffer.len().min(BOUND + 1 - response.len());
        let count = match stream.read(&mut buffer[..limit]) {
            Ok(0) => return Err("incomplete native response".into()),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        response.extend_from_slice(&buffer[..count]);
        require(response.len() <= BOUND, "native response exceeds bound")?;
    }
    require(
        response.last() == Some(&b'\n') && !response[..response.len() - 1].contains(&b'\n'),
        "invalid native response frame",
    )?;
    remaining(deadline)?;
    Ok(response)
}
fn encoded(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn user_exchange(uid: u32, request: Request, expected: ResponseBody) -> Result<()> {
    let id = RequestId::new("fixture-user-readonly")?;
    let response: Response = serde_json::from_slice(&exchange(
        &format!("/run/user/{uid}/ryoku-service.sock"),
        uid,
        &encoded(&Envelope { version: PROTOCOL_VERSION, request_id: id.clone(), request })?,
    )?)?;
    require(
        response.version == PROTOCOL_VERSION
            && response.request_id == id
            && response.body == expected,
        "unexpected native user response or correlation",
    )
}
fn unauthorized_user() -> Result<()> {
    let response: Response = serde_json::from_slice(&exchange(
        &format!("/run/user/{}/ryoku-service.sock", OPERATOR.uid),
        OPERATOR.uid,
        b"",
    )?)?;
    require(
        response.version == PROTOCOL_VERSION
            && response.request_id.as_str() == "invalid"
            && response.body == ResponseBody::Error(ServiceError::Unauthorized),
        "kernel UID denial did not precede parsing",
    )
}
fn helper(expected: ServiceError, malformed: bool, correlation: &str) -> Result<()> {
    let id = RequestId::new("fixture-helper-readonly")?;
    // Denied peers receive a typed response before any request bytes. Avoid
    // writing after that deliberate immediate close (which may report EPIPE).
    let bytes = if expected == ServiceError::Unauthorized {
        Vec::new()
    } else if malformed {
        b"not-json\n".to_vec()
    } else {
        encoded(&HelperEnvelope {
            version: HELPER_PROTOCOL_VERSION,
            request_id: id,
            request: HelperRequest::CheckAvailability {
                action: ManagedActionKind::CreateCheckpoint,
            },
        })?
    };
    let response: HelperResponse =
        serde_json::from_slice(&exchange(HELPER_SOCKET_PATH, 0, &bytes)?)?;
    require(
        response.version == HELPER_PROTOCOL_VERSION
            && response.request_id.as_str() == correlation
            && response.outcome == HelperOutcome::Rejected(expected),
        "unexpected helper denial or correlation",
    )
}
fn dac(path: &str) -> Result<()> {
    require(
        matches!(connect(path), Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied),
        "native DAC did not deny connection",
    )
}
pub(super) fn run(case: &str) -> Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    require(uid == rustix::process::getuid().as_raw(), "client real/effective UID differs")?;
    match case {
        "own" if [OPERATOR.uid, OBSERVER.uid].contains(&uid) => {
            user_exchange(uid, Request::Discover, ResponseBody::Discovery(discovery(false)))?;
            user_exchange(
                uid,
                Request::GetOperation { operation_id: OperationId::new("native-fixture-missing")? },
                ResponseBody::Error(ServiceError::NotFound),
            )?;
        }
        "cross-dac" if uid == OBSERVER.uid => {
            dac(&format!("/run/user/{}/ryoku-service.sock", OPERATOR.uid))?
        }
        "cross-policy" if uid == OBSERVER.uid => unauthorized_user()?,
        "user-root" if uid == 0 => unauthorized_user()?,
        "helper-dac" if uid == OBSERVER.uid => dac(HELPER_SOCKET_PATH)?,
        "helper-operator" if uid == OPERATOR.uid => {
            helper(ServiceError::Unsupported, false, "fixture-helper-readonly")?
        }
        "helper-malformed" if uid == OPERATOR.uid => {
            helper(ServiceError::Malformed("invalid helper request".into()), true, "invalid")?
        }
        "helper-policy" if uid == OBSERVER.uid => {
            helper(ServiceError::Unauthorized, true, "invalid")?
        }
        "helper-root" if uid == 0 => helper(ServiceError::Unauthorized, true, "invalid")?,
        _ => return Err("unknown fixed client case or wrong native UID".into()),
    }
    println!("native_service_case_{case}=passed");
    Ok(())
}
