//! Read-only installer admission for the root layout required by linked rollback.
use super::*;
use serde::Deserialize;
use std::ffi::OsStr;

const REFUSAL: &str = "unsupported recovery layout: requires the running writable numbered Snapper default, mounted /.snapshots and snapshot-boot GRUB";

pub(super) fn validate() -> Result<(), String> {
    let query = |program, args: &[&str]| {
        let args = args.iter().map(OsStr::new).collect::<Vec<_>>();
        let output = bounded_command(program, &args, 30, 128 * 1024)?;
        if !output.status.success() {
            return Err(REFUSAL.to_owned());
        }
        Ok(output.stdout)
    };
    let mounts = |path| {
        query(
            "/usr/bin/findmnt",
            &["--json", "--output", "TARGET,FSROOT,UUID,OPTIONS", "--mountpoint", path],
        )
    };
    let root = mounts("/")?;
    let snapshots = mounts("/.snapshots")?;
    let inventory = query(
        "/usr/bin/snapper",
        &[
            "--jsonout",
            "--config",
            "root",
            "list",
            "--columns",
            "number,type,default,active,read-only",
        ],
    )?;
    let grub = fs::read_to_string("/etc/default/grub").map_err(|_| REFUSAL)?;
    validate_evidence(&root, &snapshots, &inventory, &grub).map_err(|_| REFUSAL.to_owned())
}

#[derive(Deserialize)]
struct Mounts {
    filesystems: Vec<Mount>,
}
#[derive(Deserialize)]
struct Mount {
    target: String,
    fsroot: String,
    uuid: String,
    options: String,
}
#[derive(Deserialize)]
struct Inventory {
    root: Vec<Snapshot>,
}
#[derive(Deserialize)]
struct Snapshot {
    number: u64,
    #[serde(rename = "type")]
    kind: String,
    default: bool,
    active: bool,
    #[serde(rename = "read-only")]
    read_only: bool,
}
fn validate_evidence(
    root: &[u8],
    snapshots: &[u8],
    inventory: &[u8],
    grub: &str,
) -> Result<(), ()> {
    let root: Mounts = serde_json::from_slice(root).map_err(|_| ())?;
    let snapshots: Mounts = serde_json::from_slice(snapshots).map_err(|_| ())?;
    let inventory: Inventory = serde_json::from_slice(inventory).map_err(|_| ())?;
    let [root] = root.filesystems.as_slice() else { return Err(()) };
    let [snapshots] = snapshots.filesystems.as_slice() else { return Err(()) };
    let number = root
        .fsroot
        .strip_prefix("/@/.snapshots/")
        .and_then(|path| path.strip_suffix("/snapshot"))
        .ok_or(())?;
    let parsed = number.parse::<u64>().map_err(|_| ())?;
    let rw = |options: &str| {
        options.split(',').filter(|value| *value == "rw").count() == 1
            && !options.split(',').any(|value| value == "ro")
    };
    let defaults = inventory.root.iter().filter(|row| row.default).collect::<Vec<_>>();
    let active = inventory.root.iter().filter(|row| row.active).collect::<Vec<_>>();
    let boot = grub
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .filter(|(key, _)| key.trim() == "SUSE_BTRFS_SNAPSHOT_BOOTING")
        .map(|(_, value)| value.trim())
        .collect::<Vec<_>>();
    if parsed == 0
        || parsed.to_string() != number
        || root.target != "/"
        || snapshots.target != "/.snapshots"
        || snapshots.fsroot != "/@/.snapshots"
        || root.uuid.is_empty()
        || root.uuid != snapshots.uuid
        || !rw(&root.options)
        || !rw(&snapshots.options)
        || defaults.len() != 1
        || active.len() != 1
        || defaults[0].number != parsed
        || active[0].number != parsed
        || defaults[0].read_only
        || defaults[0].kind != "single"
        || inventory.root.iter().filter(|row| row.number == parsed).count() != 1
        || !matches!(boot.as_slice(), ["true"] | ["\"true\""] | ["'true'"])
    {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOT: &[u8] = br#"{"filesystems":[{"target":"/","fsroot":"/@/.snapshots/3/snapshot","uuid":"root-uuid","options":"rw,relatime"}]}"#;
    const SNAPSHOTS: &[u8] = br#"{"filesystems":[{"target":"/.snapshots","fsroot":"/@/.snapshots","uuid":"root-uuid","options":"rw,relatime"}]}"#;
    const INVENTORY: &[u8] = br#"{"root":[{"number":0,"type":"single","default":false,"active":false,"read-only":false},{"number":3,"type":"single","default":true,"active":true,"read-only":false}]}"#;
    const GRUB: &str = "SUSE_BTRFS_SNAPSHOT_BOOTING=\"true\"\n";
    #[test]
    fn installer_requires_recoverable_numbered_default_before_capture() {
        assert_eq!(validate_evidence(ROOT, SNAPSHOTS, INVENTORY, GRUB), Ok(()));
        let root = std::str::from_utf8(ROOT).unwrap();
        let inventory = std::str::from_utf8(INVENTORY).unwrap();
        for (root, snapshots, inventory, grub) in [
            (
                root.replace("/@/.snapshots/3/snapshot", "/@"),
                SNAPSHOTS.to_vec(),
                INVENTORY.to_vec(),
                GRUB,
            ),
            (root.replace("/3/", "/03/"), SNAPSHOTS.to_vec(), INVENTORY.to_vec(), GRUB),
            (root.replace("rw,", "ro,"), SNAPSHOTS.to_vec(), INVENTORY.to_vec(), GRUB),
            (
                root.to_owned(),
                SNAPSHOTS.to_vec(),
                inventory.replace("\"default\":true", "\"default\":false").into_bytes(),
                GRUB,
            ),
            (
                root.to_owned(),
                SNAPSHOTS.to_vec(),
                inventory.replace("\"active\":true", "\"active\":false").into_bytes(),
                GRUB,
            ),
            (
                root.to_owned(),
                SNAPSHOTS.to_vec(),
                inventory.replace("\"read-only\":false", "\"read-only\":true").into_bytes(),
                GRUB,
            ),
            (
                root.to_owned(),
                SNAPSHOTS.to_vec(),
                INVENTORY.to_vec(),
                "SUSE_BTRFS_SNAPSHOT_BOOTING=false\n",
            ),
            (
                root.to_owned(),
                SNAPSHOTS.to_vec(),
                INVENTORY.to_vec(),
                "SUSE_BTRFS_SNAPSHOT_BOOTING=true\nSUSE_BTRFS_SNAPSHOT_BOOTING=true\n",
            ),
            (root.to_owned(), SNAPSHOTS.to_vec(), INVENTORY.to_vec(), ""),
            (root.to_owned(), b"{\"filesystems\":[]}".to_vec(), INVENTORY.to_vec(), GRUB),
            (
                root.to_owned(),
                String::from_utf8(SNAPSHOTS.to_vec())
                    .unwrap()
                    .replace("root-uuid", "other-uuid")
                    .into_bytes(),
                INVENTORY.to_vec(),
                GRUB,
            ),
        ] {
            assert_eq!(validate_evidence(root.as_bytes(), &snapshots, &inventory, grub), Err(()));
        }
    }
}
