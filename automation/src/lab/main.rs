//! Disposable cloud fixture automation; no production recovery implementation.
mod definition;
mod desktop;
mod guest;
mod host;
mod migration;
mod repositories;
mod support;

fn main() {
    if let Err(error) = run() {
        eprintln!("VM lab failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") if args.len() == 4 => host::run(&args[2], &args[3]),
        Some("guest-setup") if args.len() == 3 => guest::setup(&args[2]),
        Some("guest-migrate") if args.len() == 3 => migration::migrate(&args[2]),
        Some("guest-inspect") => guest::inspect(),
        Some("guest-inspect-base") => guest::inspect_base(),
        Some("guest-base-setup") => guest::base_setup(),
        Some("guest-login-prepare") if args.len() == 2 => desktop::prepare(),
        Some("guest-login-inspect") if args.len() == 2 => desktop::inspect(),
        Some("guest-recover") if args.len() == 3 => guest::recover(&args[2]),
        Some("guest-marker-absent") => guest::marker_absent(),
        _ => Err("usage: vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY".into()),
    }
}
