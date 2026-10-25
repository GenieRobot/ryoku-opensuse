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
    assert!(
        state["jobs"]["crashed"].get("tool_activity").is_none(),
        "legacy jobs omit empty metadata"
    );
    state["jobs"]["crashed"]["tool_activity"] = json!([{"ordinal":0,"call_hash":"a".repeat(64),"name":"read_file","phase":"running","error":null}]);
    fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let engine = fixture.reopen();
    let interrupted = rpc(&engine, Request::Job { id: "crashed".into() });
    assert_eq!(interrupted["job"]["status"], "interrupted");
    assert_eq!(interrupted["job"]["tool_activity"][0]["phase"], "interrupted");
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let html = http(&gateway, "GET", "/job/crashed", BROWSER_AUTH, "");
    assert!(html.contains("<td>interrupted</td>"));
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
        .starts_with("HTTP/1.1 403")
    );
    let login = http(
        &gateway,
        "POST",
        "/login",
        "Origin: http://127.0.0.1:51321\r\n",
        &format!("token={token}"),
    );
    assert!(login.starts_with("HTTP/1.1 403"));
    assert!(!login.contains("Set-Cookie:"));
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
    let console = http(&gateway, "GET", "/console", &headers, "");
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
                catalog: None,
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
            provider: Provider::Fixture {
                catalog: None,
                delay_millis: 0,
                outcome: FixtureOutcome::Failure,
            },
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
                catalog: None,
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
    for package in [
        "zypper",
        "procps",
        "coreutils",
        "pciutils",
        "iproute2",
        "glibc",
        "/usr/bin/curl",
        "/usr/bin/getent",
        "/usr/bin/iconv",
    ] {
        assert!(
            runtime
                .lines()
                .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["Requires:", package])
        );
    }
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
fn handoff_stages_unfinished_but_rejects_cancelled_sources_and_preserves_pending_on_busy_until_accepted_failure()
 {
    use ryoku_rashin::provider::FixtureOutcome;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(
        &engine,
        Request::PutProvider {
            id: "slow".into(),
            provider: Provider::Fixture {
                catalog: None,
                delay_millis: 1000,
                outcome: FixtureOutcome::Failure,
            },
        },
    );
    rpc(
        &engine,
        Request::SetRoute { lane: Lane::Ask, provider: "slow".into(), model: "fixture-v1".into() },
    );
    rpc(&engine, Request::Ask { id: "unfinished".into(), prompt: "not complete".into() });
    assert_eq!(
        engine.dispatch(Request::Handoff { ask_id: "unfinished".into(), run: None }).unwrap()["pending_handoff"]
            ["origin"],
        "in_flight_ask"
    );
    assert_eq!(
        engine.dispatch(Request::Job { id: "unfinished".into() }).unwrap()["job"]["status"],
        "running"
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
            catalog: None,
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

#[test]
fn scoped_http_bearer_enforces_ask_role_and_cannot_inherit_owner_cookie_authority() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let owner = "a".repeat(64);
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        owner.clone(),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let grant = engine.dispatch(Request::IssueAskClient { lifetime_seconds: Some(60) }).unwrap();
    let token = grant["token"].as_str().unwrap();
    let id = grant["id"].as_str().unwrap();
    let scoped = format!(
        "Content-Type: application/json\r\nAuthorization: Bearer {token}\r\nCookie: rashin={owner}\r\nOrigin: http://127.0.0.1:51321\r\n"
    );
    let status = http(&gateway, "POST", "/api/action", &scoped, r#"{"action":"status"}"#);
    assert!(status.starts_with("HTTP/1.1 200"));
    let value: Value = serde_json::from_str(status.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(value["data"]["routes"]["agent"].is_null());
    assert!(value["data"].get("wired").is_none());
    for body in [
        r#"{"action":"providers"}"#,
        r#"{"action":"issue_ask_client"}"#,
        r#"{"action":"ask_clients"}"#,
        r#"{"action":"put_provider","id":"forbidden","provider":{"kind":"local"}}"#,
        r#"{"action":"update_provider","id":"forbidden","changes":{"streaming":true}}"#,
        r#"{"action":"refresh_models","id":"forbidden","provider":"local"}"#,
        r#"{"action":"new_session","id":"forbidden","lane":"agent"}"#,
        r#"{"action":"set_route","lane":"agent","provider":"local","model":"local-retrieval-v1"}"#,
        r#"{"action":"index","documents":[]}"#,
        r#"{"action":"wire","instructions":"forbidden"}"#,
        r#"{"action":"fix_preview","kind":"app","component":"synthetic.service","evidence":"finding"}"#,
        r#"{"action":"logs","component":"synthetic.service"}"#,
        r#"{"action":"enroll_usage_source","tool":"claude","path":"/not-consulted"}"#,
    ] {
        assert!(http(&gateway, "POST", "/api/action", &scoped, body).starts_with("HTTP/1.1 403"));
    }
    assert!(http(&gateway, "GET", "/console", &scoped, "").starts_with("HTTP/1.1 403"));
    let forged = r#"{"action":"ask","id":"forged","prompt":"question","access":"owner"}"#;
    assert!(http(&gateway, "POST", "/api/action", &scoped, forged).starts_with("HTTP/1.1 400"));
    let invalid = scoped.replace(token, &"b".repeat(64));
    assert!(
        http(&gateway, "POST", "/api/action", &invalid, r#"{"action":"status"}"#)
            .starts_with("HTTP/1.1 403")
    );
    assert!(
        http(
            &gateway,
            "POST",
            "/api/action",
            &scoped,
            r#"{"action":"ask","id":"scoped","prompt":"local evidence"}"#
        )
        .starts_with("HTTP/1.1 200")
    );
    done(&engine, "scoped");
    rpc(&engine, Request::NewSession { id: "private-agent".into(), lane: Lane::Agent });
    rpc(
        &engine,
        Request::Prompt {
            id: "private-turn".into(),
            session: "private-agent".into(),
            prompt: "PRIVATE_AGENT_EVIDENCE".into(),
        },
    );
    done(&engine, "private-turn");
    for action in ["job", "cancel"] {
        let body = json!({"action":action,"id":"private-turn"}).to_string();
        assert!(http(&gateway, "POST", "/api/action", &scoped, &body).starts_with("HTTP/1.1 403"));
    }
    let persisted = fs::read_to_string(fixture.path.join("state.json")).unwrap();
    assert!(!persisted.contains(token));
    assert!(!engine.dispatch(Request::AskClients {}).unwrap().to_string().contains(token));
    assert!(
        http(
            &gateway,
            "POST",
            "/api/action",
            &scoped,
            &json!({"action":"revoke_ask_client","id":id}).to_string()
        )
        .starts_with("HTTP/1.1 403")
    );
    engine.dispatch(Request::RevokeAskClient { id: id.into() }).unwrap();
    assert!(
        http(&gateway, "POST", "/api/action", &scoped, r#"{"action":"status"}"#)
            .starts_with("HTTP/1.1 403")
    );
}

#[test]
fn built_daemon_assigns_scoped_http_role_and_restart_invalidates_delegation() {
    use std::{
        net::TcpStream,
        process::{Child, Command, Stdio},
    };
    struct Daemon {
        child: Child,
        address: String,
        token: String,
    }
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    impl Daemon {
        fn start(root: &std::path::Path) -> Self {
            let child = Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
                .arg("web")
                .arg(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut daemon = Self { child, address: String::new(), token: String::new() };
            let mut startup = daemon.child.stderr.take().unwrap();
            rustix::fs::fcntl_setfl(&startup, rustix::fs::OFlags::NONBLOCK).unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 256];
                match startup.read(&mut chunk) {
                    Ok(0) => panic!("fixture daemon exited before readiness"),
                    Ok(count) => {
                        bytes.extend_from_slice(&chunk[..count]);
                        assert!(bytes.len() <= 2048);
                        if bytes.contains(&b'\n') {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("fixture readiness: {error}"),
                }
                assert!(Instant::now() < until, "fixture daemon readiness timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
            let line = std::str::from_utf8(&bytes).unwrap().lines().next().unwrap();
            let origin = line.strip_prefix("Rashin: ").unwrap().split(';').next().unwrap();
            daemon.address = origin.strip_prefix("http://").unwrap().into();
            daemon.token = fs::read_to_string(root.join("browser.token")).unwrap();
            daemon
        }

        fn request(&self, token: &str, body: &str) -> (String, Value) {
            let mut stream = TcpStream::connect(&self.address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            write!(stream,"POST /api/action HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",self.address,token,body.len(),body).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            let (header, body) = response.split_once("\r\n\r\n").unwrap();
            (header.lines().next().unwrap().into(), serde_json::from_str(body).unwrap())
        }
    }
    let fixture = Fixture::new();
    let daemon = Daemon::start(&fixture.path);
    let (status, grant) =
        daemon.request(&daemon.token, r#"{"action":"issue_ask_client","lifetime_seconds":60}"#);
    assert!(status.contains("200"));
    let token = grant["data"]["token"].as_str().unwrap().to_owned();
    let (status, _) = daemon.request(
        &token,
        r#"{"action":"ask","id":"scoped-live","prompt":"offline local question"}"#,
    );
    assert!(status.contains("200"));
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let (status, result) = daemon.request(&token, r#"{"action":"job","id":"scoped-live"}"#);
        assert!(status.contains("200"));
        if result["data"]["job"]["status"] == "completed" {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(daemon.request(&token, r#"{"action":"issue_ask_client"}"#).0.contains("403"));
    drop(daemon);
    let daemon = Daemon::start(&fixture.path);
    assert!(daemon.request(&token, r#"{"action":"recent"}"#).0.contains("403"));
    let (status, recent) = daemon.request(&daemon.token, r#"{"action":"recent"}"#);
    assert!(status.contains("200"));
    assert_eq!(recent["data"]["asks"][0]["id"], "scoped-live");
}

#[test]
fn partial_provider_edits_retain_private_key_opaque_ids_and_explicit_unavailable_picks() {
    use ryoku_rashin::provider::ProviderEdit;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let secret = "PRIVATE_RETAINED_PROVIDER_KEY";
    rpc(
        &engine,
        Request::PutProvider {
            id: "editable".into(),
            provider: Provider::OpenAiCompatible {
                endpoint: "https://example.invalid/v1".into(),
                api_key: secret.into(),
                models: vec!["original".into()],
                streaming: false,
            },
        },
    );
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Ask,
            provider: "editable".into(),
            model: "original".into(),
        },
    );
    let opaque = "π/模型@2026#fast?profile=one&label=\"quoted\"";
    let description = rpc(
        &engine,
        Request::UpdateProvider {
            id: "editable".into(),
            changes: ProviderEdit {
                models: Some(vec![opaque.into()]),
                streaming: Some(true),
                ..ProviderEdit::default()
            },
        },
    );
    assert_eq!(description["models"][0], opaque);
    assert_eq!(description["streaming"], true);
    assert!(!description.to_string().contains(secret));
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["providers"]["editable"]["api_key"], secret);
    assert_eq!(state["providers"]["editable"]["endpoint"], "https://example.invalid/v1");
    assert_eq!(rpc(&engine, Request::Status {})["routes"]["ask"]["model"], "original");
    assert_eq!(
        engine.dispatch(Request::Ask {
            id: "unavailable-pick".into(),
            prompt: "do not route elsewhere".into()
        }),
        Err(Error::ProviderUnavailable)
    );
    rpc(
        &engine,
        Request::SetRoute { lane: Lane::Ask, provider: "editable".into(), model: opaque.into() },
    );
    rpc(
        &engine,
        Request::UpdateProvider {
            id: "editable".into(),
            changes: ProviderEdit {
                endpoint: Some("https://replacement.invalid/v1".into()),
                ..ProviderEdit::default()
            },
        },
    );
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["providers"]["editable"]["api_key"], secret);
    assert_eq!(state["providers"]["editable"]["models"][0], opaque);
    assert_eq!(state["routes"]["ask"]["model"], opaque);
    assert_eq!(state["routes"]["agent"]["provider"], "local");
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(rpc(&engine, Request::Status {})["routes"]["ask"]["model"], opaque);
    assert!(!rpc(&engine, Request::Providers {}).to_string().contains(secret));
}

#[test]
fn invalid_partial_provider_edits_roll_back_without_secret_loss_or_role_escalation() {
    use ryoku_rashin::provider::ProviderEdit;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    rpc(
        &engine,
        Request::PutProvider {
            id: "editable".into(),
            provider: Provider::OpenAiCompatible {
                endpoint: "https://example.invalid/v1".into(),
                api_key: "PRIVATE_KEY".into(),
                models: vec!["model".into()],
                streaming: false,
            },
        },
    );
    let before = fs::read(fixture.path.join("state.json")).unwrap();
    let bad = [
        ProviderEdit::default(),
        ProviderEdit { api_key: Some(String::new()), ..ProviderEdit::default() },
        ProviderEdit {
            endpoint: Some("http://insecure.invalid".into()),
            ..ProviderEdit::default()
        },
        ProviderEdit { models: Some(vec!["control\nmodel".into()]), ..ProviderEdit::default() },
    ];
    for changes in bad {
        assert_eq!(
            engine.dispatch(Request::UpdateProvider { id: "editable".into(), changes }),
            Err(Error::InvalidRequest)
        );
        assert_eq!(fs::read(fixture.path.join("state.json")).unwrap(), before);
    }
    assert_eq!(
        engine.dispatch(Request::UpdateProvider {
            id: "local".into(),
            changes: ProviderEdit { streaming: Some(true), ..ProviderEdit::default() }
        }),
        Err(Error::Conflict)
    );
    assert_eq!(
        engine.dispatch_as(
            ryoku_rashin::authorization::Access::AskClient,
            Request::UpdateProvider {
                id: "editable".into(),
                changes: ProviderEdit { streaming: Some(true), ..ProviderEdit::default() }
            }
        ),
        Err(Error::Unauthorized)
    );
    let changed = rpc(
        &engine,
        Request::UpdateProvider {
            id: "editable".into(),
            changes: ProviderEdit {
                api_key: Some("ROTATED_PRIVATE_KEY".into()),
                ..ProviderEdit::default()
            },
        },
    );
    assert!(!changed.to_string().contains("ROTATED_PRIVATE_KEY"));
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["providers"]["editable"]["api_key"], "ROTATED_PRIVATE_KEY");
}

#[test]
fn asynchronous_catalog_refresh_preserves_picks_and_handles_empty_failure_and_cancellation() {
    use ryoku_rashin::provider::{FixtureCatalog, FixtureOutcome};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let config = |offered: Vec<String>, fail| Request::PutProvider {
        id: "catalog".into(),
        provider: Provider::Fixture {
            delay_millis: 300,
            outcome: FixtureOutcome::Answer("fixture".into()),
            catalog: Some(FixtureCatalog { cached: vec!["saved".into()], offered, fail }),
        },
    };
    rpc(&engine, config(vec!["模型@fresh#one".into(), "second".into()], false));
    rpc(
        &engine,
        Request::SetRoute { lane: Lane::Ask, provider: "catalog".into(), model: "saved".into() },
    );
    let refresh = |id: &str| Request::RefreshModels { id: id.into(), provider: "catalog".into() };
    rpc(&engine, refresh("catalog-success"));
    let job = done(&engine, "catalog-success");
    assert_eq!(job["status"], "completed");
    assert_eq!(job["operation"], "catalog_refresh");
    assert_eq!(job["catalog_count"], 2);
    assert!(job["answer"].is_null());
    assert!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().is_empty());
    assert!(rpc(&engine, Request::Sessions {})["sessions"].as_array().unwrap().is_empty());
    assert!(rpc(&engine, Request::Recent {})["asks"].as_array().unwrap().is_empty());
    assert_eq!(rpc(&engine, Request::Status {})["routes"]["ask"]["model"], "saved");
    assert_eq!(
        engine.dispatch(Request::Ask {
            id: "unavailable-after-refresh".into(),
            prompt: "no fallback".into()
        }),
        Err(Error::ProviderUnavailable)
    );
    let provider = rpc(&engine, Request::Providers {});
    assert_eq!(provider["providers"][0]["models"][0], "模型@fresh#one");
    rpc(&engine, config(vec![], false));
    rpc(&engine, refresh("catalog-empty"));
    assert_eq!(done(&engine, "catalog-empty")["catalog_count"], 0);
    drop(engine);
    let engine = fixture.reopen();
    assert!(
        rpc(&engine, Request::Providers {})["providers"][0]["models"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    rpc(&engine, config(vec!["not-published".into()], true));
    rpc(&engine, refresh("catalog-fail"));
    assert_eq!(done(&engine, "catalog-fail")["status"], "failed");
    assert_eq!(rpc(&engine, Request::Providers {})["providers"][0]["models"][0], "saved");
    rpc(&engine, config(vec!["not-published".into()], false));
    rpc(&engine, refresh("catalog-cancel"));
    rpc(&engine, Request::Cancel { id: "catalog-cancel".into() });
    assert_eq!(done(&engine, "catalog-cancel")["status"], "cancelled");
    assert_eq!(rpc(&engine, Request::Providers {})["providers"][0]["models"][0], "saved");
    assert_eq!(
        engine.dispatch_as(ryoku_rashin::authorization::Access::AskClient, refresh("denied")),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        engine.dispatch_as(
            ryoku_rashin::authorization::Access::AskClient,
            Request::Job { id: "catalog-success".into() }
        ),
        Err(Error::Unauthorized)
    );
}

#[test]
fn catalog_results_bind_provider_generation_and_allow_independent_provider_refresh() {
    use ryoku_rashin::provider::{FixtureCatalog, FixtureOutcome};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let config = |id: &str, cached: &str, offered: &str| Request::PutProvider {
        id: id.into(),
        provider: Provider::Fixture {
            delay_millis: 500,
            outcome: FixtureOutcome::Answer("fixture".into()),
            catalog: Some(FixtureCatalog {
                cached: vec![cached.into()],
                offered: vec![offered.into()],
                fail: false,
            }),
        },
    };
    rpc(&engine, config("one", "original", "old-server-response"));
    rpc(&engine, Request::RefreshModels { id: "stale".into(), provider: "one".into() });
    rpc(
        &engine,
        Request::PutProvider {
            id: "one".into(),
            provider: Provider::OpenAiCompatible {
                endpoint: "https://replacement.invalid/v1".into(),
                api_key: "PRIVATE_NEW_GENERATION_KEY".into(),
                models: vec!["new-owner-setting".into()],
                streaming: true,
            },
        },
    );
    let stale = done(&engine, "stale");
    assert_eq!(stale["status"], "failed");
    assert_eq!(stale["error"], "conflict");
    let listed = rpc(&engine, Request::Providers {});
    assert_eq!(listed["providers"][1]["models"][0], "new-owner-setting");
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["providers"]["one"]["api_key"], "PRIVATE_NEW_GENERATION_KEY");
    assert_eq!(state["providers"]["one"]["endpoint"], "https://replacement.invalid/v1");
    rpc(&engine, config("one", "new-owner-setting", "new-server-response"));
    rpc(&engine, config("two", "two-old", "two-fresh"));
    rpc(&engine, Request::RefreshModels { id: "one-refresh".into(), provider: "one".into() });
    rpc(&engine, Request::RefreshModels { id: "two-refresh".into(), provider: "two".into() });
    rpc(
        &engine,
        Request::SetRoute {
            lane: Lane::Agent,
            provider: "one".into(),
            model: "new-owner-setting".into(),
        },
    );
    assert_eq!(done(&engine, "one-refresh")["status"], "completed");
    assert_eq!(done(&engine, "two-refresh")["status"], "completed");
    assert_eq!(rpc(&engine, Request::Status {})["routes"]["agent"]["model"], "new-owner-setting");
    let listed = rpc(&engine, Request::Providers {});
    assert_eq!(listed["providers"][1]["models"][0], "new-server-response");
    assert_eq!(listed["providers"][2]["models"][0], "two-fresh");
    assert!(rpc(&engine, Request::Usage {})["activity"].as_array().unwrap().is_empty());
}

// Independent known canonical encoding of owner plus 64 ASCII ones; never a
// real credential. Production does not decode arbitrary Basic values.
const BROWSER_AUTH: &str = "Authorization: Basic b3duZXI6MTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMQ==\r\n";
fn ui_form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(key, value)| {
            let value: String = value.bytes().map(|byte| format!("%{byte:02X}")).collect();
            format!("{key}={value}")
        })
        .collect::<Vec<_>>()
        .join("&")
}
fn ui_submit(gateway: &ryoku_rashin::http::Gateway, fields: &[(&str, &str)]) -> String {
    http(
        gateway,
        "POST",
        "/console/action",
        &format!(
            "{BROWSER_AUTH}Origin: http://127.0.0.1:51321\r\nContent-Type: application/x-www-form-urlencoded\r\n"
        ),
        &ui_form(fields),
    )
}
#[test]
fn browser_authority_is_cookie_free_and_origin_bound_with_bearer_api_preserved() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    for path in ["/console", "/login", "/session/private", "/job/private"] {
        let response = http(&gateway, "GET", path, "", "");
        assert!(response.starts_with("HTTP/1.1 401"));
        assert!(response.contains(
            "WWW-Authenticate: Basic realm=\"Rashin http://127.0.0.1:51321\", charset=\"UTF-8\""
        ));
        assert!(!response.contains("Set-Cookie:"));
        assert!(!response.contains(&"1".repeat(64)));
    }
    let valid = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(valid.starts_with("HTTP/1.1 200"));
    assert!(!valid.contains("Set-Cookie:"));
    assert!(!valid.contains("b3duZXI6"));
    assert!(valid.contains("<h2>Usage</h2>"));
    assert!(valid.contains("quota_status"));
    assert!(valid.contains("<h2>Status</h2>"));
    let login = http(&gateway, "GET", "/login", BROWSER_AUTH, "");
    assert!(login.starts_with("HTTP/1.1 303"));
    assert!(login.contains("Location: /console\r\n"));
    for headers in [
        format!("Cookie: rashin={}\r\n", "1".repeat(64)),
        format!("Cookie: rashin={}\r\nOrigin: http://127.0.0.1:51321\r\n", "1".repeat(64)),
        "Authorization: Basic b3duZXI6c2hvcnQ=\r\n".into(),
        BROWSER_AUTH.replace("Basic ", "Basic  "),
        BROWSER_AUTH.replace("==", "="),
        BROWSER_AUTH.replace("b3duZXI6", "d3Jvbmc6"),
    ] {
        assert!(!http(&gateway, "GET", "/console", &headers, "").starts_with("HTTP/1.1 200"));
        let headers = format!("{headers}Content-Type: application/json\r\n");
        assert!(
            http(&gateway, "POST", "/api/action", &headers, r#"{"action":"status"}"#)
                .starts_with("HTTP/1.1 403")
        );
    }
    let fields = ui_form(&[("action", "new_session"), ("id", "forged"), ("lane", "agent")]);
    for headers in [
        BROWSER_AUTH.into(),
        format!("{BROWSER_AUTH}Origin: http://127.0.0.1:51322\r\n"),
        format!("{BROWSER_AUTH}Origin: null\r\n"),
    ] {
        assert!(
            http(&gateway, "POST", "/console/action", &headers, &fields)
                .starts_with("HTTP/1.1 403")
        );
    }
    let challenge = http(
        &gateway,
        "POST",
        "/console/action",
        "Origin: http://127.0.0.1:51321\r\nContent-Type: application/x-www-form-urlencoded\r\n",
        &fields,
    );
    assert!(challenge.starts_with("HTTP/1.1 401"));
    assert!(challenge.contains("WWW-Authenticate: Basic"));
    assert_eq!(engine.dispatch(Request::Sessions {}).unwrap()["sessions"], json!([]));
    let basic_json = format!("{BROWSER_AUTH}Content-Type: application/json\r\n");
    assert!(
        http(&gateway, "POST", "/api/action", &basic_json, r#"{"action":"status"}"#)
            .starts_with("HTTP/1.1 403")
    );
    assert!(
        http(
            &gateway,
            "POST",
            "/api/action",
            &format!("{basic_json}Origin: http://127.0.0.1:51321\r\n"),
            r#"{"action":"status"}"#
        )
        .starts_with("HTTP/1.1 200")
    );
    let bearer =
        format!("Authorization: Bearer {}\r\nContent-Type: application/json\r\n", "1".repeat(64));
    assert!(
        http(&gateway, "POST", "/api/action", &bearer, r#"{"action":"status"}"#)
            .starts_with("HTTP/1.1 200")
    );
    let scoped =
        engine.dispatch(Request::IssueAskClient { lifetime_seconds: Some(60) }).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
    let forbidden = format!("Authorization: Basic {scoped}\r\n");
    assert!(!http(&gateway, "GET", "/console", &forbidden, "").starts_with("HTTP/1.1 200"));
    let scoped = format!("Authorization: Bearer {scoped}\r\nContent-Type: application/json\r\n");
    assert!(
        http(&gateway, "POST", "/api/action", &scoped, r#"{"action":"jobs"}"#)
            .starts_with("HTTP/1.1 403")
    );
}

#[test]
fn no_javascript_provider_model_and_session_forms_preserve_private_workflows() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let key = "private-ui-fixture-key-DO-NOT-RENDER";
    let model = "org/model <candidate> \"opaque\"";
    let created = ui_submit(
        &gateway,
        &[
            ("action", "put_provider"),
            ("provider", "external"),
            ("endpoint", "https://provider.invalid/v1"),
            ("api_key", key),
            ("models", model),
            ("streaming", "true"),
        ],
    );
    assert!(created.starts_with("HTTP/1.1 303"));
    let home = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(home.contains("New API key (leave blank to retain current key)"));
    assert!(!home.contains(key));
    assert!(!home.contains(model));
    assert!(home.contains("org/model &lt;candidate&gt; &quot;opaque&quot;"));
    assert!(!home.contains("<script"));
    let choice = json!({"provider":"external","model":model}).to_string();
    assert!(
        ui_submit(&gateway, &[("action", "set_route"), ("lane", "chat"), ("choice", &choice)])
            .starts_with("HTTP/1.1 303")
    );
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "update_provider"),
                ("provider", "external"),
                ("endpoint", "https://provider.invalid/v2"),
                ("api_key", ""),
                ("models", "new offered")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    assert_eq!(engine.dispatch(Request::Status {}).unwrap()["routes"]["chat"]["model"], model);
    let home = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(home.contains("Saved pick unavailable: external"));
    let state = fs::read_to_string(fixture.path.join("state.json")).unwrap();
    assert!(state.contains(key), "blank key preserves existing private credential");
    assert!(!home.contains(key));
    let choice = json!({"provider":"local","model":"local-retrieval-v1"}).to_string();
    assert!(
        ui_submit(&gateway, &[("action", "set_route"), ("lane", "chat"), ("choice", &choice)])
            .starts_with("HTTP/1.1 303")
    );
    let session =
        ui_submit(&gateway, &[("action", "new_session"), ("id", "browser-chat"), ("lane", "chat")]);
    assert!(session.contains("Location: /session/browser-chat\r\n"));
    engine.dispatch(Request::Index { documents: vec![document()] }).unwrap();
    let prompt = ui_submit(
        &gateway,
        &[
            ("action", "prompt"),
            ("id", "browser-job"),
            ("session", "browser-chat"),
            ("prompt", "Tumbleweed packages <script>never execute</script>"),
        ],
    );
    assert!(prompt.contains("Location: /job/browser-job\r\n"));
    assert_eq!(done(&engine, "browser-job")["status"], "completed");
    let conversation = http(&gateway, "GET", "/session/browser-chat", BROWSER_AUTH, "");
    assert!(conversation.contains("zypper"));
    assert!(conversation.contains("&lt;script&gt;never execute&lt;/script&gt;"));
    assert!(!conversation.contains("<script>"));
    let job = http(&gateway, "GET", "/job/browser-job", BROWSER_AUTH, "");
    assert!(job.contains("zypper"));
    assert!(!job.contains("http-equiv=\"refresh\""));
    assert!(
        ui_submit(&gateway, &[("action", "forget"), ("id", "browser-job")])
            .starts_with("HTTP/1.1 303")
    );
    assert!(
        ui_submit(&gateway, &[("action", "delete_session"), ("session", "browser-chat")])
            .starts_with("HTTP/1.1 303")
    );
    assert!(
        ui_submit(&gateway, &[("action", "remove_provider"), ("provider", "external")])
            .starts_with("HTTP/1.1 303")
    );
    assert_eq!(engine.dispatch(Request::Sessions {}).unwrap()["sessions"], json!([]));
}

#[test]
fn browser_jobs_render_ephemeral_streaming_cancel_and_catalog_results() {
    use ryoku_rashin::provider::{FixtureCatalog, FixtureOutcome};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine
        .dispatch(Request::PutProvider {
            id: "ui-fixture".into(),
            provider: Provider::Fixture {
                delay_millis: 600,
                outcome: FixtureOutcome::Stream {
                    chunks: vec!["<partial>".into(), "answer".into()],
                    fail: false,
                },
                catalog: Some(FixtureCatalog {
                    cached: vec!["fixture-v1".into()],
                    offered: vec!["catalog<new>".into()],
                    fail: false,
                }),
            },
        })
        .unwrap();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let choice = json!({"provider":"ui-fixture","model":"fixture-v1"}).to_string();
    assert!(
        ui_submit(&gateway, &[("action", "set_route"), ("lane", "agent"), ("choice", &choice)])
            .starts_with("HTTP/1.1 303")
    );
    assert!(
        ui_submit(&gateway, &[("action", "new_session"), ("id", "ui-agent"), ("lane", "agent")])
            .starts_with("HTTP/1.1 303")
    );
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "prompt"),
                ("id", "ui-stream"),
                ("session", "ui-agent"),
                ("prompt", "stream fixture")
            ]
        )
        .contains("Location: /job/ui-stream\r\n")
    );
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let work = http(&gateway, "GET", "/job/ui-stream", BROWSER_AUTH, "");
        if work.contains("&lt;partial&gt;") {
            assert!(work.contains("Receiving response"));
            assert!(work.contains("http-equiv=\"refresh\""));
            assert!(work.contains(">Cancel</button>"));
            assert!(!work.contains("<partial>"));
            break;
        }
        assert!(Instant::now() < until, "stream progress missing from browser job");
        std::thread::sleep(Duration::from_millis(2));
    }
    let persisted: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert!(!persisted["jobs"]["ui-stream"].to_string().contains("<partial>"));
    assert!(persisted["sessions"]["ui-agent"]["messages"].as_array().unwrap().is_empty());
    assert!(
        ui_submit(&gateway, &[("action", "cancel"), ("id", "ui-stream")])
            .starts_with("HTTP/1.1 303")
    );
    let work = http(&gateway, "GET", "/job/ui-stream", BROWSER_AUTH, "");
    assert!(work.contains("cancelled"));
    assert!(!work.contains("&lt;partial&gt;"));
    assert!(!work.contains("http-equiv=\"refresh\""));
    assert!(
        engine.dispatch(Request::Session { id: "ui-agent".into() }).unwrap()["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        ui_submit(
            &gateway,
            &[("action", "refresh_models"), ("id", "ui-catalog"), ("provider", "ui-fixture")]
        )
        .contains("Location: /job/ui-catalog\r\n")
    );
    assert_eq!(done(&engine, "ui-catalog")["status"], "completed");
    let work = http(&gateway, "GET", "/job/ui-catalog", BROWSER_AUTH, "");
    assert!(work.contains("Model catalog refresh · offered models: 1"));
    let home = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(home.contains("catalog&lt;new&gt;"));
    assert!(home.contains("Saved pick unavailable: ui-fixture"));
    assert!(!home.contains("<partial>"));
}

fn prowl_fixture(responses: Vec<(&'static str, u16, Value)>) -> (u16, std::thread::JoinHandle<()>) {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        for (path, status, value) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "Prowl fixture accept timeout");
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("Prowl fixture accept: {error}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                assert!(bytes.len() < 8192);
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let header = String::from_utf8(bytes).unwrap();
            assert!(header.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
            assert!(header.contains("Authorization: Bearer PRIVATE_PROWL_FIXTURE_TOKEN\r\n"));
            assert!(!header.contains("Cookie:"));
            let body = serde_json::to_string(&value).unwrap();
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (port, worker)
}
#[test]
fn typed_prowl_reads_use_real_authenticated_local_gateway_and_private_browser_forms() {
    use ryoku_rashin::prowl::Query;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::Profiles {} }),
        Err(Error::ProviderUnavailable)
    );
    assert_eq!(
        engine.dispatch(Request::ConfigureProwl { port: 8788, token: None }),
        Err(Error::InvalidRequest)
    );
    let profile = json!({"id":7,"name":"Fixture <profile>","strategy":"budget","emoji":"◇","color":"blue","type":"custom","is_favorite":0,"sort_order":0,"auto_sort":null,"layout_config":null,"auto_include_new_models":1,"modelCount":0,"created_at":"2026-10-25","management_key":"not-a-public-field"});
    let (port, server) = prowl_fixture(vec![
        ("/api/profiles", 200, json!([profile])),
        ("/api/profiles/active", 200, json!({"activeProfileId":7})),
        ("/api/profiles/7/models", 200, json!([])),
        ("/api/profiles/presets", 200, json!({"presets":[]})),
    ]);
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let port_string = port.to_string();
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "configure_prowl"),
                ("port", &port_string),
                ("token", "PRIVATE_PROWL_FIXTURE_TOKEN")
            ]
        )
        .contains("Location: /prowl\r\n")
    );
    assert!(
        ui_submit(
            &gateway,
            &[("action", "configure_prowl"), ("port", &port_string), ("token", "")]
        )
        .starts_with("HTTP/1.1 303")
    );
    let settings = http(&gateway, "GET", "/prowl", BROWSER_AUTH, "");
    assert!(settings.contains("Configured. Credentials remain private."));
    assert!(!settings.contains("PRIVATE_PROWL_FIXTURE_TOKEN"));
    let profiles = http(&gateway, "GET", "/prowl/profiles", BROWSER_AUTH, "");
    assert!(profiles.contains("Fixture &lt;profile&gt;"));
    assert!(profiles.contains("href=\"/prowl/models/7\""));
    assert!(!profiles.contains("management_key"));
    assert!(!profiles.contains("not-a-public-field"));
    assert!(!profiles.contains("PRIVATE_PROWL_FIXTURE_TOKEN"));
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::ActiveProfile {} }).unwrap()["result"]["activeProfileId"],
        7
    );
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::ProfileModels { id: 7 } }).unwrap()["result"],
        json!([])
    );
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::ProfilePresets {} }).unwrap()["result"],
        json!({"presets":[]})
    );
    server.join().unwrap();
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.path.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["prowl"]["token"], "PRIVATE_PROWL_FIXTURE_TOKEN");
    assert!(state["jobs"].as_object().unwrap().is_empty());
    assert!(state["usage"].as_array().unwrap().is_empty());
    assert!(ui_submit(&gateway, &[("action", "disable_prowl")]).starts_with("HTTP/1.1 303"));
    assert_eq!(engine.dispatch(Request::ProwlStatus {}).unwrap()["configured"], false);
}
#[test]
fn prowl_authority_http_errors_and_invalid_configuration_never_leak_or_fall_back() {
    use ryoku_rashin::{authorization::Access, prowl::Query};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    for request in [
        Request::ProwlStatus {},
        Request::ConfigureProwl { port: 1, token: Some("PRIVATE_PROWL_FIXTURE_TOKEN".into()) },
        Request::DisableProwl {},
        Request::ReadProwl { query: Query::Profiles {} },
    ] {
        assert_eq!(engine.dispatch_as(Access::AskClient, request), Err(Error::Unauthorized));
    }
    let (port, server) = prowl_fixture(vec![
        ("/api/profiles", 403, json!({"error":{"message":"PRIVATE_PROWL_FIXTURE_TOKEN"}})),
        ("/api/profiles/active", 200, json!({"error":{"message":"not-active"}})),
        ("/api/profiles", 302, json!([])),
    ]);
    engine
        .dispatch(Request::ConfigureProwl {
            port,
            token: Some("PRIVATE_PROWL_FIXTURE_TOKEN".into()),
        })
        .unwrap();
    assert_eq!(
        engine.dispatch(Request::ConfigureProwl { port: 0, token: None }),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        engine.dispatch(Request::ConfigureProwl { port, token: Some("bad\nheader".into()) }),
        Err(Error::InvalidRequest)
    );
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::Profiles {} }),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::ActiveProfile {} }),
        Err(Error::ProviderFailed)
    );
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::Profiles {} }),
        Err(Error::ProviderFailed)
    );
    server.join().unwrap();
    assert_eq!(
        engine.dispatch(Request::ReadProwl { query: Query::Profiles {} }),
        Err(Error::ProviderUnavailable)
    );
    assert_eq!(engine.dispatch(Request::ProwlStatus {}).unwrap()["port"], port);
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(engine.dispatch(Request::ProwlStatus {}).unwrap()["port"], port);
    engine.dispatch(Request::DisableProwl {}).unwrap();
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(engine.dispatch(Request::ProwlStatus {}).unwrap()["configured"], false);
}

fn held_prowl_fixture(
    response: Vec<u8>,
) -> (
    u16,
    std::sync::mpsc::Receiver<String>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    use std::{net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "held Prowl accept timeout");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("held Prowl accept: {error}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 8192);
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        entered_tx.send(String::from_utf8(header).unwrap()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let _ = stream.write_all(&response); // A timed-out client deliberately closes first.
    });
    (port, entered_rx, release_tx, worker)
}
#[test]
fn prowl_read_rejects_stale_configuration_and_transport_handles_chunked_bounded_time_and_size() {
    use ryoku_rashin::prowl::Query;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, entered, release, server) = held_prowl_fixture(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_vec(),
    );
    engine
        .dispatch(Request::ConfigureProwl {
            port,
            token: Some("PRIVATE_PROWL_FIXTURE_TOKEN".into()),
        })
        .unwrap();
    let reader = Arc::clone(&engine);
    let request = std::thread::spawn(move || {
        reader.dispatch(Request::ReadProwl { query: Query::Profiles {} })
    });
    let header = entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(header.starts_with("GET /api/profiles HTTP/1.1\r\n"));
    assert!(header.contains("Authorization: Bearer PRIVATE_PROWL_FIXTURE_TOKEN\r\n"));
    engine.dispatch(Request::ConfigureProwl { port: 1, token: None }).unwrap();
    release.send(()).unwrap();
    assert_eq!(request.join().unwrap(), Err(Error::Conflict));
    server.join().unwrap();
    for (response,should_timeout,expected) in [
        (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\n[]\r\n0\r\n\r\n".to_vec(),false,Ok(json!({"source":"configured-local-prowl","result":[]}))),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n".to_vec(),false,Err(Error::Limit)),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_vec(),true,Err(Error::ProviderUnavailable)),
    ] {
        let (port,entered,release,server)=held_prowl_fixture(response);
        engine.dispatch(Request::ConfigureProwl{port,token:None}).unwrap();
        let reader=Arc::clone(&engine);let start=Instant::now();
        let request=std::thread::spawn(move||reader.dispatch(Request::ReadProwl{query:Query::Profiles{}}));
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        if !should_timeout {release.send(()).unwrap();}
        assert_eq!(request.join().unwrap(),expected);
        assert!(start.elapsed()<Duration::from_secs(5),"Prowl client exceeded bounded request deadline");
        if should_timeout {release.send(()).unwrap();}
        server.join().unwrap();
    }
    assert!(engine.dispatch(Request::Jobs {}).unwrap()["jobs"].as_array().unwrap().is_empty());
    assert!(engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().is_empty());
}

#[test]
fn browser_handoff_stages_canonical_context_and_runs_only_in_selected_agent() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine.dispatch(Request::Index { documents: vec![document()] }).unwrap();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "ask"),
                ("id", "browser-source"),
                ("prompt", "Tumbleweed packages <question>")
            ]
        )
        .contains("Location: /job/browser-source\r\n")
    );
    let source = done(&engine, "browser-source");
    for (id, lane) in [("browser-agent", "agent"), ("browser-plain", "chat")] {
        assert!(
            ui_submit(&gateway, &[("action", "new_session"), ("id", id), ("lane", lane)])
                .starts_with("HTTP/1.1 303")
        );
    }
    let page = http(&gateway, "GET", "/job/browser-source", BROWSER_AUTH, "");
    assert!(page.contains("Stage context for next Agent message"));
    assert!(page.contains("value=\"browser-agent\""));
    assert!(!page.contains("value=\"browser-plain\""));
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "handoff"),
                ("ask_id", "browser-source"),
                ("question", "FORGED_QUESTION"),
                ("answer", "FORGED_ANSWER")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    let pending = engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"].clone();
    assert_eq!(pending["question"], source["prompt"]);
    assert_eq!(pending["answer"], source["answer"]);
    let home = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(home.contains("Pending context for next Agent message"));
    assert!(home.contains("&lt;question&gt;"));
    assert!(!home.contains("FORGED_ANSWER"));
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "prompt"),
                ("id", "browser-chat-turn"),
                ("session", "browser-plain"),
                ("prompt", "plain chat")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    assert!(done(&engine, "browser-chat-turn")["handoff"].is_null());
    assert_eq!(
        engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"]["ask_id"],
        "browser-source"
    );
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "prompt"),
                ("id", "browser-agent-turn"),
                ("session", "browser-agent"),
                ("prompt", "continue")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    assert_eq!(done(&engine, "browser-agent-turn")["handoff"]["ask_id"], "browser-source");
    assert!(engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"].is_null());
    assert!(
        ui_submit(&gateway, &[("action", "handoff"), ("ask_id", "browser-source")])
            .starts_with("HTTP/1.1 303")
    );
    assert!(ui_submit(&gateway, &[("action", "clear_handoff")]).starts_with("HTTP/1.1 303"));
    assert!(engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"].is_null());
    let fields = [
        ("action", "handoff_run"),
        ("ask_id", "browser-source"),
        ("id", "browser-immediate"),
        ("session", "browser-agent"),
    ];
    assert!(ui_submit(&gateway, &fields).contains("Location: /job/browser-immediate\r\n"));
    assert_eq!(done(&engine, "browser-immediate")["handoff"]["ask_id"], "browser-source");
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 303"));
    assert_eq!(engine.dispatch(Request::Session{id:"browser-agent".into()}).unwrap()["session"]["messages"].as_array().unwrap().len(),4);
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "handoff_run"),
                ("ask_id", "browser-source"),
                ("id", "wrong-chat"),
                ("session", "browser-plain")
            ]
        )
        .starts_with("HTTP/1.1 400")
    );
    assert!(engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"].is_null());
}

#[test]
fn browser_local_usage_forms_enroll_collect_label_failures_and_clear_private_activity() {
    let fixture = Fixture::new();
    let source = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let source_path = source.path.to_str().unwrap();
    fs::create_dir_all(source.path.join("projects/project")).unwrap();
    let log = source.path.join("projects/project/session.jsonl");
    fs::write(&log,json!({"timestamp":jiff::Timestamp::now().to_string(),"message":{"role":"assistant","model":"claude-browser","content":"PRIVATE_USAGE_TRANSCRIPT","usage":{"input_tokens":23}}}).to_string()).unwrap();
    fs::write(source.path.join(".credentials.json"), "PRIVATE_USAGE_KEY").unwrap();
    assert!(http(&gateway, "GET", "/usage", "", "").starts_with("HTTP/1.1 401"));
    let enrollment =
        ui_form(&[("action", "enroll_usage"), ("tool", "claude"), ("path", source_path)]);
    assert!(
        http(&gateway, "POST", "/console/action", BROWSER_AUTH, &enrollment)
            .starts_with("HTTP/1.1 403")
    );
    assert!(engine.dispatch(Request::Usage {}).unwrap()["sources"].as_object().unwrap().is_empty());
    assert!(
        ui_submit(
            &gateway,
            &[("action", "enroll_usage"), ("tool", "claude"), ("path", source_path)]
        )
        .contains("Location: /usage\r\n")
    );
    assert!(http(&gateway, "GET", "/usage", BROWSER_AUTH, "").contains("Not yet collected."));
    assert!(
        ui_submit(&gateway, &[("action", "collect_usage"), ("tool", "claude")])
            .starts_with("HTTP/1.1 303")
    );
    let page = http(&gateway, "GET", "/usage", BROWSER_AUTH, "");
    assert!(page.contains("Source status: fresh"));
    assert!(page.contains("Five-hour local tokens: 23"));
    assert!(page.contains("Provider quota: unavailable"));
    assert!(!page.contains("PRIVATE_USAGE_TRANSCRIPT"));
    assert!(!page.contains("PRIVATE_USAGE_KEY"));
    fs::write(&log, "malformed").unwrap();
    assert!(
        ui_submit(&gateway, &[("action", "collect_usage"), ("tool", "claude")])
            .starts_with("HTTP/1.1 303")
    );
    let page = http(&gateway, "GET", "/usage", BROWSER_AUTH, "");
    assert!(page.contains("Source status: stale"));
    assert!(page.contains("Five-hour local tokens: 23"));
    assert!(
        ui_submit(&gateway, &[("action", "disable_usage"), ("tool", "claude")])
            .starts_with("HTTP/1.1 303")
    );
    assert!(engine.dispatch(Request::Usage {}).unwrap()["sources"].as_object().unwrap().is_empty());
    assert!(engine.dispatch(Request::Usage {}).unwrap()["tools"].as_object().unwrap().is_empty());
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "enroll_usage"),
                ("tool", "opencode"),
                ("path", source_path),
                ("five_hour_limit", "123"),
                ("seven_day_limit", "456")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["sources"]["opencode"]["soft_caps"]["five_hour_limit"],
        123
    );
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "enroll_usage"),
                ("tool", "opencode"),
                ("path", source_path),
                ("five_hour_limit", "5")
            ]
        )
        .starts_with("HTTP/1.1 400")
    );
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["sources"]["opencode"]["soft_caps"]["five_hour_limit"],
        123
    );
}

#[test]
fn browser_wiring_is_effective_and_fix_review_requires_explicit_bound_confirmation() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "wire"),
                (
                    "instructions",
                    "SOURCE local-skill: Use the reviewed Tumbleweed package ownership workflow."
                )
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    assert!(
        http(&gateway, "GET", "/console", BROWSER_AUTH, "")
            .contains("Instructions are enabled for Agent messages")
    );
    ui_submit(&gateway, &[("action", "new_session"), ("id", "wired-ui"), ("lane", "agent")]);
    ui_submit(
        &gateway,
        &[
            ("action", "prompt"),
            ("id", "wired-turn"),
            ("session", "wired-ui"),
            ("prompt", "Tumbleweed ownership"),
        ],
    );
    assert!(done(&engine, "wired-turn")["answer"].as_str().unwrap().contains("local-skill"));
    assert!(ui_submit(&gateway, &[("action", "unwire")]).starts_with("HTTP/1.1 303"));
    assert_eq!(engine.dispatch(Request::Status {}).unwrap()["wired"], false);
    ui_submit(
        &gateway,
        &[
            ("action", "prompt"),
            ("id", "unwired-turn"),
            ("session", "wired-ui"),
            ("prompt", "Tumbleweed ownership"),
        ],
    );
    assert!(!done(&engine, "unwired-turn")["answer"].as_str().unwrap().contains("local-skill"));
    assert!(http(&gateway, "GET", "/fix", "", "").starts_with("HTTP/1.1 401"));
    let finding = [
        ("kind", "doctor"),
        ("component", "rashin-ui-fixture"),
        ("evidence", "Browser <finding> fixture"),
    ];
    let body = ui_form(&finding);
    assert!(
        http(&gateway, "POST", "/console/fix-preview", BROWSER_AUTH, &body)
            .starts_with("HTTP/1.1 403")
    );
    let headers = format!(
        "{BROWSER_AUTH}Origin: http://127.0.0.1:51321\r\nContent-Type: application/x-www-form-urlencoded\r\n"
    );
    let preview = http(&gateway, "POST", "/console/fix-preview", &headers, &body);
    assert!(preview.starts_with("HTTP/1.1 200"));
    assert!(preview.contains("Browser &lt;finding&gt; fixture"));
    assert!(preview.contains("Bounded native observations"));
    assert!(preview.contains("Review it before confirming."));
    assert!(!preview.contains("Set-Cookie:"));
    let hidden = |name: &str| {
        preview
            .split(&format!("name=\"{name}\" value=\""))
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string()
    };
    let consent = hidden("consent");
    let id = hidden("id");
    let mut fields = vec![
        ("action", "fix"),
        ("id", id.as_str()),
        ("kind", "doctor"),
        ("component", "rashin-ui-fixture"),
        ("evidence", "Browser <finding> fixture"),
        ("consent", consent.as_str()),
    ];
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 400"));
    assert_eq!(engine.dispatch(Request::Jobs {}).unwrap()["jobs"].as_array().unwrap().len(), 2);
    fields.push(("confirm", "yes"));
    assert!(ui_submit(&gateway, &fields).contains(&format!("Location: /job/{id}\r\n")));
    let fix = done(&engine, &id);
    assert_eq!(fix["status"], "completed");
    assert_eq!(fix["lane"], "agent");
    assert!(fix["prompt"].as_str().unwrap().contains("Browser <finding> fixture"));
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 303"));
    engine
        .dispatch(Request::PutProvider {
            id: "external-fix-ui".into(),
            provider: Provider::OpenAiCompatible {
                endpoint: "https://provider.invalid/v1".into(),
                api_key: "PRIVATE_EXTERNAL_FIX_UI_KEY".into(),
                models: vec!["model".into()],
                streaming: false,
            },
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Agent,
            provider: "external-fix-ui".into(),
            model: "model".into(),
        })
        .unwrap();
    let preview = http(&gateway, "POST", "/console/fix-preview", &headers, &body);
    assert!(preview.contains("provider.invalid"));
    assert!(!preview.contains("PRIVATE_EXTERNAL_FIX_UI_KEY"));
    let consent = preview
        .split("name=\"consent\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let mismatch = [
        ("action", "fix"),
        ("id", "forbidden-egress"),
        ("kind", "tip"),
        ("component", "rashin-ui-fixture"),
        ("evidence", "Browser <finding> fixture"),
        ("consent", consent.as_str()),
        ("confirm", "yes"),
    ];
    assert!(ui_submit(&gateway, &mismatch).contains("EvidenceConsentRequired"));
    assert_eq!(
        engine.dispatch(Request::Jobs {}).unwrap()["jobs"].as_array().unwrap().len(),
        3,
        "mismatched review starts no external provider job"
    );
}

#[test]
fn unfinished_ask_handoff_run_snapshots_partial_context_cancels_ask_and_replay_leaves_new_ask_alone()
 {
    use ryoku_rashin::{api::HandoffRun, provider::FixtureOutcome};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine
        .dispatch(Request::PutProvider {
            id: "slow-handoff".into(),
            provider: Provider::Fixture {
                delay_millis: 1000,
                outcome: FixtureOutcome::Stream {
                    chunks: vec!["PARTIAL_Δ".into(), " later".into()],
                    fail: false,
                },
                catalog: None,
            },
        })
        .unwrap();
    for lane in [Lane::Ask, Lane::Chat] {
        engine
            .dispatch(Request::SetRoute {
                lane,
                provider: "slow-handoff".into(),
                model: "fixture-v1".into(),
            })
            .unwrap();
    }
    engine.dispatch(Request::NewSession { id: "handoff-agent".into(), lane: Lane::Agent }).unwrap();
    engine.dispatch(Request::NewSession { id: "handoff-chat".into(), lane: Lane::Chat }).unwrap();
    engine
        .dispatch(Request::Ask {
            id: "unfinished-source".into(),
            prompt: "Canonical unfinished question".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Prompt {
            id: "unrelated-chat".into(),
            session: "handoff-chat".into(),
            prompt: "independent chat".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let job = engine.dispatch(Request::Job { id: "unfinished-source".into() }).unwrap();
        if job["progress"].as_str().is_some_and(|text| text.starts_with("PARTIAL_Δ")) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let request = || Request::Handoff {
        ask_id: "unfinished-source".into(),
        run: Some(HandoffRun { id: "unfinished-run".into(), session: "handoff-agent".into() }),
    };
    engine.dispatch(request()).unwrap();
    let source = engine.dispatch(Request::Job { id: "unfinished-source".into() }).unwrap();
    assert_eq!(source["job"]["status"], "cancelled");
    assert!(source["job"]["answer"].is_null());
    assert!(source["progress"].is_null());
    assert_ne!(
        engine.dispatch(Request::Job { id: "unrelated-chat".into() }).unwrap()["job"]["status"],
        "cancelled"
    );
    let run = done(&engine, "unfinished-run");
    assert_eq!(run["status"], "completed");
    assert_eq!(run["handoff"]["origin"], "in_flight_ask");
    assert_eq!(run["handoff"]["ask_id"], "unfinished-source");
    assert_eq!(run["handoff"]["question"], "Canonical unfinished question");
    assert!(run["handoff"]["answer"].as_str().unwrap().starts_with("PARTIAL_Δ"));
    assert!(run["prompt"].as_str().unwrap().contains("untrusted prior conversation data"));
    assert_eq!(done(&engine, "unrelated-chat")["status"], "completed");
    let activity = engine.dispatch(Request::Usage {}).unwrap()["activity"].clone();
    assert!(activity.as_array().unwrap().iter().all(|row| row["job"] != "unfinished-source"));
    engine
        .dispatch(Request::Ask { id: "new-ask-after-run".into(), prompt: "new question".into() })
        .unwrap();
    engine.dispatch(request()).unwrap();
    assert_eq!(
        engine.dispatch(Request::Job { id: "new-ask-after-run".into() }).unwrap()["job"]["status"],
        "running",
        "accepted Run replay must not cancel later Ask work"
    );
    engine.dispatch(Request::Cancel { id: "new-ask-after-run".into() }).unwrap();
    assert_eq!(
        engine.dispatch(Request::Job { id: "unfinished-source".into() }).unwrap()["job"]["status"],
        "cancelled"
    );
    assert_eq!(engine.dispatch(Request::Session{id:"handoff-agent".into()}).unwrap()["session"]["messages"].as_array().unwrap().len(),2);
}

#[test]
fn manual_handoff_accepts_empty_answer_preserves_provenance_and_restart_and_cancels_only_on_run() {
    use ryoku_rashin::{api::HandoffRun, authorization::Access, provider::FixtureOutcome};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let manual = || Request::ManualHandoff {
        question: "  Manual <question>  ".into(),
        answer: "".into(),
        run: None,
    };
    assert_eq!(engine.dispatch_as(Access::AskClient, manual()), Err(Error::Unauthorized));
    let staged = engine.dispatch(manual()).unwrap();
    assert_eq!(staged["pending_handoff"]["origin"], "manual");
    assert!(staged["pending_handoff"]["ask_id"].is_null());
    assert_eq!(staged["pending_handoff"]["question"], "Manual <question>");
    engine.dispatch(Request::NewSession { id: "manual-agent".into(), lane: Lane::Agent }).unwrap();
    engine.dispatch(Request::NewSession { id: "manual-chat".into(), lane: Lane::Chat }).unwrap();
    drop(engine);
    let engine = fixture.reopen();
    engine
        .dispatch(Request::Prompt {
            id: "manual-chat-turn".into(),
            session: "manual-chat".into(),
            prompt: "plain".into(),
        })
        .unwrap();
    assert!(done(&engine, "manual-chat-turn")["handoff"].is_null());
    assert_eq!(
        engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"]["origin"],
        "manual"
    );
    engine
        .dispatch(Request::Prompt {
            id: "manual-agent-turn".into(),
            session: "manual-agent".into(),
            prompt: "continue".into(),
        })
        .unwrap();
    assert_eq!(done(&engine, "manual-agent-turn")["handoff"]["origin"], "manual");
    engine
        .dispatch(Request::PutProvider {
            id: "slow-manual".into(),
            provider: Provider::Fixture {
                delay_millis: 1000,
                outcome: FixtureOutcome::Answer("old Ask".into()),
                catalog: None,
            },
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "slow-manual".into(),
            model: "fixture-v1".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "manual-active-ask".into(), prompt: "active".into() })
        .unwrap();
    engine.dispatch(manual()).unwrap();
    assert_eq!(
        engine.dispatch(Request::Job { id: "manual-active-ask".into() }).unwrap()["job"]["status"],
        "running",
        "staging must not cancel Ask"
    );
    let run = || Request::ManualHandoff {
        question: "Manual run question".into(),
        answer: "".into(),
        run: Some(HandoffRun { id: "manual-run".into(), session: "manual-agent".into() }),
    };
    engine.dispatch(run()).unwrap();
    assert_eq!(
        engine.dispatch(Request::Job { id: "manual-active-ask".into() }).unwrap()["job"]["status"],
        "cancelled"
    );
    let job = done(&engine, "manual-run");
    assert_eq!(job["handoff"]["origin"], "manual");
    assert!(job["handoff"]["ask_id"].is_null());
    assert_eq!(job["handoff"]["answer"], "");
    engine.dispatch(run()).unwrap();
    assert_eq!(engine.dispatch(Request::Session{id:"manual-agent".into()}).unwrap()["session"]["messages"].as_array().unwrap().len(),4);
    assert!(engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"].is_null());
    assert_eq!(
        engine.dispatch(Request::ManualHandoff {
            question: " ".into(),
            answer: "x".into(),
            run: None
        }),
        Err(Error::InvalidRequest)
    );
    assert!(
        serde_json::from_value::<Request>(
            json!({"action":"manual_handoff","question":"fake","answer":"","origin":"recorded_ask"})
        )
        .is_err()
    );
}

#[test]
fn rejected_handoff_run_preserves_running_ask_and_pending_context_at_worker_limit() {
    use ryoku_rashin::{api::HandoffRun, provider::FixtureOutcome};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine
        .dispatch(Request::ManualHandoff {
            question: "Keep this pending".into(),
            answer: "".into(),
            run: None,
        })
        .unwrap();
    engine
        .dispatch(Request::NewSession { id: "busy-handoff-agent".into(), lane: Lane::Agent })
        .unwrap();
    engine
        .dispatch(Request::PutProvider {
            id: "busy-asks".into(),
            provider: Provider::Fixture {
                delay_millis: 1000,
                outcome: FixtureOutcome::Answer("later".into()),
                catalog: None,
            },
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "busy-asks".into(),
            model: "fixture-v1".into(),
        })
        .unwrap();
    for index in 0..4 {
        engine
            .dispatch(Request::Ask { id: format!("busy-source-{index}"), prompt: "waiting".into() })
            .unwrap();
    }
    assert_eq!(
        engine.dispatch(Request::Handoff {
            ask_id: "busy-source-0".into(),
            run: Some(HandoffRun {
                id: "rejected-handoff".into(),
                session: "busy-handoff-agent".into()
            })
        }),
        Err(Error::Busy)
    );
    assert_eq!(
        engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"]["question"],
        "Keep this pending"
    );
    for index in 0..4 {
        let id = format!("busy-source-{index}");
        assert_eq!(
            engine.dispatch(Request::Job { id: id.clone() }).unwrap()["job"]["status"],
            "running"
        );
        engine.dispatch(Request::Cancel { id }).unwrap();
    }
}

#[test]
fn browser_manual_and_unfinished_handoff_expose_provenance_and_only_accepted_run_cancels_ask() {
    use ryoku_rashin::provider::FixtureOutcome;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    ui_submit(&gateway, &[("action", "new_session"), ("id", "manual-ui-agent"), ("lane", "agent")]);
    ui_submit(&gateway, &[("action", "new_session"), ("id", "manual-ui-chat"), ("lane", "chat")]);
    let stage = [
        ("action", "manual_handoff"),
        ("question", "Manual <context>"),
        ("answer", ""),
        ("mode", "stage"),
    ];
    assert!(ui_submit(&gateway, &stage).starts_with("HTTP/1.1 303"));
    let home = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(home.contains("Manual context for Agent"));
    assert!(home.contains("Context source: manual"));
    assert!(home.contains("Manual &lt;context&gt;"));
    engine
        .dispatch(Request::PutProvider {
            id: "ui-unfinished".into(),
            provider: Provider::Fixture {
                delay_millis: 1000,
                outcome: FixtureOutcome::Stream {
                    chunks: vec!["UI_PARTIAL_Δ".into()],
                    fail: false,
                },
                catalog: None,
            },
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "ui-unfinished".into(),
            model: "fixture-v1".into(),
        })
        .unwrap();
    ui_submit(
        &gateway,
        &[("action", "ask"), ("id", "manual-ui-ask"), ("prompt", "First active Ask")],
    );
    let invalid = [
        ("action", "manual_handoff"),
        ("question", "Manual <context>"),
        ("answer", ""),
        ("mode", "run"),
        ("id", "invalid-manual-target"),
        ("session", ""),
    ];
    assert!(ui_submit(&gateway, &invalid).starts_with("HTTP/1.1 400"));
    assert_eq!(
        engine.dispatch(Request::Job { id: "manual-ui-ask".into() }).unwrap()["job"]["status"],
        "running"
    );
    assert_eq!(
        engine.dispatch(Request::Sessions {}).unwrap()["pending_handoff"]["question"],
        "Manual <context>"
    );
    let run = [
        ("action", "manual_handoff"),
        ("question", "Manual <context>"),
        ("answer", ""),
        ("mode", "run"),
        ("id", "manual-ui-run"),
        ("session", "manual-ui-agent"),
    ];
    assert!(ui_submit(&gateway, &run).contains("Location: /job/manual-ui-run\r\n"));
    assert_eq!(
        engine.dispatch(Request::Job { id: "manual-ui-ask".into() }).unwrap()["job"]["status"],
        "cancelled"
    );
    assert_eq!(done(&engine, "manual-ui-run")["handoff"]["origin"], "manual");
    ui_submit(
        &gateway,
        &[("action", "ask"), ("id", "unfinished-ui-ask"), ("prompt", "Recorded active question")],
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if engine.dispatch(Request::Job { id: "unfinished-ui-ask".into() }).unwrap()["progress"]
            .as_str()
            .is_some_and(|text| text.contains("UI_PARTIAL_Δ"))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let page = http(&gateway, "GET", "/job/unfinished-ui-ask", BROWSER_AUTH, "");
    assert!(page.contains("This Ask is unfinished"));
    assert!(page.contains("Run in selected Agent conversation"));
    assert!(!page.contains("Stage context for next Agent message"));
    let fields = [
        ("action", "handoff_run"),
        ("ask_id", "unfinished-ui-ask"),
        ("id", "unfinished-ui-run"),
        ("session", "manual-ui-agent"),
        ("question", "FORGED_UI_SOURCE"),
    ];
    assert!(ui_submit(&gateway, &fields).contains("Location: /job/unfinished-ui-run\r\n"));
    let job = done(&engine, "unfinished-ui-run");
    assert_eq!(job["handoff"]["origin"], "in_flight_ask");
    assert_eq!(job["handoff"]["question"], "Recorded active question");
    assert_eq!(job["handoff"]["answer"], "UI_PARTIAL_Δ");
    assert_eq!(
        engine.dispatch(Request::Job { id: "unfinished-ui-ask".into() }).unwrap()["job"]["status"],
        "cancelled"
    );
    assert!(engine.dispatch(Request::Session{id:"manual-ui-chat".into()}).unwrap()["session"]["messages"].as_array().unwrap().is_empty());
}

#[test]
fn offline_wiki_navigation_and_verified_indexing_really_feed_ask_and_survive_restart() {
    use ryoku_rashin::authorization::Access;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    engine.dispatch(Request::Index { documents: vec![document()] }).unwrap();
    let pages = engine.dispatch(Request::WikiPages {}).unwrap();
    assert_eq!(pages["pages"].as_array().unwrap().len(), 8);
    let guide =
        engine.dispatch(Request::ReadWiki { id: "linux-basics".into(), original: false }).unwrap();
    let sha = guide["sha256"].as_str().unwrap();
    assert!(guide["content"].as_str().unwrap().contains("zypper dup"));
    assert!(engine.dispatch(Request::ReadWiki{id:"linux-basics".into(),original:true}).unwrap()["content"].as_str().unwrap().contains("pacman"));
    for path in ["/wiki", "/wiki/linux-basics", "/wiki/linux-basics/upstream", "/wiki/license"] {
        assert!(http(&gateway, "GET", path, "", "").starts_with("HTTP/1.1 401"));
        let response = http(&gateway, "GET", path, BROWSER_AUTH, "");
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("Cache-Control: no-store"));
    }
    let listing = http(&gateway, "GET", "/wiki", BROWSER_AUTH, "");
    for id in [
        "overview",
        "desktop",
        "go-tools",
        "hyprland-lua",
        "linux-basics",
        "niri-kdl",
        "quickshell-qml",
        "rashin",
    ] {
        assert!(listing.contains(&format!("href=\"/wiki/{id}\"")));
    }
    let original = http(&gateway, "GET", "/wiki/linux-basics/upstream", BROWSER_AUTH, "");
    assert!(original.contains("Exact upstream reference edition"));
    assert!(!original.contains("name=\"action\" value=\"index_wiki\""));
    assert_eq!(
        engine.dispatch_as(Access::AskClient, Request::WikiPages {}),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::ReadWiki { id: "linux-basics".into(), original: false }
        ),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::IndexWiki {
                id: "linux-basics".into(),
                sha256: sha.into(),
                replace_sha256: None
            }
        ),
        Err(Error::Unauthorized)
    );
    assert!(
        ui_submit(&gateway, &[("action", "index_wiki"), ("id", "linux-basics"), ("sha256", sha)])
            .contains("Location: /wiki/linux-basics\r\n")
    );
    let index = engine.dispatch(Request::GetIndex {}).unwrap();
    assert_eq!(index["documents"].as_array().unwrap().len(), 2);
    assert!(
        index["documents"].as_array().unwrap().iter().any(|row| row["name"] == "tumbleweed.md")
    );
    assert!(
        ui_submit(&gateway, &[("action", "index_wiki"), ("id", "linux-basics"), ("sha256", sha)])
            .starts_with("HTTP/1.1 303")
    );
    engine
        .dispatch(Request::Ask {
            id: "wiki-answer".into(),
            prompt: "Tumbleweed Zypper distribution upgrade".into(),
        })
        .unwrap();
    let answer = done(&engine, "wiki-answer")["answer"].as_str().unwrap().to_owned();
    assert!(answer.contains("wiki/linux-basics.md"));
    assert!(answer.contains("zypper"));
    assert!(answer.contains(&sha[..12]));
    drop(gateway);
    drop(engine);
    let engine = fixture.reopen();
    assert_eq!(
        engine.dispatch(Request::GetIndex {}).unwrap()["documents"].as_array().unwrap().len(),
        2
    );
    engine
        .dispatch(Request::Ask {
            id: "wiki-after-restart".into(),
            prompt: "Tumbleweed Zypper".into(),
        })
        .unwrap();
    assert!(
        done(&engine, "wiki-after-restart")["answer"]
            .as_str()
            .unwrap()
            .contains("wiki/linux-basics.md")
    );
}

#[test]
fn wiki_index_rejects_stale_or_conflicting_replacement_and_preserves_private_documents() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let old_content = "PRIVATE_INDEX_DOCUMENT".to_string();
    let old_sha = format!("{:x}", Sha256::digest(old_content.as_bytes()));
    let clash = Document {
        name: "wiki/linux-basics.md".into(),
        content: old_content,
        sha256: old_sha.clone(),
    };
    engine.dispatch(Request::Index { documents: vec![document(), clash] }).unwrap();
    let guide =
        engine.dispatch(Request::ReadWiki { id: "linux-basics".into(), original: false }).unwrap();
    let sha = guide["sha256"].as_str().unwrap();
    assert_eq!(
        engine.dispatch(Request::IndexWiki {
            id: "linux-basics".into(),
            sha256: "0".repeat(64),
            replace_sha256: None
        }),
        Err(Error::Conflict)
    );
    assert_eq!(
        engine.dispatch(Request::IndexWiki {
            id: "linux-basics".into(),
            sha256: sha.into(),
            replace_sha256: None
        }),
        Err(Error::Conflict)
    );
    let page = http(&gateway, "GET", "/wiki/linux-basics", BROWSER_AUTH, "");
    assert!(page.contains("already has a different digest"));
    assert!(page.contains(&old_sha));
    assert!(!page.contains("PRIVATE_INDEX_DOCUMENT"));
    let mut fields = vec![
        ("action", "index_wiki"),
        ("id", "linux-basics"),
        ("sha256", sha),
        ("replace_sha256", old_sha.as_str()),
    ];
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 400"));
    assert!(
        engine.dispatch(Request::GetIndex {}).unwrap()["documents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["sha256"] == old_sha)
    );
    assert_eq!(
        engine.dispatch(Request::IndexWiki {
            id: "linux-basics".into(),
            sha256: sha.into(),
            replace_sha256: Some("stale".into())
        }),
        Err(Error::Conflict)
    );
    fields.push(("confirm_replace", "yes"));
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 303"));
    let index = engine.dispatch(Request::GetIndex {}).unwrap();
    assert_eq!(index["documents"].as_array().unwrap().len(), 2);
    assert!(
        index["documents"].as_array().unwrap().iter().any(|row| row["name"] == "tumbleweed.md")
    );
    assert!(index["documents"].as_array().unwrap().iter().any(|row| row["sha256"] == sha));
    assert_eq!(
        engine.dispatch(Request::ReadWiki { id: "../../private".into(), original: false }),
        Err(Error::NotFound)
    );
}

fn completion_gateway(
    count: usize,
) -> (u16, std::sync::mpsc::Receiver<Value>, std::thread::JoinHandle<()>) {
    use std::{net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        for _ in 0..count {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("gateway accept: {error}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                assert!(bytes.len() < 8192);
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let header = String::from_utf8(bytes).unwrap();
            assert!(header.contains("Authorization: Bearer PRIVATE_PROWL_COMPLETION_TOKEN\r\n"));
            let length = header
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert!(length <= 256 * 1024);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            assert!(
                !body
                    .windows(b"PRIVATE_PROWL_COMPLETION_TOKEN".len())
                    .any(|part| part == b"PRIVATE_PROWL_COMPLETION_TOKEN")
            );
            if header.starts_with("GET /v1/models ") {
                assert!(body.is_empty());
                let response = r#"{"data":[{"id":"auto:fast"},{"id":"profile/◇ opaque"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                )
                .unwrap();
                sender.send(json!({"path":"/v1/models"})).unwrap();
            } else {
                assert!(header.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
                let body: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["model"], "auto:fast");
                sender.send(body.clone()).unwrap();
                if body["stream"] == true {
                    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
                    let data = "data: {\"model\":\"gateway/model\",\"choices\":[{\"delta\":{\"content\":\"Real Prowl response Δ\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":7}}\n\ndata: [DONE]\n\n";
                    write!(stream, "{:x}\r\n{data}\r\n0\r\n\r\n", data.len()).unwrap();
                } else {
                    let data = r#"{"model":"gateway/model","choices":[{"message":{"content":"Real Prowl response Δ"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":7}}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",
                        data.len()
                    )
                    .unwrap();
                }
            }
        }
    });
    (port, receiver, worker)
}
#[test]
fn prowl_provider_uses_private_connection_for_real_ask_agent_chat_and_model_refresh_without_rewriting_picks()
 {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) = completion_gateway(4);
    let before = engine.dispatch(Request::Status {}).unwrap()["routes"].clone();
    engine
        .dispatch(Request::ConfigureProwl {
            port,
            token: Some("PRIVATE_PROWL_COMPLETION_TOKEN".into()),
        })
        .unwrap();
    let gateway = ryoku_rashin::http::Gateway::new(
        Arc::clone(&engine),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    assert!(
        ui_submit(
            &gateway,
            &[
                ("action", "connect_prowl_provider"),
                ("provider", "gateway"),
                ("models", "auto:fast"),
                ("streaming", "true")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );

    assert_eq!(
        engine.dispatch(Request::Status {}).unwrap()["routes"],
        before,
        "connect must not change saved picks"
    );
    for lane in [Lane::Ask, Lane::Agent, Lane::Chat] {
        engine
            .dispatch(Request::SetRoute {
                lane,
                provider: "gateway".into(),
                model: "auto:fast".into(),
            })
            .unwrap();
    }
    engine.dispatch(Request::NewSession { id: "gateway-agent".into(), lane: Lane::Agent }).unwrap();
    engine.dispatch(Request::NewSession { id: "gateway-chat".into(), lane: Lane::Chat }).unwrap();
    engine
        .dispatch(Request::Ask { id: "gateway-ask".into(), prompt: "ASK_PAYLOAD".into() })
        .unwrap();
    assert_eq!(done(&engine, "gateway-ask")["answer"], "Real Prowl response Δ");
    for (id, session, prompt) in [
        ("gateway-agent-job", "gateway-agent", "AGENT_PAYLOAD"),
        ("gateway-chat-job", "gateway-chat", "CHAT_PAYLOAD"),
    ] {
        if id == "gateway-chat-job" {
            assert!(
                ui_submit(
                    &gateway,
                    &[
                        ("action", "update_prowl_provider"),
                        ("provider", "gateway"),
                        ("models", "auto:fast")
                    ]
                )
                .starts_with("HTTP/1.1 303")
            );
        }
        engine
            .dispatch(Request::Prompt {
                id: id.into(),
                session: session.into(),
                prompt: prompt.into(),
            })
            .unwrap();
        let job = done(&engine, id);
        assert_eq!(job["actual_model"], "gateway/model");
        assert_eq!(job["model"], "auto:fast");
    }
    for expected in ["ASK_PAYLOAD", "AGENT_PAYLOAD", "CHAT_PAYLOAD"] {
        let body = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(body["messages"].as_array().unwrap().last().unwrap()["content"], expected);
        assert_eq!(body["stream"], expected != "CHAT_PAYLOAD");
    }
    engine
        .dispatch(Request::RefreshModels {
            id: "gateway-models".into(),
            provider: "gateway".into(),
        })
        .unwrap();
    assert_eq!(done(&engine, "gateway-models")["status"], "completed");
    assert_eq!(requests.recv_timeout(Duration::from_secs(3)).unwrap()["path"], "/v1/models");
    server.join().unwrap();
    let providers = engine.dispatch(Request::Providers {}).unwrap();
    let description = providers["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["id"] == "gateway")
        .unwrap();
    assert_eq!(description["kind"], "prowl_gateway");
    assert_eq!(description["models"], json!(["auto:fast", "profile/◇ opaque"]));
    assert!(!providers.to_string().contains("PRIVATE_PROWL_COMPLETION_TOKEN"));
    let activity = engine.dispatch(Request::Usage {}).unwrap()["activity"].clone();
    assert_eq!(activity.as_array().unwrap().len(), 3);
    for row in activity.as_array().unwrap() {
        assert_eq!(row["input_tokens"], 5);
        assert_eq!(row["output_tokens"], 7);
        assert_eq!(row["estimated"], false);
        assert_eq!(row["actual_model"], "gateway/model");
    }
    assert_eq!(engine.dispatch(Request::Status {}).unwrap()["routes"]["ask"]["model"], "auto:fast");
    let state = fs::read_to_string(fixture.path.join("state.json")).unwrap();
    assert_eq!(
        state.matches("PRIVATE_PROWL_COMPLETION_TOKEN").count(),
        1,
        "provider must reference the single private connection"
    );
    engine.dispatch(Request::ConfigureProwl { port, token: None }).unwrap();
    assert_eq!(
        engine.dispatch_as(
            ryoku_rashin::authorization::Access::AskClient,
            Request::ConnectProwlProvider {
                id: "forbidden".into(),
                models: vec!["auto:fast".into()],
                streaming: true
            }
        ),
        Err(Error::Unauthorized)
    );
    engine.dispatch(Request::DisableProwl {}).unwrap();
    assert_eq!(
        engine.dispatch(Request::Ask {
            id: "disconnected-gateway".into(),
            prompt: "must not send".into()
        }),
        Err(Error::ProviderUnavailable)
    );
    assert_eq!(engine.dispatch(Request::Status {}).unwrap()["routes"]["ask"]["model"], "auto:fast");
    let html = http(&gateway, "GET", "/console", BROWSER_AUTH, "");
    assert!(html.contains("Saved pick unavailable: gateway"));
    assert!(!html.contains("PRIVATE_PROWL_COMPLETION_TOKEN"));
}

#[test]
fn real_prowl_sse_progress_is_live_and_cancel_closes_stream_without_partial_turn_or_usage() {
    use std::{net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (closed_tx, closed_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("SSE accept: {error}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 8192);
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
        }
        let header = String::from_utf8(header).unwrap();
        assert!(header.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(header.contains("Authorization: Bearer PRIVATE_PROWL_COMPLETION_TOKEN\r\n"));
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).unwrap();
        let event =
            "data: {\"choices\":[{\"delta\":{\"content\":\"LIVE_PRIVATE_PROGRESS_Δ\"}}]}\n\n";
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{event}\r\n",event.len()).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut byte = [0];
        match stream.read(&mut byte) {
            Ok(0) => (),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => (),
            other => panic!("cancel did not close Prowl stream: {other:?}"),
        };
        closed_tx.send(()).unwrap();
    });
    let fixture = Fixture::new();
    let engine = fixture.engine();
    engine
        .dispatch(Request::ConfigureProwl {
            port,
            token: Some("PRIVATE_PROWL_COMPLETION_TOKEN".into()),
        })
        .unwrap();
    engine
        .dispatch(Request::ConnectProwlProvider {
            id: "stream-gateway".into(),
            models: vec!["auto:fast".into()],
            streaming: true,
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Agent,
            provider: "stream-gateway".into(),
            model: "auto:fast".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::NewSession { id: "gateway-stream-session".into(), lane: Lane::Agent })
        .unwrap();
    engine
        .dispatch(Request::Prompt {
            id: "gateway-stream-job".into(),
            session: "gateway-stream-session".into(),
            prompt: "stream request".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if engine.dispatch(Request::Job { id: "gateway-stream-job".into() }).unwrap()["progress"]
            .as_str()
            .is_some_and(|text| text.contains("LIVE_PRIVATE_PROGRESS_Δ"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "actual gateway SSE did not expose live progress");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        !fs::read_to_string(fixture.path.join("state.json"))
            .unwrap()
            .contains("LIVE_PRIVATE_PROGRESS_Δ")
    );
    engine.dispatch(Request::Cancel { id: "gateway-stream-job".into() }).unwrap();
    closed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    server.join().unwrap();
    let job = engine.dispatch(Request::Job { id: "gateway-stream-job".into() }).unwrap();
    assert_eq!(job["job"]["status"], "cancelled");
    assert!(job["progress"].is_null());
    assert!(job["job"]["answer"].is_null());
    assert!(engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().is_empty());
    assert!(engine.dispatch(Request::Session{id:"gateway-stream-session".into()}).unwrap()["session"]["messages"].as_array().unwrap().is_empty());
}

#[test]
fn prowl_gateway_credential_refresh_rejects_stale_catalog_and_keeps_reference_and_saved_model() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let body = r#"{"data":[{"id":"stale-gateway-offer"}]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (port, entered, release, server) = held_prowl_fixture(response.into_bytes());
    engine
        .dispatch(Request::ConfigureProwl {
            port,
            token: Some("PRIVATE_PROWL_COMPLETION_TOKEN".into()),
        })
        .unwrap();
    engine
        .dispatch(Request::ConnectProwlProvider {
            id: "changing-gateway".into(),
            models: vec!["auto:fast".into()],
            streaming: true,
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "changing-gateway".into(),
            model: "auto:fast".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::RefreshModels {
            id: "old-gateway-catalog".into(),
            provider: "changing-gateway".into(),
        })
        .unwrap();
    let header = entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(header.starts_with("GET /v1/models HTTP/1.1\r\n"));
    engine.dispatch(Request::ConfigureProwl { port: 1, token: None }).unwrap();
    release.send(()).unwrap();
    let job = done(&engine, "old-gateway-catalog");
    assert_eq!(job["status"], "failed");
    assert_eq!(job["error"], "conflict");
    server.join().unwrap();
    let description = engine.dispatch(Request::Providers {}).unwrap()["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["id"] == "changing-gateway")
        .unwrap()
        .clone();
    assert_eq!(description["models"], json!(["auto:fast"]));
    assert_eq!(description["port"], 1);
    assert_eq!(description["credential_present"], true);
    let state = fs::read_to_string(fixture.path.join("state.json")).unwrap();
    assert_eq!(state.matches("PRIVATE_PROWL_COMPLETION_TOKEN").count(), 1);
    assert_eq!(engine.dispatch(Request::Status {}).unwrap()["routes"]["ask"]["model"], "auto:fast");
}

fn tool_round_response(name: &str, arguments: &str, id: &str) -> Value {
    json!({"model":"tool-actual","choices":[{"message":{"content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3}})
}
fn tool_answer() -> Value {
    json!({"model":"tool-actual","choices":[{"message":{"content":"Evidence-backed tool answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":3}})
}
fn tool_gateway(
    responses: Vec<Option<Value>>,
) -> (u16, std::sync::mpsc::Receiver<Value>, std::thread::JoinHandle<()>) {
    tool_gateway_with_hook(responses, |_| {})
}
fn tool_gateway_with_hook<F: FnMut(&Value) + Send + 'static>(
    responses: Vec<Option<Value>>,
    mut hook: F,
) -> (u16, std::sync::mpsc::Receiver<Value>, std::thread::JoinHandle<()>) {
    use std::{net::TcpListener, sync::mpsc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("tool gateway accept: {e}"),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                assert!(header.len() < 8192);
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            let search = header.starts_with("GET /api/code/search?");
            assert!(search || header.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
            assert!(header.contains("Authorization: Bearer PRIVATE_TOOL_GATEWAY_KEY\r\n"));
            let length = header
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert!(length <= 256 * 1024);
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).unwrap();
            let body: Value = if search {
                assert!(bytes.is_empty());
                json!({"method":"GET","request_path":header.lines().next().unwrap().split_whitespace().nth(1).unwrap()})
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            assert!(!String::from_utf8(bytes).unwrap().contains("PRIVATE_TOOL_GATEWAY_KEY"));
            let Some(response) = response else {
                stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
                let data = "data: {\"choices\":[{\"delta\":{\"content\":\"waiting\"}}]}\n\n";
                write!(stream, "{:x}\r\n{}\r\n", data.len(), data).unwrap();
                stream.flush().unwrap();
                // Cancellation fixtures announce readiness only after the
                // nonterminal bytes are flushed. Otherwise a valid client
                // cancellation can reset the socket during this write.
                tx.send(body.clone()).unwrap();
                hook(&body);
                stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                let mut byte = [0];
                match stream.read(&mut byte) {
                    Ok(0) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => panic!("cancelled tool round stayed connected: {other:?}"),
                };
                continue;
            };
            tx.send(body.clone()).unwrap();
            hook(&body);
            let output = if body["stream"] == true {
                let message = &response["choices"][0]["message"];
                let mut delta = json!({"content":message["content"]});
                if let Some(calls) = message["tool_calls"].as_array() {
                    delta["tool_calls"] = json!(
                        calls
                            .iter()
                            .enumerate()
                            .map(|(i, call)| {
                                let mut call = call.clone();
                                call["index"] = json!(i);
                                call
                            })
                            .collect::<Vec<_>>()
                    );
                }
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"index":0,"delta":delta,"finish_reason":response["choices"][0]["finish_reason"]}],"usage":response["usage"],"model":response["model"]})
                )
            } else {
                response.to_string()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{output}",
                output.len()
            )
            .unwrap();
        }
    });
    (port, rx, worker)
}
fn connect_tool_gateway(engine: &Arc<Engine>, port: u16, streaming: bool) {
    engine
        .dispatch(Request::ConfigureProwl { port, token: Some("PRIVATE_TOOL_GATEWAY_KEY".into()) })
        .unwrap();
    engine
        .dispatch(Request::ConnectProwlProvider {
            id: "tool-gateway".into(),
            models: vec!["auto:fast".into()],
            streaming,
        })
        .unwrap();
    for lane in [Lane::Ask, Lane::Agent] {
        engine
            .dispatch(Request::SetRoute {
                lane,
                provider: "tool-gateway".into(),
                model: "auto:fast".into(),
            })
            .unwrap();
    }
}
#[test]
fn tool_round_gateway_executes_real_kernel_query_only_after_owner_enablement_and_fix_stays_disabled()
 {
    use ryoku_rashin::{api::FixKind, authorization::Access};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) = tool_gateway(vec![
        Some(tool_answer()),
        Some(tool_round_response("system_query", r#"{"topic":"kernel"}"#, "kernel-call")),
        Some(tool_answer()),
        Some(tool_answer()),
    ]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::Ask { id: "tools-off".into(), prompt: "No implicit tools".into() })
        .unwrap();
    assert_eq!(done(&engine, "tools-off")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::SetReadOnlyTools { provider: "tool-gateway".into(), enabled: true }
        ),
        Err(Error::Unauthorized)
    );
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    assert!(
        ui_submit(
            &gateway,
            &[("action", "set_read_only_tools"), ("provider", "tool-gateway"), ("enabled", "true")]
        )
        .starts_with("HTTP/1.1 303")
    );
    engine
        .dispatch(Request::Ask { id: "tools-on".into(), prompt: "Read the kernel version".into() })
        .unwrap();
    let job = done(&engine, "tools-on");
    assert_eq!(job["answer"], "Evidence-backed tool answer");
    let first = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(first["tools"][0]["function"]["name"], "system_query");
    let next = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let messages = next["messages"].as_array().unwrap();
    let result = messages.last().unwrap();
    assert_eq!(result["role"], "tool");
    assert_eq!(result["tool_call_id"], "kernel-call");
    let native: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(native["output"], fs::read_to_string("/proc/sys/kernel/osrelease").unwrap().trim());
    assert_eq!(messages[messages.len() - 2]["tool_calls"][0]["id"], "kernel-call");
    let activity = engine.dispatch(Request::Usage {}).unwrap();
    let rows = activity["activity"].as_array().unwrap();
    let row = rows.iter().find(|r| r["job"] == "tools-on").unwrap();
    assert_eq!(row["input_tokens"], 4);
    assert_eq!(row["output_tokens"], 6);
    let preview = engine
        .dispatch(Request::FixPreview {
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: "finding".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Fix {
            id: "fix-no-tools".into(),
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: "finding".into(),
            native_evidence_consent: Some(
                preview["native_evidence_consent"].as_str().unwrap().into(),
            ),
        })
        .unwrap();
    assert_eq!(done(&engine, "fix-no-tools")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    server.join().unwrap();
    drop(gateway);
    drop(engine);
    let engine = fixture.reopen();
    let providers = engine.dispatch(Request::Providers {}).unwrap();
    assert!(
        providers["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "tool-gateway" && p["read_only_tools_enabled"] == true)
    );
}
#[test]
fn tool_round_gateway_bounds_four_rounds_then_disables_tools_and_handles_unknown_malformed_calls() {
    for violate_final in [false, true] {
        let fixture = Fixture::new();
        let engine = fixture.engine();
        let mut responses = vec![
            Some(tool_round_response("PRIVATE_UNKNOWN_FUNCTION", "{}", "unknown")),
            Some(tool_round_response("system_query", "not json", "malformed")),
            Some(tool_round_response("system_query", r#"{"topic":"shell"}"#, "invalid-topic")),
            Some(tool_round_response("system_query", r#"{"topic":"kernel"}"#, "real-kernel")),
        ];
        responses.push(Some(if violate_final {
            tool_round_response("system_query", r#"{"topic":"kernel"}"#, "forbidden-fifth")
        } else {
            tool_answer()
        }));
        let (port, requests, server) = tool_gateway(responses);
        connect_tool_gateway(&engine, port, false);
        engine
            .dispatch(Request::SetReadOnlyTools { provider: "tool-gateway".into(), enabled: true })
            .unwrap();
        engine
            .dispatch(Request::Ask {
                id: "round-budget".into(),
                prompt: "Use bounded tools".into(),
            })
            .unwrap();
        let job = done(&engine, "round-budget");
        assert_eq!(job["status"], if violate_final { "failed" } else { "completed" });
        let bodies = (0..5)
            .map(|_| requests.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect::<Vec<_>>();
        assert!(bodies[..4].iter().all(|b| b.get("tools").is_some()));
        assert!(bodies[4].get("tools").is_none());
        let results = bodies[4]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool")
            .map(|m| serde_json::from_str::<Value>(m["content"].as_str().unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 4);
        let activity = job["tool_activity"].as_array().unwrap();
        assert_eq!(activity.len(), 4);
        assert_eq!(activity[0]["name"], "unknown_tool");
        assert!(!serde_json::to_string(activity).unwrap().contains("PRIVATE_UNKNOWN_FUNCTION"));
        assert!(activity[..3].iter().all(|row| row["phase"] == "failed"));
        assert_eq!(activity[3]["phase"], "completed");
        assert_eq!(results[0]["error"], "unknown_tool");
        assert_eq!(results[1]["error"], "InvalidRequest");
        assert_eq!(results[2]["error"], "InvalidRequest");
        assert!(results[3]["output"].as_str().unwrap().len() > 1);
        server.join().unwrap();
        if violate_final {
            assert!(
                engine.dispatch(Request::Usage {}).unwrap()["activity"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
#[test]
fn tool_round_gateway_cancellation_closes_followup_stream_without_partial_answer_or_usage() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) = tool_gateway(vec![
        Some(tool_round_response("system_query", r#"{"topic":"kernel"}"#, "kernel")),
        None,
    ]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::SetReadOnlyTools { provider: "tool-gateway".into(), enabled: true })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "cancel-tool-round".into(), prompt: "Read then wait".into() })
        .unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let followup = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(followup["messages"].as_array().unwrap().last().unwrap()["role"], "tool");
    engine.dispatch(Request::Cancel { id: "cancel-tool-round".into() }).unwrap();
    assert_eq!(done(&engine, "cancel-tool-round")["status"], "cancelled");
    server.join().unwrap();
    assert!(engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().is_empty());
    let state = fs::read_to_string(fixture.path.join("state.json")).unwrap();
    assert!(!state.contains("Evidence-backed tool answer"));
    assert!(!state.contains("waiting"));
}

fn file_tool_call(name: &str, path: &str, id: &str) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":json!({"path":path}).to_string()}})
}
fn file_tool_round(calls: Vec<Value>) -> Value {
    json!({"model":"tool-actual","choices":[{"message":{"content":null,"tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3}})
}
#[test]
fn enrolled_file_gateway_reads_and_lists_allowed_notes_but_refuses_renamed_store_links_and_secrets()
{
    use ryoku_rashin::authorization::Access;
    let root = Fixture::new();
    let state_path = root.path.join("account-state");
    fs::create_dir(&state_path).unwrap();
    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
    let engine = Engine::open(Store::open(&state_path).unwrap()).unwrap();
    fs::write(root.path.join("note.md"), "ENROLLED_NOTE_BODY λ").unwrap();
    fs::write(root.path.join("api-key"), "PRIVATE_ROOT_KEY_SENTINEL").unwrap();
    fs::write(root.path.join("hard-target"), "PRIVATE_HARD_LINK_SENTINEL").unwrap();
    fs::hard_link(root.path.join("hard-target"), root.path.join("hard")).unwrap();
    symlink("note.md", root.path.join("link")).unwrap();
    let moved = root.path.join("moved-account-state");
    fs::rename(&state_path, &moved).unwrap();
    let responses = vec![
        Some(file_tool_round(vec![
            file_tool_call("list_dir", "vault:/", "list"),
            file_tool_call("read_file", "vault:/note.md", "note"),
            file_tool_call("read_file", "vault:/moved-account-state/state.json", "store"),
            file_tool_call("read_file", "vault:/api-key", "key"),
            file_tool_call("read_file", "vault:/link", "link"),
            file_tool_call("read_file", "vault:/hard", "hard"),
            json!({"id":"system","type":"function","function":{"name":"system_query","arguments":"{\"topic\":\"kernel\"}"}}),
            file_tool_call("read_file", "vault:/../outside", "escape"),
        ])),
        Some(tool_answer()),
        Some(tool_answer()),
        Some(tool_answer()),
    ];
    let (port, requests, server) = tool_gateway(responses);
    connect_tool_gateway(&engine, port, true);
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::EnrollReadRoot {
                provider: "tool-gateway".into(),
                id: "vault".into(),
                path: root.path.to_str().unwrap().into()
            }
        ),
        Err(Error::Unauthorized)
    );
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let mut fields = vec![
        ("action", "enroll_read_root"),
        ("provider", "tool-gateway"),
        ("root_id", "vault"),
        ("path", root.path.to_str().unwrap()),
    ];
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 400"));
    assert!(
        engine.dispatch(Request::ReadRoots { provider: "tool-gateway".into() }).unwrap()["roots"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fields.push(("confirm", "yes"));
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 303"));
    engine
        .dispatch(Request::Ask {
            id: "file-round".into(),
            prompt: "Read the enrolled ordinary note".into(),
        })
        .unwrap();
    assert_eq!(done(&engine, "file-round")["status"], "completed");
    let first = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let names = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["read_file", "list_dir"]);
    let followup = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let results = followup["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| {
            (
                m["tool_call_id"].as_str().unwrap(),
                serde_json::from_str::<Value>(m["content"].as_str().unwrap()).unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(results["note"]["text"], "ENROLLED_NOTE_BODY λ");
    assert!(results["list"]["entries"].as_array().unwrap().iter().any(|e| e["name"] == "note.md"));
    for id in ["store", "key", "link", "hard", "system", "escape"] {
        assert!(results[id].get("error").is_some(), "{id} must fail closed");
    }
    assert_eq!(results["store"]["error"], "Unauthorized");
    assert_eq!(results["system"]["error"], "unknown_tool");
    let serialized = followup.to_string();
    for marker in
        ["PRIVATE_ROOT_KEY_SENTINEL", "PRIVATE_HARD_LINK_SENTINEL", "PRIVATE_TOOL_GATEWAY_KEY"]
    {
        assert!(!serialized.contains(marker));
    }
    assert_eq!(
        engine
            .dispatch_as(Access::AskClient, Request::ReadRoots { provider: "tool-gateway".into() }),
        Err(Error::Unauthorized)
    );
    let preview = engine
        .dispatch(Request::FixPreview {
            kind: ryoku_rashin::api::FixKind::App,
            component: "synthetic.service".into(),
            evidence: "file boundary finding".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Fix {
            id: "file-fix-no-tools".into(),
            kind: ryoku_rashin::api::FixKind::App,
            component: "synthetic.service".into(),
            evidence: "file boundary finding".into(),
            native_evidence_consent: Some(
                preview["native_evidence_consent"].as_str().unwrap().into(),
            ),
        })
        .unwrap();
    assert_eq!(done(&engine, "file-fix-no-tools")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    drop(gateway);
    drop(engine);
    let state_fixture = Fixture { path: moved };
    let engine = state_fixture.reopen();
    let roots = engine.dispatch(Request::ReadRoots { provider: "tool-gateway".into() }).unwrap();
    assert_eq!(roots["roots"][0]["available"], true);
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::DisableReadRoot { provider: "tool-gateway".into(), id: "vault".into() }
        ),
        Err(Error::Unauthorized)
    );
    engine
        .dispatch(Request::DisableReadRoot { provider: "tool-gateway".into(), id: "vault".into() })
        .unwrap();
    engine
        .dispatch(Request::Ask {
            id: "file-root-disabled".into(),
            prompt: "No tools after disable".into(),
        })
        .unwrap();
    assert_eq!(done(&engine, "file-root-disabled")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    server.join().unwrap();
}
#[test]
fn replaced_file_root_is_unavailable_and_requires_explicit_reenrollment_before_gateway_use() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let root = Fixture::new();
    fs::write(root.path.join("note"), "old").unwrap();
    let (port, requests, server) = tool_gateway(vec![
        Some(file_tool_round(vec![file_tool_call("read_file", "vault:/note", "note")])),
        Some(tool_answer()),
    ]);
    connect_tool_gateway(&engine, port, false);
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "vault".into(),
            path: root.path.to_str().unwrap().into(),
        })
        .unwrap();
    let moved = root.path.with_extension("moved");
    fs::rename(&root.path, &moved).unwrap();
    fs::create_dir(&root.path).unwrap();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.path.join("note"), "NEW_EXPLICITLY_ENROLLED_NOTE").unwrap();
    assert_eq!(
        engine.dispatch(Request::ReadRoots { provider: "tool-gateway".into() }).unwrap()["roots"]
            [0]["available"],
        false
    );
    assert_eq!(
        engine.dispatch(Request::Ask { id: "stale-file-root".into(), prompt: "Read note".into() }),
        Err(Error::Integrity)
    );
    assert!(requests.try_recv().is_err());
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "vault".into(),
            path: root.path.to_str().unwrap().into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "new-file-root".into(), prompt: "Read note".into() })
        .unwrap();
    assert_eq!(done(&engine, "new-file-root")["status"], "completed");
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let next = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let result: Value = serde_json::from_str(
        next["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(result["text"], "NEW_EXPLICITLY_ENROLLED_NOTE");
    server.join().unwrap();
    fs::remove_dir_all(moved).unwrap();
}
#[test]
fn enrolled_file_tool_cancellation_drops_ephemeral_read_results_and_closes_followup_stream() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let root = Fixture::new();
    fs::write(root.path.join("note"), "UNPERSISTED_ENROLLED_TEXT").unwrap();
    let (port, requests, server) = tool_gateway(vec![
        Some(file_tool_round(vec![file_tool_call("read_file", "vault:/note", "note")])),
        None,
    ]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "vault".into(),
            path: root.path.to_str().unwrap().into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "cancel-file-tool".into(), prompt: "Read then wait".into() })
        .unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let next = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(next.to_string().contains("UNPERSISTED_ENROLLED_TEXT"));
    engine.dispatch(Request::Cancel { id: "cancel-file-tool".into() }).unwrap();
    assert_eq!(done(&engine, "cancel-file-tool")["status"], "cancelled");
    server.join().unwrap();
    assert!(engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().is_empty());
    let state = fs::read_to_string(fixture.path.join("state.json")).unwrap();
    assert!(!state.contains("UNPERSISTED_ENROLLED_TEXT"));
}

fn code_call(query: &str) -> Value {
    tool_round_response("search_code", &json!({"query":query}).to_string(), "code-query")
}
fn percent(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            write!(out, "%{b:02X}").unwrap();
        }
    }
    out
}
#[test]
fn code_search_gateway_uses_owner_selected_root_and_returns_only_authorized_bounded_hits() {
    use ryoku_rashin::authorization::Access;
    let source = Fixture::new();
    let state = source.path.join("account-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let engine = Engine::open(Store::open(&state).unwrap()).unwrap();
    fs::create_dir(source.path.join("src")).unwrap();
    fs::write(source.path.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(source.path.join("api-key"), "PRIVATE_CODE_SENTINEL").unwrap();
    symlink("src/main.rs", source.path.join("link")).unwrap();
    let hits = json!([{"file":"src/main.rs","start_line":7,"snippet":format!("  {}  ","matching λ".repeat(40)),"unknown_secret":"PRIVATE_UNKNOWN_CODE_FIELD"},{"file":"api-key","start_line":1,"snippet":"PRIVATE_CODE_SENTINEL"},{"file":"account-state/state.json","start_line":1,"snippet":"PRIVATE_STORE_CODE_SENTINEL"},{"file":"../outside","start_line":1,"snippet":"outside"},{"file":"/etc/passwd","start_line":1,"snippet":"outside"},{"file":"link","start_line":1,"snippet":"link"},{"file":"deleted.rs","start_line":1,"snippet":"stale"}]);
    let (port, requests, server) = tool_gateway(vec![
        Some(code_call("needle & λ")),
        Some(hits),
        Some(tool_answer()),
        Some(tool_answer()),
    ]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "source".into(),
            path: source.path.to_str().unwrap().into(),
        })
        .unwrap();
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::SetCodeSearch {
                provider: "tool-gateway".into(),
                root_id: Some("source".into())
            }
        ),
        Err(Error::Unauthorized)
    );
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let mut fields =
        vec![("action", "set_code_search"), ("provider", "tool-gateway"), ("root_id", "source")];
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 400"));
    fields.push(("confirm", "yes"));
    assert!(ui_submit(&gateway, &fields).starts_with("HTTP/1.1 303"));
    engine
        .dispatch(Request::Ask { id: "source-search".into(), prompt: "Search source".into() })
        .unwrap();
    assert_eq!(done(&engine, "source-search")["status"], "completed");
    let first = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        first["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "search_code")
    );
    assert!(
        !first["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "system_query")
    );
    let query = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(query["method"], "GET");
    assert_eq!(
        query["request_path"],
        format!(
            "/api/code/search?q=needle%20%26%20%CE%BB&repo={}&limit=20",
            percent(source.path.to_str().unwrap())
        )
    );
    let final_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let result: Value = serde_json::from_str(
        final_request["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    assert_eq!(result["hits"][0]["file"], "src/main.rs");
    assert_eq!(result["hits"][0]["line"], 7);
    assert!(result["hits"][0]["text"].as_str().unwrap().len() <= 160);
    assert_eq!(result["omitted"], 6);
    for sentinel in
        ["PRIVATE_CODE_SENTINEL", "PRIVATE_UNKNOWN_CODE_FIELD", "PRIVATE_STORE_CODE_SENTINEL"]
    {
        assert!(!final_request.to_string().contains(sentinel));
    }
    drop(gateway);
    drop(engine);
    let state_fixture = Fixture { path: state };
    let engine = state_fixture.reopen();
    assert_eq!(
        engine.dispatch(Request::ReadRoots { provider: "tool-gateway".into() }).unwrap()["source_root"],
        "source"
    );
    engine
        .dispatch(Request::SetCodeSearch { provider: "tool-gateway".into(), root_id: None })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "search-disabled".into(), prompt: "No source tool".into() })
        .unwrap();
    assert_eq!(done(&engine, "search-disabled")["status"], "completed");
    let disabled = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        !disabled["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "search_code")
    );
    server.join().unwrap();
}
#[test]
fn code_search_gateway_drops_malformed_oversized_and_credential_echo_responses_and_bounds_hits() {
    for variant in 0..4 {
        let fixture = Fixture::new();
        let engine = fixture.engine();
        let source = Fixture::new();
        fs::write(source.path.join("main.rs"), "ordinary").unwrap();
        let hit = json!({"file":"main.rs","start_line":1,"snippet":"ordinary"});
        let response = match variant {
            0 => json!({"error":"PRIVATE_GATEWAY_ERROR"}),
            1 => json!((0..21).map(|_| hit.clone()).collect::<Vec<_>>()),
            2 => json!([{"file":"main.rs","start_line":1,"snippet":"PRIVATE_TOOL_GATEWAY_KEY"}]),
            _ => json!((0..20).map(|_| hit.clone()).collect::<Vec<_>>()),
        };
        let (port, requests, server) =
            tool_gateway(vec![Some(code_call("needle")), Some(response), Some(tool_answer())]);
        connect_tool_gateway(&engine, port, false);
        engine
            .dispatch(Request::EnrollReadRoot {
                provider: "tool-gateway".into(),
                id: "source".into(),
                path: source.path.to_str().unwrap().into(),
            })
            .unwrap();
        engine
            .dispatch(Request::SetCodeSearch {
                provider: "tool-gateway".into(),
                root_id: Some("source".into()),
            })
            .unwrap();
        engine
            .dispatch(Request::Ask { id: "source-errors".into(), prompt: "Search safely".into() })
            .unwrap();
        assert_eq!(done(&engine, "source-errors")["status"], "completed");
        requests.recv_timeout(Duration::from_secs(2)).unwrap();
        requests.recv_timeout(Duration::from_secs(2)).unwrap();
        let final_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
        let result: Value = serde_json::from_str(
            final_request["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        match variant {
            0 | 2 => assert_eq!(result["error"], "ProviderFailed"),
            1 => assert_eq!(result["error"], "Limit"),
            _ => {
                assert_eq!(result["hits"].as_array().unwrap().len(), 12);
                assert_eq!(result["omitted"], 8);
            }
        }
        assert!(!final_request.to_string().contains("PRIVATE_GATEWAY_ERROR"));
        server.join().unwrap();
    }
}
#[test]
fn code_search_gateway_cancellation_closes_index_request_without_final_completion_or_usage() {
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let source = Fixture::new();
    fs::write(source.path.join("main.rs"), "ordinary").unwrap();
    let (port, requests, server) = tool_gateway(vec![Some(code_call("needle")), None]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "source".into(),
            path: source.path.to_str().unwrap().into(),
        })
        .unwrap();
    engine
        .dispatch(Request::SetCodeSearch {
            provider: "tool-gateway".into(),
            root_id: Some("source".into()),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "cancel-code".into(), prompt: "Search then wait".into() })
        .unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let query = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(query["method"], "GET");
    engine.dispatch(Request::Cancel { id: "cancel-code".into() }).unwrap();
    assert_eq!(done(&engine, "cancel-code")["status"], "cancelled");
    server.join().unwrap();
    assert!(requests.try_recv().is_err());
    assert!(engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().is_empty());
}
#[test]
fn code_search_gateway_rejects_source_replacement_during_index_response_without_disclosing_hits() {
    use std::sync::mpsc;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let source = Fixture::new();
    fs::write(source.path.join("main.rs"), "ordinary").unwrap();
    let (release_tx, release_rx) = mpsc::channel();
    let (port, requests, server) = tool_gateway_with_hook(
        vec![
            Some(code_call("needle")),
            Some(
                json!([{"file":"main.rs","start_line":1,"snippet":"PRIVATE_STALE_SOURCE_SNIPPET"}]),
            ),
            Some(tool_answer()),
        ],
        move |request| {
            if request["method"] == "GET" {
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }
        },
    );
    connect_tool_gateway(&engine, port, false);
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "source".into(),
            path: source.path.to_str().unwrap().into(),
        })
        .unwrap();
    engine
        .dispatch(Request::SetCodeSearch {
            provider: "tool-gateway".into(),
            root_id: Some("source".into()),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "stale-code".into(), prompt: "Search source".into() })
        .unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let moved = source.path.with_extension("moved");
    fs::rename(&source.path, &moved).unwrap();
    fs::create_dir(&source.path).unwrap();
    fs::set_permissions(&source.path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(source.path.join("main.rs"), "replacement").unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(done(&engine, "stale-code")["status"], "completed");
    let final_request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let result: Value = serde_json::from_str(
        final_request["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(result["error"], "Integrity");
    assert!(!final_request.to_string().contains("PRIVATE_STALE_SOURCE_SNIPPET"));
    server.join().unwrap();
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "tool-gateway".into(),
            id: "source".into(),
            path: source.path.to_str().unwrap().into(),
        })
        .unwrap();
    assert!(engine.dispatch(Request::ReadRoots{provider:"tool-gateway".into()}).unwrap()["source_root"].is_null());
    fs::remove_dir_all(moved).unwrap();
}

fn web_call(url: &str) -> Value {
    tool_round_response("fetch_url", &json!({"url":url}).to_string(), "web-query")
}
#[test]
fn public_web_permission_is_owner_explicit_persistent_and_independent_with_fix_disabled() {
    use ryoku_rashin::{api::FixKind, authorization::Access};
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) = tool_gateway(vec![
        Some(tool_answer()),
        Some(web_call("http://127.0.0.1:9/private")),
        Some(tool_answer()),
        Some(tool_answer()),
        Some(tool_answer()),
    ]);
    connect_tool_gateway(&engine, port, false);
    engine
        .dispatch(Request::Ask { id: "web-default-off".into(), prompt: "No implicit web".into() })
        .unwrap();
    assert_eq!(done(&engine, "web-default-off")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::SetPublicWeb { provider: "tool-gateway".into(), enabled: true }
        ),
        Err(Error::Unauthorized)
    );
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    assert!(
        ui_submit(
            &gateway,
            &[("action", "set_public_web"), ("provider", "tool-gateway"), ("enabled", "true")]
        )
        .starts_with("HTTP/1.1 303")
    );
    engine
        .dispatch(Request::Ask { id: "web-private-refused".into(), prompt: "Try the URL".into() })
        .unwrap();
    assert_eq!(done(&engine, "web-private-refused")["status"], "completed");
    let first = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(first["tools"].as_array().unwrap().len(), 1);
    assert_eq!(first["tools"][0]["function"]["name"], "fetch_url");
    let next = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let result: Value = serde_json::from_str(
        next["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(result["error"], "Unauthorized");
    let preview = engine
        .dispatch(Request::FixPreview {
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: "web boundary".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Fix {
            id: "web-fix-off".into(),
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: "web boundary".into(),
            native_evidence_consent: Some(
                preview["native_evidence_consent"].as_str().unwrap().into(),
            ),
        })
        .unwrap();
    assert_eq!(done(&engine, "web-fix-off")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    drop(gateway);
    drop(engine);
    let engine = fixture.reopen();
    assert!(
        engine.dispatch(Request::Providers {}).unwrap()["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "tool-gateway" && p["public_web_enabled"] == true)
    );
    engine
        .dispatch(Request::SetPublicWeb { provider: "tool-gateway".into(), enabled: false })
        .unwrap();
    engine.dispatch(Request::Ask { id: "web-disabled".into(), prompt: "No web".into() }).unwrap();
    assert_eq!(done(&engine, "web-disabled")["status"], "completed");
    assert!(requests.recv_timeout(Duration::from_secs(2)).unwrap().get("tools").is_none());
    server.join().unwrap();
}
#[test]
#[ignore = "requires fresh user+network namespace and RASHIN_PUBLIC_WEB_PARENT_NET guard"]
fn isolated_public_web_gateway_returns_real_redirect_text_and_cancels_without_partial_usage() {
    use std::{net::TcpListener, process::Command, sync::mpsc};
    let parent =
        std::env::var("RASHIN_PUBLIC_WEB_PARENT_NET").expect("run in fresh user/net namespace");
    assert_ne!(fs::read_link("/proc/self/ns/net").unwrap().to_str().unwrap(), parent);
    for args in
        [vec!["link", "set", "lo", "up"], vec!["address", "add", "11.0.0.1/32", "dev", "lo"]]
    {
        assert!(Command::new("/usr/sbin/ip").args(args).status().unwrap().success());
    }
    let listener = TcpListener::bind("11.0.0.1:0").unwrap();
    let web_port = listener.local_addr().unwrap().port();
    let web = std::thread::spawn(move || {
        for step in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                assert!(header.len() < 8192);
                let mut b = [0];
                stream.read_exact(&mut b).unwrap();
                header.push(b[0]);
            }
            let header = String::from_utf8(header).unwrap();
            assert!(!header.to_ascii_lowercase().contains("authorization:"));
            assert!(!header.to_ascii_lowercase().contains("cookie:"));
            assert!(!header.contains("PRIVATE_TOOL_GATEWAY_KEY"));
            if step == 0 {
                assert!(header.starts_with("GET /start "));
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                assert!(header.starts_with("GET /final "));
                let body = "<h1>PUBLIC_FETCH_REAL_TEXT λ</h1><script>PRIVATE_SCRIPT_BODY</script>";
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html; charset=UTF-8\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        }
    });
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) = tool_gateway(vec![
        Some(web_call(&format!("http://11.0.0.1:{web_port}/start"))),
        Some(tool_answer()),
    ]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::SetPublicWeb { provider: "tool-gateway".into(), enabled: true })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "real-web".into(), prompt: "Fetch the public fixture".into() })
        .unwrap();
    assert_eq!(done(&engine, "real-web")["status"], "completed");
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let next = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    let result: Value = serde_json::from_str(
        next["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(result["status"], 200);
    assert_eq!(result["text"], "PUBLIC_FETCH_REAL_TEXT λ");
    assert!(!next.to_string().contains("PRIVATE_SCRIPT_BODY"));
    web.join().unwrap();
    server.join().unwrap();
    let listener = TcpListener::bind("11.0.0.1:0").unwrap();
    let web_port = listener.local_addr().unwrap().port();
    let (ready_tx, ready_rx) = mpsc::channel();
    let web = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            stream.read_exact(&mut b).unwrap();
            header.push(b[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\nUNPERSISTED_WEB_PREFIX")
            .unwrap();
        stream.flush().unwrap();
        ready_tx.send(()).unwrap();
        let mut b = [0];
        match stream.read(&mut b) {
            Ok(0) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("web cancel failed {other:?}"),
        }
    });
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) =
        tool_gateway(vec![Some(web_call(&format!("http://11.0.0.1:{web_port}/wait")))]);
    connect_tool_gateway(&engine, port, true);
    engine
        .dispatch(Request::SetPublicWeb { provider: "tool-gateway".into(), enabled: true })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "cancel-real-web".into(), prompt: "Fetch then wait".into() })
        .unwrap();
    requests.recv_timeout(Duration::from_secs(2)).unwrap();
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    engine.dispatch(Request::Cancel { id: "cancel-real-web".into() }).unwrap();
    assert_eq!(done(&engine, "cancel-real-web")["status"], "cancelled");
    web.join().unwrap();
    server.join().unwrap();
    assert!(engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().is_empty());
    assert!(
        !fs::read_to_string(fixture.path.join("state.json"))
            .unwrap()
            .contains("UNPERSISTED_WEB_PREFIX")
    );
}

#[test]
fn prowl_fix_requires_bound_consent_before_gateway_egress_and_disconnected_route_never_falls_back()
{
    use ryoku_rashin::api::FixKind;
    let fixture = Fixture::new();
    let engine = fixture.engine();
    let (port, requests, server) = tool_gateway(vec![Some(tool_answer())]);
    connect_tool_gateway(&engine, port, false);
    let finding = "Prowl consent regression finding";
    let fix = |id: &str, token: Option<String>| Request::Fix {
        id: id.into(),
        kind: FixKind::App,
        component: "synthetic.service".into(),
        evidence: finding.into(),
        native_evidence_consent: token,
    };
    assert_eq!(engine.dispatch(fix("no-prowl-consent", None)), Err(Error::EvidenceConsentRequired));
    assert_eq!(
        engine.dispatch(Request::Job { id: "no-prowl-consent".into() }),
        Err(Error::NotFound)
    );
    assert!(requests.try_recv().is_err());
    let preview = engine
        .dispatch(Request::FixPreview {
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: finding.into(),
        })
        .unwrap();
    assert_eq!(preview["destination"]["provider"]["kind"], "prowl_gateway");
    assert_eq!(preview["destination"]["provider"]["credential_present"], true);
    assert_eq!(
        preview["destination"]["provider"]["egress"],
        "configured_gateway_routes_may_be_external"
    );
    assert!(!preview.to_string().contains("PRIVATE_TOOL_GATEWAY_KEY"));
    let old_token = preview["native_evidence_consent"].as_str().unwrap().to_owned();
    engine.dispatch(Request::DisableProwl {}).unwrap();
    assert_eq!(
        engine.dispatch(fix("disconnected-prowl-fix", Some(old_token.clone()))),
        Err(Error::ProviderUnavailable)
    );
    assert!(matches!(
        engine.dispatch(Request::FixPreview {
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: finding.into()
        }),
        Err(Error::ProviderUnavailable)
    ));
    assert!(requests.try_recv().is_err());
    engine
        .dispatch(Request::ConfigureProwl { port, token: Some("PRIVATE_TOOL_GATEWAY_KEY".into()) })
        .unwrap();
    assert_eq!(
        engine.dispatch(fix("stale-prowl-consent", Some(old_token))),
        Err(Error::EvidenceConsentRequired)
    );
    assert!(requests.try_recv().is_err());
    let preview = engine
        .dispatch(Request::FixPreview {
            kind: FixKind::App,
            component: "synthetic.service".into(),
            evidence: finding.into(),
        })
        .unwrap();
    let token = preview["native_evidence_consent"].as_str().unwrap().to_owned();
    engine.dispatch(fix("approved-prowl-fix", Some(token.clone()))).unwrap();
    assert_eq!(done(&engine, "approved-prowl-fix")["status"], "completed");
    let body = requests.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(body.get("tools").is_none());
    assert!(
        body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains(finding)
    );
    server.join().unwrap();
    assert_eq!(
        engine.dispatch(fix("replayed-prowl-consent", Some(token.clone()))),
        Err(Error::EvidenceConsentRequired)
    );
    engine.dispatch(fix("approved-prowl-fix", Some(token))).unwrap();
    assert!(requests.try_recv().is_err());
    assert_eq!(
        engine.dispatch(Request::Usage {}).unwrap()["activity"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn tool_activity_live_gateway_browser_privacy_terminal_history_and_cancel_are_consistent() {
    use std::sync::mpsc;
    for phase in ["completed", "failed", "cancelled"] {
        let fixture = Fixture::new();
        let engine = fixture.engine();
        let source = Fixture::new();
        fs::write(source.path.join("main.rs"), "ordinary").unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let call_id = "https://private.invalid/PRIVATE_CALL_DATA";
        let initial =
            tool_round_response("search_code", r#"{"query":"PRIVATE_SEARCH_QUERY"}"#, call_id);
        let responses = if phase == "cancelled" {
            vec![Some(initial), None]
        } else {
            vec![
                Some(initial),
                Some(if phase == "failed" {
                    json!({"malformed":"PRIVATE_RESULT"})
                } else {
                    json!([{ "file":"main.rs", "start_line":1,"snippet":"PRIVATE_SOURCE_SNIPPET" }])
                }),
                Some(tool_answer()),
            ]
        };
        let is_cancel = phase == "cancelled";
        let (port, requests, server) = tool_gateway_with_hook(responses, move |request| {
            if !is_cancel && request["method"] == "GET" {
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }
        });
        connect_tool_gateway(&engine, port, false);
        engine
            .dispatch(Request::EnrollReadRoot {
                provider: "tool-gateway".into(),
                id: "source".into(),
                path: source.path.to_str().unwrap().into(),
            })
            .unwrap();
        engine
            .dispatch(Request::SetCodeSearch {
                provider: "tool-gateway".into(),
                root_id: Some("source".into()),
            })
            .unwrap();
        engine
            .dispatch(Request::Ask { id: "activity".into(), prompt: "Search source".into() })
            .unwrap();
        requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(requests.recv_timeout(Duration::from_secs(3)).unwrap()["method"], "GET");
        let live = rpc(&engine, Request::Job { id: "activity".into() });
        let records = &live["job"]["tool_activity"];
        assert_eq!(records[0]["phase"], "running");
        assert_eq!(records[0]["name"], "search_code");
        assert_eq!(records[0]["call_hash"], format!("{:x}", Sha256::digest(call_id.as_bytes())));
        let metadata = records.to_string();
        for secret in [
            "PRIVATE_CALL_DATA",
            "PRIVATE_SEARCH_QUERY",
            "PRIVATE_SOURCE_SNIPPET",
            "private.invalid",
            source.path.to_str().unwrap(),
        ] {
            assert!(!metadata.contains(secret));
        }
        assert!(records[0].get("arguments").is_none());
        assert!(records[0].get("result").is_none());
        assert!(records[0].get("call_id").is_none());
        let gateway = ryoku_rashin::http::Gateway::new(
            engine.clone(),
            "1".repeat(64),
            "http://127.0.0.1:51321".into(),
        )
        .unwrap();
        let headers = BROWSER_AUTH;
        let html = http(&gateway, "GET", "/job/activity", headers, "");
        assert!(html.contains("Tool activity"));
        assert!(html.contains("search_code"));
        assert!(html.contains("<td>running</td>"));
        assert!(!html.contains("PRIVATE_SEARCH_QUERY"));
        if is_cancel {
            engine.dispatch(Request::Cancel { id: "activity".into() }).unwrap();
        } else {
            release_tx.send(()).unwrap();
        }
        let terminal = done(&engine, "activity");
        assert_eq!(terminal["status"], if is_cancel { "cancelled" } else { "completed" });
        assert_eq!(terminal["tool_activity"][0]["phase"], phase);
        if phase == "failed" {
            assert_eq!(terminal["tool_activity"][0]["error"], "provider_failed");
        }
        let html = http(&gateway, "GET", "/job/activity", headers, "");
        assert!(html.contains(&format!("<td>{phase}</td>")));
        server.join().unwrap();
        if is_cancel {
            assert!(
                engine.dispatch(Request::Usage {}).unwrap()["activity"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(terminal["answer"].is_null());
        }
        drop(gateway);
        drop(engine);
        let engine = fixture.reopen();
        assert_eq!(
            rpc(&engine, Request::Job { id: "activity".into() })["job"]["tool_activity"],
            terminal["tool_activity"]
        );
    }
}
