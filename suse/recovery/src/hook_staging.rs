//! Private computation home and exact declared commit for configured hooks.
//! The tmpfs has a hard byte/inode quota; it is never mounted in the parent.
#![allow(unsafe_code)] // Namespace setup is isolated to the pre-exec child.
use crate::{
    EntryKind, RelativePath, StateRoot,
    capture::{CaptureLimits, capture_inventory},
    configuration_stage::StagedEntry,
    hook_protocol::{HookCall, HookFileIdentity},
    inode_metadata::prepare_inode_metadata,
};
use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::{
            fs::{DirBuilderExt, MetadataExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const RESOLVE: rustix::fs::ResolveFlags = rustix::fs::ResolveFlags::BENEATH
    .union(rustix::fs::ResolveFlags::NO_SYMLINKS)
    .union(rustix::fs::ResolveFlags::NO_XDEV);
const MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) struct HookStage {
    _directory: StageDirectory,
    receiver: UnixStream,
    _sender: UnixStream,
    plan: NamespacePlan,
    source: File,
    entries: Vec<StagedEntry>,
    identities: Vec<HookFileIdentity>,
}
struct StageDirectory {
    path: PathBuf,
    identity: (u64, u64),
}
impl StageDirectory {
    fn new() -> io::Result<Self> {
        let path = PathBuf::from("/tmp").join(format!(
            "ryoku-hook-stage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        let directory = Self { path, identity: (metadata.dev(), metadata.ino()) };
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(io::Error::other("private staging directory ownership unavailable"));
        }
        Ok(directory)
    }
}
impl Drop for StageDirectory {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && (metadata.dev(), metadata.ino()) == self.identity
        {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}
#[derive(Clone)]
struct Seed {
    path: CString,
    bytes: Arc<[u8]>,
    mode: u32,
    seconds: i64,
    nanos: i64,
}
#[derive(Clone)]
pub(crate) struct NamespacePlan {
    home: CString,
    stage: CString,
    uid_map: CString,
    gid_map: CString,
    mount_options: CString,
    directories: Vec<CString>,
    seeds: Vec<Seed>,
    sender: i32,
}
impl HookStage {
    pub(crate) fn prepare(home: &File, home_path: &str, call: &HookCall) -> io::Result<Self> {
        let paths = call
            .policy
            .approval
            .outputs
            .iter()
            .map(|output| {
                RelativePath::new(output.path())
                    .map_err(|_| io::Error::other("invalid declared output path"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let inventory =
            capture_inventory(home, StateRoot::UserHome, &paths, CaptureLimits::default())
                .map_err(io::Error::other)?;
        let directory = StageDirectory::new()?;
        let (receiver, sender) = UnixStream::pair()?;
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut directories = BTreeSet::new();
        let mut entries = Vec::new();
        let mut seeds = Vec::new();
        for record in inventory.records {
            let mut file = open_file(home, record.path.as_str())?;
            let metadata = file.metadata()?;
            let identity = call
                .output_identities
                .iter()
                .find(|identity| identity.output.path() == record.path.as_str())
                .ok_or_else(|| io::Error::other("missing output identity"))?;
            if metadata.dev() != identity.device
                || metadata.ino() != identity.inode
                || metadata.uid() != call.policy.uid
                || metadata.nlink() != 1
            {
                return Err(io::Error::other("staging source identity changed"));
            }
            let mut bytes = Vec::new();
            Read::by_ref(&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            let EntryKind::RegularFile { metadata: preserved, .. } = &record.kind else {
                return Err(io::Error::other("staging requires exact existing regular outputs"));
            };
            if bytes.len() as u64 > MAX_BYTES {
                return Err(io::Error::other("staging source exceeds output bound"));
            }
            for parent in Path::new(record.path.as_str()).ancestors().skip(1) {
                if !parent.as_os_str().is_empty() {
                    directories.insert(parent.to_string_lossy().into_owned());
                }
            }
            seeds.push(Seed {
                path: CString::new(record.path.as_str())?,
                bytes: Arc::from(bytes.clone()),
                mode: preserved.mode,
                seconds: preserved
                    .mtime_unix_ns
                    .div_euclid(1_000_000_000)
                    .try_into()
                    .map_err(io::Error::other)?,
                nanos: preserved.mtime_unix_ns.rem_euclid(1_000_000_000) as i64,
            });
            entries.push(
                StagedEntry::from_captured_regular(record, bytes, &inventory.metadata_payloads)
                    .map_err(io::Error::other)?,
            );
        }
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        let plan = NamespacePlan {
            home: CString::new(home_path)?,
            stage: CString::new(directory.path.as_os_str().as_encoded_bytes())?,
            uid_map: CString::new(format!("{uid} {uid} 1\n"))?,
            gid_map: CString::new(format!("{gid} {gid} 1\n"))?,
            mount_options: CString::new(format!(
                "size=64m,nr_inodes=256,mode=0700,uid={uid},gid={gid}"
            ))?,
            directories: directories
                .into_iter()
                .map(CString::new)
                .collect::<Result<Vec<_>, _>>()?,
            seeds,
            sender: sender.as_raw_fd(),
        };
        Ok(Self {
            _directory: directory,
            receiver,
            _sender: sender,
            plan,
            source: home.try_clone()?,
            entries,
            identities: call.output_identities.clone(),
        })
    }
    pub(crate) fn namespace(&self) -> NamespacePlan {
        self.plan.clone()
    }
    pub(crate) fn commit(&self) -> io::Result<()> {
        let staged = self.receive_root()?;
        let mut expected = BTreeSet::new();
        for entry in &self.entries {
            let path = Path::new(entry.record().path.as_str());
            expected.insert(path.to_path_buf());
            for parent in path.ancestors().skip(1) {
                if !parent.as_os_str().is_empty() {
                    expected.insert(parent.to_path_buf());
                }
            }
        }
        let mut actual = BTreeSet::new();
        inspect_tree(&staged, Path::new(""), &mut actual)?;
        if actual != expected {
            return Err(io::Error::other(
                "hook produced undeclared staged entries; no real output was committed",
            ));
        }
        self.revalidate_sources()?;
        let mut prepared = Vec::new();
        for entry in &self.entries {
            let path = entry.record().path.as_str();
            let mut file = open_file(&staged, path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_BYTES {
                return Err(io::Error::other("unsafe staged hook output"));
            }
            let mut bytes = Vec::new();
            Read::by_ref(&mut file).take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(io::Error::other("staged output exceeds bound"));
            }
            let mut record = entry.record().clone();
            let EntryKind::RegularFile { metadata: preserved, size, sha256, .. } = &mut record.kind
            else {
                return Err(io::Error::other("invalid captured hook output"));
            };
            *size = bytes.len() as u64;
            *sha256 = crate::Sha256::new(crate::wallpaper_protocol::digest(&bytes))
                .map_err(|_| io::Error::other("invalid staged output digest"))?;
            preserved.mtime_unix_ns =
                i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
            let after = entry.with_regular_content(record, bytes).map_err(io::Error::other)?;
            prepared.push(Prepared::new(&self.source, &after)?);
        }
        self.revalidate_sources()?;
        publish_all(&self.source, &self.entries, &prepared)
    }
    fn revalidate_sources(&self) -> io::Result<()> {
        for entry in &self.entries {
            let file = open_file(&self.source, entry.record().path.as_str())?;
            let metadata = file.metadata()?;
            let identity = self
                .identities
                .iter()
                .find(|identity| identity.output.path() == entry.record().path.as_str())
                .ok_or_else(|| io::Error::other("missing captured identity"))?;
            if metadata.dev() != identity.device || metadata.ino() != identity.inode {
                return Err(io::Error::other("real hook output was replaced during staging"));
            }
        }
        let paths =
            self.entries.iter().map(|entry| entry.record().path.clone()).collect::<Vec<_>>();
        let current =
            capture_inventory(&self.source, StateRoot::UserHome, &paths, CaptureLimits::default())
                .map_err(io::Error::other)?;
        if current.records
            != self.entries.iter().map(|entry| entry.record().clone()).collect::<Vec<_>>()
        {
            return Err(io::Error::other("real hook output changed during staging"));
        }
        Ok(())
    }
    fn receive_root(&self) -> io::Result<File> {
        use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};
        use std::{io::IoSliceMut, mem::MaybeUninit};
        let mut byte = [0];
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let result = recvmsg(
            &self.receiver,
            &mut [IoSliceMut::new(&mut byte)],
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC,
        )?;
        let mut fds: Vec<OwnedFd> = Vec::new();
        for message in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(received) = message {
                fds.extend(received);
            } else {
                return Err(io::Error::other("invalid staging descriptor message"));
            }
        }
        if result.bytes != 1
            || byte != *b"S"
            || result
                .flags
                .intersects(rustix::net::ReturnFlags::TRUNC | rustix::net::ReturnFlags::CTRUNC)
            || fds.len() != 1
        {
            return Err(io::Error::other("incomplete staging descriptor"));
        }
        let file = File::from(fds.pop().expect("one checked descriptor"));
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(io::Error::other("staging root ownership changed"));
        }
        let filesystem = rustix::fs::fstatfs(&file)?;
        if filesystem.f_type as u64 != 0x0102_1994
            || filesystem.f_blocks.saturating_mul(filesystem.f_bsize as u64) > 64 * 1024 * 1024
            || filesystem.f_files > 256
        {
            return Err(io::Error::other(
                "staging descriptor does not identify the quota-bounded tmpfs",
            ));
        }
        Ok(file)
    }
}

// Every replacement inode is complete before publishing the first name.
// Failed publication compensates all attempted paths from exact preimages.
fn publish_all(home: &File, before: &[StagedEntry], prepared: &[Prepared]) -> io::Result<()> {
    for (index, item) in prepared.iter().enumerate() {
        if let Err(error) = item.publish() {
            let mut compensation_failed = false;
            for before in before[..=index].iter().rev() {
                if Prepared::new(home, before).and_then(|entry| entry.publish()).is_err() {
                    compensation_failed = true;
                }
            }
            return Err(io::Error::other(format!(
                "hook commit failed: {error}; compensation_failed={compensation_failed}; inspect linked recovery"
            )));
        }
    }
    Ok(())
}

fn open_file(root: &File, path: &str) -> io::Result<File> {
    Ok(File::from(rustix::fs::openat2(
        root,
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        RESOLVE,
    )?))
}
fn inspect_tree(root: &File, relative: &Path, paths: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    let directory = if relative.as_os_str().is_empty() {
        root.try_clone()?
    } else {
        File::from(rustix::fs::openat2(
            root,
            relative,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            RESOLVE,
        )?)
    };
    for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if paths.len() >= 256 || (!kind.is_file() && !kind.is_dir()) {
            return Err(io::Error::other("undeclared/unsafe or oversized staged tree"));
        }
        paths.insert(path.clone());
        if kind.is_dir() {
            inspect_tree(root, &path, paths)?;
        }
    }
    Ok(())
}
struct Prepared {
    parent: File,
    temporary: String,
    leaf: String,
    published: std::cell::Cell<bool>,
}
impl Prepared {
    fn new(home: &File, entry: &StagedEntry) -> io::Result<Self> {
        let path = Path::new(entry.record().path.as_str());
        let parent = File::from(rustix::fs::openat2(
            home,
            path.parent().ok_or_else(|| io::Error::other("output parent missing"))?,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            RESOLVE,
        )?);
        let metadata = parent.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o7022 != 0 {
            return Err(io::Error::other(
                "hook output parent must be owned and privately writable",
            ));
        }
        let leaf = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| io::Error::other("invalid output leaf"))?
            .to_string();
        let temporary =
            format!(".ryoku-hook-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let mut file = File::from(rustix::fs::openat2(
            &parent,
            &temporary,
            rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::EXCL
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_raw_mode(0o600),
            RESOLVE,
        )?);
        let prepared = Self { parent, temporary, leaf, published: std::cell::Cell::new(false) };
        file.write_all(entry.content().ok_or_else(|| io::Error::other("missing staged bytes"))?)?;
        prepare_inode_metadata(&file, entry).map_err(io::Error::other)?;
        Ok(prepared)
    }
    fn publish(&self) -> io::Result<()> {
        rustix::fs::renameat(&self.parent, &self.temporary, &self.parent, &self.leaf)?;
        self.published.set(true);
        self.parent.sync_all()
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.published.get() {
            let _ =
                rustix::fs::unlinkat(&self.parent, &self.temporary, rustix::fs::AtFlags::empty());
        }
    }
}

#[repr(C)]
struct MountAttr {
    set: u64,
    clear: u64,
    propagation: u64,
    userns: u64,
}
impl NamespacePlan {
    /// Only prebuilt buffers and async-signal-safe syscalls are used after fork.
    pub(crate) unsafe fn install(&self) -> io::Result<i32> {
        // SAFETY: this method executes only in the fork child before its first exec.
        unsafe {
            if libc::unshare(libc::CLONE_NEWUSER) != 0 {
                return Err(io::Error::last_os_error());
            }
            write_control(c"/proc/self/setgroups", b"deny")?;
            write_control(c"/proc/self/uid_map", self.uid_map.as_bytes())?;
            write_control(c"/proc/self/gid_map", self.gid_map.as_bytes())?;
            if libc::unshare(libc::CLONE_NEWNS | libc::CLONE_NEWNET) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            let attr = MountAttr { set: 1, clear: 0, propagation: 0, userns: 0 };
            if libc::syscall(
                libc::SYS_mount_setattr,
                libc::AT_FDCWD,
                c"/".as_ptr(),
                0x8000,
                &attr,
                std::mem::size_of::<MountAttr>(),
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::mount(
                c"tmpfs".as_ptr(),
                self.stage.as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                self.mount_options.as_ptr().cast(),
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            let root = libc::open(
                self.stage.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            );
            if root < 0 {
                return Err(io::Error::last_os_error());
            }
            for directory in &self.directories {
                if libc::mkdirat(root, directory.as_ptr(), 0o700) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            for seed in &self.seeds {
                let fd = libc::openat(
                    root,
                    seed.path.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                );
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                write_bytes(fd, &seed.bytes)?;
                let timestamp = libc::timespec { tv_sec: seed.seconds, tv_nsec: seed.nanos };
                let times = [timestamp, timestamp];
                if libc::fchmod(fd, seed.mode) != 0 || libc::futimens(fd, times.as_ptr()) != 0 {
                    return Err(io::Error::last_os_error());
                }
                libc::close(fd);
            }
            if libc::mount(
                self.stage.as_ptr(),
                self.home.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            ) != 0
                || libc::chdir(self.home.as_ptr()) != 0
            {
                return Err(io::Error::last_os_error());
            }
            send_root(self.sender, root)?;
            Ok(root)
        }
    }
}
unsafe fn write_control(path: &std::ffi::CStr, bytes: &[u8]) -> io::Result<()> {
    // SAFETY: caller is the pre-exec child; path and bytes are valid prebuilt buffers.
    unsafe {
        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = write_bytes(fd, bytes);
        libc::close(fd);
        result
    }
}
unsafe fn write_bytes(fd: i32, mut bytes: &[u8]) -> io::Result<()> {
    // SAFETY: initialized byte slice remains live until each write returns.
    unsafe {
        while !bytes.is_empty() {
            let count = libc::write(fd, bytes.as_ptr().cast(), bytes.len());
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            bytes = &bytes[count as usize..];
        }
        Ok(())
    }
}
unsafe fn send_root(socket: i32, root: i32) -> io::Result<()> {
    #[repr(C)]
    struct Rights {
        header: libc::cmsghdr,
        fd: i32,
        padding: i32,
    }
    // SAFETY: native x86_64 CMSG layout; all buffers remain initialized and live for sendmsg.
    unsafe {
        let mut rights = Rights {
            header: libc::cmsghdr {
                cmsg_len: libc::CMSG_LEN(4) as usize,
                cmsg_level: libc::SOL_SOCKET,
                cmsg_type: libc::SCM_RIGHTS,
            },
            fd: root,
            padding: 0,
        };
        let mut byte = b'S';
        let mut vector = libc::iovec { iov_base: (&mut byte as *mut u8).cast(), iov_len: 1 };
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = (&mut rights as *mut Rights).cast();
        message.msg_controllen = libc::CMSG_SPACE(4) as usize;
        if libc::sendmsg(socket, &message, libc::MSG_NOSIGNAL) != 1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn second_publication_failure_compensates_both_exact_preimages_and_metadata() {
        let directory = StageDirectory::new().unwrap();
        let paths = [".config/kitty/user.conf", ".config/fish/user.fish"];
        for path in paths {
            let path = directory.path.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"before").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let home = File::open(&directory.path).unwrap();
        let inventory = capture_inventory(
            &home,
            StateRoot::UserHome,
            &paths.map(|path| RelativePath::new(path).unwrap()),
            CaptureLimits::default(),
        )
        .unwrap();
        let original_records = inventory.records.clone();
        let before = inventory
            .records
            .into_iter()
            .map(|record| {
                StagedEntry::from_captured_regular(
                    record,
                    b"before".to_vec(),
                    &inventory.metadata_payloads,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let prepared = before
            .iter()
            .map(|entry| {
                let mut record = entry.record().clone();
                let EntryKind::RegularFile { size, sha256, .. } = &mut record.kind else {
                    unreachable!()
                };
                *size = 5;
                *sha256 = crate::Sha256::new(crate::wallpaper_protocol::digest(b"after")).unwrap();
                Prepared::new(
                    &home,
                    &entry.with_regular_content(record, b"after".to_vec()).unwrap(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        // Real rename failure after the first name was published; no simulated
        // success/failure branch is introduced into the production writer.
        rustix::fs::unlinkat(
            &prepared[1].parent,
            &prepared[1].temporary,
            rustix::fs::AtFlags::empty(),
        )
        .unwrap();
        let error = publish_all(&home, &before, &prepared).unwrap_err();
        assert!(error.to_string().contains("compensation_failed=false"), "{error}");
        let after = capture_inventory(
            &home,
            StateRoot::UserHome,
            &paths.map(|path| RelativePath::new(path).unwrap()),
            CaptureLimits::default(),
        )
        .unwrap();
        assert_eq!(after.records, original_records);
        drop(prepared);
        for path in paths {
            assert_eq!(std::fs::read(directory.path.join(path)).unwrap(), b"before");
            assert_eq!(
                std::fs::read_dir(directory.path.join(path).parent().unwrap()).unwrap().count(),
                1
            );
        }
        std::fs::remove_dir_all(directory.path.join(".config")).unwrap();
    }
}
