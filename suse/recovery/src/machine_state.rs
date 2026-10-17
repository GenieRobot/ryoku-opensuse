//! Strict persisted identity for the protected Btrfs state subvolume.
//!
//! The machine record lives on the rollback-managed root at
//! `/etc/ryoku/state-machine.json`, separate from the volume it validates. Its
//! expected UUID and subvolume IDs are written from the trusted provisioning
//! transaction that creates the dedicated subvolume. Startup reads this record
//! and compares it with live mount evidence; it must never construct the
//! expected identity by copying the current mount observation.

use std::{
    fs::File,
    io::{Read, Take, Write},
};

use rustix::{
    fd::AsFd,
    fs::{
        FileType, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, fstat, fsync, mkdirat, openat,
        openat2, renameat_with,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};

use crate::state_mount::{StateMountError, StateMountIdentity, validate_trusted_root};

/// The one trusted machine-record path, outside the state mount it validates.
pub const MACHINE_STATE_PATH: &str = "/etc/ryoku/state-machine.json";
const MAX_MACHINE_STATE_BYTES: u64 = 4096;

/// A validated identity record installed during state-subvolume provisioning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineStateRecord {
    identity: StateMountIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedMachineState {
    schema_version: u16,
    filesystem_uuid: String,
    subvolume_id: u64,
    parent_subvolume_id: u64,
}

/// Typed failure reading or provisioning the trusted machine identity.
#[derive(Debug, Eq, PartialEq)]
pub enum MachineStateError {
    UnsafeRoot(StateMountError),
    UnsafeRecord,
    InvalidRecord,
    UnsupportedVersion(u16),
    InvalidIdentity,
    TooLarge,
    AlreadyProvisioned,
    Filesystem(Errno),
    Io,
}

impl std::fmt::Display for MachineStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafeRoot(_) => formatter.write_str("machine identity root is unsafe"),
            Self::UnsafeRecord => {
                formatter.write_str("machine identity is not a root-owned private regular file")
            }
            Self::InvalidRecord => formatter.write_str("machine identity record is invalid"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "machine identity schema version {version} is unsupported")
            }
            Self::InvalidIdentity => formatter.write_str("machine identity values are invalid"),
            Self::TooLarge => formatter.write_str("machine identity record exceeds 4 KiB"),
            Self::AlreadyProvisioned => {
                formatter.write_str("a different machine identity is already provisioned")
            }
            Self::Filesystem(_) => {
                formatter.write_str("machine identity filesystem operation failed")
            }
            Self::Io => formatter.write_str("machine identity file I/O failed"),
        }
    }
}

impl std::error::Error for MachineStateError {}

impl From<Errno> for MachineStateError {
    fn from(error: Errno) -> Self {
        Self::Filesystem(error)
    }
}

impl From<std::io::Error> for MachineStateError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

impl MachineStateRecord {
    /// Build a version-1 record from identity facts supplied by the trusted
    /// provisioning operation that just created the state subvolume. This
    /// function does not inspect or infer identity from the currently mounted
    /// state path.
    pub fn from_provisioned_subvolume(
        filesystem_uuid: impl Into<String>,
        subvolume_id: u64,
    ) -> Result<Self, MachineStateError> {
        let identity = StateMountIdentity {
            filesystem_uuid: filesystem_uuid.into(),
            subvolume_id,
            parent_subvolume_id: 5,
        };
        validate_identity(&identity)?;
        Ok(Self { identity })
    }

    /// Expected identity to compare with live mount observations at startup.
    pub fn identity(&self) -> &StateMountIdentity {
        &self.identity
    }

    /// Canonical compact JSON bytes for the version-1 record.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MachineStateError> {
        serde_json::to_vec(&PersistedMachineState {
            schema_version: 1,
            filesystem_uuid: self.identity.filesystem_uuid.clone(),
            subvolume_id: self.identity.subvolume_id,
            parent_subvolume_id: self.identity.parent_subvolume_id,
        })
        .map_err(|_| MachineStateError::InvalidRecord)
    }

    /// Parse only canonical schema-v1 records with no duplicate/unknown keys.
    pub fn parse(bytes: &[u8]) -> Result<Self, MachineStateError> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_MACHINE_STATE_BYTES {
            return Err(MachineStateError::TooLarge);
        }
        let persisted: PersistedMachineState =
            serde_json::from_slice(bytes).map_err(|_| MachineStateError::InvalidRecord)?;
        if persisted.schema_version != 1 {
            return Err(MachineStateError::UnsupportedVersion(persisted.schema_version));
        }
        let identity = StateMountIdentity {
            filesystem_uuid: persisted.filesystem_uuid,
            subvolume_id: persisted.subvolume_id,
            parent_subvolume_id: persisted.parent_subvolume_id,
        };
        validate_identity(&identity)?;
        let record = Self { identity };
        if record.canonical_bytes()? != bytes {
            return Err(MachineStateError::InvalidRecord);
        }
        Ok(record)
    }
}

/// Read the expected identity from the fixed root-owned record. The root
/// descriptor must identify this process's `/`; symlinks are rejected, and
/// the bounded record must be root:root mode `0600` with one link.
pub fn read_machine_state_record(
    trusted_root: &impl AsFd,
) -> Result<MachineStateRecord, MachineStateError> {
    validate_trusted_root(trusted_root).map_err(MachineStateError::UnsafeRoot)?;
    let parent = open_record_directory(trusted_root, false)?;
    let record_fd = open_record_file(&parent)?;
    read_record_fd(&File::from(record_fd), 0, 0)
}

/// Provision the root-side expected identity from trusted identity facts
/// obtained by the installation transaction. It creates the fixed file with
/// a private pending file with `O_EXCL`, synchronizes it, then publishes with
/// a no-replace rename and synchronizes the directory. A complete pending file
/// can be resumed; an incomplete or conflicting one requires reconciliation.
/// Repeating the same provision request is idempotent. The caller holds the
/// provisioning lock across this operation.
pub fn provision_machine_state_record(
    trusted_root: &impl AsFd,
    record: &MachineStateRecord,
) -> Result<(), MachineStateError> {
    validate_trusted_root(trusted_root).map_err(MachineStateError::UnsafeRoot)?;
    let parent = open_record_directory(trusted_root, true)?;
    provision_in_directory(&parent, record, 0, 0)
}

pub(crate) fn open_record_directory(
    trusted_root: &impl AsFd,
    create: bool,
) -> Result<File, MachineStateError> {
    let etc = openat2(
        trusted_root,
        "etc",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?;
    validate_etc_directory(&etc)?;
    if create {
        match mkdirat(&etc, "ryoku", Mode::from_bits_truncate(0o700)) {
            Ok(()) => fsync(&etc)?,
            Err(Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
    }
    let directory = openat2(
        &etc,
        "ryoku",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?;
    validate_private_directory(&directory, 0, 0)?;
    Ok(File::from(directory))
}

fn validate_etc_directory<Fd: AsFd>(fd: Fd) -> Result<(), MachineStateError> {
    let stat = fstat(fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o022 != 0
    {
        return Err(MachineStateError::UnsafeRecord);
    }
    Ok(())
}

fn validate_private_directory<Fd: AsFd>(
    fd: Fd,
    expected_uid: u32,
    expected_gid: u32,
) -> Result<(), MachineStateError> {
    let stat = fstat(fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != expected_uid
        || stat.st_gid != expected_gid
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(MachineStateError::UnsafeRecord);
    }
    Ok(())
}

fn open_record_file(directory: &impl AsFd) -> Result<rustix::fd::OwnedFd, MachineStateError> {
    Ok(openat2(
        directory,
        "state-machine.json",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?)
}

fn read_record_fd(
    file: &File,
    expected_uid: u32,
    expected_gid: u32,
) -> Result<MachineStateRecord, MachineStateError> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_uid
        || stat.st_gid != expected_gid
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(MachineStateError::UnsafeRecord);
    }
    if stat.st_size <= 0 || stat.st_size as u64 > MAX_MACHINE_STATE_BYTES {
        return Err(MachineStateError::TooLarge);
    }
    let mut reader: Take<&File> = file.take(MAX_MACHINE_STATE_BYTES + 1);
    let mut bytes = Vec::with_capacity(stat.st_size as usize);
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MACHINE_STATE_BYTES {
        return Err(MachineStateError::TooLarge);
    }
    MachineStateRecord::parse(&bytes)
}

fn provision_in_directory(
    directory: &File,
    record: &MachineStateRecord,
    expected_uid: u32,
    expected_gid: u32,
) -> Result<(), MachineStateError> {
    match openat2(
        directory,
        "state-machine.json",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    ) {
        Ok(existing) => {
            let existing = read_record_fd(&File::from(existing), expected_uid, expected_gid)?;
            return if existing == *record {
                fsync(directory)?;
                Ok(())
            } else {
                Err(MachineStateError::AlreadyProvisioned)
            };
        }
        Err(Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }

    let pending = "state-machine.pending";
    let raw_fd = match openat(
        directory,
        pending,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_bits_truncate(0o600),
    ) {
        Ok(raw_fd) => Some(raw_fd),
        Err(Errno::EXIST) => None,
        Err(error) => return Err(error.into()),
    };
    if let Some(raw_fd) = raw_fd {
        fchmod(&raw_fd, Mode::from_bits_truncate(0o600))?;
        let mut output = File::from(raw_fd);
        output.write_all(&record.canonical_bytes()?)?;
        output.sync_all()?;
    }
    let pending_fd = File::from(openat2(
        directory,
        pending,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    if read_record_fd(&pending_fd, expected_uid, expected_gid)? != *record {
        return Err(MachineStateError::AlreadyProvisioned);
    }
    pending_fd.sync_all()?;
    renameat_with(directory, pending, directory, "state-machine.json", RenameFlags::NOREPLACE)?;
    fsync(directory)?;
    Ok(())
}

fn validate_identity(identity: &StateMountIdentity) -> Result<(), MachineStateError> {
    let bytes = identity.filesystem_uuid.as_bytes();
    let valid_uuid = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        });
    if !valid_uuid || identity.subvolume_id <= 5 || identity.parent_subvolume_id != 5 {
        return Err(MachineStateError::InvalidIdentity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ryoku-machine-state-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn record() -> MachineStateRecord {
        MachineStateRecord::from_provisioned_subvolume("01234567-89ab-cdef-0123-456789abcdef", 256)
            .unwrap()
    }

    #[test]
    fn version_one_record_round_trips_canonically() {
        let expected = record();
        let encoded = expected.canonical_bytes().unwrap();
        assert_eq!(MachineStateRecord::parse(&encoded).unwrap(), expected);
        assert_eq!(
            encoded,
            br#"{"schema_version":1,"filesystem_uuid":"01234567-89ab-cdef-0123-456789abcdef","subvolume_id":256,"parent_subvolume_id":5}"#
        );
    }

    #[test]
    fn rejects_unknown_duplicate_noncanonical_and_unsupported_records() {
        let valid = record().canonical_bytes().unwrap();
        let unknown = String::from_utf8(valid.clone())
            .unwrap()
            .replace("\"parent_subvolume_id\":5", "\"parent_subvolume_id\":5,\"extra\":true");
        assert_eq!(
            MachineStateRecord::parse(unknown.as_bytes()),
            Err(MachineStateError::InvalidRecord)
        );
        let duplicate = br#"{"schema_version":1,"schema_version":1,"filesystem_uuid":"01234567-89ab-cdef-0123-456789abcdef","subvolume_id":256,"parent_subvolume_id":5}"#;
        assert_eq!(MachineStateRecord::parse(duplicate), Err(MachineStateError::InvalidRecord));
        let unsupported = String::from_utf8(valid.clone())
            .unwrap()
            .replace("\"schema_version\":1", "\"schema_version\":2");
        assert_eq!(
            MachineStateRecord::parse(unsupported.as_bytes()),
            Err(MachineStateError::UnsupportedVersion(2))
        );
        let spaced = String::from_utf8(valid).unwrap().replace(',', ", ");
        assert_eq!(
            MachineStateRecord::parse(spaced.as_bytes()),
            Err(MachineStateError::InvalidRecord)
        );
    }

    #[test]
    fn provisioning_facts_must_identify_direct_child_of_top_level() {
        assert_eq!(
            MachineStateRecord::from_provisioned_subvolume("not-a-uuid", 256).unwrap_err(),
            MachineStateError::InvalidIdentity
        );
        assert_eq!(
            MachineStateRecord::from_provisioned_subvolume(
                "01234567-89ab-cdef-0123-456789abcdef",
                5
            )
            .unwrap_err(),
            MachineStateError::InvalidIdentity
        );
        let nested = br#"{"schema_version":1,"filesystem_uuid":"01234567-89ab-cdef-0123-456789abcdef","subvolume_id":256,"parent_subvolume_id":257}"#;
        assert_eq!(MachineStateRecord::parse(nested), Err(MachineStateError::InvalidIdentity));
        assert_eq!(
            MachineStateRecord::parse(&vec![b' '; MAX_MACHINE_STATE_BYTES as usize + 1]),
            Err(MachineStateError::TooLarge)
        );
    }

    #[test]
    fn provision_is_exclusive_idempotent_and_never_overwrites_identity() {
        let dir_path = temp_dir();
        let dir = File::open(&dir_path).unwrap();
        let stat = fstat(&dir).unwrap();
        let owner = stat.st_uid;
        let group = stat.st_gid;
        let current = record();
        provision_in_directory(&dir, &current, owner, group).unwrap();
        provision_in_directory(&dir, &current, owner, group).unwrap();
        let file = openat2(
            &dir,
            "state-machine.json",
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .unwrap();
        let parsed = read_record_fd(&File::from(file), owner, group).unwrap();
        assert_eq!(parsed, current);
        let other = MachineStateRecord::from_provisioned_subvolume(
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            257,
        )
        .unwrap();
        assert_eq!(
            provision_in_directory(&dir, &other, owner, group),
            Err(MachineStateError::AlreadyProvisioned)
        );
        fs::remove_dir_all(dir_path).unwrap();
    }

    #[test]
    fn interrupted_publication_resumes_only_a_complete_matching_pending_record() {
        let path = temp_dir();
        let directory = File::open(&path).unwrap();
        let owner = fstat(&directory).unwrap();
        let pending_path = path.join("state-machine.pending");
        let mut pending = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pending_path)
            .unwrap();
        pending.set_permissions(fs::Permissions::from_mode(0o600)).unwrap();
        pending.write_all(b"{\"schema_version\":").unwrap();
        pending.sync_all().unwrap();
        assert!(provision_in_directory(&directory, &record(), owner.st_uid, owner.st_gid).is_err());
        assert!(!path.join("state-machine.json").exists());
        assert_eq!(fs::read(&pending_path).unwrap(), b"{\"schema_version\":");
        // Simulate explicit reconciliation of this owned test transaction.
        pending.set_len(0).unwrap();
        drop(pending);
        fs::write(&pending_path, record().canonical_bytes().unwrap()).unwrap();
        provision_in_directory(&directory, &record(), owner.st_uid, owner.st_gid).unwrap();
        assert!(!pending_path.exists());
        assert_eq!(
            fs::read(path.join("state-machine.json")).unwrap(),
            record().canonical_bytes().unwrap()
        );
        provision_in_directory(&directory, &record(), owner.st_uid, owner.st_gid).unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn reader_rejects_unsafe_mode() {
        let dir_path = temp_dir();
        let record = record();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o640)
            .open(dir_path.join("machine.json"))
            .unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o640)).unwrap();
        file.write_all(&record.canonical_bytes().unwrap()).unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            read_record_fd(&file, fstat(&file).unwrap().st_uid, fstat(&file).unwrap().st_gid),
            Err(MachineStateError::UnsafeRecord)
        );
        fs::remove_dir_all(dir_path).unwrap();
    }

    #[test]
    fn fifo_record_is_refused_without_waiting_for_a_writer() {
        let path = temp_dir();
        let directory = File::open(&path).unwrap();
        rustix::fs::mkfifoat(&directory, "state-machine.json", Mode::from_raw_mode(0o600)).unwrap();
        let stat = fstat(&directory).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = open_record_file(&directory)
                .and_then(|fd| read_record_fd(&File::from(fd), stat.st_uid, stat.st_gid));
            send.send(result).unwrap();
        });
        assert_eq!(
            receive.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            Err(MachineStateError::UnsafeRecord)
        );
        worker.join().unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn record_reader_does_not_follow_a_symlink() {
        let dir_path = temp_dir();
        let dir = File::open(&dir_path).unwrap();
        let target = dir_path.join("target.json");
        fs::write(&target, record().canonical_bytes().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, dir_path.join("state-machine.json")).unwrap();
        assert!(matches!(open_record_file(&dir), Err(MachineStateError::Filesystem(_))));
        fs::remove_dir_all(dir_path).unwrap();
    }

    #[test]
    fn reader_rejects_hard_linked_identity_records() {
        let dir_path = temp_dir();
        let file_path = dir_path.join("state-machine.json");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file_path)
            .unwrap();
        file.set_permissions(fs::Permissions::from_mode(0o600)).unwrap();
        file.write_all(&record().canonical_bytes().unwrap()).unwrap();
        file.sync_all().unwrap();
        fs::hard_link(&file_path, dir_path.join("alias.json")).unwrap();
        let stat = fstat(&file).unwrap();
        assert_eq!(
            read_record_fd(&file, stat.st_uid, stat.st_gid),
            Err(MachineStateError::UnsafeRecord)
        );
        fs::remove_dir_all(dir_path).unwrap();
    }
}
