//! #24-only fresh derivation; normal #13/#21/#22 routes do not enter this path.
use super::*;
const RESULTS: &str = "native_issue24_clean_install=passed\nnative_issue24_repeat_install=passed\nnative_issue24_versioned_migration=passed\nnative_issue24_remove_and_reinstall=passed\nnative_issue24_verified_prechange_checkpoints=passed\nnative_issue24_interrupted_before_apply=passed\nnative_issue24_interrupted_apply_linked_restore=passed\nnative_issue24_wrong_recovery_receipt_refused=passed\nnative_issue24_user_overrides_and_unrelated=passed\nnative_issue24_distro_and_kde_preserved=passed\n";
impl Lab {
    pub(super) fn installer_exercise(&mut self) -> Result<()> {
        require(cfg!(feature = "issue24-lab"), "requires isolated installer feature")?;
        let inputs = self.installer_probe.clone().ok_or("missing installer inputs")?;
        let port = self.port.ok_or("missing owned SSH port")?;
        for (path, name) in inputs.iter().zip([
            "ryoku-native24-probe",
            "ryoku-native24-packages.tar",
            "ryoku-native24-source.txt",
        ]) {
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
                "INSTALLER_COPY_TIMEOUT",
                "INSTALLER_COPY_FAILED",
            )?;
            self.ssh(&format!("sudo install -m 0755 /tmp/{name}.upload /usr/local/sbin/{name}"))?;
            let local = output(Command::new("sha256sum").arg(path))?;
            let remote = self.ssh(&format!("sha256sum /usr/local/sbin/{name}"))?;
            require(
                local.split_whitespace().next() == remote.split_whitespace().next(),
                "installer input changed",
            )?;
            self.evidence(&format!(
                "{name}_sha256={}",
                local.split_whitespace().next().ok_or("missing checksum")?
            ))?;
        }
        let setup =
            self.ssh_package_command("sudo /usr/local/sbin/ryoku-native24-probe --install")?;
        require(
            setup.trim() == "native_issue24_bootstrap_and_dependencies=passed",
            "installer setup failed",
        )?;
        self.evidence(&setup)?;
        self.stop()?;
        self.start(true)?;
        self.copy_binary()?;
        self.ssh("sudo /usr/local/sbin/ryoku-native24-probe --arm")?;
        self.ssh("sudo systemctl start --no-block ryoku-native24-exercise.service")?;
        let deadline = Instant::now() + Duration::from_secs(35 * 60);
        let mut last = String::new();
        let mut progress = Instant::now();
        let mut result = None;
        while Instant::now() < deadline {
            let mut file = fs::File::open(self.run.join("serial-private.log"))?;
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::End(-(file.metadata()?.len().min(65536) as i64)))?;
            let mut serial = String::new();
            file.read_to_string(&mut serial)?;
            if serial.contains("native_issue24_result=failed") {
                return Err("installer guest failed; inspect private run evidence".into());
            }
            if let Some(marker) =
                serial.lines().rev().find(|line| line.contains("native_issue24_phase="))
                && marker != last
            {
                last = marker.to_owned();
                progress = Instant::now();
            }
            require(
                progress.elapsed() < Duration::from_secs(300),
                "installer guest stalled; inspect private run evidence",
            )?;
            if let Ok(report) = self.ssh("sudo /usr/local/sbin/ryoku-native24-probe --report") {
                result = Some(report);
                break;
            }
            thread::sleep(Duration::from_secs(3));
        }
        let result = result.ok_or("installer exercise deadline expired")?;
        require(result == RESULTS, "installer lifecycle evidence incomplete")?;
        self.evidence(&result)?;
        let comparisons = self.ssh("sudo cat /var/lib/ryoku-state/native24-comparisons")?;
        require(
            comparisons.lines().all(|line| {
                let fields = line.split('\t').collect::<Vec<_>>();
                fields.len() == 4
                    && fields[0].starts_with("native24-")
                    && fields[0].bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && matches!(fields[1], "pre" | "post")
                    && fields[2].parse::<usize>().is_ok_and(|count| count > 0 && count < 1000)
                    && fields[3].len() == 64
                    && fields[3].bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
            "unsafe managed comparison receipt",
        )?;
        save_private_log(&self.run, "installer-comparisons.txt", comparisons.as_bytes())?;
        let hashes = self.ssh("sudo cat /var/lib/ryoku-state/native24-package-hashes")?;
        let mut sanitized = String::new();
        for line in hashes.lines() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            require(
                fields.len() == 2
                    && fields[0].len() == 64
                    && fields[0].bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid package hash receipt",
            )?;
            let name = Path::new(fields[1])
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or("invalid package receipt name")?;
            require(
                name.starts_with("ryoku-")
                    && name.ends_with(".rpm")
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte)),
                "unsafe package receipt name",
            )?;
            sanitized.push_str(&format!("{}  {name}\n", fields[0]));
        }
        save_private_log(&self.run, "installer-package-hashes.txt", sanitized.as_bytes())?;
        self.stop()?;
        self.start(false)?;
        self.copy_binary()?;
        let report = self.ssh("sudo /usr/local/sbin/ryoku-native24-probe --report")?;
        require(report == RESULTS, "installer completed evidence changed after reboot")?;
        self.evidence("native_issue24_completed_evidence_after_reboot=passed")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_evidence_has_no_partial_acceptance() {
        assert_eq!(RESULTS.lines().count(), 10);
        assert!(!RESULTS.contains("unclaimed"));
        assert!(RESULTS.lines().all(|line| line.ends_with("=passed")));
    }
}
