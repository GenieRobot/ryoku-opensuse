//! Minimal GTK4 desktop proof for the typed, read-only UI action contract.
//!
//! This binary deliberately uses a demonstration backend. It does not contact
//! a system service or expose provider and privileged operations.

use gtk::prelude::*;
use ryoku_ui::{
    ActionResult, BackendError, RequestId, UiBackend, UiRequest, UiResponse, UpdateState,
    UserAction, dispatch, render_result, render_status, service_client::UnixServiceBackend,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct DemonstrationBackend {
    response: Result<UpdateState, BackendError>,
}

impl UiBackend for DemonstrationBackend {
    fn handle(&mut self, request: UiRequest) -> UiResponse {
        let outcome = match request.action {
            UserAction::RefreshStatus => self.response.clone().map(ActionResult::StatusRefreshed),
        };
        UiResponse {
            protocol_version: request.protocol_version,
            request_id: request.request_id,
            outcome,
        }
    }
}

enum BackendMode {
    Demonstration(DemonstrationBackend),
    Service(UnixServiceBackend),
    Failure(BackendError),
}

impl UiBackend for BackendMode {
    fn handle(&mut self, request: UiRequest) -> UiResponse {
        match self {
            Self::Demonstration(backend) => backend.handle(request),
            Self::Service(backend) => backend.handle(request),
            Self::Failure(error) => UiResponse {
                protocol_version: ryoku_ui::PROTOCOL_VERSION,
                request_id: request.request_id,
                outcome: Err(*error),
            },
        }
    }
}

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let simulate_error = arguments.iter().any(|arg| arg == "--service-unavailable");
    let capture_on_launch = arguments.iter().any(|arg| arg == "--capture-on-launch");
    let connect_service = arguments.iter().any(|arg| arg == "--connect-service");
    let application =
        gtk::Application::builder().application_id("org.ryoku.OpenSUSE.UiProof").build();

    application.connect_activate(move |application| {
        let status = gtk::Label::new(Some("System status: waiting for refresh"));
        status.set_xalign(0.0);
        status.set_wrap(true);

        let result = gtk::Label::builder()
            .label("Choose Refresh status to request an update.")
            .accessible_role(gtk::AccessibleRole::Alert)
            .build();
        result.set_xalign(0.0);
        result.set_wrap(true);

        let refresh = gtk::Button::builder().label("Refresh status").receives_default(true).build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.append(&status);
        content.append(&refresh);
        content.append(&result);

        let window = gtk::ApplicationWindow::builder()
            .application(application)
            .title("Ryoku status proof")
            .default_width(420)
            .child(&content)
            .build();
        window.set_default_widget(Some(&refresh));

        let backend = if connect_service {
            match UnixServiceBackend::from_environment() {
                Ok(backend) => BackendMode::Service(backend),
                Err(error) => BackendMode::Failure(error),
            }
        } else {
            BackendMode::Demonstration(DemonstrationBackend {
                response: if simulate_error {
                    Err(BackendError::Unavailable)
                } else {
                    Ok(UpdateState::UpdatesAvailable { count: 3 })
                },
            })
        };
        let backend = Rc::new(RefCell::new(backend));
        let backend_for_refresh = Rc::clone(&backend);
        let next_request_id = Cell::new(0_u64);

        refresh.connect_clicked(move |_| {
            let request_id = next_request_id.get().wrapping_add(1);
            next_request_id.set(request_id);
            let request = UiRequest::new(RequestId(request_id), UserAction::RefreshStatus);
            let outcome = dispatch(&mut *backend_for_refresh.borrow_mut(), request);
            if let Ok(ActionResult::StatusRefreshed(state)) = &outcome {
                status.set_text(&render_status(state));
            }
            result.set_text(&render_result(&outcome));
        });

        window.present();
        if capture_on_launch {
            refresh.emit_clicked();
        }
    });

    // Keep this app's option out of GTK's own argument parser.
    application.run_with_args(&["ryoku-ui-gtk"]);
}
