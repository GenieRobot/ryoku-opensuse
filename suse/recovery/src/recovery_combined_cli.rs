//! #175 asset-bearing branches of the installed recovery console. The parent
//! command authenticates root/offline access, validates the state mount and
//! retains its exclusive mutation guard throughout these calls.
use super::*;
use ryoku_recovery::{
    combined_restore::{
        CombinedExecution, CombinedPreparation, CombinedRestoreIntent, CombinedStores,
        PreparedCombinedRestore,
    },
    generation::VerifiedGeneration,
    restore::{ArtifactStatus, PreflightEvidence},
};

pub(super) struct Prepare<'a> {
    pub root: &'a File,
    pub state: &'a File,
    pub generations: &'a GenerationStore,
    pub reader: &'a GenerationReader,
    pub journals: &'a RestoreJournalStore,
    pub source: &'a VerifiedGeneration,
    pub account: &'a LocalAccount,
    pub restore_id: &'a Identifier,
    pub scope: RestoreScope,
    pub compatibility: RestoreCompatibility,
    pub installed: &'a [PackageRevision],
    pub existing: Option<RestoreJournal>,
}
fn evidence(compatibility: &RestoreCompatibility) -> PreflightEvidence {
    PreflightEvidence {
        generation: ArtifactStatus::Verified,
        archive: ArtifactStatus::Verified,
        current_state_checkpoint: ArtifactStatus::Verified,
        system_snapshot: compatibility.source_snapshot.clone(),
        system_compatibility: compatibility.system,
        configuration_compatibility: compatibility.configuration,
    }
}
fn stage_store(state: &File) -> Result<DiskStageStore, String> {
    let staging = File::from(
        openat2(
            state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    DiskStageStore::open(&staging).map_err(|error| error.to_string())
}
fn capture_account(
    source: &VerifiedGeneration,
    account: &LocalAccount,
) -> Result<CaptureAccount, String> {
    Ok(CaptureAccount {
        uid: account.uid,
        gid: account.gid,
        principal: source
            .manifest()
            .account_principal()
            .cloned()
            .ok_or("asset source has no account binding")?,
    })
}

pub(super) fn prepare(context: Prepare<'_>) -> Result<String, String> {
    let account = capture_account(context.source, context.account)?;
    stop_account_writers(account.uid)?;
    let checkpoint = if let Some(journal) = &context.existing {
        let checkpoint = context
            .generations
            .open_verified_transaction(context.restore_id)
            .map_err(|error| error.to_string())?;
        if checkpoint.manifest().generation_id != journal.current_state_generation
            || checkpoint.manifest().manifest_sha256.as_ref()
                != Some(&journal.current_state_manifest_sha256)
        {
            return Err("retained asset checkpoint differs from preparation journal".into());
        }
        context
            .reader
            .open_historical_transaction(context.root, context.restore_id)
            .map_err(|error| error.to_string())?;
        checkpoint
    } else {
        let intent = CombinedRestoreIntent::discover(
            context.restore_id.clone(),
            context.source,
            &context.account.home,
            &account,
        )
        .map_err(|error| error.to_string())?;
        // Read-only exact enrollment validation precedes native snapshot work.
        intent
            .scope()
            .capture(&context.account.home, &account)
            .map_err(|error| error.to_string())?;
        let inventory = InstalledInventory::for_home(&context.account.home)
            .map_err(|error| error.to_string())?;
        let profiles = ryoku_recovery::installed_profiles::ProfileBundleStore::open_state_mount()
            .map_err(|error| error.to_string())?;
        let bundle =
            ryoku_recovery::installed_profiles::InstalledProfileBundle::load_packaged_defaults(
                None, None,
            )
            .map_err(|error| error.to_string())?;
        if !bundle.matches_packages(context.installed) {
            return Err("installed profile defaults differ from package inventory".into());
        }
        let request = ryoku_snapper::SnapshotPairRequest {
            config: ConfigName::new("root").map_err(|error| format!("{error:?}"))?,
            transaction_id: TransactionId::new(context.restore_id.as_str())
                .map_err(|error| format!("{error:?}"))?,
        };
        let snapshot=ryoku_snapper::SnapperCheckpointBackend.create_single(&request).map_err(|error|format!("asset restore checkpoint failed; retain transaction tag for reconciliation: {error}"))?;
        let link = SnapshotLink::new(
            Identifier::new("root").expect("fixed ID"),
            snapshot.as_str(),
            context.source.manifest().filesystem_uuid.clone(),
        )
        .map_err(|error| format!("{error:?}"))?;
        let native =
            verify_new_root_checkpoint(context.root, link).map_err(|error| error.to_string())?;
        let bundle = profiles
            .publish_candidate(context.restore_id, &bundle)
            .map_err(|error| error.to_string())?;
        ryoku_recovery::installed_capture::capture_with_installed_profile_bundle_assets(
            context.generations,&inventory,InstalledRoots{user_configuration:&context.account.home,account_home:Some(&context.account.home),system_configuration:None,account:account.clone()},
            InstalledCaptureRequest{transaction_id:context.restore_id.clone(),generation_id:generation_id_for(context.restore_id),parent_generation_id:None,snapshot:native.verified_link().clone(),reason:ryoku_recovery::OperationReason::ConfigurationRestore,created_utc:current_utc_timestamp()?,package_revisions:context.installed.to_vec(),provider_revision:None,limits:Default::default()},
            &bundle,intent.scope()).map_err(|error|format!("asset checkpoint capture failed; retain native snapshot for reconciliation: {error}"))?;
        context
            .generations
            .open_verified_transaction(context.restore_id)
            .map_err(|error| error.to_string())?
    };
    let intent = CombinedRestoreIntent::from_checkpoint(
        context.restore_id.clone(),
        context.source,
        &checkpoint,
        &account,
    )
    .map_err(|error| error.to_string())?;
    ensure_cataloged_checkpoint(
        context.root,
        context.generations,
        context.reader,
        checkpoint.manifest(),
    )?;
    let roots =
        open_restore_roots(context.root, &[context.source.manifest(), checkpoint.manifest()])?;
    let declared: Vec<_> = roots
        .iter()
        .map(|root| DeclaredRoot {
            root: root.root,
            descriptor: &root.directory,
            paths: &root.paths,
        })
        .collect();
    let destinations: Vec<_> = roots
        .iter()
        .map(|root| ConfigurationDestination { root: root.root, directory: &root.directory })
        .collect();
    let stages = stage_store(context.state)?;
    let stores =
        CombinedStores { stages: &stages, journals: context.journals, assets: context.state };
    let facts = evidence(&context.compatibility);
    let prepared = PreparedCombinedRestore::prepare(
        intent,
        CombinedPreparation {
            source: context.source,
            checkpoint: &checkpoint,
            home: &context.account.home,
            account: &account,
            scope: context.scope,
            evidence: &facts,
        },
    )
    .map_err(|error| error.to_string())?;
    let execution = CombinedExecution {
        source: context.source,
        checkpoint: &checkpoint,
        home: &context.account.home,
        account: &account,
        destinations: &destinations,
        checkpoint_roots: &declared,
    };
    let journal = if context.existing.is_some() {
        prepared.resume_preparation(&stores, &execution)
    } else {
        prepared.begin(&stores, &execution)
    }
    .map_err(|error| error.to_string())?;
    if revisions_for_source(None, context.source.manifest())? != context.installed {
        return Err(
            "packages changed during combined preparation; retained records require reconciliation"
                .into(),
        );
    }
    Ok(format!(
        "combined-restore-prepared attempt={} source={} scope={:?} phase={:?} preferences={} assets={}; continue with the offline restore command",
        journal.restore_id.as_str(),
        context.source.manifest().transaction_id.as_str(),
        journal.plan.scope,
        journal.phase,
        prepared.preference_entries(),
        prepared.asset_entries()
    ))
}

pub(super) struct Replay<'a> {
    pub root: &'a File,
    pub state: &'a File,
    pub generations: &'a GenerationStore,
    pub reader: &'a GenerationReader,
    pub journals: &'a RestoreJournalStore,
    pub source: &'a VerifiedGeneration,
    pub checkpoint: &'a VerifiedGeneration,
    pub journal: &'a RestoreJournal,
    pub compatibility: &'a RestoreCompatibility,
}
struct Parts {
    account: LocalAccount,
    capture: CaptureAccount,
    roots: Vec<RestoreRoot>,
    stages: DiskStageStore,
    facts: PreflightEvidence,
}
fn parts(context: &Replay<'_>) -> Result<Parts, String> {
    let account = resolve_local_account(
        context.root,
        context
            .source
            .manifest()
            .account_principal()
            .ok_or("asset source has no account binding")?,
    )?;
    let capture = capture_account(context.source, &account)?;
    let roots = open_restore_roots(
        context.root,
        &[context.source.manifest(), context.checkpoint.manifest()],
    )?;
    Ok(Parts {
        account,
        capture,
        roots,
        stages: stage_store(context.state)?,
        facts: evidence(context.compatibility),
    })
}

pub(super) fn restore(context: Replay<'_>) -> Result<String, String> {
    let parts = parts(&context)?;
    let stores =
        CombinedStores { stages: &parts.stages, journals: context.journals, assets: context.state };
    let declared: Vec<_> = parts
        .roots
        .iter()
        .map(|root| DeclaredRoot {
            root: root.root,
            descriptor: &root.directory,
            paths: &root.paths,
        })
        .collect();
    let destinations: Vec<_> = parts
        .roots
        .iter()
        .map(|root| ConfigurationDestination { root: root.root, directory: &root.directory })
        .collect();
    let prepared = PreparedCombinedRestore::reopen(
        context.journal.restore_id.clone(),
        &stores,
        CombinedPreparation {
            source: context.source,
            checkpoint: context.checkpoint,
            home: &parts.account.home,
            account: &parts.capture,
            scope: context.journal.plan.scope,
            evidence: &parts.facts,
        },
    )
    .map_err(|error| error.to_string())?;
    stop_journal_account_writers(
        context.root,
        context.generations,
        &context.journal.restore_id,
        &context.journal.current_state_generation,
    )?;
    let mut journal =
        context.journals.load(&context.journal.restore_id).map_err(|error| error.to_string())?;
    if journal.plan.scope == RestoreScope::ConfigurationOnly
        && journal.phase == RestorePhase::ContentStaged
    {
        context
            .journals
            .advance(&mut journal, RestorePhase::WritersStopped)
            .map_err(|error| error.to_string())?;
    }
    journal = prepared
        .apply(
            &stores,
            &CombinedExecution {
                source: context.source,
                checkpoint: context.checkpoint,
                home: &parts.account.home,
                account: &parts.capture,
                destinations: &destinations,
                checkpoint_roots: &declared,
            },
            || true,
        )
        .map_err(|error| error.to_string())?;
    let health_files: Vec<_> = context
        .source
        .manifest()
        .files()
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::RegularFile { size, .. } if entry.path.as_str().ends_with(".json") => {
                Some((entry.root, entry.path.clone(), *size as usize))
            }
            _ => None,
        })
        .collect();
    if matches!(journal.phase, RestorePhase::ConfigurationRestored | RestorePhase::HealthChecked) {
        verify_configuration_restore_health(&health_files, &parts.roots, verify_systemd_health)?;
        if journal.phase == RestorePhase::ConfigurationRestored {
            context
                .journals
                .advance(&mut journal, RestorePhase::HealthChecked)
                .map_err(|error| error.to_string())?;
        }
        context
            .journals
            .advance(&mut journal, RestorePhase::Complete)
            .map_err(|error| error.to_string())?;
    }
    let known = record_known_working_after_complete_linked_restore(
        context.root,
        context.generations,
        context.reader,
        &journal,
    )?;
    Ok(format!(
        "combined-configuration-restored attempt={} phase={:?} known_working={}; preferences and exact assets verified; reboot to resume normal sessions",
        journal.restore_id.as_str(),
        journal.phase,
        known_working_label(known)
    ))
}

pub(super) fn rollback(context: Replay<'_>) -> Result<String, String> {
    let parts = parts(&context)?;
    let stores =
        CombinedStores { stages: &parts.stages, journals: context.journals, assets: context.state };
    stop_journal_account_writers(
        context.root,
        context.generations,
        &context.journal.restore_id,
        &context.journal.current_state_generation,
    )?;
    // Re-run preparation validation before root rollback while live targets
    // still match the retained current-state checkpoint.
    let intent = CombinedRestoreIntent::from_checkpoint(
        context.journal.restore_id.clone(),
        context.source,
        context.checkpoint,
        &parts.capture,
    )
    .map_err(|error| error.to_string())?;
    let prepared = PreparedCombinedRestore::prepare(
        intent,
        CombinedPreparation {
            source: context.source,
            checkpoint: context.checkpoint,
            home: &parts.account.home,
            account: &parts.capture,
            scope: RestoreScope::CompleteLinked,
            evidence: &parts.facts,
        },
    )
    .map_err(|error| error.to_string())?;
    let declared: Vec<_> = parts
        .roots
        .iter()
        .map(|root| DeclaredRoot {
            root: root.root,
            descriptor: &root.directory,
            paths: &root.paths,
        })
        .collect();
    let destinations: Vec<_> = parts
        .roots
        .iter()
        .map(|root| ConfigurationDestination { root: root.root, directory: &root.directory })
        .collect();
    prepared
        .resume_preparation(
            &stores,
            &CombinedExecution {
                source: context.source,
                checkpoint: context.checkpoint,
                home: &parts.account.home,
                account: &parts.capture,
                destinations: &destinations,
                checkpoint_roots: &declared,
            },
        )
        .map_err(|error| error.to_string())?;
    let native = PreparationStores {
        generations: context.generations,
        journals: context.journals,
        stages: &parts.stages,
    };
    let handoff = native
        .execute_combined_root_rollback(
            &prepared,
            || {
                stop_journal_account_writers(
                    context.root,
                    context.generations,
                    &context.journal.restore_id,
                    &context.journal.current_state_generation,
                )
            },
            &mut ryoku_snapper::SnapperRollbackBackend,
        )
        .map_err(|error| error.to_string())?;
    request_recorded_rollback_reboot(handoff.journal().restore_id.as_str(), "linked")?;
    Ok(format!(
        "linked-asset-rollback-requested attempt={}; after boot run confirm-rollback, then restore-configuration; recovery remains active",
        handoff.journal().restore_id.as_str()
    ))
}

pub(super) fn discard(
    root: &File,
    state: &File,
    generations: &GenerationStore,
    journals: &RestoreJournalStore,
    restore_id: &Identifier,
) -> Result<String, String> {
    let journal = journals.load(restore_id).map_err(|error| error.to_string())?;
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    let checkpoint =
        generations.open_verified_transaction(restore_id).map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    reader
        .open_historical_transaction(root, &journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    reader.open_historical_transaction(root, restore_id).map_err(|error| error.to_string())?;
    verify_profile_source(&source)?;
    let compatibility = if journal.plan.scope == RestoreScope::CompleteLinked {
        let mut result = verify_system_source_compatibility(root, &reader, &source)?;
        result.configuration = CompatibilityStatus::Compatible;
        result
    } else {
        RestoreCompatibility {
            source_snapshot: SystemSnapshotStatus::Missing,
            system: CompatibilityStatus::Unknown,
            configuration: CompatibilityStatus::Compatible,
        }
    };
    let context = Replay {
        root,
        state,
        generations,
        reader: &reader,
        journals,
        source: &source,
        checkpoint: &checkpoint,
        journal: &journal,
        compatibility: &compatibility,
    };
    let parts = parts(&context)?;
    stop_journal_account_writers(root, generations, restore_id, &journal.current_state_generation)?;
    if !matches!(
        journal.phase,
        RestorePhase::Planned | RestorePhase::CheckpointVerified | RestorePhase::ContentStaged
    ) {
        return cleanup_live(context, parts);
    }
    let stores = CombinedStores { stages: &parts.stages, journals, assets: state };
    let declared: Vec<_> = parts
        .roots
        .iter()
        .map(|root| DeclaredRoot {
            root: root.root,
            descriptor: &root.directory,
            paths: &root.paths,
        })
        .collect();
    let destinations: Vec<_> = parts
        .roots
        .iter()
        .map(|root| ConfigurationDestination { root: root.root, directory: &root.directory })
        .collect();
    let prepared = PreparedCombinedRestore::prepare(
        CombinedRestoreIntent::from_checkpoint(
            restore_id.clone(),
            &source,
            &checkpoint,
            &parts.capture,
        )
        .map_err(|error| error.to_string())?,
        CombinedPreparation {
            source: &source,
            checkpoint: &checkpoint,
            home: &parts.account.home,
            account: &parts.capture,
            scope: journal.plan.scope,
            evidence: &parts.facts,
        },
    )
    .map_err(|error| error.to_string())?;
    prepared
        .discard_incomplete_preparation(
            &stores,
            &CombinedExecution {
                source: &source,
                checkpoint: &checkpoint,
                home: &parts.account.home,
                account: &parts.capture,
                destinations: &destinations,
                checkpoint_roots: &declared,
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "incomplete-combined-preparation-discarded attempt={}; checkpoint and live preferences/assets retained; resume preparation",
        restore_id.as_str()
    ))
}

fn cleanup_live(context: Replay<'_>, parts: Parts) -> Result<String, String> {
    let allowed = matches!(
        (context.journal.plan.scope, context.journal.phase),
        (RestoreScope::ConfigurationOnly, RestorePhase::WritersStopped)
            | (RestoreScope::CompleteLinked, RestorePhase::RootRollbackComplete)
    );
    if !allowed {
        return Err("asset cleanup is not in an interrupted apply phase".into());
    }
    if context.journal.plan.scope == RestoreScope::CompleteLinked {
        let receipt = context
            .source
            .manifest()
            .root_snapshot_receipt()
            .ok_or("missing native source receipt")?;
        ryoku_recovery::native_checkpoint::verify_running_root_rollback(
            context.root,
            context.journal,
            receipt,
        )
        .map_err(|error| error.to_string())?;
    }
    let stores =
        CombinedStores { stages: &parts.stages, journals: context.journals, assets: context.state };
    let prepared = PreparedCombinedRestore::reopen(
        context.journal.restore_id.clone(),
        &stores,
        CombinedPreparation {
            source: context.source,
            checkpoint: context.checkpoint,
            home: &parts.account.home,
            account: &parts.capture,
            scope: context.journal.plan.scope,
            evidence: &parts.facts,
        },
    )
    .map_err(|error| error.to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "cleanup clock unavailable")?
        .as_nanos();
    let operation = Identifier::new(format!(
        "asset-cleanup-{}-{nanos}",
        &ryoku_recovery::wallpaper_protocol::digest(context.journal.restore_id.as_str().as_bytes())
            [..24]
    ))
    .map_err(|error| format!("{error:?}"))?;
    let cleanup = prepared
        .partial_cleanup(operation.clone(), context.checkpoint, &parts.account.home, &parts.capture)
        .map_err(|error| error.to_string())?;
    // Capture before deletion, then explicitly pin the recoverable partials.
    // This uses the catalog's existing operator-checkpoint retention policy.
    cleanup
        .scope()
        .capture(&parts.account.home, &parts.capture)
        .map_err(|error| error.to_string())?;
    let installed = installed_ryoku_revisions()?;
    let inventory =
        InstalledInventory::for_home(&parts.account.home).map_err(|error| error.to_string())?;
    let profiles = ryoku_recovery::installed_profiles::ProfileBundleStore::open_state_mount()
        .map_err(|error| error.to_string())?;
    let bundle =
        ryoku_recovery::installed_profiles::InstalledProfileBundle::load_packaged_defaults(
            None, None,
        )
        .map_err(|error| error.to_string())?;
    if !bundle.matches_packages(&installed) {
        return Err("cleanup profile defaults differ from installed packages".into());
    }
    let request = ryoku_snapper::SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|error| format!("{error:?}"))?,
        transaction_id: TransactionId::new(operation.as_str())
            .map_err(|error| format!("{error:?}"))?,
    };
    let snapshot =
        ryoku_snapper::SnapperCheckpointBackend.create_single(&request).map_err(|error| {
            format!("cleanup native checkpoint failed; retain tag {}: {error}", operation.as_str())
        })?;
    let link = SnapshotLink::new(
        Identifier::new("root").expect("fixed ID"),
        snapshot.as_str(),
        context.source.manifest().filesystem_uuid.clone(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let native =
        verify_new_root_checkpoint(context.root, link).map_err(|error| error.to_string())?;
    let bundle =
        profiles.publish_candidate(&operation, &bundle).map_err(|error| error.to_string())?;
    let manifest = ryoku_recovery::installed_capture::capture_with_installed_profile_bundle_assets(
        context.generations,
        &inventory,
        InstalledRoots {
            user_configuration: &parts.account.home,
            account_home: Some(&parts.account.home),
            system_configuration: None,
            account: parts.capture.clone(),
        },
        InstalledCaptureRequest {
            transaction_id: operation.clone(),
            generation_id: generation_id_for(&operation),
            parent_generation_id: Some(context.checkpoint.manifest().generation_id.clone()),
            snapshot: native.verified_link().clone(),
            reason: ryoku_recovery::OperationReason::ConfigurationRestore,
            created_utc: current_utc_timestamp()?,
            package_revisions: installed.clone(),
            provider_revision: None,
            limits: Default::default(),
        },
        &bundle,
        cleanup.scope(),
    )
    .map_err(|error| {
        format!(
            "cleanup checkpoint {} capture failed; preserve native partner: {error}",
            operation.as_str()
        )
    })?;
    ensure_cataloged_checkpoint(context.root, context.generations, context.reader, &manifest)?;
    ProtectedSnapperCatalog::open(context.root)
        .and_then(|catalog| catalog.mark_manual(&operation))
        .map_err(|error| {
            format!("cleanup checkpoint could not be pinned; no partials deleted: {error}")
        })?;
    let fresh = context
        .generations
        .open_verified_transaction(&operation)
        .map_err(|error| error.to_string())?;
    context
        .reader
        .open_historical_transaction(context.root, &operation)
        .map_err(|error| error.to_string())?;
    if installed_ryoku_revisions()? != installed {
        return Err("packages changed during cleanup capture; pinned checkpoint retained".into());
    }
    let count = cleanup
        .apply(
            context.checkpoint,
            &fresh,
            &parts.account.home,
            &parts.capture,
            context.state,
            || true,
        )
        .map_err(|error| {
            format!(
                "cleanup interrupted; pinned checkpoint {} retained: {error}",
                operation.as_str()
            )
        })?;
    Ok(format!(
        "partial-asset-stages-cleaned attempt={} count={} retained_checkpoint={}; restore phase remains pending; resume restore-configuration",
        context.journal.restore_id.as_str(),
        count,
        operation.as_str()
    ))
}
