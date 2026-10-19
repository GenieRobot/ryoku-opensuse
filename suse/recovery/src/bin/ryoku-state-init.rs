//! Initial dedicated-state provisioning. Not yet installed by RPM packaging.
//! Root-only and fixed-path; failures preserve checkpoint and creation receipts.
use rustix::fs::{
    FlockOperation, Mode, OFlags, ResolveFlags, fchmod, flock, fstat, fsync, mkdirat, openat2,
};
use ryoku_recovery::{
    Identifier, SnapshotLink,
    generation::{GenerationReader, GenerationStore},
    machine_state::{MachineStateError, provision_machine_state_record, read_machine_state_record},
    native_checkpoint::verify_new_root_checkpoint,
    restore::RestoreJournalStore,
    state_creation::{CreationError, StateCreationTransaction, has_pending_state_creation},
    state_mount::{StateMountError, collect_and_open_state_mount},
    state_provision::{
        CreatedStateSubvolume, STATE_MOUNT_UNIT, StateProvisionError, provisioning_root_uuid,
        verify_initialized_state_root, verify_recorded_state_mount_unit,
    },
};
use ryoku_snapper::{ConfigName, SnapperCheckpointBackend, SnapshotPairRequest, TransactionId};
use std::{
    fs::{self, File},
    io::Read,
    process::{Command, ExitCode},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let action = match parse_action(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    match initialize(action) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ryoku-state-init: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug)]
enum Action {
    Initialize,
    Resume { transaction: Identifier, snapshot: String },
}

fn parse_action(args: &[String]) -> std::result::Result<Action, String> {
    match args {
        [command] if command == "initialize" => Ok(Action::Initialize),
        [command, transaction, snapshot] if command == "resume" => {
            let transaction = Identifier::new(transaction).map_err(|_| "invalid transaction ID")?;
            let number = snapshot.parse::<u64>().map_err(|_| "invalid snapshot ID")?;
            if number == 0 || number.to_string() != *snapshot {
                return Err("invalid snapshot ID".into());
            }
            Ok(Action::Resume { transaction, snapshot: snapshot.clone() })
        }
        _ => Err("Usage: ryoku-state-init initialize | resume TRANSACTION_ID SNAPSHOT_ID".into()),
    }
}

fn initialize(action: Action) -> Result<()> {
    if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0 {
        return Err("root user and group required".into());
    }
    let root = File::open("/")?;
    let run = open_owned_parent(&root, "run")?;
    let initializer_lock = File::from(openat2(
        &run,
        ".ryoku-state-initialize.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let stat = fstat(&initializer_lock)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != 0o600
    {
        return Err("unsafe initializer lock".into());
    }
    flock(&initializer_lock, FlockOperation::NonBlockingLockExclusive)?;
    if matches!(&action, Action::Initialize) {
        match read_machine_state_record(&root) {
            Ok(record) => {
                verify_initialized_state_root(&root, &record)?;
                collect_and_open_state_mount(&root, record.identity())?;
                let _reader = GenerationReader::open_state_mount()?;
                verify_recorded_state_mount_unit(&root)?;
                if !RestoreJournalStore::open_existing_state_mount()?
                    .incomplete_attempts()?
                    .is_empty()
                {
                    return Err(
                        "unfinished recovery requires explicit recovery coordination".into()
                    );
                }
                println!("State store already initialized and available");
                return Ok(());
            }
            Err(MachineStateError::Filesystem(rustix::io::Errno::NOENT)) => (),
            Err(error) => return Err(error.into()),
        }
    }
    // Fresh provisioning and explicit resume retain the stricter direct-child
    // root rule. A completed store's read-only verification above also works
    // after classic rollback changes the running root's parent and UUID.
    let uuid = provisioning_root_uuid(&root)?;
    verify_snapper_mapping(&root)?;
    if matches!(&action, Action::Initialize) && has_pending_state_creation(&root)? {
        return Err(
            "incomplete state initialization requires validated resume or explicit reconciliation"
                .into(),
        );
    }
    if let Action::Resume { transaction, snapshot } = action {
        let checkpoint = verify_new_root_checkpoint(
            &root,
            SnapshotLink::new(
                Identifier::new("root").map_err(|_| "invalid config")?,
                snapshot,
                uuid.clone(),
            )
            .map_err(|_| "invalid checkpoint link")?,
        )?;
        match StateCreationTransaction::resume_created(&root, &checkpoint, transaction.clone()) {
            Ok((_guard, created)) => configure_created(&root, &created, false)?,
            Err(CreationError::Creation(StateProvisionError::Observation(
                StateMountError::MissingMount,
            ))) => {
                with_top_level(&run, &uuid, |top| {
                    let (_guard, created) = StateCreationTransaction::resume_created_from_top(
                        &root,
                        &checkpoint,
                        transaction,
                        top,
                    )?;
                    // Only an absent target can be created. An existing unknown
                    // directory is never repaired, adopted or mounted over.
                    configure_created(&root, &created, true)
                })?;
            }
            Err(error) => return Err(error.into()),
        }
        println!("State store initialization resumed; bootstrap checkpoint retained");
        return Ok(());
    }
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let transaction = format!("state-initialize-{}", boot.trim());
    let transaction_id = Identifier::new(&transaction).map_err(|_| "invalid boot identity")?;
    let request = SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|_| "invalid fixed config")?,
        transaction_id: TransactionId::new(&transaction).map_err(|_| "invalid transaction")?,
    };
    let snapshot = SnapperCheckpointBackend.create_single(&request)?;
    let checkpoint = verify_new_root_checkpoint(
        &root,
        SnapshotLink::new(
            Identifier::new("root").map_err(|_| "invalid config")?,
            snapshot.as_str(),
            uuid.clone(),
        )
        .map_err(|_| "invalid snapshot link")?,
    )?;
    eprintln!("Bootstrap checkpoint root:{}; transaction {}", snapshot.as_str(), transaction);
    let mut provisioning = StateCreationTransaction::begin(&root, &checkpoint, transaction_id)?;
    with_top_level(&run, &uuid, |top| {
        let created = provisioning.create(top)?;
        configure_created(&root, &created, true)
    })?;
    println!("State store initialized; bootstrap checkpoint retained");
    Ok(())
}

fn configure_created(
    root: &File,
    created: &CreatedStateSubvolume,
    prepare_mountpoint: bool,
) -> Result<()> {
    if prepare_mountpoint {
        let var = open_owned_parent(root, "var")?;
        let lib = open_owned_parent(&var, "lib")?;
        mkdirat(&lib, "ryoku-state", Mode::from_raw_mode(0o700))?;
        let mountpoint = File::from(openat2(
            &lib,
            "ryoku-state",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?);
        fchmod(&mountpoint, Mode::from_raw_mode(0o700))?;
        fsync(&lib)?;
    } else {
        // Resume never creates or adopts an unknown underlying directory.
        collect_and_open_state_mount(root, created.machine_record().identity())?;
    }
    created.install_mount_unit(root)?;
    let unit_path = format!("/etc/systemd/system/{STATE_MOUNT_UNIT}");
    run_command("/usr/sbin/restorecon", &[&unit_path, "/var/lib/ryoku-state"])?;
    run_command("/usr/bin/systemctl", &["daemon-reload"])?;
    run_command("/usr/bin/systemctl", &["enable", "--now", STATE_MOUNT_UNIT])?;
    // Make enablement directory entries durable before root identity is published.
    let system = open_owned_parent(root, "etc")?;
    let system = open_owned_parent(&system, "systemd")?;
    let system = open_owned_parent(&system, "system")?;
    let wants = open_owned_parent(&system, "local-fs.target.wants")?;
    fsync(&wants)?;
    fsync(&system)?;
    collect_and_open_state_mount(root, created.machine_record().identity())?;
    let _store = GenerationStore::open_state_mount(created.machine_record().identity())?;
    let _journals =
        RestoreJournalStore::initialize_state_mount(created.machine_record().identity())?;
    run_command("/usr/sbin/restorecon", &["-R", "/var/lib/ryoku-state"])?;
    provision_machine_state_record(root, created.machine_record())?;
    let _reader = GenerationReader::open_state_mount()?;
    Ok(())
}

fn with_top_level(
    run: &File,
    uuid: &str,
    operation: impl FnOnce(&File) -> Result<()>,
) -> Result<()> {
    let mut entropy = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let name = format!("ryoku-state-init-top-{:032x}", u128::from_le_bytes(entropy));
    mkdirat(run, name.as_str(), Mode::from_raw_mode(0o700))?;
    let directory = open_owned_parent(run, &name)?;
    fchmod(&directory, Mode::from_raw_mode(0o700))?;
    drop(directory);
    let path = format!("/run/{name}");
    let device = format!("/dev/disk/by-uuid/{uuid}");
    if let Err(error) = run_command(
        "/usr/bin/mount",
        &["-t", "btrfs", "-o", "subvolid=5,nodev,nosuid,noexec", &device, &path],
    ) {
        let _ = fs::remove_dir(&path);
        return Err(error);
    }
    let outcome = match File::open(&path) {
        Ok(top) => operation(&top),
        Err(error) => Err(error.into()),
    };
    // Never remove persistent state or a pre-existing mount. This path was
    // created exclusively by this invocation; a killed attempt is retained.
    let cleanup = run_command("/usr/bin/umount", &[&path])
        .and_then(|()| fs::remove_dir(&path).map_err(Into::into));
    outcome?;
    cleanup
}

fn open_owned_parent(parent: &File, name: &str) -> Result<File> {
    let directory = File::from(openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let stat = fstat(&directory)?;
    if stat.st_uid != 0 || stat.st_gid != 0 || stat.st_mode & 0o022 != 0 {
        return Err("unsafe provisioning parent directory".into());
    }
    Ok(directory)
}

fn verify_snapper_mapping(root: &File) -> Result<()> {
    let etc = open_owned_parent(root, "etc")?;
    let snapper = open_owned_parent(&etc, "snapper")?;
    let configs = open_owned_parent(&snapper, "configs")?;
    let file = File::from(openat2(
        &configs,
        "root",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let stat = fstat(&file)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_nlink != 1
        || stat.st_mode & 0o022 != 0
        || stat.st_size <= 0
        || stat.st_size > 128 * 1024
    {
        return Err("unsafe Snapper root configuration".into());
    }
    let mut contents = String::new();
    file.take(128 * 1024 + 1).read_to_string(&mut contents)?;
    if contents.len() > 128 * 1024 {
        return Err("Snapper configuration grew beyond bound".into());
    }
    if !mapping_is_root(&contents) {
        return Err("Snapper root mapping is unsupported or ambiguous".into());
    }
    Ok(())
}

fn mapping_is_root(contents: &str) -> bool {
    let values: Vec<_> = contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .filter(|(key, _)| matches!(*key, "SUBVOLUME" | "FSTYPE"))
        .collect();
    values.len() == 2
        && values.contains(&("SUBVOLUME", "\"/\""))
        && values.contains(&("FSTYPE", "\"btrfs\""))
}

fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new("/usr/bin/timeout")
        .args(["--signal=TERM", "--kill-after=2s", "60s", program])
        .args(args)
        .status()?;
    if !status.success() {
        return Err(format!("{program} failed ({status})").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn resume_arguments_are_bounded_typed_identifiers_without_paths_or_options() {
        let valid = ["resume", "state-initialize-a", "42"].map(str::to_owned);
        assert!(matches!(super::parse_action(&valid), Ok(super::Action::Resume { .. })));
        for invalid in [
            vec!["resume", "../other", "42"],
            vec!["resume", "attempt", "../42"],
            vec!["resume", "attempt", "042"],
            vec!["resume", "attempt", "0"],
            vec!["resume", "attempt", "--help"],
            vec!["resume", "attempt"],
            vec!["resume", "attempt", "42", "extra"],
        ] {
            assert!(
                super::parse_action(&invalid.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    .is_err()
            );
        }
    }

    #[test]
    fn snapper_mapping_refuses_duplicate_spaced_or_foreign_targets() {
        let valid = "SUBVOLUME=\"/\"\nFSTYPE=\"btrfs\"\n";
        assert!(super::mapping_is_root(valid));
        for invalid in [
            format!("{valid}SUBVOLUME = \"/home\"\n"),
            valid.replace("\"/\"", "\"/home\""),
            valid.replace("btrfs", "lvm"),
            format!("{valid}FSTYPE=\"btrfs\"\n"),
            "SUBVOLUME=\"/\"\n".into(),
        ] {
            assert!(!super::mapping_is_root(&invalid));
        }
    }
}
