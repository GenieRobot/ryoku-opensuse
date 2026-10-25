//! Exact account and policy-bound outputs for approved native providers.
//!
//! A declaration is capture policy, never authorization. The authenticated
//! native provider must derive it from reviewed content before planning and
//! bind its digest to that plan. Public requests cannot supply declarations.
//! Directories are metadata/absence entries, never recursive enrollment.
use crate::{
    AccountPrincipal, EntryKind, RelativePath, Sha256,
    installed_capture::CaptureAccount,
    ryogami_assets::{
        AssetError, AssetNamespace, AssetPath, AssetRecord, AssetScope, MAX_CAPTURE_LIMITS,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputKind {
    RegularFile,
    Directory,
    /// Exact permitted pre/post link texts. Targets stay beneath the link's
    /// owned parent; no traversal, absolute target, or target read is granted.
    InternalSymlink {
        targets: Vec<RelativePath>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputEntry {
    pub path: RelativePath,
    pub kind: OutputKind,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDeclaration {
    principal: AccountPrincipal,
    uid: u32,
    gid: u32,
    policy_sha256: Sha256,
    entries: Vec<OutputEntry>,
}
impl OutputDeclaration {
    /// Called by a server-reviewed provider after account authorization and
    /// content approval. Ancestors are added as exact Directory entries so
    /// absence and metadata of newly created parents participate in capture.
    pub fn new(
        account: &CaptureAccount,
        policy_sha256: Sha256,
        outputs: Vec<OutputEntry>,
    ) -> Result<Self, AssetError> {
        let mut entries = BTreeMap::new();
        for output in outputs {
            if entries.insert(output.path.clone(), output.kind).is_some() {
                return Err(AssetError::Path);
            }
        }
        let paths: Vec<_> = entries.keys().cloned().collect();
        for path in paths {
            let mut parent = path.as_str();
            while let Some((prefix, _)) = parent.rsplit_once('/') {
                let relative = RelativePath::new(prefix).map_err(|_| AssetError::Path)?;
                if let Some(existing) = entries.get(&relative)
                    && existing != &OutputKind::Directory
                {
                    return Err(AssetError::Path);
                }
                entries.insert(relative, OutputKind::Directory);
                parent = prefix;
            }
        }
        let declaration = Self {
            principal: account.principal.clone(),
            uid: account.uid,
            gid: account.gid,
            policy_sha256,
            entries: entries.into_iter().map(|(path, kind)| OutputEntry { path, kind }).collect(),
        };
        declaration.validate()?;
        Ok(declaration)
    }
    pub fn entries(&self) -> &[OutputEntry] {
        &self.entries
    }
    pub fn policy_sha256(&self) -> &Sha256 {
        &self.policy_sha256
    }
    pub fn fingerprint(&self) -> Result<Sha256, AssetError> {
        self.validate()?;
        Sha256::new(crate::wallpaper_protocol::digest(
            &serde_json::to_vec(self).map_err(|_| AssetError::Manifest)?,
        ))
        .map_err(|_| AssetError::Manifest)
    }
    pub fn scope(
        &self,
        max_file_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<AssetScope, AssetError> {
        let binding = self.fingerprint()?;
        let paths = self
            .entries
            .iter()
            .map(|entry| AssetPath::for_declared_output(binding.clone(), entry.path.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        AssetScope::new_with_outputs(paths, max_file_bytes, max_total_bytes, Some(self.clone()))
    }
    pub(crate) fn validate(&self) -> Result<(), AssetError> {
        if self.uid == 0
            || self.uid == u32::MAX
            || self.gid == u32::MAX
            || self.entries.is_empty()
            || self.entries.len() > MAX_CAPTURE_LIMITS.max_paths
            || self.entries.windows(2).any(|pair| pair[0].path >= pair[1].path)
        {
            return Err(AssetError::Bounds);
        }
        for entry in &self.entries {
            if entry.path.as_str().len() > 1024
                || entry.path.as_str().split('/').count() > 16
                || crate::installed_inventory::PREFERENCES.contains(&entry.path.as_str())
            {
                return Err(AssetError::Path);
            }
            if let OutputKind::InternalSymlink { targets } = &entry.kind
                && (targets.is_empty()
                    || targets.len() > 16
                    || targets.windows(2).any(|pair| pair[0] >= pair[1])
                    || targets.iter().any(|target| target.as_str().len() > 1024))
            {
                return Err(AssetError::Path);
            }
            if let Some((parent, _)) = entry.path.as_str().rsplit_once('/')
                && !self.entries.iter().any(|candidate| {
                    candidate.path.as_str() == parent && candidate.kind == OutputKind::Directory
                })
            {
                return Err(AssetError::Path);
            }
        }
        Ok(())
    }
    pub(crate) fn require_account(&self, account: &CaptureAccount) -> Result<(), AssetError> {
        self.validate()?;
        if self.principal != account.principal || self.uid != account.uid || self.gid != account.gid
        {
            return Err(AssetError::Ownership);
        }
        Ok(())
    }
    pub(crate) fn validate_paths(&self, paths: &[AssetPath]) -> Result<(), AssetError> {
        let binding = self.fingerprint()?;
        let declared: Vec<_> = paths
            .iter()
            .filter(|path| path.namespace() == AssetNamespace::DeclaredOutput)
            .collect();
        if declared.len() != self.entries.len() {
            return Err(AssetError::Path);
        }
        for (path, entry) in declared.iter().zip(&self.entries) {
            if path.relative() != &entry.path || path.output_binding() != Some(&binding) {
                return Err(AssetError::Path);
            }
        }
        Ok(())
    }
    pub(crate) fn allows_link_target(&self, target: &str) -> bool {
        self.entries.iter().any(|entry|matches!(&entry.kind,OutputKind::InternalSymlink {targets} if targets.iter().any(|allowed|allowed.as_str()==target)))
    }
    pub(crate) fn validate_topology(&self, records: &[AssetRecord]) -> Result<(), AssetError> {
        for record in records.iter().filter(|record| {
            record.path.namespace() == AssetNamespace::DeclaredOutput
                && record.kind != EntryKind::Absent
        }) {
            if let Some((prefix, _)) = record.path.relative().as_str().rsplit_once('/')
                && !records.iter().any(|parent| {
                    parent.path.namespace() == AssetNamespace::DeclaredOutput
                        && parent.path.relative().as_str() == prefix
                        && matches!(parent.kind, EntryKind::Directory { .. })
                })
            {
                return Err(AssetError::Manifest);
            }
        }
        Ok(())
    }
    pub(crate) fn validate_record(&self, record: &AssetRecord) -> Result<(), AssetError> {
        let expected = self
            .entries
            .iter()
            .find(|entry| entry.path == *record.path.relative())
            .ok_or(AssetError::Path)?;
        let accepted = match (&expected.kind, &record.kind) {
            (_, EntryKind::Absent) => true,
            (OutputKind::RegularFile, EntryKind::RegularFile { .. })
            | (OutputKind::Directory, EntryKind::Directory { .. }) => true,
            (OutputKind::InternalSymlink { targets }, EntryKind::Symlink { target, .. }) => {
                targets.iter().any(|allowed| allowed.as_str() == target)
            }
            _ => false,
        };
        if !accepted {
            return Err(AssetError::Ownership);
        }
        Ok(())
    }
}

pub(crate) fn validate_scope(
    outputs: Option<&OutputDeclaration>,
    paths: &[AssetPath],
) -> Result<(), AssetError> {
    if let Some(outputs) = outputs {
        outputs.validate_paths(paths)?;
        for path in paths.iter().filter(|path| path.namespace() == AssetNamespace::OutputStage) {
            if let Some((parent, _)) = path.relative().as_str().split_once('/')
                && !paths.iter().any(|root| {
                    root.namespace() == AssetNamespace::OutputStage
                        && root.relative().as_str() == parent
                })
            {
                return Err(AssetError::Path);
            }
        }
        Ok(())
    } else if paths.iter().any(|path| {
        matches!(path.namespace(), AssetNamespace::DeclaredOutput | AssetNamespace::OutputStage)
    }) {
        Err(AssetError::Path)
    } else {
        Ok(())
    }
}

/// Only generated private stage roots are enumerated. Application directories
/// remain exact metadata declarations and their unrelated children are unread.
pub(crate) fn verify_stage_membership(
    home: &std::fs::File,
    account: &CaptureAccount,
    paths: &[AssetPath],
) -> Result<(), AssetError> {
    use rustix::fs::{self, Mode, OFlags, ResolveFlags};
    let flags = ResolveFlags::BENEATH
        | ResolveFlags::NO_SYMLINKS
        | ResolveFlags::NO_MAGICLINKS
        | ResolveFlags::NO_XDEV;
    for root in paths.iter().filter(|path| {
        path.namespace() == AssetNamespace::OutputStage && !path.relative().as_str().contains('/')
    }) {
        let directory = match fs::openat2(
            home,
            root.relative().as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            flags,
        ) {
            Ok(file) => std::fs::File::from(file),
            Err(rustix::io::Errno::NOENT) => continue,
            Err(error) => return Err(error.into()),
        };
        let stat = fs::fstat(&directory)?;
        if stat.st_uid != account.uid || stat.st_gid != account.gid || stat.st_mode & 0o7022 != 0 {
            return Err(AssetError::Ownership);
        }
        let mut count = 0;
        for entry in fs::Dir::read_from(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if matches!(name, b"." | b"..") {
                continue;
            }
            count += 1;
            if count > MAX_CAPTURE_LIMITS.max_paths {
                return Err(AssetError::Bounds);
            }
            let leaf = std::str::from_utf8(name).map_err(|_| AssetError::Path)?;
            let child = root.with_relative(
                RelativePath::new(format!("{}/{leaf}", root.relative().as_str()))
                    .map_err(|_| AssetError::Path)?,
            )?;
            if !paths.contains(&child) {
                return Err(AssetError::Path);
            }
            if leaf.starts_with("d-") {
                let child = fs::openat2(
                    &directory,
                    leaf,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    flags,
                )?;
                for entry in fs::Dir::read_from(&child)? {
                    let entry = entry?;
                    if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
                        return Err(AssetError::Path);
                    }
                }
            }
        }
    }
    Ok(())
}
