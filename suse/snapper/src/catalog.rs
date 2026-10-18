//! Bounded versioned catalog storage rooted at a trusted directory descriptor.

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
        AtFlags, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, flock, fsync,
        openat, openat2, renameat_with, statat,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CatalogEntry, CatalogIndex, ConfigName, GenerationId, LayoutCapability, PartnerState,
    SnapshotId, TransactionId, ValueError,
};

const STORE_NAME: &str = "catalog.json";
const LOCK_NAME: &str = ".catalog.lock";
const MAX_CATALOG_BYTES: u64 = 512 * 1024;
const MAX_CATALOG_ENTRIES: usize = 2048;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Failure categories from bounded durable catalog storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogStoreError {
    Filesystem(Errno),
    Io(std::io::ErrorKind),
    InvalidStore,
    InvalidCatalog,
    TooLarge,
    TooManyEntries,
    DuplicateTransaction,
    InvalidValue(ValueError),
}

impl std::fmt::Display for CatalogStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Snapper catalog operation failed: {self:?}")
    }
}

impl std::error::Error for CatalogStoreError {}

impl From<Errno> for CatalogStoreError {
    fn from(value: Errno) -> Self {
        Self::Filesystem(value)
    }
}

impl From<std::io::Error> for CatalogStoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.kind())
    }
}

/// Atomic, bounded catalog store relative to a private directory descriptor.
/// The caller must supply a persistent directory outside every rollback scope
/// and provide its expected owner UID. This type cannot prove mount placement.
pub struct CatalogStore {
    root: File,
    lock_file: File,
    process_lock: Mutex<()>,
    expected_uid: u32,
}

/// Read-only view of a validated Snapper catalog. Atomic rename lets readers
/// observe either the complete old file or the complete new file without
/// opening the writer lock for mutation.
pub struct CatalogReader {
    root: File,
    expected_uid: u32,
}

impl CatalogReader {
    /// Duplicates an already-open private directory descriptor. This method
    /// never creates a lock file or changes the directory.
    pub fn open(trusted_root: &File, expected_uid: u32) -> Result<Self, CatalogStoreError> {
        let stat = rustix::fs::fstat(trusted_root)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
            || stat.st_uid != expected_uid
            || stat.st_mode & 0o7777 != 0o700
        {
            return Err(CatalogStoreError::InvalidStore);
        }
        let root = File::from(rustix::io::dup(trusted_root)?);
        Ok(Self { root, expected_uid })
    }

    /// Reads, bounds, hashes, parses, versions, and validates the catalog.
    pub fn load(&self) -> Result<CatalogIndex, CatalogStoreError> {
        read_catalog(&self.root, self.expected_uid)
    }
}

impl CatalogStore {
    /// Duplicates an already-open protected directory descriptor. The
    /// directory must be owned by `expected_uid` and have exact mode 0700.
    pub fn open(trusted_root: &File, expected_uid: u32) -> Result<Self, CatalogStoreError> {
        Self::open_with_lock(trusted_root, expected_uid, true)
    }

    /// Opens an existing catalog writer without creating or repairing its
    /// lock inode. Use this on normal recovery and transaction paths; reserve
    /// [`Self::open`] for explicit state provisioning.
    pub fn open_existing(
        trusted_root: &File,
        expected_uid: u32,
    ) -> Result<Self, CatalogStoreError> {
        Self::open_with_lock(trusted_root, expected_uid, false)
    }

    fn open_with_lock(
        trusted_root: &File,
        expected_uid: u32,
        create_lock: bool,
    ) -> Result<Self, CatalogStoreError> {
        let stat = rustix::fs::fstat(trusted_root)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
            || stat.st_uid != expected_uid
            || stat.st_mode & 0o7777 != 0o700
        {
            return Err(CatalogStoreError::InvalidStore);
        }
        let root = File::from(rustix::io::dup(trusted_root)?);
        let flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        let lock_file = File::from(if create_lock {
            openat(&root, LOCK_NAME, flags | OFlags::CREATE, Mode::from_raw_mode(0o600))?
        } else {
            openat(&root, LOCK_NAME, flags, Mode::empty())?
        });
        validate_lock_path(&root, &lock_file, expected_uid)?;
        Ok(Self { root, lock_file, process_lock: Mutex::new(()), expected_uid })
    }

    /// Reads, bounds, hashes, parses, versions, and validates the stored catalog.
    pub fn load(&self) -> Result<CatalogIndex, CatalogStoreError> {
        self.with_lock(|| read_catalog(&self.root, self.expected_uid))
    }

    /// Atomically replaces the catalog with a complete, validated snapshot.
    /// The data file is synced before rename and the directory is synced after.
    pub fn replace(&self, catalog: &CatalogIndex) -> Result<(), CatalogStoreError> {
        self.with_lock(|| {
            let bytes = encode_catalog(catalog)?;
            self.write_atomic(&bytes)
        })
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, CatalogStoreError>,
    ) -> Result<T, CatalogStoreError> {
        let _process_guard =
            self.process_lock.lock().map_err(|_| CatalogStoreError::InvalidStore)?;
        validate_lock_path(&self.root, &self.lock_file, self.expected_uid)?;
        flock(&self.lock_file, FlockOperation::LockExclusive)?;
        if let Err(error) = validate_lock_path(&self.root, &self.lock_file, self.expected_uid) {
            let _ = flock(&self.lock_file, FlockOperation::Unlock);
            return Err(error);
        }
        let result = operation();
        let unlock =
            flock(&self.lock_file, FlockOperation::Unlock).map_err(CatalogStoreError::from);
        match (result, unlock) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    fn write_atomic(&self, bytes: &[u8]) -> Result<(), CatalogStoreError> {
        let (temp_name, mut file) = create_temp(&self.root, self.expected_uid)?;
        let result = (|| {
            file.write_all(bytes)?;
            fchmod(&file, Mode::from_raw_mode(0o600))?;
            file.sync_all()?;
            renameat_with(
                &self.root,
                temp_name.as_str(),
                &self.root,
                STORE_NAME,
                RenameFlags::empty(),
            )?;
            fsync(&self.root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ =
                rustix::fs::unlinkat(&self.root, temp_name.as_str(), rustix::fs::AtFlags::empty());
        }
        result
    }
}

/// Require the fixed lock name to keep referring to the inode we opened.
/// Checking again after `flock` catches a replacement while acquisition waited
/// and prevents cooperating writers from serializing on different inodes.
fn validate_lock_path(
    root: &File,
    lock_file: &File,
    expected_uid: u32,
) -> Result<(), CatalogStoreError> {
    let root_stat = rustix::fs::fstat(root)?;
    if rustix::fs::FileType::from_raw_mode(root_stat.st_mode) != rustix::fs::FileType::Directory
        || root_stat.st_uid != expected_uid
        || root_stat.st_mode & 0o7777 != 0o700
    {
        return Err(CatalogStoreError::InvalidStore);
    }
    let lock_stat = rustix::fs::fstat(lock_file)?;
    let path_stat = statat(root, LOCK_NAME, AtFlags::SYMLINK_NOFOLLOW)?;
    if rustix::fs::FileType::from_raw_mode(lock_stat.st_mode) != rustix::fs::FileType::RegularFile
        || lock_stat.st_uid != expected_uid
        || lock_stat.st_gid != root_stat.st_gid
        || lock_stat.st_mode & 0o7777 != 0o600
        || lock_stat.st_nlink != 1
        || path_stat.st_dev != lock_stat.st_dev
        || path_stat.st_ino != lock_stat.st_ino
    {
        return Err(CatalogStoreError::InvalidStore);
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogEnvelope {
    payload: CatalogPayload,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogPayload {
    schema_version: u32,
    entries: Vec<StoredEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEntry {
    transaction_id: String,
    root_config: String,
    root_snapshot: String,
    generation_id: String,
    capability: StoredCapability,
    partner: StoredPartner,
    manual_checkpoint: bool,
    known_working: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredCapability {
    FullSystem,
    ConfigurationOnly,
    Unsupported,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredPartner {
    Missing,
    PresentUnverified,
    Verified,
}

fn encode_catalog(catalog: &CatalogIndex) -> Result<Vec<u8>, CatalogStoreError> {
    if catalog.entries.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogStoreError::TooManyEntries);
    }
    let mut entries = Vec::with_capacity(catalog.entries.len());
    for entry in catalog.entries.values() {
        validate_entry(entry)?;
        entries.push(store_entry(entry));
    }
    let payload = CatalogPayload { schema_version: 1, entries };
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|_| CatalogStoreError::InvalidCatalog)?;
    let envelope = CatalogEnvelope { payload, sha256: hex_digest(&payload_bytes) };
    let bytes = serde_json::to_vec(&envelope).map_err(|_| CatalogStoreError::InvalidCatalog)?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(CatalogStoreError::TooLarge);
    }
    Ok(bytes)
}

fn read_catalog(root: &File, expected_uid: u32) -> Result<CatalogIndex, CatalogStoreError> {
    let root_stat = rustix::fs::fstat(root)?;
    if rustix::fs::FileType::from_raw_mode(root_stat.st_mode) != rustix::fs::FileType::Directory
        || root_stat.st_uid != expected_uid
        || root_stat.st_mode & 0o7777 != 0o700
    {
        return Err(CatalogStoreError::InvalidStore);
    }
    let fd = openat2(
        root,
        STORE_NAME,
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
        || stat.st_uid != expected_uid
        || stat.st_gid != root_stat.st_gid
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_nlink != 1
        || stat.st_size < 0
    {
        return Err(CatalogStoreError::InvalidCatalog);
    }
    if stat.st_size as u64 > MAX_CATALOG_BYTES {
        return Err(CatalogStoreError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CATALOG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(CatalogStoreError::TooLarge);
    }
    let envelope: CatalogEnvelope =
        serde_json::from_slice(&bytes).map_err(|_| CatalogStoreError::InvalidCatalog)?;
    if envelope.payload.schema_version != 1 {
        return Err(CatalogStoreError::InvalidCatalog);
    }
    let payload_bytes =
        serde_json::to_vec(&envelope.payload).map_err(|_| CatalogStoreError::InvalidCatalog)?;
    if envelope.sha256 != hex_digest(&payload_bytes) {
        return Err(CatalogStoreError::InvalidCatalog);
    }
    if envelope.payload.entries.len() > MAX_CATALOG_ENTRIES {
        return Err(CatalogStoreError::TooManyEntries);
    }
    let mut catalog = CatalogIndex::default();
    for stored in envelope.payload.entries {
        let entry = load_entry(stored)?;
        validate_entry(&entry)?;
        if catalog.insert(entry).is_err() {
            return Err(CatalogStoreError::DuplicateTransaction);
        }
    }
    Ok(catalog)
}

fn store_entry(entry: &CatalogEntry) -> StoredEntry {
    StoredEntry {
        transaction_id: entry.transaction_id.as_str().to_owned(),
        root_config: entry.root_config.as_str().to_owned(),
        root_snapshot: entry.root_snapshot.as_str().to_owned(),
        generation_id: entry.generation_id.as_str().to_owned(),
        capability: match entry.capability {
            LayoutCapability::FullSystem => StoredCapability::FullSystem,
            LayoutCapability::ConfigurationOnly => StoredCapability::ConfigurationOnly,
            LayoutCapability::Unsupported => StoredCapability::Unsupported,
        },
        partner: match entry.partner {
            PartnerState::Missing => StoredPartner::Missing,
            PartnerState::PresentUnverified => StoredPartner::PresentUnverified,
            PartnerState::Verified => StoredPartner::Verified,
        },
        manual_checkpoint: entry.manual_checkpoint,
        known_working: entry.known_working,
    }
}

fn load_entry(stored: StoredEntry) -> Result<CatalogEntry, CatalogStoreError> {
    Ok(CatalogEntry {
        transaction_id: TransactionId::new(stored.transaction_id)
            .map_err(CatalogStoreError::InvalidValue)?,
        root_config: ConfigName::new(stored.root_config)
            .map_err(CatalogStoreError::InvalidValue)?,
        root_snapshot: SnapshotId::new(stored.root_snapshot)
            .map_err(CatalogStoreError::InvalidValue)?,
        generation_id: GenerationId::new(stored.generation_id)
            .map_err(CatalogStoreError::InvalidValue)?,
        capability: match stored.capability {
            StoredCapability::FullSystem => LayoutCapability::FullSystem,
            StoredCapability::ConfigurationOnly => LayoutCapability::ConfigurationOnly,
            StoredCapability::Unsupported => LayoutCapability::Unsupported,
        },
        partner: match stored.partner {
            StoredPartner::Missing => PartnerState::Missing,
            StoredPartner::PresentUnverified => PartnerState::PresentUnverified,
            StoredPartner::Verified => PartnerState::Verified,
        },
        manual_checkpoint: stored.manual_checkpoint,
        known_working: stored.known_working,
    })
}

fn validate_entry(entry: &CatalogEntry) -> Result<(), CatalogStoreError> {
    TransactionId::new(entry.transaction_id.as_str().to_owned())
        .map_err(CatalogStoreError::InvalidValue)?;
    ConfigName::new(entry.root_config.as_str().to_owned())
        .map_err(CatalogStoreError::InvalidValue)?;
    SnapshotId::new(entry.root_snapshot.as_str().to_owned())
        .map_err(CatalogStoreError::InvalidValue)?;
    GenerationId::new(entry.generation_id.as_str().to_owned())
        .map_err(CatalogStoreError::InvalidValue)?;
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn create_temp(root: &File, expected_uid: u32) -> Result<(String, File), CatalogStoreError> {
    for _ in 0..32 {
        let serial = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!(".catalog-{}-{serial}.tmp", std::process::id());
        match openat(
            root,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                if rustix::fs::fstat(&file)?.st_uid != expected_uid {
                    let _ = rustix::fs::unlinkat(root, name.as_str(), rustix::fs::AtFlags::empty());
                    return Err(CatalogStoreError::InvalidStore);
                }
                return Ok((name, file));
            }
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(CatalogStoreError::InvalidStore)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    fn root() -> (std::path::PathBuf, File, u32) {
        let path = std::env::temp_dir().join(format!(
            "ryoku-snapper-catalog-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let dir = File::open(&path).unwrap();
        let owner = rustix::fs::fstat(&dir).unwrap().st_uid;
        (path, dir, owner)
    }

    fn entry(transaction: &str, snapshot: &str) -> CatalogEntry {
        CatalogEntry {
            transaction_id: TransactionId::new(transaction).unwrap(),
            root_config: ConfigName::new("root").unwrap(),
            root_snapshot: SnapshotId::new(snapshot).unwrap(),
            generation_id: GenerationId::new("generation-1").unwrap(),
            capability: LayoutCapability::FullSystem,
            partner: PartnerState::Verified,
            manual_checkpoint: false,
            known_working: false,
        }
    }

    fn index() -> CatalogIndex {
        let mut index = CatalogIndex::default();
        index.insert(entry("txn-1", "10")).unwrap();
        index.insert(entry("txn-2", "12")).unwrap();
        index
    }

    fn store(dir: &File, owner: u32) -> CatalogStore {
        CatalogStore::open(dir, owner).unwrap()
    }

    #[test]
    fn read_only_catalog_reader_does_not_create_a_writer_lock() {
        let (path, dir, owner) = root();
        let reader = CatalogReader::open(&dir, owner).unwrap();
        assert!(matches!(reader.load(), Err(CatalogStoreError::Filesystem(Errno::NOENT))));
        assert!(
            rustix::fs::statat(&dir, LOCK_NAME, rustix::fs::AtFlags::SYMLINK_NOFOLLOW).is_err()
        );
        drop(reader);
        drop(dir);
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn existing_catalog_writer_refuses_to_create_a_missing_lock() {
        let (path, dir, owner) = root();
        assert!(matches!(
            CatalogStore::open_existing(&dir, owner),
            Err(CatalogStoreError::Filesystem(Errno::NOENT))
        ));
        assert!(
            rustix::fs::statat(&dir, LOCK_NAME, rustix::fs::AtFlags::SYMLINK_NOFOLLOW).is_err()
        );
        let created = CatalogStore::open(&dir, owner).unwrap();
        created.replace(&CatalogIndex::default()).unwrap();
        drop(created);
        let existing = CatalogStore::open_existing(&dir, owner).unwrap();
        assert!(existing.load().unwrap().entries().next().is_none());
        drop(existing);
        drop(dir);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn descriptor_relative_replace_survives_directory_rename() {
        let (path, dir, owner) = root();
        let store = store(&dir, owner);
        let relocated =
            path.with_file_name(format!("{}-moved", path.file_name().unwrap().to_string_lossy()));
        fs::rename(&path, &relocated).unwrap();
        let expected = index();
        store.replace(&expected).unwrap();
        assert_eq!(
            store.load().unwrap().entries().cloned().collect::<Vec<_>>(),
            expected.entries().cloned().collect::<Vec<_>>()
        );
        assert!(!path.exists());
        let metadata = fs::metadata(relocated.join(STORE_NAME)).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        assert!(relocated.join(LOCK_NAME).is_file());

        let mut replacement = CatalogIndex::default();
        replacement.insert(entry("txn-3", "20")).unwrap();
        store.replace(&replacement).unwrap();
        assert_eq!(store.load().unwrap().entries().count(), 1);
        fs::remove_dir_all(relocated).unwrap();
    }

    #[test]
    fn tampered_catalog_and_truncated_document_are_rejected() {
        let (path, dir, owner) = root();
        let store = store(&dir, owner);
        store.replace(&index()).unwrap();
        let filename = path.join(STORE_NAME);

        let mut bytes = fs::read(&filename).unwrap();
        let needle = b"txn-1";
        let offset = bytes.windows(needle.len()).position(|window| window == needle).unwrap();
        bytes[offset + 4] = b'9';
        fs::write(&filename, &bytes).unwrap();
        assert!(matches!(store.load(), Err(CatalogStoreError::InvalidCatalog)));

        store.replace(&index()).unwrap();
        let mut bytes = fs::read(&filename).unwrap();
        bytes.truncate(bytes.len() - 7);
        fs::write(&filename, bytes).unwrap();
        assert!(matches!(store.load(), Err(CatalogStoreError::InvalidCatalog)));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn wrong_directory_owner_or_mode_is_rejected() {
        let (path, dir, owner) = root();
        assert!(matches!(
            CatalogStore::open(&dir, owner.wrapping_add(1)),
            Err(CatalogStoreError::InvalidStore)
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(CatalogStore::open(&dir, owner).err(), Some(CatalogStoreError::InvalidStore));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn duplicate_transactions_and_oversized_catalogs_fail_before_publication() {
        let (path, dir, owner) = root();
        let store = store(&dir, owner);
        let mut duplicate = CatalogIndex::default();
        duplicate.insert(entry("txn-1", "10")).unwrap();
        assert!(duplicate.insert(entry("txn-1", "11")).is_err());
        assert_eq!(store.replace(&duplicate), Ok(()));
        assert_eq!(store.load().unwrap().entries().count(), 1);

        let mut oversized = CatalogIndex::default();
        for value in 0..=MAX_CATALOG_ENTRIES {
            let record = entry(&format!("txn-{value}"), "10");
            oversized.entries.insert(record.transaction_id.clone(), record);
        }
        assert_eq!(store.replace(&oversized), Err(CatalogStoreError::TooManyEntries));
        assert_eq!(store.load().unwrap().entries().count(), 1);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn wrong_mode_catalog_file_is_rejected() {
        let (path, dir, owner) = root();
        let store = store(&dir, owner);
        store.replace(&index()).unwrap();
        fs::set_permissions(path.join(STORE_NAME), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(store.load(), Err(CatalogStoreError::InvalidCatalog)));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unsupported_schema_and_oversized_catalog_are_rejected() {
        let (path, dir, owner) = root();
        let store = store(&dir, owner);
        let payload = CatalogPayload { schema_version: 2, entries: Vec::new() };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let envelope = CatalogEnvelope { payload, sha256: hex_digest(&payload_bytes) };
        fs::write(path.join(STORE_NAME), serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(matches!(store.load(), Err(CatalogStoreError::InvalidCatalog)));

        fs::write(path.join(STORE_NAME), vec![b'x'; MAX_CATALOG_BYTES as usize + 1]).unwrap();
        fs::set_permissions(path.join(STORE_NAME), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(store.load(), Err(CatalogStoreError::TooLarge)));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn catalog_digest_is_canonical_lowercase_sha256() {
        assert_eq!(
            hex_digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn decoded_rows_revalidate_all_identifiers() {
        let payload = CatalogPayload {
            schema_version: 1,
            entries: vec![StoredEntry {
                transaction_id: "../bad".into(),
                root_config: "root".into(),
                root_snapshot: "1".into(),
                generation_id: "generation-1".into(),
                capability: StoredCapability::FullSystem,
                partner: StoredPartner::Verified,
                manual_checkpoint: false,
                known_working: false,
            }],
        };
        let catalog = CatalogIndex::default();
        for stored in payload.entries {
            let result = load_entry(stored);
            assert!(matches!(
                result,
                Err(CatalogStoreError::InvalidValue(ValueError::InvalidTransactionId))
            ));
            assert_eq!(catalog.entries.len(), 0);
        }
    }

    #[test]
    fn stable_persistence_order_is_transaction_id_order() {
        let mut reversed = CatalogIndex::default();
        reversed.insert(entry("txn-z", "1")).unwrap();
        reversed.insert(entry("txn-a", "2")).unwrap();
        let stored = encode_catalog(&reversed).unwrap();
        let envelope: CatalogEnvelope = serde_json::from_slice(&stored).unwrap();
        assert_eq!(envelope.payload.entries[0].transaction_id, "txn-a");
        assert_eq!(envelope.payload.entries[1].transaction_id, "txn-z");
    }
}
