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

/// How Ryoku presents the operation to the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationSurface {
    NativeRyoku,
    ExternalApplication,
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
}
