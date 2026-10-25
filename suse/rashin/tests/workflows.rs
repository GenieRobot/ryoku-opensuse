use ryoku_rashin::{
    Error,
    api::{Document, FixKind, Lane, Request, Response},
    provider::Provider,
    runtime::{Engine, serve_connection},
    store::Store,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixStream,
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rashin-workflows-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self { path }
    }
    fn engine(&self) -> Arc<Engine> {
        Engine::open(Store::open(&self.path).unwrap()).unwrap()
    }
    fn reopen(&self) -> Arc<Engine> {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match Store::open(&self.path) {
                Ok(store) => return Engine::open(store).unwrap(),
                Err(Error::Busy) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("reopen: {error:?}"),
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn rpc(engine: &Arc<Engine>, request: Request) -> Value {
    let (server, mut client) = UnixStream::pair().unwrap();
    let engine = Arc::clone(engine);
    let worker = std::thread::spawn(move || {
        serve_connection(server, &engine, rustix::process::geteuid().as_raw())
    });
    let bytes = serde_json::to_vec(&request).unwrap();
    client.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
    client.write_all(&bytes).unwrap();
    let mut length = [0; 4];
    client.read_exact(&mut length).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
    client.read_exact(&mut bytes).unwrap();
    worker.join().unwrap().unwrap();
    match serde_json::from_slice::<Response>(&bytes).unwrap() {
        Response::Ok { data } => data,
        Response::Error { error } => panic!("RPC error: {error:?}"),
    }
}

fn document() -> Document {
    let content = "Tumbleweed uses zypper for packages.\nInspect a user unit with systemctl --user status ryoku-rashin.service.".to_string();
    Document {
        name: "tumbleweed.md".into(),
        sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
        content,
    }
}

fn done(engine: &Arc<Engine>, id: &str) -> Value {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let job = rpc(engine, Request::Job { id: id.into() })["job"].clone();
        if job["status"] != "running" {
            return job;
        }
        assert!(Instant::now() < until, "provider did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn actual_socket_index_ask_history_and_usage_survive_restart() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Index { documents: vec![document()] });
    rpc(
        &engine,
        Request::Ask {
            id: "ask1".into(),
            prompt: "Which packages tool does Tumbleweed use?".into(),
        },
    );
    let answer = done(&engine, "ask1");
    assert_eq!(answer["status"], "completed");
    assert!(answer["answer"].as_str().unwrap().contains("zypper"));
    assert!(answer["answer"].as_str().unwrap().contains("tumbleweed.md"));
    assert!(rpc(&engine, Request::Sessions {})["sessions"].as_array().unwrap().is_empty());
    let usage = rpc(&engine, Request::Usage {});
    assert_eq!(usage["activity"][0]["estimated"], true);
    assert_eq!(usage["quota_status"], "unavailable");
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(rpc(&engine, Request::Recent {})["asks"][0]["job"]["answer"], answer["answer"]);
    assert_eq!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().len(), 1);
}

#[test]
fn agent_chat_and_private_ask_have_separate_persistent_transcripts() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Index { documents: vec![document()] });
    rpc(&engine, Request::NewSession { id: "agent".into(), lane: Lane::Agent });
    rpc(&engine, Request::NewSession { id: "chat".into(), lane: Lane::Chat });
    rpc(
        &engine,
        Request::Prompt {
            id: "agent1".into(),
            session: "agent".into(),
            prompt: "Tumbleweed packages".into(),
        },
    );
    assert_eq!(done(&engine, "agent1")["status"], "completed");
    rpc(&engine, Request::Ask { id: "ask1".into(), prompt: "private question".into() });
    done(&engine, "ask1");
    assert_eq!(
        rpc(&engine, Request::Session { id: "agent".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        rpc(&engine, Request::Session { id: "chat".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(rpc(&engine, Request::Recent {})["asks"].as_array().unwrap().len(), 1);
}

#[test]
fn fix_investigates_with_the_agent_route_without_executing_findings() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Index { documents: vec![document()] });
    rpc(
        &engine,
        Request::Fix {
            native_evidence_consent: None,
            id: "fix1".into(),
            kind: FixKind::App,
            component: "example-app".into(),
            evidence: "Tumbleweed packages lookup failed; $(touch /tmp/never-execute-rashin)"
                .into(),
        },
    );
    let job = done(&engine, "fix1");
    assert_eq!(job["lane"], "agent");
    assert!(job["answer"].as_str().unwrap().contains("zypper"));
    assert!(job["prompt"].as_str().unwrap().contains("verified linked recovery checkpoint"));
    assert!(rpc(&engine, Request::Recent {})["asks"].as_array().unwrap().is_empty());
    assert_eq!(
        engine.dispatch(Request::Fix {
            native_evidence_consent: None,
            id: "bad".into(),
            kind: FixKind::Doctor,
            component: "a;rm -rf /".into(),
            evidence: "failure".into()
        }),
        Err(Error::InvalidRequest)
    );
}

#[test]
fn local_agent_wiring_is_effective_and_can_be_removed() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(
        &engine,
        Request::Wire {
            instructions:
                "SOURCE local-skill: Use the reviewed Tumbleweed package ownership workflow.".into(),
        },
    );
    rpc(&engine, Request::NewSession { id: "agent".into(), lane: Lane::Agent });
    rpc(
        &engine,
        Request::Prompt {
            id: "wired".into(),
            session: "agent".into(),
            prompt: "Tumbleweed ownership".into(),
        },
    );
    assert!(done(&engine, "wired")["answer"].as_str().unwrap().contains("local-skill"));
    rpc(&engine, Request::Unwire {});
    rpc(
        &engine,
        Request::Prompt {
            id: "unwired".into(),
            session: "agent".into(),
            prompt: "Tumbleweed ownership".into(),
        },
    );
    assert!(!done(&engine, "unwired")["answer"].as_str().unwrap().contains("local-skill"));
}

#[test]
fn provider_keys_are_private_and_never_returned_by_inventory_or_errors() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let secret = "synthetic-test-key-not-a-real-credential";
    let provider = Provider::OpenAiCompatible {
        streaming: false,
        endpoint: "https://example.invalid/v1".into(),
        api_key: secret.into(),
        models: vec!["model-1".into()],
    };
    let response = rpc(&engine, Request::PutProvider { id: "example".into(), provider });
    assert!(!response.to_string().contains(secret));
    assert!(!rpc(&engine, Request::Providers {}).to_string().contains(secret));
    let state = fs::read(fixture.path.join("state.json")).unwrap();
    assert!(String::from_utf8(state).unwrap().contains(secret));
    assert_eq!(
        fs::metadata(fixture.path.join("state.json")).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    rpc(
        &engine,
        Request::SetRoute { lane: Lane::Ask, provider: "example".into(), model: "model-1".into() },
    );
    rpc(&engine, Request::RemoveProvider { id: "example".into() });
    assert_eq!(
        engine.dispatch(Request::Ask { id: "unavailable".into(), prompt: "hello".into() }),
        Err(Error::ProviderUnavailable)
    );
    assert_eq!(rpc(&engine, Request::Status {})["routes"]["ask"]["provider"], "example");
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Ask,
            provider: "local".into(),
            model: "local-retrieval-v1".into(),
        },
    );
    rpc(&engine, Request::Ask { id: "recovered".into(), prompt: "hello".into() });
    assert_eq!(done(&engine, "recovered")["status"], "completed");
}

#[test]
fn malformed_and_insecure_provider_sources_are_rejected() {
    for endpoint in [
        "http://example.com/v1",
        "file:///etc/passwd",
        "https://user:pass@example.com",
        "https://example.com#fragment",
        "https://example.com?key=secret",
        "https://example.com\nheader=x",
    ] {
        let provider = Provider::OpenAiCompatible {
            streaming: false,
            endpoint: endpoint.into(),
            api_key: "secret".into(),
            models: vec!["model".into()],
        };
        assert_eq!(provider.validate(), Err(Error::InvalidRequest), "{endpoint}");
    }
    let provider = Provider::OpenAiCompatible {
        streaming: false,
        endpoint: "https://example.com/v1".into(),
        api_key: "secret\nheader:bad".into(),
        models: vec!["model".into()],
    };
    assert_eq!(provider.validate(), Err(Error::InvalidRequest));
}

#[test]
fn index_hash_mismatch_never_replaces_the_published_index() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Index { documents: vec![document()] });
    let before = fs::read(fixture.path.join("state.json")).unwrap();
    let mut bad = document();
    bad.content.push_str("tampered");
    assert_eq!(engine.dispatch(Request::Index { documents: vec![bad] }), Err(Error::Integrity));
    assert_eq!(fs::read(fixture.path.join("state.json")).unwrap(), before);
}

#[test]
fn restarted_daemon_marks_inflight_work_interrupted_and_does_not_replay_it() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Ask { id: "crashed".into(), prompt: "hello".into() });
    done(&engine, "crashed");
    drop(engine);
    let engine = fixture.reopen();
    drop(engine);
    let path = fixture.path.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["jobs"]["crashed"]["status"] = json!("running");
    state["jobs"]["crashed"]["answer"] = Value::Null;
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let engine = fixture.reopen();
    assert_eq!(rpc(&engine, Request::Job { id: "crashed".into() })["job"]["status"], "interrupted");
    assert_eq!(
        rpc(&engine, Request::Ask { id: "crashed".into(), prompt: "hello".into() })["status"],
        "interrupted"
    );
    assert_eq!(
        engine.dispatch(Request::Ask { id: "crashed".into(), prompt: "different".into() }),
        Err(Error::Conflict)
    );
}

#[test]
fn cancelling_an_interrupted_or_completed_job_does_not_change_its_result() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Ask { id: "ask".into(), prompt: "hello".into() });
    let before = done(&engine, "ask");
    assert_eq!(rpc(&engine, Request::Cancel { id: "ask".into() })["status"], "completed");
    assert_eq!(rpc(&engine, Request::Job { id: "ask".into() })["job"], before);
}

#[test]
fn kernel_authorization_precedes_parsing_and_never_trusts_a_json_uid() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (server, mut client) = UnixStream::pair().unwrap();
    client.write_all(&u32::MAX.to_be_bytes()).unwrap();
    assert_eq!(
        serve_connection(server, &engine, rustix::process::geteuid().as_raw() + 1),
        Err(Error::Unauthorized)
    );
    assert!(serde_json::from_value::<Request>(json!({"action":"status","uid":1000})).is_err());
    assert!(serde_json::from_value::<Request>(json!({"action":"execute","command":"id"})).is_err());
}

#[test]
fn unsafe_paths_links_modes_and_duplicate_daemons_fail_closed() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.path).unwrap();
    assert!(matches!(Store::open(&fixture.path), Err(Error::Busy)));
    drop(store);
    let link = fixture.path.with_extension("link");
    symlink(&fixture.path, &link).unwrap();
    assert!(matches!(Store::open(&link), Err(Error::UnsafeState)));
    fs::remove_file(link).unwrap();
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(Store::open(&fixture.path), Err(Error::UnsafeState)));
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o700)).unwrap();
    symlink("/etc/passwd", fixture.path.join("state.json")).unwrap();
    assert!(matches!(fixture.engine_result(), Err(Error::UnsafeState)));
}

impl Fixture {
    fn engine_result(&self) -> ryoku_rashin::Result<Arc<Engine>> {
        // Another test's fork may briefly inherit a CLOEXEC lease. Preserve
        // the actual unsafe/integrity result once exec releases that lease.
        let until = Instant::now() + Duration::from_millis(100);
        loop {
            match Store::open(&self.path) {
                Ok(store) => return Engine::open(store),
                Err(Error::Busy) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[test]
fn state_hardlinks_and_tampered_index_are_not_consumed() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Index { documents: vec![document()] });
    drop(engine);
    fs::hard_link(fixture.path.join("state.json"), fixture.path.join("other")).unwrap();
    assert!(matches!(fixture.engine_result(), Err(Error::UnsafeState)));
    fs::remove_file(fixture.path.join("other")).unwrap();
    let path = fixture.path.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state["documents"][0]["content"] = json!("tampered");
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(matches!(fixture.engine_result(), Err(Error::Integrity)));
}

#[test]
fn oversized_socket_frames_are_rejected_before_allocation() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (server, mut client) = UnixStream::pair().unwrap();
    client.write_all(&u32::MAX.to_be_bytes()).unwrap();
    assert_eq!(
        serve_connection(server, &engine, rustix::process::geteuid().as_raw()),
        Err(Error::Limit)
    );
}

#[test]
fn stale_socket_recovery_never_unlinks_an_unrelated_file() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.path).unwrap();
    let listener = store.listener().unwrap();
    drop(listener);
    let listener = store.listener().unwrap();
    drop(listener);
    fs::remove_file(fixture.path.join("rashin.sock")).unwrap();
    fs::write(fixture.path.join("rashin.sock"), "sentinel").unwrap();
    assert!(matches!(store.listener(), Err(Error::UnsafeState)));
    assert_eq!(fs::read_to_string(fixture.path.join("rashin.sock")).unwrap(), "sentinel");
}

struct HttpBytes {
    input: std::io::Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl Read for HttpBytes {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(bytes)
    }
}
impl Write for HttpBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn http(
    gateway: &ryoku_rashin::http::Gateway,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
) -> String {
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:51321\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream =
        HttpBytes { input: std::io::Cursor::new(request.into_bytes()), output: Vec::new() };
    gateway.handle(&mut stream).unwrap();
    String::from_utf8(stream.output).unwrap()
}

#[test]
fn actual_http_api_requires_private_credentials_and_rejects_cross_origin_requests() {
    let fixture = Fixture::new();
    let store = Store::open(&fixture.path).unwrap();
    let token = store.browser_token().unwrap();
    assert_eq!(
        fs::metadata(fixture.path.join("browser.token")).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    let engine = Engine::open(store).unwrap();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        token.clone(),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let body = r#"{"action":"status"}"#;
    assert!(
        http(
            &gateway,
            "POST",
            "/api/action",
            "Content-Type: application/json\r\nX-Rashin-Client: console\r\n",
            body
        )
        .starts_with("HTTP/1.1 403")
    );
    let bearer = format!("Content-Type: application/json\r\nAuthorization: Bearer {token}\r\n");
    assert!(http(&gateway, "POST", "/api/action", &bearer, body).starts_with("HTTP/1.1 200"));
    assert!(
        http(
            &gateway,
            "POST",
            "/api/action",
            &format!("{bearer}Origin: https://attacker.invalid\r\n"),
            body
        )
        .starts_with("HTTP/1.1 403")
    );
    let cookie = format!("Content-Type: application/json\r\nCookie: rashin={token}\r\n");
    assert!(http(&gateway, "POST", "/api/action", &cookie, body).starts_with("HTTP/1.1 403"));
    assert!(
        http(
            &gateway,
            "POST",
            "/api/action",
            &format!("{cookie}Origin: http://127.0.0.1:51321\r\n"),
            body
        )
        .starts_with("HTTP/1.1 200")
    );
    let login = http(
        &gateway,
        "POST",
        "/login",
        "Origin: http://127.0.0.1:51321\r\n",
        &format!("token={token}"),
    );
    assert!(login.contains("HttpOnly; SameSite=Strict"));
    assert!(login.contains("Cache-Control: no-store"));
    assert!(login.contains("frame-ancestors 'none'"));
    assert!(!login.split("\r\n\r\n").nth(1).unwrap().contains(&token));
}

#[test]
fn http_index_ask_poll_and_browser_console_exercise_the_actual_engine() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let token = "1".repeat(64);
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        token.clone(),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let headers = format!("Content-Type: application/json\r\nAuthorization: Bearer {token}\r\n");
    let index = serde_json::to_string(&Request::Index { documents: vec![document()] }).unwrap();
    assert!(http(&gateway, "POST", "/api/action", &headers, &index).contains("tumbleweed.md"));
    assert!(http(&gateway, "POST", "/api/action", &headers, r#"{"action":"ask","id":"http-ask","prompt":"Tumbleweed packages <script>bad</script>"}"#).starts_with("HTTP/1.1 200"));
    assert_eq!(done(&engine, "http-ask")["status"], "completed");
    let poll =
        http(&gateway, "POST", "/api/action", &headers, r#"{"action":"job","id":"http-ask"}"#);
    assert!(poll.contains("zypper"));
    let console = http(&gateway, "GET", "/console", &format!("Cookie: rashin={token}\r\n"), "");
    assert!(console.contains("zypper"));
    assert!(console.contains("&lt;script&gt;bad&lt;/script&gt;"));
    assert!(!console.contains("<script>bad</script>"));
}

#[test]
fn http_rejects_request_smuggling_and_oversized_input() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway =
        ryoku_rashin::http::Gateway::new(engine, "1".repeat(64), "http://127.0.0.1:51321".into())
            .unwrap();
    for headers in
        ["Transfer-Encoding: chunked\r\n", "Content-Length: 999999999\r\n", "Content-Length: 0\r\n"]
    {
        assert!(http(&gateway, "POST", "/api/action", headers, "").starts_with("HTTP/1.1 400"));
    }
}

#[test]
fn per_action_policy_prevents_ask_clients_from_configuring_or_reading_agent_state() {
    use ryoku_rashin::authorization::Access;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    for request in [
        Request::Index { documents: vec![document()] },
        Request::Wire { instructions: "do things".into() },
        Request::Unwire {},
        Request::Usage {},
        Request::Providers {},
        Request::NewSession { id: "agent".into(), lane: Lane::Agent },
    ] {
        assert_eq!(engine.dispatch_as(Access::AskClient, request), Err(Error::Unauthorized));
    }
    rpc(
        &engine,
        Request::Fix {
            native_evidence_consent: None,
            id: "private-fix".into(),
            kind: FixKind::Doctor,
            component: "systemd".into(),
            evidence: "unit is down".into(),
        },
    );
    done(&engine, "private-fix");
    assert_eq!(
        engine.dispatch_as(Access::AskClient, Request::Job { id: "private-fix".into() }),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        engine.dispatch_as(Access::AskClient, Request::Cancel { id: "private-fix".into() }),
        Err(Error::Unauthorized)
    );
    assert!(
        engine
            .dispatch_as(
                Access::AskClient,
                Request::Ask { id: "own-ask".into(), prompt: "hello".into() }
            )
            .is_ok()
    );
    done(&engine, "own-ask");
}

#[test]
fn inflight_cancel_and_provider_failure_preserve_sessions_and_allow_recovery() {
    use ryoku_rashin::provider::FixtureOutcome;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(
        &engine,
        Request::PutProvider {
            id: "fixture".into(),
            provider: Provider::Fixture {
                delay_millis: 500,
                outcome: FixtureOutcome::Answer("fixture response".into()),
            },
        },
    );
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Agent,
            provider: "fixture".into(),
            model: "fixture-v1".into(),
        },
    );
    rpc(&engine, Request::NewSession { id: "agent".into(), lane: Lane::Agent });
    rpc(
        &engine,
        Request::Prompt {
            id: "cancel-me".into(),
            session: "agent".into(),
            prompt: "cancelled prompt".into(),
        },
    );
    assert_eq!(engine.dispatch(Request::ForgetJob { id: "cancel-me".into() }), Err(Error::Busy));
    assert_eq!(engine.dispatch(Request::DeleteSession { id: "agent".into() }), Err(Error::Busy));
    assert_eq!(rpc(&engine, Request::Cancel { id: "cancel-me".into() })["status"], "cancelled");
    assert_eq!(done(&engine, "cancel-me")["status"], "cancelled");
    assert!(
        rpc(&engine, Request::Session { id: "agent".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().is_empty());
    rpc(
        &engine,
        Request::PutProvider {
            id: "fixture".into(),
            provider: Provider::Fixture { delay_millis: 0, outcome: FixtureOutcome::Failure },
        },
    );
    rpc(
        &engine,
        Request::Prompt {
            id: "failure".into(),
            session: "agent".into(),
            prompt: "failed prompt".into(),
        },
    );
    let failed = done(&engine, "failure");
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"], "provider_failed");
    assert!(
        rpc(&engine, Request::Session { id: "agent".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    rpc(
        &engine,
        Request::PutProvider {
            id: "fixture".into(),
            provider: Provider::Fixture {
                delay_millis: 0,
                outcome: FixtureOutcome::Answer("recovered response".into()),
            },
        },
    );
    rpc(
        &engine,
        Request::Prompt {
            id: "recovered".into(),
            session: "agent".into(),
            prompt: "retry prompt".into(),
        },
    );
    assert_eq!(done(&engine, "recovered")["answer"], "recovered response");
    assert_eq!(rpc(&engine, Request::Usage {})["activity"][0]["fixture"], true);
    rpc(&engine, Request::DeleteSession { id: "agent".into() });
    assert_eq!(engine.dispatch(Request::Job { id: "recovered".into() }), Err(Error::NotFound));
    assert!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().is_empty());
    drop(engine);
    let engine = fixture.reopen();
    assert!(rpc(&engine, Request::Sessions {})["sessions"].as_array().unwrap().is_empty());
}

#[test]
fn storage_failure_never_reports_mutation_success_and_requires_restart() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    fs::create_dir(fixture.path.join("state.next")).unwrap();
    assert_eq!(
        engine.dispatch(Request::Wire { instructions: "not published".into() }),
        Err(Error::UnsafeState)
    );
    assert_eq!(engine.dispatch(Request::Status {}), Err(Error::Storage));
    fs::remove_dir(fixture.path.join("state.next")).unwrap();
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(rpc(&engine, Request::Status {})["wired"], false);
}

#[test]
fn external_client_runtime_dependency_is_declared_for_tumbleweed() {
    let spec = include_str!("../../../packaging/rpm/ryoku-desktop.spec");
    let runtime = spec
        .split("%package runtime\n")
        .nth(1)
        .unwrap()
        .split("%description runtime\n")
        .next()
        .unwrap();
    assert!(
        runtime
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["Requires:", "curl"])
    );
    assert!(
        runtime.lines().any(|line| line.split_whitespace().collect::<Vec<_>>()
            == ["Requires:", "rpm", ">=", "4.19"])
    );
    assert!(
        runtime
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["Requires:", "timezone"])
    );
    assert!(
        runtime.lines().any(
            |line| line.split_whitespace().collect::<Vec<_>>() == ["Requires:", "libsqlite3-0"]
        )
    );
}

#[test]
fn logs_api_is_read_only_bounded_and_unavailable_to_restricted_ask_clients() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let evidence =
        rpc(&engine, Request::Logs { component: "rashin-synthetic-test.service".into() });
    assert_eq!(evidence["component"], "rashin-synthetic-test.service");
    let observations = evidence["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 4);
    assert_eq!(observations[0]["kind"], "user_unit");
    assert_eq!(observations[1]["kind"], "user_journal");
    assert_eq!(observations[2]["kind"], "rpm_capability");
    assert_eq!(observations[3]["kind"], "rpm_file_owner");
    for observation in observations {
        assert!(observation["text"].as_str().unwrap().len() <= 3 * 32 * 1024);
    }
    assert_eq!(
        engine.dispatch_as(
            ryoku_rashin::authorization::Access::AskClient,
            Request::Logs { component: "curl".into() }
        ),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        engine.dispatch(Request::Logs { component: "--system".into() }),
        Err(Error::InvalidRequest)
    );
}

#[test]
fn fix_includes_native_evidence_and_keeps_idempotency_across_recollection() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let request = || Request::Fix {
        native_evidence_consent: None,
        id: "native-fix".into(),
        kind: FixKind::App,
        component: "rashin-synthetic-test.service".into(),
        evidence: "synthetic service error".into(),
    };
    rpc(&engine, request());
    let job = done(&engine, "native-fix");
    assert!(job["prompt"].as_str().unwrap().contains("rpm_file_owner"));
    assert!(job["prompt"].as_str().unwrap().contains("user_journal"));
    assert_eq!(rpc(&engine, request())["status"], "completed");
}

#[test]
fn logs_cli_uses_the_same_bounded_native_collector() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
        .args(["logs", "rashin-synthetic-test.service"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["observations"].as_array().unwrap().len(), 4);
    assert!(output.stderr.is_empty());
}

#[test]
fn built_binary_serves_real_tcp_and_cli_requests_without_autostart() {
    use std::process::{Command, Stdio};
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fixture = Fixture::new();
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
            .arg("web")
            .arg(&fixture.path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    use std::io::BufRead;
    std::io::BufReader::new(child.0.stderr.take().unwrap()).read_line(&mut line).unwrap();
    let origin = line.strip_prefix("Rashin: ").unwrap().split(';').next().unwrap();
    let address = origin.strip_prefix("http://").unwrap();
    let token = fs::read_to_string(fixture.path.join("browser.token")).unwrap();
    assert!(!line.contains(&token));
    let mut tcp = std::net::TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let body = r#"{"action":"ask","id":"executable","prompt":"offline fixture question"}"#;
    tcp.write_all(format!("POST /api/action HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
    let mut response = String::new();
    tcp.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("executable"));
    let mut client = Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
        .arg("request")
        .arg(fixture.path.join("rashin.sock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    client.stdin.take().unwrap().write_all(br#"{"action":"status"}"#).unwrap();
    let output = client.wait_with_output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["optional_at_login"], true);
    assert!(fixture.path.join("state.json").exists());
}

#[test]
fn canonical_ask_handoff_survives_restart_and_session_changes_then_is_consumed_once() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(
        &engine,
        Request::Ask { id: "source".into(), prompt: "PRIVATE_HANDOFF_QUESTION ] marker".into() },
    );
    let ask = done(&engine, "source");
    let staged = rpc(&engine, Request::Handoff { ask_id: "source".into(), run: None });
    assert_eq!(staged["pending_handoff"]["question"], ask["prompt"]);
    assert_eq!(staged["pending_handoff"]["answer"], ask["answer"]);
    rpc(&engine, Request::NewSession { id: "older-agent".into(), lane: Lane::Agent });
    drop(engine);
    let engine = fixture.reopen();
    rpc(&engine, Request::NewSession { id: "selected-agent".into(), lane: Lane::Agent });
    rpc(&engine, Request::NewSession { id: "plain-chat".into(), lane: Lane::Chat });
    rpc(
        &engine,
        Request::Prompt {
            id: "chat-turn".into(),
            session: "plain-chat".into(),
            prompt: "chat only".into(),
        },
    );
    assert!(done(&engine, "chat-turn")["handoff"].is_null());
    assert_eq!(rpc(&engine, Request::Sessions {})["pending_handoff"]["ask_id"], "source");
    rpc(
        &engine,
        Request::Prompt {
            id: "agent-turn".into(),
            session: "selected-agent".into(),
            prompt: "continue here".into(),
        },
    );
    let job = done(&engine, "agent-turn");
    assert_eq!(job["handoff"]["ask_id"], "source");
    assert!(job["prompt"].as_str().unwrap().contains("PRIVATE_HANDOFF_QUESTION ] marker"));
    assert!(job["prompt"].as_str().unwrap().contains("untrusted prior conversation data"));
    assert!(rpc(&engine, Request::Sessions {})["pending_handoff"].is_null());
    rpc(
        &engine,
        Request::Prompt {
            id: "next-turn".into(),
            session: "selected-agent".into(),
            prompt: "next question".into(),
        },
    );
    let next = done(&engine, "next-turn");
    assert!(next["handoff"].is_null());
    assert_eq!(next["prompt"], "next question");
    assert_eq!(rpc(&engine, Request::Recent {})["asks"].as_array().unwrap().len(), 1);
    assert!(
        rpc(&engine, Request::Session { id: "older-agent".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn immediate_handoff_is_atomic_and_idempotent_and_conflicts_do_not_restage_it() {
    use ryoku_rashin::api::HandoffRun;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(&engine, Request::Ask { id: "source".into(), prompt: "canonical source".into() });
    done(&engine, "source");
    rpc(&engine, Request::NewSession { id: "agent".into(), lane: Lane::Agent });
    rpc(&engine, Request::NewSession { id: "chat".into(), lane: Lane::Chat });
    let run = |session: &str| Request::Handoff {
        ask_id: "source".into(),
        run: Some(HandoffRun { id: "run-handoff".into(), session: session.into() }),
    };
    assert_eq!(engine.dispatch(run("chat")), Err(Error::InvalidRequest));
    assert!(rpc(&engine, Request::Sessions {})["pending_handoff"].is_null());
    rpc(&engine, run("agent"));
    let job = done(&engine, "run-handoff");
    assert_eq!(job["handoff"]["question"], "canonical source");
    let usage = rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().len();
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(rpc(&engine, run("agent"))["status"], "completed");
    assert_eq!(engine.dispatch(run("chat")), Err(Error::Conflict));
    assert!(rpc(&engine, Request::Sessions {})["pending_handoff"].is_null());
    assert_eq!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().len(), usage);
    assert_eq!(
        engine.dispatch(Request::Handoff { ask_id: "run-handoff".into(), run: None }),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        engine.dispatch_as(
            ryoku_rashin::authorization::Access::AskClient,
            Request::Handoff { ask_id: "source".into(), run: None }
        ),
        Err(Error::Unauthorized)
    );
    rpc(&engine, Request::Handoff { ask_id: "source".into(), run: None });
    rpc(&engine, Request::ClearHandoff {});
    assert!(rpc(&engine, Request::Sessions {})["pending_handoff"].is_null());
    rpc(&engine, Request::Handoff { ask_id: "source".into(), run: None });
    rpc(&engine, Request::ForgetJob { id: "source".into() });
    assert!(rpc(&engine, Request::Sessions {})["pending_handoff"].is_null());
}

#[test]
fn handoff_rejects_cancelled_sources_and_preserves_pending_on_busy_until_accepted_failure() {
    use ryoku_rashin::provider::FixtureOutcome;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(
        &engine,
        Request::PutProvider {
            id: "slow".into(),
            provider: Provider::Fixture { delay_millis: 1000, outcome: FixtureOutcome::Failure },
        },
    );
    rpc(
        &engine,
        Request::SetRoute { lane: Lane::Ask, provider: "slow".into(), model: "fixture-v1".into() },
    );
    rpc(&engine, Request::Ask { id: "unfinished".into(), prompt: "not complete".into() });
    assert_eq!(
        engine.dispatch(Request::Handoff { ask_id: "unfinished".into(), run: None }),
        Err(Error::Busy)
    );
    rpc(&engine, Request::Cancel { id: "unfinished".into() });
    assert_eq!(
        engine.dispatch(Request::Handoff { ask_id: "unfinished".into(), run: None }),
        Err(Error::Conflict)
    );
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Ask,
            provider: "local".into(),
            model: "local-retrieval-v1".into(),
        },
    );
    rpc(&engine, Request::Ask { id: "source".into(), prompt: "ready".into() });
    done(&engine, "source");
    rpc(&engine, Request::NewSession { id: "agent".into(), lane: Lane::Agent });
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Agent,
            provider: "slow".into(),
            model: "fixture-v1".into(),
        },
    );
    rpc(
        &engine,
        Request::Prompt { id: "busy".into(), session: "agent".into(), prompt: "wait".into() },
    );
    rpc(&engine, Request::Handoff { ask_id: "source".into(), run: None });
    assert_eq!(
        engine.dispatch(Request::Prompt {
            id: "blocked".into(),
            session: "agent".into(),
            prompt: "next".into()
        }),
        Err(Error::Busy)
    );
    assert_eq!(rpc(&engine, Request::Sessions {})["pending_handoff"]["ask_id"], "source");
    rpc(&engine, Request::Cancel { id: "busy".into() });
    rpc(
        &engine,
        Request::Prompt {
            id: "failed".into(),
            session: "agent".into(),
            prompt: "investigate".into(),
        },
    );
    assert_eq!(done(&engine, "failed")["status"], "failed");
    assert!(rpc(&engine, Request::Sessions {})["pending_handoff"].is_null());
    assert!(
        rpc(&engine, Request::Session { id: "agent".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fixture_stream_progress_is_private_ephemeral_cancellable_and_commits_only_complete_turns() {
    use ryoku_rashin::provider::FixtureOutcome;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let config = |fail| Request::PutProvider {
        id: "stream".into(),
        provider: Provider::Fixture {
            delay_millis: 600,
            outcome: FixtureOutcome::Stream {
                chunks: vec!["PRIVATE_PROGRESS_".into(), "Δelta ".into(), "answer".into()],
                fail,
            },
        },
    };
    rpc(&engine, config(false));
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Agent,
            provider: "stream".into(),
            model: "fixture-v1".into(),
        },
    );
    rpc(&engine, Request::NewSession { id: "stream-session".into(), lane: Lane::Agent });
    let prompt = |id: &str| Request::Prompt {
        id: id.into(),
        session: "stream-session".into(),
        prompt: "fixture question".into(),
    };
    rpc(&engine, prompt("cancel-stream"));
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let job = rpc(&engine, Request::Job { id: "cancel-stream".into() });
        if job["progress"].as_str().is_some_and(|text| text.contains("PRIVATE_PROGRESS_")) {
            assert_eq!(job["job"]["status"], "running");
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    let persisted: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert!(!persisted["jobs"].to_string().contains("PRIVATE_PROGRESS_"));
    assert!(persisted["jobs"]["cancel-stream"].get("progress").is_none());
    rpc(&engine, Request::Cancel { id: "cancel-stream".into() });
    assert!(rpc(&engine, Request::Job { id: "cancel-stream".into() })["progress"].is_null());
    assert_eq!(done(&engine, "cancel-stream")["status"], "cancelled");
    assert!(
        rpc(&engine, Request::Session { id: "stream-session".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().is_empty());
    rpc(&engine, config(true));
    rpc(&engine, prompt("fail-stream"));
    assert_eq!(done(&engine, "fail-stream")["status"], "failed");
    assert!(rpc(&engine, Request::Job { id: "fail-stream".into() })["progress"].is_null());
    assert!(
        rpc(&engine, Request::Session { id: "stream-session".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    rpc(&engine, config(false));
    rpc(&engine, prompt("complete-stream"));
    let job = done(&engine, "complete-stream");
    assert_eq!(job["answer"], "PRIVATE_PROGRESS_Δelta answer");
    assert!(rpc(&engine, Request::Job { id: "complete-stream".into() })["progress"].is_null());
    assert_eq!(
        rpc(&engine, Request::Session { id: "stream-session".into() })["session"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().len(), 1);
}
