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
        insert_paths(name, &files(&inventory)?, &mut owners)?;
        let scripts = query(path, &["--scripts", "--triggers", "--filetriggers"])?;
        require(scripts.trim().is_empty(), "project RPM contains a scriptlet or trigger")?;
        evidence.insert(format!("{name}-identity.txt"), identity);
        evidence.insert(format!("{name}-files.tsv"), inventory);
        evidence.insert(format!("{name}-requires.txt"), query(path, &["--requires"])?);
        evidence.insert(format!("{name}-provides.txt"), query(path, &["--provides"])?);
        evidence.insert(format!("{name}-scripts.txt"), scripts);
    }
    fs::create_dir(&output)?;
    for (name, bytes) in evidence {
        fs::write(output.join(name), bytes)?;
    }
    println!(
        "runtime_packages={} unique_paths={} non_directory_conflicts=0 mutable_paths=0 scriptlets=0",
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
