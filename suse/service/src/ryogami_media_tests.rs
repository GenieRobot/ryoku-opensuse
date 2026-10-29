use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    home: File,
    account: CaptureAccount,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-media-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        let account = CaptureAccount {
            uid,
            gid,
            principal: ryoku_recovery::AccountPrincipal::from_account(
                uid,
                gid,
                "fixture",
                path.to_str().unwrap(),
            )
            .unwrap(),
        };
        let home = File::open(&path).unwrap();
        Self { path, home, account }
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.path.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn setup(&self) -> serde_json::Value {
        self.write(
            ".config/ryoku/ryogami.json",
            br#"{"sources":{"pexels":{"apiKey":"private-fixture"}}}"#,
        );
        self.write("Pictures/Wallpapers/fixture.png", b"synthetic media");
        self.write(".cache/ryogami/wallpaper/thumbs/fixture.jpg", b"synthetic thumbnail");
        self.write(".cache/ryogami/wallpaper/anim/fixture.mp4", b"synthetic clip");
        let row = serde_json::json!({"key":"fixture","name":"fixture.png","type":"static","path":"~/Pictures/Wallpapers/fixture.png","thumb":"~/.cache/ryogami/wallpaper/thumbs/fixture.jpg","thumb_sm":"~/.cache/ryogami/wallpaper/thumbs-sm/fixture.jpg","video_file":"~/.cache/ryogami/wallpaper/anim/fixture.mp4","future":9007199254740993u64});
        self.catalog(&row);
        row
    }
    fn catalog(&self, row: &serde_json::Value) {
        self.write(CATALOG,&serde_json::to_vec(&serde_json::json!({"fixture":row,"other":{"key":"other","private":"must preserve"}})).unwrap());
    }
    fn prepare(&self) -> Result<PreparedMedia> {
        PreparedMedia::delete(
            &self.home,
            &self.account,
            None,
            "fixture",
            Sha256::new("a".repeat(64)).unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
#[test]
fn media_selection_enrolls_only_exact_source_cache_generated_names_and_absence() {
    let f = Fixture::new();
    let row = f.setup();
    f.write("private-sentinel", b"private");
    let catalog = std::fs::read(f.path.join(CATALOG)).unwrap();
    let plan = f.prepare().unwrap();
    assert_eq!(plan.moves().len(), 4);
    assert_eq!(plan.scope().paths().len(), 9);
    assert_eq!(plan.row, row);
    assert_eq!(plan.commit.mutation, CatalogMutation::Remove { key: "fixture".into() });
    assert!(plan.inventory.entries().iter().any(
        |entry| entry.path.home_relative().unwrap().as_str()
            == ".cache/ryogami/wallpaper/thumbs-sm/fixture.jpg"
            && entry.kind == EntryKind::Absent
    ));
    assert!(
        !plan.scope.paths().iter().any(|path| path
            .home_relative()
            .unwrap()
            .as_str()
            .contains("private"))
    );
    assert_eq!(std::fs::read(f.path.join(CATALOG)).unwrap(), catalog);
    assert_eq!(plan.fingerprint().unwrap(), f.prepare().unwrap().fingerprint().unwrap());
    f.write("Pictures/Wallpapers/fixture.png", b"changed");
    assert_ne!(plan.fingerprint().unwrap(), f.prepare().unwrap().fingerprint().unwrap());
}
#[test]
fn media_selection_rejects_links_arbitrary_cache_paths_injected_sources_and_stage_collisions() {
    for field in ["path", "thumb", "thumb_sm", "video_file"] {
        let f = Fixture::new();
        let mut row = f.setup();
        row[field] = "~/.config/ryoku/ryogami.json".into();
        f.catalog(&row);
        assert!(f.prepare().is_err());
    }
    for fault in ["link", "collision", "workshop", "key", "hardlink"] {
        let f = Fixture::new();
        let mut row = f.setup();
        match fault {
            "link" => {
                std::fs::remove_file(f.path.join("Pictures/Wallpapers/fixture.png")).unwrap();
                std::os::unix::fs::symlink(
                    "../../.config/ryoku/ryogami.json",
                    f.path.join("Pictures/Wallpapers/fixture.png"),
                )
                .unwrap();
            }
            "collision" => f.write(&format!(".ryoku-media-trash-{}-0", "a".repeat(64)), b"retain"),
            "workshop" => {
                row["type"] = "we".into();
                f.catalog(&row);
            }
            "key" => {
                row["key"] = "other".into();
                f.catalog(&row);
            }
            _ => std::fs::hard_link(
                f.path.join("Pictures/Wallpapers/fixture.png"),
                f.path.join("alias"),
            )
            .unwrap(),
        }
        assert!(f.prepare().is_err(), "{fault}");
    }
}
#[test]
fn media_namespaces_cannot_capture_catalog_credentials_or_unbounded_trash_names() {
    for name in
        ["index.json", "state.json", "thumbs/../../.config/secret", "thumbs/sub/leaf", "other/file"]
    {
        assert!(
            RelativePath::new(name)
                .ok()
                .and_then(|path| AssetPath::new(AssetNamespace::WallpaperCache, path).ok())
                .is_none()
        );
    }
    for name in [
        ".ryoku-media-trash-any-0",
        &format!(".ryoku-media-trash-{}-4", "a".repeat(64)),
        &format!(".ryoku-media-trash-{}-00", "a".repeat(64)),
    ] {
        assert!(
            AssetPath::new(AssetNamespace::MediaTrash, RelativePath::new(name).unwrap()).is_err()
        );
    }
}

fn media_proof(
    operation: &crate::OperationId,
    post: bool,
) -> crate::mutation_gate::VerifiedLinkedGeneration {
    use crate::mutation_gate::{CheckpointEvidence, LinkedGeneration, VerifiedLinkedGeneration};
    let number = if post { 2 } else { 1 };
    VerifiedLinkedGeneration::validate(CheckpointEvidence {
        linked: LinkedGeneration {
            transaction_id: ryoku_recovery::Identifier::new(format!(
                "{}-{}",
                operation.as_str(),
                if post { "post" } else { "pre" }
            ))
            .unwrap(),
            generation_id: ryoku_recovery::Identifier::new(format!("generation-{number}")).unwrap(),
            manifest_sha256: Sha256::new(format!("{number:064x}")).unwrap(),
            snapshot: ryoku_recovery::SnapshotLink::new(
                ryoku_recovery::Identifier::new("root").unwrap(),
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
fn media_history(
    directory: &std::path::Path,
    operation: &crate::OperationId,
    complete: bool,
) -> crate::OperationJournal {
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(directory.join(if complete { "complete-history" } else { "partial-history" }))
        .unwrap();
    let mut journal = crate::OperationJournal::open(file.try_clone().unwrap()).unwrap();
    append_media_history(&mut journal, operation, complete);
    drop(journal);
    crate::OperationJournal::open(file).unwrap()
}
fn append_media_history(
    journal: &mut crate::OperationJournal,
    operation: &crate::OperationId,
    complete: bool,
) {
    use crate::OperationPhase as P;
    use crate::mutation_gate::{MutationContext, MutationStage as S, PlannedChange};
    let stages = [
        (S::Preflight, P::Accepted),
        (S::StateChecking, P::Capturing),
        (S::Capturing, P::Capturing),
        (S::Revalidating, P::Capturing),
        (S::Applying, P::Applying),
        (S::HealthChecking, P::Verifying),
        (S::PostChangeCapture, P::Verifying),
        (S::Complete, P::Succeeded),
    ];
    for (index, (stage, phase)) in stages.into_iter().enumerate() {
        if !complete && index >= 5 {
            break;
        }
        journal
            .record_mutation(
                crate::OperationStatus {
                    operation_id: operation.clone(),
                    action: crate::ManagedActionKind::DeleteMedia,
                    phase,
                    progress: None,
                    error: None,
                },
                MutationContext {
                    plan: PlannedChange {
                        operation_key: "media-delete".into(),
                        plan_fingerprint: "a".repeat(64),
                        affected_state_fingerprint: "b".repeat(64),
                    },
                    checkpoint: (index >= 3)
                        .then(|| media_proof(operation, false).linked().clone()),
                    post_checkpoint: (stage == S::Complete)
                        .then(|| media_proof(operation, true).linked().clone()),
                    stage,
                    outcome: None,
                    installer_restore: None,
                },
            )
            .unwrap();
    }
}
fn protected_directory(f: &Fixture) -> File {
    let path = f.path.join("protected-fixture-state");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    File::open(path).unwrap()
}
#[test]
fn protected_media_trash_moves_all_existing_files_and_requires_completed_history_before_restore() {
    let f = Fixture::new();
    let original = f.setup();
    let journal = protected_directory(&f);
    let operation = crate::OperationId::new("media-fixture-delete").unwrap();
    let token = Sha256::new(digest(operation.as_str().as_bytes())).unwrap();
    let plan = PreparedMedia::delete(&f.home, &f.account, None, "fixture", token.clone()).unwrap();
    let identity = plan
        .move_files(
            &operation,
            &media_proof(&operation, false),
            &f.home,
            &f.account,
            &journal,
            || true,
        )
        .unwrap()
        .unwrap();
    assert!(!f.path.join("Pictures/Wallpapers/fixture.png").exists());
    plan.verify_moves(&f.home, &f.account).unwrap();
    let history_path = f.path.join("protected-fixture-state");
    let partial = media_history(&history_path, &operation, false);
    assert!(VerifiedTrashReceipt::load(&journal, &identity, &f.account, &partial).is_err());
    let complete = media_history(&history_path, &operation, true);
    let receipt = VerifiedTrashReceipt::load(&journal, &identity, &f.account, &complete).unwrap();
    let mut other = f.account.clone();
    other.principal = ryoku_recovery::AccountPrincipal::from_account(
        other.uid,
        other.gid,
        "other",
        f.path.to_str().unwrap(),
    )
    .unwrap();
    assert!(VerifiedTrashReceipt::load(&journal, &identity, &other, &complete).is_err());
    let receipt_file = history_path.join(format!("media-trash-{}.json", token.as_str()));
    let receipt_bytes = std::fs::read(&receipt_file).unwrap();
    std::fs::set_permissions(&receipt_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&receipt_file, b"{}").unwrap();
    std::fs::set_permissions(&receipt_file, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(VerifiedTrashReceipt::load(&journal, &identity, &f.account, &complete).is_err());
    std::fs::set_permissions(&receipt_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&receipt_file, &receipt_bytes).unwrap();
    std::fs::set_permissions(&receipt_file, std::fs::Permissions::from_mode(0o400)).unwrap();

    // Simulate only the independently tested catalog actor outcome here. The
    // combined provider fixture supplies its real Go actor in the next layer.
    f.write(CATALOG, br#"{"other":{"key":"other","private":"must preserve"}}"#);
    let restore_operation = crate::OperationId::new("media-fixture-restore").unwrap();
    let restore = PreparedMedia::restore(
        &f.home,
        &f.account,
        None,
        &receipt,
        Sha256::new(digest(restore_operation.as_str().as_bytes())).unwrap(),
    )
    .unwrap();
    assert_eq!(restore.row, original);
    restore
        .move_files(
            &restore_operation,
            &media_proof(&restore_operation, false),
            &f.home,
            &f.account,
            &journal,
            || true,
        )
        .unwrap();
    assert_eq!(
        std::fs::read(f.path.join("Pictures/Wallpapers/fixture.png")).unwrap(),
        b"synthetic media"
    );
    assert!(!f.path.join(".cache/ryogami/wallpaper/thumbs-sm/fixture.jpg").exists());
    restore.verify_moves(&f.home, &f.account).unwrap();
    assert!(
        PreparedMedia::restore(
            &f.home,
            &f.account,
            None,
            &receipt,
            Sha256::new("e".repeat(64)).unwrap()
        )
        .is_err()
    );
}
#[test]
fn media_interruption_retains_protected_receipt_and_never_overwrites_a_parent_or_target_change() {
    for stop in 1..=6 {
        let f = Fixture::new();
        f.setup();
        let journal = protected_directory(&f);
        let operation = crate::OperationId::new("media-partial").unwrap();
        let token = Sha256::new(digest(operation.as_str().as_bytes())).unwrap();
        let plan =
            PreparedMedia::delete(&f.home, &f.account, None, "fixture", token.clone()).unwrap();
        let mut calls = 0;
        assert!(
            plan.move_files(
                &operation,
                &media_proof(&operation, false),
                &f.home,
                &f.account,
                &journal,
                || {
                    calls += 1;
                    calls != stop
                }
            )
            .is_err()
        );
        let receipt = f
            .path
            .join("protected-fixture-state")
            .join(format!("media-trash-{}.json", token.as_str()));
        assert_eq!(receipt.exists(), stop > 1);
        let identity = TrashReceiptIdentity::parse(&format!(
            "{}.{}",
            token.as_str(),
            digest(&std::fs::read(&receipt).unwrap_or_default())
        ))
        .unwrap();
        let partial = media_history(&f.path.join("protected-fixture-state"), &operation, false);
        assert!(VerifiedTrashReceipt::load(&journal, &identity, &f.account, &partial).is_err());
    }
    let f = Fixture::new();
    f.setup();
    let journal = protected_directory(&f);
    let operation = crate::OperationId::new("media-parent").unwrap();
    let plan = PreparedMedia::delete(
        &f.home,
        &f.account,
        None,
        "fixture",
        Sha256::new(digest(operation.as_str().as_bytes())).unwrap(),
    )
    .unwrap();
    let parent = f.path.join("Pictures/Wallpapers");
    let moved = f.path.join("Pictures/old");
    std::fs::rename(&parent, &moved).unwrap();
    std::fs::create_dir(&parent).unwrap();
    std::fs::rename(moved.join("fixture.png"), parent.join("fixture.png")).unwrap();
    assert!(
        plan.move_files(
            &operation,
            &media_proof(&operation, false),
            &f.home,
            &f.account,
            &journal,
            || true
        )
        .is_err()
    );
    assert!(std::fs::read_dir(f.path.join("protected-fixture-state")).unwrap().next().is_none());
}

fn real_media_actor(f: &Fixture, plan: &PreparedMedia, id: i64) -> Vec<u8> {
    let binary =
        std::env::var("RYOKU_MEDIA_ACTOR_TEST_BINARY").expect("explicit cached Go fixture binary");
    f.write(".fixture-media-commit.json", &plan.commit.frame(id).unwrap());
    let _ = std::fs::remove_file(f.path.join(".fixture-media-event.json"));
    let status = std::process::Command::new(binary)
        .arg("-test.run")
        .arg("^TestBrokerCatalogRustProviderFixture$")
        .env("RYOKU_MEDIA_PROVIDER_FIXTURE", &f.path)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(f.path.join(".fixture-media-reply.json")).unwrap()
}
#[test]
#[ignore = "requires explicit locally compiled pinned Go catalog actor fixture; no downloads"]
fn rust_media_provider_to_native_go_catalog_delete_restore_and_failure_retention() {
    let f = Fixture::new();
    f.setup();
    let journal = protected_directory(&f);
    let operation = crate::OperationId::new("media-go-delete").unwrap();
    let plan = PreparedMedia::delete(
        &f.home,
        &f.account,
        None,
        "fixture",
        Sha256::new(digest(operation.as_str().as_bytes())).unwrap(),
    )
    .unwrap();
    let identity = plan
        .move_files(
            &operation,
            &media_proof(&operation, false),
            &f.home,
            &f.account,
            &journal,
            || true,
        )
        .unwrap()
        .unwrap();
    let reply = real_media_actor(&f, &plan, 71);
    plan.verify_catalog(&f.home, &f.account, &reply, 71).unwrap();
    let event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path.join(".fixture-media-event.json")).unwrap())
            .unwrap();
    assert_eq!(event["event"], "ryogami.wall.file_removed");
    assert_eq!(event["data"]["key"], "fixture");
    let history = media_history(&f.path.join("protected-fixture-state"), &operation, true);
    let page = trash::list_owned(
        &journal,
        &history,
        &f.home,
        &f.account,
        &ryoku_recovery::ryogami_media_protocol::MediaTrashQuery::default(),
    )
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    let selected = TrashReceiptIdentity::parse(&page.entries[0].receipt).unwrap();
    assert_eq!(selected, identity);
    let receipt = VerifiedTrashReceipt::load(&journal, &selected, &f.account, &history).unwrap();
    let restore_operation = crate::OperationId::new("media-go-restore").unwrap();
    let restore = PreparedMedia::restore(
        &f.home,
        &f.account,
        None,
        &receipt,
        Sha256::new(digest(restore_operation.as_str().as_bytes())).unwrap(),
    )
    .unwrap();
    restore
        .move_files(
            &restore_operation,
            &media_proof(&restore_operation, false),
            &f.home,
            &f.account,
            &journal,
            || true,
        )
        .unwrap();
    let reply = real_media_actor(&f, &restore, 72);
    restore.verify_catalog(&f.home, &f.account, &reply, 72).unwrap();
    let event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path.join(".fixture-media-event.json")).unwrap())
            .unwrap();
    assert_eq!(event["event"], "ryogami.wall.cached");
    assert_eq!(event["data"]["key"], "fixture");
    assert_eq!(
        std::fs::read(f.path.join("Pictures/Wallpapers/fixture.png")).unwrap(),
        b"synthetic media"
    );
    assert!(
        trash::list_owned(
            &journal,
            &history,
            &f.home,
            &f.account,
            &ryoku_recovery::ryogami_media_protocol::MediaTrashQuery::default()
        )
        .unwrap()
        .entries
        .is_empty()
    );
    for fault in ["catalog", "stage"] {
        let f = Fixture::new();
        f.setup();
        let journal = protected_directory(&f);
        let operation = crate::OperationId::new(format!("media-go-fail-{fault}")).unwrap();
        let plan = PreparedMedia::delete(
            &f.home,
            &f.account,
            None,
            "fixture",
            Sha256::new(digest(operation.as_str().as_bytes())).unwrap(),
        )
        .unwrap();
        let identity = plan
            .move_files(
                &operation,
                &media_proof(&operation, false),
                &f.home,
                &f.account,
                &journal,
                || true,
            )
            .unwrap()
            .unwrap();
        if fault == "catalog" {
            f.write(CATALOG, br#"{"unexpected":{"key":"unexpected","type":"static"}}"#);
        } else {
            f.write(&plan.commit.staging_name(), b"retain partial stage");
        }
        let reply = real_media_actor(&f, &plan, 73);
        assert!(plan.verify_catalog(&f.home, &f.account, &reply, 73).is_err());
        assert!(!f.path.join(".fixture-media-event.json").exists());
        let partial = media_history(&f.path.join("protected-fixture-state"), &operation, false);
        assert!(VerifiedTrashReceipt::load(&journal, &identity, &f.account, &partial).is_err());
        assert!(!f.path.join("Pictures/Wallpapers/fixture.png").exists());
        assert!(plan.moves.iter().any(|item| {
            std::fs::read(f.path.join(item.trash.home_relative().unwrap().as_str()))
                .is_ok_and(|bytes| bytes == b"synthetic media")
        }));
    }
}

fn delete_for_listing(
    f: &Fixture,
    journal: &File,
    operation: &crate::OperationId,
) -> TrashReceiptIdentity {
    f.setup();
    let token = Sha256::new(digest(operation.as_str().as_bytes())).unwrap();
    let plan = PreparedMedia::delete(&f.home, &f.account, None, "fixture", token).unwrap();
    plan.move_files(operation, &media_proof(operation, false), &f.home, &f.account, journal, || {
        true
    })
    .unwrap()
    .unwrap()
}
#[test]
fn media_trash_listing_reopens_protected_receipts_filters_accounts_and_returns_no_paths_or_credentials()
 {
    use ryoku_recovery::ryogami_media_protocol::MediaTrashQuery;
    let f = Fixture::new();
    let journal = protected_directory(&f);
    let operation = crate::OperationId::new("list-media-delete").unwrap();
    let identity = delete_for_listing(&f, &journal, &operation);
    let history_path = f.path.join("protected-fixture-state");
    let history = media_history(&history_path, &operation, true);
    drop(history);
    let history = crate::OperationJournal::open(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(history_path.join("complete-history"))
            .unwrap(),
    )
    .unwrap();
    let page =
        trash::list_owned(&journal, &history, &f.home, &f.account, &MediaTrashQuery::default())
            .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].receipt, identity.encoded());
    assert_eq!(page.entries[0].label, "fixture.png");
    assert_eq!(page.entries[0].bytes, 48);
    assert!(!page.entries[0].recovery_required);
    assert!(page.next_cursor.is_none());
    let public = serde_json::to_string(&page).unwrap();
    for private in ["private-fixture", "must preserve", "Pictures", "thumb", "future", "path"] {
        assert!(!public.contains(private), "{private}");
    }
    let other = Fixture::new();
    assert!(
        trash::list_owned(
            &journal,
            &history,
            &other.home,
            &other.account,
            &MediaTrashQuery::default()
        )
        .unwrap()
        .entries
        .is_empty()
    );
    assert!(
        trash::list_owned(
            &journal,
            &history,
            &f.home,
            &f.account,
            &MediaTrashQuery { cursor: Some("a".repeat(64)) }
        )
        .is_err()
    );
    // A partial previous restore is visible as recovery-required, not presented
    // as an ordinary restorable item or silently dropped.
    std::fs::remove_file(
        f.path.join(format!(".ryoku-media-trash-{}-1", digest(operation.as_str().as_bytes()))),
    )
    .unwrap();
    let page =
        trash::list_owned(&journal, &history, &f.home, &f.account, &MediaTrashQuery::default())
            .unwrap();
    assert!(page.entries[0].recovery_required);
}

#[test]
fn media_trash_listing_is_bounded_paginated_and_cursor_bound_to_account() {
    use ryoku_recovery::ryogami_media_protocol::MediaTrashQuery;
    let owner = Fixture::new();
    let foreign = Fixture::new();
    let directory = protected_directory(&owner);
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(owner.path.join("protected-fixture-state/list-history"))
        .unwrap();
    let mut history = crate::OperationJournal::open(file).unwrap();
    for number in 0..99 {
        let operation = crate::OperationId::new(format!("listing-{number:03}")).unwrap();
        delete_for_listing(if number < 64 { &foreign } else { &owner }, &directory, &operation);
        append_media_history(&mut history, &operation, true);
    }
    let first = trash::list_owned(
        &directory,
        &history,
        &owner.home,
        &owner.account,
        &MediaTrashQuery::default(),
    )
    .unwrap();
    assert!(first.entries.is_empty());
    let query = MediaTrashQuery { cursor: first.next_cursor };
    assert!(query.cursor.is_some());
    assert!(
        trash::list_owned(&directory, &history, &foreign.home, &foreign.account, &query).is_err()
    );
    let second =
        trash::list_owned(&directory, &history, &owner.home, &owner.account, &query).unwrap();
    assert_eq!(second.entries.len(), 32);
    assert!(second.next_cursor.is_some());
    let third = trash::list_owned(
        &directory,
        &history,
        &owner.home,
        &owner.account,
        &MediaTrashQuery { cursor: second.next_cursor },
    )
    .unwrap();
    assert_eq!(third.entries.len(), 3);
    assert!(third.next_cursor.is_none());
    assert!(!second.entries.iter().any(|entry| third.entries.contains(entry)));
}

#[test]
fn media_trash_listing_refuses_bad_receipts_and_never_reads_retained_media_payloads() {
    use ryoku_recovery::ryogami_media_protocol::MediaTrashQuery;
    for fault in ["pending", "symlink", "oversized", "fifo"] {
        let f = Fixture::new();
        let directory = protected_directory(&f);
        let operation = crate::OperationId::new("listing-refusal").unwrap();
        let identity = delete_for_listing(&f, &directory, &operation);
        let token = identity.encoded().split('.').next().unwrap().to_string();
        let history =
            media_history(&f.path.join("protected-fixture-state"), &operation, fault != "pending");
        let receipt = f.path.join(format!("protected-fixture-state/media-trash-{token}.json"));
        match fault {
            "symlink" => {
                std::fs::rename(&receipt, receipt.with_extension("kept")).unwrap();
                std::os::unix::fs::symlink(receipt.with_extension("kept"), &receipt).unwrap();
            }
            "oversized" => {
                std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&receipt)
                    .unwrap()
                    .set_len(256 * 1024 + 1)
                    .unwrap();
                std::fs::set_permissions(&receipt, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
            "fifo" => {
                let name = format!(".ryoku-media-trash-{token}-0");
                std::fs::remove_file(f.path.join(&name)).unwrap();
                fs::mknodat(&f.home, name.as_str(), FileType::Fifo, Mode::from_raw_mode(0o600), 0)
                    .unwrap();
            }
            _ => {}
        }
        let result = trash::list_owned(
            &directory,
            &history,
            &f.home,
            &f.account,
            &MediaTrashQuery::default(),
        );
        match fault {
            "pending" => assert!(result.unwrap().entries.is_empty()),
            "fifo" => assert!(result.unwrap().entries[0].recovery_required),
            _ => assert!(result.is_err()),
        }
    }
}
