//! Retained Ask control marker: a machine change requires explicit Agent handoff.
//! Marker parsing grants no capability and never launches an Agent itself.
use crate::{Error, Result};
const MARKER: &str = "NEEDS_AGENT:";
const REASON_BYTES: usize = 500;

pub(crate) fn reason(answer: &mut String) -> Option<String> {
    let suffix = answer.trim().strip_prefix(MARKER)?;
    let mut reason = suffix.split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.len() > REASON_BYTES {
        let mut end = REASON_BYTES - '…'.len_utf8();
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
        reason.push('…');
    }
    *answer = if reason.is_empty() { "(no answer)".into() } else { reason.clone() };
    Some(reason)
}

// Hold a possible leading marker across arbitrary UTF-8 delta boundaries.
// Once recognized, the final normalized reason is published atomically instead
// of streaming control syntax or an unbounded provisional reason to the UI.
#[derive(Default)]
pub(crate) struct Progress {
    pending: String,
    ordinary: bool,
    suppressed: bool,
    bytes: usize,
}
impl Progress {
    pub fn feed(&mut self, delta: &str) -> Result<String> {
        self.bytes = self.bytes.checked_add(delta.len()).ok_or(Error::Limit)?;
        if self.bytes > 128 * 1024 {
            return Err(Error::Limit);
        }
        if self.suppressed {
            return Ok(String::new());
        }
        if self.ordinary {
            return Ok(delta.into());
        }
        self.pending.push_str(delta);
        let content = self.pending.trim_start();
        if content.starts_with(MARKER) {
            self.pending.clear();
            self.suppressed = true;
            return Ok(String::new());
        }
        if MARKER.starts_with(content) {
            return Ok(String::new());
        }
        self.ordinary = true;
        Ok(std::mem::take(&mut self.pending))
    }
    pub fn next_round(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_needs_agent_reason_strips_marker_normalizes_whitespace_and_bounds_unicode() {
        let mut answer = "  NEEDS_AGENT: edit the service file\n and restart it  ".into();
        assert_eq!(reason(&mut answer).as_deref(), Some("edit the service file and restart it"));
        assert_eq!(answer, "edit the service file and restart it");
        let mut answer = format!("NEEDS_AGENT: {}", "界".repeat(500));
        let bounded = reason(&mut answer).unwrap();
        assert!(bounded.len() <= 500);
        assert!(bounded.ends_with('…'));
        assert_eq!(answer, bounded);
        let mut empty = "NEEDS_AGENT: \n".into();
        assert_eq!(reason(&mut empty), Some(String::new()));
        assert_eq!(empty, "(no answer)");
        for ordinary in
            ["NEEDS_AGEN", "needs_agent: literal", "A quoted NEEDS_AGENT: marker", "ordinary"]
        {
            let mut answer = ordinary.to_string();
            assert!(reason(&mut answer).is_none());
            assert_eq!(answer, ordinary);
        }
    }
    #[test]
    fn every_marker_split_is_suppressed_but_normal_text_and_new_rounds_continue() {
        for split in 0..=MARKER.len() {
            let mut progress = Progress::default();
            assert!(progress.feed(" \n").unwrap().is_empty());
            assert!(progress.feed(&MARKER[..split]).unwrap().is_empty());
            assert!(progress.feed(&format!("{} reason", &MARKER[split..])).unwrap().is_empty());
            assert!(progress.feed("more reason").unwrap().is_empty());
        }
        let mut progress = Progress::default();
        assert_eq!(progress.feed("N").unwrap(), "");
        assert_eq!(progress.feed("ormal answer").unwrap(), "Normal answer");
        assert_eq!(progress.feed(" continued").unwrap(), " continued");
        progress.next_round();
        assert_eq!(progress.feed(MARKER).unwrap(), "");
        assert_eq!(progress.feed(&"x".repeat(128 * 1024)), Err(Error::Limit));
    }
}
