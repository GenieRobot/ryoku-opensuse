//! Typed, unprivileged presentation and action boundary for Ryoku.
//!
//! The contract includes a protocol version and opaque request ID so that an
//! eventual asynchronous service client can reject stale or mismatched replies.
//! The only action in this proof is a read-only status refresh.

pub mod launcher_provider;
pub mod service_client;

/// Version understood by this UI/service contract.
pub const PROTOCOL_VERSION: u16 = 1;

/// Opaque correlation ID assigned by the UI for one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestId(pub u64);

/// Backend state safe to present in an ordinary user interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateState {
    Checking,
    UpToDate,
    UpdatesAvailable { count: u32 },
    ServiceReady,
    RecoveryUnavailable,
}

/// The complete set of user actions currently exposed by this proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserAction {
    /// Ask the backend to refresh its read-only update summary.
    RefreshStatus,
}

/// A versioned request sent across the presentation/backend boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiRequest {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub action: UserAction,
}

impl UiRequest {
    pub const fn new(request_id: RequestId, action: UserAction) -> Self {
        Self { protocol_version: PROTOCOL_VERSION, request_id, action }
    }
}

/// Errors returned by the service boundary or detected while validating a reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    Unavailable,
    Unauthorized,
    InvalidResponse,
    UnsupportedProtocol,
    UnsupportedOperation,
    MismatchedRequestId,
}

/// Outcome returned by the backend for one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionResult {
    StatusRefreshed(UpdateState),
}

/// A versioned response paired with the request it answers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiResponse {
    pub protocol_version: u16,
    pub request_id: RequestId,
    pub outcome: Result<ActionResult, BackendError>,
}

/// Narrow backend contract. Implementations must remain unprivileged and may
/// expose only the explicitly typed actions represented by [`UserAction`].
pub trait UiBackend {
    fn handle(&mut self, request: UiRequest) -> UiResponse;
}

/// Dispatch a request and validate the version and correlation ID before the
/// response is exposed to the view.
pub fn dispatch<B: UiBackend>(
    backend: &mut B,
    request: UiRequest,
) -> Result<ActionResult, BackendError> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(BackendError::UnsupportedProtocol);
    }

    let response = backend.handle(request);
    if response.protocol_version != PROTOCOL_VERSION {
        return Err(BackendError::UnsupportedProtocol);
    }
    if response.request_id != request.request_id {
        return Err(BackendError::MismatchedRequestId);
    }
    response.outcome
}

/// Render the proof's representative dashboard summary as plain text.
pub fn render_status(state: &UpdateState) -> String {
    match state {
        UpdateState::Checking => "System status: checking".to_owned(),
        UpdateState::UpToDate => "System status: up to date".to_owned(),
        UpdateState::UpdatesAvailable { count } => {
            format!("System status: {count} updates available")
        }
        UpdateState::ServiceReady => {
            "Ryoku service: recovery available; this UI sent no managed action".to_owned()
        }
        UpdateState::RecoveryUnavailable => {
            "Ryoku service: recovery unavailable; this UI sent no managed action".to_owned()
        }
    }
}

/// Render a typed request outcome, including typed service and protocol errors.
pub fn render_result(result: &Result<ActionResult, BackendError>) -> String {
    match result {
        Ok(ActionResult::StatusRefreshed(state)) => {
            format!("Refresh complete. {}", render_status(state))
        }
        Err(BackendError::Unavailable) => {
            "Refresh failed: the Ryoku service is unavailable.".to_owned()
        }
        Err(BackendError::Unauthorized) => {
            "Refresh failed: the Ryoku service rejected this user.".to_owned()
        }
        Err(BackendError::InvalidResponse) => {
            "Refresh failed: the Ryoku service returned invalid status.".to_owned()
        }
        Err(BackendError::UnsupportedProtocol) => {
            "Refresh failed: the Ryoku service protocol is unsupported.".to_owned()
        }
        Err(BackendError::UnsupportedOperation) => {
            "Refresh failed: the Ryoku service does not support this request.".to_owned()
        }
        Err(BackendError::MismatchedRequestId) => {
            "Refresh failed: the Ryoku service returned an unrelated response.".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionResult, BackendError, PROTOCOL_VERSION, RequestId, UiBackend, UiRequest, UiResponse,
        UpdateState, UserAction, dispatch, render_result,
    };

    struct FakeBackend(UiResponse);

    impl UiBackend for FakeBackend {
        fn handle(&mut self, _request: UiRequest) -> UiResponse {
            self.0.clone()
        }
    }

    fn request() -> UiRequest {
        UiRequest::new(RequestId(42), UserAction::RefreshStatus)
    }

    fn response(outcome: Result<ActionResult, BackendError>) -> UiResponse {
        UiResponse { protocol_version: PROTOCOL_VERSION, request_id: RequestId(42), outcome }
    }

    #[test]
    fn refresh_returns_typed_state_for_correlated_request() {
        let mut backend = FakeBackend(response(Ok(ActionResult::StatusRefreshed(
            UpdateState::UpdatesAvailable { count: 3 },
        ))));
        let result = dispatch(&mut backend, request());

        assert_eq!(
            result,
            Ok(ActionResult::StatusRefreshed(UpdateState::UpdatesAvailable { count: 3 }))
        );
        assert_eq!(render_result(&result), "Refresh complete. System status: 3 updates available");
    }

    #[test]
    fn refresh_preserves_typed_service_error() {
        let mut backend = FakeBackend(response(Err(BackendError::Unavailable)));
        let result = dispatch(&mut backend, request());

        assert_eq!(result, Err(BackendError::Unavailable));
        assert_eq!(render_result(&result), "Refresh failed: the Ryoku service is unavailable.");
    }

    #[test]
    fn service_discovery_states_do_not_claim_package_updates() {
        assert_eq!(
            super::render_status(&UpdateState::ServiceReady),
            "Ryoku service: recovery available; this UI sent no managed action"
        );
        assert_eq!(
            super::render_status(&UpdateState::RecoveryUnavailable),
            "Ryoku service: recovery unavailable; this UI sent no managed action"
        );
        assert_eq!(
            super::render_result(&Err(BackendError::Unauthorized)),
            "Refresh failed: the Ryoku service rejected this user."
        );
    }

    #[test]
    fn rejects_response_with_unrelated_request_id() {
        let mut backend = FakeBackend(UiResponse {
            request_id: RequestId(7),
            ..response(Ok(ActionResult::StatusRefreshed(UpdateState::UpToDate)))
        });
        assert_eq!(dispatch(&mut backend, request()), Err(BackendError::MismatchedRequestId));
    }

    #[test]
    fn rejects_unknown_response_protocol() {
        let mut backend = FakeBackend(UiResponse {
            protocol_version: PROTOCOL_VERSION + 1,
            ..response(Ok(ActionResult::StatusRefreshed(UpdateState::UpToDate)))
        });
        assert_eq!(dispatch(&mut backend, request()), Err(BackendError::UnsupportedProtocol));
    }

    #[test]
    fn rejects_unknown_request_protocol_before_dispatch() {
        let mut backend =
            FakeBackend(response(Ok(ActionResult::StatusRefreshed(UpdateState::UpToDate))));
        let invalid = UiRequest { protocol_version: PROTOCOL_VERSION + 1, ..request() };
        assert_eq!(dispatch(&mut backend, invalid), Err(BackendError::UnsupportedProtocol));
    }
}
