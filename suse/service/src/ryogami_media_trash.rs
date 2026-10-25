//! Protected write-ahead receipt plus exact account-owned file moves. A local
//! receipt is not completion: restore additionally requires the native linked
//! operation's successful protected history and independently checked account.
use super::*;
use crate::{
    OperationId, OperationJournal, OperationPhase,
    mutation_gate::{LinkedGeneration, MutationStage, VerifiedLinkedGeneration},
};
use std::io::Write;
const MAX_RECEIPT: usize = 256 * 1024;
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u16,
    operation: OperationId,
    pre: LinkedGeneration,
    selection: CatalogCommit,
    row: serde_json::Value,
    inventory: AssetInventory,
    moves: Vec<MediaMove>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrashReceiptIdentity {
    token: Sha256,
    sha256: Sha256,
}
impl TrashReceiptIdentity {
    pub fn parse(value: &str) -> Result<Self> {
        let (token, sha256) = value.split_once('.').ok_or(MediaError::Receipt)?;
        Ok(Self {
            token: Sha256::new(token).map_err(|_| MediaError::Receipt)?,
            sha256: Sha256::new(sha256).map_err(|_| MediaError::Receipt)?,
        })
    }
    pub fn encoded(&self) -> String {
        format!("{}.{}", self.token.as_str(), self.sha256.as_str())
    }
}
pub struct VerifiedTrashReceipt {
    record: Receipt,
}
fn private(directory: &File) -> Result<()> {
    let s = fs::fstat(directory).map_err(|_| MediaError::Receipt)?;
    if FileType::from_raw_mode(s.st_mode) != FileType::Directory
        || s.st_uid != rustix::process::geteuid().as_raw()
        || s.st_mode & 0o7777 != 0o700
    {
        return Err(MediaError::Receipt);
    }
    Ok(())
}
fn filename(token: &Sha256) -> String {
    format!("media-trash-{}.json", token.as_str())
}
impl VerifiedTrashReceipt {
    pub fn load(
        directory: &File,
        identity: &TrashReceiptIdentity,
        account: &CaptureAccount,
        history: &OperationJournal,
    ) -> Result<Self> {
        private(directory)?;
        let token = &identity.token;
        let file = File::from(
            fs::openat2(
                directory,
                filename(token).as_str(),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
                NO_LINKS,
            )
            .map_err(|_| MediaError::Receipt)?,
        );
        let stat = fs::fstat(&file).map_err(|_| MediaError::Receipt)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_uid != rustix::process::geteuid().as_raw()
            || stat.st_mode & 0o7777 != 0o400
            || stat.st_size < 0
            || stat.st_size as usize > MAX_RECEIPT
        {
            return Err(MediaError::Receipt);
        }
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_RECEIPT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| MediaError::Receipt)?;
        if bytes.len() != stat.st_size as usize
            || bytes.len() > MAX_RECEIPT
            || digest(&bytes) != identity.sha256.as_str()
        {
            return Err(MediaError::Receipt);
        }
        let record: Receipt = serde_json::from_slice(&bytes).map_err(|_| MediaError::Receipt)?;
        if record.version != 1
            || &record.selection.staging_token != token
            || digest(record.operation.as_str().as_bytes()) != token.as_str()
            || !matches!(record.selection.mutation, CatalogMutation::Remove { .. })
        {
            return Err(MediaError::Receipt);
        }
        record.inventory.require_account(account).map_err(|_| MediaError::Account)?;
        record.selection.validate().map_err(|_| MediaError::Receipt)?;
        let status = history.get(&record.operation).ok_or(MediaError::Receipt)?;
        let context = history.mutation_context(&record.operation).ok_or(MediaError::Receipt)?;
        if status.action != crate::ManagedActionKind::DeleteMedia
            || status.phase != OperationPhase::Succeeded
            || status.error.is_some()
            || context.stage != MutationStage::Complete
            || context.outcome.is_some()
            || context.checkpoint.as_ref() != Some(&record.pre)
            || context.post_checkpoint.is_none()
            || context.plan.operation_key != "media-delete"
        {
            return Err(MediaError::Receipt);
        }
        validate_moves(&record.moves, &record.selection.staging_token, &record.inventory)?;
        Ok(Self { record })
    }
}
fn validate_moves(moves: &[MediaMove], token: &Sha256, inventory: &AssetInventory) -> Result<()> {
    if moves.is_empty() || moves.len() > 4 {
        return Err(MediaError::Receipt);
    }
    let mut names = BTreeSet::new();
    for (index, item) in moves.iter().enumerate() {
        if !matches!(
            item.source.namespace(),
            AssetNamespace::ConfiguredLibrary | AssetNamespace::WallpaperCache
        ) || item.trash.namespace() != AssetNamespace::MediaTrash
            || item.trash.relative().as_str()
                != format!(".ryoku-media-trash-{}-{index}", token.as_str())
            || !names.insert(item.source.clone())
        {
            return Err(MediaError::Receipt);
        }
        for path in [&item.source, &item.trash] {
            let entry = inventory
                .entries()
                .iter()
                .find(|entry| entry.path == *path)
                .ok_or(MediaError::Receipt)?;
            if !matches!(entry.kind, EntryKind::Absent | EntryKind::RegularFile { .. })
                || (path == &item.trash && entry.kind != EntryKind::Absent)
            {
                return Err(MediaError::Receipt);
            }
        }
    }
    Ok(())
}
fn parent(home: &File, path: &AssetPath, account: &CaptureAccount) -> Result<(File, String)> {
    let relative = path.home_relative().map_err(|_| MediaError::Scope)?;
    let (prefix, leaf) = relative.as_str().rsplit_once('/').unwrap_or(("", relative.as_str()));
    let mut current = home.try_clone().map_err(|_| MediaError::Io)?;
    for component in prefix.split('/').filter(|s| !s.is_empty()) {
        current = File::from(
            fs::openat2(
                &current,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                NO_LINKS,
            )
            .map_err(|_| MediaError::Changed)?,
        );
        let stat = fs::fstat(&current).map_err(|_| MediaError::Changed)?;
        if stat.st_uid != account.uid || stat.st_gid != account.gid || stat.st_mode & 0o7022 != 0 {
            return Err(MediaError::Changed);
        }
    }
    Ok((current, leaf.into()))
}
#[derive(Clone, Eq, PartialEq, Serialize)]
pub(super) struct ParentBinding {
    path: AssetPath,
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}
fn bind_parent(home: &File, path: &AssetPath, account: &CaptureAccount) -> Result<ParentBinding> {
    let (directory, _) = parent(home, path, account)?;
    let stat = fs::fstat(&directory).map_err(|_| MediaError::Changed)?;
    if stat.st_uid != account.uid || stat.st_gid != account.gid || stat.st_mode & 0o7022 != 0 {
        return Err(MediaError::Changed);
    }
    Ok(ParentBinding {
        path: path.clone(),
        device: stat.st_dev,
        inode: stat.st_ino,
        uid: stat.st_uid,
        gid: stat.st_gid,
        mode: stat.st_mode,
    })
}
pub(super) fn parent_bindings(
    home: &File,
    account: &CaptureAccount,
    moves: &[MediaMove],
    inventory: &AssetInventory,
    deletion: bool,
) -> Result<Vec<ParentBinding>> {
    let mut result = Vec::new();
    for item in moves {
        let from = if deletion { &item.source } else { &item.trash };
        if inventory
            .entries()
            .iter()
            .any(|entry| entry.path == *from && matches!(entry.kind, EntryKind::RegularFile { .. }))
        {
            result.push(bind_parent(home, &item.source, account)?);
            result.push(bind_parent(home, &item.trash, account)?);
        }
    }
    Ok(result)
}
impl PreparedMedia {
    pub fn restore(
        home: &File,
        account: &CaptureAccount,
        actor_pictures: Option<&str>,
        receipt: &VerifiedTrashReceipt,
        token: Sha256,
    ) -> Result<Self> {
        let record = &receipt.record;
        record.inventory.require_account(account).map_err(|_| MediaError::Account)?;
        let roots =
            ryogami_paths::resolve(home, account, actor_pictures).map_err(|_| MediaError::Scope)?;
        if roots.relocated_cache() {
            return Err(MediaError::Scope);
        }
        let catalog = read_catalog(home, account)?;
        let key = record.selection.mutation.key();
        if catalog.rows.contains_key(key) {
            return Err(MediaError::Changed);
        }
        let commit = CatalogCommit {
            expected_catalog: catalog.identity.sha256.clone(),
            staging_token: token,
            mutation: CatalogMutation::Insert { key: key.into(), entry: record.row.clone() },
        };
        commit.validate().map_err(|_| MediaError::Receipt)?;
        let mut paths = vec![commit.staging_asset().map_err(|_| MediaError::Scope)?];
        for item in &record.moves {
            match item.source.namespace() {
                AssetNamespace::ConfiguredLibrary => {
                    if library_asset(
                        &roots,
                        &item.source.home_relative().map_err(|_| MediaError::Scope)?,
                    )? != item.source
                    {
                        return Err(MediaError::Scope);
                    }
                }
                AssetNamespace::WallpaperCache => (),
                _ => return Err(MediaError::Receipt),
            }
            paths.extend([item.source.clone(), item.trash.clone()]);
        }
        let scope = AssetScope::new(
            paths,
            MAX_CAPTURE_LIMITS.max_file_bytes,
            MAX_CAPTURE_LIMITS.max_total_bytes,
        )
        .map_err(|_| MediaError::Scope)?;
        let inventory =
            scope.capture(home, account).map_err(|_| MediaError::Changed)?.inventory().clone();
        for item in &record.moves {
            let original = record
                .inventory
                .entries()
                .iter()
                .find(|entry| entry.path == item.source)
                .ok_or(MediaError::Receipt)?;
            for (path, kind) in [(&item.source, &EntryKind::Absent), (&item.trash, &original.kind)]
            {
                if !inventory
                    .entries()
                    .iter()
                    .any(|entry| entry.path == *path && entry.kind == *kind)
                {
                    return Err(MediaError::Changed);
                }
            }
        }
        if inventory.entries().iter().any(|entry| {
            entry.path.namespace() == AssetNamespace::CatalogStage
                && entry.kind != EntryKind::Absent
        }) {
            return Err(MediaError::Changed);
        }
        let parents = parent_bindings(home, account, &record.moves, &inventory, false)?;
        Ok(Self {
            commit,
            row: record.row.clone(),
            catalog: catalog.identity,
            original_rows: catalog.rows,
            library_binding: roots.input_digest().into(),
            scope,
            inventory,
            moves: record.moves.clone(),
            parents,
        })
    }
    /// Writes the protected receipt before deletion. The caller owns native
    /// authorization, writer lease, durable Applying and linked post-capture.
    pub fn move_files(
        &self,
        operation: &OperationId,
        proof: &VerifiedLinkedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<Option<TrashReceiptIdentity>> {
        if proof.linked().transaction_id.as_str() != format!("{}-pre", operation.as_str()) {
            return Err(MediaError::Receipt);
        }
        self.verify_parents(home, account)?;
        if self.scope.capture(home, account).map_err(|_| MediaError::Changed)?.inventory()
            != &self.inventory
            || read_catalog(home, account)?.identity != self.catalog
        {
            return Err(MediaError::Changed);
        }
        if !boundary() {
            return Err(MediaError::Interrupted);
        }
        let deletion = matches!(self.commit.mutation, CatalogMutation::Remove { .. });
        let mut receipt_identity = None;
        if deletion {
            if digest(operation.as_str().as_bytes()) != self.commit.staging_token.as_str() {
                return Err(MediaError::Receipt);
            }
            validate_moves(&self.moves, &self.commit.staging_token, &self.inventory)?;
            private(journal)?;
            let record = Receipt {
                version: 1,
                operation: operation.clone(),
                pre: proof.linked().clone(),
                selection: self.commit.clone(),
                row: self.row.clone(),
                inventory: self.inventory.clone(),
                moves: self.moves.clone(),
            };
            let bytes = serde_json::to_vec(&record).map_err(|_| MediaError::Receipt)?;
            if bytes.len() > MAX_RECEIPT {
                return Err(MediaError::Receipt);
            }
            let mut file = File::from(
                fs::openat(
                    journal,
                    filename(&self.commit.staging_token).as_str(),
                    OFlags::RDWR
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::from_raw_mode(0o600),
                )
                .map_err(|_| MediaError::Receipt)?,
            );
            file.write_all(&bytes).map_err(|_| MediaError::Receipt)?;
            fs::fchmod(&file, Mode::from_raw_mode(0o400)).map_err(|_| MediaError::Receipt)?;
            file.sync_all().map_err(|_| MediaError::Receipt)?;
            fs::fsync(journal).map_err(|_| MediaError::Receipt)?;
            use std::os::unix::fs::FileExt;
            let mut check = vec![0; bytes.len()];
            file.read_exact_at(&mut check, 0).map_err(|_| MediaError::Receipt)?;
            if check != bytes {
                return Err(MediaError::Receipt);
            }
            receipt_identity = Some(TrashReceiptIdentity {
                token: self.commit.staging_token.clone(),
                sha256: Sha256::new(digest(&bytes)).map_err(|_| MediaError::Receipt)?,
            });
        }
        for item in &self.moves {
            if !boundary() {
                return Err(MediaError::Interrupted);
            }
            let (from, to) =
                if deletion { (&item.source, &item.trash) } else { (&item.trash, &item.source) };
            let before = self
                .inventory
                .entries()
                .iter()
                .find(|entry| entry.path == *from)
                .ok_or(MediaError::Changed)?;
            if before.kind == EntryKind::Absent {
                continue;
            }
            let current = ryoku_recovery::ryogami_assets::capture_assets(
                home,
                account,
                &[from.clone(), to.clone()],
                MAX_CAPTURE_LIMITS,
            )
            .map_err(|_| MediaError::Changed)?;
            if !current
                .inventory()
                .entries()
                .iter()
                .any(|entry| entry.path == *from && entry.kind == before.kind)
                || !current
                    .inventory()
                    .entries()
                    .iter()
                    .any(|entry| entry.path == *to && entry.kind == EntryKind::Absent)
            {
                return Err(MediaError::Changed);
            }
            self.verify_parents(home, account)?;
            let (source, source_leaf) = parent(home, from, account)?;
            let (destination, destination_leaf) = parent(home, to, account)?;
            fs::renameat_with(
                &source,
                source_leaf.as_str(),
                &destination,
                destination_leaf.as_str(),
                fs::RenameFlags::NOREPLACE,
            )
            .map_err(|_| MediaError::Changed)?;
            fs::fsync(&source).map_err(|_| MediaError::Io)?;
            fs::fsync(&destination).map_err(|_| MediaError::Io)?;
        }
        if !boundary() {
            return Err(MediaError::Interrupted);
        }
        self.verify_moves(home, account)?;
        Ok(receipt_identity)
    }
    fn verify_parents(&self, home: &File, account: &CaptureAccount) -> Result<()> {
        for expected in &self.parents {
            if bind_parent(home, &expected.path, account)? != *expected {
                return Err(MediaError::Changed);
            }
        }
        Ok(())
    }
    pub fn verify_moves(&self, home: &File, account: &CaptureAccount) -> Result<()> {
        self.verify_parents(home, account)?;
        let actual = self.scope.capture(home, account).map_err(|_| MediaError::Changed)?;
        if actual.inventory().entries().iter().any(|entry| {
            entry.path.namespace() == AssetNamespace::CatalogStage
                && entry.kind != EntryKind::Absent
        }) {
            return Err(MediaError::Changed);
        }
        let deletion = matches!(self.commit.mutation, CatalogMutation::Remove { .. });
        for item in &self.moves {
            let (from, to) =
                if deletion { (&item.source, &item.trash) } else { (&item.trash, &item.source) };
            let expected = self
                .inventory
                .entries()
                .iter()
                .find(|entry| entry.path == *from)
                .ok_or(MediaError::Changed)?;
            for (path, kind) in [(from, &EntryKind::Absent), (to, &expected.kind)] {
                if !actual
                    .inventory()
                    .entries()
                    .iter()
                    .any(|entry| entry.path == *path && entry.kind == *kind)
                {
                    return Err(MediaError::Changed);
                }
            }
        }
        Ok(())
    }
}
