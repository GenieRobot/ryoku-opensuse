//! Private, owner-controlled release token for the opt-in WSL desktop hold.
use crate::support::{Result, require};
use rustix::fs::{Mode, OFlags};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub const RECORD: &str = "development-desktop-private.txt";
const RELEASE: &str = "development-release";

pub(crate) enum Input<'a> {
    Keys(Vec<&'a str>),
    Chord(&'static str),
    Click(u32, u32),
}

/// Parse the whole request before ownership checks or any input event.
pub(crate) fn input<'a>(kind: &str, value: &'a str) -> Result<Input<'a>> {
    match kind {
        "--chord" => Ok(Input::Chord(match value {
            "super+n" => "n",
            "super+space" => "spc",
            "super+escape" => "esc",
            _ => return Err("unsupported development chord".into()),
        })),
        "--click" => {
            let (x, y) = value.split_once(',').ok_or("click requires X,Y pixels")?;
            require(
                [x, y].iter().all(|part| {
                    !part.is_empty()
                        && part.len() <= 4
                        && part.bytes().all(|byte| byte.is_ascii_digit())
                }),
                "click requires bounded unsigned X,Y pixels",
            )?;
            let (x, y) = (x.parse::<u32>()?, y.parse::<u32>()?);
            require(x < 8192 && y < 8192, "click exceeds supported framebuffer bounds")?;
            Ok(Input::Click(x, y))
        }
        _ => Ok(Input::Keys(input_keys(kind, value)?)),
    }
}

/// Resolve the complete bounded input before any connection or key event.
pub fn input_keys<'a>(kind: &str, value: &'a str) -> Result<Vec<&'a str>> {
    if kind == "--key" {
        let qcode = match value {
            "enter" => "ret",
            "escape" => "esc",
            "space" => "spc",
            "super" => "meta_l",
            "tab" | "backspace" | "left" | "right" | "up" | "down" | "home" | "end" => value,
            _ => return Err("unsupported development key".into()),
        };
        return Ok(vec![qcode]);
    }
    require(
        kind == "--text"
            && !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b' '),
        "development text requires 1-64 lowercase ASCII letters, digits or spaces",
    )?;
    Ok((0..value.len())
        .map(|index| if value.as_bytes()[index] == b' ' { "spc" } else { &value[index..index + 1] })
        .collect())
}

pub fn directory(run: &Path) -> Result<fs::File> {
    let file = fs::File::from(rustix::fs::open(
        run,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    validate_directory(&file)?;
    Ok(file)
}

fn validate_directory(file: &fs::File) -> Result<()> {
    let metadata = file.metadata()?;
    require(
        metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o7777 == 0o700,
        "development directory must be owned and mode 0700",
    )
}

pub fn record(directory: &fs::File, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut file = fs::File::from(rustix::fs::openat(
        directory,
        RECORD,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(())
}

pub fn release_requested(directory: &fs::File) -> Result<bool> {
    validate_directory(directory)?;
    let descriptor = match rustix::fs::openat(
        directory,
        RELEASE,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file,
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut file = fs::File::from(descriptor);
    let metadata = file.metadata()?;
    require(
        metadata.is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode() & 0o7777 == 0o600
            && metadata.nlink() == 1
            && metadata.len() == 8,
        "invalid development release token; require owned mode-0600 regular file",
    )?;
    let mut token = Vec::new();
    file.by_ref().take(9).read_to_end(&mut token)?;
    require(token == b"release\n", "invalid development release token contents")?;
    Ok(true)
}

pub fn clear_record(directory: &fs::File) -> Result<()> {
    rustix::fs::unlinkat(directory, RECORD, rustix::fs::AtFlags::empty())?;
    directory.sync_all()?;
    Ok(())
}

/// Linux process generation proof, never an instruction to adopt or signal a PID.
pub(crate) fn start_ticks(pid: u32) -> Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, fields) = stat.rsplit_once(") ").ok_or("invalid process stat")?;
    let value = fields.split_whitespace().nth(19).ok_or("missing process start time")?;
    require(value.parse::<u64>()? > 0, "invalid process start time")?;
    Ok(value.to_owned())
}

pub(crate) fn verify_spawned(command: &std::process::Command, pid: u32, run: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let process = std::path::PathBuf::from(format!("/proc/{pid}"));
    let mut expected = Vec::new();
    for arg in std::iter::once(command.get_program()).chain(command.get_args()) {
        expected.extend_from_slice(arg.as_bytes());
        expected.push(0);
    }
    let cwd = fs::metadata(process.join("cwd"))?;
    let owner = fs::metadata(run)?;
    require(
        fs::metadata(&process)?.uid() == rustix::process::geteuid().as_raw(),
        "new QEMU has an unexpected owner",
    )?;
    require(
        fs::read_to_string(process.join("status"))?
            .lines()
            .any(|line| line == format!("PPid:\t{}", std::process::id())),
        "new QEMU is not a direct owned child",
    )?;
    require(
        fs::read(process.join("cmdline"))? == expected,
        "new QEMU argv does not match the launch command",
    )?;
    require(
        fs::canonicalize(process.join("exe"))? == fs::canonicalize(command.get_program())?,
        "new QEMU executable does not match the launch command",
    )?;
    require(
        cwd.dev() == owner.dev() && cwd.ino() == owner.ino(),
        "new QEMU working directory does not match the retained run",
    )
}

pub fn capture_owner(run: &Path) -> Result<(fs::File, u16, u32)> {
    use std::{
        collections::BTreeMap,
        os::{fd::AsRawFd, unix::fs::FileTypeExt},
    };
    require(
        crate::runner::WSL
            && std::env::var("RYOKU_LAB_AUTHORIZED").as_deref() == Ok("existing-wsl-vm")
            && fs::read_to_string("/proc/sys/kernel/osrelease")?
                .to_ascii_lowercase()
                .contains("wsl")
            && !rustix::process::geteuid().is_root(),
        "development capture requires an authorized ordinary-user WSL runner",
    )?;
    let directory = directory(run)?;
    let pinned = std::path::PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut record = fs::File::from(rustix::fs::openat(
        &directory,
        RECORD,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = record.metadata()?;
    let uid = rustix::process::geteuid().as_raw();
    require(
        metadata.is_file()
            && metadata.uid() == uid
            && metadata.nlink() == 1
            && metadata.mode() & 0o7777 == 0o600
            && metadata.len() <= 4096,
        "unsafe development readiness record",
    )?;
    let mut contents = String::new();
    record.by_ref().take(4097).read_to_string(&mut contents)?;
    let mut fields = BTreeMap::new();
    for line in contents.lines() {
        let (key, value) = line.split_once('=').ok_or("invalid development readiness record")?;
        require(fields.insert(key, value).is_none(), "duplicate development readiness field")?;
    }
    for (key, value) in [
        ("readiness", "session_transport_only"),
        ("ssh_host", "127.0.0.1"),
        ("ssh_user", "tester"),
        ("qmp", "qmp-private.sock"),
    ] {
        require(fields.get(key) == Some(&value), "unexpected development readiness identity")?;
    }
    let port = fields.get("ssh_port").ok_or("missing development SSH port")?.parse::<u16>()?;
    require(port > 0, "invalid development SSH port")?;
    let pid = fields.get("qemu_pid").ok_or("missing development QEMU PID")?.parse::<u32>()?;
    let supervisor =
        fields.get("supervisor_pid").ok_or("missing development supervisor PID")?.parse::<u32>()?;
    require(
        fields
            .get("qemu_start_ticks")
            .is_some_and(|ticks| start_ticks(pid).is_ok_and(|actual| actual == *ticks))
            && fields
                .get("supervisor_start_ticks")
                .is_some_and(|ticks| start_ticks(supervisor).is_ok_and(|actual| actual == *ticks)),
        "development process generation no longer matches the owned record",
    )?;
    let process = std::path::PathBuf::from(format!("/proc/{pid}"));
    let status = fs::read_to_string(process.join("status"))?;
    let owner = directory.metadata()?;
    let cwd = fs::metadata(process.join("cwd"))?;
    require(
        pid > 1
            && supervisor > 1
            && fs::metadata(&process)?.uid() == uid
            && fs::metadata(format!("/proc/{supervisor}"))?.uid() == uid
            && status.lines().any(|line| line == format!("PPid:\t{supervisor}"))
            && cwd.dev() == owner.dev()
            && cwd.ino() == owner.ino()
            && fs::read_link(process.join("exe"))?.ends_with("qemu-system-x86_64"),
        "development QEMU is not owned by the recorded live supervisor",
    )?;
    let command = fs::read(format!("/proc/{supervisor}/cmdline"))?;
    require(
        command
            .split(|byte| *byte == 0)
            .any(|arg| arg == b"--development-desktop" || arg == b"development-resume"),
        "recorded supervisor is not a development desktop run",
    )?;
    let socket = fs::symlink_metadata(pinned.join("qmp-private.sock"))?;
    require(
        socket.file_type().is_socket() && socket.uid() == uid,
        "unsafe development QMP socket",
    )?;
    Ok((directory, port, pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn chords_and_clicks_are_fully_parsed_before_any_events() {
        for (name, key) in [("super+n", "n"), ("super+space", "spc"), ("super+escape", "esc")] {
            assert!(
                matches!(input("--chord", name).unwrap(), Input::Chord(actual) if actual == key)
            );
        }
        for bad in [
            "",
            "ctrl+alt+delete",
            "super+N",
            "super+n;quit",
            "super+n\n",
            "super+super",
            "{\"execute\":\"quit\"}",
        ] {
            assert!(input("--chord", bad).is_err());
        }
        assert!(matches!(input("--click", "1279,799").unwrap(), Input::Click(1279, 799)));
        assert!(matches!(input("--click", "0,0").unwrap(), Input::Click(0, 0)));
        for bad in [
            "", "1", "1,", ",1", "-1,0", "0,+1", "1,2,3", "1, 2", "1,2\n", "8192,0", "0,8192",
            "99999,1", "1;quit,2",
        ] {
            assert!(input("--click", bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn spawned_identity_requires_the_owned_child_exact_argv_and_working_directory() {
        let run = std::env::temp_dir();
        let mut command = std::process::Command::new("/usr/bin/sleep");
        command.arg("5").current_dir(&run);
        let mut child = command.spawn().unwrap();
        // procfs can briefly expose an empty argv while exec completes. Wait
        // for this test child's command to become observable before inspecting.
        for _ in 0..100 {
            if fs::read(format!("/proc/{}/cmdline", child.id()))
                .is_ok_and(|bytes| !bytes.is_empty())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let valid = verify_spawned(&command, child.id(), &run);
        let generation = start_ticks(child.id());
        let mut altered = std::process::Command::new("/usr/bin/sleep");
        altered.arg("6");
        let wrong_argv = verify_spawned(&altered, child.id(), &run);
        let wrong_cwd = verify_spawned(&command, child.id(), Path::new("/"));
        let wrong_parent = verify_spawned(&command, std::process::id(), &run);
        let _ = child.kill();
        child.wait().unwrap();
        assert!(valid.is_ok(), "{valid:?}");
        assert!(generation.is_ok());
        assert!(wrong_argv.is_err());
        assert!(wrong_cwd.is_err());
        assert!(wrong_parent.is_err());
        assert!(start_ticks(child.id()).is_err());
    }

    #[test]
    fn input_is_fully_validated_bounded_and_never_interpreted_as_commands() {
        assert_eq!(
            input_keys("--text", "firefox 2").unwrap(),
            ["f", "i", "r", "e", "f", "o", "x", "spc", "2"]
        );
        for (name, qcode) in [
            ("enter", "ret"),
            ("escape", "esc"),
            ("super", "meta_l"),
            ("space", "spc"),
            ("tab", "tab"),
            ("backspace", "backspace"),
            ("left", "left"),
            ("right", "right"),
            ("up", "up"),
            ("down", "down"),
            ("home", "home"),
            ("end", "end"),
        ] {
            assert_eq!(input_keys("--key", name).unwrap(), [qcode]);
        }
        assert_eq!(input_keys("--text", &"a".repeat(64)).unwrap().len(), 64);
        for value in
            ["", "A", "é", "a\n", "a\t", "a;reboot", "{\"execute\":\"quit\"}", &"a".repeat(65)]
        {
            assert!(input_keys("--text", value).is_err());
        }
        for value in ["a", "ctrl-alt-delete", "ret", "power", "quit", "enter\n"] {
            assert!(input_keys("--key", value).is_err());
        }
        assert!(input_keys("--command", "enter").is_err());
    }

    #[test]
    fn release_requires_private_owned_unlinked_regular_token_and_exact_contents() {
        let run =
            std::env::temp_dir().join(format!("ryoku-development-test-{}", std::process::id()));
        fs::create_dir(&run).unwrap();
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
        let directory = directory(&run).unwrap();
        assert!(!release_requested(&directory).unwrap());
        record(&directory, "ssh_host=127.0.0.1\n").unwrap();
        assert_eq!(fs::metadata(run.join(RECORD)).unwrap().mode() & 0o777, 0o600);
        assert!(record(&directory, "overwrite").is_err());
        let token = run.join(RELEASE);
        fs::write(&token, b"release\n").unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(release_requested(&directory).is_err());
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(release_requested(&directory).unwrap());
        fs::hard_link(&token, run.join("hardlink")).unwrap();
        assert!(release_requested(&directory).is_err());
        fs::remove_file(run.join("hardlink")).unwrap();
        fs::write(&token, b"refused\n").unwrap();
        assert!(release_requested(&directory).is_err());
        fs::remove_file(&token).unwrap();
        symlink(RECORD, &token).unwrap();
        assert!(release_requested(&directory).is_err());
        fs::remove_file(&token).unwrap();
        fs::create_dir(&token).unwrap();
        assert!(release_requested(&directory).is_err());
        fs::remove_dir(&token).unwrap();
        rustix::fs::mkfifoat(&directory, RELEASE, Mode::from_raw_mode(0o600)).unwrap();
        assert!(release_requested(&directory).is_err());
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(release_requested(&directory).is_err());
        assert!(super::directory(&run).is_err());
        fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
        clear_record(&directory).unwrap();
        fs::remove_dir_all(run).unwrap();
    }
}
