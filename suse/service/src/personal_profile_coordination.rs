//! Concrete native profile orchestration. No integration attestations or
//! arbitrary callbacks can manufacture the checkpoint used by this writer.
use super::*;
use crate::{
    ManagedActionKind, OperationId,
    mutation_gate::{
        GateError, JournaledOperation, LinkedGeneration, MutationStage, PlannedChange,
        pre_change_error,
    },
    protected_operation_journal::ProtectedOperationJournal,
};
use std::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Returned only after fresh native read-back and the durable Complete row.
/// These opaque proofs cover the personal-profile writer's exact scope only.
pub struct PersonalProfileCompletion {
    before: VerifiedPersonalProfileCheckpoint,
    after: VerifiedPersonalProfileCheckpoint,
}
impl PersonalProfileCompletion {
    pub fn before(&self) -> &VerifiedPersonalProfileCheckpoint {
        &self.before
    }
    pub fn after(&self) -> &VerifiedPersonalProfileCheckpoint {
        &self.after
    }
}

impl PersonalProfilePlan {
    /// Commit to the exact prepared source, account, packaged defaults, and new
    /// canonical document. No profile values are stored in operation history.
    pub fn reviewed_change(&self) -> std::result::Result<PlannedChange, GateError> {
        let inventory = serde_json::to_vec(&(
            "ryoku-personal-profile-writer-v1",
            &self.principal,
            &self.original,
        ))
        .map_err(|_| GateError::InvalidPlan)?;
        let planned = serde_json::to_vec(&(
            "ryoku-personal-profile-plan-v1",
            &self.principal,
            &self.original,
            self.defaults.id().map_err(|_| GateError::InvalidPlan)?,
            digest(&self.desired),
        ))
        .map_err(|_| GateError::InvalidPlan)?;
        let principal = serde_json::to_vec(&self.principal).map_err(|_| GateError::InvalidPlan)?;
        Ok(PlannedChange {
            operation_key: format!("personal-profile:{}", digest(&principal).as_str()),
            plan_fingerprint: digest(&planned).as_str().to_owned(),
            affected_state_fingerprint: digest(&inventory).as_str().to_owned(),
        })
    }
}

impl PersonalProfileCoordinator {
    /// Root-authorized building block for a reviewed native executor. There is
    /// no installed dispatch or account-writer handoff. The installed helper's
    /// ProtectHome/capability policy cannot run this writer and stays unchanged.
    ///
    /// Acquire writer_lock once, prepare/review the plan under that guard, and
    /// retain it until this returns. Cancellation after Applying cannot interrupt
    /// the writer or skip verification. External editors/RPM are not locked out.
    pub fn execute(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        operation_id: OperationId,
        cancelled: &AtomicBool,
    ) -> std::result::Result<PersonalProfileCompletion, GateError> {
        self.revalidate(guard).map_err(before_error)?;
        let planned = plan.reviewed_change()?;
        let ids = CaptureIds::for_operation(&operation_id)?;
        let mut journal = ProtectedOperationJournal::open(&self.state, &self.lock, guard)?;
        let mut operation = JournaledOperation::new(
            journal.journal(),
            operation_id,
            ManagedActionKind::ApplyProfile,
        );
        operation.begin(&planned)?;
        check_cancelled(&mut operation, cancelled)?;
        if let Err(error) = self.preflight(guard, plan) {
            return Err(operation.fail(pre_change_error(&error), error));
        }
        check_cancelled(&mut operation, cancelled)?;

        operation.advance(MutationStage::Capturing)?;
        let before = match current_utc().and_then(|time| {
            self.capture(guard, plan, ids.before_transaction, ids.before_generation, time)
                .map_err(before_error)
        }) {
            Ok(checkpoint) => checkpoint,
            Err(error) => return Err(operation.fail(pre_change_error(&error), error)),
        };
        let before_reference = match checkpoint_reference(&before) {
            Ok(reference) => reference,
            Err(error) => return Err(operation.fail(pre_change_error(&error), error)),
        };
        operation.captured(before_reference.clone())?;
        check_cancelled(&mut operation, cancelled)?;
        if let Err(error) = self.revalidate_checkpoint(guard, plan, &before).map_err(before_error) {
            return Err(operation.fail(pre_change_error(&error), error));
        }
        check_cancelled(&mut operation, cancelled)?;

        // No writer entry before this record has been synchronized.
        operation.advance(MutationStage::Applying)?;
        let applied = match self.apply(guard, plan, &before) {
            Ok(applied) => applied,
            Err(_) => return Err(operation.fail(ServiceError::Failed, GateError::ApplyFailed)),
        };
        operation.advance(MutationStage::HealthChecking)?;
        if self.health_check(guard, plan, &before, &applied).is_err() {
            return Err(operation.fail(ServiceError::Failed, GateError::PostChangeFailed));
        }

        operation.advance(MutationStage::PostChangeCapture)?;
        let post_plan = PersonalProfilePlan {
            original: applied,
            defaults: plan.defaults.clone(),
            desired: plan.desired.clone(),
            principal: plan.principal.clone(),
        };
        let after = match current_utc().and_then(|time| {
            self.capture_scoped(
                guard,
                &post_plan,
                ids.after_transaction,
                ids.after_generation,
                Some(before.manifest().generation_id.clone()),
                time,
            )
            .map_err(|_| GateError::PostChangeFailed)
        }) {
            Ok(checkpoint) => checkpoint,
            Err(_) => {
                return Err(operation.fail(ServiceError::Failed, GateError::PostChangeFailed));
            }
        };
        // Reopen both partners again before success; preimage unavailability,
        // source/package drift or a substituted postimage cannot become Complete.
        let final_check = self
            .revalidate_checkpoint(guard, &post_plan, &after)
            .and_then(|_| self.health_check(guard, plan, &before, &post_plan.original))
            .map_err(|_| GateError::PostChangeFailed)
            .and_then(|_| checkpoint_reference(&after))
            .and_then(|reference| {
                if reference.fresh_after(&before_reference)
                    && after.manifest().parent_generation_id.as_ref()
                        == Some(&before.manifest().generation_id)
                {
                    Ok(reference)
                } else {
                    Err(GateError::PostChangeFailed)
                }
            });
        let after_reference = match final_check {
            Ok(reference) => reference,
            Err(_) => {
                return Err(operation.fail(ServiceError::Failed, GateError::PostChangeFailed));
            }
        };
        operation.complete(after_reference)?;
        Ok(PersonalProfileCompletion { before, after })
    }

    fn preflight(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
    ) -> std::result::Result<(), GateError> {
        self.revalidate_plan(guard, plan).map_err(before_error)?;
        // Conservative fixed floors for this bounded scope, not reservations
        // or a proof that Btrfs metadata allocation cannot later return ENOSPC.
        ensure_space(&self.state, 4 * 1024 * 1024, 64)?;
        ensure_space(self.account.configuration(), plan.desired.len() as u64, 8)?;
        ensure_space(
            &File::open("/").map_err(|_| GateError::PreflightFailed)?,
            4 * 1024 * 1024,
            64,
        )?;
        Ok(())
    }

    fn health_check(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        before: &VerifiedPersonalProfileCheckpoint,
        applied: &FileRecord,
    ) -> Result<()> {
        self.revalidate_retained_checkpoint(guard, plan, before)?;
        validate_applied_record(plan, applied, self.owner())?;
        if read_personal(self.account.configuration(), self.owner())? != *applied {
            return Err(ServiceError::StaleRequest);
        }
        if matches!(plan.original.kind, EntryKind::Absent) {
            let file = File::from(
                fs::openat2(
                    self.account.configuration(),
                    PROFILE,
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                    Mode::empty(),
                    RESOLVE,
                )
                .map_err(unavailable)?,
            );
            label::validate_new_profile_label(&self.account, &file)?;
        }
        self.revalidate(guard)?;
        if read_personal(self.account.configuration(), self.owner())? != *applied {
            return Err(ServiceError::StaleRequest);
        }
        Ok(())
    }
}

pub(super) fn validate_applied_record(
    plan: &PersonalProfilePlan,
    applied: &FileRecord,
    owner: ProfileOwner,
) -> Result<()> {
    let EntryKind::RegularFile { metadata, sha256, size, hard_link_group } = &applied.kind else {
        return Err(ServiceError::RecoveryUnavailable);
    };
    if applied.root != plan.original.root
        || applied.path != plan.original.path
        || *sha256 != digest(&plan.desired)
        || *size != plan.desired.len() as u64
        || metadata.uid != owner.uid
        || metadata.gid != owner.gid
        || hard_link_group.is_some()
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    match &plan.original.kind {
        EntryKind::Absent if metadata.mode == ryoku_profiles::PERSONAL_PROFILE_CREATE_MODE => {
            Ok(())
        }
        EntryKind::RegularFile { metadata: original, .. }
            if metadata.mode == original.mode
                && metadata.acl_sha256 == original.acl_sha256
                && metadata.extended == original.extended =>
        {
            Ok(())
        }
        _ => Err(ServiceError::RecoveryUnavailable),
    }
}

fn ensure_space(
    file: &File,
    bytes: u64,
    allocation_blocks: u64,
) -> std::result::Result<(), GateError> {
    let stats = fs::fstatvfs(file).map_err(|_| GateError::PreflightFailed)?;
    let block = if stats.f_frsize == 0 { stats.f_bsize } else { stats.f_frsize };
    check_space(stats.f_bavail, block, bytes, allocation_blocks)
}
fn check_space(
    available_blocks: u64,
    block: u64,
    bytes: u64,
    allocation_blocks: u64,
) -> std::result::Result<(), GateError> {
    let available = available_blocks.checked_mul(block).ok_or(GateError::PreflightFailed)?;
    let required = allocation_blocks
        .checked_mul(block)
        .and_then(|extra| extra.checked_add(bytes))
        .ok_or(GateError::PreflightFailed)?;
    if block == 0 {
        return Err(GateError::PreflightFailed);
    }
    if available < required {
        return Err(GateError::InsufficientSpace);
    }
    Ok(())
}
fn before_error(error: ServiceError) -> GateError {
    match error {
        ServiceError::StaleRequest => GateError::PlanChanged,
        _ => GateError::PreflightFailed,
    }
}
fn checkpoint_reference(
    checkpoint: &VerifiedPersonalProfileCheckpoint,
) -> std::result::Result<LinkedGeneration, GateError> {
    LinkedGeneration::from_verified_generation(checkpoint.linked.generation())
}
fn check_cancelled(
    operation: &mut JournaledOperation<'_>,
    cancelled: &AtomicBool,
) -> std::result::Result<(), GateError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(operation.fail(ServiceError::Failed, GateError::CancelledBeforeChange));
    }
    Ok(())
}
struct CaptureIds {
    before_transaction: Identifier,
    before_generation: Identifier,
    after_transaction: Identifier,
    after_generation: Identifier,
}
impl CaptureIds {
    fn for_operation(operation: &OperationId) -> std::result::Result<Self, GateError> {
        let hash = digest(operation.as_str().as_bytes());
        let id = |prefix| {
            Identifier::new(format!("{prefix}-{}", hash.as_str()))
                .map_err(|_| GateError::InvalidPlan)
        };
        Ok(Self {
            before_transaction: id("profile-pre-txn")?,
            before_generation: id("profile-pre-gen")?,
            after_transaction: id("profile-post-txn")?,
            after_generation: id("profile-post-gen")?,
        })
    }
}
#[allow(unsafe_code)] // Fixed libc UTC conversion; safety arguments at each call.
fn current_utc() -> std::result::Result<String, GateError> {
    let seconds: libc::time_t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| GateError::PreflightFailed)?
        .as_secs()
        .try_into()
        .map_err(|_| GateError::PreflightFailed)?;
    let mut output = MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers reference live correctly sized storage. gmtime_r
    // initializes output on success, and its return value is checked first.
    if unsafe { libc::gmtime_r(&seconds, output.as_mut_ptr()) }.is_null() {
        return Err(GateError::PreflightFailed);
    }
    // SAFETY: successful gmtime_r initialized output above.
    let time = unsafe { output.assume_init() };
    let year = time.tm_year.checked_add(1900).ok_or(GateError::PreflightFailed)?;
    if !(1970..=9999).contains(&year) {
        return Err(GateError::PreflightFailed);
    }
    Ok(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        time.tm_mon + 1,
        time.tm_mday,
        time.tm_hour,
        time.tm_min,
        time.tm_sec
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn space_estimate_rejects_shortage_unknown_block_size_and_overflow() {
        assert_eq!(check_space(9, 4096, 8192, 8), Err(GateError::InsufficientSpace));
        assert_eq!(check_space(10, 4096, 8192, 8), Ok(()));
        assert_eq!(check_space(100, 0, 8192, 8), Err(GateError::PreflightFailed));
        assert_eq!(check_space(u64::MAX, 4096, 8192, 8), Err(GateError::PreflightFailed));
        assert_eq!(check_space(100, 4096, u64::MAX, 8), Err(GateError::PreflightFailed));
    }
    #[test]
    fn coordinator_assigns_distinct_bounded_identities_and_valid_current_utc() {
        let ids = CaptureIds::for_operation(&OperationId::new("X".repeat(96)).unwrap()).unwrap();
        let distinct = std::collections::BTreeSet::from([
            ids.before_transaction,
            ids.before_generation,
            ids.after_transaction,
            ids.after_generation,
        ]);
        assert_eq!(distinct.len(), 4);
        let time = current_utc().unwrap();
        RecoveryManifest::new(
            Identifier::new("txn").unwrap(),
            Identifier::new("gen").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "1",
                "11111111-1111-1111-1111-111111111111",
            )
            .unwrap(),
            OperationReason::ProfileApply,
            time,
        )
        .unwrap();
    }
}
