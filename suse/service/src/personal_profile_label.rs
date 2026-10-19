//! Fixed native policy query for one authenticated account's new profile.
//! No policy override, executable override or client path enters this module.
use super::{AccountStateRoot, Result, ServiceError, unavailable};
use rustix::{
    fs::{self, OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const MATCHPATHCON: &str = "/usr/sbin/matchpathcon";
const LIMIT: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn validate_new_profile_label(account: &AccountStateRoot, staged: &File) -> Result<()> {
    account.revalidate().map_err(unavailable)?;
    let expected = run_bounded(native_command(&account.personal_profile_policy_path()), TIMEOUT)?;
    let mut actual = [0_u8; LIMIT];
    let length = fs::fgetxattr(staged, "security.selinux", &mut actual).map_err(unavailable)?;
    compare_policy_label(&expected, &actual[..length])?;
    account.revalidate().map_err(unavailable)
}
fn native_command(path: &str) -> Command {
    let mut command = Command::new(MATCHPATHCON);
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("HOME", "/var/empty")
        .args(["-N", "-n", "-m", "file", "--", path]);
    command
}
fn compare_policy_label(expected: &[u8], actual: &[u8]) -> Result<()> {
    let expected = expected.strip_suffix(b"\n").ok_or(ServiceError::RecoveryUnavailable)?;
    let actual = actual.strip_suffix(b"\0").unwrap_or(actual);
    // Reject no-policy markers, multiple records, diagnostic output, and any
    // control bytes. Compare raw contexts (-N), never translated labels.
    if expected.is_empty()
        || expected.len() > LIMIT
        || expected == b"<<none>>"
        || !expected.contains(&b':')
        || expected.iter().any(|byte| !byte.is_ascii_graphic())
        || expected != actual
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    Ok(())
}
fn run_bounded(mut command: Command, timeout: Duration) -> Result<Vec<u8>> {
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(unavailable)?;
    let result = collect_child(&mut child, timeout);
    if result.is_err() {
        if let Some(pid) = Pid::from_raw(child.id() as i32) {
            let _ = kill_process_group(pid, Signal::KILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
fn collect_child(child: &mut Child, timeout: Duration) -> Result<Vec<u8>> {
    let mut stdout = child.stdout.take().ok_or(ServiceError::RecoveryUnavailable)?;
    let mut stderr = child.stderr.take().ok_or(ServiceError::RecoveryUnavailable)?;
    for descriptor in [&stdout as &dyn std::os::fd::AsFd, &stderr as &dyn std::os::fd::AsFd] {
        let flags = fcntl_getfl(descriptor).map_err(unavailable)?;
        fcntl_setfl(descriptor, flags | OFlags::NONBLOCK).map_err(unavailable)?;
    }
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    loop {
        if Instant::now() >= deadline {
            return Err(ServiceError::RecoveryUnavailable);
        }
        // One read per pipe per turn bounds memory and checks the clock even
        // for producers that never block. Child diagnostics stay private.
        if !stdout_done {
            stdout_done = read_chunk(&mut stdout, &mut output)?;
        }
        if !stderr_done {
            stderr_done = read_chunk(&mut stderr, &mut errors)?;
        }
        if let Some(status) = child.try_wait().map_err(unavailable)?
            && stdout_done
            && stderr_done
        {
            return if status.success() && errors.is_empty() {
                Ok(output)
            } else {
                Err(ServiceError::RecoveryUnavailable)
            };
        }
        thread::sleep(Duration::from_millis(2));
    }
}
fn read_chunk(reader: &mut impl Read, captured: &mut Vec<u8>) -> Result<bool> {
    let mut bytes = [0_u8; LIMIT];
    match reader.read(&mut bytes) {
        Ok(0) => Ok(true),
        Ok(length) => {
            if length > LIMIT.saturating_sub(captured.len()) {
                return Err(ServiceError::RecoveryUnavailable);
            }
            captured.extend_from_slice(&bytes[..length]);
            Ok(false)
        }
        Err(error)
            if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) =>
        {
            Ok(false)
        }
        Err(_) => Err(ServiceError::RecoveryUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn exact_raw_policy_context_required_without_normalization() {
        const LABEL: &[u8] = b"unconfined_u:object_r:config_home_t:s0";
        let mut expected = LABEL.to_vec();
        expected.push(b'\n');
        let mut actual = LABEL.to_vec();
        actual.push(0);
        compare_policy_label(&expected, &actual).unwrap();
        for expected in [
            b"<<none>>\n".as_slice(),
            b"\n",
            b"a:b\na:b\n",
            b" a:b\n",
            b"a:b",
            b"system_u:object_r:other_t:s0\n",
        ] {
            assert!(compare_policy_label(expected, &actual).is_err());
        }
        assert!(compare_policy_label(&expected, b"").is_err());
        assert!(
            compare_policy_label(&expected, b"unconfined_u:object_r:config_home_t:s0\0\0").is_err()
        );
    }
    #[test]
    fn command_is_fixed_raw_regular_file_query_with_no_policy_override() {
        let command = native_command("/home/account/.config/ryoku/opensuse-profile.json");
        assert_eq!(command.get_program(), MATCHPATHCON);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-N", "-n", "-m", "file", "--", "/home/account/.config/ryoku/opensuse-profile.json"]
        );
        assert_eq!(command.get_envs().count(), 3);
    }
    fn child(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env_clear().env("RYOKU_TEST_LABEL_CHILD", mode).args([
            "--exact",
            "personal_profile::label::tests::bounded_child_fixture",
            "--nocapture",
            "--quiet",
        ]);
        command
    }
    #[test]
    fn bounded_child_fixture() {
        let Ok(mode) = std::env::var("RYOKU_TEST_LABEL_CHILD") else { return };
        match mode.as_str() {
            "stdout" => {
                let _ = std::io::stdout().write_all(&vec![b'x'; LIMIT * 2]);
            }
            "stderr" => {
                let _ = std::io::stderr().write_all(&vec![b'e'; LIMIT * 2]);
            }
            "timeout" => thread::sleep(Duration::from_secs(5)),
            "failure" => std::process::exit(7),
            "success" => println!("bounded-policy-result"),
            _ => panic!("unexpected private fixture mode"),
        }
    }
    #[test]
    fn native_query_collection_is_bounded_and_errors_never_become_labels() {
        for mode in ["stdout", "stderr", "failure"] {
            assert!(run_bounded(child(mode), Duration::from_secs(2)).is_err());
        }
        let start = Instant::now();
        assert!(run_bounded(child("timeout"), Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        let output = run_bounded(child("success"), Duration::from_secs(2)).unwrap();
        assert!(
            output
                .windows(b"bounded-policy-result".len())
                .any(|bytes| bytes == b"bounded-policy-result")
        );
    }
}
