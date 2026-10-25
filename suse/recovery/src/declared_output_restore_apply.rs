use super::*;

fn inode_equal(left: &Pin, right: &Pin) -> bool {
    left.device == right.device
        && left.inode == right.inode
        && left.uid == right.uid
        && left.gid == right.gid
        && FileType::from_raw_mode(left.mode) == FileType::from_raw_mode(right.mode)
        && (FileType::from_raw_mode(left.mode) == FileType::Directory || left.mode == right.mode)
}
fn optional_pin(home: &File, path: &AssetPath, account: &CaptureAccount) -> Result<Option<Pin>> {
    match pin(home, Some(path), account) {
        Ok(value) => Ok(Some(value)),
        Err(RestoreError::Filesystem(rustix::io::Errno::NOENT)) => Ok(None),
        Err(error) => Err(error),
    }
}
fn is(actual: &Option<Pin>, expected: Option<&Pin>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(actual), Some(expected)) => inode_equal(actual, expected),
        _ => false,
    }
}
fn empty(directory: &File) -> Result<bool> {
    for entry in fs::Dir::read_from(directory)? {
        let entry = entry?;
        if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
            return Ok(false);
        }
    }
    Ok(true)
}
pub(super) fn validate_removals(
    record: &Record,
    home: &File,
    account: &CaptureAccount,
) -> Result<()> {
    for (target, previous) in record.targets.iter().zip(&record.previous) {
        if target.kind != EntryKind::Absent || !matches!(previous.kind, EntryKind::Directory { .. })
        {
            continue;
        }
        let directory = match open_directory(home, &target.path) {
            Ok(value) => value,
            Err(RestoreError::Filesystem(rustix::io::Errno::NOENT)) => continue,
            Err(error) => return Err(error),
        };
        let actual = fs::fstat(&directory)?;
        if actual.st_uid != account.uid || actual.st_gid != account.gid {
            return Err(RestoreError::Changed);
        }
        for child in fs::Dir::read_from(&directory)? {
            let child = child?;
            let bytes = child.file_name().to_bytes();
            if matches!(bytes, b"." | b"..") {
                continue;
            }
            let leaf = std::str::from_utf8(bytes).map_err(|_| RestoreError::Changed)?;
            let relative = format!("{}/{leaf}", target.path.relative().as_str());
            if !record.targets.iter().any(|entry| {
                entry.path.relative().as_str() == relative && entry.kind == EntryKind::Absent
            }) {
                return Err(RestoreError::Changed);
            }
        }
    }
    Ok(())
}
impl PreparedOutputRestore {
    fn original_pin(&self, path: &AssetPath) -> Option<&Pin> {
        self.record.original_pins.iter().find(|p| p.path.as_ref() == Some(path))
    }
    fn stage_pin(&self, index: usize) -> Result<Option<&Pin>> {
        let path = slot(
            &self.record.stage,
            index,
            directory_slot(&self.record.declaration, &self.record.targets[index])?,
        )?;
        Ok(self.ready.staged.iter().find(|p| p.path.as_ref() == Some(&path)))
    }
    fn roots(&self, home: &File, account: &CaptureAccount) -> Result<()> {
        if account.principal != self.record.account
            || pin(home, None, account)? != self.record.original_pins[0]
        {
            return Err(RestoreError::Changed);
        }
        for root in [&self.record.stage, &self.record.trash] {
            let expected = self
                .ready
                .staged
                .iter()
                .find(|p| p.path.as_ref() == Some(root))
                .ok_or(RestoreError::InvalidBinding)?;
            if pin(home, Some(root), account)? != *expected {
                return Err(RestoreError::Changed);
            }
        }
        verify_private_children(home, &self.record, account)
    }
    fn chain(&self, home: &File, path: &AssetPath, account: &CaptureAccount) -> Result<()> {
        self.roots(home, account)?;
        if path == &self.record.stage
            || path == &self.record.trash
            || path
                .relative()
                .as_str()
                .strip_prefix(self.record.stage.relative().as_str())
                .is_some_and(|tail| tail.starts_with('/'))
            || path
                .relative()
                .as_str()
                .strip_prefix(self.record.trash.relative().as_str())
                .is_some_and(|tail| tail.starts_with('/'))
        {
            return Ok(());
        }
        let mut parent_path = path.relative().as_str();
        while let Some((prefix, _)) = parent_path.rsplit_once('/') {
            let (index, entry) = self
                .record
                .targets
                .iter()
                .enumerate()
                .find(|(_, entry)| {
                    entry.path.namespace() == path.namespace()
                        && entry.path.relative().as_str() == prefix
                })
                .ok_or(RestoreError::InvalidBinding)?;
            let actual = optional_pin(home, &entry.path, account)?;
            let old = self.original_pin(&entry.path);
            let created = self.stage_pin(index)?;
            if let Some(actual) = &actual {
                if !old.is_some_and(|pin| inode_equal(actual, pin))
                    && !created.is_some_and(|pin| inode_equal(actual, pin))
                {
                    return Err(RestoreError::Changed);
                }
                if FileType::from_raw_mode(actual.mode) != FileType::Directory {
                    return Err(RestoreError::Changed);
                }
            } else if entry.kind != EntryKind::Absent
                && self.record.previous[index].kind != EntryKind::Absent
            {
                return Err(RestoreError::Changed);
            }
            parent_path = prefix;
        }
        Ok(())
    }
    fn kind(&self, home: &File, path: &AssetPath, account: &CaptureAccount) -> Result<EntryKind> {
        self.chain(home, path, account)?;
        let captured = crate::capture::capture_ryogami_assets(
            home,
            &[path.home_relative().map_err(|_| RestoreError::InvalidBinding)?],
            crate::capture::CaptureLimits { max_paths: 1, ..MAX_CAPTURE_LIMITS },
        )
        .map_err(|_| RestoreError::Changed)?;
        Ok(captured.records.into_iter().next().ok_or(RestoreError::Changed)?.kind)
    }
    fn directory_metadata(
        &self,
        home: &File,
        path: &AssetPath,
        account: &CaptureAccount,
        first: &EntryKind,
        second: &EntryKind,
    ) -> Result<()> {
        let EntryKind::Directory { mut metadata } = self.kind(home, path, account)? else {
            return Err(RestoreError::Changed);
        };
        for kind in [first, second] {
            if let EntryKind::Directory { metadata: expected } = kind {
                metadata.mtime_unix_ns = expected.mtime_unix_ns;
                if &metadata == expected {
                    return Ok(());
                }
            }
        }
        Err(RestoreError::Changed)
    }
    fn require_kind(
        &self,
        home: &File,
        path: &AssetPath,
        account: &CaptureAccount,
        kind: &EntryKind,
    ) -> Result<()> {
        if self.kind(home, path, account)? != *kind {
            return Err(RestoreError::Changed);
        }
        Ok(())
    }
    fn move_exact(
        &self,
        home: &File,
        from: &AssetPath,
        to: &AssetPath,
        account: &CaptureAccount,
        exchange: bool,
    ) -> Result<()> {
        self.chain(home, from, account)?;
        self.chain(home, to, account)?;
        let (from_parent, from_leaf) = parent(home, from, account)?;
        let (to_parent, to_leaf) = parent(home, to, account)?;
        fs::renameat_with(
            &from_parent,
            from_leaf.as_str(),
            &to_parent,
            to_leaf.as_str(),
            if exchange { fs::RenameFlags::EXCHANGE } else { fs::RenameFlags::NOREPLACE },
        )?;
        fs::fsync(&from_parent)?;
        fs::fsync(&to_parent)?;
        Ok(())
    }
    fn entry(
        &self,
        index: usize,
        home: &File,
        account: &CaptureAccount,
        checkpoint: &VerifiedGeneration,
        boundary: &mut impl FnMut() -> bool,
    ) -> Result<()> {
        check(boundary)?;
        let target = &self.record.targets[index];
        let previous = &self.record.previous[index];
        let stage = slot(
            &self.record.stage,
            index,
            directory_slot(&self.record.declaration, &self.record.targets[index])?,
        )?;
        let trash = slot(
            &self.record.trash,
            index,
            directory_slot(&self.record.declaration, &self.record.targets[index])?,
        )?;
        self.chain(home, &target.path, account)?;
        let actual = optional_pin(home, &target.path, account)?;
        let staged = optional_pin(home, &stage, account)?;
        let retained = optional_pin(home, &trash, account)?;
        let original = self.original_pin(&target.path);
        let desired = self.stage_pin(index)?;
        if target.kind == previous.kind {
            if !is(&actual, original) || staged.is_some() || retained.is_some() {
                return Err(RestoreError::Changed);
            }
            if matches!(target.kind, EntryKind::Directory { .. }) {
                self.directory_metadata(home, &target.path, account, &target.kind, &target.kind)?;
            } else {
                self.require_kind(home, &target.path, account, &target.kind)?;
            }
            return Ok(());
        }
        if target.kind == EntryKind::Absent {
            if staged.is_some() {
                return Err(RestoreError::Changed);
            }
            if actual.is_none() {
                if !is(&retained, original) {
                    return Err(RestoreError::Changed);
                }
                self.require_kind(home, &trash, account, &previous.kind)?;
                return Ok(());
            }
            if !is(&actual, original) || retained.is_some() {
                return Err(RestoreError::Changed);
            }
            if matches!(previous.kind, EntryKind::Directory { .. }) {
                let directory = open_directory(home, &target.path)?;
                if !empty(&directory)? {
                    return Err(RestoreError::Changed);
                }
                checkpoint.prepare_asset_directory_metadata(&target.path, account, &directory)?;
            }
            self.require_kind(home, &target.path, account, &previous.kind)?;
            self.move_exact(home, &target.path, &trash, account, false)?;
            check(boundary)?;
            return Ok(());
        }
        if matches!(target.kind, EntryKind::Directory { .. })
            && matches!(previous.kind, EntryKind::Directory { .. })
        {
            if !is(&actual, original) || !is(&staged, desired) || retained.is_some() {
                return Err(RestoreError::Changed);
            }
            self.require_kind(home, &stage, account, &target.kind)?;
            self.directory_metadata(home, &target.path, account, &previous.kind, &target.kind)?;
            return Ok(());
        }
        if is(&actual, desired) {
            if matches!(target.kind, EntryKind::Directory { .. }) {
                self.directory_metadata(home, &target.path, account, &target.kind, &target.kind)?;
            } else {
                self.require_kind(home, &target.path, account, &target.kind)?;
            }
            if original.is_some() && is(&staged, original) && retained.is_none() {
                self.require_kind(home, &stage, account, &previous.kind)?;
                self.move_exact(home, &stage, &trash, account, false)?;
                check(boundary)?;
            } else if staged.is_some() || !is(&retained, original) {
                return Err(RestoreError::Changed);
            }
            self.require_kind(home, &trash, account, &previous.kind)?;
            return Ok(());
        }
        if !is(&actual, original) || !is(&staged, desired) || retained.is_some() {
            return Err(RestoreError::Changed);
        }
        self.require_kind(home, &target.path, account, &previous.kind)?;
        self.require_kind(home, &stage, account, &target.kind)?;
        self.move_exact(home, &stage, &target.path, account, original.is_some())?;
        check(boundary)?;
        if original.is_some() {
            self.require_kind(home, &stage, account, &previous.kind)?;
            self.move_exact(home, &stage, &trash, account, false)?;
            check(boundary)?;
        }
        Ok(())
    }
    /// Apply or resume only exact entries from the protected staged-inode
    /// receipt. Every preexisting file/link is retained; mixed directories are
    /// never exchanged. A returned receipt still needs native post-capture.
    pub fn apply(
        &self,
        source: &VerifiedGeneration,
        checkpoint: &VerifiedGeneration,
        home: &File,
        account: &CaptureAccount,
        journal: &File,
        mut boundary: impl FnMut() -> bool,
    ) -> Result<OutputRestoreReceipt> {
        let reopened = Self::reopen(source, checkpoint, account, journal, &self.receipt)?;
        if reopened.record != self.record || reopened.ready != self.ready {
            return Err(RestoreError::Changed);
        }
        self.roots(home, account)?;
        validate_removals(&self.record, home, account)?;
        let mut directories = self
            .record
            .targets
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.kind, EntryKind::Directory { .. }))
            .map(|(index, entry)| (index, entry.path.relative().as_str().split('/').count()))
            .collect::<Vec<_>>();
        directories.sort_by_key(|(_, depth)| *depth);
        for (index, _) in &directories {
            self.entry(*index, home, account, checkpoint, &mut boundary)?;
        }
        for (index, entry) in self.record.targets.iter().enumerate() {
            if !matches!(entry.kind, EntryKind::Directory { .. })
                && !matches!(self.record.previous[index].kind, EntryKind::Directory { .. })
            {
                self.entry(index, home, account, checkpoint, &mut boundary)?;
            }
        }
        let mut removals = self
            .record
            .targets
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                entry.kind == EntryKind::Absent
                    && matches!(self.record.previous[*index].kind, EntryKind::Directory { .. })
            })
            .map(|(index, entry)| (index, entry.path.relative().as_str().split('/').count()))
            .collect::<Vec<_>>();
        removals.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
        for (index, _) in removals {
            self.entry(index, home, account, checkpoint, &mut boundary)?;
        }
        for (index, _) in directories.into_iter().rev() {
            check(&mut boundary)?;
            let target = &self.record.targets[index];
            self.chain(home, &target.path, account)?;
            let actual = optional_pin(home, &target.path, account)?;
            let expected = self.original_pin(&target.path).or(self.stage_pin(index)?);
            if !is(&actual, expected) {
                return Err(RestoreError::Changed);
            }
            if self.kind(home, &target.path, account)? != target.kind {
                source.prepare_asset_directory_metadata(
                    &target.path,
                    account,
                    &open_directory(home, &target.path)?,
                )?;
            }
        }
        check(&mut boundary)?;
        if !self.restored(home, account)? {
            return Err(RestoreError::Changed);
        }
        Ok(self.receipt.clone())
    }
    pub fn restored(&self, home: &File, account: &CaptureAccount) -> Result<bool> {
        self.roots(home, account)?;
        let actual = self.record.scope.capture(home, account).map_err(|_| RestoreError::Changed)?;
        if self.record.targets.iter().any(|target| !actual.inventory().entries().contains(target)) {
            return Ok(false);
        }
        for (index, target) in self.record.targets.iter().enumerate() {
            let stage = slot(
                &self.record.stage,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            let trash = slot(
                &self.record.trash,
                index,
                directory_slot(&self.record.declaration, &self.record.targets[index])?,
            )?;
            let unused = matches!(target.kind, EntryKind::Directory { .. })
                && matches!(self.record.previous[index].kind, EntryKind::Directory { .. });
            let unchanged = target.kind == self.record.previous[index].kind;
            let expected_stage =
                if unused && !unchanged { target.kind.clone() } else { EntryKind::Absent };
            let expected_trash = if unused || unchanged {
                EntryKind::Absent
            } else {
                self.record.previous[index].kind.clone()
            };
            if !actual
                .inventory()
                .entries()
                .iter()
                .any(|entry| entry.path == stage && entry.kind == expected_stage)
                || !actual
                    .inventory()
                    .entries()
                    .iter()
                    .any(|entry| entry.path == trash && entry.kind == expected_trash)
            {
                return Err(RestoreError::Changed);
            }
            let live = optional_pin(home, &target.path, account)?;
            let expected_live = if unused || unchanged {
                self.original_pin(&target.path)
            } else {
                self.stage_pin(index)?
            };
            if !is(&live, expected_live) {
                return Err(RestoreError::Changed);
            }
        }
        Ok(true)
    }
}
