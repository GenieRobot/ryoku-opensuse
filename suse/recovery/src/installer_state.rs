//! Exact package/absence vocabulary for installer-lifecycle-v1 checkpoints.
//! Ordinary checkpoints keep their original package requirements.
use crate::{Identifier, OperationReason, PackageName, PackageRevision, Revision};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};
pub const MARKER: &str = "installer-lifecycle-v1";
pub const PACKAGES: &[&str] = &[
    "ryoku-desktop",
    "ryoku-desktop-fonts",
    "ryoku-desktop-runtime",
    "ryoku-helper",
    "ryoku-profile-defaults",
    "ryoku-quickshell",
    "ryoku-tools",
    "ryoku-tools-launcher-status",
    "ryoku-tools-state-init",
    "ryoku-ui-proof",
];
pub fn reason() -> OperationReason {
    OperationReason::Other(Identifier::new(MARKER).expect("compiled marker"))
}
pub fn is_installer(reason: &OperationReason) -> bool {
    matches!(reason,OperationReason::Other(id) if id.as_str()==MARKER)
}
/// Validate every recognized entry, including affirmative absent records. An
/// unknown, missing, duplicate, newer version or post-transaction value refuses.
pub fn validate_inventory(packages: &[PackageRevision]) -> Result<(), String> {
    if packages.len() != PACKAGES.len() {
        return Err("installer package inventory is incomplete".into());
    }
    for (package, name) in packages.iter().zip(PACKAGES) {
        if package.package_name.as_str() != *name
            || package.after_nevra.is_some()
            || package.upstream_revision.is_some()
        {
            return Err("unknown or reordered installer package identity".into());
        }
        if let Some(revision) = &package.before_nevra {
            let version = if *name == "ryoku-quickshell" { "0.3.2" } else { "0.1.0" };
            let suffix = revision
                .as_str()
                .strip_prefix(&format!("{name}-0:{version}-"))
                .ok_or("unsupported installer package version")?;
            let (release, arch) = suffix.rsplit_once('.').ok_or("missing package architecture")?;
            let expected_arch = if matches!(*name, "ryoku-desktop-fonts" | "ryoku-profile-defaults")
            {
                "noarch"
            } else {
                "x86_64"
            };
            if arch != expected_arch
                || release.is_empty()
                || release.len() > 64
                || !release.as_bytes()[0].is_ascii_digit()
                || !release.bytes().all(|b| b.is_ascii_alphanumeric() || b"._".contains(&b))
            {
                return Err("unsupported installer release or architecture".into());
            }
        }
    }
    Ok(())
}
pub fn from_installed(
    installed: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<PackageRevision>, String> {
    if installed.keys().any(|name| !PACKAGES.contains(&name.as_str())) {
        return Err("unknown installed Ryoku package".into());
    }
    let packages = PACKAGES
        .iter()
        .map(|name| {
            Ok(PackageRevision {
                package_name: PackageName::new(*name).map_err(|e| format!("{e:?}"))?,
                before_nevra: installed
                    .get(*name)
                    .map(|version| {
                        Revision::new(format!("{name}-{version}")).map_err(|e| format!("{e:?}"))
                    })
                    .transpose()?,
                after_nevra: None,
                upstream_revision: None,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    validate_inventory(&packages)?;
    Ok(packages)
}
/// Query one fixed inventory from the native or independently verified snapshot
/// root. Bounded child output and timeout apply even to a corrupt RPM database.
pub fn query(root: Option<&Path>) -> Result<Vec<PackageRevision>, String> {
    let mut command = Command::new("/usr/bin/timeout");
    command.env_clear().env("LC_ALL", "C").args([
        "--signal=TERM",
        "--kill-after=1s",
        "30s",
        "/usr/bin/rpm",
    ]);
    if let Some(root) = root {
        command.arg("--root").arg(root);
    }
    let mut child = command
        .args(["-qa", "--qf", "%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("RPM stdout missing")?;
    let stderr = child.stderr.take().ok_or("RPM stderr missing")?;
    let read = |mut stream: Box<dyn Read + Send>, limit: u64| -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        stream.by_ref().take(limit + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > limit {
            return Err("RPM inventory exceeds bound".into());
        }
        Ok(bytes)
    };
    let out = std::thread::spawn(move || read(Box::new(stdout), 4 * 1024 * 1024));
    let err = std::thread::spawn(move || read(Box::new(stderr), 16 * 1024));
    let status = child.wait().map_err(|e| e.to_string())?;
    let bytes = out.join().map_err(|_| "RPM reader failed")??;
    let errors = err.join().map_err(|_| "RPM reader failed")??;
    if !status.success() || !errors.is_empty() {
        return Err("RPM inventory query failed".into());
    }
    parse_inventory(&bytes)
}
fn parse_inventory(bytes: &[u8]) -> Result<Vec<PackageRevision>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid RPM inventory")?;
    let mut installed = std::collections::BTreeMap::new();
    for line in text.lines() {
        let (name, version) = line.split_once('\t').ok_or("malformed RPM inventory")?;
        if name.starts_with("ryoku-") && installed.insert(name.into(), version.into()).is_some() {
            return Err("duplicate installed Ryoku package".into());
        }
    }
    from_installed(&installed)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absence_is_explicit_and_inventory_is_closed() {
        let empty = from_installed(&Default::default()).unwrap();
        assert_eq!(empty.len(), PACKAGES.len());
        assert!(empty.iter().all(|p| p.before_nevra.is_none()));
        assert!(validate_inventory(&empty[..empty.len() - 1]).is_err());
        let mut wrong = empty.clone();
        wrong[0].package_name = PackageName::new("ryoku-unknown").unwrap();
        assert!(validate_inventory(&wrong).is_err());
        let mut wrong = empty.clone();
        wrong[0].before_nevra = Some(Revision::new("ryoku-desktop-0:9.0-1.x86_64").unwrap());
        assert!(validate_inventory(&wrong).is_err());
        let mut wrong = empty.clone();
        wrong.swap(0, 1);
        assert!(validate_inventory(&wrong).is_err());
        let mut wrong = empty;
        wrong[0].after_nevra = Some(Revision::new("unknown").unwrap());
        assert!(validate_inventory(&wrong).is_err());
    }
    #[test]
    fn unrecognized_and_duplicate_database_rows_refuse() {
        assert!(parse_inventory(b"ryoku-unknown\t0:0.1.0-1.x86_64\n").is_err());
        assert!(
            parse_inventory(b"ryoku-tools\t0:0.1.0-1.x86_64\nryoku-tools\t0:0.1.0-1.x86_64\n")
                .is_err()
        );
        assert!(parse_inventory(b"ryoku-tools\t0:0.1.0-1.x86_64\n").is_ok());
    }
    #[test]
    fn only_exact_marker_enters_installer_contract() {
        assert!(is_installer(&reason()));
        for reason in [
            OperationReason::PackageChange,
            OperationReason::ConfigurationRestore,
            OperationReason::Other(Identifier::new("installer-lifecycle-v2").unwrap()),
            OperationReason::Other(Identifier::new("installer-lifecycle").unwrap()),
        ] {
            assert!(!is_installer(&reason));
        }
    }
}
