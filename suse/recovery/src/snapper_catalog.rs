//! Snapper coordination catalog anchored at the validated rollback-independent
//! state mount. All partner observations reopen the exact sealed generation
//! and kernel snapshot identities; stored status is never fresh evidence.
use crate::{
    Identifier, RelativePath,
    generation::GenerationReader,
    machine_state::read_machine_state_record,
    native_checkpoint::verify_historical_root_checkpoint,
    restore::RestoreJournalStore,
    snapper_layout::{DiscoveredLayout, DiscoveredTarget, discover_layout},
    state_mount::{
        collect_and_open_state_mount, format_uuid, read_btrfs_fs_info, read_btrfs_subvolume_info,
    },
};
use rustix::fs::{Mode, OFlags, ResolveFlags, fstat, fsync, mkdirat, openat2};
use ryoku_snapper::{
    AdditionalSnapshot, CatalogEntry, CatalogIndex, CatalogStore, CatalogStoreError, ConfigName,
    GenerationId, LayoutCapability, PartnerState, RetentionPartnerObservation, SnapperBackend,
    SnapperCliBackend, SnapperRetentionBackend, SnapperRollbackBackend, SnapshotId, TransactionId,
};
use std::fs::File;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const DIRECTORY: &str = "snapper-catalog";

pub struct ProtectedSnapperCatalog {
    root: File,
}
#[derive(Clone, Debug)]
pub struct CatalogStatus {
    pub entry: CatalogEntry,
    pub observation: RetentionPartnerObservation,
    pub complete: bool,
    pub cleanup_owned: bool,
}
fn transaction(value: &Identifier) -> Result<TransactionId> {
    TransactionId::new(value.as_str()).map_err(|_| "invalid transaction".into())
}
fn configuration() -> RelativePath {
    RelativePath::new("etc").expect("fixed path")
}
fn protected_directory(root: &File, create: bool) -> Result<File> {
    let machine = read_machine_state_record(root)?;
    let (state, _) = collect_and_open_state_mount(root, machine.identity())?;
    if create {
        mkdirat(&state, DIRECTORY, Mode::from_raw_mode(0o700))?;
        fsync(&state)?;
    }
    let directory = File::from(openat2(
        &state,
        DIRECTORY,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    let stat = fstat(&directory)?;
    if stat.st_uid != 0 || stat.st_gid != 0 || stat.st_mode & 0o7777 != 0o700 {
        return Err("unsafe catalog directory".into());
    }
    Ok(directory)
}
impl ProtectedSnapperCatalog {
    /// Explicit provisioning only: never adopt or overwrite an existing name.
    pub fn initialize(root: &File) -> Result<Self> {
        let directory = protected_directory(root, true)?;
        let store = CatalogStore::open(&directory, 0)?;
        store.replace(&CatalogIndex::default())?;
        Ok(Self { root: root.try_clone()? })
    }
    pub fn open(root: &File) -> Result<Self> {
        let directory = protected_directory(root, false)?;
        let store = CatalogStore::open_existing(&directory, 0)?;
        store.load()?;
        Ok(Self { root: root.try_clone()? })
    }
    /// Publish only an existing sealed linked generation and fresh additional
    /// snapshots from exactly its transaction. Never creates a redundant pair.
    /// All retained partners are removed from automatic Snapper cleanup before
    /// the atomic catalog publication. Interruption may over-retain, never
    /// publish an unprotected partner. The coordinator owns later pruning;
    /// this implementation deliberately provides no automatic delete actor.
    fn store(&self) -> Result<CatalogStore> {
        let directory = protected_directory(&self.root, false)?;
        Ok(CatalogStore::open_existing(&directory, 0)?)
    }
    pub fn publish(&self, id: &Identifier, additional: &[(ConfigName, SnapshotId)]) -> Result<()> {
        let tx = transaction(id)?;
        let layout = discover_layout(&self.root, &configuration())?;
        let reader = GenerationReader::open_state_mount()?;
        let linked = reader.open_historical_transaction(&self.root, id)?;
        let manifest = linked.generation().manifest();
        let mut extras = Vec::new();
        for (config, snapshot) in additional {
            let target = layout
                .targets
                .iter()
                .find(|t| t.config == *config && t.config.as_str() != "root")
                .ok_or("additional config unavailable")?;
            extras.push(observe_additional(&self.root, target, snapshot, &tx, true)?);
        }
        let entry = CatalogEntry {
            transaction_id: tx.clone(),
            root_config: ConfigName::new("root").map_err(|_| "invalid config")?,
            root_snapshot: SnapshotId::new(&manifest.snapper_snapshot_id)
                .map_err(|_| "invalid snapshot")?,
            generation_id: GenerationId::new(manifest.generation_id.as_str())
                .map_err(|_| "invalid generation")?,
            additional_snapshots: extras,
            capability: layout.assessment.capability,
            partner: PartnerState::Verified,
            manual_checkpoint: false,
            known_working: false,
        };
        self.store()?.update(|index| {
            if index.get(&tx).is_some() {
                return Err(CatalogStoreError::DuplicateTransaction);
            }
            retain(&entry).map_err(|_| CatalogStoreError::InvalidCatalog)?;
            index.insert(entry).map_err(|_| CatalogStoreError::DuplicateTransaction)
        })?;
        Ok(())
    }
    pub fn status(&self, id: &Identifier) -> Result<CatalogStatus> {
        // Revalidate placement on every operation, not just construction.
        protected_directory(&self.root, false)?;
        let index = self.store()?.load()?;
        let entry = index.get(&transaction(id)?).ok_or("catalog transaction missing")?.clone();
        status(&self.root, &entry)
    }
    pub fn mark_manual(&self, id: &Identifier) -> Result<()> {
        self.pin(id, false, None)
    }
    /// Promote only after the exact source has actually booted as a writable
    /// rollback target and all recovery attempts are complete/absent. This
    /// proves boot/storage health, not application health or ESP protection.
    pub fn mark_known_working_after_boot(
        &self,
        id: &Identifier,
        target: &SnapshotId,
        rollback_transaction: &TransactionId,
    ) -> Result<()> {
        self.pin(id, true, Some((target, rollback_transaction)))
    }
    fn pin(
        &self,
        id: &Identifier,
        known: bool,
        boot: Option<(&SnapshotId, &TransactionId)>,
    ) -> Result<()> {
        protected_directory(&self.root, false)?;
        let tx = transaction(id)?;
        self.store()?.update(|index| {
            let entry = index.get(&tx).ok_or(CatalogStoreError::InvalidCatalog)?.clone();
            let current =
                status(&self.root, &entry).map_err(|_| CatalogStoreError::InvalidCatalog)?;
            if !current.cleanup_owned {
                return Err(CatalogStoreError::InvalidCatalog);
            }
            if known {
                let (target, rollback_tx) = boot.ok_or(CatalogStoreError::InvalidCatalog)?;
                verify_booted_source(&self.root, id, target, rollback_tx)
                    .map_err(|_| CatalogStoreError::InvalidCatalog)?;
                if !current.complete || !index.mark_known_working(&current.observation) {
                    return Err(CatalogStoreError::InvalidCatalog);
                }
            } else if !index.mark_manual_checkpoint(&current.observation) {
                return Err(CatalogStoreError::InvalidCatalog);
            }
            Ok(())
        })?;
        Ok(())
    }
}
fn verify_booted_source(
    root: &File,
    id: &Identifier,
    target: &SnapshotId,
    tx: &TransactionId,
) -> Result<()> {
    if !RestoreJournalStore::open_existing_state_mount()?.incomplete_attempts()?.is_empty() {
        return Err("unfinished recovery".into());
    }
    let reader = GenerationReader::open_state_mount()?;
    let linked = reader.open_historical_transaction(root, id)?;
    let snapshot = read_btrfs_subvolume_info(linked.snapshot().descriptor())?;
    let running = read_btrfs_subvolume_info(root)?;
    let target_path = format!(".snapshots/{}/snapshot", target.as_str());
    let descriptor = openat2(
        root,
        target_path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?;
    let observed = read_btrfs_subvolume_info(&descriptor)?;
    if read_btrfs_fs_info(root)?.fsid != read_btrfs_fs_info(&descriptor)?.fsid
        || read_btrfs_fs_info(root)?.fsid
            != read_btrfs_fs_info(linked.snapshot().descriptor())?.fsid
        || running.treeid != observed.treeid
        || running.uuid != observed.uuid
        || running.parent_uuid != snapshot.uuid
        || running.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) != 0
    {
        return Err("running root is not the exact source rollback".into());
    }
    SnapperRollbackBackend.verify_running_target(
        &ConfigName::new("root").map_err(|_| "invalid config")?,
        target,
        tx,
    )?;
    Ok(())
}
fn retain(entry: &CatalogEntry) -> Result<()> {
    SnapperRetentionBackend.retain(
        &entry.root_config,
        &entry.root_snapshot,
        &entry.transaction_id,
    )?;
    for extra in &entry.additional_snapshots {
        SnapperRetentionBackend.retain(
            &ConfigName::new(&extra.config).map_err(|_| "invalid config")?,
            &SnapshotId::new(&extra.snapshot).map_err(|_| "invalid snapshot")?,
            &entry.transaction_id,
        )?;
    }
    Ok(())
}
fn status(root: &File, entry: &CatalogEntry) -> Result<CatalogStatus> {
    let id = Identifier::new(entry.transaction_id.as_str())
        .map_err(|_| "invalid catalog transaction")?;
    let layout = discover_layout(root, &configuration())?;
    let reader = GenerationReader::open_state_mount()?;
    let linked = reader.open_historical_transaction(root, &id);
    let root_verified = linked.as_ref().is_ok_and(|l| {
        let m = l.generation().manifest();
        m.generation_id.as_str() == entry.generation_id.as_str()
            && m.snapper_config.as_str() == entry.root_config.as_str()
            && m.snapper_snapshot_id == entry.root_snapshot.as_str()
            && verify_historical_root_checkpoint(root, l.snapshot().receipt()).is_ok()
    });
    let mut cleanup_owned = SnapperRetentionBackend
        .verify_retained(&entry.root_config, &entry.root_snapshot, &entry.transaction_id)
        .is_ok();
    let mut extras = Vec::new();
    for expected in &entry.additional_snapshots {
        let result = verify_additional(root, &layout, expected, &entry.transaction_id);
        if let Ok(pair) = result {
            extras.push(pair);
        } else {
            cleanup_owned = false;
        }
        cleanup_owned &= ConfigName::new(&expected.config)
            .ok()
            .zip(SnapshotId::new(&expected.snapshot).ok())
            .is_some_and(|(c, n)| {
                SnapperRetentionBackend.verify_retained(&c, &n, &entry.transaction_id).is_ok()
            });
    }
    let observation = RetentionPartnerObservation {
        transaction_id: entry.transaction_id.clone(),
        generation_present_and_verified: root_verified,
        snapper_partner_present_and_verified: root_verified,
        verified_additional: extras,
    };
    let complete = cleanup_owned
        && layout.assessment.capability == LayoutCapability::FullSystem
        && entry.is_complete_set(&observation);
    Ok(CatalogStatus { entry: entry.clone(), observation, complete, cleanup_owned })
}
fn verify_additional(
    root: &File,
    layout: &DiscoveredLayout,
    expected: &AdditionalSnapshot,
    tx: &TransactionId,
) -> Result<(ConfigName, SnapshotId)> {
    let config = ConfigName::new(&expected.config).map_err(|_| "invalid config")?;
    let snapshot = SnapshotId::new(&expected.snapshot).map_err(|_| "invalid snapshot")?;
    let target = layout
        .targets
        .iter()
        .find(|t| t.config == config && t.path == expected.path)
        .ok_or("config mapping changed")?;
    let observed = observe_additional(root, target, &snapshot, tx, false)?;
    if observed != *expected {
        return Err("additional snapshot identity changed".into());
    }
    Ok((config, snapshot))
}
fn observe_additional(
    root: &File,
    target: &DiscoveredTarget,
    snapshot: &SnapshotId,
    tx: &TransactionId,
    fresh: bool,
) -> Result<AdditionalSnapshot> {
    if target.path == "/var/lib/ryoku-state"
        || target.path.starts_with("/var/lib/ryoku-state/")
        || target.path.split('/').any(|part| part == ".snapshots")
    {
        return Err("protected internal target cannot be a configuration partner".into());
    }
    let matches = SnapperCliBackend
        .list(&target.config)?
        .into_iter()
        .filter(|r| r.id == *snapshot && r.transaction_id.as_ref() == Some(tx))
        .count();
    if matches != 1 {
        return Err("additional snapshot tag unavailable".into());
    }
    let number = snapshot.as_str().parse::<u64>()?;
    if number == 0 || number.to_string() != snapshot.as_str() {
        return Err("invalid snapshot number".into());
    }
    let relative = format!("{}/.snapshots/{number}/snapshot", target.path.trim_start_matches('/'));
    let fd = openat2(
        root,
        relative,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?;
    let volume = read_btrfs_subvolume_info(&fd)?;
    let fs = read_btrfs_fs_info(&fd)?;
    if fstat(&fd)?.st_ino != 256
        || format_uuid(&fs.fsid) != target.filesystem_uuid
        || (fresh && format_uuid(&volume.parent_uuid) != target.subvolume_uuid)
        || volume.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) == 0
    {
        return Err("additional snapshot is not exact read-only partner".into());
    }
    Ok(AdditionalSnapshot {
        config: target.config.as_str().to_owned(),
        path: target.path.clone(),
        snapshot: snapshot.as_str().to_owned(),
        filesystem_uuid: format_uuid(&fs.fsid),
        subvolume_id: volume.treeid,
        subvolume_uuid: format_uuid(&volume.uuid),
        origin_uuid: format_uuid(&volume.parent_uuid),
    })
}
