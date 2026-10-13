use std::os::fd::FromRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;

use ryoku_service::helper_protocol::{HELPER_SOCKET_PATH, serve_operator_helper_connection};
use ryoku_service::operator_group_gid;

fn main() {
    if let Err(error) = run() {
        eprintln!("ryoku-helper: {error}");
        std::process::exit(1);
    }
}

#[allow(unsafe_code)] // The one unsafe boundary is ownership of systemd's fd 3.
fn run() -> Result<(), String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("must run as root".into());
    }
    let pid = std::env::var("LISTEN_PID").map_err(|_| "missing systemd LISTEN_PID")?;
    let fds = std::env::var("LISTEN_FDS").map_err(|_| "missing systemd LISTEN_FDS")?;
    if pid.parse::<u32>().ok() != Some(std::process::id()) || fds != "1" {
        return Err("expected one systemd-activated listener".into());
    }

    // SAFETY: systemd socket activation supplies the listener as fd 3 when
    // LISTEN_PID and LISTEN_FDS match. The validated inherited descriptor is
    // transferred to UnixListener, which owns and closes it on drop.
    let listener = unsafe { UnixListener::from_raw_fd(3) };
    let path = listener
        .local_addr()
        .map_err(|error| format!("inspect activated listener: {error}"))?
        .as_pathname()
        .ok_or("activated listener is not a pathname socket")?
        .to_owned();
    if path != Path::new(HELPER_SOCKET_PATH) {
        return Err("activated listener has unexpected socket path".into());
    }
    // Keep the validated state directory descriptor alive for the helper's
    // entire request lifetime. Identity or live-mount failures abort startup,
    // so no helper request can reach a protected operation without this gate.
    let _protected_state = ryoku_service::protected_state::ProtectedState::open()
        .map_err(|error| format!("protected recovery state unavailable: {error}"))?;
    let operator_gid =
        operator_group_gid().map_err(|_| "ryoku-operators group is missing or invalid")?;
    for accepted in listener.incoming() {
        match accepted {
            Ok(stream) => {
                let _ = serve_operator_helper_connection(stream, operator_gid);
            }
            Err(error) => eprintln!("ryoku-helper: accept failed: {error}"),
        }
    }
    Ok(())
}
