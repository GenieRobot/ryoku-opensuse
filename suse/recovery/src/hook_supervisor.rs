//! Single-process hook class. Landlock confines executable and file writes;
//! seccomp denies network, fork/clone, process control and other kernel APIs.
//! The caller retains all writer freezes until the exact child is reaped.
#![allow(unsafe_code)] // Linux confinement syscalls are isolated here with explicit safety invariants.
#[cfg(test)]
#[path = "hook_supervisor_tests.rs"]
mod tests;
use crate::{hook_protocol::HookCall, wallpaper_protocol::digest};
use std::{
    fs::File,
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, process::CommandExt},
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub fn shell_digest() -> io::Result<String> {
    let mut identities = Vec::new();
    for mut file in std::iter::once(shell()?).chain(runtime_files()?) {
        let mut bytes = Vec::new();
        file.by_ref().take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(io::Error::other("hook runtime exceeds bound"));
        }
        identities.push(digest(&bytes));
    }
    Ok(digest(&serde_json::to_vec(&identities).map_err(io::Error::other)?))
}
fn runtime_files() -> io::Result<Vec<File>> {
    // Tumbleweed paths plus the exact Ubuntu CI equivalents. No directory is
    // granted read/execute: each successfully resolved regular inode is pinned.
    let candidates: &[(&[&str], bool)] = &[
        (&["/lib64/ld-linux-x86-64.so.2"], false),
        (&["/lib64/libc.so.6", "/lib/x86_64-linux-gnu/libc.so.6"], false),
        (&["/lib64/libtinfo.so.6", "/lib/x86_64-linux-gnu/libtinfo.so.6"], false),
        (&["/lib64/libreadline.so.8", "/lib/x86_64-linux-gnu/libreadline.so.8"], true),
        (&["/etc/ld.so.cache"], false),
    ];
    candidates
        .iter()
        .filter_map(|(paths, optional)| {
            let path = paths.iter().find_map(|path| std::fs::canonicalize(path).ok());
            if path.is_none() && *optional {
                return None;
            }
            Some((|| {
                let path =
                    path.ok_or_else(|| io::Error::other("reviewed hook runtime file missing"))?;
                let file = File::from(rustix::fs::open(
                    &path,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )?);
                let meta = file.metadata()?;
                if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o6022 != 0 {
                    return Err(io::Error::other("hook runtime identity/ownership unavailable"));
                }
                Ok(file)
            })())
        })
        .collect()
}
fn shell() -> io::Result<File> {
    let file = File::from(rustix::fs::open(
        "/usr/bin/bash",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o6022 != 0 || meta.mode() & 0o111 == 0 {
        return Err(io::Error::other("hook shell identity/ownership unavailable"));
    }
    Ok(file)
}
pub(crate) fn run(
    call: &HookCall,
    home: &str,
    mut cancelled: impl FnMut() -> bool,
) -> io::Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    if !call.validate(uid, home)
        || uid == 0
        || uid != rustix::process::getuid().as_raw()
        || rustix::process::getgid() != rustix::process::getegid()
    {
        return Err(io::Error::other(
            "hook account/policy mismatch; explicitly authorize the current configuration",
        ));
    }
    if cancelled() {
        return Err(io::Error::other("hook cancelled before launch"));
    }
    let executable = shell()?;
    if shell_digest()? != call.policy.shell_sha256 {
        return Err(io::Error::other("hook shell changed; review and authorize again"));
    }
    let root = File::open(home)?;
    if root.metadata()?.dev() != call.home_device || root.metadata()?.ino() != call.home_inode {
        return Err(io::Error::other("hook home differs from the captured account descriptor"));
    }
    let configuration = File::from(rustix::fs::openat2(
        &root,
        ".config/ryoku/ryogami.json",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS
            | rustix::fs::ResolveFlags::NO_XDEV,
    )?);
    let m = configuration.metadata()?;
    if !m.is_file() || m.uid() != uid || m.nlink() != 1 || m.len() > 4 * 1024 * 1024 {
        return Err(io::Error::other("hook configuration ownership/type changed"));
    }
    let mut bytes = Vec::new();
    configuration.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if digest(&bytes) != call.policy.configuration_sha256 {
        return Err(io::Error::other("hook configuration changed; review and authorize again"));
    }
    let mut outputs = Vec::new();
    for (output, identity) in call.policy.approval.outputs.iter().zip(&call.output_identities) {
        let file = File::from(rustix::fs::openat2(
            &root,
            output.path(),
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_MAGICLINKS
                | rustix::fs::ResolveFlags::NO_XDEV,
        )?);
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.dev() != identity.device
            || meta.ino() != identity.inode
            || meta.uid() != uid
            || meta.nlink() != 1
            || meta.mode() & 0o6022 != 0
            || meta.len() > 16 * 1024 * 1024
        {
            return Err(io::Error::other(
                "hook output must be an existing private account-owned captured regular file",
            ));
        }
        outputs.push(file);
    }
    let runtime = runtime_files()?;
    let staged = if call.policy.approval.execution
        == crate::hook_protocol::HookExecutionPolicy::StagedOutputs
    {
        Some(crate::hook_staging::HookStage::prepare(&root, home, call)?)
    } else {
        None
    };
    let namespace = staged.as_ref().map(|stage| stage.namespace());
    let sandbox =
        Sandbox::new(&executable, &runtime, if staged.is_some() { &[] } else { &outputs })?;
    let parent = std::process::id();
    let mut command = Command::new(format!("/proc/self/fd/{}", executable.as_raw_fd()));
    command.args(["--noprofile", "--norc", "-c", &call.expanded_command()]);
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin")
        .env("LANG", "C")
        .current_dir(format!("/proc/self/fd/{}", root.as_raw_fd()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: child callback only uses prebuilt descriptors/filter and async-signal-safe syscalls.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                || libc::getppid() != parent as i32
            {
                return Err(io::Error::last_os_error());
            }
            let staged_root =
                namespace.as_ref().map(|namespace| namespace.install()).transpose()?;
            // Namespace credential transitions may clear PDEATHSIG. Re-arm and
            // check the original parent before any untrusted image executes.
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                || libc::getppid() != parent as i32
            {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            for (resource, limit) in [
                (libc::RLIMIT_CORE, 0),
                (libc::RLIMIT_FSIZE, 16 * 1024 * 1024),
                (libc::RLIMIT_CPU, 2),
                (libc::RLIMIT_AS, 128 * 1024 * 1024),
                (libc::RLIMIT_NOFILE, 64),
            ] {
                let limits = libc::rlimit { rlim_cur: limit, rlim_max: limit };
                if libc::setrlimit(resource, &limits) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 4_u32) != 0 {
                return Err(io::Error::last_os_error());
            }
            sandbox.enforce(staged_root)
        });
    }
    let mut child = command.spawn()?;
    let deadline =
        Instant::now() + Duration::from_secs(call.policy.approval.timeout_seconds.into());
    let mut failure = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if let Some(reason) = failure {
                    Err(io::Error::other(reason))
                } else if status.success() {
                    staged.as_ref().map_or(Ok(()), |stage| stage.commit())
                } else {
                    Err(io::Error::other(
                        "hook process failed or requested an effect outside its approved policy",
                    ))
                };
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        if failure.is_none() && (cancelled() || Instant::now() >= deadline) {
            failure = Some("hook cancelled or timed out; linked recovery may be required");
            child.kill()?;
        }
        // Do not acknowledge/refreeze completion until the owned child is observed dead.
        // A kernel-stalled child keeps this writer lease held; the helper times out safely.
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[repr(C)]
struct Ruleset {
    handled_access_fs: u64,
}
#[repr(C)]
struct PathRule {
    allowed_access: u64,
    parent_fd: i32,
}
const EXECUTE: u64 = 1;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const TRUNCATE: u64 = 1 << 14;
struct Sandbox {
    ruleset: File,
    filter: Vec<libc::sock_filter>,
}
impl Sandbox {
    fn new(executable: &File, runtime: &[File], outputs: &[File]) -> io::Result<Self> {
        use std::os::fd::FromRawFd;
        // ABI 3 is mandatory: earlier kernels cannot constrain truncate(2).
        // SAFETY: version query has no pointer arguments.
        let abi = unsafe {
            libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0, 1)
        };
        if abi < 3 {
            return Err(io::Error::other("hook unavailable: Landlock ABI 3 required"));
        }
        let attr = Ruleset { handled_access_fs: 0x7fff };
        // Constrain reads as well: granting the ELF loader exec without bounding
        // its reads would let it map an unapproved executable as ordinary data.
        // SAFETY: kernel reads a correctly sized initialized structure.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr,
                std::mem::size_of::<Ruleset>(),
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful syscall returned a fresh owned descriptor.
        let ruleset = unsafe { File::from_raw_fd(fd as i32) };
        for (file, access) in std::iter::once((executable, EXECUTE | READ_FILE))
            .chain(
                runtime
                    .iter()
                    .enumerate()
                    .map(|(index, file)| (file, READ_FILE | if index == 0 { EXECUTE } else { 0 })),
            )
            .chain(outputs.iter().map(|file| (file, WRITE_FILE | TRUNCATE)))
        {
            let rule = PathRule { allowed_access: access, parent_fd: file.as_raw_fd() };
            // SAFETY: valid descriptor and initialized path rule; size is defined by kernel rule type.
            if unsafe {
                libc::syscall(libc::SYS_landlock_add_rule, ruleset.as_raw_fd(), 1, &rule, 0)
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self { ruleset, filter: filter()? })
    }
    unsafe fn enforce(&self, staged_root: Option<i32>) -> io::Result<()> {
        let program = libc::sock_fprog {
            len: self.filter.len() as u16,
            filter: self.filter.as_ptr().cast_mut(),
        };
        // SAFETY: valid ruleset and prebuilt BPF program; this affects only the fork child.
        if let Some(root) = staged_root {
            // Only the private, noexec, quota-bounded tmpfs receives computation
            // write access. No real home directory or output inode is writable.
            let rule = PathRule { allowed_access: 0x7fff & !EXECUTE, parent_fd: root };
            if unsafe {
                libc::syscall(libc::SYS_landlock_add_rule, self.ruleset.as_raw_fd(), 1, &rule, 0)
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
            || unsafe {
                libc::syscall(libc::SYS_landlock_restrict_self, self.ruleset.as_raw_fd(), 0)
            } != 0
            || unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
fn filter() -> io::Result<Vec<libc::sock_filter>> {
    if !cfg!(target_arch = "x86_64") {
        return Err(io::Error::other("hook sandbox requires audited x86_64 syscall policy"));
    }
    let stmt = |code, k| libc::sock_filter { code, jt: 0, jf: 0, k };
    let jump = |k, jt, jf| libc::sock_filter { code: 0x15, jt, jf, k };
    let mut filter =
        vec![stmt(0x20, 4), jump(0xc000003e, 1, 0), stmt(0x06, 0x80000000), stmt(0x20, 0)];
    let allowed = [
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_newfstatat,
        libc::SYS_lseek,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_brk,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_pread64,
        libc::SYS_access,
        libc::SYS_openat,
        libc::SYS_readlink,
        libc::SYS_readlinkat,
        libc::SYS_fcntl,
        libc::SYS_dup,
        libc::SYS_dup2,
        libc::SYS_dup3,
        libc::SYS_getcwd,
        libc::SYS_chdir,
        libc::SYS_getpid,
        libc::SYS_getppid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        // Newer Bash uses the saved-ID queries during startup/re-exec. These
        // only read identity; every set*id and capability mutation stays denied.
        libc::SYS_getresuid,
        libc::SYS_getresgid,
        libc::SYS_getpgrp,
        libc::SYS_getgroups,
        libc::SYS_arch_prctl,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_prlimit64,
        libc::SYS_futex,
        libc::SYS_getrandom,
        libc::SYS_clock_gettime,
        libc::SYS_uname,
        libc::SYS_sysinfo,
        libc::SYS_getrlimit,
        libc::SYS_getrusage,
        libc::SYS_times,
        libc::SYS_getdents64,
        libc::SYS_statfs,
        libc::SYS_fstatfs,
        libc::SYS_statx,
        libc::SYS_fsync,
        libc::SYS_fdatasync,
        libc::SYS_ftruncate,
        libc::SYS_truncate,
        libc::SYS_execve,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ];
    for syscall in allowed {
        filter.push(jump(syscall as u32, 0, 1));
        filter.push(stmt(0x06, 0x7fff0000));
    }
    filter.push(stmt(0x06, 0x00050000 | libc::EPERM as u32));
    Ok(filter)
}
