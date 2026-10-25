//! Root-side wallpaper operation. A client supplies an action, never a plan,
//! checkpoint, bundle choice, command, actor permit or success attestation.
use crate::{
    ManagedActionKind, OperationId, OperationPhase, PeerIdentity, ServiceError,
    account_state::AccountStateRoot,
    authorize_helper_operator,
    helper_protocol::prepare_operator_account,
    mutation_gate::{
        GateError, LinkedGeneration, MutationStage, PlannedChange, VerifiedLinkedGeneration,
    },
    native_mutation::{
        NativeMutationProvider, NativeMutationRequest, ProtectedMutationHistory,
        review_operator_plan, run_operator_mutation,
    },
};
use ryoku_recovery::{
    AccountPrincipal, OperationReason, PackageName, PackageRevision, Revision,
    capture_session::NativeCaptureSession,
    generation::GenerationReader,
    installed_capture::{InstalledInventory, InventoryError},
    installed_profiles::{ProfileBundleStore, VerifiedProfileBundle},
    native_checkpoint::verify_historical_root_checkpoint,
    wallpaper_protocol::{
        self as wire, ActorCall, WallpaperApply, WallpaperAudio, WallpaperChange, WallpaperKind,
        WallpaperPause,
    },
};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Component, Path},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static NEXT_OPERATION: AtomicU64 = AtomicU64::new(0);
const REQUIRED_PACKAGES: &[&str] =
    &["ryoku-desktop", "ryoku-desktop-runtime", "ryoku-profile-defaults"];
const MAX_STATIC_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_VIDEO_SOURCE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

pub struct CompletedWallpaper {
    pub operation_id: OperationId,
    pub reply: Vec<u8>,
}

pub fn apply(
    peer: &PeerIdentity,
    operator_gid: u32,
    action: WallpaperApply,
    rpc_id: i64,
) -> Result<CompletedWallpaper, ServiceError> {
    change(peer, operator_gid, WallpaperChange::Apply(action), rpc_id)
}
pub fn set_audio(
    peer: &PeerIdentity,
    operator_gid: u32,
    action: WallpaperAudio,
    rpc_id: i64,
) -> Result<CompletedWallpaper, ServiceError> {
    change(peer, operator_gid, WallpaperChange::Audio(action.normalized()), rpc_id)
}
pub fn pause(
    peer: &PeerIdentity,
    operator_gid: u32,
    action: WallpaperPause,
    rpc_id: i64,
) -> Result<CompletedWallpaper, ServiceError> {
    change(peer, operator_gid, WallpaperChange::Pause(action.normalized()), rpc_id)
}
pub fn workspace(
    peer: &PeerIdentity,
    operator_gid: u32,
    request: ryoku_recovery::workspace_protocol::WorkspaceRequest,
    rpc_id: i64,
) -> Result<CompletedWallpaper, ServiceError> {
    change(
        peer,
        operator_gid,
        WallpaperChange::Workspace(Box::new(
            ryoku_recovery::workspace_protocol::WorkspaceChange::intent(request),
        )),
        rpc_id,
    )
}
fn change(
    peer: &PeerIdentity,
    operator_gid: u32,
    action: WallpaperChange,
    rpc_id: i64,
) -> Result<CompletedWallpaper, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    if rustix::process::geteuid().as_raw() != 0 || rpc_id < 0 || action.validate().is_err() {
        return Err(ServiceError::Unauthorized);
    }
    let account = prepare_operator_account(peer, operator_gid)?;
    let home = account.capture_home().map_err(|_| ServiceError::RecoveryUnavailable)?;
    if let WallpaperChange::Apply(action) = &action {
        open_source(&home, account.uid(), &action.path, action.kind).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                ServiceError::Malformed("wallpaper_source_capacity".into())
            } else {
                ServiceError::Malformed("wallpaper_source_unavailable".into())
            }
        })?;
    }
    if let Err(error) = InstalledInventory::for_home(&home) {
        return Err(match error {
            ryoku_recovery::generation_capture::GenerationCaptureError::Inventory(
                InventoryError::WallpaperStoreCapacity { .. },
            ) => ServiceError::Malformed("wallpaper_state_capacity".into()),
            ryoku_recovery::generation_capture::GenerationCaptureError::Inventory(
                InventoryError::WallpaperStateLocation,
            ) => ServiceError::Malformed("wallpaper_state_location".into()),
            _ => ServiceError::RecoveryUnavailable,
        });
    }
    let principal =
        account.capture_account().map_err(|_| ServiceError::RecoveryUnavailable)?.principal;
    let packages = installed_packages().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let (bundle, selected_timestamp) = current_bundle(&principal, &packages)?;
    let created_utc = utc_now().map_err(|_| ServiceError::RecoveryUnavailable)?;
    if created_utc <= selected_timestamp {
        return Err(ServiceError::Busy);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    let operation_id = OperationId::new(format!(
        "wallpaper-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        NEXT_OPERATION.fetch_add(1, Ordering::Relaxed)
    ))
    .map_err(|_| ServiceError::Failed)?;
    let action_kind = match &action {
        WallpaperChange::Apply(_) => ManagedActionKind::ApplyWallpaper,
        WallpaperChange::Audio(_) => ManagedActionKind::SetWallpaperAudio,
        WallpaperChange::Pause(_) => ManagedActionKind::PauseWallpaper,
        WallpaperChange::Workspace(action) => match action.request.operation {
            ryoku_recovery::workspace_protocol::WorkspaceOperation::Assign => {
                ManagedActionKind::AssignWorkspace
            }
            ryoku_recovery::workspace_protocol::WorkspaceOperation::Unassign => {
                ManagedActionKind::UnassignWorkspace
            }
        },
    };
    let mut provider = WallpaperProvider {
        account,
        peer,
        operator_gid,
        action,
        rpc_id,
        bundle: &bundle,
        packages: packages.clone(),
        actor_layout_changed: false,
        last_plan: None,
        player_pins: vec![],
    };
    let planned = review_operator_plan(peer, operator_gid, &mut provider).map_err(gate_error)?;
    let result = run_operator_mutation(
        peer,
        operator_gid,
        NativeMutationRequest {
            operation_id: operation_id.clone(),
            action: action_kind,
            reason: OperationReason::ProviderChange,
            planned,
            created_utc,
            packages,
            bundle: &bundle,
        },
        &mut provider,
    );
    let reply = match result {
        Ok(reply) => reply,
        Err(_) if provider.actor_layout_changed => {
            return Err(ServiceError::Malformed("wallpaper_actor_layout_changed".into()));
        }
        Err(error) => return Err(gate_error(error)),
    };
    // run_operator_mutation returns only after durable Complete and independently
    // verified post capture. Reopen the protected record before replying.
    let history = ProtectedMutationHistory::open().map_err(gate_error)?;
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
    Ok(CompletedWallpaper { operation_id, reply })
}

#[derive(Debug, Eq, PartialEq)]
enum ActorReplyError {
    LayoutChanged,
    Failed,
}
fn validate_actor_reply(
    reply: &[u8],
    expected_id: i64,
    action: &WallpaperChange,
) -> Result<(), ActorReplyError> {
    let value: serde_json::Value =
        serde_json::from_slice(reply).map_err(|_| ActorReplyError::Failed)?;
    if value.get("id").and_then(serde_json::Value::as_i64) != Some(expected_id)
        || value.get("error").is_some() == value.get("result").is_some()
    {
        return Err(ActorReplyError::Failed);
    }
    if value.get("error").is_some() {
        return Err(if value["error"]["code"].as_i64() == Some(-32012) {
            ActorReplyError::LayoutChanged
        } else {
            ActorReplyError::Failed
        });
    }
    match action {
        WallpaperChange::Apply(_) if value["result"]["applied"].as_str().is_some() => (),
        WallpaperChange::Pause(action)
            if value["result"]["paused"].as_bool() == Some(action.paused)
                && value["result"]["effective_paused"].as_bool().is_some()
                && (!action.paused || value["result"]["effective_paused"] == true)
                && value["result"]["player_pins"].is_array() => {}
        WallpaperChange::Audio(_) => {
            audio_receipt(&value["result"]).map_err(|_| ActorReplyError::Failed)?;
        }
        WallpaperChange::Workspace(action) => {
            let prepared = action.prepared.as_ref().ok_or(ActorReplyError::Failed)?;
            match action.request.operation {
                ryoku_recovery::workspace_protocol::WorkspaceOperation::Assign
                    if value["result"]["assignment"]["id"] == prepared.target.id
                        && value["result"]["assignment"]["output"] == prepared.target.output => {}
                ryoku_recovery::workspace_protocol::WorkspaceOperation::Unassign
                    if value["result"]["removed"].as_bool()
                        == Some(!prepared.remove_keys.is_empty()) => {}
                _ => return Err(ActorReplyError::Failed),
            }
        }
        _ => return Err(ActorReplyError::Failed),
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioTarget {
    state: String,
    key: String,
}
fn audio_receipt(result: &serde_json::Value) -> Result<Vec<AudioTarget>, GateError> {
    let targets: Vec<AudioTarget> = serde_json::from_value(result["audio_written"].clone())
        .map_err(|_| GateError::PostChangeFailed)?;
    let mut unique = std::collections::BTreeSet::new();
    if targets.is_empty()
        || targets.len() > 32
        || targets.iter().any(|target| {
            !matches!(target.state.as_str(), "outputs" | "workspaces")
                || target.key.is_empty()
                || target.key.len() > 128
                || target.key.chars().any(char::is_control)
                || !unique.insert((&target.state, &target.key))
        })
    {
        return Err(GateError::PostChangeFailed);
    }
    Ok(targets)
}

struct WallpaperProvider<'a> {
    account: AccountStateRoot,
    peer: &'a PeerIdentity,
    operator_gid: u32,
    action: WallpaperChange,
    rpc_id: i64,
    bundle: &'a VerifiedProfileBundle,
    packages: Vec<PackageRevision>,
    actor_layout_changed: bool,
    last_plan: Option<PlannedChange>,
    player_pins: Vec<wire::ActorIdentity>,
}
impl NativeMutationProvider for WallpaperProvider<'_> {
    type Output = Vec<u8>;
    fn plan(&mut self, session: &NativeCaptureSession) -> Result<PlannedChange, GateError> {
        self.account.revalidate().map_err(|_| GateError::PlanChanged)?;
        self.bundle.revalidate().map_err(|_| GateError::CheckpointUnverified)?;
        if installed_packages().map_err(|_| GateError::PlanChanged)? != self.packages
            || !self.bundle.bundle().matches_packages(&self.packages)
        {
            return Err(GateError::PlanChanged);
        }
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        if let WallpaperChange::Workspace(action) = &mut self.action {
            let actor = session.wallpaper_actor().map_err(|_| GateError::PlanChanged)?;
            let (target, active) = crate::workspace_selection::read_target(
                &actor,
                self.account.uid(),
                &action.request.workspace,
            )
            .map_err(|_| GateError::PlanChanged)?;
            let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
                .map_err(|_| GateError::PlanChanged)?;
            let optional = |path, maximum| match read_home(&home, path, maximum) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(GateError::PlanChanged),
            };
            let outputs = optional(".cache/ryogami/outputs.json", 4 * 1024 * 1024)?;
            let workspaces = optional(".cache/ryogami/workspaces.json", 16 * 1024 * 1024)?;
            let settings = optional(".config/ryoku/ryogami.json", 4 * 1024 * 1024)?;
            action.prepared = Some(
                crate::workspace_selection::prepare(
                    &action.request,
                    target,
                    active,
                    crate::workspace_selection::StoredWorkspace {
                        home: &home_path,
                        outputs: outputs.as_deref(),
                        workspaces: workspaces.as_deref(),
                        settings: settings.as_deref(),
                    },
                    || {
                        crate::workspace_selection::default_source(
                            &actor,
                            self.account.uid(),
                            &home,
                            settings.as_deref(),
                        )
                    },
                )
                .map_err(|_| GateError::PlanChanged)?,
            );
        }
        let source = match &self.action {
            WallpaperChange::Apply(action) => Some(
                source_identity(&home, self.account.uid(), &action.path, action.kind)
                    .map_err(|_| GateError::PlanChanged)?,
            ),
            WallpaperChange::Audio(_) | WallpaperChange::Pause(_) => None,
            WallpaperChange::Workspace(action) => action
                .prepared
                .as_ref()
                .ok_or(GateError::InvalidPlan)?
                .wallpaper
                .as_ref()
                .map(|wallpaper| {
                    source_identity(&home, self.account.uid(), &wallpaper.path, wallpaper.kind)
                })
                .transpose()
                .map_err(|_| GateError::PlanChanged)?,
        };
        let actor = session.wallpaper_actor().map_err(|_| GateError::PlanChanged)?;
        self.player_pins = if matches!(self.action, WallpaperChange::Pause(_)) {
            session.wallpaper_players().map_err(|_| GateError::PlanChanged)?
        } else {
            vec![]
        };
        let mut plan = wallpaper_plan(
            &self.action,
            self.rpc_id,
            source.as_ref(),
            &actor,
            self.bundle.id(),
            &self.packages,
        )?;
        if matches!(self.action, WallpaperChange::Pause(_)) {
            plan.plan_fingerprint = wire::digest(
                &serde_json::to_vec(&(&plan.plan_fingerprint, &self.player_pins))
                    .map_err(|_| GateError::InvalidPlan)?,
            );
        }
        self.last_plan = Some(plan.clone());
        Ok(plan)
    }

    fn authorize(&mut self, plan: &PlannedChange) -> Result<(), GateError> {
        authorize_helper_operator(self.peer, self.operator_gid)
            .map_err(|_| GateError::PreflightFailed)?;
        if plan.operation_key != self.action.operation_key() {
            return Err(GateError::InvalidPlan);
        }
        self.account.revalidate().map_err(|_| GateError::PlanChanged)
    }
    fn apply(
        &mut self,
        proof: &VerifiedLinkedGeneration,
        session: &NativeCaptureSession,
    ) -> Result<Vec<u8>, GateError> {
        // NativeIntegration just compared this prior plan with the reviewed
        // one. Do not replace it with a changed live focus/source/state during
        // this final provider-side check while the compositor remains active.
        let previous = self.last_plan.clone().ok_or(GateError::InvalidPlan)?;
        let plan = self.plan(session)?;
        self.authorize(&plan)?;
        let frame = dispatch_frame(&previous, &plan, &self.action, self.rpc_id)?;
        // The digest is built here from the helper's current exact plan and
        // linked proof, not accepted from the public peer.
        let plan_sha256 = wire::digest(
            &serde_json::to_vec(&(plan, proof.linked())).map_err(|_| GateError::InvalidPlan)?,
        );
        // This descriptor comes from AccountStateRoot's authenticated NSS
        // lookup, never the service/daemon HOME environment.
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
            .map_err(|_| GateError::PlanChanged)?;
        let call = ActorCall {
            player_pins: self.player_pins.clone(),
            account_home_sha256: wire::digest(home_path.as_os_str().as_encoded_bytes()),
            actor: session.wallpaper_actor().map_err(|_| GateError::PlanChanged)?,
            plan_sha256,
            frame_sha256: wire::digest(&frame),
            frame,
        };
        let reply = session.apply_wallpaper_actor(&call).map_err(|_| GateError::ApplyFailed)?;
        match validate_actor_reply(&reply, self.rpc_id, &self.action) {
            Ok(()) => (),
            Err(ActorReplyError::LayoutChanged) => {
                self.actor_layout_changed = true;
                return Err(GateError::ApplyFailed);
            }
            Err(ActorReplyError::Failed) => return Err(GateError::ApplyFailed),
        }
        Ok(reply)
    }
    fn health_check(&mut self, reply: &Vec<u8>) -> Result<(), GateError> {
        self.account.revalidate().map_err(|_| GateError::PostChangeFailed)?;
        let home = self.account.capture_home().map_err(|_| GateError::PostChangeFailed)?;
        if let WallpaperChange::Pause(_) = &self.action {
            validate_actor_reply(reply, self.rpc_id, &self.action)
                .map_err(|_| GateError::PostChangeFailed)?;
            let value: serde_json::Value =
                serde_json::from_slice(reply).map_err(|_| GateError::PostChangeFailed)?;
            let players: Vec<wire::ActorIdentity> =
                serde_json::from_value(value["result"]["player_pins"].clone())
                    .map_err(|_| GateError::PostChangeFailed)?;
            if players != self.player_pins {
                return Err(GateError::PostChangeFailed);
            }
            return Ok(());
        }
        if let WallpaperChange::Workspace(action) = &self.action {
            let prepared = action.prepared.as_ref().ok_or(GateError::PostChangeFailed)?;
            let bytes = match read_home(&home, ".cache/ryogami/workspaces.json", 16 * 1024 * 1024) {
                Ok(bytes) => bytes,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && prepared.remove_keys.is_empty()
                        && action.request.operation
                            == ryoku_recovery::workspace_protocol::WorkspaceOperation::Unassign =>
                {
                    return Ok(());
                }
                Err(_) => return Err(GateError::PostChangeFailed),
            };
            let state: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| GateError::PostChangeFailed)?;
            let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
                .map_err(|_| GateError::PostChangeFailed)?;
            return crate::workspace_selection::verify_post(prepared, &state, &home_path)
                .map_err(|_| GateError::PostChangeFailed);
        }
        if let WallpaperChange::Audio(action) = &self.action {
            let reply: serde_json::Value =
                serde_json::from_slice(reply).map_err(|_| GateError::PostChangeFailed)?;
            for target in audio_receipt(&reply["result"])? {
                let relative = match target.state.as_str() {
                    "outputs" => ".cache/ryogami/outputs.json",
                    "workspaces" => ".cache/ryogami/workspaces.json",
                    _ => return Err(GateError::PostChangeFailed),
                };
                let bytes = read_home(&home, relative, 16 * 1024 * 1024)
                    .map_err(|_| GateError::PostChangeFailed)?;
                let state: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|_| GateError::PostChangeFailed)?;
                let entry = if target.state == "outputs" {
                    &state[&target.key]
                } else {
                    &state["assignments"][&target.key]
                };
                if action.mute.is_some_and(|mute| entry["mute"].as_bool() != Some(mute))
                    || action
                        .volume
                        .is_some_and(|volume| entry["volume"].as_u64() != Some(u64::from(volume)))
                {
                    return Err(GateError::PostChangeFailed);
                }
            }
            return Ok(());
        }
        let WallpaperChange::Apply(action) = &self.action else { unreachable!() };
        let outputs = read_home(&home, ".cache/ryogami/outputs.json", 4 * 1024 * 1024)
            .map_err(|_| GateError::PostChangeFailed)?;
        let outputs: serde_json::Value =
            serde_json::from_slice(&outputs).map_err(|_| GateError::PostChangeFailed)?;
        let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
            .map_err(|_| GateError::PostChangeFailed)?;
        let portable = action
            .path
            .strip_prefix(home_path.to_str().ok_or(GateError::PostChangeFailed)?)
            .map(|tail| format!("~{tail}"));
        let targets =
            if action.outputs.is_empty() { vec!["*".to_owned()] } else { action.outputs.clone() };
        for target in targets {
            let state = outputs.get(&target).ok_or(GateError::PostChangeFailed)?;
            let path = state
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or(GateError::PostChangeFailed)?;
            let expected_type = match action.kind {
                WallpaperKind::Static => "static",
                WallpaperKind::Video => "video",
            };
            if (path != action.path && portable.as_deref() != Some(path))
                || state.get("type").and_then(serde_json::Value::as_str) != Some(expected_type)
            {
                return Err(GateError::PostChangeFailed);
            }
        }
        Ok(())
    }
    fn cancellation_requested(&self) -> bool {
        false
    }
}

fn wallpaper_plan(
    action: &WallpaperChange,
    rpc_id: i64,
    source: Option<&SourceIdentity>,
    actor: &wire::ActorIdentity,
    bundle: &ryoku_recovery::Sha256,
    packages: &[PackageRevision],
) -> Result<PlannedChange, GateError> {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "action":action,"rpc_id":rpc_id,"source":source,"actor":actor,
        "bundle":bundle,"packages":packages,
    }))
    .map_err(|_| GateError::InvalidPlan)?;
    Ok(PlannedChange {
        operation_key: action.operation_key().into(),
        plan_fingerprint: wire::digest(&bytes),
        affected_state_fingerprint: "00".repeat(32),
    })
}
fn dispatch_frame(
    previous: &PlannedChange,
    current: &PlannedChange,
    action: &WallpaperChange,
    rpc_id: i64,
) -> Result<Vec<u8>, GateError> {
    if previous != current || current.operation_key != action.operation_key() {
        return Err(GateError::PlanChanged);
    }
    action.frame(rpc_id).map_err(|_| GateError::InvalidPlan)
}

#[derive(Clone)]
struct BundleCandidate {
    transaction: ryoku_recovery::Identifier,
    timestamp: String,
    principal: AccountPrincipal,
    revision: Revision,
    packages: Vec<PackageRevision>,
}
fn choose_bundle<'a>(
    candidates: &'a [BundleCandidate],
    principal: &AccountPrincipal,
    packages: &[PackageRevision],
) -> Result<&'a BundleCandidate, ServiceError> {
    let mut matching =
        candidates.iter().filter(|candidate| &candidate.principal == principal).collect::<Vec<_>>();
    if matching.iter().any(|candidate| !ryoku_recovery::is_utc_timestamp(&candidate.timestamp)) {
        return Err(ServiceError::RecoveryUnavailable);
    }
    matching.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
    let newest = matching.pop().ok_or(ServiceError::RecoveryUnavailable)?;
    if matching.last().is_some_and(|other| other.timestamp == newest.timestamp)
        || !package_association_matches(&newest.packages, packages)
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    Ok(newest)
}
fn current_bundle(
    principal: &AccountPrincipal,
    packages: &[PackageRevision],
) -> Result<(VerifiedProfileBundle, String), ServiceError> {
    let history = ProtectedMutationHistory::open().map_err(gate_error)?;
    let root = File::open("/").map_err(|_| ServiceError::RecoveryUnavailable)?;
    let reader =
        GenerationReader::open_state_mount().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let mut candidates = Vec::new();
    for status in history.journal().statuses() {
        if status.phase != OperationPhase::Succeeded || status.error.is_some() {
            continue;
        }
        let context = history
            .journal()
            .mutation_context(&status.operation_id)
            .ok_or(ServiceError::RecoveryUnavailable)?;
        if context.stage != MutationStage::Complete || context.outcome.is_some() {
            return Err(ServiceError::RecoveryUnavailable);
        }
        let post = context.post_checkpoint.as_ref().ok_or(ServiceError::RecoveryUnavailable)?;
        let linked = reader
            .open_historical_transaction(&root, &post.transaction_id)
            .map_err(|_| ServiceError::RecoveryUnavailable)?;
        if LinkedGeneration::from_verified_generation(linked.generation()).map_err(gate_error)?
            != *post
        {
            return Err(ServiceError::RecoveryUnavailable);
        }
        let manifest = linked.generation().manifest();
        let Some(account) = manifest.account_principal() else {
            return Err(ServiceError::RecoveryUnavailable);
        };
        let revision = manifest
            .profile_revision
            .as_ref()
            .ok_or(ServiceError::RecoveryUnavailable)?
            .profile_revision
            .clone();
        candidates.push(BundleCandidate {
            transaction: manifest.transaction_id.clone(),
            timestamp: manifest.created_utc.clone(),
            principal: account.clone(),
            revision,
            packages: manifest.package_revisions().to_vec(),
        });
    }
    let selected = choose_bundle(&candidates, principal, packages)?;
    let selected_linked = reader
        .open_historical_transaction(&root, &selected.transaction)
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    verify_historical_root_checkpoint(&root, selected_linked.snapshot().receipt())
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    let bundle = ProfileBundleStore::open_state_mount()
        .and_then(|store| store.load_revision(&selected.revision))
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    if !bundle.bundle().matches_packages(packages) {
        return Err(ServiceError::RecoveryUnavailable);
    }
    bundle.revalidate().map_err(|_| ServiceError::RecoveryUnavailable)?;
    Ok((bundle, selected.timestamp.clone()))
}
fn package_association_matches(saved: &[PackageRevision], installed: &[PackageRevision]) -> bool {
    installed.iter().all(|current| {
        saved.iter().find(|old| old.package_name == current.package_name).is_some_and(|old| {
            old.before_nevra == current.before_nevra
                && old.after_nevra.is_none()
                && old.upstream_revision == current.upstream_revision
        })
    })
}

#[derive(Serialize)]
struct SourceIdentity {
    dev: u64,
    ino: u64,
    size: u64,
    changed: (i64, i64),
    sha256: String,
}
fn validate_source_size(kind: WallpaperKind, size: u64) -> std::io::Result<()> {
    let maximum = match kind {
        WallpaperKind::Static => MAX_STATIC_SOURCE_BYTES,
        WallpaperKind::Video => MAX_VIDEO_SOURCE_BYTES,
    };
    if size == 0 || size > maximum {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "wallpaper source exceeds the verified static/video input capacity",
        ));
    }
    Ok(())
}
fn open_source(home: &File, uid: u32, source: &str, kind: WallpaperKind) -> std::io::Result<File> {
    let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))?;
    let share = File::open("/usr/share")?;
    let (root, relative, owner) = if let Ok(relative) = Path::new(source).strip_prefix(&home_path) {
        (home, relative, uid)
    } else {
        (&share, Path::new(source).strip_prefix("/usr/share").map_err(std::io::Error::other)?, 0)
    };
    if relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(std::io::Error::other("wallpaper source is not a canonical account path"));
    }
    let file = File::from(rustix::fs::openat2(
        root,
        relative,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS
            | rustix::fs::ResolveFlags::NO_XDEV,
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o400 == 0
        || (owner == 0 && metadata.mode() & 0o004 == 0)
    {
        return Err(std::io::Error::other("wallpaper source is not a stable readable file"));
    }
    validate_source_size(kind, metadata.len())?;
    Ok(file)
}
fn source_identity(
    home: &File,
    uid: u32,
    source: &str,
    kind: WallpaperKind,
) -> std::io::Result<SourceIdentity> {
    let mut file = open_source(home, uid, source, kind)?;
    let metadata = file.metadata()?;
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = [0; 64 * 1024];
    loop {
        let count = file.read(&mut bytes)?;
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "wallpaper source hashing deadline exceeded",
            ));
        }
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    let current = file.metadata()?;
    if (current.dev(), current.ino(), current.size(), current.ctime(), current.ctime_nsec())
        != (
            metadata.dev(),
            metadata.ino(),
            metadata.size(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    {
        return Err(std::io::Error::other("wallpaper source changed"));
    }
    Ok(SourceIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
        size: metadata.size(),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
        sha256: format!("{:x}", hash.finalize()),
    })
}
fn read_home(home: &File, path: &str, maximum: u64) -> std::io::Result<Vec<u8>> {
    let file = File::from(rustix::fs::openat2(
        home,
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS
            | rustix::fs::ResolveFlags::NO_XDEV,
    )?);
    if !file.metadata()?.is_file() || file.metadata()?.len() > maximum {
        return Err(std::io::Error::other("wallpaper state exceeds bound"));
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(std::io::Error::other("wallpaper state exceeds bound"));
    }
    Ok(bytes)
}
fn installed_packages() -> Result<Vec<PackageRevision>, String> {
    REQUIRED_PACKAGES
        .iter()
        .map(|name| {
            let output = Command::new("/usr/bin/timeout")
                .env_clear()
                .env("LC_ALL", "C")
                .args([
                    "--signal=TERM",
                    "--kill-after=1s",
                    "5s",
                    "/usr/bin/rpm",
                    "-q",
                    "--qf",
                    "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n",
                    name,
                ])
                .stdin(Stdio::null())
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() || output.stdout.len() > 1024 {
                return Err("required wallpaper package unavailable".into());
            }
            let text = std::str::from_utf8(&output.stdout).map_err(|error| error.to_string())?;
            if text.lines().count() != 1 || !text.starts_with(&format!("{name}-")) {
                return Err("ambiguous wallpaper package inventory".into());
            }
            Ok(PackageRevision {
                package_name: PackageName::new(*name).map_err(|error| format!("{error:?}"))?,
                before_nevra: Some(
                    Revision::new(text.trim_end()).map_err(|error| format!("{error:?}"))?,
                ),
                after_nevra: None,
                upstream_revision: None,
            })
        })
        .collect()
}
fn utc_now() -> Result<String, String> {
    let output = Command::new("/usr/bin/timeout")
        .env_clear()
        .args([
            "--signal=TERM",
            "--kill-after=1s",
            "3s",
            "/usr/bin/date",
            "-u",
            "+%Y-%m-%dT%H:%M:%SZ",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() || output.stdout.len() > 64 {
        return Err("UTC timestamp unavailable".into());
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|error| error.to_string())?
        .trim_end()
        .to_owned();
    if !ryoku_recovery::is_utc_timestamp(&value) {
        return Err("UTC timestamp is not canonical".into());
    }
    Ok(value)
}
fn gate_error(error: GateError) -> ServiceError {
    match error {
        GateError::RecoveryRequired
        | GateError::CheckpointUnverified
        | GateError::PostChangeFailed => ServiceError::RecoveryUnavailable,
        GateError::InsufficientSpace => ServiceError::Malformed("wallpaper_state_capacity".into()),
        GateError::PlanChanged => ServiceError::StaleRequest,
        _ => ServiceError::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_dispatch_refuses_changed_focus_state_or_source_before_emitting_a_frame() {
        use ryoku_recovery::workspace_protocol::{PreparedWorkspace, WorkspaceChange};
        let prepared: PreparedWorkspace = serde_json::from_value(serde_json::json!({
            "request":{"operation":"assign","workspace":{"id":"","name":"","output":""}},
            "provider":"hyprland","target":{"id":"1","name":"1","output":"DP-1"},
            "active":true,"wallpaper":{"type":"static","path":"/home/fixture/a.png","mute":true,"volume":40},
            "workspace_sha256":"11".repeat(32),"outputs_sha256":"22".repeat(32),"remove_keys":[],
        })).unwrap();
        let action = |prepared: PreparedWorkspace| {
            WallpaperChange::Workspace(Box::new(WorkspaceChange {
                request: prepared.request.clone(),
                prepared: Some(prepared),
            }))
        };
        let original = action(prepared.clone());
        let mut source =
            SourceIdentity { dev: 1, ino: 2, size: 3, changed: (4, 5), sha256: "33".repeat(32) };
        let actor = wire::ActorIdentity { pid: 42, start_time: 43 };
        let bundle = ryoku_recovery::Sha256::new("44".repeat(32)).unwrap();
        let make_plan = |action: &WallpaperChange, source: &SourceIdentity| {
            wallpaper_plan(action, 9, Some(source), &actor, &bundle, &[]).unwrap()
        };
        let reviewed = make_plan(&original, &source);
        let frame =
            dispatch_frame(&reviewed, &make_plan(&original, &source), &original, 9).unwrap();
        assert_eq!(wire::parse_change(&frame).unwrap(), (9, original.clone()));
        let mut changed_focus = prepared.clone();
        changed_focus.target.id = "2".into();
        changed_focus.target.name = "2".into();
        let mut changed_state = prepared.clone();
        changed_state.workspace_sha256 = Some("55".repeat(32));
        let mut changed_outputs = prepared;
        changed_outputs.outputs_sha256 = Some("66".repeat(32));
        for changed in [changed_focus, changed_state, changed_outputs] {
            let current_action = action(changed);
            let result =
                dispatch_frame(&reviewed, &make_plan(&current_action, &source), &current_action, 9);
            assert!(matches!(result, Err(GateError::PlanChanged)));
        }
        source.sha256 = "77".repeat(32);
        assert!(matches!(
            dispatch_frame(&reviewed, &make_plan(&original, &source), &original, 9),
            Err(GateError::PlanChanged)
        ));
    }

    #[test]
    fn audio_receipts_are_bounded_unique_and_not_apply_success() {
        let action = WallpaperChange::Audio(WallpaperAudio {
            outputs: vec![],
            mute: Some(true),
            volume: None,
        });
        let reply =
            |result| serde_json::to_vec(&serde_json::json!({"id":4,"result":result})).unwrap();
        assert_eq!(
            validate_actor_reply(
                &reply(serde_json::json!({"audio_written":[{"state":"outputs","key":"*"}]})),
                4,
                &action
            ),
            Ok(())
        );
        for result in [
            serde_json::json!({"applied":"a.png"}),
            serde_json::json!({"audio_written":[]}),
            serde_json::json!({"audio_written":[{"state":"arbitrary","key":"*"}]}),
            serde_json::json!({"audio_written":[{"state":"outputs","key":"*"},{"state":"outputs","key":"*"}]}),
        ] {
            assert!(validate_actor_reply(&reply(result), 4, &action).is_err());
        }
    }

    fn principal(name: &str) -> AccountPrincipal {
        AccountPrincipal::from_account(1000, 1000, name, "/home/fixture").unwrap()
    }
    fn package(version: &str) -> PackageRevision {
        PackageRevision {
            package_name: PackageName::new("ryoku-profile-defaults").unwrap(),
            before_nevra: Some(Revision::new(version).unwrap()),
            after_nevra: None,
            upstream_revision: None,
        }
    }
    fn candidate(time: &str, who: &str, version: &str) -> BundleCandidate {
        BundleCandidate {
            transaction: ryoku_recovery::Identifier::new("fixture-post").unwrap(),
            timestamp: time.into(),
            principal: principal(who),
            revision: Revision::new("bundle-sha256-fixture").unwrap(),
            packages: vec![package(version)],
        }
    }
    #[test]
    fn actor_layout_refusal_is_specific_but_never_accepted_as_success_or_wrong_reply() {
        let action = WallpaperChange::Audio(WallpaperAudio {
            outputs: vec![],
            mute: Some(true),
            volume: None,
        });
        let frame = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
        let error = serde_json::json!({"id":9,"error":{"code":-32012,"message":"loaded state roots differ"}});
        assert_eq!(
            validate_actor_reply(&frame(error.clone()), 9, &action),
            Err(ActorReplyError::LayoutChanged)
        );
        assert_eq!(
            validate_actor_reply(&frame(error.clone()), 10, &action),
            Err(ActorReplyError::Failed)
        );
        let mut ambiguous = error;
        ambiguous["result"] = serde_json::json!({"applied":"a.png"});
        assert_eq!(
            validate_actor_reply(&frame(ambiguous), 9, &action),
            Err(ActorReplyError::Failed)
        );
        assert_eq!(
            validate_actor_reply(
                &frame(serde_json::json!({"id":9,"error":{"code":-32001}})),
                9,
                &action
            ),
            Err(ActorReplyError::Failed)
        );
        assert_eq!(
            validate_actor_reply(
                &frame(serde_json::json!({"id":9,"result":{"applied":"a.png"}})),
                9,
                &WallpaperChange::Apply(WallpaperApply {
                    kind: WallpaperKind::Static,
                    path: "/home/fixture/a.png".into(),
                    outputs: vec![],
                    mute: Default::default(),
                    volume: Default::default()
                })
            ),
            Ok(())
        );
    }

    #[test]
    fn bundle_selection_is_account_bound_orders_timestamps_and_rejects_ties_or_stale_packages() {
        let current = vec![package("v2")];
        let candidates = vec![
            candidate("2026-10-24T10:00:00Z", "fixture", "v1"),
            candidate("2026-10-24T10:01:00Z", "other", "v2"),
            candidate("2026-10-24T10:00:01Z", "fixture", "v2"),
        ];
        assert_eq!(
            choose_bundle(&candidates, &principal("fixture"), &current).unwrap().timestamp,
            "2026-10-24T10:00:01Z"
        );
        let mut tied = candidates.clone();
        tied.push(candidate("2026-10-24T10:00:01Z", "fixture", "v2"));
        assert!(choose_bundle(&tied, &principal("fixture"), &current).is_err());
        assert!(choose_bundle(&candidates, &principal("unknown"), &current).is_err());
        assert!(choose_bundle(&candidates, &principal("fixture"), &[package("v3")]).is_err());
        let invalid = vec![candidate("2026-10-24T25:00:00Z", "fixture", "v2")];
        assert!(choose_bundle(&invalid, &principal("fixture"), &current).is_err());
        assert!(choose_bundle(&[], &principal("fixture"), &current).is_err());
    }
    #[test]
    fn static_and_video_input_capacity_is_checked_before_reading() {
        for (kind, maximum) in [
            (WallpaperKind::Static, MAX_STATIC_SOURCE_BYTES),
            (WallpaperKind::Video, MAX_VIDEO_SOURCE_BYTES),
        ] {
            assert!(validate_source_size(kind, maximum).is_ok());
            assert!(validate_source_size(kind, maximum + 1).is_err());
            assert!(validate_source_size(kind, 0).is_err());
        }
        assert!(validate_source_size(WallpaperKind::Video, MAX_STATIC_SOURCE_BYTES + 1).is_ok());
        assert!(validate_source_size(WallpaperKind::Static, MAX_STATIC_SOURCE_BYTES + 1).is_err());
    }
}
