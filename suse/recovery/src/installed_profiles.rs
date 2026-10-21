//! Immutable local profile bundles retained outside root rollback.
//!
//! Publication stores a candidate; it never activates a profile or changes a
//! personal override. Production access resolves the fixed machine record and
//! state mount. There is no arbitrary public directory constructor, remote
//! import, active pointer, newest-bundle selection, overwrite, or repair.

use crate::{Identifier, PackageName, PackageRevision, ProfileRevision, Revision, Sha256};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use ryoku_profiles::{ProfileDocument, ProfileLayers, ResolvedSettings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Digest256};
use std::{
    fs::File,
    io::{Read, Write},
};

const BUNDLE_SCHEMA: u16 = 1;
const MAX_BUNDLE_BYTES: usize = 512 * 1024;
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const REVISION_PREFIX: &str = "bundle-sha256-";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsPackage {
    pub name: PackageName,
    pub nevra: Revision,
    pub upstream_revision: Option<Revision>,
}

/// Original local document bytes and their digest, distinct from the semantic
/// SettingsLayer revision. Whitespace changes affect this digest and bundle ID.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalLayer {
    source_json: String,
    source_sha256: Sha256,
}
impl LocalLayer {
    fn new(bytes: &[u8]) -> Result<Self, ProfileStoreError> {
        ProfileDocument::parse(bytes).map_err(|_| ProfileStoreError::InvalidDocument)?;
        Ok(Self {
            source_json: String::from_utf8(bytes.to_vec())
                .map_err(|_| ProfileStoreError::InvalidDocument)?,
            source_sha256: digest(bytes),
        })
    }
    fn document(&self) -> Result<ProfileDocument, ProfileStoreError> {
        if digest(self.source_json.as_bytes()) != self.source_sha256 {
            return Err(ProfileStoreError::DigestMismatch);
        }
        ProfileDocument::parse(self.source_json.as_bytes())
            .map_err(|_| ProfileStoreError::InvalidDocument)
    }
}

/// A protected local import, not a restore target for packaged defaults.
/// System and companion inputs are optional. Personal selection/overrides
/// remain in the account-owned profile.json and are captured separately.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledProfileBundle {
    schema_version: u16,
    package: DefaultsPackage,
    defaults: LocalLayer,
    system: Option<LocalLayer>,
    companion: Option<LocalLayer>,
}
impl InstalledProfileBundle {
    /// Read the one reviewed package-default document from the native root.
    /// The caller separately obtains exact RPM identity and holds the package
    /// writer lock. Optional overrides are bounded local import bytes only.
    pub fn load_packaged_defaults(
        system: Option<&[u8]>,
        companion: Option<&[u8]>,
    ) -> Result<Self, ProfileStoreError> {
        let defaults = crate::rpm_defaults::VerifiedPackagedDefaults::load()
            .map_err(ProfileStoreError::RpmDefaults)?;
        Self::from_local_documents(defaults.package().clone(), defaults.bytes(), system, companion)
    }
    /// Trusted importer supplies locally read defaults and package facts from
    /// installed RPM inspection. This constructor validates data; it does not
    /// establish RPM ownership, authenticate an importer, or activate anything.
    pub fn from_local_documents(
        package: DefaultsPackage,
        defaults: &[u8],
        system: Option<&[u8]>,
        companion: Option<&[u8]>,
    ) -> Result<Self, ProfileStoreError> {
        let result = Self {
            schema_version: BUNDLE_SCHEMA,
            package,
            defaults: LocalLayer::new(defaults)?,
            system: system.map(LocalLayer::new).transpose()?,
            companion: companion.map(LocalLayer::new).transpose()?,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn package(&self) -> &DefaultsPackage {
        &self.package
    }
    pub fn defaults_source_bytes(&self) -> &[u8] {
        self.defaults.source_json.as_bytes()
    }
    pub fn defaults_source_sha256(&self) -> &Sha256 {
        &self.defaults.source_sha256
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProfileStoreError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ProfileStoreError::InvalidDocument)?;
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(ProfileStoreError::TooLarge);
        }
        Ok(bytes)
    }
    pub fn id(&self) -> Result<Sha256, ProfileStoreError> {
        Ok(digest(&self.canonical_bytes()?))
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, ProfileStoreError> {
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(ProfileStoreError::TooLarge);
        }
        let result: Self =
            serde_json::from_slice(bytes).map_err(|_| ProfileStoreError::InvalidDocument)?;
        result.validate()?;
        if result.canonical_bytes()? != bytes {
            return Err(ProfileStoreError::InvalidDocument);
        }
        Ok(result)
    }
    fn validate(&self) -> Result<(), ProfileStoreError> {
        if self.schema_version != BUNDLE_SCHEMA {
            return Err(ProfileStoreError::UnsupportedSchema);
        }
        let defaults = self.defaults.document()?;
        for layer in [&self.system, &self.companion].into_iter().flatten() {
            if layer.document()?.id != defaults.id {
                return Err(ProfileStoreError::ProfileMismatch);
            }
        }
        Ok(())
    }
    pub fn resolve(
        &self,
        personal: Option<&[u8]>,
    ) -> Result<ResolvedInstalledProfile, ProfileStoreError> {
        self.validate()?;
        let defaults = self.defaults.document()?;
        let system = self.system.as_ref().map(LocalLayer::document).transpose()?;
        let companion = self.companion.as_ref().map(LocalLayer::document).transpose()?;
        let personal = personal
            .map(ProfileDocument::parse)
            .transpose()
            .map_err(|_| ProfileStoreError::InvalidDocument)?;
        if personal.as_ref().is_some_and(|document| document.id != defaults.id) {
            return Err(ProfileStoreError::ProfileMismatch);
        }
        let layers = ProfileLayers {
            defaults: defaults.settings,
            system_overrides: system.as_ref().map(|doc| doc.settings).unwrap_or_default(),
            companion: companion.as_ref().map(|doc| doc.settings),
            personal: personal.as_ref().map(|doc| doc.settings).unwrap_or_default(),
        };
        let revisions = layers.content_revisions();
        let revision = |value| Revision::new(value).map_err(|_| ProfileStoreError::InvalidDocument);
        Ok(ResolvedInstalledProfile {
            settings: layers.resolve(),
            provenance: ProfileRevision {
                profile_id: Identifier::new(defaults.id.as_str())
                    .map_err(|_| ProfileStoreError::InvalidDocument)?,
                profile_revision: revision(format!("{REVISION_PREFIX}{}", self.id()?.as_str()))?,
                defaults_revision: revision(revisions.defaults)?,
                system_override_revision: if system.is_some() {
                    revisions.system_overrides.map(revision).transpose()?
                } else {
                    None
                },
                companion_revision: revisions.companion.map(revision).transpose()?,
                personal_override_revision: if personal.is_some() {
                    revisions.personal_overrides.map(revision).transpose()?
                } else {
                    None
                },
            },
        })
    }
    /// Match the exact installed default package recorded in a generation.
    /// Package inventory must itself come from the native trusted provider.
    pub fn matches_packages(&self, packages: &[PackageRevision]) -> bool {
        let matching: Vec<_> =
            packages.iter().filter(|package| package.package_name == self.package.name).collect();
        matching.len() == 1
            && matching[0].before_nevra.as_ref() == Some(&self.package.nevra)
            && matching[0].upstream_revision == self.package.upstream_revision
    }
}

pub struct ResolvedInstalledProfile {
    pub settings: ResolvedSettings,
    pub provenance: ProfileRevision,
}

#[derive(Debug)]
pub enum ProfileStoreError {
    RpmDefaults(crate::rpm_defaults::RpmDefaultsError),
    InvalidDocument,
    UnsupportedSchema,
    TooLarge,
    DigestMismatch,
    ProfileMismatch,
    PackageMismatch,
    UnsafeStorage,
    AlreadyExists,
    NeedsReconciliation,
    ConcurrentModification,
    UnknownBundleRevision,
    InsufficientSpace,
    Machine(crate::machine_state::MachineStateError),
    Mount(crate::state_mount::StateMountError),
    Filesystem(rustix::io::Errno),
    Io(std::io::Error),
}
impl std::fmt::Display for ProfileStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Parser errors deliberately carry no source text or document values.
        write!(f, "local profile storage unavailable: {self:?}")
    }
}
impl std::error::Error for ProfileStoreError {}
impl From<rustix::io::Errno> for ProfileStoreError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Filesystem(error)
    }
}
impl From<std::io::Error> for ProfileStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Protected storage for inactive immutable bundles. No public constructor
/// accepts a path/descriptor/claimed owner or an expected mount supplied by a
/// client. Opening requires existing storage; provisioning remains explicit.
pub struct ProfileBundleStore {
    profiles: File,
    staging: File,
    uid: u32,
    gid: u32,
    native: bool,
}
impl ProfileBundleStore {
    /// Offline console provisioning under the same protected machine lock as
    /// capture. The named lock must be the pinned inode and remain exclusively
    /// held by this descriptor; an existing partial store is never repaired.
    pub fn open_for_console(state: &File, lock: &File) -> Result<Self, ProfileStoreError> {
        require_same_inode(state, &native_state()?)?;
        let named = File::from(fs::openat2(
            state,
            crate::state_mount::MUTATION_LOCK_NAME,
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            RESOLVE,
        )?);
        require_same_inode(lock, &named)?;
        let metadata = fs::fstat(lock)?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
            || metadata.st_uid != 0
            || metadata.st_gid != 0
            || metadata.st_mode & 0o7777 != 0o600
            || metadata.st_nlink != 1
        {
            return Err(ProfileStoreError::UnsafeStorage);
        }
        fs::flock(lock, fs::FlockOperation::NonBlockingLockExclusive)?;
        match Self::open_existing(state, 0, 0, true) {
            Ok(store) => Ok(store),
            Err(ProfileStoreError::Filesystem(rustix::io::Errno::NOENT)) => {
                Self::provision_locked(state)
            }
            Err(error) => Err(error),
        }
    }
    /// Called only by the native session while it owns the machine lock and
    /// has revalidated the dedicated state mount. Partial stores are retained
    /// for reconciliation; an existing object is never repaired or adopted.
    pub(crate) fn provision_locked(state: &File) -> Result<Self, ProfileStoreError> {
        require_directory(state, 0, 0, 0o700)?;
        require_same_inode(state, &native_state()?)?;
        match fs::mkdirat(state, "profiles", Mode::from_raw_mode(0o700)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => return Err(ProfileStoreError::NeedsReconciliation),
            Err(error) => return Err(error.into()),
        }
        fs::fsync(state)?;
        let profiles = open_dir(state, "profiles")?;
        require_directory(&profiles, 0, 0, 0o700)?;
        fs::mkdirat(&profiles, ".staging", Mode::from_raw_mode(0o700))?;
        let staging = open_dir(&profiles, ".staging")?;
        require_directory(&staging, 0, 0, 0o700)?;
        fs::fsync(&staging)?;
        fs::fsync(&profiles)?;
        require_same_inode(state, &native_state()?)?;
        Self::open_existing(state, 0, 0, true)
    }
    pub fn open_state_mount() -> Result<Self, ProfileStoreError> {
        let state = native_state()?;
        Self::open_existing(&state, 0, 0, true)
    }
    fn open_existing(
        state: &File,
        uid: u32,
        gid: u32,
        native: bool,
    ) -> Result<Self, ProfileStoreError> {
        require_directory(state, uid, gid, 0o700)?;
        let profiles = open_dir(state, "profiles")?;
        require_directory(&profiles, uid, gid, 0o700)?;
        let staging = open_dir(&profiles, ".staging")?;
        require_directory(&staging, uid, gid, 0o700)?;
        Ok(Self { profiles, staging, uid, gid, native })
    }
    fn revalidate(&self) -> Result<(), ProfileStoreError> {
        require_directory(&self.profiles, self.uid, self.gid, 0o700)?;
        require_directory(&self.staging, self.uid, self.gid, 0o700)?;
        if self.native {
            let state = native_state()?;
            let current = Self::open_existing(&state, 0, 0, true)?;
            for (old, new) in
                [(&self.profiles, &current.profiles), (&self.staging, &current.staging)]
            {
                let old = fs::fstat(old)?;
                let new = fs::fstat(new)?;
                if old.st_dev != new.st_dev || old.st_ino != new.st_ino {
                    return Err(ProfileStoreError::UnsafeStorage);
                }
            }
        }
        Ok(())
    }
    /// Stage and durably publish a content-addressed candidate only. Existing
    /// identical published content is read back; any incomplete stage, unsafe
    /// metadata, conflict or extra directory entry requires reconciliation.
    /// Caller holds the protected machine writer lock; activation is separate.
    pub fn publish_candidate(
        &self,
        transaction: &Identifier,
        bundle: &InstalledProfileBundle,
    ) -> Result<VerifiedProfileBundle, ProfileStoreError> {
        self.revalidate()?;
        let bytes = bundle.canonical_bytes()?;
        let id = digest(&bytes);
        match self.load(&id) {
            Ok(existing) => return Ok(existing),
            Err(ProfileStoreError::Filesystem(rustix::io::Errno::NOENT)) => {}
            Err(error) => return Err(error),
        }
        let available = fs::fstatvfs(&self.staging)?;
        let block = if available.f_frsize == 0 { available.f_bsize } else { available.f_frsize };
        let required = (bytes.len() as u64)
            .checked_add(block.saturating_mul(4))
            .ok_or(ProfileStoreError::InsufficientSpace)?;
        if block == 0 || available.f_bavail.saturating_mul(block) < required {
            return Err(ProfileStoreError::InsufficientSpace);
        }
        match fs::mkdirat(&self.staging, transaction.as_str(), Mode::from_raw_mode(0o700)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => return Err(ProfileStoreError::NeedsReconciliation),
            Err(error) => return Err(error.into()),
        }
        let stage = open_dir(&self.staging, transaction.as_str())?;
        require_directory(&stage, self.uid, self.gid, 0o700)?;
        let mut file = File::from(fs::openat2(
            &stage,
            "bundle.json",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
            RESOLVE,
        )?);
        file.write_all(&bytes)?;
        fs::fchmod(&file, Mode::from_raw_mode(0o400))?;
        fs::fsync(&file)?;
        fs::fsync(&stage)?;
        let staged = verify_bundle(stage, &id, self.uid, self.gid, 0o700)?;
        if staged.bundle != *bundle {
            return Err(ProfileStoreError::DigestMismatch);
        }
        self.revalidate()?;
        fs::renameat_with(
            &self.staging,
            transaction.as_str(),
            &self.profiles,
            id.as_str(),
            fs::RenameFlags::NOREPLACE,
        )?;
        // A cross-parent directory rename needs write permission to update
        // '..'. Seal after rename; readers reject a crash-left mode0700 object.
        fs::fchmod(&staged.directory, Mode::from_raw_mode(0o500))?;
        fs::fsync(&staged.directory)?;
        fs::fsync(&self.staging)?;
        fs::fsync(&self.profiles)?;
        self.load(&id)
    }
    pub fn load(&self, id: &Sha256) -> Result<VerifiedProfileBundle, ProfileStoreError> {
        self.revalidate()?;
        let mut verified =
            verify_bundle(open_dir(&self.profiles, id.as_str())?, id, self.uid, self.gid, 0o500)?;
        verified.namespace = Some(File::from(rustix::io::dup(&self.profiles)?));
        verified.native = self.native;
        Ok(verified)
    }
    /// Resolve only the bundle explicitly bound by sealed profile provenance.
    /// Old opaque revisions and missing bundles never choose another object.
    pub fn load_revision(
        &self,
        revision: &Revision,
    ) -> Result<VerifiedProfileBundle, ProfileStoreError> {
        let value = revision
            .as_str()
            .strip_prefix(REVISION_PREFIX)
            .ok_or(ProfileStoreError::UnknownBundleRevision)?;
        let id = Sha256::new(value).map_err(|_| ProfileStoreError::UnknownBundleRevision)?;
        self.load(&id)
    }

    /// Reproduce a saved effective profile offline from its exact protected
    /// bundle and verified personal payload. No installed package path or
    /// companion checkout is consulted. Call alongside native snapshot and
    /// generation verification before reporting complete profile availability.
    pub fn resolve_generation(
        &self,
        generation: &crate::generation::VerifiedGeneration,
    ) -> Result<ResolvedInstalledProfile, ProfileStoreError> {
        let manifest = generation.manifest();
        let provenance =
            manifest.profile_revision.as_ref().ok_or(ProfileStoreError::UnknownBundleRevision)?;
        let bundle = self.load_revision(&provenance.profile_revision)?;
        if !bundle.bundle.matches_packages(&manifest.package_revisions) {
            return Err(ProfileStoreError::PackageMismatch);
        }
        let record = manifest
            .files()
            .iter()
            .find(|record| {
                (record.root == crate::StateRoot::UserRyokuConfig
                    && record.path.as_str() == ryoku_profiles::LOCAL_PROFILE_FILE)
                    || (record.root == crate::StateRoot::UserHome
                        && record.path.as_str() == crate::installed_inventory::PERSONAL_PROFILE)
            })
            .ok_or(ProfileStoreError::InvalidDocument)?;
        let personal = match &record.kind {
            crate::EntryKind::Absent => None,
            crate::EntryKind::RegularFile { sha256, .. } => Some(
                generation.read_payload(sha256).map_err(|_| ProfileStoreError::DigestMismatch)?,
            ),
            _ => return Err(ProfileStoreError::InvalidDocument),
        };
        let resolved = bundle.bundle.resolve(personal.as_deref())?;
        if resolved.provenance != *provenance {
            return Err(ProfileStoreError::ProfileMismatch);
        }
        bundle.revalidate()?;
        Ok(resolved)
    }
}

pub struct VerifiedProfileBundle {
    directory: File,
    id: Sha256,
    bundle: InstalledProfileBundle,
    uid: u32,
    gid: u32,
    namespace: Option<File>,
    native: bool,
}
impl VerifiedProfileBundle {
    pub fn id(&self) -> &Sha256 {
        &self.id
    }
    pub fn bundle(&self) -> &InstalledProfileBundle {
        &self.bundle
    }
    pub fn revalidate(&self) -> Result<(), ProfileStoreError> {
        let namespace = self.namespace.as_ref().ok_or(ProfileStoreError::UnsafeStorage)?;
        if self.native {
            let current = ProfileBundleStore::open_state_mount()?;
            require_same_inode(namespace, &current.profiles)?;
        }
        require_directory(namespace, self.uid, self.gid, 0o700)?;
        let current = open_dir(namespace, self.id.as_str())?;
        require_same_inode(&self.directory, &current)?;
        let verified = verify_bundle(current, &self.id, self.uid, self.gid, 0o500)?;
        if verified.bundle != self.bundle {
            return Err(ProfileStoreError::DigestMismatch);
        }
        Ok(())
    }
}
fn native_state() -> Result<File, ProfileStoreError> {
    let root = File::open("/")?;
    let record = crate::machine_state::read_machine_state_record(&root)
        .map_err(ProfileStoreError::Machine)?;
    let (state, _) = crate::state_mount::collect_and_open_state_mount(&root, record.identity())
        .map_err(ProfileStoreError::Mount)?;
    Ok(File::from(state))
}
pub(crate) fn read_packaged_defaults(
    root: &File,
    uid: u32,
    gid: u32,
) -> Result<Vec<u8>, ProfileStoreError> {
    // /usr may be a separate native mount; the fixed package directory cannot
    // contain a symlink, and the file itself may not cross to another mount.
    let directory = File::from(fs::openat2(
        root,
        "usr/share/ryoku/config",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )?);
    require_directory(&directory, uid, gid, 0o755)?;
    let file = File::from(fs::openat2(
        &directory,
        "profile.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        RESOLVE,
    )?);
    let before = fs::fstat(&file)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_uid != uid
        || before.st_gid != gid
        || before.st_mode & 0o7777 != 0o644
        || before.st_nlink != 1
        || before.st_size < 0
        || before.st_size as usize > ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES
    {
        return Err(ProfileStoreError::UnsafeStorage);
    }
    let mut bytes = Vec::new();
    (&file)
        .take((ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let after = fs::fstat(&file)?;
    if before.st_ctime != after.st_ctime
        || before.st_ctime_nsec != after.st_ctime_nsec
        || before.st_mtime != after.st_mtime
        || before.st_mtime_nsec != after.st_mtime_nsec
        || before.st_size != after.st_size
        || bytes.len() as i64 != after.st_size
    {
        return Err(ProfileStoreError::ConcurrentModification);
    }
    ProfileDocument::parse(&bytes).map_err(|_| ProfileStoreError::InvalidDocument)?;
    Ok(bytes)
}
fn open_dir(parent: &File, path: &str) -> Result<File, ProfileStoreError> {
    Ok(File::from(fs::openat2(
        parent,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        RESOLVE,
    )?))
}
fn require_directory(file: &File, uid: u32, gid: u32, mode: u32) -> Result<(), ProfileStoreError> {
    let stat = fs::fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != uid
        || stat.st_gid != gid
        || stat.st_mode & 0o7777 != mode
    {
        return Err(ProfileStoreError::UnsafeStorage);
    }
    Ok(())
}
fn verify_bundle(
    directory: File,
    id: &Sha256,
    uid: u32,
    gid: u32,
    directory_mode: u32,
) -> Result<VerifiedProfileBundle, ProfileStoreError> {
    require_directory(&directory, uid, gid, directory_mode)?;
    let mut names = Vec::new();
    for entry in fs::Dir::read_from(&directory)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(name.to_vec());
        }
        if names.len() > 1 {
            return Err(ProfileStoreError::UnsafeStorage);
        }
    }
    if names != [b"bundle.json".to_vec()] {
        return Err(ProfileStoreError::UnsafeStorage);
    }
    let file = File::from(fs::openat2(
        &directory,
        "bundle.json",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        RESOLVE,
    )?);
    let before = fs::fstat(&file)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_uid != uid
        || before.st_gid != gid
        || before.st_mode & 0o7777 != 0o400
        || before.st_nlink != 1
        || before.st_size < 0
        || before.st_size as usize > MAX_BUNDLE_BYTES
    {
        return Err(ProfileStoreError::UnsafeStorage);
    }
    let mut bytes = Vec::new();
    (&file).take((MAX_BUNDLE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    let after = fs::fstat(&file)?;
    if before.st_size != after.st_size
        || before.st_mtime != after.st_mtime
        || before.st_mtime_nsec != after.st_mtime_nsec
        || before.st_ctime != after.st_ctime
        || before.st_ctime_nsec != after.st_ctime_nsec
        || bytes.len() as i64 != after.st_size
    {
        return Err(ProfileStoreError::ConcurrentModification);
    }
    if digest(&bytes) != *id {
        return Err(ProfileStoreError::DigestMismatch);
    }
    let bundle = InstalledProfileBundle::parse(&bytes)?;
    if bundle.canonical_bytes()? != bytes {
        return Err(ProfileStoreError::InvalidDocument);
    }
    Ok(VerifiedProfileBundle {
        directory,
        id: id.clone(),
        bundle,
        uid,
        gid,
        namespace: None,
        native: false,
    })
}
fn require_same_inode(old: &File, current: &File) -> Result<(), ProfileStoreError> {
    let old = fs::fstat(old)?;
    let current = fs::fstat(current)?;
    if old.st_dev != current.st_dev || old.st_ino != current.st_ino {
        return Err(ProfileStoreError::ConcurrentModification);
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> Sha256 {
    Sha256::new(
        Digest256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
    )
    .expect("SHA256 hex is valid")
}

#[cfg(test)]
#[path = "installed_profiles_tests.rs"]
pub(crate) mod tests;
