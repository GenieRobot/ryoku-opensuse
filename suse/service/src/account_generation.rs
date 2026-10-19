//! Per-user generation access after operator authorization and account lookup.
//! Low-level recovery primitives retain their trusted-caller contracts. This
//! guard does not enable helper dispatch, freeze writers, or authorize mutation.

use crate::{ServiceError, account_state::AccountStateRoot};
use ryoku_recovery::{
    AccountPrincipal, RecoveryManifest, StateRoot,
    configuration_stage::{StagedConfiguration, stage_configuration},
    generation::VerifiedGeneration,
    restore::RestorePlan,
};

/// An original sealed generation associated with one authorized operator's
/// pinned account. No manifest field or known commitment grants this access.
pub struct AccountGenerationAccess<'a> {
    account: AccountStateRoot,
    generation: &'a VerifiedGeneration,
}
impl<'a> AccountGenerationAccess<'a> {
    pub(crate) fn new(
        account: AccountStateRoot,
        generation: &'a VerifiedGeneration,
    ) -> Result<Self, ServiceError> {
        let access = Self { account, generation };
        access.revalidate()?;
        Ok(access)
    }

    /// Repeat NSS and pinned-directory validation and require the original
    /// source association. Call with the coordinator lock held and writers
    /// frozen immediately before each preparation or apply boundary.
    pub fn revalidate(&self) -> Result<(), ServiceError> {
        checked_operation(
            self.generation.manifest(),
            &self.account.principal().map_err(account_error)?,
            || self.account.revalidate().map_err(account_error),
            || (),
        )
    }

    /// Trusted integration handoff for preparation/checkpoint/apply code. The
    /// action is never called for a stale account, legacy source, mismatched
    /// principal or changed seal. It must keep writers frozen until completion.
    pub fn with_revalidated_generation<T>(
        &self,
        action: impl FnOnce(&VerifiedGeneration, &AccountStateRoot) -> T,
    ) -> Result<T, ServiceError> {
        checked_operation(
            self.generation.manifest(),
            &self.account.principal().map_err(account_error)?,
            || self.account.revalidate().map_err(account_error),
            || action(self.generation, &self.account),
        )
    }

    /// Stage only after repeating the same account and original-source gate.
    pub fn stage(&self, plan: &RestorePlan) -> Result<StagedConfiguration, ServiceError> {
        self.with_revalidated_generation(|generation, _| {
            stage_configuration(generation, plan).map_err(|_| ServiceError::RecoveryUnavailable)
        })?
    }
}

fn account_error(error: crate::account_state::AccountStateError) -> ServiceError {
    match error {
        crate::account_state::AccountStateError::LookupBusy => ServiceError::Busy,
        _ => ServiceError::RecoveryUnavailable,
    }
}

fn checked_operation<T>(
    manifest: &RecoveryManifest,
    principal: &AccountPrincipal,
    revalidate: impl FnOnce() -> Result<(), ServiceError>,
    action: impl FnOnce() -> T,
) -> Result<T, ServiceError> {
    revalidate()?;
    // A per-user API cannot be used to reach a system-only source. Any
    // system records in a mixed source still require separate privileged
    // scope policy; account association never grants system restore authority.
    if !manifest.files().iter().any(|record| {
        matches!(
            record.root,
            StateRoot::UserRyokuConfig | StateRoot::UserRyokuState | StateRoot::UserRyokuProfile
        )
    }) || manifest.account_principal() != Some(principal)
        || !manifest.is_mutation_ready()
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    Ok(action())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryoku_recovery::{
        EntryKind, FileRecord, Identifier, OperationReason, RelativePath, SnapshotLink,
        VerificationStatus,
    };
    use std::cell::Cell;

    fn principal(uid: u32, gid: u32, name: &str, home: &str) -> AccountPrincipal {
        AccountPrincipal::from_account(uid, gid, name, home).unwrap()
    }
    fn original() -> AccountPrincipal {
        principal(1000, 100, "alice", "/home/alice")
    }
    // Build an original captured fixture, never a runtime enrollment path.
    fn source(root: StateRoot, binding: Option<AccountPrincipal>) -> RecoveryManifest {
        let mut manifest = RecoveryManifest::new(
            Identifier::new("transaction-account").unwrap(),
            Identifier::new("generation-account").unwrap(),
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
                FileRecord::new(root, RelativePath::new("missing").unwrap(), EntryKind::Absent)
                    .unwrap(),
            )
            .unwrap();
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        let mut json = serde_json::to_value(manifest).unwrap();
        if let Some(binding) = binding {
            json["account_principal"] = serde_json::to_value(binding).unwrap();
        }
        let mut manifest: RecoveryManifest = serde_json::from_value(json).unwrap();
        manifest.seal().unwrap();
        RecoveryManifest::parse_verified_json(&manifest.canonical_bytes().unwrap()).unwrap()
    }

    #[test]
    fn absent_only_sources_require_original_binding_for_both_per_user_roots() {
        for root in
            [StateRoot::UserRyokuConfig, StateRoot::UserRyokuState, StateRoot::UserRyokuProfile]
        {
            let legacy = source(root, None);
            assert!(legacy.is_mutation_ready()); // Still inspectable as a legacy artifact.
            assert_eq!(
                checked_operation(&legacy, &original(), || Ok(()), || panic!("legacy handoff")),
                Err::<(), _>(ServiceError::RecoveryUnavailable)
            );
            let bound = source(root, Some(original()));
            assert_eq!(checked_operation(&bound, &original(), || Ok(()), || 42), Ok(42));
        }
    }

    #[test]
    fn another_uid_gid_name_or_home_never_reaches_stage_or_apply_handoff() {
        let bound = source(StateRoot::UserRyokuConfig, Some(original()));
        for target in [
            principal(1001, 100, "alice", "/home/alice"),
            principal(1000, 101, "alice", "/home/alice"),
            principal(1000, 100, "bob", "/home/alice"),
            principal(1000, 100, "alice", "/srv/alice"),
        ] {
            assert_eq!(
                checked_operation(&bound, &target, || Ok(()), || panic!("foreign handoff")),
                Err::<(), _>(ServiceError::RecoveryUnavailable)
            );
        }
    }

    #[test]
    fn stale_account_or_changed_seal_denies_before_any_handoff() {
        let mut bound = source(StateRoot::UserRyokuConfig, Some(original()));
        for error in [ServiceError::Busy, ServiceError::RecoveryUnavailable] {
            assert_eq!(
                checked_operation(
                    &bound,
                    &original(),
                    || Err(error.clone()),
                    || panic!("stale handoff")
                ),
                Err::<(), _>(error)
            );
        }
        bound.created_utc = "2026-10-16T01:00:00Z".into();
        let revalidated = Cell::new(false);
        assert_eq!(
            checked_operation(
                &bound,
                &original(),
                || {
                    revalidated.set(true);
                    Ok(())
                },
                || panic!("changed seal handoff"),
            ),
            Err::<(), _>(ServiceError::RecoveryUnavailable)
        );
        assert!(revalidated.get());
        let system = source(StateRoot::EtcRyoku, None);
        assert_eq!(
            checked_operation(&system, &original(), || Ok(()), || panic!("system handoff")),
            Err::<(), _>(ServiceError::RecoveryUnavailable)
        );
    }
}
