use crate::{
    definition::scalar,
    support::{Result, output, public_evidence, require, safe_component},
};
use std::{
    fs,
    io::{Read, Write},
    net::ToSocketAddrs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "qmp.rs"]
mod qmp;

const FIRMWARE: &[(&str, &str)] =
    &[("OVMF_CODE_4M.fd", "ovmf_code_sha256"), ("OVMF_VARS_4M.fd", "ovmf_variables_sha256")];

struct Lab {
    assets: PathBuf,
    run: PathBuf,
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
        for byte in password {
            // Password alphabet is generated here, never interpreted as QMP
            // JSON. Native virtual-key events exercise the actual SDDM greeter.
            control.send_key(std::str::from_utf8(std::slice::from_ref(byte))?)?;
            thread::sleep(Duration::from_millis(150));
        }
        control.send_key("ret")?;
        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline {
            if let Ok(inspected) = self.ssh("sudo /tmp/ryoku-vm-lab guest-login-inspect") {
                return Ok(format!("{prepared}{inspected}"));
            }
            thread::sleep(Duration::from_secs(3));
        }
        Err("actual SDDM login and active Plasma session were not verified".into())
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

pub fn run(assets: &str, run: &str) -> Result<()> {
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
    if kde_installed {
        match lab.login(&password) {
            Ok(session) => {
                lab.evidence(&session)?;
                kde_ready = true;
            }
            Err(_) => {
                lab.evidence("desktop_login=blocked")?;
            }
        }
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
            Ok(session) => lab.evidence(&session)?,
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
    lab.stop()?;
    lab.start(true)?;
    lab.copy_binary()?;
    let recovered = lab.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-recover {address}"))?;
    lab.evidence(&recovered)?;
    lab.stop()?;
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
