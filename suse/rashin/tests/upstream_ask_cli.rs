//! End-to-end proof with the pinned upstream CLI and the private Ask patch.
use ryoku_rashin::{
    api::{Lane, Request, Response},
    provider::{FixtureOutcome, Provider},
};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    daemon: Child,
    cli: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let cli = PathBuf::from(
            std::env::var_os("RASHIN_UPSTREAM_ASK_BINARY").expect("patched pinned upstream binary"),
        );
        let root = std::env::temp_dir().join(format!(
            "rashin-pinned-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let daemon = Self::start(&root);
        Self { root, daemon, cli }
    }
    fn start(root: &std::path::Path) -> Child {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ryoku-rashin"))
            .arg("serve")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(root.join("upstream-ask.sock")).is_err()
            || UnixStream::connect(root.join("rashin.sock")).is_err()
        {
            assert!(child.try_wait().unwrap().is_none());
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        child
    }
    fn restart(&mut self) {
        self.daemon.kill().unwrap();
        self.daemon.wait().unwrap();
        // A stale socket is intentionally left for the daemon's guarded restart.
        self.daemon = Self::start(&self.root);
        let until = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(self.root.join("rashin.sock")).is_err() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn request(&self, request: Request) -> Value {
        let mut stream = UnixStream::connect(self.root.join("rashin.sock")).unwrap();
        let bytes = serde_json::to_vec(&request).unwrap();
        stream.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
        stream.write_all(&bytes).unwrap();
        let mut length = [0; 4];
        stream.read_exact(&mut length).unwrap();
        let mut body = vec![0; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).unwrap();
        match serde_json::from_slice::<Response>(&body).unwrap() {
            Response::Ok { data } => data,
            Response::Error { error } => panic!("{error:?}"),
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.cli);
        cmd.env_clear()
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", &self.root)
            .env("XDG_STATE_HOME", &self.root)
            .env("RYOKU_RASHIN_ASK_SOCKET", self.root.join("upstream-ask.sock"));
        cmd
    }
}

#[test]
#[ignore = "requires pinned upstream Go executable with private Ask patch"]
fn pinned_cli_cancel_stops_fixture_ask_without_persisting_an_answer() {
    let f = Fixture::new();
    f.request(Request::PutProvider {
        id: "fixture".into(),
        provider: Provider::Fixture {
            delay_millis: 1000,
            outcome: FixtureOutcome::Answer("must not persist".into()),
            catalog: None,
        },
    });
    f.request(Request::SetRoute {
        lane: Lane::Ask,
        provider: "fixture".into(),
        model: "fixture-v1".into(),
    });
    let mut ask = f.command().args(["ask", "cancel this"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut output = BufReader::new(ask.stdout.take().unwrap());
    let mut first = String::new();
    output.read_line(&mut first).unwrap();
    assert!(first.starts_with("@working "));
    assert!(f.command().args(["ask", "--cancel"]).status().unwrap().success());
    let mut rest = String::new();
    output.read_to_string(&mut rest).unwrap();
    assert!(ask.wait().unwrap().success());
    assert!(rest.contains("@error "));
    assert!(!rest.contains("must not persist"));
    let recent = f.command().args(["ask", "--recent"]).output().unwrap();
    assert!(recent.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&recent.stdout).unwrap().as_array().unwrap().len(),
        0
    );
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
#[ignore = "requires pinned upstream Go executable with private Ask patch"]
fn pinned_cli_asks_and_recovers_private_history_after_daemon_restart() {
    let mut f = Fixture::new();
    f.request(Request::PutProvider {
        id: "fixture".into(),
        provider: Provider::Fixture {
            delay_millis: 0,
            outcome: FixtureOutcome::Answer("The fixture answer is #abc123".into()),
            catalog: None,
        },
    });
    f.request(Request::SetRoute {
        lane: Lane::Ask,
        provider: "fixture".into(),
        model: "fixture-v1".into(),
    });
    let output = f.command().args(["ask", "What happened?"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let answer: Value = serde_json::from_str(
        stdout.lines().find_map(|s| s.strip_prefix("@answer ")).expect("answer marker"),
    )
    .unwrap();
    assert_eq!(answer["text"], "The fixture answer is #abc123");
    assert_eq!(answer["actions"][0]["kind"], "color");
    let second = f.command().args(["ask", "What next?"]).output().unwrap();
    assert!(second.status.success());
    f.restart();
    let output = f.command().args(["ask", "--recent"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let records: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records[0]["q"], "What next?");
    assert_eq!(records[0]["a"], "The fixture answer is #abc123");
    assert_eq!(records[0]["actions"][0]["kind"], "color");
    assert_eq!(records[1]["q"], "What happened?");
}

#[test]
fn foreign_server_fixture() {
    let Some(path) = std::env::var_os("RASHIN_FOREIGN_ASK_SOCKET") else { return };
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    println!("ready");
    std::io::stdout().flush().unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut byte = [0];
    assert_eq!(stream.read(&mut byte).unwrap(), 0, "question sent to foreign peer");
}

#[test]
#[ignore = "requires pinned patched CLI and namespace mapping owner UID 0 and foreign UID 1"]
fn pinned_cli_rejects_foreign_socket_before_sending_question() {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let root = std::env::temp_dir().join(format!("rashin-foreign-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    let socket = root.join("fake.sock");
    let executable = fs::File::open(std::env::current_exe().unwrap()).unwrap();
    let mut fake = Command::new(format!("/proc/self/fd/{}", executable.as_raw_fd()))
        .uid(1)
        .gid(1)
        .env("RASHIN_FOREIGN_ASK_SOCKET", &socket)
        .args(["--exact", "foreign_server_fixture", "--nocapture"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(fake.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("ready") {
        line.clear();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
    }
    let cli = std::env::var_os("RASHIN_UPSTREAM_ASK_BINARY").unwrap();
    let output = Command::new(cli)
        .env_clear()
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", &root)
        .env("RYOKU_RASHIN_ASK_SOCKET", &socket)
        .args(["ask", "private question"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(fake.wait().unwrap().success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn foreign_client_fixture() {
    let Some(path) = std::env::var_os("RASHIN_FOREIGN_ASK_CLIENT") else { return };
    let mut stream = UnixStream::connect(path).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = [0];
    assert_eq!(stream.read(&mut b).unwrap(), 0);
}

#[test]
#[ignore = "requires namespace mapping owner UID 0 and foreign UID 1"]
fn private_ask_daemon_rejects_foreign_client_before_request() {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let f = Fixture::new();
    fs::set_permissions(&f.root, fs::Permissions::from_mode(0o711)).unwrap();
    fs::set_permissions(f.root.join("upstream-ask.sock"), fs::Permissions::from_mode(0o777))
        .unwrap();
    let executable = fs::File::open(std::env::current_exe().unwrap()).unwrap();
    let status = Command::new(format!("/proc/self/fd/{}", executable.as_raw_fd()))
        .uid(1)
        .gid(1)
        .env("RASHIN_FOREIGN_ASK_CLIENT", f.root.join("upstream-ask.sock"))
        .args(["--exact", "foreign_client_fixture", "--nocapture"])
        .status()
        .unwrap();
    assert!(status.success());
}
