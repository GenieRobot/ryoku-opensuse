//! Session lifecycle only. This does not implement or impersonate the upstream
//! shell daemon protocol, configuration materializer, package manager or locker.
use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, ExitCode, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const PAYLOAD: &str = "/usr/share/ryoku";
const QML_MODULES: &str = "/usr/lib64/qt6/qml";
const WRITER: &str = "/usr/libexec/ryoku/ryoku-writer";
const QUICKSHELL: &str = "/usr/libexec/ryoku/quickshell/bin/qs";
const QUICKSHELL_BIN: &str = "/usr/libexec/ryoku/quickshell/bin";
const SESSION_BIN: &str = "/usr/libexec/ryoku/session/bin";
const CLIPBOARD_KEEPER: &str = "/usr/libexec/ryoku/session/upstream/wl-clip-persist";
const LOCK_UNIT: &str = "ryoku-session-lock.service";
const CONFIG: &str = "/usr/share/ryoku-opensuse/session/hyprland.conf";
const TARGET: &str = "ryoku-desktop-session.target";
const GRAPHICAL: &str = "graphical-session.target";
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
    Lock,
    ClipboardKeeper,
    ShellCommand(Vec<OsString>),
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
        Some("ryoku-shell") if arguments.first().is_some_and(|arg| arg == "lock") => {
            if arguments.len() == 1 {
                Ok(Action::Lock)
            } else {
                Err("lock takes no arguments".into())
            }
        }
        Some("ryoku-shell") => Ok(Action::ShellCommand(arguments)),
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

fn systemctl(args: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/systemctl");
    command.args(["--user", "--no-ask-password"]).args(args).stdin(Stdio::null());
    command
}

fn activate() -> Result<()> {
    for name in ["WAYLAND_DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE"] {
        if env::var_os(name).is_none_or(|value| value.is_empty()) {
            return Err(format!("activation requires the compositor's {name}").into());
        }
    }
    // Explicit names avoid importing unrelated process environment (including
    // credentials). Drop old session values before importing the current ones.
    let present: Vec<_> = SESSION_ENV.iter().filter(|name| env::var_os(name).is_some()).collect();
    let mut import = Command::new("/usr/bin/dbus-update-activation-environment");
    import.arg("--systemd").args(&present);
    let outcome = clear_activation_environment()
        .and_then(|()| checked(import))
        .and_then(|()| start_session_target());
    if outcome.is_err() {
        if let Err(error) = stop_session() {
            eprintln!("ryoku-session: activation cleanup failed: {error}");
        }
        // exec-once errors do not terminate Hyprland. Return to the native
        // greeter when required shell startup fails, so KDE stays reachable
        // without an empty compositor trapping the user at a black screen.
        if let Err(error) = checked(exit_compositor_command()) {
            eprintln!("ryoku-session: failed compositor exit: {error}");
        }
    }
    outcome
}

fn exit_compositor_command() -> Command {
    let mut command = Command::new("/usr/bin/hyprctl");
    command.args(["dispatch", "exit"]).stdin(Stdio::null());
    command
}

fn start_session_target() -> Result<()> {
    // A fresh manager has not loaded this unit. ResetFailedUnit deliberately
    // does not load units and would reject that normal cold-start state.
    // Starting the target loads its required shell; preserve its burst limits.
    checked(systemctl(&["start", TARGET]))?;
    checked(systemctl(&[
        "is-active",
        "--quiet",
        "ryoku-desktop-shell.service",
        "ryoku-service.service",
    ]))
}

fn clear_bus_environment_command() -> Command {
    // D-Bus UpdateActivationEnvironment cannot remove a key. Empty its value
    // explicitly, then remove the same key from systemd's separate environment.
    let mut command = Command::new("/usr/bin/dbus-update-activation-environment");
    command.arg("--systemd").args(SESSION_ENV.iter().map(|name| format!("{name}=")));
    command
}

fn clear_activation_environment() -> Result<()> {
    let bus = checked(clear_bus_environment_command());
    let mut clear = systemctl(&["unset-environment"]);
    clear.args(SESSION_ENV);
    // Attempt both cleanups even if the bus has already gone away at logout.
    let manager = checked(clear);
    bus.and(manager)
}

fn stop_session() -> Result<()> {
    // This supported session has one graphical login per user. Stop portals
    // while their compositor is still available during our ordinary logout.
    // Otherwise the native Hyprland portal can crash on Wayland disconnect.
    let stopped = checked(systemctl(&["stop", TARGET, GRAPHICAL]));
    let cleared = clear_activation_environment();
    stopped.and(cleared)
}

fn surface_environment(command: &mut Command, root: &Path, modules: &Path) -> Result<()> {
    for relative in ["shell/quickshell/shell/shell.qml", "hub/quickshell/shell.qml"] {
        if !root.join(relative).is_file() {
            return Err(format!("preserved upstream entrypoint missing: {relative}").into());
        }
    }
    for module in ["Ui", "FrameBars", "PluginKit", "Blobs", "Wm/Hyprland"] {
        if !modules.join("Ryoku").join(module).join("qmldir").is_file() {
            return Err(format!("upstream Ryoku.{module} module missing from payload").into());
        }
    }
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
            let stopped = stop_session();
            let exited = checked(exit_compositor_command());
            stopped.and(exited)
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
        Action::Open(surface) => checked(open_surface_command(surface)?),
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
            checked(systemctl(&["stop", TARGET]))?;
            let outcome = checked(compositor_command(software));
            let cleanup = stop_session();
            outcome.and(cleanup)
        }
    }
}

fn open_surface_command(surface: Surface) -> Result<Command> {
    let mut command = Command::new("/usr/bin/setpriv");
    // Calling existing IPC navigates the coordinated daemon's shell; it does
    // not launch a second, unregistered Quickshell writer.
    command
        .args(["--no-new-privs", QUICKSHELL, "-p"])
        .arg(Path::new(PAYLOAD).join("shell/quickshell/shell"))
        .args(["ipc", "call", "shell", "openSurface", "", surface.upstream_id()]);
    surface_environment(&mut command, Path::new(PAYLOAD), Path::new(QML_MODULES))?;
    Ok(command)
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
        let modules = root.join("qt6/qml");
        for relative in ["shell/quickshell/shell/shell.qml", "hub/quickshell/shell.qml"] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "synthetic entrypoint").unwrap();
        }
        for module in ["Ui", "FrameBars", "PluginKit", "Blobs", "Wm/Hyprland"] {
            let path = modules.join("Ryoku").join(module);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("qmldir"), "synthetic module").unwrap();
        }
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
        std::fs::remove_file(modules.join("Ryoku/Wm/Hyprland/qmldir")).unwrap();
        assert!(surface_command_at(false, &root, &modules).is_err());
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
