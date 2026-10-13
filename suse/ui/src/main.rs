//! Small Rust-only integration proof for the Ryoku dashboard status surface.

use ryoku_ui::{
    BackendError, UiBackend, UpdateState, UserAction, dispatch, render_result, render_status,
};

struct DemonstrationBackend {
    response: Result<UpdateState, BackendError>,
}

impl UiBackend for DemonstrationBackend {
    fn refresh_status(&mut self) -> Result<UpdateState, BackendError> {
        self.response.clone()
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
    let result = dispatch(&mut backend, UserAction::RefreshStatus);
    println!("{}", render_result(&result));
}
