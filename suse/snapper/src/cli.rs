//! Bounded Snapper inventory and explicitly separated checkpoint capabilities.

use std::{
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid},
};
use serde_json::Value;

use crate::{
    ConfigName, SnapperBackend, SnapshotId, SnapshotKind, SnapshotPairRequest, SnapshotRecord,
    TransactionId, ValueError,
};

const SNAPPER: &str = "/usr/bin/snapper";
const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 16 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSACTION_USERDATA_KEY: &str = "ryoku-transaction-id";

/// A fixed-path Snapper CLI adapter. It lists snapshots only; both mutation
/// methods return [`SnapperCliError::MutationUnavailable`] without invoking a
/// process. The backend never accepts a root/path override or a command string.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapperCliBackend;

/// Parsed configuration values, not native identity or mutation authority.
/// The fixed native query uses Snapper's effective configuration (including
/// its daemon); callers must separately bind secure files and kernel identity.
#[derive(Clone, Eq, PartialEq)]
pub struct RootConfigValues(std::collections::BTreeMap<String, String>);

impl SnapperCliBackend {
    /// Read only the fixed root configuration. No target/config override.
    pub fn root_configuration(&self) -> Result<RootConfigValues, SnapperCliError> {
        parse_root_configuration(&run_snapper(&root_configuration_args())?)
    }
}
fn root_configuration_args() -> [String; 4] {
    ["--jsonout".into(), "--config".into(), "root".into(), "get-config".into()]
}

/// Parse Snapper's flat string-valued get-config object, refusing duplicate
/// keys and anything except one exact root/Btrfs mapping. Parsing alone does
/// not prove that these are current native values.
pub fn parse_root_configuration(bytes: &[u8]) -> Result<RootConfigValues, SnapperCliError> {
    use serde::de::{MapAccess, Visitor};
    if bytes.len() > 128 * 1024 {
        return Err(SnapperCliError::OutputTooLarge);
    }
    struct UniqueValues;
    impl<'de> Visitor<'de> for UniqueValues {
        type Value = std::collections::BTreeMap<String, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a unique string-valued configuration object")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut values = std::collections::BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if values.len() >= 1024 || values.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("ambiguous configuration"));
                }
            }
            Ok(values)
        }
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let values = serde::de::Deserializer::deserialize_map(&mut deserializer, UniqueValues)
        .map_err(|_| SnapperCliError::InvalidInventory)?;
    deserializer.end().map_err(|_| SnapperCliError::InvalidInventory)?;
    if values.get("SUBVOLUME").map(String::as_str) != Some("/")
        || values.get("FSTYPE").map(String::as_str) != Some("btrfs")
    {
        return Err(SnapperCliError::InvalidInventory);
    }
    Ok(RootConfigValues(values))
}

/// Sanitized failure categories from the Snapper CLI adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapperCliError {
    SpawnFailed,
    TimedOut,
    OutputTooLarge,
    CommandFailed,
    InvalidJson,
    InvalidInventory,
    InvalidValue(ValueError),
    MutationUnavailable,
    CheckpointAlreadyExists,
    CheckpointNotVerified,
    PairAlreadyExists,
    PairNotVerified,
    RollbackAlreadyExists,
    RollbackUnsupportedConfig,
    RollbackSourceUnavailable,
    RollbackNotVerified,
}

impl std::fmt::Display for SnapperCliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Snapper operation failed: {self:?}")
    }
}

impl std::error::Error for SnapperCliError {}

impl SnapperBackend for SnapperCliBackend {
    type Error = SnapperCliError;

    fn list(&self, config: &ConfigName) -> Result<Vec<SnapshotRecord>, Self::Error> {
        parse_snapper_json(config, &run_snapper(&snapper_list_args(config))?)
    }

    fn create_pre(&mut self, _request: &SnapshotPairRequest) -> Result<SnapshotId, Self::Error> {
        Err(SnapperCliError::MutationUnavailable)
    }

    fn create_post(
        &mut self,
        _request: &SnapshotPairRequest,
        _pre_snapshot: &SnapshotId,
    ) -> Result<SnapshotId, Self::Error> {
        Err(SnapperCliError::MutationUnavailable)
    }
}

/// Native single-snapshot creation for a privileged, authorized checkpoint
/// coordinator. This does not authorize its caller or prove Btrfs UUID, boot
/// coverage, generation linkage, or placement outside rollback scope.
///
/// Pair mutations on `SnapperCliBackend` remain unavailable. Keep checkpoint
/// capabilities out of read-only status/service request paths.
#[derive(Debug, Default)]
pub struct SnapperCheckpointBackend;

impl SnapperCheckpointBackend {
    /// Create a read-only single snapshot without automatic cleanup, tagged
    /// with the exact transaction ID. A fresh inventory must confirm its ID,
    /// type and tag before success. The caller must hold its mutation lock and
    /// freeze relevant writers through configuration capture.
    ///
    /// Any failure after invoking create may have left a real snapshot. Never
    /// retry blindly or delete that snapshot: reconcile its transaction tag.
    pub fn create_single(
        &mut self,
        request: &SnapshotPairRequest,
    ) -> Result<SnapshotId, SnapperCliError> {
        create_single_with(request, run_snapper)
    }
}

/// Native pre/post pair creation for an authorized coordinator. It does not
/// authorize the caller, freeze writers, establish snapshot compatibility, or
/// link a Ryoku generation. Callers must hold the common mutation lock and
/// retain the pre ID durably before proceeding. A failed or timed-out create
/// must be reconciled by transaction tag before any retry.
#[derive(Debug, Default)]
pub struct SnapperPairBackend;

impl SnapperBackend for SnapperPairBackend {
    type Error = SnapperCliError;

    fn list(&self, config: &ConfigName) -> Result<Vec<SnapshotRecord>, Self::Error> {
        parse_snapper_json(config, &run_snapper(&snapper_list_args(config))?)
    }

    fn create_pre(&mut self, request: &SnapshotPairRequest) -> Result<SnapshotId, Self::Error> {
        create_pre_with(request, run_snapper)
    }

    fn create_post(
        &mut self,
        request: &SnapshotPairRequest,
        pre_snapshot: &SnapshotId,
    ) -> Result<SnapshotId, Self::Error> {
        create_post_with(request, pre_snapshot, run_snapper)
    }
}

/// A request to create a writable root snapshot from one exact retained
/// Snapper snapshot. The transaction ID must identify this restore attempt,
/// not the transaction that originally created the source snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootRollbackRequest {
    pub config: ConfigName,
    pub source_snapshot: SnapshotId,
    pub transaction_id: TransactionId,
}

/// Inventory evidence returned by Snapper after rollback. This confirms the
/// native command created and tagged its two snapshots; it does not prove
/// Btrfs's default subvolume, boot compatibility, or that the system booted
/// into the returned snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootRollbackReceipt {
    pub config: ConfigName,
    pub source_snapshot: SnapshotId,
    pub previous_default_snapshot: SnapshotId,
    pub backup_snapshot: SnapshotId,
    pub rollback_target_snapshot: SnapshotId,
    pub transaction_id: TransactionId,
}

struct RollbackSnapshotRecord {
    record: SnapshotRecord,
    is_default: bool,
    is_active: bool,
    read_only: bool,
}

/// Explicit native rollback capability for an already-authorized recovery
/// coordinator. Callers must retain the common mutation lock, revalidate the
/// source partner, verify platform compatibility, and stop writers before
/// calling. Failure after invocation may have changed the Btrfs default; the
/// caller must reconcile the attempt ID and must never retry blindly.
#[derive(Debug, Default)]
pub struct SnapperRollbackBackend;

impl SnapperRollbackBackend {
    /// Inspect only the snapshots tagged to one already-persisted restore
    /// attempt. This is the required first step after an uncertain command
    /// result; an empty or partial set is evidence to reconcile, never a
    /// reason to retry automatically.
    pub fn inspect_attempt(
        &self,
        config: &ConfigName,
        transaction_id: &TransactionId,
    ) -> Result<Vec<SnapshotRecord>, SnapperCliError> {
        inspect_attempt_with(config, transaction_id, run_snapper)
    }

    /// Verify after reboot that the attempt's writable target is both the
    /// active and default snapshot. Btrfs identity must still be checked by
    /// the recovery layer before it accepts root-rollback completion.
    pub fn verify_running_target(
        &self,
        config: &ConfigName,
        target_snapshot: &SnapshotId,
        transaction_id: &TransactionId,
    ) -> Result<(), SnapperCliError> {
        if config.as_str() != "root" {
            return Err(SnapperCliError::RollbackUnsupportedConfig);
        }
        verify_running_target_with(config, target_snapshot, transaction_id, run_snapper)
    }

    /// Verify before reboot that the recorded target is still Snapper's unique
    /// writable default and that its exact tagged rollback backup remains a
    /// retained read-only single snapshot. This does not claim the target has
    /// booted; the recovery layer must still verify Btrfs identity afterward.
    pub fn verify_reboot_handoff(
        &self,
        config: &ConfigName,
        target_snapshot: &SnapshotId,
        backup_snapshot: &SnapshotId,
        transaction_id: &TransactionId,
    ) -> Result<(), SnapperCliError> {
        if config.as_str() != "root" {
            return Err(SnapperCliError::RollbackUnsupportedConfig);
        }
        verify_reboot_handoff_with(
            config,
            target_snapshot,
            backup_snapshot,
            transaction_id,
            run_snapper,
        )
    }
}

fn verify_running_target_with(
    config: &ConfigName,
    target_snapshot: &SnapshotId,
    transaction_id: &TransactionId,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<(), SnapperCliError> {
    let records = rollback_inventory_with(config, &mut run)?;
    let targets = records
        .iter()
        .filter(|row| {
            row.record.id == *target_snapshot
                && row.record.transaction_id.as_ref() == Some(transaction_id)
        })
        .collect::<Vec<_>>();
    if targets.len() != 1
        || !targets[0].is_default
        || !targets[0].is_active
        || targets[0].read_only
        || records.iter().filter(|row| row.is_default).count() != 1
        || records.iter().filter(|row| row.is_active).count() != 1
    {
        return Err(SnapperCliError::RollbackNotVerified);
    }
    Ok(())
}

fn verify_reboot_handoff_with(
    config: &ConfigName,
    target_snapshot: &SnapshotId,
    backup_snapshot: &SnapshotId,
    transaction_id: &TransactionId,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<(), SnapperCliError> {
    if config.as_str() != "root" || target_snapshot == backup_snapshot {
        return Err(SnapperCliError::RollbackNotVerified);
    }
    let records = rollback_inventory_with(config, &mut run)?;
    let tagged = records
        .iter()
        .filter(|row| row.record.transaction_id.as_ref() == Some(transaction_id))
        .collect::<Vec<_>>();
    let target = tagged.iter().filter(|row| row.record.id == *target_snapshot).collect::<Vec<_>>();
    let backup = tagged.iter().filter(|row| row.record.id == *backup_snapshot).collect::<Vec<_>>();
    if tagged.len() != 2
        || target.len() != 1
        || backup.len() != 1
        || target[0].record.kind != SnapshotKind::Single
        || target[0].record.pre_snapshot.is_some()
        || !target[0].is_default
        || target[0].is_active
        || target[0].read_only
        || backup[0].record.kind != SnapshotKind::Single
        || backup[0].record.pre_snapshot.is_some()
        || backup[0].is_default
        || backup[0].is_active
        || !backup[0].read_only
        || records.iter().filter(|row| row.is_default).count() != 1
        || records.iter().filter(|row| row.is_active).count() != 1
    {
        return Err(SnapperCliError::RollbackNotVerified);
    }
    Ok(())
}

fn inspect_attempt_with(
    config: &ConfigName,
    transaction_id: &TransactionId,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<Vec<SnapshotRecord>, SnapperCliError> {
    let records = pair_list_with(config, &mut run)?;
    validate_inventory(&records)?;
    Ok(records
        .into_iter()
        .filter(|row| row.config == *config && row.transaction_id.as_ref() == Some(transaction_id))
        .collect())
}

impl SnapperRollbackBackend {
    pub fn rollback_to(
        &mut self,
        request: &RootRollbackRequest,
    ) -> Result<RootRollbackReceipt, SnapperCliError> {
        rollback_to_with(request, run_snapper)
    }
}

fn rollback_to_with(
    request: &RootRollbackRequest,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<RootRollbackReceipt, SnapperCliError> {
    if request.config.as_str() != "root" {
        return Err(SnapperCliError::RollbackUnsupportedConfig);
    }
    let before = rollback_inventory_with(&request.config, &mut run)?;
    if before.iter().any(|row| row.record.transaction_id.as_ref() == Some(&request.transaction_id))
    {
        return Err(SnapperCliError::RollbackAlreadyExists);
    }
    let source =
        before.iter().filter(|row| row.record.id == request.source_snapshot).collect::<Vec<_>>();
    let defaults = before.iter().filter(|row| row.is_default).collect::<Vec<_>>();
    let active_count = before.iter().filter(|row| row.is_active).count();
    if request.source_snapshot.as_str() == "0"
        || source.len() != 1
        || source[0].record.kind != SnapshotKind::Single
        || source[0].record.pre_snapshot.is_some()
        || !source[0].read_only
        || defaults.len() != 1
        || defaults[0].read_only
        || active_count != 1
    {
        return Err(SnapperCliError::RollbackSourceUnavailable);
    }
    let previous_default_snapshot = defaults[0].record.id.clone();

    let output = run(&root_rollback_args(request))?;
    let new_default_snapshot =
        parse_created_id(&output).map_err(|_| SnapperCliError::RollbackNotVerified)?;
    let after = rollback_inventory_with(&request.config, &mut run)?;
    let new_records: Vec<_> = after
        .iter()
        .filter(|row| !before.iter().any(|prior| prior.record.id == row.record.id))
        .collect();
    let source_is_retained = after
        .iter()
        .filter(|row| {
            row.record.id == request.source_snapshot
                && row.record.config == request.config
                && row.record.kind == SnapshotKind::Single
                && row.read_only
        })
        .count()
        == 1;
    let result = after
        .iter()
        .filter(|row| {
            row.record.id == new_default_snapshot
                && row.record.config == request.config
                && row.record.kind == SnapshotKind::Single
                && row.record.pre_snapshot.is_none()
                && row.record.transaction_id.as_ref() == Some(&request.transaction_id)
        })
        .collect::<Vec<_>>();
    let default_count = after.iter().filter(|row| row.is_default).count();
    if new_records.len() != 2
        || new_records.iter().any(|row| {
            row.record.kind != SnapshotKind::Single
                || row.record.pre_snapshot.is_some()
                || row.record.transaction_id.as_ref() != Some(&request.transaction_id)
        })
        || !source_is_retained
        || result.len() != 1
        || default_count != 1
        || result[0].is_active
        || !result[0].is_default
        || result[0].read_only
        || new_records
            .iter()
            .filter(|row| row.record.id != new_default_snapshot)
            .any(|row| !row.read_only || row.is_default)
    {
        return Err(SnapperCliError::RollbackNotVerified);
    }
    let backup_snapshot = new_records
        .iter()
        .find(|row| row.record.id != new_default_snapshot)
        .ok_or(SnapperCliError::RollbackNotVerified)?
        .record
        .id
        .clone();
    Ok(RootRollbackReceipt {
        config: request.config.clone(),
        source_snapshot: request.source_snapshot.clone(),
        previous_default_snapshot,
        backup_snapshot,
        rollback_target_snapshot: new_default_snapshot,
        transaction_id: request.transaction_id.clone(),
    })
}

fn rollback_inventory_with(
    config: &ConfigName,
    run: &mut impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<Vec<RollbackSnapshotRecord>, SnapperCliError> {
    let bytes = run(&rollback_list_args(config))?;
    let records = parse_snapper_json(config, &bytes)?;
    validate_inventory(&records)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| SnapperCliError::InvalidJson)?;
    let rows = value
        .as_object()
        .and_then(|object| object.get(config.as_str()))
        .and_then(Value::as_array)
        .ok_or(SnapperCliError::InvalidInventory)?;
    if rows.len() != records.len() {
        return Err(SnapperCliError::InvalidInventory);
    }
    rows.iter()
        .zip(records)
        .map(|(row, record)| {
            let row = row.as_object().ok_or(SnapperCliError::InvalidInventory)?;
            Ok(RollbackSnapshotRecord {
                record,
                is_default: row
                    .get("default")
                    .and_then(Value::as_bool)
                    .ok_or(SnapperCliError::InvalidInventory)?,
                is_active: row
                    .get("active")
                    .and_then(Value::as_bool)
                    .ok_or(SnapperCliError::InvalidInventory)?,
                read_only: row
                    .get("read-only")
                    .and_then(Value::as_bool)
                    .ok_or(SnapperCliError::InvalidInventory)?,
            })
        })
        .collect()
}

fn root_rollback_args(request: &RootRollbackRequest) -> Vec<String> {
    vec![
        "--quiet".into(),
        "--config".into(),
        request.config.as_str().into(),
        "rollback".into(),
        "--print-number".into(),
        "--description".into(),
        "Ryoku offline recovery".into(),
        "--userdata".into(),
        format!("{TRANSACTION_USERDATA_KEY}={}", request.transaction_id.as_str()),
        request.source_snapshot.as_str().into(),
    ]
}

fn rollback_list_args(config: &ConfigName) -> Vec<String> {
    let mut args = snapper_list_args(config).to_vec();
    args.extend([
        "--columns".into(),
        "number,type,pre-number,userdata,default,active,read-only".into(),
    ]);
    args
}

fn pair_list_with(
    config: &ConfigName,
    run: &mut impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<Vec<SnapshotRecord>, SnapperCliError> {
    parse_snapper_json(config, &run(&snapper_list_args(config))?)
}

fn create_pre_with(
    request: &SnapshotPairRequest,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<SnapshotId, SnapperCliError> {
    let before = pair_list_with(&request.config, &mut run)?;
    validate_inventory(&before)?;
    if before.iter().any(|row| row.transaction_id.as_ref() == Some(&request.transaction_id)) {
        return Err(SnapperCliError::PairAlreadyExists);
    }
    let output = run(&pair_create_args(request, "pre", None))?;
    let id = parse_created_id(&output)?;
    let after = pair_list_with(&request.config, &mut run)?;
    verify_created_pair_member(&before, &after, request, &id, SnapshotKind::Pre, 1)?;
    Ok(id)
}

fn create_post_with(
    request: &SnapshotPairRequest,
    pre_snapshot: &SnapshotId,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<SnapshotId, SnapperCliError> {
    let before = pair_list_with(&request.config, &mut run)?;
    validate_inventory(&before)?;
    let tagged: Vec<_> = before
        .iter()
        .filter(|row| row.transaction_id.as_ref() == Some(&request.transaction_id))
        .collect();
    if tagged.iter().any(|row| row.kind == SnapshotKind::Post) {
        return Err(SnapperCliError::PairAlreadyExists);
    }
    if tagged.len() != 1 || tagged[0].id != *pre_snapshot || tagged[0].kind != SnapshotKind::Pre {
        return Err(SnapperCliError::PairNotVerified);
    }

    let output = run(&pair_create_args(request, "post", Some(pre_snapshot)))?;
    let id = parse_created_id(&output)?;
    let after = pair_list_with(&request.config, &mut run)?;
    verify_created_pair_member(&before, &after, request, &id, SnapshotKind::Post, 2)?;
    let verified = after.iter().any(|row| {
        row.id == *pre_snapshot
            && row.kind == SnapshotKind::Pre
            && row.transaction_id.as_ref() == Some(&request.transaction_id)
    }) && after.iter().any(|row| {
        row.id == id
            && row.kind == SnapshotKind::Post
            && row.pre_snapshot.as_ref() == Some(pre_snapshot)
            && row.transaction_id.as_ref() == Some(&request.transaction_id)
    });
    if !verified {
        return Err(SnapperCliError::PairNotVerified);
    }
    Ok(id)
}

fn pair_create_args(
    request: &SnapshotPairRequest,
    kind: &str,
    pre_snapshot: Option<&SnapshotId>,
) -> Vec<String> {
    let mut args = vec![
        "--config".into(),
        request.config.as_str().into(),
        "create".into(),
        "--type".into(),
        kind.into(),
    ];
    if let Some(pre) = pre_snapshot {
        args.extend(["--pre-number".into(), pre.as_str().into()]);
    }
    args.extend([
        "--print-number".into(),
        "--description".into(),
        "Ryoku operation".into(),
        "--userdata".into(),
        format!("{TRANSACTION_USERDATA_KEY}={}", request.transaction_id.as_str()),
    ]);
    args
}

fn parse_created_id(output: &[u8]) -> Result<SnapshotId, SnapperCliError> {
    let value = std::str::from_utf8(output).map_err(|_| SnapperCliError::PairNotVerified)?;
    let number = value.strip_suffix('\n').unwrap_or(value);
    let parsed = number.parse::<u64>().map_err(|_| SnapperCliError::PairNotVerified)?;
    if parsed == 0 || parsed.to_string() != number {
        return Err(SnapperCliError::PairNotVerified);
    }
    SnapshotId::new(number.to_owned()).map_err(SnapperCliError::InvalidValue)
}

fn verify_created_pair_member(
    before: &[SnapshotRecord],
    after: &[SnapshotRecord],
    request: &SnapshotPairRequest,
    id: &SnapshotId,
    kind: SnapshotKind,
    expected_tagged: usize,
) -> Result<(), SnapperCliError> {
    validate_inventory(after)?;
    let tagged: Vec<_> = after
        .iter()
        .filter(|row| row.transaction_id.as_ref() == Some(&request.transaction_id))
        .collect();
    if before.iter().any(|row| row.id == *id)
        || tagged.len() != expected_tagged
        || tagged.iter().filter(|row| row.id == *id && row.kind == kind).count() != 1
        || after.iter().find(|row| row.id == *id).is_none_or(|row| row.config != request.config)
    {
        return Err(SnapperCliError::PairNotVerified);
    }
    Ok(())
}

fn create_single_with(
    request: &SnapshotPairRequest,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<SnapshotId, SnapperCliError> {
    let before = parse_snapper_json(&request.config, &run(&snapper_list_args(&request.config))?)?;
    validate_inventory(&before)?;
    if before.iter().any(|row| row.transaction_id.as_ref() == Some(&request.transaction_id)) {
        return Err(SnapperCliError::CheckpointAlreadyExists);
    }
    let args = vec![
        "--config".into(),
        request.config.as_str().into(),
        "create".into(),
        "--type".into(),
        "single".into(),
        "--read-only".into(),
        "--print-number".into(),
        "--description".into(),
        "Ryoku checkpoint".into(),
        "--userdata".into(),
        format!("{TRANSACTION_USERDATA_KEY}={}", request.transaction_id.as_str()),
    ];
    let output = run(&args)?;
    let number =
        std::str::from_utf8(&output).map_err(|_| SnapperCliError::CheckpointNotVerified)?;
    let number = number.strip_suffix('\n').unwrap_or(number);
    let numeric = number.parse::<u64>().map_err(|_| SnapperCliError::CheckpointNotVerified)?;
    if numeric == 0 || numeric.to_string() != number {
        return Err(SnapperCliError::CheckpointNotVerified);
    }
    let id = SnapshotId::new(number).map_err(SnapperCliError::InvalidValue)?;
    let after = parse_snapper_json(&request.config, &run(&snapper_list_args(&request.config))?)?;
    validate_inventory(&after)?;
    let tagged: Vec<_> = after
        .iter()
        .filter(|row| row.transaction_id.as_ref() == Some(&request.transaction_id))
        .collect();
    if before.iter().any(|row| row.id == id)
        || tagged.len() != 1
        || tagged[0].id != id
        || tagged[0].kind != SnapshotKind::Single
    {
        return Err(SnapperCliError::CheckpointNotVerified);
    }
    Ok(id)
}

fn validate_inventory(records: &[SnapshotRecord]) -> Result<(), SnapperCliError> {
    let mut ids = std::collections::BTreeSet::new();
    for row in records {
        let value =
            row.id.as_str().parse::<u64>().map_err(|_| SnapperCliError::InvalidInventory)?;
        if value.to_string() != row.id.as_str() || !ids.insert(&row.id) {
            return Err(SnapperCliError::InvalidInventory);
        }
    }
    Ok(())
}

fn run_snapper(args: &[String]) -> Result<Vec<u8>, SnapperCliError> {
    run_bounded(native_command(args), COMMAND_TIMEOUT)
}
fn native_command(args: &[String]) -> Command {
    let mut command = Command::new(SNAPPER);
    command.env_clear().env("LC_ALL", "C").env("PATH", "/usr/sbin:/usr/bin").args(args);
    command
}

// This helper is private: production callers retain the fixed native executable
// and argv above. Each command owns a fresh process group, including ordinary
// descendants. It does not contain a descendant that deliberately changes its
// session/process group, nor replace daemon-side Snapper transaction locking.
fn run_bounded(mut command: Command, timeout: Duration) -> Result<Vec<u8>, SnapperCliError> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| SnapperCliError::SpawnFailed)?;
    let result = collect_bounded(&mut child, deadline);
    if result.is_err() {
        // The direct child may already have exited while a descendant retains
        // either pipe. Kill the owned group before reaping; never join a pipe
        // reader or wait for EOF during error cleanup.
        if let Some(pid) = Pid::from_raw(child.id() as i32) {
            let _ = kill_process_group(pid, Signal::KILL);
        }
        let _ = child.kill();
    }
    // collect_bounded observes exit with WNOWAIT, preserving ownership of the
    // leader PID until group cleanup is complete (no process-group ID reuse).
    let reaped = child.wait().map_err(|_| SnapperCliError::CommandFailed);
    match result {
        Ok(bytes) => reaped.map(|_| bytes),
        Err(error) => Err(error),
    }
}
fn collect_bounded(child: &mut Child, deadline: Instant) -> Result<Vec<u8>, SnapperCliError> {
    let mut stdout = child.stdout.take().ok_or(SnapperCliError::SpawnFailed)?;
    let mut stderr = child.stderr.take().ok_or(SnapperCliError::SpawnFailed)?;
    for descriptor in [&stdout as &dyn std::os::fd::AsFd, &stderr as &dyn std::os::fd::AsFd] {
        let flags = fcntl_getfl(descriptor).map_err(|_| SnapperCliError::CommandFailed)?;
        fcntl_setfl(descriptor, flags | OFlags::NONBLOCK)
            .map_err(|_| SnapperCliError::CommandFailed)?;
    }
    let mut output = CappedPipe::new(MAX_STDOUT_BYTES);
    let mut errors = CappedPipe::new(MAX_STDERR_BYTES);
    loop {
        if Instant::now() >= deadline {
            return Err(SnapperCliError::TimedOut);
        }
        // At most one bounded read from each pipe per turn prevents continuous
        // output from starving the deadline or the other pipe. Continue to
        // drain overflow without retaining it, preserving existing semantics.
        output.read_once(&mut stdout)?;
        errors.read_once(&mut stderr)?;
        let pid = Pid::from_raw(child.id() as i32).ok_or(SnapperCliError::CommandFailed)?;
        if let Some(status) = waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map_err(|_| SnapperCliError::CommandFailed)?
            && output.done
            && errors.done
        {
            if output.overflow {
                return Err(SnapperCliError::OutputTooLarge);
            }
            if status.exit_status() != Some(0) {
                return Err(SnapperCliError::CommandFailed);
            }
            // Stderr is bounded and discarded, including overflow, as before.
            // Native diagnostics never enter public errors or returned data.
            return Ok(output.bytes);
        }
        thread::sleep(Duration::from_millis(2));
    }
}
struct CappedPipe {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
    done: bool,
}
impl CappedPipe {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(16 * 1024)),
            limit,
            overflow: false,
            done: false,
        }
    }
    fn read_once(&mut self, reader: &mut impl Read) -> Result<(), SnapperCliError> {
        if self.done {
            return Ok(());
        }
        let mut buffer = [0_u8; 8192];
        match reader.read(&mut buffer) {
            Ok(0) => self.done = true,
            Ok(length) => {
                let keep = length.min(self.limit.saturating_sub(self.bytes.len()));
                self.bytes.extend_from_slice(&buffer[..keep]);
                self.overflow |= keep < length;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(SnapperCliError::CommandFailed),
        }
        Ok(())
    }
}

fn snapper_list_args(config: &ConfigName) -> [String; 4] {
    ["--jsonout".to_owned(), "--config".to_owned(), config.as_str().to_owned(), "list".to_owned()]
}

/// Parses the stable machine-readable fields emitted by `snapper --jsonout
/// --config <name> list`. Transaction correlation is read only from explicit
/// `ryoku-transaction-id` userdata; descriptions and timestamps are ignored.
pub fn parse_snapper_json(
    config: &ConfigName,
    bytes: &[u8],
) -> Result<Vec<SnapshotRecord>, SnapperCliError> {
    if bytes.len() > MAX_STDOUT_BYTES {
        return Err(SnapperCliError::OutputTooLarge);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| SnapperCliError::InvalidJson)?;
    let object = value.as_object().ok_or(SnapperCliError::InvalidInventory)?;
    if object.len() != 1 {
        return Err(SnapperCliError::InvalidInventory);
    }
    let rows = object
        .get(config.as_str())
        .and_then(Value::as_array)
        .ok_or(SnapperCliError::InvalidInventory)?;

    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_object().ok_or(SnapperCliError::InvalidInventory)?;
        let id = row
            .get("number")
            .and_then(|number| {
                number
                    .as_u64()
                    .map(|n| n.to_string())
                    .or_else(|| number.as_str().map(str::to_owned))
            })
            .ok_or(SnapperCliError::InvalidInventory)?;
        let id = SnapshotId::new(id).map_err(SnapperCliError::InvalidValue)?;
        let kind = match row.get("type").and_then(Value::as_str) {
            Some("pre") => SnapshotKind::Pre,
            Some("post") => SnapshotKind::Post,
            Some("single") => SnapshotKind::Single,
            _ => return Err(SnapperCliError::InvalidInventory),
        };
        let pre_snapshot = match row.get("pre-number") {
            None | Some(Value::Null) => None,
            Some(Value::Number(number)) => {
                let number = number.as_u64().ok_or(SnapperCliError::InvalidInventory)?;
                Some(SnapshotId::new(number.to_string()).map_err(SnapperCliError::InvalidValue)?)
            }
            Some(Value::String(number)) => {
                Some(SnapshotId::new(number.clone()).map_err(SnapperCliError::InvalidValue)?)
            }
            Some(_) => return Err(SnapperCliError::InvalidInventory),
        };
        if (kind == SnapshotKind::Post) != pre_snapshot.is_some() {
            return Err(SnapperCliError::InvalidInventory);
        }
        let transaction_id = match row
            .get("userdata")
            .and_then(Value::as_object)
            .and_then(|userdata| userdata.get(TRANSACTION_USERDATA_KEY))
        {
            None => None,
            Some(value) => Some(
                TransactionId::new(
                    value.as_str().ok_or(SnapperCliError::InvalidInventory)?.to_owned(),
                )
                .map_err(SnapperCliError::InvalidValue)?,
            ),
        };
        records.push(SnapshotRecord {
            config: config.clone(),
            id,
            kind,
            pre_snapshot,
            transaction_id,
        });
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PairQueryError;

    fn config() -> ConfigName {
        ConfigName::new("root").unwrap()
    }

    #[test]
    fn effective_root_config_requires_unique_flat_exact_mapping() {
        let good = br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","TIMELINE_CREATE":"no"}"#;
        let parsed = parse_root_configuration(good).unwrap();
        assert!(parsed == parse_root_configuration(good).unwrap());
        assert!(
            parsed
                != parse_root_configuration(
                    br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","TIMELINE_CREATE":"yes"}"#
                )
                .unwrap()
        );
        for bad in [
            br#"{"root":{"SUBVOLUME":"/","FSTYPE":"btrfs"}}"#.as_slice(),
            br#"{"SUBVOLUME":"/home","FSTYPE":"btrfs"}"#,
            br#"{"SUBVOLUME":"/","FSTYPE":"ext4"}"#,
            br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","SUBVOLUME":"/"}"#,
            br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","FSTYPE":"btrfs"}"#,
            br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","x":null}"#,
            br#"{"SUBVOLUME":"/"}"#,
            br#"[]"#,
            br#"{"SUBVOLUME":"/","FSTYPE":"btrfs"}{}"#,
        ] {
            assert!(parse_root_configuration(bad).is_err());
        }
        assert!(parse_root_configuration(&vec![b' '; 128 * 1024 + 1]).is_err());
        assert_eq!(root_configuration_args(), ["--jsonout", "--config", "root", "get-config"]);
    }

    #[test]
    fn arguments_are_fixed_and_keep_config_as_one_argument() {
        assert_eq!(snapper_list_args(&config()), ["--jsonout", "--config", "root", "list"]);
    }

    fn checkpoint_request() -> SnapshotPairRequest {
        SnapshotPairRequest {
            config: config(),
            transaction_id: TransactionId::new("restore-1").unwrap(),
        }
    }

    fn rollback_request() -> RootRollbackRequest {
        RootRollbackRequest {
            config: config(),
            source_snapshot: SnapshotId::new("42").unwrap(),
            transaction_id: TransactionId::new("restore-attempt-1:rollback").unwrap(),
        }
    }

    #[test]
    fn rollback_uses_fixed_source_and_attempt_tag_then_verifies_both_new_snapshots() {
        let request = rollback_request();
        let mut calls = 0;
        let receipt = rollback_to_with(&request, |args| {
            calls += 1;
            match calls {
                1 => {
                    assert_eq!(args, ["--jsonout", "--config", "root", "list", "--columns",
                        "number,type,pre-number,userdata,default,active,read-only"]);
                    Ok(br#"{"root":[
                        {"number":42,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"source-transaction"}},
                        {"number":43,"type":"single","default":true,"active":true,"read-only":false},
                        {"number":46,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"restore-attempt-1"}}
                    ]}"#.to_vec())
                }
                2 => {
                    assert_eq!(args, ["--quiet", "--config", "root", "rollback", "--print-number",
                        "--description", "Ryoku offline recovery", "--userdata",
                        "ryoku-transaction-id=restore-attempt-1:rollback", "42"]);
                    Ok(b"45\n".to_vec())
                }
                3 => Ok(br#"{"root":[
                    {"number":42,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"source-transaction"}},
                    {"number":43,"type":"single","default":false,"active":true,"read-only":false},
                    {"number":44,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"restore-attempt-1:rollback"}},
                    {"number":45,"type":"single","default":true,"active":false,"read-only":false,"userdata":{"ryoku-transaction-id":"restore-attempt-1:rollback"}},
                    {"number":46,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"restore-attempt-1"}}
                ]}"#.to_vec()),
                _ => panic!("unexpected retry"),
            }
        })
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(receipt.config, config());
        assert_eq!(receipt.source_snapshot.as_str(), "42");
        assert_eq!(receipt.previous_default_snapshot.as_str(), "43");
        assert_eq!(receipt.backup_snapshot.as_str(), "44");
        assert_eq!(receipt.rollback_target_snapshot.as_str(), "45");
        assert_eq!(receipt.transaction_id.as_str(), "restore-attempt-1:rollback");
    }

    #[test]
    fn rollback_refuses_missing_or_nonsingle_source_before_native_mutation() {
        for inventory in [
            br#"{"root":[]}"#.as_slice(),
            br#"{"root":[{"number":42,"type":"pre","pre-number":null,"default":false,"active":false,"read-only":true}]}"#,
            br#"{"root":[{"number":42,"type":"post","pre-number":41,"default":false,"active":false,"read-only":true}]}"#,
        ] {
            let mut calls = 0;
            assert_eq!(
                rollback_to_with(&rollback_request(), |_| {
                    calls += 1;
                    Ok(inventory.to_vec())
                }),
                Err(SnapperCliError::RollbackSourceUnavailable)
            );
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn rollback_refuses_nonroot_snapper_configuration_without_native_query() {
        let mut request = rollback_request();
        request.config = ConfigName::new("home").unwrap();
        assert_eq!(
            rollback_to_with(&request, |_| panic!("non-root config must not be queried")),
            Err(SnapperCliError::RollbackUnsupportedConfig)
        );
    }

    #[test]
    fn rollback_refuses_transactional_default_and_incomplete_native_state_before_mutation() {
        for (inventory, expected) in [
            (
                br#"{"root":[
                {"number":42,"type":"single","default":false,"active":false,"read-only":true},
                {"number":43,"type":"single","default":true,"active":true,"read-only":true}
            ]}"#
                .as_slice(),
                SnapperCliError::RollbackSourceUnavailable,
            ),
            (
                br#"{"root":[
                {"number":42,"type":"single","default":false,"active":false,"read-only":true},
                {"number":43,"type":"single","default":true,"active":true}
            ]}"#,
                SnapperCliError::InvalidInventory,
            ),
        ] {
            let mut calls = 0;
            assert_eq!(
                rollback_to_with(&rollback_request(), |_| {
                    calls += 1;
                    Ok(inventory.to_vec())
                }),
                Err(expected)
            );
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn rollback_failure_to_verify_exact_created_inventory_is_not_success() {
        let mut calls = 0;
        let result = rollback_to_with(&rollback_request(), |_| {
            calls += 1;
            match calls {
                1 => Ok(br#"{"root":[
                    {"number":42,"type":"single","default":false,"active":false,"read-only":true},
                    {"number":43,"type":"single","default":true,"active":true,"read-only":false}
                ]}"#.to_vec()),
                2 => Ok(b"44\n".to_vec()),
                3 => Ok(br#"{"root":[
                    {"number":42,"type":"single","default":false,"active":false,"read-only":true},
                    {"number":43,"type":"single","default":false,"active":true,"read-only":false},
                    {"number":44,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"restore-attempt-1"}},
                    {"number":45,"type":"single","default":true,"active":false,"read-only":false,"userdata":{"ryoku-transaction-id":"restore-attempt-1"}}
                ]}"#.to_vec()),
                _ => panic!("unexpected retry"),
            }
        });
        assert_eq!(result, Err(SnapperCliError::RollbackNotVerified));
        assert_eq!(calls, 3);
    }

    #[test]
    fn rollback_attempt_inspection_is_read_only_and_scoped_to_exact_tag() {
        let mut calls = 0;
        let rows = inspect_attempt_with(
            &config(),
            &TransactionId::new("restore-attempt-1").unwrap(),
            |args| {
                calls += 1;
                assert_eq!(args, snapper_list_args(&config()));
                Ok(br#"{"root":[
                    {"number":10,"type":"single","userdata":{"ryoku-transaction-id":"restore-attempt-1"}},
                    {"number":11,"type":"single","userdata":{"ryoku-transaction-id":"other-attempt"}}
                ]}"#.to_vec())
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id.as_str(), "10");
    }

    #[test]
    fn booted_target_verification_requires_exact_active_default_and_writable_row() {
        let config = config();
        let target = SnapshotId::new("45").unwrap();
        let transaction = TransactionId::new("restore-attempt-1").unwrap();
        let mut calls = 0;
        verify_running_target_with(&config, &target, &transaction, |args| {
            calls += 1;
            assert_eq!(args, rollback_list_args(&config));
            Ok(br#"{"root":[
                {"number":45,"type":"single","default":true,"active":true,"read-only":false,"userdata":{"ryoku-transaction-id":"restore-attempt-1"}},
                {"number":44,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"restore-attempt-1"}}
            ]}"#.to_vec())
        })
        .unwrap();
        assert_eq!(calls, 1);

        for (flags, expected) in [
            ((true, false, false, "restore-attempt-1"), SnapperCliError::RollbackNotVerified),
            ((false, true, false, "restore-attempt-1"), SnapperCliError::RollbackNotVerified),
            ((true, true, true, "restore-attempt-1"), SnapperCliError::RollbackNotVerified),
            ((true, true, false, "different-attempt"), SnapperCliError::RollbackNotVerified),
        ] {
            let (is_default, is_active, read_only, attempt) = flags;
            assert_eq!(
                verify_running_target_with(&config, &target, &transaction, |_| {
                    Ok(format!(
                        "{{\"root\":[{{\"number\":45,\"type\":\"single\",\"default\":{is_default},\"active\":{is_active},\"read-only\":{read_only},\"userdata\":{{\"ryoku-transaction-id\":\"{attempt}\"}}}}]}}"
                    )
                    .into_bytes())
                }),
                Err(expected)
            );
        }
    }

    #[test]
    fn reboot_handoff_requires_the_tagged_writable_default_and_readonly_backup() {
        let config = config();
        let target = SnapshotId::new("45").unwrap();
        let backup = SnapshotId::new("44").unwrap();
        let transaction = TransactionId::new("restore-attempt-1:rollback").unwrap();
        let inventory = |target_tag: &str,
                         target_default: bool,
                         target_active: bool,
                         target_readonly: bool,
                         backup_tag: &str,
                         backup_default: bool,
                         backup_active: bool,
                         backup_readonly: bool,
                         extra_tagged: bool| {
            let extra = if extra_tagged {
                r#",{"number":46,"type":"single","default":false,"active":false,"read-only":true,"userdata":{"ryoku-transaction-id":"restore-attempt-1:rollback"}}"#
            } else {
                ""
            };
            format!(
                "{{\"root\":[\
                    {{\"number\":43,\"type\":\"single\",\"default\":false,\"active\":true,\"read-only\":false}},\
                    {{\"number\":45,\"type\":\"single\",\"default\":{target_default},\"active\":{target_active},\"read-only\":{target_readonly},\"userdata\":{{\"ryoku-transaction-id\":\"{target_tag}\"}}}},\
                    {{\"number\":44,\"type\":\"single\",\"default\":{backup_default},\"active\":{backup_active},\"read-only\":{backup_readonly},\"userdata\":{{\"ryoku-transaction-id\":\"{backup_tag}\"}}}}{extra}]}}"
            )
            .into_bytes()
        };
        verify_reboot_handoff_with(&config, &target, &backup, &transaction, |_| {
            Ok(inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                false,
                "restore-attempt-1:rollback",
                false,
                false,
                true,
                false,
            ))
        })
        .unwrap();

        for bad in [
            inventory(
                "wrong-tag",
                true,
                false,
                false,
                "restore-attempt-1:rollback",
                false,
                false,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                false,
                false,
                false,
                "restore-attempt-1:rollback",
                false,
                false,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                true,
                false,
                "restore-attempt-1:rollback",
                false,
                false,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                true,
                "restore-attempt-1:rollback",
                false,
                false,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                false,
                "wrong-tag",
                false,
                false,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                false,
                "restore-attempt-1:rollback",
                true,
                false,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                false,
                "restore-attempt-1:rollback",
                false,
                true,
                true,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                false,
                "restore-attempt-1:rollback",
                false,
                false,
                false,
                false,
            ),
            inventory(
                "restore-attempt-1:rollback",
                true,
                false,
                false,
                "restore-attempt-1:rollback",
                false,
                false,
                true,
                true,
            ),
        ] {
            assert_eq!(
                verify_reboot_handoff_with(&config, &target, &backup, &transaction, |_| {
                    Ok(bad.clone())
                }),
                Err(SnapperCliError::RollbackNotVerified)
            );
        }
    }

    #[test]
    fn single_checkpoint_requires_fresh_native_inventory_and_fixed_creation_arguments() {
        let mut calls = 0;
        let id = create_single_with(&checkpoint_request(), |args| {
            calls += 1;
            match calls {
                1 => {
                    assert_eq!(args, snapper_list_args(&config()));
                    Ok(br#"{"root":[{"number":0,"type":"single"}]}"#.to_vec())
                }
                2 => {
                    assert_eq!(args, ["--config", "root", "create", "--type", "single",
                        "--read-only", "--print-number", "--description", "Ryoku checkpoint",
                        "--userdata", "ryoku-transaction-id=restore-1"]);
                    Ok(b"42\n".to_vec())
                }
                3 => Ok(br#"{"root":[{"number":42,"type":"single","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.to_vec()),
                _ => panic!("unexpected retry"),
            }
        }).unwrap();
        assert_eq!(calls, 3);
        assert_eq!(id.as_str(), "42");
    }

    #[test]
    fn pair_pre_requires_fresh_tagged_inventory_and_fixed_creation_arguments() {
        let mut calls = 0;
        let id = create_pre_with(&checkpoint_request(), |args| {
            calls += 1;
            match calls {
                1 => {
                    assert_eq!(args, snapper_list_args(&config()));
                    Ok(br#"{"root":[]}"#.to_vec())
                }
                2 => {
                    assert_eq!(args, ["--config", "root", "create", "--type", "pre",
                        "--print-number", "--description", "Ryoku operation", "--userdata",
                        "ryoku-transaction-id=restore-1"]);
                    Ok(b"10\n".to_vec())
                }
                3 => Ok(br#"{"root":[{"number":10,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.to_vec()),
                _ => panic!("unexpected retry"),
            }
        })
        .unwrap();
        assert_eq!(id.as_str(), "10");
        assert_eq!(calls, 3);
    }

    #[test]
    fn pair_post_uses_exact_pre_number_and_verifies_both_tagged_members() {
        let request = checkpoint_request();
        let pre = SnapshotId::new("10").unwrap();
        let mut calls = 0;
        let id = create_post_with(&request, &pre, |args| {
            calls += 1;
            match calls {
                1 => Ok(br#"{"root":[{"number":10,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.to_vec()),
                2 => {
                    assert_eq!(args, ["--config", "root", "create", "--type", "post",
                        "--pre-number", "10", "--print-number", "--description",
                        "Ryoku operation", "--userdata", "ryoku-transaction-id=restore-1"]);
                    Ok(b"11\n".to_vec())
                }
                3 => Ok(br#"{"root":[
                    {"number":10,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}},
                    {"number":11,"type":"post","pre-number":10,"userdata":{"ryoku-transaction-id":"restore-1"}}
                ]}"#.to_vec()),
                _ => panic!("unexpected retry"),
            }
        })
        .unwrap();
        assert_eq!(id.as_str(), "11");
        assert_eq!(calls, 3);
    }

    #[test]
    fn pair_creation_refuses_duplicate_transactions_and_mismatched_pre_ids() {
        let request = checkpoint_request();
        let mut calls = 0;
        assert_eq!(
            create_pre_with(&request, |_| {
                calls += 1;
                Ok(br#"{"root":[{"number":10,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.to_vec())
            }),
            Err(SnapperCliError::PairAlreadyExists)
        );
        assert_eq!(calls, 1);

        calls = 0;
        assert_eq!(
            create_post_with(&request, &SnapshotId::new("12").unwrap(), |_| {
                calls += 1;
                Ok(br#"{"root":[{"number":10,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.to_vec())
            }),
            Err(SnapperCliError::PairNotVerified)
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn existing_transaction_or_invalid_inventory_never_invokes_create() {
        for (inventory, error) in [
            (br#"{"root":[{"number":42,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.as_slice(), SnapperCliError::CheckpointAlreadyExists),
            (br#"{"root":[{"number":42,"type":"single"},{"number":42,"type":"single"}]}"#.as_slice(), SnapperCliError::InvalidInventory),
            (br#"{"root":[{"number":"042","type":"single"}]}"#.as_slice(), SnapperCliError::InvalidInventory),
        ] {
            let mut calls = 0;
            assert_eq!(create_single_with(&checkpoint_request(), |_| {
                calls += 1;
                assert_eq!(calls, 1);
                Ok(inventory.to_vec())
            }), Err(error));
        }
    }

    #[test]
    fn reported_creation_is_not_success_without_matching_unique_single_snapshot() {
        for after in [
            br#"{"root":[]}"#.as_slice(),
            br#"{"root":[{"number":42,"type":"single"}]}"#.as_slice(),
            br#"{"root":[{"number":43,"type":"single","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.as_slice(),
            br#"{"root":[{"number":42,"type":"pre","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.as_slice(),
            br#"{"root":[{"number":42,"type":"single","userdata":{"ryoku-transaction-id":"restore-1"}},{"number":43,"type":"single","userdata":{"ryoku-transaction-id":"restore-1"}}]}"#.as_slice(),
        ] {
            let mut calls = 0;
            assert_eq!(create_single_with(&checkpoint_request(), |_| {
                calls += 1;
                Ok(match calls { 1 => br#"{"root":[]}"#.to_vec(), 2 => b"42\n".to_vec(), 3 => after.to_vec(), _ => panic!("unexpected mutation/retry") })
            }), Err(SnapperCliError::CheckpointNotVerified));
            assert_eq!(calls, 3);
        }
    }

    #[test]
    fn uncertain_creation_is_returned_without_automatic_retry_or_cleanup() {
        let mut calls = 0;
        assert_eq!(
            create_single_with(&checkpoint_request(), |_| {
                calls += 1;
                match calls {
                    1 => Ok(br#"{"root":[]}"#.to_vec()),
                    2 => Err(SnapperCliError::TimedOut),
                    _ => panic!("must reconcile externally"),
                }
            }),
            Err(SnapperCliError::TimedOut)
        );
        assert_eq!(calls, 2);
        for output in
            [b"0\n".as_slice(), b"042\n", b"42\n43\n", b"+42", b" 42", b"18446744073709551616"]
        {
            let mut calls = 0;
            assert_eq!(
                create_single_with(&checkpoint_request(), |_| {
                    calls += 1;
                    match calls {
                        1 => Ok(br#"{"root":[]}"#.to_vec()),
                        2 => Ok(output.to_vec()),
                        _ => panic!("invalid result must not advance"),
                    }
                }),
                Err(SnapperCliError::CheckpointNotVerified)
            );
        }
    }

    #[test]
    fn parses_snapper_json_and_only_explicit_ryoku_transaction_userdata() {
        let bytes = br#"{"root":[
            {"number":10,"type":"pre","userdata":{"ryoku-transaction-id":"zypp-1"},"description":"ignored"},
            {"number":11,"type":"post","pre-number":10,"userdata":{"ryoku-transaction-id":"zypp-1"}},
            {"number":12,"type":"single","userdata":{"other":"zypp-1"}}
        ]}"#;
        let records = parse_snapper_json(&config(), bytes).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].id.as_str(), "10");
        assert_eq!(records[0].kind, SnapshotKind::Pre);
        assert_eq!(records[0].transaction_id.as_ref().unwrap().as_str(), "zypp-1");
        assert_eq!(records[2].transaction_id, None);
    }

    #[test]
    fn refuses_config_mismatch_unknown_types_bad_ids_and_invalid_userdata() {
        assert_eq!(
            parse_snapper_json(&config(), br#"{"home":[]}"#),
            Err(SnapperCliError::InvalidInventory)
        );
        assert_eq!(
            parse_snapper_json(&config(), br#"{"root":[{"number":1,"type":"future"}]}"#),
            Err(SnapperCliError::InvalidInventory)
        );
        assert!(
            parse_snapper_json(&config(), br#"{"root":[{"number":"../1","type":"pre"}]}"#).is_err()
        );
        assert!(parse_snapper_json(
            &config(),
            br#"{"root":[{"number":1,"type":"pre","userdata":{"ryoku-transaction-id":"bad token"}}]}"#
        )
        .is_err());
        assert!(parse_snapper_json(
            &config(),
            br#"{"root":[{"number":11,"type":"post","userdata":{"ryoku-transaction-id":"zypp-1"}}]}"#
        )
        .is_err());
        assert!(
            parse_snapper_json(
                &config(),
                br#"{"root":[{"number":11,"type":"post","pre-number":"../10"}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn caps_captured_output_and_still_drains_the_reader() {
        let input = vec![b'x'; 32];
        let mut captured = CappedPipe::new(8);
        let mut reader = input.as_slice();
        while !captured.done {
            captured.read_once(&mut reader).unwrap();
        }
        assert_eq!(captured.bytes, b"xxxxxxxx");
        assert!(captured.overflow);
        assert!(reader.is_empty());
    }

    #[test]
    fn mutations_are_explicitly_unavailable() {
        let request = SnapshotPairRequest {
            config: config(),
            transaction_id: TransactionId::new("zypp-1").unwrap(),
        };
        let mut backend = SnapperCliBackend;
        assert_eq!(backend.create_pre(&request), Err(SnapperCliError::MutationUnavailable));
        assert_eq!(
            backend.create_post(&request, &SnapshotId::new("10").unwrap()),
            Err(SnapperCliError::MutationUnavailable)
        );
    }

    #[test]
    fn query_error_remains_typed_without_exposing_command_output() {
        let error: PairQueryError<SnapperCliError> =
            PairQueryError::Backend(SnapperCliError::CommandFailed);
        assert_eq!(error, PairQueryError::Backend(SnapperCliError::CommandFailed));
    }
}

#[cfg(test)]
#[path = "cli_process_tests.rs"]
mod process_tests;
