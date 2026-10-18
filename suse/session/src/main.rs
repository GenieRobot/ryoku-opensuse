//! Session lifecycle only. This does not implement or impersonate the upstream
//! shell daemon protocol, configuration materializer, package manager or locker.
mod recovery_gate;

use std::{
    env,
    error::Error,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const PAYLOAD: &str = "/usr/share/ryoku";
const QML_MODULES: &str = "/usr/lib64/qt6/qml";
const QUICKSHELL: &str = "/usr/libexec/ryoku/quickshell/bin/qs";
const QUICKSHELL_BIN: &str = "/usr/libexec/ryoku/quickshell/bin";
const CONFIG: &str = "/usr/share/ryoku-opensuse/session/hyprland.conf";
const NESTED_CONFIG: &str = "/usr/share/ryoku-opensuse/session/hyprland-nested.conf";
const TARGET: &str = "ryoku-desktop-session.target";
const SESSION_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "QT_QPA_PLATFORM",
    "LIBGL_ALWAYS_SOFTWARE",
];

#[derive(Debug, PartialEq)]
enum Action {
    Compositor { software: bool },
    Nested { software: bool },
    Activate,
    RecoveryCheck,
    Shell(PathBuf),
    Hub(PathBuf),
    Open(Surface, PathBuf),
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
        ["nested"] => Ok(Action::Nested { software: false }),
        ["nested", "--software-rendering"] => Ok(Action::Nested { software: true }),
        ["activate"] => Ok(Action::Activate),
        ["recovery-check"] => Ok(Action::RecoveryCheck),
        [surface @ ("shell" | "hub")] => surface_action(surface, PAYLOAD),
        [surface @ ("shell" | "hub"), "--payload-root", root] => surface_action(surface, root),
        ["open", surface] => Ok(Action::Open(Surface::parse(surface)?, PAYLOAD.into())),
        ["open", surface, "--payload-root", root] => {
            let Action::Shell(root) = surface_action("shell", root)? else { unreachable!() };
            Ok(Action::Open(Surface::parse(surface)?, root))
        }
        _ => Err("usage: ryoku-session compositor|nested [--software-rendering] | activate | recovery-check | shell|hub [--payload-root ABSOLUTE_PATH] | open launcher|quicksettings|clipboard|notifications [--payload-root ABSOLUTE_PATH]".into()),
    }
}

fn surface_action(surface: &str, root: &str) -> Result<Action> {
    let root = PathBuf::from(root);
    if !root.is_absolute() || root.as_os_str().as_encoded_bytes().contains(&b':') {
        return Err("payload root must be absolute and must not contain ':'".into());
    }
    Ok(if surface == "shell" { Action::Shell(root) } else { Action::Hub(root) })
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
    command.arg("--user").args(args);
    command
}

fn activate() -> Result<()> {
    // This command is also user-invocable. Do not tear down a session when
    // someone runs it outside the compositor's own exec-once hook.
    require_compositor_instance(env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref())?;
    match activate_inner() {
        Ok(()) => Ok(()),
        Err(error) => abort_failed_activation(error.to_string(), stop_session, exit_compositor),
    }
}

fn require_compositor_instance(signature: Option<&std::ffi::OsStr>) -> Result<()> {
    if signature.is_none_or(|value| value.is_empty()) {
        return Err("activation requires the compositor's HYPRLAND_INSTANCE_SIGNATURE".into());
    }
    Ok(())
}

fn activate_inner() -> Result<()> {
    recovery_gate::ensure_clear().map_err(std::io::Error::other)?;
    for name in ["WAYLAND_DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE"] {
        if env::var_os(name).is_none_or(|value| value.is_empty()) {
            return Err(format!("activation requires the compositor's {name}").into());
        }
    }
    // Explicit names avoid importing unrelated process environment (including
    // credentials). Drop old session values before importing the current ones.
    clear_activation_environment()?;
    let present: Vec<_> = SESSION_ENV.iter().filter(|name| env::var_os(name).is_some()).collect();
    let mut import = Command::new("/usr/bin/dbus-update-activation-environment");
    import.arg("--systemd").args(&present);
    checked(import)?;
    // A fresh manager has not loaded this unit. ResetFailedUnit deliberately
    // does not load units and would reject that normal cold-start state.
    // Starting the target loads its required shell; preserve its burst limits.
    checked(systemctl(&["start", TARGET]))
}

fn exit_compositor() -> Result<()> {
    let mut command = Command::new("/usr/bin/hyprctl");
    command.args(["dispatch", "exit"]);
    checked(command)
}

fn abort_failed_activation(
    cause: String,
    cleanup: impl FnOnce() -> Result<()>,
    exit: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let cleanup_result = cleanup();
    let exit_result = exit();
    let cleanup_note = cleanup_result.err().map(|error| format!("; cleanup failed: {error}"));
    let exit_note = exit_result.err().map(|error| format!("; compositor exit failed: {error}"));
    Err(format!(
        "session activation failed: {cause}{}{}",
        cleanup_note.unwrap_or_default(),
        exit_note.unwrap_or_default()
    )
    .into())
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
    let stopped = checked(systemctl(&["stop", TARGET]));
    let cleared = clear_activation_environment();
    stopped.and(cleared)
}

fn surface_command(root: &Path, hub: bool) -> Result<Command> {
    let surface = root.join(if hub { "hub/quickshell" } else { "shell/quickshell/shell" });
    if !surface.join("shell.qml").is_file() {
        return Err(format!("preserved upstream entrypoint missing: {}", surface.display()).into());
    }
    let qml_root =
        if root == Path::new(PAYLOAD) { PathBuf::from(QML_MODULES) } else { root.join("qml") };
    for module in ["Ui", "FrameBars", "PluginKit", "Blobs", "Wm/Hyprland"] {
        if !qml_root.join("Ryoku").join(module).join("qmldir").is_file() {
            return Err(format!(
                "upstream Ryoku.{} module missing from payload",
                module.replace('/', ".")
            )
            .into());
        }
    }
    let imports = env::join_paths([qml_root, root.join("shell/quickshell")])?;
    // Unchanged upstream surfaces also launch qs by name. Scope this runtime
    // selection to the surface child; never replace the distro executable or
    // import this PATH into the shared user manager.
    let inherited_path = env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let path = env::join_paths(
        std::iter::once(PathBuf::from(QUICKSHELL_BIN)).chain(env::split_paths(&inherited_path)),
    )?;
    // Preserved QML includes unported pkexec/sudo calls. Never let a surface
    // child gain privileges via executable set-ID bits or file capabilities.
    // This is not a general D-Bus or home-write sandbox; see session-port.md.
    let mut command = Command::new("/usr/bin/setpriv");
    command
        .args(["--no-new-privs", QUICKSHELL])
        .arg("-p")
        .arg(surface)
        .env("QML_IMPORT_PATH", &imports)
        .env("QML2_IMPORT_PATH", &imports)
        .env("RYOKU_SHELL_DIR", root.join("shell"))
        .env("RYOKU_I18N_DIR", root.join("i18n/catalog"))
        .env("PATH", path)
        .env("QT_QPA_PLATFORM", "wayland")
        .env("QT_MEDIA_BACKEND", "ffmpeg")
        .env("QT_FFMPEG_DECODING_HW_DEVICE_TYPES", "");
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
        Action::Shell(root) => Err(surface_command(&root, false)?.exec().into()),
        Action::Hub(root) => Err(surface_command(&root, true)?.exec().into()),
        Action::Open(surface, root) => checked(open_surface_command(surface, &root)?),
        Action::Activate => activate(),
        Action::RecoveryCheck => {
            recovery_gate::ensure_clear().map_err(std::io::Error::other)?;
            Ok(())
        }
        Action::Nested { software } => {
            if env::var_os("WAYLAND_DISPLAY").is_none_or(|value| value.is_empty()) {
                return Err("nested mode requires an existing Wayland desktop".into());
            }
            if !Path::new(NESTED_CONFIG).is_file() {
                return Err("packaged nested Ryoku configuration missing".into());
            }
            recovery_gate::ensure_clear()?;
            checked(nested_command(software))
        }
        Action::Compositor { software } => {
            if env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()) {
                return Err(
                    "full session requires a separate login; use nested inside a Wayland desktop"
                        .into(),
                );
            }
            if !Path::new(CONFIG).is_file() {
                return Err("packaged Ryoku compositor configuration missing".into());
            }
            recovery_gate::ensure_clear()?;
            // Stop only Ryoku's target: KDE and the shared graphical-session
            // target remain owned by their native session managers.
            checked(systemctl(&["stop", TARGET]))?;
            // Wait for Hyprland in this parent so its exit always reaches the
            // Ryoku-only target and activation-environment cleanup below. The
            // recovery gate itself is a one-shot check; it holds no lease.
            let outcome = checked(compositor_command(software));
            let cleanup = stop_session();
            outcome.and(cleanup)
        }
    }
}

fn open_surface_command(surface: Surface, root: &Path) -> Result<Command> {
    let mut command = surface_command(root, false)?;
    // This is the real preserved shell.qml IpcHandler protocol. Empty
    // monitor selects upstream's focused-output/first-screen fallback.
    command.args(["ipc", "call", "shell", "openSurface", "", surface.upstream_id()]);
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

fn nested_command(software: bool) -> Command {
    let mut command = Command::new("/usr/bin/Hyprland");
    command
        .args(["--config", NESTED_CONFIG])
        // Aquamarine 0.15.1 scanGPUs selects only the explicit device list.
        // Empty selects no DRM devices, leaving Hyprland's Wayland fallback.
        .env("AQ_DRM_DEVICES", "")
        // Suppress the main importer; Executor.cpp's separate importer still
        // requires native hasSession validation (see session-port.md).
        .env("HYPRLAND_NO_SD_VARS", "1")
        .env("QT_QPA_PLATFORM", "wayland");
    if software {
        command.env("LIBGL_ALWAYS_SOFTWARE", "true");
    }
    command
}

fn main() -> ExitCode {
    match parse(&env::args().skip(1).collect::<Vec<_>>()).and_then(run) {
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
    fn rejects_ambiguous_import_roots_and_extra_commands() {
        for input in [
            args(&["shell", "--payload-root", "relative"]),
            args(&["shell", "--payload-root", "/tmp/a:/tmp/b"]),
            args(&["activate", "--payload-root", "/tmp/a"]),
            args(&["compositor", "--software-rendering", "extra"]),
            args(&["open", "notifications", "extra"]),
            args(&["open", "notifications; reboot"]),
            args(&["open", "quick-settings#notifications"]),
        ] {
            assert!(parse(&input).is_err());
        }
        assert_eq!(parse(&args(&["hub"])).unwrap(), Action::Hub(PAYLOAD.into()));
        assert_eq!(parse(&args(&["compositor"])).unwrap(), Action::Compositor { software: false });
        assert_eq!(
            parse(&args(&["open", "notifications"])).unwrap(),
            Action::Open(Surface::Notifications, PAYLOAD.into())
        );
    }

    #[test]
    fn missing_payload_fails_before_starting_runtime() {
        assert!(surface_command(Path::new("/nonexistent/ryoku-payload"), false).is_err());
    }

    #[test]
    fn login_disables_main_importer_and_cleanup_clears_bus_values() {
        let command = compositor_command(false);
        let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            environment[std::ffi::OsStr::new("XDG_CURRENT_DESKTOP")],
            Some(std::ffi::OsStr::new("Hyprland"))
        );
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

    #[test]
    fn activation_failure_cleans_session_then_requests_compositor_exit() {
        let actions = std::cell::RefCell::new(Vec::new());
        let result = abort_failed_activation(
            "recovery gate became unavailable".into(),
            || {
                actions.borrow_mut().push("cleanup");
                Ok(())
            },
            || {
                actions.borrow_mut().push("exit");
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(*actions.borrow(), ["cleanup", "exit"]);
    }

    #[test]
    fn activation_requires_a_compositor_instance_before_cleanup_can_run() {
        assert!(require_compositor_instance(None).is_err());
        assert!(require_compositor_instance(Some(std::ffi::OsStr::new(""))).is_err());
        assert!(require_compositor_instance(Some(std::ffi::OsStr::new("instance-1"))).is_ok());
    }

    #[test]
    fn activation_cleanup_failure_does_not_skip_compositor_exit() {
        let mut exited = false;
        let result = abort_failed_activation(
            "target start failed".into(),
            || Err("user manager cleanup failed".into()),
            || {
                exited = true;
                Ok(())
            },
        );
        assert!(result.unwrap_err().to_string().contains("cleanup failed"));
        assert!(exited);
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
    fn nested_mode_never_uses_the_login_config_or_manager_import() {
        let command = nested_command(true);
        assert_eq!(command.get_program(), "/usr/bin/Hyprland");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--config", NESTED_CONFIG]);
        let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
        for (name, value) in [
            ("AQ_DRM_DEVICES", ""),
            ("HYPRLAND_NO_SD_VARS", "1"),
            ("LIBGL_ALWAYS_SOFTWARE", "true"),
        ] {
            assert_eq!(environment[std::ffi::OsStr::new(name)], Some(std::ffi::OsStr::new(value)));
        }
        assert!(!environment.contains_key(std::ffi::OsStr::new("DBUS_SESSION_BUS_ADDRESS")));
        assert!(!environment.contains_key(std::ffi::OsStr::new("WAYLAND_DISPLAY")));
        let config = include_str!("../../../packaging/rpm/session/hyprland-nested.conf");
        assert!(!config.contains(" activate"));
        assert!(!config.contains("systemctl"));
    }

    #[test]
    fn surfaces_keep_no_new_privileges_and_literal_payload_paths() {
        let root = env::temp_dir().join(format!("ryoku-session-test-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for relative in [
            "shell/quickshell/shell/shell.qml",
            "hub/quickshell/shell.qml",
            "qml/Ryoku/Ui/qmldir",
            "qml/Ryoku/FrameBars/qmldir",
            "qml/Ryoku/PluginKit/qmldir",
            "qml/Ryoku/Blobs/qmldir",
            "qml/Ryoku/Wm/Hyprland/qmldir",
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "fixture manifest only").unwrap();
        }
        for hub in [false, true] {
            let command = surface_command(&root, hub).unwrap();
            assert_eq!(command.get_program(), "/usr/bin/setpriv");
            let arguments: Vec<_> = command.get_args().collect();
            assert_eq!(arguments[0], "--no-new-privs");
            assert_eq!(arguments[1], QUICKSHELL);
            assert_eq!(arguments[2], "-p");
            assert_eq!(
                arguments[3],
                root.join(if hub { "hub/quickshell" } else { "shell/quickshell/shell" })
            );
            let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
            assert_eq!(
                env::split_paths(environment[std::ffi::OsStr::new("PATH")].unwrap()).next(),
                Some(PathBuf::from(QUICKSHELL_BIN))
            );
            assert_eq!(
                environment[std::ffi::OsStr::new("RYOKU_I18N_DIR")],
                Some(root.join("i18n/catalog").as_os_str())
            );
        }
        let command = open_surface_command(Surface::Notifications, &root).unwrap();
        assert_eq!(command.get_program(), "/usr/bin/setpriv");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                std::ffi::OsStr::new("--no-new-privs"),
                std::ffi::OsStr::new(QUICKSHELL),
                std::ffi::OsStr::new("-p"),
                root.join("shell/quickshell/shell").as_os_str(),
                std::ffi::OsStr::new("ipc"),
                std::ffi::OsStr::new("call"),
                std::ffi::OsStr::new("shell"),
                std::ffi::OsStr::new("openSurface"),
                std::ffi::OsStr::new(""),
                std::ffi::OsStr::new("quick-settings#notifications"),
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(Surface::QuickSettings.upstream_id(), "quick-settings");
        assert!(Surface::parse("install").is_err());
    }
}
