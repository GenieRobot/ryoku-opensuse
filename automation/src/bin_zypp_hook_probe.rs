//! Disposable-guest characterization tool for libzypp commit plugin semantics.
//! This does not provide or claim a production write gate.
#[path = "lab/zypp_plugin.rs"]
mod protocol;

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        if let Err(error) = protocol::serve() {
            eprintln!("probe_error={error}");
            std::process::exit(1);
        }
        return;
    }
    if args.as_slice() == ["--inventory"] {
        if let Err(error) = inventory() {
            eprintln!("inventory_error={error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = experiment() {
        eprintln!("experiment_error={error}");
        std::process::exit(1);
    }
}

/// Read-only provider/plugin/config inventory for the seeded disposable VM.
/// Full path metadata and allowlisted xattr values are emitted only to the
/// runner's mode-0600 private log; the host publishes selected safe facts.
fn inventory() -> Result<(), Box<dyn std::error::Error>> {
    use std::{collections::BTreeSet, os::unix::fs::MetadataExt};

    fixture_guard()?;
    let packages = Command::new("rpm")
        .args(["-qa", "--qf", "%{NAME} %{EPOCHNUM} %{VERSION} %{RELEASE} %{ARCH}\\n"])
        .output()?;
    if !packages.status.success() {
        return Err("rpm -qa provider inventory failed".into());
    }
    let package_rows = String::from_utf8(packages.stdout)?
        .lines()
        .filter(|row| {
            let name = row.split_whitespace().next().unwrap_or("");
            ["zypper", "libzypp", "PackageKit", "cockpit", "yast", "myrlyn"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut paths = BTreeSet::new();
    for row in &package_rows {
        let Some(package) = row.split_whitespace().next() else { continue };
        let config = Command::new("rpm").args(["-qc", package]).output()?;
        if config.status.success() {
            paths.extend(String::from_utf8(config.stdout)?.lines().map(PathBuf::from));
        }
    }
    let plugin_dirs = discover_plugin_dirs()?;
    for dir in &plugin_dirs {
        paths.insert(dir.clone());
        if let Ok(children) = fs::read_dir(dir) {
            paths.extend(children.filter_map(Result::ok).map(|entry| entry.path()));
        }
    }

    for row in &package_rows {
        let Some(package) = row.split_whitespace().next() else {
            continue;
        };
        let listed = Command::new("rpm").args(["-ql", package]).output()?;
        if listed.status.success() {
            for path in String::from_utf8(listed.stdout)?.lines() {
                if paths.contains(Path::new(path)) {
                    println!("inventory_rpm_ql={path}|{package}");
                }
            }
        }
    }

    println!("inventory_packages={}", package_rows.len());
    println!("inventory_plugin_directories={}", plugin_dirs.len());
    for row in package_rows {
        println!("inventory_provider={}", row.replace(' ', "|"));
    }
    print_cockpit_runtime_state();
    for path in paths {
        let display = path.to_string_lossy();
        let ownership = Command::new("rpm")
            .args([
                "-qf",
                "--qf",
                "%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}",
                display.as_ref(),
            ])
            .output()?;
        let owner = if ownership.status.success() {
            String::from_utf8_lossy(&ownership.stdout).trim().replace(' ', "@")
        } else {
            "UNOWNED".to_owned()
        };
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            println!("inventory_path={display}|missing|owner={owner}");
            continue;
        };
        let kind = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "regular-file"
        } else {
            "other"
        };
        let target = if metadata.file_type().is_symlink() {
            fs::read_link(&path)
                .ok()
                .map(|target| target.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let acl = command_digest("getfacl", &["--omit-header", "--numeric", display.as_ref()]);
        let selinux = command_digest(
            "getfattr",
            &["--only-values", "-n", "security.selinux", display.as_ref()],
        );
        let capability = command_digest(
            "getfattr",
            &["--only-values", "-n", "security.capability", display.as_ref()],
        );
        println!(
            "inventory_path={display}|{kind}|mode={:o}|uid={}|gid={}|mtime_ns={}|target={target}|owner={owner}|acl={acl}|xattr.security.selinux={selinux}|xattr.security.capability={capability}",
            metadata.mode() & 0o7777,
            metadata.uid(),
            metadata.gid(),
            metadata.mtime() as i128 * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
        );
    }
    Ok(())
}

/// Record bounded, privacy-preserving Cockpit runtime facts for the provider
/// inventory. This is diagnostic only: an inactive socket does not prove that
/// Cockpit cannot be started, and a loopback listener does not mediate writes.
fn print_cockpit_runtime_state() {
    println!("inventory_cockpit_socket_enabled={}", systemctl_state("is-enabled"));
    println!("inventory_cockpit_socket_active={}", systemctl_state("is-active"));
    let output = Command::new("systemctl")
        .args(["show", "--property=Listen", "--value", "cockpit.socket"])
        .env("LC_ALL", "C")
        .output();
    let cockpit_scope = match output {
        Ok(result) if result.status.success() => {
            classify_listener_endpoints(&String::from_utf8_lossy(&result.stdout))
        }
        _ => "unavailable",
    };
    println!("inventory_cockpit_socket_listen_scope={cockpit_scope}");

    // Keep the host-wide observation distinct: it cannot be attributed to
    // Cockpit and is useful only as a broad environment diagnostic.
    let output = Command::new("ss").args(["-H", "-ltn"]).env("LC_ALL", "C").output();
    let listener_scope = match output {
        Ok(result) if result.status.success() => {
            classify_tcp_listeners(&String::from_utf8_lossy(&result.stdout))
        }
        _ => "unavailable",
    };
    println!("inventory_host_tcp_listener_scope={listener_scope}");
}

fn systemctl_state(verb: &str) -> &'static str {
    let output =
        Command::new("systemctl").args([verb, "cockpit.socket"]).env("LC_ALL", "C").output();
    let Ok(output) = output else { return "unavailable" };
    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    match (verb, output.status.success(), state.as_str()) {
        ("is-enabled", true, "enabled") => "enabled",
        ("is-enabled", true, "static") => "static",
        ("is-enabled", true, "indirect") => "indirect",
        ("is-enabled", true, "generated") => "generated",
        ("is-enabled", false, "disabled") => "disabled",
        ("is-enabled", false, "masked") => "masked",
        ("is-enabled", false, "not-found") => "not-found",
        ("is-active", true, "active") => "active",
        ("is-active", false, "inactive") => "inactive",
        ("is-active", false, "failed") => "failed",
        ("is-active", false, "unknown") => "unknown",
        _ => "unavailable",
    }
}

fn classify_tcp_listeners(output: &str) -> &'static str {
    let endpoints = output
        .lines()
        .map(|line| line.split_whitespace().nth(3).ok_or(()))
        .collect::<Result<Vec<_>, _>>();
    let Ok(endpoints) = endpoints else { return "unavailable" };
    classify_endpoint_iter(endpoints)
}

fn classify_listener_endpoints(output: &str) -> &'static str {
    let endpoints = output
        .lines()
        .map(|line| line.rsplit_once('=').map_or(line.trim(), |(_, endpoint)| endpoint.trim()))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    classify_endpoint_iter(endpoints)
}

fn classify_endpoint_iter<'a>(endpoints: impl IntoIterator<Item = &'a str>) -> &'static str {
    let mut found = false;
    for endpoint in endpoints {
        found = true;
        let Some(host) = listener_host(endpoint) else { return "unavailable" };
        if !is_loopback_host(host) {
            return "non-loopback-present";
        }
    }
    if found { "loopback-only" } else { "none" }
}

fn listener_host(endpoint: &str) -> Option<&str> {
    if let Some(bracketed) = endpoint.strip_prefix('[') {
        let (host, suffix) = bracketed.split_once(']')?;
        return suffix.starts_with(':').then_some(host);
    }
    endpoint
        .rsplit_once(':')
        .map(|(host, port)| if port.is_empty() { "" } else { host })
        .filter(|host| !host.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host.parse::<std::net::IpAddr>().is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod cockpit_runtime_inventory_tests {
    use super::*;

    #[test]
    fn classifies_socket_activation_listener_scope_without_emitting_addresses() {
        assert_eq!(
            classify_listener_endpoints("Stream=127.0.0.1:9090\nStream=[::1]:9090\n"),
            "loopback-only"
        );
        assert_eq!(classify_listener_endpoints("Stream=0.0.0.0:9090\n"), "non-loopback-present");
        assert_eq!(classify_listener_endpoints(""), "none");
        assert_eq!(classify_listener_endpoints("Stream=not-an-endpoint"), "unavailable");
    }

    #[test]
    fn classifies_host_tcp_listener_rows_but_keeps_them_separate_from_cockpit() {
        assert_eq!(
            classify_tcp_listeners(
                "LISTEN 0 128 127.0.0.1:22 0.0.0.0:*\nLISTEN 0 128 [::1]:22 [::]:*\n"
            ),
            "loopback-only"
        );
        assert_eq!(
            classify_tcp_listeners("LISTEN 0 128 [::]:9090 [::]:*\n"),
            "non-loopback-present"
        );
        assert_eq!(classify_tcp_listeners(""), "none");
        assert_eq!(classify_tcp_listeners("malformed"), "unavailable");
    }
}

fn fixture_guard() -> Result<(), Box<dyn std::error::Error>> {
    if current_uid() != 0 {
        return Err("must run as root inside the explicitly seeded disposable QEMU fixture".into());
    }
    if fs::read_to_string("/etc/ryoku-lab-fixture")? != "disposable-issue13\n"
        || fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() != "QEMU"
    {
        return Err("refusing to run outside the explicitly seeded disposable QEMU fixture".into());
    }
    Ok(())
}

fn command_digest(program: &str, args: &[&str]) -> String {
    match Command::new(program).args(args).env("LC_ALL", "C").output() {
        Ok(result) if result.status.success() => {
            format!("present:{}", sha256_bytes(&result.stdout))
        }
        Ok(result)
            if String::from_utf8_lossy(&result.stderr)
                .to_ascii_lowercase()
                .contains("no such attribute")
                || String::from_utf8_lossy(&result.stderr)
                    .to_ascii_lowercase()
                    .contains("attribute not found") =>
        {
            "absent".to_owned()
        }
        _ => "unavailable".to_owned(),
    }
}

fn experiment() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let package = args
        .next()
        .ok_or("usage: zypp-hook-probe PACKAGE [deny-commit|malformed-commit|timeout-commit|missing-plugin]")?;
    let mode = args.next().unwrap_or_else(|| "deny-commit".into());
    if args.next().is_some()
        || package.is_empty()
        || !package.as_bytes()[0].is_ascii_alphanumeric()
        || !package.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
        || !["deny-commit", "malformed-commit", "timeout-commit", "missing-plugin"]
            .contains(&mode.as_str())
    {
        return Err("invalid package or probe mode".into());
    }
    if current_uid() != 0 {
        return Err("must run as root inside the explicitly seeded disposable QEMU fixture".into());
    }
    if fs::read_to_string("/etc/ryoku-lab-fixture")? != "disposable-issue13\n"
        || fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() != "QEMU"
    {
        return Err("refusing to run outside the explicitly seeded disposable QEMU fixture".into());
    }

    let dirs = discover_plugin_dirs()?;
    if dirs.len() != 1 {
        return Err(format!(
            "expected exactly one existing RPM-owned libzypp commit plugin directory, found {}",
            dirs.len()
        )
        .into());
    }
    let dir = &dirs[0];
    let plugin = dir.join("99-ryoku-disposable-probe");
    if plugin.exists() {
        return Err("probe plugin path already exists; refusing to overwrite it".into());
    }
    let timeout_check = Command::new("timeout").arg("--version").output()?;
    if !timeout_check.status.success()
        || !String::from_utf8_lossy(&timeout_check.stdout).contains("GNU coreutils")
    {
        return Err("GNU coreutils timeout is required to bound guest transactions".into());
    }
    let before = rpm_state()?;
    let preinstalled = rpm_query(&["-q", "--qf", "%{NAME}\n", package.as_str()]).is_some();
    if preinstalled {
        return Err("selected package is already installed; choose an absent harmless package from the frozen fixture repository".into());
    }
    let versions = provider_versions()?;
    let frame_log =
        PathBuf::from(format!("/var/tmp/ryoku-zypp-probe-{}.frames", std::process::id()));
    let mut log_options = fs::OpenOptions::new();
    log_options.write(true).create_new(true);
    let _frame_cleanup = Cleanup(frame_log.clone());
    let log_file = log_options.open(&frame_log)?;
    fs::set_permissions(&frame_log, fs::Permissions::from_mode(0o600))?;
    drop(log_file);
    let install_plugin = mode != "missing-plugin";
    if install_plugin {
        let executable = env::current_exe()?;
        fs::copy(&executable, &plugin)?;
        fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755))?;
    }
    let _cleanup = install_plugin.then(|| Cleanup(plugin.clone()));

    println!("guest_vendor=QEMU");
    println!("plugin_directory={}", dir.display());
    println!("plugin_registered={install_plugin}");
    if install_plugin {
        println!("plugin_path={}", plugin.display());
    }
    println!("package_requested={package}");
    println!("probe_mode={mode}");
    println!("rpm_state_before_sha256={}", sha256(&before));
    println!("rpm_state_before_entries={}", before.lines().count());
    for version in versions {
        println!("provider={version}");
    }
    println!(
        "provider_command=timeout --signal=TERM --kill-after=5s 45s zypper --non-interactive --no-refresh install {package}"
    );

    const DEADLINE_SECONDS: &str = "45s";
    let started = std::time::Instant::now();
    let mut command = Command::new("timeout");
    command
        .args([
            "--signal=TERM",
            "--kill-after=5s",
            DEADLINE_SECONDS,
            "zypper",
            "--non-interactive",
            "--no-refresh",
            "install",
            package.as_str(),
        ])
        .env(
            "RYOKU_ZYPP_PROBE_MODE",
            if mode == "missing-plugin" { "deny-commit" } else { mode.as_str() },
        )
        .env("RYOKU_ZYPP_PROBE_LOG", &frame_log);
    if !install_plugin {
        command.env_remove("RYOKU_ZYPP_PROBE_LOG");
    }
    let result = command.output()?;
    let elapsed = started.elapsed();
    // Keep provider diagnostics in the runner's mode-0600 private log. They
    // explain resolution failures that happen before libzypp invokes the
    // plugin, but may contain repository details and must not enter public
    // evidence. The host publishes only the parsed outcome fields below.
    print_private_provider_output("stdout", &result.stdout);
    print_private_provider_output("stderr", &result.stderr);
    let after = rpm_state()?;
    println!(
        "zypper_exit_code={}",
        result.status.code().map_or("signal".into(), |c| c.to_string())
    );
    println!("zypper_success={}", result.status.success());
    println!("transaction_deadline_seconds=45");
    println!("timeout_kill_grace_seconds=5");
    println!("transaction_elapsed_ms={}", elapsed.as_millis());
    println!(
        "transaction_timed_out={}",
        result.status.code() == Some(124) || result.status.code() == Some(137)
    );
    let frames = fs::read_to_string(&frame_log)?;
    let plugin_pids = frames
        .lines()
        .filter_map(|line| line.strip_prefix("probe_pid=")?.parse::<u32>().ok())
        .collect::<Vec<_>>();
    println!("plugin_invoked={}", !plugin_pids.is_empty());
    println!(
        "plugin_pids={}",
        plugin_pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
    );
    if mode == "timeout-commit" {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while plugin_pids.iter().any(|pid| process_active(*pid))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!(
            "timeout_plugin_terminated={}",
            plugin_pids.iter().all(|pid| !process_active(*pid))
        );
    }
    let commit_seen = frames.lines().any(|line| line == "probe_frame=COMMITBEGIN");
    println!("plugin_commitbegin_seen={commit_seen}");
    println!(
        "plugin_frames={}",
        frames
            .lines()
            .map(|line| line.strip_prefix("probe_frame=").unwrap_or("invalid"))
            .collect::<Vec<_>>()
            .join(",")
    );
    println!("rpm_state_after_sha256={}", sha256(&after));
    println!("rpm_state_after_entries={}", after.lines().count());
    let rpm_unchanged = before == after;
    let requested_package_installed = rpm_query(&["-q", package.as_str()]).is_some();
    println!("rpm_state_unchanged={rpm_unchanged}");
    println!("requested_package_installed={requested_package_installed}");
    drop(_cleanup);
    println!(
        "plugin_cleanup={}",
        if !install_plugin {
            "not-installed"
        } else if plugin.exists() {
            "failed"
        } else {
            "passed"
        }
    );
    if install_plugin && plugin.exists() {
        return Err("failed to remove probe plugin".into());
    }
    if mode == "timeout-commit"
        && result.status.code() != Some(124)
        && result.status.code() != Some(137)
    {
        return Err("timeout mode completed before the bounded deadline instead of exercising the timeout path".into());
    }
    if mode == "timeout-commit" && plugin_pids.iter().any(|pid| process_active(*pid)) {
        return Err(
            "timed out plugin process remains; discard and reset the disposable guest".into()
        );
    }
    if mode == "timeout-commit" && !rpm_unchanged {
        println!("outcome=unsafe_timeout_commit_detected");
        return Err(
            "RPM state changed during timed-out plugin; discard and reset the overlay".into()
        );
    }
    if mode == "timeout-commit" && rpm_unchanged {
        println!("outcome=timeout_aborted_transaction_rpm_state_unchanged");
    }
    if mode == "missing-plugin" && !rpm_unchanged {
        println!("outcome=missing_plugin_allowed_rpm_change");
        return Err("transaction proceeded without this plugin and changed RPM state; discard and reset the overlay".into());
    }
    if mode == "missing-plugin"
        && rpm_unchanged
        && result.status.success()
        && !requested_package_installed
    {
        println!("outcome=missing_plugin_transaction_inconclusive");
        return Err("zypper returned success but the requested package is absent and RPM state is unchanged".into());
    }
    if mode == "missing-plugin" && rpm_unchanged && !result.status.success() {
        println!("outcome=missing_plugin_transaction_failed_elsewhere_or_was_blocked");
        return Err("missing-plugin run did not establish a successful package transaction; investigate zypper result and cached repository".into());
    }
    if !before.eq(&after) {
        return Err(
            "RPM state changed during the probe; disposable guest now requires reset before reuse"
                .into(),
        );
    }
    Ok(())
}

fn print_private_provider_output(stream: &str, bytes: &[u8]) {
    const MAX_BYTES: usize = 16 * 1024;
    let end = bytes.len().min(MAX_BYTES);
    let text = String::from_utf8_lossy(&bytes[..end]);
    println!("zypper_{stream}_private={text:?}");
    if end < bytes.len() {
        println!("zypper_{stream}_private_truncated_bytes={}", bytes.len() - end);
    }
}

/// A zombie has exited and cannot handle protocol input, even if a slow init
/// process has not yet reaped its /proc entry.
fn process_active(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(end_of_comm) = stat.rfind(')') else {
        return true;
    };
    match stat[end_of_comm + 1..].trim_start().chars().next() {
        Some('Z' | 'X') => false,
        Some(_) | None => true,
    }
}

fn discover_plugin_dirs() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let output = Command::new("rpm").args(["-ql", "libzypp"]).output()?;
    if !output.status.success() {
        return Err("could not enumerate libzypp-owned paths with rpm -ql libzypp".into());
    }
    let mut dirs = String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| line.ends_with("/plugins/commit") && Path::new(line).is_dir())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    dirs.sort();
    dirs.dedup();
    Ok(dirs)
}

fn provider_versions() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    for package in [
        "zypper",
        "libzypp",
        "PackageKit",
        "PackageKit-backend-zypp",
        "cockpit-packages",
        "myrlyn",
        "yast2",
    ] {
        let queried = rpm_query(&[
            "-q",
            "--qf",
            "%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n",
            package,
        ]);
        output.push(queried.unwrap_or_else(|| format!("{package} NOT_INSTALLED")));
    }
    Ok(output)
}

fn rpm_query(args: &[&str]) -> Option<String> {
    let output = Command::new("rpm").args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn rpm_state() -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("rpm")
        .args(["-qa", "--qf", "%{NAME} %{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n"])
        .output()?;
    if !output.status.success() {
        return Err("rpm -qa failed".into());
    }
    let mut rows = String::from_utf8(output.stdout)?.lines().map(str::to_owned).collect::<Vec<_>>();
    rows.sort();
    Ok(rows.join("\n"))
}

fn sha256(data: &str) -> String {
    sha256_bytes(data.as_bytes())
}

fn sha256_bytes(data: &[u8]) -> String {
    // Use the platform's coreutils binary rather than introducing another crate.
    let mut child = Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("sha256sum available in Tumbleweed");
    use std::io::Write;
    child.stdin.take().expect("piped stdin").write_all(data).expect("write hash input");
    let output = child.wait_with_output().expect("read hash output");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("hash-error")
        .to_owned()
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_file(&self.0);
        }
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // Avoid FFI and extra dependencies: `id -u` is stable on the supported guest.
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(u32::MAX)
}

#[cfg(all(test, target_os = "linux"))]
mod timeout_process_tree_tests {
    use std::{
        env, fs,
        io::Write,
        os::unix::fs::OpenOptionsExt,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    const CHILD_MODE: &str = "RYOKU_TIMEOUT_TREE_CHILD";
    const PID_FILE: &str = "RYOKU_TIMEOUT_TREE_PID_FILE";

    fn record_pid() {
        let path = env::var_os(PID_FILE).expect("timeout test PID file path");
        let mut file =
            fs::OpenOptions::new().append(true).open(path).expect("open timeout test PID file");
        writeln!(file, "{}", std::process::id()).expect("record timeout test PID");
    }

    #[test]
    fn timeout_probe_child() {
        if env::var_os(CHILD_MODE).is_none() {
            return;
        }
        record_pid();
        let executable = env::current_exe().expect("test executable path");
        let mut grandchild = Command::new(executable)
            .args([
                "--exact",
                "timeout_process_tree_tests::timeout_probe_grandchild",
                "--nocapture",
            ])
            .env(CHILD_MODE, "grandchild")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Rust timeout grandchild");
        loop {
            let _ = grandchild.try_wait();
            thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn timeout_probe_grandchild() {
        if env::var_os(CHILD_MODE).as_deref() != Some(std::ffi::OsStr::new("grandchild")) {
            return;
        }
        record_pid();
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn gnu_timeout_kills_the_rust_probe_process_tree() {
        let version =
            Command::new("timeout").arg("--version").output().expect("GNU timeout available on CI");
        assert!(version.status.success());
        assert!(String::from_utf8_lossy(&version.stdout).contains("GNU coreutils"));

        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let pid_file =
            env::temp_dir().join(format!("ryoku-timeout-tree-{}-{nonce}", std::process::id()));
        let _cleanup = super::Cleanup(pid_file.clone());
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pid_file)
            .expect("create private timeout test PID file");
        drop(file);

        let executable = env::current_exe().expect("test executable path");
        let result = Command::new("timeout")
            .args(["--signal=TERM", "--kill-after=1s", "3s"])
            .arg(executable)
            .args(["--exact", "timeout_process_tree_tests::timeout_probe_child", "--nocapture"])
            .env(CHILD_MODE, "parent")
            .env(PID_FILE, &pid_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("run bounded Rust process tree");
        assert_eq!(result.status.code(), Some(124), "timeout wrapper must report deadline");

        let pids = fs::read_to_string(&pid_file)
            .expect("Rust child and grandchild started")
            .lines()
            .map(|line| line.parse::<u32>().expect("valid child PID"))
            .collect::<Vec<_>>();
        assert_eq!(pids.len(), 2, "both the child and grandchild must have started");
        let deadline = Instant::now() + Duration::from_secs(3);
        while pids.iter().any(|pid| super::process_active(*pid)) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        for pid in &pids {
            if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) {
                eprintln!(
                    "test child {pid} status: {}",
                    status
                        .lines()
                        .find(|line| line.starts_with("State:"))
                        .unwrap_or("missing State")
                );
            }
        }
        assert!(
            pids.iter().all(|pid| !super::process_active(*pid)),
            "GNU timeout must terminate both Rust processes (zombies count as terminated)"
        );
    }
}
