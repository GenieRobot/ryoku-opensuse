//! Reviewed media selection and exact recoverable trash scope. Public RPCs
//! supply keys only; catalog paths are resolved under authenticated owned
//! library/cache roots before they become a mutation plan.
use crate::ryogami_paths::{self, LibraryRoots};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use ryoku_recovery::{
    EntryKind, RelativePath, Sha256,
    installed_capture::CaptureAccount,
    ryogami_assets::{AssetInventory, AssetNamespace, AssetPath, AssetScope, MAX_CAPTURE_LIMITS},
    ryogami_media_protocol::{CatalogCommit, CatalogMutation, MAX_ENTRY_BYTES, valid_key},
    wallpaper_protocol::digest,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs::File, io::Read, os::fd::AsRawFd, path::Path};
const CATALOG: &str = ".cache/ryogami/wallpaper/index.json";
const CATALOG_LIMIT: u64 = 16 * 1024 * 1024;
const NO_LINKS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
#[derive(Debug)]
pub enum MediaError {
    Input,
    Account,
    Catalog,
    Scope,
    Changed,
    Interrupted,
    Receipt,
    Io,
}
impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Input => "invalid media selection",
            Self::Account => "media account binding unavailable",
            Self::Catalog => "media catalog unavailable or changed",
            Self::Scope => "media path enrollment unavailable",
            Self::Changed => "media transaction state changed",
            Self::Interrupted => "media transaction interrupted; linked recovery retained",
            Self::Receipt => "protected media trash receipt unavailable",
            Self::Io => "media transaction I/O failed",
        })
    }
}
impl std::error::Error for MediaError {}
type Result<T> = std::result::Result<T, MediaError>;
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    changed: (i64, u64),
    sha256: Sha256,
}
struct Catalog {
    bytes: Vec<u8>,
    identity: FileIdentity,
    rows: serde_json::Map<String, serde_json::Value>,
}
fn read_catalog(home: &File, account: &CaptureAccount) -> Result<Catalog> {
    let file = File::from(
        fs::openat2(
            home,
            CATALOG,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            NO_LINKS,
        )
        .map_err(|_| MediaError::Catalog)?,
    );
    let before = fs::fstat(&file).map_err(|_| MediaError::Catalog)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_uid != account.uid
        || before.st_gid != account.gid
        || before.st_nlink != 1
        || before.st_mode & 0o7022 != 0
        || before.st_size < 0
        || before.st_size as u64 > CATALOG_LIMIT
    {
        return Err(MediaError::Catalog);
    }
    let mut bytes = Vec::new();
    (&file).take(CATALOG_LIMIT + 1).read_to_end(&mut bytes).map_err(|_| MediaError::Catalog)?;
    let after = fs::fstat(&file).map_err(|_| MediaError::Catalog)?;
    if before.st_dev != after.st_dev
        || before.st_ino != after.st_ino
        || before.st_mode != after.st_mode
        || before.st_uid != after.st_uid
        || before.st_gid != after.st_gid
        || before.st_nlink != after.st_nlink
        || before.st_size != after.st_size
        || before.st_ctime != after.st_ctime
        || before.st_ctime_nsec != after.st_ctime_nsec
        || bytes.len() as i64 != before.st_size
        || bytes.len() as u64 > CATALOG_LIMIT
    {
        return Err(MediaError::Changed);
    }
    let rows = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|_| MediaError::Catalog)?
        .as_object()
        .cloned()
        .ok_or(MediaError::Catalog)?;
    if rows.len() > 65536 {
        return Err(MediaError::Catalog);
    }
    let identity = FileIdentity {
        device: before.st_dev,
        inode: before.st_ino,
        size: bytes.len() as u64,
        mode: before.st_mode,
        uid: before.st_uid,
        gid: before.st_gid,
        changed: (before.st_ctime, before.st_ctime_nsec),
        sha256: Sha256::new(digest(&bytes)).map_err(|_| MediaError::Catalog)?,
    };
    Ok(Catalog { bytes, identity, rows })
}
fn home_path(home: &File) -> Result<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))
        .map_err(|_| MediaError::Account)
}
fn relative(home: &Path, path: &str) -> Result<RelativePath> {
    let value = if let Some(path) = path.strip_prefix("~/") {
        path
    } else {
        Path::new(path).strip_prefix(home).ok().and_then(Path::to_str).ok_or(MediaError::Scope)?
    };
    if path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(MediaError::Scope);
    }
    RelativePath::new(value).map_err(|_| MediaError::Scope)
}
fn library_asset(roots: &LibraryRoots, path: &RelativePath) -> Result<AssetPath> {
    for root in [roots.images(), roots.videos()] {
        if let Some(tail) = path
            .as_str()
            .strip_prefix(root.relative().as_str())
            .and_then(|tail| tail.strip_prefix('/'))
        {
            return root
                .asset(RelativePath::new(tail).map_err(|_| MediaError::Scope)?)
                .map_err(|_| MediaError::Scope);
        }
    }
    Err(MediaError::Scope)
}
fn cache_asset(path: &RelativePath) -> Result<AssetPath> {
    let tail = path.as_str().strip_prefix(".cache/ryogami/wallpaper/").ok_or(MediaError::Scope)?;
    AssetPath::new(
        AssetNamespace::WallpaperCache,
        RelativePath::new(tail).map_err(|_| MediaError::Scope)?,
    )
    .map_err(|_| MediaError::Scope)
}
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaMove {
    source: AssetPath,
    trash: AssetPath,
}
impl MediaMove {
    pub fn source(&self) -> &AssetPath {
        &self.source
    }
    pub fn trash(&self) -> &AssetPath {
        &self.trash
    }
}
#[derive(Clone)]
pub struct PreparedMedia {
    commit: CatalogCommit,
    row: serde_json::Value,
    catalog: FileIdentity,
    original_rows: serde_json::Map<String, serde_json::Value>,
    library_binding: String,
    scope: AssetScope,
    inventory: AssetInventory,
    moves: Vec<MediaMove>,
    parents: Vec<trash::ParentBinding>,
}
impl PreparedMedia {
    pub fn delete(
        home: &File,
        account: &CaptureAccount,
        actor_pictures: Option<&str>,
        key: &str,
        token: Sha256,
    ) -> Result<Self> {
        if !valid_key(key) {
            return Err(MediaError::Input);
        }
        let roots =
            ryogami_paths::resolve(home, account, actor_pictures).map_err(|_| MediaError::Scope)?;
        if roots.relocated_cache() {
            return Err(MediaError::Scope);
        }
        let catalog = read_catalog(home, account)?;
        let row = catalog.rows.get(key).ok_or(MediaError::Input)?.clone();
        if serde_json::to_vec(&row).map_err(|_| MediaError::Catalog)?.len() > MAX_ENTRY_BYTES {
            return Err(MediaError::Catalog);
        }
        let text = |field: &str| -> Result<&str> {
            match row.get(field) {
                Some(serde_json::Value::String(value)) => Ok(value),
                None | Some(serde_json::Value::Null) => Ok(""),
                _ => Err(MediaError::Catalog),
            }
        };
        if text("key")? != key || !matches!(text("type")?, "static" | "video") {
            return Err(MediaError::Input);
        }
        let home_path = home_path(home)?;
        let source = if text("path")?.is_empty() {
            let name = text("name")?;
            if name.is_empty()
                || name.contains('/')
                || name.contains('\\')
                || name.chars().any(char::is_control)
            {
                return Err(MediaError::Scope);
            }
            roots
                .images()
                .asset(RelativePath::new(name).map_err(|_| MediaError::Scope)?)
                .map_err(|_| MediaError::Scope)?
        } else {
            library_asset(&roots, &relative(&home_path, text("path")?)?)?
        };
        let source_home = source.home_relative().map_err(|_| MediaError::Scope)?;
        let mut sources = BTreeSet::from([source]);
        for field in ["thumb", "thumb_sm", "video_file"] {
            let path = text(field)?;
            if path.is_empty() {
                continue;
            }
            let relative = relative(&home_path, path)?;
            if relative == source_home {
                continue;
            }
            sources.insert(cache_asset(&relative)?);
        }
        let commit = CatalogCommit {
            expected_catalog: catalog.identity.sha256.clone(),
            staging_token: token.clone(),
            mutation: CatalogMutation::Remove { key: key.into() },
        };
        let mut paths = vec![commit.staging_asset().map_err(|_| MediaError::Scope)?];
        let mut moves = Vec::new();
        for (index, source) in sources.into_iter().enumerate() {
            let trash = AssetPath::new(
                AssetNamespace::MediaTrash,
                RelativePath::new(format!(".ryoku-media-trash-{}-{index}", token.as_str()))
                    .map_err(|_| MediaError::Scope)?,
            )
            .map_err(|_| MediaError::Scope)?;
            paths.extend([source.clone(), trash.clone()]);
            moves.push(MediaMove { source, trash });
        }
        let scope = AssetScope::new(
            paths,
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
        )
        .map_err(|_| MediaError::Scope)?;
        let inventory =
            scope.capture(home, account).map_err(|_| MediaError::Scope)?.inventory().clone();
        for entry in inventory.entries() {
            if !matches!(entry.kind, EntryKind::Absent | EntryKind::RegularFile { .. })
                || (matches!(
                    entry.path.namespace(),
                    AssetNamespace::MediaTrash | AssetNamespace::CatalogStage
                ) && entry.kind != EntryKind::Absent)
            {
                return Err(MediaError::Scope);
            }
        }
        if read_catalog(home, account)?.bytes != catalog.bytes {
            return Err(MediaError::Changed);
        }
        let parents = trash::parent_bindings(home, account, &moves, &inventory, true)?;
        Ok(Self {
            commit,
            row,
            catalog: catalog.identity,
            original_rows: catalog.rows,
            library_binding: roots.input_digest().into(),
            scope,
            inventory,
            moves,
            parents,
        })
    }
    pub fn scope(&self) -> &AssetScope {
        &self.scope
    }
    pub fn commit(&self) -> &CatalogCommit {
        &self.commit
    }
    pub fn moves(&self) -> &[MediaMove] {
        &self.moves
    }
    pub fn fingerprint(&self) -> Result<String> {
        serde_json::to_vec(&(
            &self.commit,
            &self.row,
            &self.catalog,
            &self.library_binding,
            &self.scope,
            &self.inventory,
            &self.moves,
            &self.parents,
        ))
        .map(|bytes| digest(&bytes))
        .map_err(|_| MediaError::Catalog)
    }
}

#[cfg(test)]
#[path = "ryogami_media_tests.rs"]
mod tests;

#[path = "ryogami_media_trash.rs"]
mod trash;
pub use trash::{MediaTrashEntry, MediaTrashPage, TrashReceiptIdentity, VerifiedTrashReceipt};

impl PreparedMedia {
    pub fn verify_catalog(
        &self,
        home: &File,
        account: &CaptureAccount,
        reply: &[u8],
        id: i64,
    ) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Reply {
            id: i64,
            result: Body,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Body {
            committed: bool,
            key: String,
            action: String,
        }
        let result: Reply = serde_json::from_slice(reply).map_err(|_| MediaError::Catalog)?;
        let mut expected = self.original_rows.clone();
        let action = match &self.commit.mutation {
            CatalogMutation::Remove { key } => {
                expected.remove(key);
                "remove"
            }
            CatalogMutation::Insert { key, entry } => {
                expected.insert(key.clone(), entry.clone());
                "insert"
            }
        };
        if result.id != id
            || !result.result.committed
            || result.result.key != self.commit.mutation.key()
            || result.result.action != action
        {
            return Err(MediaError::Catalog);
        }
        let actual = read_catalog(home, account)?;
        if actual.rows != expected
            || actual.identity.mode != self.catalog.mode
            || actual.identity.uid != self.catalog.uid
            || actual.identity.gid != self.catalog.gid
        {
            return Err(MediaError::Catalog);
        }
        self.verify_moves(home, account)
    }
}

#[path = "ryogami_media_provider.rs"]
mod provider;
pub use provider::{CompletedMedia, change, list};
