use super::*;
use std::{
    cell::Cell,
    fs as stdfs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const RECORD: &[u8] =
    b"# fixed native root\nSUBVOLUME=\"/\"\nFSTYPE=\"btrfs\"\nTIMELINE_CREATE=\"no\"\n";
struct Fixture {
    path: PathBuf,
    uid: u32,
    gid: u32,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryoku-root-mapping-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        stdfs::create_dir(&path).unwrap();
        stdfs::create_dir_all(path.join("etc/snapper/configs")).unwrap();
        stdfs::create_dir(path.join(".snapshots")).unwrap();
        stdfs::write(path.join("etc/snapper/configs/root"), RECORD).unwrap();
        for name in ["", "etc", "etc/snapper", "etc/snapper/configs", ".snapshots"] {
            stdfs::set_permissions(path.join(name), stdfs::Permissions::from_mode(0o700)).unwrap();
        }
        stdfs::set_permissions(
            path.join("etc/snapper/configs/root"),
            stdfs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let stat = fs::fstat(File::open(&path).unwrap()).unwrap();
        Self { path, uid: stat.st_uid, gid: stat.st_gid }
    }
    fn root(&self) -> File {
        File::open(&self.path).unwrap()
    }
    fn mapping(&self) -> RootCaptureMapping {
        RootCaptureMapping::open_with(
            self.root(),
            self.uid,
            self.gid,
            &format_uuid(&[1; 16]),
            good_query,
            fake_native,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = stdfs::remove_dir_all(&self.path);
    }
}
fn good_query() -> Result<RootConfigValues> {
    ryoku_snapper::parse_root_configuration(
        br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","TIMELINE_CREATE":"no"}"#,
    )
    .map_err(unavailable)
}
// Unit-only native observation substitution exercises composition failures;
// no public constructor accepts it and no test invokes a snapshot mutation.
fn fake_native(_: &File, _: &File) -> Result<NativeIdentity> {
    Ok(NativeIdentity {
        filesystem: [1; 16],
        root_uuid: [2; 16],
        root_id: 256,
        snapshots_uuid: [3; 16],
        snapshots_id: 257,
    })
}

#[test]
fn original_and_effective_mapping_must_both_be_root_before_capture() {
    let fixture = Fixture::new();
    fixture.mapping();
    let queries = Cell::new(0);
    for record in [
        b"SUBVOLUME=\"/home\"\nFSTYPE=\"btrfs\"\n".as_slice(),
        b"SUBVOLUME=\"/\"\nFSTYPE=\"ext4\"\n",
        b"SUBVOLUME=\"/\"\nSUBVOLUME = \"/home\"\nFSTYPE=\"btrfs\"\n",
        b"SUBVOLUME=\"/\"\nFSTYPE=\"btrfs\"\nFSTYPE=\"btrfs\"\n",
        b"SUBVOLUME=\"/\"\n",
        b"SUBVOLUME=\"/\"\nFSTYPE=\"btrfs\"\0\n",
    ] {
        stdfs::write(fixture.path.join("etc/snapper/configs/root"), record).unwrap();
        assert!(
            RootCaptureMapping::open_with(
                fixture.root(),
                fixture.uid,
                fixture.gid,
                &format_uuid(&[1; 16]),
                || {
                    queries.set(queries.get() + 1);
                    good_query()
                },
                fake_native
            )
            .is_err()
        );
    }
    assert_eq!(queries.get(), 0);
    stdfs::write(fixture.path.join("etc/snapper/configs/root"), RECORD).unwrap();
    assert!(
        RootCaptureMapping::open_with(
            fixture.root(),
            fixture.uid,
            fixture.gid,
            &format_uuid(&[1; 16]),
            || {
                ryoku_snapper::parse_root_configuration(
                    br#"{"SUBVOLUME":"/home","FSTYPE":"btrfs"}"#,
                )
                .map_err(unavailable)
            },
            fake_native
        )
        .is_err()
    );
    assert!(
        RootCaptureMapping::open_with(
            fixture.root(),
            fixture.uid,
            fixture.gid,
            &format_uuid(&[9; 16]),
            good_query,
            fake_native
        )
        .is_err()
    );
}

#[test]
fn stale_native_number_collision_is_refused_even_with_valid_effective_values() {
    let fixture = Fixture::new();
    stdfs::create_dir_all(fixture.path.join(".snapshots/42/snapshot")).unwrap();
    let mapping = fixture.mapping();
    // A foreign config could claim that 42 is fresh in *its* inventory. The
    // actual native baseline independently proves that this number was old.
    assert_eq!(mapping.require_fresh_number("42"), Err(RootMappingError::ExistingSnapshot));
    mapping.require_fresh_number("43").unwrap();
    for bad in ["0", "043", "../43", "-1", "18446744073709551616"] {
        assert!(mapping.require_fresh_number(bad).is_err());
    }
    // Later creation does not rewrite the immutable pre-create baseline.
    stdfs::create_dir(fixture.path.join(".snapshots/43")).unwrap();
    mapping.require_fresh_number("43").unwrap();
}

#[test]
fn original_record_replacement_changes_and_effective_changes_are_refused() {
    let fixture = Fixture::new();
    let mapping = fixture.mapping();
    mapping.revalidate_with(fixture.uid, fixture.gid, good_query, fake_native).unwrap();
    assert_eq!(
        mapping.revalidate_with(
            fixture.uid,
            fixture.gid,
            || {
                ryoku_snapper::parse_root_configuration(
                    br#"{"SUBVOLUME":"/","FSTYPE":"btrfs","TIMELINE_CREATE":"yes"}"#,
                )
                .map_err(unavailable)
            },
            fake_native
        ),
        Err(RootMappingError::Changed)
    );
    assert_eq!(
        mapping.revalidate_with(
            fixture.uid,
            fixture.gid,
            || Err(RootMappingError::Unavailable),
            fake_native
        ),
        Err(RootMappingError::Unavailable)
    );
    stdfs::write(
        fixture.path.join("etc/snapper/configs/root"),
        b"SUBVOLUME=\"/\"\nFSTYPE=\"btrfs\"\n",
    )
    .unwrap();
    assert_eq!(
        mapping.revalidate_with(fixture.uid, fixture.gid, good_query, fake_native),
        Err(RootMappingError::Changed)
    );
    stdfs::write(fixture.path.join("etc/snapper/configs/root"), RECORD).unwrap();
    stdfs::rename(
        fixture.path.join("etc/snapper/configs/root"),
        fixture.path.join("retained-config"),
    )
    .unwrap();
    stdfs::write(fixture.path.join("etc/snapper/configs/root"), RECORD).unwrap();
    stdfs::set_permissions(
        fixture.path.join("etc/snapper/configs/root"),
        stdfs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        mapping.revalidate_with(fixture.uid, fixture.gid, good_query, fake_native),
        Err(RootMappingError::Changed)
    );
}

#[test]
fn config_change_during_effective_query_and_namespace_substitution_fail() {
    let fixture = Fixture::new();
    let mapping = fixture.mapping();
    assert_eq!(
        mapping.revalidate_with(
            fixture.uid,
            fixture.gid,
            || {
                stdfs::write(
                    fixture.path.join("etc/snapper/configs/root"),
                    b"SUBVOLUME=\"/home\"\nFSTYPE=\"btrfs\"\n",
                )
                .unwrap();
                good_query()
            },
            fake_native
        ),
        Err(RootMappingError::WrongMapping)
    );
    stdfs::write(fixture.path.join("etc/snapper/configs/root"), RECORD).unwrap();
    stdfs::rename(fixture.path.join(".snapshots"), fixture.path.join("retained-snapshots"))
        .unwrap();
    stdfs::create_dir(fixture.path.join(".snapshots")).unwrap();
    stdfs::set_permissions(fixture.path.join(".snapshots"), stdfs::Permissions::from_mode(0o700))
        .unwrap();
    assert_eq!(
        mapping.revalidate_with(fixture.uid, fixture.gid, good_query, fake_native),
        Err(RootMappingError::Changed)
    );
}

#[test]
fn unsafe_records_links_parents_and_numeric_aliases_fail_without_repair() {
    let fixture = Fixture::new();
    let path = fixture.path.join("etc/snapper/configs/root");
    stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o666)).unwrap();
    assert!(MappingFiles::read(&fixture.root(), fixture.uid, fixture.gid).is_err());
    stdfs::set_permissions(&path, stdfs::Permissions::from_mode(0o600)).unwrap();
    stdfs::hard_link(&path, fixture.path.join("alias")).unwrap();
    assert!(MappingFiles::read(&fixture.root(), fixture.uid, fixture.gid).is_err());
    stdfs::remove_file(fixture.path.join("alias")).unwrap();
    stdfs::rename(&path, fixture.path.join("retained")).unwrap();
    symlink(fixture.path.join("retained"), &path).unwrap();
    assert!(MappingFiles::read(&fixture.root(), fixture.uid, fixture.gid).is_err());
    let fixture = Fixture::new();
    stdfs::create_dir(fixture.path.join(".snapshots/01")).unwrap();
    assert!(
        numeric_entries(
            &File::open(fixture.path.join(".snapshots")).unwrap(),
            fixture.uid,
            fixture.gid
        )
        .is_err()
    );
    stdfs::remove_dir(fixture.path.join(".snapshots/01")).unwrap();
    symlink("../etc", fixture.path.join(".snapshots/1")).unwrap();
    assert!(
        numeric_entries(
            &File::open(fixture.path.join(".snapshots")).unwrap(),
            fixture.uid,
            fixture.gid
        )
        .is_err()
    );
}

#[test]
fn inventory_bound_counts_non_numeric_entries_and_never_scans_recursively() {
    let fixture = Fixture::new();
    stdfs::create_dir_all(fixture.path.join(".snapshots/1/unrelated/deep/tree")).unwrap();
    let snapshots = File::open(fixture.path.join(".snapshots")).unwrap();
    assert_eq!(numeric_entries(&snapshots, fixture.uid, fixture.gid).unwrap(), BTreeSet::from([1]));
    for index in 0..ENTRY_LIMIT {
        stdfs::write(fixture.path.join(format!(".snapshots/ignored-{index}")), b"").unwrap();
    }
    assert_eq!(
        numeric_entries(&snapshots, fixture.uid, fixture.gid),
        Err(RootMappingError::InventoryLimit)
    );
}

#[test]
fn native_identity_change_and_record_rewrite_invalidate_original_observation() {
    let fixture = Fixture::new();
    let mapping = fixture.mapping();
    assert_eq!(
        mapping.revalidate_with(fixture.uid, fixture.gid, good_query, |root, snapshots| {
            let mut observed = fake_native(root, snapshots)?;
            observed.snapshots_uuid = [9; 16];
            Ok(observed)
        }),
        Err(RootMappingError::Changed)
    );
    stdfs::write(fixture.path.join("etc/snapper/configs/root"), RECORD).unwrap();
    assert_eq!(
        mapping.revalidate_with(fixture.uid, fixture.gid, good_query, fake_native),
        Err(RootMappingError::Changed)
    );
}
