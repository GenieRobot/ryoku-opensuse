//! Bounded-memory asset payload publication inside immutable generations.
use super::*;
use crate::ryogami_assets::{CapturedAssets, MAX_CAPTURE_LIMITS};
use std::os::unix::fs::FileExt;

pub(super) fn total_limit(manifest: &RecoveryManifest) -> u64 {
    if manifest.assets().is_some() {
        MAX_GENERATION_BYTES + MAX_CAPTURE_LIMITS.max_total_bytes
    } else {
        MAX_GENERATION_BYTES
    }
}
pub(super) fn payload_limit(manifest: &RecoveryManifest) -> usize {
    if manifest.assets().is_some() {
        MAX_GENERATION_PAYLOADS + MAX_CAPTURE_LIMITS.max_paths * 3
    } else {
        MAX_GENERATION_PAYLOADS
    }
}
pub(super) fn content_size(
    manifest: &RecoveryManifest,
    digest: &Sha256,
) -> Result<u64, GenerationError> {
    let mut found = None;
    for kind in manifest
        .files()
        .iter()
        .map(|r| &r.kind)
        .chain(manifest.assets().into_iter().flat_map(|a| a.entries().iter().map(|r| &r.kind)))
    {
        if let EntryKind::RegularFile { size, sha256, .. } = kind
            && sha256 == digest
        {
            if found.is_some_and(|previous| previous != *size) {
                return Err(GenerationError::PayloadMismatch);
            }
            found = Some(*size);
        }
        if let Some(metadata) = entry_metadata(kind)
            && (metadata.acl_sha256.as_ref() == Some(digest)
                || metadata.extended.values.values().any(|d| d == digest))
        {
            // A digest serving a metadata role must use the bounded metadata
            // decoder. Never bypass its shape validation via a stream.
            return Err(GenerationError::PayloadMismatch);
        }
    }
    found.ok_or(GenerationError::PayloadMismatch)
}
pub(super) fn is_large_asset(
    manifest: &RecoveryManifest,
    digest: &Sha256,
) -> Result<bool, GenerationError> {
    let large = manifest.assets().is_some_and(|assets| assets.entries().iter().any(|r| matches!(&r.kind, EntryKind::RegularFile { size, sha256, .. } if sha256 == digest && *size > MAX_GENERATION_FILE_BYTES)));
    if large {
        content_size(manifest, digest)?;
    }
    Ok(large)
}
#[derive(Eq, PartialEq)]
struct InodeFingerprint {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    ctime: i64,
    ctime_nsec: u64,
}
fn fingerprint(file: &File) -> Result<InodeFingerprint, GenerationError> {
    let stat = rustix::fs::fstat(file)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_size < 0
    {
        return Err(GenerationError::InvalidEntryType);
    }
    Ok(InodeFingerprint {
        device: stat.st_dev,
        inode: stat.st_ino,
        size: stat.st_size as u64,
        mode: stat.st_mode,
        uid: stat.st_uid,
        gid: stat.st_gid,
        ctime: stat.st_ctime,
        ctime_nsec: stat.st_ctime_nsec,
    })
}
pub(super) fn verify_source(
    file: &File,
    size: u64,
    expected: &Sha256,
) -> Result<(), GenerationError> {
    stream(file, size, expected, |_| Ok(()), &mut || true)
}
fn stream(
    file: &File,
    size: u64,
    expected: &Sha256,
    mut consume: impl FnMut(&[u8]) -> Result<(), GenerationError>,
    boundary: &mut impl FnMut() -> bool,
) -> Result<(), GenerationError> {
    let before = fingerprint(file)?;
    if before.size != size || size > MAX_CAPTURE_LIMITS.max_file_bytes {
        return Err(GenerationError::PayloadMismatch);
    }
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0u64;
    let mut hasher = Sha256Digest::new();
    while offset < size {
        if !boundary() {
            return Err(GenerationError::PublicationRejected);
        }
        let count = file.read_at(&mut buffer[..(size - offset).min(64 * 1024) as usize], offset)?;
        if count == 0 {
            return Err(GenerationError::PayloadMismatch);
        }
        hasher.update(&buffer[..count]);
        consume(&buffer[..count])?;
        offset += count as u64;
    }
    if file.read_at(&mut [0], size)? != 0
        || fingerprint(file)? != before
        || format!("{:x}", hasher.finalize()) != expected.as_str()
    {
        return Err(GenerationError::PayloadMismatch);
    }
    if !boundary() {
        return Err(GenerationError::PublicationRejected);
    }
    Ok(())
}
pub(super) fn write_new_stream(
    parent: &File,
    digest: &Sha256,
    source: &File,
    size: u64,
    boundary: &mut impl FnMut() -> bool,
) -> Result<(), GenerationError> {
    let name = format!("{}.blob", digest.as_str());
    let mut output = File::from(openat(
        parent,
        name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )?);
    stream(
        source,
        size,
        digest,
        |bytes| output.write_all(bytes).map_err(GenerationError::Io),
        boundary,
    )?;
    fchmod(&output, Mode::from_raw_mode(0o400))?;
    output.sync_all()?;
    Ok(())
}
pub(super) fn open_verified_blob(
    parent: &File,
    size: u64,
    digest: &Sha256,
) -> Result<File, GenerationError> {
    let file = File::from(openat2(
        parent,
        format!("{}.blob", digest.as_str()).as_str(),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_XDEV,
    )?);
    let stat = rustix::fs::fstat(&file)?;
    let owner = rustix::fs::fstat(parent)?.st_uid;
    if stat.st_uid != owner || stat.st_mode & 0o7777 != 0o400 {
        return Err(GenerationError::InvalidEntryType);
    }
    verify_source(&file, size, digest)?;
    Ok(file)
}

impl GenerationStore {
    /// Attach exact captured assets to a native schema-2 preference manifest,
    /// stream the payloads, and publish one schema-3 sealed generation. Legacy
    /// manifests and publication retain their byte-for-byte serialization.
    /// Account binding/writer lease/native root checks remain the coordinator's
    /// responsibility; the live callback is checked throughout streaming.
    pub fn publish_with_assets(
        &self,
        mut manifest: RecoveryManifest,
        preferences: &Payloads,
        assets: &CapturedAssets,
        home: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<RecoveryManifest, GenerationError> {
        if !boundary() {
            return Err(GenerationError::PublicationRejected);
        }
        manifest.attach_assets(assets.inventory().clone())?;
        let mut payloads = preferences.clone();
        for (digest, bytes) in assets.metadata_payloads() {
            if let Some(existing) = payloads.insert(digest.clone(), bytes.clone())
                && existing != *bytes
            {
                return Err(GenerationError::PayloadMismatch);
            }
        }
        let mut streams =
            assets.open_payloads(home).map_err(|_| GenerationError::PayloadMismatch)?;
        for digest in payloads.keys() {
            if let Some(source) = streams.remove(digest) {
                verify_source(&source, payloads[digest].len() as u64, digest)?;
            }
        }
        manifest.seal()?;
        self.publish_streamed_checked(&manifest, &payloads, &streams, |full| {
            boundary() && (!full || assets.revalidate(home).is_ok())
        })?;
        Ok(manifest)
    }
}
impl VerifiedGeneration {
    /// Restore one exact, account-bound regular asset into a caller-owned,
    /// empty private staging inode. Copies and verifies with bounded memory,
    /// preserves the captured ownership/mode/time/ACL/label, and polls the
    /// caller's cancellation/authorization boundary between chunks. Failure
    /// leaves an unpublished staging inode which the caller must discard.
    ///
    /// This does not authorize a live replacement, publish a destination,
    /// establish a linked current-state checkpoint or advance a journal. The
    /// trusted coordinator must hold its writer lease and own the staging
    /// inode exclusively throughout this call and subsequent publication.
    pub fn prepare_asset_inode(
        &self,
        path: &crate::ryogami_assets::AssetPath,
        account: &crate::installed_capture::CaptureAccount,
        staging: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<(), GenerationError> {
        if !boundary() {
            return Err(GenerationError::PublicationRejected);
        }
        let inventory = self.manifest.assets().ok_or(GenerationError::PayloadMismatch)?;
        inventory.require_account(account).map_err(|_| GenerationError::PayloadMismatch)?;
        let record = inventory
            .entries()
            .iter()
            .find(|entry| entry.path == *path)
            .ok_or(GenerationError::PayloadMismatch)?;
        let EntryKind::RegularFile { metadata, size, sha256, .. } = &record.kind else {
            return Err(GenerationError::InvalidEntryType);
        };
        let stat = rustix::fs::fstat(staging)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_size != 0
            || stat.st_mode & 0o7777 != 0o600
            || (stat.st_uid != rustix::process::geteuid().as_raw() && stat.st_uid != account.uid)
        {
            return Err(GenerationError::InvalidEntryType);
        }
        let acl = metadata
            .acl_sha256
            .as_ref()
            .map(|digest| {
                crate::metadata::AclPayload::decode(&self.read_payload(digest)?)
                    .map_err(|_| GenerationError::PayloadMismatch)
            })
            .transpose()?;
        let mut xattrs = BTreeMap::new();
        for (name, digest) in &metadata.extended.values {
            xattrs.insert(*name, self.read_payload(digest)?);
        }
        let source = self.open_asset_payload(sha256)?;
        let mut offset = 0;
        stream(
            &source,
            *size,
            sha256,
            |bytes| {
                staging.write_all_at(bytes, offset)?;
                offset += bytes.len() as u64;
                Ok(())
            },
            &mut boundary,
        )?;
        staging.sync_all()?;
        stream(staging, *size, sha256, |_| Ok(()), &mut boundary)?;
        crate::inode_metadata::apply_verified_metadata(
            staging,
            metadata,
            false,
            acl.as_ref(),
            xattrs.iter().map(|(name, value)| (*name, value.as_slice())).collect(),
        )
        .map_err(GenerationError::InodeMetadata)?;
        if !boundary() {
            return Err(GenerationError::PublicationRejected);
        }
        Ok(())
    }

    /// Restore metadata on an already-created owned tree directory after its
    /// children are complete. The caller retains its protected tree journal.
    pub(crate) fn prepare_asset_directory_metadata(
        &self,
        path: &crate::ryogami_assets::AssetPath,
        account: &crate::installed_capture::CaptureAccount,
        directory: &File,
    ) -> Result<(), GenerationError> {
        let assets = self.manifest.assets().ok_or(GenerationError::PayloadMismatch)?;
        assets.require_account(account).map_err(|_| GenerationError::PayloadMismatch)?;
        let record = assets
            .entries()
            .iter()
            .find(|record| record.path == *path)
            .ok_or(GenerationError::PayloadMismatch)?;
        let EntryKind::Directory { metadata } = &record.kind else {
            return Err(GenerationError::InvalidEntryType);
        };
        let stat = rustix::fs::fstat(directory)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory
            || stat.st_uid != account.uid
            || stat.st_gid != account.gid
        {
            return Err(GenerationError::InvalidEntryType);
        }
        let acl = metadata
            .acl_sha256
            .as_ref()
            .map(|digest| {
                crate::metadata::AclPayload::decode(&self.read_payload(digest)?)
                    .map_err(|_| GenerationError::PayloadMismatch)
            })
            .transpose()?;
        let mut xattrs = BTreeMap::new();
        for (name, digest) in &metadata.extended.values {
            xattrs.insert(*name, self.read_payload(digest)?);
        }
        crate::inode_metadata::apply_verified_metadata(
            directory,
            metadata,
            true,
            acl.as_ref(),
            xattrs.iter().map(|(name, value)| (*name, value.as_slice())).collect(),
        )
        .map_err(GenerationError::InodeMetadata)
    }

    /// Provides only an independently verified immutable referenced asset;
    /// restore code can copy it in bounded chunks without a payload-sized Vec.
    pub fn open_asset_payload(&self, digest: &Sha256) -> Result<File, GenerationError> {
        if !self.manifest.assets().is_some_and(|assets| {
            assets.entries().iter().any(
                |r| matches!(&r.kind, EntryKind::RegularFile { sha256, .. } if sha256 == digest),
            )
        }) {
            return Err(GenerationError::PayloadMismatch);
        }
        open_verified_blob(&self.payloads, content_size(&self.manifest, digest)?, digest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccountPrincipal, EntryMetadata, ExtendedMetadata, FileRecord, OperationReason,
        RelativePath, SnapshotLink, StateRoot, VerificationStatus,
        installed_capture::CaptureAccount,
        ryogami_assets::{AssetNamespace, AssetPath, capture_assets},
    };
    use std::{
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        path: PathBuf,
        home: File,
        store: GenerationStore,
        account: CaptureAccount,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ryogami-generation-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            for sub in ["home", "state"] {
                std::fs::create_dir(path.join(sub)).unwrap();
                std::fs::set_permissions(path.join(sub), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            let uid = rustix::process::geteuid().as_raw();
            let gid = rustix::process::getegid().as_raw();
            let home = File::open(path.join("home")).unwrap();
            let account = CaptureAccount {
                uid,
                gid,
                principal: AccountPrincipal::from_account(
                    uid,
                    gid,
                    "fixture",
                    path.join("home").to_str().unwrap(),
                )
                .unwrap(),
            };
            let store =
                GenerationStore::open_owned(&File::open(path.join("state")).unwrap(), uid).unwrap();
            Self { path, home, store, account }
        }
        fn assets(&self, size: u64) -> CapturedAssets {
            let declared = AssetPath::new(
                AssetNamespace::SemanticModels,
                RelativePath::new("fixture/model.onnx").unwrap(),
            )
            .unwrap();
            let file = self.path.join("home").join(declared.home_relative().unwrap().as_str());
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            let data = File::create(&file).unwrap();
            data.set_len(size).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
            capture_assets(&self.home, &self.account, &[declared], MAX_CAPTURE_LIMITS).unwrap()
        }
        fn preferences(&self) -> (RecoveryManifest, Payloads) {
            let link = SnapshotLink::new(
                Identifier::new("root").unwrap(),
                "42",
                "11111111-1111-1111-1111-111111111111",
            )
            .unwrap();
            let receipt = serde_json::from_value(serde_json::json!({"schema":1,"link":link,"subvolume_id":300,"subvolume_uuid":"22222222-2222-2222-2222-222222222222","origin_root_uuid":"33333333-3333-3333-3333-333333333333"})).unwrap();
            let mut manifest = RecoveryManifest::new(
                Identifier::new("asset-test-transaction").unwrap(),
                Identifier::new("asset-test-generation").unwrap(),
                None,
                link,
                OperationReason::ProviderChange,
                "2026-10-25T12:00:00Z",
            )
            .unwrap();
            manifest.attach_root_snapshot_receipt(receipt).unwrap();
            manifest.attach_account_principal(self.account.principal.clone());
            let bytes = b"preference fixture".to_vec();
            let digest = digest_bytes(&bytes);
            manifest
                .add_file(
                    FileRecord::new(
                        StateRoot::UserRyokuConfig,
                        RelativePath::new("profile.json").unwrap(),
                        EntryKind::RegularFile {
                            metadata: EntryMetadata {
                                mode: 0o600,
                                uid: self.account.uid,
                                gid: self.account.gid,
                                mtime_unix_ns: 1,
                                acl_sha256: None,
                                extended: ExtendedMetadata::default(),
                            },
                            size: bytes.len() as u64,
                            sha256: digest.clone(),
                            hard_link_group: None,
                        },
                    )
                    .unwrap(),
                )
                .unwrap();
            manifest.set_verification(
                VerificationStatus::Verified,
                VerificationStatus::Verified,
                VerificationStatus::Verified,
            );
            manifest.seal().unwrap();
            (manifest, Payloads::from([(digest, bytes)]))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Sealed fixture directories are deliberately immutable to normal
            // writers. Only this disposable test owner relaxes them to clean up.
            fn writable(path: &std::path::Path) {
                if path.is_dir() {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
                    for entry in std::fs::read_dir(path).unwrap() {
                        writable(&entry.unwrap().path());
                    }
                }
            }
            writable(&self.path);
            std::fs::remove_dir_all(&self.path).unwrap();
        }
    }
    #[test]
    fn schema3_streams_large_assets_and_verifies_both_payload_families() {
        let fixture = Fixture::new();
        let size = MAX_GENERATION_FILE_BYTES + 1;
        let assets = fixture.assets(size);
        let (base, preferences) = fixture.preferences();
        let legacy = base.canonical_bytes().unwrap();
        assert!(!String::from_utf8_lossy(&legacy).contains("\"assets\""));
        let manifest = fixture
            .store
            .publish_with_assets(base, &preferences, &assets, &fixture.home, || true)
            .unwrap();
        assert_eq!(manifest.schema_version, 3);
        assert_eq!(manifest.assets().unwrap(), assets.inventory());
        assert_eq!(catalog_schema(&manifest), 4);
        let verified = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
        for (digest, expected) in preferences {
            assert_eq!(verified.read_payload(&digest).unwrap(), expected);
        }
        let EntryKind::RegularFile { sha256, .. } = &assets.inventory().entries()[0].kind else {
            panic!("expected payload")
        };
        let file = verified.open_asset_payload(sha256).unwrap();
        assert_eq!(file.metadata().unwrap().len(), size);
        assert!(
            verified.read_payload(sha256).is_err(),
            "large assets must never allocate a payload-sized Vec"
        );
        assert!(verified.open_asset_payload(&digest_bytes(b"unreferenced")).is_err());
    }
    #[test]
    fn streamed_restore_preserves_asset_bytes_and_metadata_without_legacy_stage() {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let fixture = Fixture::new();
        let assets = fixture.assets(MAX_GENERATION_FILE_BYTES + 1);
        let (base, preferences) = fixture.preferences();
        let manifest = fixture
            .store
            .publish_with_assets(base, &preferences, &assets, &fixture.home, || true)
            .unwrap();
        let verified = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
        let target = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(fixture.path.join("staged-model"))
            .unwrap();
        let record = &assets.inventory().entries()[0];
        verified.prepare_asset_inode(&record.path, &fixture.account, &target, || true).unwrap();
        let EntryKind::RegularFile { metadata, size, sha256, .. } = &record.kind else {
            panic!("file")
        };
        verify_source(&target, *size, sha256).unwrap();
        let actual = target.metadata().unwrap();
        assert_eq!(
            (actual.uid(), actual.gid(), actual.mode() & 0o7777),
            (metadata.uid, metadata.gid, metadata.mode)
        );
        assert_eq!(
            i128::from(actual.mtime()) * 1_000_000_000 + i128::from(actual.mtime_nsec()),
            metadata.mtime_unix_ns
        );
        let plan = crate::restore::RestorePlan {
            source_manifest_sha256: manifest.manifest_sha256.clone().unwrap(),
            transaction_id: manifest.transaction_id.clone(),
            generation_id: manifest.generation_id.clone(),
            snapper_config: manifest.snapper_config.clone(),
            snapshot_id: manifest.snapper_snapshot_id.clone(),
            filesystem_uuid: manifest.filesystem_uuid.clone(),
            scope: crate::restore::RestoreScope::ConfigurationOnly,
            configuration_entries: 1,
        };
        assert!(matches!(
            crate::configuration_stage::stage_configuration(&verified, &plan),
            Err(crate::configuration_stage::StageError::AssetRestoreRequired)
        ));
        let evidence = crate::restore::RestoreSourceEvidence {
            generation: crate::restore::ArtifactStatus::Verified,
            archive: crate::restore::ArtifactStatus::Verified,
            system_snapshot: crate::restore::SystemSnapshotStatus::Available {
                snapper_config: manifest.snapper_config.clone(),
                snapshot_id: manifest.snapper_snapshot_id.clone(),
                filesystem_uuid: manifest.filesystem_uuid.clone(),
            },
            system_compatibility: crate::restore::CompatibilityStatus::Compatible,
            configuration_compatibility: crate::restore::CompatibilityStatus::Compatible,
        };
        for scope in [
            crate::restore::RestoreScope::ConfigurationOnly,
            crate::restore::RestoreScope::SystemOnly,
            crate::restore::RestoreScope::CompleteLinked,
        ] {
            assert!(matches!(
                crate::restore::plan_restore_source(&manifest, scope, &evidence),
                Err(crate::restore::PreflightError::AssetRestoreRequired)
            ));
        }
    }

    #[test]
    fn asset_restore_refuses_wrong_account_path_existing_inode_and_cancels() {
        use std::os::unix::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let assets = fixture.assets(256 * 1024);
        let (base, preferences) = fixture.preferences();
        let manifest = fixture
            .store
            .publish_with_assets(base, &preferences, &assets, &fixture.home, || true)
            .unwrap();
        let verified = fixture.store.open_verified_transaction(&manifest.transaction_id).unwrap();
        let target = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(fixture.path.join("staged-model"))
            .unwrap();
        let path = &assets.inventory().entries()[0].path;
        let wrong = CaptureAccount {
            principal: AccountPrincipal::from_account(
                fixture.account.uid,
                fixture.account.gid,
                "other",
                "/home/other",
            )
            .unwrap(),
            ..fixture.account.clone()
        };
        assert!(verified.prepare_asset_inode(path, &wrong, &target, || true).is_err());
        let absent = AssetPath::new(
            AssetNamespace::SemanticModels,
            RelativePath::new("unrelated.onnx").unwrap(),
        )
        .unwrap();
        assert!(verified.prepare_asset_inode(&absent, &fixture.account, &target, || true).is_err());
        assert_eq!(target.metadata().unwrap().len(), 0);
        let mut calls = 0;
        assert!(matches!(
            verified.prepare_asset_inode(path, &fixture.account, &target, || {
                calls += 1;
                calls < 3
            }),
            Err(GenerationError::PublicationRejected)
        ));
        assert!(target.metadata().unwrap().len() < 256 * 1024);
        assert!(
            verified.prepare_asset_inode(path, &fixture.account, &target, || true).is_err(),
            "an interrupted inode must be discarded, never appended/reused"
        );
        assert!(fixture.store.verify_transaction(&manifest.transaction_id).is_ok());
        assert_eq!(
            std::fs::metadata(
                fixture.path.join("home/.local/share/ryogami/lens/models/fixture/model.onnx")
            )
            .unwrap()
            .len(),
            256 * 1024
        );
    }

    #[test]
    fn missing_account_link_or_changed_asset_prevents_publication() {
        for changed in [false, true] {
            let fixture = Fixture::new();
            let assets = fixture.assets(64);
            let (mut base, preferences) = fixture.preferences();
            if changed {
                std::fs::write(
                    fixture.path.join("home/.local/share/ryogami/lens/models/fixture/model.onnx"),
                    b"changed",
                )
                .unwrap();
            } else {
                base.attach_account_principal(
                    AccountPrincipal::from_account(
                        fixture.account.uid,
                        fixture.account.gid,
                        "another",
                        "/home/another",
                    )
                    .unwrap(),
                );
            }
            assert!(
                fixture
                    .store
                    .publish_with_assets(base.clone(), &preferences, &assets, &fixture.home, || {
                        true
                    })
                    .is_err()
            );
            assert!(fixture.store.verify_transaction(&base.transaction_id).is_err());
        }
    }
    #[test]
    fn interruption_during_stream_never_publishes_a_partial_generation() {
        let fixture = Fixture::new();
        let assets = fixture.assets(128 * 1024);
        let (base, preferences) = fixture.preferences();
        let mut calls = 0;
        let result = fixture.store.publish_with_assets(
            base.clone(),
            &preferences,
            &assets,
            &fixture.home,
            || {
                calls += 1;
                calls < 4
            },
        );
        assert!(matches!(result, Err(GenerationError::PublicationRejected)));
        assert!(fixture.store.verify_transaction(&base.transaction_id).is_err());
        assert_eq!(
            std::fs::metadata(
                fixture.path.join("home/.local/share/ryogami/lens/models/fixture/model.onnx")
            )
            .unwrap()
            .len(),
            128 * 1024
        );
    }
    #[test]
    fn legacy_schema2_generation_retains_canonical_bytes_and_limits() {
        let fixture = Fixture::new();
        let (base, preferences) = fixture.preferences();
        let canonical = base.canonical_bytes().unwrap();
        fixture.store.publish(&base, &preferences).unwrap();
        assert_eq!(
            fixture
                .store
                .verify_transaction(&base.transaction_id)
                .unwrap()
                .canonical_bytes()
                .unwrap(),
            canonical
        );
        assert_eq!(catalog_schema(&base), 3);
        assert!(matches!(
            fixture.store.ensure_available_space(MAX_GENERATION_BYTES + 1, 1),
            Err(GenerationError::InventoryTooLarge)
        ));
    }
}
