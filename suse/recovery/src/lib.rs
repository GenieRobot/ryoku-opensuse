//! Validated, local recovery manifest primitives.
//!
//! This crate validates typed schema values and provides canonical JSON,
//! SHA-256, and bounded exact-file capture primitives. Capture is anchored at
//! caller-opened directory descriptors and performs no command or network
//! access. Callers must still establish root authorization, persistence,
//! generation publication, and recovery properties before using a manifest to
//! authorize a mutation.

#[cfg(target_os = "linux")]
pub mod capture;
pub mod configuration_apply;
#[cfg(target_os = "linux")]
pub mod configuration_stage;
#[cfg(target_os = "linux")]
pub mod disk_stage;
#[cfg(target_os = "linux")]
pub mod generation;
#[cfg(target_os = "linux")]
pub mod generation_capture;
#[cfg(target_os = "linux")]
pub mod inode_metadata;
#[cfg(target_os = "linux")]
pub mod machine_state;
pub mod metadata;
pub mod native_checkpoint;
#[cfg(target_os = "linux")]
pub mod preparation;
#[cfg(target_os = "linux")]
pub mod restore;
mod state_boot_unit;
pub mod state_creation;
#[cfg(target_os = "linux")]
pub mod state_mount;
pub mod state_provision;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256 as Sha256Digest};

/// Canonical lowercase hexadecimal SHA-256 value.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
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
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
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

/// Bounded package identity; unlike an internal identifier, RPM names may
/// begin with a digit and may contain dots, underscores, or plus signs.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
pub struct PackageName(String);

impl PackageName {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let valid = (1..=128).contains(&value.len())
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
            });
        if valid { Ok(Self(value)) } else { Err(ValidationError::InvalidPackageName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded opaque package/profile revision, allowing common NEVRA/version
/// punctuation while excluding whitespace, separators, and control bytes.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
pub struct Revision(String);

impl Revision {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let valid = (1..=256).contains(&value.len())
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'+' | b':' | b'~' | b'^' | b'-')
            });
        if valid { Ok(Self(value)) } else { Err(ValidationError::InvalidRevision) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A normalized path relative to one fixed manifest root.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
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

macro_rules! deserialize_validated_string {
    ($type:ty) => {
        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                <$type>::new(value).map_err(|error| serde::de::Error::custom(format!("{error:?}")))
            }
        }
    };
}

deserialize_validated_string!(Sha256);
deserialize_validated_string!(Identifier);
deserialize_validated_string!(PackageName);
deserialize_validated_string!(Revision);
deserialize_validated_string!(RelativePath);

/// Fixed roots that a manifest may name. Callers map these variants to
/// administrator-defined directories; manifest text never supplies a root.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateRoot {
    /// Ryoku-owned system configuration below `/etc/ryoku`.
    EtcRyoku,
    /// Declared per-user Ryoku configuration below its resolved config root.
    UserRyokuConfig,
    /// Reserved internal selector for immutable generations. External capture
    /// inventories cannot select it; the recovery service may use it for
    /// verified internal generation staging records.
    StateGeneration,
    /// User-owned profile selection below the resolved Ryoku config root.
    UserRyokuProfile,
}

impl StateRoot {
    /// Stable root name used by manifest encoders; concrete XDG paths are
    /// resolved for the target UID rather than accepted from manifest data.
    pub fn manifest_name(self) -> &'static str {
        match self {
            Self::EtcRyoku => "etc-ryoku",
            Self::UserRyokuConfig => "user-ryoku-config",
            Self::StateGeneration => "state-generation",
            Self::UserRyokuProfile => "user-ryoku-profile",
        }
    }

    /// Whether records in this fixed root belong to the writable
    /// configuration restore scope. The generation namespace is retained as
    /// immutable source data and is never a configuration target.
    pub const fn is_configuration_state(self) -> bool {
        matches!(self, Self::EtcRyoku | Self::UserRyokuConfig | Self::UserRyokuProfile)
    }
}

/// Only non-ACL xattr names captured by the initial contract; additions require review.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum XattrName {
    /// SELinux label, when supported by the filesystem/provider.
    SelinuxLabel,
}

impl XattrName {
    /// Canonical Linux xattr key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelinuxLabel => "security.selinux",
        }
    }
}

/// Digests of allowed metadata values; public manifests do not embed labels.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtendedMetadata {
    pub values: BTreeMap<XattrName, Sha256>,
}

/// Typed operation intent recorded in each manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationReason {
    ProfileApply,
    ProviderChange,
    PackageChange,
    ConfigurationRestore,
    Other(Identifier),
}

/// Capture/linkage/verification phase state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationStatus {
    Pending,
    Verified,
    Failed,
}

/// Profile layer identity when a transaction resolves profile settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRevision {
    pub profile_id: Identifier,
    pub profile_revision: Revision,
    pub defaults_revision: Revision,
    pub system_override_revision: Option<Revision>,
    pub companion_revision: Option<Revision>,
    pub personal_override_revision: Option<Revision>,
}

/// Identity and before/after revisions for one changed package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRevision {
    pub package_name: PackageName,
    pub before_nevra: Option<Revision>,
    pub after_nevra: Option<Revision>,
    pub upstream_revision: Option<Revision>,
}

/// Identity and before/after revisions for the provider used by an operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRevision {
    pub provider_id: Identifier,
    pub before_revision: Option<Revision>,
    pub after_revision: Option<Revision>,
}

/// Validated link to the system-side pre-change snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLink {
    pub snapper_config: Identifier,
    pub snapper_snapshot_id: String,
    pub filesystem_uuid: String,
}

impl SnapshotLink {
    pub fn new(
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
        Ok(Self { snapper_config, snapper_snapshot_id, filesystem_uuid })
    }
}

/// Exact metadata required for a declared entry.
/// serde_json's Value serializer does not support i128; represent the exact
/// nanosecond count as a canonical decimal string in persisted v1 manifests.
mod i128_decimal {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &i128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || (value.starts_with('0') && value.len() > 1)
            || (value.starts_with("-0") && value.len() > 2)
            || value.starts_with('+')
        {
            return Err(serde::de::Error::custom("non-canonical i128 decimal"));
        }
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMetadata {
    /// POSIX permission and file-type-independent mode bits.
    pub mode: u32,
    /// Numeric owner ID.
    pub uid: u32,
    /// Numeric group ID.
    pub gid: u32,
    /// Modification time as nanoseconds since the Unix epoch.
    #[serde(with = "i128_decimal")]
    pub mtime_unix_ns: i128,
    /// Optional digest of the explicitly supported ACL representation.
    pub acl_sha256: Option<Sha256>,
    /// Digests for present, explicitly supported extended attributes.
    pub extended: ExtendedMetadata,
}

/// One declared entry and the metadata/content proof required for its kind.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EntryKind {
    /// The declared destination did not exist before the transaction.
    Absent,
    /// A directory whose metadata is captured without recursive traversal.
    Directory { metadata: EntryMetadata },
    /// A regular file with a verified content digest and exact byte size.
    RegularFile {
        metadata: EntryMetadata,
        size: u64,
        sha256: Sha256,
        hard_link_group: Option<Identifier>,
    },
    /// A symlink recorded by raw target text; callers must never follow it.
    Symlink { metadata: EntryMetadata, target: String },
}

/// A single explicitly declared path in a transaction generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
        let record = Self { root, path, kind };
        record.validate()?;
        Ok(record)
    }

    fn validate(&self) -> Result<(), ValidationError> {
        let metadata = match &self.kind {
            EntryKind::Absent => return Ok(()),
            EntryKind::Directory { metadata }
            | EntryKind::RegularFile { metadata, .. }
            | EntryKind::Symlink { metadata, .. } => metadata,
        };
        if metadata.mode & !0o7777 != 0 {
            return Err(ValidationError::InvalidMode);
        }
        if let EntryKind::Symlink { target, .. } = &self.kind
            && (target.is_empty() || target.len() > 4096 || target.as_bytes().contains(&0))
        {
            return Err(ValidationError::InvalidSymlinkTarget);
        }
        Ok(())
    }
}

/// Linkage required before a managed operation may begin.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Operation reason and bounded RFC3339 UTC creation time.
    pub reason: OperationReason,
    pub created_utc: String,
    /// Per-package before/after NEVRA and upstream source provenance.
    package_revisions: Vec<PackageRevision>,
    /// Provider identity and before/after provider revision, when applicable.
    pub provider_revision: Option<ProviderRevision>,
    /// Resolved profile provenance, when applicable.
    pub profile_revision: Option<ProfileRevision>,
    /// Capture, linkage, and verification checkpoints.
    pub capture_status: VerificationStatus,
    pub linkage_status: VerificationStatus,
    pub verification_status: VerificationStatus,
    /// Canonical digest, assigned by [`Self::seal`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<Sha256>,
    /// Complete explicitly declared affected path set.
    files: Vec<FileRecord>,
    /// Set used to reject duplicate root/path pairs.
    #[serde(skip, default)]
    file_keys: BTreeSet<(StateRoot, RelativePath)>,
}

impl RecoveryManifest {
    /// Creates a version-1 manifest with validated system/generation linkage.
    pub fn new(
        transaction_id: Identifier,
        generation_id: Identifier,
        parent_generation_id: Option<Identifier>,
        snapshot: SnapshotLink,
        reason: OperationReason,
        created_utc: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let created_utc = created_utc.into();
        if !is_utc_timestamp(&created_utc) {
            return Err(ValidationError::InvalidTimestamp);
        }
        Ok(Self {
            schema_version: 1,
            transaction_id,
            generation_id,
            parent_generation_id,
            snapper_config: snapshot.snapper_config,
            snapper_snapshot_id: snapshot.snapper_snapshot_id,
            filesystem_uuid: snapshot.filesystem_uuid,
            reason,
            created_utc,
            package_revisions: Vec::new(),
            provider_revision: None,
            profile_revision: None,
            capture_status: VerificationStatus::Pending,
            linkage_status: VerificationStatus::Pending,
            verification_status: VerificationStatus::Pending,
            manifest_sha256: None,
            files: Vec::new(),
            file_keys: BTreeSet::new(),
        })
    }

    /// Adds one declared path, rejecting duplicate normalized root/path pairs.
    pub fn add_file(&mut self, file: FileRecord) -> Result<(), ValidationError> {
        file.validate()?;
        let key = (file.root, file.path.clone());
        if !self.file_keys.insert(key) {
            return Err(ValidationError::DuplicatePath);
        }
        self.files.push(file);
        self.files.sort_by(|left, right| {
            (left.root.manifest_name(), left.path.as_str())
                .cmp(&(right.root.manifest_name(), right.path.as_str()))
        });
        Ok(())
    }

    /// Returns the validated bounded path inventory.
    pub fn files(&self) -> &[FileRecord] {
        &self.files
    }

    /// Assigns package revisions in stable package-name order, rejecting
    /// duplicate package identities before serialization.
    pub fn set_provenance(
        &mut self,
        mut package_revisions: Vec<PackageRevision>,
        provider_revision: Option<ProviderRevision>,
        profile_revision: Option<ProfileRevision>,
    ) -> Result<(), ValidationError> {
        package_revisions.sort_by(|left, right| left.package_name.cmp(&right.package_name));
        if package_revisions.windows(2).any(|pair| pair[0].package_name == pair[1].package_name) {
            return Err(ValidationError::DuplicatePackageRevision);
        }
        self.package_revisions = package_revisions;
        self.provider_revision = provider_revision;
        self.profile_revision = profile_revision;
        Ok(())
    }

    /// Returns per-package provenance in stable package-name order.
    pub fn package_revisions(&self) -> &[PackageRevision] {
        &self.package_revisions
    }

    /// Records phase outcomes; callers cannot claim readiness with a failed phase.
    pub fn set_verification(
        &mut self,
        capture: VerificationStatus,
        linkage: VerificationStatus,
        verification: VerificationStatus,
    ) {
        self.capture_status = capture;
        self.linkage_status = linkage;
        self.verification_status = verification;
    }

    /// Indicates all currently modeled pre-mutation prerequisites are present.
    pub fn is_mutation_ready(&self) -> bool {
        !self.files.is_empty()
            && self.capture_status == VerificationStatus::Verified
            && self.linkage_status == VerificationStatus::Verified
            && self.verification_status == VerificationStatus::Verified
            && self.verify_digest().is_ok_and(|valid| valid)
    }

    /// Parses a bounded persisted JSON manifest, rejects unknown fields and
    /// unsupported schema versions, validates all typed values, and verifies
    /// the canonical digest before returning it. This performs no I/O beyond
    /// the caller-provided bytes and never consults a Git remote.
    pub fn parse_verified_json(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::InputTooLarge);
        }
        let mut manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate()?;
        manifest.files.sort_by(|left, right| {
            (left.root.manifest_name(), left.path.as_str())
                .cmp(&(right.root.manifest_name(), right.path.as_str()))
        });
        manifest.file_keys =
            manifest.files.iter().map(|file| (file.root, file.path.clone())).collect();
        if !manifest.verify_digest()? {
            return Err(ManifestError::DigestMismatch);
        }
        Ok(manifest)
    }

    /// Validates the supported v1 schema and its cross-field invariants.
    /// This validates the typed value after construction or parsing.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != 1 {
            return Err(ValidationError::UnsupportedSchemaVersion);
        }
        if self.snapper_snapshot_id.is_empty()
            || !self.snapper_snapshot_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ValidationError::InvalidSnapshotId);
        }
        if !is_uuid(&self.filesystem_uuid) {
            return Err(ValidationError::InvalidFilesystemUuid);
        }
        if !is_utc_timestamp(&self.created_utc) {
            return Err(ValidationError::InvalidTimestamp);
        }

        let mut file_keys = BTreeSet::new();
        let mut link_groups = BTreeMap::new();
        for file in &self.files {
            file.validate()?;
            if !file_keys.insert((file.root, file.path.clone())) {
                return Err(ValidationError::DuplicatePath);
            }
            if let EntryKind::RegularFile { hard_link_group: Some(group), .. } = &file.kind {
                let (first, count) = link_groups.entry(group).or_insert((file, 0_usize));
                if first.root != file.root || first.kind != file.kind {
                    return Err(ValidationError::InvalidHardLinkGroup);
                }
                *count += 1;
            }
        }
        if link_groups.values().any(|(_, count)| *count < 2) {
            return Err(ValidationError::InvalidHardLinkGroup);
        }
        if self
            .package_revisions
            .windows(2)
            .any(|pair| pair[0].package_name >= pair[1].package_name)
        {
            return Err(ValidationError::DuplicateOrUnsortedPackageRevision);
        }
        Ok(())
    }

    /// Returns canonical JSON for the manifest payload, excluding its digest.
    /// Object keys are lexically ordered by serde_json's map representation;
    /// declared files are sorted by fixed root and normalized relative path.
    pub fn canonical_payload_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        let mut payload = self.clone();
        payload.manifest_sha256 = None;
        Ok(canonical_json(&payload)?)
    }

    /// Serializes the complete manifest as canonical JSON, including its
    /// digest once sealed.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        Ok(canonical_json(self)?)
    }

    /// Computes and assigns the SHA-256 of canonical JSON with the digest field
    /// omitted, avoiding a self-referential checksum.
    pub fn seal(&mut self) -> Result<(), ManifestError> {
        let bytes = self.canonical_payload_bytes()?;
        let digest = Sha256Digest::digest(bytes);
        let text = digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        self.manifest_sha256 = Some(Sha256::new(text).expect("SHA-256 formatter is canonical"));
        Ok(())
    }

    /// Verifies the assigned checksum against the canonical payload.
    pub fn verify_digest(&self) -> Result<bool, ManifestError> {
        self.validate()?;
        let Some(expected) = &self.manifest_sha256 else {
            return Ok(false);
        };
        let mut payload = self.clone();
        payload.manifest_sha256 = None;
        let digest = Sha256Digest::digest(canonical_json(&payload)?);
        let actual = digest.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        Ok(expected.as_str() == actual)
    }
}

pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    serde_json::to_vec(&value)
}

/// Mandatory capture rules for a future filesystem boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureRule {
    ExactDeclaredEntries,
    NoSymlinkTraversal,
    RejectSpecialFiles,
    FailOnUnsupportedMetadata,
    ExcludeUndeclaredPaths,
}

pub const REQUIRED_CAPTURE_RULES: [CaptureRule; 5] = [
    CaptureRule::ExactDeclaredEntries,
    CaptureRule::NoSymlinkTraversal,
    CaptureRule::RejectSpecialFiles,
    CaptureRule::FailOnUnsupportedMetadata,
    CaptureRule::ExcludeUndeclaredPaths,
];

/// Validation failures for untrusted or malformed manifest input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    /// Identifier violates the canonical bounded syntax.
    InvalidIdentifier,
    /// Manifest schema version is not supported by this writer.
    UnsupportedSchemaVersion,
    /// Package name is empty, oversized, or contains unsupported characters.
    InvalidPackageName,
    /// Revision is empty, oversized, or contains unsupported characters.
    InvalidRevision,
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
    /// A package identity was declared more than once in one operation.
    DuplicatePackageRevision,
    /// Package revisions are duplicated or not in canonical identity order.
    DuplicateOrUnsortedPackageRevision,
    /// File mode contains bits outside POSIX permission/special-mode bits.
    InvalidMode,
    /// Creation time is not a bounded RFC3339 UTC timestamp.
    InvalidTimestamp,
    /// Hard-link groups require at least two identical inode records in one root.
    InvalidHardLinkGroup,
}

/// Failure while parsing, validating, or canonically encoding a manifest.
#[derive(Debug)]
pub enum ManifestError {
    /// Input exceeds the bounded persisted-manifest size.
    InputTooLarge,
    /// Parsed manifest is unsealed or its canonical digest does not match.
    DigestMismatch,
    Validation(ValidationError),
    Serialization(serde_json::Error),
}

impl From<ValidationError> for ManifestError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

impl From<serde_json::Error> for ManifestError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge => write!(formatter, "manifest exceeds the 1 MiB input limit"),
            Self::DigestMismatch => {
                write!(formatter, "manifest digest is missing or does not match")
            }
            Self::Validation(error) => write!(formatter, "invalid recovery manifest: {error:?}"),
            Self::Serialization(error) => {
                write!(formatter, "manifest JSON processing failed: {error}")
            }
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InputTooLarge | Self::DigestMismatch | Self::Validation(_) => None,
            Self::Serialization(error) => Some(error),
        }
    }
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

fn is_utc_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || b.is_ascii_digit())
    {
        return false;
    }
    let number = |start: usize, end: usize| -> u32 {
        value[start..end].parse().expect("digits validated above")
    };
    let (year, month, day) = (number(0, 4), number(5, 7), number(8, 10));
    let (hour, minute, second) = (number(11, 13), number(14, 16), number(17, 19));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day) && hour < 24 && minute < 60 && second < 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_manifest_round_trips_and_rejects_unknown_fields() {
        let mut original = manifest();
        original.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        original
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("theme.json").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        original.seal().unwrap();
        let encoded = original.canonical_bytes().unwrap();
        let decoded = RecoveryManifest::parse_verified_json(&encoded).unwrap();
        assert_eq!(decoded.canonical_bytes().unwrap(), encoded);
        assert_eq!(decoded.files(), original.files());

        let json = String::from_utf8(encoded).unwrap();
        let unknown = format!("{{\"unknown\":\"x\",{}", &json[1..]);
        assert!(serde_json::from_str::<RecoveryManifest>(&unknown).is_err());
    }

    #[test]
    fn persisted_manifest_round_trips_exact_i128_mtime() {
        let mut value = manifest();
        value
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("profile.json").unwrap(),
                    EntryKind::Directory {
                        metadata: EntryMetadata {
                            mode: 0o700,
                            uid: 1000,
                            gid: 1000,
                            mtime_unix_ns: 1_765_000_000_123_456_789,
                            acl_sha256: None,
                            extended: ExtendedMetadata::default(),
                        },
                    },
                )
                .unwrap(),
            )
            .unwrap();
        value.seal().unwrap();
        let bytes = value.canonical_bytes().unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("\"mtime_unix_ns\":\"1765000000123456789\""));
        let parsed = RecoveryManifest::parse_verified_json(&bytes).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn persisted_manifest_parser_validates_wrapper_values() {
        let mut original = manifest();
        original.seal().unwrap();
        let encoded = String::from_utf8(original.canonical_bytes().unwrap()).unwrap();
        let invalid_id = encoded.replace("txn-example-01", "../escape");
        assert!(matches!(
            RecoveryManifest::parse_verified_json(invalid_id.as_bytes()),
            Err(ManifestError::Serialization(_))
        ));
        let invalid_uuid = encoded.replace("00000000-0000-0000-0000-000000000000", "not-a-uuid");
        assert!(matches!(
            RecoveryManifest::parse_verified_json(invalid_uuid.as_bytes()),
            Err(ManifestError::Validation(ValidationError::InvalidFilesystemUuid))
        ));

        let mut with_path = manifest();
        with_path
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("settings.json").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        with_path.seal().unwrap();
        let encoded_path = String::from_utf8(with_path.canonical_bytes().unwrap()).unwrap();
        let invalid_path = encoded_path.replace("settings.json", "../outside");
        assert!(matches!(
            RecoveryManifest::parse_verified_json(invalid_path.as_bytes()),
            Err(ManifestError::Serialization(_))
        ));
    }

    #[test]
    fn persisted_manifest_parser_rejects_oversize_and_tampered_digest() {
        assert!(matches!(
            RecoveryManifest::parse_verified_json(&vec![b' '; MAX_MANIFEST_BYTES + 1]),
            Err(ManifestError::InputTooLarge)
        ));

        let mut original = manifest();
        original.seal().unwrap();
        let mut encoded = String::from_utf8(original.canonical_bytes().unwrap()).unwrap();
        encoded = encoded.replace("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z");
        assert!(matches!(
            RecoveryManifest::parse_verified_json(encoded.as_bytes()),
            Err(ManifestError::DigestMismatch)
        ));
    }

    fn manifest() -> RecoveryManifest {
        RecoveryManifest::new(
            Identifier::new("txn-example-01").unwrap(),
            Identifier::new("gen-example-01").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ProfileApply,
            "2026-01-01T00:00:00Z",
        )
        .unwrap()
    }

    fn metadata() -> EntryMetadata {
        EntryMetadata {
            mode: 0o640,
            uid: 1000,
            gid: 1000,
            mtime_unix_ns: 0,
            acl_sha256: None,
            extended: ExtendedMetadata::default(),
        }
    }

    #[test]
    fn hard_link_groups_require_identical_inode_records_within_one_root() {
        let first = FileRecord::new(
            StateRoot::EtcRyoku,
            RelativePath::new("a").unwrap(),
            EntryKind::RegularFile {
                metadata: metadata(),
                size: 1,
                sha256: Sha256::new("a".repeat(64)).unwrap(),
                hard_link_group: Some(Identifier::new("group-a").unwrap()),
            },
        )
        .unwrap();
        let mut second = first.clone();
        second.path = RelativePath::new("b").unwrap();
        let mut singleton = manifest();
        singleton.add_file(first.clone()).unwrap();
        assert_eq!(singleton.validate(), Err(ValidationError::InvalidHardLinkGroup));
        for variant in 0..4 {
            let mut other = second.clone();
            match (&mut other.kind, variant) {
                (EntryKind::RegularFile { size, .. }, 0) => *size += 1,
                (EntryKind::RegularFile { sha256, .. }, 1) => {
                    *sha256 = Sha256::new("b".repeat(64)).unwrap()
                }
                (EntryKind::RegularFile { metadata, .. }, 2) => metadata.uid += 1,
                (_, 3) => other.root = StateRoot::UserRyokuConfig,
                _ => unreachable!(),
            }
            let mut invalid = singleton.clone();
            invalid.add_file(other).unwrap();
            assert_eq!(invalid.validate(), Err(ValidationError::InvalidHardLinkGroup));
            assert!(matches!(
                invalid.seal(),
                Err(ManifestError::Validation(ValidationError::InvalidHardLinkGroup))
            ));
        }
        singleton.add_file(second).unwrap();
        singleton.seal().unwrap();
        assert!(singleton.verify_digest().unwrap());
        assert!(
            RecoveryManifest::parse_verified_json(&singleton.canonical_bytes().unwrap()).is_ok()
        );
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
        assert_eq!(
            Revision::new("ryoku-1.2.3-1.x86_64".to_owned()).unwrap().as_str(),
            "ryoku-1.2.3-1.x86_64"
        );
        assert!(Revision::new("../outside".to_owned()).is_err());
    }

    #[test]
    fn snapshot_link_and_filesystem_uuid_are_required() {
        assert!(
            SnapshotLink::new(
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
                SnapshotLink::new(
                    Identifier::new("root").unwrap(),
                    "42",
                    "00000000-0000-0000-0000-000000000000",
                )
                .unwrap(),
                OperationReason::ProviderChange,
                "2026-19-48T29:99:99Z",
            )
            .is_err()
        );
        assert!(
            SnapshotLink::new(
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

    #[test]
    fn manifest_requires_verified_capture_link_and_digest_before_ready() {
        let mut manifest = manifest();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("profile.json").unwrap(),
                    EntryKind::RegularFile {
                        metadata: metadata(),
                        size: 2,
                        sha256: Sha256::new("a".repeat(64)).unwrap(),
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
        assert!(!manifest.is_mutation_ready());
        manifest.seal().unwrap();
        assert!(manifest.is_mutation_ready());
        assert!(manifest.verify_digest().unwrap());
        manifest.reason = OperationReason::PackageChange;
        assert!(!manifest.verify_digest().unwrap());
        assert!(!manifest.is_mutation_ready());
        manifest.set_verification(
            VerificationStatus::Failed,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        assert!(!manifest.is_mutation_ready());
    }

    #[test]
    fn manifest_serialization_revalidates_mutable_schema_fields() {
        let mut manifest = manifest();
        manifest.schema_version = 2;
        assert_eq!(manifest.validate(), Err(ValidationError::UnsupportedSchemaVersion));
        assert!(matches!(
            manifest.seal(),
            Err(ManifestError::Validation(ValidationError::UnsupportedSchemaVersion))
        ));

        manifest.schema_version = 1;
        manifest.filesystem_uuid = "not-a-uuid".to_owned();
        assert!(matches!(
            manifest.canonical_bytes(),
            Err(ManifestError::Validation(ValidationError::InvalidFilesystemUuid))
        ));
    }

    #[test]
    fn file_records_reject_invalid_mode_bits_before_serialization() {
        let mut invalid_metadata = metadata();
        invalid_metadata.mode = 0o100640;
        assert_eq!(
            FileRecord::new(
                StateRoot::UserRyokuConfig,
                RelativePath::new("profile.json").unwrap(),
                EntryKind::RegularFile {
                    metadata: invalid_metadata,
                    size: 2,
                    sha256: Sha256::new("a".repeat(64)).unwrap(),
                    hard_link_group: None,
                },
            ),
            Err(ValidationError::InvalidMode)
        );
    }

    #[test]
    fn capture_policy_is_bounded_and_metadata_keys_are_allowlisted() {
        assert_eq!(REQUIRED_CAPTURE_RULES.len(), 5);
        assert_eq!(XattrName::SelinuxLabel.as_str(), "security.selinux");
        let mut metadata = metadata();
        metadata
            .extended
            .values
            .insert(XattrName::SelinuxLabel, Sha256::new("c".repeat(64)).unwrap());
        assert_eq!(metadata.extended.values.len(), 1);
        assert_eq!(StateRoot::UserRyokuProfile.manifest_name(), "user-ryoku-profile");
    }

    #[test]
    fn configuration_restore_root_classification_excludes_generation_storage() {
        assert!(StateRoot::EtcRyoku.is_configuration_state());
        assert!(StateRoot::UserRyokuConfig.is_configuration_state());
        assert!(StateRoot::UserRyokuProfile.is_configuration_state());
        assert!(!StateRoot::StateGeneration.is_configuration_state());
    }

    #[test]
    fn canonical_encoding_is_independent_of_file_insertion_order() {
        let mut first = manifest();
        let mut second = manifest();
        let records = [
            FileRecord::new(
                StateRoot::UserRyokuConfig,
                RelativePath::new("z.toml").unwrap(),
                EntryKind::Absent,
            )
            .unwrap(),
            FileRecord::new(
                StateRoot::EtcRyoku,
                RelativePath::new("a.toml").unwrap(),
                EntryKind::Absent,
            )
            .unwrap(),
        ];
        for file in records.clone() {
            first.add_file(file).unwrap();
        }
        for file in records.into_iter().rev() {
            second.add_file(file).unwrap();
        }
        first.seal().unwrap();
        second.seal().unwrap();
        assert_eq!(first.canonical_bytes().unwrap(), second.canonical_bytes().unwrap());
        assert!(first.verify_digest().unwrap());
    }

    #[test]
    fn provenance_serializes_package_and_provider_identity_with_revisions() {
        let zypper = PackageRevision {
            package_name: PackageName::new("zypper").unwrap(),
            before_nevra: Some(Revision::new("1.0-1.x86_64").unwrap()),
            after_nevra: Some(Revision::new("1.1-1.x86_64").unwrap()),
            upstream_revision: Some(Revision::new("release-1.1").unwrap()),
        };
        let libzypp = PackageRevision {
            package_name: PackageName::new("libzypp").unwrap(),
            before_nevra: None,
            after_nevra: Some(Revision::new("17.36-1.x86_64").unwrap()),
            upstream_revision: None,
        };
        let provider = Some(ProviderRevision {
            provider_id: Identifier::new("libzypp").unwrap(),
            before_revision: Some(Revision::new("17.35").unwrap()),
            after_revision: Some(Revision::new("17.36").unwrap()),
        });
        let mut first_manifest = manifest();
        first_manifest
            .set_provenance(vec![zypper.clone(), libzypp.clone()], provider.clone(), None)
            .unwrap();
        let json = String::from_utf8(first_manifest.canonical_payload_bytes().unwrap()).unwrap();
        assert!(json.contains("\"package_name\":\"zypper\""));
        assert!(json.contains("\"before_nevra\":\"1.0-1.x86_64\""));
        assert!(json.contains("\"after_nevra\":\"1.1-1.x86_64\""));
        assert!(json.contains("\"provider_id\":\"libzypp\""));
        assert!(json.contains("\"before_revision\":\"17.35\""));
        assert!(json.contains("\"after_revision\":\"17.36\""));

        let mut reversed = manifest();
        reversed.set_provenance(vec![libzypp.clone(), zypper.clone()], provider, None).unwrap();
        assert_eq!(
            first_manifest.canonical_payload_bytes().unwrap(),
            reversed.canonical_payload_bytes().unwrap()
        );

        assert_eq!(
            first_manifest.set_provenance(vec![zypper.clone(), zypper], None, None),
            Err(ValidationError::DuplicatePackageRevision)
        );
        assert_eq!(PackageName::new("7zip").unwrap().as_str(), "7zip");
        assert!(PackageName::new("../outside").is_err());
    }
}
