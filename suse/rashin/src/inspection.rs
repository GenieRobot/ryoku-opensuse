//! Read-only Tumbleweed evidence for Fix and terminal `logs`. No caller can
//! supply a command, option, path, system unit action, or privileged journal.
use crate::{Error, Result};
use serde::Serialize;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MAX_PROBE_BYTES: usize = 32 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    UserUnit,
    UserJournal,
    RpmCapability,
    RpmFileOwner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    TimedOut,
    Truncated,
}

#[derive(Serialize)]
pub struct Observation {
    kind: ProbeKind,
    availability: Availability,
    text: String,
}

#[derive(Serialize)]
pub struct ComponentEvidence {
    component: String,
    observations: Vec<Observation>,
}

impl ComponentEvidence {
    /// Keep Fix within its prompt budget without hiding a truncated observation.
    /// The full bounded result remains available through the explicit logs API.
    pub fn prompt_summary(&self) -> String {
        serde_json::json!({"component":self.component,"observations":self.observations.iter().map(|observation| {
            let boundary = observation.text.char_indices().map(|(index, _)| index).take_while(|index| *index <= 512).last().unwrap_or(0);
            let text = if observation.text.len() <= 512 { observation.text.as_str() } else { &observation.text[..boundary] };
            serde_json::json!({"kind":observation.kind,"availability":observation.availability,"text":text,"truncated_for_prompt":text.len() != observation.text.len()})
        }).collect::<Vec<_>>()}).to_string()
    }
}

pub fn component_name(component: &str) -> Result<()> {
    if component.is_empty()
        || component.len() > 64
        || !component.as_bytes()[0].is_ascii_alphanumeric()
        || !component.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._+@-".contains(&byte))
    {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

pub fn collect(component: &str) -> Result<ComponentEvidence> {
    component_name(component)?;
    Ok(ComponentEvidence {
        component: component.into(),
        observations: [
            ProbeKind::UserUnit,
            ProbeKind::UserJournal,
            ProbeKind::RpmCapability,
            ProbeKind::RpmFileOwner,
        ]
        .into_iter()
        .map(|kind| probe(kind, component))
        .collect(),
    })
}

fn command(kind: ProbeKind, component: &str) -> Command {
    let mut command = match kind {
        ProbeKind::UserUnit => {
            let mut command = Command::new("/usr/bin/systemctl");
            command.args([
                "--user",
                "--no-pager",
                "show",
                "--property=Id,ActiveState,SubState,Result",
                "--",
                component,
            ]);
            command
        }
        ProbeKind::UserJournal => {
            let mut command = Command::new("/usr/bin/journalctl");
            command.args([
                "--user",
                "--no-pager",
                "--quiet",
                "--output=json",
                "--lines=40",
                "--unit",
                component,
            ]);
            command
        }
        ProbeKind::RpmCapability => {
            let mut command = Command::new("/usr/bin/rpm");
            command.args(["--query", "--whatprovides", "--", component]);
            command
        }
        ProbeKind::RpmFileOwner => {
            let path = if component.ends_with(".service")
                || component.ends_with(".socket")
                || component.ends_with(".target")
            {
                format!("/usr/lib/systemd/user/{component}")
            } else {
                format!("/usr/bin/{component}")
            };
            let mut command = Command::new("/usr/bin/rpm");
            command.args(["--query", "--file", "--", &path]);
            command
        }
    };
    // User-bus selection comes from kernel identity, not request/environment.
    command
        .env_clear()
        .env("LANG", "C")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", rustix::process::geteuid().as_raw()));
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    command
}

fn probe(kind: ProbeKind, component: &str) -> Observation {
    probe_command(kind, command(kind, component))
}

fn probe_command(kind: ProbeKind, mut command: Command) -> Observation {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            return Observation {
                kind,
                availability: Availability::Unavailable,
                text: String::new(),
            };
        }
    };
    let result = (|| {
        let mut stdout = child.stdout.take().ok_or(Error::ProviderUnavailable)?;
        rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::ProviderUnavailable)?;
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let mut bytes = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Ok((Availability::TimedOut, bytes));
            }
            let mut chunk = [0; 4096];
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let remaining = MAX_PROBE_BYTES.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..count.min(remaining)]);
                    if count > remaining {
                        return Ok((Availability::Truncated, bytes));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err(Error::ProviderUnavailable),
            }
        }
        loop {
            if Instant::now() >= deadline {
                return Ok((Availability::TimedOut, bytes));
            }
            if let Some(status) = child.try_wait().map_err(|_| Error::ProviderUnavailable)? {
                return Ok((
                    if status.success() {
                        Availability::Available
                    } else {
                        Availability::Unavailable
                    },
                    bytes,
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    // kill/wait also applies to timed-out or truncated output, and reaps an
    // already exited child. No diagnostics or account logs enter stderr.
    let _ = child.kill();
    let _ = child.wait();
    match result {
        Ok((availability, bytes)) => {
            Observation { kind, availability, text: String::from_utf8_lossy(&bytes).into_owned() }
        }
        Err(_) => {
            Observation { kind, availability: Availability::Unavailable, text: String::new() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collector_retains_upstream_component_alphabet_without_option_or_shell_escape() {
        for name in ["ryoku-rashin.service", "app@example", "libstdc++6", "my-app_2"] {
            component_name(name).unwrap();
        }
        for name in ["", "--system", "/etc/passwd", "a;id", "a b", "a\n--system", "$(id)"] {
            assert_eq!(component_name(name), Err(Error::InvalidRequest));
        }
    }
    #[test]
    fn fixed_collectors_use_only_user_read_operations_and_distro_rpm() {
        let unit = command(ProbeKind::UserUnit, "ryoku-rashin.service");
        assert_eq!(unit.get_program(), "/usr/bin/systemctl");
        assert_eq!(
            unit.get_args().map(|value| value.to_str().unwrap()).collect::<Vec<_>>(),
            [
                "--user",
                "--no-pager",
                "show",
                "--property=Id,ActiveState,SubState,Result",
                "--",
                "ryoku-rashin.service"
            ]
        );
        let journal = command(ProbeKind::UserJournal, "ryoku-rashin.service");
        assert_eq!(journal.get_program(), "/usr/bin/journalctl");
        assert_eq!(
            journal.get_args().map(|value| value.to_str().unwrap()).collect::<Vec<_>>(),
            [
                "--user",
                "--no-pager",
                "--quiet",
                "--output=json",
                "--lines=40",
                "--unit",
                "ryoku-rashin.service"
            ]
        );
        let rpm = command(ProbeKind::RpmCapability, "curl");
        assert_eq!(rpm.get_program(), "/usr/bin/rpm");
        assert_eq!(
            rpm.get_args().map(|value| value.to_str().unwrap()).collect::<Vec<_>>(),
            ["--query", "--whatprovides", "--", "curl"]
        );
        for command in [unit, journal, rpm] {
            assert_eq!(command.get_envs().count(), 2);
        }
        let file = command(ProbeKind::RpmFileOwner, "curl");
        assert_eq!(
            file.get_args().map(|value| value.to_str().unwrap()).collect::<Vec<_>>(),
            ["--query", "--file", "--", "/usr/bin/curl"]
        );
        let unit_file = command(ProbeKind::RpmFileOwner, "ryoku-rashin.service");
        assert_eq!(
            unit_file.get_args().map(|value| value.to_str().unwrap()).collect::<Vec<_>>(),
            ["--query", "--file", "--", "/usr/lib/systemd/user/ryoku-rashin.service"]
        );
    }

    #[test]
    fn fix_summary_is_utf8_safe_bounded_and_labels_truncation() {
        let evidence = ComponentEvidence {
            component: "synthetic".into(),
            observations: vec![Observation {
                kind: ProbeKind::UserJournal,
                availability: Availability::Available,
                text: "\0λ".repeat(20000),
            }],
        };
        let summary = evidence.prompt_summary();
        assert!(summary.len() < 8192);
        let parsed: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(parsed["observations"][0]["truncated_for_prompt"], true);
        assert!(parsed["observations"][0]["text"].as_str().unwrap().len() <= 512);
    }

    #[test]
    fn probe_process_fixture() {
        use std::io::Write;
        match std::env::var("RASHIN_PROBE_TEST_ROLE").as_deref() {
            Ok("oversize") => {
                std::io::stdout().write_all(&vec![b'x'; MAX_PROBE_BYTES + 8192]).unwrap();
            }
            Ok("timeout") => std::thread::sleep(Duration::from_secs(2)),
            _ => (),
        }
    }

    #[test]
    fn stalled_and_oversized_collectors_are_killed_and_reaped() {
        for (role, expected) in
            [("oversize", Availability::Truncated), ("timeout", Availability::TimedOut)]
        {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "inspection::tests::probe_process_fixture", "--nocapture"])
                .env("RASHIN_PROBE_TEST_ROLE", role)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .stdin(Stdio::null());
            let start = Instant::now();
            let result = probe_command(ProbeKind::UserJournal, command);
            assert_eq!(result.availability, expected);
            assert!(result.text.len() <= MAX_PROBE_BYTES);
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }
}
