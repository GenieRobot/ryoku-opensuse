use std::{fs::File, io::Read};

use rustix::fs::{FileType, OFlags, ResolveFlags, fstat, openat2};

use crate::{MAX_PROFILE_DOCUMENT_BYTES, ProfileDocument, ProfileDocumentError};

/// The only profile document name this loader can open beneath its root.
pub const LOCAL_PROFILE_FILE: &str = "profile.json";

/// Expected identity for the supplied user-owned profile root and document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileOwner {
    pub uid: u32,
    pub gid: u32,
}

/// Failure to open, validate, read, or parse the fixed local profile document.
#[derive(Debug)]
pub enum ProfileLoadError {
    InvalidRoot,
    InvalidFile,
    InputTooLarge,
    ConcurrentModification,
    Filesystem(rustix::io::Errno),
    Io(std::io::Error),
    Document(ProfileDocumentError),
}

impl std::fmt::Display for ProfileLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("profile root identity or mode is invalid"),
            Self::InvalidFile => {
                formatter.write_str("profile file identity, type, or mode is invalid")
            }
            Self::InputTooLarge => formatter.write_str("profile file exceeds size limit"),
            Self::ConcurrentModification => {
                formatter.write_str("profile file changed while loading")
            }
            Self::Filesystem(error) => write!(formatter, "profile file access failed: {error}"),
            Self::Io(error) => write!(formatter, "profile file read failed: {error}"),
            Self::Document(error) => write!(formatter, "profile document is invalid: {error}"),
        }
    }
}

impl std::error::Error for ProfileLoadError {}

impl From<rustix::io::Errno> for ProfileLoadError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Filesystem(error)
    }
}

impl From<ProfileDocumentError> for ProfileLoadError {
    fn from(error: ProfileDocumentError) -> Self {
        Self::Document(error)
    }
}

/// Load `profile.json` relative to an already-open profile root descriptor.
///
/// The root and file must have the supplied UID/GID. Root modes are limited to
/// `0700` or `0755`; file modes are limited to `0600` or `0644`. Those exact
/// allowlists reject special bits, group-writable files, and world-writable
/// content. The file is opened beneath the descriptor without following
/// symlinks, crossing mounts, or accepting non-regular files.
pub fn load_local_profile(
    root: &File,
    expected_owner: ProfileOwner,
) -> Result<ProfileDocument, ProfileLoadError> {
    load_local_profile_with_owners(root, expected_owner, expected_owner)
}

fn load_local_profile_with_owners(
    root: &File,
    root_owner: ProfileOwner,
    file_owner: ProfileOwner,
) -> Result<ProfileDocument, ProfileLoadError> {
    let root_stat = fstat(root).map_err(ProfileLoadError::Filesystem)?;
    let root_mode = root_stat.st_mode & 0o7777;
    if FileType::from_raw_mode(root_stat.st_mode) != FileType::Directory
        || root_stat.st_uid != root_owner.uid
        || root_stat.st_gid != root_owner.gid
        || !matches!(root_mode, 0o700 | 0o755)
    {
        return Err(ProfileLoadError::InvalidRoot);
    }

    let fd = openat2(
        root,
        LOCAL_PROFILE_FILE,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )
    .map_err(ProfileLoadError::Filesystem)?;
    let file = File::from(fd);
    let before = fstat(&file).map_err(ProfileLoadError::Filesystem)?;
    let mode = before.st_mode & 0o7777;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_uid != file_owner.uid
        || before.st_gid != file_owner.gid
        || !crate::PERSONAL_PROFILE_MODES.contains(&mode)
        || before.st_nlink != 1
    {
        return Err(ProfileLoadError::InvalidFile);
    }
    if before.st_size < 0 || before.st_size as usize > MAX_PROFILE_DOCUMENT_BYTES {
        return Err(ProfileLoadError::InputTooLarge);
    }

    let mut bytes = Vec::with_capacity(before.st_size as usize);
    (&file)
        .take((MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(ProfileLoadError::Io)?;
    if bytes.len() > MAX_PROFILE_DOCUMENT_BYTES {
        return Err(ProfileLoadError::InputTooLarge);
    }
    let after = fstat(&file).map_err(ProfileLoadError::Filesystem)?;
    if !same_file_version(&before, &after) || bytes.len() != after.st_size as usize {
        return Err(ProfileLoadError::ConcurrentModification);
    }

    ProfileDocument::parse(&bytes).map_err(ProfileLoadError::Document)
}

fn same_file_version(before: &rustix::fs::Stat, after: &rustix::fs::Stat) -> bool {
    before.st_dev == after.st_dev
        && before.st_ino == after.st_ino
        && before.st_mode == after.st_mode
        && before.st_uid == after.st_uid
        && before.st_gid == after.st_gid
        && before.st_nlink == after.st_nlink
        && before.st_size == after.st_size
        && before.st_mtime == after.st_mtime
        && before.st_mtime_nsec == after.st_mtime_nsec
        && before.st_ctime == after.st_ctime
        && before.st_ctime_nsec == after.st_ctime_nsec
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File, OpenOptions},
        io::Write,
        os::unix::fs::{MetadataExt, PermissionsExt},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ryoku-profile-loader-{}-{}",
                std::process::id(),
                NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn descriptor(&self) -> File {
            File::open(&self.0).unwrap()
        }

        fn owner(&self) -> ProfileOwner {
            let metadata = fs::metadata(&self.0).unwrap();
            ProfileOwner { uid: metadata.uid(), gid: metadata.gid() }
        }

        fn write_profile(&self, bytes: &[u8], mode: u32) {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.0.join(LOCAL_PROFILE_FILE))
                .unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
            fs::set_permissions(self.0.join(LOCAL_PROFILE_FILE), fs::Permissions::from_mode(mode))
                .unwrap();
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const VALID_PROFILE: &[u8] =
        br#"{"schema_version":1,"id":"demo","settings":{"desktop":"kde"}}"#;

    #[test]
    fn loads_valid_owned_profile_from_fixed_root() {
        let root = TestRoot::new();
        root.write_profile(VALID_PROFILE, 0o600);

        let document = load_local_profile(&root.descriptor(), root.owner()).unwrap();

        assert_eq!(document.id.as_str(), "demo");
        assert_eq!(document.settings.desktop, Some(crate::Desktop::Kde));
    }

    #[test]
    fn refuses_symlink_profile_without_following_its_target() {
        let root = TestRoot::new();
        let target = root.0.with_extension("outside");
        fs::write(&target, VALID_PROFILE).unwrap();
        std::os::unix::fs::symlink(&target, root.0.join(LOCAL_PROFILE_FILE)).unwrap();

        let result = load_local_profile(&root.descriptor(), root.owner());

        assert!(matches!(result, Err(ProfileLoadError::Filesystem(_))));
        fs::remove_file(target).unwrap();
    }

    #[test]
    fn refuses_oversized_profile_before_parsing() {
        let root = TestRoot::new();
        root.write_profile(&vec![b' '; MAX_PROFILE_DOCUMENT_BYTES + 1], 0o600);

        assert!(matches!(
            load_local_profile(&root.descriptor(), root.owner()),
            Err(ProfileLoadError::InputTooLarge)
        ));
    }

    #[test]
    fn refuses_profile_with_unexpected_owner() {
        let root = TestRoot::new();
        root.write_profile(VALID_PROFILE, 0o600);
        let mut wrong_owner = root.owner();
        wrong_owner.uid = wrong_owner.uid.wrapping_add(1);

        assert!(matches!(
            load_local_profile_with_owners(&root.descriptor(), root.owner(), wrong_owner),
            Err(ProfileLoadError::InvalidFile)
        ));
    }

    #[test]
    fn refuses_profile_with_unsafe_mode() {
        let root = TestRoot::new();
        root.write_profile(VALID_PROFILE, 0o666);

        assert!(matches!(
            load_local_profile(&root.descriptor(), root.owner()),
            Err(ProfileLoadError::InvalidFile)
        ));
    }
}
