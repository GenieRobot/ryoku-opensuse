use super::*;
use std::{
    fs,
    os::unix::{
        fs::{MetadataExt, symlink},
        net::UnixStream,
    },
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
#[ignore = "requires a native ordinary-user /run/user/UID with trusted root-owned parents"]
fn native_runtime_mount_boundaries_preserve_strict_ownership_and_fixed_namespace() {
    let uid = rustix::process::geteuid().as_raw();
    assert_ne!(uid, 0, "native runtime probe requires an ordinary user");
    let root = File::open("/").unwrap();
    let run = open_directory(&root, "run", false).unwrap();
    let users = open_directory(&run, "user", false).unwrap();
    validate_directory(&run, 0, false).unwrap();
    validate_directory(&users, 0, false).unwrap();
    let pinned = open_runtime(&root, uid).unwrap();
    validate_directory(&pinned, uid, true).unwrap();
    let reopened = open_runtime(&root, uid).unwrap();
    same_inode(&fstat(&pinned).unwrap(), &fstat(&reopened).unwrap()).unwrap();
    assert!(validate_directory(&pinned, uid + 1, true).is_err());
    println!(
        "native_runtime_run_to_user_mount_boundary={}\nnative_runtime_user_to_uid_mount_boundary={}\nnative_runtime_fixed_namespace=passed\nnative_runtime_owner_mode_policy=passed",
        fstat(&run).unwrap().st_dev != fstat(&users).unwrap().st_dev,
        fstat(&users).unwrap().st_dev != fstat(&pinned).unwrap().st_dev,
    );
}

struct Fixture {
    path: PathBuf,
    root: File,
    uid: u32,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-runtime-paths-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let root = File::open(&path).unwrap();
        let uid = fstat(&root).unwrap().st_uid;
        Self { path, root, uid }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn unsafe_directory_modes_owners_and_symlinks_are_not_repaired_or_traversed() {
    let fixture = Fixture::new();
    assert!(validate_directory(&fixture.root, fixture.uid + 1, true).is_err());
    let child = child_directory(&fixture.root, "private", fixture.uid, true).unwrap();
    assert_eq!(fstat(&child).unwrap().st_mode & 0o7777, 0o700);
    fs::set_permissions(fixture.path.join("private"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(child_directory(&fixture.root, "private", fixture.uid, true).is_err());
    assert_eq!(fs::metadata(fixture.path.join("private")).unwrap().mode() & 0o7777, 0o777);
    symlink("private", fixture.path.join("link")).unwrap();
    assert!(child_directory(&fixture.root, "link", fixture.uid, true).is_err());
    assert!(open_directory(&fixture.root, "link/child", true).is_err());
}

#[test]
fn journal_rejects_symlink_hardlink_fifo_directory_foreign_uid_and_unsafe_mode_before_io() {
    let fixture = Fixture::new();
    fs::write(fixture.path.join("sentinel"), b"unchanged").unwrap();
    fs::set_permissions(fixture.path.join("sentinel"), fs::Permissions::from_mode(0o600)).unwrap();
    symlink("sentinel", fixture.path.join(JOURNAL)).unwrap();
    assert!(open_regular(&fixture.root, JOURNAL, fixture.uid, true).is_err());
    fs::remove_file(fixture.path.join(JOURNAL)).unwrap();
    fs::hard_link(fixture.path.join("sentinel"), fixture.path.join(JOURNAL)).unwrap();
    assert!(open_regular(&fixture.root, JOURNAL, fixture.uid, true).is_err());
    fs::remove_file(fixture.path.join(JOURNAL)).unwrap();
    rustix::fs::mknodat(&fixture.root, JOURNAL, FileType::Fifo, Mode::from_raw_mode(0o600), 0)
        .unwrap();
    let start = Instant::now();
    assert!(open_regular(&fixture.root, JOURNAL, fixture.uid, true).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    fs::remove_file(fixture.path.join(JOURNAL)).unwrap();
    fs::create_dir(fixture.path.join(JOURNAL)).unwrap();
    assert!(open_regular(&fixture.root, JOURNAL, fixture.uid, true).is_err());
    fs::remove_dir(fixture.path.join(JOURNAL)).unwrap();
    let journal = open_regular(&fixture.root, JOURNAL, fixture.uid, true).unwrap();
    assert!(open_regular(&fixture.root, JOURNAL, fixture.uid + 1, false).is_err());
    fs::set_permissions(fixture.path.join(JOURNAL), fs::Permissions::from_mode(0o660)).unwrap();
    assert!(validate_regular(&journal, fixture.uid).is_err());
    assert_eq!(fs::read(fixture.path.join("sentinel")).unwrap(), b"unchanged");
}

#[test]
fn shared_state_ancestor_accepts_safe_modes_without_chmodding_existing_directories() {
    for mode in [0o700, 0o750, 0o755] {
        let fixture = Fixture::new();
        let shared = fixture.path.join(".local/state");
        fs::create_dir_all(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(mode)).unwrap();
        let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
        assert_eq!(directories.revalidate(&fixture.root, fixture.uid), Ok(()));
        assert_eq!(fs::metadata(&shared).unwrap().mode() & 0o7777, mode);
        assert_eq!(fs::metadata(shared.join("ryoku")).unwrap().mode() & 0o7777, 0o700);
        assert_eq!(fs::metadata(shared.join("ryoku/service")).unwrap().mode() & 0o7777, 0o700);
        assert!(StateDirectories::open(&fixture.root, fixture.uid + 1).is_err());
        assert!(directories.revalidate(&fixture.root, fixture.uid + 1).is_err());
    }
}

#[test]
fn unsafe_shared_ancestor_and_nonprivate_service_modes_fail_open_and_revalidation() {
    let fixture = Fixture::new();
    let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
    for (name, modes) in [
        (".local", &[0o770, 0o777][..]),
        (".local/state", &[0o770, 0o777][..]),
        (".local/state/ryoku", &[0o770, 0o777][..]),
        (".local/state/ryoku/service", &[0o750, 0o755, 0o770, 0o777][..]),
    ] {
        let path = fixture.path.join(name);
        for &mode in modes {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(StateDirectories::open(&fixture.root, fixture.uid).is_err());
            assert!(directories.revalidate(&fixture.root, fixture.uid).is_err());
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, mode);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(directories.revalidate(&fixture.root, fixture.uid), Ok(()));
        }
    }
}

#[test]
fn reopening_state_namespace_rejects_each_replaced_parent_even_with_original_app_inode() {
    for name in [".local", ".local/state", ".local/state/ryoku", ".local/state/ryoku/service"] {
        let fixture = Fixture::new();
        let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
        for shared in [".local", ".local/state", ".local/state/ryoku"] {
            fs::set_permissions(fixture.path.join(shared), fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        assert_eq!(directories.revalidate(&fixture.root, fixture.uid), Ok(()));
        let old = fixture.path.join(name);
        let retained = old.with_file_name("retained");
        fs::rename(&old, &retained).unwrap();
        fs::create_dir(&old).unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o700)).unwrap();
        // Retain the original descendants. Parent identity must still fail.
        match name {
            ".local" => fs::rename(retained.join("state"), old.join("state")).unwrap(),
            ".local/state" => fs::rename(retained.join("ryoku"), old.join("ryoku")).unwrap(),
            ".local/state/ryoku" => {
                fs::rename(retained.join("service"), old.join("service")).unwrap()
            }
            _ => {}
        }
        assert!(directories.revalidate(&fixture.root, fixture.uid).is_err());
        fs::remove_dir_all(&old).unwrap();
        symlink("retained", &old).unwrap();
        assert!(directories.revalidate(&fixture.root, fixture.uid).is_err());
    }
}

#[test]
fn upstream_shared_app_directory_and_data_are_preserved_with_private_native_journal() {
    for mode in [0o700, 0o750, 0o755] {
        let fixture = Fixture::new();
        let app = fixture.path.join(".local/state/ryoku");
        fs::create_dir_all(&app).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(mode)).unwrap();
        let ui = app.join("upstream-state.json");
        fs::write(&ui, b"upstream data").unwrap();
        fs::set_permissions(&ui, fs::Permissions::from_mode(0o644)).unwrap();
        let app_inode = fs::metadata(&app).unwrap().ino();
        let ui_inode = fs::metadata(&ui).unwrap().ino();
        let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
        let journal = directories.open_journal(&fixture.root, fixture.uid).unwrap();
        directories.revalidate_journal(&journal, fixture.uid).unwrap();
        assert_eq!(fs::metadata(&app).unwrap().ino(), app_inode);
        assert_eq!(fs::metadata(&app).unwrap().mode() & 0o7777, mode);
        assert_eq!(fs::metadata(&ui).unwrap().ino(), ui_inode);
        assert_eq!(fs::metadata(&ui).unwrap().mode() & 0o7777, 0o644);
        assert_eq!(fs::read(&ui).unwrap(), b"upstream data");
        assert_eq!(fs::metadata(app.join(SERVICE_STATE)).unwrap().mode() & 0o7777, 0o700);
        assert_eq!(fstat(&journal).unwrap().st_mode & 0o7777, 0o600);
        assert!(!app.join(JOURNAL).exists());
    }
}

fn record_legacy_operation(file: File) -> crate::OperationStatus {
    let status = crate::OperationStatus {
        operation_id: crate::OperationId::new("legacy-durable-operation").unwrap(),
        action: crate::ManagedActionKind::InstallPackage,
        phase: crate::OperationPhase::Accepted,
        progress: None,
        error: None,
    };
    let mut journal = crate::OperationJournal::open(file).unwrap();
    journal.record(status.clone()).unwrap();
    status
}

#[test]
fn legacy_migration_and_restarts_preserve_inode_history_and_durable_lookup() {
    // These on-disk states also cover restart before and after the atomic
    // migration rename. No marker file or copying window needs recovery.
    for interrupted_after_rename in [false, true] {
        let fixture = Fixture::new();
        let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
        let legacy = open_regular(&directories.app, JOURNAL, fixture.uid, true).unwrap();
        let inode = fstat(&legacy).unwrap();
        let status = record_legacy_operation(legacy);
        let original = fs::read(fixture.path.join(".local/state/ryoku/operations.log")).unwrap();
        if interrupted_after_rename {
            renameat_with(
                &directories.app,
                JOURNAL,
                &directories.service,
                JOURNAL,
                RenameFlags::NOREPLACE,
            )
            .unwrap();
        }
        drop(directories);
        for _restart in 0..2 {
            let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
            let file = directories.open_journal(&fixture.root, fixture.uid).unwrap();
            same_inode(&inode, &fstat(&file).unwrap()).unwrap();
            let journal = crate::OperationJournal::open(file).unwrap();
            assert_eq!(journal.get(&status.operation_id), Some(&status));
            assert_eq!(
                fs::read(fixture.path.join(".local/state/ryoku/service/operations.log")).unwrap(),
                original
            );
            assert!(!fixture.path.join(".local/state/ryoku/operations.log").exists());
        }
    }
}

#[test]
fn conflicting_journals_fail_closed_without_selecting_even_an_empty_journal() {
    let fixture = Fixture::new();
    let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
    let legacy = open_regular(&directories.app, JOURNAL, fixture.uid, true).unwrap();
    let original = fstat(&legacy).unwrap();
    record_legacy_operation(legacy);
    let current = open_regular(&directories.service, JOURNAL, fixture.uid, true).unwrap();
    assert!(directories.open_journal(&fixture.root, fixture.uid).is_err());
    same_inode(&original, &statat(&directories.app, JOURNAL, AtFlags::SYMLINK_NOFOLLOW).unwrap())
        .unwrap();
    require_named_inode(&directories.service, JOURNAL, &current).unwrap();
    assert_eq!(fstat(&current).unwrap().st_size, 0);
}

#[test]
fn unsafe_legacy_entries_never_migrate_or_create_a_fresh_journal() {
    for kind in ["symlink", "dangling", "hardlink", "mode", "directory", "fifo", "foreign-owner"] {
        let fixture = Fixture::new();
        let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
        let app = fixture.path.join(".local/state/ryoku");
        let legacy = app.join(JOURNAL);
        let sentinel = app.join("sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o600)).unwrap();
        match kind {
            "symlink" => symlink("sentinel", &legacy).unwrap(),
            "dangling" => symlink("missing", &legacy).unwrap(),
            "hardlink" => fs::hard_link(&sentinel, &legacy).unwrap(),
            "directory" => fs::create_dir(&legacy).unwrap(),
            "fifo" => rustix::fs::mknodat(
                &directories.app,
                JOURNAL,
                FileType::Fifo,
                Mode::from_raw_mode(0o600),
                0,
            )
            .unwrap(),
            _ => {
                let _file = open_regular(&directories.app, JOURNAL, fixture.uid, true).unwrap();
                if kind == "mode" {
                    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o640)).unwrap();
                }
            }
        }
        let before = fs::symlink_metadata(&legacy).unwrap();
        let uid = if kind == "foreign-owner" { fixture.uid + 1 } else { fixture.uid };
        // Validate the legacy file under a different owner expectation directly;
        // StateDirectories also rejects that mismatch at every ancestor.
        if kind == "foreign-owner" {
            assert!(open_regular(&directories.app, JOURNAL, uid, false).is_err());
        }
        assert!(directories.open_journal(&fixture.root, uid).is_err(), "{kind}");
        assert!(!named_entry_exists(&directories.service, JOURNAL).unwrap());
        assert_eq!(fs::symlink_metadata(&legacy).unwrap().ino(), before.ino());
        assert_eq!(fs::symlink_metadata(&legacy).unwrap().mode(), before.mode());
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
    }
}

#[test]
fn corrupt_migrated_history_remains_in_place_and_never_falls_back_to_fresh_journal() {
    use std::io::Write;
    let fixture = Fixture::new();
    let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
    let mut legacy = open_regular(&directories.app, JOURNAL, fixture.uid, true).unwrap();
    legacy.write_all(b"invalid record\n").unwrap();
    let original = fstat(&legacy).unwrap();
    drop(legacy);
    for _restart in 0..2 {
        let file = directories.open_journal(&fixture.root, fixture.uid).unwrap();
        same_inode(&original, &fstat(&file).unwrap()).unwrap();
        assert!(crate::OperationJournal::open(file).is_err());
        assert_eq!(
            fs::read(fixture.path.join(".local/state/ryoku/service/operations.log")).unwrap(),
            b"invalid record\n"
        );
    }
}

#[test]
fn journal_revalidation_rejects_metadata_links_replacement_and_new_legacy_entries() {
    let fixture = Fixture::new();
    let directories = StateDirectories::open(&fixture.root, fixture.uid).unwrap();
    let file = directories.open_journal(&fixture.root, fixture.uid).unwrap();
    let app = fixture.path.join(".local/state/ryoku");
    let path = app.join("service/operations.log");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(directories.revalidate_journal(&file, fixture.uid).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&path, app.join("extra-link")).unwrap();
    assert!(directories.revalidate_journal(&file, fixture.uid).is_err());
    fs::remove_file(app.join("extra-link")).unwrap();
    symlink("missing", app.join(JOURNAL)).unwrap();
    assert!(directories.revalidate_journal(&file, fixture.uid).is_err());
    assert!(directories.open_journal(&fixture.root, fixture.uid).is_err());
    fs::remove_file(app.join(JOURNAL)).unwrap();
    directories.revalidate_journal(&file, fixture.uid).unwrap();
    fs::rename(&path, path.with_file_name("retained")).unwrap();
    let _replacement = open_regular(&directories.service, JOURNAL, fixture.uid, true).unwrap();
    assert!(directories.revalidate_journal(&file, fixture.uid).is_err());
}

#[test]
fn replacement_journal_or_lock_does_not_reuse_the_pinned_inode() {
    let fixture = Fixture::new();
    for name in [LOCK, JOURNAL] {
        let pinned = open_regular(&fixture.root, name, fixture.uid, true).unwrap();
        fs::rename(fixture.path.join(name), fixture.path.join(format!("{name}.old"))).unwrap();
        let _replacement = open_regular(&fixture.root, name, fixture.uid, true).unwrap();
        assert!(require_named_inode(&fixture.root, name, &pinned).is_err());
    }
}

#[test]
fn daemon_lifetime_lock_excludes_independent_opens_and_releases_on_close() {
    let fixture = Fixture::new();
    let first = open_regular(&fixture.root, LOCK, fixture.uid, true).unwrap();
    let second = open_regular(&fixture.root, LOCK, fixture.uid, true).unwrap();
    flock(&first, FlockOperation::NonBlockingLockExclusive).unwrap();
    assert!(flock(&second, FlockOperation::NonBlockingLockExclusive).is_err());
    drop(first);
    flock(&second, FlockOperation::NonBlockingLockExclusive).unwrap();
}

#[test]
fn socket_bind_is_private_and_live_socket_is_never_removed_but_closed_stale_socket_is() {
    let fixture = Fixture::new();
    let (listener, pinned) = bind_listener(&fixture.root, fixture.uid).unwrap();
    assert_eq!(fs::metadata(fixture.path.join(SOCKET)).unwrap().mode() & 0o7777, 0o600);
    let connected = UnixStream::connect(fixture.path.join(SOCKET)).unwrap();
    let (server, _) = listener.accept().unwrap();
    assert_eq!(crate::PeerIdentity::from_unix_socket(&server).unwrap().uid(), fixture.uid);
    assert!(remove_stale_socket(&fixture.root, fixture.uid).is_err());
    require_named_inode(&fixture.root, SOCKET, &pinned).unwrap();
    drop(connected);
    drop(server);
    drop(listener);
    assert!(remove_stale_socket(&fixture.root, fixture.uid + 1).is_err());
    require_named_inode(&fixture.root, SOCKET, &pinned).unwrap();
    remove_stale_socket(&fixture.root, fixture.uid).unwrap();
    assert!(!fixture.path.join(SOCKET).exists());
}

#[test]
fn socket_leaf_substitution_cannot_chmod_or_remove_an_unrelated_file() {
    let fixture = Fixture::new();
    let (listener, pinned) = bind_listener(&fixture.root, fixture.uid).unwrap();
    fs::rename(fixture.path.join(SOCKET), fixture.path.join("old.sock")).unwrap();
    fs::write(fixture.path.join("sentinel"), b"unchanged").unwrap();
    fs::set_permissions(fixture.path.join("sentinel"), fs::Permissions::from_mode(0o640)).unwrap();
    symlink("sentinel", fixture.path.join(SOCKET)).unwrap();
    // The actual chmod operation uses the retained socket inode descriptor.
    fs::set_permissions(fd_path(&pinned, None), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(require_named_inode(&fixture.root, SOCKET, &pinned).is_err());
    assert!(remove_stale_socket(&fixture.root, fixture.uid).is_err());
    assert!(bind_listener(&fixture.root, fixture.uid).is_err());
    assert_eq!(fs::read(fixture.path.join("sentinel")).unwrap(), b"unchanged");
    assert_eq!(fs::metadata(fixture.path.join("sentinel")).unwrap().mode() & 0o7777, 0o640);
    drop(listener);
}
