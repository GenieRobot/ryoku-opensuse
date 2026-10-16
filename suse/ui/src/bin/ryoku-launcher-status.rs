//! Native executable provider for the unchanged pinned Ryoku launcher.

use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::net::UnixDatagram,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

use rustix::process::geteuid;
use ryoku_ui::{
    BackendError, RequestId, UpdateState,
    launcher_provider::{discover, parse_refresh, render_row, result_code},
    service_client::UnixServiceBackend,
};

fn main() -> io::Result<()> {
    let retv = std::env::var("ROFI_RETV").ok();
    let info = std::env::var("ROFI_INFO").ok();
    if !parse_refresh(std::env::args_os().nth(1).is_some(), retv.as_deref(), info.as_deref()) {
        return Ok(());
    }
    let mut nonce = [0_u8; 8];
    File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let request_id = RequestId(u64::from_ne_bytes(nonce));
    // Client construction never accepts a UI/environment-selected endpoint.
    let runtime = PathBuf::from(format!("/run/user/{}", geteuid().as_raw()));
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::Builder::new().name("service-discovery".into()).spawn(move || {
        let mut backend = UnixServiceBackend::from_runtime_dir(&runtime);
        journal_event(request_id, None);
        let _ = sender.send(discover(&mut backend, request_id));
    });
    // Also bound a blocked local connect. On timeout main returns and process
    // exit ends its sole read-only worker; no child process is left behind.
    let result = match worker {
        Ok(_) => {
            receiver.recv_timeout(Duration::from_secs(6)).unwrap_or(Err(BackendError::Unavailable))
        }
        Err(_) => Err(BackendError::Unavailable),
    };
    journal_event(request_id, Some(&result));
    io::stdout().lock().write_all(render_row(request_id, &result).as_bytes())
}

fn journal_event(request_id: RequestId, result: Option<&Result<UpdateState, BackendError>>) {
    let event = result.map_or_else(
        || "action=Discover".to_owned(),
        |result| format!("result={}", result_code(result)),
    );
    let message = format!("ryoku-launcher-status request=ui-{} {event}", request_id.0);
    // The unchanged provider discards stderr. Keep it useful for direct probes,
    // and send one nonblocking native journal datagram with fixed safe fields.
    // Logging failures never suppress the actual result row.
    let _ = writeln!(io::stderr().lock(), "{message}");
    let Ok(socket) = UnixDatagram::unbound() else { return };
    if socket.set_nonblocking(true).is_err() {
        return;
    }
    let entry = format!("PRIORITY=6\nSYSLOG_IDENTIFIER=ryoku-launcher-status\nMESSAGE={message}\n");
    let _ = socket.send_to(entry.as_bytes(), "/run/systemd/journal/socket");
}
