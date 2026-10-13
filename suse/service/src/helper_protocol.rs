//! Versioned, typed boundary between the per-user service and root helper.
//! The installed helper currently rejects all requests and has no mutation
//! runner.

use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    HelperRequest, ManagedActionKind, PeerIdentity, ProtocolError, RequestId, ServiceError,
    authorize_helper_operator, authorize_helper_peer,
};

pub const HELPER_PROTOCOL_VERSION: u16 = 1;
pub const MAX_HELPER_REQUEST_BYTES: usize = 64 * 1024;
const HELPER_IO_TIMEOUT: Duration = Duration::from_secs(5);
pub const HELPER_SOCKET_PATH: &str = "/run/ryoku/helper.sock";

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

/// The current protocol can only reject requests. There is no success or
/// accepted response until a reviewed helper backend exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperResponse {
    pub version: u16,
    pub request_id: RequestId,
    pub outcome: HelperOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", content = "error", rename_all = "snake_case")]
pub enum HelperOutcome {
    Rejected(ServiceError),
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
    peer: PeerIdentity,
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

/// Dispatch for the installed root helper. The fixed socket's group/mode
/// gates membership; kernel peer credentials identify the user principal.
pub fn dispatch_operator_request(
    peer: PeerIdentity,
    envelope: HelperEnvelope,
) -> Result<HelperResponse, ProtocolError> {
    envelope.validate()?;
    let outcome = if authorize_helper_operator(peer.uid()).is_err() {
        HelperOutcome::Rejected(ServiceError::Unauthorized)
    } else {
        // Availability probes and every future provider request fail closed.
        HelperOutcome::Rejected(ServiceError::Unsupported)
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
    stream.set_read_timeout(Some(HELPER_IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
    stream.set_write_timeout(Some(HELPER_IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;

    // Read kernel credentials before parsing or dispatching a request. The
    // validated request ID is used only to correlate the typed rejection.
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    let envelope = match read_helper_envelope(&mut std::io::BufReader::new(&mut stream)) {
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
            return write_helper_response(
                stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("invalid").expect("static request ID is valid"),
                    outcome,
                },
            );
        }
    };

    let response = if authorize_helper_peer(peer, service_uid).is_err() {
        HelperResponse {
            version: HELPER_PROTOCOL_VERSION,
            request_id: envelope.request_id.clone(),
            outcome: HelperOutcome::Rejected(ServiceError::Unauthorized),
        }
    } else {
        dispatch_helper_request(peer, service_uid, envelope)?
    };
    write_helper_response(stream, response)
}

/// Serve one connection from the opt-in group-owned helper socket.
pub fn serve_operator_helper_connection(mut stream: UnixStream) -> Result<(), ProtocolError> {
    stream.set_read_timeout(Some(HELPER_IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
    stream.set_write_timeout(Some(HELPER_IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    let envelope = match read_helper_envelope(&mut std::io::BufReader::new(&mut stream)) {
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
            return write_helper_response(
                stream,
                HelperResponse {
                    version: HELPER_PROTOCOL_VERSION,
                    request_id: RequestId::new("invalid").expect("static request ID is valid"),
                    outcome,
                },
            );
        }
    };
    let response = dispatch_operator_request(peer, envelope)?;
    write_helper_response(stream, response)
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
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return ServiceError::Unauthorized;
        }
        Err(_) => return ServiceError::Unsupported,
    };
    let result = (|| -> Result<ServiceError, ProtocolError> {
        stream.set_read_timeout(Some(HELPER_IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
        stream.set_write_timeout(Some(HELPER_IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
        let request = HelperEnvelope {
            version: HELPER_PROTOCOL_VERSION,
            request_id: RequestId::new("availability").expect("static request ID is valid"),
            request: HelperRequest::CheckAvailability { action },
        };
        serde_json::to_writer(&mut stream, &request).map_err(|_| ProtocolError::Io)?;
        stream.write_all(b"\n").map_err(|_| ProtocolError::Io)?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).map_err(|_| ProtocolError::Io)?;
        let response: HelperResponse = serde_json::from_str(&line)
            .map_err(|_| ProtocolError::Malformed("invalid helper response"))?;
        if response.version != HELPER_PROTOCOL_VERSION || response.request_id != request.request_id
        {
            return Err(ProtocolError::Malformed("helper response correlation mismatch"));
        }
        let HelperOutcome::Rejected(error) = response.outcome;
        Ok(error)
    })();
    match result {
        Ok(error) => error,
        Err(_) => ServiceError::Unsupported,
    }
}

fn write_helper_response(
    stream: UnixStream,
    response: HelperResponse,
) -> Result<(), ProtocolError> {
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
        let allowed = dispatch_helper_request(peer, peer.uid(), envelope.clone()).unwrap();
        assert_eq!(allowed.outcome, HelperOutcome::Rejected(ServiceError::Unsupported));
        let denied = dispatch_helper_request(peer, u32::MAX, envelope).unwrap();
        assert_eq!(denied.outcome, HelperOutcome::Rejected(ServiceError::Unauthorized));
    }

    #[test]
    fn operator_socket_accepts_only_nonroot_uid_and_valid_requests_stay_rejected() {
        assert_eq!(authorize_helper_operator(0), Err(ServiceError::Unauthorized));
        assert_eq!(authorize_helper_operator(1000), Ok(()));

        let request = valid_envelope(HelperRequest::CheckAvailability {
            action: ManagedActionKind::InstallPackage,
        });
        let root_peer = PeerIdentity { uid: 0, gid: 0, pid: None };
        assert_eq!(
            dispatch_operator_request(root_peer, request.clone()).unwrap().outcome,
            HelperOutcome::Rejected(ServiceError::Unauthorized)
        );
        let user_peer = PeerIdentity { uid: 1000, gid: 1000, pid: None };
        assert_eq!(
            dispatch_operator_request(user_peer, request).unwrap().outcome,
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
                    user_peer,
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

        let (server, mut client) = UnixStream::pair().unwrap();
        let peer = PeerIdentity::from_unix_socket(&client).unwrap();
        let envelope = valid_envelope(HelperRequest::CheckAvailability {
            action: ManagedActionKind::InstallPackage,
        });
        send_envelope(&mut client, &envelope);
        serve_operator_helper_connection(server).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id, envelope.request_id);
        let expected =
            if peer.uid() == 0 { ServiceError::Unauthorized } else { ServiceError::Unsupported };
        assert_eq!(response.outcome, HelperOutcome::Rejected(expected));
        assert_eq!(peer.uid(), rustix::process::geteuid().as_raw());
    }

    #[test]
    fn operator_socket_returns_malformed_rejection_without_dispatching() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"not-json\n").unwrap();
        serve_operator_helper_connection(server).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "invalid");
        assert!(matches!(response.outcome, HelperOutcome::Rejected(ServiceError::Malformed(_))));
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
        send_envelope(&mut client, &socket_request("unauthorized-req"));
        serve_helper_connection(server, u32::MAX).unwrap();
        let response = receive_response(&mut client);
        assert_eq!(response.request_id.as_str(), "unauthorized-req");
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
