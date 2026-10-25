use std::{
    collections::BTreeSet,
    env,
    fs::File,
    io::{self, Read, Write},
    process::{Command, ExitCode, Output, Stdio},
    thread,
};

use rustix::{
    fs::{FileType, FlockOperation, Mode, OFlags, ResolveFlags, flock, fstat, openat2},
    io::Errno,
};
use ryoku_recovery::{
    AccountPrincipal, EntryKind, FileRecord, Identifier, PackageName, PackageRevision,
    RecoveryManifest, RelativePath, Revision, SnapshotLink, StateRoot,
    configuration_apply::ConfigurationDestination,
    disk_stage::DiskStageStore,
    generation::{GenerationReader, GenerationStore},
    generation_capture::DeclaredRoot,
    installed_capture::{
        CaptureAccount, InstalledCaptureRequest, InstalledInventory, InstalledRoots,
        capture_with_installed_profile_bundle,
    },
    machine_state::read_machine_state_record,
    native_checkpoint::verify_new_root_checkpoint,
    preparation::{
        ConfigurationPreparationRequest, PreparationStores, RestoreCompatibility,
        SystemPreparationRequest,
    },
    restore::{
        CompatibilityStatus, JournalError, RestoreJournal, RestoreJournalStore, RestorePhase,
        RestoreScope, SystemSnapshotStatus,
    },
    snapper_catalog::ProtectedSnapperCatalog,
    state_mount::collect_and_open_state_mount,
};
use ryoku_snapper::{ConfigName, LayoutCapability, SnapshotId, TransactionId};

#[path = "../recovery_combined_cli.rs"]
mod recovery_combined_cli;

const USAGE: &str = "Usage:\n  ryoku-recovery admit-session\n  ryoku-recovery layout\n  ryoku-recovery list\n  ryoku-recovery pending\n  ryoku-recovery inspect TRANSACTION_ID\n  ryoku-recovery verify TRANSACTION_ID\n  ryoku-recovery verify-linked TRANSACTION_ID\n  ryoku-recovery plan TRANSACTION_ID configuration-only|system-only|complete-linked\n  sudo ryoku-recovery manual-checkpoint TRANSACTION_ID USERNAME\n  sudo ryoku-recovery prepare-configuration RESTORE_ID SOURCE_TRANSACTION_ID\n  sudo ryoku-recovery discard-incomplete-stage RESTORE_ID\n  sudo ryoku-recovery prepare-system RESTORE_ID SOURCE_TRANSACTION_ID\n  sudo ryoku-recovery rollback-system RESTORE_ID\n  sudo ryoku-recovery prepare-linked RESTORE_ID SOURCE_TRANSACTION_ID\n  sudo ryoku-recovery rollback-linked RESTORE_ID\n  sudo ryoku-recovery confirm-rollback RESTORE_ID\n  sudo ryoku-recovery restore-configuration RESTORE_ID\n  sudo ryoku-recovery complete-system RESTORE_ID\n";

fn main() -> ExitCode {
    match execute(env::args().skip(1), &mut io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "ryoku-recovery: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute(
    arguments: impl IntoIterator<Item = String>,
    output: &mut impl Write,
) -> Result<(), String> {
    execute_with(arguments, output, confirm_rollback_on_host)
}

fn execute_with(
    arguments: impl IntoIterator<Item = String>,
    output: &mut impl Write,
    mut confirm_rollback: impl FnMut(&Identifier) -> Result<String, String>,
) -> Result<(), String> {
    let args: Vec<_> = arguments.into_iter().collect();
    match args.as_slice() {
        [command] if command == "admit-session" => {
            admit_session_on_host()?;
            writeln!(output, "recovery-complete; graphical login admitted")
                .map_err(|error| error.to_string())
        }
        [command] if command == "layout" => {
            let root = File::open("/").map_err(|error| error.to_string())?;
            let assessment = ryoku_recovery::snapper_layout::discover_layout(
                &root,
                &RelativePath::new("etc").expect("fixed path"),
            )
            .map_err(|error| error.to_string())?
            .assessment;
            writeln!(output, "system_capability={:?}", assessment.capability)
                .map_err(|error| error.to_string())?;
            writeln!(output, "system_reason={:?}", assessment.reason)
                .map_err(|error| error.to_string())?;
            writeln!(output, "ryoku_generation=not_checked").map_err(|error| error.to_string())?;
            writeln!(output, "root_rollback=not_tested").map_err(|error| error.to_string())?;
            Ok(())
        }
        [command] if command == "pending" => {
            let store = RestoreJournalStore::open_existing_state_mount()
                .map_err(|error| error.to_string())?;
            let pending = store.incomplete_attempts().map_err(|error| error.to_string())?;
            if pending.is_empty() {
                writeln!(output, "No unfinished recovery attempts")
                    .map_err(|error| error.to_string())?;
            } else {
                for attempt in pending {
                    writeln!(
                        output,
                        "attempt={} phase={:?} scope={:?} source_transaction={} source_generation={} current_state_generation={}",
                        attempt.restore_id.as_str(),
                        attempt.phase,
                        attempt.plan.scope,
                        attempt.plan.transaction_id.as_str(),
                        attempt.plan.generation_id.as_str(),
                        attempt.current_state_generation.as_str(),
                    )
                    .map_err(|error| error.to_string())?;
                }
                writeln!(
                    output,
                    "Recovery is unfinished; normal-session startup is not gated by this command."
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        [command] if command == "list" => {
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let transactions = reader.list_transactions().map_err(|error| error.to_string())?;
            if transactions.is_empty() {
                writeln!(output, "No committed transactions").map_err(|error| error.to_string())?;
            } else {
                for transaction in transactions {
                    match reader.verify_transaction(&transaction) {
                        Ok(manifest) => writeln!(
                            output,
                            "transaction={} integrity=verified generation={} snapshot={}:{} root-receipt={}",
                            manifest.transaction_id.as_str(),
                            manifest.generation_id.as_str(),
                            manifest.snapper_config.as_str(),
                            manifest.snapper_snapshot_id,
                            if manifest.root_snapshot_receipt().is_some() {
                                "recorded"
                            } else {
                                "absent"
                            },
                        ),
                        Err(_) => writeln!(
                            output,
                            "transaction={} integrity=unverified; restore is unavailable",
                            transaction.as_str(),
                        ),
                    }
                    .map_err(|error| error.to_string())?;
                }
                writeln!(
                    output,
                    "Integrity covers local generation contents only; use verify-linked to check the current Snapper partner."
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        [command, transaction_id] if command == "verify-linked" => {
            let transaction = Identifier::new(transaction_id.clone())
                .map_err(|error| format!("invalid transaction ID: {error:?}"))?;
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let root = File::open("/").map_err(|error| error.to_string())?;
            let linked = reader
                .open_historical_transaction(&root, &transaction)
                .map_err(|error| error.to_string())?;
            let manifest = linked.generation().manifest();
            writeln!(
                output,
                "partners-verified transaction={} generation={} snapshot={}:{}",
                manifest.transaction_id.as_str(),
                manifest.generation_id.as_str(),
                manifest.snapper_config.as_str(),
                manifest.snapper_snapshot_id,
            )
            .map_err(|error| error.to_string())?;
            writeln!(output, "Boot compatibility and restore readiness are not assessed")
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        [command, transaction_id, requested_scope] if command == "plan" => {
            let transaction = Identifier::new(transaction_id.clone())
                .map_err(|error| format!("invalid transaction ID: {error:?}"))?;
            let scope = match requested_scope.as_str() {
                "configuration-only" => RestoreScope::ConfigurationOnly,
                "system-only" => RestoreScope::SystemOnly,
                "complete-linked" => RestoreScope::CompleteLinked,
                _ => return Err(USAGE.into()),
            };
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let manifest =
                reader.verify_transaction(&transaction).map_err(|error| error.to_string())?;
            let configuration_entries = manifest
                .files()
                .iter()
                .filter(|entry| {
                    matches!(
                        entry.root,
                        StateRoot::EtcRyoku
                            | StateRoot::UserRyokuConfig
                            | StateRoot::UserHome
                            | StateRoot::UserRyokuProfile
                    )
                })
                .count();
            if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked)
                && configuration_entries == 0
            {
                return Err("selected generation has no declared configuration entries".into());
            }
            let (source_snapshot, ryoku_package_compatibility) =
                if matches!(scope, RestoreScope::SystemOnly | RestoreScope::CompleteLinked) {
                    let root = File::open("/").map_err(|error| error.to_string())?;
                    if reader.open_historical_transaction(&root, &transaction).is_ok() {
                        let snapshot_root = std::path::PathBuf::from(format!(
                            "/.snapshots/{}/snapshot",
                            manifest.snapper_snapshot_id
                        ));
                        let compatibility =
                            match revisions_for_source(Some(&snapshot_root), &manifest) {
                                Ok(revisions) if revisions == manifest.package_revisions() => {
                                    "match"
                                }
                                Ok(_) => "mismatch",
                                Err(_) => "unavailable",
                            };
                        ("verified", compatibility)
                    } else {
                        ("unavailable", "unavailable")
                    }
                } else {
                    let compatibility = match installed_ryoku_revisions() {
                        Ok(revisions) if revisions == manifest.package_revisions() => "match",
                        Ok(_) => "mismatch",
                        Err(_) => "unavailable",
                    };
                    ("not-required", compatibility)
                };
            writeln!(
                output,
                "restore-preview transaction={} generation={} scope={} configuration_entries={} source_snapshot={} ryoku_package_compatibility={}",
                manifest.transaction_id.as_str(),
                manifest.generation_id.as_str(),
                scope_name(scope),
                configuration_entries,
                source_snapshot,
                ryoku_package_compatibility,
            )
            .map_err(|error| error.to_string())?;
            if matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked) {
                write_configuration_preview(manifest.files(), output)
                    .map_err(|error| error.to_string())?;
            }
            writeln!(
                output,
                "restore_ready=no current_state_checkpoint=not-created compatibility=not-assessed writer_quiescence=not-acquired authorization=not-integrated"
            )
            .map_err(|error| error.to_string())?;
            Ok(())
        }
        [command, restore_id] if command == "confirm-rollback" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            writeln!(output, "{}", confirm_rollback(&restore_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id] if command == "restore-configuration" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            writeln!(output, "{}", restore_configuration_on_host(&restore_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id] if command == "discard-incomplete-stage" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            writeln!(output, "{}", discard_incomplete_stage_on_host(&restore_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id, source_id] if command == "prepare-configuration" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            let source_id = Identifier::new(source_id.clone())
                .map_err(|error| format!("invalid source transaction ID: {error:?}"))?;
            writeln!(output, "{}", prepare_configuration_on_host(&restore_id, &source_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id, source_id] if command == "prepare-system" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            let source_id = Identifier::new(source_id.clone())
                .map_err(|error| format!("invalid source transaction ID: {error:?}"))?;
            writeln!(output, "{}", prepare_system_on_host(&restore_id, &source_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id, source_id] if command == "prepare-linked" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            let source_id = Identifier::new(source_id.clone())
                .map_err(|error| format!("invalid source transaction ID: {error:?}"))?;
            writeln!(output, "{}", prepare_linked_on_host(&restore_id, &source_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id] if command == "rollback-system" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            writeln!(output, "{}", rollback_system_on_host(&restore_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id] if command == "rollback-linked" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            writeln!(output, "{}", rollback_linked_on_host(&restore_id)?)
                .map_err(|error| error.to_string())
        }
        [command, restore_id] if command == "complete-system" => {
            let restore_id = Identifier::new(restore_id.clone())
                .map_err(|error| format!("invalid restore ID: {error:?}"))?;
            writeln!(output, "{}", complete_system_on_host(&restore_id)?)
                .map_err(|error| error.to_string())
        }
        [command, transaction_id, account_name]
            if command == "manual-checkpoint" || command == "checkpoint-session" =>
        {
            let transaction = Identifier::new(transaction_id.clone())
                .map_err(|error| format!("invalid transaction ID: {error:?}"))?;
            if !valid_account_name(account_name) {
                return Err("invalid local account name".into());
            }
            writeln!(output, "{}", checkpoint_session_on_host(&transaction, account_name)?)
                .map_err(|error| error.to_string())
        }
        [command, transaction_id] if command == "inspect" || command == "verify" => {
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let transaction = Identifier::new(transaction_id.clone())
                .map_err(|error| format!("invalid transaction ID: {error:?}"))?;
            let manifest =
                reader.verify_transaction(&transaction).map_err(|error| error.to_string())?;
            if command == "verify" {
                writeln!(
                    output,
                    "verified transaction={} generation={} entries={}",
                    manifest.transaction_id.as_str(),
                    manifest.generation_id.as_str(),
                    manifest.files().len()
                )
                .map_err(|error| error.to_string())?;
            } else {
                writeln!(output, "Integrity: verified").map_err(|error| error.to_string())?;
                writeln!(
                    output,
                    "Original native receipt: {} (partner availability unchecked)",
                    if manifest.root_snapshot_receipt().is_some() { "recorded" } else { "absent" }
                )
                .map_err(|error| error.to_string())?;
                writeln!(output, "Transaction: {}", manifest.transaction_id.as_str())
                    .map_err(|error| error.to_string())?;
                writeln!(output, "Generation: {}", manifest.generation_id.as_str())
                    .map_err(|error| error.to_string())?;
                writeln!(
                    output,
                    "Snapper snapshot: {}:{}",
                    manifest.snapper_config.as_str(),
                    manifest.snapper_snapshot_id
                )
                .map_err(|error| error.to_string())?;
                writeln!(output, "Filesystem UUID: {}", manifest.filesystem_uuid)
                    .map_err(|error| error.to_string())?;
                writeln!(output, "Created UTC: {}", manifest.created_utc)
                    .map_err(|error| error.to_string())?;
                writeln!(output, "Declared entries: {}", manifest.files().len())
                    .map_err(|error| error.to_string())?;
                writeln!(
                    output,
                    "Manifest SHA-256: {}",
                    manifest.manifest_sha256.as_ref().map_or("missing", |digest| digest.as_str())
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn confirm_rollback_on_host(restore_id: &Identifier) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("confirm-rollback must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let pending = journals.load(restore_id).map_err(|error| error.to_string())?;
    stop_journal_account_writers(
        &root,
        &generations,
        &pending.restore_id,
        &pending.current_state_generation,
    )?;
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let journal = stores
        .confirm_booted_root_rollback(restore_id, &root)
        .map_err(|error| error.to_string())?;
    let next_action = match (journal.plan.scope, journal.phase) {
        (_, RestorePhase::Complete) => "the restore journal is already complete",
        (RestoreScope::CompleteLinked, _) => {
            "complete or resume the staged configuration restore before Ryoku login"
        }
        (RestoreScope::SystemOnly, _) => {
            "run or resume system health checks before marking recovery complete"
        }
        (RestoreScope::ConfigurationOnly, _) => {
            return Err("configuration-only attempt has no root rollback".into());
        }
    };
    let gate_status = if journal.phase == RestorePhase::Complete {
        "recovery journal is complete"
    } else {
        "the recovery gate remains closed"
    };
    Ok(format!(
        "rollback-target-verified attempt={} scope={:?} phase={:?}; {next_action}; {gate_status}",
        journal.restore_id.as_str(),
        journal.plan.scope,
        journal.phase,
    ))
}

fn discard_incomplete_stage_on_host(restore_id: &Identifier) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("discard-incomplete-stage must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    ryoku_recovery::state_provision::verify_initialized_state_root(&root, &record)
        .map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let journal = journals.load(restore_id).map_err(|error| error.to_string())?;
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    if source.manifest().assets().is_some() {
        return recovery_combined_cli::discard(&root, &state, &generations, &journals, restore_id);
    }
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    stores.discard_incomplete_stage(restore_id).map_err(|error| error.to_string())?;
    Ok(format!(
        "incomplete-stage-discarded attempt={}; retained checkpoint remains available for preparation resume",
        restore_id.as_str()
    ))
}

struct RestoreRoot {
    root: StateRoot,
    directory: File,
    paths: Vec<RelativePath>,
}

fn restore_configuration_on_host(restore_id: &Identifier) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("restore-configuration must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let mutation_guard = acquire_mutation_guard(&state)?;
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let journal = journals.load(restore_id).map_err(|error| error.to_string())?;
    let phase_ready = match journal.plan.scope {
        RestoreScope::ConfigurationOnly => matches!(
            journal.phase,
            RestorePhase::ContentStaged
                | RestorePhase::WritersStopped
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        ),
        RestoreScope::CompleteLinked => matches!(
            journal.phase,
            RestorePhase::RootRollbackComplete
                | RestorePhase::ConfigurationRestored
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        ),
        RestoreScope::SystemOnly => false,
    };
    if !phase_ready {
        return Err("attempt is not ready for configuration replay".into());
    }
    if journal.phase == RestorePhase::Complete {
        let known_working = record_known_working_after_complete_linked_restore(
            &root,
            &generations,
            &reader,
            &journal,
        )?;
        return Ok(format!(
            "restore-complete attempt={} known_working={}",
            journal.restore_id.as_str(),
            known_working_label(known_working)
        ));
    }
    if journal.plan.scope == RestoreScope::CompleteLinked
        && journal.phase == RestorePhase::WritersStopped
    {
        return Err("confirm the booted root rollback before configuration replay".into());
    }

    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    verify_profile_source(&source)?;
    if journal.plan.scope == RestoreScope::ConfigurationOnly
        && source.manifest().package_revisions() != installed_ryoku_revisions()?.as_slice()
    {
        return Err("installed Ryoku package revisions changed after restore preparation".into());
    }
    if journal.plan.scope == RestoreScope::CompleteLinked {
        reader
            .open_historical_transaction(&root, &journal.plan.transaction_id)
            .map_err(|error| error.to_string())?;
        let receipt = source
            .manifest()
            .root_snapshot_receipt()
            .ok_or("source generation has no verified native root receipt")?;
        ryoku_recovery::native_checkpoint::verify_running_root_rollback(&root, &journal, receipt)
            .map_err(|error| error.to_string())?;
    }
    let checkpoint = generations
        .open_verified_transaction(&journal.restore_id)
        .map_err(|error| error.to_string())?;
    reader
        .open_historical_transaction(&root, &journal.restore_id)
        .map_err(|error| error.to_string())?;
    let checkpoint_manifest = checkpoint.manifest();
    let checkpoint_status = SystemSnapshotStatus::Available {
        snapper_config: checkpoint_manifest.snapper_config.clone(),
        snapshot_id: checkpoint_manifest.snapper_snapshot_id.clone(),
        filesystem_uuid: checkpoint_manifest.filesystem_uuid.clone(),
    };
    let source_snapshot = if journal.plan.scope == RestoreScope::CompleteLinked {
        SystemSnapshotStatus::Available {
            snapper_config: source.manifest().snapper_config.clone(),
            snapshot_id: source.manifest().snapper_snapshot_id.clone(),
            filesystem_uuid: source.manifest().filesystem_uuid.clone(),
        }
    } else {
        SystemSnapshotStatus::Missing
    };
    let compatibility = RestoreCompatibility {
        source_snapshot,
        // The immutable journal proves the selected plan passed preflight;
        // complete-linked also reopens both native partners above.
        system: if journal.plan.scope == RestoreScope::CompleteLinked {
            CompatibilityStatus::Compatible
        } else {
            CompatibilityStatus::Unknown
        },
        configuration: CompatibilityStatus::Compatible,
    };

    let restore_roots = open_restore_roots(&root, &[source.manifest(), checkpoint_manifest])?;
    let current_roots: Vec<_> = restore_roots
        .iter()
        .filter(|restore_root| {
            checkpoint_manifest.files().iter().any(|entry| entry.root == restore_root.root)
        })
        .map(|restore_root| DeclaredRoot {
            root: restore_root.root,
            descriptor: &restore_root.directory,
            paths: &restore_root.paths,
        })
        .collect();
    stop_journal_account_writers(
        &root,
        &generations,
        &journal.restore_id,
        &journal.current_state_generation,
    )?;
    if source.manifest().assets().is_some() {
        return recovery_combined_cli::restore(recovery_combined_cli::Replay {
            root: &root,
            state: &state,
            generations: &generations,
            reader: &reader,
            journals: &journals,
            source: &source,
            checkpoint: &checkpoint,
            journal: &journal,
            compatibility: &compatibility,
        });
    }
    let checkpoint_snapshot = &checkpoint_status;
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let mut prepared = stores
        .resume(restore_id, &compatibility, checkpoint_snapshot, &current_roots)
        .map_err(|error| error.to_string())?;
    let needed: BTreeSet<_> =
        prepared.source().entries().iter().map(|entry| entry.record().root).collect();
    let destinations: Vec<_> = needed
        .iter()
        .map(|root| {
            let directory = restore_roots
                .iter()
                .find(|restore_root| restore_root.root == *root)
                .ok_or_else(|| format!("no trusted destination for {}", root.manifest_name()))?;
            Ok(ConfigurationDestination { root: *root, directory: &directory.directory })
        })
        .collect::<Result<_, String>>()?;
    let health_files: Vec<_> = prepared
        .source()
        .entries()
        .iter()
        .filter_map(|entry| match &entry.record().kind {
            EntryKind::RegularFile { size, .. }
                if entry.record().path.as_str().ends_with(".json") =>
            {
                Some((entry.record().root, entry.record().path.clone(), *size as usize))
            }
            _ => None,
        })
        .collect();
    let completed = stores
        .execute_configuration_restore(
            &mut prepared,
            &destinations,
            || {
                // Recheck immediately before publication. The exclusive state-mount
                // mutation lock remains held until this command exits.
                stop_journal_account_writers(
                    &root,
                    &generations,
                    &journal.restore_id,
                    &journal.current_state_generation,
                )
            },
            |_| {
                verify_configuration_restore_health(
                    &health_files,
                    &restore_roots,
                    verify_systemd_health,
                )
            },
        )
        .map_err(|error| error.to_string())?;
    let known_working = record_known_working_after_complete_linked_restore(
        &root,
        &generations,
        &reader,
        &completed,
    )?;
    drop(mutation_guard);
    Ok(format!(
        "configuration-restored attempt={} phase={:?} known_working={}; exact content, metadata, JSON, profile and systemd recovery-gate checks passed; reboot to resume normal sessions",
        completed.restore_id.as_str(),
        completed.phase,
        known_working_label(known_working)
    ))
}

fn known_working_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "recorded",
        Some(false) => "not-eligible",
        None => "not-applicable",
    }
}

/// Promote a source only after a complete linked rollback, configuration replay,
/// and health check have reached the durable Complete journal phase. The
/// caller holds the exclusive common mutation lock; live partners are reopened
/// before the retention pin is written. Retrying a completed journal is safe.
fn record_known_working_after_complete_linked_restore(
    root: &File,
    generations: &GenerationStore,
    reader: &GenerationReader,
    journal: &RestoreJournal,
) -> Result<Option<bool>, String> {
    if journal.plan.scope != RestoreScope::CompleteLinked || journal.phase != RestorePhase::Complete
    {
        return Ok(None);
    }
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    let receipt = source
        .manifest()
        .root_snapshot_receipt()
        .ok_or("completed linked source has no native root receipt")?;
    reader
        .open_historical_transaction(root, &journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    ryoku_recovery::native_checkpoint::verify_running_root_rollback(root, journal, receipt)
        .map_err(|error| error.to_string())?;

    let rollback = journal.root_rollback.as_ref().ok_or("missing rollback receipt")?;
    let catalog = ProtectedSnapperCatalog::open(root).map_err(|error| error.to_string())?;
    let status = catalog.status(&journal.plan.transaction_id).map_err(|error| error.to_string())?;
    if !status.complete || status.entry.capability != LayoutCapability::FullSystem {
        return Ok(Some(false));
    }
    catalog
        .mark_known_working_after_boot(
            &journal.plan.transaction_id,
            &SnapshotId::new(rollback.target_snapshot()).map_err(|error| format!("{error:?}"))?,
            &ryoku_recovery::restore::root_rollback_tag(&journal.restore_id)
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    Ok(Some(true))
}

/// Prepare a system-only rollback through the same manifest-linked journal as
/// configuration restores. The current-state generation is captured before
/// Snapper is called, and includes every declared source path.
fn prepare_system_on_host(
    restore_id: &Identifier,
    source_id: &Identifier,
) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("prepare-system must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    ryoku_recovery::state_provision::verify_initialized_state_root(&root, &record)
        .map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    if restore_id == source_id
        || reader.list_transactions().map_err(|error| error.to_string())?.contains(restore_id)
    {
        return Err("restore ID is already used by a transaction; choose a new ID".into());
    }
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    match journals.load(restore_id) {
        Ok(_) => return Err("restore ID already has a journal; choose a new ID".into()),
        Err(JournalError::Filesystem(Errno::NOENT)) => {}
        Err(error) => return Err(error.to_string()),
    }
    let source =
        generations.open_verified_transaction(source_id).map_err(|error| error.to_string())?;
    let compatibility = verify_system_source_compatibility(&root, &reader, &source)?;
    let principal = source
        .manifest()
        .account_principal()
        .ok_or("system rollback source has no bound Ryoku account")?;
    let account = resolve_local_account(&root, principal)?;
    stop_account_writers(account.uid)?;

    let restore_roots = open_restore_roots(&root, &[source.manifest()])?;
    if restore_roots.is_empty() {
        return Err("system rollback source has no supported Ryoku state inventory".into());
    }
    let current_roots: Vec<_> = restore_roots
        .iter()
        .map(|item| DeclaredRoot {
            root: item.root,
            descriptor: &item.directory,
            paths: &item.paths,
        })
        .collect();
    let request = ryoku_snapper::SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|error| format!("{error:?}"))?,
        transaction_id: TransactionId::new(restore_id.as_str())
            .map_err(|error| format!("{error:?}"))?,
    };
    let snapshot =
        ryoku_snapper::SnapperCheckpointBackend.create_single(&request).map_err(|error| {
            format!("current-state Snapper checkpoint failed; reconcile before retrying: {error}")
        })?;
    let link = SnapshotLink::new(
        Identifier::new("root").map_err(|error| format!("{error:?}"))?,
        snapshot.as_str(),
        record.identity().filesystem_uuid.clone(),
    )
    .map_err(|error| format!("invalid native snapshot link: {error:?}"))?;
    let native = verify_new_root_checkpoint(&root, link).map_err(|error| error.to_string())?;
    let installed = installed_ryoku_revisions()?;
    let generation = generation_id_for(restore_id);
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let prepared = stores
        .begin_system(SystemPreparationRequest {
            restore_id: restore_id.clone(),
            source: &source,
            compatibility,
            checkpoint: ryoku_recovery::generation_capture::GenerationCaptureRequest {
                transaction_id: restore_id.clone(),
                generation_id: generation,
                parent_generation_id: None,
                snapshot: native.verified_link().clone(),
                reason: ryoku_recovery::OperationReason::Other(
                    Identifier::new("system-restore").expect("fixed reason ID is canonical"),
                ),
                created_utc: current_utc_timestamp()?,
                package_revisions: installed.clone(),
                provider_revision: None,
                profile_revision: None,
                roots: &current_roots,
                limits: Default::default(),
            },
        })
        .map_err(|error| error.to_string())?;
    if installed_ryoku_revisions()? != installed {
        return Err("Ryoku packages changed during restore preparation; staged attempt retained for reconciliation".into());
    }
    ryoku_recovery::native_checkpoint::verify_historical_root_checkpoint(&root, native.receipt())
        .map_err(|error| {
        format!("system restore is prepared but its fresh Snapper checkpoint changed: {error}")
    })?;
    let checkpoint = prepared.checkpoint().manifest();
    ensure_cataloged_checkpoint(&root, &generations, &reader, checkpoint)?;
    Ok(format!(
        "system-restore-prepared attempt={} source={} phase={:?} current_checkpoint={} entries={}; run rollback-system from rescue/emergency console to roll back and reboot",
        prepared.journal().restore_id.as_str(),
        source_id.as_str(),
        prepared.journal().phase,
        checkpoint.generation_id.as_str(),
        checkpoint.files().len(),
    ))
}

fn rollback_system_on_host(restore_id: &Identifier) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("rollback-system must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let journal = journals.load(restore_id).map_err(|error| error.to_string())?;
    if journal.plan.scope != RestoreScope::SystemOnly {
        return Err("attempt is not a system-only restore".into());
    }
    if journal.has_recorded_root_rollback_handoff(RestoreScope::SystemOnly) {
        return resume_recorded_rollback_handoff(
            &root,
            &generations,
            &reader,
            &journals,
            journal,
            RestoreScope::SystemOnly,
        );
    }
    if !matches!(journal.phase, RestorePhase::Planned | RestorePhase::CheckpointVerified) {
        return Err("system restore is not at a resumable pre-rollback phase; pending attempts are never retried automatically".into());
    }
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    let compatibility = verify_system_source_compatibility(&root, &reader, &source)?;
    let checkpoint =
        generations.open_verified_transaction(restore_id).map_err(|error| error.to_string())?;
    reader.open_historical_transaction(&root, restore_id).map_err(|error| error.to_string())?;
    let checkpoint_receipt = checkpoint
        .manifest()
        .root_snapshot_receipt()
        .ok_or("current-state checkpoint has no verified native root receipt")?;
    ryoku_recovery::native_checkpoint::verify_checkpoint_origin_is_running_root(
        &root,
        checkpoint_receipt,
    )
    .map_err(|error| format!("running root changed after the current-state checkpoint: {error}"))?;
    let checkpoint_snapshot = SystemSnapshotStatus::Available {
        snapper_config: checkpoint.manifest().snapper_config.clone(),
        snapshot_id: checkpoint.manifest().snapper_snapshot_id.clone(),
        filesystem_uuid: checkpoint.manifest().filesystem_uuid.clone(),
    };
    let restore_roots = open_restore_roots(&root, &[source.manifest(), checkpoint.manifest()])?;
    let current_roots: Vec<_> = restore_roots
        .iter()
        .map(|item| DeclaredRoot {
            root: item.root,
            descriptor: &item.directory,
            paths: &item.paths,
        })
        .collect();
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let mut prepared = stores
        .resume_system(restore_id, &compatibility, &checkpoint_snapshot, &current_roots)
        .map_err(|error| error.to_string())?;
    let handoff = stores
        .execute_system_root_rollback(
            &mut prepared,
            || {
                stop_journal_account_writers(
                    &root,
                    &generations,
                    restore_id,
                    &journal.current_state_generation,
                )
            },
            &mut ryoku_snapper::SnapperRollbackBackend,
        )
        .map_err(|error| error.to_string())?;
    let attempt = handoff.journal().restore_id.as_str().to_owned();
    request_recorded_rollback_reboot(&attempt, "system")?;
    Ok(format!(
        "system-rollback-requested attempt={attempt}; the recovery gate remains closed until boot verification and health checks complete"
    ))
}

fn verify_profile_source(
    source: &ryoku_recovery::generation::VerifiedGeneration,
) -> Result<(), String> {
    ryoku_recovery::installed_profiles::ProfileBundleStore::open_state_mount()
        .and_then(|store| store.resolve_generation(source))
        .map(|_| ())
        .map_err(|error| format!("saved profile inputs are unavailable or incompatible: {error}"))
}

fn verify_system_source_compatibility(
    root: &File,
    reader: &GenerationReader,
    source: &ryoku_recovery::generation::VerifiedGeneration,
) -> Result<RestoreCompatibility, String> {
    verify_profile_source(source)?;
    let manifest = source.manifest();
    let catalog = ProtectedSnapperCatalog::open(root).map_err(|error| error.to_string())?;
    let status = catalog.status(&manifest.transaction_id).map_err(|error| error.to_string())?;
    let layout = ryoku_recovery::snapper_layout::discover_layout(
        root,
        &RelativePath::new("etc").expect("fixed path"),
    )
    .map_err(|error| error.to_string())?;
    if !status.complete
        || !status.entry.additional_snapshots.is_empty()
        || layout.targets.len() != 1
        || layout.targets[0].config.as_str() != "root"
    {
        return Err("system restore requires a verified single-root layout; additional Snapper configurations are unsupported".into());
    }
    let receipt = manifest
        .root_snapshot_receipt()
        .ok_or("system restore source has no verified native root receipt")?;
    reader
        .open_historical_transaction(root, &manifest.transaction_id)
        .map_err(|error| error.to_string())?;
    let snapshot_root = std::path::PathBuf::from(format!(
        "/.snapshots/{}/snapshot",
        receipt.link().snapper_snapshot_id
    ));
    let snapshot_descriptor = File::open(&snapshot_root).map_err(|error| error.to_string())?;
    ryoku_recovery::session_admission::verify_installed_units(&snapshot_descriptor)
        .map_err(|error| format!("rollback source cannot enforce graphical admission: {error}"))?;
    let system = match revisions_for_source(Some(&snapshot_root), manifest) {
        Ok(revisions) if revisions == manifest.package_revisions() => {
            CompatibilityStatus::Compatible
        }
        Ok(_) => CompatibilityStatus::Incompatible,
        Err(_) => CompatibilityStatus::Unknown,
    };
    Ok(RestoreCompatibility {
        source_snapshot: SystemSnapshotStatus::Available {
            snapper_config: receipt.link().snapper_config.clone(),
            snapshot_id: receipt.link().snapper_snapshot_id.clone(),
            filesystem_uuid: receipt.link().filesystem_uuid.clone(),
        },
        system,
        configuration: CompatibilityStatus::Unknown,
    })
}

fn rollback_linked_on_host(restore_id: &Identifier) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("rollback-linked must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let journal = journals.load(restore_id).map_err(|error| error.to_string())?;
    if journal.plan.scope != RestoreScope::CompleteLinked {
        return Err("attempt is not a complete-linked restore".into());
    }
    if journal.has_recorded_root_rollback_handoff(RestoreScope::CompleteLinked) {
        return resume_recorded_rollback_handoff(
            &root,
            &generations,
            &reader,
            &journals,
            journal,
            RestoreScope::CompleteLinked,
        );
    }
    if journal.phase != RestorePhase::ContentStaged {
        return Err("linked restore is not at its prepared phase; pending attempts are never retried automatically".into());
    }
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    let mut compatibility = verify_system_source_compatibility(&root, &reader, &source)?;
    compatibility.configuration = CompatibilityStatus::Compatible;
    let checkpoint =
        generations.open_verified_transaction(restore_id).map_err(|error| error.to_string())?;
    reader.open_historical_transaction(&root, restore_id).map_err(|error| error.to_string())?;
    let checkpoint_receipt = checkpoint
        .manifest()
        .root_snapshot_receipt()
        .ok_or("current-state checkpoint has no verified native root receipt")?;
    ryoku_recovery::native_checkpoint::verify_checkpoint_origin_is_running_root(
        &root,
        checkpoint_receipt,
    )
    .map_err(|error| format!("running root changed after the current-state checkpoint: {error}"))?;
    let checkpoint_snapshot = SystemSnapshotStatus::Available {
        snapper_config: checkpoint.manifest().snapper_config.clone(),
        snapshot_id: checkpoint.manifest().snapper_snapshot_id.clone(),
        filesystem_uuid: checkpoint.manifest().filesystem_uuid.clone(),
    };
    let restore_roots = open_restore_roots(&root, &[source.manifest(), checkpoint.manifest()])?;
    let current_roots: Vec<_> = restore_roots
        .iter()
        .map(|item| DeclaredRoot {
            root: item.root,
            descriptor: &item.directory,
            paths: &item.paths,
        })
        .collect();
    if source.manifest().assets().is_some() {
        return recovery_combined_cli::rollback(recovery_combined_cli::Replay {
            root: &root,
            state: &state,
            generations: &generations,
            reader: &reader,
            journals: &journals,
            source: &source,
            checkpoint: &checkpoint,
            journal: &journal,
            compatibility: &compatibility,
        });
    }
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let mut prepared = stores
        .resume(restore_id, &compatibility, &checkpoint_snapshot, &current_roots)
        .map_err(|error| error.to_string())?;
    let handoff = stores
        .execute_complete_linked_root_rollback(
            &mut prepared,
            || {
                stop_journal_account_writers(
                    &root,
                    &generations,
                    restore_id,
                    &journal.current_state_generation,
                )
            },
            &mut ryoku_snapper::SnapperRollbackBackend,
        )
        .map_err(|error| error.to_string())?;
    let attempt = handoff.journal().restore_id.as_str().to_owned();
    request_recorded_rollback_reboot(&attempt, "linked")?;
    Ok(format!(
        "linked-rollback-requested attempt={attempt}; after boot run confirm-rollback, then restore-configuration; the recovery gate remains closed"
    ))
}

/// Resume only the reboot handoff after the exact Snapper rollback receipt is
/// already durable. This never invokes Snapper again, so process interruption
/// cannot duplicate a root rollback.
fn request_recorded_rollback_reboot(attempt: &str, scope: &str) -> Result<(), String> {
    let reboot = Command::new("/usr/bin/systemctl")
        .env_clear()
        .args(["reboot"])
        .status()
        .map_err(|error| {
            format!("{scope} rollback is journaled for {attempt}, but reboot could not be requested: {error}")
        })?;
    if !reboot.success() {
        return Err(format!(
            "{scope} rollback is journaled for {attempt}, but systemd rejected the reboot request"
        ));
    }
    Ok(())
}

fn resume_recorded_rollback_handoff(
    root: &File,
    generations: &GenerationStore,
    reader: &GenerationReader,
    journals: &RestoreJournalStore,
    mut journal: RestoreJournal,
    scope: RestoreScope,
) -> Result<String, String> {
    if !journal.has_recorded_root_rollback_handoff(scope) {
        return Err("attempt has no matching recorded rollback handoff".into());
    }
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    reader
        .open_historical_transaction(root, &journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    let source_receipt = source
        .manifest()
        .root_snapshot_receipt()
        .ok_or("rollback source has no verified native root receipt")?;

    match ryoku_recovery::native_checkpoint::verify_running_root_rollback(
        root,
        &journal,
        source_receipt,
    ) {
        Ok(evidence) => {
            stop_journal_account_writers(
                root,
                generations,
                &journal.restore_id,
                &journal.current_state_generation,
            )?;
            journals
                .confirm_root_rollback_booted(&mut journal, &evidence)
                .map_err(|error| error.to_string())?;
            let next = match scope {
                RestoreScope::SystemOnly => "run complete-system to check system health",
                RestoreScope::CompleteLinked => {
                    "run restore-configuration before resuming the normal session"
                }
                RestoreScope::ConfigurationOnly => {
                    return Err("configuration-only attempt has no root rollback".into());
                }
            };
            return Ok(format!(
                "rollback-target-verified attempt={}; the target root had already booted, so no reboot was requested; {next}; the recovery gate remains closed",
                journal.restore_id.as_str()
            ));
        }
        Err(ryoku_recovery::native_checkpoint::NativeCheckpointError::RollbackTargetNotBooted) => {}
        Err(error) => return Err(error.to_string()),
    }

    let checkpoint = generations
        .open_verified_transaction(&journal.restore_id)
        .map_err(|error| error.to_string())?;
    reader
        .open_historical_transaction(root, &journal.restore_id)
        .map_err(|error| error.to_string())?;
    let checkpoint_receipt = checkpoint
        .manifest()
        .root_snapshot_receipt()
        .ok_or("current-state checkpoint has no verified native root receipt")?;
    ryoku_recovery::native_checkpoint::verify_checkpoint_origin_is_running_root(
        root,
        checkpoint_receipt,
    )
    .map_err(|error| {
        format!("recorded rollback target is not booted and current root is not its verified checkpoint: {error}")
    })?;
    let rollback = journal
        .root_rollback
        .as_ref()
        .ok_or("recorded rollback handoff has no rollback receipt")?;
    let target = SnapshotId::new(rollback.target_snapshot())
        .map_err(|error| format!("invalid rollback target in journal: {error:?}"))?;
    let backup = SnapshotId::new(rollback.backup_snapshot())
        .map_err(|error| format!("invalid rollback backup in journal: {error:?}"))?;
    let transaction = ryoku_recovery::restore::root_rollback_tag(&journal.restore_id)
        .map_err(|error| error.to_string())?;
    ryoku_snapper::SnapperRollbackBackend
        .verify_reboot_handoff(
            &ConfigName::new("root").map_err(|error| format!("invalid root config: {error:?}"))?,
            &target,
            &backup,
            &transaction,
        )
        .map_err(|error| format!("recorded Snapper reboot handoff is no longer valid: {error}"))?;
    let scope_name = match scope {
        RestoreScope::SystemOnly => "system",
        RestoreScope::CompleteLinked => "linked",
        RestoreScope::ConfigurationOnly => {
            return Err("configuration-only attempt has no root rollback".into());
        }
    };
    request_recorded_rollback_reboot(journal.restore_id.as_str(), scope_name)?;
    Ok(format!(
        "{scope_name}-rollback-reboot-requested attempt={}; the recorded Snapper rollback was not repeated; the recovery gate remains closed",
        journal.restore_id.as_str()
    ))
}

fn prepare_configuration_on_host(
    restore_id: &Identifier,
    source_id: &Identifier,
) -> Result<String, String> {
    prepare_configuration_scope_on_host(restore_id, source_id, RestoreScope::ConfigurationOnly)
}

fn prepare_linked_on_host(
    restore_id: &Identifier,
    source_id: &Identifier,
) -> Result<String, String> {
    prepare_configuration_scope_on_host(restore_id, source_id, RestoreScope::CompleteLinked)
}

fn prepare_configuration_scope_on_host(
    restore_id: &Identifier,
    source_id: &Identifier,
    scope: RestoreScope,
) -> Result<String, String> {
    if !matches!(scope, RestoreScope::ConfigurationOnly | RestoreScope::CompleteLinked) {
        return Err("unsupported configuration restore scope".into());
    }
    if rustix::process::geteuid().as_raw() != 0 {
        return Err(
            "configuration restore preparation must run as root from the recovery console".into()
        );
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    ryoku_recovery::state_provision::verify_initialized_state_root(&root, &record)
        .map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    if restore_id == source_id {
        return Err("restore ID must differ from the source transaction ID".into());
    }
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let existing_journal = match journals.load(restore_id) {
        Ok(journal) => {
            if journal.plan.transaction_id != *source_id || journal.plan.scope != scope {
                return Err("restore ID is already bound to a different source or scope".into());
            }
            if !matches!(
                journal.phase,
                RestorePhase::Planned
                    | RestorePhase::CheckpointVerified
                    | RestorePhase::ContentStaged
            ) {
                return Err("restore attempt has advanced beyond resumable preparation".into());
            }
            Some(journal)
        }
        Err(JournalError::Filesystem(Errno::NOENT)) => None,
        Err(error) => return Err(error.to_string()),
    };
    if existing_journal.is_none()
        && reader.list_transactions().map_err(|error| error.to_string())?.contains(restore_id)
    {
        return Err("restore ID is already used by a transaction; choose a new ID".into());
    }
    let source =
        generations.open_verified_transaction(source_id).map_err(|error| error.to_string())?;
    reader.open_historical_transaction(&root, source_id).map_err(|error| error.to_string())?;

    verify_profile_source(&source)?;
    let inventory = InstalledInventory::current().map_err(|error| error.to_string())?;
    let local_account = resolve_local_account(
        &root,
        source.manifest().account_principal().ok_or("source has no account binding")?,
    )?;
    {
        let state_root = StateRoot::UserHome;
        let expected =
            inventory.paths_for_root(state_root).ok_or("incomplete session inventory")?;
        let actual: BTreeSet<_> = source
            .manifest()
            .files()
            .iter()
            .filter(|entry| entry.root == state_root)
            .map(|entry| entry.path.clone())
            .collect();
        if actual.len() != expected.len() || expected.iter().any(|path| !actual.contains(path)) {
            return Err("source is not an exact session-preferences checkpoint".into());
        }
    }
    let recorded = source.manifest().package_revisions();
    let installed = revisions_for_source(None, source.manifest())?;
    if scope == RestoreScope::ConfigurationOnly && recorded != installed {
        return Err(
            "source Ryoku package revisions differ from the installed settings writers".into()
        );
    }
    let compatibility = if scope == RestoreScope::CompleteLinked {
        let mut compatibility = verify_system_source_compatibility(&root, &reader, &source)?;
        compatibility.configuration = CompatibilityStatus::Compatible;
        compatibility
    } else {
        RestoreCompatibility {
            source_snapshot: SystemSnapshotStatus::Missing,
            system: CompatibilityStatus::Unknown,
            configuration: CompatibilityStatus::Compatible,
        }
    };
    for entry in source.manifest().files() {
        if !session_checkpoint_entry_kind_allowed(entry) {
            return Err(format!(
                "source entry kind is unsupported for session preferences: {}",
                entry.path.as_str()
            ));
        }
        let metadata = match &entry.kind {
            EntryKind::Directory { metadata } | EntryKind::RegularFile { metadata, .. } => {
                Some(metadata)
            }
            EntryKind::Absent => None,
            EntryKind::Symlink { metadata, .. } => Some(metadata),
        };
        if metadata.is_some_and(|metadata| {
            metadata.uid != local_account.uid || metadata.gid != local_account.gid
        }) {
            return Err(format!(
                "source entry ownership differs from the bound account: {}",
                entry.path.as_str()
            ));
        }
        if let EntryKind::RegularFile { sha256, .. } = &entry.kind {
            let bytes = source.read_payload(sha256).map_err(|error| error.to_string())?;
            if entry.path.as_str().ends_with(".json") && !is_json_object(&bytes) {
                return Err(format!(
                    "source settings are not a JSON object: {}",
                    entry.path.as_str()
                ));
            }
            if entry.root == StateRoot::UserHome
                && entry.path.as_str() == ryoku_recovery::installed_inventory::PERSONAL_PROFILE
                && ryoku_profiles::ProfileDocument::parse(&bytes).is_err()
            {
                return Err("source personal profile failed schema validation".into());
            }
        }
    }

    if source.manifest().assets().is_some() {
        return recovery_combined_cli::prepare(recovery_combined_cli::Prepare {
            root: &root,
            state: &state,
            generations: &generations,
            reader: &reader,
            journals: &journals,
            source: &source,
            account: &local_account,
            restore_id,
            scope,
            compatibility,
            installed: &installed,
            existing: existing_journal,
        });
    }

    if let Some(journal) = existing_journal {
        let checkpoint = generations.open_verified_transaction(restore_id).map_err(|error| {
            format!("retained current-state generation is unavailable: {error}")
        })?;
        let checkpoint_manifest = checkpoint.manifest();
        if checkpoint_manifest.transaction_id != *restore_id
            || checkpoint_manifest.generation_id != journal.current_state_generation
            || checkpoint_manifest.manifest_sha256.as_ref()
                != Some(&journal.current_state_manifest_sha256)
        {
            return Err("retained checkpoint does not match the preparation journal".into());
        }
        ensure_cataloged_checkpoint(&root, &generations, &reader, checkpoint_manifest)?;
        let checkpoint_snapshot = SystemSnapshotStatus::Available {
            snapper_config: checkpoint_manifest.snapper_config.clone(),
            snapshot_id: checkpoint_manifest.snapper_snapshot_id.clone(),
            filesystem_uuid: checkpoint_manifest.filesystem_uuid.clone(),
        };
        let restore_roots = open_restore_roots(&root, &[source.manifest(), checkpoint_manifest])?;
        let current_roots: Vec<_> = restore_roots
            .iter()
            .filter(|item| checkpoint_manifest.files().iter().any(|entry| entry.root == item.root))
            .map(|item| DeclaredRoot {
                root: item.root,
                descriptor: &item.directory,
                paths: &item.paths,
            })
            .collect();
        stop_journal_account_writers(
            &root,
            &generations,
            restore_id,
            &journal.current_state_generation,
        )?;
        let staging = File::from(
            openat2(
                &state,
                "staging",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .map_err(|error| error.to_string())?,
        );
        let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
        let stores =
            PreparationStores { generations: &generations, journals: &journals, stages: &stages };
        let prepared = stores
            .resume(restore_id, &compatibility, &checkpoint_snapshot, &current_roots)
            .map_err(|error| error.to_string())?;
        if revisions_for_source(None, source.manifest())? != installed {
            return Err("Ryoku packages changed during preparation resume".into());
        }
        return Ok(format!(
            "configuration-restore-preparation-resumed attempt={} source={} scope={:?} phase={:?} current_checkpoint={} entries={}; continue with the offline restore command when ready",
            prepared.journal().restore_id.as_str(),
            source_id.as_str(),
            prepared.journal().plan.scope,
            prepared.journal().phase,
            checkpoint_manifest.generation_id.as_str(),
            checkpoint_manifest.files().len(),
        ));
    }

    // The current-state snapshot and capture must observe a quiescent account.
    // The writer lock prevents the gated Ryoku session from restarting while
    // this preparation is in progress.
    stop_account_writers(local_account.uid)?;

    let restore_roots = open_restore_roots(&root, &[source.manifest()])?;
    let current_roots: Vec<_> = restore_roots
        .iter()
        .map(|item| DeclaredRoot {
            root: item.root,
            descriptor: &item.directory,
            paths: inventory.paths_for_root(item.root).unwrap_or(&[]),
        })
        .collect();
    let request = ryoku_snapper::SnapshotPairRequest {
        config: ConfigName::new("root").map_err(|error| format!("{error:?}"))?,
        transaction_id: TransactionId::new(restore_id.as_str())
            .map_err(|error| format!("{error:?}"))?,
    };
    let snapshot =
        ryoku_snapper::SnapperCheckpointBackend.create_single(&request).map_err(|error| {
            format!("current-state Snapper checkpoint failed; reconcile before retrying: {error}")
        })?;
    let link = SnapshotLink::new(
        Identifier::new("root").map_err(|error| format!("{error:?}"))?,
        snapshot.as_str(),
        record.identity().filesystem_uuid.clone(),
    )
    .map_err(|error| format!("invalid native snapshot link: {error:?}"))?;
    let native = verify_new_root_checkpoint(&root, link).map_err(|error| error.to_string())?;
    let generation = generation_id_for(restore_id);
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let prepared = stores
        .begin(ConfigurationPreparationRequest {
            restore_id: restore_id.clone(),
            source: &source,
            scope,
            compatibility,
            checkpoint: ryoku_recovery::generation_capture::GenerationCaptureRequest {
                transaction_id: restore_id.clone(),
                generation_id: generation.clone(),
                parent_generation_id: None,
                snapshot: native.verified_link().clone(),
                reason: ryoku_recovery::OperationReason::ConfigurationRestore,
                created_utc: current_utc_timestamp()?,
                package_revisions: installed.clone(),
                provider_revision: None,
                profile_revision: None,
                roots: &current_roots,
                limits: Default::default(),
            },
        })
        .map_err(|error| error.to_string())?;
    if revisions_for_source(None, source.manifest())? != installed {
        return Err("Ryoku packages changed during restore preparation; staged attempt retained for reconciliation".into());
    }
    let checkpoint = prepared.checkpoint().manifest();
    ensure_cataloged_checkpoint(&root, &generations, &reader, checkpoint)?;
    Ok(format!(
        "configuration-restore-prepared attempt={} source={} scope={:?} phase={:?} current_checkpoint={} entries={}; {}",
        prepared.journal().restore_id.as_str(),
        source_id.as_str(),
        prepared.journal().plan.scope,
        prepared.journal().phase,
        checkpoint.generation_id.as_str(),
        checkpoint.files().len(),
        if scope == RestoreScope::CompleteLinked {
            "run rollback-linked from rescue/emergency console"
        } else {
            "offline configuration restore only"
        }
    ))
}

fn ensure_cataloged_checkpoint(
    root: &File,
    _generations: &GenerationStore,
    reader: &GenerationReader,
    manifest: &RecoveryManifest,
) -> Result<(), String> {
    let receipt = manifest
        .root_snapshot_receipt()
        .ok_or("current-state generation has no verified native root snapshot receipt")?;
    ryoku_recovery::native_checkpoint::verify_historical_root_checkpoint(root, receipt).map_err(
        |error| format!("current-state Snapper partner is unavailable or changed: {error}"),
    )?;
    let catalog = ProtectedSnapperCatalog::open(root)
        .or_else(|_| ProtectedSnapperCatalog::initialize(root))
        .map_err(|error| {
            format!("protected catalog unavailable; no existing object was adopted: {error}")
        })?;
    match catalog.status(&manifest.transaction_id) {
        Ok(status) => {
            if status.entry.generation_id.as_str() != manifest.generation_id.as_str()
                || status.entry.root_snapshot.as_str() != manifest.snapper_snapshot_id
                || !status.observation.generation_present_and_verified
                || !status.observation.snapper_partner_present_and_verified
                || !status.cleanup_owned
            {
                return Err("retained checkpoint conflicts with its catalog entry".into());
            }
        }
        Err(_) => catalog
            .publish(&manifest.transaction_id, &[])
            .map_err(|error| format!("checkpoint catalog publication failed: {error}"))?,
    }
    reader
        .open_historical_transaction(root, &manifest.transaction_id)
        .map_err(|error| format!("checkpoint catalog reconciliation failed: {error}"))?;
    Ok(())
}

fn session_checkpoint_entry_kind_allowed(record: &FileRecord) -> bool {
    InstalledInventory::current().is_ok_and(|inventory| inventory.accepts_record_kind(record))
}

fn validate_restored_json(
    files: &[(StateRoot, RelativePath, usize)],
    roots: &[RestoreRoot],
) -> Result<(), String> {
    for (root, path, expected_size) in files {
        let directory = roots
            .iter()
            .find(|candidate| candidate.root == *root)
            .ok_or("restored configuration root is unavailable")?;
        let file = File::from(
            openat2(
                &directory.directory,
                path.as_str(),
                OFlags::RDONLY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(|_| "restored configuration could not be reopened for health checking")?,
        );
        let stat = fstat(&file).map_err(|_| "restored configuration metadata is unavailable")?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_size < 0
            || stat.st_size as usize != *expected_size
            || *expected_size > 1024 * 1024
        {
            return Err("restored configuration health check failed".into());
        }
        let mut bytes = Vec::with_capacity(*expected_size);
        file.take((*expected_size as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| "restored configuration could not be read for health checking")?;
        if bytes.len() != *expected_size || !is_json_object(&bytes) {
            return Err("restored JSON configuration is not an object".into());
        }
        if *root == StateRoot::UserHome
            && path.as_str() == ryoku_recovery::installed_inventory::PERSONAL_PROFILE
            && ryoku_profiles::ProfileDocument::parse(&bytes).is_err()
        {
            return Err("restored personal profile failed schema validation".into());
        }
    }
    Ok(())
}

fn verify_configuration_restore_health(
    files: &[(StateRoot, RelativePath, usize)],
    roots: &[RestoreRoot],
    systemd_health_check: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    validate_restored_json(files, roots)?;
    systemd_health_check()
}

fn is_json_object(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| value.is_object())
}

fn complete_system_on_host(restore_id: &Identifier) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("complete-system must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    let journals =
        RestoreJournalStore::open_existing_state_mount().map_err(|error| error.to_string())?;
    let mut journal = journals.load(restore_id).map_err(|error| error.to_string())?;
    if journal.plan.scope != RestoreScope::SystemOnly
        || !matches!(
            journal.phase,
            RestorePhase::RootRollbackComplete
                | RestorePhase::HealthChecked
                | RestorePhase::Complete
        )
    {
        return Err("attempt is not ready for system-only health completion".into());
    }
    if journal.phase == RestorePhase::Complete {
        return Ok(format!("restore-complete attempt={}", journal.restore_id.as_str()));
    }
    stop_journal_account_writers(
        &root,
        &generations,
        &journal.restore_id,
        &journal.current_state_generation,
    )?;
    let source = generations
        .open_verified_transaction(&journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    reader
        .open_historical_transaction(&root, &journal.plan.transaction_id)
        .map_err(|error| error.to_string())?;
    let receipt = source
        .manifest()
        .root_snapshot_receipt()
        .ok_or("source generation has no verified native root receipt")?;
    ryoku_recovery::native_checkpoint::verify_running_root_rollback(&root, &journal, receipt)
        .map_err(|error| error.to_string())?;
    let staging = File::from(
        openat2(
            &state,
            "staging",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| error.to_string())?,
    );
    let stages = DiskStageStore::open(&staging).map_err(|error| error.to_string())?;
    let stores =
        PreparationStores { generations: &generations, journals: &journals, stages: &stages };
    let completed = stores
        .complete_system_restore(&mut journal, |_| verify_systemd_health())
        .map_err(|error| error.to_string())?;
    Ok(format!(
        "system-restore-complete attempt={}; booted Btrfs/Snapper target and required systemd health checks passed; reboot to resume normal sessions",
        completed.restore_id.as_str(),
    ))
}

fn checkpoint_session_on_host(
    transaction_id: &Identifier,
    account_name: &str,
) -> Result<String, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("manual-checkpoint must run as root from the recovery console".into());
    }
    require_offline_recovery_console()?;
    let root = File::open("/").map_err(|error| error.to_string())?;
    let record = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    ryoku_recovery::state_provision::verify_initialized_state_root(&root, &record)
        .map_err(|error| error.to_string())?;
    let (state, _) = collect_and_open_state_mount(&root, record.identity())
        .map_err(|error| error.to_string())?;
    let state = File::from(state);
    let _mutation_guard = acquire_mutation_guard(&state)?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    if reader.list_transactions().map_err(|error| error.to_string())?.contains(transaction_id) {
        return Err("transaction ID is already present; choose a new ID".into());
    }
    let account = resolve_account_by_name(&root, account_name)?;
    stop_account_writers(account.uid)?;
    let home = open_checked_directory(&root, &account.home_path[1..], account.uid, account.gid)?;
    let config_path = format!("{}/.config/ryoku", account.home_path.trim_start_matches('/'));
    let config = open_checked_directory(&root, &config_path, account.uid, account.gid)?;
    let package_revisions = installed_ryoku_revisions()?;
    let profile_store = ryoku_recovery::installed_profiles::ProfileBundleStore::open_for_console(
        &state,
        &_mutation_guard,
    )
    .map_err(|error| format!("verified profile-bundle storage is unavailable: {error}"))?;
    let profile_bundle =
        ryoku_recovery::installed_profiles::InstalledProfileBundle::load_packaged_defaults(
            None, None,
        )
        .map_err(|error| format!("installed profile defaults are unavailable: {error}"))?;
    if !profile_bundle.matches_packages(&package_revisions) {
        return Err(
            "packaged profile defaults do not match the installed Ryoku package inventory".into()
        );
    }

    let request = ryoku_snapper::SnapshotPairRequest {
        config: ryoku_snapper::ConfigName::new("root").map_err(|error| format!("{error:?}"))?,
        transaction_id: ryoku_snapper::TransactionId::new(transaction_id.as_str())
            .map_err(|error| format!("{error:?}"))?,
    };
    let snapshot =
        ryoku_snapper::SnapperCheckpointBackend.create_single(&request).map_err(|error| {
            format!(
                "Snapper checkpoint failed; reconcile transaction tag {} before retrying: {error}",
                transaction_id.as_str(),
            )
        })?;
    let link = SnapshotLink::new(
        Identifier::new("root").map_err(|error| format!("{error:?}"))?,
        snapshot.as_str(),
        record.identity().filesystem_uuid.clone(),
    )
    .map_err(|error| format!("invalid native snapshot link: {error:?}"))?;
    let native = verify_new_root_checkpoint(&root, link).map_err(|error| error.to_string())?;
    let verified_bundle = profile_store
        .publish_candidate(transaction_id, &profile_bundle)
        .map_err(|error| format!("could not retain exact profile layers: {error}"))?;
    let generation = generation_id_for(transaction_id);
    let generations = GenerationStore::open_initialized_state_mount(record.identity())
        .map_err(|error| error.to_string())?;
    let principal =
        AccountPrincipal::from_account(account.uid, account.gid, &account.name, &account.home_path)
            .map_err(|error| format!("invalid account binding: {error:?}"))?;
    let inventory = InstalledInventory::for_home(&home).map_err(|error| error.to_string())?;
    let captured = capture_with_installed_profile_bundle(
        &generations,
        &inventory,
        InstalledRoots {
            account_home: Some(&home),
            user_configuration: &config,
            system_configuration: None,
            account: CaptureAccount { uid: account.uid, gid: account.gid, principal },
        },
        InstalledCaptureRequest {
            transaction_id: transaction_id.clone(),
            generation_id: generation.clone(),
            parent_generation_id: None,
            provider_revision: None,
            limits: Default::default(),
            snapshot: native.verified_link().clone(),
            created_utc: current_utc_timestamp()?,
            package_revisions: package_revisions.clone(),
            reason: ryoku_recovery::OperationReason::Other(
                Identifier::new("manual-checkpoint").expect("fixed reason ID is canonical"),
            ),
        },
        &verified_bundle,
    )
    .map_err(|error| {
        format!(
            "session checkpoint capture failed; retain Snapper snapshot {} and reconcile transaction {}: {error}",
            snapshot.as_str(),
            transaction_id.as_str(),
        )
    })?;
    if installed_ryoku_revisions()? != package_revisions {
        return Err(format!(
            "session generation {} is retained but Ryoku packages changed during capture; reconcile the checkpoint",
            captured.generation_id.as_str(),
        ));
    }
    ryoku_recovery::native_checkpoint::verify_historical_root_checkpoint(&root, native.receipt())
        .map_err(|error| {
        format!(
            "captured generation is retained but its Snapper partner changed; reconcile transaction {}: {error}",
            transaction_id.as_str(),
        )
    })?;
    ensure_cataloged_checkpoint(&root, &generations, &reader, &captured)?;
    ProtectedSnapperCatalog::open(&root)
        .and_then(|catalog| catalog.mark_manual(transaction_id))
        .map_err(|error| format!("manual checkpoint retention failed: {error}"))?;
    let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
    reader
        .open_historical_transaction(&root, transaction_id)
        .map_err(|error| format!("linked checkpoint read-back failed: {error}"))?;
    Ok(format!(
        "session-checkpoint-verified transaction={} generation={} snapshot=root:{} entries={} account_uid={}; scope=session preferences only; catalog=recorded manual_checkpoint=true; system_coverage=unverified",
        captured.transaction_id.as_str(),
        generation.as_str(),
        snapshot.as_str(),
        captured.files().len(),
        account.uid,
    ))
}

struct NamedAccount {
    uid: u32,
    gid: u32,
    name: String,
    home_path: String,
}

fn valid_account_name(name: &str) -> bool {
    (1..=256).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn resolve_account_by_name(root: &File, name: &str) -> Result<NamedAccount, String> {
    let passwd = File::from(
        openat2(
            root,
            "etc/passwd",
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| format!("open local account database: {error}"))?,
    );
    let stat = fstat(&passwd).map_err(|error| error.to_string())?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_mode & 0o022 != 0
        || stat.st_size < 0
        || stat.st_size > 1024 * 1024
    {
        return Err("local account database has unsafe metadata".into());
    }
    let mut bytes = Vec::new();
    passwd.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("local account database exceeds size limit".into());
    }
    let contents =
        std::str::from_utf8(&bytes).map_err(|_| "local account database is not UTF-8")?;
    let mut selected = None;
    for line in contents.lines() {
        let fields: Vec<_> = line.split(':').collect();
        if fields.len() != 7 || fields[0] != name {
            continue;
        }
        let (Ok(uid), Ok(gid)) = (fields[2].parse::<u32>(), fields[3].parse::<u32>()) else {
            return Err("selected account record is invalid".into());
        };
        if uid < 1000 || !fields[5].starts_with('/') {
            return Err("selected account is not an ordinary local user".into());
        }
        AccountPrincipal::from_account(uid, gid, fields[0], fields[5])
            .map_err(|_| "selected account identity is invalid")?;
        if selected.is_some() {
            return Err("selected account name is ambiguous".into());
        }
        selected = Some(NamedAccount {
            uid,
            gid,
            name: fields[0].to_owned(),
            home_path: fields[5].to_owned(),
        });
    }
    selected.ok_or_else(|| "selected ordinary account is not present locally".into())
}

fn generation_id_for(transaction: &Identifier) -> Identifier {
    let short_transaction: String = transaction.as_str().chars().take(40).collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    Identifier::new(format!("generation-{short_transaction}-{nanos}"))
        .expect("bounded transaction ID and timestamp form a valid generation ID")
}

fn installed_ryoku_revisions() -> Result<Vec<PackageRevision>, String> {
    ryoku_package_revisions(None)
}

/// Installer source manifests explicitly name every recognized RPM, including
/// absence. Ordinary recovery retains the original three required packages.
fn revisions_for_source(
    root: Option<&std::path::Path>,
    manifest: &ryoku_recovery::RecoveryManifest,
) -> Result<Vec<PackageRevision>, String> {
    if ryoku_recovery::installer_state::is_installer(&manifest.reason) {
        ryoku_recovery::installer_state::validate_inventory(manifest.package_revisions())?;
        ryoku_recovery::installer_state::query(root)
    } else {
        ryoku_package_revisions(root)
    }
}

const RPM_REVISION_FORMAT: &str = "%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\n";

fn ryoku_package_revisions(root: Option<&std::path::Path>) -> Result<Vec<PackageRevision>, String> {
    ["ryoku-desktop", "ryoku-desktop-runtime", "ryoku-profile-defaults"]
        .into_iter()
        .map(|name| {
            let mut command = Command::new("/usr/bin/timeout");
            command.env_clear().env("LC_ALL", "C").args([
                "--signal=TERM",
                "--kill-after=1s",
                "5s",
                "/usr/bin/rpm",
            ]);
            if let Some(root) = root {
                command.arg("--root").arg(root);
            }
            let output = command
                .args(["-q", "--qf", RPM_REVISION_FORMAT, name])
                .output()
                .map_err(|_| format!("package database query could not start: {name}"))?;
            if !output.status.success() || output.stdout.len() > 1024 {
                return Err(format!("required Ryoku package is unavailable: {name}"));
            }
            parse_installed_revision(name, &output.stdout)
        })
        .collect()
}

fn parse_installed_revision(name: &str, output: &[u8]) -> Result<PackageRevision, String> {
    let value =
        std::str::from_utf8(output).map_err(|_| format!("invalid RPM response for {name}"))?;
    let value = value.trim_end_matches('\n');
    if value.lines().count() != 1 || !value.starts_with(&format!("{name}-")) {
        return Err(format!("ambiguous RPM response for {name}"));
    }
    let revision = Revision::new(value)
        .map_err(|error| format!("invalid installed RPM revision: {error:?}"))?;
    Ok(PackageRevision {
        package_name: PackageName::new(name).map_err(|error| format!("{error:?}"))?,
        before_nevra: Some(revision.clone()),
        after_nevra: Some(revision),
        upstream_revision: None,
    })
}

fn current_utc_timestamp() -> Result<String, String> {
    let output = Command::new("/usr/bin/timeout")
        .env_clear()
        .args([
            "--signal=TERM",
            "--kill-after=1s",
            "3s",
            "/usr/bin/date",
            "-u",
            "+%Y-%m-%dT%H:%M:%SZ",
        ])
        .output()
        .map_err(|_| "UTC timestamp command could not start")?;
    if !output.status.success() || output.stdout.len() > 64 {
        return Err("UTC timestamp command failed".into());
    }
    let timestamp = String::from_utf8(output.stdout)
        .map_err(|_| "UTC timestamp is not UTF-8")?
        .trim_end()
        .to_owned();
    if timestamp.len() != 20 {
        return Err("UTC timestamp has invalid format".into());
    }
    Ok(timestamp)
}

fn verify_systemd_health() -> Result<(), String> {
    verify_loaded_admission_gate()?;
    for arguments in [
        &["is-active", "--quiet", "local-fs.target"][..],
        &["is-active", "--quiet", "systemd-logind.service"][..],
    ] {
        let status = Command::new("/usr/bin/timeout")
            .env_clear()
            .env("LC_ALL", "C")
            .env("PATH", "/usr/sbin:/usr/bin")
            .args(["--signal=TERM", "--kill-after=2s", "5s", "/usr/bin/systemctl"])
            .args(arguments)
            .status()
            .map_err(|_| "required systemd health check could not start")?;
        if !status.success() {
            return Err("required systemd health check failed".into());
        }
    }
    Ok(())
}

fn admit_session_on_host() -> Result<(), String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("graphical admission requires the root system service".into());
    }
    let root = File::open("/").map_err(|error| error.to_string())?;
    let machine = read_machine_state_record(&root).map_err(|error| error.to_string())?;
    let state = File::from(
        ryoku_recovery::state_mount::collect_and_open_state_mount_readonly(
            &root,
            machine.identity(),
        )
        .map_err(|error| error.to_string())?
        .0,
    );
    let _guard = acquire_shared_mutation_guard(&state)?;
    let journals = RestoreJournalStore::open_existing_state_directory_readonly(&state)
        .map_err(|error| error.to_string())?;
    ryoku_recovery::session_admission::require_complete_journal(&journals)
}

fn verify_loaded_admission_gate() -> Result<(), String> {
    let root = File::open("/").map_err(|error| error.to_string())?;
    ryoku_recovery::session_admission::verify_installed_units(&root)?;
    for property in ["Requires", "After"] {
        let output = recovery_command_output(
            "systemctl",
            &["show", "--value", "--property", property, "display-manager.service"],
            "5s",
        )?;
        if !output.status.success() || !admission_dependency_present(&output.stdout) {
            return Err("display manager does not load the required recovery admission gate; reload systemd units".into());
        }
    }
    Ok(())
}

fn admission_dependency_present(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| {
        text.split_ascii_whitespace()
            .any(|unit| unit == ryoku_recovery::session_admission::SERVICE_NAME)
    })
}

fn recovery_command_output(
    program: &str,
    arguments: &[&str],
    deadline: &str,
) -> Result<Output, String> {
    let executable = match program {
        "loginctl" => "/usr/bin/loginctl",
        "systemctl" => "/usr/bin/systemctl",
        _ => return Err("unsupported recovery utility".into()),
    };
    let mut child = Command::new("/usr/bin/timeout")
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin")
        .args(["--signal=TERM", "--kill-after=2s", deadline, executable])
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "account writer control utility could not start")?;

    let Some(stdout) = child.stdout.take() else {
        terminate_account_writer_utility(&mut child);
        return Err("account writer control stdout was unavailable".into());
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_account_writer_utility(&mut child);
        return Err("account writer control stderr was unavailable".into());
    };
    let stdout_reader = match thread::Builder::new()
        .name("ryoku-recovery-command-stdout".into())
        .spawn(move || read_capped_while_draining(stdout, 64 * 1024))
    {
        Ok(reader) => reader,
        Err(_) => {
            terminate_account_writer_utility(&mut child);
            return Err("account writer control stdout reader could not start".into());
        }
    };
    let stderr_reader = match thread::Builder::new()
        .name("ryoku-recovery-command-stderr".into())
        .spawn(move || read_capped_while_draining(stderr, 16 * 1024))
    {
        Ok(reader) => reader,
        Err(_) => {
            terminate_account_writer_utility(&mut child);
            let _ = stdout_reader.join();
            return Err("account writer control stderr reader could not start".into());
        }
    };

    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => {
            terminate_account_writer_utility(&mut child);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err("account writer control utility could not be reaped".into());
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "account writer control stdout reader failed")?
        .map_err(|_| "account writer control stdout could not be read")?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "account writer control stderr reader failed")?
        .map_err(|_| "account writer control stderr could not be read")?;

    if stdout.len() > 64 * 1024 || stderr.len() > 16 * 1024 {
        return Err("account writer control output exceeded its bound".into());
    }
    if matches!(status.code(), Some(124 | 137)) {
        return Err("account writer control utility timed out".into());
    }
    Ok(Output { status, stdout, stderr })
}

fn terminate_account_writer_utility(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Retain at most `limit + 1` bytes so the caller can detect overflow, while
/// continuing to drain the pipe and preventing a verbose child from blocking.
fn read_capped_while_draining(mut input: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut retained = Vec::with_capacity(limit.saturating_add(1));
    let mut buffer = [0_u8; 8192];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(retained);
        }
        let remaining = limit.saturating_add(1).saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..count.min(remaining)]);
    }
}

fn parse_session_uids(output: &str) -> Result<BTreeSet<u32>, String> {
    let mut uids = BTreeSet::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line == "No sessions." {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 3 {
            return Err("loginctl returned an unrecognized session row".into());
        }
        let uid =
            fields[1].parse::<u32>().map_err(|_| "loginctl returned an invalid session UID")?;
        uids.insert(uid);
    }
    Ok(uids)
}

/// Terminate every logind session and the lingering user manager for the
/// checkpointed account. A successful return means neither can write enrolled
/// preferences during snapshot capture or restore. This is called only from
/// the root recovery console while the common machine mutation lock is held.
fn stop_account_writers(uid: u32) -> Result<(), String> {
    stop_account_writers_with(uid, &mut SystemAccountWriterControl)
}

trait AccountWriterControl {
    fn session_uids(&mut self) -> Result<BTreeSet<u32>, String>;
    fn terminate_user(&mut self, uid: u32) -> Result<(), String>;
    fn user_manager_active(&mut self, uid: u32) -> Result<bool, String>;
    fn stop_user_manager(&mut self, uid: u32) -> Result<(), String>;
}

struct SystemAccountWriterControl;

impl AccountWriterControl for SystemAccountWriterControl {
    fn session_uids(&mut self) -> Result<BTreeSet<u32>, String> {
        let output = recovery_command_output(
            "loginctl",
            &["list-sessions", "--no-legend", "--no-pager"],
            "5s",
        )?;
        if !output.status.success() {
            return Err("could not enumerate logind sessions".into());
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|_| "loginctl session list is not UTF-8")?;
        parse_session_uids(text)
    }

    fn terminate_user(&mut self, uid: u32) -> Result<(), String> {
        let uid_text = uid.to_string();
        let output = recovery_command_output("loginctl", &["terminate-user", &uid_text], "15s")?;
        if output.status.success() {
            Ok(())
        } else {
            Err("logind could not terminate the checkpointed account".into())
        }
    }

    fn user_manager_active(&mut self, uid: u32) -> Result<bool, String> {
        let unit = format!("user@{uid}.service");
        let output = recovery_command_output("systemctl", &["is-active", "--quiet", &unit], "5s")?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(3 | 4) => Ok(false),
            _ => Err("checkpointed user-manager state is unknown".into()),
        }
    }

    fn stop_user_manager(&mut self, uid: u32) -> Result<(), String> {
        let unit = format!("user@{uid}.service");
        let output = recovery_command_output("systemctl", &["stop", &unit], "20s")?;
        if output.status.success() {
            Ok(())
        } else {
            Err("systemd could not stop the checkpointed user manager".into())
        }
    }
}

fn stop_account_writers_with(
    uid: u32,
    control: &mut impl AccountWriterControl,
) -> Result<(), String> {
    if control.session_uids()?.contains(&uid) {
        control.terminate_user(uid)?;
    }
    if control.user_manager_active(uid)? {
        control.stop_user_manager(uid)?;
    }
    if control.user_manager_active(uid)? {
        return Err("checkpointed user manager remains active".into());
    }
    if control.session_uids()?.contains(&uid) {
        return Err("checkpointed account still has a logind session".into());
    }
    Ok(())
}

fn stop_journal_account_writers(
    root: &File,
    generations: &GenerationStore,
    restore_id: &Identifier,
    expected_generation: &Identifier,
) -> Result<(), String> {
    let checkpoint =
        generations.open_verified_transaction(restore_id).map_err(|error| error.to_string())?;
    if checkpoint.manifest().generation_id != *expected_generation {
        return Err("restore journal current-state generation does not match its checkpoint".into());
    }
    let principal = checkpoint
        .manifest()
        .account_principal()
        .ok_or("restore checkpoint has no account binding")?;
    let account = resolve_local_account(root, principal)?;
    stop_account_writers(account.uid)
}

/// Keep recovery mutations out of a live desktop session. Rescue/emergency
/// targets stop the graphical session tree before any enrolled user files are
/// captured or replayed; a TTY on an otherwise graphical boot is insufficient.
fn require_offline_recovery_console() -> Result<(), String> {
    fn target_active(target: &str) -> Result<bool, String> {
        let status = Command::new("/usr/bin/timeout")
            .env_clear()
            .env("LC_ALL", "C")
            .args([
                "--signal=TERM",
                "--kill-after=1s",
                "5s",
                "/usr/bin/systemctl",
                "is-active",
                "--quiet",
                target,
            ])
            .status()
            .map_err(|_| "systemd recovery-target check could not start")?;
        match status.code() {
            Some(0) => Ok(true),
            Some(3) => Ok(false),
            _ => Err("systemd recovery-target check failed".into()),
        }
    }

    let recovery_target = target_active("rescue.target")? || target_active("emergency.target")?;
    let graphical_target = target_active("graphical.target")?;
    validate_recovery_target_state(recovery_target, graphical_target)?;
    verify_loaded_admission_gate()
}

fn validate_recovery_target_state(
    recovery_target: bool,
    graphical_target: bool,
) -> Result<(), String> {
    if !recovery_target || graphical_target {
        return Err(
            "recovery mutation requires rescue.target or emergency.target with graphical.target inactive".into(),
        );
    }
    Ok(())
}

fn open_restore_roots(
    trusted_root: &File,
    manifests: &[&RecoveryManifest],
) -> Result<Vec<RestoreRoot>, String> {
    let needed: BTreeSet<_> = manifests
        .iter()
        .flat_map(|manifest| manifest.files().iter().map(|entry| entry.root))
        .filter(|root| *root != StateRoot::StateGeneration)
        .collect();
    let needs_account = needed.iter().any(|root| {
        matches!(
            root,
            StateRoot::UserRyokuConfig | StateRoot::UserHome | StateRoot::UserRyokuProfile
        )
    });
    let mut principal = None;
    for candidate in manifests.iter().filter_map(|manifest| manifest.account_principal()) {
        if principal.is_some_and(|current| current != candidate) {
            return Err("source and checkpoint identify different user accounts".into());
        }
        principal = Some(candidate);
    }
    let account = if needs_account {
        Some(resolve_local_account(
            trusted_root,
            principal.ok_or("generation has user state but no authenticated account binding")?,
        )?)
    } else {
        None
    };
    let mut roots = Vec::new();
    for root in needed {
        let directory = match root {
            StateRoot::EtcRyoku => open_checked_directory(trusted_root, "etc/ryoku", 0, 0)?,
            StateRoot::UserRyokuConfig => {
                let account = account.as_ref().ok_or("user account binding unavailable")?;
                open_checked_directory(&account.home, ".config/ryoku", account.uid, account.gid)?
            }
            StateRoot::UserHome => account
                .as_ref()
                .ok_or("user account binding unavailable")?
                .home
                .try_clone()
                .map_err(|error| error.to_string())?,
            StateRoot::UserRyokuProfile => {
                return Err("legacy profile root has no approved installed path mapping".into());
            }
            StateRoot::StateGeneration => continue,
        };
        let paths = manifests
            .last()
            .ok_or("checkpoint manifest unavailable")?
            .files()
            .iter()
            .filter(|entry| entry.root == root)
            .map(|entry| entry.path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        roots.push(RestoreRoot { root, directory, paths });
    }
    Ok(roots)
}

struct LocalAccount {
    uid: u32,
    gid: u32,
    home: File,
}

fn resolve_local_account(root: &File, expected: &AccountPrincipal) -> Result<LocalAccount, String> {
    let passwd = File::from(
        openat2(
            root,
            "etc/passwd",
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| format!("open local account database: {error}"))?,
    );
    let stat = fstat(&passwd).map_err(|error| error.to_string())?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != 0
        || stat.st_mode & 0o022 != 0
        || stat.st_size < 0
        || stat.st_size > 1024 * 1024
    {
        return Err("local account database has unsafe metadata".into());
    }
    let mut bytes = Vec::new();
    passwd.take(1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("local account database exceeds size limit".into());
    }
    let passwd = std::str::from_utf8(&bytes).map_err(|_| "local account database is not UTF-8")?;
    let mut matched = None;
    for line in passwd.lines() {
        let fields: Vec<_> = line.split(':').collect();
        if fields.len() != 7 {
            continue;
        }
        let (Ok(uid), Ok(gid)) = (fields[2].parse::<u32>(), fields[3].parse::<u32>()) else {
            continue;
        };
        let Ok(principal) = AccountPrincipal::from_account(uid, gid, fields[0], fields[5]) else {
            continue;
        };
        if &principal == expected {
            if matched.is_some() {
                return Err("account binding matches multiple local users".into());
            }
            matched = Some((uid, gid, fields[5].to_owned()));
        }
    }
    let (uid, gid, path) = matched.ok_or("bound user account is not present locally")?;
    let home = open_checked_directory(root, &path[1..], uid, gid)?;
    Ok(LocalAccount { uid, gid, home })
}

fn open_checked_directory(
    root: &File,
    relative_path: &str,
    uid: u32,
    gid: u32,
) -> Result<File, String> {
    let directory = File::from(
        openat2(
            root,
            relative_path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| format!("open restore root: {error}"))?,
    );
    let stat = fstat(&directory).map_err(|error| error.to_string())?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != uid
        || stat.st_gid != gid
        || stat.st_mode & 0o022 != 0
    {
        return Err(
            "restore root ownership or permissions do not match the sealed inventory".into()
        );
    }
    Ok(directory)
}

fn acquire_mutation_guard(state: &File) -> Result<File, String> {
    let lock = File::from(
        openat2(
            state,
            ryoku_recovery::state_mount::MUTATION_LOCK_NAME,
            OFlags::RDWR | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| format!("open protected mutation lock: {error}"))?,
    );
    let metadata = fstat(&lock).map_err(|error| error.to_string())?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_uid != 0
        || metadata.st_gid != 0
        || metadata.st_mode & 0o7777 != 0o600
        || metadata.st_nlink != 1
    {
        return Err("protected mutation lock has unsafe identity or permissions".into());
    }
    acquire_exclusive_lock(&lock)?;
    Ok(lock)
}

/// Hold a coherent read view while assembling catalog and Snapper partner
/// observations. Shared readers may continue, while integrated mutations that
/// use the common exclusive lock cannot change the linked rows mid-report.
fn acquire_shared_mutation_guard(state: &File) -> Result<File, String> {
    let lock = File::from(
        openat2(
            state,
            ryoku_recovery::state_mount::MUTATION_LOCK_NAME,
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| format!("open protected mutation lock: {error}"))?,
    );
    let metadata = fstat(&lock).map_err(|error| error.to_string())?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_uid != 0
        || metadata.st_mode & 0o7777 != 0o600
        || metadata.st_nlink != 1
    {
        return Err("protected mutation lock has unsafe identity or permissions".into());
    }
    acquire_shared_lock(&lock)?;
    Ok(lock)
}

fn acquire_shared_lock(lock: &File) -> Result<(), String> {
    flock(lock, FlockOperation::NonBlockingLockShared).map_err(|error| {
        format!(
            "protected mutation lock is unavailable (an exclusive Ryoku operation is active): {error}"
        )
    })
}

fn acquire_exclusive_lock(lock: &File) -> Result<(), String> {
    flock(lock, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
        format!(
            "protected mutation lock is unavailable (close active Ryoku sessions or wait for the current mutation): {error}"
        )
    })
}

fn scope_name(scope: RestoreScope) -> &'static str {
    match scope {
        RestoreScope::ConfigurationOnly => "configuration-only",
        RestoreScope::SystemOnly => "system-only",
        RestoreScope::CompleteLinked => "complete-linked",
    }
}

fn configuration_action(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::Absent => "remove-if-present",
        EntryKind::Directory { .. } => "restore-directory-metadata",
        EntryKind::RegularFile { .. } => "restore-file",
        EntryKind::Symlink { .. } => "restore-symlink",
    }
}

fn write_configuration_preview(entries: &[FileRecord], output: &mut impl Write) -> io::Result<()> {
    for entry in entries.iter().filter(|entry| {
        matches!(
            entry.root,
            StateRoot::EtcRyoku
                | StateRoot::UserRyokuConfig
                | StateRoot::UserHome
                | StateRoot::UserRyokuProfile
        )
    }) {
        writeln!(
            output,
            "configuration-entry root={} path={} action={}",
            entry.root.manifest_name(),
            entry.path.as_str(),
            configuration_action(&entry.kind),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryoku_recovery::RelativePath;

    #[test]
    fn installed_rpm_query_uses_a_real_newline_and_rejects_ambiguous_revisions() {
        assert!(RPM_REVISION_FORMAT.ends_with('\n'));
        assert!(!RPM_REVISION_FORMAT.contains('\\'));
        let parsed =
            parse_installed_revision("ryoku-desktop", b"ryoku-desktop-0:0.1.0-5.x86_64\n").unwrap();
        assert_eq!(parsed.after_nevra.unwrap().as_str(), "ryoku-desktop-0:0.1.0-5.x86_64");
        for output in [
            b"ryoku-desktop-0:0.1.0-5.x86_64\\n".as_slice(),
            b"ryoku-desktop-0:0.1.0-5.x86_64\nryoku-desktop-0:0.1.0-6.x86_64\n",
            b"other-0:0.1.0-5.x86_64\n",
            b"\xff",
        ] {
            assert!(parse_installed_revision("ryoku-desktop", output).is_err());
        }
    }

    #[test]
    fn rejects_unknown_commands_without_partial_output() {
        let mut output = Vec::new();
        assert!(execute(["restore".into()], &mut output).is_err());
        assert!(String::from_utf8(output).unwrap().is_empty());
    }

    #[test]
    fn parses_logind_session_uids_and_fails_closed_on_unknown_rows() {
        let sessions =
            parse_session_uids("c1 1001 tester seat0 tty2\nc2 1002 guest - pts/0\n").unwrap();
        assert_eq!(sessions, BTreeSet::from([1001, 1002]));
        assert!(parse_session_uids("No sessions.\n").unwrap().is_empty());
        assert!(parse_session_uids("unexpected output\n").is_err());
        assert!(parse_session_uids("c1 not-a-uid tester seat0 tty2\n").is_err());
    }

    #[test]
    fn recovery_command_output_retains_only_limit_plus_one_bytes() {
        let output =
            read_capped_while_draining(std::io::Cursor::new(vec![b'x'; 128 * 1024]), 32).unwrap();
        assert_eq!(output.len(), 33);
    }

    #[derive(Default)]
    struct FakeWriterControl {
        sessions: BTreeSet<u32>,
        manager_active: bool,
        calls: Vec<&'static str>,
        leave_session: bool,
        leave_manager: bool,
    }

    impl AccountWriterControl for FakeWriterControl {
        fn session_uids(&mut self) -> Result<BTreeSet<u32>, String> {
            self.calls.push("sessions");
            Ok(self.sessions.clone())
        }

        fn terminate_user(&mut self, uid: u32) -> Result<(), String> {
            assert_eq!(uid, 1001);
            self.calls.push("terminate");
            if !self.leave_session {
                self.sessions.remove(&uid);
            }
            Ok(())
        }

        fn user_manager_active(&mut self, _uid: u32) -> Result<bool, String> {
            self.calls.push("active");
            Ok(self.manager_active)
        }

        fn stop_user_manager(&mut self, uid: u32) -> Result<(), String> {
            assert_eq!(uid, 1001);
            self.calls.push("stop");
            if !self.leave_manager {
                self.manager_active = false;
            }
            Ok(())
        }
    }

    #[test]
    fn account_writers_are_stopped_and_verified_in_order() {
        let mut control = FakeWriterControl {
            sessions: BTreeSet::from([1001]),
            manager_active: true,
            ..FakeWriterControl::default()
        };
        stop_account_writers_with(1001, &mut control).unwrap();
        assert_eq!(
            control.calls,
            ["sessions", "terminate", "active", "stop", "active", "sessions"]
        );
    }

    #[test]
    fn account_writer_shutdown_fails_if_sessions_or_manager_remain() {
        let mut session_remains = FakeWriterControl {
            sessions: BTreeSet::from([1001]),
            leave_session: true,
            ..FakeWriterControl::default()
        };
        assert!(
            stop_account_writers_with(1001, &mut session_remains)
                .unwrap_err()
                .contains("still has a logind session")
        );

        let mut manager_remains = FakeWriterControl {
            manager_active: true,
            leave_manager: true,
            ..FakeWriterControl::default()
        };
        assert!(
            stop_account_writers_with(1001, &mut manager_remains)
                .unwrap_err()
                .contains("user manager remains active")
        );
    }

    #[test]
    fn confirm_rollback_validates_id_and_reports_the_verified_transition() {
        let mut output = Vec::new();
        let mut observed = None;
        let result = execute_with(
            ["confirm-rollback".into(), "restore-17".into()],
            &mut output,
            |restore_id| {
                observed = Some(restore_id.as_str().to_owned());
                Ok("rollback-target-verified; recovery gate remains closed".into())
            },
        );
        assert!(result.is_ok());
        assert_eq!(observed.as_deref(), Some("restore-17"));
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "rollback-target-verified; recovery gate remains closed\n"
        );

        let mut called = false;
        let result =
            execute_with(["confirm-rollback".into(), "../bad".into()], &mut Vec::new(), |_| {
                called = true;
                Ok(String::new())
            });
        assert!(result.unwrap_err().starts_with("invalid restore ID:"));
        assert!(!called);
    }

    #[test]
    fn restore_configuration_rejects_malformed_ids_before_opening_protected_state() {
        let mut output = Vec::new();
        let error =
            execute(["restore-configuration".into(), "../unsafe".into()], &mut output).unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());
    }

    #[test]
    fn discard_incomplete_stage_rejects_malformed_ids_before_opening_protected_state() {
        let mut output = Vec::new();
        let error = execute(["discard-incomplete-stage".into(), "../unsafe".into()], &mut output)
            .unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());
    }

    #[test]
    fn prepare_configuration_rejects_malformed_ids_before_opening_protected_state() {
        let mut output = Vec::new();
        let error = execute(
            ["prepare-configuration".into(), "../unsafe".into(), "source-1".into()],
            &mut output,
        )
        .unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());

        let error = execute(
            ["prepare-configuration".into(), "restore-1".into(), "../unsafe".into()],
            &mut output,
        )
        .unwrap_err();
        assert!(error.starts_with("invalid source transaction ID:"));
        assert!(output.is_empty());
    }

    #[test]
    fn system_restore_commands_reject_malformed_ids_before_opening_protected_state() {
        let mut output = Vec::new();
        let error =
            execute(["prepare-system".into(), "../unsafe".into(), "source-1".into()], &mut output)
                .unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());

        let error =
            execute(["prepare-system".into(), "restore-1".into(), "../unsafe".into()], &mut output)
                .unwrap_err();
        assert!(error.starts_with("invalid source transaction ID:"));
        assert!(output.is_empty());

        let error =
            execute(["rollback-system".into(), "../unsafe".into()], &mut output).unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());

        let error =
            execute(["prepare-linked".into(), "../unsafe".into(), "source-1".into()], &mut output)
                .unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());

        let error =
            execute(["prepare-linked".into(), "restore-1".into(), "../unsafe".into()], &mut output)
                .unwrap_err();
        assert!(error.starts_with("invalid source transaction ID:"));
        assert!(output.is_empty());

        let error =
            execute(["rollback-linked".into(), "../unsafe".into()], &mut output).unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());
    }

    #[test]
    fn recovery_mutations_require_rescue_or_emergency_without_graphical_target() {
        assert!(validate_recovery_target_state(true, false).is_ok());
        assert!(validate_recovery_target_state(false, false).is_err());
        assert!(validate_recovery_target_state(true, true).is_err());
    }

    #[test]
    fn restore_inventory_matches_current_capture_scope_and_rejects_other_home_files() {
        let record = |path: &str, kind| {
            FileRecord::new(StateRoot::UserHome, RelativePath::new(path).unwrap(), kind).unwrap()
        };
        for path in [
            ".config/ryoku/shell.json",
            ".config/ryoku/opensuse-profile.json",
            ".local/state/ryoku/flags.json",
            ".config/hypr/user.lua",
            ".config/ryoku/plugins.json",
        ] {
            assert!(session_checkpoint_entry_kind_allowed(&record(path, EntryKind::Absent)));
        }
        for path in ["Documents/private", ".ssh/id_ed25519", ".config/ryoku/unreviewed.json"] {
            assert!(
                FileRecord::new(
                    StateRoot::UserHome,
                    RelativePath::new(path).unwrap(),
                    EntryKind::Absent,
                )
                .is_err()
            );
        }
        assert!(!session_checkpoint_entry_kind_allowed(&record(
            ".config/ryoku/shell.json",
            EntryKind::Directory { metadata: test_metadata() }
        )));
        assert!(!session_checkpoint_entry_kind_allowed(&record(
            ".config/ryoku/opensuse-profile.json",
            EntryKind::Symlink { metadata: test_metadata(), target: "private-profile".into() }
        )));
        assert!(session_checkpoint_entry_kind_allowed(&record(
            ".config/ryoku/shell.json",
            EntryKind::Symlink { metadata: test_metadata(), target: "other-settings".into() }
        )));
    }

    #[test]
    fn admission_dependency_requires_an_exact_loaded_unit_token() {
        assert!(admission_dependency_present(
            b"local-fs.target ryoku-recovery-admission.service\n"
        ));
        assert!(!admission_dependency_present(b"other-ryoku-recovery-admission.service"));
        assert!(!admission_dependency_present(b""));
        assert!(!admission_dependency_present(&[0xff]));
    }

    #[test]
    fn complete_system_rejects_malformed_ids_before_opening_protected_state() {
        let mut output = Vec::new();
        let error =
            execute(["complete-system".into(), "../unsafe".into()], &mut output).unwrap_err();
        assert!(error.starts_with("invalid restore ID:"));
        assert!(output.is_empty());
    }

    #[test]
    fn checkpoint_session_validates_arguments_before_opening_protected_state() {
        let mut output = Vec::new();
        let error = execute(
            ["checkpoint-session".into(), "safe-id".into(), "bad/name".into()],
            &mut output,
        )
        .unwrap_err();
        assert_eq!(error, "invalid local account name");
        assert!(output.is_empty());
    }

    #[test]
    fn manual_checkpoint_alias_validates_arguments_before_opening_protected_state() {
        let mut output = Vec::new();
        let error =
            execute(["manual-checkpoint".into(), "safe-id".into(), "bad/name".into()], &mut output)
                .unwrap_err();
        assert_eq!(error, "invalid local account name");
        assert!(output.is_empty());
    }

    #[test]
    fn checkpoint_generation_id_stays_canonical_for_long_transaction_ids() {
        let transaction = Identifier::new(format!("restore-{}", "x".repeat(80))).unwrap();
        let generation = generation_id_for(&transaction);
        assert!(generation.as_str().starts_with("generation-"));
        assert!(generation.as_str().len() <= 96);
    }

    #[test]
    fn mutation_lock_fails_fast_while_a_shared_read_lease_is_held() {
        let path = std::env::temp_dir().join(format!(
            "ryoku-recovery-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        let shared = File::options().read(true).write(true).create_new(true).open(&path).unwrap();
        let exclusive = File::open(&path).unwrap();
        flock(&shared, FlockOperation::LockShared).unwrap();
        let error = acquire_exclusive_lock(&exclusive).unwrap_err();
        assert!(error.contains("protected mutation lock is unavailable"));
        drop(shared);
        acquire_exclusive_lock(&exclusive).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn admission_shared_lock_fails_fast_during_an_exclusive_mutation() {
        let path = std::env::temp_dir().join(format!(
            "ryoku-retention-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        let exclusive =
            File::options().read(true).write(true).create_new(true).open(&path).unwrap();
        let shared = File::open(&path).unwrap();
        flock(&exclusive, FlockOperation::LockExclusive).unwrap();
        let error = acquire_shared_lock(&shared).unwrap_err();
        assert!(error.contains("exclusive Ryoku operation is active"));
        drop(exclusive);
        acquire_shared_lock(&shared).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn restored_json_health_check_reopens_declared_files_and_validates_profile_schema() {
        let path = std::env::temp_dir().join(format!(
            "ryoku-recovery-cli-health-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir(&path).unwrap();
        let profile =
            r#"{"schema_version":1,"id":"generic-desktop","settings":{"desktop":"hyprland"}}"#;
        std::fs::write(path.join(ryoku_profiles::LOCAL_PROFILE_FILE), profile).unwrap();
        let directory = File::open(&path).unwrap();
        let roots =
            [RestoreRoot { root: StateRoot::UserRyokuConfig, directory, paths: Vec::new() }];
        let files = [(
            StateRoot::UserRyokuConfig,
            RelativePath::new(ryoku_profiles::LOCAL_PROFILE_FILE).unwrap(),
            profile.len(),
        )];
        assert!(validate_restored_json(&files, &roots).is_ok());
        std::fs::write(path.join(ryoku_profiles::LOCAL_PROFILE_FILE), b"{invalid").unwrap();
        assert!(validate_restored_json(&files, &roots).is_err());
        assert!(is_json_object(br#"{"theme":"Tokyo Night"}"#));
        assert!(!is_json_object(b"[]"));
        assert!(!is_json_object(b"true"));
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn configuration_restore_completion_requires_systemd_and_recovery_gate_health() {
        let path = std::env::temp_dir().join(format!(
            "ryoku-recovery-health-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("theme.json"), b"not-json").unwrap();
        let roots = [RestoreRoot {
            root: StateRoot::UserRyokuConfig,
            directory: File::open(&path).unwrap(),
            paths: Vec::new(),
        }];
        let files = [(
            StateRoot::UserRyokuConfig,
            RelativePath::new("theme.json").unwrap(),
            b"not-json".len(),
        )];
        let checked = std::cell::Cell::new(false);
        assert!(
            verify_configuration_restore_health(&files, &roots, || {
                checked.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!checked.get());

        std::fs::write(path.join("theme.json"), br#"{"theme":"Tokyo Night"}"#).unwrap();
        let files =
            [(StateRoot::UserRyokuConfig, files[0].1.clone(), br#"{"theme":"Tokyo Night"}"#.len())];
        let checked = std::cell::Cell::new(false);
        assert!(
            verify_configuration_restore_health(&files, &roots, || {
                checked.set(true);
                Err("required systemd health check failed".into())
            })
            .is_err()
        );
        assert!(checked.get());

        assert!(verify_configuration_restore_health(&files, &roots, || Ok(())).is_ok());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn cli_does_not_accept_an_arbitrary_store_path() {
        let mut output = Vec::new();
        assert!(execute(["list".into(), "/tmp".into()], &mut output).is_err());
        assert!(String::from_utf8(output).unwrap().is_empty());
    }

    #[test]
    fn linked_verification_rejects_malformed_ids_and_path_overrides() {
        let mut output = Vec::new();
        let result = execute(["verify-linked".into(), "../outside".into()], &mut output);
        assert!(result.unwrap_err().starts_with("invalid transaction ID:"));
        assert_eq!(
            execute(["verify-linked".into(), "txn-1".into(), "/tmp".into()], &mut output),
            Err(USAGE.into())
        );
        assert!(output.is_empty());
    }

    #[test]
    fn preview_maps_entries_to_non_content_action_labels() {
        assert_eq!(configuration_action(&EntryKind::Absent), "remove-if-present");
        assert_eq!(
            configuration_action(&EntryKind::Directory { metadata: test_metadata() }),
            "restore-directory-metadata"
        );
        assert_eq!(
            configuration_action(&EntryKind::RegularFile {
                metadata: test_metadata(),
                size: 0,
                sha256: ryoku_recovery::Sha256::new("0".repeat(64)).unwrap(),
                hard_link_group: None,
            }),
            "restore-file"
        );
        assert_eq!(
            configuration_action(&EntryKind::Symlink {
                metadata: test_metadata(),
                target: "private-target".into(),
            }),
            "restore-symlink"
        );
    }

    #[test]
    fn configuration_preview_filters_generation_and_never_prints_saved_targets() {
        let entries = vec![
            FileRecord::new(
                StateRoot::UserRyokuConfig,
                RelativePath::new("theme.json").unwrap(),
                EntryKind::Symlink { metadata: test_metadata(), target: "private-target".into() },
            )
            .unwrap(),
            FileRecord::new(
                StateRoot::StateGeneration,
                RelativePath::new("manifest.json").unwrap(),
                EntryKind::Absent,
            )
            .unwrap(),
        ];
        let mut output = Vec::new();
        write_configuration_preview(&entries, &mut output).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("root=user-ryoku-config path=theme.json action=restore-symlink"));
        assert!(!output.contains("private-target"));
        assert!(!output.contains("manifest.json"));
    }

    fn test_metadata() -> ryoku_recovery::EntryMetadata {
        ryoku_recovery::EntryMetadata {
            uid: 1000,
            gid: 1000,
            mode: 0o600,
            mtime_unix_ns: 0,
            acl_sha256: None,
            extended: Default::default(),
        }
    }
}
