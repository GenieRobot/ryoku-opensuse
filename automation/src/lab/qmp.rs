//! Bounded QMP access to the lab's privately owned Unix socket.
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    thread,
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE: usize = 65_536;
const MAX_EVENTS: usize = 64;
const MAX_DEPTH: usize = 32;

pub struct QmpClient {
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl QmpClient {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let stream = loop {
            remaining(deadline)?;
            match UnixStream::connect(path) {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error),
            }
        };
        Self::handshake(stream, deadline)
    }

    fn handshake(stream: UnixStream, deadline: Instant) -> io::Result<Self> {
        let mut client = Self { reader: BufReader::new(stream), next_id: 1 };
        let greeting = client.read_message(deadline)?;
        let Json::Object(greeting) = greeting else {
            return Err(invalid("QMP greeting is not an object"));
        };
        let Some(Json::Object(qmp)) = greeting.get("QMP") else {
            return Err(invalid("missing QMP greeting"));
        };
        if greeting.contains_key("return") || greeting.contains_key("error") {
            return Err(invalid("unexpected reply in QMP greeting"));
        }
        if !matches!(qmp.get("version"), Some(Json::Object(_))) {
            return Err(invalid("missing QMP version"));
        }
        match qmp.get("capabilities") {
            Some(Json::Array(values)) if values.iter().all(|v| matches!(v, Json::String(_))) => {}
            _ => return Err(invalid("invalid QMP capabilities")),
        }
        let response = client.request("qmp_capabilities", None, deadline)?;
        expect_empty(response)?;
        Ok(client)
    }

    /// Discover the port already bound by QEMU; never reserve and release a port.
    pub fn forwarded_ssh_port(&mut self) -> io::Result<u16> {
        let response = self.request(
            "human-monitor-command",
            Some(r#"{"command-line":"info usernet"}"#),
            Instant::now() + REQUEST_TIMEOUT,
        )?;
        let Json::String(output) = response else {
            return Err(invalid("QMP usernet reply is not a string"));
        };
        forwarded_port(&output)
    }

    pub fn send_key(&mut self, qcode: &str) -> io::Result<()> {
        validate_qcode(qcode)?;
        let arguments = format!(r#"{{"keys":[{{"type":"qcode","data":"{qcode}"}}]}}"#);
        let response =
            self.request("send-key", Some(&arguments), Instant::now() + REQUEST_TIMEOUT)?;
        expect_empty(response)
    }

    /// Ask this VM's monitor to save its current VGA framebuffer. Callers use
    /// a fixed filename in QEMU's private run directory, never a user path.
    pub fn screendump(&mut self, filename: &str) -> io::Result<()> {
        validate_screenshot_filename(filename)?;
        let arguments = format!(r#"{{"command-line":"screendump {filename}"}}"#);
        let response = self.request(
            "human-monitor-command",
            Some(&arguments),
            Instant::now() + REQUEST_TIMEOUT,
        )?;
        match response {
            Json::String(message) if message.is_empty() => Ok(()),
            Json::String(_) => Err(io::Error::other("QMP screendump failed")),
            _ => Err(invalid("unexpected QMP screendump result")),
        }
    }

    fn request(
        &mut self,
        command: &str,
        arguments: Option<&str>,
        deadline: Instant,
    ) -> io::Result<Json> {
        // Commands and argument JSON are private constants or validated qcodes.
        let id = format!("ryoku-qmp-{}", self.next_id);
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| invalid("QMP ID exhausted"))?;
        let args = arguments.map(|value| format!(r#", "arguments":{value}"#)).unwrap_or_default();
        let request = format!("{{\"execute\":\"{command}\",\"id\":\"{id}\"{args}}}\n");
        let mut bytes = request.as_bytes();
        while !bytes.is_empty() {
            self.reader.get_mut().set_write_timeout(Some(remaining(deadline)?))?;
            let count = self.reader.get_mut().write(bytes)?;
            if count == 0 {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "QMP write returned zero"));
            }
            bytes = &bytes[count..];
        }
        for _ in 0..=MAX_EVENTS {
            let Json::Object(mut response) = self.read_message(deadline)? else {
                return Err(invalid("QMP reply is not an object"));
            };
            if response.contains_key("event") {
                if !matches!(response.get("event"), Some(Json::String(_)))
                    || ["id", "return", "error"].iter().any(|key| response.contains_key(*key))
                {
                    return Err(invalid("malformed QMP event"));
                }
                continue;
            }
            if !matches!(response.get("id"), Some(Json::String(actual)) if actual == &id) {
                return Err(invalid("QMP reply ID mismatch"));
            }
            if response.contains_key("error") {
                return Err(io::Error::other("QMP command returned an error"));
            }
            return response.remove("return").ok_or_else(|| invalid("missing QMP return field"));
        }
        Err(invalid("too many QMP events before reply"))
    }

    fn read_message(&mut self, deadline: Instant) -> io::Result<Json> {
        let mut message = Vec::new();
        loop {
            self.reader.get_mut().set_read_timeout(Some(remaining(deadline)?))?;
            let available = self.reader.fill_buf()?;
            if available.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "QMP closed before reply",
                ));
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            if message.len() + count > MAX_MESSAGE {
                return Err(invalid("QMP message exceeds size limit"));
            }
            message.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            if newline.is_some() {
                return Parser::parse(&message);
            }
        }
    }
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "QMP deadline expired"))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn expect_empty(value: Json) -> io::Result<()> {
    match value {
        Json::Object(fields) if fields.is_empty() => Ok(()),
        _ => Err(invalid("unexpected QMP command result")),
    }
}

fn validate_qcode(qcode: &str) -> io::Result<()> {
    if matches!(qcode, "ret" | "tab")
        || (qcode.len() == 1
            && qcode.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))
    {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "unsupported lab key"))
    }
}

fn validate_screenshot_filename(filename: &str) -> io::Result<()> {
    if filename.len() <= 64
        && filename.ends_with(".ppm")
        && filename.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
        && !filename.starts_with('.')
        && !filename.contains("..")
    {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "unsupported screenshot filename"))
    }
}

fn forwarded_port(output: &str) -> io::Result<u16> {
    let mut net0_sections = 0;
    let mut in_net0 = false;
    let mut port = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix("Hub ") {
            let Some((hub, name)) = section.split_once(' ') else {
                return Err(invalid("malformed usernet section"));
            };
            hub.parse::<i32>().map_err(|_| invalid("invalid usernet hub ID"))?;
            in_net0 = name == "(net0):";
            if in_net0 {
                net0_sections += 1;
            }
            continue;
        }
        if !line.contains("[HOST_FORWARD]") {
            continue;
        }
        if !in_net0 || port.is_some() {
            return Err(invalid("unexpected or duplicate host forwarding rule"));
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 8
            || fields[0] != "TCP[HOST_FORWARD]"
            || fields[2] != "127.0.0.1"
            || fields[4] != "10.0.2.15"
            || fields[5] != "22"
        {
            return Err(invalid("unexpected SSH forwarding endpoints"));
        }
        for index in [1, 6, 7] {
            fields[index].parse::<u64>().map_err(|_| invalid("invalid usernet numeric field"))?;
        }
        let parsed = fields[3].parse::<u16>().map_err(|_| invalid("invalid forwarded SSH port"))?;
        if parsed == 0 {
            return Err(invalid("QEMU has not allocated an SSH port"));
        }
        port = Some(parsed);
    }
    if net0_sections != 1 {
        return Err(invalid("missing or ambiguous net0 section"));
    }
    port.ok_or_else(|| invalid("missing SSH forwarding rule"))
}

#[derive(Debug)]
enum Json {
    Object(BTreeMap<String, Self>),
    Array(Vec<Self>),
    String(String),
    Number,
    Bool,
    Null,
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Parser<'a> {
    fn parse(bytes: &'a [u8]) -> io::Result<Json> {
        if bytes.len() > MAX_MESSAGE {
            return Err(invalid("JSON exceeds size limit"));
        }
        let mut parser = Self { bytes, index: 0 };
        let value = parser.value(0)?;
        parser.whitespace();
        if parser.index != bytes.len() {
            return Err(invalid("trailing data after JSON value"));
        }
        Ok(value)
    }

    fn whitespace(&mut self) {
        while self.bytes.get(self.index).is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
        {
            self.index += 1;
        }
    }

    fn take(&mut self, expected: u8) -> io::Result<()> {
        if self.bytes.get(self.index) != Some(&expected) {
            return Err(invalid("invalid JSON syntax"));
        }
        self.index += 1;
        Ok(())
    }

    fn value(&mut self, depth: usize) -> io::Result<Json> {
        if depth > MAX_DEPTH {
            return Err(invalid("JSON exceeds nesting limit"));
        }
        self.whitespace();
        match self.bytes.get(self.index).copied() {
            Some(b'{') => {
                self.index += 1;
                let mut fields = BTreeMap::new();
                self.whitespace();
                if self.bytes.get(self.index) != Some(&b'}') {
                    loop {
                        self.whitespace();
                        let key = self.string()?;
                        self.whitespace();
                        self.take(b':')?;
                        let value = self.value(depth + 1)?;
                        if fields.insert(key, value).is_some() {
                            return Err(invalid("duplicate JSON object field"));
                        }
                        self.whitespace();
                        if self.bytes.get(self.index) != Some(&b',') {
                            break;
                        }
                        self.index += 1;
                    }
                }
                self.take(b'}')?;
                Ok(Json::Object(fields))
            }
            Some(b'[') => {
                self.index += 1;
                let mut items = Vec::new();
                self.whitespace();
                if self.bytes.get(self.index) != Some(&b']') {
                    loop {
                        items.push(self.value(depth + 1)?);
                        self.whitespace();
                        if self.bytes.get(self.index) != Some(&b',') {
                            break;
                        }
                        self.index += 1;
                    }
                }
                self.take(b']')?;
                Ok(Json::Array(items))
            }
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal(b"true", Json::Bool),
            Some(b'f') => self.literal(b"false", Json::Bool),
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(invalid("invalid JSON value")),
        }
    }

    fn literal(&mut self, expected: &[u8], value: Json) -> io::Result<Json> {
        if self.bytes.get(self.index..self.index + expected.len()) != Some(expected) {
            return Err(invalid("invalid JSON literal"));
        }
        self.index += expected.len();
        Ok(value)
    }

    fn digits(&mut self) -> io::Result<()> {
        let start = self.index;
        while self.bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
            self.index += 1;
        }
        if self.index == start { Err(invalid("missing JSON number digits")) } else { Ok(()) }
    }

    fn number(&mut self) -> io::Result<Json> {
        if self.bytes.get(self.index) == Some(&b'-') {
            self.index += 1;
        }
        if self.bytes.get(self.index) == Some(&b'0') {
            self.index += 1;
        } else {
            self.digits()?;
        }
        if self.bytes.get(self.index) == Some(&b'.') {
            self.index += 1;
            self.digits()?;
        }
        if self.bytes.get(self.index).is_some_and(|b| matches!(b, b'e' | b'E')) {
            self.index += 1;
            if self.bytes.get(self.index).is_some_and(|b| matches!(b, b'+' | b'-')) {
                self.index += 1;
            }
            self.digits()?;
        }
        Ok(Json::Number)
    }

    fn hex4(&mut self) -> io::Result<u32> {
        let mut number = 0;
        for _ in 0..4 {
            let byte = *self.bytes.get(self.index).ok_or_else(|| invalid("short JSON escape"))?;
            number = number * 16
                + char::from(byte)
                    .to_digit(16)
                    .ok_or_else(|| invalid("invalid JSON hex escape"))?;
            self.index += 1;
        }
        Ok(number)
    }

    fn string(&mut self) -> io::Result<String> {
        self.take(b'"')?;
        let mut result = Vec::new();
        loop {
            let byte =
                *self.bytes.get(self.index).ok_or_else(|| invalid("unterminated JSON string"))?;
            self.index += 1;
            match byte {
                b'"' => {
                    return String::from_utf8(result).map_err(|_| invalid("invalid JSON UTF-8"));
                }
                b'\\' => {
                    let escape =
                        *self.bytes.get(self.index).ok_or_else(|| invalid("short JSON escape"))?;
                    self.index += 1;
                    match escape {
                        b'"' | b'\\' | b'/' => result.push(escape),
                        b'b' => result.push(8),
                        b'f' => result.push(12),
                        b'n' => result.push(b'\n'),
                        b'r' => result.push(b'\r'),
                        b't' => result.push(b'\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let scalar = if (0xd800..=0xdbff).contains(&first) {
                                self.take(b'\\')?;
                                self.take(b'u')?;
                                let second = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err(invalid("invalid JSON surrogate pair"));
                                }
                                0x10000 + ((first - 0xd800) << 10) + second - 0xdc00
                            } else {
                                first
                            };
                            let character = char::from_u32(scalar)
                                .ok_or_else(|| invalid("invalid JSON Unicode scalar"))?;
                            result.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        0..=255 => return Err(invalid("unsupported JSON escape")),
                    }
                }
                0..=31 => return Err(invalid("unescaped JSON control character")),
                _ => result.push(byte),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORWARD: &str = "Hub -1 (net0):\r\n  Protocol[State] FD Source Address Port Dest. Address Port RecvQ SendQ\r\n  TCP[HOST_FORWARD] 9 127.0.0.1 41617 10.0.2.15 22 0 0\r\n";

    #[test]
    fn forwarding_requires_one_exact_owned_loopback_endpoint() {
        assert_eq!(forwarded_port(FORWARD).unwrap(), 41617);
        for bad in [
            FORWARD.replace("127.0.0.1", "0.0.0.0"),
            FORWARD.replace("41617", "0"),
            FORWARD.replace("41617", "65536"),
            FORWARD.replace("10.0.2.15", "10.0.2.16"),
            FORWARD.replace(" 22 ", " 23 "),
            FORWARD.replace("TCP[", "UDP["),
            FORWARD.replace("net0", "net1"),
            FORWARD.repeat(2),
            format!("{FORWARD} TCP[HOST_FORWARD] 10 127.0.0.1 40000 10.0.2.15 22 0 0\n"),
        ] {
            assert!(forwarded_port(&bad).is_err(), "accepted {bad:?}");
        }
        assert!(forwarded_port("").is_err());
    }

    #[test]
    fn json_handles_escapes_and_rejects_ambiguous_or_malformed_frames() {
        let Json::String(value) = Parser::parse(br#""a\r\n\t\"\\\u0041\ud83d\ude00""#).unwrap()
        else {
            panic!("expected string");
        };
        assert_eq!(value, "a\r\n\t\"\\A😀");
        for bad in [
            r#"{"return":{},"return":"fake"}"#,
            r#"{"return":{},}"#,
            r#"[1,]"#,
            r#"01"#,
            r#"1e"#,
            r#""\ud800""#,
            r#""\udc00""#,
            r#""\x41""#,
            r#"{}{}"#,
        ] {
            assert!(Parser::parse(bad.as_bytes()).is_err(), "accepted {bad}");
        }
        assert!(Parser::parse(&[b'"', 0xff, b'"']).is_err());
        assert!(Parser::parse("[".repeat(MAX_DEPTH + 2).as_bytes()).is_err());
        assert!(Parser::parse(&vec![b' '; MAX_MESSAGE + 1]).is_err());
    }

    #[test]
    fn qcodes_are_a_small_explicit_allowlist() {
        for valid in ["a", "z", "0", "9", "ret", "tab"] {
            assert!(validate_qcode(valid).is_ok());
        }
        for bad in ["", "A", "enter", "ctrl", "a\"", "a\n", "ab", "é"] {
            assert!(validate_qcode(bad).is_err());
        }
    }

    #[test]
    fn screenshot_names_are_fixed_private_run_file_components() {
        for valid in ["ryoku-ui-success.ppm", "ryoku-ui-error.ppm"] {
            assert!(validate_screenshot_filename(valid).is_ok());
        }
        for bad in [
            "",
            "../capture.ppm",
            "/tmp/capture.ppm",
            ".private.ppm",
            "two words.ppm",
            "capture.png",
        ] {
            assert!(validate_screenshot_filename(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn screendump_uses_qmp_monitor_and_only_the_fixed_run_filename() {
        let (client, mut server) = UnixStream::pair().unwrap();
        server.write_all(b"{\"id\":\"ryoku-qmp-1\",\"return\":\"\"}\n").unwrap();
        let receiver = thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(server).read_line(&mut request).unwrap();
            request
        });
        let mut client = QmpClient { reader: BufReader::new(client), next_id: 1 };
        client.screendump("ryoku-ui-success.ppm").unwrap();
        assert_eq!(
            receiver.join().unwrap(),
            r#"{"execute":"human-monitor-command","id":"ryoku-qmp-1", "arguments":{"command-line":"screendump ryoku-ui-success.ppm"}}"#
                .to_owned()
                + "\n"
        );
    }

    fn reply_client(reply: &[u8]) -> QmpClient {
        let (client, mut server) = UnixStream::pair().unwrap();
        server.write_all(reply).unwrap();
        // Keep the peer alive until the client sends its tiny request.
        thread::spawn(move || {
            let mut request = String::new();
            let _ = BufReader::new(server).read_line(&mut request);
        });
        QmpClient { reader: BufReader::new(client), next_id: 1 }
    }

    #[test]
    fn handshake_requires_greeting_and_matching_capability_reply() {
        let greeting = r#"{"QMP":{"version":{"qemu":{"major":10,"minor":0,"micro":13},"package":"native"},"capabilities":["oob"]}}"#;
        let frames = format!("{greeting}\n{{\"id\":\"ryoku-qmp-1\",\"return\":{{}}}}\n");
        let stream = reply_client(frames.as_bytes()).reader.into_inner();
        assert!(QmpClient::handshake(stream, Instant::now() + REQUEST_TIMEOUT).is_ok());
        for frames in [
            "{\"nested\":{\"QMP\":{}}}\n".to_string(),
            "{\"QMP\":{\"version\":{},\"capabilities\":[1]}}\n".to_string(),
            format!("{greeting}\n{{\"id\":\"wrong\",\"return\":{{}}}}\n"),
            format!("{greeting}\n{{\"id\":\"ryoku-qmp-1\",\"error\":{{}}}}\n"),
            format!("{greeting}\n{{\"id\":\"ryoku-qmp-1\",\"return\":\"fake\"}}\n"),
        ] {
            let stream = reply_client(frames.as_bytes()).reader.into_inner();
            assert!(QmpClient::handshake(stream, Instant::now() + REQUEST_TIMEOUT).is_err());
        }
    }

    #[test]
    fn reply_ids_events_and_top_level_fields_are_checked() {
        let valid = b"{\"event\":\"STOP\"}\n{\"id\":\"ryoku-qmp-1\",\"return\":{}}\n";
        expect_empty(
            reply_client(valid).request("test", None, Instant::now() + REQUEST_TIMEOUT).unwrap(),
        )
        .unwrap();
        for reply in [
            "{\"id\":\"wrong\",\"return\":{}}\n",
            "{\"id\":\"ryoku-qmp-1\",\"nested\":{\"return\":{}}}\n",
            "{\"id\":\"ryoku-qmp-1\",\"error\":{\"desc\":\"failed\"}}\n",
            "{\"id\":\"ryoku-qmp-1\",\"event\":\"STOP\",\"return\":{}}\n",
            "{\"return\":{},\"id\":\"ryoku-qmp-1\",\"id\":\"wrong\"}\n",
        ] {
            assert!(
                reply_client(reply.as_bytes())
                    .request("test", None, Instant::now() + REQUEST_TIMEOUT)
                    .is_err()
            );
        }
        let flood = "{\"event\":\"STOP\"}\n".repeat(MAX_EVENTS + 1);
        assert!(
            reply_client(flood.as_bytes())
                .request("test", None, Instant::now() + REQUEST_TIMEOUT)
                .is_err()
        );
    }

    #[test]
    fn frames_fail_on_eof_deadline_and_oversize() {
        let (client, server) = UnixStream::pair().unwrap();
        drop(server);
        let mut client = QmpClient { reader: BufReader::new(client), next_id: 1 };
        assert_eq!(
            client.read_message(Instant::now() + REQUEST_TIMEOUT).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            client.read_message(Instant::now()).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        let (stream, peer) = UnixStream::pair().unwrap();
        let mut waiting = QmpClient { reader: BufReader::new(stream), next_id: 1 };
        let error = waiting.read_message(Instant::now() + Duration::from_millis(20)).unwrap_err();
        assert!(matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock));
        drop(peer);
        let (client, mut server) = UnixStream::pair().unwrap();
        let writer = thread::spawn(move || {
            let _ = server.write_all(&vec![b' '; MAX_MESSAGE + 1]);
        });
        let mut client = QmpClient { reader: BufReader::new(client), next_id: 1 };
        assert!(client.read_message(Instant::now() + REQUEST_TIMEOUT).is_err());
        drop(client);
        writer.join().unwrap();
    }
}
