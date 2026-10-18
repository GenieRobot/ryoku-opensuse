//! Versioned user-service protocol, durable operation journal, and narrow
//! Linux helper authorization primitives.
//!
//! This crate deliberately contains no command runner and accepts no caller
//! supplied executable, path, shell text, or provider arguments. Mutations are
//! rejected until an integration can supply verified linked-capture evidence.

#[cfg(test)]
pub(crate) fn test_unix_sockets_available() -> bool {
    let (mut stream, _peer) = match std::os::unix::net::UnixStream::pair() {
        Ok(pair) => pair,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return false,
        Err(error) => panic!("UnixStream::pair failed unexpectedly: {error}"),
    };
    if let Err(error) = std::io::Write::write_all(&mut stream, b"probe") {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return false;
        }
        panic!("UnixStream write probe failed: {error}");
    }
    match rustix::net::sockopt::socket_peercred(&stream) {
        Ok(_) => true,
        Err(error) if error == rustix::io::Errno::PERM => false,
        Err(error) => panic!("SO_PEERCRED probe failed unexpectedly: {error}"),
    }
}

#[cfg(test)]
#[macro_export]
macro_rules! test_unix_pair_or_skip {
    () => {{
        if !$crate::test_unix_sockets_available() {
            eprintln!("skipping Unix socket test: socket I/O or SO_PEERCRED denied by sandbox");
            return;
        }
        match std::os::unix::net::UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping Unix socket test: socketpair denied by sandbox");
                return;
            }
            Err(error) => panic!("UnixStream::pair failed unexpectedly: {error}"),
        }
    }};
}

pub mod account_state;
pub mod helper_protocol;
pub mod mutation_gate;
#[cfg(target_os = "linux")]
pub mod protected_state;
#[cfg(target_os = "linux")]
pub mod recovery_gate;
pub mod runtime;
mod runtime_paths;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, AsRawFd};

use rustix::net::sockopt::socket_peercred;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_JOURNAL_LINE_BYTES: usize = 16 * 1024;
const MUTATION_JOURNAL_VERSION: u16 = 2;
const MAX_OPERATION_ID_BYTES: usize = 96;

/// Opaque client correlation identifier with a bounded, printable alphabet.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if (1..=MAX_OPERATION_ID_BYTES).contains(&value.len())
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            Ok(Self(value))
        } else {
            Err(ProtocolError::Malformed("invalid request id"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestId {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Domain requests shared by UI, CLI, and the unprivileged service.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Discover,
    GetOperation { operation_id: OperationId },
    GetLocalProfile,
    SetLocalProfile { settings: ryoku_profiles::SettingsLayer },
    MigrateLegacyLocalProfile,
    Start { action: ManagedAction },
}

/// Only named product operations can cross the service boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedAction {
    RefreshCatalog,
    InstallPackage { package: PackageId },
    RemovePackage { package: PackageId },
    ApplyProfile { profile: ProfileId },
    CreateCheckpoint,
    RestoreCheckpoint { checkpoint: CheckpointId },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct PackageId(String);
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ProfileId(String);
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct CheckpointId(String);

macro_rules! domain_id {
    ($type:ty, $label:literal) => {
        impl $type {
            pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
                let value = value.into();
                if valid_token(&value, 96) {
                    Ok(Self(value))
                } else {
                    Err(ProtocolError::Malformed($label))
                }
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $type {
            type Error = ProtocolError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
    };
}

domain_id!(PackageId, "invalid package id");
domain_id!(ProfileId, "invalid profile id");
domain_id!(CheckpointId, "invalid checkpoint id");

fn valid_token(value: &str, max: usize) -> bool {
    (1..=max).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'+')
}

/// Durable operation identity. It is generated by the service, never supplied
/// by a client as an instruction to rerun an action.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if valid_token(&value, MAX_OPERATION_ID_BYTES) {
            Ok(Self(value))
        } else {
            Err(ProtocolError::Malformed("invalid operation id"))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OperationId {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u16,
    pub request_id: RequestId,
    pub request: Request,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ResponseBody {
    Discovery(Discovery),
    Operation(OperationStatus),
    LocalProfile(ryoku_profiles::ProfileDocument),
    Accepted { operation_id: OperationId },
    Error(ServiceError),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub version: u16,
    pub request_id: RequestId,
    pub body: ResponseBody,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    pub protocol_version: u16,
    pub service_state: ServiceState,
    pub supported_actions: Vec<ManagedActionKind>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Ready,
    RecoveryUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedActionKind {
    RefreshCatalog,
    InstallPackage,
    RemovePackage,
    ApplyProfile,
    CreateCheckpoint,
    RestoreCheckpoint,
}

impl From<&ManagedAction> for ManagedActionKind {
    fn from(action: &ManagedAction) -> Self {
        match action {
            ManagedAction::RefreshCatalog => Self::RefreshCatalog,
            ManagedAction::InstallPackage { .. } => Self::InstallPackage,
            ManagedAction::RemovePackage { .. } => Self::RemovePackage,
            ManagedAction::ApplyProfile { .. } => Self::ApplyProfile,
            ManagedAction::CreateCheckpoint => Self::CreateCheckpoint,
            ManagedAction::RestoreCheckpoint { .. } => Self::RestoreCheckpoint,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum ServiceError {
    Unsupported,
    Unauthorized,
    Malformed(String),
    StaleRequest,
    RecoveryUnavailable,
    Busy,
    NotFound,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationStatus {
    pub operation_id: OperationId,
    pub action: ManagedActionKind,
    pub phase: OperationPhase,
    pub progress: Option<Progress>,
    pub error: Option<ServiceError>,
}

impl OperationStatus {
    /// Whether the phase, progress, and error fields describe a valid
    /// reconnectable service state. Consumers must reject invalid remote
    /// statuses instead of presenting them as authoritative operation state.
    pub fn is_valid(&self) -> bool {
        if self.progress.as_ref().is_some_and(|progress| !progress.is_valid()) {
            return false;
        }
        match self.phase {
            OperationPhase::Accepted => self.progress.is_none() && self.error.is_none(),
            OperationPhase::Capturing | OperationPhase::Applying | OperationPhase::Verifying => {
                self.error.is_none()
            }
            OperationPhase::Succeeded => {
                self.error.is_none()
                    && self.progress.as_ref().is_none_or(|progress| {
                        progress.total.is_none_or(|total| progress.completed == total)
                    })
            }
            OperationPhase::Failed => self.error.is_some(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub completed: u32,
    pub total: Option<u32>,
}

impl Progress {
    fn is_valid(&self) -> bool {
        self.total.is_none_or(|total| self.completed <= total)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPhase {
    Accepted,
    Capturing,
    Applying,
    Verifying,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    Malformed(&'static str),
    UnsupportedVersion(u16),
    TooLarge,
    Io,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "malformed protocol message: {reason}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported protocol version {version}")
            }
            Self::TooLarge => f.write_str("protocol message exceeds configured size limit"),
            Self::Io => f.write_str("protocol I/O failed"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Reads one bounded newline-delimited JSON envelope and rejects unknown
/// versions/fields before it reaches service policy.
pub fn read_envelope(reader: &mut impl BufRead) -> Result<Envelope, ProtocolError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut limited = reader.take((MAX_REQUEST_BYTES + 2) as u64);
    limited.read_until(b'\n', &mut bytes).map_err(|_| ProtocolError::Io)?;
    if bytes.len() > MAX_REQUEST_BYTES + 1 {
        return Err(ProtocolError::TooLarge);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(ProtocolError::Malformed("incomplete request frame"));
    }
    bytes.pop();
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|_| ProtocolError::Malformed("invalid JSON envelope"))?;
    if envelope.version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(envelope.version));
    }
    Ok(envelope)
}

/// Helper requests are a deliberately smaller language than service requests.
/// There is no command string, executable, path, environment, or shell field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperRequest {
    /// Non-mutating availability probe used to connect the user daemon to the
    /// installed helper boundary while provider operations remain unavailable.
    CheckAvailability {
        action: ManagedActionKind,
    },
    VerifyLinkedCheckpoint {
        transaction_id: String,
        manifest_id: String,
    },
    ApplyPackageTransaction {
        transaction_id: String,
        manifest_id: String,
        packages: Vec<PackageId>,
    },
    RestoreCheckpoint {
        transaction_id: String,
        manifest_id: String,
        checkpoint_id: CheckpointId,
    },
}

impl HelperRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let (transaction, manifest) = match self {
            Self::CheckAvailability { .. } => return Ok(()),
            Self::VerifyLinkedCheckpoint { transaction_id, manifest_id }
            | Self::ApplyPackageTransaction { transaction_id, manifest_id, .. }
            | Self::RestoreCheckpoint { transaction_id, manifest_id, .. } => {
                (transaction_id, manifest_id)
            }
        };
        if !valid_token(transaction, 96) || !valid_token(manifest, 96) {
            return Err(ProtocolError::Malformed("invalid helper reference"));
        }
        if let Self::ApplyPackageTransaction { packages, .. } = self {
            if packages.is_empty() || packages.len() > 128 {
                return Err(ProtocolError::Malformed("invalid package list"));
            }
            let mut unique = packages.clone();
            unique.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            unique.dedup();
            if unique.len() != packages.len() {
                return Err(ProtocolError::Malformed("duplicate package id"));
            }
        }
        Ok(())
    }
}

/// Kernel-authenticated credentials of a connected Unix-domain socket peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    uid: u32,
    gid: u32,
    supplementary_gids: Vec<u32>,
    pid: Option<u32>,
}

impl PeerIdentity {
    pub fn from_unix_socket<T: AsFd>(socket: &T) -> Result<Self, ProtocolError> {
        let cred = socket_peercred(socket).map_err(|_| ProtocolError::Io)?;
        Ok(Self {
            uid: cred.uid.as_raw(),
            gid: cred.gid.as_raw(),
            supplementary_gids: socket_peer_groups(socket)?,
            pid: Some(cred.pid.as_raw_pid() as u32),
        })
    }
    pub fn uid(&self) -> u32 {
        self.uid
    }
    pub fn gid(&self) -> u32 {
        self.gid
    }
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

/// Read supplementary groups from the kernel's credentials snapshot for this
/// connected AF_UNIX peer. A fixed upper bound matches Linux NGROUPS_MAX and
/// keeps the getsockopt buffer bounded.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn socket_peer_groups<T: AsFd>(socket: &T) -> Result<Vec<u32>, ProtocolError> {
    use std::mem::size_of;

    const MAX_GROUPS: usize = 65_536;
    let mut groups = vec![0 as libc::gid_t; MAX_GROUPS];
    let mut length = libc::socklen_t::try_from(groups.len() * size_of::<libc::gid_t>())
        .map_err(|_| ProtocolError::Io)?;
    // SAFETY: `groups` is writable for `length` bytes; getsockopt receives a
    // valid connected socket fd and an initialized length pointer.
    let result = unsafe {
        libc::getsockopt(
            socket.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERGROUPS,
            groups.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(ProtocolError::Io);
    }
    let bytes = usize::try_from(length).map_err(|_| ProtocolError::Io)?;
    let capacity = groups.len() * size_of::<libc::gid_t>();
    if bytes > capacity || bytes % size_of::<libc::gid_t>() != 0 {
        return Err(ProtocolError::Io);
    }
    groups.truncate(bytes / size_of::<libc::gid_t>());
    Ok(groups)
}

#[cfg(not(target_os = "linux"))]
fn socket_peer_groups<T: AsFd>(_socket: &T) -> Result<Vec<u32>, ProtocolError> {
    Err(ProtocolError::Io)
}

/// The helper may only use the dedicated service identity for this legacy
/// authorization path. Ordinary operator access is checked separately using
/// the explicit `ryoku-operators` group policy.
pub fn authorize_helper_peer(peer: &PeerIdentity, service_uid: u32) -> Result<(), ServiceError> {
    if service_uid != 0 && peer.uid == service_uid {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}

/// The privileged helper only trusts non-root peers whose primary or
/// supplementary group set contains the installed `ryoku-operators` GID.
pub fn authorize_helper_operator(
    peer: &PeerIdentity,
    operator_gid: u32,
) -> Result<(), ServiceError> {
    if peer.uid != 0
        && (peer.gid == operator_gid || peer.supplementary_gids.contains(&operator_gid))
    {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}

/// Resolve the opt-in operator group from the local system group database.
/// The RPM sysusers declaration ensures this entry exists before activation.
pub fn operator_group_gid() -> Result<u32, ProtocolError> {
    let contents = std::fs::read_to_string("/etc/group").map_err(|_| ProtocolError::Io)?;
    parse_operator_group_gid(&contents)
}

fn parse_operator_group_gid(contents: &str) -> Result<u32, ProtocolError> {
    let mut found = None;
    for line in contents.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let mut fields = line.split(':');
        if fields.next() != Some("ryoku-operators") {
            continue;
        }
        let _password = fields.next().ok_or(ProtocolError::Malformed("invalid operator group"))?;
        let gid = fields
            .next()
            .ok_or(ProtocolError::Malformed("invalid operator group"))?
            .parse::<u32>()
            .map_err(|_| ProtocolError::Malformed("invalid operator group"))?;
        if gid == 0 {
            return Err(ProtocolError::Malformed("invalid operator group"));
        }
        let _members = fields.next().ok_or(ProtocolError::Malformed("invalid operator group"))?;
        if fields.next().is_some() {
            return Err(ProtocolError::Malformed("invalid operator group"));
        }
        if found.replace(gid).is_some() {
            return Err(ProtocolError::Malformed("duplicate operator group"));
        }
    }
    found.ok_or(ProtocolError::Malformed("operator group is not installed"))
}

/// Append-only operation state journal. Construct it from a root/service-owned
/// file descriptor whose path, owner, and mode were checked by the installer.
/// Each record is written and synchronized before updating the in-memory view.
pub struct OperationJournal {
    file: File,
    current: BTreeMap<OperationId, OperationStatus>,
    write_failed: bool,
    mutations: BTreeMap<OperationId, mutation_gate::MutationContext>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalRecord {
    version: u16,
    status: OperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mutation: Option<mutation_gate::MutationContext>,
}

impl OperationJournal {
    pub fn open(file: File) -> Result<Self, JournalError> {
        Self::open_with_sync(file, File::sync_all)
    }

    fn open_with_sync(
        file: File,
        mut sync: impl FnMut(&File) -> std::io::Result<()>,
    ) -> Result<Self, JournalError> {
        use std::io::{Seek, SeekFrom};
        let mut current = BTreeMap::new();
        let mut mutations = BTreeMap::new();
        let mut reader = BufReader::new(file.try_clone().map_err(|_| JournalError::Io)?);
        reader.seek(SeekFrom::Start(0)).map_err(|_| JournalError::Io)?;
        let mut line = Vec::new();
        let mut valid_len = 0u64;
        loop {
            line.clear();
            let count = reader
                .by_ref()
                .take((MAX_JOURNAL_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|_| JournalError::Io)?;
            if count == 0 {
                break;
            }
            if line.len() > MAX_JOURNAL_LINE_BYTES {
                return Err(JournalError::Corrupt);
            }
            if line.last() != Some(&b'\n') {
                file.set_len(valid_len).map_err(|_| JournalError::Io)?;
                break;
            }
            let record: JournalRecord = serde_json::from_slice(&line[..line.len() - 1])
                .map_err(|_| JournalError::Corrupt)?;
            if !matches!(record.version, PROTOCOL_VERSION | MUTATION_JOURNAL_VERSION) {
                return Err(JournalError::UnsupportedVersion(record.version));
            }
            if (record.version == MUTATION_JOURNAL_VERSION) != record.mutation.is_some() {
                return Err(JournalError::Corrupt);
            }
            validate_journal_record(
                current.get(&record.status.operation_id),
                mutations.get(&record.status.operation_id),
                &record.status,
                record.mutation.as_ref(),
            )?;
            if let Some(context) = record.mutation {
                mutations.insert(record.status.operation_id.clone(), context);
            }
            current.insert(record.status.operation_id.clone(), record.status);
            valid_len = valid_len.saturating_add(count as u64);
        }
        // Readable complete records may still be in the page cache after an
        // earlier failed sync. Establish durability before publishing replayed
        // state, including on process restart and after truncating a torn tail.
        sync(&file).map_err(|_| JournalError::Io)?;
        Ok(Self { file, current, mutations, write_failed: false })
    }

    pub fn get(&self, id: &OperationId) -> Option<&OperationStatus> {
        self.current.get(id)
    }

    pub(crate) fn refresh_after_write_failure(&mut self) -> Result<(), JournalError> {
        self.refresh_with_sync(File::sync_all)
    }

    fn refresh_with_sync(
        &mut self,
        sync: impl FnMut(&File) -> std::io::Result<()>,
    ) -> Result<(), JournalError> {
        if self.write_failed {
            *self =
                Self::open_with_sync(self.file.try_clone().map_err(|_| JournalError::Io)?, sync)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn simulate_uncertain_write_failure(&mut self) {
        self.write_failed = true;
    }
    /// Detailed historical association; this never authorizes replay/apply.
    pub fn mutation_context(&self, id: &OperationId) -> Option<&mutation_gate::MutationContext> {
        self.mutations.get(id)
    }

    /// Reopen under the existing coordinator lock before accepting new work.
    /// This refresh prevents a process from using a stale pre-lock journal view.
    pub(crate) fn refresh(&mut self) -> Result<(), JournalError> {
        *self = Self::open(self.file.try_clone().map_err(|_| JournalError::Io)?)?;
        Ok(())
    }

    /// Unfinished work and failures that may have changed state require explicit
    /// reconciliation. Legacy records retain their status but provide no missing
    /// checkpoint proof; legacy failed operations are conservatively unresolved.
    pub fn recovery_required(&self) -> Vec<&OperationStatus> {
        self.current
            .values()
            .filter(|status| match status.phase {
                OperationPhase::Succeeded => false,
                OperationPhase::Failed => !matches!(
                    self.mutations.get(&status.operation_id).and_then(|context| context.outcome),
                    Some(
                        mutation_gate::MutationOutcome::FailedBeforeChange
                            | mutation_gate::MutationOutcome::CancelledBeforeChange
                    )
                ),
                _ => true,
            })
            .collect()
    }

    pub fn record(&mut self, status: OperationStatus) -> Result<(), JournalError> {
        self.record_with_sync(status, File::sync_all)
    }

    pub(crate) fn record_mutation(
        &mut self,
        status: OperationStatus,
        context: mutation_gate::MutationContext,
    ) -> Result<(), JournalError> {
        self.record_inner(status, Some(context), File::sync_all)
    }

    fn record_with_sync(
        &mut self,
        status: OperationStatus,
        sync: impl FnMut(&File) -> std::io::Result<()>,
    ) -> Result<(), JournalError> {
        self.record_inner(status, None, sync)
    }

    fn record_inner(
        &mut self,
        status: OperationStatus,
        mutation: Option<mutation_gate::MutationContext>,
        mut sync: impl FnMut(&File) -> std::io::Result<()>,
    ) -> Result<(), JournalError> {
        if self.write_failed {
            return Err(JournalError::Io);
        }
        validate_journal_record(
            self.current.get(&status.operation_id),
            self.mutations.get(&status.operation_id),
            &status,
            mutation.as_ref(),
        )?;
        let mut record = serde_json::to_vec(&JournalRecord {
            version: if mutation.is_some() { MUTATION_JOURNAL_VERSION } else { PROTOCOL_VERSION },
            status: status.clone(),
            mutation: mutation.clone(),
        })
        .map_err(|_| JournalError::Io)?;
        if record.len() + 1 > MAX_JOURNAL_LINE_BYTES {
            return Err(JournalError::Corrupt);
        }
        record.push(b'\n');
        use std::io::{Seek, SeekFrom};
        self.file.seek(SeekFrom::End(0)).map_err(|_| JournalError::Io)?;
        if self.file.write_all(&record).is_err() || sync(&self.file).is_err() {
            self.write_failed = true;
            return Err(JournalError::Io);
        }
        if let Some(context) = mutation {
            self.mutations.insert(status.operation_id.clone(), context);
        }
        self.current.insert(status.operation_id.clone(), status);
        Ok(())
    }
}

fn validate_journal_record(
    previous: Option<&OperationStatus>,
    old_context: Option<&mutation_gate::MutationContext>,
    next: &OperationStatus,
    context: Option<&mutation_gate::MutationContext>,
) -> Result<(), JournalError> {
    let valid = match (context, old_context, previous) {
        (None, None, _) => return validate_transition(previous, next),
        (Some(context), None, None) => context.initial() && context.valid_for(next),
        (Some(context), Some(old_context), Some(previous)) => {
            previous.action == next.action
                && context.valid_for(next)
                && context.follows(old_context)
        }
        _ => false,
    };
    if valid { Ok(()) } else { Err(JournalError::InvalidTransition) }
}

fn validate_transition(
    previous: Option<&OperationStatus>,
    next: &OperationStatus,
) -> Result<(), JournalError> {
    if !next.is_valid() {
        return Err(JournalError::InvalidTransition);
    }
    let valid = match previous {
        None => {
            next.phase == OperationPhase::Accepted
                && next.progress.is_none()
                && next.error.is_none()
        }
        Some(old) if old.action != next.action => false,
        Some(old) if matches!(old.phase, OperationPhase::Succeeded | OperationPhase::Failed) => {
            false
        }
        Some(old)
            if old.phase == next.phase
                && matches!(
                    old.phase,
                    OperationPhase::Capturing
                        | OperationPhase::Applying
                        | OperationPhase::Verifying
                ) =>
        {
            next.is_valid() && valid_progress_update(old.progress.as_ref(), next.progress.as_ref())
        }
        Some(old)
            if old.phase == OperationPhase::Verifying
                && next.phase == OperationPhase::Succeeded =>
        {
            next.is_valid()
                && old.progress.as_ref().is_none_or(|progress| {
                    progress.total.is_none_or(|total| progress.completed == total)
                })
        }
        Some(old) => matches!(
            (old.phase, next.phase),
            (OperationPhase::Accepted, OperationPhase::Capturing | OperationPhase::Failed)
                | (OperationPhase::Capturing, OperationPhase::Applying | OperationPhase::Failed)
                | (OperationPhase::Applying, OperationPhase::Verifying | OperationPhase::Failed)
                | (OperationPhase::Verifying, OperationPhase::Succeeded | OperationPhase::Failed)
        ),
    };
    if valid { Ok(()) } else { Err(JournalError::InvalidTransition) }
}

/// Progress may advance within an active phase, but cannot disappear, regress,
/// or change a previously declared total. An unknown total may become known.
fn valid_progress_update(previous: Option<&Progress>, next: Option<&Progress>) -> bool {
    match (previous, next) {
        (None, Some(_)) => true,
        (Some(previous), Some(next)) => {
            next != previous
                && next.completed >= previous.completed
                && match (previous.total, next.total) {
                    (Some(previous), Some(next)) => previous == next,
                    (Some(_), None) => false,
                    (None, _) => true,
                }
        }
        (None, None) | (Some(_), None) => false,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalError {
    Io,
    Corrupt,
    UnsupportedVersion(u16),
    InvalidTransition,
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "operation journal error: {self:?}")
    }
}
impl std::error::Error for JournalError {}

/// Current implementation capabilities. No write is advertised because no
/// production executor can yet prove the required linked pre-change capture.
pub fn discovery(recovery_available: bool) -> Discovery {
    Discovery {
        protocol_version: PROTOCOL_VERSION,
        service_state: if recovery_available {
            ServiceState::Ready
        } else {
            ServiceState::RecoveryUnavailable
        },
        supported_actions: Vec::new(),
    }
}

/// Classifies actions against this build's explicitly enabled allowlist.
pub fn action_available(_action: &ManagedAction) -> bool {
    false
}

/// Helper dispatcher policy. Known bounded operations are still refused until
/// a backend integrates #16's verified manifest/capture and provider path.
pub fn helper_action_available(_request: &HelperRequest) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn versioned_request_round_trips_and_rejects_unknown_fields() {
        let env = Envelope {
            version: 1,
            request_id: RequestId::new("req-1").unwrap(),
            request: Request::Discover,
        };
        let mut json = serde_json::to_vec(&env).unwrap();
        json.push(b'\n');
        assert_eq!(read_envelope(&mut json.as_slice()).unwrap(), env);
        let bad = br#"{"version":1,"request_id":"r","request":{"type":"discover"},"extra":true}
"#;
        assert!(read_envelope(&mut bad.as_slice()).is_err());
        let malformed_id =
            br#"{"version":1,"request_id":"../outside","request":{"type":"discover"}}
"#;
        assert!(read_envelope(&mut malformed_id.as_slice()).is_err());
        let malformed_package = br#"{"version":1,"request_id":"r","request":{"type":"start","data":{"action":{"type":"install_package","data":{"package":"../outside"}}}}}
"#;
        assert!(read_envelope(&mut malformed_package.as_slice()).is_err());
    }

    #[test]
    fn rejects_unknown_protocol_version_and_oversized_message() {
        let version = br#"{"version":9,"request_id":"r","request":{"type":"discover"}}
"#;
        assert!(matches!(
            read_envelope(&mut version.as_slice()),
            Err(ProtocolError::UnsupportedVersion(9))
        ));
        let mut too_big = vec![b'x'; MAX_REQUEST_BYTES + 2];
        *too_big.last_mut().unwrap() = b'\n';
        assert_eq!(read_envelope(&mut too_big.as_slice()), Err(ProtocolError::TooLarge));
    }

    #[test]
    fn helper_language_has_no_arbitrary_command_fields_and_validates_ids() {
        let request = HelperRequest::ApplyPackageTransaction {
            transaction_id: "txn-1".into(),
            manifest_id: "manifest-1".into(),
            packages: vec![PackageId::new("firefox").unwrap()],
        };
        assert!(request.validate().is_ok());
        let bad = HelperRequest::VerifyLinkedCheckpoint {
            transaction_id: "../escape".into(),
            manifest_id: "m".into(),
        };
        assert!(bad.validate().is_err());
        assert!(!helper_action_available(&request));
    }

    #[test]
    fn peer_auth_uses_kernel_credentials_and_rejects_root_identity_config() {
        let (a, _b) = UnixStream::pair().unwrap();
        let peer = PeerIdentity::from_unix_socket(&a).unwrap();
        assert_eq!(peer.uid(), rustix::process::geteuid().as_raw());
        assert!(authorize_helper_peer(&peer, peer.uid()).is_ok());
        assert_eq!(authorize_helper_peer(&peer, 0), Err(ServiceError::Unauthorized));
    }

    #[test]
    fn journal_reconnect_returns_last_durable_operation_state() {
        let path = std::env::temp_dir().join(format!("ryoku-journal-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let id = OperationId::new("operation-1").unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let accepted = OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        journal.record(accepted).unwrap();
        let capturing = OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Capturing,
            progress: Some(Progress { completed: 0, total: Some(1) }),
            error: None,
        };
        journal.record(capturing.clone()).unwrap();
        drop(journal);
        let reopened = OperationJournal::open(
            std::fs::OpenOptions::new().read(true).append(true).open(&path).unwrap(),
        )
        .unwrap();
        assert_eq!(reopened.get(&id), Some(&capturing));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journal_refuses_appends_until_replay_after_uncertain_write() {
        let path =
            std::env::temp_dir().join(format!("ryoku-journal-poison-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let durable_id = OperationId::new("durable-before-error").unwrap();
        let durable_status = OperationStatus {
            operation_id: durable_id.clone(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        journal.record(durable_status.clone()).unwrap();
        let valid_len = std::fs::metadata(&path).unwrap().len();
        // Model the bytes left behind when a previous append failed midway.
        let mut interrupted = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        interrupted.write_all(b"{\"version\":1,\"status\":").unwrap();
        interrupted.sync_all().unwrap();
        drop(interrupted);
        journal.write_failed = true;
        let status = OperationStatus {
            operation_id: OperationId::new("after-uncertain-write").unwrap(),
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        assert_eq!(journal.record(status.clone()), Err(JournalError::Io));
        let torn_len = std::fs::metadata(&path).unwrap().len();
        assert!(torn_len > 0);

        journal.refresh_after_write_failure().unwrap();
        assert!(!journal.write_failed);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_len);
        assert_eq!(journal.get(&durable_id), Some(&durable_status));
        journal.record(status).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn complete_record_after_failed_sync_requires_successful_sync_before_replay() {
        let path = std::env::temp_dir().join(format!("ryoku-sync-failure-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let accepted = OperationStatus {
            operation_id: OperationId::new("uncertain-sync").unwrap(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        journal.record(accepted.clone()).unwrap();
        let capturing = OperationStatus { phase: OperationPhase::Capturing, ..accepted.clone() };
        let fail_sync = |_: &File| Err(std::io::Error::other("injected sync failure"));
        assert_eq!(journal.record_with_sync(capturing.clone(), fail_sync), Err(JournalError::Io));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(journal.get(&accepted.operation_id), Some(&accepted));
        for _ in 0..2 {
            assert_eq!(journal.refresh_with_sync(fail_sync), Err(JournalError::Io));
            assert!(journal.write_failed);
            assert_eq!(journal.get(&accepted.operation_id), Some(&accepted));
            assert_eq!(journal.record(capturing.clone()), Err(JournalError::Io));
        }
        // A fresh process must also establish durability, not trust readable bytes.
        assert!(matches!(
            OperationJournal::open_with_sync(File::open(&path).unwrap(), fail_sync),
            Err(JournalError::Io)
        ));
        journal.refresh_with_sync(File::sync_all).unwrap();
        assert!(!journal.write_failed);
        assert_eq!(journal.get(&accepted.operation_id), Some(&capturing));
        journal.record(OperationStatus { phase: OperationPhase::Applying, ..capturing }).unwrap();
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_append_poison_is_set_by_actual_io_error() {
        let path =
            std::env::temp_dir().join(format!("ryoku-readonly-journal-{}", std::process::id()));
        std::fs::write(&path, []).unwrap();
        let mut journal = OperationJournal::open(File::open(&path).unwrap()).unwrap();
        let status = OperationStatus {
            operation_id: OperationId::new("cannot-write").unwrap(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        assert_eq!(journal.record(status.clone()), Err(JournalError::Io));
        assert!(journal.write_failed);
        assert_eq!(journal.get(&status.operation_id), None);
        assert_eq!(journal.record(status), Err(JournalError::Io));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journal_rejects_regressing_progress_without_changing_disk() {
        let path = std::env::temp_dir().join(format!("ryoku-progress-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let mut status = OperationStatus {
            operation_id: OperationId::new("progress").unwrap(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        journal.record(status.clone()).unwrap();
        status.phase = OperationPhase::Capturing;
        status.progress = Some(Progress { completed: 1, total: Some(3) });
        journal.record(status.clone()).unwrap();
        let length = std::fs::metadata(&path).unwrap().len();
        for progress in [
            None,
            Some(Progress { completed: 0, total: Some(3) }),
            Some(Progress { completed: 2, total: Some(4) }),
            Some(Progress { completed: 2, total: None }),
        ] {
            assert_eq!(
                journal.record(OperationStatus { progress, ..status.clone() }),
                Err(JournalError::InvalidTransition)
            );
        }
        assert_eq!(std::fs::metadata(&path).unwrap().len(), length);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journal_rejects_success_when_the_last_verification_total_is_incomplete() {
        let id = OperationId::new("incomplete-final-progress").unwrap();
        let verifying = OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::RestoreCheckpoint,
            phase: OperationPhase::Verifying,
            progress: Some(Progress { completed: 2, total: Some(3) }),
            error: None,
        };
        let succeeded = OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::RestoreCheckpoint,
            phase: OperationPhase::Succeeded,
            progress: None,
            error: None,
        };
        assert_eq!(
            validate_transition(Some(&verifying), &succeeded),
            Err(JournalError::InvalidTransition)
        );

        let completed = OperationStatus {
            progress: Some(Progress { completed: 3, total: Some(3) }),
            ..verifying.clone()
        };
        assert_eq!(validate_transition(Some(&completed), &succeeded), Ok(()));
    }

    #[test]
    fn journal_rejects_skipped_or_terminal_transitions() {
        let path =
            std::env::temp_dir().join(format!("ryoku-journal-invalid-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let invalid = OperationStatus {
            operation_id: OperationId::new("op").unwrap(),
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Succeeded,
            progress: None,
            error: None,
        };
        assert_eq!(journal.record(invalid), Err(JournalError::InvalidTransition));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journal_rejects_impossible_progress_and_phase_error_combinations() {
        let path = std::env::temp_dir()
            .join(format!("ryoku-journal-status-invariants-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let id = OperationId::new("invariant-op").unwrap();
        journal
            .record(OperationStatus {
                operation_id: id.clone(),
                action: ManagedActionKind::InstallPackage,
                phase: OperationPhase::Accepted,
                progress: None,
                error: None,
            })
            .unwrap();
        let accepted_record_len = std::fs::metadata(&path).unwrap().len();

        let impossible_progress = OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Capturing,
            progress: Some(Progress { completed: 2, total: Some(1) }),
            error: None,
        };
        assert_eq!(journal.record(impossible_progress), Err(JournalError::InvalidTransition));

        let missing_failure_reason = OperationStatus {
            operation_id: id,
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Failed,
            progress: None,
            error: None,
        };
        assert_eq!(journal.record(missing_failure_reason), Err(JournalError::InvalidTransition));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), accepted_record_len);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn journal_discards_only_a_torn_final_record() {
        use std::io::Write as _;
        let path = std::env::temp_dir().join(format!("ryoku-journal-torn-{}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let id = OperationId::new("op").unwrap();
        let status = OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::InstallPackage,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        writeln!(
            file,
            "{}",
            serde_json::to_string(&JournalRecord {
                version: 1,
                status: status.clone(),
                mutation: None
            })
            .unwrap()
        )
        .unwrap();
        file.write_all(b"{\"version\":1,\"status\":").unwrap();
        file.sync_all().unwrap();
        let journal = OperationJournal::open(file).unwrap();
        assert_eq!(journal.get(&id), Some(&status));
        drop(journal);
        let persisted = std::fs::read(&path).unwrap();
        assert!(persisted.ends_with(b"\n"));
        assert!(!persisted.ends_with(b"status\":"));
        std::fs::remove_file(path).unwrap();
    }
}
