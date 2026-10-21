//! Stop the actual CLI only after a successful durability syscall. No production
//! failpoint or journal rewrite is needed. This module is Linux/x86_64 lab code.
#![allow(unsafe_code)]
use super::{Result, require};
use std::{
    fs,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Traced {
    child: Child,
    reaped: bool,
}
#[cfg(test)]
thread_local! {
    static KILL_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
impl Traced {
    fn kill_owned(&mut self) -> std::io::Result<()> {
        #[cfg(test)]
        KILL_CALLS.with(|calls| calls.set(calls.get() + 1));
        self.child.kill()
    }
}
impl Drop for Traced {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.kill_owned();
            let _ = self.child.wait();
        }
    }
}

fn ptrace(request: libc::c_uint, pid: i32, data: usize) -> Result<()> {
    // SAFETY: these requests use no address pointer; data is a signal/options
    // integer. The pid is exclusively the child created below.
    if unsafe { libc::ptrace(request, pid, 0usize, data) } == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Kill after the selected fsync returns successfully. The callback runs while
/// the child is stopped, so no next phase can race the observation or SIGKILL.
pub fn kill_after_sync(
    executable: &str,
    arguments: &[&str],
    mut selected: impl FnMut(&str) -> Result<bool>,
) -> Result<()> {
    let mut command = Command::new(executable);
    command.args(arguments).stdout(Stdio::null()).stderr(Stdio::inherit());
    // SAFETY: pre_exec invokes only ptrace, which does not allocate or lock.
    unsafe {
        command.pre_exec(|| {
            if libc::ptrace(libc::PTRACE_TRACEME, 0, 0usize, 0usize) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = Traced { child: command.spawn()?, reaped: false };
    let pid = i32::try_from(child.child.id())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut initialized = false;
    loop {
        require(Instant::now() < deadline, "bounded CLI tracing deadline exceeded")?;
        let mut status = 0;
        // SAFETY: status is a live integer pointer and pid names our child.
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if waited == 0 {
            thread::sleep(Duration::from_micros(100));
            continue;
        }
        if waited == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.raw_os_error() == Some(libc::ECHILD) {
                child.reaped = true;
            }
            return Err(error.into());
        }
        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            child.reaped = true;
        }
        require(libc::WIFSTOPPED(status), "CLI exited before the selected durable boundary")?;
        let signal = libc::WSTOPSIG(status);
        if !initialized {
            require(signal == libc::SIGTRAP, "missing exec tracing stop")?;
            ptrace(
                libc::PTRACE_SETOPTIONS,
                pid,
                (libc::PTRACE_O_TRACESYSGOOD | libc::PTRACE_O_EXITKILL) as usize,
            )?;
            initialized = true;
        } else if signal == libc::SIGTRAP | 0x80 {
            // SAFETY: GETREGS writes exactly one native x86_64 register record.
            let mut regs: libc::user_regs_struct = unsafe { std::mem::zeroed() };
            if unsafe { libc::ptrace(libc::PTRACE_GETREGS, pid, 0usize, &mut regs) } == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            if regs.orig_rax == libc::SYS_fsync as u64 && regs.rax == 0 {
                let path = fs::read_link(format!("/proc/{pid}/fd/{}", regs.rdi))?;
                if selected(path.to_str().ok_or("non-UTF8 fixture descriptor")?)? {
                    child.kill_owned()?;
                    // Reap through waitpid because Child::wait cannot interpret
                    // intermediate tracing stops after a termination signal.
                    loop {
                        let mut killed = 0;
                        // SAFETY: same owned child and valid status pointer.
                        if unsafe { libc::waitpid(pid, &mut killed, 0) } == -1 {
                            let error = std::io::Error::last_os_error();
                            if error.kind() == std::io::ErrorKind::Interrupted {
                                continue;
                            }
                            if error.raw_os_error() == Some(libc::ECHILD) {
                                child.reaped = true;
                            }
                            return Err(error.into());
                        }
                        if libc::WIFSIGNALED(killed) {
                            // Do not signal this numeric pid again after waitpid
                            // has reaped it; the kernel may reuse that identity.
                            child.reaped = true;
                            require(libc::WTERMSIG(killed) == libc::SIGKILL, "wrong death signal")?;
                            return Ok(());
                        }
                        if libc::WIFEXITED(killed) {
                            child.reaped = true;
                            return Err("observed child exited instead of receiving SIGKILL".into());
                        }
                        ptrace(libc::PTRACE_CONT, pid, 0)?;
                    }
                }
            }
        }
        let deliver =
            if matches!(signal, libc::SIGTRAP | libc::SIGSTOP) || signal == libc::SIGTRAP | 0x80 {
                0
            } else {
                signal as usize
            };
        ptrace(libc::PTRACE_SYSCALL, pid, deliver)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kills_only_after_successful_fsync_and_reaps_child() {
        KILL_CALLS.with(|calls| calls.set(0));
        let path = std::env::temp_dir().join(format!("ryoku22-trace-{}", std::process::id()));
        assert!(!path.exists());
        kill_after_sync(
            "/usr/bin/dd",
            &[
                "if=/dev/zero",
                &format!("of={}", path.display()),
                "bs=1",
                "count=1",
                "conv=fsync",
                "status=none",
            ],
            |fd| Ok(fd == path.to_str().unwrap()),
        )
        .unwrap();
        KILL_CALLS.with(|calls| assert_eq!(calls.get(), 1, "reaped pid was signalled again"));
        assert_eq!(fs::read(&path).unwrap(), b"\0");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn exit_before_a_selected_durability_boundary_is_an_error() {
        KILL_CALLS.with(|calls| calls.set(0));
        assert!(kill_after_sync("/usr/bin/true", &[], |_| Ok(true)).is_err());
        KILL_CALLS.with(|calls| assert_eq!(calls.get(), 0, "exited child no longer owns its pid"));
    }
}
