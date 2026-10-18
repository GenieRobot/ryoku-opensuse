//! User-session clipboard history provider for the pinned Ryoku Clipboard.qml.
//!
//! The Wayland selection is observed with wl-paste's native watch mode. This
//! process owns the bounded history and the existing Ryoku Unix-socket topic;
//! it never elevates or invokes a shell. Clipboard contents are private user
//! state and are written with mode 0600 below XDG_STATE_HOME.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const MAX_ENTRIES: usize = 100;
const MAX_ENTRY_BYTES: usize = 10 * 1024 * 1024;
const MAX_PREVIEW_CHARS: usize = 200;
const MAX_MIME_TYPES: usize = 64;
const MAX_MIME_BYTES: usize = 256;
const MAX_IMAGE_PIXELS: u64 = 16_777_216;
const MAX_IPC_LINE_BYTES: usize = 4096;
const MAX_CLIENTS: usize = 32;
const IPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: u64,
    kind: String,
    mime: String,
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb_w: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb_h: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumb_bytes: Option<usize>,
    starred: bool,
    hash: u64,
    ts: i64,
    sha256: String,
    #[serde(skip)]
    digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    #[serde(rename = "nextID")]
    next_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_prune: Option<i64>,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredIndex {
    #[serde(rename = "nextID")]
    next_id: u64,
    #[serde(default)]
    last_prune: Option<i64>,
    entries: Vec<StoredEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEntry {
    id: u64,
    mime: String,
    size: usize,
    preview: Option<String>,
    thumb: Option<String>,
    thumb_w: Option<u32>,
    thumb_h: Option<u32>,
    thumb_bytes: Option<usize>,
    starred: bool,
    hash: StoredHash,
    #[serde(default)]
    ts: i64,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredHash {
    Native(String),
    Upstream(u64),
}

struct History {
    state_dir: PathBuf,
    cache_dir: PathBuf,
    next_id: u64,
    last_prune: Option<i64>,
    entries: Vec<Entry>,
}

impl History {
    fn open(state_dir: PathBuf, cache_dir: PathBuf) -> Result<Self> {
        create_private_dir(&state_dir)?;
        create_private_dir(&cache_dir)?;
        let mut history =
            Self { state_dir, cache_dir, next_id: 1, last_prune: None, entries: Vec::new() };
        history.load()?;
        Ok(history)
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn insert(&mut self, mime: &str, data: &[u8]) -> Result<bool> {
        validate_mime(mime)?;
        if data.is_empty() || data.len() > MAX_ENTRY_BYTES {
            return Err("clipboard entry is empty or exceeds 10 MiB".into());
        }
        let digest = format!("{:x}", Sha256::digest(data));
        let hash = fnv1a64(data);
        if let Some(index) =
            self.entries.iter().position(|entry| entry.digest == digest && entry.mime == mime)
        {
            let previous = self.entries.clone();
            let entry = self.entries.remove(index);
            self.entries.insert(0, entry);
            if let Err(error) = self.persist() {
                self.entries = previous;
                return Err(error);
            }
            return Ok(true);
        }

        let id = self.next_id;
        let next_id = self.next_id.checked_add(1).ok_or("clipboard id space exhausted")?;
        let dimensions = image_dimensions(mime, data);
        let kind = if is_text_mime(mime) {
            "text"
        } else if dimensions.is_some() {
            "image"
        } else {
            "binary"
        };
        let preview = (kind == "text").then(|| text_preview(data));
        let thumb = (kind == "image").then(|| image_extension(mime)).flatten().map(|extension| {
            self.cache_dir.join(format!("{digest}.{extension}")).to_string_lossy().into_owned()
        });
        let entry = Entry {
            id,
            kind: kind.to_owned(),
            mime: mime.to_owned(),
            size: data.len(),
            preview,
            thumb,
            thumb_w: dimensions.map(|(width, _)| width),
            thumb_h: dimensions.map(|(_, height)| height),
            thumb_bytes: (kind == "image").then_some(data.len()),
            starred: false,
            hash,
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .min(i64::MAX as u128) as i64,
            sha256: digest.clone(),
            digest: digest.clone(),
        };
        write_private_atomic_new(&self.data_path(hash), data)?;
        if let (Some(path), Some(_)) = (&entry.thumb, image_extension(mime)) {
            write_private_atomic_new(Path::new(path), data)?;
        }
        let previous = self.entries.clone();
        if let Err(error) = self.make_room() {
            let _ = self.remove_unreferenced_data();
            return Err(error);
        }
        self.next_id = next_id;
        self.entries.insert(0, entry);
        if let Err(error) = self.persist() {
            self.entries = previous;
            self.next_id = id;
            return Err(error);
        }
        self.remove_unreferenced_data()?;
        Ok(true)
    }

    fn copy(&mut self, id: u64) -> Result<()> {
        self.copy_with(id, |mime, bytes| {
            let mut child = Command::new("/usr/bin/wl-copy")
                .args(["--type", mime])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            child.stdin.take().ok_or("wl-copy stdin unavailable")?.write_all(bytes)?;
            let status = child.wait()?;
            if !status.success() {
                return Err(format!("wl-copy failed: {status}").into());
            }
            Ok(())
        })
    }

    fn copy_with(
        &mut self,
        id: u64,
        publish: impl FnOnce(&str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let entry =
            self.entries.iter().find(|entry| entry.id == id).ok_or("clipboard entry not found")?;
        let mime = entry.mime.clone();
        let bytes = self.read_entry_data(entry)?;
        publish(&mime, &bytes)?;
        let previous = self.entries.clone();
        let entry = self.entries.remove(
            self.entries
                .iter()
                .position(|entry| entry.id == id)
                .ok_or("clipboard entry not found")?,
        );
        self.entries.insert(0, entry);
        if let Err(error) = self.persist() {
            self.entries = previous;
            return Err(error);
        }
        Ok(())
    }

    fn read_entry_data(&self, entry: &Entry) -> Result<Vec<u8>> {
        let bytes = read_private_file(&self.data_path(entry.hash), MAX_ENTRY_BYTES)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if bytes.len() != entry.size || digest != entry.digest {
            return Err("stored clipboard entry failed its integrity check".into());
        }
        Ok(bytes)
    }

    fn delete(&mut self, id: u64) -> Result<()> {
        let previous = self.entries.clone();
        let old_len = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        if self.entries.len() == old_len {
            return Err("clipboard entry not found".into());
        }
        if let Err(error) = self.persist() {
            self.entries = previous;
            return Err(error);
        }
        self.remove_unreferenced_data()
    }

    fn clear(&mut self) -> Result<()> {
        let previous = self.entries.clone();
        self.entries.retain(|entry| entry.starred);
        if let Err(error) = self.persist() {
            self.entries = previous;
            return Err(error);
        }
        self.remove_unreferenced_data()
    }

    fn star(&mut self, id: u64, starred: bool) -> Result<()> {
        let previous = self.entries.clone();
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or("clipboard entry not found")?;
        entry.starred = starred;
        if let Err(error) = self.persist() {
            self.entries = previous;
            return Err(error);
        }
        Ok(())
    }

    fn make_room(&mut self) -> Result<()> {
        if self.entries.len() < MAX_ENTRIES {
            return Ok(());
        }
        let Some(index) = self.entries.iter().rposition(|entry| !entry.starred) else {
            return Err("clipboard history is full of starred entries".into());
        };
        self.entries.remove(index);
        Ok(())
    }

    fn data_path(&self, hash: u64) -> PathBuf {
        self.state_dir.join("data").join(format!("{hash:016x}.bin"))
    }

    fn load(&mut self) -> Result<()> {
        let data_dir = self.state_dir.join("data");
        create_private_dir(&data_dir)?;
        let path = self.state_dir.join("index.json");
        let raw = match read_private_file(&path, 1024 * 1024) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let index: StoredIndex = serde_json::from_slice(&raw)?;
        if index.next_id == 0 {
            return Err("clipboard index contains an invalid next ID".into());
        }
        if index.entries.len() > MAX_ENTRIES {
            return Err("clipboard index exceeds entry limit".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for stored in index.entries {
            validate_mime(&stored.mime)?;
            if stored.id == 0
                || !ids.insert(stored.id)
                || stored.size == 0
                || stored.size > MAX_ENTRY_BYTES
            {
                return Err("clipboard index contains an invalid entry".into());
            }
            let (old_path, upstream_hash) = match &stored.hash {
                StoredHash::Native(digest)
                    if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    (self.state_dir.join("data").join(format!("{digest}.bin")), None)
                }
                StoredHash::Upstream(hash) => (self.data_path(*hash), Some(*hash)),
                _ => return Err("clipboard index contains an invalid content hash".into()),
            };
            let data = read_private_file(&old_path, MAX_ENTRY_BYTES)?;
            let digest = format!("{:x}", Sha256::digest(data.as_slice()));
            let hash = upstream_hash.unwrap_or_else(|| fnv1a64(&data));
            let hash_matches = upstream_hash.is_none_or(|expected| fnv1a64(&data) == expected)
                && stored.sha256.as_deref().is_none_or(|expected| digest == expected)
                && match &stored.hash {
                    StoredHash::Native(expected) => digest == *expected,
                    StoredHash::Upstream(_) => true,
                };
            if data.len() != stored.size || !hash_matches {
                return Err("clipboard data failed integrity verification".into());
            }
            if old_path != self.data_path(hash) {
                write_private_atomic_new(&self.data_path(hash), &data)?;
            }
            let thumb = stored.thumb.filter(|path| {
                let path = Path::new(path);
                if !path.is_absolute() || path.parent() != Some(self.cache_dir.as_path()) {
                    return false;
                }
                read_private_metadata(path).is_ok()
            });
            let dimensions = image_dimensions(&stored.mime, &data);
            let kind = if is_text_mime(&stored.mime) {
                "text"
            } else if thumb.is_some() && dimensions.is_some() {
                "image"
            } else {
                "binary"
            };
            let preview = (kind == "text").then(|| text_preview(&data));
            self.entries.push(Entry {
                id: stored.id,
                kind: kind.to_owned(),
                mime: stored.mime,
                size: stored.size,
                preview: preview.or(stored.preview),
                thumb,
                thumb_w: stored.thumb_w.or(dimensions.map(|(width, _)| width)),
                thumb_h: stored.thumb_h.or(dimensions.map(|(_, height)| height)),
                thumb_bytes: stored.thumb_bytes,
                starred: stored.starred,
                hash,
                ts: stored.ts,
                sha256: digest.clone(),
                digest,
            });
        }
        self.next_id = index.next_id.max(ids.last().copied().unwrap_or(0).saturating_add(1));
        self.last_prune = index.last_prune;
        self.persist()?;
        self.remove_unreferenced_data()?;
        Ok(())
    }

    fn persist(&self) -> Result<()> {
        let index = Index {
            next_id: self.next_id,
            last_prune: self.last_prune,
            entries: self.entries.clone(),
        };
        write_private_atomic(&self.state_dir.join("index.json"), &serde_json::to_vec(&index)?)
    }

    fn remove_unreferenced_data(&self) -> Result<()> {
        let retained: std::collections::BTreeSet<_> =
            self.entries.iter().map(|entry| format!("{:016x}", entry.hash)).collect();
        for entry in fs::read_dir(self.state_dir.join("data"))? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(digest) = managed_data_digest(&name) else { continue };
            if !retained.contains(&digest) && private_single_link_regular(&entry.path())? {
                fs::remove_file(entry.path())?;
            }
        }
        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some((stem, _extension)) = managed_thumbnail_name(&path) else { continue };
            let referenced_thumb = self
                .entries
                .iter()
                .any(|entry| entry.thumb.as_deref() == Some(path.to_string_lossy().as_ref()));
            if !retained.contains(stem) && !referenced_thumb && private_single_link_regular(&path)?
            {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

fn validate_mime(mime: &str) -> Result<()> {
    if mime.is_empty()
        || mime.len() > MAX_MIME_BYTES
        || !mime.is_ascii()
        || mime.starts_with('-')
        || mime.bytes().any(|byte| byte.is_ascii_control() || byte == b'\n' || byte == b'\r')
    {
        return Err("invalid clipboard MIME type".into());
    }
    Ok(())
}

fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/") || mime.eq_ignore_ascii_case("UTF8_STRING")
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn managed_data_digest(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    let digest = name.strip_suffix(".bin")?;
    (is_lower_hex(digest, 64) || is_lower_hex(digest, 16)).then(|| digest.to_owned())
}

fn managed_thumbnail_name(path: &Path) -> Option<(&str, &str)> {
    let stem = path.file_stem()?.to_str()?;
    let extension = path.extension()?.to_str()?;
    (is_lower_hex(stem, 64) || is_lower_hex(stem, 16))
        .then_some((stem, extension))
        .filter(|(_, extension)| matches!(*extension, "png" | "jpg" | "gif"))
}

fn private_single_link_regular(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o7777 == 0o600
                && metadata.nlink() == 1)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn image_extension(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}

fn image_dimensions(mime: &str, data: &[u8]) -> Option<(u32, u32)> {
    let dimensions = match mime {
        "image/png" if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") => (
            u32::from_be_bytes(data[16..20].try_into().ok()?),
            u32::from_be_bytes(data[20..24].try_into().ok()?),
        ),
        "image/gif"
            if data.len() >= 10 && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) =>
        {
            (
                u16::from_le_bytes(data[6..8].try_into().ok()?) as u32,
                u16::from_le_bytes(data[8..10].try_into().ok()?) as u32,
            )
        }
        "image/jpeg" => jpeg_dimensions(data)?,
        _ => return None,
    };
    let pixels = u64::from(dimensions.0).checked_mul(u64::from(dimensions.1))?;
    (dimensions.0 > 0 && dimensions.1 > 0 && pixels <= MAX_IMAGE_PIXELS).then_some(dimensions)
}

fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if !data.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut offset = 2;
    while offset + 4 <= data.len() {
        if data[offset] != 0xff {
            offset += 1;
            continue;
        }
        while offset < data.len() && data[offset] == 0xff {
            offset += 1;
        }
        let marker = *data.get(offset)?;
        offset += 1;
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length =
            usize::from(u16::from_be_bytes(data.get(offset..offset + 2)?.try_into().ok()?));
        if length < 2 || offset.checked_add(length)? > data.len() {
            return None;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            let height =
                u16::from_be_bytes(data.get(offset + 3..offset + 5)?.try_into().ok()?) as u32;
            let width =
                u16::from_be_bytes(data.get(offset + 5..offset + 7)?.try_into().ok()?) as u32;
            return Some((width, height));
        }
        offset += length;
    }
    None
}

fn text_preview(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .take(MAX_PREVIEW_CHARS)
        .collect()
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err("clipboard storage must be a user-owned real directory".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn read_private_metadata(path: &Path) -> std::io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "clipboard file is not a private regular file",
        ));
    }
    Ok(metadata)
}

fn read_private_file(path: &Path, maximum: usize) -> std::io::Result<Vec<u8>> {
    use rustix::fs::{FileType, OFlags, fstat, open};

    let fd = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let file = File::from(fd);
    let metadata = fstat(&file).map_err(std::io::Error::from)?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile
        || metadata.st_uid != rustix::process::geteuid().as_raw()
        || metadata.st_mode & 0o077 != 0
        || metadata.st_nlink != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "clipboard file is not a private single-linked regular file",
        ));
    }
    if metadata.st_size < 0 || metadata.st_size as u64 > maximum as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "clipboard file exceeds its size limit",
        ));
    }
    let mut contents = Vec::with_capacity(metadata.st_size as usize);
    file.take(maximum as u64 + 1).read_to_end(&mut contents)?;
    if contents.len() > maximum {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "clipboard file exceeds its size limit",
        ));
    }
    Ok(contents)
}

fn write_private_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("clipboard path has no parent")?;
    create_private_dir(parent)?;
    let file_name = path.file_name().ok_or("clipboard path has no filename")?.to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

/// Publish content-addressed data without replacing an existing path. If the
/// exact blob is already present it is reused; a collision, symlink or other
/// pre-existing object is preserved and reported as an error.
fn write_private_atomic_new(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("clipboard path has no parent")?;
    create_private_dir(parent)?;
    let file_name = path.file_name().ok_or("clipboard path has no filename")?.to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file =
            OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&temporary, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if read_private_file(path, contents.len())? != contents {
                    return Err("content-addressed clipboard path has different data".into());
                }
            }
            Err(error) => return Err(error.into()),
        }
        fs::remove_file(&temporary)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn state_path() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or("HOME is missing")?;
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".local/state"));
    if !base.is_absolute() {
        return Err("XDG_STATE_HOME must be absolute".into());
    }
    Ok(base.join("ryoku/clipboard"))
}

fn cache_path() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or("HOME is missing")?;
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".cache"));
    if !base.is_absolute() {
        return Err("XDG_CACHE_HOME must be absolute".into());
    }
    Ok(base.join("ryoku/clipboard"))
}

fn socket_path() -> Result<PathBuf> {
    Ok(PathBuf::from(env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is missing")?)
        .join("ryoku-clipboard.sock"))
}

fn prepare_socket(path: &Path) -> Result<UnixListener> {
    let parent = path.parent().ok_or("socket has no parent")?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err("XDG_RUNTIME_DIR must be an owner-only directory owned by this user".into());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_socket()
                && metadata.uid() == rustix::process::geteuid().as_raw() =>
        {
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err("another Ryoku clipboard provider already owns the socket".into());
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) =>
                {
                    match fs::remove_file(path) {
                        Ok(()) => {}
                        Err(remove_error)
                            if remove_error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(remove_error) => return Err(remove_error.into()),
                    }
                }
                Err(error) => {
                    return Err(format!("cannot verify existing clipboard socket: {error}").into());
                }
            }
        }
        Ok(_) => return Err("refusing to replace a non-socket clipboard path".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

type Shared = Arc<(Mutex<History>, Mutex<Vec<mpsc::SyncSender<String>>>)>;

struct ClientSlot(Arc<AtomicUsize>);

impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn frame(history: &History) -> Result<String> {
    Ok(format!(
        "{}\n",
        serde_json::to_string(&serde_json::json!({ "entries": history.entries() }))?
    ))
}

fn publish(shared: &Shared) -> Result<()> {
    let snapshot = {
        let (history, _) = &**shared;
        let guard = history.lock().map_err(|_| "clipboard history lock poisoned")?;
        frame(&guard)?
    };
    let (_, subscribers) = &**shared;
    subscribers.lock().map_err(|_| "clipboard subscriber lock poisoned")?.retain(|subscriber| {
        match subscriber.try_send(snapshot.clone()) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) | Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    });
    Ok(())
}

fn subscribe(mut stream: UnixStream, shared: Shared) -> Result<()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let initial = {
        let (history, subscribers) = &*shared;
        let guard = history.lock().map_err(|_| "clipboard history lock poisoned")?;
        let initial = frame(&guard)?;
        subscribers.lock().map_err(|_| "clipboard subscriber lock poisoned")?.push(sender);
        drop(guard);
        initial
    };
    stream.write_all(initial.as_bytes())?;
    for update in receiver {
        stream.write_all(update.as_bytes())?;
    }
    Ok(())
}

fn handle_call(line: &str, shared: &Shared) -> Result<bool> {
    if line == "capture" {
        let mut history = shared.0.lock().map_err(|_| "clipboard history lock poisoned")?;
        let (mime, bytes) = read_selection()?;
        history.insert(&mime, &bytes)?;
        drop(history);
        publish(shared)?;
        return Ok(true);
    }
    let Some(rest) = line.strip_prefix("call ") else {
        return Err("invalid clipboard request".into());
    };
    let split = rest.find(' ').ok_or("clipboard call is missing arguments")?;
    let method = &rest[..split];
    let args: serde_json::Value = serde_json::from_str(rest[split + 1..].trim())?;
    let mut history = shared.0.lock().map_err(|_| "clipboard history lock poisoned")?;
    match method {
        "clipboard.copy" => history.copy(
            args.get("entry").and_then(serde_json::Value::as_u64).ok_or("invalid entry id")?,
        )?,
        "clipboard.delete" => history.delete(
            args.get("entry").and_then(serde_json::Value::as_u64).ok_or("invalid entry id")?,
        )?,
        "clipboard.clear" => history.clear()?,
        "clipboard.star" => history.star(
            args.get("entry").and_then(serde_json::Value::as_u64).ok_or("invalid entry id")?,
            args.get("starred")
                .and_then(serde_json::Value::as_bool)
                .ok_or("invalid starred value")?,
        )?,
        _ => return Err("unsupported clipboard operation".into()),
    }
    drop(history);
    publish(shared)?;
    Ok(true)
}

fn client(mut stream: UnixStream, shared: Shared) -> Result<()> {
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut line = String::new();
        if reader.by_ref().take((MAX_IPC_LINE_BYTES + 1) as u64).read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line.len() > MAX_IPC_LINE_BYTES {
            return Err("clipboard request is too long".into());
        }
        if line.trim_end() == "subscribe clipboard" {
            return subscribe(stream, shared);
        }
        let response = handle_call(line.trim_end(), &shared)
            .map(|_| "{\"ok\":true}\n".to_owned())
            .unwrap_or_else(|error| {
                serde_json::json!({ "ok": false, "error": error.to_string() }).to_string() + "\n"
            });
        stream.write_all(response.as_bytes())?;
    }
}

fn call_server(path: &Path, request: &str) -> Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut response = String::new();
    BufReader::new(stream).take((MAX_IPC_LINE_BYTES + 1) as u64).read_line(&mut response)?;
    if response.len() > MAX_IPC_LINE_BYTES {
        return Err("clipboard provider response exceeds its size limit".into());
    }
    validate_server_response(&response)
}

fn validate_server_response(response: &str) -> Result<()> {
    if response.len() > MAX_IPC_LINE_BYTES {
        return Err("clipboard provider response exceeds its size limit".into());
    }
    let response: serde_json::Value = serde_json::from_str(response)?;
    if response.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(response
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("clipboard request failed")
            .to_owned()
            .into())
    }
}

fn read_selection() -> Result<(String, Vec<u8>)> {
    let mut type_child = Command::new("/usr/bin/wl-paste")
        .arg("--list-types")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut type_output = Vec::new();
    type_child
        .stdout
        .take()
        .ok_or("wl-paste MIME list stdout unavailable")?
        .take((MAX_MIME_TYPES * MAX_MIME_BYTES + 1) as u64)
        .read_to_end(&mut type_output)?;
    if type_output.len() > MAX_MIME_TYPES * MAX_MIME_BYTES {
        let _ = type_child.kill();
        let _ = type_child.wait();
        return Err("clipboard MIME list exceeds its size limit".into());
    }
    if !type_child.wait()?.success() {
        return Err("wl-paste could not read the Wayland selection".into());
    }
    let types = String::from_utf8(type_output)?;
    let mut mimes: Vec<_> = types
        .lines()
        .map(str::trim)
        .filter(|mime| !mime.is_empty() && validate_mime(mime).is_ok())
        .take(MAX_MIME_TYPES)
        .collect();
    let priority = [
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
        "text/plain;charset=utf-8",
        "text/plain",
        "UTF8_STRING",
        "text/html",
    ];
    mimes.sort_by_key(|mime| {
        priority
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(mime))
            .unwrap_or(priority.len())
    });
    for mime in mimes {
        let mut child = Command::new("/usr/bin/wl-paste")
            .args(["--no-newline", "--type", mime])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut bytes = Vec::with_capacity(16 * 1024);
        child
            .stdout
            .take()
            .ok_or("wl-paste stdout unavailable")?
            .take((MAX_ENTRY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_ENTRY_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }
        let status = child.wait()?;
        if status.success() && !bytes.is_empty() {
            return Ok((mime.to_owned(), bytes));
        }
    }
    Err("Wayland selection has no supported non-empty MIME payload under 10 MiB".into())
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("notify") => call_server(&socket_path()?, "capture"),
        Some("serve") => {
            if args.next().is_some() {
                return Err("unexpected clipboard service argument".into());
            }
            if env::var_os("WAYLAND_DISPLAY").is_none_or(|value| value.is_empty()) {
                return Err("Wayland session is not available".into());
            }
            let path = socket_path()?;
            let listener = prepare_socket(&path)?;
            listener.set_nonblocking(true)?;
            let history = History::open(state_path()?, cache_path()?)?;
            let shared: Shared = Arc::new((Mutex::new(history), Mutex::new(Vec::new())));
            let active_clients = Arc::new(AtomicUsize::new(0));
            let executable = env::current_exe()?;
            let mut watcher = Command::new("/usr/bin/wl-paste")
                .args(["--watch"])
                .arg(executable)
                .arg("notify")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()?;
            let capture_shared = shared.clone();
            thread::spawn(move || {
                if let Err(error) = handle_call("capture", &capture_shared) {
                    eprintln!("initial clipboard capture failed: {error}");
                }
            });
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if active_clients
                            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                                (active < MAX_CLIENTS).then_some(active + 1)
                            })
                            .is_err()
                        {
                            drop(stream);
                            continue;
                        }
                        let active = active_clients.clone();
                        let shared = shared.clone();
                        if thread::Builder::new()
                            .spawn(move || {
                                let _slot = ClientSlot(active);
                                if let Err(error) = client(stream, shared) {
                                    eprintln!("clipboard request failed: {error}");
                                }
                            })
                            .is_err()
                        {
                            active_clients.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(error) => return Err(error.into()),
                }
                if watcher.try_wait()?.is_some() {
                    break;
                }
            }
            let _ = watcher.kill();
            let _ = fs::remove_file(path);
            Err("wl-paste clipboard watcher exited".into())
        }
        _ => Err("usage: ryoku-clipboard serve|notify".into()),
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ryoku-clipboard: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn history() -> (History, PathBuf) {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = env::temp_dir().join(format!(
            "ryoku-clipboard-{}-{nonce}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let state = root.join("state");
        let cache = root.join("cache");
        (History::open(state, cache).unwrap(), root)
    }

    #[test]
    fn text_is_previewed_deduplicated_and_persisted_privately() {
        let (mut history, root) = history();
        let text = "Ryoku clipboard proof".repeat(16);
        history.insert("text/plain;charset=utf-8", text.as_bytes()).unwrap();
        assert_eq!(history.entries()[0].kind, "text");
        assert_eq!(
            history.entries()[0].preview.as_deref().unwrap().chars().count(),
            MAX_PREVIEW_CHARS
        );
        let first_id = history.entries()[0].id;
        history.insert("text/plain;charset=utf-8", text.as_bytes()).unwrap();
        assert_eq!(history.entries().len(), 1);
        assert_eq!(history.entries()[0].id, first_id);

        let data = fs::read_dir(root.join("state/data")).unwrap().next().unwrap().unwrap();
        assert_eq!(data.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(history);
        let history = History::open(root.join("state"), root.join("cache")).unwrap();
        assert_eq!(history.entries()[0].id, first_id);
        assert_eq!(
            history.entries()[0].preview.as_deref().unwrap().chars().count(),
            MAX_PREVIEW_CHARS
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn starred_items_survive_clear_and_protect_the_history_limit() {
        let (mut history, root) = history();
        history.insert("text/plain", b"keep").unwrap();
        let starred = history.entries()[0].id;
        history.star(starred, true).unwrap();
        history.insert("text/plain", b"discard").unwrap();
        history.clear().unwrap();
        assert_eq!(history.entries().len(), 1);
        assert_eq!(history.entries()[0].id, starred);
        for index in 0..(MAX_ENTRIES - 1) {
            history.insert("text/plain", format!("entry-{index}").as_bytes()).unwrap();
        }
        assert_eq!(history.entries().len(), MAX_ENTRIES);
        history.insert("text/plain", b"overflow").unwrap();
        assert_eq!(history.entries().len(), MAX_ENTRIES);
        assert!(history.entries().iter().any(|entry| entry.id == starred && entry.starred));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_oversize_and_malformed_mime_without_changing_history() {
        let (mut history, root) = history();
        assert!(history.insert("text/plain", &vec![b'x'; MAX_ENTRY_BYTES + 1]).is_err());
        assert!(history.insert("text/plain\ncall clipboard.clear {}", b"x").is_err());
        assert!(history.insert("--help", b"x").is_err());
        assert!(history.entries().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_bytes_with_distinct_mimes_remain_distinct_and_utf8_string_is_text() {
        let (mut history, root) = history();
        history.insert("text/plain", b"same payload").unwrap();
        history.insert("text/html", b"same payload").unwrap();
        assert_eq!(history.entries().len(), 2);
        assert_eq!(history.entries()[0].mime, "text/html");
        assert_eq!(history.entries()[1].mime, "text/plain");
        history.insert("UTF8_STRING", b"native text selection").unwrap();
        assert_eq!(history.entries()[0].kind, "text");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_acknowledgements_are_bounded_and_require_success() {
        assert!(validate_server_response("{\"ok\":true}\n").is_ok());
        assert!(validate_server_response("{\"ok\":false,\"error\":\"failed\"}\n").is_err());
        assert!(validate_server_response(&"x".repeat(MAX_IPC_LINE_BYTES + 1)).is_err());
        assert!(validate_server_response("{}\n").is_err());
    }

    #[test]
    fn zero_next_id_is_rejected_without_rewriting_the_index() {
        let (_history, root) = history();
        let state = root.join("state");
        let index_path = state.join("index.json");
        let malformed = br#"{"nextID":0,"entries":[]}"#;
        write_private_atomic(&index_path, malformed).unwrap();

        assert!(History::open(state, root.join("cache")).is_err());
        assert_eq!(fs::read(&index_path).unwrap(), malformed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_data_read_rejects_replaced_bytes_and_symlinks() {
        let (mut history, root) = history();
        history.insert("text/plain", b"original clipboard data").unwrap();
        let entry = history.entries()[0].clone();
        assert_eq!(history.read_entry_data(&entry).unwrap(), b"original clipboard data");

        let path = history.data_path(entry.hash);
        fs::write(&path, b"altered clipboard data").unwrap();
        assert!(history.read_entry_data(&entry).is_err());

        fs::remove_file(&path).unwrap();
        let outside = root.join("outside");
        fs::write(&outside, b"original clipboard data").unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(history.read_entry_data(&entry).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_back_publishes_exact_payload_before_persisting_history_order() {
        let (mut history, root) = history();
        history.insert("text/plain", b"older entry").unwrap();
        let older_id = history.entries()[0].id;
        history.insert("text/html", b"<b>newer entry</b>").unwrap();
        let newer_id = history.entries()[0].id;

        let mut published = None;
        history
            .copy_with(older_id, |mime, payload| {
                published = Some((mime.to_owned(), payload.to_vec()));
                Ok(())
            })
            .unwrap();
        assert_eq!(published, Some(("text/plain".into(), b"older entry".to_vec())));
        assert_eq!(
            history.entries().iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [older_id, newer_id]
        );

        let before_failure = history.entries().iter().map(|entry| entry.id).collect::<Vec<_>>();
        assert!(
            history.copy_with(newer_id, |_, _| Err("simulated wl-copy failure".into())).is_err()
        );
        assert_eq!(
            history.entries().iter().map(|entry| entry.id).collect::<Vec<_>>(),
            before_failure
        );
        drop(history);
        let reopened = History::open(root.join("state"), root.join("cache")).unwrap();
        assert_eq!(
            reopened.entries().iter().map(|entry| entry.id).collect::<Vec<_>>(),
            before_failure
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clear_preserves_only_starred_data_and_delete_removes_the_rest() {
        let (mut history, root) = history();
        history.insert("text/plain", b"starred").unwrap();
        let starred = history.entries()[0].id;
        history.star(starred, true).unwrap();
        history.insert("text/plain", b"ordinary").unwrap();
        history.clear().unwrap();
        assert_eq!(fs::read_dir(root.join("state/data")).unwrap().count(), 1);
        history.delete(starred).unwrap();
        assert!(history.entries().is_empty());
        assert_eq!(fs::read_dir(root.join("state/data")).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn content_addressed_publication_never_replaces_existing_data() {
        let (history, root) = history();
        let path = root.join("state/data/existing.bin");
        write_private_atomic(&path, b"preserve this").unwrap();

        assert!(write_private_atomic_new(&path, b"different bytes").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"preserve this");
        write_private_atomic_new(&path, b"preserve this").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"preserve this");
        drop(history);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_preserves_unrecognized_private_files() {
        let (mut history, root) = history();
        let unrelated_blob = root.join("state/data/notes.bin");
        let unrelated_cache = root.join(format!("cache/{}.json", "a".repeat(64)));
        write_private_atomic(&unrelated_blob, b"not a clipboard blob").unwrap();
        write_private_atomic(&unrelated_cache, b"not an image thumbnail").unwrap();

        history.insert("text/plain", b"managed entry").unwrap();
        history.delete(history.entries()[0].id).unwrap();

        assert_eq!(fs::read(&unrelated_blob).unwrap(), b"not a clipboard blob");
        assert_eq!(fs::read(&unrelated_cache).unwrap(), b"not an image thumbnail");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imports_the_pinned_upstream_numeric_hash_index_without_losing_history() {
        let (_history, root) = history();
        let state = root.join("state");
        let cache = root.join("cache");
        let data_dir = state.join("data");
        create_private_dir(&data_dir).unwrap();
        let contents = b"history from the upstream clipboard backend";
        let old_hash = fnv1a64(contents);
        write_private_atomic(&data_dir.join(format!("{old_hash:016x}.bin")), contents).unwrap();
        let fixture = serde_json::json!({
            "nextID": 8,
            "lastPrune": 0,
            "entries": [{
                "id": 7,
                "kind": "text",
                "mime": "text/plain;charset=utf-8",
                "size": contents.len(),
                "preview": "old preview",
                "starred": true,
                "hash": old_hash,
                "ts": 1
            }]
        });
        write_private_atomic(&state.join("index.json"), &serde_json::to_vec(&fixture).unwrap())
            .unwrap();

        let history = History::open(state.clone(), cache).unwrap();
        assert_eq!(history.entries()[0].id, 7);
        assert!(history.entries()[0].starred);
        assert_eq!(
            history.entries()[0].preview.as_deref(),
            Some("history from the upstream clipboard backend")
        );
        let migrated_index: serde_json::Value =
            serde_json::from_slice(&fs::read(state.join("index.json")).unwrap()).unwrap();
        assert_eq!(migrated_index["nextID"], 8);
        assert_eq!(migrated_index["lastPrune"], 0);
        assert_eq!(migrated_index["entries"][0]["hash"], old_hash);
        assert_eq!(migrated_index["entries"][0]["ts"], 1);
        assert_eq!(
            migrated_index["entries"][0]["sha256"],
            format!("{:x}", Sha256::digest(contents))
        );
        assert!(state.join(format!("data/{old_hash:016x}.bin")).exists());
        assert_eq!(fs::read_dir(state.join("data")).unwrap().count(), 1);
        drop(history);
        let reopened = History::open(state.clone(), root.join("cache")).unwrap();
        assert_eq!(reopened.entries()[0].id, 7);
        assert_eq!(reopened.entries()[0].hash, old_hash);
        assert_eq!(reopened.entries()[0].ts, 1);
        assert_eq!(reopened.read_entry_data(&reopened.entries()[0]).unwrap(), contents);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_previews_require_a_recognized_bounded_dimensions_header() {
        let mut png = vec![0; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[16..20].copy_from_slice(&512_u32.to_be_bytes());
        png[20..24].copy_from_slice(&256_u32.to_be_bytes());
        assert_eq!(image_dimensions("image/png", &png), Some((512, 256)));
        png[16..20].copy_from_slice(&20_000_u32.to_be_bytes());
        png[20..24].copy_from_slice(&20_000_u32.to_be_bytes());
        assert_eq!(image_dimensions("image/png", &png), None);
        assert_eq!(image_dimensions("image/webp", b"RIFF"), None);
    }
}
