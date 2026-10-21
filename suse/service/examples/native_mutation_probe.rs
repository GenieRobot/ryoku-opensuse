//! Disposable signed Tumbleweed fixture only. Its local marker actuator is not
//! a provider capability and is never linked into installed helper dispatch.
use ryoku_recovery::{
    Identifier, OperationReason, PackageName, PackageRevision, Revision,
    capture_session::NativeCaptureSession,
    generation::GenerationReader,
    installed_profiles::{DefaultsPackage, InstalledProfileBundle, ProfileBundleStore},
    snapper_catalog::ProtectedSnapperCatalog,
    writer_protocol::{WriterKind, initialize_writer_namespace, run_writer},
};
use ryoku_service::{
    ManagedActionKind, OperationId, OperationJournal, PeerIdentity,
    helper_protocol::prepare_operator_account,
    mutation_gate::{
        GateError, LinkedGeneration, MutationOutcome, MutationStage, PlannedChange,
        VerifiedLinkedGeneration,
    },
    native_mutation::{
        NativeMutationProvider, NativeMutationRequest, ProtectedMutationHistory,
        review_operator_plan, run_operator_mutation,
    },
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const HOME: &str = "/home/tester";
const STATE: &str = "/var/lib/ryoku-state";
const CONFIG: &str = "/home/tester/.config/ryoku";
const FIXTURE: &str = "/run/ryoku-issue20";
fn require(value: bool, message: &str) -> Result<()> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn id(value: &str) -> Result<Identifier> {
    Identifier::new(value).map_err(|_| "invalid fixture ID".into())
}
fn operation(value: &str) -> OperationId {
    OperationId::new(value).expect("fixed fixture ID")
}
fn write_new(path: &str, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(std::path::Path::new(path).parent().ok_or("no parent")?)?.sync_all()?;
    Ok(())
}
fn user(arguments: &[&str]) -> Result<std::process::Output> {
    let output = Command::new("/usr/sbin/runuser")
        .args(["-u", "tester", "--"])
        .arg(std::env::current_exe()?)
        .args(arguments)
        .env("HOME", HOME)
        .output()?;
    require(
        output.status.success(),
        &format!("account fixture failed: {}", String::from_utf8_lossy(&output.stderr)),
    )?;
    Ok(output)
}
fn peer() -> Result<PeerIdentity> {
    let path = format!("{FIXTURE}/peer-{}.sock", std::process::id());
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666))?;
    let mut client = Command::new("/usr/sbin/runuser")
        .args(["-u", "tester", "--"])
        .arg(std::env::current_exe()?)
        .args(["--client", &path])
        .env("HOME", HOME)
        .spawn()?;
    let (stream, _) = listener.accept()?;
    let peer = PeerIdentity::from_unix_socket(&stream).map_err(|e| format!("peer: {e:?}"))?;
    drop(stream);
    require(client.wait()?.success(), "peer disconnected abnormally")?;
    fs::remove_file(path)?;
    Ok(peer)
}
fn group() -> Result<u32> {
    let output = Command::new("/usr/bin/id").args(["-g", "tester"]).output()?;
    require(output.status.success(), "account primary group missing")?;
    Ok(String::from_utf8(output.stdout)?.trim().parse()?)
}
fn bundle_id() -> Result<ryoku_recovery::Sha256> {
    Ok(ryoku_recovery::Sha256::new(fs::read_to_string(format!("{STATE}/issue20-bundle"))?.trim())
        .map_err(|_| "bad bundle ID")?)
}
fn package() -> PackageRevision {
    PackageRevision {
        package_name: PackageName::new("issue20-local-fixture").unwrap(),
        before_nevra: Some(Revision::new("fixture-1").unwrap()),
        after_nevra: None,
        upstream_revision: None,
    }
}
fn journal() -> Result<OperationJournal> {
    Ok(OperationJournal::open(
        OpenOptions::new().read(true).write(true).open(format!("{STATE}/mutations.log"))?,
    )
    .map_err(|e| format!("journal: {e:?}"))?)
}
fn event(kind: &str, name: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(format!("{STATE}/issue20-events"))?;
    writeln!(file, "{kind}:{name}")?;
    file.sync_all()?;
    Ok(())
}
struct Provider {
    name: String,
    mode: String,
    plans: usize,
    auths: usize,
}
impl Provider {
    fn new(name: &str, mode: &str) -> Self {
        Self { name: name.into(), mode: mode.into(), plans: 0, auths: 0 }
    }
}
impl NativeMutationProvider for Provider {
    type Output = ();
    fn plan(&mut self) -> std::result::Result<PlannedChange, GateError> {
        self.plans += 1;
        Ok(PlannedChange {
            operation_key: format!("fixture:{}", self.name),
            plan_fingerprint: if self.mode == "stale" && self.plans >= 3 { "bb" } else { "aa" }
                .repeat(32),
            affected_state_fingerprint: "00".repeat(32),
        })
    }
    fn authorize(&mut self, _: &PlannedChange) -> std::result::Result<(), GateError> {
        self.auths += 1;
        if self.mode == "changed-path" && self.auths == 2 {
            user(&["--changed-shell"]).map_err(|_| GateError::PreflightFailed)?;
        }
        if self.mode == "interrupt-before" && self.auths == 2 {
            std::process::exit(73);
        }
        Ok(())
    }
    fn apply(&mut self, proof: &VerifiedLinkedGeneration) -> std::result::Result<(), GateError> {
        self.apply_checked(proof).map_err(|_| GateError::ApplyFailed)
    }
    fn health_check(&mut self, _: &()) -> std::result::Result<(), GateError> {
        if self.mode == "post-failure" {
            user(&["--unknown"]).map_err(|_| GateError::ApplyFailed)?;
        }
        let marker = fs::read(format!("/etc/ryoku-issue20/{}", self.name))
            .map_err(|_| GateError::ApplyFailed)?;
        if marker != self.name.as_bytes() {
            return Err(GateError::ApplyFailed);
        }
        Ok(())
    }
    fn cancellation_requested(&self) -> bool {
        self.mode == "cancel"
    }
}
impl Provider {
    fn apply_checked(&self, proof: &VerifiedLinkedGeneration) -> Result<()> {
        let journal = journal()?;
        let context = journal.mutation_context(&operation(&self.name)).ok_or("no context")?;
        require(
            context.stage == MutationStage::Applying
                && context.checkpoint.as_ref() == Some(proof.linked()),
            "first mutation lacks durable exact proof",
        )?;
        let root = File::open("/")?;
        let reader = GenerationReader::open_state_mount()?;
        let linked = reader.open_historical_transaction(&root, &proof.linked().transaction_id)?;
        require(
            LinkedGeneration::from_verified_generation(linked.generation())
                .map_err(|e| format!("{e:?}"))?
                == *proof.linked(),
            "proof identity changed",
        )?;
        require(
            ProtectedSnapperCatalog::open(&root)?.status(&proof.linked().transaction_id)?.complete,
            "catalog incomplete before mutation",
        )?;
        let marker = format!("etc/ryoku-issue20/{}", self.name);
        let absent = rustix::fs::openat(
            linked.snapshot().descriptor(),
            marker.as_str(),
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        );
        require(
            matches!(absent, Err(rustix::io::Errno::NOENT)),
            "preimage already contains mutation marker",
        )?;
        require(
            !std::path::Path::new(&format!("/{marker}")).exists(),
            "mutation marker predates apply",
        )?;
        event("verified", &self.name)?;
        if self.mode == "writer" {
            let original = fs::read(format!("{CONFIG}/shell.json"))?;
            write_new(&format!("{FIXTURE}/writer/trigger"), b"go", 0o644)?;
            thread::sleep(Duration::from_millis(150));
            require(
                fs::read(format!("{CONFIG}/shell.json"))? == original,
                "account writer escaped retained capture lease",
            )?;
        }
        write_new(&format!("/{marker}"), self.name.as_bytes(), 0o600)?;
        event("mutated", &self.name)?;
        if self.mode == "interrupt-apply" {
            std::process::exit(74);
        }
        Ok(())
    }
}
fn transaction(name: &str, mode: &str) -> Result<()> {
    id(name)?;
    let peer = peer()?;
    let mut provider = Provider::new(name, mode);
    let planned = review_operator_plan(&peer, group()?, &mut provider)
        .map_err(|e| format!("review: {e:?}"))?;
    let bundle = ProfileBundleStore::open_state_mount()?.load(&bundle_id()?)?;
    let result = run_operator_mutation(
        &peer,
        group()?,
        NativeMutationRequest {
            operation_id: operation(name),
            action: ManagedActionKind::ApplyProfile,
            reason: OperationReason::Other(id("issue20-fixture")?),
            planned,
            created_utc: "2026-10-21T00:00:00Z".into(),
            packages: vec![package()],
            bundle: &bundle,
        },
        &mut provider,
    );
    println!("transaction={name} result={result:?}");
    match mode {
        "stale" | "changed-path" => {
            require(result == Err(GateError::PlanChanged), "stale plan unexpectedly accepted")
        }
        "capture-failure" => require(
            result == Err(GateError::CheckpointUnverified),
            "capture did not refuse invalid profile",
        ),
        "low-space" => {
            require(result == Err(GateError::InsufficientSpace), "native low-space did not block")
        }
        "cancel" => {
            require(result == Err(GateError::CancelledBeforeChange), "cancel did not block")
        }
        "post-failure" => require(result.is_err(), "post capture unexpectedly succeeded"),
        "blocked" => require(
            result == Err(GateError::RecoveryRequired),
            "unresolved operation did not block",
        ),
        _ => require(result.is_ok(), "native protected fixture mutation failed"),
    }
}
fn child(name: &str, mode: &str, exit: i32) -> Result<()> {
    let result =
        Command::new(std::env::current_exe()?).args(["--transaction", name, mode]).output()?;
    require(
        result.status.code() == Some(exit),
        &format!(
            "child {name}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        ),
    )
}
fn status(
    name: &str,
    stage: MutationStage,
    outcome: Option<MutationOutcome>,
    marker: bool,
) -> Result<()> {
    let history = ProtectedMutationHistory::open().map_err(|e| format!("history: {e:?}"))?;
    let context =
        history.journal().mutation_context(&operation(name)).ok_or("missing durable operation")?;
    require(
        context.stage == stage && context.outcome == outcome,
        "durable stage/outcome mismatch",
    )?;
    require(
        std::path::Path::new(&format!("/etc/ryoku-issue20/{name}")).exists() == marker,
        "mutation marker differs from expected boundary",
    )?;
    if stage == MutationStage::Complete {
        let root = File::open("/")?;
        let post = context.post_checkpoint.as_ref().ok_or("missing post image")?;
        let reader = GenerationReader::open_state_mount()?;
        let linked = reader.open_historical_transaction(&root, &post.transaction_id)?;
        let file = File::from(rustix::fs::openat(
            linked.snapshot().descriptor(),
            format!("etc/ryoku-issue20/{name}"),
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        )?);
        let mut bytes = Vec::new();
        file.take(1024).read_to_end(&mut bytes)?;
        require(bytes == name.as_bytes(), "native post image missing exact mutation")?;
    }
    Ok(())
}
fn prepare() -> Result<()> {
    fs::DirBuilder::new().mode(0o755).create(FIXTURE)?;
    let uid_text = Command::new("/usr/bin/id").args(["-u", "tester"]).output()?;
    require(uid_text.status.success(), "fixture account missing")?;
    let uid: u32 = String::from_utf8(uid_text.stdout)?.trim().parse()?;
    let gid = group()?;
    let runtime = format!("/run/user/{uid}");
    if !std::path::Path::new(&runtime).exists() {
        fs::DirBuilder::new().mode(0o700).create(&runtime)?;
        rustix::fs::chown(
            runtime.as_str(),
            Some(rustix::process::Uid::from_raw(uid)),
            Some(rustix::process::Gid::from_raw(gid)),
        )?;
    }
    let writer_directory = format!("{FIXTURE}/writer");
    fs::DirBuilder::new().mode(0o700).create(&writer_directory)?;
    rustix::fs::chown(
        writer_directory.as_str(),
        Some(rustix::process::Uid::from_raw(uid)),
        Some(rustix::process::Gid::from_raw(gid)),
    )?;
    fs::DirBuilder::new().mode(0o700).create("/etc/ryoku-issue20")?;
    user(&["--account-setup"])?;
    let peer = peer()?;
    let account =
        prepare_operator_account(&peer, group()?).map_err(|e| format!("account: {e:?}"))?;
    let session = NativeCaptureSession::open(account.capture_account()?, account.capture_home()?)?;
    // Check the actual systemd runtime boundary and retain the original lease
    // while root substitutes owned mounts/symlinks. Always restore those paths.
    let namespace = format!("{runtime}/ryoku-writers");
    for target in [&runtime, &namespace] {
        let mounted = Command::new("/usr/bin/mount").args(["--bind", target, target]).output()?;
        require(mounted.status.success(), "runtime substitution fixture mount failed")?;
        let refused = session.revalidate().is_err();
        let unmounted = Command::new("/usr/bin/umount").arg(target).output()?;
        require(
            unmounted.status.success() && refused,
            "runtime substituted/child mount was accepted",
        )?;
        session.revalidate()?;
    }
    let saved_namespace = format!("{runtime}/issue20-saved-writers");
    fs::rename(&namespace, &saved_namespace)?;
    std::os::unix::fs::symlink(&saved_namespace, &namespace)?;
    let refused = session.revalidate().is_err();
    fs::remove_file(&namespace)?;
    fs::rename(&saved_namespace, &namespace)?;
    require(refused, "symlink substituted writer namespace accepted")?;
    session.revalidate()?;
    let state = session.coordinator_directory()?;
    rustix::fs::mkdirat(state, "profiles", rustix::fs::Mode::from_raw_mode(0o700))?;
    let profiles = File::open(format!("{STATE}/profiles"))?;
    rustix::fs::mkdirat(&profiles, ".staging", rustix::fs::Mode::from_raw_mode(0o700))?;
    profiles.sync_all()?;
    state.sync_all()?;
    let bundle = InstalledProfileBundle::from_local_documents(
        DefaultsPackage {
            name: PackageName::new("issue20-local-fixture").unwrap(),
            nevra: Revision::new("fixture-1").unwrap(),
            upstream_revision: None,
        },
        br#"{"schema_version":1,"id":"generic","settings":{}}"#,
        None,
        None,
    )?;
    let published = ProfileBundleStore::open_state_mount()?
        .publish_candidate(&id("issue20-input")?, &bundle)?;
    write_new(&format!("{STATE}/issue20-bundle"), published.id().as_str().as_bytes(), 0o600)?;
    ProtectedSnapperCatalog::initialize(&File::open("/")?)?;
    Ok(())
}
fn low_space() -> Result<()> {
    let path = format!("{STATE}/issue20-space-fill");
    let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    let mut offset = 0;
    loop {
        let allocated = rustix::fs::fallocate(
            &file,
            rustix::fs::FallocateFlags::empty(),
            offset,
            32 * 1024 * 1024,
        );
        if allocated == Err(rustix::io::Errno::NOSPC) {
            // Leave a small amount for the failure record, while an actual
            // maximum checkpoint allocation must still fail.
            if offset >= 4 * 1024 * 1024 {
                rustix::fs::fallocate(
                    &file,
                    rustix::fs::FallocateFlags::PUNCH_HOLE | rustix::fs::FallocateFlags::KEEP_SIZE,
                    offset - 4 * 1024 * 1024,
                    4 * 1024 * 1024,
                )?;
            }
            break;
        }
        allocated?;
        offset += 32 * 1024 * 1024;
        require(offset < 24 * 1024 * 1024 * 1024, "space fixture exceeded owned disk bound")?;
    }
    file.sync_all()?;
    let state = File::open(STATE)?;
    let allocation = rustix::fs::openat(
        &state,
        ".",
        rustix::fs::OFlags::TMPFILE | rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::from_raw_mode(0o600),
    )?;
    require(
        rustix::fs::fallocate(
            &allocation,
            rustix::fs::FallocateFlags::empty(),
            0,
            128 * 1024 * 1024,
        ) == Err(rustix::io::Errno::NOSPC),
        "unnamed checkpoint allocation did not detect real pressure",
    )?;
    drop(allocation);
    let result = child("issue20-low-space", "low-space", 0);
    fs::remove_file(path)?;
    drop(file);
    rustix::fs::syncfs(&File::open(STATE)?)?;
    result?;
    status(
        "issue20-low-space",
        MutationStage::StateChecking,
        Some(MutationOutcome::FailedBeforeChange),
        false,
    )
}
fn exercise(interrupt: bool) -> Result<()> {
    prepare()?;
    println!("native_issue20_runtime_boundary=passed");
    low_space()?;
    println!("native_issue20_low_space_blocks=passed");
    child("issue20-cancel", "cancel", 0)?;
    status(
        "issue20-cancel",
        MutationStage::Preflight,
        Some(MutationOutcome::CancelledBeforeChange),
        false,
    )?;
    println!("native_issue20_cancel_blocks=passed");
    user(&["--invalid-profile"])?;
    child("issue20-capture", "capture-failure", 0)?;
    status(
        "issue20-capture",
        MutationStage::Capturing,
        Some(MutationOutcome::FailedBeforeChange),
        false,
    )?;
    fs::remove_file(format!("{CONFIG}/opensuse-profile.json"))?;
    println!("native_issue20_capture_error_blocks=passed");
    child("issue20-stale", "stale", 0)?;
    status(
        "issue20-stale",
        MutationStage::Revalidating,
        Some(MutationOutcome::FailedBeforeChange),
        false,
    )?;
    println!("native_issue20_changed_plan_blocks=passed");
    child("issue20-changed-path", "changed-path", 0)?;
    status(
        "issue20-changed-path",
        MutationStage::Revalidating,
        Some(MutationOutcome::FailedBeforeChange),
        false,
    )?;
    println!("native_issue20_changed_path_blocks=passed");
    child("issue20-interrupted-before", "interrupt-before", 73)?;
    status("issue20-interrupted-before", MutationStage::Revalidating, None, false)?;
    child("issue20-blocked-before", "blocked", 0)?;
    let console = Command::new("/usr/local/sbin/ryoku-mutation-history")
        .args(["reconcile-before-change", "issue20-interrupted-before"])
        .output()?;
    require(
        console.status.success(),
        "independent console could not seal interrupted prechange failure",
    )?;
    println!("native_issue20_interruption_before_apply=passed");
    // Two independent helper processes contend on the actual machine lock.
    let first = Command::new(std::env::current_exe()?)
        .args(["--transaction", "issue20-concurrent-first", "success"])
        .stdout(Stdio::null())
        .spawn()?;
    let second = Command::new(std::env::current_exe()?)
        .args(["--transaction", "issue20-concurrent-second", "success"])
        .stdout(Stdio::null())
        .spawn()?;
    for mut process in [first, second] {
        require(process.wait()?.success(), "concurrent transaction failed")?;
    }
    for name in ["issue20-concurrent-first", "issue20-concurrent-second"] {
        status(name, MutationStage::Complete, None, true)?;
    }
    let records: Vec<serde_json::Value> = fs::read_to_string(format!("{STATE}/mutations.log"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    let identities: Vec<_> = records
        .iter()
        .filter_map(|r| r["status"]["operation_id"].as_str())
        .filter(|name| name.starts_with("issue20-concurrent"))
        .collect();
    require(
        identities.len() == 16
            && identities[..8].iter().all(|id| *id == identities[0])
            && identities[8..].iter().all(|id| *id == identities[8])
            && identities[0] != identities[8],
        "native phases interleaved",
    )?;
    println!("native_issue20_concurrent_generations=passed");
    // Use the real native wrapper with a fixture child at its fixed payload
    // path. The child attempts one enrolled write while the root marker changes.
    fs::create_dir_all("/usr/libexec/ryoku/upstream")?;
    fs::copy(std::env::current_exe()?, "/usr/libexec/ryoku/upstream/ryoku-shell")?;
    let mut writer = Command::new("/usr/sbin/runuser")
        .args(["-u", "tester", "--"])
        .arg(std::env::current_exe()?)
        .arg("--wrapper")
        .env("HOME", HOME)
        .stdout(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::path::Path::new(&format!("{FIXTURE}/writer/ready")).exists() {
        require(Instant::now() < deadline, "writer never started")?;
        thread::sleep(Duration::from_millis(10));
    }
    child("issue20-writer", "writer", 0)?;
    require(writer.wait()?.success(), "writer did not resume after lease release")?;
    require(
        fs::read(format!("{CONFIG}/shell.json"))? == b"{\"after_lease\":true}\n",
        "writer preference change absent after thaw",
    )?;
    status("issue20-writer", MutationStage::Complete, None, true)?;
    println!("native_issue20_retained_writer_lease=passed");
    let mode = if interrupt { "interrupt-apply" } else { "post-failure" };
    child("issue20-final", mode, if interrupt { 74 } else { 0 })?;
    status(
        "issue20-final",
        if interrupt { MutationStage::Applying } else { MutationStage::PostChangeCapture },
        if interrupt { None } else { Some(MutationOutcome::PostChangeCaptureFailed) },
        true,
    )?;
    if !interrupt {
        fs::remove_file(format!("{CONFIG}/unknown.fixture"))?;
    }
    child("issue20-blocked-after", "blocked", 0)?;
    let refused = Command::new("/usr/local/sbin/ryoku-mutation-history")
        .args(["reconcile-before-change", "issue20-final"])
        .output()?;
    require(!refused.status.success(), "console cleared potentially mutating history")?;
    println!(
        "native_issue20_{}=passed",
        if interrupt { "interrupted_apply" } else { "post_capture_failure" }
    );
    verify(interrupt)
}
fn verify(interrupt: bool) -> Result<()> {
    let history = ProtectedMutationHistory::open().map_err(|e| format!("{e:?}"))?;
    require(history.journal().recovery_required().len() == 1, "wrong unresolved native history")?;
    drop(history);
    // Execute the same protected startup gate used by the installed helper.
    drop(ryoku_service::protected_state::ProtectedState::open()?);
    status(
        "issue20-final",
        if interrupt { MutationStage::Applying } else { MutationStage::PostChangeCapture },
        if interrupt { None } else { Some(MutationOutcome::PostChangeCaptureFailed) },
        true,
    )?;
    let console = Command::new("/usr/local/sbin/ryoku-mutation-history").arg("status").output()?;
    require(
        console.status.success()
            && String::from_utf8(console.stdout)?.lines().last() == Some("recovery_required=1"),
        "independent console history differs",
    )?;
    let events = fs::read_to_string(format!("{STATE}/issue20-events"))?;
    let lines: Vec<_> = events.lines().collect();
    require(
        lines.len() == 8
            && lines
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| pair[0].strip_prefix("verified:") == pair[1].strip_prefix("mutated:")),
        "first mutation occurred before verified native capture",
    )?;
    println!("native_issue20_first_mutation_order=passed");
    println!("native_issue20_console_history=passed");
    println!("native_issue20_provider_capabilities=disabled");
    println!("native_issue20_multisubvolume_atomicity=unclaimed");
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    require(
        fs::read_to_string("/etc/ryoku-lab-fixture")? == "disposable-issue13\n"
            && fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() == "QEMU",
        "requires seeded disposable QEMU fixture",
    )?;
    if std::env::current_exe()?.file_name().is_some_and(|name| name == "ryoku-shell") {
        write_new(&format!("{FIXTURE}/writer/ready"), b"ready", 0o600)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !std::path::Path::new(&format!("{FIXTURE}/writer/trigger")).exists() {
            require(Instant::now() < deadline, "writer trigger absent")?;
            thread::sleep(Duration::from_millis(10));
        }
        let mut file =
            OpenOptions::new().write(true).truncate(true).open(format!("{CONFIG}/shell.json"))?;
        file.write_all(b"{\"after_lease\":true}\n")?;
        file.sync_all()?;
        return Ok(());
    }
    match args.as_slice() {
        [mode] if mode == "--account-setup" => {
            fs::create_dir_all(CONFIG)?;
            fs::set_permissions(CONFIG, fs::Permissions::from_mode(0o700))?;
            write_new(&format!("{CONFIG}/shell.json"), b"{}\n", 0o600)?;
            initialize_writer_namespace()?;
        }
        [mode] if mode == "--changed-shell" => {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(format!("{CONFIG}/shell.json"))?;
            file.write_all(b"{\"external_change\":true}\n")?;
            file.sync_all()?;
        }
        [mode] if mode == "--invalid-profile" => {
            write_new(&format!("{CONFIG}/opensuse-profile.json"), b"not json", 0o600)?
        }
        [mode] if mode == "--unknown" => {
            write_new(&format!("{CONFIG}/unknown.fixture"), b"unreviewed", 0o600)?
        }
        [mode] if mode == "--wrapper" => {
            std::process::exit(run_writer(WriterKind::Daemon, &[])?);
        }
        [mode, path] if mode == "--client" && path.starts_with(FIXTURE) => {
            let _stream = UnixStream::connect(path)?;
        }
        _ => {
            require(rustix::process::geteuid().as_raw() == 0, "root fixture mode required")?;
            match args.as_slice() {
                [mode] if mode == "--exercise" => exercise(false)?,
                [mode] if mode == "--interrupt" => exercise(true)?,
                [mode] if mode == "--verify" => verify(false)?,
                [mode] if mode == "--verify-interrupt" => verify(true)?,
                [mode, name, kind] if mode == "--transaction" => transaction(name, kind)?,
                _ => return Err("unsupported fixture mode".into()),
            }
        }
    }
    Ok(())
}
