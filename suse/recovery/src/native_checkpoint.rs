//! Kernel verification of a newly created default-root Snapper checkpoint.
//!
//! Fresh capture checks current-root ancestry. Historical verification instead
//! requires an independently retained receipt and matches exact kernel identity.

use crate::{
    SnapshotLink,
    generation_capture::VerifiedSnapshotLink,
    restore::{RestoreJournal, RestorePhase, RestoreScope},
    state_mount::{
        self, StateMountError, format_uuid, read_btrfs_fs_info, read_btrfs_subvolume_info,
    },
};
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags, ResolveFlags, fstat, openat2},
};
use serde::{Deserialize, Serialize};
use std::fs::File;

/// Identity captured with a newly verified checkpoint. Persist this alongside
/// the sealed transaction linkage on protected storage outside rollback scope.
/// Deserialized receipt data is untrusted until kernel verification succeeds;
/// it does not establish authorization, provenance or catalog association.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootSnapshotReceipt {
    schema: u16,
    link: SnapshotLink,
    subvolume_id: u64,
    subvolume_uuid: String,
    origin_root_uuid: String,
}

impl RootSnapshotReceipt {
    pub fn link(&self) -> &SnapshotLink {
        &self.link
    }

    /// Structural validation only. Provenance comes from the protected sealed
    /// generation, and availability still requires live kernel verification.
    pub(crate) fn validate(&self) -> Result<(), NativeCheckpointError> {
        let nil = "00000000-0000-0000-0000-000000000000";
        validate_link(&self.link)?;
        if self.schema != 1
            || self.subvolume_id < 256
            || !crate::is_uuid(&self.link.filesystem_uuid)
            || !crate::is_uuid(&self.subvolume_uuid)
            || !crate::is_uuid(&self.origin_root_uuid)
            || self.link.filesystem_uuid == nil
            || self.subvolume_uuid == nil
            || self.origin_root_uuid == nil
            || self.subvolume_uuid == self.origin_root_uuid
        {
            return Err(NativeCheckpointError::IdentityMismatch);
        }
        Ok(())
    }
}

/// An exact retained source matched against a protected receipt. Unlike a new
/// checkpoint, this does not attest that its parent is the running root.
pub struct HistoricalRootCheckpoint {
    descriptor: OwnedFd,
    receipt: RootSnapshotReceipt,
}
impl HistoricalRootCheckpoint {
    pub fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }
    pub fn receipt(&self) -> &RootSnapshotReceipt {
        &self.receipt
    }
    pub fn revalidate(&self, root: &File) -> Result<(), NativeCheckpointError> {
        verify_historical_root_checkpoint(root, &self.receipt).map(|_| ())
    }
}

/// A pinned, independently observed native snapshot. Holding this descriptor
/// does not prevent privileged deletion or remounts; revalidate before use.
pub struct NativeRootCheckpoint {
    descriptor: OwnedFd,
    link: VerifiedSnapshotLink,
    pub subvolume_id: u64,
    pub subvolume_uuid: String,
    receipt: RootSnapshotReceipt,
}

impl NativeRootCheckpoint {
    pub fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }
    pub fn verified_link(&self) -> &VerifiedSnapshotLink {
        &self.link
    }
    pub fn receipt(&self) -> &RootSnapshotReceipt {
        &self.receipt
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum NativeCheckpointError {
    UnsupportedLink,
    NotSubvolumeRoot,
    IdentityMismatch,
    WritableSnapshot,
    NotCurrentRootSnapshot,
    RollbackTargetNotBooted,
    RollbackTargetMismatch,
    Snapper(ryoku_snapper::SnapperCliError),
    Observation(StateMountError),
    Filesystem(rustix::io::Errno),
}

impl std::fmt::Display for NativeCheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native checkpoint verification failed: {self:?}")
    }
}
impl std::error::Error for NativeCheckpointError {}

/// Opaque evidence that the machine has rebooted into the exact writable
/// target recorded for a pending root rollback attempt.
pub struct VerifiedRunningRollbackTarget {
    restore_id: crate::Identifier,
    target_snapshot: String,
    filesystem_uuid: String,
}

impl VerifiedRunningRollbackTarget {
    pub fn restore_id(&self) -> &crate::Identifier {
        &self.restore_id
    }

    pub fn target_snapshot(&self) -> &str {
        &self.target_snapshot
    }

    pub fn filesystem_uuid(&self) -> &str {
        &self.filesystem_uuid
    }
}

#[cfg(test)]
pub(crate) fn test_running_rollback_target(
    restore_id: crate::Identifier,
    target_snapshot: String,
    filesystem_uuid: String,
) -> VerifiedRunningRollbackTarget {
    VerifiedRunningRollbackTarget { restore_id, target_snapshot, filesystem_uuid }
}

/// Confirm after reboot that the actual running `/` is the exact writable
/// Snapper target associated with the original verified source. This remains
/// valid through later linked/system restore phases so retries can revalidate
/// the root before completing health checks or retention updates. The retained
/// source, target, and backup are reopened beneath the trusted root without
/// symlink traversal and checked with Btrfs ioctls.
pub fn verify_running_root_rollback(
    trusted_root: &File,
    journal: &RestoreJournal,
    source_receipt: &RootSnapshotReceipt,
) -> Result<VerifiedRunningRollbackTarget, NativeCheckpointError> {
    let result =
        journal.root_rollback.as_ref().ok_or(NativeCheckpointError::RollbackTargetMismatch)?;
    if !rollback_verification_phase_supported(journal.plan.scope, journal.phase)
        || journal.plan.snapper_config.as_str() != "root"
        || source_receipt.link.snapper_config.as_str() != journal.plan.snapper_config.as_str()
        || source_receipt.link.snapper_snapshot_id != journal.plan.snapshot_id
        || source_receipt.link.filesystem_uuid != journal.plan.filesystem_uuid
    {
        return Err(NativeCheckpointError::RollbackTargetMismatch);
    }
    let historical = verify_historical_root_checkpoint(trusted_root, source_receipt)?;
    let target_link = SnapshotLink::new(
        journal.plan.snapper_config.clone(),
        result.target_snapshot(),
        journal.plan.filesystem_uuid.clone(),
    )
    .map_err(|_| NativeCheckpointError::UnsupportedLink)?;
    let backup_link = SnapshotLink::new(
        journal.plan.snapper_config.clone(),
        result.backup_snapshot(),
        journal.plan.filesystem_uuid.clone(),
    )
    .map_err(|_| NativeCheckpointError::UnsupportedLink)?;
    let target = open_snapshot_descriptor(trusted_root, &target_link)?;
    let backup = open_snapshot_descriptor(trusted_root, &backup_link)?;
    let root_fs = read_btrfs_fs_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let source_fs =
        read_btrfs_fs_info(historical.descriptor()).map_err(NativeCheckpointError::Observation)?;
    let target_fs = read_btrfs_fs_info(&target).map_err(NativeCheckpointError::Observation)?;
    let backup_fs = read_btrfs_fs_info(&backup).map_err(NativeCheckpointError::Observation)?;
    let root_info =
        read_btrfs_subvolume_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let source_info = read_btrfs_subvolume_info(historical.descriptor())
        .map_err(NativeCheckpointError::Observation)?;
    let target_info =
        read_btrfs_subvolume_info(&target).map_err(NativeCheckpointError::Observation)?;
    let backup_info =
        read_btrfs_subvolume_info(&backup).map_err(NativeCheckpointError::Observation)?;
    validate_running_rollback_identity(
        journal,
        source_receipt,
        &root_fs.fsid,
        &source_fs.fsid,
        &target_fs.fsid,
        &backup_fs.fsid,
        &root_info,
        &source_info,
        &target_info,
        &backup_info,
    )?;
    let config = ryoku_snapper::ConfigName::new("root")
        .map_err(|_| NativeCheckpointError::RollbackTargetMismatch)?;
    let target = ryoku_snapper::SnapshotId::new(result.target_snapshot())
        .map_err(|_| NativeCheckpointError::RollbackTargetMismatch)?;
    let transaction = crate::restore::root_rollback_tag(&journal.restore_id)
        .map_err(|_| NativeCheckpointError::RollbackTargetMismatch)?;
    ryoku_snapper::SnapperRollbackBackend
        .verify_running_target(&config, &target, &transaction)
        .map_err(NativeCheckpointError::Snapper)?;
    Ok(VerifiedRunningRollbackTarget {
        restore_id: journal.restore_id.clone(),
        target_snapshot: result.target_snapshot().to_owned(),
        filesystem_uuid: journal.plan.filesystem_uuid.clone(),
    })
}

fn rollback_verification_phase_supported(scope: RestoreScope, phase: RestorePhase) -> bool {
    match scope {
        RestoreScope::SystemOnly => matches!(
            phase,
            RestorePhase::WritersStopped
                | RestorePhase::RootRollbackComplete
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        ),
        RestoreScope::CompleteLinked => matches!(
            phase,
            RestorePhase::WritersStopped
                | RestorePhase::RootRollbackComplete
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        ),
        RestoreScope::ConfigurationOnly => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_running_rollback_identity(
    journal: &RestoreJournal,
    source_receipt: &RootSnapshotReceipt,
    root_fs: &[u8; 16],
    source_fs: &[u8; 16],
    target_fs: &[u8; 16],
    backup_fs: &[u8; 16],
    root: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    source: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    target: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    backup: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
) -> Result<(), NativeCheckpointError> {
    let readonly = u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY);
    let result =
        journal.root_rollback.as_ref().ok_or(NativeCheckpointError::RollbackTargetMismatch)?;
    validate_rollback_objects_identity(
        journal,
        source_receipt,
        source_fs,
        target_fs,
        backup_fs,
        source,
        target,
        backup,
    )?;
    let fs_uuid = format_uuid(root_fs);
    if *root_fs == [0; 16]
        || root_fs != source_fs
        || fs_uuid != journal.plan.filesystem_uuid
        || root.flags & readonly != 0
        || root.treeid == 0
    {
        return Err(NativeCheckpointError::RollbackTargetMismatch);
    }
    if root.treeid != target.treeid || root.uuid != target.uuid {
        return Err(NativeCheckpointError::RollbackTargetNotBooted);
    }
    if result.backup_snapshot() == result.target_snapshot() {
        return Err(NativeCheckpointError::RollbackTargetMismatch);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_rollback_objects_identity(
    journal: &RestoreJournal,
    source_receipt: &RootSnapshotReceipt,
    source_fs: &[u8; 16],
    target_fs: &[u8; 16],
    backup_fs: &[u8; 16],
    source: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    target: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    backup: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
) -> Result<(), NativeCheckpointError> {
    let readonly = u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY);
    let result =
        journal.root_rollback.as_ref().ok_or(NativeCheckpointError::RollbackTargetMismatch)?;
    if *source_fs == [0; 16]
        || source_fs != target_fs
        || source_fs != backup_fs
        || format_uuid(source_fs) != journal.plan.filesystem_uuid
        || format_uuid(&source.uuid) != source_receipt.subvolume_uuid
        || target.treeid == source.treeid
        || target.flags & readonly != 0
        || target.parent_uuid != source.uuid
        || backup.flags & readonly == 0
        || backup.treeid == source.treeid
        || backup.treeid == target.treeid
        || result.backup_snapshot() == result.target_snapshot()
    {
        return Err(NativeCheckpointError::RollbackTargetMismatch);
    }
    Ok(())
}

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
    let descriptor = open_snapshot_descriptor(trusted_root, &link)?;
    let root_fs = read_btrfs_fs_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let snapshot_fs =
        read_btrfs_fs_info(&descriptor).map_err(NativeCheckpointError::Observation)?;
    let root =
        read_btrfs_subvolume_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let snapshot =
        read_btrfs_subvolume_info(&descriptor).map_err(NativeCheckpointError::Observation)?;
    validate_identity(&link.filesystem_uuid, &root_fs.fsid, &snapshot_fs.fsid, &root, &snapshot)?;
    let receipt = RootSnapshotReceipt {
        schema: 1,
        link: link.clone(),
        subvolume_id: snapshot.treeid,
        subvolume_uuid: format_uuid(&snapshot.uuid),
        origin_root_uuid: format_uuid(&root.uuid),
    };
    Ok(NativeRootCheckpoint {
        descriptor,
        link: VerifiedSnapshotLink::from_native_receipt(receipt.clone()),
        subvolume_id: snapshot.treeid,
        subvolume_uuid: format_uuid(&snapshot.uuid),
        receipt,
    })
}

fn open_snapshot_descriptor(
    trusted_root: &File,
    link: &SnapshotLink,
) -> Result<OwnedFd, NativeCheckpointError> {
    state_mount::validate_trusted_root(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let number = validate_link(link)?;
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
    Ok(descriptor)
}

fn validate_link(link: &SnapshotLink) -> Result<u64, NativeCheckpointError> {
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
    Ok(number)
}

/// Match the fixed Snapper path against a receipt from original capture even
/// after root identity changes. The caller must authenticate and verify the
/// receipt's association with the selected immutable generation independently.
/// No source is accepted from snapshot number, timestamp or current observation
/// alone. This function neither rolls back root nor declares boot compatibility.
pub fn verify_historical_root_checkpoint(
    trusted_root: &File,
    receipt: &RootSnapshotReceipt,
) -> Result<HistoricalRootCheckpoint, NativeCheckpointError> {
    receipt.validate()?;
    let descriptor = open_snapshot_descriptor(trusted_root, &receipt.link)?;
    let root_fs = read_btrfs_fs_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let snapshot_fs =
        read_btrfs_fs_info(&descriptor).map_err(NativeCheckpointError::Observation)?;
    let snapshot =
        read_btrfs_subvolume_info(&descriptor).map_err(NativeCheckpointError::Observation)?;
    validate_historical_identity(receipt, &root_fs.fsid, &snapshot_fs.fsid, &snapshot)?;
    Ok(HistoricalRootCheckpoint { descriptor, receipt: receipt.clone() })
}

/// Recheck immediately before rollback that the writable root is still the
/// exact subvolume from which the retained current-state checkpoint was made.
/// This prevents using a valid old checkpoint after an intervening root
/// switch or replacement.
pub fn verify_checkpoint_origin_is_running_root(
    trusted_root: &File,
    receipt: &RootSnapshotReceipt,
) -> Result<(), NativeCheckpointError> {
    let historical = verify_historical_root_checkpoint(trusted_root, receipt)?;
    let root_fs = read_btrfs_fs_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let checkpoint_fs =
        read_btrfs_fs_info(historical.descriptor()).map_err(NativeCheckpointError::Observation)?;
    let root =
        read_btrfs_subvolume_info(trusted_root).map_err(NativeCheckpointError::Observation)?;
    let checkpoint = read_btrfs_subvolume_info(historical.descriptor())
        .map_err(NativeCheckpointError::Observation)?;
    validate_checkpoint_origin_identity(
        receipt,
        &root_fs.fsid,
        &checkpoint_fs.fsid,
        &root,
        &checkpoint,
    )
}

fn validate_checkpoint_origin_identity(
    receipt: &RootSnapshotReceipt,
    root_fs: &[u8; 16],
    checkpoint_fs: &[u8; 16],
    root: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
    checkpoint: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
) -> Result<(), NativeCheckpointError> {
    let readonly = u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY);
    if root_fs != checkpoint_fs
        || *root_fs == [0; 16]
        || format_uuid(root_fs) != receipt.link.filesystem_uuid
        || format_uuid(&root.uuid) != receipt.origin_root_uuid
        || root.treeid < 256
        || root.treeid == checkpoint.treeid
        || root.flags & readonly != 0
        || checkpoint.flags & readonly == 0
        || checkpoint.parent_uuid != root.uuid
    {
        return Err(NativeCheckpointError::NotCurrentRootSnapshot);
    }
    Ok(())
}

fn validate_historical_identity(
    receipt: &RootSnapshotReceipt,
    root_fs: &[u8; 16],
    snapshot_fs: &[u8; 16],
    snapshot: &linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args,
) -> Result<(), NativeCheckpointError> {
    receipt.validate()?;
    if root_fs != snapshot_fs
        || *root_fs == [0; 16]
        || receipt.link.filesystem_uuid != format_uuid(root_fs)
        || snapshot.treeid != receipt.subvolume_id
        || format_uuid(&snapshot.uuid) != receipt.subvolume_uuid
        || format_uuid(&snapshot.parent_uuid) != receipt.origin_root_uuid
    {
        return Err(NativeCheckpointError::IdentityMismatch);
    }
    if snapshot.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) == 0 {
        return Err(NativeCheckpointError::WritableSnapshot);
    }
    Ok(())
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
    // GET_SUBVOL_INFO returns root-item flags, not the SUBVOL_GETFLAGS /
    // snapshot-creation flag namespace (BTRFS_SUBVOL_RDONLY has value 2).
    // Linux fs/btrfs/ioctl.c assigns btrfs_root_flags(root_item) here.
    if snapshot.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) == 0 {
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
    use crate::{
        Sha256,
        restore::{RestorePlan, RestoreScope, RootRollbackResult},
    };
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
            flags: u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY),
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

    fn receipt() -> RootSnapshotReceipt {
        RootSnapshotReceipt {
            schema: 1,
            link: SnapshotLink::new(
                crate::Identifier::new("root").unwrap(),
                "42",
                format_uuid(&[3; 16]),
            )
            .unwrap(),
            subvolume_id: 300,
            subvolume_uuid: format_uuid(&[2; 16]),
            origin_root_uuid: format_uuid(&[1; 16]),
        }
    }

    fn rollback_journal() -> RestoreJournal {
        let config = crate::Identifier::new("root").unwrap();
        let source_snapshot = ryoku_snapper::SnapshotId::new("42").unwrap();
        let restore_id = crate::Identifier::new("restore-attempt-1").unwrap();
        let receipt = ryoku_snapper::RootRollbackReceipt {
            config: ryoku_snapper::ConfigName::new("root").unwrap(),
            source_snapshot,
            previous_default_snapshot: ryoku_snapper::SnapshotId::new("43").unwrap(),
            backup_snapshot: ryoku_snapper::SnapshotId::new("44").unwrap(),
            rollback_target_snapshot: ryoku_snapper::SnapshotId::new("45").unwrap(),
            transaction_id: ryoku_snapper::TransactionId::new("restore-attempt-1:rollback")
                .unwrap(),
        };
        let plan = RestorePlan {
            source_manifest_sha256: Sha256::new(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
            .unwrap(),
            transaction_id: crate::Identifier::new("txn-source-1").unwrap(),
            generation_id: crate::Identifier::new("gen-source-1").unwrap(),
            snapper_config: config,
            snapshot_id: "42".into(),
            filesystem_uuid: format_uuid(&[3; 16]),
            scope: RestoreScope::CompleteLinked,
            configuration_entries: 1,
        };
        let mut journal = RestoreJournal::new(
            restore_id,
            plan,
            crate::Identifier::new("gen-current-1").unwrap(),
            Sha256::new("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap(),
        );
        journal.phase = RestorePhase::WritersStopped;
        journal.root_rollback = Some(RootRollbackResult::from_snapper(&receipt).unwrap());
        journal
    }

    #[test]
    fn running_rollback_verification_remains_available_through_completion_phases() {
        for (scope, accepted, rejected) in [
            (
                RestoreScope::SystemOnly,
                vec![
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
                vec![
                    RestorePhase::Planned,
                    RestorePhase::CheckpointVerified,
                    RestorePhase::ContentStaged,
                    RestorePhase::ConfigurationRestored,
                ],
            ),
            (
                RestoreScope::CompleteLinked,
                vec![
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::ConfigurationRestored,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
                vec![
                    RestorePhase::Planned,
                    RestorePhase::CheckpointVerified,
                    RestorePhase::ContentStaged,
                ],
            ),
            (
                RestoreScope::ConfigurationOnly,
                vec![],
                vec![
                    RestorePhase::WritersStopped,
                    RestorePhase::RootRollbackComplete,
                    RestorePhase::ConfigurationRestored,
                    RestorePhase::HealthChecked,
                    RestorePhase::Complete,
                ],
            ),
        ] {
            for phase in accepted {
                assert!(rollback_verification_phase_supported(scope, phase), "{scope:?} {phase:?}");
            }
            for phase in rejected {
                assert!(
                    !rollback_verification_phase_supported(scope, phase),
                    "{scope:?} {phase:?}"
                );
            }
        }
    }

    #[test]
    fn booted_root_rollback_requires_exact_writable_target_and_readonly_backup() {
        let journal = rollback_journal();
        let source_receipt = receipt();
        let fs = [3; 16];
        let mut root = info(500, 5, 2);
        root.flags = 0;
        let source = info(300, 2, 1);
        let target = root;
        let backup = info(501, 6, 4);
        assert_eq!(
            validate_running_rollback_identity(
                &journal,
                &source_receipt,
                &fs,
                &fs,
                &fs,
                &fs,
                &root,
                &source,
                &target,
                &backup,
            ),
            Ok(())
        );

        let mut wrong_root = root;
        wrong_root.treeid += 1;
        let mut readonly_target = target;
        readonly_target.flags = u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY);
        let wrong_parent = info(500, 5, 9);
        let mut writable_backup = backup;
        writable_backup.flags = 0;
        assert_eq!(
            validate_running_rollback_identity(
                &journal,
                &source_receipt,
                &fs,
                &fs,
                &fs,
                &fs,
                &wrong_root,
                &source,
                &target,
                &backup,
            ),
            Err(NativeCheckpointError::RollbackTargetNotBooted)
        );
        for (bad_root, bad_target, bad_backup, bad_fs) in [
            (root, readonly_target, backup, fs),
            (root, wrong_parent, backup, fs),
            (root, target, writable_backup, fs),
            (root, target, backup, [4; 16]),
        ] {
            assert_eq!(
                validate_running_rollback_identity(
                    &journal,
                    &source_receipt,
                    &bad_fs,
                    &fs,
                    &fs,
                    &fs,
                    &bad_root,
                    &source,
                    &bad_target,
                    &bad_backup,
                ),
                Err(NativeCheckpointError::RollbackTargetMismatch)
            );
        }
    }

    #[test]
    fn historical_identity_survives_changed_root_but_never_substitution() {
        let receipt = receipt();
        let fs = [3; 16];
        let snapshot = info(300, 2, 1);
        let changed_root = info(400, 4, 1);
        // A historical source cannot prove a fresh snapshot of the new root.
        assert_eq!(
            validate_identity(&format_uuid(&fs), &fs, &fs, &changed_root, &snapshot),
            Err(NativeCheckpointError::NotCurrentRootSnapshot)
        );
        assert_eq!(validate_historical_identity(&receipt, &fs, &fs, &snapshot), Ok(()));
        for substituted in [info(301, 2, 1), info(300, 9, 1), info(300, 2, 4)] {
            assert_eq!(
                validate_historical_identity(&receipt, &fs, &fs, &substituted),
                Err(NativeCheckpointError::IdentityMismatch)
            );
        }
        assert_eq!(
            validate_historical_identity(&receipt, &[4; 16], &fs, &snapshot),
            Err(NativeCheckpointError::IdentityMismatch)
        );
        assert_eq!(
            validate_historical_identity(&receipt, &[4; 16], &[4; 16], &snapshot),
            Err(NativeCheckpointError::IdentityMismatch)
        );
        let mut writable = snapshot;
        writable.flags = u64::from(linux_raw_sys::btrfs::BTRFS_SUBVOL_RDONLY);
        assert_eq!(
            validate_historical_identity(&receipt, &fs, &fs, &writable),
            Err(NativeCheckpointError::WritableSnapshot)
        );
    }

    #[test]
    fn current_checkpoint_cannot_be_reused_after_the_running_root_changes() {
        let receipt = receipt();
        let fs = [3; 16];
        let mut root = info(500, 1, 0);
        root.flags = 0;
        let checkpoint = info(300, 2, 1);
        assert_eq!(
            validate_checkpoint_origin_identity(&receipt, &fs, &fs, &root, &checkpoint),
            Ok(())
        );

        let mut substituted_root = root;
        substituted_root.uuid = [9; 16];
        let mut readonly_root = root;
        readonly_root.flags = u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY);
        let wrong_parent = info(300, 2, 4);
        for (candidate_root, candidate_checkpoint, candidate_fs) in [
            (substituted_root, checkpoint, fs),
            (readonly_root, checkpoint, fs),
            (root, wrong_parent, fs),
            (root, checkpoint, [4; 16]),
        ] {
            assert_eq!(
                validate_checkpoint_origin_identity(
                    &receipt,
                    &candidate_fs,
                    &fs,
                    &candidate_root,
                    &candidate_checkpoint,
                ),
                Err(NativeCheckpointError::NotCurrentRootSnapshot)
            );
        }
    }

    #[test]
    fn receipt_schema_and_fixed_root_link_are_validated_before_use() {
        let receipt = receipt();
        let bytes = serde_json::to_vec(&receipt).unwrap();
        assert_eq!(serde_json::from_slice::<RootSnapshotReceipt>(&bytes).unwrap(), receipt);
        for variant in 0..9 {
            let mut invalid = receipt.clone();
            match variant {
                0 => invalid.schema = 2,
                1 => invalid.subvolume_id = 5,
                2 => invalid.subvolume_uuid = format_uuid(&[0; 16]),
                3 => invalid.origin_root_uuid = invalid.subvolume_uuid.clone(),
                4 => invalid.link.snapper_config = crate::Identifier::new("home").unwrap(),
                5 => invalid.link.snapper_snapshot_id = "042".into(),
                6 => invalid.link.snapper_snapshot_id = "0".into(),
                7 => invalid.link.filesystem_uuid = format_uuid(&[0; 16]),
                8 => invalid.subvolume_uuid = "not-a-uuid".into(),
                _ => unreachable!(),
            }
            assert!(invalid.validate().is_err(), "variant {variant}");
        }
        let mut extra = serde_json::to_value(&receipt).unwrap();
        extra["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RootSnapshotReceipt>(extra).is_err());
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
        // The similarly named ioctl-input flag cannot establish read-only
        // status in a root-item flag field.
        writable.flags = u64::from(linux_raw_sys::btrfs::BTRFS_SUBVOL_RDONLY);
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
