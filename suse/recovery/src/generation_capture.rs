//! End-to-end orchestration for one declared, exact-file generation capture.
//!
//! This layer composes the existing collector with immutable generation
//! publication. It does not resolve approved roots, stop writers, authenticate
//! a caller, validate mounts or Snapper state, or authorize a mutation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
};

use rustix::fs::{FileType, OFlags, ResolveFlags, fstat, openat2};
use sha2::{Digest, Sha256 as Sha256Digest};

use crate::{
    EntryKind, Identifier, ManifestError, OperationReason, PackageRevision, ProfileRevision,
    ProviderRevision, RecoveryManifest, RelativePath, Sha256, SnapshotLink, StateRoot,
    ValidationError, VerificationStatus,
    capture::{CaptureError, CaptureLimits, capture_declared},
    generation::{GenerationError, GenerationStore, Payloads},
};

/// One approved root descriptor and its exact, nonrecursive inventory.
pub struct DeclaredRoot<'a> {
    pub root: StateRoot,
    pub descriptor: &'a File,
    pub paths: &'a [RelativePath],
}

/// Caller attestation for an independently verified exact Snapper/filesystem
/// link. This wrapper does not contact Snapper or prove the attestation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSnapshotLink(SnapshotLink);

impl VerifiedSnapshotLink {
    /// Construct only after a trusted caller verified the pre-change snapshot
    /// and matching filesystem UUID against the intended root.
    pub fn from_preverified_link(snapshot: SnapshotLink) -> Self {
        Self(snapshot)
    }
}

/// Inputs needed to capture, seal, publish, and read back one generation.
pub struct GenerationCaptureRequest<'a> {
    pub transaction_id: Identifier,
    pub generation_id: Identifier,
    pub parent_generation_id: Option<Identifier>,
    pub snapshot: VerifiedSnapshotLink,
    pub reason: OperationReason,
    pub created_utc: String,
    pub package_revisions: Vec<PackageRevision>,
    pub provider_revision: Option<ProviderRevision>,
    pub profile_revision: Option<ProfileRevision>,
    pub roots: &'a [DeclaredRoot<'a>],
    pub limits: CaptureLimits,
}

/// A capture failure aborts the entire generation; partial records are never
/// returned as a verified result.
#[derive(Debug)]
pub enum GenerationCaptureError {
    EmptyInventory,
    TooManyRoots,
    DuplicateRoot,
    InventoryTooLarge,
    PayloadMismatch,
    VerificationMismatch,
    Capture(CaptureError),
    Generation(GenerationError),
    Filesystem(rustix::io::Errno),
    Io(std::io::Error),
    Validation(ValidationError),
    Manifest(ManifestError),
}

impl std::fmt::Display for GenerationCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInventory => formatter.write_str("declared inventory is empty"),
            Self::TooManyRoots => formatter.write_str("declared root count exceeds fixed limit"),
            Self::DuplicateRoot => formatter.write_str("declared root is duplicated"),
            Self::InventoryTooLarge => formatter.write_str("declared inventory exceeds limits"),
            Self::PayloadMismatch => {
                formatter.write_str("captured payload does not match its record")
            }
            Self::VerificationMismatch => {
                formatter.write_str("published generation failed read-back verification")
            }
            Self::Capture(error) => write!(formatter, "declared capture failed: {error}"),
            Self::Generation(error) => write!(formatter, "generation publication failed: {error}"),
            Self::Filesystem(error) => write!(formatter, "capture payload open failed: {error}"),
            Self::Io(error) => write!(formatter, "capture payload read failed: {error}"),
            Self::Validation(error) => write!(formatter, "manifest construction failed: {error:?}"),
            Self::Manifest(error) => write!(formatter, "manifest verification failed: {error}"),
        }
    }
}

impl std::error::Error for GenerationCaptureError {}

impl From<CaptureError> for GenerationCaptureError {
    fn from(error: CaptureError) -> Self {
        Self::Capture(error)
    }
}

impl From<GenerationError> for GenerationCaptureError {
    fn from(error: GenerationError) -> Self {
        Self::Generation(error)
    }
}

impl From<rustix::io::Errno> for GenerationCaptureError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Filesystem(error)
    }
}

impl From<std::io::Error> for GenerationCaptureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ValidationError> for GenerationCaptureError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

impl From<ManifestError> for GenerationCaptureError {
    fn from(error: ManifestError) -> Self {
        Self::Manifest(error)
    }
}

/// Capture every declared root, build and seal the manifest, publish the
/// content-addressed generation, then verify it from the store before return.
///
/// The caller must freeze all writers for the declared entries across this
/// operation and must create `snapshot` only after verifying the exact
/// pre-change Snapper/filesystem link. This function performs no authorization
/// and must not itself be treated as permission to mutate a provider.
pub fn capture_publish_verify(
    store: &GenerationStore,
    request: GenerationCaptureRequest<'_>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_publish_verify_using(store, request, capture_declared)
}

#[cfg(test)]
fn capture_publish_verify_without_xattrs_for_test(
    store: &GenerationStore,
    request: GenerationCaptureRequest<'_>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_publish_verify_using(
        store,
        request,
        crate::capture::capture_declared_without_xattrs_for_test,
    )
}

fn capture_publish_verify_using(
    store: &GenerationStore,
    request: GenerationCaptureRequest<'_>,
    capture_files: impl Fn(
        &File,
        StateRoot,
        &[RelativePath],
        CaptureLimits,
    ) -> Result<Vec<crate::FileRecord>, CaptureError>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    validate_inventory(request.roots, request.limits)?;
    let roots = sorted_roots(request.roots)?;

    let mut records = Vec::new();
    for root in &roots {
        records.extend(capture_files(root.descriptor, root.root, root.paths, request.limits)?);
    }
    if records.is_empty() {
        return Err(GenerationCaptureError::EmptyInventory);
    }

    let payloads = collect_payloads(&roots, &records, request.limits)?;

    // A second exact metadata/content capture brackets payload reading. A
    // mismatch means the source set changed after its initial capture.
    let mut confirmed = Vec::with_capacity(records.len());
    for root in &roots {
        confirmed.extend(capture_files(root.descriptor, root.root, root.paths, request.limits)?);
    }
    if confirmed != records {
        return Err(GenerationCaptureError::VerificationMismatch);
    }

    let mut manifest = RecoveryManifest::new(
        request.transaction_id,
        request.generation_id,
        request.parent_generation_id,
        request.snapshot.0,
        request.reason,
        request.created_utc,
    )?;
    for record in records {
        manifest.add_file(record)?;
    }
    manifest.set_provenance(
        request.package_revisions,
        request.provider_revision,
        request.profile_revision,
    )?;
    manifest.set_verification(
        VerificationStatus::Verified,
        VerificationStatus::Verified,
        VerificationStatus::Verified,
    );
    manifest.seal()?;
    if !manifest.is_mutation_ready() {
        return Err(GenerationCaptureError::VerificationMismatch);
    }

    store.publish(&manifest, &payloads)?;
    let stored = store.verify_transaction(&manifest.transaction_id)?;
    if stored.canonical_bytes()? != manifest.canonical_bytes()? {
        return Err(GenerationCaptureError::VerificationMismatch);
    }
    Ok(stored)
}

fn validate_inventory(
    roots: &[DeclaredRoot<'_>],
    limits: CaptureLimits,
) -> Result<(), GenerationCaptureError> {
    if roots.is_empty() {
        return Err(GenerationCaptureError::EmptyInventory);
    }
    if roots.len() > 4 {
        return Err(GenerationCaptureError::TooManyRoots);
    }
    if limits.max_paths == 0 || limits.max_file_bytes == 0 || limits.max_total_bytes == 0 {
        return Err(GenerationCaptureError::InventoryTooLarge);
    }
    let mut unique_roots = BTreeSet::new();
    let mut total_paths = 0usize;
    for root in roots {
        if !unique_roots.insert(root.root) {
            return Err(GenerationCaptureError::DuplicateRoot);
        }
        total_paths = total_paths.saturating_add(root.paths.len());
        if total_paths > limits.max_paths {
            return Err(GenerationCaptureError::InventoryTooLarge);
        }
    }
    if total_paths == 0 {
        return Err(GenerationCaptureError::EmptyInventory);
    }
    Ok(())
}

fn sorted_roots<'a>(
    roots: &'a [DeclaredRoot<'a>],
) -> Result<Vec<&'a DeclaredRoot<'a>>, GenerationCaptureError> {
    let mut sorted: Vec<_> = roots.iter().collect();
    sorted.sort_by_key(|root| root.root);
    if sorted.windows(2).any(|pair| pair[0].root == pair[1].root) {
        return Err(GenerationCaptureError::DuplicateRoot);
    }
    Ok(sorted)
}

fn collect_payloads(
    roots: &[&DeclaredRoot<'_>],
    records: &[crate::FileRecord],
    limits: CaptureLimits,
) -> Result<Payloads, GenerationCaptureError> {
    let descriptors: BTreeMap<_, _> =
        roots.iter().map(|root| (root.root, root.descriptor)).collect();
    let mut payloads = Payloads::new();
    let mut total_bytes = 0u64;
    for record in records {
        let EntryKind::RegularFile { size, sha256, .. } = &record.kind else {
            continue;
        };
        if *size > limits.max_file_bytes {
            return Err(GenerationCaptureError::InventoryTooLarge);
        }
        total_bytes =
            total_bytes.checked_add(*size).ok_or(GenerationCaptureError::InventoryTooLarge)?;
        if total_bytes > limits.max_total_bytes {
            return Err(GenerationCaptureError::InventoryTooLarge);
        }
        let root = descriptors
            .get(&record.root)
            .copied()
            .ok_or(GenerationCaptureError::VerificationMismatch)?;
        let bytes = read_declared_file(root, &record.path, *size)?;
        if bytes.len() as u64 != *size || digest(&bytes) != *sha256 {
            return Err(GenerationCaptureError::PayloadMismatch);
        }
        match payloads.entry(sha256.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(bytes);
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                if entry.get() != &bytes {
                    return Err(GenerationCaptureError::PayloadMismatch);
                }
            }
        }
    }
    Ok(payloads)
}

fn read_declared_file(
    root: &File,
    path: &RelativePath,
    expected_size: u64,
) -> Result<Vec<u8>, GenerationCaptureError> {
    let fd = openat2(
        root,
        path.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )?;
    let file = File::from(fd);
    let before = fstat(&file)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_size < 0
        || before.st_size as u64 != expected_size
        || before.st_nlink == 0
    {
        return Err(GenerationCaptureError::PayloadMismatch);
    }
    let capacity =
        usize::try_from(expected_size).map_err(|_| GenerationCaptureError::InventoryTooLarge)?;
    let mut bytes = Vec::with_capacity(capacity);
    (&file).take(expected_size.saturating_add(1)).read_to_end(&mut bytes)?;
    let after = fstat(&file)?;
    if bytes.len() != capacity || !same_file_fingerprint(&before, &after) {
        return Err(GenerationCaptureError::VerificationMismatch);
    }
    Ok(bytes)
}

fn same_file_fingerprint(before: &rustix::fs::Stat, after: &rustix::fs::Stat) -> bool {
    before.st_dev == after.st_dev
        && before.st_ino == after.st_ino
        && before.st_mode == after.st_mode
        && before.st_uid == after.st_uid
        && before.st_gid == after.st_gid
        && before.st_nlink == after.st_nlink
        && before.st_size == after.st_size
        && before.st_mtime == after.st_mtime
        && before.st_mtime_nsec == after.st_mtime_nsec
        && before.st_ctime == after.st_ctime
        && before.st_ctime_nsec == after.st_ctime_nsec
}

fn digest(bytes: &[u8]) -> Sha256 {
    let text =
        Sha256Digest::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    Sha256::new(text).expect("SHA-256 formatter is canonical")
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::SnapshotLink;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "ryoku-generation-capture-{label}-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn descriptor(&self) -> File {
            File::open(&self.0).unwrap()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            make_writable(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn make_writable(path: &std::path::Path) {
        let Ok(entries) = fs::read_dir(path) else { return };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                make_writable(&entry.path());
            }
        }
    }

    fn request<'a>(roots: &'a [DeclaredRoot<'a>]) -> GenerationCaptureRequest<'a> {
        GenerationCaptureRequest {
            transaction_id: Identifier::new("capture-transaction-1").unwrap(),
            generation_id: Identifier::new("capture-generation-1").unwrap(),
            parent_generation_id: None,
            snapshot: VerifiedSnapshotLink::from_preverified_link(
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
            ),
            reason: OperationReason::ProfileApply,
            created_utc: "2026-01-01T00:00:00Z".into(),
            package_revisions: Vec::new(),
            provider_revision: None,
            profile_revision: None,
            roots,
            limits: CaptureLimits::default(),
        }
    }

    #[test]
    fn capture_publishes_only_complete_verified_inventory() {
        let source = TempDir::new("source");
        let store_root = TempDir::new("store");
        fs::create_dir(source.0.join("preferences")).unwrap();
        fs::write(source.0.join("preferences/theme.json"), br#"{"theme":"dark"}"#).unwrap();
        let source_fd = source.descriptor();
        let store_fd = store_root.descriptor();
        let store = GenerationStore::open_for_test(&store_fd).unwrap();
        let paths = [
            RelativePath::new("missing.json").unwrap(),
            RelativePath::new("preferences").unwrap(),
            RelativePath::new("preferences/theme.json").unwrap(),
        ];
        let roots = [DeclaredRoot {
            root: StateRoot::UserRyokuConfig,
            descriptor: &source_fd,
            paths: &paths,
        }];

        let manifest =
            capture_publish_verify_without_xattrs_for_test(&store, request(&roots)).unwrap();

        assert!(manifest.is_mutation_ready());
        assert_eq!(manifest.files().len(), 3);
        assert_eq!(store.verify_transaction(&manifest.transaction_id).unwrap(), manifest);
    }

    #[test]
    fn symlink_failure_does_not_publish_a_generation() {
        let source = TempDir::new("symlink-source");
        let store_root = TempDir::new("symlink-store");
        let outside = source.0.with_extension("outside");
        fs::write(&outside, b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, source.0.join("profile.json")).unwrap();
        let source_fd = source.descriptor();
        let store_fd = store_root.descriptor();
        let store = GenerationStore::open_for_test(&store_fd).unwrap();
        let paths = [RelativePath::new("profile.json").unwrap()];
        let roots = [DeclaredRoot {
            root: StateRoot::UserRyokuProfile,
            descriptor: &source_fd,
            paths: &paths,
        }];
        let request = request(&roots);

        assert!(matches!(
            capture_publish_verify_without_xattrs_for_test(&store, request),
            Err(GenerationCaptureError::Capture(CaptureError::UnsupportedEntryType))
        ));
        assert!(
            store.verify_transaction(&Identifier::new("capture-transaction-1").unwrap()).is_err()
        );
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn duplicate_roots_are_rejected_before_capture() {
        let source = TempDir::new("duplicate-source");
        let store_root = TempDir::new("duplicate-store");
        let source_fd = source.descriptor();
        let store_fd = store_root.descriptor();
        let store = GenerationStore::open_for_test(&store_fd).unwrap();
        let paths = [RelativePath::new("missing").unwrap()];
        let roots = [
            DeclaredRoot {
                root: StateRoot::UserRyokuProfile,
                descriptor: &source_fd,
                paths: &paths,
            },
            DeclaredRoot {
                root: StateRoot::UserRyokuProfile,
                descriptor: &source_fd,
                paths: &paths,
            },
        ];

        assert!(matches!(
            capture_publish_verify_without_xattrs_for_test(&store, request(&roots)),
            Err(GenerationCaptureError::DuplicateRoot)
        ));
    }

    #[test]
    fn unsupported_metadata_aborts_without_catalog_publication() {
        let source = TempDir::new("metadata-source");
        let store_root = TempDir::new("metadata-store");
        fs::write(source.0.join("profile.json"), b"ignored").unwrap();
        let source_fd = source.descriptor();
        let store_fd = store_root.descriptor();
        let store = GenerationStore::open_for_test(&store_fd).unwrap();
        let paths = [RelativePath::new("profile.json").unwrap()];
        let roots = [DeclaredRoot {
            root: StateRoot::UserRyokuProfile,
            descriptor: &source_fd,
            paths: &paths,
        }];

        let result = capture_publish_verify_using(&store, request(&roots), |_, _, _, _| {
            Err(CaptureError::UnsupportedMetadata("system.posix_acl_access"))
        });

        assert!(matches!(
            result,
            Err(GenerationCaptureError::Capture(CaptureError::UnsupportedMetadata(
                "system.posix_acl_access"
            )))
        ));
        assert!(
            store.verify_transaction(&Identifier::new("capture-transaction-1").unwrap()).is_err()
        );
    }
}
