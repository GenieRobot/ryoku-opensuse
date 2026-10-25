//! Native issue #20 integration. There is no installed provider dispatcher.
//! Authenticated callers supply a reviewed typed provider; this module owns
//! the machine lock, writer lease, native checkpoints and protected journal.
use crate::{
    ManagedActionKind, OperationId, OperationJournal, PeerIdentity, ServiceError,
    account_state::AccountStateRoot,
    helper_protocol::prepare_operator_account,
    mutation_gate::{
        self, CheckpointEvidence, GateError, JournaledOperation, LinkedGeneration,
        MutationIntegration, MutationStage, PlannedChange, VerifiedLinkedGeneration,
        VerifiedPostChangeGeneration,
    },
};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use ryoku_recovery::{
    Identifier, OperationReason, PackageRevision, RelativePath, SnapshotLink,
    capture::CaptureLimits, capture_session::NativeCaptureSession, generation::GenerationReader,
    installed_capture::InstalledCaptureRequest, installed_profiles::VerifiedProfileBundle,
    machine_state::read_machine_state_record, native_checkpoint::verify_new_root_checkpoint,
    snapper_catalog::ProtectedSnapperCatalog, snapper_layout::discover_layout,
    state_mount::collect_and_open_state_mount,
};
use ryoku_snapper::{ConfigName, SnapperCheckpointBackend, SnapshotPairRequest, TransactionId};
use std::fs::File;

type Result<T> = std::result::Result<T, GateError>;
const JOURNAL: &str = "mutations.log";
const NO_LINKS: ResolveFlags =
    ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_XDEV);

/// Trusted provider boundary; plan() must include current provider state and
/// its complete typed write set. authorize() rechecks permission for that exact
/// plan. Only audited providers may call this API; no RPC accepts callbacks.
pub trait NativeMutationProvider {
    type Output;
    /// Complete server-owned asset declaration, fixed before review and held
    /// unchanged through post-capture. Preference-only providers retain None.
    fn asset_scope(&self) -> Option<ryoku_recovery::ryogami_assets::AssetScope> {
        None
    }
    fn plan(&mut self, session: &NativeCaptureSession) -> Result<PlannedChange>;
    fn authorize(&mut self, plan: &PlannedChange) -> Result<()>;
    fn apply(
        &mut self,
        checkpoint: &VerifiedLinkedGeneration,
        session: &NativeCaptureSession,
    ) -> Result<Self::Output>;
    fn health_check(&mut self, output: &Self::Output) -> Result<()>;
    fn cancellation_requested(&self) -> bool;
}

/// Locally selected immutable input bundle and transaction facts, never client
/// paths. The bundle is revalidated and package association is checked by capture.
pub struct NativeMutationRequest<'a> {
    pub operation_id: OperationId,
    pub action: ManagedActionKind,
    pub reason: OperationReason,
    pub planned: PlannedChange,
    pub created_utc: String,
    pub packages: Vec<PackageRevision>,
    pub bundle: &'a VerifiedProfileBundle,
}

/// Shared authenticated helper entry. No installed action calls it until its
/// provider is separately certified. Account identity comes only from kernel
/// credentials, the operator policy and the bounded NSS resolver.
pub fn run_operator_mutation<P: NativeMutationProvider>(
    peer: &PeerIdentity,
    operator_gid: u32,
    request: NativeMutationRequest<'_>,
    provider: &mut P,
) -> Result<P::Output> {
    let account =
        prepare_operator_account(peer, operator_gid).map_err(|_| GateError::PreflightFailed)?;
    run_bound_mutation(&account, request, provider)
}

/// Review the complete plan while writers are leased. A later execution must
/// recheck it under a fresh lease; this result supplies no authorization token.
pub fn review_operator_plan<P: NativeMutationProvider>(
    peer: &PeerIdentity,
    operator_gid: u32,
    provider: &mut P,
) -> Result<PlannedChange> {
    let account =
        prepare_operator_account(peer, operator_gid).map_err(|_| GateError::PreflightFailed)?;
    let session = NativeCaptureSession::open(
        account.capture_account().map_err(|_| GateError::PreflightFailed)?,
        account.capture_home().map_err(|_| GateError::PreflightFailed)?,
    )
    .map_err(|_| GateError::CheckpointUnverified)?;
    let mut plan = provider.plan(&session)?;
    plan.affected_state_fingerprint =
        scoped_fingerprint(&session, provider.asset_scope().as_ref())?;
    account.revalidate().map_err(|_| GateError::PlanChanged)?;
    Ok(plan)
}

fn run_bound_mutation<P: NativeMutationProvider>(
    account: &AccountStateRoot,
    request: NativeMutationRequest<'_>,
    provider: &mut P,
) -> Result<P::Output> {
    account.revalidate().map_err(|_| GateError::PreflightFailed)?;
    // Session acquisition owns /var/lib/ryoku-state/mutation.lock before the
    // first journal read/repair and stops the account's registered writers.
    let session = NativeCaptureSession::open(
        account.capture_account().map_err(|_| GateError::PreflightFailed)?,
        account.capture_home().map_err(|_| GateError::PreflightFailed)?,
    )
    .map_err(|_| GateError::CheckpointUnverified)?;
    let mut journal = open_journal(
        session.coordinator_directory().map_err(|_| GateError::CheckpointUnverified)?,
    )?;
    let root = File::open("/").map_err(|_| GateError::PreflightFailed)?;
    let planned = request.planned.clone();
    let mut operation =
        JournaledOperation::new(&mut journal, request.operation_id.clone(), request.action);
    let asset_scope = provider.asset_scope();
    if let Some(scope) = &asset_scope {
        scope.validate().map_err(|_| GateError::InvalidPlan)?;
    }
    let mut integration =
        NativeIntegration { account, session: &session, root, provider, request, asset_scope };
    mutation_gate::coordinate_mutation_locked(&planned, &mut operation, &mut integration)
}

pub(crate) fn open_journal(directory: &File) -> Result<OperationJournal> {
    let file = File::from(
        fs::openat2(
            directory,
            JOURNAL,
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::from_raw_mode(0o600),
            NO_LINKS,
        )
        .map_err(|_| GateError::PreflightFailed)?,
    );
    let stat = fs::fstat(&file).map_err(|_| GateError::PreflightFailed)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o7777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(GateError::PreflightFailed);
    }
    fs::fsync(directory).map_err(|_| GateError::PreflightFailed)?;
    OperationJournal::open(file).map_err(GateError::Journal)
}

/// Helper startup and console history share this exact protected location and
/// lock. Holding this view serializes journal replay/repair against mutation.
/// Root-only local access; never accept a caller-supplied path or state identity.
pub struct ProtectedMutationHistory {
    _guard: mutation_gate::WriterGuard,
    journal: OperationJournal,
}
impl ProtectedMutationHistory {
    pub fn open() -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 {
            return Err(GateError::PreflightFailed);
        }
        let root = File::open("/").map_err(|_| GateError::PreflightFailed)?;
        let machine = read_machine_state_record(&root).map_err(|_| GateError::PreflightFailed)?;
        let (state, _) = collect_and_open_state_mount(&root, machine.identity())
            .map_err(|_| GateError::PreflightFailed)?;
        let directory = File::from(state);
        let lock = mutation_gate::MutationLock::open(
            directory.try_clone().map_err(|_| GateError::PreflightFailed)?,
            0,
        )?;
        let guard = lock.acquire()?;
        let journal = open_journal(&directory)?;
        Ok(Self { _guard: guard, journal })
    }
    pub fn journal(&self) -> &OperationJournal {
        &self.journal
    }

    /// Explicit root console reconciliation only for a recorded boundary that
    /// cannot have entered apply. Never clears potentially mutating work.
    pub fn reconcile_before_change(&mut self, id: &OperationId) -> Result<()> {
        let mut context =
            self.journal.mutation_context(id).cloned().ok_or(GateError::RecoveryRequired)?;
        let mut status = self.journal.get(id).cloned().ok_or(GateError::RecoveryRequired)?;
        if context.outcome.is_some()
            || !matches!(
                context.stage,
                MutationStage::Preflight
                    | MutationStage::StateChecking
                    | MutationStage::Capturing
                    | MutationStage::Revalidating
            )
        {
            return Err(GateError::RecoveryRequired);
        }
        context.outcome = Some(mutation_gate::MutationOutcome::FailedBeforeChange);
        status.phase = crate::OperationPhase::Failed;
        status.error = Some(ServiceError::RecoveryUnavailable);
        self.journal.record_mutation(status, context).map_err(GateError::Journal)
    }
}

struct NativeIntegration<'a, P> {
    account: &'a AccountStateRoot,
    session: &'a NativeCaptureSession,
    root: File,
    provider: &'a mut P,
    request: NativeMutationRequest<'a>,
    asset_scope: Option<ryoku_recovery::ryogami_assets::AssetScope>,
}

fn scoped_fingerprint(
    session: &NativeCaptureSession,
    scope: Option<&ryoku_recovery::ryogami_assets::AssetScope>,
) -> Result<String> {
    let fingerprint = match scope {
        Some(scope) => session.inventory_fingerprint_with_assets(scope),
        None => session.inventory_fingerprint(),
    }
    .map_err(|_| GateError::PlanChanged)?;
    Ok(fingerprint.as_str().into())
}

impl<P: NativeMutationProvider> NativeIntegration<'_, P> {
    fn current_plan(&mut self) -> Result<PlannedChange> {
        let mut plan = self.provider.plan(self.session)?;
        if self.provider.asset_scope() != self.asset_scope {
            return Err(GateError::PlanChanged);
        }
        plan.affected_state_fingerprint =
            scoped_fingerprint(self.session, self.asset_scope.as_ref())?;
        Ok(plan)
    }
    fn boundary(&self) -> Result<()> {
        self.account.revalidate().map_err(|_| GateError::PlanChanged)?;
        self.session.revalidate().map_err(|_| GateError::CheckpointUnverified)?;
        self.request.bundle.revalidate().map_err(|_| GateError::CheckpointUnverified)
    }
    fn capture_native(&self, post: bool) -> Result<CheckpointEvidence> {
        self.boundary()?;
        if self.provider.asset_scope() != self.asset_scope {
            return Err(GateError::PlanChanged);
        }
        let suffix = if post { "post" } else { "pre" };
        let transaction =
            Identifier::new(format!("{}-{suffix}", self.request.operation_id.as_str()))
                .map_err(|_| GateError::InvalidPlan)?;
        let generation =
            Identifier::new(format!("{}-{suffix}-generation", self.request.operation_id.as_str()))
                .map_err(|_| GateError::InvalidPlan)?;
        let machine =
            read_machine_state_record(&self.root).map_err(|_| GateError::CheckpointUnverified)?;
        let snapshot = SnapperCheckpointBackend
            .create_single(&SnapshotPairRequest {
                config: ConfigName::new("root").map_err(|_| GateError::InvalidPlan)?,
                transaction_id: TransactionId::new(transaction.as_str())
                    .map_err(|_| GateError::InvalidPlan)?,
            })
            .map_err(|_| GateError::CheckpointUnverified)?;
        let native = verify_new_root_checkpoint(
            &self.root,
            SnapshotLink::new(
                Identifier::new("root").map_err(|_| GateError::InvalidPlan)?,
                snapshot.as_str(),
                &machine.identity().filesystem_uuid,
            )
            .map_err(|_| GateError::CheckpointUnverified)?,
        )
        .map_err(|_| GateError::CheckpointUnverified)?;
        let request = InstalledCaptureRequest {
            transaction_id: transaction.clone(),
            generation_id: generation,
            parent_generation_id: None,
            snapshot: native.verified_link().clone(),
            reason: self.request.reason.clone(),
            created_utc: self.request.created_utc.clone(),
            package_revisions: self.request.packages.clone(),
            provider_revision: None,
            limits: CaptureLimits::default(),
        };
        match &self.asset_scope {
            Some(scope) => self.session.capture_with_assets(request, self.request.bundle, scope),
            None => self.session.capture(request, self.request.bundle),
        }
        .map_err(|_| GateError::CheckpointUnverified)?;
        // Do not call a generation complete based on a saved flag. Reopen exact
        // immutable bytes and the original native receipt before publication.
        let reader =
            GenerationReader::open_state_mount().map_err(|_| GateError::CheckpointUnverified)?;
        let linked = reader
            .open_historical_transaction(&self.root, &transaction)
            .map_err(|_| GateError::CheckpointUnverified)?;
        self.session
            .verify_inventory(linked.generation().manifest())
            .map_err(|_| GateError::PlanChanged)?;
        let identity = LinkedGeneration::from_verified_generation(linked.generation())?;
        let catalog = ProtectedSnapperCatalog::open(&self.root)
            .map_err(|_| GateError::CheckpointUnverified)?;
        catalog.publish(&transaction, &[]).map_err(|_| GateError::CheckpointUnverified)?;
        if !catalog.status(&transaction).map_err(|_| GateError::CheckpointUnverified)?.complete {
            return Err(GateError::CheckpointUnverified);
        }
        self.boundary()?;
        Ok(CheckpointEvidence {
            linked: identity,
            system_component_verified: true,
            ryoku_component_verified: true,
            contents_and_metadata_verified: true,
        })
    }
    fn recheck(&mut self, plan: &PlannedChange, proof: &VerifiedLinkedGeneration) -> Result<bool> {
        self.boundary()?;
        self.provider.authorize(plan)?;
        if self.current_plan()? != *plan {
            return Ok(false);
        }
        let reader =
            GenerationReader::open_state_mount().map_err(|_| GateError::CheckpointUnverified)?;
        let linked = reader
            .open_historical_transaction(&self.root, &proof.linked().transaction_id)
            .map_err(|_| GateError::CheckpointUnverified)?;
        if LinkedGeneration::from_verified_generation(linked.generation())? != *proof.linked() {
            return Ok(false);
        }
        self.session
            .verify_inventory(linked.generation().manifest())
            .map_err(|_| GateError::PlanChanged)?;
        if !ProtectedSnapperCatalog::open(&self.root)
            .and_then(|catalog| catalog.status(&proof.linked().transaction_id))
            .map_err(|_| GateError::CheckpointUnverified)?
            .complete
        {
            return Err(GateError::CheckpointUnverified);
        }
        self.boundary()?;
        Ok(true)
    }
}
impl<P: NativeMutationProvider> MutationIntegration for NativeIntegration<'_, P> {
    type Output = P::Output;
    fn preflight(&mut self, plan: &PlannedChange) -> Result<()> {
        self.boundary()?;
        self.provider.authorize(plan)?;
        if self.current_plan()? != *plan {
            return Err(GateError::PlanChanged);
        }
        Ok(())
    }
    fn state_space_check(&mut self, _: &PlannedChange) -> Result<()> {
        self.boundary()?;
        self.session.ensure_checkpoint_space().map_err(|_| GateError::InsufficientSpace)?;
        if let Some(scope) = &self.asset_scope {
            self.session
                .ensure_asset_checkpoint_space(scope)
                .map_err(|_| GateError::InsufficientSpace)?;
        }
        let stats = fs::fstatvfs(&self.root).map_err(|_| GateError::InsufficientSpace)?;
        if stats.f_bavail.saturating_mul(stats.f_frsize) < 128 * 1024 * 1024 {
            return Err(GateError::InsufficientSpace);
        }
        let layout = discover_layout(
            &self.root,
            &RelativePath::new("etc").map_err(|_| GateError::PreflightFailed)?,
        )
        .map_err(|_| GateError::PreflightFailed)?;
        // No uncoordinated extra subvolume is silently covered. This executor
        // supports only root plus the enrolled account inventory.
        if layout.targets.iter().any(|target| target.config.as_str() != "root") {
            return Err(GateError::PreflightFailed);
        }
        ProtectedSnapperCatalog::open(&self.root).map_err(|_| GateError::CheckpointUnverified)?;
        Ok(())
    }
    fn capture(&mut self, _: &PlannedChange) -> Result<CheckpointEvidence> {
        self.capture_native(false)
    }
    fn revalidate(
        &mut self,
        plan: &PlannedChange,
        proof: &VerifiedLinkedGeneration,
    ) -> Result<bool> {
        self.recheck(plan, proof)
    }
    fn apply(&mut self, proof: &VerifiedLinkedGeneration) -> Result<Self::Output> {
        // Repeat native/plan checks after Applying has been synchronized, just
        // before the first provider call. Any failure is conservatively marked
        // potentially mutating because durable history has crossed that boundary.
        if !self.recheck(&self.request.planned.clone(), proof)? {
            return Err(GateError::PlanChanged);
        }
        self.provider.apply(proof, self.session)
    }
    fn health_check(&mut self, output: &Self::Output, _: &VerifiedLinkedGeneration) -> Result<()> {
        self.boundary()?;
        self.provider.health_check(output)
    }
    fn capture_post_change(
        &mut self,
        _: &Self::Output,
        proof: &VerifiedLinkedGeneration,
    ) -> Result<VerifiedPostChangeGeneration> {
        VerifiedPostChangeGeneration::validate(
            self.capture_native(true).map_err(|_| GateError::PostChangeFailed)?,
            proof,
        )
    }
    fn cancellation_requested(&self) -> bool {
        self.provider.cancellation_requested()
    }
}
