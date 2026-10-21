//! Code-owned enrollment and captured-profile provenance for complete capture.
//!
//! `InstalledInventory` cannot be deserialized or constructed from a client's
//! paths. The installed catalogue enumerates bounded preferences from the
//! pinned upstream runtime and the port-owned typed personal profile. Referenced
//! wallpaper/media paths, credentials, caches and unrelated home data are never
//! followed. Immutable local bundles retain all profile layer inputs offline.
//!
//! The service must resolve these descriptors (the user descriptor comes from
//! its authenticated account resolver), validate the protected store, and hold
//! all enrolled writers frozen from native snapshot creation through mutation.
//! This module neither authenticates descriptors nor claims an advisory lock
//! quiesces upstream writers. No paths or profile values come from a manifest.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
};

use rustix::fs::{FileType, fstat};
use ryoku_profiles::{ProfileDocument, ProfileLayers, ResolvedSettings};
use sha2::{Digest, Sha256 as Sha256Digest};

use crate::{
    EntryKind, FileRecord, Identifier, ProfileRevision, RecoveryManifest, RelativePath, Revision,
    StateRoot,
    capture::capture_inventory,
    generation::{GenerationStore, Payloads},
    generation_capture::{
        DeclaredRoot, GenerationCaptureError, GenerationCaptureRequest,
        capture_publish_verify_checked,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryError {
    NoAuditedWriters,
    IncompleteWriterCoverage,
    InvalidEnrollment,
    MissingRoot,
    UnsafeRoot,
    IncompleteInventory,
    EntryPolicy,
    MissingProfileDefaults,
    InvalidProfile,
    ProfileIdentityMismatch,
    UnsupportedManagedState,
}
impl std::fmt::Display for InventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never include private profile bytes or host paths in errors.
        write!(f, "{self:?}")
    }
}
impl std::error::Error for InventoryError {}
impl From<InventoryError> for GenerationCaptureError {
    fn from(error: InventoryError) -> Self {
        Self::Inventory(error)
    }
}

#[derive(Clone, Copy)]
enum Owner {
    Account,
    #[cfg(test)]
    System,
}
#[derive(Clone, Copy)]
enum Kind {
    Regular,
    Preference,
    #[cfg(test)]
    Directory,
    #[cfg(test)]
    Symlink,
}

#[derive(Clone, Copy)]
struct Enrollment {
    root: StateRoot,
    path: &'static str,
    writer: &'static str,
    owner: Owner,
    kind: Kind,
    modes: &'static [u32],
    max_bytes: u64,
    absence_allowed: bool,
}

#[derive(Clone, Copy)]
struct ProfileSources {
    defaults: (StateRoot, &'static str),
    system: (StateRoot, &'static str),
    companion: (StateRoot, &'static str),
    personal: (StateRoot, &'static str),
}
impl ProfileSources {
    fn keys(self) -> [(StateRoot, &'static str); 4] {
        [self.defaults, self.system, self.companion, self.personal]
    }
}

/// Unforgeable through the public API: only the code-owned installed catalogue
/// can produce this plan. It contains all enrolled state, never only the files
/// changed by the requested operation. An absent path stays in this plan.
pub struct InstalledInventory {
    entries: Vec<Enrollment>,
    paths: BTreeMap<StateRoot, Vec<RelativePath>>,
    profiles: Option<ProfileSources>,
}
impl InstalledInventory {
    /// Independently collect the complete code-owned scope twice for plan
    /// binding. This includes absence, content and metadata; unknown managed
    /// files refuse. The caller holds the native account writer lease.
    pub(crate) fn fingerprint(
        &self,
        home: &File,
        account: &CaptureAccount,
    ) -> Result<crate::Sha256, GenerationCaptureError> {
        Self::validate_home(home)?;
        let paths =
            self.paths.get(&StateRoot::UserHome).ok_or(InventoryError::IncompleteInventory)?;
        let first = crate::capture::capture_inventory(
            home,
            StateRoot::UserHome,
            paths,
            crate::capture::CaptureLimits::default(),
        )?;
        let second = crate::capture::capture_inventory(
            home,
            StateRoot::UserHome,
            paths,
            crate::capture::CaptureLimits::default(),
        )?;
        self.validate_records(&first.records, account)?;
        self.validate_records(&second.records, account)?;
        if first.records != second.records || first.metadata_payloads != second.metadata_payloads {
            return Err(GenerationCaptureError::CoordinationLost);
        }
        Self::validate_home(home)?;
        let bytes =
            serde_json::to_vec(&first.records).map_err(|_| InventoryError::InvalidEnrollment)?;
        crate::Sha256::new(digest(&bytes)).map_err(|_| InventoryError::InvalidEnrollment.into())
    }
    /// Validate bounded asset/override namespaces before selecting the fixed
    /// inventory. An unsupported managed file blocks capture instead of being
    /// silently omitted from a supposedly complete generation.
    pub fn for_home(home: &File) -> Result<Self, GenerationCaptureError> {
        let inventory = Self::current()?;
        Self::validate_home(home)?;
        Ok(inventory)
    }

    fn validate_home(home: &File) -> Result<(), GenerationCaptureError> {
        for path in [".config/ryoku", ".config/ryogami-wall"] {
            let mut visited = 0;
            validate_managed_tree(home, path, 0, &mut visited)?;
        }
        validate_wallpaper_state_location(home)?;
        Ok(())
    }
    /// Complete fixed first-sprint preference scope for the pinned runtime on
    /// the standard XDG layout. Unknown data remains outside this scope.
    pub fn current() -> Result<Self, InventoryError> {
        let entries: Vec<_> = crate::installed_inventory::PREFERENCES
            .iter()
            .map(|path| Enrollment {
                root: StateRoot::UserHome,
                path,
                writer: "pinned-session-preferences",
                owner: Owner::Account,
                kind: if *path == crate::installed_inventory::PERSONAL_PROFILE {
                    Kind::Regular
                } else {
                    Kind::Preference
                },
                modes: &[0o600, 0o640, 0o644, 0o777],
                max_bytes: if *path == crate::installed_inventory::PERSONAL_PROFILE {
                    ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES as u64
                } else {
                    4 * 1024 * 1024
                },
                absence_allowed: true,
            })
            .collect();
        Self::compile(&["pinned-session-preferences"], &entries, None)
    }

    fn compile(
        required_writers: &[&'static str],
        entries: &[Enrollment],
        profiles: Option<ProfileSources>,
    ) -> Result<Self, InventoryError> {
        if required_writers.is_empty() {
            return Err(InventoryError::NoAuditedWriters);
        }
        let writers: BTreeSet<_> = required_writers.iter().copied().collect();
        let covered: BTreeSet<_> = entries.iter().map(|entry| entry.writer).collect();
        if writers.len() != required_writers.len() || writers != covered {
            return Err(InventoryError::IncompleteWriterCoverage);
        }
        let mut keys = BTreeSet::new();
        let mut paths: BTreeMap<StateRoot, Vec<RelativePath>> = BTreeMap::new();
        for entry in entries {
            let valid_owner = matches!(
                (entry.root, entry.owner),
                (StateRoot::UserRyokuConfig | StateRoot::UserHome, Owner::Account)
            );
            #[cfg(test)]
            let valid_owner = valid_owner
                || matches!((entry.root, entry.owner), (StateRoot::EtcRyoku, Owner::System));
            if !valid_owner
                || entry.writer.is_empty()
                || entry.modes.is_empty()
                || entry
                    .modes
                    .iter()
                    .any(|mode| mode & !0o777 != 0 || (mode != &0o777 && mode & 0o022 != 0))
                || entry.max_bytes == 0
                || !keys.insert((entry.root, entry.path))
            {
                return Err(InventoryError::InvalidEnrollment);
            }
            let path =
                RelativePath::new(entry.path).map_err(|_| InventoryError::InvalidEnrollment)?;
            paths.entry(entry.root).or_default().push(path);
        }
        // No captured defaults means the protected bundle supplies immutable
        // package/system/companion inputs; only personal selection is a file
        // restore target. Its path is fixed, never supplied by a bundle.
        let profile_keys = profiles.map(|sources| sources.keys().to_vec()).unwrap_or_else(|| {
            vec![if entries.iter().any(|entry| entry.root == StateRoot::UserHome) {
                (StateRoot::UserHome, crate::installed_inventory::PERSONAL_PROFILE)
            } else {
                (StateRoot::UserRyokuConfig, ryoku_profiles::LOCAL_PROFILE_FILE)
            }]
        });
        if profile_keys.iter().collect::<BTreeSet<_>>().len() != profile_keys.len()
            || profile_keys.iter().any(|key| {
                !entries.iter().any(|entry| {
                    (entry.root, entry.path) == *key && matches!(entry.kind, Kind::Regular)
                })
            })
        {
            return Err(InventoryError::InvalidEnrollment);
        }
        for paths in paths.values_mut() {
            paths.sort();
        }
        Ok(Self { entries: entries.to_vec(), paths, profiles })
    }

    fn validate_records(
        &self,
        records: &[FileRecord],
        account: &CaptureAccount,
    ) -> Result<(), InventoryError> {
        let actual: BTreeSet<_> =
            records.iter().map(|record| (record.root, record.path.as_str())).collect();
        let expected: BTreeSet<_> =
            self.entries.iter().map(|entry| (entry.root, entry.path)).collect();
        if records.len() != self.entries.len() || actual != expected {
            return Err(InventoryError::IncompleteInventory);
        }
        for entry in &self.entries {
            let record = records
                .iter()
                .find(|record| record.root == entry.root && record.path.as_str() == entry.path)
                .ok_or(InventoryError::IncompleteInventory)?;
            let metadata = match (&record.kind, entry.kind) {
                (EntryKind::Absent, _) if entry.absence_allowed => continue,
                (
                    EntryKind::RegularFile { metadata, size, hard_link_group, .. },
                    Kind::Regular | Kind::Preference,
                ) if *size <= entry.max_bytes && hard_link_group.is_none() => metadata,
                #[cfg(test)]
                (EntryKind::Directory { metadata }, Kind::Directory) => metadata,
                #[cfg(test)]
                (EntryKind::Symlink { metadata, target }, Kind::Symlink)
                    if target.len() as u64 <= entry.max_bytes =>
                {
                    metadata
                }
                (EntryKind::Symlink { metadata, target }, Kind::Preference)
                    if target.len() <= 4096 =>
                {
                    metadata
                }
                _ => return Err(InventoryError::EntryPolicy),
            };
            let (uid, gid) = match entry.owner {
                Owner::Account => (account.uid, account.gid),
                #[cfg(test)]
                Owner::System => (0, 0),
            };
            if metadata.uid != uid
                || metadata.gid != gid
                || !entry.modes.contains(&metadata.mode)
                || (!matches!(record.kind, EntryKind::Symlink { .. }) && metadata.mode & 0o022 != 0)
            {
                return Err(InventoryError::EntryPolicy);
            }
        }
        Ok(())
    }

    fn profile(
        &self,
        records: &[FileRecord],
        payloads: &Payloads,
    ) -> Result<CapturedProfile, InventoryError> {
        let mut documents = Vec::with_capacity(4);
        for key in self.profiles.ok_or(InventoryError::MissingProfileDefaults)?.keys() {
            let record = records
                .iter()
                .find(|record| (record.root, record.path.as_str()) == key)
                .ok_or(InventoryError::IncompleteInventory)?;
            documents.push(match &record.kind {
                EntryKind::Absent => None,
                EntryKind::RegularFile { sha256, size, .. } => {
                    let bytes = payloads.get(sha256).ok_or(InventoryError::InvalidProfile)?;
                    if bytes.len() as u64 != *size || digest(bytes) != sha256.as_str() {
                        return Err(InventoryError::InvalidProfile);
                    }
                    Some(
                        ProfileDocument::parse(bytes)
                            .map_err(|_| InventoryError::InvalidProfile)?,
                    )
                }
                _ => return Err(InventoryError::InvalidProfile),
            });
        }
        let defaults = documents[0].as_ref().ok_or(InventoryError::MissingProfileDefaults)?;
        if documents.iter().flatten().any(|document| document.id != defaults.id) {
            return Err(InventoryError::ProfileIdentityMismatch);
        }
        let layers = ProfileLayers {
            defaults: defaults.settings,
            system_overrides: documents[1].as_ref().map(|doc| doc.settings).unwrap_or_default(),
            companion: documents[2].as_ref().map(|doc| doc.settings),
            personal: documents[3].as_ref().map(|doc| doc.settings).unwrap_or_default(),
        };
        let revisions = layers.content_revisions();
        // Hash canonical documents in fixed layer order, preserving None vs a
        // present empty layer and the selected profile identity. Raw originals
        // (including formatting) remain in the immutable generation payloads.
        let canonical =
            serde_json::to_vec(&documents).map_err(|_| InventoryError::InvalidProfile)?;
        let revision = |value| Revision::new(value).map_err(|_| InventoryError::InvalidProfile);
        Ok(CapturedProfile {
            effective: layers.resolve(),
            provenance: ProfileRevision {
                profile_id: Identifier::new(defaults.id.as_str())
                    .map_err(|_| InventoryError::InvalidProfile)?,
                profile_revision: revision(format!("sha256-{}", digest(&canonical)))?,
                defaults_revision: revision(revisions.defaults)?,
                system_override_revision: documents[1]
                    .as_ref()
                    .map(|_| {
                        revisions
                            .system_overrides
                            .ok_or(InventoryError::InvalidProfile)
                            .and_then(revision)
                    })
                    .transpose()?,
                companion_revision: revisions.companion.map(revision).transpose()?,
                personal_override_revision: documents[3]
                    .as_ref()
                    .map(|_| {
                        revisions
                            .personal_overrides
                            .ok_or(InventoryError::InvalidProfile)
                            .and_then(revision)
                    })
                    .transpose()?,
            },
        })
    }
}

/// Identity resolved by the service from the authenticated peer, never from
/// a client-provided owner or environment variable. This is data, not a token
/// conferring authorization; the service retains the account binding.
#[derive(Clone)]
pub struct CaptureAccount {
    pub uid: u32,
    pub gid: u32,
    pub principal: crate::AccountPrincipal,
}

/// The service obtains these pinned descriptors from fixed native resolvers.
/// There is no arbitrary root map, directory string, or path-list input.
pub struct InstalledRoots<'a> {
    pub user_configuration: &'a File,
    pub account_home: Option<&'a File>,
    pub system_configuration: Option<&'a File>,
    pub account: CaptureAccount,
}

pub struct CapturedProfile {
    pub effective: ResolvedSettings,
    pub provenance: ProfileRevision,
}

/// Transaction facts supplied by the authorized coordinator. Paths, owner
/// policy, profile identity and layer revisions cannot be supplied here.
pub struct InstalledCaptureRequest {
    pub transaction_id: Identifier,
    pub generation_id: Identifier,
    pub parent_generation_id: Option<Identifier>,
    pub snapshot: crate::generation_capture::VerifiedSnapshotLink,
    pub reason: crate::OperationReason,
    pub created_utc: String,
    pub package_revisions: Vec<crate::PackageRevision>,
    pub provider_revision: Option<crate::ProviderRevision>,
    pub limits: crate::capture::CaptureLimits,
}

/// Capture all enrolled state and derive local profile provenance from the
/// exact bytes being sealed. All paths come from the compiled catalogue.
/// This does not authorize a provider write; native linkage, root provenance,
/// writer quiescence, and rollback-independent storage remain service gates.
pub fn capture_installed_configuration<'a>(
    store: &GenerationStore,
    inventory: &'a InstalledInventory,
    roots: InstalledRoots<'a>,
    request: InstalledCaptureRequest,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_installed_configuration_using(store, inventory, roots, request, capture_inventory)
}

/// Capture with a verified local immutable bundle selected by the authorized
/// service. Bind its exact ID in profile_revision, capture only the enrolled
/// personal file, and retain packaged defaults as provenance, not restore files.
pub fn capture_with_installed_profile_bundle(
    store: &GenerationStore,
    inventory: &InstalledInventory,
    roots: InstalledRoots<'_>,
    request: InstalledCaptureRequest,
    bundle: &crate::installed_profiles::VerifiedProfileBundle,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_installed_configuration_using_profile(
        store,
        inventory,
        roots,
        request,
        capture_inventory,
        Some(bundle),
    )
}

fn capture_installed_configuration_using(
    store: &GenerationStore,
    inventory: &InstalledInventory,
    roots: InstalledRoots<'_>,
    request: InstalledCaptureRequest,
    capture_files: impl Fn(
        &File,
        StateRoot,
        &[RelativePath],
        crate::capture::CaptureLimits,
    )
        -> Result<crate::capture::CapturedInventory, crate::capture::CaptureError>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_installed_configuration_using_profile(
        store,
        inventory,
        roots,
        request,
        capture_files,
        None,
    )
}

fn capture_installed_configuration_using_profile(
    store: &GenerationStore,
    inventory: &InstalledInventory,
    roots: InstalledRoots<'_>,
    request: InstalledCaptureRequest,
    capture_files: impl Fn(
        &File,
        StateRoot,
        &[RelativePath],
        crate::capture::CaptureLimits,
    )
        -> Result<crate::capture::CapturedInventory, crate::capture::CaptureError>,
    bundle: Option<&crate::installed_profiles::VerifiedProfileBundle>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_installed_configuration_checked(
        store,
        inventory,
        roots,
        request,
        capture_files,
        bundle,
        || Ok(()),
    )
}

pub(crate) fn capture_installed_configuration_checked(
    store: &GenerationStore,
    inventory: &InstalledInventory,
    roots: InstalledRoots<'_>,
    request: InstalledCaptureRequest,
    capture_files: impl Fn(
        &File,
        StateRoot,
        &[RelativePath],
        crate::capture::CaptureLimits,
    )
        -> Result<crate::capture::CapturedInventory, crate::capture::CaptureError>,
    bundle: Option<&crate::installed_profiles::VerifiedProfileBundle>,
    boundary: impl Fn() -> Result<(), GenerationCaptureError>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    let checked_boundary = || {
        boundary()?;
        if inventory.paths.contains_key(&StateRoot::UserHome) {
            InstalledInventory::validate_home(
                roots.account_home.ok_or(InventoryError::MissingRoot)?,
            )?;
            // A namespace scan may take time; do not let a writer dying during
            // that scan turn its earlier lease check into publication approval.
            boundary()?;
        }
        Ok(())
    };
    checked_boundary()?;
    if let Some(bundle) = bundle {
        if inventory.profiles.is_some() {
            return Err(InventoryError::InvalidEnrollment.into());
        }
        bundle.revalidate().map_err(GenerationCaptureError::ProfileStore)?;
        if !bundle.bundle().matches_packages(&request.package_revisions) {
            return Err(GenerationCaptureError::ProfileStore(
                crate::installed_profiles::ProfileStoreError::PackageMismatch,
            ));
        }
    }
    let mut declared = Vec::new();
    for (root, paths) in &inventory.paths {
        let (descriptor, uid, gid) = match root {
            StateRoot::UserRyokuConfig => {
                (roots.user_configuration, roots.account.uid, roots.account.gid)
            }
            StateRoot::UserHome => (
                roots.account_home.ok_or(InventoryError::MissingRoot)?,
                roots.account.uid,
                roots.account.gid,
            ),
            StateRoot::EtcRyoku => {
                (roots.system_configuration.ok_or(InventoryError::MissingRoot)?, 0, 0)
            }
            _ => return Err(InventoryError::InvalidEnrollment.into()),
        };
        let stat = fstat(descriptor)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
            || stat.st_uid != uid
            || stat.st_gid != gid
            || !matches!(stat.st_mode & 0o7777, 0o700 | 0o750 | 0o755)
        {
            return Err(InventoryError::UnsafeRoot.into());
        }
        declared.push(DeclaredRoot { root: *root, descriptor, paths });
    }
    let request = GenerationCaptureRequest {
        transaction_id: request.transaction_id,
        generation_id: request.generation_id,
        parent_generation_id: request.parent_generation_id,
        snapshot: request.snapshot,
        reason: request.reason,
        created_utc: request.created_utc,
        package_revisions: request.package_revisions,
        provider_revision: request.provider_revision,
        profile_revision: None,
        roots: &declared,
        limits: request.limits,
    };
    capture_publish_verify_checked(
        store,
        request,
        capture_files,
        |store, bytes, files| store.ensure_available_space(bytes, files),
        |records, payloads, _| {
            checked_boundary()?;
            inventory.validate_records(records, &roots.account)?;
            let provenance = if let Some(bundle) = bundle {
                bundle.revalidate().map_err(GenerationCaptureError::ProfileStore)?;
                let record = records
                    .iter()
                    .find(|record| {
                        (record.root == StateRoot::UserRyokuConfig
                            && record.path.as_str() == ryoku_profiles::LOCAL_PROFILE_FILE)
                            || (record.root == StateRoot::UserHome
                                && record.path.as_str()
                                    == crate::installed_inventory::PERSONAL_PROFILE)
                    })
                    .ok_or(InventoryError::IncompleteInventory)?;
                let personal = match &record.kind {
                    EntryKind::Absent => None,
                    EntryKind::RegularFile { sha256, .. } => {
                        Some(payloads.get(sha256).ok_or(InventoryError::InvalidProfile)?.as_slice())
                    }
                    _ => return Err(InventoryError::InvalidProfile.into()),
                };
                bundle
                    .bundle()
                    .resolve(personal)
                    .map_err(GenerationCaptureError::ProfileStore)?
                    .provenance
            } else {
                inventory.profile(records, payloads)?.provenance
            };
            Ok(crate::generation_capture::CaptureProvenance {
                profile_revision: Some(provenance),
                account_principal: Some(roots.account.principal.clone()),
            })
        },
        checked_boundary,
    )
}

fn digest(bytes: &[u8]) -> String {
    Sha256Digest::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_managed_tree(
    home: &File,
    path: &str,
    depth: usize,
    visited: &mut usize,
) -> Result<(), GenerationCaptureError> {
    use rustix::fs::{Dir, Mode, OFlags, ResolveFlags, openat2};
    if depth > 16 || *visited > 4096 {
        return Err(InventoryError::UnsupportedManagedState.into());
    }
    let fd = match openat2(
        home,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in Dir::read_from(&fd)? {
        let entry = entry?;
        let name =
            entry.file_name().to_str().map_err(|_| InventoryError::UnsupportedManagedState)?;
        if matches!(name, "." | "..") {
            continue;
        }
        *visited += 1;
        if *visited > 4096 {
            return Err(InventoryError::UnsupportedManagedState.into());
        }
        let child = format!("{path}/{name}");
        if child == ".config/ryoku/.desktop.lock" {
            // Upstream's flock inode is coordination state, never a preference.
            continue;
        }
        if crate::installed_inventory::PREFERENCES.contains(&child.as_str()) {
            continue;
        }
        let prefix = format!("{child}/");
        if crate::installed_inventory::PREFERENCES.iter().any(|known| known.starts_with(&prefix)) {
            validate_managed_tree(home, &child, depth + 1, visited)?;
        } else {
            return Err(InventoryError::UnsupportedManagedState.into());
        }
    }
    Ok(())
}

fn validate_wallpaper_state_location(home: &File) -> Result<(), GenerationCaptureError> {
    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    use std::{io::Read, os::fd::AsRawFd};
    let fd = match openat2(
        home,
        ".config/ryoku/ryogami.json",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let stat = fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_size > 4 * 1024 * 1024
    {
        return Err(InventoryError::UnsupportedManagedState.into());
    }
    let mut bytes = Vec::new();
    File::from(fd).take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let config: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| InventoryError::UnsupportedManagedState)?;
    if let Some(cache) = config.get("paths").and_then(|v| v.get("cache")) {
        let value = cache.as_str().ok_or(InventoryError::UnsupportedManagedState)?;
        let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))?;
        let expected = home_path.join(".cache/ryogami");
        if !value.is_empty()
            && value != "~/.cache/ryogami"
            && std::path::Path::new(value) != expected
        {
            return Err(InventoryError::UnsupportedManagedState.into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "installed_capture_tests.rs"]
mod tests;
