use jiff::Timestamp;
use ryoku_rashin::{
    Error,
    api::Request,
    authorization::Access,
    runtime::Engine,
    store::Store,
    usage::{self, Enrollment, SoftCaps, Tool},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rashin-usage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn enrollment(&self, tool: Tool) -> Enrollment {
        Enrollment::enroll(tool, &self.0).unwrap()
    }
    fn engine(&self) -> std::sync::Arc<Engine> {
        Engine::open(Store::open(&self.0).unwrap()).unwrap()
    }
    fn path(&self) -> String {
        self.0.to_str().unwrap().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn seconds(value: &str) -> u64 {
    value.parse::<Timestamp>().unwrap().as_second() as u64
}
fn timestamp(value: u64) -> String {
    Timestamp::from_second(value as i64).unwrap().to_string()
}
fn lines(rows: &[Value]) -> Vec<u8> {
    rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n").into_bytes()
}
fn claude(at: u64, tokens: u64) -> Value {
    json!({"timestamp":timestamp(at),"message":{"role":"assistant","model":"claude-test","content":"PRIVATE_PROMPT_SENTINEL","usage":{"input_tokens":tokens}}})
}
fn current() -> u64 {
    Timestamp::now().as_second() as u64
}

#[test]
fn claude_local_windows_include_both_cache_types_without_reading_credentials_or_prompts() {
    let source = Fixture::new();
    let now = seconds("2026-10-25T12:00:00Z");
    source.write(".credentials.json", br#"{"accessToken":"PRIVATE_KEY_SENTINEL"}"#);
    source.write("projects/project/session.jsonl", &lines(&[
        json!({"timestamp":"2026-10-25T11:30:00Z","message":{"role":"assistant","model":"claude-test","content":"PRIVATE_PROMPT_SENTINEL","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}}}),
        claude(now - 6 * 3600, 30),
        json!({"timestamp":"2026-10-25T11:30:00Z","message":{"role":"user","content":"PRIVATE_USER_SENTINEL","usage":{"input_tokens":99999}}}),
    ]));
    let report = usage::collect(&source.enrollment(Tool::Claude), now).unwrap();
    assert_eq!(report.five_hour_tokens, 135);
    assert_eq!(report.seven_day_tokens, 165);
    assert_eq!(report.last_hour_tokens, 135);
    assert_eq!(report.today_tokens, 165);
    assert_eq!(report.calendar, "UTC");
    assert_eq!(report.quota_status, "unavailable");
    assert_eq!(report.cost_status, "unavailable");
    assert_eq!(report.recent_days.len(), 7);
    assert_eq!(report.source_status, "fresh");
    let serialized = serde_json::to_string(&report).unwrap();
    for secret in
        ["PRIVATE_PROMPT_SENTINEL", "PRIVATE_USER_SENTINEL", "PRIVATE_KEY_SENTINEL", "accessToken"]
    {
        assert!(!serialized.contains(secret));
    }
}

#[test]
fn codex_cumulative_counts_deduplicate_and_handle_resets_and_last_only_records() {
    let source = Fixture::new();
    let now = current();
    let event = |total: Option<u64>, last: u64| json!({"timestamp":timestamp(now),"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":total},"last_token_usage":{"total_tokens":last}},"rate_limits":{"primary":{"used_percent":80}}}});
    source.write(
        "sessions/2026/10/25/rollout.jsonl",
        &lines(&[
            event(Some(100), 100),
            event(Some(160), 60),
            event(Some(160), 60),
            event(Some(20), 20),
            event(None, 11),
        ]),
    );
    let report = usage::collect(&source.enrollment(Tool::Codex), now).unwrap();
    assert_eq!(report.five_hour_tokens, 191);
    assert_eq!(report.seven_day_tokens, 191);
    assert_eq!(report.quota_status, "unavailable");
    assert_eq!(report.calendar, "system-local");
    // Session-recorded limits are not mislabeled as a live account RPC quota.
    assert!(report.soft_caps.is_none());
}

#[test]
fn opencode_json_and_legacy_layouts_exclude_cache_reads_from_soft_caps_and_keep_costs() {
    let source = Fixture::new();
    let now = current();
    source.write("storage/message/session/a.json", json!({"role":"assistant","modelID":"test-model","providerID":"test-provider","content":"PRIVATE_OPENCODE_TEXT","tokens":{"input":10,"output":4,"reasoning":2,"cache":{"read":1000,"write":3}},"time":{"completed":now*1000},"cost":0.25}).to_string().as_bytes());
    source.write("storage/session/message/legacy/b.json", json!({"role":"assistant","modelID":"test-model","providerID":"test-provider","tokens":{"cache":{"read":2000}},"time":now*1000,"cost":0.5}).to_string().as_bytes());
    let mut enrollment = source.enrollment(Tool::Opencode);
    enrollment.soft_caps = SoftCaps { five_hour_limit: 38, seven_day_limit: 190 };
    let report = usage::collect(&enrollment, now).unwrap();
    assert_eq!(report.five_hour_tokens, 19);
    assert_eq!(report.five_hour_cost, 0.75);
    assert_eq!(report.cost_status, "local_reported");
    assert_eq!(report.models[0]["cache_read"], 3000);
    let caps = report.soft_caps.as_ref().unwrap();
    assert_eq!(caps["five_hour_utilization"], 0.5);
    assert_eq!(caps["provider_quota"], false);
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_OPENCODE_TEXT"));
}

#[test]
fn malformed_logs_are_labeled_partial_and_total_parse_failure_is_unavailable() {
    let source = Fixture::new();
    let now = current();
    let mut bytes = lines(&[claude(now, 10)]);
    bytes.extend_from_slice(b"\nmalformed-json\n");
    source.write("projects/project/session.jsonl", &bytes);
    let report = usage::collect(&source.enrollment(Tool::Claude), now).unwrap();
    assert_eq!(report.source_status, "partial");
    assert!(report.partial);
    assert_eq!(report.five_hour_tokens, 10);
    source.write("projects/project/session.jsonl", b"only malformed data\n");
    assert!(matches!(
        usage::collect(&source.enrollment(Tool::Claude), now),
        Err(Error::ProviderUnavailable)
    ));
}

#[test]
fn oversized_codex_tail_uses_last_turn_and_never_counts_unknown_prefix_as_recent() {
    let source = Fixture::new();
    let now = current();
    source.write("sessions/session.jsonl", b"");
    let row = json!({"timestamp":timestamp(now),"payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":999999},"last_token_usage":{"total_tokens":5}}}});
    let suffix = format!("\n{row}\n");
    let mut file =
        fs::OpenOptions::new().write(true).open(source.0.join("sessions/session.jsonl")).unwrap();
    file.set_len(4 * 1024 * 1024 + 4096).unwrap();
    file.seek(SeekFrom::End(-(suffix.len() as i64))).unwrap();
    file.write_all(suffix.as_bytes()).unwrap();
    drop(file);
    let report = usage::collect(&source.enrollment(Tool::Codex), now).unwrap();
    assert!(report.partial);
    assert_eq!(report.five_hour_tokens, 5);
}

#[test]
fn source_enrollment_and_walk_reject_symlinks_hardlinks_modes_and_replacement() {
    let source = Fixture::new();
    let target = Fixture::new();
    let now = current();
    source.write("projects/project/session.jsonl", &lines(&[claude(now, 10)]));
    let enrollment = source.enrollment(Tool::Claude);
    let file = source.0.join("projects/project/session.jsonl");
    fs::hard_link(&file, source.0.join("linked")).unwrap();
    assert!(matches!(usage::collect(&enrollment, now), Err(Error::UnsafeState)));
    fs::remove_file(source.0.join("linked")).unwrap();
    fs::remove_file(&file).unwrap();
    symlink(target.0.join("private"), &file).unwrap();
    assert!(matches!(usage::collect(&enrollment, now), Err(Error::UnsafeState)));
    fs::remove_file(&file).unwrap();
    fs::set_permissions(&source.0, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(Enrollment::enroll(Tool::Claude, &source.0), Err(Error::UnsafeState)));
    fs::set_permissions(&source.0, fs::Permissions::from_mode(0o700)).unwrap();
    let link = target.0.join("source-link");
    symlink(&source.0, &link).unwrap();
    assert!(matches!(Enrollment::enroll(Tool::Claude, &link), Err(Error::UnsafeState)));
    let saved = target.0.join("saved");
    fs::rename(&source.0, &saved).unwrap();
    fs::create_dir(&source.0).unwrap();
    fs::set_permissions(&source.0, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(usage::collect(&enrollment, now), Err(Error::Integrity)));
}

#[test]
fn enrolled_source_failure_preserves_private_last_good_activity_and_survives_restart() {
    let state = Fixture::new();
    let source = Fixture::new();
    let now = current();
    source.write("projects/project/session.jsonl", &lines(&[claude(now, 10)]));
    let engine = state.engine();
    engine
        .dispatch(Request::EnrollUsageSource {
            tool: Tool::Claude,
            path: source.path(),
            soft_caps: None,
        })
        .unwrap();
    let report = engine.dispatch(Request::CollectUsage { tool: Tool::Claude }).unwrap();
    assert_eq!(report["five_hour_tokens"], 10);
    let saved = state.0.join("retained-source");
    fs::rename(&source.0, &saved).unwrap();
    fs::create_dir(&source.0).unwrap();
    fs::set_permissions(&source.0, fs::Permissions::from_mode(0o700)).unwrap();
    source.write("projects/project/session.jsonl", &lines(&[claude(now, 99999)]));
    let stale = engine.dispatch(Request::CollectUsage { tool: Tool::Claude }).unwrap();
    assert_eq!(stale["source_status"], "stale");
    assert_eq!(stale["five_hour_tokens"], 10);
    assert_eq!(stale["last_error"], "integrity");
    drop(engine);
    let engine = state.engine();
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"]["source_status"],
        "stale"
    );
    engine.dispatch(Request::DisableUsageSource { tool: Tool::Claude }).unwrap();
    assert_eq!(engine.dispatch(Request::CollectUsage { tool: Tool::Claude }), Err(Error::NotFound));
    assert!(engine.dispatch(Request::Usage {}).unwrap()["sources"].as_object().unwrap().is_empty());
}

#[test]
fn persistent_five_minute_schedule_catches_up_once_and_records_failed_attempts() {
    let state = Fixture::new();
    let source = Fixture::new();
    let now = current();
    source.write("projects/project/session.jsonl", &lines(&[claude(now, 10)]));
    let engine = state.engine();
    engine.collect_due_usage(now).unwrap();
    assert!(engine.dispatch(Request::Usage {}).unwrap()["tools"].as_object().unwrap().is_empty());
    engine
        .dispatch(Request::EnrollUsageSource {
            tool: Tool::Claude,
            path: source.path(),
            soft_caps: None,
        })
        .unwrap();
    engine.collect_due_usage(now).unwrap();
    source.write("projects/project/session.jsonl", &lines(&[claude(now, 20)]));
    engine.collect_due_usage(now + 299).unwrap();
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"]["five_hour_tokens"],
        10
    );
    engine.collect_due_usage(now + 300).unwrap();
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"]["five_hour_tokens"],
        20
    );
    drop(engine);
    let engine = state.engine();
    fs::set_permissions(&source.0, fs::Permissions::from_mode(0o755)).unwrap();
    engine.collect_due_usage(now + 599).unwrap();
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"]["source_status"],
        "fresh"
    );
    engine.collect_due_usage(now + 600).unwrap();
    let stale = engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"].clone();
    assert_eq!(stale["source_status"], "stale");
    assert_eq!(stale["attempted_at"], now + 600);
    assert_eq!(stale["five_hour_tokens"], 20);
    fs::set_permissions(&source.0, fs::Permissions::from_mode(0o700)).unwrap();
    engine.collect_due_usage(now + 899).unwrap();
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"]["source_status"],
        "stale"
    );
    engine.collect_due_usage(now + 900).unwrap();
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["tools"]["claude"]["source_status"],
        "fresh"
    );
}

#[test]
fn malformed_sqlite_opencode_is_unavailable_instead_of_claiming_zero_activity() {
    let source = Fixture::new();
    source.write("storage/message/.keep", b"");
    source.write("opencode.db", b"synthetic unsupported database");
    assert!(matches!(
        usage::collect(&source.enrollment(Tool::Opencode), current()),
        Err(Error::Integrity)
    ));
    let state = Fixture::new();
    let engine = state.engine();
    engine
        .dispatch(Request::EnrollUsageSource {
            tool: Tool::Opencode,
            path: source.path(),
            soft_caps: None,
        })
        .unwrap();
    let report = engine.dispatch(Request::CollectUsage { tool: Tool::Opencode }).unwrap();
    assert_eq!(report["source_status"], "unavailable");
    assert_eq!(report["updated_at"], Value::Null);
}

#[test]
fn usage_sources_are_owner_actions_and_require_valid_explicit_soft_caps() {
    let state = Fixture::new();
    let source = Fixture::new();
    let engine = state.engine();
    for request in [
        Request::EnrollUsageSource { tool: Tool::Claude, path: source.path(), soft_caps: None },
        Request::CollectUsage { tool: Tool::Claude },
        Request::DisableUsageSource { tool: Tool::Claude },
    ] {
        assert_eq!(engine.dispatch_as(Access::AskClient, request), Err(Error::Unauthorized));
    }
    assert_eq!(
        engine.dispatch(Request::EnrollUsageSource {
            tool: Tool::Opencode,
            path: source.path(),
            soft_caps: Some(SoftCaps { five_hour_limit: 0, seven_day_limit: 100 })
        }),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        engine.dispatch(Request::EnrollUsageSource {
            tool: Tool::Claude,
            path: source.path(),
            soft_caps: Some(SoftCaps::default())
        }),
        Err(Error::InvalidRequest)
    );
}

fn cli(tool: &str, root: &Path, zone: &str) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
        .args([tool, root.to_str().unwrap()])
        .env("TZ", zone)
        .output()
        .unwrap();
    assert!(output.status.success(), "collector failed without exposing source data");
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn daemon_scheduler_really_collects_overdue_enrolled_sources_after_restart() {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let state = Fixture::new();
    let source = Fixture::new();
    let now = current();
    source.write("projects/project/session.jsonl", &lines(&[claude(now - 301, 10)]));
    let engine = state.engine();
    engine
        .dispatch(Request::EnrollUsageSource {
            tool: Tool::Claude,
            path: source.path(),
            soft_caps: None,
        })
        .unwrap();
    engine.collect_due_usage(now - 301).unwrap();
    drop(engine);
    source.write("projects/project/session.jsonl", &lines(&[claude(now, 20)]));
    let _daemon = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
            .arg("serve")
            .arg(&state.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let mut client = Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
            .arg("request")
            .arg(state.0.join("rashin.sock"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        client.stdin.take().unwrap().write_all(br#"{"action":"usage"}"#).unwrap();
        let output = client.wait_with_output().unwrap();
        if output.status.success() {
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            if value["data"]["tools"]["claude"]["five_hour_tokens"] == 20 {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon scheduler did not catch up");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn local_day_boundaries_follow_system_timezone_and_claude_stays_utc() {
    let source = Fixture::new();
    let now = current();
    let at = now - 3600;
    source.write(
        "storage/message/session/a.json",
        json!({"role":"assistant","modelID":"model","tokens":{"input":5},"time":at*1000})
            .to_string()
            .as_bytes(),
    );
    source.write("projects/project/session.jsonl", &lines(&[claude(at, 5)]));
    let hour =
        Timestamp::from_second(now as i64).unwrap().to_zoned(jiff::tz::TimeZone::UTC).hour() as i32;
    let midnight_offset = if hour <= 12 { -hour } else { 24 - hour };
    let noon_offset =
        if midnight_offset <= 0 { midnight_offset + 12 } else { midnight_offset - 12 };
    let zone = |offset: i32| {
        if offset >= 0 { format!("Etc/GMT-{offset}") } else { format!("Etc/GMT+{}", -offset) }
    };
    let midnight = zone(midnight_offset);
    let noon = zone(noon_offset);
    assert_eq!(cli("usage-opencode", &source.0, &midnight)["today_tokens"], 0);
    assert_eq!(cli("usage-opencode", &source.0, &noon)["today_tokens"], 5);
    let first = cli("usage-claude", &source.0, &midnight);
    let second = cli("usage-claude", &source.0, &noon);
    assert_eq!(first["today_tokens"], second["today_tokens"]);
    assert_eq!(first["calendar"], "UTC");
}
