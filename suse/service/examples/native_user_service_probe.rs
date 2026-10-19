//! Read-only native client probe. The operator starts/stops the daemon separately.
use ryoku_service::{
    Envelope, OperationId, PROTOCOL_VERSION, PeerIdentity, Request, RequestId, Response,
    ResponseBody, ServiceError, discovery,
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(5);
const MISSING_OPERATION: &str = "native-probe-missing-operation";

fn main() -> Result<(), &'static str> {
    if std::env::args_os().skip(1).collect::<Vec<_>>() != ["--run"] {
        return Err("usage: native_user_service_probe --run");
    }
    let uid = rustix::process::geteuid().as_raw();
    if uid == 0 || uid != rustix::process::getuid().as_raw() {
        return Err("probe requires an ordinary user with matching real/effective UID");
    }
    exchange(
        uid,
        "native-probe-discover",
        Request::Discover,
        ResponseBody::Discovery(discovery(false)),
    )?;
    exchange(
        uid,
        "native-probe-missing",
        Request::GetOperation {
            operation_id: OperationId::new(MISSING_OPERATION)
                .map_err(|_| "invalid fixed operation ID")?,
        },
        ResponseBody::Error(ServiceError::NotFound),
    )?;
    println!(
        "native_user_service_peer_uid=passed\nnative_user_service_discovery=passed\nnative_user_service_missing_operation=passed\nnative_user_service_read_only=passed"
    );
    Ok(())
}

fn exchange(
    uid: u32,
    id: &str,
    request: Request,
    expected: ResponseBody,
) -> Result<(), &'static str> {
    let deadline = Instant::now() + DEADLINE;
    // Nonblocking connect refuses a missing or full listener immediately.
    // There is no endpoint argument or environment-selected runtime root.
    let address = rustix::net::SocketAddrUnix::new(format!("/run/user/{uid}/ryoku-service.sock"))
        .map_err(|_| "fixed service address unavailable")?;
    let socket = rustix::net::socket_with(
        rustix::net::AddressFamily::UNIX,
        rustix::net::SocketType::STREAM,
        rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK,
        None,
    )
    .map_err(|_| "client socket unavailable")?;
    rustix::net::connect(&socket, &address).map_err(|_| "native service connection unavailable")?;
    let mut stream = UnixStream::from(socket);
    let peer =
        PeerIdentity::from_unix_socket(&stream).map_err(|_| "server identity unavailable")?;
    if peer.uid() != uid {
        return Err("server kernel UID mismatch");
    }
    stream.set_nonblocking(false).map_err(|_| "socket configuration failed")?;
    let request_id = RequestId::new(id).map_err(|_| "invalid fixed request ID")?;
    let mut bytes = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        request,
    })
    .map_err(|_| "request encoding failed")?;
    bytes.push(b'\n');
    let mut written = 0;
    while written < bytes.len() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(|_| "write deadline failed")?;
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err("service closed during request"),
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("request write failed or timed out"),
        }
    }
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    loop {
        stream.set_read_timeout(Some(remaining(deadline)?)).map_err(|_| "read deadline failed")?;
        let limit = buffer.len().min(MAX_RESPONSE_BYTES + 1 - bytes.len());
        let count = match stream.read(&mut buffer[..limit]) {
            Ok(0) => return Err("service closed before a complete response"),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("response read failed or timed out"),
        };
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err("response exceeds frame bound");
        }
        if bytes.contains(&b'\n') {
            break;
        }
    }
    check_response(&bytes, &request_id, &expected)?;
    remaining(deadline)?;
    Ok(())
}

fn remaining(deadline: Instant) -> Result<Duration, &'static str> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or("native service exchange deadline expired")
}

fn check_response(
    bytes: &[u8],
    id: &RequestId,
    expected: &ResponseBody,
) -> Result<(), &'static str> {
    if bytes.len() > MAX_RESPONSE_BYTES
        || bytes.last() != Some(&b'\n')
        || bytes[..bytes.len().saturating_sub(1)].contains(&b'\n')
    {
        return Err("invalid response frame");
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| "invalid typed response")?;
    if response.version != PROTOCOL_VERSION || response.request_id != *id {
        return Err("response protocol or correlation mismatch");
    }
    if response.body != *expected {
        return Err("unexpected read-only service response");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_stale_correlations_versions_and_unexpected_capabilities() {
        let id = RequestId::new("native-probe-discover").unwrap();
        let expected = ResponseBody::Discovery(discovery(false));
        let original =
            Response { version: PROTOCOL_VERSION, request_id: id.clone(), body: expected.clone() };
        let encode = |response: &Response| {
            let mut bytes = serde_json::to_vec(response).unwrap();
            bytes.push(b'\n');
            bytes
        };
        assert_eq!(check_response(&encode(&original), &id, &expected), Ok(()));
        for response in [
            Response { version: PROTOCOL_VERSION + 1, ..original.clone() },
            Response { request_id: RequestId::new("other").unwrap(), ..original.clone() },
            Response { body: ResponseBody::Discovery(discovery(true)), ..original.clone() },
            Response { body: ResponseBody::Error(ServiceError::Unauthorized), ..original },
        ] {
            assert!(check_response(&encode(&response), &id, &expected).is_err());
        }
    }

    #[test]
    fn rejects_truncated_multiple_oversized_frames_and_expired_deadlines() {
        let id = RequestId::new("native-probe-missing").unwrap();
        let expected = ResponseBody::Error(ServiceError::NotFound);
        let response =
            Response { version: PROTOCOL_VERSION, request_id: id.clone(), body: expected.clone() };
        let mut bytes = serde_json::to_vec(&response).unwrap();
        assert!(check_response(&bytes, &id, &expected).is_err());
        bytes.push(b'\n');
        assert_eq!(check_response(&bytes, &id, &expected), Ok(()));
        bytes.push(b'\n');
        assert!(check_response(&bytes, &id, &expected).is_err());
        assert!(check_response(&vec![b'\n'; MAX_RESPONSE_BYTES + 1], &id, &expected).is_err());
        assert!(remaining(Instant::now()).is_err());
    }
}
