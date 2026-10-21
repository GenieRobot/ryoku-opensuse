//! Disposable-VM-only override preservation receipts for issue 23 RPM tests.
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const BASELINE: &str = "/home/tester/issue23-preservation-receipt.txt";
const PERSONAL: &str = "/home/tester/.config/ryoku/opensuse-profile.json";
const UPSTREAM: &str = "/home/tester/.config/ryoku/profile.json";
const UNRELATED: &str = "/home/tester/issue23-unrelated-home-sentinel";
const LINK: &str = "/home/tester/.config/ryoku/issue23-preservation-link";
const STATE: &str = "/var/lib/ryoku-state/issue23-preservation-only";
const PATHS: &[&str] = &[PERSONAL, UPSTREAM, UNRELATED, LINK, STATE];

fn require(condition: bool, message: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(message.into()) }
}

#[derive(Clone, Copy)]
struct Identity<'a> {
    uid: &'a [u8],
    fixture: &'a str,
    owned: &'a str,
    dmi: &'a str,
    release: &'a str,
    arch: &'a str,
    authorization: &'a str,
}

fn authorized(identity: Identity<'_>) -> bool {
    identity.uid == b"0\n"
        && identity.fixture == "disposable-issue13\n"
        && identity.owned == "issue23-clean-install-20261021\n"
        && identity.dmi.trim() == "Standard PC (Q35 + ICH9, 2009)"
        && identity.release.lines().any(|line| line == "ID=\"opensuse-tumbleweed\"")
        && identity.release.lines().any(|line| line == "VERSION_ID=\"20261016\"")
        && identity.arch == "x86_64"
        && identity.authorization == "issue23-owned-overlay"
}

fn guard() -> Result<()> {
    let uid = Command::new("/usr/bin/id").arg("-u").output()?.stdout;
    require(uid == b"0\n", "fixture requires root inside its owned guest")?;
    require(
        authorized(Identity {
            uid: &uid,
            fixture: &fs::read_to_string("/etc/ryoku-lab-fixture")?,
            owned: &fs::read_to_string("/etc/ryoku-issue23-acceptance-owned")?,
            dmi: &fs::read_to_string("/sys/class/dmi/id/product_name")?,
            release: &fs::read_to_string("/etc/os-release")?,
            arch: std::env::consts::ARCH,
            authorization: &std::env::var("RYOKU_RPM_TEST_AUTHORIZED").unwrap_or_default(),
        }),
        "requires explicitly authorized, marked issue23 20261016 QEMU overlay",
    )
}

fn receipt() -> Result<String> {
    receipt_paths(&PATHS.iter().map(Path::new).collect::<Vec<_>>())
}

fn receipt_paths(paths: &[&Path]) -> Result<String> {
    let mut result = String::new();
    for path in paths {
        let metadata = fs::symlink_metadata(path)?;
        let bytes = if metadata.file_type().is_symlink() {
            fs::read_link(path)?.as_os_str().as_encoded_bytes().to_vec()
        } else {
            require(metadata.is_file() && metadata.len() < 4096, "invalid fixture file")?;
            fs::read(path)?
        };
        let data: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        // atime/ctime are deliberately excluded: reading and RPM transactions
        // may change them. File bytes, link targets, ownership, type, mode,
        // mtime, inode and hard-link count must remain identical.
        result.push_str(&format!(
            "{}\t{}\t{}\t{:o}\t{}\t{}\t{}\t{}\t{}\t{data}\n",
            path.display(),
            metadata.uid(),
            metadata.gid(),
            metadata.mode(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ino(),
            metadata.nlink(),
            metadata.len(),
        ));
    }
    Ok(result)
}

fn create(path: &str, bytes: &[u8], user_owned: bool) -> Result<()> {
    let parent = Path::new(path).parent().ok_or("missing fixture parent")?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if user_owned {
        require(
            Command::new("/usr/bin/chown").args(["tester:tester", path]).status()?.success(),
            "fixture ownership failed",
        )?;
    }
    Ok(())
}

fn prepare() -> Result<()> {
    for path in PATHS.iter().copied().chain([BASELINE]) {
        require(
            fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "fixture path already exists or cannot be inspected",
        )?;
    }
    let group = Command::new("/usr/bin/getent").args(["group", "ryoku-operators"]).output()?;
    require(
        group.status.code() == Some(2) && group.stdout.is_empty(),
        "operator group must initially be absent",
    )?;
    let home = fs::metadata("/home/tester")?;
    for directory in ["/home/tester/.config", "/home/tester/.config/ryoku"] {
        match fs::symlink_metadata(directory) {
            Ok(metadata) => require(
                metadata.is_dir() && metadata.uid() == home.uid(),
                "fixture config directory has an unexpected owner or type",
            )?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(directory)?;
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
                require(
                    Command::new("/usr/bin/chown")
                        .args(["tester:tester", directory])
                        .status()?
                        .success(),
                    "fixture directory ownership failed",
                )?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    create(PERSONAL, b"{\"schema_version\":1,\"id\":\"issue23-personal\",\"settings\":{\"density\":\"compact\"}}\n", true)?;
    create(UPSTREAM, b"{\"issue23\":\"opaque upstream profile preservation\"}\n", true)?;
    create(UNRELATED, b"unrelated home bytes must survive package transactions\n", true)?;
    symlink(UNRELATED, LINK)?;
    require(
        Command::new("/usr/bin/chown")
            .args(["--no-dereference", "tester:tester", LINK])
            .status()?
            .success(),
        "fixture symlink ownership failed",
    )?;
    // This sentinel is deliberately not a valid state store, identity, profile
    // object or restore point. Packaging must preserve the existing namespace.
    create(STATE, b"preservation fixture only; no valid recovery state or readiness\n", false)?;
    create(BASELINE, receipt()?.as_bytes(), false)?;
    println!(
        "preservation_fixture=prepared operator_group_before=absent valid_recovery_state=not_created"
    );
    Ok(())
}

fn verify() -> Result<()> {
    require(
        fs::read_to_string(BASELINE)? == receipt()?,
        "package transaction changed an override or unrelated fixture",
    )?;
    let group = Command::new("/usr/bin/getent").args(["group", "ryoku-operators"]).output()?;
    require(group.status.success(), "operator group was not created")?;
    let value = String::from_utf8(group.stdout)?;
    let fields: Vec<_> = value.trim_end().split(':').collect();
    require(
        fields.len() == 4
            && fields[0] == "ryoku-operators"
            && fields[2].parse::<u32>().is_ok_and(|gid| gid > 0)
            && fields[3].is_empty(),
        "operator group has unexpected identity or enrolled users",
    )?;
    require(
        !Path::new("/etc/systemd/system/sockets.target.wants/ryoku-helper.socket").exists(),
        "package enabled helper socket",
    )?;
    require(
        !Path::new("/etc/systemd/user/default.target.wants/ryoku-service.service").exists(),
        "package globally enabled user service",
    )?;
    println!(
        "preservation_fixture=unchanged files=5 bytes_links_owner_mode_mtime_inode=matched operator_group_gid={} group_members=empty activation_links=absent",
        fields[2]
    );
    Ok(())
}

fn run() -> Result<()> {
    guard()?;
    match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [action] if action == "prepare" => prepare(),
        [action] if action == "verify" => verify(),
        _ => Err("usage: rpm-install-fixture {prepare|verify}".into()),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("RPM fixture failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_requires_every_owned_guest_fact() {
        let valid = Identity {
            uid: b"0\n",
            fixture: "disposable-issue13\n",
            owned: "issue23-clean-install-20261021\n",
            dmi: "Standard PC (Q35 + ICH9, 2009)\n",
            release: "ID=\"opensuse-tumbleweed\"\nVERSION_ID=\"20261016\"\n",
            arch: "x86_64",
            authorization: "issue23-owned-overlay",
        };
        assert!(authorized(valid));
        for invalid in [
            Identity { uid: b"1000\n", ..valid },
            Identity { fixture: "", ..valid },
            Identity { owned: "issue25\n", ..valid },
            Identity { dmi: "physical hardware", ..valid },
            Identity { release: "ID=\"opensuse-tumbleweed\"\nVERSION_ID=\"20261017\"\n", ..valid },
            Identity { release: "ID=\"another-system\"\nVERSION_ID=\"20261016\"\n", ..valid },
            Identity { arch: "aarch64", ..valid },
            Identity { authorization: "", ..valid },
        ] {
            assert!(!authorized(invalid));
        }
    }

    #[test]
    fn receipts_detect_byte_mode_link_and_inode_changes_and_refuse_other_types() {
        let nonce =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory =
            std::env::temp_dir().join(format!("issue23-receipt-{}-{nonce}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let file = directory.join("file");
        let link = directory.join("link");
        fs::write(&file, b"before").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        symlink("file", &link).unwrap();
        let baseline = receipt_paths(&[&file, &link]).unwrap();
        assert_eq!(baseline, receipt_paths(&[&file, &link]).unwrap());
        fs::write(&file, b"after!").unwrap();
        assert_ne!(baseline, receipt_paths(&[&file, &link]).unwrap());
        let before_mode = receipt_paths(&[&file]).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_ne!(before_mode, receipt_paths(&[&file]).unwrap());
        let before_link = receipt_paths(&[&link]).unwrap();
        fs::remove_file(&link).unwrap();
        symlink("other", &link).unwrap();
        assert_ne!(before_link, receipt_paths(&[&link]).unwrap());
        let before_inode = receipt_paths(&[&file]).unwrap();
        let replacement = directory.join("replacement");
        fs::copy(&file, &replacement).unwrap();
        fs::rename(&replacement, &file).unwrap();
        assert_ne!(before_inode, receipt_paths(&[&file]).unwrap());
        assert!(receipt_paths(&[&directory]).is_err());
        fs::write(&file, vec![0; 4096]).unwrap();
        assert!(receipt_paths(&[&file]).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
