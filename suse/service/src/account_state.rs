//! Account-bound default configuration root; not authorization or enrollment.
//! No client environment or manifest path participates in root selection.
use crate::PeerIdentity;
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags, fstat, openat2};
use std::{
    fs::File,
    mem::MaybeUninit,
    path::{Component, Path},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const ACCOUNT_BUFFER_BYTES: usize = 64 * 1024;
const ACCOUNT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
static ACCOUNT_WORKER: OnceLock<BoundedWorker> = OnceLock::new();

/// NSS modules are arbitrary native code and cannot be safely cancelled in a
/// thread. Keep at most one outstanding task, including after its caller times
/// out. A late result belongs only to its original channel and is discarded.
struct BoundedWorker {
    active: Arc<AtomicBool>,
    timeout: Duration,
}
impl BoundedWorker {
    fn new(timeout: Duration) -> Self {
        Self { active: Arc::new(AtomicBool::new(false)), timeout }
    }

    fn run<T: Send + 'static>(
        &self,
        task: impl FnOnce() -> Result<T, AccountStateError> + Send + 'static,
    ) -> Result<T, AccountStateError> {
        self.active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| AccountStateError::LookupBusy)?;
        let permit = WorkerPermit(Arc::clone(&self.active));
        let deadline = Instant::now() + self.timeout;
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("ryoku-account".into())
            .spawn(move || {
                let result = task();
                // Completion must release the worker before the caller can
                // observe its result and immediately request revalidation.
                // A stalled or panicking task still owns the permit until it
                // completes or unwinds; timeout never releases it early.
                drop(permit);
                // The receiver is unique to this invocation. Never cache or expose
                // a late account result to another request, even for the same UID.
                let _ = sender.send(result);
            })
            .map_err(|_| AccountStateError::LookupFailed)?;
        let result = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => AccountStateError::LookupTimedOut,
                mpsc::RecvTimeoutError::Disconnected => AccountStateError::LookupFailed,
            })?;
        // recv_timeout may observe a queued result after this caller was
        // descheduled. An expired invocation never returns a usable binding.
        if Instant::now() >= deadline {
            return Err(AccountStateError::LookupTimedOut);
        }
        result
    }
}

struct WorkerPermit(Arc<AtomicBool>);
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn account_worker() -> &'static BoundedWorker {
    ACCOUNT_WORKER.get_or_init(|| BoundedWorker::new(ACCOUNT_LOOKUP_TIMEOUT))
}

#[derive(Debug, Eq, PartialEq)]
pub enum AccountStateError {
    UnsupportedAccount,
    LookupFailed,
    LookupBusy,
    LookupTimedOut,
    InvalidAccount,
    AccountChanged,
    UnsafeDirectory,
    Filesystem(rustix::io::Errno),
}
impl std::fmt::Display for AccountStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "account state root unavailable: {self:?}")
    }
}
impl std::error::Error for AccountStateError {}
impl From<rustix::io::Errno> for AccountStateError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Filesystem(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Account {
    uid: u32,
    gid: u32,
    name: String,
    home: String,
}

/// NSS-resolved home for the executable user daemon. This is independent of
/// configuration enrollment and accepts no environment-selected path.
pub(crate) struct AccountHomeRoot {
    account: Account,
    home: File,
}
impl AccountHomeRoot {
    pub(crate) fn for_current_user() -> Result<Self, AccountStateError> {
        let uid = rustix::process::geteuid().as_raw();
        if uid == 0
            || uid != rustix::process::getuid().as_raw()
            || rustix::process::getgid() != rustix::process::getegid()
        {
            return Err(AccountStateError::UnsupportedAccount);
        }
        account_worker().run(move || {
            let account = lookup(uid)?;
            let root = File::open("/").map_err(|_| AccountStateError::LookupFailed)?;
            let home = open_directory(&root, &account.home[1..], false)?;
            validate_owner(&home, &account)?;
            Ok(Self { account, home })
        })
    }

    pub(crate) fn uid(&self) -> u32 {
        self.account.uid
    }
    pub(crate) fn home(&self) -> &File {
        &self.home
    }
    pub(crate) fn revalidate(&self) -> Result<(), AccountStateError> {
        let account = self.account.clone();
        let home = self.home.try_clone().map_err(|_| AccountStateError::LookupFailed)?;
        account_worker().run(move || {
            let current = lookup(account.uid)?;
            if account != current {
                return Err(AccountStateError::AccountChanged);
            }
            let root = File::open("/").map_err(|_| AccountStateError::LookupFailed)?;
            let current_home = open_directory(&root, &current.home[1..], false)?;
            validate_owner(&current_home, &current)?;
            require_same_directory(&home, &current_home)
        })
    }
}

/// Pins the selected account's existing default Ryoku directory. It does not
/// enroll any files, authorize capture/restore, or freeze session writers.
pub struct AccountStateRoot {
    account: Account,
    home: File,
    configuration_parent: File,
    configuration: File,
}
impl AccountStateRoot {
    /// Resolve only a kernel-authenticated peer UID. Authorization remains a
    /// separate gate. Account lookup and root opening run in a process-wide
    /// worker with a two-second response deadline and no queued work. A stuck
    /// NSS module retains the sole worker permit; timeout does not cancel it.
    /// Nondefault XDG roots are not accepted.
    pub fn for_peer(peer: &PeerIdentity) -> Result<Self, AccountStateError> {
        let uid = peer.uid();
        if uid == 0 || uid == u32::MAX {
            return Err(AccountStateError::UnsupportedAccount);
        }
        account_worker().run(move || {
            let account = lookup(uid)?;
            let root = File::open("/").map_err(|_| AccountStateError::LookupFailed)?;
            let (home, configuration_parent, configuration) = open_roots(&root, &account)?;
            Ok(Self { account, home, configuration_parent, configuration })
        })
    }
    pub fn uid(&self) -> u32 {
        self.account.uid
    }
    pub fn primary_gid(&self) -> u32 {
        self.account.gid
    }
    pub fn configuration(&self) -> &File {
        &self.configuration
    }
    /// Duplicate the already NSS-bound home for native capture. Revalidation
    /// remains required before every protected boundary.
    pub fn capture_home(&self) -> Result<File, AccountStateError> {
        self.home.try_clone().map_err(|_| AccountStateError::LookupFailed)
    }
    pub fn capture_account(
        &self,
    ) -> Result<ryoku_recovery::installed_capture::CaptureAccount, AccountStateError> {
        Ok(ryoku_recovery::installed_capture::CaptureAccount {
            uid: self.account.uid,
            gid: self.account.gid,
            principal: ryoku_recovery::AccountPrincipal::from_account(
                self.account.uid,
                self.account.gid,
                &self.account.name,
                &self.account.home,
            )
            .map_err(|_| AccountStateError::InvalidAccount)?,
        })
    }
    /// Re-resolve NSS and reopen the fixed path. A renamed/replaced directory,
    /// changed account identity or metadata cannot reuse the previous binding.
    /// Callers still retain the operation lock and freeze all enrolled writers.
    pub fn revalidate(&self) -> Result<(), AccountStateError> {
        let account = self.account.clone();
        let home = self.home.try_clone().map_err(|_| AccountStateError::LookupFailed)?;
        let configuration_parent =
            self.configuration_parent.try_clone().map_err(|_| AccountStateError::LookupFailed)?;
        let configuration =
            self.configuration.try_clone().map_err(|_| AccountStateError::LookupFailed)?;
        account_worker().run(move || {
            Self { account, home, configuration_parent, configuration }.revalidate_inner()
        })
    }

    fn revalidate_inner(&self) -> Result<(), AccountStateError> {
        let account = lookup(self.account.uid)?;
        let root = File::open("/").map_err(|_| AccountStateError::LookupFailed)?;
        self.revalidate_at(&root, &account)
    }

    fn revalidate_at(&self, root: &File, account: &Account) -> Result<(), AccountStateError> {
        if account != &self.account {
            return Err(AccountStateError::AccountChanged);
        }
        let (home, configuration_parent, configuration) = open_roots(root, account)?;
        for (old, current) in [
            (&self.home, &home),
            (&self.configuration_parent, &configuration_parent),
            (&self.configuration, &configuration),
        ] {
            require_same_directory(old, current)?;
        }
        Ok(())
    }
}

fn require_same_directory(old: &File, current: &File) -> Result<(), AccountStateError> {
    let old = fstat(old)?;
    let current = fstat(current)?;
    if old.st_dev != current.st_dev || old.st_ino != current.st_ino {
        return Err(AccountStateError::AccountChanged);
    }
    Ok(())
}

fn lookup(uid: u32) -> Result<Account, AccountStateError> {
    if uid == 0 || uid == u32::MAX {
        return Err(AccountStateError::UnsupportedAccount);
    }
    let mut buffer = vec![0_u8; ACCOUNT_BUFFER_BYTES];
    let mut entry = MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: entry and result are writable, correctly aligned output slots;
    // buffer is writable for its exact length and lives until fields are copied.
    #[allow(unsafe_code)]
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 {
        return Err(AccountStateError::LookupFailed);
    }
    if result.is_null() {
        return Err(AccountStateError::UnsupportedAccount);
    }
    if result != entry.as_mut_ptr() {
        return Err(AccountStateError::InvalidAccount);
    }
    // SAFETY: successful getpwuid_r with result == entry initialized the record.
    #[allow(unsafe_code)]
    let entry = unsafe { entry.assume_init() };
    let name = buffer_string(entry.pw_name, &buffer)?;
    let home = buffer_string(entry.pw_dir, &buffer)?;
    if entry.pw_uid != uid
        || entry.pw_gid == u32::MAX
        || name.is_empty()
        || name.len() > 256
        || name.chars().any(|c| c.is_control() || matches!(c, '/' | ':'))
        || !valid_home(home)
    {
        return Err(AccountStateError::InvalidAccount);
    }
    Ok(Account { uid, gid: entry.pw_gid, name: name.into(), home: home.into() })
}

// NSS string pointers must refer to terminated strings within the supplied
// bounded buffer. Inspect slices only; never dereference an unchecked pointer.
fn buffer_string(pointer: *const libc::c_char, buffer: &[u8]) -> Result<&str, AccountStateError> {
    let offset = (pointer as usize)
        .checked_sub(buffer.as_ptr() as usize)
        .filter(|offset| *offset < buffer.len())
        .ok_or(AccountStateError::InvalidAccount)?;
    let tail = &buffer[offset..];
    let length =
        tail.iter().position(|byte| *byte == 0).ok_or(AccountStateError::InvalidAccount)?;
    std::str::from_utf8(&tail[..length]).map_err(|_| AccountStateError::InvalidAccount)
}

fn valid_home(home: &str) -> bool {
    home.len() <= 4096
        && home.starts_with('/')
        && home != "/"
        && !home.ends_with('/')
        && !home.contains("//")
        && !home.chars().any(char::is_control)
        && home.split('/').skip(1).all(|part| !matches!(part, "" | "." | ".."))
        && Path::new(home).components().skip(1).all(|part| matches!(part, Component::Normal(_)))
}

fn open_roots(root: &File, account: &Account) -> Result<(File, File, File), AccountStateError> {
    if !valid_home(&account.home) {
        return Err(AccountStateError::InvalidAccount);
    }
    let home = open_directory(root, &account.home[1..], false)?;
    validate_owner(&home, account)?;
    let config = open_directory(&home, ".config", true)?;
    validate_owner(&config, account)?;
    let configuration = open_directory(&config, "ryoku", true)?;
    validate_owner(&configuration, account)?;
    Ok((home, config, configuration))
}
fn open_directory(parent: &File, path: &str, same_mount: bool) -> Result<File, AccountStateError> {
    let mut resolve =
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS;
    if same_mount {
        resolve |= ResolveFlags::NO_XDEV;
    }
    Ok(File::from(openat2(
        parent,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve,
    )?))
}
fn validate_owner(directory: &File, account: &Account) -> Result<(), AccountStateError> {
    let stat = fstat(directory)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != account.uid
        || stat.st_gid != account.gid
        || !matches!(stat.st_mode & 0o7777, 0o700 | 0o750 | 0o755)
    {
        return Err(AccountStateError::UnsafeDirectory);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn await_idle(worker: &BoundedWorker) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.active.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "worker did not release its permit");
            std::thread::yield_now();
        }
    }

    #[test]
    fn completed_lookup_allows_immediate_revalidation_without_waiting() {
        let worker = BoundedWorker::new(Duration::from_secs(2));
        for uid in 1000..1512 {
            let account = worker.run(move || Ok(uid)).unwrap();
            assert!(
                !worker.active.load(Ordering::Acquire),
                "published lookup result retained the sole worker permit"
            );
            // Mirrors startup's lookup followed immediately by pinned-home
            // revalidation, without a scheduling wait between those requests.
            assert_eq!(worker.run(move || Ok(account)), Ok(uid));
        }
        assert_eq!(
            worker.run(|| Err::<(), _>(AccountStateError::InvalidAccount)),
            Err(AccountStateError::InvalidAccount)
        );
        assert_eq!(worker.run(|| Ok(())), Ok(()));
    }

    #[test]
    fn stalled_lookup_times_out_without_growing_workers_or_reusing_late_results() {
        let worker = BoundedWorker::new(Duration::from_millis(100));
        let (release, stalled) = mpsc::channel();
        let start = Instant::now();
        assert_eq!(
            worker.run(move || {
                stalled.recv().unwrap();
                Ok(1000)
            }),
            Err(AccountStateError::LookupTimedOut)
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        for _ in 0..32 {
            assert_eq!(
                worker.run(|| -> Result<(), _> {
                    panic!("a stalled worker must retain the sole permit");
                }),
                Err(AccountStateError::LookupBusy)
            );
        }
        release.send(()).unwrap();
        await_idle(&worker);
        // The timed-out account result 1000 cannot be consumed by this peer.
        assert_eq!(worker.run(|| Ok(2000)), Ok(2000));
        await_idle(&worker);
    }

    #[test]
    fn worker_releases_permit_on_error_and_panic() {
        let worker = BoundedWorker::new(Duration::from_secs(1));
        assert_eq!(
            worker.run(|| Err::<(), _>(AccountStateError::InvalidAccount)),
            Err(AccountStateError::InvalidAccount)
        );
        await_idle(&worker);
        assert_eq!(
            worker.run(|| -> Result<(), _> { panic!("simulated NSS panic") }),
            Err(AccountStateError::LookupFailed)
        );
        await_idle(&worker);
        assert_eq!(worker.run(|| Ok(())), Ok(()));
        await_idle(&worker);
    }

    #[test]
    fn expired_worker_invocations_cannot_return_an_account_binding() {
        let worker = BoundedWorker::new(Duration::ZERO);
        assert_eq!(worker.run(|| Ok(1000)), Err(AccountStateError::LookupTimedOut));
        await_idle(&worker);
    }

    #[test]
    fn root_identity_is_refused_before_the_shared_worker() {
        let peer = PeerIdentity { uid: 0, gid: 0, supplementary_gids: vec![], pid: None };
        assert!(matches!(
            AccountStateRoot::for_peer(&peer),
            Err(AccountStateError::UnsupportedAccount)
        ));
    }

    #[test]
    fn account_strings_are_bounded_and_home_paths_are_canonical() {
        let bytes = b"alice\0/home/alice\0";
        assert_eq!(buffer_string(bytes.as_ptr().cast(), bytes).unwrap(), "alice");
        assert_eq!(buffer_string(std::ptr::null(), bytes), Err(AccountStateError::InvalidAccount));
        assert!(buffer_string(bytes[bytes.len()..].as_ptr().cast(), bytes).is_err());
        assert!(buffer_string(b"unterminated".as_ptr().cast(), b"unterminated").is_err());
        for path in [
            "/",
            "relative",
            "/home/../other",
            "/home/./user",
            "/home//user",
            "/home/user/",
            "/home/u\nser",
        ] {
            assert!(!valid_home(path), "{path:?}");
        }
        assert!(valid_home("/home/alice"));
        assert!(valid_home("/srv/accounts/alice"));
        assert_eq!(lookup(0), Err(AccountStateError::UnsupportedAccount));
        assert_eq!(lookup(u32::MAX), Err(AccountStateError::UnsupportedAccount));
    }

    #[test]
    fn roots_reject_symlinks_foreign_owners_and_writable_directories() {
        let path = std::env::temp_dir().join(format!(
            "ryoku-account-root-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let config = path.join("home/alice/.config/ryoku");
        fs::create_dir_all(&config).unwrap();
        let root = File::open(&path).unwrap();
        let stat = fstat(&root).unwrap();
        let account = Account {
            uid: stat.st_uid,
            gid: stat.st_gid,
            name: "alice".into(),
            home: "/home/alice".into(),
        };
        let (home, pinned_parent, pinned) = open_roots(&root, &account).unwrap();
        let binding = AccountStateRoot {
            account: account.clone(),
            home,
            configuration_parent: pinned_parent.try_clone().unwrap(),
            configuration: pinned.try_clone().unwrap(),
        };
        assert_eq!(binding.revalidate_at(&root, &account), Ok(()));
        for changed in [
            Account { uid: account.uid + 1, ..account.clone() },
            Account { gid: account.gid + 1, ..account.clone() },
            Account { name: "bob".into(), ..account.clone() },
            Account { home: "/home/bob".into(), ..account.clone() },
        ] {
            assert_eq!(
                binding.revalidate_at(&root, &changed),
                Err(AccountStateError::AccountChanged)
            );
        }
        assert_eq!(
            fstat(&pinned).unwrap().st_ino,
            fstat(File::open(&config).unwrap()).unwrap().st_ino
        );
        let foreign = Account { gid: account.gid + 1, ..account };
        assert!(matches!(open_roots(&root, &foreign), Err(AccountStateError::UnsafeDirectory)));
        let account = Account { gid: stat.st_gid, ..foreign };
        fs::set_permissions(&config, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(open_roots(&root, &account), Err(AccountStateError::UnsafeDirectory)));
        assert_eq!(binding.revalidate_at(&root, &account), Err(AccountStateError::UnsafeDirectory));
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(&config, config.with_file_name("previous")).unwrap();
        fs::create_dir(&config).unwrap();
        let (_, _, replacement) = open_roots(&root, &account).unwrap();
        assert_eq!(binding.revalidate_at(&root, &account), Err(AccountStateError::AccountChanged));
        assert_eq!(
            require_same_directory(&pinned, &replacement),
            Err(AccountStateError::AccountChanged)
        );
        fs::remove_dir(&config).unwrap();
        fs::rename(config.with_file_name("previous"), &config).unwrap();
        fs::rename(&config, config.with_file_name("retained")).unwrap();
        std::os::unix::fs::symlink("retained", &config).unwrap();
        assert!(open_roots(&root, &account).is_err());
        fs::remove_file(&config).unwrap();
        fs::rename(config.with_file_name("retained"), &config).unwrap();
        // Moving the original Ryoku inode under a replaced .config parent
        // must not preserve the earlier account binding.
        let parent = config.parent().unwrap();
        fs::rename(parent, parent.with_file_name("old-config")).unwrap();
        fs::create_dir(parent).unwrap();
        fs::rename(parent.with_file_name("old-config").join("ryoku"), &config).unwrap();
        let (_, replacement_parent, original_configuration) = open_roots(&root, &account).unwrap();
        assert_eq!(require_same_directory(&pinned, &original_configuration), Ok(()));
        assert_eq!(binding.revalidate_at(&root, &account), Err(AccountStateError::AccountChanged));
        assert_eq!(
            require_same_directory(&pinned_parent, &replacement_parent),
            Err(AccountStateError::AccountChanged)
        );
        fs::rename(path.join("home/alice"), path.join("home/retained")).unwrap();
        std::os::unix::fs::symlink("retained", path.join("home/alice")).unwrap();
        assert!(open_roots(&root, &account).is_err());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn private_account_directories_remain_private_after_binding_and_revalidation() {
        let path =
            std::env::temp_dir().join(format!("ryoku-private-account-{}", std::process::id()));
        let config = path.join("home/alice/.config/ryoku");
        fs::create_dir_all(&config).unwrap();
        for directory in [path.join("home/alice"), path.join("home/alice/.config"), config.clone()]
        {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let root = File::open(&path).unwrap();
        let metadata = fstat(&root).unwrap();
        let account = Account {
            uid: metadata.st_uid,
            gid: metadata.st_gid,
            name: "alice".into(),
            home: "/home/alice".into(),
        };
        let (home, configuration_parent, configuration) = open_roots(&root, &account).unwrap();
        let binding = AccountStateRoot {
            account: account.clone(),
            home,
            configuration_parent,
            configuration,
        };
        binding.revalidate_at(&root, &account).unwrap();
        for directory in [&binding.home, &binding.configuration_parent, &binding.configuration] {
            assert_eq!(fstat(directory).unwrap().st_mode & 0o7777, 0o700);
        }
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn native_nss_lookup_copies_the_running_ordinary_account() {
        let uid = rustix::process::getuid().as_raw();
        if uid == 0 {
            return;
        }
        let account = account_worker().run(move || lookup(uid)).unwrap();
        assert_eq!(account.uid, uid);
        assert!(valid_home(&account.home));
        assert!(!account.name.is_empty());
    }
}
