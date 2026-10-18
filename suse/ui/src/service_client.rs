//! Client for service discovery and typed local preferences.
//!
//! This adapter is a proof against the current Unix-socket service runtime,
//! not a selection of the final UI transport. Personal preference requests stay
//! in the authenticated user daemon and never invoke privileged actions.

use std::{
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rustix::process::geteuid;
use ryoku_service::{
    Discovery, Envelope, PROTOCOL_VERSION as SERVICE_PROTOCOL_VERSION, PeerIdentity, Request,
    RequestId as ServiceRequestId, Response, ResponseBody, ServiceError, ServiceState,
};

use crate::{
    ActionResult, BackendError, PROTOCOL_VERSION, RequestId, UiBackend, UiRequest, UiResponse,
    UpdateState,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// UI adapter that queries service state and personal preferences through its fixed runtime
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
        let mut stream =
            UnixStream::connect(&self.socket_path).map_err(|_| BackendError::Unavailable)?;
        let peer =
            PeerIdentity::from_unix_socket(&stream).map_err(|_| BackendError::Unauthorized)?;
        if peer.uid() != runtime_metadata.uid() {
            return Err(BackendError::Unauthorized);
        }
        Self::exchange(&mut stream, request_id)
    }

    pub fn get_local_profile(
        &self,
        request_id: RequestId,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        self.profile_request(request_id, Request::GetLocalProfile)
    }
    pub fn set_local_profile(
        &self,
        request_id: RequestId,
        settings: ryoku_profiles::SettingsLayer,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        self.profile_request(request_id, Request::SetLocalProfile { settings })
    }
    pub fn migrate_legacy_local_profile(
        &self,
        request_id: RequestId,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        self.profile_request(request_id, Request::MigrateLegacyLocalProfile)
    }

    fn profile_request(
        &self,
        request_id: RequestId,
        request: Request,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
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
        let mut stream =
            UnixStream::connect(&self.socket_path).map_err(|_| BackendError::Unavailable)?;
        let peer =
            PeerIdentity::from_unix_socket(&stream).map_err(|_| BackendError::Unauthorized)?;
        if peer.uid() != runtime_metadata.uid() {
            return Err(BackendError::Unauthorized);
        }
        Self::exchange_profile(&mut stream, request_id, request)
    }

    fn exchange(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
    ) -> Result<UpdateState, BackendError> {
        Self::exchange_with_timeout(stream, ui_request_id, IO_TIMEOUT)
    }

    fn exchange_with_timeout(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
        timeout: Duration,
    ) -> Result<UpdateState, BackendError> {
        let deadline = Instant::now() + timeout;
        let remaining = || {
            deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or(BackendError::Unavailable)
        };
        let request_id = ServiceRequestId::new(format!("ui-{}", ui_request_id.0))
            .map_err(|_| BackendError::InvalidResponse)?;
        let envelope = Envelope {
            version: SERVICE_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            request: Request::Discover,
        };
        let mut frame = serde_json::to_vec(&envelope).map_err(|_| BackendError::InvalidResponse)?;
        frame.push(b'\n');
        let mut pending = frame.as_slice();
        while !pending.is_empty() {
            stream.set_write_timeout(Some(remaining()?)).map_err(|_| BackendError::Unavailable)?;
            match stream.write(pending) {
                Ok(0) => return Err(BackendError::Unavailable),
                Ok(count) => pending = &pending[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(BackendError::Unavailable),
            }
        }

        let mut bytes = Vec::with_capacity(1024);
        let mut buffer = [0_u8; 1024];
        loop {
            stream.set_read_timeout(Some(remaining()?)).map_err(|_| BackendError::Unavailable)?;
            let count = match stream.read(&mut buffer) {
                Ok(0) => return Err(BackendError::InvalidResponse),
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(BackendError::Unavailable),
            };
            let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
            let end = newline.map_or(count, |index| index + 1);
            if bytes.len() + end > MAX_RESPONSE_BYTES {
                return Err(BackendError::InvalidResponse);
            }
            bytes.extend_from_slice(&buffer[..end]);
            if newline.is_some() {
                break;
            }
        }
        bytes.pop();
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| BackendError::InvalidResponse)?;
        if response.version != SERVICE_PROTOCOL_VERSION {
            return Err(BackendError::UnsupportedProtocol);
        }
        if response.request_id != request_id {
            return Err(BackendError::MismatchedRequestId);
        }
        match response.body {
            ResponseBody::Discovery(Discovery { protocol_version, service_state, .. }) => {
                if protocol_version != SERVICE_PROTOCOL_VERSION {
                    return Err(BackendError::UnsupportedProtocol);
                }
                Ok(match service_state {
                    ServiceState::Ready => UpdateState::ServiceReady,
                    ServiceState::RecoveryUnavailable => UpdateState::RecoveryUnavailable,
                })
            }
            ResponseBody::Error(ServiceError::Unauthorized) => Err(BackendError::Unauthorized),
            ResponseBody::Error(ServiceError::Unsupported) => {
                Err(BackendError::UnsupportedOperation)
            }
            _ => Err(BackendError::InvalidResponse),
        }
    }
    fn exchange_profile(
        stream: &mut UnixStream,
        ui_request_id: RequestId,
        request: Request,
    ) -> Result<ryoku_profiles::ProfileDocument, BackendError> {
        let deadline = Instant::now() + IO_TIMEOUT;
        let remaining = || {
            deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or(BackendError::Unavailable)
        };
        let request_id = ServiceRequestId::new(format!("ui-{}", ui_request_id.0))
            .map_err(|_| BackendError::InvalidResponse)?;
        let envelope =
            Envelope { version: SERVICE_PROTOCOL_VERSION, request_id: request_id.clone(), request };
        let mut frame = serde_json::to_vec(&envelope).map_err(|_| BackendError::InvalidResponse)?;
        frame.push(b'\n');
        let mut pending = frame.as_slice();
        while !pending.is_empty() {
            stream.set_write_timeout(Some(remaining()?)).map_err(|_| BackendError::Unavailable)?;
            match stream.write(pending) {
                Ok(0) => return Err(BackendError::Unavailable),
                Ok(count) => pending = &pending[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(BackendError::Unavailable),
            }
        }

        let mut bytes = Vec::with_capacity(1024);
        let mut buffer = [0_u8; 1024];
        loop {
            stream.set_read_timeout(Some(remaining()?)).map_err(|_| BackendError::Unavailable)?;
            let count = match stream.read(&mut buffer) {
                Ok(0) => return Err(BackendError::InvalidResponse),
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(BackendError::Unavailable),
            };
            let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
            let end = newline.map_or(count, |index| index + 1);
            if bytes.len() + end > MAX_RESPONSE_BYTES {
                return Err(BackendError::InvalidResponse);
            }
            bytes.extend_from_slice(&buffer[..end]);
            if newline.is_some() {
                break;
            }
        }
        bytes.pop();
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| BackendError::InvalidResponse)?;
        if response.version != SERVICE_PROTOCOL_VERSION {
            return Err(BackendError::UnsupportedProtocol);
        }
        if response.request_id != request_id {
            return Err(BackendError::MismatchedRequestId);
        }
        match response.body {
            ResponseBody::LocalProfile(document)
                if document.id.as_str() == ryoku_profiles::GENERIC_PROFILE_ID
                    && document.canonical_bytes().is_ok() =>
            {
                Ok(document)
            }
            ResponseBody::Error(ServiceError::Unauthorized) => Err(BackendError::Unauthorized),
            ResponseBody::Error(ServiceError::NotFound) => Err(BackendError::NotFound),
            ResponseBody::Error(ServiceError::Unsupported) => {
                Err(BackendError::UnsupportedOperation)
            }
            ResponseBody::Error(ServiceError::StaleRequest) => Err(BackendError::StaleRequest),
            ResponseBody::Error(ServiceError::RecoveryUnavailable) => {
                Err(BackendError::RecoveryUnavailable)
            }
            ResponseBody::Error(ServiceError::Busy) => Err(BackendError::Busy),
            ResponseBody::Error(ServiceError::Failed) => Err(BackendError::Failed),
            ResponseBody::Error(ServiceError::Malformed(_)) => Err(BackendError::MalformedRequest),
            _ => Err(BackendError::InvalidResponse),
        }
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
        time::{SystemTime, UNIX_EPOCH},
    };

    use ryoku_service::{
        Envelope, PROTOCOL_VERSION as SERVICE_PROTOCOL_VERSION, Request,
        RequestId as ServiceRequestId, Response, ResponseBody, ServiceError, ServiceState,
        discovery,
    };

    use crate::{BackendError, RequestId, UiRequest, UpdateState, UserAction, dispatch};

    use super::{MAX_RESPONSE_BYTES, UnixServiceBackend};

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
    fn discovery_preserves_authentication_failure_as_a_typed_error() {
        let (mut client, worker) =
            mock_service(ServiceState::Ready, Some(ServiceError::Unauthorized));
        let result = UnixServiceBackend::exchange(&mut client, RequestId(17));
        worker.join().unwrap();
        assert_eq!(result, Err(BackendError::Unauthorized));
    }

    #[test]
    fn discovery_rejects_mismatched_ids_and_unknown_service_versions() {
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
    fn slow_trickle_cannot_extend_the_exchange_deadline() {
        use std::time::{Duration, Instant};

        let (mut server, mut client) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(server.try_clone().unwrap()).read_line(&mut request).unwrap();
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
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let runtime =
            std::env::temp_dir().join(format!("ryoku-ui-service-{}-{unique}", std::process::id()));
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
    #[test]
    fn local_profile_exchange_sends_typed_request_and_validates_response() {
        use std::io::{BufRead, BufReader};
        let (server, mut client) = UnixStream::pair().unwrap();
        let settings = ryoku_profiles::SettingsLayer {
            theme: Some(ryoku_profiles::Theme::System),
            ..Default::default()
        };
        let worker = thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(&server).read_line(&mut line).unwrap();
            let request: Envelope = serde_json::from_str(&line).unwrap();
            assert_eq!(request.request, Request::SetLocalProfile { settings });
            let profile = ryoku_profiles::ProfileDocument::new(
                ryoku_profiles::ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).unwrap(),
                settings,
            );
            let response = Response {
                version: SERVICE_PROTOCOL_VERSION,
                request_id: request.request_id,
                body: ResponseBody::LocalProfile(profile.clone()),
            };
            serde_json::to_writer(&mut &server, &response).unwrap();
            (&server).write_all(b"\n").unwrap();
            profile
        });
        let got = UnixServiceBackend::exchange_profile(
            &mut client,
            RequestId(31),
            Request::SetLocalProfile { settings },
        )
        .unwrap();
        assert_eq!(got, worker.join().unwrap());
    }
}
