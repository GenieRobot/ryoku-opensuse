//! Typed primitives for Snapper layout discovery and checkpoint correlation.
//!
//! [`SnapperCliBackend`] is read-only. [`SnapperPairBackend`] and
//! [`SnapperCheckpointBackend`] are separate native mutation capabilities for
//! an authorized coordinator; they do not provide authorization, writer
//! quiescence, or linked-generation verification. Durable catalog storage uses
//! a caller-supplied protected directory descriptor; placement outside rollback
//! scope remains the caller's responsibility.

mod catalog;
mod cli;
mod retention;
pub use retention::SnapperRetentionBackend;

pub use catalog::{CatalogReader, CatalogStore, CatalogStoreError};
pub use cli::{
    RootConfigValues, RootRollbackReceipt, RootRollbackRequest, SnapperCheckpointBackend,
    SnapperCliBackend, SnapperCliError, SnapperConfigMapping, SnapperPairBackend,
    SnapperRollbackBackend, parse_root_configuration, parse_snapper_json,
};

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
    /// Exact pre snapshot referenced by a post record, when this is a post.
    pub pre_snapshot: Option<SnapshotId>,
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
/// descriptions or timestamp proximity. Duplicate IDs or an unexpected
/// single snapshot with that transaction tag are ambiguous and must not
/// trigger automatic snapshot creation.
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
            SnapshotKind::Single => return PairCorrelation::Ambiguous,
        }
    }
    pre.sort();
    post.sort();
    match (pre.as_slice(), post.as_slice()) {
        ([], []) => PairCorrelation::Missing,
        ([pre], []) => PairCorrelation::PreOnly { pre: pre.clone() },
        ([pre], [post]) if pre != post => {
            let matching_pre = records.iter().any(|record| {
                &record.config == config
                    && record.transaction_id.as_ref() == Some(transaction_id)
                    && record.kind == SnapshotKind::Post
                    && record.id == *post
                    && record.pre_snapshot.as_ref() == Some(pre)
            });
            if !matching_pre {
                return PairCorrelation::Ambiguous;
            }
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

/// Exact additional partner identity persisted with the transaction catalog.
/// These fields must originate in fresh kernel observations; readback compares
/// every field before treating the additional partner as verified.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdditionalSnapshot {
    pub config: String,
    pub path: String,
    pub snapshot: String,
    pub filesystem_uuid: String,
    pub subvolume_id: u64,
    pub subvolume_uuid: String,
    pub origin_uuid: String,
}

/// One catalog row. This is an in-memory domain value, not a durable catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    pub transaction_id: TransactionId,
    pub root_config: ConfigName,
    pub root_snapshot: SnapshotId,
    pub generation_id: GenerationId,
    pub additional_snapshots: Vec<AdditionalSnapshot>,
    pub capability: LayoutCapability,
    pub partner: PartnerState,
    pub manual_checkpoint: bool,
    pub known_working: bool,
}

impl CatalogEntry {
    /// True only when fresh observations confirm both partners and the layout
    /// supports full-system protection. The persisted partner enum is a last
    /// recorded status; it does not replace the live Snapper observation.
    pub fn is_complete(
        &self,
        generation_present_and_verified: bool,
        snapper_partner_present_and_verified: bool,
    ) -> bool {
        self.additional_snapshots.is_empty()
            && self.root_config.as_str() == "root"
            && self.capability == LayoutCapability::FullSystem
            && self.partner == PartnerState::Verified
            && generation_present_and_verified
            && snapper_partner_present_and_verified
    }
    /// Completeness for the exact complete set, including additional configs.
    pub fn is_complete_set(&self, observation: &RetentionPartnerObservation) -> bool {
        self.transaction_id == observation.transaction_id
            && self.root_config.as_str() == "root"
            && self.capability == LayoutCapability::FullSystem
            && self.partner == PartnerState::Verified
            && observation.generation_present_and_verified
            && observation.snapper_partner_present_and_verified
            && self.additional_verified(observation)
    }

    fn additional_verified(&self, observation: &RetentionPartnerObservation) -> bool {
        let expected: std::collections::BTreeSet<_> = self
            .additional_snapshots
            .iter()
            .map(|p| (p.config.as_str(), p.snapshot.as_str()))
            .collect();
        let actual: std::collections::BTreeSet<_> =
            observation.verified_additional.iter().map(|(c, n)| (c.as_str(), n.as_str())).collect();
        expected.len() == self.additional_snapshots.len()
            && actual.len() == observation.verified_additional.len()
            && expected == actual
    }
}

/// In-memory index used to validate uniqueness and mark retention intent.
/// `CatalogStore` supplies bounded durable persistence; callers remain
/// responsible for authenticated access, rollback-independent placement, and
/// safe retention execution.
#[derive(Clone, Debug, Default)]
pub struct CatalogIndex {
    entries: BTreeMap<TransactionId, CatalogEntry>,
}

/// Fresh partner evidence supplied to the retention planner. Both values must
/// come from reopening the exact linked generation and live Snapper snapshot;
/// the planner never treats the catalog's last recorded `Verified` state as
/// current evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionPartnerObservation {
    pub transaction_id: TransactionId,
    pub generation_present_and_verified: bool,
    pub snapper_partner_present_and_verified: bool,
    pub verified_additional: Vec<(ConfigName, SnapshotId)>,
}

/// A non-mutating retention decision. `prune` contains only old, unpinned,
/// full-system rows whose two partners were freshly verified. An authorized
/// coordinator must still serialize and execute partner cleanup atomically
/// with respect to catalog publication; this value grants no delete authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetentionPlan {
    pub pinned: Vec<TransactionId>,
    pub keep: Vec<TransactionId>,
    pub prune: Vec<TransactionId>,
    pub reconcile: Vec<TransactionId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetentionPlanError {
    KeepAtLeastOneRequired,
    DuplicateObservation,
    UnknownTransaction,
    MissingObservation,
}

impl CatalogIndex {
    pub fn insert(&mut self, entry: CatalogEntry) -> Result<(), Box<CatalogEntry>> {
        if self.entries.contains_key(&entry.transaction_id) {
            return Err(Box::new(entry));
        }
        self.entries.insert(entry.transaction_id.clone(), entry);
        Ok(())
    }

    pub fn get(&self, id: &TransactionId) -> Option<&CatalogEntry> {
        self.entries.get(id)
    }

    /// Entries in stable transaction-ID order for persistence and review.
    pub fn entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }

    pub fn remove(&mut self, id: &TransactionId) -> Option<CatalogEntry> {
        self.entries.remove(id)
    }

    pub fn mark_known_working(&mut self, observation: &RetentionPartnerObservation) -> bool {
        let Some(entry) = self.entries.get_mut(&observation.transaction_id) else { return false };
        if !entry.is_complete_set(observation) {
            return false;
        }
        entry.known_working = true;
        true
    }

    pub fn mark_manual_checkpoint(&mut self, observation: &RetentionPartnerObservation) -> bool {
        let Some(entry) = self.entries.get_mut(&observation.transaction_id) else { return false };
        // A manual checkpoint can still protect a verified configuration/root
        // pair when boot coverage is unknown. Keep that retention intent
        // separate from the stronger known-working/full-system designation,
        // while still requiring the declared root Snapper partner.
        if entry.root_config.as_str() != "root"
            || entry.partner != PartnerState::Verified
            || !observation.generation_present_and_verified
            || !observation.snapper_partner_present_and_verified
            || !entry.additional_verified(observation)
        {
            return false;
        }
        entry.manual_checkpoint = true;
        true
    }

    /// Plan a conservative retention set from fresh, exact partner
    /// observations. Manual and known-working entries are always pinned.
    /// Incomplete, unsupported, duplicated-snapshot, or unverified rows are
    /// returned for reconciliation and can never be selected for pruning.
    pub fn plan_retention(
        &self,
        keep_latest: usize,
        observations: &[RetentionPartnerObservation],
    ) -> Result<RetentionPlan, RetentionPlanError> {
        if keep_latest == 0 {
            return Err(RetentionPlanError::KeepAtLeastOneRequired);
        }
        let mut observed = BTreeMap::<TransactionId, &RetentionPartnerObservation>::new();
        for observation in observations {
            if !self.entries.contains_key(&observation.transaction_id) {
                return Err(RetentionPlanError::UnknownTransaction);
            }
            if observed.insert(observation.transaction_id.clone(), observation).is_some() {
                return Err(RetentionPlanError::DuplicateObservation);
            }
        }
        if observed.len() != self.entries.len() {
            return Err(RetentionPlanError::MissingObservation);
        }

        let mut snapshot_counts = BTreeMap::<String, usize>::new();
        for entry in self.entries.values() {
            for key in snapshot_keys(entry) {
                *snapshot_counts.entry(key).or_default() += 1;
            }
        }

        let mut plan = RetentionPlan::default();
        let mut eligible = Vec::new();
        for entry in self.entries.values() {
            let observation = observed
                .get(&entry.transaction_id)
                .ok_or(RetentionPlanError::MissingObservation)?;
            if entry.manual_checkpoint || entry.known_working {
                plan.pinned.push(entry.transaction_id.clone());
                let duplicate_snapshot =
                    snapshot_keys(entry).iter().any(|key| snapshot_counts.get(key) != Some(&1));
                let invalid_known_working =
                    entry.known_working && !entry.is_complete_set(observation);
                if duplicate_snapshot
                    || entry.root_config.as_str() != "root"
                    || invalid_known_working
                    || entry.partner != PartnerState::Verified
                    || !observation.generation_present_and_verified
                    || !observation.snapper_partner_present_and_verified
                    || !entry.additional_verified(observation)
                {
                    plan.reconcile.push(entry.transaction_id.clone());
                }
                continue;
            }
            if snapshot_keys(entry).iter().any(|key| snapshot_counts.get(key) != Some(&1))
                || entry.root_config.as_str() != "root"
                || !entry.is_complete_set(observation)
            {
                plan.reconcile.push(entry.transaction_id.clone());
                continue;
            }
            eligible.push(entry);
        }

        eligible.sort_by(|left, right| {
            compare_snapshot_numbers(&right.root_snapshot, &left.root_snapshot)
                .then_with(|| left.transaction_id.cmp(&right.transaction_id))
        });
        for (index, entry) in eligible.into_iter().enumerate() {
            if index < keep_latest {
                plan.keep.push(entry.transaction_id.clone());
            } else {
                plan.prune.push(entry.transaction_id.clone());
            }
        }
        Ok(plan)
    }
}

fn compare_snapshot_numbers(left: &SnapshotId, right: &SnapshotId) -> std::cmp::Ordering {
    let left = snapshot_number_key(left);
    let right = snapshot_number_key(right);
    left.len().cmp(&right.len()).then_with(|| left.cmp(&right))
}

fn snapshot_keys(entry: &CatalogEntry) -> Vec<String> {
    let mut keys = vec![format!(
        "{}:{}",
        entry.root_config.as_str(),
        snapshot_number_key(&entry.root_snapshot)
    )];
    keys.extend(
        entry
            .additional_snapshots
            .iter()
            .map(|extra| format!("{}:{}", extra.config, extra.snapshot.trim_start_matches('0'))),
    );
    keys
}

fn snapshot_number_key(snapshot: &SnapshotId) -> String {
    let value = snapshot.as_str().trim_start_matches('0');
    if value.is_empty() { "0".into() } else { value.into() }
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
            pre_snapshot: (kind == SnapshotKind::Post).then(|| SnapshotId::new("10").unwrap()),
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

        let mut mismatched_link = record(SnapshotKind::Post, "11", "root", "zypp-1234");
        mismatched_link.pre_snapshot = Some(SnapshotId::new("12").unwrap());
        assert_eq!(
            correlate_pair(
                &config(),
                &transaction(),
                &[record(SnapshotKind::Pre, "10", "root", "zypp-1234"), mismatched_link],
            ),
            PairCorrelation::Ambiguous
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
            additional_snapshots: Vec::new(),
            capability,
            partner,
            manual_checkpoint: false,
            known_working: false,
        }
    }

    #[test]
    fn complete_requires_root_boot_capability_and_both_verified_partners() {
        let complete = catalog_entry(LayoutCapability::FullSystem, PartnerState::Verified);
        assert!(complete.is_complete(true, true));
        assert!(!complete.is_complete(false, true));
        assert!(!complete.is_complete(true, false));
        assert!(
            !catalog_entry(LayoutCapability::FullSystem, PartnerState::Missing)
                .is_complete(true, true)
        );
        assert!(
            !catalog_entry(LayoutCapability::ConfigurationOnly, PartnerState::Verified)
                .is_complete(true, true)
        );
        assert!(
            !catalog_entry(LayoutCapability::FullSystem, PartnerState::PresentUnverified)
                .is_complete(true, true)
        );
        let non_root = CatalogEntry {
            root_config: ConfigName::new("home").unwrap(),
            ..catalog_entry(LayoutCapability::FullSystem, PartnerState::Verified)
        };
        assert!(!non_root.is_complete(true, true));
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
        let mut observation = RetentionPartnerObservation {
            transaction_id: transaction(),
            generation_present_and_verified: false,
            snapper_partner_present_and_verified: true,
            verified_additional: Vec::new(),
        };
        assert!(!catalog.mark_known_working(&observation));
        observation.generation_present_and_verified = true;
        observation.snapper_partner_present_and_verified = false;
        assert!(!catalog.mark_known_working(&observation));
        observation.snapper_partner_present_and_verified = true;
        assert!(catalog.mark_known_working(&observation));
        assert!(catalog.mark_manual_checkpoint(&observation));
        let wrong_transaction = RetentionPartnerObservation {
            transaction_id: TransactionId::new("other-transaction").unwrap(),
            generation_present_and_verified: true,
            snapper_partner_present_and_verified: true,
            verified_additional: Vec::new(),
        };
        assert!(!catalog.mark_known_working(&wrong_transaction));
        assert!(!catalog.mark_manual_checkpoint(&wrong_transaction));
        let entry = catalog.get(&transaction()).unwrap();
        assert!(entry.known_working && entry.manual_checkpoint);
        assert_eq!(catalog.remove(&transaction()).unwrap().transaction_id, transaction());
    }

    #[test]
    fn manual_checkpoint_retention_intent_does_not_claim_full_system_coverage() {
        let mut catalog = CatalogIndex::default();
        let id = transaction();
        assert!(
            catalog
                .insert(catalog_entry(LayoutCapability::Unsupported, PartnerState::Verified))
                .is_ok()
        );

        let observation = RetentionPartnerObservation {
            transaction_id: id.clone(),
            generation_present_and_verified: true,
            snapper_partner_present_and_verified: true,
            verified_additional: Vec::new(),
        };
        assert!(catalog.mark_manual_checkpoint(&observation));
        assert!(!catalog.mark_known_working(&observation));
        let entry = catalog.get(&id).unwrap();
        assert!(entry.manual_checkpoint);
        assert!(!entry.known_working);
        assert!(!entry.is_complete(true, true));
    }

    fn retention_entry(
        transaction_id: &str,
        snapshot: &str,
        manual_checkpoint: bool,
        known_working: bool,
    ) -> CatalogEntry {
        CatalogEntry {
            transaction_id: TransactionId::new(transaction_id).unwrap(),
            root_config: config(),
            root_snapshot: SnapshotId::new(snapshot).unwrap(),
            generation_id: GenerationId::new(format!("generation-{transaction_id}")).unwrap(),
            additional_snapshots: Vec::new(),
            capability: LayoutCapability::FullSystem,
            partner: PartnerState::Verified,
            manual_checkpoint,
            known_working,
        }
    }

    fn verified_partners(transaction_id: &str) -> RetentionPartnerObservation {
        RetentionPartnerObservation {
            transaction_id: TransactionId::new(transaction_id).unwrap(),
            generation_present_and_verified: true,
            snapper_partner_present_and_verified: true,
            verified_additional: Vec::new(),
        }
    }

    #[test]
    fn retention_plan_preserves_pins_and_only_prunes_old_verified_complete_rows() {
        let mut catalog = CatalogIndex::default();
        catalog.insert(retention_entry("txn-manual", "3", true, false)).unwrap();
        catalog.insert(retention_entry("txn-known", "12", false, true)).unwrap();
        catalog.insert(retention_entry("txn-new", "11", false, false)).unwrap();
        catalog.insert(retention_entry("txn-old", "9", false, false)).unwrap();
        catalog
            .insert(CatalogEntry {
                capability: LayoutCapability::ConfigurationOnly,
                ..retention_entry("txn-unsupported", "8", false, false)
            })
            .unwrap();
        catalog
            .insert(CatalogEntry {
                root_config: ConfigName::new("home").unwrap(),
                ..retention_entry("txn-other-config", "7", false, false)
            })
            .unwrap();
        let mut stale = verified_partners("txn-old");
        stale.snapper_partner_present_and_verified = false;

        let plan = catalog
            .plan_retention(
                1,
                &[
                    verified_partners("txn-manual"),
                    verified_partners("txn-known"),
                    verified_partners("txn-new"),
                    stale,
                    verified_partners("txn-unsupported"),
                    verified_partners("txn-other-config"),
                ],
            )
            .unwrap();

        assert_eq!(
            plan.pinned,
            [TransactionId::new("txn-known").unwrap(), TransactionId::new("txn-manual").unwrap()]
        );
        assert_eq!(plan.keep, [TransactionId::new("txn-new").unwrap()]);
        assert!(plan.prune.is_empty());
        assert_eq!(
            plan.reconcile,
            [
                TransactionId::new("txn-old").unwrap(),
                TransactionId::new("txn-other-config").unwrap(),
                TransactionId::new("txn-unsupported").unwrap()
            ]
        );
    }

    #[test]
    fn missing_partner_keeps_a_pinned_checkpoint_and_requests_reconciliation() {
        let mut catalog = CatalogIndex::default();
        catalog.insert(retention_entry("txn-manual", "3", true, false)).unwrap();
        catalog.insert(retention_entry("txn-known", "12", false, true)).unwrap();

        let mut missing_generation = verified_partners("txn-manual");
        missing_generation.generation_present_and_verified = false;
        let mut missing_snapper = verified_partners("txn-known");
        missing_snapper.snapper_partner_present_and_verified = false;

        let plan = catalog.plan_retention(1, &[missing_generation, missing_snapper]).unwrap();

        assert_eq!(
            plan.pinned,
            [TransactionId::new("txn-known").unwrap(), TransactionId::new("txn-manual").unwrap()]
        );
        assert_eq!(
            plan.reconcile,
            [TransactionId::new("txn-known").unwrap(), TransactionId::new("txn-manual").unwrap()]
        );
        assert!(plan.prune.is_empty());
    }

    #[test]
    fn pinned_duplicate_and_invalid_known_working_rows_require_reconciliation() {
        let mut catalog = CatalogIndex::default();
        catalog.insert(retention_entry("txn-manual", "1", true, false)).unwrap();
        catalog.insert(retention_entry("txn-duplicate", "01", false, false)).unwrap();
        catalog
            .insert(CatalogEntry {
                capability: LayoutCapability::ConfigurationOnly,
                ..retention_entry("txn-invalid-known", "2", false, true)
            })
            .unwrap();

        let plan = catalog
            .plan_retention(
                1,
                &[
                    verified_partners("txn-manual"),
                    verified_partners("txn-duplicate"),
                    verified_partners("txn-invalid-known"),
                ],
            )
            .unwrap();

        assert_eq!(
            plan.pinned,
            [
                TransactionId::new("txn-invalid-known").unwrap(),
                TransactionId::new("txn-manual").unwrap()
            ]
        );
        assert_eq!(
            plan.reconcile,
            [
                TransactionId::new("txn-duplicate").unwrap(),
                TransactionId::new("txn-invalid-known").unwrap(),
                TransactionId::new("txn-manual").unwrap()
            ]
        );
        assert!(plan.prune.is_empty());
    }

    #[test]
    fn retention_plan_prunes_only_verified_old_unpinned_rows() {
        let mut catalog = CatalogIndex::default();
        catalog.insert(retention_entry("txn-9", "9", false, false)).unwrap();
        catalog.insert(retention_entry("txn-10", "10", false, false)).unwrap();
        catalog.insert(retention_entry("txn-100", "100", false, false)).unwrap();
        let plan = catalog
            .plan_retention(
                1,
                &[
                    verified_partners("txn-9"),
                    verified_partners("txn-10"),
                    verified_partners("txn-100"),
                ],
            )
            .unwrap();
        assert_eq!(plan.keep, [TransactionId::new("txn-100").unwrap()]);
        assert_eq!(
            plan.prune,
            [TransactionId::new("txn-10").unwrap(), TransactionId::new("txn-9").unwrap()]
        );
    }

    #[test]
    fn retention_plan_rejects_incomplete_or_ambiguous_observation_sets() {
        let mut catalog = CatalogIndex::default();
        catalog.insert(retention_entry("txn-1", "01", false, false)).unwrap();
        catalog.insert(retention_entry("txn-2", "1", false, false)).unwrap();
        assert_eq!(
            catalog.plan_retention(1, &[verified_partners("txn-1")]),
            Err(RetentionPlanError::MissingObservation)
        );
        assert_eq!(
            catalog.plan_retention(
                1,
                &[
                    verified_partners("txn-1"),
                    verified_partners("txn-1"),
                    verified_partners("txn-2"),
                ]
            ),
            Err(RetentionPlanError::DuplicateObservation)
        );
        let plan = catalog
            .plan_retention(1, &[verified_partners("txn-1"), verified_partners("txn-2")])
            .unwrap();
        assert!(plan.prune.is_empty());
        assert_eq!(plan.reconcile.len(), 2);
        assert_eq!(catalog.plan_retention(0, &[]), Err(RetentionPlanError::KeepAtLeastOneRequired));
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
