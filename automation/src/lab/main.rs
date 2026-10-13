//! Disposable cloud fixture automation; no production recovery implementation.
mod definition;
mod desktop;
mod guest;
mod host;
mod migration;
mod repositories;
mod support;
#[cfg(test)]
mod zypp_plugin;

fn main() {
    if let Err(error) = run() {
        eprintln!("VM lab failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let ui_binary = capture_ui_binary(&args)?;
            host::run(&args[2], &args[3], ui_binary)
        }
        Some("guest-setup") if args.len() == 3 => guest::setup(&args[2]),
        Some("guest-migrate") if args.len() == 3 => migration::migrate(&args[2]),
        Some("guest-inspect") => guest::inspect(),
        Some("guest-inspect-base") => guest::inspect_base(),
        Some("guest-base-setup") => guest::base_setup(),
        Some("guest-login-prepare") if args.len() == 2 => desktop::prepare(),
        Some("guest-login-inspect") if args.len() == 2 => desktop::inspect(),
        Some("guest-ui-install") if args.len() == 2 => guest::install_ui_runtime(),
        Some("guest-ui-launch") if args.len() == 3 => desktop::launch_ui(args[2].as_str()),
        Some("guest-ui-stop") if args.len() == 2 => desktop::stop_ui(),
        Some("guest-recover") if args.len() == 3 => guest::recover(&args[2]),
        Some("guest-marker-absent") => guest::marker_absent(),
        _ => Err("usage: vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY [--capture-ui GTK_BINARY]"
            .into()),
    }
}

fn capture_ui_binary(args: &[String]) -> Result<Option<&str>, &'static str> {
    match args {
        [_, _, _, _] => Ok(None),
        [_, _, _, _, option, binary] if option == "--capture-ui" && !binary.is_empty() => {
            Ok(Some(binary))
        }
        _ => Err("usage: vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY [--capture-ui GTK_BINARY]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_capture_is_opt_in_and_requires_a_binary() {
        let normal = ["vm-lab", "run", "assets", "run"].map(str::to_owned);
        assert_eq!(capture_ui_binary(&normal).unwrap(), None);
        let capture = ["vm-lab", "run", "assets", "run", "--capture-ui", "/tmp/ryoku-ui-gtk"]
            .map(str::to_owned);
        assert_eq!(capture_ui_binary(&capture).unwrap(), Some("/tmp/ryoku-ui-gtk"));
        for invalid in [
            vec!["vm-lab", "run", "assets", "run", "--capture-ui", ""],
            vec!["vm-lab", "run", "assets", "run", "--capture-ui"],
            vec!["vm-lab", "run", "assets", "run", "--display", "gtk"],
        ] {
            let invalid = invalid.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(capture_ui_binary(&invalid).is_err());
        }
    }
}
