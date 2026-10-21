//! Offline administrator lifecycle. RPM executes the exact locally reviewed
//! package set; dependencies must already be installed through distro policy.
//! Recovery tools remain installed after desktop removal.
use super::*;
use crate::{
    ManagedActionKind, OperationId,
    mutation_gate::{
        self, CheckpointEvidence, GateError, JournaledOperation, LinkedGeneration,
        MutationIntegration, PlannedChange, VerifiedLinkedGeneration, VerifiedPostChangeGeneration,
    },
};
use ryoku_recovery::{
    Identifier, SnapshotLink,
    account_principal::AccountPrincipal,
    capture_session::NativeCaptureSession,
    generation::GenerationReader,
    installed_capture::{CaptureAccount, InstalledCaptureRequest},
    installed_profiles::{InstalledProfileBundle, ProfileBundleStore},
    native_checkpoint::{verify_historical_root_checkpoint, verify_new_root_checkpoint},
    snapper_catalog::ProtectedSnapperCatalog,
};
use std::ffi::{OsStr, OsString};

const RETAINED: &[&str] = &["ryoku-tools", "ryoku-tools-state-init"];
const POLICY_FILES: &[&str] = &[
    "/etc/os-release",
    "/etc/default/grub",
    "/etc/sysconfig/bootloader",
    "/etc/selinux/config",
    "/etc/sysconfig/security",
    "/etc/sysconfig/displaymanager",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Install,
    Remove,
    Migrate,
}
impl Action {
    fn parse(value: &OsStr) -> Result<Self, String> {
        match value.to_str() {
            Some("install") => Ok(Self::Install),
            Some("remove") => Ok(Self::Remove),
            Some("migrate") => Ok(Self::Migrate),
            _ => Err("unknown installer action".into()),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Remove => "remove",
            Self::Migrate => "migrate",
        }
    }
    fn managed(self) -> ManagedActionKind {
        match self {
            Self::Install => ManagedActionKind::InstallPackage,
            Self::Remove => ManagedActionKind::RemovePackage,
            Self::Migrate => ManagedActionKind::ApplyProfile,
        }
    }
}
struct Account {
    uid: u32,
    gid: u32,
    name: String,
    home_path: String,
    home: File,
}
impl Account {
    fn resolve(uid: u32) -> Result<Self, String> {
        if !(1000..65534).contains(&uid) {
            return Err("ordinary target account required".into());
        }
        let reply = bounded_command(
            "/usr/bin/getent",
            &[OsStr::new("passwd"), OsStr::new(&uid.to_string())],
            5,
            16 * 1024,
        )?;
        if !reply.status.success() {
            return Err("target account lookup failed".into());
        }
        let text = std::str::from_utf8(&reply.stdout).map_err(|_| "invalid account record")?;
        let lines: Vec<_> = text.lines().collect();
        if lines.len() != 1 {
            return Err("ambiguous target account".into());
        }
        let parts: Vec<_> = lines[0].split(':').collect();
        if parts.len() != 7
            || parts[2].parse::<u32>().ok() != Some(uid)
            || !is_canonical_absolute_path(parts[5])
            || parts[5] == "/"
        {
            return Err("unsupported target account".into());
        }
        let gid = parts[3].parse::<u32>().map_err(|_| "invalid account group")?;
        let root = File::open("/").map_err(|e| e.to_string())?;
        let home = File::from(
            openat2(
                &root,
                parts[5].trim_start_matches('/'),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(|e| e.to_string())?,
        );
        let stat = fstat(&home).map_err(|e| e.to_string())?;
        if stat.st_uid != uid || stat.st_gid != gid || stat.st_mode & 0o7022 != 0 {
            return Err("unsafe target home".into());
        }
        Ok(Self { uid, gid, name: parts[0].into(), home_path: parts[5].into(), home })
    }
    fn capture(&self) -> Result<CaptureAccount, String> {
        Ok(CaptureAccount {
            uid: self.uid,
            gid: self.gid,
            principal: AccountPrincipal::from_account(
                self.uid,
                self.gid,
                &self.name,
                &self.home_path,
            )
            .map_err(|e| format!("{e:?}"))?,
        })
    }
    fn revalidate(&self) -> Result<(), String> {
        let current = Self::resolve(self.uid)?;
        let old = fstat(&self.home).map_err(|e| e.to_string())?;
        let new = fstat(&current.home).map_err(|e| e.to_string())?;
        if (self.gid, &self.name, &self.home_path, old.st_dev, old.st_ino)
            != (current.gid, &current.name, &current.home_path, new.st_dev, new.st_ino)
        {
            return Err("account changed".into());
        }
        Ok(())
    }
}

pub(super) fn execute(args: &[OsString], output: &mut impl Write) -> Result<(), String> {
    if args.first().is_some_and(|arg| arg == "internal-lifecycle") {
        return execute_owned(&args[1..], output);
    }
    if args.first().is_some_and(|arg| arg == "lifecycle-preview") {
        return execute_owned(args, output);
    }
    if args[0] == "recover" {
        return recover(args, output);
    }
    if args.len() < 5 {
        return Err("usage: ACTION UID OPERATION LAYERS REVIEWED_SHA256 [RPM ...]".into());
    }
    let operation = OperationId::new(args[2].to_str().ok_or("invalid operation ID")?.to_owned())
        .map_err(|_| "invalid operation ID")?;
    let unit = format!("ryoku-installer-{}.service", operation.as_str());
    let status = Command::new("/usr/bin/systemd-run")
        .env_clear()
        .env("LC_ALL", "C")
        .args([
            "--wait",
            "--collect",
            "--service-type=exec",
            "--property=KillMode=control-group",
            "--property=TimeoutStartSec=1800",
            "--property=OnFailure=emergency.target",
            "--unit",
            &unit,
            "/usr/sbin/ryoku-install",
            "internal-lifecycle",
        ])
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "installer unit {unit} failed; inspect journalctl -u {unit} and protected mutation history"
        ));
    }
    writeln!(output, "installer_unit={unit} result=success").map_err(|e| e.to_string())?;
    Ok(())
}
fn execute_owned(args: &[OsString], output: &mut impl Write) -> Result<(), String> {
    if args.first().is_some_and(|arg| arg == "recover") {
        return recover(args, output);
    }
    let preview = args[0] == "lifecycle-preview";
    let offset = usize::from(preview);
    if args.len() < offset + if preview { 3 } else { 5 } {
        return Err("usage: lifecycle-preview ACTION UID LAYERS [RPM ...] | ACTION UID OPERATION LAYERS REVIEWED_SHA256 [RPM ...]; LAYERS is none or an exact retained bundle SHA256".into());
    }
    let action = Action::parse(&args[offset])?;
    let uid = args[offset + 1].to_str().ok_or("invalid UID")?.parse().map_err(|_| "invalid UID")?;
    let (op, layers_arg, expected, paths) = if preview {
        ("installer-preview".to_owned(), &args[offset + 2], None, &args[offset + 3..])
    } else {
        (
            args[2].to_str().ok_or("invalid operation ID")?.to_owned(),
            &args[3],
            Some(args[4].to_str().ok_or("invalid reviewed digest")?),
            &args[5..],
        )
    };
    let operation = OperationId::new(op.clone()).map_err(|_| "invalid operation ID")?;
    // Reserve suffix room for independent pre/post generation identities.
    Identifier::new(format!("{op}-post-generation")).map_err(|_| "operation ID too long")?;
    require_console()?;
    validate_host()?;
    let account = Account::resolve(uid)?;
    let session = NativeCaptureSession::open(
        account.capture()?,
        account.home.try_clone().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let layer_bundle = if layers_arg == "none" {
        None
    } else {
        let id = ryoku_recovery::Sha256::new(layers_arg.to_str().ok_or("invalid layers digest")?)
            .map_err(|_| "invalid layers digest")?;
        Some(
            ProfileBundleStore::open_state_mount()
                .and_then(|store| store.load(&id))
                .map_err(|e| e.to_string())?,
        )
    };
    let layers = layer_bundle.as_ref().map(|bundle| bundle.bundle());
    let staged = if action == Action::Install {
        Some(validate_inputs(paths)?)
    } else {
        if !paths.is_empty() {
            return Err("RPM inputs only apply to installation".into());
        }
        None
    };
    let packages = staged
        .as_ref()
        .map(|inputs| validate_packages(&inputs.inputs))
        .transpose()?
        .unwrap_or_default();
    let baseline = installed_ryoku_packages()?;
    let policy = policy_snapshot()?;
    let mut provider = Installer {
        action,
        account,
        session: &session,
        staged,
        packages,
        baseline,
        policy,
        layers,
        operation: op.clone(),
        reviewed: None,
    };
    let planned = provider.plan()?;
    if preview {
        writeln!(output, "installer_action={} target_uid={uid} plan_sha256={} affected_state_sha256={} layers={}\npackage_executor=rpm-exact-set dependency_changes=refused fallback=existing-kde recovery_tools=retained", action.name(), planned.plan_fingerprint, planned.affected_state_fingerprint, layers_arg.to_string_lossy()).map_err(|e| e.to_string())?;
        for package in &provider.packages {
            writeln!(
                output,
                "package={} current={} target={} source_sha256={}",
                package.name,
                provider.baseline.get(&package.name).map(String::as_str).unwrap_or("absent"),
                package.nevra,
                package.source_sha256.as_deref().unwrap_or("none")
            )
            .map_err(|e| e.to_string())?;
        }
        for package in &provider.packages {
            for path in &package.payload_paths {
                writeln!(
                    output,
                    "resolved_payload package={} path={} ownership=validated",
                    package.name, path
                )
                .map_err(|e| e.to_string())?;
            }
            for path in &package.config_paths {
                writeln!(output, "resolved_config package={} path={}", package.name, path)
                    .map_err(|e| e.to_string())?;
            }
        }
        if action == Action::Remove {
            for (name, nevra) in provider.removal_set() {
                writeln!(output, "remove_package={name} installed={nevra}")
                    .map_err(|e| e.to_string())?;
            }
        }
        if action == Action::Migrate {
            let (_, bytes) = migration_bytes(&provider.account)?;
            writeln!(output,"migration_rule=canonical-personal-v1 source=profile.json source_sha256={:x} destination=opensuse-profile.json destination_mode=0600 source=retained",Sha256::digest(bytes)).map_err(|e|e.to_string())?;
        }
        writeln!(output,"configuration_effect=preserve-personal-system-companion-and-unrelated-data distro_packages=unchanged").map_err(|e|e.to_string())?;
        writeln!(output, "mutation=not_performed checkpoint=not_created")
            .map_err(|e| e.to_string())?;
        session.finish().map_err(|e| e.to_string())?;
        return Ok(());
    }
    if Some(planned.plan_fingerprint.as_str()) != expected {
        return Err("reviewed plan changed; run lifecycle-preview again".into());
    }
    provider.reviewed = Some(planned.clone());
    let mut journal = crate::native_mutation::open_journal(
        session.coordinator_directory().map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut operation = JournaledOperation::new(&mut journal, operation, action.managed());
    mutation_gate::coordinate_mutation_locked(&planned, &mut operation, &mut provider).map_err(|e| format!("installer stopped: {e:?}; retain checkpoints and inspect protected mutation history before recovery"))?;
    drop(provider);
    session.finish().map_err(|e| e.to_string())?;
    writeln!(
        output,
        "installer_complete action={} operation={} pre_checkpoint={}-pre post_checkpoint={}-post",
        action.name(),
        op,
        op,
        op
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn require_console() -> Result<(), String> {
    for unit in ["graphical.target", "display-manager.service"] {
        let result = bounded_command(
            "/usr/bin/systemctl",
            &[OsStr::new("is-active"), OsStr::new(unit)],
            10,
            4096,
        )?;
        if result.status.success() {
            return Err("stop graphical login and run from a root recovery console".into());
        }
    }
    Ok(())
}
fn policy_snapshot() -> Result<String, String> {
    let mut hash = Sha256::new();
    for path in POLICY_FILES {
        hash.update(path.as_bytes());
        match fs::read(path) {
            Ok(bytes) => {
                if bytes.len() > 1024 * 1024 {
                    return Err("oversized distro policy".into());
                }
                hash.update([1]);
                hash.update(bytes);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => hash.update([0]),
            Err(e) => return Err(e.to_string()),
        }
    }
    let kde = [
        "/usr/share/wayland-sessions/plasma.desktop",
        "/usr/share/xsessions/plasma.desktop",
        "/usr/share/xsessions/plasmax11.desktop",
    ];
    let mut found = false;
    for path in kde {
        if let Ok(bytes) = fs::read(path) {
            found = true;
            hash.update(path.as_bytes());
            hash.update(bytes);
        }
    }
    if !found {
        return Err("existing KDE fallback session is required".into());
    }
    let inventory = bounded_command(
        "/usr/bin/rpm",
        &[OsStr::new("-qa"), OsStr::new("--qf"), OsStr::new("%{NAME}\t%{NEVRA}\n")],
        30,
        MAX_QUERY_BYTES,
    )?;
    if !inventory.status.success() {
        return Err("distro package inventory unavailable".into());
    }
    let text = std::str::from_utf8(&inventory.stdout).map_err(|_| "invalid distro inventory")?;
    let mut lines: Vec<_> = text.lines().filter(|line| !line.starts_with("ryoku-")).collect();
    lines.sort();
    for line in lines {
        hash.update(line.as_bytes());
        hash.update(b"\n");
    }
    Ok(format!("{:x}", hash.finalize()))
}
struct Installer<'a> {
    action: Action,
    account: Account,
    session: &'a NativeCaptureSession,
    staged: Option<StagedPackageInputs>,
    packages: Vec<Package>,
    baseline: BTreeMap<String, String>,
    policy: String,
    layers: Option<&'a InstalledProfileBundle>,
    operation: String,
    reviewed: Option<PlannedChange>,
}
impl Installer<'_> {
    fn plan(&self) -> Result<PlannedChange, String> {
        self.account.revalidate()?;
        if installed_ryoku_packages()? != self.baseline || policy_snapshot()? != self.policy {
            return Err("package or distro baseline changed".into());
        }
        ryoku_recovery::installer_state::from_installed(&self.baseline)?;
        let mut hash = Sha256::new();
        hash.update(self.action.name());
        hash.update(self.account.uid.to_le_bytes());
        hash.update(&self.policy);
        hash.update(serde_json::to_vec(&self.baseline).map_err(|e| e.to_string())?);
        hash.update(
            self.layers
                .map(|bundle| bundle.id().map(|id| id.as_str().to_owned()))
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_else(|| "none".into()),
        );
        match self.action {
            Action::Install => {
                let current = validate_packages(
                    &self.staged.as_ref().ok_or("missing staged inputs")?.inputs,
                )?;
                let mut target = self.baseline.clone();
                for package in &current {
                    target.insert(package.name.clone(), package.nevra.clone());
                }
                ryoku_recovery::installer_state::from_installed(&target)?;
                let ownership = validate_payload_file_conflicts(&current)?;
                let dirs = validate_payload_directory_types(&current)?;
                hash.update(install_plan_sha256(
                    &self.baseline,
                    &current,
                    b"exact-rpm-transaction-v1",
                    &format!("{ownership}:{dirs}"),
                ));
                self.rpm(true)?;
                for input in &self.staged.as_ref().ok_or("missing staged inputs")?.inputs {
                    verify_signature(&input.path)?;
                }
            }
            Action::Remove => {
                let selected = self.removal_set();
                for (name, nevra) in &selected {
                    let package = inspect_installed_package(name, nevra)?;
                    removal_payload_ownership(&[package], &installed_file_owners()?)?;
                }
                if !selected.is_empty() {
                    self.rpm(true)?;
                }
                hash.update(serde_json::to_vec(&selected).map_err(|e| e.to_string())?);
            }
            Action::Migrate => {
                hash.update(migration_bytes(&self.account)?.1);
            }
        }
        Ok(PlannedChange {
            operation_key: format!("installer-{}-v1", self.action.name()),
            plan_fingerprint: format!("{:x}", hash.finalize()),
            affected_state_fingerprint: self
                .session
                .inventory_fingerprint()
                .map_err(|e| e.to_string())?
                .as_str()
                .into(),
        })
    }
    fn removal_set(&self) -> BTreeMap<String, String> {
        self.baseline
            .iter()
            .filter(|(name, _)| !RETAINED.contains(&name.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
    fn rpm(&self, dry: bool) -> Result<(), String> {
        let mut arguments: Vec<OsString> = match self.action {
            Action::Install => vec!["-U".into(), "--replacepkgs".into()],
            Action::Remove => vec!["-e".into()],
            Action::Migrate => return Err("migration is not a package transaction".into()),
        };
        if dry {
            arguments.push("--test".into());
        }
        match self.action {
            Action::Install => arguments.extend(
                self.staged
                    .as_ref()
                    .ok_or("missing staged RPMs")?
                    .inputs
                    .iter()
                    .map(|input| input.path.clone().into_os_string()),
            ),
            Action::Remove => arguments.extend(self.removal_set().keys().map(OsString::from)),
            Action::Migrate => {}
        }
        let references: Vec<_> = arguments.iter().map(OsString::as_os_str).collect();
        let result = bounded_command(
            "/usr/bin/rpm",
            &references,
            if dry { 120 } else { 900 },
            MAX_SOLVER_BYTES,
        )?;
        if !result.status.success() {
            return Err(format!(
                "exact RPM transaction failed: {} {}",
                sanitize(&result.stdout),
                sanitize(&result.stderr)
            ));
        }
        Ok(())
    }
    fn capture(&self, post: bool) -> Result<CheckpointEvidence, String> {
        self.account.revalidate()?;
        self.session.revalidate().map_err(|e| e.to_string())?;
        let root = File::open("/").map_err(|e| e.to_string())?;
        let installed = installed_ryoku_packages()?;
        let system = self.layers.and_then(InstalledProfileBundle::system_source_bytes);
        let companion = self.layers.and_then(InstalledProfileBundle::companion_source_bytes);
        let bundle = if installed.contains_key("ryoku-profile-defaults") {
            InstalledProfileBundle::load_packaged_defaults(system, companion)
        } else {
            match fs::symlink_metadata(ryoku_recovery::rpm_defaults::DEFAULTS_PATH) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                _ => return Err("unowned defaults path prevents absent-default capture".into()),
            }
            InstalledProfileBundle::without_installed_defaults(system, companion)
        }
        .map_err(|e| e.to_string())?;
        let revisions = ryoku_recovery::installer_state::from_installed(&installed)?;
        if !bundle.matches_packages(&revisions) {
            return Err("profile/defaults package mismatch".into());
        }
        let suffix = if post { "post" } else { "pre" };
        let transaction = Identifier::new(format!("{}-{suffix}", self.operation))
            .map_err(|e| format!("{e:?}"))?;
        let retained = self
            .session
            .publish_installer_bundle(&transaction, &bundle)
            .map_err(|e| e.to_string())?;
        let machine = read_machine_state_record(&root).map_err(|e| e.to_string())?;
        let snapshot = ryoku_snapper::SnapperCheckpointBackend
            .create_single(&ryoku_snapper::SnapshotPairRequest {
                config: ryoku_snapper::ConfigName::new("root").map_err(|e| format!("{e:?}"))?,
                transaction_id: ryoku_snapper::TransactionId::new(transaction.as_str())
                    .map_err(|e| format!("{e:?}"))?,
            })
            .map_err(|e| e.to_string())?;
        let native = verify_new_root_checkpoint(
            &root,
            SnapshotLink::new(
                Identifier::new("root").map_err(|e| format!("{e:?}"))?,
                snapshot.as_str(),
                &machine.identity().filesystem_uuid,
            )
            .map_err(|e| format!("{e:?}"))?,
        )
        .map_err(|e| e.to_string())?;
        self.session
            .capture(
                InstalledCaptureRequest {
                    transaction_id: transaction.clone(),
                    generation_id: Identifier::new(format!(
                        "{}-{suffix}-generation",
                        self.operation
                    ))
                    .map_err(|e| format!("{e:?}"))?,
                    parent_generation_id: None,
                    provider_revision: None,
                    limits: Default::default(),
                    snapshot: native.verified_link().clone(),
                    created_utc: timestamp()?,
                    package_revisions: revisions,
                    reason: ryoku_recovery::installer_state::reason(),
                },
                &retained,
            )
            .map_err(|e| e.to_string())?;
        if installed_ryoku_packages()? != installed {
            return Err("packages changed during checkpoint".into());
        }
        verify_historical_root_checkpoint(&root, native.receipt()).map_err(|e| e.to_string())?;
        let reader = GenerationReader::open_state_mount().map_err(|e| e.to_string())?;
        let linked =
            reader.open_historical_transaction(&root, &transaction).map_err(|e| e.to_string())?;
        self.session.verify_inventory(linked.generation().manifest()).map_err(|e| e.to_string())?;
        let catalog = ProtectedSnapperCatalog::open(&root).map_err(|e| e.to_string())?;
        catalog.publish(&transaction, &[]).map_err(|e| e.to_string())?;
        catalog.mark_manual(&transaction).map_err(|e| e.to_string())?;
        if !catalog.status(&transaction).map_err(|e| e.to_string())?.complete {
            return Err("incomplete catalog partner".into());
        }
        Ok(CheckpointEvidence {
            linked: LinkedGeneration::from_verified_generation(linked.generation())
                .map_err(|e| format!("{e:?}"))?,
            system_component_verified: true,
            ryoku_component_verified: true,
            contents_and_metadata_verified: true,
        })
    }
}
impl MutationIntegration for Installer<'_> {
    type Output = ();
    fn preflight(&mut self, plan: &PlannedChange) -> Result<(), GateError> {
        require_console().map_err(|_| GateError::PreflightFailed)?;
        validate_host().map_err(|_| GateError::PreflightFailed)?;
        if self.plan().map_err(|_| GateError::PreflightFailed)? != *plan {
            return Err(GateError::PlanChanged);
        }
        Ok(())
    }
    fn state_space_check(&mut self, _: &PlannedChange) -> Result<(), GateError> {
        self.session.ensure_checkpoint_space().map_err(|_| GateError::InsufficientSpace)
    }
    fn capture(&mut self, _: &PlannedChange) -> Result<CheckpointEvidence, GateError> {
        Installer::capture(self, false).map_err(|_| GateError::CheckpointUnverified)
    }
    fn revalidate(
        &mut self,
        plan: &PlannedChange,
        proof: &VerifiedLinkedGeneration,
    ) -> Result<bool, GateError> {
        let root = File::open("/").map_err(|_| GateError::PreflightFailed)?;
        let reader =
            GenerationReader::open_state_mount().map_err(|_| GateError::CheckpointUnverified)?;
        let linked = reader
            .open_historical_transaction(&root, &proof.linked().transaction_id)
            .map_err(|_| GateError::CheckpointUnverified)?;
        self.session
            .verify_inventory(linked.generation().manifest())
            .map_err(|_| GateError::PlanChanged)?;
        Ok(self.plan().map_err(|_| GateError::PlanChanged)? == *plan
            && LinkedGeneration::from_verified_generation(linked.generation())? == *proof.linked())
    }
    fn apply(&mut self, proof: &VerifiedLinkedGeneration) -> Result<(), GateError> {
        let plan = self.reviewed.clone().ok_or(GateError::InvalidPlan)?;
        if !self.revalidate(&plan, proof)? {
            return Err(GateError::PlanChanged);
        }
        match self.action {
            Action::Install => self.rpm(false),
            Action::Remove if self.removal_set().is_empty() => Ok(()),
            Action::Remove => self.rpm(false),
            Action::Migrate => migrate(&self.account),
        }
        .map_err(|_| GateError::ApplyFailed)
    }
    fn health_check(&mut self, _: &(), _: &VerifiedLinkedGeneration) -> Result<(), GateError> {
        self.account.revalidate().map_err(|_| GateError::PostChangeFailed)?;
        self.session.revalidate().map_err(|_| GateError::PostChangeFailed)?;
        let root = File::open("/").map_err(|_| GateError::PostChangeFailed)?;
        let reader =
            GenerationReader::open_state_mount().map_err(|_| GateError::PostChangeFailed)?;
        let before = reader
            .open_historical_transaction(
                &root,
                &Identifier::new(format!("{}-pre", self.operation))
                    .map_err(|_| GateError::PostChangeFailed)?,
            )
            .map_err(|_| GateError::PostChangeFailed)?;
        if self.action != Action::Migrate {
            self.session
                .verify_inventory(before.generation().manifest())
                .map_err(|_| GateError::PostChangeFailed)?;
        } else {
            self.session
                .verify_installer_migration_inventory(before.generation().manifest())
                .map_err(|_| GateError::PostChangeFailed)?;
        }
        if policy_snapshot().map_err(|_| GateError::PostChangeFailed)? != self.policy {
            return Err(GateError::PostChangeFailed);
        }
        let current = installed_ryoku_packages().map_err(|_| GateError::PostChangeFailed)?;
        let mut expected = self.baseline.clone();
        match self.action {
            Action::Install => {
                for package in &self.packages {
                    expected.insert(package.name.clone(), package.nevra.clone());
                }
            }
            Action::Remove => expected.retain(|name, _| RETAINED.contains(&name.as_str())),
            Action::Migrate => {
                migration_bytes(&self.account).map_err(|_| GateError::PostChangeFailed)?;
            }
        }
        if current != expected {
            return Err(GateError::PostChangeFailed);
        }
        Ok(())
    }
    fn capture_post_change(
        &mut self,
        _: &(),
        before: &VerifiedLinkedGeneration,
    ) -> Result<VerifiedPostChangeGeneration, GateError> {
        VerifiedPostChangeGeneration::validate(
            Installer::capture(self, true).map_err(|_| GateError::PostChangeFailed)?,
            before,
        )
    }
}
fn verify_signature(path: &std::path::Path) -> Result<(), String> {
    let result = bounded_command(
        "/usr/bin/rpm",
        &[OsStr::new("--checksig"), path.as_os_str()],
        30,
        16 * 1024,
    )?;
    let text = String::from_utf8_lossy(&result.stdout);
    if !result.status.success()
        || !text.contains("signatures OK")
        || text.contains("NOT OK")
        || text.contains("NOKEY")
    {
        return Err("RPM requires an already trusted valid signature".into());
    }
    Ok(())
}
fn timestamp() -> Result<String, String> {
    let result = bounded_command(
        "/usr/bin/date",
        &[OsStr::new("-u"), OsStr::new("+%Y-%m-%dT%H:%M:%SZ")],
        5,
        128,
    )?;
    if !result.status.success() {
        return Err("clock unavailable".into());
    }
    String::from_utf8(result.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "invalid timestamp".into())
}
fn migration_bytes(account: &Account) -> Result<(File, Vec<u8>), String> {
    let config = open_optional_directory(&account.home, ".config", account.uid, account.gid)?
        .ok_or("legacy profile missing")?;
    let ryoku = open_optional_directory(&config, "ryoku", account.uid, account.gid)?
        .ok_or("legacy profile missing")?;
    let mut source = File::from(
        openat2(
            &ryoku,
            "profile.json",
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|e| e.to_string())?,
    );
    let before = fstat(&source).map_err(|e| e.to_string())?;
    if before.st_uid != account.uid
        || before.st_gid != account.gid
        || before.st_nlink != 1
        || FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || !matches!(before.st_mode & 0o7777, 0o600 | 0o644)
    {
        return Err("unsafe legacy profile".into());
    }
    let mut bytes = Vec::new();
    (&mut source)
        .take((ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if classify_legacy_personal_profile(&bytes).is_none()
        || !same_file_version(&before, &fstat(&source).map_err(|e| e.to_string())?)
    {
        return Err("unsupported or changed legacy format; source preserved".into());
    }
    match openat2(
        &ryoku,
        "opensuse-profile.json",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(fd) => {
            let mut target = File::from(fd);
            let stat = fstat(&target).map_err(|e| e.to_string())?;
            let mut target_bytes = Vec::new();
            (&mut target)
                .take((ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
                .read_to_end(&mut target_bytes)
                .map_err(|e| e.to_string())?;
            if stat.st_uid != account.uid
                || stat.st_gid != account.gid
                || stat.st_nlink != 1
                || stat.st_mode & 0o7777 != 0o600
                || FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
                || target_bytes != bytes
            {
                return Err("personal override conflict; both files preserved".into());
            }
        }
        Err(rustix::io::Errno::NOENT) => (),
        Err(e) => return Err(e.to_string()),
    }
    Ok((ryoku, bytes))
}
fn migrate(account: &Account) -> Result<(), String> {
    let (directory, bytes) = migration_bytes(account)?;
    if statat(&directory, "opensuse-profile.json", AtFlags::SYMLINK_NOFOLLOW).is_ok() {
        return Ok(());
    }
    let mut file = File::from(
        rustix::fs::openat(
            &directory,
            ".",
            OFlags::TMPFILE | OFlags::WRONLY | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|e| e.to_string())?,
    );
    rustix::fs::fchown(
        &file,
        Some(rustix::process::Uid::from_raw(account.uid)),
        Some(rustix::process::Gid::from_raw(account.gid)),
    )
    .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    // AT_EMPTY_PATH links the unnamed, fully synced inode without replacing an
    // existing override. A crash before link leaves no selectable partial file.
    rustix::fs::linkat(&file, "", &directory, "opensuse-profile.json", AtFlags::EMPTY_PATH)
        .map_err(|e| e.to_string())?;
    directory.sync_all().map_err(|e| e.to_string())?;
    migration_bytes(account)?;
    Ok(())
}

fn recover(args: &[OsString], output: &mut impl Write) -> Result<(), String> {
    use crate::mutation_gate::{InstallerRestoreReceipt, MutationOutcome, MutationStage};
    use ryoku_recovery::restore::{RestorePhase, RestoreScope};
    if args.len() != 4 {
        return Err(
            "usage: recover UID OPERATION RESTORE_ID_OR_none; none is allowed only before Applying"
                .into(),
        );
    }
    require_console()?;
    validate_host()?;
    let uid = args[1].to_str().ok_or("invalid UID")?.parse().map_err(|_| "invalid UID")?;
    let account = Account::resolve(uid)?;
    let session = NativeCaptureSession::open(
        account.capture()?,
        account.home.try_clone().map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let operation = OperationId::new(args[2].to_str().ok_or("invalid operation ID")?.to_owned())
        .map_err(|_| "invalid operation ID")?;
    let mut journal = crate::native_mutation::open_journal(
        session.coordinator_directory().map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut context = journal
        .mutation_context(&operation)
        .cloned()
        .ok_or("operation has no protected mutation record")?;
    let mut status = journal.get(&operation).cloned().ok_or("operation has no status")?;
    if !matches!(
        context.plan.operation_key.as_str(),
        "installer-install-v1" | "installer-remove-v1" | "installer-migrate-v1"
    ) || context.stage == MutationStage::Complete
    {
        return Err("not an unfinished installer operation".into());
    }
    if matches!(
        context.stage,
        MutationStage::Preflight
            | MutationStage::StateChecking
            | MutationStage::Capturing
            | MutationStage::Revalidating
    ) {
        if args[3] != "none" || context.outcome.is_some() {
            return Err("before-change reconciliation requires unresolved pre-apply history and none receipt".into());
        }
        context.outcome = Some(MutationOutcome::FailedBeforeChange);
    } else {
        let restore_id = Identifier::new(args[3].to_str().ok_or("invalid restore ID")?.to_owned())
            .map_err(|_| "invalid restore ID")?;
        let receipt = ryoku_recovery::restore::RestoreJournalStore::open_existing_state_mount()
            .and_then(|store| store.load(&restore_id))
            .map_err(|e| e.to_string())?;
        let checkpoint = context.checkpoint.as_ref().ok_or("no verified original checkpoint")?;
        if receipt.phase != RestorePhase::Complete
            || receipt.plan.scope != RestoreScope::CompleteLinked
            || receipt.plan.transaction_id != checkpoint.transaction_id
            || receipt.plan.generation_id != checkpoint.generation_id
            || receipt.plan.source_manifest_sha256 != checkpoint.manifest_sha256
            || checkpoint.transaction_id.as_str() != format!("{}-pre", operation.as_str())
        {
            return Err(
                "restore receipt does not complete this installer's exact pre-change state".into(),
            );
        }
        let root = File::open("/").map_err(|e| e.to_string())?;
        let reader = GenerationReader::open_state_mount().map_err(|e| e.to_string())?;
        let linked = reader
            .open_historical_transaction(&root, &checkpoint.transaction_id)
            .map_err(|e| e.to_string())?;
        if LinkedGeneration::from_verified_generation(linked.generation())
            .map_err(|e| format!("{e:?}"))?
            != *checkpoint
        {
            return Err("original linked identity changed".into());
        }
        let manifest = linked.generation().manifest();
        ryoku_recovery::native_checkpoint::verify_running_root_rollback(
            &root,
            &receipt,
            manifest.root_snapshot_receipt().ok_or("source root receipt missing")?,
        )
        .map_err(|e| e.to_string())?;
        session.verify_inventory(manifest).map_err(|e| e.to_string())?;
        let expected: BTreeMap<_, _> = manifest
            .package_revisions()
            .iter()
            .filter(|package| package.before_nevra.is_some())
            .map(|package| {
                let name = package.package_name.as_str();
                let value = package
                    .before_nevra
                    .as_ref()
                    .and_then(|revision| revision.as_str().strip_prefix(&format!("{name}-")))
                    .ok_or("source package revision is unsupported")?;
                Ok((name.to_owned(), value.to_owned()))
            })
            .collect::<Result<_, String>>()?;
        if installed_ryoku_packages()? != expected {
            return Err("restored installed packages differ from pre-change checkpoint".into());
        }
        for name in expected.keys() {
            let result = bounded_command(
                "/usr/bin/rpm",
                &[OsStr::new("-V"), OsStr::new(name)],
                60,
                MAX_QUERY_BYTES,
            )?;
            if !result.status.success() || !result.stdout.is_empty() {
                return Err("restored package files changed after linked recovery".into());
            }
        }
        ProfileBundleStore::open_state_mount()
            .and_then(|store| store.resolve_generation(linked.generation()))
            .map_err(|e| e.to_string())?;
        account.revalidate()?;
        session.revalidate().map_err(|e| e.to_string())?;
        context.outcome = Some(MutationOutcome::InstallerRestored);
        context.installer_restore = Some(InstallerRestoreReceipt {
            restore_id,
            source_manifest_sha256: checkpoint.manifest_sha256.clone(),
        });
    }
    status.phase = crate::OperationPhase::Failed;
    status.error = Some(crate::ServiceError::RecoveryUnavailable);
    journal.record_mutation(status, context).map_err(|e| format!("{e:?}"))?;
    session.finish().map_err(|e| e.to_string())?;
    writeln!(output,"installer_reconciled operation={} original_failure=retained next_operation=requires_fresh_preview_and_checkpoint",operation.as_str()).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    struct Fixture {
        path: PathBuf,
        account: Account,
    }
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("ryoku-installer-migration-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join(".config")).unwrap();
            fs::create_dir(path.join(".config/ryoku")).unwrap();
            let uid = rustix::process::geteuid().as_raw();
            let gid = rustix::process::getegid().as_raw();
            let account = Account {
                uid,
                gid,
                name: "fixture".into(),
                home_path: path.to_str().unwrap().into(),
                home: File::open(&path).unwrap(),
            };
            let bytes = ryoku_profiles::ProfileDocument::new(
                ryoku_profiles::ProfileId::new("generic-desktop").unwrap(),
                Default::default(),
            )
            .canonical_bytes()
            .unwrap();
            fs::write(path.join(".config/ryoku/profile.json"), bytes).unwrap();
            fs::set_permissions(
                path.join(".config/ryoku/profile.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            Self { path, account }
        }
        fn target(&self) -> PathBuf {
            self.path.join(".config/ryoku/opensuse-profile.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
    #[test]
    fn migration_conflicts_and_symlinks_preserve_both_files() {
        let fixture = Fixture::new();
        let source = fixture.path.join(".config/ryoku/profile.json");
        let bytes = fs::read(&source).unwrap();
        assert_eq!(migration_bytes(&fixture.account).unwrap().1, bytes);
        fs::write(fixture.target(), b"personal override").unwrap();
        fs::set_permissions(fixture.target(), fs::Permissions::from_mode(0o600)).unwrap();
        assert!(migration_bytes(&fixture.account).is_err());
        assert_eq!(fs::read(fixture.target()).unwrap(), b"personal override");
        assert_eq!(fs::read(&source).unwrap(), bytes);
        fs::remove_file(fixture.target()).unwrap();
        symlink("profile.json", fixture.target()).unwrap();
        assert!(migration_bytes(&fixture.account).is_err());
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
    #[test]
    fn repeated_migration_recognizes_only_identical_safe_target() {
        let fixture = Fixture::new();
        let bytes = migration_bytes(&fixture.account).unwrap().1;
        fs::write(fixture.target(), &bytes).unwrap();
        fs::set_permissions(fixture.target(), fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(migration_bytes(&fixture.account).unwrap().1, bytes);
        fs::set_permissions(fixture.target(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(migration_bytes(&fixture.account).is_err());
        fs::remove_file(fixture.target()).unwrap();
        fs::write(fixture.path.join(".config/ryoku/profile.json"), b"{\"hero\":{}}\n").unwrap();
        assert!(migration_bytes(&fixture.account).is_err());
        assert!(!fixture.target().exists());
    }
}
