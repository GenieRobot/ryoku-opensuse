use super::*;
use crate::{
    configuration_apply::{ConfigurationApplyError, ConfigurationDestination, apply_with_owner},
    restore::{RestoreJournal, RestoreJournalStore, RestorePhase},
};
use std::{
    io::{BufRead, Write},
    ops::ControlFlow,
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

fn ready(f: &Fixture, advance: bool) -> (RestoreJournalStore, Identifier) {
    let path = f.path.join("journals");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let store = RestoreJournalStore::open(&File::open(path).unwrap()).unwrap();
    let id = Identifier::new("apply-attempt").unwrap();
    let mut journal = RestoreJournal::new(
        id.clone(),
        f.source.plan().clone(),
        Identifier::new("retained-current-generation").unwrap(),
        f.source.manifest_sha256().clone(),
    );
    store.create(&journal).unwrap();
    if advance {
        for phase in [
            RestorePhase::CheckpointVerified,
            RestorePhase::ContentStaged,
            RestorePhase::WritersStopped,
        ] {
            store.advance(&mut journal, phase).unwrap();
        }
    }
    (store, id)
}
fn live_root(f: &Fixture) -> File {
    let path = f.path.join("live");
    fs::create_dir(&path).unwrap();
    fs::hard_link(f.path.join("unrelated"), path.join("a")).unwrap();
    fs::write(path.join("b"), b"corrupt b").unwrap();
    fs::create_dir(path.join("directory")).unwrap();
    fs::write(path.join("directory/unrelated"), b"preserve child").unwrap();
    symlink(f.path.join("unrelated"), path.join("link")).unwrap();
    fs::write(path.join("missing"), b"remove enrolled file").unwrap();
    let config = f.path.join("config");
    fs::create_dir(&config).unwrap();
    fs::write(config.join("shell.json"), b"corrupt shell settings").unwrap();
    fs::write(config.join("matugen.json"), b"corrupt Matugen settings").unwrap();
    fs::write(config.join("profile.json"), b"corrupt profile selection").unwrap();
    fs::create_dir(config.join("profile")).unwrap();
    fs::write(config.join("profile/hero.png"), b"corrupt hero asset").unwrap();
    fs::write(config.join("profile/unselected.png"), b"destination-only user asset").unwrap();
    let home = f.path.join("home");
    fs::create_dir(&home).unwrap();
    fs::write(home.join("unrelated"), b"unrelated home sentinel").unwrap();
    fs::create_dir_all(home.join(".local/state/ryoku")).unwrap();
    fs::write(home.join(".local/state/ryoku/flags.json"), b"corrupt session preferences").unwrap();
    fs::write(home.join(".local/state/ryoku/stage-walls.json"), b"corrupt Stage preferences")
        .unwrap();
    File::open(path).unwrap()
}
fn apply(
    f: &Fixture,
    live: &File,
    journals: &RestoreJournalStore,
    id: &Identifier,
    progress: impl FnMut(usize) -> ControlFlow<()>,
) -> Result<RestoreJournal, ConfigurationApplyError> {
    let home = File::open(f.path.join("home")).unwrap();
    let configuration = File::open(f.path.join("config")).unwrap();
    apply_with_owner(
        &f.source,
        &f.store(),
        journals,
        id,
        &[
            ConfigurationDestination { root: StateRoot::EtcRyoku, directory: live },
            ConfigurationDestination {
                root: StateRoot::UserRyokuConfig,
                directory: &configuration,
            },
            ConfigurationDestination { root: StateRoot::UserRyokuState, directory: &home },
        ],
        progress,
        live.metadata().unwrap().uid(),
    )
}
fn check(f: &Fixture) {
    let live = f.path.join("live");
    assert_eq!(fs::read(live.join("a")).unwrap(), b"saved configuration");
    assert_eq!(
        fs::metadata(live.join("a")).unwrap().ino(),
        fs::metadata(live.join("b")).unwrap().ino()
    );
    assert_eq!(fs::read_link(live.join("link")).unwrap(), Path::new("a"));
    assert!(!live.join("missing").exists());
    assert_eq!(fs::read(f.path.join("unrelated")).unwrap(), b"home sentinel");
    assert_eq!(fs::read(f.path.join("home/unrelated")).unwrap(), b"unrelated home sentinel");
    let shell = f.path.join("config/shell.json");
    let matugen = f.path.join("config/matugen.json");
    if f.source.entries().iter().any(|entry| {
        entry.record().root == StateRoot::UserRyokuConfig
            && entry.record().path.as_str() == "shell.json"
    }) {
        assert_eq!(
            fs::read(shell).unwrap(),
            br#"{"theme":{"theme":"Tokyo Night"},"wallpaper":{"enabled":false}}"#
        );
        assert_eq!(
            fs::read(matugen).unwrap(),
            br#"{"mode":"dark","contrast":0.0,"appTheme":false}"#
        );
    } else {
        assert_eq!(fs::read(shell).unwrap(), b"corrupt shell settings");
        assert_eq!(fs::read(matugen).unwrap(), b"corrupt Matugen settings");
    }
    if f.source.entries().iter().any(|entry| {
        entry.record().root == StateRoot::UserRyokuConfig
            && entry.record().path.as_str() == "profile.json"
    }) {
        assert_eq!(
            fs::read(f.path.join("config/profile.json")).unwrap(),
            br#"{"hero":{"kind":"custom","source":"hero.png"}}"#
        );
        assert_eq!(
            fs::read(f.path.join("config/profile/hero.png")).unwrap(),
            b"saved custom hero bytes"
        );
        let profile = fs::metadata(f.path.join("config/profile")).unwrap();
        let selection = fs::metadata(f.path.join("config/profile.json")).unwrap();
        let hero = fs::metadata(f.path.join("config/profile/hero.png")).unwrap();
        assert_eq!(profile.mode() & 0o7777, 0o700);
        assert_eq!(selection.mode() & 0o7777, 0o644);
        assert_eq!(hero.mode() & 0o7777, 0o644);
        let source_hero = fs::metadata(f.path.join("source/profile/hero.png")).unwrap();
        assert_eq!((hero.uid(), hero.gid()), (source_hero.uid(), source_hero.gid()));
    } else {
        assert_eq!(
            fs::read(f.path.join("config/profile.json")).unwrap(),
            b"corrupt profile selection"
        );
        assert_eq!(
            fs::read(f.path.join("config/profile/hero.png")).unwrap(),
            b"corrupt hero asset"
        );
    }
    assert_eq!(
        fs::read(f.path.join("config/profile/unselected.png")).unwrap(),
        b"destination-only user asset"
    );
    let flags = f.path.join("home/.local/state/ryoku/flags.json");
    if f.source.entries().iter().any(|entry| entry.record().root == StateRoot::UserRyokuState) {
        assert_eq!(
            fs::read(flags).unwrap(),
            br#"{"dnd":true,"keepAwake":false,"keepAwakeSince":0,"gameMode":false,"gameDndPrev":false}"#
        );
        assert_eq!(
            fs::read(f.path.join("home/.local/state/ryoku/stage-walls.json")).unwrap(),
            br#"{"current":"/home/alice/Pictures/wall.jpg","walls":{"/home/alice/Pictures/wall.jpg":{"effect":{"enabled":true,"depth":0.4},"layers":[]}}}"#
        );
        assert_eq!(
            fs::metadata(f.path.join("home/.local/state/ryoku/stage-walls.json")).unwrap().mode()
                & 0o7777,
            0o600
        );
    } else {
        assert_eq!(fs::read(flags).unwrap(), b"corrupt session preferences");
        assert_eq!(
            fs::read(f.path.join("home/.local/state/ryoku/stage-walls.json")).unwrap(),
            b"corrupt Stage preferences"
        );
    }
    assert_eq!(fs::read(live.join("directory/unrelated")).unwrap(), b"preserve child");
    assert!(!live.join(".ryoku-restore-apply-attempt").exists());
}

#[test]
fn applies_all_inode_kinds_and_absence_without_mutating_unrelated_peers() {
    let f = Fixture::new();
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    let old = fs::metadata(f.path.join("unrelated")).unwrap();
    assert_eq!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap().phase,
        RestorePhase::ConfigurationRestored
    );
    check(&f);
    let after = fs::metadata(f.path.join("unrelated")).unwrap();
    assert_eq!(
        (old.mode(), old.uid(), old.gid(), old.mtime_nsec(), old.mtime()),
        (after.mode(), after.uid(), after.gid(), after.mtime_nsec(), after.mtime())
    );
    // A durable completed phase is verified without rewriting the live files.
    let inode = fs::metadata(f.path.join("live/a")).unwrap().ino();
    apply(&f, &live, &journals, &id, |_| panic!("must only verify completed application")).unwrap();
    assert_eq!(fs::metadata(f.path.join("live/a")).unwrap().ino(), inode);
}

#[test]
fn configuration_apply_restores_exact_base_default_acl() {
    use rustix::fs::{XattrFlags, fgetxattr, fsetxattr};

    let acl = [
        2_u32.to_le_bytes().as_slice(),
        &[
            1, 0, 7, 0, 255, 255, 255, 255, // ACL_USER_OBJ: rwx
            4, 0, 0, 0, 255, 255, 255, 255, // ACL_GROUP_OBJ: ---
            16, 0, 0, 0, 255, 255, 255, 255, // ACL_MASK: ---
            32, 0, 0, 0, 255, 255, 255, 255, // ACL_OTHER: ---
        ][..],
    ]
    .concat();
    let fixture = Fixture::with_extra(&[], |source| {
        let source_directory = File::open(source.join("directory")).unwrap();
        fsetxattr(&source_directory, "system.posix_acl_default", &acl, XattrFlags::empty())
            .unwrap();
        let mut source_acl = vec![0_u8; acl.len()];
        let source_length =
            fgetxattr(&source_directory, "system.posix_acl_default", &mut source_acl).unwrap();
        source_acl.truncate(source_length);
        assert_eq!(source_acl, acl, "fixture filesystem must retain the base default ACL");
    });

    let live = live_root(&fixture);
    let (journals, id) = ready(&fixture, true);
    fixture.store().create(&fixture.source).unwrap();
    apply(&fixture, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    check(&fixture);

    let restored = File::open(fixture.path.join("live/directory")).unwrap();
    let mut actual = vec![0_u8; acl.len()];
    let length = fgetxattr(&restored, "system.posix_acl_default", &mut actual).unwrap();
    actual.truncate(length);
    assert_eq!(actual, acl);
}

#[test]
fn aliased_destination_roots_are_rejected_before_any_restore_write() {
    let f = Fixture::new();
    let live = live_root(&f);
    let aliased_live = File::open(f.path.join("live")).unwrap();
    let home = File::open(f.path.join("home")).unwrap();
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();

    let result = apply_with_owner(
        &f.source,
        &f.store(),
        &journals,
        &id,
        &[
            ConfigurationDestination { root: StateRoot::EtcRyoku, directory: &live },
            ConfigurationDestination { root: StateRoot::UserRyokuConfig, directory: &aliased_live },
            ConfigurationDestination { root: StateRoot::UserRyokuState, directory: &home },
        ],
        |_| ControlFlow::Continue(()),
        live.metadata().unwrap().uid(),
    );
    assert!(matches!(result, Err(ConfigurationApplyError::InvalidInventory)));

    let live_path = f.path.join("live");
    assert_eq!(fs::read(live_path.join("a")).unwrap(), b"home sentinel");
    assert_eq!(fs::read(live_path.join("b")).unwrap(), b"corrupt b");
    assert_eq!(fs::read(f.path.join("config/shell.json")).unwrap(), b"corrupt shell settings");
    assert!(!live_path.join(".ryoku-restore-apply-attempt").exists());
    assert_eq!(journals.load(&id).unwrap().phase, RestorePhase::WritersStopped);
}

#[test]
fn restores_pinned_session_preferences_under_the_separate_home_root() {
    let f = Fixture::with_state_preferences();
    assert!(!f.source.entries().iter().any(|entry| {
        entry.record().root == StateRoot::UserRyokuConfig
            && entry.record().path.as_str() == "profile/unselected.png"
    }));
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    check(&f);
}

#[test]
fn every_durable_boundary_can_reopen_and_replay_without_losing_checkpoint() {
    for boundary in 1..=6 {
        let f = Fixture::new();
        let live = live_root(&f);
        let (journals, id) = ready(&f, true);
        f.store().create(&f.source).unwrap();
        assert!(matches!(
            apply(&f, &live, &journals, &id, |step| if step == boundary {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }),
            Err(ConfigurationApplyError::Cancelled)
        ));
        assert_eq!(journals.load(&id).unwrap().phase, RestorePhase::WritersStopped);
        drop(journals);
        let reopened =
            RestoreJournalStore::open(&File::open(f.path.join("journals")).unwrap()).unwrap();
        let state = apply(&f, &live, &reopened, &id, |_| ControlFlow::Continue(())).unwrap();
        assert_eq!(state.current_state_generation.as_str(), "retained-current-generation");
        check(&f);
    }
}

#[test]
fn process_exit_after_each_durable_apply_boundary_replays_the_same_checkpoint() {
    const CHILD_TEST: &str = "configuration_stage::disk_stage_tests::configuration_apply_tests::exit_after_durable_apply_boundary_child";

    for boundary in 1..=6 {
        let f = Fixture::new();
        let live = live_root(&f);
        let (journals, id) = ready(&f, true);
        f.store().create(&f.source).unwrap();
        drop(journals);

        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
            .env("RYOKU_APPLY_CHILD_ROOT", &f.path)
            .env("RYOKU_APPLY_CHILD_BOUNDARY", boundary.to_string())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77), "child did not exit at boundary {boundary}");

        let reopened =
            RestoreJournalStore::open(&File::open(f.path.join("journals")).unwrap()).unwrap();
        let state = apply(&f, &live, &reopened, &id, |_| ControlFlow::Continue(())).unwrap();
        assert_eq!(state.phase, RestorePhase::ConfigurationRestored);
        assert_eq!(state.current_state_generation.as_str(), "retained-current-generation");
        check(&f);
    }
}

#[test]
fn sigkill_after_each_durable_apply_boundary_replays_the_same_checkpoint() {
    const CHILD_TEST: &str = "configuration_stage::disk_stage_tests::configuration_apply_tests::sigkill_after_durable_apply_boundary_child";
    for boundary in 1..=6 {
        let f = Fixture::new();
        let live = live_root(&f);
        let (journals, id) = ready(&f, true);
        f.store().create(&f.source).unwrap();
        drop(journals);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
            .env("RYOKU_APPLY_CHILD_ROOT", &f.path)
            .env("RYOKU_APPLY_CHILD_BOUNDARY", boundary.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let marker = format!("RYOKU_APPLY_DURABLE_BOUNDARY={boundary}");
        loop {
            match receiver.recv_timeout(Duration::from_secs(15)) {
                Ok(Ok(line)) if line == marker => break,
                Ok(Ok(_)) => continue,
                Ok(Err(error)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("failed reading child boundary output: {error}");
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("child did not reach durable boundary {boundary}: {error}");
                }
            }
        }

        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(9), "child was expected to receive SIGKILL at {boundary}");

        let reopened =
            RestoreJournalStore::open(&File::open(f.path.join("journals")).unwrap()).unwrap();
        assert_eq!(reopened.load(&id).unwrap().phase, RestorePhase::WritersStopped);
        apply(&f, &live, &reopened, &id, |_| ControlFlow::Continue(())).unwrap();
        check(&f);
    }
}

#[test]
#[ignore = "spawned by the process-exit replay test"]
fn exit_after_durable_apply_boundary_child() {
    let (Ok(root), Ok(boundary)) =
        (std::env::var("RYOKU_APPLY_CHILD_ROOT"), std::env::var("RYOKU_APPLY_CHILD_BOUNDARY"))
    else {
        return;
    };
    let boundary = boundary.parse::<usize>().unwrap();
    let f = Fixture::reopen(PathBuf::from(root));
    let live = File::open(f.path.join("live")).unwrap();
    let journals =
        RestoreJournalStore::open(&File::open(f.path.join("journals")).unwrap()).unwrap();
    let id = Identifier::new("apply-attempt").unwrap();
    let _ = apply(&f, &live, &journals, &id, |step| {
        if step == boundary {
            // Exit bypasses Rust destructors and leaves only the durable journal
            // and filesystem writes for the parent to reopen and reconcile.
            std::process::exit(77);
        }
        ControlFlow::Continue(())
    });
    panic!("configuration apply returned before the selected boundary");
}

#[test]
#[ignore]
fn sigkill_after_durable_apply_boundary_child() {
    let (Ok(root), Ok(boundary)) =
        (std::env::var("RYOKU_APPLY_CHILD_ROOT"), std::env::var("RYOKU_APPLY_CHILD_BOUNDARY"))
    else {
        return;
    };
    let boundary = boundary.parse::<usize>().unwrap();
    let f = Fixture::reopen(PathBuf::from(root));
    let live = File::open(f.path.join("live")).unwrap();
    let journals =
        RestoreJournalStore::open(&File::open(f.path.join("journals")).unwrap()).unwrap();
    let id = Identifier::new("apply-attempt").unwrap();
    let _ = apply(&f, &live, &journals, &id, |step| {
        if step == boundary {
            println!("RYOKU_APPLY_DURABLE_BOUNDARY={step}");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park_timeout(std::time::Duration::from_secs(60));
            }
        }
        ControlFlow::Continue(())
    });
    panic!("configuration apply returned before the selected boundary");
}

#[test]
fn phase_and_source_stage_failures_precede_live_writes() {
    let f = Fixture::new();
    let live = live_root(&f);
    let (journals, id) = ready(&f, false);
    assert!(matches!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())),
        Err(ConfigurationApplyError::WrongPhaseOrPlan)
    ));
    assert_eq!(fs::read(f.path.join("live/a")).unwrap(), b"home sentinel");
    let f = Fixture::new();
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    fs::write(f.stage().join("object-000"), b"corrupt stage").unwrap();
    assert!(matches!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())),
        Err(ConfigurationApplyError::Stage(_))
    ));
    assert_eq!(fs::read(f.path.join("live/a")).unwrap(), b"home sentinel");
}

#[test]
fn nonempty_directory_removal_preserves_undeclared_children_in_recovery() {
    let f = Fixture::new();
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    fs::remove_file(f.path.join("live/missing")).unwrap();
    fs::create_dir(f.path.join("live/missing")).unwrap();
    fs::write(f.path.join("live/missing/unrelated"), b"keep me").unwrap();
    f.store().create(&f.source).unwrap();
    assert!(apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).is_err());
    assert_eq!(fs::read(f.path.join("live/missing/unrelated")).unwrap(), b"keep me");
    assert_eq!(journals.load(&id).unwrap().phase, RestorePhase::WritersStopped);
}

#[test]
fn enrolled_directory_replaces_a_symlink_without_following_its_old_target() {
    let f = Fixture::with_extra(&["directory/nested"], |source| {
        fs::write(source.join("directory/nested"), b"saved nested bytes").unwrap();
    });
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    fs::remove_dir_all(f.path.join("live/directory")).unwrap();
    fs::create_dir(f.path.join("outside")).unwrap();
    fs::write(f.path.join("outside/nested"), b"outside unchanged").unwrap();
    symlink(f.path.join("outside"), f.path.join("live/directory")).unwrap();
    f.store().create(&f.source).unwrap();
    apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    assert_eq!(fs::read(f.path.join("live/directory/nested")).unwrap(), b"saved nested bytes");
    assert_eq!(fs::read(f.path.join("outside/nested")).unwrap(), b"outside unchanged");
}

#[test]
fn undeclared_symlink_parent_blocks_before_any_configuration_write() {
    let f = Fixture::with_extra(&["opaque/file"], |source| {
        fs::create_dir(source.join("opaque")).unwrap();
        fs::write(source.join("opaque/file"), b"saved nested bytes").unwrap();
    });
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    fs::create_dir(f.path.join("outside")).unwrap();
    fs::write(f.path.join("outside/file"), b"outside unchanged").unwrap();
    symlink(f.path.join("outside"), f.path.join("live/opaque")).unwrap();
    f.store().create(&f.source).unwrap();
    assert!(apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).is_err());
    assert_eq!(fs::read(f.path.join("live/a")).unwrap(), b"home sentinel");
    assert_eq!(fs::read(f.path.join("outside/file")).unwrap(), b"outside unchanged");
}

#[test]
fn absent_ancestor_and_descendant_restore_without_traversing_a_current_file() {
    let f = Fixture::with_extra(&["missing/nested"], |_| {});
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    check(&f);
}

#[test]
fn hard_links_share_an_inode_across_distinct_destination_parents() {
    let f = Fixture::with_extra(&["directory/c"], |source| {
        fs::hard_link(source.join("a"), source.join("directory/c")).unwrap();
    });
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    check(&f);
    assert_eq!(
        fs::metadata(f.path.join("live/a")).unwrap().ino(),
        fs::metadata(f.path.join("live/directory/c")).unwrap().ino()
    );
    assert!(!f.path.join("live/directory/.ryoku-restore-apply-attempt").exists());
}

#[test]
fn completed_phase_rechecks_bytes_instead_of_silently_overwriting_new_changes() {
    let f = Fixture::new();
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    fs::write(f.path.join("live/a"), b"changed after application").unwrap();
    assert!(matches!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())),
        Err(ConfigurationApplyError::VerificationFailed)
    ));
    assert_eq!(fs::read(f.path.join("live/a")).unwrap(), b"changed after application");
}

#[test]
fn foreign_private_stage_is_not_adopted_or_deleted() {
    let f = Fixture::new();
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    let path = f.path.join("live/.ryoku-restore-apply-attempt");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(path.join("owner.json"), b"unrelated existing data").unwrap();
    fs::set_permissions(path.join("owner.json"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())),
        Err(ConfigurationApplyError::PrivateStageCollision)
    ));
    assert_eq!(fs::read(f.path.join("live/a")).unwrap(), b"home sentinel");
    assert_eq!(fs::read(path.join("owner.json")).unwrap(), b"unrelated existing data");
}

#[test]
fn unrepresentable_linux_component_is_rejected_before_any_live_write() {
    // Such an absent path can be declared in a sealed manifest without ever
    // opening it during flat-slot staging; it must not fail only after earlier
    // targets have already been replaced.
    let mut f = Fixture::new();
    let absent =
        f.source.entries.iter_mut().find(|e| matches!(e.record.kind, EntryKind::Absent)).unwrap();
    absent.record.path = crate::RelativePath::new("x".repeat(256)).unwrap();
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    assert!(matches!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())),
        Err(ConfigurationApplyError::InvalidInventory)
    ));
    assert_eq!(fs::read(f.path.join("live/a")).unwrap(), b"home sentinel");
}

#[test]
#[ignore = "requires privileged native ACL/SELinux fixture"]
fn native_cross_owner_acl_and_selinux_metadata_are_preserved() {
    use rustix::fs::{Gid, Uid, XattrFlags, fchown, fsetxattr};
    assert_eq!(fs::metadata("/proc/self").unwrap().uid(), 0);
    let f = Fixture::with_extra(&[], |source| {
        let file = File::open(source.join("a")).unwrap();
        fchown(&file, Some(Uid::from_raw(1)), Some(Gid::from_raw(1))).unwrap();
        let mut acl = 2_u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in [
            (1_u16, 6_u16, u32::MAX),
            (2, 4, 2),
            (4, 0, u32::MAX),
            (16, 4, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permissions.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        fsetxattr(&file, "system.posix_acl_access", &acl, XattrFlags::empty()).unwrap();
    });
    let entry = f.source.entries().iter().find(|e| e.record().path.as_str() == "a").unwrap();
    assert!(entry.acl().unwrap().access.is_some());
    assert!(
        entry
            .xattrs()
            .any(|(name, value)| name == crate::XattrName::SelinuxLabel && !value.is_empty())
    );
    let live = live_root(&f);
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap();
    check(&f);
    let stat = fs::metadata(f.path.join("live/a")).unwrap();
    assert_eq!((stat.uid(), stat.gid()), (1, 1));
}

#[test]
#[ignore = "requires TMPDIR on a different filesystem from /tmp"]
fn native_destination_local_stage_crosses_the_persistent_store_filesystem() {
    let f = Fixture::new();
    assert_ne!(f.path.metadata().unwrap().dev(), fs::metadata("/tmp").unwrap().dev());
    let path = Path::new("/tmp").join(f.path.file_name().unwrap());
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(path.clone());
    let live = File::open(&path).unwrap();
    let (journals, id) = ready(&f, true);
    f.store().create(&f.source).unwrap();
    assert_eq!(
        apply(&f, &live, &journals, &id, |_| ControlFlow::Continue(())).unwrap().phase,
        RestorePhase::ConfigurationRestored
    );
    assert_eq!(fs::read(path.join("a")).unwrap(), b"saved configuration");
    assert_eq!(
        fs::metadata(path.join("a")).unwrap().ino(),
        fs::metadata(path.join("b")).unwrap().ino()
    );
}
