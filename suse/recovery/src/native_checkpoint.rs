//! Kernel verification of a newly created default-root Snapper checkpoint.
//!
//! This deliberately checks a snapshot of the currently running root. It is
//! not an availability check for historical restore sources after rollback.

use crate::{
    SnapshotLink,
    generation_capture::VerifiedSnapshotLink,
    state_mount::{
        self, StateMountError, format_uuid, read_btrfs_fs_info, read_btrfs_subvolume_info,
    },
};
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags, ResolveFlags, fstat, openat2},
};
use std::fs::File;

/// A pinned, independently observed native snapshot. Holding this descriptor
/// does not prevent privileged deletion or remounts; revalidate before use.
pub struct NativeRootCheckpoint {
    descriptor: OwnedFd,
    link: VerifiedSnapshotLink,
    pub subvolume_id: u64,
    pub subvolume_uuid: String,
}

impl NativeRootCheckpoint {
    pub fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }
    pub fn verified_link(&self) -> &VerifiedSnapshotLink {
        &self.link
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum NativeCheckpointError {
    UnsupportedLink,
    NotSubvolumeRoot,
    IdentityMismatch,
    WritableSnapshot,
    NotCurrentRootSnapshot,
    Observation(StateMountError),
    Filesystem(rustix::io::Errno),
}

impl std::fmt::Display for NativeCheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native checkpoint verification failed: {self:?}")
    }
}
impl std::error::Error for NativeCheckpointError {}

/// Verify `.snapshots/<number>/snapshot` beneath the actual process root,
/// without following any symlink. Only the default root layout is supported.
/// The filesystem UUID, snapshot UUID, parent UUID and read-only flag come
/// from read-only Btrfs ioctls on pinned descriptors, never command output.
///
/// The caller must separately verify Snapper config-to-root mapping and its
/// transaction tag, boot coverage, authorization, and writer quiescence. This
/// routine neither creates a snapshot nor claims a complete linked checkpoint.
pub fn verify_new_root_checkpoint(
    trusted_root: &File,
    link: SnapshotLink,
) -> Result<NativeRootCheckpoint, NativeCheckpointError> {
    state_mount::validate_trusted_root(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let number = link
        .snapper_snapshot_id
        .parse::<u64>()
        .map_err(|_| NativeCheckpointError::UnsupportedLink)?;
    if link.snapper_config.as_str() != "root"
        || number == 0
        || number.to_string() != link.snapper_snapshot_id
    {
        return Err(NativeCheckpointError::UnsupportedLink);
    }
    let descriptor = openat2(
        trusted_root,
        format!(".snapshots/{number}/snapshot"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .map_err(NativeCheckpointError::Filesystem)?;
    if fstat(trusted_root).map_err(NativeCheckpointError::Filesystem)?.st_ino != 256
        || fstat(&descriptor).map_err(NativeCheckpointError::Filesystem)?.st_ino != 256
    {
        return Err(NativeCheckpointError::NotSubvolumeRoot);
    }
    let root_fs = read_btrfs_fs_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let snapshot_fs =
        read_btrfs_fs_info(&descriptor).map_err(NativeCheckpointError::Observation)?;
    let root =
        read_btrfs_subvolume_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let snapshot =
        read_btrfs_subvolume_info(&descriptor).map_err(NativeCheckpointError::Observation)?;
    validate_identity(&link.filesystem_uuid, &root_fs.fsid, &snapshot_fs.fsid, &root, &snapshot)?;
    Ok(NativeRootCheckpoint {
        descriptor,
        link: VerifiedSnapshotLink::from_preverified_link(link),
        subvolume_id: snapshot.treeid,
        subvolume_uuid: format_uuid(&snapshot.uuid),
    })
}

fn validate_identity(
    expected_fs: &str,
    root_fs: &[u8; 16],
    snapshot_fs: &[u8; 16],
    root: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    snapshot: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
) -> Result<(), NativeCheckpointError> {
    if root_fs != snapshot_fs || format_uuid(root_fs) != expected_fs || *root_fs == [0; 16] {
        return Err(NativeCheckpointError::IdentityMismatch);
    }
    if snapshot.flags & u64::from(linux_raw_sys::btrfs::BTRFS_SUBVOL_RDONLY) == 0 {
        return Err(NativeCheckpointError::WritableSnapshot);
    }
    if root.uuid == [0; 16]
        || snapshot.uuid == [0; 16]
        || root.uuid == snapshot.uuid
        || root.treeid < 256
        || snapshot.treeid < 256
        || root.treeid == snapshot.treeid
        || snapshot.parent_uuid != root.uuid
    {
        return Err(NativeCheckpointError::NotCurrentRootSnapshot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_raw_sys::btrfs::{
        btrfs_ioctl_get_subvol_info_args as Info, btrfs_ioctl_timespec as Time,
    };

    fn info(id: u64, uuid: u8, parent: u8) -> Info {
        let time = Time { sec: 0, nsec: 0 };
        Info {
            treeid: id,
            name: [0; 256],
            parent_id: 5,
            dirid: 256,
            generation: 1,
            flags: u64::from(linux_raw_sys::btrfs::BTRFS_SUBVOL_RDONLY),
            uuid: [uuid; 16],
            parent_uuid: [parent; 16],
            received_uuid: [0; 16],
            ctransid: 0,
            otransid: 0,
            stransid: 0,
            rtransid: 0,
            ctime: time,
            otime: time,
            stime: time,
            rtime: time,
            reserved: [0; 8],
        }
    }

    #[test]
    fn only_distinct_readonly_snapshot_of_current_root_matches() {
        let root = info(256, 1, 0);
        let snapshot = info(300, 2, 1);
        let fs = [3; 16];
        assert_eq!(validate_identity(&format_uuid(&fs), &fs, &fs, &root, &snapshot), Ok(()));
        for altered in
            [info(256, 2, 1), info(5, 2, 1), info(300, 1, 1), info(300, 0, 1), info(300, 2, 4)]
        {
            assert_eq!(
                validate_identity(&format_uuid(&fs), &fs, &fs, &root, &altered),
                Err(NativeCheckpointError::NotCurrentRootSnapshot)
            );
        }
        let mut writable = snapshot;
        writable.flags = 0;
        assert_eq!(
            validate_identity(&format_uuid(&fs), &fs, &fs, &root, &writable),
            Err(NativeCheckpointError::WritableSnapshot)
        );
        assert_eq!(
            validate_identity(&format_uuid(&fs), &fs, &[4; 16], &root, &snapshot),
            Err(NativeCheckpointError::IdentityMismatch)
        );
        assert_eq!(
            validate_identity(&format_uuid(&[4; 16]), &fs, &fs, &root, &snapshot),
            Err(NativeCheckpointError::IdentityMismatch)
        );
        assert_eq!(
            validate_identity(&format_uuid(&[0; 16]), &[0; 16], &[0; 16], &root, &snapshot),
            Err(NativeCheckpointError::IdentityMismatch)
        );
    }

    #[test]
    fn directory_fixture_cannot_impersonate_the_running_root() {
        let directory = File::open("/tmp").unwrap();
        let link = SnapshotLink::new(
            crate::Identifier::new("root").unwrap(),
            "42",
            "11111111-1111-1111-1111-111111111111",
        )
        .unwrap();
        assert!(matches!(
            verify_new_root_checkpoint(&directory, link),
            Err(NativeCheckpointError::Observation(StateMountError::UnsafeRootDescriptor))
        ));
    }
}
