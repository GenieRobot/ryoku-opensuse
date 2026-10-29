use ryoku_rashin::{
    Error, Result,
    acp::{Arguments, Client, Config, Handler},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Default)]
struct Surface {
    text: String,
    reads: usize,
    allow_terminal: bool,
    replayed: usize,
    ready: Option<std::sync::mpsc::Sender<()>>,
}
impl Handler for Surface {
    fn capabilities(&self) -> Value {
        json!({"fs":{"readTextFile":true,"writeTextFile":false},"terminal":false})
    }
    fn request(&mut self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "fs/read_text_file" => {
                self.reads += 1;
                if params["path"] != "fixture:/allowed.txt" {
                    return Err(Error::Unauthorized);
                }
                Ok(json!({"content":"Actual capability-bound read result"}))
            }
            "session/request_permission" => Ok(
                json!({"outcome":{"outcome":"selected","optionId":if self.allow_terminal {"allow-once"} else {"reject-once"}}}),
            ),
            _ => Err(Error::NotFound),
        }
    }
    fn update(&mut self, params: &Value) -> Result<()> {
        if params["update"]["sessionUpdate"] == "agent_message_chunk" {
            let text = params["update"]["content"]["text"].as_str().ok_or(Error::ProviderFailed)?;
            self.text.push_str(text);
            if text == "descendant ready"
                && let Some(ready) = &self.ready
            {
                ready.send(()).unwrap();
            }
            if text == "Replayed private session" {
                self.replayed += 1;
            }
        }
        Ok(())
    }
}
fn config() -> Config {
    Config::enroll(env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture").into(), Arguments::Stdio, false)
        .unwrap()
}
#[test]
fn captured_adapter_refuses_alias_or_changed_artifact_and_malformed_configuration() {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    let path = std::env::temp_dir().join(format!("rashin-acp-artifact-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    let artifact = path.join("adapter");
    fs::copy(env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture"), &artifact).unwrap();
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&artifact, path.join("alias")).unwrap();
    assert!(
        Config::enroll(path.join("alias").to_str().unwrap().into(), Arguments::Stdio, false)
            .is_err()
    );
    assert!(matches!(
        Client::connect(
            &config(),
            Path::new(env!("CARGO_BIN_EXE_ryoku-rashin")),
            Instant::now() + Duration::from_secs(1),
            &AtomicBool::new(true),
            &mut Surface::default()
        ),
        Err(Error::Cancelled)
    ));
    let enrolled =
        Config::enroll(artifact.to_str().unwrap().into(), Arguments::Stdio, false).unwrap();
    let mut changed = fs::read(&artifact).unwrap();
    changed[100] ^= 1;
    fs::write(&artifact, changed).unwrap();
    assert!(matches!(
        Client::connect(
            &enrolled,
            Path::new(env!("CARGO_BIN_EXE_ryoku-rashin")),
            Instant::now() + Duration::from_secs(1),
            &AtomicBool::new(false),
            &mut Surface::default()
        ),
        Err(Error::ProviderUnavailable)
    ));
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(Config::enroll(artifact.to_str().unwrap().into(), Arguments::Stdio, false).is_err());
    fs::remove_dir_all(path).unwrap();
}
#[test]
#[ignore = "requires unprivileged user/mount/PID/network namespaces and mapped subordinate UID;explicit host proof"]
fn real_restricted_acp_stdio_negotiates_sessions_models_replay_reads_permissions_terminal_and_cancel()
 {
    let flag = AtomicBool::new(false);
    let mut surface = Surface::default();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut client = Client::connect(
        &config(),
        Path::new(env!("CARGO_BIN_EXE_ryoku-rashin")),
        deadline,
        &flag,
        &mut surface,
    )
    .unwrap();
    assert!(client.capabilities.load_session);
    assert!(client.capabilities.images);
    let session = client.new_session(deadline, &flag, &mut surface).unwrap();
    assert_eq!(session.models.len(), 2);
    client.set_model("fixture/λ", deadline, &flag, &mut surface).unwrap();
    assert_eq!(client.session.as_ref().unwrap().current_model.as_deref(), Some("fixture/λ"));
    assert_eq!(
        client.set_model("unavailable-saved-pick", deadline, &flag, &mut surface),
        Err(Error::ProviderUnavailable)
    );
    client.prompt("jail boundary", deadline, &flag, &mut surface).unwrap();
    assert_eq!(surface.text, "Verified restricted jail");
    surface.text.clear();
    client.prompt("read:fixture:/allowed.txt", deadline, &flag, &mut surface).unwrap();
    assert_eq!(surface.text, "Actual capability-bound read result");
    assert_eq!(surface.reads, 1);
    surface.text.clear();
    client.prompt("read:/private/credentials", deadline, &flag, &mut surface).unwrap();
    assert_eq!(surface.text, "Denied by client capability policy");
    surface.text.clear();
    client.prompt("permission terminal kernel", deadline, &flag, &mut surface).unwrap();
    assert_eq!(surface.text, "Permission denied");
    surface.allow_terminal = true;
    surface.text.clear();
    client.prompt("permission terminal kernel", deadline, &flag, &mut surface).unwrap();
    assert!(!surface.text.trim().is_empty());
    assert!(!surface.text.contains("Permission denied"));
    surface.text.clear();
    let loaded =
        client.load_session("prior-opaque/session", deadline, &flag, &mut surface).unwrap();
    assert_eq!(loaded.id, "prior-opaque/session");
    assert_eq!(surface.replayed, 1);
    assert_eq!(surface.text, "Replayed private session");
    drop(client);
    let flag = Arc::new(AtomicBool::new(false));
    let mut surface = Surface::default();
    let mut client = Client::connect(
        &config(),
        Path::new(env!("CARGO_BIN_EXE_ryoku-rashin")),
        Instant::now() + Duration::from_secs(5),
        &flag,
        &mut surface,
    )
    .unwrap();
    client.new_session(Instant::now() + Duration::from_secs(2), &flag, &mut surface).unwrap();
    let cancel = Arc::clone(&flag);
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::Release);
    });
    let started = Instant::now();
    assert_eq!(
        client.prompt(
            "wait for cancellation",
            Instant::now() + Duration::from_secs(4),
            &flag,
            &mut surface
        ),
        Err(Error::Cancelled)
    );
    drop(client);
    thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
}

struct HttpBytes {
    input: std::io::Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl std::io::Read for HttpBytes {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.input, bytes)
    }
}
impl std::io::Write for HttpBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn browser(gateway: &ryoku_rashin::http::Gateway, method: &str, path: &str, body: &str) -> String {
    let basic = "b3duZXI6MTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMTExMQ==";
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:51321\r\nAuthorization: Basic {basic}\r\nOrigin: http://127.0.0.1:51321\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream =
        HttpBytes { input: std::io::Cursor::new(request.into_bytes()), output: Vec::new() };
    gateway.handle(&mut stream).unwrap();
    String::from_utf8(stream.output).unwrap()
}
fn form(gateway: &ryoku_rashin::http::Gateway, fields: &[(&str, &str)]) -> String {
    let mut encoded = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in fields {
        encoded.append_pair(key, value);
    }
    browser(gateway, "POST", "/console/action", &encoded.finish())
}
fn done(engine: &Arc<ryoku_rashin::runtime::Engine>, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let value = engine.dispatch(ryoku_rashin::api::Request::Job { id: id.into() }).unwrap();
        if value["job"]["status"] != "running" {
            return value["job"].clone();
        }
        assert!(Instant::now() < deadline, "ACP job {id} did not complete");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "requires real namespace runner; private ACP thinking host proof"]
fn acp_thinking_is_live_private_bounded_and_persisted_only_on_completion() {
    use ryoku_rashin::{
        api::{Lane, Request},
        authorization::Access,
        runtime::Engine,
        store::Store,
    };
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = std::env::temp_dir().join(format!("rashin-acp-thought-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let runner = std::path::PathBuf::from(env!("CARGO_BIN_EXE_ryoku-rashin"));
    let engine =
        Engine::open_with_adapter_runner(Store::open(&root).unwrap(), runner.clone()).unwrap();
    engine
        .dispatch(Request::EnrollAcpProvider {
            id: "adapter".into(),
            path: env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture").into(),
            models: vec!["fixture/default".into()],
            arguments: Arguments::Stdio,
            network: false,
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Chat,
            provider: "adapter".into(),
            model: "fixture/default".into(),
        })
        .unwrap();
    engine.dispatch(Request::NewSession { id: "chat".into(), lane: Lane::Chat }).unwrap();
    engine
        .dispatch(Request::Prompt {
            id: "thinking-cancel".into(),
            session: "chat".into(),
            prompt: "thought then wait".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let view = engine.dispatch(Request::Job { id: "thinking-cancel".into() }).unwrap();
        if view["thought_progress"] == "Private <thinking> Δ" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!fs::read_to_string(root.join("state.json")).unwrap().contains("Private <thinking>"));
    assert_eq!(
        engine.dispatch_as(Access::AskClient, Request::Job { id: "thinking-cancel".into() }),
        Err(Error::Unauthorized)
    );
    engine.dispatch(Request::Cancel { id: "thinking-cancel".into() }).unwrap();
    let view = engine.dispatch(Request::Job { id: "thinking-cancel".into() }).unwrap();
    assert!(view["thought_progress"].is_null());
    assert!(view["job"]["thought"].is_null());
    engine
        .dispatch(Request::Prompt {
            id: "thinking-complete".into(),
            session: "chat".into(),
            prompt: "show thought".into(),
        })
        .unwrap();
    let job = done(&engine, "thinking-complete");
    assert_eq!(job["thought"], "Private <thinking> Δ");
    assert_eq!(job["answer"], "Thought complete");
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let html = browser(&gateway, "GET", "/job/thinking-complete", "");
    assert!(html.contains("Private &lt;thinking&gt; Δ"));
    assert!(!html.contains("Private <thinking> Δ"));
    engine
        .dispatch(Request::Prompt {
            id: "thinking-limit".into(),
            session: "chat".into(),
            prompt: "excessive thought".into(),
        })
        .unwrap();
    let limited = done(&engine, "thinking-limit");
    assert_eq!(limited["error"], "limit");
    assert!(limited["thought"].is_null());
    drop(gateway);
    drop(engine);
    let deadline = Instant::now() + Duration::from_secs(6);
    let engine = loop {
        match Store::open(&root) {
            Ok(store) => break Engine::open_with_adapter_runner(store, runner.clone()).unwrap(),
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("reopen: {error:?}"),
        }
    };
    assert_eq!(
        engine.dispatch(Request::Job { id: "thinking-complete".into() }).unwrap()["job"]["thought"],
        "Private <thinking> Δ"
    );
    drop(engine);
    fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "requires real namespace runner; explicit Ask fallback host proof"]
fn ask_fallback_captures_route_and_grants_suppresses_marker_and_accounts_both_attempts() {
    use ryoku_rashin::{
        api::{Lane, Request},
        authorization::Access,
        provider::{FixtureOutcome, Provider},
        runtime::Engine,
        store::Store,
    };
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = std::env::temp_dir().join(format!("rashin-ask-fallback-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let state = root.join("state");
    let notes = root.join("notes");
    for path in [&state, &notes] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(notes.join("note.txt"), "CAPTURED CONTENT\n").unwrap();
    let engine = Engine::open_with_adapter_runner(
        Store::open(&state).unwrap(),
        env!("CARGO_BIN_EXE_ryoku-rashin").into(),
    )
    .unwrap();
    engine
        .dispatch(Request::EnrollAcpProvider {
            id: "adapter".into(),
            path: env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture").into(),
            models: vec!["fixture/default".into()],
            arguments: Arguments::Stdio,
            network: false,
        })
        .unwrap();
    engine
        .dispatch(Request::EnrollReadRoot {
            provider: "adapter".into(),
            id: "notes".into(),
            path: notes.to_str().unwrap().into(),
        })
        .unwrap();
    let selection = Request::SetAskFallback {
        provider: Some("adapter".into()),
        model: Some("fixture/default".into()),
    };
    assert_eq!(engine.dispatch_as(Access::AskClient, selection), Err(Error::Unauthorized));
    let gateway = ryoku_rashin::http::Gateway::new(
        engine.clone(),
        "1".repeat(64),
        "http://127.0.0.1:51321".into(),
    )
    .unwrap();
    let choice = json!({"provider":"adapter","model":"fixture/default"}).to_string();
    assert!(
        form(&gateway, &[("action", "set_ask_fallback"), ("choice", &choice)])
            .starts_with("HTTP/1.1 303")
    );
    assert!(browser(&gateway, "GET", "/console", "").contains("Ask fallback ACP model"));
    engine
        .dispatch(Request::PutProvider {
            id: "fast".into(),
            provider: Provider::Fixture {
                delay_millis: 600,
                outcome: FixtureOutcome::Stream {
                    chunks: vec!["TOOLS_".into(), "REQUIRED".into()],
                    fail: false,
                },
                catalog: None,
            },
        })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "fast".into(),
            model: "fixture-v1".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "snapshot".into(), prompt: "read:notes:/note.txt".into() })
        .unwrap();
    engine
        .dispatch(Request::DisableReadRoot { provider: "adapter".into(), id: "notes".into() })
        .unwrap();
    engine.dispatch(Request::SetAskFallback { provider: None, model: None }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let view = engine.dispatch(Request::Job { id: "snapshot".into() }).unwrap();
        assert!(!view["progress"].as_str().unwrap_or("").contains("TOOLS_"));
        if view["job"]["status"] != "running" {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let job = done(&engine, "snapshot");
    assert_eq!(job["answer"], "CAPTURED CONTENT\n");
    assert_eq!(job["fallback"]["reason"], "needs_tools");
    assert_eq!(job["fallback"]["route"]["provider"], "adapter");
    assert!(browser(&gateway, "GET", "/job/snapshot", "").contains("Ask fallback: adapter"));
    let usage = engine.dispatch(Request::Usage {}).unwrap();
    let entries = usage["activity"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["job"] == "snapshot")
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["provider"], "fast");
    assert_eq!(entries[1]["provider"], "adapter");
    engine
        .dispatch(Request::Ask { id: "disabled".into(), prompt: "read:notes:/note.txt".into() })
        .unwrap();
    assert_eq!(done(&engine, "disabled")["error"], "provider_unavailable");
    engine
        .dispatch(Request::SetAskFallback {
            provider: Some("adapter".into()),
            model: Some("fixture/default".into()),
        })
        .unwrap();
    engine
        .dispatch(Request::PutProvider {
            id: "fast".into(),
            provider: Provider::Fixture {
                delay_millis: 0,
                outcome: FixtureOutcome::Failure,
                catalog: None,
            },
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "failure".into(), prompt: "read:notes:/note.txt".into() })
        .unwrap();
    let job = done(&engine, "failure");
    assert_eq!(job["fallback"]["reason"], "fast_lane_unavailable");
    assert_eq!(job["answer"], "Denied by client capability policy");
    engine
        .dispatch(Request::Ask {
            id: "cancel-fallback".into(),
            prompt: "wait for cancellation".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let view = engine.dispatch(Request::Job { id: "cancel-fallback".into() }).unwrap();
        if view["job"]["fallback"]["used"] == true {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    engine.dispatch(Request::Cancel { id: "cancel-fallback".into() }).unwrap();
    assert_eq!(done(&engine, "cancel-fallback")["status"], "cancelled");
    engine.dispatch(Request::RemoveProvider { id: "fast".into() }).unwrap();
    engine
        .dispatch(Request::Ask { id: "missing-fast".into(), prompt: "read:notes:/note.txt".into() })
        .unwrap();
    let job = done(&engine, "missing-fast");
    assert_eq!(job["fallback"]["reason"], "fast_lane_unavailable");
    assert_eq!(job["answer"], "Denied by client capability policy");
    engine
        .dispatch(Request::PutProvider {
            id: "fast".into(),
            provider: Provider::Fixture {
                delay_millis: 0,
                outcome: FixtureOutcome::Answer("ordinary answer".into()),
                catalog: None,
            },
        })
        .unwrap();
    engine.dispatch(Request::Ask { id: "ordinary".into(), prompt: "hello".into() }).unwrap();
    let job = done(&engine, "ordinary");
    assert_eq!(job["answer"], "ordinary answer");
    assert_eq!(job["fallback"]["used"], false);
    engine
        .dispatch(Request::PutProvider {
            id: "fast".into(),
            provider: Provider::Fixture {
                delay_millis: 0,
                outcome: FixtureOutcome::Answer("TOOLS_REQUIRED".into()),
                catalog: None,
            },
        })
        .unwrap();
    for (lane, id) in [(Lane::Agent, "agent"), (Lane::Chat, "chat")] {
        engine
            .dispatch(Request::SetRoute {
                lane,
                provider: "fast".into(),
                model: "fixture-v1".into(),
            })
            .unwrap();
        engine.dispatch(Request::NewSession { id: id.into(), lane }).unwrap();
        engine
            .dispatch(Request::Prompt {
                id: format!("{id}-job"),
                session: id.into(),
                prompt: "hello".into(),
            })
            .unwrap();
        let job = done(&engine, &format!("{id}-job"));
        assert_eq!(job["answer"], "TOOLS_REQUIRED");
        assert!(job["fallback"].is_null());
    }
    drop(gateway);
    drop(engine);
    fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "requires real namespace runner;explicit configured API/runtime/browser host acceptance proof"]
fn enrolled_acp_provider_browser_and_real_engine_execute_reads_denials_models_cancel_and_restart() {
    use ryoku_rashin::{
        api::{Lane, Request},
        authorization::Access,
        runtime::Engine,
        store::Store,
    };
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = std::env::temp_dir().join(format!("rashin-acp-runtime-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let state = root.join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let notes = root.join("notes");
    fs::create_dir(&notes).unwrap();
    fs::set_permissions(&notes, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(notes.join("note.txt"), "REAL_ENROLLED_NOTE Δ\r\nSECOND_LINE\r\n").unwrap();
    fs::write(notes.join("lf.txt"), "first\nlast\n").unwrap();
    fs::write(notes.join("bound.txt"), "x\n".repeat(16 * 1024)).unwrap();
    fs::write(notes.join("large-lines.txt"), "y\n".repeat(20000)).unwrap();
    fs::write(notes.join("invalid-utf8.txt"), b"invalid\xfftext").unwrap();
    fs::write(notes.join("oversized.txt"), "x".repeat(32 * 1024 + 1)).unwrap();
    fs::write(notes.join(".env"), "OPENAI_API_KEY=PRIVATE_FIXTURE_SECRET").unwrap();
    let runner = std::path::PathBuf::from(env!("CARGO_BIN_EXE_ryoku-rashin"));
    let engine =
        Engine::open_with_adapter_runner(Store::open(&state).unwrap(), runner.clone()).unwrap();
    assert_eq!(
        engine.dispatch_as(
            Access::AskClient,
            Request::EnrollAcpProvider {
                id: "adapter".into(),
                path: env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture").into(),
                models: vec!["fixture/default".into()],
                arguments: Arguments::Stdio,
                network: false
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
        ("action", "enroll_acp_provider"),
        ("provider", "adapter"),
        ("path", env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture")),
        ("models", "fixture/default\nfixture/λ\nunavailable-saved"),
        ("arguments", "stdio"),
    ];
    assert!(form(&gateway, &fields).starts_with("HTTP/1.1 400"));
    fields.push(("confirm", "yes"));
    assert!(form(&gateway, &fields).starts_with("HTTP/1.1 303"));
    let html = browser(&gateway, "GET", "/console", "");
    assert!(html.contains("Enrolled ACP file read roots"));
    assert!(html.contains("artifact"));
    for lane in [Lane::Ask, Lane::Agent, Lane::Chat] {
        engine
            .dispatch(Request::SetRoute {
                lane,
                provider: "adapter".into(),
                model: "fixture/default".into(),
            })
            .unwrap();
    }
    engine
        .dispatch(Request::Ask {
            id: "without-grant".into(),
            prompt: "read:notes:/note.txt".into(),
        })
        .unwrap();
    assert_eq!(done(&engine, "without-grant")["answer"], "Denied by client capability policy");
    assert!(
        form(
            &gateway,
            &[
                ("action", "enroll_read_root"),
                ("provider", "adapter"),
                ("root_id", "notes"),
                ("path", notes.to_str().unwrap()),
                ("confirm", "yes")
            ]
        )
        .starts_with("HTTP/1.1 303")
    );
    assert!(
        form(&gateway, &[("action", "ask"), ("id", "allowed"), ("prompt", "read:notes:/note.txt")])
            .starts_with("HTTP/1.1 303")
    );
    let job = done(&engine, "allowed");
    assert_eq!(job["status"], "completed");
    assert_eq!(job["answer"], "REAL_ENROLLED_NOTE Δ\r\nSECOND_LINE\r\n");
    assert_eq!(job["actual_model"], "fixture/default");
    assert!(
        browser(&gateway, "GET", "/job/allowed", "")
            .contains("REAL_ENROLLED_NOTE Δ\r\nSECOND_LINE\r\n")
    );
    engine
        .dispatch(Request::Ask { id: "secret-denied".into(), prompt: "read:notes:/.env".into() })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "dynamic-model".into(), prompt: "model update".into() })
        .unwrap();
    assert_eq!(done(&engine, "dynamic-model")["actual_model"], "fixture/dynamic");
    assert_eq!(
        engine.dispatch(Request::Status {}).unwrap()["routes"]["ask"]["model"],
        "fixture/default"
    );
    for (id, prompt, expected) in [
        ("full-lf", "read:notes:/lf.txt".to_string(), "first\nlast\n".to_string()),
        ("full-bound", "read:notes:/bound.txt".to_string(), "x\n".repeat(16 * 1024)),
        (
            "range-crlf",
            format!("read-params:{}", json!({"path":"notes:/note.txt","line":2,"limit":1})),
            "SECOND_LINE\r\n".to_string(),
        ),
        (
            "range-last",
            format!("read-params:{}", json!({"path":"notes:/bound.txt","line":16384,"limit":1})),
            "x\n".to_string(),
        ),
        (
            "range-eof",
            format!("read-params:{}", json!({"path":"notes:/bound.txt","line":16385,"limit":1})),
            "<empty read>".to_string(),
        ),
        (
            "range-large-file",
            format!(
                "read-params:{}",
                json!({"path":"notes:/large-lines.txt","line":19000,"limit":1})
            ),
            "y\n".to_string(),
        ),
        (
            "invalid-utf8",
            "read:notes:/invalid-utf8.txt".to_string(),
            "Denied by client capability policy".to_string(),
        ),
        (
            "range-zero",
            format!("read-params:{}", json!({"path":"notes:/bound.txt","line":0,"limit":1})),
            "Denied by client capability policy".to_string(),
        ),
        (
            "range-large",
            format!("read-params:{}", json!({"path":"notes:/bound.txt","line":1,"limit":4097})),
            "Denied by client capability policy".to_string(),
        ),
        (
            "oversized",
            "read:notes:/oversized.txt".to_string(),
            "Denied by client capability policy".to_string(),
        ),
    ] {
        engine.dispatch(Request::Ask { id: id.into(), prompt }).unwrap();
        let job = done(&engine, id);
        assert_eq!(job["status"], "completed");
        assert_eq!(job["answer"], expected, "ACP file case {id}");
    }
    let denied = done(&engine, "secret-denied");
    assert_eq!(denied["answer"], "Denied by client capability policy");
    assert!(!denied.to_string().contains("PRIVATE_FIXTURE_SECRET"));
    engine
        .dispatch(Request::Ask {
            id: "kernel-denied".into(),
            prompt: "permission terminal kernel".into(),
        })
        .unwrap();
    let denied = done(&engine, "kernel-denied");
    assert_eq!(denied["needs_agent"], true);
    assert_eq!(denied["answer"], "Permission denied");
    engine
        .dispatch(Request::SetReadOnlyTools { provider: "adapter".into(), enabled: true })
        .unwrap();
    engine
        .dispatch(Request::Ask {
            id: "kernel-allowed".into(),
            prompt: "permission terminal kernel".into(),
        })
        .unwrap();
    assert_ne!(done(&engine, "kernel-allowed")["answer"], "Permission denied");
    for (id, lane) in [("agent", Lane::Agent), ("chat", Lane::Chat)] {
        engine.dispatch(Request::NewSession { id: id.into(), lane }).unwrap();
        engine
            .dispatch(Request::Prompt {
                id: format!("{id}-turn"),
                session: id.into(),
                prompt: format!("hello {id}"),
            })
            .unwrap();
        assert_eq!(
            done(&engine, &format!("{id}-turn"))["answer"],
            format!("ACP fixture: hello {id}")
        );
    }
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "adapter".into(),
            model: "unavailable-saved".into(),
        })
        .unwrap();
    engine.dispatch(Request::Ask { id: "unavailable".into(), prompt: "hello".into() }).unwrap();
    assert_eq!(done(&engine, "unavailable")["error"], "provider_unavailable");
    engine
        .dispatch(Request::RefreshModels { id: "catalog".into(), provider: "adapter".into() })
        .unwrap();
    assert_eq!(done(&engine, "catalog")["catalog_count"], 2);
    assert_eq!(
        engine.dispatch(Request::Status {}).unwrap()["routes"]["ask"]["model"],
        "unavailable-saved"
    );
    assert_eq!(
        engine.dispatch(Request::Ask { id: "no-fallback".into(), prompt: "hello".into() }),
        Err(Error::ProviderUnavailable)
    );
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Ask,
            provider: "adapter".into(),
            model: "fixture/default".into(),
        })
        .unwrap();
    engine
        .dispatch(Request::Ask { id: "cancel".into(), prompt: "wait for cancellation".into() })
        .unwrap();
    engine.dispatch(Request::Cancel { id: "cancel".into() }).unwrap();
    assert_eq!(done(&engine, "cancel")["status"], "cancelled");
    let usage = engine.dispatch(Request::Usage {}).unwrap();
    assert!(
        usage["activity"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["job"] != "cancel" && row["job"] != "unavailable")
    );
    assert!(form(&gateway, &fields).starts_with("HTTP/1.1 303"));
    assert!(
        engine.dispatch(Request::ReadRoots { provider: "adapter".into() }).unwrap()["roots"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(gateway);
    drop(engine);
    let deadline = Instant::now() + Duration::from_secs(3);
    let engine = loop {
        match Store::open(&state) {
            Ok(store) => break Engine::open_with_adapter_runner(store, runner.clone()).unwrap(),
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => panic!("ACP reopen {error:?}"),
        }
    };
    assert_eq!(done(&engine, "allowed")["answer"], "REAL_ENROLLED_NOTE Δ\r\nSECOND_LINE\r\n");
    drop(engine);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires actual restricted ACP namespace execution;malformed/session/size/process host proof"]
fn real_acp_refuses_malformed_or_cross_session_frames_and_reaps_descendants_on_cancel_or_timeout() {
    for (prompt, expected) in [
        ("bad session update", Error::ProviderFailed),
        ("wrong response id", Error::ProviderFailed),
        ("malformed frame", Error::ProviderFailed),
        ("error with private secret", Error::ProviderFailed),
        ("oversized frame", Error::Limit),
        ("wait for cancellation", Error::ProviderUnavailable),
    ] {
        let flag = AtomicBool::new(false);
        let mut surface = Surface::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut client = Client::connect(
            &config(),
            Path::new(env!("CARGO_BIN_EXE_ryoku-rashin")),
            deadline,
            &flag,
            &mut surface,
        )
        .unwrap();
        client.new_session(deadline, &flag, &mut surface).unwrap();
        assert_eq!(
            client.prompt(prompt, Instant::now() + Duration::from_millis(200), &flag, &mut surface),
            Err(expected)
        );
        assert!(surface.text.is_empty());
    }
    let flag = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let mut surface = Surface { ready: Some(ready_tx), ..Surface::default() };
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = Client::connect(
        &config(),
        Path::new(env!("CARGO_BIN_EXE_ryoku-rashin")),
        deadline,
        &flag,
        &mut surface,
    )
    .unwrap();
    client.new_session(deadline, &flag, &mut surface).unwrap();
    let manager = client.process_id();
    fn children(pid: u32, into: &mut Vec<(u32, String)>) {
        if let Ok(data) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) {
            for child in data.split_whitespace().map(|id| id.parse::<u32>().unwrap()) {
                if let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) {
                    let fields =
                        stat.rsplit_once(") ").unwrap().1.split_whitespace().collect::<Vec<_>>();
                    into.push((child, fields[19].into()));
                    children(child, into);
                }
            }
        }
    }
    let cancelled = Arc::clone(&flag);
    let thread = std::thread::spawn(move || {
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let mut descendants = Vec::new();
        children(manager, &mut descendants);
        assert!(descendants.len() >= 3);
        cancelled.store(true, Ordering::Release);
        descendants
    });
    assert_eq!(
        client.prompt("spawn descendant then wait", deadline, &flag, &mut surface),
        Err(Error::Cancelled)
    );
    drop(client);
    let descendants = thread.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let alive = descendants.iter().any(|(pid, start)| {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                let fields =
                    stat.rsplit_once(") ").unwrap().1.split_whitespace().collect::<Vec<_>>();
                fields[0] != "Z" && fields[19] == start
            })
        });
        if !alive {
            break;
        }
        assert!(Instant::now() < deadline, "ACP descendant survived cancellation");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires actual configured network-enabled ACP namespace;Fix egress consent host proof"]
fn network_enabled_acp_fix_requires_review_and_keeps_file_and_terminal_tools_disabled() {
    use ryoku_rashin::{
        api::{FixKind, Lane, Request},
        runtime::Engine,
        store::Store,
    };
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = std::env::temp_dir().join(format!("rashin-acp-fix-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let engine = Engine::open_with_adapter_runner(
        Store::open(&root).unwrap(),
        env!("CARGO_BIN_EXE_ryoku-rashin").into(),
    )
    .unwrap();
    engine
        .dispatch(Request::EnrollAcpProvider {
            id: "fix-adapter".into(),
            path: env!("CARGO_BIN_EXE_ryoku-rashin-acp-fixture").into(),
            models: vec!["fixture/default".into()],
            arguments: Arguments::Stdio,
            network: true,
        })
        .unwrap();
    engine
        .dispatch(Request::SetReadOnlyTools { provider: "fix-adapter".into(), enabled: true })
        .unwrap();
    engine
        .dispatch(Request::SetRoute {
            lane: Lane::Agent,
            provider: "fix-adapter".into(),
            model: "fixture/default".into(),
        })
        .unwrap();
    let observer = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    observer.set_nonblocking(true).unwrap();
    let evidence = format!("ACP_FIX_PERMISSION_KERNEL:{}", observer.local_addr().unwrap().port());
    let fix = |id: &str, token: Option<String>| Request::Fix {
        id: id.into(),
        kind: FixKind::Tip,
        component: "rashin-acp-fixture".into(),
        evidence: evidence.clone(),
        native_evidence_consent: token,
    };
    assert_eq!(engine.dispatch(fix("not-reviewed", None)), Err(Error::EvidenceConsentRequired));
    assert!(matches!(observer.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
    let preview = engine
        .dispatch(Request::FixPreview {
            kind: FixKind::Tip,
            component: "rashin-acp-fixture".into(),
            evidence: evidence.clone(),
        })
        .unwrap();
    let token = preview["native_evidence_consent"].as_str().unwrap().to_string();
    engine.dispatch(fix("reviewed", Some(token.clone()))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match observer.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("ACP Fix observer: {error}"),
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut message = [0u8; 19];
    std::io::Read::read_exact(&mut stream, &mut message).unwrap();
    assert_eq!(&message, b"ACP_FIX_AUTHORIZED\n");
    std::io::Write::write_all(&mut stream, b"OK").unwrap();
    let job = done(&engine, "reviewed");
    assert_eq!(job["status"], "completed");
    assert_eq!(job["answer"], "Permission denied");
    assert_ne!(job["needs_agent"], true);
    assert_eq!(
        engine.dispatch(fix("replay", Some(token.clone()))),
        Err(Error::EvidenceConsentRequired)
    );
    assert_eq!(engine.dispatch(fix("reviewed", Some(token))).unwrap()["status"], "completed");
    assert!(matches!(observer.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
    drop(engine);
    let deadline = Instant::now() + Duration::from_secs(2);
    while fs::remove_dir_all(&root).is_err() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
