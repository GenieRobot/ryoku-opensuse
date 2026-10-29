//! Authenticated HTTP metadata and immutable remote-request/result receipts.
//! These receipts grant no local mutation or linked-checkpoint authority.
use crate::{PeerIdentity, RequestId, ServiceError, authorize_helper_operator};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use ryoku_recovery::{
    AccountPrincipal, Identifier, Sha256,
    installed_capture::CaptureAccount,
    ryogami_sources_protocol::{
        MAX_METADATA_RESULT_BYTES, PreparedSourceMetadataQuery, SourceMetadataFailure,
        SourceMetadataItem, SourceMetadataQuery, SourceMetadataResponse,
    },
    wallpaper_protocol::{ActorIdentity, digest},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const MAX_REQUEST: usize = 64 * 1024;
const MAX_RESULT: usize = MAX_METADATA_RESULT_BYTES + MAX_REQUEST + 4096;
const MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STORE_FILES: usize = 4096;
const MAX_PUBLIC_REPLY: usize = 12 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

pub use ryoku_recovery::ryogami_sources_protocol::SourceMetadataCursorQuery as MetadataCursorQuery;

pub fn metadata(
    peer: &PeerIdentity,
    operator_gid: u32,
    query: SourceMetadataQuery,
    rpc_id: i64,
    correlation: &RequestId,
) -> Result<Vec<u8>, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    if rustix::process::geteuid().as_raw() != 0 || rpc_id < 0 || query.validate().is_err() {
        return Err(ServiceError::Unauthorized);
    }
    let account = crate::helper_protocol::prepare_operator_account(peer, operator_gid)?;
    let home = account.capture_home().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let capture = account.capture_account().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let settings = crate::ryogami_paths::settings(&home, &capture)
        .map_err(|_| ServiceError::Malformed("source_settings_unavailable".into()))?;
    let prepared = PreparedSourceMetadataQuery {
        expected_settings: settings.as_ref().map(|bytes| hash(bytes)),
        query,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    let id = Identifier::new(format!(
        "source-{}-{}-{}-{}",
        now.as_secs(),
        now.subsec_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
    .map_err(|_| ServiceError::RecoveryUnavailable)?;
    let directory = protected_directory()?;
    let store = ReceiptStore::open(&directory, &capture, 0)?;
    let request = store.prepare(RequestRecord {
        version: 1,
        account: Binding::from(&capture),
        request_id: id,
        client_correlation: correlation.as_str().into(),
        created_unix: now.as_secs(),
        rpc_id,
        query: prepared.clone(),
    })?;
    // This durable request exists before any provider dispatch. Missing final
    // data after a crash truthfully leaves the remote outcome unconfirmed.
    request.revalidate(&store)?;
    account.revalidate().map_err(|_| ServiceError::StaleRequest)?;
    let outcome = match ryoku_recovery::writer_protocol::read_source_metadata(
        &capture,
        &home,
        request.record.request_id.clone(),
        prepared,
        rpc_id,
    ) {
        Ok(read) => Outcome::Responded { actor: read.actor, response: read.response },
        Err(_) => Outcome::TransportUnavailable,
    };
    let saved = store.complete(&request, outcome)?;
    account.revalidate().map_err(|_| ServiceError::StaleRequest)?;
    saved.page(rpc_id, 0)
}

pub fn page(
    peer: &PeerIdentity,
    operator_gid: u32,
    query: MetadataCursorQuery,
    rpc_id: i64,
) -> Result<Vec<u8>, ServiceError> {
    authorize_helper_operator(peer, operator_gid)?;
    if rustix::process::geteuid().as_raw() != 0 || rpc_id < 0 {
        return Err(ServiceError::Unauthorized);
    }
    let account = crate::helper_protocol::prepare_operator_account(peer, operator_gid)?;
    let capture = account.capture_account().map_err(|_| ServiceError::RecoveryUnavailable)?;
    let directory = protected_directory()?;
    let store = ReceiptStore::open(&directory, &capture, 0)?;
    let (saved, offset) = store.cursor(&query.cursor)?;
    account.revalidate().map_err(|_| ServiceError::StaleRequest)?;
    saved.page(rpc_id, offset)
}

fn protected_directory() -> Result<File, ServiceError> {
    let history = crate::native_mutation::ProtectedMutationHistory::open()
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
    history.directory().try_clone().map_err(|_| ServiceError::RecoveryUnavailable)
}
fn hash(bytes: &[u8]) -> Sha256 {
    Sha256::new(digest(bytes)).expect("SHA-256 encoding")
}
fn unavailable<T>() -> Result<T, ServiceError> {
    Err(ServiceError::RecoveryUnavailable)
}
fn private(
    file: &File,
    owner: u32,
    group: u32,
    directory: bool,
    mode: u32,
) -> Result<(), ServiceError> {
    let stat = fs::fstat(file).map_err(|_| ServiceError::RecoveryUnavailable)?;
    if stat.st_uid != owner
        || stat.st_gid != group
        || stat.st_mode & 0o7777 != mode
        || FileType::from_raw_mode(stat.st_mode)
            != if directory { FileType::Directory } else { FileType::RegularFile }
        || !directory && stat.st_nlink != 1
    {
        return unavailable();
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    principal: AccountPrincipal,
    uid: u32,
    gid: u32,
}
impl From<&CaptureAccount> for Binding {
    fn from(account: &CaptureAccount) -> Self {
        Self { principal: account.principal.clone(), uid: account.uid, gid: account.gid }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    version: u16,
    account: Binding,
    request_id: Identifier,
    client_correlation: String,
    created_unix: u64,
    rpc_id: i64,
    query: PreparedSourceMetadataQuery,
}
impl RequestRecord {
    fn token(&self) -> Result<String, ServiceError> {
        let bytes =
            serde_json::to_vec(&("ryoku-source-request-v1", &self.account, &self.request_id))
                .map_err(|_| ServiceError::RecoveryUnavailable)?;
        Ok(digest(&bytes)[..40].into())
    }
    fn validate(&self, account: &Binding) -> Result<(), ServiceError> {
        if self.version != 1
            || &self.account != account
            || self.account.uid == 0
            || self.rpc_id < 0
            || self.query.validate().is_err()
            || RequestId::new(&self.client_correlation).is_err()
        {
            return unavailable();
        }
        Ok(())
    }
}
struct PreparedRequest {
    record: RequestRecord,
    sha256: Sha256,
    token: String,
}
impl PreparedRequest {
    fn revalidate(&self, store: &ReceiptStore) -> Result<(), ServiceError> {
        let bytes = store.read(&format!("q-{}.request.json", self.token), MAX_REQUEST)?;
        if hash(&bytes) != self.sha256 {
            return unavailable();
        }
        let record: RequestRecord =
            serde_json::from_slice(&bytes).map_err(|_| ServiceError::RecoveryUnavailable)?;
        record.validate(&store.account)?;
        if record != self.record {
            return unavailable();
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Outcome {
    Responded { actor: ActorIdentity, response: SourceMetadataResponse },
    TransportUnavailable,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultRecord {
    version: u16,
    request_sha256: Sha256,
    request: RequestRecord,
    outcome: Outcome,
}
struct SavedMetadata {
    record: ResultRecord,
    sha256: Sha256,
    token: String,
}
impl SavedMetadata {
    fn validate(&self, account: &Binding) -> Result<(), ServiceError> {
        self.record.request.validate(account)?;
        if self.record.version != 1 || self.record.request.token()? != self.token {
            return unavailable();
        }
        if let Outcome::Responded { actor, response } = &self.record.outcome {
            if actor.pid == 0 || actor.start_time == 0 {
                return unavailable();
            }
            response
                .validate_for_query(
                    self.record.request.rpc_id,
                    &self.record.request.request_id,
                    &self.record.request.query.query,
                )
                .map_err(|_| ServiceError::RecoveryUnavailable)?;
        }
        Ok(())
    }
    // Tokens are account-scoped record/index locators and data bindings. They
    // are never bearer authorization or permission to fetch a stored URL.
    fn locator(&self, index: usize, purpose: &str) -> Result<Sha256, ServiceError> {
        if index > 256 {
            return unavailable();
        }
        let bound = serde_json::to_vec(&(purpose, &self.sha256, index))
            .map_err(|_| ServiceError::RecoveryUnavailable)?;
        Sha256::new(format!("{}{:04x}{}", self.token, index, &digest(&bound)[..20]))
            .map_err(|_| ServiceError::RecoveryUnavailable)
    }
    fn offset(&self, locator: &Sha256, purpose: &str) -> Result<usize, ServiceError> {
        let value = locator.as_str();
        let index =
            usize::from_str_radix(&value[40..44], 16).map_err(|_| ServiceError::StaleRequest)?;
        if self.locator(index, purpose)?.as_str() != value {
            return Err(ServiceError::StaleRequest);
        }
        Ok(index)
    }
    fn item(&self, locator: &Sha256) -> Result<&SourceMetadataItem, ServiceError> {
        let index = self.offset(locator, "ryoku-source-item-v1")?;
        let Outcome::Responded { response, .. } = &self.record.outcome else {
            return unavailable();
        };
        response
            .result
            .as_ref()
            .and_then(|page| page.results.get(index))
            .ok_or(ServiceError::StaleRequest)
    }
    fn page(&self, id: i64, offset: usize) -> Result<Vec<u8>, ServiceError> {
        if id < 0 {
            return Err(ServiceError::StaleRequest);
        }
        let source = self.record.request.query.query.source;
        let error = match &self.record.outcome {
            Outcome::TransportUnavailable=>Some("Source request outcome is unconfirmed; retry after checking the source connection"),
            Outcome::Responded{response,..}=>response.error.map(|error|match error {
                SourceMetadataFailure::Configuration=>"Source settings changed or are unavailable; review the configured source and retry",
                SourceMetadataFailure::Disabled=>"This source is disabled; enable it in source settings",
                SourceMetadataFailure::MissingCredentials=>"This source requires credentials; configure its access key",
                SourceMetadataFailure::ProviderUnavailable=>"Source request failed; check the connection and configured credentials, then retry",
                SourceMetadataFailure::Timeout=>"Source search timed out; retry the request",
                SourceMetadataFailure::InvalidResponse=>"Source returned invalid or oversized metadata; retry the request",
            }),
        };
        if let Some(message) = error {
            return serde_json::to_vec(&serde_json::json!({"id":id,"error":{"code":-32036,"message":message,"data":{"request_id":self.record.request.request_id,"source":source}}}))
                .map(|mut bytes|{bytes.push(b'\n');bytes}).map_err(|_|ServiceError::RecoveryUnavailable);
        }
        let Outcome::Responded { response, .. } = &self.record.outcome else {
            return unavailable();
        };
        let data = response.result.as_ref().ok_or(ServiceError::RecoveryUnavailable)?;
        if offset > data.results.len() {
            return Err(ServiceError::StaleRequest);
        }
        let mut rows = Vec::new();
        let make_reply =
            |rows: &Vec<serde_json::Value>, next: usize| -> Result<Vec<u8>, ServiceError> {
                let cursor = if next < data.results.len() {
                    Some(self.locator(next, "ryoku-source-page-v1")?)
                } else {
                    None
                };
                let mut bytes = serde_json::to_vec(&serde_json::json!({"id":id,"result":{
                    "source":source,"request_id":self.record.request.request_id,"results":rows,
                    "lastPage":data.last_page,"currentPage":data.current_page,
                    "generation":self.record.request.query.query.search.generation,
                    "total":data.results.len(),"next_cursor":cursor
                }}))
                .map_err(|_| ServiceError::RecoveryUnavailable)?;
                bytes.push(b'\n');
                Ok(bytes)
            };
        for index in offset..data.results.len().min(offset.saturating_add(32)) {
            let selection = self.locator(index, "ryoku-source-item-v1")?;
            let item = self.item(&selection)?;
            rows.push(serde_json::json!({"id":item.id,"selection":selection,
                "title":item.title,"resolution":item.resolution,"fileSize":item.file_size,
                "attribution":item.attribution,"durationSecs":item.duration_secs,
                "purity":item.purity,"category":item.category}));
            if make_reply(&rows, index + 1)?.len() > MAX_PUBLIC_REPLY {
                rows.pop();
                break;
            }
        }
        if rows.is_empty() && offset < data.results.len() {
            return unavailable();
        }
        let bytes = make_reply(&rows, offset + rows.len())?;
        if bytes.len() > MAX_PUBLIC_REPLY {
            return unavailable();
        }
        Ok(bytes)
    }
}

struct ReceiptStore {
    directory: File,
    lock: File,
    account: Binding,
    owner: u32,
    group: u32,
}
impl ReceiptStore {
    fn open(base: &File, account: &CaptureAccount, owner: u32) -> Result<Self, ServiceError> {
        let group = if owner == 0 { 0 } else { rustix::process::getegid().as_raw() };
        private(base, owner, group, true, 0o700)?;
        let binding = Binding::from(account);
        if binding.uid == 0 {
            return unavailable();
        }
        let suffix =
            digest(&serde_json::to_vec(&binding).map_err(|_| ServiceError::RecoveryUnavailable)?);
        let name = format!("source-queries-{suffix}");
        match fs::mkdirat(base, name.as_str(), Mode::from_raw_mode(0o700)) {
            Ok(()) => fs::fsync(base).map_err(|_| ServiceError::RecoveryUnavailable)?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(_) => return unavailable(),
        }
        let directory = File::from(
            fs::openat2(
                base,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                NO_LINKS,
            )
            .map_err(|_| ServiceError::RecoveryUnavailable)?,
        );
        private(&directory, owner, group, true, 0o700)?;
        let lock = File::from(
            fs::openat2(
                &directory,
                "quota.lock",
                OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o600),
                NO_LINKS,
            )
            .map_err(|_| ServiceError::RecoveryUnavailable)?,
        );
        private(&lock, owner, group, false, 0o600)?;
        fs::fsync(&directory).map_err(|_| ServiceError::RecoveryUnavailable)?;
        Ok(Self { directory, lock, account: binding, owner, group })
    }
    fn locked<T>(&self, body: impl FnOnce() -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        fs::flock(&self.lock, fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| ServiceError::Busy)?;
        struct Unlock<'a>(&'a File);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                let _ = fs::flock(self.0, fs::FlockOperation::Unlock);
            }
        }
        let _unlock = Unlock(&self.lock);
        body()
    }
    fn capacity(
        &self,
        extra: u64,
        extra_files: usize,
        fulfilling: Option<&str>,
    ) -> Result<(), ServiceError> {
        private(&self.directory, self.owner, self.group, true, 0o700)?;
        let mut bytes = extra;
        let mut requests = BTreeSet::new();
        let mut results = BTreeSet::new();
        let mut count = 0;
        for entry in
            fs::Dir::read_from(&self.directory).map_err(|_| ServiceError::RecoveryUnavailable)?
        {
            let entry = entry.map_err(|_| ServiceError::RecoveryUnavailable)?;
            let name = entry.file_name().to_str().map_err(|_| ServiceError::RecoveryUnavailable)?;
            if matches!(name, "." | ".." | "quota.lock") {
                continue;
            }
            count += 1;
            if count > MAX_STORE_FILES {
                return Err(ServiceError::Malformed("source_history_capacity".into()));
            }
            let (token, kind) = name
                .strip_prefix("q-")
                .and_then(|name| name.split_once('.'))
                .ok_or(ServiceError::RecoveryUnavailable)?;
            if token.len() != 40
                || !token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return unavailable();
            }
            match kind {
                "request.json" | "request.pending" => {
                    requests.insert(token.to_owned());
                }
                "result.json" => {
                    results.insert(token.to_owned());
                }
                "result.pending" => {}
                _ => return unavailable(),
            }
            let file = File::from(
                fs::openat2(
                    &self.directory,
                    name,
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                    Mode::empty(),
                    NO_LINKS,
                )
                .map_err(|_| ServiceError::RecoveryUnavailable)?,
            );
            let mode =
                fs::fstat(&file).map_err(|_| ServiceError::RecoveryUnavailable)?.st_mode & 0o7777;
            if kind.ends_with("pending") && !matches!(mode, 0o600 | 0o400) {
                return unavailable();
            }
            private(
                &file,
                self.owner,
                self.group,
                false,
                if kind.ends_with("pending") { mode } else { 0o400 },
            )?;
            let size = fs::fstat(&file).map_err(|_| ServiceError::RecoveryUnavailable)?.st_size;
            if size < 0 || size as usize > MAX_RESULT {
                return unavailable();
            }
            bytes = bytes.checked_add(size as u64).ok_or(ServiceError::RecoveryUnavailable)?;
        }
        if !results.is_subset(&requests) {
            return unavailable();
        }
        let unfinished = requests
            .difference(&results)
            .filter(|token| Some(token.as_str()) != fulfilling)
            .count();
        bytes = bytes
            .checked_add((unfinished as u64).saturating_mul(MAX_RESULT as u64))
            .ok_or(ServiceError::RecoveryUnavailable)?;
        // A durable request reserves both result bytes and its directory entry
        // before any remote request can start. Interrupted files remain counted.
        if bytes > MAX_STORE_BYTES
            || count.saturating_add(unfinished).saturating_add(extra_files) > MAX_STORE_FILES
        {
            return Err(ServiceError::Malformed("source_history_capacity".into()));
        }
        Ok(())
    }
    fn read(&self, name: &str, maximum: usize) -> Result<Vec<u8>, ServiceError> {
        let file = File::from(
            fs::openat2(
                &self.directory,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                NO_LINKS,
            )
            .map_err(|_| ServiceError::RecoveryUnavailable)?,
        );
        private(&file, self.owner, self.group, false, 0o400)?;
        let before = fs::fstat(&file).map_err(|_| ServiceError::RecoveryUnavailable)?;
        if before.st_size < 0 || before.st_size as usize > maximum {
            return unavailable();
        }
        let mut bytes = Vec::new();
        (&file)
            .take(maximum as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ServiceError::RecoveryUnavailable)?;
        let after = fs::fstat(&file).map_err(|_| ServiceError::RecoveryUnavailable)?;
        if bytes.len() != before.st_size as usize
            || bytes.len() > maximum
            || (before.st_dev, before.st_ino, before.st_size, before.st_ctime, before.st_ctime_nsec)
                != (after.st_dev, after.st_ino, after.st_size, after.st_ctime, after.st_ctime_nsec)
        {
            return unavailable();
        }
        Ok(bytes)
    }
    fn publish(&self, token: &str, kind: &str, bytes: &[u8]) -> Result<(), ServiceError> {
        let pending = format!("q-{token}.{kind}.pending");
        let final_name = format!("q-{token}.{kind}.json");
        if !matches!(
            fs::statat(&self.directory, final_name.as_str(), fs::AtFlags::SYMLINK_NOFOLLOW),
            Err(rustix::io::Errno::NOENT)
        ) {
            return unavailable();
        }
        let mut file = File::from(
            fs::openat2(
                &self.directory,
                pending.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o600),
                NO_LINKS,
            )
            .map_err(|_| ServiceError::RecoveryUnavailable)?,
        );
        private(&file, self.owner, self.group, false, 0o600)?;
        file.write_all(bytes).map_err(|_| ServiceError::RecoveryUnavailable)?;
        fs::fsync(&file).map_err(|_| ServiceError::RecoveryUnavailable)?;
        fs::fchmod(&file, Mode::from_raw_mode(0o400))
            .map_err(|_| ServiceError::RecoveryUnavailable)?;
        fs::fsync(&file).map_err(|_| ServiceError::RecoveryUnavailable)?;
        fs::renameat_with(
            &self.directory,
            pending.as_str(),
            &self.directory,
            final_name.as_str(),
            fs::RenameFlags::NOREPLACE,
        )
        .map_err(|_| ServiceError::RecoveryUnavailable)?;
        fs::fsync(&self.directory).map_err(|_| ServiceError::RecoveryUnavailable)?;
        Ok(())
    }
    fn prepare(&self, record: RequestRecord) -> Result<PreparedRequest, ServiceError> {
        record.validate(&self.account)?;
        let bytes = serde_json::to_vec(&record).map_err(|_| ServiceError::RecoveryUnavailable)?;
        if bytes.len() > MAX_REQUEST {
            return unavailable();
        }
        let token = record.token()?;
        self.locked(|| {
            self.capacity(bytes.len() as u64 + MAX_RESULT as u64, 2, None)?;
            self.publish(&token, "request", &bytes)
        })?;
        let request = PreparedRequest { record, sha256: hash(&bytes), token };
        request.revalidate(self)?;
        Ok(request)
    }
    fn complete(
        &self,
        request: &PreparedRequest,
        outcome: Outcome,
    ) -> Result<SavedMetadata, ServiceError> {
        request.revalidate(self)?;
        let record = ResultRecord {
            version: 1,
            request_sha256: request.sha256.clone(),
            request: request.record.clone(),
            outcome,
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| ServiceError::RecoveryUnavailable)?;
        if bytes.len() > MAX_RESULT {
            return unavailable();
        }
        let saved = SavedMetadata { record, sha256: hash(&bytes), token: request.token.clone() };
        saved.validate(&self.account)?;
        self.locked(|| {
            self.capacity(bytes.len() as u64, 1, Some(&request.token))?;
            self.publish(&request.token, "result", &bytes)
        })?;
        self.load(&request.token)
    }
    fn load(&self, token: &str) -> Result<SavedMetadata, ServiceError> {
        if token.len() != 40
            || !token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return unavailable();
        }
        let bytes = self.read(&format!("q-{token}.result.json"), MAX_RESULT)?;
        let record: ResultRecord =
            serde_json::from_slice(&bytes).map_err(|_| ServiceError::RecoveryUnavailable)?;
        let request = self.read(&format!("q-{token}.request.json"), MAX_REQUEST)?;
        if hash(&request) != record.request_sha256
            || serde_json::from_slice::<RequestRecord>(&request)
                .map_err(|_| ServiceError::RecoveryUnavailable)?
                != record.request
        {
            return unavailable();
        }
        let saved = SavedMetadata { record, sha256: hash(&bytes), token: token.into() };
        saved.validate(&self.account)?;
        Ok(saved)
    }
    fn cursor(&self, cursor: &Sha256) -> Result<(SavedMetadata, usize), ServiceError> {
        let saved = self.load(&cursor.as_str()[..40])?;
        let offset = saved.offset(cursor, "ryoku-source-page-v1")?;
        Ok((saved, offset))
    }
}

#[cfg(test)]
#[path = "ryogami_sources_tests.rs"]
mod tests;
