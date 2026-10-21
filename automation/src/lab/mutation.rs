//! Isolated #20 native mutation fixture; generic and #19 paths never invoke it.
use super::*;
const COMMON: &[&str] = &[
    "native_issue20_first_mutation_order=passed",
    "native_issue20_console_history=passed",
    "native_issue20_provider_capabilities=disabled",
    "native_issue20_multisubvolume_atomicity=unclaimed",
];
fn validate(result: &str, exercise: bool, interrupted: bool) -> Result<()> {
    let mut expected = Vec::new();
    if exercise {
        expected.extend([
            "native_issue20_runtime_boundary=passed",
            "native_issue20_low_space_blocks=passed",
            "native_issue20_cancel_blocks=passed",
            "native_issue20_capture_error_blocks=passed",
            "native_issue20_changed_plan_blocks=passed",
            "native_issue20_changed_path_blocks=passed",
            "native_issue20_interruption_before_apply=passed",
            "native_issue20_concurrent_generations=passed",
            "native_issue20_retained_writer_lease=passed",
            if interrupted {
                "native_issue20_interrupted_apply=passed"
            } else {
                "native_issue20_post_capture_failure=passed"
            },
        ]);
    }
    expected.extend_from_slice(COMMON);
    require(
        result.lines().collect::<Vec<_>>() == expected,
        "incomplete or unexpected native mutation result",
    )
}
impl Lab {
    fn mutation_command(&mut self, command: &str, log: &str) -> Result<String> {
        let result = self.ssh(command);
        save_private_log(&self.run, log, &fs::read(self.run.join("guest-private.log"))?)?;
        result
    }
    pub(super) fn mutation_capture(&mut self, interrupted: bool) -> Result<()> {
        require(cfg!(feature = "issue20-lab"), "requires isolated issue20 feature")?;
        self.install_linked_input(
            self.mutation_probe.as_ref().ok_or("missing mutation probe")?,
            "ryoku-mutation-probe",
        )?;
        self.install_linked_input(
            self.mutation_console.as_ref().ok_or("missing mutation console")?,
            "ryoku-mutation-history",
        )?;
        let result = self.mutation_command(
            if interrupted {
                "sudo /usr/local/sbin/ryoku-mutation-probe --interrupt"
            } else {
                "sudo /usr/local/sbin/ryoku-mutation-probe --exercise"
            },
            "mutation-exercise-private.log",
        )?;
        validate(&result, true, interrupted)?;
        self.evidence(&result)
    }
    pub(super) fn mutation_after_reboot(&mut self, interrupted: bool) -> Result<()> {
        let result = self.mutation_command(
            if interrupted {
                "sudo /usr/local/sbin/ryoku-mutation-probe --verify-interrupt"
            } else {
                "sudo /usr/local/sbin/ryoku-mutation-probe --verify"
            },
            "mutation-after-reboot-private.log",
        )?;
        validate(&result, false, interrupted)?;
        self.evidence(&result)?;
        self.evidence("native_issue20_history_after_reboot=passed")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_claims_require_exact_order_and_explicit_limits() {
        let text = COMMON.join("\n");
        assert!(validate(&text, false, false).is_ok());
        assert!(validate(&text, true, false).is_err());
        assert!(validate(&text.replace("disabled", "enabled"), false, false).is_err());
        assert!(validate(&text.replace("unclaimed", "atomic"), false, false).is_err());
        assert!(validate(&format!("{text}\nextra=passed"), false, false).is_err());
    }
}
