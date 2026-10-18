use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, fstat, openat2};
use std::{
    fs::{self, File},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
pub(super) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(super) fn require(value: bool, message: &'static str) -> Result<()> {
    if value { Ok(()) } else { Err(message.into()) }
}
pub(super) fn bounded_file(path: &Path, max: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    require(bytes.len() as u64 <= max, "fixture input exceeds bound")?;
    Ok(bytes)
}
pub(super) fn guest_guard() -> Result<()> {
    require(
        bounded_file(Path::new("/etc/ryoku-lab-fixture"), 128)? == b"disposable-issue13\n"
            && bounded_file(Path::new("/sys/class/dmi/id/sys_vendor"), 128)? == b"QEMU\n"
            && bounded_file(Path::new("/proc/1/comm"), 128)? == b"systemd\n"
            && matches!(
                bounded_file(Path::new("/sys/fs/selinux/enforce"), 8)?.as_slice(),
                b"1" | b"1\n"
            ),
        "requires seeded disposable QEMU with systemd and enforcing SELinux",
    )?;
    let os = String::from_utf8(bounded_file(Path::new("/etc/os-release"), 16 * 1024)?)?;
    require(
        os.lines()
            .any(|line| matches!(line, "ID=opensuse-tumbleweed" | "ID=\"opensuse-tumbleweed\"")),
        "requires Tumbleweed guest",
    )
}

pub(super) struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
fn drain(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            return Ok(bytes);
        }
        let keep = count.min((64 * 1024 + 1_usize).saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
    }
}
pub(super) fn bounded(command: &mut Command) -> Result<Output> {
    bounded_until(command, Instant::now() + Duration::from_secs(30))
}
pub(super) fn bounded_until(command: &mut Command, deadline: Instant) -> Result<Output> {
    command
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let out = child.stdout.take().ok_or("command stdout unavailable")?;
    let err = child.stderr.take().ok_or("command stderr unavailable")?;
    let stdout = thread::spawn(move || drain(out));
    let stderr = thread::spawn(move || drain(err));
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.kill();
            timed_out = true;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().map_err(|_| "command reader failed")??;
    let stderr = stderr.join().map_err(|_| "command reader failed")??;
    if timed_out || stdout.len() > 64 * 1024 || stderr.len() > 64 * 1024 {
        diagnostics(command, &stdout, &stderr);
        return Err("fixture command timed out or exceeded output bound".into());
    }
    Ok(Output { status, stdout, stderr })
}
pub(super) fn command(program: &str, args: &[&str]) -> Result<Output> {
    bounded(Command::new(program).args(args))
}
pub(super) fn success(command: &mut Command) -> Result<String> {
    success_until(command, Instant::now() + Duration::from_secs(30))
}
pub(super) fn success_until(command: &mut Command, deadline: Instant) -> Result<String> {
    let result = bounded_until(command, deadline)?;
    if !result.status.success() {
        diagnostics(command, &result.stdout, &result.stderr);
        return Err("native fixture command failed; retain guest diagnostics".into());
    }
    Ok(String::from_utf8(result.stdout)?)
}
fn diagnostics(command: &Command, stdout: &[u8], stderr: &[u8]) {
    // The harness retains stderr in its private guest log, never public markers.
    eprintln!(
        "fixture command: {command:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
}
pub(super) fn checked(program: &str, args: &[&str]) -> Result<String> {
    success(Command::new(program).args(args))
}

pub(super) fn trusted_directory(path: &Path) -> Result<()> {
    let stat = fs::symlink_metadata(path)?;
    require(
        stat.is_dir() && stat.uid() == 0 && stat.gid() == 0 && stat.mode() & 0o022 == 0,
        "unsafe native payload parent directory",
    )
}
pub(super) fn directory(path: &Path, uid: u32, mode: u32) -> Result<()> {
    let stat = fs::symlink_metadata(path)?;
    require(
        stat.is_dir() && stat.uid() == uid && stat.mode() & 0o7777 == mode,
        "native directory ownership or mode mismatch",
    )
}
pub(super) fn private_file(path: &Path, uid: u32, mode: u32) -> Result<()> {
    let stat = fs::symlink_metadata(path)?;
    require(
        stat.is_file()
            && stat.uid() == uid
            && stat.gid() == uid
            && stat.nlink() == 1
            && stat.mode() & 0o7777 == mode,
        "native file ownership, type, links or mode mismatch",
    )
}

pub(super) struct NativeTool {
    pub invocation: PathBuf,
    target: PathBuf,
}
fn validated_tool(invocation: PathBuf, uid: u32) -> Result<NativeTool> {
    let alias = fs::symlink_metadata(&invocation)?;
    let target = fs::canonicalize(&invocation)?;
    let stat = fs::metadata(&target)?;
    require(
        invocation.is_absolute()
            && alias.uid() == uid
            && stat.is_file()
            && stat.uid() == uid
            && stat.mode() & 0o022 == 0
            && stat.mode() & 0o111 != 0,
        "unsafe native tool invocation or target",
    )?;
    Ok(NativeTool { invocation, target })
}
pub(super) fn tool(name: &str) -> Result<NativeTool> {
    for directory in ["/usr/bin", "/usr/sbin", "/bin", "/sbin"] {
        if let Ok(tool) = validated_tool(Path::new(directory).join(name), 0) {
            // Multicall programs dispatch using argv[0]. Retain the safe
            // selected alias for invocation; canonicalize only for inspection.
            return Ok(tool);
        }
    }
    Err(format!("required native tool missing: {name}").into())
}
pub(super) fn inventory() -> Result<()> {
    let tools = [
        "rpm",
        "systemctl",
        "loginctl",
        "journalctl",
        "systemd-sysusers",
        "getent",
        "groupadd",
        "useradd",
        "usermod",
        "runuser",
        "restorecon",
        "matchpathcon",
        "bash",
    ];
    let mut missing = Vec::new();
    for name in tools {
        match tool(name) {
            Ok(tool) => {
                let owner = command(
                    "/usr/bin/rpm",
                    &[
                        "-qf",
                        "--qf",
                        "%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}",
                        tool.target.to_str().ok_or("invalid native target path")?,
                    ],
                )?;
                if !owner.status.success() {
                    missing.push(format!("RPM owner for {name}"));
                    continue;
                }
                let owner = String::from_utf8(owner.stdout)?;
                require(
                    !owner.is_empty()
                        && owner
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._+:~-".contains(&b)),
                    "invalid native RPM tool owner",
                )?;
                println!("native_service_tool_{name}_rpm={owner}");
            }
            Err(_) => missing.push(format!("tool {name}")),
        }
    }
    for (key, path) in [
        ("user_manager", "/usr/lib/systemd/system/user@.service"),
        ("user_runtime", "/usr/lib/systemd/system/user-runtime-dir@.service"),
        ("user_bus_socket", "/usr/lib/systemd/user/dbus.socket"),
    ] {
        match fs::canonicalize(path) {
            Ok(path) => {
                let owner = checked(
                    "/usr/bin/rpm",
                    &[
                        "-qf",
                        "--qf",
                        "%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}",
                        path.to_str().ok_or("invalid unit path")?,
                    ],
                )?;
                require(
                    !owner.is_empty()
                        && owner
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._+:~-".contains(&b)),
                    "invalid native RPM unit owner",
                )?;
                println!("native_service_unit_{key}_rpm={owner}");
            }
            Err(_) => missing.push(format!("native unit {key}")),
        }
    }
    for path in [
        "/usr/libexec",
        "/usr/lib/systemd/user",
        "/usr/lib/systemd/system",
        "/usr/lib/sysusers.d",
        "/home",
        "/run",
        "/run/user",
    ] {
        if trusted_directory(Path::new(path)).is_err() {
            missing.push(format!("trusted parent {path}"));
        }
    }
    require(missing.is_empty(), "missing native prerequisites; inventory reported before setup")
        .map_err(|_| format!("missing native prerequisites: {}", missing.join(", ")).into())
}

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Regular,
    Directory,
    Socket,
}
pub(super) struct Pinned {
    file: File,
    path: PathBuf,
    uid: u32,
    gid: u32,
    mode: u32,
    kind: Kind,
}
impl Pinned {
    pub(super) fn open(path: &str, uid: u32, gid: u32, mode: u32, kind: Kind) -> Result<Self> {
        let root = File::open("/")?;
        let file = File::from(openat2(
            &root,
            path.strip_prefix('/').ok_or("nonabsolute fixed path")?,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )?);
        let value = Self { file, path: path.into(), uid, gid, mode, kind };
        value.revalidate()?;
        Ok(value)
    }
    pub(super) fn revalidate(&self) -> Result<()> {
        let stat = fstat(&self.file)?;
        let current = fs::symlink_metadata(&self.path)?;
        let kind = match self.kind {
            Kind::Regular => FileType::RegularFile,
            Kind::Directory => FileType::Directory,
            Kind::Socket => FileType::Socket,
        };
        require(
            FileType::from_raw_mode(stat.st_mode) == kind
                && stat.st_uid == self.uid
                && stat.st_gid == self.gid
                && stat.st_mode & 0o7777 == self.mode
                && (matches!(self.kind, Kind::Directory) || stat.st_nlink == 1)
                && stat.st_dev == current.dev()
                && stat.st_ino == current.ino(),
            "fixture-owned inode metadata or namespace changed",
        )
    }
    fn permissions(&self, mode: u32) -> Result<()> {
        fs::set_permissions(
            format!("/proc/self/fd/{}", self.file.as_raw_fd()),
            fs::Permissions::from_mode(mode),
        )?;
        Ok(())
    }
}
struct Restore<'a>(&'a Pinned);
impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if self.0.permissions(self.0.mode).is_err() {
            eprintln!("native_service_fixture_mode_restore=failed");
        }
    }
}
pub(super) fn with_modes(
    pins: &[(&Pinned, u32)],
    action: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let mut guards = Vec::new();
    for (pin, mode) in pins {
        pin.revalidate()?;
        guards.push(Restore(pin));
        pin.permissions(*mode)?;
    }
    let result = action();
    for guard in &guards {
        guard.0.permissions(guard.0.mode)?;
    }
    drop(guards);
    for (pin, _) in pins {
        pin.revalidate()?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "multicall invocation child only"]
    fn multicall_alias_child() {
        if std::env::var_os("RYOKU_SERVICE_MULTICALL_CHILD").is_some() {
            assert_eq!(
                Path::new(&std::env::args_os().next().unwrap()).file_name(),
                Some(std::ffi::OsStr::new("restorecon"))
            );
        }
    }

    #[test]
    fn multicall_alias_is_invoked_while_canonical_target_is_inspected_separately() {
        let directory =
            std::env::temp_dir().join(format!("ryoku-service-alias-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let alias = directory.join("restorecon");
        let target = std::env::current_exe().unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let tool = validated_tool(alias.clone(), fs::metadata(&target).unwrap().uid()).unwrap();
        assert_eq!(tool.invocation, alias);
        assert_eq!(tool.target, fs::canonicalize(target).unwrap());
        let child = |path: &Path| {
            let mut command = Command::new(path);
            command
                .args(["--exact", "support::tests::multicall_alias_child", "--ignored"])
                .env("RYOKU_SERVICE_MULTICALL_CHILD", "1");
            bounded(&mut command).unwrap()
        };
        assert!(child(&tool.invocation).status.success());
        assert!(!child(&tool.target).status.success());
        fs::remove_file(alias).unwrap();
        fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn temporary_modes_restore_on_action_error_and_refuse_replaced_names() {
        let path = std::env::temp_dir().join(format!("ryoku-service-mode-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let pin =
            Pinned::open(path.to_str().unwrap(), meta.uid(), meta.gid(), 0o700, Kind::Directory)
                .unwrap();
        assert!(
            with_modes(&[(&pin, 0o711)], || {
                assert_eq!(fs::metadata(&path)?.mode() & 0o7777, 0o711);
                Err("injected policy-test failure".into())
            })
            .is_err()
        );
        pin.revalidate().unwrap();
        fs::rename(&path, path.with_extension("held")).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(pin.revalidate().is_err());
        fs::remove_dir(&path).unwrap();
        fs::remove_dir(path.with_extension("held")).unwrap();
    }

    #[test]
    #[ignore = "bounded runner child only"]
    fn bounded_child() {
        if std::env::var_os("RYOKU_SERVICE_FIXTURE_TIMEOUT_CHILD").is_some() {
            std::thread::sleep(Duration::from_secs(10));
        }
    }

    #[test]
    fn command_deadline_kills_and_reaps_the_rust_test_child() {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "support::tests::bounded_child", "--ignored"])
            .env("RYOKU_SERVICE_FIXTURE_TIMEOUT_CHILD", "1");
        let start = Instant::now();
        assert!(bounded_until(&mut child, start + Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
