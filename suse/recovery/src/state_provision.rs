//! Creation-side identity for the dedicated state subvolume.
//!
//! The installer must hold its provisioning lock, retain a bootstrap checkpoint
//! and persist creation intent before calling this operation. Existing names
//! are never adopted. Failure after the ioctl can retain a newly created volume;
//! reconcile the durable intent, never delete an unknown volume automatically.
//! Mounting, enabling the boot unit and installing the independent machine
//! record remain the installation transaction's responsibility.

use crate::{
    machine_state::{MachineStateError, MachineStateRecord},
    state_mount::{
        StateMountError, StateMountIdentity, format_uuid, read_btrfs_fs_info,
        read_btrfs_subvolume_info, validate_trusted_root,
    },
};
use rustix::{
    fs::{
        FileType, Gid, Mode, OFlags, ResolveFlags, Uid, fchmod, fchown, fstat, fsync, openat2,
        syncfs,
    },
    io::Errno,
    ioctl::{Setter, ioctl},
};
use std::fs::File;

pub const STATE_SUBVOLUME_NAME: &str = "@ryoku-state";
pub const STATE_MOUNT_UNIT: &str = "var-lib-ryoku\\x2dstate.mount";

/// Read-only preflight of the running writable direct-child Btrfs root.
/// Reject unsupported roots before an initializer creates even a checkpoint.
pub fn provisioning_root_uuid(root: &File) -> Result<String, StateProvisionError> {
    initialization_root_uuid(root, None)
}

/// Read-only existing-store preflight permits a writable Snapper rollback root
/// nested below the snapshot namespace. It still requires the actual safe
/// Btrfs process root on the independently recorded filesystem, distinct from
/// the protected state subvolume. This does not authorize creation or resume;
/// callers must also verify the live state mount, store, boot unit and journals.
pub fn verify_initialized_state_root(
    root: &File,
    record: &MachineStateRecord,
) -> Result<(), StateProvisionError> {
    initialization_root_uuid(root, Some(record.identity())).map(|_| ())
}

fn initialization_root_uuid(
    root: &File,
    existing: Option<&StateMountIdentity>,
) -> Result<String, StateProvisionError> {
    validate_trusted_root(root).map_err(StateProvisionError::Observation)?;
    let fs = read_btrfs_fs_info(root).map_err(StateProvisionError::Observation)?;
    let volume = read_btrfs_subvolume_info(root).map_err(StateProvisionError::Observation)?;
    let stat = fstat(root)?;
    if rustix::fs::fstatvfs(root)?.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY) {
        return Err(StateProvisionError::IdentityMismatch);
    }
    if volume.treeid < 256
        || stat.st_ino != 256
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o022 != 0
        || volume.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) != 0
    {
        return Err(StateProvisionError::IdentityMismatch);
    }
    validate_initialization_root_identity(&fs.fsid, volume.treeid, volume.parent_id, existing)
}

fn validate_initialization_root_identity(
    fsid: &[u8; 16],
    root_id: u64,
    parent_id: u64,
    existing: Option<&StateMountIdentity>,
) -> Result<String, StateProvisionError> {
    let uuid = format_uuid(fsid);
    let parent_supported = match existing {
        None => parent_id == 5,
        Some(expected) => {
            (parent_id == 5 || parent_id >= 256)
                && uuid == expected.filesystem_uuid
                && root_id != expected.subvolume_id
        }
    };
    if *fsid == [0; 16] || !parent_supported {
        return Err(StateProvisionError::IdentityMismatch);
    }
    Ok(uuid)
}

#[derive(Debug)]
pub enum StateProvisionError {
    RootRequired,
    UnsafeTopLevel,
    IdentityMismatch,
    ExistingVolume,
    UnsafeBootUnit,
    ConflictingBootUnit,
    Observation(StateMountError),
    Record(MachineStateError),
    Filesystem(Errno),
    Io(std::io::Error),
}
impl std::fmt::Display for StateProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "state subvolume provisioning failed: {self:?}")
    }
}
impl std::error::Error for StateProvisionError {}
impl From<Errno> for StateProvisionError {
    fn from(error: Errno) -> Self {
        Self::Filesystem(error)
    }
}
impl From<std::io::Error> for StateProvisionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Proof derived only from successful creation and independent kernel read-back.
/// It is not authorization, a durable provisioning journal, or a mounted store.
pub struct CreatedStateSubvolume {
    directory: File,
    record: MachineStateRecord,
    subvolume_uuid: String,
}
impl CreatedStateSubvolume {
    pub fn directory(&self) -> &File {
        &self.directory
    }
    pub fn machine_record(&self) -> &MachineStateRecord {
        &self.record
    }
    pub fn subvolume_uuid(&self) -> &str {
        &self.subvolume_uuid
    }

    /// Bind boot mounting to the created numeric ID, not a mutable volume name.
    pub fn mount_unit(&self) -> String {
        render_mount_unit(&self.record)
    }

    /// Durably install the fixed mount unit without replacing administrator
    /// content. Does not enable/start it or publish the machine record; those
    /// steps follow SELinux relabeling and mount verification in the installer.
    pub fn install_mount_unit(&self, trusted_root: &File) -> Result<(), StateProvisionError> {
        validate_trusted_root(trusted_root).map_err(StateProvisionError::Observation)?;
        let fs = read_btrfs_fs_info(trusted_root).map_err(StateProvisionError::Observation)?;
        if format_uuid(&fs.fsid) != self.record.identity().filesystem_uuid {
            return Err(StateProvisionError::IdentityMismatch);
        }
        crate::state_boot_unit::install(trusted_root, self.mount_unit().as_bytes())
    }
}

/// Create exactly `@ryoku-state` directly below Btrfs ID 5 on the running
/// root's filesystem. The supplied top-level FD must point at inode 256 of
/// root-owned, non-group/world-writable Btrfs ID 5; a directory merely living
/// inside that volume is insufficient. No device or arbitrary target path is
/// accepted. The installer supplies and owns the temporary top-level mount.
pub fn create_state_subvolume(
    trusted_root: &File,
    top_level: &File,
) -> Result<CreatedStateSubvolume, StateProvisionError> {
    if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0 {
        return Err(StateProvisionError::RootRequired);
    }
    validate_trusted_root(trusted_root).map_err(StateProvisionError::Observation)?;
    let root_fs = read_btrfs_fs_info(trusted_root).map_err(StateProvisionError::Observation)?;
    let top_fs = read_btrfs_fs_info(top_level).map_err(StateProvisionError::Observation)?;
    let top = read_btrfs_subvolume_info(top_level).map_err(StateProvisionError::Observation)?;
    let stat = fstat(top_level)?;
    validate_top_level(
        &root_fs.fsid,
        &top_fs.fsid,
        &TopLevelFacts {
            tree_id: top.treeid,
            inode: stat.st_ino,
            directory: FileType::from_raw_mode(stat.st_mode) == FileType::Directory,
            uid: stat.st_uid,
            gid: stat.st_gid,
            mode: stat.st_mode,
        },
    )?;
    create_volume(top_level).map_err(|error| {
        if error == Errno::EXIST {
            StateProvisionError::ExistingVolume
        } else {
            StateProvisionError::Filesystem(error)
        }
    })?;
    let directory = File::from(openat2(
        top_level,
        STATE_SUBVOLUME_NAME,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    fchown(&directory, Some(Uid::ROOT), Some(Gid::ROOT))?;
    fchmod(&directory, Mode::from_raw_mode(0o700))?;
    fsync(&directory)?;
    fsync(top_level)?;
    syncfs(top_level)?;
    let fs = read_btrfs_fs_info(&directory).map_err(StateProvisionError::Observation)?;
    let volume = read_btrfs_subvolume_info(&directory).map_err(StateProvisionError::Observation)?;
    if fs.fsid != root_fs.fsid
        || volume.treeid <= 5
        || volume.parent_id != 5
        || volume.uuid == [0; 16]
        || volume.parent_uuid != [0; 16]
        || volume.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) != 0
    {
        return Err(StateProvisionError::IdentityMismatch);
    }
    let stat = fstat(&directory)?;
    if stat.st_ino != 256 || stat.st_uid != 0 || stat.st_gid != 0 || stat.st_mode & 0o7777 != 0o700
    {
        return Err(StateProvisionError::IdentityMismatch);
    }
    let record =
        MachineStateRecord::from_provisioned_subvolume(format_uuid(&fs.fsid), volume.treeid)
            .map_err(StateProvisionError::Record)?;
    Ok(CreatedStateSubvolume { directory, record, subvolume_uuid: format_uuid(&volume.uuid) })
}

/// Reopen only a volume bound by protected creation receipts, before mounting.
pub(crate) fn reopen_created_from_top_level(
    root: &File,
    top_level: &File,
    record: &MachineStateRecord,
    expected_uuid: &str,
) -> Result<CreatedStateSubvolume, StateProvisionError> {
    if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0 {
        return Err(StateProvisionError::RootRequired);
    }
    validate_trusted_root(root).map_err(StateProvisionError::Observation)?;
    let root_fs = read_btrfs_fs_info(root).map_err(StateProvisionError::Observation)?;
    let top_fs = read_btrfs_fs_info(top_level).map_err(StateProvisionError::Observation)?;
    let top = read_btrfs_subvolume_info(top_level).map_err(StateProvisionError::Observation)?;
    let stat = fstat(top_level)?;
    validate_top_level(
        &root_fs.fsid,
        &top_fs.fsid,
        &TopLevelFacts {
            tree_id: top.treeid,
            inode: stat.st_ino,
            directory: FileType::from_raw_mode(stat.st_mode) == FileType::Directory,
            uid: stat.st_uid,
            gid: stat.st_gid,
            mode: stat.st_mode,
        },
    )?;
    let directory = File::from(openat2(
        top_level,
        STATE_SUBVOLUME_NAME,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let fs = read_btrfs_fs_info(&directory).map_err(StateProvisionError::Observation)?;
    let volume = read_btrfs_subvolume_info(&directory).map_err(StateProvisionError::Observation)?;
    let stat = fstat(&directory)?;
    if fs.fsid != root_fs.fsid
        || format_uuid(&fs.fsid) != record.identity().filesystem_uuid
        || volume.treeid != record.identity().subvolume_id
        || volume.parent_id != 5
        || volume.uuid == [0; 16]
        || format_uuid(&volume.uuid) != expected_uuid
        || volume.parent_uuid != [0; 16]
        || volume.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) != 0
        || stat.st_ino != 256
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(StateProvisionError::IdentityMismatch);
    }
    Ok(CreatedStateSubvolume {
        directory,
        record: record.clone(),
        subvolume_uuid: expected_uuid.into(),
    })
}

struct TopLevelFacts {
    tree_id: u64,
    inode: u64,
    directory: bool,
    uid: u32,
    gid: u32,
    mode: u32,
}

fn validate_top_level(
    root_fs: &[u8; 16],
    top_fs: &[u8; 16],
    facts: &TopLevelFacts,
) -> Result<(), StateProvisionError> {
    if *root_fs == [0; 16] || root_fs != top_fs {
        return Err(StateProvisionError::IdentityMismatch);
    }
    if facts.tree_id != 5
        || facts.inode != 256
        || !facts.directory
        || facts.uid != 0
        || facts.gid != 0
        || facts.mode & 0o022 != 0
    {
        return Err(StateProvisionError::UnsafeTopLevel);
    }
    Ok(())
}

#[allow(unsafe_code)]
fn create_volume(top: &File) -> Result<(), Errno> {
    type Args = linux_raw_sys::btrfs::btrfs_ioctl_vol_args;
    let mut args = Args { fd: 0, name: [0; 4088] };
    for (destination, byte) in args.name.iter_mut().zip(STATE_SUBVOLUME_NAME.bytes()) {
        *destination = byte as _;
    }
    // SAFETY: opcode and exact input layout are the Linux Btrfs UAPI pair.
    // The input is initialized, NUL-terminated and contains one fixed basename;
    // the kernel creates beneath the pinned directory FD, never a user path.
    let request = unsafe {
        Setter::<{ linux_raw_sys::ioctl::BTRFS_IOC_SUBVOL_CREATE as _ }, Args>::new(args)
    };
    // SAFETY: the request owns its correctly sized input for the duration of
    // this synchronous filesystem-mutating ioctl on the validated top-level FD.
    unsafe { ioctl(top, request) }
}

fn render_mount_unit(record: &MachineStateRecord) -> String {
    format!(
        "[Unit]\nDescription=Ryoku persistent recovery state\nBefore=ryoku-helper.service\n\n[Mount]\nWhat=/dev/disk/by-uuid/{}\nWhere=/var/lib/ryoku-state\nType=btrfs\nOptions=subvolid={},nodev,nosuid,noexec\n\n[Install]\nWantedBy=local-fs.target\n",
        record.identity().filesystem_uuid,
        record.identity().subvolume_id
    )
}

/// Resume boot-unit publication after machine identity was independently
/// provisioned. The current mount must match that protected root-side record;
/// observing an arbitrary mount never enrolls it as the expected volume.
/// The caller holds the provisioning lock and performs relabel/enable/start.
pub fn install_recorded_state_mount_unit(trusted_root: &File) -> Result<(), StateProvisionError> {
    if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0 {
        return Err(StateProvisionError::RootRequired);
    }
    let record = crate::machine_state::read_machine_state_record(trusted_root)
        .map_err(StateProvisionError::Record)?;
    crate::state_mount::collect_and_open_state_mount(trusted_root, record.identity())
        .map_err(StateProvisionError::Observation)?;
    crate::state_boot_unit::install(trusted_root, render_mount_unit(&record).as_bytes())
}

/// Verify the recorded unit and its fixed local-fs enablement without changing
/// either. A currently mounted store alone does not prove next-boot setup.
pub fn verify_recorded_state_mount_unit(trusted_root: &File) -> Result<(), StateProvisionError> {
    crate::state_mount::validate_trusted_root(trusted_root)
        .map_err(StateProvisionError::Observation)?;
    let record = crate::machine_state::read_machine_state_record(trusted_root)
        .map_err(StateProvisionError::Record)?;
    crate::state_mount::collect_and_open_state_mount(trusted_root, record.identity())
        .map_err(StateProvisionError::Observation)?;
    crate::state_boot_unit::verify_enabled(trusted_root, render_mount_unit(&record).as_bytes())
}

/// Reopen only a mounted partner identified by a protected creation receipt.
/// No ownership, mode, mount, or subvolume metadata is changed on mismatch.
pub(crate) fn reopen_created_state_mount(
    root: &File,
    record: &MachineStateRecord,
    expected_uuid: &str,
) -> Result<CreatedStateSubvolume, StateProvisionError> {
    let (descriptor, _) = crate::state_mount::collect_and_open_state_mount(root, record.identity())
        .map_err(StateProvisionError::Observation)?;
    let directory = File::from(descriptor);
    let volume = read_btrfs_subvolume_info(&directory).map_err(StateProvisionError::Observation)?;
    if format_uuid(&volume.uuid) != expected_uuid || volume.parent_uuid != [0; 16] {
        return Err(StateProvisionError::IdentityMismatch);
    }
    Ok(CreatedStateSubvolume {
        directory,
        record: record.clone(),
        subvolume_uuid: expected_uuid.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialized_rollback_root_does_not_relax_fresh_creation_identity() {
        let fsid = [3; 16];
        let record =
            MachineStateRecord::from_provisioned_subvolume(format_uuid(&fsid), 261).unwrap();
        let check = |root, parent, existing| {
            validate_initialization_root_identity(&fsid, root, parent, existing)
        };
        assert!(check(256, 5, None).is_ok());
        assert!(check(256, 5, Some(record.identity())).is_ok());
        // Classic rollback creates a distinct writable root in /.snapshots,
        // while the same independent state volume remains directly under ID 5.
        assert!(check(300, 259, Some(record.identity())).is_ok());
        assert!(check(300, 259, None).is_err());
        assert!(check(261, 5, Some(record.identity())).is_err());
        for parent in [0, 1, 6, 255] {
            assert!(check(300, parent, None).is_err());
            assert!(check(300, parent, Some(record.identity())).is_err());
        }
        for foreign in [[0; 16], [4; 16]] {
            assert!(
                validate_initialization_root_identity(&foreign, 300, 259, Some(record.identity()))
                    .is_err()
            );
        }
        assert!(validate_initialization_root_identity(&[0; 16], 256, 5, None).is_err());
    }

    #[test]
    fn public_creation_rejects_unprivileged_or_non_root_descriptor_before_ioctl() {
        let directory = File::open("/tmp").unwrap();
        let record = MachineStateRecord::from_provisioned_subvolume(
            "01234567-89ab-cdef-0123-456789abcdef",
            300,
        )
        .unwrap();
        for result in [
            create_state_subvolume(&directory, &directory),
            reopen_created_from_top_level(
                &directory,
                &directory,
                &record,
                "01234567-89ab-cdef-0123-456789abcdef",
            ),
        ] {
            if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0
            {
                assert!(matches!(result, Err(StateProvisionError::RootRequired)));
            } else {
                assert!(matches!(
                    result,
                    Err(StateProvisionError::Observation(StateMountError::UnsafeRootDescriptor))
                ));
            }
        }
    }

    #[test]
    fn top_level_requires_matching_filesystem_and_exact_kernel_root_identity() {
        let fs = [1; 16];
        let valid =
            TopLevelFacts { tree_id: 5, inode: 256, directory: true, uid: 0, gid: 0, mode: 0o755 };
        assert!(validate_top_level(&fs, &fs, &valid).is_ok());
        for (id, ino, dir, uid, gid, mode) in [
            (256, 256, true, 0, 0, 0o755),
            (5, 257, true, 0, 0, 0o755),
            (5, 256, false, 0, 0, 0o755),
            (5, 256, true, 1, 0, 0o755),
            (5, 256, true, 0, 1, 0o755),
            (5, 256, true, 0, 0, 0o775),
        ] {
            assert!(matches!(
                validate_top_level(
                    &fs,
                    &fs,
                    &TopLevelFacts { tree_id: id, inode: ino, directory: dir, uid, gid, mode }
                ),
                Err(StateProvisionError::UnsafeTopLevel)
            ));
        }
        assert!(matches!(
            validate_top_level(&fs, &[2; 16], &valid),
            Err(StateProvisionError::IdentityMismatch)
        ));
    }

    #[test]
    fn unit_pins_created_volume_id_and_fixed_mountpoint_without_adopting_names() {
        let record = MachineStateRecord::from_provisioned_subvolume(
            "01234567-89ab-cdef-0123-456789abcdef",
            300,
        )
        .unwrap();
        let unit = render_mount_unit(&record);
        assert!(unit.contains("What=/dev/disk/by-uuid/01234567-89ab-cdef-0123-456789abcdef\n"));
        assert!(unit.contains("Where=/var/lib/ryoku-state\n"));
        assert!(unit.contains("Options=subvolid=300,nodev,nosuid,noexec\n"));
        assert!(!unit.contains("subvol=@"));
    }
}
