use crate::{
    definition::scalar,
    support::{Result, output, public_evidence, require, safe_component},
};
use std::{
    fs,
    io::{Read, Write},
    net::ToSocketAddrs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "qmp.rs"]
mod qmp;

const FIRMWARE: &[(&str, &str)] =
    &[("OVMF_CODE_4M.fd", "ovmf_code_sha256"), ("OVMF_VARS_4M.fd", "ovmf_variables_sha256")];
const LOGIN_JOURNAL_FILENAME: &str = "sddm-logind-journal-private.log";
const LOGIN_JOURNAL_STREAM_LIMIT: usize = 1024 * 1024;
const LOGIN_JOURNAL_COMMAND: &str = "sudo /usr/bin/timeout --signal=KILL --kill-after=2s 15s /usr/bin/journalctl --no-pager --output=short-monotonic --since=-5min --lines=1000 --unit=sddm.service --unit=systemd-logind.service";

fn login_journal_path(run: &Path) -> PathBuf {
    run.join(LOGIN_JOURNAL_FILENAME)
}

fn read_capped<R: Read>(mut reader: R, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(64 * 1024));
    let mut truncated = false;
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok((retained, truncated));
        }
        let available = limit.saturating_sub(retained.len());
        let keep = count.min(available);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
}

fn save_private_login_journal(
    run: &Path,
    stdout: &[u8],
    stdout_truncated: bool,
    stderr: &[u8],
    stderr_truncated: bool,
) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(login_journal_path(run))?;
    file.write_all(stdout)?;
    if stdout_truncated {
        file.write_all(b"\n[stdout truncated at per-stream byte cap]\n")?;
    }
    file.write_all(b"\n--- journalctl stderr ---\n")?;
    file.write_all(stderr)?;
    if stderr_truncated {
        file.write_all(b"\n[stderr truncated at per-stream byte cap]\n")?;
    }
    file.sync_all()?;
    Ok(())
}

fn retain_probe_result<T, E>(
    probe: std::result::Result<T, E>,
    capture: impl FnOnce() -> Result<()>,
) -> std::result::Result<T, E> {
    match probe {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = capture();
            Err(error)
        }
    }
}

fn optional_ui_result(requested: bool, captured: bool) -> Option<&'static str> {
    requested.then_some(if captured {
        "ui_visual_capture=passed"
    } else {
        "ui_visual_capture=blocked"
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoginCapturePoint {
    BeforeGreeterInput,
    LoginTimeout,
}

impl LoginCapturePoint {
    fn filename(self) -> &'static str {
        match self {
            Self::BeforeGreeterInput => "sddm-before-input.ppm",
            Self::LoginTimeout => "sddm-login-timeout.ppm",
        }
    }
}

trait LoginMonitor {
    fn send_key(&mut self, qcode: &str) -> Result<()>;
    fn capture(&mut self, point: LoginCapturePoint) -> Result<()>;
}

struct PrivateLoginMonitor<'a> {
    control: &'a mut qmp::QmpClient,
    run: &'a Path,
}

impl LoginMonitor for PrivateLoginMonitor<'_> {
    fn send_key(&mut self, qcode: &str) -> Result<()> {
        self.control.send_key(qcode)?;
        Ok(())
    }

    fn capture(&mut self, point: LoginCapturePoint) -> Result<()> {
        let filename = point.filename();
        let path = self.run.join(filename);
        let result = (|| {
            self.control.screendump(filename)?;
            let metadata = fs::symlink_metadata(&path)?;
            require(
                metadata.is_file() && metadata.len() > 0,
                "QEMU did not create a private login framebuffer capture",
            )?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(path);
            let mut failures = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(self.run.join("login-frame-capture-private.log"))?;
            writeln!(failures, "{filename}=capture-failed")?;
            failures.sync_all()?;
        }
        result
    }
}

fn send_greeter_password(
    control: &mut impl LoginMonitor,
    password: &[u8],
    key_delay: Duration,
) -> Result<()> {
    // Diagnostic capture must never prevent the actual authentication probe.
    let _ = control.capture(LoginCapturePoint::BeforeGreeterInput);
    for byte in password {
        // The generated password alphabet is lowercase ASCII and is never
        // interpreted as QMP JSON.
        control.send_key(std::str::from_utf8(std::slice::from_ref(byte))?)?;
        thread::sleep(key_delay);
    }
    control.send_key("ret")?;
    Ok(())
}

fn capture_login_timeout(control: &mut impl LoginMonitor) {
    // Preserve the timeout result even if QEMU cannot write the diagnostic.
    let _ = control.capture(LoginCapturePoint::LoginTimeout);
}

struct Lab {
    assets: PathBuf,
    run: PathBuf,
    ui_binary: Option<PathBuf>,
    zypp_probe: Option<PathBuf>,
    zypp_package: Option<String>,
    port: Option<u16>,
    vm: Option<Child>,
    report: fs::File,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(mut child) = self.vm.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Lab {
    fn tool(&self, binary: &str) -> Command {
        let mut command = Command::new(self.assets.join("qemu/usr/bin").join(binary));
        command.env("LD_LIBRARY_PATH", self.assets.join("qemu/usr/lib/x86_64-linux-gnu"));
        command
    }

    fn evidence(&mut self, lines: &str) -> Result<()> {
        for line in lines.lines() {
            // Publish a strict key/value alphabet; never arbitrary guest output.
            if public_evidence(line) {
                writeln!(self.report, "{line}")?;
                println!("{line}");
            }
        }
        self.report.flush()?;
        Ok(())
    }

    fn overlay(&self) -> Result<()> {
        output(
            self.tool("qemu-img")
                .args(["create", "-f", "qcow2", "-F", "qcow2", "-b"])
                .arg(self.assets.join(scalar("image")?))
                .arg(self.run.join("disk.qcow2"))
                .arg(format!("{}G", scalar("disk_virtual_gib")?)),
        )?;
        fs::copy(
            self.assets.join("qemu/usr/share/OVMF/OVMF_VARS_4M.fd"),
            self.run.join("OVMF_VARS.fd"),
        )?;
        Ok(())
    }

    fn start(&mut self, offline: bool) -> Result<()> {
        require(self.vm.is_none(), "VM already owned by run")?;
        self.port = None;
        let mut command = self.tool("qemu-system-x86_64");
        // QMP screendumps use fixed relative names and stay below this private
        // mode-0700 directory. All other QEMU inputs use absolute paths.
        command.current_dir(&self.run);
        command
            .arg("-L")
            .arg(self.assets.join("qemu/usr/share/qemu"))
            .arg("-drive")
            .arg(format!(
                "if=pflash,format=raw,readonly=on,file={}",
                self.assets.join("qemu/usr/share/OVMF/OVMF_CODE_4M.fd").display()
            ))
            .arg("-drive")
            .arg(format!("if=pflash,format=raw,file={}", self.run.join("OVMF_VARS.fd").display()))
            .args([
                "-machine",
                scalar("machine")?,
                "-accel",
                "tcg,thread=multi",
                "-cpu",
                "max",
                "-smp",
                scalar("cpus")?,
                "-m",
                scalar("memory_mib")?,
            ])
            .arg("-drive")
            .arg(format!("file={},format=qcow2,if=virtio", self.run.join("disk.qcow2").display()))
            .arg("-drive")
            .arg(format!(
                "file={},format=raw,media=cdrom,readonly=on",
                self.run.join("seed.iso").display()
            ))
            .arg("-netdev")
            .arg(format!(
                "user,id=net0,restrict={},hostfwd=tcp:127.0.0.1:0-:22",
                if offline { "on" } else { "off" }
            ))
            .args(["-device", "virtio-net-pci,netdev=net0,romfile=", "-vga", "none"])
            .arg("-device")
            .arg(format!(
                "VGA,romfile={}",
                self.assets.join("qemu/usr/share/seabios/vgabios-stdvga.bin").display()
            ))
            .args(["-display", "none", "-monitor", "none"])
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", self.run.join("qmp-private.sock").display()))
            .arg("-serial")
            .arg(format!("file:{}", self.run.join("serial-private.log").display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(self.run.join("qemu-private.log"))?);
        if self.run.join("migration.qcow2").exists() {
            command.arg("-drive").arg(format!(
                "file={},format=qcow2,if=virtio",
                self.run.join("migration.qcow2").display()
            ));
        }
        self.vm = Some(command.spawn()?);
        // QEMU binds port zero atomically. Query only our private owned QMP
        // endpoint; no temporary listener is dropped before another bind.
        let mut control = qmp::QmpClient::connect(&self.run.join("qmp-private.sock"))?;
        self.port = Some(control.forwarded_ssh_port()?);
        self.wait_ssh()
    }

    fn ssh_command(&self) -> Result<Command> {
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        let mut command = Command::new("ssh");
        command
            .args(["-F", "/dev/null", "-i"])
            .arg(self.run.join("id_ed25519"))
            .args([
                "-p",
                &port.to_string(),
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=30",
                "-o",
                "ServerAliveInterval=10",
                "-o",
                "ServerAliveCountMax=6",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
            ])
            .arg(format!("UserKnownHostsFile={}", self.run.join("known_hosts").display()))
            .arg("tester@127.0.0.1");
        Ok(command)
    }

    fn ssh(&self, request: &str) -> Result<String> {
        let result = self.ssh_command()?.arg(request).output()?;
        fs::write(
            self.run.join("guest-private.log"),
            [&result.stdout[..], &result.stderr[..]].concat(),
        )?;
        require(result.status.success(), "guest command failed; inspect private guest log")?;
        Ok(String::from_utf8(result.stdout)?)
    }

    fn login(&self, password: &[u8]) -> Result<String> {
        let mut child = self
            .ssh_command()?
            .arg("sudo /tmp/ryoku-vm-lab guest-login-prepare")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // No password is placed in argv, environment, disk files or command
        // logs. Guest preparation consumes this pipe and suppresses output from
        // the native secret-bearing password command.
        let mut input = child.stdin.take().ok_or("missing private password pipe")?;
        let written = input.write_all(password).and_then(|()| input.write_all(b"\n"));
        drop(input);
        let result = child.wait_with_output()?;
        written?;
        fs::write(
            self.run.join("desktop-prepare-private.log"),
            [&result.stdout[..], &result.stderr[..]].concat(),
        )?;
        require(result.status.success(), "native desktop login preparation failed")?;
        let prepared = String::from_utf8(result.stdout)?;
        let mut control = qmp::QmpClient::connect(&self.run.join("qmp-private.sock"))?;
        let mut monitor = PrivateLoginMonitor { control: &mut control, run: &self.run };
        send_greeter_password(&mut monitor, password, Duration::from_millis(150))?;
        let deadline = Instant::now() + Duration::from_secs(180);
        let mut last_error = String::from("login inspection has not completed");
        let mut journal_capture_attempted = false;
        while Instant::now() < deadline {
            let inspected = self.ssh("sudo /tmp/ryoku-vm-lab guest-login-inspect");
            let inspected = if !journal_capture_attempted && inspected.is_err() {
                journal_capture_attempted = true;
                retain_probe_result(inspected, || self.capture_login_journal())
            } else {
                inspected
            };
            match inspected {
                Ok(inspected) => return Ok(format!("{prepared}{inspected}")),
                Err(error) => {
                    last_error = error.to_string();
                    // Keep the most recent failed inspection separately from
                    // the rolling guest log. It contains only safe login
                    // state diagnostics, never the generated password.
                    let diagnostic = fs::read(self.run.join("guest-private.log"))
                        .unwrap_or_else(|_| last_error.as_bytes().to_vec());
                    fs::write(self.run.join("desktop-login-last-private.log"), diagnostic)?;
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(Duration::from_secs(3)));
        }
        capture_login_timeout(&mut monitor);
        Err(format!(
            "actual SDDM login and active Plasma session were not verified; last inspection: {last_error}"
        )
        .into())
    }

    fn capture_login_journal(&self) -> Result<()> {
        let mut child = self
            .ssh_command()?
            .arg(LOGIN_JOURNAL_COMMAND)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("journal stdout pipe unavailable")?;
        let stderr = child.stderr.take().ok_or("journal stderr pipe unavailable")?;
        let stdout_reader = thread::spawn(move || read_capped(stdout, LOGIN_JOURNAL_STREAM_LIMIT));
        let stderr_reader = thread::spawn(move || read_capped(stderr, LOGIN_JOURNAL_STREAM_LIMIT));
        let status = child.wait()?;
        let (stdout, stdout_truncated) =
            stdout_reader.join().map_err(|_| "journal stdout reader panicked")??;
        let (stderr, stderr_truncated) =
            stderr_reader.join().map_err(|_| "journal stderr reader panicked")??;
        save_private_login_journal(
            &self.run,
            &stdout,
            stdout_truncated,
            &stderr,
            stderr_truncated,
        )?;
        require(status.success(), "bounded private login journal capture failed")?;
        if stdout_truncated || stderr_truncated {
            return Err("private login journal output exceeded the per-stream byte cap".into());
        }
        Ok(())
    }

    fn wait_ssh(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(600);
        while Instant::now() < deadline {
            if let Some(child) = &mut self.vm {
                require(child.try_wait()?.is_none(), "QEMU exited before SSH")?;
            }
            if self.ssh("timeout 120 cloud-init status --wait").is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_secs(3));
        }
        Err("guest did not become ready in 600 seconds".into())
    }

    fn copy_binary(&self) -> Result<()> {
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        output(
            Command::new("scp")
                .args(["-F", "/dev/null", "-i"])
                .arg(self.run.join("id_ed25519"))
                .args([
                    "-P",
                    &port.to_string(),
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                ])
                .arg(format!("UserKnownHostsFile={}", self.run.join("known_hosts").display()))
                .arg(std::env::current_exe()?)
                .arg("tester@127.0.0.1:/tmp/ryoku-vm-lab"),
        )?;
        Ok(())
    }

    fn copy_zypp_probe(&self) -> Result<()> {
        let binary = self.zypp_probe.as_ref().ok_or("no zypper probe binary configured")?;
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        output(
            Command::new("scp")
                .args(["-F", "/dev/null", "-i"])
                .arg(self.run.join("id_ed25519"))
                .args([
                    "-P",
                    &port.to_string(),
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                ])
                .arg(format!("UserKnownHostsFile={}", self.run.join("known_hosts").display()))
                .arg(binary)
                .arg("tester@127.0.0.1:/tmp/ryoku-zypp-hook-probe.upload"),
        )?;
        self.ssh("sudo install -m 0755 /tmp/ryoku-zypp-hook-probe.upload /usr/local/sbin/ryoku-zypp-hook-probe")?;
        Ok(())
    }

    fn zypp_probe(&mut self, mode: &str, log_name: &str) -> Result<(bool, String)> {
        let package = self.zypp_package.as_deref().ok_or("missing zypper probe package")?;
        let request = if mode == "inventory" {
            "sudo /usr/local/sbin/ryoku-zypp-hook-probe --inventory".to_owned()
        } else {
            format!("sudo /usr/local/sbin/ryoku-zypp-hook-probe {package} {mode}")
        };
        let result = self.ssh_command()?.arg(&request).output()?;
        let private_log = self.run.join(log_name);
        fs::write(&private_log, [&result.stdout[..], &result.stderr[..]].concat())?;
        fs::set_permissions(&private_log, fs::Permissions::from_mode(0o600))?;
        let stdout = String::from_utf8(result.stdout)?;
        let label_storage = mode.replace('-', "_");
        let label = if mode == "inventory" { "inventory" } else { label_storage.as_str() };
        let mut state_unchanged = None;
        let mut commit_seen = None;
        let mut timeout_seen = None;
        let mut terminated = None;
        let mut installed = None;
        let mut outcome = None;
        for line in stdout.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key {
                "rpm_state_unchanged" => state_unchanged = Some(value == "true"),
                "plugin_commitbegin_seen" => commit_seen = Some(value == "true"),
                "transaction_timed_out" => timeout_seen = Some(value == "true"),
                "timeout_plugin_terminated" => terminated = Some(value == "true"),
                "requested_package_installed" => installed = Some(value == "true"),
                "outcome" => outcome = Some(value),
                _ => {}
            }
            let safe_value = !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-:|/@".contains(&byte));
            if safe_value {
                match key {
                    "inventory_packages" | "inventory_plugin_directories" => {
                        let field = if key == "inventory_packages" {
                            "packages"
                        } else {
                            "plugin_directories"
                        };
                        self.evidence(&format!("zypp_inventory_{field}={value}"))?;
                    }
                    "inventory_rpm_ql" => {
                        let fields = value.split('|').collect::<Vec<_>>();
                        if let [path, owner] = fields.as_slice() {
                            let normalized = path.trim_matches('/').replace(['/', '.'], "_");
                            let owner = owner.to_ascii_lowercase().replace('-', "_");
                            if normalized.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
                            }) && owner
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                            {
                                self.evidence(&format!(
                                    "zypp_inventory_rpm_ql={normalized}_{owner}"
                                ))?;
                            }
                        }
                    }
                    "inventory_provider" => {
                        let fields = value.split('|').collect::<Vec<_>>();
                        if let [name, epoch, version, release, arch] = fields.as_slice() {
                            let name = name.to_ascii_lowercase().replace('-', "_");
                            if name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                            {
                                for (suffix, field) in [
                                    ("epoch", *epoch),
                                    ("version", *version),
                                    ("release", *release),
                                    ("arch", *arch),
                                ] {
                                    self.evidence(&format!(
                                        "zypp_inventory_{name}_{suffix}={field}"
                                    ))?;
                                }
                            }
                        }
                    }
                    "inventory_path" => {
                        let fields = value.split('|').collect::<Vec<_>>();
                        if let Some(path) = fields.first() {
                            let normalized = path.trim_matches('/').replace(['/', '.'], "_");
                            if normalized.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
                            }) {
                                let index = self.run.join(".zypp-inventory-counter");
                                let current = fs::read_to_string(&index)
                                    .ok()
                                    .and_then(|x| x.parse::<usize>().ok())
                                    .unwrap_or(0)
                                    + 1;
                                fs::write(&index, current.to_string())?;
                                fs::set_permissions(&index, fs::Permissions::from_mode(0o600))?;
                                self.evidence(&format!(
                                    "zypp_inventory_path_{current}={normalized}"
                                ))?;
                                if let Some(owner) =
                                    fields.iter().find_map(|field| field.strip_prefix("owner="))
                                {
                                    let owner = owner.split('|').next().unwrap_or(owner);
                                    let (owner_name, owner_evr) =
                                        owner.split_once('@').unwrap_or((owner, ""));
                                    let owner_name =
                                        owner_name.to_ascii_lowercase().replace('-', "_");
                                    if owner_name
                                        .bytes()
                                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                                    {
                                        self.evidence(&format!(
                                            "zypp_inventory_owner_{current}={owner_name}"
                                        ))?;
                                    }
                                    if let Some((epoch, rest)) = owner_evr.split_once(':')
                                        && let Some((version, release_arch)) = rest.split_once('-')
                                        && let Some((release, arch)) = release_arch.rsplit_once('.')
                                    {
                                        for (suffix, field) in [
                                            ("owner_epoch", epoch),
                                            ("owner_version", version),
                                            ("owner_release", release),
                                            ("owner_arch", arch),
                                        ] {
                                            self.evidence(&format!(
                                                "zypp_inventory_{suffix}_{current}={field}"
                                            ))?;
                                        }
                                    }
                                }
                                for prefix in ["mode=", "uid=", "gid="] {
                                    if let Some(field) =
                                        fields.iter().find_map(|field| field.strip_prefix(prefix))
                                    {
                                        let field = field.split('|').next().unwrap_or(field);
                                        if field.bytes().all(|byte| byte.is_ascii_digit()) {
                                            self.evidence(&format!(
                                                "zypp_inventory_{}_{current}={field}",
                                                prefix.trim_end_matches('=')
                                            ))?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "provider" => {
                        let mut fields = value.split_whitespace();
                        if let (Some(name), Some(evr), None) =
                            (fields.next(), fields.next(), fields.next())
                            && let Some((epoch, rest)) = evr.split_once(':')
                            && let Some((version, release_arch)) = rest.split_once('-')
                            && let Some((release, arch)) = release_arch.rsplit_once('.')
                        {
                            let name = name.to_ascii_lowercase().replace('-', "_");
                            for (suffix, field) in [
                                ("epoch", epoch),
                                ("version", version),
                                ("release", release),
                                ("arch", arch),
                            ] {
                                self.evidence(&format!("zypp_{label}_{name}_{suffix}={field}"))?;
                            }
                        }
                    }
                    "package_requested" => {
                        self.evidence(&format!("zypp_{label}_package={value}"))?;
                    }
                    "rpm_state_before_sha256" | "rpm_state_after_sha256" => {
                        let field = if key == "rpm_state_before_sha256" {
                            "rpm_before_sha256"
                        } else {
                            "rpm_after_sha256"
                        };
                        self.evidence(&format!("zypp_{label}_{field}={value}"))?;
                    }
                    "plugin_directory" => {
                        let normalized = value.trim_matches('/').replace('/', "_");
                        if normalized
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        {
                            self.evidence(&format!("zypp_plugin_directory={normalized}"))?;
                        }
                    }
                    "plugin_registered"
                    | "plugin_invoked"
                    | "plugin_commitbegin_seen"
                    | "rpm_state_unchanged"
                    | "requested_package_installed"
                    | "transaction_timed_out"
                    | "timeout_plugin_terminated"
                    | "plugin_cleanup"
                    | "zypper_success"
                    | "outcome" => {
                        let field = match key {
                            "plugin_registered" => "registered",
                            "plugin_invoked" => "invoked",
                            "plugin_commitbegin_seen" => "commitbegin",
                            "rpm_state_unchanged" => "rpm_unchanged",
                            "requested_package_installed" => "package_installed",
                            "transaction_timed_out" => "timed_out",
                            "timeout_plugin_terminated" => "plugin_terminated",
                            "plugin_cleanup" => "plugin_cleanup",
                            "zypper_success" => "zypper_success",
                            "outcome" => "outcome",
                            _ => unreachable!(),
                        };
                        self.evidence(&format!("zypp_{label}_{field}={value}"))?;
                    }
                    _ => {}
                }
            }
        }
        let _ = fs::remove_file(self.run.join(".zypp-inventory-counter"));
        let passed = if mode == "inventory" {
            result.status.success()
                && stdout.lines().any(|line| line.starts_with("inventory_packages="))
        } else if mode == "timeout-commit" {
            result.status.success()
                && commit_seen == Some(true)
                && timeout_seen == Some(true)
                && terminated == Some(true)
                && state_unchanged == Some(true)
        } else if mode == "missing-plugin" {
            state_unchanged == Some(true) && result.status.success() && installed == Some(true)
        } else {
            result.status.success()
                && commit_seen == Some(true)
                && state_unchanged == Some(true)
                && timeout_seen == Some(false)
        };
        self.evidence(&format!("zypp_{label}_exit_code={}", result.status.code().unwrap_or(255)))?;
        Ok((passed, outcome.unwrap_or("no_outcome").to_owned()))
    }

    fn copy_ui_binary(&self) -> Result<()> {
        let Some(binary) = &self.ui_binary else {
            return Ok(());
        };
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        output(
            Command::new("scp")
                .args(["-F", "/dev/null", "-i"])
                .arg(self.run.join("id_ed25519"))
                .args([
                    "-P",
                    &port.to_string(),
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                ])
                .arg(format!("UserKnownHostsFile={}", self.run.join("known_hosts").display()))
                .arg(binary)
                .arg("tester@127.0.0.1:/tmp/ryoku-ui-gtk.upload"),
        )?;
        self.ssh("sudo /tmp/ryoku-vm-lab guest-ui-install")?;
        Ok(())
    }

    fn capture_screenshot(&self, filename: &str) -> Result<()> {
        let mut control = qmp::QmpClient::connect(&self.run.join("qmp-private.sock"))?;
        control.screendump(filename)?;
        let path = self.run.join(filename);
        let metadata = fs::symlink_metadata(&path)?;
        require(
            metadata.is_file() && metadata.len() > 0,
            "QEMU did not create a private framebuffer capture",
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    fn capture_ui_scenarios(&mut self) -> Result<()> {
        if self.ui_binary.is_none() {
            return Ok(());
        }
        self.copy_ui_binary()?;
        for (scenario, filename) in
            [("success", "ryoku-ui-success.ppm"), ("error", "ryoku-ui-error.ppm")]
        {
            self.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-ui-launch {scenario}"))?;
            thread::sleep(Duration::from_secs(3));
            let mut control = qmp::QmpClient::connect(&self.run.join("qmp-private.sock"))?;
            control.send_key("ret")?;
            thread::sleep(Duration::from_secs(2));
            self.capture_screenshot(filename)?;
            self.ssh("sudo /tmp/ryoku-vm-lab guest-ui-stop")?;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        // A closed SSH stream can be the successful poweroff transition.
        let _ = self.ssh("sudo systemctl poweroff");
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if let Some(status) = self.vm.as_mut().ok_or("no VM owned")?.try_wait()? {
                require(status.success(), "QEMU exited abnormally during poweroff")?;
                self.vm.take();
                self.port = None;
                return Ok(());
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err("clean poweroff failed; Drop will terminate owned QEMU and retain diagnostics".into())
    }

    fn seed(&self) -> Result<()> {
        output(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(self.run.join("id_ed25519")),
        )?;
        let public = fs::read_to_string(self.run.join("id_ed25519.pub"))?;
        let public = public.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        let mut config = format!(
            "#cloud-config\nusers:\n  - name: tester\n    groups: [wheel]\n    sudo: 'ALL=(ALL) NOPASSWD:ALL'\n    shell: /bin/bash\n    lock_passwd: true\n    ssh_authorized_keys:\n      - {public}\ndisable_root: true\nssh_pwauth: false\nwrite_files:\n  - path: /etc/pki/trust/anchors/cloud-lab.crt\n    content: |\n"
        );
        for entry in fs::read_dir("/usr/local/share/ca-certificates")? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "crt") {
                for line in fs::read_to_string(path)?.lines() {
                    config.push_str(&format!("      {line}\n"));
                }
            }
        }
        config.push_str(
            "  - path: /etc/ryoku-lab-fixture\n    content: |\n      disposable-issue13\nruncmd:\n  - [update-ca-certificates]\n  - [systemctl, enable, --now, sshd]\n",
        );
        fs::write(self.run.join("user-data"), config)?;
        fs::write(
            self.run.join("meta-data"),
            "instance-id: ryoku-lab-fixture\nlocal-hostname: ryoku-lab\n",
        )?;
        output(
            self.tool("genisoimage")
                .args(["-quiet", "-output"])
                .arg(self.run.join("seed.iso"))
                .args(["-volid", "cidata", "-joliet", "-rock"])
                .arg(self.run.join("user-data"))
                .arg(self.run.join("meta-data")),
        )?;
        Ok(())
    }
}

fn desktop_password() -> Result<Vec<u8>> {
    let mut entropy = fs::File::open("/dev/urandom")?;
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut password = Vec::with_capacity(32);
    while password.len() < 32 {
        let mut byte = [0_u8];
        entropy.read_exact(&mut byte)?;
        if byte[0] < 252 {
            password.push(alphabet[usize::from(byte[0]) % alphabet.len()]);
        }
    }
    Ok(password)
}

pub fn run(
    assets: &str,
    run: &str,
    ui_binary: Option<&str>,
    zypp_probe: Option<(&str, &str)>,
) -> Result<()> {
    let image = scalar("image")?;
    let hash = scalar("sha256")?;
    let signer = scalar("release_key_fingerprint")?;
    let qemu = scalar("qemu")?;
    let memory = scalar("memory_mib")?;
    let cpus = scalar("cpus")?;
    require(
        scalar("architecture")? == "x86_64" && scalar("accelerator")? == "tcg",
        "unsupported fixture architecture or accelerator",
    )?;
    require(
        std::env::var("RYOKU_LAB_AUTHORIZED").as_deref() == Ok("existing-cloud-workspace"),
        "set RYOKU_LAB_AUTHORIZED=existing-cloud-workspace only on authorized existing cloud capacity",
    )?;
    let assets = fs::canonicalize(assets)?;
    let ui_binary = ui_binary.map(fs::canonicalize).transpose()?;
    if let Some(binary) = &ui_binary {
        require(binary.is_file(), "GTK proof binary is not a regular file")?;
    }
    let zypp_probe = zypp_probe
        .map(|(binary, package)| {
            let binary = fs::canonicalize(binary)?;
            require(binary.is_file(), "zypper probe binary is not a regular file")?;
            require(
                !package.is_empty()
                    && package
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte)),
                "invalid zypper probe package identifier",
            )?;
            Ok::<_, Box<dyn std::error::Error>>((binary, package.to_owned()))
        })
        .transpose()?;
    let run_path = Path::new(run);
    require(run_path.is_absolute(), "run directory must be absolute")?;
    safe_component(run_path.file_name().and_then(|x| x.to_str()).ok_or("missing run name")?)?;
    let parent = fs::canonicalize(run_path.parent().ok_or("missing parent")?)?;
    require(
        parent.starts_with("/workspace") && !parent.starts_with(&assets),
        "use separate authorized cloud workspace run directory",
    )?;
    fs::create_dir(run_path)?; // Never resume or delete an unowned existing directory.
    fs::set_permissions(run_path, fs::Permissions::from_mode(0o700))?;
    let checksum = output(Command::new("sha256sum").arg(assets.join(image)))?;
    require(checksum.split_whitespace().next() == Some(hash), "base checksum mismatch")?;
    let signed_sum = fs::read_to_string(assets.join("image.sha256"))?;
    require(
        signed_sum.split_whitespace().next() == Some(hash),
        "signed checksum does not describe pinned base",
    )?;
    for (file, key) in FIRMWARE {
        let digest =
            output(Command::new("sha256sum").arg(assets.join("qemu/usr/share/OVMF").join(file)))?;
        require(
            digest.split_whitespace().next() == Some(scalar(key)?),
            "firmware checksum mismatch",
        )?;
    }
    let signature = output(
        Command::new("gpg")
            .args(["--no-auto-check-trustdb", "--homedir"])
            .arg(assets.join("gnupg"))
            .args(["--status-fd", "1", "--verify"])
            .arg(assets.join("image.sha256.asc"))
            .arg(assets.join("image.sha256")),
    )?;
    require(
        signature.lines().any(|x| x.starts_with(&format!("[GNUPG:] VALIDSIG {signer} "))),
        "unexpected release signer",
    )?;
    let address = ("proxy", 8080).to_socket_addrs()?.next().ok_or("managed proxy DNS missing")?;
    let mut lab = Lab {
        assets,
        run: fs::canonicalize(run_path)?,
        ui_binary,
        zypp_probe: zypp_probe.as_ref().map(|(binary, _)| binary.clone()),
        zypp_package: zypp_probe.as_ref().map(|(_, package)| package.clone()),
        port: None,
        vm: None,
        report: fs::File::create(run_path.join("evidence.txt"))?,
    };
    lab.evidence(&format!("fixture_sha256={hash}\nrelease_signer={signer}\nhost_runner=existing-managed-cloud\naccelerator=tcg\nvm_memory_mib={memory}\nvm_cpus={cpus}"))?;
    let version = output(lab.tool("qemu-system-x86_64").arg("--version"))?;
    require(
        version.starts_with(&format!("QEMU emulator version {qemu}")),
        "unexpected QEMU version",
    )?;
    lab.evidence(&format!("qemu={qemu}\nimage_signature=verified"))?;
    lab.seed()?;
    lab.overlay()?;
    output(
        lab.tool("qemu-img")
            .args(["create", "-f", "qcow2"])
            .arg(lab.run.join("migration.qcow2"))
            .arg(format!("{}G", scalar("disk_virtual_gib")?)),
    )?;
    lab.evidence("fresh_overlay=created")?;
    lab.start(false)?;
    lab.evidence("fresh_boot=passed")?;
    lab.copy_binary()?;
    let migrated = lab.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-migrate {address}"))?;
    lab.evidence(&migrated)?;
    lab.stop()?;
    fs::rename(lab.run.join("disk.qcow2"), lab.run.join("source-overlay.qcow2"))?;
    fs::rename(lab.run.join("migration.qcow2"), lab.run.join("disk.qcow2"))?;
    fs::copy(lab.assets.join("qemu/usr/share/OVMF/OVMF_VARS_4M.fd"), lab.run.join("OVMF_VARS.fd"))?;
    lab.start(false)?;
    lab.copy_binary()?;
    let base = lab.ssh("sudo /tmp/ryoku-vm-lab guest-base-setup")?;
    lab.evidence(&base)?;
    lab.evidence("derived_btrfs_boot=passed")?;
    let kde_installed = match lab.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-setup {address}")) {
        Ok(setup) => {
            lab.evidence(&setup)?;
            true
        }
        Err(_) => {
            fs::copy(lab.run.join("guest-private.log"), lab.run.join("kde-failure-private.log"))?;
            lab.evidence("kde_provisioning=blocked")?;
            false
        }
    };
    let mut password = desktop_password()?;
    let mut kde_ready = false;
    let mut ui_captured = false;
    if kde_installed {
        match lab.login(&password) {
            Ok(session) => {
                lab.evidence(&session)?;
                kde_ready = true;
                if lab.ui_binary.is_some() {
                    ui_captured = lab.capture_ui_scenarios().is_ok();
                }
            }
            Err(_) => {
                lab.evidence("desktop_login=blocked")?;
            }
        }
    } else {
        lab.evidence("desktop_login=blocked")?;
    }
    let before = lab.ssh("cat /proc/sys/kernel/random/boot_id")?;
    let _ = lab.ssh("sudo systemctl reboot");
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut changed = false;
    while Instant::now() < deadline {
        if let Ok(after) = lab.ssh("cat /proc/sys/kernel/random/boot_id")
            && before.trim() != after.trim()
        {
            changed = true;
            break;
        }
        thread::sleep(Duration::from_secs(3));
    }
    require(changed, "boot identity did not change within 600 seconds")?;
    lab.wait_ssh()?;
    lab.copy_binary()?;
    if kde_ready {
        match lab.login(&password) {
            Ok(session) => {
                lab.evidence(&session)?;
            }
            Err(_) => {
                kde_ready = false;
                lab.evidence("desktop_login=blocked")?;
            }
        }
    }
    password.fill(0);
    let inspect = if kde_ready {
        "sudo /tmp/ryoku-vm-lab guest-inspect"
    } else {
        "sudo /tmp/ryoku-vm-lab guest-inspect-base"
    };
    let inspected = lab.ssh(inspect)?;
    lab.evidence(&inspected)?;
    lab.evidence("reboot=passed")?;
    if zypp_probe.is_some() {
        // Probe modes deliberately include a missing-plugin bypass and can
        // alter RPM state. Keep their output private and jump directly to the
        // disposable-overlay reset, even when a result is unsafe/incomplete.
        let copy_ok = match lab.copy_zypp_probe() {
            Ok(()) => true,
            Err(_) => {
                lab.evidence("zypp_probe_copy=failed")?;
                false
            }
        };
        if copy_ok {
            match lab.zypp_probe("inventory", "zypp-inventory-private.log") {
                Ok((true, _)) => lab.evidence("zypp_inventory=passed")?,
                Ok((false, _)) | Err(_) => lab.evidence("zypp_inventory=failed")?,
            }
            for (mode, log) in [
                ("deny-commit", "zypp-deny-private.log"),
                ("malformed-commit", "zypp-malformed-private.log"),
                ("timeout-commit", "zypp-timeout-private.log"),
                ("missing-plugin", "zypp-missing-private.log"),
            ] {
                match lab.zypp_probe(mode, log) {
                    Ok((passed, outcome)) => {
                        if !passed && mode != "missing-plugin" {
                            lab.evidence(&format!(
                                "zypp_{}_gate=unsupported",
                                mode.replace('-', "_")
                            ))?;
                            // A failed denial may have changed RPM state; do
                            // not issue more package transactions in that case.
                            let private = fs::read_to_string(lab.run.join(log)).unwrap_or_default();
                            if private.contains("rpm_state_unchanged=false") {
                                lab.evidence("zypp_later_modes=skipped_after_rpm_change")?;
                                break;
                            }
                        } else if mode == "missing-plugin" {
                            let classification = if outcome == "missing_plugin_allowed_rpm_change" {
                                "bypass_committed"
                            } else if passed {
                                "unexpectedly_passed"
                            } else {
                                "inconclusive_or_blocked_elsewhere"
                            };
                            lab.evidence(&format!("zypp_missing_plugin_gate={classification}"))?;
                        } else if passed {
                            lab.evidence(&format!(
                                "zypp_{}_gate=observed_block",
                                mode.replace('-', "_")
                            ))?;
                        }
                    }
                    Err(_) => {
                        lab.evidence(&format!("zypp_{mode}_gate=probe_failed"))?;
                        break;
                    }
                }
            }
        } else {
            lab.evidence("zypp_probe_modes=skipped_copy_failed")?;
        }
        lab.evidence("packagekit_cockpit_myrlyn_yast_write_paths=unsupported")?;
        lab.evidence("zypp_experiment_is_not_a_protected_write_gate=confirmed")?;
        // This is a disposable test overlay. Terminate only the QEMU process
        // this Lab owns; its private overlay is deleted below and never reused.
        if let Some(mut child) = lab.vm.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        lab.port = None;
        lab.evidence("offline_recovery=skipped_provider_probe_overlay_discarded")?;
    } else {
        lab.stop()?;
        lab.start(true)?;
        lab.copy_binary()?;
        let recovered = lab.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-recover {address}"))?;
        lab.evidence(&recovered)?;
        lab.stop()?;
    }
    fs::remove_file(lab.run.join("disk.qcow2"))?;
    fs::remove_file(lab.run.join("OVMF_VARS.fd"))?;
    fs::remove_file(lab.run.join("known_hosts"))?;
    lab.overlay()?;
    lab.start(true)?;
    lab.copy_binary()?;
    let reset = lab.ssh("sudo /tmp/ryoku-vm-lab guest-marker-absent")?;
    lab.evidence(&reset)?;
    // Guest checks RPM's exact absence result; transport failures cannot pass.
    lab.evidence("reset_fresh_boot=passed")?;
    lab.stop()?;
    for name in [
        "disk.qcow2",
        "source-overlay.qcow2",
        "OVMF_VARS.fd",
        "seed.iso",
        "user-data",
        "meta-data",
        "id_ed25519",
        "id_ed25519.pub",
        "known_hosts",
    ] {
        fs::remove_file(lab.run.join(name))?;
    }
    let checksum = output(Command::new("sha256sum").arg(lab.assets.join(image)))?;
    require(checksum.split_whitespace().next() == Some(hash), "base changed during run")?;
    lab.evidence("base_unchanged=passed\nreset_disk_and_keys=removed")?;
    lab.evidence("lifecycle_result=passed")?;
    if let Some(result) = optional_ui_result(lab.ui_binary.is_some(), ui_captured) {
        lab.evidence(result)?;
    }
    if !kde_ready {
        lab.evidence("lab_result=blocked")?;
    }
    require(
        kde_ready,
        "KDE package/login acceptance blocked; mandatory base VM evidence completed, inspect private diagnostics",
    )?;
    lab.evidence("lab_result=passed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingLoginMonitor {
        events: Vec<String>,
        fail_captures: bool,
    }

    impl LoginMonitor for RecordingLoginMonitor {
        fn send_key(&mut self, qcode: &str) -> Result<()> {
            self.events.push(format!("key:{qcode}"));
            Ok(())
        }

        fn capture(&mut self, point: LoginCapturePoint) -> Result<()> {
            self.events.push(format!("capture:{}", point.filename()));
            if self.fail_captures { Err("mock screenshot failure".into()) } else { Ok(()) }
        }
    }

    #[test]
    fn login_captures_pre_input_and_timeout_frames_in_order() {
        let mut monitor = RecordingLoginMonitor { fail_captures: true, ..Default::default() };
        send_greeter_password(&mut monitor, b"ab2", Duration::ZERO).unwrap();
        capture_login_timeout(&mut monitor);

        assert_eq!(
            monitor.events,
            [
                "capture:sddm-before-input.ppm",
                "key:a",
                "key:b",
                "key:2",
                "key:ret",
                "capture:sddm-login-timeout.ppm",
            ]
        );
    }

    #[test]
    fn login_capture_names_are_fixed_distinct_ppm_files() {
        let before = LoginCapturePoint::BeforeGreeterInput.filename();
        let timeout = LoginCapturePoint::LoginTimeout.filename();
        assert_eq!(before, "sddm-before-input.ppm");
        assert_eq!(timeout, "sddm-login-timeout.ppm");
        assert_ne!(before, timeout);
        assert!(qmp::validate_screenshot_filename(before).is_ok());
        assert!(qmp::validate_screenshot_filename(timeout).is_ok());
    }

    #[test]
    fn failed_session_probe_captures_journal_without_replacing_probe_result() {
        use std::cell::RefCell;

        let events = RefCell::new(vec!["ordinary-user session probe failed"]);
        let probe: std::result::Result<(), &str> = Err("ordinary-user session probe failed");
        let result = retain_probe_result(probe, || {
            events.borrow_mut().push("journal capture immediately after probe failure");
            Err("journal capture failed".into())
        });
        events.borrow_mut().push("session probe retry");
        assert_eq!(result, Err("ordinary-user session probe failed"));
        assert_eq!(
            *events.borrow(),
            [
                "ordinary-user session probe failed",
                "journal capture immediately after probe failure",
                "session probe retry"
            ]
        );

        let called = RefCell::new(false);
        let success: std::result::Result<&str, &str> = Ok("session passed");
        let result = retain_probe_result(success, || {
            *called.borrow_mut() = true;
            Ok(())
        });
        assert_eq!(result, Ok("session passed"));
        assert!(!*called.borrow());
    }

    #[test]
    fn optional_ui_result_remains_separate_from_login_and_lifecycle_results() {
        assert_eq!(optional_ui_result(false, false), None);
        assert_eq!(optional_ui_result(true, false), Some("ui_visual_capture=blocked"));
        assert_eq!(optional_ui_result(true, true), Some("ui_visual_capture=passed"));
    }

    #[test]
    fn login_journal_uses_fixed_bounded_command_and_private_output_path() {
        let run = std::env::temp_dir().join(format!(
            "ryoku-login-journal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(login_journal_path(&run), run.join("sddm-logind-journal-private.log"));
        assert_eq!(
            LOGIN_JOURNAL_COMMAND,
            "sudo /usr/bin/timeout --signal=KILL --kill-after=2s 15s /usr/bin/journalctl --no-pager --output=short-monotonic --since=-5min --lines=1000 --unit=sddm.service --unit=systemd-logind.service"
        );

        save_private_login_journal(&run, b"sddm entry", false, b"", false).unwrap();
        let output = login_journal_path(&run);
        let metadata = fs::metadata(&output).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read(&output).unwrap(), b"sddm entry\n--- journalctl stderr ---\n");
        fs::remove_dir_all(run).unwrap();
    }

    #[test]
    fn journal_reader_caps_retained_bytes_and_reports_truncation() {
        let source = vec![b'x'; LOGIN_JOURNAL_STREAM_LIMIT + 37];
        let (retained, truncated) =
            read_capped(source.as_slice(), LOGIN_JOURNAL_STREAM_LIMIT).unwrap();
        assert_eq!(retained.len(), LOGIN_JOURNAL_STREAM_LIMIT);
        assert!(truncated);

        let (retained, truncated) = read_capped(b"short".as_slice(), 10).unwrap();
        assert_eq!(retained, b"short");
        assert!(!truncated);
    }
}
