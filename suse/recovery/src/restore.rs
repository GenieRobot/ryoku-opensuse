//! Typed offline restore-scope planning and a durable, monotonic phase journal.
//!
//! This module does not write restore targets, stop services, invoke Snapper,
//! or coordinate a root rollback. The caller must pass a trusted, persistent
//! directory descriptor outside every rollback scope to [`RestoreJournalStore`].

use std::{
    fs::File,
    io::{Read, Write},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use rustix::{
    fs::{
        FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, flock, fsync, openat,
        openat2, renameat_with,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};

use crate::{Identifier, RecoveryManifest, Sha256, StateRoot, ValidationError};

const MAX_JOURNAL_BYTES: u64 = 16 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The concrete content selected for restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreScope {
    /// Restore only declared Ryoku configuration from the immutable generation.
    ConfigurationOnly,
    /// Restore only the linked supported system snapshot.
    SystemOnly,
    /// Restore both the linked system snapshot and declared configuration.
    CompleteLinked,
}

/// Availability and compatibility evidence required before scope planning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightEvidence {
    pub generation: ArtifactStatus,
    pub archive: ArtifactStatus,
    pub current_state_checkpoint: ArtifactStatus,
    pub system_snapshot: SystemSnapshotStatus,
    pub system_compatibility: CompatibilityStatus,
    pub configuration_compatibility: CompatibilityStatus,
}

/// Result of comparing a candidate restore partner's package/profile versions
/// against the revisions declared by the verified manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatibilityStatus {
    Compatible,
    Incompatible,
    Unknown,
}

/// Typed verification outcome for an immutable artifact or retained checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactStatus {
    Verified,
    Missing,
    Invalid,
}

/// Whether the snapshot named by the manifest is locally available and matched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SystemSnapshotStatus {
    Available { snapper_config: Identifier, snapshot_id: String, filesystem_uuid: String },
    Missing,
    Incompatible,
}

/// A validated scope and its immutable transaction linkage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestorePlan {
    pub transaction_id: Identifier,
    pub generation_id: Identifier,
    pub snapper_config: Identifier,
    pub snapshot_id: String,
    pub filesystem_uuid: String,
    pub scope: RestoreScope,
    pub configuration_entries: u32,
}

/// Planning errors always occur before a journal is created or targets change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightError {
    ManifestInvalid,
    GenerationUnavailable,
    GenerationInvalid,
    ArchiveUnavailable,
    ArchiveInvalid,
    CheckpointUnavailable,
    SnapshotUnavailable,
    SnapshotIncompatible,
    SystemCompatibilityUnknown,
    SystemIncompatible,
    ConfigurationCompatibilityUnknown,
    ConfigurationIncompatible,
    ConfigurationScopeEmpty,
    EntryCountOverflow,
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "restore preflight failed: {self:?}")
    }
}
impl std::error::Error for PreflightError {}

/// Validates local artifacts and requested scope against immutable manifest linkage.
pub fn plan_restore(
    manifest: &RecoveryManifest,
    scope: RestoreScope,
    evidence: &PreflightEvidence,
) -> Result<RestorePlan, PreflightError> {
    if !manifest.is_mutation_ready() || manifest.validate().is_err() {
        return Err(PreflightError::ManifestInvalid);
    }
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked) {
        match evidence.generation {
            ArtifactStatus::Verified => {}
            ArtifactStatus::Missing => return Err(PreflightError::GenerationUnavailable),
            ArtifactStatus::Invalid => return Err(PreflightError::GenerationInvalid),
        }
        match evidence.archive {
            ArtifactStatus::Verified => {}
            ArtifactStatus::Missing => return Err(PreflightError::ArchiveUnavailable),
            ArtifactStatus::Invalid => return Err(PreflightError::ArchiveInvalid),
        }
    }
    if evidence.current_state_checkpoint != ArtifactStatus::Verified {
        return Err(PreflightError::CheckpointUnavailable);
    }

    let configuration_entries = manifest
        .files()
        .iter()
        .filter(|record| {
            matches!(
                record.root,
                StateRoot::EtcRyoku | StateRoot::UserRyokuConfig | StateRoot::UserRyokuProfile
            )
        })
        .count();
    let configuration_entries =
        u32::try_from(configuration_entries).map_err(|_| PreflightError::EntryCountOverflow)?;
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked)
        && configuration_entries == 0
    {
        return Err(PreflightError::ConfigurationScopeEmpty);
    }
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked) {
        match evidence.configuration_compatibility {
            CompatibilityStatus::Compatible => {}
            CompatibilityStatus::Incompatible => {
                return Err(PreflightError::ConfigurationIncompatible);
            }
            CompatibilityStatus::Unknown => {
                return Err(PreflightError::ConfigurationCompatibilityUnknown);
            }
        }
    }
    if matches!(scope, RestoreScope::SystemOnly | RestoreScope::CompleteLinked) {
        match evidence.system_compatibility {
            CompatibilityStatus::Compatible => {}
            CompatibilityStatus::Incompatible => return Err(PreflightError::SystemIncompatible),
            CompatibilityStatus::Unknown => {
                return Err(PreflightError::SystemCompatibilityUnknown);
            }
        }
        match &evidence.system_snapshot {
            SystemSnapshotStatus::Missing => return Err(PreflightError::SnapshotUnavailable),
            SystemSnapshotStatus::Incompatible => {
                return Err(PreflightError::SnapshotIncompatible);
            }
            SystemSnapshotStatus::Available { snapper_config, snapshot_id, filesystem_uuid }
                if snapper_config.as_str() == manifest.snapper_config.as_str()
                    && snapshot_id == &manifest.snapper_snapshot_id
                    && filesystem_uuid == &manifest.filesystem_uuid => {}
            SystemSnapshotStatus::Available { .. } => {
                return Err(PreflightError::SnapshotIncompatible);
            }
        }
    }

    Ok(RestorePlan {
        transaction_id: manifest.transaction_id.clone(),
        generation_id: manifest.generation_id.clone(),
        snapper_config: manifest.snapper_config.clone(),
        snapshot_id: manifest.snapper_snapshot_id.clone(),
        filesystem_uuid: manifest.filesystem_uuid.clone(),
        scope,
        configuration_entries,
    })
}

/// Durable restore phase. Later phases are defined for recovery continuity, but
/// this crate currently only plans and journals; it does not execute them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestorePhase {
    Planned,
    CheckpointVerified,
    ContentStaged,
    WritersStopped,
    RootRollbackComplete,
    ConfigurationRestored,
    HealthChecked,
    Complete,
}

impl RestorePhase {
    fn next(self) -> Option<Self> {
        Some(match self {
            Self::Planned => Self::CheckpointVerified,
            Self::CheckpointVerified => Self::ContentStaged,
            Self::ContentStaged => Self::WritersStopped,
            Self::WritersStopped => Self::RootRollbackComplete,
            Self::RootRollbackComplete => Self::ConfigurationRestored,
            Self::ConfigurationRestored => Self::HealthChecked,
            Self::HealthChecked => Self::Complete,
            Self::Complete => return None,
        })
    }
}

/// Persisted state. Contains no target contents or secrets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreJournal {
    pub schema_version: u16,
    pub plan: RestorePlan,
    pub current_state_generation: Identifier,
    pub current_state_manifest_sha256: Sha256,
    pub phase: RestorePhase,
}

impl RestoreJournal {
    pub fn new(
        plan: RestorePlan,
        current_state_generation: Identifier,
        current_state_manifest_sha256: Sha256,
    ) -> Self {
        Self {
            schema_version: 1,
            plan,
            current_state_generation,
            current_state_manifest_sha256,
            phase: RestorePhase::Planned,
        }
    }
}

/// Errors from the persistent phase journal.
#[derive(Debug)]
pub enum JournalError {
    InvalidStore,
    InvalidJournal,
    AlreadyExists,
    InvalidTransition,
    TooLarge,
    Filesystem(Errno),
    Io(std::io::Error),
    Validation(ValidationError),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStore => f.write_str("restore journal directory descriptor is invalid"),
            Self::InvalidJournal => f.write_str("restore journal is invalid or corrupt"),
            Self::AlreadyExists => f.write_str("restore journal already exists"),
            Self::InvalidTransition => f.write_str("restore phase transition is not monotonic"),
            Self::TooLarge => f.write_str("restore journal exceeds its fixed size bound"),
            Self::Filesystem(error) => write!(f, "restore journal filesystem error: {error}"),
            Self::Io(error) => write!(f, "restore journal I/O error: {error}"),
            Self::Validation(error) => {
                write!(f, "restore journal identifier is invalid: {error:?}")
            }
        }
    }
}
impl std::error::Error for JournalError {}
impl From<Errno> for JournalError {
    fn from(value: Errno) -> Self {
        Self::Filesystem(value)
    }
}
impl From<std::io::Error> for JournalError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<ValidationError> for JournalError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

/// Atomically persists bounded journals relative to a trusted private directory FD.
pub struct RestoreJournalStore {
    root: File,
    lock_file: File,
    process_lock: Mutex<()>,
}

impl RestoreJournalStore {
    /// Takes an already-open directory descriptor on persistent storage outside
    /// rollback scope. Descriptor authorization and mount validation belong to
    /// the privileged caller and are intentionally not inferred from a path.
    pub fn open(trusted_root: &File) -> Result<Self, JournalError> {
        let stat = rustix::fs::fstat(trusted_root)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
            || stat.st_mode & 0o7777 != 0o700
        {
            return Err(JournalError::InvalidStore);
        }
        let root = File::from(rustix::io::dup(trusted_root)?);
        let lock_file = File::from(openat(
            &root,
            ".restore-journal.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )?);
        let lock_stat = rustix::fs::fstat(&lock_file)?;
        if rustix::fs::FileType::from_raw_mode(lock_stat.st_mode)
            != rustix::fs::FileType::RegularFile
            || lock_stat.st_mode & 0o7777 != 0o600
        {
            return Err(JournalError::InvalidStore);
        }
        Ok(Self { root, lock_file, process_lock: Mutex::new(()) })
    }

    /// Creates the initial durable phase without replacing existing state.
    pub fn create(&self, journal: &RestoreJournal) -> Result<(), JournalError> {
        self.with_lock(|| {
            let name = journal_name(&journal.plan.transaction_id)?;
            match read_journal(&self.root, &name) {
                Ok(_) => return Err(JournalError::AlreadyExists),
                Err(JournalError::Filesystem(Errno::NOENT)) => {}
                Err(error) => return Err(error),
            }
            self.write_atomic(&name, journal)
        })
    }

    /// Reads and validates a journal, suitable for process-restart recovery.
    pub fn load(&self, transaction_id: &Identifier) -> Result<RestoreJournal, JournalError> {
        self.with_lock(|| read_journal(&self.root, &journal_name(transaction_id)?))
    }

    /// Persists exactly one next phase before updating the caller's value.
    pub fn advance(
        &self,
        journal: &mut RestoreJournal,
        next: RestorePhase,
    ) -> Result<(), JournalError> {
        self.with_lock(|| {
            if journal.phase.next() != Some(next) {
                return Err(JournalError::InvalidTransition);
            }
            let name = journal_name(&journal.plan.transaction_id)?;
            if read_journal(&self.root, &name)? != *journal {
                return Err(JournalError::InvalidTransition);
            }
            let mut updated = journal.clone();
            updated.phase = next;
            self.write_atomic(&name, &updated)?;
            *journal = updated;
            Ok(())
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, JournalError>,
    ) -> Result<T, JournalError> {
        let _process_guard = self.process_lock.lock().map_err(|_| JournalError::InvalidStore)?;
        flock(&self.lock_file, FlockOperation::LockExclusive).map_err(JournalError::Filesystem)?;
        let result = operation();
        let unlock =
            flock(&self.lock_file, FlockOperation::Unlock).map_err(JournalError::Filesystem);
        match (result, unlock) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    fn write_atomic(&self, name: &str, journal: &RestoreJournal) -> Result<(), JournalError> {
        let bytes = serde_json::to_vec(journal).map_err(|_| JournalError::InvalidJournal)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(JournalError::TooLarge);
        }
        let (temp_name, mut file) = create_temp(&self.root)?;
        let write_result = (|| -> Result<(), JournalError> {
            file.write_all(&bytes)?;
            fchmod(&file, Mode::from_raw_mode(0o600))?;
            file.sync_all()?;
            renameat_with(&self.root, temp_name.as_str(), &self.root, name, RenameFlags::empty())?;
            fsync(&self.root)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ =
                rustix::fs::unlinkat(&self.root, temp_name.as_str(), rustix::fs::AtFlags::empty());
        }
        write_result
    }
}

fn journal_name(id: &Identifier) -> Result<String, JournalError> {
    Identifier::new(id.as_str())?;
    Ok(format!("{}.json", id.as_str()))
}

fn validate_plan(plan: &RestorePlan) -> bool {
    let linked_snapshot_is_valid = crate::SnapshotLink::new(
        plan.snapper_config.clone(),
        plan.snapshot_id.clone(),
        plan.filesystem_uuid.clone(),
    )
    .is_ok();
    let config_scope_has_entries =
        !matches!(plan.scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked)
            || plan.configuration_entries > 0;
    linked_snapshot_is_valid && config_scope_has_entries && plan.configuration_entries <= 256
}

fn create_temp(root: &File) -> Result<(String, File), JournalError> {
    for _ in 0..32 {
        let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(".restore-{}-{serial}.tmp", std::process::id());
        match openat(
            root,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(fd) => return Ok((name, File::from(fd))),
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(JournalError::InvalidStore)
}

fn read_journal(root: &File, name: &str) -> Result<RestoreJournal, JournalError> {
    let fd = openat2(
        root,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )?;
    let file = File::from(fd);
    let stat = rustix::fs::fstat(&file)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_size < 0
        || stat.st_size as u64 > MAX_JOURNAL_BYTES
    {
        return Err(JournalError::InvalidJournal);
    }
    let mut bytes = Vec::new();
    file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(JournalError::TooLarge);
    }
    let journal: RestoreJournal =
        serde_json::from_slice(&bytes).map_err(|_| JournalError::InvalidJournal)?;
    if journal.schema_version != 1
        || journal_name(&journal.plan.transaction_id)? != name
        || !validate_plan(&journal.plan)
    {
        return Err(JournalError::InvalidJournal);
    }
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EntryKind, EntryMetadata, ExtendedMetadata, FileRecord, OperationReason, RelativePath,
        SnapshotLink, VerificationStatus,
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    fn root() -> (std::path::PathBuf, File) {
        let path = std::env::temp_dir().join(format!(
            "ryoku-restore-journal-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let dir = File::open(&path).unwrap();
        (path, dir)
    }

    fn manifest() -> RecoveryManifest {
        let mut manifest = RecoveryManifest::new(
            Identifier::new("txn-restore-1").unwrap(),
            Identifier::new("gen-restore-1").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "47",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            "2026-10-12T12:00:00Z",
        )
        .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("profile.json").unwrap(),
                    EntryKind::RegularFile {
                        metadata: EntryMetadata {
                            mode: 0o600,
                            uid: 1000,
                            gid: 1000,
                            mtime_unix_ns: 1,
                            acl_sha256: None,
                            extended: ExtendedMetadata::default(),
                        },
                        size: 0,
                        sha256: Sha256::new(
                            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                        )
                        .unwrap(),
                        hard_link_group: None,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        manifest.seal().unwrap();
        manifest
    }

    fn evidence() -> PreflightEvidence {
        PreflightEvidence {
            generation: ArtifactStatus::Verified,
            archive: ArtifactStatus::Verified,
            current_state_checkpoint: ArtifactStatus::Verified,
            system_snapshot: SystemSnapshotStatus::Available {
                snapper_config: Identifier::new("root").unwrap(),
                snapshot_id: "47".into(),
                filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
            },
            system_compatibility: CompatibilityStatus::Compatible,
            configuration_compatibility: CompatibilityStatus::Compatible,
        }
    }

    fn journal(plan: RestorePlan) -> RestoreJournal {
        RestoreJournal::new(
            plan,
            Identifier::new("gen-current-1").unwrap(),
            Sha256::new("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap(),
        )
    }

    #[test]
    fn preflight_rejects_bad_archives_and_unavailable_linked_partner() {
        let manifest = manifest();
        let mut facts = evidence();
        facts.archive = ArtifactStatus::Invalid;
        assert_eq!(
            plan_restore(&manifest, RestoreScope::CompleteLinked, &facts),
            Err(PreflightError::ArchiveInvalid)
        );
        let mut facts = evidence();
        facts.system_snapshot = SystemSnapshotStatus::Missing;
        assert_eq!(
            plan_restore(&manifest, RestoreScope::CompleteLinked, &facts),
            Err(PreflightError::SnapshotUnavailable)
        );
    }

    #[test]
    fn system_only_does_not_require_configuration_generation_or_archive() {
        let manifest = manifest();
        let mut facts = evidence();
        facts.generation = ArtifactStatus::Missing;
        facts.archive = ArtifactStatus::Missing;
        assert!(plan_restore(&manifest, RestoreScope::SystemOnly, &facts).is_ok());
        assert_eq!(
            plan_restore(&manifest, RestoreScope::CompleteLinked, &facts),
            Err(PreflightError::GenerationUnavailable)
        );
        facts.generation = ArtifactStatus::Verified;
        assert_eq!(
            plan_restore(&manifest, RestoreScope::CompleteLinked, &facts),
            Err(PreflightError::ArchiveUnavailable)
        );
    }

    #[test]
    fn preflight_requires_a_sealed_valid_manifest() {
        let mut manifest = manifest();
        manifest.set_verification(
            VerificationStatus::Pending,
            VerificationStatus::Pending,
            VerificationStatus::Pending,
        );
        assert_eq!(
            plan_restore(&manifest, RestoreScope::ConfigurationOnly, &evidence()),
            Err(PreflightError::ManifestInvalid)
        );
    }

    #[test]
    fn preflight_rejects_mismatched_filesystem_before_journaling() {
        let manifest = manifest();
        let mut facts = evidence();
        facts.system_snapshot = SystemSnapshotStatus::Available {
            snapper_config: Identifier::new("root").unwrap(),
            snapshot_id: "47".into(),
            filesystem_uuid: "11111111-1111-1111-1111-111111111111".into(),
        };
        assert_eq!(
            plan_restore(&manifest, RestoreScope::SystemOnly, &facts),
            Err(PreflightError::SnapshotIncompatible)
        );
        assert!(plan_restore(&manifest, RestoreScope::ConfigurationOnly, &facts).is_ok());
    }

    #[test]
    fn preflight_rejects_incompatible_versions_for_the_requested_scope() {
        let manifest = manifest();
        let mut facts = evidence();
        facts.system_compatibility = CompatibilityStatus::Incompatible;
        assert_eq!(
            plan_restore(&manifest, RestoreScope::CompleteLinked, &facts),
            Err(PreflightError::SystemIncompatible)
        );
        let mut facts = evidence();
        facts.configuration_compatibility = CompatibilityStatus::Unknown;
        assert_eq!(
            plan_restore(&manifest, RestoreScope::ConfigurationOnly, &facts),
            Err(PreflightError::ConfigurationCompatibilityUnknown)
        );
    }

    #[test]
    fn journal_survives_restart_and_rejects_skipped_phases() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let plan = plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap();
        let mut state = journal(plan);
        store.create(&state).unwrap();
        assert!(matches!(store.create(&state), Err(JournalError::AlreadyExists)));
        assert!(matches!(
            store.advance(&mut state, RestorePhase::ContentStaged),
            Err(JournalError::InvalidTransition)
        ));
        store.advance(&mut state, RestorePhase::CheckpointVerified).unwrap();
        let mut stale = journal(state.plan.clone());
        assert!(matches!(
            store.advance(&mut stale, RestorePhase::CheckpointVerified),
            Err(JournalError::InvalidTransition)
        ));

        // Simulate process death by reopening only the durable descriptor/state.
        drop(store);
        let reopened = RestoreJournalStore::open(&File::open(&path).unwrap()).unwrap();
        let mut recovered = reopened.load(&Identifier::new("txn-restore-1").unwrap()).unwrap();
        assert_eq!(recovered.phase, RestorePhase::CheckpointVerified);
        reopened.advance(&mut recovered, RestorePhase::ContentStaged).unwrap();
        assert_eq!(
            reopened.load(&Identifier::new("txn-restore-1").unwrap()).unwrap().phase,
            RestorePhase::ContentStaged
        );
    }

    #[test]
    fn journal_load_rejects_invalid_snapshot_reference() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let plan = plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap();
        store.create(&journal(plan)).unwrap();
        let journal_path = path.join("txn-restore-1.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        value["plan"]["snapshot_id"] = serde_json::Value::String("../evil".into());
        fs::write(&journal_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            store.load(&Identifier::new("txn-restore-1").unwrap()),
            Err(JournalError::InvalidJournal)
        ));
    }

    #[test]
    fn journal_store_requires_private_directory() {
        let (_path, fd) = root();
        fs::set_permissions(_path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(RestoreJournalStore::open(&fd), Err(JournalError::InvalidStore)));
    }
}
