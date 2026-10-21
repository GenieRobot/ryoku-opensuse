//! Installed admission contract for the normal graphical login. Recovery
//! mutations require these exact unit definitions and their loaded dependency;
//! missing or substituted integration fails before any checkpoint or write.
use crate::restore::RestoreJournalStore;
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, fstat, openat2};
use std::{fs::File, io::Read};

pub const SERVICE_NAME: &str = "ryoku-recovery-admission.service";
pub const SERVICE_PATH: &str = "usr/lib/systemd/system/ryoku-recovery-admission.service";
pub const DROPIN_PATH: &str =
    "usr/lib/systemd/system/display-manager.service.d/ryoku-recovery.conf";
pub const SERVICE: &str =
    include_str!("../../../packaging/rpm/systemd/ryoku-recovery-admission.service");
pub const DROPIN: &str =
    include_str!("../../../packaging/rpm/systemd/display-manager.service.d/ryoku-recovery.conf");

/// Caller holds the shared machine mutation lock for this entire scan. A
/// malformed, missing, busy or incomplete journal can never return all-clear.
pub fn require_complete_journal(store: &RestoreJournalStore) -> Result<(), String> {
    let attempts = store
        .incomplete_attempts_under_mutation_lock()
        .map_err(|error| format!("recovery journal cannot admit a session: {error}"))?;
    if attempts.is_empty() {
        Ok(())
    } else {
        Err("recovery is unfinished; use the offline console before graphical login".into())
    }
}

pub fn verify_installed_units(root: &File) -> Result<(), String> {
    for (path, expected) in [(SERVICE_PATH, SERVICE), (DROPIN_PATH, DROPIN)] {
        let file = File::from(
            openat2(
                root,
                path,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            )
            .map_err(|_| "installed recovery admission unit is unavailable")?,
        );
        let stat = fstat(&file).map_err(|_| "recovery admission unit metadata is unavailable")?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_uid != 0
            || stat.st_gid != 0
            || stat.st_mode & 0o7777 != 0o644
            || stat.st_nlink != 1
            || stat.st_size != expected.len() as i64
        {
            return Err("installed recovery admission unit has unsafe metadata".into());
        }
        let mut bytes = Vec::new();
        file.take(expected.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "installed recovery admission unit cannot be read")?;
        if bytes != expected.as_bytes() {
            return Err(
                "installed recovery admission unit differs from the required contract".into()
            );
        }
    }
    Ok(())
}
