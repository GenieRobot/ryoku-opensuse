//! ACP adapter execution and bounded protocol. The configured artifact is
//! captured by identity and SHA256; no model message chooses a command or argv.
pub(crate) mod bridge;
mod client;
mod protocol;
pub use client::{Client, Handler};
pub use protocol::{Capabilities, Model, Session};
mod sandbox;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binary {
    path: String,
    dev: u64,
    ino: u64,
    bytes: u64,
    sha256: String,
}
impl Binary {
    pub fn enroll(path: String) -> Result<Self> {
        let (file, bytes) = read_binary(&path)?;
        let metadata = file.metadata().map_err(|_| Error::ProviderUnavailable)?;
        Ok(Self {
            path,
            dev: metadata.dev(),
            ino: metadata.ino(),
            bytes: metadata.len(),
            sha256: crate::digest(&bytes),
        })
    }
    fn capture(&self) -> Result<Vec<u8>> {
        let (file, bytes) = read_binary(&self.path)?;
        let metadata = file.metadata().map_err(|_| Error::ProviderUnavailable)?;
        if metadata.dev() != self.dev
            || metadata.ino() != self.ino
            || metadata.len() != self.bytes
            || crate::digest(&bytes) != self.sha256
        {
            return Err(Error::ProviderUnavailable);
        }
        Ok(bytes)
    }
}
fn read_binary(path: &str) -> Result<(File, Vec<u8>)> {
    use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
    if !Path::new(path).is_absolute() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(Error::InvalidRequest);
    }
    let fd = openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|_| Error::ProviderUnavailable)?;
    let mut file = File::from(fd);
    let before = file.metadata().map_err(|_| Error::ProviderUnavailable)?;
    if !before.is_file()
        || before.mode() & 0o022 != 0
        || before.mode() & 0o111 == 0
        || before.len() > 64 * 1024 * 1024
    {
        return Err(Error::UnsafeState);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::ProviderUnavailable)?;
    let after = file.metadata().map_err(|_| Error::ProviderUnavailable)?;
    if before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || bytes.len() as u64 != before.len()
        || !bytes.starts_with(b"\x7fELF")
    {
        return Err(Error::ProviderUnavailable);
    }
    Ok((file, bytes))
}
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arguments {
    #[default]
    Stdio,
    Acp,
    ExperimentalAcp,
}
impl Arguments {
    fn argv(self) -> &'static [&'static str] {
        match self {
            Self::Stdio => &[],
            Self::Acp => &["acp"],
            Self::ExperimentalAcp => &["--experimental-acp"],
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    binary: Binary,
    #[serde(default)]
    arguments: Arguments,
    /// Owner consent to adapter network access, independently of host files.
    #[serde(default)]
    network: bool,
}
impl Config {
    pub(crate) fn network_enabled(&self) -> bool {
        self.network
    }
    pub(crate) fn fingerprint(&self) -> &str {
        &self.binary.sha256
    }

    pub fn enroll(path: String, arguments: Arguments, network: bool) -> Result<Self> {
        Ok(Self { binary: Binary::enroll(path)?, arguments, network })
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if !Path::new(&self.binary.path).is_absolute()
            || self.binary.path.len() > 4096
            || self.binary.bytes > 64 * 1024 * 1024
            || self.binary.sha256.len() != 64
            || !self.binary.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

/// Executable-only bootstrap called before ordinary CLI/API handling.
pub fn worker() -> Result<()> {
    sandbox::worker()
}
