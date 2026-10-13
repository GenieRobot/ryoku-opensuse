//! Validated, local recovery manifest primitives.
//!
//! This crate models only bounded metadata. It performs no filesystem access,
//! serialization, hashing, command execution, or network access. Callers must
//! establish those properties at a privileged storage boundary before using a
//! manifest to authorize a mutation.

use std::collections::BTreeSet;

/// Canonical lowercase hexadecimal SHA-256 value.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Sha256(String);

impl Sha256 {
    /// Accepts exactly 64 lowercase hexadecimal characters.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            Ok(Self(value))
        } else {
            Err(ValidationError::InvalidDigest)
        }
    }

    /// Returns the canonical digest text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded canonical identifier used for transaction and generation lookup.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Identifier(String);

impl Identifier {
    /// Accepts lowercase ASCII letters/digits with single internal hyphens.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid = (1..=96).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes[bytes.len() - 1].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
            && !value.contains("--");
        if valid { Ok(Self(value)) } else { Err(ValidationError::InvalidIdentifier) }
    }

    /// Returns the validated identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A normalized path relative to one fixed manifest root.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RelativePath(String);

impl RelativePath {
    /// Rejects absolute paths, traversal, empty or dot components, backslashes,
    /// NUL bytes, and paths longer than 4096 bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 4096
            || value.starts_with('/')
            || value.contains('\\')
            || value.as_bytes().contains(&0)
            || value
                .split('/')
                .any(|component| component.is_empty() || component == "." || component == "..")
        {
            return Err(ValidationError::InvalidRelativePath);
        }
        Ok(Self(value))
    }

    /// Returns the validated relative path text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Fixed roots that a manifest may name. Callers map these variants to
/// administrator-defined directories; manifest text never supplies a root.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum StateRoot {
    /// Ryoku-owned system configuration below `/etc/ryoku`.
    EtcRyoku,
    /// Declared per-user Ryoku configuration below its resolved config root.
    UserRyokuConfig,
    /// Immutable generations below the dedicated Ryoku state volume.
    StateGeneration,
}

/// Exact metadata required for a declared entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryMetadata {
    /// POSIX permission and file-type-independent mode bits.
    pub mode: u32,
    /// Numeric owner ID.
    pub uid: u32,
    /// Numeric group ID.
    pub gid: u32,
    /// Modification time as nanoseconds since the Unix epoch.
    pub mtime_unix_ns: i128,
    /// Optional digest of the explicitly supported ACL representation.
    pub acl_sha256: Option<Sha256>,
}

/// One declared entry and the metadata/content proof required for its kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// The declared destination did not exist before the transaction.
    Absent,
    /// A directory whose metadata is captured without recursive traversal.
    Directory { metadata: EntryMetadata },
    /// A regular file with a verified content digest and exact byte size.
    RegularFile { metadata: EntryMetadata, size: u64, sha256: Sha256 },
    /// A symlink recorded by raw target text; callers must never follow it.
    Symlink { metadata: EntryMetadata, target: String },
}

/// A single explicitly declared path in a transaction generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileRecord {
    /// Fixed root selector.
    pub root: StateRoot,
    /// Validated path relative to that root.
    pub path: RelativePath,
    /// Entry kind and its declared proof metadata.
    pub kind: EntryKind,
}

impl FileRecord {
    /// Constructs a record while validating metadata shape.
    pub fn new(
        root: StateRoot,
        path: RelativePath,
        kind: EntryKind,
    ) -> Result<Self, ValidationError> {
        if let EntryKind::Symlink { target, .. } = &kind
            && (target.is_empty() || target.len() > 4096 || target.as_bytes().contains(&0))
        {
            return Err(ValidationError::InvalidSymlinkTarget);
        }
        Ok(Self { root, path, kind })
    }
}

/// Linkage required before a managed operation may begin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryManifest {
    /// Supported manifest schema. This model accepts version 1 only.
    pub schema_version: u16,
    /// Idempotency identity for the operation.
    pub transaction_id: Identifier,
    /// Immutable Ryoku state generation linked to the system snapshot.
    pub generation_id: Identifier,
    /// Parent generation, if this operation replaces an earlier generation.
    pub parent_generation_id: Option<Identifier>,
    /// Snapper configuration name.
    pub snapper_config: Identifier,
    /// Decimal Snapper pre-change snapshot ID.
    pub snapper_snapshot_id: String,
    /// Lowercase canonical filesystem UUID.
    pub filesystem_uuid: String,
    /// Complete explicitly declared affected path set.
    files: Vec<FileRecord>,
    /// Set used to reject duplicate root/path pairs.
    file_keys: BTreeSet<(StateRoot, RelativePath)>,
}

impl RecoveryManifest {
    /// Creates a version-1 manifest with validated system/generation linkage.
    pub fn new(
        transaction_id: Identifier,
        generation_id: Identifier,
        parent_generation_id: Option<Identifier>,
        snapper_config: Identifier,
        snapper_snapshot_id: impl Into<String>,
        filesystem_uuid: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let snapper_snapshot_id = snapper_snapshot_id.into();
        if snapper_snapshot_id.is_empty()
            || !snapper_snapshot_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ValidationError::InvalidSnapshotId);
        }
        let filesystem_uuid = filesystem_uuid.into();
        if !is_uuid(&filesystem_uuid) {
            return Err(ValidationError::InvalidFilesystemUuid);
        }
        Ok(Self {
            schema_version: 1,
            transaction_id,
            generation_id,
            parent_generation_id,
            snapper_config,
            snapper_snapshot_id,
            filesystem_uuid,
            files: Vec::new(),
            file_keys: BTreeSet::new(),
        })
    }

    /// Adds one declared path, rejecting duplicate normalized root/path pairs.
    pub fn add_file(&mut self, file: FileRecord) -> Result<(), ValidationError> {
        let key = (file.root, file.path.clone());
        if !self.file_keys.insert(key) {
            return Err(ValidationError::DuplicatePath);
        }
        self.files.push(file);
        Ok(())
    }

    /// Returns the validated bounded path inventory.
    pub fn files(&self) -> &[FileRecord] {
        &self.files
    }
}

/// Validation failures for untrusted or malformed manifest input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    /// Identifier violates the canonical bounded syntax.
    InvalidIdentifier,
    /// A relative path is unsafe or not normalized.
    InvalidRelativePath,
    /// SHA-256 text is not canonical lowercase hexadecimal.
    InvalidDigest,
    /// Symlink target is empty, oversized, or contains a NUL byte.
    InvalidSymlinkTarget,
    /// Snapper snapshot ID is not decimal text.
    InvalidSnapshotId,
    /// Filesystem UUID is not canonical lowercase hexadecimal UUID text.
    InvalidFilesystemUuid,
    /// The same fixed root/path was declared more than once.
    DuplicatePath,
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> RecoveryManifest {
        RecoveryManifest::new(
            Identifier::new("txn-example-01").unwrap(),
            Identifier::new("gen-example-01").unwrap(),
            None,
            Identifier::new("root").unwrap(),
            "42",
            "00000000-0000-0000-0000-000000000000",
        )
        .unwrap()
    }

    fn metadata() -> EntryMetadata {
        EntryMetadata { mode: 0o640, uid: 1000, gid: 1000, mtime_unix_ns: 0, acl_sha256: None }
    }

    #[test]
    fn relative_paths_reject_traversal_and_ambiguous_components() {
        for invalid in
            ["", "/etc/passwd", "../outside", "etc/../outside", "a//b", "./a", "a/.", "a\\b"]
        {
            assert_eq!(RelativePath::new(invalid), Err(ValidationError::InvalidRelativePath));
        }
        assert_eq!(
            RelativePath::new("appearance/theme.toml").unwrap().as_str(),
            "appearance/theme.toml"
        );
    }

    #[test]
    fn identifiers_and_digests_are_canonical() {
        assert!(Identifier::new("Txn-1").is_err());
        assert!(Identifier::new("txn--1").is_err());
        assert!(Sha256::new("A".repeat(64)).is_err());
        assert_eq!(Sha256::new("a".repeat(64)).unwrap().as_str(), "a".repeat(64));
    }

    #[test]
    fn snapshot_link_and_filesystem_uuid_are_required() {
        assert!(
            RecoveryManifest::new(
                Identifier::new("txn-1").unwrap(),
                Identifier::new("gen-1").unwrap(),
                None,
                Identifier::new("root").unwrap(),
                "42x",
                "00000000-0000-0000-0000-000000000000",
            )
            .is_err()
        );
        assert!(
            RecoveryManifest::new(
                Identifier::new("txn-1").unwrap(),
                Identifier::new("gen-1").unwrap(),
                None,
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-00000000000Z",
            )
            .is_err()
        );
    }

    #[test]
    fn file_inventory_rejects_duplicate_paths_and_keeps_absence_explicit() {
        let path = RelativePath::new("settings.toml").unwrap();
        let mut manifest = manifest();
        manifest
            .add_file(
                FileRecord::new(StateRoot::EtcRyoku, path.clone(), EntryKind::Absent).unwrap(),
            )
            .unwrap();
        assert_eq!(manifest.files()[0].kind, EntryKind::Absent);
        assert_eq!(
            manifest.add_file(
                FileRecord::new(
                    StateRoot::EtcRyoku,
                    path,
                    EntryKind::Directory { metadata: metadata() }
                )
                .unwrap()
            ),
            Err(ValidationError::DuplicatePath)
        );
        assert_eq!(manifest.files().len(), 1);
    }

    #[test]
    fn symlink_targets_are_recorded_without_path_interpretation() {
        let target = "../shared/theme.toml".to_owned();
        let file = FileRecord::new(
            StateRoot::UserRyokuConfig,
            RelativePath::new("theme.toml").unwrap(),
            EntryKind::Symlink { metadata: metadata(), target: target.clone() },
        )
        .unwrap();
        assert!(
            matches!(file.kind, EntryKind::Symlink { target: captured, .. } if captured == target)
        );
        assert!(
            FileRecord::new(
                StateRoot::UserRyokuConfig,
                RelativePath::new("theme.toml").unwrap(),
                EntryKind::Symlink { metadata: metadata(), target: "".to_owned() },
            )
            .is_err()
        );
    }
}
