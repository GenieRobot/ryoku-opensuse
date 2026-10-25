use super::*;
use crate::{
    AccountPrincipal,
    hook_protocol::{HookApprovalRequest, HookOutput, HookPolicy, HookRequest},
};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    home: String,
    call: HookCall,
}
impl Fixture {
    fn new(command: &str) -> Self {
        let home = format!(
            "/tmp/ryoku-hook-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        std::fs::create_dir_all(format!("{home}/.config/ryoku")).unwrap();
        std::fs::create_dir_all(format!("{home}/.config/kitty")).unwrap();
        let config = serde_json::to_vec(&serde_json::json!({"postProcessing":[command]})).unwrap();
        std::fs::write(format!("{home}/.config/ryoku/ryogami.json"), &config).unwrap();
        std::fs::write(format!("{home}/.config/kitty/user.conf"), b"before").unwrap();
        std::fs::set_permissions(
            format!("{home}/.config/kitty/user.conf"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        std::fs::write(format!("{home}/sentinel"), b"unrelated").unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let call = HookCall {
            policy: HookPolicy {
                account: AccountPrincipal::from_account(
                    uid,
                    rustix::process::getegid().as_raw(),
                    "fixture",
                    &home,
                )
                .unwrap(),
                uid,
                approval: HookApprovalRequest {
                    index: 0,
                    command_sha256: digest(command.as_bytes()),
                    outputs: vec![HookOutput::KittyUser],
                    timeout_seconds: 1,
                    allow_restore: false,
                },
                configuration_sha256: digest(&config),
                shell_sha256: shell_digest().unwrap(),
                command: command.into(),
                kind: "all".into(),
            },
            request: HookRequest {
                index: 0,
                command_sha256: digest(command.as_bytes()),
                restoring: false,
                path: "/wall/quote'%path%.png".into(),
                kind: "static".into(),
            },
            home_sha256: digest(home.as_bytes()),
            home_device: std::fs::metadata(&home).unwrap().dev(),
            home_inode: std::fs::metadata(&home).unwrap().ino(),
            output_identities: crate::hook_protocol::output_identities(
                &File::open(&home).unwrap(),
                &[HookOutput::KittyUser],
            )
            .unwrap(),
            linked_plan_sha256: "ab".repeat(32),
            thumb: String::new(),
        };
        Self { home, call }
    }
    fn output(&self) -> Vec<u8> {
        std::fs::read(format!("{}/.config/kitty/user.conf", self.home)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
#[test]
fn approved_builtin_writes_only_exact_existing_output_with_literal_placeholders() {
    let fixture = Fixture::new("printf '%s' %path% > .config/kitty/user.conf");
    run(&fixture.call, &fixture.home, || false).unwrap();
    assert_eq!(fixture.output(), fixture.call.request.path.as_bytes());
    assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
}
#[test]
fn write_escape_new_file_exec_child_and_network_cannot_mutate_unrelated_state() {
    for command in [
        "printf escaped > sentinel",
        "printf escaped > new-file",
        "exec /usr/bin/true",
        "exec /lib64/ld-linux-x86-64.so.2 /usr/bin/true",
        "read value < .config/kitty/user.conf",
        "(printf escaped > sentinel)",
        "printf escaped > /dev/tcp/127.0.0.1/9",
    ] {
        let fixture = Fixture::new(command);
        assert!(run(&fixture.call, &fixture.home, || false).is_err(), "{command}");
        assert_eq!(fixture.output(), b"before");
        assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
        assert!(!std::path::Path::new(&format!("{}/new-file", fixture.home)).exists());
    }
}
#[test]
fn changed_account_command_configuration_shell_or_restore_policy_never_launches() {
    for field in 0..8 {
        let mut fixture = Fixture::new("printf changed > .config/kitty/user.conf");
        match field {
            0 => fixture.call.policy.uid += 1,
            1 => fixture.call.policy.command.push_str("; true"),
            2 => std::fs::write(format!("{}/.config/ryoku/ryogami.json", fixture.home), b"{}")
                .unwrap(),
            3 => fixture.call.policy.shell_sha256 = "00".repeat(32),
            4 => fixture.call.request.restoring = true,
            5 => fixture.call.request.index = 1,
            6 => fixture.call.home_inode += 1,
            _ => fixture.call.output_identities[0].inode += 1,
        }
        assert!(run(&fixture.call, &fixture.home, || false).is_err());
        assert_eq!(fixture.output(), b"before");
    }
}
#[test]
fn linked_or_hardlinked_outputs_are_refused_before_launch() {
    for hard in [false, true] {
        let fixture = Fixture::new("printf changed > .config/kitty/user.conf");
        let output = format!("{}/.config/kitty/user.conf", fixture.home);
        std::fs::remove_file(&output).unwrap();
        if hard {
            std::fs::hard_link(format!("{}/sentinel", fixture.home), &output).unwrap();
        } else {
            symlink(format!("{}/sentinel", fixture.home), &output).unwrap();
        }
        assert!(run(&fixture.call, &fixture.home, || false).is_err());
        assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
    }
}
#[test]
fn timeout_and_cancellation_reap_the_owned_process() {
    let fixture = Fixture::new("while :; do :; done");
    let start = Instant::now();
    assert!(run(&fixture.call, &fixture.home, || false).is_err());
    assert!(start.elapsed() < Duration::from_secs(3));
    let start = Instant::now();
    let mut ticks = 0;
    assert!(
        run(&fixture.call, &fixture.home, || {
            ticks += 1;
            ticks > 5
        })
        .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.output(), b"before");
}
#[test]
fn held_writers_stay_frozen_across_success_and_exec_refusal() {
    const TEST: &str =
        "hook_supervisor::tests::held_writers_stay_frozen_across_success_and_exec_refusal";
    const ROLE: &str = "RYOKU_HOOK_ISOLATED_ROLE";
    if let Ok(marker) = std::env::var("RYOKU_HOOK_COUNTER") {
        for tick in 1_u64..=u64::MAX {
            std::fs::write(&marker, tick.to_string()).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        unreachable!();
    }
    if std::env::var(ROLE).as_deref() != Ok("scenario") {
        let fixture = Fixture::new("printf approved > .config/kitty/user.conf");
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(ROLE, "scenario")
            .env("HOME", &fixture.home)
            .env("RYOKU_HOOK_CALL", serde_json::to_string(&fixture.call).unwrap())
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_STATE_HOME")
            .env_remove("XDG_CACHE_HOME")
            .status()
            .unwrap();
        assert!(result.success());
        return;
    }
    let fixture = Fixture {
        home: std::env::var("HOME").unwrap(),
        call: serde_json::from_str(&std::env::var("RYOKU_HOOK_CALL").unwrap()).unwrap(),
    };
    let marker = format!("{}/counter", fixture.home);
    let mut sibling = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env("RYOKU_HOOK_COUNTER", &marker)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::path::Path::new(&marker).exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let freeze =
        crate::writer_freeze::WriterFreeze::for_children(&fixture.home, vec![], vec![]).unwrap();
    let before = std::fs::read(&marker).unwrap();
    run(&fixture.call, &fixture.home, || false).unwrap();
    freeze.check().unwrap();
    assert_eq!(before, std::fs::read(&marker).unwrap());
    assert_eq!(fixture.output(), b"approved");
    let denied = Fixture::new("exec /usr/bin/true");
    assert!(run(&denied.call, &denied.home, || false).is_err());
    freeze.check().unwrap();
    assert_eq!(before, std::fs::read(&marker).unwrap());
    drop(freeze);
    let deadline = Instant::now() + Duration::from_secs(5);
    while before == std::fs::read(&marker).unwrap() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    sibling.kill().unwrap();
    sibling.wait().unwrap();
}
