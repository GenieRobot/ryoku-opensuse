//! Optional profile experiment; inputs are frozen before the first VM boot.
use super::*;
use crate::profile_bundle;

impl Lab {
    fn profile_command(&self, command: &str, log: &str) -> Result<String> {
        let result = self.ssh(command);
        save_private_log(&self.run, log, &fs::read(self.run.join("guest-private.log"))?)?;
        result
    }
    fn upload_profile_input(&self, path: &Path, name: &str) -> Result<()> {
        require(
            matches!(name, "ryoku-profile-probe" | "ryoku-profile-defaults"),
            "invalid profile input target",
        )?;
        let port = self.port.ok_or("owned SSH endpoint unknown")?;
        let mut copy = Command::new("scp");
        copy.args(["-F", "/dev/null", "-i"])
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
            .arg(path)
            .arg(format!("tester@127.0.0.1:/tmp/{name}.upload"));
        self.run_private_child(
            &mut copy,
            &format!("{name}-copy-private.log"),
            "PROFILE_COPY_TIMEOUT",
            "PROFILE_COPY_FAILED",
        )?;
        Ok(())
    }
    pub(super) fn profile_bundle(&mut self) -> Result<()> {
        let preflight = self.profile_command(
            "sudo /tmp/ryoku-vm-lab guest-profile-preflight",
            "profile-preflight-private.log",
        )?;
        require(
            preflight == "native_profile_guest_prerequisites=passed\n",
            "profile guest preflight incomplete",
        )?;
        self.evidence(&preflight)?;
        let probe = self.profile_probe.as_ref().ok_or("missing profile probe")?;
        let rpm = self.defaults_rpm.as_ref().ok_or("missing default RPM")?;
        let (probe_sha, rpm_sha) =
            self.profile_hashes.as_ref().ok_or("missing frozen profile hashes")?;
        require(
            profile_bundle::digest(probe)? == *probe_sha
                && profile_bundle::digest(rpm)? == *rpm_sha,
            "profile input changed after launch freeze",
        )?;
        self.upload_profile_input(probe, "ryoku-profile-probe")?;
        self.upload_profile_input(rpm, "ryoku-profile-defaults")?;
        let result = self.profile_command(
            &format!("sudo /tmp/ryoku-vm-lab guest-profile-bundle {probe_sha} {rpm_sha}"),
            "profile-bundle-private.log",
        )?;
        let proof = result.strip_prefix("native_profile_guest_prerequisites=passed\nnative_profile_guest_artifacts=verified\nnative_profile_defaults_install=passed\n")
            .ok_or("profile import proof incomplete")?;
        profile_bundle::validate_markers(proof)?;
        self.evidence(&result)
    }
}
