//! Idempotent configuration application after writer shutdown and, for linked
//! restores, root rollback.
//! The privileged coordinator must authenticate the operator, validate root
//! identities and the retained checkpoint, and keep writers stopped throughout.
//! Journal phases are caller observations, not proof of those native actions.
use crate::{
    EntryKind, Identifier, RelativePath, StateRoot,
    capture::{CaptureError, CaptureLimits, capture_declared},
    configuration_stage::StagedConfiguration,
    disk_stage::{DiskStageError, DiskStageStore},
    inode_metadata::{InodeMetadataError, prepare_inode_metadata, prepare_symlink_metadata},
    restore::{JournalError, RestoreJournal, RestoreJournalStore, RestorePhase, RestoreScope},
};
use rustix::{
    fs::{
        AtFlags, FileType, Mode, OFlags, ResolveFlags, fsync, linkat, mkdirat, openat, openat2,
        renameat, statat, symlinkat, syncfs, unlinkat,
    },
    io::Errno,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    ops::ControlFlow,
    os::unix::fs::MetadataExt,
};

pub struct ConfigurationDestination<'a> {
    pub root: StateRoot,
    pub directory: &'a File,
}

#[derive(Debug)]
pub enum ConfigurationApplyError {
    WrongPhaseOrPlan,
    InvalidInventory,
    MissingRoot,
    PrivateStageCollision,
    VerificationFailed,
    Cancelled,
    Filesystem(Errno),
    Io(std::io::Error),
    Stage(DiskStageError),
    Metadata(InodeMetadataError),
    Capture(CaptureError),
    Journal(JournalError),
    Encoding(serde_json::Error),
}
impl std::fmt::Display for ConfigurationApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "configuration application failed: {self:?}")
    }
}
impl std::error::Error for ConfigurationApplyError {}
macro_rules! convert {
    ($t:ty, $v:ident) => {
        impl From<$t> for ConfigurationApplyError {
            fn from(e: $t) -> Self {
                Self::$v(e)
            }
        }
    };
}
convert!(Errno, Filesystem);
convert!(std::io::Error, Io);
convert!(DiskStageError, Stage);
convert!(InodeMetadataError, Metadata);
convert!(CaptureError, Capture);
convert!(JournalError, Journal);
convert!(serde_json::Error, Encoding);
type Result<T> = std::result::Result<T, ConfigurationApplyError>;

struct LocalParent {
    parent: File,
    stage: File,
    name: String,
}

/// Apply every configuration entry, using private destination-local objects
/// rather than renaming from the persistent stage across mounts/subvolumes.
/// Replays replace complete file inodes, preserving unrelated hard-link peers.
/// Nonempty directories are never recursively deleted. Interrupted attempts
/// retain `RootRollbackComplete`; replay verifies the source and repeats the
/// writes. Only full read-back verification advances `ConfigurationRestored`.
/// A cancellation callback runs after each durable operation, outside a rename
/// gap. This does not perform health checks or enable normal session startup.
pub fn apply_configuration(
    source: &StagedConfiguration,
    stages: &DiskStageStore,
    journals: &RestoreJournalStore,
    restore_id: &Identifier,
    destinations: &[ConfigurationDestination<'_>],
    progress: impl FnMut(usize) -> ControlFlow<()>,
) -> Result<RestoreJournal> {
    apply_with_owner(source, stages, journals, restore_id, destinations, progress, 0)
}

pub(crate) fn apply_with_owner(
    source: &StagedConfiguration,
    stages: &DiskStageStore,
    journals: &RestoreJournalStore,
    restore_id: &Identifier,
    destinations: &[ConfigurationDestination<'_>],
    progress: impl FnMut(usize) -> ControlFlow<()>,
    owner: u32,
) -> Result<RestoreJournal> {
    apply_subset(
        source,
        stages,
        journals,
        restore_id,
        destinations,
        progress,
        ApplyControl { owner, advance: true },
    )
}

pub(crate) struct ApplyControl {
    pub owner: u32,
    pub advance: bool,
}

/// The asset-aware coordinator delays ConfigurationRestored until its streamed
/// payloads and the preference subset both verify. Legacy callers keep the
/// original final transition through apply_with_owner above.
pub(crate) fn apply_subset(
    source: &StagedConfiguration,
    stages: &DiskStageStore,
    journals: &RestoreJournalStore,
    restore_id: &Identifier,
    destinations: &[ConfigurationDestination<'_>],
    mut progress: impl FnMut(usize) -> ControlFlow<()>,
    control: ApplyControl,
) -> Result<RestoreJournal> {
    let owner = control.owner;
    let mut journal = journals.load(restore_id)?;
    let phase_ready = match journal.plan.scope {
        RestoreScope::ConfigurationOnly => matches!(
            journal.phase,
            RestorePhase::WritersStopped
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
        ),
        RestoreScope::CompleteLinked => matches!(
            journal.phase,
            RestorePhase::RootRollbackComplete
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
        ),
        RestoreScope::SystemOnly => false,
    };
    if journal.plan != *source.plan() || !phase_ready {
        return Err(ConfigurationApplyError::WrongPhaseOrPlan);
    }
    let _verified_stage = stages.open_verified(source)?;
    let mut roots = BTreeMap::new();
    let mut root_inodes = BTreeSet::new();
    for destination in destinations {
        let metadata = destination.directory.metadata()?;
        if destination.root == StateRoot::StateGeneration
            || !metadata.is_dir()
            || !root_inodes.insert((metadata.dev(), metadata.ino()))
            || roots.insert(destination.root, destination.directory).is_some()
        {
            return Err(ConfigurationApplyError::InvalidInventory);
        }
    }
    let records: BTreeMap<_, _> = source
        .entries()
        .iter()
        .map(|e| ((e.record().root, e.record().path.as_str()), &e.record().kind))
        .collect();
    for entry in source.entries() {
        if !roots.contains_key(&entry.record().root) {
            return Err(ConfigurationApplyError::MissingRoot);
        }
        let components: Vec<_> = entry.record().path.as_str().split('/').collect();
        if components.iter().any(|p| p.len() > 255 || p.starts_with(".ryoku-restore-")) {
            return Err(ConfigurationApplyError::InvalidInventory);
        }
        for end in 1..components.len() {
            let parent = components[..end].join("/");
            if records.get(&(entry.record().root, parent.as_str())).is_some_and(|kind| {
                !matches!(kind, EntryKind::Directory { .. })
                    && !(matches!(kind, EntryKind::Absent)
                        && matches!(entry.record().kind, EntryKind::Absent))
            }) {
                return Err(ConfigurationApplyError::InvalidInventory);
            }
        }
        let mut needs_creation = false;
        for end in 1..components.len() {
            let parent = components[..end].join("/");
            let declared = records.get(&(entry.record().root, parent.as_str()));
            if matches!(entry.record().kind, EntryKind::Absent)
                && declared.is_some_and(|kind| matches!(kind, EntryKind::Absent))
            {
                break; // Descendant absence can be checked after ancestor removal.
            }
            if needs_creation {
                if !declared.is_some_and(|kind| matches!(kind, EntryKind::Directory { .. })) {
                    return Err(ConfigurationApplyError::InvalidInventory);
                }
                continue;
            }
            match openat2(
                roots[&entry.record().root],
                parent.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            ) {
                Ok(_) => {}
                Err(Errno::NOENT | Errno::NOTDIR | Errno::LOOP)
                    if declared.is_some_and(|kind| matches!(kind, EntryKind::Directory { .. })) =>
                {
                    needs_creation = true
                }
                Err(Errno::NOENT) if matches!(entry.record().kind, EntryKind::Absent) => break,
                Err(e) => return Err(e.into()),
            }
        }
    }
    if journal.phase == RestorePhase::ConfigurationRestored {
        verify_live(source, &roots)?;
        return Ok(journal);
    }
    let mut steps = 0;
    let mut checkpoint = || -> Result<()> {
        steps += 1;
        if progress(steps).is_break() { Err(ConfigurationApplyError::Cancelled) } else { Ok(()) }
    };
    let mut directories: Vec<_> = source
        .entries()
        .iter()
        .filter(|e| matches!(e.record().kind, EntryKind::Directory { .. }))
        .collect();
    directories.sort_by_key(|e| e.record().path.as_str().split('/').count());
    // Materialize enrolled parents before children; defer their final metadata
    // until all child writes and temporary-stage cleanup have completed.
    for entry in &directories {
        let (parent, leaf) = open_parent(roots[&entry.record().root], &entry.record().path)?;
        match statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {}
            Ok(_) => {
                unlinkat(&parent, leaf, AtFlags::empty())?;
                mkdirat(&parent, leaf, Mode::from_raw_mode(0o700))?;
            }
            Err(Errno::NOENT) => mkdirat(&parent, leaf, Mode::from_raw_mode(0o700))?,
            Err(e) => return Err(e.into()),
        }
        let child = File::from(openat(
            &parent,
            leaf,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        fsync(&child)?;
        fsync(&parent)?;
        checkpoint()?;
    }
    let mut parents: BTreeMap<(StateRoot, String), LocalParent> = BTreeMap::new();
    let mut links: BTreeMap<String, (File, String)> = BTreeMap::new();
    for (index, entry) in source.entries().iter().enumerate() {
        if matches!(entry.record().kind, EntryKind::Directory { .. } | EntryKind::Absent) {
            continue;
        }
        let (parent_path, leaf) = split_path(&entry.record().path);
        let key = (entry.record().root, parent_path.to_owned());
        if !parents.contains_key(&key) {
            let (parent, _) = open_parent(roots[&entry.record().root], &entry.record().path)?;
            let local = local_parent(parent, restore_id, source, &key, owner)?;
            parents.insert(key.clone(), local);
        }
        let local = &parents[&key];
        let slot = format!("entry-{index:03}");
        remove_nondirectory(&local.stage, &slot)?;
        match &entry.record().kind {
            EntryKind::RegularFile { hard_link_group, .. } => {
                if let Some((first_parent, first_leaf)) =
                    hard_link_group.as_ref().and_then(|g| links.get(g.as_str()))
                {
                    linkat(
                        first_parent,
                        first_leaf.as_str(),
                        &local.stage,
                        slot.as_str(),
                        AtFlags::empty(),
                    )?;
                } else {
                    let mut file = File::from(openat(
                        &local.stage,
                        slot.as_str(),
                        OFlags::CREATE
                            | OFlags::EXCL
                            | OFlags::RDWR
                            | OFlags::NOFOLLOW
                            | OFlags::CLOEXEC,
                        Mode::from_raw_mode(0o600),
                    )?);
                    file.write_all(
                        entry.content().ok_or(ConfigurationApplyError::InvalidInventory)?,
                    )?;
                    prepare_inode_metadata(&file, entry)?;
                }
            }
            EntryKind::Symlink { target, .. } => {
                symlinkat(target.as_str(), &local.stage, slot.as_str())?;
                prepare_symlink_metadata(
                    &local.stage,
                    &RelativePath::new(slot.clone()).expect("fixed slot"),
                    entry,
                )?;
            }
            _ => unreachable!(),
        }
        fsync(&local.stage)?;
        syncfs(&local.stage)?;
        // rename replaces file/symlink inodes atomically. Replacing an empty
        // directory needs a durable, replayable gap; never remove its children.
        if let Ok(stat) = statat(&local.parent, leaf, AtFlags::SYMLINK_NOFOLLOW)
            && FileType::from_raw_mode(stat.st_mode) == FileType::Directory
        {
            unlinkat(&local.parent, leaf, AtFlags::REMOVEDIR)?;
            fsync(&local.parent)?;
        }
        renameat(&local.stage, slot.as_str(), &local.parent, leaf)?;
        fsync(&local.parent)?;
        fsync(&local.stage)?;
        if let EntryKind::RegularFile { hard_link_group: Some(group), .. } = &entry.record().kind {
            links
                .entry(group.as_str().to_owned())
                .or_insert((File::from(rustix::io::dup(&local.parent)?), leaf.to_owned()));
        }
        checkpoint()?;
    }
    let mut absent: Vec<_> =
        source.entries().iter().filter(|e| matches!(e.record().kind, EntryKind::Absent)).collect();
    absent.sort_by_key(|e| std::cmp::Reverse(e.record().path.as_str().split('/').count()));
    for entry in absent {
        match open_parent(roots[&entry.record().root], &entry.record().path) {
            Ok((parent, leaf)) => {
                match statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) => unlinkat(
                        &parent,
                        leaf,
                        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                            AtFlags::REMOVEDIR
                        } else {
                            AtFlags::empty()
                        },
                    )?,
                    Err(Errno::NOENT) => {}
                    Err(e) => return Err(e.into()),
                }
                fsync(&parent)?;
            }
            Err(ConfigurationApplyError::Filesystem(Errno::NOENT)) => {}
            Err(ConfigurationApplyError::Filesystem(Errno::NOTDIR | Errno::LOOP))
                if entry.record().path.as_str().split('/').enumerate().skip(1).any(
                    |(end, _)| {
                        let parent = entry
                            .record()
                            .path
                            .as_str()
                            .split('/')
                            .take(end)
                            .collect::<Vec<_>>()
                            .join("/");
                        records
                            .get(&(entry.record().root, parent.as_str()))
                            .is_some_and(|kind| matches!(kind, EntryKind::Absent))
                    },
                ) => {}
            Err(e) => return Err(e),
        }
        checkpoint()?;
    }
    for local in parents.values() {
        unlinkat(&local.stage, "owner.json", AtFlags::empty())?;
        fsync(&local.stage)?;
        unlinkat(&local.parent, local.name.as_str(), AtFlags::REMOVEDIR)?;
        fsync(&local.parent)?;
    }
    for entry in directories.into_iter().rev() {
        let file = File::from(openat2(
            roots[&entry.record().root],
            entry.record().path.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?);
        prepare_inode_metadata(&file, entry)?;
        checkpoint()?;
    }
    verify_live(source, &roots)?;
    if control.advance
        && matches!(
            journal.phase,
            RestorePhase::WritersStopped | RestorePhase::RootRollbackComplete
        )
    {
        journals.advance(&mut journal, RestorePhase::ConfigurationRestored)?;
    }
    Ok(journal)
}

fn split_path(path: &RelativePath) -> (&str, &str) {
    path.as_str().rsplit_once('/').unwrap_or(("", path.as_str()))
}
fn open_parent<'a>(root: &File, path: &'a RelativePath) -> Result<(File, &'a str)> {
    let (parent, leaf) = split_path(path);
    let fd = if parent.is_empty() {
        rustix::io::dup(root)?
    } else {
        openat2(
            root,
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?
    };
    Ok((File::from(fd), leaf))
}
fn remove_nondirectory(parent: &File, name: &str) -> Result<()> {
    match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat)
            if matches!(
                FileType::from_raw_mode(stat.st_mode),
                FileType::RegularFile | FileType::Symlink
            ) =>
        {
            unlinkat(parent, name, AtFlags::empty())?;
            Ok(())
        }
        Err(Errno::NOENT) => Ok(()),
        Ok(_) => Err(ConfigurationApplyError::PrivateStageCollision),
        Err(e) => Err(e.into()),
    }
}
fn local_parent(
    parent: File,
    attempt: &Identifier,
    source: &StagedConfiguration,
    key: &(StateRoot, String),
    owner: u32,
) -> Result<LocalParent> {
    let name = format!(".ryoku-restore-{}", attempt.as_str());
    let expected = serde_json::to_vec(&(1_u16, attempt, source.plan(), key))?;
    let created = match mkdirat(&parent, name.as_str(), Mode::from_raw_mode(0o700)) {
        Ok(()) => true,
        Err(Errno::EXIST) => false,
        Err(e) => return Err(e.into()),
    };
    let stage = File::from(openat(
        &parent,
        name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let stat = stage.metadata()?;
    if stat.uid() != owner || stat.mode() & 0o7777 != 0o700 {
        return Err(ConfigurationApplyError::PrivateStageCollision);
    }
    if created {
        let mut receipt = File::from(openat(
            &stage,
            "owner.json",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        receipt.write_all(&expected)?;
        fsync(&receipt)?;
        fsync(&stage)?;
        fsync(&parent)?;
    } else {
        let receipt = File::from(openat(
            &stage,
            "owner.json",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let stat = receipt.metadata()?;
        if !stat.is_file()
            || stat.uid() != owner
            || stat.mode() & 0o7777 != 0o600
            || stat.nlink() != 1
        {
            return Err(ConfigurationApplyError::PrivateStageCollision);
        }
        let mut bytes = Vec::new();
        receipt.take(expected.len() as u64 + 1).read_to_end(&mut bytes)?;
        if bytes != expected {
            return Err(ConfigurationApplyError::PrivateStageCollision);
        }
    }
    Ok(LocalParent { parent, stage, name })
}
fn verify_live(source: &StagedConfiguration, roots: &BTreeMap<StateRoot, &File>) -> Result<()> {
    let mut links = BTreeMap::new();
    let mut reverse = BTreeMap::new();
    for (root, fd) in roots {
        let entries: Vec<_> =
            source.entries().iter().filter(|e| e.record().root == *root).collect();
        if entries.is_empty() {
            continue;
        }
        let paths: Vec<_> = entries.iter().map(|e| e.record().path.clone()).collect();
        let observed = capture_declared(fd, *root, &paths, CaptureLimits::default())?;
        for (entry, mut actual) in entries.into_iter().zip(observed) {
            if let (
                EntryKind::RegularFile { hard_link_group: expected, .. },
                EntryKind::RegularFile { hard_link_group: found, .. },
            ) = (&entry.record().kind, &mut actual.kind)
            {
                match (expected, found.as_ref()) {
                    (Some(a), Some(b)) => {
                        if links.insert(a.clone(), b.clone()).is_some_and(|old| old != *b)
                            || reverse.insert(b.clone(), a.clone()).is_some_and(|old| old != *a)
                        {
                            return Err(ConfigurationApplyError::VerificationFailed);
                        }
                    }
                    (None, None) => {}
                    _ => return Err(ConfigurationApplyError::VerificationFailed),
                }
                *found = expected.clone();
            }
            if entry.record().kind != actual.kind {
                return Err(ConfigurationApplyError::VerificationFailed);
            }
        }
    }
    Ok(())
}
