//! Serialized, durable intent before the first dedicated-state creation ioctl.
//! This guard is an installer component, not an installed initializer. A prior
//! attempt is always a reconciliation case; unknown volumes are never adopted.
use crate::{
    Identifier, SnapshotLink,
    machine_state::open_record_directory,
    native_checkpoint::{NativeRootCheckpoint, verify_new_root_checkpoint},
    state_provision::{
        CreatedStateSubvolume, create_state_subvolume, reopen_created_from_top_level,
        reopen_created_state_mount,
    },
};
use rustix::fs::{FileType, FlockOperation, Mode, OFlags, flock, fstat, fsync, openat};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    io::{Read, Write},
};

#[derive(Debug)]
pub enum CreationError {
    RootRequired,
    NeedsReconciliation,
    UnsafeLock,
    CheckpointChanged,
    Machine(crate::machine_state::MachineStateError),
    Checkpoint(crate::native_checkpoint::NativeCheckpointError),
    Creation(crate::state_provision::StateProvisionError),
    Filesystem(rustix::io::Errno),
    Io(std::io::Error),
    Encoding,
    InvalidRecord,
}
impl std::fmt::Display for CreationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "state creation transaction failed: {self:?}")
    }
}
impl std::error::Error for CreationError {}
impl From<rustix::io::Errno> for CreationError {
    fn from(value: rustix::io::Errno) -> Self {
        Self::Filesystem(value)
    }
}
impl From<std::io::Error> for CreationError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Intent {
    schema: u16,
    transaction_id: Identifier,
    bootstrap_checkpoint: SnapshotLink,
    bootstrap_subvolume_uuid: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRecord {
    schema: u16,
    transaction_id: Identifier,
    filesystem_uuid: String,
    subvolume_id: u64,
    subvolume_uuid: String,
}

/// Keep this guard alive across mount, unit installation and final identity
/// publication. Dropping it releases only the lock, never retained evidence.
/// A second attempt refuses the durable intent even if the first process died.
pub struct StateCreationTransaction {
    root: File,
    directory: File,
    _lock: File,
    intent: Intent,
    attempted: bool,
}

/// Refuse a fresh attempt before creating another checkpoint when protected
/// provisioning remnants exist, including malformed or dangling entries.
pub fn has_pending_state_creation(root: &File) -> Result<bool, CreationError> {
    crate::state_mount::validate_trusted_root(root).map_err(|error| {
        CreationError::Machine(crate::machine_state::MachineStateError::UnsafeRoot(error))
    })?;
    let directory = match open_record_directory(root, false) {
        Ok(directory) => directory,
        Err(crate::machine_state::MachineStateError::Filesystem(rustix::io::Errno::NOENT)) => {
            return Ok(false);
        }
        Err(error) => return Err(CreationError::Machine(error)),
    };
    pending_in_directory(&directory)
}

fn pending_in_directory(directory: &File) -> Result<bool, CreationError> {
    for name in [
        "state-creation-intent.json",
        "state-creation-result.json",
        ".state-creation.lock",
        "state-machine.pending",
    ] {
        match rustix::fs::statat(directory, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Ok(true),
            Err(rustix::io::Errno::NOENT) => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

impl StateCreationTransaction {
    /// Revalidate a native root checkpoint before any provisioning writes.
    /// Caller separately establishes authorization, supported boot layout and
    /// serialization with other protected operations, including snapshot creation.
    pub fn begin(
        root: &File,
        checkpoint: &NativeRootCheckpoint,
        transaction_id: Identifier,
    ) -> Result<Self, CreationError> {
        Self::open_locked(root, checkpoint, transaction_id, true)
    }

    /// Resume only when complete protected intent/result records and the
    /// already-mounted state partner agree. Missing/partial receipts never
    /// authorize adoption or recreation. The caller retains ownership of mount
    /// setup and the global mutation lock; this guard holds the provisioning lock.
    pub fn resume_created(
        root: &File,
        checkpoint: &NativeRootCheckpoint,
        transaction_id: Identifier,
    ) -> Result<(Self, CreatedStateSubvolume), CreationError> {
        let guard = Self::open_locked(root, checkpoint, transaction_id, false)?;
        let result: ResultRecord = read_record(&guard.directory, "state-creation-result.json")?;
        let record = result_identity(&guard.intent, &result)?;
        let created = reopen_created_state_mount(root, &record, &result.subvolume_uuid)
            .map_err(CreationError::Creation)?;
        Ok((guard, created))
    }

    /// Resume a complete receipt through a caller-owned temporary ID-5 mount.
    /// No existing volume is adopted without its exact recorded UUID and ID;
    /// no metadata repair, mount or creation is performed by this lookup.
    pub fn resume_created_from_top(
        root: &File,
        checkpoint: &NativeRootCheckpoint,
        transaction_id: Identifier,
        top_level: &File,
    ) -> Result<(Self, CreatedStateSubvolume), CreationError> {
        let guard = Self::open_locked(root, checkpoint, transaction_id, false)?;
        let result: ResultRecord = read_record(&guard.directory, "state-creation-result.json")?;
        let record = result_identity(&guard.intent, &result)?;
        let created =
            reopen_created_from_top_level(root, top_level, &record, &result.subvolume_uuid)
                .map_err(CreationError::Creation)?;
        Ok((guard, created))
    }

    fn open_locked(
        root: &File,
        checkpoint: &NativeRootCheckpoint,
        transaction_id: Identifier,
        fresh: bool,
    ) -> Result<Self, CreationError> {
        if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0 {
            return Err(CreationError::RootRequired);
        }
        let link = checkpoint.verified_link().link().clone();
        let current =
            verify_new_root_checkpoint(root, link.clone()).map_err(CreationError::Checkpoint)?;
        if current.subvolume_uuid != checkpoint.subvolume_uuid
            || current.subvolume_id != checkpoint.subvolume_id
        {
            return Err(CreationError::CheckpointChanged);
        }
        let directory = open_record_directory(root, fresh).map_err(CreationError::Machine)?;
        let lock = File::from(openat(
            &directory,
            ".state-creation.lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        let stat = fstat(&lock)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_uid != 0
            || stat.st_gid != 0
            || stat.st_nlink != 1
            || stat.st_mode & 0o7777 != 0o600
        {
            return Err(CreationError::UnsafeLock);
        }
        flock(&lock, FlockOperation::LockExclusive)?;
        let intent = Intent {
            schema: 1,
            transaction_id,
            bootstrap_checkpoint: link,
            bootstrap_subvolume_uuid: current.subvolume_uuid,
        };
        if fresh {
            persist_exclusive(&directory, "state-creation-intent.json", &intent)?;
        } else {
            let saved: Intent = read_record(&directory, "state-creation-intent.json")?;
            if saved != intent {
                return Err(CreationError::InvalidRecord);
            }
        }
        Ok(Self { root: root.try_clone()?, directory, _lock: lock, intent, attempted: !fresh })
    }

    /// Persisted intent must exist before this call can reach the creation
    /// ioctl. A failed or interrupted attempt must not be retried blindly.
    pub fn create(&mut self, top_level: &File) -> Result<CreatedStateSubvolume, CreationError> {
        if self.attempted {
            return Err(CreationError::NeedsReconciliation);
        }
        self.attempted = true;
        let current =
            verify_new_root_checkpoint(&self.root, self.intent.bootstrap_checkpoint.clone())
                .map_err(CreationError::Checkpoint)?;
        if current.subvolume_uuid != self.intent.bootstrap_subvolume_uuid {
            return Err(CreationError::CheckpointChanged);
        }
        let created =
            create_state_subvolume(&self.root, top_level).map_err(CreationError::Creation)?;
        persist_exclusive(
            &self.directory,
            "state-creation-result.json",
            &ResultRecord {
                schema: 1,
                transaction_id: self.intent.transaction_id.clone(),
                filesystem_uuid: created.machine_record().identity().filesystem_uuid.clone(),
                subvolume_id: created.machine_record().identity().subvolume_id,
                subvolume_uuid: created.subvolume_uuid().into(),
            },
        )?;
        Ok(created)
    }
}

fn result_identity(
    intent: &Intent,
    result: &ResultRecord,
) -> Result<crate::machine_state::MachineStateRecord, CreationError> {
    if result.schema != 1
        || result.transaction_id != intent.transaction_id
        || result.filesystem_uuid != intent.bootstrap_checkpoint.filesystem_uuid
    {
        return Err(CreationError::InvalidRecord);
    }
    crate::machine_state::MachineStateRecord::from_provisioned_subvolume(
        result.filesystem_uuid.clone(),
        result.subvolume_id,
    )
    .map_err(CreationError::Machine)
}

fn read_record<T: DeserializeOwned + Serialize>(
    directory: &File,
    name: &str,
) -> Result<T, CreationError> {
    let file = File::from(openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let stat = fstat(&file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_nlink != 1
        || stat.st_size <= 0
        || stat.st_size > 4096
    {
        return Err(CreationError::InvalidRecord);
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    parse_record(&bytes)
}

fn parse_record<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, CreationError> {
    if bytes.is_empty() || bytes.len() > 4096 {
        return Err(CreationError::InvalidRecord);
    }
    let record: T = serde_json::from_slice(bytes).map_err(|_| CreationError::InvalidRecord)?;
    if serde_json::to_vec(&record).map_err(|_| CreationError::Encoding)? != bytes {
        return Err(CreationError::InvalidRecord);
    }
    Ok(record)
}

fn persist_exclusive(
    directory: &File,
    name: &str,
    value: &impl Serialize,
) -> Result<(), CreationError> {
    let bytes = serde_json::to_vec(value).map_err(|_| CreationError::Encoding)?;
    let fd = openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            CreationError::NeedsReconciliation
        } else {
            CreationError::Filesystem(error)
        }
    })?;
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600))?;
    let mut file = File::from(fd);
    file.write_all(&bytes)?;
    file.sync_all()?;
    fsync(directory)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn preflight_detects_even_partial_and_dangling_attempt_records() {
        let path = std::env::temp_dir().join(format!("ryoku-pending-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = File::open(&path).unwrap();
        assert!(!pending_in_directory(&directory).unwrap());
        fs::write(path.join("state-creation-intent.json"), b"partial").unwrap();
        assert!(pending_in_directory(&directory).unwrap());
        fs::remove_file(path.join("state-creation-intent.json")).unwrap();
        std::os::unix::fs::symlink("missing", path.join("state-creation-result.json")).unwrap();
        assert!(pending_in_directory(&directory).unwrap());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn receipt_rejects_cross_transaction_wrong_filesystem_and_noncanonical_encoding() {
        let uuid = "01234567-89ab-cdef-0123-456789abcdef";
        let intent = Intent {
            schema: 1,
            transaction_id: Identifier::new("attempt-a").unwrap(),
            bootstrap_checkpoint: SnapshotLink::new(Identifier::new("root").unwrap(), "42", uuid)
                .unwrap(),
            bootstrap_subvolume_uuid: uuid.into(),
        };
        let mut result = ResultRecord {
            schema: 1,
            transaction_id: intent.transaction_id.clone(),
            filesystem_uuid: uuid.into(),
            subvolume_id: 300,
            subvolume_uuid: uuid.into(),
        };
        assert!(result_identity(&intent, &result).is_ok());
        result.transaction_id = Identifier::new("attempt-b").unwrap();
        assert!(result_identity(&intent, &result).is_err());
        result.transaction_id = intent.transaction_id.clone();
        result.filesystem_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into();
        assert!(result_identity(&intent, &result).is_err());
        result.filesystem_uuid = uuid.into();
        result.schema = 2;
        assert!(result_identity(&intent, &result).is_err());
        result.schema = 1;
        let bytes = serde_json::to_vec(&result).unwrap();
        assert!(parse_record::<ResultRecord>(&bytes).is_ok());
        for invalid in [
            bytes[..bytes.len() - 1].to_vec(),
            [bytes.as_slice(), b"\n"].concat(),
            String::from_utf8(bytes.clone())
                .unwrap()
                .replace("\"schema\":1", "\"schema\":1,\"schema\":1")
                .into_bytes(),
            String::from_utf8(bytes)
                .unwrap()
                .replace("\"schema\":1", "\"schema\":1,\"foreign\":true")
                .into_bytes(),
        ] {
            assert!(parse_record::<ResultRecord>(&invalid).is_err());
        }
    }

    #[test]
    fn prior_partial_or_complete_intent_is_preserved_for_reconciliation() {
        let path =
            std::env::temp_dir().join(format!("ryoku-creation-intent-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = File::open(&path).unwrap();
        let intent = "state-creation-intent.json";
        fs::write(path.join(intent), b"partial").unwrap();
        assert!(matches!(
            persist_exclusive(&directory, intent, &"new"),
            Err(CreationError::NeedsReconciliation)
        ));
        assert_eq!(fs::read(path.join(intent)).unwrap(), b"partial");
        fs::remove_file(path.join(intent)).unwrap();
        persist_exclusive(&directory, intent, &"original").unwrap();
        assert_eq!(fs::metadata(path.join(intent)).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(matches!(
            persist_exclusive(&directory, intent, &"different"),
            Err(CreationError::NeedsReconciliation)
        ));
        assert_eq!(fs::read(path.join(intent)).unwrap(), b"\"original\"");
        fs::remove_dir_all(path).unwrap();
    }
}
