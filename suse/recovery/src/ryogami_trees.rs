//! Bounded complete membership enrollment for explicitly selected provider trees.
//! Only the supplied owned tree is enumerated. This is read-only planning, not
//! authorization, a directory creator, or permission to delete a subtree.
use crate::{
    EntryKind, RelativePath,
    installed_capture::CaptureAccount,
    ryogami_assets::{
        AssetError, AssetInventory, AssetNamespace, AssetPath, AssetScope, MAX_CAPTURE_LIMITS,
        check_home,
    },
};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use std::{collections::BTreeSet, fs::File};
const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

/// Resolve a complete current tree and additional exact future destinations
/// before checkpoint planning. The root is provider-owned typed data, never a
/// request-selected privileged path. Cancellation leaves no filesystem writes.
pub fn inspect_scope(
    home: &File,
    account: &CaptureAccount,
    root: AssetPath,
    additional: Vec<AssetPath>,
    max_file_bytes: u64,
    max_total_bytes: u64,
    mut boundary: impl FnMut() -> bool,
) -> Result<AssetScope, AssetError> {
    let bounds = AssetScope::new(vec![root.clone()], max_file_bytes, max_total_bytes)?;
    validate_roots(std::slice::from_ref(&root), bounds.paths())?;
    let limits = crate::capture::CaptureLimits {
        max_paths: MAX_CAPTURE_LIMITS.max_paths,
        ..bounds.limits()
    };
    let mut paths = enumerate(home, account, &root, limits, &mut boundary)?;
    paths.extend(additional);
    paths.sort();
    paths.dedup();
    AssetScope::new(paths, max_file_bytes, max_total_bytes)?.with_trees(vec![root])
}

pub(crate) fn validate_roots(roots: &[AssetPath], paths: &[AssetPath]) -> Result<(), AssetError> {
    if roots.len() > 64 || roots.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(AssetError::Bounds);
    }
    for (index, root) in roots.iter().enumerate() {
        if !matches!(
            root.namespace(),
            AssetNamespace::SemanticModels
                | AssetNamespace::WorkshopContent
                | AssetNamespace::ConfiguredLibrary
                | AssetNamespace::RemotePreviews
                | AssetNamespace::RemoteThumbnails
        ) || !paths.contains(root)
        {
            return Err(AssetError::Path);
        }
        for other in &roots[index + 1..] {
            if beneath(root, other)? || beneath(other, root)? {
                return Err(AssetError::Path);
            }
        }
    }
    Ok(())
}
fn beneath(root: &AssetPath, path: &AssetPath) -> Result<bool, AssetError> {
    let root = root.home_relative()?;
    let path = path.home_relative()?;
    Ok(path == root
        || path.as_str().strip_prefix(root.as_str()).is_some_and(|tail| tail.starts_with('/')))
}
pub(crate) fn validate_inventory(inventory: &AssetInventory) -> Result<(), AssetError> {
    let paths: Vec<_> = inventory.entries().iter().map(|entry| entry.path.clone()).collect();
    validate_roots(inventory.trees(), &paths)?;
    for root in inventory.trees() {
        let record = inventory
            .entries()
            .iter()
            .find(|entry| entry.path == *root)
            .ok_or(AssetError::Manifest)?;
        if !matches!(record.kind, EntryKind::Directory { .. } | EntryKind::Absent) {
            return Err(AssetError::Manifest);
        }
        for entry in inventory.entries() {
            if entry.path == *root
                || matches!(entry.kind, EntryKind::Absent)
                || !beneath(root, &entry.path)?
            {
                continue;
            }
            // Every present descendant has a declared directory parent. This
            // prevents a file list from being relabelled as complete topology.
            let home_path = entry.path.home_relative()?;
            let parent = home_path.as_str().rsplit_once('/').ok_or(AssetError::Path)?.0;
            if !inventory.entries().iter().any(|candidate| {
                candidate.path.home_relative().is_ok_and(|path| path.as_str() == parent)
                    && matches!(candidate.kind, EntryKind::Directory { .. })
            }) {
                return Err(AssetError::Manifest);
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_declared(
    home: &File,
    account: &CaptureAccount,
    scope: &AssetScope,
) -> Result<(), AssetError> {
    for root in scope.trees() {
        let live = enumerate(home, account, root, scope.limits(), &mut || true)?;
        if live.iter().any(|path| !scope.paths().contains(path)) {
            return Err(AssetError::Changed);
        }
    }
    Ok(())
}
pub(crate) fn verify_inventory(
    home: &File,
    account: &CaptureAccount,
    inventory: &AssetInventory,
) -> Result<(), AssetError> {
    for root in inventory.trees() {
        let live = enumerate(home, account, root, MAX_CAPTURE_LIMITS, &mut || true)?;
        let root_absent = inventory
            .entries()
            .iter()
            .any(|entry| entry.path == *root && matches!(entry.kind, EntryKind::Absent));
        let mut expected = BTreeSet::new();
        for entry in inventory.entries() {
            if beneath(root, &entry.path)?
                && (!matches!(entry.kind, EntryKind::Absent)
                    || (root_absent && entry.path == *root))
            {
                expected.insert(entry.path.clone());
            }
        }
        if live.into_iter().collect::<BTreeSet<_>>() != expected {
            return Err(AssetError::Changed);
        }
    }
    Ok(())
}

#[derive(Eq, PartialEq)]
struct Pin {
    device: u64,
    inode: u64,
    size: i64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    modified: (i64, u64),
    changed: (i64, u64),
}
fn pin(file: &File) -> Result<Pin, AssetError> {
    let stat = fs::fstat(file)?;
    Ok(Pin {
        device: stat.st_dev,
        inode: stat.st_ino,
        size: stat.st_size,
        mode: stat.st_mode,
        uid: stat.st_uid,
        gid: stat.st_gid,
        links: stat.st_nlink,
        modified: (stat.st_mtime, stat.st_mtime_nsec),
        changed: (stat.st_ctime, stat.st_ctime_nsec),
    })
}
fn owned(pin: &Pin, account: &CaptureAccount, device: u64) -> Result<FileType, AssetError> {
    let kind = FileType::from_raw_mode(pin.mode);
    if pin.device != device
        || pin.uid != account.uid
        || pin.gid != account.gid
        || pin.mode & 0o7022 != 0
        || !matches!(kind, FileType::Directory | FileType::RegularFile)
        || (kind == FileType::RegularFile
            && (pin.links != 1 || pin.mode & 0o111 != 0 || pin.size < 0))
    {
        return Err(AssetError::Ownership);
    }
    Ok(kind)
}
fn enumerate(
    home: &File,
    account: &CaptureAccount,
    root: &AssetPath,
    limits: crate::capture::CaptureLimits,
    boundary: &mut impl FnMut() -> bool,
) -> Result<Vec<AssetPath>, AssetError> {
    if !boundary() {
        return Err(AssetError::Interrupted);
    }
    check_home(home, account)?;
    let name = root.home_relative()?;
    let fd = match fs::openat2(
        home,
        name.as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        NO_LINKS,
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(vec![root.clone()]),
        Err(error) => return Err(error.into()),
    };
    let directory = File::from(fd);
    let device = fs::fstat(home)?.st_dev;
    if owned(&pin(&directory)?, account, device)? != FileType::Directory {
        return Err(AssetError::Path);
    }
    let mut walker =
        Walker { account, device, limits, paths: vec![root.clone()], bytes: 0, boundary };
    walker.directory(&directory, root)?;
    check_home(home, account)?;
    walker.paths.sort();
    Ok(walker.paths)
}
struct Walker<'a, F> {
    account: &'a CaptureAccount,
    device: u64,
    limits: crate::capture::CaptureLimits,
    paths: Vec<AssetPath>,
    bytes: u64,
    boundary: &'a mut F,
}
impl<F: FnMut() -> bool> Walker<'_, F> {
    fn directory(&mut self, directory: &File, path: &AssetPath) -> Result<(), AssetError> {
        let before = pin(directory)?;
        let mut entries = fs::Dir::read_from(directory)?;
        for entry in &mut entries {
            if !(self.boundary)() {
                return Err(AssetError::Interrupted);
            }
            let entry = entry?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if self.paths.len() >= self.limits.max_paths {
                return Err(AssetError::Bounds);
            }
            let name = std::str::from_utf8(bytes).map_err(|_| AssetError::Path)?;
            let child = path.with_relative(
                RelativePath::new(format!("{}/{}", path.relative().as_str(), name))
                    .map_err(|_| AssetError::Path)?,
            )?;
            let file = File::from(fs::openat2(
                directory,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                NO_LINKS,
            )?);
            let identity = pin(&file)?;
            let kind = owned(&identity, self.account, self.device)?;
            self.paths.push(child.clone());
            if kind == FileType::Directory {
                self.directory(&file, &child)?;
            } else {
                let size = identity.size as u64;
                self.bytes = self.bytes.checked_add(size).ok_or(AssetError::Bounds)?;
                if size > self.limits.max_file_bytes || self.bytes > self.limits.max_total_bytes {
                    return Err(AssetError::Bounds);
                }
            }
            if pin(&file)? != identity {
                return Err(AssetError::Changed);
            }
        }
        if pin(directory)? != before {
            return Err(AssetError::Changed);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "ryogami_trees_tests.rs"]
mod tests;
