//! Minimal per-user Unix socket service runtime.
//!
//! This runtime exposes discovery and durable operation lookup. Managed
//! actions are explicitly refused until an audited provider and linked
//! recovery gate are integrated.

use std::io::{BufReader, BufWriter, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{
    OperationJournal, PROTOCOL_VERSION, PeerIdentity, ProtocolError, Request, RequestId, Response,
    ResponseBody, ServiceError, authorize_helper_peer, discovery,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Serve requests until the listener is closed. Connections are processed
/// serially and each connection carries exactly one bounded request frame.
pub fn serve(
    listener: UnixListener,
    mut journal: OperationJournal,
    uid: u32,
) -> std::io::Result<()> {
    for accepted in listener.incoming() {
        let stream = accepted?;
        let _ = serve_connection(stream, &mut journal, uid);
    }
    Ok(())
}

/// Handle one connected socket. The authenticated socket UID must match this
/// unprivileged daemon's effective UID; requests never execute provider work.
pub fn serve_connection(
    stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
) -> Result<(), ProtocolError> {
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(|_| ProtocolError::Io)?;
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    let request_id = RequestId::new("invalid").expect("static request id is valid");
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| ProtocolError::Io)?);
    let envelope = match crate::read_envelope(&mut reader) {
        Ok(envelope) => envelope,
        Err(error) => {
            let body = match error {
                ProtocolError::UnsupportedVersion(_) => {
                    ResponseBody::Error(ServiceError::Unsupported)
                }
                ProtocolError::Malformed(_) | ProtocolError::TooLarge => {
                    ResponseBody::Error(ServiceError::Malformed("invalid request frame".into()))
                }
                ProtocolError::Io => return Err(ProtocolError::Io),
            };
            return write_response(
                stream,
                Response { version: PROTOCOL_VERSION, request_id, body },
            );
        }
    };
    let id = envelope.request_id;
    let body = if authorize_helper_peer(peer, uid).is_err() {
        ResponseBody::Error(ServiceError::Unauthorized)
    } else {
        match envelope.request {
            Request::Discover => ResponseBody::Discovery(discovery(false)),
            Request::GetOperation { operation_id } => journal
                .get(&operation_id)
                .cloned()
                .map(ResponseBody::Operation)
                .unwrap_or(ResponseBody::Error(ServiceError::NotFound)),
            // No executor has passed the linked capture and provider review
            // gates. Never create an accepted operation for work not run.
            Request::Start { .. } => ResponseBody::Error(ServiceError::Unsupported),
        }
    };
    write_response(stream, Response { version: PROTOCOL_VERSION, request_id: id, body })
}

fn write_response(stream: UnixStream, response: Response) -> Result<(), ProtocolError> {
    let mut writer = BufWriter::new(stream);
    serde_json::to_writer(&mut writer, &response).map_err(|_| ProtocolError::Io)?;
    writer.write_all(b"\n").map_err(|_| ProtocolError::Io)?;
    writer.flush().map_err(|_| ProtocolError::Io)
}

/// Create a private runtime socket and journal for the current unprivileged
/// account, then run the service. Requires standard XDG runtime/state roots.
pub fn run_from_environment() -> Result<(), String> {
    let uid = rustix::process::geteuid().as_raw();
    if uid == 0 {
        return Err("the user service refuses to run as root".into());
    }
    let runtime = env_path("XDG_RUNTIME_DIR")?;
    let state = env_path("XDG_STATE_HOME")?;
    validate_private_dir(&runtime, uid, false)?;
    let app_state = state.join("ryoku");
    create_private_dir(&state, uid)?;
    create_private_dir(&app_state, uid)?;
    let socket = runtime.join("ryoku-service.sock");
    remove_owned_stale_socket(&socket, uid)?;
    let listener = UnixListener::bind(&socket).map_err(|e| format!("bind service socket: {e}"))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("restrict service socket: {e}"))?;
    let journal_path = app_state.join("operations.log");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&journal_path)
        .map_err(|e| format!("open operation journal: {e}"))?;
    validate_owned_file(&journal_path, uid)?;
    let journal =
        OperationJournal::open(file).map_err(|e| format!("load operation journal: {e}"))?;
    serve(listener, journal, uid).map_err(|e| format!("service listener failed: {e}"))
}

fn env_path(name: &str) -> Result<PathBuf, String> {
    let value = std::env::var_os(name).ok_or_else(|| format!("{name} is not set"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{name} must be absolute"));
    }
    Ok(path)
}

fn create_private_dir(path: &Path, uid: u32) -> Result<(), String> {
    if !path.exists() {
        std::fs::create_dir(path)
            .map_err(|e| format!("create private directory {}: {e}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("restrict directory {}: {e}", path.display()))?;
    }
    validate_private_dir(path, uid, true)
}

fn validate_private_dir(path: &Path, uid: u32, exact_mode: bool) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("inspect directory {}: {e}", path.display()))?;
    let mode = metadata.mode() & 0o777;
    if !metadata.file_type().is_dir()
        || metadata.uid() != uid
        || mode & 0o022 != 0
        || (exact_mode && mode & 0o077 != 0)
    {
        return Err(format!("directory {} is not privately owned by this user", path.display()));
    }
    Ok(())
}

fn validate_owned_file(path: &Path, uid: u32) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| format!("inspect journal: {e}"))?;
    if !metadata.file_type().is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err("operation journal is not a private regular file owned by this user".into());
    }
    Ok(())
}

fn remove_owned_stale_socket(path: &Path, uid: u32) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspect service socket: {error}")),
        Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == uid => {
            if UnixStream::connect(path).is_ok() {
                return Err("a Ryoku service is already listening on this socket".into());
            }
            std::fs::remove_file(path).map_err(|e| format!("remove stale socket: {e}"))
        }
        Ok(_) => Err("service socket path exists and is not an owned socket".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Envelope, ManagedAction, ManagedActionKind, PackageId, Request, ResponseBody};
    use std::io::{BufRead, BufReader, Write};

    fn exchange(journal: &mut OperationJournal, request: Request) -> ResponseBody {
        let (server, client) = UnixStream::pair().unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            request_id: RequestId::new("test").unwrap(),
            request,
        };
        let mut client = client;
        serde_json::to_writer(&mut client, &envelope).unwrap();
        client.write_all(b"\n").unwrap();
        serve_connection(server, journal, uid).unwrap();
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        serde_json::from_str::<Response>(&line).unwrap().body
    }

    #[test]
    fn discovery_and_start_are_live_but_provider_actions_stay_unavailable() {
        let path = std::env::temp_dir().join(format!("ryoku-runtime-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        match exchange(&mut journal, Request::Discover) {
            ResponseBody::Discovery(discovery) => {
                assert!(discovery.supported_actions.is_empty());
                assert_eq!(discovery.service_state, crate::ServiceState::RecoveryUnavailable);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        let action = ManagedAction::InstallPackage { package: PackageId::new("firefox").unwrap() };
        assert_eq!(
            exchange(&mut journal, Request::Start { action }),
            ResponseBody::Error(ServiceError::Unsupported)
        );
        assert_eq!(journal.get(&crate::OperationId::new("not-created").unwrap()), None);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reconnect_lookup_returns_replayed_journal_state() {
        let path =
            std::env::temp_dir().join(format!("ryoku-runtime-journal-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let id = crate::OperationId::new("durable-op").unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let status = crate::OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: crate::OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        journal.record(status.clone()).unwrap();
        assert_eq!(
            exchange(&mut journal, Request::GetOperation { operation_id: id }),
            ResponseBody::Operation(status)
        );
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn wrong_peer_uid_and_malformed_frames_get_structured_errors() {
        let path = std::env::temp_dir().join(format!("ryoku-runtime-deny-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        let request = Envelope {
            version: PROTOCOL_VERSION,
            request_id: RequestId::new("wrong-peer").unwrap(),
            request: Request::Discover,
        };
        serde_json::to_writer(&mut client, &request).unwrap();
        client.write_all(b"\n").unwrap();
        assert_eq!(serve_connection(server, &mut journal, u32::MAX), Ok(()));
        let mut line = String::new();
        BufReader::new(&mut client).read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Response>(&line).unwrap().body,
            ResponseBody::Error(ServiceError::Unauthorized)
        );

        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"not-json\n").unwrap();
        serve_connection(server, &mut journal, rustix::process::geteuid().as_raw()).unwrap();
        line.clear();
        BufReader::new(&mut client).read_line(&mut line).unwrap();
        assert!(matches!(
            serde_json::from_str::<Response>(&line).unwrap().body,
            ResponseBody::Error(ServiceError::Malformed(_))
        ));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
}
