use super::*;
use std::{
    cell::Cell,
    fs as stdfs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const ORIGINAL: &[u8] =
    b"{ \"schema_version\":1, \"id\":\"generic-desktop\", \"settings\":{\"theme\":\"system\"} }\n";
struct Fixture {
    path: PathBuf,
    root: File,
    owner: ProfileOwner,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-personal-writer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        stdfs::create_dir(&path).unwrap();
        stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o700)).unwrap();
        let root = File::open(&path).unwrap();
        let stat = fs::fstat(&root).unwrap();
        Self { path, root, owner: ProfileOwner { uid: stat.st_uid, gid: stat.st_gid } }
    }
    fn write(&self, bytes: &[u8], mode: u32) {
        stdfs::write(self.path.join(PROFILE), bytes).unwrap();
        stdfs::set_permissions(self.path.join(PROFILE), stdfs::Permissions::from_mode(mode))
            .unwrap();
    }
    fn plan(&self) -> PersonalProfilePlan {
        let defaults = InstalledProfileBundle::from_local_documents(
            ryoku_recovery::installed_profiles::DefaultsPackage {
                name: ryoku_recovery::PackageName::new("ryoku-profile-defaults").unwrap(),
                nevra: ryoku_recovery::Revision::new("ryoku-profile-defaults-0:0.1.0-1.noarch")
                    .unwrap(),
                upstream_revision: None,
            },
            include_bytes!("../../../packaging/rpm/defaults/profile.json"),
            None,
            None,
        )
        .unwrap();
        PersonalProfilePlan {
            original: read_personal(&self.root, self.owner).unwrap(),
            defaults,
            desired: ProfileDocument::new(
                ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).unwrap(),
                SettingsLayer {
                    density: Some(ryoku_profiles::Density::Compact),
                    ..SettingsLayer::default()
                },
            )
            .canonical_bytes()
            .unwrap(),
            principal: AccountPrincipal::from_account(
                self.owner.uid,
                self.owner.gid,
                "fixture",
                "/home/fixture",
            )
            .unwrap(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = stdfs::remove_dir_all(&self.path);
    }
}
fn transaction() -> Identifier {
    Identifier::new("profile-change").unwrap()
}
fn no_payload(_: &Sha256) -> Result<Vec<u8>> {
    Err(ServiceError::RecoveryUnavailable)
}

#[test]
fn reviewed_plan_commits_to_actual_source_defaults_account_and_desired_bytes() {
    let fixture = Fixture::new();
    fixture.write(ORIGINAL, 0o600);
    let plan = fixture.plan();
    let expected = plan.reviewed_change().unwrap();
    assert_eq!(fixture.plan().reviewed_change().unwrap(), expected);
    let mut changed = fixture.plan();
    changed.desired = ORIGINAL.to_vec();
    assert_ne!(changed.reviewed_change().unwrap().plan_fingerprint, expected.plan_fingerprint);
    assert_eq!(
        changed.reviewed_change().unwrap().affected_state_fingerprint,
        expected.affected_state_fingerprint
    );
    let mut changed = fixture.plan();
    changed.principal =
        AccountPrincipal::from_account(2001, 2001, "another", "/home/another").unwrap();
    assert_ne!(changed.reviewed_change().unwrap().operation_key, expected.operation_key);
    assert_ne!(changed.reviewed_change().unwrap().plan_fingerprint, expected.plan_fingerprint);
    assert_ne!(
        changed.reviewed_change().unwrap().affected_state_fingerprint,
        expected.affected_state_fingerprint
    );
    let mut changed = fixture.plan();
    changed.defaults = InstalledProfileBundle::from_local_documents(
        changed.defaults.package().clone(),
        b"{\"schema_version\":1,\"id\":\"generic-desktop\",\"settings\":{\"theme\":\"ryoku_dark\"}}",
        None,
        None,
    )
    .unwrap();
    assert_ne!(changed.reviewed_change().unwrap().plan_fingerprint, expected.plan_fingerprint);
    fixture.write(ORIGINAL, 0o644);
    let changed = fixture.plan().reviewed_change().unwrap();
    assert_ne!(changed.plan_fingerprint, expected.plan_fingerprint);
    assert_ne!(changed.affected_state_fingerprint, expected.affected_state_fingerprint);
    let serialized = serde_json::to_string(&expected).unwrap();
    assert!(!serialized.contains("theme"));
    assert!(!serialized.contains("fixture"));
}

#[test]
fn health_record_comes_from_actual_writer_and_rejects_wrong_content_or_metadata() {
    for absent in [false, true] {
        let fixture = Fixture::new();
        if !absent {
            fixture.write(ORIGINAL, 0o644);
        }
        let plan = fixture.plan();
        let applied =
            write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| {
                Ok(())
            })
            .unwrap();
        coordination::validate_applied_record(&plan, &applied, fixture.owner).unwrap();
        assert_eq!(applied, read_personal(&fixture.root, fixture.owner).unwrap());
        for field in ["digest", "size", "uid", "gid", "mode", "hardlink", "root", "path"] {
            let mut wrong = applied.clone();
            let EntryKind::RegularFile { sha256, size, metadata, hard_link_group } =
                &mut wrong.kind
            else {
                panic!()
            };
            match field {
                "digest" => *sha256 = digest(ORIGINAL),
                "size" => *size += 1,
                "uid" => metadata.uid += 1,
                "gid" => metadata.gid += 1,
                "mode" => metadata.mode = 0o666,
                "hardlink" => *hard_link_group = Some(Identifier::new("linked").unwrap()),
                "root" => wrong.root = StateRoot::EtcRyoku,
                "path" => wrong.path = RelativePath::new("other.json").unwrap(),
                _ => unreachable!(),
            }
            assert!(
                coordination::validate_applied_record(&plan, &wrong, fixture.owner).is_err(),
                "{field}"
            );
        }
        if !absent {
            for attribute in ["acl", "selinux"] {
                let mut wrong = applied.clone();
                let EntryKind::RegularFile { metadata, .. } = &mut wrong.kind else { panic!() };
                if attribute == "acl" {
                    metadata.acl_sha256 = Some(digest(b"changed ACL"));
                } else {
                    metadata
                        .extended
                        .values
                        .insert(ryoku_recovery::XattrName::SelinuxLabel, digest(b"changed label"));
                }
                assert!(
                    coordination::validate_applied_record(&plan, &wrong, fixture.owner).is_err(),
                    "{attribute}"
                );
            }
        }
    }
}

#[test]
fn absence_is_not_a_default_write_and_first_change_creates_only_the_enrolled_file() {
    let fixture = Fixture::new();
    stdfs::write(fixture.path.join("unrelated"), b"keep").unwrap();
    let plan = fixture.plan();
    assert!(matches!(plan.original.kind, EntryKind::Absent));
    assert!(!fixture.path.join(PROFILE).exists());
    write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| Ok(()))
        .unwrap();
    assert_eq!(stdfs::read(fixture.path.join(PROFILE)).unwrap(), plan.desired);
    let stat = stdfs::metadata(fixture.path.join(PROFILE)).unwrap();
    assert_eq!(stat.permissions().mode() & 0o7777, 0o600);
    assert_eq!(stdfs::read(fixture.path.join("unrelated")).unwrap(), b"keep");
    assert!(!fixture.path.join(".ryoku-profile-profile-change.pending").exists());
}

#[test]
fn update_preserves_existing_mode_owner_and_original_capture_bytes() {
    for mode in [0o600, 0o644] {
        let fixture = Fixture::new();
        fixture.write(ORIGINAL, mode);
        let plan = fixture.plan();
        let original = plan.original.clone();
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| Ok(()))
            .unwrap();
        let current = read_personal(&fixture.root, fixture.owner).unwrap();
        let EntryKind::RegularFile { metadata, sha256, .. } = current.kind else { panic!() };
        assert_eq!(metadata.mode, mode);
        assert_eq!((metadata.uid, metadata.gid), (fixture.owner.uid, fixture.owner.gid));
        assert_eq!(sha256, digest(&plan.desired));
        assert_eq!(plan.original, original);
        let EntryKind::RegularFile { sha256, .. } = &plan.original.kind else { panic!() };
        assert_eq!(*sha256, digest(ORIGINAL));
    }
}

#[test]
fn stale_content_or_failed_guard_never_replaces_live_personal_state() {
    let fixture = Fixture::new();
    fixture.write(ORIGINAL, 0o600);
    let plan = fixture.plan();
    let calls = Cell::new(0);
    assert_eq!(
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 { Err(ServiceError::StaleRequest) } else { Ok(()) }
        }),
        Err(ServiceError::StaleRequest)
    );
    assert_eq!(stdfs::read(fixture.path.join(PROFILE)).unwrap(), ORIGINAL);
    assert!(fixture.path.join(".ryoku-profile-profile-change.pending").exists());
    // Existing incomplete staging is never adopted or truncated by a retry.
    assert!(
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| Ok(()))
            .is_err()
    );
    let fixture = Fixture::new();
    let plan = fixture.plan();
    fixture.write(ORIGINAL, 0o600);
    assert_eq!(
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| Ok(())),
        Err(ServiceError::StaleRequest)
    );
    assert_eq!(stdfs::read(fixture.path.join(PROFILE)).unwrap(), ORIGINAL);
}

#[test]
fn rejects_foreign_profile_type_mode_links_and_unreviewed_attributes() {
    let fixture = Fixture::new();
    fixture.write(ORIGINAL, 0o666);
    assert!(read_personal(&fixture.root, fixture.owner).is_err());
    fixture.write(ORIGINAL, 0o600);
    stdfs::hard_link(fixture.path.join(PROFILE), fixture.path.join("alias")).unwrap();
    assert!(read_personal(&fixture.root, fixture.owner).is_err());
    stdfs::remove_file(fixture.path.join("alias")).unwrap();
    let file = File::open(fixture.path.join(PROFILE)).unwrap();
    fs::fsetxattr(&file, "user.external", b"preserve", fs::XattrFlags::empty()).unwrap();
    assert_eq!(read_personal(&fixture.root, fixture.owner), Err(ServiceError::Unsupported));
    fs::fremovexattr(&file, "user.external").unwrap();
    stdfs::remove_file(fixture.path.join(PROFILE)).unwrap();
    symlink("/etc/passwd", fixture.path.join(PROFILE)).unwrap();
    assert!(read_personal(&fixture.root, fixture.owner).is_err());
}

#[test]
fn corrupted_staged_content_is_refused_before_live_rename() {
    let fixture = Fixture::new();
    fixture.write(ORIGINAL, 0o600);
    let plan = fixture.plan();
    let calls = Cell::new(0);
    assert!(
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                stdfs::write(
                    fixture.path.join(".ryoku-profile-profile-change.pending"),
                    b"corrupt",
                )
                .unwrap();
            }
            Ok(())
        })
        .is_err()
    );
    assert_eq!(stdfs::read(fixture.path.join(PROFILE)).unwrap(), ORIGINAL);
}

#[test]
fn held_writer_guard_is_revalidated_without_recursive_locking_and_rejects_replacement() {
    let fixture = Fixture::new();
    let lock = MutationLock::open(fixture.root.try_clone().unwrap(), fixture.owner.uid).unwrap();
    let guard = lock.acquire().unwrap();
    lock.revalidate_guard(&guard).unwrap();
    stdfs::rename(fixture.path.join("mutation.lock"), fixture.path.join("retained-lock")).unwrap();
    assert!(lock.revalidate_guard(&guard).is_err());
    assert!(!fixture.path.join("mutation.lock").exists());
    let other = MutationLock::open(fixture.root.try_clone().unwrap(), fixture.owner.uid).unwrap();
    assert!(other.revalidate_guard(&guard).is_err());
}

#[test]
fn update_replays_exact_saved_acl_and_refuses_missing_metadata_payload() {
    use ryoku_recovery::metadata::AclPayload;
    let fixture = Fixture::new();
    fixture.write(ORIGINAL, 0o644);
    let file = File::open(fixture.path.join(PROFILE)).unwrap();
    let mut acl = 2_u32.to_le_bytes().to_vec();
    for (tag, permissions, id) in [
        (1_u16, 6_u16, u32::MAX),
        (2, 4, fixture.owner.uid + 1),
        (4, 4, u32::MAX),
        (16, 4, u32::MAX),
        (32, 4, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    fs::fsetxattr(&file, "system.posix_acl_access", &acl, fs::XattrFlags::empty()).unwrap();
    let plan = fixture.plan();
    let payload =
        AclPayload { access: Some(acl.clone()), default: None }.encode().unwrap().unwrap();
    let hash = digest(&payload);
    let EntryKind::RegularFile { metadata, .. } = &plan.original.kind else { panic!() };
    assert_eq!(metadata.acl_sha256.as_ref(), Some(&hash));
    assert!(
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| Ok(()))
            .is_err()
    );
    assert_eq!(stdfs::read(fixture.path.join(PROFILE)).unwrap(), ORIGINAL);
    write_personal(
        &fixture.root,
        fixture.owner,
        &plan,
        &Identifier::new("with-acl").unwrap(),
        |wanted| {
            assert_eq!(wanted, &hash);
            Ok(payload.clone())
        },
        |_| Ok(()),
    )
    .unwrap();
    let current = File::open(fixture.path.join(PROFILE)).unwrap();
    let mut bytes = [0_u8; 256];
    let size = fs::fgetxattr(&current, "system.posix_acl_access", &mut bytes).unwrap();
    assert_eq!(&bytes[..size], acl);
}

#[test]
fn absent_creation_closes_inherited_default_acl_access_with_explicit_0600() {
    let fixture = Fixture::new();
    let mut acl = 2_u32.to_le_bytes().to_vec();
    for (tag, permissions, id) in [
        (1_u16, 7_u16, u32::MAX),
        (2, 7, fixture.owner.uid + 1),
        (4, 7, u32::MAX),
        (16, 7, u32::MAX),
        (32, 7, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    fs::fsetxattr(&fixture.root, "system.posix_acl_default", &acl, fs::XattrFlags::empty())
        .unwrap();
    let plan = fixture.plan();
    write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |_| Ok(()))
        .unwrap();
    let file = File::open(fixture.path.join(PROFILE)).unwrap();
    assert_eq!(fs::fstat(&file).unwrap().st_mode & 0o7777, 0o600);
    let mut bytes = [0_u8; 256];
    let size = fs::fgetxattr(&file, "system.posix_acl_access", &mut bytes).unwrap();
    let mut saw_mask = false;
    for entry in bytes[4..size].as_chunks::<8>().0 {
        let tag = u16::from_le_bytes(entry[..2].try_into().unwrap());
        let permissions = u16::from_le_bytes(entry[2..4].try_into().unwrap());
        if tag == 16 {
            saw_mask = true;
            assert_eq!(permissions, 0);
        }
        if tag == 32 {
            assert_eq!(permissions, 0);
        }
    }
    assert!(saw_mask);
}

#[test]
fn absent_creation_policy_failure_retains_stage_without_creating_profile() {
    let fixture = Fixture::new();
    let plan = fixture.plan();
    assert_eq!(
        write_personal(&fixture.root, fixture.owner, &plan, &transaction(), no_payload, |staged| {
            if staged.is_some() { Err(ServiceError::RecoveryUnavailable) } else { Ok(()) }
        },),
        Err(ServiceError::RecoveryUnavailable)
    );
    assert!(!fixture.path.join(PROFILE).exists());
    assert!(fixture.path.join(".ryoku-profile-profile-change.pending").exists());
}
