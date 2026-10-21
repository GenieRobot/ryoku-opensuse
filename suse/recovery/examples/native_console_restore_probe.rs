//! #21 acceptance probe. It refuses every non-disposable host before writing.
//! The only normal-session executable here is a Rust marker fixture; actual
//! desktop/login acceptance belongs to the separate first-sprint gate.
use ryoku_recovery::{
    Identifier, StateRoot,
    capture::{CaptureLimits, capture_declared},
    generation::GenerationReader,
    restore::{RestoreJournalStore, RestorePhase},
    session_admission::{DROPIN, DROPIN_PATH, SERVICE, SERVICE_PATH},
};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const STATE: &str = "/var/lib/ryoku-state";
const CLI: &str = "/usr/bin/ryoku-recovery";
const PROBE: &str = "/usr/local/sbin/ryoku-native21-probe";
const UNIT: &str = "ryoku-native21-exercise.service";
const HOME: &str = "/home/tester";
const SOURCE: &str = "native21-source";
const CONFIG: &str = "native21-config";
const LINKED: &str = "native21-linked";
const SYSTEM: &str = "native21-system";
const PACKAGES: &[&str] = &[
    "ryoku-desktop-0.1.0-5.x86_64.rpm",
    "ryoku-desktop-fonts-0.1.0-5.noarch.rpm",
    "ryoku-desktop-runtime-0.1.0-5.x86_64.rpm",
    "ryoku-helper-0.1.0-5.x86_64.rpm",
    "ryoku-profile-defaults-0.1.0-5.noarch.rpm",
    "ryoku-quickshell-0.3.2-2.x86_64.rpm",
    "ryoku-tools-0.1.0-5.x86_64.rpm",
];
const RESULTS: &str = "native_issue21_offline_cli=passed\nnative_issue21_configuration_metadata_absence=passed\nnative_issue21_unrelated_home=passed\nnative_issue21_interrupted_apply_resume=passed\nnative_issue21_pending_graphical_admission_refused=passed\nnative_issue21_linked_root_reboot_resume=passed\nnative_issue21_system_only_scope=passed\nnative_issue21_current_checkpoint_retained=passed\nnative_issue21_bad_archive_refused=passed\nnative_issue21_unavailable_partner_refused=passed\nnative_issue21_incompatible_packages=unclaimed\nnative_issue21_complete_graphical_admission=passed\nnative_issue21_provider_actions=not_exercised\nnative_issue21_desktop_login=unclaimed\n";

fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn id(value: &str) -> Result<Identifier> {
    Identifier::new(value).map_err(|_| "invalid fixed fixture identity".into())
}
fn guard() -> Result<()> {
    require(rustix::process::geteuid().as_raw() == 0, "root fixture mode required")?;
    require(
        fs::read_to_string("/etc/ryoku-lab-fixture")? == "disposable-issue13\n",
        "requires the seeded disposable fixture",
    )?;
    require(
        fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() == "QEMU",
        "requires the owned QEMU guest",
    )?;
    let root = File::open("/")?;
    let record = ryoku_recovery::machine_state::read_machine_state_record(&root)?;
    ryoku_recovery::state_mount::collect_and_open_state_mount(&root, record.identity())?;
    Ok(())
}
fn command(program: &str, args: &[&str]) -> Result<String> {
    command_for(program, args, "120s")
}
fn command_for(program: &str, args: &[&str], timeout: &str) -> Result<String> {
    let out = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=3s", timeout, program])
        .args(args)
        .env("LC_ALL", "C")
        .output()?;
    if !out.status.success() {
        eprintln!(
            "fixture command failed: {program} {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return Err("native console fixture command failed; inspect private log".into());
    }
    Ok(String::from_utf8(out.stdout)?)
}
fn cli(args: &[&str]) -> Result<String> {
    serial_phase(args[0])?;
    let result = command(CLI, args)?;
    eprintln!("console {args:?}: {}", result.trim());
    serial_complete(args[0])?;
    Ok(result)
}
fn serial_line(line: &str) -> Result<()> {
    let mut serial = OpenOptions::new().write(true).open("/dev/ttyS0")?;
    writeln!(serial, "\n{line}")?;
    Ok(())
}
fn serial_complete(operation: &str) -> Result<()> {
    serial_line(&format!("native_issue21_completed={operation}"))
}
fn serial_phase(operation: &str) -> Result<()> {
    require(
        matches!(
            operation,
            "admit-session"
                | "manual-checkpoint"
                | "verify-linked"
                | "plan"
                | "prepare-configuration"
                | "restore-configuration"
                | "prepare-linked"
                | "rollback-linked"
                | "confirm-rollback"
                | "prepare-system"
                | "rollback-system"
                | "complete-system"
                | "offline"
                | "admission-refused"
                | "interrupted-restore"
                | "refusal-cases"
        ),
        "unrecognized fixture console operation",
    )?;
    serial_line(&format!("native_issue21_operation={operation}"))
}
fn refuses(args: &[&str]) -> Result<()> {
    let out = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=3s", "120s", CLI])
        .args(args)
        .output()?;
    require(out.status.code() == Some(2), "recovery command did not produce a bounded refusal")
}
fn systemctl(args: &[&str]) -> Result<String> {
    command("/usr/bin/systemctl", args)
}
fn write_new(path: &str, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(Path::new(path).parent().ok_or("fixture parent missing")?)?.sync_all()?;
    Ok(())
}
fn state_path(name: &str) -> String {
    format!("{STATE}/{name}")
}
fn phase(value: &str) -> Result<()> {
    let path = state_path("native21-phase");
    let mut file =
        OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    File::open(STATE)?.sync_all()?;
    Ok(())
}
fn offline() -> Result<()> {
    serial_phase("offline")?;
    systemctl(&["isolate", "rescue.target"])?;
    systemctl(&["start", "systemd-logind.service"])?;
    require(
        systemctl(&["is-active", "rescue.target"])?.trim() == "active",
        "rescue target inactive",
    )?;
    serial_complete("offline")
}

fn exercise_service() -> String {
    format!(
        "[Unit]\nDescription=Disposable issue21 offline console exercise\nDefaultDependencies=no\nAfter=local-fs.target var-lib-ryoku\\x2dstate.mount\nBefore=shutdown.target umount.target\nConflicts=shutdown.target\nIgnoreOnIsolate=yes\n[Service]\nType=oneshot\nExecStart={PROBE} --exercise\nExecStopPost={PROBE} --service-stopped\nTimeoutStartSec=15min\nTimeoutStopSec=15s\nStandardOutput=append:{STATE}/native21-private.log\nStandardError=append:{STATE}/native21-private.log\n[Install]\nWantedBy=multi-user.target\n"
    )
}
fn account() -> Result<(u32, u32)> {
    Ok((
        command("/usr/bin/id", &["-u", "tester"])?.trim().parse()?,
        command("/usr/bin/id", &["-g", "tester"])?.trim().parse()?,
    ))
}
fn own(path: &str) -> Result<()> {
    let (uid, gid) = account()?;
    rustix::fs::chown(
        path,
        Some(rustix::fs::Uid::from_raw(uid)),
        Some(rustix::fs::Gid::from_raw(gid)),
    )?;
    Ok(())
}
fn user_file(relative: &str, bytes: &[u8], mode: u32) -> Result<()> {
    let path = format!("{HOME}/{relative}");
    write_new(&path, bytes, mode)?;
    own(&path)
}
fn validate_packages(listing: &str) -> Result<()> {
    let mut names = Vec::new();
    for line in listing.lines() {
        require(line.starts_with('-'), "package archive contains a nonregular entry")?;
        names.push(line.split_whitespace().last().ok_or("invalid package archive entry")?);
    }
    names.sort_unstable();
    require(names == PACKAGES, "package archive is not the exact signed fixture set")
}
fn install() -> Result<()> {
    require(
        !Path::new("/etc/systemd/system/display-manager.service").exists(),
        "refuse to replace an existing display manager",
    )?;
    command("/usr/bin/rpm", &["--import", "/usr/local/sbin/ryoku-native21-signing.asc"])?;
    let archive = "/usr/local/sbin/ryoku-native21-packages.tar";
    let listing = command("/usr/bin/tar", &["--list", "--verbose", "--file", archive])?;
    validate_packages(&listing)?;
    let directory = "/usr/local/share/ryoku-native21-packages";
    fs::create_dir(directory)?;
    command(
        "/usr/bin/tar",
        &[
            "--extract",
            "--file",
            archive,
            "--directory",
            directory,
            "--no-same-owner",
            "--no-same-permissions",
            "--keep-old-files",
        ],
    )?;
    let paths: Vec<_> = PACKAGES.iter().map(|name| format!("{directory}/{name}")).collect();
    for path in &paths {
        command("/usr/bin/rpm", &["--checksig", path])?;
    }
    let mut install = vec!["--non-interactive", "install", "--no-recommends"];
    install.extend(paths.iter().map(String::as_str));
    command_for("/usr/bin/zypper", &install, "1200s")?;
    command("/usr/bin/zypper", &["--non-interactive", "--no-refresh", "verify", "--dry-run"])?;
    require(
        command(
            "/usr/bin/rpm",
            &[
                "-V",
                "ryoku-desktop",
                "ryoku-desktop-runtime",
                "ryoku-helper",
                "ryoku-profile-defaults",
                "ryoku-desktop-fonts",
                "ryoku-quickshell",
                "ryoku-tools",
            ],
        )?
        .is_empty(),
        "installed signed package verification failed",
    )?;
    command("/usr/bin/install", &["-m", "0755", "/usr/local/sbin/ryoku-recovery", CLI])?;
    for (relative, content) in [(SERVICE_PATH, SERVICE), (DROPIN_PATH, DROPIN)] {
        let path = format!("/{relative}");
        let parent = Path::new(&path).parent().ok_or("unit parent missing")?;
        fs::create_dir_all(parent)?;
        write_new(&path, content.as_bytes(), 0o644)?;
    }
    write_new("/etc/systemd/system/display-manager.service",
        format!("[Unit]\nDescription=Disposable Rust normal-session marker\n[Service]\nType=oneshot\nExecStart={PROBE} --normal-session-marker\nRemainAfterExit=yes\n").as_bytes(), 0o644)?;
    write_new(&format!("/etc/systemd/system/{UNIT}"), exercise_service().as_bytes(), 0o644)?;
    for relative in [".config", ".config/ryoku", ".config/ryoku/profile", "Documents"] {
        let path = format!("{HOME}/{relative}");
        if !Path::new(&path).exists() {
            fs::create_dir(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            own(&path)?;
        }
    }
    user_file(".config/ryoku/shell.json", b"{\"source\":\"before\"}", 0o600)?;
    user_file(".config/ryoku/profile/hero.png", &vec![42; 4 * 1024 * 1024], 0o640)?;
    user_file("Documents/native21-unrelated", b"unrelated home sentinel", 0o640)?;
    write_new("/etc/ryoku-native21-system", b"system source sentinel", 0o640)?;
    systemctl(&["daemon-reload"])?;
    bootstrap_numbered_root()?;
    println!("native_issue21_fixture_installed=passed");
    Ok(())
}
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
    command("/usr/bin/sync", &[])?;
    Ok(())
}

fn corrupt_config() -> Result<()> {
    fs::write(format!("{HOME}/.config/ryoku/shell.json"), b"{\"corrupt\":true}")?;
    fs::set_permissions(
        format!("{HOME}/.config/ryoku/shell.json"),
        fs::Permissions::from_mode(0o640),
    )?;
    fs::write(format!("{HOME}/.config/ryoku/profile/hero.png"), b"corrupted artwork")?;
    user_file(".config/ryoku/theme.json", b"{\"must\":\"disappear\"}", 0o600)?;
    Ok(())
}
fn verify_configuration(transaction: &str) -> Result<()> {
    let reader = GenerationReader::open_state_mount()?;
    let source = reader.open_verified_transaction(&id(transaction)?)?;
    let records: Vec<_> = source
        .manifest()
        .files()
        .iter()
        .filter(|entry| entry.root == StateRoot::UserHome)
        .cloned()
        .collect();
    let paths: Vec<_> = records.iter().map(|entry| entry.path.clone()).collect();
    let actual = capture_declared(
        &File::open(HOME)?,
        StateRoot::UserHome,
        &paths,
        CaptureLimits::default(),
    )?;
    require(actual == records, "restored content/metadata/absence does not match source")?;
    let sentinel = format!("{HOME}/Documents/native21-unrelated");
    require(fs::read(&sentinel)? == b"unrelated home sentinel", "unrelated home content changed")?;
    let meta = fs::metadata(sentinel)?;
    let (uid, gid) = account()?;
    require(
        meta.uid() == uid && meta.gid() == gid && meta.mode() & 0o7777 == 0o640,
        "unrelated home ownership or mode changed",
    )?;
    Ok(())
}
fn journal_phase(restore: &str) -> Result<RestorePhase> {
    Ok(RestoreJournalStore::open_existing_state_mount()?.load(&id(restore)?)?.phase)
}
fn checkpoint_retained(restore: &str) -> Result<()> {
    let journal = RestoreJournalStore::open_existing_state_mount()?.load(&id(restore)?)?;
    let checkpoint =
        GenerationReader::open_state_mount()?.open_verified_transaction(&id(restore)?)?;
    require(
        checkpoint.manifest().generation_id == journal.current_state_generation
            && checkpoint.manifest().manifest_sha256.as_ref()
                == Some(&journal.current_state_manifest_sha256),
        "current-state checkpoint was lost or mismatched",
    )
}
fn admission_refused() -> Result<()> {
    serial_phase("admission-refused")?;
    refuses(&["admit-session"])?;
    let _ = systemctl(&["start", "display-manager.service"]);
    require(
        !Path::new(&state_path("native21-normal-session")).exists(),
        "half-restored normal session started",
    )?;
    require(
        systemctl(&["is-failed", "ryoku-recovery-admission.service"])?.trim() == "failed",
        "admission unit did not fail closed",
    )?;
    // Clear only this fixture's unit failures; return to the recovery target.
    let _ =
        systemctl(&["reset-failed", "display-manager.service", "ryoku-recovery-admission.service"]);
    offline()?;
    serial_complete("admission-refused")
}
fn interrupted_restore() -> Result<()> {
    serial_phase("interrupted-restore")?;
    let mut child = Command::new(CLI)
        .args(["restore-configuration", CONFIG])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut killed = false;
    while Instant::now() < deadline {
        if journal_phase(CONFIG)? == RestorePhase::WritersStopped {
            child.kill()?;
            let _ = child.wait()?;
            killed = true;
            break;
        }
        if child.try_wait()?.is_some() {
            break;
        }
        thread::sleep(Duration::from_micros(200));
    }
    if !killed {
        let _ = child.kill();
        let _ = child.wait();
    }
    require(killed, "failed to interrupt the actual CLI at WritersStopped")?;
    require(
        journal_phase(CONFIG)? == RestorePhase::WritersStopped,
        "interrupted configuration restore invented completion",
    )?;
    checkpoint_retained(CONFIG)?;
    admission_refused()?;
    cli(&["restore-configuration", CONFIG])?;
    require(journal_phase(CONFIG)? == RestorePhase::Complete, "configuration resume incomplete")?;
    verify_configuration(SOURCE)?;
    serial_complete("interrupted-restore")
}
fn failure_cases() -> Result<()> {
    serial_phase("refusal-cases")?;
    let reader = GenerationReader::open_state_mount()?;
    let source = reader.open_verified_transaction(&id(SOURCE)?)?;
    let record = source
        .manifest()
        .files()
        .iter()
        .find(|entry| entry.path.as_str() == ".config/ryoku/profile/hero.png")
        .ok_or("fixture artwork record missing")?;
    let ryoku_recovery::EntryKind::RegularFile { sha256, .. } = &record.kind else {
        return Err("fixture artwork is not a regular payload".into());
    };
    let original = source.read_payload(sha256)?;
    let payload = format!(
        "{STATE}/generations/{}/payloads/{}.blob",
        source.manifest().generation_id.as_str(),
        sha256.as_str()
    );
    fs::set_permissions(&payload, fs::Permissions::from_mode(0o600))?;
    fs::write(&payload, b"bad archived payload")?;
    let refused = refuses(&["prepare-configuration", "native21-bad-archive", SOURCE]);
    fs::write(&payload, &original)?;
    fs::set_permissions(&payload, fs::Permissions::from_mode(0o400))?;
    File::open(&payload)?.sync_all()?;
    refused?;
    checkpoint_retained(CONFIG)?;
    verify_configuration(SOURCE)?;

    cli(&["manual-checkpoint", "native21-unavailable", "tester"])?;
    let missing = reader.open_verified_transaction(&id("native21-unavailable")?)?;
    command(
        "/usr/bin/snapper",
        &["--config", "root", "delete", "--sync", &missing.manifest().snapper_snapshot_id],
    )?;
    refuses(&["prepare-configuration", "native21-missing-partner", "native21-unavailable"])?;
    checkpoint_retained(CONFIG)?;
    verify_configuration(SOURCE)?;

    // No complete signed alternate desktop/runtime/defaults set is available.
    // Native incompatible-version acceptance remains explicitly unclaimed.
    serial_complete("refusal-cases")
}

fn exercise() -> Result<()> {
    offline()?;
    match fs::read_to_string(state_path("native21-phase")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cli(&["admit-session"])?;
            cli(&["manual-checkpoint", SOURCE, "tester"])?;
            cli(&["verify-linked", SOURCE])?;
            cli(&["plan", SOURCE, "configuration-only"])?;
            corrupt_config()?;
            cli(&["prepare-configuration", CONFIG, SOURCE])?;
            admission_refused()?;
            interrupted_restore()?;
            checkpoint_retained(CONFIG)?;
            failure_cases()?;
            corrupt_config()?;
            cli(&["prepare-linked", LINKED, SOURCE])?;
            checkpoint_retained(LINKED)?;
            phase("linked")?;
            cli(&["rollback-linked", LINKED])?;
            Ok(())
        }
        Ok(value) if value == "linked" => {
            admission_refused()?;
            cli(&["confirm-rollback", LINKED])?;
            cli(&["restore-configuration", LINKED])?;
            verify_configuration(SOURCE)?;
            checkpoint_retained(LINKED)?;
            cli(&["manual-checkpoint", "native21-system-source", "tester"])?;
            fs::write("/etc/ryoku-native21-system", b"corrupt system state")?;
            fs::write(
                format!("{HOME}/.config/ryoku/shell.json"),
                b"{\"system_only\":\"keep current\"}",
            )?;
            cli(&["prepare-system", SYSTEM, "native21-system-source"])?;
            checkpoint_retained(SYSTEM)?;
            phase("system")?;
            cli(&["rollback-system", SYSTEM])?;
            Ok(())
        }
        Ok(value) if value == "system" => {
            admission_refused()?;
            cli(&["confirm-rollback", SYSTEM])?;
            cli(&["complete-system", SYSTEM])?;
            require(
                fs::read("/etc/ryoku-native21-system")? == b"system source sentinel",
                "system snapshot content not restored",
            )?;
            verify_configuration(SYSTEM)?;
            for restore in [CONFIG, LINKED, SYSTEM] {
                checkpoint_retained(restore)?;
            }
            cli(&["admit-session"])?;
            systemctl(&["start", "display-manager.service"])?;
            require(
                fs::read(state_path("native21-normal-session"))? == b"admitted\n",
                "complete recovery did not admit the normal-session fixture",
            )?;
            systemctl(&["stop", "display-manager.service"])?;
            systemctl(&["disable", UNIT])?;
            phase("complete")?;
            write_new(&state_path("native21-result"), RESULTS.as_bytes(), 0o600)?;
            systemctl(&["isolate", "multi-user.target"])?;
            Ok(())
        }
        Ok(value) if value == "complete" => report(),
        Ok(_) => Err("invalid persisted native console phase".into()),
        Err(error) => Err(error.into()),
    }
}
fn report() -> Result<()> {
    let result = fs::read_to_string(state_path("native21-result"))?;
    if result == RESULTS {
        cli(&["admit-session"])?;
        for restore in [CONFIG, LINKED, SYSTEM] {
            require(
                journal_phase(restore)? == RestorePhase::Complete,
                "completed phase regressed",
            )?;
            checkpoint_retained(restore)?;
        }
    }
    print!("{result}");
    Ok(())
}
fn stop_event(
    phase: &str,
    complete: bool,
    journal: Option<RestorePhase>,
    rollback: bool,
) -> &'static str {
    if phase == "complete" && complete {
        "native_issue21_result=passed"
    } else if journal == Some(RestorePhase::WritersStopped) && rollback {
        match phase {
            "linked" => "native_issue21_handoff=linked",
            "system" => "native_issue21_handoff=system",
            _ => "native_issue21_result=failed",
        }
    } else {
        "native_issue21_result=failed"
    }
}
fn service_stopped() -> Result<()> {
    let phase = fs::read_to_string(state_path("native21-phase")).unwrap_or_default();
    let restore = match phase.as_str() {
        "linked" => Some(LINKED),
        "system" => Some(SYSTEM),
        _ => None,
    };
    let journal = restore.and_then(|restore| {
        RestoreJournalStore::open_existing_state_mount().ok()?.load(&id(restore).ok()?).ok()
    });
    let completed =
        fs::read_to_string(state_path("native21-result")).is_ok_and(|value| value == RESULTS);
    serial_line(stop_event(
        &phase,
        completed,
        journal.as_ref().map(|j| j.phase),
        journal.as_ref().is_some_and(|j| j.root_rollback.is_some()),
    ))
}
fn run() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--install") => install(),
        Some("--arm") => {
            systemctl(&["enable", UNIT])?;
            Ok(())
        }
        Some("--exercise") => exercise(),
        Some("--service-stopped") => service_stopped(),
        Some("--report") => report(),
        Some("--normal-session-marker") => {
            write_new(&state_path("native21-normal-session"), b"admitted\n", 0o600)
        }
        _ => Err("requires a fixed native console fixture mode".into()),
    }
}
fn main() {
    if let Err(error) = guard() {
        eprintln!("native console fixture refused: {error}");
        std::process::exit(1);
    }
    if let Err(error) = run() {
        eprintln!("native console fixture failed: {error}");
        if std::env::args().nth(1).as_deref() == Some("--exercise") {
            let _ =
                write_new(&state_path("native21-result"), b"native_issue21_result=failed\n", 0o600);
            let _ = serial_line("native_issue21_result=failed");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_survives_emergency_isolation_and_records_every_service_exit() {
        let unit = exercise_service();
        for line in [
            "DefaultDependencies=no",
            "IgnoreOnIsolate=yes",
            "After=local-fs.target var-lib-ryoku\\x2dstate.mount",
            "Before=shutdown.target umount.target",
            "Conflicts=shutdown.target",
        ] {
            assert!(unit.lines().any(|candidate| candidate == line));
        }
        assert!(unit.contains(&format!("ExecStopPost={PROBE} --service-stopped\n")));
        assert_eq!(stop_event("", false, None, false), "native_issue21_result=failed");
        assert_eq!(
            stop_event("linked", false, Some(RestorePhase::ContentStaged), true),
            "native_issue21_result=failed"
        );
        assert_eq!(
            stop_event("linked", false, Some(RestorePhase::WritersStopped), false),
            "native_issue21_result=failed"
        );
        assert_eq!(
            stop_event("linked", false, Some(RestorePhase::WritersStopped), true),
            "native_issue21_handoff=linked"
        );
        assert_eq!(
            stop_event("system", false, Some(RestorePhase::WritersStopped), true),
            "native_issue21_handoff=system"
        );
        assert_eq!(stop_event("complete", false, None, false), "native_issue21_result=failed");
        assert_eq!(stop_event("complete", true, None, false), "native_issue21_result=passed");
    }

    #[test]
    fn package_archive_requires_exact_regular_signed_package_names() {
        let listing: String = PACKAGES
            .iter()
            .rev()
            .map(|name| format!("-rw-r--r-- owner/group 123 2026-10-21 00:00 {name}\n"))
            .collect();
        validate_packages(&listing).unwrap();
        assert!(validate_packages(&listing.replacen('-', "l", 1)).is_err());
        assert!(validate_packages(&listing.replace("ryoku-tools-", "../ryoku-tools-")).is_err());
        assert!(validate_packages(&format!("{listing}{listing}")).is_err());
        assert!(validate_packages("").is_err());
        assert!(validate_packages(&listing.replace("0.1.0-5", "0.1.0-4")).is_err());
    }
}
