use super::*;

#[path = "configuration_apply_tests.rs"]
mod configuration_apply_tests;
#[path = "preparation_tests.rs"]
mod preparation_tests;
use crate::Identifier;
use crate::{
    OperationReason, RecoveryManifest, SnapshotLink, VerificationStatus,
    capture::{CaptureLimits, capture_inventory},
    configuration_stage::stage_configuration,
    disk_stage::{DiskStageError, DiskStageStore},
    generation::{GenerationStore, Payloads},
    inode_metadata::prepare_inode_metadata,
    restore::{RestorePlan, RestoreScope},
};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    path: PathBuf,
    generation: GenerationStore,
    source: StagedConfiguration,
}
impl Fixture {
    fn reopen(path: PathBuf) -> Self {
        let generation =
            GenerationStore::open_for_test(&File::open(path.join("generation")).unwrap()).unwrap();
        let verified = generation
            .open_verified_transaction(&Identifier::new("transaction-disk-stage").unwrap())
            .unwrap();
        let plan = RestorePlan {
            source_manifest_sha256: verified.manifest().manifest_sha256.clone().unwrap(),
            transaction_id: verified.manifest().transaction_id.clone(),
            generation_id: verified.manifest().generation_id.clone(),
            snapper_config: verified.manifest().snapper_config.clone(),
            snapshot_id: verified.manifest().snapper_snapshot_id.clone(),
            filesystem_uuid: verified.manifest().filesystem_uuid.clone(),
            scope: RestoreScope::ConfigurationOnly,
            configuration_entries: verified.manifest().files().len() as u32,
        };
        let source = stage_configuration(&verified, &plan).unwrap();
        Self { path, generation, source }
    }

    fn new() -> Self {
        Self::with_extra(&[], |_| {})
    }
    fn with_extra(extra: &[&str], populate: impl FnOnce(&Path)) -> Self {
        Self::build(extra, populate, false)
    }
    fn with_state_preferences() -> Self {
        Self::build(&[], |_| {}, true)
    }
    fn build(extra: &[&str], populate: impl FnOnce(&Path), state_preferences: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-disk-stage-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        for name in ["source", "generation", "stages"] {
            fs::create_dir(path.join(name)).unwrap();
            fs::set_permissions(path.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(path.join("source/a"), b"saved configuration").unwrap();
        fs::set_permissions(path.join("source/a"), fs::Permissions::from_mode(0o640)).unwrap();
        fs::hard_link(path.join("source/a"), path.join("source/b")).unwrap();
        fs::create_dir(path.join("source/directory")).unwrap();
        symlink("a", path.join("source/link")).unwrap();
        if state_preferences {
            for directory in [".local", ".local/state", ".local/state/ryoku"] {
                fs::create_dir_all(path.join("source").join(directory)).unwrap();
                fs::set_permissions(
                    path.join("source").join(directory),
                    fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
            fs::write(
                path.join("source/.local/state/ryoku/flags.json"),
                br#"{"dnd":true,"keepAwake":false,"keepAwakeSince":0,"gameMode":false,"gameDndPrev":false}"#,
            )
            .unwrap();
            fs::set_permissions(
                path.join("source/.local/state/ryoku/flags.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            fs::write(
                path.join("source/.local/state/ryoku/stage-walls.json"),
                br#"{"current":"/home/alice/Pictures/wall.jpg","walls":{"/home/alice/Pictures/wall.jpg":{"effect":{"enabled":true,"depth":0.4},"layers":[]}}}"#,
            )
            .unwrap();
            fs::set_permissions(
                path.join("source/.local/state/ryoku/stage-walls.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            fs::write(
                path.join("source/shell.json"),
                br#"{"theme":{"theme":"Tokyo Night"},"wallpaper":{"enabled":false}}"#,
            )
            .unwrap();
            fs::set_permissions(path.join("source/shell.json"), fs::Permissions::from_mode(0o644))
                .unwrap();
            fs::write(
                path.join("source/matugen.json"),
                br#"{"mode":"dark","contrast":0.0,"appTheme":false}"#,
            )
            .unwrap();
            fs::set_permissions(
                path.join("source/matugen.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            fs::create_dir(path.join("source/profile")).unwrap();
            fs::set_permissions(path.join("source/profile"), fs::Permissions::from_mode(0o700))
                .unwrap();
            fs::write(
                path.join("source/profile.json"),
                br#"{"hero":{"kind":"custom","source":"hero.png"}}"#,
            )
            .unwrap();
            fs::set_permissions(
                path.join("source/profile.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            fs::write(path.join("source/profile/hero.png"), b"saved custom hero bytes").unwrap();
            fs::set_permissions(
                path.join("source/profile/hero.png"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            fs::write(path.join("source/profile/unselected.png"), b"source-only unselected asset")
                .unwrap();
        }
        fs::write(path.join("unrelated"), b"home sentinel").unwrap();
        populate(&path.join("source"));
        let paths: Vec<_> = ["a", "b", "directory", "link", "missing"]
            .into_iter()
            .chain(extra.iter().copied())
            .map(|name| crate::RelativePath::new(name).unwrap())
            .collect();
        let inventory = capture_inventory(
            &File::open(path.join("source")).unwrap(),
            StateRoot::EtcRyoku,
            &paths,
            CaptureLimits::default(),
        )
        .unwrap();
        let mut manifest = RecoveryManifest::new(
            Identifier::new("transaction-disk-stage").unwrap(),
            Identifier::new("generation-disk-stage").unwrap(),
            None,
            SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "00000000-0000-0000-0000-000000000000",
            )
            .unwrap(),
            OperationReason::ConfigurationRestore,
            "2026-10-15T00:00:00Z",
        )
        .unwrap();
        let mut payloads: Payloads = inventory.metadata_payloads;
        for record in inventory.records {
            if let EntryKind::RegularFile { sha256, .. } = &record.kind {
                payloads.insert(
                    sha256.clone(),
                    fs::read(path.join("source").join(record.path.as_str())).unwrap(),
                );
            }
            manifest.add_file(record).unwrap();
        }
        if state_preferences {
            let state_paths: Vec<_> = [
                ".local",
                ".local/state",
                ".local/state/ryoku",
                ".local/state/ryoku/flags.json",
                ".local/state/ryoku/stage-walls.json",
            ]
            .into_iter()
            .map(|name| crate::RelativePath::new(name).unwrap())
            .collect();
            let state_inventory = capture_inventory(
                &File::open(path.join("source")).unwrap(),
                StateRoot::UserRyokuState,
                &state_paths,
                CaptureLimits::default(),
            )
            .unwrap();
            for (digest, bytes) in state_inventory.metadata_payloads {
                payloads.insert(digest, bytes);
            }
            for record in state_inventory.records {
                if let EntryKind::RegularFile { sha256, .. } = &record.kind {
                    payloads.insert(
                        sha256.clone(),
                        fs::read(path.join("source").join(record.path.as_str())).unwrap(),
                    );
                }
                manifest.add_file(record).unwrap();
            }
            let settings_paths = [
                crate::RelativePath::new("shell.json").unwrap(),
                crate::RelativePath::new("matugen.json").unwrap(),
                crate::RelativePath::new("profile.json").unwrap(),
                crate::RelativePath::new("profile").unwrap(),
                crate::RelativePath::new("profile/hero.png").unwrap(),
            ];
            let settings_inventory = capture_inventory(
                &File::open(path.join("source")).unwrap(),
                StateRoot::UserRyokuConfig,
                &settings_paths,
                CaptureLimits::default(),
            )
            .unwrap();
            for (digest, bytes) in settings_inventory.metadata_payloads {
                payloads.insert(digest, bytes);
            }
            for record in settings_inventory.records {
                if let EntryKind::RegularFile { sha256, .. } = &record.kind {
                    payloads.insert(
                        sha256.clone(),
                        fs::read(path.join("source").join(record.path.as_str())).unwrap(),
                    );
                }
                manifest.add_file(record).unwrap();
            }
        }
        manifest.set_verification(
            VerificationStatus::Verified,
            VerificationStatus::Verified,
            VerificationStatus::Verified,
        );
        manifest.seal().unwrap();
        let generation =
            GenerationStore::open_for_test(&File::open(path.join("generation")).unwrap()).unwrap();
        generation.publish(&manifest, &payloads).unwrap();
        let plan = RestorePlan {
            source_manifest_sha256: manifest.manifest_sha256.clone().unwrap(),
            transaction_id: manifest.transaction_id.clone(),
            generation_id: manifest.generation_id.clone(),
            snapper_config: manifest.snapper_config.clone(),
            snapshot_id: manifest.snapper_snapshot_id.clone(),
            filesystem_uuid: manifest.filesystem_uuid.clone(),
            scope: RestoreScope::ConfigurationOnly,
            configuration_entries: manifest.files().len() as u32,
        };
        let verified = generation.open_verified_transaction(&manifest.transaction_id).unwrap();
        let source = stage_configuration(&verified, &plan).unwrap();
        Self { path, generation, source }
    }
    fn store(&self) -> DiskStageStore {
        DiskStageStore::open_for_test(&File::open(self.path.join("stages")).unwrap()).unwrap()
    }
    fn stage(&self) -> PathBuf {
        self.path.join("stages/stage-transaction-disk-stage")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    writable(&entry.path());
                }
            }
        }
        writable(&self.path);
        fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn stage_capture_process_exit_never_adopts_incomplete_objects() {
    const CHILD_TEST: &str =
        "configuration_stage::disk_stage_tests::stage_capture_process_exit_child";

    for boundary in 1..=4 {
        let fixture = Fixture::new();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
            .env("RYOKU_STAGE_CHILD_ROOT", &fixture.path)
            .env("RYOKU_STAGE_CHILD_BOUNDARY", boundary.to_string())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77), "child missed boundary {boundary}");

        let store = fixture.store();
        let reopened = store.open_verified(&fixture.source);
        if boundary == 4 {
            assert!(reopened.is_ok(), "completed stage was not verified");
            assert!(store.discard_incomplete(&fixture.source).is_err());
            assert!(store.open_verified(&fixture.source).is_ok());
        } else {
            assert!(reopened.is_err(), "incomplete stage was accepted at {boundary}");
            assert!(matches!(
                store.create(&fixture.source),
                Err(DiskStageError::Filesystem(rustix::io::Errno::EXIST))
            ));
            store.discard_incomplete(&fixture.source).unwrap();
            assert!(!fixture.stage().exists());
            let created = store.create(&fixture.source).unwrap();
            drop(created);
            assert!(store.open_verified(&fixture.source).is_ok());
        }
    }
}

#[test]
#[ignore = "spawned by stage_capture_process_exit_never_adopts_incomplete_objects"]
fn stage_capture_process_exit_child() {
    let (Ok(root), Ok(boundary)) =
        (std::env::var("RYOKU_STAGE_CHILD_ROOT"), std::env::var("RYOKU_STAGE_CHILD_BOUNDARY"))
    else {
        return;
    };
    let boundary = boundary.parse::<usize>().unwrap();
    let fixture = Fixture::reopen(PathBuf::from(root));
    let store = fixture.store();
    let _ = store.create_with_hook(&fixture.source, |step| {
        if step == boundary {
            // Leave only synchronized fixture state for the parent to inspect.
            std::process::exit(77);
        }
    });
    panic!("disk stage creation returned before the selected boundary");
}

fn leave_populated_stage_without_receipt(fixture: &Fixture) {
    let store = fixture.store();
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = store.create_with_hook(&fixture.source, |boundary| {
            if boundary == 2 {
                panic!("simulated interruption before publishing the stage receipt");
            }
        });
    }));
    assert!(interrupted.is_err());
}

#[test]
fn incomplete_stage_discard_refuses_unknown_objects_and_symlink_substitutions() {
    let fixture = Fixture::new();
    let stage = fixture.stage();
    fs::create_dir(&stage).unwrap();
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(stage.join("unexpected"), b"retain unexpected data").unwrap();
    let store = fixture.store();
    assert!(matches!(
        store.discard_incomplete(&fixture.source),
        Err(DiskStageError::IncompleteOrChanged)
    ));
    assert_eq!(fs::read(stage.join("unexpected")).unwrap(), b"retain unexpected data");
    fs::remove_file(stage.join("unexpected")).unwrap();
    fs::remove_dir(&stage).unwrap();

    fs::create_dir(&stage).unwrap();
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
    let outside = fixture.path.join("unrelated");
    symlink(&outside, stage.join("object-000")).unwrap();
    assert!(matches!(
        store.discard_incomplete(&fixture.source),
        Err(DiskStageError::IncompleteOrChanged)
    ));
    assert_eq!(fs::read(&outside).unwrap(), b"home sentinel");
    assert!(stage.join("object-000").is_symlink());
}

#[test]
fn incomplete_stage_discard_refuses_external_hardlinks_modes_and_populated_directories() {
    let fixture = Fixture::new();
    leave_populated_stage_without_receipt(&fixture);
    let outside_link = fixture.path.join("unrelated-link");
    fs::hard_link(fixture.stage().join("object-000"), &outside_link).unwrap();
    assert!(fixture.store().discard_incomplete(&fixture.source).is_err());
    assert!(fixture.stage().join("object-000").exists());
    assert!(outside_link.exists());
    drop(fixture);

    let fixture = Fixture::new();
    leave_populated_stage_without_receipt(&fixture);
    fs::set_permissions(fixture.stage().join("object-000"), fs::Permissions::from_mode(0o600))
        .unwrap();
    assert!(fixture.store().discard_incomplete(&fixture.source).is_err());
    assert!(fixture.stage().join("object-000").exists());
    drop(fixture);

    let fixture = Fixture::new();
    leave_populated_stage_without_receipt(&fixture);
    let directory = fixture.stage().join("object-002");
    fs::write(directory.join("unexpected-child"), b"retain child").unwrap();
    assert!(fixture.store().discard_incomplete(&fixture.source).is_err());
    assert_eq!(fs::read(directory.join("unexpected-child")).unwrap(), b"retain child");
}

#[test]
fn disk_stage_reopens_from_durable_generation_with_links_metadata_and_absence() {
    let fixture = Fixture::new();
    let created = fixture.store().create(&fixture.source).unwrap();
    assert_eq!(created.plan(), fixture.source.plan());
    drop(created);
    let generation = fixture
        .generation
        .open_verified_transaction(&fixture.source.plan().transaction_id)
        .unwrap();
    let resumed = stage_configuration(&generation, fixture.source.plan()).unwrap();
    assert_eq!(fixture.store().open_verified(&resumed).unwrap().plan(), resumed.plan());
    assert_eq!(
        fs::metadata(fixture.stage().join("object-000")).unwrap().ino(),
        fs::metadata(fixture.stage().join("object-001")).unwrap().ino()
    );
    assert_eq!(fs::read_link(fixture.stage().join("object-003")).unwrap(), Path::new("a"));
    assert!(!fixture.stage().join("object-004").exists());
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
}

#[test]
fn missing_completion_is_never_adopted_or_overwritten() {
    let fixture = Fixture::new();
    fixture.store().create(&fixture.source).unwrap();
    fs::remove_file(fixture.stage().join("complete.json")).unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
    assert!(fixture.store().create(&fixture.source).is_err());
    assert!(!fixture.stage().join("complete.json").exists());
}

#[test]
fn changed_content_and_unexpected_objects_invalidate_completed_stage() {
    let fixture = Fixture::new();
    fixture.store().create(&fixture.source).unwrap();
    fs::write(fixture.stage().join("unexpected"), b"extra").unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
    fs::remove_file(fixture.stage().join("unexpected")).unwrap();
    fs::write(fixture.stage().join("object-000"), b"tampered").unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
}

#[test]
fn matching_bytes_and_metadata_do_not_substitute_for_hard_links() {
    let fixture = Fixture::new();
    fixture.store().create(&fixture.source).unwrap();
    let alias = fixture.stage().join("object-001");
    fs::remove_file(&alias).unwrap();
    fs::write(&alias, fixture.source.entries()[1].content().unwrap()).unwrap();
    let file = OpenOptions::new().read(true).write(true).open(&alias).unwrap();
    prepare_inode_metadata(&file, &fixture.source.entries()[1]).unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
}

#[test]
fn receipt_substitution_and_symlinked_stage_directory_fail_without_following() {
    let fixture = Fixture::new();
    fixture.store().create(&fixture.source).unwrap();
    let receipt = fixture.stage().join("complete.json");
    fs::remove_file(&receipt).unwrap();
    symlink(fixture.path.join("unrelated"), &receipt).unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
    fs::rename(fixture.stage(), fixture.path.join("retained-stage")).unwrap();
    symlink(fixture.path.join("retained-stage"), fixture.stage()).unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
}

#[test]
fn changed_source_receipt_and_injected_absent_slot_are_rejected() {
    let fixture = Fixture::new();
    fixture.store().create(&fixture.source).unwrap();
    let verified = fixture
        .generation
        .open_verified_transaction(&fixture.source.plan().transaction_id)
        .unwrap();
    let mut changed = stage_configuration(&verified, fixture.source.plan()).unwrap();
    changed.plan.source_manifest_sha256 = Sha256::new("f".repeat(64)).unwrap();
    assert!(fixture.store().open_verified(&changed).is_err());
    fs::write(fixture.stage().join("object-004"), b"should be absent").unwrap();
    assert!(fixture.store().open_verified(&fixture.source).is_err());
}

#[test]
fn unsafe_symlink_target_is_not_followed_or_credited_as_complete() {
    let fixture = Fixture::new();
    let before = fs::metadata(fixture.path.join("unrelated")).unwrap();
    let verified = fixture
        .generation
        .open_verified_transaction(&fixture.source.plan().transaction_id)
        .unwrap();
    let mut source = stage_configuration(&verified, fixture.source.plan()).unwrap();
    if let EntryKind::Symlink { target, .. } = &mut source.entries[3].record.kind {
        *target = fixture.path.join("unrelated").to_str().unwrap().to_owned();
    }
    assert!(fixture.store().create(&source).is_err());
    assert!(!fixture.stage().join("complete.json").exists());
    assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
    let after = fs::metadata(fixture.path.join("unrelated")).unwrap();
    assert_eq!(
        (after.uid(), after.gid(), after.mode(), after.mtime(), after.mtime_nsec()),
        (before.uid(), before.gid(), before.mode(), before.mtime(), before.mtime_nsec())
    );
}

#[test]
fn expanded_payload_budget_is_checked_before_creating_a_stage_directory() {
    let fixture = Fixture::new();
    let verified = fixture
        .generation
        .open_verified_transaction(&fixture.source.plan().transaction_id)
        .unwrap();
    let mut source = stage_configuration(&verified, fixture.source.plan()).unwrap();
    // One shared 16-MiB blob fits the generation store, but five aliases
    // exceed the capture/realization budget when counted by declared path.
    let content: Arc<[u8]> = vec![0_u8; 16 * 1024 * 1024].into();
    let template = &source.entries[0];
    let entries = (0..5)
        .map(|index| {
            let mut record = template.record.clone();
            record.path = crate::RelativePath::new(format!("alias-{index}")).unwrap();
            if let EntryKind::RegularFile { size, .. } = &mut record.kind {
                *size = content.len() as u64;
            }
            StagedEntry {
                record,
                content: Some(Arc::clone(&content)),
                acl: template.acl.clone(),
                xattrs: template.xattrs.clone(),
            }
        })
        .collect();
    source.entries = entries;
    assert!(matches!(
        fixture.store().create(&source),
        Err(DiskStageError::Capture(crate::capture::CaptureError::TotalCaptureTooLarge))
    ));
    assert!(!fixture.stage().exists());
}

#[test]
fn staging_store_requires_private_directory() {
    let fixture = Fixture::new();
    fs::set_permissions(fixture.path.join("stages"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        DiskStageStore::open_for_test(&File::open(fixture.path.join("stages")).unwrap()),
        Err(DiskStageError::InvalidStore)
    ));
}
