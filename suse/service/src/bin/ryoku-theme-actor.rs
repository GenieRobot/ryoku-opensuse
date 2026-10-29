//! Private one-shot capture worker. This binary has no settings-write command.
#![allow(unsafe_code)]
use std::{
    io,
    os::{fd::FromRawFd, unix::net::UnixStream},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("ryoku-theme-actor: {error}");
        std::process::exit(1);
    }
}
fn run() -> io::Result<()> {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--control-fd=3"] {
        return Err(io::Error::other(
            "private capture worker requires its inherited control descriptor",
        ));
    }
    ryoku_service::theme_actor_sandbox::harden_worker()?;
    // SAFETY: the fixed parent launcher duplicates its owned one-shot stream
    // to descriptor3; peer credentials are checked before any packet/GIO work.
    let control = unsafe { UnixStream::from_raw_fd(3) };
    ryoku_service::theme_actor::serve_capture(
        control,
        ryoku_service::theme_actor_scope::native_factory,
    )
}
