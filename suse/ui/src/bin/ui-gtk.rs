//! Minimal GTK4 desktop proof for the typed, read-only UI action contract.
//!
//! Service mode sends only Discover. Demonstration mode is explicitly separate.

use gtk::prelude::*;
use ryoku_ui::{
    ActionResult, BackendError, RequestId, UiBackend, UiRequest, UiResponse, UpdateState,
    UserAction, dispatch, render_result, render_status, service_client::UnixServiceBackend,
};
use std::{cell::Cell, rc::Rc, sync::mpsc, time::Duration};

#[derive(Clone)]
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

#[derive(Clone)]
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
    let capture_input = arguments.iter().any(|arg| arg == "--capture-input");
    let capture = capture_on_launch || capture_input;
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
            .title(if connect_service {
                "Ryoku service status proof"
            } else {
                "Ryoku status demonstration"
            })
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
        let next_request_id = Cell::new(0_u64);
        let pending_frame = Rc::new(Cell::new(None));
        let pending_for_refresh = Rc::clone(&pending_frame);

        refresh.connect_clicked(move |button| {
            if !button.is_sensitive() {
                return;
            }
            let request_id = next_request_id.get().wrapping_add(1);
            next_request_id.set(request_id);
            let request = UiRequest::new(RequestId(request_id), UserAction::RefreshStatus);
            if capture {
                println!("gtk_proof_request={request_id}");
            }
            button.set_sensitive(false);
            status.set_text(&render_status(&UpdateState::Checking));
            result.set_text("Refresh in progress.");
            let (sender, receiver) = mpsc::sync_channel(1);
            let mut backend = backend.clone();
            std::thread::spawn(move || {
                let _ = sender.send(dispatch(&mut backend, request));
            });
            let status = status.clone();
            let result = result.clone();
            let button = button.clone();
            let pending_frame = Rc::clone(&pending_for_refresh);
            gtk::glib::timeout_add_local(Duration::from_millis(20), move || {
                let outcome = match receiver.try_recv() {
                    Ok(outcome) => outcome,
                    Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => Err(BackendError::Unavailable),
                };
                if let Ok(ActionResult::StatusRefreshed(state)) = &outcome {
                    status.set_text(&render_status(state));
                } else {
                    status.set_text("Ryoku service: status unavailable");
                }
                result.set_text(&render_result(&outcome));
                button.set_sensitive(true);
                if capture {
                    let code = match outcome {
                        Ok(ActionResult::StatusRefreshed(UpdateState::RecoveryUnavailable)) => {
                            "recovery_unavailable"
                        }
                        Err(BackendError::Unavailable) => "unavailable",
                        _ => "other",
                    };
                    println!("gtk_proof_result={request_id}:{code}");
                    pending_frame.set(Some(request_id));
                }
                gtk::glib::ControlFlow::Break
            });
        });

        if capture {
            let painted = Rc::new(Cell::new(false));
            window.connect_map(move |window| {
                if let Some(clock) = window.frame_clock() {
                    let window = window.downgrade();
                    let painted = Rc::clone(&painted);
                    let pending_frame = Rc::clone(&pending_frame);
                    clock.connect_after_paint(move |_| {
                        let mapped = window.upgrade().is_some_and(|window| {
                            window.is_mapped() && window.width() > 0 && window.height() > 0
                        });
                        if mapped && !painted.get() {
                            painted.set(true);
                            println!("gtk_proof_first_frame=painted");
                        }
                        if mapped && let Some(request_id) = pending_frame.take() {
                            println!("gtk_proof_result_frame={request_id}");
                        }
                    });
                }
            });
        }
        window.present();
        refresh.grab_focus();
        if capture_on_launch {
            refresh.emit_clicked();
        }
    });

    // Keep this app's option out of GTK's own argument parser.
    application.run_with_args(&["ryoku-ui-gtk"]);
}
