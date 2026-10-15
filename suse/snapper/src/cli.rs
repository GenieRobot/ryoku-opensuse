//! Bounded Snapper inventory adapter and explicit single-checkpoint capability.

use std::{
    io::{self, Read},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
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
/// Pair mutations on `SnapperCliBackend` remain unavailable. Keep this explicit
/// capability out of read-only status/service request paths.
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
    let mut child = Command::new(SNAPPER)
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| SnapperCliError::SpawnFailed)?;

    let stdout = child.stdout.take().ok_or(SnapperCliError::SpawnFailed)?;
    let stderr = child.stderr.take().ok_or(SnapperCliError::SpawnFailed)?;
    // Both pipes are drained concurrently so a full stderr pipe cannot
    // deadlock the child. Capture is capped while draining continues.
    let stdout_reader = thread::spawn(move || read_capped(stdout, MAX_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_STDERR_BYTES));
    let status = match wait_bounded(&mut child, COMMAND_TIMEOUT) {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(error);
        }
    };
    let (stdout, stdout_overflow) = stdout_reader
        .join()
        .map_err(|_| SnapperCliError::CommandFailed)?
        .map_err(|_| SnapperCliError::CommandFailed)?;
    let (_, stderr_overflow) = stderr_reader
        .join()
        .map_err(|_| SnapperCliError::CommandFailed)?
        .map_err(|_| SnapperCliError::CommandFailed)?;

    if stdout_overflow {
        return Err(SnapperCliError::OutputTooLarge);
    }
    if !status.success() {
        // Do not return Snapper diagnostics: they may contain host paths,
        // account names, or details supplied by configuration files.
        let _ = stderr_overflow;
        return Err(SnapperCliError::CommandFailed);
    }
    Ok(stdout)
}

fn snapper_list_args(config: &ConfigName) -> [String; 4] {
    ["--jsonout".to_owned(), "--config".to_owned(), config.as_str().to_owned(), "list".to_owned()]
}

fn read_capped(mut reader: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::with_capacity(limit.min(16 * 1024));
    let mut overflow = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(captured.len());
        let keep = remaining.min(read);
        captured.extend_from_slice(&buffer[..keep]);
        overflow |= keep < read;
    }
    Ok((captured, overflow))
}

fn wait_bounded(
    child: &mut Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, SnapperCliError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|_| SnapperCliError::CommandFailed)? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SnapperCliError::TimedOut);
        }
        thread::sleep(Duration::from_millis(10));
    }
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
        records.push(SnapshotRecord { config: config.clone(), id, kind, transaction_id });
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
    fn arguments_are_fixed_and_keep_config_as_one_argument() {
        assert_eq!(snapper_list_args(&config()), ["--jsonout", "--config", "root", "list"]);
    }

    fn checkpoint_request() -> SnapshotPairRequest {
        SnapshotPairRequest {
            config: config(),
            transaction_id: TransactionId::new("restore-1").unwrap(),
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
            {"number":11,"type":"post","userdata":{"ryoku-transaction-id":"zypp-1"}},
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
    }

    #[test]
    fn caps_captured_output_and_still_drains_the_reader() {
        let input = vec![b'x'; 32];
        let (captured, overflow) = read_capped(input.as_slice(), 8).unwrap();
        assert_eq!(captured, b"xxxxxxxx");
        assert!(overflow);
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
