//! Capture-worker confinement. Filesystem writes are denied before exec;
//! the worker denies further exec/fork before opening any GIO/user data.
#![allow(unsafe_code)]
use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
};
#[repr(C)]
struct Ruleset {
    handled_access_fs: u64,
}
pub(crate) unsafe fn read_only_filesystem() -> io::Result<()> {
    // SAFETY: bounded Linux Landlock structures; caller is the pre-exec child.
    unsafe {
        if libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<u8>(), 0, 1) < 3 {
            return Err(io::Error::other("GSettings capture requires Landlock ABI3"));
        }
        let attributes = Ruleset { handled_access_fs: 0x7fff & !(1 | 4 | 8) };
        let fd = libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attributes,
            std::mem::size_of::<Ruleset>(),
            0,
        ) as i32;
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let rules = File::from_raw_fd(fd);
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            || libc::syscall(libc::SYS_landlock_restrict_self, rules.as_raw_fd(), 0) != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
pub fn harden_worker() -> io::Result<()> {
    if !cfg!(target_arch = "x86_64") {
        return Err(io::Error::other("GSettings worker requires reviewed x86_64 syscall policy"));
    }
    let statement = |code, k| libc::sock_filter { code, k, jt: 0, jf: 0 };
    let jump = |k, jt, jf| libc::sock_filter { code: 0x15, k, jt, jf };
    let deny = 0x0005_0000 | libc::EPERM as u32;
    let allow = 0x7fff_0000;
    let mut filter = vec![
        statement(0x20, 4),
        jump(0xc000003e, 1, 0),
        statement(0x06, 0x8000_0000),
        statement(0x20, 0),
    ];
    let allowed_flags = (libc::CLONE_VM
        | libc::CLONE_FS
        | libc::CLONE_FILES
        | libc::CLONE_SIGHAND
        | libc::CLONE_THREAD
        | libc::CLONE_SYSVSEM
        | libc::CLONE_SETTLS
        | libc::CLONE_PARENT_SETTID
        | libc::CLONE_CHILD_CLEARTID
        | libc::CLONE_CHILD_SETTID) as u32;
    let required = (libc::CLONE_VM | libc::CLONE_SIGHAND | libc::CLONE_THREAD) as u32;
    filter.extend([
        jump(libc::SYS_clone as u32, 0, 12),
        statement(0x20, 20),
        jump(0, 1, 0),
        statement(0x06, deny),
        statement(0x20, 16),
        statement(0x54, !allowed_flags),
        jump(0, 1, 0),
        statement(0x06, deny),
        statement(0x20, 16),
        statement(0x54, required),
        jump(required, 0, 1),
        statement(0x06, allow),
        statement(0x06, deny),
        statement(0x20, 0),
        jump(libc::SYS_clone3 as u32, 0, 1),
        statement(0x06, 0x0005_0000 | libc::ENOSYS as u32),
    ]);
    for socket in [libc::SYS_socket, libc::SYS_socketpair] {
        filter.extend([
            jump(socket as u32, 0, 4),
            statement(0x20, 16),
            jump(libc::AF_UNIX as u32, 0, 1),
            statement(0x06, allow),
            statement(0x06, deny),
            statement(0x20, 0),
        ]);
    }
    for syscall in [
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_newfstatat,
        libc::SYS_statx,
        libc::SYS_lseek,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_pread64,
        libc::SYS_access,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
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
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_getresuid,
        libc::SYS_getresgid,
        libc::SYS_getgroups,
        libc::SYS_getpgrp,
        libc::SYS_arch_prctl,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_prlimit64,
        libc::SYS_futex,
        libc::SYS_getrandom,
        libc::SYS_clock_gettime,
        libc::SYS_gettimeofday,
        libc::SYS_uname,
        libc::SYS_sysinfo,
        libc::SYS_sched_getaffinity,
        libc::SYS_getrlimit,
        libc::SYS_getrusage,
        libc::SYS_times,
        libc::SYS_getdents64,
        libc::SYS_statfs,
        libc::SYS_fstatfs,
        libc::SYS_poll,
        libc::SYS_ppoll,
        libc::SYS_select,
        libc::SYS_pselect6,
        libc::SYS_epoll_create1,
        libc::SYS_epoll_ctl,
        libc::SYS_epoll_wait,
        libc::SYS_epoll_pwait,
        libc::SYS_eventfd2,
        libc::SYS_pipe,
        libc::SYS_pipe2,
        libc::SYS_connect,
        libc::SYS_getsockopt,
        libc::SYS_setsockopt,
        libc::SYS_getsockname,
        libc::SYS_getpeername,
        libc::SYS_pidfd_open,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_shutdown,
        libc::SYS_inotify_init1,
        libc::SYS_inotify_add_watch,
        libc::SYS_inotify_rm_watch,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ] {
        filter.extend([jump(syscall as u32, 0, 1), statement(0x06, allow)]);
    }
    filter.push(statement(0x06, deny));
    let program = libc::sock_fprog { len: filter.len() as u16, filter: filter.as_ptr().cast_mut() };
    // SAFETY: current worker is single-threaded at entry, before any user data.
    unsafe {
        read_only_filesystem()?;
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
            || libc::prctl(libc::PR_SET_SECCOMP, 2, &program) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
