//! Optional linked-partner run. The issue19 fixture also exercises root rollback.
use super::*;

const READBACK: &[&str] = &[
    "native_linked_catalog_generation_readback=passed",
    "native_linked_exact_snapshot_readback=passed",
    "native_linked_planned_journal_readback=passed",
];
const LIMITS: &[&str] =
    &["native_linked_boot_compatibility=unproven", "native_linked_restore_execution=unproven"];

fn validate_probe(mode: &str, text: &str) -> Result<()> {
    let mut lines: Vec<_> = text.lines().collect();
    let expected: Vec<&str> = match mode {
        "--coordination-capture" => [
            &[
                "native_issue19_full_root_boot_files=passed",
                "native_issue19_esp_coverage=excluded",
                "native_issue19_configuration_only=passed",
                "native_issue19_unsupported_layout=passed",
                "native_issue19_package_hook_pair=passed",
                "native_issue19_no_nested_pair=passed",
                "native_issue19_additional_catalog=passed",
                "native_issue19_manual_pin=passed",
                "native_issue19_missing_additional=passed",
                "native_issue19_missing_primary_partners=passed",
                "native_issue19_catalog_corruption_refused=passed",
            ][..],
            LIMITS,
        ]
        .concat(),
        "--coordination-after-boot" => [
            &[
                "native_issue19_catalog_after_rollback=passed",
                "native_issue19_known_working_pin=passed",
                "native_issue19_cleanup_ownership=passed",
                "native_issue19_deleted_partner=passed",
            ][..],
            LIMITS,
        ]
        .concat(),
        "--capture" => {
            let number = lines
                .get(1)
                .and_then(|line| line.strip_prefix("native_linked_snapshot_id="))
                .ok_or("missing linked snapshot number")?;
            require(
                number.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == number),
                "invalid linked snapshot number",
            )?;
            lines.remove(1);
            [
                vec![
                    "native_linked_duplicate_checkpoint_refused=passed",
                    "native_linked_original_identity_recorded=passed",
                ],
                READBACK.to_vec(),
                LIMITS.to_vec(),
            ]
            .concat()
        }
        "--verify" => [READBACK, LIMITS].concat(),
        "--verify-after-root-change" => {
            [&["native_linked_changed_root_historical_readback=passed"][..], READBACK, LIMITS]
                .concat()
        }
        "--prepare-root-rollback" => {
            let index = READBACK.len();
            let number = lines
                .get(index)
                .and_then(|line| line.strip_prefix("native_linked_root_rollback_target="))
                .ok_or("missing rollback target")?;
            require(
                number.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == number),
                "invalid rollback target",
            )?;
            lines.remove(index);
            [READBACK, LIMITS].concat()
        }
        "--exercise-failures" => [
            READBACK,
            &[
                "native_linked_writable_snapshot_refused=passed",
                "native_linked_missing_snapshot_refused=passed",
                "native_linked_substituted_snapshot_refused=passed",
                "native_linked_missing_generation_refused=passed",
            ],
            READBACK,
            LIMITS,
        ]
        .concat(),
        "--cleanup-journal" => {
            [READBACK, &["native_linked_owning_journal_cleanup=passed"], LIMITS].concat()
        }
        "--observe-layout" => {
            let facts = lines.iter().take_while(|line| line.starts_with("boot_fact=")).count();
            require(facts == 12, "incomplete private boot observation")?;
            lines.drain(..facts);
            [
                vec![
                    "native_linked_layout_observed=recorded",
                    "native_linked_root_switch=unproven",
                ],
                LIMITS.to_vec(),
            ]
            .concat()
        }
        _ => return Err("unsupported linked probe mode".into()),
    };
    require(lines == expected, "incomplete or unexpected linked probe result")
}

fn validate_cli_link(text: &str) -> Result<()> {
    let lines: Vec<_> = text.lines().collect();
    require(lines.len() == 2, "unexpected verify-linked output")?;
    let number = lines[0].strip_prefix("partners-verified transaction=issue19-linked-probe generation=issue19-linked-generation snapshot=root:")
        .ok_or("verify-linked did not name exact fixture partners")?;
    require(
        number.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == number),
        "invalid CLI snapshot number",
    )?;
    require(
        lines[1] == "Boot compatibility and restore readiness are not assessed",
        "CLI overstates proof scope",
    )
}

impl Lab {
    fn linked_command(&self, command: &str, log: &str) -> Result<String> {
        let result = self.ssh(command);
        save_private_log(&self.run, log, &fs::read(self.run.join("guest-private.log"))?)?;
        result
    }

    fn linked_probe_mode(&mut self, mode: &str, log: &str) -> Result<()> {
        let result =
            self.linked_command(&format!("sudo /usr/local/sbin/ryoku-linked-probe {mode}"), log)?;
        validate_probe(mode, &result)?;
        self.evidence(&result)
    }

    pub(super) fn install_linked_input(&self, binary: &Path, name: &str) -> Result<()> {
        require(
            matches!(name, "ryoku-linked-probe" | "ryoku-recovery")
                || (cfg!(feature = "issue20-lab")
                    && matches!(name, "ryoku-mutation-probe" | "ryoku-mutation-history")),
            "invalid linked binary target",
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
            .arg(binary)
            .arg(format!("tester@127.0.0.1:/tmp/{name}.upload"));
        self.run_private_child(
            &mut copy,
            &format!("{name}-copy-private.log"),
            "LINKED_COPY_TIMEOUT",
            "LINKED_COPY_FAILED",
        )?;
        self.ssh(&format!("sudo install -m 0755 /tmp/{name}.upload /usr/local/sbin/{name}"))?;
        let local = output(Command::new("sha256sum").arg(binary))?;
        let remote = self.ssh(&format!("sha256sum /usr/local/sbin/{name}"))?;
        require(
            local.split_whitespace().next() == remote.split_whitespace().next(),
            "installed linked binary digest mismatch",
        )
    }

    fn linked_cli(&mut self, phase: &str) -> Result<()> {
        let pending = self.linked_command(
            "sudo /usr/local/sbin/ryoku-recovery pending",
            &format!("linked-{phase}-pending-private.log"),
        )?;
        require(
            pending.trim() == "issue19-linked-journal Planned",
            "CLI pending did not report exact owned planned journal",
        )?;
        let verified = self.linked_command(
            "sudo /usr/local/sbin/ryoku-recovery verify-linked issue19-linked-probe",
            &format!("linked-{phase}-verify-cli-private.log"),
        )?;
        validate_cli_link(&verified)?;
        self.evidence("native_linked_cli_pending=passed\nnative_linked_cli_verify=passed")
    }

    pub(super) fn linked_capture(&mut self) -> Result<()> {
        self.install_linked_input(
            self.linked_probe.as_ref().ok_or("missing linked probe")?,
            "ryoku-linked-probe",
        )?;
        self.install_linked_input(
            self.recovery_cli.as_ref().ok_or("missing recovery CLI")?,
            "ryoku-recovery",
        )?;
        self.linked_probe_mode("--observe-layout", "linked-layout-before-private.log")?;
        self.linked_probe_mode("--capture", "linked-capture-private.log")?;
        if cfg!(feature = "issue19-lab") {
            self.linked_probe_mode("--coordination-capture", "coordination-capture-private.log")?;
        }
        self.linked_cli("before")?;
        self.linked_probe_mode("--exercise-failures", "linked-failures-private.log")?;
        self.linked_probe_mode("--verify", "linked-verify-private.log")
    }

    pub(super) fn linked_after_reboot(&mut self) -> Result<()> {
        self.linked_probe_mode("--observe-layout", "linked-layout-after-private.log")?;
        self.linked_probe_mode(
            if cfg!(feature = "issue19-lab") { "--verify-after-root-change" } else { "--verify" },
            "linked-after-reboot-private.log",
        )?;
        self.linked_cli("after")?;
        self.evidence(if cfg!(feature = "issue19-lab") {
            "native_linked_root_rollback_readback=passed"
        } else {
            "native_linked_normal_reboot_readback=passed"
        })?;
        // The initializer correctly refuses pending recovery. Remove only this
        // fixture's exact verified Planned journal before its healthy check.
        self.linked_probe_mode("--cleanup-journal", "linked-cleanup-private.log")?;
        let empty = self.linked_command(
            "sudo /usr/local/sbin/ryoku-recovery pending",
            "linked-empty-pending-private.log",
        )?;
        require(
            empty.trim() == "No unfinished recovery attempts",
            "CLI pending not empty after owning fixture cleanup",
        )?;
        self.evidence("native_linked_cli_empty_after_cleanup=passed")?;
        if cfg!(feature = "issue19-lab") {
            self.linked_probe_mode(
                "--coordination-after-boot",
                "coordination-after-boot-private.log",
            )?;
        }
        Ok(())
    }

    pub(super) fn linked_prepare_rollback(&mut self) -> Result<()> {
        self.linked_probe_mode("--prepare-root-rollback", "linked-rollback-private.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_reject_missing_duplicate_fabricated_scope_and_wrong_cli_partner() {
        let good = [READBACK, LIMITS].concat().join("\n");
        validate_probe("--verify", &good).unwrap();
        for bad in [
            good.replace("catalog_generation_readback", "missing"),
            good.clone() + "\nnative_linked_exact_snapshot_readback=passed",
            good.replace("unproven", "passed"),
        ] {
            assert!(validate_probe("--verify", &bad).is_err());
        }
        let cli = "partners-verified transaction=issue19-linked-probe generation=issue19-linked-generation snapshot=root:42\nBoot compatibility and restore readiness are not assessed\n";
        validate_cli_link(cli).unwrap();
        for bad in [
            cli.replace("root:42", "home:42"),
            cli.replace("root:42", "root:042"),
            cli.replace("issue19-linked-generation", "other-generation"),
        ] {
            assert!(validate_cli_link(&bad).is_err());
        }
    }

    #[test]
    fn rollback_readback_requires_changed_root_and_a_canonical_target() {
        let changed =
            [&["native_linked_changed_root_historical_readback=passed"][..], READBACK, LIMITS]
                .concat()
                .join("\n");
        validate_probe("--verify-after-root-change", &changed).unwrap();
        assert!(
            validate_probe("--verify-after-root-change", &[READBACK, LIMITS].concat().join("\n"))
                .is_err()
        );
        assert!(validate_probe("--verify", &changed).is_err());
        for target in ["42", "0", "042", "42\n42", "other", ""] {
            let line = format!("native_linked_root_rollback_target={target}");
            let result = [READBACK, &[line.as_str()], LIMITS].concat().join("\n");
            assert_eq!(validate_probe("--prepare-root-rollback", &result).is_ok(), target == "42");
        }
    }
}
