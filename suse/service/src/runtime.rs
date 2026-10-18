//! Minimal per-user Unix socket service runtime.
//!
//! This runtime exposes discovery and durable operation lookup. Managed
//! actions are explicitly refused until an audited provider and linked
//! recovery gate are integrated.

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::{
    OperationJournal, PROTOCOL_VERSION, PeerIdentity, ProtocolError, Request, RequestId, Response,
    ResponseBody, ServiceError, authorize_helper_peer, discovery,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const RESPONSE_RESERVE: Duration = Duration::from_millis(250);

/// Serve requests until the listener is closed. Connections are processed
/// serially and each connection carries exactly one bounded request frame.
fn serve(
    listener: UnixListener,
    mut journal: OperationJournal,
    paths: crate::runtime_paths::RuntimePaths,
) -> std::io::Result<()> {
    for accepted in listener.incoming() {
        let stream = accepted?;
        let _ = serve_connection_checked(
            stream,
            &mut journal,
            paths.uid(),
            Path::new(crate::helper_protocol::HELPER_SOCKET_PATH),
            || paths.revalidate().map_err(|_| ServiceError::RecoveryUnavailable),
        );
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
    serve_connection_with_helper_path(
        stream,
        journal,
        uid,
        Path::new(crate::helper_protocol::HELPER_SOCKET_PATH),
    )
}

fn serve_connection_with_helper_path(
    stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
    helper_socket_path: &Path,
) -> Result<(), ProtocolError> {
    serve_connection_checked(stream, journal, uid, helper_socket_path, || Ok(()))
}

fn serve_connection_checked(
    stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
    helper_socket_path: &Path,
    validate_paths: impl Fn() -> Result<(), ServiceError>,
) -> Result<(), ProtocolError> {
    let deadline = Instant::now() + IO_TIMEOUT;
    serve_connection_until(stream, journal, uid, helper_socket_path, validate_paths, deadline)
}

fn serve_connection_until(
    mut stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
    helper_socket_path: &Path,
    validate_paths: impl Fn() -> Result<(), ServiceError>,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    stream.set_nonblocking(true).map_err(|_| ProtocolError::Io)?;
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    if authorize_helper_peer(&peer, uid).is_err() {
        return write_response(
            &mut stream,
            Response {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("invalid").expect("static request id is valid"),
                body: ResponseBody::Error(ServiceError::Unauthorized),
            },
            deadline,
        );
    }
    // Kernel peer authentication precedes NSS/path work and request parsing.
    // Stale namespaces cannot expose replayed lookup results or probe helpers.
    if let Err(error) = validate_paths() {
        return write_response(
            &mut stream,
            Response {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("invalid").expect("static request id is valid"),
                body: ResponseBody::Error(error),
            },
            deadline,
        );
    }
    let request_id = RequestId::new("invalid").expect("static request id is valid");
    let frame = match read_frame_until(&mut stream, deadline) {
        Ok(frame) => frame,
        Err(ProtocolError::Io) => return Err(ProtocolError::Io),
        Err(error) => {
            let body = match error {
                ProtocolError::TooLarge | ProtocolError::Malformed(_) => {
                    ResponseBody::Error(ServiceError::Malformed("invalid request frame".into()))
                }
                ProtocolError::UnsupportedVersion(_) | ProtocolError::Io => unreachable!(),
            };
            return write_response(
                &mut stream,
                Response { version: PROTOCOL_VERSION, request_id, body },
                deadline,
            );
        }
    };
    let envelope = match crate::read_envelope(&mut std::io::Cursor::new(frame)) {
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
                &mut stream,
                Response { version: PROTOCOL_VERSION, request_id, body },
                deadline,
            );
        }
    };
    // Input may have taken the full framing deadline. Revalidate once more
    // immediately before dispatch rather than relying on its earlier check.
    if let Err(error) = validate_paths() {
        return write_response(
            &mut stream,
            Response { version: PROTOCOL_VERSION, request_id, body: ResponseBody::Error(error) },
            deadline,
        );
    }
    let id = envelope.request_id;
    if journal.refresh_after_write_failure().is_err() {
        return write_response(
            &mut stream,
            Response {
                version: PROTOCOL_VERSION,
                request_id: id,
                body: ResponseBody::Error(ServiceError::RecoveryUnavailable),
            },
            deadline,
        );
    }
    let body = match envelope.request {
        Request::Discover => ResponseBody::Discovery(discovery(false)),
        Request::GetOperation { operation_id } => journal
            .get(&operation_id)
            .cloned()
            .map(ResponseBody::Operation)
            .unwrap_or(ResponseBody::Error(ServiceError::NotFound)),
        Request::Start { action } => {
            ResponseBody::Error(crate::helper_protocol::check_action_availability_until(
                helper_socket_path,
                (&action).into(),
                deadline.checked_sub(RESPONSE_RESERVE).unwrap_or(deadline),
            ))
        }
    };
    write_response(
        &mut stream,
        Response { version: PROTOCOL_VERSION, request_id: id, body },
        deadline,
    )
}

fn read_frame_until(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 4096];
    loop {
        wait_ready(stream, PollFlags::IN, deadline)?;
        let count = match stream.read(&mut buffer) {
            Ok(0) => return Err(ProtocolError::Malformed("incomplete request frame")),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err(ProtocolError::Io),
        };
        let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
        let end = newline.map_or(count, |index| index + 1);
        if bytes.len() + end > crate::MAX_REQUEST_BYTES + 1 {
            return Err(ProtocolError::TooLarge);
        }
        if newline.is_some_and(|index| index + 1 != count) {
            return Err(ProtocolError::Malformed("trailing request data"));
        }
        bytes.extend_from_slice(&buffer[..end]);
        if newline.is_some() {
            return Ok(bytes);
        }
        if bytes.len() > crate::MAX_REQUEST_BYTES {
            return Err(ProtocolError::TooLarge);
        }
    }
}

fn write_response(
    stream: &mut UnixStream,
    response: Response,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    let mut bytes = serde_json::to_vec(&response).map_err(|_| ProtocolError::Io)?;
    bytes.push(b'\n');
    let mut pending = bytes.as_slice();
    while !pending.is_empty() {
        wait_ready(stream, PollFlags::OUT, deadline)?;
        match stream.write(pending) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(count) => pending = &pending[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err(ProtocolError::Io),
        }
    }
    Ok(())
}

fn wait_ready(
    stream: &UnixStream,
    interest: PollFlags,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(ProtocolError::Io)?;
        let timeout = Timespec::try_from(remaining).map_err(|_| ProtocolError::Io)?;
        let mut descriptor = [PollFd::new(stream, interest)];
        match poll(&mut descriptor, Some(&timeout)) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(_) => return Err(ProtocolError::Io),
        }
    }
}

/// Run using `/run/user/<kernel UID>` and the NSS account's fixed default
/// `.local/state/ryoku`. Environment variables cannot select a journal/socket.
pub fn run_native() -> Result<(), String> {
    let (paths, listener, file) = crate::runtime_paths::RuntimePaths::open()?;
    // Path validation happened before replay. Repeat after replay and before
    // each authenticated connection can query the in-memory operation state.
    let journal = OperationJournal::open(file).map_err(|_| "operation journal replay failed")?;
    paths.revalidate()?;
    serve(listener, journal, paths).map_err(|_| "service listener failed".into())
}

/// Compatibility entry point; all paths now come from native identity.
pub fn run_from_environment() -> Result<(), String> {
    run_native()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Envelope, ManagedAction, ManagedActionKind, PackageId, Request, ResponseBody};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn stalled_helper_returns_typed_error_within_the_original_service_deadline() {
        let path =
            std::env::temp_dir().join(format!("ryoku-shared-deadline-{}", std::process::id()));
        let socket = path.with_extension("sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let helper = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut frame = String::new();
            BufReader::new(&stream).read_line(&mut frame).unwrap();
            assert!(frame.contains("check_availability"));
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let deadline = Instant::now() + Duration::from_secs(1);
        let request = Envelope {
            version: PROTOCOL_VERSION,
            request_id: RequestId::new("stalled-helper").unwrap(),
            request: Request::Start { action: ManagedAction::CreateCheckpoint },
        };
        let client_thread = thread::spawn(move || {
            // Framing has already consumed part of the service's budget.
            thread::sleep(Duration::from_millis(100));
            serde_json::to_writer(&mut client, &request).unwrap();
            client.write_all(b"\n").unwrap();
            client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line).unwrap();
            serde_json::from_str::<Response>(&line).unwrap()
        });
        serve_connection_until(server, &mut journal, uid, &socket, || Ok(()), deadline).unwrap();
        assert!(Instant::now() < deadline);
        let response = client_thread.join().unwrap();
        assert_eq!(response.request_id.as_str(), "stalled-helper");
        assert_eq!(response.body, ResponseBody::Error(ServiceError::RecoveryUnavailable));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        release.send(()).unwrap();
        helper.join().unwrap();
        drop(journal);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(socket).unwrap();
    }

    #[test]
    fn response_reserve_exhausted_skips_helper_and_still_returns_typed_error() {
        let path =
            std::env::temp_dir().join(format!("ryoku-deadline-reserve-{}", std::process::id()));
        let socket = path.with_extension("sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            request_id: RequestId::new("late").unwrap(),
            request: Request::Start { action: ManagedAction::CreateCheckpoint },
        };
        serde_json::to_writer(&mut client, &envelope).unwrap();
        client.write_all(b"\n").unwrap();
        serve_connection_until(
            server,
            &mut journal,
            rustix::process::geteuid().as_raw(),
            &socket,
            || Ok(()),
            Instant::now() + Duration::from_millis(200),
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Response>(&line).unwrap().body,
            ResponseBody::Error(ServiceError::RecoveryUnavailable)
        );
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        drop(journal);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(socket).unwrap();
    }

    #[test]
    fn expired_socket_deadlines_fail_before_reading_or_writing() {
        let (_client, mut server) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let expired = Instant::now() - Duration::from_millis(1);
        assert_eq!(read_frame_until(&mut server, expired), Err(ProtocolError::Io));
        assert_eq!(
            write_response(
                &mut server,
                Response {
                    version: PROTOCOL_VERSION,
                    request_id: RequestId::new("expired").unwrap(),
                    body: ResponseBody::Error(ServiceError::Unsupported),
                },
                expired,
            ),
            Err(ProtocolError::Io)
        );
    }

    fn exchange(journal: &mut OperationJournal, request: Request) -> ResponseBody {
        let unavailable_socket =
            std::env::temp_dir().join(format!("ryoku-no-helper-{}.sock", std::process::id()));
        exchange_with_helper_path(journal, request, &unavailable_socket)
    }

    fn exchange_with_helper_path(
        journal: &mut OperationJournal,
        request: Request,
        helper_socket_path: &Path,
    ) -> ResponseBody {
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
        serve_connection_with_helper_path(server, journal, uid, helper_socket_path).unwrap();
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
            ResponseBody::Error(ServiceError::RecoveryUnavailable)
        );
        assert_eq!(journal.get(&crate::OperationId::new("not-created").unwrap()), None);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn user_service_probes_helper_and_rejection_does_not_write_journal() {
        let socket_path = std::env::temp_dir()
            .join(format!("ryoku-helper-integration-{}.sock", std::process::id()));
        let journal_path = std::env::temp_dir()
            .join(format!("ryoku-helper-integration-{}.log", std::process::id()));
        let listener = UnixListener::bind(&socket_path).unwrap();
        let helper_thread = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            crate::helper_protocol::serve_operator_helper_connection(
                stream,
                rustix::process::getegid().as_raw(),
            )
            .unwrap();
        });

        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&journal_path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let action = ManagedAction::InstallPackage { package: PackageId::new("firefox").unwrap() };
        let expected = if rustix::process::geteuid().as_raw() == 0 {
            ServiceError::Unauthorized
        } else {
            ServiceError::Unsupported
        };
        assert_eq!(
            exchange_with_helper_path(&mut journal, Request::Start { action }, &socket_path,),
            ResponseBody::Error(expected)
        );
        helper_thread.join().unwrap();
        assert_eq!(std::fs::metadata(&journal_path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(journal_path).unwrap();
        std::fs::remove_file(socket_path).unwrap();
    }

    #[test]
    fn reconnect_lookup_returns_state_after_journal_reopen() {
        let path =
            std::env::temp_dir().join(format!("ryoku-runtime-journal-{}", std::process::id()));
        let id = crate::OperationId::new("durable-op").unwrap();
        let status = crate::OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: crate::OperationPhase::Accepted,
            progress: None,
            error: None,
        };

        // Simulate the service stopping after a durable write, then starting
        // again and replaying the on-disk journal before a client reconnects.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        journal.record(status.clone()).unwrap();
        drop(journal);

        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let mut restarted_journal = OperationJournal::open(file).unwrap();
        assert_eq!(restarted_journal.get(&id), Some(&status));
        assert_eq!(
            exchange(&mut restarted_journal, Request::GetOperation { operation_id: id }),
            ResponseBody::Operation(status)
        );
        drop(restarted_journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn client_disconnect_does_not_cancel_durable_progress_or_restart_work() {
        let path = std::env::temp_dir().join(format!("ryoku-disconnected-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let id = crate::OperationId::new("disconnected-client").unwrap();
        let mut status = crate::OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: crate::OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        journal.record(status.clone()).unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        serde_json::to_writer(
            &mut client,
            &Envelope {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("closing").unwrap(),
                request: Request::GetOperation { operation_id: id.clone() },
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        client.shutdown(std::net::Shutdown::Both).unwrap();
        drop(client);
        // Losing the status response does not own or cancel the journal.
        assert!(
            serve_connection(server, &mut journal, rustix::process::geteuid().as_raw()).is_err()
        );
        status.phase = crate::OperationPhase::Capturing;
        status.progress = Some(crate::Progress { completed: 1, total: Some(2) });
        journal.record(status.clone()).unwrap();
        status.progress = Some(crate::Progress { completed: 2, total: Some(2) });
        journal.record(status.clone()).unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        // Duplicate submission of the accepted identity cannot reset progress.
        let reset = crate::OperationStatus {
            phase: crate::OperationPhase::Accepted,
            progress: None,
            ..status.clone()
        };
        assert_eq!(journal.record(reset), Err(crate::JournalError::InvalidTransition));
        drop(journal);
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        assert_eq!(
            exchange(&mut journal, Request::GetOperation { operation_id: id }),
            ResponseBody::Operation(status)
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reconnect_replays_uncertain_journal_before_lookup_or_fails_closed() {
        for (label, tail, expected_error) in
            [("torn", &b"{\"version\":1,"[..], false), ("corrupt", &b"not-json\n"[..], true)]
        {
            let path = std::env::temp_dir()
                .join(format!("ryoku-runtime-reconnect-{label}-{}", std::process::id()));
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut journal = OperationJournal::open(file).unwrap();
            let id = crate::OperationId::new("durable-before-error").unwrap();
            let durable = crate::OperationStatus {
                operation_id: id.clone(),
                action: ManagedActionKind::CreateCheckpoint,
                phase: crate::OperationPhase::Accepted,
                progress: None,
                error: None,
            };
            journal.record(durable.clone()).unwrap();
            let valid_len = std::fs::metadata(&path).unwrap().len();
            let mut interrupted = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            interrupted.write_all(tail).unwrap();
            interrupted.sync_all().unwrap();
            drop(interrupted);
            journal.simulate_uncertain_write_failure();

            let response = exchange(&mut journal, Request::GetOperation { operation_id: id });
            if expected_error {
                assert_eq!(response, ResponseBody::Error(ServiceError::RecoveryUnavailable));
                assert!(journal.get(&durable.operation_id).is_some());
            } else {
                assert_eq!(response, ResponseBody::Operation(durable));
                assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_len);
            }
            drop(journal);
            std::fs::remove_file(path).unwrap();
        }
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
        // A rejected UID is answered before parsing, even if its bytes are
        // malformed. The untrusted request ID is not reflected.
        client.write_all(b"not-json\n").unwrap();
        assert_eq!(serve_connection(server, &mut journal, u32::MAX), Ok(()));
        let mut line = String::new();
        BufReader::new(&mut client).read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.body, ResponseBody::Error(ServiceError::Unauthorized));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);

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

    #[test]
    fn peer_denial_precedes_path_work_and_stale_paths_precede_parsing_or_lookup() {
        let path = std::env::temp_dir().join(format!("ryoku-runtime-stale-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        for unauthorized in [true, false] {
            let (server, mut client) = UnixStream::pair().unwrap();
            client.write_all(b"not-json\n").unwrap();
            serve_connection_checked(
                server,
                &mut journal,
                if unauthorized { u32::MAX } else { rustix::process::geteuid().as_raw() },
                Path::new("/unavailable-helper"),
                || {
                    assert!(!unauthorized, "denied peer reached namespace validation");
                    Err(ServiceError::RecoveryUnavailable)
                },
            )
            .unwrap();
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line).unwrap();
            let response: Response = serde_json::from_str(&line).unwrap();
            assert_eq!(response.request_id.as_str(), "invalid");
            assert_eq!(
                response.body,
                ResponseBody::Error(if unauthorized {
                    ServiceError::Unauthorized
                } else {
                    ServiceError::RecoveryUnavailable
                })
            );
        }
        let checks = std::cell::Cell::new(0);
        let (server, mut client) = UnixStream::pair().unwrap();
        serde_json::to_writer(
            &mut client,
            &Envelope {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("must-not-reflect").unwrap(),
                request: Request::Discover,
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        serve_connection_checked(
            server,
            &mut journal,
            rustix::process::geteuid().as_raw(),
            Path::new("/unavailable-helper"),
            || {
                checks.set(checks.get() + 1);
                if checks.get() == 1 { Ok(()) } else { Err(ServiceError::RecoveryUnavailable) }
            },
        )
        .unwrap();
        assert_eq!(checks.get(), 2);
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.body, ResponseBody::Error(ServiceError::RecoveryUnavailable));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
}
