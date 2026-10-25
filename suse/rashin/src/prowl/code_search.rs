//! Retained Prowl source search over a separately owner-selected enrolled root.
use super::{Connection, completion, read_roots::Captured};
use crate::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    query: String,
}
pub(super) fn path(repo: &str, query: &str) -> Result<String> {
    crate::text(query, 1024)?;
    if repo.len() > 4096 || !repo.starts_with('/') || repo.contains('\0') {
        return Err(Error::InvalidRequest);
    }
    fn encode(value: &str) -> String {
        let mut out = String::new();
        for b in value.bytes() {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                out.push(b as char);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
        out
    }
    let path = format!("/api/code/search?q={}&repo={}&limit=20", encode(query), encode(repo));
    if path.len() > 8192 {
        return Err(Error::Limit);
    }
    Ok(path)
}
#[derive(Deserialize)]
struct Hit {
    file: String,
    start_line: u64,
    snippet: String,
}
impl Hit {
    fn validate(&self, token: &str) -> Result<()> {
        if self.file.len() > 4096
            || self.file.contains('\0')
            || self.start_line > 9_007_199_254_740_991
            || self.snippet.len() > 64 * 1024
            || self.snippet.contains('\0')
            || self.file.contains(token)
            || self.snippet.contains(token)
        {
            Err(Error::ProviderFailed)
        } else {
            Ok(())
        }
    }
}
fn parse(bytes: &[u8], root: &Captured, token: &str) -> Result<Value> {
    let hits: Vec<Hit> = serde_json::from_slice(bytes).map_err(|_| Error::ProviderFailed)?;
    if hits.len() > 20 {
        return Err(Error::Limit);
    }
    let mut result = Vec::new();
    let mut omitted = 0;
    for hit in hits {
        hit.validate(token)?;
        let file = match root.authorize_source_hit(&hit.file) {
            Ok(file) => file,
            Err(_) => {
                omitted += 1;
                continue;
            }
        };
        let mut text = hit.snippet.trim().to_owned();
        if text.len() > 160 {
            let mut end = 160;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        if result.len() < 12 {
            result.push(json!({"file":file,"line":hit.start_line,"text":text}));
        } else {
            omitted += 1;
        }
    }
    Ok(json!({"hits":result,"omitted":omitted,"source":"configured_prowl_index"}))
}
pub(super) fn execute(
    connection: &Connection,
    root: &Captured,
    args: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<String> {
    let args: Arguments = serde_json::from_str(args).map_err(|_| Error::InvalidRequest)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    root.ensure_current()?;
    path(root.path().to_str().ok_or(Error::InvalidRequest)?, &args.query)?;
    let endpoint = completion::Endpoint::CodeSearch {
        repo: root.path().to_str().ok_or(Error::InvalidRequest)?,
        query: &args.query,
    };
    let mut bytes = Vec::new();
    completion::request_endpoint(
        connection,
        endpoint,
        None,
        cancelled,
        deadline.min(Instant::now() + Duration::from_secs(20)),
        &mut |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )?;
    if bytes.windows(connection.token.len()).any(|w| w == connection.token.as_bytes()) {
        return Err(Error::ProviderFailed);
    }
    root.ensure_current()?;
    if cancelled.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    Ok(parse(&bytes, root, &connection.token)?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_escaped_key_in_decoded_snippet_is_rejected_before_truncation_or_disclosure() {
        let bytes = br#"{"file":"main.rs","start_line":1,"snippet":"PRIVATE\u005fMANAGEMENT_KEY"}"#;
        assert!(
            !bytes.windows(b"PRIVATE_MANAGEMENT_KEY".len()).any(|w| w == b"PRIVATE_MANAGEMENT_KEY")
        );
        let hit: Hit = serde_json::from_slice(bytes).unwrap();
        assert_eq!(hit.validate("PRIVATE_MANAGEMENT_KEY"), Err(Error::ProviderFailed));
    }
    #[test]
    fn fixed_route_encodes_all_query_repo_bytes_without_a_url_or_header_escape() {
        let path = path("/tmp/repo & λ", "needle&repo=/etc\r\nX: y").unwrap();
        assert_eq!(
            path,
            "/api/code/search?q=needle%26repo%3D%2Fetc%0D%0AX%3A%20y&repo=%2Ftmp%2Frepo%20%26%20%CE%BB&limit=20"
        );
        assert!(!path.contains(['\r', '\n', ' ']));
        for q in ["", "\0", &"x".repeat(1025)] {
            assert!(super::path("/repo", q).is_err());
        }
        assert!(super::path("https://untrusted.invalid", "needle").is_err());
    }
}
