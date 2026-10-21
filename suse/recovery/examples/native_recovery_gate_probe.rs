//! Destructive #22 gate. Every write requires the existing marked disposable
//! QEMU fixture and verified protected mount. The accepted #21 lifecycle is reused.
#[path = "native_console_restore_probe.rs"]
mod console;
#[path = "support/issue22_results.rs"]
mod results;
#[path = "support/issue22_trace.rs"]
mod trace;

use console::{
    account, checkpoint_retained, cli, command, guard, id, journal_phase, offline, own, require,
    serial_line, state_path, verify_configuration, write_new,
};
use ryoku_recovery::{
    EntryKind, StateRoot,
    capture::{CaptureLimits, capture_declared},
    generation::GenerationReader,
    installed_inventory::PREFERENCES,
    restore::RestorePhase,
};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt, symlink},
        process::CommandExt,
    },
    path::Path,
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const STATE: &str = "/var/lib/ryoku-state";
const HOME: &str = "/home/tester";
const REAL_CLI: &str = "/usr/local/sbin/ryoku-recovery";
const PROBE: &str = "/usr/local/sbin/ryoku-native21-probe";
const SOURCE: &str = "native22-source";
const RESTORE: &str = "native22-phases";

fn receipt(name: &str) -> String {
    state_path(&format!("native22-{name}"))
}
fn mark(name: &str) -> Result<()> {
    write_new(&receipt(name), b"passed\n", 0o600)
}
fn marked(name: &str) -> bool {
    fs::read(receipt(name)).is_ok_and(|bytes| bytes == b"passed\n")
}
fn progress() -> Result<()> {
    serial_line("native_issue21_operation=refusal-cases")
}
fn refuses(args: &[&str]) -> Result<()> {
    let output = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=3s", "120s", "/usr/bin/ryoku-recovery"])
        .args(args)
        .output()?;
    // Captured only by this fixture's private service log, never public evidence.
    eprintln!(
        "native22 refusal {args:?} exit={:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    require(output.status.code() == Some(2), "recovery command did not produce a bounded refusal")
}
fn require_backup_pressure(available: u64, required_payload: u64) -> Result<()> {
    require(
        required_payload > 0 && available < required_payload,
        "fixture has enough nominal space for its mandatory payload; exhaustion is unproven",
    )
}
fn read_only_invocation(args: &[String]) -> bool {
    matches!(args, [command] if command == "admit-session")
}
fn guard_readonly() -> Result<()> {
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
    ryoku_recovery::state_mount::collect_and_open_state_mount_readonly(&root, record.identity())?;
    Ok(())
}
fn fixture() -> Result<()> {
    let (uid, gid) = account()?;
    for (index, relative) in PREFERENCES.iter().enumerate() {
        if index % 3 != 0
            || *relative == ".config/ryoku/theme.json"
            || *relative == ryoku_recovery::installed_inventory::PERSONAL_PROFILE
            || Path::new(&format!("{HOME}/{relative}")).symlink_metadata().is_ok()
        {
            continue;
        }
        let path = format!("{HOME}/{relative}");
        let mut parents = Vec::new();
        let mut next = Path::new(&path).parent().ok_or("missing fixture parent")?;
        while !next.exists() {
            parents.push(next.to_owned());
            next = next.parent().ok_or("fixture escaped home")?;
        }
        for parent in parents.iter().rev() {
            fs::create_dir(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            own(parent.to_str().ok_or("invalid fixture parent")?)?;
        }
        let bytes = if relative.ends_with(".json") { b"{}\n".as_slice() } else { b"fixture\n" };
        write_new(&path, bytes, 0o600)?;
        own(&path)?;
    }
    let sentinel = format!("{HOME}/Documents/native22-unrelated");
    write_new(&sentinel, b"unrelated native22 exact bytes\n", 0o640)?;
    own(&sentinel)?;
    command("/usr/bin/setfacl", &["-m", &format!("u:{uid}:r--"), &sentinel])?;
    let shell = format!("{HOME}/.config/ryoku/shell.json");
    command("/usr/bin/setfacl", &["-m", &format!("u:{uid}:r--"), &shell])?;
    let link = format!("{HOME}/.config/ryoku/profile/hero.jpg");
    if fs::symlink_metadata(&link).is_ok() {
        fs::remove_file(&link)?;
    }
    symlink("hero.png", &link)?;
    rustix::fs::chownat(
        rustix::fs::CWD,
        link.as_str(),
        Some(rustix::fs::Uid::from_raw(uid)),
        Some(rustix::fs::Gid::from_raw(gid)),
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
    )?;
    let unrelated = sentinel_records(Path::new(HOME))?;
    write_new(&receipt("sentinel.json"), &serde_json::to_vec(&unrelated)?, 0o600)?;
    Ok(())
}
fn compare() -> Result<()> {
    verify_configuration(SOURCE)?;
    compare_sentinel()?;
    let source = GenerationReader::open_state_mount()?.open_verified_transaction(&id(SOURCE)?)?;
    let records = source.manifest().files();
    require(
        records.iter().filter(|entry| entry.root == StateRoot::UserHome).count()
            == PREFERENCES.len(),
        "comparison did not cover the entire compiled inventory",
    )?;
    for entry in records {
        if let EntryKind::RegularFile { sha256, .. } = &entry.kind {
            source.read_payload(sha256)?;
        }
    }
    Ok(())
}
fn compare_sentinel() -> Result<()> {
    let actual = sentinel_records(Path::new(HOME))?;
    require(
        serde_json::to_vec(&actual)? == fs::read(receipt("sentinel.json"))?,
        "unrelated sentinel bytes/ownership/mode/ACL/xattrs changed",
    )?;
    Ok(())
}
fn sentinel_records(home: &Path) -> Result<Vec<ryoku_recovery::FileRecord>> {
    // This synthetic comparison root is never enrolled or published. UserHome
    // remains restricted to the compiled managed preference inventory.
    Ok(capture_declared(
        &File::open(home.join("Documents"))?,
        StateRoot::UserRyokuConfig,
        &[ryoku_recovery::RelativePath::new("native22-unrelated")
            .map_err(|_| "invalid fixed sentinel path")?],
        CaptureLimits::default(),
    )?)
}
fn no_attempt(attempt: &str) -> Result<()> {
    require(
        !Path::new(&format!("{STATE}/restore-journals/{attempt}.json")).exists(),
        "failed backup invented a restore journal",
    )?;
    require(
        !GenerationReader::open_state_mount()?.list_transactions()?.contains(&id(attempt)?),
        "failed backup published a transaction",
    )?;
    compare()
}
fn replace_sealed(path: &str, bytes: &[u8]) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    fs::write(path, bytes)?;
    // Keep the valid immutable inode policy during the negative command, so
    // content/path corruption reaches validation instead of failing mode checks.
    fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
    File::open(path)?.sync_all()?;
    Ok(())
}
fn poison_manifest(name: &str, traversal: bool) -> Result<()> {
    let source = GenerationReader::open_state_mount()?.open_verified_transaction(&id(SOURCE)?)?;
    let path =
        format!("{STATE}/generations/{}/manifest.json", source.manifest().generation_id.as_str());
    let original = fs::read(&path)?;
    let mut value: serde_json::Value = serde_json::from_slice(&original)?;
    if traversal {
        let files = value
            .get_mut("files")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("manifest inventory absent")?;
        files[0]["path"] = serde_json::Value::String("../Documents/native22-escape".into());
    } else {
        value["schema_version"] = serde_json::Value::from(65535);
    }
    replace_sealed(&path, &serde_json::to_vec(&value)?)?;
    let outcome = refuses(&["prepare-configuration", name, SOURCE]);
    replace_sealed(&path, &original)?;
    outcome?;
    no_attempt(name)?;
    require(!Path::new(&format!("{HOME}/Documents/native22-escape")).exists(), "traversal escaped")
}
fn failures() -> Result<()> {
    poison_manifest("native22-bad-manifest", false)?;
    poison_manifest("native22-traversal", true)?;
    progress()?;
    let reader = GenerationReader::open_state_mount()?;
    let source = reader.open_verified_transaction(&id(SOURCE)?)?;
    let record = source
        .manifest()
        .files()
        .iter()
        .find(|record| record.path.as_str() == ".config/ryoku/profile/hero.png")
        .ok_or("source artwork absent")?;
    let EntryKind::RegularFile { sha256, .. } = &record.kind else {
        return Err("expected artwork payload".into());
    };
    let path = format!(
        "{STATE}/generations/{}/payloads/{}.blob",
        source.manifest().generation_id.as_str(),
        sha256.as_str()
    );
    let original = fs::read(&path)?;
    replace_sealed(&path, b"corrupt archive")?;
    let outcome = refuses(&["prepare-configuration", "native22-bad-archive", SOURCE]);
    replace_sealed(&path, &original)?;
    outcome?;
    no_attempt("native22-bad-archive")?;

    // A FIFO at an enrolled file is a real collector failure, never a timeout
    // or a successful checkpoint with that entry silently omitted.
    let path = format!("{HOME}/.config/ryoku/theme.json");
    require(!Path::new(&path).exists(), "capture-failure path must start absent")?;
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path.as_str(),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )?;
    own(&path)?;
    let outcome = refuses(&["manual-checkpoint", "native22-capture-failure", "tester"]);
    let restore_outcome =
        refuses(&["prepare-configuration", "native22-capture-failure-restore", SOURCE]);
    fs::remove_file(path)?;
    outcome?;
    restore_outcome?;
    no_attempt("native22-capture-failure")?;
    no_attempt("native22-capture-failure-restore")?;

    // Reserve actual Btrfs extents, leaving less than the required checkpoint
    // payload. Only this private fixture file is removed after the refusal.
    let fill = receipt("capacity-fill");
    let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&fill)?;
    let stats = rustix::fs::fstatvfs(&file)?;
    let free = stats.f_bavail.saturating_mul(stats.f_frsize);
    require(free > 256 * 1024 * 1024, "fixture has insufficient initial storage")?;
    let required = source
        .manifest()
        .files()
        .iter()
        .filter_map(|entry| {
            if let EntryKind::RegularFile { size, .. } = entry.kind { Some(size) } else { None }
        })
        .max()
        .ok_or("fixture has no required payload")?;
    require(required >= 4 * 1024 * 1024, "fixture lacks its required large artwork payload")?;
    let allocate = free - required / 2;
    let allocation = rustix::fs::fallocate(&file, rustix::fs::FallocateFlags::empty(), 0, allocate);
    // Btrfs may exhaust a metadata/data chunk before statvfs's nominal count.
    require(
        allocation.is_ok() || allocation == Err(rustix::io::Errno::NOSPC),
        "unexpected allocation failure",
    )?;
    file.sync_all()?;
    let observed = rustix::fs::fstatvfs(&file)?;
    let available = observed.f_bavail.saturating_mul(observed.f_frsize);
    eprintln!("native22 storage before={free} after={} allocation={allocation:?}", available);
    require_backup_pressure(available, required)?;
    let result = refuses(&["prepare-configuration", "native22-low-space", SOURCE]);
    drop(file);
    fs::remove_file(fill)?;
    rustix::fs::syncfs(&File::open(STATE)?)?;
    result?;
    no_attempt("native22-low-space")?;
    progress()?;

    // A successful syscall in the generation's private staging tree is a
    // deterministic real capture boundary. Restart must refuse its used native
    // Snapper tag, retaining the interrupted artifacts for explicit inspection.
    trace::kill_after_sync(
        REAL_CLI,
        &["manual-checkpoint", "native22-capture-killed", "tester"],
        |path| {
            if !path.starts_with(&format!("{STATE}/staging/")) {
                return Ok(false);
            }
            // The actual capture process is stopped inside its owned machine
            // lock. Launch two other supported CLI writers concurrently; both
            // must refuse before publishing or touching managed targets.
            let mut capture = Command::new(REAL_CLI)
                .args(["manual-checkpoint", "native22-concurrent-capture", "tester"])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?;
            let mut restore = Command::new(REAL_CLI)
                .args(["prepare-configuration", "native22-concurrent-restore", SOURCE])
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?;
            let capture_status = capture.wait()?;
            let restore_status = restore.wait()?;
            require(
                capture_status.code() == Some(2) && restore_status.code() == Some(2),
                "concurrent supported writer bypassed the capture owner's lock",
            )?;
            no_attempt("native22-concurrent-capture")?;
            no_attempt("native22-concurrent-restore")?;
            Ok(true)
        },
    )?;
    refuses(&["manual-checkpoint", "native22-capture-killed", "tester"])?;
    no_attempt("native22-capture-killed")?;
    cli(&["manual-checkpoint", "native22-after-capture-death", "tester"])?;
    compare()?;
    cli(&["manual-checkpoint", "native22-missing-source", "tester"])?;
    let missing = reader.open_verified_transaction(&id("native22-missing-source")?)?;
    command(
        "/usr/bin/snapper",
        &["--config", "root", "delete", "--sync", &missing.manifest().snapper_snapshot_id],
    )?;
    refuses(&["prepare-configuration", "native22-missing-partner", "native22-missing-source"])?;
    no_attempt("native22-missing-partner")?;
    Ok(())
}
fn phases() -> Result<()> {
    let shell = format!("{HOME}/.config/ryoku/shell.json");
    fs::write(&shell, b"{\"corrupted\":true}")?;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o640))?;
    command("/usr/bin/setfacl", &["-b", &shell])?;
    let link = format!("{HOME}/.config/ryoku/profile/hero.jpg");
    fs::remove_file(&link)?;
    write_new(&link, b"replace with original symlink", 0o600)?;
    own(&link)?;
    let absent = format!("{HOME}/.config/ryoku/theme.json");
    write_new(&absent, b"{}", 0o600)?;
    own(&absent)?;
    cli(&["prepare-configuration", RESTORE, SOURCE])?;
    cli(&["restore-configuration", RESTORE])?;
    require(journal_phase(RESTORE)? == RestorePhase::Complete, "phase sweep incomplete")?;
    checkpoint_retained(RESTORE)?;
    compare()?;
    Ok(())
}

fn cli_wrapper(args: &[String]) -> Result<()> {
    let borrowed: Vec<_> = args.iter().map(String::as_str).collect();
    let phases: &[RestorePhase] = match borrowed.as_slice() {
        ["prepare-configuration", RESTORE, SOURCE] => {
            &[RestorePhase::Planned, RestorePhase::CheckpointVerified, RestorePhase::ContentStaged]
        }
        ["restore-configuration", RESTORE] => &[
            RestorePhase::WritersStopped,
            RestorePhase::ConfigurationRestored,
            RestorePhase::HealthChecked,
            RestorePhase::Complete,
        ],
        ["rollback-linked", "native21-linked"] => &[RestorePhase::WritersStopped],
        ["confirm-rollback", "native21-linked"] => &[RestorePhase::RootRollbackComplete],
        ["complete-system", "native21-system"] => {
            &[RestorePhase::HealthChecked, RestorePhase::Complete]
        }
        _ => &[],
    };
    for phase in phases {
        let name = format!("{}-{}-{phase:?}", args[0], args[1]);
        if marked(&name) {
            continue;
        }
        progress()?;
        trace::kill_after_sync(REAL_CLI, &borrowed, |path| {
            if path != format!("{STATE}/restore-journals") {
                return Ok(false);
            }
            // The child still owns the store lock at its fsync boundary. Read
            // the exact published bytes directly while the owner is stopped.
            let path = format!("{STATE}/restore-journals/{}.json", args[1]);
            let journal: ryoku_recovery::restore::RestoreJournal =
                serde_json::from_slice(&fs::read(path)?)?;
            Ok(journal.phase == *phase
                && (args[0] != "rollback-linked" || journal.root_rollback.is_some()))
        })?;
        require(journal_phase(&args[1])? == *phase, "durable journal phase changed after SIGKILL")?;
        checkpoint_retained(&args[1])?;
        if *phase != RestorePhase::Complete {
            let status = Command::new(REAL_CLI)
                .arg("admit-session")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            require(status.code() == Some(2), "incomplete phase admitted a normal session")?;
        }
        mark(&name)?;
    }
    // Preserve the production CLI's PID for the accepted #21 SIGKILL case and
    // systemd ownership. No unobserved child may outlive a killed launcher.
    Err(Command::new(REAL_CLI).args(args).exec().into())
}
fn report() -> Result<()> {
    require(marked("initial-gate-complete"), "initial failure gate not complete")?;
    for (command, restore, phase) in [
        ("prepare-configuration", RESTORE, RestorePhase::Planned),
        ("prepare-configuration", RESTORE, RestorePhase::CheckpointVerified),
        ("prepare-configuration", RESTORE, RestorePhase::ContentStaged),
        ("restore-configuration", RESTORE, RestorePhase::WritersStopped),
        ("restore-configuration", RESTORE, RestorePhase::ConfigurationRestored),
        ("restore-configuration", RESTORE, RestorePhase::HealthChecked),
        ("restore-configuration", RESTORE, RestorePhase::Complete),
        ("rollback-linked", "native21-linked", RestorePhase::WritersStopped),
        ("confirm-rollback", "native21-linked", RestorePhase::RootRollbackComplete),
        ("complete-system", "native21-system", RestorePhase::HealthChecked),
        ("complete-system", "native21-system", RestorePhase::Complete),
    ] {
        require(
            marked(&format!("{command}-{restore}-{phase:?}")),
            "missing phase death/restart receipt",
        )?;
    }
    for restore in [RESTORE, "native21-config", "native21-linked", "native21-system"] {
        require(
            journal_phase(restore)? == RestorePhase::Complete,
            "reboot regressed a completed attempt",
        )?;
        checkpoint_retained(restore)?;
    }
    GenerationReader::open_state_mount()?
        .open_historical_transaction(&File::open("/")?, &id(SOURCE)?)?;
    compare_sentinel()?;
    // System-only baseline deliberately preserves its checkpoint configuration.
    // Its full readback is independently checked by the accepted base report.
    for line in results::RESULTS {
        println!("{line}");
    }
    Ok(())
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if read_only_invocation(&args) {
        // Admission's real systemd sandbox makes protected state read-only.
        // Preserve that supported read path without authorizing fixture writes.
        guard_readonly()?;
    } else {
        guard()?;
    }
    match args.first().map(String::as_str) {
        Some("--install") => {
            // Install the Rust observer as a fixture-only launcher. The exact
            // supplied production CLI remains unmodified at REAL_CLI. This
            // precedes the numbered-root bootstrap snapshot, so reboot retains
            // the observer without modifying a captured snapshot afterward.
            console::install_with_cli(PROBE)
        }
        Some("--exercise") => {
            if !marked("initial-gate-complete") {
                offline()?;
                fixture()?;
                cli(&["manual-checkpoint", SOURCE, "tester"])?;
                compare()?;
                failures()?;
                phases()?;
                mark("initial-gate-complete")?;
            }
            console::main();
            Ok(())
        }
        Some("--report") => {
            console::main();
            report()
        }
        Some(mode) if mode.starts_with("--") => {
            console::main();
            Ok(())
        }
        Some(_) => cli_wrapper(&args),
        None => Err("requires a fixed fixture mode or production CLI arguments".into()),
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("issue22 gate failed: {error}");
        if std::env::args().nth(1).as_deref() == Some("--exercise") {
            let _ = serial_line("native_issue21_result=failed");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_sentinel_comparison_never_enrolls_it_in_managed_home() {
        let root = std::env::temp_dir().join(format!("native22-sentinel-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("Documents")).unwrap();
        let sentinel = root.join("Documents/native22-unrelated");
        fs::write(&sentinel, b"preserved outside inventory").unwrap();
        let before = sentinel_records(&root).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].root, StateRoot::UserRyokuConfig);
        assert!(
            capture_declared(
                &File::open(&root).unwrap(),
                StateRoot::UserHome,
                &[ryoku_recovery::RelativePath::new("Documents/native22-unrelated").unwrap()],
                CaptureLimits::default(),
            )
            .is_err()
        );
        assert!(!PREFERENCES.contains(&"Documents/native22-unrelated"));
        assert_eq!(sentinel_records(&root).unwrap(), before);
        fs::write(&sentinel, b"changed").unwrap();
        assert_ne!(sentinel_records(&root).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safe_sibling_symlink_captures_but_traversal_link_refuses() {
        let root = std::env::temp_dir().join(format!("native22-symlink-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("hero.png"), b"artwork").unwrap();
        let link = root.join("hero.jpg");
        symlink("hero.png", &link).unwrap();
        let directory = File::open(&root).unwrap();
        let paths = [ryoku_recovery::RelativePath::new("hero.jpg").unwrap()];
        let records = capture_declared(
            &directory,
            StateRoot::UserRyokuConfig,
            &paths,
            CaptureLimits::default(),
        )
        .unwrap();
        assert!(
            matches!(&records[0].kind, EntryKind::Symlink { target, .. } if target == "hero.png")
        );
        fs::remove_file(&link).unwrap();
        symlink("../../../Documents/native22-unrelated", &link).unwrap();
        assert!(matches!(
            capture_declared(
                &directory,
                StateRoot::UserRyokuConfig,
                &paths,
                CaptureLimits::default()
            ),
            Err(ryoku_recovery::capture::CaptureError::UnsupportedEntryType)
        ));
        assert_eq!(fs::read(root.join("hero.png")).unwrap(), b"artwork");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corruption_keeps_the_sealed_inode_policy_and_restores_exact_bytes() {
        use std::os::unix::fs::MetadataExt;
        let path = std::env::temp_dir().join(format!("native22-sealed-{}", std::process::id()));
        let path = path.to_str().unwrap();
        write_new(path, b"original", 0o400).unwrap();
        replace_sealed(path, b"corrupt").unwrap();
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o7777, 0o400);
        assert_eq!(fs::read(path).unwrap(), b"corrupt");
        replace_sealed(path, b"original").unwrap();
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o7777, 0o400);
        assert_eq!(fs::read(path).unwrap(), b"original");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn nominal_low_space_is_not_exhaustion_when_the_required_payload_still_fits() {
        let payload = 4 * 1024 * 1024;
        assert!(require_backup_pressure(32_100_352, payload).is_err());
        assert!(require_backup_pressure(payload, payload).is_err());
        assert!(require_backup_pressure(2 * 1024 * 1024, payload).is_ok());
        assert!(require_backup_pressure(0, 0).is_err());
    }

    #[test]
    fn only_exact_admission_invocation_uses_the_readonly_fixture_guard() {
        assert!(read_only_invocation(&["admit-session".into()]));
        for arguments in [
            vec![],
            vec!["admit-session", "extra"],
            vec!["--exercise"],
            vec!["--normal-session-marker"],
            vec!["manual-checkpoint", SOURCE, "tester"],
            vec!["prepare-configuration", RESTORE, SOURCE],
        ] {
            assert!(!read_only_invocation(
                &arguments.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>()
            ));
        }
    }
}
