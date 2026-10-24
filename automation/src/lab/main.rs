//! Disposable cloud fixture automation; no production recovery implementation.
mod console_clock;
mod definition;
mod desktop;
mod guest;
mod host;
mod migration;
mod process;
mod repositories;
mod runner;
mod support;
#[cfg(test)]
mod zypp_plugin;

fn main() {
    if let Err(error) = run() {
        eprintln!("VM lab failed: {error}");
        let status = if error.to_string() == "KDE_PROVISIONING_TIMEOUT" { 124 } else { 1 };
        std::process::exit(status);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("run") => {
            let options = run_options(&args)?;
            host::run(&args[2], &args[3], options)
        }
        Some("guest-setup") if args.len() == 3 => guest::setup(&args[2]),
        Some("guest-migrate") if args.len() == 3 => migration::migrate(&args[2]),
        Some("guest-issue21-clock") if cfg!(feature = "issue21-lab") && args.len() == 3 => {
            console_clock::prepare(&args[2])
        }
        Some("guest-inspect") => guest::inspect(),
        Some("guest-inspect-base") => guest::inspect_base(),
        Some("guest-base-setup") => guest::base_setup(),
        Some("guest-issue19-rollback-setup") if cfg!(feature = "issue19-lab") => {
            guest::issue19_rollback_setup()
        }
        Some("guest-login-prepare") if args.len() == 2 => desktop::prepare(),
        Some("guest-login-inspect") if args.len() == 2 => desktop::inspect(),
        Some("guest-ui-install") if args.len() == 2 => guest::install_ui_runtime(),
        Some("guest-ui-launch") if args.len() == 3 => desktop::launch_ui(args[2].as_str()),
        Some("guest-ui-stop") if args.len() == 2 => desktop::stop_ui(),
        Some("guest-ui-diagnostics") if args.len() == 2 => desktop::ui_diagnostics(),
        Some("guest-recover") if args.len() == 3 => guest::recover(&args[2]),
        Some("guest-online-probe") if args.len() == 3 => guest::online_probe(&args[2]),
        Some("guest-marker-absent") => guest::marker_absent(),
        _ => Err("usage: vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY [--capture-ui GTK_BINARY]"
            .into()),
    }
}

pub(crate) struct RunOptions<'a> {
    capture_ui: Option<&'a str>,
    zypp_probe: Option<(&'a str, &'a str)>,
    state_probe: Option<&'a str>,
    state_initializer: Option<&'a str>,
    state_prepare: Option<&'a str>,
    linked_probe: Option<&'a str>,
    recovery_cli: Option<&'a str>,
    mutation_probe: Option<&'a str>,
    mutation_console: Option<&'a str>,
    mutation_interrupt: bool,
    console_only: bool,
    console_restore: Option<(&'a str, &'a str, &'a str)>,
    installer_probe: Option<(&'a str, &'a str, &'a str)>,
}

fn run_options(args: &[String]) -> Result<RunOptions<'_>, &'static str> {
    if args.len() < 4 {
        return Err(usage());
    }
    let mut options = RunOptions {
        capture_ui: None,
        zypp_probe: None,
        state_probe: None,
        state_initializer: None,
        state_prepare: None,
        linked_probe: None,
        recovery_cli: None,
        mutation_probe: None,
        mutation_console: None,
        mutation_interrupt: false,
        console_only: false,
        console_restore: None,
        installer_probe: None,
    };
    let mut index = 4;
    while index < args.len() {
        match args[index].as_str() {
            "--installer-lifecycle-probe"
                if cfg!(feature = "issue24-lab")
                    && options.installer_probe.is_none()
                    && index + 3 < args.len()
                    && args[index + 1..=index + 3].iter().all(|arg| !arg.is_empty()) =>
            {
                options.installer_probe =
                    Some((&args[index + 1], &args[index + 2], &args[index + 3]));
                index += 4;
            }
            "--console-restore-probe"
                if cfg!(feature = "issue21-lab")
                    && options.console_restore.is_none()
                    && index + 3 < args.len()
                    && args[index + 1..=index + 3].iter().all(|arg| !arg.is_empty()) =>
            {
                options.console_restore =
                    Some((&args[index + 1], &args[index + 2], &args[index + 3]));
                index += 4;
            }
            "--mutation-probe"
                if cfg!(feature = "issue20-lab")
                    && options.mutation_probe.is_none()
                    && index + 1 < args.len()
                    && !args[index + 1].is_empty() =>
            {
                options.mutation_probe = Some(&args[index + 1]);
                index += 2;
            }
            "--mutation-console"
                if cfg!(feature = "issue20-lab")
                    && options.mutation_console.is_none()
                    && index + 1 < args.len()
                    && !args[index + 1].is_empty() =>
            {
                options.mutation_console = Some(&args[index + 1]);
                index += 2;
            }
            "--mutation-interrupt"
                if cfg!(feature = "issue20-lab") && !options.mutation_interrupt =>
            {
                options.mutation_interrupt = true;
                index += 1;
            }
            "--linked-checkpoint-probe"
                if options.linked_probe.is_none() && index + 1 < args.len() =>
            {
                if args[index + 1].is_empty() {
                    return Err(usage());
                }
                options.linked_probe = Some(&args[index + 1]);
                index += 2;
            }
            "--recovery-cli" if options.recovery_cli.is_none() && index + 1 < args.len() => {
                if args[index + 1].is_empty() {
                    return Err(usage());
                }
                options.recovery_cli = Some(&args[index + 1]);
                index += 2;
            }
            "--state-prepare-probe"
                if options.state_prepare.is_none() && index + 1 < args.len() =>
            {
                let binary = args[index + 1].as_str();
                if binary.is_empty() {
                    return Err(usage());
                }
                options.state_prepare = Some(binary);
                index += 2;
            }
            "--state-initializer"
                if options.state_initializer.is_none() && index + 1 < args.len() =>
            {
                let binary = args[index + 1].as_str();
                if binary.is_empty() {
                    return Err(usage());
                }
                options.state_initializer = Some(binary);
                index += 2;
            }
            "--console-only" if !options.console_only => {
                options.console_only = true;
                index += 1;
            }
            "--state-probe" if options.state_probe.is_none() && index + 1 < args.len() => {
                let binary = args[index + 1].as_str();
                if binary.is_empty() {
                    return Err(usage());
                }
                options.state_probe = Some(binary);
                index += 2;
            }
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
                    || !package.as_bytes()[0].is_ascii_alphanumeric()
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
    if options.console_only
        && (options.capture_ui.is_some()
            || (options.state_probe.is_none()
                && options.zypp_probe.is_none()
                && options.state_initializer.is_none()))
    {
        return Err(usage());
    }
    if options.state_initializer.is_some()
        && (!options.console_only || options.state_probe.is_some() || options.zypp_probe.is_some())
    {
        return Err(usage());
    }
    if options.state_prepare.is_some() && options.state_initializer.is_none() {
        return Err(usage());
    }
    if (options.linked_probe.is_some() || options.console_restore.is_some())
        != options.recovery_cli.is_some()
        || (options.linked_probe.is_some()
            && (!options.console_only || options.state_initializer.is_none()))
    {
        return Err(usage());
    }
    if options.mutation_probe.is_some() != options.mutation_console.is_some()
        || (options.mutation_probe.is_some()
            && (!options.console_only
                || options.state_initializer.is_none()
                || options.linked_probe.is_some()
                || options.state_prepare.is_some()))
        || (options.mutation_interrupt && options.mutation_probe.is_none())
    {
        return Err(usage());
    }
    if options.console_restore.is_some()
        && (!options.console_only
            || options.state_initializer.is_none()
            || options.linked_probe.is_some()
            || options.mutation_probe.is_some()
            || options.state_prepare.is_some()
            || options.state_probe.is_some()
            || options.zypp_probe.is_some())
    {
        return Err(usage());
    }
    Ok(options)
}

fn usage() -> &'static str {
    "usage: vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY [--capture-ui GTK_BINARY] [--zypp-probe PROBE_BINARY PACKAGE] [--state-probe PROBE_BINARY] [--state-initializer INITIALIZER_BINARY] [--state-prepare-probe PROBE_BINARY] [--linked-checkpoint-probe PROBE_BINARY --recovery-cli CLI_BINARY] [--mutation-probe PROBE_BINARY --mutation-console CONSOLE_BINARY [--mutation-interrupt]] [--console-restore-probe PROBE PACKAGES_TAR SIGNING_KEY --recovery-cli CLI] [--console-only]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_restore_requires_its_feature_and_complete_isolated_inputs() {
        let args = [
            "vm-lab",
            "run",
            "assets",
            "fresh-run",
            "--console-only",
            "--state-initializer",
            "init",
            "--recovery-cli",
            "cli",
            "--console-restore-probe",
            "probe",
            "packages.tar",
            "key.asc",
        ]
        .map(str::to_owned);
        if cfg!(feature = "issue21-lab") {
            assert_eq!(
                run_options(&args).unwrap().console_restore,
                Some(("probe", "packages.tar", "key.asc"))
            );
        } else {
            assert!(run_options(&args).is_err());
        }
        assert!(run_options(&args[..args.len() - 1]).is_err());
        for extra in [
            vec!["--linked-checkpoint-probe", "other"],
            vec!["--mutation-probe", "other", "--mutation-console", "history"],
            vec!["--state-prepare-probe", "other"],
        ] {
            let mut mixed = args.to_vec();
            mixed.extend(extra.into_iter().map(str::to_owned));
            assert!(run_options(&mixed).is_err());
        }
    }

    #[test]
    fn mutation_inputs_require_the_isolated_initialized_console_feature() {
        let base =
            ["vm-lab", "run", "assets", "run", "--console-only", "--state-initializer", "init"]
                .map(str::to_owned);
        let mut args = base.to_vec();
        args.extend(["--mutation-probe", "probe"].map(str::to_owned));
        assert!(run_options(&args).is_err());
        args.extend(["--mutation-console", "console"].map(str::to_owned));
        if cfg!(feature = "issue20-lab") {
            assert_eq!(run_options(&args).unwrap().mutation_probe, Some("probe"));
            args.push("--mutation-interrupt".into());
            assert!(run_options(&args).unwrap().mutation_interrupt);
            args.extend(
                ["--linked-checkpoint-probe", "linked", "--recovery-cli", "recovery"]
                    .map(str::to_owned),
            );
            assert!(run_options(&args).is_err());
        } else {
            assert!(run_options(&args).is_err());
        }
    }

    #[test]
    fn linked_probe_requires_exact_binary_pair_and_initialized_console_fixture() {
        let base =
            ["vm-lab", "run", "assets", "run", "--console-only", "--state-initializer", "init"]
                .map(str::to_owned);
        let mut arguments = base.to_vec();
        arguments.extend(["--linked-checkpoint-probe", "probe"].map(str::to_owned));
        assert!(run_options(&arguments).is_err());
        arguments.extend(["--recovery-cli", "cli"].map(str::to_owned));
        assert_eq!(run_options(&arguments).unwrap().linked_probe, Some("probe"));
        assert_eq!(run_options(&arguments).unwrap().recovery_cli, Some("cli"));
        arguments.extend(["--recovery-cli", "other"].map(str::to_owned));
        assert!(run_options(&arguments).is_err());
        let absent_init = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--console-only",
            "--linked-checkpoint-probe",
            "probe",
            "--recovery-cli",
            "cli",
        ]
        .map(str::to_owned);
        assert!(run_options(&absent_init).is_err());
    }

    #[test]
    fn console_probes_cannot_claim_desktop_capture_or_run_without_a_probe() {
        let valid =
            ["vm-lab", "run", "assets", "run", "--console-only", "--state-probe", "/tmp/probe"]
                .map(str::to_owned);
        assert!(run_options(&valid).unwrap().console_only);
        let initializer = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--console-only",
            "--state-initializer",
            "/tmp/init",
        ]
        .map(str::to_owned);
        assert_eq!(run_options(&initializer).unwrap().state_initializer, Some("/tmp/init"));
        let mut paired = initializer.to_vec();
        paired.extend(["--state-prepare-probe", "/tmp/prepare"].map(str::to_owned));
        assert_eq!(run_options(&paired).unwrap().state_prepare, Some("/tmp/prepare"));
        for invalid in [
            vec!["vm-lab", "run", "assets", "run", "--state-prepare-probe", "/tmp/prepare"],
            vec!["vm-lab", "run", "assets", "run", "--state-initializer", "/tmp/init"],
            vec![
                "vm-lab",
                "run",
                "assets",
                "run",
                "--console-only",
                "--state-initializer",
                "/tmp/init",
                "--state-probe",
                "/tmp/probe",
            ],
            vec!["vm-lab", "run", "assets", "run", "--console-only"],
            vec![
                "vm-lab",
                "run",
                "assets",
                "run",
                "--console-only",
                "--state-probe",
                "/tmp/probe",
                "--capture-ui",
                "/tmp/ui",
            ],
        ] {
            assert!(
                run_options(&invalid.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
    }

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
            vec!["vm-lab", "run", "assets", "run", "--zypp-probe", "/tmp/probe", "-bad"],
        ] {
            let invalid = invalid.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(run_options(&invalid).is_err());
        }
    }
}
#[path = "../../../suse/recovery/examples/support/issue22_results.rs"]
mod recovery_gate_results;
