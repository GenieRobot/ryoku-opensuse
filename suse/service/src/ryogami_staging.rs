//! Descriptor-relative local content staging for #175 provider implementations.
//!
//! This is a storage primitive, not an installed provider or authorization API.
//! Its caller must hold the authenticated mutation lease and verified linked
//! checkpoint covering the staging directory. No socket currently exposes it.
//! The directory must be an already opened, private, account-owned directory;
//! neither a provider response nor a client may select that descriptor.
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{self, AtFlags, FileType, Mode, OFlags, RenameFlags, ResolveFlags},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    os::fd::AsFd,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 4096;
const MAX_TRANSFER: u64 = 8 * 1024 * 1024 * 1024;
const MAX_STORE: u64 = 32 * 1024 * 1024 * 1024;
const MAX_DURATION: Duration = Duration::from_secs(30 * 60);
const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Quota {
    pub transfer_bytes: u64,
    pub directory_bytes: u64,
    pub duration: Duration,
}
impl Quota {
    fn validate(self) -> Result<(), StageError> {
        if self.transfer_bytes == 0
            || self.transfer_bytes > MAX_TRANSFER
            || self.directory_bytes < self.transfer_bytes
            || self.directory_bytes > MAX_STORE
            || self.duration.is_zero()
            || self.duration > MAX_DURATION
        {
            return Err(StageError::Quota);
        }
        Ok(())
    }
}

/// An expected payload identity is resolved by the trusted provider; it is not
/// a client-supplied download URL, arbitrary file path or successful-copy claim.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentIdentity {
    pub sha256: String,
    pub bytes: u64,
}
impl ContentIdentity {
    fn validate(&self) -> Result<(), StageError> {
        if !valid_digest(&self.sha256) || self.bytes == 0 || self.bytes > MAX_TRANSFER {
            return Err(StageError::Identity);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum StageError {
    Directory,
    Entry,
    Identity,
    Quota,
    Cancelled,
    TimedOut,
    Changed,
    Collision,
    UnsafeInput,
    Io(std::io::Error),
}
impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Raw I/O messages can contain provider paths; do not disclose them.
        f.write_str(match self {
            Self::Directory => "private provider staging directory is unavailable",
            Self::Entry => "unsafe provider staging entry",
            Self::Identity => "provider payload identity mismatch",
            Self::Quota => "provider storage quota exceeded",
            Self::Cancelled => "provider transfer cancelled",
            Self::TimedOut => "provider transfer deadline exceeded",
            Self::Changed => "provider staging plan changed",
            Self::Collision => "provider content already exists",
            Self::UnsafeInput => "provider input must be a regular file or nonblocking stream",
            Self::Io(_) => "provider staging I/O failed",
        })
    }
}
impl std::error::Error for StageError {}
impl From<std::io::Error> for StageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<Errno> for StageError {
    fn from(e: Errno) -> Self {
        Self::Io(e.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FilePin {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    modified: (i64, u64),
    changed: (i64, u64),
}
fn pin(file: &File) -> Result<FilePin, StageError> {
    let stat = fs::fstat(file)?;
    Ok(FilePin {
        device: stat.st_dev,
        inode: stat.st_ino,
        size: stat.st_size as u64,
        mode: stat.st_mode,
        uid: stat.st_uid,
        gid: stat.st_gid,
        modified: (stat.st_mtime, stat.st_mtime_nsec),
        changed: (stat.st_ctime, stat.st_ctime_nsec),
    })
}

/// A read-only plan. Private fields prevent a deserialized request from
/// supplying its own destination or passing off a quota as already checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagePlan {
    identity: ContentIdentity,
    quota: Quota,
    root: FilePin,
    inventory: Vec<(String, FilePin)>,
}
impl StagePlan {
    pub fn identity(&self) -> &ContentIdentity {
        &self.identity
    }
}

pub struct StagingDirectory {
    directory: File,
    uid: u32,
    gid: u32,
}
impl StagingDirectory {
    /// Opens no paths and creates no files. A root helper must resolve the
    /// fixed staging root from AccountStateRoot before supplying this handle.
    pub fn from_owned_directory(directory: File, uid: u32, gid: u32) -> Result<Self, StageError> {
        let result = Self { directory, uid, gid };
        result.validate_directory()?;
        Ok(result)
    }
    fn validate_directory(&self) -> Result<FilePin, StageError> {
        let current = pin(&self.directory)?;
        if FileType::from_raw_mode(current.mode) != FileType::Directory
            || current.uid != self.uid
            || current.gid != self.gid
            || current.mode & 0o7777 != 0o700
        {
            return Err(StageError::Directory);
        }
        Ok(current)
    }
    fn open_file(&self, name: &str) -> Result<File, StageError> {
        let file = File::from(fs::openat2(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            NO_LINKS,
        )?);
        self.validate_file(&file)?;
        Ok(file)
    }
    fn validate_file(&self, file: &File) -> Result<FilePin, StageError> {
        let current = pin(file)?;
        if FileType::from_raw_mode(current.mode) != FileType::RegularFile
            || current.uid != self.uid
            || current.gid != self.gid
            || current.mode & 0o7777 != 0o600
            || fs::fstat(file)?.st_nlink != 1
        {
            return Err(StageError::Entry);
        }
        Ok(current)
    }
    fn inventory(&self) -> Result<Vec<(String, FilePin)>, StageError> {
        let mut entries = fs::Dir::read_from(&self.directory)?;
        let mut inventory = Vec::new();
        for entry in &mut entries {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let name = std::str::from_utf8(name).map_err(|_| StageError::Entry)?;
            if inventory.len() >= MAX_ENTRIES || !valid_name(name) {
                return Err(StageError::Entry);
            }
            let file = self.open_file(name)?;
            inventory.push((name.to_owned(), self.validate_file(&file)?));
        }
        inventory.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(inventory)
    }
    pub fn plan(&self, identity: ContentIdentity, quota: Quota) -> Result<StagePlan, StageError> {
        identity.validate()?;
        quota.validate()?;
        let root = self.validate_directory()?;
        let inventory = self.inventory()?;
        if pin(&self.directory)? != root {
            return Err(StageError::Changed);
        }
        if inventory.iter().any(|(name, _)| name == &identity.sha256) {
            return Err(StageError::Collision);
        }
        let used = inventory
            .iter()
            .try_fold(0u64, |sum, (_, pin)| sum.checked_add(pin.size))
            .ok_or(StageError::Quota)?;
        if identity.bytes > quota.transfer_bytes
            || used.checked_add(identity.bytes).is_none_or(|total| total > quota.directory_bytes)
            || inventory.len() >= MAX_ENTRIES
        {
            return Err(StageError::Quota);
        }
        Ok(StagePlan { identity, quota, root, inventory })
    }

    /// Streams into a private exclusive temporary file, verifies identity and
    /// atomically publishes without replacing existing content. Every failure
    /// after creation attempts cleanup; interrupted process staging remains a
    /// named `.part-*` entry which counts against the next plan's quota.
    ///
    /// Nonregular input must already be nonblocking. This method never changes
    /// flags on a shared descriptor and polls in 20 ms cancellation slices.
    pub fn transfer(
        &self,
        reviewed: &StagePlan,
        input: &impl AsFd,
        cancelled: &AtomicBool,
    ) -> Result<ContentIdentity, StageError> {
        check_abort(cancelled, Instant::now() + reviewed.quota.duration)?;
        if &self.plan(reviewed.identity.clone(), reviewed.quota)? != reviewed {
            return Err(StageError::Changed);
        }
        let stat = fs::fstat(input)?;
        let regular = FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile;
        if !regular
            && (!matches!(FileType::from_raw_mode(stat.st_mode), FileType::Socket | FileType::Fifo)
                || !fs::fcntl_getfl(input)?.contains(OFlags::NONBLOCK))
        {
            return Err(StageError::UnsafeInput);
        }
        let part = format!(".part-{}", reviewed.identity.sha256);
        let mut output = File::from(fs::openat2(
            &self.directory,
            part.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
            NO_LINKS,
        )?);
        fs::fchmod(&output, Mode::from_raw_mode(0o600))?;
        // Newly created files belong to the account even when a root helper
        // performs the copy. Never chown an existing peer-controlled file.
        if rustix::process::geteuid().as_raw() == 0 {
            fs::fchown(
                &output,
                Some(rustix::process::Uid::from_raw(self.uid)),
                Some(rustix::process::Gid::from_raw(self.gid)),
            )?;
        }
        let created = self.validate_file(&output)?;
        let result = (|| {
            let deadline = Instant::now() + reviewed.quota.duration;
            let mut digest = Sha256::new();
            let mut written = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                check_abort(cancelled, deadline)?;
                if !regular {
                    let wait = deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(20));
                    let timeout = Timespec::try_from(wait).map_err(|_| StageError::TimedOut)?;
                    let mut descriptors = [PollFd::new(input, PollFlags::IN)];
                    match poll(&mut descriptors, Some(&timeout)) {
                        Ok(0) => continue,
                        Ok(_) => {}
                        Err(Errno::INTR) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                let count = match rustix::io::read(input, &mut buffer) {
                    Ok(count) => count,
                    Err(Errno::INTR | Errno::AGAIN) => continue,
                    Err(error) => return Err(error.into()),
                };
                check_abort(cancelled, deadline)?;
                if count == 0 {
                    break;
                }
                written = written.checked_add(count as u64).ok_or(StageError::Quota)?;
                if written > reviewed.identity.bytes || written > reviewed.quota.transfer_bytes {
                    return Err(StageError::Quota);
                }
                output.write_all(&buffer[..count])?;
                digest.update(&buffer[..count]);
            }
            if written != reviewed.identity.bytes
                || format!("{:x}", digest.finalize()) != reviewed.identity.sha256
            {
                return Err(StageError::Identity);
            }
            output.sync_all()?;
            check_abort(cancelled, deadline)?;
            // Recheck all earlier entries and total directory bytes. The only
            // addition allowed since review is our exact staging inode.
            let current = self.inventory()?;
            let mut prior = current.clone();
            let position =
                prior.iter().position(|(name, _)| name == &part).ok_or(StageError::Changed)?;
            let staged = prior.remove(position).1;
            if prior != reviewed.inventory
                || staged.inode != created.inode
                || staged.device != created.device
                || staged.size != written
                || self.validate_directory()?.inode != reviewed.root.inode
            {
                return Err(StageError::Changed);
            }
            fs::renameat_with(
                &self.directory,
                part.as_str(),
                &self.directory,
                reviewed.identity.sha256.as_str(),
                RenameFlags::NOREPLACE,
            )?;
            fs::fsync(&self.directory)?;
            self.verify(&reviewed.identity)?;
            Ok(reviewed.identity.clone())
        })();
        if result.is_err() {
            // Best-effort cleanup under the caller's writer lease. A detected
            // replacement is retained for reconciliation, never followed.
            if let Ok(file) = self.open_file(&part)
                && let Ok(current) = pin(&file)
                && (current.device, current.inode) == (created.device, created.inode)
            {
                fs::unlinkat(&self.directory, part.as_str(), AtFlags::empty())?;
                fs::fsync(&self.directory)?;
            }
        }
        result
    }

    /// Independently read back a final blob. Identity is checked with bounded
    /// memory; credentials, paths and provider responses never appear here.
    pub fn verify(&self, identity: &ContentIdentity) -> Result<(), StageError> {
        identity.validate()?;
        self.validate_directory()?;
        self.verify_named(&identity.sha256, identity)
    }

    fn verify_named(&self, name: &str, identity: &ContentIdentity) -> Result<(), StageError> {
        let mut file = self.open_file(name)?;
        let before = pin(&file)?;
        if before.size != identity.bytes {
            return Err(StageError::Identity);
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut remaining = identity.bytes;
        while remaining > 0 {
            let count = file.read(&mut buffer[..remaining.min(64 * 1024) as usize])?;
            if count == 0 {
                return Err(StageError::Identity);
            }
            hasher.update(&buffer[..count]);
            remaining -= count as u64;
        }
        if file.read(&mut [0])? != 0
            || pin(&file)? != before
            || format!("{:x}", hasher.finalize()) != identity.sha256
        {
            return Err(StageError::Identity);
        }
        Ok(())
    }
}
fn check_abort(cancelled: &AtomicBool, deadline: Instant) -> Result<(), StageError> {
    if cancelled.load(Ordering::Acquire) {
        Err(StageError::Cancelled)
    } else if Instant::now() >= deadline {
        Err(StageError::TimedOut)
    } else {
        Ok(())
    }
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_name(name: &str) -> bool {
    valid_digest(name)
        || [".part-", ".trash-", ".receipt-"]
            .iter()
            .any(|prefix| name.strip_prefix(prefix).is_some_and(valid_digest))
}

#[cfg(test)]
#[path = "ryogami_staging_tests.rs"]
mod tests;

#[path = "ryogami_staging_trash.rs"]
mod trash;
pub use trash::{RestorePlan, TrashObservation, TrashPlan, TrashReceiptIdentity};
