use super::*;
use crate::{
    EntryMetadata, ExtendedMetadata, RelativePath,
    inode_metadata::{InodeMetadataError, prepare_inode_metadata},
};
use rustix::fs::{XattrFlags, fgetxattr, fsetxattr};
use sha2::{Digest, Sha256 as Hash};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-inode-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self) -> File {
        fs::write(self.0.join("file"), b"verified content").unwrap();
        fs::set_permissions(self.0.join("file"), fs::Permissions::from_mode(0o600)).unwrap();
        OpenOptions::new().read(true).write(true).open(self.0.join("file")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn digest(bytes: &[u8]) -> Sha256 {
    Sha256::new(Hash::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap()
}

fn entry(file: &File, directory: bool, acl: Option<AclPayload>) -> StagedEntry {
    let stat = file.metadata().unwrap();
    let metadata = EntryMetadata {
        mode: 0o640,
        uid: stat.uid(),
        gid: stat.gid(),
        mtime_unix_ns: -123_456_789,
        acl_sha256: acl.as_ref().and_then(|acl| acl.encode().unwrap()).map(|bytes| digest(&bytes)),
        extended: ExtendedMetadata::default(),
    };
    let content: Arc<[u8]> = Arc::from(b"verified content".as_slice());
    let kind = if directory {
        EntryKind::Directory { metadata }
    } else {
        EntryKind::RegularFile {
            metadata,
            size: content.len() as u64,
            sha256: digest(&content),
            hard_link_group: None,
        }
    };
    StagedEntry {
        record: FileRecord::new(StateRoot::EtcRyoku, RelativePath::new("file").unwrap(), kind)
            .unwrap(),
        content: (!directory).then_some(content),
        acl: acl.map(Arc::new),
        xattrs: BTreeMap::new(),
    }
}

// Linux POSIX ACL xattr v2: owner rw, named user r, group none, mask r, other none.
// The named user ID must be mapped in this process's user namespace. Test
// callers use the fixture owner's mapped UID so the ACL behavior is exercised
// even in single-ID build containers.
fn access_acl(uid: u32) -> Vec<u8> {
    let mut bytes = 2_u32.to_le_bytes().to_vec();
    for (tag, permissions, id) in [
        (1_u16, 6_u16, u32::MAX),
        (2, 4, uid),
        (4, 0, u32::MAX),
        (16, 4, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(permissions.to_le_bytes());
        bytes.extend(id.to_le_bytes());
    }
    bytes
}

fn read_acl(file: &File, name: &str) -> Option<Vec<u8>> {
    let mut value = vec![0; 1024];
    match fgetxattr(file, name, &mut value) {
        Ok(length) => {
            value.truncate(length);
            Some(value)
        }
        Err(rustix::io::Errno::NODATA) => None,
        Err(error) => panic!("ACL read failed: {error}"),
    }
}

#[test]
fn pinned_inode_roundtrip_preserves_content_offset_and_replacement_path() {
    let fixture = Fixture::new();
    let mut file = fixture.file();
    let entry = entry(&file, false, None);
    file.seek(SeekFrom::Start(4)).unwrap();
    fs::rename(fixture.0.join("file"), fixture.0.join("pinned")).unwrap();
    fs::write(fixture.0.join("file"), b"unrelated replacement").unwrap();
    prepare_inode_metadata(&file, &entry).unwrap();
    assert_eq!(file.stream_position().unwrap(), 4);
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o640);
    assert_eq!(
        i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
        -123_456_789
    );
    assert_eq!(fs::read(fixture.0.join("pinned")).unwrap(), b"verified content");
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"unrelated replacement");
}

#[test]
fn regular_file_acl_is_restored_then_absence_clears_it_without_touching_user_xattrs() {
    let fixture = Fixture::new();
    let file = fixture.file();
    let acl = access_acl(file.metadata().unwrap().uid());
    let staged = entry(&file, false, Some(AclPayload { access: Some(acl.clone()), default: None }));
    fsetxattr(&file, "user.ryoku-test", b"unrelated", XattrFlags::empty()).unwrap();
    prepare_inode_metadata(&file, &staged).unwrap();
    assert_eq!(read_acl(&file, "system.posix_acl_access"), Some(acl));
    prepare_inode_metadata(&file, &entry(&file, false, None)).unwrap();
    assert_eq!(read_acl(&file, "system.posix_acl_access"), None);
    assert_eq!(read_acl(&file, "user.ryoku-test"), Some(b"unrelated".to_vec()));
}

#[test]
fn directory_access_and_default_acls_restore_and_clear_exactly() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("directory")).unwrap();
    let file = File::open(fixture.0.join("directory")).unwrap();
    let acl = access_acl(file.metadata().unwrap().uid());
    let staged = entry(
        &file,
        true,
        Some(AclPayload { access: Some(acl.clone()), default: Some(acl.clone()) }),
    );
    prepare_inode_metadata(&file, &staged).unwrap();
    assert_eq!(read_acl(&file, "system.posix_acl_access"), Some(acl.clone()));
    assert_eq!(read_acl(&file, "system.posix_acl_default"), Some(acl));
    prepare_inode_metadata(&file, &entry(&file, true, None)).unwrap();
    assert_eq!(read_acl(&file, "system.posix_acl_access"), None);
    assert_eq!(read_acl(&file, "system.posix_acl_default"), None);
}

#[test]
fn mismatched_content_or_external_hardlink_fails_before_permissions_change() {
    let fixture = Fixture::new();
    let file = fixture.file();
    let staged = entry(&file, false, None);
    fs::write(fixture.0.join("file"), b"different bytes!").unwrap();
    assert!(matches!(
        prepare_inode_metadata(&file, &staged),
        Err(InodeMetadataError::InodeMismatch)
    ));
    fs::write(fixture.0.join("file"), b"verified content").unwrap();
    fs::hard_link(fixture.0.join("file"), fixture.0.join("alias")).unwrap();
    assert!(matches!(
        prepare_inode_metadata(&file, &staged),
        Err(InodeMetadataError::InodeMismatch)
    ));
    assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn unrepresentable_timestamp_and_invalid_owner_fail_before_mutation() {
    let fixture = Fixture::new();
    let file = fixture.file();
    let mut staged = entry(&file, false, None);
    if let EntryKind::RegularFile { metadata, .. } = &mut staged.record.kind {
        metadata.mtime_unix_ns = i128::MAX;
    }
    assert!(matches!(
        prepare_inode_metadata(&file, &staged),
        Err(InodeMetadataError::InvalidMetadata)
    ));
    if let EntryKind::RegularFile { metadata, .. } = &mut staged.record.kind {
        metadata.mtime_unix_ns = 0;
        metadata.uid = u32::MAX;
    }
    assert!(matches!(
        prepare_inode_metadata(&file, &staged),
        Err(InodeMetadataError::InvalidMetadata)
    ));
    assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn incompatible_acl_mode_is_rejected_by_exact_readback() {
    let fixture = Fixture::new();
    let file = fixture.file();
    let acl = access_acl(file.metadata().unwrap().uid());
    let mut staged = entry(&file, false, Some(AclPayload { access: Some(acl), default: None }));
    if let EntryKind::RegularFile { metadata, .. } = &mut staged.record.kind {
        metadata.mode = 0o600;
    }
    assert!(matches!(
        prepare_inode_metadata(&file, &staged),
        Err(InodeMetadataError::ReadbackMismatch)
    ));
}

#[test]
fn absence_and_symlink_records_cannot_change_a_regular_inode() {
    let fixture = Fixture::new();
    let file = fixture.file();
    let mut staged = entry(&file, false, None);
    let metadata = match &staged.record.kind {
        EntryKind::RegularFile { metadata, .. } => metadata.clone(),
        _ => unreachable!(),
    };
    for kind in [EntryKind::Absent, EntryKind::Symlink { metadata, target: "target".into() }] {
        staged.record.kind = kind;
        assert!(matches!(
            prepare_inode_metadata(&file, &staged),
            Err(InodeMetadataError::UnsupportedKind)
        ));
        assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
    }
}
