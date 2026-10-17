//! Immutable configuration bytes prepared from a verified durable generation.
//!
//! This module does not open restore targets or perform writes. It does not authorize
//! restore, establish compatibility, create a current-state checkpoint, advance
//! the journal, stop writers, roll back root, or gate normal session startup.
//! Resume must reopen the exact durable source and repeat preparation; an
//! in-memory bundle is not itself a durable `ContentStaged` phase record.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    EntryKind, FileRecord, Sha256, XattrName,
    generation::{GenerationError, VerifiedGeneration},
    metadata::{AclPayload, InvalidMetadataPayload, entry_metadata},
    restore::{RestorePlan, RestoreScope},
};

#[cfg(test)]
use crate::StateRoot;

const MAX_STAGED_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;

#[cfg(test)]
#[path = "inode_metadata_tests.rs"]
mod inode_metadata_tests;

#[cfg(test)]
#[path = "disk_stage_tests.rs"]
mod disk_stage_tests;

/// One exact declared entry, with retained raw metadata and content. Shared
/// digests use shared immutable allocations rather than per-path byte copies.
pub struct StagedEntry {
    record: FileRecord,
    content: Option<Arc<[u8]>>,
    acl: Option<Arc<AclPayload>>,
    xattrs: BTreeMap<XattrName, Arc<[u8]>>,
}

impl StagedEntry {
    pub fn record(&self) -> &FileRecord {
        &self.record
    }
    pub fn content(&self) -> Option<&[u8]> {
        self.content.as_deref()
    }
    pub fn acl(&self) -> Option<&AclPayload> {
        self.acl.as_deref()
    }
    pub fn xattrs(&self) -> impl Iterator<Item = (XattrName, &[u8])> {
        self.xattrs.iter().map(|(name, value)| (*name, value.as_ref()))
    }
}

/// Fully prepared configuration subset bound to one exact plan and manifest.
/// Construction is private so callers cannot supply unchecked source bytes.
pub struct StagedConfiguration {
    plan: RestorePlan,
    manifest_sha256: Sha256,
    entries: Vec<StagedEntry>,
    raw_payload_bytes: usize,
}

impl StagedConfiguration {
    pub fn plan(&self) -> &RestorePlan {
        &self.plan
    }
    pub fn manifest_sha256(&self) -> &Sha256 {
        &self.manifest_sha256
    }
    pub fn entries(&self) -> &[StagedEntry] {
        &self.entries
    }
    /// Distinct raw payload bytes loaded, counted once per digest. Content,
    /// labels and decoded ACLs remain shared in the prepared bundle.
    pub fn raw_payload_bytes(&self) -> usize {
        self.raw_payload_bytes
    }
}

#[derive(Debug)]
pub enum StageError {
    SystemOnlyScope,
    PlanMismatch,
    EmptyConfiguration,
    PayloadBudgetExceeded,
    Generation(GenerationError),
    Metadata(InvalidMetadataPayload),
}

impl std::fmt::Display for StageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SystemOnlyScope => {
                formatter.write_str("system-only restore has no configuration stage")
            }
            Self::PlanMismatch => {
                formatter.write_str("restore plan does not match the verified generation")
            }
            Self::EmptyConfiguration => formatter.write_str("configuration restore scope is empty"),
            Self::PayloadBudgetExceeded => formatter.write_str("restore payload budget exceeded"),
            Self::Generation(error) => write!(formatter, "restore generation read failed: {error}"),
            Self::Metadata(error) => write!(formatter, "restore metadata decode failed: {error}"),
        }
    }
}
impl std::error::Error for StageError {}
impl From<GenerationError> for StageError {
    fn from(error: GenerationError) -> Self {
        Self::Generation(error)
    }
}
impl From<InvalidMetadataPayload> for StageError {
    fn from(error: InvalidMetadataPayload) -> Self {
        Self::Metadata(error)
    }
}

/// Prepares every entry in the requested configuration scope, or returns no
/// bundle. Immutable profile-generation data remains in the durable source;
/// it is not a writable configuration root. A verified plan still requires
/// separate caller-established compatibility and current-state checkpoint
/// evidence before any destructive phase may begin.
pub fn stage_configuration(
    generation: &VerifiedGeneration,
    plan: &RestorePlan,
) -> Result<StagedConfiguration, StageError> {
    if plan.scope == RestoreScope::SystemOnly {
        return Err(StageError::SystemOnlyScope);
    }
    let manifest = generation.manifest();
    if manifest.manifest_sha256.as_ref() != Some(&plan.source_manifest_sha256)
        || plan.transaction_id != manifest.transaction_id
        || plan.generation_id != manifest.generation_id
        || plan.snapper_config != manifest.snapper_config
        || plan.snapshot_id != manifest.snapper_snapshot_id
        || plan.filesystem_uuid != manifest.filesystem_uuid
    {
        return Err(StageError::PlanMismatch);
    }
    let records: Vec<_> =
        manifest.files().iter().filter(|record| record.root.is_configuration_state()).collect();
    if records.len() != plan.configuration_entries as usize {
        return Err(StageError::PlanMismatch);
    }
    if records.is_empty() {
        return Err(StageError::EmptyConfiguration);
    }

    let mut payloads = BTreeMap::new();
    let mut acls = BTreeMap::new();
    let mut raw_payload_bytes = 0;
    let mut entries = Vec::with_capacity(records.len());
    for record in records {
        let content = if let EntryKind::RegularFile { sha256, .. } = &record.kind {
            Some(load_payload(generation, sha256, &mut payloads, &mut raw_payload_bytes)?)
        } else {
            None
        };
        let mut acl = None;
        let mut xattrs = BTreeMap::new();
        if let Some(metadata) = entry_metadata(&record.kind) {
            if let Some(digest) = &metadata.acl_sha256 {
                let value = if let Some(value) = acls.get(digest) {
                    Arc::clone(value)
                } else {
                    let bytes =
                        load_payload(generation, digest, &mut payloads, &mut raw_payload_bytes)?;
                    let value = Arc::new(AclPayload::decode(&bytes)?);
                    acls.insert(digest.clone(), Arc::clone(&value));
                    value
                };
                acl = Some(value);
            }
            for (name, digest) in &metadata.extended.values {
                xattrs.insert(
                    *name,
                    load_payload(generation, digest, &mut payloads, &mut raw_payload_bytes)?,
                );
            }
        }
        entries.push(StagedEntry { record: record.clone(), content, acl, xattrs });
    }
    Ok(StagedConfiguration {
        plan: plan.clone(),
        manifest_sha256: manifest.manifest_sha256.clone().ok_or(StageError::PlanMismatch)?,
        entries,
        raw_payload_bytes,
    })
}

fn load_payload(
    generation: &VerifiedGeneration,
    digest: &Sha256,
    cache: &mut BTreeMap<Sha256, Arc<[u8]>>,
    total: &mut usize,
) -> Result<Arc<[u8]>, StageError> {
    if let Some(bytes) = cache.get(digest) {
        return Ok(Arc::clone(bytes));
    }
    let bytes = generation.read_payload(digest)?;
    *total = total.checked_add(bytes.len()).ok_or(StageError::PayloadBudgetExceeded)?;
    if *total > MAX_STAGED_PAYLOAD_BYTES {
        return Err(StageError::PayloadBudgetExceeded);
    }
    let bytes: Arc<[u8]> = bytes.into();
    cache.insert(digest.clone(), Arc::clone(&bytes));
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EntryMetadata, ExtendedMetadata, Identifier, OperationReason, RecoveryManifest,
        RelativePath, SnapshotLink, StateRoot, VerificationStatus,
        generation::{GenerationStore, Payloads},
    };
    use sha2::{Digest, Sha256 as Hash};
    use std::{
        fs::{self, File},
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const CONTENT: &[u8] = b"theme fixture";
    const PROFILE: &[u8] = b"immutable profile fixture";
    const LABEL: &[u8] = b"synthetic_u:object_r:ryoku_t:s0\0";

    struct Fixture {
        path: PathBuf,
        store: GenerationStore,
        manifest: RecoveryManifest,
        payloads: Payloads,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fn writable(path: &Path) {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
                for entry in fs::read_dir(path).unwrap() {
                    let entry = entry.unwrap();
                    if entry.file_type().unwrap().is_dir() {
                        writable(&entry.path());
                    }
                }
            }
            writable(&self.path);
            fs::remove_dir_all(&self.path).unwrap();
        }
    }

    fn digest(bytes: &[u8]) -> Sha256 {
        Sha256::new(
            Hash::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        )
        .unwrap()
    }

    fn metadata(acl: Option<Sha256>) -> EntryMetadata {
        EntryMetadata {
            uid: 1000,
            gid: 1000,
            mode: 0o700,
            mtime_unix_ns: 123,
            acl_sha256: acl,
            extended: ExtendedMetadata {
                values: BTreeMap::from([(XattrName::SelinuxLabel, digest(LABEL))]),
            },
        }
    }

    fn fixture() -> Fixture {
        fixture_with_timestamp("2026-10-15T00:00:00Z")
    }

    fn fixture_with_timestamp(created_utc: &str) -> Fixture {
        let path = std::env::temp_dir().join(format!(
            "ryoku-configuration-stage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let store = GenerationStore::open_for_test(&File::open(&path).unwrap()).unwrap();
        let mut manifest = RecoveryManifest::new(
            Identifier::new("transaction-stage").unwrap(),
            Identifier::new("generation-stage").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            created_utc,
        )
        .unwrap();
        let acl =
            AclPayload { access: Some(vec![2, 0, 0, 0]), default: None }.encode().unwrap().unwrap();
        let directory_acl =
            AclPayload { access: Some(vec![2, 0, 0, 0]), default: Some(vec![2, 0, 0, 0]) }
                .encode()
                .unwrap()
                .unwrap();
        for (name, kind) in [
            ("settings", EntryKind::Directory { metadata: metadata(Some(digest(&directory_acl))) }),
            (
                "settings/theme.json",
                EntryKind::RegularFile {
                    metadata: metadata(Some(digest(&acl))),
                    size: CONTENT.len() as u64,
                    sha256: digest(CONTENT),
                    hard_link_group: Some(Identifier::new("theme-links").unwrap()),
                },
            ),
            (
                "settings/theme.backup",
                EntryKind::RegularFile {
                    metadata: metadata(Some(digest(&acl))),
                    size: CONTENT.len() as u64,
                    sha256: digest(CONTENT),
                    hard_link_group: Some(Identifier::new("theme-links").unwrap()),
                },
            ),
            (
                "current",
                EntryKind::Symlink {
                    metadata: metadata(None),
                    target: "settings/theme.json".into(),
                },
            ),
            ("settings/unused", EntryKind::Absent),
        ] {
            manifest
                .add_file(
                    FileRecord::new(
                        StateRoot::UserRyokuConfig,
                        RelativePath::new(name).unwrap(),
                        kind,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::StateGeneration,
                    RelativePath::new("profile/default.json").unwrap(),
                    EntryKind::RegularFile {
                        metadata: metadata(None),
                        size: PROFILE.len() as u64,
                        sha256: digest(PROFILE),
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
        let payloads = Payloads::from([
            (digest(CONTENT), CONTENT.to_vec()),
            (digest(PROFILE), PROFILE.to_vec()),
            (digest(LABEL), LABEL.to_vec()),
            (digest(&acl), acl),
            (digest(&directory_acl), directory_acl),
        ]);
        store.publish(&manifest, &payloads).unwrap();
        fs::write(path.join("unrelated"), b"sentinel").unwrap();
        Fixture { path, store, manifest, payloads }
    }

    fn plan(manifest: &RecoveryManifest) -> RestorePlan {
        RestorePlan {
            source_manifest_sha256: manifest.manifest_sha256.clone().unwrap(),
            transaction_id: manifest.transaction_id.clone(),
            generation_id: manifest.generation_id.clone(),
            snapper_config: manifest.snapper_config.clone(),
            snapshot_id: manifest.snapper_snapshot_id.clone(),
            filesystem_uuid: manifest.filesystem_uuid.clone(),
            scope: RestoreScope::ConfigurationOnly,
            configuration_entries: 5,
        }
    }

    #[test]
    fn stages_complete_configuration_with_shared_content_and_raw_metadata() {
        let source = fixture();
        let generation =
            source.store.open_verified_transaction(&source.manifest.transaction_id).unwrap();
        let plan = plan(&source.manifest);
        let staged = stage_configuration(&generation, &plan).unwrap();
        assert_eq!(staged.plan(), &plan);
        assert_eq!(Some(staged.manifest_sha256()), source.manifest.manifest_sha256.as_ref());
        assert_eq!(staged.entries().len(), 5);
        let expected_bytes: usize = source
            .payloads
            .iter()
            .filter(|(key, _)| **key != digest(PROFILE))
            .map(|(_, value)| value.len())
            .sum();
        assert_eq!(staged.raw_payload_bytes(), expected_bytes);
        let files: Vec<_> =
            staged.entries().iter().filter(|entry| entry.content().is_some()).collect();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].content().unwrap(), CONTENT);
        assert!(Arc::ptr_eq(
            files[0].content.as_ref().unwrap(),
            files[1].content.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(files[0].acl.as_ref().unwrap(), files[1].acl.as_ref().unwrap()));
        for entry in staged.entries() {
            match &entry.record().kind {
                EntryKind::Absent => {
                    assert!(entry.content().is_none());
                    assert!(entry.acl().is_none());
                    assert_eq!(entry.xattrs().count(), 0);
                }
                EntryKind::Directory { .. } => assert!(entry.acl().unwrap().default.is_some()),
                EntryKind::RegularFile { hard_link_group, .. } => {
                    assert_eq!(hard_link_group.as_ref().unwrap().as_str(), "theme-links")
                }
                EntryKind::Symlink { target, .. } => {
                    assert_eq!(target, "settings/theme.json");
                    assert!(entry.content().is_none());
                }
            }
            if !matches!(entry.record().kind, EntryKind::Absent) {
                assert_eq!(
                    entry.xattrs().collect::<Vec<_>>(),
                    vec![(XattrName::SelinuxLabel, LABEL)]
                );
            }
        }
        assert_eq!(fs::read(source.path.join("unrelated")).unwrap(), b"sentinel");
    }

    #[test]
    fn resumed_plan_rejects_a_different_verified_manifest_with_identical_linkage() {
        let original = fixture();
        let selected = plan(&original.manifest);
        let replacement = fixture_with_timestamp("2026-10-15T01:00:00Z");
        let generation = replacement
            .store
            .open_verified_transaction(&replacement.manifest.transaction_id)
            .unwrap();
        assert_eq!(selected.transaction_id, generation.manifest().transaction_id);
        assert_ne!(
            Some(&selected.source_manifest_sha256),
            generation.manifest().manifest_sha256.as_ref()
        );
        assert!(matches!(
            stage_configuration(&generation, &selected),
            Err(StageError::PlanMismatch)
        ));
    }

    #[test]
    fn refuses_every_mismatched_plan_link_and_system_only_scope() {
        let source = fixture();
        let generation =
            source.store.open_verified_transaction(&source.manifest.transaction_id).unwrap();
        let original = plan(&source.manifest);
        let mut candidates = Vec::new();
        let mut wrong = original.clone();
        wrong.transaction_id = Identifier::new("other-transaction").unwrap();
        candidates.push(wrong);
        let mut wrong = original.clone();
        wrong.generation_id = Identifier::new("other-generation").unwrap();
        candidates.push(wrong);
        let mut wrong = original.clone();
        wrong.snapper_config = Identifier::new("other-root").unwrap();
        candidates.push(wrong);
        let mut wrong = original.clone();
        wrong.snapshot_id = "43".into();
        candidates.push(wrong);
        let mut wrong = original.clone();
        wrong.filesystem_uuid = "11111111-1111-1111-1111-111111111111".into();
        candidates.push(wrong);
        let mut wrong = original.clone();
        wrong.configuration_entries = 4;
        candidates.push(wrong);
        for wrong in candidates {
            assert!(matches!(
                stage_configuration(&generation, &wrong),
                Err(StageError::PlanMismatch)
            ));
        }
        let mut system_only = original.clone();
        system_only.scope = RestoreScope::SystemOnly;
        assert!(matches!(
            stage_configuration(&generation, &system_only),
            Err(StageError::SystemOnlyScope)
        ));
        let mut linked = original;
        linked.scope = RestoreScope::CompleteLinked;
        assert!(stage_configuration(&generation, &linked).is_ok());
    }

    #[test]
    fn corrupted_source_never_returns_a_partial_stage_and_prepared_bytes_stay_immutable() {
        let source = fixture();
        let generation =
            source.store.open_verified_transaction(&source.manifest.transaction_id).unwrap();
        let plan = plan(&source.manifest);
        let staged = stage_configuration(&generation, &plan).unwrap();
        for (payload_digest, original) in &source.payloads {
            if *payload_digest == digest(PROFILE) {
                continue;
            }
            let blob = source.path.join(format!(
                "generations/generation-stage/payloads/{}.blob",
                payload_digest.as_str()
            ));
            fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&blob, b"corrupt bytes").unwrap();
            fs::set_permissions(&blob, fs::Permissions::from_mode(0o400)).unwrap();
            assert!(matches!(
                stage_configuration(&generation, &plan),
                Err(StageError::Generation(_))
            ));
            fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&blob, original).unwrap();
            fs::set_permissions(&blob, fs::Permissions::from_mode(0o400)).unwrap();
        }
        for entry in staged.entries().iter().filter(|entry| entry.content().is_some()) {
            assert_eq!(entry.content().unwrap(), CONTENT);
        }
        assert_eq!(fs::read(source.path.join("unrelated")).unwrap(), b"sentinel");
    }

    #[test]
    fn retained_profile_data_does_not_become_a_writable_configuration_scope() {
        let source = fixture();
        let mut manifest = source.manifest.clone();
        manifest.transaction_id = Identifier::new("transaction-profile-only").unwrap();
        manifest.generation_id = Identifier::new("generation-profile-only").unwrap();
        manifest.files.retain(|record| record.root == StateRoot::StateGeneration);
        manifest.seal().unwrap();
        let payloads =
            Payloads::from([(digest(PROFILE), PROFILE.to_vec()), (digest(LABEL), LABEL.to_vec())]);
        source.store.publish(&manifest, &payloads).unwrap();
        let generation = source.store.open_verified_transaction(&manifest.transaction_id).unwrap();
        let mut plan = plan(&manifest);
        plan.configuration_entries = 0;
        assert!(matches!(
            stage_configuration(&generation, &plan),
            Err(StageError::EmptyConfiguration)
        ));
    }
}
