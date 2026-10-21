//! Offline console exercise; #22 explicitly adds the destructive failure gate.
use super::*;
use std::io::{Seek, SeekFrom};

const RESULTS: &[&str] = &[
    "native_issue21_offline_cli=passed",
    "native_issue21_configuration_metadata_absence=passed",
    "native_issue21_unrelated_home=passed",
    "native_issue21_interrupted_apply_resume=passed",
    "native_issue21_pending_graphical_admission_refused=passed",
    "native_issue21_linked_root_reboot_resume=passed",
    "native_issue21_system_only_scope=passed",
    "native_issue21_current_checkpoint_retained=passed",
    "native_issue21_bad_archive_refused=passed",
    "native_issue21_unavailable_partner_refused=passed",
    "native_issue21_incompatible_packages=unclaimed",
    "native_issue21_complete_graphical_admission=passed",
    "native_issue21_provider_actions=not_exercised",
    "native_issue21_desktop_login=unclaimed",
];

fn validate(text: &str) -> Result<()> {
    let mut expected = RESULTS.to_vec();
    if cfg!(feature = "issue22-lab") {
        expected.extend(crate::recovery_gate_results::RESULTS);
    }
    require(
        text.lines().collect::<Vec<_>>() == expected,
        "incomplete or unexpected offline console results",
    )
}

fn known_operation(value: &str) -> bool {
    matches!(
        value,
        "admit-session"
            | "manual-checkpoint"
            | "verify-linked"
            | "plan"
            | "prepare-configuration"
            | "restore-configuration"
            | "prepare-linked"
            | "rollback-linked"
            | "confirm-rollback"
            | "prepare-system"
            | "rollback-system"
            | "complete-system"
            | "offline"
            | "admission-refused"
            | "interrupted-restore"
            | "refusal-cases"
    )
}

fn failed_operation(serial: &str) -> Option<&str> {
    let mut operation = "before-console";
    for line in serial.lines().map(|line| line.trim_end_matches('\r')) {
        if let Some(value) = line.strip_prefix("native_issue21_operation=") {
            if known_operation(value) {
                operation = value;
            }
        } else if line == "native_issue21_result=failed" {
            return Some(operation);
        }
    }
    None
}

fn serial_progress(serial: &str) -> Option<&str> {
    serial.lines().map(|line| line.trim_end_matches('\r')).rev().find(|line| {
        line.strip_prefix("native_issue21_operation=").is_some_and(known_operation)
            || line.strip_prefix("native_issue21_completed=").is_some_and(known_operation)
            || matches!(
                *line,
                "native_issue21_handoff=linked"
                    | "native_issue21_handoff=system"
                    | "native_issue21_result=passed"
                    | "native_issue21_result=failed"
            )
    })
}

struct ProgressWatch {
    marker: Option<String>,
    since: Instant,
}
impl ProgressWatch {
    fn stalled(&mut self, marker: Option<&str>, now: Instant) -> bool {
        if marker != self.marker.as_deref() {
            self.marker = marker.map(str::to_owned);
            self.since = now;
        }
        now.saturating_duration_since(self.since) >= Duration::from_secs(180)
    }
}

fn serial_tail(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(65_536);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(65_536).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl Lab {
    pub(super) fn console_restore_clock(&mut self) -> Result<()> {
        require(cfg!(feature = "issue21-lab"), "requires isolated issue21 fixture")?;
        let epoch = crate::console_clock::host_epoch()?;
        let before_utc = crate::console_clock::utc()?;
        let observation =
            self.ssh(&format!("sudo /tmp/ryoku-vm-lab guest-issue21-clock {epoch}"))?;
        let after_epoch = crate::console_clock::host_epoch()?;
        let record = serde_json::json!({"host_before":{"epoch":epoch,"utc":before_utc},
            "guest":serde_json::from_str::<serde_json::Value>(&observation)?,
            "host_after":{"epoch":after_epoch,"utc":crate::console_clock::utc()?}});
        save_private_log(&self.run, "console-clock-private.log", record.to_string().as_bytes())?;
        crate::console_clock::verify(&observation, epoch, after_epoch)?;
        self.evidence("native_issue21_fixture_clock=passed")
    }
    fn console_restore_diagnostics(&self) {
        if let Ok(log) =
            self.ssh("sudo /usr/bin/tail -c 131072 /var/lib/ryoku-state/native21-private.log")
        {
            let _ = save_private_log(&self.run, "console-exercise-private.log", log.as_bytes());
        }
        if let Ok(log) = self.ssh("sudo /usr/bin/journalctl --no-pager --lines=200 --unit=ryoku-native21-exercise.service --unit=ryoku-recovery-admission.service --unit=display-manager.service") {
            let _ = save_private_log(&self.run, "console-systemd-private.log", log.as_bytes());
        }
    }
    fn install_console_input(&self, input: &Path, name: &str, mode: &str) -> Result<()> {
        require(
            matches!(
                (name, mode),
                ("ryoku-native21-probe", "0755")
                    | ("ryoku-recovery", "0755")
                    | ("ryoku-native21-packages.tar", "0644")
                    | ("ryoku-native21-signing.asc", "0644")
            ),
            "unsupported console input",
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
            .arg(input)
            .arg(format!("tester@127.0.0.1:/tmp/{name}.upload"));
        self.run_private_child(
            &mut copy,
            &format!("{name}-copy-private.log"),
            "CONSOLE_COPY_TIMEOUT",
            "CONSOLE_COPY_FAILED",
        )?;
        self.ssh(&format!("sudo install -m {mode} /tmp/{name}.upload /usr/local/sbin/{name}"))?;
        let local = output(Command::new("sha256sum").arg(input))?;
        let remote = self.ssh(&format!("sha256sum /usr/local/sbin/{name}"))?;
        require(
            local.split_whitespace().next() == remote.split_whitespace().next(),
            "console input hash mismatch",
        )
    }

    pub(super) fn console_restore_exercise(&mut self) -> Result<()> {
        require(cfg!(feature = "issue21-lab"), "requires isolated issue21 feature")?;
        let inputs = self.console_restore.as_ref().ok_or("missing console inputs")?.clone();
        let cli = self.recovery_cli.as_ref().ok_or("missing console CLI")?.clone();
        for (path, name, mode, digest_key) in [
            (&inputs[0], "ryoku-native21-probe", "0755", "console_restore_probe_sha256"),
            (&inputs[1], "ryoku-native21-packages.tar", "0644", "console_packages_sha256"),
            (&inputs[2], "ryoku-native21-signing.asc", "0644", "console_signing_key_sha256"),
            (&cli, "ryoku-recovery", "0755", "recovery_cli_sha256"),
        ] {
            let digest = output(Command::new("sha256sum").arg(path))?;
            self.evidence(&format!(
                "{}={}",
                digest_key,
                digest.split_whitespace().next().ok_or("missing input digest")?
            ))?;
            self.install_console_input(path, name, mode)?;
        }
        let setup =
            self.ssh_package_command("sudo /usr/local/sbin/ryoku-native21-probe --install")?;
        require(setup.trim() == "native_issue21_fixture_installed=passed", "console setup failed")?;
        self.evidence(&setup)?;
        // Change QEMU networking before any restore. The guest has neither
        // source repositories nor a recovery-time network dependency.
        self.stop()?;
        self.start(true)?;
        self.copy_binary()?;
        self.ssh("sudo /usr/local/sbin/ryoku-native21-probe --arm")?;
        // The service survives rescue isolation and performs the reboot handoffs.
        self.ssh("sudo systemctl start --no-block ryoku-native21-exercise.service")?;
        let deadline = Instant::now() + Duration::from_secs(30 * 60);
        let mut result = None;
        let mut progress = ProgressWatch { marker: None, since: Instant::now() };
        while Instant::now() < deadline {
            let serial = serial_tail(&self.run.join("serial-private.log"))?;
            if let Some(operation) = failed_operation(&serial) {
                return Err(format!(
                    "native console fixture failed during {operation}; private disk/logs retained"
                )
                .into());
            }
            require(
                !progress.stalled(serial_progress(&serial), Instant::now()),
                "native console fixture stopped reporting progress; private disk/logs retained",
            )?;
            if let Ok(text) = self.ssh("sudo /usr/local/sbin/ryoku-native21-probe --report") {
                result = Some(text);
                break;
            }
            thread::sleep(Duration::from_secs(3));
        }
        self.console_restore_diagnostics();
        let result =
            result.ok_or("native console exercise did not finish; private logs retained")?;
        validate(&result)?;
        save_private_log(&self.run, "console-result-private.log", result.as_bytes())?;
        self.evidence(&result)?;
        self.stop()?;
        self.start(false)?;
        self.copy_binary()
    }

    pub(super) fn console_restore_after_reboot(&mut self) -> Result<()> {
        let result = self.ssh("sudo /usr/local/sbin/ryoku-native21-probe --report")?;
        validate(&result)?;
        self.evidence("native_issue21_completed_journals_after_reboot=passed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_evidence_requires_every_result_and_keeps_acceptance_limits() {
        let mut expected = RESULTS.to_vec();
        if cfg!(feature = "issue22-lab") {
            expected.extend(crate::recovery_gate_results::RESULTS);
        }
        let text = expected.join("\n");
        validate(&text).unwrap();
        for line in RESULTS {
            assert!(public_evidence(line));
        }
        assert!(!public_evidence("native_issue21_private_profile=secret"));
        assert!(
            validate(&text.replace("provider_actions=not_exercised", "provider_actions=passed"))
                .is_err()
        );
        assert!(
            validate(&text.replace("desktop_login=unclaimed", "desktop_login=passed")).is_err()
        );
        assert!(validate(&format!("{text}\nextra=passed")).is_err());
        assert!(validate(RESULTS[1..].join("\n").as_str()).is_err());
        assert!(!public_evidence("native_issue21_incompatible_packages=passed"));
        for line in crate::recovery_gate_results::RESULTS {
            assert!(public_evidence(line));
            assert!(!public_evidence(&format!("{line}-unexpected")));
        }
        assert!(!public_evidence("native_issue22_provider_writes=passed"));
        assert!(!public_evidence("native_issue22_desktop_login=passed"));
    }

    #[test]
    fn serial_failure_records_only_exact_markers_and_fixed_operations() {
        assert_eq!(
            failed_operation(
                "native_issue21_operation=manual-checkpoint\r\nnative_issue21_result=failed\r\n"
            ),
            Some("manual-checkpoint")
        );
        assert_eq!(
            failed_operation(
                "native_issue21_operation=private-data\nnative_issue21_result=failed\n"
            ),
            Some("before-console")
        );
        assert_eq!(
            failed_operation(
                "native_issue21_operation=admit-session\npartial native_issue21_result=failed\n"
            ),
            None
        );
        assert_eq!(failed_operation("native_issue21_operation=restore-configuration\n"), None);
        assert_eq!(
            serial_progress(
                "native_issue21_operation=manual-checkpoint\nnative_issue21_completed=manual-checkpoint\n"
            ),
            Some("native_issue21_completed=manual-checkpoint")
        );
        assert_eq!(serial_progress("native_issue21_completed=secret\n"), None);
        assert_eq!(
            serial_progress("native_issue21_handoff=linked\n"),
            Some("native_issue21_handoff=linked")
        );
    }

    #[test]
    fn bounded_progress_watch_requires_new_valid_events() {
        let now = Instant::now();
        let mut watch = ProgressWatch { marker: None, since: now };
        assert!(!watch.stalled(None, now + Duration::from_secs(179)));
        assert!(watch.stalled(None, now + Duration::from_secs(180)));
        let start = serial_progress("native_issue21_operation=manual-checkpoint\n");
        assert!(!watch.stalled(start, now + Duration::from_secs(181)));
        assert!(!watch.stalled(start, now + Duration::from_secs(360)));
        assert!(watch.stalled(start, now + Duration::from_secs(361)));
        assert!(!watch.stalled(
            serial_progress("native_issue21_completed=manual-checkpoint\n"),
            now + Duration::from_secs(362)
        ));
    }

    #[test]
    fn serial_tail_bounds_private_input_and_retains_terminal_marker() {
        let path = std::env::temp_dir().join(format!(
            "issue21-serial-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut file =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).unwrap();
        file.write_all(&vec![b'x'; 131_072]).unwrap();
        file.write_all(
            b"\nnative_issue21_operation=prepare-linked\nnative_issue21_result=failed\n",
        )
        .unwrap();
        drop(file);
        let tail = serial_tail(&path).unwrap();
        assert_eq!(tail.len(), 65_536);
        assert_eq!(failed_operation(&tail), Some("prepare-linked"));
        fs::remove_file(path).unwrap();
    }
}
