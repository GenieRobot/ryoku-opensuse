//! A deliberately unregistered libzypp commit-plugin protocol probe.
//!
//! Pinned libzypp source shows that plugin errors do not veto a commit.
//! This module measures that boundary; it cannot provide a system write gate.

use std::io::{self, Read, Write};

const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 64;

#[derive(Debug, PartialEq, Eq)]
struct Frame {
    command: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
enum FrameRead {
    Eof,
    Frame(Frame),
}

#[derive(Clone, Copy, Debug)]
struct Policy<'a> {
    /// A recognized libzypp frame to deny explicitly. `None` permits all
    /// recognized frames and is useful only for protocol tests.
    deny_command: Option<&'a str>,
    deny_message: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Ack,
    Deny(String),
}

fn read_frame(reader: &mut impl Read) -> io::Result<FrameRead> {
    let mut prefix = Vec::new();
    loop {
        let byte = read_byte(reader, prefix.is_empty())?;
        let Some(byte) = byte else {
            return Ok(FrameRead::Eof);
        };
        prefix.push(byte);
        if prefix.len() > MAX_HEADER_BYTES {
            return Err(invalid_data("libzypp frame headers exceed size limit"));
        }
        if prefix.ends_with(b"\n\n") {
            break;
        }
    }

    let body_header = prefix;
    let header_end = body_header.len() - 2;
    let header_text = std::str::from_utf8(&body_header[..header_end])
        .map_err(|_| invalid_data("libzypp frame header is not UTF-8"))?;
    let mut lines = header_text.split('\n');
    let command = lines
        .next()
        .filter(|command| valid_command(command))
        .ok_or_else(|| invalid_data("invalid libzypp frame command"))?
        .to_owned();

    let mut headers = Vec::new();
    let mut content_length = None;
    for line in lines {
        if headers.len() == MAX_HEADER_COUNT {
            return Err(invalid_data("libzypp frame has too many headers"));
        }
        let (raw_key, raw_value) = line
            .split_once(':')
            .ok_or_else(|| invalid_data("libzypp frame header is missing ':'"))?;
        let key = unescape_header(raw_key)?;
        let value = unescape_header(raw_value)?;
        if key.is_empty() || key.bytes().any(|byte| byte == b'\n' || byte == b'\r') {
            return Err(invalid_data("invalid libzypp frame header name"));
        }
        if key == "content-length" {
            if content_length.is_some() {
                return Err(invalid_data("duplicate libzypp content-length header"));
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| invalid_data("invalid libzypp content-length"))?,
            );
        } else {
            headers.push((key, value));
        }
    }

    let mut body = Vec::new();
    if let Some(length) = content_length {
        if body_header.len().saturating_add(length).saturating_add(1) > MAX_FRAME_BYTES {
            return Err(invalid_data("libzypp frame exceeds size limit"));
        }
        body.resize(length, 0);
        reader.read_exact(&mut body)?;
        let mut terminator = [0];
        reader.read_exact(&mut terminator)?;
        if terminator[0] != 0 {
            return Err(invalid_data("libzypp frame is missing NUL terminator"));
        }
    } else {
        loop {
            let Some(byte) = read_byte(reader, false)? else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "libzypp frame is missing NUL terminator",
                ));
            };
            if byte == 0 {
                break;
            }
            if body_header.len() + body.len() + 1 > MAX_FRAME_BYTES {
                return Err(invalid_data("libzypp frame exceeds size limit"));
            }
            body.push(byte);
        }
    }

    Ok(FrameRead::Frame(Frame { command, headers, body }))
}

fn read_byte(reader: &mut impl Read, allow_clean_eof: bool) -> io::Result<Option<u8>> {
    let mut byte = [0];
    match reader.read(&mut byte)? {
        0 if allow_clean_eof => Ok(None),
        0 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated libzypp frame")),
        _ => Ok(Some(byte[0])),
    }
}

fn valid_command(command: &str) -> bool {
    !command.is_empty()
        && command.bytes().all(|byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

fn unescape_header(value: &str) -> io::Result<String> {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('\\') => output.push('\\'),
            Some('c') => output.push(':'),
            _ => return Err(invalid_data("invalid libzypp header escape")),
        }
    }
    Ok(output)
}

fn decide(frame: &Frame, policy: Policy<'_>) -> Decision {
    const KNOWN: &[&str] = &["PLUGINBEGIN", "COMMITBEGIN", "COMMITEND", "PLUGINEND"];
    if !KNOWN.contains(&frame.command.as_str()) {
        return Decision::Deny(format!("unsupported libzypp frame: {}", frame.command));
    }
    if policy.deny_command == Some(frame.command.as_str()) {
        return Decision::Deny(policy.deny_message.to_owned());
    }
    Decision::Ack
}

fn write_response(writer: &mut impl Write, decision: Decision) -> io::Result<()> {
    let (command, headers, body) = match decision {
        Decision::Ack => ("ACK", Vec::new(), Vec::new()),
        Decision::Deny(message) => ("ERROR", vec![("message".to_owned(), message)], Vec::new()),
    };

    write!(writer, "{command}\ncontent-length:0\n")?;
    for (key, value) in headers {
        writeln!(writer, "{}:{}", escape_header(&key, true), escape_header(&value, false))?;
    }
    writer.write_all(b"\n")?;
    writer.write_all(&body)?;
    writer.write_all(&[0])?;
    writer.flush()
}

fn escape_header(value: &str, escape_colon: bool) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\\' => output.push_str("\\\\"),
            ':' if escape_colon => output.push_str("\\c"),
            _ => output.push(ch),
        }
    }
    output
}

/// Process one frame. EOF is clean only before a new frame begins. Unknown
/// frames receive ERROR and are never acknowledged.
#[cfg(test)]
fn process_one(stream: &mut (impl Read + Write), policy: Policy<'_>) -> io::Result<bool> {
    let frame = match read_frame(stream)? {
        FrameRead::Eof => return Ok(false),
        FrameRead::Frame(frame) => frame,
    };
    write_response(stream, decide(&frame, policy))?;
    Ok(true)
}

/// Run as a libzypp commit plugin. The mode is inherited from the disposable
/// experiment process and exists only to characterize the guest's behavior.
#[allow(dead_code)] // The same protocol module is included by vm-lab tests and the probe binary.
pub(crate) fn serve() -> io::Result<()> {
    let mode = std::env::var("RYOKU_ZYPP_PROBE_MODE").unwrap_or_else(|_| "deny-commit".into());
    let deny_command = match mode.as_str() {
        "deny-commit" | "malformed-commit" | "timeout-commit" => Some("COMMITBEGIN"),
        "observe-commit" => None,
        _ => return Err(invalid_data("unsupported zypp probe mode")),
    };
    if let Some(path) = std::env::var_os("RYOKU_ZYPP_PROBE_LOG") {
        use std::io::Write;
        use std::{fs::OpenOptions, path::Path};
        let mut log = OpenOptions::new().append(true).open(Path::new(&path))?;
        writeln!(log, "probe_pid={}", std::process::id())?;
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    loop {
        let frame = match read_frame(&mut input)? {
            FrameRead::Eof => break,
            FrameRead::Frame(frame) => frame,
        };
        if let Some(path) = std::env::var_os("RYOKU_ZYPP_PROBE_LOG") {
            use std::io::Write;
            use std::{fs::OpenOptions, path::Path};
            let mut log = OpenOptions::new().append(true).open(Path::new(&path))?;
            writeln!(log, "probe_frame={}", frame.command)?;
        }
        if Some(frame.command.as_str()) == deny_command {
            match mode.as_str() {
                "deny-commit" => write_response(
                    &mut output,
                    Decision::Deny("Ryoku disposable probe denied COMMITBEGIN".into()),
                )?,
                "malformed-commit" => {
                    output.write_all(b"NOT-A-LIBZYPP-RESPONSE\n\n\0")?;
                    output.flush()?;
                    break;
                }
                "timeout-commit" => loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                },
                _ => unreachable!(),
            }
        } else {
            write_response(
                &mut output,
                decide(
                    &frame,
                    Policy { deny_command: None, deny_message: "Ryoku disposable probe" },
                ),
            )?;
        }
    }
    Ok(())
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct MockStream {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl MockStream {
        fn new(input: &[u8]) -> Self {
            Self { input: Cursor::new(input.to_vec()), output: Vec::new() }
        }
    }

    impl Read for MockStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for MockStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.output.flush()
        }
    }

    const ALLOW: Policy<'static> =
        Policy { deny_command: None, deny_message: "denied by probe policy" };

    #[test]
    fn acknowledges_known_plugin_frame_with_libzypp_content_length() {
        let mut stream = MockStream::new(b"PLUGINBEGIN\n\n\0");
        assert!(process_one(&mut stream, ALLOW).unwrap());
        assert_eq!(stream.output, b"ACK\ncontent-length:0\n\n\0");
    }

    #[test]
    fn configured_deny_returns_explicit_error_before_acknowledging_commit() {
        let mut stream = MockStream::new(b"COMMITBEGIN\n\n{}\0");
        let policy =
            Policy { deny_command: Some("COMMITBEGIN"), deny_message: "checkpoint unavailable" };
        assert!(process_one(&mut stream, policy).unwrap());
        assert_eq!(stream.output, b"ERROR\ncontent-length:0\nmessage:checkpoint unavailable\n\n\0");
    }

    #[test]
    fn unknown_frame_fails_closed_with_error_response() {
        let mut stream = MockStream::new(b"REPOCHANGE\n\n\0");
        assert!(process_one(&mut stream, ALLOW).unwrap());
        assert_eq!(
            stream.output,
            b"ERROR\ncontent-length:0\nmessage:unsupported libzypp frame: REPOCHANGE\n\n\0"
        );
    }

    #[test]
    fn reads_content_length_bodies_containing_nul_without_consuming_next_frame() {
        let mut stream = MockStream::new(b"COMMITBEGIN\ncontent-length:3\n\na\0b\0PLUGINEND\n\n\0");
        assert_eq!(
            read_frame(&mut stream).unwrap(),
            FrameRead::Frame(Frame {
                command: "COMMITBEGIN".to_owned(),
                headers: Vec::new(),
                body: b"a\0b".to_vec(),
            })
        );
        assert_eq!(
            read_frame(&mut stream).unwrap(),
            FrameRead::Frame(Frame {
                command: "PLUGINEND".to_owned(),
                headers: Vec::new(),
                body: Vec::new(),
            })
        );
    }

    #[test]
    fn rejects_invalid_header_escape_and_truncated_frame() {
        let mut bad_escape = MockStream::new(b"PLUGINBEGIN\nx:bad\\t\n\n\0");
        assert_eq!(read_frame(&mut bad_escape).unwrap_err().kind(), io::ErrorKind::InvalidData);

        let mut truncated = MockStream::new(b"PLUGINBEGIN\n\npartial");
        assert_eq!(read_frame(&mut truncated).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn enforces_frame_size_limit() {
        let mut input = b"PLUGINBEGIN\n\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', MAX_FRAME_BYTES));
        let mut stream = MockStream::new(&input);
        assert_eq!(read_frame(&mut stream).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
