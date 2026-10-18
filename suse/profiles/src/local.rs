use std::{
    fs::File,
    io::{Read, Write},
    sync::atomic::{AtomicU64, Ordering},
};

use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, RenameFlags, ResolveFlags, XattrFlags, fchmod, fgetxattr,
    flistxattr, fsetxattr, fstat, fsync, openat2, renameat, renameat_with, unlinkat,
};

use crate::{MAX_PROFILE_DOCUMENT_BYTES, ProfileDocument, ProfileDocumentError};

/// The only profile document name this loader can open beneath its root.
/// The port-owned typed preference file. The preserved upstream Hub owns its
/// separate `profile.json` (including profile hero selection); sharing that
/// path would let the two formats overwrite one another.
pub const LOCAL_PROFILE_FILE: &str = "opensuse-profile.json";
const TEMP_FILE_PREFIX: &str = ".opensuse-profile-";
const LEGACY_PROFILE_FILE: &str = "profile.json";
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

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

/// Failure to atomically save the fixed local profile document.
#[derive(Debug)]
pub enum ProfileSaveError {
    InvalidRoot,
    InvalidFile,
    DestinationExists,
    Document(ProfileDocumentError),
    Filesystem(rustix::io::Errno),
    Io(std::io::Error),
    DirectorySyncFailed(rustix::io::Errno),
}

impl std::fmt::Display for ProfileSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("profile root identity or mode is invalid"),
            Self::InvalidFile => {
                formatter.write_str("existing profile file identity, type, or mode is invalid")
            }
            Self::DestinationExists => formatter.write_str("local profile already exists"),
            Self::Document(error) => write!(formatter, "profile document is invalid: {error}"),
            Self::Filesystem(error) => write!(formatter, "profile file access failed: {error}"),
            Self::Io(error) => write!(formatter, "profile file write failed: {error}"),
            Self::DirectorySyncFailed(error) => {
                write!(formatter, "profile was replaced but its directory sync failed: {error}")
            }
        }
    }
}

impl std::error::Error for ProfileSaveError {}
impl From<rustix::io::Errno> for ProfileSaveError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Filesystem(error)
    }
}
impl From<std::io::Error> for ProfileSaveError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<ProfileDocumentError> for ProfileSaveError {
    fn from(error: ProfileDocumentError) -> Self {
        Self::Document(error)
    }
}

/// Atomically save the port-owned local profile beneath an already-open
/// per-user Ryoku configuration root. No ambient path, remote, credential, or
/// elevated helper is used. Existing files must satisfy the same owner, type,
/// link-count, and mode policy as [`load_local_profile`]; their mode is
/// preserved. A new file is private (`0600`). The temporary file is created
/// relative to the root descriptor, synced, renamed over the fixed leaf, and
/// followed by a directory sync. If that final sync fails, the save has
/// occurred but its crash durability is uncertain and the error says so.
pub fn save_local_profile(
    root: &File,
    expected_owner: ProfileOwner,
    document: &ProfileDocument,
) -> Result<(), ProfileSaveError> {
    save_local_profile_with_policy(root, expected_owner, document, false)
}

/// Copy a recognized legacy profile into the port-owned file without changing
/// or removing the upstream-owned source. The migration only accepts the
/// exact canonical schema emitted by the earlier Rust prototype. It will not
/// replace a destination created by another writer while migration is in
/// progress. A retry succeeds only when the validated destination already
/// contains the exact same imported document.
pub fn migrate_legacy_local_profile(
    root: &File,
    expected_owner: ProfileOwner,
) -> Result<ProfileDocument, ProfileMigrationError> {
    let document = read_legacy_local_profile(root, expected_owner)?;
    match save_local_profile_with_policy(root, expected_owner, &document, true) {
        Ok(()) => (),
        Err(ProfileSaveError::DestinationExists)
        | Err(ProfileSaveError::Filesystem(rustix::io::Errno::EXIST)) => {
            match load_local_profile(root, expected_owner) {
                Ok(existing) if existing == document => (),
                _ => return Err(ProfileMigrationError::DestinationExists),
            }
        }
        Err(error) => return Err(ProfileMigrationError::Save(error)),
    }
    Ok(document)
}

#[derive(Debug)]
pub enum ProfileMigrationError {
    InvalidLegacy,
    DestinationExists,
    InvalidRoot(ProfileSaveError),
    Load(ProfileLoadError),
    Save(ProfileSaveError),
}

impl std::fmt::Display for ProfileMigrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLegacy => formatter.write_str("legacy profile is not recognized"),
            Self::DestinationExists => formatter.write_str("local profile already exists"),
            Self::InvalidRoot(error) => write!(formatter, "profile root is invalid: {error}"),
            Self::Load(error) => write!(formatter, "legacy profile could not be read: {error}"),
            Self::Save(error) => write!(formatter, "local profile could not be saved: {error}"),
        }
    }
}

impl std::error::Error for ProfileMigrationError {}

fn save_local_profile_with_policy(
    root: &File,
    expected_owner: ProfileOwner,
    document: &ProfileDocument,
    create_only: bool,
) -> Result<(), ProfileSaveError> {
    validate_profile_root(root, expected_owner)?;
    let bytes = document.canonical_bytes()?;
    let existing_mode = existing_profile_mode(root, expected_owner)?;
    if create_only && existing_mode.is_some() {
        return Err(ProfileSaveError::DestinationExists);
    }
    let mode = existing_mode.unwrap_or(0o600);

    let (name, mut file) = create_profile_temp(root)?;
    let result = (|| {
        file.write_all(&bytes)?;
        fchmod(&file, Mode::from_bits_retain(mode))?;
        copy_profile_attributes(root, expected_owner, &file)?;
        fsync(&file)?;
        if create_only {
            renameat_with(root, name.as_str(), root, LOCAL_PROFILE_FILE, RenameFlags::NOREPLACE)?;
        } else {
            renameat(root, name.as_str(), root, LOCAL_PROFILE_FILE)?;
        }
        fsync(root).map_err(ProfileSaveError::DirectorySyncFailed)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = unlinkat(root, name.as_str(), AtFlags::empty());
    }
    result
}

fn read_legacy_local_profile(
    root: &File,
    expected_owner: ProfileOwner,
) -> Result<ProfileDocument, ProfileMigrationError> {
    validate_profile_root(root, expected_owner).map_err(ProfileMigrationError::InvalidRoot)?;
    let fd = openat2(
        root,
        LEGACY_PROFILE_FILE,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )
    .map_err(|error| ProfileMigrationError::Load(ProfileLoadError::Filesystem(error)))?;
    let file = File::from(fd);
    let before = fstat(&file)
        .map_err(|error| ProfileMigrationError::Load(ProfileLoadError::Filesystem(error)))?;
    let mode = before.st_mode & 0o7777;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_uid != expected_owner.uid
        || before.st_gid != expected_owner.gid
        || !crate::PERSONAL_PROFILE_MODES.contains(&mode)
        || before.st_nlink != 1
    {
        return Err(ProfileMigrationError::InvalidLegacy);
    }
    if before.st_size < 0 || before.st_size as usize > MAX_PROFILE_DOCUMENT_BYTES {
        return Err(ProfileMigrationError::InvalidLegacy);
    }
    let mut bytes = Vec::with_capacity(before.st_size as usize);
    (&file)
        .take((MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ProfileMigrationError::Load(ProfileLoadError::Io(error)))?;
    let after = fstat(&file)
        .map_err(|error| ProfileMigrationError::Load(ProfileLoadError::Filesystem(error)))?;
    if !same_file_version(&before, &after) || bytes.len() != after.st_size as usize {
        return Err(ProfileMigrationError::Load(ProfileLoadError::ConcurrentModification));
    }
    crate::classify_legacy_personal_profile(&bytes)
        .map(|candidate| candidate.document)
        .ok_or(ProfileMigrationError::InvalidLegacy)
}

fn copy_profile_attributes(
    root: &File,
    expected_owner: ProfileOwner,
    target: &File,
) -> Result<(), ProfileSaveError> {
    let source = match openat2(
        root,
        LOCAL_PROFILE_FILE,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(rustix::io::Errno::LOOP) => return Err(ProfileSaveError::InvalidFile),
        Err(error) => return Err(error.into()),
    };
    let stat = fstat(&source)?;
    let mode = stat.st_mode & 0o7777;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_owner.uid
        || stat.st_gid != expected_owner.gid
        || !crate::PERSONAL_PROFILE_MODES.contains(&mode)
        || stat.st_nlink != 1
    {
        return Err(ProfileSaveError::InvalidFile);
    }

    let mut names = vec![0_u8; 64 * 1024];
    let length = match flistxattr(&source, &mut names) {
        Ok(length) => length,
        Err(rustix::io::Errno::OPNOTSUPP) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for name in names[..length].split(|byte| *byte == 0).filter(|name| !name.is_empty()) {
        if !matches!(name, b"system.posix_acl_access" | b"security.selinux") {
            return Err(ProfileSaveError::InvalidFile);
        }
        let mut value = vec![0_u8; 64 * 1024];
        let value_length = fgetxattr(&source, name, &mut value)?;
        fsetxattr(target, name, &value[..value_length], XattrFlags::empty())?;
    }
    Ok(())
}

fn validate_profile_root(
    root: &File,
    expected_owner: ProfileOwner,
) -> Result<(), ProfileSaveError> {
    let stat = fstat(root)?;
    let mode = stat.st_mode & 0o7777;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != expected_owner.uid
        || stat.st_gid != expected_owner.gid
        || !matches!(mode, 0o700 | 0o755)
    {
        return Err(ProfileSaveError::InvalidRoot);
    }
    Ok(())
}

fn existing_profile_mode(
    root: &File,
    expected_owner: ProfileOwner,
) -> Result<Option<u32>, ProfileSaveError> {
    let fd = match openat2(
        root,
        LOCAL_PROFILE_FILE,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP) => return Err(ProfileSaveError::InvalidFile),
        Err(error) => return Err(error.into()),
    };
    let file = File::from(fd);
    let stat = fstat(&file)?;
    let mode = stat.st_mode & 0o7777;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_owner.uid
        || stat.st_gid != expected_owner.gid
        || !crate::PERSONAL_PROFILE_MODES.contains(&mode)
        || stat.st_nlink != 1
    {
        return Err(ProfileSaveError::InvalidFile);
    }
    Ok(Some(mode))
}

fn create_profile_temp(root: &File) -> Result<(String, File), ProfileSaveError> {
    for _ in 0..16 {
        let nonce = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let name = format!("{TEMP_FILE_PREFIX}{}-{nonce}.tmp", std::process::id());
        match openat2(
            root,
            name.as_str(),
            OFlags::WRONLY
                | OFlags::CLOEXEC
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::from_bits_retain(0o600),
            ResolveFlags::BENEATH
                | ResolveFlags::NO_SYMLINKS
                | ResolveFlags::NO_MAGICLINKS
                | ResolveFlags::NO_XDEV,
        ) {
            Ok(fd) => return Ok((name, File::from(fd))),
            Err(rustix::io::Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ProfileSaveError::Filesystem(rustix::io::Errno::EXIST))
}

/// Load the port-owned `opensuse-profile.json` relative to an already-open
/// Ryoku configuration root descriptor.
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

    #[test]
    fn port_profile_filename_does_not_shadow_upstream_hub_data() {
        assert_eq!(LOCAL_PROFILE_FILE, "opensuse-profile.json");
        assert_ne!(LOCAL_PROFILE_FILE, "profile.json");
    }

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

        fn write_legacy_profile(&self, bytes: &[u8], mode: u32) {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.0.join(LEGACY_PROFILE_FILE))
                .unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
            fs::set_permissions(self.0.join(LEGACY_PROFILE_FILE), fs::Permissions::from_mode(mode))
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

    fn sample_document(theme: crate::Theme) -> ProfileDocument {
        ProfileDocument::new(
            crate::ProfileId::new("demo").unwrap(),
            crate::SettingsLayer { theme: Some(theme), ..crate::SettingsLayer::default() },
        )
    }

    #[test]
    fn saves_new_profile_privately_and_atomically() {
        let root = TestRoot::new();
        let descriptor = root.descriptor();
        let owner = root.owner();

        save_local_profile(&descriptor, owner, &sample_document(crate::Theme::RyokuDark)).unwrap();

        let metadata = fs::metadata(root.0.join(LOCAL_PROFILE_FILE)).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        let loaded = load_local_profile(&descriptor, owner).unwrap();
        assert_eq!(loaded.settings.theme, Some(crate::Theme::RyokuDark));
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }

    #[test]
    fn explicit_legacy_migration_copies_canonical_profile_and_preserves_source() {
        let root = TestRoot::new();
        let legacy = ProfileDocument::new(
            crate::ProfileId::new(crate::GENERIC_PROFILE_ID).unwrap(),
            crate::SettingsLayer {
                desktop: Some(crate::Desktop::Kde),
                theme: Some(crate::Theme::System),
                density: Some(crate::Density::Compact),
            },
        )
        .canonical_bytes()
        .unwrap();
        root.write_legacy_profile(&legacy, 0o600);
        let descriptor = root.descriptor();

        let migrated = migrate_legacy_local_profile(&descriptor, root.owner()).unwrap();

        assert_eq!(migrated, legacy_document());
        assert_eq!(load_local_profile(&descriptor, root.owner()).unwrap(), migrated);
        assert_eq!(fs::read(root.0.join(LEGACY_PROFILE_FILE)).unwrap(), legacy);
        assert_eq!(fs::metadata(root.0.join(LOCAL_PROFILE_FILE)).unwrap().mode() & 0o7777, 0o600);
        assert_eq!(migrate_legacy_local_profile(&descriptor, root.owner()).unwrap(), migrated);
        assert_eq!(fs::read(root.0.join(LEGACY_PROFILE_FILE)).unwrap(), legacy);
    }

    fn legacy_document() -> ProfileDocument {
        ProfileDocument::new(
            crate::ProfileId::new(crate::GENERIC_PROFILE_ID).unwrap(),
            crate::SettingsLayer {
                desktop: Some(crate::Desktop::Kde),
                theme: Some(crate::Theme::System),
                density: Some(crate::Density::Compact),
            },
        )
    }

    #[test]
    fn legacy_migration_rejects_unknown_source_and_existing_destination() {
        let root = TestRoot::new();
        root.write_legacy_profile(br#"{"hero":{"kind":"builtin"}}"#, 0o600);
        let descriptor = root.descriptor();
        assert!(matches!(
            migrate_legacy_local_profile(&descriptor, root.owner()),
            Err(ProfileMigrationError::InvalidLegacy)
        ));

        fs::remove_file(root.0.join(LEGACY_PROFILE_FILE)).unwrap();
        let legacy = legacy_document().canonical_bytes().unwrap();
        root.write_legacy_profile(&legacy, 0o600);
        root.write_profile(VALID_PROFILE, 0o644);
        let before = fs::read(root.0.join(LOCAL_PROFILE_FILE)).unwrap();
        assert!(matches!(
            migrate_legacy_local_profile(&descriptor, root.owner()),
            Err(ProfileMigrationError::DestinationExists)
        ));
        assert_eq!(fs::read(root.0.join(LOCAL_PROFILE_FILE)).unwrap(), before);
        assert_eq!(fs::read(root.0.join(LEGACY_PROFILE_FILE)).unwrap(), legacy);
    }

    #[test]
    fn updates_profile_and_preserves_supported_mode() {
        let root = TestRoot::new();
        root.write_profile(VALID_PROFILE, 0o644);
        let descriptor = root.descriptor();

        save_local_profile(&descriptor, root.owner(), &sample_document(crate::Theme::System))
            .unwrap();

        let metadata = fs::metadata(root.0.join(LOCAL_PROFILE_FILE)).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o644);
        assert_eq!(
            load_local_profile(&descriptor, root.owner()).unwrap().settings.theme,
            Some(crate::Theme::System)
        );
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }

    #[test]
    fn refuses_symlink_destination_without_replacing_target() {
        let root = TestRoot::new();
        let outside = root.0.with_extension("outside");
        fs::write(&outside, VALID_PROFILE).unwrap();
        std::os::unix::fs::symlink(&outside, root.0.join(LOCAL_PROFILE_FILE)).unwrap();

        let result = save_local_profile(
            &root.descriptor(),
            root.owner(),
            &sample_document(crate::Theme::RyokuLight),
        );

        assert!(matches!(result, Err(ProfileSaveError::InvalidFile)));
        assert_eq!(fs::read(&outside).unwrap(), VALID_PROFILE);
        assert!(
            fs::symlink_metadata(root.0.join(LOCAL_PROFILE_FILE)).unwrap().file_type().is_symlink()
        );
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn refuses_unmanaged_xattrs_without_replacing_existing_profile() {
        let root = TestRoot::new();
        root.write_profile(VALID_PROFILE, 0o600);
        let original = File::open(root.0.join(LOCAL_PROFILE_FILE)).unwrap();
        rustix::fs::fsetxattr(&original, &b"user.ryoku-test"[..], b"preserve", XattrFlags::empty())
            .unwrap();

        let result = save_local_profile(
            &root.descriptor(),
            root.owner(),
            &sample_document(crate::Theme::RyokuLight),
        );

        assert!(matches!(result, Err(ProfileSaveError::InvalidFile)));
        assert_eq!(
            load_local_profile(&root.descriptor(), root.owner()).unwrap().settings.desktop,
            Some(crate::Desktop::Kde)
        );
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
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
