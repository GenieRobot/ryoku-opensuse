//! Bind fresh root capture to effective Snapper configuration and native paths.
//! A config name or snapshot number alone does not establish this association.
use crate::{
    SnapshotLink,
    native_checkpoint::{NativeRootCheckpoint, verify_new_root_checkpoint},
    state_mount::{
        format_uuid, read_btrfs_fs_info, read_btrfs_subvolume_info, validate_trusted_root,
    },
};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, ResolveFlags, StatxFlags};
use ryoku_snapper::{RootConfigValues, SnapperCliBackend};
use std::{collections::BTreeSet, fs::File, io::Read};
const CONFIG_LIMIT: u64 = 128 * 1024;
const ENTRY_LIMIT: usize = 4096;
const RESOLVE: ResolveFlags =
    ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootMappingError {
    Unavailable,
    UnsafeConfiguration,
    WrongMapping,
    Changed,
    InventoryLimit,
    ExistingSnapshot,
}
impl std::fmt::Display for RootMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "root capture mapping unavailable: {self:?}")
    }
}
impl std::error::Error for RootMappingError {}
type Result<T> = std::result::Result<T, RootMappingError>;
fn unavailable<T>(_: T) -> RootMappingError {
    RootMappingError::Unavailable
}

/// Runtime binding for a single capture. It carries no authorization or writer
/// exclusion and cannot be reconstructed from caller JSON. Historical receipt
/// verification remains separate; do not reinterpret this as boot coverage.
pub struct RootCaptureMapping {
    root: File,
    files: MappingFiles,
    effective: RootConfigValues,
    native: NativeIdentity,
    before: BTreeSet<u64>,
}
impl RootCaptureMapping {
    /// Open only the actual process root and fixed native configuration paths.
    /// The expected filesystem identity comes from validated machine state.
    pub fn open(expected_filesystem_uuid: &str) -> Result<Self> {
        let root = File::open("/").map_err(unavailable)?;
        validate_trusted_root(&root).map_err(unavailable)?;
        Self::open_with(
            root,
            0,
            0,
            expected_filesystem_uuid,
            || SnapperCliBackend.root_configuration().map_err(unavailable),
            observe_native,
        )
    }
    fn open_with(
        root: File,
        uid: u32,
        gid: u32,
        expected: &str,
        query: impl Fn() -> Result<RootConfigValues>,
        observe: impl Fn(&File, &File) -> Result<NativeIdentity>,
    ) -> Result<Self> {
        let files = MappingFiles::read(&root, uid, gid)?;
        let effective = query()?;
        let native = observe(&root, &files.snapshots)?;
        if format_uuid(&native.filesystem) != expected {
            return Err(RootMappingError::WrongMapping);
        }
        let before = numeric_entries(&files.snapshots, uid, gid)?;
        let mapping = Self { root, files, effective, native, before };
        mapping.revalidate_with(uid, gid, query, observe)?;
        Ok(mapping)
    }
    pub fn revalidate(&self) -> Result<()> {
        validate_trusted_root(&self.root).map_err(unavailable)?;
        self.revalidate_with(
            0,
            0,
            || SnapperCliBackend.root_configuration().map_err(unavailable),
            observe_native,
        )
    }
    fn revalidate_with(
        &self,
        uid: u32,
        gid: u32,
        query: impl Fn() -> Result<RootConfigValues>,
        observe: impl Fn(&File, &File) -> Result<NativeIdentity>,
    ) -> Result<()> {
        let current = MappingFiles::read(&self.root, uid, gid)?;
        self.files.same_as(&current)?;
        if query()? != self.effective || observe(&self.root, &current.snapshots)? != self.native {
            return Err(RootMappingError::Changed);
        }
        // Re-open after the effective query too: a changed file/namespace must
        // not be hidden by a pinned descriptor or daemon-cached configuration.
        self.files.same_as(&MappingFiles::read(&self.root, uid, gid)?)
    }
    fn require_fresh_number(&self, number: &str) -> Result<()> {
        let value = number.parse::<u64>().map_err(|_| RootMappingError::WrongMapping)?;
        if value == 0 || value.to_string() != number {
            return Err(RootMappingError::WrongMapping);
        }
        if self.before.contains(&value) {
            return Err(RootMappingError::ExistingSnapshot);
        }
        Ok(())
    }
    /// A newly returned number must also be new in the actual native namespace,
    /// not just absent from a possibly misdirected Snapper config's inventory.
    pub fn verify_created(&self, link: SnapshotLink) -> Result<NativeRootCheckpoint> {
        self.revalidate()?;
        self.require_fresh_number(&link.snapper_snapshot_id)?;
        if link.filesystem_uuid != format_uuid(&self.native.filesystem) {
            return Err(RootMappingError::WrongMapping);
        }
        let native = verify_new_root_checkpoint(&self.root, link).map_err(unavailable)?;
        self.revalidate()?;
        Ok(native)
    }
}

#[derive(Clone, Eq, PartialEq)]
struct NativeIdentity {
    filesystem: [u8; 16],
    root_uuid: [u8; 16],
    root_id: u64,
    snapshots_uuid: [u8; 16],
    snapshots_id: u64,
}
fn observe_native(root: &File, snapshots: &File) -> Result<NativeIdentity> {
    let root_fs = read_btrfs_fs_info(root).map_err(unavailable)?;
    let snapshot_fs = read_btrfs_fs_info(snapshots).map_err(unavailable)?;
    let root_info = read_btrfs_subvolume_info(root).map_err(unavailable)?;
    let snapshot_info = read_btrfs_subvolume_info(snapshots).map_err(unavailable)?;
    if root_fs.fsid == [0; 16]
        || root_fs.fsid != snapshot_fs.fsid
        || fs::fstat(root).map_err(unavailable)?.st_ino != 256
        || fs::fstat(snapshots).map_err(unavailable)?.st_ino != 256
        || root_info.uuid == [0; 16]
        || snapshot_info.uuid == [0; 16]
        || root_info.uuid == snapshot_info.uuid
        || root_info.treeid == snapshot_info.treeid
        || root_info.treeid < 256
        || snapshot_info.treeid < 256
        || snapshot_info.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) != 0
    {
        return Err(RootMappingError::WrongMapping);
    }
    Ok(NativeIdentity {
        filesystem: root_fs.fsid,
        root_uuid: root_info.uuid,
        root_id: root_info.treeid,
        snapshots_uuid: snapshot_info.uuid,
        snapshots_id: snapshot_info.treeid,
    })
}

#[derive(Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
    mount: u64,
    mode: u32,
    uid: u32,
    gid: u32,
}
fn identity(file: &File) -> Result<Identity> {
    let stat = fs::fstat(file).map_err(unavailable)?;
    let mount =
        fs::statx(file, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID).map_err(unavailable)?;
    if mount.stx_mask & StatxFlags::MNT_ID.bits() == 0 || mount.stx_mnt_id == 0 {
        return Err(RootMappingError::Unavailable);
    }
    Ok(Identity {
        device: stat.st_dev,
        inode: stat.st_ino,
        mount: mount.stx_mnt_id,
        mode: stat.st_mode,
        uid: stat.st_uid,
        gid: stat.st_gid,
    })
}
#[derive(Eq, PartialEq)]
struct RecordStamp {
    size: i64,
    modified: (i64, u64),
    changed: (i64, u64),
}
fn record_stamp(stat: &fs::Stat) -> RecordStamp {
    RecordStamp {
        size: stat.st_size,
        modified: (stat.st_mtime, stat.st_mtime_nsec),
        changed: (stat.st_ctime, stat.st_ctime_nsec),
    }
}
struct MappingFiles {
    parents: Vec<File>,
    config: File,
    bytes: Vec<u8>,
    stamp: RecordStamp,
    snapshots: File,
}
impl MappingFiles {
    fn read(root: &File, uid: u32, gid: u32) -> Result<Self> {
        let mut parents = Vec::new();
        for path in ["etc", "etc/snapper", "etc/snapper/configs"] {
            parents.push(open_directory(root, path, uid, gid)?);
        }
        let config = File::from(
            fs::openat2(
                parents.last().ok_or(RootMappingError::Unavailable)?,
                "root",
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                RESOLVE,
            )
            .map_err(unavailable)?,
        );
        let before = fs::fstat(&config).map_err(unavailable)?;
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
            || before.st_uid != uid
            || before.st_gid != gid
            || before.st_nlink != 1
            || before.st_mode & 0o7022 != 0
            || before.st_size <= 0
            || before.st_size as u64 > CONFIG_LIMIT
        {
            return Err(RootMappingError::UnsafeConfiguration);
        }
        let mut bytes = Vec::new();
        (&config).take(CONFIG_LIMIT + 1).read_to_end(&mut bytes).map_err(unavailable)?;
        let after = fs::fstat(&config).map_err(unavailable)?;
        if bytes.len() as u64 > CONFIG_LIMIT
            || before.st_size != after.st_size
            || before.st_mtime != after.st_mtime
            || before.st_mtime_nsec != after.st_mtime_nsec
            || before.st_ctime != after.st_ctime
            || before.st_ctime_nsec != after.st_ctime_nsec
            || bytes.len() as i64 != after.st_size
        {
            return Err(RootMappingError::Changed);
        }
        validate_record(&bytes)?;
        let snapshots = open_directory(root, ".snapshots", uid, gid)?;
        Ok(Self { parents, config, bytes, stamp: record_stamp(&after), snapshots })
    }
    fn same_as(&self, current: &Self) -> Result<()> {
        if self.bytes != current.bytes
            || self.stamp != current.stamp
            || self.stamp != record_stamp(&fs::fstat(&self.config).map_err(unavailable)?)
            || identity(&self.config)? != identity(&current.config)?
            || identity(&self.snapshots)? != identity(&current.snapshots)?
        {
            return Err(RootMappingError::Changed);
        }
        for (old, new) in self.parents.iter().zip(&current.parents) {
            if identity(old)? != identity(new)? {
                return Err(RootMappingError::Changed);
            }
        }
        Ok(())
    }
}
fn open_directory(root: &File, path: &str, uid: u32, gid: u32) -> Result<File> {
    let file = File::from(
        fs::openat2(
            root,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(unavailable)?,
    );
    let stat = fs::fstat(&file).map_err(unavailable)?;
    if stat.st_uid != uid || stat.st_gid != gid || stat.st_mode & 0o7022 != 0 {
        return Err(RootMappingError::UnsafeConfiguration);
    }
    Ok(file)
}
fn validate_record(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(|_| RootMappingError::UnsafeConfiguration)?;
    let mut values = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(RootMappingError::WrongMapping);
        };
        if matches!(key.trim(), "SUBVOLUME" | "FSTYPE") {
            values.push((key.trim(), value.trim()));
        }
    }
    if values.len() != 2
        || !values.contains(&("SUBVOLUME", "\"/\""))
        || !values.contains(&("FSTYPE", "\"btrfs\""))
        || text.contains('\0')
    {
        return Err(RootMappingError::WrongMapping);
    }
    Ok(())
}
fn numeric_entries(directory: &File, uid: u32, gid: u32) -> Result<BTreeSet<u64>> {
    let mut names = BTreeSet::new();
    let mut count = 0;
    for entry in fs::Dir::read_from(directory).map_err(unavailable)? {
        let entry = entry.map_err(unavailable)?;
        let name = entry.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        count += 1;
        if count > ENTRY_LIMIT {
            return Err(RootMappingError::InventoryLimit);
        }
        if !name.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let text = std::str::from_utf8(name).map_err(unavailable)?;
        let number = text.parse::<u64>().map_err(unavailable)?;
        if number == 0 || number.to_string() != text || !names.insert(number) {
            return Err(RootMappingError::WrongMapping);
        }
        let stat = fs::statat(directory, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(unavailable)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
            || stat.st_uid != uid
            || stat.st_gid != gid
            || stat.st_mode & 0o7022 != 0
        {
            return Err(RootMappingError::UnsafeConfiguration);
        }
    }
    Ok(names)
}

#[cfg(test)]
#[path = "root_capture_mapping_tests.rs"]
mod tests;
