//! Bounded, per-generation tool status. Names are fixed, IDs are hashed;
//! arguments, URLs, paths, inputs and outputs never enter this metadata.
//! Running rows are ephemeral; terminal rows share the private job transaction.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub ordinal: u8,
    pub call_hash: String,
    pub name: String,
    pub phase: Phase,
    pub error: Option<Error>,
}
impl Record {
    pub fn validate(&self) -> Result<()> {
        if self.ordinal >= 32
            || self.call_hash.len() != 64
            || !self.call_hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !matches!(
                self.name.as_str(),
                "system_query"
                    | "read_file"
                    | "list_dir"
                    | "search_code"
                    | "fetch_url"
                    | "unknown_tool"
            )
        {
            return Err(Error::Integrity);
        }
        if matches!(self.phase, Phase::Running | Phase::Completed) && self.error.is_some() {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
pub(crate) fn apply(records: &mut Vec<Record>, event: Record) -> Result<()> {
    event.validate()?;
    let index = event.ordinal as usize;
    if index == records.len() && event.phase == Phase::Running && records.len() < 32 {
        records.push(event);
        return Ok(());
    }
    let old = records.get_mut(index).ok_or(Error::Integrity)?;
    if old.call_hash != event.call_hash
        || old.name != event.name
        || old.phase != Phase::Running
        || event.phase == Phase::Running
    {
        return Err(Error::Integrity);
    }
    *old = event;
    Ok(())
}
pub(crate) fn terminal(
    mut records: Vec<Record>,
    phase: Phase,
    error: Option<Error>,
) -> Vec<Record> {
    for record in &mut records {
        if record.phase == Phase::Running {
            record.phase = phase;
            record.error = error;
        }
    }
    records
}
pub(crate) struct Callbacks<'a> {
    pub text: &'a mut dyn FnMut(&str) -> Result<()>,
    pub tool: &'a mut dyn FnMut(Record) -> Result<()>,
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(phase: Phase) -> Record {
        Record {
            ordinal: 0,
            call_hash: crate::digest("opaque-call-λ".as_bytes()),
            name: "read_file".into(),
            phase,
            error: None,
        }
    }
    #[test]
    fn bounded_monotonic_metadata_preserves_opaque_identity_and_refuses_replays() {
        let mut rows = Vec::new();
        apply(&mut rows, record(Phase::Running)).unwrap();
        apply(&mut rows, record(Phase::Completed)).unwrap();
        assert_eq!(rows[0].phase, Phase::Completed);
        assert!(apply(&mut rows, record(Phase::Failed)).is_err());
        let mut bounded = Vec::new();
        for ordinal in 0..32 {
            let mut row = record(Phase::Running);
            row.ordinal = ordinal;
            apply(&mut bounded, row.clone()).unwrap();
            row.phase = Phase::Completed;
            apply(&mut bounded, row).unwrap();
        }
        assert_eq!(bounded.len(), 32);
        let mut invalid = record(Phase::Running);
        invalid.ordinal = 32;
        assert!(apply(&mut Vec::new(), invalid).is_err());
        let mut invalid = record(Phase::Running);
        invalid.name = "PRIVATE_UNKNOWN_FUNCTION".into();
        assert!(apply(&mut Vec::new(), invalid).is_err());
        let mut invalid = record(Phase::Running);
        invalid.call_hash = "PRIVATE_RAW_ID".into();
        assert!(invalid.validate().is_err());
        let json = serde_json::to_value(&rows).unwrap();
        assert!(json[0].get("arguments").is_none());
        assert!(json[0].get("result").is_none());
    }
    #[test]
    fn interruption_or_cancel_marks_only_unfinished_tools_without_rewriting_completed_work() {
        let mut completed = record(Phase::Completed);
        completed.ordinal = 1;
        let rows = terminal(
            vec![record(Phase::Running), completed.clone()],
            Phase::Cancelled,
            Some(Error::Cancelled),
        );
        assert_eq!(rows[0].phase, Phase::Cancelled);
        assert_eq!(rows[1], completed);
    }
}
