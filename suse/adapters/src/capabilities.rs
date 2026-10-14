//! Evidence-gated provider capability declarations for M0-06.
//!
//! This is a conservative product allowlist, not runtime detection. A provider
//! operation is protected only when its pre-mutation checkpoint, affected
//! state inventory, and post-state verification are all explicitly proven.

/// A concrete provider operation, rather than a whole application or backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderOperation {
    PackageSearch,
    InstalledPackageAndRepositoryQuery,
    PackageTransaction,
    RepositoryOrTrustMutation,
    SnapperStatusQuery,
    GeneralSnapperAdministration,
    MyrlynWrite,
    CockpitPackageWrite,
    YastModuleWrite,
}

/// Persistent state that a pre-change recovery point must include for an
/// operation. These are product-owned coverage requirements, not a claim that
/// the current checkpoint implementation has captured or verified them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffectedState {
    SystemSnapshot,
    PackageDatabase,
    InstalledRpmPayloadAndConfiguration,
    PackageScriptAndServiceEffects,
    BootKernelAndInitramfsArtifacts,
    RepositoryDefinitionsAndPolicy,
    TrustKeysAndImportedSigningState,
    PackageMetadataCache,
    SnapperConfiguration,
    RyokuCatalogAndRecoveryJournal,
    RyokuConfigurationAndProfiles,
    UnboundedAdministrativeState,
}

const NO_MUTATION_SCOPE: &[AffectedState] = &[];
const PACKAGE_TRANSACTION_SCOPE: &[AffectedState] = &[
    AffectedState::SystemSnapshot,
    AffectedState::PackageDatabase,
    AffectedState::InstalledRpmPayloadAndConfiguration,
    AffectedState::PackageScriptAndServiceEffects,
    AffectedState::BootKernelAndInitramfsArtifacts,
    AffectedState::RepositoryDefinitionsAndPolicy,
    AffectedState::PackageMetadataCache,
    AffectedState::RyokuCatalogAndRecoveryJournal,
    AffectedState::RyokuConfigurationAndProfiles,
];
const REPOSITORY_MUTATION_SCOPE: &[AffectedState] = &[
    AffectedState::SystemSnapshot,
    AffectedState::RepositoryDefinitionsAndPolicy,
    AffectedState::TrustKeysAndImportedSigningState,
    AffectedState::PackageMetadataCache,
    AffectedState::RyokuCatalogAndRecoveryJournal,
    AffectedState::RyokuConfigurationAndProfiles,
];
const SNAPPER_ADMIN_SCOPE: &[AffectedState] = &[
    AffectedState::SystemSnapshot,
    AffectedState::SnapperConfiguration,
    AffectedState::RyokuCatalogAndRecoveryJournal,
    AffectedState::RyokuConfigurationAndProfiles,
];
const UNBOUNDED_SCOPE: &[AffectedState] = &[AffectedState::UnboundedAdministrativeState];

/// Minimum persistent-state coverage required before capturing an operation's
/// linked checkpoint. An empty slice means the operation is read-only. An
/// unbounded scope is intentionally impossible to certify as protected.
pub const fn required_recovery_scope(operation: ProviderOperation) -> &'static [AffectedState] {
    match operation {
        ProviderOperation::PackageSearch
        | ProviderOperation::InstalledPackageAndRepositoryQuery
        | ProviderOperation::SnapperStatusQuery => NO_MUTATION_SCOPE,
        ProviderOperation::PackageTransaction | ProviderOperation::CockpitPackageWrite => {
            PACKAGE_TRANSACTION_SCOPE
        }
        ProviderOperation::RepositoryOrTrustMutation => REPOSITORY_MUTATION_SCOPE,
        ProviderOperation::GeneralSnapperAdministration => SNAPPER_ADMIN_SCOPE,
        ProviderOperation::MyrlynWrite => {
            // Myrlyn can edit repositories as well as run package transactions.
            // A future supported operation must narrow this to one typed action.
            UNBOUNDED_SCOPE
        }
        ProviderOperation::YastModuleWrite => UNBOUNDED_SCOPE,
    }
}

/// How Ryoku presents the operation to the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationSurface {
    /// Controls rendered and dispatched by the native Ryoku application.
    NativeRyoku,
    /// A provider panel rendered inside a Ryoku-owned view. This is a
    /// presentation classification only; it does not assert that any panel is
    /// implemented or that its writes are mediated by the Rust service.
    EmbeddedPanel,
    /// A separately launched desktop application.
    ExternalApplication,
    /// A separately launched browser page or administration panel.
    ExternalBrowserPanel,
}

/// Product-facing conclusion supported by current evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityStatus {
    /// The exposed operation is proven not to mutate state.
    ReadOnly,
    /// Ryoku will not offer this operation in the initial supported workflow.
    Unsupported,
    /// A candidate exists, but the target/provider behavior is unproven.
    AwaitingProof,
    /// All required operation-specific protection evidence is present.
    VerifiedProtected,
}

/// Evidence state for one required recovery boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProofState {
    NotApplicable,
    Missing,
    Verified,
}

/// Evidence required before Ryoku may advertise an operation as protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderCapability {
    pub operation: ProviderOperation,
    pub surface: IntegrationSurface,
    pub status: CapabilityStatus,
    pub mutates_state: bool,
    /// A pre-change capture that runs before the provider can mutate.
    pub pre_mutation_checkpoint: ProofState,
    /// A reviewed exact inventory of every affected persistent path/state.
    pub affected_state_inventory: ProofState,
    /// Provider outcome and resulting state are checked after completion.
    pub post_state_verification: ProofState,
}

impl ProviderCapability {
    /// True only when this record carries the full evidence required for a
    /// protected write. Status alone cannot bypass missing proof predicates.
    pub const fn may_offer_protected_write(self) -> bool {
        self.mutates_state
            && matches!(self.status, CapabilityStatus::VerifiedProtected)
            && matches!(self.pre_mutation_checkpoint, ProofState::Verified)
            && matches!(self.affected_state_inventory, ProofState::Verified)
            && matches!(self.post_state_verification, ProofState::Verified)
    }
}

const MISSING: ProofState = ProofState::Missing;
const NA: ProofState = ProofState::NotApplicable;

/// Initial conservative capability matrix for the operations in issue #15.
/// There are intentionally no protected writes until guest evidence is linked.
pub const INITIAL_CAPABILITIES: &[ProviderCapability] = &[
    ProviderCapability {
        operation: ProviderOperation::PackageSearch,
        surface: IntegrationSurface::NativeRyoku,
        status: CapabilityStatus::AwaitingProof,
        mutates_state: false,
        pre_mutation_checkpoint: NA,
        affected_state_inventory: NA,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::InstalledPackageAndRepositoryQuery,
        surface: IntegrationSurface::NativeRyoku,
        status: CapabilityStatus::AwaitingProof,
        mutates_state: false,
        pre_mutation_checkpoint: NA,
        affected_state_inventory: NA,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::PackageTransaction,
        surface: IntegrationSurface::NativeRyoku,
        status: CapabilityStatus::AwaitingProof,
        mutates_state: true,
        pre_mutation_checkpoint: MISSING,
        affected_state_inventory: MISSING,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::RepositoryOrTrustMutation,
        surface: IntegrationSurface::NativeRyoku,
        status: CapabilityStatus::Unsupported,
        mutates_state: true,
        pre_mutation_checkpoint: MISSING,
        affected_state_inventory: MISSING,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::SnapperStatusQuery,
        surface: IntegrationSurface::NativeRyoku,
        status: CapabilityStatus::AwaitingProof,
        mutates_state: false,
        pre_mutation_checkpoint: NA,
        affected_state_inventory: NA,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::GeneralSnapperAdministration,
        surface: IntegrationSurface::NativeRyoku,
        status: CapabilityStatus::Unsupported,
        mutates_state: true,
        pre_mutation_checkpoint: MISSING,
        affected_state_inventory: MISSING,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::MyrlynWrite,
        surface: IntegrationSurface::ExternalApplication,
        status: CapabilityStatus::Unsupported,
        mutates_state: true,
        pre_mutation_checkpoint: MISSING,
        affected_state_inventory: MISSING,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::CockpitPackageWrite,
        surface: IntegrationSurface::ExternalBrowserPanel,
        status: CapabilityStatus::Unsupported,
        mutates_state: true,
        pre_mutation_checkpoint: MISSING,
        affected_state_inventory: MISSING,
        post_state_verification: MISSING,
    },
    ProviderCapability {
        operation: ProviderOperation::YastModuleWrite,
        surface: IntegrationSurface::ExternalApplication,
        status: CapabilityStatus::Unsupported,
        mutates_state: true,
        pre_mutation_checkpoint: MISSING,
        affected_state_inventory: MISSING,
        post_state_verification: MISSING,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_provider_matrix_has_no_unproven_protected_writes() {
        assert!(INITIAL_CAPABILITIES.iter().all(|capability| {
            !capability.may_offer_protected_write()
                && (capability.status != CapabilityStatus::VerifiedProtected
                    || (!capability.mutates_state
                        && capability.pre_mutation_checkpoint == ProofState::Verified
                        && capability.affected_state_inventory == ProofState::Verified
                        && capability.post_state_verification == ProofState::Verified))
        }));
    }

    #[test]
    fn every_protected_write_proof_predicate_is_required() {
        let verified = ProviderCapability {
            operation: ProviderOperation::PackageTransaction,
            surface: IntegrationSurface::NativeRyoku,
            status: CapabilityStatus::VerifiedProtected,
            mutates_state: true,
            pre_mutation_checkpoint: ProofState::Verified,
            affected_state_inventory: ProofState::Verified,
            post_state_verification: ProofState::Verified,
        };
        assert!(verified.may_offer_protected_write());
        assert!(
            !ProviderCapability { pre_mutation_checkpoint: ProofState::Missing, ..verified }
                .may_offer_protected_write()
        );
        assert!(
            !ProviderCapability { affected_state_inventory: ProofState::Missing, ..verified }
                .may_offer_protected_write()
        );
        assert!(
            !ProviderCapability { post_state_verification: ProofState::Missing, ..verified }
                .may_offer_protected_write()
        );
        assert!(
            !ProviderCapability { mutates_state: false, ..verified }.may_offer_protected_write()
        );
    }

    #[test]
    fn package_mutations_require_linked_system_and_complete_ryoku_state() {
        for operation in
            [ProviderOperation::PackageTransaction, ProviderOperation::CockpitPackageWrite]
        {
            let scope = required_recovery_scope(operation);
            for required in [
                AffectedState::SystemSnapshot,
                AffectedState::PackageDatabase,
                AffectedState::InstalledRpmPayloadAndConfiguration,
                AffectedState::PackageScriptAndServiceEffects,
                AffectedState::BootKernelAndInitramfsArtifacts,
                AffectedState::RyokuCatalogAndRecoveryJournal,
                AffectedState::RyokuConfigurationAndProfiles,
            ] {
                assert!(scope.contains(&required), "{operation:?} omits {required:?}");
            }
        }
    }

    #[test]
    fn ambiguous_external_admin_scope_cannot_be_certified_as_protected() {
        for operation in [ProviderOperation::MyrlynWrite, ProviderOperation::YastModuleWrite] {
            assert_eq!(
                required_recovery_scope(operation),
                &[AffectedState::UnboundedAdministrativeState]
            );
            let entry =
                INITIAL_CAPABILITIES.iter().find(|entry| entry.operation == operation).unwrap();
            assert_eq!(entry.status, CapabilityStatus::Unsupported);
            assert!(!entry.may_offer_protected_write());
        }
    }

    #[test]
    fn repository_mutations_cover_policy_trust_cache_and_ryoku_state() {
        let scope = required_recovery_scope(ProviderOperation::RepositoryOrTrustMutation);
        for required in [
            AffectedState::SystemSnapshot,
            AffectedState::RepositoryDefinitionsAndPolicy,
            AffectedState::TrustKeysAndImportedSigningState,
            AffectedState::PackageMetadataCache,
            AffectedState::RyokuCatalogAndRecoveryJournal,
            AffectedState::RyokuConfigurationAndProfiles,
        ] {
            assert!(scope.contains(&required), "repository mutation omits {required:?}");
        }
    }
}
