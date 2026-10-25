//! Typed configured post-processing policy. A policy value is not approval;
//! only the root helper may load its protected approval record and dispatch it.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOutput {
    KittyUser,
    FishUser,
}
impl HookOutput {
    pub fn path(self) -> &'static str {
        match self {
            Self::KittyUser => ".config/kitty/user.conf",
            Self::FishUser => ".config/fish/user.fish",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookApprovalRequest {
    pub index: u16,
    pub command_sha256: String,
    pub outputs: Vec<HookOutput>,
    pub timeout_seconds: u16,
    pub allow_restore: bool,
}
impl HookApprovalRequest {
    pub fn validate(&self) -> bool {
        self.index < 128
            && self.command_sha256.len() == 64
            && self.command_sha256.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            && (1..=30).contains(&self.timeout_seconds)
            && self.outputs.len() <= 2
            && (self.outputs.len() != 2 || self.outputs[0] != self.outputs[1])
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookRequest {
    pub index: u16,
    pub command_sha256: String,
    pub restoring: bool,
    pub path: String,
    pub kind: String,
}
impl HookRequest {
    pub fn validate(&self) -> bool {
        self.index < 128
            && self.command_sha256.len() == 64
            && self.command_sha256.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            && matches!(self.kind.as_str(), "static" | "video")
            && self.path.starts_with('/')
            && self.path.len() <= 4096
            && !self.path.chars().any(char::is_control)
            && self.path.split('/').skip(1).all(|part| !matches!(part, "" | "." | ".."))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookPolicy {
    pub account: crate::AccountPrincipal,
    pub uid: u32,
    pub approval: HookApprovalRequest,
    pub configuration_sha256: String,
    pub shell_sha256: String,
    pub command: String,
    pub kind: String,
}
impl HookPolicy {
    pub fn validate(&self) -> bool {
        self.uid != 0
            && self.uid != u32::MAX
            && self.approval.validate()
            && !self.command.is_empty()
            && self.command.len() <= 8192
            && !self.command.contains('\0')
            && self.approval.command_sha256
                == crate::wallpaper_protocol::digest(self.command.as_bytes())
            && [self.configuration_sha256.as_str(), self.shell_sha256.as_str()].iter().all(|hash| {
                hash.len() == 64
                    && hash.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
            && matches!(self.kind.as_str(), "all" | "static" | "video")
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookCall {
    pub policy: HookPolicy,
    pub request: HookRequest,
    pub home_sha256: String,
    pub home_device: u64,
    pub home_inode: u64,
    pub output_identities: Vec<HookFileIdentity>,
    pub linked_plan_sha256: String,
    pub thumb: String,
}
impl HookCall {
    pub fn validate(&self, uid: u32, home: &str) -> bool {
        self.policy.validate()
            && self.request.validate()
            && self.policy.uid == uid
            && self.request.index == self.policy.approval.index
            && self.request.command_sha256 == self.policy.approval.command_sha256
            && (!self.request.restoring || self.policy.approval.allow_restore)
            && (self.policy.kind == "all" || self.policy.kind == self.request.kind)
            && self.home_sha256 == crate::wallpaper_protocol::digest(home.as_bytes())
            && self.output_identities.len() == self.policy.approval.outputs.len()
            && self
                .output_identities
                .iter()
                .zip(&self.policy.approval.outputs)
                .all(|(identity, output)| identity.output == *output)
            && self.linked_plan_sha256.len() == 64
            && self
                .linked_plan_sha256
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            && self.thumb.len() <= 4096
            && !self.thumb.chars().any(char::is_control)
    }
    pub fn expanded_command(&self) -> String {
        let name = self.request.path.rsplit('/').next().unwrap_or("");
        let replacements = [
            ("%type%", self.request.kind.as_str()),
            ("%name%", name),
            ("%path%", self.request.path.as_str()),
            ("%thumb%", self.thumb.as_str()),
        ];
        // Match upstream strings.NewReplacer: substituted values are not expanded again.
        let mut result = String::new();
        let mut remaining = self.policy.command.as_str();
        while !remaining.is_empty() {
            if let Some((key, value)) =
                replacements.iter().find(|(key, _)| remaining.starts_with(key))
            {
                result.push('\'');
                result.push_str(&value.replace('\'', "'\\''"));
                result.push('\'');
                remaining = &remaining[key.len()..];
            } else {
                let c = remaining.chars().next().expect("nonempty");
                result.push(c);
                remaining = &remaining[c.len_utf8()..];
            }
        }
        result
    }
}
/// Equality binding from the helper's open capture home, never authorization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookFileIdentity {
    pub output: HookOutput,
    pub device: u64,
    pub inode: u64,
}
pub fn output_identities(
    home: &std::fs::File,
    outputs: &[HookOutput],
) -> std::io::Result<Vec<HookFileIdentity>> {
    use std::os::unix::fs::MetadataExt;
    outputs
        .iter()
        .map(|output| {
            let file = std::fs::File::from(rustix::fs::openat2(
                home,
                output.path(),
                rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
                rustix::fs::ResolveFlags::BENEATH
                    | rustix::fs::ResolveFlags::NO_SYMLINKS
                    | rustix::fs::ResolveFlags::NO_XDEV,
            )?);
            let metadata = file.metadata()?;
            Ok(HookFileIdentity { output: *output, device: metadata.dev(), inode: metadata.ino() })
        })
        .collect()
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookReceipt {
    pub policy_sha256: String,
    pub completed: bool,
    pub refusal: Option<String>,
}
