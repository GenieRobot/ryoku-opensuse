//! Exercise the shipped entrypoint before it can construct GIO or read settings.
#![allow(unsafe_code)]
use std::{
    io::Write,
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::Command,
};

#[test]
fn capture_binary_requires_its_exact_private_invocation() {
    let output =
        Command::new(env!("CARGO_BIN_EXE_ryoku-theme-actor")).arg("--apply").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("private capture worker"));
}

#[test]
fn capture_binary_rejects_a_real_nonroot_control_peer_before_decoding_data() {
    assert_ne!(rustix::process::geteuid().as_raw(), 0);
    let (worker, mut caller) = UnixStream::pair().unwrap();
    caller.write_all(b"malformed caller-selected settings\n").unwrap();
    let inherited = worker.as_raw_fd();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryoku-theme-actor"));
    command.arg("--control-fd=3");
    // SAFETY: this child inherits only the descriptor owned by this test.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(inherited, 3) != 3 || libc::fcntl(3, libc::F_SETFD, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("control peer is not root"));
}
