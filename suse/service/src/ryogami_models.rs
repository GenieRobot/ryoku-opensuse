//! Authenticated #175 model-reference mutations through the existing native
//! actor, exact stage enrollment and linked pre/post recovery runner.
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
    ryogami_model_references::{self, ReferenceError},
};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use ryoku_recovery::{
    OperationReason, PackageRevision, Sha256,
    capture_session::NativeCaptureSession,
    installed_capture::InstalledInventory,
    installed_profiles::VerifiedProfileBundle,
    ryogami_assets::AssetScope,
    ryogami_models_protocol::{ModelReferenceCommit, ModelReferenceIntent},
    wallpaper_protocol::{ActorCall, ActorIdentity, digest},
};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const SETTINGS: &str = ".config/ryoku/ryogami.json";
const SETTINGS_LIMIT: u64 = 4 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

pub struct CompletedModels {
    pub operation_id: OperationId,
    pub reply: Vec<u8>,
}

pub fn change(
    peer: &PeerIdentity,
    operator_gid: u32,
    intent: ModelReferenceIntent,
    rpc_id: i64,
) -> Result<CompletedModels, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    if rustix::process::geteuid().as_raw() != 0 || rpc_id < 0 || intent.validate().is_err() {
        return Err(ServiceError::Unauthorized);
    }
    let account = prepare_operator_account(peer, operator_gid)?;
    let home = account.capture_home().map_err(|_| ServiceError::RecoveryUnavailable)?;
    // Preserve #25's existing inventory checks, including cache relocation.
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
        "model-reference-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
    .map_err(|_| ServiceError::Failed)?;
    let token = Sha256::new(digest(operation_id.as_str().as_bytes())).expect("SHA-256");
    // Fail unsupported selection-changing callbacks before native checkpoints.
    prepare_owned(&home, account.uid(), &intent, token.clone()).map_err(input_error)?;
    let kind = match &intent {
        ModelReferenceIntent::Add { .. } => ManagedActionKind::AddModelReference,
        ModelReferenceIntent::Forget { .. } => ManagedActionKind::ForgetModelReference,
    };
    let mut provider = ModelProvider {
        account,
        peer,
        operator_gid,
        intent,
        rpc_id,
        token,
        bundle: &bundle,
        packages: packages.clone(),
        reviewed: None,
    };
    let planned = review_operator_plan(peer, operator_gid, &mut provider).map_err(gate_error)?;
    let _actor_reply = run_operator_mutation(
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
    .map_err(gate_error)?;
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
    let prepared = &provider.reviewed.as_ref().ok_or(ServiceError::RecoveryUnavailable)?.0;
    let reply = prepared.public_reply(&provider.intent, rpc_id).map_err(gate_error)?;
    Ok(CompletedModels { operation_id, reply })
}
fn input_error(error: ReferenceError) -> ServiceError {
    let code = match error {
        ReferenceError::SelectionCallbackRequired => "model_selection_callback_required",
        ReferenceError::Capacity => "model_reference_capacity",
        ReferenceError::MissingReference => "model_reference_missing",
        _ => "model_reference_configuration",
    };
    ServiceError::Malformed(code.into())
}
fn gate_error(error: GateError) -> ServiceError {
    match error {
        GateError::PlanChanged => ServiceError::StaleRequest,
        GateError::RecoveryRequired
        | GateError::CheckpointUnverified
        | GateError::PostChangeFailed => ServiceError::RecoveryUnavailable,
        GateError::InsufficientSpace => ServiceError::Malformed("model_reference_capacity".into()),
        _ => ServiceError::Failed,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    changed: (i64, u64),
    sha256: String,
}
struct OwnedFile {
    bytes: Vec<u8>,
    identity: FileIdentity,
}
fn read_owned(
    root: &File,
    path: &str,
    uid: u32,
    maximum: u64,
    cross_mounts: bool,
) -> Result<OwnedFile, ReferenceError> {
    let mut resolve =
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
    if !cross_mounts {
        resolve |= ResolveFlags::NO_XDEV;
    }
    let file = File::from(
        fs::openat2(
            root,
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            resolve,
        )
        .map_err(|_| ReferenceError::Configuration)?,
    );
    let before = fs::fstat(&file).map_err(|_| ReferenceError::Configuration)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_uid != uid
        || before.st_nlink != 1
        || before.st_mode & 0o400 == 0
        || before.st_mode & 0o6022 != 0
        || before.st_size < 0
    {
        return Err(ReferenceError::Configuration);
    }
    if before.st_size as u64 > maximum {
        return Err(ReferenceError::Capacity);
    }
    let mut bytes = Vec::new();
    (&file).take(maximum + 1).read_to_end(&mut bytes).map_err(|_| ReferenceError::Configuration)?;
    let after = fs::fstat(&file).map_err(|_| ReferenceError::Configuration)?;
    if bytes.len() as u64 > maximum {
        return Err(ReferenceError::Capacity);
    }
    if before.st_dev != after.st_dev
        || before.st_ino != after.st_ino
        || before.st_size != after.st_size
        || before.st_mode != after.st_mode
        || before.st_uid != after.st_uid
        || before.st_gid != after.st_gid
        || before.st_nlink != after.st_nlink
        || before.st_ctime != after.st_ctime
        || before.st_ctime_nsec != after.st_ctime_nsec
        || before.st_size as usize != bytes.len()
    {
        return Err(ReferenceError::Configuration);
    }
    Ok(OwnedFile {
        identity: FileIdentity {
            device: before.st_dev,
            inode: before.st_ino,
            length: bytes.len() as u64,
            mode: before.st_mode,
            uid: before.st_uid,
            gid: before.st_gid,
            changed: (before.st_ctime, before.st_ctime_nsec),
            sha256: digest(&bytes),
        },
        bytes,
    })
}

#[derive(Clone)]
struct PreparedReference {
    commit: ModelReferenceCommit,
    settings_identity: FileIdentity,
    source_identity: Option<FileIdentity>,
    // Retained only in helper memory to verify unrelated settings and secrets.
    original: serde_json::Value,
}
fn prepare_owned(
    home: &File,
    uid: u32,
    intent: &ModelReferenceIntent,
    token: Sha256,
) -> Result<PreparedReference, ReferenceError> {
    let settings = read_owned(home, SETTINGS, uid, SETTINGS_LIMIT, false)?;
    let (name, source_identity) = match intent {
        ModelReferenceIntent::Add { path: Some(path) } => {
            let fallback = Path::new(path)
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty() && *s != ".")
                .unwrap_or("model")
                .to_owned();
            let source = if let Some(relative) = path.strip_prefix("~/") {
                read_owned(home, relative, uid, 1024 * 1024, false).ok()
            } else if let Some(relative) = path.strip_prefix('/') {
                File::open("/")
                    .ok()
                    .and_then(|root| read_owned(&root, relative, uid, 1024 * 1024, true).ok())
            } else {
                None
            };
            let name = source
                .as_ref()
                .and_then(|source| serde_json::from_slice::<serde_json::Value>(&source.bytes).ok())
                .and_then(|value| {
                    value
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or(fallback);
            (Some(name), source.map(|source| source.identity))
        }
        _ => (None, None),
    };
    let commit =
        ryogami_model_references::prepare(Some(&settings.bytes), intent, name.as_deref(), token)?;
    let stage = fs::statat(home, commit.staging_name().as_str(), fs::AtFlags::SYMLINK_NOFOLLOW);
    if !matches!(stage, Err(rustix::io::Errno::NOENT)) {
        return Err(ReferenceError::Configuration);
    }
    let original =
        serde_json::from_slice(&settings.bytes).map_err(|_| ReferenceError::Configuration)?;
    Ok(PreparedReference {
        commit,
        settings_identity: settings.identity,
        source_identity,
        original,
    })
}
impl PreparedReference {
    fn plan(
        &self,
        intent: &ModelReferenceIntent,
        rpc_id: i64,
        actor: &ActorIdentity,
        bundle: &Sha256,
        packages: &[PackageRevision],
    ) -> Result<PlannedChange, GateError> {
        let bytes = serde_json::to_vec(&(
            intent,
            rpc_id,
            &self.commit,
            &self.settings_identity,
            &self.source_identity,
            actor,
            bundle,
            packages,
        ))
        .map_err(|_| GateError::InvalidPlan)?;
        Ok(PlannedChange {
            operation_key: operation_key(intent).into(),
            plan_fingerprint: digest(&bytes),
            affected_state_fingerprint: "00".repeat(32),
        })
    }
    fn verify(&self, home: &File, uid: u32, reply: &[u8], id: i64) -> Result<(), GateError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reply {
            id: i64,
            result: ResultBody,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ResultBody {
            committed: bool,
            models: usize,
        }
        let result: Reply =
            serde_json::from_slice(reply).map_err(|_| GateError::PostChangeFailed)?;
        if result.id != id
            || !result.result.committed
            || result.result.models != self.commit.models.len()
        {
            return Err(GateError::PostChangeFailed);
        }
        let settings = read_owned(home, SETTINGS, uid, SETTINGS_LIMIT, false)
            .map_err(|_| GateError::PostChangeFailed)?;
        let actual: serde_json::Value =
            serde_json::from_slice(&settings.bytes).map_err(|_| GateError::PostChangeFailed)?;
        let mut expected = self.original.clone();
        if expected.get("semantic").is_none_or(serde_json::Value::is_null) {
            expected["semantic"] = serde_json::json!({});
        }
        expected["semantic"]["models"] =
            serde_json::to_value(&self.commit.models).map_err(|_| GateError::PostChangeFailed)?;
        // Go emits optional zero-valued metadata with its canonical omissions.
        let mut normalized = actual.clone();
        let models: Vec<ryoku_recovery::ryogami_models_protocol::ModelReference> =
            serde_json::from_value(actual["semantic"]["models"].clone())
                .map_err(|_| GateError::PostChangeFailed)?;
        normalized["semantic"]["models"] =
            serde_json::to_value(models).map_err(|_| GateError::PostChangeFailed)?;
        if normalized != expected
            || settings.identity.mode != self.settings_identity.mode
            || settings.identity.uid != self.settings_identity.uid
            || settings.identity.gid != self.settings_identity.gid
            || !matches!(
                fs::statat(
                    home,
                    self.commit.staging_name().as_str(),
                    fs::AtFlags::SYMLINK_NOFOLLOW
                ),
                Err(rustix::io::Errno::NOENT)
            )
        {
            return Err(GateError::PostChangeFailed);
        }
        Ok(())
    }
    fn public_reply(&self, intent: &ModelReferenceIntent, id: i64) -> Result<Vec<u8>, GateError> {
        let result = match intent {
            ModelReferenceIntent::Add { .. } => {
                serde_json::json!({"index": self.commit.models.len()-1})
            }
            ModelReferenceIntent::Forget { .. } => serde_json::json!({"removed":true}),
        };
        let mut bytes = serde_json::to_vec(&serde_json::json!({"id":id,"result":result}))
            .map_err(|_| GateError::PostChangeFailed)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}
fn operation_key(intent: &ModelReferenceIntent) -> &'static str {
    match intent {
        ModelReferenceIntent::Add { .. } => "ryogami.model-reference.add",
        ModelReferenceIntent::Forget { .. } => "ryogami.model-reference.forget",
    }
}
struct ModelProvider<'a> {
    account: AccountStateRoot,
    peer: &'a PeerIdentity,
    operator_gid: u32,
    intent: ModelReferenceIntent,
    rpc_id: i64,
    token: Sha256,
    bundle: &'a VerifiedProfileBundle,
    packages: Vec<PackageRevision>,
    reviewed: Option<(PreparedReference, PlannedChange)>,
}
impl NativeMutationProvider for ModelProvider<'_> {
    type Output = Vec<u8>;
    fn asset_scope(&self) -> Option<AssetScope> {
        let commit = ModelReferenceCommit {
            expected_config: None,
            staging_token: self.token.clone(),
            models: vec![],
            expected_selected: String::new(),
        };
        Some(
            AssetScope::new(
                vec![commit.staging_asset().expect("derived stage")],
                SETTINGS_LIMIT,
                SETTINGS_LIMIT,
            )
            .expect("fixed scope"),
        )
    }
    fn plan(&mut self, session: &NativeCaptureSession) -> Result<PlannedChange, GateError> {
        self.account.revalidate().map_err(|_| GateError::PlanChanged)?;
        self.bundle.revalidate().map_err(|_| GateError::CheckpointUnverified)?;
        if crate::wallpaper::installed_packages().map_err(|_| GateError::PlanChanged)?
            != self.packages
            || !self.bundle.bundle().matches_packages(&self.packages)
        {
            return Err(GateError::PlanChanged);
        }
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let prepared = prepare_owned(&home, self.account.uid(), &self.intent, self.token.clone())
            .map_err(|_| GateError::PlanChanged)?;
        let actor = session.wallpaper_actor().map_err(|_| GateError::PlanChanged)?;
        let plan =
            prepared.plan(&self.intent, self.rpc_id, &actor, self.bundle.id(), &self.packages)?;
        self.reviewed = Some((prepared, plan.clone()));
        Ok(plan)
    }
    fn authorize(&mut self, plan: &PlannedChange) -> Result<(), GateError> {
        authorize_helper_operator(self.peer, self.operator_gid)
            .map_err(|_| GateError::PreflightFailed)?;
        if plan.operation_key != operation_key(&self.intent) {
            return Err(GateError::InvalidPlan);
        }
        self.account.revalidate().map_err(|_| GateError::PlanChanged)
    }
    fn apply(
        &mut self,
        proof: &VerifiedLinkedGeneration,
        session: &NativeCaptureSession,
    ) -> Result<Vec<u8>, GateError> {
        let previous = self.reviewed.as_ref().ok_or(GateError::InvalidPlan)?.1.clone();
        let current = self.plan(session)?;
        self.authorize(&current)?;
        if previous != current {
            return Err(GateError::PlanChanged);
        }
        let prepared = &self.reviewed.as_ref().ok_or(GateError::InvalidPlan)?.0;
        let frame = prepared.commit.frame(self.rpc_id).map_err(|_| GateError::InvalidPlan)?;
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
            .map_err(|_| GateError::PlanChanged)?;
        let call = ActorCall {
            player_pins: vec![],
            inherit_manual_pause: false,
            account_home_sha256: digest(path.as_os_str().as_encoded_bytes()),
            actor: session.wallpaper_actor().map_err(|_| GateError::PlanChanged)?,
            plan_sha256: digest(
                &serde_json::to_vec(&(current, proof.linked()))
                    .map_err(|_| GateError::InvalidPlan)?,
            ),
            frame_sha256: digest(&frame),
            frame,
        };
        session.apply_wallpaper_actor(&call).map_err(|_| GateError::ApplyFailed)
    }
    fn health_check(&mut self, reply: &Vec<u8>) -> Result<(), GateError> {
        self.account.revalidate().map_err(|_| GateError::PostChangeFailed)?;
        let home = self.account.capture_home().map_err(|_| GateError::PostChangeFailed)?;
        self.reviewed.as_ref().ok_or(GateError::PostChangeFailed)?.0.verify(
            &home,
            self.account.uid(),
            reply,
            self.rpc_id,
        )
    }
    fn cancellation_requested(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "ryogami_models_tests.rs"]
mod tests;
