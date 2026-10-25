//! Deterministic ACP adapter for offline process/permission/terminal acceptance.
//! This executable speaks the real protocol; it performs no model/network calls.
use serde_json::{Value, json};
use std::io::{BufRead, Write};
fn send(value: Value) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &value)?;
    out.write_all(b"\n")?;
    out.flush()
}
fn reply(id: Value, result: Value) -> std::io::Result<()> {
    send(json!({"jsonrpc":"2.0","id":id,"result":result}))
}
fn update(session: &str, kind: &str, text: &str) -> std::io::Result<()> {
    send(
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":kind,"content":{"type":"text","text":text}}}}),
    )
}
fn answer(session: &str, id: Value, text: &str) -> std::io::Result<()> {
    update(session, "agent_message_chunk", text)?;
    reply(id, json!({"stopReason":"end_turn"}))
}
fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--fixture-descendant") {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    let mut session = String::new();
    let mut serial = 0u64;
    let mut pending: Option<(Value, String)> = None;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.len() > 128 * 1024 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let frame: Value = serde_json::from_str(&line)?;
        if frame["jsonrpc"] != "2.0" {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        if frame.get("method").is_none() {
            let Some((id, operation)) = pending.take() else {
                return Err(std::io::ErrorKind::InvalidData.into());
            };
            if frame.get("error").is_some() {
                answer(&session, id, "Denied by client capability policy")?;
                continue;
            }
            let text = if operation == "read" {
                match frame["result"]["content"].as_str() {
                    Some("") => "<empty read>".into(),
                    Some(text) => text.into(),
                    None => "Invalid read result".into(),
                }
            } else {
                let allowed = frame["result"]["outcome"]["optionId"] == "allow-once";
                if allowed {
                    let output = std::process::Command::new("/usr/bin/uname")
                        .arg("-r")
                        .env_clear()
                        .output()?;
                    if !output.status.success() {
                        return Err(std::io::ErrorKind::Other.into());
                    }
                    String::from_utf8(output.stdout).map_err(|_| std::io::ErrorKind::InvalidData)?
                } else {
                    "Permission denied".into()
                }
            };
            if operation == "terminal" {
                send(
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"tool_call_update","toolCallId":"terminal-kernel","status":if text=="Permission denied" {"failed"} else {"completed"}}}}),
                )?;
            }
            answer(&session, id, &text)?;
            continue;
        }
        let id = frame["id"].clone();
        let params = &frame["params"];
        match frame["method"].as_str().unwrap_or("") {
            "initialize" => {
                if params["protocolVersion"] != 1 {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                reply(
                    id,
                    json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true}},"agentInfo":{"name":"deterministic-rust-acp"},"private_fixture_field":"PRIVATE_UNKNOWN_FIELD"}),
                )?;
            }
            "session/new" => {
                serial += 1;
                session = format!("fixture-session-{serial}");
                reply(
                    id,
                    json!({"sessionId":session,"models":{"currentModelId":"fixture/default","availableModels":[{"modelId":"fixture/default","name":"Default"},{"modelId":"fixture/λ","name":"Opaque model"}]}}),
                )?;
            }
            "session/load" => {
                session =
                    params["sessionId"].as_str().ok_or(std::io::ErrorKind::InvalidData)?.into();
                update(&session, "agent_message_chunk", "Replayed private session")?;
                reply(
                    id,
                    json!({"models":{"currentModelId":"fixture/λ","availableModels":[{"modelId":"fixture/λ","name":"Opaque model"}]}}),
                )?;
            }
            "session/set_model" => reply(id, json!({}))?,
            "session/prompt" => {
                if params["sessionId"] != session {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                let text =
                    params["prompt"][0]["text"].as_str().ok_or(std::io::ErrorKind::InvalidData)?;
                let decoded: Option<Vec<Value>> = serde_json::from_str(text).ok();
                let text = decoded
                    .as_ref()
                    .and_then(|rows| rows.iter().rev().find(|row| row["role"] == "user"))
                    .and_then(|row| row["content"].as_str())
                    .unwrap_or(text);

                if text == "model update" {
                    send(
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"current_model_update","currentModelId":"fixture/dynamic"}}}),
                    )?;
                    answer(&session, id, "Updated actual model")?;
                    continue;
                }
                if text == "wait for cancellation" {
                    pending = Some((id, "waiting".into()));
                    continue;
                }
                if text == "spawn descendant then wait" {
                    let _child = std::process::Command::new("/adapter")
                        .arg("--fixture-descendant")
                        .spawn()?;
                    update(&session, "agent_message_chunk", "descendant ready")?;
                    pending = Some((id, "waiting".into()));
                    continue;
                }
                if text == "wrong response id" {
                    reply(json!(999999), json!({"stopReason":"end_turn"}))?;
                    continue;
                }
                if text == "malformed frame" {
                    std::io::stdout().write_all(b"not json\n")?;
                    std::io::stdout().flush()?;
                    continue;
                }
                if text == "error with private secret" {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"PRIVATE_DIAGNOSTIC","data":{"token":"PRIVATE_ERROR_TOKEN"}}}),
                    )?;
                    continue;
                }
                if text == "bad session update" {
                    update("wrong-session", "agent_message_chunk", "PRIVATE_WRONG_SESSION")?;
                    continue;
                }
                if text == "oversized frame" {
                    send(
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":session,"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x".repeat(128*1024)}}}}),
                    )?;
                    continue;
                }
                if let Some(raw) = text.strip_prefix("read-params:") {
                    let mut params: Value = serde_json::from_str(raw)?;
                    params["sessionId"] = json!(session);
                    pending = Some((id, "read".into()));
                    send(
                        json!({"jsonrpc":"2.0","id":"read-range-opaque","method":"fs/read_text_file","params":params}),
                    )?;
                    continue;
                }
                if let Some(path) = text.strip_prefix("read:") {
                    pending = Some((id, "read".into()));
                    send(
                        json!({"jsonrpc":"2.0","id":"read-opaque-λ","method":"fs/read_text_file","params":{"sessionId":session,"path":path}}),
                    )?;
                    continue;
                }
                if text == "permission terminal kernel"
                    || text.contains("ACP_FIX_PERMISSION_KERNEL")
                {
                    if let Some(suffix) = text.split("ACP_FIX_PERMISSION_KERNEL:").nth(1) {
                        let port = suffix
                            .chars()
                            .take_while(|ch| ch.is_ascii_digit())
                            .collect::<String>()
                            .parse::<u16>()
                            .map_err(|_| std::io::ErrorKind::InvalidData)?;
                        let mut observer =
                            std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))?;
                        observer.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
                        observer.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
                        observer.write_all(b"ACP_FIX_AUTHORIZED\n")?;
                        let mut ack = [0u8; 2];
                        std::io::Read::read_exact(&mut observer, &mut ack)?;
                        assert_eq!(&ack, b"OK");
                    }

                    pending = Some((id, "terminal".into()));
                    send(
                        json!({"jsonrpc":"2.0","id":"permission-opaque","method":"session/request_permission","params":{"sessionId":session,"toolCall":{"toolCallId":"terminal-kernel","kind":"execute","title":"kernel read","rawInput":{"command":"/usr/bin/uname","args":["-r"]}},"options":[{"optionId":"allow-once","kind":"allow_once","name":"Allow once"},{"optionId":"reject-once","kind":"reject_once","name":"Deny"}]}}),
                    )?;
                    continue;
                }
                if text == "jail boundary" {
                    let status = std::fs::read_to_string("/proc/self/status")?;
                    assert!(status.lines().any(|line| line == "Uid:\t65534\t65534\t65534\t65534"));
                    assert!(status.lines().any(|line| line == "CapEff:\t0000000000000000"));
                    assert!(status.lines().any(|line| line == "NoNewPrivs:\t1"));
                    assert!(!std::path::Path::new("/home/jin").exists());
                    assert!(std::fs::write("/usr/forbidden", "no").is_err());
                    assert!(std::fs::read("/proc/1/root/etc/passwd").is_err());
                    std::fs::write("/tmp/allowed-ephemeral", "private fixture")?;
                    std::os::unix::fs::symlink("/proc/1/root/etc/passwd", "/tmp/parent-escape")?;
                    assert!(std::fs::read("/tmp/parent-escape").is_err());
                    assert!(std::fs::read("/proc/1/fd/0").is_err());
                    std::fs::create_dir("/tmp/mount-deny")?;
                    assert!(
                        !std::process::Command::new("/usr/bin/mount")
                            .args(["--bind", "/usr/bin", "/tmp/mount-deny"])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()?
                            .success()
                    );
                    assert!(
                        !std::process::Command::new("/usr/bin/kill")
                            .args(["-0", "1"])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()?
                            .success()
                    );
                    let mut fds = std::fs::read_dir("/proc/self/fd")?
                        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .collect::<Vec<_>>();
                    fds.sort();
                    assert_eq!(fds, ["0", "1", "2", "3"]);

                    answer(&session, id, "Verified restricted jail")?;
                    continue;
                }
                answer(&session, id, &format!("ACP fixture: {text}"))?;
            }
            "session/cancel" => {
                if params["sessionId"] != session {
                    return Err(std::io::ErrorKind::InvalidData.into());
                }
                if let Some((id, _)) = pending.take() {
                    reply(id, json!({"stopReason":"cancelled"}))?;
                }
            }
            _ => send(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported fixture method"}}),
            )?,
        }
    }
    Ok(())
}
