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
fn an_ordinary_account_cannot_create_or_receive_a_root_capture_descriptor() {
    let (carrier, _peer) = UnixStream::pair().unwrap();
    let (control, _root) = UnixStream::pair().unwrap();
    assert_eq!(
        send_root_control(&carrier, &control).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(receive_root_control(&carrier).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
}

#[test]
fn capture_reply_requires_the_exact_nonce_checkpoint_policy_plan_and_key_scope() {
    use crate::{
        AccountPrincipal,
        logical_settings::{RuntimeFile, SessionWitness, ThemeSettingState},
        wallpaper_protocol::ActorIdentity,
    };
    let sha = |pair: &str| Sha256::new(pair.repeat(32)).unwrap();
    let uid = rustix::process::geteuid().as_raw();
    let session = SessionWitness {
        device: 1,
        inode: 1,
        uid,
        process: ActorIdentity { pid: 1, start_time: 1 },
        executable_sha256: sha("11"),
    };
    let request = CaptureRequest {
        version: 1,
        nonce: Identifier::new("capture-exact-reply").unwrap(),
        binding: ThemeSettingsBinding {
            account: AccountPrincipal::from_account(uid, uid, "fixture", "/home/fixture").unwrap(),
            uid,
            executable_sha256: sha("11"),
            schema_sha256: sha("11"),
            session_service_sha256: session.digest().unwrap(),
        },
        approved_policy_sha256: sha("11"),
        plan_sha256: sha("11"),
        desired: [(ThemeSettingKey::ColorScheme, ThemeSettingChange::Set("prefer-light".into()))]
            .into(),
        native_receipt: crate::theme_settings_actor::test_native_receipt(),
        session,
        runtime_files: vec![RuntimeFile {
            path: "/usr/libexec/ryoku/ryoku-theme-actor".into(),
            sha256: sha("11"),
        }],
    };
    for fault in 0..10 {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut reply = CaptureReply {
            version: 1,
            nonce: request.nonce.clone(),
            capsule: ThemeSettingsCapsule {
                version: 1,
                native_receipt: request.native_receipt.clone(),
                binding: request.binding.clone(),
                approved_policy_sha256: request.approved_policy_sha256.clone(),
                plan_sha256: request.plan_sha256.clone(),
                changes_sha256: crate::logical_settings::changes_digest(&request.desired).unwrap(),
                keys: [(
                    ThemeSettingKey::ColorScheme,
                    ThemeSettingState {
                        user_value: None,
                        effective_value: "default".into(),
                        default_value: "default".into(),
                        writable: true,
                    },
                )]
                .into(),
            },
        };
        match fault {
            0 | 9 => {}
            1 => reply.version = 2,
            2 => reply.nonce = Identifier::new("other-call").unwrap(),
            3 => reply.capsule.binding.uid += 1,
            4 => {
                let mut receipt = serde_json::to_value(&reply.capsule.native_receipt).unwrap();
                receipt["subvolume_id"] = 301.into();
                reply.capsule.native_receipt = serde_json::from_value(receipt).unwrap();
            }
            5 => reply.capsule.approved_policy_sha256 = sha("22"),
            6 => reply.capsule.plan_sha256 = sha("22"),
            7 => reply.capsule.changes_sha256 = sha("22"),
            8 => {
                reply.capsule.keys.insert(
                    ThemeSettingKey::GtkTheme,
                    reply.capsule.keys[&ThemeSettingKey::ColorScheme].clone(),
                );
            }
            _ => unreachable!(),
        }
        let sender = std::thread::spawn(move || {
            read_frame(&mut server).unwrap();
            let mut bytes = serde_json::to_vec(&reply).unwrap();
            bytes.push(b'\n');
            if fault == 9 {
                bytes.extend_from_slice(b"{}\n");
            }
            server.write_all(&bytes).unwrap();
        });
        assert_eq!(request_capture(client, &request).is_ok(), fault == 0, "fault {fault}");
        sender.join().unwrap();
    }
    for fault in 0..4 {
        let mut invalid = request.clone();
        match fault {
            0 => invalid.version = 2,
            1 => invalid.desired.clear(),
            2 => invalid.runtime_files.push(invalid.runtime_files[0].clone()),
            3 => {
                invalid.runtime_files = (0..128)
                    .map(|index| RuntimeFile {
                        path: format!("/usr/lib/{index:03}/{}", "x".repeat(490)),
                        sha256: sha("11"),
                    })
                    .collect()
            }
            _ => unreachable!(),
        }
        let error = invalid.validate().unwrap_err();
        if fault == 3 {
            assert!(error.to_string().contains("exceeds its bound"));
        }
    }
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
            "wait" => {
                let mut byte = [0];
                control.read_exact(&mut byte).expect("cancellation fixture keeps its control peer");
                panic!("cancellation fixture received an unexpected request");
            }
            _ => panic!("unknown fixture role"),
        }
    }
    for (role, cancel) in [
        ("success", false),
        ("confinement", false),
        ("failure", false),
        ("spin", false),
        ("wait", true),
    ] {
        let (worker, mut root) = UnixStream::pair().unwrap();
        root.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", FIXTURE, "--nocapture"]).env(ROLE, role);
        let write_target = std::env::temp_dir()
            .join(format!("ryoku-theme-readonly-{}-{role}-{cancel}", std::process::id()));
        std::fs::write(&write_target, b"before").unwrap();
        command.env("RYOKU_THEME_WRITE_TARGET", &write_target);
        let start = Instant::now();
        let child = spawn_owned(command, &worker).unwrap();
        // Identity comes from the owned OS child, so a deadline before its
        // first fixture instruction still proves exact process reaping.
        let pid = child.id();
        if cancel {
            let bytes =
                read_frame(&mut root).expect("wait fixture must be blocked before cancellation");
            assert_eq!(std::str::from_utf8(&bytes).unwrap().trim().parse::<u32>().unwrap(), pid);
        }
        let timeout =
            if role == "spin" { Duration::from_millis(200) } else { Duration::from_secs(3) };
        let result = wait_owned(child, timeout, &mut || cancel);
        assert_eq!(result.is_ok(), matches!(role, "success" | "confinement"), "{role}: {result:?}");
        if role == "failure" {
            assert!(result.as_ref().unwrap_err().to_string().contains("worker failed"));
        }
        if role == "spin" {
            assert!(result.as_ref().unwrap_err().to_string().contains("timed out"));
        }
        if cancel {
            assert!(result.as_ref().unwrap_err().to_string().contains("cancelled"));
        }
        assert!(start.elapsed() < Duration::from_secs(4));
        drop(worker);
        if matches!(role, "success" | "confinement" | "failure") {
            let bytes = read_frame(&mut root).unwrap();
            assert_eq!(std::str::from_utf8(&bytes).unwrap().trim().parse::<u32>().unwrap(), pid);
        }
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "owned worker remained alive"
        );
        assert_eq!(std::fs::read(&write_target).unwrap(), b"before");
        std::fs::remove_file(write_target).unwrap();
    }
    let (worker, _peer) = UnixStream::pair().unwrap();
    let error = run_owned(
        Command::new("/does-not-exist/ryoku-never-launched"),
        &worker,
        Duration::from_secs(1),
        &mut || true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled before launch"));
    for timeout in [Duration::ZERO, Duration::from_secs(31)] {
        let error = run_owned(
            Command::new("/does-not-exist/ryoku-never-launched"),
            &worker,
            timeout,
            &mut || false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
    }
}

/// Run explicitly by the hosted root harness after building this test binary.
/// This proves real root-peer transport to a nonroot worker, not native DConf
/// or desktop-session success; the injected backend never writes any setting.
#[test]
#[ignore = "requires a root parent that launches the worker as uid65534"]
fn root_peer_capture_binds_the_native_receipt_and_rejects_account_mismatch() {
    use crate::theme_settings_actor::ThemeSettingState;
    use crate::{AccountPrincipal, wallpaper_protocol::ActorIdentity};
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
        // SAFETY: run_owned passes the root-authenticated lease carrier. Its
        // separately transferred descriptor must retain the root peer after
        // crossing into this ordinary account.
        let carrier = unsafe { UnixStream::from_raw_fd(3) };
        let received = receive_root_control(&carrier);
        if role == "missing-fd" {
            assert!(received.unwrap_err().to_string().contains("incomplete or ambiguous"));
            return;
        }
        let worker = received.unwrap();
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
    for role in ["matching", "mismatching", "missing-fd"] {
        let mismatch = role == "mismatching";
        let (worker, mut caller) = UnixStream::pair().unwrap();
        let (carrier_sender, carrier_receiver) = UnixStream::pair().unwrap();
        if role == "missing-fd" {
            (&carrier_sender).write_all(b"C").unwrap();
        } else {
            send_root_control(&carrier_sender, &worker).unwrap();
        }
        drop(worker);
        caller.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut request = CaptureRequest {
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
            native_receipt: crate::theme_settings_actor::test_native_receipt(),
            session: crate::logical_settings::SessionWitness {
                device: 1,
                inode: 1,
                uid: 65534,
                process: ActorIdentity { pid: 1, start_time: 1 },
                executable_sha256: sha(),
            },
            runtime_files: vec![crate::logical_settings::RuntimeFile {
                path: "/usr/libexec/ryoku/ryoku-theme-actor".into(),
                sha256: sha(),
            }],
        };
        request.binding.session_service_sha256 = request.session.digest().unwrap();
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        if role != "missing-fd" {
            caller.write_all(&bytes).unwrap();
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--ignored", "--exact", TEST, "--nocapture"]).env(ROLE, role);
        run_owned(command, &carrier_receiver, Duration::from_secs(2), &mut || false).unwrap();
        drop(carrier_receiver);
        if role != "matching" {
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
