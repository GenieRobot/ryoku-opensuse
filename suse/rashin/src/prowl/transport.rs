//! One connected IPv4 stream, authenticated before the first request byte.
//! Linux's established server-side reverse tuple identifies its socket UID.
//! Rebinding a listening port cannot replace this already-connected stream;
//! closure fails the request, with no reconnect or redirect. Same-UID processes
//! retain account authority; this protects against another account's listener.
use crate::{Error, Result};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    time::{Duration, Instant},
};
const BODY_LIMIT: usize = 1024 * 1024;
const HEADER_LIMIT: usize = 16 * 1024;
const PROC_LIMIT: u64 = 4 * 1024 * 1024;

pub(super) fn get(port: u16, token: &str, path: &str) -> Result<(u16, Vec<u8>)> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let mut stream = TcpStream::connect_timeout(&SocketAddr::V4(address), Duration::from_secs(1))
        .map_err(|_| Error::ProviderUnavailable)?;
    verify_peer(&stream, deadline)?;
    // No request, token, probe or Host header is sent before identity succeeds.
    stream.set_write_timeout(Some(remaining(deadline)?)).map_err(|_| Error::ProviderUnavailable)?;
    write!(stream,"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nAccept: application/json\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n").map_err(|_|Error::ProviderUnavailable)?;
    // Cap framing overhead too, rather than only decoded JSON body bytes.
    let mut reader = BufReader::new(Timed { stream: &mut stream, deadline })
        .take((BODY_LIMIT + 2 * HEADER_LIMIT) as u64);
    response(&mut reader)
}
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(Error::ProviderUnavailable)
}
pub(super) fn verify_peer(stream: &TcpStream, deadline: Instant) -> Result<()> {
    let SocketAddr::V4(client) = stream.local_addr().map_err(|_| Error::ProviderUnavailable)?
    else {
        return Err(Error::ProviderUnavailable);
    };
    let SocketAddr::V4(server) = stream.peer_addr().map_err(|_| Error::ProviderUnavailable)? else {
        return Err(Error::ProviderUnavailable);
    };
    if server.ip() != &Ipv4Addr::LOCALHOST || client.ip() != &Ipv4Addr::LOCALHOST {
        return Err(Error::Unauthorized);
    }
    wait_for_peer(deadline, || {
        let file = File::open("/proc/self/net/tcp").map_err(|_| Error::ProviderUnavailable)?;
        if rustix::fs::fstatfs(&file).map_err(|_| Error::ProviderUnavailable)?.f_type
            != rustix::fs::PROC_SUPER_MAGIC
        {
            return Err(Error::ProviderUnavailable);
        }
        let mut bytes = Vec::new();
        file.take(PROC_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::ProviderUnavailable)?;
        if bytes.len() as u64 > PROC_LIMIT {
            return Err(Error::Limit);
        }
        let rows = std::str::from_utf8(&bytes).map_err(|_| Error::ProviderUnavailable)?;
        peer_owner(rows, server, client, rustix::process::geteuid().as_raw())
    })
}
fn wait_for_peer(deadline: Instant, mut probe: impl FnMut() -> Result<bool>) -> Result<()> {
    loop {
        remaining(deadline)?;
        match probe() {
            Ok(true) => return Ok(()),
            // A procfs sequence can contain duplicate zero-inode rows while
            // the established socket is accepted/moved between hash buckets.
            // Missing/ambiguous rows never authorize a write. Re-read within
            // the original deadline until a unique owned nonzero inode exists.
            Ok(false) | Err(Error::ProviderUnavailable) => {}
            Err(error) => return Err(error), // Foreign UID is immediately denied.
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn tuple(address: SocketAddrV4) -> String {
    format!("{:08X}:{:04X}", u32::from_ne_bytes(address.ip().octets()), address.port())
}
fn peer_owner(
    rows: &str,
    server: SocketAddrV4,
    client: SocketAddrV4,
    expected: u32,
) -> Result<bool> {
    let mut lines = rows.lines();
    let header = lines.next().ok_or(Error::ProviderUnavailable)?;
    if !["local_address", "rem_address", "uid", "inode"]
        .iter()
        .all(|field| header.split_ascii_whitespace().any(|name| name == *field))
    {
        return Err(Error::ProviderUnavailable);
    }
    let local = tuple(server);
    let remote = tuple(client);
    let mut matched = None;
    for row in lines {
        let fields = row.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.get(1) != Some(&local.as_str())
            || fields.get(2) != Some(&remote.as_str())
            || fields.get(3) != Some(&"01")
        {
            continue;
        }
        let uid = fields
            .get(7)
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or(Error::ProviderUnavailable)?;
        let inode = fields
            .get(9)
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(Error::ProviderUnavailable)?;
        if matched.replace((uid, inode)).is_some() {
            return Err(Error::ProviderUnavailable);
        }
    }
    match matched {
        Some((_, 0)) | None => Ok(false),
        Some((uid, _)) if uid == expected => Ok(true),
        Some(_) => Err(Error::Unauthorized),
    }
}
struct Timed<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl Read for Timed<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = remaining(self.deadline)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}
fn line(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= limit {
            return Err(Error::Limit);
        }
        let mut byte = [0];
        reader.read_exact(&mut byte).map_err(|_| Error::ProviderUnavailable)?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len() - 2);
            return Ok(bytes);
        }
    }
}
fn response(reader: &mut impl Read) -> Result<(u16, Vec<u8>)> {
    let mut bytes = Vec::new();
    let status = response_to(reader, &mut |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok((status, bytes))
}
pub(super) fn response_to(
    reader: &mut impl Read,
    emit: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<u16> {
    let first = line(reader, HEADER_LIMIT)?;
    let mut header_size = first.len() + 2;
    let first = std::str::from_utf8(&first).map_err(|_| Error::ProviderFailed)?;
    let mut fields = first.splitn(3, ' ');
    if !matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(Error::ProviderFailed);
    }
    let status = fields
        .next()
        .filter(|code| code.len() == 3)
        .and_then(|code| code.parse::<u16>().ok())
        .filter(|code| (200..=599).contains(code))
        .ok_or(Error::ProviderFailed)?;
    let mut headers = BTreeMap::new();
    loop {
        let row = line(reader, HEADER_LIMIT.saturating_sub(header_size))?;
        header_size += row.len() + 2;
        if row.is_empty() {
            break;
        }
        let row = std::str::from_utf8(&row).map_err(|_| Error::ProviderFailed)?;
        let (name, value) = row.split_once(':').ok_or(Error::ProviderFailed)?;
        if name.is_empty()
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value.bytes().any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err(Error::ProviderFailed);
        }
        if headers.insert(name.to_ascii_lowercase(), value.trim().to_owned()).is_some() {
            return Err(Error::ProviderFailed);
        }
    }
    if status != 200 {
        return Ok(status);
    }
    if headers.get("content-encoding").is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(Error::ProviderFailed);
    }

    match (headers.get("content-length"), headers.get("transfer-encoding")) {
        (Some(_), Some(_)) => return Err(Error::ProviderFailed),
        (Some(length), None) => {
            let length = length.parse::<usize>().map_err(|_| Error::ProviderFailed)?;
            if length > BODY_LIMIT {
                return Err(Error::Limit);
            }
            transfer(reader, length, emit)?;
        }
        (None, Some(encoding)) if encoding.eq_ignore_ascii_case("chunked") => {
            chunked_to(reader, emit)?
        }
        (None, Some(_)) => return Err(Error::ProviderFailed),
        (None, None) => {
            let mut total = 0usize;
            let mut chunk = [0; 8192];
            loop {
                let count = reader.read(&mut chunk).map_err(|_| Error::ProviderUnavailable)?;
                if count == 0 {
                    break;
                }
                total = total
                    .checked_add(count)
                    .filter(|total| *total <= BODY_LIMIT)
                    .ok_or(Error::Limit)?;
                emit(&chunk[..count])?;
            }
        }
    }
    Ok(status)
}
fn transfer(
    reader: &mut impl Read,
    mut length: usize,
    emit: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut bytes = [0; 8192];
    while length > 0 {
        let count =
            reader.read(&mut bytes[..length.min(8192)]).map_err(|_| Error::ProviderUnavailable)?;
        if count == 0 {
            return Err(Error::ProviderUnavailable);
        }
        emit(&bytes[..count])?;
        length -= count;
    }
    Ok(())
}
fn chunked_to(reader: &mut impl Read, emit: &mut impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
    let mut total = 0usize;
    loop {
        let size = line(reader, 1024)?;
        let size = std::str::from_utf8(&size)
            .map_err(|_| Error::ProviderFailed)?
            .split(';')
            .next()
            .ok_or(Error::ProviderFailed)?;
        if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::ProviderFailed);
        }
        let size = usize::from_str_radix(size, 16).map_err(|_| Error::Limit)?;
        if size == 0 {
            let mut count = 0;
            loop {
                let trailer = line(reader, HEADER_LIMIT.saturating_sub(count))?;
                count += trailer.len() + 2;
                if trailer.is_empty() {
                    return Ok(());
                }
                if !trailer.contains(&b':')
                    || trailer.iter().any(|byte| byte.is_ascii_control() && *byte != b'\t')
                {
                    return Err(Error::ProviderFailed);
                }
            }
        }
        total = total.checked_add(size).filter(|total| *total <= BODY_LIMIT).ok_or(Error::Limit)?;
        transfer(reader, size, emit)?;
        let mut delimiter = [0; 2];
        reader.read_exact(&mut delimiter).map_err(|_| Error::ProviderUnavailable)?;
        if delimiter != *b"\r\n" {
            return Err(Error::ProviderFailed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        net::TcpListener,
        os::fd::AsRawFd,
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    fn table(row: &str) -> String {
        format!(
            "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n{row}\n"
        )
    }
    #[test]
    fn peer_poll_requires_unique_owned_inode_after_ambiguity_and_never_retries_foreign_uid() {
        let server = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8788);
        let client = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000);
        let header = "sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n";
        let row = |uid, inode| {
            format!("0: {} {} 01 0:0 00:0 0 {uid} 0 {inode}\n", tuple(server), tuple(client))
        };
        let ambiguous = format!("{header}{}{}", row(1000, 0), row(1000, 0));
        let owned = format!("{header}{}", row(1000, 42));
        let foreign = format!("{header}{}", row(1001, 43));
        let mut calls = 0;
        assert_eq!(
            wait_for_peer(Instant::now() + Duration::from_secs(1), || {
                calls += 1;
                peer_owner(if calls == 1 { &ambiguous } else { &owned }, server, client, 1000)
            }),
            Ok(())
        );
        assert_eq!(calls, 2);
        assert_eq!(
            wait_for_peer(Instant::now() + Duration::from_millis(5), || peer_owner(
                &ambiguous, server, client, 1000
            )),
            Err(Error::ProviderUnavailable)
        );
        calls = 0;
        assert_eq!(
            wait_for_peer(Instant::now() + Duration::from_secs(1), || {
                calls += 1;
                peer_owner(&foreign, server, client, 1000)
            }),
            Err(Error::Unauthorized)
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn established_reverse_tuple_requires_unambiguous_owned_inode_not_a_listening_port() {
        let server = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8788);
        let client = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 40000);
        let row = format!(
            "0: {} {} 01 00000000:00000000 00:00000000 00000000 1000 0 12345",
            tuple(server),
            tuple(client)
        );
        assert_eq!(peer_owner(&table(&row), server, client, 1000), Ok(true));
        assert_eq!(peer_owner(&table(&row), server, client, 1001), Err(Error::Unauthorized));
        assert_eq!(peer_owner(&table(&row.replace("12345", "0")), server, client, 1000), Ok(false));
        assert_eq!(
            peer_owner(&table(&row.replace(" 01 ", " 0A ")), server, client, 1000),
            Ok(false)
        );
        assert_eq!(peer_owner(&table(&row), client, server, 1000), Ok(false));
        assert_eq!(
            peer_owner(&table(&format!("{row}\n{row}")), server, client, 1000),
            Err(Error::ProviderUnavailable)
        );
        assert_eq!(
            peer_owner(&table(&row.replace(" 1000 ", " bad ")), server, client, 1000),
            Err(Error::ProviderUnavailable)
        );
        assert_eq!(
            peer_owner("invalid procfs", server, client, 1000),
            Err(Error::ProviderUnavailable)
        );
    }
    #[test]
    fn response_framing_rejects_ambiguous_oversized_truncated_and_malformed_data() {
        for raw in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n[]".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n[]",
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\n[]",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n[]xx",
            b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n[]",
        ] {
            assert_eq!(response(&mut Cursor::new(raw)), Err(Error::ProviderFailed));
        }
        assert_eq!(
            response(&mut Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n")),
            Err(Error::Limit)
        );
        assert_eq!(
            response(&mut Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n[]")),
            Err(Error::ProviderUnavailable)
        );
        assert_eq!(response(&mut Cursor::new(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;extension=ok\r\n[]\r\n0\r\nX-End: yes\r\n\r\n")),Ok((200,b"[]".to_vec())));
    }
    #[test]
    fn connected_same_uid_is_verified_without_sending_request_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        verify_peer(&client, Instant::now() + Duration::from_secs(3)).unwrap();
        server.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let mut byte = [0];
        let error = server.read(&mut byte).unwrap_err();
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
        drop(client);
        assert_eq!(server.read(&mut byte).unwrap(), 0);
    }
    #[test]
    fn foreign_listener_fixture() {
        if std::env::var_os("RASHIN_FOREIGN_LISTENER").is_none() {
            return;
        }
        assert_eq!(rustix::process::geteuid().as_raw(), 1);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        eprintln!("PROWL_FIXTURE_PORT={}", listener.local_addr().unwrap().port());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("foreign accept: {error}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty(), "foreign listener received request bytes");
        eprintln!("PROWL_FIXTURE_NO_BYTES");
    }
    // Run explicitly inside an existing subordinate-ID namespace, without
    // creating host accounts or requiring root. Portable CI does not assume
    // subordinate mappings; the receipt records this separately executed test.
    #[test]
    #[ignore = "requires user namespace mapping owner UID0 and foreign UID1"]
    fn connected_foreign_uid_receives_no_request_or_management_credential() {
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        for completion in [false, true] {
            let executable = File::open(std::env::current_exe().unwrap()).unwrap();
            // Resolve the already-open ELF through procfs after setuid, avoiding
            // any need to traverse the owner's private workspace as the foreign UID.
            let mut command = Command::new(format!("/proc/self/fd/{}", executable.as_raw_fd()));
            command
                .uid(1)
                .gid(1)
                .env("RASHIN_FOREIGN_LISTENER", "1")
                .args([
                    "--exact",
                    "prowl::transport::tests::foreign_listener_fixture",
                    "--nocapture",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            struct Guard(std::process::Child);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
            let mut child = Guard(command.spawn().unwrap());
            let mut stderr = child.0.stderr.take().unwrap();
            rustix::fs::fcntl_setfl(&stderr, rustix::fs::OFlags::NONBLOCK).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut diagnostic = Vec::new();
            let port = loop {
                let mut chunk = [0; 256];
                match stderr.read(&mut chunk) {
                    Ok(count) => diagnostic.extend_from_slice(&chunk[..count]),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(error) => panic!("foreign fixture readiness: {error}"),
                }
                assert!(diagnostic.len() < 2048);
                if let Some(port) = String::from_utf8_lossy(&diagnostic).lines().find_map(|line| {
                    line.strip_prefix("PROWL_FIXTURE_PORT=")
                        .and_then(|port| port.parse::<u16>().ok())
                }) {
                    break port;
                }
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "foreign fixture exited: {}",
                    String::from_utf8_lossy(&diagnostic)
                );
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            };
            let result = if completion {
                crate::prowl::Connection { port, token: "PRIVATE_PROWL_FIXTURE_TOKEN".into() }
                    .complete(
                        "auto:fast",
                        &[crate::provider::Message {
                            role: crate::provider::Role::User,
                            content: "PRIVATE_PROMPT_MUST_NOT_REACH_FOREIGN_UID".into(),
                        }],
                        true,
                        &std::sync::atomic::AtomicBool::new(false),
                        &mut |_| Ok(()),
                    )
                    .map(|_| ())
            } else {
                get(port, "PRIVATE_PROWL_FIXTURE_TOKEN", "/api/profiles").map(|_| ())
            };
            assert_eq!(result, Err(Error::Unauthorized));
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            let mut tail = Vec::new();
            stderr.read_to_end(&mut tail).unwrap();
            diagnostic.extend(tail);
            assert!(String::from_utf8_lossy(&diagnostic).contains("PROWL_FIXTURE_NO_BYTES"));
        }
    }
}
