//! Rashin's account-private assistant runtime. Transport authenticates the
//! kernel peer before deserializing any action. No action executes shell text.
pub mod acp;
pub mod api;
mod ask;
pub mod authorization;
mod delegation;
pub mod http;
pub mod images;
pub mod inspection;
pub mod provider;
pub mod prowl;
pub mod runtime;
pub mod store;
pub mod usage;
pub mod wiki;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

/// Errors deliberately contain no provider body, user path, prompt, or key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    Unauthorized,
    InvalidRequest,
    UnsafeState,
    Storage,
    Busy,
    NotFound,
    Conflict,
    Integrity,
    EvidenceConsentRequired,
    ProviderUnavailable,
    ProviderFailed,
    Cancelled,
    Limit,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn text(value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}

pub(crate) fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}
