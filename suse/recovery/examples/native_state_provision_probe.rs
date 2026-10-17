//! Destructive component experiment for an explicitly seeded disposable VM.
//! The harness mounts root filesystem ID 5 at /run/ryoku-lab-top first.
//! Resources are retained for inspection; discard the disposable disk afterward.
use ryoku_recovery::{
    Identifier, SnapshotLink,
    generation::{GenerationReader, GenerationStore},
    machine_state::{provision_machine_state_record, read_machine_state_record},
    native_checkpoint::verify_new_root_checkpoint,
    state_creation::StateCreationTransaction,
    state_mount::collect_and_open_state_mount,
    state_provision::{
        STATE_MOUNT_UNIT, StateProvisionError, create_state_subvolume,
        install_recorded_state_mount_unit,
    },
};
use ryoku_snapper::{ConfigName, SnapperCheckpointBackend, SnapshotPairRequest, TransactionId};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    process::Command,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let inspect = arguments.as_slice() == ["--inspect-existing"];
    let install_boot = arguments.as_slice() == ["--install-boot-unit"];
    let verify_boot = arguments.as_slice() == ["--verify-boot"];
    let resume_child = arguments.len() == 3
        && matches!(arguments[0].as_str(), "--resume-created" | "--hold-resumed");
    if (!arguments.is_empty() && !inspect && !install_boot && !verify_boot && !resume_child)
        || rustix::process::geteuid().as_raw() != 0
        || rustix::process::getegid().as_raw() != 0
        || fs::read_to_string("/etc/ryoku-lab-fixture")? != "disposable-issue13\n"
        || fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() != "QEMU"
    {
        return Err(
            "requires root in the seeded disposable QEMU fixture and a supported probe mode".into(),
        );
    }
    let root = File::open("/")?;
    if resume_child {
        return resumed_child(&root, &arguments);
    }
    if install_boot || verify_boot {
        return boot_unit_probe(&root, install_boot);
    }
    if inspect {
        let record = read_machine_state_record(&root)?;
        collect_and_open_state_mount(&root, record.identity())?;
        let mut wrong = record.identity().clone();
        wrong.subvolume_id += 1;
        if collect_and_open_state_mount(&root, &wrong).is_ok() {
            return Err("wrong expected identity was accepted".into());
        }
        let _reader = GenerationReader::open_state_mount()?;
        println!(
            "native_state_readback=passed\nnative_state_wrong_identity_refused=passed\nnative_state_store_reopen=passed"
        );
        println!("native_state_boot_mount=unproven\nnative_state_interruption_resume=unproven");
        return Ok(());
    }
    let top = File::open("/run/ryoku-lab-top")?;
    let observed = Command::new("/usr/bin/findmnt")
        .args(["--noheadings", "--output", "UUID", "--target", "/"])
        .output()?;
    if !observed.status.success() {
        return Err("root UUID observation failed".into());
    }
    let uuid = String::from_utf8(observed.stdout)?.trim().to_owned();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let request = SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|_| "invalid fixed config")?,
        transaction_id: TransactionId::new(format!("state-provision-probe-{}", boot.trim()))
            .map_err(|_| "invalid boot token")?,
    };
    let snapshot = SnapperCheckpointBackend.create_single(&request)?;
    let checkpoint = verify_new_root_checkpoint(
        &root,
        SnapshotLink::new(
            Identifier::new("root").map_err(|_| "invalid fixed config")?,
            snapshot.as_str(),
            uuid,
        )
        .map_err(|_| "invalid checkpoint link")?,
    )?;
    let transaction_id = Identifier::new(format!("state-provision-probe-{}", boot.trim()))
        .map_err(|_| "invalid transaction")?;
    let mut provisioning =
        StateCreationTransaction::begin(&root, &checkpoint, transaction_id.clone())?;
    let created = provisioning.create(&top)?;
    if !matches!(create_state_subvolume(&root, &top), Err(StateProvisionError::ExistingVolume)) {
        return Err("existing state volume was not refused".into());
    }
    let original_record = created.machine_record().clone();
    let original_uuid = created.subvolume_uuid().to_owned();
    drop(provisioning);
    drop(created);
    if StateCreationTransaction::resume_created_from_top(
        &root,
        &checkpoint,
        Identifier::new("foreign-attempt").map_err(|_| "invalid test identifier")?,
        &top,
    )
    .is_ok()
    {
        return Err("unmounted receipt admitted a foreign transaction".into());
    }
    let (provisioning, created) = StateCreationTransaction::resume_created_from_top(
        &root,
        &checkpoint,
        transaction_id.clone(),
        &top,
    )?;
    if created.machine_record() != &original_record || created.subvolume_uuid() != original_uuid {
        return Err("unmounted receipt changed created identity".into());
    }
    println!("native_state_unmounted_receipt_resume=passed");
    // Additional experiment receipt; production intent/result are retained separately.
    let mut receipt = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open("/etc/ryoku-lab-created-state.json")?;
    receipt.write_all(&created.machine_record().canonical_bytes()?)?;
    receipt.sync_all()?;
    File::open("/etc")?.sync_all()?;
    fs::DirBuilder::new().mode(0o700).create("/var/lib/ryoku-state")?;
    let device =
        format!("/dev/disk/by-uuid/{}", created.machine_record().identity().filesystem_uuid);
    let options = format!(
        "subvolid={},nodev,nosuid,noexec",
        created.machine_record().identity().subvolume_id
    );
    let status = Command::new("/usr/bin/mount")
        .args(["-t", "btrfs", "-o", &options, &device, "/var/lib/ryoku-state"])
        .status()?;
    if !status.success() {
        return Err("state mount failed; retain fixture for inspection".into());
    }
    collect_and_open_state_mount(&root, created.machine_record().identity())?;
    let mut wrong = created.machine_record().identity().clone();
    wrong.subvolume_id += 1;
    if collect_and_open_state_mount(&root, &wrong).is_ok() {
        return Err("wrong independent state identity was accepted".into());
    }
    drop(provisioning);
    exercise_process_restart(snapshot.as_str(), transaction_id.as_str())?;
    let (_resumed_guard, created) =
        StateCreationTransaction::resume_created(&root, &checkpoint, transaction_id)?;
    let _store = GenerationStore::open_state_mount(created.machine_record().identity())?;
    provision_machine_state_record(&root, created.machine_record())?;
    assert_eq!(read_machine_state_record(&root)?, *created.machine_record());
    let _reader = GenerationReader::open_state_mount()?;
    println!(
        "native_state_creation=passed\nnative_state_existing_refused=passed\nnative_state_mount_identity=passed\nnative_state_wrong_identity_refused=passed\nnative_state_machine_record=passed\nnative_state_store_reopen=passed"
    );
    println!("native_state_boot_mount=unproven\nnative_state_interruption_resume=unproven");
    Ok(())
}

fn resumed_child(root: &File, args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let observed = Command::new("/usr/bin/findmnt")
        .args(["--noheadings", "--output", "UUID", "--target", "/"])
        .output()?;
    if !observed.status.success() {
        return Err("root UUID unavailable".into());
    }
    let checkpoint = verify_new_root_checkpoint(
        root,
        SnapshotLink::new(
            Identifier::new("root").map_err(|_| "invalid config")?,
            &args[1],
            String::from_utf8(observed.stdout)?.trim(),
        )
        .map_err(|_| "invalid checkpoint")?,
    )?;
    let (_guard, _created) = StateCreationTransaction::resume_created(
        root,
        &checkpoint,
        Identifier::new(&args[2]).map_err(|_| "invalid transaction")?,
    )?;
    if args[0] == "--hold-resumed" {
        let mut ready = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open("/run/ryoku-state-resume-ready")?;
        ready.write_all(b"resumed\n")?;
        ready.sync_all()?;
        std::thread::sleep(std::time::Duration::from_secs(60));
        return Err("parent did not exercise process death within deadline".into());
    }
    println!("native_state_resume_fresh_process=passed");
    Ok(())
}

fn exercise_process_restart(
    snapshot: &str,
    transaction: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let marker = "/run/ryoku-state-resume-ready";
    match fs::symlink_metadata(marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        _ => return Err("refuse an existing resume readiness marker".into()),
    }
    let executable = std::env::current_exe()?;
    let mut child =
        Command::new(&executable).args(["--hold-resumed", snapshot, transaction]).spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if child.try_wait()?.is_some() {
            return Err("resume child exited before readiness".into());
        }
        if fs::read(marker).is_ok_and(|bytes| bytes == b"resumed\n") {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("resume child readiness timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    child.kill()?;
    child.wait()?;
    fs::remove_file(marker)?;
    if !Command::new(executable)
        .args(["--resume-created", snapshot, transaction])
        .status()?
        .success()
    {
        return Err("fresh process failed to resume after predecessor was killed".into());
    }
    // This proves one durable creation boundary, not the entire restore journal.
    println!("native_state_creation_killed_process_resume=passed");
    Ok(())
}

fn boot_unit_probe(root: &File, install: bool) -> Result<(), Box<dyn std::error::Error>> {
    let record = read_machine_state_record(root)?;
    collect_and_open_state_mount(root, record.identity())?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let origin = "/etc/ryoku-lab-state-boot-origin";
    if install {
        let request = SnapshotPairRequest {
            config: ConfigName::new("root").map_err(|_| "invalid fixed config")?,
            transaction_id: TransactionId::new(format!("state-boot-probe-{}", boot.trim()))
                .map_err(|_| "invalid boot token")?,
        };
        let snapshot = SnapperCheckpointBackend.create_single(&request)?;
        verify_new_root_checkpoint(
            root,
            SnapshotLink::new(
                Identifier::new("root").map_err(|_| "invalid fixed config")?,
                snapshot.as_str(),
                record.identity().filesystem_uuid.clone(),
            )
            .map_err(|_| "invalid snapshot link")?,
        )?;
        let mut receipt =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(origin)?;
        receipt.write_all(boot.as_bytes())?;
        receipt.sync_all()?;
        File::open("/etc")?.sync_all()?;
        install_recorded_state_mount_unit(root)?;
        // Repetition verifies no-replace publication is idempotent.
        install_recorded_state_mount_unit(root)?;
        let unit_path = format!("/etc/systemd/system/{STATE_MOUNT_UNIT}");
        for (program, args) in [
            ("/usr/sbin/restorecon", vec![unit_path.as_str()]),
            ("/usr/bin/systemctl", vec!["daemon-reload"]),
            ("/usr/bin/systemctl", vec!["enable", STATE_MOUNT_UNIT]),
        ] {
            if !Command::new(program).args(args).status()?.success() {
                return Err("boot unit setup failed; retain fixture for reconciliation".into());
            }
        }
        println!("native_state_boot_unit_install=passed\nnative_state_boot_mount=awaiting_reboot");
    } else {
        if fs::read_to_string(origin)? == boot {
            return Err("no reboot occurred after unit installation".into());
        }
        for state in ["is-enabled", "is-active"] {
            if !Command::new("/usr/bin/systemctl")
                .args(["--quiet", state, STATE_MOUNT_UNIT])
                .status()?
                .success()
            {
                return Err("state mount unit is not enabled and active".into());
            }
        }
        let _reader = GenerationReader::open_state_mount()?;
        println!("native_state_boot_mount=passed\nnative_state_store_reopen_after_reboot=passed");
    }
    Ok(())
}
