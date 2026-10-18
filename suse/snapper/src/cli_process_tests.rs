use super::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const CHILD_TEST: &str = "cli::process_tests::rust_child_fixture";
const CHILD_MODE: &str = "RYOKU_SNAPPER_PROCESS_FIXTURE";
const CHILD_PATH: &str = "RYOKU_SNAPPER_PROCESS_PATH";
fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear().env(CHILD_MODE, mode).args([
        "--exact",
        CHILD_TEST,
        "--nocapture",
        "--quiet",
    ]);
    command
}

#[test]
fn rust_child_fixture() {
    let Ok(mode) = std::env::var(CHILD_MODE) else { return };
    match mode.as_str() {
        "success" => println!("bounded-success-marker"),
        "malformed" => println!("{{broken-json"),
        "failure" => std::process::exit(7),
        "stdout-overflow" => {
            std::io::stdout().write_all(&vec![b'x'; MAX_STDOUT_BYTES + 8192]).unwrap();
        }
        "stderr-overflow" => {
            std::io::stderr().write_all(&vec![b'e'; MAX_STDERR_BYTES * 2]).unwrap();
            println!("bounded-success-marker");
        }
        "both-pipes" => {
            let stderr = thread::spawn(|| {
                std::io::stderr().write_all(&vec![b'e'; MAX_STDERR_BYTES * 2]).unwrap();
            });
            std::io::stdout().write_all(&vec![b'x'; 128 * 1024]).unwrap();
            stderr.join().unwrap();
        }
        "continuous" => loop {
            if std::io::stderr().write_all(&[b'e'; 8192]).is_err() {
                break;
            }
        },
        "sleep" => thread::sleep(Duration::from_secs(5)),
        "parent-stdout" | "parent-stderr" => {
            let path = PathBuf::from(std::env::var_os(CHILD_PATH).unwrap());
            let mut descendant = fixture_command("hold-pipe");
            descendant
                .env(CHILD_PATH, &path)
                .stdin(Stdio::null())
                .stdout(if mode == "parent-stdout" { Stdio::inherit() } else { Stdio::null() })
                .stderr(if mode == "parent-stderr" { Stdio::inherit() } else { Stdio::null() });
            // Deliberately inherit the process group and one pipe. Keep the
            // parent alive until the descendant confirms readiness, then exit
            // without waiting: the parent's exit must not end supervision.
            let _child = descendant.spawn().unwrap();
            let until = Instant::now() + Duration::from_secs(2);
            while !path.join("descendant").exists() {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(2));
            }
            fs::write(path.join("parent"), std::process::id().to_string()).unwrap();
            std::process::exit(0);
        }
        "hold-pipe" => {
            let path = PathBuf::from(std::env::var_os(CHILD_PATH).unwrap());
            fs::write(
                path.join("descendant"),
                format!("{} {}", std::process::id(), rustix::process::getpgrp().as_raw_pid()),
            )
            .unwrap();
            thread::sleep(Duration::from_secs(5));
            fs::write(path.join("survived-deadline"), b"unexpected").unwrap();
        }
        _ => panic!("unexpected private fixture mode"),
    }
}

#[test]
fn native_program_arguments_and_environment_remain_fixed() {
    let args = snapper_list_args(&ConfigName::new("root").unwrap());
    let command = native_command(&args);
    assert_eq!(command.get_program(), SNAPPER);
    assert_eq!(command.get_args().collect::<Vec<_>>(), ["--jsonout", "--config", "root", "list"]);
    assert_eq!(
        command.get_envs().collect::<Vec<_>>(),
        [
            (std::ffi::OsStr::new("LC_ALL"), Some(std::ffi::OsStr::new("C"))),
            (std::ffi::OsStr::new("PATH"), Some(std::ffi::OsStr::new("/usr/sbin:/usr/bin"))),
        ]
    );
}

#[test]
fn finite_success_failure_overflow_and_parser_semantics_are_preserved() {
    let output = run_bounded(fixture_command("success"), Duration::from_secs(3)).unwrap();
    assert!(String::from_utf8(output).unwrap().contains("bounded-success-marker"));
    assert_eq!(
        run_bounded(fixture_command("failure"), Duration::from_secs(3)),
        Err(SnapperCliError::CommandFailed)
    );
    assert_eq!(
        run_bounded(fixture_command("stdout-overflow"), Duration::from_secs(3)),
        Err(SnapperCliError::OutputTooLarge)
    );
    let output = run_bounded(fixture_command("stderr-overflow"), Duration::from_secs(3)).unwrap();
    assert!(String::from_utf8(output).unwrap().contains("bounded-success-marker"));
    let output = run_bounded(fixture_command("both-pipes"), Duration::from_secs(3)).unwrap();
    assert!(output.len() >= 128 * 1024 && output.len() <= MAX_STDOUT_BYTES);
    let output = run_bounded(fixture_command("malformed"), Duration::from_secs(3)).unwrap();
    assert_eq!(
        parse_snapper_json(&ConfigName::new("root").unwrap(), &output),
        Err(SnapperCliError::InvalidJson)
    );
}

#[test]
fn silent_or_continuously_writable_children_cannot_starve_the_deadline() {
    for mode in ["sleep", "continuous"] {
        let start = Instant::now();
        assert_eq!(
            run_bounded(fixture_command(mode), Duration::from_millis(150)),
            Err(SnapperCliError::TimedOut)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

struct PrivateFixture(PathBuf);
impl PrivateFixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-snapper-pipes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for PrivateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn process_is_running(pid: u32) -> bool {
    let path = format!("/proc/{pid}/stat");
    match fs::read_to_string(path) {
        Ok(text) => {
            // A killed orphan can remain a zombie until PID 1 reaps it. It is
            // dead and holds no pipe; do not demand a particular init policy.
            let state = text.rsplit_once(") ").unwrap().1.as_bytes()[0];
            !matches!(state, b'Z' | b'X')
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => panic!("cannot inspect own fixture child: {error}"),
    }
}
fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path).unwrap().parse().unwrap()
}

#[test]
fn descendant_held_stdout_or_stderr_is_timed_out_and_owned_group_is_killed() {
    for mode in ["parent-stdout", "parent-stderr"] {
        let fixture = PrivateFixture::new();
        let mut command = fixture_command(mode);
        command.env(CHILD_PATH, &fixture.0);
        let start = Instant::now();
        assert_eq!(
            run_bounded(command, Duration::from_millis(700)),
            Err(SnapperCliError::TimedOut)
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        let parent = read_pid(&fixture.0.join("parent"));
        let descendant = fs::read_to_string(fixture.0.join("descendant")).unwrap();
        let ids: Vec<u32> =
            descendant.split_whitespace().map(|value| value.parse().unwrap()).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], parent);
        assert_eq!(ids[1], parent, "descendant must inherit the adapter-owned group");
        assert!(!Path::new(&format!("/proc/{parent}")).exists(), "direct child must be reaped");
        let until = Instant::now() + Duration::from_secs(1);
        while process_is_running(ids[0]) && Instant::now() < until {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !process_is_running(ids[0]),
            "owned descendant must be killed, not merely have its pipe dropped"
        );
        assert!(!fixture.0.join("survived-deadline").exists());
    }
}
