//! Write-ahead local trash receipts. This primitive only moves owned content
//! inside the fixed provider staging namespace; it never follows source paths.
use super::*;
use crate::{
    OperationId,
    mutation_gate::{LinkedGeneration, VerifiedLinkedGeneration},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrashReceipt {
    version: u16,
    operation: OperationId,
    content: ContentIdentity,
    pre_checkpoint: LinkedGeneration,
}

/// Persist this small identity in the protected operation result. A receipt
/// found in the account-owned directory alone is not authoritative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrashReceiptIdentity {
    pub token: String,
    pub sha256: String,
}

/// The filesystem observation is deliberately separate from operation success:
/// OriginalPresent also describes an interrupted operation before its rename.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrashObservation {
    OriginalPresent,
    Trashed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrashPlan {
    operation: OperationId,
    identity: ContentIdentity,
    quota: Quota,
    root: FilePin,
    inventory: Vec<(String, FilePin)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlan {
    receipt: TrashReceiptIdentity,
    content: ContentIdentity,
    root: FilePin,
    inventory: Vec<(String, FilePin)>,
}

impl StagingDirectory {
    /// Read-only planning verifies content, metadata and absence of the two
    /// generated names before local deletion is eligible for checkpointing.
    pub fn plan_trash(
        &self,
        operation: OperationId,
        identity: ContentIdentity,
        quota: Quota,
    ) -> Result<TrashPlan, StageError> {
        quota.validate()?;
        self.verify(&identity)?;
        let root = self.validate_directory()?;
        let inventory = self.inventory()?;
        let token = token(&operation);
        if inventory.iter().any(|(name, _)| {
            name == &format!(".receipt-{token}") || name == &format!(".trash-{token}")
        }) {
            return Err(StageError::Collision);
        }
        let used = inventory
            .iter()
            .try_fold(0u64, |n, (_, pin)| n.checked_add(pin.size))
            .ok_or(StageError::Quota)?;
        if inventory.len() + 1 > MAX_ENTRIES
            || used.checked_add(4096).is_none_or(|n| n > quota.directory_bytes)
        {
            return Err(StageError::Quota);
        }
        if pin(&self.directory)? != root {
            return Err(StageError::Changed);
        }
        Ok(TrashPlan { operation, identity, quota, root, inventory })
    }

    /// Requires the verified preimage supplied by the native mutation runner.
    /// The runner remains responsible for scope coverage, authorization, lease,
    /// post capture and protected journal publication; this is not an RPC.
    pub fn trash(
        &self,
        reviewed: &TrashPlan,
        checkpoint: &VerifiedLinkedGeneration,
        cancelled: &AtomicBool,
    ) -> Result<TrashReceiptIdentity, StageError> {
        check_abort(cancelled, Instant::now() + reviewed.quota.duration)?;
        if self.plan_trash(reviewed.operation.clone(), reviewed.identity.clone(), reviewed.quota)?
            != *reviewed
        {
            return Err(StageError::Changed);
        }
        let receipt = TrashReceipt {
            version: 1,
            operation: reviewed.operation.clone(),
            content: reviewed.identity.clone(),
            pre_checkpoint: checkpoint.linked().clone(),
        };
        let bytes = serde_json::to_vec(&receipt).map_err(|_| StageError::Identity)?;
        if bytes.len() > 4096 {
            return Err(StageError::Quota);
        }
        let token = token(&reviewed.operation);
        let receipt_identity = TrashReceiptIdentity {
            token: token.clone(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        let name = format!(".receipt-{token}");
        let mut file = File::from(fs::openat2(
            &self.directory,
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
            NO_LINKS,
        )?);
        fs::fchmod(&file, Mode::from_raw_mode(0o600))?;
        if rustix::process::geteuid().as_raw() == 0 {
            fs::fchown(
                &file,
                Some(rustix::process::Uid::from_raw(self.uid)),
                Some(rustix::process::Gid::from_raw(self.gid)),
            )?;
        }
        self.validate_file(&file)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::fsync(&self.directory)?;
        // Only a durable verified receipt allows the original to move. A
        // partial receipt or cancelled request retains the original payload.
        self.read_receipt(&receipt_identity)?;
        check_abort(cancelled, Instant::now() + reviewed.quota.duration)?;
        self.verify(&reviewed.identity)?;
        let original = self.open_file(&reviewed.identity.sha256)?;
        let expected = reviewed
            .inventory
            .iter()
            .find(|(n, _)| n == &reviewed.identity.sha256)
            .ok_or(StageError::Changed)?;
        if pin(&original)? != expected.1 {
            return Err(StageError::Changed);
        }
        fs::renameat_with(
            &self.directory,
            reviewed.identity.sha256.as_str(),
            &self.directory,
            format!(".trash-{token}").as_str(),
            RenameFlags::NOREPLACE,
        )?;
        fs::fsync(&self.directory)?;
        if self.observe_trash(&receipt_identity)? != TrashObservation::Trashed {
            return Err(StageError::Changed);
        }
        Ok(receipt_identity)
    }

    fn read_receipt(&self, expected: &TrashReceiptIdentity) -> Result<TrashReceipt, StageError> {
        if !valid_digest(&expected.token) || !valid_digest(&expected.sha256) {
            return Err(StageError::Identity);
        }
        let mut file = self.open_file(&format!(".receipt-{}", expected.token))?;
        let before = pin(&file)?;
        if before.size > 4096 {
            return Err(StageError::Quota);
        }
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096
            || pin(&file)? != before
            || format!("{:x}", Sha256::digest(&bytes)) != expected.sha256
        {
            return Err(StageError::Identity);
        }
        let receipt: TrashReceipt =
            serde_json::from_slice(&bytes).map_err(|_| StageError::Identity)?;
        receipt.content.validate()?;
        if receipt.version != 1 || token(&receipt.operation) != expected.token {
            return Err(StageError::Identity);
        }
        Ok(receipt)
    }

    pub fn observe_trash(
        &self,
        expected: &TrashReceiptIdentity,
    ) -> Result<TrashObservation, StageError> {
        self.validate_directory()?;
        let receipt = self.read_receipt(expected)?;
        let exists = |name: &str| -> Result<bool, StageError> {
            match fs::statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => Ok(true),
                Err(Errno::NOENT) => Ok(false),
                Err(error) => Err(error.into()),
            }
        };
        let trash = format!(".trash-{}", expected.token);
        match (exists(&receipt.content.sha256)?, exists(&trash)?) {
            (true, false) => {
                self.verify(&receipt.content)?;
                Ok(TrashObservation::OriginalPresent)
            }
            (false, true) => {
                self.verify_named(&trash, &receipt.content)?;
                Ok(TrashObservation::Trashed)
            }
            _ => Err(StageError::Changed),
        }
    }

    pub fn plan_restore(&self, receipt: TrashReceiptIdentity) -> Result<RestorePlan, StageError> {
        if self.observe_trash(&receipt)? != TrashObservation::Trashed {
            return Err(StageError::Collision);
        }
        let content = self.read_receipt(&receipt)?.content;
        let root = self.validate_directory()?;
        let inventory = self.inventory()?;
        if pin(&self.directory)? != root {
            return Err(StageError::Changed);
        }
        Ok(RestorePlan { receipt, content, root, inventory })
    }

    /// Restore is another protected mutation with its own preimage. It never
    /// replaces a new download, unrelated entry or earlier restored payload.
    pub fn restore(
        &self,
        reviewed: &RestorePlan,
        checkpoint: &VerifiedLinkedGeneration,
        cancelled: &AtomicBool,
    ) -> Result<ContentIdentity, StageError> {
        check_abort(cancelled, Instant::now() + MAX_DURATION)?;
        if self.plan_restore(reviewed.receipt.clone())? != *reviewed {
            return Err(StageError::Changed);
        }
        let receipt = self.read_receipt(&reviewed.receipt)?;
        if checkpoint.linked() == &receipt.pre_checkpoint {
            return Err(StageError::Changed);
        }
        fs::renameat_with(
            &self.directory,
            format!(".trash-{}", reviewed.receipt.token).as_str(),
            &self.directory,
            reviewed.content.sha256.as_str(),
            RenameFlags::NOREPLACE,
        )?;
        fs::fsync(&self.directory)?;
        if self.observe_trash(&reviewed.receipt)? != TrashObservation::OriginalPresent {
            return Err(StageError::Changed);
        }
        Ok(reviewed.content.clone())
    }
}
fn token(operation: &OperationId) -> String {
    format!("{:x}", Sha256::digest(operation.as_str().as_bytes()))
}
