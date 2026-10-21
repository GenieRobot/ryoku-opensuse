//! Live read-only Btrfs/Snapper layout discovery. Layout coverage is distinct
//! from authorization, generation linkage and firmware/boot compatibility.
use crate::{
    RelativePath,
    state_mount::{self, format_uuid, read_btrfs_fs_info, read_btrfs_subvolume_info},
};
use rustix::fs::{Mode, OFlags, ResolveFlags, fstat, openat2};
use ryoku_snapper::{
    BootLayout, ConfigName, ConfigState, FilesystemKind, LayoutAssessment, LayoutObservation,
    SnapperBackend, SnapperCliBackend, assess_layout,
};
use std::{fs::File, io::Read};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredTarget {
    pub config: ConfigName,
    pub path: String,
    pub filesystem_uuid: String,
    pub subvolume_id: u64,
    pub subvolume_uuid: String,
}

#[derive(Clone, Debug)]
pub struct DiscoveredLayout {
    pub assessment: LayoutAssessment,
    pub targets: Vec<DiscoveredTarget>,
    /// A separate ESP is explicitly outside root-/boot-files coverage.
    pub external_firmware_mount: bool,
}

fn open_directory(root: &File, path: &str) -> Result<File> {
    if path == "/" {
        return Ok(root.try_clone()?);
    }
    let relative = path.strip_prefix('/').ok_or("absolute target required")?;
    RelativePath::new(relative).map_err(|_| "invalid configuration path")?;
    Ok(File::from(openat2(
        root,
        relative,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?))
}

fn identity(file: &File) -> Result<(String, u64, String)> {
    let fs = read_btrfs_fs_info(file)?;
    let volume = read_btrfs_subvolume_info(file)?;
    if fs.fsid == [0; 16] || volume.uuid == [0; 16] || volume.treeid < 256 {
        return Err("invalid Btrfs identity".into());
    }
    Ok((format_uuid(&fs.fsid), volume.treeid, format_uuid(&volume.uuid)))
}

/// Inspect the actual process root and a declared configuration directory.
/// Config mappings are read through the fixed bounded Snapper adapter and
/// independently bound to kernel Btrfs identities via no-follow descriptors.
/// Unknown, symlinked or unqueryable mappings fail closed. Root-/boot-files
/// FullSystem is never a statement about ESP coverage or restore readiness.
pub fn discover_layout(root: &File, configuration_path: &RelativePath) -> Result<DiscoveredLayout> {
    state_mount::validate_trusted_root(root)?;
    let backend = SnapperCliBackend;
    let mappings = backend.configurations()?;
    let mut targets = Vec::new();
    for mapping in mappings {
        let file = open_directory(root, &mapping.subvolume)?;
        let stat = fstat(&file)?;
        let Ok((filesystem_uuid, subvolume_id, subvolume_uuid)) = identity(&file) else { continue };
        if stat.st_ino != 256 || stat.st_uid != 0 || stat.st_mode & 0o022 != 0 {
            return Err("unsafe configured subvolume".into());
        }
        // A config existing on disk is insufficient: the native service must
        // also return a valid inventory for precisely that config.
        backend.list(&mapping.config)?;
        targets.push(DiscoveredTarget {
            config: mapping.config,
            path: mapping.subvolume,
            filesystem_uuid,
            subvolume_id,
            subvolume_uuid,
        });
    }
    let root_identity = identity(root).ok();
    let config = open_directory(root, &format!("/{}", configuration_path.as_str()))?;
    let config_identity = identity(&config).ok();
    let same = |target: &DiscoveredTarget, value: &(String, u64, String)| {
        target.filesystem_uuid == value.0
            && target.subvolume_id == value.1
            && target.subvolume_uuid == value.2
    };
    let ready_root = root_identity.as_ref().is_some_and(|value| {
        targets.iter().any(|t| t.config.as_str() == "root" && t.path == "/" && same(t, value))
    });
    let ready_config =
        config_identity.as_ref().is_some_and(|value| targets.iter().any(|t| same(t, value)));
    let mut boot = BootLayout::Unknown;
    if let Ok(directory) = open_directory(root, "/boot") {
        boot = if identity(&directory).ok().as_ref() == root_identity.as_ref()
            && root_identity.is_some()
        {
            BootLayout::IncludedInRoot
        } else {
            BootLayout::SeparateUnsnapshotted
        };
    }
    let mut mounts = String::new();
    File::open("/proc/self/mountinfo")?.take(4 * 1024 * 1024 + 1).read_to_string(&mut mounts)?;
    if mounts.len() > 4 * 1024 * 1024 {
        return Err("mount inventory exceeds bound".into());
    }
    let paths = mounted_boot_paths(&mounts)?;
    let external_firmware_mount = paths.iter().any(|p| p == "/boot/efi");
    // Any other mount below /boot can hide files from the root snapshot.
    if paths.iter().any(|p| p != "/boot/efi" && p.starts_with("/boot/")) {
        boot = BootLayout::SeparateUnsnapshotted;
    }
    let observation = LayoutObservation {
        root_filesystem: if root_identity.is_some() {
            FilesystemKind::Btrfs
        } else {
            FilesystemKind::Other
        },
        root_is_btrfs_subvolume: fstat(root)?.st_ino == 256,
        root_snapper: if ready_root { ConfigState::Ready } else { ConfigState::Missing },
        ryoku_config_filesystem: if config_identity.is_some() {
            FilesystemKind::Btrfs
        } else {
            FilesystemKind::Other
        },
        ryoku_config_snapper: if ready_config { ConfigState::Ready } else { ConfigState::Missing },
        boot,
    };
    Ok(DiscoveredLayout {
        assessment: assess_layout(observation),
        targets,
        external_firmware_mount,
    })
}

fn mounted_boot_paths(mountinfo: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for line in mountinfo.lines() {
        let (before, after) = line.split_once(" - ").ok_or("invalid mount row")?;
        let fields: Vec<_> = before.split_ascii_whitespace().collect();
        if fields.len() < 6
            || after.split_ascii_whitespace().count() < 3
            || !ids.insert(fields[0].parse::<u64>()?)
        {
            return Err("ambiguous mount row".into());
        }
        let path = fields[4];
        if path == "/boot" || path.starts_with("/boot/") {
            // Escaped boot mount paths are unsupported instead of guessed.
            if path.contains('\\') {
                return Err("escaped boot mount path unsupported".into());
            }
            result.push(path.to_owned());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_mount_inventory_refuses_ambiguity_and_escaped_paths() {
        let row = "30 20 0:1 / /boot/efi rw - vfat /dev/vda1 rw";
        assert_eq!(mounted_boot_paths(row).unwrap(), ["/boot/efi"]);
        assert!(mounted_boot_paths(&format!("{row}\n{row}")).is_err());
        assert!(mounted_boot_paths("30 20 0:1 / /boot/a\\040b rw - btrfs /dev/vda2 rw").is_err());
        assert!(mounted_boot_paths("truncated").is_err());
    }
}
