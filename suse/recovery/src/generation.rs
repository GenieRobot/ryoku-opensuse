//! Deterministic, bounded generation serialization and dirfd-relative publication.
//!
//! This module stores and verifies captured data. It does not establish the
//! trustworthiness of the supplied root descriptor, create Snapper links, take
//! writer locks, authorize provider mutations, or restore files.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
};

use rustix::{
    fs::{
        Dir, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, fsync, mkdirat, openat, openat2,
        renameat_with,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Digest};

use crate::{EntryKind, Identifier, MAX_MANIFEST_BYTES, ManifestError, RecoveryManifest, Sha256};

const MAX_GENERATION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GENERATION_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_GENERATION_FILES: usize = 256;

/// Immutable copy inputs. Keys must equal the set of distinct regular-file
/// content digests in the manifest; repeated identical content is deduplicated.
pub type Payloads = BTreeMap<Sha256, Vec<u8>>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogLink {
    schema_version: u16,
    transaction_id: Identifier,
    generation_id: Identifier,
    manifest_sha256: Sha256,
}

impl CatalogLink {
    fn canonical_bytes(&self) -> Result<Vec<u8>, GenerationError> {
        // All members are bounded strings/numbers; serde_json's map ordering
        // gives this small index a deterministic lexical-key representation.
        let value = serde_json::to_value(self)
            .map_err(|error| GenerationError::Io(std::io::Error::other(error)))?;
        serde_json::to_vec(&value)
            .map_err(|error| GenerationError::Io(std::io::Error::other(error)))
    }
}

/// Failure to construct, publish, or verify a generation.
#[derive(Debug)]
pub enum GenerationError {
    InvalidStore,
    InvalidEntryType,
    NotMutationReady,
    UnsafeSymlinkTarget,
    InventoryTooLarge,
    PayloadMismatch,
    GenerationCollision,
    IncompletePublication,
    Manifest(ManifestError),
    Filesystem(Errno),
    Io(std::io::Error),
    Validation(crate::ValidationError),
}

impl std::fmt::Display for GenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStore => f.write_str("generation store descriptor is invalid"),
            Self::InvalidEntryType => f.write_str("generation entry has an unsafe type or mode"),
            Self::NotMutationReady => f.write_str("manifest phases are not verified"),
            Self::UnsafeSymlinkTarget => {
                f.write_str("symlink target is outside safe relative scope")
            }
            Self::InventoryTooLarge => f.write_str("generation exceeds fixed inventory bounds"),
            Self::PayloadMismatch => {
                f.write_str("payload set does not match manifest content proofs")
            }
            Self::GenerationCollision => {
                f.write_str("generation ID already names different content")
            }
            Self::IncompletePublication => f.write_str("generation is incomplete or corrupt"),
            Self::Manifest(error) => write!(f, "manifest validation failed: {error}"),
            Self::Filesystem(error) => write!(f, "generation filesystem operation failed: {error}"),
            Self::Io(error) => write!(f, "generation payload I/O failed: {error}"),
            Self::Validation(error) => write!(f, "manifest validation failed: {error:?}"),
        }
    }
}

impl std::error::Error for GenerationError {}

impl From<ManifestError> for GenerationError {
    fn from(value: ManifestError) -> Self {
        Self::Manifest(value)
    }
}
impl From<Errno> for GenerationError {
    fn from(value: Errno) -> Self {
        Self::Filesystem(value)
    }
}
impl From<std::io::Error> for GenerationError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<crate::ValidationError> for GenerationError {
    fn from(value: crate::ValidationError) -> Self {
        Self::Validation(value)
    }
}

/// Publish immutable generations below an already-open, trusted state root.
/// The root must be dedicated storage validated by a higher privileged layer.
pub struct GenerationStore {
    staging: File,
    generations: File,
    catalog: File,
}

impl GenerationStore {
    /// Opens/creates fixed store directories relative to a root-owned 0700
    /// descriptor. It does not validate that the descriptor is a dedicated mount.
    pub fn open(root: &File) -> Result<Self, GenerationError> {
        Self::open_owned(root, 0)
    }

    fn open_owned(root: &File, expected_uid: u32) -> Result<Self, GenerationError> {
        if !is_directory(root)? {
            return Err(GenerationError::InvalidStore);
        }
        let root = duplicate(root)?;
        let root_stat = rustix::fs::fstat(&root)?;
        if root_stat.st_uid != expected_uid || root_stat.st_mode & 0o7777 != 0o700 {
            return Err(GenerationError::InvalidStore);
        }
        let staging = directory(&root, "staging", expected_uid)?;
        let generations = directory(&root, "generations", expected_uid)?;
        let catalog = directory(&root, "catalog", expected_uid)?;
        fsync(&staging)?;
        fsync(&generations)?;
        fsync(&catalog)?;
        fsync(&root)?;
        Ok(Self { staging, generations, catalog })
    }

    /// Publishes all content, verifies it from disk, atomically makes the
    /// generation visible, then atomically publishes the transaction index.
    /// A crash before the final rename leaves ignored staging; a crash between
    /// the two renames may leave an unindexed immutable generation.
    pub fn publish(
        &self,
        manifest: &RecoveryManifest,
        payloads: &Payloads,
    ) -> Result<(), GenerationError> {
        if !manifest.is_mutation_ready() {
            return Err(GenerationError::NotMutationReady);
        }
        manifest.validate()?;
        if !manifest.verify_digest()? {
            return Err(GenerationError::IncompletePublication);
        }
        let manifest_bytes = manifest.canonical_bytes()?;
        if manifest_bytes.len() > MAX_MANIFEST_BYTES
            || manifest.files().len() > MAX_GENERATION_FILES
        {
            return Err(GenerationError::InventoryTooLarge);
        }
        let required = required_payloads(manifest)?;
        if required != payloads.keys().cloned().collect() {
            return Err(GenerationError::PayloadMismatch);
        }
        let mut total = 0_u64;
        for (digest, bytes) in payloads {
            total =
                total.checked_add(bytes.len() as u64).ok_or(GenerationError::InventoryTooLarge)?;
            if total > MAX_GENERATION_BYTES || digest_bytes(bytes) != *digest {
                return Err(GenerationError::PayloadMismatch);
            }
        }

        let tx = manifest.transaction_id.as_str();
        let generation = manifest.generation_id.as_str();
        let index = CatalogLink {
            schema_version: 1,
            transaction_id: manifest.transaction_id.clone(),
            generation_id: manifest.generation_id.clone(),
            manifest_sha256: manifest
                .manifest_sha256
                .clone()
                .ok_or(GenerationError::NotMutationReady)?,
        }
        .canonical_bytes()?;
        let catalog_name = format!("{tx}.json");
        match read_file(&self.catalog, &catalog_name, 256) {
            Ok(existing) => {
                if existing != index {
                    return Err(GenerationError::GenerationCollision);
                }
                let verified = self.verify_transaction(&manifest.transaction_id)?;
                return if verified.canonical_bytes()? == manifest.canonical_bytes()? {
                    Ok(())
                } else {
                    Err(GenerationError::GenerationCollision)
                };
            }
            Err(GenerationError::Filesystem(Errno::NOENT)) => {}
            Err(error) => return Err(error),
        }
        match open_dir(&self.generations, generation) {
            Ok(existing) => {
                verify_directory(&existing, Some(manifest), false)?;
            }
            Err(Errno::NOENT) => {
                let stage_name = format!("{tx}.stage");
                let stage = match open_dir(&self.staging, &stage_name) {
                    Ok(existing_stage) => {
                        verify_directory(&existing_stage, Some(manifest), false)?;
                        existing_stage
                    }
                    Err(Errno::NOENT) => {
                        let stage = create_directory(&self.staging, &stage_name)?;
                        let payload_dir = create_directory(&stage, "payloads")?;
                        write_new_file(&stage, "manifest.json", &manifest_bytes)?;
                        for (digest, bytes) in payloads {
                            write_new_file(
                                &payload_dir,
                                &format!("{}.blob", digest.as_str()),
                                bytes,
                            )?;
                        }
                        fsync(&payload_dir)?;
                        fsync(&stage)?;
                        verify_directory(&stage, Some(manifest), false)?;
                        stage
                    }
                    Err(error) => return Err(error.into()),
                };
                renameat_with(
                    &self.staging,
                    stage_name.as_str(),
                    &self.generations,
                    generation,
                    RenameFlags::NOREPLACE,
                )?;
                let _ = stage;
            }
            Err(error) => return Err(error.into()),
        }
        let generation_dir = open_dir(&self.generations, generation)?;
        verify_directory(&generation_dir, Some(manifest), false)?;
        let payload_dir = open_dir(&generation_dir, "payloads")?;
        seal_directory(&payload_dir, 0o500)?;
        fsync(&payload_dir)?;
        seal_directory(&generation_dir, 0o500)?;
        fsync(&generation_dir)?;
        verify_directory(&generation_dir, Some(manifest), true)?;
        fsync(&self.staging)?;
        fsync(&self.generations)?;

        let temp_name = format!("{}.tmp", manifest.transaction_id.as_str());
        // Never replace an existing transaction index. Reuse is accepted only
        // when the existing exact link bytes are equal.
        match write_new_file(&self.catalog, &temp_name, &index) {
            Ok(()) => {}
            Err(GenerationError::Filesystem(Errno::EXIST)) => {
                if read_file(&self.catalog, &temp_name, 256)? != index {
                    return Err(GenerationError::GenerationCollision);
                }
            }
            Err(error) => return Err(error),
        }
        if let Err(error) = renameat_with(
            &self.catalog,
            temp_name.as_str(),
            &self.catalog,
            catalog_name.as_str(),
            RenameFlags::NOREPLACE,
        ) {
            return Err(error.into());
        }
        fsync(&self.catalog)?;
        Ok(())
    }

    /// Verifies an already published generation directly from its directory.
    pub fn verify(&self, generation_id: &Identifier) -> Result<RecoveryManifest, GenerationError> {
        let dir = open_dir(&self.generations, generation_id.as_str())?;
        let manifest = verify_directory(&dir, None, true)?;
        if &manifest.generation_id != generation_id {
            return Err(GenerationError::IncompletePublication);
        }
        Ok(manifest)
    }

    /// Verifies the transaction catalog link and the complete linked
    /// generation. This remains a data-integrity check, not authorization.
    pub fn verify_transaction(
        &self,
        transaction_id: &Identifier,
    ) -> Result<RecoveryManifest, GenerationError> {
        let link_bytes =
            read_file(&self.catalog, &format!("{}.json", transaction_id.as_str()), 4096)?;
        let link: CatalogLink = serde_json::from_slice(&link_bytes)
            .map_err(|error| GenerationError::Io(std::io::Error::other(error)))?;
        if link.schema_version != 1
            || &link.transaction_id != transaction_id
            || link.canonical_bytes()? != link_bytes
        {
            return Err(GenerationError::IncompletePublication);
        }
        let manifest = self.verify(&link.generation_id)?;
        if &manifest.transaction_id != transaction_id
            || manifest.manifest_sha256.as_ref() != Some(&link.manifest_sha256)
        {
            return Err(GenerationError::IncompletePublication);
        }
        Ok(manifest)
    }
}

fn required_payloads(manifest: &RecoveryManifest) -> Result<BTreeSet<Sha256>, GenerationError> {
    let mut result = BTreeSet::new();
    for entry in manifest.files() {
        if let EntryKind::RegularFile { size, sha256, .. } = &entry.kind {
            if *size > MAX_GENERATION_FILE_BYTES {
                return Err(GenerationError::InventoryTooLarge);
            }
            result.insert(sha256.clone());
        }
        if let EntryKind::Symlink { target, .. } = &entry.kind
            && (target.starts_with('/') || target.split('/').any(|component| component == ".."))
        {
            return Err(GenerationError::UnsafeSymlinkTarget);
        }
    }
    Ok(result)
}

fn verify_directory(
    dir: &File,
    expected: Option<&RecoveryManifest>,
    require_sealed: bool,
) -> Result<RecoveryManifest, GenerationError> {
    if require_sealed {
        require_sealed_directory(dir)?;
    }
    let manifest_bytes = read_file(dir, "manifest.json", MAX_MANIFEST_BYTES as u64)?;
    let manifest = RecoveryManifest::parse_verified_json(&manifest_bytes)?;
    if !manifest.is_mutation_ready() || required_payloads(&manifest)?.len() > MAX_GENERATION_FILES {
        return Err(GenerationError::IncompletePublication);
    }
    if let Some(expected) = expected
        && manifest.canonical_bytes()? != expected.canonical_bytes()?
    {
        return Err(GenerationError::GenerationCollision);
    }
    let payload_dir = open_dir(dir, "payloads")?;
    if require_sealed {
        require_sealed_directory(&payload_dir)?;
    }
    let required = required_payloads(&manifest)?;
    ensure_exact_entries(
        dir,
        ["manifest.json", "payloads"].into_iter().map(str::to_owned).collect(),
    )?;
    ensure_exact_entries(
        &payload_dir,
        required.iter().map(|digest| format!("{}.blob", digest.as_str())).collect(),
    )?;
    let mut total = 0_u64;
    for digest in required {
        let bytes =
            read_file(&payload_dir, &format!("{}.blob", digest.as_str()), MAX_GENERATION_BYTES)?;
        total = total.checked_add(bytes.len() as u64).ok_or(GenerationError::InventoryTooLarge)?;
        if total > MAX_GENERATION_BYTES || digest_bytes(&bytes) != digest {
            return Err(GenerationError::IncompletePublication);
        }
        let expected_sizes: Vec<_> = manifest
            .files()
            .iter()
            .filter_map(|record| match &record.kind {
                EntryKind::RegularFile { size, sha256, .. } if *sha256 == digest => Some(*size),
                _ => None,
            })
            .collect();
        if expected_sizes.is_empty()
            || expected_sizes.iter().any(|size| *size != bytes.len() as u64)
        {
            return Err(GenerationError::IncompletePublication);
        }
    }
    Ok(manifest)
}

fn digest_bytes(bytes: &[u8]) -> Sha256 {
    let digest = Sha256Digest::digest(bytes);
    Sha256::new(digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>())
        .expect("SHA-256 is canonical")
}

fn directory(parent: &File, name: &str, expected_uid: u32) -> Result<File, GenerationError> {
    match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(Errno::EXIST) => {
            let dir = open_dir(parent, name)?;
            let stat = rustix::fs::fstat(&dir)?;
            if stat.st_uid != expected_uid || stat.st_mode & 0o7777 != 0o700 {
                return Err(GenerationError::InvalidStore);
            }
            Ok(dir)
        }
        Err(error) => Err(error.into()),
    }
}

fn create_directory(parent: &File, name: &str) -> Result<File, Errno> {
    mkdirat(parent, name, Mode::from_raw_mode(0o700))?;
    open_dir(parent, name)
}

fn open_dir(parent: &File, name: &str) -> Result<File, Errno> {
    let fd = openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )?;
    Ok(File::from(fd))
}

fn write_new_file(parent: &File, name: &str, bytes: &[u8]) -> Result<(), GenerationError> {
    let fd = openat(
        parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )?;
    let mut file = File::from(fd);
    file.write_all(bytes)?;
    fchmod(&file, Mode::from_raw_mode(0o400))?;
    file.sync_all()?;
    Ok(())
}

fn read_file(parent: &File, name: &str, limit: u64) -> Result<Vec<u8>, GenerationError> {
    let fd = openat2(
        parent,
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
        || stat.st_mode & 0o7777 != 0o400
    {
        return Err(GenerationError::InvalidEntryType);
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(GenerationError::InventoryTooLarge);
    }
    Ok(bytes)
}

fn is_directory(file: &File) -> Result<bool, GenerationError> {
    use rustix::fs::{FileType, fstat};
    let stat = fstat(file).map_err(GenerationError::Filesystem)?;
    Ok(FileType::from_raw_mode(stat.st_mode) == FileType::Directory)
}

fn duplicate(file: &File) -> Result<File, GenerationError> {
    rustix::io::dup(file).map(File::from).map_err(GenerationError::Filesystem)
}

fn seal_directory(file: &File, permissions: u32) -> Result<(), GenerationError> {
    fchmod(file, Mode::from_raw_mode(permissions))?;
    Ok(())
}

fn require_sealed_directory(file: &File) -> Result<(), GenerationError> {
    let stat = rustix::fs::fstat(file)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
        || stat.st_mode & 0o7777 != 0o500
    {
        return Err(GenerationError::InvalidEntryType);
    }
    Ok(())
}

fn ensure_exact_entries(dir: &File, mut expected: BTreeSet<String>) -> Result<(), GenerationError> {
    let mut entries = Dir::read_from(dir)?;
    for entry in &mut entries {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let name = name.to_str().map_err(|_| GenerationError::IncompletePublication)?;
        if !expected.remove(name) {
            return Err(GenerationError::IncompletePublication);
        }
    }
    if expected.is_empty() { Ok(()) } else { Err(GenerationError::IncompletePublication) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EntryMetadata, ExtendedMetadata, FileRecord, OperationReason, RelativePath, SnapshotLink,
        StateRoot, VerificationStatus,
    };
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_root() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ryoku-generation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn test_store(root: &File) -> GenerationStore {
        let owner = rustix::fs::fstat(root).unwrap().st_uid;
        GenerationStore::open_owned(root, owner).unwrap()
    }

    fn ready_manifest(content: &[u8]) -> RecoveryManifest {
        let mut manifest = RecoveryManifest::new(
            Identifier::new("txn-generation-1").unwrap(),
            Identifier::new("gen-generation-1").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            "2026-01-01T00:00:00Z",
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
                            mtime_unix_ns: 1_765_000_000_123_456_789,
                            acl_sha256: None,
                            extended: ExtendedMetadata::default(),
                        },
                        size: content.len() as u64,
                        sha256: digest_bytes(content),
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

    #[test]
    fn publication_is_deterministic_complete_and_idempotent() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let content = b"customized profile";
        let manifest = ready_manifest(content);
        let payloads = BTreeMap::from([(digest_bytes(content), content.to_vec())]);
        store.publish(&manifest, &payloads).unwrap();
        assert_eq!(store.verify(&manifest.generation_id).unwrap(), manifest);
        assert_eq!(store.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
        store.publish(&manifest, &payloads).unwrap();
        let manifest_bytes =
            fs::read(root_path.join("generations/gen-generation-1/manifest.json")).unwrap();
        assert_eq!(manifest_bytes, manifest.canonical_bytes().unwrap());
        let catalog = fs::read(root_path.join("catalog/txn-generation-1.json")).unwrap();
        let catalog: CatalogLink = serde_json::from_slice(&catalog).unwrap();
        assert_eq!(catalog.generation_id, manifest.generation_id);
        fs::set_permissions(
            root_path.join("generations/gen-generation-1"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(
            root_path.join("generations/gen-generation-1/payloads"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn safe_symlink_directory_and_absence_records_round_trip() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let mut manifest = RecoveryManifest::new(
            Identifier::new("txn-layer-set-1").unwrap(),
            Identifier::new("gen-layer-set-1").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        let metadata = EntryMetadata {
            mode: 0o700,
            uid: 1000,
            gid: 1000,
            mtime_unix_ns: 1,
            acl_sha256: None,
            extended: ExtendedMetadata::default(),
        };
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::EtcRyoku,
                    RelativePath::new("settings").unwrap(),
                    EntryKind::Directory { metadata: metadata.clone() },
                )
                .unwrap(),
            )
            .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("new.json").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("theme-current").unwrap(),
                    EntryKind::Symlink { metadata, target: "themes/neutral.toml".into() },
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
        store.publish(&manifest, &Payloads::new()).unwrap();
        assert_eq!(store.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
        fs::set_permissions(
            root_path.join("generations/gen-layer-set-1"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(
            root_path.join("generations/gen-layer-set-1/payloads"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn corrupted_payload_makes_generation_unverifiable() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let content = b"before";
        let manifest = ready_manifest(content);
        store
            .publish(&manifest, &BTreeMap::from([(digest_bytes(content), content.to_vec())]))
            .unwrap();
        let payload = root_path.join(format!(
            "generations/gen-generation-1/payloads/{}.blob",
            digest_bytes(content).as_str()
        ));
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&payload, b"after").unwrap();
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(matches!(
            store.verify(&manifest.generation_id),
            Err(GenerationError::IncompletePublication)
        ));
        assert!(matches!(
            store.publish(&manifest, &BTreeMap::from([(digest_bytes(content), content.to_vec())])),
            Err(GenerationError::IncompletePublication)
        ));
        fs::set_permissions(
            root_path.join("generations/gen-generation-1"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(
            root_path.join("generations/gen-generation-1/payloads"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn extra_or_wrong_payloads_are_rejected_before_publication() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let manifest = ready_manifest(b"content");
        let wrong = BTreeMap::from([(digest_bytes(b"other"), b"other".to_vec())]);
        assert!(matches!(store.publish(&manifest, &wrong), Err(GenerationError::PayloadMismatch)));
        assert!(!root_path.join("generations/gen-generation-1").exists());
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn pending_or_interrupted_generations_never_verify() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let content = b"content";
        let mut manifest = ready_manifest(content);
        manifest.set_verification(
            VerificationStatus::Pending,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        let payloads = BTreeMap::from([(digest_bytes(content), content.to_vec())]);
        assert!(matches!(
            store.publish(&manifest, &payloads),
            Err(GenerationError::NotMutationReady)
        ));

        fs::create_dir(root_path.join("staging/txn-generation-1.stage")).unwrap();
        assert!(matches!(
            store.verify_transaction(&Identifier::new("txn-generation-1").unwrap()),
            Err(GenerationError::Filesystem(Errno::NOENT))
        ));
        assert!(matches!(
            store.verify(&Identifier::new("gen-generation-1").unwrap()),
            Err(GenerationError::Filesystem(Errno::NOENT))
        ));
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn extra_stage_and_payload_entries_are_rejected() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let content = b"content";
        let manifest = ready_manifest(content);
        let digest = digest_bytes(content);
        let stage = create_directory(&store.staging, "txn-generation-1.stage").unwrap();
        let payload_dir = create_directory(&stage, "payloads").unwrap();
        write_new_file(&stage, "manifest.json", &manifest.canonical_bytes().unwrap()).unwrap();
        write_new_file(&stage, "extra", b"unlisted").unwrap();
        write_new_file(&payload_dir, &format!("{}.blob", digest.as_str()), content).unwrap();
        assert!(matches!(
            store.publish(&manifest, &BTreeMap::from([(digest.clone(), content.to_vec())])),
            Err(GenerationError::IncompletePublication)
        ));

        fs::remove_file(root_path.join("staging/txn-generation-1.stage/extra")).unwrap();
        write_new_file(&payload_dir, "extra.blob", b"unlisted").unwrap();
        assert!(matches!(
            store.publish(&manifest, &BTreeMap::from([(digest, content.to_vec())])),
            Err(GenerationError::IncompletePublication)
        ));
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn store_requires_expected_owner_and_private_subdirectory_modes() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let owner = rustix::fs::fstat(&root).unwrap().st_uid;
        assert!(matches!(
            GenerationStore::open_owned(&root, owner.wrapping_add(1)),
            Err(GenerationError::InvalidStore)
        ));
        fs::set_permissions(&root_path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            GenerationStore::open_owned(&root, owner),
            Err(GenerationError::InvalidStore)
        ));
        fs::set_permissions(&root_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root_path.join("staging")).unwrap();
        fs::set_permissions(root_path.join("staging"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            GenerationStore::open_owned(&root, owner),
            Err(GenerationError::InvalidStore)
        ));
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn external_symlink_targets_are_rejected_without_following_them() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let store = test_store(&root);
        let mut manifest;
        manifest = RecoveryManifest::new(
            Identifier::new("txn-symlink-1").unwrap(),
            Identifier::new("gen-symlink-1").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("link").unwrap(),
                    EntryKind::Symlink {
                        metadata: EntryMetadata {
                            mode: 0o777,
                            uid: 1000,
                            gid: 1000,
                            mtime_unix_ns: 0,
                            acl_sha256: None,
                            extended: ExtendedMetadata::default(),
                        },
                        target: "../../outside".into(),
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
        assert!(matches!(
            store.publish(&manifest, &Payloads::new()),
            Err(GenerationError::UnsafeSymlinkTarget)
        ));
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn store_subdirectories_cannot_be_symlinked_outside_root() {
        let root_path = temp_root();
        let outside = temp_root();
        symlink(&outside, root_path.join("staging")).unwrap();
        let root = File::open(&root_path).unwrap();
        let owner = rustix::fs::fstat(&root).unwrap().st_uid;
        assert!(matches!(
            GenerationStore::open_owned(&root, owner),
            Err(GenerationError::Filesystem(_)) | Err(GenerationError::InvalidStore)
        ));
        fs::remove_dir_all(root_path).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
