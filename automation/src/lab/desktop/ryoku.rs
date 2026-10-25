//! Read-only installed Ryoku inspection. This neither starts units nor injects
//! GUI input; the lab owner supplies an ordinary SDDM password login separately.
use super::*;
use crate::process;

const TARGET: &str = "ryoku-desktop-session.target";
const SHELL: &str = "ryoku-desktop-shell.service";
const SERVICE: &str = "ryoku-service.service";
const UNITS: &[&str] = &[TARGET, SHELL, SERVICE];
const LOCK: &str = "ryoku-session-lock.service";
const LOGOUT: &str = "ryoku-session-logout.service";
const FALLBACK_UNITS: &[&str] = &[TARGET, SHELL, SERVICE, LOCK, LOGOUT];
const GRAPHICAL_ENV: &[&str] = &[
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionMode {
    Plasma,
    Ryoku,
    RyokuFallback,
}

impl InspectionMode {
    pub fn parse(command: &str) -> Option<Self> {
        match command {
            "guest-login-inspect" => Some(Self::Plasma),
            "guest-ryoku-login-inspect" => Some(Self::Ryoku),
            "guest-ryoku-fallback-inspect" => Some(Self::RyokuFallback),
            _ => None,
        }
    }
}

pub fn inspect_mode(mode: InspectionMode) -> Result<()> {
    if mode == InspectionMode::Plasma {
        return super::inspect();
    }
    // Static category only: native output/environment/account data stay private.
    let mut stage = "fixture";
    inspect_ryoku(mode, &mut stage).map_err(|_| {
        println!("ryoku_inspection_reason={stage}");
        "Ryoku inspection failed; inspect the reported stage in the owned guest".into()
    })
}

fn query(command: &mut Command) -> Result<String> {
    command.env_clear().env("PATH", "/usr/bin:/bin").env("LC_ALL", "C").stdin(Stdio::null());
    let result = process::run(command, Duration::from_secs(10), 1024 * 1024)?;
    require(
        result.status.success()
            && !result.timed_out
            && !result.stdout_truncated
            && !result.stderr_truncated,
        "bounded read-only session query failed",
    )?;
    Ok(String::from_utf8(result.stdout)?)
}

enum UserQuery<'a> {
    Unit(&'a str),
    Environment,
}

fn user_command(uid: u32, request: UserQuery<'_>) -> Result<Command> {
    let mut command = Command::new("/usr/sbin/runuser");
    command
        .args(["-u", USER, "--", "/usr/bin/env", "-i"])
        .args([
            format!("HOME={HOME}"),
            format!("USER={USER}"),
            format!("LOGNAME={USER}"),
            format!("XDG_RUNTIME_DIR=/run/user/{uid}"),
            format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"),
            "PATH=/usr/bin:/bin".into(),
            "LC_ALL=C".into(),
        ])
        .args(["/usr/bin/systemctl", "--user", "--no-pager", "--no-ask-password"]);
    match request {
        UserQuery::Unit(unit) => {
            require(FALLBACK_UNITS.contains(&unit), "unsupported inspected unit")?;
            command.args(["show", unit, "--property=Id,LoadState,ActiveState,SubState,MainPID,ControlGroup,FragmentPath,PartOf"]);
        }
        UserQuery::Environment => {
            command.arg("show-environment");
        }
    }
    Ok(command)
}

fn user_query(uid: u32, request: UserQuery<'_>) -> Result<String> {
    query(&mut user_command(uid, request)?)
}

fn unit_properties(uid: u32, unit: &str) -> Result<BTreeMap<String, String>> {
    properties(&user_query(uid, UserQuery::Unit(unit))?)
}

fn manager_properties(uid: u32) -> Result<BTreeMap<String, String>> {
    properties(&query(Command::new("/usr/bin/systemctl").args([
        "show",
        &format!("user@{uid}.service"),
        "--no-pager",
        "--no-ask-password",
        "--property=Id,LoadState,ActiveState,SubState,MainPID",
    ]))?)
}

fn manager_stopped(values: &BTreeMap<String, String>, uid: u32) -> bool {
    matches(
        values,
        &[
            ("Id", &format!("user@{uid}.service")),
            ("LoadState", "loaded"),
            ("ActiveState", "inactive"),
            ("SubState", "dead"),
            ("MainPID", "0"),
        ],
    )
}

fn user_group(group: &str, uid: u32) -> bool {
    group.starts_with(&format!("/user.slice/user-{uid}.slice/user@{uid}.service/"))
        && !group.contains("..")
}

fn unit_ready(values: &BTreeMap<String, String>, unit: &str, uid: u32) -> bool {
    matches(values, &[("Id", unit), ("LoadState", "loaded"), ("ActiveState", "active")])
        && values
            .get("FragmentPath")
            .is_some_and(|path| path == &format!("/usr/lib/systemd/user/{unit}"))
        && if unit == TARGET {
            values.get("SubState").is_some_and(|s| s == "active")
        } else {
            values.get("SubState").is_some_and(|s| s == "running")
                && values.get("MainPID").and_then(|s| s.parse::<u32>().ok()).is_some_and(|p| p > 1)
                && values
                    .get("ControlGroup")
                    .is_some_and(|g| user_group(g, uid) && g.ends_with(&format!("/{unit}")))
                && values.get("PartOf").is_some_and(|s| s.split_whitespace().any(|v| v == TARGET))
        }
}

fn unit_stopped(values: &BTreeMap<String, String>, unit: &str) -> bool {
    matches(values, &[("Id", unit), ("LoadState", "loaded")])
        && values
            .get("FragmentPath")
            .is_some_and(|path| path == &format!("/usr/lib/systemd/user/{unit}"))
        && if unit == TARGET {
            values.get("MainPID").is_none_or(|pid| pid == "0")
        } else {
            values.get("MainPID").is_some_and(|pid| pid == "0")
        }
        && matches!(values.get("ActiveState").map(String::as_str), Some("inactive" | "failed"))
        && matches!(values.get("SubState").map(String::as_str), Some("dead" | "failed"))
}

fn group_unpopulated(source: &str) -> Result<bool> {
    let mut populated = None;
    for line in source.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        require(fields.len() == 2, "malformed cgroup event state")?;
        if fields[0] == "populated" {
            require(populated.is_none(), "duplicate cgroup populated state")?;
            populated = Some(match fields[1] {
                "0" => true,
                "1" => false,
                _ => return Err("invalid cgroup populated state".into()),
            });
        }
    }
    populated.ok_or_else(|| "cgroup populated state missing".into())
}

fn unit_cgroup_empty(
    root: &fs::File,
    values: &BTreeMap<String, String>,
    unit: &str,
    uid: u32,
) -> Result<()> {
    if unit == TARGET {
        return Ok(());
    }
    require(FALLBACK_UNITS.contains(&unit), "unsupported lifecycle unit")?;
    let group = values.get("ControlGroup").ok_or("unit control group state missing")?;
    if group.is_empty() {
        return Ok(());
    }
    require(
        user_group(group, uid)
            && group.ends_with(&format!("/{unit}"))
            && Path::new(group).components().all(|part| {
                matches!(part, std::path::Component::RootDir | std::path::Component::Normal(_))
            })
            && !group.contains("//"),
        "unit control group outside authenticated user unit",
    )?;
    let relative = Path::new(group).strip_prefix("/")?;
    let flags = rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS;
    let directory = match rustix::fs::openat2(
        root,
        relative,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        flags,
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(std::io::Error::from(error).into()),
    };
    let events = fs::File::from(rustix::fs::openat2(
        &directory,
        "cgroup.events",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        flags,
    )?);
    let mut bytes = Vec::new();
    events.take(4097).read_to_end(&mut bytes)?;
    require(
        bytes.len() <= 4096 && group_unpopulated(std::str::from_utf8(&bytes)?)?,
        "unit cgroup still contains tasks; wait for native lifecycle cleanup",
    )
}

fn ryoku_entry(source: &str) -> Result<bool> {
    let values = ini(source)?;
    Ok(matches!(value(&values, "Desktop Entry", "Type"), Some("Application"))
        && value(&values, "Desktop Entry", "Exec") == Some("/usr/bin/ryoku-session compositor")
        && value(&values, "Desktop Entry", "TryExec") == Some("/usr/bin/ryoku-session")
        && value(&values, "Desktop Entry", "DesktopNames") == Some("Hyprland")
        && !["Hidden", "NoDisplay"].iter().any(|key| {
            value(&values, "Desktop Entry", key).is_some_and(|v| v.eq_ignore_ascii_case("true"))
        }))
}

fn ryoku_session(values: &BTreeMap<String, String>, uid: u32) -> bool {
    let mut expected = values.clone();
    if expected.get("Desktop").map(String::as_str) != Some("Hyprland") {
        return false;
    }
    expected.insert("Desktop".into(), "KDE".into());
    session_matches(&expected, uid)
}

fn environment(source: &[u8]) -> Result<BTreeMap<String, String>> {
    require(source.len() <= 1024 * 1024, "session environment exceeds inspection bound")?;
    let mut values = BTreeMap::new();
    for entry in source.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        let entry = std::str::from_utf8(entry)?;
        let (key, value) = entry.split_once('=').ok_or("malformed process environment")?;
        require(
            values.insert(key.into(), value.into()).is_none(),
            "duplicate process environment",
        )?;
    }
    Ok(values)
}

fn manager_environment(source: &str) -> Result<BTreeMap<String, String>> {
    // systemctl shell-quotes complex values. Only fixed scalar values below are
    // compared; nothing here is executed or interpreted as shell code.
    properties(source)
}

fn session_environment(values: &BTreeMap<String, String>, uid: u32, id: &str) -> bool {
    matches(
        values,
        &[
            ("HOME", HOME),
            ("XDG_RUNTIME_DIR", &format!("/run/user/{uid}")),
            ("XDG_SESSION_TYPE", "wayland"),
            ("XDG_CURRENT_DESKTOP", "Hyprland"),
            ("XDG_SESSION_ID", id),
        ],
    ) && values.get("WAYLAND_DISPLAY").is_some_and(|s| {
        s.starts_with("wayland-") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) && values.get("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|s| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ProcessIdentity {
    start: u64,
    dev: u64,
    ino: u64,
    group: String,
}

fn process_start(stat: &str) -> Result<u64> {
    let (_, rest) = stat.rsplit_once(") ").ok_or("invalid process stat")?;
    Ok(rest.split_whitespace().nth(19).ok_or("missing process start")?.parse()?)
}

fn process_identity(pid: u32, executable: &Path, uid: u32) -> Result<ProcessIdentity> {
    require(
        process_status(&fs::read_to_string(format!("/proc/{pid}/status"))?, uid),
        "session process not live or UID changed",
    )?;
    require(
        fs::read_link(format!("/proc/{pid}/exe"))? == fs::canonicalize(executable)?,
        "session process executable mismatch",
    )?;
    let actual = fs::File::open(format!("/proc/{pid}/exe"))?.metadata()?;
    let installed = fs::metadata(executable)?;
    require(
        actual.dev() == installed.dev() && actual.ino() == installed.ino(),
        "session process image differs from installed file",
    )?;
    Ok(ProcessIdentity {
        start: process_start(&fs::read_to_string(format!("/proc/{pid}/stat"))?)?,
        dev: actual.dev(),
        ino: actual.ino(),
        group: fs::read_to_string(format!("/proc/{pid}/cgroup"))?,
    })
}

fn unit_process(
    pid: u32,
    executable: &str,
    package: &str,
    group: &str,
    uid: u32,
) -> Result<ProcessIdentity> {
    rpm_intact(Path::new(executable), package)?;
    let identity = process_identity(pid, Path::new(executable), uid)?;
    require(
        identity.group.lines().any(|line| {
            line.strip_prefix("0::")
                .is_some_and(|g| g == group || g.starts_with(&format!("{group}/")))
        }),
        "session process outside required unit",
    )?;
    Ok(identity)
}

fn bounded_file(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    require(bytes.len() <= 1024 * 1024, "inspection file exceeds bound")?;
    Ok(bytes)
}

fn has_uid(source: &str, uid: u32) -> bool {
    source
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .is_some_and(|ids| ids.split_whitespace().any(|id| id == uid.to_string()))
}

fn ryoku_process_image(executable: &Path) -> bool {
    let Some(name) = executable.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    // procfs appends this exact suffix when an executing file is unlinked.
    // A surviving paused player still counts as incomplete logout cleanup.
    let name = name.strip_suffix(" (deleted)").unwrap_or(name);
    [
        "Hyprland",
        "ryoku-session",
        "ryoku-writer",
        "ryoku-shell",
        "ryoku-service",
        "ryogami",
        "ryogami-live",
        "qs",
    ]
    .contains(&name)
}

fn no_ryoku_processes(uid: u32) -> Result<()> {
    // Unlike the KDE live-process helper, include stopped tasks: a manually
    // paused player or ptrace-stopped writer is not completed logout cleanup.
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        if !has_uid(&status, uid) {
            continue;
        }
        let Ok(exe) = fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        require(!ryoku_process_image(&exe), "Ryoku process remains after failure/logout")?;
    }
    Ok(())
}

fn inspect_ryoku(mode: InspectionMode, stage: &mut &'static str) -> Result<()> {
    guard()?;
    authentication_policy()?;
    display_manager()?;
    let uid = desktop_uid()?;
    let plasma = plasma_session()?;
    *stage = "installed_metadata";
    // The existing preparation record binds the account and independent KDE
    // entry, not whichever session is selected for this read-only inspection.
    require(existing_record()? == Some(record(uid, &plasma)?), "fixture account record mismatch")?;
    let entry = Path::new(SESSION_DIR).join("ryoku-opensuse.desktop");
    rpm_intact(&entry, "ryoku-session")?;
    require(ryoku_entry(&fs::read_to_string(entry)?)?, "installed Ryoku session entry mismatch")?;
    for (path, owner) in [
        ("/usr/bin/ryoku-session", "ryoku-session"),
        ("/usr/lib/systemd/user/ryoku-desktop-session.target", "ryoku-session"),
        ("/usr/lib/systemd/user/ryoku-desktop-shell.service", "ryoku-session"),
        ("/usr/lib/systemd/user/ryoku-service.service", "ryoku-helper"),
        ("/usr/lib/systemd/user/ryoku-service.service.d/25-session.conf", "ryoku-session"),
        ("/usr/lib/systemd/user/ryoku-session-lock.service", "ryoku-session"),
        ("/usr/lib/systemd/user/ryoku-session-logout.service", "ryoku-session"),
        ("/usr/libexec/ryoku/quickshell/qs", "ryoku-desktop-runtime"),
    ] {
        rpm_intact(Path::new(path), owner)?;
    }
    if mode == InspectionMode::RyokuFallback {
        *stage = "fallback_cleanup";
        greeter_ready()?;
        require(
            registered_sessions(uid)?.is_empty(),
            "fixture login still registered during fallback",
        )?;
        no_ryoku_processes(uid)?;
        let manager = manager_properties(uid)?;
        let manager_absent = if manager_stopped(&manager, uid) {
            // Logind normally terminates an ordinary user's manager after the
            // last logout. Never start it merely to inspect empty unit state.
            for path in [format!("/run/user/{uid}/bus"), format!("/run/user/{uid}/systemd/private")]
            {
                require(
                    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
                    "inactive user manager still has a runtime endpoint",
                )?;
            }
            require(manager_properties(uid)? == manager, "user manager changed during fallback")?;
            true
        } else {
            require(
                manager.get("ActiveState").is_some_and(|s| s == "active"),
                "user manager cleanup in progress or failed",
            )?;
            let units: Vec<_> = FALLBACK_UNITS
                .iter()
                .map(|unit| unit_properties(uid, unit))
                .collect::<Result<_>>()?;
            let cgroups = fs::File::open("/sys/fs/cgroup")?;
            for (unit, values) in FALLBACK_UNITS.iter().zip(&units) {
                require(unit_stopped(values, unit), "Ryoku unit still active during fallback")?;
                unit_cgroup_empty(&cgroups, values, unit, uid)?;
            }
            let env = manager_environment(&user_query(uid, UserQuery::Environment)?)?;
            require(
                GRAPHICAL_ENV.iter().all(|key| !env.contains_key(*key)),
                "graphical environment remains after logout",
            )?;
            for (unit, before) in FALLBACK_UNITS.iter().zip(&units) {
                require(unit_properties(uid, unit)? == *before, "unit changed during fallback")?;
                unit_cgroup_empty(&cgroups, before, unit, uid)?;
            }
            require(
                manager_environment(&user_query(uid, UserQuery::Environment)?)? == env,
                "environment changed during fallback",
            )?;
            false
        };
        greeter_ready()?;
        no_ryoku_processes(uid)?;
        require(registered_sessions(uid)?.is_empty(), "fixture session appeared during fallback")?;
        guard()?;
        if manager_absent {
            println!("ryoku_units=manager_absent\nryoku_environment=manager_absent");
        } else {
            println!("ryoku_units=stopped\nryoku_environment=cleared");
        }
        println!(
            "ryoku_fallback=greeter_ready\nryoku_kde_entry=selectable\nryoku_kde_runtime=unverified"
        );
        return Ok(());
    }
    *stage = "native_login";
    let id = active_session()?;
    let login = show_session(&id)?;
    require(ryoku_session(&login, uid), "ordinary Hyprland SDDM session mismatch")?;
    require(
        login.get("Scope") == Some(&format!("session-{id}.scope")),
        "Ryoku login scope mismatch",
    )?;
    for candidate in registered_sessions(uid)? {
        require(
            candidate == id
                || show_session(&candidate)?.get("Class").is_some_and(|v| v == "manager"),
            "ambiguous concurrent fixture session",
        )?;
    }
    *stage = "unit_readiness";
    let units: Vec<_> =
        UNITS.iter().map(|unit| unit_properties(uid, unit)).collect::<Result<_>>()?;
    for (unit, values) in UNITS.iter().zip(&units) {
        require(unit_ready(values, unit, uid), "required Ryoku unit not ready")?;
    }
    *stage = "process_identity";
    let compositor = user_process("Hyprland", "hyprland", uid, &format!("session-{id}.scope"))?;
    let compositor_identity = process_identity(compositor, Path::new("/usr/bin/Hyprland"), uid)?;
    let daemon = process_ids("ryoku-shell", uid)?;
    require(daemon.len() == 1, "expected one native shell daemon")?;
    let daemon_exe = "/usr/libexec/ryoku/upstream/ryoku-shell";
    let daemon_identity = unit_process(
        daemon[0],
        daemon_exe,
        "ryoku-desktop-runtime",
        &units[1]["ControlGroup"],
        uid,
    )?;
    // Hyprland creates its display/instance variables after exec; /proc's
    // initial compositor environ need not contain them. Inspect its admitted
    // shell daemon child, which inherits the activated manager environment.
    let raw_environment = bounded_file(format!("/proc/{}/environ", daemon[0]))?;
    *stage = "session_environment";
    let env = environment(&raw_environment)?;
    require(session_environment(&env, uid, &id), "Ryoku daemon session environment mismatch")?;
    let manager = manager_environment(&user_query(uid, UserQuery::Environment)?)?;
    for key in [
        "WAYLAND_DISPLAY",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_SESSION_ID",
    ] {
        require(manager.get(key) == env.get(key), "imported user manager environment mismatch")?;
    }
    let socket = plasma_socket(&raw_environment, uid)?;
    *stage = "wayland_peer";
    let metadata = fs::symlink_metadata(&socket)?;
    require(
        metadata.file_type().is_socket() && metadata.uid() == uid,
        "Ryoku Wayland socket ownership mismatch",
    )?;
    let connection = UnixStream::connect(&socket)?;
    let peer = rustix::net::sockopt::socket_peercred(&connection)?;
    require(
        peer.pid.as_raw_nonzero().get() as u32 == compositor && peer.uid.as_raw() == uid,
        "Wayland peer is not inspected Hyprland",
    )?;
    let mut identities = Vec::new();
    *stage = "process_identity";
    for (index, executable, owner) in [
        (1, "/usr/libexec/ryoku/ryoku-writer", "ryoku-desktop-runtime"),
        (2, "/usr/libexec/ryoku/ryoku-service", "ryoku-helper"),
    ] {
        let pid: u32 = units[index]["MainPID"].parse()?;
        identities.push((
            pid,
            executable,
            unit_process(pid, executable, owner, &units[index]["ControlGroup"], uid)?,
        ));
    }
    identities.push((daemon[0], daemon_exe, daemon_identity));
    let shell_path = b"/usr/share/ryoku/shell/quickshell/shell";
    let mut surface_count = 0;
    for pid in process_ids("qs", uid)? {
        let args = bounded_file(format!("/proc/{pid}/cmdline"))?;
        if surface_arguments(&args, shell_path) {
            let executable = "/usr/libexec/ryoku/quickshell/bin/qs";
            identities.push((
                pid,
                executable,
                unit_process(pid, executable, "ryoku-quickshell", &units[1]["ControlGroup"], uid)?,
            ));
            surface_count += 1;
        }
    }
    require(surface_count == 1, "expected one packaged shell surface")?;
    // Recheck unit snapshots, exact process starts/images/groups and active
    // logind identity after socket/RPM inspection; changing observations fail.
    *stage = "revalidation";
    for (unit, before) in UNITS.iter().zip(&units) {
        require(unit_properties(uid, unit)? == *before, "unit changed during inspection")?;
    }
    for (pid, executable, before) in identities {
        require(
            process_identity(pid, Path::new(executable), uid)? == before,
            "process changed during inspection",
        )?;
    }
    require(
        process_identity(compositor, Path::new("/usr/bin/Hyprland"), uid)? == compositor_identity
            && active_session()? == id
            && show_session(&id)? == login,
        "Ryoku login changed during inspection",
    )?;
    guard()?;
    println!(
        "ryoku_login=inspected\nryoku_session=sddm_password_wayland\nryoku_units=ready\nryoku_processes=rpm_bound_live\nryoku_environment=matched\nryoku_wayland=peer_connected\nryoku_kde_entry=selectable\nryoku_visible_frame=unverified\nryoku_reboot=unverified\nryoku_devices=unverified"
    );
    Ok(())
}

fn surface_arguments(arguments: &[u8], path: &[u8]) -> bool {
    let args: Vec<_> = arguments.split(|b| *b == 0).filter(|arg| !arg.is_empty()).collect();
    args.len() == 3 && args[1] == b"-p" && args[2] == path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_modes_preserve_kde_and_require_exact_named_commands() {
        assert_eq!(InspectionMode::parse("guest-login-inspect"), Some(InspectionMode::Plasma));
        assert_eq!(InspectionMode::parse("guest-ryoku-login-inspect"), Some(InspectionMode::Ryoku));
        assert_eq!(
            InspectionMode::parse("guest-ryoku-fallback-inspect"),
            Some(InspectionMode::RyokuFallback)
        );
        for command in [
            "guest-login-inspect Ryoku",
            "guest-ryoku-login-inspect;reboot",
            "guest-login-prepare",
            "",
        ] {
            assert_eq!(InspectionMode::parse(command), None);
        }
    }

    #[test]
    fn user_manager_queries_cannot_start_stop_or_select_arbitrary_units() {
        for unit in FALLBACK_UNITS {
            let command = user_command(1001, UserQuery::Unit(unit)).unwrap();
            assert_eq!(command.get_program(), "/usr/sbin/runuser");
            let args: Vec<_> = command.get_args().map(|s| s.to_str().unwrap()).collect();
            assert!(args.windows(2).any(|pair| pair == ["show", *unit]));
            assert!(args.contains(&"HOME=/home/ryoku-desktop"));
            assert!(args.contains(&"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1001/bus"));
            for forbidden in [
                "start",
                "stop",
                "restart",
                "reset-failed",
                "import-environment",
                "set-environment",
                "sh",
                "-c",
            ] {
                assert!(!args.contains(&forbidden));
            }
        }
        assert!(user_command(1001, UserQuery::Unit("other.service")).is_err());
        let command = user_command(1001, UserQuery::Environment).unwrap();
        assert_eq!(command.get_args().last().unwrap(), "show-environment");
    }

    #[test]
    fn fallback_lifecycle_and_descendant_population_do_not_change_startup_requirements() {
        assert_eq!(UNITS, &[TARGET, SHELL, SERVICE]);
        assert!(FALLBACK_UNITS.contains(&LOCK) && FALLBACK_UNITS.contains(&LOGOUT));
        assert!(!UNITS.contains(&LOCK) && !UNITS.contains(&LOGOUT));
        for unit in [LOCK, LOGOUT] {
            let source = format!(
                "Id={unit}\nLoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nFragmentPath=/usr/lib/systemd/user/{unit}\nControlGroup=\n"
            );
            assert!(unit_stopped(&properties(&source).unwrap(), unit));
            assert!(!unit_stopped(
                &properties(&source.replace("ActiveState=inactive", "ActiveState=active")).unwrap(),
                unit
            ));
            assert!(!unit_stopped(
                &properties(&source.replace("MainPID=0", "MainPID=42")).unwrap(),
                unit
            ));
            assert!(!unit_stopped(
                &properties(&source.replace("/usr/lib/systemd", "/etc/systemd")).unwrap(),
                unit
            ));
        }
        assert!(group_unpopulated("populated 0\nfrozen 0\n").unwrap());
        assert!(!group_unpopulated("populated 1\nfrozen 0\n").unwrap());
        for source in
            ["", "frozen 0\n", "populated 0\npopulated 1\n", "populated 2\n", "populated 0 extra\n"]
        {
            assert!(group_unpopulated(source).is_err());
        }
    }

    #[test]
    fn fallback_cgroup_read_is_bounded_confined_and_checks_descendant_population() {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = std::env::temp_dir().join(format!("issue25-cgroup-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let _cleanup = Cleanup(root.clone());
        let directory = fs::File::open(&root).unwrap();
        let group = format!("/user.slice/user-1001.slice/user@1001.service/session.slice/{SHELL}");
        let mut values = BTreeMap::from([("ControlGroup".into(), group.clone())]);
        let path = root.join(group.strip_prefix('/').unwrap());
        // An already removed inactive cgroup is empty; no service is started.
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_ok());
        fs::create_dir_all(&path).unwrap();
        let events = path.join("cgroup.events");
        fs::write(&events, "populated 0\nfrozen 0\n").unwrap();
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_ok());
        // Empty direct membership does not certify empty descendant groups.
        fs::write(path.join("cgroup.procs"), "").unwrap();
        fs::write(&events, "populated 1\nfrozen 0\n").unwrap();
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_err());
        fs::write(&events, "x".repeat(4097)).unwrap();
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_err());
        fs::remove_file(&events).unwrap();
        std::os::unix::fs::symlink(root.join("outside-events"), &events).unwrap();
        fs::write(root.join("outside-events"), "populated 0\n").unwrap();
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_err());
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1002).is_err());
        for invalid in [
            group.replace("user-1001.slice", "user-0.slice"),
            group.replace("session.slice/", "../"),
            group.replace(SHELL, "other.service"),
        ] {
            values.insert("ControlGroup".into(), invalid);
            assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_err());
        }
        values.insert("ControlGroup".into(), String::new());
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_ok());
        values.clear();
        assert!(unit_cgroup_empty(&directory, &values, SHELL, 1001).is_err());
    }

    #[test]
    fn shell_surface_is_not_an_ipc_client_or_an_unrelated_qml_surface() {
        let path = b"/usr/share/ryoku/shell/quickshell/shell";
        assert!(surface_arguments(b"qs\0-p\0/usr/share/ryoku/shell/quickshell/shell\0", path));
        for args in [
            b"qs\0-p\0/usr/share/ryoku/shell/quickshell/shell\0ipc\0call\0".as_slice(),
            b"qs\0-p\0/other\0",
            b"bash\0-c\0echo /usr/share/ryoku/shell/quickshell/shell\0",
            b"qs\0-c\0shell\0",
        ] {
            assert!(!surface_arguments(args, path));
        }
    }

    #[test]
    fn fallback_inventory_never_drops_stopped_players_or_mixed_uid_processes() {
        for state in ["R", "S", "T", "t", "Z"] {
            assert!(has_uid(&format!("Uid:\t1001\t1001\t1001\t1001\nState:\t{state}\n"), 1001));
        }
        assert!(has_uid("Uid:\t0\t1001\t0\t0\nState:\tT\n", 1001));
        assert!(!has_uid("Uid:\t10010\t10010\t10010\t10010\n", 1001));
        assert!(!has_uid("State:\tT\n", 1001));
    }

    #[test]
    fn fallback_recognizes_deleted_actor_images_without_matching_unrelated_paths() {
        for name in [
            "Hyprland",
            "ryoku-session",
            "ryoku-writer",
            "ryoku-shell",
            "ryoku-service",
            "ryogami",
            "ryogami-live",
            "qs",
        ] {
            for suffix in ["", " (deleted)"] {
                assert!(ryoku_process_image(Path::new(&format!("/fixture/{name}{suffix}"))));
            }
        }
        for path in [
            "/fixture/other",
            "/fixture/not-ryogami-live",
            "/fixture/ryogami-live-extra",
            "/fixture/ryogami-live (deleted).bak",
            "/fixture/ryogami-live (deleted) (deleted)",
            "/fixture/ryogami-live/other",
            "/",
        ] {
            assert!(!ryoku_process_image(Path::new(path)));
        }
    }

    fn retry_owned_image_busy<T>(
        limit: Duration,
        mut spawn: impl FnMut() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let deadline = Instant::now() + limit;
        let mut retries = 0;
        loop {
            match spawn() {
                Err(error)
                    if error.raw_os_error() == Some(rustix::io::Errno::TXTBSY.raw_os_error())
                        && Instant::now() < deadline
                        && retries < 1000 =>
                {
                    retries += 1;
                    thread::sleep(Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    #[test]
    fn owned_image_busy_retry_is_bounded_and_preserves_other_spawn_failures() {
        let busy = || std::io::Error::from_raw_os_error(rustix::io::Errno::TXTBSY.raw_os_error());
        let mut attempts = 0;
        assert_eq!(
            retry_owned_image_busy(Duration::from_secs(1), || {
                attempts += 1;
                if attempts == 1 { Err(busy()) } else { Ok("started") }
            })
            .unwrap(),
            "started"
        );
        assert_eq!(attempts, 2);
        assert_eq!(
            retry_owned_image_busy::<()>(Duration::from_secs(1), || Err(
                std::io::ErrorKind::PermissionDenied.into()
            ))
            .unwrap_err()
            .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let started = Instant::now();
        assert_eq!(
            retry_owned_image_busy::<()>(Duration::from_millis(5), || Err(busy()))
                .unwrap_err()
                .raw_os_error(),
            Some(rustix::io::Errno::TXTBSY.raw_os_error())
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn fallback_detects_owned_actor_after_its_running_image_is_unlinked() {
        const READY: &str = "RYOKU_ISSUE25_TEST_ACTOR_READY";
        if let Some(ready) = std::env::var_os(READY) {
            let mut marker =
                fs::OpenOptions::new().write(true).create_new(true).open(ready).unwrap();
            marker.write_all(b"ready\n").unwrap();
            thread::sleep(Duration::from_secs(30));
            return;
        }
        struct Fixture {
            directory: PathBuf,
            child: Option<std::process::Child>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Some(child) = &mut self.child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let _ = fs::remove_dir_all(&self.directory);
            }
        }
        let directory =
            std::env::temp_dir().join(format!("issue25-unlinked-actor-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let mut fixture = Fixture { directory, child: None };
        let image = fixture.directory.join("ryogami-live");
        // Use this known native Rust ELF, not a host utility which may be an
        // interpreted or re-executing launcher. The exact child test emits
        // readiness after exec and holds only its disposable process alive.
        fs::copy(std::env::current_exe().unwrap(), &image).unwrap();
        let mut header = [0; 4];
        fs::File::open(&image).unwrap().read_exact(&mut header).unwrap();
        assert_eq!(&header, b"\x7fELF");
        let ready = fixture.directory.join("ready");
        let mut command = Command::new(&image);
        command
            .args(["--exact", "desktop::ryoku::tests::fallback_detects_owned_actor_after_its_running_image_is_unlinked", "--nocapture"])
            .env(READY, &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // A parallel fork can briefly inherit fs::copy's write FD before
        // CLOEXEC closes it. Retry only ETXTBSY on this owned test image;
        // no image-name/identity predicate or production admission is changed.
        fixture.child =
            Some(retry_owned_image_busy(Duration::from_secs(1), || command.spawn()).unwrap());
        let pid = fixture.child.as_ref().unwrap().id();
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let uid = rustix::process::getuid().as_raw();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                fixture.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "owned actor exited before readiness"
            );
            assert!(Instant::now() < deadline, "owned actor did not become ready");
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(fs::read(&ready).unwrap(), b"ready\n");
        assert_eq!(fs::read_link(proc.join("exe")).unwrap(), fs::canonicalize(&image).unwrap());
        assert!(has_uid(&fs::read_to_string(proc.join("status")).unwrap(), uid));
        assert!(ryoku_process_image(&fs::read_link(proc.join("exe")).unwrap()));
        fs::remove_file(&image).unwrap();
        let unlinked = fs::read_link(proc.join("exe")).unwrap();
        assert!(unlinked.as_os_str().to_string_lossy().ends_with("/ryogami-live (deleted)"));
        assert!(ryoku_process_image(&unlinked));
        assert!(fixture.child.as_mut().unwrap().try_wait().unwrap().is_none());
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(pid as i32).unwrap(),
            rustix::process::Signal::STOP,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = fs::read_to_string(proc.join("status")).unwrap();
            if status.lines().any(|line| line.starts_with("State:\tT")) {
                assert!(has_uid(&status, uid));
                assert!(ryoku_process_image(&fs::read_link(proc.join("exe")).unwrap()));
                break;
            }
            assert!(Instant::now() < deadline, "owned fixture did not stop");
            thread::sleep(Duration::from_millis(1));
        }
        let child = fixture.child.as_mut().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        fixture.child = None;
        assert!(!proc.exists(), "owned fixture was not reaped");
    }

    #[test]
    fn ryoku_entry_is_exact_and_does_not_accept_hidden_or_rewritten_commands() {
        let entry =
            include_str!("../../../../packaging/rpm/wayland-sessions/ryoku-opensuse.desktop");
        assert!(ryoku_entry(entry).unwrap());
        for changed in [
            entry.replace("compositor", "shell"),
            entry.replace("Hyprland", "KDE"),
            format!("{entry}Hidden=true\n"),
            format!("{entry}Exec=/bin/sh\n"),
        ] {
            assert!(!ryoku_entry(&changed).unwrap());
        }
    }

    #[test]
    fn ryoku_login_requires_native_active_password_session_not_kde_or_ssh() {
        let raw = "Name=ryoku-desktop\nUser=1001\nService=sddm\nClass=user\nSeat=seat0\nType=wayland\nDesktop=Hyprland\nActive=yes\nState=active\nRemote=no\n";
        assert!(ryoku_session(&properties(raw).unwrap(), 1001));
        for (old, new) in [
            ("Desktop=Hyprland", "Desktop=KDE"),
            ("Service=sddm", "Service=sshd"),
            ("User=1001", "User=0"),
            ("Remote=no", "Remote=yes"),
            ("Active=yes", "Active=no"),
        ] {
            assert!(!ryoku_session(&properties(&raw.replace(old, new)).unwrap(), 1001));
        }
    }

    #[test]
    fn independent_unit_readiness_and_fallback_never_accept_starting_or_wrong_owner() {
        let raw = format!(
            "Id={SHELL}\nLoadState=loaded\nActiveState=active\nSubState=running\nMainPID=42\nControlGroup=/user.slice/user-1001.slice/user@1001.service/session.slice/{SHELL}\nFragmentPath=/usr/lib/systemd/user/{SHELL}\nPartOf={TARGET}\n"
        );
        let values = properties(&raw).unwrap();
        assert!(unit_ready(&values, SHELL, 1001));
        assert!(!unit_ready(&values, SERVICE, 1001));
        assert!(!unit_ready(&values, SHELL, 1002));
        assert!(!unit_stopped(&values, SHELL));
        for (old, new) in [
            ("SubState=running", "SubState=start"),
            ("MainPID=42", "MainPID=0"),
            ("LoadState=loaded", "LoadState=not-found"),
            ("PartOf=ryoku-desktop-session.target", "PartOf="),
            ("/usr/lib/systemd", "/etc/systemd"),
        ] {
            assert!(!unit_ready(&properties(&raw.replace(old, new)).unwrap(), SHELL, 1001));
        }
        for state in ["inactive", "failed"] {
            let stopped = properties(&format!(
                "Id={SHELL}\nLoadState=loaded\nActiveState={state}\nSubState={}\nMainPID=0\nFragmentPath=/usr/lib/systemd/user/{SHELL}\n",
                if state == "failed" { "failed" } else { "dead" }
            ))
            .unwrap();
            assert!(unit_stopped(&stopped, SHELL));
            assert!(!unit_stopped(&stopped, SERVICE));
        }
        let target = properties(&format!(
            "Id={TARGET}\nLoadState=loaded\nActiveState=inactive\nSubState=dead\nFragmentPath=/usr/lib/systemd/user/{TARGET}\n"
        ))
        .unwrap();
        assert!(unit_stopped(&target, TARGET));
        assert!(!unit_stopped(&target, SHELL));
        let manager = properties("Id=user@1001.service\nLoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\n").unwrap();
        assert!(manager_stopped(&manager, 1001));
        assert!(!manager_stopped(&manager, 1002));
        let mut running = manager.clone();
        running.insert("MainPID".into(), "42".into());
        assert!(!manager_stopped(&running, 1001));
    }

    #[test]
    fn environment_identity_is_account_and_session_bound_without_shell_evaluation() {
        let raw = b"HOME=/home/ryoku-desktop\0XDG_RUNTIME_DIR=/run/user/1001\0XDG_SESSION_TYPE=wayland\0XDG_CURRENT_DESKTOP=Hyprland\0XDG_SESSION_ID=2\0WAYLAND_DISPLAY=wayland-0\0HYPRLAND_INSTANCE_SIGNATURE=abcdef_123\0";
        let values = environment(raw).unwrap();
        assert!(session_environment(&values, 1001, "2"));
        assert!(!session_environment(&values, 1001, "3"));
        assert!(!session_environment(&values, 0, "2"));
        for (key, value) in [
            ("HOME", "/other"),
            ("WAYLAND_DISPLAY", "../socket"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "$(touch file)"),
        ] {
            let mut changed = values.clone();
            changed.insert(key.into(), value.into());
            assert!(!session_environment(&changed, 1001, "2"));
        }
        assert!(environment(b"HOME=a\0HOME=b\0").is_err());
        assert!(manager_environment("X=1\nX=2\n").is_err());
        assert!(environment(&vec![0; 1024 * 1024 + 1]).is_err());
    }

    #[test]
    fn process_start_parser_handles_comm_parentheses_and_refuses_incomplete_stat() {
        let stat = format!("42 (strange ) comm) S {} 987 0", vec!["0"; 18].join(" "));
        assert_eq!(process_start(&stat).unwrap(), 987);
        assert!(process_start("42 (test) S 0").is_err());
        assert!(process_start("no stat").is_err());
    }
}
