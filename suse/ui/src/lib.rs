//! Typed, unprivileged presentation and action boundary for Ryoku.
//!
//! This crate deliberately has no desktop-toolkit or FFI dependency yet. The
//! current proof models a read-only status refresh so that a future native
//! surface can consume the same request/result contract without gaining
//! authority to change the system.

/// Backend state safe to present in an ordinary user interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateState {
    Checking,
    UpToDate,
    UpdatesAvailable { count: u32 },
}

/// The complete set of user actions currently exposed by this proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserAction {
    /// Ask the backend to refresh its read-only update summary.
    RefreshStatus,
}

/// Errors returned by the service boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    Unavailable,
    InvalidResponse,
}

/// Outcome returned to the surface after dispatching an action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionResult {
    StatusRefreshed(UpdateState),
}

/// Narrow asynchronous-service-shaped contract for the presentation layer.
///
/// Implementations should use an authenticated, unprivileged service client;
/// the UI must not call package managers, privileged helpers, or providers.
pub trait UiBackend {
    fn refresh_status(&mut self) -> Result<UpdateState, BackendError>;
}

/// Dispatch an explicit user action and preserve backend failures as typed
/// errors for the surface to render.
pub fn dispatch<B: UiBackend>(
    backend: &mut B,
    action: UserAction,
) -> Result<ActionResult, BackendError> {
    match action {
        UserAction::RefreshStatus => backend.refresh_status().map(ActionResult::StatusRefreshed),
    }
}

/// Render the proof's representative dashboard summary as plain text.
pub fn render_status(state: &UpdateState) -> String {
    match state {
        UpdateState::Checking => "System status: checking".to_owned(),
        UpdateState::UpToDate => "System status: up to date".to_owned(),
        UpdateState::UpdatesAvailable { count } => {
            format!("System status: {count} updates available")
        }
    }
}

/// Render a typed request outcome, including a typed service error.
pub fn render_result(result: &Result<ActionResult, BackendError>) -> String {
    match result {
        Ok(ActionResult::StatusRefreshed(state)) => {
            format!("Refresh complete. {}", render_status(state))
        }
        Err(BackendError::Unavailable) => {
            "Refresh failed: the Ryoku service is unavailable.".to_owned()
        }
        Err(BackendError::InvalidResponse) => {
            "Refresh failed: the Ryoku service returned invalid status.".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionResult, BackendError, UiBackend, UpdateState, UserAction, dispatch, render_result,
    };

    struct FakeBackend(Result<UpdateState, BackendError>);

    impl UiBackend for FakeBackend {
        fn refresh_status(&mut self) -> Result<UpdateState, BackendError> {
            self.0.clone()
        }
    }

    #[test]
    fn refresh_returns_typed_state() {
        let mut backend = FakeBackend(Ok(UpdateState::UpdatesAvailable { count: 3 }));
        let result = dispatch(&mut backend, UserAction::RefreshStatus);

        assert_eq!(
            result,
            Ok(ActionResult::StatusRefreshed(UpdateState::UpdatesAvailable { count: 3 }))
        );
        assert_eq!(render_result(&result), "Refresh complete. System status: 3 updates available");
    }

    #[test]
    fn refresh_preserves_service_error() {
        let mut backend = FakeBackend(Err(BackendError::Unavailable));
        let result = dispatch(&mut backend, UserAction::RefreshStatus);

        assert_eq!(result, Err(BackendError::Unavailable));
        assert_eq!(render_result(&result), "Refresh failed: the Ryoku service is unavailable.");
    }
}
