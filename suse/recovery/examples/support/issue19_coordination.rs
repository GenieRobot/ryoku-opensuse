//! Native issue19 acceptance in the already guarded disposable guest only.
use super::{ProbeResult, TRANSACTION, id};
use ryoku_recovery::{
    RelativePath, snapper_catalog::ProtectedSnapperCatalog, snapper_layout::discover_layout,
};
use ryoku_snapper::{
    ConfigName, LayoutCapability, PairCorrelation, PairDisposition, SnapperBackend,
    SnapperCheckpointBackend, SnapperCliBackend, SnapperCliError, SnapperPairBackend,
    SnapperRollbackBackend, SnapshotPairRequest, TransactionId, query_pair,
};
use std::{
    fs::{self, File},
    process::Command,
};

fn command(program: &str, args: &[&str]) -> ProbeResult<String> {
    let result = Command::new("/usr/bin/timeout")
        .env_clear()
        .args(["--signal=KILL", "180s", program])
        .args(args)
        .output()?;
    if !result.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&result.stderr));
        return Err("issue19 native fixture command failed".into());
    }
    Ok(String::from_utf8(result.stdout)?)
}
fn require(condition: bool, message: &str) -> ProbeResult<()> {
    if condition { Ok(()) } else { Err(message.to_owned().into()) }
}
fn root_config() -> ConfigName {
    ConfigName::new("root").expect("fixed config")
}
fn extra_config() -> ConfigName {
    ConfigName::new("issue19-extra").expect("fixed config")
}
fn tx() -> TransactionId {
    TransactionId::new(TRANSACTION).expect("fixed transaction")
}

pub fn capture(root: &File) -> ProbeResult<()> {
    let etc = RelativePath::new("etc").map_err(|_| "fixed path")?;
    let layout = discover_layout(root, &etc)?;
    require(
        layout.assessment.capability == LayoutCapability::FullSystem
            && layout.external_firmware_mount,
        "root-/boot-files layout or explicit external ESP not observed",
    )?;
    require(
        layout.targets.iter().any(|t| t.config == extra_config()),
        "additional config missing",
    )?;
    println!("native_issue19_full_root_boot_files=passed");
    println!("native_issue19_esp_coverage=excluded");
    // A real tmpfs over /boot must prevent the full-system layout label.
    command(
        "/usr/bin/mount",
        &["-t", "tmpfs", "-o", "size=1m", "issue19-unsupported-boot", "/boot"],
    )?;
    let inspected = (|| -> ProbeResult<()> {
        require(
            discover_layout(root, &etc)?.assessment.capability
                == LayoutCapability::ConfigurationOnly,
            "separate boot was not config-only",
        )?;
        let run = RelativePath::new("run").map_err(|_| "fixed path")?;
        require(
            discover_layout(root, &run)?.assessment.capability == LayoutCapability::Unsupported,
            "unsupported config and boot overclaimed",
        )
    })();
    let restored = command("/usr/bin/umount", &["/boot"]);
    inspected?;
    restored?;
    require(
        discover_layout(root, &etc)?.assessment.capability == LayoutCapability::FullSystem,
        "boot fixture not restored",
    )?;
    println!("native_issue19_configuration_only=passed");
    println!("native_issue19_unsupported_layout=passed");
    // Stock upstream hook telemetry: a real package transaction with explicit
    // userdata. This is not an authorization/veto or protected-write test.
    require(
        !Command::new("/usr/bin/rpm").args(["-q", "hello"]).output()?.status.success(),
        "package already present",
    )?;
    let request = SnapshotPairRequest {
        config: root_config(),
        transaction_id: TransactionId::new("issue19-package-hook").map_err(|_| "fixed tag")?,
    };
    require(
        SnapperCliBackend
            .list(&request.config)?
            .iter()
            .all(|r| r.transaction_id.as_ref() != Some(&request.transaction_id)),
        "hook tag already used",
    )?;
    command(
        "/usr/bin/zypper",
        &[
            "--non-interactive",
            "--no-refresh",
            "--userdata",
            "ryoku-transaction-id=issue19-package-hook",
            "install",
            "--no-recommends",
            "hello",
        ],
    )?;
    require(
        Command::new("/usr/bin/rpm").args(["-q", "hello"]).output()?.status.success(),
        "package mutation absent",
    )?;
    let pair =
        query_pair(&SnapperCliBackend, &request).map_err(|_| "hook pair inventory failed")?;
    require(
        pair.reconciliation.disposition == PairDisposition::ReuseExistingPair,
        "native hook pair not reusable",
    )?;
    let PairCorrelation::Existing { pre, post } = &pair.reconciliation.correlation else {
        return Err("native hook pair incomplete".into());
    };
    require(pre != post, "identical hook partners")?;
    require(
        matches!(SnapperPairBackend.create_pre(&request), Err(SnapperCliError::PairAlreadyExists)),
        "redundant pre creation not refused",
    )?;
    require(
        SnapperCliBackend.list(&request.config)? == pair.records,
        "pair reuse changed inventory",
    )?;
    println!("native_issue19_package_hook_pair=passed");
    println!("native_issue19_no_nested_pair=passed");
    eprintln!("issue19 hook root pre={} post={}", pre.as_str(), post.as_str());
    fs::write("/mnt/ryoku-issue19-extra/fixture", b"retained additional configuration\n")?;
    let extra = SnapperCheckpointBackend
        .create_single(&SnapshotPairRequest { config: extra_config(), transaction_id: tx() })?;
    // Exercise the real transfer from native number cleanup ownership, not
    // merely a snapshot that happened to start with no cleanup algorithm.
    let manifest = ryoku_recovery::generation::GenerationReader::open_state_mount()?
        .verify_transaction(&id(TRANSACTION)?)?;
    command(
        "/usr/bin/snapper",
        &[
            "--config",
            "root",
            "modify",
            "--cleanup-algorithm",
            "number",
            &manifest.snapper_snapshot_id,
        ],
    )?;
    command(
        "/usr/bin/snapper",
        &["--config", "issue19-extra", "modify", "--cleanup-algorithm", "number", extra.as_str()],
    )?;
    let catalog = ProtectedSnapperCatalog::initialize(root)?;
    catalog.publish(&id(TRANSACTION)?, &[(extra_config(), extra.clone())])?;
    require(
        catalog.publish(&id(TRANSACTION)?, &[(extra_config(), extra.clone())]).is_err(),
        "duplicate catalog publish accepted",
    )?;
    require(catalog.status(&id(TRANSACTION)?)?.complete, "complete snapshot set unavailable")?;
    catalog.mark_manual(&id(TRANSACTION)?)?;
    let status = catalog.status(&id(TRANSACTION)?)?;
    require(
        status.complete && status.entry.manual_checkpoint && !status.entry.known_working,
        "manual pin unavailable",
    )?;
    // A running-root mismatch and incomplete Planned journal cannot promote.
    require(
        catalog.mark_known_working_after_boot(&id(TRANSACTION)?, &extra, &tx()).is_err(),
        "premature known-working promotion accepted",
    )?;
    println!("native_issue19_additional_catalog=passed");
    println!("native_issue19_manual_pin=passed");
    eprintln!(
        "issue19 catalog root={} additional={}:{}",
        status.entry.root_snapshot.as_str(),
        extra_config().as_str(),
        extra.as_str()
    );
    let path = format!("/mnt/ryoku-issue19-extra/.snapshots/{}/snapshot", extra.as_str());
    let held = format!("/mnt/ryoku-issue19-extra/.snapshots/{}/issue19-held", extra.as_str());
    fs::rename(&path, &held)?;
    let missing = catalog.status(&id(TRANSACTION)?);
    fs::rename(&held, &path)?;
    require(!missing?.complete, "missing additional snapshot reported complete")?;
    require(catalog.status(&id(TRANSACTION)?)?.complete, "additional partner not restored")?;
    println!("native_issue19_missing_additional=passed");
    for (original, held) in [
        (
            format!("/.snapshots/{}/snapshot", status.entry.root_snapshot.as_str()),
            format!("/.snapshots/{}/issue19-primary-held", status.entry.root_snapshot.as_str()),
        ),
        (
            "/var/lib/ryoku-state/generations/issue19-linked-generation".to_owned(),
            "/var/lib/ryoku-state/generations/issue19-primary-held".to_owned(),
        ),
    ] {
        fs::rename(&original, &held)?;
        let missing = catalog.status(&id(TRANSACTION)?);
        fs::rename(&held, &original)?;
        require(
            !missing.is_ok_and(|value| value.complete),
            "missing primary partner reported complete",
        )?;
        require(catalog.status(&id(TRANSACTION)?)?.complete, "primary partner not restored")?;
    }
    println!("native_issue19_missing_primary_partners=passed");
    let path = "/var/lib/ryoku-state/snapper-catalog/catalog.json";
    let original = fs::read(path)?;
    let mut tampered = original.clone();
    let offset = tampered
        .windows(TRANSACTION.len())
        .position(|w| w == TRANSACTION.as_bytes())
        .ok_or("transaction absent from protected catalog")?;
    tampered[offset] = b'x';
    for malformed in [b"{".as_slice(), tampered.as_slice()] {
        write_catalog_fixture(path, malformed)?;
        let refused = ProtectedSnapperCatalog::open(root).is_err();
        write_catalog_fixture(path, &original)?;
        require(refused, "corrupt protected catalog accepted")?;
        require(
            ProtectedSnapperCatalog::open(root)?.status(&id(TRANSACTION)?)?.complete,
            "original protected catalog not restored",
        )?;
    }
    println!("native_issue19_catalog_corruption_refused=passed");
    Ok(())
}
pub fn after_boot(root: &File) -> ProbeResult<()> {
    let catalog = ProtectedSnapperCatalog::open(root)?;
    let before = catalog.status(&id(TRANSACTION)?)?;
    require(
        before.complete && before.entry.manual_checkpoint && !before.entry.known_working,
        "catalog/manual pin failed rollback readback",
    )?;
    let rollback_tx = TransactionId::new("issue19-root-rollback").map_err(|_| "fixed tag")?;
    let mut targets = Vec::new();
    for row in SnapperCliBackend.list(&root_config())? {
        if row.transaction_id.as_ref() == Some(&rollback_tx)
            && SnapperRollbackBackend
                .verify_running_target(&root_config(), &row.id, &rollback_tx)
                .is_ok()
        {
            targets.push(row.id);
        }
    }
    require(targets.len() == 1, "booted rollback target ambiguous")?;
    catalog.mark_known_working_after_boot(&id(TRANSACTION)?, &targets[0], &rollback_tx)?;
    let status = catalog.status(&id(TRANSACTION)?)?;
    require(
        status.complete
            && status.cleanup_owned
            && status.entry.manual_checkpoint
            && status.entry.known_working,
        "known-working pin unavailable",
    )?;
    println!("native_issue19_catalog_after_rollback=passed");
    println!("native_issue19_known_working_pin=passed");
    // Native cleanup removes an owned sacrificial number-policy snapshot while
    // leaving every catalog partner excluded from automatic cleanup.
    let sacrifice = command(
        "/usr/bin/snapper",
        &[
            "--config",
            "root",
            "create",
            "--type",
            "single",
            "--read-only",
            "--print-number",
            "--cleanup-algorithm",
            "number",
            "--userdata",
            "ryoku-transaction-id=issue19-cleanup-sacrifice",
        ],
    )?;
    let sacrifice = sacrifice.trim();
    require(
        sacrifice.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == sacrifice),
        "invalid sacrificial snapshot",
    )?;
    command(
        "/usr/bin/snapper",
        &[
            "--config",
            "root",
            "set-config",
            "NUMBER_MIN_AGE=0",
            "NUMBER_LIMIT=0",
            "NUMBER_LIMIT_IMPORTANT=0",
        ],
    )?;
    command("/usr/bin/snapper", &["--config", "root", "cleanup", "number"])?;
    require(
        SnapperCliBackend.list(&root_config())?.iter().all(|r| r.id.as_str() != sacrifice),
        "number cleanup did not remove sacrificial snapshot",
    )?;
    command(
        "/usr/bin/snapper",
        &[
            "--config",
            "issue19-extra",
            "set-config",
            "NUMBER_MIN_AGE=0",
            "NUMBER_LIMIT=0",
            "NUMBER_LIMIT_IMPORTANT=0",
        ],
    )?;
    command("/usr/bin/snapper", &["--config", "issue19-extra", "cleanup", "number"])?;
    let retained = ProtectedSnapperCatalog::open(root)?.status(&id(TRANSACTION)?)?;
    require(
        retained.complete && retained.entry.manual_checkpoint && retained.entry.known_working,
        "native cleanup orphaned pinned catalog",
    )?;
    println!("native_issue19_cleanup_ownership=passed");
    let extra = &retained.entry.additional_snapshots[0];
    // Explicit disposable deletion exercises permanent partner loss, after
    // the preservation result. Never label the now-incomplete set complete.
    command("/usr/bin/snapper", &["--config", &extra.config, "delete", &extra.snapshot])?;
    let deleted = catalog.status(&id(TRANSACTION)?)?;
    require(
        !deleted.complete && deleted.entry.known_working && deleted.entry.manual_checkpoint,
        "deleted partner did not invalidate retained set",
    )?;
    println!("native_issue19_deleted_partner=passed");
    Ok(())
}

fn write_catalog_fixture(path: &str, bytes: &[u8]) -> ProbeResult<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().write(true).truncate(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
