//! Typed offline restore-scope planning and a durable, monotonic phase journal.
//!
//! This module does not write restore targets, stop services, or invoke
//! Snapper. It durably records root rollback results and accepts completion only
//! with native evidence that the recorded target is the running root. The
//! caller must pass a trusted, persistent directory descriptor outside every
//! rollback scope to [`RestoreJournalStore`].

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
        FileType, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, flock, fstat,
        fsync, openat, openat2, renameat_with,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};

use crate::{Identifier, RecoveryManifest, Sha256, StateRoot, ValidationError};

const MAX_JOURNAL_BYTES: u64 = 16 * 1024;
const MAX_JOURNAL_DIRECTORY_ENTRIES: usize = 1024;
const MAX_CONFIGURATION_ENTRIES: u32 = 256;
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

/// Source-side facts that can be validated before creating a current-state
/// checkpoint. They never establish that the restore is ready to execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreSourceEvidence {
    pub generation: ArtifactStatus,
    pub archive: ArtifactStatus,
    pub system_snapshot: SystemSnapshotStatus,
    pub system_compatibility: CompatibilityStatus,
    pub configuration_compatibility: CompatibilityStatus,
}

/// Source validation result that cannot be inserted into a restore journal.
/// A verified current-state checkpoint is still required to produce a
/// [`RestorePlan`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreSourcePlan(RestorePlan);

impl RestoreSourcePlan {
    pub fn matches(&self, plan: &RestorePlan) -> bool {
        &self.0 == plan
    }
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
    /// Exact immutable source chosen during preflight, retained across resume.
    pub source_manifest_sha256: Sha256,
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
    ConfigurationScopeTooLarge,
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
    if evidence.current_state_checkpoint != ArtifactStatus::Verified {
        return Err(PreflightError::CheckpointUnavailable);
    }
    build_restore_plan(
        manifest,
        scope,
        evidence.generation,
        evidence.archive,
        &evidence.system_snapshot,
        evidence.system_compatibility,
        evidence.configuration_compatibility,
    )
}

/// Validate a restore source before checkpoint capture. This cannot produce a
/// ready-to-execute plan by itself; callers must still verify and retain a
/// current-state checkpoint with [`plan_restore`].
pub fn plan_restore_source(
    manifest: &RecoveryManifest,
    scope: RestoreScope,
    evidence: &RestoreSourceEvidence,
) -> Result<RestoreSourcePlan, PreflightError> {
    build_restore_plan(
        manifest,
        scope,
        evidence.generation,
        evidence.archive,
        &evidence.system_snapshot,
        evidence.system_compatibility,
        evidence.configuration_compatibility,
    )
    .map(RestoreSourcePlan)
}

fn build_restore_plan(
    manifest: &RecoveryManifest,
    scope: RestoreScope,
    generation: ArtifactStatus,
    archive: ArtifactStatus,
    system_snapshot: &SystemSnapshotStatus,
    system_compatibility: CompatibilityStatus,
    configuration_compatibility: CompatibilityStatus,
) -> Result<RestorePlan, PreflightError> {
    if !manifest.is_mutation_ready() || manifest.validate().is_err() {
        return Err(PreflightError::ManifestInvalid);
    }
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked) {
        match generation {
            ArtifactStatus::Verified => {}
            ArtifactStatus::Missing => return Err(PreflightError::GenerationUnavailable),
            ArtifactStatus::Invalid => return Err(PreflightError::GenerationInvalid),
        }
        match archive {
            ArtifactStatus::Verified => {}
            ArtifactStatus::Missing => return Err(PreflightError::ArchiveUnavailable),
            ArtifactStatus::Invalid => return Err(PreflightError::ArchiveInvalid),
        }
    }
    let configuration_entries = manifest
        .files()
        .iter()
        .filter(|record| {
            matches!(
                record.root,
                StateRoot::EtcRyoku
                    | StateRoot::UserRyokuConfig
                    | StateRoot::UserRyokuState
                    | StateRoot::UserRyokuProfile
            )
        })
        .count();
    let configuration_entries =
        u32::try_from(configuration_entries).map_err(|_| PreflightError::EntryCountOverflow)?;
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked)
        && configuration_entries > MAX_CONFIGURATION_ENTRIES
    {
        return Err(PreflightError::ConfigurationScopeTooLarge);
    }
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked)
        && configuration_entries == 0
    {
        return Err(PreflightError::ConfigurationScopeEmpty);
    }
    if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked) {
        match configuration_compatibility {
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
        match system_compatibility {
            CompatibilityStatus::Compatible => {}
            CompatibilityStatus::Incompatible => return Err(PreflightError::SystemIncompatible),
            CompatibilityStatus::Unknown => {
                return Err(PreflightError::SystemCompatibilityUnknown);
            }
        }
        match system_snapshot {
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
        source_manifest_sha256: manifest
            .manifest_sha256
            .clone()
            .ok_or(PreflightError::ManifestInvalid)?,
        transaction_id: manifest.transaction_id.clone(),
        generation_id: manifest.generation_id.clone(),
        snapper_config: manifest.snapper_config.clone(),
        snapshot_id: manifest.snapper_snapshot_id.clone(),
        filesystem_uuid: manifest.filesystem_uuid.clone(),
        scope,
        configuration_entries,
    })
}

/// Durable restore phase shared by configuration-only, system-only, and linked
/// recovery. `ContentStaged` and `ConfigurationRestored` depend on the scope.
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
    fn next(self, scope: RestoreScope) -> Option<Self> {
        match self {
            Self::Planned => Some(Self::CheckpointVerified),
            Self::CheckpointVerified => match scope {
                RestoreScope::SystemOnly => Some(Self::WritersStopped),
                RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked => {
                    Some(Self::ContentStaged)
                }
            },
            Self::ContentStaged => Some(Self::WritersStopped),
            Self::WritersStopped => match scope {
                RestoreScope::ConfigurationOnly => Some(Self::ConfigurationRestored),
                RestoreScope::SystemOnly | RestoreScope::CompleteLinked => {
                    Some(Self::RootRollbackComplete)
                }
            },
            Self::RootRollbackComplete => match scope {
                RestoreScope::CompleteLinked => Some(Self::ConfigurationRestored),
                RestoreScope::SystemOnly => Some(Self::HealthChecked),
                RestoreScope::ConfigurationOnly => None,
            },
            Self::ConfigurationRestored => match scope {
                RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked => {
                    Some(Self::HealthChecked)
                }
                RestoreScope::SystemOnly => None,
            },
            Self::HealthChecked => Some(Self::Complete),
            Self::Complete => None,
        }
    }
}

/// Persisted state. Contains no target contents or secrets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreJournal {
    pub schema_version: u16,
    /// Fresh restore-attempt identity, distinct from the selected source transaction.
    pub restore_id: Identifier,
    pub plan: RestorePlan,
    pub current_state_generation: Identifier,
    pub current_state_manifest_sha256: Sha256,
    /// Snapper's verified backup/target pair for an in-progress system restore.
    /// This stays present across reboot until the complete attempt is retired.
    #[serde(default)]
    pub root_rollback: Option<RootRollbackResult>,
    pub phase: RestorePhase,
}

/// Derive the distinct Snapper tag used for the root rollback created by one
/// restore attempt. The restore ID itself already tags the retained current
/// state checkpoint, so reusing it would make Snapper reject the rollback as a
/// duplicate transaction.
pub fn root_rollback_tag(
    restore_id: &Identifier,
) -> Result<ryoku_snapper::TransactionId, JournalError> {
    ryoku_snapper::TransactionId::new(format!("{}:rollback", restore_id.as_str()))
        .map_err(|_| JournalError::InvalidJournal)
}

/// Native Snapper rollback identities retained in the rollback-independent
/// journal. Values are canonical numeric snapshot IDs, not command text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRollbackResult {
    previous_default_snapshot: String,
    backup_snapshot: String,
    target_snapshot: String,
}

impl RootRollbackResult {
    pub fn from_snapper(
        receipt: &ryoku_snapper::RootRollbackReceipt,
    ) -> Result<Self, JournalError> {
        let result = Self {
            previous_default_snapshot: receipt.previous_default_snapshot.as_str().into(),
            backup_snapshot: receipt.backup_snapshot.as_str().into(),
            target_snapshot: receipt.rollback_target_snapshot.as_str().into(),
        };
        if !valid_root_rollback_result(&result) {
            return Err(JournalError::InvalidJournal);
        }
        Ok(result)
    }

    pub fn target_snapshot(&self) -> &str {
        &self.target_snapshot
    }

    pub fn backup_snapshot(&self) -> &str {
        &self.backup_snapshot
    }
}

impl RestoreJournal {
    /// A known Snapper receipt persisted while writers are stopped means the
    /// native rollback call must not run again; the remaining safe action is
    /// only to hand off to reboot and verify the booted target.
    pub fn has_recorded_root_rollback_handoff(&self, scope: RestoreScope) -> bool {
        self.plan.scope == scope
            && self.phase == RestorePhase::WritersStopped
            && self.root_rollback.is_some()
    }

    pub fn new(
        restore_id: Identifier,
        plan: RestorePlan,
        current_state_generation: Identifier,
        current_state_manifest_sha256: Sha256,
    ) -> Self {
        Self {
            schema_version: 4,
            restore_id,
            plan,
            current_state_generation,
            current_state_manifest_sha256,
            root_rollback: None,
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
    Busy,
    Filesystem(Errno),
    Io(std::io::Error),
    Validation(ValidationError),
    Machine(crate::machine_state::MachineStateError),
    Mount(crate::state_mount::StateMountError),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStore => f.write_str("restore journal directory descriptor is invalid"),
            Self::InvalidJournal => f.write_str("restore journal is invalid or corrupt"),
            Self::AlreadyExists => f.write_str("restore journal already exists"),
            Self::InvalidTransition => f.write_str("restore phase transition is not monotonic"),
            Self::TooLarge => f.write_str("restore journal exceeds its fixed size bound"),
            Self::Busy => f.write_str("restore journal is busy with another operation"),
            Self::Filesystem(error) => write!(f, "restore journal filesystem error: {error}"),
            Self::Io(error) => write!(f, "restore journal I/O error: {error}"),
            Self::Validation(error) => {
                write!(f, "restore journal identifier is invalid: {error:?}")
            }
            Self::Machine(error) => write!(f, "restore journal machine identity: {error}"),
            Self::Mount(error) => write!(f, "restore journal state mount: {error}"),
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
    /// Open the fixed journal directory using independent machine identity and
    /// live mount verification. Missing storage is an error, never an empty scan.
    pub fn open_existing_state_mount() -> Result<Self, JournalError> {
        let root = File::open("/")?;
        let record = crate::machine_state::read_machine_state_record(&root)
            .map_err(JournalError::Machine)?;
        let (state, _) = crate::state_mount::collect_and_open_state_mount(&root, record.identity())
            .map_err(JournalError::Mount)?;
        Self::state_directory(&File::from(state), false)
    }

    /// Read the protected recovery journal in a sandbox where its verified
    /// dedicated mount is made read-only. Mutating preparation uses the regular
    /// writable-state entry point above.
    pub fn open_existing_state_mount_readonly() -> Result<Self, JournalError> {
        let root = File::open("/")?;
        let record = crate::machine_state::read_machine_state_record(&root)
            .map_err(JournalError::Machine)?;
        let (state, _) =
            crate::state_mount::collect_and_open_state_mount_readonly(&root, record.identity())
                .map_err(JournalError::Mount)?;
        Self::open_existing_state_directory_readonly(&File::from(state))
    }

    /// Open existing journal storage through an already validated, pinned
    /// state-mount descriptor. Callers that coordinate with the mutation lock
    /// must use the same descriptor for both operations so a mount change
    /// cannot split the lock and journal scan across different mount instances.
    pub fn open_existing_state_directory_readonly(state: &File) -> Result<Self, JournalError> {
        Self::state_directory(state, false)
    }

    /// Explicit provisioning only: create the private journal directory and
    /// lock on a mount matching the caller's trusted creation identity.
    pub fn initialize_state_mount(
        expected: &crate::state_mount::StateMountIdentity,
    ) -> Result<Self, JournalError> {
        let root = File::open("/")?;
        let (state, _) = crate::state_mount::collect_and_open_state_mount(&root, expected)
            .map_err(JournalError::Mount)?;
        Self::state_directory(&File::from(state), true)
    }

    /// Provision the common mutation lock after verifying the fixed protected
    /// state mount. This is an explicit administrator initialization action;
    /// the session gate itself only opens the existing inode read-only.
    pub fn ensure_mutation_lock_state_mount(
        expected: &crate::state_mount::StateMountIdentity,
    ) -> Result<(), JournalError> {
        let root = File::open("/")?;
        let (state, _) = crate::state_mount::collect_and_open_state_mount(&root, expected)
            .map_err(JournalError::Mount)?;
        let state = File::from(state);
        ensure_mutation_lock(&state)?;
        fsync(&state)?;
        Ok(())
    }

    fn state_directory(state: &File, create: bool) -> Result<Self, JournalError> {
        if create {
            ensure_mutation_lock(state)?;
            match rustix::fs::mkdirat(state, "restore-journals", Mode::from_raw_mode(0o700)) {
                Ok(()) => (),
                Err(Errno::EXIST) => (),
                Err(error) => return Err(error.into()),
            }
        }
        let directory = File::from(openat2(
            state,
            "restore-journals",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        let stat = rustix::fs::fstat(&directory)?;
        if stat.st_uid != 0 || stat.st_gid != 0 {
            return Err(JournalError::InvalidStore);
        }
        let store = Self::open_directory(&directory, create)?;
        if create {
            fsync(state)?;
        }
        Ok(store)
    }

    /// Takes an already-open directory descriptor on persistent storage outside
    /// rollback scope. Descriptor authorization and mount validation belong to
    /// the privileged caller and are intentionally not inferred from a path.
    pub fn open(trusted_root: &File) -> Result<Self, JournalError> {
        Self::open_directory(trusted_root, true)
    }

    /// Open existing journal storage without creating a directory or lock file.
    pub fn open_existing(trusted_root: &File) -> Result<Self, JournalError> {
        Self::open_directory(trusted_root, false)
    }

    fn open_directory(trusted_root: &File, create: bool) -> Result<Self, JournalError> {
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
            if create { OFlags::RDWR | OFlags::CREATE } else { OFlags::RDONLY }
                | OFlags::CLOEXEC
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::from_raw_mode(0o600),
        )?);
        let lock_stat = rustix::fs::fstat(&lock_file)?;
        if rustix::fs::FileType::from_raw_mode(lock_stat.st_mode)
            != rustix::fs::FileType::RegularFile
            || lock_stat.st_mode & 0o7777 != 0o600
            || lock_stat.st_uid != stat.st_uid
            || lock_stat.st_gid != stat.st_gid
            || lock_stat.st_nlink != 1
        {
            return Err(JournalError::InvalidStore);
        }
        if create {
            fsync(&lock_file)?;
            fsync(&root)?;
        }
        Ok(Self { root, lock_file, process_lock: Mutex::new(()) })
    }

    /// Creates the initial durable phase without replacing existing state.
    pub fn create(&self, journal: &RestoreJournal) -> Result<(), JournalError> {
        validate_journal(journal)?;
        if journal.phase != RestorePhase::Planned {
            return Err(JournalError::InvalidTransition);
        }
        self.with_lock(|| {
            let name = journal_name(&journal.restore_id)?;
            match read_journal(&self.root, &name) {
                Ok(_) => return Err(JournalError::AlreadyExists),
                Err(JournalError::Filesystem(Errno::NOENT)) => {}
                Err(error) => return Err(error),
            }
            self.write_atomic(&name, journal)
        })
    }

    /// Reads and validates a journal, suitable for process-restart recovery.
    pub fn load(&self, restore_id: &Identifier) -> Result<RestoreJournal, JournalError> {
        self.with_lock(|| read_journal(&self.root, &journal_name(restore_id)?))
    }

    /// Discover unfinished attempts after process restart, without knowing IDs
    /// from a previous client. The scan and reads share the journal writer lock.
    /// Any unexpected entry (including an interrupted temporary publication),
    /// corrupt record or excessive directory size refuses an all-clear result.
    /// A concurrent journal operation returns [`JournalError::Busy`] instead
    /// of making a session-start check wait behind the writer.
    /// This is journal evidence only; callers must still validate the persistent
    /// mount and enforce their session/recovery policy under the operation lock.
    pub fn incomplete_attempts(&self) -> Result<Vec<RestoreJournal>, JournalError> {
        self.with_nonblocking_lock(|| self.scan_incomplete_attempts())
    }

    /// Scan without taking the journal's exclusive flock. This is reserved for
    /// a caller that already holds the common Ryoku mutation lock in shared or
    /// exclusive mode and has opened the state mount read-only. Every writer
    /// must participate in that common lock before changing journal state.
    pub fn incomplete_attempts_under_mutation_lock(
        &self,
    ) -> Result<Vec<RestoreJournal>, JournalError> {
        self.scan_incomplete_attempts()
    }

    /// Persist the independently inventoried result of a Snapper classic-root
    /// rollback attempt. This keeps the backup and target identities outside
    /// root rollback scope before the machine is rebooted. The attempt remains
    /// in `WritersStopped`, so the session gate continues to refuse Ryoku.
    pub fn record_root_rollback_result(
        &self,
        journal: &mut RestoreJournal,
        receipt: &ryoku_snapper::RootRollbackReceipt,
    ) -> Result<(), JournalError> {
        if journal.phase != RestorePhase::WritersStopped
            || !matches!(
                journal.plan.scope,
                RestoreScope::SystemOnly | RestoreScope::CompleteLinked
            )
            || journal.plan.snapper_config.as_str() != "root"
            || receipt.config.as_str() != journal.plan.snapper_config.as_str()
            || receipt.source_snapshot.as_str() != journal.plan.snapshot_id
            || receipt.transaction_id != root_rollback_tag(&journal.restore_id)?
        {
            return Err(JournalError::InvalidTransition);
        }
        let result = RootRollbackResult::from_snapper(receipt)?;
        if result.previous_default_snapshot == result.target_snapshot
            || result.backup_snapshot == result.target_snapshot
            || result.backup_snapshot == result.previous_default_snapshot
            || result.target_snapshot == journal.plan.snapshot_id
        {
            return Err(JournalError::InvalidJournal);
        }

        self.with_lock(|| {
            let name = journal_name(&journal.restore_id)?;
            if read_journal(&self.root, &name)? != *journal {
                return Err(JournalError::InvalidTransition);
            }
            if journal.root_rollback.as_ref().is_some_and(|existing| existing != &result) {
                return Err(JournalError::InvalidTransition);
            }
            let mut updated = journal.clone();
            updated.schema_version = 4;
            updated.root_rollback = Some(result);
            validate_journal(&updated)?;
            self.write_atomic(&name, &updated)?;
            *journal = updated;
            Ok(())
        })
    }

    /// Advance only after native Btrfs identity confirms that the exact
    /// journaled target is now the running writable root. A Snapper success
    /// receipt or caller-written phase is insufficient.
    pub fn confirm_root_rollback_booted(
        &self,
        journal: &mut RestoreJournal,
        evidence: &crate::native_checkpoint::VerifiedRunningRollbackTarget,
    ) -> Result<(), JournalError> {
        let Some(result) = &journal.root_rollback else {
            return Err(JournalError::InvalidTransition);
        };
        if journal.phase != RestorePhase::WritersStopped
            || !matches!(
                journal.plan.scope,
                RestoreScope::SystemOnly | RestoreScope::CompleteLinked
            )
            || evidence.restore_id() != &journal.restore_id
            || evidence.target_snapshot() != result.target_snapshot()
            || evidence.filesystem_uuid() != journal.plan.filesystem_uuid
        {
            return Err(JournalError::InvalidTransition);
        }
        self.with_lock(|| {
            let name = journal_name(&journal.restore_id)?;
            if read_journal(&self.root, &name)? != *journal {
                return Err(JournalError::InvalidTransition);
            }
            let mut updated = journal.clone();
            updated.schema_version = 4;
            updated.phase = RestorePhase::RootRollbackComplete;
            validate_journal(&updated)?;
            self.write_atomic(&name, &updated)?;
            *journal = updated;
            Ok(())
        })
    }

    fn scan_incomplete_attempts(&self) -> Result<Vec<RestoreJournal>, JournalError> {
        let mut entries = rustix::fs::Dir::read_from(&self.root)?;
        let mut count = 0;
        let mut pending = Vec::new();
        for entry in &mut entries {
            let entry = entry?;
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b".." | b".restore-journal.lock") {
                continue;
            }
            count += 1;
            if count > MAX_JOURNAL_DIRECTORY_ENTRIES {
                return Err(JournalError::TooLarge);
            }
            let name = name.to_str().map_err(|_| JournalError::InvalidJournal)?;
            let id = name.strip_suffix(".json").ok_or(JournalError::InvalidJournal)?;
            Identifier::new(id).map_err(|_| JournalError::InvalidJournal)?;
            let journal = read_journal(&self.root, name)?;
            if journal.phase != RestorePhase::Complete {
                pending.push(journal);
            }
        }
        pending.sort_by(|left, right| left.restore_id.cmp(&right.restore_id));
        Ok(pending)
    }

    /// Persists exactly one next phase before updating the caller's value.
    pub fn advance(
        &self,
        journal: &mut RestoreJournal,
        next: RestorePhase,
    ) -> Result<(), JournalError> {
        self.with_lock(|| {
            if next == RestorePhase::RootRollbackComplete
                || journal.phase.next(journal.plan.scope) != Some(next)
            {
                return Err(JournalError::InvalidTransition);
            }
            let name = journal_name(&journal.restore_id)?;
            if read_journal(&self.root, &name)? != *journal {
                return Err(JournalError::InvalidTransition);
            }
            let mut updated = journal.clone();
            updated.schema_version = 4;
            updated.phase = next;
            validate_journal(&updated)?;
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

    fn with_nonblocking_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, JournalError>,
    ) -> Result<T, JournalError> {
        let _process_guard = self.process_lock.try_lock().map_err(|_| JournalError::Busy)?;
        flock(&self.lock_file, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            if error == Errno::AGAIN || error == Errno::WOULDBLOCK {
                JournalError::Busy
            } else {
                JournalError::Filesystem(error)
            }
        })?;
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

fn ensure_mutation_lock(state: &File) -> Result<(), JournalError> {
    let state_stat = fstat(state)?;
    if FileType::from_raw_mode(state_stat.st_mode) != FileType::Directory
        || state_stat.st_uid != 0
        || state_stat.st_gid != 0
        || state_stat.st_mode & 0o7777 != 0o700
    {
        return Err(JournalError::InvalidStore);
    }
    let (lock, created) = match openat(
        state,
        crate::state_mount::MUTATION_LOCK_NAME,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    ) {
        Ok(lock) => (File::from(lock), true),
        Err(Errno::EXIST) => (
            File::from(openat(
                state,
                crate::state_mount::MUTATION_LOCK_NAME,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )?),
            false,
        ),
        Err(error) => return Err(error.into()),
    };
    if created {
        fchmod(&lock, Mode::from_raw_mode(0o600))?;
    }
    let lock_stat = fstat(&lock)?;
    if FileType::from_raw_mode(lock_stat.st_mode) != FileType::RegularFile
        || lock_stat.st_uid != state_stat.st_uid
        || lock_stat.st_gid != state_stat.st_gid
        || lock_stat.st_mode & 0o7777 != 0o600
        || lock_stat.st_nlink != 1
    {
        return Err(JournalError::InvalidStore);
    }
    if created {
        fsync(&lock)?;
        fsync(state)?;
    }
    Ok(())
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
    let config_scope_has_valid_count =
        !matches!(plan.scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked)
            || (1..=MAX_CONFIGURATION_ENTRIES).contains(&plan.configuration_entries);
    linked_snapshot_is_valid && config_scope_has_valid_count
}

fn valid_snapshot_number(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|number| number > 0 && number.to_string() == value)
}

fn valid_root_rollback_result(result: &RootRollbackResult) -> bool {
    valid_snapshot_number(&result.previous_default_snapshot)
        && valid_snapshot_number(&result.backup_snapshot)
        && valid_snapshot_number(&result.target_snapshot)
        && result.previous_default_snapshot != result.backup_snapshot
        && result.previous_default_snapshot != result.target_snapshot
        && result.backup_snapshot != result.target_snapshot
}

fn validate_journal(journal: &RestoreJournal) -> Result<(), JournalError> {
    let root_rollback_phase = matches!(
        journal.phase,
        RestorePhase::RootRollbackComplete
            | RestorePhase::ConfigurationRestored
            | RestorePhase::HealthChecked
            | RestorePhase::Complete
    ) && journal.plan.scope != RestoreScope::ConfigurationOnly;
    if !matches!(journal.schema_version, 3 | 4)
        || journal.restore_id == journal.plan.transaction_id
        || !validate_plan(&journal.plan)
        || !phase_matches_scope(journal.phase, journal.plan.scope)
        || (journal.schema_version == 3 && journal.root_rollback.is_some())
        || (journal.schema_version == 3 && root_rollback_phase)
        || (journal.root_rollback.is_some()
            && (journal.schema_version != 4
                || journal.plan.snapper_config.as_str() != "root"
                || !matches!(
                    journal.plan.scope,
                    RestoreScope::SystemOnly | RestoreScope::CompleteLinked
                )
                || !matches!(
                    journal.phase,
                    RestorePhase::WritersStopped
                        | RestorePhase::RootRollbackComplete
                        | RestorePhase::ConfigurationRestored
                        | RestorePhase::HealthChecked
                        | RestorePhase::Complete
                )
                || !valid_root_rollback_result(journal.root_rollback.as_ref().unwrap())
                || journal.root_rollback.as_ref().unwrap().target_snapshot
                    == journal.plan.snapshot_id))
        || (root_rollback_phase && journal.root_rollback.is_none())
    {
        return Err(JournalError::InvalidJournal);
    }
    Ok(())
}

fn phase_matches_scope(phase: RestorePhase, scope: RestoreScope) -> bool {
    match scope {
        RestoreScope::ConfigurationOnly => matches!(
            phase,
            RestorePhase::Planned
                | RestorePhase::CheckpointVerified
                | RestorePhase::ContentStaged
                | RestorePhase::WritersStopped
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        ),
        RestoreScope::SystemOnly => matches!(
            phase,
            RestorePhase::Planned
                | RestorePhase::CheckpointVerified
                | RestorePhase::WritersStopped
                | RestorePhase::RootRollbackComplete
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        ),
        RestoreScope::CompleteLinked => true,
    }
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
    let directory_stat = rustix::fs::fstat(root)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_uid != directory_stat.st_uid
        || stat.st_gid != directory_stat.st_gid
        || stat.st_nlink != 1
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
    validate_journal(&journal)?;
    if journal_name(&journal.restore_id)? != name {
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
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    fn root() -> (std::path::PathBuf, File) {
        let path = std::env::temp_dir().join(format!(
            "ryoku-restore-journal-{}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
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
            Identifier::new("restore-attempt-1").unwrap(),
            plan,
            Identifier::new("gen-current-1").unwrap(),
            Sha256::new("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap(),
        )
    }

    #[test]
    fn root_rollback_uses_a_distinct_snapper_tag_from_its_checkpoint() {
        let restore_id = Identifier::new("restore-attempt-1").unwrap();
        let checkpoint_tag = ryoku_snapper::TransactionId::new(restore_id.as_str()).unwrap();
        let rollback_tag = root_rollback_tag(&restore_id).unwrap();
        assert_ne!(checkpoint_tag, rollback_tag);
        assert_eq!(rollback_tag.as_str(), "restore-attempt-1:rollback");
    }

    fn confirm_test_root_rollback(store: &RestoreJournalStore, attempt: &mut RestoreJournal) {
        let receipt = ryoku_snapper::RootRollbackReceipt {
            config: ryoku_snapper::ConfigName::new("root").unwrap(),
            source_snapshot: ryoku_snapper::SnapshotId::new(attempt.plan.snapshot_id.clone())
                .unwrap(),
            previous_default_snapshot: ryoku_snapper::SnapshotId::new("48").unwrap(),
            backup_snapshot: ryoku_snapper::SnapshotId::new("49").unwrap(),
            rollback_target_snapshot: ryoku_snapper::SnapshotId::new("50").unwrap(),
            transaction_id: root_rollback_tag(&attempt.restore_id).unwrap(),
        };
        store.record_root_rollback_result(attempt, &receipt).unwrap();
        let evidence = crate::native_checkpoint::test_running_rollback_target(
            attempt.restore_id.clone(),
            "50".into(),
            attempt.plan.filesystem_uuid.clone(),
        );
        store.confirm_root_rollback_booted(attempt, &evidence).unwrap();
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
    fn source_plan_cannot_replace_a_verified_current_state_checkpoint() {
        let manifest = manifest();
        let mut facts = evidence();
        facts.current_state_checkpoint = ArtifactStatus::Missing;
        let source = RestoreSourceEvidence {
            generation: facts.generation,
            archive: facts.archive,
            system_snapshot: facts.system_snapshot.clone(),
            system_compatibility: facts.system_compatibility,
            configuration_compatibility: facts.configuration_compatibility,
        };
        let source_plan =
            plan_restore_source(&manifest, RestoreScope::SystemOnly, &source).unwrap();
        assert_eq!(
            plan_restore(&manifest, RestoreScope::SystemOnly, &facts),
            Err(PreflightError::CheckpointUnavailable)
        );
        let ready = plan_restore(&manifest, RestoreScope::SystemOnly, &evidence()).unwrap();
        assert!(source_plan.matches(&ready));
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
    fn aliased_journal_is_not_accepted_as_restart_evidence() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let plan = plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap();
        let state = journal(plan);
        store.create(&state).unwrap();
        let record = path.join(journal_name(&state.restore_id).unwrap());
        let alias = path.join("retained-alias");
        fs::hard_link(&record, &alias).unwrap();
        assert!(matches!(store.load(&state.restore_id), Err(JournalError::InvalidJournal)));
        assert!(store.incomplete_attempts().is_err());
        fs::remove_file(alias).unwrap();
        assert_eq!(store.incomplete_attempts().unwrap(), [state]);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn existing_journal_open_never_creates_a_missing_lock_or_repairs_one() {
        let (path, fd) = root();
        assert!(RestoreJournalStore::open_existing(&fd).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        drop(RestoreJournalStore::open(&fd).unwrap());
        let existing = RestoreJournalStore::open_existing(&fd).unwrap();
        assert!(existing.incomplete_attempts().unwrap().is_empty());
        drop(existing);
        let lock = path.join(".restore-journal.lock");
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(RestoreJournalStore::open_existing(&fd), Err(JournalError::InvalidStore)));
        assert_eq!(fs::metadata(&lock).unwrap().permissions().mode() & 0o7777, 0o644);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn restart_discovery_retains_every_unfinished_phase_and_omits_only_complete() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        assert!(store.incomplete_attempts().unwrap().is_empty());
        let plan = plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap();
        let mut state = journal(plan);
        store.create(&state).unwrap();
        for phase in [
            RestorePhase::Planned,
            RestorePhase::CheckpointVerified,
            RestorePhase::ContentStaged,
            RestorePhase::WritersStopped,
            RestorePhase::RootRollbackComplete,
            RestorePhase::ConfigurationRestored,
            RestorePhase::HealthChecked,
            RestorePhase::Complete,
        ] {
            if state.phase != phase {
                if phase == RestorePhase::RootRollbackComplete {
                    confirm_test_root_rollback(&store, &mut state);
                } else {
                    store.advance(&mut state, phase).unwrap();
                }
            }
            let reopened = RestoreJournalStore::open(&fd).unwrap();
            let expected =
                if phase == RestorePhase::Complete { vec![] } else { vec![state.clone()] };
            assert_eq!(reopened.incomplete_attempts().unwrap(), expected);
        }
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn restart_discovery_is_bounded_and_returns_stable_attempt_order() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let plan = plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap();
        let mut state = journal(plan);
        for id in ["restore-z", "restore-a"] {
            state.restore_id = Identifier::new(id).unwrap();
            store.create(&state).unwrap();
        }
        assert_eq!(
            store
                .incomplete_attempts()
                .unwrap()
                .iter()
                .map(|j| j.restore_id.as_str())
                .collect::<Vec<_>>(),
            ["restore-a", "restore-z"]
        );
        for index in 0..MAX_JOURNAL_DIRECTORY_ENTRIES {
            state.restore_id = Identifier::new(format!("restore-bound-{index}")).unwrap();
            let file = path.join(journal_name(&state.restore_id).unwrap());
            fs::write(&file, serde_json::to_vec(&state).unwrap()).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(matches!(store.incomplete_attempts(), Err(JournalError::TooLarge)));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn restart_discovery_refuses_corruption_unknown_entries_and_external_links() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        for name in [".restore-interrupted.tmp", "foreign-entry", "bad.json"] {
            let entry = path.join(name);
            fs::write(&entry, b"partial").unwrap();
            fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(store.incomplete_attempts().is_err());
            assert_eq!(fs::read(&entry).unwrap(), b"partial");
            fs::remove_file(entry).unwrap();
        }
        std::os::unix::fs::symlink("/etc/passwd", path.join("external.json")).unwrap();
        assert!(store.incomplete_attempts().is_err());
        fs::remove_file(path.join("external.json")).unwrap();
        assert!(store.incomplete_attempts().unwrap().is_empty());
        fs::remove_dir_all(path).unwrap();
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
        let mut recovered = reopened.load(&Identifier::new("restore-attempt-1").unwrap()).unwrap();
        assert_eq!(recovered.phase, RestorePhase::CheckpointVerified);
        reopened.advance(&mut recovered, RestorePhase::ContentStaged).unwrap();
        assert_eq!(
            reopened.load(&Identifier::new("restore-attempt-1").unwrap()).unwrap().phase,
            RestorePhase::ContentStaged
        );
    }

    #[test]
    fn journal_transitions_follow_the_selected_restore_scope() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        for (scope, id, phases) in [
            (
                RestoreScope::ConfigurationOnly,
                "restore-config-only",
                vec![
                    RestorePhase::CheckpointVerified,
                    RestorePhase::ContentStaged,
                    RestorePhase::WritersStopped,
                    RestorePhase::ConfigurationRestored,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
            (
                RestoreScope::SystemOnly,
                "restore-system-only",
                vec![
                    RestorePhase::CheckpointVerified,
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
            (
                RestoreScope::CompleteLinked,
                "restore-complete-linked",
                vec![
                    RestorePhase::CheckpointVerified,
                    RestorePhase::ContentStaged,
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::ConfigurationRestored,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
        ] {
            let plan = plan_restore(&manifest(), scope, &evidence()).unwrap();
            let mut attempt = journal(plan);
            attempt.restore_id = Identifier::new(id).unwrap();
            store.create(&attempt).unwrap();
            for phase in phases {
                if phase == RestorePhase::RootRollbackComplete {
                    confirm_test_root_rollback(&store, &mut attempt);
                } else {
                    store.advance(&mut attempt, phase).unwrap();
                }
                assert_eq!(store.load(&attempt.restore_id).unwrap().phase, phase);
            }
        }
        let mut invalid = journal(
            plan_restore(&manifest(), RestoreScope::ConfigurationOnly, &evidence()).unwrap(),
        );
        invalid.restore_id = Identifier::new("restore-invalid-root-phase").unwrap();
        invalid.phase = RestorePhase::RootRollbackComplete;
        assert!(matches!(store.create(&invalid), Err(JournalError::InvalidJournal)));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn journal_load_rejects_invalid_snapshot_reference() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let plan = plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap();
        store.create(&journal(plan)).unwrap();
        let journal_path = path.join("restore-attempt-1.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        value["plan"]["snapshot_id"] = serde_json::Value::String("../evil".into());
        fs::write(&journal_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            store.load(&Identifier::new("restore-attempt-1").unwrap()),
            Err(JournalError::InvalidJournal)
        ));
    }

    #[test]
    fn journal_store_requires_private_directory() {
        let (_path, fd) = root();
        fs::set_permissions(_path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(RestoreJournalStore::open(&fd), Err(JournalError::InvalidStore)));
    }

    #[test]
    fn create_requires_valid_schema_plan_attempt_and_initial_phase() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let valid =
            journal(plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap());
        let mut invalid = valid.clone();
        invalid.schema_version = 2;
        assert!(matches!(store.create(&invalid), Err(JournalError::InvalidJournal)));
        invalid = valid.clone();
        invalid.restore_id = invalid.plan.transaction_id.clone();
        assert!(matches!(store.create(&invalid), Err(JournalError::InvalidJournal)));
        invalid = valid.clone();
        invalid.plan.configuration_entries = 257;
        assert!(matches!(store.create(&invalid), Err(JournalError::InvalidJournal)));
        for phase in [
            RestorePhase::CheckpointVerified,
            RestorePhase::ContentStaged,
            RestorePhase::WritersStopped,
            RestorePhase::RootRollbackComplete,
            RestorePhase::ConfigurationRestored,
            RestorePhase::HealthChecked,
            RestorePhase::Complete,
        ] {
            invalid = valid.clone();
            invalid.phase = phase;
            assert!(matches!(
                store.create(&invalid),
                Err(JournalError::InvalidTransition | JournalError::InvalidJournal)
            ));
        }
        assert!(!path.join("restore-attempt-1.json").exists());
        store.create(&valid).unwrap();
        assert_eq!(store.load(&valid.restore_id).unwrap(), valid);
    }

    #[test]
    fn preflight_rejects_configuration_counts_the_journal_cannot_persist() {
        let mut source = manifest();
        for index in 1..=MAX_CONFIGURATION_ENTRIES {
            source
                .add_file(
                    FileRecord::new(
                        StateRoot::EtcRyoku,
                        RelativePath::new(format!("extra-{index}")).unwrap(),
                        EntryKind::Absent,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        source.seal().unwrap();
        for scope in [RestoreScope::ConfigurationOnly, RestoreScope::CompleteLinked] {
            assert_eq!(
                plan_restore(&source, scope, &evidence()),
                Err(PreflightError::ConfigurationScopeTooLarge)
            );
        }
        // Root-only recovery does not stage configuration, so neither its
        // content availability nor its staging budget constrains this scope.
        let mut facts = evidence();
        facts.generation = ArtifactStatus::Missing;
        facts.archive = ArtifactStatus::Missing;
        facts.configuration_compatibility = CompatibilityStatus::Unknown;
        let plan = plan_restore(&source, RestoreScope::SystemOnly, &facts).unwrap();
        assert_eq!(plan.configuration_entries, MAX_CONFIGURATION_ENTRIES + 1);
        let state = journal(plan);
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        store.create(&state).unwrap();
        drop(store);
        let reopened = RestoreJournalStore::open(&File::open(&path).unwrap()).unwrap();
        assert_eq!(reopened.load(&state.restore_id).unwrap(), state);
    }

    #[test]
    fn every_phase_reopens_under_attempt_identity_and_rejects_wrong_filename() {
        let (path, fd) = root();
        let store = RestoreJournalStore::open(&fd).unwrap();
        let mut state =
            journal(plan_restore(&manifest(), RestoreScope::CompleteLinked, &evidence()).unwrap());
        store.create(&state).unwrap();
        for phase in [
            RestorePhase::CheckpointVerified,
            RestorePhase::ContentStaged,
            RestorePhase::WritersStopped,
            RestorePhase::RootRollbackComplete,
            RestorePhase::ConfigurationRestored,
            RestorePhase::HealthChecked,
            RestorePhase::Complete,
        ] {
            if phase == RestorePhase::RootRollbackComplete {
                confirm_test_root_rollback(&store, &mut state);
            } else {
                store.advance(&mut state, phase).unwrap();
            }
            let reopened = RestoreJournalStore::open(&File::open(&path).unwrap()).unwrap();
            assert_eq!(reopened.load(&state.restore_id).unwrap(), state);
        }
        assert!(matches!(
            store.advance(&mut state, RestorePhase::Complete),
            Err(JournalError::InvalidTransition)
        ));
        fs::copy(path.join("restore-attempt-1.json"), path.join("wrong-attempt.json")).unwrap();
        assert!(matches!(
            store.load(&Identifier::new("wrong-attempt").unwrap()),
            Err(JournalError::InvalidJournal)
        ));
    }

    #[test]
    fn process_exit_after_every_scope_phase_is_discoverable_and_resumable() {
        const CHILD: &str = "restore::tests::restore_phase_process_exit_child";
        let scenarios = [
            (
                RestoreScope::ConfigurationOnly,
                "configuration-only",
                vec![
                    RestorePhase::CheckpointVerified,
                    RestorePhase::ContentStaged,
                    RestorePhase::WritersStopped,
                    RestorePhase::ConfigurationRestored,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
            (
                RestoreScope::SystemOnly,
                "system-only",
                vec![
                    RestorePhase::CheckpointVerified,
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
            (
                RestoreScope::CompleteLinked,
                "complete-linked",
                vec![
                    RestorePhase::CheckpointVerified,
                    RestorePhase::ContentStaged,
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::ConfigurationRestored,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
        ];

        for (_, scope_name, phases) in scenarios {
            for (boundary, expected_phase) in phases.into_iter().enumerate() {
                let (path, descriptor) = root();
                let store = RestoreJournalStore::open(&descriptor).unwrap();
                let scope = match scope_name {
                    "configuration-only" => RestoreScope::ConfigurationOnly,
                    "system-only" => RestoreScope::SystemOnly,
                    "complete-linked" => RestoreScope::CompleteLinked,
                    _ => unreachable!(),
                };
                let plan = plan_restore(&manifest(), scope, &evidence()).unwrap();
                let state = journal(plan);
                store.create(&state).unwrap();
                drop(store);

                let status = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", CHILD, "--ignored"])
                    .env("RYOKU_RESTORE_PHASE_ROOT", &path)
                    .env("RYOKU_RESTORE_PHASE_SCOPE", scope_name)
                    .env("RYOKU_RESTORE_PHASE_BOUNDARY", boundary.to_string())
                    .status()
                    .unwrap();
                assert_eq!(status.code(), Some(77), "child missed {scope_name} {expected_phase:?}");

                let reopened =
                    RestoreJournalStore::open_existing(&File::open(&path).unwrap()).unwrap();
                let recovered =
                    reopened.load(&Identifier::new("restore-attempt-1").unwrap()).unwrap();
                assert_eq!(recovered.phase, expected_phase, "{scope_name} boundary {boundary}");
                let pending = reopened.incomplete_attempts().unwrap();
                if expected_phase == RestorePhase::Complete {
                    assert!(pending.is_empty());
                } else {
                    assert_eq!(pending.as_slice(), std::slice::from_ref(&recovered));
                }
                let mut resumed = recovered;
                while let Some(next) = resumed.phase.next(scope) {
                    if next == RestorePhase::RootRollbackComplete {
                        confirm_test_root_rollback(&reopened, &mut resumed);
                    } else {
                        reopened.advance(&mut resumed, next).unwrap();
                    }
                }
                assert_eq!(resumed.phase, RestorePhase::Complete);
                assert!(reopened.incomplete_attempts().unwrap().is_empty());
                fs::remove_dir_all(path).unwrap();
            }
        }
    }

    #[test]
    fn process_exit_after_root_rollback_receipt_preserves_the_single_reboot_handoff() {
        const CHILD: &str = "restore::tests::rollback_receipt_process_exit_child";
        for (scope, scope_name) in [
            (RestoreScope::SystemOnly, "system-only"),
            (RestoreScope::CompleteLinked, "complete-linked"),
        ] {
            let (path, descriptor) = root();
            let store = RestoreJournalStore::open(&descriptor).unwrap();
            let plan = plan_restore(&manifest(), scope, &evidence()).unwrap();
            store.create(&journal(plan)).unwrap();
            drop(store);

            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", CHILD, "--ignored"])
                .env("RYOKU_ROLLBACK_RECEIPT_ROOT", &path)
                .env("RYOKU_ROLLBACK_RECEIPT_SCOPE", scope_name)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(77));

            let reopened = RestoreJournalStore::open_existing(&File::open(&path).unwrap()).unwrap();
            let recovered = reopened.load(&Identifier::new("restore-attempt-1").unwrap()).unwrap();
            assert_eq!(recovered.phase, RestorePhase::WritersStopped);
            assert!(recovered.has_recorded_root_rollback_handoff(scope));
            assert_eq!(recovered.root_rollback.as_ref().unwrap().target_snapshot(), "50");
            assert_eq!(reopened.incomplete_attempts().unwrap(), vec![recovered]);
            fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    #[ignore = "spawned by process_exit_after_root_rollback_receipt_preserves_the_single_reboot_handoff"]
    fn rollback_receipt_process_exit_child() {
        let (Ok(root_path), Ok(scope_name)) = (
            std::env::var("RYOKU_ROLLBACK_RECEIPT_ROOT"),
            std::env::var("RYOKU_ROLLBACK_RECEIPT_SCOPE"),
        ) else {
            return;
        };
        let scope = match scope_name.as_str() {
            "system-only" => RestoreScope::SystemOnly,
            "complete-linked" => RestoreScope::CompleteLinked,
            _ => panic!("unknown restore scope"),
        };
        let root = File::open(root_path).unwrap();
        let store = RestoreJournalStore::open(&root).unwrap();
        let mut attempt = store.load(&Identifier::new("restore-attempt-1").unwrap()).unwrap();
        store.advance(&mut attempt, RestorePhase::CheckpointVerified).unwrap();
        if scope == RestoreScope::CompleteLinked {
            store.advance(&mut attempt, RestorePhase::ContentStaged).unwrap();
        }
        store.advance(&mut attempt, RestorePhase::WritersStopped).unwrap();
        let receipt = ryoku_snapper::RootRollbackReceipt {
            config: ryoku_snapper::ConfigName::new("root").unwrap(),
            source_snapshot: ryoku_snapper::SnapshotId::new(attempt.plan.snapshot_id.clone())
                .unwrap(),
            previous_default_snapshot: ryoku_snapper::SnapshotId::new("48").unwrap(),
            backup_snapshot: ryoku_snapper::SnapshotId::new("49").unwrap(),
            rollback_target_snapshot: ryoku_snapper::SnapshotId::new("50").unwrap(),
            transaction_id: root_rollback_tag(&attempt.restore_id).unwrap(),
        };
        store.record_root_rollback_result(&mut attempt, &receipt).unwrap();
        // Simulate an unclean process exit after the receipt sync, before any
        // caller can request reboot or run the Snapper operation again.
        std::process::exit(77);
    }

    #[test]
    #[ignore = "spawned by process_exit_after_every_scope_phase_is_discoverable_and_resumable"]
    fn restore_phase_process_exit_child() {
        let (Ok(root_path), Ok(scope_name), Ok(boundary)) = (
            std::env::var("RYOKU_RESTORE_PHASE_ROOT"),
            std::env::var("RYOKU_RESTORE_PHASE_SCOPE"),
            std::env::var("RYOKU_RESTORE_PHASE_BOUNDARY"),
        ) else {
            return;
        };
        let scope = match scope_name.as_str() {
            "configuration-only" => RestoreScope::ConfigurationOnly,
            "system-only" => RestoreScope::SystemOnly,
            "complete-linked" => RestoreScope::CompleteLinked,
            _ => panic!("unknown restore scope"),
        };
        let boundary = boundary.parse::<usize>().unwrap();
        let root = File::open(root_path).unwrap();
        let store = RestoreJournalStore::open(&root).unwrap();
        let mut state = store.load(&Identifier::new("restore-attempt-1").unwrap()).unwrap();
        let mut completed = 0;
        while let Some(next) = state.phase.next(scope) {
            if next == RestorePhase::RootRollbackComplete {
                confirm_test_root_rollback(&store, &mut state);
            } else {
                store.advance(&mut state, next).unwrap();
            }
            if completed == boundary {
                // Bypass destructors to leave only durable journal state.
                std::process::exit(77);
            }
            completed += 1;
        }
        panic!("restore child advanced past selected boundary {boundary}");
    }

    #[test]
    fn journal_pins_source_digest_and_rejects_unbound_legacy_plans() {
        let (path, fd) = root();
        let source = manifest();
        let plan = plan_restore(&source, RestoreScope::CompleteLinked, &evidence()).unwrap();
        assert_eq!(Some(&plan.source_manifest_sha256), source.manifest_sha256.as_ref());
        let store = RestoreJournalStore::open(&fd).unwrap();
        store.create(&journal(plan.clone())).unwrap();
        assert_eq!(
            store
                .load(&Identifier::new("restore-attempt-1").unwrap())
                .unwrap()
                .plan
                .source_manifest_sha256,
            plan.source_manifest_sha256
        );
        let journal_path = path.join("restore-attempt-1.json");
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        for (schema, remove_digest) in [(1, false), (1, true), (2, false), (2, true), (3, true)] {
            let mut invalid = original.clone();
            invalid["schema_version"] = schema.into();
            if remove_digest {
                invalid["plan"].as_object_mut().unwrap().remove("source_manifest_sha256");
            }
            fs::write(&journal_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(matches!(
                store.load(&Identifier::new("restore-attempt-1").unwrap()),
                Err(JournalError::InvalidJournal)
            ));
        }
    }
}
