//! Disabled production integration for the exact native personal-profile scope.
//!
//! No socket dispatch calls this module. A scoped checkpoint is not complete
//! Ryoku recovery, boot compatibility, or a permit for unrelated mutations.
//! The concrete durable coordinator retains the common writer guard across
//! these calls, records its phases, and requires post-change capture. External
//! editors/package tools are not stopped by our advisory lock.
use crate::{
    PeerIdentity, ServiceError,
    account_state::AccountStateRoot,
    helper_protocol::prepare_operator_account,
    mutation_gate::{MutationLock, WriterGuard},
};
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, ResolveFlags};
use ryoku_profiles::{ProfileDocument, ProfileId, ProfileOwner, SettingsLayer};
use ryoku_recovery::{
    AccountPrincipal, EntryKind, FileRecord, Identifier, OperationReason, PackageRevision,
    RecoveryManifest, RelativePath, Sha256, SnapshotLink, StateRoot,
    capture::{CaptureLimits, capture_declared},
    generation::GenerationStore,
    generation_capture::GenerationCaptureError,
    installed_capture::{
        InstalledCaptureRequest, InstalledInventory, InstalledRoots,
        capture_with_installed_profile_bundle_guarded,
    },
    installed_profiles::{InstalledProfileBundle, ProfileBundleStore, VerifiedProfileBundle},
    linked_checkpoint::HistoricalLinkedGeneration,
    machine_state::read_machine_state_record,
    native_checkpoint::verify_historical_root_checkpoint,
    root_capture_mapping::RootCaptureMapping,
    state_mount::collect_and_open_state_mount,
};
use ryoku_snapper::{ConfigName, SnapperCheckpointBackend, SnapshotPairRequest, TransactionId};
use sha2::{Digest, Sha256 as Hash};
use std::{fs::File, io::Write};

#[path = "personal_profile_coordination.rs"]
mod coordination;
#[path = "personal_profile_label.rs"]
mod label;
pub use coordination::PersonalProfileCompletion;

const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const PROFILE: &str = ryoku_profiles::LOCAL_PROFILE_FILE;
const LIMITS: CaptureLimits = CaptureLimits {
    max_paths: 1,
    max_file_bytes: ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES as u64,
    max_total_bytes: 512 * 1024,
};
type Result<T> = std::result::Result<T, ServiceError>;
fn unavailable<T>(_: T) -> ServiceError {
    ServiceError::RecoveryUnavailable
}

/// A bounded, canonical personal change prepared from authenticated current
/// state and actual installed RPM defaults. Fields are never client-authored.
pub struct PersonalProfilePlan {
    original: FileRecord,
    defaults: InstalledProfileBundle,
    desired: Vec<u8>,
    principal: AccountPrincipal,
}

/// Native receipt + complete exact personal-profile scope + immutable bundle.
/// This type intentionally has no conversion to the generic #20 boolean proof.
pub struct VerifiedPersonalProfileCheckpoint {
    linked: HistoricalLinkedGeneration,
    bundle: VerifiedProfileBundle,
    desired: Sha256,
    principal: AccountPrincipal,
    mapping: RootCaptureMapping,
}
impl VerifiedPersonalProfileCheckpoint {
    pub fn manifest(&self) -> &RecoveryManifest {
        self.linked.generation().manifest()
    }
}

/// One authorized account and fixed protected state namespace. No client may
/// supply a UID, home, arbitrary root descriptor, bundle digest or path list.
pub struct PersonalProfileCoordinator {
    account: AccountStateRoot,
    state: File,
    lock: MutationLock,
    store: GenerationStore,
    bundles: ProfileBundleStore,
    filesystem_uuid: String,
}
impl PersonalProfileCoordinator {
    pub fn open(peer: &PeerIdentity, operator_gid: u32) -> Result<Self> {
        // Authorization precedes account lookup and protected-state access.
        let account = prepare_operator_account(peer, operator_gid)?;
        if rustix::process::geteuid().as_raw() != 0 || rustix::process::getegid().as_raw() != 0 {
            return Err(ServiceError::Unauthorized);
        }
        let root = File::open("/").map_err(unavailable)?;
        let record = read_machine_state_record(&root).map_err(unavailable)?;
        let state = File::from(
            collect_and_open_state_mount(&root, record.identity()).map_err(unavailable)?.0,
        );
        let store = GenerationStore::open_initialized_state_mount(record.identity())
            .map_err(unavailable)?;
        // Readers never provision missing bundle storage.
        let bundles = ProfileBundleStore::open_state_mount().map_err(unavailable)?;
        let lock =
            MutationLock::open(state.try_clone().map_err(unavailable)?, 0).map_err(unavailable)?;
        Ok(Self {
            account,
            state,
            lock,
            store,
            bundles,
            filesystem_uuid: record.identity().filesystem_uuid.clone(),
        })
    }
    /// Acquire once in the owning coordinator. Never call coordinate_mutation
    /// recursively while holding this same lock. Its guard is borrowed below.
    pub fn writer_lock(&self) -> &MutationLock {
        &self.lock
    }

    fn revalidate(&self, guard: &WriterGuard) -> Result<()> {
        self.account.revalidate().map_err(unavailable)?;
        let root = File::open("/").map_err(unavailable)?;
        let record = read_machine_state_record(&root).map_err(unavailable)?;
        let state = File::from(
            collect_and_open_state_mount(&root, record.identity()).map_err(unavailable)?.0,
        );
        same_inode(&self.state, &state)?;
        self.lock.revalidate_guard(guard).map_err(unavailable)?;
        let stat =
            fs::statat(&state, "mutation.lock", AtFlags::SYMLINK_NOFOLLOW).map_err(unavailable)?;
        if stat.st_gid != 0 {
            return Err(ServiceError::RecoveryUnavailable);
        }
        Ok(())
    }

    pub fn prepare(
        &self,
        guard: &WriterGuard,
        settings: SettingsLayer,
    ) -> Result<PersonalProfilePlan> {
        self.revalidate(guard)?;
        let principal = self.account.principal().map_err(unavailable)?;
        let owner = self.owner();
        let original = read_personal(self.account.configuration(), owner)?;
        let defaults =
            InstalledProfileBundle::load_packaged_defaults(None, None).map_err(unavailable)?;
        validate_generic_defaults(&defaults)?;
        let desired = ProfileDocument::new(
            ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).map_err(unavailable)?,
            settings,
        )
        .canonical_bytes()
        .map_err(unavailable)?;
        let plan = PersonalProfilePlan { original, defaults, desired, principal };
        self.revalidate_plan(guard, &plan)?;
        Ok(plan)
    }

    fn owner(&self) -> ProfileOwner {
        ProfileOwner { uid: self.account.uid(), gid: self.account.primary_gid() }
    }
    fn revalidate_plan(&self, guard: &WriterGuard, plan: &PersonalProfilePlan) -> Result<()> {
        self.revalidate(guard)?;
        if self.account.principal().map_err(unavailable)? != plan.principal
            || read_personal(self.account.configuration(), self.owner())? != plan.original
            || InstalledProfileBundle::load_packaged_defaults(None, None).map_err(unavailable)?
                != plan.defaults
        {
            return Err(ServiceError::StaleRequest);
        }
        Ok(())
    }

    /// Create the native checkpoint while this writer guard is held, capture
    /// every record in the explicit personal scope, then reopen all partners.
    /// IDs/timestamp are trusted coordinator facts; they select no source path.
    pub fn capture(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        transaction: Identifier,
        generation: Identifier,
        created_utc: String,
    ) -> Result<VerifiedPersonalProfileCheckpoint> {
        self.capture_scoped(guard, plan, transaction, generation, None, created_utc)
    }

    fn capture_scoped(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        transaction: Identifier,
        generation: Identifier,
        parent_generation: Option<Identifier>,
        created_utc: String,
    ) -> Result<VerifiedPersonalProfileCheckpoint> {
        self.revalidate_plan(guard, plan)?;
        let mapping = RootCaptureMapping::open(&self.filesystem_uuid).map_err(unavailable)?;
        let bundle =
            self.bundles.publish_candidate(&transaction, &plan.defaults).map_err(unavailable)?;
        let root = File::open("/").map_err(unavailable)?;
        let request = SnapshotPairRequest {
            config: ConfigName::new("root").map_err(unavailable)?,
            transaction_id: TransactionId::new(transaction.as_str()).map_err(unavailable)?,
        };
        mapping.revalidate().map_err(unavailable)?;
        let snapshot = SnapperCheckpointBackend.create_single(&request).map_err(unavailable)?;
        let native = mapping
            .verify_created(
                SnapshotLink::new(
                    Identifier::new("root").map_err(unavailable)?,
                    snapshot.as_str(),
                    &self.filesystem_uuid,
                )
                .map_err(unavailable)?,
            )
            .map_err(unavailable)?;
        let inventory = InstalledInventory::personal_profile_writer().map_err(unavailable)?;
        let package = plan.defaults.package();
        let manifest = capture_with_installed_profile_bundle_guarded(
            &self.store,
            &inventory,
            InstalledRoots {
                user_configuration: self.account.configuration(),
                system_configuration: None,
                account: self.account.capture_account().map_err(unavailable)?,
            },
            InstalledCaptureRequest {
                transaction_id: transaction.clone(),
                generation_id: generation,
                parent_generation_id: parent_generation,
                snapshot: native.verified_link().clone(),
                reason: OperationReason::ProfileApply,
                created_utc,
                package_revisions: vec![PackageRevision {
                    package_name: package.name.clone(),
                    before_nevra: Some(package.nevra.clone()),
                    after_nevra: None,
                    upstream_revision: package.upstream_revision.clone(),
                }],
                provider_revision: None,
                limits: LIMITS,
            },
            &bundle,
            || {
                mapping.revalidate().map_err(|_| GenerationCaptureError::VerificationMismatch)?;
                self.revalidate_plan(guard, plan)
                    .map_err(|_| GenerationCaptureError::VerificationMismatch)?;
                verify_historical_root_checkpoint(&root, native.receipt())
                    .map(|_| ())
                    .map_err(|_| GenerationCaptureError::VerificationMismatch)
            },
        )
        .map_err(unavailable)?;
        if manifest.root_snapshot_receipt() != Some(native.receipt()) {
            return Err(ServiceError::RecoveryUnavailable);
        }
        let linked =
            self.store.open_historical_transaction(&root, &transaction).map_err(unavailable)?;
        validate_scoped_source(linked.generation().manifest(), plan)?;
        self.bundles.resolve_generation(linked.generation()).map_err(unavailable)?;
        self.revalidate_plan(guard, plan)?;
        Ok(VerifiedPersonalProfileCheckpoint {
            linked,
            bundle,
            desired: digest(&plan.desired),
            principal: plan.principal.clone(),
            mapping,
        })
    }

    /// This actual writer is deliberately inaccessible to transport callers.
    /// The concrete coordinator journals Applying before invoking this method,
    /// then checks the actual resulting file and captures a fresh postimage.
    fn apply(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        checkpoint: &VerifiedPersonalProfileCheckpoint,
    ) -> Result<FileRecord> {
        let current = self.revalidate_checkpoint(guard, plan, checkpoint)?;
        let applied = write_personal(
            self.account.configuration(),
            self.owner(),
            plan,
            &current.generation().manifest().transaction_id,
            |hash| current.generation().read_payload(hash).map_err(unavailable),
            |staged| {
                self.revalidate_checkpoint(guard, plan, checkpoint)?;
                if matches!(plan.original.kind, EntryKind::Absent)
                    && let Some(staged) = staged
                {
                    label::validate_new_profile_label(&self.account, staged)?;
                }
                Ok(())
            },
        )?;
        // A failed post-write check is an Applying failure for the durable
        // coordinator, never an assertion that nothing changed.
        self.revalidate(guard)?;
        if InstalledProfileBundle::load_packaged_defaults(None, None).map_err(unavailable)?
            != plan.defaults
        {
            return Err(ServiceError::StaleRequest);
        }
        checkpoint.bundle.revalidate().map_err(unavailable)?;
        Ok(applied)
    }

    fn revalidate_checkpoint(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        checkpoint: &VerifiedPersonalProfileCheckpoint,
    ) -> Result<HistoricalLinkedGeneration> {
        self.revalidate_plan(guard, plan)?;
        self.revalidate_retained_checkpoint(guard, plan, checkpoint)
    }

    /// After Applying the live source differs from the saved preimage. Reopen
    /// its exact native and bundle partners without treating live state as old.
    fn revalidate_retained_checkpoint(
        &self,
        guard: &WriterGuard,
        plan: &PersonalProfilePlan,
        checkpoint: &VerifiedPersonalProfileCheckpoint,
    ) -> Result<HistoricalLinkedGeneration> {
        self.revalidate(guard)?;
        if self.account.principal().map_err(unavailable)? != plan.principal
            || InstalledProfileBundle::load_packaged_defaults(None, None).map_err(unavailable)?
                != plan.defaults
        {
            return Err(ServiceError::StaleRequest);
        }
        if checkpoint.desired != digest(&plan.desired)
            || checkpoint.principal != plan.principal
            || checkpoint.bundle.bundle() != &plan.defaults
        {
            return Err(ServiceError::StaleRequest);
        }
        checkpoint.mapping.revalidate().map_err(unavailable)?;
        let root = File::open("/").map_err(unavailable)?;
        let current = self
            .store
            .open_historical_transaction(&root, &checkpoint.manifest().transaction_id)
            .map_err(unavailable)?;
        if current.generation().manifest() != checkpoint.manifest() {
            return Err(ServiceError::RecoveryUnavailable);
        }
        validate_scoped_source(current.generation().manifest(), plan)?;
        checkpoint.bundle.revalidate().map_err(unavailable)?;
        self.bundles.resolve_generation(current.generation()).map_err(unavailable)?;
        Ok(current)
    }
}

fn validate_generic_defaults(bundle: &InstalledProfileBundle) -> Result<()> {
    let defaults = ProfileDocument::parse(bundle.defaults_source_bytes()).map_err(unavailable)?;
    if defaults.id.as_str() != ryoku_profiles::GENERIC_PROFILE_ID
        || bundle.system_source_bytes().is_some()
        || bundle.companion_source_bytes().is_some()
    {
        return Err(ServiceError::Unsupported);
    }
    Ok(())
}
fn validate_scoped_source(manifest: &RecoveryManifest, plan: &PersonalProfilePlan) -> Result<()> {
    if manifest.files() != [plan.original.clone()]
        || manifest.account_principal() != Some(&plan.principal)
        || !manifest.is_mutation_ready()
        || manifest.root_snapshot_receipt().is_none()
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    Ok(())
}
fn same_inode(old: &File, new: &File) -> Result<()> {
    let old = fs::fstat(old).map_err(unavailable)?;
    let new = fs::fstat(new).map_err(unavailable)?;
    if old.st_dev != new.st_dev || old.st_ino != new.st_ino {
        return Err(ServiceError::StaleRequest);
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> Sha256 {
    Sha256::new(Hash::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>())
        .expect("SHA256 hex")
}
fn record(root: &File) -> Result<FileRecord> {
    let mut records = capture_declared(
        root,
        StateRoot::UserRyokuConfig,
        &[RelativePath::new(PROFILE).map_err(unavailable)?],
        LIMITS,
    )
    .map_err(unavailable)?;
    records.pop().ok_or(ServiceError::RecoveryUnavailable)
}
fn read_personal(root: &File, owner: ProfileOwner) -> Result<FileRecord> {
    let before = record(root)?;
    match &before.kind {
        EntryKind::Absent => {}
        EntryKind::RegularFile { metadata, hard_link_group, .. }
            if metadata.uid == owner.uid
                && metadata.gid == owner.gid
                && ryoku_profiles::PERSONAL_PROFILE_MODES.contains(&metadata.mode)
                && hard_link_group.is_none() =>
        {
            let document = ryoku_profiles::load_local_profile(root, owner).map_err(unavailable)?;
            if document.id.as_str() != ryoku_profiles::GENERIC_PROFILE_ID {
                return Err(ServiceError::Unsupported);
            }
            // Atomic replacement must not silently discard an unreviewed
            // attribute that this writer cannot reproduce from its capture.
            let file = File::from(
                fs::openat2(
                    root,
                    PROFILE,
                    OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                    Mode::empty(),
                    RESOLVE,
                )
                .map_err(unavailable)?,
            );
            validate_attributes(&file)?;
        }
        _ => return Err(ServiceError::RecoveryUnavailable),
    }
    if before != record(root)? {
        return Err(ServiceError::StaleRequest);
    }
    Ok(before)
}

fn validate_attributes(file: &File) -> Result<()> {
    let mut names = vec![0_u8; 64 * 1024];
    let length = fs::flistxattr(file, &mut names).map_err(unavailable)?;
    if names[..length]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .any(|name| !matches!(name, b"system.posix_acl_access" | b"security.selinux"))
    {
        return Err(ServiceError::Unsupported);
    }
    Ok(())
}

// The only filesystem mutation implementation for this enrolled writer.
// All source metadata comes from the exact persisted preimage, never a client.
fn write_personal(
    root: &File,
    owner: ProfileOwner,
    plan: &PersonalProfilePlan,
    transaction: &Identifier,
    read_payload: impl Fn(&Sha256) -> Result<Vec<u8>>,
    revalidate: impl Fn(Option<&File>) -> Result<()>,
) -> Result<FileRecord> {
    revalidate(None)?;
    let stage_name = format!(".ryoku-profile-{}.pending", transaction.as_str());
    let mut staged = File::from(
        fs::openat2(
            root,
            &stage_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::from_raw_mode(0o600),
            RESOLVE,
        )
        .map_err(unavailable)?,
    );
    staged.write_all(&plan.desired).map_err(unavailable)?;
    prepare_metadata(&staged, owner, &plan.original, &read_payload)?;
    fs::fsync(&staged).map_err(unavailable)?;
    revalidate(Some(&staged))?;
    if read_personal(root, owner)? != plan.original {
        return Err(ServiceError::StaleRequest);
    }
    let named = File::from(
        fs::openat2(
            root,
            &stage_name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            RESOLVE,
        )
        .map_err(unavailable)?,
    );
    same_inode(&staged, &named)?;
    validate_attributes(&staged)?;
    let staged_record = capture_declared(
        root,
        StateRoot::UserRyokuConfig,
        &[RelativePath::new(&stage_name).map_err(unavailable)?],
        LIMITS,
    )
    .map_err(unavailable)?;
    let Some(FileRecord { kind: EntryKind::RegularFile { sha256, metadata, .. }, .. }) =
        staged_record.first()
    else {
        return Err(ServiceError::RecoveryUnavailable);
    };
    if *sha256 != digest(&plan.desired)
        || metadata.uid != owner.uid
        || metadata.gid != owner.gid
        || metadata.mode
            != match &plan.original.kind {
                EntryKind::RegularFile { metadata, .. } => metadata.mode,
                _ => ryoku_profiles::PERSONAL_PROFILE_CREATE_MODE,
            }
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    if let EntryKind::RegularFile { metadata: old, .. } = &plan.original.kind
        && (metadata.acl_sha256 != old.acl_sha256 || metadata.extended != old.extended)
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    let before = fs::fstat(&staged).map_err(unavailable)?;
    if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
        || before.st_nlink != 1
        || before.st_uid != owner.uid
        || before.st_gid != owner.gid
    {
        return Err(ServiceError::RecoveryUnavailable);
    }
    fs::renameat_with(
        root,
        &stage_name,
        root,
        PROFILE,
        if matches!(plan.original.kind, EntryKind::Absent) {
            fs::RenameFlags::NOREPLACE
        } else {
            fs::RenameFlags::empty()
        },
    )
    .map_err(unavailable)?;
    fs::fsync(root).map_err(unavailable)?;
    let after = read_personal(root, owner)?;
    match &after.kind {
        EntryKind::RegularFile { sha256, metadata, .. } if *sha256 == digest(&plan.desired) => {
            if let EntryKind::RegularFile { metadata: original, .. } = &plan.original.kind
                && (metadata.uid != original.uid
                    || metadata.gid != original.gid
                    || metadata.mode != original.mode
                    || metadata.acl_sha256 != original.acl_sha256
                    || metadata.extended != original.extended)
            {
                return Err(ServiceError::RecoveryUnavailable);
            }
        }
        _ => return Err(ServiceError::RecoveryUnavailable),
    }
    // Preserve the exact post-write observation for health checking and the
    // post-change capture plan; neither may silently recapture another edit.
    Ok(after)
}

fn prepare_metadata(
    file: &File,
    owner: ProfileOwner,
    original: &FileRecord,
    read_payload: &impl Fn(&Sha256) -> Result<Vec<u8>>,
) -> Result<()> {
    let stat = fs::fstat(file).map_err(unavailable)?;
    if stat.st_uid != owner.uid || stat.st_gid != owner.gid {
        fs::fchown(file, Some(fs::Uid::from_raw(owner.uid)), Some(fs::Gid::from_raw(owner.gid)))
            .map_err(unavailable)?;
    }
    match &original.kind {
        EntryKind::Absent => {
            fs::fchmod(file, Mode::from_raw_mode(ryoku_profiles::PERSONAL_PROFILE_CREATE_MODE))
                .map_err(unavailable)?
        }
        EntryKind::RegularFile { metadata, .. } => {
            fs::fchmod(file, Mode::from_raw_mode(metadata.mode)).map_err(unavailable)?;
            let acl = metadata
                .acl_sha256
                .as_ref()
                .map(|hash| {
                    read_payload(hash).and_then(|bytes| {
                        ryoku_recovery::metadata::AclPayload::decode(&bytes).map_err(unavailable)
                    })
                })
                .transpose()?;
            if acl.as_ref().is_some_and(|acl| acl.default.is_some()) {
                return Err(ServiceError::RecoveryUnavailable);
            }
            replace_xattr(
                file,
                "system.posix_acl_access",
                acl.as_ref().and_then(|acl| acl.access.as_deref()),
            )?;
            let label = metadata
                .extended
                .values
                .get(&ryoku_recovery::XattrName::SelinuxLabel)
                .map(read_payload)
                .transpose()?;
            replace_xattr(file, "security.selinux", label.as_deref())?;
        }
        _ => return Err(ServiceError::RecoveryUnavailable),
    }
    Ok(())
}
fn replace_xattr(file: &File, name: &str, value: Option<&[u8]>) -> Result<()> {
    match value {
        Some(value) => {
            fs::fsetxattr(file, name, value, fs::XattrFlags::empty()).map_err(unavailable)?
        }
        None => {
            let mut value = [0_u8; 64 * 1024];
            match fs::fgetxattr(file, name, &mut value) {
                Ok(_) => fs::fremovexattr(file, name).map_err(unavailable)?,
                Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP) => {}
                Err(_) => return Err(ServiceError::RecoveryUnavailable),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "personal_profile_tests.rs"]
mod tests;
