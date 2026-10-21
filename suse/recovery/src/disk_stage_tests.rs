use super::*;

#[path = "configuration_apply_tests.rs"]
mod configuration_apply_tests;
#[path = "preparation_tests.rs"]
mod preparation_tests;
use crate::Identifier;
use crate::{
    OperationReason, RecoveryManifest, SnapshotLink, VerificationStatus,
    capture::{CaptureLimits, capture_inventory},
    disk_stage::{DiskStageError, DiskStageStore},
    generation::{GenerationStore, Payloads},
    inode_metadata::prepare_inode_metadata,
};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    path: PathBuf,
    generation: GenerationStore,
    source: StagedConfiguration,
}
impl Fixture {
    fn new() -> Self {
        Self::with_extra(&[], |_| {})
    }
    fn with_extra(extra: &[&str], populate: impl FnOnce(&Path)) -> Self {
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
fn interrupted_stage_publication_retains_source_and_requires_explicit_repair() {
    for boundary in 1..=4 {
        let fixture = Fixture::new();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fixture
                .store()
                .create_with_hook(&fixture.source, |step| {
                    if step == boundary {
                        panic!("simulate interrupted stage publication");
                    }
                })
                .unwrap();
        }));
        assert!(interrupted.is_err());
        assert!(
            fixture
                .generation
                .open_verified_transaction(&fixture.source.plan().transaction_id,)
                .is_ok()
        );
        assert_eq!(fs::read(fixture.path.join("unrelated")).unwrap(), b"home sentinel");
        if boundary < 4 {
            assert!(fixture.store().open_verified(&fixture.source).is_err());
            assert!(fixture.store().create(&fixture.source).is_err());
            fixture.store().discard_incomplete(&fixture.source).unwrap();
            fixture.store().create(&fixture.source).unwrap();
        }
        fixture.store().open_verified(&fixture.source).unwrap();
    }
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
