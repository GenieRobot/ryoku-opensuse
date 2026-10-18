//! Component experiment restricted to the seeded disposable QEMU fixture.
//! The harness must initialize state storage first. This does not restore root
//! or configuration; it records/reopens a linked generation and Planned journal.
//! Fixed probe snapshots and records are retained until the disk is discarded.

use ryoku_recovery::{
    Identifier, OperationReason, RecoveryManifest, RelativePath, SnapshotLink, StateRoot,
    capture::CaptureLimits,
    generation::{GenerationReader, GenerationStore},
    generation_capture::{DeclaredRoot, GenerationCaptureRequest, capture_publish_verify},
    linked_checkpoint::HistoricalLookupError,
    machine_state::read_machine_state_record,
    native_checkpoint::{NativeCheckpointError, verify_new_root_checkpoint},
    restore::{RestoreJournal, RestoreJournalStore, RestorePhase, RestorePlan, RestoreScope},
    state_mount::collect_and_open_state_mount,
};
use ryoku_snapper::{
    ConfigName, SnapperCheckpointBackend, SnapperCliError, SnapshotPairRequest, TransactionId,
};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    process::Command,
};

type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;
const TRANSACTION: &str = "issue19-linked-probe";
const GENERATION: &str = "issue19-linked-generation";
const JOURNAL: &str = "issue19-linked-journal";
const SOURCE: &str = "/run/ryoku-issue19-linked-source";
const CONTENT: &[u8] = b"synthetic linked checkpoint payload\n";

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
                | "--observe-layout"
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
        "--observe-layout" => observe_layout()?,
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
    let paths = [RelativePath::new("fixture.txt").map_err(|_| "invalid fixed path")?];
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
            configuration_entries: 1,
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
    let record = manifest.files().first().ok_or("missing synthetic payload")?;
    let ryoku_recovery::EntryKind::RegularFile { sha256, .. } = &record.kind else {
        return Err("wrong synthetic payload type".into());
    };
    if linked.generation().read_payload(sha256)? != CONTENT {
        return Err("synthetic payload changed".into());
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
    let source = format!("/.snapshots/{}/snapshot", manifest.snapper_snapshot_id);
    let retained = format!("/.snapshots/{}/ryoku-probe-retained", manifest.snapper_snapshot_id);
    if fs::symlink_metadata(&retained).is_ok() {
        return Err("prior probe path exists; explicit reconciliation required".into());
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
        SnapperCheckpointBackend.create_single(&request("issue19-linked-substitute")?)?;
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

    let generation = format!("/var/lib/ryoku-state/generations/{GENERATION}");
    let missing = format!("/var/lib/ryoku-state/generations/{GENERATION}-retained");
    if fs::symlink_metadata(&missing).is_ok() {
        return Err("prior generation probe path exists".into());
    }
    fs::rename(&generation, &missing)?;
    let absent = reader.open_historical_transaction(root, &id(TRANSACTION)?);
    fs::rename(&missing, &generation)?;
    if !matches!(absent, Err(HistoricalLookupError::GenerationMissing)) {
        return Err("missing generation was not refused".into());
    }
    println!("native_linked_missing_generation_refused=passed");
    verify(root, false)
}
