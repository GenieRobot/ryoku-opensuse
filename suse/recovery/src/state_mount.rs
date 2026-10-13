//! Fail-closed validation contract for the protected Ryoku state mount.
//!
//! This module validates kernel observations supplied by the privileged mount
//! inspector and the already-opened mountpoint descriptor. It deliberately
//! does not fabricate mount or Btrfs identity from a pathname. A production
//! caller must obtain [`MountEvidence`] from a trusted mountinfo/Btrfs
//! inspector and must open the fixed path below a trusted descriptor for `/`.

use rustix::{
    fd::AsFd,
    fs::{AtFlags, FileType, OFlags, ResolveFlags, StatxFlags, fstat, openat2, statx},
    io::Errno,
};

/// The one supported state location.
pub const STATE_MOUNTPOINT: &str = "/var/lib/ryoku-state";
const STATE_RELATIVE_PATH: &str = "var/lib/ryoku-state";

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
/// intentionally mandatory: `None` fails closed because this implementation
/// has no safe Btrfs parent-subvolume query yet.
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
    MountSubstituted,
    KernelObservationUnavailable,
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
            Self::MountSubstituted => "protected state descriptor resolves to a different mount",
            Self::KernelObservationUnavailable => "required kernel mount identity is unavailable",
            Self::Filesystem(_) => "protected state mount inspection failed",
        })
    }
}

impl std::error::Error for StateMountError {}

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
}
