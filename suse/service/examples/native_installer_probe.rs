//! #24 disposable VM acceptance; every mutating entry checks the seeded QEMU
//! marker and independently provisioned rollback-resistant state first.
use ryoku_recovery::{
    Identifier, StateRoot,
    account_principal::AccountPrincipal,
    capture::{CaptureLimits, capture_declared},
    capture_session::NativeCaptureSession,
    generation::GenerationReader,
    installed_capture::CaptureAccount,
    installed_profiles::{InstalledProfileBundle, ProfileBundleStore},
};
use ryoku_service::{OperationId, native_mutation::ProtectedMutationHistory};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const STATE: &str = "/var/lib/ryoku-state";
const HOME: &str = "/home/tester";
const UNIT: &str = "ryoku-native24-exercise.service";
const PROBE: &str = "/usr/local/sbin/ryoku-native24-probe";
const INSTALLER: &str = "/usr/sbin/ryoku-install";
const DIRECTORY: &str = "/usr/local/share/ryoku-native24-packages";
const BUILD_DEPENDENCIES: &[&str] = &[
    "rpm",
    "rpm-build",
    "gpg2",
    "cargo",
    "rust",
    "go",
    "patch",
    "cmake",
    "ninja",
    "gcc-c++",
    "qt6-base-devel",
    "qt6-declarative-devel",
    "qt6-shadertools-devel",
    "qt6-multimedia-devel",
    "qml-autoreqprov",
    "qmlpluginexports-qt6",
    "systemd-rpm-macros",
    "pkgconf-pkg-config",
];
fn valid_signing_provider(value: &str) -> bool {
    value == "rpm-0:4.20.1-11.1.x86_64\n"
}
const SOURCES: &[&str] = &[
    "issue14-runtime-payload.tar.gz",
    "ryoku-desktop.spec",
    "ryoku-helper.spec",
    "ryoku-opensuse-0.1.0.tar.gz",
    "ryoku-opensuse-vendor-0.1.0.tar.xz",
    "ryoku-profile-defaults.spec",
    "ryoku-quickshell-0.3.2-2.x86_64.rpm",
    "ryoku-tools.spec",
    "ryoku-writers-98a0368.tar.xz",
];
const PACKAGES: &[&str] = &[
    "ryoku-desktop-0.1.0-7.x86_64.rpm",
    "ryoku-desktop-fonts-0.1.0-7.noarch.rpm",
    "ryoku-desktop-runtime-0.1.0-7.x86_64.rpm",
    "ryoku-helper-0.1.0-7.x86_64.rpm",
    "ryoku-profile-defaults-0.1.0-7.noarch.rpm",
    "ryoku-quickshell-0.3.2-2.x86_64.rpm",
    "ryoku-tools-0.1.0-7.x86_64.rpm",
    "ryoku-tools-state-init-0.1.0-7.x86_64.rpm",
];
#[cfg(test)]
const SELECTED: &[&str] = &[
    "ryoku-desktop",
    "ryoku-desktop-fonts",
    "ryoku-desktop-runtime",
    "ryoku-helper",
    "ryoku-profile-defaults",
    "ryoku-quickshell",
    "ryoku-tools",
];
const SYSTEM: &[u8] =
    br#"{"schema_version":1,"id":"generic-desktop","settings":{"desktop":"kde"}}"#;
const COMPANION: &[u8] =
    br#"{"schema_version":1,"id":"generic-desktop","settings":{"theme":"system"}}"#;
const PERSONAL: &[u8] =
    br#"{"schema_version":1,"id":"generic-desktop","settings":{"density":"compact"}}"#;
const SENTINEL: &[u8] = b"unrelated native24 sentinel\n";
const RESULTS: &str = "native_issue24_clean_install=passed\nnative_issue24_repeat_install=passed\nnative_issue24_versioned_migration=passed\nnative_issue24_remove_and_reinstall=passed\nnative_issue24_verified_prechange_checkpoints=passed\nnative_issue24_interrupted_before_apply=passed\nnative_issue24_interrupted_apply_linked_restore=passed\nnative_issue24_wrong_recovery_receipt_refused=passed\nnative_issue24_user_overrides_and_unrelated=passed\nnative_issue24_distro_and_kde_preserved=passed\n";
fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn state(name: &str) -> String {
    format!("{STATE}/native24-{name}")
}
fn id(name: &str) -> Result<Identifier> {
    Identifier::new(name).map_err(|_| "invalid fixture identifier".into())
}
fn marker(value: &str) -> Result<()> {
    let mut serial = OpenOptions::new().write(true).open("/dev/ttyS0")?;
    writeln!(serial, "\nnative_issue24_phase={value}")?;
    Ok(())
}
fn guard() -> Result<()> {
    require(rustix::process::geteuid().as_raw() == 0, "requires root fixture execution")?;
    require(
        fs::read_to_string("/etc/ryoku-lab-fixture")? == "disposable-issue13\n",
        "requires disposable fixture marker",
    )?;
    require(
        fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() == "QEMU",
        "requires owned QEMU guest",
    )?;
    let root = File::open("/")?;
    let record = ryoku_recovery::machine_state::read_machine_state_record(&root)?;
    ryoku_recovery::state_mount::collect_and_open_state_mount(&root, record.identity())?;
    Ok(())
}
fn command(program: &str, args: &[&str]) -> Result<String> {
    command_for(program, args, "180s")
}
fn command_for(program: &str, args: &[&str], timeout: &str) -> Result<String> {
    let output = Command::new("/usr/bin/timeout")
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_TARGET_DIR", "/var/tmp/ryoku-native24-cargo-target")
        .env("GOCACHE", "/var/tmp/ryoku-native24-go-cache")
        .env("GOPATH", "/var/tmp/ryoku-native24-go-workspace")
        .args(["--signal=TERM", "--kill-after=3s", timeout, program])
        .args(args)
        .output()?;
    if !output.status.success() {
        eprintln!(
            "native24 {program} {args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Err("native installer fixture command failed; inspect private log".into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
fn refuses(args: &[&str]) -> Result<()> {
    let output = Command::new(INSTALLER).args(args).output()?;
    eprintln!("native24 refusal {:?}: {}", args, String::from_utf8_lossy(&output.stderr));
    require(output.status.code() == Some(2), "installer did not refuse with exit 2")
}
fn write(path: &str, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(Path::new(path).parent().ok_or("missing parent")?)?.sync_all()?;
    Ok(())
}
fn phase(value: &str) -> Result<()> {
    let path = state("phase");
    let stage = format!("{path}.new");
    write(&stage, value.as_bytes(), 0o600)?;
    fs::rename(&stage, &path)?;
    File::open(STATE)?.sync_all()?;
    marker(value)
}
fn account() -> Result<(u32, u32)> {
    let text = command("/usr/bin/getent", &["passwd", "tester"])?;
    let fields: Vec<_> = text.trim().split(':').collect();
    require(fields.len() == 7 && fields[5] == HOME, "unexpected fixture account")?;
    Ok((fields[2].parse()?, fields[3].parse()?))
}
fn own(path: &str, uid: u32, gid: u32) -> Result<()> {
    rustix::fs::chown(
        path,
        Some(rustix::process::Uid::from_raw(uid)),
        Some(rustix::process::Gid::from_raw(gid)),
    )?;
    Ok(())
}
fn package_paths() -> Vec<String> {
    PACKAGES.iter().map(|name| format!("{DIRECTORY}/{name}")).collect()
}
fn selected_paths() -> Vec<String> {
    package_paths().into_iter().filter(|path| !path.contains("ryoku-tools-state-init-")).collect()
}
fn archive_names(listing: &str) -> Result<()> {
    let names: Vec<_> = listing.lines().collect();
    require(names == SOURCES, "package archive has unexpected paths/order")
}
fn rpms_below(path: &Path, results: &mut Vec<PathBuf>) -> Result<()> {
    for item in fs::read_dir(path)? {
        let item = item?;
        let metadata = item.file_type()?;
        if metadata.is_dir() {
            rpms_below(&item.path(), results)?;
        } else if metadata.is_file()
            && item.path().extension().is_some_and(|extension| extension == "rpm")
        {
            results.push(item.path());
        } else if !metadata.is_file() {
            return Err("unsupported dependency cache entry".into());
        }
    }
    Ok(())
}
fn setup() -> Result<()> {
    guard()?;
    require(
        ryoku_recovery::installer_state::query(None)?
            .iter()
            .all(|package| package.before_nevra.is_none()),
        "fresh target already contains Ryoku",
    )?;
    let source_directory = "/usr/local/share/ryoku-native24-sources";
    let listing = command(
        "/usr/bin/tar",
        &["--list", "--file", "/usr/local/sbin/ryoku-native24-packages.tar"],
    )?;
    archive_names(&listing)?;
    fs::create_dir(source_directory)?;
    command(
        "/usr/bin/tar",
        &[
            "--extract",
            "--file",
            "/usr/local/sbin/ryoku-native24-packages.tar",
            "--directory",
            source_directory,
            "--no-same-owner",
            "--no-same-permissions",
            "--keep-old-files",
        ],
    )?;
    let source = fs::read_to_string("/usr/local/sbin/ryoku-native24-source.txt")?;
    require(
        source.trim().len() == 40 && source.trim().bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid exact source commit",
    )?;
    let provider = command(
        "/usr/bin/rpm",
        &["-qf", "--qf", "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n", "/usr/bin/rpmsign"],
    )?;
    require(
        valid_signing_provider(&provider),
        "rpmsign owner differs from authenticated pinned rpm provider",
    )?;
    let mut dependencies = vec!["--non-interactive", "--no-refresh", "install", "--no-recommends"];
    dependencies.extend_from_slice(BUILD_DEPENDENCIES);
    command_for("/usr/bin/zypper", &dependencies, "1800s")?;
    let caches = command("/usr/bin/go", &["env", "GOCACHE", "GOPATH"])?;
    require(
        caches == "/var/tmp/ryoku-native24-go-cache\n/var/tmp/ryoku-native24-go-workspace\n",
        "native Go cache/workspace preflight failed",
    )?;
    let top = "/var/tmp/ryoku-native24-rpm";
    fs::create_dir(top)?;
    for spec in [
        "ryoku-desktop.spec",
        "ryoku-helper.spec",
        "ryoku-profile-defaults.spec",
        "ryoku-tools.spec",
    ] {
        marker(&format!("build-{spec}"))?;
        command_for(
            "/usr/bin/rpmbuild",
            &[
                "-ba",
                "--define",
                &format!("_topdir {top}"),
                "--define",
                &format!("_sourcedir {source_directory}"),
                "--define",
                "_smp_mflags -j2",
                &format!("{source_directory}/{spec}"),
            ],
            "1800s",
        )?;
    }
    fs::create_dir(DIRECTORY)?;
    for name in PACKAGES {
        let arch = if name.ends_with(".noarch.rpm") { "noarch" } else { "x86_64" };
        let path = if name.starts_with("ryoku-quickshell-") {
            format!("{source_directory}/{name}")
        } else {
            format!("{top}/RPMS/{arch}/{name}")
        };
        fs::copy(path, format!("{DIRECTORY}/{name}"))?;
    }
    let keyhome = "/var/tmp/ryoku-native24-signing";
    fs::create_dir(keyhome)?;
    fs::set_permissions(keyhome, fs::Permissions::from_mode(0o700))?;
    command_for(
        "/usr/bin/gpg",
        &[
            "--homedir",
            keyhome,
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "",
            "--quick-generate-key",
            "Ryoku issue24 disposable fixture <issue24@example.invalid>",
            "rsa2048",
            "sign",
            "1d",
        ],
        "180s",
    )?;
    let keys = command("/usr/bin/gpg", &["--homedir", keyhome, "--with-colons", "--list-keys"])?;
    let fingerprints = keys
        .lines()
        .filter(|line| line.starts_with("fpr:"))
        .filter_map(|line| line.split(':').nth(9))
        .collect::<Vec<_>>();
    require(fingerprints.len() == 1, "ambiguous disposable signing key")?;
    let fingerprint = fingerprints[0];
    let public =
        command("/usr/bin/gpg", &["--homedir", keyhome, "--armor", "--export", fingerprint])?;
    write("/var/tmp/ryoku-native24-signing.asc", public.as_bytes(), 0o644)?;
    command("/usr/bin/rpm", &["--import", "/var/tmp/ryoku-native24-signing.asc"])?;
    let mut hashes = String::new();
    for path in package_paths() {
        command(
            "/usr/bin/rpmsign",
            &[
                "--define",
                &format!("_gpg_path {keyhome}"),
                "--define",
                &format!("_gpg_name {fingerprint}"),
                "--addsign",
                &path,
            ],
        )?;
        hashes.push_str(&command("/usr/bin/sha256sum", &[&path])?);
    }
    write(&state("package-hashes"), hashes.as_bytes(), 0o600)?;
    command("/usr/bin/gpgconf", &["--homedir", keyhome, "--kill", "gpg-agent"])?;
    fs::remove_dir_all(keyhome)?;
    let paths = package_paths();
    for path in &paths {
        let result = command("/usr/bin/rpm", &["--checksig", path])?;
        require(result.contains("signatures OK"), "fixture RPM signature missing")?;
    }
    let cache = "/var/cache/ryoku-native24-dependencies";
    fs::create_dir(cache)?;
    let mut args = vec![
        "--non-interactive",
        "--pkg-cache-dir",
        cache,
        "install",
        "--download-only",
        "--no-recommends",
    ];
    args.extend(paths.iter().map(String::as_str));
    command_for("/usr/bin/zypper", &args, "1200s")?;
    let mut deps = Vec::new();
    rpms_below(Path::new(cache), &mut deps)?;
    deps.sort();
    let mut dependency_paths = Vec::new();
    for path in &deps {
        let path = path.to_str().ok_or("invalid cached package path")?;
        let name = command("/usr/bin/rpm", &["-qp", "--qf", "%{NAME}", path])?;
        if !name.starts_with("ryoku-") {
            dependency_paths.push(path);
        }
    }
    if !dependency_paths.is_empty() {
        let mut args = vec!["-U", "--replacepkgs"];
        args.extend(dependency_paths);
        command_for("/usr/bin/rpm", &args, "1200s")?;
    }
    require(
        ryoku_recovery::installer_state::query(None)?
            .iter()
            .all(|package| package.before_nevra.is_none()),
        "dependency preparation installed a Ryoku payload",
    )?;
    command(
        "/usr/bin/rpm",
        &[
            "-U",
            &format!("{DIRECTORY}/ryoku-tools-0.1.0-7.x86_64.rpm"),
            &format!("{DIRECTORY}/ryoku-tools-state-init-0.1.0-7.x86_64.rpm"),
        ],
    )?;
    let installed = ryoku_recovery::installer_state::query(None)?;
    for package in installed {
        require(
            package.before_nevra.is_some()
                == matches!(
                    package.package_name.as_str(),
                    "ryoku-tools" | "ryoku-tools-state-init"
                ),
            "bootstrap contains runtime packages",
        )?;
    }
    command("/usr/bin/systemctl", &["daemon-reload"])?;
    let (uid, gid) = account()?;
    for relative in [".config", ".config/ryoku", "Documents"] {
        let path = format!("{HOME}/{relative}");
        if !Path::new(&path).exists() {
            fs::create_dir(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            own(&path, uid, gid)?;
        }
    }
    write(&format!("{HOME}/Documents/native24-unrelated"), SENTINEL, 0o640)?;
    own(&format!("{HOME}/Documents/native24-unrelated"), uid, gid)?;
    write(&format!("{HOME}/.config/ryoku/opensuse-profile.json"), PERSONAL, 0o600)?;
    own(&format!("{HOME}/.config/ryoku/opensuse-profile.json"), uid, gid)?;
    let legacy = ryoku_profiles::ProfileDocument::new(
        ryoku_profiles::ProfileId::new("generic-desktop")?,
        ryoku_profiles::SettingsLayer::default(),
    )
    .canonical_bytes()?;
    write(&format!("{HOME}/.config/ryoku/profile.json"), &legacy, 0o644)?;
    own(&format!("{HOME}/.config/ryoku/profile.json"), uid, gid)?;
    write(&format!("/etc/systemd/system/{UNIT}"),format!("[Unit]\nDescription=Disposable issue24 protected installer acceptance\nDefaultDependencies=no\nAfter=local-fs.target var-lib-ryoku\\x2dstate.mount\nBefore=shutdown.target umount.target\nConflicts=shutdown.target\nIgnoreOnIsolate=yes\n[Service]\nType=oneshot\nExecStart={PROBE} --exercise\nExecStopPost={PROBE} --stopped\nTimeoutStartSec=30min\nStandardOutput=append:{STATE}/native24-private.log\nStandardError=append:{STATE}/native24-private.log\n[Install]\nWantedBy=multi-user.target\n").as_bytes(),0o644)?;
    command("/usr/bin/systemctl", &["daemon-reload"])?;
    unnumbered_refusal()?;
    bootstrap_numbered_root()?;
    println!("native_issue24_bootstrap_and_dependencies=passed");
    Ok(())
}
fn unnumbered_refusal() -> Result<()> {
    require(
        command("/usr/bin/findmnt", &["-n", "-o", "FSROOT", "/"])?.trim() == "/@",
        "unsupported-layout test requires original unnumbered root",
    )?;
    let history = fs::read(format!("{STATE}/mutations.log")).ok();
    let snapshots = command("/usr/bin/snapper", &["--jsonout", "--config", "root", "list"])?;
    let packages = command("/usr/bin/rpm", &["-qa"])?;
    let result = Command::new(INSTALLER).arg("remove-preview").output()?;
    require(
        result.status.code() == Some(2)
            && String::from_utf8_lossy(&result.stderr).contains("unsupported recovery layout"),
        "unnumbered root was not rejected by actual installer preflight",
    )?;
    require(
        fs::read(format!("{STATE}/mutations.log")).ok() == history
            && command("/usr/bin/snapper", &["--jsonout", "--config", "root", "list"])?
                == snapshots
            && command("/usr/bin/rpm", &["-qa"])? == packages,
        "unsupported root refusal changed journal, checkpoints or packages",
    )?;
    let proof = format!(
        "unnumbered=refused\nsnapshots_sha256={:x}\npackages_sha256={:x}\njournal_sha256={:x}\n",
        Sha256::digest(snapshots.as_bytes()),
        Sha256::digest(packages.as_bytes()),
        Sha256::digest(history.as_deref().unwrap_or_default()),
    );
    layout_proof("unnumbered-refused", proof.as_bytes())?;
    Ok(())
}
fn layout_proof(name: &str, measured: &[u8]) -> Result<()> {
    // Reboot revalidates the layout; retain the original successful proof.
    match fs::read(state(name)) {
        Ok(bytes) => require(bytes == measured, "existing layout proof differs"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write(&state(name), measured, 0o600)
        }
        Err(error) => Err(error.into()),
    }
}
// The signed Minimal-VM derivation is initially an unnumbered direct root.
// Provision the same supported classic Snapper boot layout as the #21 fixture;
// this administrative fixture setup is separate from installer acceptance.
fn bootstrap_numbered_root() -> Result<()> {
    require(
        command("/usr/bin/findmnt", &["-n", "-o", "FSROOT", "/"])?.trim() == "/@",
        "bootstrap requires the fresh directly mounted root",
    )?;
    let fstab = fs::read_to_string("/etc/fstab")?;
    require(
        !fstab.lines().any(|line| line.split_whitespace().nth(1) == Some("/.snapshots")),
        "bootstrap refuses an existing snapshot mount",
    )?;
    let root = fstab
        .lines()
        .find(|line| line.split_whitespace().nth(1) == Some("/"))
        .ok_or("root mount missing")?;
    let device = root.split_whitespace().next().ok_or("root source missing")?;
    require(device.starts_with("UUID="), "unbound fixture root device")?;
    fs::write(
        "/etc/fstab",
        format!("{fstab}{device} /.snapshots btrfs defaults,subvol=@/.snapshots 0 0\n"),
    )?;
    command("/usr/bin/mount", &["/.snapshots"])?;
    let grub = fs::read_to_string("/etc/default/grub")?;
    let mut grub = grub
        .lines()
        .filter(|line| !line.trim_start().starts_with("SUSE_BTRFS_SNAPSHOT_BOOTING="))
        .collect::<Vec<_>>()
        .join("\n");
    grub.push_str("\nSUSE_BTRFS_SNAPSHOT_BOOTING=\"true\"\n");
    fs::write("/etc/default/grub", grub)?;
    command("/usr/sbin/grub2-mkconfig", &["-o", "/boot/grub2/grub.cfg"])?;
    command(
        "/usr/sbin/grub2-install",
        &[
            "--target=x86_64-efi",
            "--efi-directory=/boot/efi",
            "--boot-directory=/boot",
            "--removable",
            "--no-nvram",
        ],
    )?;
    let source = command(
        "/usr/bin/snapper",
        &["--config", "root", "create", "--type", "single", "--read-only", "--print-number"],
    )?;
    let source = source.trim();
    require(
        source.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == source),
        "invalid bootstrap source",
    )?;
    let target = command(
        "/usr/bin/snapper",
        &[
            "--quiet",
            "--ambit",
            "classic",
            "--config",
            "root",
            "rollback",
            "--print-number",
            source,
        ],
    )?;
    require(
        target.trim().parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == target.trim()),
        "invalid bootstrap rollback target",
    )?;
    write(&state("bootstrap-target"), target.as_bytes(), 0o600)?;
    command("/usr/bin/sync", &[])?;
    Ok(())
}
fn verify_numbered_root() -> Result<()> {
    let root = command("/usr/bin/findmnt", &["-n", "-o", "FSROOT", "/"])?;
    let number = root
        .trim()
        .strip_prefix("/@/.snapshots/")
        .and_then(|path| path.strip_suffix("/snapshot"))
        .ok_or("root did not boot numbered snapshot")?;
    require(
        number.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == number),
        "noncanonical numbered root",
    )?;
    let inventory = command(
        "/usr/bin/snapper",
        &[
            "--jsonout",
            "--config",
            "root",
            "list",
            "--columns",
            "number,type,default,active,read-only",
        ],
    )?;
    let inventory: serde_json::Value = serde_json::from_str(&inventory)?;
    let rows = inventory["root"].as_array().ok_or("root inventory missing")?;
    let defaults = rows.iter().filter(|row| row["default"] == true).collect::<Vec<_>>();
    let active = rows.iter().filter(|row| row["active"] == true).collect::<Vec<_>>();
    require(
        defaults.len() == 1
            && active.len() == 1
            && defaults[0]["number"].as_u64().map(|n| n.to_string()).as_deref() == Some(number)
            && defaults[0] == active[0]
            && defaults[0]["read-only"] == false,
        "running root is not sole writable numbered Snapper default",
    )?;
    require(
        command("/usr/bin/findmnt", &["-n", "-o", "FSROOT", "--mountpoint", "/.snapshots"])?.trim()
            == "/@/.snapshots",
        "snapshot tree not mounted",
    )?;
    if fs::read_to_string(state("phase")).is_err() {
        require(
            fs::read_to_string(state("bootstrap-target"))?.trim() == number,
            "booted root differs from bootstrap target",
        )?;
    }
    let uuid = command("/usr/bin/findmnt", &["-n", "-o", "UUID", "/"])?;
    require(
        !uuid.trim().is_empty()
            && command("/usr/bin/findmnt", &["-n", "-o", "UUID", "--mountpoint", "/.snapshots"])?
                .trim()
                == uuid.trim(),
        "snapshot tree filesystem differs",
    )?;
    let grub = fs::read_to_string("/etc/default/grub")?;
    require(
        grub.lines()
            .map(str::trim)
            .filter(|line| line.starts_with("SUSE_BTRFS_SNAPSHOT_BOOTING="))
            .collect::<Vec<_>>()
            == ["SUSE_BTRFS_SNAPSHOT_BOOTING=\"true\""],
        "snapshot boot GRUB configuration differs",
    )?;
    let proof = format!(
        "running_root={}\nfilesystem_uuid={}\ndefault=sole-active-writable\nsnapshot_tree=/@/.snapshots\ngrub_sha256={:x}\n",
        root.trim(),
        uuid.trim(),
        Sha256::digest(grub.as_bytes())
    );
    layout_proof(&format!("numbered-layout-{number}"), proof.as_bytes())?;
    Ok(())
}
fn layer_id() -> Result<String> {
    Ok(fs::read_to_string(state("layers"))?)
}
fn retain_layers() -> Result<()> {
    let (uid, gid) = account()?;
    let session = NativeCaptureSession::open(
        CaptureAccount {
            uid,
            gid,
            principal: AccountPrincipal::from_account(uid, gid, "tester", HOME)
                .map_err(|error| format!("{error:?}"))?,
        },
        File::open(HOME)?,
    )?;
    let bundle = InstalledProfileBundle::without_installed_defaults(Some(SYSTEM), Some(COMPANION))?;
    let verified = session.publish_installer_bundle(&id("native24-layers")?, &bundle)?;
    write(&state("layers"), verified.id().as_str().as_bytes(), 0o600)?;
    session.finish()?;
    Ok(())
}
fn preview(action: &str) -> Result<String> {
    marker(&format!("preview-{action}"))?;
    let (uid, _) = account()?;
    let uid = uid.to_string();
    let layers = layer_id()?;
    let paths = selected_paths();
    let mut args = vec!["lifecycle-preview", action, &uid, &layers];
    if action == "install" {
        args.extend(paths.iter().map(String::as_str));
    }
    let result = command(INSTALLER, &args)?;
    eprintln!("preview {action}: {result}");
    let digest = result
        .split_whitespace()
        .find_map(|field| field.strip_prefix("plan_sha256="))
        .ok_or("review digest missing")?;
    require(digest.len() == 64, "invalid review digest")?;
    Ok(digest.into())
}
fn apply(action: &str, operation: &str) -> Result<()> {
    let digest = preview(action)?;
    let (uid, _) = account()?;
    let uid = uid.to_string();
    let layers = layer_id()?;
    let paths = selected_paths();
    let mut args = vec![action, &uid, operation, &layers, &digest];
    if action == "install" {
        args.extend(paths.iter().map(String::as_str));
    }
    marker(operation)?;
    command_for(INSTALLER, &args, "1200s")?;
    verify_checkpoints(operation)?;
    Ok(())
}
fn verify_checkpoints(operation: &str) -> Result<()> {
    let root = File::open("/")?;
    let reader = GenerationReader::open_state_mount()?;
    for suffix in ["pre", "post"] {
        let linked =
            reader.open_historical_transaction(&root, &id(&format!("{operation}-{suffix}"))?)?;
        let catalog = ryoku_recovery::snapper_catalog::ProtectedSnapperCatalog::open(&root)?;
        let cataloged = catalog.status(&id(&format!("{operation}-{suffix}"))?)?;
        require(
            cataloged.complete && cataloged.cleanup_owned && cataloged.entry.manual_checkpoint,
            "installer checkpoint lacks complete retained catalog partner",
        )?;
        let manifest = linked.generation().manifest();
        require(
            ryoku_recovery::installer_state::is_installer(&manifest.reason),
            "checkpoint lost installer marker",
        )?;
        ryoku_recovery::installer_state::validate_inventory(manifest.package_revisions())?;
        require(
            manifest.files().len() == ryoku_recovery::installed_inventory::PREFERENCES.len(),
            "checkpoint omitted managed paths",
        )?;
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(manifest.files())?));
        let mut comparisons =
            OpenOptions::new().append(true).create(true).mode(0o600).open(state("comparisons"))?;
        writeln!(comparisons, "{operation}\t{suffix}\t{}\t{digest}", manifest.files().len())?;
        comparisons.sync_all()?;
        let bundle = ProfileBundleStore::open_state_mount()?.load_revision(
            &manifest
                .profile_revision
                .as_ref()
                .ok_or("missing profile provenance")?
                .profile_revision,
        )?;
        require(
            bundle.bundle().system_source_bytes() == Some(SYSTEM)
                && bundle.bundle().companion_source_bytes() == Some(COMPANION),
            "profile ownership layers changed",
        )?;
        let resolved =
            ProfileBundleStore::open_state_mount()?.resolve_generation(linked.generation())?;
        require(
            resolved.settings.desktop == Some(ryoku_profiles::Desktop::Kde)
                && resolved.settings.theme == Some(ryoku_profiles::Theme::System),
            "profile resolution changed",
        )?;
    }
    Ok(())
}
fn verify_user(expected_personal: &[u8]) -> Result<()> {
    require(
        fs::read(format!("{HOME}/.config/ryoku/opensuse-profile.json"))? == expected_personal,
        "personal override changed",
    )?;
    require(
        fs::read(format!("{HOME}/Documents/native24-unrelated"))? == SENTINEL,
        "unrelated user data changed",
    )
}
fn catalog_refusals() -> Result<()> {
    guard()?;
    let path = Path::new(STATE).join("snapper-catalog");
    require(
        matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "catalog refusal fixture requires fresh bootstrap",
    )?;
    require(!Path::new(&state("catalog-refusals")).exists(), "catalog fixture already completed")?;
    let sentinel = Path::new(STATE).join("native24-catalog-sentinel");
    write(sentinel.to_str().ok_or("invalid sentinel path")?, SENTINEL, 0o600)?;
    let baseline = ryoku_recovery::installer_state::query(None)?;
    let (uid, gid) = account()?;
    let uid_arg = uid.to_string();
    for case in ["corrupt", "symlink", "foreign-owner", "unsafe-mode", "regular-file"] {
        marker(&format!("catalog-refusal-{case}"))?;
        match case {
            "symlink" => std::os::unix::fs::symlink(&sentinel, &path)?,
            "regular-file" => write(path.to_str().ok_or("invalid catalog path")?, SENTINEL, 0o600)?,
            _ => {
                fs::create_dir(&path)?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                write(
                    path.join("catalog.json").to_str().ok_or("invalid catalog path")?,
                    b"corrupt\n",
                    0o600,
                )?;
                write(
                    path.join(".catalog.lock").to_str().ok_or("invalid catalog lock")?,
                    b"",
                    0o600,
                )?;
                if case == "foreign-owner" {
                    own(path.to_str().ok_or("invalid catalog path")?, uid, gid)?;
                } else if case == "unsafe-mode" {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
                }
            }
        }
        let before = fs::symlink_metadata(&path)?;
        let operation = format!("native24-catalog-{case}");
        let digest = preview("install")?;
        let layers = layer_id()?;
        let paths = selected_paths();
        let mut args =
            vec!["internal-lifecycle", "install", &uid_arg, &operation, &layers, &digest];
        args.extend(paths.iter().map(String::as_str));
        refuses(&args)?;
        let after = fs::symlink_metadata(&path)?;
        require(
            (before.dev(), before.ino(), before.uid(), before.gid(), before.mode())
                == (after.dev(), after.ino(), after.uid(), after.gid(), after.mode()),
            "catalog refusal changed the existing entry",
        )?;
        if case == "symlink" {
            require(fs::read_link(&path)? == sentinel, "catalog symlink changed")?;
        } else if case == "regular-file" {
            require(fs::read(&path)? == SENTINEL, "catalog file changed")?;
        } else {
            require(fs::read(path.join("catalog.json"))? == b"corrupt\n", "catalog bytes changed")?;
        }
        require(fs::read(&sentinel)? == SENTINEL, "catalog target changed")?;
        require(
            ryoku_recovery::installer_state::query(None)? == baseline,
            "catalog refusal changed packages",
        )?;
        verify_user(PERSONAL)?;
        {
            use ryoku_service::{
                OperationPhase,
                mutation_gate::{MutationOutcome, MutationStage},
            };
            let history = ProtectedMutationHistory::open().map_err(|e| format!("{e:?}"))?;
            let operation = OperationId::new(&operation)?;
            let status =
                history.journal().get(&operation).ok_or("missing catalog refusal status")?;
            let context = history
                .journal()
                .mutation_context(&operation)
                .ok_or("missing catalog refusal history")?;
            require(
                status.phase == OperationPhase::Failed
                    && context.stage == MutationStage::Capturing
                    && context.checkpoint.is_none()
                    && context.outcome == Some(MutationOutcome::FailedBeforeChange),
                "catalog refusal passed the pre-change gate",
            )?;
        }
        let journal_before = fs::read(format!("{STATE}/mutations.log"))?;
        for _ in 0..2 {
            command(INSTALLER, &["recover", &uid_arg, &operation, "none"])?;
        }
        require(
            fs::read(format!("{STATE}/mutations.log"))? == journal_before,
            "idempotent native recovery rewrote history",
        )?;
        if after.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    fs::remove_file(sentinel)?;
    write(&state("catalog-refusals"), b"passed\n", 0o600)?;
    Ok(())
}
fn compare_to_checkpoint(transaction: &str) -> Result<()> {
    let reader = GenerationReader::open_state_mount()?;
    let source = reader.open_verified_transaction(&id(transaction)?)?;
    let manifest = source.manifest();
    let paths = manifest.files().iter().map(|record| record.path.clone()).collect::<Vec<_>>();
    let actual = capture_declared(
        &File::open(HOME)?,
        StateRoot::UserHome,
        &paths,
        CaptureLimits::default(),
    )?;
    require(actual == manifest.files(), "restored managed file bytes/metadata/absence differ")
}
fn interrupted(operation: &str, stage: &str) -> Result<()> {
    let digest = preview("install")?;
    let (uid, _) = account()?;
    let uid = uid.to_string();
    let layers = layer_id()?;
    let paths = selected_paths();
    let mut args = vec!["install", &uid, operation, &layers, &digest];
    args.extend(paths.iter().map(String::as_str));
    marker(operation)?;
    let mut child =
        Command::new(INSTALLER).args(&args).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut hit = false;
    while Instant::now() < deadline {
        let text = fs::read_to_string(format!("{STATE}/mutations.log")).unwrap_or_default();
        let latest = text
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok());
        if latest.as_ref().is_some_and(|record| {
            record["status"]["operation_id"] == operation && record["mutation"]["stage"] == stage
        }) {
            command(
                "/usr/bin/systemctl",
                &[
                    "kill",
                    "--kill-whom=all",
                    "--signal=KILL",
                    &format!("ryoku-installer-{operation}.service"),
                ],
            )?;
            hit = true;
            break;
        }
        if child.try_wait()?.is_some() {
            break;
        }
        thread::sleep(Duration::from_micros(100));
    }
    if !hit {
        let _ = child.kill();
    }
    let _ = child.wait()?;
    require(hit, "did not interrupt the actual installer at requested durable phase")?;
    let history = ProtectedMutationHistory::open().map_err(|e| format!("{e:?}"))?;
    let op = OperationId::new(operation)?;
    require(
        history.journal().recovery_required().iter().any(|status| status.operation_id == op),
        "interrupted operation was incorrectly clear",
    )?;
    drop(history);
    require_emergency()?;
    record_target(&format!("{operation} stage={stage} emergency=active recovery_required=true"))?;
    enter_maintenance()
}
fn record_target(line: &str) -> Result<()> {
    let mut file =
        OpenOptions::new().append(true).create(true).mode(0o600).open(state("failure-targets"))?;
    writeln!(file, "{line}")?;
    file.sync_all()?;
    Ok(())
}
fn public_conflict(operation: &str) -> Result<()> {
    let (uid, _) = account()?;
    let uid = uid.to_string();
    let digest = preview("install")?;
    let packages_before = command("/usr/bin/rpm", &["-qa"])?;
    let journal_before = fs::read(format!("{STATE}/mutations.log"))?;
    let paths = ryoku_recovery::installed_inventory::PREFERENCES
        .iter()
        .map(|path| ryoku_recovery::RelativePath::new(*path))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| "invalid compiled preference path")?;
    let unrelated_path = format!("{HOME}/Documents/native24-unrelated");
    let unrelated = fs::read(&unrelated_path)?;
    let unrelated_before = fs::symlink_metadata(&unrelated_path)?;
    let before = capture_declared(
        &File::open(HOME)?,
        StateRoot::UserHome,
        &paths,
        CaptureLimits::default(),
    )?;
    refuses(&["migrate", &uid, operation, &layer_id()?, &digest])?;
    require(
        unit_active("rescue.target")? && !unit_active("emergency.target")?,
        "preflight conflict disrupted maintenance targets",
    )?;
    require(
        command("/usr/bin/rpm", &["-qa"])? == packages_before,
        "preflight conflict changed package inventory",
    )?;
    require(
        fs::read(format!("{STATE}/mutations.log"))? == journal_before,
        "preflight conflict changed protected mutation history",
    )?;
    require(
        capture_declared(
            &File::open(HOME)?,
            StateRoot::UserHome,
            &paths,
            CaptureLimits::default(),
        )? == before,
        "preflight conflict changed user bytes metadata or absence",
    )?;
    let unrelated_after = fs::symlink_metadata(&unrelated_path)?;
    let metadata = |value: &fs::Metadata| {
        (
            value.dev(),
            value.ino(),
            value.uid(),
            value.gid(),
            value.mode(),
            value.len(),
            value.nlink(),
            value.mtime(),
            value.mtime_nsec(),
            value.ctime(),
            value.ctime_nsec(),
        )
    };
    require(
        unrelated_before.is_file()
            && fs::read(&unrelated_path)? == unrelated
            && metadata(&unrelated_before) == metadata(&unrelated_after),
        "preflight conflict changed unrelated file bytes or metadata",
    )?;
    record_target(&format!(
        "{operation} preflight=refused rescue=active emergency=inactive packages=unchanged user_inventory=unchanged"
    ))
}
fn failure_policy() -> Result<()> {
    guard()?;
    enter_maintenance()?;
    let personal = format!("{HOME}/.config/ryoku/opensuse-profile.json");
    if !Path::new(&personal).try_exists()? {
        write(&personal, PERSONAL, 0o600)?;
        let (uid, gid) = account()?;
        own(&personal, uid, gid)?;
    }
    verify_user(PERSONAL)?;
    public_conflict("native24-policy-conflict")?;
    interrupted("native24-policy-after", "health_checking")?;
    write(&state("failure-policy"), b"preflight-preserved post-gate-emergency\n", 0o600)?;
    Ok(())
}
fn unit_active(unit: &str) -> Result<bool> {
    Ok(Command::new("/usr/bin/timeout")
        .args(["2s", "/usr/bin/systemctl", "is-active", unit])
        .output()?
        .status
        .success())
}
fn require_emergency() -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if unit_active("emergency.target")? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("post-gate installer failure did not activate emergency target".into())
}
fn enter_maintenance() -> Result<()> {
    command("/usr/bin/systemctl", &["isolate", "rescue.target"])?;
    command("/usr/bin/systemctl", &["start", "systemd-logind.service"])?;
    if !Path::new(&format!("/run/user/{}/ryoku-writers", account()?.0)).exists() {
        bootstrap_account_namespace()?;
    }
    Ok(())
}
fn bootstrap_account_namespace() -> Result<()> {
    let (uid, gid) = account()?;
    let history_before = match fs::read(format!("{STATE}/mutations.log")) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    marker("account-writer-bootstrap")?;
    refuses(&["account-bootstrap"])?;
    let runtime_unit = format!("user-runtime-dir@{uid}.service");
    command("/usr/bin/systemctl", &["stop", &format!("user@{uid}.service"), &runtime_unit])?;
    require(
        !Path::new(&format!("/run/user/{uid}")).exists(),
        "runtime negative precondition missing",
    )?;
    let uid_arg = format!("--reuid={uid}");
    let gid_arg = format!("--regid={gid}");
    let prefix = [&*uid_arg, &*gid_arg, "--init-groups", INSTALLER];
    for action in [vec!["account-bootstrap"], vec!["account-bootstrap", "extra"]] {
        let mut args = prefix.to_vec();
        args.extend(action);
        let status = Command::new("/usr/bin/setpriv").args(args).output()?;
        require(status.status.code() == Some(2), "unsafe bootstrap was not refused")?;
    }
    command("/usr/bin/systemctl", &["start", &runtime_unit])?;
    let mut args = prefix.to_vec();
    args.push("account-bootstrap");
    let initialized = command("/usr/bin/setpriv", &args)?;
    require(
        initialized
            == format!(
                "account_writer_namespace=initialized target_uid={uid} mutation=not_performed\n"
            ),
        "account bootstrap evidence differs",
    )?;
    let entries = fs::read_dir(format!("/run/user/{uid}/ryoku-writers"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    require(
        entries == [std::ffi::OsString::from("writers.lock")],
        "bootstrap created extra state",
    )?;
    require(
        fs::read(format!("{STATE}/mutations.log")).ok() == history_before,
        "bootstrap changed mutation history",
    )?;
    Ok(())
}
fn exercise() -> Result<()> {
    guard()?;
    // Fresh bootstrap has no desktop writer process to initialize its private
    // namespace. Keep the shared capture lease contract and explicitly enroll
    // the target account through its fixed own-account installer command.
    verify_numbered_root()?;
    enter_maintenance()?;
    match fs::read_to_string(state("phase")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            retain_layers()?;
            catalog_refusals()?;
            apply("install", "native24-clean")?;
            verify_user(PERSONAL)?;
            compare_to_checkpoint("native24-clean-pre")?;
            apply("install", "native24-repeat")?;
            verify_user(PERSONAL)?;
            compare_to_checkpoint("native24-repeat-pre")?;
            let (uid, _) = account()?;
            let uid = uid.to_string();
            public_conflict("native24-conflict")?;
            verify_user(PERSONAL)?;
            fs::remove_file(format!("{HOME}/.config/ryoku/opensuse-profile.json"))?;
            apply("migrate", "native24-migrate")?;
            let legacy = fs::read(format!("{HOME}/.config/ryoku/profile.json"))?;
            verify_user(&legacy)?;
            apply("migrate", "native24-migrate-again")?;
            verify_user(&legacy)?;
            apply("remove", "native24-remove")?;
            verify_user(&legacy)?;
            compare_to_checkpoint("native24-remove-pre")?;
            apply("remove", "native24-remove-again")?;
            apply("install", "native24-reinstall")?;
            verify_user(&legacy)?;
            apply("remove", "native24-before-interrupt")?;
            interrupted("native24-before", "revalidating")?;
            let layers = layer_id()?;
            let digest = preview("install")?;
            let paths = selected_paths();
            let mut blocked =
                vec!["internal-lifecycle", "install", &uid, "native24-blocked", &layers, &digest];
            blocked.extend(paths.iter().map(String::as_str));
            refuses(&blocked)?;
            command(INSTALLER, &["recover", &uid, "native24-before", "none"])?;
            interrupted("native24-after", "health_checking")?;
            require(
                ryoku_recovery::installer_state::query(None)?.iter().any(|package| {
                    package.package_name.as_str() == "ryoku-desktop"
                        && package.before_nevra.is_some()
                }),
                "Applying interruption did not observe installed desktop",
            )?;
            refuses(&["recover", &uid, "native24-after", "none"])?;
            refuses(&["recover", &uid, "native24-after", "nonexistent-restore"])?;
            marker("prepare-linked")?;
            command(
                "/usr/bin/ryoku-recovery",
                &["prepare-linked", "native24-linked", "native24-after-pre"],
            )?;
            phase("restore")?;
            command("/usr/bin/ryoku-recovery", &["rollback-linked", "native24-linked"])?;
            Ok(())
        }
        Ok(value) if value == "restore" => {
            marker("confirm-linked")?;
            command("/usr/bin/ryoku-recovery", &["confirm-rollback", "native24-linked"])?;
            command("/usr/bin/ryoku-recovery", &["restore-configuration", "native24-linked"])?;
            compare_to_checkpoint("native24-after-pre")?;
            let (uid, _) = account()?;
            let uid = uid.to_string();
            refuses(&["recover", &uid, "native24-after", "native24-clean"])?;
            command(INSTALLER, &["recover", &uid, "native24-after", "native24-linked"])?;
            apply("install", "native24-recovered-reinstall")?;
            let legacy = fs::read(format!("{HOME}/.config/ryoku/profile.json"))?;
            verify_user(&legacy)?;
            command(
                "/usr/bin/zypper",
                &["--non-interactive", "--no-refresh", "verify", "--dry-run"],
            )?;
            command("/usr/bin/systemctl", &["disable", UNIT])?;
            phase("complete")?;
            write(&state("result"), RESULTS.as_bytes(), 0o600)?;
            marker("complete")?;
            command("/usr/bin/systemctl", &["isolate", "multi-user.target"])?;
            command("/usr/bin/systemctl", &["start", "sshd.service"])?;
            Ok(())
        }
        Ok(value) if value == "complete" => Ok(()),
        _ => Err("unknown persisted installer fixture phase".into()),
    }
}
fn run() -> Result<()> {
    guard()?;
    match std::env::args().nth(1).as_deref() {
        Some("--install") => setup(),
        Some("--arm") => {
            command("/usr/bin/systemctl", &["enable", UNIT])?;
            Ok(())
        }
        Some("--exercise") => exercise(),
        Some("--catalog-refusals") => catalog_refusals(),
        Some("--failure-policy") => failure_policy(),
        Some("--unsupported-layout") => unnumbered_refusal(),
        Some("--bootstrap-numbered") => bootstrap_numbered_root(),
        Some("--verify-numbered") => verify_numbered_root(),
        Some("--report") => {
            verify_numbered_root()?;
            require(
                fs::read(state("unnumbered-refused"))?
                    .starts_with(b"unnumbered=refused\nsnapshots_sha256="),
                "supported-layout and refusal proof missing",
            )?;
            require(
                fs::read(state("catalog-refusals"))? == b"passed\n",
                "catalog bootstrap refusal proof missing",
            )?;
            require(
                fs::read_to_string(state("result"))? == RESULTS,
                "installer acceptance incomplete",
            )?;
            let targets = fs::read_to_string(state("failure-targets"))?;
            for required in [
                "native24-conflict preflight=refused rescue=active emergency=inactive packages=unchanged user_inventory=unchanged",
                "native24-before stage=revalidating emergency=active recovery_required=true",
                "native24-after stage=health_checking emergency=active recovery_required=true",
            ] {
                require(
                    targets.lines().filter(|line| *line == required).count() == 1,
                    "native failure-target proof incomplete",
                )?;
            }
            print!("{RESULTS}");
            Ok(())
        }
        Some("--stopped") => {
            let phase = fs::read_to_string(state("phase")).unwrap_or_default();
            if phase == "restore" || phase == "complete" {
                Ok(())
            } else {
                Err("installer exercise stopped before reboot handoff or completion".into())
            }
        }
        _ => Err("unknown native installer fixture command".into()),
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("native24 fixture failed: {error}");
        // The host stops the failed VM as soon as it sees the serial marker.
        // Make the regular-file stderr diagnostic durable before that marker.
        let _ = rustix::fs::fsync(std::io::stderr());
        let _ = OpenOptions::new()
            .write(true)
            .open("/dev/ttyS0")
            .and_then(|mut file| writeln!(file, "\nnative_issue24_result=failed"));
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_tool_cache_environment_survives_cleared_parent_environment() {
        let environment = command("/usr/bin/env", &[]).unwrap();
        let values = environment
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(values.get("GOCACHE"), Some(&"/var/tmp/ryoku-native24-go-cache"));
        assert_eq!(values.get("GOPATH"), Some(&"/var/tmp/ryoku-native24-go-workspace"));
        assert_eq!(values.get("CARGO_TARGET_DIR"), Some(&"/var/tmp/ryoku-native24-cargo-target"));
        // Native GRUB snapshot scripts look up btrfs by name. Only fixed
        // distribution administrator/binary directories may participate.
        assert_eq!(values.get("PATH"), Some(&"/usr/sbin:/usr/bin:/sbin:/bin"));
        assert!(!values.contains_key("HOME"));
    }
    #[test]
    fn native_sdk_uses_authenticated_rpm_signing_provider() {
        assert!(BUILD_DEPENDENCIES.contains(&"rpm"));
        assert!(!BUILD_DEPENDENCIES.contains(&"rpm-sign"));
        assert!(valid_signing_provider("rpm-0:4.20.1-11.1.x86_64\n"));
        for wrong in [
            "rpm-sign-0:4.20.1-11.1.x86_64\n",
            "rpm-0:4.20.1-9.1.x86_64\n",
            "rpm-0:4.20.1-11.1.aarch64\n",
            "",
        ] {
            assert!(!valid_signing_provider(wrong));
        }
    }
    #[test]
    fn archive_is_exact_and_closed() {
        archive_names(&format!("{}\n", SOURCES.join("\n"))).unwrap();
        assert!(archive_names("../ryoku-tools.rpm\n").is_err());
        let mut reversed = SOURCES.to_vec();
        reversed.reverse();
        assert!(archive_names(&reversed.join("\n")).is_err());
        assert_eq!(SELECTED.len() + 1, PACKAGES.len());
    }
}
