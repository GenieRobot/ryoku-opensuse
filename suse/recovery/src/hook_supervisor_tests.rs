use super::*;
use crate::{
    AccountPrincipal,
    hook_protocol::{HookApprovalRequest, HookOutput, HookPolicy, HookRequest},
};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
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
                    execution: crate::hook_protocol::HookExecutionPolicy::DirectWriteOnly,
                    templates: vec![],
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
    fn staged(&mut self) {
        self.call.policy.approval.execution =
            crate::hook_protocol::HookExecutionPolicy::StagedOutputs;
    }
    fn add_fish(&mut self) {
        std::fs::create_dir_all(format!("{}/.config/fish", self.home)).unwrap();
        std::fs::write(format!("{}/.config/fish/user.fish", self.home), b"fish before").unwrap();
        std::fs::set_permissions(
            format!("{}/.config/fish/user.fish", self.home),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        self.call.policy.approval.outputs.push(HookOutput::FishUser);
        self.call.output_identities = crate::hook_protocol::output_identities(
            &File::open(&self.home).unwrap(),
            &self.call.policy.approval.outputs,
        )
        .unwrap();
    }
    fn add_template(&mut self) -> String {
        let relative = ".config/matugen/templates/example.conf";
        let path = format!("{}/{relative}", self.home);
        std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, b"template before\n").unwrap();
        self.call.policy.approval.templates.push(crate::hook_protocol::HookTemplateInput {
            path: crate::RelativePath::new(relative).unwrap(),
            sha256: crate::Sha256::new(digest(b"template before\n")).unwrap(),
        });
        path
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
fn staged_output_preserves_literal_home_and_commits_only_the_declared_file() {
    let mut fixture = Fixture::new("printf '%s' \"$HOME\" > \"$HOME/.config/kitty/user.conf\"");
    fixture.call.policy.approval.execution =
        crate::hook_protocol::HookExecutionPolicy::StagedOutputs;
    run(&fixture.call, &fixture.home, || false).unwrap();
    assert_eq!(fixture.output(), fixture.home.as_bytes());
    assert_eq!(
        std::fs::metadata(format!("{}/.config/kitty/user.conf", fixture.home)).unwrap().mode()
            & 0o777,
        0o600
    );
    assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
}
#[test]
fn staged_unknown_writes_and_parent_namespace_escape_commit_nothing() {
    for command in [
        "printf changed > .config/kitty/user.conf; printf rogue > sentinel",
        "printf rogue > /proc/$PPID/root$HOME/sentinel",
    ] {
        let mut fixture = Fixture::new(command);
        fixture.call.policy.approval.execution =
            crate::hook_protocol::HookExecutionPolicy::StagedOutputs;
        assert!(run(&fixture.call, &fixture.home, || false).is_err());
        assert_eq!(fixture.output(), b"before");
        assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
    }
}
#[test]
fn staged_data_cannot_become_executable_through_the_elf_loader() {
    let mut fixture = Fixture::new("exec /lib64/ld-linux-x86-64.so.2 .config/kitty/user.conf");
    let binary = std::fs::read("/usr/bin/true").unwrap();
    std::fs::write(format!("{}/.config/kitty/user.conf", fixture.home), &binary).unwrap();
    fixture.call.policy.approval.execution =
        crate::hook_protocol::HookExecutionPolicy::StagedOutputs;
    assert!(run(&fixture.call, &fixture.home, || false).is_err());
    assert_eq!(fixture.output(), binary);
}
#[test]
fn staged_root_pinned_bash_exec_preserves_home_cwd_uid_and_two_outputs() {
    if let Ok(expected_profile) = std::env::var("RYOKU_CI_APPARMOR_PROFILE") {
        let actual = std::fs::read_to_string("/proc/self/attr/current").unwrap();
        eprintln!("exact staged integration test executable AppArmor label: {actual}");
        assert_eq!(actual.trim(), format!("{expected_profile} (unconfined)"));
    }
    let mut fixture = Fixture::new(
        "exec /usr/bin/bash --noprofile --norc -c 'IFS= read -r previous < .config/kitty/user.conf || :; printf \"%s:%s:%s:%s:%s:%s\" \"$HOME\" \"$PWD\" \"$UID\" \"$EUID\" \"${GROUPS[0]}\" \"$previous\" > .config/kitty/user.conf; printf fish-after > .config/fish/user.fish'",
    );
    fixture.staged();
    fixture.add_fish();
    run(&fixture.call, &fixture.home, || false).unwrap();
    assert_eq!(
        fixture.output(),
        format!(
            "{}:{}:{}:{}:{}:before",
            fixture.home,
            fixture.home,
            fixture.call.policy.uid,
            fixture.call.policy.uid,
            rustix::process::getegid().as_raw()
        )
        .as_bytes()
    );
    let fish = format!("{}/.config/fish/user.fish", fixture.home);
    assert_eq!(std::fs::read(&fish).unwrap(), b"fish-after");
    let metadata = std::fs::metadata(&fish).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o640);
    assert_eq!(metadata.uid(), fixture.call.policy.uid);
    assert_eq!(metadata.gid(), rustix::process::getegid().as_raw());
    assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
}
#[test]
fn staged_changed_preimage_refuses_publication_without_clobbering_the_change() {
    for replace in [false, true] {
        let mut fixture = Fixture::new(
            "printf after > .config/kitty/user.conf; for ((i=0;i<20000;i++)); do :; done",
        );
        fixture.staged();
        let mut calls = 0;
        let result = run(&fixture.call, &fixture.home, || {
            calls += 1;
            if calls == 2 {
                let output = format!("{}/.config/kitty/user.conf", fixture.home);
                if replace {
                    let replacement = format!("{}/replacement", fixture.home);
                    std::fs::write(&replacement, b"concurrent").unwrap();
                    std::fs::rename(replacement, &output).unwrap();
                } else {
                    std::fs::write(output, b"concurrent").unwrap();
                }
            }
            false
        });
        assert!(result.unwrap_err().to_string().contains("during staging"));
        assert!(calls >= 2);
        assert_eq!(fixture.output(), b"concurrent");
    }
}
#[test]
fn staged_exact_template_input_is_readable_and_preserved_without_sibling_exposure() {
    let mut fixture = Fixture::new(
        "IFS= read -r text < .config/matugen/templates/example.conf; printf '%s' \"$text\" > .config/kitty/user.conf",
    );
    fixture.staged();
    let template = fixture.add_template();
    let sibling = Path::new(&template).with_file_name("unapproved.conf");
    std::fs::write(&sibling, b"unapproved sibling").unwrap();
    let metadata = std::fs::metadata(&template).unwrap();
    run(&fixture.call, &fixture.home, || false).unwrap();
    assert_eq!(fixture.output(), b"template before");
    assert_eq!(std::fs::read(&template).unwrap(), b"template before\n");
    assert_eq!(std::fs::metadata(&template).unwrap().ino(), metadata.ino());
    assert_eq!(std::fs::read(sibling).unwrap(), b"unapproved sibling");
}
#[test]
fn staged_template_modified_digest_links_escape_oversize_or_write_refuse() {
    for case in 0..8 {
        let command = if case == 5 {
            "printf changed > .config/matugen/templates/example.conf"
        } else if case == 6 {
            "IFS= read -r text < .config/matugen/templates/unapproved.conf"
        } else {
            "printf changed > .config/kitty/user.conf"
        };
        let mut fixture = Fixture::new(command);
        fixture.staged();
        let template = fixture.add_template();
        match case {
            0 => std::fs::write(&template, b"modified").unwrap(),
            1 => {
                std::fs::remove_file(&template).unwrap();
                symlink(format!("{}/sentinel", fixture.home), &template).unwrap();
            }
            2 => {
                std::fs::remove_file(&template).unwrap();
                std::fs::hard_link(format!("{}/sentinel", fixture.home), &template).unwrap();
            }
            3 => std::fs::write(&template, vec![b'x'; 65537]).unwrap(),
            4 => {
                fixture.call.policy.approval.templates[0].path =
                    crate::RelativePath::new("sentinel").unwrap()
            }
            6 => {
                std::fs::write(
                    Path::new(&template).with_file_name("unapproved.conf"),
                    b"unapproved",
                )
                .unwrap();
            }
            7 => {
                fixture.call.policy.approval.execution =
                    crate::hook_protocol::HookExecutionPolicy::DirectWriteOnly
            }
            _ => {}
        }
        assert!(run(&fixture.call, &fixture.home, || false).is_err(), "case {case}");
        assert_eq!(fixture.output(), b"before");
        assert_eq!(std::fs::read(format!("{}/sentinel", fixture.home)).unwrap(), b"unrelated");
        if case == 5 {
            assert_eq!(std::fs::read(&template).unwrap(), b"template before\n");
        }
    }
}
#[test]
fn staged_template_change_during_computation_refuses_output_publication() {
    for replace in [false, true] {
        let mut fixture = Fixture::new(
            "printf changed > .config/kitty/user.conf; for ((i=0;i<20000;i++)); do :; done",
        );
        fixture.staged();
        let template = fixture.add_template();
        let mut ticks = 0;
        let error = run(&fixture.call, &fixture.home, || {
            ticks += 1;
            if ticks == 2 {
                if replace {
                    let replacement = format!("{}/replacement", fixture.home);
                    std::fs::write(&replacement, b"template before\n").unwrap();
                    std::fs::rename(replacement, &template).unwrap();
                } else {
                    std::fs::write(&template, b"concurrent template").unwrap();
                }
            }
            false
        })
        .unwrap_err();
        assert!(
            error.to_string().contains(if replace {
                "template input was replaced"
            } else {
                "template input changed"
            }),
            "{error}"
        );
        assert_eq!(fixture.output(), b"before");
        assert_eq!(
            std::fs::read(&template).unwrap(),
            if replace {
                b"template before\n".as_slice()
            } else {
                b"concurrent template".as_slice()
            }
        );
    }
}
#[test]
fn staged_cancel_deadline_and_byte_limit_never_publish_partial_outputs() {
    for (command, cancel) in [
        ("printf partial > .config/kitty/user.conf; while :; do :; done", true),
        ("printf partial > .config/kitty/user.conf; while :; do :; done", false),
        ("printf '%17825792s' x > .config/kitty/user.conf", false),
    ] {
        let mut fixture = Fixture::new(command);
        fixture.staged();
        let started = Instant::now();
        let mut calls = 0;
        assert!(
            run(&fixture.call, &fixture.home, || {
                calls += 1;
                cancel && calls > 5
            })
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(fixture.output(), b"before");
    }
}
#[test]
fn staged_inode_quota_stops_creation_and_undeclared_names_prevent_commit() {
    let mut fixture = Fixture::new(
        "i=0; while ((i<300)); do if ! printf x > \"extra-$i\"; then exit 0; fi; ((i+=1)); done; exit 23",
    );
    fixture.staged();
    let error = run(&fixture.call, &fixture.home, || false).unwrap_err();
    // The command succeeds only when creation fails before 300 inodes. A
    // generic process failure would not establish the tmpfs quota was reached.
    assert!(error.to_string().contains("undeclared staged entries"), "{error}");
    assert_eq!(fixture.output(), b"before");
    assert!(!Path::new(&format!("{}/extra-0", fixture.home)).exists());
}
#[test]
fn staged_namespace_denial_reports_the_exact_syscall_and_errno_without_launch() {
    const TEST: &str = "hook_supervisor::tests::staged_namespace_denial_reports_the_exact_syscall_and_errno_without_launch";
    const ROLE: &str = "RYOKU_HOOK_NAMESPACE_DENIAL";
    if let Ok(operation) = std::env::var(ROLE) {
        let mut fixture = Fixture::new("printf should-never-run > .config/kitty/user.conf");
        fixture.staged();
        let error = run(&fixture.call, &fixture.home, || false).unwrap_err();
        eprintln!("isolated namespace policy denial: {error}");
        let expected = match operation.as_str() {
            "user" => "unshare(CLONE_NEWUSER)",
            "mount-net" => "unshare(CLONE_NEWNS|CLONE_NEWNET)",
            _ => panic!("unknown isolated denial"),
        };
        assert!(error.to_string().contains(expected), "{error}");
        assert!(error.to_string().contains("errno=13"), "{error}");
        assert_eq!(fixture.output(), b"before");
        return;
    }
    for (name, flags) in
        [("user", libc::CLONE_NEWUSER), ("mount-net", libc::CLONE_NEWNS | libc::CLONE_NEWNET)]
    {
        let statement = |code, k| libc::sock_filter { code, jt: 0, jf: 0, k };
        let jump = |k, jt, jf| libc::sock_filter { code: 0x15, k, jt, jf };
        let filter = [
            statement(0x20, 0),
            jump(libc::SYS_unshare as u32, 0, 3),
            statement(0x20, 16),
            jump(flags as u32, 0, 1),
            statement(0x06, 0x0005_0000 | libc::EACCES as u32),
            statement(0x06, 0x7fff_0000),
        ];
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", TEST, "--nocapture"]).env(ROLE, name);
        // SAFETY: this process-scoped kernel denial is installed only in an
        // isolated fixture subprocess. Production sandbox policy is unchanged.
        unsafe {
            child.pre_exec(move || {
                let program = libc::sock_fprog {
                    len: filter.len() as u16,
                    filter: filter.as_ptr().cast_mut(),
                };
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                    || libc::prctl(libc::PR_SET_SECCOMP, 2, &program) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        assert!(child.status().unwrap().success());
    }
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
    let mut staged = Fixture::new("printf staged > .config/kitty/user.conf");
    staged.staged();
    run(&staged.call, &staged.home, || false).unwrap();
    freeze.check().unwrap();
    assert_eq!(before, std::fs::read(&marker).unwrap());
    assert_eq!(staged.output(), b"staged");
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
