//! Exact helper client shared by the retained native hook handoff and the
//! explicit account approval tool. No command or executable crosses this wire.
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    time::Duration,
};
pub fn exchange(request: serde_json::Value, expected: &str) -> io::Result<serde_json::Value> {
    let mut stream = UnixStream::connect("/run/ryoku/helper.sock")?;
    if rustix::net::sockopt::socket_peercred(&stream)?.uid.as_raw() != 0 {
        return Err(io::Error::other("hook helper peer is not root"));
    }
    exchange_stream(&mut stream, request, expected)
}
fn exchange_stream(
    stream: &mut UnixStream,
    request: serde_json::Value,
    expected: &str,
) -> io::Result<serde_json::Value> {
    let id = format!("hook-{}", std::process::id());
    let request = serde_json::json!({"version":1,"request_id":id,"request":request});
    let mut bytes = serde_json::to_vec(&request).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > 64 * 1024 {
        return Err(io::Error::other("hook request exceeds bound"));
    }
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    stream.write_all(&bytes)?;
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    std::io::Read::take(reader.by_ref(), 65537).read_until(b'\n', &mut bytes)?;
    if bytes.len() > 65536 || bytes.last() != Some(&b'\n') {
        return Err(io::Error::other("hook reply incomplete or too large"));
    }
    let reply: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if reply["version"] != 1 || reply["request_id"] != id {
        return Err(io::Error::other("hook reply correlation mismatch"));
    }
    let outcome = &reply["outcome"];
    if outcome["result"] == "rejected" {
        return Err(io::Error::other(format!(
            "hook refused: {}; use ryoku-hook authorize for the current command and existing captured outputs; inspect linked operation history for capture/apply failure",
            outcome["error"]
        )));
    }
    if outcome["result"] != expected
        || !outcome["operation_id"].is_string()
        || outcome["receipt"]["completed"] != true
        || !outcome["receipt"]["refusal"].is_null()
    {
        return Err(io::Error::other("hook helper did not attest durable completion"));
    }
    Ok(outcome.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_hook_reply_requires_exact_correlation_kind_and_completed_receipt() {
        for mode in 0..5 {
            let (mut client, mut helper) = UnixStream::pair().unwrap();
            let server = std::thread::spawn(move || {
                let mut line = String::new();
                BufReader::new(helper.try_clone().unwrap()).read_line(&mut line).unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["request"]["operation"], "run_configured_hook");
                let mut reply = serde_json::json!({"version":1,"request_id":request["request_id"],"outcome":{"result":"configured_hook_completed","operation_id":"hook-test","receipt":{"completed":true,"refusal":null}}});
                match mode {
                    1 => reply["request_id"] = "wrong".into(),
                    2 => reply["outcome"]["receipt"]["completed"] = false.into(),
                    3 => reply["outcome"]["result"] = "hook_policy_authorized".into(),
                    4 => {
                        reply["outcome"] = serde_json::json!({"result":"rejected","error":{"code":"recovery_unavailable"}})
                    }
                    _ => {}
                }
                serde_json::to_writer(&mut helper, &reply).unwrap();
                helper.write_all(b"\n").unwrap();
            });
            let result = exchange_stream(
                &mut client,
                serde_json::json!({"operation":"run_configured_hook","action":{"index":0}}),
                "configured_hook_completed",
            );
            assert_eq!(result.is_ok(), mode == 0);
            server.join().unwrap();
        }
    }
}
