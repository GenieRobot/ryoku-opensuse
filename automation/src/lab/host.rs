use crate::{
    definition::scalar,
    process::{self, BoundedOutput},
    runner,
    support::{Result, desktop_login_reason, output, public_evidence, require, safe_component},
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
const GREETER_READINESS_FRAME: &str = "sddm-readiness.ppm";
const GREETER_READINESS_TIMEOUT: Duration = Duration::from_secs(60);
const GREETER_READINESS_POLL: Duration = Duration::from_millis(500);
const KDE_PROVISION_TIMEOUT: Duration = Duration::from_secs(38 * 60);
const KDE_REMOTE_TIMEOUT_SECONDS: u32 = 36 * 60;
const KDE_OUTPUT_LIMIT: usize = 1024 * 1024;
const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SSH_OUTPUT_LIMIT: usize = 1024 * 1024;
const SSH_READINESS_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GUEST_PACKAGE_COMMAND_TIMEOUT: Duration = Duration::from_secs(25 * 60);
const LOGIN_PREPARE_TIMEOUT: Duration = Duration::from_secs(60);
const LOGIN_JOURNAL_TIMEOUT: Duration = Duration::from_secs(30);
const ZYPP_PROBE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const SSH_TRANSFER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KdeProvisionFailure {
    Timeout,
    Failed,
}

fn kde_failure(timed_out: bool, exit_code: Option<i32>) -> Option<KdeProvisionFailure> {
    if timed_out || exit_code == Some(124) {
        Some(KdeProvisionFailure::Timeout)
    } else if exit_code == Some(0) {
        None
    } else {
        Some(KdeProvisionFailure::Failed)
    }
}

fn bounded_output_log(output: &BoundedOutput) -> Vec<u8> {
    let mut log = Vec::with_capacity(output.stdout.len() + output.stderr.len() + 128);
    log.extend_from_slice(&output.stdout);
    if output.stdout_truncated {
        log.extend_from_slice(b"\n[stdout truncated at private capture limit]\n");
    }
    log.extend_from_slice(b"\n--- stderr ---\n");
    log.extend_from_slice(&output.stderr);
    if output.stderr_truncated {
        log.extend_from_slice(b"\n[stderr truncated at private capture limit]\n");
    }
    log
}

fn safe_login_reason(log: &[u8]) -> Option<&'static str> {
    String::from_utf8_lossy(log).lines().find_map(desktop_login_reason)
}

fn save_private_log(run: &Path, name: &str, contents: &[u8]) -> Result<()> {
    let mut file =
        fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(run.join(name))?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

fn login_journal_path(run: &Path) -> PathBuf {
    run.join(LOGIN_JOURNAL_FILENAME)
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
    fn framebuffer_has_rendered_content(&mut self) -> Result<bool>;
    fn capture(&mut self, point: LoginCapturePoint) -> Result<()>;
}

fn ppm_frame_has_rendered_content(frame: &[u8]) -> Result<bool> {
    let mut lines = frame.splitn(4, |byte| *byte == b'\n');
    require(lines.next() == Some(b"P6".as_slice()), "QEMU framebuffer is not a binary PPM")?;
    let dimensions = std::str::from_utf8(lines.next().ok_or("PPM dimensions missing")?)?;
    let mut fields = dimensions.split_ascii_whitespace();
    let width: usize = fields.next().ok_or("PPM width missing")?.parse()?;
    let height: usize = fields.next().ok_or("PPM height missing")?.parse()?;
    require(fields.next().is_none(), "invalid PPM dimensions")?;
    let maximum = lines.next().ok_or("PPM maximum color missing")?;
    require(maximum == b"255", "unsupported PPM color depth")?;
    let pixels = lines.next().ok_or("PPM pixels missing")?;
    let expected = width
        .checked_mul(height)
        .and_then(|count| count.checked_mul(3))
        .ok_or("PPM pixel count overflow")?;
    require(
        width > 0 && height > 0 && width <= 8192 && height <= 8192 && pixels.len() == expected,
        "invalid QEMU framebuffer dimensions or pixel length",
    )?;
    // A newly-started X server can have a black framebuffer with only the
    // hardware cursor drawn. Treating one nonblack pixel as readiness caused
    // us to type credentials before SDDM had painted its user chooser. Ignore
    // near-black noise and require visible content across a small fraction of
    // the framebuffer; a real greeter has large UI/text regions, while a
    // cursor does not.
    let visible_pixels = pixels
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|rgb| rgb.iter().any(|channel| *channel > 16))
        .count();
    let minimum_visible_pixels = (width * height / 1000).max(32);
    Ok(visible_pixels >= minimum_visible_pixels)
}

fn wait_for_rendered_greeter(
    control: &mut impl LoginMonitor,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if control.framebuffer_has_rendered_content()? {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        require(!remaining.is_zero(), "SDDM greeter framebuffer remained black")?;
        if poll_interval.is_zero() {
            thread::yield_now();
        } else {
            thread::sleep(poll_interval.min(remaining));
        }
    }
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

    fn framebuffer_has_rendered_content(&mut self) -> Result<bool> {
        let path = self.run.join(GREETER_READINESS_FRAME);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let result = (|| {
            self.control.screendump(GREETER_READINESS_FRAME)?;
            let metadata = fs::symlink_metadata(&path)?;
            require(
                metadata.is_file() && metadata.len() <= 32 * 1024 * 1024,
                "QEMU created an invalid private readiness framebuffer",
            )?;
            ppm_frame_has_rendered_content(&fs::read(&path)?)
        })();
        let _ = fs::remove_file(path);
        result
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
    ready_timeout: Duration,
    ready_poll: Duration,
) -> Result<()> {
    if let Err(error) = wait_for_rendered_greeter(control, ready_timeout, ready_poll) {
        capture_login_timeout(control);
        return Err(error);
    }
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
        let _ = self.terminate_owned_vm();
    }
}

impl Lab {
    fn terminate_owned_vm(&mut self) -> Result<()> {
        let Some(child) = self.vm.as_mut() else {
            self.port = None;
            return Ok(());
        };

        if child.try_wait()?.is_none()
            && let Err(kill_error) = child.kill()
        {
            // QEMU may have exited between try_wait and kill. Keep the
            // child and port owned unless that exit can be observed.
            if child.try_wait()?.is_none() {
                return Err(kill_error.into());
            }
        }
        child.wait()?;

        // Clear ownership only after wait has successfully reaped QEMU.
        self.vm = None;
        self.port = None;
        Ok(())
    }

    fn tool(&self, binary: &str) -> Command {
        runner::tool(&self.assets, binary)
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
        fs::copy(runner::firmware(&self.assets, "OVMF_VARS_4M.fd"), self.run.join("OVMF_VARS.fd"))?;
        Ok(())
    }

    fn start(&mut self, offline: bool) -> Result<()> {
        require(self.vm.is_none(), "VM already owned by run")?;
        self.port = None;
        let mut command = self.tool("qemu-system-x86_64");
        // The pinned Debian package ships some PCI devices, including
        // virtio-vga, as modules under qemu-system-common. The assets are
        // extracted into a private directory rather than installed globally.
        if !runner::WSL {
            command.env("QEMU_MODULE_DIR", self.assets.join("qemu/usr/lib/x86_64-linux-gnu/qemu"));
        }
        // QMP screendumps use fixed relative names and stay below this private
        // mode-0700 directory. All other QEMU inputs use absolute paths.
        command.current_dir(&self.run);
        command
            .arg("-L")
            .arg(runner::data(&self.assets))
            .arg("-drive")
            .arg(format!(
                "if=pflash,format=raw,readonly=on,file={}",
                runner::firmware(&self.assets, "OVMF_CODE_4M.fd").display()
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
            // virtio-vga provides the guest virtio_gpu KMS/DRM driver with a
            // render node; the legacy VGA device's bochs-drm only exposed a
            // primary node and left KWin's Wayland compositor on a black
            // framebuffer. Runtime acceptance still requires the headed
            // ordinary-user session to render successfully.
            .arg("-device")
            .arg(format!("virtio-vga,romfile={}", runner::vgabios(&self.assets).display()))
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
        self.ssh_until(request, Instant::now() + SSH_COMMAND_TIMEOUT)
    }

    fn ssh_package_command(&self, request: &str) -> Result<String> {
        self.ssh_until(request, Instant::now() + GUEST_PACKAGE_COMMAND_TIMEOUT)
    }

    fn ssh_migration_command(&self, request: &str) -> Result<String> {
        self.ssh_until(request, Instant::now() + MIGRATION_TIMEOUT)
    }

    fn ssh_until(&self, request: &str, deadline: Instant) -> Result<String> {
        let mut command = self.ssh_command()?;
        command.arg(request);
        let result = process::run_until(&mut command, deadline, SSH_OUTPUT_LIMIT)?;
        fs::write(self.run.join("guest-private.log"), bounded_output_log(&result))?;
        require(!result.timed_out, "SSH_COMMAND_TIMEOUT")?;
        require(result.status.success(), "guest command failed; inspect private guest log")?;
        require(
            !result.stdout_truncated && !result.stderr_truncated,
            "guest command output exceeded private capture limit",
        )?;
        Ok(String::from_utf8(result.stdout)?)
    }

    fn run_private_child(
        &self,
        command: &mut Command,
        log_name: &str,
        timeout_category: &'static str,
        failure_category: &'static str,
    ) -> Result<()> {
        let result = process::run(command, SSH_TRANSFER_TIMEOUT, SSH_OUTPUT_LIMIT)?;
        let log_path = self.run.join(log_name);
        fs::write(&log_path, bounded_output_log(&result))?;
        fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600))?;
        require(!result.timed_out, timeout_category)?;
        require(result.status.success(), failure_category)?;
        require(!result.stdout_truncated && !result.stderr_truncated, "SSH_TRANSFER_OUTPUT_LIMIT")?;
        Ok(())
    }

    fn kde_setup(&self, proxy: &str) -> std::result::Result<String, KdeProvisionFailure> {
        let request = format!(
            "sudo /usr/bin/timeout --signal=TERM --kill-after=10s {KDE_REMOTE_TIMEOUT_SECONDS}s /tmp/ryoku-vm-lab guest-setup {proxy}"
        );
        let mut command = self.ssh_command().map_err(|_| KdeProvisionFailure::Failed)?;
        command.arg(request);
        let output = process::run(&mut command, KDE_PROVISION_TIMEOUT, KDE_OUTPUT_LIMIT)
            .map_err(|_| KdeProvisionFailure::Failed)?;
        save_private_log(&self.run, "kde-provision-private.log", &bounded_output_log(&output))
            .map_err(|_| KdeProvisionFailure::Failed)?;
        if let Some(failure) = kde_failure(output.timed_out, output.status.code()) {
            return Err(failure);
        }
        String::from_utf8(output.stdout).map_err(|_| KdeProvisionFailure::Failed)
    }

    fn login(&mut self, password: &[u8]) -> Result<String> {
        let result = self.login_inner(password);
        if let Err(error) = &result {
            let reason = error
                .to_string()
                .lines()
                .find_map(desktop_login_reason)
                .or_else(|| {
                    fs::read(self.run.join("guest-private.log"))
                        .ok()
                        .and_then(|b| safe_login_reason(&b))
                })
                .unwrap_or("inspection_precondition_failed");
            self.evidence(&format!("desktop_login_reason={reason}"))?;
        }
        result
    }

    fn login_inner(&self, password: &[u8]) -> Result<String> {
        let mut command = self.ssh_command()?;
        command.arg(
            "sudo /usr/bin/timeout --signal=TERM --kill-after=3s 45s /tmp/ryoku-vm-lab guest-login-prepare",
        );
        // No password is placed in argv, environment, disk files or command
        // logs. Guest preparation consumes this pipe and suppresses output from
        // the native secret-bearing password command.
        let mut private_password = Vec::with_capacity(password.len() + 1);
        private_password.extend_from_slice(password);
        private_password.push(b'\n');
        let result = process::run_with_input_until(
            &mut command,
            Some(&private_password),
            Instant::now() + LOGIN_PREPARE_TIMEOUT,
            SSH_OUTPUT_LIMIT,
        );
        private_password.fill(0);
        let result = result?;
        fs::write(self.run.join("desktop-prepare-private.log"), bounded_output_log(&result))?;
        require(!result.timed_out, "DESKTOP_LOGIN_PREPARE_TIMEOUT")?;
        require(result.status.success(), "native desktop login preparation failed")?;
        require(
            !result.stdout_truncated && !result.stderr_truncated,
            "desktop login preparation output exceeded private capture limit",
        )?;
        let prepared = String::from_utf8(result.stdout)?;
        let mut control = qmp::QmpClient::connect(&self.run.join("qmp-private.sock"))?;
        let mut monitor = PrivateLoginMonitor { control: &mut control, run: &self.run };
        send_greeter_password(
            &mut monitor,
            password,
            Duration::from_millis(150),
            GREETER_READINESS_TIMEOUT,
            GREETER_READINESS_POLL,
        )?;
        let deadline = Instant::now() + Duration::from_secs(180);
        let mut last_error = String::from("login inspection has not completed");
        let mut last_reason = None;
        let mut journal_capture_attempted = false;
        while Instant::now() < deadline {
            let inspected = self.ssh_until("sudo /tmp/ryoku-vm-lab guest-login-inspect", deadline);
            if inspected.is_err()
                && let Ok(diagnostic) = fs::read(self.run.join("guest-private.log"))
            {
                last_reason = safe_login_reason(&diagnostic).or(last_reason);
            }
            let inspected = if !journal_capture_attempted && inspected.is_err() {
                journal_capture_attempted = true;
                retain_probe_result(inspected, || self.capture_login_journal(deadline))
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
        let reason = last_reason.unwrap_or("inspection_timeout");
        Err(format!(
            "desktop_login_reason={reason}\nactual SDDM login and active Plasma session were not verified; last inspection: {last_error}"
        )
        .into())
    }

    fn capture_login_journal(&self, deadline: Instant) -> Result<()> {
        let mut command = self.ssh_command()?;
        command.arg(LOGIN_JOURNAL_COMMAND);
        let journal_deadline = deadline.min(Instant::now() + LOGIN_JOURNAL_TIMEOUT);
        let result =
            process::run_until(&mut command, journal_deadline, LOGIN_JOURNAL_STREAM_LIMIT)?;
        save_private_login_journal(
            &self.run,
            &result.stdout,
            result.stdout_truncated,
            &result.stderr,
            result.stderr_truncated,
        )?;
        require(!result.timed_out, "LOGIN_JOURNAL_TIMEOUT")?;
        require(result.status.success(), "bounded private login journal capture failed")?;
        if result.stdout_truncated || result.stderr_truncated {
            return Err("private login journal output exceeded the per-stream byte cap".into());
        }
        Ok(())
    }

    fn wait_ssh(&mut self) -> Result<()> {
        let deadline = Instant::now() + SSH_READINESS_TIMEOUT;
        loop {
            if let Some(child) = &mut self.vm {
                require(child.try_wait()?.is_none(), "QEMU exited before SSH")?;
            }
            if Instant::now() >= deadline {
                break;
            }
            if self.ssh_until("timeout 120 cloud-init status --wait", deadline).is_ok() {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(Duration::from_secs(3)));
        }
        Err("SSH_READINESS_TIMEOUT".into())
    }

    fn copy_binary(&self) -> Result<()> {
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        let mut command = Command::new("scp");
        command
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
            .arg("tester@127.0.0.1:/tmp/ryoku-vm-lab");
        self.run_private_child(
            &mut command,
            "binary-copy-private.log",
            "SSH_BINARY_COPY_TIMEOUT",
            "SSH_BINARY_COPY_FAILED",
        )
    }

    fn copy_zypp_probe(&self) -> Result<()> {
        let binary = self.zypp_probe.as_ref().ok_or("no zypper probe binary configured")?;
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        let mut command = Command::new("scp");
        command
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
            .arg("tester@127.0.0.1:/tmp/ryoku-zypp-hook-probe.upload");
        self.run_private_child(
            &mut command,
            "zypp-probe-copy-private.log",
            "SSH_ZYPP_PROBE_COPY_TIMEOUT",
            "SSH_ZYPP_PROBE_COPY_FAILED",
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
        let mut command = self.ssh_command()?;
        command.arg(&request);
        let result = process::run(&mut command, ZYPP_PROBE_TIMEOUT, SSH_OUTPUT_LIMIT)?;
        let error_category = classify_zypp_error(&result.stderr);
        let private_log = self.run.join(log_name);
        fs::write(&private_log, bounded_output_log(&result))?;
        fs::set_permissions(&private_log, fs::Permissions::from_mode(0o600))?;
        let label_storage = mode.replace('-', "_");
        let label = if mode == "inventory" { "inventory" } else { label_storage.as_str() };
        if result.timed_out || result.stdout_truncated || result.stderr_truncated {
            let (category, code, outcome) = if result.timed_out {
                ("timeout", 124, "timeout")
            } else {
                ("output_limit", result.status.code().unwrap_or(255), "output_limit")
            };
            self.evidence(&format!("zypp_{label}_exit_code={code}"))?;
            self.evidence(&format!("zypp_{label}_error_category={category}"))?;
            return Ok((false, outcome.to_owned()));
        }
        let stdout = String::from_utf8(result.stdout)?;
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
            let safe_value = safe_zypp_probe_value(key, value);
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
                    "inventory_cockpit_socket_enabled"
                    | "inventory_cockpit_socket_active"
                    | "inventory_cockpit_socket_listen_scope"
                    | "inventory_host_tcp_listener_scope" => {
                        self.evidence(&format!("zypp_{key}={value}"))?;
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
                                if let Some(kind) = fields.get(1).copied().filter(|kind| {
                                    ["missing", "regular-file", "directory", "symlink", "other"]
                                        .contains(kind)
                                }) {
                                    self.evidence(&format!(
                                        "zypp_inventory_kind_{current}={kind}"
                                    ))?;
                                }
                                if let Some(target) = fields
                                    .iter()
                                    .find_map(|field| field.strip_prefix("target="))
                                    .filter(|target| !target.is_empty())
                                {
                                    let normalized_target =
                                        target.trim_matches('/').replace(['/', '.'], "_");
                                    if normalized_target.bytes().all(|byte| {
                                        byte.is_ascii_alphanumeric() || b"_+-".contains(&byte)
                                    }) {
                                        self.evidence(&format!(
                                            "zypp_inventory_symlink_target_{current}={normalized_target}"
                                        ))?;
                                    }
                                }
                                if let Some(acl) =
                                    fields.iter().find_map(|field| field.strip_prefix("acl="))
                                {
                                    let (status, digest) = acl.split_once(':').unwrap_or((acl, ""));
                                    self.evidence(&format!(
                                        "zypp_inventory_acl_{current}={status}"
                                    ))?;
                                    if digest.len() == 64
                                        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                                    {
                                        self.evidence(&format!(
                                            "zypp_inventory_acl_sha256_{current}={digest}"
                                        ))?;
                                    }
                                }
                                for (source, label) in [
                                    ("xattr.security.selinux=", "selinux"),
                                    ("xattr.security.capability=", "capability"),
                                ] {
                                    if let Some(attribute) =
                                        fields.iter().find_map(|field| field.strip_prefix(source))
                                    {
                                        let (status, digest) =
                                            attribute.split_once(':').unwrap_or((attribute, ""));
                                        self.evidence(&format!(
                                            "zypp_inventory_xattr_{label}_{current}={status}"
                                        ))?;
                                        if digest.len() == 64
                                            && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                                        {
                                            self.evidence(&format!(
                                                "zypp_inventory_xattr_{label}_sha256_{current}={digest}"
                                            ))?;
                                        }
                                    }
                                }
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
        self.evidence(&format!("zypp_{label}_error_category={error_category}"))?;
        Ok((passed, outcome.unwrap_or("no_outcome").to_owned()))
    }

    fn copy_ui_binary(&self) -> Result<()> {
        let Some(binary) = &self.ui_binary else {
            return Ok(());
        };
        let port = self.port.ok_or("owned QEMU SSH forwarding endpoint unknown")?;
        let mut command = Command::new("scp");
        command
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
            .arg("tester@127.0.0.1:/tmp/ryoku-ui-gtk.upload");
        self.run_private_child(
            &mut command,
            "ui-binary-copy-private.log",
            "SSH_UI_BINARY_COPY_TIMEOUT",
            "SSH_UI_BINARY_COPY_FAILED",
        )?;
        self.ssh_package_command("sudo /tmp/ryoku-vm-lab guest-ui-install")?;
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
            "#cloud-config\nusers:\n  - name: tester\n    groups: [wheel]\n    sudo: 'ALL=(ALL) NOPASSWD:ALL'\n    shell: /bin/bash\n    lock_passwd: true\n    ssh_authorized_keys:\n      - {public}\ndisable_root: true\nssh_pwauth: false\nwrite_files:\n"
        );
        if !runner::WSL {
            config.push_str("  - path: /etc/pki/trust/anchors/cloud-lab.crt\n    content: |\n");
            for entry in fs::read_dir("/usr/local/share/ca-certificates")? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "crt") {
                    for line in fs::read_to_string(path)?.lines() {
                        config.push_str(&format!("      {line}\n"));
                    }
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

fn safe_zypp_probe_value(key: &str, value: &str) -> bool {
    if key == "inventory_path" {
        return safe_inventory_path_value(value);
    }

    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.+-:|/@".contains(&byte))
}

/// Reduce provider stderr to a small public category. Raw stderr remains in
/// the permission-restricted private run log and is never copied into evidence.
fn classify_zypp_error(stderr: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    // Order specific signatures before broad words such as "repository" and
    // "plugin". The returned value is always a fixed label; input text is
    // never copied into public evidence.
    if text.contains("permission denied") || text.contains("not authorized") {
        "authorization"
    } else if text.contains("could not get lock") || text.contains("another package manager") {
        "package_manager_lock"
    } else if text.contains("no provider") || text.contains("not found") {
        "package_resolution"
    } else if text.contains("failed to download")
        || text.contains("could not resolve host")
        || text.contains("connection timed out")
    {
        "network_or_download"
    } else if text.contains("no repositories defined")
        || text.contains("no repository defined")
        || text.contains("repository is not defined")
    {
        "repository_configuration"
    } else if text.contains("repository") || text.contains("metadata") {
        "repository_or_metadata"
    } else if text.contains("plugin") {
        "plugin"
    } else if stderr.is_empty() {
        "no_stderr"
    } else {
        "unclassified"
    }
}

fn safe_inventory_path_value(value: &str) -> bool {
    let mut fields = value.split('|');
    let Some(path) = fields.next() else { return false };
    if path.is_empty()
        || !path.starts_with('/')
        || !path.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/_.+-".contains(&byte))
    {
        return false;
    }
    let Some(kind) = fields.next() else { return false };
    if !["missing", "regular-file", "directory", "symlink", "other"].contains(&kind) {
        return false;
    }

    let rest = fields.collect::<Vec<_>>();
    if kind == "missing" {
        return matches!(rest.as_slice(), [owner] if owner.strip_prefix("owner=").is_some_and(safe_inventory_owner));
    }
    if rest.len() != 9 {
        return false;
    }

    let mut mode = false;
    let mut uid = false;
    let mut gid = false;
    let mut mtime = false;
    let mut target = false;
    let mut owner = false;
    let mut acl = false;
    let mut selinux = false;
    let mut capability = false;
    for field in rest {
        let Some((key, value)) = field.split_once('=') else { return false };
        let valid = match key {
            "mode" if !mode => {
                mode = true;
                !value.is_empty() && value.bytes().all(|byte| (b'0'..=b'7').contains(&byte))
            }
            "uid" if !uid => {
                uid = true;
                safe_decimal(value)
            }
            "gid" if !gid => {
                gid = true;
                safe_decimal(value)
            }
            "mtime_ns" if !mtime => {
                mtime = true;
                let digits = value.strip_prefix('-').unwrap_or(value);
                safe_decimal(digits)
            }
            "target" if !target => {
                target = true;
                value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/_.+-".contains(&byte))
            }
            "owner" if !owner => {
                owner = true;
                safe_inventory_owner(value)
            }
            "acl" if !acl => {
                acl = true;
                safe_inventory_digest_status(value)
            }
            "xattr.security.selinux" if !selinux => {
                selinux = true;
                safe_inventory_digest_status(value)
            }
            "xattr.security.capability" if !capability => {
                capability = true;
                safe_inventory_digest_status(value)
            }
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    mode && uid && gid && mtime && target && owner && acl && selinux && capability
}

fn safe_decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn safe_inventory_owner(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.+-:@".contains(&byte))
}

fn safe_inventory_digest_status(value: &str) -> bool {
    match value.split_once(':') {
        None => ["absent", "unavailable"].contains(&value),
        Some(("present", digest)) => {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }
        _ => false,
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
                    && package.as_bytes()[0].is_ascii_alphanumeric()
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
    runner::validate_host(&assets, &parent)?;
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
        let digest = output(Command::new("sha256sum").arg(runner::firmware(&assets, file)))?;
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
    let (network, offline_probe) = if runner::WSL {
        ("direct".to_owned(), "1.1.1.1:443".to_owned())
    } else {
        let address =
            ("proxy", 8080).to_socket_addrs()?.next().ok_or("managed proxy DNS missing")?;
        (address.to_string(), address.to_string())
    };
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
    let runner_name = if runner::WSL { "existing-opensuse-wsl" } else { "existing-managed-cloud" };
    lab.evidence(&format!("fixture_sha256={hash}\nrelease_signer={signer}\nhost_runner={runner_name}\naccelerator=tcg\nvm_memory_mib={memory}\nvm_cpus={cpus}"))?;
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
    let migrated =
        lab.ssh_migration_command(&format!("sudo /tmp/ryoku-vm-lab guest-migrate {network}"))?;
    lab.evidence(&migrated)?;
    lab.stop()?;
    fs::rename(lab.run.join("disk.qcow2"), lab.run.join("source-overlay.qcow2"))?;
    fs::rename(lab.run.join("migration.qcow2"), lab.run.join("disk.qcow2"))?;
    fs::copy(runner::firmware(&lab.assets, "OVMF_VARS_4M.fd"), lab.run.join("OVMF_VARS.fd"))?;
    lab.start(false)?;
    lab.copy_binary()?;
    let base = lab.ssh("sudo /tmp/ryoku-vm-lab guest-base-setup")?;
    lab.evidence(&base)?;
    lab.evidence("derived_btrfs_boot=passed")?;
    // The migrated disk is standalone and has now booted without the XFS
    // source attached. Retain the signed base for the later reset check,
    // but release the disposable migration source before desktop setup.
    fs::remove_file(lab.run.join("source-overlay.qcow2"))?;
    lab.evidence("migration_xfs_overlay=removed")?;
    let kde_installed = match lab.kde_setup(&network) {
        Ok(setup) => {
            lab.evidence(&setup)?;
            true
        }
        Err(failure) => {
            lab.evidence(match failure {
                KdeProvisionFailure::Timeout => "kde_provisioning=timeout",
                KdeProvisionFailure::Failed => "kde_provisioning=blocked",
            })?;
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
                            let private = fs::read_to_string(lab.run.join(log)).unwrap_or_default();
                            let classification = if !private
                                .lines()
                                .any(|line| line == "plugin_commitbegin_seen=true")
                            {
                                "awaiting_proof"
                            } else {
                                "failed_probe"
                            };
                            lab.evidence(&format!(
                                "zypp_{}_gate={classification}",
                                mode.replace('-', "_")
                            ))?;
                            // A failed denial may have changed RPM state; do
                            // not issue more package transactions in that case.
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
                        lab.evidence(&format!(
                            "zypp_{}_gate=probe_failed",
                            mode.replace('-', "_")
                        ))?;
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
        lab.terminate_owned_vm()?;
        lab.evidence("offline_recovery=skipped_provider_probe_overlay_discarded")?;
    } else {
        lab.stop()?;
        lab.start(true)?;
        lab.copy_binary()?;
        let recovered =
            lab.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-recover {offline_probe}"))?;
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
    if zypp_probe.is_some() {
        lab.evidence("lifecycle_result=partial")?;
        if let Some(result) = optional_ui_result(lab.ui_binary.is_some(), ui_captured) {
            lab.evidence(result)?;
        }
        lab.evidence("lab_result=partial")?;
        return Ok(());
    }
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

    #[test]
    fn login_reason_extraction_accepts_only_static_allowlisted_categories() {
        assert_eq!(
            safe_login_reason(
                b"ordinary diagnostic\ndesktop_login_reason=kwin_absent\nraw error detail"
            ),
            Some("kwin_absent")
        );
        assert_eq!(safe_login_reason(b"desktop_login_reason=private-session-id\n"), None);
        assert_eq!(
            safe_login_reason(b"desktop_login_reason=active_session_absent_or_mismatch extra\n"),
            None
        );
    }

    #[test]
    fn kde_diagnostics_survive_later_ssh_output_in_a_private_stage_log() {
        let run = std::env::temp_dir().join(format!(
            "ryoku-kde-log-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
        let diagnostic = b"zypper timed out\nprivate repository detail\n";
        save_private_log(&run, "kde-provision-private.log", diagnostic).unwrap();
        fs::write(run.join("guest-private.log"), b"later stop SSH output\n").unwrap();

        let path = run.join("kde-provision-private.log");
        assert_eq!(fs::read(&path).unwrap(), diagnostic);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(save_private_log(&run, "kde-provision-private.log", b"replacement").is_err());
        fs::remove_dir_all(run).unwrap();
    }

    #[test]
    fn zypp_error_categories_are_allowlisted_and_never_return_source_text() {
        assert_eq!(classify_zypp_error(b"Permission denied: private path"), "authorization");
        assert_eq!(
            classify_zypp_error(b"Could not get lock /secret; another package manager is running"),
            "package_manager_lock"
        );
        assert_eq!(
            classify_zypp_error(b"No provider of package secret-name"),
            "package_resolution"
        );
        assert_eq!(
            classify_zypp_error(b"Failed to download https://user:pass@example.invalid/private"),
            "network_or_download"
        );
        assert_eq!(
            classify_zypp_error(b"No repositories defined in /private/config"),
            "repository_configuration"
        );
        assert_eq!(
            classify_zypp_error(b"Repository metadata expired at /private/cache"),
            "repository_or_metadata"
        );
        assert_eq!(classify_zypp_error(b"commit plugin failed for private transaction"), "plugin");
        assert_eq!(classify_zypp_error(b"private implementation detail"), "unclassified");
        assert_eq!(classify_zypp_error(b""), "no_stderr");
    }

    #[test]
    fn zypp_inventory_accepts_structured_metadata_but_rejects_untrusted_fields() {
        let record = "/etc/zypp/zypper.conf|regular-file|mode=644|uid=0|gid=0|mtime_ns=0|target=|owner=zypper@0:1.2-1.noarch|acl=absent|xattr.security.selinux=absent|xattr.security.capability=unavailable";
        assert!(safe_zypp_probe_value("inventory_path", record));
        assert!(safe_zypp_probe_value(
            "inventory_path",
            "/etc/zypp/missing.conf|missing|owner=UNOWNED"
        ));
        assert!(!safe_zypp_probe_value(
            "inventory_path",
            &record.replace("mode=644", "mode=644;touch")
        ));
        assert!(!safe_zypp_probe_value(
            "inventory_path",
            &record.replace("target=", "target=$(id)")
        ));
        assert!(!safe_zypp_probe_value(
            "inventory_path",
            &record.replace("uid=0", "uid=0|secret=private")
        ));
        assert!(!safe_zypp_probe_value(
            "inventory_path",
            &record.replace("acl=absent", "acl=present:not-a-digest")
        ));
    }

    #[test]
    fn zypp_probe_scalar_grammar_stays_restricted() {
        assert!(safe_zypp_probe_value("outcome", "denied"));
        assert!(!safe_zypp_probe_value("outcome", "denied=unexpected"));
    }

    #[derive(Default)]
    struct RecordingLoginMonitor {
        events: Vec<String>,
        readiness_frames: Vec<Vec<u8>>,
        fail_captures: bool,
    }

    fn ppm_pixel(rgb: [u8; 3]) -> Vec<u8> {
        [b"P6\n1 1\n255\n".as_slice(), rgb.as_slice()].concat()
    }

    fn ppm_solid(width: usize, height: usize, rgb: [u8; 3]) -> Vec<u8> {
        let mut frame = format!("P6\n{width} {height}\n255\n").into_bytes();
        frame.extend(std::iter::repeat_n(rgb, width * height).flatten());
        frame
    }

    impl LoginMonitor for RecordingLoginMonitor {
        fn send_key(&mut self, qcode: &str) -> Result<()> {
            self.events.push(format!("key:{qcode}"));
            Ok(())
        }

        fn framebuffer_has_rendered_content(&mut self) -> Result<bool> {
            let rendered = self
                .readiness_frames
                .first()
                .map(|frame| ppm_frame_has_rendered_content(frame))
                .transpose()?
                .unwrap_or(false);
            if !self.readiness_frames.is_empty() {
                self.readiness_frames.remove(0);
            }
            self.events.push(if rendered { "frame:rendered" } else { "frame:black" }.into());
            Ok(rendered)
        }

        fn capture(&mut self, point: LoginCapturePoint) -> Result<()> {
            self.events.push(format!("capture:{}", point.filename()));
            if self.fail_captures { Err("mock screenshot failure".into()) } else { Ok(()) }
        }
    }

    #[test]
    fn greeter_readiness_ignores_a_cursor_on_an_otherwise_black_frame() {
        let mut frame = ppm_solid(1280, 800, [0, 0, 0]);
        // A small white cursor is visible but does not mean SDDM has rendered.
        let pixel_start = b"P6\n1280 800\n255\n".len();
        frame[pixel_start..pixel_start + 3].copy_from_slice(&[255, 255, 255]);
        assert!(!ppm_frame_has_rendered_content(&frame).unwrap());
    }

    #[test]
    fn greeter_readiness_accepts_a_frame_with_substantial_ui_content() {
        assert!(ppm_frame_has_rendered_content(&ppm_solid(1280, 800, [24, 24, 24])).unwrap());
    }

    #[test]
    fn login_waits_through_black_frame_before_capturing_and_typing() {
        let mut monitor = RecordingLoginMonitor {
            readiness_frames: vec![ppm_pixel([0, 0, 0]), ppm_solid(128, 64, [32, 32, 32])],
            fail_captures: true,
            ..Default::default()
        };
        send_greeter_password(
            &mut monitor,
            b"ab2",
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .unwrap();
        capture_login_timeout(&mut monitor);

        assert_eq!(
            monitor.events,
            [
                "frame:black",
                "frame:rendered",
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
    fn all_black_greeter_frame_times_out_without_sending_credentials() {
        let mut monitor = RecordingLoginMonitor {
            readiness_frames: vec![ppm_pixel([0, 0, 0])],
            fail_captures: true,
            ..Default::default()
        };
        let result = send_greeter_password(
            &mut monitor,
            b"ab2",
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        );

        assert!(result.is_err());
        assert_eq!(monitor.events, ["frame:black", "capture:sddm-login-timeout.ppm"]);
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
    fn kde_timeout_classification_accepts_only_timeout_status_or_host_deadline() {
        assert_eq!(kde_failure(true, None), Some(KdeProvisionFailure::Timeout));
        assert_eq!(kde_failure(false, Some(124)), Some(KdeProvisionFailure::Timeout));
        assert_eq!(kde_failure(false, Some(1)), Some(KdeProvisionFailure::Failed));
        assert_eq!(kde_failure(false, Some(0)), None);
    }

    #[test]
    fn kde_setup_deadlines_nest_around_the_guest_zypper_deadline() {
        assert_eq!(KDE_REMOTE_TIMEOUT_SECONDS, 36 * 60);
        assert_eq!(KDE_PROVISION_TIMEOUT, Duration::from_secs(38 * 60));
        assert!(Duration::from_secs(u64::from(KDE_REMOTE_TIMEOUT_SECONDS)) < KDE_PROVISION_TIMEOUT);
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
}
