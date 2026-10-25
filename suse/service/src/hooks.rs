//! Operational configured-command slice. Only an index/event and reviewed
//! policy class cross the helper boundary; the helper resolves command bytes.
#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
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
    capture_session::NativeCaptureSession,
    hook_protocol::{HookApprovalRequest, HookCall, HookPolicy, HookReceipt, HookRequest},
    installed_profiles::VerifiedProfileBundle,
    wallpaper_protocol::digest,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalRecord {
    policy: HookPolicy,
    operation_id: OperationId,
}
enum Intent {
    Approve(HookApprovalRequest),
    Run(HookRequest),
}
pub struct CompletedHook {
    pub operation_id: OperationId,
    pub receipt: HookReceipt,
}

pub fn approve(
    peer: &PeerIdentity,
    gid: u32,
    request: HookApprovalRequest,
) -> Result<CompletedHook, ServiceError> {
    if !request.validate() {
        return Err(ServiceError::Malformed("invalid_hook_approval".into()));
    }
    change(peer, gid, Intent::Approve(request))
}
pub fn run(
    peer: &PeerIdentity,
    gid: u32,
    request: HookRequest,
) -> Result<CompletedHook, ServiceError> {
    if !request.validate() {
        return Err(ServiceError::Malformed("invalid_hook_event".into()));
    }
    change(peer, gid, Intent::Run(request))
}
fn change(peer: &PeerIdentity, gid: u32, intent: Intent) -> Result<CompletedHook, ServiceError> {
    authorize_helper_operator(peer, gid)?;
    if rustix::process::geteuid().as_raw() != 0 {
        return Err(ServiceError::Unauthorized);
    }
    let account = prepare_operator_account(peer, gid)?;
    let packages =
        crate::wallpaper::installed_packages().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let principal =
        account.capture_account().map_err(|_| ServiceError::RecoveryUnavailable)?.principal;
    let (bundle, timestamp) = crate::wallpaper::current_bundle(&principal, &packages)?;
    let mut created_utc =
        crate::wallpaper::utc_now().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let timestamp_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while created_utc <= timestamp {
        if std::time::Instant::now() >= timestamp_deadline {
            return Err(ServiceError::Busy);
        }
        // Wait for a fresh operation timestamp before any mutation begins;
        // this never retries a started wallpaper or hook operation.
        std::thread::sleep(std::time::Duration::from_millis(50));
        created_utc = crate::wallpaper::utc_now().map_err(|_| ServiceError::RecoveryUnavailable)?;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| ServiceError::Failed)?;
    let operation_id =
        OperationId::new(format!("hook-{}-{}-{}", peer.uid(), now.as_secs(), now.subsec_nanos()))
            .map_err(|_| ServiceError::Failed)?;
    let action = if matches!(intent, Intent::Approve(_)) {
        ManagedActionKind::AuthorizeConfiguredHook
    } else {
        ManagedActionKind::RunConfiguredHook
    };
    let mut provider = HookProvider {
        account,
        peer,
        gid,
        intent,
        bundle: &bundle,
        operation_id: operation_id.clone(),
        packages: packages.clone(),
        current: None,
        last_plan: None,
        last_refusal: None,
    };
    let planned = review_operator_plan(peer, gid, &mut provider).map_err(hook_error)?;
    let result = run_operator_mutation(
        peer,
        gid,
        NativeMutationRequest {
            operation_id: operation_id.clone(),
            action,
            reason: ryoku_recovery::OperationReason::ProviderChange,
            planned,
            created_utc,
            packages,
            bundle: &bundle,
        },
        &mut provider,
    );
    let receipt = result.map_err(|error| {
        provider.last_refusal.as_ref().map_or_else(
            || hook_error(error),
            |reason| {
                ServiceError::Malformed(format!(
                    "hook_execution_failed: {reason}; inspect linked history before retrying"
                ))
            },
        )
    })?;
    let history = ProtectedMutationHistory::open().map_err(hook_error)?;
    if !completed(history.journal(), &operation_id) {
        return Err(ServiceError::RecoveryUnavailable);
    }
    Ok(CompletedHook { operation_id, receipt })
}
fn hook_error(error: GateError) -> ServiceError {
    match error {
        GateError::InvalidPlan | GateError::PlanChanged | GateError::PreflightFailed => ServiceError::Malformed(
            "hook_policy_unavailable_or_changed: review ryoku-hook authorize; existing captured outputs and exact current command are required".into()),
        _ => ServiceError::RecoveryUnavailable,
    }
}
fn completed(journal: &crate::OperationJournal, operation: &OperationId) -> bool {
    journal
        .get(operation)
        .is_some_and(|status| status.phase == OperationPhase::Succeeded && status.error.is_none())
        && journal.mutation_context(operation).is_some_and(|context| {
            context.stage == MutationStage::Complete
                && context.outcome.is_none()
                && context.post_checkpoint.is_some()
        })
}
struct HookProvider<'a> {
    account: AccountStateRoot,
    peer: &'a PeerIdentity,
    gid: u32,
    intent: Intent,
    bundle: &'a VerifiedProfileBundle,
    operation_id: OperationId,
    packages: Vec<ryoku_recovery::PackageRevision>,
    current: Option<ApprovalRecord>,
    last_plan: Option<PlannedChange>,
    last_refusal: Option<String>,
}
impl HookProvider<'_> {
    fn record_name(&self) -> String {
        let index = match &self.intent {
            Intent::Approve(request) => request.index,
            Intent::Run(request) => request.index,
        };
        format!("hook-policy-{}-{index}.json", self.account.uid())
    }
    fn policy(&self, session: &NativeCaptureSession) -> Result<ApprovalRecord, GateError> {
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let configuration =
            crate::wallpaper::read_home(&home, ".config/ryoku/ryogami.json", 4 * 1024 * 1024)
                .map_err(|_| GateError::InvalidPlan)?;
        let configuration_sha256 = digest(&configuration);
        let (approval, operation_id) = match &self.intent {
            Intent::Approve(request) => (request.clone(), self.operation_id.clone()),
            Intent::Run(_) => {
                let record = read_record(
                    session.coordinator_directory().map_err(|_| GateError::PlanChanged)?,
                    &self.record_name(),
                )?;
                let journal = crate::native_mutation::open_journal(
                    session.coordinator_directory().map_err(|_| GateError::PlanChanged)?,
                )?;
                if !completed(&journal, &record.operation_id) {
                    return Err(GateError::PreflightFailed);
                }
                (record.policy.approval, record.operation_id)
            }
        };
        let (command, kind, restore_enabled) = command(&configuration, approval.index)?;
        if digest(command.as_bytes()) != approval.command_sha256 {
            return Err(GateError::PlanChanged);
        }
        if let Intent::Run(request) = &self.intent {
            if request.command_sha256 != approval.command_sha256 {
                return Err(GateError::PlanChanged);
            }
            if request.restoring && (!restore_enabled || !approval.allow_restore)
                || kind != "all" && kind != request.kind
            {
                return Err(GateError::PreflightFailed);
            }
            validate_event(&home, request)?;
        }
        for output in &approval.outputs {
            let file = File::from(
                rustix::fs::openat2(
                    &home,
                    output.path(),
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::NONBLOCK
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                    rustix::fs::ResolveFlags::BENEATH
                        | rustix::fs::ResolveFlags::NO_SYMLINKS
                        | rustix::fs::ResolveFlags::NO_XDEV,
                )
                .map_err(|_| GateError::InvalidPlan)?,
            );
            let m = file.metadata().map_err(|_| GateError::InvalidPlan)?;
            if !m.is_file()
                || m.uid() != self.account.uid()
                || m.nlink() != 1
                || m.mode() & 0o6022 != 0
                || m.len() > 16 * 1024 * 1024
            {
                return Err(GateError::InvalidPlan);
            }
        }
        let record = ApprovalRecord {
            operation_id,
            policy: HookPolicy {
                account: self
                    .account
                    .capture_account()
                    .map_err(|_| GateError::PlanChanged)?
                    .principal,
                uid: self.account.uid(),
                approval,
                configuration_sha256,
                shell_sha256: ryoku_recovery::hook_shell_digest()
                    .map_err(|_| GateError::InvalidPlan)?,
                command,
                kind,
            },
        };
        if !record.policy.validate() {
            return Err(GateError::InvalidPlan);
        }
        if matches!(self.intent, Intent::Run(_)) {
            let old = read_record(
                session.coordinator_directory().map_err(|_| GateError::PlanChanged)?,
                &self.record_name(),
            )?;
            if old.policy != record.policy {
                return Err(GateError::PlanChanged);
            }
        }
        Ok(record)
    }
}
impl NativeMutationProvider for HookProvider<'_> {
    type Output = HookReceipt;
    fn plan(&mut self, session: &NativeCaptureSession) -> Result<PlannedChange, GateError> {
        self.account.revalidate().map_err(|_| GateError::PlanChanged)?;
        self.bundle.revalidate().map_err(|_| GateError::PlanChanged)?;
        if crate::wallpaper::installed_packages().map_err(|_| GateError::PlanChanged)?
            != self.packages
        {
            return Err(GateError::PlanChanged);
        }
        let record = self.policy(session)?;
        let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
        let home_metadata = home.metadata().map_err(|_| GateError::PlanChanged)?;
        let outputs = ryoku_recovery::hook_protocol::output_identities(
            &home,
            &record.policy.approval.outputs,
        )
        .map_err(|_| GateError::PlanChanged)?;
        let event = match &self.intent {
            Intent::Run(event) => Some(event),
            _ => None,
        };
        let plan = PlannedChange {
            operation_key: if event.is_some() {
                "configured-hook.run"
            } else {
                "configured-hook.authorize"
            }
            .into(),
            plan_fingerprint: digest(
                &serde_json::to_vec(&(
                    &record,
                    event,
                    self.bundle.id(),
                    &self.packages,
                    home_metadata.dev(),
                    home_metadata.ino(),
                    &outputs,
                ))
                .map_err(|_| GateError::InvalidPlan)?,
            ),
            affected_state_fingerprint: "00".repeat(32),
        };
        self.current = Some(record);
        self.last_plan = Some(plan.clone());
        Ok(plan)
    }
    fn authorize(&mut self, plan: &PlannedChange) -> Result<(), GateError> {
        authorize_helper_operator(self.peer, self.gid).map_err(|_| GateError::PreflightFailed)?;
        if !self.last_plan.as_ref().is_some_and(|current| {
            current.operation_key == plan.operation_key
                && current.plan_fingerprint == plan.plan_fingerprint
        }) {
            return Err(GateError::PlanChanged);
        }
        self.account.revalidate().map_err(|_| GateError::PlanChanged)
    }
    fn apply(
        &mut self,
        proof: &VerifiedLinkedGeneration,
        session: &NativeCaptureSession,
    ) -> Result<HookReceipt, GateError> {
        let old = self.last_plan.clone().ok_or(GateError::InvalidPlan)?;
        if self.plan(session)? != old {
            return Err(GateError::PlanChanged);
        }
        self.authorize(&old)?;
        let record = self.current.as_ref().ok_or(GateError::InvalidPlan)?;
        let policy_sha256 =
            digest(&serde_json::to_vec(&record.policy).map_err(|_| GateError::InvalidPlan)?);
        match &self.intent {
            Intent::Approve(_) => {
                save_record(
                    session.coordinator_directory().map_err(|_| GateError::PlanChanged)?,
                    &self.record_name(),
                    record,
                )?;
                Ok(HookReceipt { policy_sha256, completed: true, refusal: None })
            }
            Intent::Run(request) => {
                let home = self.account.capture_home().map_err(|_| GateError::PlanChanged)?;
                let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
                    .map_err(|_| GateError::PlanChanged)?;
                let receipt = session
                    .run_configured_hook(&HookCall {
                        policy: record.policy.clone(),
                        request: request.clone(),
                        home_sha256: digest(home_path.as_os_str().as_encoded_bytes()),
                        home_device: home.metadata().map_err(|_| GateError::PlanChanged)?.dev(),
                        home_inode: home.metadata().map_err(|_| GateError::PlanChanged)?.ino(),
                        output_identities: ryoku_recovery::hook_protocol::output_identities(
                            &home,
                            &record.policy.approval.outputs,
                        )
                        .map_err(|_| GateError::PlanChanged)?,
                        linked_plan_sha256: digest(
                            &serde_json::to_vec(&(&old, proof.linked()))
                                .map_err(|_| GateError::InvalidPlan)?,
                        ),
                        thumb: thumbnail(&home, request)?,
                    })
                    .map_err(|_| GateError::ApplyFailed)?;
                if !receipt.completed
                    || receipt.refusal.is_some()
                    || receipt.policy_sha256 != policy_sha256
                {
                    self.last_refusal =
                        receipt.refusal.or(Some("invalid hook supervisor receipt".into()));
                    return Err(GateError::ApplyFailed);
                }
                Ok(receipt)
            }
        }
    }
    fn health_check(&mut self, receipt: &HookReceipt) -> Result<(), GateError> {
        self.account.revalidate().map_err(|_| GateError::PostChangeFailed)?;
        if !receipt.completed || receipt.refusal.is_some() {
            return Err(GateError::PostChangeFailed);
        }
        Ok(())
    }
    fn cancellation_requested(&self) -> bool {
        false
    }
}
pub fn command(bytes: &[u8], index: u16) -> Result<(String, String, bool), GateError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| GateError::InvalidPlan)?;
    let list = value["postProcessing"].as_array().ok_or(GateError::InvalidPlan)?;
    if list.len() > 128 {
        return Err(GateError::InvalidPlan);
    }
    let commands = list
        .iter()
        .filter_map(|value| {
            let (command, kind) = if let Some(command) = value.as_str() {
                (command, "all")
            } else {
                (
                    value.get("command")?.as_str()?,
                    value
                        .get("type")
                        .and_then(|v| v.as_str())
                        .filter(|v| !v.is_empty())
                        .unwrap_or("all"),
                )
            };
            (!command.trim().is_empty()).then(|| (command.to_string(), kind.to_string()))
        })
        .collect::<Vec<_>>();
    let (command, kind) = commands.get(index as usize).cloned().ok_or(GateError::InvalidPlan)?;
    Ok((command, kind, value["postProcessOnRestore"].as_bool() == Some(true)))
}
fn validate_event(home: &File, request: &HookRequest) -> Result<(), GateError> {
    let outputs = crate::wallpaper::read_home(home, ".cache/ryogami/outputs.json", 4 * 1024 * 1024)
        .map_err(|_| GateError::PlanChanged)?;
    let outputs: serde_json::Value =
        serde_json::from_slice(&outputs).map_err(|_| GateError::PlanChanged)?;
    let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
        .map_err(|_| GateError::PlanChanged)?;
    let portable = request
        .path
        .strip_prefix(home_path.to_str().ok_or(GateError::PlanChanged)?)
        .filter(|p| p.starts_with('/'))
        .map(|p| format!("~{p}"));
    if !outputs.as_object().ok_or(GateError::PlanChanged)?.values().any(|entry| {
        (entry["path"] == request.path || portable.as_deref().is_some_and(|p| entry["path"] == p))
            && entry["type"] == request.kind
    }) {
        return Err(GateError::PlanChanged);
    }
    Ok(())
}
fn thumbnail(home: &File, request: &HookRequest) -> Result<String, GateError> {
    let bytes = match crate::wallpaper::read_home(
        home,
        ".cache/ryogami/wallpaper/index.json",
        16 * 1024 * 1024,
    ) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => return Err(GateError::PlanChanged),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| GateError::PlanChanged)?;
    let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
        .map_err(|_| GateError::PlanChanged)?;
    let portable = request
        .path
        .strip_prefix(home_path.to_str().ok_or(GateError::PlanChanged)?)
        .filter(|p| p.starts_with('/'))
        .map(|p| format!("~{p}"));
    let matching = value
        .as_object()
        .ok_or(GateError::PlanChanged)?
        .values()
        .filter(|entry| {
            entry["path"] == request.path || portable.as_deref().is_some_and(|p| entry["path"] == p)
        })
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [] => Ok(String::new()),
        [entry] => {
            let thumb = entry.get("thumb").and_then(|v| v.as_str()).unwrap_or("");
            Ok(if let Some(rel) = thumb.strip_prefix("~/") {
                format!("{}/{rel}", home_path.to_str().ok_or(GateError::PlanChanged)?)
            } else {
                thumb.into()
            })
        }
        _ => Err(GateError::PlanChanged),
    }
}

fn read_record(directory: &File, name: &str) -> Result<ApprovalRecord, GateError> {
    let file = File::from(
        rustix::fs::openat2(
            directory,
            name,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| GateError::PreflightFailed)?,
    );
    let m = file.metadata().map_err(|_| GateError::PreflightFailed)?;
    if !m.is_file()
        || m.uid() != 0
        || m.gid() != 0
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
        || m.len() > 32768
    {
        return Err(GateError::PreflightFailed);
    }
    let mut bytes = Vec::new();
    file.take(32769).read_to_end(&mut bytes).map_err(|_| GateError::PreflightFailed)?;
    serde_json::from_slice(&bytes).map_err(|_| GateError::PreflightFailed)
}
fn save_record(directory: &File, name: &str, record: &ApprovalRecord) -> Result<(), GateError> {
    let temporary = format!("{}.pending", record.operation_id.as_str());
    let mut file = File::from(
        rustix::fs::openat2(
            directory,
            &temporary,
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
            rustix::fs::ResolveFlags::BENEATH | rustix::fs::ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| GateError::ApplyFailed)?,
    );
    let bytes = serde_json::to_vec(record).map_err(|_| GateError::ApplyFailed)?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|_| GateError::ApplyFailed)?;
    rustix::fs::renameat(directory, &temporary, directory, name)
        .map_err(|_| GateError::ApplyFailed)?;
    directory.sync_all().map_err(|_| GateError::ApplyFailed)
}
