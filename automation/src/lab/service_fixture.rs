//! Fixed input staging and strict public evidence for the optional service fixture.
use crate::{
    guest, preflight, process, profile_bundle,
    support::{Result, require},
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
    time::Duration,
};

pub(crate) const NAMES: [&str; 3] =
    ["ryoku-service-fixture", "ryoku-user-service", "ryoku-root-helper"];
const INPUTS: &str = "/run/ryoku-service-fixture-input";
pub(crate) const PROBE: &str = "/usr/local/sbin/ryoku-native-service-fixture";
const TOOLS: &[&str] = &[
    "rpm",
    "systemctl",
    "loginctl",
    "journalctl",
    "systemd-sysusers",
    "getent",
    "groupadd",
    "useradd",
    "usermod",
    "runuser",
    "restorecon",
    "matchpathcon",
    "bash",
];
const UNITS: &[&str] = &["user_manager", "user_runtime", "user_bus_socket"];
const RUN_MARKERS: &[(&str, &str)] = &[
    ("native_service_logind_user_managers", "passed"),
    ("native_service_user_unit_activation", "passed"),
    ("native_service_helper_socket_activation", "passed"),
    ("native_service_native_dac_denials", "passed"),
    ("native_service_root_policy", "passed"),
    ("native_service_operator_policy", "passed"),
    ("native_service_assisted_kernel_policy", "passed"),
    ("native_service_dac_modes_restored", "passed"),
    ("native_service_duplicate_start", "passed"),
    ("native_service_fresh_process_restart", "passed"),
    ("native_service_empty_journals_retained", "passed"),
    ("native_service_task_units_stopped", "passed"),
    ("native_service_artifacts", "retained_for_disposable_guest_teardown"),
    ("native_service_rpm_install", "not_tested"),
    ("native_service_interactive_login", "not_tested"),
    ("native_service_accepted_transactions", "not_tested"),
];

fn package_value(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+:~-".contains(&b))
}

/// Exact public rows shared by full-proof validation and the report sanitizer.
/// Inventory accepts only the thirteen tool and three unit keys above.
pub(crate) fn public_row(line: &str) -> bool {
    let Some((key, value)) = line.split_once('=') else {
        return false;
    };
    match key {
        "service_fixture_sha256" | "user_service_sha256" | "root_helper_sha256" => {
            return profile_bundle::valid_digest(value);
        }
        "native_service_guest_artifacts" => return value == "verified",
        "native_service_fixture_preflight" => return value == "passed",
        _ => {}
    }
    if RUN_MARKERS.iter().any(|(name, expected)| key == *name && value == *expected) {
        return true;
    }
    package_value(value)
        && (TOOLS.iter().any(|name| key == format!("native_service_tool_{name}_rpm"))
            || UNITS.iter().any(|name| key == format!("native_service_unit_{name}_rpm")))
}
pub(crate) fn validate(text: &str, full: bool) -> Result<String> {
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').ok_or("invalid service fixture evidence row")?;
        require(values.insert(key, value).is_none(), "duplicate service fixture evidence key")?;
    }
    require(
        values.remove("native_service_fixture_preflight") == Some("passed"),
        "service fixture preflight incomplete",
    )?;
    let mut safe =
        BTreeMap::from([("native_service_fixture_preflight".to_owned(), "passed".to_owned())]);
    for (prefix, suffix, names) in
        [("native_service_tool_", "_rpm", TOOLS), ("native_service_unit_", "_rpm", UNITS)]
    {
        for name in names {
            let key = format!("{prefix}{name}{suffix}");
            let value =
                values.remove(key.as_str()).ok_or("missing service fixture native inventory")?;
            require(package_value(value), "invalid service fixture package inventory")?;
            safe.insert(key, value.to_owned());
        }
    }
    if full {
        for (key, expected) in RUN_MARKERS {
            require(
                values.remove(key) == Some(*expected),
                "service fixture pass/scope marker incomplete",
            )?;
            safe.insert((*key).to_owned(), (*expected).to_owned());
        }
    }
    require(values.is_empty(), "unknown service fixture evidence or private payload")?;
    Ok(safe.into_iter().map(|(key, value)| format!("{key}={value}\n")).collect())
}

fn native(command: &mut Command) -> Result<()> {
    let result = process::run(command, Duration::from_secs(30), 64 * 1024)?;
    if result.timed_out
        || !result.status.success()
        || result.stdout_truncated
        || result.stderr_truncated
    {
        eprintln!(
            "service staging command {command:?}\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return Err("native service staging command failed".into());
    }
    Ok(())
}
fn absent(path: &Path) -> Result<()> {
    require(
        matches!(fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound),
        "refuse existing service fixture staging path",
    )
}
pub(crate) fn guest_stage(hashes: [&str; 3]) -> Result<()> {
    guest::fixture_only()?;
    require(
        rustix::process::geteuid().is_root()
            && hashes.iter().all(|hash| profile_bundle::valid_digest(hash)),
        "service staging requires root and canonical hashes",
    )?;
    require(
        fs::read_to_string("/sys/fs/selinux/enforce")?.trim() == "1",
        "service fixture requires enforcing SELinux",
    )?;
    absent(Path::new(INPUTS))?;
    absent(Path::new(PROBE))?;
    preflight::input_binary("/usr/bin/install")?;
    let targets = ["fixture", "ryoku-service", "ryoku-helper"];
    // Read-only validation of every upload precedes any staging creation.
    for (name, hash) in NAMES.iter().zip(hashes) {
        let source = format!("/tmp/{name}.upload");
        let file = upload(Path::new(&source))?;
        require(
            profile_bundle::digest(Path::new(&source))? == hash,
            "uploaded service input digest mismatch",
        )?;
        preflight::input_binary(&source)?;
        drop(file);
    }
    fs::create_dir(INPUTS)?;
    fs::set_permissions(INPUTS, fs::Permissions::from_mode(0o700))?;
    for ((name, target), hash) in NAMES.iter().zip(targets).zip(hashes) {
        let source = format!("/tmp/{name}.upload");
        let file = upload(Path::new(&source))?;
        let before = file.metadata()?;
        let target = Path::new(INPUTS).join(target);
        let mut frozen =
            fs::OpenOptions::new().write(true).create_new(true).mode(0o755).open(&target)?;
        let copied = std::io::copy(&mut (&file).take(256 * 1024 * 1024 + 1), &mut frozen)?;
        require(
            copied == before.len()
                && file.metadata()?.len() == before.len()
                && file.metadata()?.modified()? == before.modified()?,
            "service upload changed during freeze",
        )?;
        frozen.set_permissions(fs::Permissions::from_mode(0o755))?;
        frozen.sync_all()?;
        require(profile_bundle::digest(&target)? == hash, "frozen service input digest mismatch")?;
        preflight::input_binary(target.to_str().ok_or("invalid fixed input path")?)?;
    }
    File::open(INPUTS)?.sync_all()?;
    native(Command::new("/usr/bin/install").args([
        "-Z",
        "-m",
        "0755",
        "/run/ryoku-service-fixture-input/fixture",
        PROBE,
    ]))?;
    require(
        profile_bundle::digest(Path::new(PROBE))? == hashes[0],
        "installed service fixture digest mismatch",
    )?;
    println!("native_service_guest_artifacts=verified");
    Ok(())
}
fn upload(path: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as _)
        .open(path)?;
    let meta = file.metadata()?;
    require(
        meta.is_file() && meta.nlink() == 1 && meta.len() > 0 && meta.len() <= 128 * 1024 * 1024,
        "unsafe service fixture upload",
    )?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn evidence(full: bool) -> String {
        let mut text = "native_service_fixture_preflight=passed\n".to_string();
        for name in TOOLS {
            text += &format!("native_service_tool_{name}_rpm=systemd-261-1.x86_64\n");
        }
        for name in UNITS {
            text += &format!("native_service_unit_{name}_rpm=dbus-provider-1.x86_64\n");
        }
        if full {
            for (key, value) in RUN_MARKERS {
                text += &format!("{key}={value}\n");
            }
        }
        text
    }
    #[test]
    fn accepts_exact_inventory_and_scope_but_rejects_incomplete_or_private_evidence() {
        for full in [false, true] {
            let valid = evidence(full);
            assert!(validate(&valid, full).is_ok());
            assert!(validate(&(valid.clone() + "private_path=/home/user\n"), full).is_err());
            assert!(
                validate(&(valid.clone() + "native_service_fixture_preflight=passed\n"), full)
                    .is_err()
            );
            assert!(
                validate(&valid.replace("systemd-261-1.x86_64", "/private/package"), full).is_err()
            );
            assert!(
                validate(&valid.replacen("native_service_fixture_preflight=passed\n", "", 1), full)
                    .is_err()
            );
        }
        assert!(validate(&evidence(false), true).is_err());
        assert!(
            validate(
                &evidence(true).replace(
                    "native_service_rpm_install=not_tested",
                    "native_service_rpm_install=passed"
                ),
                true
            )
            .is_err()
        );
    }

    #[test]
    fn complete_validated_proof_hashes_and_stage_row_survive_public_report_filter() {
        let proof = validate(&evidence(true), true).unwrap();
        assert_eq!(
            proof.lines().filter(|line| crate::support::public_evidence(line)).count(),
            proof.lines().count()
        );
        assert!(crate::support::public_evidence("native_service_guest_artifacts=verified"));
        for key in ["service_fixture_sha256", "user_service_sha256", "root_helper_sha256"] {
            assert!(crate::support::public_evidence(&format!("{key}={}", "a".repeat(64))));
            assert!(!crate::support::public_evidence(&format!("{key}={}", "A".repeat(64))));
            assert!(!crate::support::public_evidence(&format!("{key}=private-token")));
        }
        assert!(crate::support::public_evidence(
            "native_service_tool_systemctl_rpm=systemd-2:261~rc1+2.x86_64"
        ));
        for row in [
            "native_service_tool_private_rpm=package-1.x86_64",
            "native_service_unit_private_rpm=package-1.x86_64",
            "native_service_tool_getent_rpm=/home/private",
            "native_service_unit_user_manager_rpm=private package",
            "native_service_root_policy=failed",
            "native_service_rpm_install=passed",
            "native_service_guest_artifacts=unverified",
            "native_service_uid=23171",
            "native_service_private=token",
        ] {
            assert!(!crate::support::public_evidence(row), "unexpected public row: {row}");
        }
    }
}
