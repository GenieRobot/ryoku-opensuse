use super::*;
use crate::{
    ManagedActionKind, OperationId, OperationPhase,
    mutation_gate::{JournaledOperation, MutationOutcome, PlannedChange},
};
use std::{
    fs as stdfs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    state: File,
    lock: MutationLock,
    uid: u32,
    gid: u32,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-protected-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        stdfs::create_dir(&path).unwrap();
        stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o700)).unwrap();
        let state = File::open(&path).unwrap();
        let stat = fs::fstat(&state).unwrap();
        let lock = MutationLock::open(state.try_clone().unwrap(), stat.st_uid).unwrap();
        Self { path, state, lock, uid: stat.st_uid, gid: stat.st_gid }
    }
    fn open<'a>(
        &self,
        guard: &'a WriterGuard,
    ) -> Result<ProtectedOperationJournal<'a>, JournalError> {
        self.lock.revalidate_guard(guard).unwrap();
        self.lock.revalidate_directory(&self.state).unwrap();
        ProtectedOperationJournal::open_owned(&self.state, guard, self.uid, self.gid, None)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = stdfs::remove_dir_all(&self.path);
    }
}
fn plan() -> PlannedChange {
    PlannedChange {
        operation_key: "personal-profile:fixture".into(),
        plan_fingerprint: "aa".repeat(32),
        affected_state_fingerprint: "bb".repeat(32),
    }
}
fn id(value: &str) -> OperationId {
    OperationId::new(value).unwrap()
}

#[test]
fn fixed_journal_refuses_unsafe_names_without_repairing_them() {
    for kind in ["symlink", "hardlink", "directory", "fifo", "mode"] {
        let fixture = Fixture::new();
        let path = fixture.path.join(JOURNAL_NAME);
        let guard = fixture.lock.acquire().unwrap();
        match kind {
            "symlink" => {
                stdfs::write(fixture.path.join("other"), b"untouched").unwrap();
                symlink("other", &path).unwrap();
            }
            "hardlink" => {
                stdfs::write(fixture.path.join("other"), b"untouched").unwrap();
                stdfs::set_permissions(
                    fixture.path.join("other"),
                    stdfs::Permissions::from_mode(0o600),
                )
                .unwrap();
                stdfs::hard_link(fixture.path.join("other"), &path).unwrap();
            }
            "directory" => stdfs::create_dir(&path).unwrap(),
            "fifo" => fs::mknodat(
                &fixture.state,
                JOURNAL_NAME,
                FileType::Fifo,
                Mode::from_raw_mode(0o600),
                0,
            )
            .unwrap(),
            "mode" => {
                stdfs::write(&path, b"untouched").unwrap();
                stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(fixture.open(&guard).is_err(), "{kind}");
        if matches!(kind, "symlink" | "hardlink") {
            assert_eq!(stdfs::read(fixture.path.join("other")).unwrap(), b"untouched");
        }
        if kind == "mode" {
            assert_eq!(stdfs::read(path).unwrap(), b"untouched");
        }
    }
}

#[test]
fn namespace_binding_survives_refresh_and_rejects_replacement_before_append() {
    let fixture = Fixture::new();
    let guard = fixture.lock.acquire().unwrap();
    let mut journal = fixture.open(&guard).unwrap();
    journal.journal().refresh().unwrap();
    stdfs::rename(fixture.path.join(JOURNAL_NAME), fixture.path.join("detached")).unwrap();
    stdfs::write(fixture.path.join(JOURNAL_NAME), b"").unwrap();
    stdfs::set_permissions(fixture.path.join(JOURNAL_NAME), stdfs::Permissions::from_mode(0o600))
        .unwrap();
    assert_eq!(journal.journal().refresh(), Err(JournalError::Io));
    let result = journal.journal().record(crate::OperationStatus {
        operation_id: id("unaccepted"),
        action: ManagedActionKind::ApplyProfile,
        phase: OperationPhase::Accepted,
        progress: None,
        error: None,
    });
    assert_eq!(result, Err(JournalError::Io));
    assert_eq!(stdfs::metadata(fixture.path.join(JOURNAL_NAME)).unwrap().len(), 0);
    assert_eq!(stdfs::metadata(fixture.path.join("detached")).unwrap().len(), 0);
}

#[test]
fn replaced_common_lock_blocks_journal_replay_and_append_under_old_guard() {
    let fixture = Fixture::new();
    let guard = fixture.lock.acquire().unwrap();
    let mut journal = fixture.open(&guard).unwrap();
    stdfs::rename(fixture.path.join(MUTATION_LOCK_NAME), fixture.path.join("old-lock")).unwrap();
    let replacement = MutationLock::open(fixture.state.try_clone().unwrap(), fixture.uid).unwrap();
    let _replacement_guard = replacement.acquire().unwrap();
    assert_eq!(journal.journal().refresh(), Err(JournalError::Io));
    let result = journal.journal().record(crate::OperationStatus {
        operation_id: id("unaccepted"),
        action: ManagedActionKind::ApplyProfile,
        phase: OperationPhase::Accepted,
        progress: None,
        error: None,
    });
    assert_eq!(result, Err(JournalError::Io));
    assert_eq!(stdfs::metadata(fixture.path.join(JOURNAL_NAME)).unwrap().len(), 0);
}

#[test]
fn restart_replays_protected_unfinished_history_and_refuses_another_acceptance() {
    let fixture = Fixture::new();
    {
        let guard = fixture.lock.acquire().unwrap();
        let mut journal = fixture.open(&guard).unwrap();
        let mut operation = JournaledOperation::new(
            journal.journal(),
            id("first"),
            ManagedActionKind::ApplyProfile,
        );
        operation.begin(&plan()).unwrap();
    }
    // Simulate a torn append without inventing a later phase. Only this tail
    // may be repaired, while holding the freshly reacquired common lock.
    stdfs::OpenOptions::new()
        .append(true)
        .open(fixture.path.join(JOURNAL_NAME))
        .unwrap()
        .write_all(b"{\"torn\"")
        .unwrap();
    let guard = fixture.lock.acquire().unwrap();
    let mut journal = fixture.open(&guard).unwrap();
    let length = stdfs::metadata(fixture.path.join(JOURNAL_NAME)).unwrap().len();
    assert_eq!(journal.journal().recovery_required().len(), 1);
    let mut operation =
        JournaledOperation::new(journal.journal(), id("second"), ManagedActionKind::ApplyProfile);
    assert_eq!(operation.begin(&plan()), Err(GateError::RecoveryRequired));
    assert_eq!(stdfs::metadata(fixture.path.join(JOURNAL_NAME)).unwrap().len(), length);
}

#[test]
fn cancelled_preflight_is_durable_but_same_operation_cannot_be_reused() {
    let fixture = Fixture::new();
    let guard = fixture.lock.acquire().unwrap();
    let mut journal = fixture.open(&guard).unwrap();
    let mut operation = JournaledOperation::new(
        journal.journal(),
        id("cancelled"),
        ManagedActionKind::ApplyProfile,
    );
    operation.begin(&plan()).unwrap();
    assert_eq!(
        operation.fail(crate::ServiceError::Failed, GateError::CancelledBeforeChange),
        GateError::CancelledBeforeChange
    );
    drop(journal);
    let mut journal = fixture.open(&guard).unwrap();
    assert_eq!(
        journal.journal().mutation_context(&id("cancelled")).unwrap().outcome,
        Some(MutationOutcome::CancelledBeforeChange)
    );
    assert!(journal.journal().recovery_required().is_empty());
    let mut retry = JournaledOperation::new(
        journal.journal(),
        id("cancelled"),
        ManagedActionKind::ApplyProfile,
    );
    assert_eq!(retry.begin(&plan()), Err(GateError::Journal(JournalError::InvalidTransition)));
}
