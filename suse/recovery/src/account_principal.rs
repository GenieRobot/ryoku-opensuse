//! Original account association for protected per-user generations.
//!
//! A commitment is an equality check, never an authentication or authorization
//! token. Its inputs must come from the trusted account resolver at capture
//! and again at use. Hashing avoids storing NSS names/home paths; it does not
//! make low-entropy account identities secret or detect recreation with the
//! exact same UID, GID, name and home.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Hash};

use crate::{Sha256, ValidationError};

/// Versioned, opaque association sealed with the original capture.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "WirePrincipal", into = "WirePrincipal")]
pub struct AccountPrincipal(Sha256);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePrincipal {
    version: u16,
    commitment: Sha256,
}

impl TryFrom<WirePrincipal> for AccountPrincipal {
    type Error = &'static str;
    fn try_from(value: WirePrincipal) -> Result<Self, Self::Error> {
        if value.version != 1 {
            return Err("invalid account principal");
        }
        Ok(Self(value.commitment))
    }
}
impl From<AccountPrincipal> for WirePrincipal {
    fn from(value: AccountPrincipal) -> Self {
        Self { version: 1, commitment: value.0 }
    }
}
impl std::fmt::Debug for AccountPrincipal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountPrincipal([redacted])")
    }
}

impl AccountPrincipal {
    /// Construct from one trusted, freshly resolved ordinary account. This
    /// public data constructor cannot grant permission to access a generation.
    pub fn from_account(
        uid: u32,
        primary_gid: u32,
        name: &str,
        home: &str,
    ) -> Result<Self, ValidationError> {
        if uid == 0
            || uid == u32::MAX
            || primary_gid == u32::MAX
            || name.is_empty()
            || name.len() > 256
            || name.chars().any(|c| c.is_control() || matches!(c, '/' | ':'))
            || home.len() > 4096
            || !home.starts_with('/')
            || home == "/"
            || home.chars().any(char::is_control)
            || home.split('/').skip(1).any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(ValidationError::InvalidAccountPrincipal);
        }
        let mut hash = Hash::new();
        hash.update(b"ryoku-account-principal-v1\0");
        hash.update(uid.to_be_bytes());
        hash.update(primary_gid.to_be_bytes());
        for field in [name, home] {
            hash.update((field.len() as u64).to_be_bytes());
            hash.update(field.as_bytes());
        }
        Ok(Self(Sha256::new(
            hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EntryKind, FileRecord, Identifier, ManifestError, OperationReason, RecoveryManifest,
        RelativePath, SnapshotLink, StateRoot,
    };

    fn manifest() -> RecoveryManifest {
        let mut manifest = RecoveryManifest::new(
            Identifier::new("transaction-principal").unwrap(),
            Identifier::new("generation-principal").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            "2026-10-16T00:00:00Z",
        )
        .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserRyokuConfig,
                    RelativePath::new("missing.json").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        manifest
    }

    #[test]
    fn legacy_absence_stays_unproven_and_original_principal_is_sealed() {
        let mut legacy = manifest();
        legacy.seal().unwrap();
        let bytes = legacy.canonical_bytes().unwrap();
        assert!(!std::str::from_utf8(&bytes).unwrap().contains("account_principal"));
        let legacy = RecoveryManifest::parse_verified_json(&bytes).unwrap();
        assert!(legacy.account_principal().is_none());
        assert_eq!(legacy.canonical_bytes().unwrap(), bytes);

        let alice = AccountPrincipal::from_account(1000, 100, "alice", "/home/alice").unwrap();
        let mut bound = legacy;
        bound.attach_account_principal(alice.clone());
        assert!(bound.manifest_sha256.is_none());
        bound.seal().unwrap();
        let bytes = bound.canonical_bytes().unwrap();
        let roundtrip = RecoveryManifest::parse_verified_json(&bytes).unwrap();
        assert_eq!(roundtrip.account_principal(), Some(&alice));
        let mut tampered = serde_json::to_value(&bound).unwrap();
        tampered["account_principal"] = serde_json::to_value(
            AccountPrincipal::from_account(1001, 100, "bob", "/home/bob").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            RecoveryManifest::parse_verified_json(&serde_json::to_vec(&tampered).unwrap()),
            Err(ManifestError::DigestMismatch)
        ));
    }

    #[test]
    fn binds_every_account_field_and_redacts_diagnostics() {
        let original = AccountPrincipal::from_account(1000, 100, "alice", "/home/alice").unwrap();
        for (uid, gid, name, home) in [
            (1001, 100, "alice", "/home/alice"),
            (1000, 101, "alice", "/home/alice"),
            (1000, 100, "bob", "/home/alice"),
            (1000, 100, "alice", "/srv/alice"),
        ] {
            assert_ne!(original, AccountPrincipal::from_account(uid, gid, name, home).unwrap());
        }
        assert_eq!(format!("{original:?}"), "AccountPrincipal([redacted])");
        let bytes = serde_json::to_vec(&original).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains("alice"));
        assert!(!text.contains("/home"));
        assert_eq!(serde_json::from_slice::<AccountPrincipal>(&bytes).unwrap(), original);
        let mut wrong = serde_json::to_value(&original).unwrap();
        wrong["version"] = 2.into();
        assert!(serde_json::from_value::<AccountPrincipal>(wrong).is_err());
    }
}
