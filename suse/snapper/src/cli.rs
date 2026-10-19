//! Bounded Snapper inventory adapter and explicit single-checkpoint capability.

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
