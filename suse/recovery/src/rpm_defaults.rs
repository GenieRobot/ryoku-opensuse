//! Read-only installed RPM proof for the one port-owned generic default file.
//! No public package, path, root, queryformat or executable override exists.

use crate::{PackageName, Revision, Sha256, installed_profiles::DefaultsPackage};
use rustix::{
    fs::{OFlags, fcntl_getfl, fcntl_setfl},
    process::{Pid, Signal, kill_process_group},
};
use sha2::{Digest, Sha256 as Hash};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const DEFAULTS_PACKAGE: &str = "ryoku-profile-defaults";
pub const DEFAULTS_PATH: &str = "/usr/share/ryoku/config/profile.json";
const RPM: &str = "/usr/bin/rpm";
const MAX_STDOUT: usize = 128 * 1024;
const MAX_STDERR: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
// Scalar package facts followed by paired file-array rows. The selected path
// may occur anywhere in that array; never use the first file's digest.
const FORMAT: &str = "PKG\t%{NAME}\t%{EPOCHNUM}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\t%{FILEDIGESTALGO}\n[FILE\t%{FILENAMES}\t%{FILEDIGESTS}\t%{FILEMODES}\t%{FILEUSERNAME}\t%{FILEGROUPNAME}\t%{LONGFILESIZES}\t%{FILEFLAGS}\n]";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpmDefaultsError {
    SpawnFailed,
    TimedOut,
    OutputTooLarge,
    CommandFailed,
    InvalidInventory,
    WrongPackage,
    UnsupportedDigest,
    UnsafeDefaultRecord,
    DefaultFileUnavailable,
    ContentMismatch,
    ConcurrentChange,
}
impl std::fmt::Display for RpmDefaultsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "installed generic defaults unavailable: {self:?}")
    }
}
impl std::error::Error for RpmDefaultsError {}

/// Facts were queried from the installed RPM database and matched to actual
/// fixed-path bytes. This is not package-signature or repository trust proof;
/// installation policy and serialization with native RPM writers stay external.
pub struct VerifiedPackagedDefaults {
    package: DefaultsPackage,
    bytes: Vec<u8>,
}
impl VerifiedPackagedDefaults {
    pub fn load() -> Result<Self, RpmDefaultsError> {
        let root = File::open("/").map_err(|_| RpmDefaultsError::DefaultFileUnavailable)?;
        load_with(query_native, || {
            crate::installed_profiles::read_packaged_defaults(&root, 0, 0)
                .map_err(|_| RpmDefaultsError::DefaultFileUnavailable)
        })
    }
    pub fn package(&self) -> &DefaultsPackage {
        &self.package
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PackageFile {
    package: DefaultsPackage,
    digest: Sha256,
    size: u64,
}
fn load_with(
    mut query: impl FnMut() -> Result<Vec<u8>, RpmDefaultsError>,
    mut read: impl FnMut() -> Result<Vec<u8>, RpmDefaultsError>,
) -> Result<VerifiedPackagedDefaults, RpmDefaultsError> {
    let before = parse_inventory(&query()?)?;
    let bytes = read()?;
    if bytes.len() as u64 != before.size || digest(&bytes) != before.digest {
        return Err(RpmDefaultsError::ContentMismatch);
    }
    let after = parse_inventory(&query()?)?;
    if before != after || bytes != read()? {
        return Err(RpmDefaultsError::ConcurrentChange);
    }
    Ok(VerifiedPackagedDefaults { package: before.package, bytes })
}

fn parse_inventory(bytes: &[u8]) -> Result<PackageFile, RpmDefaultsError> {
    if bytes.len() > MAX_STDOUT {
        return Err(RpmDefaultsError::OutputTooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| RpmDefaultsError::InvalidInventory)?;
    if !text.ends_with('\n') || text.bytes().any(|byte| byte == 0 || byte == b'\r') {
        return Err(RpmDefaultsError::InvalidInventory);
    }
    let mut lines = text.lines();
    let header: Vec<_> =
        lines.next().ok_or(RpmDefaultsError::InvalidInventory)?.split('\t').collect();
    if header.len() != 7 || header[0] != "PKG" {
        return Err(RpmDefaultsError::InvalidInventory);
    }
    if header[1] != DEFAULTS_PACKAGE || header[5] != "noarch" {
        return Err(RpmDefaultsError::WrongPackage);
    }
    if header[6] != "8" {
        return Err(RpmDefaultsError::UnsupportedDigest);
    }
    let epoch = canonical_number(header[2])?;
    if epoch > u32::MAX as u64 || !version_component(header[3]) || !version_component(header[4]) {
        return Err(RpmDefaultsError::InvalidInventory);
    }
    let package = DefaultsPackage {
        name: PackageName::new(header[1]).map_err(|_| RpmDefaultsError::InvalidInventory)?,
        nevra: Revision::new(format!(
            "{}-{}:{}-{}.{}",
            header[1], epoch, header[3], header[4], header[5]
        ))
        .map_err(|_| RpmDefaultsError::InvalidInventory)?,
        // This JSON is newly authored port data, not an upstream import.
        upstream_revision: None,
    };
    let mut selected = None;
    for (index, line) in lines.enumerate() {
        if index >= 512 {
            return Err(RpmDefaultsError::OutputTooLarge);
        }
        let fields: Vec<_> = line.split('\t').collect();
        // A second owner header also fails here, even if byte-identical.
        if fields.len() != 8 || fields[0] != "FILE" {
            return Err(RpmDefaultsError::InvalidInventory);
        }
        if fields[1] != DEFAULTS_PATH {
            continue;
        }
        if selected.is_some() {
            return Err(RpmDefaultsError::InvalidInventory);
        }
        if fields[3] != "33188" || fields[4] != "root" || fields[5] != "root" || fields[7] != "0" {
            return Err(RpmDefaultsError::UnsafeDefaultRecord);
        }
        let size = canonical_number(fields[6])?;
        if size == 0 || size > ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES as u64 {
            return Err(RpmDefaultsError::UnsafeDefaultRecord);
        }
        selected =
            Some((Sha256::new(fields[2]).map_err(|_| RpmDefaultsError::UnsupportedDigest)?, size));
    }
    let (digest, size) = selected.ok_or(RpmDefaultsError::InvalidInventory)?;
    Ok(PackageFile { package, digest, size })
}
fn canonical_number(value: &str) -> Result<u64, RpmDefaultsError> {
    let parsed = value.parse::<u64>().map_err(|_| RpmDefaultsError::InvalidInventory)?;
    if parsed.to_string() != value {
        return Err(RpmDefaultsError::InvalidInventory);
    }
    Ok(parsed)
}
fn version_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'~' | b'^')
        })
}
fn query_native() -> Result<Vec<u8>, RpmDefaultsError> {
    run_bounded(native_command(), TIMEOUT)
}
fn native_command() -> Command {
    let mut command = Command::new(RPM);
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("HOME", "/var/empty")
        .args(["--noplugins", "--query", "--file", DEFAULTS_PATH, "--queryformat", FORMAT]);
    command
}
fn run_bounded(mut command: Command, timeout: Duration) -> Result<Vec<u8>, RpmDefaultsError> {
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| RpmDefaultsError::SpawnFailed)?;
    let result = collect_child(&mut child, timeout);
    if result.is_err() {
        terminate(&mut child);
    }
    result
}
fn collect_child(child: &mut Child, timeout: Duration) -> Result<Vec<u8>, RpmDefaultsError> {
    let mut stdout = child.stdout.take().ok_or(RpmDefaultsError::SpawnFailed)?;
    let mut stderr = child.stderr.take().ok_or(RpmDefaultsError::SpawnFailed)?;
    for descriptor in [&stdout as &dyn std::os::fd::AsFd, &stderr as &dyn std::os::fd::AsFd] {
        let flags = fcntl_getfl(descriptor).map_err(|_| RpmDefaultsError::CommandFailed)?;
        fcntl_setfl(descriptor, flags | OFlags::NONBLOCK)
            .map_err(|_| RpmDefaultsError::CommandFailed)?;
    }
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    loop {
        if Instant::now() >= deadline {
            return Err(RpmDefaultsError::TimedOut);
        }
        // One bounded read per pipe per iteration prevents an always-writable
        // producer from starving timeout checks or the other pipe.
        if !stdout_done {
            stdout_done = read_chunk(&mut stdout, &mut output, MAX_STDOUT)?;
        }
        if !stderr_done {
            stderr_done = read_chunk(&mut stderr, &mut errors, MAX_STDERR)?;
        }
        if let Some(status) = child.try_wait().map_err(|_| RpmDefaultsError::CommandFailed)?
            && stdout_done
            && stderr_done
        {
            return if status.success() {
                Ok(output)
            } else {
                Err(RpmDefaultsError::CommandFailed)
            };
        }
        thread::sleep(Duration::from_millis(2));
    }
}
fn read_chunk(
    reader: &mut impl Read,
    captured: &mut Vec<u8>,
    limit: usize,
) -> Result<bool, RpmDefaultsError> {
    let mut bytes = [0_u8; 8192];
    match reader.read(&mut bytes) {
        Ok(0) => Ok(true),
        Ok(length) => {
            if length > limit.saturating_sub(captured.len()) {
                return Err(RpmDefaultsError::OutputTooLarge);
            }
            captured.extend_from_slice(&bytes[..length]);
            Ok(false)
        }
        Err(error)
            if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) =>
        {
            Ok(false)
        }
        Err(_) => Err(RpmDefaultsError::CommandFailed),
    }
}
fn terminate(child: &mut Child) {
    if let Some(pid) = Pid::from_raw(child.id() as i32) {
        let _ = kill_process_group(pid, Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
fn digest(bytes: &[u8]) -> Sha256 {
    Sha256::new(Hash::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>())
        .expect("SHA256 hex is valid")
}

#[cfg(test)]
#[path = "rpm_defaults_tests.rs"]
mod tests;
