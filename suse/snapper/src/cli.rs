//! Bounded read-only adapter for Snapper's JSON list output.

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
        let mut child = Command::new(SNAPPER)
            .env_clear()
            .env("LC_ALL", "C")
            .env("PATH", "/usr/sbin:/usr/bin")
            .args(snapper_list_args(config))
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
        let status = wait_bounded(&mut child, COMMAND_TIMEOUT)?;
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
        parse_snapper_json(config, &stdout)
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
