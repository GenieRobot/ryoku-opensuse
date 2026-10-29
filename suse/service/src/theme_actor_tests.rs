use super::*;
use std::os::fd::FromRawFd;

#[test]
fn actual_kernel_nonroot_peer_never_constructs_a_settings_actor() {
    assert_ne!(
        rustix::process::geteuid().as_raw(),
        0,
        "run the ordinary workspace suite as its account user"
    );
    let (worker, _caller) = UnixStream::pair().unwrap();
    let error = serve_capture(worker, |_, _| panic!("unauthenticated actor factory was reached"))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("peer is not root"));
}

#[test]
fn extra_or_replayed_control_data_and_root_disconnect_fail_the_live_guard() {
    let (worker, mut root) = UnixStream::pair().unwrap();
    control_lifetime(&worker).unwrap();
    root.write_all(b"second request\n").unwrap();
    assert!(control_lifetime(&worker).unwrap_err().to_string().contains("replayed"));
    let (worker, root) = UnixStream::pair().unwrap();
    drop(root);
    assert!(control_lifetime(&worker).unwrap_err().to_string().contains("disconnected"));
}

#[test]
fn capture_wire_has_no_apply_or_restore_operation_and_rejects_oversized_input() {
    let (mut worker, mut root) = UnixStream::pair().unwrap();
    let sender = std::thread::spawn(move || root.write_all(&vec![b'x'; MAX_FRAME]));
    assert!(read_frame(&mut worker).unwrap_err().to_string().contains("exceeds bound"));
    sender.join().unwrap().unwrap();
    assert!(serde_json::from_str::<CaptureRequest>(r#"{"operation":"apply","keys":{}}"#).is_err());
}

#[test]
fn owned_worker_success_failure_timeout_and_cancellation_are_reaped() {
    const FIXTURE: &str =
        "theme_actor::tests::owned_worker_success_failure_timeout_and_cancellation_are_reaped";
    const ROLE: &str = "RYOKU_THEME_CAPTURE_FIXTURE";
    if let Ok(role) = std::env::var(ROLE) {
        if role == "confinement" {
            crate::theme_actor_sandbox::harden_worker().unwrap();
            let process = rustix::process::pidfd_open(
                rustix::process::getpid(),
                rustix::process::PidfdFlags::empty(),
            )
            .unwrap();
            let mut events = [rustix::event::PollFd::new(&process, rustix::event::PollFlags::IN)];
            assert_eq!(
                rustix::event::poll(&mut events, Some(&rustix::event::Timespec::default()))
                    .unwrap(),
                0
            );
            // SAFETY: fork/exec must be rejected in this isolated worker fixture.
            assert_eq!(unsafe { libc::fork() }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            let arguments = [c"true".as_ptr(), std::ptr::null()];
            assert_eq!(unsafe { libc::execv(c"/usr/bin/true".as_ptr(), arguments.as_ptr()) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
            assert!(std::net::TcpStream::connect("127.0.0.1:9").is_err());
            assert!(
                std::fs::write(std::env::var("RYOKU_THEME_WRITE_TARGET").unwrap(), b"escaped")
                    .is_err()
            );
            assert_eq!(std::thread::spawn(|| 42).join().unwrap(), 42);
        }
        // SAFETY: the isolated run_owned child explicitly inherits only this
        // control descriptor at3; this test owns it until process exit.
        let mut control = unsafe { UnixStream::from_raw_fd(3) };
        writeln!(control, "{}", std::process::id()).unwrap();
        match role.as_str() {
            "success" | "confinement" => return,
            "failure" => std::process::exit(23),
            "spin" => loop {
                std::hint::spin_loop();
            },
            _ => panic!("unknown fixture role"),
        }
    }
    for (role, cancel) in [
        ("success", false),
        ("confinement", false),
        ("failure", false),
        ("spin", false),
        ("spin", true),
    ] {
        let (worker, mut root) = UnixStream::pair().unwrap();
        root.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", FIXTURE, "--nocapture"]).env(ROLE, role);
        let write_target = std::env::temp_dir()
            .join(format!("ryoku-theme-readonly-{}-{role}-{cancel}", std::process::id()));
        std::fs::write(&write_target, b"before").unwrap();
        command.env("RYOKU_THEME_WRITE_TARGET", &write_target);
        let mut ticks = 0;
        let start = Instant::now();
        let result = run_owned(command, &worker, Duration::from_millis(200), &mut || {
            ticks += 1;
            cancel && ticks > 5
        });
        assert_eq!(result.is_ok(), matches!(role, "success" | "confinement"), "{role}: {result:?}");
        assert!(start.elapsed() < Duration::from_secs(2));
        let bytes = read_frame(&mut root).unwrap();
        let pid = std::str::from_utf8(&bytes).unwrap().trim().parse::<u32>().unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "owned worker remained alive"
        );
        assert_eq!(std::fs::read(&write_target).unwrap(), b"before");
        std::fs::remove_file(write_target).unwrap();
    }
}

/// Run explicitly by the hosted root harness after building this test binary.
/// This proves real root-peer transport to a nonroot worker, not native DConf
/// or desktop-session success; the injected backend never writes any setting.
#[test]
#[ignore = "requires a root parent that launches the worker as uid65534"]
fn root_peer_capture_binds_the_native_receipt_and_rejects_account_mismatch() {
    use crate::theme_settings::ThemeSettingState;
    use ryoku_recovery::{AccountPrincipal, wallpaper_protocol::ActorIdentity};
    const ROLE: &str = "RYOKU_THEME_ROOT_CAPTURE_CASE";
    const TEST: &str = "theme_actor::tests::root_peer_capture_binds_the_native_receipt_and_rejects_account_mismatch";
    struct ReadOnlyActor {
        binding: ThemeSettingsBinding,
    }
    impl ThemeSettingsActor for ReadOnlyActor {
        fn binding(&mut self) -> io::Result<ThemeSettingsBinding> {
            Ok(self.binding.clone())
        }
        fn check_lifetime(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn read(&mut self, _: ThemeSettingKey) -> io::Result<ThemeSettingState> {
            Ok(ThemeSettingState {
                user_value: None,
                effective_value: "default".into(),
                default_value: "default".into(),
                writable: true,
            })
        }
        fn validate_change(
            &mut self,
            _: ThemeSettingKey,
            _: &ThemeSettingChange,
        ) -> io::Result<()> {
            Ok(())
        }
        fn replace_user_value(&mut self, _: ThemeSettingKey, _: Option<&str>) -> io::Result<()> {
            panic!("capture attempted a settings write")
        }
        fn synchronize(&mut self) -> io::Result<()> {
            panic!("capture attempted backend synchronization")
        }
    }
    if let Ok(role) = std::env::var(ROLE) {
        // SAFETY: this isolated root fixture drops all supplementary groups and
        // every real/effective/saved account identity before decoding input.
        unsafe {
            assert_eq!(libc::setgroups(0, std::ptr::null()), 0);
            assert_eq!(libc::setresgid(65534, 65534, 65534), 0);
            assert_eq!(libc::setresuid(65534, 65534, 65534), 0);
        }
        assert_eq!(rustix::process::geteuid().as_raw(), 65534);
        crate::theme_actor_sandbox::harden_worker().unwrap();
        // SAFETY: run_owned passes this sole inherited one-shot connection.
        let worker = unsafe { UnixStream::from_raw_fd(3) };
        let result = serve_capture(worker, |request, _control| {
            assert_eq!(role, "matching");
            Ok(Box::new(ReadOnlyActor { binding: request.binding.clone() }))
        });
        if role == "matching" {
            result.unwrap();
        } else {
            assert!(result.unwrap_err().to_string().contains("account mismatch"));
        }
        return;
    }
    assert_eq!(rustix::process::geteuid().as_raw(), 0, "run this ignored harness as root");
    let sha = || Sha256::new("11".repeat(32)).unwrap();
    for mismatch in [false, true] {
        let (worker, mut caller) = UnixStream::pair().unwrap();
        caller.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let request = CaptureRequest {
            version: 1,
            nonce: Identifier::new("capture-root-peer-fixture").unwrap(),
            binding: ThemeSettingsBinding {
                account: AccountPrincipal::from_account(65534, 65534, "nobody", "/nonexistent")
                    .unwrap(),
                uid: if mismatch { 1000 } else { 65534 },
                executable_sha256: sha(),
                schema_sha256: sha(),
                session_service_sha256: sha(),
            },
            approved_policy_sha256: sha(),
            plan_sha256: sha(),
            desired: [(
                ThemeSettingKey::ColorScheme,
                ThemeSettingChange::Set("prefer-light".into()),
            )]
            .into(),
            native_receipt: crate::theme_settings::test_native_receipt(),
            session: crate::theme_actor_scope::SessionWitness {
                device: 1,
                inode: 1,
                uid: 65534,
                process: ActorIdentity { pid: 1, start_time: 1 },
                executable_sha256: sha(),
            },
            runtime_files: vec![],
        };
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        caller.write_all(&bytes).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--ignored", "--exact", TEST, "--nocapture"])
            .env(ROLE, if mismatch { "mismatching" } else { "matching" });
        run_owned(command, &worker, Duration::from_secs(2), &mut || false).unwrap();
        drop(worker);
        if mismatch {
            let mut byte = [0];
            assert_eq!(caller.read(&mut byte).unwrap(), 0);
        } else {
            let reply: CaptureReply =
                serde_json::from_slice(&read_frame(&mut caller).unwrap()).unwrap();
            assert_eq!(reply.nonce, request.nonce);
            assert_eq!(reply.capsule.native_receipt, request.native_receipt);
            assert_eq!(reply.capsule.binding, request.binding);
            assert_eq!(reply.capsule.keys[&ThemeSettingKey::ColorScheme].user_value, None);
        }
    }
}
