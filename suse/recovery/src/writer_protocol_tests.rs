use super::*;
use std::{
    path::PathBuf,
    process::Child,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const TEST: &str =
    "writer_protocol::tests::leases_cover_children_start_races_and_abrupt_owner_death";
const ROLE: &str = "RYOKU_LEASE_TEST_ROLE";
const PATH: &str = "RYOKU_LEASE_TEST_PATH";
const PREFIX: &str = "RYOKU_LEASE_TEST_PREFIX";

struct Fixture {
    path: PathBuf,
    namespace_path: PathBuf,
    children: Vec<Child>,
}
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "ryoku-writer-lease-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let namespace_path = path.join("namespace");
        std::fs::create_dir(&namespace_path).unwrap();
        std::fs::set_permissions(&namespace_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = Self { path, namespace_path, children: Vec::new() };
        result.namespace().open_lock(true).unwrap();
        result
    }
    fn namespace(&self) -> Namespace {
        test_namespace(&self.path)
    }
    fn spawn(&mut self, role: &str, prefix: &str) -> usize {
        let child = test_command(&self.path, role, prefix).spawn().unwrap();
        self.children.push(child);
        self.children.len() - 1
    }
    fn wait(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.path.join(name).exists() {
            assert!(Instant::now() < deadline, "missing {name}");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn counters(&self, prefix: &str) -> Vec<Vec<u8>> {
        ["parent", "grandchild"]
            .into_iter()
            .map(|name| std::fs::read(self.path.join(format!("{prefix}-{name}"))).unwrap())
            .collect()
    }
    fn resumes(&self, prefix: &str, saved: &[Vec<u8>]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.counters(prefix).iter().zip(saved).all(|(a, b)| a != b) {
                return;
            }
            assert!(Instant::now() < deadline, "{prefix} writers remained stopped");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn lease(&self) -> CaptureWriterLease {
        CaptureWriterLease::acquire(self.namespace(), self.path.to_str().unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::write(self.path.join("stop"), "1");
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let active = std::fs::read_dir(&self.path).ok().is_some_and(|entries| {
                entries.flatten().any(|entry| {
                    entry.file_name().to_str().is_some_and(|name| name.ends_with(".active"))
                })
            });
            if !active {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn test_namespace(path: &std::path::Path) -> Namespace {
    let root = File::open(path.join("namespace")).unwrap();
    let stat = fs::fstat(&root).unwrap();
    Namespace { root, uid: stat.st_uid, gid: stat.st_gid, native_mount: None }
}
fn test_command(path: &std::path::Path, role: &str, prefix: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(PATH, path)
        .env(PREFIX, prefix)
        .env("HOME", path)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("RYOGAMI_WALL_CONFIG")
        .env_remove("RYOKU_STATE_PATH");
    command
}
fn counter(path: &std::path::Path, name: &str) {
    let marker = path.join(format!("{name}.active"));
    std::fs::write(&marker, "1").unwrap();
    for value in 1_u64..=u64::MAX {
        if path.join("stop").exists() {
            break;
        }
        let next = path.join(format!("{name}.next"));
        std::fs::write(&next, value.to_string()).unwrap();
        std::fs::rename(&next, path.join(name)).unwrap();
        thread::sleep(Duration::from_millis(2));
    }
    std::fs::remove_file(marker).unwrap();
}

#[test]
fn leases_cover_children_start_races_and_abrupt_owner_death() {
    if let Ok(role) = std::env::var(ROLE) {
        let path = PathBuf::from(std::env::var_os(PATH).unwrap());
        let prefix = std::env::var(PREFIX).unwrap();
        match role.as_str() {
            "wrapper" => {
                install_signal_forwarding().unwrap();
                run_using(
                    test_namespace(&path),
                    WriterKind::Daemon,
                    test_command(&path, "writer", &prefix),
                    rustix::process::geteuid().as_raw(),
                )
                .unwrap();
            }
            "pinned-daemon" => {
                let binary = std::env::var_os("RYOKU_ISSUE18_PINNED_SHELL").unwrap();
                let mut command = Command::new(binary);
                command
                    .arg("daemon")
                    .env_clear()
                    .env("HOME", &path)
                    .env("XDG_RUNTIME_DIR", path.join("run"))
                    .env(
                        "DBUS_SYSTEM_BUS_ADDRESS",
                        format!("unix:path={}/absent-system-bus", path.display()),
                    )
                    .env(
                        "DBUS_SESSION_BUS_ADDRESS",
                        format!("unix:path={}/absent-session-bus", path.display()),
                    )
                    .env("PATH", path.join("empty-bin"))
                    .stdout(std::fs::File::create(path.join("daemon.log")).unwrap())
                    .stderr(std::process::Stdio::null());
                run_using(
                    test_namespace(&path),
                    WriterKind::Daemon,
                    command,
                    rustix::process::geteuid().as_raw(),
                )
                .unwrap();
            }
            "nested-wrapper" => {
                run_using(
                    test_namespace(&path),
                    WriterKind::Daemon,
                    test_command(&path, "nested-writer", &prefix),
                    rustix::process::geteuid().as_raw(),
                )
                .unwrap();
            }
            "nested-writer" => {
                let mut child = test_command(&path, "wrapper", "inner").spawn().unwrap();
                counter(&path, "outer-parent");
                child.wait().unwrap();
            }
            "writer" => {
                let mut child = test_command(&path, "grandchild", &prefix).spawn().unwrap();
                counter(&path, &format!("{prefix}-parent"));
                child.wait().unwrap();
            }
            "grandchild" => counter(&path, &format!("{prefix}-grandchild")),
            "helper" => {
                let guard =
                    CaptureWriterLease::acquire(test_namespace(&path), path.to_str().unwrap())
                        .unwrap();
                std::fs::write(path.join("captured"), "1").unwrap();
                loop {
                    guard.check().unwrap();
                    thread::sleep(Duration::from_millis(20));
                }
            }
            _ => panic!("unknown fixture role"),
        }
        return;
    }
    let mut fixture = Fixture::new();
    let first = fixture.spawn("wrapper", "first");
    fixture.wait("first-parent");
    fixture.wait("first-grandchild");
    let mut lease = fixture.lease();
    let saved = fixture.counters("first");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(fixture.counters("first"), saved);
    let second = fixture.spawn("wrapper", "second");
    thread::sleep(Duration::from_millis(50));
    assert!(
        !fixture.path.join("second-parent").exists(),
        "new writer started through exclusive capture"
    );
    lease.check().unwrap();
    lease.release().unwrap();
    fixture.resumes("first", &saved);
    fixture.wait("second-parent");
    fixture.wait("second-grandchild");

    let helper = fixture.spawn("helper", "helper");
    fixture.wait("captured");
    let first_saved = fixture.counters("first");
    let second_saved = fixture.counters("second");
    thread::sleep(Duration::from_millis(50));
    assert_eq!(fixture.counters("first"), first_saved);
    assert_eq!(fixture.counters("second"), second_saved);
    fixture.children[helper].kill().unwrap();
    fixture.children[helper].wait().unwrap();
    fixture.resumes("first", &first_saved);
    fixture.resumes("second", &second_saved);

    let lease = fixture.lease();
    let saved = fixture.counters("first");
    fixture.children[first].kill().unwrap();
    fixture.children[first].wait().unwrap();
    assert!(lease.check().is_err(), "dead coordinator remained valid for capture");
    fixture.resumes("first", &saved);
    drop(lease);
    assert!(
        CaptureWriterLease::acquire(fixture.namespace(), fixture.path.to_str().unwrap()).is_err(),
        "stale writer endpoint was silently adopted"
    );
    std::fs::write(fixture.path.join("stop"), "1").unwrap();
    fixture.children[second].wait().unwrap();

    let mut nested = Fixture::new();
    let wrapper = nested.spawn("nested-wrapper", "outer");
    nested.wait("outer-parent");
    nested.wait("inner-parent");
    nested.wait("inner-grandchild");
    let mut lease = nested.lease();
    assert_eq!(lease.names.len(), 2);
    let inner = nested.counters("inner");
    let outer = std::fs::read(nested.path.join("outer-parent")).unwrap();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(nested.counters("inner"), inner);
    assert_eq!(std::fs::read(nested.path.join("outer-parent")).unwrap(), outer);
    lease.check().unwrap();
    lease.release().unwrap();
    nested.resumes("inner", &inner);
    std::fs::write(nested.path.join("stop"), "1").unwrap();
    nested.children[wrapper].wait().unwrap();

    let mut shutdown = Fixture::new();
    let wrapper = shutdown.spawn("wrapper", "shutdown");
    shutdown.wait("shutdown-parent");
    shutdown.wait("shutdown-grandchild");
    let pid = rustix::process::Pid::from_raw(shutdown.children[wrapper].id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if shutdown.children[wrapper].try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "wrapper failed to forward termination");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(shutdown.namespace().sockets().unwrap().is_empty());
}

#[test]
fn wrapper_death_at_publication_boundary_retracts_the_generation() {
    use crate::{
        EntryKind, FileRecord, Identifier, OperationReason, RecoveryManifest, RelativePath,
        SnapshotLink, StateRoot, VerificationStatus,
        generation::{GenerationError, GenerationStore, Payloads},
    };
    use std::os::unix::fs::PermissionsExt;

    for fail_at in [2, 4] {
        let mut fixture = Fixture::new();
        let wrapper = fixture.spawn("wrapper", "publication");
        fixture.wait("publication-parent");
        fixture.wait("publication-grandchild");
        let lease = fixture.lease();
        let saved = fixture.counters("publication");
        let state = fixture.path.join("store");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = GenerationStore::open_for_test(&File::open(&state).unwrap()).unwrap();
        let mut manifest = RecoveryManifest::new(
            Identifier::new("writer-publication").unwrap(),
            Identifier::new("writer-generation").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ProfileApply,
            "2026-10-20T00:00:00Z",
        )
        .unwrap();
        manifest
            .add_file(
                FileRecord::new(
                    StateRoot::UserHome,
                    RelativePath::new(".config/ryoku/shell.json").unwrap(),
                    EntryKind::Absent,
                )
                .unwrap(),
            )
            .unwrap();
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        manifest.seal().unwrap();
        let mut checks = 0;
        let result = store.publish_checked(&manifest, &Payloads::new(), || {
            checks += 1;
            if checks == fail_at {
                if fail_at == 4 {
                    assert!(store.verify_transaction(&manifest.transaction_id).is_ok());
                } else {
                    assert!(state.join("staging/writer-publication.stage/manifest.json").exists());
                    assert!(!state.join("generations/writer-generation").exists());
                }
                fixture.children[wrapper].kill().unwrap();
                fixture.children[wrapper].wait().unwrap();
            }
            lease.check().is_ok()
        });
        assert!(matches!(result, Err(GenerationError::PublicationRejected)));
        assert_eq!(checks, fail_at);
        assert!(store.verify_transaction(&manifest.transaction_id).is_err());
        assert!(store.verify(&manifest.generation_id).is_err());
        for directory in ["catalog", "generations", "staging"] {
            assert_eq!(std::fs::read_dir(state.join(directory)).unwrap().count(), 0);
        }
        fixture.resumes("publication", &saved);
    }
}

#[test]
#[ignore = "requires exact pinned upstream Go daemon binary; never downloads at test time"]
fn pinned_upstream_daemon_writes_are_quiesced() {
    use std::os::unix::fs::PermissionsExt;
    assert!(std::env::var_os("RYOKU_ISSUE18_PINNED_SHELL").is_some());
    let mut fixture = Fixture::new();
    std::fs::create_dir(fixture.path.join("run")).unwrap();
    std::fs::set_permissions(fixture.path.join("run"), std::fs::Permissions::from_mode(0o700))
        .unwrap();
    std::fs::create_dir(fixture.path.join("empty-bin")).unwrap();
    fixture.spawn("pinned-daemon", "native");
    fixture.wait("run/ryoku-shell.sock");
    let socket = fixture.path.join("run/ryoku-shell.sock");
    let setting = fixture.path.join(".config/ryoku/shell.json");
    let request = |value: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("native daemon socket unavailable: {error}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        writeln!(stream, "barstyle {value}").unwrap();
        stream
    };
    let mut first = request("obi");
    let mut reply = [0; 1024];
    assert!(first.read(&mut reply).unwrap() > 0);
    let before = std::fs::read(&setting).unwrap();
    assert!(String::from_utf8_lossy(&before).contains("obi"));
    let mut lease = fixture.lease();
    let mut pending = request("sumi");
    pending.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    assert!(pending.read(&mut reply).is_err());
    assert_eq!(std::fs::read(&setting).unwrap(), before);
    lease.check().unwrap();
    lease.release().unwrap();
    pending.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert!(pending.read(&mut reply).unwrap() > 0);
    assert!(String::from_utf8_lossy(&std::fs::read(&setting).unwrap()).contains("sumi"));
    let mut quit = UnixStream::connect(socket).unwrap();
    writeln!(quit, "quit").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = fixture.children[0].try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "native daemon did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn pinned_daemon_launch_patch_covers_the_supervised_native_surface_tree() {
    let original = include_bytes!("../tests/fixtures/issue18-pinned-ryoku-shell.service");
    let fixture = Fixture::new();
    let path = fixture.path.join("ryoku/shell/systemd/user");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("ryoku-shell.service"), original).unwrap();
    let hash = Command::new("git")
        .args(["hash-object", "ryoku/shell/systemd/user/ryoku-shell.service"])
        .current_dir(&fixture.path)
        .output()
        .unwrap();
    assert!(hash.status.success());
    assert_eq!(
        String::from_utf8(hash.stdout).unwrap().trim(),
        "a18ecd57a7373d59a5d4611010cec0f04678f115"
    );
    let full_patch = include_str!("../../../upstream/patches/issue18-writer-launch.patch");
    let patch = fixture.path.join("unit.patch");
    std::fs::write(
        &patch,
        full_patch
            .split("diff --git ")
            .find(|text| text.starts_with("a/ryoku/shell/systemd/user/ryoku-shell.service "))
            .map(|text| format!("diff --git {text}"))
            .unwrap(),
    )
    .unwrap();
    let output = Command::new("patch")
        .args(["--batch", "--fuzz=0", "-p1", "-i"])
        .arg(patch)
        .current_dir(&fixture.path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let unit = std::fs::read_to_string(path.join("ryoku-shell.service")).unwrap();
    assert!(unit.contains("ExecStart=/usr/libexec/ryoku/ryoku-writer daemon daemon\n"));
    assert!(!unit.contains("ExecStart=/usr/bin/ryoku-shell daemon\n"));
    let helper = include_str!("../../../packaging/rpm/systemd/ryoku-helper.service");
    assert!(helper.contains("CapabilityBoundingSet=CAP_DAC_READ_SEARCH\n"));
    assert!(!helper.contains("CAP_SYS_PTRACE"));
}

#[test]
fn coordination_lock_rejects_symlinks_hardlinks_and_replacement() {
    let fixture = Fixture::new();
    let namespace = fixture.namespace();
    let file = namespace.open_lock(false).unwrap();
    std::fs::rename(fixture.namespace_path.join(LOCK), fixture.namespace_path.join("retained"))
        .unwrap();
    std::fs::write(fixture.namespace_path.join(LOCK), "different").unwrap();
    assert!(namespace.check_lock(&file).is_err());
    std::fs::remove_file(fixture.namespace_path.join(LOCK)).unwrap();
    std::os::unix::fs::symlink("retained", fixture.namespace_path.join(LOCK)).unwrap();
    assert!(namespace.open_lock(false).is_err());
    std::fs::remove_file(fixture.namespace_path.join(LOCK)).unwrap();
    std::fs::hard_link(fixture.namespace_path.join("retained"), fixture.namespace_path.join(LOCK))
        .unwrap();
    assert!(namespace.open_lock(false).is_err());
}

#[test]
fn every_declared_writer_has_a_fixed_launch_route() {
    let roles = [
        ("daemon", "/usr/libexec/ryoku/upstream/ryoku-shell", None),
        ("provider", "/usr/libexec/ryoku/upstream/ryoku-wm-hyprland", None),
        ("hub-backend", "/usr/libexec/ryoku/upstream/ryoku-hub", None),
        ("ryogami", "/usr/libexec/ryoku/upstream/ryogami", None),
        (
            "ryoshot",
            "/usr/libexec/ryoku/quickshell/qs",
            Some("/usr/share/ryoku/shell/quickshell/ryoshot"),
        ),
        (
            "welcome",
            "/usr/libexec/ryoku/quickshell/qs",
            Some("/usr/share/ryoku/shell/quickshell/welcome"),
        ),
    ];
    let map = include_str!("../../../packaging/writer-coordination/payload-map.toml");
    for (name, program, path) in roles {
        let role = WriterKind::parse(name).unwrap();
        assert_eq!(role.name(), name);
        let command = role.command();
        assert_eq!(command.get_program(), program);
        let args: Vec<_> = command.get_args().collect();
        match path {
            Some(path) => {
                assert_eq!(args, ["-p", path]);
                assert!(map.contains(path));
            }
            None => {
                assert!(args.is_empty());
                assert!(map.contains(program));
            }
        }
        assert!(map.contains(&format!("writer_role = \"{name}\"")));
    }
    assert!(WriterKind::parse("widgets").is_err()); // consolidated into shell at the pin
    assert!(WriterKind::parse("/bin/sh").is_err());
}

#[test]
#[ignore = "requires offline exact pinned source audit tree"]
fn all_pinned_upstream_writer_routes_apply_without_fuzz() {
    let root = PathBuf::from(std::env::var_os("RYOKU_ISSUE18_PINNED_SOURCE").unwrap());
    let sources: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!(
        "../../../packaging/writer-coordination/pinned-route-sources.json"
    ))
    .unwrap();
    let fixture = Fixture::new();
    for (path, expected) in &sources {
        let original = root.join(path);
        let hash = Command::new("git").arg("hash-object").arg(&original).output().unwrap();
        assert!(hash.status.success());
        assert_eq!(String::from_utf8(hash.stdout).unwrap().trim(), expected, "{path}");
        let target = fixture.path.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(original, target).unwrap();
    }
    let patch = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../upstream/patches/issue18-writer-launch.patch");
    for apply in [false, true] {
        let mut command = Command::new("git");
        command.args(["apply", "--unsafe-paths"]);
        if !apply {
            command.arg("--check");
        }
        let result = command.arg(&patch).current_dir(&fixture.path).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    for path in sources.keys() {
        let contents = std::fs::read_to_string(fixture.path.join(path)).unwrap();
        assert!(
            contents.contains("/usr/libexec/ryoku/ryoku-writer")
                || contents.contains("/usr/libexec/ryoku/quickshell/qs")
                || contents
                    .contains("Based on iNiR by snowarch (https://github.com/snowarch/inir)")
                || (path.ends_with("nomarchy/plugins/panels/network/Model.js")
                    && !contents.contains("enterpriseConnectScript")),
            "{path}"
        );
        assert!(!contents.contains("qs -c ryoshot"), "{path}");
        assert!(!contents.contains("qs -c welcome"), "{path}");
        for forbidden in [
            "Networking.wifiEnabled =",
            ".discovering =",
            ".scannerEnabled =",
            ".trusted =",
            "d.blocked =",
            "device.blocked =",
            "root.device.blocked =",
            "net.disconnect()",
            "net.connect()",
            "network.connect()",
            "network.forget()",
            "network.connectWithPsk(psk)",
            "network.connectWithPsk(passphrase)",
            "entry.network.connect()",
            "entry.network.forget()",
            "d.disconnect()",
            "d.forget()",
            "device.disconnect()",
            "device.forget()",
            "targetNet.connect()",
            "targetNet.connectWithPsk(password)",
            "devToForget.forget()",
            "devToDisconnect.disconnect()",
        ] {
            assert!(
                !contents.lines().filter(|line| !line.trim_start().starts_with("//")).any(|line| {
                    line.split_once(forbidden).is_some_and(|(_, suffix)| {
                        !forbidden.ends_with('=') || !suffix.starts_with('=')
                    })
                }),
                "{path}: {forbidden}"
            );
        }
    }
    // These QML actuators bypassed the public Hub alias in v1.0. The
    // applied, blob-verified payload must send them to the Rust refusal
    // boundary rather than package managers, privileged shells or systemctl.
    for (path, forbidden) in [
        ("ryoku/hub/quickshell/SettingsSheet.qml", "sudo ryoku-host pkg install"),
        ("ryoku/hub/quickshell/pages/DictationPage.qml", "rm -f ~/.config/systemd"),
        ("ryoku/hub/quickshell/pages/DictationPage.qml", "sudo ryoku-host pkg remove"),
        ("ryoku/hub/quickshell/pages/ConnectionsPage.qml", "[\"pkexec\""),
        ("ryoku/hub/quickshell/pages/users/UsersTab.qml", "pkexec"),
        ("ryoku/hub/quickshell/pages/LockscreenPage.qml", "cmd: [\"fprintd-enroll\""),
        ("ryoku/shell/quickshell/inir/services/Vpn.qml", "pkexec"),
        ("ryoku/shell/quickshell/shell/services/Tailscale.qml", "[\"pkexec\""),
        (
            "ryoku/shell/quickshell/shell/modules/bar/barstyles/nomarchy/plugins/panels/tailscale/Service.qml",
            "[\"pkexec\"",
        ),
        (
            "ryoku/shell/quickshell/shell/modules/dock/ii/services/DictationService.qml",
            "[\"pkexec\"",
        ),
        (
            "ryoku/shell/quickshell/shell/modules/dock/ii/services/DictationService.qml",
            "[\"systemctl\", \"--user\", \"restart\"",
        ),
        ("ryoku/shell/quickshell/inir/services/Battery.qml", "/usr/bin/pkexec"),
        ("ryoku/shell/quickshell/inir/services/deferred/PackageSearch.qml", "sudo "),
        ("ryoku/shell/quickshell/inir/services/deferred/PackageSearch.qml", "_runTerminalScript"),
        (
            "ryoku/shell/quickshell/inir/services/Network.qml",
            "[\"nmcli\", \"connection\", \"modify\"",
        ),
        (
            "ryoku/hub/quickshell/pages/ConnectionsPage.qml",
            "[\"nmcli\", \"connection\", \"modify\"",
        ),
        ("ryoku/hub/quickshell/pages/ConnectionsPage.qml", "nmcli connection add"),
        (
            "ryoku/shell/quickshell/inir/modules/common/models/quickToggles/HotspotToggle.qml",
            "nmcli connection delete id Hotspot",
        ),
        (
            "ryoku/shell/quickshell/shell/modules/bar/barstyles/qsbar/panels/NetworkManagerAdapter.qml",
            "[\"nmcli\", \"--wait\", \"20\", \"connection\"",
        ),
        (
            "ryoku/shell/quickshell/shell/services/AiUsage.qml",
            "[\"systemctl\", \"--user\", \"start\"",
        ),
    ] {
        let contents = std::fs::read_to_string(fixture.path.join(path)).unwrap();
        assert!(!contents.contains(forbidden), "{path}: {forbidden}");
    }
    let phone = std::fs::read_to_string(
        fixture.path.join("ryoku/shell/quickshell/shell/modules/phone/KdeConnectService.qml"),
    )
    .unwrap();
    for forbidden in [
        "Pairing accepted",
        "Pairing declined",
        "devId + \" org.kde.kdeconnect.device.acceptPairing",
        "devId + \" org.kde.kdeconnect.device.cancelPairing",
        "adbActionProc.command = [\"adb\"",
        "\" tcpip 5555 && sleep 1 && adb connect ",
    ] {
        assert!(!phone.contains(forbidden), "{forbidden}");
    }
    for verb in ["accept-pair", "reject-pair", "adb-wifi", "adb-pair", "adb-connect", "device-call"]
    {
        assert!(phone.contains(&format!("\"hub-backend\", \"phone\", \"{verb}\"")), "{verb}");
        assert!(!crate::writer_command_policy::hub_query(&[
            "phone".into(),
            verb.into(),
            "device".into(),
        ]));
    }
    let bridge = std::fs::read_to_string(
        fixture.path.join("ryoku/shell/quickshell/shell/modules/phone/PhoneBridge.qml"),
    )
    .unwrap();
    assert!(bridge.contains("property bool installed: false"));
    assert!(bridge.contains("const ready = root.enabled && root.installed"));
    assert!(bridge.contains("PhoneState.runtimeReady = ready"));
    assert!(bridge.contains(
        "command: [\"/usr/libexec/ryoku/ryoku-writer\", \"hub-backend\", \"phone\", \"status\"]"
    ));
    for handler in
        ["pair", "unpair", "acceptPair", "rejectPair", "adbWifi", "adbPair", "adbConnect"]
    {
        let function = bridge.split(&format!("function {handler}(")).nth(1).unwrap();
        let body = function.split('}').next().unwrap();
        assert!(
            body.trim_start_matches(|c: char| c != '{')
                .starts_with("{\n            if (!PhoneState.runtimeReady) return root.fail("),
            "{handler}"
        );
    }
    // No successful status can reach either existing capability gate: the
    // public alias and explicit role reject status/devices before child setup.
    for query in ["status", "devices"] {
        assert!(matches!(
            run_writer(WriterKind::HubBackend, &["phone".into(), query.into()]),
            Err(crate::capture_session::CaptureSessionError::Unsupported)
        ));
    }
    for (path, blob, fragments) in [
        (
            "ryoku/shell/quickshell/shell/modules/phone/PhoneState.qml",
            "3e2b13db7e8deeaf6503bb8f190a70d641a76d12",
            vec!["property bool runtimeReady: false"],
        ),
        (
            "ryoku/hub/quickshell/pages/PhoneConnectionTab.qml",
            "d415d3515ae7285eca7bf9b899644b12d1778d9d",
            vec![
                "property var status: ({ installed: false",
                "readonly property bool phoneActive: enabledOnDisk && status.installed",
                "readonly property bool featureReady: phoneActive",
                "if (!pg.phoneActive)",
            ],
        ),
    ] {
        let original = root.join(path);
        let hash = Command::new("git").arg("hash-object").arg(&original).output().unwrap();
        assert!(hash.status.success());
        assert_eq!(String::from_utf8(hash.stdout).unwrap().trim(), blob, "{path}");
        let contents = std::fs::read_to_string(original).unwrap();
        for fragment in fragments {
            assert!(contents.contains(fragment), "{path}: {fragment}");
        }
    }
}

#[test]
fn native_runtime_exception_is_only_the_exact_private_tmpfs_mount_root() {
    let valid = "41 22 0:33 / /run/user/1000 rw,nosuid,nodev,relatime - tmpfs tmpfs rw,size=64m,uid=1000,gid=100\n";
    assert!(runtime_mount_allowed(valid, 41, 1000));
    for changed in [
        valid.replace("/run/user/1000", "/run/user/1001"),
        valid.replace("/run/user/1000", "/run/user/1000/ryoku-writers"),
        valid.replace("0:33 / ", "0:33 /subtree "),
        valid.replace("tmpfs tmpfs", "ext4 /dev/vda"),
        valid.replace("rw,nosuid,nodev", "rw,nodev"),
        valid.replace("rw,nosuid,nodev", "ro,nosuid,nodev"),
        format!("{valid}{valid}"),
    ] {
        assert!(!runtime_mount_allowed(&changed, 41, 1000));
    }
    assert!(!runtime_mount_allowed(valid, 42, 1000));
}
