use std::os::fd::FromRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;

use ryoku_service::helper_protocol::{
    HELPER_SOCKET_PATH, serve_protected_operator_helper_connection,
};
use ryoku_service::operator_group_gid;
use ryoku_service::recovery_gate::serve_recovery_gate_listener;

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        None => run(),
        Some("--recovery-gate") => run_recovery_gate(),
        Some(_) => Err("unsupported helper mode".into()),
    };
    if let Err(error) = result {
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
    // Pin the validated read-only generation store for the helper lifetime.
    let recovery_reader = ryoku_recovery::generation::GenerationReader::open_state_mount()
        .map_err(|error| format!("protected recovery state unavailable: {error}"))?;
    let operator_gid =
        operator_group_gid().map_err(|_| "ryoku-operators group is missing or invalid")?;
    for accepted in listener.incoming() {
        match accepted {
            Ok(stream) => {
                let _ = serve_protected_operator_helper_connection(
                    stream,
                    operator_gid,
                    &recovery_reader,
                );
            }
            Err(error) => eprintln!("ryoku-helper: accept failed: {error}"),
        }
    }
    Ok(())
}

#[allow(unsafe_code)] // The one unsafe boundary is ownership of systemd's fd 3.
fn run_recovery_gate() -> Result<(), String> {
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
    ryoku_service::recovery_gate::validate_recovery_gate_listener_path(&path)
        .map_err(|_| "activated listener has unexpected socket path")?;
    serve_recovery_gate_listener(listener).map_err(|_| "recovery gate listener failed".into())
}
