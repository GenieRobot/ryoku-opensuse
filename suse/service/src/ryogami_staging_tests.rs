use super::*;
use std::{
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    sync::{Arc, atomic::AtomicU64},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    store: StagingDirectory,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryogami-stage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = StagingDirectory::from_owned_directory(
            File::open(&path).unwrap(),
            rustix::process::geteuid().as_raw(),
            rustix::process::getegid().as_raw(),
        )
        .unwrap();
        Self { path, store }
    }
    fn names(&self) -> Vec<String> {
        let mut result = std::fs::read_dir(&self.path)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        result.sort();
        result
    }
    fn input(&self, bytes: &[u8]) -> File {
        // Anonymous input is outside the staged namespace, with no retained
        // fixture path a provider could use to replace a reviewed destination.
        let path = self.path.with_extension("input");
        std::fs::write(&path, bytes).unwrap();
        let file = File::open(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        file
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
fn identity(bytes: &[u8]) -> ContentIdentity {
    ContentIdentity { sha256: format!("{:x}", Sha256::digest(bytes)), bytes: bytes.len() as u64 }
}
fn quota() -> Quota {
    Quota { transfer_bytes: 1024, directory_bytes: 4096, duration: Duration::from_secs(1) }
}

#[test]
fn successful_transfer_is_private_atomic_verified_and_never_replaces() {
    let fixture = Fixture::new();
    let data = b"small deterministic wallpaper payload";
    let expected = identity(data);
    let plan = fixture.store.plan(expected.clone(), quota()).unwrap();
    assert!(fixture.names().is_empty(), "planning wrote a file");
    assert_eq!(
        fixture.store.transfer(&plan, &fixture.input(data), &AtomicBool::new(false)).unwrap(),
        expected
    );
    fixture.store.verify(&expected).unwrap();
    assert_eq!(fixture.names(), std::slice::from_ref(&expected.sha256));
    let metadata = std::fs::metadata(fixture.path.join(&expected.sha256)).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    assert!(matches!(fixture.store.plan(expected, quota()), Err(StageError::Collision)));
}

#[test]
fn oversized_truncated_wrong_hash_and_cancelled_leave_no_payload() {
    for (input, expected, cancelled, kind) in [
        (b"oversized".as_slice(), identity(b"short"), false, "quota"),
        (b"tiny".as_slice(), identity(b"expected longer"), false, "identity"),
        (b"wrong".as_slice(), identity(b"right"), false, "identity"),
        (b"right".as_slice(), identity(b"right"), true, "cancelled"),
    ] {
        let fixture = Fixture::new();
        let plan = fixture.store.plan(expected, quota()).unwrap();
        let error = fixture
            .store
            .transfer(&plan, &fixture.input(input), &AtomicBool::new(cancelled))
            .unwrap_err();
        assert!(
            matches!(
                (&error, kind),
                (StageError::Quota, "quota")
                    | (StageError::Identity, "identity")
                    | (StageError::Cancelled, "cancelled")
            ),
            "{error}"
        );
        assert!(fixture.names().is_empty());
    }
}

#[test]
fn nonblocking_stream_has_real_timeout_and_cancellation() {
    for cancel in [false, true] {
        let fixture = Fixture::new();
        let (input, _writer) = UnixStream::pair().unwrap();
        input.set_nonblocking(true).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let thread = std::thread::spawn(move || {
            if cancel {
                std::thread::sleep(Duration::from_millis(30));
                signal.store(true, Ordering::Release);
            }
        });
        let mut limit = quota();
        limit.duration = Duration::from_millis(150);
        let plan = fixture.store.plan(identity(b"expected"), limit).unwrap();
        let before = Instant::now();
        let error = fixture.store.transfer(&plan, &input, &cancelled).unwrap_err();
        thread.join().unwrap();
        assert!(matches!(
            (cancel, error),
            (false, StageError::TimedOut) | (true, StageError::Cancelled)
        ));
        assert!(before.elapsed() < Duration::from_secs(2));
        assert!(fixture.names().is_empty());
    }
}

#[test]
fn stream_success_is_bounded_without_large_media_or_network() {
    let fixture = Fixture::new();
    let (input, mut writer) = UnixStream::pair().unwrap();
    input.set_nonblocking(true).unwrap();
    let data = vec![42; 512];
    let expected = identity(&data);
    let worker = std::thread::spawn(move || {
        for chunk in data.chunks(17) {
            writer.write_all(chunk).unwrap();
        }
    });
    let plan = fixture.store.plan(expected.clone(), quota()).unwrap();
    fixture.store.transfer(&plan, &input, &AtomicBool::new(false)).unwrap();
    worker.join().unwrap();
    fixture.store.verify(&expected).unwrap();
}

#[test]
fn blocking_stream_is_rejected_before_creating_staging_file() {
    let fixture = Fixture::new();
    let (input, _writer) = UnixStream::pair().unwrap();
    let plan = fixture.store.plan(identity(b"test"), quota()).unwrap();
    assert!(matches!(
        fixture.store.transfer(&plan, &input, &AtomicBool::new(false)),
        Err(StageError::UnsafeInput)
    ));
    assert!(fixture.names().is_empty());
}

#[test]
fn changed_plan_and_full_directory_refuse_before_writing() {
    let fixture = Fixture::new();
    let plan = fixture.store.plan(identity(b"next"), quota()).unwrap();
    let old = identity(b"existing");
    let name = fixture.path.join(&old.sha256);
    std::fs::write(&name, b"existing").unwrap();
    std::fs::set_permissions(&name, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        fixture.store.transfer(&plan, &fixture.input(b"next"), &AtomicBool::new(false)),
        Err(StageError::Changed)
    ));
    let mut limit = quota();
    limit.transfer_bytes = 10;
    limit.directory_bytes = 10;
    assert!(matches!(fixture.store.plan(identity(b"next"), limit), Err(StageError::Quota)));
    assert_eq!(fixture.names(), [old.sha256]);
}

#[test]
fn symlinks_hardlinks_foreign_files_and_unsafe_modes_refuse() {
    for variant in ["symlink", "hardlink", "name", "mode", "directory"] {
        let fixture = Fixture::new();
        let unrelated = fixture.path.with_extension("unrelated");
        std::fs::write(&unrelated, b"private sentinel").unwrap();
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o600)).unwrap();
        let name = fixture.path.join("a".repeat(64));
        match variant {
            "symlink" => std::os::unix::fs::symlink(&unrelated, &name).unwrap(),
            "hardlink" => std::fs::hard_link(&unrelated, &name).unwrap(),
            "name" => std::fs::write(fixture.path.join("credentials"), b"private").unwrap(),
            "directory" => std::fs::create_dir(&name).unwrap(),
            _ => {
                std::fs::write(&name, b"x").unwrap();
                std::fs::set_permissions(&name, std::fs::Permissions::from_mode(0o666)).unwrap();
            }
        }
        assert!(fixture.store.plan(identity(b"next"), quota()).is_err(), "{variant}");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"private sentinel");
        std::fs::remove_file(unrelated).unwrap();
    }
}

#[test]
fn interrupted_partial_content_counts_against_quota_and_is_not_silently_removed() {
    let fixture = Fixture::new();
    let part = format!(".part-{}", "a".repeat(64));
    let path = fixture.path.join(&part);
    std::fs::write(&path, [0; 1024]).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut limit = quota();
    limit.directory_bytes = 1024;
    assert!(matches!(fixture.store.plan(identity(b"next"), limit), Err(StageError::Quota)));
    assert_eq!(fixture.names(), [part]);
}

#[test]
fn bad_root_and_invalid_quota_are_read_only_failures() {
    let fixture = Fixture::new();
    for limit in [
        Quota { transfer_bytes: 0, ..quota() },
        Quota { directory_bytes: 1, ..quota() },
        Quota { duration: Duration::ZERO, ..quota() },
    ] {
        assert!(matches!(fixture.store.plan(identity(b"next"), limit), Err(StageError::Quota)));
    }
    std::fs::set_permissions(&fixture.path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(fixture.store.plan(identity(b"next"), quota()), Err(StageError::Directory)));
    assert!(fixture.names().is_empty());
}

#[test]
fn final_readback_detects_content_replacement() {
    let fixture = Fixture::new();
    let expected = identity(b"original");
    let plan = fixture.store.plan(expected.clone(), quota()).unwrap();
    fixture.store.transfer(&plan, &fixture.input(b"original"), &AtomicBool::new(false)).unwrap();
    std::fs::write(fixture.path.join(&expected.sha256), b"replaced").unwrap();
    assert!(matches!(fixture.store.verify(&expected), Err(StageError::Identity)));
}

fn checkpoint(number: u64) -> crate::mutation_gate::VerifiedLinkedGeneration {
    use crate::mutation_gate::{CheckpointEvidence, LinkedGeneration, VerifiedLinkedGeneration};
    use ryoku_recovery::{Identifier, Sha256, SnapshotLink};
    VerifiedLinkedGeneration::validate(CheckpointEvidence {
        linked: LinkedGeneration {
            transaction_id: Identifier::new(format!("fixture-transaction-{number}")).unwrap(),
            generation_id: Identifier::new(format!("fixture-generation-{number}")).unwrap(),
            manifest_sha256: Sha256::new(format!("{number:064x}")).unwrap(),
            snapshot: SnapshotLink::new(
                Identifier::new("root").unwrap(),
                number.to_string(),
                "11111111-1111-1111-1111-111111111111",
            )
            .unwrap(),
        },
        system_component_verified: true,
        ryoku_component_verified: true,
        contents_and_metadata_verified: true,
    })
    .unwrap()
}
fn installed(fixture: &Fixture) -> ContentIdentity {
    let content = identity(b"recoverable local payload");
    let plan = fixture.store.plan(content.clone(), quota()).unwrap();
    fixture
        .store
        .transfer(&plan, &fixture.input(b"recoverable local payload"), &AtomicBool::new(false))
        .unwrap();
    content
}
fn trash_quota() -> Quota {
    Quota { directory_bytes: 32 * 1024, ..quota() }
}
fn operation() -> crate::OperationId {
    crate::OperationId::new("fixture-delete-1").unwrap()
}

#[test]
fn trash_and_restore_preserve_payload_and_receipt_with_distinct_checkpoint() {
    let fixture = Fixture::new();
    let content = installed(&fixture);
    let before = std::fs::metadata(fixture.path.join(&content.sha256)).unwrap();
    let plan = fixture.store.plan_trash(operation(), content.clone(), trash_quota()).unwrap();
    let receipt = fixture.store.trash(&plan, &checkpoint(42), &AtomicBool::new(false)).unwrap();
    assert_eq!(fixture.store.observe_trash(&receipt).unwrap(), TrashObservation::Trashed);
    assert!(!fixture.path.join(&content.sha256).exists());
    let restore = fixture.store.plan_restore(receipt.clone()).unwrap();
    assert!(matches!(
        fixture.store.restore(&restore, &checkpoint(42), &AtomicBool::new(false)),
        Err(StageError::Changed)
    ));
    assert_eq!(
        fixture.store.restore(&restore, &checkpoint(43), &AtomicBool::new(false)).unwrap(),
        content
    );
    assert_eq!(fixture.store.observe_trash(&receipt).unwrap(), TrashObservation::OriginalPresent);
    fixture.store.verify(&content).unwrap();
    let after = std::fs::metadata(fixture.path.join(&content.sha256)).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        (before.ino(), before.mode(), before.uid(), before.gid()),
        (after.ino(), after.mode(), after.uid(), after.gid())
    );
    assert!(fixture.path.join(format!(".receipt-{}", receipt.token)).exists());
}

#[test]
fn cancelled_delete_and_stale_plan_leave_original_untouched() {
    let fixture = Fixture::new();
    let content = installed(&fixture);
    let plan = fixture.store.plan_trash(operation(), content.clone(), trash_quota()).unwrap();
    assert!(matches!(
        fixture.store.trash(&plan, &checkpoint(42), &AtomicBool::new(true)),
        Err(StageError::Cancelled)
    ));
    assert_eq!(fixture.names(), std::slice::from_ref(&content.sha256));
    std::fs::write(fixture.path.join(&content.sha256), b"changed content same path").unwrap();
    assert!(fixture.store.trash(&plan, &checkpoint(42), &AtomicBool::new(false)).is_err());
    assert_eq!(fixture.names(), [content.sha256]);
}

#[test]
fn missing_or_tampered_receipt_never_restores_or_deletes_anything() {
    for mutation in ["missing", "tampered", "symlink"] {
        let fixture = Fixture::new();
        let content = installed(&fixture);
        let plan = fixture.store.plan_trash(operation(), content, trash_quota()).unwrap();
        let receipt = fixture.store.trash(&plan, &checkpoint(42), &AtomicBool::new(false)).unwrap();
        let path = fixture.path.join(format!(".receipt-{}", receipt.token));
        std::fs::remove_file(&path).unwrap();
        match mutation {
            "tampered" => std::fs::write(&path, b"forged secret receipt").unwrap(),
            "symlink" => std::os::unix::fs::symlink("/etc/passwd", &path).unwrap(),
            _ => (),
        }
        assert!(fixture.store.plan_restore(receipt.clone()).is_err());
        assert_eq!(
            std::fs::read(fixture.path.join(format!(".trash-{}", receipt.token))).unwrap(),
            b"recoverable local payload"
        );
    }
}

#[test]
fn restore_never_overwrites_a_new_file_at_original_name() {
    let fixture = Fixture::new();
    let content = installed(&fixture);
    let plan = fixture.store.plan_trash(operation(), content.clone(), trash_quota()).unwrap();
    let receipt = fixture.store.trash(&plan, &checkpoint(42), &AtomicBool::new(false)).unwrap();
    let restore = fixture.store.plan_restore(receipt.clone()).unwrap();
    std::fs::write(fixture.path.join(&content.sha256), b"new private sentinel").unwrap();
    assert!(fixture.store.restore(&restore, &checkpoint(43), &AtomicBool::new(false)).is_err());
    assert_eq!(std::fs::read(fixture.path.join(&content.sha256)).unwrap(), b"new private sentinel");
    assert!(fixture.path.join(format!(".trash-{}", receipt.token)).exists());
}

#[test]
fn interrupted_move_has_truthful_observation_and_recoverable_local_state() {
    let fixture = Fixture::new();
    let content = installed(&fixture);
    let plan = fixture.store.plan_trash(operation(), content.clone(), trash_quota()).unwrap();
    let receipt = fixture.store.trash(&plan, &checkpoint(42), &AtomicBool::new(false)).unwrap();
    // Recreate the exact crash window after durable receipt and before rename.
    std::fs::rename(
        fixture.path.join(format!(".trash-{}", receipt.token)),
        fixture.path.join(&content.sha256),
    )
    .unwrap();
    assert_eq!(fixture.store.observe_trash(&receipt).unwrap(), TrashObservation::OriginalPresent);
    assert!(matches!(fixture.store.plan_restore(receipt), Err(StageError::Collision)));
    fixture.store.verify(&content).unwrap();
}

#[test]
fn invalid_checkpoint_evidence_cannot_produce_a_trash_execution_proof() {
    use crate::mutation_gate::{CheckpointEvidence, VerifiedLinkedGeneration};
    let fixture = Fixture::new();
    let content = installed(&fixture);
    let proof = VerifiedLinkedGeneration::validate(CheckpointEvidence {
        linked: checkpoint(42).linked().clone(),
        system_component_verified: false,
        ryoku_component_verified: true,
        contents_and_metadata_verified: true,
    });
    assert!(proof.is_err());
    assert_eq!(fixture.names(), std::slice::from_ref(&content.sha256));
    fixture.store.verify(&content).unwrap();
}
