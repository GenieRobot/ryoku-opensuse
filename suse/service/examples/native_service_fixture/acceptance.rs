//! Native boundary checks; journal history is fixture-seeded while the daemon is stopped.
use super::{
    OPERATOR, child, client, ready,
    support::{Result, require},
    user_command,
};
use ryoku_service::{
    Envelope, ManagedAction, ManagedActionKind, OperationId, OperationJournal, OperationPhase,
    OperationStatus, PROTOCOL_VERSION, Request, RequestId, Response, ResponseBody, ServiceError,
};
use std::{fs, os::unix::fs::MetadataExt};

fn status() -> OperationStatus {
    OperationStatus {
        operation_id: OperationId::new("native-fixture-durable").unwrap(),
        action: ManagedActionKind::CreateCheckpoint,
        phase: OperationPhase::Failed,
        progress: None,
        error: Some(ServiceError::RecoveryUnavailable),
    }
}
pub(super) fn reconnect() -> Result<()> {
    client::user_exchange(
        OPERATOR.uid,
        Request::GetOperation { operation_id: status().operation_id },
        ResponseBody::Operation(status()),
    )
}
fn raw(bytes: &[u8], expected: ServiceError) -> Result<()> {
    let response: Response = serde_json::from_slice(&client::exchange(
        &format!("{}/ryoku-service.sock", OPERATOR.runtime()),
        OPERATOR.uid,
        bytes,
    )?)?;
    require(
        response.version == PROTOCOL_VERSION
            && response.request_id.as_str() == "invalid"
            && response.body == ResponseBody::Error(expected),
        "unexpected invalid request response",
    )
}
pub(super) fn invalid_requests() -> Result<()> {
    raw(b"not-json\n", ServiceError::Malformed("invalid request frame".into()))?;
    raw(b"{\"version\":1,\"request_id\":\"escape\",\"request\":{\"type\":\"exec\",\"data\":{\"command\":\"touch /tmp/ryoku-arbitrary-command\"}}}\n",
        ServiceError::Malformed("invalid request frame".into()))?;
    let stale_version = Envelope {
        version: PROTOCOL_VERSION + 1,
        request_id: RequestId::new("future-version")?,
        request: Request::Discover,
    };
    raw(&client::encoded(&stale_version)?, ServiceError::Unsupported)?;
    client::user_exchange(
        OPERATOR.uid,
        Request::Start { action: ManagedAction::CreateCheckpoint },
        ResponseBody::Error(ServiceError::Unsupported),
    )
}
pub(super) fn stale() -> Result<()> {
    raw(
        &client::encoded(&Envelope {
            version: PROTOCOL_VERSION,
            request_id: RequestId::new("stale-namespace")?,
            request: Request::Discover,
        })?,
        ServiceError::RecoveryUnavailable,
    )
}
pub(super) fn capabilities(helper_pid: u32) -> Result<()> {
    let process = fs::read_to_string(format!("/proc/{helper_pid}/status"))?;
    for field in ["CapPrm:", "CapEff:", "CapBnd:"] {
        let line = process
            .lines()
            .find(|line| line.starts_with(field))
            .ok_or("capability field absent")?;
        require(
            u64::from_str_radix(
                line.split_whitespace().nth(1).ok_or("capability value absent")?,
                16,
            )? == 4,
            "installed helper does not have only CAP_DAC_READ_SEARCH",
        )?;
    }
    require(
        process.lines().any(|line| line == "NoNewPrivs:\t1"),
        "helper no-new-privileges absent",
    )?;
    Ok(())
}
pub(super) fn installed_checks(helper_pid: u32) -> Result<()> {
    capabilities(helper_pid)?;
    user_command(OPERATOR, &["stop", "ryoku-service.service"])?;
    let file = fs::OpenOptions::new().read(true).write(true).open(OPERATOR.journal())?;
    let mut journal = OperationJournal::open(file)?;
    let mut accepted = status();
    accepted.phase = OperationPhase::Accepted;
    accepted.error = None;
    journal.record(accepted)?;
    journal.record(status())?;
    drop(journal);
    let expected = fs::read(OPERATOR.journal())?;
    let inode = fs::metadata(OPERATOR.journal())?.ino();
    require(!expected.is_empty(), "durable fixture history is empty")?;
    user_command(OPERATOR, &["start", "ryoku-service.service"])?;
    ready(OPERATOR)?;
    child(Some(OPERATOR), "reconnect")?;
    child(Some(OPERATOR), "invalid")?;
    // A new client connection reads the same persisted status; no Start is replayed.
    child(Some(OPERATOR), "reconnect")?;
    user_command(OPERATOR, &["restart", "ryoku-service.service"])?;
    ready(OPERATOR)?;
    child(Some(OPERATOR), "reconnect")?;
    let parent = std::path::Path::new(&OPERATOR.journal()).parent().unwrap().to_owned();
    let moved = parent.with_file_name("service-fixture-held");
    fs::rename(&parent, &moved)?;
    let stale_result = child(Some(OPERATOR), "stale");
    fs::rename(&moved, &parent)?;
    stale_result?;
    child(Some(OPERATOR), "reconnect")?;
    require(
        fs::read(OPERATOR.journal())? == expected
            && fs::metadata(OPERATOR.journal())?.ino() == inode,
        "reconnect or denied request mutated durable history",
    )?;
    require(
        !std::path::Path::new("/tmp/ryoku-arbitrary-command").exists(),
        "arbitrary command was executed",
    )?;
    // Extended evidence goes to the private fixture log, retaining compatibility
    // with the existing lab runner's strict public marker schema.
    eprintln!(
        "native_service_installed_capability_mask=passed\nnative_service_nonempty_durable_reconnect=passed\nnative_service_structured_invalid_stale_unsupported=passed\nnative_service_nonempty_history_unchanged=passed"
    );
    Ok(())
}
