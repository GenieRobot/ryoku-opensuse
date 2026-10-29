//! Server-side library root resolution for retained Ryogami providers.
//!
//! The helper supplies its NSS-bound home/account and the frozen actor's
//! inspected XDG_PICTURES_DIR, never a request-supplied destination. Settings
//! are read only from the fixed account file. No directory is created here.
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use ryoku_recovery::{RelativePath, installed_capture::CaptureAccount, ryogami_assets::AssetPath};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    path::{Component, Path, PathBuf},
};

const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const MAX_SETTINGS: usize = 4 * 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub enum PathError {
    Account,
    Configuration,
    OutsideLibrary,
    UnsafeEntry,
    Capacity,
    Changed,
}
impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Account => "library account binding is unavailable",
            Self::Configuration => "library path configuration is invalid",
            Self::OutsideLibrary => "path is outside the account-owned library",
            Self::UnsafeEntry => "library path contains an unsafe owner, link or entry type",
            Self::Capacity => "library path input exceeds its bound",
            Self::Changed => "library configuration changed while resolving paths",
        })
    }
}
impl std::error::Error for PathError {}
type Result<T> = std::result::Result<T, PathError>;

#[derive(Clone, Eq, PartialEq)]
pub struct LibraryRoot {
    relative: RelativePath,
    exists: bool,
}
impl LibraryRoot {
    pub fn relative(&self) -> &RelativePath {
        &self.relative
    }
    pub fn exists(&self) -> bool {
        self.exists
    }
    /// A native provider still validates catalog/media identity and the exact
    /// current file before this path can enter its reviewed asset scope.
    pub fn asset(&self, relative: RelativePath) -> Result<AssetPath> {
        AssetPath::for_library(self.relative.clone(), relative)
            .map_err(|_| PathError::OutsideLibrary)
    }
}
#[derive(Clone, Eq, PartialEq)]
pub struct LibraryRoots {
    images: LibraryRoot,
    videos: LibraryRoot,
    input_digest: String,
    relocated_cache: bool,
}
impl LibraryRoots {
    pub fn images(&self) -> &LibraryRoot {
        &self.images
    }
    pub fn videos(&self) -> &LibraryRoot {
        &self.videos
    }
    pub fn input_digest(&self) -> &str {
        &self.input_digest
    }
    /// #25's fixed persistent-state capture rejects this configuration. The
    /// provider must retain that guard until a complete relocated catalog/state
    /// scope is captured; this flag never grants an exception.
    pub fn relocated_cache(&self) -> bool {
        self.relocated_cache
    }
}

pub fn resolve(
    home: &File,
    account: &CaptureAccount,
    actor_pictures: Option<&str>,
) -> Result<LibraryRoots> {
    let before = fs::fstat(home).map_err(|_| PathError::Account)?;
    if account.uid == 0
        || before.st_uid != account.uid
        || before.st_gid != account.gid
        || FileType::from_raw_mode(before.st_mode) != FileType::Directory
        || before.st_mode & 0o7022 != 0
    {
        return Err(PathError::Account);
    }
    let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
        .map_err(|_| PathError::Account)?;
    if !home_path.is_absolute() || !home_path.is_dir() {
        return Err(PathError::Account);
    }
    let bytes = settings(home, account)?;
    let config: serde_json::Value = bytes
        .as_ref()
        .map(|bytes| serde_json::from_slice(bytes))
        .transpose()
        .map_err(|_| PathError::Configuration)?
        .unwrap_or_else(|| serde_json::json!({}));
    if !config.is_object()
        || config.get("paths").is_some_and(|paths| !paths.is_object() && !paths.is_null())
    {
        return Err(PathError::Configuration);
    }
    let configured = |name: &str| -> Result<Option<&str>> {
        match config.get("paths").and_then(|paths| paths.get(name)) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => {
                Ok((!value.is_empty()).then_some(value.as_str()))
            }
            _ => Err(PathError::Configuration),
        }
    };
    let pictures = match actor_pictures.filter(|value| !value.is_empty()) {
        Some(value) => relative_root(&home_path, value)?,
        None => RelativePath::new("Pictures").map_err(|_| PathError::Configuration)?,
    };
    let pictures_sub = |sub: &str| -> Result<LibraryRoot> {
        let localized = checked_root(
            home,
            account,
            RelativePath::new(format!("{}/{sub}", pictures.as_str()))
                .map_err(|_| PathError::Configuration)?,
        )?;
        if localized.exists {
            return Ok(localized);
        }
        let english = checked_root(
            home,
            account,
            RelativePath::new(format!("Pictures/{sub}")).map_err(|_| PathError::Configuration)?,
        )?;
        Ok(if english.exists { english } else { localized })
    };
    let images = if let Some(value) = configured("wallpaper")? {
        checked_root(home, account, relative_root(&home_path, value)?)?
    } else {
        pictures_sub("Wallpapers")?
    };
    let videos = if let Some(value) = configured("videoWallpaper")? {
        checked_root(home, account, relative_root(&home_path, value)?)?
    } else {
        let live = pictures_sub("livewalls")?;
        if live.exists { live } else { images.clone() }
    };
    let relocated_cache = configured("cache")?.is_some_and(|value| {
        value != "~/.cache/ryogami" && Path::new(value) != home_path.join(".cache/ryogami")
    });
    if settings(home, account)? != bytes {
        return Err(PathError::Changed);
    }
    let after = fs::fstat(home).map_err(|_| PathError::Changed)?;
    if (before.st_dev, before.st_ino, before.st_uid, before.st_gid, before.st_mode)
        != (after.st_dev, after.st_ino, after.st_uid, after.st_gid, after.st_mode)
    {
        return Err(PathError::Changed);
    }
    let binding = serde_json::to_vec(&(
        account.principal.clone(),
        bytes,
        actor_pictures,
        images.relative.as_str(),
        images.exists,
        videos.relative.as_str(),
        videos.exists,
    ))
    .map_err(|_| PathError::Configuration)?;
    Ok(LibraryRoots {
        images,
        videos,
        input_digest: format!("{:x}", Sha256::digest(binding)),
        relocated_cache,
    })
}

fn relative_root(home: &Path, value: &str) -> Result<RelativePath> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(PathError::Capacity);
    }
    let path = if let Some(relative) = value.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(value)
    };
    let relative = path.strip_prefix(home).map_err(|_| PathError::OutsideLibrary)?;
    if relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(PathError::OutsideLibrary);
    }
    let relative = RelativePath::new(relative.to_str().ok_or(PathError::OutsideLibrary)?)
        .map_err(|_| PathError::OutsideLibrary)?;
    // Do not turn credential/configuration namespaces or the whole account home
    // into a media library. Custom/localized media folders are exact
    // account-owned destinations; hidden service data has its compiled selector.
    AssetPath::for_library(
        relative.clone(),
        RelativePath::new("probe").map_err(|_| PathError::OutsideLibrary)?,
    )
    .map_err(|_| PathError::OutsideLibrary)?;
    Ok(relative)
}
fn checked_root(
    home: &File,
    account: &CaptureAccount,
    relative: RelativePath,
) -> Result<LibraryRoot> {
    let mut directory = home.try_clone().map_err(|_| PathError::Account)?;
    let mut exists = true;
    for component in relative.as_str().split('/') {
        let next = match fs::openat2(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            NO_LINKS,
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => {
                exists = false;
                break;
            }
            Err(_) => return Err(PathError::UnsafeEntry),
        };
        let stat = fs::fstat(&next).map_err(|_| PathError::UnsafeEntry)?;
        if stat.st_uid != account.uid || stat.st_gid != account.gid || stat.st_mode & 0o7022 != 0 {
            return Err(PathError::UnsafeEntry);
        }
        directory = next;
    }
    Ok(LibraryRoot { relative, exists })
}
pub(crate) fn settings(home: &File, account: &CaptureAccount) -> Result<Option<Vec<u8>>> {
    let mut file = match fs::openat2(
        home,
        ".config/ryoku/ryogami.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        NO_LINKS,
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(PathError::UnsafeEntry),
    };
    let before = fs::fstat(&file).map_err(|_| PathError::UnsafeEntry)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_nlink != 1
        || before.st_uid != account.uid
        || before.st_gid != account.gid
        || before.st_mode & 0o7022 != 0
    {
        return Err(PathError::UnsafeEntry);
    }
    if before.st_size < 0 || before.st_size as usize > MAX_SETTINGS {
        return Err(PathError::Capacity);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_SETTINGS + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| PathError::Changed)?;
    let after = fs::fstat(&file).map_err(|_| PathError::Changed)?;
    if bytes.len() > MAX_SETTINGS
        || bytes.len() as u64 != before.st_size as u64
        || (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_ctime,
            before.st_ctime_nsec,
            before.st_mode,
        ) != (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_ctime,
            after.st_ctime_nsec,
            after.st_mode,
        )
    {
        return Err(PathError::Changed);
    }
    Ok(Some(bytes))
}

#[cfg(test)]
#[path = "ryogami_paths_tests.rs"]
mod tests;
