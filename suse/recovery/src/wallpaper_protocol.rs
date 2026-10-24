//! Exact first wallpaper action shared by the helper and native actor broker.
//! No executable, hook, package operation or backend socket path is in this wire.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_ACTOR_REPLY_BYTES: usize = 64 * 1024;
pub const MAX_QUERY_REPLY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum PublicRequest {
    Query { id: Option<i64>, frame: Vec<u8> },
    Apply { id: i64, action: WallpaperApply, plain: bool },
    Handoff { id: i64, action: AutomaticHandoff },
}

/// Transport to issue176, not an executable permit or protected claim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomaticHandoff {
    pub action: AutomaticAction,
    #[serde(default)]
    pub frame: Option<WallpaperFrame>,
    #[serde(default)]
    pub replay: Option<WallpaperReplay>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AutomaticAction {
    #[serde(rename = "wallpaper.restore")]
    WallpaperRestore,
    #[serde(rename = "workspace.playback")]
    WorkspacePlayback,
    #[serde(rename = "wallpaper.reload")]
    WallpaperReload,
    #[serde(rename = "startup.migrate")]
    StartupMigrate,
    #[serde(rename = "startup.restore-and-scan")]
    StartupRestoreAndScan,
    #[serde(rename = "catalog.initialize")]
    CatalogInitialize,
    #[serde(rename = "catalog.rescan")]
    CatalogRescan,
    #[serde(rename = "watch.settings")]
    WatchSettings,
    #[serde(rename = "watch.library")]
    WatchLibrary,
    #[serde(rename = "watch.window-manager")]
    WatchWindowManager,
    #[serde(rename = "watch.workshop")]
    WatchWorkshop,
    #[serde(rename = "hooks.start")]
    HooksStart,
    #[serde(rename = "hooks.after-apply")]
    HooksAfterApply,
    #[serde(rename = "wallpaper-engine.stop")]
    WallpaperEngineStop,
    #[serde(rename = "renderer.orphans")]
    RendererOrphans,
    #[serde(rename = "shell.wallpaper.stage")]
    ShellWallpaperStage,
    #[serde(rename = "shell.wallpaper.theme")]
    ShellWallpaperTheme,
}
/// Declarative handoff only. The owner must independently resolve the stored
/// selection/assignment, authorize this exact purpose and capture its write set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperReplay {
    pub reason: ReplayReason,
    pub stored_state: ReplayState,
    pub record: bool,
    pub action: WallpaperApply,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayReason {
    Restore,
    Workspace,
    Reload,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayState {
    Outputs,
    WorkspaceAssignments,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperFrame {
    pub default: WallpaperFrameEntry,
    pub outputs: BTreeMap<String, WallpaperFrameEntry>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperFrameEntry {
    pub path: String,
    pub live: bool,
    pub video: bool,
    pub depth: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicRpc {
    method: String,
    #[serde(default)]
    params: serde_json::Value,
    id: i64,
}

/// Parse every frame on a public connection, including after a subscription.
/// An earlier query never grants a raw/reusable native write channel.
pub fn parse_public(bytes: &[u8]) -> Result<PublicRequest, WallpaperProtocolError> {
    bounded_line(bytes, MAX_REQUEST_BYTES)?;
    if bytes == b"subscribe wallpaper\n" {
        return Ok(PublicRequest::Query { id: None, frame: bytes.to_vec() });
    }
    if bytes == b"wallpaper ui\n" {
        return Ok(PublicRequest::Query { id: None, frame: bytes.to_vec() });
    }
    if let Some(rest) = bytes.strip_prefix(b"wallpaper set ") {
        let rest = std::str::from_utf8(&rest[..rest.len() - 1])
            .map_err(|_| WallpaperProtocolError::InvalidFrame)?;
        let (outputs, path) = if let Some(rest) = rest.strip_prefix("--screen ") {
            let (output, path) =
                rest.split_once(' ').ok_or(WallpaperProtocolError::InvalidAction)?;
            (vec![output.to_owned()], path)
        } else {
            (vec![], rest)
        };
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let kind = if ["mp4", "webm", "mkv", "avi", "mov", "gif"].contains(&extension.as_str()) {
            WallpaperKind::Video
        } else {
            WallpaperKind::Static
        };
        let action = WallpaperApply {
            kind,
            path: path.into(),
            outputs,
            mute: BTreeMap::new(),
            volume: BTreeMap::new(),
        };
        action.validate()?;
        return Ok(PublicRequest::Apply { id: 0, action, plain: true });
    }
    let request: PublicRpc =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if request.id < 0 || !(request.params.is_object() || request.params.is_null()) {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    if request.method == "wall.apply" {
        let action: WallpaperApply = serde_json::from_value(request.params)
            .map_err(|_| WallpaperProtocolError::InvalidAction)?;
        action.validate()?;
        return Ok(PublicRequest::Apply { id: request.id, action, plain: false });
    }
    if request.method == "broker.automatic" {
        let action: AutomaticHandoff = serde_json::from_value(request.params)
            .map_err(|_| WallpaperProtocolError::InvalidAction)?;
        let expected_replay = match action.action {
            AutomaticAction::WallpaperRestore => {
                Some((ReplayReason::Restore, ReplayState::Outputs))
            }
            AutomaticAction::WorkspacePlayback => {
                Some((ReplayReason::Workspace, ReplayState::WorkspaceAssignments))
            }
            AutomaticAction::WallpaperReload => Some((ReplayReason::Reload, ReplayState::Outputs)),
            _ => None,
        };
        match (&action.replay, expected_replay) {
            (Some(replay), Some((reason, state)))
                if replay.reason == reason && replay.stored_state == state =>
            {
                replay.action.validate()?;
            }
            (None, None) => {}
            _ => return Err(WallpaperProtocolError::InvalidAction),
        }
        if let Some(frame) = &action.frame
            && (frame.outputs.len() > 16
                || frame.outputs.keys().any(|name| !valid_output(name))
                || std::iter::once(&frame.default).chain(frame.outputs.values()).any(|entry| {
                    entry.path.len() > 4096
                        || entry.depth.len() > 4096
                        || entry.path.chars().chain(entry.depth.chars()).any(char::is_control)
                }))
        {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        return Ok(PublicRequest::Handoff { id: request.id, action });
    }
    if query_method(&request.method) {
        return Ok(PublicRequest::Query { id: Some(request.id), frame: bytes.to_vec() });
    }
    Err(WallpaperProtocolError::UnownedAction)
}

/// Reviewed nonmutating cases in the pinned daemon's RPC dispatcher. Preview,
/// picker lifecycle, settings setters and spelling-based "get" guesses do not
/// enter this table: several generate files or launch processes.
pub fn query_method(method: &str) -> bool {
    matches!(
        method,
        "subscribe"
            | "status"
            | "wm.focusedOutput"
            | "state.get"
            | "wall.list"
            | "wall.outputs"
            | "wall.workspaces"
            | "effects.list"
            | "optimize.status"
            | "wall.rotation_status"
            | "playlist.list"
            | "playlist.members"
            | "playlist.memberships"
            | "upscale.status"
    )
}

pub fn bounded_line(bytes: &[u8], maximum: usize) -> Result<(), WallpaperProtocolError> {
    if bytes.len() > maximum {
        return Err(WallpaperProtocolError::TooLarge);
    }
    if bytes.last() != Some(&b'\n') || bytes[..bytes.len().saturating_sub(1)].contains(&b'\n') {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperKind {
    Static,
    Video,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WallpaperApply {
    #[serde(rename = "type")]
    pub kind: WallpaperKind,
    pub path: String,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default, rename = "outputs_audio")]
    pub mute: BTreeMap<String, bool>,
    #[serde(default, rename = "outputs_volume")]
    pub volume: BTreeMap<String, u8>,
}

impl WallpaperApply {
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        if !self.path.starts_with('/')
            || self.path.len() > 4096
            || self.path.chars().any(char::is_control)
            || self.outputs.len() > 16
            || self.mute.len() > 16
            || self.volume.len() > 16
            || self.outputs.iter().any(|name| !valid_output(name))
            || self.outputs.iter().collect::<std::collections::BTreeSet<_>>().len()
                != self.outputs.len()
            || self.mute.keys().any(|name| !valid_output(name))
            || self.volume.iter().any(|(name, volume)| !valid_output(name) || *volume > 100)
            || self
                .mute
                .keys()
                .chain(self.volume.keys())
                .any(|name| !self.outputs.is_empty() && !self.outputs.contains(name))
        {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        Ok(())
    }

    pub fn frame(&self, id: i64) -> Result<Vec<u8>, WallpaperProtocolError> {
        self.validate()?;
        if id < 0 {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        let mut bytes =
            serde_json::to_vec(&RpcApply { method: "wall.apply".into(), params: self.clone(), id })
                .map_err(|_| WallpaperProtocolError::InvalidFrame)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(WallpaperProtocolError::TooLarge);
        }
        Ok(bytes)
    }
}

fn valid_output(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcApply {
    method: String,
    params: WallpaperApply,
    id: i64,
}

pub fn parse_apply(bytes: &[u8]) -> Result<(i64, WallpaperApply), WallpaperProtocolError> {
    bounded_line(bytes, MAX_REQUEST_BYTES)?;
    let request: RpcApply =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if request.method != "wall.apply" || request.id < 0 {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    request.params.validate()?;
    Ok((request.id, request.params))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorIdentity {
    pub pid: u32,
    pub start_time: u64,
}

/// The helper sends this only after Applying is durable and the linked proof
/// has been read back. Root peer authentication is required independently;
/// a hash supplied by an unprivileged client is never an authorization token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorCall {
    pub actor: ActorIdentity,
    pub plan_sha256: String,
    pub frame: Vec<u8>,
    pub frame_sha256: String,
}

impl ActorCall {
    pub fn validate(&self, expected_actor: &ActorIdentity) -> Result<(), WallpaperProtocolError> {
        if self.actor != *expected_actor
            || self.actor.pid == 0
            || self.actor.start_time == 0
            || !valid_sha256(&self.plan_sha256)
            || self.frame_sha256 != digest(&self.frame)
        {
            return Err(WallpaperProtocolError::InvalidPermit);
        }
        let (id, action) = parse_apply(&self.frame)?;
        if action.frame(id)? != self.frame {
            return Err(WallpaperProtocolError::InvalidPermit);
        }
        Ok(())
    }
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WallpaperProtocolError {
    InvalidFrame,
    InvalidAction,
    InvalidPermit,
    TooLarge,
    UnownedAction,
}

impl std::fmt::Display for WallpaperProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid wallpaper request: {self:?}")
    }
}
impl std::error::Error for WallpaperProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> WallpaperApply {
        WallpaperApply {
            kind: WallpaperKind::Static,
            path: "/home/fixture/Pictures/a; literal wallpaper.jpg".into(),
            outputs: vec!["DP-1".into()],
            mute: BTreeMap::from([("DP-1".into(), true)]),
            volume: BTreeMap::from([("DP-1".into(), 35)]),
        }
    }

    #[test]
    fn restore_and_workspace_handoffs_require_exact_purpose_and_stored_state_binding() {
        for (name, reason, stored) in [
            ("wallpaper.restore", "restore", "outputs"),
            ("workspace.playback", "workspace", "workspace_assignments"),
            ("wallpaper.reload", "reload", "outputs"),
        ] {
            let mut request = serde_json::json!({"id":5,"method":"broker.automatic","params":{
                "action":name,"replay":{"reason":reason,"stored_state":stored,"record":false,
                    "action":{"type":"static","path":"/home/fixture/a.png","outputs":[]}}
            }});
            let frame = |value: &serde_json::Value| {
                let mut frame = serde_json::to_vec(value).unwrap();
                frame.push(b'\n');
                frame
            };
            assert!(matches!(parse_public(&frame(&request)), Ok(PublicRequest::Handoff { .. })));
            // A typed dependency handoff is never an executable public apply.
            assert!(parse_apply(&frame(&request)).is_err());
            request["params"]["replay"]["stored_state"] = serde_json::json!("arbitrary/path");
            assert!(parse_public(&frame(&request)).is_err());
            request["params"]["replay"]["stored_state"] = serde_json::json!(stored);
            request["params"]["replay"]["reason"] = serde_json::json!("user");
            assert!(parse_public(&frame(&request)).is_err());
            request["params"].as_object_mut().unwrap().remove("replay");
            assert!(parse_public(&frame(&request)).is_err());
        }
    }

    #[test]
    fn exact_action_roundtrips_without_executable_or_shell_interpretation() {
        let value = action();
        let frame = value.frame(17).unwrap();
        assert_eq!(parse_apply(&frame).unwrap(), (17, value));
        for extra in ["workspace", "we_id", "command", "hook", "target"] {
            let mut request: serde_json::Value = serde_json::from_slice(&frame).unwrap();
            request["params"][extra] = serde_json::json!("unexpected");
            let mut bytes = serde_json::to_vec(&request).unwrap();
            bytes.push(b'\n');
            assert!(parse_apply(&bytes).is_err(), "{extra}");
        }
        for suffix in [b"{}\n".as_slice(), b"\n", b"trailing"] {
            let mut changed = frame.clone();
            changed.extend(suffix);
            assert!(parse_apply(&changed).is_err());
        }
        assert!(parse_apply(&frame[..frame.len() - 1]).is_err());
        assert!(parse_apply(&vec![b'x'; MAX_REQUEST_BYTES + 1]).is_err());
    }

    #[test]
    fn permit_binds_exact_frame_and_process_start_identity() {
        let actor = ActorIdentity { pid: 17, start_time: 42 };
        let frame = action().frame(1).unwrap();
        let call = ActorCall {
            actor: actor.clone(),
            plan_sha256: "ab".repeat(32),
            frame_sha256: digest(&frame),
            frame,
        };
        call.validate(&actor).unwrap();
        let mut changed = call.clone();
        changed.frame = action().frame(2).unwrap();
        assert!(changed.validate(&actor).is_err());
        assert!(call.validate(&ActorIdentity { pid: 17, start_time: 43 }).is_err());
        assert!(call.validate(&ActorIdentity { pid: 18, start_time: 42 }).is_err());
        let mut changed = call;
        changed.plan_sha256 = "not-a-plan".into();
        assert!(changed.validate(&actor).is_err());
    }

    #[test]
    fn invalid_outputs_volume_and_ambiguous_paths_fail_before_dispatch() {
        for path in ["relative.jpg", "", "/tmp/a\nwall.jpg", "/tmp/a\0wall.jpg"] {
            let mut value = action();
            value.path = path.into();
            assert!(value.validate().is_err());
        }
        let mut value = action();
        value.outputs.push("DP-1".into());
        assert!(value.validate().is_err());
        let mut value = action();
        value.volume.insert("DP-1".into(), 101);
        assert!(value.validate().is_err());
        let mut value = action();
        value.mute.insert("HDMI-A-1".into(), true);
        assert!(value.validate().is_err());
    }

    #[test]
    fn query_connections_do_not_grant_mutations_or_accept_client_permits() {
        let query = b"{\"method\":\"wall.list\",\"params\":{},\"id\":1}\n";
        assert!(matches!(parse_public(query), Ok(PublicRequest::Query { .. })));
        assert!(matches!(
            parse_public(&action().frame(1).unwrap()),
            Ok(PublicRequest::Apply { .. })
        ));
        for method in [
            "state.set",
            "wall.delete",
            "wall.cache_rebuild",
            "source.download",
            "workshop.download",
            "models.add",
            "effects.preview",
            "grade.commit",
            "picker.state",
        ] {
            let mut frame =
                serde_json::to_vec(&serde_json::json!({"method":method,"params":{},"id":1}))
                    .unwrap();
            frame.push(b'\n');
            assert_eq!(
                parse_public(&frame).unwrap_err(),
                WallpaperProtocolError::UnownedAction,
                "{method}"
            );
        }
        let mut forged: serde_json::Value =
            serde_json::from_slice(&action().frame(1).unwrap()).unwrap();
        forged["permit"] = serde_json::json!({"plan_sha256":"ab".repeat(32)});
        let mut frame = serde_json::to_vec(&forged).unwrap();
        frame.push(b'\n');
        assert!(parse_public(&frame).is_err());
        let mut reused = query.to_vec();
        reused.extend(action().frame(1).unwrap());
        assert!(parse_public(&reused).is_err());
    }
}
