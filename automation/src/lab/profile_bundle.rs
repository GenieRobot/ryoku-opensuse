//! Optional validation of the inactive profile component in a fresh lab guest.
//! An explicit artifact hash binds a local build; it is not signer trust.
use crate::{
    guest, preflight, process,
    support::{Result, require},
};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

pub(crate) const PACKAGE: &str = "ryoku-profile-defaults";
const INPUTS: &str = "/run/ryoku-profile-probe-inputs";
const LIMIT: u64 = 256 * 1024 * 1024;
const RPM_LIMIT: u64 = 1024 * 1024;
const FORMAT: &str = "PKG\t%{NAME}\t%{EPOCHNUM}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\t%{FILEDIGESTALGO}\n[FILE\t%{FILENAMES}\t%{FILEDIGESTS}\t%{FILEMODES}\t%{FILEUSERNAME}\t%{FILEGROUPNAME}\t%{LONGFILESIZES}\t%{FILEFLAGS}\n]";
const INVENTORY: &str = concat!(
    "PKG\tryoku-profile-defaults\t0\t0.1.0\t1\tnoarch\t8\n",
    "FILE\t/usr/share/doc/packages/ryoku-profile-defaults\t\t16877\troot\troot\t0\t0\n",
    "FILE\t/usr/share/doc/packages/ryoku-profile-defaults/README.md\tbff6974a013befd0a7cc8b024004c344783004b05844faf02358938fa6c962e2\t33188\troot\troot\t1054\t2\n",
    "FILE\t/usr/share/licenses/ryoku-profile-defaults\t\t16877\troot\troot\t0\t0\n",
    "FILE\t/usr/share/licenses/ryoku-profile-defaults/LICENSE\t3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986\t33188\troot\troot\t35149\t128\n",
    "FILE\t/usr/share/ryoku\t\t16877\troot\troot\t0\t0\n",
    "FILE\t/usr/share/ryoku/config\t\t16877\troot\troot\t0\t0\n",
    "FILE\t/usr/share/ryoku/config/profile.json\t605ac095a781ee4b3908272591f15b56f3a9f5d9a251e177e7d60dbbade02ba5\t33188\troot\troot\t123\t0\n",
);
pub(crate) const MARKERS: &[&str] = &[
    "native_profile_rpm_proof=passed",
    "native_profile_explicit_provisioning=passed",
    "native_profile_exact_reopen=passed",
    "native_profile_raw_fidelity=passed",
    "native_profile_four_layers=passed",
    "native_profile_optional_absence=passed",
    "native_profile_rename_replacement_refusal=passed",
    "native_profile_tamper_refusal=passed",
    "native_profile_exact_cleanup=passed",
];
pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn rpm() -> Command {
    let mut command = Command::new("/usr/bin/rpm");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("HOME", "/var/empty")
        .stdin(Stdio::null());
    command
}
pub(crate) fn digest(path: &Path) -> Result<String> {
    let text = preflight::output(Command::new("/usr/bin/sha256sum").arg(path))?;
    let value = text.split_whitespace().next().ok_or("missing checksum")?;
    require(valid_digest(value), "invalid checksum")?;
    Ok(value.into())
}
fn regular(path: &Path, limit: u64) -> Result<File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as _)
        .open(path)?;
    let metadata = file.metadata()?;
    require(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.len() > 0
            && metadata.len() <= limit,
        "unsafe artifact type, links or size",
    )?;
    Ok(file)
}
fn validate_inventory(text: &str) -> Result<()> {
    require(text == INVENTORY, "defaults RPM package or exact payload inventory mismatch")
}
fn validate_hooks(scripts: &str, triggers: &str) -> Result<()> {
    require(
        scripts.is_empty() && triggers == "(none)\n(none)\n(none)\n",
        "defaults RPM contains executable hooks",
    )
}
pub(crate) fn inspect_rpm(path: &Path, expected: &str) -> Result<u64> {
    require(valid_digest(expected), "invalid expected RPM checksum")?;
    let file = regular(path, RPM_LIMIT)?;
    require(digest(path)? == expected, "defaults RPM checksum mismatch")?;
    // Preserve native digest/signature checking; no --nodigest/--nosignature.
    preflight::output(rpm().arg("--checksig").arg(path))?;
    let inventory =
        preflight::output(rpm().args(["--query", "--package", "--queryformat", FORMAT]).arg(path))?;
    validate_inventory(&inventory)?;
    let scripts = preflight::output(
        rpm().args(["--query", "--package", "--scripts", "--triggers"]).arg(path),
    )?;
    let triggers = preflight::output(
        rpm()
            .args([
                "--query",
                "--package",
                "--queryformat",
                "%{FILETRIGGERSCRIPTS}\n%{TRANSFILETRIGGERSCRIPTS}\n%{TRIGGERSCRIPTS}\n",
            ])
            .arg(path),
    )?;
    validate_hooks(&scripts, &triggers)?;
    require(digest(path)? == expected, "defaults RPM changed during inspection")?;
    Ok(file.metadata()?.len())
}
pub(crate) fn validate_markers(text: &str) -> Result<()> {
    require(
        text.lines().eq(MARKERS.iter().copied()),
        "missing, repeated or unexpected profile proof marker",
    )
}
fn absent(path: impl AsRef<Path>) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("profile fixture target already exists".into()),
    }
}
fn guest_guards() -> Result<File> {
    guest::fixture_only()?;
    require(
        rustix::process::geteuid().as_raw() == 0 && rustix::process::getegid().as_raw() == 0,
        "profile fixture requires root user and group",
    )?;
    let root = File::open("/")?;
    let record = ryoku_recovery::machine_state::read_machine_state_record(&root)?;
    let state = File::from(
        ryoku_recovery::state_mount::collect_and_open_state_mount(&root, record.identity())?.0,
    );
    match rustix::fs::statat(&state, "profiles", rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => {}
        _ => return Err("profile store must be absent in the fresh fixture".into()),
    }
    Ok(state)
}
pub(crate) fn guest_preflight() -> Result<()> {
    let _state = guest_guards()?;
    absent(INPUTS)?;
    for path in [
        "/usr/share/ryoku",
        "/usr/share/doc/packages/ryoku-profile-defaults",
        "/usr/share/licenses/ryoku-profile-defaults",
    ] {
        absent(path)?;
    }
    let result = process::run(rpm().args(["--query", PACKAGE]), Duration::from_secs(30), 4096)?;
    require(
        !result.timed_out
            && !result.stdout_truncated
            && !result.stderr_truncated
            && result.status.code() == Some(1)
            && result.stdout == b"package ryoku-profile-defaults is not installed\n"
            && result.stderr.is_empty(),
        "default package absence is unproven",
    )?;
    // Verify the same native tools used below before creating input storage.
    preflight::input_binary("/usr/bin/rpm")?;
    preflight::input_binary("/usr/bin/sha256sum")?;
    println!("native_profile_guest_prerequisites=passed");
    Ok(())
}
fn freeze_upload(
    source: &str,
    name: &str,
    limit: u64,
    mode: u32,
    expected: &str,
) -> Result<PathBuf> {
    let input = regular(Path::new(source), limit)?;
    let before = input.metadata()?;
    let path = Path::new(INPUTS).join(name);
    let mut output = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    let copied = std::io::copy(&mut (&input).take(limit + 1), &mut output)?;
    require(
        copied == before.len()
            && input.metadata()?.len() == before.len()
            && input.metadata()?.modified()? == before.modified()?,
        "uploaded input changed during freeze",
    )?;
    output.set_permissions(fs::Permissions::from_mode(mode))?;
    output.sync_all()?;
    require(digest(&path)? == expected, "guest frozen artifact checksum mismatch")?;
    Ok(path)
}
pub(crate) fn guest_run(probe_sha: &str, rpm_sha: &str) -> Result<()> {
    require(valid_digest(probe_sha) && valid_digest(rpm_sha), "invalid artifact checksum")?;
    // Repeat all guards immediately before any write, not merely at host preflight.
    guest_preflight()?;
    fs::create_dir(INPUTS)?;
    fs::set_permissions(INPUTS, fs::Permissions::from_mode(0o700))?;
    let probe = freeze_upload("/tmp/ryoku-profile-probe.upload", "probe", LIMIT, 0o500, probe_sha)?;
    let package = freeze_upload(
        "/tmp/ryoku-profile-defaults.upload",
        "defaults.rpm",
        RPM_LIMIT,
        0o400,
        rpm_sha,
    )?;
    File::open(INPUTS)?.sync_all()?;
    preflight::input_binary(probe.to_str().ok_or("invalid fixed path")?)?;
    inspect_rpm(&package, rpm_sha)?;
    preflight::output(rpm().args(["--install", "--test"]).arg(&package))?;
    // The seeded guest is disposable and has no previous package or path. This
    // plain native install retains every RPM dependency/digest/signature check.
    preflight::output(rpm().arg("--install").arg(&package))?;
    let inventory = preflight::output(rpm().args(["--query", "--queryformat", FORMAT, PACKAGE]))?;
    validate_inventory(&inventory)?;
    let verified = preflight::output(rpm().args(["--verify", PACKAGE]))?;
    require(verified.is_empty(), "installed defaults metadata or bytes differ from RPM")?;
    let defaults = ryoku_recovery::rpm_defaults::VerifiedPackagedDefaults::load()?;
    require(
        defaults.bytes() == include_bytes!("../../../packaging/rpm/defaults/profile.json"),
        "installed generic default bytes differ from reviewed source",
    )?;
    guest_guards()?;
    require(digest(&probe)? == probe_sha, "guest probe changed before execution")?;
    let result = process::run(
        Command::new(&probe)
            .arg("--run")
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin")
            .env("LC_ALL", "C")
            .stdin(Stdio::null()),
        Duration::from_secs(90),
        64 * 1024,
    )?;
    // Raw failure text remains in the harness's private guest log.
    eprint!("{}", String::from_utf8_lossy(&result.stderr));
    require(
        !result.timed_out
            && !result.stdout_truncated
            && !result.stderr_truncated
            && result.status.success(),
        "profile bundle probe failed",
    )?;
    let text = String::from_utf8(result.stdout)?;
    validate_markers(&text)?;
    print!(
        "native_profile_guest_artifacts=verified\nnative_profile_defaults_install=passed\n{text}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn rpm_preflight_refuses_links_special_or_oversize_inputs_before_native_inspection() {
        let path =
            std::env::temp_dir().join(format!("ryoku-profile-input-check-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let empty = path.join("empty");
        File::create(&empty).unwrap();
        assert!(inspect_rpm(&empty, &"a".repeat(64)).is_err());
        let oversized = path.join("oversized");
        File::create(&oversized).unwrap().set_len(RPM_LIMIT + 1).unwrap();
        assert!(inspect_rpm(&oversized, &"a".repeat(64)).is_err());
        let link = path.join("link");
        symlink(&oversized, &link).unwrap();
        assert!(inspect_rpm(&link, &"a".repeat(64)).is_err());
        assert!(inspect_rpm(&path, &"a".repeat(64)).is_err());
        assert!(inspect_rpm(&empty, "bad-checksum").is_err());
        fs::remove_file(link).unwrap();
        fs::remove_file(oversized).unwrap();
        fs::remove_file(empty).unwrap();
        fs::remove_dir(path).unwrap();
    }
    #[test]
    fn package_contract_rejects_extras_wrong_owner_mode_payload_and_hooks() {
        validate_inventory(INVENTORY).unwrap();
        for wrong in [
            INVENTORY.replace("noarch", "x86_64"),
            INVENTORY.replace("33188", "33261"),
            INVENTORY.replace("root\troot", "tester\troot"),
            INVENTORY.replace("605ac095", "00000000"),
            format!("{INVENTORY}FILE\t/etc/foreign\n"),
            INVENTORY.replace("profile.json", "profile.json.extra"),
        ] {
            assert!(validate_inventory(&wrong).is_err());
        }
        validate_hooks("", "(none)\n(none)\n(none)\n").unwrap();
        assert!(validate_hooks("postinstall scriptlet", "(none)\n(none)\n(none)\n").is_err());
        assert!(validate_hooks("", "(none)\n/bin/true\n(none)\n").is_err());
    }
    #[test]
    fn fixed_markers_and_artifact_digests_are_exact() {
        let good = format!("{}\n", MARKERS.join("\n"));
        validate_markers(&good).unwrap();
        for wrong in [
            good.replacen("passed", "failed", 1),
            format!("{good}extra=passed\n"),
            MARKERS[1..].join("\n"),
        ] {
            assert!(validate_markers(&wrong).is_err());
        }
        assert!(valid_digest(&"a".repeat(64)));
        for wrong in ["a".repeat(63), "A".repeat(64), format!("{};", "a".repeat(63))] {
            assert!(!valid_digest(&wrong));
        }
    }
}
