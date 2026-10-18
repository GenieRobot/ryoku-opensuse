use ryoku_recovery::writer_protocol::{WriterKind, run_writer};

fn main() {
    let mut arguments = std::env::args_os();
    let executable = arguments.next().unwrap_or_default();
    let alias = match std::path::Path::new(&executable).file_name().and_then(|v| v.to_str()) {
        Some("ryoku-shell") => Some(WriterKind::Daemon),
        Some("ryoku-wm" | "ryoku-wm-hyprland") => Some(WriterKind::Provider),
        Some("ryoku-hub") => Some(WriterKind::HubBackend),
        Some("ryogami") => Some(WriterKind::Ryogami),
        _ => None,
    };
    let result = alias
        .map(Ok)
        .unwrap_or_else(|| {
            arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or(ryoku_recovery::capture_session::CaptureSessionError::Unsupported)
                .and_then(|kind| WriterKind::parse(&kind))
        })
        .and_then(|kind| run_writer(kind, &arguments.collect::<Vec<_>>()));
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("ryoku-writer: {error}");
            std::process::exit(1);
        }
    }
}
