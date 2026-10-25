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
    /// Read only the actor registered in this account's still-frozen lease.
    pub fn wallpaper_actor(&self) -> Result<crate::wallpaper_protocol::ActorIdentity> {
        self.revalidate()?;
        self.freezer.wallpaper_actor()
    }

    /// Prior manual choice recorded only from a verified typed actor receipt.
    pub fn wallpaper_manual_paused(&self) -> Result<bool> {
        self.revalidate()?;
        self.freezer.wallpaper_manual_paused()
    }

    /// Exact installed native-player identities in the still-frozen actor lease.
    pub fn wallpaper_players(&self) -> Result<Vec<crate::wallpaper_protocol::ActorIdentity>> {
        self.revalidate()?;
        self.freezer.wallpaper_players()
    }

    /// Trusted helper integration only: call after authorization, durable
    /// Applying and complete linked-proof revalidation. The machine lock and
    /// every unrelated writer remain held; return requires a full refreeze.
    pub fn apply_wallpaper_actor(
        &self,
        call: &crate::wallpaper_protocol::ActorCall,
    ) -> Result<Vec<u8>> {
        self.revalidate()?;
        let reply = self.freezer.apply_wallpaper(call)?;
        self.revalidate()?;
        Ok(reply)
    }
    /// Shared protected journal anchor. Access is valid only while this session
    /// owns the machine lock and all registered account writers are stopped.
    pub fn coordinator_directory(&self) -> Result<&File> {
        self.revalidate()?;
        Ok(&self.state)
    }

    pub fn inventory_fingerprint(&self) -> Result<crate::Sha256> {
        self.revalidate()?;
        let inventory =
            InstalledInventory::for_home(&self.home).map_err(CaptureSessionError::Capture)?;
        let fingerprint = inventory
            .fingerprint(&self.home, &self.account)
            .map_err(CaptureSessionError::Capture)?;
        self.revalidate()?;
        Ok(fingerprint)
    }

    pub fn verify_inventory(&self, manifest: &RecoveryManifest) -> Result<()> {
        self.revalidate()?;
        if manifest.account_principal() != Some(&self.account.principal) {
            return Err(CaptureSessionError::UnsafeBoundary);
        }
        let bytes = serde_json::to_vec(manifest.files())
            .map_err(|_| CaptureSessionError::UnsafeBoundary)?;
        use sha2::{Digest, Sha256};
        let expected = format!("{:x}", Sha256::digest(&bytes));
        if self.inventory_fingerprint()?.as_str() != expected {
            return Err(CaptureSessionError::UnsafeBoundary);
        }
        if let Some(assets) = manifest.assets() {
            assets
                .require_account(&self.account)
                .map_err(|_| CaptureSessionError::UnsafeBoundary)?;
            let paths =
                assets.entries().iter().map(|record| record.path.clone()).collect::<Vec<_>>();
            let current = crate::ryogami_assets::capture_assets(
                &self.home,
                &self.account,
                &paths,
                crate::ryogami_assets::MAX_CAPTURE_LIMITS,
            )
            .map_err(|_| CaptureSessionError::UnsafeBoundary)?;
            if current.inventory() != assets {
                return Err(CaptureSessionError::UnsafeBoundary);
            }
            self.revalidate()?;
        }
        Ok(())
    }

    /// Installer schema-1 filename migration may add only the fixed personal
    /// profile. All other enrolled bytes, metadata and absence must match its
    /// verified preimage while the same writer lease remains held.
    pub fn verify_installer_migration_inventory(&self, manifest: &RecoveryManifest) -> Result<()> {
        self.revalidate()?;
        if !crate::installer_state::is_installer(&manifest.reason)
            || manifest.account_principal() != Some(&self.account.principal)
        {
            return Err(CaptureSessionError::UnsafeBoundary);
        }
        let inventory =
            InstalledInventory::for_home(&self.home).map_err(CaptureSessionError::Capture)?;
        let actual = crate::capture::capture_inventory(
            &self.home,
            crate::StateRoot::UserHome,
            inventory
                .paths_for_root(crate::StateRoot::UserHome)
                .ok_or(CaptureSessionError::UnsafeBoundary)?,
            Default::default(),
        )
        .map_err(|error| CaptureSessionError::Capture(error.into()))?;
        let retained = |records: &[crate::FileRecord]| {
            records
                .iter()
                .filter(|record| {
                    record.path.as_str() != crate::installed_inventory::PERSONAL_PROFILE
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        if retained(&actual.records) != retained(manifest.files()) {
            return Err(CaptureSessionError::UnsafeBoundary);
        }
        self.revalidate()
    }

    /// Conservative room for both maximum bounded generations. Later
    /// allocation/fsync failures still fail the checkpoint; this is no reserve.
    pub fn ensure_checkpoint_space(&self) -> Result<()> {
        self.revalidate()?;
        self.store.ensure_available_space(64 * 1024 * 1024, 512)?;
        let stats = fs::fstatvfs(&self.state)?;
        if stats.f_bavail.saturating_mul(stats.f_frsize) < 192 * 1024 * 1024 {
            return Err(CaptureSessionError::Generation(
                crate::generation::GenerationError::InsufficientSpace {
                    required_bytes: 192 * 1024 * 1024,
                    available_bytes: stats.f_bavail.saturating_mul(stats.f_frsize),
                },
            ));
        }
        // Btrfs can refuse allocation before statvfs's nominal free count is
        // exhausted. Probe a maximum pair on an unnamed inode; no path or
        // reservation survives process death. Later writes must still handle
        // ENOSPC because this allocation is released before capture.
        let allocation = fs::openat(
            &self.state,
            ".",
            OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        fs::fallocate(&allocation, fs::FallocateFlags::empty(), 0, 128 * 1024 * 1024)?;
        drop(allocation);
        fs::syncfs(&self.state)?;
        self.revalidate()?;
        Ok(())
    }
    /// Retain the installed default RPM and validated local optional inputs as
    /// an immutable object. This publishes candidate bytes; selection and
    /// activation remain separate authenticated coordinator decisions.
    pub fn publish_profile_inputs(
        &self,
        transaction: &crate::Identifier,
        system: Option<&[u8]>,
        companion: Option<&[u8]>,
    ) -> Result<VerifiedProfileBundle> {
        use crate::installed_profiles::InstalledProfileBundle;
        self.revalidate()?;
        let bundle = InstalledProfileBundle::load_packaged_defaults(system, companion)
            .map_err(CaptureSessionError::Profile)?;
        self.publish_installer_bundle(transaction, &bundle)
    }

    /// Retain an installer boundary bundle, including the schema-2 affirmative
    /// absence of defaults. The trusted installer independently verifies RPM
    /// presence/absence and exact package association before capture.
    pub fn publish_installer_bundle(
        &self,
        transaction: &crate::Identifier,
        bundle: &crate::installed_profiles::InstalledProfileBundle,
    ) -> Result<VerifiedProfileBundle> {
        use crate::installed_profiles::{ProfileBundleStore, ProfileStoreError};
        self.revalidate()?;
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
            store.publish_candidate(transaction, bundle).map_err(CaptureSessionError::Profile)?;
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
        // Publication performs the final lease check while it can still retract
        // its generation. A fallible check here would strand a published result.
        Ok(manifest)
    }

    /// Capture the fixed preferences and a server-enrolled exact asset set in
    /// one linked generation while the same authenticated account lease holds.
    /// Wire requests never provide roots, accounts or a native snapshot receipt.
    pub fn capture_with_assets(
        &self,
        request: InstalledCaptureRequest,
        bundle: &VerifiedProfileBundle,
        declarations: &[crate::ryogami_assets::AssetPath],
    ) -> Result<RecoveryManifest> {
        self.revalidate()?;
        let inventory =
            InstalledInventory::for_home(&self.home).map_err(CaptureSessionError::Capture)?;
        crate::installed_capture::capture_installed_configuration_with_sources_checked(
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
            crate::installed_capture::InstalledCaptureSources {
                bundle: Some(bundle),
                assets: Some(declarations),
            },
            || self.revalidate().map_err(|_| GenerationCaptureError::CoordinationLost),
        )
        .map_err(CaptureSessionError::Capture)
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
