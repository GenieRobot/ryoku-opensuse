//! One machine journal shared by future cooperating helper and console writers.
//! Opening/replay (including torn-tail repair) requires the existing writer guard.
use crate::{
    JournalError, OperationJournal,
    mutation_gate::{GateError, MUTATION_LOCK_NAME, MutationLock, WriterGuard},
};
use rustix::fs::{self, FileType, Mode, OFlags};
use ryoku_recovery::{
    machine_state::read_machine_state_record,
    state_mount::{StateMountIdentity, collect_and_open_state_mount},
};
use std::fs::File;

pub(crate) const JOURNAL_NAME: &str = "mutation-journal.jsonl";

/// No caller-selected journal path, owner, or per-account status file.
pub(crate) struct ProtectedOperationJournal<'a> {
    journal: OperationJournal,
    _guard: &'a WriterGuard,
}
impl<'a> ProtectedOperationJournal<'a> {
    pub(crate) fn open(
        state: &File,
        lock: &MutationLock,
        guard: &'a WriterGuard,
    ) -> Result<Self, GateError> {
        lock.revalidate_guard(guard)?;
        lock.revalidate_directory(state)?;
        let root = File::open("/").map_err(|_| GateError::Journal(JournalError::Io))?;
        let identity = read_machine_state_record(&root)
            .map_err(|_| GateError::Journal(JournalError::Io))?
            .identity()
            .clone();
        Self::open_owned(state, guard, 0, 0, Some(identity)).map_err(GateError::Journal)
    }

    fn open_owned(
        state: &File,
        guard: &'a WriterGuard,
        uid: u32,
        gid: u32,
        identity: Option<StateMountIdentity>,
    ) -> Result<Self, JournalError> {
        let namespace = JournalNamespace {
            directory: state.try_clone().map_err(|_| JournalError::Io)?,
            writer: guard.pin_file()?,
            uid,
            gid,
            identity,
        };
        namespace.revalidate_directory()?;
        let file = File::from(
            fs::openat(
                state,
                JOURNAL_NAME,
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::CLOEXEC
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|_| JournalError::Io)?,
        );
        namespace.revalidate(&file)?;
        fs::fsync(state).map_err(|_| JournalError::Io)?;
        let mut journal = OperationJournal::open(file)?;
        journal.protected = Some(namespace);
        Ok(Self { journal, _guard: guard })
    }

    pub(crate) fn journal(&mut self) -> &mut OperationJournal {
        &mut self.journal
    }
}

/// Rechecked before every replay and append, not only at initial file open.
pub(crate) struct JournalNamespace {
    directory: File,
    writer: File,
    uid: u32,
    gid: u32,
    identity: Option<StateMountIdentity>,
}
impl JournalNamespace {
    fn revalidate_directory(&self) -> Result<(), JournalError> {
        let stat = fs::fstat(&self.directory).map_err(|_| JournalError::Io)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
            || stat.st_uid != self.uid
            || stat.st_gid != self.gid
            || stat.st_mode & 0o7777 != 0o700
        {
            return Err(JournalError::Io);
        }
        self.revalidate_named_file(MUTATION_LOCK_NAME, &self.writer)?;
        if let Some(expected) = &self.identity {
            let root = File::open("/").map_err(|_| JournalError::Io)?;
            let record = read_machine_state_record(&root).map_err(|_| JournalError::Io)?;
            if record.identity() != expected {
                return Err(JournalError::Io);
            }
            let (current, _) =
                collect_and_open_state_mount(&root, expected).map_err(|_| JournalError::Io)?;
            let current = fs::fstat(&current).map_err(|_| JournalError::Io)?;
            if current.st_dev != stat.st_dev || current.st_ino != stat.st_ino {
                return Err(JournalError::Io);
            }
        }
        Ok(())
    }

    pub(crate) fn revalidate(&self, file: &File) -> Result<(), JournalError> {
        self.revalidate_directory()?;
        self.revalidate_named_file(JOURNAL_NAME, file)
    }

    fn revalidate_named_file(&self, name: &str, file: &File) -> Result<(), JournalError> {
        let current = fs::openat(
            &self.directory,
            name,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| JournalError::Io)?;
        let current = fs::fstat(&current).map_err(|_| JournalError::Io)?;
        let pinned = fs::fstat(file).map_err(|_| JournalError::Io)?;
        if FileType::from_raw_mode(current.st_mode) != FileType::RegularFile
            || current.st_uid != self.uid
            || current.st_gid != self.gid
            || current.st_mode & 0o7777 != 0o600
            || current.st_nlink != 1
            || current.st_dev != pinned.st_dev
            || current.st_ino != pinned.st_ino
        {
            return Err(JournalError::Io);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "protected_operation_journal_tests.rs"]
mod tests;
