use super::{
    sqlite::{
        Reader,
        testing::{Writer, denied, subprocess},
    },
    sqlite_image, *,
};
use crate::{api::Request, runtime::Engine, store::Store};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) struct Directory(pub(super) PathBuf);
impl Directory {
    pub(super) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rashin-sqlite-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    pub(super) fn root(&self) -> File {
        private_directory(&self.0).unwrap()
    }
    pub(super) fn old(&self) -> Writer {
        let writer = Writer::create(&self.0.join("opencode.db"));
        writer.execute("CREATE TABLE message(session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT)");
        writer
    }
    fn enroll(&self) -> Enrollment {
        Enrollment::enroll(Tool::Opencode, &self.0).unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn now() -> u64 {
    Timestamp::now().as_second() as u64
}
fn record(at: u64, tokens: u64) -> String {
    json!({"role":"assistant","modelID":"fixture-model","providerID":"fixture-provider","time":at*1000,"tokens":{"input":tokens,"cache":{"read":900}},"cost":0.25,"content":"PRIVATE_SQLITE_TRANSCRIPT"}).to_string().replace('\'', "''")
}
fn insert(writer: &Writer, at: u64, tokens: u64) {
    writer.execute(&format!(
        "INSERT INTO message VALUES('s',{},{},'{}')",
        at * 1000,
        at * 1000,
        record(at, tokens)
    ));
}
fn collect_dir(directory: &Directory) -> Report {
    collect(&directory.enroll(), now()).unwrap()
}

#[test]
fn sqlite_old_schema_retains_counters_cost_cache_and_source_privacy() {
    let directory = Directory::new();
    let writer = directory.old();
    let at = now();
    insert(&writer, at, 42);
    writer.execute("CREATE TABLE private_secrets(value TEXT);INSERT INTO private_secrets VALUES('PRIVATE_SQLITE_CREDENTIAL')");
    drop(writer);
    let before = fs::read(directory.0.join("opencode.db")).unwrap();
    let report = collect_dir(&directory);
    assert_eq!(report.five_hour_tokens, 42);
    assert_eq!(report.five_hour_cost, 0.25);
    assert_eq!(report.models[0]["cache_read"], 900);
    assert_eq!(report.source_format, "opencode_sqlite");
    assert_eq!(report.source_status, "fresh");
    assert_eq!(report.quota_status, "unavailable");
    let output = serde_json::to_string(&report).unwrap();
    assert!(!output.contains("PRIVATE_SQLITE"));
    assert_eq!(fs::read(directory.0.join("opencode.db")).unwrap(), before);
    assert!(!directory.0.join("opencode.db-journal").exists());
    assert!(!directory.0.join("opencode.db-shm").exists());
}

#[test]
fn sqlite_v2_aliases_nested_model_and_sql_timestamp_fallback_work() {
    for (blob, role, time) in [
        ("data", "type", "time_updated"),
        ("content", "role", "created_at"),
        ("body", "type", "updated_at"),
        ("message", "role", "time_created"),
        ("json", "type", "time_updated"),
    ] {
        let directory = Directory::new();
        let writer = Writer::create(&directory.0.join("opencode.db"));
        let at = now();
        writer.execute(&format!(
            "CREATE TABLE session_message({role} TEXT,{time} INTEGER,{blob} TEXT)"
        ));
        let data = json!({"model":{"id":"nested-model","providerID":"nested-provider"},"tokens":{"input":17},"cost":0.5}).to_string();
        writer.execute(&format!(
            "INSERT INTO session_message VALUES('assistant',{},'{data}')",
            at * 1000
        ));
        drop(writer);
        let report = collect_dir(&directory);
        assert_eq!(report.five_hour_tokens, 17);
        assert_eq!(report.models[0]["model"], "nested-model");
        assert_eq!(report.models[0]["provider"], "nested-provider");
    }
}

#[test]
fn valid_filesystem_json_precedes_sqlite_and_invalid_v2_rows_fall_back_to_old_table() {
    let directory = Directory::new();
    let writer = directory.old();
    let at = now();
    insert(&writer, at, 42);
    writer.execute("CREATE TABLE session_message(type TEXT,time_updated INTEGER,data TEXT)");
    writer.execute(&format!("INSERT INTO session_message VALUES('user',{},'{{\"modelID\":\"ignored\",\"time\":{},\"tokens\":{{\"input\":9999}}}}')",at*1000,at*1000));
    drop(writer);
    assert_eq!(collect_dir(&directory).five_hour_tokens, 42);
    fs::create_dir_all(directory.0.join("storage/message/session")).unwrap();
    fs::write(directory.0.join("storage/message/session/entry.json"), record(at, 9)).unwrap();
    let report = collect_dir(&directory);
    assert_eq!(report.five_hour_tokens, 9);
    assert_eq!(report.source_format, "local_json_activity");
}

#[test]
fn reader_has_no_file_authority_and_denies_mutating_or_schema_escape_sql() {
    let directory = Directory::new();
    let writer = directory.old();
    insert(&writer, now(), 42);
    writer.execute("CREATE TABLE private_secrets(value TEXT)");
    let reader = Reader::new(writer.serialized(), Instant::now() + Duration::from_secs(2)).unwrap();
    for sql in [
        "CREATE TABLE forbidden(x)",
        "CREATE TEMP TABLE forbidden(x)",
        "INSERT INTO message(data) VALUES('bad')",
        "UPDATE message SET data='bad'",
        "DELETE FROM message",
        "DROP TABLE message",
        "PRAGMA query_only=OFF",
        "PRAGMA writable_schema=ON",
        "PRAGMA temp_store=FILE",
        "PRAGMA trusted_schema=ON",
        "DETACH DATABASE main",
        "SELECT value FROM private_secrets",
        "SELECT load_extension('forbidden')",
    ] {
        assert!(denied(&reader, sql), "dangerous statement accepted");
    }
    let target = directory.0.join("must-not-exist.db");
    assert!(denied(&reader, &format!("ATTACH '{}' AS other", target.display())));
    assert!(!target.exists());
}

#[test]
fn live_wal_uncommitted_transaction_is_excluded_without_touching_sidecars() {
    let directory = Directory::new();
    drop(directory.old());
    let _child = subprocess(&directory.0, "uncommitted", now());
    let names = ["opencode.db", "opencode.db-wal", "opencode.db-shm"];
    let before: Vec<_> =
        names.iter().map(|name| fs::read(directory.0.join(name)).unwrap()).collect();
    assert_eq!(collect_dir(&directory).five_hour_tokens, 42);
    for (name, bytes) in names.iter().zip(before) {
        assert_eq!(fs::read(directory.0.join(name)).unwrap(), bytes);
    }
}

#[test]
fn native_wal_reset_retained_tail_matches_committed_usage() {
    let directory = Directory::new();
    drop(directory.old());
    let _child = subprocess(&directory.0, "reset", now());
    assert_eq!(collect_dir(&directory).five_hour_tokens, 69);
}

#[test]
fn sqlite_exclusive_lock_is_unavailable_then_recovers() {
    let directory = Directory::new();
    let writer = directory.old();
    insert(&writer, now(), 42);
    drop(writer);
    let child = subprocess(&directory.0, "locked", now());
    assert!(matches!(collect(&directory.enroll(), now()), Err(Error::Busy)));
    drop(child);
    assert_eq!(collect_dir(&directory).five_hour_tokens, 42);
}

#[test]
fn sqlite_reserved_writer_can_coexist_with_the_reader_snapshot() {
    let directory = Directory::new();
    let writer = directory.old();
    insert(&writer, now(), 42);
    drop(writer);
    let _child = subprocess(&directory.0, "reserved", now());
    assert_eq!(collect_dir(&directory).five_hour_tokens, 42);
}

#[test]
fn corrupt_database_and_unsafe_sidecars_preserve_last_good_api_report() {
    let source = Directory::new();
    let writer = source.old();
    insert(&writer, now(), 42);
    drop(writer);
    let state = Directory::new();
    let engine = Engine::open(Store::open(&state.0).unwrap()).unwrap();
    engine
        .dispatch(Request::EnrollUsageSource {
            tool: Tool::Opencode,
            path: source.0.to_str().unwrap().into(),
            soft_caps: None,
        })
        .unwrap();
    assert_eq!(
        engine.dispatch(Request::CollectUsage { tool: Tool::Opencode }).unwrap()["five_hour_tokens"],
        42
    );
    fs::write(source.0.join("opencode.db"), b"malformed private DB data").unwrap();
    let report = engine.dispatch(Request::CollectUsage { tool: Tool::Opencode }).unwrap();
    assert_eq!(report["source_status"], "stale");
    assert_eq!(report["five_hour_tokens"], 42);
    assert_eq!(report["last_error"], "integrity");
    fs::remove_file(source.0.join("opencode.db")).unwrap();
    let writer = source.old();
    insert(&writer, now(), 9);
    drop(writer);
    symlink("/etc/passwd", source.0.join("opencode.db-wal")).unwrap();
    assert!(matches!(collect(&source.enroll(), now()), Err(Error::UnsafeState)));
}

#[test]
fn sqlite_deadline_and_source_size_limits_refuse_without_partial_claims() {
    let directory = Directory::new();
    let writer = directory.old();
    insert(&writer, now(), 42);
    assert!(matches!(
        Reader::new(writer.serialized(), Instant::now() - Duration::from_secs(1)),
        Err(Error::Limit)
    ));
    assert!(matches!(
        sqlite_image::capture(&directory.root(), 1, Instant::now() + Duration::from_secs(1)),
        Err(Error::Limit)
    ));
}

#[test]
fn sqlite_model_rows_keep_sql_session_hints_without_exposing_ids() {
    let directory = Directory::new();
    let writer = directory.old();
    let at = now();
    for session in ["PRIVATE_SQL_SESSION_A", "PRIVATE_SQL_SESSION_A", "PRIVATE_SQL_SESSION_B"] {
        writer.execute(&format!(
            "INSERT INTO message VALUES('{session}',{},{},'{}')",
            at * 1000,
            at * 1000,
            record(at, 42)
        ));
    }
    drop(writer);
    let report = collect_dir(&directory);
    assert_eq!(report.models[0]["sessions"], 2);
    assert_eq!(report.models[0]["total"], 126);
    assert_eq!(report.models[0]["totalWithCacheRead"], 2826);
    assert_eq!(report.models[0]["sessions_status"], "source_identified");
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_SQL"));
    let directory = Directory::new();
    let writer = Writer::create(&directory.0.join("opencode.db"));
    writer.execute(
        "CREATE TABLE session_message(role TEXT,created_at INTEGER,sessionID TEXT,data TEXT)",
    );
    let row = json!({"model":{"id":"v2","providerID":"test"},"tokens":{"input":9},"cost":0.0})
        .to_string();
    writer.execute(&format!(
        "INSERT INTO session_message VALUES('assistant',{},'PRIVATE_V2_SESSION','{row}')",
        at * 1000
    ));
    drop(writer);
    let report = collect_dir(&directory);
    assert_eq!(report.models[0]["sessions"], 1);
    assert_eq!(report.models[0]["cost_status"], "local_reported");
    assert_eq!(report.models[0]["costLabel"], "");
    assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_V2"));
}

#[test]
fn unknown_sql_sessions_are_labeled_and_model_counter_overflow_is_refused() {
    let directory = Directory::new();
    let writer = Writer::create(&directory.0.join("opencode.db"));
    let at = now();
    writer.execute("CREATE TABLE session_message(role TEXT,time_created INTEGER,data TEXT)");
    writer.execute(&format!(
        "INSERT INTO session_message VALUES('assistant',{},'{}')",
        at * 1000,
        record(at, 42)
    ));
    drop(writer);
    let report = collect_dir(&directory);
    assert_eq!(report.models[0]["sessions"], 1);
    assert_eq!(report.models[0]["sessions_status"], "includes_unknown");
    let directory = Directory::new();
    let writer = directory.old();
    writer.execute(&format!(
        "INSERT INTO message VALUES('s',{},{},'{}')",
        at * 1000,
        at * 1000,
        record(at, u64::MAX)
    ));
    drop(writer);
    assert!(matches!(collect(&directory.enroll(), at), Err(Error::Limit)));
}
