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
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
};

use rustix::{
    fs::{FileType, Mode, OFlags, ResolveFlags, fstat, openat2},
    io::Errno,
};
use ryoku_profiles::{ProfileDocument, ProfileLayers, ResolvedSettings};
use serde::Deserialize;
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

const MAX_UPSTREAM_PROFILE_BYTES: usize = 1024 * 1024;
const MAX_UPSTREAM_HERO_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct UpstreamProfileSelection {
    #[serde(default)]
    hero: Option<UpstreamHeroSelection>,
}

#[derive(Deserialize)]
struct UpstreamHeroSelection {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    source: String,
}

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

#[derive(Clone)]
struct Enrollment {
    root: StateRoot,
    path: Cow<'static, str>,
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
                path: ryoku_profiles::LOCAL_PROFILE_FILE.into(),
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

    /// Exact first-sprint persistent Ryoku preferences: the Rust-owned profile
    /// selection, pinned shell settings and declared user preference files.
    /// Operational history, plugin-defined settings and every other XDG state
    /// path remain outside this inventory.
    pub fn session_preferences() -> Result<Self, InventoryError> {
        Self::compile(
            &[
                ryoku_profiles::PERSONAL_PROFILE_WRITER_ID,
                "upstream-shell-settings-v1",
                "upstream-matugen-settings-v1",
                "upstream-profile-selection-v1",
                "upstream-session-flags-v1",
                "upstream-desktop-preferences-v1",
                "upstream-capture-settings-v1",
                "upstream-camera-settings-v1",
                "upstream-audio-preferences-v1",
                "upstream-power-preferences-v1",
                "upstream-keypress-preferences-v1",
                "upstream-recording-settings-v1",
                "upstream-ryoshot-settings-v1",
                "upstream-device-control-preferences-v1",
                "upstream-stage-preferences-v1",
            ],
            &[
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: ryoku_profiles::LOCAL_PROFILE_FILE.into(),
                    writer: ryoku_profiles::PERSONAL_PROFILE_WRITER_ID,
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: ryoku_profiles::PERSONAL_PROFILE_MODES,
                    max_bytes: ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES as u64,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "shell.json".into(),
                    writer: "upstream-shell-settings-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    // The pinned writer has no file-size limit. Keep recovery
                    // bounded and fail capture when a user's settings exceed
                    // the supported inventory limit.
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "matugen.json".into(),
                    writer: "upstream-matugen-settings-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "profile.json".into(),
                    writer: "upstream-profile-selection-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    // Preserve the raw document; checkpoint capture parses only
                    // the hero selection to enroll one exact custom asset.
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "theme.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "brand.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "launcher.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "widgets.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "performance.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "recording.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "visualizer.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "stage.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "camera.json".into(),
                    writer: "upstream-camera-settings-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "ryoshot-beautify.json".into(),
                    writer: "upstream-desktop-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "ryoshot.json".into(),
                    writer: "upstream-ryoshot-settings-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "equalizer.json".into(),
                    writer: "upstream-audio-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "power.json".into(),
                    writer: "upstream-power-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "power-profile.json".into(),
                    writer: "upstream-power-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuConfig,
                    path: "keypresses.json".into(),
                    writer: "upstream-keypress-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 64 * 1024,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local".into(),
                    writer: "upstream-session-flags-v1",
                    owner: Owner::Account,
                    kind: Kind::Directory,
                    modes: &[0o700, 0o750, 0o755],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state".into(),
                    writer: "upstream-session-flags-v1",
                    owner: Owner::Account,
                    kind: Kind::Directory,
                    modes: &[0o700, 0o750, 0o755],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku".into(),
                    writer: "upstream-session-flags-v1",
                    owner: Owner::Account,
                    kind: Kind::Directory,
                    modes: &[0o700, 0o750, 0o755],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/flags.json".into(),
                    writer: "upstream-session-flags-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/capture.json".into(),
                    writer: "upstream-capture-settings-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/record.json".into(),
                    writer: "upstream-recording-settings-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/nvibrant-value".into(),
                    writer: "upstream-device-control-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 16,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/backlight".into(),
                    writer: "upstream-device-control-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 32,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/touchpad.disabled".into(),
                    writer: "upstream-device-control-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku-nightlight".into(),
                    writer: "upstream-device-control-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 16,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku-nightlight-enabled".into(),
                    writer: "upstream-device-control-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku-nightlight-schedule.json".into(),
                    writer: "upstream-device-control-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 4096,
                    absence_allowed: true,
                },
                Enrollment {
                    root: StateRoot::UserRyokuState,
                    path: ".local/state/ryoku/stage-walls.json".into(),
                    writer: "upstream-stage-preferences-v1",
                    owner: Owner::Account,
                    kind: Kind::Regular,
                    modes: &[0o600, 0o640, 0o644],
                    max_bytes: 1024 * 1024,
                    absence_allowed: true,
                },
            ],
            None,
        )
    }

    /// Borrow the fixed path set for one enrolled root. Callers may use these
    /// paths to create a descriptor-backed request, but cannot add entries to
    /// or mutate the compiled inventory.
    pub fn paths_for_root(&self, root: StateRoot) -> Option<&[RelativePath]> {
        self.paths.get(&root).map(Vec::as_slice)
    }

    /// Check whether a captured record's path and inode kind belong to this
    /// compiled inventory. Restore acceptance uses the same declaration as
    /// capture so newly enrolled preferences cannot be checkpointed but then
    /// silently refused by the restore preflight.
    pub fn accepts_record_kind(&self, record: &FileRecord) -> bool {
        let Some(enrollment) = self
            .entries
            .iter()
            .find(|entry| entry.root == record.root && entry.path.as_ref() == record.path.as_str())
        else {
            return false;
        };
        match &record.kind {
            EntryKind::Absent => enrollment.absence_allowed,
            EntryKind::RegularFile { .. } => matches!(enrollment.kind, Kind::Regular),
            EntryKind::Directory { .. } => matches!(enrollment.kind, Kind::Directory),
            EntryKind::Symlink { .. } => matches!(enrollment.kind, Kind::Symlink),
        }
    }

    fn include_selected_custom_hero(&mut self, source: &str) -> Result<String, InventoryError> {
        if !is_safe_custom_hero_source(source) {
            return Err(InventoryError::InvalidProfile);
        }
        let relative = format!("profile/{source}");
        let validated = RelativePath::new(&relative).map_err(|_| InventoryError::InvalidProfile)?;
        let directory_path = RelativePath::new("profile").expect("fixed profile directory");
        let writer = "upstream-profile-selection-v1";
        self.entries.push(Enrollment {
            root: StateRoot::UserRyokuConfig,
            path: Cow::Borrowed("profile"),
            writer,
            owner: Owner::Account,
            kind: Kind::Directory,
            modes: &[0o700, 0o750, 0o755],
            max_bytes: 4096,
            absence_allowed: false,
        });
        self.entries.push(Enrollment {
            root: StateRoot::UserRyokuConfig,
            path: Cow::Owned(relative.clone()),
            writer,
            owner: Owner::Account,
            kind: Kind::Regular,
            modes: &[0o600, 0o640, 0o644],
            max_bytes: MAX_UPSTREAM_HERO_BYTES,
            absence_allowed: false,
        });
        let paths = self.paths.entry(StateRoot::UserRyokuConfig).or_default();
        paths.push(directory_path);
        paths.push(validated);
        paths.sort();
        Ok(relative)
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
                (StateRoot::UserRyokuConfig | StateRoot::UserRyokuState, Owner::Account)
                    | (StateRoot::EtcRyoku, Owner::System)
            );
            if !valid_owner
                || entry.writer.is_empty()
                || entry.modes.is_empty()
                || entry.modes.iter().any(|mode| {
                    mode & !0o777 != 0
                        || (!matches!(entry.kind, Kind::Symlink) && mode & 0o022 != 0)
                })
                || entry.max_bytes == 0
                || !keys.insert((entry.root, entry.path.as_ref()))
            {
                return Err(InventoryError::InvalidEnrollment);
            }
            let path = RelativePath::new(entry.path.as_ref())
                .map_err(|_| InventoryError::InvalidEnrollment)?;
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
                    (entry.root, entry.path.as_ref()) == *key && matches!(entry.kind, Kind::Regular)
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
            self.entries.iter().map(|entry| (entry.root, entry.path.as_ref())).collect();
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

/// Exact identity and native root link for a manually requested snapshot of
/// the supported persistent Ryoku session preferences.
pub struct SessionPreferenceCheckpointRequest {
    pub transaction_id: Identifier,
    pub generation_id: Identifier,
    pub snapshot: crate::generation_capture::VerifiedSnapshotLink,
    pub created_utc: String,
    pub package_revisions: Vec<crate::PackageRevision>,
    pub reason: crate::OperationReason,
}

fn upstream_custom_hero_source(bytes: &[u8]) -> Option<String> {
    let profile: UpstreamProfileSelection = serde_json::from_slice(bytes).ok()?;
    let hero = profile.hero?;
    if hero.kind == "custom" { Some(hero.source) } else { None }
}

fn is_safe_custom_hero_source(source: &str) -> bool {
    let Some(extension) = source.strip_prefix("hero.") else { return false };
    !extension.is_empty()
        && extension.len() <= 250
        && extension.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'+' | b'-')
        })
}

fn read_upstream_profile_selection(
    root: &File,
    account: &CaptureAccount,
) -> Result<Option<String>, GenerationCaptureError> {
    let descriptor = match openat2(
        root,
        "profile.json",
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(descriptor) => File::from(descriptor),
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = fstat(&descriptor)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_nlink != 1
        || metadata.st_uid != account.uid
        || metadata.st_gid != account.gid
        || ![0o600, 0o640, 0o644].contains(&(metadata.st_mode & 0o7777))
        || metadata.st_size < 0
        || metadata.st_size as usize > MAX_UPSTREAM_PROFILE_BYTES
    {
        return Err(InventoryError::EntryPolicy.into());
    }
    let mut bytes = Vec::with_capacity(metadata.st_size as usize);
    descriptor.take((MAX_UPSTREAM_PROFILE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_UPSTREAM_PROFILE_BYTES {
        return Err(InventoryError::InvalidProfile.into());
    }
    Ok(upstream_custom_hero_source(&bytes))
}

/// Capture only the compiled first-sprint persistent session inventory. This
/// records raw current bytes (including a malformed personal profile so that
/// it can be recovered later), validates owner/type/mode against the fixed
/// catalogue, and seals the selected local account binding. Native snapshot
/// verification and writer exclusion remain caller responsibilities.
pub fn capture_session_preference_checkpoint(
    store: &GenerationStore,
    roots: InstalledRoots<'_>,
    request: SessionPreferenceCheckpointRequest,
    bundle: &crate::installed_profiles::VerifiedProfileBundle,
) -> Result<RecoveryManifest, GenerationCaptureError> {
    let mut inventory = InstalledInventory::session_preferences()?;
    let selected_hero = read_upstream_profile_selection(roots.user_configuration, &roots.account)?;
    let selected_hero_path = selected_hero
        .as_deref()
        .map(|source| inventory.include_selected_custom_hero(source))
        .transpose()?;
    bundle.revalidate().map_err(GenerationCaptureError::ProfileStore)?;
    if !bundle.bundle().matches_packages(&request.package_revisions) {
        return Err(GenerationCaptureError::ProfileStore(
            crate::installed_profiles::ProfileStoreError::PackageMismatch,
        ));
    }
    let mut declared = Vec::new();
    for root in [StateRoot::UserRyokuConfig, StateRoot::UserRyokuState] {
        let paths = inventory.paths_for_root(root).ok_or(InventoryError::IncompleteInventory)?;
        let descriptor = match root {
            StateRoot::UserRyokuConfig => roots.user_configuration,
            StateRoot::UserRyokuState => roots.user_home,
            _ => return Err(InventoryError::InvalidEnrollment.into()),
        };
        declared.push(DeclaredRoot { root, descriptor, paths });
    }
    let capture_request = GenerationCaptureRequest {
        transaction_id: request.transaction_id,
        generation_id: request.generation_id,
        parent_generation_id: None,
        snapshot: request.snapshot,
        reason: request.reason,
        created_utc: request.created_utc,
        package_revisions: request.package_revisions,
        provider_revision: None,
        profile_revision: None,
        roots: &declared,
        limits: crate::capture::CaptureLimits::default(),
    };
    capture_publish_verify_checked(
        store,
        capture_request,
        capture_inventory,
        |store, bytes, files| store.ensure_available_space(bytes, files),
        |records, payloads, _| {
            inventory.validate_records(records, &roots.account)?;
            let upstream_profile = records
                .iter()
                .find(|record| {
                    record.root == StateRoot::UserRyokuConfig
                        && record.path.as_str() == "profile.json"
                })
                .ok_or(InventoryError::IncompleteInventory)?;
            let captured_hero = match &upstream_profile.kind {
                EntryKind::Absent => None,
                EntryKind::RegularFile { sha256, size, .. } => {
                    let bytes = payloads.get(sha256).ok_or(InventoryError::InvalidProfile)?;
                    if bytes.len() as u64 != *size || digest(bytes) != sha256.as_str() {
                        return Err(InventoryError::InvalidProfile.into());
                    }
                    upstream_custom_hero_source(bytes)
                }
                _ => return Err(InventoryError::InvalidProfile.into()),
            };
            if let Some(source) = captured_hero.as_deref() {
                if !is_safe_custom_hero_source(source)
                    || selected_hero_path.as_deref() != Some(format!("profile/{source}").as_str())
                {
                    return Err(InventoryError::InvalidProfile.into());
                }
            } else if selected_hero_path.is_some() {
                return Err(InventoryError::InvalidProfile.into());
            }
            let personal = records
                .iter()
                .find(|record| {
                    record.root == StateRoot::UserRyokuConfig
                        && record.path.as_str() == ryoku_profiles::LOCAL_PROFILE_FILE
                })
                .ok_or(InventoryError::IncompleteInventory)?;
            let profile_revision = match &personal.kind {
                EntryKind::Absent => Some(
                    bundle
                        .bundle()
                        .resolve(None)
                        .map_err(GenerationCaptureError::ProfileStore)?
                        .provenance,
                ),
                EntryKind::RegularFile { sha256, size, .. } => {
                    let bytes = payloads.get(sha256).ok_or(InventoryError::InvalidProfile)?;
                    if bytes.len() as u64 != *size || digest(bytes) != sha256.as_str() {
                        return Err(InventoryError::InvalidProfile.into());
                    }
                    match bundle.bundle().resolve(Some(bytes)) {
                        Ok(profile) => Some(profile.provenance),
                        // Preserve malformed user bytes in the generation so
                        // they can be restored, but do not claim an effective
                        // profile revision that cannot be resolved offline.
                        Err(crate::installed_profiles::ProfileStoreError::InvalidDocument) => None,
                        Err(error) => {
                            return Err(GenerationCaptureError::ProfileStore(error));
                        }
                    }
                }
                _ => return Err(InventoryError::InvalidProfile.into()),
            };
            Ok(crate::generation_capture::CaptureProvenance {
                profile_revision,
                account_principal: Some(roots.account.principal.clone()),
            })
        },
    )
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
    /// Pinned NSS home descriptor used only for exact state-root paths.
    pub user_home: &'a File,
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
            StateRoot::UserRyokuState => (roots.user_home, roots.account.uid, roots.account.gid),
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
                Some(
                    bundle
                        .bundle()
                        .resolve(personal)
                        .map_err(GenerationCaptureError::ProfileStore)?
                        .provenance,
                )
            } else if inventory.profiles.is_some() {
                Some(inventory.profile(records, payloads)?.provenance)
            } else if inventory.entries.iter().any(|entry| {
                entry.root == StateRoot::UserRyokuConfig
                    && entry.path.as_ref() == ryoku_profiles::LOCAL_PROFILE_FILE
            }) {
                return Err(InventoryError::MissingProfileDefaults.into());
            } else {
                None
            };
            Ok(crate::generation_capture::CaptureProvenance {
                profile_revision: provenance,
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
