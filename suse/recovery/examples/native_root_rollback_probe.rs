//! Explicit disposable-guest experiment, never a production rollback interface.
//! Four separate invocations straddle two harness-owned reboots. Any incomplete
//! phase is retained for review; there is deliberately no resume or repair mode.
//! Only unchanged-kernel/ESP root rollback and protected-record persistence are
//! tested. The linked probe and recovery CLI remain independent readback gates.
#[path = "support/rollback_native.rs"]
mod native;

use native::{
    Result, SMALL, Volume, command, create, hash, open, read, replace, require, text, volume,
};
use rustix::fs::{FlockOperation, Mode, OFlags, ResolveFlags, flock, openat2};
use ryoku_recovery::{
    Identifier,
    generation::GenerationReader,
    machine_state::read_machine_state_record,
    native_checkpoint::{RootSnapshotReceipt, verify_new_root_checkpoint},
    state_mount::collect_and_open_state_mount,
    state_provision::{verify_initialized_state_root, verify_recorded_state_mount_unit},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    os::unix::fs::{DirBuilderExt, MetadataExt},
};

const PLAN: &str = "native-root-rollback-fixture.json";
const MARKER: &str = "ryoku-lab-root-marker";
const BEFORE: &[u8] = b"before native linked source snapshot\n";
const AFTER: &[u8] = b"after native linked source snapshot\n";
const RELEASE: &str = "7.2.9-1-default";
const KERNEL: &str = "usr/lib/modules/7.2.9-1-default/vmlinuz";
const INITRD: &str = "boot/initrd-7.2.9-1-default";
const EFI: &str = "boot/efi/EFI/BOOT/BOOTX64.EFI";
const GRUB: &str = "boot/grub2/grub.cfg";
const ENV: &str = "boot/grub2/grubenv";
const DIAGNOSTICS: &str = "/run/ryoku-rollback-initrd-diagnostics";
const RECORDS: &[&str] = &[
    "catalog/native-linked-probe.json",
    "generations/native-linked-generation/manifest.json",
    "restore-journals/native-linked-journal.json",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Phase {
    Preparing,
    Prepared,
    Ready,
    Switching,
    Armed,
    Verified,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    schema: u16,
    phase: Phase,
    original_boot: String,
    prepared_boot: Option<String>,
    original: Volume,
    namespace: Volume,
    state: Volume,
    machine: Vec<u8>,
    fstab: String,
    prepared_fstab: String,
    grub: String,
    prepared_grub: String,
    boot_assets: BTreeMap<String, String>,
    source: Option<Source>,
    target_number: Option<String>,
    target: Option<Volume>,
    backup_number: Option<String>,
    records: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    receipt: RootSnapshotReceipt,
    volume: Volume,
}

fn boot_id(root: &File) -> Result<String> {
    let boot = text(root, "proc/sys/kernel/random/boot_id")?;
    let id = boot.trim();
    require(
        id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "invalid boot ID",
    )?;
    Ok(id.into())
}

fn check_dmi_class_link(class: &File) -> Result<()> {
    let target = rustix::fs::readlinkat(class, "id", Vec::new())?;
    require(
        target.to_bytes() == b"../../devices/virtual/dmi/id",
        "unexpected native DMI class link",
    )
}

fn qemu_vendor(root: &File) -> Result<bool> {
    let sys = open(root, "sys", true)?;
    require(
        rustix::fs::fstatfs(&sys)?.f_type == i64::from(linux_raw_sys::general::SYSFS_MAGIC),
        "DMI guard requires native sysfs",
    )?;
    // Linux registers the parentless DMI class device under devices/virtual;
    // class/dmi/id is its kernel-owned class link. Validate that one exact link,
    // then open the physical device path without following any symlinks. Keep
    // the general fixture reader's NO_SYMLINKS policy unchanged.
    let class = open(&sys, "class/dmi", true)?;
    check_dmi_class_link(&class)?;
    Ok(text(&sys, "devices/virtual/dmi/id/sys_vendor")?.trim() == "QEMU")
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    require(
        args.len() == 1
            && matches!(
                args[0].as_str(),
                "--prepare" | "--verify-prepared" | "--rollback" | "--verify-rollback"
            ),
        "requires --prepare | --verify-prepared | --rollback | --verify-rollback",
    )?;
    require(
        rustix::process::geteuid().as_raw() == 0 && rustix::process::getegid().as_raw() == 0,
        "root required",
    )?;
    let root = File::open("/")?;
    require(
        text(&root, "etc/ryoku-lab-fixture")? == "disposable-issue13\n" && qemu_vendor(&root)?,
        "seeded disposable QEMU required",
    )?;
    let machine = read_machine_state_record(&root)?;
    verify_initialized_state_root(&root, &machine)?;
    verify_recorded_state_mount_unit(&root)?;
    let (state, _) = collect_and_open_state_mount(&root, machine.identity())?;
    let state = File::from(state);
    let run = open(&root, "run", true)?;
    let lock = File::from(openat2(
        &run,
        ".ryoku-root-rollback-fixture.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let meta = lock.metadata()?;
    require(
        meta.is_file()
            && meta.uid() == 0
            && meta.gid() == 0
            && meta.nlink() == 1
            && meta.mode() & 0o7777 == 0o600,
        "unsafe fixture lock",
    )?;
    flock(&lock, FlockOperation::NonBlockingLockExclusive)?;
    match args[0].as_str() {
        "--prepare" => prepare(&root, &state)?,
        mode => {
            let metadata = open(&state, PLAN, false)?.metadata()?;
            require(
                metadata.mode() & 0o7777 == 0o600 && metadata.nlink() == 1,
                "unsafe fixture plan",
            )?;
            let bytes = read(&state, PLAN, SMALL)?;
            let mut plan: Baseline = serde_json::from_slice(&bytes)?;
            require(
                plan.schema == 1 && serde_json::to_vec(&plan)? == bytes,
                "noncanonical fixture plan",
            )?;
            verify_common(&root, &state, &plan)?;
            match mode {
                "--verify-prepared" => verify_prepared(&root, &state, &mut plan)?,
                "--rollback" => rollback(&root, &state, &mut plan)?,
                "--verify-rollback" => verify_rollback(&root, &state, &mut plan)?,
                _ => unreachable!(),
            }
        }
    }
    println!("native_root_rollback_boot_compatibility=unproven");
    println!("native_root_rollback_full_system=unproven");
    Ok(())
}

fn save(state: &File, plan: &mut Baseline, phase: Phase) -> Result<()> {
    let before = serde_json::to_vec(plan)?;
    plan.phase = phase;
    let after = serde_json::to_vec(plan)?;
    require(after.len() as u64 <= SMALL, "fixture plan exceeds bound")?;
    replace(state, PLAN, &before, &after)
}

fn default_id() -> Result<u64> {
    parse_default(&command("/usr/sbin/btrfs", &["subvolume", "get-default", "/"], None)?)
}

fn parse_default(output: &str) -> Result<u64> {
    let tokens: Vec<_> = output.split_whitespace().collect();
    require(
        output.lines().count() == 1 && tokens.first() == Some(&"ID") && tokens.len() >= 2,
        "ambiguous Btrfs default",
    )?;
    number(tokens[1])
}

fn number(value: &str) -> Result<u64> {
    let n = value.parse::<u64>()?;
    require(n > 0 && n.to_string() == value, "noncanonical snapshot or subvolume number")?;
    Ok(n)
}

fn assets(root: &File, include_efi: bool) -> Result<BTreeMap<String, String>> {
    let boot = open(root, "boot", true)?;
    let selected = format!("vmlinuz-{RELEASE}");
    match open(&boot, &selected, false) {
        Ok(_) => require(
            hash(&boot, &selected)? == hash(root, KERNEL)?,
            "GRUB kernel differs from recorded module image",
        )?,
        Err(error)
            if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::LOOP) =>
        {
            let target = rustix::fs::readlinkat(&boot, selected.as_str(), Vec::new())?;
            require(
                target.to_bytes() == format!("../{KERNEL}").as_bytes(),
                "unexpected selected /boot kernel alias",
            )?;
        }
        Err(error) => return Err(error),
    }
    let mut result = BTreeMap::new();
    for path in [
        KERNEL,
        INITRD,
        ENV,
        "etc/ryoku/state-machine.json",
        "etc/systemd/system/var-lib-ryoku\\x2dstate.mount",
    ] {
        result.insert(path.into(), hash(root, path)?);
    }
    if include_efi {
        result.insert(EFI.into(), hash(root, EFI)?);
    }
    Ok(result)
}

fn verify_assets(root: &File, plan: &Baseline, include_efi: bool) -> Result<()> {
    let actual = assets(root, include_efi)?;
    require(
        actual.iter().all(|(path, digest)| plan.boot_assets.get(path) == Some(digest)),
        "boot asset or independent state record changed; do not reboot",
    )
}

fn namespace_mount(root: &File, plan: &Baseline) -> Result<()> {
    require(
        volume(&open(root, ".snapshots", true)?)? == plan.namespace,
        "snapshot namespace substituted",
    )?;
    let output = command(
        "/usr/bin/findmnt",
        &[
            "--noheadings",
            "--raw",
            "--mountpoint",
            "/.snapshots",
            "--output",
            "FSTYPE,FSROOT,OPTIONS",
        ],
        None,
    )?;
    let columns: Vec<_> = output.split_whitespace().collect();
    require(
        columns.len() == 3
            && columns[0] == "btrfs"
            && columns[1] == "/@/.snapshots"
            && columns[2].split(',').any(|o| o == format!("subvolid={}", plan.namespace.id))
            && columns[2].split(',').any(|o| o == "rw"),
        "snapshot namespace is not its recorded independent mount",
    )
}

fn verify_common(root: &File, state: &File, plan: &Baseline) -> Result<()> {
    require(volume(state)? == plan.state, "protected state subvolume changed")?;
    require(
        read_machine_state_record(root)?.canonical_bytes()? == plan.machine,
        "independent machine record changed",
    )?;
    require(text(root, "etc/fstab")? == plan.prepared_fstab, "prepared fstab changed")?;
    namespace_mount(root, plan)?;
    verify_assets(root, plan, true)
}

fn prepare(root: &File, state: &File) -> Result<()> {
    // A prior plan, even a partial one, must never be adopted or overwritten.
    match open(state, PLAN, false) {
        Err(error)
            if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT) => {}
        _ => return Err("fixture plan already exists or cannot be safely inspected".into()),
    }
    let original = volume(root)?;
    require(
        original.id == 256
            && original.parent == 5
            && !original.readonly
            && default_id()? == original.id,
        "requires unchanged fixture @ root and default",
    )?;
    require(command("/usr/bin/uname", &["-r"], None)?.trim() == RELEASE, "fixture kernel changed")?;
    require(
        command("/usr/bin/grub2-editenv", &["/boot/grub2/grubenv", "list"], None)?.is_empty(),
        "nonempty grubenv requires review",
    )?;
    let namespace = volume(&open(root, ".snapshots", true)?)?;
    let mount_id = |file: &File| -> Result<u64> {
        let stat = rustix::fs::statx(
            file,
            "",
            rustix::fs::AtFlags::EMPTY_PATH,
            rustix::fs::StatxFlags::MNT_ID,
        )?;
        require(
            stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0 && stat.stx_mnt_id != 0,
            "mount identity unavailable",
        )?;
        Ok(stat.stx_mnt_id)
    };
    require(
        mount_id(root)? == mount_id(&open(root, ".snapshots", true)?)?,
        "snapshot namespace already mounted; prior preparation is not adopted",
    )?;
    require(
        namespace.parent == original.id
            && namespace.filesystem == original.filesystem
            && !namespace.readonly,
        "unexpected original snapshot namespace",
    )?;
    let fstab = text(root, "etc/fstab")?;
    let prepared_fstab = prepare_fstab(&fstab, &original.filesystem, namespace.id)?;
    let grub = text(root, GRUB)?;
    let prepared_grub = prepare_grub(&grub, &original.filesystem)?;
    let mapping = text(root, "etc/snapper/configs/root")?;
    for (key, expected) in [("SUBVOLUME", "\"/\""), ("FSTYPE", "\"btrfs\"")] {
        let values: Vec<_> = mapping
            .lines()
            .filter_map(|line| line.trim().split_once('='))
            .filter(|(name, _)| name.trim() == key)
            .map(|(_, value)| value.trim())
            .collect();
        require(values == [expected], "root Snapper mapping changed")?;
    }
    let cmdline = text(root, "proc/cmdline")?;
    require(
        cmdline.split_whitespace().filter(|s| s.starts_with("rootflags=")).collect::<Vec<_>>()
            == ["rootflags=subvol=@"],
        "unexpected original command-line root selection",
    )?;
    initrd_diagnostic(root, state)?;
    let mut plan = Baseline {
        schema: 1,
        phase: Phase::Preparing,
        original_boot: boot_id(root)?,
        prepared_boot: None,
        original,
        namespace,
        state: volume(state)?,
        machine: read_machine_state_record(root)?.canonical_bytes()?,
        fstab,
        prepared_fstab,
        grub,
        prepared_grub,
        boot_assets: assets(root, true)?,
        source: None,
        target_number: None,
        target: None,
        backup_number: None,
        records: BTreeMap::new(),
    };
    let encoded = serde_json::to_vec(&plan)?;
    require(encoded.len() as u64 <= SMALL, "fixture plan exceeds bound")?;
    create(state, PLAN, &encoded)?;
    let etc = open(root, "etc", true)?;
    replace(&etc, "fstab", plan.fstab.as_bytes(), plan.prepared_fstab.as_bytes())?;
    command("/usr/bin/systemctl", &["daemon-reload"], None)?;
    let namespace_id = format!("subvolid={}", plan.namespace.id);
    let device = format!("/dev/disk/by-uuid/{}", plan.original.filesystem);
    command("/usr/bin/mount", &["-t", "btrfs", "-o", &namespace_id, &device, "/.snapshots"], None)?;
    namespace_mount(root, &plan)?;
    let grub_dir = open(root, "boot/grub2", true)?;
    replace(&grub_dir, "grub.cfg", plan.grub.as_bytes(), plan.prepared_grub.as_bytes())?;
    verify_assets(root, &plan, true)?;
    require(default_id()? == plan.original.id, "default changed during preparation")?;
    save(state, &mut plan, Phase::Prepared)?;
    println!("native_root_rollback_preparation=passed");
    Ok(())
}

fn prepare_fstab(original: &str, filesystem: &str, namespace_id: u64) -> Result<String> {
    require(original.ends_with('\n') && namespace_id >= 256, "invalid fixture fstab")?;
    let rows: Vec<Vec<_>> = original
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| l.split_whitespace().collect())
        .filter(|l: &Vec<&str>| !l.is_empty())
        .collect();
    require(
        rows.iter().all(|r| r.len() == 6 && r[1] != "/.snapshots"),
        "existing or malformed snapshot mount entry",
    )?;
    let roots: Vec<_> = rows.iter().filter(|r| r[1] == "/").collect();
    require(
        roots.len() == 1
            && roots[0][0] == format!("UUID={filesystem}")
            && roots[0][2] == "btrfs"
            && roots[0][3] == "defaults",
        "unexpected root fstab options",
    )?;
    Ok(format!("{original}UUID={filesystem} /.snapshots btrfs subvolid={namespace_id} 0 0\n"))
}

fn prepare_grub(original: &str, filesystem: &str) -> Result<String> {
    let mut count = 0;
    let mut initrds = 0;
    for line in original.lines() {
        let tokens: Vec<_> = line.split_whitespace().collect();
        if tokens.first().is_some_and(|s| matches!(*s, "linux" | "linuxefi")) {
            require(
                tokens.get(1) == Some(&format!("/@/boot/vmlinuz-{RELEASE}").as_str())
                    && tokens
                        .iter()
                        .filter(|s| s.starts_with("rootflags="))
                        .copied()
                        .collect::<Vec<_>>()
                        == ["rootflags=subvol=@"]
                    && tokens
                        .iter()
                        .filter(|s| s.starts_with("root="))
                        .copied()
                        .collect::<Vec<_>>()
                        == [format!("root=UUID={filesystem}")],
                "unexpected GRUB kernel, root or pinning",
            )?;
            count += 1;
        }
        if tokens.first().is_some_and(|s| matches!(*s, "initrd" | "initrdefi")) {
            require(
                tokens.len() == 2 && tokens[1] == format!("/@/boot/initrd-{RELEASE}"),
                "unexpected GRUB initrd path",
            )?;
            initrds += 1;
        }
    }
    require(
        count == 3 && initrds == 3 && original.matches(" rootflags=subvol=@").count() == 3,
        "unexpected GRUB entry count",
    )?;
    Ok(original.replace(" rootflags=subvol=@", ""))
}

fn initrd_diagnostic(root: &File, state: &File) -> Result<()> {
    let inventory = command(
        "/usr/bin/rpm",
        &["-q", "--qf", "%{NAME}=%{EVR}.%{ARCH}\n[%{FILENAMES}\n]", "dracut"],
        None,
    )?;
    require(
        inventory.lines().any(|l| l == "/usr/bin/lsinitrd"),
        "installed lsinitrd path missing from RPM inventory",
    )?;
    let script = text(root, "usr/bin/lsinitrd")?;
    println!("rollback_private_lsinitrd_script={}", serde_json::to_string(&script)?);
    // This bounded fixture supports the inventoried native fixed-file path.
    // A changed implementation is reported for review before executing it.
    for expected in [
        "--file",
        "cpio --extract --quiet --to-stdout",
        "mktemp -d -t lsinitrd.",
        "trap",
        "skipcpio",
    ] {
        require(script.contains(expected), "installed lsinitrd fixed-file behavior needs review")?;
    }
    require(
        open(root, "usr/lib/dracut/skipcpio", false)?.metadata()?.mode() & 0o111 != 0,
        "native early-archive inspector unavailable",
    )?;
    fs::DirBuilder::new().mode(0o700).create(DIAGNOSTICS)?;
    let result = (|| -> Result<()> {
        let image = format!("/{INITRD}");
        let digest = hash(root, INITRD)?;
        let listing = native::archive_listing(&image, DIAGNOSTICS)?;
        create(state, "native-root-rollback-initrd-listing.txt", listing.as_bytes())?;
        let (main_present, fragments_present) = generic_initrd_listing(&listing)?;
        let mut displayed = String::new();
        for (member, present) in
            [("etc/cmdline", main_present), ("etc/cmdline.d/*.conf", fragments_present)]
        {
            if present {
                displayed.push_str(&command(
                    "/usr/bin/lsinitrd",
                    &["-f", member, &image],
                    Some(DIAGNOSTICS),
                )?);
                displayed.push('\n');
            } else {
                println!("rollback_private_initrd_absent_member={member}");
            }
        }
        require(hash(root, INITRD)? == digest, "initrd changed during inspection")?;
        affirmative_initrd(&displayed)
    })();
    // Never recurse over unexpected leftovers. Native lsinitrd owns its temp
    // directory; an interrupted diagnostic is preserved for explicit review.
    let cleanup = fs::remove_dir(DIAGNOSTICS);
    println!("rollback_private_initrd_temporary_cleanup={}", cleanup.is_ok());
    cleanup?;
    result
}

fn generic_initrd_listing(listing: &str) -> Result<(bool, bool)> {
    require(
        listing.lines().any(|line| {
            line.strip_prefix("Arguments: ")
                .is_some_and(|args| args.split_whitespace().any(|arg| arg == "--no-hostonly"))
        }) && listing.contains("dracut modules:")
            && listing.contains("dracut cmdline:")
            && listing.contains(&format!("usr/lib/modules/{RELEASE}/")),
        "native listing did not establish the fixture's generic initrd build contract",
    )?;
    require(
        !["squash-root.img", "squashfs-root.img", "erofs-root.img"]
            .iter()
            .any(|name| listing.contains(name)),
        "nested initrd archive requires explicit review",
    )?;
    // Parse fixed paths in native verbose output, never archive structures.
    let members: Vec<_> = listing
        .lines()
        .filter(|line| line.starts_with('-'))
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    require(
        !listing.lines().any(|line| line.starts_with('l') && line.contains("etc/cmdline")),
        "linked initrd command-line members require review",
    )?;
    Ok((
        members.contains(&"etc/cmdline"),
        members.iter().any(|path| path.starts_with("etc/cmdline.d/") && path.ends_with(".conf")),
    ))
}

fn affirmative_initrd(content: &str) -> Result<()> {
    let tokens: Vec<_> = content.split_whitespace().collect();
    require(
        tokens.iter().all(|s| !s.contains("subvol") && !s.contains("rootflags=")),
        "embedded root flags require explicit reviewed boot override; no normalization performed",
    )
}

fn verify_prepared(root: &File, state: &File, plan: &mut Baseline) -> Result<()> {
    require(plan.phase == Phase::Prepared, "preparation phase already consumed or interrupted")?;
    let boot = boot_id(root)?;
    require(
        boot != plan.original_boot
            && volume(root)? == plan.original
            && default_id()? == plan.original.id,
        "preparation reboot did not retain original root identity",
    )?;
    require(text(root, GRUB)? == plan.prepared_grub, "effective GRUB config changed")?;
    let cmdline = text(root, "proc/cmdline")?;
    require(
        !cmdline.split_whitespace().any(|s| s.starts_with("rootflags="))
            && cmdline
                .split_whitespace()
                .any(|s| s == format!("root=UUID={}", plan.original.filesystem)),
        "preparation did not boot reviewed no-pin command line",
    )?;
    create(&open(root, "etc", true)?, MARKER, BEFORE)?;
    let before = serde_json::to_vec(plan)?;
    plan.prepared_boot = Some(boot);
    plan.phase = Phase::Ready;
    replace(state, PLAN, &before, &serde_json::to_vec(plan)?)?;
    println!("native_root_rollback_preparation_reboot=passed");
    Ok(())
}

fn record_hashes(state: &File) -> Result<BTreeMap<String, String>> {
    RECORDS.iter().map(|path| Ok(((*path).into(), hash(state, path)?))).collect()
}

fn linked(root: &File) -> Result<(RootSnapshotReceipt, Volume)> {
    let reader = GenerationReader::open_state_mount()?;
    let linked = reader.open_historical_transaction(
        root,
        &Identifier::new("native-linked-probe").map_err(|_| "fixed transaction ID")?,
    )?;
    require(
        linked.generation().manifest().generation_id.as_str() == "native-linked-generation",
        "wrong fixture generation",
    )?;
    Ok((linked.snapshot().receipt().clone(), volume(linked.snapshot().descriptor())?))
}

fn inventory() -> Result<BTreeMap<u64, serde_json::Value>> {
    // Avoid used-space accounting and deliberately changing default/active
    // fields. Native get-default and ioctls independently prove those facts.
    let output = command(
        "/usr/bin/snapper",
        &["--jsonout", "--config", "root", "list", "--columns", "number,type,cleanup,userdata"],
        None,
    )?;
    parse_inventory(&output)
}

fn parse_inventory(output: &str) -> Result<BTreeMap<u64, serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_str(output)?;
    require(value.as_object().is_some_and(|o| o.len() == 1), "ambiguous Snapper inventory")?;
    let mut rows = BTreeMap::new();
    for row in value["root"].as_array().ok_or("missing root inventory")? {
        let id = match (row["number"].as_u64(), row["number"].as_str()) {
            (Some(id), _) => id,
            (_, Some("0")) => 0,
            (_, Some(value)) => number(value)?,
            _ => return Err("non-numeric snapshot ID".into()),
        };
        require(rows.insert(id, row.clone()).is_none(), "duplicate native snapshot ID")?;
    }
    Ok(rows)
}

fn rollback(root: &File, state: &File, plan: &mut Baseline) -> Result<()> {
    require(
        plan.phase == Phase::Ready
            && plan.prepared_boot.as_ref() == Some(&boot_id(root)?)
            && volume(root)? == plan.original
            && default_id()? == plan.original.id,
        "rollback requires exact prepared running root",
    )?;
    require(text(root, GRUB)? == plan.prepared_grub, "GRUB changed before rollback")?;
    let (receipt, source) = linked(root)?;
    let fresh = verify_new_root_checkpoint(root, receipt.link().clone())?;
    require(
        fresh.receipt() == &receipt && source.origin == plan.original.uuid && source.readonly,
        "source does not match original fresh native receipt",
    )?;
    require(
        read(
            &open(
                root,
                &format!(".snapshots/{}/snapshot", receipt.link().snapper_snapshot_id),
                true,
            )?,
            &format!("etc/{MARKER}"),
            SMALL,
        )? == BEFORE,
        "source snapshot predates required root marker",
    )?;
    let before_inventory = inventory()?;
    let source_number = number(&receipt.link().snapper_snapshot_id)?;
    let source_row =
        before_inventory.get(&source_number).ok_or("source absent from root inventory")?;
    require(
        source_row["type"] == "single"
            && source_row["cleanup"].as_str() == Some("")
            && source_row["userdata"]["ryoku-transaction-id"] == "native-linked-probe",
        "source snapshot ownership or retention changed",
    )?;
    let tag = format!("ryoku-lab-rollback={}", plan.original_boot);
    require(
        before_inventory.values().all(|row| row["userdata"]["ryoku-lab-rollback"].is_null()),
        "prior rollback fixture snapshot exists",
    )?;
    let encoded = serde_json::to_vec(plan)?;
    plan.source = Some(Source { receipt: receipt.clone(), volume: source });
    plan.records = record_hashes(state)?;
    plan.phase = Phase::Switching;
    replace(state, PLAN, &encoded, &serde_json::to_vec(plan)?)?;
    replace(&open(root, "etc", true)?, MARKER, BEFORE, AFTER)?;
    let output = command(
        "/usr/bin/snapper",
        &[
            "--config",
            "root",
            "--ambit",
            "classic",
            "--quiet",
            "rollback",
            "--print-number",
            "--cleanup-algorithm",
            "",
            "--userdata",
            &tag,
            &receipt.link().snapper_snapshot_id,
        ],
        None,
    )?;
    let target_number = output.strip_suffix('\n').unwrap_or(&output);
    let target_id = number(target_number)?;
    let after_inventory = inventory()?;
    let added: Vec<_> =
        after_inventory.iter().filter(|(id, _)| !before_inventory.contains_key(id)).collect();
    require(
        added.len() == 2
            && after_inventory.len() == before_inventory.len() + 2
            && before_inventory.iter().all(|(id, old)| after_inventory.get(id) == Some(old)),
        "unexpected native inventory change; do not reboot",
    )?;
    require(
        added.iter().all(|(_, row)| {
            row["type"] == "single"
                && row["userdata"]["ryoku-lab-rollback"] == plan.original_boot
                && row["cleanup"].as_str() == Some("")
        }),
        "rollback snapshot tags or retention differ",
    )?;
    let backup_number = added
        .iter()
        .find(|(id, _)| **id != target_id)
        .ok_or("backup snapshot missing")?
        .0
        .to_string();
    require(added.iter().any(|(id, _)| **id == target_id), "printed target is not newly created")?;
    let target_root = open(root, &format!(".snapshots/{target_number}/snapshot"), true)?;
    let target = volume(&target_root)?;
    let backup = volume(&open(root, &format!(".snapshots/{backup_number}/snapshot"), true)?)?;
    let source = &plan.source.as_ref().ok_or("source missing")?.volume;
    validate_rollback_identity(
        &plan.original,
        &plan.namespace,
        source,
        &target,
        &backup,
        default_id()?,
    )?;
    require(
        text(root, GRUB)? == plan.prepared_grub && text(&target_root, GRUB)? == plan.prepared_grub,
        "rollback plugin changed an effective boot config; do not reboot",
    )?;
    verify_common(root, state, plan)?;
    verify_assets(&target_root, plan, false)?;
    require(
        text(&target_root, "etc/fstab")? == plan.prepared_fstab
            && read(&target_root, &format!("etc/{MARKER}"), SMALL)? == BEFORE,
        "target root does not contain the prepared snapshot state",
    )?;
    require(
        linked(root)?.0 == receipt && record_hashes(state)? == plan.records,
        "protected linkage changed during rollback",
    )?;
    let encoded = serde_json::to_vec(plan)?;
    plan.target_number = Some(target_number.into());
    plan.target = Some(target);
    plan.backup_number = Some(backup_number);
    plan.phase = Phase::Armed;
    replace(state, PLAN, &encoded, &serde_json::to_vec(plan)?)?;
    println!("native_root_rollback_default_changed=passed");
    println!("native_root_rollback_reboot_gate=passed");
    Ok(())
}

fn verify_rollback(root: &File, state: &File, plan: &mut Baseline) -> Result<()> {
    require(
        plan.phase == Phase::Armed && plan.prepared_boot.as_ref() != Some(&boot_id(root)?),
        "changed-root verification requires a new boot and an armed plan",
    )?;
    let target = plan.target.as_ref().ok_or("target identity missing")?;
    require(
        volume(root)? == *target && default_id()? == target.id,
        "boot did not select the exact rollback root",
    )?;
    require(
        read(root, &format!("etc/{MARKER}"), SMALL)? == BEFORE,
        "root marker did not roll back",
    )?;
    require(text(root, GRUB)? == plan.prepared_grub, "rollback root boot config changed")?;
    require(
        !text(root, "proc/cmdline")?.split_whitespace().any(|s| s.starts_with("rootflags=")),
        "rollback boot command line introduced unexpected root flags",
    )?;
    verify_retained_original(root, plan)?;
    let source = plan.source.as_ref().ok_or("source receipt missing")?;
    let (receipt, actual) = linked(root)?;
    require(
        receipt == source.receipt
            && actual == source.volume
            && record_hashes(state)? == plan.records,
        "catalog, generation, source identity or journal failed exact rollback persistence",
    )?;
    require(
        matches!(
            verify_new_root_checkpoint(root, receipt.link().clone()),
            Err(ryoku_recovery::native_checkpoint::NativeCheckpointError::NotCurrentRootSnapshot)
        ),
        "historical source incorrectly accepted as a new checkpoint",
    )?;
    save(state, plan, Phase::Verified)?;
    println!("native_root_rollback_exact_root_changed=passed");
    println!("native_root_rollback_marker_reverted=passed");
    println!("native_root_rollback_protected_records_retained=passed");
    Ok(())
}

fn validate_rollback_identity(
    original: &Volume,
    namespace: &Volume,
    source: &Volume,
    target: &Volume,
    backup: &Volume,
    default_id: u64,
) -> Result<()> {
    let ids = [original.id, namespace.id, source.id, target.id, backup.id];
    require(
        ids.iter().all(|id| *id >= 256)
            && ids.iter().collect::<std::collections::BTreeSet<_>>().len() == ids.len()
            && target.filesystem == original.filesystem
            && target.parent == namespace.id
            && target.uuid != original.uuid
            && target.uuid != source.uuid
            && target.origin == source.uuid
            && !target.readonly
            && backup.filesystem == original.filesystem
            && backup.parent == namespace.id
            && backup.uuid != target.uuid
            && backup.readonly
            && backup.origin == original.uuid
            && default_id == target.id,
        "rollback native identities do not match the saved plan; do not reboot",
    )
}

fn verify_retained_original(root: &File, plan: &Baseline) -> Result<()> {
    const MOUNT: &str = "/run/ryoku-rollback-original-root";
    fs::DirBuilder::new().mode(0o700).create(MOUNT)?;
    let options = format!("ro,nosuid,nodev,noexec,subvolid={}", plan.original.id);
    let device = format!("/dev/disk/by-uuid/{}", plan.original.filesystem);
    command("/usr/bin/mount", &["-t", "btrfs", "-o", &options, &device, MOUNT], None)?;
    let original = open(root, MOUNT.trim_start_matches('/'), true)?;
    require(
        volume(&original)? == plan.original
            && rustix::fs::fstatvfs(&original)?
                .f_flag
                .contains(rustix::fs::StatVfsMountFlags::RDONLY),
        "retained original root mount differs; leave fixture for inspection",
    )?;
    let result = (|| -> Result<()> {
        verify_assets(&original, plan, false)?;
        require(
            text(&original, GRUB)? == plan.prepared_grub
                && read(&original, &format!("etc/{MARKER}"), SMALL)? == AFTER,
            "original @ boot configuration or post-source marker changed",
        )
    })();
    drop(original);
    // The exact read-only mount above is owned by this invocation. No force or
    // lazy unmount and no recursive deletion on failure.
    command("/usr/bin/umount", &[MOUNT], None)?;
    fs::remove_dir(MOUNT)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dmi_guard_accepts_only_the_native_class_link_without_relaxing_other_reads() {
        let path = std::env::temp_dir().join(format!("ryoku-dmi-link-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = File::open(&path).unwrap();
        for target in [
            "../../devices/virtual/dmi/id",
            "/sys/devices/virtual/dmi/id",
            "../../devices/virtual/dmi/other",
        ] {
            std::os::unix::fs::symlink(target, path.join("id")).unwrap();
            assert_eq!(
                check_dmi_class_link(&directory).is_ok(),
                target == "../../devices/virtual/dmi/id"
            );
            let error = open(&directory, "id", false).unwrap_err();
            assert_eq!(error.downcast_ref::<rustix::io::Errno>(), Some(&rustix::io::Errno::LOOP));
            fs::remove_file(path.join("id")).unwrap();
        }
        fs::write(path.join("id"), "../../devices/virtual/dmi/id").unwrap();
        assert!(check_dmi_class_link(&directory).is_err());
        fs::remove_file(path.join("id")).unwrap();
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn namespace_fstab_rejects_duplicates_pins_and_foreign_roots() {
        let input = "UUID=fixture / btrfs defaults 0 0\n";
        let prepared = prepare_fstab(input, "fixture", 259).unwrap();
        assert_eq!(
            prepared,
            "UUID=fixture / btrfs defaults 0 0\nUUID=fixture /.snapshots btrfs subvolid=259 0 0\n"
        );
        assert!(prepare_fstab(&prepared, "fixture", 259).is_err());
        assert!(prepare_fstab(input, "foreign", 259).is_err());
        assert!(prepare_fstab("UUID=fixture / btrfs subvol=@ 0 0\n", "fixture", 259).is_err());
    }
    #[test]
    fn grub_transformation_only_removes_exact_reviewed_root_pins() {
        let entry = format!(
            "linux /@/boot/vmlinuz-{RELEASE} root=UUID=fixture rootflags=subvol=@ rw\ninitrd /@/boot/initrd-{RELEASE}\n"
        );
        let original = entry.repeat(3);
        assert_eq!(
            prepare_grub(&original, "fixture").unwrap(),
            original.replace(" rootflags=subvol=@", "")
        );
        for invalid in [
            entry.repeat(2),
            original.replace("subvol=@", "subvol=other"),
            original.replace("root=UUID=fixture", "root=UUID=foreign"),
            original.replace(" rw", " rootflags=subvolid=256 rw"),
        ] {
            assert!(prepare_grub(&invalid, "fixture").is_err());
        }
    }
    #[test]
    fn generic_initrd_listing_distinguishes_absent_members_from_failed_observation() {
        let listing = format!(
            "Arguments: --force --no-hostonly\ndracut modules:\nbase\n-rw-r--r-- 1 root root 10 Jan 1 usr/lib/modules/{RELEASE}/kernel/module.ko\ndracut cmdline:\n"
        );
        assert_eq!(generic_initrd_listing(&listing).unwrap(), (false, false));
        assert!(affirmative_initrd("").is_ok());
        for invalid in [
            "".into(),
            listing.replace("--no-hostonly", "--hostonly"),
            listing.replace("dracut cmdline:", "incomplete"),
            format!("{listing}squash-root.img\n"),
        ] {
            assert!(generic_initrd_listing(&invalid).is_err());
        }
        let with_members = format!(
            "{listing}-rw-r--r-- 1 root root 10 Jan 1 etc/cmdline\n-rw-r--r-- 1 root root 10 Jan 1 etc/cmdline.d/95root-dev.conf\n"
        );
        assert_eq!(generic_initrd_listing(&with_members).unwrap(), (true, true));
        for invalid in ["root=UUID=fixture rootflags=subvol=@", "root=UUID=fixture rootflags=rw"] {
            assert!(affirmative_initrd(invalid).is_err());
        }
        assert!(affirmative_initrd("root=UUID=fixture rootfstype=btrfs").is_ok());
    }
    #[test]
    fn native_rollback_gate_requires_exact_origin_mount_namespace_and_default() {
        let original = Volume {
            filesystem: "fs".into(),
            id: 256,
            parent: 5,
            uuid: "original".into(),
            origin: "nil".into(),
            readonly: false,
        };
        let namespace =
            Volume { id: 259, parent: 256, uuid: "namespace".into(), ..original.clone() };
        let source = Volume {
            id: 270,
            parent: 259,
            uuid: "source".into(),
            origin: original.uuid.clone(),
            readonly: true,
            ..original.clone()
        };
        let target = Volume {
            id: 275,
            parent: 259,
            uuid: "target".into(),
            origin: source.uuid.clone(),
            ..original.clone()
        };
        let backup = Volume {
            id: 274,
            parent: 259,
            uuid: "backup".into(),
            origin: original.uuid.clone(),
            readonly: true,
            ..original.clone()
        };
        let check = |target: &Volume, backup: &Volume, default| {
            validate_rollback_identity(&original, &namespace, &source, target, backup, default)
        };
        assert!(check(&target, &backup, 275).is_ok());
        assert!(check(&target, &backup, 256).is_err());
        for changed in [
            Volume { id: source.id, ..target.clone() },
            Volume { parent: 5, ..target.clone() },
            Volume { origin: original.uuid.clone(), ..target.clone() },
            Volume { filesystem: "foreign".into(), ..target.clone() },
            Volume { readonly: true, ..target.clone() },
        ] {
            assert!(check(&changed, &backup, changed.id).is_err());
        }
        assert!(check(&target, &Volume { readonly: false, ..backup }, 275).is_err());
    }
    #[test]
    fn native_numbers_and_inventory_cannot_adopt_ambiguous_results() {
        assert_eq!(parse_default("ID 256 gen 10 top level 5 path @\n").unwrap(), 256);
        for invalid in ["", "ID 0256 path @", "garbage 256", "ID 0", "ID 256\nID 300\n"] {
            assert!(parse_default(invalid).is_err());
        }
        assert!(parse_inventory(r#"{"root":[{"number":2},{"number":2}]}"#).is_err());
        assert!(parse_inventory(r#"{"root":[{"number":"02"}]}"#).is_err());
        assert!(parse_inventory(r#"{"root":[{"number":"2"}]}"#).is_ok());
        assert!(parse_inventory(r#"{"root":[],"foreign":[]}"#).is_err());
    }
}
