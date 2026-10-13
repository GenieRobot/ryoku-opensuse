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
            let options = run_options(&args)?;
            host::run(&args[2], &args[3], options.capture_ui, options.zypp_probe)
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

struct RunOptions<'a> {
    capture_ui: Option<&'a str>,
    zypp_probe: Option<(&'a str, &'a str)>,
}

fn run_options(args: &[String]) -> Result<RunOptions<'_>, &'static str> {
    if args.len() < 4 {
        return Err(usage());
    }
    let mut options = RunOptions { capture_ui: None, zypp_probe: None };
    let mut index = 4;
    while index < args.len() {
        match args[index].as_str() {
            "--capture-ui" if options.capture_ui.is_none() && index + 1 < args.len() => {
                let binary = args[index + 1].as_str();
                if binary.is_empty() {
                    return Err(usage());
                }
                options.capture_ui = Some(binary);
                index += 2;
            }
            "--zypp-probe" if options.zypp_probe.is_none() && index + 2 < args.len() => {
                let binary = args[index + 1].as_str();
                let package = args[index + 2].as_str();
                if binary.is_empty()
                    || package.is_empty()
                    || !package
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte))
                {
                    return Err(usage());
                }
                options.zypp_probe = Some((binary, package));
                index += 3;
            }
            _ => return Err(usage()),
        }
    }
    Ok(options)
}

fn usage() -> &'static str {
    "usage: vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY [--capture-ui GTK_BINARY] [--zypp-probe PROBE_BINARY PACKAGE]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_capture_is_opt_in_and_requires_a_binary() {
        let normal = ["vm-lab", "run", "assets", "run"].map(str::to_owned);
        assert!(run_options(&normal).unwrap().capture_ui.is_none());
        let capture = ["vm-lab", "run", "assets", "run", "--capture-ui", "/tmp/ryoku-ui-gtk"]
            .map(str::to_owned);
        assert_eq!(run_options(&capture).unwrap().capture_ui, Some("/tmp/ryoku-ui-gtk"));
        let probe = ["vm-lab", "run", "assets", "run", "--zypp-probe", "/tmp/probe", "hello"]
            .map(str::to_owned);
        assert_eq!(run_options(&probe).unwrap().zypp_probe, Some(("/tmp/probe", "hello")));
        let both = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--zypp-probe",
            "/tmp/probe",
            "hello",
            "--capture-ui",
            "/tmp/ui",
        ]
        .map(str::to_owned);
        assert_eq!(run_options(&both).unwrap().capture_ui, Some("/tmp/ui"));
        assert!(run_options(&both).unwrap().zypp_probe.is_some());
        for invalid in [
            vec!["vm-lab", "run", "assets", "run", "--capture-ui", ""],
            vec!["vm-lab", "run", "assets", "run", "--capture-ui"],
            vec!["vm-lab", "run", "assets", "run", "--display", "gtk"],
            vec!["vm-lab", "run", "assets", "run", "--zypp-probe", "/tmp/probe"],
            vec!["vm-lab", "run", "assets", "run", "--zypp-probe", "/tmp/probe", "bad package"],
        ] {
            let invalid = invalid.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(run_options(&invalid).is_err());
        }
    }
}
