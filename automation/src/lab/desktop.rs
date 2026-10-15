//! Native SDDM/PAM login acceptance in the explicitly disposable guest.
//! Credentials arrive only on stdin; GUI input is supplied by the owning host.
use crate::{
    guest::fixture_only,
    support::{Result, require},
};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const USER: &str = "ryoku-desktop";
const HOME: &str = "/home/ryoku-desktop";
const RECORD: &str = "/var/lib/ryoku-lab-desktop";
const RECORD_HEADER: &str = "disposable-issue13-desktop\n";
const SESSION_DIR: &str = "/usr/share/wayland-sessions";

// Do not include native output in errors: PAM/password tools may echo inputs.
fn native(program: &str, args: &[&str]) -> Result<String> {
    let result = Command::new(program).args(args).env("LC_ALL", "C").output()?;
    require(result.status.success(), &format!("native {program} failed"))?;
    Ok(String::from_utf8(result.stdout)?)
}

struct Secret(Vec<u8>);

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

fn password_input(input: &mut impl Read) -> Result<Secret> {
    let mut bytes = Secret(Vec::new());
    input.take(66).read_to_end(&mut bytes.0)?;
    require(
        (25..=65).contains(&bytes.0.len())
            && bytes.0.last() == Some(&b'\n')
            && bytes.0[..bytes.0.len() - 1]
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "desktop password stdin must be 24-64 lowercase ASCII alphanumeric bytes and one newline",
    )?;
    bytes.0.pop();
    Ok(bytes)
}

fn guard() -> Result<()> {
    fixture_only()?;
    require(
        native("/usr/bin/id", &["-u"])?.trim() == "0",
        "desktop inspection requires fixture root",
    )?;
    require(
        fs::read_to_string("/sys/fs/selinux/enforce")?.trim() == "1",
        "SELinux must remain enforcing",
    )
}

fn rpm_owner(path: &Path, expected: &str) -> Result<()> {
    let path = path.to_str().ok_or("non-UTF8 native path")?;
    require(
        native("/usr/bin/rpm", &["-qf", "--qf", "%{NAME}", path])? == expected,
        "desktop file has unexpected native RPM owner",
    )
}

fn rpm_intact(path: &Path, expected: &str) -> Result<()> {
    rpm_owner(path, expected)?;
    let name = path.to_str().ok_or("non-UTF8 native path")?;
    // rpm verifies the whole owning package. Match only this exact file;
    // unrelated machine configuration changes do not substitute for its digest.
    let result = Command::new("/usr/bin/rpm").args(["-Vf", name]).env("LC_ALL", "C").output()?;
    require(
        matches!(result.status.code(), Some(0 | 1)) && result.stderr.is_empty(),
        "native RPM verification failed",
    )?;
    let report = String::from_utf8(result.stdout)?;
    require(
        !report.lines().any(|line| line.split_whitespace().last() == Some(name)),
        "native desktop executable/session/PAM file differs from its RPM",
    )
}

fn native_owner(path: &Path, allowed: &[&str]) -> Result<String> {
    let name = native(
        "/usr/bin/rpm",
        &["-qf", "--qf", "%{NAME}", path.to_str().ok_or("invalid native file path")?],
    )?;
    require(
        allowed.contains(&name.as_str()),
        "file is not owned by a supported native desktop package",
    )?;
    rpm_intact(path, &name)?;
    Ok(name)
}

fn ini(source: &str) -> Result<BTreeMap<(String, String), String>> {
    let mut section = String::from("General");
    let mut values = BTreeMap::new();
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_owned();
        } else {
            let (key, value) =
                line.split_once('=').ok_or("malformed native desktop/SDDM config")?;
            values.insert((section.clone(), key.trim().to_owned()), value.trim().to_owned());
        }
    }
    Ok(values)
}

fn value<'a>(
    values: &'a BTreeMap<(String, String), String>,
    section: &str,
    key: &str,
) -> Option<&'a str> {
    values.get(&(section.to_owned(), key.to_owned())).map(String::as_str)
}

fn plasma_entry(source: &str) -> Result<bool> {
    let values = ini(source)?;
    let desktop = |key| value(&values, "Desktop Entry", key);
    Ok(desktop("DesktopNames").is_some_and(|names| names.split(';').any(|name| name == "KDE"))
        && desktop("TryExec") == Some("/usr/bin/startplasma-wayland")
        && desktop("Exec").is_some_and(|exec| {
            exec.split_whitespace().any(|word| word == "/usr/bin/startplasma-wayland")
        })
        && !desktop("Hidden").is_some_and(|v| v.eq_ignore_ascii_case("true"))
        && !desktop("NoDisplay").is_some_and(|v| v.eq_ignore_ascii_case("true")))
}

fn plasma_session() -> Result<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(SESSION_DIR)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "desktop")
            && plasma_entry(&fs::read_to_string(&path)?)?
        {
            require(
                fs::canonicalize(&path)?.parent() == Some(Path::new(SESSION_DIR)),
                "non-native Plasma session path",
            )?;
            rpm_intact(&path, "plasma6-session")?;
            found.push(path);
        }
    }
    require(found.len() == 1, "exactly one installed native Plasma Wayland session is required")?;
    native_owner(
        Path::new("/usr/bin/startplasma-wayland"),
        &["plasma6-session", "plasma6-workspace"],
    )?;
    Ok(found.remove(0))
}

fn authentication_policy() -> Result<()> {
    let pam = if Path::new("/etc/pam.d/sddm").exists() {
        Path::new("/etc/pam.d/sddm")
    } else {
        Path::new("/usr/lib/pam.d/sddm")
    };
    rpm_intact(pam, "sddm")?;
    for directory in ["/usr/lib/sddm/sddm.conf.d", "/etc/sddm.conf.d"] {
        match fs::read_dir(directory) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    if path.is_file() {
                        reject_autologin(&fs::read_to_string(path)?)?;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    match fs::read_to_string("/etc/sddm.conf") {
        Ok(source) => reject_autologin(&source)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn reject_autologin(source: &str) -> Result<()> {
    let values = ini(source)?;
    require(
        value(&values, "Autologin", "User").is_none_or(str::is_empty)
            && value(&values, "Autologin", "Relogin")
                .is_none_or(|v| v.eq_ignore_ascii_case("false")),
        "native password login is required; existing SDDM autologin/relogin rejected",
    )
}

fn passwd(user: &str) -> Result<Vec<String>> {
    let output = native("/usr/bin/getent", &["passwd", user])?;
    let fields: Vec<_> = output.trim_end().split(':').map(str::to_owned).collect();
    require(
        fields.len() == 7 && fields[0] == user && !output.trim_end().contains('\n'),
        "unexpected native account metadata",
    )?;
    Ok(fields)
}

fn desktop_uid() -> Result<u32> {
    let account = passwd(USER)?;
    let uid: u32 = account[2].parse()?;
    let gid: u32 = account[3].parse()?;
    require(
        (1000..60000).contains(&uid) && gid != 0 && account[5] == HOME && account[6] == "/bin/bash",
        "desktop account is not an ordinary fixture user",
    )?;
    let groups = native("/usr/bin/id", &["-G", USER])?;
    require(
        groups.trim() == gid.to_string(),
        "desktop user must have only its ordinary private group",
    )?;
    let group = native("/usr/bin/getent", &["group", &gid.to_string()])?;
    let fields: Vec<_> = group.trim_end().split(':').collect();
    require(
        fields.len() == 4 && fields[0] == USER && fields[3].is_empty(),
        "desktop primary group is not private",
    )?;
    let home = fs::symlink_metadata(HOME)?;
    require(
        home.is_dir() && home.uid() == uid && home.gid() == gid,
        "desktop home ownership changed",
    )?;
    Ok(uid)
}

fn record(uid: u32, session: &Path) -> Result<String> {
    Ok(format!(
        "{RECORD_HEADER}uid={uid}\nsession={}\n",
        session.to_str().ok_or("invalid session path")?
    ))
}

fn existing_record() -> Result<Option<String>> {
    match fs::symlink_metadata(RECORD) {
        Ok(metadata) => {
            require(
                metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o777 == 0o600,
                "desktop fixture record is not private root-owned data",
            )?;
            Ok(Some(fs::read_to_string(RECORD)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn set_password(password: &Secret) -> Result<()> {
    let mut input = Secret(format!("{USER}:").into_bytes());
    input.0.extend_from_slice(&password.0);
    input.0.push(b'\n');
    let mut child = Command::new("/usr/sbin/chpasswd")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let write = child.stdin.take().ok_or("native password stdin unavailable")?.write_all(&input.0);
    let status = child.wait()?;
    write?;
    require(status.success(), "native fixture password provisioning failed")
}

fn properties(source: &str) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for line in source.lines() {
        let (key, value) = line.split_once('=').ok_or("malformed loginctl properties")?;
        require(
            values.insert(key.to_owned(), value.to_owned()).is_none(),
            "duplicate loginctl property",
        )?;
    }
    Ok(values)
}

fn show_session(id: &str) -> Result<BTreeMap<String, String>> {
    require(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid native session id",
    )?;
    properties(&native("/usr/bin/loginctl", &["show-session", id, "--no-pager"])?)
}

fn active_session() -> Result<String> {
    let id = native(
        "/usr/bin/loginctl",
        &["show-seat", "seat0", "--property=ActiveSession", "--value"],
    )?;
    let id = id.trim().to_owned();
    require(!id.is_empty(), "seat0 has no active native session")?;
    Ok(id)
}

fn matches(values: &BTreeMap<String, String>, expected: &[(&str, &str)]) -> bool {
    expected.iter().all(|(key, expected)| values.get(*key).map(String::as_str) == Some(*expected))
}

fn login_session_diagnostic(values: &BTreeMap<String, String>) -> String {
    ["Name", "User", "Service", "Class", "Seat", "Type", "Desktop", "Active", "State", "Remote"]
        .iter()
        .map(|key| format!("{key}={}", values.get(*key).map(String::as_str).unwrap_or("<missing>")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn greeter_ready() -> Result<()> {
    display_manager()?;
    let values = show_session(&active_session()?)?;
    require(
        matches(
            &values,
            &[
                ("Name", "sddm"),
                ("Service", "sddm-greeter"),
                ("Class", "greeter"),
                ("Seat", "seat0"),
                ("Active", "yes"),
                ("State", "active"),
                ("Remote", "no"),
            ],
        ),
        "active native SDDM greeter on seat0 is not ready",
    )?;
    let uid = passwd("sddm")?[2].parse::<u32>()?;
    let mut processes = Vec::new();
    for name in ["sddm-greeter", "sddm-greeter-qt6", "sddm-greeter-qt5"] {
        processes.extend(process_ids(name, uid)?);
    }
    require(processes.len() == 1, "exactly one native SDDM greeter process required")?;
    for pid in processes {
        native_owner(
            &fs::read_link(format!("/proc/{pid}/exe"))?,
            &["sddm", "sddm-greeter-qt6", "sddm-greeter-qt5"],
        )?;
    }
    Ok(())
}

fn display_manager() -> Result<()> {
    native("/usr/bin/systemctl", &["is-active", "display-manager"])?;
    let processes = process_ids("sddm", 0)?;
    require(processes.len() == 1, "exactly one native SDDM display manager required")?;
    let pid = processes[0];
    let main = native(
        "/usr/bin/systemctl",
        &["show", "display-manager", "--property=MainPID", "--value"],
    )?;
    require(
        main.trim() == pid.to_string(),
        "active display manager is not the native SDDM daemon",
    )?;
    let executable = fs::read_link(format!("/proc/{pid}/exe"))?;
    require(
        executable == fs::canonicalize("/usr/bin/sddm")?,
        "unexpected native SDDM daemon path",
    )?;
    rpm_intact(&executable, "sddm")
}

fn registered_sessions(uid: u32) -> Result<Vec<String>> {
    let output = native("/usr/bin/loginctl", &["list-sessions", "--no-legend", "--no-pager"])?;
    parse_sessions(&output, uid)
}

fn parse_sessions(output: &str, uid: u32) -> Result<Vec<String>> {
    let mut sessions = Vec::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        require(fields.len() >= 3, "malformed native session inventory")?;
        let owner: u32 = fields[1].parse()?;
        if owner == uid {
            require(fields[2] == USER, "native desktop session identity changed")?;
            sessions.push(fields[0].to_owned());
        } else {
            require(fields[2] != USER, "native desktop username has another UID")?;
        }
    }
    Ok(sessions)
}

pub fn prepare() -> Result<()> {
    guard()?;
    let password = password_input(&mut std::io::stdin().lock())?;
    authentication_policy()?;
    let session = plasma_session()?;
    let existing = existing_record()?;
    if existing.is_none() {
        // Refuse any pre-existing account, home, or group; do not adopt user data.
        let account = Command::new("/usr/bin/getent").args(["passwd", USER]).output()?;
        let group = Command::new("/usr/bin/getent").args(["group", USER]).output()?;
        require(
            account.status.code() == Some(2) && group.status.code() == Some(2),
            "refuse to adopt an existing desktop account/group",
        )?;
        require(
            matches!(fs::symlink_metadata(HOME), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "refuse to adopt an existing desktop home",
        )?;
        native(
            "/usr/sbin/useradd",
            &["--create-home", "--user-group", "--shell", "/bin/bash", USER],
        )?;
    }
    let uid = desktop_uid()?;
    let expected = record(uid, &session)?;
    if let Some(existing) = existing {
        require(
            existing == expected,
            "desktop account does not match this lab's recorded fixture",
        )?;
    } else {
        let mut file =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(RECORD)?;
        file.write_all(expected.as_bytes())?;
        file.sync_all()?;
    }
    require(
        registered_sessions(uid)?.is_empty(),
        "refuse to reset credentials while desktop user has a session",
    )?;
    set_password(&password)?;

    // SDDM's native state file preselects the last user and full session path.
    // It does not authenticate or launch the user's session.
    native("/usr/bin/systemctl", &["stop", "display-manager"])?;
    let sddm = passwd("sddm")?;
    let home = Path::new(&sddm[5]);
    require(
        home.is_absolute() && home.starts_with("/var/lib") && home.is_dir(),
        "unexpected native SDDM state home",
    )?;
    let path = home.join("state.conf");
    match fs::symlink_metadata(&path) {
        Ok(metadata) => require(metadata.is_file(), "unsafe SDDM state file")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut state =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    state.write_all(format!("[Last]\nUser={USER}\nSession={}\n", session.display()).as_bytes())?;
    state.sync_all()?;
    native("/usr/bin/chown", &["sddm:sddm", path.to_str().ok_or("invalid SDDM state path")?])?;
    native("/usr/bin/chmod", &["600", path.to_str().ok_or("invalid SDDM state path")?])?;
    native(
        "/usr/sbin/restorecon",
        &["-F", path.to_str().ok_or("invalid SDDM state path")?, RECORD, HOME],
    )?;
    native("/usr/bin/systemctl", &["start", "display-manager"])?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if greeter_ready().is_ok() {
            break;
        }
        require(
            Instant::now() < deadline,
            "native SDDM greeter did not become ready in 120 seconds; inspect guest interactively",
        )?;
        thread::sleep(Duration::from_secs(1));
    }
    guard()?;
    println!(
        "desktop_login_prepared=passed\ndesktop_user={USER}\ndesktop_session_type=wayland\ndesktop_greeter=active"
    );
    Ok(())
}

fn process_ids(name: &str, uid: u32) -> Result<Vec<u32>> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let path = entry.path();
        let Ok(status) = fs::read_to_string(path.join("status")) else {
            continue;
        };
        if !process_status(&status, uid) {
            continue;
        }
        let Ok(exe) = fs::read_link(path.join("exe")) else {
            continue;
        };
        if exe.file_name().is_some_and(|filename| filename == name) {
            found.push(pid);
        }
    }
    Ok(found)
}

fn process_status(source: &str, uid: u32) -> bool {
    let owner = source.lines().find_map(|line| line.strip_prefix("Uid:"));
    let live = source.lines().find_map(|line| line.strip_prefix("State:"));
    owner.is_some_and(|ids| {
        let ids: Vec<_> = ids.split_whitespace().collect();
        ids.len() == 4 && ids.iter().all(|id| *id == uid.to_string())
    }) && live
        .is_some_and(|state| matches!(state.split_whitespace().next(), Some("R" | "S" | "D" | "I")))
}

fn session_matches(values: &BTreeMap<String, String>, uid: u32) -> bool {
    matches(
        values,
        &[
            ("Name", USER),
            ("User", &uid.to_string()),
            ("Service", "sddm"),
            ("Class", "user"),
            ("Seat", "seat0"),
            ("Type", "wayland"),
            ("Desktop", "KDE"),
            ("Active", "yes"),
            ("State", "active"),
            ("Remote", "no"),
        ],
    )
}

fn user_process(name: &str, package: &str, uid: u32, scope: &str) -> Result<u32> {
    let processes = process_ids(name, uid)?;
    require(
        processes.len() == 1,
        "exactly one live native desktop process for fixture user required",
    )?;
    let pid = processes[0];
    let exe = fs::read_link(format!("/proc/{pid}/exe"))?;
    require(
        exe == fs::canonicalize(format!("/usr/bin/{name}"))?,
        "unexpected native desktop executable path",
    )?;
    rpm_intact(&exe, package)?;
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
    require(
        process_cgroup(&cgroup, uid, scope),
        "desktop process is outside fixture user's native session/manager",
    )?;
    Ok(pid)
}

fn process_cgroup(source: &str, uid: u32, scope: &str) -> bool {
    source.lines().any(|line| {
        let Some(path) = line.strip_prefix("0::") else {
            return false;
        };
        let prefix = format!("/user.slice/user-{uid}.slice/");
        let Some(path) = path.strip_prefix(&prefix) else {
            return false;
        };
        path == scope
            || path.starts_with(&format!("{scope}/"))
            || path == format!("user@{uid}.service")
            || path.starts_with(&format!("user@{uid}.service/"))
    })
}

fn plasma_socket(environment: &[u8], uid: u32) -> Result<PathBuf> {
    let env: BTreeMap<_, _> = environment
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let equals = entry.iter().position(|b| *b == b'=')?;
            Some((&entry[..equals], &entry[equals + 1..]))
        })
        .collect();
    let runtime = format!("/run/user/{uid}");
    require(
        env.get(b"XDG_RUNTIME_DIR".as_slice()).copied() == Some(runtime.as_bytes()),
        "Plasma runtime directory does not match ordinary user",
    )?;
    require(
        env.get(b"XDG_SESSION_TYPE".as_slice()).copied() == Some(b"wayland"),
        "Plasma process is not in a Wayland session",
    )?;
    let display = env.get(b"WAYLAND_DISPLAY".as_slice()).ok_or("Plasma Wayland display missing")?;
    let display = std::str::from_utf8(display)?;
    require(
        display.starts_with("wayland-")
            && display.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "unexpected native Wayland display name",
    )?;
    Ok(Path::new(&runtime).join(display))
}

pub fn inspect() -> Result<()> {
    match inspect_inner() {
        Ok(()) => Ok(()),
        Err(reason) => {
            println!("desktop_login_reason={}", reason.code());
            Err("desktop login inspection failed".into())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginFailureReason {
    Precondition,
    ActiveSession,
    PlasmaShell,
    KWin,
    WaylandSocketAbsent,
    WaylandSocketConnect,
}

impl LoginFailureReason {
    fn code(self) -> &'static str {
        match self {
            Self::Precondition => "inspection_precondition_failed",
            Self::ActiveSession => "active_session_absent_or_mismatch",
            Self::PlasmaShell => "plasma_shell_absent",
            Self::KWin => "kwin_absent",
            Self::WaylandSocketAbsent => "wayland_socket_absent",
            Self::WaylandSocketConnect => "wayland_socket_connect_failure",
        }
    }
}

fn inspect_inner() -> std::result::Result<(), LoginFailureReason> {
    guard().map_err(|_| LoginFailureReason::Precondition)?;
    authentication_policy().map_err(|_| LoginFailureReason::Precondition)?;
    let session = plasma_session().map_err(|_| LoginFailureReason::Precondition)?;
    let uid = desktop_uid().map_err(|_| LoginFailureReason::Precondition)?;
    require(
        existing_record().map_err(|_| LoginFailureReason::Precondition)?
            == Some(record(uid, &session).map_err(|_| LoginFailureReason::Precondition)?),
        "desktop fixture identity record is absent or changed",
    )
    .map_err(|_| LoginFailureReason::Precondition)?;
    display_manager().map_err(|_| LoginFailureReason::Precondition)?;
    let id = active_session().map_err(|_| LoginFailureReason::ActiveSession)?;
    let values = show_session(&id).map_err(|_| LoginFailureReason::ActiveSession)?;
    require(
        session_matches(&values, uid),
        &format!(
            "ordinary-user Plasma Wayland SDDM/PAM session properties did not match: {}",
            login_session_diagnostic(&values)
        ),
    )
    .map_err(|_| LoginFailureReason::ActiveSession)?;
    let scope = values.get("Scope").ok_or(LoginFailureReason::ActiveSession)?;
    require(scope == &format!("session-{id}.scope"), "unexpected native login session scope")
        .map_err(|_| LoginFailureReason::ActiveSession)?;
    let user_sessions = native(
        "/usr/bin/loginctl",
        &["show-user", &uid.to_string(), "--property=Sessions", "--value"],
    )
    .map_err(|_| LoginFailureReason::ActiveSession)?;
    require(
        user_sessions.split_whitespace().any(|candidate| candidate == id),
        "active SDDM session is not registered for ordinary user",
    )
    .map_err(|_| LoginFailureReason::ActiveSession)?;
    for candidate in user_sessions.split_whitespace() {
        if candidate != id {
            let other = show_session(candidate).map_err(|_| LoginFailureReason::ActiveSession)?;
            require(
                other.get("Class").is_some_and(|class| class == "manager"),
                "desktop user has another login session; attribution is ambiguous",
            )
            .map_err(|_| LoginFailureReason::ActiveSession)?;
        }
    }
    let shell = user_process("plasmashell", "plasma6-workspace", uid, scope)
        .map_err(|_| LoginFailureReason::PlasmaShell)?;
    let compositor =
        user_process("kwin_wayland", "kwin6", uid, scope).map_err(|_| LoginFailureReason::KWin)?;
    let socket = plasma_socket(
        &fs::read(format!("/proc/{shell}/environ"))
            .map_err(|_| LoginFailureReason::WaylandSocketAbsent)?,
        uid,
    )
    .map_err(|_| LoginFailureReason::WaylandSocketAbsent)?;
    let metadata =
        fs::symlink_metadata(&socket).map_err(|_| LoginFailureReason::WaylandSocketAbsent)?;
    require(
        metadata.file_type().is_socket() && metadata.uid() == uid,
        "ordinary user's Wayland compositor socket missing",
    )
    .map_err(|_| LoginFailureReason::WaylandSocketAbsent)?;
    let _connection =
        UnixStream::connect(socket).map_err(|_| LoginFailureReason::WaylandSocketConnect)?;
    // Recheck after executable/socket inspection to reject session exits/races.
    require(
        active_session().map_err(|_| LoginFailureReason::ActiveSession)? == id
            && session_matches(
                &show_session(&id).map_err(|_| LoginFailureReason::ActiveSession)?,
                uid,
            ),
        "native Plasma session changed during inspection",
    )
    .map_err(|_| LoginFailureReason::ActiveSession)?;
    require(
        process_status(
            &fs::read_to_string(format!("/proc/{shell}/status"))
                .map_err(|_| LoginFailureReason::PlasmaShell)?,
            uid,
        ),
        "native Plasma shell exited during inspection",
    )
    .map_err(|_| LoginFailureReason::PlasmaShell)?;
    require(
        process_status(
            &fs::read_to_string(format!("/proc/{compositor}/status"))
                .map_err(|_| LoginFailureReason::KWin)?,
            uid,
        ),
        "native desktop process exited during inspection",
    )
    .map_err(|_| LoginFailureReason::KWin)?;
    guard().map_err(|_| LoginFailureReason::Precondition)?;
    println!(
        "desktop_login=passed\ndesktop_user={USER}\ndesktop_session_service=sddm\ndesktop_session_class=user\ndesktop_session_seat=seat0\ndesktop_session_type=wayland\ndesktop_session_state=active\ndesktop_session_remote=no\ndesktop_session_owner=ordinary-test-user\ndesktop_pam=native-sddm\ndesktop_plasmashell=plasma6-workspace\ndesktop_compositor=kwin6\ndesktop_wayland_socket=connected\ndesktop_authentication=password-required"
    );
    Ok(())
}

fn plasma_environment(uid: u32) -> Result<BTreeMap<String, String>> {
    let shell = user_process(
        "plasmashell",
        "plasma6-workspace",
        uid,
        &format!("session-{}", active_session()?),
    )?;
    let raw = fs::read(format!("/proc/{shell}/environ"))?;
    let values: BTreeMap<_, _> = raw
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let equals = entry.iter().position(|byte| *byte == b'=')?;
            Some((entry[..equals].to_vec(), entry[equals + 1..].to_vec()))
        })
        .collect();
    let get = |key: &[u8]| -> Result<String> {
        String::from_utf8(
            values.get(key).ok_or("Plasma session environment is incomplete")?.clone(),
        )
        .map_err(Into::into)
    };
    let runtime = format!("/run/user/{uid}");
    let display = get(b"WAYLAND_DISPLAY")?;
    let session_type = get(b"XDG_SESSION_TYPE")?;
    require(session_type == "wayland", "GTK proof requires the Plasma Wayland session")?;
    let socket = plasma_socket(&raw, uid)?;
    require(
        socket == Path::new(&runtime).join(&display),
        "Plasma display changed during GTK setup",
    )?;
    let metadata = fs::symlink_metadata(socket)?;
    require(
        metadata.file_type().is_socket() && metadata.uid() == uid,
        "ordinary user's Wayland compositor socket missing",
    )?;
    let bus = get(b"DBUS_SESSION_BUS_ADDRESS")?;
    require(
        bus == format!("unix:path={runtime}/bus"),
        "unexpected ordinary-user desktop bus address",
    )?;
    Ok(BTreeMap::from([
        ("HOME".into(), HOME.into()),
        ("USER".into(), USER.into()),
        ("LOGNAME".into(), USER.into()),
        ("XDG_RUNTIME_DIR".into(), runtime),
        ("XDG_SESSION_TYPE".into(), session_type),
        ("WAYLAND_DISPLAY".into(), display),
        ("DBUS_SESSION_BUS_ADDRESS".into(), bus),
        ("PATH".into(), "/usr/bin:/bin".into()),
    ]))
}

fn ui_processes(uid: u32) -> Result<Vec<u32>> {
    let processes = process_ids("ryoku-ui-gtk", uid)?;
    for pid in &processes {
        require(
            fs::read_link(format!("/proc/{pid}/exe"))? == Path::new("/usr/local/bin/ryoku-ui-gtk"),
            "GTK proof process has an unexpected executable",
        )?;
    }
    Ok(processes)
}

/// Start the opt-in GTK proof inside the already authenticated Wayland user
/// session. Only a fixed success/error scenario is accepted.
pub fn launch_ui(scenario: &str) -> Result<()> {
    guard()?;
    inspect()?;
    require(matches!(scenario, "success" | "error"), "unsupported GTK proof scenario")?;
    let uid = desktop_uid()?;
    require(ui_processes(uid)?.is_empty(), "another GTK proof window is still active")?;
    let environment = plasma_environment(uid)?;
    let mut command = Command::new("/usr/sbin/runuser");
    command.args(["--user", USER, "--", "/usr/bin/env", "-i"]);
    for (name, value) in environment {
        command.arg(format!("{name}={value}"));
    }
    command.arg("/usr/local/bin/ryoku-ui-gtk").args(["--demo", "--capture-on-launch"]);
    if scenario == "error" {
        command.arg("--service-unavailable");
    }
    // Keep renderer/runtime errors until the host retrieves them. A fresh
    // root-owned file prevents following or replacing a pre-existing path.
    let log = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(format!("/var/log/ryoku-lab-ui-{scenario}-private.log"))?;
    command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    let _child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if !ui_processes(uid)?.is_empty() {
            println!("gtk_proof_window=running\ngtk_proof_scenario={scenario}");
            return Ok(());
        }
        require(Instant::now() < deadline, "GTK proof window did not remain active")?;
        thread::sleep(Duration::from_millis(200));
    }
}

/// Retrieve only the fixed proof logs, bounded independently of GTK output.
/// The host retains these in its private run directory before discarding disks.
pub fn ui_diagnostics() -> Result<()> {
    guard()?;
    let mut output = std::io::stdout().lock();
    for scenario in ["success", "error"] {
        let path = format!("/var/log/ryoku-lab-ui-{scenario}-private.log");
        let file = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        require(metadata.is_file() && metadata.uid() == 0, "invalid private GTK log")?;
        writeln!(output, "--- GTK {scenario} ---")?;
        std::io::copy(&mut file.take(128 * 1024), &mut output)?;
    }
    Ok(())
}

pub fn stop_ui() -> Result<()> {
    guard()?;
    let uid = desktop_uid()?;
    let processes = ui_processes(uid)?;
    require(processes.len() <= 1, "ambiguous GTK proof process inventory")?;
    if let Some(pid) = processes.first() {
        native("/usr/bin/kill", &["-TERM", &pid.to_string()])?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while process_ids("ryoku-ui-gtk", uid)?.contains(pid) {
            require(Instant::now() < deadline, "GTK proof window did not exit")?;
            thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_failure_reasons_are_static_and_stage_specific() {
        assert_eq!(LoginFailureReason::ActiveSession.code(), "active_session_absent_or_mismatch");
        assert_eq!(LoginFailureReason::PlasmaShell.code(), "plasma_shell_absent");
        assert_eq!(LoginFailureReason::KWin.code(), "kwin_absent");
        assert_eq!(LoginFailureReason::WaylandSocketAbsent.code(), "wayland_socket_absent");
        assert_eq!(
            LoginFailureReason::WaylandSocketConnect.code(),
            "wayland_socket_connect_failure"
        );
        assert_eq!(LoginFailureReason::Precondition.code(), "inspection_precondition_failed");
    }

    #[test]
    fn secret_stdin_requires_bounded_single_ascii_password() {
        assert!(password_input(&mut &b"abcdefghijklmnopqrstuvwx01234567\n"[..]).is_ok());
        for mut invalid in [
            b"".as_slice(),
            b"short\n",
            b"abcdefghijklmnopqrstuvwx01234567",
            b"abcdefghijklmnopqrstuvwx01234567\n\n",
            b"abcdefghijklmnopqrstuvwx0123456!\n",
            b"Abcdefghijklmnopqrstuvwx01234567\n",
        ] {
            assert!(password_input(&mut invalid).is_err());
        }
        assert!(password_input(&mut format!("{}\n", "a".repeat(65)).as_bytes()).is_err());
    }

    #[test]
    fn accepts_only_native_visible_plasma_wayland_entry() {
        let entry = "[Desktop Entry]\nExec=/usr/libexec/plasma-dbus-run-session-if-needed /usr/bin/startplasma-wayland\nTryExec=/usr/bin/startplasma-wayland\nDesktopNames=KDE\n";
        assert!(plasma_entry(entry).unwrap());
        assert!(!plasma_entry(&entry.replace("startplasma-wayland", "startplasma-x11")).unwrap());
        assert!(!plasma_entry(&format!("{entry}Hidden=true\n")).unwrap());
        assert!(!plasma_entry(&format!("{entry}NoDisplay=true\n")).unwrap());
    }

    #[test]
    fn autologin_and_relogin_never_satisfy_native_password_proof() {
        assert!(reject_autologin("[Autologin]\nUser=\nRelogin=false\n").is_ok());
        assert!(reject_autologin("[Autologin]\nUser=ryoku-desktop\n").is_err());
        assert!(reject_autologin("[Autologin]\nRelogin=true\n").is_err());
    }

    #[test]
    fn native_login_contract_rejects_root_remote_greeter_and_inactive() {
        let output = "Name=ryoku-desktop\nUser=1001\nService=sddm\nClass=user\nSeat=seat0\nType=wayland\nDesktop=KDE\nActive=yes\nState=active\nRemote=no\n";
        let values = properties(output).unwrap();
        assert!(session_matches(&values, 1001));
        assert!(login_session_diagnostic(&values).contains("User=1001"));
        for (old, new) in [
            ("User=1001", "User=0"),
            ("Service=sddm", "Service=sshd"),
            ("Class=user", "Class=greeter"),
            ("Seat=seat0", "Seat=seat1"),
            ("Type=wayland", "Type=tty"),
            ("Active=yes", "Active=no"),
            ("Remote=no", "Remote=yes"),
            ("Desktop=KDE", "Desktop="),
        ] {
            assert!(!session_matches(&properties(&output.replace(old, new)).unwrap(), 1001));
        }
        assert!(properties("User=1001\nUser=0\n").is_err());
    }

    #[test]
    fn credential_reset_requires_successful_unambiguous_session_inventory() {
        assert!(parse_sessions("", 1001).unwrap().is_empty());
        assert!(
            parse_sessions("2 1000 tester - 42 user tty2 active no -\n", 1001).unwrap().is_empty()
        );
        assert_eq!(
            parse_sessions("3 1001 ryoku-desktop seat0 45 user tty3 active no -\n", 1001).unwrap(),
            ["3"]
        );
        for invalid in [
            "not an inventory\n",
            "3 1001 otheruser\n",
            "3 0 ryoku-desktop\n",
            "3 unknown ryoku-desktop\n",
        ] {
            assert!(parse_sessions(invalid, 1001).is_err());
        }
    }

    #[test]
    fn native_processes_must_be_live_and_owned_entirely_by_user() {
        assert!(process_status("Uid:\t1001\t1001\t1001\t1001\nState:\tS (sleeping)\n", 1001));
        for status in [
            "Uid:\t0\t0\t0\t0\nState:\tS (sleeping)\n",
            "Uid:\t1001\t0\t1001\t1001\nState:\tS (sleeping)\n",
            "Uid:\t1001\t1001\t1001\t1001\nState:\tZ (zombie)\n",
        ] {
            assert!(!process_status(status, 1001));
        }
        assert!(process_cgroup(
            "0::/user.slice/user-1001.slice/user@1001.service/app.slice/plasma.service\n",
            1001,
            "session-2.scope"
        ));
        assert!(process_cgroup(
            "0::/user.slice/user-1001.slice/session-2.scope\n",
            1001,
            "session-2.scope"
        ));
        assert!(!process_cgroup(
            "0::/user.slice/user-1001.slice/session-3.scope\n",
            1001,
            "session-2.scope"
        ));
        assert!(!process_cgroup(
            "0::/user.slice/user-0.slice/user@0.service\n",
            1001,
            "session-2.scope"
        ));
    }

    #[test]
    fn wayland_socket_is_confined_to_native_user_runtime_directory() {
        let env = b"XDG_RUNTIME_DIR=/run/user/1001\0XDG_SESSION_TYPE=wayland\0WAYLAND_DISPLAY=wayland-0\0";
        assert_eq!(plasma_socket(env, 1001).unwrap(), Path::new("/run/user/1001/wayland-0"));
        assert!(plasma_socket(env, 0).is_err());
        assert!(plasma_socket(b"XDG_RUNTIME_DIR=/run/user/1001\0XDG_SESSION_TYPE=wayland\0WAYLAND_DISPLAY=../socket\0", 1001).is_err());
        assert!(plasma_socket(b"XDG_RUNTIME_DIR=/run/user/1001\0XDG_SESSION_TYPE=x11\0WAYLAND_DISPLAY=wayland-0\0", 1001).is_err());
    }
}
