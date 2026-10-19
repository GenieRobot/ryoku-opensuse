//! Disposable cloud fixture automation; no production recovery implementation.
mod accelerator;
mod definition;
mod desktop;
mod development;
mod guest;
mod host;
mod migration;
mod preflight;
mod process;
mod profile_bundle;
mod repositories;
mod retained;
mod runner;
mod service_fixture;
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
        Some("development-resume" | "development-resume-preflight") => {
            let accelerator = resume_options(&args)?;
            if args[1] == "development-resume-preflight" {
                retained::preflight(&args[2], &args[3], accelerator).map(|_| {
                    println!("development_resume_preflight=passed\nvm_runtime=not_started");
                })
            } else {
                host::development_resume(&args[2], &args[3], accelerator)
            }
        }
        Some("development-capture") if args.len() == 3 => host::development_capture(&args[2]),
        Some("development-input") if args.len() == 5 => {
            host::development_input(&args[2], &args[3], &args[4])
        }
        Some("development-login") if args.len() == 5 && args[3] == "--session" => {
            host::development_login(&args[2], desktop::NativeSession::parse(&args[4])?)
        }
        Some("run") => {
            let options = run_options(&args)?;
            host::run(&args[2], &args[3], options)
        }
        Some("preflight") => {
            let options = run_options(&args)?;
            preflight::report(&args[2], &args[3], options)
        }
        Some("guest-setup") if args.len() == 3 => guest::setup(&args[2]),
        Some("guest-migrate") if args.len() == 3 => migration::migrate(&args[2]),
        Some("guest-inspect") => guest::inspect(),
        Some("guest-inspect-base") => guest::inspect_base(),
        Some("guest-base-setup") => guest::base_setup(),
        Some("guest-profile-preflight") if args.len() == 2 => profile_bundle::guest_preflight(),
        Some("guest-service-stage") if args.len() == 5 => {
            service_fixture::guest_stage([&args[2], &args[3], &args[4]])
        }
        Some("guest-profile-bundle") if args.len() == 4 => {
            profile_bundle::guest_run(&args[2], &args[3])
        }
        Some("guest-login-prepare") if args.len() == 2 => desktop::prepare(),
        Some("guest-login-inspect") if args.len() == 2 => desktop::inspect(),
        Some("guest-development-login-prepare") if args.len() == 3 => {
            desktop::prepare_existing(desktop::NativeSession::parse(&args[2])?)
        }
        Some("guest-development-login-inspect") if args.len() == 3 => {
            desktop::inspect_selected(desktop::NativeSession::parse(&args[2])?)
        }
        Some("guest-ui-install") if args.len() == 2 => guest::install_ui_runtime(),
        Some("guest-ui-service-install") if args.len() == 3 => {
            desktop::install_ui_service(&args[2])
        }
        Some("guest-ui-launch") if args.len() == 3 => desktop::launch_ui(args[2].as_str()),
        Some("guest-ui-result") if args.len() == 3 => desktop::ui_result(&args[2]),
        Some("guest-ui-stop") if args.len() == 2 => desktop::stop_ui(),
        Some("guest-ui-diagnostics") if args.len() == 2 => desktop::ui_diagnostics(),
        Some("guest-recover") if args.len() == 3 => guest::recover(&args[2]),
        Some("guest-online-probe") if args.len() == 3 => guest::online_probe(&args[2]),
        Some("guest-marker-absent") => guest::marker_absent(),
        _ => Err(usage().into()),
    }
}

fn resume_options(args: &[String]) -> Result<accelerator::Accelerator, &'static str> {
    if !runner::WSL || args.len() != 6 || args[4] != "--accelerator" {
        return Err(
            "usage: vm-lab development-resume[-preflight] ASSET_DIRECTORY RETAINED_RUN_DIRECTORY --accelerator tcg|kvm (WSL only)",
        );
    }
    accelerator::Accelerator::parse(&args[5], true)
}

#[derive(Clone, Copy)]
pub(crate) struct RunOptions<'a> {
    accelerator: Option<accelerator::Accelerator>,
    capture_ui: Option<&'a str>,
    capture_user_service: Option<&'a str>,
    zypp_probe: Option<(&'a str, &'a str)>,
    state_probe: Option<&'a str>,
    state_initializer: Option<&'a str>,
    state_prepare: Option<&'a str>,
    linked_probe: Option<&'a str>,
    recovery_cli: Option<&'a str>,
    profile_bundle: Option<(&'a str, &'a str, &'a str)>,
    service_fixture: Option<(&'a str, &'a str, &'a str)>,
    console_only: bool,
    development_desktop: bool,
}

fn run_options(args: &[String]) -> Result<RunOptions<'_>, &'static str> {
    if args.len() < 4 {
        return Err(usage());
    }
    let mut options = RunOptions {
        accelerator: None,
        capture_ui: None,
        capture_user_service: None,
        zypp_probe: None,
        state_probe: None,
        state_initializer: None,
        state_prepare: None,
        linked_probe: None,
        recovery_cli: None,
        profile_bundle: None,
        service_fixture: None,
        console_only: false,
        development_desktop: false,
    };
    let mut index = 4;
    while index < args.len() {
        match args[index].as_str() {
            "--development-desktop" if runner::WSL && !options.development_desktop => {
                options.development_desktop = true;
                index += 1;
            }
            "--capture-user-service"
                if options.capture_user_service.is_none() && index + 1 < args.len() =>
            {
                if args[index + 1].is_empty() {
                    return Err(usage());
                }
                options.capture_user_service = Some(&args[index + 1]);
                index += 2;
            }
            "--service-fixture" if options.service_fixture.is_none() && index + 3 < args.len() => {
                if args[index + 1..index + 4].iter().any(String::is_empty) {
                    return Err(usage());
                }
                options.service_fixture =
                    Some((&args[index + 1], &args[index + 2], &args[index + 3]));
                index += 4;
            }
            "--profile-bundle-probe"
                if options.profile_bundle.is_none() && index + 3 < args.len() =>
            {
                if args[index + 1].is_empty()
                    || args[index + 2].is_empty()
                    || !profile_bundle::valid_digest(&args[index + 3])
                {
                    return Err(usage());
                }
                options.profile_bundle =
                    Some((&args[index + 1], &args[index + 2], &args[index + 3]));
                index += 4;
            }
            "--accelerator" if options.accelerator.is_none() && index + 1 < args.len() => {
                options.accelerator =
                    Some(accelerator::Accelerator::parse(&args[index + 1], runner::WSL)?);
                index += 2;
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
    if options.development_desktop
        && (options.console_only
            || options.zypp_probe.is_some()
            || options.state_probe.is_some()
            || options.state_initializer.is_some()
            || options.state_prepare.is_some()
            || options.linked_probe.is_some()
            || options.recovery_cli.is_some()
            || options.profile_bundle.is_some()
            || options.service_fixture.is_some())
    {
        return Err(usage());
    }
    if options.capture_user_service.is_some()
        && (options.capture_ui.is_none() || options.console_only)
    {
        return Err(usage());
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
    if options.profile_bundle.is_some()
        && (!options.console_only || options.state_initializer.is_none())
    {
        return Err(usage());
    }
    if options.service_fixture.is_some()
        && (!options.console_only || options.state_initializer.is_none())
    {
        return Err(usage());
    }
    if options.linked_probe.is_some() != options.recovery_cli.is_some()
        || (options.linked_probe.is_some()
            && (!options.console_only || options.state_initializer.is_none()))
    {
        return Err(usage());
    }
    Ok(options)
}

fn usage() -> &'static str {
    "usage: vm-lab {run|preflight} ASSET_DIRECTORY FRESH_RUN_DIRECTORY [--accelerator tcg|kvm] [--capture-ui GTK_BINARY [--capture-user-service DAEMON]] [--zypp-probe PROBE_BINARY PACKAGE] [--state-probe PROBE_BINARY] [--state-initializer INITIALIZER_BINARY] [--state-prepare-probe PROBE_BINARY] [--linked-checkpoint-probe PROBE_BINARY --recovery-cli CLI_BINARY] [--profile-bundle-probe PROBE_BINARY DEFAULTS_RPM RPM_SHA256] [--service-fixture FIXTURE SERVICE HELPER] [--console-only] [--development-desktop (WSL only)]"
}

#[cfg(test)]
mod development_options_tests {
    use super::*;

    #[test]
    fn resume_requires_its_own_command_and_exact_explicit_accelerator() {
        let args = ["vm-lab", "development-resume", "assets", "/run", "--accelerator", "kvm"]
            .map(String::from)
            .to_vec();
        assert_eq!(resume_options(&args).is_ok(), runner::WSL);
        for length in 0..args.len() {
            assert!(resume_options(&args[..length]).is_err());
        }
        let mut extra = args.clone();
        extra.push("--development-desktop".into());
        assert!(resume_options(&extra).is_err());
        let mut implicit = args;
        implicit[5] = "auto".into();
        assert!(resume_options(&implicit).is_err());
    }

    #[test]
    fn desktop_hold_is_explicit_wsl_only_and_excludes_mutation_probes() {
        let base = ["vm-lab", "run", "assets", "run"];
        let mut args = base.map(String::from).to_vec();
        assert!(!run_options(&args).unwrap().development_desktop);
        args.push("--development-desktop".into());
        assert_eq!(run_options(&args).is_ok(), runner::WSL);
        if !runner::WSL {
            return;
        }
        for extra in [
            vec!["--development-desktop"],
            vec!["--console-only"],
            vec!["--zypp-probe", "/tmp/probe", "test"],
            vec!["--state-probe", "/tmp/probe"],
            vec!["--state-initializer", "/tmp/probe"],
            vec!["--state-prepare-probe", "/tmp/probe"],
            vec!["--linked-checkpoint-probe", "/tmp/probe"],
            vec!["--recovery-cli", "/tmp/probe"],
            vec!["--profile-bundle-probe", "/tmp/probe", "/tmp/defaults", &"a".repeat(64)],
            vec!["--service-fixture", "/tmp/probe", "/tmp/service", "/tmp/helper"],
        ] {
            let mut invalid = args.clone();
            invalid.extend(extra.iter().map(|value| value.to_string()));
            assert!(run_options(&invalid).is_err(), "{extra:?}");
        }
        args.extend(["--capture-ui", "/tmp/ui"].map(String::from));
        assert!(run_options(&args).is_ok());
    }
}

#[cfg(test)]
mod service_capture_options_tests {
    use super::run_options;

    #[test]
    fn real_daemon_capture_requires_graphical_ui_and_single_nonempty_input() {
        let args = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--capture-ui",
            "/tmp/ui",
            "--capture-user-service",
            "/tmp/daemon",
        ]
        .map(str::to_owned);
        assert_eq!(run_options(&args).unwrap().capture_user_service, Some("/tmp/daemon"));
        for invalid in [
            vec!["vm-lab", "run", "assets", "run", "--capture-user-service", "/tmp/daemon"],
            vec![
                "vm-lab",
                "run",
                "assets",
                "run",
                "--capture-ui",
                "/tmp/ui",
                "--capture-user-service",
                "",
            ],
            vec![
                "vm-lab",
                "run",
                "assets",
                "run",
                "--capture-ui",
                "/tmp/ui",
                "--capture-user-service",
            ],
            vec![
                "vm-lab",
                "run",
                "assets",
                "run",
                "--capture-ui",
                "/tmp/ui",
                "--capture-user-service",
                "/tmp/daemon",
                "--console-only",
                "--state-probe",
                "/tmp/probe",
            ],
            vec![
                "vm-lab",
                "run",
                "assets",
                "run",
                "--capture-ui",
                "/tmp/ui",
                "--capture-user-service",
                "/tmp/daemon",
                "--capture-user-service",
                "/tmp/other",
            ],
        ] {
            assert!(
                run_options(&invalid.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_fixture_requires_complete_unique_inputs_and_initialized_console() {
        let base = [
            "vm-lab",
            "preflight",
            "assets",
            "run",
            "--console-only",
            "--state-initializer",
            "init",
        ]
        .map(str::to_owned);
        let mut args = base.to_vec();
        args.extend(["--service-fixture", "fixture", "service"].map(str::to_owned));
        assert!(run_options(&args).is_err());
        args.push("helper".into());
        assert_eq!(
            run_options(&args).unwrap().service_fixture,
            Some(("fixture", "service", "helper"))
        );
        let mut duplicate = args.clone();
        duplicate.extend(args[7..].iter().cloned());
        assert!(run_options(&duplicate).is_err());
        args.remove(4);
        assert!(run_options(&args).is_err());
        let no_init = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--console-only",
            "--service-fixture",
            "fixture",
            "service",
            "helper",
        ]
        .map(str::to_owned);
        assert!(run_options(&no_init).is_err());
    }

    #[test]
    fn profile_probe_requires_complete_inputs_canonical_digest_and_initialized_console() {
        let mut arguments = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--console-only",
            "--state-initializer",
            "init",
            "--profile-bundle-probe",
            "probe",
            "defaults.rpm",
        ]
        .map(str::to_owned)
        .to_vec();
        assert!(run_options(&arguments).is_err());
        arguments.push("a".repeat(64));
        assert_eq!(
            run_options(&arguments).unwrap().profile_bundle,
            Some((
                "probe",
                "defaults.rpm",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ))
        );
        let mut duplicate = arguments.clone();
        duplicate.extend(arguments[7..].iter().cloned());
        assert!(run_options(&duplicate).is_err());
        arguments[10] = "A".repeat(64);
        assert!(run_options(&arguments).is_err());
        arguments[10] = "a".repeat(64);
        arguments.remove(4);
        assert!(run_options(&arguments).is_err());
        let mut missing_init = [
            "vm-lab",
            "run",
            "assets",
            "run",
            "--console-only",
            "--profile-bundle-probe",
            "probe",
            "defaults.rpm",
        ]
        .map(str::to_owned)
        .to_vec();
        missing_init.push("a".repeat(64));
        assert!(run_options(&missing_init).is_err());
    }

    #[test]
    fn accelerator_options_default_to_tcg_and_reject_duplicates_or_cloud_kvm() {
        let base = ["vm-lab", "preflight", "assets", "run"].map(str::to_owned);
        assert_eq!(
            run_options(&base).unwrap().accelerator.unwrap_or_default(),
            accelerator::Accelerator::Tcg
        );
        let mut arguments = base.to_vec();
        arguments.extend(["--accelerator", "kvm"].map(str::to_owned));
        assert_eq!(run_options(&arguments).is_ok(), runner::WSL);
        arguments.extend(["--accelerator", "tcg"].map(str::to_owned));
        assert!(run_options(&arguments).is_err());
        let mut arguments = base.to_vec();
        arguments.push("--accelerator".into());
        assert!(run_options(&arguments).is_err());
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
