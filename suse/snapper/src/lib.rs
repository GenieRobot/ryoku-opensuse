//! Typed primitives for Snapper layout discovery and checkpoint correlation.
//!
//! This crate deliberately contains no command runner, D-Bus client, durable
//! catalog implementation, or mutation executor. A caller must gather
//! trustworthy observations and provide an authorized backend before any
//! operation can affect a system.

use std::collections::BTreeMap;

/// Filesystem type observed for a mount or subvolume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilesystemKind {
    Btrfs,
    Other,
    Unknown,
}

/// Whether a Snapper config is present and usable for a path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigState {
    Missing,
    PresentButDisabled,
    Ready,
    Unknown,
}

/// Boot partition relationship to the root filesystem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootLayout {
    /// `/boot` is part of the root subvolume and is captured with it.
    IncludedInRoot,
    /// `/boot` is a separate mount with its own proven snapshot coverage.
    SeparateAndSnapshotted,
    /// `/boot` is separate and is not known to be snapshotted.
    SeparateUnsnapshotted,
    /// The observation could not establish the layout.
    Unknown,
}

/// Inputs from a read-only host probe. The probe that constructs this value
/// must validate mount identity and Snapper configuration independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutObservation {
    pub root_filesystem: FilesystemKind,
    pub root_is_btrfs_subvolume: bool,
    pub root_snapper: ConfigState,
    pub ryoku_config_filesystem: FilesystemKind,
    pub ryoku_config_snapper: ConfigState,
    pub boot: BootLayout,
}

/// Root/boot protection that can be claimed from the supplied layout
/// observations. `FullSystem` says nothing about Ryoku generation capture or
/// linkage; catalog completeness checks that separate partner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutCapability {
    FullSystem,
    ConfigurationOnly,
    Unsupported,
}

/// Stable explanation accompanying a capability decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutReason {
    RootAndBootCovered,
    OnlyRyokuConfigurationCovered,
    RootNotBtrfs,
    RootNotSubvolume,
    RootSnapperUnavailable,
    BootNotCovered,
    UnknownObservation,
    NoSupportedSnapperTarget,
}

/// Result of classifying a layout. Unsupported and configuration-only results
/// can never be promoted to full-system protection by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutAssessment {
    pub capability: LayoutCapability,
    pub reason: LayoutReason,
}

/// Classifies observations without probing the machine or guessing defaults.
pub fn assess_layout(observation: LayoutObservation) -> LayoutAssessment {
    let config_only = observation.ryoku_config_filesystem == FilesystemKind::Btrfs
        && observation.ryoku_config_snapper == ConfigState::Ready;

    let full_system = matches!(
        (
            observation.root_filesystem,
            observation.root_is_btrfs_subvolume,
            observation.root_snapper,
            observation.boot,
        ),
        (FilesystemKind::Btrfs, true, ConfigState::Ready, BootLayout::IncludedInRoot)
            | (FilesystemKind::Btrfs, true, ConfigState::Ready, BootLayout::SeparateAndSnapshotted)
    );

    if full_system {
        return LayoutAssessment {
            capability: LayoutCapability::FullSystem,
            reason: LayoutReason::RootAndBootCovered,
        };
    }
    if config_only {
        return LayoutAssessment {
            capability: LayoutCapability::ConfigurationOnly,
            reason: LayoutReason::OnlyRyokuConfigurationCovered,
        };
    }

    let reason = if observation.root_filesystem == FilesystemKind::Unknown
        || observation.root_snapper == ConfigState::Unknown
        || observation.ryoku_config_filesystem == FilesystemKind::Unknown
        || observation.ryoku_config_snapper == ConfigState::Unknown
        || observation.boot == BootLayout::Unknown
    {
        LayoutReason::UnknownObservation
    } else if observation.root_filesystem != FilesystemKind::Btrfs {
        LayoutReason::RootNotBtrfs
    } else if !observation.root_is_btrfs_subvolume {
        LayoutReason::RootNotSubvolume
    } else if observation.root_snapper != ConfigState::Ready {
        LayoutReason::RootSnapperUnavailable
    } else if matches!(observation.boot, BootLayout::SeparateUnsnapshotted) {
        LayoutReason::BootNotCovered
    } else {
        LayoutReason::NoSupportedSnapperTarget
    };
    LayoutAssessment { capability: LayoutCapability::Unsupported, reason }
}

/// Validated Snapper config name.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ConfigName(String);

impl ConfigName {
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        if (1..=64).contains(&value.len())
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            Ok(Self(value))
        } else {
            Err(ValueError::InvalidConfigName)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Numeric Snapper snapshot ID, parsed without accepting command text.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SnapshotId(String);

impl SnapshotId {
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        if (1..=20).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit()) {
            Ok(Self(value))
        } else {
            Err(ValueError::InvalidSnapshotId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque transaction correlation token supplied by the package transaction.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TransactionId(String);

impl TransactionId {
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        if (1..=128).contains(&value.len())
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._:+-".contains(&byte))
        {
            Ok(Self(value))
        } else {
            Err(ValueError::InvalidTransactionId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated identity for an immutable Ryoku state generation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct GenerationId(String);

impl GenerationId {
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let bytes = value.as_bytes();
        if (1..=96).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes[bytes.len() - 1].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
            && !value.contains("--")
        {
            Ok(Self(value))
        } else {
            Err(ValueError::InvalidGenerationId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Input validation failure for public opaque values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueError {
    InvalidConfigName,
    InvalidSnapshotId,
    InvalidTransactionId,
    InvalidGenerationId,
}

/// Snapshot type visible through a typed Snapper adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotKind {
    Pre,
    Post,
    Single,
}

/// Snapshot metadata needed to correlate package-hook transactions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotRecord {
    pub config: ConfigName,
    pub id: SnapshotId,
    pub kind: SnapshotKind,
    pub transaction_id: Option<TransactionId>,
}

/// A typed request for a Snapper pre/post snapshot pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotPairRequest {
    pub config: ConfigName,
    pub transaction_id: TransactionId,
}

/// Opaque SnapshotPair operation boundary for an authorized concrete adapter.
/// No implementation in this crate executes the operation.
pub trait SnapperBackend {
    type Error;

    fn list(&self, config: &ConfigName) -> Result<Vec<SnapshotRecord>, Self::Error>;
    fn create_pre(&mut self, request: &SnapshotPairRequest) -> Result<SnapshotId, Self::Error>;
    fn create_post(
        &mut self,
        request: &SnapshotPairRequest,
        pre_snapshot: &SnapshotId,
    ) -> Result<SnapshotId, Self::Error>;
}

/// Failure while reading and validating the inventory for one pair query.
/// Backend errors are preserved; malformed backend inventories fail closed.
#[derive(Debug, Eq, PartialEq)]
pub enum PairQueryError<E> {
    Backend(E),
    ForeignConfigRecord,
    DuplicateSnapshotId(SnapshotId),
}

/// A fresh read-only observation of the requested transaction's hook pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairQuery {
    /// Complete inventory returned for the requested config.
    pub records: Vec<SnapshotRecord>,
    /// Correlation derived only from that fresh inventory.
    pub reconciliation: PairReconciliation,
}

/// Lists one config and reconciles its transaction pair without invoking any
/// backend mutation operation. A backend must return a config-scoped list;
/// foreign config records or duplicate snapshot IDs invalidate the whole
/// observation rather than being silently ignored.
pub fn query_pair<B: SnapperBackend>(
    backend: &B,
    request: &SnapshotPairRequest,
) -> Result<PairQuery, PairQueryError<B::Error>> {
    let records = backend.list(&request.config).map_err(PairQueryError::Backend)?;
    let mut ids = std::collections::BTreeSet::new();
    for record in &records {
        if record.config != request.config {
            return Err(PairQueryError::ForeignConfigRecord);
        }
        if !ids.insert(record.id.clone()) {
            return Err(PairQueryError::DuplicateSnapshotId(record.id.clone()));
        }
    }
    let reconciliation = reconcile_pair(request, &records);
    Ok(PairQuery { records, reconciliation })
}

/// Existing package-hook pair state for one transaction ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairCorrelation {
    Missing,
    PreOnly { pre: SnapshotId },
    Existing { pre: SnapshotId, post: SnapshotId },
    Ambiguous,
}

/// Safe disposition after matching an existing package-hook pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairDisposition {
    CreatePair,
    ReuseExistingPair,
    ReconcileBeforeProceeding,
}

impl PairCorrelation {
    /// An incomplete or ambiguous pair must be reconciled, never duplicated.
    pub fn disposition(&self) -> PairDisposition {
        match self {
            Self::Missing => PairDisposition::CreatePair,
            Self::Existing { .. } => PairDisposition::ReuseExistingPair,
            Self::PreOnly { .. } | Self::Ambiguous => PairDisposition::ReconcileBeforeProceeding,
        }
    }
}

/// Complete pure-Rust reconciliation result for a requested Snapper pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairReconciliation {
    /// Observed records matching both requested config and transaction ID.
    pub correlation: PairCorrelation,
    /// Safe next step; incomplete and ambiguous pairs are never duplicated.
    pub disposition: PairDisposition,
}

/// Reconciles a requested pair against observed Snapper records.
///
/// An existing package-hook pair is reused, so the caller does not create a
/// redundant nested pair. A missing exact match may be created. Partial,
/// duplicate, or otherwise ambiguous matching records require explicit
/// reconciliation before proceeding. This function only classifies supplied
/// observations; it does not query or mutate Snapper.
pub fn reconcile_pair(
    request: &SnapshotPairRequest,
    records: &[SnapshotRecord],
) -> PairReconciliation {
    let correlation = correlate_pair(&request.config, &request.transaction_id, records);
    let disposition = correlation.disposition();
    PairReconciliation { correlation, disposition }
}

/// Matches hook-created records by exact transaction ID and config, never by
/// descriptions or timestamp proximity. Duplicate IDs are treated as
/// ambiguous and must not trigger automatic snapshot creation.
pub fn correlate_pair(
    config: &ConfigName,
    transaction_id: &TransactionId,
    records: &[SnapshotRecord],
) -> PairCorrelation {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    for record in records {
        if &record.config != config || record.transaction_id.as_ref() != Some(transaction_id) {
            continue;
        }
        match record.kind {
            SnapshotKind::Pre => pre.push(record.id.clone()),
            SnapshotKind::Post => post.push(record.id.clone()),
            SnapshotKind::Single => {}
        }
    }
    pre.sort();
    post.sort();
    match (pre.as_slice(), post.as_slice()) {
        ([], []) => PairCorrelation::Missing,
        ([pre], []) => PairCorrelation::PreOnly { pre: pre.clone() },
        ([pre], [post]) if pre != post => {
            PairCorrelation::Existing { pre: pre.clone(), post: post.clone() }
        }
        _ => PairCorrelation::Ambiguous,
    }
}

/// Whether the system-side restore point still exists and passed verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartnerState {
    Missing,
    PresentUnverified,
    Verified,
}

/// One catalog row. This is an in-memory domain value, not a durable catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub transaction_id: TransactionId,
    pub root_config: ConfigName,
    pub root_snapshot: SnapshotId,
    pub generation_id: GenerationId,
    pub capability: LayoutCapability,
    pub partner: PartnerState,
    pub manual_checkpoint: bool,
    pub known_working: bool,
}

impl CatalogEntry {
    /// True only when system and generation partners are both present and
    /// verified, and the layout supports full-system protection.
    pub fn is_complete(&self, generation_present_and_verified: bool) -> bool {
        self.capability == LayoutCapability::FullSystem
            && self.partner == PartnerState::Verified
            && generation_present_and_verified
    }
}

/// In-memory index used to validate uniqueness and mark retention intent.
/// Persistence, authentication, fsync, rollback survival, and retention
/// execution are intentionally left to a later implementation.
#[derive(Clone, Debug, Default)]
pub struct CatalogIndex {
    entries: BTreeMap<TransactionId, CatalogEntry>,
}

impl CatalogIndex {
    pub fn insert(&mut self, entry: CatalogEntry) -> Result<(), CatalogEntry> {
        if self.entries.contains_key(&entry.transaction_id) {
            return Err(entry);
        }
        self.entries.insert(entry.transaction_id.clone(), entry);
        Ok(())
    }

    pub fn get(&self, id: &TransactionId) -> Option<&CatalogEntry> {
        self.entries.get(id)
    }

    pub fn remove(&mut self, id: &TransactionId) -> Option<CatalogEntry> {
        self.entries.remove(id)
    }

    pub fn mark_known_working(
        &mut self,
        id: &TransactionId,
        generation_present_and_verified: bool,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(id) else { return false };
        if !entry.is_complete(generation_present_and_verified) {
            return false;
        }
        entry.known_working = true;
        true
    }

    pub fn mark_manual_checkpoint(
        &mut self,
        id: &TransactionId,
        generation_present_and_verified: bool,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(id) else { return false };
        if !entry.is_complete(generation_present_and_verified) {
            return false;
        }
        entry.manual_checkpoint = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend {
        records: Result<Vec<SnapshotRecord>, &'static str>,
    }

    impl SnapperBackend for FakeBackend {
        type Error = &'static str;

        fn list(&self, _config: &ConfigName) -> Result<Vec<SnapshotRecord>, Self::Error> {
            self.records.clone()
        }

        fn create_pre(
            &mut self,
            _request: &SnapshotPairRequest,
        ) -> Result<SnapshotId, Self::Error> {
            panic!("read-only pair query must not create snapshots")
        }

        fn create_post(
            &mut self,
            _request: &SnapshotPairRequest,
            _pre_snapshot: &SnapshotId,
        ) -> Result<SnapshotId, Self::Error> {
            panic!("read-only pair query must not create snapshots")
        }
    }

    fn base_layout() -> LayoutObservation {
        LayoutObservation {
            root_filesystem: FilesystemKind::Btrfs,
            root_is_btrfs_subvolume: true,
            root_snapper: ConfigState::Ready,
            ryoku_config_filesystem: FilesystemKind::Btrfs,
            ryoku_config_snapper: ConfigState::Ready,
            boot: BootLayout::IncludedInRoot,
        }
    }

    fn config() -> ConfigName {
        ConfigName::new("root").unwrap()
    }

    fn transaction() -> TransactionId {
        TransactionId::new("zypp-1234").unwrap()
    }

    fn record(kind: SnapshotKind, id: &str, config: &str, transaction: &str) -> SnapshotRecord {
        SnapshotRecord {
            config: ConfigName::new(config).unwrap(),
            id: SnapshotId::new(id).unwrap(),
            kind,
            transaction_id: Some(TransactionId::new(transaction).unwrap()),
        }
    }

    #[test]
    fn full_system_requires_root_and_boot_coverage() {
        assert_eq!(assess_layout(base_layout()).capability, LayoutCapability::FullSystem);
        let mut separate_boot = base_layout();
        separate_boot.boot = BootLayout::SeparateUnsnapshotted;
        assert_eq!(assess_layout(separate_boot).capability, LayoutCapability::ConfigurationOnly);
        assert_eq!(
            assess_layout(separate_boot).reason,
            LayoutReason::OnlyRyokuConfigurationCovered
        );
    }

    #[test]
    fn unknown_and_non_btrfs_layouts_cannot_claim_full_system_protection() {
        let mut unknown = base_layout();
        unknown.boot = BootLayout::Unknown;
        assert_eq!(
            assess_layout(unknown),
            LayoutAssessment {
                capability: LayoutCapability::ConfigurationOnly,
                reason: LayoutReason::OnlyRyokuConfigurationCovered,
            }
        );
        let mut other = base_layout();
        other.root_filesystem = FilesystemKind::Other;
        assert_eq!(assess_layout(other).capability, LayoutCapability::ConfigurationOnly);
    }

    #[test]
    fn exact_transaction_pair_is_reused_and_incomplete_or_ambiguous_is_explicit() {
        let records = vec![
            record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
            record(SnapshotKind::Post, "11", "root", "zypp-1234"),
            record(SnapshotKind::Pre, "12", "root", "zypp-9999"),
            record(SnapshotKind::Post, "13", "home", "zypp-1234"),
        ];
        let existing = correlate_pair(&config(), &transaction(), &records);
        assert_eq!(
            existing,
            PairCorrelation::Existing {
                pre: SnapshotId::new("10").unwrap(),
                post: SnapshotId::new("11").unwrap(),
            }
        );
        assert_eq!(existing.disposition(), PairDisposition::ReuseExistingPair);
        assert_eq!(
            correlate_pair(&config(), &transaction(), &records[..0]),
            PairCorrelation::Missing
        );
        let pre_only = correlate_pair(
            &config(),
            &transaction(),
            &[record(SnapshotKind::Pre, "10", "root", "zypp-1234")],
        );
        assert_eq!(pre_only, PairCorrelation::PreOnly { pre: SnapshotId::new("10").unwrap() });
        assert_eq!(pre_only.disposition(), PairDisposition::ReconcileBeforeProceeding);
        assert_eq!(
            correlate_pair(
                &config(),
                &transaction(),
                &[
                    record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                    record(SnapshotKind::Pre, "11", "root", "zypp-1234"),
                ]
            ),
            PairCorrelation::Ambiguous
        );
        assert_eq!(
            correlate_pair(
                &config(),
                &transaction(),
                &[
                    record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                    record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                ]
            ),
            PairCorrelation::Ambiguous
        );
    }

    #[test]
    fn reconciliation_reuses_exact_hook_pair_and_never_duplicates_partial_pairs() {
        let request = SnapshotPairRequest { config: config(), transaction_id: transaction() };
        let exact = reconcile_pair(
            &request,
            &[
                record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                record(SnapshotKind::Post, "11", "root", "zypp-1234"),
                record(SnapshotKind::Pre, "12", "root", "zypp-9999"),
                record(SnapshotKind::Post, "13", "home", "zypp-1234"),
            ],
        );
        assert_eq!(
            exact,
            PairReconciliation {
                correlation: PairCorrelation::Existing {
                    pre: SnapshotId::new("10").unwrap(),
                    post: SnapshotId::new("11").unwrap(),
                },
                disposition: PairDisposition::ReuseExistingPair,
            }
        );

        let incomplete = reconcile_pair(
            &request,
            &[
                record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                // A post record with the wrong config is not its partner.
                record(SnapshotKind::Post, "11", "home", "zypp-1234"),
            ],
        );
        assert_eq!(
            incomplete.correlation,
            PairCorrelation::PreOnly { pre: SnapshotId::new("10").unwrap() }
        );
        assert_eq!(incomplete.disposition, PairDisposition::ReconcileBeforeProceeding);

        let mismatched_transaction = reconcile_pair(
            &request,
            &[
                record(SnapshotKind::Pre, "10", "root", "zypp-9999"),
                record(SnapshotKind::Post, "11", "root", "zypp-9999"),
            ],
        );
        assert_eq!(mismatched_transaction.correlation, PairCorrelation::Missing);
        assert_eq!(mismatched_transaction.disposition, PairDisposition::CreatePair);

        let unrelated =
            reconcile_pair(&request, &[record(SnapshotKind::Single, "10", "root", "zypp-9999")]);
        assert_eq!(unrelated.correlation, PairCorrelation::Missing);
        assert_eq!(unrelated.disposition, PairDisposition::CreatePair);

        let duplicate_id = reconcile_pair(
            &request,
            &[
                record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                record(SnapshotKind::Post, "10", "root", "zypp-1234"),
            ],
        );
        assert_eq!(duplicate_id.correlation, PairCorrelation::Ambiguous);
        assert_eq!(duplicate_id.disposition, PairDisposition::ReconcileBeforeProceeding);
    }

    #[test]
    fn runtime_pair_query_lists_once_and_reconciles_read_only() {
        let request = SnapshotPairRequest { config: config(), transaction_id: transaction() };
        let result = query_pair(
            &FakeBackend {
                records: Ok(vec![
                    record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                    record(SnapshotKind::Post, "11", "root", "zypp-1234"),
                ]),
            },
            &request,
        )
        .unwrap();
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.reconciliation.disposition, PairDisposition::ReuseExistingPair);
    }

    #[test]
    fn runtime_pair_query_rejects_unscoped_or_duplicate_inventory() {
        let request = SnapshotPairRequest { config: config(), transaction_id: transaction() };
        assert_eq!(
            query_pair(
                &FakeBackend {
                    records: Ok(vec![record(SnapshotKind::Pre, "10", "home", "zypp-1234")]),
                },
                &request,
            ),
            Err(PairQueryError::ForeignConfigRecord)
        );
        assert_eq!(
            query_pair(
                &FakeBackend {
                    records: Ok(vec![
                        record(SnapshotKind::Pre, "10", "root", "zypp-1234"),
                        record(SnapshotKind::Post, "10", "root", "zypp-1234"),
                    ]),
                },
                &request,
            ),
            Err(PairQueryError::DuplicateSnapshotId(SnapshotId::new("10").unwrap()))
        );
        assert_eq!(
            query_pair(&FakeBackend { records: Err("unavailable") }, &request),
            Err(PairQueryError::Backend("unavailable"))
        );
    }

    fn catalog_entry(capability: LayoutCapability, partner: PartnerState) -> CatalogEntry {
        CatalogEntry {
            transaction_id: transaction(),
            root_config: config(),
            root_snapshot: SnapshotId::new("10").unwrap(),
            generation_id: GenerationId::new("generation-1").unwrap(),
            capability,
            partner,
            manual_checkpoint: false,
            known_working: false,
        }
    }

    #[test]
    fn complete_requires_root_boot_capability_and_both_verified_partners() {
        let complete = catalog_entry(LayoutCapability::FullSystem, PartnerState::Verified);
        assert!(complete.is_complete(true));
        assert!(!complete.is_complete(false));
        assert!(
            !catalog_entry(LayoutCapability::FullSystem, PartnerState::Missing).is_complete(true)
        );
        assert!(
            !catalog_entry(LayoutCapability::ConfigurationOnly, PartnerState::Verified)
                .is_complete(true)
        );
        assert!(
            !catalog_entry(LayoutCapability::FullSystem, PartnerState::PresentUnverified)
                .is_complete(true)
        );
    }

    #[test]
    fn catalog_rejects_duplicate_transactions_and_tracks_retention_pins() {
        let mut catalog = CatalogIndex::default();
        assert!(
            catalog
                .insert(catalog_entry(LayoutCapability::FullSystem, PartnerState::Verified))
                .is_ok()
        );
        assert!(
            catalog
                .insert(catalog_entry(LayoutCapability::FullSystem, PartnerState::Verified))
                .is_err()
        );
        assert!(!catalog.mark_known_working(&transaction(), false));
        assert!(catalog.mark_known_working(&transaction(), true));
        assert!(catalog.mark_manual_checkpoint(&transaction(), true));
        let entry = catalog.get(&transaction()).unwrap();
        assert!(entry.known_working && entry.manual_checkpoint);
        assert_eq!(catalog.remove(&transaction()).unwrap().transaction_id, transaction());
    }

    #[test]
    fn identifiers_reject_command_and_path_injection() {
        assert!(ConfigName::new("root;rm").is_err());
        assert!(SnapshotId::new("../1").is_err());
        assert!(TransactionId::new("txn\n--").is_err());
        assert!(TransactionId::new("..").is_err());
        assert!(TransactionId::new(".txn").is_err());
        assert!(TransactionId::new("txn.").is_err());
    }
}
