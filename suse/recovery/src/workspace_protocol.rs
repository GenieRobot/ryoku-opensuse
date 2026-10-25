//! Manual workspace selection is an intent; only a helper-resolved plan may
//! become an actor call. Automatic workspace playback owns a separate permit.
use crate::wallpaper_protocol::{WallpaperKind, WallpaperProtocolError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceOperation {
    Assign,
    Unassign,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSelector {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub output: String,
}
impl WorkspaceSelector {
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        if self.id.len() > 128
            || self.name.len() > 256
            || self.output.len() > 128
            || self
                .id
                .chars()
                .chain(self.name.chars())
                .chain(self.output.chars())
                .any(char::is_control)
            || !self.output.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
        {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWallpaper {
    #[serde(rename = "type")]
    pub kind: WallpaperKind,
    pub path: String,
    pub mute: bool,
    pub volume: u8,
}
impl WorkspaceWallpaper {
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        if self.path.is_empty()
            || self.path.len() > 4096
            || !self.path.starts_with('/')
            || self.path.chars().any(char::is_control)
            || self.volume > 100
        {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWallpaperIntent {
    #[serde(rename = "type")]
    pub kind: WallpaperKind,
    pub path: String,
    pub mute: Option<bool>,
    pub volume: Option<u8>,
}
impl WorkspaceWallpaperIntent {
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        WorkspaceWallpaper {
            kind: self.kind,
            path: self.path.clone(),
            mute: self.mute.unwrap_or(true),
            volume: self.volume.unwrap_or(100),
        }
        .validate()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRequest {
    pub operation: WorkspaceOperation,
    pub workspace: WorkspaceSelector,
    pub wallpaper: Option<WorkspaceWallpaperIntent>,
}
impl WorkspaceRequest {
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        self.workspace.validate()?;
        if self.operation == WorkspaceOperation::Unassign && self.wallpaper.is_some() {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        if let Some(wallpaper) = &self.wallpaper {
            wallpaper.validate()?;
        }
        Ok(())
    }
}

/// This structure is never part of the public/helper request language. The
/// helper resolves identity, current target, source and captured file bindings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWorkspace {
    pub request: WorkspaceRequest,
    pub provider: String,
    pub target: WorkspaceSelector,
    pub active: bool,
    pub wallpaper: Option<WorkspaceWallpaper>,
    pub workspace_sha256: Option<String>,
    pub outputs_sha256: Option<String>,
    pub remove_keys: Vec<String>,
}
impl PreparedWorkspace {
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        self.request.validate()?;
        self.target.validate()?;
        if self.provider != "hyprland"
            || self.target.id.is_empty()
            || self.target.output.is_empty()
            || self.remove_keys.len() > 2
            || self
                .remove_keys
                .iter()
                .any(|key| key.is_empty() || key.len() > 256 || key.chars().any(char::is_control))
            || self.remove_keys.iter().collect::<std::collections::BTreeSet<_>>().len()
                != self.remove_keys.len()
            || self.workspace_sha256.iter().chain(self.outputs_sha256.iter()).any(|hash| {
                hash.len() != 64
                    || !hash.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            || (self.request.operation == WorkspaceOperation::Assign && self.wallpaper.is_none())
        {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        if !self.request.workspace.id.is_empty() && self.request.workspace.id != self.target.id
            || !self.request.workspace.output.is_empty()
                && self.request.workspace.output != self.target.output
            || self.request.workspace.id.is_empty() && !self.active
        {
            return Err(WallpaperProtocolError::InvalidPermit);
        }
        if let Some(wallpaper) = &self.wallpaper {
            wallpaper.validate()?;
        }
        if let Some(intent) = &self.request.wallpaper {
            let resolved = self.wallpaper.as_ref().ok_or(WallpaperProtocolError::InvalidPermit)?;
            if intent.path != resolved.path
                || intent.kind != resolved.kind
                || intent.mute.is_some_and(|value| value != resolved.mute)
                || intent.volume.is_some_and(|value| value != resolved.volume)
            {
                return Err(WallpaperProtocolError::InvalidPermit);
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRpcParams {
    #[serde(default)]
    pub workspace: Option<WorkspaceSelector>,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub output: String,
    #[serde(default, rename = "type")]
    pub kind: Option<WallpaperKind>,
    #[serde(default)]
    pub path: String,
    #[serde(default, rename = "outputs_audio")]
    pub mute: BTreeMap<String, bool>,
    #[serde(default, rename = "outputs_volume")]
    pub volume: BTreeMap<String, u8>,
}
impl WorkspaceRpcParams {
    pub fn into_request(
        self,
        operation: WorkspaceOperation,
    ) -> Result<WorkspaceRequest, WallpaperProtocolError> {
        let workspace = match self.workspace {
            Some(value) if self.id.is_empty() && self.name.is_empty() && self.output.is_empty() => {
                value
            }
            None => WorkspaceSelector { id: self.id, name: self.name, output: self.output },
            _ => return Err(WallpaperProtocolError::InvalidAction),
        };
        let kind = self.kind.or_else(|| (!self.path.is_empty()).then(|| inferred_kind(&self.path)));
        let wallpaper = match (kind, self.path.is_empty()) {
            (Some(kind), false) => Some(WorkspaceWallpaperIntent {
                kind,
                path: self.path,
                mute: self.mute.get(&workspace.output).copied(),
                volume: self.volume.get(&workspace.output).copied(),
            }),
            (None, true) if self.mute.is_empty() && self.volume.is_empty() => None,
            _ => return Err(WallpaperProtocolError::InvalidAction),
        };
        if self.mute.keys().chain(self.volume.keys()).any(|output| output != &workspace.output) {
            return Err(WallpaperProtocolError::InvalidAction);
        }
        let request = WorkspaceRequest { operation, workspace, wallpaper };
        request.validate()?;
        Ok(request)
    }
}

pub(crate) fn inferred_kind(path: &str) -> WallpaperKind {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ["mp4", "webm", "mkv", "avi", "mov"].contains(&extension.as_str()) {
        WallpaperKind::Video
    } else {
        WallpaperKind::Static
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_resolved_bound_manual_actions_can_become_private_actor_frames() {
        let params: WorkspaceRpcParams = serde_json::from_str(
            r#"{"workspace":{"id":"work","output":"DP-1"},"path":"/home/fixture/a.gif"}"#,
        )
        .unwrap();
        let request = params.into_request(WorkspaceOperation::Assign).unwrap();
        assert_eq!(request.wallpaper.as_ref().unwrap().kind, WallpaperKind::Static);
        let mut change = WorkspaceChange::intent(request.clone());
        assert!(change.frame(9).is_err());
        change.prepared = Some(PreparedWorkspace {
            request,
            provider: "hyprland".into(),
            target: WorkspaceSelector {
                id: "work".into(),
                name: "work".into(),
                output: "DP-1".into(),
            },
            active: true,
            wallpaper: Some(WorkspaceWallpaper {
                kind: WallpaperKind::Static,
                path: "/home/fixture/a.gif".into(),
                mute: true,
                volume: 100,
            }),
            workspace_sha256: Some("ab".repeat(32)),
            outputs_sha256: None,
            remove_keys: vec![],
        });
        let frame = change.frame(9).unwrap();
        assert_eq!(parse_frame(&frame).unwrap(), (9, change.clone()));
        assert!(crate::wallpaper_protocol::parse_public(&frame).is_err());
        let actor = crate::wallpaper_protocol::ActorIdentity { pid: 17, start_time: 42 };
        let call = crate::wallpaper_protocol::ActorCall {
            actor: actor.clone(),
            account_home_sha256: crate::wallpaper_protocol::digest(b"/home/fixture"),
            plan_sha256: "cd".repeat(32),
            frame_sha256: crate::wallpaper_protocol::digest(&frame),
            frame,
        };
        call.validate_for_home(&actor, "/home/fixture").unwrap();
        change.prepared.as_mut().unwrap().target.id = "different".into();
        assert!(change.frame(9).is_err());
        change.prepared.as_mut().unwrap().target.id = "work".into();
        change.prepared.as_mut().unwrap().wallpaper.as_mut().unwrap().path =
            "/home/fixture/other.png".into();
        assert!(change.frame(9).is_err());
    }
    #[test]
    fn retained_current_screen_plain_aliases_reach_only_manual_intents() {
        for (frame, operation) in [
            (b"wallpaper assign --screen DP-1\n".as_slice(), WorkspaceOperation::Assign),
            (b"wallpaper unassign --screen DP-1\n", WorkspaceOperation::Unassign),
        ] {
            let crate::wallpaper_protocol::PublicRequest::Workspace { action, plain, .. } =
                crate::wallpaper_protocol::parse_public(frame).unwrap()
            else {
                panic!("wrong boundary")
            };
            assert!(plain);
            assert_eq!(action.operation, operation);
            assert_eq!(action.workspace.output, "DP-1");
            assert!(action.workspace.id.is_empty());
        }
        for frame in [
            b"wallpaper --screen DP-1 assign\n".as_slice(),
            b"wallpaper set --workspace --screen DP-1 /home/fixture/a.png\n",
            b"wallpaper set /home/fixture/a.png --screen DP-1 --workspace\n",
        ] {
            let crate::wallpaper_protocol::PublicRequest::Workspace { action, plain, .. } =
                crate::wallpaper_protocol::parse_public(frame).unwrap()
            else {
                panic!("lost workspace alias")
            };
            assert!(plain);
            assert_eq!(action.operation, WorkspaceOperation::Assign);
            assert_eq!(action.workspace.output, "DP-1");
        }
        for frame in [
            b"wallpaper unassign --screen DP-1 /tmp/extra.png\n".as_slice(),
            b"wallpaper assign --screen DP-1 relative.png\n",
        ] {
            assert!(crate::wallpaper_protocol::parse_public(frame).is_err());
        }
    }
    #[test]
    fn current_screen_intent_stays_unprepared_and_cannot_smuggle_root_plan() {
        let params: WorkspaceRpcParams = serde_json::from_str(r#"{"output":"DP-1"}"#).unwrap();
        let request = params.into_request(WorkspaceOperation::Assign).unwrap();
        assert!(request.workspace.id.is_empty());
        assert!(request.wallpaper.is_none());
        assert!(
            serde_json::from_str::<WorkspaceRpcParams>(
                r#"{"output":"DP-1","prepared":{"active":true}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<WorkspaceRequest>(r#"{"operation":"assign","workspace":{"output":"DP-1"},"wallpaper":null,"provider":"hyprland"}"#).is_err());
    }
    #[test]
    fn ambiguous_targets_unassign_sources_and_unbound_audio_are_rejected() {
        for params in [
            r#"{"workspace":{"id":"1","output":"DP-1"},"output":"DP-2"}"#,
            r#"{"output":"DP-1","outputs_audio":{"DP-2":true}}"#,
            r#"{"output":"DP-1","type":"video","path":"/home/fixture/a.mp4","outputs_volume":{"DP-1":101}}"#,
        ] {
            let value: WorkspaceRpcParams = serde_json::from_str(params).unwrap();
            assert!(value.into_request(WorkspaceOperation::Assign).is_err());
        }
        let params: WorkspaceRpcParams = serde_json::from_str(
            r#"{"output":"DP-1","type":"static","path":"/home/fixture/a.png"}"#,
        )
        .unwrap();
        assert!(params.into_request(WorkspaceOperation::Unassign).is_err());
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceChange {
    pub request: WorkspaceRequest,
    pub prepared: Option<PreparedWorkspace>,
}
impl WorkspaceChange {
    pub fn intent(request: WorkspaceRequest) -> Self {
        Self { request, prepared: None }
    }
    pub fn validate(&self) -> Result<(), WallpaperProtocolError> {
        self.request.validate()?;
        if let Some(prepared) = &self.prepared {
            prepared.validate()?;
            if prepared.request != self.request {
                return Err(WallpaperProtocolError::InvalidPermit);
            }
        }
        Ok(())
    }
    pub fn frame(&self, id: i64) -> Result<Vec<u8>, WallpaperProtocolError> {
        self.validate()?;
        let prepared = self.prepared.as_ref().ok_or(WallpaperProtocolError::InvalidPermit)?;
        if id < 0 {
            return Err(WallpaperProtocolError::InvalidFrame);
        }
        let mut frame = serde_json::to_vec(&WorkspaceFrame {
            method: "broker.workspace".into(),
            id,
            params: prepared.clone(),
        })
        .map_err(|_| WallpaperProtocolError::InvalidFrame)?;
        frame.push(b'\n');
        Ok(frame)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFrame {
    method: String,
    id: i64,
    params: PreparedWorkspace,
}
pub fn parse_frame(bytes: &[u8]) -> Result<(i64, WorkspaceChange), WallpaperProtocolError> {
    crate::wallpaper_protocol::bounded_line(bytes, crate::wallpaper_protocol::MAX_REQUEST_BYTES)?;
    let frame: WorkspaceFrame =
        serde_json::from_slice(bytes).map_err(|_| WallpaperProtocolError::InvalidFrame)?;
    if frame.method != "broker.workspace" || frame.id < 0 {
        return Err(WallpaperProtocolError::InvalidFrame);
    }
    frame.params.validate()?;
    Ok((
        frame.id,
        WorkspaceChange { request: frame.params.request.clone(), prepared: Some(frame.params) },
    ))
}
