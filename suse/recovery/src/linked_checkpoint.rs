//! Read-back of both partners through the protected transaction catalog.
//!
//! A saved manifest flag is never a current availability observation. These
//! entry points reopen the exact generation and check its original native
//! receipt against the fixed Snapper path on every lookup. Boot coverage,
//! authorization, writer locking and restore execution remain separate gates.

use std::fs::File;

use crate::{
    Identifier,
    generation::{GenerationError, GenerationReader, GenerationStore, VerifiedGeneration},
    native_checkpoint::{
        HistoricalRootCheckpoint, NativeCheckpointError, RootSnapshotReceipt,
        verify_historical_root_checkpoint,
    },
};
use ryoku_snapper::{
    CatalogEntry, CatalogReader, PartnerState, SnapperBackend, SnapperCliBackend, SnapshotKind,
    SnapshotRecord, TransactionId,
};

/// Both exact partners were available when reopened. This is deliberately
/// distinct from a fresh checkpoint and cannot supply a new capture link.
pub struct HistoricalLinkedGeneration {
    generation: VerifiedGeneration,
    snapshot: HistoricalRootCheckpoint,
}

impl HistoricalLinkedGeneration {
    pub fn generation(&self) -> &VerifiedGeneration {
        &self.generation
    }

    pub fn snapshot(&self) -> &HistoricalRootCheckpoint {
        &self.snapshot
    }
}

/// A historical linked generation whose durable Snapper catalog row and live
/// tagged snapshot were also checked. This still does not establish boot
/// compatibility or permission to restore.
pub struct CatalogedLinkedGeneration {
    linked: HistoricalLinkedGeneration,
    entry: CatalogEntry,
}

impl CatalogedLinkedGeneration {
    pub fn generation(&self) -> &VerifiedGeneration {
        self.linked.generation()
    }

    pub fn snapshot(&self) -> &HistoricalRootCheckpoint {
        self.linked.snapshot()
    }

    pub fn catalog_entry(&self) -> &CatalogEntry {
        &self.entry
    }
}

/// A failed partner check invalidates linked availability. No error selects
/// another generation or snapshot, repairs state, or creates a replacement.
#[derive(Debug)]
pub enum HistoricalLookupError {
    GenerationMissing,
    GenerationInvalid(GenerationError),
    SnapshotReceiptMissing,
    SnapshotMissing,
    SnapshotMismatch,
    SnapshotWritable,
    SnapshotUnavailable(NativeCheckpointError),
    CatalogUnavailable,
    CatalogEntryMissing,
    CatalogMismatch,
    SnapperInventoryUnavailable,
}

impl std::fmt::Display for HistoricalLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "historical linked generation is unavailable: {self:?}")
    }
}
impl std::error::Error for HistoricalLookupError {}

impl GenerationReader {
    /// Use `open_state_mount` for protected machine-local reads. An offline
    /// `open_existing` reader does not establish protected mount placement.
    pub fn open_historical_transaction(
        &self,
        trusted_root: &File,
        transaction: &Identifier,
    ) -> Result<HistoricalLinkedGeneration, HistoricalLookupError> {
        open_linked(self.open_verified_transaction(transaction), |receipt| {
            verify_historical_root_checkpoint(trusted_root, receipt)
        })
        .map(|(generation, snapshot)| HistoricalLinkedGeneration { generation, snapshot })
    }

    /// Reopens both generation and root snapshot partners, then requires the
    /// exact protected Snapper catalog row and a current unique tagged single
    /// snapshot. A stored `Verified` marker alone is never treated as current.
    pub fn open_cataloged_transaction(
        &self,
        trusted_root: &File,
        transaction: &Identifier,
    ) -> Result<CatalogedLinkedGeneration, HistoricalLookupError> {
        let linked = self.open_historical_transaction(trusted_root, transaction)?;
        let directory = self
            .open_snapper_catalog_directory()
            .map_err(|_| HistoricalLookupError::CatalogUnavailable)?;
        let catalog = CatalogReader::open(&directory, self.expected_uid())
            .map_err(|_| HistoricalLookupError::CatalogUnavailable)?;
        let index = catalog.load().map_err(|_| HistoricalLookupError::CatalogUnavailable)?;
        let snapper_transaction = TransactionId::new(transaction.as_str())
            .map_err(|_| HistoricalLookupError::CatalogMismatch)?;
        let entry =
            index.get(&snapper_transaction).ok_or(HistoricalLookupError::CatalogEntryMissing)?;
        let manifest = linked.generation().manifest();
        if !catalog_entry_matches_manifest(entry, manifest) {
            return Err(HistoricalLookupError::CatalogMismatch);
        }
        let records = SnapperCliBackend
            .list(&entry.root_config)
            .map_err(|_| HistoricalLookupError::SnapperInventoryUnavailable)?;
        if !live_partner_matches(entry, &records) {
            return Err(HistoricalLookupError::SnapshotMissing);
        }
        Ok(CatalogedLinkedGeneration { linked, entry: entry.clone() })
    }
}

fn catalog_entry_matches_manifest(
    entry: &CatalogEntry,
    manifest: &crate::RecoveryManifest,
) -> bool {
    entry.transaction_id.as_str() == manifest.transaction_id.as_str()
        && entry.generation_id.as_str() == manifest.generation_id.as_str()
        && entry.root_config.as_str() == manifest.snapper_config.as_str()
        && entry.root_snapshot.as_str() == manifest.snapper_snapshot_id
        && entry.partner == PartnerState::Verified
}

fn live_partner_matches(entry: &CatalogEntry, records: &[SnapshotRecord]) -> bool {
    records
        .iter()
        .filter(|record| {
            record.config == entry.root_config
                && record.id == entry.root_snapshot
                && record.kind == SnapshotKind::Single
                && record.transaction_id.as_ref() == Some(&entry.transaction_id)
        })
        .count()
        == 1
}

impl GenerationStore {
    /// Reopen the sealed catalog/manifest association before observing its
    /// original snapshot. The store was constructed from verified state-mount
    /// identity; the caller still holds the machine writer/cleanup lock.
    pub fn open_historical_transaction(
        &self,
        trusted_root: &File,
        transaction: &Identifier,
    ) -> Result<HistoricalLinkedGeneration, HistoricalLookupError> {
        open_linked(self.open_verified_transaction(transaction), |receipt| {
            verify_historical_root_checkpoint(trusted_root, receipt)
        })
        .map(|(generation, snapshot)| HistoricalLinkedGeneration { generation, snapshot })
    }
}

fn open_linked<T>(
    generation: Result<VerifiedGeneration, GenerationError>,
    observe: impl FnOnce(&RootSnapshotReceipt) -> Result<T, NativeCheckpointError>,
) -> Result<(VerifiedGeneration, T), HistoricalLookupError> {
    let generation = generation.map_err(|error| match error {
        GenerationError::Filesystem(rustix::io::Errno::NOENT) => {
            HistoricalLookupError::GenerationMissing
        }
        error => HistoricalLookupError::GenerationInvalid(error),
    })?;
    let receipt = generation
        .manifest()
        .root_snapshot_receipt()
        .ok_or(HistoricalLookupError::SnapshotReceiptMissing)?;
    let snapshot = observe(receipt).map_err(|error| match error {
        NativeCheckpointError::Filesystem(rustix::io::Errno::NOENT) => {
            HistoricalLookupError::SnapshotMissing
        }
        NativeCheckpointError::IdentityMismatch
        | NativeCheckpointError::NotSubvolumeRoot
        | NativeCheckpointError::UnsupportedLink
        | NativeCheckpointError::Filesystem(rustix::io::Errno::LOOP)
        | NativeCheckpointError::Filesystem(rustix::io::Errno::NOTDIR) => {
            HistoricalLookupError::SnapshotMismatch
        }
        NativeCheckpointError::WritableSnapshot => HistoricalLookupError::SnapshotWritable,
        error => HistoricalLookupError::SnapshotUnavailable(error),
    })?;
    Ok((generation, snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EntryKind, FileRecord, OperationReason, RecoveryManifest, RelativePath, StateRoot,
        VerificationStatus, generation::Payloads,
    };
    use std::{
        cell::Cell,
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        path: PathBuf,
        store: GenerationStore,
    }

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ryoku-historical-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            let store = GenerationStore::open_for_test(&File::open(&path).unwrap()).unwrap();
            Self { path, store }
        }

        fn reader(&self) -> GenerationReader {
            let root = File::open(&self.path).unwrap();
            GenerationReader::open_existing(&root, rustix::fs::fstat(&root).unwrap().st_uid)
                .unwrap()
        }

        fn publish(&self, manifest: &RecoveryManifest) {
            self.store.publish(manifest, &Payloads::new()).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for generation in fs::read_dir(self.path.join("generations")).unwrap().flatten() {
                for path in [generation.path(), generation.path().join("payloads")] {
                    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                }
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn synthetic_receipt() -> RootSnapshotReceipt {
        // Synthetic data tests serialization/association only; it is never a
        // NativeRootCheckpoint and cannot supply native availability evidence.
        serde_json::from_value(serde_json::json!({
            "schema": 1,
            "link": {
                "snapper_config": "root",
                "snapper_snapshot_id": "42",
                "filesystem_uuid": "03030303-0303-0303-0303-030303030303"
            },
            "subvolume_id": 300,
            "subvolume_uuid": "02020202-0202-0202-0202-020202020202",
            "origin_root_uuid": "01010101-0101-0101-0101-010101010101"
        }))
        .unwrap()
    }

    fn manifest(native: bool) -> RecoveryManifest {
        let receipt = synthetic_receipt();
        let mut manifest = RecoveryManifest::new(
            Identifier::new("historical-tx").unwrap(),
            Identifier::new("historical-gen").unwrap(),
            None,
            receipt.link().clone(),
            OperationReason::ProfileApply,
            "2026-10-15T00:00:00Z",
        )
        .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::EtcRyoku,
                    RelativePath::new("missing.toml").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        if native {
            manifest.attach_root_snapshot_receipt(receipt).unwrap();
        }
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        manifest.seal().unwrap();
        manifest
    }

    fn catalog_entry(manifest: &RecoveryManifest) -> CatalogEntry {
        CatalogEntry {
            transaction_id: TransactionId::new(manifest.transaction_id.as_str()).unwrap(),
            root_config: ryoku_snapper::ConfigName::new(manifest.snapper_config.as_str()).unwrap(),
            root_snapshot: ryoku_snapper::SnapshotId::new(manifest.snapper_snapshot_id.as_str())
                .unwrap(),
            generation_id: ryoku_snapper::GenerationId::new(manifest.generation_id.as_str())
                .unwrap(),
            capability: ryoku_snapper::LayoutCapability::ConfigurationOnly,
            partner: PartnerState::Verified,
            manual_checkpoint: false,
            known_working: false,
        }
    }

    #[test]
    fn cataloged_link_verification_requires_exact_persisted_and_live_partners() {
        let manifest = manifest(true);
        let entry = catalog_entry(&manifest);
        let record = SnapshotRecord {
            config: entry.root_config.clone(),
            id: entry.root_snapshot.clone(),
            kind: SnapshotKind::Single,
            pre_snapshot: None,
            transaction_id: Some(entry.transaction_id.clone()),
        };
        assert!(catalog_entry_matches_manifest(&entry, &manifest));
        assert!(live_partner_matches(&entry, std::slice::from_ref(&record)));

        let mut wrong_generation = entry.clone();
        wrong_generation.generation_id =
            ryoku_snapper::GenerationId::new("other-generation").unwrap();
        assert!(!catalog_entry_matches_manifest(&wrong_generation, &manifest));

        let mut unverified = entry.clone();
        unverified.partner = PartnerState::Missing;
        assert!(!catalog_entry_matches_manifest(&unverified, &manifest));
        assert!(!live_partner_matches(&entry, &[]));
        assert!(!live_partner_matches(&entry, &[record.clone(), record.clone()]));

        let mut wrong_kind = record.clone();
        wrong_kind.kind = SnapshotKind::Pre;
        assert!(!live_partner_matches(&entry, &[wrong_kind]));
        let mut wrong_transaction = record;
        wrong_transaction.transaction_id = None;
        assert!(!live_partner_matches(&entry, &[wrong_transaction]));
    }

    #[test]
    fn original_receipt_survives_publication_and_fresh_reader_reopen() {
        let fixture = Fixture::new();
        let expected = manifest(true);
        fixture.publish(&expected);
        fixture.publish(&expected); // exact identity remains idempotent
        let catalog: serde_json::Value = serde_json::from_slice(
            &fs::read(fixture.path.join("catalog/historical-tx.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(catalog["schema_version"], 3);
        let reader = fixture.reader();
        let opened = open_linked(reader.open_verified_transaction(&expected.transaction_id), |r| {
            assert_eq!(Some(r), expected.root_snapshot_receipt());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            opened.0.manifest().canonical_bytes().unwrap(),
            expected.canonical_bytes().unwrap()
        );
        // Fresh observation always runs; no persisted verification flag or
        // previously successful lookup can mask later snapshot deletion.
        assert!(matches!(
            open_linked(reader.open_verified_transaction(&expected.transaction_id), |_| {
                Err::<(), _>(NativeCheckpointError::Filesystem(rustix::io::Errno::NOENT))
            }),
            Err(HistoricalLookupError::SnapshotMissing)
        ));
    }

    #[test]
    fn legacy_generation_cannot_acquire_provenance_from_current_observation() {
        let fixture = Fixture::new();
        let legacy = manifest(false);
        fixture.publish(&legacy);
        let reader = fixture.reader();
        let observed = Cell::new(false);
        assert!(matches!(
            open_linked(reader.open_verified_transaction(&legacy.transaction_id), |_| {
                observed.set(true);
                Ok(())
            }),
            Err(HistoricalLookupError::SnapshotReceiptMissing)
        ));
        assert!(!observed.get());
        let bytes = legacy.canonical_bytes().unwrap();
        assert!(!String::from_utf8(bytes.clone()).unwrap().contains("root_snapshot_receipt"));
        assert_eq!(RecoveryManifest::parse_verified_json(&bytes).unwrap().schema_version, 1);
        assert!(matches!(
            fixture.store.publish(&manifest(true), &Payloads::new()),
            Err(GenerationError::GenerationCollision)
        ));
    }

    #[test]
    fn receipt_is_sealed_and_cross_bound_to_config_number_and_filesystem() {
        let expected = manifest(true);
        let bytes = expected.canonical_bytes().unwrap();
        let decoded = RecoveryManifest::parse_verified_json(&bytes).unwrap();
        assert_eq!(decoded.root_snapshot_receipt(), expected.root_snapshot_receipt());
        for field in ["snapper_config", "snapper_snapshot_id", "filesystem_uuid"] {
            let mut altered = serde_json::to_value(&expected).unwrap();
            altered[field] = match field {
                "snapper_config" => "home",
                "snapper_snapshot_id" => "43",
                _ => "04040404-0404-0404-0404-040404040404",
            }
            .into();
            assert!(matches!(
                RecoveryManifest::parse_verified_json(&serde_json::to_vec(&altered).unwrap()),
                Err(crate::ManifestError::Validation(
                    crate::ValidationError::InvalidSnapshotReceipt
                ))
            ));
        }
        let mut altered = serde_json::to_value(&expected).unwrap();
        altered["root_snapshot_receipt"]["subvolume_id"] = 301.into();
        assert!(matches!(
            RecoveryManifest::parse_verified_json(&serde_json::to_vec(&altered).unwrap()),
            Err(crate::ManifestError::DigestMismatch)
        ));
        let mut missing = serde_json::to_value(&expected).unwrap();
        missing.as_object_mut().unwrap().remove("root_snapshot_receipt");
        assert!(
            RecoveryManifest::parse_verified_json(&serde_json::to_vec(&missing).unwrap()).is_err()
        );
    }

    #[test]
    fn corrupt_catalog_or_generation_stops_before_snapshot_observation() {
        let fixture = Fixture::new();
        let expected = manifest(true);
        fixture.publish(&expected);
        let reader = fixture.reader();
        let catalog_path = fixture.path.join("catalog/historical-tx.json");
        let original = fs::read(&catalog_path).unwrap();
        for changed in
            [b"{".to_vec(), original.iter().map(|b| if *b == b'3' { b'2' } else { *b }).collect()]
        {
            fs::set_permissions(&catalog_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&catalog_path, changed).unwrap();
            fs::set_permissions(&catalog_path, fs::Permissions::from_mode(0o400)).unwrap();
            assert!(matches!(
                open_linked(
                    reader.open_verified_transaction(&expected.transaction_id),
                    |_| -> Result<(), _> { panic!("corrupt linkage must prevent observation") }
                ),
                Err(HistoricalLookupError::GenerationInvalid(_))
            ));
        }
        fs::set_permissions(&catalog_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&catalog_path, &original).unwrap();
        fs::set_permissions(&catalog_path, fs::Permissions::from_mode(0o400)).unwrap();
        fs::rename(
            fixture.path.join("generations/historical-gen"),
            fixture.path.join("generations/other-gen"),
        )
        .unwrap();
        assert!(matches!(
            open_linked(
                reader.open_verified_transaction(&expected.transaction_id),
                |_| -> Result<(), _> { panic!("missing generation must prevent observation") }
            ),
            Err(HistoricalLookupError::GenerationMissing)
        ));
        assert!(reader.list_transactions().unwrap().contains(&expected.transaction_id));
    }

    #[test]
    fn substituted_or_writable_snapshot_is_never_available() {
        let fixture = Fixture::new();
        let expected = manifest(true);
        fixture.publish(&expected);
        for failure in [
            NativeCheckpointError::IdentityMismatch,
            NativeCheckpointError::NotSubvolumeRoot,
            NativeCheckpointError::Filesystem(rustix::io::Errno::LOOP),
        ] {
            assert!(matches!(
                open_linked(
                    fixture.store.open_verified_transaction(&expected.transaction_id),
                    |_| Err::<(), _>(failure)
                ),
                Err(HistoricalLookupError::SnapshotMismatch)
            ));
        }
        assert!(matches!(
            open_linked(
                fixture.store.open_verified_transaction(&expected.transaction_id),
                |_| Err::<(), _>(NativeCheckpointError::WritableSnapshot)
            ),
            Err(HistoricalLookupError::SnapshotWritable)
        ));
    }
}
