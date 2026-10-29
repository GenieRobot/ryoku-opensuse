//! Descriptor-only snapshots. SQLite receives no filename or source descriptor.
//! WAL format/reader contract: https://sqlite.org/fileformat2.html#walformat
use crate::{Error, Result};
use rustix::fs::{self, FileType, Mode, OFlags};
use std::{fs::File, io::Read, time::Instant};

pub(super) struct Image {
    pub bytes: Vec<u8>,
    pub source_bytes: u64,
    pub files: usize,
}
struct Source {
    name: &'static str,
    file: File,
    stat: fs::Stat,
}

impl Source {
    fn open(root: &File, name: &'static str) -> Result<Option<Self>> {
        let file = match fs::openat2(
            root,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            super::NO_LINKS,
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(Error::UnsafeState),
        };
        let stat = fs::fstat(&file).map_err(|_| Error::Storage)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_uid != rustix::process::geteuid().as_raw()
            || stat.st_nlink != 1
            || stat.st_mode & 0o022 != 0
            || stat.st_size < 0
        {
            return Err(Error::UnsafeState);
        }
        Ok(Some(Self { name, file, stat }))
    }
    fn bytes(&self, budget: usize) -> Result<Vec<u8>> {
        let size = usize::try_from(self.stat.st_size).map_err(|_| Error::Limit)?;
        if size > budget {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::with_capacity(size + 1);
        (&self.file).take(size as u64 + 1).read_to_end(&mut bytes).map_err(|_| Error::Storage)?;
        if bytes.len() != size {
            return Err(Error::Busy);
        }
        Ok(bytes)
    }
    fn unchanged(&self, root: &File) -> Result<()> {
        let fd = fs::fstat(&self.file).map_err(|_| Error::Storage)?;
        let named =
            fs::statat(root, self.name, fs::AtFlags::SYMLINK_NOFOLLOW).map_err(|_| Error::Busy)?;
        if same(&self.stat, &fd) && same(&self.stat, &named) { Ok(()) } else { Err(Error::Busy) }
    }
}
fn same(a: &fs::Stat, b: &fs::Stat) -> bool {
    a.st_dev == b.st_dev
        && a.st_ino == b.st_ino
        && a.st_size == b.st_size
        && a.st_mtime == b.st_mtime
        && a.st_mtime_nsec == b.st_mtime_nsec
        && a.st_ctime == b.st_ctime
        && a.st_ctime_nsec == b.st_ctime_nsec
        && a.st_mode == b.st_mode
        && a.st_uid == b.st_uid
        && a.st_nlink == b.st_nlink
}
fn unchanged(root: &File, name: &str, source: Option<&Source>) -> Result<()> {
    if let Some(source) = source {
        return source.unchanged(root);
    }
    match fs::statat(root, name, fs::AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        _ => Err(Error::Busy),
    }
}

pub(super) fn capture(root: &File, budget: u64, deadline: Instant) -> Result<Image> {
    capture_inner(root, budget, deadline, || {})
}

fn capture_inner(
    root: &File,
    budget: u64,
    deadline: Instant,
    after_copy: impl FnOnce(),
) -> Result<Image> {
    if Instant::now() >= deadline {
        return Err(Error::Limit);
    }
    let db = Source::open(root, "opencode.db")?.ok_or(Error::ProviderUnavailable)?;
    let lock = super::sqlite::SharedLock::acquire(&db.file)?;
    let mut wal = Source::open(root, "opencode.db-wal")?;
    let shm = Source::open(root, "opencode.db-shm")?;
    let wal_lock =
        shm.as_ref().map(|source| super::sqlite::WalLock::acquire(&source.file)).transpose()?;
    if let Some(source) = &shm {
        let stat = fs::fstat(&source.file).map_err(|_| Error::Storage)?;
        let mut version = [0; 4];
        (&source.file).read_exact(&mut version).map_err(|_| Error::Integrity)?;
        if stat.st_size < 32768
            || stat.st_size % 32768 != 0
            || u32::from_ne_bytes(version) != 3_007_000
        {
            return Err(Error::Integrity);
        }
    }
    let mut journal = Source::open(root, "opencode.db-journal")?;
    if let Some(journal) = &mut journal {
        let mut header = [0; 8];
        let count = (&mut journal.file).take(8).read(&mut header).map_err(|_| Error::Storage)?;
        if count != 0 && (count != 8 || header.iter().any(|byte| *byte != 0)) {
            return Err(Error::Busy);
        }
    }
    let budget = usize::try_from(budget.min(super::TOTAL_BYTES)).map_err(|_| Error::Limit)?;
    let bytes = db.bytes(budget)?;
    let wal_bytes =
        if let Some(wal) = &mut wal { wal.bytes(budget - bytes.len())? } else { Vec::new() };
    after_copy();
    db.unchanged(root)?;
    unchanged(root, "opencode.db-wal", wal.as_ref())?;
    unchanged(root, "opencode.db-shm", shm.as_ref())?;
    unchanged(root, "opencode.db-journal", journal.as_ref())?;
    if Instant::now() >= deadline {
        return Err(Error::Limit);
    }
    let source_bytes = (bytes.len() + wal_bytes.len()) as u64;
    // Bytes are now private and stable. Release source read leases before the
    // potentially longer in-memory validation/query, so writers can progress.
    drop(wal_lock);
    drop(lock);
    let bytes = replay(bytes, &wal_bytes, deadline)?;
    Ok(Image { bytes, source_bytes, files: 1 + usize::from(wal.is_some()) })
}

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(Error::Integrity)?
        .try_into()
        .map_err(|_| Error::Integrity)?;
    Ok(u32::from_be_bytes(bytes))
}
fn checksum(bytes: &[u8], big: bool, mut sums: (u32, u32)) -> Result<(u32, u32)> {
    if !bytes.len().is_multiple_of(8) {
        return Err(Error::Integrity);
    }
    for pair in bytes.as_chunks::<8>().0 {
        let first: [u8; 4] = pair[..4].try_into().map_err(|_| Error::Integrity)?;
        let second: [u8; 4] = pair[4..].try_into().map_err(|_| Error::Integrity)?;
        let (a, b) = if big {
            (u32::from_be_bytes(first), u32::from_be_bytes(second))
        } else {
            (u32::from_le_bytes(first), u32::from_le_bytes(second))
        };
        sums.0 = sums.0.wrapping_add(a).wrapping_add(sums.1);
        sums.1 = sums.1.wrapping_add(b).wrapping_add(sums.0);
    }
    Ok(sums)
}

fn page_size(db: &[u8]) -> Result<usize> {
    if db.len() < 100
        || &db[..16] != b"SQLite format 3\0"
        || ![1, 2].contains(&db[18])
        || ![1, 2].contains(&db[19])
    {
        return Err(Error::Integrity);
    }
    let raw = u16::from_be_bytes(db[16..18].try_into().map_err(|_| Error::Integrity)?);
    let page = if raw == 1 { 65536 } else { usize::from(raw) };
    if !page.is_power_of_two() || !(512..=65536).contains(&page) || !db.len().is_multiple_of(page) {
        return Err(Error::Integrity);
    }
    Ok(page)
}

fn replay(mut db: Vec<u8>, wal: &[u8], deadline: Instant) -> Result<Vec<u8>> {
    let page = page_size(&db)?;
    if !wal.is_empty() {
        if wal.len() < 32 {
            return Err(Error::Integrity);
        }
        let big = match word(wal, 0)? {
            0x377f0682 => false,
            0x377f0683 => true,
            _ => return Err(Error::Integrity),
        };
        if word(wal, 4)? != 3_007_000 || word(wal, 8)? as usize != page {
            return Err(Error::Integrity);
        }
        let mut sums = checksum(&wal[..24], big, (0, 0))?;
        if sums != (word(wal, 24)?, word(wal, 28)?) {
            return Err(Error::Integrity);
        }
        let frame = 24 + page;
        if !(wal.len() - 32).is_multiple_of(frame) {
            return Err(Error::Integrity);
        }
        let mut frames = Vec::new();
        let mut commit = None;
        for offset in (32..wal.len()).step_by(frame) {
            if Instant::now() >= deadline {
                return Err(Error::Limit);
            }
            let header = &wal[offset..offset + 24];
            // A reset can retain a stale older-generation suffix. Like SQLite's
            // reader, stop there only after establishing a valid current commit.
            if header[8..16] != wal[16..24] {
                if commit.is_none() {
                    return Err(Error::Integrity);
                }
                break;
            }
            let number = word(header, 0)? as usize;
            if number == 0 || number == u32::MAX as usize {
                return Err(Error::Integrity);
            }
            sums = checksum(&header[..8], big, sums)?;
            sums = checksum(&wal[offset + 24..offset + frame], big, sums)?;
            if sums != (word(header, 16)?, word(header, 20)?) {
                return Err(Error::Integrity);
            }
            frames.push((number, offset + 24));
            let pages = word(header, 4)? as usize;
            if pages > 0 {
                commit = Some((frames.len(), pages));
            }
        }
        if let Some((count, pages)) = commit {
            let size = pages.checked_mul(page).ok_or(Error::Limit)?;
            if size > super::TOTAL_BYTES as usize || size < page {
                return Err(Error::Limit);
            }
            db.resize(size, 0);
            for (number, offset) in frames.into_iter().take(count) {
                if number <= pages {
                    db[(number - 1) * page..number * page]
                        .copy_from_slice(&wal[offset..offset + page]);
                }
            }
            db[28..32].copy_from_slice(&(pages as u32).to_be_bytes());
        }
    }
    if page_size(&db)? != page {
        return Err(Error::Integrity);
    }
    // SQLite's documented deserialize contract disallows WAL-mode memory DBs.
    // Only the private committed image is normalized; source bytes never change.
    db[18] = 1;
    db[19] = 1;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{
        sqlite::{
            Reader,
            testing::{Writer, subprocess},
        },
        sqlite_tests::Directory,
    };
    use std::{fs, time::Duration};
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }
    fn big_endian(mut wal: Vec<u8>) -> Vec<u8> {
        wal[..4].copy_from_slice(&0x377f0683u32.to_be_bytes());
        let mut sums = checksum(&wal[..24], true, (0, 0)).unwrap();
        wal[24..28].copy_from_slice(&sums.0.to_be_bytes());
        wal[28..32].copy_from_slice(&sums.1.to_be_bytes());
        let frame = 24 + word(&wal, 8).unwrap() as usize;
        for offset in (32..wal.len()).step_by(frame) {
            sums = checksum(&wal[offset..offset + 8], true, sums).unwrap();
            sums = checksum(&wal[offset + 24..offset + frame], true, sums).unwrap();
            wal[offset + 16..offset + 20].copy_from_slice(&sums.0.to_be_bytes());
            wal[offset + 20..offset + 24].copy_from_slice(&sums.1.to_be_bytes());
        }
        wal
    }
    #[test]
    fn full_wal_header_frame_salt_checksum_and_version_validation_is_fail_closed() {
        let directory = Directory::new();
        drop(directory.old());
        let _child = subprocess(&directory.0, "wal", 1_800_000_000);
        let db = fs::read(directory.0.join("opencode.db")).unwrap();
        let wal = fs::read(directory.0.join("opencode.db-wal")).unwrap();
        assert!(replay(db.clone(), &wal, deadline()).is_ok());
        for offset in [0, 4, 24, 32 + 8, 32 + 24 + 100] {
            let mut damaged = wal.clone();
            damaged[offset] ^= 1;
            assert!(matches!(replay(db.clone(), &damaged, deadline()), Err(Error::Integrity)));
        }
        assert!(matches!(
            replay(db.clone(), &wal[..wal.len() - 1], deadline()),
            Err(Error::Integrity)
        ));
        let converted = big_endian(wal);
        let reader =
            Reader::new(replay(db.clone(), &converted, deadline()).unwrap(), deadline()).unwrap();
        let mut tokens = 0;
        reader
            .rows(0, |bytes, _, _, _| {
                let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                tokens += value["tokens"]["input"].as_u64().unwrap_or_default();
                Ok(true)
            })
            .unwrap();
        assert_eq!(tokens, 42);
        let mut broken = replay(db, &converted, deadline()).unwrap();
        broken[100] = 0xff;
        let refused = match Reader::new(broken, deadline()) {
            Err(_) => true,
            Ok(reader) => reader.rows(0, |_, _, _, _| Ok(true)).is_err(),
        };
        assert!(refused);
    }
    #[test]
    fn actual_sqlite_commit_during_snapshot_revalidation_is_refused() {
        let directory = Directory::new();
        let writer = directory.old();
        writer.execute("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;INSERT INTO message VALUES('s',1,1,'{}')");
        let result = capture_inner(&directory.root(), 64 * 1024 * 1024, deadline(), || {
            writer.execute("INSERT INTO message VALUES('s',2,2,'{}')")
        });
        assert!(matches!(result, Err(Error::Busy)));
    }
    #[test]
    fn hot_rollback_journal_and_unknown_schema_are_unavailable() {
        let directory = Directory::new();
        let writer = Writer::create(&directory.0.join("opencode.db"));
        writer.execute("CREATE TABLE unrelated(value TEXT)");
        drop(writer);
        fs::write(
            directory.0.join("opencode.db-journal"),
            [0xd9, 0xd5, 5, 0xf9, 0x20, 0xa1, 0x63, 0xd7],
        )
        .unwrap();
        assert!(matches!(
            capture(&directory.root(), 64 * 1024 * 1024, deadline()),
            Err(Error::Busy)
        ));
        fs::remove_file(directory.0.join("opencode.db-journal")).unwrap();
        let image = capture(&directory.root(), 64 * 1024 * 1024, deadline()).unwrap();
        let reader = Reader::new(image.bytes, deadline()).unwrap();
        assert!(matches!(reader.rows(0, |_, _, _, _| Ok(true)), Err(Error::ProviderUnavailable)));
    }

    #[test]
    fn wal_checkpoint_is_blocked_during_copy_and_succeeds_after_lease_release() {
        let directory = Directory::new();
        let writer = directory.old();
        writer.execute("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;INSERT INTO message VALUES('s',1,1,'{}')");
        let image = capture_inner(&directory.root(), 64 * 1024 * 1024, deadline(), || {
            assert_eq!(writer.checkpoint(), 5)
        })
        .unwrap();
        assert!(!image.bytes.is_empty());
        assert_eq!(writer.checkpoint(), 0);
    }
    #[test]
    fn birth_of_a_wal_index_during_capture_is_refused_even_without_timestamp_changes() {
        let directory = Directory::new();
        let writer = directory.old();
        writer.execute("INSERT INTO message VALUES('s',1,1,'{}')");
        drop(writer);
        let result = capture_inner(&directory.root(), 64 * 1024 * 1024, deadline(), || {
            std::fs::write(directory.0.join("opencode.db-shm"), [0; 4]).unwrap();
        });
        assert!(matches!(result, Err(Error::Busy)));
    }
}
