use crate::provider::Provider;
use serde::{Deserialize, Serialize};

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// Each action is individually typed. There is no shell, arbitrary URL proxy,
/// privilege escalation, or generic service/package dispatcher.
#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status {},
    Providers {},
    ProwlStatus {},
    ConfigureProwl {
        port: u16,
        token: Option<String>,
    },
    DisableProwl {},
    ReadProwl {
        query: crate::prowl::Query,
    },
    IssueAskClient {
        lifetime_seconds: Option<u32>,
    },
    AskClients {},
    RevokeAskClient {
        id: String,
    },
    PutProvider {
        id: String,
        provider: Provider,
    },
    RefreshModels {
        id: String,
        provider: String,
    },
    UpdateProvider {
        id: String,
        changes: crate::provider::ProviderEdit,
    },
    RemoveProvider {
        id: String,
    },
    SetRoute {
        lane: Lane,
        provider: String,
        model: String,
    },
    Index {
        documents: Vec<Document>,
    },
    GetIndex {},
    Ask {
        id: String,
        prompt: String,
    },
    Job {
        id: String,
    },
    Cancel {
        id: String,
    },
    ForgetJob {
        id: String,
    },
    DeleteSession {
        id: String,
    },
    Recent {},
    Jobs {},
    Handoff {
        ask_id: String,
        run: Option<HandoffRun>,
    },
    ManualHandoff {
        question: String,
        #[serde(default)]
        answer: String,
        run: Option<HandoffRun>,
    },
    ClearHandoff {},
    NewSession {
        id: String,
        lane: Lane,
    },
    Sessions {},
    Session {
        id: String,
    },
    Prompt {
        id: String,
        session: String,
        prompt: String,
    },
    FixPreview {
        kind: FixKind,
        component: String,
        evidence: String,
    },
    Fix {
        id: String,
        kind: FixKind,
        component: String,
        evidence: String,
        /// Echo the owner-reviewed preview token for external native evidence.
        #[serde(default)]
        native_evidence_consent: Option<String>,
    },
    Wire {
        instructions: String,
    },
    Unwire {},
    Usage {},
    Logs {
        component: String,
    },
    EnrollUsageSource {
        tool: crate::usage::Tool,
        path: String,
        soft_caps: Option<crate::usage::SoftCaps>,
    },
    CollectUsage {
        tool: crate::usage::Tool,
    },
    DisableUsageSource {
        tool: crate::usage::Tool,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffRun {
    pub id: String,
    pub session: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Ask,
    Agent,
    Chat,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixKind {
    Doctor,
    Tip,
    App,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub name: String,
    pub content: String,
    /// Hash of the exact UTF-8 document bytes; verified before index publication.
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: serde_json::Value },
    Error { error: crate::Error },
}
impl From<crate::Result<serde_json::Value>> for Response {
    fn from(value: crate::Result<serde_json::Value>) -> Self {
        match value {
            Ok(data) => Self::Ok { data },
            Err(error) => Self::Error { error },
        }
    }
}
