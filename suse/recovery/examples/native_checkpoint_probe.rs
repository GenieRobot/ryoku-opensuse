//! Native creation/verification experiment, restricted to the seeded QEMU lab.
use ryoku_recovery::{Identifier, SnapshotLink, native_checkpoint::verify_new_root_checkpoint};
use ryoku_snapper::{
    ConfigName, SnapperCheckpointBackend, SnapperCliError, SnapshotPairRequest, TransactionId,
};
use std::{
    fs::{self, File},
    process::Command,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if fs::read_to_string("/etc/ryoku-lab-fixture")? != "disposable-issue13\n"
        || fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() != "QEMU"
    {
        return Err("native checkpoint probe requires seeded disposable QEMU guest".into());
    }
    let identity = Command::new("/usr/bin/findmnt")
        .args(["--noheadings", "--output", "UUID", "--target", "/"])
        .output()?;
    if !identity.status.success() {
        return Err("root UUID observation failed".into());
    }
    let uuid = String::from_utf8(identity.stdout)?.trim().to_owned();
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let request = SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|_| "invalid fixed config")?,
        transaction_id: TransactionId::new(format!("native-probe-{}", boot_id.trim()))
            .map_err(|_| "invalid boot transaction token")?,
    };
    let mut backend = SnapperCheckpointBackend;
    let id = backend.create_single(&request)?;
    let link = SnapshotLink::new(
        Identifier::new("root").map_err(|_| "invalid fixed config")?,
        id.as_str(),
        uuid,
    )
    .map_err(|_| "invalid observed snapshot link")?;
    let root = File::open("/")?;
    let checkpoint = verify_new_root_checkpoint(&root, link.clone())?;
    if backend.create_single(&request) != Err(SnapperCliError::CheckpointAlreadyExists) {
        return Err("duplicate transaction was not rejected".into());
    }
    let mut wrong_uuid = link;
    wrong_uuid.filesystem_uuid = "00000000-0000-0000-0000-000000000000".into();
    if verify_new_root_checkpoint(&root, wrong_uuid).is_ok() {
        return Err("wrong filesystem UUID was accepted".into());
    }
    println!(
        "native_checkpoint_creation=passed\nnative_checkpoint_kernel_identity=passed\nnative_checkpoint_duplicate_guard=passed\nnative_checkpoint_wrong_uuid_guard=passed"
    );
    println!("native_checkpoint_subvolume_id={}", checkpoint.subvolume_id);
    println!("native_checkpoint_subvolume_uuid={}", checkpoint.subvolume_uuid);
    Ok(())
}
