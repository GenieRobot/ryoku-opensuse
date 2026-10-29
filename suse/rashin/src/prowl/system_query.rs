//! Fixed read-only native probes for explicitly enabled assistant tools.
//! No model-selected executable, environment, shell, path or command option.
use crate::{Error, Result};
use serde::Deserialize;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    topic: Topic,
    #[serde(default)]
    arg: String,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Topic {
    Packages,
    Updates,
    Service,
    Processes,
    Disk,
    Kernel,
    Gpu,
    Network,
}
struct Probe {
    executable: &'static str,
    args: Vec<String>,
    limit: usize,
    count: bool,
}
fn probe(arguments: Arguments) -> Result<Probe> {
    let Arguments { topic, arg } = arguments;
    if arg.len() > 128
        || (!arg.is_empty()
            && (!arg.as_bytes()[0].is_ascii_alphanumeric()
                || !arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+@:-".contains(&b))))
    {
        return Err(Error::InvalidRequest);
    }
    if !arg.is_empty() && !matches!(topic, Topic::Packages | Topic::Service) {
        return Err(Error::InvalidRequest);
    }
    let (executable, args, limit, count) = match topic {
        Topic::Packages if arg.is_empty() => {
            ("/usr/bin/rpm", vec!["--query", "--all", "--queryformat", "."], 65536, true)
        }
        Topic::Packages => (
            "/usr/bin/rpm",
            vec![
                "--query",
                "--queryformat",
                "%{NAME} %{VERSION}-%{RELEASE}.%{ARCH}\\n",
                "--",
                &arg,
            ],
            4096,
            false,
        ),
        Topic::Updates => (
            "/usr/bin/zypper",
            vec!["--non-interactive", "--no-refresh", "list-updates"],
            4096,
            false,
        ),
        Topic::Service => {
            crate::inspection::component_name(&arg)?;
            (
                "/usr/bin/systemctl",
                vec![
                    "--user",
                    "--no-pager",
                    "show",
                    "--property=Id,ActiveState,SubState,Result",
                    "--",
                    &arg,
                ],
                4096,
                false,
            )
        }
        Topic::Processes => {
            ("/usr/bin/ps", vec!["-eo", "pid,pcpu,pmem,comm", "--sort=-pcpu"], 4096, false)
        }
        Topic::Disk => (
            "/usr/bin/df",
            vec!["-h", "--output=source,size,used,avail,pcent,target", "/", "/home"],
            2048,
            false,
        ),
        Topic::Kernel => ("/usr/bin/uname", vec!["-r"], 512, false),
        Topic::Gpu => ("/usr/sbin/lspci", vec!["-d", "::03xx"], 2048, false),
        Topic::Network => ("/usr/sbin/ip", vec!["-brief", "addr"], 2048, false),
    };
    Ok(Probe { executable, args: args.into_iter().map(str::to_owned).collect(), limit, count })
}
fn command(probe: &Probe) -> Command {
    let mut command = Command::new(probe.executable);
    command
        .args(&probe.args)
        .env_clear()
        .env("LANG", "C")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", rustix::process::geteuid().as_raw()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}
fn run(
    mut command: Command,
    limit: usize,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(String, bool)> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(Error::ProviderUnavailable);
    }
    let mut child = command.spawn().map_err(|_| Error::ProviderUnavailable)?;
    let result = (|| {
        let mut stdout = child.stdout.take().ok_or(Error::ProviderUnavailable)?;
        rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::ProviderUnavailable)?;
        let mut bytes = Vec::new();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(Error::ProviderUnavailable);
            }
            let mut chunk = [0; 4096];
            match stdout.read(&mut chunk) {
                Ok(0) => {
                    let status = loop {
                        if let Some(status) =
                            child.try_wait().map_err(|_| Error::ProviderUnavailable)?
                        {
                            break status;
                        }
                        if cancelled.load(Ordering::Acquire) {
                            return Err(Error::Cancelled);
                        }
                        if Instant::now() >= deadline {
                            return Err(Error::ProviderUnavailable);
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    };
                    if !status.success() {
                        return Err(Error::ProviderUnavailable);
                    }
                    return Ok((String::from_utf8_lossy(&bytes).into_owned(), false));
                }
                Ok(count) => {
                    let room = limit.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..count.min(room)]);
                    if count > room {
                        return Ok((String::from_utf8_lossy(&bytes).into_owned(), true));
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err(Error::ProviderUnavailable),
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
pub(super) fn execute(
    arguments: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<String> {
    let probe = probe(serde_json::from_str(arguments).map_err(|_| Error::InvalidRequest)?)?;
    let (mut output, mut truncated) = run(
        command(&probe),
        probe.limit,
        cancelled,
        deadline.min(Instant::now() + Duration::from_secs(8)),
    )?;
    let rows = match probe.executable {
        "/usr/bin/ps" => Some(16),
        "/usr/sbin/ip" => Some(12),
        _ => None,
    };
    if let Some(limit) = rows
        && output.lines().count() > limit
    {
        output = output.lines().take(limit).collect::<Vec<_>>().join("\n");
        truncated = true;
    }
    if probe.count {
        if truncated {
            return Err(Error::Limit);
        }
        return Ok(serde_json::json!({"packages":output.len(),"source":"installed_rpm_database"})
            .to_string());
    }
    Ok(serde_json::json!({"output":output.trim(),"truncated":truncated,"source":"read_only_native_query"}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_argv_has_no_shell_refresh_or_model_selected_executable() {
        for (topic, exe) in [
            ("packages", "/usr/bin/rpm"),
            ("updates", "/usr/bin/zypper"),
            ("service", "/usr/bin/systemctl"),
            ("processes", "/usr/bin/ps"),
            ("disk", "/usr/bin/df"),
            ("kernel", "/usr/bin/uname"),
            ("gpu", "/usr/sbin/lspci"),
            ("network", "/usr/sbin/ip"),
        ] {
            let arg = if topic == "service" { "ryoku-rashin.service" } else { "" };
            let probe = probe(
                serde_json::from_value(serde_json::json!({"topic":topic,"arg":arg})).unwrap(),
            )
            .unwrap();
            let command = command(&probe);
            assert_eq!(command.get_program(), exe);
            assert!(
                command.get_envs().all(|(name, _)| name == "LANG" || name == "XDG_RUNTIME_DIR")
            );
            assert!(!probe.args.iter().any(|arg| arg == "-c" || arg == "refresh"));
            if topic == "updates" {
                assert!(probe.args.iter().any(|arg| arg == "--no-refresh"));
            }
        }
        for value in [
            r#"{"topic":"packages","arg":"--root=/tmp"}"#,
            r#"{"topic":"service","arg":"x;touch z"}"#,
            r#"{"topic":"kernel","arg":"anything"}"#,
            r#"{"topic":"shell"}"#,
            r#"{"topic":"kernel","command":"ignored?"}"#,
        ] {
            assert!(
                execute(value, &AtomicBool::new(false), Instant::now() + Duration::from_secs(1))
                    .is_err()
            );
        }
    }
    #[test]
    fn cancellation_prevents_even_fixed_native_probe_and_kernel_is_real() {
        assert_eq!(
            execute(
                r#"{"topic":"kernel"}"#,
                &AtomicBool::new(true),
                Instant::now() + Duration::from_secs(1)
            ),
            Err(Error::Cancelled)
        );
        let result = execute(
            r#"{"topic":"kernel"}"#,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            value["output"],
            std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap().trim()
        );
        assert_eq!(value["truncated"], false);
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;
    #[test]
    fn subprocess_fixture() {
        match std::env::var("RASHIN_SYSTEM_TOOL_FIXTURE").as_deref() {
            Ok("oversize") => {
                use std::io::Write;
                std::io::stdout().write_all(&vec![b'x'; 8192]).unwrap();
            }
            Ok("stall") => std::thread::sleep(Duration::from_secs(2)),
            _ => {}
        }
    }
    fn fixture(role: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "prowl::system_query::process_tests::subprocess_fixture",
            "--nocapture",
        ])
        .env("RASHIN_SYSTEM_TOOL_FIXTURE", role)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        cmd
    }
    #[test]
    fn native_probe_output_timeout_and_midflight_cancel_are_bounded_and_reaped() {
        let result = run(
            fixture("oversize"),
            512,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(result.0.len(), 512);
        assert!(result.1);
        assert_eq!(
            run(
                fixture("stall"),
                512,
                &AtomicBool::new(false),
                Instant::now() + Duration::from_millis(30)
            ),
            Err(Error::ProviderUnavailable)
        );
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let setter = flag.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            setter.store(true, Ordering::Release);
        });
        let start = Instant::now();
        assert_eq!(
            run(fixture("stall"), 512, &flag, Instant::now() + Duration::from_secs(1)),
            Err(Error::Cancelled)
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
    }
}
