//! Root console installer for the fixed Ryoku RPM set.
//! Mutation execution uses the shared durable coordinator and native linked checkpoints.

mod lifecycle;

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::{Command, ExitCode},
};

use rustix::fs::{AtFlags, FileType, OFlags, ResolveFlags, fstat, openat2, statat};
use rustix::fs::{Mode, open};
use ryoku_profiles::classify_legacy_personal_profile;
use ryoku_recovery::{
    generation::GenerationReader, machine_state::read_machine_state_record,
    restore::RestoreJournalStore, state_mount::collect_and_open_state_mount,
};
use ryoku_snapper::LayoutCapability;
use sha2::{Digest, Sha256};

const REQUIRED: &[&str] = &[
    "ryoku-desktop",
    "ryoku-desktop-runtime",
    "ryoku-desktop-fonts",
    "ryoku-helper",
    "ryoku-tools",
    "ryoku-profile-defaults",
    "ryoku-quickshell",
];
const OPTIONAL: &[&str] = &["ryoku-ui-proof", "ryoku-tools-launcher-status"];
const KNOWN_INSTALLED: &[&str] = &[
    "ryoku-desktop",
    "ryoku-desktop-runtime",
    "ryoku-desktop-fonts",
    "ryoku-helper",
    "ryoku-tools",
    "ryoku-tools-state-init",
    "ryoku-ui-proof",
    "ryoku-tools-launcher-status",
    "ryoku-profile-defaults",
    "ryoku-quickshell",
];
const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_PACKAGE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_QUERY_BYTES: usize = 4 * 1024 * 1024;
const MAX_SOLVER_BYTES: usize = 2 * 1024 * 1024;

pub fn main() -> ExitCode {
    match execute(env::args_os().skip(1).collect(), &mut io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "ryoku-install: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute(arguments: Vec<std::ffi::OsString>, output: &mut impl Write) -> Result<(), String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("run installer commands as root".into());
    }
    if arguments.first().is_some_and(|arg| {
        matches!(
            arg.to_str(),
            Some(
                "install"
                    | "remove"
                    | "migrate"
                    | "lifecycle-preview"
                    | "internal-lifecycle"
                    | "recover"
            )
        )
    }) {
        return lifecycle::execute(&arguments, output);
    }
    if arguments.len() == 1 && arguments[0] == "remove-preview" {
        return remove_preview(output);
    }
    if arguments.len() == 2 && arguments[0] == "migration-preview" {
        let uid = arguments[1]
            .to_str()
            .ok_or("UID must be ASCII decimal")?
            .parse::<u32>()
            .map_err(|_| "UID must be ASCII decimal")?;
        validate_host()?;
        return migration_preview(uid, output);
    }
    if arguments.len() < 2 || arguments[0] != "preview" {
        return Err(
            "Usage: sudo ryoku-install preview RPM [RPM ...] | sudo ryoku-install remove-preview | sudo ryoku-install migration-preview UID"
                .into(),
        );
    }
    let inputs = validate_inputs(&arguments[1..])?;
    validate_host()?;
    let installed = installed_ryoku_packages()?;
    // Inspect and constrain every package before asking the native solver to
    // consider it. This diagnostic solver result stays separate from the exact
    // local RPM transaction revalidated by the protected executor.
    let packages = validate_packages(&inputs.inputs)?;
    let payload_ownership_sha256 = validate_payload_file_conflicts(&packages)?;
    let directory_snapshot_sha256 = validate_payload_directory_types(&packages)?;
    let solver = run_zypper_preview(&inputs.inputs)?;
    if installed_ryoku_packages()? != installed {
        return Err("installed Ryoku packages changed during the install preview".into());
    }
    if validate_payload_file_conflicts(&packages)? != payload_ownership_sha256
        || validate_payload_directory_types(&packages)? != directory_snapshot_sha256
    {
        return Err("payload file ownership changed during the install preview".into());
    }
    let plan_payload_snapshot = format!("{payload_ownership_sha256}:{directory_snapshot_sha256}");
    let plan_sha256 = install_plan_sha256(&installed, &packages, &solver, &plan_payload_snapshot);

    writeln!(output, "install_preflight=ready-for-review")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(
        output,
        "existing_installation={}",
        if installed.is_empty() { "fresh" } else { "recognized" }
    )
    .map_err(|_| "could not write preview".to_owned())?;
    for (name, nevra) in &installed {
        writeln!(output, "installed_package name={name} nevra={nevra}")
            .map_err(|_| "could not write preview".to_owned())?;
    }
    writeln!(output, "package_source=operator-selected-local-rpms")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "package_signature_policy=zypper-default-gpg-checks-enabled")
        .map_err(|_| "could not write preview".to_owned())?;
    // The direct collision check intentionally covers only regular files and
    // symlinks. Do not let the summary imply that directory metadata or the
    // transitive dependency payload has received the same ownership review.
    writeln!(output, "direct_file_payload_ownership_conflicts=none")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "direct_directory_metadata_conflicts=none")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "dependency_payload_conflicts=solver_preview_only")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "payload_ownership_snapshot_sha256={payload_ownership_sha256}")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "payload_directory_snapshot_sha256={directory_snapshot_sha256}")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "direct_directory_type_conflicts=none")
        .map_err(|_| "could not write preview".to_owned())?;
    for package in &packages {
        writeln!(
            output,
            "package={} current={} target={}",
            package.name,
            installed.get(&package.name).map(String::as_str).unwrap_or("absent"),
            package.nevra
        )
        .map_err(|_| "could not write preview".to_owned())?;
        writeln!(
            output,
            "package_source_sha256 name={} digest={}",
            package.name,
            package.source_sha256.as_deref().ok_or("package source digest unavailable")?
        )
        .map_err(|_| "could not write preview".to_owned())?;
        writeln!(
            output,
            "package_policy name={} scriptlets=none triggers=none filetriggers=none",
            package.name
        )
        .map_err(|_| "could not write preview".to_owned())?;
        for path in &package.payload_paths {
            writeln!(output, "payload package={} path={}", package.name, path)
                .map_err(|_| "could not write preview".to_owned())?;
        }
        for path in &package.config_paths {
            writeln!(output, "config package={} path={}", package.name, path)
                .map_err(|_| "could not write preview".to_owned())?;
        }
    }
    writeln!(
        output,
        "direct_package_payload_prefix=/usr direct_package_home_payload=none user_overrides=not_modified_by_preview dependencies=listed_in_solver_preview"
    )
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "solver_preview_begin").map_err(|_| "could not write preview".to_owned())?;

    output.write_all(&solver).map_err(|_| "could not write solver preview".to_owned())?;
    if !solver.ends_with(b"\n") {
        writeln!(output).map_err(|_| "could not write solver preview".to_owned())?;
    }
    writeln!(output, "solver_preview_end").map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "install_plan_sha256={plan_sha256}")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "mutation=not_performed checkpoint=not_created authorization=not_requested")
        .map_err(|_| "could not write preview".to_owned())?;
    Ok(())
}

fn migration_preview(uid: u32, output: &mut impl Write) -> Result<(), String> {
    if !(1_000..65_534).contains(&uid) {
        return Err("migration preview requires an ordinary account UID".into());
    }
    let uid_arg = uid.to_string();
    let account = bounded_command(
        "/usr/bin/getent",
        &[std::ffi::OsStr::new("passwd"), std::ffi::OsStr::new(&uid_arg)],
        5,
        16 * 1024,
    )?;
    if !account.status.success() {
        return Err("selected account is not present in NSS".into());
    }
    let text =
        std::str::from_utf8(&account.stdout).map_err(|_| "NSS account record is not UTF-8")?;
    let line = text.strip_suffix('\n').ok_or("NSS account record is malformed")?;
    if line.bytes().any(|byte| matches!(byte, b'\n' | b'\r')) {
        return Err("NSS returned multiple account records for the selected UID".into());
    }
    let fields: Vec<_> = line.split(':').collect();
    if fields.len() != 7 || fields[2].parse::<u32>().ok() != Some(uid) {
        return Err("NSS returned an ambiguous account record".into());
    }
    let gid = fields[3].parse::<u32>().map_err(|_| "NSS primary GID is invalid")?;
    let home = fields[5];
    if !home.starts_with('/')
        || home == "/"
        || home.split('/').any(|part| part == "." || part == "..")
        || home.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err("NSS home path is unsupported".into());
    }

    let root = File::open("/").map_err(|_| "filesystem root is unavailable")?;
    let home_rel = home.trim_start_matches('/');
    let home_fd = File::from(
        openat2(
            &root,
            home_rel,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|_| "selected account home cannot be opened without following links")?,
    );
    let home_stat = fstat(&home_fd).map_err(|_| "selected account home cannot be inspected")?;
    if FileType::from_raw_mode(home_stat.st_mode) != FileType::Directory || home_stat.st_uid != uid
    {
        return Err("selected account home has unexpected owner or type".into());
    }
    match inspect_legacy_profile(&home_fd, uid, gid)? {
        MigrationAssessment::NoCandidate => {
            writeln!(output, "migration_candidate=none source=absent target=unchanged")
                .map_err(|_| "could not write migration preview".to_owned())?;
        }
        MigrationAssessment::Unrecognized => {
            writeln!(output, "migration_candidate=unrecognized source=preserved target=unchanged mutation=not_performed")
                .map_err(|_| "could not write migration preview".to_owned())?;
        }
        MigrationAssessment::Recognized { source_sha256, source_mode } => {
            writeln!(output, "migration_candidate=recognized schema=1 source_sha256={source_sha256} source_mode={source_mode:04o} target=absent mutation=not_performed checkpoint=not_created")
                .map_err(|_| "could not write migration preview".to_owned())?;
        }
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum MigrationAssessment {
    NoCandidate,
    Unrecognized,
    Recognized { source_sha256: String, source_mode: u32 },
}

fn inspect_legacy_profile(home: &File, uid: u32, gid: u32) -> Result<MigrationAssessment, String> {
    let Some(config) = open_optional_directory(home, ".config", uid, gid)? else {
        return Ok(MigrationAssessment::NoCandidate);
    };
    let Some(ryoku) = open_optional_directory(&config, "ryoku", uid, gid)? else {
        return Ok(MigrationAssessment::NoCandidate);
    };
    let source = match openat2(
        &ryoku,
        "profile.json",
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(MigrationAssessment::NoCandidate),
        Err(_) => return Err("legacy profile cannot be opened safely".into()),
    };
    let source_stat = fstat(&source).map_err(|_| "legacy profile cannot be inspected")?;
    let mode = source_stat.st_mode & 0o7777;
    if FileType::from_raw_mode(source_stat.st_mode) != FileType::RegularFile
        || source_stat.st_uid != uid
        || source_stat.st_gid != gid
        || source_stat.st_nlink != 1
        || !matches!(mode, 0o600 | 0o644)
        || source_stat.st_size < 0
        || source_stat.st_size as usize > ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES
    {
        return Err("legacy profile has unsupported owner, type, mode, links or size".into());
    }
    let mut bytes = Vec::with_capacity(source_stat.st_size as usize);
    (&source)
        .take((ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "legacy profile cannot be read")?;
    let after = fstat(&source).map_err(|_| "legacy profile cannot be rechecked")?;
    if after.st_dev != source_stat.st_dev
        || after.st_ino != source_stat.st_ino
        || after.st_mode != source_stat.st_mode
        || after.st_uid != source_stat.st_uid
        || after.st_gid != source_stat.st_gid
        || after.st_nlink != source_stat.st_nlink
        || after.st_size != source_stat.st_size
        || after.st_mtime != source_stat.st_mtime
        || after.st_mtime_nsec != source_stat.st_mtime_nsec
        || after.st_ctime != source_stat.st_ctime
        || after.st_ctime_nsec != source_stat.st_ctime_nsec
        || bytes.len() != after.st_size as usize
    {
        return Err("legacy profile changed during migration preview".into());
    }
    match classify_legacy_personal_profile(&bytes) {
        Some(candidate) => {
            match statat(&ryoku, "opensuse-profile.json", AtFlags::SYMLINK_NOFOLLOW) {
                Err(rustix::io::Errno::NOENT) => (),
                Ok(_) => {
                    return Err("port profile already exists; migration requires review".into());
                }
                Err(_) => return Err("port profile destination cannot be inspected safely".into()),
            }
            Ok(MigrationAssessment::Recognized {
                source_sha256: candidate.source_sha256,
                source_mode: mode,
            })
        }
        None => Ok(MigrationAssessment::Unrecognized),
    }
}

fn open_optional_directory(
    parent: &File,
    name: &str,
    uid: u32,
    gid: u32,
) -> Result<Option<File>, String> {
    let directory = match openat2(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(directory) => File::from(directory),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err("Ryoku configuration directory is unsafe".into()),
    };
    let metadata = fstat(&directory).map_err(|_| "Ryoku configuration directory is unsafe")?;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
        || metadata.st_uid != uid
        || metadata.st_gid != gid
        || !matches!(metadata.st_mode & 0o7777, 0o700 | 0o755)
    {
        return Err("Ryoku configuration directory has unsupported ownership or mode".into());
    }
    Ok(Some(directory))
}

fn remove_preview(output: &mut impl Write) -> Result<(), String> {
    validate_host()?;
    let installed = installed_ryoku_packages()?;
    if installed.is_empty() {
        return Err("no recognized Ryoku RPMs are installed".into());
    }
    let mut packages = Vec::with_capacity(installed.len());
    for (name, expected_nevra) in &installed {
        packages.push(inspect_installed_package(name, expected_nevra)?);
    }
    let ownership = installed_file_owners()?;
    let payload_dispositions = removal_payload_ownership(&packages, &ownership)?;
    if installed_ryoku_packages()? != installed {
        return Err("installed Ryoku packages changed during the removal preview".into());
    }
    let solver = run_zypper_remove_preview(&installed)?;
    if installed_ryoku_packages()? != installed || installed_file_owners()? != ownership {
        return Err("installed Ryoku packages changed during the removal preview".into());
    }

    writeln!(output, "remove_preflight=ready-for-review")
        .map_err(|_| "could not write preview".to_owned())?;
    for package in &packages {
        writeln!(output, "remove_package={} installed={}", package.name, package.nevra)
            .map_err(|_| "could not write preview".to_owned())?;
        writeln!(
            output,
            "package_policy name={} scriptlets=none triggers=none filetriggers=none",
            package.name
        )
        .map_err(|_| "could not write preview".to_owned())?;
        for path in &package.payload_paths {
            let disposition = payload_dispositions
                .get(path)
                .ok_or("installed payload disposition is unavailable")?;
            writeln!(
                output,
                "remove_payload_ownership package={} path={} disposition={}",
                package.name,
                path,
                disposition.as_str()
            )
            .map_err(|_| "could not write preview".to_owned())?;
        }
        for path in &package.config_paths {
            writeln!(output, "remove_config package={} path={}", package.name, path)
                .map_err(|_| "could not write preview".to_owned())?;
        }
    }
    writeln!(output, "user_home_payload=none protected_state=retained clean_dependencies=disabled")
        .map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "solver_preview_begin").map_err(|_| "could not write preview".to_owned())?;
    output.write_all(&solver).map_err(|_| "could not write solver preview".to_owned())?;
    if !solver.ends_with(b"\n") {
        writeln!(output).map_err(|_| "could not write solver preview".to_owned())?;
    }
    writeln!(output, "solver_preview_end").map_err(|_| "could not write preview".to_owned())?;
    writeln!(output, "mutation=not_performed checkpoint=not_created authorization=not_requested")
        .map_err(|_| "could not write preview".to_owned())?;
    Ok(())
}

fn installed_ryoku_packages() -> Result<BTreeMap<String, String>, String> {
    let output = bounded_command(
        "/usr/bin/rpm",
        &[
            std::ffi::OsStr::new("-qa"),
            std::ffi::OsStr::new("--qf"),
            std::ffi::OsStr::new("%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\\n"),
        ],
        20,
        MAX_QUERY_BYTES,
    )?;
    if !output.status.success() {
        return Err("installed RPM inventory query failed".into());
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| "invalid installed RPM inventory")?;
    let mut installed = BTreeMap::new();
    for line in text.lines() {
        let (name, nevra) = line.split_once('\t').ok_or("malformed installed RPM inventory")?;
        if name.starts_with("ryoku-") {
            if !KNOWN_INSTALLED.contains(&name) {
                return Err(format!(
                    "unrecognized installed Ryoku package requires explicit migration review: {name}"
                ));
            }
            ryoku_recovery::Revision::new(nevra.to_owned())
                .map_err(|_| "invalid installed Ryoku package revision")?;
            let arch = nevra.rsplit_once('.').map(|(_, arch)| arch).unwrap_or_default();
            if !matches!(arch, "x86_64" | "noarch") {
                return Err(format!("unsupported installed Ryoku package architecture: {name}"));
            }
            if installed.insert(name.to_owned(), nevra.to_owned()).is_some() {
                return Err(format!("multiple installed revisions found for {name}"));
            }
        }
    }
    Ok(installed)
}

#[derive(Clone, Debug)]
struct Package {
    name: String,
    nevra: String,
    source_sha256: Option<String>,
    payload_paths: Vec<String>,
    payload_file_paths: Vec<String>,
    payload_directories: Vec<DirectoryPayload>,
    config_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryPayload {
    path: String,
    mode: u32,
    uid: u32,
    gid: u32,
}

struct PackageInput {
    path: PathBuf,
    sha256: String,
}

struct StagedPackageInputs {
    _directory: PrivatePackageDirectory,
    inputs: Vec<PackageInput>,
}

struct PrivatePackageDirectory(PathBuf);

impl Drop for PrivatePackageDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn validate_inputs(paths: &[std::ffi::OsString]) -> Result<StagedPackageInputs, String> {
    validate_inputs_in(paths, PathBuf::from("/var/tmp").as_path())
}

fn validate_inputs_in(
    paths: &[std::ffi::OsString],
    staging_parent: &std::path::Path,
) -> Result<StagedPackageInputs, String> {
    if paths.len() < REQUIRED.len() || paths.len() > REQUIRED.len() + OPTIONAL.len() {
        return Err(
            "provide every required Ryoku RPM and only explicitly selected optional RPMs".into()
        );
    }
    let directory = create_private_package_directory(staging_parent)?;
    let mut inputs = Vec::with_capacity(paths.len());
    let mut total_bytes = 0_u64;
    for (index, argument) in paths.iter().enumerate() {
        let original = PathBuf::from(argument);
        let source_fd = open(
            &original,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| "an RPM path is unavailable or is a symlink")?;
        let source = File::from(source_fd);
        let before = fstat(&source).map_err(|_| "an RPM input cannot be inspected")?;
        if FileType::from_raw_mode(before.st_mode) != FileType::RegularFile
            || before.st_size <= 0
            || before.st_size as u64 > MAX_PACKAGE_BYTES
            || before.st_nlink == 0
        {
            return Err("RPM inputs must be regular files within the size limit".into());
        }
        total_bytes = total_bytes
            .checked_add(before.st_size as u64)
            .filter(|total| *total <= MAX_TOTAL_PACKAGE_BYTES)
            .ok_or("total RPM inputs exceed the staging size limit")?;
        let mut bytes = Vec::with_capacity(before.st_size as usize);
        (&source)
            .take(MAX_PACKAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "an RPM input could not be read")?;
        let after = fstat(&source).map_err(|_| "an RPM input could not be rechecked")?;
        if !same_file_version(&before, &after) || bytes.len() != after.st_size as usize {
            return Err("an RPM input changed during preflight".into());
        }

        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let path = directory.0.join(format!("package-{index}.rpm"));
        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| "could not stage RPM inputs privately")?;
        staged
            .write_all(&bytes)
            .and_then(|()| staged.sync_all())
            .map_err(|_| "could not persist staged RPM input")?;
        drop(staged);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| "could not verify staged RPM input")?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() != 1
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() != bytes.len() as u64
        {
            return Err("staged RPM input has unsafe metadata".into());
        }
        let mut readback = Vec::with_capacity(bytes.len());
        File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut readback))
            .map_err(|_| "could not read back staged RPM input")?;
        if readback != bytes || format!("{:x}", Sha256::digest(&readback)) != sha256 {
            return Err("staged RPM input differs from the selected file snapshot".into());
        }
        inputs.push(PackageInput { path, sha256 });
    }
    Ok(StagedPackageInputs { _directory: directory, inputs })
}

fn create_private_package_directory(
    parent: &std::path::Path,
) -> Result<PrivatePackageDirectory, String> {
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|_| "RPM staging parent is unavailable")?;
    let parent_mode = parent_metadata.mode() & 0o7777;
    let parent_safe = if parent == std::path::Path::new("/var/tmp") {
        parent_metadata.uid() == 0 && parent_mode == 0o1777
    } else {
        parent_metadata.uid() == rustix::process::geteuid().as_raw() && parent_mode & 0o077 == 0
    };
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() || !parent_safe {
        return Err("RPM staging parent has unsafe ownership or permissions".into());
    }
    let mut nonce = [0_u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut nonce))
        .map_err(|_| "secure temporary name generation failed")?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = parent.join(format!("ryoku-install-{suffix}"));
    fs::create_dir(&path).map_err(|_| "could not create private RPM staging directory")?;
    let directory = PrivatePackageDirectory(path);
    fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700))
        .map_err(|_| "could not secure private RPM staging directory")?;
    let metadata = fs::symlink_metadata(&directory.0)
        .map_err(|_| "could not verify private RPM staging directory")?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err("private RPM staging directory has unsafe metadata".into());
    }
    Ok(directory)
}

fn same_file_version(before: &rustix::fs::Stat, after: &rustix::fs::Stat) -> bool {
    before.st_dev == after.st_dev
        && before.st_ino == after.st_ino
        && before.st_mode == after.st_mode
        && before.st_uid == after.st_uid
        && before.st_gid == after.st_gid
        && before.st_nlink == after.st_nlink
        && before.st_size == after.st_size
        && before.st_mtime == after.st_mtime
        && before.st_mtime_nsec == after.st_mtime_nsec
        && before.st_ctime == after.st_ctime
        && before.st_ctime_nsec == after.st_ctime_nsec
}

fn validate_packages(inputs: &[PackageInput]) -> Result<Vec<Package>, String> {
    let mut packages = Vec::with_capacity(inputs.len());
    for input in inputs {
        let path = &input.path;
        let output = bounded_command(
            "/usr/bin/rpm",
            &[
                std::ffi::OsStr::new("-qp"),
                std::ffi::OsStr::new("--qf"),
                std::ffi::OsStr::new(
                    "%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\\n[%{FILENAMES}\\n]",
                ),
                path.as_os_str(),
            ],
            20,
            MAX_QUERY_BYTES,
        )?;
        if !output.status.success() {
            return Err("RPM metadata query failed".into());
        }
        for option in ["--scripts", "--triggers", "--filetriggers"] {
            let policy_output = bounded_command(
                "/usr/bin/rpm",
                &[std::ffi::OsStr::new("-qp"), std::ffi::OsStr::new(option), path.as_os_str()],
                20,
                16 * 1024,
            )?;
            if !policy_output.status.success() || !policy_output.stdout.is_empty() {
                return Err(format!("RPM contains executable package policy: {option}"));
            }
        }
        let text = std::str::from_utf8(&output.stdout).map_err(|_| "invalid RPM metadata")?;
        let mut lines = text.lines();
        let header = lines.next().ok_or("RPM metadata is empty")?;
        let (name, nevra) = header.split_once('\t').ok_or("RPM metadata is malformed")?;
        ryoku_recovery::Revision::new(nevra.to_owned())
            .map_err(|_| "RPM NEVRA contains invalid characters")?;
        let arch =
            nevra.rsplit_once('.').map(|(_, arch)| arch).ok_or("RPM architecture is missing")?;
        if !matches!(arch, "x86_64" | "noarch") {
            return Err(format!("unsupported RPM architecture for {name}"));
        }
        let allowed = REQUIRED.contains(&name) || OPTIONAL.contains(&name);
        if !allowed || packages.iter().any(|existing: &Package| existing.name == name) {
            return Err(format!("unexpected or duplicate RPM package: {name}"));
        }
        let mut payload_paths = Vec::new();
        for package_path in lines {
            if !is_canonical_managed_payload_path(package_path) {
                return Err(format!("RPM contains an out-of-scope managed path: {package_path}"));
            }
            payload_paths.push(package_path.to_owned());
        }
        if payload_paths.is_empty() {
            return Err(format!("RPM contains no payload paths: {name}"));
        }
        let modes = bounded_command(
            "/usr/bin/rpm",
            &[
                std::ffi::OsStr::new("-qp"),
                std::ffi::OsStr::new("--qf"),
                std::ffi::OsStr::new(
                    "[%{FILEMODES}\t%{FILEUSERNAME}\t%{FILEGROUPNAME}\t%{FILENAMES}\n]",
                ),
                path.as_os_str(),
            ],
            20,
            MAX_QUERY_BYTES,
        )?;
        if !modes.status.success() {
            return Err("RPM payload mode query failed".into());
        }
        let mode_text = std::str::from_utf8(&modes.stdout).map_err(|_| "invalid RPM mode list")?;
        let (payload_file_paths, payload_directories) =
            parse_payload_paths(mode_text, &payload_paths)?;
        let config_output = bounded_command(
            "/usr/bin/rpm",
            &[std::ffi::OsStr::new("-qcp"), path.as_os_str()],
            20,
            MAX_QUERY_BYTES,
        )?;
        if !config_output.status.success() {
            return Err("RPM configuration-file query failed".into());
        }
        let config_text = std::str::from_utf8(&config_output.stdout)
            .map_err(|_| "invalid RPM configuration-file list")?;
        let config_paths: Vec<_> = config_text.lines().map(str::to_owned).collect();
        if config_paths.iter().any(|path| !payload_paths.contains(path)) {
            return Err("RPM configuration list is inconsistent with its payload".into());
        }
        packages.push(Package {
            name: name.to_owned(),
            nevra: nevra.to_owned(),
            source_sha256: Some(input.sha256.clone()),
            payload_paths,
            payload_file_paths,
            payload_directories,
            config_paths,
        });
    }
    for name in REQUIRED {
        if !packages.iter().any(|package| package.name == *name) {
            return Err(format!("required RPM is missing: {name}"));
        }
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(packages)
}

fn install_plan_sha256(
    installed: &BTreeMap<String, String>,
    packages: &[Package],
    solver: &[u8],
    payload_ownership_sha256: &str,
) -> String {
    fn field(hash: &mut Sha256, value: &[u8]) {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }

    let mut hash = Sha256::new();
    field(&mut hash, b"ryoku-install-plan-v1");
    field(&mut hash, b"installed");
    field(&mut hash, &(installed.len() as u64).to_le_bytes());
    for (name, nevra) in installed {
        field(&mut hash, name.as_bytes());
        field(&mut hash, nevra.as_bytes());
    }
    field(&mut hash, b"selected-packages");
    field(&mut hash, &(packages.len() as u64).to_le_bytes());
    for package in packages {
        field(&mut hash, package.name.as_bytes());
        field(&mut hash, package.nevra.as_bytes());
        field(&mut hash, package.source_sha256.as_deref().unwrap_or("").as_bytes());
        field(&mut hash, &(package.payload_paths.len() as u64).to_le_bytes());
        for path in &package.payload_paths {
            field(&mut hash, path.as_bytes());
        }
        field(&mut hash, &(package.payload_file_paths.len() as u64).to_le_bytes());
        for path in &package.payload_file_paths {
            field(&mut hash, path.as_bytes());
        }
        field(&mut hash, b"config-paths");
        field(&mut hash, &(package.config_paths.len() as u64).to_le_bytes());
        for path in &package.config_paths {
            field(&mut hash, path.as_bytes());
        }
    }
    field(&mut hash, b"payload-ownership-snapshot");
    field(&mut hash, payload_ownership_sha256.as_bytes());
    field(&mut hash, b"solver-preview");
    field(&mut hash, solver);
    format!("{:x}", hash.finalize())
}

fn inspect_installed_package(name: &str, expected_nevra: &str) -> Result<Package, String> {
    let query = bounded_command(
        "/usr/bin/rpm",
        &[
            std::ffi::OsStr::new("-q"),
            std::ffi::OsStr::new("--qf"),
            std::ffi::OsStr::new("%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\\n"),
            std::ffi::OsStr::new(name),
        ],
        20,
        4096,
    )?;
    if !query.status.success()
        || std::str::from_utf8(&query.stdout).map_err(|_| "invalid installed RPM metadata")?
            != format!("{name}\t{expected_nevra}\n")
    {
        return Err(format!("installed RPM changed during inspection: {name}"));
    }
    let files = bounded_command(
        "/usr/bin/rpm",
        &[std::ffi::OsStr::new("-ql"), std::ffi::OsStr::new(name)],
        20,
        MAX_QUERY_BYTES,
    )?;
    if !files.status.success() {
        return Err(format!("could not inventory installed package files: {name}"));
    }
    let files_text =
        std::str::from_utf8(&files.stdout).map_err(|_| "invalid installed package file list")?;
    let payload_paths: Vec<_> = files_text.lines().map(str::to_owned).collect();
    if payload_paths.is_empty()
        || payload_paths.iter().any(|path| !is_canonical_managed_payload_path(path))
    {
        return Err(format!("installed package contains an out-of-scope path: {name}"));
    }
    let modes = bounded_command(
        "/usr/bin/rpm",
        &[
            std::ffi::OsStr::new("-q"),
            std::ffi::OsStr::new("--qf"),
            std::ffi::OsStr::new(
                "[%{FILEMODES}\\t%{FILEUSERNAME}\\t%{FILEGROUPNAME}\\t%{FILENAMES}\\n]",
            ),
            std::ffi::OsStr::new(name),
        ],
        20,
        MAX_QUERY_BYTES,
    )?;
    if !modes.status.success() {
        return Err(format!("could not inventory installed payload types: {name}"));
    }
    let mode_text = std::str::from_utf8(&modes.stdout)
        .map_err(|_| "invalid installed RPM payload mode list")?;
    let (payload_file_paths, payload_directories) = parse_payload_paths(mode_text, &payload_paths)?;
    let configs = bounded_command(
        "/usr/bin/rpm",
        &[std::ffi::OsStr::new("-qc"), std::ffi::OsStr::new(name)],
        20,
        MAX_QUERY_BYTES,
    )?;
    if !configs.status.success() {
        return Err(format!("could not inventory installed configuration paths: {name}"));
    }
    let config_text =
        std::str::from_utf8(&configs.stdout).map_err(|_| "invalid installed config file list")?;
    let config_paths: Vec<_> = config_text.lines().map(str::to_owned).collect();
    if config_paths.iter().any(|path| !payload_paths.contains(path)) {
        return Err(format!("installed package configuration inventory differs: {name}"));
    }
    for option in ["--scripts", "--triggers", "--filetriggers"] {
        let policy = bounded_command(
            "/usr/bin/rpm",
            &[std::ffi::OsStr::new("-q"), std::ffi::OsStr::new(option), std::ffi::OsStr::new(name)],
            20,
            16 * 1024,
        )?;
        if !policy.status.success() || !policy.stdout.is_empty() {
            return Err(format!("installed RPM contains executable package policy: {name}"));
        }
    }
    Ok(Package {
        name: name.to_owned(),
        nevra: expected_nevra.to_owned(),
        source_sha256: None,
        payload_file_paths,
        payload_directories,
        payload_paths,
        config_paths,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemovalPayloadDisposition {
    RetainedSharedOwner,
    RemovedWithPackage,
    RemovedIfEmptyDirectory,
}

impl RemovalPayloadDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::RetainedSharedOwner => "retained-shared-owner",
            Self::RemovedWithPackage => "removed-with-package",
            Self::RemovedIfEmptyDirectory => "removed-if-empty-directory",
        }
    }
}

fn removal_payload_ownership(
    packages: &[Package],
    installed_owners: &BTreeMap<String, BTreeSet<String>>,
) -> Result<BTreeMap<String, RemovalPayloadDisposition>, String> {
    let selected: BTreeSet<_> = packages.iter().map(|package| package.name.as_str()).collect();
    let mut dispositions = BTreeMap::new();
    for package in packages {
        for path in &package.payload_paths {
            let owners = installed_owners
                .get(path)
                .ok_or_else(|| format!("installed payload ownership is missing: {path}"))?;
            if !owners.contains(&package.name) {
                return Err(format!(
                    "installed payload owner inventory differs for {}: {path}",
                    package.name
                ));
            }
            let external_owner = owners.iter().any(|owner| !selected.contains(owner.as_str()));
            let directory = !package.payload_file_paths.contains(path);
            let disposition = if external_owner {
                RemovalPayloadDisposition::RetainedSharedOwner
            } else if directory {
                RemovalPayloadDisposition::RemovedIfEmptyDirectory
            } else {
                RemovalPayloadDisposition::RemovedWithPackage
            };
            dispositions
                .entry(path.clone())
                .and_modify(|existing| {
                    if disposition == RemovalPayloadDisposition::RetainedSharedOwner
                        || (*existing == RemovalPayloadDisposition::RemovedIfEmptyDirectory
                            && disposition == RemovalPayloadDisposition::RemovedWithPackage)
                    {
                        *existing = disposition;
                    }
                })
                .or_insert(disposition);
        }
    }
    Ok(dispositions)
}

fn validate_payload_file_conflicts(packages: &[Package]) -> Result<String, String> {
    let owners = installed_file_owners()?;
    let root = File::open("/").map_err(|_| "filesystem root is unavailable")?;
    let mut existing = BTreeSet::new();
    for package in packages {
        for path in &package.payload_file_paths {
            if payload_path_exists(&root, path)? {
                existing.insert(path.clone());
            }
        }
    }
    validate_payload_file_owners(packages, &owners, &existing)?;
    let mut hash = Sha256::new();
    for package in packages {
        for path in &package.payload_file_paths {
            hash.update((path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            hash.update([u8::from(existing.contains(path))]);
            if let Some(path_owners) = owners.get(path) {
                hash.update((path_owners.len() as u64).to_le_bytes());
                for owner in path_owners {
                    hash.update((owner.len() as u64).to_le_bytes());
                    hash.update(owner.as_bytes());
                }
            } else {
                hash.update(0_u64.to_le_bytes());
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Snapshot direct directory destinations with descriptor-relative no-symlink
/// traversal. Shared directories are common RPM payload, so this preflight
/// permits co-ownership but refuses a file, symlink, or special object where a
/// directory is expected. Existing directory mode and root ownership must
/// match the RPM payload exactly, so installation cannot silently adjust them.
fn validate_payload_directory_types(packages: &[Package]) -> Result<String, String> {
    let owners = installed_file_owners()?;
    let root = File::open("/").map_err(|_| "filesystem root is unavailable")?;
    validate_payload_directory_types_with(&root, packages, &owners)
}

fn validate_payload_directory_types_with(
    root: &File,
    packages: &[Package],
    owners: &BTreeMap<String, BTreeSet<String>>,
) -> Result<String, String> {
    let mut hash = Sha256::new();
    hash.update(b"ryoku-install-directory-snapshot-v2");
    let mut selected = BTreeMap::<&str, &DirectoryPayload>::new();
    for package in packages {
        for directory in &package.payload_directories {
            let path = directory.path.as_str();
            if let Some(existing) = selected.insert(path, directory)
                && existing != directory
            {
                return Err(format!(
                    "selected RPMs request conflicting directory metadata: {path}"
                ));
            }
            let metadata = payload_path_metadata(root, path)?;
            hash.update((path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            hash.update(directory.mode.to_le_bytes());
            hash.update(directory.uid.to_le_bytes());
            hash.update(directory.gid.to_le_bytes());
            match metadata {
                None => hash.update([0]),
                Some(metadata) => {
                    if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
                        return Err(format!(
                            "refusing an existing non-directory RPM directory path: {path}"
                        ));
                    }
                    if metadata.st_mode & 0o7777 != directory.mode
                        || metadata.st_uid != directory.uid
                        || metadata.st_gid != directory.gid
                    {
                        return Err(format!(
                            "existing RPM directory metadata conflicts with selected package: {path}"
                        ));
                    }
                    hash.update([1]);
                    hash.update(metadata.st_mode.to_le_bytes());
                    hash.update(metadata.st_uid.to_le_bytes());
                    hash.update(metadata.st_gid.to_le_bytes());
                }
            }
            if let Some(path_owners) = owners.get(path) {
                hash.update((path_owners.len() as u64).to_le_bytes());
                for owner in path_owners {
                    hash.update((owner.len() as u64).to_le_bytes());
                    hash.update(owner.as_bytes());
                }
            } else {
                hash.update(0_u64.to_le_bytes());
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn validate_payload_file_owners(
    packages: &[Package],
    installed_owners: &BTreeMap<String, BTreeSet<String>>,
    existing_paths: &BTreeSet<String>,
) -> Result<(), String> {
    let mut selected_paths = BTreeSet::new();
    for package in packages {
        for path in &package.payload_file_paths {
            if !selected_paths.insert(path.as_str()) {
                return Err(format!("multiple selected RPMs contain the same file path: {path}"));
            }
            if existing_paths.contains(path)
                && !installed_owners
                    .get(path)
                    .is_some_and(|owners| owners.len() == 1 && owners.contains(&package.name))
            {
                return Err(format!(
                    "refusing to replace an existing file not owned solely by {}: {path}",
                    package.name
                ));
            }
        }
    }
    Ok(())
}

fn parse_payload_paths(
    mode_text: &str,
    payload_paths: &[String],
) -> Result<(Vec<String>, Vec<DirectoryPayload>), String> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for line in mode_text.lines() {
        let mut fields = line.split('\t');
        let mode = fields.next().ok_or("RPM mode list is malformed")?;
        let user = fields.next().ok_or("RPM mode list is malformed")?;
        let group = fields.next().ok_or("RPM mode list is malformed")?;
        let path = fields.next().ok_or("RPM mode list is malformed")?;
        if fields.next().is_some() || user.is_empty() || group.is_empty() {
            return Err("RPM mode list is malformed".into());
        }
        let mode = mode.parse::<u32>().map_err(|_| "RPM file mode is invalid")?;
        if !payload_paths.iter().any(|known| known == path) || !seen.insert(path) {
            return Err("RPM mode list differs from its payload list".into());
        }
        match mode & 0o170000 {
            0o040000 => {
                if user != "root" || group != "root" {
                    return Err(format!("RPM directory payload must be root-owned: {path}"));
                }
                directories.push(DirectoryPayload {
                    path: path.to_owned(),
                    mode: mode & 0o7777,
                    uid: 0,
                    gid: 0,
                });
            }
            0o100000 | 0o120000 => files.push(path.to_owned()),
            _ => return Err("RPM contains an unsupported special file".into()),
        }
    }
    if seen.len() != payload_paths.len() {
        return Err("RPM mode list is incomplete".into());
    }
    Ok((files, directories))
}

fn installed_file_owners() -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    const MAX_INSTALLED_FILE_QUERY_BYTES: usize = 32 * 1024 * 1024;
    let output = bounded_command(
        "/usr/bin/rpm",
        &[
            std::ffi::OsStr::new("-qa"),
            std::ffi::OsStr::new("--qf"),
            std::ffi::OsStr::new("[P%{NAME:json}\\n[F%{FILENAMES:json}\\n]]"),
        ],
        30,
        MAX_INSTALLED_FILE_QUERY_BYTES,
    )?;
    if !output.status.success() {
        return Err("installed RPM file ownership query failed".into());
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| "installed RPM file list is invalid")?;
    parse_installed_file_owners(text)
}

fn parse_installed_file_owners(text: &str) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
    let mut current = None::<String>;
    for line in text.lines() {
        if let Some(encoded_name) = line.strip_prefix('P') {
            let owner: String = serde_json::from_str(encoded_name)
                .map_err(|_| "installed RPM name is malformed")?;
            if owner.is_empty()
                || !owner
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+_.-".contains(&byte))
            {
                return Err("installed RPM name is malformed".into());
            }
            current = Some(owner);
        } else if let Some(encoded_path) = line.strip_prefix('F') {
            let owner = current.as_ref().ok_or("installed RPM file list is malformed")?;
            let path: String = serde_json::from_str(encoded_path)
                .map_err(|_| "installed RPM file path is malformed")?;
            if !is_canonical_absolute_path(&path) {
                return Err("installed RPM file path is not canonical".into());
            }
            owners.entry(path).or_default().insert(owner.clone());
        } else {
            return Err("installed RPM file ownership record is malformed".into());
        }
    }
    Ok(owners)
}

fn is_canonical_absolute_path(path: &str) -> bool {
    if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
        return false;
    }
    if path == "/" {
        return true;
    }
    path[1..]
        .split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn payload_path_metadata(root: &File, path: &str) -> Result<Option<rustix::fs::Stat>, String> {
    if !is_canonical_managed_payload_path(path) {
        return Err("RPM payload path is not canonical or is outside /usr".into());
    }
    let relative = path.strip_prefix('/').ok_or("RPM payload path is not absolute")?;
    let (parent_path, leaf) = relative.rsplit_once('/').unwrap_or(("", relative));
    let parent = if parent_path.is_empty() {
        File::from(rustix::io::dup(root).map_err(|_| "filesystem root cannot be duplicated")?)
    } else {
        match openat2(
            root,
            parent_path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        ) {
            Ok(parent) => File::from(parent),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err("payload parent cannot be opened safely".into()),
        }
    };
    match statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(_) => Err("existing payload path cannot be inspected safely".into()),
    }
}

fn is_canonical_managed_payload_path(path: &str) -> bool {
    if (path != "/usr" && !path.starts_with("/usr/")) || path.chars().any(char::is_control) {
        return false;
    }
    let mut components = path.split('/');
    if components.next() != Some("") {
        return false;
    }
    let mut found_component = false;
    for component in components {
        if component.is_empty() || component == "." || component == ".." {
            return false;
        }
        found_component = true;
    }
    found_component
}

fn payload_path_exists(root: &File, path: &str) -> Result<bool, String> {
    Ok(payload_path_metadata(root, path)?.is_some())
}

fn validate_host() -> Result<(), String> {
    let os_release = fs::read_to_string("/etc/os-release")
        .map_err(|_| "cannot identify the installed operating system")?;
    validate_platform(&os_release, std::env::consts::ARCH)?;
    let root = File::open("/").map_err(|_| "root filesystem is unavailable")?;
    let layout = ryoku_recovery::snapper_layout::discover_layout(
        &root,
        &ryoku_recovery::RelativePath::new("etc").map_err(|_| "invalid fixed path")?,
    )
    .map_err(|_| "root/boot snapshot layout is unavailable")?;
    if layout.assessment.capability != LayoutCapability::FullSystem
        || layout.targets.iter().any(|target| target.config.as_str() != "root")
    {
        return Err(format!(
            "unsupported root/boot snapshot layout: {:?}",
            layout.assessment.reason
        ));
    }

    let root = File::open("/").map_err(|_| "root filesystem is unavailable")?;
    let machine = read_machine_state_record(&root)
        .map_err(|_| "protected Ryoku state has not been initialized")?;
    collect_and_open_state_mount(&root, machine.identity())
        .map_err(|_| "protected Ryoku state mount is unavailable")?;
    GenerationReader::open_state_mount()
        .map_err(|_| "protected generation store is unavailable")?;
    let journals = RestoreJournalStore::open_existing_state_mount()
        .map_err(|_| "protected recovery journal is unavailable")?;
    if !journals.incomplete_attempts().map_err(|_| "recovery state cannot be verified")?.is_empty()
    {
        return Err("unfinished recovery must be resolved before installation".into());
    }
    Ok(())
}

fn validate_platform(os_release: &str, architecture: &str) -> Result<(), String> {
    let mut id = None;
    for line in os_release.lines() {
        let Some(value) = line.strip_prefix("ID=") else { continue };
        if id.is_some() {
            return Err("operating system ID is ambiguous".into());
        }
        id = Some(parse_os_release_value(value)?);
    }
    let id = id.ok_or("operating system ID is missing")?;
    if id != "opensuse-tumbleweed" || architecture != "x86_64" {
        return Err("only openSUSE Tumbleweed x86_64 is supported".into());
    }
    Ok(())
}

fn parse_os_release_value(value: &str) -> Result<&str, String> {
    if value.is_empty() {
        return Err("operating system ID is empty".into());
    }
    let first = value.as_bytes()[0];
    if first == b'"' || first == b'\'' {
        if value.as_bytes().last() != Some(&first) || value.len() < 2 {
            return Err("operating system ID is malformed".into());
        }
        let inner = &value[1..value.len() - 1];
        if inner.is_empty() {
            return Err("operating system ID is empty".into());
        }
        return Ok(inner);
    }
    if value.as_bytes().last().is_some_and(|last| *last == b'"' || *last == b'\'') {
        return Err("operating system ID is malformed".into());
    }
    Ok(value)
}

fn run_zypper_preview(packages: &[PackageInput]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("TERM", "dumb")
        .args(["--signal=TERM", "--kill-after=3s", "120s", "/usr/bin/zypper"])
        .args([
            "--no-refresh",
            "--non-interactive",
            "install",
            "--dry-run",
            "--details",
            "--no-recommends",
            "--no-allow-downgrade",
            "--no-allow-name-change",
            "--no-allow-arch-change",
            "--no-allow-vendor-change",
        ]);
    for package in packages {
        command.arg(&package.path);
    }
    let output = bounded_output(command, MAX_SOLVER_BYTES, 16 * 1024)
        .map_err(|error| format!("zypper preview failed: {error}"))?;
    let mut combined = output.stdout;
    if !output.stderr.is_empty() {
        combined.extend_from_slice(b"\nsolver_stderr:\n");
        combined.extend_from_slice(&output.stderr);
    }
    if !output.status.success() {
        return Err(format!(
            "zypper did not produce an acceptable dry-run solution ({}): {}",
            output.status,
            sanitize(&combined)
        ));
    }
    Ok(sanitize_bytes(&combined))
}

fn run_zypper_remove_preview(packages: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("TERM", "dumb")
        .args(["--signal=TERM", "--kill-after=3s", "120s", "/usr/bin/zypper"])
        .args(["--no-refresh", "--non-interactive", "remove", "--dry-run", "--details"]);
    for name in packages.keys() {
        command.arg(name);
    }
    let output = bounded_output(command, MAX_SOLVER_BYTES, 16 * 1024)
        .map_err(|error| format!("zypper removal preview failed: {error}"))?;
    let mut combined = output.stdout;
    if !output.stderr.is_empty() {
        combined.extend_from_slice(b"\nsolver_stderr:\n");
        combined.extend_from_slice(&output.stderr);
    }
    if !output.status.success() {
        return Err(format!(
            "zypper did not produce an acceptable removal dry run ({}): {}",
            output.status,
            sanitize(&combined)
        ));
    }
    Ok(sanitize_bytes(&combined))
}

fn bounded_command(
    program: &str,
    arguments: &[&std::ffi::OsStr],
    timeout_seconds: u64,
    max_output: usize,
) -> Result<std::process::Output, String> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .args(["--signal=TERM", "--kill-after=1s"])
        .arg(format!("{timeout_seconds}s"))
        .arg(program)
        .args(arguments);
    bounded_output(command, max_output, 16 * 1024)
        .map_err(|_| "native package query failed or exceeded the output limit".into())
}

fn bounded_output(
    mut command: Command,
    stdout_limit: usize,
    stderr_limit: usize,
) -> Result<std::process::Output, String> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|_| "command could not start")?;
    let stdout = child.stdout.take().ok_or("stdout pipe is unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr pipe is unavailable")?;
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout, stdout_limit));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr, stderr_limit));
    let status = child.wait().map_err(|_| "command could not be reaped")?;
    let stdout = stdout_reader.join().map_err(|_| "stdout reader failed")??;
    let stderr = stderr_reader.join().map_err(|_| "stderr reader failed")??;
    Ok(std::process::Output { status, stdout, stderr })
}

fn read_bounded(mut reader: impl std::io::Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer).map_err(|_| "command output could not be read")?;
        if count == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(count) > limit {
            // Closing the pipe makes the producer fail with EPIPE; timeout is
            // retained as a second bound for commands that ignore the pipe.
            return Err("command output exceeded the limit".into());
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

fn sanitize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&sanitize_bytes(bytes)).into_owned()
}

fn sanitize_bytes(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|byte| {
            if matches!(*byte, b'\n' | b'\r' | b'\t') || (0x20..=0x7e).contains(byte) {
                *byte
            } else {
                b'?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        io::Write,
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ryoku-install-preview-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn owner() -> (u32, u32) {
            (rustix::process::geteuid().as_raw(), rustix::process::getegid().as_raw())
        }

        fn home(&self) -> File {
            File::open(&self.0).unwrap()
        }

        fn ryoku_config(&self) -> PathBuf {
            let config = self.0.join(".config");
            fs::create_dir(&config).unwrap();
            fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
            let ryoku = config.join("ryoku");
            fs::create_dir(&ryoku).unwrap();
            fs::set_permissions(&ryoku, fs::Permissions::from_mode(0o700)).unwrap();
            ryoku
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn platform_gate_requires_one_unambiguous_exact_tumbleweed_id_and_architecture() {
        assert!(
            validate_platform("NAME=\"openSUSE Tumbleweed\"\nID=opensuse-tumbleweed\n", "x86_64")
                .is_ok()
        );
        assert!(validate_platform("ID='opensuse-tumbleweed'\n", "x86_64").is_ok());
        for (release, architecture) in [
            ("NAME=openSUSE\n", "x86_64"),
            ("ID=opensuse-leap\n", "x86_64"),
            ("ID=opensuse-tumbleweed\nID=opensuse-leap\n", "x86_64"),
            ("ID=\n", "x86_64"),
            ("ID=\"opensuse-tumbleweed\n", "x86_64"),
            ("ID=opensuse-tumbleweed\n", "aarch64"),
        ] {
            assert!(validate_platform(release, architecture).is_err());
        }
    }

    #[test]
    fn optional_profile_directories_allow_absence_and_open_owned_directory() {
        let fixture = Fixture::new();
        let root = File::open(&fixture.0).unwrap();
        let (uid, gid) = Fixture::owner();
        assert!(open_optional_directory(&root, "missing", uid, gid).unwrap().is_none());

        fs::create_dir(fixture.0.join(".config")).unwrap();
        fs::set_permissions(fixture.0.join(".config"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(open_optional_directory(&root, ".config", uid, gid).unwrap().is_some());
    }

    #[test]
    fn optional_profile_directories_reject_symlinks() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("target")).unwrap();
        symlink("target", fixture.0.join(".config")).unwrap();
        let root = File::open(&fixture.0).unwrap();
        let (uid, gid) = Fixture::owner();
        assert!(open_optional_directory(&root, ".config", uid, gid).is_err());
    }

    #[test]
    fn package_inputs_are_copied_to_a_private_stable_directory() {
        let fixture = Fixture::new();
        let mut arguments = Vec::new();
        let mut originals = Vec::new();
        for index in 0..REQUIRED.len() {
            let path = fixture.0.join(format!("input-{index}.rpm"));
            let contents = format!("synthetic RPM fixture {index}").into_bytes();
            fs::write(&path, &contents).unwrap();
            arguments.push(path.into_os_string());
            originals.push(contents);
        }

        let staged = validate_inputs_in(&arguments, &fixture.0).unwrap();
        let directory = staged._directory.0.clone();
        let metadata = fs::metadata(&directory).unwrap();
        assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(metadata.mode() & 0o7777, 0o700);
        assert_eq!(staged.inputs.len(), REQUIRED.len());
        assert!(staged.inputs.iter().all(|input| input.sha256.len() == 64));
        for argument in &arguments {
            fs::write(PathBuf::from(argument), b"replaced after preflight").unwrap();
        }
        for (input, expected) in staged.inputs.iter().zip(originals) {
            assert_eq!(input.sha256, format!("{:x}", Sha256::digest(&expected)));
            let metadata = fs::metadata(&input.path).unwrap();
            assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
            assert_eq!(metadata.mode() & 0o7777, 0o600);
            assert_eq!(fs::read(&input.path).unwrap(), expected);
        }
        drop(staged);
        assert!(!directory.exists());
    }

    #[test]
    fn payload_path_probe_handles_absence_and_does_not_follow_symlinks() {
        let fixture = Fixture::new();
        let parent = fixture.0.join("usr/bin");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("tool"), b"managed payload").unwrap();
        symlink("tool", parent.join("tool-link")).unwrap();
        symlink("usr", fixture.0.join("usr-link")).unwrap();
        let root = File::open(&fixture.0).unwrap();
        assert!(payload_path_exists(&root, "/usr/bin/tool").unwrap());
        assert!(payload_path_exists(&root, "/usr/bin/tool-link").unwrap());
        assert!(!payload_path_exists(&root, "/usr/bin/missing").unwrap());
        assert!(!payload_path_exists(&root, "/usr/share/missing/tool").unwrap());
        assert!(payload_path_exists(&root, "/usr-link/bin/tool").is_err());
    }

    #[test]
    fn rpm_payload_mode_inventory_separates_directories_and_rejects_unsafe_entries() {
        let paths = vec!["/usr/bin/tool".to_owned(), "/usr/share/tool".to_owned()];
        assert_eq!(
            parse_payload_paths(
                "33261\troot\troot\t/usr/bin/tool\n16877\troot\troot\t/usr/share/tool\n",
                &paths
            )
            .unwrap(),
            (
                vec!["/usr/bin/tool".into()],
                vec![DirectoryPayload {
                    path: "/usr/share/tool".into(),
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                }]
            )
        );
        assert!(parse_payload_paths("33261\troot\troot\t/usr/bin/tool\n", &paths).is_err());
        assert!(
            parse_payload_paths(
                "4096\troot\troot\t/usr/bin/tool\n16877\troot\troot\t/usr/share/tool\n",
                &paths
            )
            .is_err()
        );
        assert!(
            parse_payload_paths(
                "16877\tnobody\troot\t/usr/share/tool\n",
                &["/usr/share/tool".into()]
            )
            .is_err()
        );
    }

    #[test]
    fn managed_payload_paths_must_have_one_canonical_spelling() {
        for path in ["/usr", "/usr/bin/ryoku-session", "/usr/share/ryoku-opensuse/qml/shell.qml"] {
            assert!(is_canonical_managed_payload_path(path), "{path}");
        }
        for path in [
            "usr/bin/ryoku-session",
            "/etc/ryoku.conf",
            "/usr/../etc/ryoku.conf",
            "/usr/./bin/ryoku-session",
            "/usr//bin/ryoku-session",
            "/usr/bin/",
            "/usr/",
            "/usr/bin/ryoku\n-session",
        ] {
            assert!(!is_canonical_managed_payload_path(path), "{path:?}");
        }
    }

    #[test]
    fn directory_payload_preflight_rejects_type_collisions_and_binds_metadata() {
        let fixture = Fixture::new();
        let path = fixture.0.join("usr/share/ryoku");
        fs::create_dir_all(&path).unwrap();
        let root = File::open(&fixture.0).unwrap();
        let current = fs::metadata(&path).unwrap();
        let package = Package {
            name: "ryoku-desktop".into(),
            nevra: "0:0.1.0-2.x86_64".into(),
            source_sha256: None,
            payload_paths: vec!["/usr/share/ryoku".into()],
            payload_file_paths: Vec::new(),
            payload_directories: vec![DirectoryPayload {
                path: "/usr/share/ryoku".into(),
                mode: 0o755,
                uid: current.uid(),
                gid: current.gid(),
            }],
            config_paths: Vec::new(),
        };
        let owners = BTreeMap::from([(
            "/usr/share/ryoku".into(),
            BTreeSet::from(["ryoku-desktop".into(), "other-package".into()]),
        )]);
        let first =
            validate_payload_directory_types_with(&root, std::slice::from_ref(&package), &owners)
                .unwrap();
        let changed_owner =
            BTreeMap::from([("/usr/share/ryoku".into(), BTreeSet::from(["ryoku-desktop".into()]))]);
        assert_ne!(
            first,
            validate_payload_directory_types_with(
                &root,
                std::slice::from_ref(&package),
                &changed_owner,
            )
            .unwrap()
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            validate_payload_directory_types_with(&root, std::slice::from_ref(&package), &owners)
                .is_err()
        );

        fs::remove_dir(&path).unwrap();
        fs::write(&path, b"foreign file").unwrap();
        assert!(
            validate_payload_directory_types_with(&root, std::slice::from_ref(&package), &owners)
                .is_err()
        );

        fs::remove_file(&path).unwrap();
        symlink("/tmp", &path).unwrap();
        assert!(validate_payload_directory_types_with(&root, &[package], &owners).is_err());
    }

    #[test]
    fn install_preview_refuses_unowned_or_foreign_existing_payload_files() {
        let package = Package {
            name: "ryoku-desktop".into(),
            nevra: "0:0.1.0-2.x86_64".into(),
            source_sha256: Some("a".repeat(64)),
            payload_paths: vec!["/usr/bin/ryoku-session".into()],
            payload_file_paths: vec!["/usr/bin/ryoku-session".into()],
            payload_directories: Vec::new(),
            config_paths: Vec::new(),
        };
        let packages = vec![package.clone()];
        let path = "/usr/bin/ryoku-session".to_owned();
        let existing = BTreeSet::from([path.clone()]);
        assert!(validate_payload_file_owners(&packages, &BTreeMap::new(), &existing).is_err());
        assert!(
            validate_payload_file_owners(
                &packages,
                &BTreeMap::from([(path.clone(), BTreeSet::from(["bash".to_owned()]))]),
                &existing,
            )
            .is_err()
        );
        assert!(
            validate_payload_file_owners(
                &packages,
                &BTreeMap::from([(path.clone(), BTreeSet::from(["ryoku-desktop".to_owned()]))]),
                &existing,
            )
            .is_ok()
        );
        assert!(
            validate_payload_file_owners(&packages, &BTreeMap::new(), &BTreeSet::new()).is_ok()
        );
        assert!(
            validate_payload_file_owners(
                &[package.clone(), package],
                &BTreeMap::new(),
                &BTreeSet::new()
            )
            .is_err()
        );
    }

    #[test]
    fn installed_rpm_file_owner_inventory_parses_multiple_packages() {
        let owners = parse_installed_file_owners(
            "P\"filesystem\"\nF\"/usr/bin\"\nP\"first\"\nF\"/usr/bin/tool\"\nP\"second\"\nF\"/usr/bin/other\"\n",
        )
        .unwrap();
        assert_eq!(owners["/usr/bin/tool"], BTreeSet::from(["first".to_owned()]));
        assert!(parse_installed_file_owners("F\"/usr/bin/orphan\"\n").is_err());
    }

    #[test]
    fn installed_file_owner_inventory_cannot_be_forged_by_filename_control_bytes() {
        let records = "P\"untrusted\"\nF\"/usr/forged\\nPryoku-desktop\\nF/escaped\"\n";
        assert!(parse_installed_file_owners(records).is_err());
        for path in [
            "/usr/../bin/tool",
            "/usr//bin/tool",
            "/usr/./bin/tool",
            "/usr/tool\nP\"ryoku-desktop\"",
        ] {
            let records = format!("P\"untrusted\"\nF{}\n", serde_json::to_string(path).unwrap());
            assert!(parse_installed_file_owners(&records).is_err(), "{path:?}");
        }
    }

    #[test]
    fn removal_preview_distinguishes_shared_payloads_and_rejects_owner_drift() {
        let package = Package {
            name: "ryoku-desktop".into(),
            nevra: "0:0.1.0-2.x86_64".into(),
            source_sha256: None,
            payload_paths: vec![
                "/usr/bin".into(),
                "/usr/share/ryoku".into(),
                "/usr/bin/ryoku-session".into(),
                "/usr/bin/ryoku-app".into(),
            ],
            payload_file_paths: vec!["/usr/bin/ryoku-session".into(), "/usr/bin/ryoku-app".into()],
            payload_directories: vec![
                DirectoryPayload { path: "/usr/bin".into(), mode: 0o755, uid: 0, gid: 0 },
                DirectoryPayload { path: "/usr/share/ryoku".into(), mode: 0o755, uid: 0, gid: 0 },
            ],
            config_paths: Vec::new(),
        };
        let owners = BTreeMap::from([
            ("/usr/bin".into(), BTreeSet::from(["ryoku-desktop".into(), "filesystem".into()])),
            ("/usr/share/ryoku".into(), BTreeSet::from(["ryoku-desktop".into()])),
            (
                "/usr/bin/ryoku-session".into(),
                BTreeSet::from(["ryoku-desktop".into(), "third-party-tools".into()]),
            ),
            ("/usr/bin/ryoku-app".into(), BTreeSet::from(["ryoku-desktop".into()])),
        ]);
        assert_eq!(
            removal_payload_ownership(std::slice::from_ref(&package), &owners).unwrap(),
            BTreeMap::from([
                ("/usr/bin".into(), RemovalPayloadDisposition::RetainedSharedOwner,),
                ("/usr/share/ryoku".into(), RemovalPayloadDisposition::RemovedIfEmptyDirectory,),
                ("/usr/bin/ryoku-session".into(), RemovalPayloadDisposition::RetainedSharedOwner,),
                ("/usr/bin/ryoku-app".into(), RemovalPayloadDisposition::RemovedWithPackage,),
            ])
        );
        assert!(
            removal_payload_ownership(std::slice::from_ref(&package), &BTreeMap::new()).is_err()
        );
        let foreign = BTreeMap::from([(
            "/usr/bin/ryoku-session".into(),
            BTreeSet::from(["third-party-tools".into()]),
        )]);
        assert!(removal_payload_ownership(&[package], &foreign).is_err());
    }

    #[test]
    fn install_plan_digest_binds_installed_state_sources_payloads_ownership_and_solver() {
        let package = |source_sha256: &str| Package {
            name: "ryoku-desktop".into(),
            nevra: "0:0.1.0-2.x86_64".into(),
            source_sha256: Some(source_sha256.into()),
            payload_paths: vec!["/usr/bin/ryoku-session".into()],
            payload_file_paths: vec!["/usr/bin/ryoku-session".into()],
            payload_directories: Vec::new(),
            config_paths: vec!["/usr/share/ryoku/config/profile.json".into()],
        };
        let installed = BTreeMap::from([("ryoku-desktop".into(), "0:0.1.0-1.x86_64".into())]);
        let selected = vec![package(&"a".repeat(64))];
        let plan =
            install_plan_sha256(&installed, &selected, b"solver transaction\n", &"c".repeat(64));
        assert_eq!(
            plan,
            install_plan_sha256(&installed, &selected, b"solver transaction\n", &"c".repeat(64))
        );
        assert_ne!(
            plan,
            install_plan_sha256(&installed, &selected, b"different solver\n", &"c".repeat(64))
        );
        assert_ne!(
            plan,
            install_plan_sha256(&installed, &selected, b"solver transaction\n", &"d".repeat(64))
        );
        assert_ne!(
            plan,
            install_plan_sha256(
                &BTreeMap::new(),
                &selected,
                b"solver transaction\n",
                &"c".repeat(64)
            )
        );
        assert_ne!(
            plan,
            install_plan_sha256(
                &installed,
                &[package(&"b".repeat(64))],
                b"solver transaction\n",
                &"c".repeat(64)
            )
        );
        let mut changed_target = selected.clone();
        changed_target[0].nevra = "0:0.1.0-3.x86_64".into();
        assert_ne!(
            plan,
            install_plan_sha256(
                &installed,
                &changed_target,
                b"solver transaction\n",
                &"c".repeat(64)
            )
        );
        let mut changed = selected.clone();
        changed[0].payload_paths.push("/usr/bin/unreviewed".into());
        assert_ne!(
            plan,
            install_plan_sha256(&installed, &changed, b"solver transaction\n", &"c".repeat(64))
        );
        let mut changed_config = selected;
        changed_config[0].config_paths.push("/usr/share/ryoku/config/other.json".into());
        assert_ne!(
            plan,
            install_plan_sha256(
                &installed,
                &changed_config,
                b"solver transaction\n",
                &"c".repeat(64)
            )
        );
    }

    #[test]
    fn package_input_symlinks_are_rejected_and_staging_is_cleaned() {
        let fixture = Fixture::new();
        let target = fixture.0.join("real.rpm");
        fs::write(&target, b"synthetic rpm").unwrap();
        let link = fixture.0.join("linked.rpm");
        symlink(&target, &link).unwrap();
        let mut arguments = vec![link.into_os_string()];
        for index in 1..REQUIRED.len() {
            let path = fixture.0.join(format!("input-{index}.rpm"));
            fs::write(&path, b"synthetic rpm").unwrap();
            arguments.push(path.into_os_string());
        }
        let before: BTreeSet<_> = fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .collect();
        assert!(validate_inputs_in(&arguments, &fixture.0).is_err());
        let after: BTreeSet<_> = fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(before, after, "failed staging left a private directory behind");
    }

    #[test]
    fn migration_inspection_recognizes_exact_legacy_document_without_mutating_source() {
        use ryoku_profiles::{GENERIC_PROFILE_ID, ProfileDocument, ProfileId, SettingsLayer};

        let fixture = Fixture::new();
        let config = fixture.ryoku_config();
        let source = ProfileDocument::new(
            ProfileId::new(GENERIC_PROFILE_ID).unwrap(),
            SettingsLayer::default(),
        )
        .canonical_bytes()
        .unwrap();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(config.join("profile.json"))
            .unwrap();
        file.write_all(&source).unwrap();
        file.sync_all().unwrap();
        fs::set_permissions(config.join("profile.json"), fs::Permissions::from_mode(0o644))
            .unwrap();
        let (uid, gid) = Fixture::owner();

        let assessment = inspect_legacy_profile(&fixture.home(), uid, gid).unwrap();
        assert!(matches!(assessment, MigrationAssessment::Recognized { source_mode: 0o644, .. }));
        assert_eq!(fs::read(config.join("profile.json")).unwrap(), source);
        assert!(!config.join("opensuse-profile.json").exists());
    }

    #[test]
    fn migration_inspection_treats_fresh_and_upstream_accounts_as_non_candidates() {
        let fixture = Fixture::new();
        let (uid, gid) = Fixture::owner();
        assert_eq!(
            inspect_legacy_profile(&fixture.home(), uid, gid).unwrap(),
            MigrationAssessment::NoCandidate
        );

        let config = fixture.ryoku_config();
        fs::write(config.join("profile.json"), br#"{"hero":{"kind":"builtin","source":"ryoku"}}"#)
            .unwrap();
        fs::set_permissions(config.join("profile.json"), fs::Permissions::from_mode(0o600))
            .unwrap();
        assert_eq!(
            inspect_legacy_profile(&fixture.home(), uid, gid).unwrap(),
            MigrationAssessment::Unrecognized
        );
    }

    #[test]
    fn migration_inspection_refuses_symlinks_and_existing_destination() {
        use ryoku_profiles::{GENERIC_PROFILE_ID, ProfileDocument, ProfileId, SettingsLayer};

        let (uid, gid) = Fixture::owner();
        let linked = Fixture::new();
        let config = linked.ryoku_config();
        fs::write(config.join("outside"), b"outside").unwrap();
        symlink("outside", config.join("profile.json")).unwrap();
        assert!(inspect_legacy_profile(&linked.home(), uid, gid).is_err());

        let occupied = Fixture::new();
        let config = occupied.ryoku_config();
        let bytes = ProfileDocument::new(
            ProfileId::new(GENERIC_PROFILE_ID).unwrap(),
            SettingsLayer::default(),
        )
        .canonical_bytes()
        .unwrap();
        fs::write(config.join("profile.json"), bytes).unwrap();
        fs::write(config.join("opensuse-profile.json"), b"existing").unwrap();
        assert!(inspect_legacy_profile(&occupied.home(), uid, gid).is_err());
    }
}
