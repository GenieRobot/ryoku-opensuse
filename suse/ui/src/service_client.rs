//! Client for user service discovery, local profiles, and operation queries.
//!
//! This adapter is a proof against the current Unix-socket service runtime,
//! not a selection of the final UI transport. It never sends privileged
//! managed actions; local profile preferences stay in the authenticated user
//! daemon and do not invoke the helper.

use std::{
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::sockopt::socket_error,
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with},
    process::geteuid,
};
use ryoku_service::{
    Discovery, Envelope, OperationId, OperationStatus,
    PROTOCOL_VERSION as SERVICE_PROTOCOL_VERSION, PeerIdentity, Request,
    RequestId as ServiceRequestId, Response, ResponseBody, ServiceError, ServiceState,
};

use crate::{
    ActionResult, BackendError, PROTOCOL_VERSION, RequestId, UiBackend, UiRequest, UiResponse,
    UpdateState,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Read-only UI adapter that queries service state through its fixed runtime
/// socket. Construct it from `XDG_RUNTIME_DIR`; callers cannot supply a socket
/// path from UI data or a request.
#[derive(Clone, Debug)]
pub struct UnixServiceBackend {
    socket_path: PathBuf,
}

impl UnixServiceBackend {
    pub fn from_runtime_dir(runtime_dir: &Path) -> Self {
        Self { socket_path: runtime_dir.join("ryoku-service.sock") }
    }

    /// Resolve the user's runtime socket from the standard environment.
    pub fn from_environment() -> Result<Self, BackendError> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or(BackendError::Unavailable)?;
        let runtime = PathBuf::from(runtime);
        if !runtime.is_absolute() {
            return Err(BackendError::Unavailable);
        }
        Ok(Self::from_runtime_dir(&runtime))
    }

    fn discover(&self, request_id: RequestId) -> Result<UpdateState, BackendError> {
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut stream = self.connect_until(deadline)?;
        Self::exchange_until(&mut stream, request_id, deadline)
    }

    /// Query durable operation state by its service-issued identity. This is
    /// read-only and never resubmits or restarts the associated action.
    pub fn get_operation_status(
        &self,
        request_id: RequestId,
        operation_id: OperationId,
    ) -> Result<OperationStatus, BackendError> {
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut stream = self.connect_until(deadline)?;
        let expected_operation_id = operation_id.clone();
        operation_status_from_body(
            Self::exchange_body_until(
                &mut stream,
                request_id,
                Request::GetOperation { operation_id },
                deadline,
            )?,
            &expected_operation_id,
        )
    }

    /// Read the local typed profile. An absent file is returned as the
    /// built-in generic profile defaults by the service.
    pub fn get_local_profile(
        &self,
        request_id: RequestId,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        self.exchange_local_profile(request_id, Request::GetLocalProfile)
    }

    /// Submit a typed user-profile update request. The service currently
    /// returns Unsupported until the linked checkpoint coordinator is wired;
    /// this client method does not write the profile directly.
    pub fn set_local_profile(
        &self,
        request_id: RequestId,
        settings: ryoku_profiles::SettingsLayer,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        self.exchange_local_profile(request_id, Request::SetLocalProfile { settings })
    }

    /// Request migration of a recognized old Rust profile. The service
    /// currently returns Unsupported until the verified linked-checkpoint
    /// coordinator is connected; this client never writes either file itself.
    pub fn migrate_legacy_local_profile(
        &self,
        request_id: RequestId,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        self.exchange_local_profile(request_id, Request::MigrateLegacyLocalProfile)
    }

    fn exchange_local_profile(
        &self,
        request_id: RequestId,
        request: Request,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        let deadline = Instant::now() + IO_TIMEOUT;
        let mut stream = self.connect_until(deadline)?;
        Self::exchange_local_profile_until(&mut stream, request_id, request, deadline)
    }

    fn exchange_local_profile_until(
        stream: &mut UnixStream,
        request_id: RequestId,
        request: Request,
        deadline: Instant,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        local_profile_from_body(Self::exchange_body_until(stream, request_id, request, deadline)?)
    }

    fn _profile_response_type_anchor() {}

    fn connect_until(&self, deadline: Instant) -> Result<UnixStream, BackendError> {
        let runtime = self.socket_path.parent().ok_or(BackendError::Unavailable)?;
        let runtime_metadata =
            std::fs::symlink_metadata(runtime).map_err(|_| BackendError::Unavailable)?;
        if !runtime_metadata.file_type().is_dir()
            || runtime_metadata.uid() == 0
            || runtime_metadata.uid() != geteuid().as_raw()
            || runtime_metadata.mode() & 0o022 != 0
        {
            return Err(BackendError::Unauthorized);
        }
        let socket_metadata =
            std::fs::symlink_metadata(&self.socket_path).map_err(|_| BackendError::Unavailable)?;
        if !socket_metadata.file_type().is_socket()
            || socket_metadata.uid() != runtime_metadata.uid()
            || socket_metadata.mode() & 0o077 != 0
        {
            return Err(BackendError::Unauthorized);
        }
        let stream = connect_until(&self.socket_path, deadline)?;
        let peer =
            PeerIdentity::from_unix_socket(&stream).map_err(|_| BackendError::Unauthorized)?;
        if peer.uid() != runtime_metadata.uid() {
            return Err(BackendError::Unauthorized);
        }
        Ok(stream)
    }

    #[cfg(test)]
    fn exchange(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
    ) -> Result<UpdateState, BackendError> {
        Self::exchange_with_timeout(stream, ui_request_id, IO_TIMEOUT)
    }

    #[cfg(test)]
    fn exchange_with_timeout(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
        timeout: Duration,
    ) -> Result<UpdateState, BackendError> {
        Self::exchange_until(stream, ui_request_id, Instant::now() + timeout)
    }

    fn exchange_until(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
        deadline: Instant,
    ) -> Result<UpdateState, BackendError> {
        match Self::exchange_body_until(stream, ui_request_id, Request::Discover, deadline)? {
            ResponseBody::Discovery(Discovery { protocol_version, service_state, .. }) => {
                if protocol_version != SERVICE_PROTOCOL_VERSION {
                    return Err(BackendError::UnsupportedProtocol);
                }
                Ok(match service_state {
                    ServiceState::Ready => UpdateState::ServiceReady,
                    ServiceState::RecoveryUnavailable => UpdateState::RecoveryUnavailable,
                })
            }
            ResponseBody::Error(error) => Err(service_error_to_backend(error)),
            _ => Err(BackendError::InvalidResponse),
        }
    }

    fn exchange_body_until(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
        request: Request,
        deadline: Instant,
    ) -> Result<ResponseBody, BackendError> {
        stream.set_nonblocking(true).map_err(|_| BackendError::Unavailable)?;
        let request_id = ServiceRequestId::new(format!("ui-{}", ui_request_id.0))
            .map_err(|_| BackendError::InvalidResponse)?;
        let envelope =
            Envelope { version: SERVICE_PROTOCOL_VERSION, request_id: request_id.clone(), request };
        let mut frame = serde_json::to_vec(&envelope).map_err(|_| BackendError::InvalidResponse)?;
        frame.push(b'\n');
        let mut pending = frame.as_slice();
        while !pending.is_empty() {
            wait_ready(stream, PollFlags::OUT, deadline)?;
            match stream.write(pending) {
                Ok(0) => return Err(BackendError::Unavailable),
                Ok(count) => pending = &pending[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => return Err(BackendError::Unavailable),
            }
        }

        let mut bytes = Vec::with_capacity(1024);
        let mut buffer = [0_u8; 1024];
        loop {
            wait_ready(stream, PollFlags::IN, deadline)?;
            let count = match stream.read(&mut buffer) {
                Ok(0) => return Err(BackendError::InvalidResponse),
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => return Err(BackendError::Unavailable),
            };
            let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
            let end = newline.map_or(count, |index| index + 1);
            if bytes.len() + end > MAX_RESPONSE_BYTES + 1
                || newline.is_some_and(|index| index + 1 != count)
            {
                return Err(BackendError::InvalidResponse);
            }
            bytes.extend_from_slice(&buffer[..end]);
            if newline.is_some() {
                break;
            }
        }
        decode_response_frame(&bytes, &request_id)
    }
}

fn local_profile_from_body(
    body: ResponseBody,
) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
    match body {
        ResponseBody::LocalProfile(document)
            if document.id.as_str() == ryoku_profiles::GENERIC_PROFILE_ID
                && document.canonical_bytes().is_ok() =>
        {
            Ok(document)
        }
        ResponseBody::LocalProfile(_) => Err(BackendError::InvalidResponse),
        ResponseBody::Error(error) => Err(service_error_to_backend(error)),
        _ => Err(BackendError::InvalidResponse),
    }
}

fn wait_ready(
    stream: &UnixStream,
    interest: PollFlags,
    deadline: Instant,
) -> Result<(), BackendError> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(BackendError::Unavailable)?;
        let timeout = Timespec::try_from(remaining).map_err(|_| BackendError::Unavailable)?;
        let mut descriptor = [PollFd::new(stream, interest)];
        match poll(&mut descriptor, Some(&timeout)) {
            Ok(0) => return Err(BackendError::Unavailable),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(_) => return Err(BackendError::Unavailable),
        }
    }
}

fn connect_until(path: &Path, deadline: Instant) -> Result<UnixStream, BackendError> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC.union(SocketFlags::NONBLOCK),
        None,
    )
    .map_err(|_| BackendError::Unavailable)?;
    let address = SocketAddrUnix::new(path).map_err(|_| BackendError::Unavailable)?;
    match connect(&socket, &address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS | Errno::AGAIN) => loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or(BackendError::Unavailable)?;
            let timeout = Timespec::try_from(remaining).map_err(|_| BackendError::Unavailable)?;
            let mut descriptor = [PollFd::new(&socket, PollFlags::OUT)];
            match poll(&mut descriptor, Some(&timeout)) {
                Ok(0) => return Err(BackendError::Unavailable),
                Ok(_) => match socket_error(&socket).map_err(|_| BackendError::Unavailable)? {
                    Ok(()) => break,
                    Err(_) => return Err(BackendError::Unavailable),
                },
                Err(Errno::INTR) => continue,
                Err(_) => return Err(BackendError::Unavailable),
            }
        },
        Err(_) => return Err(BackendError::Unavailable),
    }
    if Instant::now() >= deadline {
        return Err(BackendError::Unavailable);
    }
    Ok(UnixStream::from(socket))
}

fn decode_response_body(
    bytes: &[u8],
    expected_request_id: &ServiceRequestId,
) -> Result<ResponseBody, BackendError> {
    let response: Response =
        serde_json::from_slice(bytes).map_err(|_| BackendError::InvalidResponse)?;
    if response.version != SERVICE_PROTOCOL_VERSION {
        return Err(BackendError::UnsupportedProtocol);
    }
    if &response.request_id != expected_request_id {
        return Err(BackendError::MismatchedRequestId);
    }
    Ok(response.body)
}

fn decode_response_frame(
    frame: &[u8],
    expected_request_id: &ServiceRequestId,
) -> Result<ResponseBody, BackendError> {
    let json = frame.strip_suffix(b"\n").ok_or(BackendError::InvalidResponse)?;
    if json.contains(&b'\n') || json.contains(&b'\r') {
        return Err(BackendError::InvalidResponse);
    }
    decode_response_body(json, expected_request_id)
}

fn operation_status_from_body(
    body: ResponseBody,
    expected_operation_id: &OperationId,
) -> Result<OperationStatus, BackendError> {
    match body {
        ResponseBody::Operation(status)
            if &status.operation_id == expected_operation_id && status.is_valid() =>
        {
            Ok(status)
        }
        ResponseBody::Operation(_) => Err(BackendError::InvalidResponse),
        ResponseBody::Error(error) => Err(service_error_to_backend(error)),
        _ => Err(BackendError::InvalidResponse),
    }
}

fn service_error_to_backend(error: ServiceError) -> BackendError {
    match error {
        ServiceError::Unsupported => BackendError::UnsupportedOperation,
        ServiceError::Unauthorized => BackendError::Unauthorized,
        ServiceError::Malformed(_) => BackendError::MalformedRequest,
        ServiceError::StaleRequest => BackendError::StaleRequest,
        ServiceError::RecoveryUnavailable => BackendError::RecoveryUnavailable,
        ServiceError::Busy => BackendError::Busy,
        ServiceError::NotFound => BackendError::NotFound,
        ServiceError::Failed => BackendError::Failed,
    }
}

impl UiBackend for UnixServiceBackend {
    fn handle(&mut self, request: UiRequest) -> UiResponse {
        let outcome = match request.action {
            crate::UserAction::RefreshStatus => {
                self.discover(request.request_id).map(ActionResult::StatusRefreshed)
            }
        };
        UiResponse { protocol_version: PROTOCOL_VERSION, request_id: request.request_id, outcome }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        thread,
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    use ryoku_service::{
        Envelope, ManagedActionKind, OperationId, OperationPhase, OperationStatus,
        PROTOCOL_VERSION as SERVICE_PROTOCOL_VERSION, Progress, Request,
        RequestId as ServiceRequestId, Response, ResponseBody, ServiceError, ServiceState,
        discovery,
    };

    use crate::{BackendError, RequestId, UiRequest, UpdateState, UserAction, dispatch};

    use super::{IO_TIMEOUT, MAX_RESPONSE_BYTES, UnixServiceBackend};

    fn unix_stream_writes_are_available() -> bool {
        let (mut client, _server) = UnixStream::pair().unwrap();
        match client.write(b"probe") {
            Ok(5) => true,
            Ok(_) => panic!("short probe write to UnixStream pair"),
            Err(error) if error.raw_os_error() == Some(1) => {
                eprintln!("skipping UnixStream transport assertion: write denied with EPERM");
                false
            }
            Err(error) => panic!("UnixStream write probe failed: {error}"),
        }
    }

    fn mock_service(
        state: ServiceState,
        request_error: Option<ServiceError>,
    ) -> (UnixStream, thread::JoinHandle<()>) {
        mock_service_with(state, request_error, SERVICE_PROTOCOL_VERSION, None)
    }

    fn mock_service_with(
        state: ServiceState,
        request_error: Option<ServiceError>,
        response_version: u16,
        response_request_id: Option<ServiceRequestId>,
    ) -> (UnixStream, thread::JoinHandle<()>) {
        let (server, client) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Envelope = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(request.version, SERVICE_PROTOCOL_VERSION);
            assert_eq!(request.request, Request::Discover);
            let body = request_error.map_or_else(
                || {
                    let mut result = discovery(matches!(state, ServiceState::Ready));
                    result.service_state = state;
                    ResponseBody::Discovery(result)
                },
                ResponseBody::Error,
            );
            let response = Response {
                version: response_version,
                request_id: response_request_id.unwrap_or(request.request_id),
                body,
            };
            serde_json::to_writer(&mut &server, &response).unwrap();
            (&server).write_all(b"\n").unwrap();
            server.shutdown(std::net::Shutdown::Write).unwrap();
        });
        (client, worker)
    }

    #[test]
    fn discovery_maps_typed_service_states_into_the_ui_contract() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut client, worker) = mock_service(ServiceState::RecoveryUnavailable, None);
        let result = UnixServiceBackend::exchange(&mut client, RequestId(17));
        worker.join().unwrap();
        assert_eq!(result, Ok(UpdateState::RecoveryUnavailable));

        let (mut client, worker) = mock_service(ServiceState::Ready, None);
        let result = UnixServiceBackend::exchange(&mut client, RequestId(17));
        worker.join().unwrap();
        assert_eq!(result, Ok(UpdateState::ServiceReady));
    }

    #[test]
    fn local_profile_update_uses_typed_user_service_request() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut client, server) = UnixStream::pair().unwrap();
        let settings = ryoku_profiles::SettingsLayer {
            desktop: Some(ryoku_profiles::Desktop::Kde),
            theme: Some(ryoku_profiles::Theme::System),
            density: Some(ryoku_profiles::Density::Compact),
        };
        let expected_settings = settings;
        let worker = thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let envelope: Envelope = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(envelope.version, SERVICE_PROTOCOL_VERSION);
            assert_eq!(envelope.request_id.as_str(), "ui-41");
            assert_eq!(envelope.request, Request::SetLocalProfile { settings: expected_settings });
            let profile = ryoku_profiles::ProfileDocument::new(
                ryoku_profiles::ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).unwrap(),
                expected_settings,
            );
            let response = Response {
                version: SERVICE_PROTOCOL_VERSION,
                request_id: envelope.request_id,
                body: ResponseBody::LocalProfile(profile),
            };
            serde_json::to_writer(&mut &server, &response).unwrap();
            (&server).write_all(b"\n").unwrap();
        });
        let result = UnixServiceBackend::exchange_local_profile_until(
            &mut client,
            RequestId(41),
            Request::SetLocalProfile { settings },
            Instant::now() + IO_TIMEOUT,
        );
        worker.join().unwrap();
        assert_eq!(result.unwrap().settings, settings);
    }

    #[test]
    fn local_profile_read_uses_typed_request_and_accepts_builtin_personal_defaults() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut client, server) = UnixStream::pair().unwrap();
        let profile = ryoku_profiles::ProfileDocument::new(
            ryoku_profiles::ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).unwrap(),
            ryoku_profiles::SettingsLayer::default(),
        );
        let expected = profile.clone();
        let worker = thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let envelope: Envelope = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(envelope.version, SERVICE_PROTOCOL_VERSION);
            assert_eq!(envelope.request_id.as_str(), "ui-42");
            assert_eq!(envelope.request, Request::GetLocalProfile);
            let response = Response {
                version: SERVICE_PROTOCOL_VERSION,
                request_id: envelope.request_id,
                body: ResponseBody::LocalProfile(expected),
            };
            serde_json::to_writer(&mut &server, &response).unwrap();
            (&server).write_all(b"\n").unwrap();
        });
        let result = UnixServiceBackend::exchange_local_profile_until(
            &mut client,
            RequestId(42),
            Request::GetLocalProfile,
            Instant::now() + IO_TIMEOUT,
        );
        worker.join().unwrap();
        assert_eq!(result, Ok(profile));
    }

    #[test]
    fn local_profile_response_rejects_unknown_schema_and_identity() {
        let document = ryoku_profiles::ProfileDocument::new(
            ryoku_profiles::ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).unwrap(),
            ryoku_profiles::SettingsLayer::default(),
        );
        let mut unsupported = document.clone();
        unsupported.schema_version += 1;
        assert_eq!(
            super::local_profile_from_body(ResponseBody::LocalProfile(unsupported)),
            Err(BackendError::InvalidResponse)
        );

        let other = ryoku_profiles::ProfileDocument::new(
            ryoku_profiles::ProfileId::new("another-profile").unwrap(),
            ryoku_profiles::SettingsLayer::default(),
        );
        assert_eq!(
            super::local_profile_from_body(ResponseBody::LocalProfile(other)),
            Err(BackendError::InvalidResponse)
        );
    }

    #[test]
    fn discovery_preserves_authentication_failure_as_a_typed_error() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut client, worker) =
            mock_service(ServiceState::Ready, Some(ServiceError::Unauthorized));
        let result = UnixServiceBackend::exchange(&mut client, RequestId(17));
        worker.join().unwrap();
        assert_eq!(result, Err(BackendError::Unauthorized));
    }

    #[test]
    fn discovery_rejects_mismatched_ids_and_unknown_service_versions() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut client, worker) = mock_service_with(
            ServiceState::Ready,
            None,
            SERVICE_PROTOCOL_VERSION,
            Some(ServiceRequestId::new("unrelated").unwrap()),
        );
        assert_eq!(
            UnixServiceBackend::exchange(&mut client, RequestId(17)),
            Err(BackendError::MismatchedRequestId)
        );
        worker.join().unwrap();

        let (mut client, worker) =
            mock_service_with(ServiceState::Ready, None, SERVICE_PROTOCOL_VERSION + 1, None);
        assert_eq!(
            UnixServiceBackend::exchange(&mut client, RequestId(17)),
            Err(BackendError::UnsupportedProtocol)
        );
        worker.join().unwrap();
    }

    #[test]
    fn discovery_rejects_oversized_response_frames() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            server.write_all(&vec![b'x'; MAX_RESPONSE_BYTES + 1]).unwrap();
        });
        assert_eq!(
            UnixServiceBackend::exchange(&mut client, RequestId(17)),
            Err(BackendError::InvalidResponse)
        );
        worker.join().unwrap();
    }

    #[test]
    fn operation_reconnect_queries_durable_status_without_resubmitting_action() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let runtime = std::env::current_dir()
            .unwrap()
            .join(format!("ryoku-ui-operation-{}-{unique}", std::process::id()));
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = runtime.join("ryoku-service.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).unwrap();
        let operation_id = OperationId::new("operation-restore-1").unwrap();
        let expected = OperationStatus {
            operation_id: operation_id.clone(),
            action: ManagedActionKind::RestoreCheckpoint,
            phase: OperationPhase::Applying,
            progress: Some(Progress { completed: 2, total: Some(3) }),
            error: None,
        };
        let worker_status = expected.clone();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Envelope = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(request.version, SERVICE_PROTOCOL_VERSION);
            assert_eq!(request.request_id.as_str(), "ui-23");
            assert_eq!(request.request, Request::GetOperation { operation_id });
            let response = Response {
                version: SERVICE_PROTOCOL_VERSION,
                request_id: request.request_id,
                body: ResponseBody::Operation(worker_status),
            };
            serde_json::to_writer(&mut &stream, &response).unwrap();
            (&stream).write_all(b"\n").unwrap();
        });
        let backend = UnixServiceBackend::from_runtime_dir(&runtime);
        assert_eq!(
            backend.get_operation_status(RequestId(23), expected.operation_id.clone()),
            Ok(expected)
        );
        worker.join().unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }

    #[test]
    fn operation_lookup_preserves_missing_identity_and_rejects_wrong_response_kind() {
        let operation_id = OperationId::new("operation-restore-1").unwrap();
        let request = Envelope {
            version: SERVICE_PROTOCOL_VERSION,
            request_id: ServiceRequestId::new("ui-23").unwrap(),
            request: Request::GetOperation { operation_id: operation_id.clone() },
        };
        let decoded: Envelope =
            serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(decoded.request, Request::GetOperation { operation_id });
        assert_eq!(
            super::operation_status_from_body(
                ResponseBody::Error(ServiceError::NotFound),
                &OperationId::new("operation-restore-1").unwrap(),
            ),
            Err(BackendError::NotFound)
        );
        assert_eq!(
            super::operation_status_from_body(
                ResponseBody::Discovery(discovery(true)),
                &OperationId::new("operation-restore-1").unwrap(),
            ),
            Err(BackendError::InvalidResponse)
        );
    }

    #[test]
    fn operation_reconnect_rejects_invalid_phase_progress_and_error_combinations() {
        let operation_id = OperationId::new("operation-restore-1").unwrap();
        let valid = OperationStatus {
            operation_id: operation_id.clone(),
            action: ManagedActionKind::RestoreCheckpoint,
            phase: OperationPhase::Applying,
            progress: Some(Progress { completed: 2, total: Some(3) }),
            error: None,
        };
        let invalid_statuses = [
            OperationStatus {
                progress: Some(Progress { completed: 4, total: Some(3) }),
                ..valid.clone()
            },
            OperationStatus {
                phase: OperationPhase::Accepted,
                progress: Some(Progress { completed: 0, total: Some(3) }),
                ..valid.clone()
            },
            OperationStatus { phase: OperationPhase::Failed, error: None, ..valid.clone() },
            OperationStatus {
                phase: OperationPhase::Applying,
                error: Some(ServiceError::Failed),
                ..valid.clone()
            },
            OperationStatus {
                phase: OperationPhase::Succeeded,
                error: None,
                progress: Some(Progress { completed: 2, total: Some(3) }),
                ..valid.clone()
            },
            OperationStatus {
                phase: OperationPhase::Succeeded,
                error: Some(ServiceError::Failed),
                ..valid
            },
        ];

        for status in invalid_statuses {
            assert!(!status.is_valid());
            assert_eq!(
                super::operation_status_from_body(ResponseBody::Operation(status), &operation_id,),
                Err(BackendError::InvalidResponse)
            );
        }
    }

    #[test]
    fn typed_service_errors_survive_without_copying_server_details() {
        for (service_error, expected) in [
            (ServiceError::Unsupported, BackendError::UnsupportedOperation),
            (ServiceError::Unauthorized, BackendError::Unauthorized),
            (ServiceError::Malformed("private detail".into()), BackendError::MalformedRequest),
            (ServiceError::StaleRequest, BackendError::StaleRequest),
            (ServiceError::RecoveryUnavailable, BackendError::RecoveryUnavailable),
            (ServiceError::Busy, BackendError::Busy),
            (ServiceError::NotFound, BackendError::NotFound),
            (ServiceError::Failed, BackendError::Failed),
        ] {
            assert_eq!(super::service_error_to_backend(service_error), expected);
        }
        let rendered = crate::render_result(&Err(BackendError::MalformedRequest));
        assert!(!rendered.contains("private detail"));
    }

    #[test]
    fn operation_reconnect_validates_correlated_versioned_status_without_socket_io() {
        let operation_id = OperationId::new("operation-restore-1").unwrap();
        let expected = OperationStatus {
            operation_id,
            action: ManagedActionKind::RestoreCheckpoint,
            phase: OperationPhase::Succeeded,
            progress: Some(Progress { completed: 3, total: Some(3) }),
            error: None,
        };
        let request_id = ServiceRequestId::new("ui-23").unwrap();
        let response = Response {
            version: SERVICE_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            body: ResponseBody::Operation(expected.clone()),
        };
        let mut bytes = serde_json::to_vec(&response).unwrap();
        bytes.push(b'\n');
        let body = super::decode_response_frame(&bytes, &request_id).unwrap();
        assert_eq!(
            super::operation_status_from_body(body, &expected.operation_id),
            Ok(expected.clone())
        );

        let unrelated = OperationStatus {
            operation_id: OperationId::new("operation-package-2").unwrap(),
            ..expected.clone()
        };
        assert_eq!(
            super::operation_status_from_body(
                ResponseBody::Operation(unrelated),
                &expected.operation_id
            ),
            Err(BackendError::InvalidResponse)
        );

        let stale_request_id = ServiceRequestId::new("ui-22").unwrap();
        assert_eq!(
            super::decode_response_frame(&bytes, &stale_request_id),
            Err(BackendError::MismatchedRequestId)
        );
        let unsupported = Response { version: SERVICE_PROTOCOL_VERSION + 1, ..response };
        assert_eq!(
            super::decode_response_frame(
                &[serde_json::to_vec(&unsupported).unwrap(), b"\n".to_vec()].concat(),
                &request_id
            ),
            Err(BackendError::UnsupportedProtocol)
        );
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(b"unexpected\n");
        assert_eq!(
            super::decode_response_frame(&trailing, &request_id),
            Err(BackendError::InvalidResponse)
        );
        assert_eq!(
            super::decode_response_frame(&bytes[..bytes.len() - 1], &request_id),
            Err(BackendError::InvalidResponse)
        );
    }

    #[test]
    fn slow_trickle_cannot_extend_the_exchange_deadline() {
        use std::time::{Duration, Instant};

        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(server.try_clone().unwrap()).read_line(&mut request).unwrap();
            assert!(!request.is_empty(), "client did not send its request");
            for _ in 0..20 {
                if server.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let started = Instant::now();
        assert_eq!(
            UnixServiceBackend::exchange_with_timeout(
                &mut client,
                RequestId(17),
                Duration::from_millis(250)
            ),
            Err(BackendError::Unavailable)
        );
        assert!(started.elapsed() < Duration::from_millis(700));
        drop(client);
        worker.join().unwrap();
    }

    #[test]
    fn gtk_backend_uses_fixed_runtime_socket_and_user_refresh_dispatch() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let runtime = std::env::current_dir()
            .unwrap()
            .join(format!("ryoku-ui-service-{}-{unique}", std::process::id()));
        fs::create_dir(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let socket_path = runtime.join("ryoku-service.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).unwrap();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Envelope = serde_json::from_str(line.trim_end()).unwrap();
            let response = Response {
                version: SERVICE_PROTOCOL_VERSION,
                request_id: request.request_id,
                body: ResponseBody::Discovery(discovery(false)),
            };
            serde_json::to_writer(&mut &stream, &response).unwrap();
            (&stream).write_all(b"\n").unwrap();
        });
        let mut backend = UnixServiceBackend::from_runtime_dir(&runtime);
        let request = UiRequest::new(RequestId(17), UserAction::RefreshStatus);
        assert_eq!(
            dispatch(&mut backend, request),
            Ok(crate::ActionResult::StatusRefreshed(UpdateState::RecoveryUnavailable))
        );
        worker.join().unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }
}
