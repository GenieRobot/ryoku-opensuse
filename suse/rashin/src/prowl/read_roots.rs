//! Explicit owner grants for read_file/list_dir. Paths never confer authority:
//! each grant binds a directory inode and each job holds its verified descriptor.
use crate::{Error, Result};
use rustix::fs::{self, FileType, Mode, OFlags, ResolveFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
const NO_LINKS: ResolveFlags =
    ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);
const WALK: ResolveFlags = NO_LINKS.union(ResolveFlags::NO_XDEV);
const MAX_FILE: usize = 32 * 1024;
pub(crate) type Identity = (u64, u64);
fn identity(file: &File) -> Result<Identity> {
    let s = fs::fstat(file).map_err(|_| Error::Storage)?;
    Ok((s.st_dev, s.st_ino))
}
fn secret(path: &str) -> bool {
    let low = path.to_lowercase();
    [
        ".ssh",
        ".gnupg",
        ".password-store",
        "keyrings",
        "id_rsa",
        "id_ed25519",
        ".env",
        ".netrc",
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
        ".hermes",
        "environ",
        "token",
        "secret",
        "api-key",
        "apikey",
        "password",
        ".config/prowl-agent",
        "gateway/token",
        "logins.json",
        "key4.db",
        "cookies.sqlite",
        ".docker/config.json",
        ".kube/config",
        "gh/hosts.yml",
        ".aws/credentials",
        "credentials",
    ]
    .iter()
    .any(|m| low.contains(m))
        || low
            .split(['/', '\\', ' ', '\t', '\n', ':', ','])
            .any(|p| p.ends_with(".pem") || p.ends_with(".key"))
}
fn absolute(path: &Path) -> Result<&str> {
    let value = path.to_str().ok_or(Error::InvalidRequest)?;
    if !path.is_absolute()
        || value.len() > 4096
        || value.contains('\0')
        || value.split('/').any(|p| p == ".." || p == ".")
    {
        return Err(Error::InvalidRequest);
    }
    if secret(value) {
        return Err(Error::Unauthorized);
    }
    Ok(value)
}
fn directory(path: &Path) -> Result<File> {
    absolute(path)?;
    let root = File::open("/").map_err(|_| Error::Storage)?;
    let directory = File::from(
        fs::openat2(
            &root,
            path.strip_prefix("/").map_err(|_| Error::InvalidRequest)?,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            NO_LINKS,
        )
        .map_err(|_| Error::UnsafeState)?,
    );
    let s = fs::fstat(&directory).map_err(|_| Error::Storage)?;
    if s.st_uid != rustix::process::geteuid().as_raw() || s.st_mode & 0o022 != 0 {
        return Err(Error::UnsafeState);
    }
    Ok(directory)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Enrollment {
    pub id: String,
    pub path: PathBuf,
    device: u64,
    inode: u64,
}
impl Enrollment {
    pub fn enroll(id: String, path: PathBuf, protected: Identity) -> Result<Self> {
        crate::identifier(&id)?;
        if secret(&id) {
            return Err(Error::InvalidRequest);
        }
        let root = directory(&path)?;
        let (device, inode) = identity(&root)?;
        if (device, inode) == protected {
            return Err(Error::Unauthorized);
        }
        Ok(Self { id, path, device, inode })
    }
    pub fn capture(&self, protected: Identity) -> Result<Captured> {
        crate::identifier(&self.id)?;
        let root = directory(&self.path)?;
        if identity(&root)? != (self.device, self.inode) {
            return Err(Error::Integrity);
        }
        if identity(&root)? == protected {
            return Err(Error::Unauthorized);
        }
        Ok(Captured { grant: self.clone(), root, protected })
    }
    pub fn describe(&self) -> Value {
        json!({"id":self.id,"path":self.path})
    }
}
pub(crate) struct Captured {
    grant: Enrollment,
    root: File,
    protected: Identity,
}
impl Captured {
    pub fn describe(&self) -> Value {
        self.grant.describe()
    }
    fn relative(&self, path: &str) -> Result<PathBuf> {
        if path.len() > 4096 || path.contains('\0') || secret(path) {
            return Err(Error::Unauthorized);
        }
        let relative = if let Some(value) = path.strip_prefix(&format!("{}:", self.grant.id)) {
            PathBuf::from(value.trim_start_matches('/'))
        } else {
            Path::new(path).strip_prefix(&self.grant.path).map_err(|_| Error::NotFound)?.to_owned()
        };
        if relative.is_absolute()
            || relative.to_str().is_none_or(|p| p.split('/').any(|v| v == ".." || v == "."))
        {
            return Err(Error::Unauthorized);
        }
        Ok(relative)
    }
    fn open(&self, relative: &Path) -> Result<File> {
        if secret(self.grant.path.join(relative).to_str().ok_or(Error::InvalidRequest)?) {
            return Err(Error::Unauthorized);
        }
        let mut file = self.root.try_clone().map_err(|_| Error::Storage)?;
        if identity(&file)? == self.protected {
            return Err(Error::Unauthorized);
        }
        let parts = relative.components().collect::<Vec<_>>();
        if parts.len() > 64 {
            return Err(Error::Limit);
        }
        for (i, part) in parts.iter().enumerate() {
            let std::path::Component::Normal(name) = part else {
                return Err(Error::Unauthorized);
            };
            let flags = OFlags::PATH
                | OFlags::CLOEXEC
                | if i + 1 < parts.len() { OFlags::DIRECTORY } else { OFlags::empty() };
            file = File::from(
                fs::openat2(&file, *name, flags, Mode::empty(), WALK)
                    .map_err(|_| Error::UnsafeState)?,
            );
            let s = fs::fstat(&file).map_err(|_| Error::Storage)?;
            if (s.st_dev, s.st_ino) == self.protected {
                return Err(Error::Unauthorized);
            }
            if !matches!(
                FileType::from_raw_mode(s.st_mode),
                FileType::Directory | FileType::RegularFile
            ) || (FileType::from_raw_mode(s.st_mode) == FileType::RegularFile && s.st_nlink != 1)
            {
                return Err(Error::UnsafeState);
            }
            if ![rustix::process::geteuid().as_raw(), 0].contains(&s.st_uid)
                || s.st_mode & 0o022 != 0
            {
                return Err(Error::UnsafeState);
            }
        }
        // Open data only after O_PATH proved a regular file/directory. Reopen
        // this held kernel descriptor, never a caller-selected magic link.
        // This prevents a racing FIFO/device from being opened for data.
        let proc = File::open("/proc/self/fd").map_err(|_| Error::Storage)?;
        if fs::fstatfs(&proc).map_err(|_| Error::Storage)?.f_type != fs::PROC_SUPER_MAGIC {
            return Err(Error::UnsafeState);
        }
        let opened = File::from(
            fs::openat2(
                &proc,
                file.as_raw_fd().to_string(),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                ResolveFlags::empty(),
            )
            .map_err(|_| Error::UnsafeState)?,
        );
        if identity(&opened)? != identity(&file)? {
            return Err(Error::Integrity);
        }
        Ok(opened)
    }
    fn read(&self, path: &str, cancelled: &AtomicBool, deadline: Instant) -> Result<Value> {
        let relative = self.relative(path)?;
        let mut file = self.open(&relative)?;
        let before = fs::fstat(&file).map_err(|_| Error::Storage)?;
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile || before.st_nlink != 1
        {
            return Err(Error::UnsafeState);
        }
        check(cancelled, deadline)?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_FILE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        check(cancelled, deadline)?;
        let after = fs::fstat(&file).map_err(|_| Error::Storage)?;
        let current = fs::fstat(&self.open(&relative)?).map_err(|_| Error::Storage)?;
        if !same(&before, &after) || !same(&after, &current) {
            return Err(Error::Integrity);
        }
        if bytes.contains(&0) {
            return Err(Error::InvalidRequest);
        }
        let truncated = bytes.len() > MAX_FILE;
        bytes.truncate(MAX_FILE);
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        let truncated = truncated || text.len() > MAX_FILE;
        if text.len() > MAX_FILE {
            let mut end = MAX_FILE;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        Ok(json!({"text":text,"truncated":truncated,"root":self.grant.id}))
    }
    fn list(&self, path: &str, cancelled: &AtomicBool, deadline: Instant) -> Result<Value> {
        let relative = self.relative(path)?;
        let directory = self.open(&relative)?;
        let before = fs::fstat(&directory).map_err(|_| Error::Storage)?;
        if FileType::from_raw_mode(before.st_mode) != FileType::Directory {
            return Err(Error::UnsafeState);
        }
        let mut names = Vec::new();
        let mut scanned = 0;
        let mut omitted = 0;
        for entry in fs::Dir::read_from(&directory).map_err(|_| Error::Storage)? {
            check(cancelled, deadline)?;
            let entry = entry.map_err(|_| Error::Storage)?;
            let name = entry.file_name().to_str().map_err(|_| Error::InvalidRequest)?;
            if name == "." || name == ".." {
                continue;
            }
            scanned += 1;
            if scanned > 4096 {
                return Err(Error::Limit);
            }
            if secret(name) {
                omitted += 1;
                continue;
            }
            let child = match self.open(&relative.join(name)) {
                Ok(child) => child,
                Err(_) => {
                    omitted += 1;
                    continue;
                }
            };
            let stat = fs::fstat(&child).map_err(|_| Error::Storage)?;
            let kind = FileType::from_raw_mode(stat.st_mode);
            if kind != FileType::Directory && (kind != FileType::RegularFile || stat.st_nlink != 1)
            {
                omitted += 1;
                continue;
            }
            names.push((name.to_owned(), kind == FileType::Directory));
        }
        let after = fs::fstat(&directory).map_err(|_| Error::Storage)?;
        if !same(&before, &after) || identity(&self.open(&relative)?)? != identity(&directory)? {
            return Err(Error::Integrity);
        }
        names.sort();
        let truncated = names.len() > 200;
        names.truncate(200);
        Ok(
            json!({"entries":names.into_iter().map(|(name,directory)|json!({"name":name,"directory":directory})).collect::<Vec<_>>(),"truncated":truncated,"omitted":omitted,"root":self.grant.id}),
        )
    }
}
fn same(a: &fs::Stat, b: &fs::Stat) -> bool {
    a.st_uid == b.st_uid
        && a.st_gid == b.st_gid
        && a.st_dev == b.st_dev
        && a.st_ino == b.st_ino
        && a.st_size == b.st_size
        && a.st_mtime == b.st_mtime
        && a.st_mtime_nsec == b.st_mtime_nsec
        && a.st_ctime == b.st_ctime
        && a.st_ctime_nsec == b.st_ctime_nsec
        && a.st_nlink == b.st_nlink
        && a.st_mode == b.st_mode
}
fn check(cancelled: &AtomicBool, deadline: Instant) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else if Instant::now() >= deadline {
        Err(Error::ProviderUnavailable)
    } else {
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    path: String,
}
pub(crate) fn execute(
    roots: &[Captured],
    name: &str,
    args: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Value> {
    let args: Arguments = serde_json::from_str(args).map_err(|_| Error::InvalidRequest)?;
    check(cancelled, deadline)?;
    let mut matches = roots.iter().filter(|root| root.relative(&args.path).is_ok());
    let root = matches.next().ok_or(Error::Unauthorized)?;
    if matches.next().is_some() {
        return Err(Error::Conflict);
    }
    match name {
        "read_file" => root.read(&args.path, cancelled, deadline),
        "list_dir" => root.list(&args.path, cancelled, deadline),
        _ => Err(Error::InvalidRequest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::AtomicU64,
        time::Duration,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rashin-file-roots-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn query(roots: &[Captured], name: &str, path: &str) -> Result<Value> {
        execute(
            roots,
            name,
            &json!({"path":path}).to_string(),
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(1),
        )
    }
    #[test]
    fn held_roots_read_and_list_real_text_while_store_identity_survives_rename() {
        let f = Fixture::new();
        let state = f.dir("private-state");
        let store = crate::store::Store::open(&state).unwrap();
        store.save(br#"{"key":"PRIVATE_FILE_STORE_SENTINEL"}"#).unwrap();
        let protected = store.identity().unwrap();
        let note = f.0.join("note.md");
        fs::write(&note, "ordinary note λ").unwrap();
        let grant = Enrollment::enroll("vault".into(), f.0.clone(), protected).unwrap();
        let roots = vec![grant.capture(protected).unwrap()];
        assert_eq!(
            query(&roots, "read_file", "vault:/note.md").unwrap()["text"],
            "ordinary note λ"
        );
        let listing = query(&roots, "list_dir", "vault:/").unwrap();
        assert_eq!(listing["entries"].as_array().unwrap().len(), 1);
        assert!(listing["omitted"].as_u64().unwrap() > 0);
        assert_eq!(
            query(&roots, "read_file", "vault:/private-state/state.json"),
            Err(Error::Unauthorized)
        );
        fs::rename(&state, f.0.join("renamed-state")).unwrap();
        assert_eq!(
            query(&roots, "read_file", "vault:/renamed-state/state.json"),
            Err(Error::Unauthorized)
        );
        assert!(matches!(
            Enrollment::enroll("direct".into(), f.0.join("renamed-state"), protected),
            Err(Error::Unauthorized)
        ));
        assert!(!listing.to_string().contains("PRIVATE_FILE_STORE_SENTINEL"));
    }
    #[test]
    fn logical_alias_cannot_bypass_retained_secret_markers_or_link_and_root_replacement_guards() {
        let f = Fixture::new();
        let store = crate::store::Store::open(&f.dir("private-state")).unwrap();
        let protected = store.identity().unwrap();
        let root = f.dir(".config");
        fs::create_dir(root.join("prowl-agent")).unwrap();
        fs::write(root.join("prowl-agent/state"), "PRIVATE_GATEWAY_SENTINEL").unwrap();
        fs::write(root.join("note.md"), "ordinary").unwrap();
        fs::write(root.join("api-key"), "PRIVATE_KEY_SENTINEL").unwrap();
        let grant = Enrollment::enroll("config".into(), root.clone(), protected).unwrap();
        let roots = vec![grant.capture(protected).unwrap()];
        for path in [
            "config:/prowl-agent/state",
            "config:/api-key",
            "config:/../private-state/state.json",
            "/etc/passwd",
        ] {
            assert!(query(&roots, "read_file", path).is_err());
        }
        let listing = query(&roots, "list_dir", "config:/").unwrap();
        assert_eq!(listing["entries"].as_array().unwrap().len(), 1);
        symlink("note.md", root.join("link")).unwrap();
        assert!(query(&roots, "read_file", "config:/link").is_err());
        fs::hard_link(root.join("note.md"), root.join("hard")).unwrap();
        assert!(query(&roots, "read_file", "config:/hard").is_err());
        fs::remove_file(root.join("hard")).unwrap();
        let socket = std::os::unix::net::UnixListener::bind(root.join("socket")).unwrap();
        assert!(query(&roots, "read_file", "config:/socket").is_err());
        drop(socket);
        fs::rename(&root, f.0.join("moved-config")).unwrap();
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(grant.capture(protected), Err(Error::Integrity)));
        assert_eq!(query(&roots, "read_file", "config:/note.md").unwrap()["text"], "ordinary");
    }
    #[test]
    fn bounded_text_listing_cancel_and_expiry_do_not_turn_into_unlimited_reads() {
        let f = Fixture::new();
        let store = crate::store::Store::open(&f.dir("private-state")).unwrap();
        let protected = store.identity().unwrap();
        let root = f.dir("vault");
        fs::write(root.join("large"), "λ".repeat(MAX_FILE)).unwrap();
        fs::write(root.join("binary"), b"x\0y").unwrap();
        for n in 0..210 {
            fs::write(root.join(format!("note-{n:03}")), b"text").unwrap();
        }
        let roots = vec![
            Enrollment::enroll("vault".into(), root, protected)
                .unwrap()
                .capture(protected)
                .unwrap(),
        ];
        let large = query(&roots, "read_file", "vault:/large").unwrap();
        assert_eq!(large["truncated"], true);
        assert!(large["text"].as_str().unwrap().len() <= MAX_FILE);
        let listing = query(&roots, "list_dir", "vault:/").unwrap();
        assert_eq!(listing["truncated"], true);
        assert_eq!(listing["entries"].as_array().unwrap().len(), 200);
        assert_eq!(query(&roots, "read_file", "vault:/binary"), Err(Error::InvalidRequest));
        assert_eq!(
            execute(
                &roots,
                "read_file",
                r#"{"path":"vault:/large"}"#,
                &AtomicBool::new(true),
                Instant::now() + Duration::from_secs(1)
            ),
            Err(Error::Cancelled)
        );
        assert_eq!(
            execute(
                &roots,
                "read_file",
                r#"{"path":"vault:/large"}"#,
                &AtomicBool::new(false),
                Instant::now()
            ),
            Err(Error::ProviderUnavailable)
        );
    }
    #[test]
    #[ignore = "requires isolated user/mount namespace with mount capability"]
    fn bind_alias_and_mount_crossing_never_enter_credential_store() {
        let f = Fixture::new();
        let store_path = f.dir("private-state");
        let store = crate::store::Store::open(&store_path).unwrap();
        store.save(br#"{"key":"PRIVATE_BIND_SENTINEL"}"#).unwrap();
        let protected = store.identity().unwrap();
        let root = f.dir("vault");
        let alias = f.dir("vault/alias");
        assert!(
            std::process::Command::new("/usr/bin/mount")
                .args(["--bind"])
                .arg(&store_path)
                .arg(&alias)
                .status()
                .unwrap()
                .success()
        );
        let result = std::panic::catch_unwind(|| {
            assert!(matches!(
                Enrollment::enroll("alias".into(), alias.clone(), protected),
                Err(Error::Unauthorized)
            ));
            let roots = vec![
                Enrollment::enroll("vault".into(), root, protected)
                    .unwrap()
                    .capture(protected)
                    .unwrap(),
            ];
            assert!(query(&roots, "read_file", "vault:/alias/state.json").is_err());
        });
        assert!(
            std::process::Command::new("/usr/bin/umount").arg(&alias).status().unwrap().success()
        );
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
}
