//! Combined preference/asset restore planning for #175. A preference subset is
//! kept private so it cannot be passed to the legacy writer and mistaken for
//! complete recovery. Preparation is read-only; native authorization, snapshot
//! verification and exclusive writer ownership remain caller prerequisites.
use crate::{
    AccountPrincipal, EntryKind, Identifier, Sha256,
    asset_restore::{FileRestoreIntent, FileRestorePlan, RestoreError},
    configuration_stage::{StageError, StagedConfiguration, stage_preference_subset},
    generation::VerifiedGeneration,
    installed_capture::CaptureAccount,
    restore::{
        ArtifactStatus, PreflightError, PreflightEvidence, RestoreScope,
        build_preference_subset_plan,
    },
    ryogami_assets::{AssetError, AssetScope, MAX_CAPTURE_LIMITS},
};
use std::{collections::BTreeSet, fs::File};

#[derive(Debug)]
pub enum CombinedError {
    Binding,
    Record,
    Apply,
    Interrupted,
    UnsupportedAssetKind,
    Coverage,
    Asset(AssetError),
    Restore(RestoreError),
    Preflight(PreflightError),
    Stage(StageError),
}
impl std::fmt::Display for CombinedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Record => "combined restore durable record is unavailable or changed",
            Self::Apply => "combined restore preference application failed",
            Self::Interrupted => "combined restore interrupted with durable recovery retained",
            Self::Binding => "combined restore identity or account changed",
            Self::UnsupportedAssetKind => "combined restore requires a supported exact asset kind",
            Self::Coverage => "combined restore checkpoint omits declared preferences or assets",
            Self::Asset(_) => "combined restore asset scope is invalid",
            Self::Restore(_) => "combined restore live asset preparation failed",
            Self::Preflight(_) => "combined restore preflight failed",
            Self::Stage(_) => "combined restore preference preparation failed",
        })
    }
}
impl std::error::Error for CombinedError {}
type Result<T> = std::result::Result<T, CombinedError>;

/// All restore-generated file names are derived before the fresh checkpoint.
/// No client-provided root, prepared plan or checkpoint evidence is accepted.
pub struct CombinedRestoreIntent {
    operation: Identifier,
    source_manifest: Sha256,
    account: AccountPrincipal,
    assets: Vec<FileRestoreIntent>,
    scope: AssetScope,
}
impl CombinedRestoreIntent {
    pub fn new(operation: Identifier, source: &VerifiedGeneration) -> Result<Self> {
        let manifest = source.manifest();
        let inventory = manifest.assets().ok_or(CombinedError::Binding)?;
        if manifest.root_snapshot_receipt().is_none() || inventory.entries().is_empty() {
            return Err(CombinedError::Binding);
        }
        let account = manifest.account_principal().cloned().ok_or(CombinedError::Binding)?;
        let targets: BTreeSet<_> =
            inventory.entries().iter().map(|entry| entry.path.clone()).collect();
        let mut stages = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut assets = Vec::new();
        for (index, entry) in inventory.entries().iter().enumerate() {
            if !matches!(entry.kind, EntryKind::RegularFile { .. } | EntryKind::Absent) {
                return Err(CombinedError::UnsupportedAssetKind);
            }
            let item = Identifier::new(format!(
                "asset-{}-{index}",
                &crate::wallpaper_protocol::digest(operation.as_str().as_bytes())[..32]
            ))
            .map_err(|_| CombinedError::Binding)?;
            let intent = FileRestoreIntent::new(item, source, &entry.path)
                .map_err(CombinedError::Restore)?;
            if targets.contains(intent.staging()) || !stages.insert(intent.staging().clone()) {
                return Err(CombinedError::Coverage);
            }
            paths.extend(intent.scope().map_err(CombinedError::Restore)?.paths().iter().cloned());
            assets.push(intent);
        }
        let scope = AssetScope::new(
            paths.into_iter().collect(),
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
        )
        .map_err(CombinedError::Asset)?;
        Ok(Self {
            operation,
            source_manifest: manifest.manifest_sha256.clone().ok_or(CombinedError::Binding)?,
            account,
            assets,
            scope,
        })
    }
    pub fn scope(&self) -> &AssetScope {
        &self.scope
    }
    pub fn operation(&self) -> &Identifier {
        &self.operation
    }
}

pub struct CombinedPreparation<'a> {
    pub source: &'a VerifiedGeneration,
    pub checkpoint: &'a VerifiedGeneration,
    pub home: &'a File,
    pub account: &'a CaptureAccount,
    pub scope: RestoreScope,
    pub evidence: &'a PreflightEvidence,
}

pub struct PreparedCombinedRestore {
    intent: CombinedRestoreIntent,
    checkpoint_manifest: Sha256,
    preferences: StagedConfiguration,
    assets: Vec<FileRestorePlan>,
}
impl PreparedCombinedRestore {
    pub fn prepare(
        intent: CombinedRestoreIntent,
        request: CombinedPreparation<'_>,
    ) -> Result<Self> {
        let source = request.source.manifest();
        let checkpoint = request.checkpoint.manifest();
        if request.scope == RestoreScope::SystemOnly
            || source.manifest_sha256.as_ref() != Some(&intent.source_manifest)
            || request.account.principal != intent.account
            || checkpoint.account_principal() != Some(&intent.account)
            || checkpoint.root_snapshot_receipt().is_none()
            || checkpoint.transaction_id == source.transaction_id
            || checkpoint.generation_id == source.generation_id
            || checkpoint.snapper_config != source.snapper_config
            || checkpoint.filesystem_uuid != source.filesystem_uuid
            || checkpoint.snapper_snapshot_id == source.snapper_snapshot_id
            || request.evidence.current_state_checkpoint != ArtifactStatus::Verified
        {
            return Err(CombinedError::Binding);
        }
        let before: BTreeSet<_> =
            checkpoint.files().iter().map(|entry| (entry.root, &entry.path)).collect();
        if source
            .files()
            .iter()
            .filter(|entry| entry.root.is_configuration_state())
            .any(|entry| !before.contains(&(entry.root, &entry.path)))
        {
            return Err(CombinedError::Coverage);
        }
        let covered: BTreeSet<_> = checkpoint
            .assets()
            .ok_or(CombinedError::Coverage)?
            .entries()
            .iter()
            .map(|entry| &entry.path)
            .collect();
        if intent.scope.paths().iter().any(|path| !covered.contains(path)) {
            return Err(CombinedError::Coverage);
        }
        let facts = request.evidence;
        let plan = build_preference_subset_plan(
            source,
            request.scope,
            facts.generation,
            facts.archive,
            &facts.system_snapshot,
            facts.system_compatibility,
            facts.configuration_compatibility,
        )
        .map_err(CombinedError::Preflight)?;
        let preferences =
            stage_preference_subset(request.source, &plan).map_err(CombinedError::Stage)?;
        let assets = intent
            .assets
            .iter()
            .map(|asset| {
                FileRestorePlan::prepare(
                    asset,
                    request.source,
                    request.checkpoint,
                    request.home,
                    request.account,
                )
                .map_err(CombinedError::Restore)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            intent,
            checkpoint_manifest: checkpoint
                .manifest_sha256
                .clone()
                .ok_or(CombinedError::Binding)?,
            preferences,
            assets,
        })
    }
    pub fn preference_entries(&self) -> usize {
        self.preferences.entries().len()
    }
    pub fn asset_entries(&self) -> usize {
        self.assets.len()
    }
    pub fn source_manifest(&self) -> &Sha256 {
        &self.intent.source_manifest
    }
    pub fn checkpoint_manifest(&self) -> &Sha256 {
        &self.checkpoint_manifest
    }
}

#[path = "combined_restore_apply.rs"]
mod apply;
pub use apply::{CombinedExecution, CombinedStores};
