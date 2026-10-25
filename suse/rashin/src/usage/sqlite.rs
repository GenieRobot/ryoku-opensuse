//! The only SQLite/record-lock FFI boundary. No production SQLite call receives
//! a source filename: it reads an owned padded image in memory, with temp data
//! in memory, read-only backing, query-only mode and a deny-by-default authorizer.
//!
//! SAFETY: the installed Linux SQLite 3 ABI creates unique opaque handles. The
//! non-Send owner retains the stable image and callback allocation through close;
//! statements borrow that owner, finalize once, and copy values only while at
//! SQLITE_ROW. Callback strings follow SQLite's nullable terminated-string
//! contract; callbacks neither reenter SQLite nor panic. No raw pointer escapes.
//! The image has 64 padding bytes for SQLite's documented malicious-image margin.
//! https://sqlite.org/c3ref/deserialize.html
//! https://sqlite.org/c3ref/column_blob.html
//! https://sqlite.org/c3ref/set_authorizer.html
use crate::{Error, Result};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_uint, c_void},
    fs::File,
    marker::PhantomData,
    os::fd::AsRawFd,
    ptr::{self, NonNull},
    rc::Rc,
    time::Instant,
};

#[repr(C)]
struct Db {
    _private: [u8; 0],
}
#[repr(C)]
struct Stmt {
    _private: [u8; 0],
}
type Authorizer = Option<
    unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
    ) -> c_int,
>;

#[link(name = "libsqlite3.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn sqlite3_open_v2(
        path: *const c_char,
        db: *mut *mut Db,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close_v2(db: *mut Db) -> c_int;
    fn sqlite3_libversion_number() -> c_int;
    fn sqlite3_threadsafe() -> c_int;
    fn sqlite3_compileoption_used(option: *const c_char) -> c_int;
    fn sqlite3_deserialize(
        db: *mut Db,
        schema: *const c_char,
        data: *mut u8,
        length: i64,
        capacity: i64,
        flags: c_uint,
    ) -> c_int;
    fn sqlite3_db_config(db: *mut Db, operation: c_int, ...) -> c_int;
    fn sqlite3_limit(db: *mut Db, category: c_int, value: c_int) -> c_int;
    fn sqlite3_set_authorizer(db: *mut Db, callback: Authorizer, context: *mut c_void) -> c_int;
    fn sqlite3_progress_handler(
        db: *mut Db,
        instructions: c_int,
        callback: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
        context: *mut c_void,
    );
    fn sqlite3_prepare_v3(
        db: *mut Db,
        sql: *const c_char,
        length: c_int,
        flags: c_uint,
        stmt: *mut *mut Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_stmt_readonly(stmt: *mut Stmt) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut Stmt, index: c_int, value: i64) -> c_int;
    fn sqlite3_step(stmt: *mut Stmt) -> c_int;
    fn sqlite3_column_count(stmt: *mut Stmt) -> c_int;
    fn sqlite3_column_type(stmt: *mut Stmt, index: c_int) -> c_int;
    fn sqlite3_column_text(stmt: *mut Stmt, index: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: *mut Stmt, index: c_int) -> c_int;
    fn sqlite3_finalize(stmt: *mut Stmt) -> c_int;
}

const OK: c_int = 0;
const ROW: c_int = 100;
const DONE: c_int = 101;
const READONLY_IMAGE: c_uint = 4;
const NO_VIRTUAL_TABLES: c_uint = 4;
const PENDING_BYTE: i64 = 0x4000_0000;
const SHARED_FIRST: i64 = PENDING_BYTE + 2;
const SHARED_BYTES: i64 = 510;

pub(super) struct SharedLock<'a> {
    file: &'a File,
    _single_thread: PhantomData<Rc<()>>,
}
impl<'a> SharedLock<'a> {
    pub(super) fn acquire(file: &'a File) -> Result<Self> {
        record_lock(file, libc::F_RDLCK, PENDING_BYTE, 1)?;
        if let Err(error) = record_lock(file, libc::F_RDLCK, SHARED_FIRST, SHARED_BYTES) {
            let _ = record_lock(file, libc::F_UNLCK, PENDING_BYTE, 1);
            return Err(error);
        }
        if let Err(error) = record_lock(file, libc::F_UNLCK, PENDING_BYTE, 1) {
            let _ = record_lock(file, libc::F_UNLCK, SHARED_FIRST, SHARED_BYTES);
            return Err(error);
        }
        Ok(Self { file, _single_thread: PhantomData })
    }
}
impl Drop for SharedLock<'_> {
    fn drop(&mut self) {
        let _ = record_lock(self.file, libc::F_UNLCK, SHARED_FIRST, SHARED_BYTES);
    }
}
fn record_lock(file: &File, kind: c_int, start: i64, length: i64) -> Result<()> {
    // SAFETY: zero is valid for all libc::flock fields; the synchronous syscall
    // borrows this live descriptor/stack struct and retains neither pointer.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = kind as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = start;
    lock.l_len = length;
    // OFD locks conflict with SQLite's POSIX locks even in the same process.
    // They do not replace another connection's locks or write source bytes.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &lock) } < 0 {
        return Err(match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EAGAIN | libc::EACCES) => Error::Busy,
            _ => Error::ProviderUnavailable,
        });
    }
    Ok(())
}

unsafe extern "C" fn authorize(
    _: *mut c_void,
    operation: c_int,
    a: *const c_char,
    b: *const c_char,
    database: *const c_char,
    _: *const c_char,
) -> c_int {
    // SAFETY: SQLite owns each nullable terminated string until return. Byte
    // comparisons allocate nothing and cannot unwind through the C callback.
    let a = if a.is_null() { b"".as_slice() } else { unsafe { CStr::from_ptr(a) }.to_bytes() };
    let b = if b.is_null() { b"".as_slice() } else { unsafe { CStr::from_ptr(b) }.to_bytes() };
    let database = if database.is_null() {
        b"".as_slice()
    } else {
        unsafe { CStr::from_ptr(database) }.to_bytes()
    };
    let allowed = operation == 21 // SELECT
        || (operation == 20 && database == b"main" && [b"message".as_slice(), b"session_message", b"sqlite_master", b"sqlite_schema"].contains(&a))
        || (operation == 19 && a == b"table_info" && [b"message".as_slice(), b"session_message"].contains(&b));
    if allowed { OK } else { 1 } // SQLITE_DENY, including functions/ATTACH/DDL/DML.
}
unsafe extern "C" fn progress(context: *mut c_void) -> c_int {
    if context.is_null() {
        return 1;
    }
    // SAFETY: the stable immutable boxed deadline outlives registration; the
    // callback is disabled before close/drop and never accesses the connection.
    (Instant::now() >= unsafe { *context.cast::<Instant>() }) as c_int
}

pub(super) struct Reader {
    db: NonNull<Db>,
    _image: Box<[u8]>,
    deadline: Box<Instant>,
    _single_thread: PhantomData<Rc<()>>,
}
impl Reader {
    pub(super) fn new(mut image: Vec<u8>, deadline: Instant) -> Result<Self> {
        if image.len() > super::TOTAL_BYTES as usize || Instant::now() >= deadline {
            return Err(Error::Limit);
        }
        // SAFETY: these ABI probes borrow static C strings, no handles/buffers.
        if unsafe { sqlite3_libversion_number() } < 3_031_000
            || unsafe { sqlite3_threadsafe() } == 0
            || unsafe { sqlite3_compileoption_used(c"OMIT_DESERIALIZE".as_ptr()) } != 0
        {
            return Err(Error::ProviderUnavailable);
        }
        let length = image.len();
        image.resize(length + 64, 0);
        let mut db = ptr::null_mut();
        // SAFETY: only the fixed memory name is supplied. A returned handle is
        // owned uniquely and closed even when opening reports failure.
        let status = unsafe {
            sqlite3_open_v2(c":memory:".as_ptr(), &mut db, 2 | 4 | 0x8000 | 0x40000, ptr::null())
        };
        let db = NonNull::new(db).ok_or(Error::ProviderUnavailable)?;
        if status != OK {
            unsafe {
                sqlite3_close_v2(db.as_ptr());
            }
            return Err(Error::ProviderUnavailable);
        }
        let mut reader = Self {
            db,
            _image: image.into_boxed_slice(),
            deadline: Box::new(deadline),
            _single_thread: PhantomData,
        };
        for (operation, value) in
            [(1010, 1), (1017, 0), (1003, 0), (1015, 0), (1005, 0), (1013, 0), (1014, 0)]
        {
            let mut actual: c_int = 0;
            // SAFETY: these documented db_config operations all take int,int*;
            // the live connection and stack output remain valid for the call.
            if unsafe { sqlite3_db_config(reader.db.as_ptr(), operation, value, &mut actual) } != OK
                || actual != value
            {
                return Err(Error::ProviderUnavailable);
            }
        }
        // Only fixed connection settings run before the untrusted image exists.
        reader.setting(c"PRAGMA temp_store=MEMORY")?;
        reader.setting(c"PRAGMA query_only=ON")?;
        // SAFETY: Box is stable, padded, and retained through close; READONLY
        // permits neither writes, growth nor ownership transfer/free by SQLite.
        if unsafe {
            sqlite3_deserialize(
                reader.db.as_ptr(),
                c"main".as_ptr(),
                reader._image.as_mut_ptr(),
                length as i64,
                reader._image.len() as i64,
                READONLY_IMAGE,
            )
        } != OK
        {
            return Err(Error::Integrity);
        }
        // SAFETY: connection-only limits, no pointer retention or source I/O.
        for (category, value) in [
            (0, super::FILE_BYTES as c_int),
            (1, 16384),
            (2, 256),
            (3, 64),
            (4, 0),
            (5, 100000),
            (6, 16),
            (7, 0),
            (8, 128),
            (9, 2),
            (10, 0),
            (11, 0),
        ] {
            unsafe {
                sqlite3_limit(reader.db.as_ptr(), category, value);
            }
        }
        // SAFETY: callback has the documented calling convention, no context,
        // and remains registered until the live connection is closed.
        if unsafe { sqlite3_set_authorizer(reader.db.as_ptr(), Some(authorize), ptr::null_mut()) }
            != OK
        {
            return Err(Error::ProviderUnavailable);
        }
        // SAFETY: boxed deadline is stable until disabled in Drop.
        unsafe {
            sqlite3_progress_handler(
                reader.db.as_ptr(),
                1000,
                Some(progress),
                (&mut *reader.deadline as *mut Instant).cast(),
            );
        }
        Ok(reader)
    }
    fn setting(&self, sql: &CStr) -> Result<()> {
        let mut statement = self.prepare(sql, None, false)?;
        if statement.next()? {
            return Err(Error::Integrity);
        }
        Ok(())
    }
    fn prepare(&self, sql: &CStr, cutoff: Option<i64>, read_only: bool) -> Result<Statement<'_>> {
        if Instant::now() >= *self.deadline {
            return Err(Error::Limit);
        }
        let mut statement = ptr::null_mut();
        // SAFETY: CStr is terminated and SQLite copies SQL at prepare_v3. The
        // unique statement borrows this connection until finalization.
        let status = unsafe {
            sqlite3_prepare_v3(
                self.db.as_ptr(),
                sql.as_ptr(),
                -1,
                NO_VIRTUAL_TABLES,
                &mut statement,
                ptr::null_mut(),
            )
        };
        let pointer = match NonNull::new(statement) {
            Some(pointer) => pointer,
            None => return Err(self.failure(status)),
        };
        let statement = Statement { pointer, reader: self, on_row: false };
        if status != OK {
            return Err(self.failure(status));
        }
        // SAFETY: statement belongs to this live connection, not yet stepped.
        if read_only && unsafe { sqlite3_stmt_readonly(pointer.as_ptr()) } == 0 {
            return Err(Error::Integrity);
        }
        if let Some(cutoff) = cutoff
            && unsafe { sqlite3_bind_int64(pointer.as_ptr(), 1, cutoff) } != OK
        {
            return Err(Error::Integrity);
        }
        Ok(statement)
    }
    fn failure(&self, _: c_int) -> Error {
        if Instant::now() >= *self.deadline { Error::Limit } else { Error::Integrity }
    }

    pub(super) fn rows(
        &self,
        cutoff: i64,
        mut visit: impl FnMut(&[u8], Option<&[u8]>, Option<i64>) -> Result<bool>,
    ) -> Result<usize> {
        let mut processed = 0;
        for table in ["session_message", "message"] {
            let sql = CString::new(format!("PRAGMA main.table_info(\"{table}\")"))
                .map_err(|_| Error::Integrity)?;
            let mut columns = self.prepare(&sql, None, true)?;
            let mut names = Vec::new();
            while columns.next()? {
                if names.len() >= 256 {
                    return Err(Error::Limit);
                }
                let bytes = columns.bytes(1)?.ok_or(Error::Integrity)?;
                if let Ok(name) = String::from_utf8(bytes) {
                    names.push(name.to_ascii_lowercase());
                }
            }
            let pick = |candidates: &'static [&'static str]| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| names.iter().any(|name| name == candidate))
            };
            let Some(blob) = pick(&["data", "content", "body", "message", "json"]) else {
                continue;
            };
            let role = pick(&["type", "role"]);
            let time = pick(&["time_updated", "updated_at", "time_created", "created_at"]);
            // Identifiers come only from the static candidate lists, never from
            // schema text. No API accepts SQL or arbitrary table/column names.
            let projection = |column: Option<&str>| {
                column.map(|name| format!("\"{name}\"")).unwrap_or_else(|| "NULL".into())
            };
            let where_time = time
                .map(|name| format!(" WHERE \"{name}\" >= ?1 ORDER BY \"{name}\" ASC"))
                .unwrap_or_default();
            let sql = CString::new(format!(
                "SELECT \"{blob}\",{},{} FROM \"{table}\"{where_time} LIMIT 100001",
                projection(role),
                projection(time)
            ))
            .map_err(|_| Error::Integrity)?;
            let mut statement = self.prepare(&sql, time.map(|_| cutoff), true)?;
            let mut count = 0;
            while statement.next()? {
                if processed >= super::MAX_EVENTS {
                    return Err(Error::Limit);
                }
                let blob = statement.bytes(0)?.unwrap_or_default();
                let role = statement.bytes(1)?;
                let time = statement
                    .bytes(2)?
                    .and_then(|bytes| std::str::from_utf8(&bytes).ok()?.parse::<i64>().ok());
                if visit(&blob, role.as_deref(), time)? {
                    count += 1;
                }
                processed += 1;
            }
            if count > 0 {
                return Ok(count);
            }
        }
        Err(Error::ProviderUnavailable)
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: Rust borrows prevent live statements at owner drop. Disable
        // the callback and close before Rust frees image/deadline allocations.
        unsafe {
            sqlite3_progress_handler(self.db.as_ptr(), 0, None, ptr::null_mut());
            sqlite3_close_v2(self.db.as_ptr());
        }
    }
}
struct Statement<'a> {
    pointer: NonNull<Stmt>,
    reader: &'a Reader,
    on_row: bool,
}
impl Statement<'_> {
    fn next(&mut self) -> Result<bool> {
        self.on_row = false;
        if Instant::now() >= *self.reader.deadline {
            return Err(Error::Limit);
        }
        // SAFETY: unique live statement, owner borrowed; no concurrent access.
        match unsafe { sqlite3_step(self.pointer.as_ptr()) } {
            ROW => {
                self.on_row = true;
                Ok(true)
            }
            DONE => Ok(false),
            status => Err(self.reader.failure(status)),
        }
    }
    fn bytes(&self, column: c_int) -> Result<Option<Vec<u8>>> {
        if !self.on_row || column < 0 {
            return Err(Error::Integrity);
        }
        // SAFETY: access follows ROW; index is checked against column_count.
        let count = unsafe { sqlite3_column_count(self.pointer.as_ptr()) };
        if column >= count || count > 8 {
            return Err(Error::Integrity);
        }
        if unsafe { sqlite3_column_type(self.pointer.as_ptr(), column) } == 5 {
            return Ok(None);
        }
        // Request UTF-8 before its byte length, as required by SQLite. Copy
        // before any step/reset/finalize or conversion can invalidate the view.
        let bytes = unsafe { sqlite3_column_text(self.pointer.as_ptr(), column) };
        let length = unsafe { sqlite3_column_bytes(self.pointer.as_ptr(), column) };
        if bytes.is_null() || length < 0 || length as u64 > super::FILE_BYTES {
            return Err(Error::Integrity);
        }
        Ok(Some(unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec()))
    }
}
impl Drop for Statement<'_> {
    fn drop(&mut self) {
        unsafe {
            sqlite3_finalize(self.pointer.as_ptr());
        }
    }
}

#[cfg(test)]
pub(in crate::usage) mod testing {
    use super::*;
    use std::{
        fs,
        os::unix::fs::OpenOptionsExt,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        time::Duration,
    };
    unsafe extern "C" {
        fn sqlite3_exec(
            db: *mut Db,
            sql: *const c_char,
            callback: Option<
                unsafe extern "C" fn(
                    *mut c_void,
                    c_int,
                    *mut *mut c_char,
                    *mut *mut c_char,
                ) -> c_int,
            >,
            context: *mut c_void,
            error: *mut *mut c_char,
        ) -> c_int;
        fn sqlite3_serialize(
            db: *mut Db,
            schema: *const c_char,
            length: *mut i64,
            flags: c_uint,
        ) -> *mut u8;
        fn sqlite3_free(pointer: *mut c_void);
        fn sqlite3_db_cacheflush(db: *mut Db) -> c_int;
    }
    pub(in crate::usage) struct Writer {
        db: NonNull<Db>,
        _single_thread: PhantomData<Rc<()>>,
    }
    impl Writer {
        pub(in crate::usage) fn create(path: &Path) -> Self {
            fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).unwrap();
            Self::open(path)
        }
        fn open(path: &Path) -> Self {
            let path = CString::new(path.to_str().unwrap()).unwrap();
            let mut pointer = ptr::null_mut();
            // SAFETY: test-only fixture path, no production caller can open a
            // filesystem database through this boundary. Handle is owned once.
            assert_eq!(
                unsafe {
                    sqlite3_open_v2(path.as_ptr(), &mut pointer, 2 | 0x8000 | 0x40000, ptr::null())
                },
                OK
            );
            Self { db: NonNull::new(pointer).unwrap(), _single_thread: PhantomData }
        }
        pub(in crate::usage) fn execute(&self, sql: &str) {
            let sql = CString::new(sql).unwrap();
            // SAFETY: test-only SQL fixture construction with no callback or
            // retained error pointer. Production has no generic SQL executor.
            assert_eq!(
                unsafe {
                    sqlite3_exec(
                        self.db.as_ptr(),
                        sql.as_ptr(),
                        None,
                        ptr::null_mut(),
                        ptr::null_mut(),
                    )
                },
                OK
            );
        }
        pub(in crate::usage) fn serialized(&self) -> Vec<u8> {
            let mut length = 0;
            // SAFETY: SQLite allocates a copy; copy before freeing it once.
            let bytes =
                unsafe { sqlite3_serialize(self.db.as_ptr(), c"main".as_ptr(), &mut length, 0) };
            assert!(!bytes.is_null() && length > 0 && length <= super::super::TOTAL_BYTES as i64);
            let result = unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec();
            unsafe {
                sqlite3_free(bytes.cast());
            }
            result
        }
    }
    impl Drop for Writer {
        fn drop(&mut self) {
            unsafe {
                sqlite3_close_v2(self.db.as_ptr());
            }
        }
    }
    pub(in crate::usage) struct RunningFixture(Child);
    impl Drop for RunningFixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    pub(in crate::usage) fn subprocess(root: &Path, role: &str, at: u64) -> RunningFixture {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "usage::sqlite::testing::fixture_subprocess", "--nocapture"])
            .env("RASHIN_SQLITE_TEST_ROOT", root)
            .env("RASHIN_SQLITE_TEST_ROLE", role)
            .env("RASHIN_SQLITE_TEST_AT", at.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let child = RunningFixture(child);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !root.join("ready").exists() {
            assert!(Instant::now() < deadline, "SQLite fixture did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        child
    }
    fn insert(writer: &Writer, at: u64, tokens: u64) {
        let data = serde_json::json!({"role":"assistant","modelID":"fixture-model","providerID":"fixture","time":at*1000,"tokens":{"input":tokens},"content":"PRIVATE_SQLITE_PROMPT"}).to_string().replace('\'', "''");
        writer.execute(&format!(
            "INSERT INTO message VALUES('s',{},{},'{data}')",
            at * 1000,
            at * 1000
        ));
    }
    #[test]
    fn fixture_subprocess() {
        let Ok(role) = std::env::var("RASHIN_SQLITE_TEST_ROLE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("RASHIN_SQLITE_TEST_ROOT").unwrap());
        let at = std::env::var("RASHIN_SQLITE_TEST_AT").unwrap().parse::<u64>().unwrap();
        let writer = Writer::open(&root.join("opencode.db"));
        if role == "locked" || role == "reserved" {
            writer.execute(if role == "locked" {
                "PRAGMA journal_mode=DELETE;BEGIN EXCLUSIVE"
            } else {
                "PRAGMA journal_mode=DELETE;BEGIN IMMEDIATE"
            });
        } else {
            writer.execute("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0");
            insert(&writer, at, 42);
            if role == "uncommitted" {
                writer.execute("BEGIN IMMEDIATE");
                insert(&writer, at, 99);
                // SAFETY: test-only explicit spill of this live transaction.
                assert_eq!(unsafe { sqlite3_db_cacheflush(writer.db.as_ptr()) }, OK);
            }
            if role == "reset" {
                for _ in 0..20 {
                    insert(&writer, at, 1);
                }
                writer.execute("PRAGMA wal_checkpoint(RESTART)");
                insert(&writer, at, 7);
            }
        }
        fs::write(root.join("ready"), b"ready").unwrap();
        // Parent owns this synthetic child and kills/reaps it after the assertion.
        std::thread::sleep(Duration::from_secs(20));
    }
    pub(in crate::usage) fn denied(reader: &Reader, sql: &str) -> bool {
        let sql = CString::new(sql).unwrap();
        match reader.prepare(&sql, None, false) {
            Err(_) => true,
            Ok(mut statement) => statement.next().is_err(),
        }
    }
}
