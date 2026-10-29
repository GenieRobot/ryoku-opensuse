//! Bounded, explicitly enrolled local activity collectors. Parsing never retains
//! prompt/message content or credentials. Activity and soft caps are not quota.
#[allow(unsafe_code)]
mod sqlite;
mod sqlite_image;
#[cfg(test)]
mod sqlite_tests;
use crate::{Error, Result};
use jiff::{Timestamp, ToSpan, tz::TimeZone};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS);
const FILE_BYTES: u64 = 4 * 1024 * 1024;
const TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 4096;
const MAX_DIRECTORIES: usize = 512;
const MAX_EVENTS: usize = 100_000;
pub const INTERVAL_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Claude,
    Codex,
    Opencode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub tool: Tool,
    pub path: PathBuf,
    device: u64,
    inode: u64,
    pub soft_caps: SoftCaps,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftCaps {
    pub five_hour_limit: u64,
    pub seven_day_limit: u64,
}
impl Default for SoftCaps {
    fn default() -> Self {
        Self { five_hour_limit: 1_000_000, seven_day_limit: 20_000_000 }
    }
}
impl SoftCaps {
    pub fn validate(&self) -> Result<()> {
        if self.five_hour_limit == 0
            || self.seven_day_limit == 0
            || self.five_hour_limit > 1_000_000_000_000_000
            || self.seven_day_limit > 1_000_000_000_000_000
        {
            Err(Error::InvalidRequest)
        } else {
            Ok(())
        }
    }
}

impl Enrollment {
    pub fn enroll(tool: Tool, path: &Path) -> Result<Self> {
        let directory = private_directory(path)?;
        let stat = fs::fstat(&directory).map_err(|_| Error::Storage)?;
        Ok(Self {
            tool,
            path: path.to_owned(),
            device: stat.st_dev,
            inode: stat.st_ino,
            soft_caps: SoftCaps::default(),
        })
    }
    fn open(&self) -> Result<File> {
        self.soft_caps.validate()?;
        let directory = private_directory(&self.path)?;
        let stat = fs::fstat(&directory).map_err(|_| Error::Storage)?;
        if stat.st_dev != self.device || stat.st_ino != self.inode {
            return Err(Error::Integrity);
        }
        Ok(directory)
    }
}

fn private_directory(path: &Path) -> Result<File> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(part, std::path::Component::ParentDir | std::path::Component::CurDir)
        })
    {
        return Err(Error::UnsafeState);
    }
    let root = File::open("/").map_err(|_| Error::Storage)?;
    let directory = File::from(
        fs::openat2(
            &root,
            path.strip_prefix("/").map_err(|_| Error::UnsafeState)?,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            NO_LINKS,
        )
        .map_err(|_| Error::UnsafeState)?,
    );
    let stat = fs::fstat(&directory).map_err(|_| Error::Storage)?;
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o7777 != 0o700 {
        return Err(Error::UnsafeState);
    }
    Ok(directory)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub tool: Tool,
    pub source_status: String,
    #[serde(default = "json_source_format")]
    pub source_format: String,
    pub attempted_at: u64,
    pub updated_at: Option<u64>,
    pub files_scanned: usize,
    pub partial: bool,
    pub calendar: String,
    pub five_hour_tokens: u64,
    pub seven_day_tokens: u64,
    pub last_hour_tokens: u64,
    pub today_tokens: u64,
    pub recent_days: Vec<Value>,
    pub five_hour_cost: f64,
    pub seven_day_cost: f64,
    pub cost_status: String,
    pub models: Vec<Value>,
    pub soft_caps: Option<Value>,
    pub quota_status: String,
    pub last_error: Option<Error>,
}

impl Report {
    pub fn unavailable(tool: Tool, now: u64) -> Self {
        Self {
            tool,
            source_status: "unavailable".into(),
            source_format: json_source_format(),
            attempted_at: now,
            updated_at: None,
            files_scanned: 0,
            partial: false,
            calendar: String::new(),
            five_hour_tokens: 0,
            seven_day_tokens: 0,
            last_hour_tokens: 0,
            today_tokens: 0,
            recent_days: Vec::new(),
            five_hour_cost: 0.0,
            seven_day_cost: 0.0,
            cost_status: "unavailable".into(),
            models: Vec::new(),
            soft_caps: None,
            quota_status: "unavailable".into(),
            last_error: None,
        }
    }
    pub fn stale(mut self, now: u64) -> Self {
        self.source_status = "stale".into();
        self.attempted_at = now;
        self
    }
}

pub fn due(now: u64, last_attempt: Option<u64>) -> bool {
    last_attempt.is_none_or(|last| now.saturating_sub(last) >= INTERVAL_SECONDS || last > now)
}

#[derive(Default)]
struct Scan {
    bytes: u64,
    files: usize,
    directories: usize,
    partial: bool,
    events: Vec<Event>,
    seen_files: BTreeSet<(u64, u64)>,
    opencode_records: usize,
    sqlite: bool,
}
struct Event {
    time: i64,
    tokens: u64,
    cache_read: u64,
    cost: f64,
    cost_reported: bool,
    model: String,
    provider: String,
    opencode: Option<OpencodeDetail>,
}
struct OpencodeDetail {
    session_key: Option<String>,
    input: u64,
    output: u64,
    reasoning: u64,
    cache_write: u64,
    time_ms: i64,
}

pub fn collect(enrollment: &Enrollment, now: u64) -> Result<Report> {
    let root = enrollment.open()?;
    let roots: &[&str] = match enrollment.tool {
        Tool::Claude => &["projects"],
        Tool::Codex => &["sessions", "archived_sessions"],
        Tool::Opencode => &["storage/message", "storage/session/message"],
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut scan = Scan::default();
    let mut found = false;
    for path in roots {
        match fs::openat2(
            &root,
            *path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            NO_LINKS,
        ) {
            Ok(_) => {
                found = true;
                scan_directory(&root, Path::new(path), 0, enrollment.tool, deadline, &mut scan)?;
            }
            Err(rustix::io::Errno::NOENT) => (),
            Err(_) => return Err(Error::UnsafeState),
        }
    }
    if enrollment.tool == Tool::Opencode
        && scan.opencode_records == 0
        && fs::statat(&root, "opencode.db", fs::AtFlags::SYMLINK_NOFOLLOW).is_ok()
    {
        let image = sqlite_image::capture(&root, TOTAL_BYTES.saturating_sub(scan.bytes), deadline)?;
        scan.bytes += image.source_bytes;
        scan.files += image.files;
        let reader = sqlite::Reader::new(image.bytes, deadline)?;
        let cutoff = i64::try_from(now)
            .map_err(|_| Error::InvalidRequest)?
            .saturating_sub(8 * 86400)
            .saturating_mul(1000);
        reader.rows(cutoff, |bytes, role, time, session| {
            let Ok(mut value) = serde_json::from_slice::<Value>(bytes) else {
                scan.partial = true;
                return Ok(false);
            };
            let Some(object) = value.as_object_mut() else {
                scan.partial = true;
                return Ok(false);
            };
            if let Some(role) = role.filter(|role| !role.is_empty()) {
                let Ok(role) = std::str::from_utf8(role) else {
                    scan.partial = true;
                    return Ok(false);
                };
                object.insert("role".into(), Value::String(role.into()));
            }
            if object.get("sessionID").and_then(Value::as_str).is_none_or(str::is_empty)
                && let Some(session) = session.and_then(|bytes| std::str::from_utf8(bytes).ok())
            {
                object.insert("sessionID".into(), Value::String(session.into()));
            }
            let model = object.get("model").cloned().unwrap_or(Value::Null);
            if object.get("modelID").and_then(Value::as_str).is_none_or(str::is_empty) {
                object.insert("modelID".into(), model["id"].clone());
            }
            if object.get("providerID").and_then(Value::as_str).is_none_or(str::is_empty) {
                object.insert("providerID".into(), model["providerID"].clone());
            }
            if message_time(&value).unwrap_or_default() == 0
                && let Some(time) = time
            {
                value["time"] = json!(time);
            }
            let before = scan.opencode_records;
            parse_opencode(&value, &mut scan);
            Ok(scan.opencode_records != before)
        })?;
        scan.sqlite = true;
        found = true;
    }
    // A replaced enrollment never publishes results from its former identity.
    enrollment.open()?;
    if !found {
        return Ok(Report::unavailable(enrollment.tool, now));
    }
    if scan.partial && scan.events.is_empty() {
        return Err(Error::ProviderUnavailable);
    }
    aggregate(enrollment.tool, now, &enrollment.soft_caps, scan)
}

fn scan_directory(
    root: &File,
    path: &Path,
    depth: usize,
    tool: Tool,
    deadline: Instant,
    scan: &mut Scan,
) -> Result<()> {
    if depth > 8 || scan.directories >= MAX_DIRECTORIES || Instant::now() >= deadline {
        scan.partial = true;
        return Ok(());
    }
    scan.directories += 1;
    let directory = File::from(
        fs::openat2(
            root,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            NO_LINKS,
        )
        .map_err(|_| Error::UnsafeState)?,
    );
    let stat = fs::fstat(&directory).map_err(|_| Error::Storage)?;
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o022 != 0 {
        return Err(Error::UnsafeState);
    }
    let entries = std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
        .map_err(|_| Error::Storage)?;
    for entry in entries {
        if scan.files >= MAX_FILES || scan.bytes >= TOTAL_BYTES || Instant::now() >= deadline {
            scan.partial = true;
            break;
        }
        let entry = entry.map_err(|_| Error::Storage)?;
        let kind = entry.file_type().map_err(|_| Error::Storage)?;
        let relative = path.join(entry.file_name());
        if kind.is_symlink() {
            return Err(Error::UnsafeState);
        }
        if kind.is_dir() {
            scan_directory(root, &relative, depth + 1, tool, deadline, scan)?;
            continue;
        }
        let extension = relative.extension().and_then(|name| name.to_str());
        if !kind.is_file()
            || extension != Some(if tool == Tool::Opencode { "json" } else { "jsonl" })
        {
            continue;
        }
        let mut file = File::from(
            fs::openat2(
                root,
                &relative,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                NO_LINKS,
            )
            .map_err(|_| Error::UnsafeState)?,
        );
        let before = fs::fstat(&file).map_err(|_| Error::Storage)?;
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
            || before.st_uid != rustix::process::geteuid().as_raw()
            || before.st_nlink != 1
            || before.st_mode & 0o022 != 0
        {
            return Err(Error::UnsafeState);
        }
        if !scan.seen_files.insert((before.st_dev, before.st_ino)) {
            continue;
        }
        scan.files += 1;
        let mut tailed = false;
        if before.st_size as u64 > FILE_BYTES {
            scan.partial = true;
            if tool == Tool::Opencode {
                continue;
            }
            file.seek(SeekFrom::End(-(FILE_BYTES as i64))).map_err(|_| Error::Storage)?;
            tailed = true;
        }
        let remaining = FILE_BYTES.min(TOTAL_BYTES - scan.bytes);
        if before.st_size as u64 > remaining {
            scan.partial = true;
        }
        let mut bytes = Vec::new();
        (&mut file).take(remaining).read_to_end(&mut bytes).map_err(|_| Error::Storage)?;
        scan.bytes += bytes.len() as u64;
        let after = fs::fstat(&file).map_err(|_| Error::Storage)?;
        if before.st_size != after.st_size
            || before.st_mtime != after.st_mtime
            || before.st_mtime_nsec != after.st_mtime_nsec
        {
            scan.partial = true;
        }
        if tool == Tool::Opencode {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(mut value) => {
                    if !value.is_object() {
                        scan.partial = true;
                        continue;
                    }
                    if value["sessionID"].as_str().is_none_or(str::is_empty)
                        && let Some(session) = relative
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|name| name.to_str())
                    {
                        value["sessionID"] = json!(session);
                    }
                    parse_opencode(&value, scan);
                }
                Err(_) => scan.partial = true,
            }
        } else {
            let mut previous = None;
            for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
                if (tailed && index == 0) || line.is_empty() {
                    continue;
                }
                match serde_json::from_slice::<Value>(line) {
                    Ok(value) if tool == Tool::Claude => parse_claude(&value, scan),
                    Ok(value) => parse_codex(&value, &mut previous, tailed, scan),
                    Err(_) => scan.partial = true,
                }
            }
        }
    }
    let after = fs::fstat(&directory).map_err(|_| Error::Storage)?;
    if after.st_mtime != stat.st_mtime || after.st_mtime_nsec != stat.st_mtime_nsec {
        scan.partial = true;
    }
    let current = fs::openat2(
        root,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        NO_LINKS,
    )
    .map_err(|_| Error::UnsafeState)?;
    let current = fs::fstat(current).map_err(|_| Error::Storage)?;
    if current.st_dev != stat.st_dev || current.st_ino != stat.st_ino {
        return Err(Error::Integrity);
    }
    Ok(())
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}
fn label(value: Option<&str>) -> String {
    value
        .unwrap_or("unknown")
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect()
}
fn stamp(value: &Value) -> Option<i64> {
    value.as_str()?.parse::<Timestamp>().ok().map(Timestamp::as_second)
}
fn push(scan: &mut Scan, event: Event) {
    if event.opencode.is_some() || event.tokens > 0 || event.cache_read > 0 || event.cost > 0.0 {
        if scan.events.len() >= MAX_EVENTS {
            scan.partial = true;
        } else {
            scan.events.push(event);
        }
    }
}

fn parse_claude(value: &Value, scan: &mut Scan) {
    let message = &value["message"];
    if message["role"] != "assistant" {
        return;
    }
    let Some(time) = stamp(&value["timestamp"]) else {
        scan.partial = true;
        return;
    };
    let usage = &message["usage"];
    let Some(tokens) =
        ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]
            .iter()
            .try_fold(0u64, |total, key| total.checked_add(number(usage, key)))
    else {
        scan.partial = true;
        return;
    };
    push(
        scan,
        Event {
            time,
            tokens,
            cache_read: number(usage, "cache_read_input_tokens"),
            cost: 0.0,
            cost_reported: false,
            model: label(message["model"].as_str()),
            provider: "anthropic".into(),
            opencode: None,
        },
    );
}

fn parse_codex(value: &Value, previous: &mut Option<u64>, tailed: bool, scan: &mut Scan) {
    let payload = &value["payload"];
    if payload["type"] != "token_count" {
        return;
    }
    let Some(time) = stamp(&value["timestamp"]) else {
        scan.partial = true;
        return;
    };
    let info = &payload["info"];
    let total = ["total_token_usage", "totalTokenUsage", "token_usage", "tokenUsage"]
        .iter()
        .find_map(|key| info[*key]["total_tokens"].as_u64());
    let last = ["last_token_usage", "lastTokenUsage"]
        .iter()
        .find_map(|key| info[*key]["total_tokens"].as_u64())
        .unwrap_or_default();
    let tokens = if let Some(total) = total {
        let delta = match *previous {
            Some(before) if total >= before => total - before,
            Some(_) => total,
            None if tailed => last,
            None => total,
        };
        *previous = Some(total);
        delta
    } else {
        last
    };
    push(
        scan,
        Event {
            time,
            tokens,
            cache_read: 0,
            cost: 0.0,
            cost_reported: false,
            model: label(payload["model"].as_str()),
            provider: "openai".into(),
            opencode: None,
        },
    );
}

fn message_time(value: &Value) -> Option<i64> {
    value["time"]["completed"]
        .as_i64()
        .filter(|time| *time != 0)
        .or_else(|| value["time"]["created"].as_i64())
        .or_else(|| value["time"].as_i64())
}
fn json_source_format() -> String {
    "local_json_activity".into()
}
fn parse_opencode(value: &Value, scan: &mut Scan) {
    if value["role"] != "assistant" || value["modelID"].as_str().is_none_or(str::is_empty) {
        return;
    }
    let time = message_time(value);
    let Some(time) = time else {
        scan.partial = true;
        return;
    };
    scan.opencode_records += 1;
    let usage = &value["tokens"];
    let cache = &usage["cache"];
    let Some(tokens) = [
        number(usage, "input"),
        number(usage, "output"),
        number(usage, "reasoning"),
        number(cache, "write"),
    ]
    .into_iter()
    .try_fold(0u64, u64::checked_add) else {
        scan.partial = true;
        return;
    };
    let cost =
        value["cost"].as_f64().filter(|cost| cost.is_finite() && *cost >= 0.0).unwrap_or_default();
    push(
        scan,
        Event {
            time: time / 1000,
            tokens,
            cache_read: number(cache, "read"),
            cost,
            cost_reported: value["cost"]
                .as_f64()
                .is_some_and(|cost| cost.is_finite() && cost >= 0.0),
            model: label(value["modelID"].as_str()),
            provider: label(value["providerID"].as_str()),
            opencode: Some(OpencodeDetail {
                session_key: value["sessionID"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(|id| crate::digest(id.as_bytes())),
                input: number(usage, "input"),
                output: number(usage, "output"),
                reasoning: number(usage, "reasoning"),
                cache_write: number(cache, "write"),
                time_ms: time,
            }),
        },
    );
}

fn aggregate(tool: Tool, now: u64, soft_caps: &SoftCaps, scan: Scan) -> Result<Report> {
    soft_caps.validate()?;
    let now = i64::try_from(now).map_err(|_| Error::InvalidRequest)?;
    let zone = if tool == Tool::Claude {
        TimeZone::UTC
    } else {
        TimeZone::try_system().map_err(|_| Error::ProviderUnavailable)?
    };
    let today = Timestamp::from_second(now)
        .map_err(|_| Error::InvalidRequest)?
        .to_zoned(zone.clone())
        .date();
    let mut daily = BTreeMap::new();
    for day in (0..7).rev() {
        daily.insert(
            today.checked_sub((day as i64).days()).map_err(|_| Error::InvalidRequest)?.to_string(),
            0u64,
        );
    }
    let mut report = Report::unavailable(tool, now as u64);
    report.source_status = if scan.partial { "partial" } else { "fresh" }.into();
    if scan.sqlite {
        report.source_format = "opencode_sqlite".into();
    }
    report.partial = scan.partial;
    report.updated_at = Some(now as u64);
    report.files_scanned = scan.files;
    report.calendar = if tool == Tool::Claude { "UTC" } else { "system-local" }.into();
    let mut models: BTreeMap<(String, String), ModelUsage> = BTreeMap::new();
    for event in scan.events {
        if event.time > now + 120 || event.time < now - 8 * 86400 {
            continue;
        }
        let date = Timestamp::from_second(event.time)
            .map_err(|_| Error::InvalidRequest)?
            .to_zoned(zone.clone())
            .date();
        let add = |total: &mut u64| -> Result<()> {
            *total = total.checked_add(event.tokens).ok_or(Error::Limit)?;
            Ok(())
        };
        if event.time >= now - 7 * 86400 {
            if event.cost_reported {
                report.cost_status = "local_reported".into();
            }
            add(&mut report.seven_day_tokens)?;
            report.seven_day_cost += event.cost;
        }
        if event.time >= now - 5 * 3600 {
            add(&mut report.five_hour_tokens)?;
            report.five_hour_cost += event.cost;
        }
        if event.time >= now - 3600 {
            add(&mut report.last_hour_tokens)?;
        }
        if date == today {
            add(&mut report.today_tokens)?;
        }
        if let Some(tokens) = daily.get_mut(&date.to_string()) {
            add(tokens)?;
        }
        let key = (event.provider, event.model);
        if !models.contains_key(&key) && models.len() >= 128 {
            report.partial = true;
            report.source_status = "partial".into();
            continue;
        }
        let model = models.entry(key).or_default();
        model.tokens = model.tokens.checked_add(event.tokens).ok_or(Error::Limit)?;
        model.cache_read = model.cache_read.checked_add(event.cache_read).ok_or(Error::Limit)?;
        model.cost += event.cost;
        model.cost_reported |= event.cost_reported;
        if let Some(detail) = event.opencode {
            model.sessions.insert(detail.session_key);
            for (total, value) in [
                (&mut model.input, detail.input),
                (&mut model.output, detail.output),
                (&mut model.reasoning, detail.reasoning),
                (&mut model.cache_write, detail.cache_write),
            ] {
                *total = total.checked_add(value).ok_or(Error::Limit)?;
            }
            model.last_seen = model.last_seen.max(detail.time_ms);
            if date == today {
                model.today = model.today.checked_add(event.tokens).ok_or(Error::Limit)?;
            }
        }
        if !model.cost.is_finite() {
            return Err(Error::Limit);
        }
    }
    if !report.five_hour_cost.is_finite() || !report.seven_day_cost.is_finite() {
        return Err(Error::Limit);
    }
    report.recent_days =
        daily.into_iter().map(|(date, tokens)| json!({"date":date,"tokens":tokens})).collect();
    let mut models = models.into_iter().collect::<Vec<_>>();
    if tool == Tool::Opencode {
        // Stable provider/model tie-break makes filesystem enumeration order
        // irrelevant; the retained collector displays six most recent rows.
        models.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then_with(|| a.0.cmp(&b.0)));
        models.truncate(6);
    }
    let max_tokens = models.iter().map(|(_, usage)| usage.tokens).max().unwrap_or_default();
    report.models = models.into_iter().map(|((provider, model), usage)| {
        let mut row = json!({"provider":provider,"model":model,"tokens":usage.tokens,"cache_read":usage.cache_read,"cache_read_available":tool != Tool::Codex,"cost":usage.cost});
        if tool == Tool::Opencode {
            let total_with_cache_read = usage.tokens.checked_add(usage.cache_read).ok_or(Error::Limit)?;
            let name = if provider.is_empty() || provider == "unknown" { model } else { format!("{model} ({provider})") };
            let extra = json!({"name":name,"sessions":usage.sessions.len(),"sessions_status":if usage.sessions.contains(&None) { "includes_unknown" } else { "source_identified" },
                "input":usage.input,"output":usage.output,"reasoning":usage.reasoning,"cache_write":usage.cache_write,"today":usage.today,
                "total":usage.tokens,"totalLabel":token_label(usage.tokens),"inputLabel":token_label(usage.input),"outputLabel":token_label(usage.output),
                "reasoningLabel":token_label(usage.reasoning),"cacheReadLabel":token_label(usage.cache_read),"cacheWriteLabel":token_label(usage.cache_write),
                "todayLabel":token_label(usage.today),"costLabel":if usage.cost>0.0 { format!("${:.2}",usage.cost) } else { String::new() },
                "cost_status":if usage.cost_reported { "local_reported" } else { "unavailable" },"lastSeen":usage.last_seen,
                "totalWithCacheRead":total_with_cache_read,"totalWithCacheReadLabel":token_label(total_with_cache_read),"pct":relative_percent(usage.tokens,max_tokens)});
            row.as_object_mut().ok_or(Error::Integrity)?.extend(extra.as_object().ok_or(Error::Integrity)?.clone());
        }
        Ok(row)
    }).collect::<Result<_>>()?;
    if tool == Tool::Opencode {
        report.soft_caps = Some(
            json!({"source":"local_soft_caps","five_hour_limit":soft_caps.five_hour_limit,"seven_day_limit":soft_caps.seven_day_limit,"five_hour_utilization":(report.five_hour_tokens as f64 / soft_caps.five_hour_limit as f64).min(1.0),"seven_day_utilization":(report.seven_day_tokens as f64 / soft_caps.seven_day_limit as f64).min(1.0),"provider_quota":false}),
        );
    }
    Ok(report)
}

#[derive(Default)]
struct ModelUsage {
    tokens: u64,
    cache_read: u64,
    cost: f64,
    cost_reported: bool,
    sessions: BTreeSet<Option<String>>,
    input: u64,
    output: u64,
    reasoning: u64,
    cache_write: u64,
    today: u64,
    last_seen: i64,
}
fn token_label(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
fn relative_percent(tokens: u64, maximum: u64) -> u64 {
    if maximum == 0 {
        return 0;
    }
    // Match Python round's ties-to-even, using exact integer arithmetic rather
    // than lossy floating-point ratios. Multiplication fits u128 for any u64.
    let numerator = u128::from(tokens) * 100;
    let denominator = u128::from(maximum);
    let whole = numerator / denominator;
    let remainder = numerator % denominator;
    (whole
        + u128::from(
            remainder * 2 > denominator || (remainder * 2 == denominator && whole % 2 == 1),
        )) as u64
}
