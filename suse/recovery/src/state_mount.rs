//! Fail-closed validation contract for the protected Ryoku state mount.
//!
//! This module validates kernel observations supplied by the privileged mount
//! inspector and the already-opened mountpoint descriptor. It deliberately
//! does not fabricate mount or Btrfs identity from a pathname. A production
//! caller must obtain [`MountEvidence`] from a trusted mountinfo/Btrfs
//! inspector and must open the fixed path below a trusted descriptor for `/`.

use rustix::{
    fd::AsFd,
    fs::{AtFlags, FileType, OFlags, ResolveFlags, StatxFlags, fstat, open, openat2, statx},
    io::Errno,
    ioctl::{Getter, ioctl},
};
use std::{
    fs::File,
    io::{Read, Take},
};

/// The one supported state location.
pub const STATE_MOUNTPOINT: &str = "/var/lib/ryoku-state";
const STATE_RELATIVE_PATH: &str = "var/lib/ryoku-state";
const MAX_MOUNTINFO_BYTES: u64 = 4 * 1024 * 1024;

/// Machine-record identity required before a protected write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateMountIdentity {
    pub filesystem_uuid: String,
    pub subvolume_id: u64,
    /// The Btrfs top-level subvolume ID is 5.
    pub parent_subvolume_id: u64,
}

/// Facts collected by the privileged Linux mount/Btrfs inspector.
///
/// Values must come from the live mount table and Btrfs metadata, not from
/// persisted machine state or a caller-supplied path. Parent identity is
/// intentionally mandatory; `None` fails closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountEvidence {
    pub mountpoint: String,
    pub mount_id: u64,
    pub filesystem_type: String,
    pub filesystem_uuid: String,
    pub subvolume_id: Option<u64>,
    pub parent_subvolume_id: Option<u64>,
    pub read_only: bool,
}

/// Typed reason protected state is unavailable for writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateMountError {
    MissingMount,
    WrongMountpoint,
    WrongFilesystem,
    ReadOnly,
    IdentityMismatch,
    MissingSubvolumeParent,
    UnsafeDescriptor,
    UnsafeRootDescriptor,
    MountSubstituted,
    KernelObservationUnavailable,
    InvalidMountInfo,
    MountInfoTooLarge,
    NotBtrfsSubvolume,
    NotSubvolumeRoot,
    Filesystem(Errno),
}

impl std::fmt::Display for StateMountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingMount => "protected state mount is absent",
            Self::WrongMountpoint => "protected state mount is at an unexpected location",
            Self::WrongFilesystem => "protected state mount is not Btrfs",
            Self::ReadOnly => "protected state mount is read-only",
            Self::IdentityMismatch => "protected state mount identity differs from machine record",
            Self::MissingSubvolumeParent => "Btrfs parent subvolume identity is unavailable",
            Self::UnsafeDescriptor => {
                "protected state descriptor is not root-owned mode 0700 directory"
            }
            Self::UnsafeRootDescriptor => "supplied root descriptor is not the process root mount",
            Self::MountSubstituted => "protected state descriptor resolves to a different mount",
            Self::KernelObservationUnavailable => "required kernel mount identity is unavailable",
            Self::InvalidMountInfo => "kernel mount table could not be parsed unambiguously",
            Self::MountInfoTooLarge => "kernel mount table exceeds the inspection bound",
            Self::NotBtrfsSubvolume => "protected state directory is not a Btrfs subvolume root",
            Self::NotSubvolumeRoot => "protected state mount is not at the Btrfs subvolume root",
            Self::Filesystem(_) => "protected state mount inspection failed",
        })
    }
}

impl std::error::Error for StateMountError {}

/// Collect live mount and Btrfs identity for the fixed protected mount, pin
/// its descriptor, and compare it with the installed machine identity.
///
/// No caller-supplied mount or device path is accepted. Mount state comes from
/// `/proc/self/mountinfo`; filesystem UUID and subvolume IDs come from Btrfs
/// read-only ioctls on the pinned directory descriptor. A missing mount table
/// entry, unsupported ioctl, ambiguous record, or identity mismatch fails
/// closed.
pub fn collect_and_open_state_mount(
    trusted_root: &impl AsFd,
    expected: &StateMountIdentity,
) -> Result<(rustix::fd::OwnedFd, MountEvidence), StateMountError> {
    validate_trusted_root(trusted_root)?;
    let state_fd = openat2(
        trusted_root,
        STATE_RELATIVE_PATH,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .map_err(|error| {
        if error == Errno::NOENT {
            StateMountError::MissingMount
        } else {
            StateMountError::Filesystem(error)
        }
    })?;
    validate_state_descriptor(&state_fd)?;
    let statx = statx(&state_fd, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    if statx.stx_mask & StatxFlags::MNT_ID.bits() == 0 || statx.stx_mnt_id == 0 {
        return Err(StateMountError::KernelObservationUnavailable);
    }

    let mountinfo = read_mountinfo()?;
    let mount = parse_mountinfo_for_id(&mountinfo, statx.stx_mnt_id)?;
    if mount.mountpoint != STATE_MOUNTPOINT {
        return Err(StateMountError::WrongMountpoint);
    }
    if mount.filesystem_type != "btrfs" {
        return Err(StateMountError::WrongFilesystem);
    }
    if mount.read_only {
        return Err(StateMountError::ReadOnly);
    }
    validate_btrfs_root_inode(fstat(&state_fd).map_err(StateMountError::Filesystem)?.st_ino)?;

    let fs_info = read_btrfs_fs_info(&state_fd)?;
    let subvolume_info = read_btrfs_subvolume_info(&state_fd)?;
    let evidence = MountEvidence {
        mountpoint: mount.mountpoint,
        mount_id: statx.stx_mnt_id,
        filesystem_type: mount.filesystem_type,
        filesystem_uuid: format_uuid(&fs_info.fsid),
        subvolume_id: Some(subvolume_info.treeid),
        parent_subvolume_id: Some(subvolume_info.parent_id),
        read_only: mount.read_only,
    };
    validate_evidence(&evidence, expected)?;
    Ok((state_fd, evidence))
}

fn validate_state_descriptor<Fd: AsFd>(fd: Fd) -> Result<(), StateMountError> {
    let stat = fstat(fd).map_err(StateMountError::Filesystem)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(StateMountError::UnsafeDescriptor);
    }
    validate_btrfs_root_inode(stat.st_ino)?;
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct ParsedMountInfo {
    /// Root path inside the mounted filesystem. For Btrfs subvolume mounts
    /// this may name the subvolume path, so it is not necessarily `/`.
    root: String,
    mountpoint: String,
    filesystem_type: String,
    read_only: bool,
}

fn read_mountinfo() -> Result<String, StateMountError> {
    let file = File::open("/proc/self/mountinfo")
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    let mut bounded: Take<File> = file.take(MAX_MOUNTINFO_BYTES + 1);
    let mut contents = String::new();
    bounded
        .read_to_string(&mut contents)
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    if contents.len() as u64 > MAX_MOUNTINFO_BYTES {
        return Err(StateMountError::MountInfoTooLarge);
    }
    Ok(contents)
}

fn parse_mountinfo_for_id(
    contents: &str,
    expected_id: u64,
) -> Result<ParsedMountInfo, StateMountError> {
    let mut found = None;
    for line in contents.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            return Err(StateMountError::InvalidMountInfo);
        };
        let before: Vec<_> = before.split_ascii_whitespace().collect();
        let after: Vec<_> = after.split_ascii_whitespace().collect();
        if before.len() < 6 || after.len() < 3 {
            return Err(StateMountError::InvalidMountInfo);
        }
        let mount_id: u64 = before[0].parse().map_err(|_| StateMountError::InvalidMountInfo)?;
        if mount_id != expected_id {
            continue;
        }
        let root = decode_mount_field(before[3])?;
        if !root.starts_with('/') {
            return Err(StateMountError::InvalidMountInfo);
        }
        let mountpoint = decode_mount_field(before[4])?;
        let mut ro = false;
        let mut rw = false;
        for option in before[5].split(',') {
            ro |= option == "ro";
            rw |= option == "rw";
        }
        if ro == rw {
            return Err(StateMountError::InvalidMountInfo);
        }
        let parsed = ParsedMountInfo {
            root,
            mountpoint,
            filesystem_type: after[0].to_string(),
            read_only: ro,
        };
        if found.replace(parsed).is_some() {
            return Err(StateMountError::InvalidMountInfo);
        }
    }
    found.ok_or(StateMountError::MissingMount)
}

/// Btrfs assigns inode 256 to the root directory of every subvolume. The
/// GET_SUBVOL_INFO ioctl reports the containing subvolume even for a regular
/// directory bind-mounted from inside it; requiring inode 256 proves that the
/// pinned mount root itself is the subvolume root. The mountinfo `root` field
/// is decoded separately, but cannot be required to equal `/`: Btrfs reports
/// the subvolume's filesystem-relative path there for normal subvolume mounts.
fn validate_btrfs_root_inode(inode: u64) -> Result<(), StateMountError> {
    if inode == 256 { Ok(()) } else { Err(StateMountError::NotSubvolumeRoot) }
}

fn decode_mount_field(value: &str) -> Result<String, StateMountError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 3 >= bytes.len() {
            return Err(StateMountError::InvalidMountInfo);
        }
        let replacement = match &bytes[index + 1..index + 4] {
            b"040" => b' ',
            b"011" => b'\t',
            b"012" => b'\n',
            b"134" => b'\\',
            _ => return Err(StateMountError::InvalidMountInfo),
        };
        decoded.push(replacement);
        index += 4;
    }
    String::from_utf8(decoded).map_err(|_| StateMountError::InvalidMountInfo)
}

#[allow(unsafe_code)]
fn read_btrfs_fs_info<Fd: AsFd>(
    fd: Fd,
) -> Result<linux_raw_sys::btrfs::btrfs_ioctl_fs_info_args, StateMountError> {
    type FsInfo = linux_raw_sys::btrfs::btrfs_ioctl_fs_info_args;
    // SAFETY: `Getter` requires the caller to guarantee a valid ioctl opcode
    // and output layout. The UAPI request and exact kernel struct type below
    // come from linux-raw-sys for this target architecture.
    let request =
        unsafe { Getter::<{ linux_raw_sys::ioctl::BTRFS_IOC_FS_INFO as _ }, FsInfo>::new() };
    // SAFETY: the request number and output layout are the Linux Btrfs UAPI
    // definitions provided by linux-raw-sys; this ioctl only fills the output
    // structure and does not mutate filesystem state.
    unsafe { ioctl(fd, request) }.map_err(|_| StateMountError::NotBtrfsSubvolume)
}

#[allow(unsafe_code)]
fn read_btrfs_subvolume_info<Fd: AsFd>(
    fd: Fd,
) -> Result<linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args, StateMountError> {
    type SubvolumeInfo = linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args;
    // SAFETY: `Getter` requires the caller to guarantee a valid ioctl opcode
    // and output layout. The UAPI request and exact kernel struct type below
    // come from linux-raw-sys for this target architecture.
    let request = unsafe {
        Getter::<{ linux_raw_sys::ioctl::BTRFS_IOC_GET_SUBVOL_INFO as _ }, SubvolumeInfo>::new()
    };
    // SAFETY: the request number and output layout are the Linux Btrfs UAPI
    // definitions provided by linux-raw-sys; this ioctl only fills the output
    // structure and does not mutate filesystem state.
    unsafe { ioctl(fd, request) }.map_err(|_| StateMountError::NotBtrfsSubvolume)
}

fn format_uuid(uuid: &[u8; 16]) -> String {
    let hex: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Open the fixed state mount beneath a trusted descriptor for `/`, then
/// verify its descriptor security and mount ID against live mount evidence.
/// The returned descriptor pins the validated directory for subsequent
/// descriptor-relative operations. The caller must repeat this check before
/// each protected write.
pub fn open_validated_state_mount(
    trusted_root: &impl AsFd,
    evidence: Option<&MountEvidence>,
    expected: &StateMountIdentity,
) -> Result<rustix::fd::OwnedFd, StateMountError> {
    validate_trusted_root(trusted_root)?;
    let evidence = evidence.ok_or(StateMountError::MissingMount)?;
    validate_evidence(evidence, expected)?;

    let state_fd = openat2(
        trusted_root,
        STATE_RELATIVE_PATH,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .map_err(StateMountError::Filesystem)?;

    let stat = fstat(&state_fd).map_err(StateMountError::Filesystem)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_mode & 0o7777 != 0o700
    {
        return Err(StateMountError::UnsafeDescriptor);
    }

    let mount = statx(&state_fd, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    if mount.stx_mask & StatxFlags::MNT_ID.bits() == 0 || mount.stx_mnt_id != evidence.mount_id {
        return Err(StateMountError::MountSubstituted);
    }
    Ok(state_fd)
}

/// Ensure the caller-provided anchor is exactly this process's `/` mount,
/// rather than a different root-owned directory or a bind-mounted substitute.
fn validate_trusted_root(root: &impl AsFd) -> Result<(), StateMountError> {
    let supplied = fstat(root).map_err(StateMountError::Filesystem)?;
    let actual_fd =
        open("/", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, rustix::fs::Mode::empty())
            .map_err(StateMountError::Filesystem)?;
    let actual = fstat(&actual_fd).map_err(StateMountError::Filesystem)?;
    if FileType::from_raw_mode(supplied.st_mode) != FileType::Directory
        || supplied.st_mode & 0o022 != 0
        || supplied.st_dev != actual.st_dev
        || supplied.st_ino != actual.st_ino
    {
        return Err(StateMountError::UnsafeRootDescriptor);
    }
    let supplied_mount = statx(root, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    let actual_mount = statx(&actual_fd, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    if supplied_mount.stx_mask & StatxFlags::MNT_ID.bits() == 0
        || actual_mount.stx_mask & StatxFlags::MNT_ID.bits() == 0
        || supplied_mount.stx_mnt_id != actual_mount.stx_mnt_id
    {
        return Err(StateMountError::UnsafeRootDescriptor);
    }
    Ok(())
}

/// Validate fields that must agree before opening or writing the state store.
pub fn validate_evidence(
    evidence: &MountEvidence,
    expected: &StateMountIdentity,
) -> Result<(), StateMountError> {
    if evidence.mountpoint != STATE_MOUNTPOINT {
        return Err(StateMountError::WrongMountpoint);
    }
    if evidence.filesystem_type != "btrfs" {
        return Err(StateMountError::WrongFilesystem);
    }
    if evidence.read_only {
        return Err(StateMountError::ReadOnly);
    }
    let parent = evidence.parent_subvolume_id.ok_or(StateMountError::MissingSubvolumeParent)?;
    let subvolume = evidence.subvolume_id.ok_or(StateMountError::MissingSubvolumeParent)?;
    if evidence.filesystem_uuid != expected.filesystem_uuid
        || subvolume != expected.subvolume_id
        || parent != expected.parent_subvolume_id
        || parent != 5
    {
        return Err(StateMountError::IdentityMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO_FIXTURE: &str = "25 1 0:22 / /var/lib/ryoku-state rw,nosuid,nodev shared:1 - btrfs /dev/vda2 rw,space_cache=v2\n";

    fn expected() -> StateMountIdentity {
        StateMountIdentity {
            filesystem_uuid: "11111111-2222-3333-4444-555555555555".into(),
            subvolume_id: 256,
            parent_subvolume_id: 5,
        }
    }

    fn evidence() -> MountEvidence {
        MountEvidence {
            mountpoint: STATE_MOUNTPOINT.into(),
            mount_id: 41,
            filesystem_type: "btrfs".into(),
            filesystem_uuid: expected().filesystem_uuid,
            subvolume_id: Some(256),
            parent_subvolume_id: Some(5),
            read_only: false,
        }
    }

    #[test]
    fn accepts_exact_writable_dedicated_subvolume() {
        assert_eq!(validate_evidence(&evidence(), &expected()), Ok(()));
    }

    #[test]
    fn rejects_absent_mount() {
        assert_eq!(
            open_validated_state_mount(
                &rustix::fs::open(
                    "/",
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .unwrap(),
                None,
                &expected(),
            )
            .unwrap_err(),
            StateMountError::MissingMount
        );
    }

    #[test]
    fn root_anchor_must_be_the_process_root_mount() {
        let root = open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        assert_eq!(validate_trusted_root(&root), Ok(()));

        let unrelated = open(
            "/tmp",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        assert_eq!(validate_trusted_root(&unrelated), Err(StateMountError::UnsafeRootDescriptor));
    }

    #[test]
    fn rejects_non_fixed_mountpoint_and_non_btrfs() {
        let mut observed = evidence();
        observed.mountpoint = "/var/lib/ryoku-state-copy".into();
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::WrongMountpoint)
        );
        observed = evidence();
        observed.filesystem_type = "xfs".into();
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::WrongFilesystem)
        );
    }

    #[test]
    fn rejects_read_only_and_identity_mismatch() {
        let mut observed = evidence();
        observed.read_only = true;
        assert_eq!(validate_evidence(&observed, &expected()), Err(StateMountError::ReadOnly));
        observed = evidence();
        observed.filesystem_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into();
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::IdentityMismatch)
        );
        observed = evidence();
        observed.parent_subvolume_id = Some(257);
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::IdentityMismatch)
        );
    }

    #[test]
    fn missing_parent_fails_closed() {
        let mut observed = evidence();
        observed.parent_subvolume_id = None;
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::MissingSubvolumeParent)
        );
    }

    #[test]
    fn parses_mountinfo_for_the_descriptor_mount_id() {
        let parsed = parse_mountinfo_for_id(MOUNTINFO_FIXTURE, 25).unwrap();
        assert_eq!(parsed.mountpoint, STATE_MOUNTPOINT);
        assert_eq!(parsed.root, "/");
        assert_eq!(parsed.filesystem_type, "btrfs");
        assert!(!parsed.read_only);
        assert_eq!(
            parse_mountinfo_for_id(MOUNTINFO_FIXTURE, 26),
            Err(StateMountError::MissingMount)
        );
    }

    #[test]
    fn parses_mountinfo_read_only_and_escaped_mountpoints() {
        let fixture = "26 1 0:22 / /var/lib/ryoku\\040state ro - btrfs /dev/vda2 ro\n";
        let parsed = parse_mountinfo_for_id(fixture, 26).unwrap();
        assert_eq!(parsed.mountpoint, "/var/lib/ryoku state");
        assert!(parsed.read_only);
    }

    #[test]
    fn accepts_btrfs_subvolume_root_path_and_rejects_bind_root_subpath() {
        // Btrfs mountinfo's root field may be the filesystem-relative path of
        // the mounted subvolume. The descriptor inode check proves the mount
        // is at that subvolume's root.
        let subvolume = "27 1 0:22 /@ryoku-state /var/lib/ryoku-state rw - btrfs /dev/vda2 rw\n";
        let parsed = parse_mountinfo_for_id(subvolume, 27).unwrap();
        assert_eq!(parsed.root, "/@ryoku-state");
        assert_eq!(validate_btrfs_root_inode(256), Ok(()));

        // A same-filesystem bind mount of a directory inside the subvolume
        // retains the containing tree ID but has a non-root inode and a
        // filesystem-relative mount root.
        let bind_subpath =
            "28 1 0:22 /@ryoku-state/catalog /var/lib/ryoku-state rw - btrfs /dev/vda2 rw\n";
        let parsed = parse_mountinfo_for_id(bind_subpath, 28).unwrap();
        assert_eq!(parsed.root, "/@ryoku-state/catalog");
        assert_eq!(validate_btrfs_root_inode(257), Err(StateMountError::NotSubvolumeRoot));
    }

    #[test]
    fn rejects_ambiguous_or_malformed_mount_options() {
        let both = "25 1 0:22 / /var/lib/ryoku-state rw,ro - btrfs /dev/vda2 rw\n";
        assert_eq!(parse_mountinfo_for_id(both, 25), Err(StateMountError::InvalidMountInfo));
        let malformed = "25 1 0:22 / /var/lib/ryoku-state rw -\n";
        assert_eq!(parse_mountinfo_for_id(malformed, 25), Err(StateMountError::InvalidMountInfo));
    }

    #[test]
    fn formats_btrfs_filesystem_uuid() {
        assert_eq!(
            format_uuid(&[
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
                0x32, 0x10,
            ]),
            "01234567-89ab-cdef-fedc-ba9876543210"
        );
    }
}
