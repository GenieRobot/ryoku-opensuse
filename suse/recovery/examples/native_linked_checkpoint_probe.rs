//! Component experiment restricted to the seeded disposable QEMU fixture.
//! The harness must initialize state storage first. This applies configuration
//! to one isolated `/run` fixture; it does not switch or restore the guest root.
//! Fixed probe snapshots and records are retained until the disk is discarded.

use rustix::{
    fs::{
        AtFlags, Mode, OFlags, ResolveFlags, XattrFlags, fchmod, fgetxattr, fsetxattr, fsync,
        openat2, statat, unlinkat,
    },
    io::Errno,
};
use ryoku_recovery::{
    Identifier, OperationReason, RecoveryManifest, RelativePath, SnapshotLink, StateRoot,
    capture::CaptureLimits,
    configuration_apply::ConfigurationDestination,
    disk_stage::DiskStageStore,
    generation::{GenerationReader, GenerationStore},
    generation_capture::{
        DeclaredRoot, GenerationCaptureError, GenerationCaptureRequest, capture_publish_verify,
    },
    linked_checkpoint::HistoricalLookupError,
    machine_state::read_machine_state_record,
    native_checkpoint::{NativeCheckpointError, verify_new_root_checkpoint},
    preparation::{ConfigurationPreparationRequest, PreparationStores, RestoreCompatibility},
    restore::{
        CompatibilityStatus, RestoreJournal, RestoreJournalStore, RestorePhase, RestorePlan,
        RestoreScope, SystemSnapshotStatus,
    },
    state_mount::collect_and_open_state_mount,
};
use ryoku_snapper::{
    CatalogEntry, ConfigName, GenerationId, LayoutCapability, PartnerState,
    SnapperCheckpointBackend, SnapperCliError, SnapshotId, SnapshotPairRequest, TransactionId,
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    process::Command,
};

#[path = "support/boot_chain.rs"]
mod boot_chain;

type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;
const TRANSACTION: &str = "native-linked-probe";
const GENERATION: &str = "native-linked-generation";
const JOURNAL: &str = "native-linked-journal";
const SOURCE: &str = "/run/ryoku-native-linked-source";
const RESTORE_TARGET: &str = "/run/ryoku-native-linked-restore-target";
const HOME_SENTINEL: &str = "/home/tester/ryoku-native-linked-home-sentinel";
const CONTENT: &[u8] = b"synthetic linked checkpoint payload\n";
const ABSENT_PATH: &str = "optional.json";
const ACL_PRINCIPAL_UID: u32 = 65_534;

fn named_principal_acl() -> Vec<u8> {
    let mut acl = 2_u32.to_le_bytes().to_vec();
    for (tag, permissions, id) in [
        (1_u16, 6_u16, u32::MAX),
        (2, 4, ACL_PRINCIPAL_UID),
        (4, 0, u32::MAX),
        (16, 4, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    acl
}

fn verify_named_principal_acl(file: &File) -> ProbeResult<()> {
    let expected = named_principal_acl();
    let mut actual = vec![0; expected.len()];
    let length = fgetxattr(file, "system.posix_acl_access", &mut actual)?;
    if length != expected.len() || actual != expected {
        return Err("native named-principal access ACL did not round-trip exactly".into());
    }
    Ok(())
}

fn id(value: &str) -> ProbeResult<Identifier> {
    Identifier::new(value).map_err(|_| "invalid fixed probe identifier".into())
}

fn main() -> ProbeResult<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 1
        || !matches!(
            arguments[0].as_str(),
            "--capture"
                | "--verify"
                | "--verify-after-root-change"
                | "--exercise-failures"
                | "--probe-mutation-lock-busy"
                | "--probe-mutation-lock-available"
                | "--interrupt-configuration-restore"
                | "--observe-layout"
                | "--observe-boot-chain"
                | "--cleanup-journal"
        )
        || rustix::process::geteuid().as_raw() != 0
        || rustix::process::getegid().as_raw() != 0
        || fs::read_to_string("/etc/ryoku-lab-fixture")? != "disposable-issue13\n"
        || fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() != "QEMU"
    {
        return Err("requires root in seeded disposable QEMU and an explicit probe mode".into());
    }
    let root = File::open("/")?;
    match arguments[0].as_str() {
        "--capture" => capture(&root)?,
        "--verify" => verify(&root, false)?,
        "--verify-after-root-change" => verify(&root, true)?,
        "--exercise-failures" => exercise_failures(&root)?,
        "--probe-mutation-lock-busy" => probe_mutation_lock_busy(&root)?,
        "--probe-mutation-lock-available" => probe_mutation_lock_available(&root)?,
        "--interrupt-configuration-restore" => interrupt_configuration_restore(&root)?,
        "--observe-layout" => observe_layout()?,
        "--observe-boot-chain" => boot_chain::observe(),
        "--cleanup-journal" => cleanup_journal(&root)?,
        _ => unreachable!(),
    }
    println!("native_linked_boot_compatibility=unproven");
    println!("native_linked_restore_execution=unproven");
    Ok(())
}

fn bounded_file(path: &str) -> ProbeResult<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(32 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 {
        return Err("layout file exceeds fixture evidence limit".into());
    }
    Ok(bytes)
}

fn print_boot_fact(value: serde_json::Value) {
    let encoded = value.to_string();
    // Twelve bounded records fit the harness's 1 MiB stdout limit, including
    // JSON escaping. Oversize observations remain unproven rather than fatal.
    if encoded.len() <= 48 * 1024 {
        println!("boot_fact={encoded}");
    } else {
        println!("boot_fact={{\"status\":\"evidence-too-large\"}}");
    }
}

/// Raw facts go only to the harness's private log. No fact below is a boot
/// compatibility verdict; missing observations remain explicit and unproven.
fn observe_layout() -> ProbeResult<()> {
    for path in [
        "/proc/cmdline",
        "/etc/fstab",
        "/etc/default/grub",
        "/etc/snapper/configs/root",
        "/boot/grub2/grub.cfg",
    ] {
        match bounded_file(path) {
            Ok(bytes) => print_boot_fact(serde_json::json!({
                "file": path, "content": String::from_utf8_lossy(&bytes),
            })),
            Err(_) => print_boot_fact(serde_json::json!({"file": path, "status": "unavailable"})),
        }
    }
    for (program, arguments) in [
        ("/usr/bin/findmnt", vec!["--json", "--output", "TARGET,SOURCE,FSTYPE,OPTIONS,FSROOT"]),
        ("/usr/sbin/btrfs", vec!["subvolume", "get-default", "/"]),
        ("/usr/sbin/btrfs", vec!["subvolume", "show", "/"]),
        ("/usr/sbin/btrfs", vec!["subvolume", "show", "/.snapshots"]),
        ("/usr/bin/snapper", vec!["--version"]),
        ("/usr/bin/snapper", vec!["--help"]),
        ("/usr/bin/uname", vec!["-r"]),
    ] {
        let observed = Command::new("/usr/bin/timeout")
            .env_clear()
            .args(["--signal=KILL", "15s", program])
            .args(&arguments)
            .output();
        match observed {
            Ok(output) if output.stdout.len() <= 32 * 1024 => print_boot_fact(serde_json::json!({
                "program": program, "arguments": arguments,
                "success": output.status.success(), "stdout": String::from_utf8_lossy(&output.stdout),
            })),
            _ => print_boot_fact(
                serde_json::json!({"program": program, "arguments": arguments, "status": "unavailable"}),
            ),
        }
    }
    println!("native_linked_layout_observed=recorded");
    println!("native_linked_root_switch=unproven");
    Ok(())
}

fn cleanup_journal(root: &File) -> ProbeResult<()> {
    use rustix::fs::{AtFlags, FlockOperation, Mode, OFlags, ResolveFlags, flock, openat2};
    verify(root, false)?;
    let reader = GenerationReader::open_state_mount()?;
    let expected = journal_for(&reader.verify_transaction(&id(TRANSACTION)?)?)?;
    let machine = read_machine_state_record(root)?;
    let (state, _) = collect_and_open_state_mount(root, machine.identity())?;
    let directory = openat2(
        &state,
        "restore-journals",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?;
    let lock = openat2(
        &directory,
        ".restore-journal.lock",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?;
    flock(&lock, FlockOperation::LockExclusive)?;
    let name = format!("{JOURNAL}.json");
    let file = File::from(openat2(
        &directory,
        name.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let stat = rustix::fs::fstat(&file)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_nlink != 1
    {
        return Err("unsafe owning journal cannot be cleaned".into());
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 || serde_json::from_slice::<RestoreJournal>(&bytes)? != expected {
        return Err("changed journal cannot be cleaned".into());
    }
    rustix::fs::unlinkat(&directory, name.as_str(), AtFlags::empty())?;
    rustix::fs::fsync(&directory)?;
    flock(&lock, FlockOperation::Unlock)?;
    println!("native_linked_owning_journal_cleanup=passed");
    Ok(())
}

fn request(transaction: &str) -> ProbeResult<SnapshotPairRequest> {
    Ok(SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|_| "invalid fixed config")?,
        transaction_id: TransactionId::new(transaction).map_err(|_| "invalid fixed transaction")?,
    })
}

fn capture(root: &File) -> ProbeResult<()> {
    let machine = read_machine_state_record(root)?;
    let store = GenerationStore::open_initialized_state_mount(machine.identity())?;
    // The synthetic source is exclusively owned by this one fixture attempt.
    // Never adopt or overwrite a prior source after an interrupted run.
    fs::DirBuilder::new().mode(0o700).create(SOURCE)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(format!("{SOURCE}/fixture.txt"))?;
    file.write_all(CONTENT)?;
    fsetxattr(&file, "system.posix_acl_access", &named_principal_acl(), XattrFlags::empty())?;
    if file.metadata()?.mode() & 0o7777 != 0o640 {
        return Err("native ACL mask did not produce the expected mode bits".into());
    }
    verify_named_principal_acl(&file)?;
    file.sync_all()?;
    let source = File::open(SOURCE)?;
    source.sync_all()?;
    let request = request(TRANSACTION)?;
    let snapshot = SnapperCheckpointBackend.create_single(&request)?;
    let native = verify_new_root_checkpoint(
        root,
        SnapshotLink::new(id("root")?, snapshot.as_str(), &machine.identity().filesystem_uuid)
            .map_err(|_| "invalid native snapshot link")?,
    )?;
    let paths = [
        RelativePath::new("fixture.txt").map_err(|_| "invalid fixed path")?,
        RelativePath::new(ABSENT_PATH).map_err(|_| "invalid fixed path")?,
    ];
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &source, paths: &paths }];
    let manifest = capture_publish_verify(
        &store,
        GenerationCaptureRequest {
            transaction_id: id(TRANSACTION)?,
            generation_id: id(GENERATION)?,
            parent_generation_id: None,
            snapshot: native.verified_link().clone(),
            reason: OperationReason::ProfileApply,
            created_utc: "2026-10-15T00:00:00Z".into(),
            package_revisions: Vec::new(),
            provider_revision: None,
            profile_revision: None,
            roots: &roots,
            limits: CaptureLimits::default(),
        },
    )?;
    if manifest.root_snapshot_receipt() != Some(native.receipt()) {
        return Err("publication did not retain original native identity".into());
    }
    let catalog_store = store.open_snapper_catalog_store()?;
    let mut catalog = catalog_store.load()?;
    catalog
        .insert(CatalogEntry {
            transaction_id: TransactionId::new(TRANSACTION)
                .map_err(|_| "invalid fixture catalog transaction")?,
            root_config: ConfigName::new("root").map_err(|_| "invalid fixture catalog config")?,
            root_snapshot: SnapshotId::new(snapshot.as_str())
                .map_err(|_| "invalid fixture catalog snapshot")?,
            generation_id: GenerationId::new(GENERATION)
                .map_err(|_| "invalid fixture catalog generation")?,
            capability: LayoutCapability::ConfigurationOnly,
            partner: PartnerState::Verified,
            manual_checkpoint: false,
            known_working: false,
        })
        .map_err(|_| "fixture catalog entry was duplicated")?;
    catalog_store.replace(&catalog)?;
    // Planned is a persistence fixture only. It does not claim that a second
    // current-state checkpoint, compatibility check or restore has run.
    RestoreJournalStore::initialize_state_mount(machine.identity())?
        .create(&journal_for(&manifest)?)?;
    if SnapperCheckpointBackend.create_single(&request)
        != Err(SnapperCliError::CheckpointAlreadyExists)
    {
        return Err("duplicate transaction checkpoint was not refused".into());
    }
    println!("native_linked_duplicate_checkpoint_refused=passed");
    println!("native_linked_snapshot_id={}", snapshot.as_str());
    println!("native_linked_original_identity_recorded=passed");
    verify(root, false)
}

fn journal_for(manifest: &RecoveryManifest) -> ProbeResult<RestoreJournal> {
    let digest = manifest.manifest_sha256.clone().ok_or("unsealed fixture generation")?;
    Ok(RestoreJournal::new(
        id(JOURNAL)?,
        RestorePlan {
            source_manifest_sha256: digest.clone(),
            transaction_id: manifest.transaction_id.clone(),
            generation_id: manifest.generation_id.clone(),
            snapper_config: manifest.snapper_config.clone(),
            snapshot_id: manifest.snapper_snapshot_id.clone(),
            filesystem_uuid: manifest.filesystem_uuid.clone(),
            scope: RestoreScope::ConfigurationOnly,
            configuration_entries: 2,
        },
        manifest.generation_id.clone(),
        digest,
    ))
}

fn verify(root: &File, require_changed_root: bool) -> ProbeResult<()> {
    let reader = GenerationReader::open_state_mount()?;
    let linked = reader.open_historical_transaction(root, &id(TRANSACTION)?)?;
    let manifest = linked.generation().manifest();
    if manifest.schema_version != 2 || manifest.generation_id != id(GENERATION)? {
        return Err("wrong persisted generation identity".into());
    }
    if manifest.files().len() != 2 {
        return Err("linked fixture inventory has an unexpected number of entries".into());
    }
    let record = manifest
        .files()
        .iter()
        .find(|entry| entry.path.as_str() == "fixture.txt")
        .ok_or("missing synthetic payload")?;
    if record.root != StateRoot::EtcRyoku || record.path.as_str() != "fixture.txt" {
        return Err("synthetic payload path or managed root changed".into());
    }
    let ryoku_recovery::EntryKind::RegularFile { metadata, sha256, .. } = &record.kind else {
        return Err("wrong synthetic payload type".into());
    };
    if metadata.uid != 0 || metadata.gid != 0 || metadata.mode != 0o640 {
        return Err("synthetic payload ownership or mode was not preserved".into());
    }
    let fixture = File::open(format!("{SOURCE}/fixture.txt"))?;
    verify_named_principal_acl(&fixture)?;
    if linked.generation().read_payload(sha256)? != CONTENT {
        return Err("synthetic payload changed".into());
    }
    let absent = manifest
        .files()
        .iter()
        .find(|entry| entry.path.as_str() == ABSENT_PATH)
        .ok_or("missing declared absent path")?;
    if absent.root != StateRoot::EtcRyoku || absent.kind != ryoku_recovery::EntryKind::Absent {
        return Err("declared absent path was not retained as absent".into());
    }
    let journal = RestoreJournalStore::open_existing_state_mount()?.load(&id(JOURNAL)?)?;
    if journal != journal_for(manifest)? || journal.phase != RestorePhase::Planned {
        return Err("persisted planned journal differs from recorded linkage".into());
    }
    if require_changed_root {
        if !matches!(
            verify_new_root_checkpoint(root, linked.snapshot().receipt().link().clone()),
            Err(NativeCheckpointError::NotCurrentRootSnapshot)
        ) {
            return Err("running root identity has not demonstrably changed".into());
        }
        println!("native_linked_changed_root_historical_readback=passed");
    }
    println!("native_linked_catalog_generation_readback=passed");
    println!("native_linked_exact_snapshot_readback=passed");
    println!("native_linked_fixture_metadata_readback=passed");
    println!("native_linked_named_acl_readback=passed");
    println!("native_linked_absent_entry_readback=passed");
    println!("native_linked_planned_journal_readback=passed");
    Ok(())
}

fn btrfs(arguments: &[&str]) -> ProbeResult<()> {
    if !Command::new("/usr/sbin/btrfs").env_clear().args(arguments).status()?.success() {
        return Err("fixture Btrfs operation failed; retain fixture for inspection".into());
    }
    Ok(())
}

fn exercise_failures(root: &File) -> ProbeResult<()> {
    verify(root, false)?;
    let reader = GenerationReader::open_state_mount()?;
    let manifest = reader.verify_transaction(&id(TRANSACTION)?)?;
    exercise_corrupt_manifest(root, &reader)?;
    exercise_manifest_symlink_refusal(root, &reader)?;
    exercise_traversal_manifest(root, &reader)?;
    exercise_corrupt_payload(root, &reader, &manifest)?;
    exercise_artifact_mode_refusal(root, &reader)?;
    exercise_artifact_owner_refusal(root, &reader)?;
    exercise_artifact_group_refusal(root, &reader)?;
    exercise_artifact_directory_group_refusal(root, &reader)?;
    exercise_failed_capture(root)?;
    exercise_mutation_lock_contention(root)?;
    exercise_configuration_restore(root)?;
    let source = format!("/.snapshots/{}/snapshot", manifest.snapper_snapshot_id);
    let retained = format!("/.snapshots/{}/ryoku-probe-retained", manifest.snapper_snapshot_id);
    if fs::symlink_metadata(&retained).is_ok() {
        return Err("prior probe path exists; explicit reconciliation required".into());
    }

    let snapshot_identity = fs::symlink_metadata(&source)?;
    if !snapshot_identity.is_dir() {
        return Err("recorded Snapper snapshot path is not a real directory".into());
    }
    btrfs(&["property", "set", "-ts", &source, "ro", "false"])?;
    let writable = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    btrfs(&["property", "set", "-ts", &source, "ro", "true"])?;
    if !matches!(writable, Err(HistoricalLookupError::SnapshotWritable)) {
        return Err("writable snapshot was not refused".into());
    }
    println!("native_linked_writable_snapshot_refused=passed");

    fs::rename(&source, &retained)?;
    let absent = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    fs::rename(&retained, &source)?;
    let restored_snapshot_identity = fs::symlink_metadata(&source)?;
    if restored_snapshot_identity.dev() != snapshot_identity.dev()
        || restored_snapshot_identity.ino() != snapshot_identity.ino()
    {
        return Err("original Snapper snapshot directory identity was not restored".into());
    }
    if !matches!(absent, Err(HistoricalLookupError::SnapshotMissing)) {
        return Err("missing snapshot was not refused".into());
    }
    println!("native_linked_missing_snapshot_refused=passed");

    // A second real snapshot supplies a different UUID/ID at the recorded
    // number. Exchanging the numbered wrapper directories keeps both snapshots
    // read-only: moving a read-only subvolume across parents requires write
    // permission on its root inode and fails with EROFS. These wrappers share
    // one writable parent, so neither snapshot's parent inode changes.
    let substitute =
        SnapperCheckpointBackend.create_single(&request("native-linked-substitute")?)?;
    let substitute_link =
        SnapshotLink::new(id("root")?, substitute.as_str(), &manifest.filesystem_uuid)
            .map_err(|_| "invalid substitute snapshot link")?;
    let substitute_native = verify_new_root_checkpoint(root, substitute_link.clone())?;
    let snapshots = File::open("/.snapshots")?;
    let exchange = |operation: &str| -> ProbeResult<()> {
        rustix::fs::renameat_with(
            &snapshots,
            manifest.snapper_snapshot_id.as_str(),
            &snapshots,
            substitute.as_str(),
            rustix::fs::RenameFlags::EXCHANGE,
        )
        .map_err(|error| {
            format!("{operation}: {error}; retain fixture for explicit reconciliation").into()
        })
    };
    exchange("exchange original and substitute snapshot wrappers")?;
    let replaced = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    // Restore both original paths before judging the lookup result. Do not
    // retry an interrupted run or repair an ambiguous exchange automatically.
    exchange("restore original and substitute snapshot wrappers")?;
    reader.open_historical_transaction(root, &id(TRANSACTION)?)?;
    if verify_new_root_checkpoint(root, substitute_link)?.receipt() != substitute_native.receipt() {
        return Err("substitute identity changed while restoring original paths".into());
    }
    if !matches!(replaced, Err(HistoricalLookupError::SnapshotMismatch)) {
        return Err("substituted native snapshot was not refused".into());
    }
    println!("native_linked_substituted_snapshot_refused=passed");

    const TRANSACTION_LINK: &str = "native-linked-probe.json";
    const RETAINED_TRANSACTION_LINK: &str = "native-linked-probe.json.probe-retained";
    let machine = read_machine_state_record(root)?;
    let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state_fd);
    let catalog = File::from(openat2(
        &state,
        "catalog",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    match statat(&catalog, RETAINED_TRANSACTION_LINK, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err("prior transaction-link probe path exists; reconcile explicitly".into());
        }
        Err(Errno::NOENT) => (),
        Err(error) => return Err(error.into()),
    }
    let link = statat(&catalog, TRANSACTION_LINK, AtFlags::SYMLINK_NOFOLLOW)?;
    if rustix::fs::FileType::from_raw_mode(link.st_mode) != rustix::fs::FileType::RegularFile
        || link.st_uid != 0
        || link.st_gid != 0
        || link.st_nlink != 1
        || link.st_mode & 0o7777 != 0o400
    {
        return Err("transaction link has unsafe metadata before missing-partner probe".into());
    }
    let rename_transaction_link = |from: &str, to: &str| -> ProbeResult<()> {
        rustix::fs::renameat_with(
            &catalog,
            from,
            &catalog,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        fsync(&catalog)?;
        Ok(())
    };
    rename_transaction_link(TRANSACTION_LINK, RETAINED_TRANSACTION_LINK)?;
    let retained_link = statat(&catalog, RETAINED_TRANSACTION_LINK, AtFlags::SYMLINK_NOFOLLOW)?;
    if retained_link.st_dev != link.st_dev || retained_link.st_ino != link.st_ino {
        return Err("transaction link identity changed while moving it aside".into());
    }
    let missing_link = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    rename_transaction_link(RETAINED_TRANSACTION_LINK, TRANSACTION_LINK)?;
    let restored_link = statat(&catalog, TRANSACTION_LINK, AtFlags::SYMLINK_NOFOLLOW)?;
    if restored_link.st_dev != link.st_dev || restored_link.st_ino != link.st_ino {
        return Err("transaction link identity changed while restoring it".into());
    }
    reader.open_historical_transaction(root, &id(TRANSACTION)?)?;
    if !matches!(missing_link, Err(HistoricalLookupError::GenerationMissing)) {
        return Err("missing generation transaction link was not refused".into());
    }
    println!("native_linked_missing_transaction_link_refused=passed");

    let generation = format!("/var/lib/ryoku-state/generations/{GENERATION}");
    let missing = format!("/var/lib/ryoku-state/generations/{GENERATION}-retained");
    if fs::symlink_metadata(&missing).is_ok() {
        return Err("prior generation probe path exists".into());
    }
    let generation_identity = fs::symlink_metadata(&generation)?;
    if !generation_identity.is_dir() {
        return Err("recorded generation path is not a real directory".into());
    }
    fs::rename(&generation, &missing)?;
    let absent = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    fs::rename(&missing, &generation)?;
    let restored_generation_identity = fs::symlink_metadata(&generation)?;
    if restored_generation_identity.dev() != generation_identity.dev()
        || restored_generation_identity.ino() != generation_identity.ino()
    {
        return Err("original generation directory identity was not restored".into());
    }
    if !matches!(absent, Err(HistoricalLookupError::GenerationMissing)) {
        return Err("missing generation was not refused".into());
    }
    println!("native_linked_missing_generation_refused=passed");
    verify(root, false)
}

fn mutation_lock_file(root: &File) -> ProbeResult<File> {
    let machine = read_machine_state_record(root)?;
    let (state, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state);
    let lock = File::from(openat2(
        &state,
        ryoku_recovery::state_mount::MUTATION_LOCK_NAME,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let metadata = rustix::fs::fstat(&lock)?;
    if rustix::fs::FileType::from_raw_mode(metadata.st_mode) != rustix::fs::FileType::RegularFile
        || metadata.st_uid != 0
        || metadata.st_gid != 0
        || metadata.st_mode & 0o7777 != 0o600
        || metadata.st_nlink != 1
    {
        return Err("persistent mutation lock has unsafe metadata".into());
    }
    // The probe never creates or adopts a replacement lock file.
    Ok(lock)
}

fn probe_mutation_lock_busy(root: &File) -> ProbeResult<()> {
    use rustix::fs::{FlockOperation, flock};

    let lock = mutation_lock_file(root)?;
    match flock(&lock, FlockOperation::NonBlockingLockExclusive) {
        Err(Errno::AGAIN) => println!("native_linked_mutation_lock_busy=passed"),
        Ok(()) => {
            flock(&lock, FlockOperation::Unlock)?;
            return Err("exclusive mutation writer bypassed an existing lock holder".into());
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn probe_mutation_lock_available(root: &File) -> ProbeResult<()> {
    use rustix::fs::{FlockOperation, flock};

    let lock = mutation_lock_file(root)?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
    flock(&lock, FlockOperation::Unlock)?;
    println!("native_linked_mutation_lock_available=passed");
    Ok(())
}

fn exercise_mutation_lock_contention(root: &File) -> ProbeResult<()> {
    use rustix::fs::{FlockOperation, flock};

    let lock = mutation_lock_file(root)?;
    flock(&lock, FlockOperation::NonBlockingLockShared)
        .map_err(|_| "cannot start lock probe while recovery holds mutation.lock")?;
    run_mutation_lock_child(
        "--probe-mutation-lock-busy",
        "native_linked_mutation_lock_busy=passed",
    )?;
    flock(&lock, FlockOperation::Unlock)?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(
        |_| "cannot start lock probe while another exclusive writer holds mutation.lock",
    )?;
    run_mutation_lock_child(
        "--probe-mutation-lock-busy",
        "native_linked_mutation_lock_busy=passed",
    )?;
    flock(&lock, FlockOperation::Unlock)?;
    run_mutation_lock_child(
        "--probe-mutation-lock-available",
        "native_linked_mutation_lock_available=passed",
    )?;
    println!("native_linked_mutation_lock_contention=passed");
    Ok(())
}

fn run_mutation_lock_child(mode: &str, marker: &str) -> ProbeResult<()> {
    let output = Command::new(std::env::current_exe()?).arg(mode).output()?;
    let expected = format!(
        "{marker}\nnative_linked_boot_compatibility=unproven\nnative_linked_restore_execution=unproven\n"
    );
    if !output.status.success() || output.stdout != expected.as_bytes() {
        return Err(format!("mutation lock child {mode} failed its exact result check").into());
    }
    Ok(())
}

/// Apply the original verified generation to a disposable managed-state root.
/// The current-state checkpoint covers both the overwritten file and the path
/// that must be removed; a separate sentinel proves unrelated files survive.
fn exercise_configuration_restore(root: &File) -> ProbeResult<()> {
    let target_exists = match fs::symlink_metadata(RESTORE_TARGET) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let home_exists = match fs::symlink_metadata(HOME_SENTINEL) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if target_exists != home_exists {
        return Err("partial configuration-restore fixture requires explicit reconciliation".into());
    }
    let fixture_exists = target_exists;
    if fixture_exists {
        let target = fs::symlink_metadata(RESTORE_TARGET)?;
        let home = fs::symlink_metadata(HOME_SENTINEL)?;
        if !target.is_dir()
            || target.uid() != 0
            || target.gid() != 0
            || target.mode() & 0o7777 != 0o700
            || !home.is_file()
            || home.uid() != 0
            || home.gid() != 0
            || home.mode() & 0o7777 != 0o600
            || fs::read(HOME_SENTINEL)? != b"unrelated home sentinel\n"
        {
            return Err("existing configuration-restore fixture identity changed".into());
        }
    } else {
        fs::DirBuilder::new().mode(0o700).create(RESTORE_TARGET)?;
        let mut home_sentinel =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(HOME_SENTINEL)?;
        home_sentinel.write_all(b"unrelated home sentinel\n")?;
        home_sentinel.sync_all()?;
        drop(home_sentinel);
        File::open("/home/tester")?.sync_all()?;
    }
    let home_sentinel_before = fs::symlink_metadata(HOME_SENTINEL)?;
    let target = (|| -> ProbeResult<()> {
        if !fixture_exists {
            for (name, bytes, mode) in [
                ("fixture.txt", &b"current fixture before restore\n"[..], 0o644),
                (ABSENT_PATH, &b"current file that should be removed\n"[..], 0o644),
                ("unrelated-sentinel", &b"unrelated managed-state sentinel\n"[..], 0o640),
            ] {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(mode)
                    .open(format!("{RESTORE_TARGET}/{name}"))?;
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            File::open(RESTORE_TARGET)?.sync_all()?;
        } else if fs::read(format!("{RESTORE_TARGET}/fixture.txt"))?
            != b"current fixture before restore\n"
            || fs::read(format!("{RESTORE_TARGET}/{ABSENT_PATH}"))?
                != b"current file that should be removed\n"
            || fs::read(format!("{RESTORE_TARGET}/unrelated-sentinel"))?
                != b"unrelated managed-state sentinel\n"
        {
            return Err("existing configuration-restore contents changed".into());
        }
        let sentinel_before = fs::symlink_metadata(format!("{RESTORE_TARGET}/unrelated-sentinel"))?;

        let machine = read_machine_state_record(root)?;
        let source =
            GenerationReader::open_state_mount()?.open_verified_transaction(&id(TRANSACTION)?)?;
        let store = GenerationStore::open_initialized_state_mount(machine.identity())?;
        let journals = RestoreJournalStore::open_existing_state_mount()?;
        let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
        let state = File::from(state_fd);
        let stage_fd = openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?;
        let stage_root = File::from(stage_fd);
        let stages = DiskStageStore::open(&stage_root)?;
        let stores =
            PreparationStores { generations: &store, journals: &journals, stages: &stages };
        let target_root = File::open(RESTORE_TARGET)?;
        let paths = [
            RelativePath::new("fixture.txt").map_err(|_| "invalid restore fixture path")?,
            RelativePath::new(ABSENT_PATH).map_err(|_| "invalid restore absent path")?,
        ];
        let roots =
            [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &target_root, paths: &paths }];
        let restore_id = id("native-linked-restore-attempt")?;
        let compatibility = RestoreCompatibility {
            source_snapshot: SystemSnapshotStatus::Available {
                snapper_config: source.manifest().snapper_config.clone(),
                snapshot_id: source.manifest().snapper_snapshot_id.clone(),
                filesystem_uuid: source.manifest().filesystem_uuid.clone(),
            },
            system: CompatibilityStatus::Unknown,
            configuration: CompatibilityStatus::Compatible,
        };
        let (prepared, checkpoint_status) = if fixture_exists {
            let journal = journals.load(&restore_id)?;
            if journal.plan.transaction_id != source.manifest().transaction_id
                || journal.plan.scope != RestoreScope::ConfigurationOnly
            {
                return Err("existing configuration-restore journal identity changed".into());
            }
            let checkpoint_manifest = store.verify(&journal.current_state_generation)?;
            let checkpoint_link = SnapshotLink::new(
                checkpoint_manifest.snapper_config.clone(),
                checkpoint_manifest.snapper_snapshot_id.as_str(),
                &checkpoint_manifest.filesystem_uuid,
            )
            .map_err(|_| "invalid resumed current-state checkpoint link")?;
            let _native = verify_new_root_checkpoint(root, checkpoint_link.clone())?;
            let checkpoint_status = SystemSnapshotStatus::Available {
                snapper_config: checkpoint_link.snapper_config.clone(),
                snapshot_id: checkpoint_link.snapper_snapshot_id.clone(),
                filesystem_uuid: checkpoint_link.filesystem_uuid.clone(),
            };
            let prepared =
                stores.resume(&restore_id, &compatibility, &checkpoint_status, &roots)?;
            (prepared, checkpoint_status)
        } else {
            let checkpoint_snapshot =
                SnapperCheckpointBackend.create_single(&request(restore_id.as_str())?)?;
            let checkpoint_link = SnapshotLink::new(
                id("root")?,
                checkpoint_snapshot.as_str(),
                &machine.identity().filesystem_uuid,
            )
            .map_err(|_| "invalid current-state checkpoint link")?;
            let native = verify_new_root_checkpoint(root, checkpoint_link.clone())?;
            let checkpoint_status = SystemSnapshotStatus::Available {
                snapper_config: checkpoint_link.snapper_config.clone(),
                snapshot_id: checkpoint_link.snapper_snapshot_id.clone(),
                filesystem_uuid: checkpoint_link.filesystem_uuid.clone(),
            };
            let prepared = stores.begin(ConfigurationPreparationRequest {
                restore_id: restore_id.clone(),
                source: &source,
                scope: RestoreScope::ConfigurationOnly,
                compatibility: compatibility.clone(),
                checkpoint: GenerationCaptureRequest {
                    transaction_id: restore_id.clone(),
                    generation_id: id("native-linked-restore-checkpoint")?,
                    parent_generation_id: Some(source.manifest().generation_id.clone()),
                    snapshot: native.verified_link().clone(),
                    reason: OperationReason::ConfigurationRestore,
                    created_utc: "2026-10-18T00:00:00Z".into(),
                    package_revisions: Vec::new(),
                    provider_revision: None,
                    profile_revision: None,
                    roots: &roots,
                    limits: CaptureLimits::default(),
                },
            })?;
            (prepared, checkpoint_status)
        };
        let destination =
            [ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &target_root }];
        drop(prepared);
        let interruption = Command::new(std::env::current_exe()?)
            .arg("--interrupt-configuration-restore")
            .env_clear()
            .status()?;
        if interruption.code() != Some(77) {
            return Err(
                "restore child did not exit at the durable ConfigurationRestored phase".into()
            );
        }
        let mut resumed = stores.resume(&restore_id, &compatibility, &checkpoint_status, &roots)?;
        let completed = stores.execute_configuration_restore(
            &mut resumed,
            &destination,
            || Ok(()),
            |_| Ok(()),
        )?;
        let restored_fixture = format!("{RESTORE_TARGET}/fixture.txt");
        let restored_metadata = fs::symlink_metadata(&restored_fixture)?;
        if completed.phase != RestorePhase::Complete
            || fs::read(&restored_fixture)? != CONTENT
            || restored_metadata.mode() & 0o7777 != 0o640
            || restored_metadata.uid() != 0
            || restored_metadata.gid() != 0
            || fs::symlink_metadata(format!("{RESTORE_TARGET}/{ABSENT_PATH}")).is_ok()
        {
            return Err("configuration restore did not reproduce the linked fixture".into());
        }
        let restored_acl = File::open(&restored_fixture)?;
        verify_named_principal_acl(&restored_acl)?;
        let sentinel_path = format!("{RESTORE_TARGET}/unrelated-sentinel");
        let sentinel_after = fs::symlink_metadata(&sentinel_path)?;
        if fs::read(&sentinel_path)? != b"unrelated managed-state sentinel\n"
            || sentinel_after.dev() != sentinel_before.dev()
            || sentinel_after.ino() != sentinel_before.ino()
            || sentinel_after.mode() != sentinel_before.mode()
            || sentinel_after.uid() != sentinel_before.uid()
            || sentinel_after.gid() != sentinel_before.gid()
        {
            return Err("configuration restore changed the unrelated sentinel".into());
        }
        let home_sentinel_after = fs::symlink_metadata(HOME_SENTINEL)?;
        if fs::read(HOME_SENTINEL)? != b"unrelated home sentinel\n"
            || home_sentinel_after.dev() != home_sentinel_before.dev()
            || home_sentinel_after.ino() != home_sentinel_before.ino()
            || home_sentinel_after.mode() != home_sentinel_before.mode()
            || home_sentinel_after.uid() != home_sentinel_before.uid()
            || home_sentinel_after.gid() != home_sentinel_before.gid()
        {
            return Err("configuration restore changed the unrelated home sentinel".into());
        }
        GenerationReader::open_state_mount()?.verify_transaction(&id(TRANSACTION)?)?;
        GenerationReader::open_state_mount()?
            .verify_transaction(&id("native-linked-restore-attempt")?)?;
        Ok(())
    })();
    if target.is_ok() {
        fs::remove_dir_all(RESTORE_TARGET)?;
        fs::remove_file(HOME_SENTINEL)?;
        File::open("/home/tester")?.sync_all()?;
    }
    target?;
    println!("native_linked_configuration_restore=passed");
    println!("native_linked_restore_process_exit=passed");
    println!("native_linked_restore_restart=passed");
    println!("native_linked_named_acl_restore=passed");
    Ok(())
}

/// Resume the native configuration restore in a separate process and exit
/// without unwinding after ConfigurationRestored has reached durable storage.
fn interrupt_configuration_restore(root: &File) -> ProbeResult<()> {
    eprintln!("linked_restore_resume_step=machine_state");
    let machine = read_machine_state_record(root)?;
    eprintln!("linked_restore_resume_step=source_generation");
    let reader = GenerationReader::open_state_mount()?;
    let source = reader.open_historical_transaction(root, &id(TRANSACTION)?)?;
    let restore_id = id("native-linked-restore-attempt")?;
    eprintln!("linked_restore_resume_step=journal");
    let journal_store = RestoreJournalStore::open_existing_state_mount()?;
    let journal = journal_store.load(&restore_id)?;
    eprintln!("linked_restore_resume_step=checkpoint_generation");
    let generations = GenerationStore::open_initialized_state_mount(machine.identity())?;
    let checkpoint = generations.verify(&journal.current_state_generation)?;
    let checkpoint_link = SnapshotLink::new(
        checkpoint.snapper_config.clone(),
        checkpoint.snapper_snapshot_id.as_str(),
        &checkpoint.filesystem_uuid,
    )
    .map_err(|_| "invalid current-state checkpoint link in restore journal")?;
    eprintln!("linked_restore_resume_step=native_snapshot");
    let _native = verify_new_root_checkpoint(root, checkpoint_link.clone())?;
    let compatibility = RestoreCompatibility {
        source_snapshot: SystemSnapshotStatus::Available {
            snapper_config: source.generation().manifest().snapper_config.clone(),
            snapshot_id: source.generation().manifest().snapper_snapshot_id.clone(),
            filesystem_uuid: source.generation().manifest().filesystem_uuid.clone(),
        },
        system: CompatibilityStatus::Unknown,
        configuration: CompatibilityStatus::Compatible,
    };
    let checkpoint_status = SystemSnapshotStatus::Available {
        snapper_config: checkpoint_link.snapper_config.clone(),
        snapshot_id: checkpoint_link.snapper_snapshot_id.clone(),
        filesystem_uuid: checkpoint_link.filesystem_uuid.clone(),
    };
    let paths = [
        RelativePath::new("fixture.txt").map_err(|_| "invalid restore fixture path")?,
        RelativePath::new(ABSENT_PATH).map_err(|_| "invalid restore absent path")?,
    ];
    let target_root = File::open(RESTORE_TARGET)?;
    let roots =
        [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &target_root, paths: &paths }];
    let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state_fd);
    let stage_fd = openat2(
        &state,
        "staging",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?;
    let stage_root = File::from(stage_fd);
    let stages = DiskStageStore::open(&stage_root)?;
    let store = GenerationStore::open_initialized_state_mount(machine.identity())?;
    let stores =
        PreparationStores { generations: &store, journals: &journal_store, stages: &stages };
    eprintln!("linked_restore_resume_step=preparation_resume");
    let mut prepared = stores.resume(&restore_id, &compatibility, &checkpoint_status, &roots)?;
    eprintln!("linked_restore_resume_step=configuration_apply");
    stores.execute_configuration_restore(
        &mut prepared,
        &[ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &target_root }],
        || Ok(()),
        |_| std::process::exit(77),
    )?;
    Err("restore child returned without reaching the interruption boundary".into())
}

fn exercise_manifest_symlink_refusal(root: &File, reader: &GenerationReader) -> ProbeResult<()> {
    const BACKUP: &str = "native-linked-symlink-manifest-backup";
    const REPLACEMENT: &str = "native-linked-symlink-manifest-replacement";

    let machine = read_machine_state_record(root)?;
    let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state_fd);
    let staging = File::from(openat2(
        &state,
        "staging",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let generation = File::from(openat2(
        &state,
        format!("generations/{GENERATION}").as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    for name in [BACKUP, REPLACEMENT] {
        match statat(&staging, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => {
                return Err(format!(
                    "prior symlink probe entry {name} exists; explicit reconciliation required"
                )
                .into());
            }
            Err(Errno::NOENT) => (),
            Err(error) => return Err(error.into()),
        }
    }

    let original_file = File::from(openat2(
        &generation,
        "manifest.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let original_stat = rustix::fs::fstat(&original_file)?;
    if rustix::fs::FileType::from_raw_mode(original_stat.st_mode)
        != rustix::fs::FileType::RegularFile
        || original_stat.st_uid != 0
        || original_stat.st_nlink != 1
        || original_stat.st_mode & 0o7777 != 0o400
    {
        return Err("generation manifest has unsafe metadata before symlink probe".into());
    }
    let mut original = Vec::new();
    original_file.take(32 * 1024 + 1).read_to_end(&mut original)?;
    if original.len() > 32 * 1024 {
        return Err("generation manifest exceeds symlink-probe evidence limit".into());
    }

    // Retain a durable byte copy and a same-filesystem exchange path before
    // replacing the sealed manifest. If interrupted, the original remains in
    // staging under REPLACEMENT and this probe refuses to overwrite either
    // recovery artifact on a retry.
    let mut backup = File::from(openat2(
        &staging,
        BACKUP,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    backup.write_all(&original)?;
    fsync(&backup)?;
    drop(backup);
    rustix::fs::symlinkat("/etc/passwd", &staging, REPLACEMENT)?;
    fsync(&staging)?;

    let exchange = || -> ProbeResult<()> {
        rustix::fs::renameat_with(
            &generation,
            "manifest.json",
            &staging,
            REPLACEMENT,
            rustix::fs::RenameFlags::EXCHANGE,
        )?;
        fsync(&generation)?;
        fsync(&staging)?;
        Ok(())
    };
    exchange()?;
    let refusal = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    // Restore the original manifest before checking the result. If this fails,
    // the original bytes remain reachable through the staging exchange path.
    exchange()?;

    let restored = File::from(openat2(
        &generation,
        "manifest.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let restored_stat = rustix::fs::fstat(&restored)?;
    let mut restored_bytes = Vec::new();
    restored.take(32 * 1024 + 1).read_to_end(&mut restored_bytes)?;
    if rustix::fs::FileType::from_raw_mode(restored_stat.st_mode)
        != rustix::fs::FileType::RegularFile
        || restored_stat.st_uid != original_stat.st_uid
        || restored_stat.st_gid != original_stat.st_gid
        || restored_stat.st_nlink != original_stat.st_nlink
        || restored_stat.st_mode & 0o7777 != original_stat.st_mode & 0o7777
        || restored_bytes != original
    {
        return Err("original manifest did not restore exactly after symlink probe".into());
    }
    let exchanged = statat(&staging, REPLACEMENT, AtFlags::SYMLINK_NOFOLLOW)?;
    if rustix::fs::FileType::from_raw_mode(exchanged.st_mode) != rustix::fs::FileType::Symlink {
        return Err("manifest symlink exchange path changed unexpectedly".into());
    }
    unlinkat(&staging, REPLACEMENT, AtFlags::empty())?;
    unlinkat(&staging, BACKUP, AtFlags::empty())?;
    fsync(&staging)?;

    if !matches!(refusal, Err(HistoricalLookupError::GenerationInvalid(_))) {
        return Err("symlink manifest was not refused as an invalid generation".into());
    }
    println!("native_linked_symlink_manifest_refused=passed");
    verify(root, false)
}

fn exercise_failed_capture(root: &File) -> ProbeResult<()> {
    const FAILED_TRANSACTION: &str = "native-linked-capture-failure";
    const FAILED_GENERATION: &str = "native-linked-failed-generation";
    const FAILED_SOURCE: &str = "/run/ryoku-native-linked-capture-failure";

    let machine = read_machine_state_record(root)?;
    let store = GenerationStore::open_initialized_state_mount(machine.identity())?;
    fs::DirBuilder::new().mode(0o700).create(FAILED_SOURCE)?;
    // An absolute target is outside the declared restore scope. Capture must
    // reject it as data and must never open or archive the referenced file.
    std::os::unix::fs::symlink("/etc/passwd", format!("{FAILED_SOURCE}/fixture.txt"))?;
    let source = File::open(FAILED_SOURCE)?;
    let snapshot = SnapperCheckpointBackend.create_single(&request(FAILED_TRANSACTION)?)?;
    let native = verify_new_root_checkpoint(
        root,
        SnapshotLink::new(id("root")?, snapshot.as_str(), &machine.identity().filesystem_uuid)
            .map_err(|_| "invalid failed-capture snapshot link")?,
    )?;
    let paths = [
        RelativePath::new("fixture.txt").map_err(|_| "invalid fixed failure path")?,
        RelativePath::new(ABSENT_PATH).map_err(|_| "invalid fixed absent path")?,
    ];
    let roots = [DeclaredRoot { root: StateRoot::EtcRyoku, descriptor: &source, paths: &paths }];
    let result = capture_publish_verify(
        &store,
        GenerationCaptureRequest {
            transaction_id: id(FAILED_TRANSACTION)?,
            generation_id: id(FAILED_GENERATION)?,
            parent_generation_id: None,
            snapshot: native.verified_link().clone(),
            reason: OperationReason::ProfileApply,
            created_utc: "2026-10-15T00:00:01Z".into(),
            package_revisions: Vec::new(),
            provider_revision: None,
            profile_revision: None,
            roots: &roots,
            limits: CaptureLimits::default(),
        },
    );
    if !matches!(
        result,
        Err(GenerationCaptureError::Capture(
            ryoku_recovery::capture::CaptureError::UnsupportedEntryType
        ))
    ) {
        return Err("out-of-scope symlink did not fail the complete capture".into());
    }
    let transaction = id(FAILED_TRANSACTION)?;
    let published = GenerationReader::open_state_mount()?.list_transactions()?;
    if store.verify_transaction(&transaction).is_ok() || published.contains(&transaction) {
        return Err("failed capture published a generation catalog entry".into());
    }
    println!("native_linked_capture_failure_unpublished=passed");
    Ok(())
}

fn exercise_corrupt_manifest(root: &File, reader: &GenerationReader) -> ProbeResult<()> {
    exercise_corrupt_generation_artifact(
        root,
        reader,
        CorruptionSpec {
            artifact_path: "manifest.json",
            backup_name: "native-linked-manifest-backup",
            corrupted_bytes: Some(b"{"),
            success_marker: "native_linked_corrupt_manifest_refused=passed",
            description: "manifest",
            expected_refusal: None,
            corrupted_mode: None,
            corrupted_owner: None,
        },
    )
}

fn exercise_traversal_manifest(root: &File, reader: &GenerationReader) -> ProbeResult<()> {
    let machine = read_machine_state_record(root)?;
    let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state_fd);
    let generation = File::from(openat2(
        &state,
        format!("generations/{GENERATION}").as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let manifest = File::from(openat2(
        &generation,
        "manifest.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let metadata = rustix::fs::fstat(&manifest)?;
    if rustix::fs::FileType::from_raw_mode(metadata.st_mode) != rustix::fs::FileType::RegularFile
        || metadata.st_uid != 0
        || metadata.st_gid != 0
        || metadata.st_nlink != 1
        || metadata.st_mode & 0o7777 != 0o400
    {
        return Err("generation traversal fixture has unsafe manifest metadata".into());
    }
    let mut original = Vec::new();
    manifest.take(32 * 1024 + 1).read_to_end(&mut original)?;
    if original.len() > 32 * 1024 {
        return Err("generation manifest exceeds fixture evidence limit".into());
    }
    let mut value: serde_json::Value = serde_json::from_slice(&original)?;
    let files = value
        .get_mut("files")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("generation manifest has no file records")?;
    let fixture = files
        .iter_mut()
        .find(|record| {
            record.get("path").and_then(serde_json::Value::as_str) == Some("fixture.txt")
        })
        .ok_or("generation manifest has no fixture record")?;
    fixture["path"] = serde_json::Value::String("../etc/passwd".into());
    let traversal = serde_json::to_vec(&value)?;
    exercise_corrupt_generation_artifact(
        root,
        reader,
        CorruptionSpec {
            artifact_path: "manifest.json",
            backup_name: "native-linked-traversal-backup",
            corrupted_bytes: Some(&traversal),
            success_marker: "native_linked_traversal_manifest_refused=passed",
            description: "traversal manifest",
            expected_refusal: Some("InvalidRelativePath"),
            corrupted_mode: None,
            corrupted_owner: None,
        },
    )
}

fn exercise_corrupt_payload(
    root: &File,
    reader: &GenerationReader,
    manifest: &RecoveryManifest,
) -> ProbeResult<()> {
    let record = manifest
        .files()
        .iter()
        .find(|entry| entry.path.as_str() == "fixture.txt")
        .ok_or("missing payload selected for corruption")?;
    let ryoku_recovery::EntryKind::RegularFile { sha256, .. } = &record.kind else {
        return Err("fixture payload is not a regular file".into());
    };
    exercise_corrupt_generation_artifact(
        root,
        reader,
        CorruptionSpec {
            artifact_path: &format!("payloads/{}.blob", sha256.as_str()),
            backup_name: "native-linked-payload-backup",
            corrupted_bytes: Some(b"corrupt payload\n"),
            success_marker: "native_linked_corrupt_payload_refused=passed",
            description: "payload",
            expected_refusal: None,
            corrupted_mode: None,
            corrupted_owner: None,
        },
    )
}

fn exercise_artifact_mode_refusal(root: &File, reader: &GenerationReader) -> ProbeResult<()> {
    exercise_corrupt_generation_artifact(
        root,
        reader,
        CorruptionSpec {
            artifact_path: "manifest.json",
            backup_name: "native-linked-mode-backup",
            corrupted_bytes: None,
            success_marker: "native_linked_artifact_mode_refused=passed",
            description: "artifact mode",
            expected_refusal: Some("unsafe type or mode"),
            corrupted_mode: Some(0o600),
            corrupted_owner: None,
        },
    )
}

fn exercise_artifact_owner_refusal(root: &File, reader: &GenerationReader) -> ProbeResult<()> {
    exercise_corrupt_generation_artifact(
        root,
        reader,
        CorruptionSpec {
            artifact_path: "manifest.json",
            backup_name: "native-linked-owner-backup",
            corrupted_bytes: None,
            success_marker: "native_linked_artifact_owner_refused=passed",
            description: "artifact owner",
            expected_refusal: Some("unsafe type or mode"),
            corrupted_mode: None,
            corrupted_owner: Some((1, 1)),
        },
    )
}

fn exercise_artifact_group_refusal(root: &File, reader: &GenerationReader) -> ProbeResult<()> {
    exercise_corrupt_generation_artifact(
        root,
        reader,
        CorruptionSpec {
            artifact_path: "manifest.json",
            backup_name: "native-linked-group-backup",
            corrupted_bytes: None,
            success_marker: "native_linked_artifact_group_refused=passed",
            description: "artifact group",
            expected_refusal: Some("unsafe type or mode"),
            corrupted_mode: None,
            corrupted_owner: Some((0, 1)),
        },
    )
}

fn exercise_artifact_directory_group_refusal(
    root: &File,
    reader: &GenerationReader,
) -> ProbeResult<()> {
    const BACKUP: &str = "native-linked-directory-group-backup";
    const DIRECTORY: &str = "payloads";

    let machine = read_machine_state_record(root)?;
    let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state_fd);
    let staging = File::from(openat2(
        &state,
        "staging",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let generation = File::from(openat2(
        &state,
        format!("generations/{GENERATION}").as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    match statat(&staging, BACKUP, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(
                "directory group probe backup exists; explicit reconciliation required".into()
            );
        }
        Err(Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    let directory = File::from(openat2(
        &generation,
        DIRECTORY,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let original = rustix::fs::fstat(&directory)?;
    if rustix::fs::FileType::from_raw_mode(original.st_mode) != rustix::fs::FileType::Directory
        || original.st_uid != 0
        || original.st_gid != 0
        || original.st_mode & 0o7777 != 0o500
    {
        return Err("generation payload directory has unsafe type, owner, group or mode".into());
    }
    let original_record =
        serde_json::to_vec(&(1_u16, original.st_uid, original.st_gid, original.st_mode & 0o7777))?;
    let mut backup = File::from(openat2(
        &staging,
        BACKUP,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    backup.write_all(&original_record)?;
    fsync(&backup)?;
    drop(backup);
    fsync(&staging)?;

    let outcome = (|| -> ProbeResult<()> {
        rustix::fs::fchown(&directory, None, Some(rustix::fs::Gid::from_raw(1)))?;
        fsync(&directory)?;
        if !matches!(
            reader.open_historical_transaction(&File::open("/")?, &id(TRANSACTION)?),
            Err(HistoricalLookupError::GenerationInvalid(_))
        ) {
            return Err("generation child directory with changed GID was not refused".into());
        }
        Ok(())
    })();

    let restore = (|| -> ProbeResult<()> {
        let backup = File::from(openat2(
            &staging,
            BACKUP,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        let backup_stat = rustix::fs::fstat(&backup)?;
        if rustix::fs::FileType::from_raw_mode(backup_stat.st_mode)
            != rustix::fs::FileType::RegularFile
            || backup_stat.st_uid != 0
            || backup_stat.st_gid != 0
            || backup_stat.st_nlink != 1
            || backup_stat.st_mode & 0o7777 != 0o600
        {
            return Err("generation directory group backup has unsafe metadata".into());
        }
        let mut saved = Vec::new();
        backup.take(256).read_to_end(&mut saved)?;
        if saved != original_record {
            return Err("generation directory group backup changed during probe".into());
        }
        rustix::fs::fchown(
            &directory,
            Some(rustix::fs::Uid::from_raw(original.st_uid)),
            Some(rustix::fs::Gid::from_raw(original.st_gid)),
        )?;
        rustix::fs::fchmod(&directory, Mode::from_raw_mode(original.st_mode & 0o7777))?;
        fsync(&directory)?;
        let restored = rustix::fs::fstat(&directory)?;
        if restored.st_uid != original.st_uid
            || restored.st_gid != original.st_gid
            || restored.st_mode & 0o7777 != original.st_mode & 0o7777
        {
            return Err("restored generation directory metadata differs from backup".into());
        }
        unlinkat(&staging, BACKUP, AtFlags::empty())?;
        fsync(&staging)?;
        Ok(())
    })();
    restore?;
    outcome?;
    reader.open_historical_transaction(&File::open("/")?, &id(TRANSACTION)?)?;
    println!("native_linked_artifact_directory_group_refused=passed");
    Ok(())
}

struct CorruptionSpec<'a> {
    artifact_path: &'a str,
    backup_name: &'a str,
    corrupted_bytes: Option<&'a [u8]>,
    success_marker: &'a str,
    description: &'a str,
    expected_refusal: Option<&'a str>,
    corrupted_mode: Option<u32>,
    corrupted_owner: Option<(u32, u32)>,
}

fn exercise_corrupt_generation_artifact(
    root: &File,
    reader: &GenerationReader,
    spec: CorruptionSpec<'_>,
) -> ProbeResult<()> {
    let CorruptionSpec {
        artifact_path,
        backup_name,
        corrupted_bytes,
        success_marker,
        description,
        expected_refusal,
        corrupted_mode,
        corrupted_owner,
    } = spec;
    // Keep a durable copy outside the immutable generation. If the process or
    // VM stops during this destructive probe, the next invocation refuses to
    // overwrite this receipt and an operator can restore it explicitly.
    let machine = read_machine_state_record(root)?;
    let (state_fd, _) = collect_and_open_state_mount(root, machine.identity())?;
    let state = File::from(state_fd);
    let staging = File::from(openat2(
        &state,
        "staging",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let generation = File::from(openat2(
        &state,
        format!("generations/{GENERATION}").as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    match statat(&staging, backup_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => {
            return Err(format!(
                "{description} probe backup exists; explicit reconciliation required"
            )
            .into());
        }
        Err(Errno::NOENT) => (),
        Err(error) => return Err(error.into()),
    }
    let original_file = File::from(openat2(
        &generation,
        artifact_path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let original_stat = rustix::fs::fstat(&original_file)?;
    if rustix::fs::FileType::from_raw_mode(original_stat.st_mode)
        != rustix::fs::FileType::RegularFile
        || original_stat.st_uid != 0
        || original_stat.st_gid != 0
        || original_stat.st_nlink != 1
        || original_stat.st_mode & 0o7777 != 0o400
    {
        return Err(
            format!("generation {description} has unsafe type, owner, links or mode").into()
        );
    }
    let original_mode = original_stat.st_mode & 0o7777;
    let mut original = Vec::new();
    original_file.take(32 * 1024 + 1).read_to_end(&mut original)?;
    if original.len() > 32 * 1024 {
        return Err(format!("generation {description} exceeds fixture evidence limit").into());
    }
    if corrupted_bytes.is_some_and(|corrupted| original == corrupted) {
        return Err(format!("corruption input does not alter generation {description}").into());
    }
    let mut backup = File::from(openat2(
        &staging,
        backup_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    backup.write_all(&original)?;
    fsync(&backup)?;
    drop(backup);
    fsync(&staging)?;

    let outcome = (|| -> ProbeResult<()> {
        let mut manifest = File::from(openat2(
            &generation,
            artifact_path,
            if corrupted_bytes.is_some() {
                OFlags::WRONLY | OFlags::TRUNC | OFlags::NONBLOCK | OFlags::CLOEXEC
            } else {
                OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC
            },
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        if let Some(corrupted_bytes) = corrupted_bytes {
            manifest.write_all(corrupted_bytes)?;
        }
        if let Some(corrupted_mode) = corrupted_mode {
            fchmod(&manifest, Mode::from_raw_mode(corrupted_mode))?;
        } else {
            fchmod(&manifest, Mode::from_raw_mode(original_mode))?;
        }
        if let Some((uid, gid)) = corrupted_owner {
            rustix::fs::fchown(
                &manifest,
                Some(rustix::fs::Uid::from_raw(uid)),
                Some(rustix::fs::Gid::from_raw(gid)),
            )?;
        }
        fsync(&manifest)?;
        let refusal = reader.open_historical_transaction(&File::open("/")?, &id(TRANSACTION)?);
        let rejected = match refusal {
            Err(HistoricalLookupError::GenerationInvalid(error)) => {
                expected_refusal.is_none_or(|expected| error.to_string().contains(expected))
            }
            _ => false,
        };
        if !rejected {
            return Err(format!("corrupt generation {description} was not refused").into());
        }
        Ok(())
    })();

    let restore = (|| -> ProbeResult<()> {
        let backup = File::from(openat2(
            &staging,
            backup_name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        let backup_stat = rustix::fs::fstat(&backup)?;
        if rustix::fs::FileType::from_raw_mode(backup_stat.st_mode)
            != rustix::fs::FileType::RegularFile
            || backup_stat.st_uid != 0
            || backup_stat.st_gid != 0
            || backup_stat.st_nlink != 1
            || backup_stat.st_mode & 0o7777 != 0o600
        {
            return Err(format!(
                "generation {description} backup has unsafe type, owner, links or mode"
            )
            .into());
        }
        let mut backup_bytes = Vec::new();
        backup.take(32 * 1024 + 1).read_to_end(&mut backup_bytes)?;
        if backup_bytes.len() > 32 * 1024 {
            return Err(
                format!("generation {description} backup exceeds fixture evidence limit").into()
            );
        }
        if backup_bytes != original {
            return Err(format!("generation {description} backup changed during probe").into());
        }
        let mut manifest = File::from(openat2(
            &generation,
            artifact_path,
            OFlags::WRONLY | OFlags::TRUNC | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        manifest.write_all(&backup_bytes)?;
        fsync(&manifest)?;
        rustix::fs::fchown(
            &manifest,
            Some(rustix::fs::Uid::from_raw(original_stat.st_uid)),
            Some(rustix::fs::Gid::from_raw(original_stat.st_gid)),
        )?;
        fchmod(&manifest, Mode::from_raw_mode(original_mode))?;
        fsync(&manifest)?;
        let restored = File::from(openat2(
            &generation,
            artifact_path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        let restored_stat = rustix::fs::fstat(&restored)?;
        if rustix::fs::FileType::from_raw_mode(restored_stat.st_mode)
            != rustix::fs::FileType::RegularFile
            || restored_stat.st_uid != original_stat.st_uid
            || restored_stat.st_gid != original_stat.st_gid
            || restored_stat.st_nlink != 1
            || restored_stat.st_mode & 0o7777 != original_mode
        {
            return Err(format!(
                "restored generation {description} metadata differs from the original"
            )
            .into());
        }
        let mut restored_bytes = Vec::new();
        restored.take(32 * 1024 + 1).read_to_end(&mut restored_bytes)?;
        if restored_bytes != original {
            return Err(format!(
                "generation {description} restoration did not match its durable backup"
            )
            .into());
        }
        unlinkat(&staging, backup_name, AtFlags::empty())?;
        fsync(&staging)?;
        Ok(())
    })();
    restore?;
    outcome?;
    println!("{success_marker}");
    Ok(())
}
