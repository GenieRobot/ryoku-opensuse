//! Bounded, newly owned groups for short desktop control clients only.
use std::{
    error::Error,
    io::{ErrorKind, Read},
    os::unix::process::CommandExt,
    process::{Child, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const REAP_TIMEOUT: Duration = Duration::from_secs(1);
const OUTPUT_LIMIT: usize = 4096;

pub fn checked_control(command: Command, timeout: Duration) -> Result<()> {
    run(command, timeout, false).map(|_| ())
}

pub fn capture_control(command: Command, timeout: Duration) -> Result<Vec<u8>> {
    run(command, timeout, true)
}

fn drain(pipe: &mut ChildStdout, output: &mut Vec<u8>) -> Result<bool> {
    loop {
        let mut bytes = [0; OUTPUT_LIMIT + 1];
        match pipe.read(&mut bytes) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if output.len() + count > OUTPUT_LIMIT {
                    return Err("desktop handler query exceeded 4096 bytes".into());
                }
                output.extend_from_slice(&bytes[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn terminate(mut child: Child) -> Result<()> {
    let group = rustix::process::Pid::from_raw(child.id() as i32)
        .ok_or("invalid control process identity")?;
    // The direct child has not been reaped: its PID still reserves our group.
    let killed = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            thread::Builder::new().name("ryoku-session-control-reaper".into()).spawn(
                move || {
                    let _ = child.wait();
                },
            )?;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    match killed {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(format!("control group termination failed: {error}").into()),
    }
}

fn run(mut command: Command, timeout: Duration, capture: bool) -> Result<Vec<u8>> {
    let program = command.get_program().to_owned();
    if capture {
        command.stdout(Stdio::piped());
    }
    let mut child = command.process_group(0).spawn()?;
    let mut output = Vec::new();
    if let Some(pipe) = child.stdout.as_ref() {
        let result = rustix::fs::fcntl_getfl(pipe)
            .and_then(|flags| rustix::fs::fcntl_setfl(pipe, flags | rustix::fs::OFlags::NONBLOCK));
        if let Err(error) = result {
            terminate(child)?;
            return Err(error.into());
        }
    }
    let deadline = Instant::now() + timeout;
    loop {
        let eof = match child.stdout.as_mut().map(|pipe| drain(pipe, &mut output)).transpose() {
            Ok(eof) => eof.unwrap_or(true),
            Err(error) => {
                terminate(child)?;
                return Err(error);
            }
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(format!("{} failed: {status}", program.to_string_lossy()).into());
                }
                // Re-drain after wait: the child may have written between the
                // first drain and exit. Never wait on an inherited writer FD.
                if !eof
                    && let Some(pipe) = child.stdout.as_mut()
                    && !drain(pipe, &mut output)?
                {
                    return Err("desktop handler query left an open output stream".into());
                }
                return Ok(output);
            }
            Ok(None) => {}
            Err(error) => {
                terminate(child)?;
                return Err(error.into());
            }
        }
        if Instant::now() >= deadline {
            terminate(child)?;
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                format!("{} exceeded its control deadline", program.to_string_lossy()),
            )
            .into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    #[test]
    fn query_capture_is_bounded_and_propagates_failures() {
        let mut success = Command::new("/usr/bin/printf");
        success.arg("org.example.Native.desktop\n");
        assert_eq!(
            capture_control(success, Duration::from_secs(1)).unwrap(),
            b"org.example.Native.desktop\n"
        );
        let mut boundary = Command::new("/usr/bin/printf");
        boundary.arg("a".repeat(OUTPUT_LIMIT));
        assert_eq!(capture_control(boundary, Duration::from_secs(1)).unwrap().len(), OUTPUT_LIMIT);
        let mut overflow = Command::new("/usr/bin/printf");
        overflow.arg("a".repeat(OUTPUT_LIMIT + 1));
        assert!(
            capture_control(overflow, Duration::from_secs(1))
                .unwrap_err()
                .to_string()
                .contains("4096")
        );
        assert!(capture_control(Command::new("/usr/bin/false"), Duration::from_secs(1)).is_err());
        assert!(
            capture_control(Command::new("/not-an-installed-client"), Duration::from_secs(1))
                .is_err()
        );
    }

    #[test]
    fn stalled_owned_client_group_is_reaped_without_stopping_an_independent_server() {
        const MARKER: &str = "RYOKU_TEST_CLIENT_GROUP";
        if let Some(path) = std::env::var_os(MARKER) {
            let mut child = Command::new("/usr/bin/sleep").arg("30").spawn().unwrap();
            fs::write(path, format!("{} {}", std::process::id(), child.id())).unwrap();
            // Waiting on the descendant deliberately stalls this client;
            // timeout cleanup must kill both members of the owned group.
            let _ = child.wait();
            return;
        }
        let path = PathBuf::from(format!("/tmp/ryoku-app-client-{}", std::process::id()));
        let mut server = Command::new("/usr/bin/sleep").arg("30").spawn().unwrap();
        let mut fixture = Command::new(std::env::current_exe().unwrap());
        fixture.args(["--exact", "client::tests::stalled_owned_client_group_is_reaped_without_stopping_an_independent_server", "--nocapture"]).env(MARKER, &path);
        let started = Instant::now();
        let result = capture_control(fixture, Duration::from_millis(500));
        let server_alive = server.try_wait().unwrap().is_none();
        server.kill().unwrap();
        server.wait().unwrap();
        assert!(result.unwrap_err().to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(server_alive);
        let pids = fs::read_to_string(&path).unwrap();
        fs::remove_file(path).unwrap();
        let pids = pids.split_whitespace().collect::<Vec<_>>();
        assert!(
            !PathBuf::from(format!("/proc/{}", pids[0])).exists(),
            "direct client was not reaped"
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let state = fs::read_to_string(format!("/proc/{}/stat", pids[1]));
            if state.as_ref().is_err_and(|e| e.kind() == ErrorKind::NotFound)
                || state.is_ok_and(|s| s.rsplit_once(')').unwrap().1.trim_start().starts_with('Z'))
            {
                break;
            }
            assert!(Instant::now() < deadline, "owned descendant survived timeout");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
