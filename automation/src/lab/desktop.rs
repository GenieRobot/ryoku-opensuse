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
const UI_SERVICE: &str = "/usr/libexec/ryoku/ryoku-service";
const UI_SERVICE_UNIT: &str = "ryoku-lab-ui-service.service";
const UI_SERVICE_RECORD: &str = "/var/lib/ryoku-lab-ui-service";
const UI_SERVICE_DIAGNOSTIC: &str = "/var/log/ryoku-lab-ui-service-startup-private.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSession {
    Kde,
    Ryoku,
}

impl NativeSession {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "kde" => Ok(Self::Kde),
            "ryoku" => Ok(Self::Ryoku),
            _ => Err("native session must be kde or ryoku".into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Kde => "kde",
            Self::Ryoku => "ryoku",
        }
    }
}

fn ryoku_session() -> Result<PathBuf> {
    let entry = Path::new(SESSION_DIR).join("ryoku-opensuse.desktop");
    for path in [&entry, Path::new("/usr/bin/ryoku-session")] {
        let metadata = fs::symlink_metadata(path)?;
        require(
            metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
            "Ryoku session entry/launcher must be root-owned regular files",
        )?;
    }
    require(
        fs::read_to_string(&entry)?
            == include_str!("../../../packaging/rpm/wayland-sessions/ryoku-opensuse.desktop"),
        "installed Ryoku session entry differs from reviewed source",
    )?;
    Ok(entry)
}

/// Select a fixed session for an already recorded fixture account. Never adopt
/// an existing account or stop a live user login to make this operation succeed.
pub fn prepare_existing(selected: NativeSession) -> Result<()> {
    guard()?;
    let password = password_input(&mut std::io::stdin().lock())?;
    authentication_policy()?;
    let plasma = plasma_session()?;
    let uid = desktop_uid()?;
    require(
        existing_record()? == Some(record(uid, &plasma)?),
        "existing desktop fixture identity required",
    )?;
    let session = match selected {
        NativeSession::Kde => plasma,
        NativeSession::Ryoku => ryoku_session()?,
    };
    prepare_greeter(uid, &session, &password)
}

/// Install one frozen daemon for the disposable graphical proof. This is not
/// an RPM installation or activation of the production user service unit.
pub fn install_ui_service(hash: &str) -> Result<()> {
    guard()?;
    require(crate::profile_bundle::valid_digest(hash), "invalid UI daemon digest")?;
    let source = Path::new("/tmp/ryoku-ui-service.upload");
    let metadata = fs::symlink_metadata(source)?;
    require(metadata.is_file() && metadata.nlink() == 1, "unsafe UI daemon upload")?;
    require(crate::profile_bundle::digest(source)? == hash, "UI daemon upload digest mismatch")?;
    crate::preflight::input_binary(source.to_str().ok_or("invalid UI upload")?)?;
    require(
        matches!(fs::symlink_metadata(UI_SERVICE), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "refuse existing installed UI daemon",
    )?;
    let directory = Path::new(UI_SERVICE).parent().ok_or("missing UI daemon parent")?;
    match fs::create_dir(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let parent = fs::symlink_metadata(directory)?;
    require(
        parent.is_dir() && parent.uid() == 0 && parent.mode() & 0o022 == 0,
        "unsafe UI daemon directory",
    )?;
    native(
        "/usr/bin/install",
        &["-Z", "-m", "0755", source.to_str().ok_or("invalid upload")?, UI_SERVICE],
    )?;
    native("/usr/sbin/restorecon", &["-F", UI_SERVICE])?;
    require(
        crate::profile_bundle::digest(Path::new(UI_SERVICE))? == hash,
        "installed UI daemon digest mismatch",
    )?;
    fs::remove_file(source)?;
    Ok(())
}

fn desktop_command(program: &str, args: &[&str]) -> Result<String> {
    let environment = plasma_environment(desktop_uid()?)?;
    let mut command = Command::new("/usr/sbin/runuser");
    command.args(["--user", USER, "--", "/usr/bin/env", "-i"]);
    for (name, value) in environment {
        command.arg(format!("{name}={value}"));
    }
    command.arg(program).args(args).env("LC_ALL", "C");
    let result = crate::process::run(&mut command, Duration::from_secs(30), 64 * 1024)?;
    if result.timed_out
        || !result.status.success()
        || result.stdout_truncated
        || result.stderr_truncated
    {
        // No credentials enter these fixed service commands. The owning host
        // retains this bounded diagnostic privately rather than publishing it.
        eprintln!(
            "desktop UI fixture command failed\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    require(
        !result.timed_out
            && result.status.success()
            && !result.stdout_truncated
            && !result.stderr_truncated,
        "bounded desktop command failed",
    )?;
    Ok(String::from_utf8(result.stdout)?)
}

fn service_journal_command(uid: u32) -> Command {
    let mut command = Command::new("/usr/bin/journalctl");
    // journalctl --user-unit defaults to the caller's UID (root here). Match
    // the actual desktop UID explicitly, including user-manager lifecycle
    // messages. These journal records survive transient --collect removal.
    command.args([
        "--no-pager",
        "--output=short-monotonic",
        "--boot=0",
        "--since=-5min",
        "--lines=128",
        &format!("_UID={uid}"),
        &format!("_SYSTEMD_USER_UNIT={UI_SERVICE_UNIT}"),
        "+",
        &format!("_UID={uid}"),
        &format!("USER_UNIT={UI_SERVICE_UNIT}"),
    ]);
    command.env("LC_ALL", "C");
    command
}

fn diagnostic_output(log: &mut impl Write, name: &str, mut command: Command) -> Result<()> {
    writeln!(log, "--- {name} ---")?;
    match crate::process::run(&mut command, Duration::from_secs(10), 16 * 1024) {
        Ok(output) => {
            writeln!(
                log,
                "exit_code={} timed_out={} stdout_truncated={} stderr_truncated={}",
                output.status.code().unwrap_or(-1),
                output.timed_out,
                output.stdout_truncated,
                output.stderr_truncated,
            )?;
            log.write_all(&output.stdout)?;
            log.write_all(b"\n--- stderr ---\n")?;
            log.write_all(&output.stderr)?;
            log.write_all(b"\n")?;
        }
        Err(_) => writeln!(log, "diagnostic_command=unavailable")?,
    }
    Ok(())
}

fn capture_ui_service_diagnostics(uid: u32, reason: &str) -> Result<()> {
    let mut log = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(UI_SERVICE_DIAGNOSTIC)?;
    writeln!(log, "startup_error={}", reason.chars().take(512).collect::<String>())?;
    // Status must remain inspectable if Plasma itself exited. No session
    // discovery commands or graphical environment are needed for this bus.
    let mut status = Command::new("/usr/sbin/runuser");
    status.args([
        "--user",
        USER,
        "--",
        "/usr/bin/env",
        "-i",
        &format!("HOME={HOME}"),
        &format!("XDG_RUNTIME_DIR=/run/user/{uid}"),
        &format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"),
        "PATH=/usr/bin:/bin",
        "LC_ALL=C",
        "/usr/bin/systemctl",
        "--user",
        "show",
        "--no-pager",
        "--property=LoadState,ActiveState,SubState,MainPID,ExecMainCode,ExecMainStatus,Result,Transient,InvocationID",
        UI_SERVICE_UNIT,
    ]);
    diagnostic_output(&mut log, "user unit state (may already be collected)", status)?;
    diagnostic_output(&mut log, "user unit journal", service_journal_command(uid))?;
    log.sync_all()?;
    Ok(())
}

fn service_property(name: &str) -> Result<String> {
    Ok(desktop_command(
        "/usr/bin/systemctl",
        &["--user", "show", UI_SERVICE_UNIT, "--property", name, "--value"],
    )?
    .trim()
    .to_owned())
}

fn service_unit_loaded() -> Result<bool> {
    let output = desktop_command(
        "/usr/bin/systemctl",
        &["--user", "list-units", "--all", "--plain", "--no-legend", "--no-pager", UI_SERVICE_UNIT],
    )?;
    require(
        output.lines().count() <= 1
            && output.lines().all(|line| line.split_whitespace().next() == Some(UI_SERVICE_UNIT)),
        "unexpected transient UI unit inventory",
    )?;
    Ok(!output.trim().is_empty())
}

fn process_start(stat: &str) -> Result<u64> {
    // comm may itself contain spaces and parentheses. starttime is field 22.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .ok_or("missing daemon process start identity")?
        .parse()
        .map_err(Into::into)
}

fn daemon_identity(pid: u32, uid: u32) -> Result<u64> {
    require(
        process_status(&fs::read_to_string(format!("/proc/{pid}/status"))?, uid),
        "UI daemon owner or liveness changed",
    )?;
    require(
        fs::read_link(format!("/proc/{pid}/exe"))? == Path::new(UI_SERVICE),
        "UI daemon executable changed",
    )?;
    process_start(&fs::read_to_string(format!("/proc/{pid}/stat"))?)
}

fn start_ui_service(uid: u32) -> Result<()> {
    require(
        matches!(fs::symlink_metadata(UI_SERVICE_RECORD), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "refuse existing UI daemon ownership record",
    )?;
    require(!service_unit_loaded()?, "refuse pre-existing UI daemon fixture unit")?;
    require(
        matches!(fs::symlink_metadata(format!("/run/user/{uid}/ryoku-service.sock")), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "refuse existing UI daemon socket",
    )?;
    let metadata = fs::symlink_metadata(UI_SERVICE)?;
    require(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o755,
        "unsafe installed UI daemon",
    )?;
    let submitted = desktop_command(
        "/usr/bin/systemd-run",
        &[
            "--user",
            "--quiet",
            "--collect",
            "--unit",
            UI_SERVICE_UNIT,
            "--property=Type=exec",
            "--property=UMask=0077",
            "--property=RuntimeMaxSec=300",
            UI_SERVICE,
        ],
    );
    if let Err(error) = submitted {
        let _ = capture_ui_service_diagnostics(uid, &error.to_string());
        // A failed submission does not establish ownership of a running unit.
        return Err(error);
    }
    let mut invocation = None;
    let started = (|| -> Result<()> {
        let id = service_property("InvocationID")?;
        require(
            id.len() == 32
                && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                && id.bytes().any(|byte| byte != b'0'),
            "UI daemon unit has no invocation identity",
        )?;
        invocation = Some(id);
        let pid: u32 = service_property("MainPID")?.parse()?;
        require(pid > 1, "UI daemon unit has no process")?;
        let start = daemon_identity(pid, uid)?;
        let mut record = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(UI_SERVICE_RECORD)?;
        writeln!(record, "{pid} {start}")?;
        record.sync_all()?;
        let socket = format!("/run/user/{uid}/ryoku-service.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            require(daemon_identity(pid, uid)? == start, "UI daemon process identity changed")?;
            if fs::symlink_metadata(&socket).is_ok_and(|metadata| {
                metadata.file_type().is_socket()
                    && metadata.uid() == uid
                    && metadata.mode() & 0o077 == 0
            }) {
                break;
            }
            require(Instant::now() < deadline, "UI daemon runtime socket not ready")?;
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })();
    if let Err(error) = &started {
        // Capture before stop/collection; a diagnostic failure must never
        // replace the original startup error or suppress owned-unit cleanup.
        let _ = capture_ui_service_diagnostics(uid, &error.to_string());
        // Stop only the successfully created invocation, never a replacement
        // unit that adopted its name after collection. Unknown identity is
        // left to the native RuntimeMaxSec bound and disposable VM teardown.
        if invocation.as_ref().is_some_and(|expected| {
            service_property("InvocationID").is_ok_and(|current| current == *expected)
        }) {
            let _ = desktop_command("/usr/bin/systemctl", &["--user", "stop", UI_SERVICE_UNIT]);
        }
        let _ = fs::remove_file(UI_SERVICE_RECORD);
    }
    started
}

fn stop_ui_service(uid: u32) -> Result<()> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(UI_SERVICE_RECORD)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    require(
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.nlink() == 1
            && metadata.len() <= 128
            && metadata.mode() & 0o7777 == 0o600,
        "invalid UI daemon ownership record",
    )?;
    let mut text = String::new();
    file.take(128).read_to_string(&mut text)?;
    let fields: Vec<_> = text.split_whitespace().collect();
    require(fields.len() == 2, "invalid UI daemon identity")?;
    let pid: u32 = fields[0].parse()?;
    require(pid > 1, "invalid owned UI daemon PID")?;
    let start: u64 = fields[1].parse()?;
    let active_pid: u32 =
        if service_unit_loaded()? { service_property("MainPID")?.parse()? } else { 0 };
    if active_pid != 0 {
        require(
            active_pid == pid && daemon_identity(pid, uid)? == start,
            "refuse to stop substituted UI daemon",
        )?;
        desktop_command("/usr/bin/systemctl", &["--user", "stop", UI_SERVICE_UNIT])?;
    }
    require(!Path::new(&format!("/proc/{pid}")).exists(), "owned UI daemon was not reaped")?;
    fs::remove_file(UI_SERVICE_RECORD)?;
    Ok(())
}

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
    prepare_greeter(uid, &session, &password)
}

fn prepare_greeter(uid: u32, session: &Path, password: &Secret) -> Result<()> {
    require(
        registered_sessions(uid)?.is_empty(),
        "refuse to reset credentials while desktop user has a session",
    )?;
    set_password(password)?;

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
    session_matches_desktop(values, uid, "KDE")
}

fn session_matches_desktop(values: &BTreeMap<String, String>, uid: u32, desktop: &str) -> bool {
    matches(
        values,
        &[
            ("Name", USER),
            ("User", &uid.to_string()),
            ("Service", "sddm"),
            ("Class", "user"),
            ("Seat", "seat0"),
            ("Type", "wayland"),
            ("Desktop", desktop),
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

fn plasma_splash_active(uid: u32, scope: &str) -> Result<bool> {
    let processes = process_ids("ksplashqml", uid)?;
    require(processes.len() <= 1, "ambiguous Plasma splash process inventory")?;
    let Some(pid) = processes.first().copied() else {
        return Ok(false);
    };
    let exe = fs::read_link(format!("/proc/{pid}/exe"))?;
    require(
        exe == fs::canonicalize("/usr/bin/ksplashqml")?,
        "unexpected Plasma splash executable path",
    )?;
    rpm_intact(&exe, "plasma6-workspace")?;
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
    require(
        process_cgroup(&cgroup, uid, scope),
        "Plasma splash process is outside the authenticated user's session",
    )?;
    Ok(true)
}

fn wait_for_plasma_splash_exit(uid: u32) -> Result<(String, String)> {
    let session = active_session()?;
    let values = show_session(&session)?;
    require(
        session_matches(&values, uid),
        "Plasma splash wait requires the active authenticated KDE session",
    )?;
    let scope = values.get("Scope").ok_or("KDE session scope unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if !plasma_splash_active(uid, scope)? {
            require(
                active_session()? == session,
                "authenticated KDE session changed during splash wait",
            )?;
            return Ok((session, scope.clone()));
        }
        require(Instant::now() < deadline, "Plasma splash obscured the UI proof for 60 seconds")?;
        thread::sleep(Duration::from_millis(250));
    }
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

pub fn inspect_selected(selected: NativeSession) -> Result<()> {
    if selected == NativeSession::Kde {
        return inspect();
    }
    guard()?;
    authentication_policy()?;
    ryoku_session()?;
    let uid = desktop_uid()?;
    require(
        existing_record()? == Some(record(uid, &plasma_session()?)?),
        "desktop fixture identity changed",
    )?;
    display_manager()?;
    let id = active_session()?;
    let values = show_session(&id)?;
    require(
        session_matches_desktop(&values, uid, "Hyprland"),
        "active ordinary-user Hyprland SDDM session required",
    )?;
    let scope = format!("session-{id}.scope");
    require(values.get("Scope") == Some(&scope), "unexpected native session scope")?;
    for candidate in registered_sessions(uid)? {
        require(
            candidate == id
                || show_session(&candidate)?.get("Class").is_some_and(|class| class == "manager"),
            "another desktop user login makes attribution ambiguous",
        )?;
    }
    let compositor = user_process("Hyprland", "hyprland", uid, &scope)?;
    let mut command = Command::new("/usr/sbin/runuser");
    command.args([
        "--user",
        USER,
        "--",
        "/usr/bin/env",
        "-i",
        &format!("XDG_RUNTIME_DIR=/run/user/{uid}"),
        &format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"),
        "PATH=/usr/bin:/bin",
        "/usr/bin/systemctl",
        "--user",
        "show",
        "ryoku-desktop-shell.service",
        "--property=ActiveState,SubState,MainPID,NoNewPrivileges",
    ]);
    let output = crate::process::run(&mut command, Duration::from_secs(10), 4096)?;
    require(
        !output.timed_out
            && output.status.success()
            && !output.stdout_truncated
            && !output.stderr_truncated,
        "cannot inspect native Ryoku shell unit",
    )?;
    let unit = properties(&String::from_utf8(output.stdout)?)?;
    require(
        matches(
            &unit,
            &[("ActiveState", "active"), ("SubState", "running"), ("NoNewPrivileges", "yes")],
        ),
        "native NNP Ryoku shell unit is not running",
    )?;
    let pid = unit.get("MainPID").ok_or("shell MainPID absent")?.parse::<u32>()?;
    require(pid > 1, "invalid shell PID")?;
    let writer = fs::canonicalize("/usr/libexec/ryoku/ryoku-writer")?;
    let daemon = fs::canonicalize("/usr/libexec/ryoku/upstream/ryoku-shell")?;
    let quickshell = fs::canonicalize("/usr/libexec/ryoku/quickshell/bin/qs")?;
    require(
        fs::read_link(format!("/proc/{pid}/exe"))? == writer
            && process_arguments(pid)? == [b"/usr/bin/ryoku-shell".to_vec(), b"daemon".to_vec()],
        "shell unit does not run the packaged writer alias",
    )?;
    rpm_intact(&writer, "ryoku-desktop-runtime")?;
    let daemon_processes = ryoku_child_processes(uid, &scope, pid, &daemon)?;
    require(daemon_processes.len() == 1, "writer must supervise exactly one pinned Ryoku daemon")?;
    let daemon_pid = daemon_processes[0];
    require(
        process_arguments(daemon_pid)?
            == [b"/usr/libexec/ryoku/upstream/ryoku-shell".to_vec(), b"daemon".to_vec()],
        "pinned Ryoku daemon arguments differ from the packaged unit",
    )?;
    rpm_intact(&daemon, "ryoku-desktop-runtime")?;
    let shell_processes = ryoku_child_processes(uid, &scope, daemon_pid, &quickshell)?;
    require(
        shell_processes.len() == 1,
        "upstream daemon must supervise exactly one Ryoku shell Quickshell process",
    )?;
    let shell_pid = shell_processes[0];
    require(
        ryoku_shell_arguments(&process_arguments(shell_pid)?),
        "Ryoku Quickshell process does not select the preserved shell",
    )?;
    rpm_intact(&quickshell, "ryoku-quickshell")?;
    for process in [pid, daemon_pid, shell_pid] {
        let status = fs::read_to_string(format!("/proc/{process}/status"))?;
        require(
            process_status(&status, uid) && process_has_no_new_privs(&status),
            "Ryoku shell process has unexpected identity or privileges",
        )?;
        require(
            process_cgroup(&fs::read_to_string(format!("/proc/{process}/cgroup"))?, uid, &scope),
            "Ryoku shell process is outside fixture user's manager",
        )?;
    }
    let socket = plasma_socket(&fs::read(format!("/proc/{shell_pid}/environ"))?, uid)?;
    let metadata = fs::symlink_metadata(&socket)?;
    require(
        metadata.file_type().is_socket() && metadata.uid() == uid,
        "Ryoku Wayland socket ownership mismatch",
    )?;
    let _connection = UnixStream::connect(socket)?;
    for process in [pid, daemon_pid, shell_pid, compositor] {
        require(
            process_status(&fs::read_to_string(format!("/proc/{process}/status"))?, uid),
            "Ryoku desktop process exited",
        )?;
    }
    require(
        active_session()? == id && session_matches_desktop(&show_session(&id)?, uid, "Hyprland"),
        "native Ryoku login changed during inspection",
    )?;
    guard()?;
    println!(
        "desktop_login=passed\ndesktop_user={USER}\ndesktop_session_service=sddm\ndesktop_session_type=wayland\ndesktop_session_desktop=Hyprland\ndesktop_compositor=hyprland\ndesktop_shell=preserved-ryoku-quickshell\ndesktop_wayland_socket=connected\ndesktop_authentication=password-required\ndesktop_visual_acceptance=separate-capture-required"
    );
    Ok(())
}

fn process_arguments(pid: u32) -> Result<Vec<Vec<u8>>> {
    let bytes = fs::read(format!("/proc/{pid}/cmdline"))?;
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn ryoku_shell_arguments(arguments: &[Vec<u8>]) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair == [b"-p".to_vec(), b"/usr/share/ryoku/shell/quickshell/shell".to_vec()])
}

fn process_parent(source: &str) -> Option<u32> {
    source.lines().find_map(|line| line.strip_prefix("PPid:")?.trim().parse::<u32>().ok())
}

fn process_has_no_new_privs(source: &str) -> bool {
    source
        .lines()
        .any(|line| line.strip_prefix("NoNewPrivs:").is_some_and(|value| value.trim() == "1"))
}

fn ryoku_child_processes(
    uid: u32,
    scope: &str,
    parent: u32,
    executable: &Path,
) -> Result<Vec<u32>> {
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
        let Ok(cgroup) = fs::read_to_string(path.join("cgroup")) else {
            continue;
        };
        if !process_cgroup(&cgroup, uid, scope) {
            continue;
        }
        let Ok(exe) = fs::read_link(path.join("exe")) else {
            continue;
        };
        if exe == executable && process_parent(&status) == Some(parent) {
            found.push(pid);
        }
    }
    Ok(found)
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

fn painted_frame_marker(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| line == b"gtk_proof_first_frame=painted")
}

fn ui_log(scenario: &str) -> Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(format!("/var/log/ryoku-lab-ui-{scenario}-private.log"))?;
    let metadata = file.metadata()?;
    require(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o7777 == 0o600,
        "invalid private GTK readiness log",
    )?;
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1).read_to_end(&mut bytes)?;
    require(bytes.len() <= 128 * 1024, "GTK readiness log exceeded bound")?;
    Ok(bytes)
}

fn rendered_result(bytes: &[u8], expected: &str) -> bool {
    let markers: Vec<_> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| {
            line.starts_with(b"gtk_proof_request=")
                || line.starts_with(b"gtk_proof_result=")
                || line.starts_with(b"gtk_proof_result_frame=")
        })
        .collect();
    markers
        == [
            b"gtk_proof_request=1".as_slice(),
            format!("gtk_proof_result=1:{expected}").as_bytes(),
            b"gtk_proof_result_frame=1".as_slice(),
        ]
}

pub(crate) fn result_evidence(scenario: &str) -> Result<String> {
    let (name, result) = match scenario {
        "service-success" => ("success", "recovery_unavailable"),
        "service-error" => ("error", "unavailable"),
        _ => return Err("unsupported real UI result scenario".into()),
    };
    Ok(format!(
        "ui_service_{name}_result={result}\nui_service_{name}_input=qmp_enter\nui_service_{name}_fresh_request=1\nui_service_{name}_result_frame=painted\n"
    ))
}

pub fn ui_result(scenario: &str) -> Result<()> {
    guard()?;
    let evidence = result_evidence(scenario)?;
    let expected =
        if scenario == "service-success" { "recovery_unavailable" } else { "unavailable" };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if rendered_result(&ui_log(scenario)?, expected) {
            print!("{evidence}");
            return Ok(());
        }
        require(
            Instant::now() < deadline,
            "fresh keyboard-triggered typed UI result was not painted",
        )?;
        thread::sleep(Duration::from_millis(100));
    }
}

/// Start the opt-in GTK proof inside the already authenticated Wayland user
/// session. Only a fixed success/error scenario is accepted.
pub fn launch_ui(scenario: &str) -> Result<()> {
    guard()?;
    inspect()?;
    require(
        matches!(scenario, "success" | "error" | "service-success" | "service-error"),
        "unsupported GTK proof scenario",
    )?;
    let uid = desktop_uid()?;
    require(ui_processes(uid)?.is_empty(), "another GTK proof window is still active")?;
    if scenario == "service-success" {
        start_ui_service(uid)?;
    } else if scenario == "service-error" {
        stop_ui_service(uid)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while service_unit_loaded()? {
            require(
                Instant::now() < deadline,
                "UI daemon unit still exists before unavailable proof",
            )?;
            thread::sleep(Duration::from_millis(100));
        }
    }
    let (splash_session, splash_scope) = wait_for_plasma_splash_exit(uid)?;
    let environment = plasma_environment(uid)?;
    let mut command = Command::new("/usr/sbin/runuser");
    command.args(["--user", USER, "--", "/usr/bin/env", "-i"]);
    for (name, value) in environment {
        command.arg(format!("{name}={value}"));
    }
    command.arg("/usr/local/bin/ryoku-ui-gtk");
    if scenario.starts_with("service-") {
        command.args(["--connect-service", "--capture-input"]);
    } else {
        command.args(["--demo", "--capture-on-launch"]);
    }
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
    require(
        active_session()? == splash_session && !plasma_splash_active(uid, &splash_scope)?,
        "Plasma splash or authenticated session changed immediately before GTK launch",
    )?;
    println!("ui_capture_splash=absent");
    command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    let _child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let bytes = ui_log(scenario)?;
        if !ui_processes(uid)?.is_empty() && painted_frame_marker(&bytes) {
            require(
                !scenario.starts_with("service-")
                    || !bytes
                        .windows(b"gtk_proof_request=".len())
                        .any(|window| window == b"gtk_proof_request="),
                "real service request occurred before keyboard input",
            )?;
            println!("gtk_proof_window=painted\ngtk_proof_scenario={scenario}");
            return Ok(());
        }
        require(Instant::now() < deadline, "GTK proof first mapped frame was not painted")?;
        thread::sleep(Duration::from_millis(200));
    }
}

/// Retrieve only the fixed proof logs, bounded independently of GTK output.
/// The host retains these in its private run directory before discarding disks.
pub fn ui_diagnostics() -> Result<()> {
    guard()?;
    let mut output = std::io::stdout().lock();
    for scenario in ["success", "error", "service-success", "service-error", "service-startup"] {
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
    let stopped = (|| -> Result<()> {
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
    })();
    let service_stopped = stop_ui_service(uid);
    stopped?;
    service_stopped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_user_unit_journal_matches_desktop_uid_and_manager_messages() {
        let command = service_journal_command(1234);
        assert_eq!(command.get_program(), "/usr/bin/journalctl");
        let args: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(
            &args[5..],
            [
                "_UID=1234",
                "_SYSTEMD_USER_UNIT=ryoku-lab-ui-service.service",
                "+",
                "_UID=1234",
                "USER_UNIT=ryoku-lab-ui-service.service",
            ]
        );
        assert!(args.contains(&"--boot=0"));
        assert!(!args.iter().any(|arg| arg.starts_with("--user-unit")));
    }

    #[test]
    fn failed_diagnostic_does_not_prevent_the_next_private_record() {
        let mut log = Vec::new();
        diagnostic_output(
            &mut log,
            "unavailable status",
            Command::new("/ryoku-lab-diagnostic-test-does-not-exist"),
        )
        .unwrap();
        let mut command = Command::new("/usr/bin/printf");
        command.arg("retained daemon failure");
        diagnostic_output(&mut log, "journal", command).unwrap();
        let text = String::from_utf8(log).unwrap();
        assert!(text.contains("diagnostic_command=unavailable"));
        assert!(text.contains("retained daemon failure"));
    }

    #[test]
    fn real_results_require_one_fresh_request_in_order_after_keyboard_input() {
        let ready = b"gtk_proof_first_frame=painted\ngtk_proof_request=1\ngtk_proof_result=1:recovery_unavailable\ngtk_proof_result_frame=1\n";
        assert!(rendered_result(ready, "recovery_unavailable"));
        assert!(!rendered_result(ready, "unavailable"));
        for invalid in [
            "gtk_proof_result=1:recovery_unavailable\ngtk_proof_result_frame=1\n",
            "gtk_proof_request=1\ngtk_proof_result=1:recovery_unavailable\n",
            "gtk_proof_request=1\ngtk_proof_result_frame=1\ngtk_proof_result=1:recovery_unavailable\n",
            "gtk_proof_request=2\ngtk_proof_result=2:recovery_unavailable\ngtk_proof_result_frame=2\n",
            "gtk_proof_request=1\ngtk_proof_result=1:recovery_unavailable\ngtk_proof_result_frame=1\ngtk_proof_request=2\n",
        ] {
            assert!(!rendered_result(invalid.as_bytes(), "recovery_unavailable"));
        }
        for scenario in ["service-success", "service-error"] {
            let evidence = result_evidence(scenario).unwrap();
            assert!(evidence.lines().all(crate::support::public_evidence));
        }
        assert!(result_evidence("success").is_err());
    }

    #[test]
    fn daemon_start_identity_handles_spaces_and_parentheses_in_comm() {
        let stat = format!("42 (name with ) parentheses) S {}1234 0", "0 ".repeat(18));
        assert_eq!(process_start(&stat).unwrap(), 1234);
        assert!(process_start("42 invalid").is_err());
    }

    #[test]
    fn process_and_renderer_diagnostics_do_not_substitute_for_a_painted_frame() {
        assert!(!painted_frame_marker(b"gtk_proof_window=running\nMESA-EGL: renderer warning\n"));
        assert!(!painted_frame_marker(b"prefix gtk_proof_first_frame=painted\n"));
        assert!(!painted_frame_marker(b"gtk_proof_first_frame=painted-extra\n"));
        assert!(painted_frame_marker(b"MESA-EGL: warning\ngtk_proof_first_frame=painted\n"));
    }

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
    fn retained_session_selection_cannot_substitute_a_command_or_the_other_desktop() {
        assert_eq!(NativeSession::parse("kde").unwrap(), NativeSession::Kde);
        assert_eq!(NativeSession::parse("ryoku").unwrap(), NativeSession::Ryoku);
        for value in ["", "Hyprland", "/usr/bin/Hyprland", "ryoku;reboot", "kde\n", "--session"] {
            assert!(NativeSession::parse(value).is_err());
        }
        let source = "Name=ryoku-desktop\nUser=1001\nService=sddm\nClass=user\nSeat=seat0\nType=wayland\nDesktop=Hyprland\nActive=yes\nState=active\nRemote=no\n";
        let values = properties(source).unwrap();
        assert!(session_matches_desktop(&values, 1001, "Hyprland"));
        assert!(!session_matches(&values, 1001));
        assert!(!session_matches_desktop(
            &properties(&source.replace("Desktop=Hyprland", "Desktop=KDE")).unwrap(),
            1001,
            "Hyprland"
        ));
        for (old, new) in [
            ("User=1001", "User=0"),
            ("Service=sddm", "Service=sshd"),
            ("Class=user", "Class=greeter"),
            ("Active=yes", "Active=no"),
            ("Remote=no", "Remote=yes"),
        ] {
            assert!(!session_matches_desktop(
                &properties(&source.replace(old, new)).unwrap(),
                1001,
                "Hyprland"
            ));
        }
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
    fn ryoku_shell_inspection_accepts_only_the_exact_payload_and_supervisor_relationship() {
        let shell = b"/usr/share/ryoku/shell/quickshell/shell".to_vec();
        assert!(ryoku_shell_arguments(&[b"qs".to_vec(), b"-p".to_vec(), shell]));
        assert!(!ryoku_shell_arguments(&[
            b"qs".to_vec(),
            b"-p".to_vec(),
            b"/home/tester/checkout/shell".to_vec(),
        ]));
        assert!(!ryoku_shell_arguments(&[b"qs".to_vec(), b"-c".to_vec(), b"shell".to_vec()]));
        assert_eq!(process_parent("Name: qs\nPPid:\t420\n"), Some(420));
        assert_eq!(process_parent("Name: qs\nPPid: invalid\n"), None);
        assert!(process_has_no_new_privs("Uid:\t1001\nNoNewPrivs:\t1\n"));
        assert!(!process_has_no_new_privs("Uid:\t1001\nNoNewPrivs:\t0\n"));
        assert!(!process_has_no_new_privs("Uid:\t1001\n"));
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
