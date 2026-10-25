//! Session lifecycle only. This does not implement or impersonate the upstream
//! shell daemon protocol, configuration materializer, package manager or locker.
use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const PAYLOAD: &str = "/usr/share/ryoku";
const QML_MODULES: &str = "/usr/lib64/qt6/qml";
const WRITER: &str = "/usr/libexec/ryoku/ryoku-writer";
const QUICKSHELL: &str = "/usr/libexec/ryoku/quickshell/bin/qs";
const QUICKSHELL_BIN: &str = "/usr/libexec/ryoku/quickshell/bin";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_TIMEOUT: Duration = Duration::from_secs(20);
const START_TIMEOUT: Duration = Duration::from_secs(20);
const CONTROL_REAP_TIMEOUT: Duration = Duration::from_secs(1);
const SESSION_BIN: &str = "/usr/libexec/ryoku/session/bin";
const CLIPBOARD_KEEPER: &str = "/usr/libexec/ryoku/session/upstream/wl-clip-persist";
const LOCK_UNIT: &str = "ryoku-session-lock.service";
const LOGOUT_UNIT: &str = "ryoku-session-logout.service";
const CONFIG: &str = "/usr/share/ryoku-opensuse/session/hyprland.conf";
const TARGET: &str = "ryoku-desktop-session.target";
const REQUIRED_SERVICES: &[&str] = &["ryoku-desktop-shell.service", "ryoku-service.service"];
const GRAPHICAL: &str = "graphical-session.target";
const SURFACE_MODULES: &[&str] =
    &["Ui", "FrameBars", "PluginKit", "Blobs", "Spatial3D", "Wm/Hyprland"];
const SESSION_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_ID",
    "QT_QPA_PLATFORM",
    "LIBGL_ALWAYS_SOFTWARE",
];

#[derive(Debug, PartialEq)]
enum Action {
    Compositor { software: bool },
    Activate,
    Shell,
    Hub,
    Screenshot,
    Logout,
    FinishLogout,
    Lock,
    ClipboardKeeper,
    ShellCommand(Vec<OsString>),
    ProviderCommand(Vec<OsString>),
    Open(Surface),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Surface {
    Launcher,
    QuickSettings,
    Clipboard,
    Notifications,
}

impl Surface {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "launcher" => Ok(Self::Launcher),
            "quicksettings" => Ok(Self::QuickSettings),
            "clipboard" => Ok(Self::Clipboard),
            "notifications" => Ok(Self::Notifications),
            _ => {
                Err("supported surfaces: launcher, quicksettings, clipboard, notifications".into())
            }
        }
    }

    fn upstream_id(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::QuickSettings => "quick-settings",
            Self::Clipboard => "clipboard",
            // The pinned notification rail opens this module of quick settings.
            // Its NotificationServer/history is native Quickshell, not a topic
            // requiring the unported ryoku-shell daemon.
            Self::Notifications => "quick-settings#notifications",
        }
    }
}

fn parse(args: &[String]) -> Result<Action> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["compositor"] => Ok(Action::Compositor { software: false }),
        ["compositor", "--software-rendering"] => Ok(Action::Compositor { software: true }),
        ["activate"] => Ok(Action::Activate),
        ["shell"] => Ok(Action::Shell),
        ["hub"] => Ok(Action::Hub),
        ["screenshot"] => Ok(Action::Screenshot),
        ["logout"] => Ok(Action::Logout),
        ["lock"] => Ok(Action::Lock),
        ["open", surface] => Ok(Action::Open(Surface::parse(surface)?)),
        _ => Err("usage: ryoku-session compositor [--software-rendering] | activate | shell | hub | screenshot | lock | logout | open launcher|quicksettings|clipboard|notifications".into()),
    }
}

fn dispatch(executable: &OsStr, arguments: Vec<OsString>) -> Result<Action> {
    match Path::new(executable).file_name().and_then(OsStr::to_str) {
        Some("ryoku-shell")
            if arguments.first().is_some_and(|arg| arg == "lock" || arg == "logout") =>
        {
            if arguments.len() == 1 {
                Ok(if arguments[0] == "lock" { Action::Lock } else { Action::Logout })
            } else {
                Err("lock and logout take no arguments".into())
            }
        }
        Some("ryoku-shell") => Ok(Action::ShellCommand(arguments)),
        Some("ryoku-session-logout") => {
            if arguments.is_empty() {
                Ok(Action::FinishLogout)
            } else {
                Err("session logout worker takes no arguments".into())
            }
        }
        Some("ryoku-wm-hyprland") => {
            if arguments.starts_with(&["act".into(), "session.exit".into()]) {
                if arguments.len() == 2 {
                    Ok(Action::Logout)
                } else {
                    Err("session.exit takes no arguments".into())
                }
            } else {
                Ok(Action::ProviderCommand(arguments))
            }
        }
        Some("wl-clip-persist") => {
            if arguments == ["--clipboard", "regular"] {
                Ok(Action::ClipboardKeeper)
            } else {
                Err("session clipboard keeper supports only --clipboard regular".into())
            }
        }
        _ => parse(
            &arguments
                .into_iter()
                .map(|value| value.into_string().map_err(|_| "session action must be UTF-8".into()))
                .collect::<Result<Vec<_>>>()?,
        ),
    }
}

fn checked(mut command: Command) -> Result<()> {
    let program = command.get_program().to_owned();
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{} failed: {status}", program.to_string_lossy()).into());
    }
    Ok(())
}

// Bound only short control/IPC clients, never the compositor or lock lifetime.
// A fresh process group keeps a timed-out client's descendants in the same
// cleanup boundary; the direct child remains unreaped until its group is killed.
fn checked_control(mut command: Command, timeout: Duration) -> Result<()> {
    let program = command.get_program().to_owned();
    let mut child = command.process_group(0).spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("{} failed: {status}", program.to_string_lossy()).into())
            };
        }
        if Instant::now() >= deadline {
            let group = rustix::process::Pid::from_raw(child.id() as i32)
                .ok_or("invalid control process identity")?;
            // A zombie still reserves its PID until this owner reaps it, so
            // this freshly created group cannot become an unrelated group.
            let killed = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            let reaped_by = Instant::now() + CONTROL_REAP_TIMEOUT;
            loop {
                if child.try_wait()?.is_some() {
                    break;
                }
                if Instant::now() >= reaped_by {
                    // An uninterruptible kernel task must not consume the
                    // reserved compositor-exit budget. SIGKILL remains pending;
                    // a detached owner reaps it when the kernel permits exit.
                    thread::Builder::new().name("ryoku-session-control-reaper".into()).spawn(
                        move || {
                            let _ = child.wait();
                        },
                    )?;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            if let Err(error) = killed
                && error != rustix::io::Errno::SRCH
            {
                return Err(format!(
                    "{} timed out; control group termination failed: {error}",
                    program.to_string_lossy()
                )
                .into());
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{} exceeded its control deadline", program.to_string_lossy()),
            )
            .into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn systemctl(args: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/systemctl");
    command.args(["--user", "--no-ask-password"]).args(args).stdin(Stdio::null());
    command
}

fn activate() -> Result<()> {
    require_current_compositor()?;
    activate_using(&mut checked_control, || {
        validate_surface_payload(Path::new(PAYLOAD), Path::new(QML_MODULES))
    })
}

fn activate_using(
    execute: &mut impl FnMut(Command, Duration) -> Result<()>,
    validate_payload: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let present: Vec<_> = SESSION_ENV.iter().filter(|name| env::var_os(name).is_some()).collect();
    let mut import = Command::new("/usr/bin/dbus-update-activation-environment");
    import.arg("--systemd").args(&present);
    let outcome = clear_activation_environment_using(execute)
        .and_then(|()| execute(import, CONTROL_TIMEOUT))
        .and_then(|()| validate_payload())
        .and_then(|()| start_session_target_using(execute));
    if outcome.is_err()
        && let Err(error) = finish_logout_using(execute)
    {
        eprintln!("ryoku-session: activation cleanup failed: {error}");
    }
    outcome
}

fn validate_compositor_environment(
    wayland: Option<&OsStr>,
    instance: Option<&OsStr>,
) -> Result<()> {
    for (name, value) in [("WAYLAND_DISPLAY", wayland), ("HYPRLAND_INSTANCE_SIGNATURE", instance)] {
        if value.is_none_or(OsStr::is_empty) {
            return Err(format!("session action requires the compositor's {name}").into());
        }
    }
    Ok(())
}

fn require_current_compositor() -> Result<()> {
    validate_compositor_environment(
        env::var_os("WAYLAND_DISPLAY").as_deref(),
        env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
    )
}

fn exit_compositor_command() -> Command {
    let mut command = Command::new("/usr/bin/hyprctl");
    command.args(["dispatch", "exit"]).stdin(Stdio::null());
    command
}

fn start_session_target_using(
    execute: &mut impl FnMut(Command, Duration) -> Result<()>,
) -> Result<()> {
    // A fresh manager need not have loaded this unit: retain its burst limits.
    execute(systemctl(&["start", TARGET]), START_TIMEOUT)?;
    // is-active with several units succeeds when *any* is active. Each
    // required partner must pass its own bounded readiness check.
    for unit in REQUIRED_SERVICES {
        execute(systemctl(&["is-active", "--quiet", unit]), CONTROL_TIMEOUT)?;
    }
    Ok(())
}

fn clear_bus_environment_command() -> Command {
    // D-Bus UpdateActivationEnvironment cannot remove a key. Empty its value
    // explicitly, then remove the same key from systemd's separate environment.
    let mut command = Command::new("/usr/bin/dbus-update-activation-environment");
    command.arg("--systemd").args(SESSION_ENV.iter().map(|name| format!("{name}=")));
    command
}

fn clear_activation_environment_using(
    execute: &mut impl FnMut(Command, Duration) -> Result<()>,
) -> Result<()> {
    let bus = execute(clear_bus_environment_command(), CONTROL_TIMEOUT);
    let mut clear = systemctl(&["unset-environment"]);
    clear.args(SESSION_ENV);
    // Both cleanups run even if the first client failed or timed out.
    let manager = execute(clear, CONTROL_TIMEOUT);
    bus.and(manager)
}

fn stop_session_using(execute: &mut impl FnMut(Command, Duration) -> Result<()>) -> Result<()> {
    // Stop portals while Wayland still exists. Keep enough of the logout unit's
    // 45s budget for both environment cleanups and compositor exit on failure.
    let stopped = execute(systemctl(&["stop", TARGET, GRAPHICAL]), STOP_TIMEOUT);
    let cleared = clear_activation_environment_using(execute);
    stopped.and(cleared)
}

fn stop_session() -> Result<()> {
    stop_session_using(&mut checked_control)
}

fn finish_logout_using(execute: &mut impl FnMut(Command, Duration) -> Result<()>) -> Result<()> {
    let stopped = stop_session_using(execute);
    let exited = execute(exit_compositor_command(), CONTROL_TIMEOUT);
    stopped.and(exited)
}

fn validate_surface_payload(root: &Path, modules: &Path) -> Result<()> {
    for relative in ["shell/quickshell/shell/shell.qml", "hub/quickshell/shell.qml"] {
        if !root.join(relative).is_file() {
            return Err(format!("preserved upstream entrypoint missing: {relative}").into());
        }
    }
    for module in SURFACE_MODULES {
        if !modules.join("Ryoku").join(module).join("qmldir").is_file() {
            return Err(format!("upstream Ryoku.{module} module missing from payload").into());
        }
    }
    Ok(())
}

fn surface_environment(command: &mut Command, root: &Path, modules: &Path) -> Result<()> {
    validate_surface_payload(root, modules)?;
    let imports = env::join_paths([modules.to_path_buf(), root.join("shell/quickshell")])?;
    // Pin child lookup only; never import PATH into the shared user manager.
    command
        .env("QML_IMPORT_PATH", &imports)
        .env("QML2_IMPORT_PATH", &imports)
        .env("RYOKU_SHELL_DIR", root.join("shell"))
        .env("RYOKU_I18N_DIR", root.join("i18n/catalog"))
        .env("PATH", format!("{SESSION_BIN}:{QUICKSHELL_BIN}:/usr/bin:/bin"))
        .env("QT_QPA_PLATFORM", "wayland")
        .env("RYOKU_POLKIT_AGENT", "0")
        .env("QT_MEDIA_BACKEND", "ffmpeg")
        .env("QT_FFMPEG_DECODING_HW_DEVICE_TYPES", "");
    Ok(())
}

fn surface_command(hub: bool) -> Result<Command> {
    surface_command_at(hub, Path::new(PAYLOAD), Path::new(QML_MODULES))
}

fn surface_command_at(hub: bool, root: &Path, modules: &Path) -> Result<Command> {
    let arguments = if hub { vec!["hub".into(), "open".into()] } else { vec!["daemon".into()] };
    daemon_command_at(&arguments, root, modules)
}

fn daemon_command_at(arguments: &[OsString], root: &Path, modules: &Path) -> Result<Command> {
    let mut command = Command::new("/usr/bin/setpriv");
    command.args(["--no-new-privs", WRITER, "daemon"]);
    // The existing writer coordinator owns the daemon and its entire surface
    // tree. Hub requests enter the same pinned daemon through its native CLI.
    command.args(arguments);
    surface_environment(&mut command, root, modules)?;
    Ok(command)
}

fn provider_command(arguments: &[OsString]) -> Command {
    let mut command = Command::new("/usr/bin/setpriv");
    command.args(["--no-new-privs", WRITER, "provider"]).args(arguments);
    command
}

fn clipboard_keeper_command() -> Command {
    let mut command = Command::new(CLIPBOARD_KEEPER);
    command
        .arg0("wl-clip-persist")
        .args([
            "--clipboard",
            "regular",
            "--selection-size-limit",
            "10485760",
            "--disable-timestamps",
        ])
        .env("RUST_LOG", "warn");
    command
}

fn screenshot_command_at(root: &Path, modules: &Path) -> Result<Command> {
    let mut command = Command::new("/usr/bin/setpriv");
    command.args(["--no-new-privs", WRITER, "ryoshot"]);
    surface_environment(&mut command, root, modules)?;
    Ok(command)
}

fn run(action: Action) -> Result<()> {
    use rustix::process::{getegid, geteuid, getgid, getuid};
    validate_identity(
        getuid().as_raw(),
        geteuid().as_raw(),
        getgid().as_raw(),
        getegid().as_raw(),
    )?;
    match action {
        Action::Shell => Err(surface_command(false)?.exec().into()),
        Action::Hub => Err(surface_command(true)?.exec().into()),
        Action::Screenshot => {
            Err(screenshot_command_at(Path::new(PAYLOAD), Path::new(QML_MODULES))?.exec().into())
        }
        Action::Logout => {
            require_current_compositor()?;
            // Surface requests arrive from the shell's cgroup. A separate
            // fixed unit must outlive stopping that cgroup and its portals.
            checked_control(systemctl(&["start", "--no-block", LOGOUT_UNIT]), CONTROL_TIMEOUT)
        }
        Action::FinishLogout => {
            // Never let hyprctl choose a different graphical login when our
            // session environment has already disappeared from the manager.
            require_current_compositor()?;
            finish_logout_using(&mut checked_control)
        }
        // Waiting covers the complete native lock/unlock lifetime. This does
        // not return a fabricated secure-lock acknowledgement for suspend.
        Action::Lock => checked(systemctl(&["start", "--wait", LOCK_UNIT])),
        Action::ClipboardKeeper => Err(clipboard_keeper_command().exec().into()),
        Action::ShellCommand(arguments) => {
            Err(daemon_command_at(&arguments, Path::new(PAYLOAD), Path::new(QML_MODULES))?
                .exec()
                .into())
        }
        Action::ProviderCommand(arguments) => Err(provider_command(&arguments).exec().into()),
        Action::Open(surface) => open_surface_using(
            surface,
            Path::new(PAYLOAD),
            Path::new(QML_MODULES),
            &mut checked_control,
        ),
        Action::Activate => activate(),
        Action::Compositor { software } => {
            if env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()) {
                return Err("full session requires a separate display-manager login".into());
            }
            if !Path::new(CONFIG).is_file() {
                return Err("packaged Ryoku compositor configuration missing".into());
            }
            // Stop only Ryoku's target: KDE and the shared graphical-session
            // target remain owned by their native session managers.
            checked_control(systemctl(&["stop", TARGET]), STOP_TIMEOUT)?;
            let outcome = checked(compositor_command(software));
            let cleanup = stop_session();
            outcome.and(cleanup)
        }
    }
}

fn open_surface_using(
    surface: Surface,
    root: &Path,
    modules: &Path,
    execute: &mut impl FnMut(Command, Duration) -> Result<()>,
) -> Result<()> {
    let mut command = Command::new("/usr/bin/setpriv");
    // Calling existing IPC navigates the coordinated daemon's shell; it does
    // not launch a second, unregistered Quickshell writer.
    command
        .args(["--no-new-privs", QUICKSHELL, "-p"])
        .arg(root.join("shell/quickshell/shell"))
        .args(["ipc", "call", "shell", "openSurface", "", surface.upstream_id()]);
    surface_environment(&mut command, root, modules)?;
    execute(command, CONTROL_TIMEOUT)
}

fn compositor_command(software: bool) -> Command {
    let mut command = Command::new("/usr/bin/Hyprland");
    command
        .args(["--config", CONFIG])
        .env("XDG_CURRENT_DESKTOP", "Hyprland")
        .env("XDG_SESSION_DESKTOP", "ryoku-opensuse")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("QT_QPA_PLATFORM", "wayland")
        // Suppress Compositor.cpp's asynchronous importer and cleanup. The
        // pinned Executor.cpp has a second first-frame importer that ignores
        // this flag; do not claim exclusive ownership of startup environment.
        .env("HYPRLAND_NO_SD_VARS", "1");
    if software {
        command.env("LIBGL_ALWAYS_SOFTWARE", "true");
    }
    command
}

fn validate_identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<()> {
    if uid == 0 || uid != euid || gid != egid {
        return Err(
            "Ryoku session requires a non-root user with matching real/effective IDs".into()
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let executable = arguments.next().unwrap_or_default();
    match dispatch(&executable, arguments.collect()).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ryoku-session: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(input: &[&str]) -> Vec<String> {
        input.iter().map(|s| s.to_string()).collect()
    }

    const CONTROL_FIXTURE: &str = "tests::lifecycle_control_process_fixture";
    fn fixture_command(role: &str) -> Command {
        let mut command = Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", CONTROL_FIXTURE, "--test-threads=1"])
            .env("RYOKU_SESSION_TEST_CONTROL", role)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn fixture_surface_payload(root: &Path) -> std::path::PathBuf {
        let modules = root.join("qt6/qml");
        for relative in ["shell/quickshell/shell/shell.qml", "hub/quickshell/shell.qml"] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "synthetic entrypoint").unwrap();
        }
        for module in SURFACE_MODULES {
            let path = modules.join("Ryoku").join(module);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("qmldir"), "synthetic module").unwrap();
        }
        modules
    }

    #[test]
    fn lifecycle_control_process_fixture() {
        let Ok(role) = env::var("RYOKU_SESSION_TEST_CONTROL") else { return };
        match role.as_str() {
            "success" => (),
            "failure" => std::process::exit(23),
            "signal" => {
                rustix::process::kill_process(
                    rustix::process::getpid(),
                    rustix::process::Signal::TERM,
                )
                .unwrap();
                panic!("fixture signal was ignored");
            }
            "blocked" => thread::sleep(Duration::from_secs(10)),
            "blocked-tree" => {
                let path = env::var_os("RYOKU_SESSION_TEST_CONTROL_DIR").unwrap();
                let mut child = fixture_command("blocked").spawn().unwrap();
                std::fs::write(
                    Path::new(&path).join("children"),
                    format!("{} {}", std::process::id(), child.id()),
                )
                .unwrap();
                child.wait().unwrap();
            }
            _ => panic!("unknown fixture role"),
        }
    }

    fn control_step(command: &Command) -> &'static str {
        let arguments: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
        match command.get_program().to_str().unwrap() {
            "/usr/bin/dbus-update-activation-environment" => {
                if arguments.iter().any(|arg| arg.ends_with('=')) { "clear-bus" } else { "import" }
            }
            "/usr/bin/hyprctl" => {
                assert_eq!(arguments, ["dispatch", "exit"]);
                "exit"
            }
            "/usr/bin/systemctl" => {
                assert_eq!(&arguments[..2], ["--user", "--no-ask-password"]);
                match arguments[2] {
                    "stop" => "stop",
                    "unset-environment" => "clear-manager",
                    "start" => "start",
                    "is-active" => {
                        assert_eq!(arguments.len(), 5);
                        assert_eq!(arguments[3], "--quiet");
                        match arguments[4] {
                            "ryoku-desktop-shell.service" => "check-shell",
                            "ryoku-service.service" => "check-service",
                            _ => panic!("unreviewed required session unit"),
                        }
                    }
                    _ => panic!("unknown control command"),
                }
            }
            _ => panic!("unreviewed lifecycle executable"),
        }
    }

    #[test]
    fn logout_attempts_both_cleanups_and_exit_after_every_control_failure_or_timeout() {
        let expected = ["stop", "clear-bus", "clear-manager", "exit"];
        for failure in expected {
            for timeout in [false, true] {
                let mut observed = Vec::new();
                let error = finish_logout_using(&mut |command, limit| {
                    let step = control_step(&command);
                    observed.push(step);
                    assert_eq!(limit, if step == "stop" { STOP_TIMEOUT } else { CONTROL_TIMEOUT });
                    let role = if step == failure {
                        if timeout { "blocked" } else { "failure" }
                    } else {
                        "success"
                    };
                    // Only the intentionally blocked fixture needs a short
                    // deadline; normal child startup must tolerate busy CI.
                    let fixture_deadline = if step == failure && timeout {
                        Duration::from_millis(40)
                    } else {
                        Duration::from_secs(2)
                    };
                    checked_control(fixture_command(role), fixture_deadline)
                })
                .unwrap_err();
                assert_eq!(observed, expected, "failed step={failure}, timeout={timeout}");
                if timeout {
                    assert!(error.to_string().contains("deadline"));
                } else {
                    assert!(error.to_string().contains("23"));
                }
            }
        }
    }

    #[test]
    fn activation_failure_never_skips_shutdown_or_exit_and_missing_payload_never_starts() {
        let startup =
            ["clear-bus", "clear-manager", "import", "start", "check-shell", "check-service"];
        for failed in startup {
            let mut observed = Vec::new();
            let mut failed_once = false;
            let result = activate_using(
                &mut |command, _| {
                    let step = control_step(&command);
                    observed.push(step);
                    let role = if step == failed && !failed_once {
                        failed_once = true;
                        "failure"
                    } else {
                        "success"
                    };
                    checked_control(fixture_command(role), Duration::from_secs(2))
                },
                || Ok(()),
            );
            assert!(result.is_err());
            assert_eq!(
                &observed[observed.len() - 4..],
                ["stop", "clear-bus", "clear-manager", "exit"]
            );
            if !failed.starts_with("check-") {
                assert!(!observed.contains(&"check-shell"));
                assert!(!observed.contains(&"check-service"));
            }
        }
        let mut observed = Vec::new();
        let error = activate_using(
            &mut |command, _| {
                observed.push(control_step(&command));
                checked_control(fixture_command("success"), Duration::from_secs(2))
            },
            || Err("missing fixed payload".into()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "missing fixed payload");
        assert!(!observed.contains(&"start"));
        assert_eq!(&observed[observed.len() - 4..], ["stop", "clear-bus", "clear-manager", "exit"]);
    }

    #[test]
    fn successful_activation_does_not_exit_and_control_signal_failure_is_reported() {
        let mut observed = Vec::new();
        activate_using(
            &mut |command, _| {
                observed.push(control_step(&command));
                checked_control(fixture_command("success"), Duration::from_secs(2))
            },
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            observed,
            ["clear-bus", "clear-manager", "import", "start", "check-shell", "check-service"]
        );
        assert!(checked_control(fixture_command("signal"), Duration::from_secs(2)).is_err());
    }

    #[test]
    fn startup_requires_every_partner_and_one_active_unit_never_masks_the_other() {
        let units = ["ryoku-desktop-shell.service", "ryoku-service.service"];
        for active in [[true, true], [true, false], [false, true], [false, false]] {
            for timeout in [false, true] {
                let mut observed = Vec::new();
                let mut queries = Vec::new();
                let result = activate_using(
                    &mut |command, limit| {
                        let arguments: Vec<_> =
                            command.get_args().map(|value| value.to_str().unwrap()).collect();
                        let query = command.get_program() == "/usr/bin/systemctl"
                            && arguments.get(2) == Some(&"is-active");
                        let role = if query {
                            assert_eq!(limit, CONTROL_TIMEOUT);
                            // Emulate the real systemctl any-unit exit contract.
                            // A grouped query would falsely succeed for either
                            // one-active case; the production fix must split it.
                            let requested = arguments[4..].to_vec();
                            let any_active = requested.iter().any(|unit| {
                                active[units.iter().position(|known| known == unit).unwrap()]
                            });
                            queries
                                .push(requested.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                            observed.push("check");
                            if any_active {
                                "success"
                            } else if timeout {
                                "blocked"
                            } else {
                                "failure"
                            }
                        } else {
                            observed.push(control_step(&command));
                            "success"
                        };
                        checked_control(
                            fixture_command(role),
                            if role == "blocked" {
                                Duration::from_millis(40)
                            } else {
                                Duration::from_secs(2)
                            },
                        )
                    },
                    || Ok(()),
                );
                assert_eq!(
                    result.is_ok(),
                    active.iter().all(|value| *value),
                    "active={active:?}, timeout={timeout}, queries={queries:?}"
                );
                assert!(queries.iter().all(|requested| requested.len() == 1));
                if active == [true, true] {
                    assert_eq!(queries, units.map(|unit| vec![unit.to_string()]));
                    assert!(!observed.contains(&"exit"));
                } else {
                    assert_eq!(
                        &observed[observed.len() - 4..],
                        ["stop", "clear-bus", "clear-manager", "exit"]
                    );
                    let error = result.unwrap_err().to_string();
                    assert!(error.contains(if timeout { "deadline" } else { "23" }));
                }
            }
        }
    }

    #[test]
    fn navigation_timeout_reaps_only_its_client_tree_and_preserves_existing_server() {
        let root = env::temp_dir().join(format!("ryoku25-control-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let modules = fixture_surface_payload(&root);
        let mut unrelated = fixture_command("blocked").spawn().unwrap();
        let started = Instant::now();
        let result =
            open_surface_using(Surface::Launcher, &root, &modules, &mut |command, limit| {
                assert_eq!(command.get_program(), "/usr/bin/setpriv");
                assert_eq!(limit, CONTROL_TIMEOUT);
                let mut client = fixture_command("blocked-tree");
                client.env("RYOKU_SESSION_TEST_CONTROL_DIR", &root);
                checked_control(client, limit)
            });
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(
            started.elapsed() < CONTROL_TIMEOUT + CONTROL_REAP_TIMEOUT + Duration::from_secs(1)
        );
        let pids = std::fs::read_to_string(root.join("children")).unwrap();
        let mut processes = pids.split_whitespace();
        let client = processes.next().unwrap();
        assert!(
            std::fs::metadata(format!("/proc/{client}")).is_err(),
            "timed-out client was not reaped"
        );
        for pid in processes {
            let state = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            assert!(
                state.is_err() || state.unwrap().rsplit_once(") ").unwrap().1.starts_with('Z'),
                "control descendant survived its deadline"
            );
        }
        assert!(unrelated.try_wait().unwrap().is_none(), "timeout signaled the existing server");
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn all_navigation_ipc_requests_keep_fixed_arguments_and_success_failure_results() {
        let root = env::temp_dir().join(format!("ryoku25-navigation-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let modules = fixture_surface_payload(&root);
        for surface in
            [Surface::Launcher, Surface::QuickSettings, Surface::Clipboard, Surface::Notifications]
        {
            for role in ["success", "failure", "blocked"] {
                let result = open_surface_using(surface, &root, &modules, &mut |command, limit| {
                    assert_eq!(limit, CONTROL_TIMEOUT);
                    assert_eq!(command.get_program(), "/usr/bin/setpriv");
                    let expected: Vec<OsString> = vec![
                        "--no-new-privs".into(),
                        QUICKSHELL.into(),
                        "-p".into(),
                        root.join("shell/quickshell/shell").into_os_string(),
                        "ipc".into(),
                        "call".into(),
                        "shell".into(),
                        "openSurface".into(),
                        "".into(),
                        surface.upstream_id().into(),
                    ];
                    assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
                    checked_control(
                        fixture_command(role),
                        if role == "blocked" { Duration::from_millis(25) } else { limit },
                    )
                });
                if role == "success" {
                    assert!(result.is_ok());
                } else {
                    assert!(result.unwrap_err().to_string().contains(if role == "blocked" {
                        "deadline"
                    } else {
                        "23"
                    }));
                }
            }
        }
        std::fs::remove_file(modules.join("Ryoku/Spatial3D/qmldir")).unwrap();
        let mut called = false;
        assert!(
            open_surface_using(Surface::Clipboard, &root, &modules, &mut |_, _| {
                called = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!called, "missing required payload still dispatched navigation IPC");
        std::fs::remove_dir_all(root).unwrap();
    }

    // Interpret the trusted metadata's whitespace lists, including systemd's
    // empty-value reset, without starting or substituting a service manager.
    fn metadata_words<'a>(text: &'a str, section: &str, key: &str) -> Vec<&'a str> {
        let mut current = "";
        let mut words = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
                current = name;
                continue;
            }
            if current == section
                && let Some((name, value)) = line.split_once('=')
                && name.trim() == key
            {
                if value.trim().is_empty() {
                    words.clear();
                } else {
                    words.extend(value.split_whitespace());
                }
            }
        }
        words
    }

    #[test]
    fn packaged_entrypoint_target_and_independent_readiness_checks_cannot_drift() {
        let entry = include_str!("../../../packaging/rpm/wayland-sessions/ryoku-opensuse.desktop");
        let target =
            include_str!("../../../packaging/rpm/systemd/user/ryoku-desktop-session.target");
        let shell = include_str!("../../../packaging/rpm/systemd/user/ryoku-desktop-shell.service");
        let service = include_str!(
            "../../../packaging/rpm/systemd/user/ryoku-service.service.d/25-session.conf"
        );
        let spec = include_str!("../../../packaging/rpm/ryoku-session.spec");
        assert_eq!(
            metadata_words(entry, "Desktop Entry", "Exec"),
            ["/usr/bin/ryoku-session", "compositor"]
        );
        assert_eq!(metadata_words(entry, "Desktop Entry", "TryExec"), ["/usr/bin/ryoku-session"]);
        assert_eq!(metadata_words(entry, "Desktop Entry", "Type"), ["Application"]);
        assert_eq!(metadata_words(entry, "Desktop Entry", "DesktopNames"), ["Hyprland"]);
        assert_eq!(parse(&args(&["compositor"])).unwrap(), Action::Compositor { software: false });
        let required: std::collections::BTreeSet<_> =
            metadata_words(target, "Unit", "Requires").into_iter().collect();
        assert_eq!(required, REQUIRED_SERVICES.iter().copied().collect());
        let after = metadata_words(target, "Unit", "After");
        assert!(required.iter().all(|unit| after.contains(unit)));
        assert_eq!(metadata_words(target, "Unit", "BindsTo"), [GRAPHICAL]);
        assert_eq!(metadata_words(target, "Unit", "Before"), [GRAPHICAL]);
        assert_eq!(metadata_words(shell, "Unit", "PartOf"), [TARGET]);
        assert_eq!(metadata_words(service, "Unit", "PartOf"), [TARGET]);
        assert_eq!(
            metadata_words(shell, "Service", "ExecStart"),
            ["/usr/bin/ryoku-session", "shell"]
        );
        assert_eq!(metadata_words(shell, "Unit", "ConditionEnvironment"), ["WAYLAND_DISPLAY"]);
        assert_eq!(metadata_words(shell, "Service", "NoNewPrivileges"), ["yes"]);
        assert_eq!(metadata_words(shell, "Service", "KillMode"), ["control-group"]);
        let mut checked = Vec::new();
        start_session_target_using(&mut |command, limit| {
            assert_eq!(command.get_program(), "/usr/bin/systemctl");
            let values: Vec<_> = command.get_args().map(|value| value.to_str().unwrap()).collect();
            assert_eq!(&values[..2], ["--user", "--no-ask-password"]);
            if values[2] == "start" {
                assert_eq!(&values[3..], [TARGET]);
                assert_eq!(limit, START_TIMEOUT);
            } else {
                assert_eq!(values[2], "is-active");
                assert_eq!(values[3], "--quiet");
                assert_eq!(values.len(), 5);
                assert_eq!(limit, CONTROL_TIMEOUT);
                checked.push(values[4].to_string());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            checked.iter().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
            required
        );
        assert_eq!(checked.len(), required.len());
        assert!(spec.contains("%{_datadir}/wayland-sessions/ryoku-opensuse.desktop"));
        assert!(
            !spec.contains("wayland-sessions/plasma"),
            "session RPM must preserve KDE's independent entry"
        );
    }

    #[test]
    fn lifecycle_unit_ownership_keeps_logout_alive_and_optional_features_off_the_boot_gate() {
        let target =
            include_str!("../../../packaging/rpm/systemd/user/ryoku-desktop-session.target");
        let shell = include_str!("../../../packaging/rpm/systemd/user/ryoku-desktop-shell.service");
        let logout =
            include_str!("../../../packaging/rpm/systemd/user/ryoku-session-logout.service");
        let lock = include_str!("../../../packaging/rpm/systemd/user/ryoku-session-lock.service");
        assert_eq!(
            metadata_words(logout, "Service", "ExecStart"),
            [format!("{SESSION_BIN}/ryoku-session-logout")]
        );
        assert_eq!(metadata_words(logout, "Service", "Type"), ["oneshot"]);
        for relation in ["PartOf", "BindsTo", "Requires", "Wants"] {
            assert!(
                metadata_words(logout, "Unit", relation).is_empty(),
                "logout worker must outlive desktop stop: {relation}"
            );
        }
        assert_eq!(metadata_words(lock, "Unit", "PartOf"), [TARGET]);
        assert_eq!(metadata_words(lock, "Unit", "After"), [TARGET]);
        assert_eq!(metadata_words(lock, "Unit", "ConditionEnvironment"), ["WAYLAND_DISPLAY"]);
        assert_eq!(
            metadata_words(lock, "Service", "ExecStart"),
            ["/usr/bin/hyprlock", "--config", "/usr/share/ryoku-opensuse/session/hyprlock.conf"]
        );
        assert!(
            metadata_words(lock, "Service", "TimeoutStartSec").is_empty(),
            "lock lifetime is not a short control deadline"
        );
        for unit in [target, shell, logout, lock] {
            assert!(metadata_words(unit, "Install", "WantedBy").is_empty());
            assert!(metadata_words(unit, "Install", "RequiredBy").is_empty());
        }
        let required = metadata_words(target, "Unit", "Requires");
        assert_eq!(required.len(), 2);
        assert!(
            metadata_words(target, "Unit", "Wants").is_empty(),
            "optional providers must not add ordered startup work to the core target"
        );
        for optional in [
            LOCK_UNIT,
            LOGOUT_UNIT,
            "ryogami.service",
            "rashin.service",
            "voxtype.service",
            "bluetooth.service",
        ] {
            assert!(!required.contains(&optional));
            assert!(!metadata_words(target, "Unit", "Wants").contains(&optional));
        }
        // A metadata reset must not leave an earlier dependency in this test's
        // parsed contract and accidentally certify a now-optional core unit.
        assert_eq!(
            metadata_words(
                "[Unit]\nRequires=old.service\nRequires=\nRequires=new.service\n",
                "Unit",
                "Requires"
            ),
            ["new.service"]
        );
    }

    #[test]
    fn packaged_logout_unit_reserves_time_for_exit_after_all_control_deadlines() {
        let unit = include_str!("../../../packaging/rpm/systemd/user/ryoku-session-logout.service");
        let seconds = unit
            .lines()
            .find_map(|line| line.strip_prefix("TimeoutStartSec="))
            .unwrap()
            .strip_suffix('s')
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let worst = STOP_TIMEOUT + CONTROL_TIMEOUT * 3 + CONTROL_REAP_TIMEOUT * 4;
        assert!(worst + Duration::from_secs(5) < Duration::from_secs(seconds));
        assert!(!unit.contains("PartOf="));
        assert!(!unit.contains("BindsTo="));
    }

    #[test]
    fn rejects_unreviewed_payloads_and_extra_commands() {
        for input in [
            args(&["shell", "--payload-root", "/tmp/source"]),
            args(&["compositor", "--software-rendering", "extra"]),
            args(&["open", "notifications; reboot"]),
            args(&["open", "quick-settings#notifications"]),
            args(&["nested"]),
        ] {
            assert!(parse(&input).is_err());
        }
        assert_eq!(parse(&args(&["hub"])).unwrap(), Action::Hub);
        assert_eq!(parse(&args(&["screenshot"])).unwrap(), Action::Screenshot);
        assert_eq!(parse(&args(&["logout"])).unwrap(), Action::Logout);
        assert_eq!(parse(&args(&["lock"])).unwrap(), Action::Lock);
        assert_eq!(parse(&args(&["shell"])).unwrap(), Action::Shell);
        assert_eq!(parse(&args(&["compositor"])).unwrap(), Action::Compositor { software: false });
        assert_eq!(
            parse(&args(&["open", "notifications"])).unwrap(),
            Action::Open(Surface::Notifications)
        );
        assert_eq!(Surface::Notifications.upstream_id(), "quick-settings#notifications");
        assert_eq!(Surface::QuickSettings.upstream_id(), "quick-settings");
    }

    #[test]
    fn missing_payload_fails_before_starting_runtime() {
        let mut command = Command::new("/usr/bin/false");
        assert!(
            surface_environment(
                &mut command,
                Path::new("/nonexistent/ryoku-payload"),
                Path::new(QML_MODULES)
            )
            .is_err()
        );
    }

    #[test]
    fn session_refuses_root_and_set_id_invocation() {
        for (uid, euid, gid, egid) in
            [(0, 0, 0, 0), (1000, 0, 1000, 1000), (1000, 1001, 1000, 1000), (1000, 1000, 1000, 0)]
        {
            assert!(validate_identity(uid, euid, gid, egid).is_err());
        }
        assert!(validate_identity(1000, 1000, 100, 100).is_ok());
    }

    #[test]
    fn private_aliases_route_real_lock_requests_and_preserve_other_native_arguments() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(dispatch(OsStr::new("ryoku-shell"), vec!["lock".into()]).unwrap(), Action::Lock);
        assert!(dispatch(OsStr::new("ryoku-shell"), vec!["lock".into(), "extra".into()]).is_err());
        assert_eq!(
            dispatch(OsStr::new("ryoku-shell"), vec!["logout".into()]).unwrap(),
            Action::Logout
        );
        assert!(
            dispatch(OsStr::new("ryoku-shell"), vec!["logout".into(), "extra".into()]).is_err()
        );
        let literal = OsString::from_vec(vec![b'a', 0xff, b';', b'b']);
        assert_eq!(
            dispatch(OsStr::new("ryoku-shell"), vec!["open".into(), literal.clone()]).unwrap(),
            Action::ShellCommand(vec!["open".into(), literal])
        );
        assert_eq!(
            dispatch(OsStr::new("wl-clip-persist"), vec!["--clipboard".into(), "regular".into()])
                .unwrap(),
            Action::ClipboardKeeper
        );
        for args in
            [vec![], vec!["--clipboard", "primary"], vec!["--clipboard", "regular", "--help"]]
        {
            assert!(
                dispatch(
                    OsStr::new("wl-clip-persist"),
                    args.into_iter().map(OsString::from).collect()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn shell_provider_logout_outlives_its_cgroup_and_other_calls_keep_writer_coordination() {
        let logout = vec!["act".into(), "session.exit".into()];
        assert_eq!(
            dispatch(OsStr::new("ryoku-wm-hyprland"), logout.clone()).unwrap(),
            Action::Logout
        );
        let mut extra = logout;
        extra.push("unexpected".into());
        assert!(dispatch(OsStr::new("ryoku-wm-hyprland"), extra).is_err());
        assert_eq!(
            dispatch(OsStr::new("ryoku-session-logout"), vec![]).unwrap(),
            Action::FinishLogout
        );
        assert!(dispatch(OsStr::new("ryoku-session-logout"), vec!["extra".into()]).is_err());

        use std::os::unix::ffi::OsStringExt;
        let literal = OsString::from_vec(vec![b'a', 0xff, b';', b'b']);
        let args = vec!["act".into(), "window.focus".into(), literal];
        assert_eq!(
            dispatch(OsStr::new("ryoku-wm-hyprland"), args.clone()).unwrap(),
            Action::ProviderCommand(args.clone())
        );
        let command = provider_command(&args);
        assert_eq!(command.get_program(), "/usr/bin/setpriv");
        let mut expected: Vec<OsString> =
            ["--no-new-privs", WRITER, "provider"].map(Into::into).into();
        expected.extend(args);
        assert_eq!(command.get_args().collect::<Vec<_>>(), expected.iter().collect::<Vec<_>>());

        let unit = include_str!("../../../packaging/rpm/systemd/user/ryoku-session-logout.service");
        assert!(unit.contains("ExecStart=/usr/libexec/ryoku/session/bin/ryoku-session-logout\n"));
        assert!(!unit.contains("PartOf="));
        assert!(!unit.contains("BindsTo="));
    }

    #[test]
    fn session_actions_require_both_inherited_compositor_identities() {
        let wayland = Some(OsStr::new("wayland-1"));
        let instance = Some(OsStr::new("synthetic-hyprland-instance"));
        assert!(validate_compositor_environment(wayland, instance).is_ok());
        for (wayland, instance) in [
            (None, instance),
            (wayland, None),
            (Some(OsStr::new("")), instance),
            (wayland, Some(OsStr::new(""))),
        ] {
            assert!(validate_compositor_environment(wayland, instance).is_err());
        }
    }

    #[test]
    fn clipboard_keeper_limits_untrusted_selection_size_and_content_logging() {
        let command = clipboard_keeper_command();
        assert_eq!(command.get_program(), CLIPBOARD_KEEPER);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--clipboard",
                "regular",
                "--selection-size-limit",
                "10485760",
                "--disable-timestamps"
            ]
        );
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            [(OsStr::new("RUST_LOG"), Some(OsStr::new("warn")))]
        );
    }

    #[test]
    fn failed_activation_exits_the_inherited_compositor_without_a_shell() {
        let command = exit_compositor_command();
        assert_eq!(command.get_program(), "/usr/bin/hyprctl");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["dispatch", "exit"]);
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn shell_and_hub_keep_existing_writer_coordination_and_native_paths() {
        let root = env::temp_dir().join(format!("ryoku25-environment-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let modules = fixture_surface_payload(&root);
        for hub in [false, true] {
            let command = surface_command_at(hub, &root, &modules).unwrap();
            assert_eq!(command.get_program(), "/usr/bin/setpriv");
            let expected = if hub {
                vec!["--no-new-privs", WRITER, "daemon", "hub", "open"]
            } else {
                vec!["--no-new-privs", WRITER, "daemon", "daemon"]
            };
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
            assert_eq!(
                environment[std::ffi::OsStr::new("RYOKU_POLKIT_AGENT")],
                Some(std::ffi::OsStr::new("0"))
            );
            assert_eq!(
                environment[std::ffi::OsStr::new("RYOKU_SHELL_DIR")],
                Some(root.join("shell").as_os_str())
            );
            assert_eq!(
                environment[std::ffi::OsStr::new("PATH")],
                Some(std::ffi::OsStr::new(
                    "/usr/libexec/ryoku/session/bin:/usr/libexec/ryoku/quickshell/bin:/usr/bin:/bin"
                ))
            );
        }
        let screenshot = screenshot_command_at(&root, &modules).unwrap();
        assert_eq!(screenshot.get_program(), "/usr/bin/setpriv");
        assert_eq!(
            screenshot.get_args().collect::<Vec<_>>(),
            ["--no-new-privs", WRITER, "ryoshot"]
        );
        for module in SURFACE_MODULES {
            let metadata = modules.join("Ryoku").join(module).join("qmldir");
            std::fs::remove_file(&metadata).unwrap();
            assert!(surface_command_at(false, &root, &modules).is_err(), "missing {module}");
            std::fs::write(metadata, "synthetic module").unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn login_disables_main_importer_and_cleanup_clears_bus_values() {
        let command = compositor_command(false);
        let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            environment[std::ffi::OsStr::new("HYPRLAND_NO_SD_VARS")],
            Some(std::ffi::OsStr::new("1"))
        );
        assert!(!environment.contains_key(std::ffi::OsStr::new("LIBGL_ALWAYS_SOFTWARE")));
        assert!(!environment.contains_key(std::ffi::OsStr::new("AQ_DRM_DEVICES")));
        let clear = clear_bus_environment_command();
        let arguments: Vec<_> = clear.get_args().collect();
        assert_eq!(arguments[0], "--systemd");
        assert_eq!(arguments.len(), SESSION_ENV.len() + 1);
        for (argument, name) in arguments[1..].iter().zip(SESSION_ENV) {
            assert_eq!(*argument, std::ffi::OsStr::new(&format!("{name}=")));
        }
        assert!(!SESSION_ENV.contains(&"PATH"));
        assert!(!SESSION_ENV.contains(&"DBUS_SESSION_BUS_ADDRESS"));
    }
}
