//! Explicit public HTTP(S) text GETs. DNS results are validated and pinned for
//! each hop; curl never follows redirects or receives account credentials.
use crate::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use url::{Host, Url};
const HEADERS: usize = 16 * 1024;
const BODY: usize = 24 * 1024;
fn public4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && ((c == 0 && !([9, 10].contains(&d))) || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}
fn prefix6(ip: Ipv6Addr, network: [u16; 8], bits: u32) -> bool {
    ip.to_bits() >> (128 - bits) == Ipv6Addr::from(network).to_bits() >> (128 - bits)
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public4(v4);
            }
            let s = ip.segments();
            if prefix6(ip, [0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96) {
                let b = ip.octets();
                return public4(Ipv4Addr::new(b[12], b[13], b[14], b[15]));
            }
            if s[0] & 0xe000 != 0x2000
                || prefix6(ip, [0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32)
                || prefix6(ip, [0x3fff, 0, 0, 0, 0, 0, 0, 0], 20)
            {
                return false;
            }
            if s[0] == 0x2002 {
                let b = ip.octets();
                return public4(Ipv4Addr::new(b[2], b[3], b[4], b[5]));
            }
            if prefix6(ip, [0x2001, 0, 0, 0, 0, 0, 0, 0], 23) {
                if s[1] == 0 {
                    let b = ip.octets();
                    return public4(Ipv4Addr::new(b[4], b[5], b[6], b[7]))
                        && public4(Ipv4Addr::new(!b[12], !b[13], !b[14], !b[15]));
                }
                return [1, 2, 3]
                    .iter()
                    .any(|last| ip == Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, *last))
                    || prefix6(ip, [0x2001, 3, 0, 0, 0, 0, 0, 0], 32)
                    || prefix6(ip, [0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48)
                    || prefix6(ip, [0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28)
                    || prefix6(ip, [0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28);
            }
            true
        }
    }
}
fn target(raw: &str) -> Result<Url> {
    if raw.len() > 8192 || raw.bytes().any(|b| b.is_ascii_control()) {
        return Err(Error::InvalidRequest);
    }
    let mut url = Url::parse(raw.trim()).map_err(|_| Error::InvalidRequest)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default().is_none_or(|p| p == 0)
    {
        return Err(Error::InvalidRequest);
    }
    match url.host().ok_or(Error::InvalidRequest)? {
        Host::Ipv4(ip) => {
            if !public_ip(ip.into()) {
                return Err(Error::Unauthorized);
            }
        }
        Host::Ipv6(ip) => {
            if !public_ip(ip.into()) {
                return Err(Error::Unauthorized);
            }
        }
        Host::Domain(host) => {
            if host.trim_end_matches('.').eq_ignore_ascii_case("localhost") || host.is_empty() {
                return Err(Error::Unauthorized);
            }
        }
    }
    url.set_fragment(None);
    Ok(url)
}
fn check(cancelled: &AtomicBool, deadline: Instant) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else if Instant::now() >= deadline {
        Err(Error::ProviderUnavailable)
    } else {
        Ok(())
    }
}
struct Output {
    bytes: Vec<u8>,
    cut: bool,
}
fn run(
    command: Command,
    input: Option<&[u8]>,
    limit: usize,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Output> {
    run_mode(command, input, limit, cancelled, deadline, false)
}
fn run_mode(
    mut command: Command,
    input: Option<&[u8]>,
    limit: usize,
    cancelled: &AtomicBool,
    deadline: Instant,
    partial: bool,
) -> Result<Output> {
    check(cancelled, deadline)?;
    if input.is_some_and(|v| v.len() > 128 * 1024) {
        return Err(Error::Limit);
    }
    let mut child = command.spawn().map_err(|_| Error::ProviderUnavailable)?;
    let result = (|| {
        if input.is_some() {
            // curl waits for stdin config, so limits apply before any request.
            use rustix::process::{Pid, Resource, Rlimit, getrlimit, prlimit};
            for (resource, cap) in
                [(Resource::As, 512 * 1024 * 1024), (Resource::Cpu, 8), (Resource::Core, 0)]
            {
                let old = getrlimit(resource);
                let limit = old.current.unwrap_or(cap).min(old.maximum.unwrap_or(cap)).min(cap);
                prlimit(
                    Some(Pid::from_child(&child)),
                    resource,
                    Rlimit { current: Some(limit), maximum: Some(limit) },
                )
                .map_err(|_| Error::ProviderUnavailable)?;
            }
        }
        let mut stdin = if input.is_some() {
            Some(child.stdin.take().ok_or(Error::ProviderFailed)?)
        } else {
            None
        };
        if let Some(stdin) = &stdin {
            rustix::fs::fcntl_setfl(stdin, rustix::fs::OFlags::NONBLOCK)
                .map_err(|_| Error::ProviderFailed)?;
        }
        let mut stdout = child.stdout.take().ok_or(Error::ProviderFailed)?;
        rustix::fs::fcntl_setfl(&stdout, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| Error::ProviderFailed)?;
        let mut input_offset = 0;
        let mut bytes = Vec::new();
        loop {
            check(cancelled, deadline)?;
            if let (Some(pipe), Some(input)) = (&mut stdin, input) {
                match pipe.write(&input[input_offset..]) {
                    Ok(count) => input_offset += count,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => return Err(Error::ProviderUnavailable),
                }
                if input_offset == input.len() {
                    stdin.take();
                }
            }
            let mut chunk = [0; 4096];
            match stdout.read(&mut chunk) {
                Ok(0) => loop {
                    check(cancelled, deadline)?;
                    if let Some(status) = child.try_wait().map_err(|_| Error::ProviderFailed)? {
                        if !status.success() && !(partial && !bytes.is_empty()) {
                            return Err(Error::ProviderUnavailable);
                        }
                        return Ok(Output { bytes, cut: !status.success() });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                },
                Ok(count) => {
                    let remaining = limit.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..count.min(remaining)]);
                    if count > remaining {
                        return Ok(Output { bytes, cut: true });
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err(Error::ProviderUnavailable),
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn command(path: &str) -> Command {
    let mut cmd = Command::new(path);
    cmd.env_clear().env("LANG", "C").stdout(Stdio::piped()).stderr(Stdio::null());
    cmd
}
fn dns_output(bytes: &[u8]) -> Result<Vec<IpAddr>> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::ProviderUnavailable)?;
    let mut ips = BTreeSet::new();
    for line in text.lines() {
        let ip: IpAddr = line
            .split_whitespace()
            .next()
            .ok_or(Error::ProviderUnavailable)?
            .parse()
            .map_err(|_| Error::ProviderUnavailable)?;
        if !public_ip(ip) {
            return Err(Error::Unauthorized);
        }
        ips.insert(ip);
        if ips.len() > 16 {
            return Err(Error::Limit);
        }
    }
    if ips.is_empty() {
        return Err(Error::ProviderUnavailable);
    }
    Ok(ips.into_iter().collect())
}
fn resolve(url: &Url, cancelled: &AtomicBool, deadline: Instant) -> Result<Vec<IpAddr>> {
    match url.host().ok_or(Error::InvalidRequest)? {
        Host::Ipv4(ip) => Ok(vec![ip.into()]),
        Host::Ipv6(ip) => Ok(vec![ip.into()]),
        Host::Domain(host) => {
            let mut cmd = command("/usr/bin/getent");
            cmd.args(["-A", "ahosts", "--", host]).stdin(Stdio::null());
            let output = run(cmd, None, 16 * 1024, cancelled, deadline)?;
            if output.cut {
                return Err(Error::Limit);
            }
            dns_output(&output.bytes)
        }
    }
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn curl(url: &Url, ips: &[IpAddr]) -> Result<(Command, String)> {
    if ips.is_empty() || ips.iter().any(|ip| !public_ip(*ip)) {
        return Err(Error::Unauthorized);
    }
    let mut cmd = command("/usr/bin/curl");
    cmd.args([
        "--disable",
        "--silent",
        "--show-error",
        "--globoff",
        "--no-buffer",
        "--http1.1",
        "--include",
        "--compressed",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
        "--proxy",
        "",
        "--noproxy",
        "*",
        "--connect-timeout",
        "3",
        "--max-time",
        "12",
        "--max-redirs",
        "0",
        "--retry",
        "0",
        "--config",
        "-",
    ])
    .stdin(Stdio::piped());
    let host = url.host_str().ok_or(Error::InvalidRequest)?;
    let port = url.port_or_known_default().ok_or(Error::InvalidRequest)?;
    let addresses = ips
        .iter()
        .map(|ip| match ip {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        })
        .collect::<Vec<_>>()
        .join(",");
    let resolve = if matches!(url.host(), Some(Host::Domain(_))) {
        format!("resolve = {}\n", quote(&format!("{host}:{port}:{addresses}")))
    } else {
        let literal = match url.host().ok_or(Error::InvalidRequest)? {
            Host::Ipv4(ip) => IpAddr::V4(ip),
            Host::Ipv6(ip) => IpAddr::V6(ip),
            Host::Domain(_) => return Err(Error::InvalidRequest),
        };
        if ips != [literal] {
            return Err(Error::Unauthorized);
        }
        String::new()
    };
    let config = format!(
        "url = {}\nrequest = \"GET\"\n{resolve}user-agent = \"ryoku-rashin/quick\"\n",
        quote(url.as_str())
    );
    if config.len() > 16 * 1024 {
        return Err(Error::Limit);
    }
    Ok((cmd, config))
}
struct Response {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
    truncated: bool,
    charset: Option<String>,
}
fn response(output: Output) -> Result<Response> {
    let mut offset = 0;
    for _ in 0..5 {
        let end = output.bytes[offset..]
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| offset + p + 4)
            .ok_or(Error::ProviderFailed)?;
        if end > HEADERS {
            return Err(Error::Limit);
        }
        let head =
            std::str::from_utf8(&output.bytes[offset..end]).map_err(|_| Error::ProviderFailed)?;
        let mut lines = head.split("\r\n");
        let first = lines.next().ok_or(Error::ProviderFailed)?;
        let mut words = first.split_whitespace();
        if !matches!(words.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
            return Err(Error::ProviderFailed);
        }
        let status = words
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|s| (100..=599).contains(s))
            .ok_or(Error::ProviderFailed)?;
        let mut location = None;
        let mut charset = None;
        for line in lines.filter(|l| !l.is_empty()) {
            let (name, value) = line.split_once(':').ok_or(Error::ProviderFailed)?;
            if name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || value.bytes().any(|b| b.is_ascii_control() && b != b'\t')
            {
                return Err(Error::ProviderFailed);
            }
            if name.eq_ignore_ascii_case("content-type") {
                for item in value.split(';').skip(1) {
                    if let Some((name, value)) = item.split_once('=')
                        && name.trim().eq_ignore_ascii_case("charset")
                    {
                        let label = value.trim().trim_matches(['\"', '\'']);
                        if label.is_empty()
                            || label.len() > 64
                            || !label
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                            || charset.replace(label.to_owned()).is_some()
                        {
                            return Err(Error::ProviderFailed);
                        }
                    }
                }
            }
            if name.eq_ignore_ascii_case("location")
                && location.replace(value.trim().to_owned()).is_some()
            {
                return Err(Error::ProviderFailed);
            }
        }
        offset = end;
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        let mut body = output.bytes[end..].to_vec();
        let truncated = output.cut || body.len() > BODY;
        body.truncate(BODY);
        return Ok(Response { status, location, body, truncated, charset });
    }
    Err(Error::ProviderFailed)
}
fn decode(reply: &Response, cancelled: &AtomicBool, deadline: Instant) -> Result<(String, bool)> {
    if reply.charset.as_deref().is_none_or(|v| {
        v.eq_ignore_ascii_case("utf-8")
            || v.eq_ignore_ascii_case("utf8")
            || v.eq_ignore_ascii_case("us-ascii")
    }) {
        let text = String::from_utf8_lossy(&reply.body);
        let lossy = matches!(text, std::borrow::Cow::Owned(_));
        return Ok((text.into_owned(), lossy));
    }
    let charset = reply.charset.as_ref().ok_or(Error::ProviderFailed)?;
    let mut cmd = command("/usr/bin/iconv");
    cmd.args([format!("--from-code={charset}"), "--to-code=UTF-8".into()]).stdin(Stdio::piped());
    let output = run_mode(cmd, Some(&reply.body), 128 * 1024, cancelled, deadline, true)?;
    let text = String::from_utf8(output.bytes).map_err(|_| Error::ProviderFailed)?;
    Ok((text, output.cut))
}
fn plain(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut inside = false;
    let mut skip: Option<&str> = None;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                let tag = rest[1..end].trim().to_ascii_lowercase();
                if skip.is_none() && (tag.starts_with("script") || tag.starts_with("style")) {
                    skip = Some(if tag.starts_with("script") { "script" } else { "style" });
                } else if skip.is_some_and(|s| tag.starts_with(&format!("/{s}"))) {
                    skip = None;
                }
                inside = true;
                i += end + 1;
                continue;
            }
            break;
        }
        let ch = rest.chars().next().expect("nonempty UTF8 suffix");
        i += ch.len_utf8();
        if skip.is_some() {
            continue;
        }
        if inside && !out.ends_with(char::is_whitespace) {
            out.push(' ');
        }
        inside = false;
        out.push(ch);
        if out.len() > 8 * 1024 {
            break;
        }
    }
    while out.len() > 8 * 1024 {
        out.pop();
    }
    out.trim().to_owned()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    url: String,
}
fn fetch_with(
    raw: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
    mut lookup: impl FnMut(&Url, &AtomicBool, Instant) -> Result<Vec<IpAddr>>,
) -> Result<Value> {
    let mut url = target(raw)?;
    let deadline = deadline.min(Instant::now() + Duration::from_secs(12));
    for hop in 0..=3 {
        check(cancelled, deadline)?;
        let ips = lookup(&url, cancelled, deadline)?;
        let (command, config) = curl(&url, &ips)?;
        let reply =
            response(run(command, Some(config.as_bytes()), HEADERS + BODY, cancelled, deadline)?)?;
        if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
            if hop == 3 {
                return Err(Error::Limit);
            }
            let location = reply.location.ok_or(Error::ProviderFailed)?;
            let next = url.join(&location).map_err(|_| Error::InvalidRequest)?;
            url = target(next.as_str())?;
            continue;
        }
        let (decoded, lossy) = decode(&reply, cancelled, deadline)?;
        let text = plain(decoded.as_bytes());
        return Ok(
            json!({"status":reply.status,"url":url.as_str(),"text":text,"truncated":reply.truncated||lossy||text.len()>=8*1024,"charset":reply.charset,"decoding_lossy":lossy}),
        );
    }
    Err(Error::Limit)
}
pub(super) fn execute(args: &str, cancelled: &AtomicBool, deadline: Instant) -> Result<String> {
    let args: Arguments = serde_json::from_str(args).map_err(|_| Error::InvalidRequest)?;
    Ok(fetch_with(&args.url, cancelled, deadline, resolve)?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_iana_non_global_prefixes_and_embedded_private_addresses_are_blocked() {
        // IANA special-purpose IPv4/IPv6 registries, updated2025-10-09;
        // https://www.iana.org/assignments/iana-ipv6-special-registry
        assert!(prefix6("2001:2::1".parse().unwrap(), [0x2001, 0, 0, 0, 0, 0, 0, 0], 23));
        for ip in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "100.100.100.200",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.8",
            "192.0.0.170",
            "192.0.2.1",
            "192.88.99.2",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "64:ff9b:1::1",
            "100::1",
            "100:0:0:1::1",
            "2001:2::1",
            "2001:2:0:ffff::1",
            "2001:10::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "3fff::1",
            "5f00::1",
            "fc00::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "nonpublic {ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "11.0.0.1",
            "192.0.0.9",
            "192.0.0.10",
            "192.31.196.1",
            "192.52.193.1",
            "192.175.48.1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2001:1::1",
            "2001:1::2",
            "2001:1::3",
            "2001:3::1",
            "2001:4:112::1",
            "2001:20::1",
            "2001:30::1",
            "2001:4860:4860::8888",
            "2002:808:808::1",
            "2620:4f:8000::1",
        ] {
            assert!(public_ip(ip.parse().unwrap()), "public {ip}");
        }
    }
    #[test]
    fn url_parser_normalizes_idna_and_unusual_ip_spellings_without_private_or_auth_escape() {
        for url in [
            "file:///etc/passwd",
            "gopher://8.8.8.8/",
            "http://localhost/",
            "http://localhost./",
            "http://127.1/",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0177.0.0.1/",
            "http://%31%32%37.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://[fe80::1%25eth0]/",
            "https://owner:password@example.com/",
            "http://example.com/\r\nX: y",
        ] {
            assert!(target(url).is_err(), "refuse {url}");
        }
        let url = target("https://éxample.org/λ?q=γ#section").unwrap();
        assert_eq!(url.host_str(), Some("xn--xample-9ua.org"));
        assert!(url.fragment().is_none());
        assert!(url.as_str().contains("%CE%BB"));
    }
    #[test]
    fn every_dns_address_is_validated_and_mixed_private_answers_never_get_a_pin() {
        assert!(matches!(
            dns_output(b"8.8.8.8 STREAM example\n10.0.0.1 STREAM example\n"),
            Err(Error::Unauthorized)
        ));
        assert!(matches!(
            dns_output(b"2001:4860::1 STREAM example\n2001:2::1 STREAM example\n"),
            Err(Error::Unauthorized)
        ));
        assert_eq!(
            dns_output(b"8.8.8.8 STREAM example\n8.8.8.8 DGRAM example\n").unwrap(),
            vec!["8.8.8.8".parse::<IpAddr>().unwrap()]
        );
        assert!(dns_output(b"not-an-ip\n").is_err());
        assert!(dns_output(b"").is_err());
    }
    #[test]
    fn installed_getent_resolver_reads_local_hosts_and_refuses_loopback_results() {
        let url = Url::parse("http://localhost/").unwrap();
        assert_eq!(
            resolve(&url, &AtomicBool::new(false), Instant::now() + Duration::from_secs(2)),
            Err(Error::Unauthorized)
        );
    }
    #[test]
    fn fixed_curl_environment_and_stdin_pin_have_no_url_auth_shell_or_automatic_redirect() {
        let url = target("https://example.invalid:8443/path?q=value").unwrap();
        let (cmd, config) =
            curl(&url, &["8.8.8.8".parse().unwrap(), "2001:4860::1".parse().unwrap()]).unwrap();
        assert_eq!(cmd.get_program(), "/usr/bin/curl");
        let args = cmd.get_args().map(|a| a.to_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--disable",
                "--silent",
                "--show-error",
                "--globoff",
                "--no-buffer",
                "--http1.1",
                "--include",
                "--compressed",
                "--proto",
                "=http,https",
                "--proto-redir",
                "=http,https",
                "--proxy",
                "",
                "--noproxy",
                "*",
                "--connect-timeout",
                "3",
                "--max-time",
                "12",
                "--max-redirs",
                "0",
                "--retry",
                "0",
                "--config",
                "-"
            ]
        );
        assert!(args.iter().all(|a| !a.contains("example.invalid") && !a.contains("q=value")));
        assert_eq!(cmd.get_envs().count(), 1);
        assert!(config.contains("example.invalid:8443:8.8.8.8,[2001:4860::1]"));
        assert!(!config.contains("Authorization"));
        assert!(!config.contains("Cookie"));
        assert!(curl(&url, &["127.0.0.1".parse().unwrap()]).is_err());
        let literal = target("http://[2001:4860::1]/").unwrap();
        let (_, config) = curl(&literal, &["2001:4860::1".parse().unwrap()]).unwrap();
        assert!(!config.contains("resolve ="));
    }
    #[test]
    fn http_headers_interim_status_html_and_output_limits_are_bounded() {
        let r=response(Output{bytes:b"HTTP/1.1 103 Early Hints\r\nLink: ignored\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<h1>Read \xCE\xBB</h1><script>PRIVATE_SCRIPT_TEXT</script><style>PRIVATE_STYLE_TEXT</style><p>body</p>".to_vec(),cut:false}).unwrap();
        assert_eq!(r.status, 200);
        let text = plain(&r.body);
        assert!(text.contains("Read λ"));
        assert!(text.contains("body"));
        assert!(!text.contains("PRIVATE_"));
        assert!(plain("λ".repeat(10000).as_bytes()).len() <= 8 * 1024);
        assert!(
            response(Output {
                bytes: b"HTTP/1.1 302 Found\r\nLocation: /one\r\nLocation: /two\r\n\r\n".to_vec(),
                cut: false
            })
            .is_err()
        );
        assert!(response(Output { bytes: vec![b'x'; HEADERS + 1], cut: false }).is_err());
    }
    #[test]
    fn missing_tools_and_cancel_before_spawn_fail_without_fallback_or_network() {
        let mut cmd = command("/nonexistent/ryoku-public-web-tool");
        cmd.stdin(Stdio::null());
        assert!(matches!(
            run(cmd, None, 128, &AtomicBool::new(false), Instant::now() + Duration::from_secs(1)),
            Err(Error::ProviderUnavailable)
        ));
        assert!(matches!(
            execute(
                r#"{"url":"http://8.8.8.8/"}"#,
                &AtomicBool::new(true),
                Instant::now() + Duration::from_secs(1)
            ),
            Err(Error::Cancelled)
        ));
    }
}

#[cfg(test)]
mod net_tests {
    use super::*;
    use std::{
        net::{SocketAddr, TcpListener},
        sync::mpsc,
    };
    fn setup() {
        let parent = std::env::var("RASHIN_PUBLIC_WEB_PARENT_NET")
            .expect("run only inside documented fresh user+net namespace");
        let current = std::fs::read_link("/proc/self/ns/net").unwrap();
        assert_ne!(
            current.to_str().unwrap(),
            parent,
            "refuse changes to the caller network namespace"
        );
        for args in [
            vec!["link", "set", "lo", "up"],
            vec!["address", "add", "11.0.0.1/32", "dev", "lo"],
            vec!["address", "add", "11.0.0.2/32", "dev", "lo"],
        ] {
            assert!(Command::new("/usr/sbin/ip").args(args).status().unwrap().success());
        }
    }
    fn server(
        replies: Vec<Vec<u8>>,
    ) -> (u16, mpsc::Receiver<(SocketAddr, String)>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("0.0.0.0:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            for reply in replies {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(e) => panic!("web fixture accept {e}"),
                    }
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    assert!(bytes.len() < 8192);
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let header = String::from_utf8(bytes).unwrap();
                assert!(header.starts_with("GET "));
                assert!(!header.to_ascii_lowercase().contains("authorization:"));
                assert!(!header.to_ascii_lowercase().contains("cookie:"));
                tx.send((stream.local_addr().unwrap(), header)).unwrap();
                if let Err(e) = stream.write_all(&reply) {
                    assert!(matches!(
                        e.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ));
                }
            }
        });
        (port, rx, worker)
    }
    fn http(body: &[u8], extra: &str) -> Vec<u8> {
        let mut bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }
    #[test]
    fn charset_conversion_is_bounded_and_nonblocking_without_changing_any_file() {
        let reply = Response {
            status: 200,
            location: None,
            body: vec![0xe9; BODY],
            truncated: false,
            charset: Some("ISO-8859-1".into()),
        };
        let (text, lossy) =
            decode(&reply, &AtomicBool::new(false), Instant::now() + Duration::from_secs(2))
                .unwrap();
        assert!(!lossy);
        assert_eq!(text, "é".repeat(BODY));
        assert_eq!(plain(text.as_bytes()).len(), 8 * 1024);
        let invalid = Response { charset: Some("no_such_charset".into()), ..reply };
        assert!(
            decode(&invalid, &AtomicBool::new(false), Instant::now() + Duration::from_secs(1))
                .is_err()
        );
    }
    #[test]
    #[ignore = "requires fresh user+network namespace and RASHIN_PUBLIC_WEB_PARENT_NET guard"]
    fn isolated_public_http_redirects_dns_pins_compression_and_cancel_use_real_curl() {
        setup();
        let flag = AtomicBool::new(false);
        let (port,requests,worker)=server(vec![b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),http(b"<h1>public text</h1><script>PRIVATE_SCRIPT</script>","Content-Type: text/html\r\n")]);
        let mut lookups = 0;
        let value = fetch_with(
            &format!("http://fixture.invalid:{port}/start"),
            &flag,
            Instant::now() + Duration::from_secs(4),
            |_, _, _| {
                lookups += 1;
                Ok(vec![if lookups == 1 { "11.0.0.1" } else { "11.0.0.2" }.parse().unwrap()])
            },
        )
        .unwrap();
        assert!(value["text"].as_str().unwrap().contains("public text"));
        assert!(!value.to_string().contains("PRIVATE_SCRIPT"));
        assert_eq!(lookups, 2);
        let first = requests.recv_timeout(Duration::from_secs(1)).unwrap();
        let next = requests.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(first.0.ip(), "11.0.0.1".parse::<IpAddr>().unwrap());
        assert_eq!(next.0.ip(), "11.0.0.2".parse::<IpAddr>().unwrap());
        assert!(next.1.starts_with("GET /final "));
        worker.join().unwrap();
        let (port,requests,worker)=server(vec![b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()]);
        let mut lookups = 0;
        assert_eq!(
            fetch_with(
                &format!("http://fixture.invalid:{port}/"),
                &flag,
                Instant::now() + Duration::from_secs(3),
                |_, _, _| {
                    lookups += 1;
                    Ok(vec![if lookups == 1 { "11.0.0.1" } else { "127.0.0.1" }.parse().unwrap()])
                }
            ),
            Err(Error::Unauthorized)
        );
        assert_eq!(lookups, 2);
        requests.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        let (port,requests,worker)=server(vec![b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/private\r\nContent-Length: 0\r\n\r\n".to_vec()]);
        assert_eq!(
            fetch_with(
                &format!("http://11.0.0.1:{port}/"),
                &flag,
                Instant::now() + Duration::from_secs(2),
                resolve
            ),
            Err(Error::Unauthorized)
        );
        requests.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        let redirects=vec![b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();4];
        let (port, requests, worker) = server(redirects);
        assert_eq!(
            fetch_with(
                &format!("http://11.0.0.1:{port}/"),
                &flag,
                Instant::now() + Duration::from_secs(3),
                resolve
            ),
            Err(Error::Limit)
        );
        let served = (0..4)
            .map(|_| requests.recv_timeout(Duration::from_secs(1)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(served.len(), 4);
        worker.join().unwrap();
        let mut gzip = Command::new("/usr/bin/gzip")
            .args(["-n", "-c"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        gzip.stdin.take().unwrap().write_all(b"<p>caf\xe9</p>").unwrap();
        let compressed = gzip.wait_with_output().unwrap();
        assert!(compressed.status.success());
        let (port, requests, worker) = server(vec![http(
            &compressed.stdout,
            "Content-Encoding: gzip\r\nContent-Type: text/html; charset=ISO-8859-1\r\n",
        )]);
        let value = fetch_with(
            &format!("http://11.0.0.1:{port}/"),
            &flag,
            Instant::now() + Duration::from_secs(3),
            resolve,
        )
        .unwrap();
        assert_eq!(value["text"], "café");
        assert_eq!(value["decoding_lossy"], false);
        requests.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        let listener = TcpListener::bind("11.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                stream.read_exact(&mut b).unwrap();
                header.push(b[0]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\nwaiting").unwrap();
            stream.flush().unwrap();
            ready_tx.send(()).unwrap();
            let mut b = [0];
            match stream.read(&mut b) {
                Ok(0) => {}
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("cancel web socket {other:?}"),
            }
        });
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let task_flag = cancelled.clone();
        let task = std::thread::spawn(move || {
            fetch_with(
                &format!("http://11.0.0.1:{port}/"),
                &task_flag,
                Instant::now() + Duration::from_secs(3),
                resolve,
            )
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancelled.store(true, Ordering::Release);
        assert_eq!(task.join().unwrap(), Err(Error::Cancelled));
        worker.join().unwrap();
    }
}
