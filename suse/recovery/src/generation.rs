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
        Dir, Mode, OFlags, RenameFlags, ResolveFlags, fchmod, fstatvfs, fsync, mkdirat, openat,
        openat2, renameat_with,
    },
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Digest};

use crate::machine_state::{MachineStateError, read_machine_state_record};
use crate::metadata::{AclPayload, MAX_METADATA_VALUE_BYTES, entry_metadata};
use crate::state_mount::{StateMountError, StateMountIdentity, collect_and_open_state_mount};
use crate::{EntryKind, Identifier, MAX_MANIFEST_BYTES, ManifestError, RecoveryManifest, Sha256};

const MAX_GENERATION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GENERATION_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_GENERATION_FILES: usize = 256;
const MAX_GENERATION_PAYLOADS: usize = MAX_GENERATION_FILES * 3;
const GENERATION_AUXILIARY_BLOCKS: u64 = 4;

/// Immutable copy inputs: regular-file bytes, canonical ACL bundles, and raw
/// allowlisted xattr values. Keys must equal all distinct manifest-referenced
/// digests; repeated identical content is deduplicated. Raw metadata stays in
/// this protected store rather than in public manifest fields.
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
    InsufficientSpace { required_bytes: u64, available_bytes: u64 },
    CapacityOverflow,
    Manifest(ManifestError),
    Filesystem(Errno),
    Io(std::io::Error),
    Validation(crate::ValidationError),
    MachineState(MachineStateError),
    StateMount(StateMountError),
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
            Self::InsufficientSpace { required_bytes, available_bytes } => write!(
                f,
                "recovery generation needs {required_bytes} bytes but only {available_bytes} are available"
            ),
            Self::CapacityOverflow => {
                f.write_str("recovery generation capacity calculation overflowed")
            }
            Self::Manifest(error) => write!(f, "manifest validation failed: {error}"),
            Self::Filesystem(error) => write!(f, "generation filesystem operation failed: {error}"),
            Self::Io(error) => write!(f, "generation payload I/O failed: {error}"),
            Self::Validation(error) => write!(f, "manifest validation failed: {error:?}"),
            Self::MachineState(error) => {
                write!(f, "trusted machine identity is unavailable: {error}")
            }
            Self::StateMount(error) => write!(f, "protected state mount is unavailable: {error}"),
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

impl From<StateMountError> for GenerationError {
    fn from(value: StateMountError) -> Self {
        Self::StateMount(value)
    }
}
impl From<MachineStateError> for GenerationError {
    fn from(value: MachineStateError) -> Self {
        Self::MachineState(value)
    }
}
impl From<crate::ValidationError> for GenerationError {
    fn from(value: crate::ValidationError) -> Self {
        Self::Validation(value)
    }
}

/// Publish immutable generations below the fixed, validated state subvolume.
/// Production construction validates that mount before opening store paths.
pub struct GenerationStore {
    staging: File,
    generations: File,
    catalog: File,
}

/// Read-only access to an existing generation store. Unlike [`GenerationStore`],
/// opening this view never creates directories, synchronizes metadata, or writes
/// recovery state. It validates the directory owner/modes but does not prove
/// the protected mount identity or authorize restore.
pub struct GenerationReader {
    generations: File,
    catalog: File,
}

/// A fully verified generation with its payload directory pinned by descriptor.
/// This is byte-integrity evidence, not authorization or compatibility proof.
pub struct VerifiedGeneration {
    manifest: RecoveryManifest,
    payloads: File,
}

impl VerifiedGeneration {
    pub fn manifest(&self) -> &RecoveryManifest {
        &self.manifest
    }

    /// Reads only a digest referenced by this generation. Rechecks its hash
    /// and shape on every read, even after the initial full verification.
    pub fn read_payload(&self, digest: &Sha256) -> Result<Vec<u8>, GenerationError> {
        if !required_payloads(&self.manifest)?.contains(digest) {
            return Err(GenerationError::PayloadMismatch);
        }
        let bytes = read_file(
            &self.payloads,
            &format!("{}.blob", digest.as_str()),
            MAX_GENERATION_FILE_BYTES,
        )?;
        if digest_bytes(&bytes) != *digest || !valid_payload_shape(&self.manifest, digest, &bytes) {
            return Err(GenerationError::IncompletePublication);
        }
        Ok(bytes)
    }
}

impl GenerationReader {
    /// Opens the fixed protected state mount after reading the installed
    /// machine identity from the root-owned record and matching live Btrfs
    /// evidence. This is the supported entry point for machine-local reads.
    pub fn open_state_mount() -> Result<Self, GenerationError> {
        let process_root = rustix::fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Self::open_state_mount_from(&File::from(process_root))
    }

    fn open_state_mount_from(root: &File) -> Result<Self, GenerationError> {
        let expected = read_machine_state_record(root)?;
        let (state_fd, _evidence) = collect_and_open_state_mount(root, expected.identity())?;
        let state_root = File::from(state_fd);
        Self::open_existing(&state_root, 0)
    }

    /// Opens an existing store without write-capable operations. The caller
    /// supplies the expected directory owner; offline verification alone is
    /// not an authorization signal for destructive recovery.
    pub fn open_existing(root: &File, expected_uid: u32) -> Result<Self, GenerationError> {
        let root_stat = rustix::fs::fstat(root)?;
        if rustix::fs::FileType::from_raw_mode(root_stat.st_mode) != rustix::fs::FileType::Directory
            || root_stat.st_uid != expected_uid
            || root_stat.st_mode & 0o7777 != 0o700
        {
            return Err(GenerationError::InvalidStore);
        }
        let root = duplicate(root)?;
        let staging = existing_directory(&root, "staging", expected_uid)?;
        let generations = existing_directory(&root, "generations", expected_uid)?;
        let catalog = existing_directory(&root, "catalog", expected_uid)?;
        drop(staging);
        Ok(Self { generations, catalog })
    }

    /// Lists committed transaction catalog names in stable order. Each ID is
    /// validated, but callers should use [`Self::verify_transaction`] before
    /// treating an entry as intact.
    pub fn list_transactions(&self) -> Result<Vec<Identifier>, GenerationError> {
        let mut entries = Dir::read_from(&self.catalog)?;
        let mut transactions = Vec::new();
        for entry in &mut entries {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let name = name.to_str().map_err(|_| GenerationError::IncompletePublication)?;
            if !name.ends_with(".json") {
                // Incomplete `.tmp` catalog publications are intentionally not
                // listed as committed transactions.
                continue;
            }
            let transaction =
                name.strip_suffix(".json").ok_or(GenerationError::IncompletePublication)?;
            transactions.push(Identifier::new(transaction).map_err(GenerationError::Validation)?);
        }
        transactions.sort();
        Ok(transactions)
    }

    /// Verifies the canonical transaction link, manifest seal, and every
    /// generation payload from the existing store.
    pub fn verify_transaction(
        &self,
        transaction_id: &Identifier,
    ) -> Result<RecoveryManifest, GenerationError> {
        verify_transaction_from(&self.catalog, &self.generations, transaction_id)
    }

    pub fn open_verified_transaction(
        &self,
        transaction_id: &Identifier,
    ) -> Result<VerifiedGeneration, GenerationError> {
        open_verified_transaction_from(&self.catalog, &self.generations, transaction_id)
    }
}

impl GenerationStore {
    /// Opens the store only after collecting and validating the fixed
    /// `/var/lib/ryoku-state` Btrfs mount against the installed machine
    /// identity. Callers must obtain `expected` from trusted installation
    /// state. There is intentionally no public constructor from an arbitrary
    /// directory descriptor.
    pub fn open_state_mount(expected: &StateMountIdentity) -> Result<Self, GenerationError> {
        let process_root = rustix::fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let (state_fd, _evidence) = collect_and_open_state_mount(&process_root, expected)?;
        let state_root = File::from(state_fd);
        Self::open_owned(&state_root, 0)
    }

    /// Fail closed unless the generation's payload, bounded manifest and
    /// per-file allocation overhead fit on the filesystem backing `staging`.
    /// This is a preflight estimate, not a reservation: callers must still
    /// treat any later write or synchronization failure as a failed checkpoint.
    pub fn ensure_available_space(
        &self,
        payload_bytes: u64,
        payload_file_count: usize,
    ) -> Result<(), GenerationError> {
        if payload_bytes > MAX_GENERATION_BYTES || payload_file_count > MAX_GENERATION_PAYLOADS {
            return Err(GenerationError::InventoryTooLarge);
        }
        let stats = fstatvfs(&self.staging)?;
        let block_size = if stats.f_frsize != 0 { stats.f_frsize } else { stats.f_bsize };
        if block_size == 0 {
            return Err(GenerationError::CapacityOverflow);
        }
        let allocation_blocks = u64::try_from(payload_file_count)
            .map_err(|_| GenerationError::CapacityOverflow)?
            .checked_add(GENERATION_AUXILIARY_BLOCKS)
            .ok_or(GenerationError::CapacityOverflow)?;
        let allocation_overhead =
            allocation_blocks.checked_mul(block_size).ok_or(GenerationError::CapacityOverflow)?;
        let required_bytes = payload_bytes
            .checked_add(MAX_MANIFEST_BYTES as u64)
            .and_then(|value| value.checked_add(allocation_overhead))
            .ok_or(GenerationError::CapacityOverflow)?;
        let available_bytes =
            stats.f_bavail.checked_mul(block_size).ok_or(GenerationError::CapacityOverflow)?;
        if available_bytes < required_bytes {
            return Err(GenerationError::InsufficientSpace { required_bytes, available_bytes });
        }
        Ok(())
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

    #[cfg(test)]
    pub(crate) fn open_for_test(root: &File) -> Result<Self, GenerationError> {
        let owner = rustix::fs::fstat(root)?.st_uid;
        Self::open_owned(root, owner)
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
            if total > MAX_GENERATION_BYTES
                || digest_bytes(bytes) != *digest
                || !valid_payload_shape(manifest, digest, bytes)
            {
                return Err(GenerationError::PayloadMismatch);
            }
        }
        self.ensure_available_space(total, payloads.len())?;

        let tx = manifest.transaction_id.as_str();
        let generation = manifest.generation_id.as_str();
        let index = CatalogLink {
            schema_version: catalog_schema(manifest),
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
        verify_generation_from(&self.generations, generation_id)
    }

    /// Verifies the transaction catalog link and the complete linked
    /// generation. This remains a data-integrity check, not authorization.
    pub fn verify_transaction(
        &self,
        transaction_id: &Identifier,
    ) -> Result<RecoveryManifest, GenerationError> {
        verify_transaction_from(&self.catalog, &self.generations, transaction_id)
    }

    pub fn open_verified_transaction(
        &self,
        transaction_id: &Identifier,
    ) -> Result<VerifiedGeneration, GenerationError> {
        open_verified_transaction_from(&self.catalog, &self.generations, transaction_id)
    }
}

fn open_verified_transaction_from(
    catalog: &File,
    generations: &File,
    transaction_id: &Identifier,
) -> Result<VerifiedGeneration, GenerationError> {
    let expected = verify_transaction_from(catalog, generations, transaction_id)?;
    let directory = open_dir(generations, expected.generation_id.as_str())?;
    // Verify the pinned directory rather than relying on the previous path
    // lookup. Any replacement with another manifest fails before return.
    let manifest = verify_directory(&directory, Some(&expected), true)?;
    let payloads = open_dir(&directory, "payloads")?;
    require_sealed_directory(&payloads)?;
    Ok(VerifiedGeneration { manifest, payloads })
}

fn verify_generation_from(
    generations: &File,
    generation_id: &Identifier,
) -> Result<RecoveryManifest, GenerationError> {
    let dir = open_dir(generations, generation_id.as_str())?;
    let manifest = verify_directory(&dir, None, true)?;
    if &manifest.generation_id != generation_id {
        return Err(GenerationError::IncompletePublication);
    }
    Ok(manifest)
}

fn verify_transaction_from(
    catalog: &File,
    generations: &File,
    transaction_id: &Identifier,
) -> Result<RecoveryManifest, GenerationError> {
    let link_bytes = read_file(catalog, &format!("{}.json", transaction_id.as_str()), 4096)?;
    let link: CatalogLink = serde_json::from_slice(&link_bytes)
        .map_err(|error| GenerationError::Io(std::io::Error::other(error)))?;
    if !matches!(link.schema_version, 1 | 2)
        || &link.transaction_id != transaction_id
        || link.canonical_bytes()? != link_bytes
    {
        return Err(GenerationError::IncompletePublication);
    }
    let manifest = verify_generation_from(generations, &link.generation_id)?;
    if &manifest.transaction_id != transaction_id
        || manifest.manifest_sha256.as_ref() != Some(&link.manifest_sha256)
        || link.schema_version != catalog_schema(&manifest)
    {
        return Err(GenerationError::IncompletePublication);
    }
    Ok(manifest)
}

// Metadata-bearing generations require a richer payload contract. Older
// readers reject catalog v2 before interpreting the new generation format;
// content-only generations remain compatible with the v1 catalog contract.
fn catalog_schema(manifest: &RecoveryManifest) -> u16 {
    if manifest.files().iter().any(|record| {
        entry_metadata(&record.kind).is_some_and(|metadata| {
            metadata.acl_sha256.is_some() || !metadata.extended.values.is_empty()
        })
    }) {
        2
    } else {
        1
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
        if let Some(metadata) = entry_metadata(&entry.kind) {
            if let Some(digest) = &metadata.acl_sha256 {
                result.insert(digest.clone());
            }
            result.extend(metadata.extended.values.values().cloned());
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
    if !manifest.is_mutation_ready()
        || manifest.files().len() > MAX_GENERATION_FILES
        || required_payloads(&manifest)?.len() > MAX_GENERATION_PAYLOADS
    {
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
        let bytes = read_file(
            &payload_dir,
            &format!("{}.blob", digest.as_str()),
            MAX_GENERATION_FILE_BYTES,
        )?;
        total = total.checked_add(bytes.len() as u64).ok_or(GenerationError::InventoryTooLarge)?;
        if total > MAX_GENERATION_BYTES || digest_bytes(&bytes) != digest {
            return Err(GenerationError::IncompletePublication);
        }
        if !valid_payload_shape(&manifest, &digest, &bytes) {
            return Err(GenerationError::IncompletePublication);
        }
    }
    Ok(manifest)
}

fn valid_payload_shape(manifest: &RecoveryManifest, digest: &Sha256, bytes: &[u8]) -> bool {
    let mut referenced = false;
    for record in manifest.files() {
        if let EntryKind::RegularFile { size, sha256, .. } = &record.kind
            && sha256 == digest
        {
            referenced = true;
            if *size != bytes.len() as u64 {
                return false;
            }
        }
        if let Some(metadata) = entry_metadata(&record.kind) {
            if metadata.acl_sha256.as_ref() == Some(digest) {
                referenced = true;
                if AclPayload::decode(bytes).is_err() {
                    return false;
                }
            }
            if metadata.extended.values.values().any(|value| value == digest) {
                referenced = true;
                if bytes.len() > MAX_METADATA_VALUE_BYTES {
                    return false;
                }
            }
        }
    }
    referenced
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

fn existing_directory(
    parent: &File,
    name: &str,
    expected_uid: u32,
) -> Result<File, GenerationError> {
    let dir = open_dir(parent, name)?;
    let stat = rustix::fs::fstat(&dir)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
        || stat.st_uid != expected_uid
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(GenerationError::InvalidStore);
    }
    Ok(dir)
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

    #[test]
    fn production_store_open_fails_closed_without_the_expected_state_mount() {
        let expected = StateMountIdentity {
            filesystem_uuid: "00000000-0000-0000-0000-000000000000".into(),
            subvolume_id: u64::MAX,
            parent_subvolume_id: 5,
        };
        assert!(matches!(
            GenerationStore::open_state_mount(&expected),
            Err(GenerationError::StateMount(_))
        ));
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
        let reader =
            GenerationReader::open_existing(&root, rustix::fs::fstat(&root).unwrap().st_uid)
                .unwrap();
        assert_eq!(reader.list_transactions().unwrap(), vec![manifest.transaction_id.clone()]);
        assert_eq!(reader.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
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
    fn verified_payload_handle_refuses_unlisted_digests_and_rechecks_content() {
        let root_path = temp_root();
        let store = test_store(&File::open(&root_path).unwrap());
        let content = b"original content";
        let manifest = ready_manifest(content);
        let digest = digest_bytes(content);
        store.publish(&manifest, &BTreeMap::from([(digest.clone(), content.to_vec())])).unwrap();
        let generation = store.open_verified_transaction(&manifest.transaction_id).unwrap();
        assert_eq!(generation.manifest(), &manifest);
        assert_eq!(generation.read_payload(&digest).unwrap(), content);
        assert!(matches!(
            generation.read_payload(&digest_bytes(b"unlisted")),
            Err(GenerationError::PayloadMismatch)
        ));
        let path = root_path
            .join(format!("generations/gen-generation-1/payloads/{}.blob", digest.as_str()));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, b"tampered content").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(generation.read_payload(&digest).is_err());
    }

    #[test]
    fn verified_payload_handle_is_bound_to_its_pinned_directory() {
        let root_path = temp_root();
        let store = test_store(&File::open(&root_path).unwrap());
        let content = b"pinned content";
        let manifest = ready_manifest(content);
        let digest = digest_bytes(content);
        store.publish(&manifest, &BTreeMap::from([(digest.clone(), content.to_vec())])).unwrap();
        let root = File::open(&root_path).unwrap();
        let reader =
            GenerationReader::open_existing(&root, rustix::fs::fstat(&root).unwrap().st_uid)
                .unwrap();
        let generation = reader.open_verified_transaction(&manifest.transaction_id).unwrap();
        let original = root_path.join("generations/gen-generation-1");
        fs::rename(&original, root_path.join("generations/retained-generation")).unwrap();
        fs::create_dir(&original).unwrap();
        fs::create_dir(original.join("payloads")).unwrap();
        fs::write(original.join(format!("payloads/{}.blob", digest.as_str())), b"replacement")
            .unwrap();
        assert_eq!(generation.read_payload(&digest).unwrap(), content);
        assert!(reader.open_verified_transaction(&manifest.transaction_id).is_err());
    }

    #[test]
    fn verified_payload_handle_does_not_follow_replaced_blob_symlinks() {
        let root_path = temp_root();
        let store = test_store(&File::open(&root_path).unwrap());
        let content = b"original";
        let manifest = ready_manifest(content);
        let digest = digest_bytes(content);
        store.publish(&manifest, &BTreeMap::from([(digest.clone(), content.to_vec())])).unwrap();
        let generation = store.open_verified_transaction(&manifest.transaction_id).unwrap();
        let directory = root_path.join("generations/gen-generation-1/payloads");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join(format!("{}.blob", digest.as_str()));
        let outside = root_path.join("unrelated");
        fs::write(&outside, b"unrelated sentinel").unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(generation.read_payload(&digest).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"unrelated sentinel");
    }

    #[test]
    fn metadata_blobs_are_mandatory_private_and_verified_on_readback() {
        let root_path = temp_root();
        let store = test_store(&File::open(&root_path).unwrap());
        let content = b"customized profile";
        let acl = crate::metadata::AclPayload {
            access: Some(vec![2, 0, 0, 0]),
            default: Some(vec![2, 0, 0, 0, 1, 0]),
        }
        .encode()
        .unwrap()
        .unwrap();
        let label = b"synthetic_u:object_r:ryoku_t:s0\0";
        let mut manifest = ready_manifest(content);
        let EntryKind::RegularFile { metadata, .. } = &mut manifest.files[0].kind else {
            unreachable!()
        };
        metadata.acl_sha256 = Some(digest_bytes(&acl));
        metadata.extended.values.insert(crate::XattrName::SelinuxLabel, digest_bytes(label));
        manifest.seal().unwrap();
        let mut payloads = BTreeMap::from([(digest_bytes(content), content.to_vec())]);
        assert!(matches!(
            store.publish(&manifest, &payloads),
            Err(GenerationError::PayloadMismatch)
        ));
        payloads.insert(digest_bytes(&acl), acl.clone());
        assert!(matches!(
            store.publish(&manifest, &payloads),
            Err(GenerationError::PayloadMismatch)
        ));
        payloads.insert(digest_bytes(label), label.to_vec());
        store.publish(&manifest, &payloads).unwrap();
        assert_eq!(store.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
        let index: serde_json::Value = serde_json::from_slice(
            &fs::read(root_path.join("catalog/txn-generation-1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(index["schema_version"], 2);
        let index_path = root_path.join("catalog/txn-generation-1.json");
        let mut link: CatalogLink =
            serde_json::from_slice(&fs::read(&index_path).unwrap()).unwrap();
        for incompatible in [0, 1, 3] {
            link.schema_version = incompatible;
            fs::set_permissions(&index_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&index_path, link.canonical_bytes().unwrap()).unwrap();
            fs::set_permissions(&index_path, fs::Permissions::from_mode(0o400)).unwrap();
            assert!(store.verify_transaction(&manifest.transaction_id).is_err());
        }
        link.schema_version = 2;
        fs::set_permissions(&index_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&index_path, link.canonical_bytes().unwrap()).unwrap();
        fs::set_permissions(&index_path, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(store.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
        let serialized = String::from_utf8(manifest.canonical_bytes().unwrap()).unwrap();
        assert!(!serialized.contains("synthetic_u"));
        let label_path = root_path.join(format!(
            "generations/gen-generation-1/payloads/{}.blob",
            digest_bytes(label).as_str()
        ));
        assert_eq!(fs::read(&label_path).unwrap(), label);
        assert_eq!(fs::metadata(&label_path).unwrap().permissions().mode() & 0o777, 0o400);
        fs::set_permissions(&label_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&label_path, b"corrupt").unwrap();
        fs::set_permissions(&label_path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(store.verify_transaction(&manifest.transaction_id).is_err());
    }

    #[test]
    fn valid_hash_does_not_make_an_invalid_acl_encoding_publishable() {
        let root_path = temp_root();
        let store = test_store(&File::open(&root_path).unwrap());
        let content = b"profile";
        let invalid_acl = b"unrecognized ACL encoding";
        let mut manifest = ready_manifest(content);
        let EntryKind::RegularFile { metadata, .. } = &mut manifest.files[0].kind else {
            unreachable!()
        };
        metadata.acl_sha256 = Some(digest_bytes(invalid_acl));
        manifest.seal().unwrap();
        let payloads = BTreeMap::from([
            (digest_bytes(content), content.to_vec()),
            (digest_bytes(invalid_acl), invalid_acl.to_vec()),
        ]);
        assert!(matches!(
            store.publish(&manifest, &payloads),
            Err(GenerationError::PayloadMismatch)
        ));
        assert!(store.verify_transaction(&manifest.transaction_id).is_err());
    }

    #[test]
    fn full_inventory_can_have_more_metadata_blobs_than_declared_paths() {
        let root_path = temp_root();
        let store = test_store(&File::open(&root_path).unwrap());
        let content = b"profile fixture";
        let mut manifest = ready_manifest(content);
        let mut payloads = BTreeMap::from([(digest_bytes(content), content.to_vec())]);
        for index in 0..MAX_GENERATION_FILES {
            let acl =
                AclPayload { access: Some((index as u32).to_le_bytes().to_vec()), default: None }
                    .encode()
                    .unwrap()
                    .unwrap();
            let label = format!("synthetic-label-{index}").into_bytes();
            let mut metadata = EntryMetadata {
                mode: 0o700,
                uid: 1000,
                gid: 1000,
                mtime_unix_ns: 1,
                acl_sha256: Some(digest_bytes(&acl)),
                extended: ExtendedMetadata::default(),
            };
            metadata.extended.values.insert(crate::XattrName::SelinuxLabel, digest_bytes(&label));
            payloads.insert(digest_bytes(&acl), acl);
            payloads.insert(digest_bytes(&label), label);
            if index == 0 {
                let EntryKind::RegularFile { metadata: first, .. } = &mut manifest.files[0].kind
                else {
                    unreachable!()
                };
                *first = metadata;
            } else {
                manifest
                    .add_file(
                        FileRecord::new(
                            StateRoot::UserRyokuConfig,
                            RelativePath::new(format!("directory-{index}")).unwrap(),
                            EntryKind::Directory { metadata },
                        )
                        .unwrap(),
                    )
                    .unwrap();
            }
        }
        manifest.seal().unwrap();
        assert_eq!(payloads.len(), MAX_GENERATION_FILES * 2 + 1);
        store.publish(&manifest, &payloads).unwrap();
        assert_eq!(store.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
    }

    #[test]
    fn read_only_reader_does_not_create_a_missing_store() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        let owner = rustix::fs::fstat(&root).unwrap().st_uid;
        assert!(matches!(
            GenerationReader::open_existing(&root, owner),
            Err(GenerationError::Filesystem(_))
        ));
        assert_eq!(fs::read_dir(&root_path).unwrap().count(), 0);
        fs::remove_dir(root_path).unwrap();
    }

    #[test]
    fn protected_reader_rejects_a_caller_supplied_directory_as_identity_root() {
        let root_path = temp_root();
        let root = File::open(&root_path).unwrap();
        assert!(matches!(
            GenerationReader::open_state_mount_from(&root),
            Err(GenerationError::MachineState(MachineStateError::UnsafeRoot(_)))
        ));
        assert_eq!(fs::read_dir(&root_path).unwrap().count(), 0);
        fs::remove_dir(root_path).unwrap();
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
