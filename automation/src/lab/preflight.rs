//! Shared, read-only launch prerequisites. No run directory, disk or key is created.
use crate::{
    RunOptions,
    definition::scalar,
    process, runner,
    support::{Result, require, safe_component},
};
use rustix::fs::{Access, AtFlags, CWD, accessat, statvfs};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const INPUT_LIMIT: u64 = 256 * 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;
const LOADER: &str = "/lib64/ld-linux-x86-64.so.2";
const FIRMWARE: &[(&str, &str)] =
    &[("OVMF_CODE_4M.fd", "ovmf_code_sha256"), ("OVMF_VARS_4M.fd", "ovmf_variables_sha256")];

#[derive(Default)]
pub(crate) struct Checks(Vec<String>);
impl Checks {
    pub(crate) fn check<T>(
        &mut self,
        label: &str,
        action: impl FnOnce() -> Result<T>,
    ) -> Option<T> {
        match action() {
            Ok(value) => Some(value),
            Err(error) => {
                self.0.push(format!("{label}: {error}"));
                None
            }
        }
    }

    pub(crate) fn finish(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(format!("{} host prerequisite(s) failed:\n- {}", self.0.len(), self.0.join("\n- "))
                .into())
        }
    }
}

pub(crate) struct Inputs {
    pub(crate) assets: PathBuf,
    pub(crate) run: PathBuf,
    pub(crate) binaries: [Option<PathBuf>; 12],
    pub(crate) defaults_rpm: Option<PathBuf>,
}

/// Commands are bounded, produce no artifacts, and never invoke an input's main.
pub(crate) fn output(command: &mut Command) -> Result<String> {
    command.stdin(Stdio::null()).env("LC_ALL", "C");
    let result = process::run(command, Duration::from_secs(60), 64 * 1024)?;
    require(!result.timed_out, "read-only prerequisite command timed out")?;
    require(!result.stdout_truncated && !result.stderr_truncated, "prerequisite output too large")?;
    require(result.status.success(), "read-only prerequisite command failed")?;
    Ok(String::from_utf8(result.stdout)?)
}

fn executable_path(program: &Path) -> Result<PathBuf> {
    let path = if program.components().count() > 1 {
        program.to_owned()
    } else {
        std::env::split_paths(&std::env::var_os("PATH").ok_or("PATH is unavailable")?)
            .map(|directory| directory.join(program))
            .find(|path| accessat(CWD, path, Access::EXEC_OK, AtFlags::EACCESS).is_ok())
            .ok_or("required command is absent from PATH")?
    };
    let path = fs::canonicalize(path)?;
    require(path.is_file(), "command is not a regular file")?;
    accessat(CWD, &path, Access::READ_OK | Access::EXEC_OK, AtFlags::EACCESS)?;
    Ok(path)
}

/// Inspect ELF program headers ourselves, so dependency checks never select an
/// interpreter supplied by the input. Static x86_64 ELF binaries are supported.
fn interpreter(file: &mut File) -> Result<Option<String>> {
    let mut header = [0_u8; 64];
    file.read_exact(&mut header)?;
    require(
        &header[..7] == b"\x7fELF\x02\x01\x01"
            && [2, 3].contains(&u16::from_le_bytes(header[16..18].try_into()?))
            && u16::from_le_bytes(header[18..20].try_into()?) == 62,
        "requires a native x86_64 executable ELF input",
    )?;
    let offset = u64::from_le_bytes(header[32..40].try_into()?);
    let size = u16::from_le_bytes(header[54..56].try_into()?);
    let count = u16::from_le_bytes(header[56..58].try_into()?);
    require(size == 56 && count > 0 && count <= 1024, "unsupported ELF program headers")?;
    require(
        offset
            .checked_add(u64::from(size) * u64::from(count))
            .is_some_and(|end| end <= file.metadata().map(|m| m.len()).unwrap_or(0)),
        "truncated ELF program headers",
    )?;
    let mut found = None;
    for index in 0..count {
        file.seek(SeekFrom::Start(offset + u64::from(index) * u64::from(size)))?;
        let mut entry = [0_u8; 56];
        file.read_exact(&mut entry)?;
        if u32::from_le_bytes(entry[..4].try_into()?) != 3 {
            continue;
        }
        require(found.is_none(), "multiple ELF interpreters")?;
        let location = u64::from_le_bytes(entry[8..16].try_into()?);
        let length = u64::from_le_bytes(entry[32..40].try_into()?);
        require((2..=4096).contains(&length), "invalid ELF interpreter size")?;
        file.seek(SeekFrom::Start(location))?;
        let mut bytes = vec![0; length as usize];
        file.read_exact(&mut bytes)?;
        require(bytes.pop() == Some(0) && !bytes.contains(&0), "invalid ELF interpreter")?;
        let name = String::from_utf8(bytes)?;
        require(name == LOADER, "unsupported ELF interpreter")?;
        found = Some(name);
    }
    Ok(found)
}

fn native_dependencies(path: &Path, library_path: Option<&std::ffi::OsStr>) -> Result<()> {
    if interpreter(&mut File::open(path)?)?.is_some() {
        let mut command = Command::new(LOADER);
        command.env_clear().args(["--list"]).arg(path);
        if let Some(path) = library_path {
            command.env("LD_LIBRARY_PATH", path);
        }
        let listed = output(&mut command)?;
        require(!listed.contains("not found"), "native shared library is missing")?;
    }
    Ok(())
}

pub(crate) fn input_binary(path: &str) -> Result<(PathBuf, u64)> {
    // Input arguments are paths relative to cwd, never command names in PATH.
    let path = fs::canonicalize(path)?;
    accessat(CWD, &path, Access::READ_OK | Access::EXEC_OK, AtFlags::EACCESS)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as _)
        .open(&path)?;
    let metadata = file.metadata()?;
    require(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= INPUT_LIMIT,
        "invalid input binary size or kind",
    )?;
    native_dependencies(&path, None)?;
    Ok((path, metadata.len()))
}

fn fresh_directory(run: &Path) -> Result<PathBuf> {
    require(run.is_absolute(), "run directory must be absolute")?;
    let name = safe_component(run.file_name().and_then(|x| x.to_str()).ok_or("missing run name")?)?;
    let parent = fs::canonicalize(run.parent().ok_or("missing run parent")?)?;
    let destination = parent.join(name);
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err("run path already exists; use a fresh directory".into()),
    }
    accessat(CWD, &parent, Access::WRITE_OK | Access::EXEC_OK, AtFlags::EACCESS)?;
    Ok(destination)
}

fn memory_bytes(meminfo: &str) -> Result<u64> {
    let values: Vec<_> =
        meminfo.lines().filter_map(|line| line.strip_prefix("MemAvailable:")).collect();
    require(values.len() == 1, "MemAvailable is unavailable or ambiguous")?;
    let words: Vec<_> = values[0].split_whitespace().collect();
    require(words.len() == 2 && words[1] == "kB", "invalid MemAvailable units")?;
    words[0].parse::<u64>()?.checked_mul(1024).ok_or_else(|| "MemAvailable overflow".into())
}

fn headroom(available: u64, required: u64) -> Result<()> {
    require(
        available >= required,
        &format!("requires {required} available bytes; found {available}"),
    )
}

fn hash_matches(path: &Path, expected: &str) -> Result<()> {
    require(path.is_file(), "input is not a regular readable file")?;
    let checksum = output(Command::new("sha256sum").arg(path))?;
    require(checksum.split_whitespace().next() == Some(expected), "checksum mismatch")
}

/// Retained guests need the pinned host runtime, not a base image, ISO builder,
/// package payloads or provisioning inputs.
pub(crate) fn resume_tools(assets: Option<&Path>, checks: &mut Checks) {
    for program in ["/usr/bin/ssh", "/usr/bin/sha256sum"] {
        checks.check(&format!("resume host command {program}"), || {
            native_dependencies(&executable_path(Path::new(program))?, None)
        });
    }
    if let Some(assets) = assets {
        checks.check("resume QEMU dependencies", || {
            let command = runner::tool(assets, "qemu-system-x86_64");
            native_dependencies(&executable_path(Path::new(command.get_program()))?, None)
        });
        checks.check("resume QEMU version", || {
            let version = output(runner::tool(assets, "qemu-system-x86_64").arg("--version"))?;
            require(
                version.starts_with(&format!("QEMU emulator version {}", scalar("qemu")?)),
                "unexpected QEMU version",
            )
        });
        checks.check("resume immutable UEFI code", || {
            hash_matches(&runner::firmware(assets, "OVMF_CODE_4M.fd"), scalar("ovmf_code_sha256")?)
        });
    }
    checks.check("resume available memory (guest plus 1 GiB)", || {
        headroom(
            memory_bytes(&fs::read_to_string("/proc/meminfo")?)?,
            scalar("memory_mib")?.parse::<u64>()? * 1024 * 1024 + GIB,
        )
    });
}

pub(crate) fn inspect(assets: &str, run: &str, options: RunOptions<'_>) -> Result<Inputs> {
    let mut checks = Checks::default();
    checks.check("selected accelerator", || options.accelerator.unwrap_or_default().preflight());
    checks.check("fixture", || {
        require(
            scalar("architecture")? == "x86_64" && scalar("accelerator")? == "tcg",
            "unsupported fixture architecture or accelerator",
        )
    });
    let assets = checks.check("asset directory", || {
        let path = fs::canonicalize(assets)?;
        require(path.is_dir(), "assets must be a directory")?;
        Ok(path)
    });
    let parent = checks.check("run parent", || {
        Ok(fs::canonicalize(Path::new(run).parent().ok_or("missing run parent")?)?)
    });
    let run = checks.check("fresh run directory", || fresh_directory(Path::new(run)));
    // Continue checking independent guards even when a path cannot be resolved.
    runner::validate_host(assets.as_deref(), parent.as_deref(), &mut checks);

    for program in ["ssh", "scp", "ssh-keygen", "gpg", "sha256sum"] {
        checks.check(&format!("host command {program}"), || {
            native_dependencies(&executable_path(Path::new(program))?, None)
        });
    }
    if let Some(assets) = &assets {
        for program in ["qemu-system-x86_64", "qemu-img", "genisoimage"] {
            checks.check(&format!("runner command {program}"), || {
                let command = runner::tool(assets, program);
                let library_path = command.get_envs().find_map(|(name, value)| {
                    (name == "LD_LIBRARY_PATH").then_some(value).flatten()
                });
                native_dependencies(
                    &executable_path(Path::new(command.get_program()))?,
                    library_path,
                )
            });
        }
        checks.check("QEMU version", || {
            let version = output(runner::tool(assets, "qemu-system-x86_64").arg("--version"))?;
            require(
                version.starts_with(&format!("QEMU emulator version {}", scalar("qemu")?)),
                "unexpected QEMU version",
            )
        });
        checks.check("signed base image", || {
            hash_matches(&assets.join(scalar("image")?), scalar("sha256")?)
        });
        checks.check("signed checksum", || {
            let signed_sum = fs::read_to_string(assets.join("image.sha256"))?;
            require(
                signed_sum.split_whitespace().next() == Some(scalar("sha256")?),
                "signed checksum does not describe pinned base",
            )
        });
        for (file, key) in FIRMWARE {
            checks.check(&format!("firmware {file}"), || {
                hash_matches(&runner::firmware(assets, file), scalar(key)?)
            });
        }
        checks.check("release signature", || {
            require(assets.join("gnupg").is_dir(), "isolated existing GPG home is required")?;
            let signer = scalar("release_key_fingerprint")?;
            // The pinned signer below is the trust anchor. Disable key lookup,
            // trustdb/signature caches and locking so verification is read-only.
            let signature = output(
                Command::new("gpg")
                    .args([
                        "--no-options",
                        "--batch",
                        "--no-autostart",
                        "--lock-never",
                        "--no-auto-key-retrieve",
                        "--no-auto-key-import",
                        "--no-auto-check-trustdb",
                        "--no-random-seed-file",
                        "--no-sig-cache",
                        "--trust-model",
                        "always",
                        "--homedir",
                    ])
                    .arg(assets.join("gnupg"))
                    .args(["--status-fd", "1", "--verify"])
                    .arg(assets.join("image.sha256.asc"))
                    .arg(assets.join("image.sha256")),
            )?;
            require(
                signature
                    .lines()
                    .any(|line| line.starts_with(&format!("[GNUPG:] VALIDSIG {signer} "))),
                "unexpected release signer",
            )
        });
    }
    if let Some((_, package)) = options.zypp_probe {
        checks.check("zypp package", || {
            require(
                !package.is_empty()
                    && package.as_bytes()[0].is_ascii_alphanumeric()
                    && package
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.+".contains(&byte)),
                "invalid zypper probe package identifier",
            )
        });
    }
    let mut binaries: [Option<PathBuf>; 12] = Default::default();
    let mut staged_bytes = 0_u64;
    for (index, (name, path)) in [
        ("UI input", options.capture_ui),
        ("state probe", options.state_probe),
        ("initializer", options.state_initializer),
        ("state preparation", options.state_prepare),
        ("linked probe", options.linked_probe),
        ("recovery CLI", options.recovery_cli),
        ("zypp probe", options.zypp_probe.map(|(path, _)| path)),
        ("profile bundle probe", options.profile_bundle.map(|(path, _, _)| path)),
        ("service fixture", options.service_fixture.map(|(path, _, _)| path)),
        ("user daemon", options.service_fixture.map(|(_, path, _)| path)),
        ("root helper", options.service_fixture.map(|(_, _, path)| path)),
        ("UI user daemon", options.capture_user_service),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(path) = path
            && let Some((path, bytes)) = checks.check(name, || {
                let (path, bytes) = input_binary(path)?;
                require(
                    index < 8 || bytes <= 128 * 1024 * 1024,
                    "service fixture input exceeds guest bound",
                )?;
                Ok((path, bytes))
            })
        {
            binaries[index] = Some(path);
            staged_bytes += bytes;
        }
    }
    let defaults_rpm = options.profile_bundle.and_then(|(_, path, expected)| {
        checks.check("native RPM command", || {
            native_dependencies(&executable_path(Path::new("/usr/bin/rpm"))?, None)
        });
        checks.check("profile defaults RPM", || {
            let path = fs::canonicalize(path)?;
            let bytes = crate::profile_bundle::inspect_rpm(&path, expected)?;
            staged_bytes += bytes;
            Ok(path)
        })
    });
    checks.check("orchestrator native dependencies", || {
        native_dependencies(&fs::canonicalize("/proc/self/exe")?, None)
    });
    checks.check("available memory (guest plus 1 GiB)", || {
        headroom(
            memory_bytes(&fs::read_to_string("/proc/meminfo")?)?,
            scalar("memory_mib")?.parse::<u64>()? * 1024 * 1024 + GIB,
        )
    });
    if let Some(parent) = &parent {
        checks.check("available run-filesystem space (12 GiB plus staged inputs)", || {
            let stats = statvfs(parent)?;
            let available =
                stats.f_bavail.checked_mul(stats.f_frsize).ok_or("disk capacity overflow")?;
            headroom(available, 12 * GIB + staged_bytes)
        });
    }
    checks.finish()?;
    Ok(Inputs {
        assets: assets.ok_or("asset directory unavailable")?,
        run: run.ok_or("run directory unavailable")?,
        binaries,
        defaults_rpm,
    })
}

pub(crate) fn report(assets: &str, run: &str, options: RunOptions<'_>) -> Result<()> {
    let result = inspect(assets, run, options);
    println!(
        "host_preflight={}\nrunner={}\naccelerator={}\nguest_prerequisites=not_checked\nvm_runtime=unproven\nNo run directory, VM, disk or key was created",
        if result.is_ok() { "passed" } else { "failed" },
        if runner::WSL { "wsl" } else { "cloud" },
        options.accelerator.unwrap_or_default().name(),
    );
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "ryoku-preflight-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn freshness_checks_create_nothing_and_reject_dangling_symlinks() {
        let scratch = Scratch::new();
        let run = scratch.0.join("fresh");
        assert_eq!(fresh_directory(&run).unwrap(), run);
        assert!(!run.exists());
        symlink("absent", &run).unwrap();
        assert!(fresh_directory(&run).is_err());
        fs::remove_file(&run).unwrap();
        fs::create_dir(&run).unwrap();
        assert!(fresh_directory(&run).is_err());
        assert!(fresh_directory(Path::new("relative")).is_err());
    }

    #[test]
    fn memory_units_missing_values_and_capacity_boundaries_fail_closed() {
        assert_eq!(memory_bytes("MemAvailable:  1024 kB\n").unwrap(), 1024 * 1024);
        for invalid in [
            "MemFree: 100 kB",
            "MemAvailable: 10 MB",
            "MemAvailable: 10 kB\nMemAvailable: 20 kB",
            "MemAvailable: 18446744073709551615 kB",
        ] {
            assert!(memory_bytes(invalid).is_err());
        }
        assert!(headroom(12 * GIB, 12 * GIB).is_ok());
        assert!(headroom(12 * GIB - 1, 12 * GIB).is_err());
    }

    #[test]
    fn untrusted_interpreter_and_truncated_elf_are_rejected_before_execution() {
        let scratch = Scratch::new();
        let path = scratch.0.join("input");
        let mut bytes = vec![0_u8; 120];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&3_u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&120_u64.to_le_bytes());
        let bad = b"/tmp/arbitrary-interpreter\0";
        bytes[96..104].copy_from_slice(&(bad.len() as u64).to_le_bytes());
        bytes.extend_from_slice(bad);
        fs::write(&path, &bytes).unwrap();
        assert!(
            interpreter(&mut File::open(&path).unwrap())
                .unwrap_err()
                .to_string()
                .contains("unsupported ELF interpreter")
        );
        fs::write(&path, &bytes[..70]).unwrap();
        assert!(interpreter(&mut File::open(&path).unwrap()).is_err());
        fs::write(&path, b"not an executable").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(input_binary(path.to_str().unwrap()).is_err());
        assert!(input_binary(scratch.0.to_str().unwrap()).is_err());
        assert!(input_binary(scratch.0.join("missing").to_str().unwrap()).is_err());
    }

    #[test]
    fn independent_failures_are_aggregated_without_creating_a_run() {
        let scratch = Scratch::new();
        let run = scratch.0.join("fresh");
        let one = scratch.0.join("one");
        let two = scratch.0.join("two");
        let options = RunOptions {
            development_desktop: false,
            accelerator: None,
            capture_ui: one.to_str(),
            capture_user_service: None,
            state_probe: two.to_str(),
            state_initializer: None,
            state_prepare: None,
            linked_probe: None,
            recovery_cli: None,
            profile_bundle: None,
            service_fixture: None,
            zypp_probe: None,
            console_only: false,
        };
        let errors = inspect(
            scratch.0.join("assets-absent").to_str().unwrap(),
            run.to_str().unwrap(),
            options,
        )
        .err()
        .unwrap()
        .to_string();
        for label in ["asset directory", "UI input", "state probe"] {
            assert!(errors.contains(label));
        }
        assert!(!run.exists());
    }

    #[test]
    fn native_loader_can_list_this_binary_without_executing_its_test_main() {
        native_dependencies(&fs::canonicalize("/proc/self/exe").unwrap(), None).unwrap();
    }
}
