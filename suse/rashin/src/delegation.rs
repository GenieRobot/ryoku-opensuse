//! Owner-issued account-scoped Ask capabilities. These restrict HTTP authority;
//! they do not isolate a process that can read the owner's files/socket.
use crate::{Error, Result, digest};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Grant {
    hash: String,
    created: Instant,
    lifetime: u32,
    expires_at: u64,
}
#[derive(Default)]
pub(crate) struct Delegations {
    grants: Mutex<BTreeMap<String, Grant>>,
}
impl Grant {
    fn valid(&self) -> bool {
        self.created.elapsed() < Duration::from_secs(u64::from(self.lifetime))
    }
}
impl Delegations {
    pub(crate) fn issue(&self, lifetime: Option<u32>) -> Result<Value> {
        let lifetime = lifetime.unwrap_or(900);
        if !(60..=3600).contains(&lifetime) {
            return Err(Error::InvalidRequest);
        }
        let mut grants = self.grants.lock().map_err(|_| Error::Storage)?;
        grants.retain(|_, grant| grant.valid());
        if grants.len() >= 8 {
            return Err(Error::Limit);
        }
        let mut random = [0; 48];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut random))
            .map_err(|_| Error::Storage)?;
        let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let id = hex(&random[..16]);
        let token = hex(&random[16..]);
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .checked_add(u64::from(lifetime))
            .ok_or(Error::Limit)?;
        grants.insert(
            id.clone(),
            Grant { hash: digest(token.as_bytes()), created: Instant::now(), lifetime, expires_at },
        );
        Ok(
            json!({"id":id,"token":token,"scope":"ask_client","expires_at":expires_at,"expires_in_seconds":lifetime,"persistent":false,"shared_ask_history":true,"actions":["ask","cancel","status","get_index","recent","job"]}),
        )
    }
    pub(crate) fn authenticate(&self, token: &str) -> Result<bool> {
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(false);
        }
        let hash = digest(token.as_bytes());
        let mut grants = self.grants.lock().map_err(|_| Error::Storage)?;
        grants.retain(|_, grant| grant.valid());
        Ok(grants.values().any(|grant| {
            grant.hash.bytes().zip(hash.bytes()).fold(0u8, |different, (a, b)| different | (a ^ b))
                == 0
        }))
    }
    pub(crate) fn list(&self) -> Result<Value> {
        let mut grants = self.grants.lock().map_err(|_| Error::Storage)?;
        grants.retain(|_, grant| grant.valid());
        Ok(
            json!({"grants":grants.iter().map(|(id,grant)|json!({"id":id,"scope":"ask_client","expires_at":grant.expires_at})).collect::<Vec<_>>()}),
        )
    }
    pub(crate) fn revoke(&self, id: &str) -> Result<Value> {
        crate::identifier(id)?;
        self.grants.lock().map_err(|_| Error::Storage)?.remove(id).ok_or(Error::NotFound)?;
        Ok(json!({"revoked":id}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capabilities_are_bounded_hashed_expiring_and_revocable_without_persistence() {
        let grants = Delegations::default();
        assert!(matches!(grants.issue(Some(0)), Err(Error::InvalidRequest)));
        assert!(matches!(grants.issue(Some(3601)), Err(Error::InvalidRequest)));
        let issued = grants.issue(Some(60)).unwrap();
        let token = issued["token"].as_str().unwrap();
        let id = issued["id"].as_str().unwrap();
        assert!(grants.authenticate(token).unwrap());
        assert!(!grants.authenticate("wrong").unwrap());
        assert!(!grants.list().unwrap().to_string().contains(token));
        assert_ne!(grants.grants.lock().unwrap()[id].hash, token);
        grants.grants.lock().unwrap().get_mut(id).unwrap().created =
            Instant::now() - Duration::from_secs(60);
        assert!(!grants.authenticate(token).unwrap());
        for _ in 0..8 {
            grants.issue(None).unwrap();
        }
        assert!(matches!(grants.issue(None), Err(Error::Limit)));
        let id = grants.list().unwrap()["grants"][0]["id"].as_str().unwrap().to_owned();
        grants.revoke(&id).unwrap();
        assert!(grants.issue(None).is_ok());
        assert!(Delegations::default().list().unwrap()["grants"].as_array().unwrap().is_empty());
    }
}
