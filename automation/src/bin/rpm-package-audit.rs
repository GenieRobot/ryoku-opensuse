//! Read-only inventory and ownership checks for the explicit issue 23 RPM set.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const PACKAGES: &[&str] = &[
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
const FILE_QUERY: &str =
    "[%{FILENAMES:json}\t%{FILEMODES}\t%{FILEUSERNAME:json}\t%{FILEGROUPNAME:json}\t%{FILECAPS}\n]";
const PICKER_LICENSES: &[&str] = &[
    "ryogami-picker-LICENSE",
    "ryogami-picker-NOTICE",
    "ryogami-picker-RobotoCondensed-LICENSE-Apache-2.0.txt",
    "ryogami-picker-SymbolsNerdFont-LICENSE-MIT.txt",
];

fn require(condition: bool, message: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(message.into()) }
}

fn query(path: &Path, arguments: &[&str]) -> Result<String> {
    let result = Command::new("/usr/bin/rpm").arg("-qp").args(arguments).arg(path).output()?;
    require(result.status.success(), "RPM query failed")?;
    Ok(String::from_utf8(result.stdout)?)
}

#[derive(Debug)]
struct FileRecord {
    path: String,
    mode: u32,
}

fn files(input: &str) -> Result<Vec<FileRecord>> {
    let mut result = Vec::new();
    let mut paths = BTreeSet::new();
    for line in input.lines() {
        let row: Vec<_> = line.split('\t').collect();
        require(row.len() == 5, "invalid RPM file row")?;
        let path: String = serde_json::from_str(row[0])?;
        require(
            path.starts_with("/usr/")
                && !path.bytes().any(|byte| byte < 32 || byte == 127)
                && Path::new(&path)
                    .components()
                    .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
                && !path.split('/').skip(1).any(|part| part.is_empty() || part == "."),
            "RPM path is outside the immutable /usr payload or is not canonical",
        )?;
        require(paths.insert(path.clone()), "duplicate path in one RPM")?;
        let mode: u32 = row[1].parse()?;
        require(
            serde_json::from_str::<String>(row[2])? == "root"
                && serde_json::from_str::<String>(row[3])? == "root",
            "RPM payload is not root:root",
        )?;
        require(row[4].is_empty() || row[4] == "(none)", "unexpected file capabilities")?;
        require(mode & 0o6000 == 0, "unexpected set-ID file")?;
        let kind = mode & 0o170000;
        require(matches!(kind, 0o040000 | 0o100000 | 0o120000), "unexpected RPM file type")?;
        require(kind == 0o120000 || mode & 0o022 == 0, "writable RPM payload")?;
        result.push(FileRecord { path, mode });
    }
    require(!result.is_empty(), "empty RPM file inventory")?;
    Ok(result)
}

fn insert_paths(
    owner: &str,
    records: &[FileRecord],
    owners: &mut BTreeMap<String, (String, u32)>,
) -> Result<()> {
    for record in records {
        if let Some((previous, mode)) = owners.get(&record.path) {
            require(
                mode & 0o170000 == 0o040000
                    && record.mode & 0o170000 == 0o040000
                    && *mode == record.mode,
                &format!("payload conflict: {} ({previous}, {owner})", record.path),
            )?;
        } else {
            owners.insert(record.path.clone(), (owner.to_owned(), record.mode));
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("RPM audit failed: {error}");
        std::process::exit(1);
    }
}

fn qml_imports(requires: &str, provides: &BTreeSet<String>) -> Result<()> {
    for required in
        ["qt6qmlimport(QtQuick)", "qt6qmlimport(QtMultimedia)", "qt6qmlimport(Quickshell)"]
    {
        require(
            requires.lines().any(|line| line == required),
            "Qt6 import scanner output is absent or incomplete",
        )?;
    }
    for line in requires.lines() {
        if ["qt6qmlimport(Quickshell", "qt6qmlimport(Ryoku.", "qt6qmlimport(shell."]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        {
            require(provides.contains(line), &format!("unresolved internal QML import: {line}"))?;
        }
    }
    Ok(())
}

fn picker_licenses(records: &[FileRecord]) -> Result<()> {
    for name in PICKER_LICENSES {
        let mut matching = records.iter().filter(|record| {
            let path = Path::new(&record.path);
            path.starts_with("/usr/share/licenses/ryoku-desktop-runtime")
                && path.file_name().is_some_and(|value| value == *name)
        });
        require(
            matching.next().is_some_and(|record| record.mode & 0o170000 == 0o100000)
                && matching.next().is_none(),
            &format!("missing or ambiguous installed picker license: {name}"),
        )?;
    }
    Ok(())
}

fn run() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    require(
        arguments.len() == 2,
        "usage: rpm-package-audit RPM_DIRECTORY FRESH_EVIDENCE_DIRECTORY",
    )?;
    let directory = fs::canonicalize(&arguments[0])?;
    let output = PathBuf::from(&arguments[1]);
    require(!output.exists(), "evidence directory already exists")?;
    let mut packages = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "rpm") {
            require(entry.file_type()?.is_file(), "RPM input is not a regular file")?;
            let name: String =
                serde_json::from_str(query(&path, &["--qf", "%{NAME:json}"])?.trim())?;
            require(PACKAGES.contains(&name.as_str()), "unexpected RPM in runtime input set")?;
            require(packages.insert(name, path).is_none(), "duplicate runtime package")?;
        }
    }
    require(packages.len() == PACKAGES.len(), "incomplete ten-package runtime set")?;
    let mut owners = BTreeMap::new();
    // Finish all validation before publishing evidence.
    let mut evidence = BTreeMap::new();
    for (name, path) in &packages {
        let identity = query(path, &["--qf", "%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n"])?;
        let inventory = query(path, &["--qf", FILE_QUERY])?;
        let records = files(&inventory)?;
        if name == "ryoku-desktop-runtime" {
            picker_licenses(&records)?;
            let license = query(path, &["--qf", "%{LICENSE}\n"])?;
            require(
                license.split_whitespace().any(|term| term == "GPL-3.0-or-later"),
                "runtime license omits the pinned picker license identity",
            )?;
            evidence.insert("ryoku-desktop-runtime-license.txt".into(), license);
        }
        insert_paths(name, &records, &mut owners)?;
        let scripts = query(path, &["--scripts", "--triggers", "--filetriggers"])?;
        require(scripts.trim().is_empty(), "project RPM contains a scriptlet or trigger")?;
        evidence.insert(format!("{name}-identity.txt"), identity);
        evidence.insert(format!("{name}-files.tsv"), inventory);
        evidence.insert(format!("{name}-requires.txt"), query(path, &["--requires"])?);
        evidence.insert(format!("{name}-provides.txt"), query(path, &["--provides"])?);
        evidence.insert(format!("{name}-scripts.txt"), scripts);
    }
    let provides = evidence
        .iter()
        .filter(|(name, _)| name.ends_with("-provides.txt"))
        .flat_map(|(_, value)| value.lines().map(str::to_owned))
        .collect();
    qml_imports(&evidence["ryoku-desktop-runtime-requires.txt"], &provides)?;
    fs::create_dir(&output)?;
    for (name, bytes) in evidence {
        fs::write(output.join(name), bytes)?;
    }
    println!(
        "runtime_packages={} unique_paths={} non_directory_conflicts=0 mutable_paths=0 scriptlets=0 qt6_scanning=verified internal_qml_imports=resolved",
        packages.len(),
        owners.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, mode: u32) -> String {
        format!("{}\t{mode}\t\"root\"\t\"root\"\t(none)\n", serde_json::to_string(path).unwrap())
    }

    #[test]
    fn picker_license_inventory_rejects_missing_links_wrong_locations_and_duplicates() {
        let inventory: String = PICKER_LICENSES
            .iter()
            .map(|name| {
                row(
                    &format!("/usr/share/licenses/ryoku-desktop-runtime/ryoku-v1.0/{name}"),
                    0o100644,
                )
            })
            .collect();
        let records = files(&inventory).unwrap();
        assert!(picker_licenses(&records).is_ok());
        for index in 0..PICKER_LICENSES.len() {
            let mut records = files(&inventory).unwrap();
            records.remove(index);
            assert!(picker_licenses(&records).is_err());
        }
        let mut records = files(&inventory).unwrap();
        records[0].mode = 0o120777;
        assert!(picker_licenses(&records).is_err());
        records[0].mode = 0o100644;
        records[0].path = "/usr/share/doc/ryogami-picker-LICENSE".into();
        assert!(picker_licenses(&records).is_err());
        let mut records = files(&inventory).unwrap();
        records.push(FileRecord {
            path: "/usr/share/licenses/ryoku-desktop-runtime/other/ryogami-picker-LICENSE".into(),
            mode: 0o100644,
        });
        assert!(picker_licenses(&records).is_err());
        records.last_mut().unwrap().mode = 0o120777;
        assert!(picker_licenses(&records).is_err());
    }

    #[test]
    fn picker_license_copies_match_pinned_source_and_selected_writer_inputs() {
        use sha2::{Digest, Sha256};

        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../upstream/evidence/ryoku-v1.0/picker-license-provenance.json"
        ))
        .unwrap();
        let selected: BTreeMap<_, _> =
            include_str!("../../../upstream/evidence/ryoku-v1.0/writer-source-manifest.tsv")
                .lines()
                .skip(1)
                .map(|line| {
                    let fields: Vec<_> = line.split('\t').collect();
                    (fields[0], (fields[1], fields[2]))
                })
                .collect();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let copies = evidence["license_copies"].as_array().unwrap();
        assert_eq!(copies.len(), PICKER_LICENSES.len());
        let mut basenames = BTreeSet::new();
        for entry in copies {
            let source = entry["source"].as_str().unwrap();
            let blob = entry["git_blob"].as_str().unwrap();
            assert_eq!(selected.get(source), Some(&(blob, "644")));
            assert_eq!(entry["git_mode"], "100644");
            let copy = root.join(entry["package_copy"].as_str().unwrap());
            assert!(fs::symlink_metadata(&copy).unwrap().is_file());
            let bytes = fs::read(copy).unwrap();
            assert_eq!(u64::try_from(bytes.len()).unwrap(), entry["bytes"].as_u64().unwrap());
            assert_eq!(format!("{:x}", Sha256::digest(bytes)), entry["sha256"]);
            basenames.insert(entry["installed_basename"].as_str().unwrap());
        }
        assert_eq!(basenames, PICKER_LICENSES.iter().copied().collect());
        let fonts = evidence["embedded_fonts"].as_array().unwrap();
        let embedded: BTreeSet<_> =
            fonts.iter().map(|font| font["source"].as_str().unwrap()).collect();
        let selected_fonts = selected
            .keys()
            .copied()
            .filter(|source| {
                source.starts_with("ryoku/shell/ryogami/picker/qml/theme/fonts/")
                    && source.ends_with(".ttf")
            })
            .collect();
        assert_eq!(embedded, selected_fonts);
        for font in fonts {
            assert_eq!(
                selected.get(font["source"].as_str().unwrap()),
                Some(&(font["git_blob"].as_str().unwrap(), "644"))
            );
            assert!(copies.iter().any(|copy| copy["source"] == font["license_source"]));
        }
        let spec = include_str!("../../../packaging/rpm/ryoku-desktop.spec");
        assert!(spec.contains("%license packaging/licenses/ryoku-v1.0/\n"));
        assert!(!spec.contains("%license upstream-writers/ryoku/shell/ryogami/picker/"));
    }

    #[test]
    fn qml_metadata_rejects_silent_scanner_failure_and_missing_internal_providers() {
        let required = "qt6qmlimport(QtQuick)\nqt6qmlimport(QtMultimedia)\nqt6qmlimport(Quickshell)\nqt6qmlimport(Ryoku.Ui.Singletons)\n";
        let mut provided = BTreeSet::from(["qt6qmlimport(Quickshell)".into()]);
        assert!(qml_imports("libQt6Qml.so.6()(64bit)\n", &provided).is_err());
        assert!(qml_imports(required, &provided).is_err());
        provided.insert("qt6qmlimport(Ryoku.Ui.Singletons)".into());
        assert!(qml_imports(required, &provided).is_ok());
        let tray = format!("{required}qt6qmlimport(Quickshell.Services.SystemTray)\n");
        assert!(qml_imports(&tray, &provided).is_err());
        provided.insert("qt6qmlimport(Quickshell.Services.SystemTray)".into());
        assert!(qml_imports(&tray, &provided).is_ok());
        assert!(
            qml_imports(&format!("{required}qt6qmlimport(shell.unknown)\n"), &provided).is_err()
        );
    }

    #[test]
    fn parsed_selected_quickshell_modules_have_real_static_rpm_providers() {
        let imports: BTreeSet<_> =
            include_str!("../../../upstream/evidence/ryoku-v1.0/qml-import-names.txt")
                .lines()
                .filter(|name| name.starts_with("Quickshell"))
                .map(|name| format!("qt6qmlimport({name})"))
                .collect();
        let spec = include_str!("../../../packaging/rpm/ryoku-quickshell.spec");
        let provided: BTreeSet<_> = spec
            .lines()
            .filter_map(|line| line.strip_prefix("Provides:"))
            .map(str::trim)
            .filter(|name| name.starts_with("qt6qmlimport(Quickshell"))
            .map(str::to_owned)
            .collect();
        assert_eq!(imports, provided);
        assert!(spec.lines().any(|line| {
            line.strip_prefix("Release:").is_some_and(|value| value.trim() == "3%{?dist}")
        }));
        assert!(include_str!("../../../packaging/rpm/ryoku-desktop.spec").lines().any(|line| {
            line.strip_prefix("Requires:")
                .is_some_and(|value| value.trim() == "ryoku-quickshell = 0.3.2-3")
        }));
    }

    #[test]
    #[ignore = "requires native rpmspec; verifies parsed spec metadata, not a built RPM"]
    fn native_spec_parser_preserves_systemtray_provider_and_matching_release() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/rpm");
        for (file, query, expected) in [
            ("ryoku-quickshell.spec", "--provides", "qt6qmlimport(Quickshell.Services.SystemTray)"),
            ("ryoku-quickshell.spec", "--provides", "ryoku-quickshell = 0.3.2-3"),
            ("ryoku-desktop.spec", "--requires", "ryoku-quickshell = 0.3.2-3"),
        ] {
            let output = Command::new("rpmspec")
                .args(["-q", "--rpms", query])
                .arg(root.join(file))
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(
                String::from_utf8(output.stdout).unwrap().lines().any(|line| line == expected),
                "{file}: {expected}"
            );
        }
    }

    #[test]
    fn inventories_reject_mutable_noncanonical_privileged_and_writable_payloads() {
        for path in [
            "/etc/ryoku",
            "/home/user/ryoku",
            "/usr/../etc/ryoku",
            "/usr//bin/ryoku",
            "/usr/./bin/ryoku",
            "/usr/bin/bad\nrow",
        ] {
            assert!(files(&row(path, 0o100644)).is_err(), "{path}");
        }
        for mode in [0o104755, 0o102755, 0o100666, 0o020600] {
            assert!(files(&row("/usr/bin/ryoku", mode)).is_err());
        }
        assert!(files(&row("/usr/bin/name\"quoted", 0o100755)).is_ok());
        assert!(
            files(&row("/usr/bin/ryoku", 0o100755).replace("(none)", "cap_dac_override=ep"))
                .is_err()
        );
        assert!(files(&row("/usr/bin/ryoku", 0o100755).repeat(2)).is_err());
    }

    #[test]
    fn shared_directories_require_identical_modes_and_files_have_single_owners() {
        let mut owners = BTreeMap::new();
        insert_paths("one", &files(&row("/usr/libexec/ryoku", 0o040755)).unwrap(), &mut owners)
            .unwrap();
        insert_paths("two", &files(&row("/usr/libexec/ryoku", 0o040755)).unwrap(), &mut owners)
            .unwrap();
        assert!(
            insert_paths(
                "three",
                &files(&row("/usr/libexec/ryoku", 0o040750)).unwrap(),
                &mut owners
            )
            .is_err()
        );
        let file = files(&row("/usr/bin/ryoku", 0o100755)).unwrap();
        insert_paths("one", &file, &mut owners).unwrap();
        assert!(insert_paths("two", &file, &mut owners).is_err());
    }
}
