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
    if env::args().len() == 1 {
        if let Err(error) = protocol::serve() {
            eprintln!("probe_error={error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = experiment() {
        eprintln!("experiment_error={error}");
        std::process::exit(1);
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
        while plugin_pids.iter().any(|pid| Path::new(&format!("/proc/{pid}")).exists())
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!(
            "timeout_plugin_terminated={}",
            plugin_pids.iter().all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
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
    if mode == "timeout-commit"
        && plugin_pids.iter().any(|pid| Path::new(&format!("/proc/{pid}")).exists())
    {
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
    // Use the platform's coreutils binary rather than introducing another crate.
    let mut child = Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("sha256sum available in Tumbleweed");
    use std::io::Write;
    child.stdin.take().expect("piped stdin").write_all(data.as_bytes()).expect("write hash input");
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
