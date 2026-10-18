//! Fail-closed validation contract for the protected Ryoku state mount.
//!
//! The live [`collect_and_open_state_mount`] entry point reads mountinfo and
//! Btrfs identity from the kernel and returns the pinned validated directory.
//! The lower-level evidence validator is also available to callers that
//! already have trusted observations. Both paths require a descriptor verified
//! as this process's `/` mount and never derive filesystem identity from a
//! caller-supplied path.

use rustix::{
    fd::AsFd,
    fs::{AtFlags, FileType, OFlags, ResolveFlags, StatxFlags, fstat, open, openat2, statx},
    io::Errno,
    ioctl::{Getter, ioctl},
};
use ryoku_snapper::{
    BootLayout, ConfigState, FilesystemKind, LayoutAssessment, LayoutObservation, assess_layout,
};
use std::{
    fs::File,
    io::{Read, Take},
};

/// The one supported state location.
pub const STATE_MOUNTPOINT: &str = "/var/lib/ryoku-state";
/// Shared advisory lock inode used to serialize Ryoku protected mutations
/// and short-lived recovery-state readers. Created only by explicit state
/// initialization.
pub const MUTATION_LOCK_NAME: &str = "mutation.lock";
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
    /// Mount root within the filesystem (for Btrfs this can be the subvolume
    /// path, such as `/@ryoku-state`, rather than `/`).
    pub filesystem_root: String,
    pub mount_id: u64,
    pub filesystem_type: String,
    /// Kernel-reported filesystem source; its stable identity is checked with
    /// the Btrfs UUID read from the pinned descriptor below.
    pub mount_source: String,
    /// Per-mount flags from the sixth field in `/proc/self/mountinfo`.
    pub mount_options: Vec<String>,
    /// Superblock options from the final mountinfo field.
    pub super_options: Vec<String>,
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
    UnsafeMountOptions,
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
            Self::UnsafeMountOptions => {
                "protected state mount options differ from the required safe subvolume policy"
            }
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
    collect_and_open_state_mount_with_access(trusted_root, expected, true)
}

/// Validate and pin the same dedicated state mount for readers that operate
/// inside a read-only filesystem sandbox. Identity checks remain identical;
/// only the requirement that the mount be writable is omitted.
pub fn collect_and_open_state_mount_readonly(
    trusted_root: &impl AsFd,
    expected: &StateMountIdentity,
) -> Result<(rustix::fd::OwnedFd, MountEvidence), StateMountError> {
    collect_and_open_state_mount_with_access(trusted_root, expected, false)
}

fn collect_and_open_state_mount_with_access(
    trusted_root: &impl AsFd,
    expected: &StateMountIdentity,
    require_writable: bool,
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
    validate_btrfs_root_inode(fstat(&state_fd).map_err(StateMountError::Filesystem)?.st_ino)?;

    let fs_info = read_btrfs_fs_info(&state_fd)?;
    let subvolume_info = read_btrfs_subvolume_info(&state_fd)?;
    let evidence = MountEvidence {
        mountpoint: mount.mountpoint,
        filesystem_root: mount.root,
        mount_id: statx.stx_mnt_id,
        filesystem_type: mount.filesystem_type,
        mount_source: mount.source,
        mount_options: mount.mount_options,
        super_options: mount.super_options,
        filesystem_uuid: format_uuid(&fs_info.fsid),
        subvolume_id: Some(subvolume_info.treeid),
        parent_subvolume_id: Some(subvolume_info.parent_id),
        read_only: mount.read_only,
    };
    if require_writable {
        validate_evidence(&evidence, expected)?;
    } else {
        validate_readonly_evidence(&evidence, expected)?;
    }
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
    mount_id: u64,
    /// Root path inside the mounted filesystem. For Btrfs subvolume mounts
    /// this may name the subvolume path, so it is not necessarily `/`.
    root: String,
    mountpoint: String,
    filesystem_type: String,
    source: String,
    mount_options: Vec<String>,
    super_options: Vec<String>,
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
        let mount_options = parse_mount_options(before[5])?;
        let source = decode_mount_field(after[1])?;
        if source.is_empty() {
            return Err(StateMountError::InvalidMountInfo);
        }
        let super_options = parse_mount_options(after[2])?;
        let mut ro = false;
        let mut rw = false;
        for option in &mount_options {
            ro |= option == "ro";
            rw |= option == "rw";
        }
        if ro == rw {
            return Err(StateMountError::InvalidMountInfo);
        }
        let parsed = ParsedMountInfo {
            mount_id,
            root,
            mountpoint,
            filesystem_type: after[0].to_string(),
            source,
            mount_options,
            super_options,
            read_only: ro,
        };
        if found.replace(parsed).is_some() {
            return Err(StateMountError::InvalidMountInfo);
        }
    }
    found.ok_or(StateMountError::MissingMount)
}

fn parse_mount_options(value: &str) -> Result<Vec<String>, StateMountError> {
    if value.is_empty() {
        return Err(StateMountError::InvalidMountInfo);
    }
    let options: Vec<_> = value.split(',').map(str::to_owned).collect();
    if options.iter().any(|option| option.is_empty()) {
        return Err(StateMountError::InvalidMountInfo);
    }
    Ok(options)
}

/// Inspect the live root and `/boot` mounts and classify only protection that
/// can be established from the current mount namespace and Snapper. A
/// separately mounted `/boot` is conservatively unsupported until its own
/// snapshot coverage is proven. This root/boot probe does not assess the Ryoku
/// state filesystem; callers must validate it through the machine-state record
/// and linked-generation reader instead of inferring coverage from this result.
///
/// This is an observation, not authorization or a checkpoint. Callers must
/// still verify a fresh linked pre-change checkpoint before any protected
/// mutation.
pub fn detect_host_layout() -> Result<LayoutAssessment, StateMountError> {
    let root = File::open("/").map_err(|_| StateMountError::KernelObservationUnavailable)?;
    validate_trusted_root(&root)?;
    let root_id = mount_id(&root)?;
    let boot = match openat2(
        &root,
        "boot",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(boot) => Some(File::from(boot)),
        Err(Errno::NOENT) => None,
        Err(error) => return Err(StateMountError::Filesystem(error)),
    };
    let mountinfo = read_mountinfo()?;
    let root_mount = parse_mountinfo_for_id(&mountinfo, root_id)?;
    let boot_id = boot.as_ref().map(mount_id).transpose()?;
    let boot_mount = boot_id.map(|id| parse_mountinfo_for_id(&mountinfo, id)).transpose()?;
    let nested_boot_mount = has_distinct_mount_under_boot(&mountinfo, root_id)?;

    let root_is_btrfs_subvolume = if root_mount.filesystem_type == "btrfs" {
        let info = read_btrfs_subvolume_info(&root)?;
        fstat(&root).map_err(StateMountError::Filesystem)?.st_ino == 256 && info.treeid >= 256
    } else {
        false
    };
    let root_snapper = if root_mount.filesystem_type == "btrfs" {
        let fs_info = read_btrfs_fs_info(&root)?;
        if crate::root_capture_mapping::RootCaptureMapping::open(&format_uuid(&fs_info.fsid))
            .is_ok()
        {
            ConfigState::Ready
        } else {
            ConfigState::Unknown
        }
    } else {
        ConfigState::Missing
    };
    Ok(assess_host_mounts(
        &root_mount,
        boot_mount.as_ref(),
        nested_boot_mount,
        root_is_btrfs_subvolume,
        root_snapper,
    ))
}

fn assess_host_mounts(
    root: &ParsedMountInfo,
    boot: Option<&ParsedMountInfo>,
    nested_boot_mount: bool,
    root_is_btrfs_subvolume: bool,
    root_snapper: ConfigState,
) -> LayoutAssessment {
    let root_filesystem =
        if root.filesystem_type == "btrfs" { FilesystemKind::Btrfs } else { FilesystemKind::Other };
    let boot_layout = if nested_boot_mount {
        BootLayout::SeparateUnsnapshotted
    } else if boot.is_some_and(|mount| mount.mount_id == root.mount_id) {
        BootLayout::IncludedInRoot
    } else if boot.is_some_and(|mount| mount.mountpoint == "/boot") {
        BootLayout::SeparateUnsnapshotted
    } else {
        BootLayout::Unknown
    };
    assess_layout(LayoutObservation {
        root_filesystem,
        root_is_btrfs_subvolume,
        root_snapper,
        // This probe reports only root/boot protection; configuration-only
        // coverage is deliberately withheld and stated separately by callers.
        ryoku_config_filesystem: FilesystemKind::Other,
        ryoku_config_snapper: ConfigState::Missing,
        boot: boot_layout,
    })
}

/// A root snapshot does not cover a distinct mount nested below `/boot`, even
/// when `/boot` itself resolves to the root mount (for example, a separately
/// mounted EFI system partition at `/boot/efi`).
fn has_distinct_mount_under_boot(
    contents: &str,
    root_mount_id: u64,
) -> Result<bool, StateMountError> {
    let mut found = false;
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
        let mountpoint = decode_mount_field(before[4])?;
        if mount_id != root_mount_id && (mountpoint == "/boot" || mountpoint.starts_with("/boot/"))
        {
            found = true;
        }
    }
    Ok(found)
}

fn mount_id(descriptor: &File) -> Result<u64, StateMountError> {
    let observed = statx(descriptor, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID)
        .map_err(|_| StateMountError::KernelObservationUnavailable)?;
    if observed.stx_mask & StatxFlags::MNT_ID.bits() == 0 || observed.stx_mnt_id == 0 {
        return Err(StateMountError::KernelObservationUnavailable);
    }
    Ok(observed.stx_mnt_id)
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
pub(crate) fn read_btrfs_fs_info<Fd: AsFd>(
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
pub(crate) fn read_btrfs_subvolume_info<Fd: AsFd>(
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

pub(crate) fn format_uuid(uuid: &[u8; 16]) -> String {
    let hex: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Open the fixed state mount beneath a descriptor for `/`, then verify its
/// descriptor security and mount ID against trusted live mount evidence. The
/// returned descriptor pins the validated directory for subsequent
/// descriptor-relative operations. The caller must repeat this check before
/// each protected write. Prefer [`collect_and_open_state_mount`] when a trusted
/// evidence collector is not already available.
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
pub(crate) fn validate_trusted_root(root: &impl AsFd) -> Result<(), StateMountError> {
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
    if evidence.read_only {
        return Err(StateMountError::ReadOnly);
    }
    validate_readonly_evidence(evidence, expected)
}

/// Validate mount identity for operations that never mutate protected state.
pub fn validate_readonly_evidence(
    evidence: &MountEvidence,
    expected: &StateMountIdentity,
) -> Result<(), StateMountError> {
    if evidence.mountpoint != STATE_MOUNTPOINT {
        return Err(StateMountError::WrongMountpoint);
    }
    if !evidence.filesystem_root.starts_with('/') {
        return Err(StateMountError::InvalidMountInfo);
    }
    if evidence.mount_source.is_empty() {
        return Err(StateMountError::InvalidMountInfo);
    }
    if evidence.filesystem_type != "btrfs" {
        return Err(StateMountError::WrongFilesystem);
    }
    if !safe_mount_options(evidence, expected) {
        return Err(StateMountError::UnsafeMountOptions);
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

fn safe_mount_options(evidence: &MountEvidence, expected: &StateMountIdentity) -> bool {
    let has_once = |options: &[String], name: &str| {
        options.iter().filter(|option| option.as_str() == name).count() == 1
    };
    let has_never =
        |options: &[String], name: &str| options.iter().all(|option| option.as_str() != name);
    let subvolume_options: Vec<_> = evidence
        .super_options
        .iter()
        .filter_map(|option| option.strip_prefix("subvolid="))
        .collect();
    let expected_subvolume = expected.subvolume_id.to_string();
    has_once(&evidence.mount_options, "nodev")
        && has_once(&evidence.mount_options, "nosuid")
        && has_once(&evidence.mount_options, "noexec")
        && has_never(&evidence.mount_options, "dev")
        && has_never(&evidence.mount_options, "suid")
        && has_never(&evidence.mount_options, "exec")
        && subvolume_options.len() == 1
        && subvolume_options[0] == expected_subvolume
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO_FIXTURE: &str = "25 1 0:22 / /var/lib/ryoku-state rw,nosuid,nodev,noexec shared:1 - btrfs /dev/vda2 rw,space_cache=v2,subvolid=256\n";

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
            filesystem_root: "/".into(),
            mount_id: 41,
            filesystem_type: "btrfs".into(),
            mount_source: "/dev/vda2".into(),
            mount_options: vec!["rw".into(), "nosuid".into(), "nodev".into(), "noexec".into()],
            super_options: vec!["rw".into(), "space_cache=v2".into(), "subvolid=256".into()],
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
    fn mount_options_must_match_the_hardened_unit_and_exact_subvolume() {
        let mut observed = evidence();
        observed.mount_options.retain(|option| option != "nodev");
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::UnsafeMountOptions)
        );

        observed = evidence();
        observed.mount_options.push("exec".into());
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::UnsafeMountOptions)
        );

        observed = evidence();
        observed.super_options.retain(|option| option != "subvolid=256");
        observed.super_options.push("subvolid=257".into());
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::UnsafeMountOptions)
        );

        observed = evidence();
        observed.super_options.push("subvolid=256".into());
        assert_eq!(
            validate_evidence(&observed, &expected()),
            Err(StateMountError::UnsafeMountOptions)
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
        assert_eq!(parsed.source, "/dev/vda2");
        assert!(parsed.mount_options.contains(&"nodev".into()));
        assert!(parsed.mount_options.contains(&"nosuid".into()));
        assert!(parsed.mount_options.contains(&"noexec".into()));
        assert!(parsed.super_options.contains(&"subvolid=256".into()));
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
    fn host_layout_requires_verified_root_snapper_and_boot_coverage() {
        let root = ParsedMountInfo {
            mount_id: 1,
            root: "/@".into(),
            mountpoint: "/".into(),
            filesystem_type: "btrfs".into(),
            source: "/dev/root".into(),
            mount_options: vec!["rw".into()],
            super_options: vec!["rw".into()],
            read_only: false,
        };
        let covered = assess_host_mounts(&root, Some(&root), false, true, ConfigState::Ready);
        assert_eq!(covered.capability, ryoku_snapper::LayoutCapability::FullSystem);

        let separate_boot = ParsedMountInfo {
            mount_id: 2,
            root: "/@boot".into(),
            mountpoint: "/boot".into(),
            filesystem_type: "btrfs".into(),
            source: "/dev/boot".into(),
            mount_options: vec!["rw".into()],
            super_options: vec!["rw".into()],
            read_only: false,
        };
        let assessment =
            assess_host_mounts(&root, Some(&separate_boot), false, true, ConfigState::Ready);
        assert_eq!(assessment.capability, ryoku_snapper::LayoutCapability::Unsupported);

        let assessment = assess_host_mounts(&root, Some(&root), true, true, ConfigState::Ready);
        assert_eq!(assessment.capability, ryoku_snapper::LayoutCapability::Unsupported);

        let assessment = assess_host_mounts(&root, Some(&root), false, true, ConfigState::Unknown);
        assert_eq!(assessment.capability, ryoku_snapper::LayoutCapability::Unsupported);
    }

    #[test]
    fn detects_nested_efi_mounts_without_matching_prefixes_or_same_root_mounts() {
        let root_and_efi = concat!(
            "1 0 0:1 /@ / rw - btrfs /dev/root rw\n",
            "2 1 0:2 / /boot/efi rw - vfat /dev/efi rw\n",
        );
        assert_eq!(has_distinct_mount_under_boot(root_and_efi, 1), Ok(true));

        let same_root_only = "1 0 0:1 /@ / rw - btrfs /dev/root rw\n";
        assert_eq!(has_distinct_mount_under_boot(same_root_only, 1), Ok(false));

        let sibling_prefix = "2 1 0:2 / /booted rw - vfat /dev/other rw\n";
        assert_eq!(has_distinct_mount_under_boot(sibling_prefix, 1), Ok(false));

        let escaped_mountpoint = "2 1 0:2 / /boot/efi\\040partition rw - vfat /dev/efi rw\n";
        assert_eq!(has_distinct_mount_under_boot(escaped_mountpoint, 1), Ok(true));
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
