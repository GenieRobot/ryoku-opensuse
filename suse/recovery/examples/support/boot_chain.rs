//! Read-only observations for this disposable fixture, never a boot verdict.
//! Six private records fit below the harness's 1 MiB stdout cap. Commands have
//! a 15-second deadline within a 90-second observation budget; regular assets
//! use 64 KiB streaming hashes capped at 512 MiB. Missing tools, unsupported
//! layouts, truncated output and time limits remain explicit unknowns.
//! No boot variables, GRUB files, snapshots or archive members are written.
use rustix::{
    fs::{Mode, OFlags, ResolveFlags, openat2},
    process::{Pid, Signal, kill_process_group},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const TEXT_LIMIT: usize = 32 * 1024;
const COMMAND_LIMIT: usize = 128 * 1024;
const ASSET_LIMIT: u64 = 512 * 1024 * 1024;
const FACT_LIMIT: usize = 128 * 1024;
const EFI_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
const PACKAGES: &[&str] = &[
    "rpm",
    "coreutils",
    "grub2-common",
    "grub2-x86_64-efi",
    "dracut",
    "efibootmgr",
    "3cpio",
    "kernel-default",
    "kernel-default-base",
    "cpio",
    "gzip",
    "xz",
    "zstd",
    "bzip2",
    "lz4",
    "lzop",
];

fn unavailable(reason: impl ToString) -> Value {
    json!({"status": "unproven", "reason": reason.to_string()})
}

fn emit(category: &str, observation: Value) {
    let mut fact = json!({"category": category, "observation": observation}).to_string();
    if fact.len() > FACT_LIMIT {
        fact =
            json!({"category": category, "observation": unavailable("evidence limit")}).to_string();
    }
    println!("boot_chain_fact={fact}");
}

fn read_regular(path: &Path) -> io::Result<File> {
    let root = File::open("/")?;
    let relative = path.strip_prefix("/").map_err(io::Error::other)?;
    let file = File::from(openat2(
        root,
        relative,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    Ok(file)
}

fn small_file(path: &Path) -> Value {
    let result = (|| -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        read_regular(path)?.take(TEXT_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > TEXT_LIMIT {
            return Err(io::Error::other("file exceeds text limit"));
        }
        Ok(bytes)
    })();
    match result {
        Ok(bytes) => json!({"path": path, "sha256": format!("{:x}", Sha256::digest(&bytes)),
            "content": String::from_utf8_lossy(&bytes)}),
        Err(error) => json!({"path": path, "observation": unavailable(error)}),
    }
}

fn hash_asset(path: &Path, deadline: Instant) -> Value {
    let result = (|| -> io::Result<(u64, String)> {
        let mut file = read_regular(path)?;
        let before = file.metadata()?;
        if before.len() > ASSET_LIMIT {
            return Err(io::Error::other("asset exceeds 512 MiB limit"));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut count = 0;
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::other("observation time limit"));
            }
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > ASSET_LIMIT {
                return Err(io::Error::other("asset grew beyond limit"));
            }
            hash.update(&buffer[..n]);
        }
        let after = file.metadata()?;
        if count != before.len()
            || before.len() != after.len()
            || (before.mtime(), before.mtime_nsec()) != (after.mtime(), after.mtime_nsec())
            || (before.ctime(), before.ctime_nsec()) != (after.ctime(), after.ctime_nsec())
        {
            return Err(io::Error::other("asset changed during observation"));
        }
        Ok((count, format!("{:x}", hash.finalize())))
    })();
    match result {
        Ok((size, sha256)) => json!({"path": path, "size": size, "sha256": sha256}),
        Err(error) => json!({"path": path, "observation": unavailable(error)}),
    }
}

fn executable(path: &str) -> bool {
    read_regular(Path::new(path))
        .and_then(|file| file.metadata())
        .is_ok_and(|meta| meta.mode() & 0o111 != 0)
}

/// Native process group deadline and bounded pipes; no shell, temporary files,
/// source execution, or dependency on a not-yet-inventoried timeout command.
fn command(program: &str, args: &[&str], deadline: Instant) -> Value {
    let result = (|| -> io::Result<Value> {
        if Instant::now() >= deadline || !executable(program) {
            return Err(io::Error::other("deadline or regular executable unavailable"));
        }
        let mut child = Command::new(program)
            .args(args)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin")
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let drain = |reader: Box<dyn Read + Send>| {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                reader.take(COMMAND_LIMIT as u64 + 1).read_to_end(&mut bytes).map(|_| bytes)
            })
        };
        let stdout =
            drain(Box::new(child.stdout.take().ok_or_else(|| io::Error::other("stdout"))?));
        let stderr =
            drain(Box::new(child.stderr.take().ok_or_else(|| io::Error::other("stderr"))?));
        let end = deadline.min(Instant::now() + Duration::from_secs(15));
        let mut timed_out = false;
        let mut reaped = None;
        let status = loop {
            if reaped.is_none() {
                reaped = child.try_wait()?;
            }
            if let Some(status) = reaped
                && stdout.is_finished()
                && stderr.is_finished()
            {
                break status;
            }
            if Instant::now() >= end {
                timed_out = true;
                if let Some(pid) = Pid::from_raw(child.id() as i32) {
                    let _ = kill_process_group(pid, Signal::KILL);
                }
                break match reaped {
                    Some(status) => status,
                    None => {
                        child.kill()?;
                        child.wait()?
                    }
                };
            }
            thread::sleep(Duration::from_millis(20));
        };
        let stdout = stdout.join().map_err(|_| io::Error::other("stdout reader"))??;
        let stderr = stderr.join().map_err(|_| io::Error::other("stderr reader"))??;
        let truncated = stdout.len() > COMMAND_LIMIT || stderr.len() > COMMAND_LIMIT;
        let success = status.success() && !timed_out && !truncated;
        Ok(json!({"success": success, "status": if success {"observed"} else {"unproven"},
            "exit_code": status.code(), "timed_out": timed_out, "truncated": truncated,
            "stdout": String::from_utf8_lossy(&stdout), "stderr": String::from_utf8_lossy(&stderr)}))
    })();
    json!({"program": program, "arguments": args,
        "result": result.unwrap_or_else(unavailable)})
}

fn successful_stdout(observation: &Value) -> Option<&str> {
    let result = observation.get("result")?;
    (result.get("success")?.as_bool()? && result.get("stderr")?.as_str()?.is_empty())
        .then(|| result.get("stdout")?.as_str())
        .flatten()
}

fn inventory(deadline: Instant) -> (Value, BTreeMap<&'static str, String>) {
    let mut args = vec!["-q", "--qf", "%{NAME}=%{EVR}.%{ARCH}\n"];
    args.extend(PACKAGES);
    let versions = command("/usr/bin/rpm", &args, deadline);
    let mut observations = vec![versions];
    let mut tools = BTreeMap::new();
    for (package, name) in [
        ("grub2-common", "grub2-editenv"),
        ("dracut", "lsinitrd"),
        ("efibootmgr", "efibootmgr"),
        ("3cpio", "3cpio"),
    ] {
        let mut listing = command(
            "/usr/bin/rpm",
            &["-q", "--qf", "%{NAME}=%{EVR}.%{ARCH}\n[%{FILENAMES}\n]", package],
            deadline,
        );
        if let Some(list) = successful_stdout(&listing) {
            if list
                .lines()
                .next()
                .is_some_and(|version| version.starts_with(&format!("{package}=")))
            {
                for prefix in ["/usr/bin", "/usr/sbin"] {
                    let path = format!("{prefix}/{name}");
                    if list.lines().any(|line| line == path) && executable(&path) {
                        tools.insert(name, path);
                        break;
                    }
                }
            }
            // Preserve exact version and candidate paths without unrelated
            // package files crowding out the remaining observations.
            let selected = list
                .lines()
                .enumerate()
                .filter_map(|(n, line)| {
                    (n == 0
                        || line == format!("/usr/bin/{name}")
                        || line == format!("/usr/sbin/{name}"))
                    .then_some(line)
                })
                .collect::<Vec<_>>()
                .join("\n");
            listing["result"]["stdout"] = json!(selected);
            listing["result"]["unrelated_package_paths_omitted"] = json!(true);
        }
        observations.push(listing);
    }
    (
        json!({"packages": observations, "discovered_readers": tools,
        "lsinitrd_policy": "not invoked: even display modes may write temporary files"}),
        tools,
    )
}

fn entries(path: &Path, limit: usize) -> io::Result<Vec<PathBuf>> {
    // Do not traverse a symlink, including any parent component.
    let root = File::open("/")?;
    let _directory = openat2(
        root,
        path.strip_prefix("/").map_err(io::Error::other)?,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?;
    let mut paths = fs::read_dir(path)?
        .take(limit + 1)
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    if paths.len() > limit {
        return Err(io::Error::other("directory entry limit"));
    }
    paths.sort();
    Ok(paths)
}

fn efi_files(deadline: Instant) -> Value {
    let result = (|| -> io::Result<Vec<Value>> {
        let mut files = Vec::new();
        let mut paths = entries(Path::new("/boot/efi/EFI"), 16)?;
        for path in paths.clone() {
            if fs::symlink_metadata(&path)?.is_dir() {
                paths.extend(entries(&path, 64_usize.saturating_sub(paths.len()))?);
            }
        }
        for path in paths {
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                files.push(json!({"directory": path}));
            } else if metadata.is_file() {
                if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cfg")) {
                    files.push(small_file(&path));
                } else {
                    files.push(hash_asset(&path, deadline));
                }
            } else {
                files.push(
                    json!({"path": path, "observation": unavailable("nonregular EFI entry")}),
                );
            }
        }
        Ok(files)
    })();
    json!({"files": result.map_or_else(unavailable, |files| json!(files)),
        "active_loader_selection": "unproven; file presence is not execution evidence"})
}

fn firmware_variables() -> Value {
    let root = Path::new("/sys/firmware/efi/efivars");
    let result = (|| -> io::Result<Vec<Value>> {
        let mut values = Vec::new();
        for path in entries(root, 256)? {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if !boot_variable(name) {
                continue;
            }
            let mut bytes = Vec::new();
            read_regular(&path)?.take(4097).read_to_end(&mut bytes)?;
            if bytes.len() > 4096 || values.len() == 32 {
                return Err(io::Error::other("boot variable evidence limit"));
            }
            let hex = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            values.push(json!({"path": path, "raw_hex_including_attributes": hex}));
        }
        Ok(values)
    })();
    result.map_or_else(unavailable, |values| json!({"variables": values, "selection": "unproven"}))
}

fn boot_variable(name: &str) -> bool {
    let Some(prefix) = name.strip_suffix(&format!("-{EFI_GUID}")) else {
        return false;
    };
    matches!(prefix, "BootCurrent" | "BootOrder" | "BootNext")
        || prefix
            .strip_prefix("Boot")
            .is_some_and(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn alias_target(alias: &str, release: &str, base: &str) -> io::Result<PathBuf> {
    let target = fs::canonicalize(alias)?;
    if target != Path::new(&format!("/boot/{base}-{release}"))
        && target != Path::new(&format!("/usr/lib/modules/{release}/{base}"))
    {
        return Err(io::Error::other("alias outside exact running-kernel boot targets"));
    }
    Ok(target)
}

fn initramfs(tool: Option<&String>, initrd: Option<&Path>, deadline: Instant) -> Value {
    let (Some(tool), Some(initrd)) = (tool, initrd) else {
        return unavailable(
            "embedded cmdline needs an RPM-listed 3cpio stdout reader and a validated initrd; no packages installed",
        );
    };
    let path = initrd.to_str().unwrap_or("");
    let listing = command(
        tool,
        &[
            "--list",
            path,
            "etc/cmdline",
            "etc/cmdline.d/*.conf",
            "squash-root.img",
            "squashfs-root.img",
            "erofs-root.img",
        ],
        deadline,
    );
    let mut contents = Vec::new();
    if let Some(stdout) = successful_stdout(&listing) {
        let matches: Vec<_> = stdout.lines().filter(|line| cmdline_member(line)).collect();
        if matches.len() <= 16 {
            for member in matches {
                contents.push(command(tool, &["--extract", "--to-stdout", path, member], deadline));
            }
        } else {
            contents.push(unavailable("too many embedded cmdline members"));
        }
    }
    json!({"listing": listing, "cmdline_members": contents,
        "absence_of_root_pinning": "unproven; only selected plain CPIO members are inspected; nested images, symlinks and other boot logic are not assessed"})
}

fn cmdline_member(path: &str) -> bool {
    path == "etc/cmdline"
        || path
            .strip_prefix("etc/cmdline.d/")
            .is_some_and(|name| safe_component(name) && name.ends_with(".conf"))
}

pub(super) fn observe() {
    let deadline = Instant::now() + Duration::from_secs(90);
    let (packages, tools) = inventory(deadline);
    emit("inventory", packages);
    emit("efi_files", efi_files(deadline));
    emit(
        "firmware_variables",
        json!({"raw": firmware_variables(), "efibootmgr":
        tools.get("efibootmgr").map_or_else(|| unavailable("RPM-listed efibootmgr unavailable"),
            |tool| command(tool, &["-v"], deadline))}),
    );
    emit(
        "grub",
        json!({"config": small_file(Path::new("/boot/grub2/grub.cfg")),
        "environment": small_file(Path::new("/boot/grub2/grubenv")), "environment_list":
        tools.get("grub2-editenv").filter(|_| read_regular(Path::new("/boot/grub2/grubenv")).is_ok())
            .map_or_else(|| unavailable("RPM-listed grub2-editenv or nonsymlink grubenv unavailable"),
            |tool| command(tool, &["/boot/grub2/grubenv", "list"], deadline))}),
    );
    let mut release = String::new();
    if let Ok(file) = read_regular(Path::new("/proc/sys/kernel/osrelease"))
        && file.take(130).read_to_string(&mut release).is_err()
    {
        release.clear();
    }
    let release = release.trim();
    let mut assets = Vec::new();
    let mut initrd = None;
    if safe_component(release) {
        for base in ["vmlinuz", "initrd"] {
            let alias = format!("/boot/{base}");
            // A missing alias does not prevent observing the exact release path.
            let target = alias_target(&alias, release, base);
            assets.push(json!({"alias": alias, "target": target.as_ref().map_or_else(
                unavailable, |path| json!(path))}));
            let path = target.unwrap_or_else(|_| PathBuf::from(format!("/boot/{base}-{release}")));
            let hash = hash_asset(&path, deadline);
            if base == "initrd" && hash.get("sha256").is_some() {
                initrd = Some(path);
            }
            assets.push(hash);
        }
    } else {
        assets.push(unavailable("running kernel release unavailable or invalid"));
    }
    emit("kernel_assets", json!({"running_release": release, "assets": assets}));
    emit("initramfs", initramfs(tools.get("3cpio"), initrd.as_deref(), deadline));
    println!("native_linked_boot_chain_observed=recorded");
    println!("native_linked_root_switch=unproven");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_boot_variables_and_plain_cmdline_members_are_selected() {
        for name in ["BootCurrent", "BootOrder", "BootNext", "Boot000A"] {
            assert!(boot_variable(&format!("{name}-{EFI_GUID}")));
        }
        for name in ["Boot", "Boot12345", "BootZZZZ", "SecureBoot"] {
            assert!(!boot_variable(&format!("{name}-{EFI_GUID}")));
        }
        for path in ["etc/cmdline", "etc/cmdline.d/95root-dev.conf"] {
            assert!(cmdline_member(path));
        }
        for path in [
            "/etc/cmdline",
            "etc/cmdline.d/../secret.conf",
            "etc/cmdline.d/*.conf",
            "etc/cmdline.d/a/b.conf",
            "squashfs-root/etc/cmdline",
            "etc/cmdline\nother",
        ] {
            assert!(!cmdline_member(path));
        }
    }

    #[test]
    fn failed_partial_or_stderr_output_never_establishes_inventory_or_archive_listing() {
        for result in [
            json!({"success": false, "stdout": "/usr/bin/3cpio", "stderr": ""}),
            json!({"success": true, "stdout": "etc/cmdline", "stderr": "warning"}),
            unavailable("missing tool"),
        ] {
            assert!(successful_stdout(&json!({"result": result})).is_none());
        }
        assert_eq!(
            successful_stdout(&json!({"result": {"success": true, "stdout": "ok", "stderr": ""}})),
            Some("ok")
        );
        assert_eq!(initramfs(None, None, Instant::now())["status"], "unproven");
    }

    #[test]
    fn command_time_and_output_limits_remain_unproven() {
        let started = Instant::now();
        let timed = command("/usr/bin/sleep", &["30"], started + Duration::from_millis(100));
        assert_eq!(timed["result"]["timed_out"], true);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(successful_stdout(&timed).is_none());
        let large =
            command("/usr/bin/seq", &["1", "200000"], Instant::now() + Duration::from_secs(5));
        assert_eq!(large["result"]["truncated"], true);
        assert!(large["result"]["stdout"].as_str().unwrap().len() <= COMMAND_LIMIT + 1);
        assert!(successful_stdout(&large).is_none());
    }

    #[test]
    fn symlinks_nonregular_files_and_expired_hashes_are_not_read_as_assets() {
        assert!(read_regular(Path::new("/proc/self/exe")).is_err());
        assert!(read_regular(Path::new("/dev/zero")).is_err());
        assert!(alias_target("/proc/self/exe", "fixture-release", "initrd").is_err());
        let expired = hash_asset(Path::new("/usr/bin/rpm"), Instant::now());
        assert!(expired.get("sha256").is_none());
        assert_eq!(expired["observation"]["status"], "unproven");
    }
}
