//! Bounded child-process execution for lab operations that may block on the
//! network or package manager.
use rustix::process::{Pid, Signal, kill_process_group};
use std::os::unix::process::CommandExt;
use std::{
    io::{self, Read, Write},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) struct BoundedOutput {
    pub(crate) status: ExitStatus,
    pub(crate) timed_out: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stderr_truncated: bool,
}

fn drain<R: Read>(mut reader: R, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::with_capacity(limit.min(64 * 1024));
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok((retained, truncated));
        }
        let keep = count.min(limit.saturating_sub(retained.len()));
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
}

/// Execute a child with bounded retained output. Reader threads keep draining
/// both pipes even after the retention cap is reached, so a noisy child cannot
/// block on a full pipe. If the deadline expires, kill and reap the child
/// before returning.
pub(crate) fn run(
    command: &mut Command,
    timeout: Duration,
    output_limit: usize,
) -> io::Result<BoundedOutput> {
    run_until(command, Instant::now() + timeout, output_limit)
}

/// Execute a child using a caller-owned absolute deadline. This lets retry
/// loops bound the complete readiness operation instead of granting every
/// attempt a fresh timeout.
pub(crate) fn run_until(
    command: &mut Command,
    deadline: Instant,
    output_limit: usize,
) -> io::Result<BoundedOutput> {
    run_with_input_until(command, None, deadline, output_limit)
}

/// Variant for commands that must receive a small private input over stdin
/// (for example a generated password). Input is never added to argv or logs.
pub(crate) fn run_with_input_until(
    command: &mut Command,
    input: Option<&[u8]>,
    deadline: Instant,
    output_limit: usize,
) -> io::Result<BoundedOutput> {
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child =
        command.process_group(0).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().ok_or_else(|| io::Error::other("stdin pipe missing"))?;
        if let Err(error) = stdin.write_all(input) {
            if let Some(pid) = Pid::from_raw(child.id() as i32) {
                let _ = kill_process_group(pid, Signal::KILL);
            }
            let _ = child.wait();
            return Err(error);
        }
    }
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("stdout pipe missing"))?;
    let stderr = child.stderr.take().ok_or_else(|| io::Error::other("stderr pipe missing"))?;
    let stdout_reader = thread::spawn(move || drain(stdout, output_limit));
    let stderr_reader = thread::spawn(move || drain(stderr, output_limit));
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // zypper may have an rpm subprocess holding the output pipes. Put
            // the command in its own process group and terminate that group so
            // readers can reach EOF after the leader is reaped.
            let pid = Pid::from_raw(child.id() as i32)
                .ok_or_else(|| io::Error::other("child PID is invalid"))?;
            match kill_process_group(pid, Signal::KILL) {
                Ok(()) => timed_out = true,
                Err(_) => {
                    // It may have exited between try_wait and the group kill.
                    if let Some(status) = child.try_wait()? {
                        break status;
                    }
                    child.kill()?;
                    timed_out = true;
                }
            }
            break child.wait()?;
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    };
    let (stdout, stdout_truncated) =
        stdout_reader.join().map_err(|_| io::Error::other("stdout reader panicked"))??;
    let (stderr, stderr_truncated) =
        stderr_reader.join().map_err(|_| io::Error::other("stderr reader panicked"))??;
    Ok(BoundedOutput { status, timed_out, stdout, stdout_truncated, stderr, stderr_truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_out_child_is_killed_reaped_and_output_is_drained() {
        use std::os::unix::process::ExitStatusExt;

        let mut command = Command::new("sh");
        command.args(["-c", "printf before-timeout; sleep 30 & wait"]);
        let started = Instant::now();
        let output = run(&mut command, Duration::from_millis(100), 1024).unwrap();
        assert!(output.timed_out);
        assert!(!output.status.success());
        assert_eq!(output.status.signal(), Some(9));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(output.stdout, b"before-timeout");
        assert!(!output.stdout_truncated);
    }

    #[test]
    fn output_is_capped_while_both_pipes_continue_draining() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 2000000 /dev/zero; head -c 2000000 /dev/zero >&2"]);
        let output = run(&mut command, Duration::from_secs(5), 4096).unwrap();
        assert!(output.status.success());
        assert!(!output.timed_out);
        assert_eq!(output.stdout.len(), 4096);
        assert_eq!(output.stderr.len(), 4096);
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
    }

    #[test]
    fn caller_deadline_bounds_child_instead_of_granting_a_fresh_timeout() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let started = Instant::now();
        let output = run_until(&mut command, started + Duration::from_millis(120), 1024).unwrap();
        assert!(output.timed_out);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn private_input_is_forwarded_without_changing_command_arguments() {
        let mut command = Command::new("sh");
        command.args(["-c", "read value; printf '%s' \"$value\""]);
        let output = run_with_input_until(
            &mut command,
            Some(b"private-value\n"),
            Instant::now() + Duration::from_secs(2),
            1024,
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"private-value");
        assert_eq!(command.get_args().count(), 2);
    }
}
