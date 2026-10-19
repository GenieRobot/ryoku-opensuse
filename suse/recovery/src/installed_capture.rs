//! Code-owned enrollment and captured-profile provenance for complete capture.
//!
//! `InstalledInventory` cannot be deserialized or constructed from a client's
//! paths. The full installed catalogue remains unavailable. A separate explicit
//! personal-profile scope covers its Rust writer only. Adding an enrollment
//! requires a reviewed writer, exact paths/metadata policy, and local profile
//! persistence contract. Tests use private synthetic catalogues, not runtime
//! exceptions. The low-level declared capture API remains a trusted primitive.
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

#[allow(dead_code)] // Constructors become live only with audited installed writers.
#[derive(Clone, Copy)]
enum Owner {
    Account,
    System,
}
#[allow(dead_code)] // No candidate is silently promoted to an installed entry.
#[derive(Clone, Copy)]
enum Kind {
    Regular,
    Directory,
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
    /// Full installed Ryoku scope is unavailable. The separate personal-profile
    /// scope does not establish coverage for all configuration writers.
    pub fn current() -> Result<Self, InventoryError> {
        Self::compile(&[], &[], None)
    }

    /// Exact scope of the native personal-profile writer only. This is not
    /// the full installed Ryoku inventory and cannot cover any other writer.
    /// Authorization, its common writer guard and native bundle checks remain
    /// the service coordinator's responsibility; no transport accepts this plan.
    pub fn personal_profile_writer() -> Result<Self, InventoryError> {
        Self::compile(
            &[ryoku_profiles::PERSONAL_PROFILE_WRITER_ID],
            &[Enrollment {
                root: StateRoot::UserRyokuConfig,
                path: ryoku_profiles::LOCAL_PROFILE_FILE,
                writer: ryoku_profiles::PERSONAL_PROFILE_WRITER_ID,
                owner: Owner::Account,
                kind: Kind::Regular,
                modes: ryoku_profiles::PERSONAL_PROFILE_MODES,
                max_bytes: ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES as u64,
                absence_allowed: true,
            }],
            None,
        )
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
                (StateRoot::UserRyokuConfig, Owner::Account) | (StateRoot::EtcRyoku, Owner::System)
            );
            if !valid_owner
                || entry.writer.is_empty()
                || entry.modes.is_empty()
                || entry.modes.iter().any(|mode| {
                    mode & !0o777 != 0
                        || (!matches!(entry.kind, Kind::Symlink) && mode & 0o022 != 0)
                })
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
            vec![(StateRoot::UserRyokuConfig, ryoku_profiles::LOCAL_PROFILE_FILE)]
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
                (EntryKind::RegularFile { metadata, size, hard_link_group, .. }, Kind::Regular)
                    if *size <= entry.max_bytes && hard_link_group.is_none() =>
                {
                    metadata
                }
                (EntryKind::Directory { metadata }, Kind::Directory) => metadata,
                (EntryKind::Symlink { metadata, target }, Kind::Symlink)
                    if target.len() as u64 <= entry.max_bytes =>
                {
                    metadata
                }
                _ => return Err(InventoryError::EntryPolicy),
            };
            let (uid, gid) = match entry.owner {
                Owner::Account => (account.uid, account.gid),
                Owner::System => (0, 0),
            };
            if metadata.uid != uid || metadata.gid != gid || !entry.modes.contains(&metadata.mode) {
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
    capture_installed_configuration_guarded(
        store,
        inventory,
        roots,
        request,
        capture_files,
        bundle,
        || Ok(()),
    )
}

/// Recheck authenticated roots, writer exclusion and installed inputs both at
/// entry and immediately before sealing/publication. The callback is a trusted
/// service boundary; it cannot establish full inventory coverage by assertion.
pub fn capture_with_installed_profile_bundle_guarded(
    store: &GenerationStore,
    inventory: &InstalledInventory,
    roots: InstalledRoots<'_>,
    request: InstalledCaptureRequest,
    bundle: &crate::installed_profiles::VerifiedProfileBundle,
    revalidate: impl Fn() -> Result<(), GenerationCaptureError>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    capture_installed_configuration_guarded(
        store,
        inventory,
        roots,
        request,
        capture_inventory,
        Some(bundle),
        revalidate,
    )
}

fn capture_installed_configuration_guarded(
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
    revalidate: impl Fn() -> Result<(), GenerationCaptureError>,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    revalidate()?;
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
            revalidate()?;
            inventory.validate_records(records, &roots.account)?;
            let provenance = if let Some(bundle) = bundle {
                bundle.revalidate().map_err(GenerationCaptureError::ProfileStore)?;
                let record = records
                    .iter()
                    .find(|record| {
                        record.root == StateRoot::UserRyokuConfig
                            && record.path.as_str() == ryoku_profiles::LOCAL_PROFILE_FILE
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
    )
}

fn digest(bytes: &[u8]) -> String {
    Sha256Digest::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[path = "installed_capture_tests.rs"]
mod tests;
