//! Native utilities used only by the explicitly selected disposable rollback fixture.
use rustix::{
    fd::AsFd,
    fs::{Mode, OFlags, ResolveFlags, openat2},
    ioctl::{Getter, ioctl},
    process::{Pid, Signal, kill_process_group},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const SMALL: u64 = 128 * 1024;
const COMMAND_LIMIT: u64 = 1024 * 1024;
const ASSET_LIMIT: u64 = 512 * 1024 * 1024;

pub fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}

pub fn open(root: &impl AsFd, relative: &str, directory: bool) -> Result<File> {
    let mut flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
    if directory {
        flags |= OFlags::DIRECTORY;
    }
    let file = File::from(openat2(
        root,
        relative,
        flags,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?);
    let meta = file.metadata()?;
    require(
        (if directory { meta.is_dir() } else { meta.is_file() })
            && meta.uid() == 0
            && meta.gid() == 0
            && meta.mode() & 0o022 == 0,
        "unsafe fixture path",
    )?;
    Ok(file)
}

pub fn read(root: &impl AsFd, relative: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open(root, relative, false)?.take(limit + 1).read_to_end(&mut bytes)?;
    require(bytes.len() as u64 <= limit, "fixture file exceeds bound")?;
    Ok(bytes)
}

pub fn text(root: &impl AsFd, relative: &str) -> Result<String> {
    Ok(String::from_utf8(read(root, relative, SMALL)?)?)
}

pub fn create(root: &impl AsFd, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = File::from(openat2(
        root,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )?);
    file.write_all(bytes)?;
    file.sync_all()?;
    rustix::fs::fsync(root)?;
    Ok(())
}

/// Replace only the exact baseline, retaining a uniquely named pending file on
/// any error. This fixture has no adoption, repair, or resume path.
pub fn replace(root: &impl AsFd, name: &str, before: &[u8], after: &[u8]) -> Result<()> {
    let original = open(root, name, false)?;
    let metadata = original.metadata()?;
    require(metadata.nlink() == 1, "multiply linked fixture replacement")?;
    require(read(root, name, SMALL)? == before, "file changed before fixture replacement")?;
    let pending = format!(".{name}.ryoku-rollback-pending");
    create(root, &pending, after)?;
    let pending_file = open(root, &pending, false)?;
    // Preserve SELinux labels and any existing ACL/xattrs on the two boot
    // configuration files. Unexpected inherited attributes stop publication.
    let attributes = xattrs(&original)?;
    for (name, value) in &attributes {
        rustix::fs::fsetxattr(
            &pending_file,
            name.as_str(),
            value,
            rustix::fs::XattrFlags::empty(),
        )?;
    }
    rustix::fs::fchmod(&pending_file, Mode::from_raw_mode(metadata.mode() & 0o7777))?;
    require(xattrs(&pending_file)? == attributes, "replacement inode attributes differ")?;
    pending_file.sync_all()?;
    require(read(root, name, SMALL)? == before, "file changed during fixture replacement")?;
    rustix::fs::renameat(root, pending.as_str(), root, name)?;
    rustix::fs::fsync(root)?;
    Ok(())
}

fn xattrs(file: &File) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    let mut names = [0_u8; 32768];
    let count = rustix::fs::flistxattr(file, &mut names)?;
    let mut attributes = std::collections::BTreeMap::new();
    for name in names[..count].split(|byte| *byte == 0).filter(|name| !name.is_empty()) {
        let name = std::str::from_utf8(name)?;
        let mut value = vec![0; 65536];
        let count = rustix::fs::fgetxattr(file, name, &mut value)?;
        value.truncate(count);
        attributes.insert(name.into(), value);
    }
    Ok(attributes)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    pub filesystem: String,
    pub id: u64,
    pub parent: u64,
    pub uuid: String,
    pub origin: String,
    pub readonly: bool,
}

fn uuid(bytes: &[u8; 16]) -> String {
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

#[allow(unsafe_code)]
pub fn volume(fd: &impl AsFd) -> Result<Volume> {
    type Fs = linux_raw_sys::btrfs::btrfs_ioctl_fs_info_args;
    type Sub = linux_raw_sys::btrfs::btrfs_ioctl_get_subvol_info_args;
    // SAFETY: requests and exact output layouts are the target Linux Btrfs
    // UAPI types. Both ioctls only read kernel metadata from the pinned fd.
    let (fs, sub) = unsafe {
        (
            ioctl(fd, Getter::<{ linux_raw_sys::ioctl::BTRFS_IOC_FS_INFO as _ }, Fs>::new())?,
            ioctl(
                fd,
                Getter::<{ linux_raw_sys::ioctl::BTRFS_IOC_GET_SUBVOL_INFO as _ }, Sub>::new(),
            )?,
        )
    };
    require(
        rustix::fs::fstat(fd)?.st_ino == 256 && fs.fsid != [0; 16] && sub.uuid != [0; 16],
        "not an identified Btrfs subvolume root",
    )?;
    Ok(Volume {
        filesystem: uuid(&fs.fsid),
        id: sub.treeid,
        parent: sub.parent_id,
        uuid: uuid(&sub.uuid),
        origin: uuid(&sub.parent_uuid),
        readonly: sub.flags & u64::from(linux_raw_sys::btrfs::BTRFS_ROOT_SUBVOL_RDONLY) != 0,
    })
}

pub fn hash(root: &impl AsFd, relative: &str) -> Result<String> {
    let mut file = open(root, relative, false)?;
    let before = file.metadata()?;
    require(before.len() <= ASSET_LIMIT, "asset exceeds 512 MiB")?;
    let end = Instant::now() + Duration::from_secs(20);
    let mut sha = Sha256::new();
    let mut buffer = [0; 65536];
    let mut size = 0;
    loop {
        require(Instant::now() < end, "asset hashing timed out")?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        require(size <= ASSET_LIMIT, "asset grew beyond bound")?;
        sha.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    require(
        size == before.len()
            && before.len() == after.len()
            && (before.mtime(), before.mtime_nsec()) == (after.mtime(), after.mtime_nsec())
            && (before.ctime(), before.ctime_nsec()) == (after.ctime(), after.ctime_nsec()),
        "asset changed during hashing",
    )?;
    Ok(format!("{:x}", sha.finalize()))
}

/// Fixed native programs only. Entire process groups and both output pipes are
/// bounded; a failed mutation is never retried. Full observations stay private.
pub fn command(program: &str, args: &[&str], work: Option<&str>) -> Result<String> {
    command_with_limit(program, args, work, COMMAND_LIMIT, false)
}

pub fn archive_listing(image: &str, work: &str) -> Result<String> {
    command_with_limit("/usr/bin/lsinitrd", &[image], Some(work), 4 * COMMAND_LIMIT, true)
}

fn command_with_limit(
    program: &str,
    args: &[&str],
    work: Option<&str>,
    limit: u64,
    summarize: bool,
) -> Result<String> {
    let root = File::open("/")?;
    require(
        open(&root, program.strip_prefix('/').ok_or("nonabsolute command")?, false)?
            .metadata()?
            .mode()
            & 0o111
            != 0,
        "native executable unavailable",
    )?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("LC_ALL", "C")
        .current_dir(work.unwrap_or("/"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(work) = work {
        cmd.env("TMPDIR", work);
    }
    let mut child = cmd.spawn()?;
    let drain = |pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(limit + 1).read_to_end(&mut bytes).map(|_| bytes)
        })
    };
    let stdout = drain(Box::new(child.stdout.take().ok_or("stdout pipe")?));
    let stderr = drain(Box::new(child.stderr.take().ok_or("stderr pipe")?));
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut status = None;
    let mut timed_out = false;
    loop {
        if status.is_none() {
            status = child.try_wait()?;
        }
        if status.is_some() && stdout.is_finished() && stderr.is_finished() {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            if let Some(pid) = Pid::from_raw(child.id() as i32) {
                let _ = kill_process_group(pid, Signal::KILL);
            }
            if status.is_none() {
                let _ = child.kill();
                status = Some(child.wait()?);
            }
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let stdout = stdout.join().map_err(|_| "stdout reader")??;
    let stderr = stderr.join().map_err(|_| "stderr reader")??;
    println!(
        "rollback_private_command={}",
        serde_json::json!({"program":program,"arguments":args,"exit_code":status.and_then(|s| s.code()),
            "timed_out":timed_out,
            "stdout": if summarize { format!("listing retained separately; bytes={} sha256={:x}",stdout.len(),Sha256::digest(&stdout)) } else { String::from_utf8_lossy(&stdout).into_owned() },
            "stderr":String::from_utf8_lossy(&stderr[..stderr.len().min(8192)])})
    );
    require(!timed_out, "native command timed out; retain fixture")?;
    require(
        stdout.len() as u64 <= limit && stderr.len() as u64 <= limit,
        "native command output exceeded bound; retain fixture",
    )?;
    require(status.is_some_and(|s| s.success()), "native command failed; retain fixture")?;
    require(stderr.is_empty(), "native command diagnostic requires review; retain fixture")?;
    Ok(String::from_utf8(stdout)?)
}
