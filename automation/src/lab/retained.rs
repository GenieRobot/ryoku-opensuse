//! Resume prerequisites for an already stopped development guest. Never creates
//! disks, keys, firmware, seed media or guest installation state.
use crate::{
    accelerator::Accelerator,
    definition::scalar,
    preflight::{self, Checks},
    runner,
    support::{Result, require},
};
use rustix::fs::{FlockOperation, Mode, OFlags};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(crate) struct Inputs {
    pub assets: PathBuf,
    pub run: PathBuf,
    // A directory flock serializes resumed supervisors without leaving a token
    // that could itself become stale after a crash.
    pub directory: fs::File,
}

fn file(directory: &fs::File, name: &str, private: bool, limit: u64) -> Result<fs::File> {
    let file = fs::File::from(rustix::fs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    require(
        metadata.is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.nlink() == 1
            && metadata.len() > 0
            && metadata.len() <= limit
            && if private {
                metadata.mode() & 0o7777 == 0o600
            } else {
                matches!(metadata.mode() & 0o7777, 0o600 | 0o644)
            },
        &format!(
            "unsafe retained file {name}: require owned, single-link regular input with safe mode"
        ),
    )?;
    Ok(file)
}

fn absent(directory: &fs::File, name: &str) -> Result<()> {
    match rustix::fs::statat(directory, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(format!("retained {name} exists; live or stale metadata requires inspection, never PID adoption or automatic deletion").into()),
    }
}

fn stopped(run: &Path) -> Result<fs::File> {
    require(
        run.is_absolute()
            && fs::canonicalize(run)? == run
            && run
                .as_os_str()
                .as_encoded_bytes()
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(byte)),
        "retained run must be a canonical absolute path without symlinks",
    )?;
    for ancestor in run.ancestors().skip(1) {
        let metadata = fs::metadata(ancestor)?;
        require(
            metadata.is_dir()
                && (metadata.uid() == 0 || metadata.uid() == rustix::process::geteuid().as_raw())
                && (metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0),
            "retained run has an untrusted writable ancestor",
        )?;
    }
    let directory = crate::development::directory(run)?;
    rustix::fs::flock(&directory, FlockOperation::NonBlockingLockExclusive)?;
    for name in
        [crate::development::RECORD, "qmp-private.sock", "development-release", "migration.qcow2"]
    {
        absent(&directory, name)?;
    }
    let mut disk = file(&directory, "disk.qcow2", false, 64 * 1024 * 1024 * 1024)?;
    let mut header = [0; 32];
    disk.read_exact(&mut header)?;
    require(
        &header[..4] == b"QFI\xfb"
            && matches!(u32::from_be_bytes(header[4..8].try_into()?), 2 | 3)
            && header[8..20] == [0; 12]
            && u64::from_be_bytes(header[24..32].try_into()?)
                == scalar("disk_virtual_gib")?.parse::<u64>()? * 1024 * 1024 * 1024,
        "resume requires the retained standalone qcow2 disk with the fixture virtual size and no backing image",
    )?;
    file(&directory, "OVMF_VARS.fd", false, 8 * 1024 * 1024)?;
    file(&directory, "seed.iso", false, 16 * 1024 * 1024)?;
    file(&directory, "id_ed25519", true, 16 * 1024)?;
    let mut hosts = String::new();
    file(&directory, "known_hosts", true, 64 * 1024)?.read_to_string(&mut hosts)?;
    host_key(&hosts)?;
    let mut evidence = String::new();
    file(&directory, "evidence.txt", false, 1024 * 1024)?.read_to_string(&mut evidence)?;
    require(
        evidence.lines().last() == Some("development_desktop=retained_stopped"),
        "retained run has no final stopped development receipt",
    )?;
    no_live_qemu(run, &disk.metadata()?)?;
    Ok(directory)
}

fn no_live_qemu(run: &Path, disk: &fs::Metadata) -> Result<()> {
    let needle = run.to_str().ok_or("non-UTF8 retained path")?.as_bytes();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        if !entry.file_name().as_encoded_bytes().iter().all(u8::is_ascii_digit) {
            continue;
        }
        let path = entry.path();
        let command = match fs::read(path.join("cmdline")) {
            Ok(command) => command,
            // procfs can report ESRCH as well as ENOENT when a process exits.
            Err(error) if vanished(&error) => continue,
            Err(error) => return Err(error.into()),
        };
        let Some(program) = command.split(|b| *b == 0).next() else { continue };
        if !program.ends_with(b"qemu-system-x86_64") {
            continue;
        }
        require(
            !command.windows(needle.len()).any(|window| window == needle),
            "a live QEMU already references the retained run",
        )?;
        // Detect another spelling (hardlink or /proc/fd path) of this disk.
        let descriptors = match fs::read_dir(path.join("fd")) {
            Ok(descriptors) => descriptors,
            Err(error) if vanished(&error) => continue,
            Err(error) => return Err(error.into()),
        };
        for descriptor in descriptors {
            match fs::metadata(descriptor?.path()) {
                Ok(metadata) => require(
                    metadata.dev() != disk.dev() || metadata.ino() != disk.ino(),
                    "a live QEMU already owns the retained disk",
                )?,
                Err(error) if vanished(&error) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn vanished(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
        || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error())
}

// Historical ports may repeat the same key. Refuse ambiguous identities and
// non-loopback entries; a new allocated port must inherit exactly this key.
fn host_key(hosts: &str) -> Result<String> {
    let mut key = None;
    for line in hosts.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        require(
            words.len() == 3
                && words[0].starts_with("[127.0.0.1]:")
                && words[0][12..].parse::<u16>().is_ok_and(|port| port > 0)
                && words[1] == "ssh-ed25519"
                && words[2].len() == 68
                && words[2].bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
            "retained known_hosts must contain only one pinned loopback ed25519 identity",
        )?;
        let value = format!("{} {}", words[1], words[2]);
        require(key.as_ref().is_none_or(|key| key == &value), "ambiguous retained SSH host keys")?;
        key = Some(value);
    }
    key.ok_or_else(|| "retained SSH host key missing".into())
}

pub(crate) fn pin_port(directory: &fs::File, port: u16) -> Result<()> {
    require(port > 0, "invalid resume SSH port")?;
    let mut hosts = String::new();
    file(directory, "known_hosts", true, 64 * 1024)?.read_to_string(&mut hosts)?;
    let line = format!("[127.0.0.1]:{port} {}", host_key(&hosts)?);
    if !hosts.lines().any(|existing| existing == line) {
        let mut output = fs::File::from(rustix::fs::openat(
            directory,
            "known_hosts",
            OFlags::WRONLY | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        if !hosts.ends_with('\n') {
            output.write_all(b"\n")?;
        }
        writeln!(output, "{line}")?;
        output.sync_all()?;
    }
    Ok(())
}

pub(crate) fn preflight(assets: &str, run: &str, accelerator: Accelerator) -> Result<Inputs> {
    require(runner::WSL, "retained development resume requires wsl-lab")?;
    let mut checks = Checks::default();
    checks.check("selected accelerator", || accelerator.preflight());
    let assets = checks.check("asset directory", || {
        let path = fs::canonicalize(assets)?;
        require(path.is_dir(), "assets must be a directory")?;
        Ok(path)
    });
    let run = PathBuf::from(run);
    let directory = checks.check("stopped retained development guest", || stopped(&run));
    runner::validate_host(assets.as_deref(), run.parent(), &mut checks);
    preflight::resume_tools(assets.as_deref(), &mut checks);
    checks.finish()?;
    Ok(Inputs {
        assets: assets.ok_or("asset directory unavailable")?,
        run,
        directory: directory.ok_or("retained directory unavailable")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::{AtomicUsize, Ordering},
    };

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAID5nM1GTM0PouQobcJjMvVL1utn+b0GCmM0w1QW4+aRg";
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "ryoku-retained-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            let mut header = [0; 32];
            header[..4].copy_from_slice(b"QFI\xfb");
            header[4..8].copy_from_slice(&3u32.to_be_bytes());
            header[24..32].copy_from_slice(
                &(scalar("disk_virtual_gib").unwrap().parse::<u64>().unwrap() * 1024 * 1024 * 1024)
                    .to_be_bytes(),
            );
            fs::write(path.join("disk.qcow2"), header).unwrap();
            for name in ["OVMF_VARS.fd", "seed.iso", "id_ed25519"] {
                fs::write(path.join(name), b"retained").unwrap();
            }
            fs::write(path.join("evidence.txt"), b"development_desktop=retained_stopped\n")
                .unwrap();
            fs::write(path.join("known_hosts"), format!("[127.0.0.1]:52056 {KEY}\n")).unwrap();
            for name in ["id_ed25519", "known_hosts"] {
                fs::set_permissions(path.join(name), fs::Permissions::from_mode(0o600)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stopped_inspection_is_read_only_and_serializes_supervisors() {
        let fixture = Fixture::new();
        let original: Vec<_> = fs::read_dir(&fixture.0)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        let owner = stopped(&fixture.0).unwrap();
        assert!(stopped(&fixture.0).is_err());
        for (path, bytes) in &original {
            assert_eq!(fs::read(path).unwrap(), *bytes);
        }
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), original.len());
        drop(owner);
        stopped(&fixture.0).unwrap();
    }

    #[test]
    fn stale_live_and_release_metadata_are_refused_without_deletion() {
        let fixture = Fixture::new();
        for name in [
            crate::development::RECORD,
            "qmp-private.sock",
            "development-release",
            "migration.qcow2",
        ] {
            let path = fixture.0.join(name);
            fs::write(&path, b"stale").unwrap();
            assert!(stopped(&fixture.0).is_err(), "accepted {name}");
            assert_eq!(fs::read(&path).unwrap(), b"stale");
            fs::remove_file(path).unwrap();
        }
        fs::write(
            fixture.0.join("evidence.txt"),
            b"development_desktop=retained_stopped\ndevelopment_resume=starting\n",
        )
        .unwrap();
        assert!(stopped(&fixture.0).is_err());
        assert!(crate::support::public_evidence("development_resume=starting"));
    }

    #[test]
    fn paths_modes_links_and_backing_disks_fail_closed() {
        let fixture = Fixture::new();
        let directory = crate::development::directory(&fixture.0).unwrap();
        for name in
            ["disk.qcow2", "OVMF_VARS.fd", "seed.iso", "id_ed25519", "known_hosts", "evidence.txt"]
        {
            let path = fixture.0.join(name);
            let mode = fs::metadata(&path).unwrap().permissions();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            assert!(stopped(&fixture.0).is_err(), "accepted writable {name}");
            fs::set_permissions(&path, mode).unwrap();
            let link = fixture.0.join("linked");
            fs::hard_link(&path, &link).unwrap();
            assert!(stopped(&fixture.0).is_err(), "accepted linked {name}");
            fs::remove_file(link).unwrap();
        }
        symlink("id_ed25519", fixture.0.join("alias")).unwrap();
        assert!(file(&directory, "alias", true, 1024).is_err());
        let mut disk = fs::read(fixture.0.join("disk.qcow2")).unwrap();
        disk[15] = 1;
        fs::write(fixture.0.join("disk.qcow2"), disk).unwrap();
        assert!(stopped(&fixture.0).is_err());
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(stopped(&fixture.0).is_err());
    }

    #[test]
    fn fresh_port_inherits_only_the_retained_host_key_and_preserves_history() {
        let fixture = Fixture::new();
        let directory = crate::development::directory(&fixture.0).unwrap();
        let old = fs::read_to_string(fixture.0.join("known_hosts")).unwrap();
        pin_port(&directory, 41617).unwrap();
        let updated = fs::read_to_string(fixture.0.join("known_hosts")).unwrap();
        assert_eq!(updated, format!("{old}[127.0.0.1]:41617 {KEY}\n"));
        pin_port(&directory, 41617).unwrap();
        assert_eq!(fs::read_to_string(fixture.0.join("known_hosts")).unwrap(), updated);
        for bad in [
            "".to_owned(),
            old.replace("127.0.0.1", "0.0.0.0"),
            old.replace("52056", "0"),
            old.replace("52056", "65536"),
            format!("{old}{}", old.replace("aRg", "bRg")),
            old.replace("ssh-ed25519", "@cert-authority"),
        ] {
            assert!(host_key(&bad).is_err(), "accepted {bad}");
        }
    }
}
