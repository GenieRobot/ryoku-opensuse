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
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
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
        .env("PATH", "/usr/bin:/bin")
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_TARGET_DIR", "/var/tmp/ryoku-native24-cargo-target")
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
    println!("native_issue24_bootstrap_and_dependencies=passed");
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
    Ok(())
}
fn exercise() -> Result<()> {
    guard()?;
    command("/usr/bin/systemctl", &["isolate", "rescue.target"])?;
    command("/usr/bin/systemctl", &["start", "systemd-logind.service"])?;
    match fs::read_to_string(state("phase")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            retain_layers()?;
            apply("install", "native24-clean")?;
            verify_user(PERSONAL)?;
            compare_to_checkpoint("native24-clean-pre")?;
            apply("install", "native24-repeat")?;
            verify_user(PERSONAL)?;
            compare_to_checkpoint("native24-repeat-pre")?;
            let (uid, _) = account()?;
            let uid = uid.to_string();
            let digest = preview("install")?;
            refuses(&["migrate", &uid, "native24-conflict", &layer_id()?, &digest])?;
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
            refuses(&[
                "internal-lifecycle",
                "install",
                &uid,
                "native24-blocked",
                &layer_id()?,
                &preview("install")?,
            ])?;
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
        Some("--report") => {
            require(
                fs::read_to_string(state("result"))? == RESULTS,
                "installer acceptance incomplete",
            )?;
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
