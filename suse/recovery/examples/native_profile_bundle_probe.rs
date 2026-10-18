//! Inactive bundle experiment for the seeded disposable QEMU guest only.
//! It holds the production mutation lock and refuses incomplete recovery.
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags, ResolveFlags};
use ryoku_profiles::{Density, Desktop, ResolvedSettings, Theme};
use ryoku_recovery::{
    Identifier,
    installed_profiles::{
        InstalledProfileBundle, ProfileBundleStore, StateMountProfileLifecycleGuard,
    },
    machine_state::read_machine_state_record,
    state_mount::collect_and_open_state_mount,
};
use std::{
    fs::File,
    io::{Read, Write},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
const SYSTEM: &[u8] =
    b"{ \"schema_version\":1, \"id\":\"generic-desktop\", \"settings\":{\"desktop\":\"kde\"} }\n";
const COMPANION: &[u8] =
    b"{\n\"schema_version\":1,\"id\":\"generic-desktop\",\"settings\":{\"theme\":\"system\"}}\n";
const PERSONAL: &[u8] =
    b"{\"schema_version\":1,\"id\":\"generic-desktop\",\"settings\":{\"density\":\"compact\"}}\n";
const EMPTY: &[u8] = b"{\"schema_version\":1,\"id\":\"generic-desktop\",\"settings\":{}}\n";

fn main() -> Result<()> {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--run"]
        || rustix::process::geteuid().as_raw() != 0
        || rustix::process::getegid().as_raw() != 0
        || bounded_file("/etc/ryoku-lab-fixture")? != b"disposable-issue13\n"
        || bounded_file("/sys/class/dmi/id/sys_vendor")? != b"QEMU\n"
    {
        return Err("requires --run as root in the seeded disposable QEMU guest".into());
    }
    let exclusion = StateMountProfileLifecycleGuard::acquire()?;
    exclusion.revalidate()?;
    let state = native_state()?;
    // Read/verify installed input before creating any profile directories.
    let full = InstalledProfileBundle::load_packaged_defaults(Some(SYSTEM), Some(COMPANION))?;
    let absent = InstalledProfileBundle::load_packaged_defaults(None, None)?;
    let empty = InstalledProfileBundle::load_packaged_defaults(Some(EMPTY), Some(EMPTY))?;
    if full.package() != absent.package()
        || full.package() != empty.package()
        || full.defaults_source_bytes() != absent.defaults_source_bytes()
        || full.defaults_source_bytes() != empty.defaults_source_bytes()
    {
        return Err("installed defaults changed across imports".into());
    }
    // Exclusive provisioning refuses prior runs and partial evidence.
    let store = ProfileBundleStore::provision_state_mount(&exclusion)?;
    let profiles = open_dir(&state, "profiles")?;
    let staging = open_dir(&profiles, ".staging")?;
    let mut owned = Vec::new();
    for (name, bundle) in [("full", &full), ("absent", &absent), ("empty", &empty)] {
        exclusion.revalidate()?;
        same(&profiles, &open_dir(&state, "profiles")?)?;
        let transaction = Identifier::new(format!("profile-probe-{name}"))
            .map_err(|_| "invalid fixed probe transaction")?;
        let verified = store.publish_candidate(&transaction, bundle, &exclusion)?;
        owned.push(OwnedBundle::pin(&profiles, verified.id().as_str(), bundle.canonical_bytes()?)?);
        verified.revalidate()?;
    }
    drop(store);
    let store = ProfileBundleStore::open_state_mount()?;
    let reopened = store.load(&full.id()?)?;
    if reopened.bundle().canonical_bytes()? != full.canonical_bytes()?
        || reopened.bundle().defaults_source_bytes() != full.defaults_source_bytes()
        || reopened.bundle().system_source_bytes() != Some(SYSTEM)
        || reopened.bundle().companion_source_bytes() != Some(COMPANION)
    {
        return Err("raw profile source fidelity failed".into());
    }
    // Resolution below uses only retained bundle bytes and synthetic personal
    // bytes. It performs no live RPM, checkout, package-path or network reads.
    let resolved = reopened.bundle().resolve(Some(PERSONAL))?;
    if resolved.settings
        != (ResolvedSettings {
            desktop: Some(Desktop::Kde),
            theme: Some(Theme::System),
            density: Some(Density::Compact),
        })
    {
        return Err("four-layer resolution failed".into());
    }
    let absent_saved = store.load(&absent.id()?)?;
    let empty_saved = store.load(&empty.id()?)?;
    let no_layers = absent_saved.bundle().resolve(None)?;
    let empty_layers = empty_saved.bundle().resolve(Some(EMPTY))?;
    if no_layers.settings != empty_layers.settings
        || absent_saved.id() == empty_saved.id()
        || no_layers.provenance.system_override_revision.is_some()
        || no_layers.provenance.companion_revision.is_some()
        || no_layers.provenance.personal_override_revision.is_some()
        || empty_layers.provenance.system_override_revision.is_none()
        || empty_layers.provenance.companion_revision.is_none()
        || empty_layers.provenance.personal_override_revision.is_none()
    {
        return Err("absent versus empty layer distinction failed".into());
    }

    // Move our exact original inode aside, then publish an identical replacement.
    // The old pinned handle must reject the changed name-to-inode association.
    exclusion.revalidate()?;
    owned[0].validate(&profiles, 0, 0)?;
    let held_name = format!(".probe-held-{}", full.id()?.as_str());
    fs::renameat_with(
        &profiles,
        &owned[0].name,
        &profiles,
        &held_name,
        fs::RenameFlags::NOREPLACE,
    )?;
    owned[0].name = held_name;
    if reopened.revalidate().is_ok() || store.load(&full.id()?).is_ok() {
        return Err("renamed bundle was accepted".into());
    }
    let transaction = Identifier::new("profile-probe-replacement")
        .map_err(|_| "invalid fixed probe transaction")?;
    let replacement = store.publish_candidate(&transaction, &full, &exclusion)?;
    owned.push(OwnedBundle::pin(&profiles, replacement.id().as_str(), full.canonical_bytes()?)?);
    if reopened.revalidate().is_ok() {
        return Err("replaced bundle was accepted by pinned handle".into());
    }
    replacement.revalidate()?;

    // Deliberate corruption retains the exact original inode ownership receipt.
    exclusion.revalidate()?;
    owned[2].validate(&profiles, 0, 0)?;
    let mut target = File::from(fs::openat2(
        &owned[2].directory,
        "bundle.json",
        OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        RESOLVE,
    )?);
    same(&target, &owned[2].file)?;
    target.set_len(0)?;
    target.write_all(b"x")?;
    target.sync_all()?;
    owned[2].expected = b"x".to_vec();
    if store.load(&empty.id()?).is_ok() || empty_saved.revalidate().is_ok() {
        return Err("corrupt bundle was accepted".into());
    }

    // No cleanup runs on an earlier failure. Validate the entire expected set
    // before deleting anything. Never recursively remove an arbitrary path.
    exclusion.revalidate()?;
    same(&profiles, &open_dir(&state, "profiles")?)?;
    same(&staging, &open_dir(&profiles, ".staging")?)?;
    check_dir(&profiles, 0, 0, 0o700)?;
    check_dir(&staging, 0, 0, 0o700)?;
    let mut expected = owned.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>();
    expected.push(".staging".into());
    members(&profiles, &expected)?;
    members(&staging, &[])?;
    for entry in &owned {
        entry.validate(&profiles, 0, 0)?;
    }
    for entry in owned {
        exclusion.revalidate()?;
        same(&profiles, &open_dir(&state, "profiles")?)?;
        entry.remove(&profiles, 0, 0)?;
    }
    exclusion.revalidate()?;
    same(&profiles, &open_dir(&state, "profiles")?)?;
    same(&staging, &open_dir(&profiles, ".staging")?)?;
    members(&staging, &[])?;
    members(&profiles, &[".staging".into()])?;
    fs::unlinkat(&profiles, ".staging", AtFlags::REMOVEDIR)?;
    fs::fsync(&profiles)?;
    members(&profiles, &[])?;
    fs::unlinkat(&state, "profiles", AtFlags::REMOVEDIR)?;
    fs::fsync(&state)?;
    println!(
        "native_profile_rpm_proof=passed\nnative_profile_explicit_provisioning=passed\nnative_profile_exact_reopen=passed\nnative_profile_raw_fidelity=passed\nnative_profile_four_layers=passed\nnative_profile_optional_absence=passed\nnative_profile_rename_replacement_refusal=passed\nnative_profile_tamper_refusal=passed\nnative_profile_exact_cleanup=passed"
    );
    Ok(())
}

struct OwnedBundle {
    name: String,
    directory: File,
    file: File,
    expected: Vec<u8>,
}
impl OwnedBundle {
    fn pin(parent: &File, name: &str, expected: Vec<u8>) -> Result<Self> {
        let directory = open_dir(parent, name)?;
        let file = open_file(&directory)?;
        Ok(Self { name: name.into(), directory, file, expected })
    }
    fn validate(&self, parent: &File, uid: u32, gid: u32) -> Result<()> {
        same(&self.directory, &open_dir(parent, &self.name)?)?;
        check_dir(&self.directory, uid, gid, 0o500)?;
        members(&self.directory, &["bundle.json".into()])?;
        let current = open_file(&self.directory)?;
        same(&self.file, &current)?;
        check_file(&current, uid, gid, 0o400)?;
        let mut bytes = Vec::new();
        current.take(512 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes != self.expected {
            return Err("fixture content changed".into());
        }
        Ok(())
    }
    fn remove(self, parent: &File, uid: u32, gid: u32) -> Result<()> {
        self.validate(parent, uid, gid)?;
        // Only our exact directory inode is made writable for ordinary-user
        // fixture tests; file identity and metadata have already been checked.
        fs::fchmod(&self.directory, Mode::from_raw_mode(0o700))?;
        same(&self.file, &open_file(&self.directory)?)?;
        fs::unlinkat(&self.directory, "bundle.json", AtFlags::empty())?;
        fs::fsync(&self.directory)?;
        members(&self.directory, &[])?;
        same(&self.directory, &open_dir(parent, &self.name)?)?;
        fs::unlinkat(parent, &self.name, AtFlags::REMOVEDIR)?;
        fs::fsync(parent)?;
        Ok(())
    }
}
fn native_state() -> Result<File> {
    let root = File::open("/")?;
    let record = read_machine_state_record(&root)?;
    Ok(File::from(collect_and_open_state_mount(&root, record.identity())?.0))
}
fn bounded_file(path: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(256).read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn open_dir(parent: &File, path: &str) -> Result<File> {
    Ok(File::from(fs::openat2(
        parent,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        RESOLVE,
    )?))
}
fn open_file(directory: &File) -> Result<File> {
    Ok(File::from(fs::openat2(
        directory,
        "bundle.json",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
        RESOLVE,
    )?))
}
fn same(old: &File, current: &File) -> Result<()> {
    let old = fs::fstat(old)?;
    let current = fs::fstat(current)?;
    if old.st_dev != current.st_dev || old.st_ino != current.st_ino {
        return Err("fixture inode changed".into());
    }
    Ok(())
}
fn check_dir(file: &File, uid: u32, gid: u32, mode: u32) -> Result<()> {
    let stat = fs::fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != uid
        || stat.st_gid != gid
        || stat.st_mode & 0o7777 != mode
        || stat.st_nlink == 0
    {
        return Err("unsafe fixture directory".into());
    }
    Ok(())
}
fn check_file(file: &File, uid: u32, gid: u32, mode: u32) -> Result<()> {
    let stat = fs::fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != uid
        || stat.st_gid != gid
        || stat.st_mode & 0o7777 != mode
        || stat.st_nlink != 1
    {
        return Err("unsafe fixture file".into());
    }
    Ok(())
}
fn members(directory: &File, expected: &[String]) -> Result<()> {
    let mut names = Vec::new();
    for entry in fs::Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            names.push(String::from_utf8(name.to_vec())?);
        }
        if names.len() > expected.len() {
            return Err("unexpected fixture entry".into());
        }
    }
    names.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    if names != expected {
        return Err("fixture membership changed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn cleanup_accepts_only_exact_owned_file_and_directory_inodes() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "ryoku-probe-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        let parent = File::open(&path)?;
        let owner = fs::fstat(&parent)?;
        fs::mkdirat(&parent, "fixture", Mode::from_raw_mode(0o700))?;
        let directory = open_dir(&parent, "fixture")?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join("fixture/bundle.json"))?;
        file.write_all(b"synthetic")?;
        fs::fchmod(&file, Mode::from_raw_mode(0o400))?;
        fs::fchmod(&directory, Mode::from_raw_mode(0o500))?;
        let owned = OwnedBundle::pin(&parent, "fixture", b"synthetic".to_vec())?;
        owned.validate(&parent, owner.st_uid, owner.st_gid)?;
        fs::fchmod(&directory, Mode::from_raw_mode(0o700))?;
        std::fs::rename(path.join("fixture/bundle.json"), path.join("original"))?;
        let mut substitute = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join("fixture/bundle.json"))?;
        substitute.write_all(b"synthetic")?;
        fs::fchmod(&substitute, Mode::from_raw_mode(0o400))?;
        fs::fchmod(&directory, Mode::from_raw_mode(0o500))?;
        assert!(owned.validate(&parent, owner.st_uid, owner.st_gid).is_err());
        assert!(path.join("fixture/bundle.json").exists());
        fs::fchmod(&directory, Mode::from_raw_mode(0o700))?;
        std::fs::remove_file(path.join("fixture/bundle.json"))?;
        std::fs::rename(path.join("original"), path.join("fixture/bundle.json"))?;
        fs::fchmod(&directory, Mode::from_raw_mode(0o500))?;
        owned.remove(&parent, owner.st_uid, owner.st_gid)?;
        assert!(!path.join("fixture").exists());
        std::fs::remove_dir(path)?;
        Ok(())
    }

    #[test]
    fn cleanup_refuses_extra_entries_and_hardlinks_without_removing_evidence() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "ryoku-probe-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        let parent = File::open(&path)?;
        let owner = fs::fstat(&parent)?;
        fs::mkdirat(&parent, "fixture", Mode::from_raw_mode(0o700))?;
        std::fs::write(path.join("fixture/bundle.json"), b"x")?;
        std::fs::set_permissions(
            path.join("fixture/bundle.json"),
            std::fs::Permissions::from_mode(0o400),
        )?;
        let directory = open_dir(&parent, "fixture")?;
        let owned = OwnedBundle::pin(&parent, "fixture", b"x".to_vec())?;
        std::fs::write(path.join("fixture/foreign"), b"preserve")?;
        fs::fchmod(&directory, Mode::from_raw_mode(0o500))?;
        assert!(owned.validate(&parent, owner.st_uid, owner.st_gid).is_err());
        assert_eq!(std::fs::read(path.join("fixture/foreign"))?, b"preserve");
        fs::fchmod(&directory, Mode::from_raw_mode(0o700))?;
        std::fs::remove_file(path.join("fixture/foreign"))?;
        fs::fchmod(&directory, Mode::from_raw_mode(0o500))?;
        std::fs::hard_link(path.join("fixture/bundle.json"), path.join("alias"))?;
        assert!(owned.validate(&parent, owner.st_uid, owner.st_gid).is_err());
        std::fs::remove_file(path.join("alias"))?;
        owned.remove(&parent, owner.st_uid, owner.st_gid)?;
        std::fs::remove_dir(path)?;
        Ok(())
    }
}
