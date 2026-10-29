//! One-shot, capture-only account-worker transport. No write operation exists
//! on this wire. Logical restore/enrollment is separate from file output receipts.
#![allow(unsafe_code)] // The child-only lifecycle callback uses Linux prctl/rlimit/FD syscalls.
use crate::theme_settings::{
    self, ThemeSettingChange, ThemeSettingKey, ThemeSettingsActor, ThemeSettingsBinding,
    ThemeSettingsCapsule,
};
use ryoku_recovery::{Identifier, Sha256};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, net::UnixStream, process::CommandExt},
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const MAX_FRAME: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRequest {
    pub version: u16,
    pub nonce: Identifier,
    pub binding: ThemeSettingsBinding,
    pub approved_policy_sha256: Sha256,
    pub plan_sha256: Sha256,
    #[serde(deserialize_with = "ryoku_recovery::logical_settings::deserialize_setting_changes")]
    pub desired: BTreeMap<ThemeSettingKey, ThemeSettingChange>,
    pub native_receipt: ryoku_recovery::native_checkpoint::RootSnapshotReceipt,
    pub session: crate::theme_actor_scope::SessionWitness,
    pub runtime_files: Vec<crate::theme_actor_scope::RuntimeFile>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureReply {
    pub version: u16,
    pub nonce: Identifier,
    pub capsule: ThemeSettingsCapsule,
}

/// Consume one root-authenticated inherited connection. The factory must add
/// exact runtime/schema/session binding and its live OS-backed lifetime guard.
pub fn serve_capture(
    mut control: UnixStream,
    factory: impl FnOnce(&CaptureRequest, UnixStream) -> io::Result<Box<dyn ThemeSettingsActor>>,
) -> io::Result<()> {
    let peer = rustix::net::sockopt::socket_peercred(&control)?;
    if peer.uid.as_raw() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "theme capture control peer is not root",
        ));
    }
    control.set_read_timeout(Some(Duration::from_secs(5)))?;
    control.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request: CaptureRequest = serde_json::from_slice(&read_frame(&mut control)?)?;
    let uid = rustix::process::geteuid().as_raw();
    if request.version != 1
        || uid == 0
        || request.binding.uid != uid
        || rustix::process::getuid() != rustix::process::geteuid()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "theme capture version/account mismatch",
        ));
    }
    request.native_receipt.validate().map_err(io::Error::other)?;
    reject_extra_request(&control)?;
    let mut actor = factory(&request, control.try_clone()?)?;
    let capsule = theme_settings::capture(
        &mut *actor,
        &request.binding,
        &request.native_receipt,
        request.approved_policy_sha256,
        request.plan_sha256,
        &request.desired,
    )
    .map_err(io::Error::other)?;
    reject_extra_request(&control)?;
    let reply = CaptureReply { version: 1, nonce: request.nonce, capsule };
    let mut bytes = serde_json::to_vec(&reply)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("logical capture reply exceeds bound"));
    }
    control.write_all(&bytes)?;
    control.shutdown(std::net::Shutdown::Both)
}
fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    // Do not prefetch a second frame into a BufReader and hide a replay from
    // the live socket guard. The single bounded control packet is small.
    let mut bytes = Vec::new();
    let mut byte = [0_u8];
    while bytes.len() < MAX_FRAME {
        stream.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(bytes);
        }
    }
    Err(io::Error::other("logical capture request exceeds bound"))
}
fn reject_extra_request(control: &UnixStream) -> io::Result<()> {
    let mut byte = [0];
    match rustix::net::recv(
        control,
        &mut byte,
        rustix::net::RecvFlags::PEEK | rustix::net::RecvFlags::DONTWAIT,
    ) {
        Err(rustix::io::Errno::AGAIN) => Ok(()),
        Err(error) => Err(error.into()),
        Ok((0, _)) => Err(io::Error::other("root actor control disconnected")),
        Ok(_) => Err(io::Error::other("one-shot actor rejected an extra/replayed request")),
    }
}
pub fn control_lifetime(control: &UnixStream) -> io::Result<()> {
    reject_extra_request(control)
}

/// No caller-selected executable or arguments enter the production route.
/// This currently provides only a capture worker; applying settings remains
/// unavailable until its distinct logical manifest family and restore barrier.
pub fn capture_worker(
    control: &UnixStream,
    home: &str,
    timeout: Duration,
    mut cancelled: impl FnMut() -> bool,
) -> io::Result<()> {
    let path = "/usr/libexec/ryoku/ryoku-theme-actor";
    let image = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = image.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o6022 != 0
        || metadata.mode() & 0o111 == 0
    {
        return Err(io::Error::other("theme actor executable identity unavailable"));
    }
    let mut command = Command::new(format!("/proc/self/fd/{}", image.as_raw_fd()));
    command
        .arg("--control-fd=3")
        .env_clear()
        .env("HOME", home)
        .env("LANG", "C")
        .env("GSETTINGS_BACKEND", "dconf")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/run/user/{}/bus", rustix::process::geteuid().as_raw()),
        );
    run_owned(command, control, timeout, &mut cancelled)
}
fn run_owned(
    mut command: Command,
    control: &UnixStream,
    timeout: Duration,
    cancelled: &mut impl FnMut() -> bool,
) -> io::Result<()> {
    if timeout.is_zero() || timeout > Duration::from_secs(30) {
        return Err(io::Error::other("actor deadline must be within30 seconds"));
    }
    if cancelled() {
        return Err(io::Error::other("actor cancelled before launch"));
    }
    let inherited = control.try_clone()?;
    let fd = inherited.as_raw_fd();
    let parent = std::process::id() as i32;
    command.current_dir("/").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only raw lifecycle/limit/descriptor syscalls run in the fork child.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 || libc::getppid() != parent
            {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            for (resource, limit) in [
                (libc::RLIMIT_CORE, 0),
                (libc::RLIMIT_FSIZE, 0),
                (libc::RLIMIT_CPU, 2),
                (libc::RLIMIT_AS, 128 * 1024 * 1024),
                (libc::RLIMIT_NOFILE, 64),
            ] {
                let limits = libc::rlimit { rlim_cur: limit, rlim_max: limit };
                if libc::setrlimit(resource, &limits) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 4_u32) != 0
                || libc::dup2(fd, 3) != 3
                || libc::fcntl(3, libc::F_SETFD, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            crate::theme_actor_sandbox::read_only_filesystem()?;
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    let mut failure = None;
    loop {
        let observed = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if let Some(status) = observed {
            return if let Some(reason) = failure {
                Err(io::Error::other(reason))
            } else if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "theme capture worker failed; no settings write was authorized",
                ))
            };
        }
        if failure.is_none() && (cancelled() || Instant::now() >= deadline) {
            failure = Some(
                "theme capture worker cancelled/timed out; owned process reaped before return",
            );
            let _ = child.kill();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
#[path = "theme_actor_tests.rs"]
mod tests;
