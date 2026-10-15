//! Controlled failure cases, only in the seeded disposable QEMU guest.
//! The harness installs the real initializer at its fixed native path first.
use ryoku_recovery::{
    Identifier, SnapshotLink,
    generation::GenerationReader,
    machine_state::read_machine_state_record,
    native_checkpoint::verify_new_root_checkpoint,
    state_creation::{StateCreationTransaction, has_pending_state_creation},
    state_mount::collect_and_open_state_mount,
    state_provision::{STATE_MOUNT_UNIT, provisioning_root_uuid},
};
use ryoku_snapper::{
    ConfigName, SnapperBackend, SnapperCheckpointBackend, SnapperCliBackend, SnapshotPairRequest,
    TransactionId,
};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    process::{Command, Output},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const INITIALIZER: &str = "/usr/local/sbin/ryoku-state-init";
const RECEIPT: &str = "/etc/ryoku/state-creation-result.json";
const TARGET: &str = "/var/lib/ryoku-state";

fn main() -> Result<()> {
    if std::env::args_os().len() != 1
        || rustix::process::geteuid().as_raw() != 0
        || rustix::process::getegid().as_raw() != 0
        || fs::read_to_string("/etc/ryoku-lab-fixture")? != "disposable-issue13\n"
        || fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() != "QEMU"
    {
        return Err("requires root in the seeded disposable QEMU fixture".into());
    }
    let root = File::open("/")?;
    let uuid = provisioning_root_uuid(&root)?;
    require_absent(TARGET)?;
    require_absent("/etc/ryoku/state-machine.json")?;
    let unit_path = format!("/etc/systemd/system/{STATE_MOUNT_UNIT}");
    require_absent(&unit_path)?;
    if has_pending_state_creation(&root)? {
        return Err("fixture already has a provisioning attempt".into());
    }
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let transaction = format!("state-initialize-prepared-{}", boot.trim());
    let request = SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|_| "invalid config")?,
        transaction_id: TransactionId::new(&transaction).map_err(|_| "invalid transaction")?,
    };
    let snapshot = SnapperCheckpointBackend.create_single(&request)?;
    let checkpoint = verify_new_root_checkpoint(
        &root,
        SnapshotLink::new(
            Identifier::new("root").map_err(|_| "invalid config")?,
            snapshot.as_str(),
            &uuid,
        )
        .map_err(|_| "invalid checkpoint")?,
    )?;
    let mut guard = StateCreationTransaction::begin(
        &root,
        &checkpoint,
        Identifier::new(&transaction).map_err(|_| "invalid transaction")?,
    )?;
    let top_path = "/run/ryoku-init-prep-top";
    fs::DirBuilder::new().mode(0o700).create(top_path)?;
    native(
        "/usr/bin/mount",
        &[
            "-t",
            "btrfs",
            "-o",
            "subvolid=5,nodev,nosuid,noexec",
            &format!("/dev/disk/by-uuid/{uuid}"),
            top_path,
        ],
    )?;
    let top = File::open(top_path)?;
    let created = guard.create(&top)?;
    let expected = created.machine_record().clone();
    let volume_uuid = created.subvolume_uuid().to_owned();
    drop(created);
    drop(guard);
    drop(top);
    native("/usr/bin/umount", &[top_path])?;
    fs::remove_dir(top_path)?;
    let snapshots = initializer_snapshots()?;
    let retry = initializer(&["initialize"])?;
    if retry.status.success()
        || !String::from_utf8_lossy(&retry.stderr).contains("incomplete state initialization")
        || initializer_snapshots()? != snapshots
    {
        return Err("fresh retry did not preserve the pending checkpoint set".into());
    }
    require_absent(TARGET)?;
    require_absent("/etc/ryoku/state-machine.json")?;
    require_absent(&unit_path)?;
    println!("native_initializer_prior_attempt_refused=passed");

    let original = fs::read_to_string(RECEIPT)?;
    let needle = format!("\"subvolume_uuid\":\"{volume_uuid}\"");
    if original.matches(&needle).count() != 1 {
        return Err("receipt UUID binding unavailable".into());
    }
    let poisoned = original.replacen(
        &needle,
        "\"subvolume_uuid\":\"00000000-0000-0000-0000-000000000000\"",
        1,
    );
    rewrite_receipt(poisoned.as_bytes())?;
    let wrong = initializer(&["resume", &transaction, snapshot.as_str()])?;
    if wrong.status.success()
        || !String::from_utf8_lossy(&wrong.stderr).contains("IdentityMismatch")
        || fs::read_to_string(RECEIPT)? != poisoned
    {
        return Err("bad UUID was accepted or repaired from current observation".into());
    }
    require_absent(TARGET)?;
    require_absent("/etc/ryoku/state-machine.json")?;
    require_absent(&unit_path)?;
    println!("native_initializer_wrong_uuid_refused=passed");
    println!("native_initializer_bad_receipt_not_rewritten=passed");
    // Explicit reconciliation of this fixture's deliberately poisoned record.
    rewrite_receipt(original.as_bytes())?;

    fs::DirBuilder::new().mode(0o700).create(TARGET)?;
    fs::set_permissions(TARGET, fs::Permissions::from_mode(0o700))?;
    let sentinel = format!("{TARGET}/foreign-sentinel");
    let mut file =
        fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&sentinel)?;
    file.write_all(b"preserve-foreign-state\n")?;
    file.sync_all()?;
    drop(file);
    let foreign = initializer(&["resume", &transaction, snapshot.as_str()])?;
    if foreign.status.success()
        || fs::read(&sentinel)? != b"preserve-foreign-state\n"
        || fs::metadata(TARGET)?.permissions().mode() & 0o7777 != 0o700
    {
        return Err("foreign target was adopted or changed".into());
    }
    require_absent("/etc/ryoku/state-machine.json")?;
    println!("native_initializer_foreign_target_preserved=passed");
    require_absent(&unit_path)?;
    fs::remove_file(&sentinel)?;
    fs::remove_dir(TARGET)?; // Only our known fixture objects, never recursive deletion.

    let foreign_unit = b"[Unit]\nDescription=foreign-fixture\n";
    let mut file =
        fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&unit_path)?;
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    file.write_all(foreign_unit)?;
    file.sync_all()?;
    drop(file);
    let refused = initializer(&["resume", &transaction, snapshot.as_str()])?;
    if refused.status.success()
        || fs::read(&unit_path)? != foreign_unit
        || fs::metadata(&unit_path)?.permissions().mode() & 0o7777 != 0o644
    {
        return Err("foreign unit was replaced or changed".into());
    }
    require_absent("/etc/ryoku/state-machine.json")?;
    println!("native_initializer_foreign_unit_preserved=passed");
    fs::remove_file(&unit_path)?;
    // This empty target was newly created by this test's failed resume.
    // Removal is nonrecursive and fails if anything unexpected was placed there.
    match fs::remove_dir(TARGET) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }

    let resumed = initializer(&["resume", &transaction, snapshot.as_str()])?;
    if !resumed.status.success()
        || String::from_utf8_lossy(&resumed.stdout).trim()
            != "State store initialization resumed; bootstrap checkpoint retained"
        || initializer_snapshots()? != snapshots
    {
        return Err("receipt-bound unmounted CLI resume failed".into());
    }
    if read_machine_state_record(&root)? != expected {
        return Err("machine identity changed during resume".into());
    }
    collect_and_open_state_mount(&root, expected.identity())?;
    let _reader = GenerationReader::open_state_mount()?;
    println!("native_initializer_unmounted_resume=passed");
    // Change only boot files created by this seeded fixture. The live mount
    // stays available, so refusal must come from persistent boot verification.
    let unit_bytes = fs::read(&unit_path)?;
    fs::remove_file(&unit_path)?;
    if initializer(&["initialize"])?.status.success() {
        return Err("repeat accepted missing boot unit".into());
    }
    require_absent(&unit_path)?;
    fs::write(&unit_path, &unit_bytes)?;
    fs::set_permissions(&unit_path, fs::Permissions::from_mode(0o644))?;
    println!("native_initializer_missing_boot_unit_refused=passed");
    fs::write(&unit_path, foreign_unit)?;
    if initializer(&["initialize"])?.status.success() || fs::read(&unit_path)? != foreign_unit {
        return Err("repeat accepted or repaired altered boot unit".into());
    }
    fs::write(&unit_path, &unit_bytes)?;
    println!("native_initializer_changed_boot_unit_preserved=passed");
    let enabled = format!("/etc/systemd/system/local-fs.target.wants/{STATE_MOUNT_UNIT}");
    let target = fs::read_link(&enabled)?;
    fs::remove_file(&enabled)?;
    if initializer(&["initialize"])?.status.success() {
        return Err("repeat accepted missing enablement".into());
    }
    require_absent(&enabled)?;
    println!("native_initializer_missing_enablement_refused=passed");
    std::os::unix::fs::symlink("../foreign-fixture.mount", &enabled)?;
    if initializer(&["initialize"])?.status.success()
        || fs::read_link(&enabled)? != std::path::Path::new("../foreign-fixture.mount")
    {
        return Err("repeat accepted or repaired foreign enablement".into());
    }
    fs::remove_file(&enabled)?;
    std::os::unix::fs::symlink(target, &enabled)?;
    println!("native_initializer_foreign_enablement_preserved=passed");
    File::open(&unit_path)?.sync_all()?;
    File::open("/etc/systemd/system/local-fs.target.wants")?.sync_all()?;
    File::open("/etc/systemd/system")?.sync_all()?;
    if !initializer(&["initialize"])?.status.success()
        || read_machine_state_record(&root)? != expected
        || initializer_snapshots()? != snapshots
    {
        return Err("healthy repeat after fixture restoration failed".into());
    }
    Ok(())
}

fn initializer(args: &[&str]) -> Result<Output> {
    let output = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=2s", "60s", INITIALIZER])
        .args(args)
        .output()?;
    if output.stdout.len() + output.stderr.len() > 128 * 1024 {
        return Err("initializer output exceeds fixture bound".into());
    }
    std::io::stderr().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
    Ok(output)
}
fn initializer_snapshots() -> Result<Vec<String>> {
    let mut ids: Vec<_> = SnapperCliBackend
        .list(&ConfigName::new("root").map_err(|_| "invalid config")?)?
        .into_iter()
        .filter(|row| {
            row.transaction_id
                .as_ref()
                .is_some_and(|id| id.as_str().starts_with("state-initialize-"))
        })
        .map(|row| row.id.as_str().to_owned())
        .collect();
    ids.sort();
    Ok(ids)
}
fn require_absent(path: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err("unexpected existing fixture path".into()),
    }
}
fn rewrite_receipt(bytes: &[u8]) -> Result<()> {
    fs::write(RECEIPT, bytes)?;
    File::open(RECEIPT)?.sync_all()?;
    File::open("/etc/ryoku")?.sync_all()?;
    Ok(())
}
fn native(program: &str, args: &[&str]) -> Result<()> {
    if !Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=2s", "60s", program])
        .args(args)
        .status()?
        .success()
    {
        return Err("native fixture preparation failed".into());
    }
    Ok(())
}
