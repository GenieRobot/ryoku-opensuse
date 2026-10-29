//! Process jail established before any adapter byte is accepted. Only exact,
//! nonrecursive public-runtime binds enter it; private state and inherited
//! submounts are absent. A different mapped UID cannot inspect the bootstrap
//! process through procfs. Killing the unshare manager kills the PID namespace.
use super::Config;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
const HELPERS: [&str; 7] = [
    "/usr/bin/unshare",
    "/usr/bin/mount",
    "/usr/bin/setpriv",
    "/usr/bin/chroot",
    "/usr/bin/prlimit",
    "/usr/bin/newuidmap",
    "/usr/bin/newgidmap",
];
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    root: String,
    config: Config,
    namespaces: Vec<String>,
}
fn unavailable<T>(_: T) -> Error {
    Error::ProviderUnavailable
}
fn helpers() -> Result<()> {
    check_helpers(&HELPERS)
}
fn check_helpers(paths: &[&str]) -> Result<()> {
    for helper in paths {
        let metadata = fs::metadata(helper).map_err(unavailable)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::ProviderUnavailable);
        }
    }
    Ok(())
}
fn namespaces() -> Result<Vec<String>> {
    ["user", "mnt", "pid", "ipc", "uts", "net"]
        .into_iter()
        .map(|name| {
            fs::read_link(format!("/proc/self/ns/{name}"))
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(unavailable)
        })
        .collect()
}
fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(unavailable)?;
    if !status.success() {
        return Err(Error::ProviderUnavailable);
    }
    Ok(())
}
fn only_stdio() -> Result<()> {
    let mut fds = fs::read_dir("/proc/self/fd")
        .map_err(unavailable)?
        .map(|entry| {
            entry.map(|entry| entry.file_name().to_string_lossy().into_owned()).map_err(unavailable)
        })
        .collect::<Result<Vec<_>>>()?;
    fds.sort();
    if fds != ["0", "1", "2", "3"] {
        return Err(Error::UnsafeState);
    }
    if fs::read_link("/proc/self/fd/3").is_ok() {
        return Err(Error::UnsafeState);
    } // scanner was dropped; descriptor3 must be closed.
    Ok(())
}
pub(crate) struct Process {
    child: Child,
    root: PathBuf,
}
impl Process {
    pub(super) fn spawn(config: &Config, runner: &Path) -> Result<Self> {
        config.validate()?;
        config.binary.capture()?;
        helpers()?;
        let mut entropy = [0u8; 16];
        fs::File::open("/dev/urandom")
            .map_err(unavailable)?
            .read_exact(&mut entropy)
            .map_err(unavailable)?;
        let root = std::env::temp_dir().join(format!("rashin-acp-{}", crate::digest(&entropy)));
        fs::create_dir(&root).map_err(unavailable)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(unavailable)?;
        let plan = Plan {
            root: root.to_str().ok_or(Error::UnsafeState)?.into(),
            config: config.clone(),
            namespaces: namespaces()?,
        };
        let mut command = Command::new(HELPERS[0]);
        command.args([
            "--user",
            "--map-auto",
            "--map-root-user",
            "--mount",
            "--pid",
            "--ipc",
            "--uts",
            "--fork",
            "--kill-child=KILL",
        ]);
        if !config.network {
            command.arg("--net");
        }
        let spawn = command
            .arg("--")
            .arg(runner)
            .arg("acp-worker")
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("LANG", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let child = match spawn {
            Ok(child) => child,
            Err(_) => {
                let _ = fs::remove_dir(&root);
                return Err(Error::ProviderUnavailable);
            }
        };
        let mut process = Self { child, root };
        let bytes = serde_json::to_vec(&plan).map_err(|_| Error::InvalidRequest)?;
        if bytes.len() > 16 * 1024 {
            return Err(Error::Limit);
        }
        let stdin = process.child.stdin.as_mut().ok_or(Error::ProviderUnavailable)?;
        rustix::fs::fcntl_setfl(&*stdin, rustix::fs::OFlags::NONBLOCK).map_err(unavailable)?;
        let mut payload = (bytes.len() as u32).to_be_bytes().to_vec();
        payload.extend_from_slice(&bytes);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut written = 0;
        while written < payload.len() {
            if std::time::Instant::now() >= deadline {
                return Err(Error::ProviderUnavailable);
            }
            match stdin.write(&payload[written..]) {
                Ok(0) => return Err(Error::ProviderUnavailable),
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(Error::ProviderUnavailable),
            }
        }
        Ok(process)
    }
    pub(super) fn id(&self) -> u32 {
        self.child.id()
    }
    pub(super) fn stdin(&mut self) -> Result<std::process::ChildStdin> {
        self.child.stdin.take().ok_or(Error::ProviderUnavailable)
    }
    pub(super) fn stdout(&mut self) -> Result<std::process::ChildStdout> {
        self.child.stdout.take().ok_or(Error::ProviderUnavailable)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir(&self.root);
    }
}
// Bootstrap shares fd0 with the eventual adapter. Buffered Stdin would read
// ahead into the initialize frame and strand those bytes in this process.
fn bootstrap_read(mut bytes: &mut [u8]) -> Result<()> {
    let stdin = std::io::stdin();
    while !bytes.is_empty() {
        let count = match rustix::io::read(&stdin, &mut *bytes) {
            Ok(0) => return Err(Error::ProviderUnavailable),
            Ok(count) => count,
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return Err(Error::ProviderUnavailable),
        };
        bytes = &mut bytes[count..];
    }
    Ok(())
}
pub fn worker() -> Result<()> {
    // The worker is a private executable mode, not an API-selected executable.
    // Verify unshare completed before any path/mount operation.
    only_stdio()?;
    helpers()?;
    let mut size = [0u8; 4];
    bootstrap_read(&mut size)?;
    let length = u32::from_be_bytes(size) as usize;
    if length > 16 * 1024 {
        return Err(Error::Limit);
    }
    let mut bytes = vec![0; length];
    bootstrap_read(&mut bytes)?;
    let plan: Plan = serde_json::from_slice(&bytes).map_err(|_| Error::InvalidRequest)?;
    let current = namespaces()?;
    if current.len() != plan.namespaces.len()
        || current.iter().zip(&plan.namespaces).take(5).any(|(a, b)| a == b)
        || (!plan.config.network && current[5] == plan.namespaces[5])
        || rustix::process::geteuid().as_raw() != 0
    {
        return Err(Error::UnsafeState);
    }
    let root = Path::new(&plan.root);
    let metadata = fs::symlink_metadata(root).map_err(unavailable)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(Error::UnsafeState);
    }
    let binary = plan.config.binary.capture()?;
    run(HELPERS[1], &["--make-rprivate", "/"])?;
    run(HELPERS[1], &["-t", "tmpfs", "-o", "size=256m,nosuid,nodev", "tmpfs", &plan.root])?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o755)).map_err(unavailable)?;
    for directory in ["usr", "tmp", "proc", "dev", "etc", "home", "home/adapter"] {
        fs::create_dir(root.join(directory)).map_err(unavailable)?;
    }
    for directory in ["bin", "lib64"] {
        let source = format!("/usr/{directory}");
        let target = root.join(format!("usr/{directory}"));
        fs::create_dir(&target).map_err(unavailable)?;
        let target = target.to_str().ok_or(Error::UnsafeState)?;
        run(HELPERS[1], &["--bind", &source, target])?;
        run(HELPERS[1], &["-o", "remount,bind,ro,nosuid,nodev", &source, target])?;
    }
    for name in ["bin", "lib", "lib64"] {
        std::os::unix::fs::symlink(format!("usr/{name}"), root.join(name)).map_err(unavailable)?;
    }
    for (directory, options) in [
        ("tmp", "size=16m,nosuid,nodev,mode=1777"),
        ("home/adapter", "size=32m,nosuid,nodev,mode=0700,uid=65534,gid=65534"),
    ] {
        run(
            HELPERS[1],
            &[
                "-t",
                "tmpfs",
                "-o",
                options,
                "tmpfs",
                root.join(directory).to_str().ok_or(Error::UnsafeState)?,
            ],
        )?;
    }
    run(
        HELPERS[1],
        &[
            "-t",
            "proc",
            "-o",
            "nosuid,nodev,noexec,hidepid=2",
            "proc",
            root.join("proc").to_str().ok_or(Error::UnsafeState)?,
        ],
    )?;
    for name in ["null", "urandom", "random"] {
        let source = format!("/dev/{name}");
        if !fs::metadata(&source).map_err(unavailable)?.file_type().is_char_device() {
            return Err(Error::UnsafeState);
        }
        let target = root.join(format!("dev/{name}"));
        fs::write(&target, []).map_err(unavailable)?;
        run(HELPERS[1], &["--bind", &source, target.to_str().ok_or(Error::UnsafeState)?])?;
    }
    let adapter = root.join("adapter");
    fs::write(&adapter, &binary).map_err(unavailable)?;
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o555)).map_err(unavailable)?;
    run(HELPERS[1], &["-o", "remount,ro,nosuid,nodev", &plan.root])?;
    only_stdio()?;
    // prlimit runs only after chroot and dropping to a different mapped UID.
    // It cannot regain setup credentials; all inherited environment is fixed.
    let mut command = Command::new(HELPERS[2]);
    command
        .args([
            "--no-new-privs",
            "--",
            HELPERS[3],
            "--userspec=65534:65534",
            "--groups=65534",
            &plan.root,
            HELPERS[4],
            "--as=2147483648",
            "--cpu=120",
            "--nproc=64",
            "--nofile=64",
            "--fsize=16777216",
            "--core=0",
            "--",
            "/adapter",
        ])
        .args(plan.config.arguments.argv())
        .env_clear()
        .env("LANG", "C")
        .env("PATH", "/usr/bin")
        .env("HOME", "/home/adapter")
        .env("TMPDIR", "/tmp")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null());
    let status = command.status().map_err(unavailable)?;
    if !status.success() {
        return Err(Error::ProviderUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_required_runtime_tool_fails_unavailable_without_fallback() {
        assert_eq!(
            super::check_helpers(&["/usr/bin/issue183-acp-missing"]),
            Err(crate::Error::ProviderUnavailable)
        );
    }
}
