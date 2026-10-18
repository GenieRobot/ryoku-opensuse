//! Read-only adapter for the pinned launcher's existing rofi-script extension.
//!
//! The extension's process and row protocol is a presentation boundary, not the
//! Ryoku Go daemon protocol. Only an explicit refresh query reaches the typed
//! service client. Row activation never dispatches an operation.

use crate::{
    ActionResult, BackendError, RequestId, UiBackend, UiRequest, UpdateState, UserAction, dispatch,
};

/// Accept only the initial/list pass, no argv, and either literal read query.
/// Alternating the two queries changes the unchanged launcher's cache key.
pub fn parse_refresh(has_arguments: bool, retv: Option<&str>, info: Option<&str>) -> bool {
    !has_arguments && retv == Some("0") && matches!(info, Some("status" | "refresh"))
}

/// Query only the read-only typed action and validate response correlation.
pub fn discover<B: UiBackend>(
    backend: &mut B,
    request_id: RequestId,
) -> Result<UpdateState, BackendError> {
    match dispatch(backend, UiRequest::new(request_id, UserAction::RefreshStatus))? {
        ActionResult::StatusRefreshed(
            state @ (UpdateState::ServiceReady | UpdateState::RecoveryUnavailable),
        ) => Ok(state),
        // Demonstration/update counts are not service discovery facts.
        ActionResult::StatusRefreshed(_) => Err(BackendError::InvalidResponse),
    }
}

/// Fixed safe labels; no backend detail, environment or user-supplied text is
/// copied into the rofi row protocol or diagnostics.
pub fn result_code(result: &Result<UpdateState, BackendError>) -> &'static str {
    match result {
        Ok(UpdateState::ServiceReady) => "Ready",
        Ok(UpdateState::RecoveryUnavailable) => "RecoveryUnavailable",
        Ok(_) | Err(BackendError::InvalidResponse) => "InvalidResponse",
        Err(BackendError::Unavailable) => "Unavailable",
        Err(BackendError::Unauthorized) => "Unauthorized",
        Err(BackendError::NotFound) => "NotFound",
        Err(BackendError::MalformedRequest) => "MalformedRequest",
        Err(BackendError::StaleRequest) => "StaleRequest",
        Err(BackendError::RecoveryUnavailable) => "RecoveryUnavailable",
        Err(BackendError::Busy) => "Busy",
        Err(BackendError::Failed) => "Failed",
        Err(BackendError::UnsupportedProtocol) => "UnsupportedProtocol",
        Err(BackendError::UnsupportedOperation) => "UnsupportedOperation",
        Err(BackendError::MismatchedRequestId) => "MismatchedRequestId",
    }
}

/// A normal result row carries an explicit success/error distinction. Both
/// outcomes are delivered successfully through the legacy presentation pipe;
/// a typed service failure is never changed into a successful discovery.
pub fn render_row(request_id: RequestId, result: &Result<UpdateState, BackendError>) -> String {
    let outcome =
        if matches!(result, Ok(UpdateState::ServiceReady | UpdateState::RecoveryUnavailable)) {
            "succeeded"
        } else {
            "failed"
        };
    format!("Discover {outcome}: {} [ui-{}]\n", result_code(result), request_id.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PROTOCOL_VERSION, UiResponse};

    struct Backend {
        response: UiResponse,
        calls: Vec<UiRequest>,
    }

    impl UiBackend for Backend {
        fn handle(&mut self, request: UiRequest) -> UiResponse {
            self.calls.push(request);
            self.response.clone()
        }
    }

    fn backend(outcome: Result<ActionResult, BackendError>) -> Backend {
        Backend {
            response: UiResponse {
                protocol_version: PROTOCOL_VERSION,
                request_id: RequestId(2),
                outcome,
            },
            calls: Vec::new(),
        }
    }

    #[test]
    fn accepts_only_explicit_bounded_list_refreshes() {
        assert!(parse_refresh(false, Some("0"), Some("status")));
        assert!(parse_refresh(false, Some("0"), Some("refresh")));
        for query in [
            "",
            "refresh 0",
            " refresh",
            "refresh\ninstall",
            "refresh\0icon\u{1f}root",
            "$(id)",
            "ｒｅｆｒｅｓｈ",
            "install x",
            "refresh ",
        ] {
            assert!(!parse_refresh(false, Some("0"), Some(query)), "{query:?}");
        }
        for retv in [None, Some("1"), Some("2"), Some("-1"), Some("0\n")] {
            assert!(!parse_refresh(false, retv, Some("refresh")));
        }
        assert!(!parse_refresh(true, Some("0"), Some("refresh")));
        assert!(!parse_refresh(false, Some("0"), None));
    }

    #[test]
    fn invalid_invocations_and_selection_cannot_dispatch() {
        let mut client =
            backend(Ok(ActionResult::StatusRefreshed(UpdateState::RecoveryUnavailable)));
        for (argv, retv, info) in [
            (true, "0", "refresh"),
            (false, "1", "refresh"),
            (false, "0", "Start PackageInstall"),
            (false, "0", "refresh 2; rm -rf /"),
        ] {
            if parse_refresh(argv, Some(retv), Some(info)) {
                let _ = discover(&mut client, RequestId(2));
            }
        }
        assert!(client.calls.is_empty());
    }

    #[test]
    fn preserves_real_typed_state_and_sends_only_refresh() {
        let mut client =
            backend(Ok(ActionResult::StatusRefreshed(UpdateState::RecoveryUnavailable)));
        let result = discover(&mut client, RequestId(2));
        assert_eq!(client.calls, vec![UiRequest::new(RequestId(2), UserAction::RefreshStatus)]);
        assert_eq!(
            render_row(RequestId(2), &result),
            "Discover succeeded: RecoveryUnavailable [ui-2]\n"
        );
    }

    #[test]
    fn preserves_error_and_rejects_wrong_correlation_or_sample_data() {
        let mut client = backend(Err(BackendError::Unavailable));
        let result = discover(&mut client, RequestId(2));
        assert_eq!(render_row(RequestId(2), &result), "Discover failed: Unavailable [ui-2]\n");
        client.response.request_id = RequestId(3);
        assert_eq!(discover(&mut client, RequestId(2)), Err(BackendError::MismatchedRequestId));
        client.response.request_id = RequestId(2);
        client.response.protocol_version = PROTOCOL_VERSION + 1;
        assert_eq!(discover(&mut client, RequestId(2)), Err(BackendError::UnsupportedProtocol));
        client.response.protocol_version = PROTOCOL_VERSION;
        client.response.outcome =
            Ok(ActionResult::StatusRefreshed(UpdateState::UpdatesAvailable { count: 3 }));
        assert_eq!(discover(&mut client, RequestId(2)), Err(BackendError::InvalidResponse));
    }

    #[test]
    fn every_error_has_a_bounded_single_line_without_rofi_directives() {
        for error in [
            BackendError::Unavailable,
            BackendError::Unauthorized,
            BackendError::InvalidResponse,
            BackendError::UnsupportedProtocol,
            BackendError::UnsupportedOperation,
            BackendError::MismatchedRequestId,
        ] {
            let row = render_row(RequestId(u64::MAX), &Err(error));
            assert!(row.starts_with("Discover failed:"));
            assert!(row.len() < 96);
            assert_eq!(row.lines().count(), 1);
            assert!(!row.contains(['\0', '\u{1f}']));
        }
    }
}
