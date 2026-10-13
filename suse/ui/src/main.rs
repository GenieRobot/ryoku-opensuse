//! Small Rust-only integration proof for the Ryoku dashboard status surface.

use ryoku_ui::{
    ActionResult, BackendError, PROTOCOL_VERSION, RequestId, UiBackend, UiRequest, UiResponse,
    UpdateState, UserAction, dispatch, render_result, render_status,
};

struct DemonstrationBackend {
    response: Result<UpdateState, BackendError>,
}

impl UiBackend for DemonstrationBackend {
    fn handle(&mut self, request: UiRequest) -> UiResponse {
        let outcome = self.response.clone().map(ActionResult::StatusRefreshed);
        UiResponse { protocol_version: PROTOCOL_VERSION, request_id: request.request_id, outcome }
    }
}

fn main() {
    let simulate_error = std::env::args().skip(1).any(|arg| arg == "--service-unavailable");
    let initial_state = UpdateState::UpToDate;
    println!("Ryoku dashboard proof");
    println!("{}", render_status(&initial_state));
    println!("Action: refresh status (read-only)");

    let response = if simulate_error {
        Err(BackendError::Unavailable)
    } else {
        Ok(UpdateState::UpdatesAvailable { count: 3 })
    };
    let mut backend = DemonstrationBackend { response };
    let request = UiRequest::new(RequestId(1), UserAction::RefreshStatus);
    let result = dispatch(&mut backend, request);
    println!("{}", render_result(&result));
}
