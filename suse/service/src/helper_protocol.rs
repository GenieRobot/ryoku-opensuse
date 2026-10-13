//! Versioned, typed boundary for requests from the user service to a future
//! privileged helper. It intentionally provides no command or mutation runner.

use std::io::{BufRead, Read};

use serde::{Deserialize, Serialize};

use crate::{
    HelperRequest, PeerIdentity, ProtocolError, RequestId, ServiceError, authorize_helper_peer,
};

pub const HELPER_PROTOCOL_VERSION: u16 = 1;
pub const MAX_HELPER_REQUEST_BYTES: usize = 64 * 1024;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckpointId, PackageId};
    use std::io::BufReader;
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
    fn frame_limit_is_enforced() {
        let mut input = vec![b'x'; MAX_HELPER_REQUEST_BYTES + 2];
        *input.last_mut().unwrap() = b'\n';
        assert_eq!(
            read_helper_envelope(&mut BufReader::new(input.as_slice())),
            Err(ProtocolError::TooLarge)
        );
    }
}
