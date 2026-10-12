//! Derive a bootable Btrfs fixture without altering the signed XFS source base.
use crate::{
    definition::scalar,
    guest::{command, fixture_only},
    support::{Result, require},
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

pub fn migrate(proxy: &str) -> Result<()> {
    fixture_only()?;
    require(
        Command::new("blkid").args(["--probe", "/dev/vdb"]).status()?.code() == Some(2),
        "migration target already has a signature or cannot be inspected",
    )?;
    let _: std::net::SocketAddr = proxy.parse()?;
    require(
        command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?
            .trim()
            .eq_ignore_ascii_case(scalar("source_filesystem")?),
        "unexpected source filesystem",
    )?;
    require(
        command("lsblk", &["-n", "-o", "TYPE", "/dev/vdb"])?.trim() == "disk",
        "migration target is not a fresh whole disk",
    )?;
    require(
        command("lsblk", &["-n", "-o", "MOUNTPOINTS", "/dev/vdb"])?.trim().is_empty(),
        "migration disk is mounted",
    )?;
    // The host supplies only a brand new second disk; formatting existing disks is forbidden.
    for entry in fs::read_dir("/etc/zypp/repos.d")? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "repo") {
            let repo = fs::read_to_string(&path)?;
            fs::write(
                path,
                repo.replace(
                    "http://download.opensuse.org/",
                    "https://downloadcontent2.opensuse.org/",
                )
                .replace(
                    "https://download.opensuse.org/",
                    "https://downloadcontent2.opensuse.org/",
                ),
            )?;
        }
    }
    command("update-ca-certificates", &[])?;
    for args in [
        vec!["--non-interactive", "refresh"],
        vec![
            "--non-interactive",
            "install",
            "--no-recommends",
            "btrfsprogs",
            "dosfstools",
            "parted",
            "rsync",
            "acl",
            "snapper",
            "grub2-x86_64-efi",
            "grub2-snapper-plugin",
        ],
    ] {
        let result = Command::new("zypper")
            .args(args)
            .env("HTTP_PROXY", format!("http://{proxy}"))
            .env("HTTPS_PROXY", format!("http://{proxy}"))
            .output()?;
        if !result.status.success() {
            eprintln!(
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return Err(format!("zypper exited {}", result.status).into());
        }
    }
    // Native package installation created this config with generic etc_t on
    // the cloud source. Apply the installed policy before copying: snapperd's
    // normal D-Bus initialization must retain its confined authorization.
    command("restorecon", &["/etc/sysconfig/snapper"])?;
    command("matchpathcon", &["-V", "/etc/sysconfig/snapper"])?;
    command(
        "parted",
        &[
            "--script", "/dev/vdb", "mklabel", "gpt", "mkpart", "ESP", "fat32", "1MiB", "257MiB",
            "set", "1", "esp", "on", "mkpart", "root", "btrfs", "257MiB", "100%",
        ],
    )?;
    command("udevadm", &["settle"])?;
    command("mkfs.vfat", &["-F", "32", "/dev/vdb1"])?;
    command("mkfs.btrfs", &["/dev/vdb2"])?;
    let target = "/mnt/ryoku-lab-btrfs";
    fs::create_dir(target)?;
    command("mount", &["/dev/vdb2", target])?;
    command("btrfs", &["subvolume", "create", &format!("{target}/@")])?;
    let default_id = command("btrfs", &["inspect-internal", "rootid", &format!("{target}/@")])?
        .trim()
        .parse::<u64>()?;
    require(default_id > 5, "new root subvolume has invalid ID")?;
    command("btrfs", &["subvolume", "set-default", &default_id.to_string(), target])?;
    command("umount", &[target])?;
    command("mount", &["-o", "subvol=@", "/dev/vdb2", target])?;
    // Keep unrelated home contents and persistent /var data outside root snapshots.
    for directory in ["home", "var"] {
        command("btrfs", &["subvolume", "create", &format!("{target}/{directory}")])?;
    }
    // Include the root inode and mountpoint directory metadata. Omitting them
    // loses ACL/security contexts even when every ordinary file is copied.
    command(
        "rsync",
        &[
            "--archive",
            "--hard-links",
            "--acls",
            "--xattrs",
            "--numeric-ids",
            "--one-file-system",
            "--exclude=/dev/*",
            "--exclude=/proc/*",
            "--exclude=/sys/*",
            "--exclude=/run/*",
            "--exclude=/tmp/*",
            "--exclude=/mnt/*",
            "--exclude=/lost+found/*",
            "/",
            &format!("{target}/"),
        ],
    )?;
    for directory in ["dev", "proc", "sys", "run", "tmp", "mnt", "boot/efi"] {
        fs::create_dir_all(format!("{target}/{directory}"))?;
    }
    fs::set_permissions(format!("{target}/tmp"), fs::Permissions::from_mode(0o1777))?;
    for source in [
        "/",
        "/dev",
        "/proc",
        "/sys",
        "/run",
        "/tmp",
        "/mnt",
        "/usr",
        "/usr/lib64",
        "/usr/lib64/libeconf.so.0",
        "/usr/sbin/sshd",
        "/etc/sysconfig/snapper",
        "/etc/snapper",
    ] {
        let destination = format!("{target}{source}");
        require(
            command("stat", &["--dereference", "--format=%u:%g:%a:%C", source])?
                == command("stat", &["--dereference", "--format=%u:%g:%a:%C", &destination])?,
            "copied inode ownership/mode/security context mismatch",
        )?;
        require(
            command("getfacl", &["--numeric", "--omit-header", "--absolute-names", source])?
                == command(
                    "getfacl",
                    &["--numeric", "--omit-header", "--absolute-names", &destination],
                )?,
            "copied ACL mismatch",
        )?;
    }
    let root_uuid = command("blkid", &["-s", "UUID", "-o", "value", "/dev/vdb2"])?;
    let efi_uuid = command("blkid", &["-s", "UUID", "-o", "value", "/dev/vdb1"])?;
    fs::write(
        format!("{target}/etc/fstab"),
        format!(
            "UUID={root} / btrfs defaults 0 0\nUUID={root} /home btrfs defaults,subvol=@/home 0 0\nUUID={root} /var btrfs defaults,subvol=@/var 0 0\nUUID={efi} /boot/efi vfat umask=0077 0 2\n",
            root = root_uuid.trim(),
            efi = efi_uuid.trim()
        ),
    )?;
    command("mount", &["/dev/vdb1", &format!("{target}/boot/efi")])?;
    for directory in ["dev", "proc", "sys", "run"] {
        command("mount", &["--rbind", &format!("/{directory}"), &format!("{target}/{directory}")])?;
        command("mount", &["--make-rslave", &format!("{target}/{directory}")])?;
    }
    command("chroot", &[target, "dracut", "--force", "--no-hostonly"])?;
    command(
        "chroot",
        &[
            target,
            "grub2-install",
            "--target=x86_64-efi",
            "--efi-directory=/boot/efi",
            "--boot-directory=/boot",
            "--removable",
            "--no-nvram",
        ],
    )?;
    command("chroot", &[target, "grub2-mkconfig", "-o", "/boot/grub2/grub.cfg"])?;
    command("sync", &[])?;
    command("umount", &["--recursive", target])?;
    println!(
        "source_root=xfs\nderived_root=btrfs\nderived_esp=created\nderived_grub2=installed\nderived_initramfs=generated\nmetadata_acl_context=matched"
    );
    Ok(())
}
