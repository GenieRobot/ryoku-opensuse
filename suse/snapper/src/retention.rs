//! Explicit transfer from Snapper's automatic cleanup to Ryoku retention.
//! Only exact transaction-tagged, read-only partners can be retained. This
//! operation never deletes a snapshot or grants authorization to a caller.
use crate::cli::run_snapper;
use crate::{
    ConfigName, SnapperCliError, SnapshotId, SnapshotRecord, TransactionId, parse_snapper_json,
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapperRetentionBackend;
impl SnapperRetentionBackend {
    /// Verify before and after modification. Failure after mutation is an
    /// uncertain outcome requiring reconciliation; never automatic retry.
    pub fn retain(
        &self,
        config: &ConfigName,
        snapshot: &SnapshotId,
        transaction: &TransactionId,
    ) -> Result<(), SnapperCliError> {
        retain_with(config, snapshot, transaction, run_snapper)
    }
    pub fn verify_retained(
        &self,
        config: &ConfigName,
        snapshot: &SnapshotId,
        transaction: &TransactionId,
    ) -> Result<(), SnapperCliError> {
        let (_, cleanup) = inspect(config, snapshot, transaction, &mut run_snapper)?;
        if cleanup.is_empty() { Ok(()) } else { Err(SnapperCliError::CheckpointNotVerified) }
    }
}
fn list_args(config: &ConfigName) -> Vec<String> {
    [
        "--jsonout",
        "--config",
        config.as_str(),
        "list",
        "--columns",
        "number,type,pre-number,userdata,read-only,cleanup",
    ]
    .map(str::to_owned)
    .to_vec()
}
fn inspect(
    config: &ConfigName,
    snapshot: &SnapshotId,
    transaction: &TransactionId,
    run: &mut impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<(SnapshotRecord, String), SnapperCliError> {
    let bytes = run(&list_args(config))?;
    let records = parse_snapper_json(config, &bytes)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| SnapperCliError::InvalidJson)?;
    let rows = value
        .get(config.as_str())
        .and_then(Value::as_array)
        .ok_or(SnapperCliError::InvalidInventory)?;
    if rows.len() != records.len() {
        return Err(SnapperCliError::InvalidInventory);
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut found = None;
    for (record, row) in records.into_iter().zip(rows) {
        if !ids.insert(record.id.clone()) {
            return Err(SnapperCliError::InvalidInventory);
        }
        if record.id == *snapshot {
            if snapshot
                .as_str()
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0 && n.to_string() == snapshot.as_str())
                .is_none()
                || record.transaction_id.as_ref() != Some(transaction)
                || row.get("read-only").and_then(Value::as_bool) != Some(true)
            {
                return Err(SnapperCliError::CheckpointNotVerified);
            }
            let cleanup = row
                .get("cleanup")
                .and_then(Value::as_str)
                .ok_or(SnapperCliError::InvalidInventory)?;
            found = Some((record, cleanup.to_owned()));
        }
    }
    found.ok_or(SnapperCliError::CheckpointNotVerified)
}
fn retain_with(
    config: &ConfigName,
    snapshot: &SnapshotId,
    transaction: &TransactionId,
    mut run: impl FnMut(&[String]) -> Result<Vec<u8>, SnapperCliError>,
) -> Result<(), SnapperCliError> {
    let (before, cleanup) = inspect(config, snapshot, transaction, &mut run)?;
    if !cleanup.is_empty() {
        run(&[
            "--config",
            config.as_str(),
            "modify",
            "--cleanup-algorithm",
            "",
            snapshot.as_str(),
        ]
        .map(str::to_owned))?;
    }
    let (after, cleanup) = inspect(config, snapshot, transaction, &mut run)?;
    if before != after || !cleanup.is_empty() {
        return Err(SnapperCliError::CheckpointNotVerified);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row(tag: &str, ro: bool, cleanup: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"root":[{"number":42,"type":"single","userdata":{"ryoku-transaction-id":tag},"read-only":ro,"cleanup":cleanup}]})).unwrap()
    }
    #[test]
    fn retention_requires_exact_owner_readonly_and_verified_cleanup_exclusion() {
        let config = ConfigName::new("root").unwrap();
        let id = SnapshotId::new("42").unwrap();
        let tx = TransactionId::new("txn-1").unwrap();
        let mut calls = 0;
        retain_with(&config, &id, &tx, |args| {
            calls += 1;
            match calls {
                1 => {
                    assert_eq!(args, list_args(&config));
                    Ok(row("txn-1", true, "number"))
                }
                2 => {
                    assert_eq!(
                        args,
                        ["--config", "root", "modify", "--cleanup-algorithm", "", "42"]
                    );
                    Ok(vec![])
                }
                3 => Ok(row("txn-1", true, "")),
                _ => panic!("unexpected retry"),
            }
        })
        .unwrap();
        assert_eq!(calls, 3);
        for (tag, ro) in [("foreign", true), ("txn-1", false)] {
            let mut calls = 0;
            assert!(
                retain_with(&config, &id, &tx, |_| {
                    calls += 1;
                    Ok(row(tag, ro, "number"))
                })
                .is_err()
            );
            assert_eq!(calls, 1);
        }
    }
    #[test]
    fn substituted_or_still_cleanup_owned_poststate_never_passes() {
        for tag in ["txn-1", "foreign"] {
            let mut calls = 0;
            let result = retain_with(
                &ConfigName::new("root").unwrap(),
                &SnapshotId::new("42").unwrap(),
                &TransactionId::new("txn-1").unwrap(),
                |_| {
                    calls += 1;
                    Ok(if calls == 2 {
                        vec![]
                    } else {
                        row(if calls == 1 { "txn-1" } else { tag }, true, "number")
                    })
                },
            );
            assert!(result.is_err());
            assert_eq!(calls, 3);
        }
    }
}
