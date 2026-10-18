//! Validation only: actual systemd/logind and two NSS users in seeded QEMU.
//! No host setup, package installation, managed Start or provider request.
#[path = "native_service_fixture/acceptance.rs"]
mod acceptance;
#[path = "native_service_fixture/account_probe.rs"]
mod account_probe;
#[path = "native_service_fixture/client.rs"]
mod client;
#[path = "native_service_fixture/support.rs"]
mod support;

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use support::{Result, checked, command, require};

const SELF: &str = "/usr/local/sbin/ryoku-native-service-fixture";
const INPUT: &str = "/run/ryoku-service-fixture-input";
const RECEIPT: &str = "/run/ryoku-service-fixture";
const SERVICE: &str = "/usr/libexec/ryoku/ryoku-service";
const HELPER: &str = "/usr/libexec/ryoku/ryoku-helper";
const USER_UNIT: &str = "/usr/lib/systemd/user/ryoku-service.service";
const HELPER_UNIT: &str = "/usr/lib/systemd/system/ryoku-helper.service";
const SOCKET_UNIT: &str = "/usr/lib/systemd/system/ryoku-helper.socket";
const SYSUSERS: &str = "/usr/lib/sysusers.d/ryoku.conf";
const PAYLOADS: [(&str, &[u8]); 4] = [
    (USER_UNIT, include_bytes!("../../../packaging/rpm/systemd/user/ryoku-service.service")),
    (HELPER_UNIT, include_bytes!("../../../packaging/rpm/systemd/ryoku-helper.service")),
    (SOCKET_UNIT, include_bytes!("../../../packaging/rpm/systemd/ryoku-helper.socket")),
    (SYSUSERS, include_bytes!("../../../packaging/rpm/sysusers.d/ryoku.conf")),
];

#[derive(Clone, Copy)]
struct Account {
    name: &'static str,
    uid: u32,
}
const OPERATOR: Account = Account { name: "ryoku-lab-operator", uid: 23171 };
const OBSERVER: Account = Account { name: "ryoku-lab-observer", uid: 23172 };
impl Account {
    fn home(self) -> String {
        format!("/home/{}", self.name)
    }
    fn runtime(self) -> String {
        format!("/run/user/{}", self.uid)
    }
    fn journal(self) -> String {
        format!("{}/.local/state/ryoku/service/operations.log", self.home())
    }
}

fn main() -> Result<()> {
    support::guest_guard()?;
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["--check"] => {
            require_root()?;
            preflight()
        }
        ["--run"] => {
            require_root()?;
            preflight()?;
            let result = run();
            if result.is_err() {
                failure_diagnostics();
            }
            result
        }
        ["--client", case] => client::run(case),
        ["--account-probe"] => account_probe::server(),
        _ => Err("usage: native_service_fixture --check|--run|--client FIXED_CASE".into()),
    }
}

fn failure_diagnostics() {
    // Failure preserves the fixture for VM disposal. Capture bounded native
    // unit logs now so diagnosis does not require another activation attempt.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut command = Command::new("/usr/bin/journalctl");
    command.args([
        "--no-pager",
        "--lines=80",
        "--unit=ryoku-helper.service",
        "--unit=user@23171.service",
        "--unit=user@23172.service",
    ]);
    if let Ok(log) = support::bounded_until(&mut command, deadline) {
        eprintln!(
            "native service fixture private unit diagnostics:\n{}\n{}",
            String::from_utf8_lossy(&log.stdout),
            String::from_utf8_lossy(&log.stderr)
        );
    }
}

fn require_root() -> Result<()> {
    require(
        rustix::process::geteuid().as_raw() == 0 && rustix::process::getegid().as_raw() == 0,
        "fixture setup requires root in disposable QEMU",
    )
}

fn absent(path: impl AsRef<Path>) -> Result<()> {
    require(
        matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "refuse existing fixture path",
    )
}

fn preflight() -> Result<()> {
    // Collect *all* missing tools/unit providers before touching guest state.
    support::inventory()?;
    require(
        fs::canonicalize(std::env::current_exe()?)? == Path::new(SELF),
        "fixture is not at fixed installed probe path",
    )?;
    support::private_file(Path::new(SELF), 0, 0o755)?;
    support::directory(Path::new(INPUT), 0, 0o700)?;
    for name in ["ryoku-service", "ryoku-helper"] {
        let input = Path::new(INPUT).join(name);
        support::private_file(&input, 0, 0o755)?;
        checked(
            "/lib64/ld-linux-x86-64.so.2",
            &["--list", input.to_str().ok_or("invalid fixed input path")?],
        )?;
    }
    for path in
        [RECEIPT, "/usr/libexec/ryoku", USER_UNIT, HELPER_UNIT, SOCKET_UNIT, SYSUSERS, "/run/ryoku"]
    {
        absent(path)?;
    }
    for (unit, scopes) in [
        ("ryoku-helper.service", ["/etc/systemd/system", "/run/systemd/system"]),
        ("ryoku-helper.socket", ["/etc/systemd/system", "/run/systemd/system"]),
        ("ryoku-service.service", ["/etc/systemd/user", "/run/systemd/user"]),
    ] {
        for scope in scopes {
            absent(format!("{scope}/{unit}"))?;
            absent(format!("{scope}/{unit}.d"))?;
        }
    }
    for account in [OPERATOR, OBSERVER] {
        for database in ["passwd", "group"] {
            for key in [account.name.to_string(), account.uid.to_string()] {
                require(
                    command("/usr/bin/getent", &[database, &key])?.status.code() == Some(2),
                    "refuse existing fixture NSS identity",
                )?;
            }
        }
        absent(account.home())?;
        absent(account.runtime())?;
        absent(format!("/var/lib/systemd/linger/{}", account.name))?;
    }
    require(
        command("/usr/bin/getent", &["group", "ryoku-operators"])?.status.code() == Some(2),
        "refuse existing operator group",
    )?;
    let package = command("/usr/bin/rpm", &["-q", "ryoku-helper"])?;
    require(
        package.status.code() == Some(1),
        "fixture requires helper RPM absent; no RPM install claim",
    )?;
    checked("/usr/bin/systemctl", &["is-active", "systemd-logind.service"])?;
    // This is independent live mount/record validation, before setup changes.
    let _state = ryoku_service::protected_state::ProtectedState::open()
        .map_err(|_| "initialized protected state unavailable before fixture")?;
    println!("native_service_fixture_preflight=passed");
    Ok(())
}

fn publish(path: &str, bytes: &[u8], mode: u32) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let parent = Path::new(path).parent().ok_or("payload parent unavailable")?;
    support::trusted_directory(parent)?;
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::File::open(parent)?.sync_all()?;
    support::private_file(Path::new(path), 0, mode)?;
    Ok(())
}

fn user_command(account: Account, args: &[&str]) -> Result<String> {
    user_command_until(account, args, Instant::now() + Duration::from_secs(30))
}
fn user_command_until(account: Account, args: &[&str], deadline: Instant) -> Result<String> {
    let mut cmd = Command::new("/usr/sbin/runuser");
    cmd.args([
        "--user",
        account.name,
        "--",
        "/usr/bin/systemctl",
        "--user",
        "--no-pager",
        "--no-ask-password",
    ])
    .args(args)
    .env("XDG_RUNTIME_DIR", account.runtime())
    .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/bus", account.runtime()));
    support::success_until(&mut cmd, deadline)
}

fn bus_ready(account: Account) -> Result<()> {
    // Native sockets.target starts the bus socket. Some providers deliberately
    // refuse manual starts; inspect readiness and query the actual user manager.
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if user_command_until(account, &["is-active", "dbus.socket"], deadline)
            .is_ok_and(|state| state.trim() == "active")
            && user_command_until(
                account,
                &["show", "default.target", "--property=ActiveState", "--value"],
                deadline,
            )
            .is_ok_and(|state| state.trim() == "active")
        {
            return Ok(());
        }
        std::thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    Err("native user bus readiness deadline expired".into())
}

fn service_pid(account: Account) -> Result<u32> {
    let pid =
        user_command(account, &["show", "ryoku-service.service", "--property=MainPID", "--value"])?
            .trim()
            .parse::<u32>()?;
    require(pid > 0, "native user unit has no live main PID")?;
    Ok(pid)
}

fn child(account: Option<Account>, case: &str) -> Result<()> {
    child_until(account, case, Instant::now() + Duration::from_secs(30))
}
fn child_until(account: Option<Account>, case: &str, deadline: Instant) -> Result<()> {
    let mut cmd = if let Some(account) = account {
        let mut cmd = Command::new("/usr/sbin/runuser");
        cmd.args(["--user", account.name, "--", SELF]);
        cmd
    } else {
        Command::new(SELF)
    };
    cmd.args(["--client", case]);
    let output = support::success_until(&mut cmd, deadline)?;
    require(
        output.trim() == format!("native_service_case_{case}=passed"),
        "client fixture case did not pass",
    )
}

fn ready(account: Account) -> Result<()> {
    // Type=simple does not wait for bind/replay. All read-only attempts share
    // one deadline, including child execution and native account lookup waits.
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if child_until(Some(account), "own", deadline).is_ok() {
            return Ok(());
        }
        std::thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    Err("native user unit read-only readiness deadline expired".into())
}

fn identity(account: Account) -> Result<()> {
    let passwd = checked("/usr/bin/getent", &["passwd", account.name])?;
    let fields: Vec<_> = passwd.trim_end().split(':').collect();
    require(
        fields.len() == 7
            && fields[0] == account.name
            && fields[2] == account.uid.to_string()
            && fields[3] == account.uid.to_string()
            && fields[5] == account.home(),
        "fixture NSS mapping changed",
    )
}

fn run() -> Result<()> {
    fs::create_dir(RECEIPT)?;
    fs::set_permissions(RECEIPT, fs::Permissions::from_mode(0o700))?;
    for (path, bytes) in PAYLOADS {
        publish(path, bytes, 0o644)?;
    }
    fs::create_dir("/usr/libexec/ryoku")?;
    fs::set_permissions("/usr/libexec/ryoku", fs::Permissions::from_mode(0o755))?;
    for (name, destination) in [("ryoku-service", SERVICE), ("ryoku-helper", HELPER)] {
        let bytes = support::bounded_file(&Path::new(INPUT).join(name), 128 * 1024 * 1024)?;
        publish(destination, &bytes, 0o755)?;
    }
    // Exact payload files are restored to installed policy labels; SELinux stays enforcing.
    for path in [SERVICE, HELPER, USER_UNIT, HELPER_UNIT, SOCKET_UNIT, SYSUSERS] {
        checked(
            support::tool("restorecon")?
                .invocation
                .to_str()
                .ok_or("invalid restorecon invocation")?,
            &["-F", path],
        )?;
        checked(
            support::tool("matchpathcon")?
                .invocation
                .to_str()
                .ok_or("invalid matchpathcon invocation")?,
            &["-V", path],
        )?;
    }
    checked("/usr/bin/systemd-sysusers", &[SYSUSERS])?;
    let operator_gid =
        ryoku_service::operator_group_gid().map_err(|_| "operator group unavailable")?;
    for account in [OPERATOR, OBSERVER] {
        let number = account.uid.to_string();
        checked("/usr/sbin/groupadd", &["--gid", &number, account.name])?;
        checked(
            "/usr/sbin/useradd",
            &[
                "--uid",
                &number,
                "--gid",
                &number,
                "--create-home",
                "--home-dir",
                &account.home(),
                "--shell",
                "/bin/bash",
                "--password",
                "!",
                account.name,
            ],
        )?;
        fs::set_permissions(account.home(), fs::Permissions::from_mode(0o700))?;
        identity(account)?;
    }
    checked("/usr/sbin/usermod", &["--append", "--groups", "ryoku-operators", OPERATOR.name])?;
    require(
        checked("/usr/bin/getent", &["group", "ryoku-operators"])?.trim().split(':').next_back()
            == Some(OPERATOR.name),
        "operator group membership differs from fixture",
    )?;
    checked("/usr/bin/systemctl", &["daemon-reload"])?;
    checked("/usr/bin/systemctl", &["start", "ryoku-helper.socket"])?;
    let helper_socket = support::Pinned::open(
        "/run/ryoku/helper.sock",
        0,
        operator_gid,
        0o660,
        support::Kind::Socket,
    )?;
    for account in [OPERATOR, OBSERVER] {
        checked("/usr/bin/loginctl", &["enable-linger", account.name])?;
        checked("/usr/bin/systemctl", &["start", &format!("user@{}.service", account.uid)])?;
        checked(
            "/usr/bin/systemctl",
            &["is-active", &format!("user-runtime-dir@{}.service", account.uid)],
        )?;
        let runtime = checked(
            "/usr/bin/loginctl",
            &["show-user", account.name, "--property=RuntimePath", "--value"],
        )?;
        require(runtime.trim() == account.runtime(), "logind runtime path mismatch")?;
        require(
            checked(
                "/usr/bin/loginctl",
                &["show-user", account.name, "--property=Linger", "--value"],
            )?
            .trim()
                == "yes",
            "native linger state missing",
        )?;
        support::directory(Path::new(&account.runtime()), account.uid, 0o700)?;
        bus_ready(account)?;
        user_command(account, &["daemon-reload"])?;
        user_command(account, &["start", "ryoku-service.service"])?;
        user_command(account, &["is-active", "ryoku-service.service"])?;
        require(
            user_command(
                account,
                &["show", "ryoku-service.service", "--property=FragmentPath", "--value"],
            )?
            .trim()
                == USER_UNIT,
            "user manager did not load exact installed fixture unit",
        )?;
        ready(account)?;
    }
    println!(
        "native_service_logind_user_managers=passed\nnative_service_user_unit_activation=passed"
    );
    let operator_journal = journal(OPERATOR)?;
    let observer_journal = journal(OBSERVER)?;
    child(Some(OBSERVER), "cross-dac")?;
    child(Some(OBSERVER), "helper-dac")?;
    child(Some(OPERATOR), "helper-operator")?;
    child(Some(OPERATOR), "helper-malformed")?;
    child(None, "user-root")?;
    child(None, "helper-root")?;
    checked("/usr/bin/systemctl", &["is-active", "ryoku-helper.service"])?;
    require(
        checked(
            "/usr/bin/systemctl",
            &["show", "ryoku-helper.service", "--property=FragmentPath", "--value"],
        )?
        .trim()
            == HELPER_UNIT,
        "system manager did not load exact installed helper unit",
    )?;
    let helper_pid = checked(
        "/usr/bin/systemctl",
        &["show", "ryoku-helper.service", "--property=MainPID", "--value"],
    )?;
    require(helper_pid.trim().parse::<u32>()? > 0, "helper activation produced no live process")?;
    println!(
        "native_service_helper_socket_activation=passed\nnative_service_native_dac_denials=passed\nnative_service_root_policy=passed\nnative_service_operator_policy=passed"
    );

    let runtime = support::Pinned::open(
        &OPERATOR.runtime(),
        OPERATOR.uid,
        OPERATOR.uid,
        0o700,
        support::Kind::Directory,
    )?;
    let socket_path = format!("{}/ryoku-service.sock", OPERATOR.runtime());
    let socket = support::Pinned::open(
        &socket_path,
        OPERATOR.uid,
        OPERATOR.uid,
        0o600,
        support::Kind::Socket,
    )?;
    support::with_modes(&[(&runtime, 0o711), (&socket, 0o666)], || {
        child(Some(OBSERVER), "cross-policy")
    })?;
    support::with_modes(&[(&helper_socket, 0o666)], || child(Some(OBSERVER), "helper-policy"))?;
    runtime.revalidate()?;
    socket.revalidate()?;
    helper_socket.revalidate()?;
    child(Some(OPERATOR), "own")?;
    child(Some(OPERATOR), "helper-operator")?;
    println!(
        "native_service_assisted_kernel_policy=passed\nnative_service_dac_modes_restored=passed"
    );

    for account in [OPERATOR, OBSERVER] {
        let before = service_pid(account)?;
        let mut duplicate = Command::new("/usr/sbin/runuser");
        duplicate.args(["--user", account.name, "--", SERVICE]);
        let result = support::bounded(&mut duplicate)?;
        require(
            !result.status.success()
                && result.stderr == b"ryoku-service: daemon lock unavailable\n",
            "duplicate daemon was not refused by lifetime lock",
        )?;
        require(service_pid(account)? == before, "duplicate changed original service PID")?;
        ready(account)?;
        let lock = support::Pinned::open(
            &format!("{}/ryoku-service.lock", account.runtime()),
            account.uid,
            account.uid,
            0o600,
            support::Kind::Regular,
        )?;
        let old_socket = support::Pinned::open(
            &format!("{}/ryoku-service.sock", account.runtime()),
            account.uid,
            account.uid,
            0o600,
            support::Kind::Socket,
        )?;
        user_command(account, &["restart", "ryoku-service.service"])?;
        ready(account)?;
        require(service_pid(account)? != before, "restart did not create a fresh service process")?;
        lock.revalidate()?;
        require(old_socket.revalidate().is_err(), "restart reused old socket inode")?;
    }
    operator_journal.revalidate()?;
    observer_journal.revalidate()?;
    require(
        fs::metadata(OPERATOR.journal())?.len() == 0
            && fs::metadata(OBSERVER.journal())?.len() == 0,
        "read-only fixture changed operation journal",
    )?;
    for account in [OPERATOR, OBSERVER] {
        identity(account)?;
    }
    println!(
        "native_service_duplicate_start=passed\nnative_service_fresh_process_restart=passed\nnative_service_empty_journals_retained=passed"
    );
    acceptance::installed_checks(helper_pid.trim().parse()?)?;
    account_probe::run()?;
    for account in [OPERATOR, OBSERVER] {
        user_command(account, &["stop", "ryoku-service.service"])?;
        checked("/usr/bin/loginctl", &["terminate-user", account.name])?;
        checked("/usr/bin/loginctl", &["disable-linger", account.name])?;
    }
    checked("/usr/bin/systemctl", &["stop", "ryoku-helper.socket", "ryoku-helper.service"])?;
    println!(
        "native_service_task_units_stopped=passed\nnative_service_artifacts=retained_for_disposable_guest_teardown\nnative_service_rpm_install=not_tested\nnative_service_interactive_login=not_tested\nnative_service_accepted_transactions=not_tested"
    );
    Ok(())
}

fn journal(account: Account) -> Result<support::Pinned> {
    let file = support::Pinned::open(
        &account.journal(),
        account.uid,
        account.uid,
        0o600,
        support::Kind::Regular,
    )?;
    require(fs::metadata(account.journal())?.len() == 0, "fixture journal is not empty")?;
    Ok(file)
}
