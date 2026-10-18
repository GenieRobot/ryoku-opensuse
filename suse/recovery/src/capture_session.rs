//! Native capture coordination shared by the helper and recovery tools.
//!
//! A session shares the machine mutation lock and holds a kernel tracing guard
//! for the exact account's writers. The kernel resumes traced tasks if the
//! helper dies. Root providers must participate in the machine lock.

use crate::{
    RecoveryManifest,
    generation::GenerationStore,
    generation_capture::GenerationCaptureError,
    installed_capture::{
        CaptureAccount, InstalledCaptureRequest, InstalledInventory, InstalledRoots,
        capture_installed_configuration_checked,
    },
    installed_profiles::VerifiedProfileBundle,
    state_mount::StateMountIdentity,
};
use rustix::fs::{self, FileType, FlockOperation, Mode, OFlags, ResolveFlags};
use std::fs::File;

const NO_LINKS: ResolveFlags =
    ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);

#[derive(Debug)]
pub enum CaptureSessionError {
    Unsupported,
    UnsafeBoundary,
    WriterUnavailable,
    Io(std::io::Error),
    Filesystem(rustix::io::Errno),
    Generation(crate::generation::GenerationError),
    Capture(GenerationCaptureError),
    Profile(crate::installed_profiles::ProfileStoreError),
}
impl std::fmt::Display for CaptureSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CaptureSessionError {}
impl From<std::io::Error> for CaptureSessionError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rustix::io::Errno> for CaptureSessionError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Filesystem(e)
    }
}
impl From<crate::generation::GenerationError> for CaptureSessionError {
    fn from(e: crate::generation::GenerationError) -> Self {
        Self::Generation(e)
    }
}
pub(crate) type Result<T> = std::result::Result<T, CaptureSessionError>;

/// Own this session from before creating the native snapshot until the
/// coordinator finishes its protected operation. Dropping it thaws the account;
/// `finish` additionally reports a failure to thaw. It is never serialized.
pub struct NativeCaptureSession {
    account: CaptureAccount,
    home: File,
    state: File,
    identity: StateMountIdentity,
    lock: File,
    freezer: crate::writer_protocol::CaptureWriterLease,
    store: GenerationStore,
}
impl NativeCaptureSession {
    /// Retain the installed default RPM and validated local optional inputs as
    /// an immutable object. This publishes candidate bytes; selection and
    /// activation remain separate authenticated coordinator decisions.
    pub fn publish_profile_inputs(
        &self,
        transaction: &crate::Identifier,
        system: Option<&[u8]>,
        companion: Option<&[u8]>,
    ) -> Result<VerifiedProfileBundle> {
        use crate::installed_profiles::{
            InstalledProfileBundle, ProfileBundleStore, ProfileStoreError,
        };
        self.revalidate()?;
        let bundle = InstalledProfileBundle::load_packaged_defaults(system, companion)
            .map_err(CaptureSessionError::Profile)?;
        let store = match ProfileBundleStore::open_state_mount() {
            Ok(store) => store,
            Err(ProfileStoreError::Filesystem(rustix::io::Errno::NOENT)) => {
                ProfileBundleStore::provision_locked(&self.state)
                    .map_err(CaptureSessionError::Profile)?
            }
            Err(error) => return Err(CaptureSessionError::Profile(error)),
        };
        self.revalidate()?;
        let bundle =
            store.publish_candidate(transaction, &bundle).map_err(CaptureSessionError::Profile)?;
        self.revalidate()?;
        Ok(bundle)
    }
    /// `account` and `home` must be freshly bound by the authenticated helper's
    /// account resolver. This storage primitive is not an authorization API.
    pub fn open(account: CaptureAccount, home: File) -> Result<Self> {
        if rustix::process::geteuid().as_raw() != 0 || account.uid == 0 {
            return Err(CaptureSessionError::Unsupported);
        }
        let stat = fs::fstat(&home)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
            || stat.st_uid != account.uid
            || stat.st_gid != account.gid
            || stat.st_mode & 0o7022 != 0
        {
            return Err(CaptureSessionError::UnsafeBoundary);
        }
        let root = File::open("/")?;
        let record = crate::machine_state::read_machine_state_record(&root)
            .map_err(|_| CaptureSessionError::UnsafeBoundary)?;
        let identity = record.identity().clone();
        let state = File::from(
            crate::state_mount::collect_and_open_state_mount(&root, &identity)
                .map_err(|_| CaptureSessionError::UnsafeBoundary)?
                .0,
        );
        let lock = open_lock(&state)?;
        fs::flock(&lock, FlockOperation::LockExclusive)?;
        same_entry(&state, "mutation.lock", &lock)?;
        let freezer = crate::writer_protocol::CaptureWriterLease::for_account(
            account.uid,
            account.gid,
            &home,
        )?;
        let store = GenerationStore::open_initialized_state_mount(&identity)?;
        Ok(Self { account, home, state, identity, lock, freezer, store })
    }

    pub fn revalidate(&self) -> Result<()> {
        let root = File::open("/")?;
        let current = File::from(
            crate::state_mount::collect_and_open_state_mount(&root, &self.identity)
                .map_err(|_| CaptureSessionError::UnsafeBoundary)?
                .0,
        );
        same_inode(&self.state, &current)?;
        same_entry(&self.state, "mutation.lock", &self.lock)?;
        self.freezer.check()
    }

    /// Capture the compiled whole preference inventory with exact local layer
    /// inputs. A missing bundle or failed writer/mount check cannot publish.
    pub fn capture(
        &self,
        request: InstalledCaptureRequest,
        bundle: &VerifiedProfileBundle,
    ) -> Result<RecoveryManifest> {
        self.revalidate()?;
        let inventory =
            InstalledInventory::for_home(&self.home).map_err(CaptureSessionError::Capture)?;
        let manifest = capture_installed_configuration_checked(
            &self.store,
            &inventory,
            InstalledRoots {
                user_configuration: &self.home,
                account_home: Some(&self.home),
                system_configuration: None,
                account: self.account.clone(),
            },
            request,
            crate::capture::capture_inventory,
            Some(bundle),
            || self.revalidate().map_err(|_| GenerationCaptureError::CoordinationLost),
        )
        .map_err(CaptureSessionError::Capture)?;
        self.revalidate()?;
        Ok(manifest)
    }

    pub fn finish(mut self) -> Result<()> {
        self.freezer.release()
    }
}

fn open_lock(state: &File) -> Result<File> {
    let lock = File::from(fs::openat2(
        state,
        "mutation.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
        NO_LINKS.union(ResolveFlags::NO_XDEV),
    )?);
    let stat = fs::fstat(&lock)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != 0o600
    {
        return Err(CaptureSessionError::UnsafeBoundary);
    }
    fs::fsync(state)?;
    Ok(lock)
}
fn same_inode(a: &File, b: &File) -> Result<()> {
    let a = fs::fstat(a)?;
    let b = fs::fstat(b)?;
    if (a.st_dev, a.st_ino) != (b.st_dev, b.st_ino) {
        return Err(CaptureSessionError::UnsafeBoundary);
    }
    Ok(())
}
fn same_entry(parent: &File, path: &str, file: &File) -> Result<()> {
    let current = File::from(fs::openat2(
        parent,
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        NO_LINKS.union(ResolveFlags::NO_XDEV),
    )?);
    same_inode(&current, file)?;
    let stat = fs::fstat(&current)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_gid != 0
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != 0o600
    {
        return Err(CaptureSessionError::UnsafeBoundary);
    }
    Ok(())
}
pub(crate) fn check_layout(bytes: &[u8], home: &str) -> Result<()> {
    let expected = [
        ("HOME", home.to_owned()),
        ("XDG_CONFIG_HOME", format!("{home}/.config")),
        ("XDG_STATE_HOME", format!("{home}/.local/state")),
        ("XDG_CACHE_HOME", format!("{home}/.cache")),
        ("RYOGAMI_WALL_CONFIG", format!("{home}/.config/ryogami-wall")),
        ("RYOKU_STATE_PATH", format!("{home}/.local/state/ryoku")),
    ];
    for field in bytes.split(|byte| *byte == 0) {
        for (name, value) in &expected {
            if let Some(actual) = field.strip_prefix(format!("{name}=").as_bytes())
                && !actual.is_empty()
                && actual != value.as_bytes()
            {
                return Err(CaptureSessionError::Unsupported);
            }
        }
        if field.starts_with(b"RYOKU_GAMEMODE_PROFILE_FILE=")
            && field != b"RYOKU_GAMEMODE_PROFILE_FILE="
        {
            return Err(CaptureSessionError::Unsupported);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alternate_writer_layout_is_refused() {
        assert!(
            check_layout(b"HOME=/home/test\0XDG_CONFIG_HOME=/home/test/.config\0", "/home/test")
                .is_ok()
        );
        for value in [
            b"XDG_CONFIG_HOME=/outside\0".as_slice(),
            b"XDG_STATE_HOME=/outside\0",
            b"HOME=/another\0",
            b"RYOKU_STATE_PATH=/outside\0",
        ] {
            assert!(check_layout(value, "/home/test").is_err());
        }
    }
}
