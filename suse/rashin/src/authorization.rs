//! Roles are assigned by the authenticated transport, never deserialized from
//! a model or request body. The owner may configure its own runtime. An
//! assistant connection receives only the explicitly enumerated read-only lane.
use crate::{Error, Result, api::Request};

#[derive(Clone, Copy, Debug)]
pub enum Access {
    Owner,
    AskClient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Inspect,
    Ask,
    Conversation,
    ProviderCredentials,
    Route,
    Index,
    Fix,
    AgentInstructions,
    UsageSource,
    UsageCollect,
}

impl Request {
    pub fn action(&self) -> Action {
        match self {
            Self::Status {}
            | Self::Providers {}
            | Self::ProwlStatus {}
            | Self::ReadProwl { .. }
            | Self::GetIndex {}
            | Self::Recent {}
            | Self::Jobs {}
            | Self::Sessions {}
            | Self::Session { .. }
            | Self::Usage {}
            | Self::Logs { .. }
            | Self::Job { .. } => Action::Inspect,
            Self::Ask { .. } | Self::Cancel { .. } => Action::Ask,
            Self::Handoff { .. }
            | Self::ClearHandoff {}
            | Self::NewSession { .. }
            | Self::Prompt { .. }
            | Self::ForgetJob { .. }
            | Self::DeleteSession { .. } => Action::Conversation,
            Self::PutProvider { .. }
            | Self::RefreshModels { .. }
            | Self::UpdateProvider { .. }
            | Self::RemoveProvider { .. }
            | Self::IssueAskClient { .. }
            | Self::AskClients {}
            | Self::RevokeAskClient { .. }
            | Self::ConfigureProwl { .. }
            | Self::DisableProwl {} => Action::ProviderCredentials,
            Self::SetRoute { .. } => Action::Route,
            Self::Index { .. } => Action::Index,
            Self::Fix { .. } | Self::FixPreview { .. } => Action::Fix,
            Self::Wire { .. } | Self::Unwire {} => Action::AgentInstructions,
            Self::EnrollUsageSource { .. } | Self::DisableUsageSource { .. } => Action::UsageSource,
            Self::CollectUsage { .. } => Action::UsageCollect,
        }
    }
}

pub fn authorize(access: Access, request: &Request) -> Result<()> {
    match (access, request.action()) {
        (Access::Owner, _) | (Access::AskClient, Action::Ask) => Ok(()),
        // A restricted client must not read the owner's Agent/Chat transcripts,
        // credential metadata or unrelated provider activity.
        (Access::AskClient, Action::Inspect)
            if matches!(
                request,
                Request::Status {}
                    | Request::GetIndex {}
                    | Request::Recent {}
                    | Request::Job { .. }
            ) =>
        {
            Ok(())
        }
        _ => Err(Error::Unauthorized),
    }
}
