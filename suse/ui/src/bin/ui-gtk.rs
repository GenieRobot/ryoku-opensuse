//! Minimal GTK4 desktop proof for the typed, read-only UI action contract.
//!
//! This binary deliberately uses a demonstration backend. It does not contact
//! a system service or expose provider and privileged operations.

use gtk::prelude::*;
use ryoku_ui::{
    ActionResult, BackendError, UiBackend, UpdateState, UserAction, dispatch, render_result,
    render_status,
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
    let application =
        gtk::Application::builder().application_id("org.ryoku.OpenSUSE.UiProof").build();

    application.connect_activate(move |application| {
        let status = gtk::Label::new(Some("System status: waiting for refresh"));
        status.set_xalign(0.0);
        status.set_wrap(true);

        let result = gtk::Label::new(Some("Choose Refresh status to request an update."));
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

        let mut backend = DemonstrationBackend {
            response: if simulate_error {
                Err(BackendError::Unavailable)
            } else {
                Ok(UpdateState::UpdatesAvailable { count: 3 })
            },
        };

        refresh.connect_clicked(move |_| {
            let outcome = dispatch(&mut backend, UserAction::RefreshStatus);
            if let Ok(ActionResult::StatusRefreshed(state)) = &outcome {
                status.set_text(&render_status(state));
            }
            result.set_text(&render_result(&outcome));
        });

        window.present();
    });

    // Keep this app's option out of GTK's own argument parser.
    application.run_with_args(&["ryoku-ui-gtk"]);
}
