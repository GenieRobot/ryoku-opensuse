//! Authenticated native media provider composition with linked recovery.
use super::*;
use crate::{
    ManagedActionKind, OperationId, OperationPhase, PeerIdentity, ServiceError,
    account_state::AccountStateRoot,
    authorize_helper_operator,
    helper_protocol::prepare_operator_account,
    mutation_gate::{GateError, MutationStage, PlannedChange, VerifiedLinkedGeneration},
    native_mutation::{
        NativeMutationProvider, NativeMutationRequest, ProtectedMutationHistory,
        review_operator_plan, run_operator_mutation,
    },
};
use ryoku_recovery::{
    OperationReason, PackageRevision,
    capture_session::NativeCaptureSession,
    installed_capture::InstalledInventory,
    installed_profiles::VerifiedProfileBundle,
    ryogami_media_protocol::MediaIntent,
    wallpaper_protocol::{ActorCall, ActorIdentity},
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct CompletedMedia {
    pub operation_id: OperationId,
    pub reply: Vec<u8>,
}
fn key(intent: &MediaIntent) -> &'static str {
    match intent {
        MediaIntent::Delete { .. } => "media-delete",
        MediaIntent::Restore { .. } => "media-restore",
    }
}
fn gate(error: GateError) -> ServiceError {
    match error {
        GateError::PlanChanged => ServiceError::StaleRequest,
        GateError::RecoveryRequired
        | GateError::CheckpointUnverified
        | GateError::PostChangeFailed => ServiceError::RecoveryUnavailable,
        _ => ServiceError::Failed,
    }
}
pub fn change(
    peer: &PeerIdentity,
    operator_gid: u32,
    intent: MediaIntent,
    rpc_id: i64,
) -> std::result::Result<CompletedMedia, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    if rustix::process::geteuid().as_raw() != 0 || rpc_id < 0 || intent.validate().is_err() {
        return Err(ServiceError::Unauthorized);
    }
    let account = prepare_operator_account(peer, operator_gid)?;
    let home = account.capture_home().map_err(|_| ServiceError::RecoveryUnavailable)?;
    InstalledInventory::for_home(&home).map_err(|_| ServiceError::RecoveryUnavailable)?;
    let principal =
        account.capture_account().map_err(|_| ServiceError::RecoveryUnavailable)?.principal;
    let packages =
        crate::wallpaper::installed_packages().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let (bundle, selected_timestamp) = crate::wallpaper::current_bundle(&principal, &packages)?;
    let created_utc = crate::wallpaper::utc_now().map_err(|_| ServiceError::RecoveryUnavailable)?;
    if created_utc <= selected_timestamp {
        return Err(ServiceError::Busy);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    let operation_id = OperationId::new(format!(
        "media-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
    .map_err(|_| ServiceError::Failed)?;
    let token = Sha256::new(digest(operation_id.as_str().as_bytes())).expect("SHA-256");
    let kind = match intent {
        MediaIntent::Delete { .. } => ManagedActionKind::DeleteMedia,
        MediaIntent::Restore { .. } => ManagedActionKind::RestoreMedia,
    };
    let mut provider = MediaProvider {
        account,
        peer,
        operator_gid,
        intent,
        rpc_id,
        operation: operation_id.clone(),
        token,
        bundle: &bundle,
        packages: packages.clone(),
        reviewed: None,
    };
    let planned = review_operator_plan(peer, operator_gid, &mut provider).map_err(gate)?;
    let output = run_operator_mutation(
        peer,
        operator_gid,
        NativeMutationRequest {
            operation_id: operation_id.clone(),
            action: kind,
            reason: OperationReason::ProviderChange,
            planned,
            created_utc,
            packages,
            bundle: &bundle,
        },
        &mut provider,
    )
    .map_err(gate)?;
    let history = ProtectedMutationHistory::open().map_err(gate)?;
    let status = history.journal().get(&operation_id).ok_or(ServiceError::RecoveryUnavailable)?;
    let context = history
        .journal()
        .mutation_context(&operation_id)
        .ok_or(ServiceError::RecoveryUnavailable)?;
    if status.phase != OperationPhase::Succeeded
        || status.error.is_some()
        || context.stage != MutationStage::Complete
        || context.outcome.is_some()
        || context.post_checkpoint.is_none()
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    let result = match &provider.intent {
        MediaIntent::Delete { .. } => {
            serde_json::json!({"ok":true,"trash_receipt":output.receipt.as_ref().ok_or(ServiceError::RecoveryUnavailable)?.encoded()})
        }
        MediaIntent::Restore { .. } => serde_json::json!({"ok":true,"restored":true}),
    };
    let mut reply = serde_json::to_vec(&serde_json::json!({"id":rpc_id,"result":result}))
        .map_err(|_| ServiceError::Failed)?;
    reply.push(b'\n');
    Ok(CompletedMedia { operation_id, reply })
}
struct MediaProvider<'a> {
    account: AccountStateRoot,
    peer: &'a PeerIdentity,
    operator_gid: u32,
    intent: MediaIntent,
    rpc_id: i64,
    operation: OperationId,
    token: Sha256,
    bundle: &'a VerifiedProfileBundle,
    packages: Vec<PackageRevision>,
    reviewed: Option<(PreparedMedia, PlannedChange, ActorIdentity)>,
}
struct MediaOutput {
    reply: Vec<u8>,
    receipt: Option<TrashReceiptIdentity>,
}
fn actor_pictures(actor: &ActorIdentity) -> std::result::Result<Option<String>, GateError> {
    let path = format!("/proc/{}/environ", actor.pid);
    let file = File::open(path).map_err(|_| GateError::PlanChanged)?;
    let mut bytes = Vec::new();
    file.take(128 * 1024 + 1).read_to_end(&mut bytes).map_err(|_| GateError::PlanChanged)?;
    if bytes.len() > 128 * 1024 {
        return Err(GateError::PlanChanged);
    }
    let mut result = None;
    for field in bytes.split(|byte| *byte == 0) {
        if let Some(value) = field.strip_prefix(b"XDG_PICTURES_DIR=") {
            if result.is_some() {
                return Err(GateError::PlanChanged);
            }
            result =
                Some(std::str::from_utf8(value).map_err(|_| GateError::PlanChanged)?.to_owned());
        }
    }
    Ok(result)
}
impl NativeMutationProvider for MediaProvider<'_> {
    type Output = MediaOutput;
    fn asset_scope(&self) -> Option<AssetScope> {
        self.reviewed.as_ref().map(|(prepared, _, _)| prepared.scope.clone())
    }
    fn plan(
        &mut self,
        session: &NativeCaptureSession,
    ) -> std::result::Result<PlannedChange, GateError> {
        self.account.revalidate().map_err(|_| GateError::PlanChanged)?;
        self.bundle.revalidate().map_err(|_| GateError::CheckpointUnverified)?;
        if crate::wallpaper::installed_packages().map_err(|_| GateError::PlanChanged)?
            != self.packages
            || !self.bundle.bundle().matches_packages(&self.packages)
        {
            return Err(GateError::PlanChanged);
        }
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let account = self.account.capture_account().map_err(|_| GateError::PlanChanged)?;
        let actor = session.wallpaper_actor().map_err(|_| GateError::PlanChanged)?;
        let pictures = actor_pictures(&actor)?;
        let prepared = match &self.intent {
            MediaIntent::Delete { key } => {
                PreparedMedia::delete(&home, &account, pictures.as_deref(), key, self.token.clone())
            }
            MediaIntent::Restore { receipt } => {
                let directory =
                    session.coordinator_directory().map_err(|_| GateError::CheckpointUnverified)?;
                let history = crate::native_mutation::open_journal(directory)?;
                let identity =
                    TrashReceiptIdentity::parse(receipt).map_err(|_| GateError::InvalidPlan)?;
                let receipt = VerifiedTrashReceipt::load(directory, &identity, &account, &history)
                    .map_err(|_| GateError::PlanChanged)?;
                PreparedMedia::restore(
                    &home,
                    &account,
                    pictures.as_deref(),
                    &receipt,
                    self.token.clone(),
                )
            }
        }
        .map_err(|_| GateError::PlanChanged)?;
        if session.wallpaper_actor().map_err(|_| GateError::PlanChanged)? != actor {
            return Err(GateError::PlanChanged);
        }
        let fingerprint = digest(
            &serde_json::to_vec(&(
                &self.intent,
                self.rpc_id,
                &actor,
                prepared.fingerprint().map_err(|_| GateError::InvalidPlan)?,
                self.bundle.id(),
                &self.packages,
            ))
            .map_err(|_| GateError::InvalidPlan)?,
        );
        let plan = PlannedChange {
            operation_key: key(&self.intent).into(),
            plan_fingerprint: fingerprint,
            affected_state_fingerprint: "00".repeat(32),
        };
        self.reviewed = Some((prepared, plan.clone(), actor));
        Ok(plan)
    }
    fn authorize(&mut self, plan: &PlannedChange) -> std::result::Result<(), GateError> {
        authorize_helper_operator(self.peer, self.operator_gid)
            .map_err(|_| GateError::PreflightFailed)?;
        if plan.operation_key != key(&self.intent) {
            return Err(GateError::InvalidPlan);
        }
        self.account.revalidate().map_err(|_| GateError::PlanChanged)
    }
    fn apply(
        &mut self,
        proof: &VerifiedLinkedGeneration,
        session: &NativeCaptureSession,
    ) -> std::result::Result<MediaOutput, GateError> {
        let previous = self.reviewed.as_ref().ok_or(GateError::InvalidPlan)?.1.clone();
        let current = self.plan(session)?;
        self.authorize(&current)?;
        if previous != current {
            return Err(GateError::PlanChanged);
        }
        let (prepared, _, actor) = self.reviewed.as_ref().ok_or(GateError::InvalidPlan)?;
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let account = self.account.capture_account().map_err(|_| GateError::PlanChanged)?;
        let frame = prepared.commit.frame(self.rpc_id).map_err(|_| GateError::InvalidPlan)?;
        let receipt = prepared
            .move_files(
                &self.operation,
                proof,
                &home,
                &account,
                session.coordinator_directory().map_err(|_| GateError::CheckpointUnverified)?,
                || true,
            )
            .map_err(|_| GateError::ApplyFailed)?;
        let path = home_path(&home).map_err(|_| GateError::PlanChanged)?;
        let call = ActorCall {
            player_pins: vec![],
            inherit_manual_pause: false,
            account_home_sha256: digest(path.as_os_str().as_encoded_bytes()),
            actor: actor.clone(),
            plan_sha256: digest(
                &serde_json::to_vec(&(current, proof.linked()))
                    .map_err(|_| GateError::InvalidPlan)?,
            ),
            frame_sha256: digest(&frame),
            frame,
        };
        let reply = session.apply_wallpaper_actor(&call).map_err(|_| GateError::ApplyFailed)?;
        Ok(MediaOutput { reply, receipt })
    }
    fn health_check(&mut self, output: &MediaOutput) -> std::result::Result<(), GateError> {
        self.account.revalidate().map_err(|_| GateError::PostChangeFailed)?;
        let home = self.account.capture_home().map_err(|_| GateError::PostChangeFailed)?;
        let account = self.account.capture_account().map_err(|_| GateError::PostChangeFailed)?;
        self.reviewed
            .as_ref()
            .ok_or(GateError::PostChangeFailed)?
            .0
            .verify_catalog(&home, &account, &output.reply, self.rpc_id)
            .map_err(|_| GateError::PostChangeFailed)
    }
    fn cancellation_requested(&self) -> bool {
        false
    }
}
