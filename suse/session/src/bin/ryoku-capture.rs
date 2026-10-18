//! Screenshot adapter for the pinned Ryoku Capture surface.
//!
//! The UI owns delay, selection and preferences. This adapter receives one
//! typed target, invokes grim with argv, copies through the running Ryoku
//! daemon when requested, and only reports success after all requested work.

use std::{
    env,
    error::Error,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode},
};

use rustix::{
    fs::{FileType, Mode, OFlags, ResolveFlags, fstat, mkdirat, openat2},
    io::Errno,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const DAEMON: &str = "/usr/bin/ryoku-shell";
const QUICKSHELL: &str = "/usr/libexec/ryoku/quickshell/bin/qs";
const USAGE: &str = "usage: ryoku-capture --save both|file|clipboard [--beautify] --all|--monitor OUTPUT|--region 'X,Y WIDTHxHEIGHT'";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveMode {
    Both,
    File,
    Clipboard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    All,
    Monitor(String),
    Region(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    save: SaveMode,
    beautify: bool,
    target: Target,
}

fn parse_args(args: &[String]) -> Result<Options> {
    let mut save = None;
    let mut beautify = false;
    let mut target = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--save" if save.is_none() && index + 1 < args.len() => {
                save = Some(match args[index + 1].as_str() {
                    "both" => SaveMode::Both,
                    "file" => SaveMode::File,
                    "clipboard" => SaveMode::Clipboard,
                    _ => return Err(USAGE.into()),
                });
                index += 2;
            }
            "--beautify" if !beautify => {
                beautify = true;
                index += 1;
            }
            "--all" if target.is_none() => {
                target = Some(Target::All);
                index += 1;
            }
            "--monitor" if target.is_none() && index + 1 < args.len() => {
                let output = args[index + 1].clone();
                if output.is_empty()
                    || !output.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                    })
                {
                    return Err("invalid monitor name".into());
                }
                target = Some(Target::Monitor(output));
                index += 2;
            }
            "--region" if target.is_none() && index + 1 < args.len() => {
                let geometry = args[index + 1].clone();
                validate_geometry(&geometry)?;
                target = Some(Target::Region(geometry));
                index += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Options { save: save.ok_or(USAGE)?, beautify, target: target.ok_or(USAGE)? })
}

fn validate_geometry(value: &str) -> Result<()> {
    let (origin, size) = value.split_once(' ').ok_or("invalid screenshot region")?;
    let (x, y) = origin.split_once(',').ok_or("invalid screenshot region")?;
    let (width, height) = size.split_once('x').ok_or("invalid screenshot region")?;
    let x = x.parse::<i32>().map_err(|_| "invalid screenshot region")?;
    let y = y.parse::<i32>().map_err(|_| "invalid screenshot region")?;
    let width = width.parse::<u32>().map_err(|_| "invalid screenshot region")?;
    let height = height.parse::<u32>().map_err(|_| "invalid screenshot region")?;
    if value
        .bytes()
        .any(|byte| !(byte.is_ascii_digit() || matches!(byte, b'-' | b',' | b' ' | b'x')))
        || width == 0
        || height == 0
        || x.unsigned_abs() > 1_000_000
        || y.unsigned_abs() > 1_000_000
        || width > 1_000_000
        || height > 1_000_000
    {
        return Err("invalid screenshot region".into());
    }
    Ok(())
}

fn validate_identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<()> {
    if uid == 0 || uid != euid || gid != egid {
        return Err("screenshot capture requires the matching non-root session identity".into());
    }
    Ok(())
}

fn pictures_directory(uid: u32) -> Result<PathBuf> {
    let base = match env::var_os("XDG_PICTURES_DIR").filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(env::var_os("HOME").ok_or("HOME is unavailable")?).join("Pictures"),
    };
    if !base.is_absolute() {
        return Err("Pictures directory must be absolute".into());
    }
    screenshot_directory(&base, uid)
}

fn screenshot_directory(base_path: &Path, uid: u32) -> Result<PathBuf> {
    let base = open_or_create_owned_directory_tree(base_path, uid)?;
    let screenshots = open_or_create_directory_at(&base, OsStr::new("Screenshots"))?;
    validate_owned_directory_fd(&screenshots, uid, false)?;
    Ok(base_path.join("Screenshots"))
}

fn open_or_create_owned_directory_tree(path: &Path, uid: u32) -> Result<File> {
    if !path.is_absolute() {
        return Err("Pictures directory must be absolute".into());
    }
    let mut directory = File::open("/")?;
    let mut saw_component = false;
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::Normal(name) => {
                saw_component = true;
                directory = open_or_create_directory_at(&directory, name)?;
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err("Pictures directory contains an unsafe path component".into());
            }
        }
    }
    if !saw_component {
        return Err("Pictures directory cannot be the filesystem root".into());
    }
    validate_owned_directory_fd(&directory, uid, false)?;
    Ok(directory)
}

fn open_or_create_directory_at(parent: &File, name: &OsStr) -> Result<File> {
    let open = || {
        openat2(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
    };
    let descriptor = match open() {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => {
            match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            open()?
        }
        Err(error) => return Err(error.into()),
    };
    Ok(File::from(descriptor))
}

fn validate_owned_directory_fd(directory: &File, uid: u32, private: bool) -> Result<()> {
    let metadata = fstat(directory)?;
    let mode = metadata.st_mode & 0o7777;
    if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
        || metadata.st_uid != uid
        || mode & 0o022 != 0
        || (private && mode & 0o077 != 0)
    {
        return Err("capture directory has unsafe type, owner or permissions".into());
    }
    Ok(())
}

fn validate_owned_directory(path: &Path, uid: u32, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let mode = metadata.mode();
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || mode & 0o022 != 0
        || (private && mode & 0o077 != 0)
    {
        return Err("capture directory has unsafe type, owner or permissions".into());
    }
    Ok(())
}

fn timestamp() -> Result<String> {
    let output = Command::new("/usr/bin/date")
        .env_clear()
        .env("LC_ALL", "C")
        .args(["-u", "+%Y_%m_%d_%H_%M_%S"])
        .output()?;
    if !output.status.success() {
        return Err("could not create a UTC screenshot name".into());
    }
    let value = String::from_utf8(output.stdout)?;
    let value = value.trim();
    if value.len() != 19
        || !value.bytes().enumerate().all(|(index, byte)| {
            if [4, 7, 10, 13, 16].contains(&index) { byte == b'_' } else { byte.is_ascii_digit() }
        })
    {
        return Err("date returned an invalid screenshot timestamp".into());
    }
    Ok(value.to_owned())
}

fn reserve_file(directory: &Path, name: &str) -> Result<(PathBuf, File)> {
    for suffix in 0..1000_u32 {
        let filename = if suffix == 0 {
            format!("{name}_screenshot.png")
        } else {
            format!("{name}_{suffix:03}_screenshot.png")
        };
        let path = directory.join(filename);
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not reserve a unique screenshot filename".into())
}

fn reserve_clipboard_file(uid: u32) -> Result<(PathBuf, File)> {
    let runtime =
        PathBuf::from(env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is unavailable")?);
    if !runtime.is_absolute() {
        return Err("XDG_RUNTIME_DIR must be absolute".into());
    }
    validate_owned_directory(&runtime, uid, true)?;
    for suffix in 0..1000_u32 {
        let path = runtime.join(format!("ryoku-capture-{}-{suffix}.png", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not reserve a temporary screenshot filename".into())
}

fn run_grim(target: &Target, path: &Path) -> Result<()> {
    let mut command = Command::new("/usr/bin/grim");
    match target {
        Target::All => {}
        Target::Monitor(name) => {
            command.args(["-o", name]);
        }
        Target::Region(geometry) => {
            command.args(["-g", geometry]);
        }
    }
    let status = command.arg(path).status()?;
    if !status.success() {
        return Err(format!("grim failed: {status}").into());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.len() == 0
    {
        return Err("grim did not produce a private, non-empty screenshot".into());
    }
    Ok(())
}

fn copy_to_clipboard(path: &Path) -> Result<()> {
    let status = Command::new(DAEMON).args(["clip-copy", "image/png"]).arg(path).status()?;
    if !status.success() {
        return Err(format!("Ryoku clipboard copy failed: {status}").into());
    }
    Ok(())
}

fn notify(title: &str, body: &str) {
    let _ = Command::new("/usr/bin/notify-send").args(["--app-name=ryoku", title, body]).status();
}

fn shutter_sound() {
    let _ = Command::new(DAEMON).args(["sound", "shutter"]).status();
}

fn open_beautify(path: &Path, uid: u32) -> Result<()> {
    let runtime =
        PathBuf::from(env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is unavailable")?);
    if !runtime.is_absolute() {
        return Err("XDG_RUNTIME_DIR must be absolute".into());
    }
    validate_owned_directory(&runtime, uid, true)?;
    let lock_path = runtime.join("ryoku-capture-beautify.lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock_path)
        && (metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0)
    {
        return Err("beautify lock has unsafe type, owner or permissions".into());
    }
    let lock =
        OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&lock_path)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err("beautify lock has unsafe type, owner or permissions".into());
    }
    drop(lock);
    Command::new("/usr/bin/flock")
        .args(["-n", "-o"])
        .arg(lock_path)
        .args([QUICKSHELL, "-c", "ryoshot"])
        .env("RYOSHOT_OPEN", path)
        .spawn()?;
    Ok(())
}

fn clipboard_failure_actions(
    persistent: bool,
    beautify: bool,
    remove_temporary: impl FnOnce(),
    play_shutter: impl FnOnce(),
    open_editor: impl FnOnce(),
) {
    if persistent {
        play_shutter();
        if beautify {
            open_editor();
        }
    } else {
        remove_temporary();
    }
}

fn capture(options: Options, uid: u32) -> Result<()> {
    let save_to_file = options.save != SaveMode::Clipboard || options.beautify;
    let copy_to_clipboard = options.save != SaveMode::File;
    let (path, _reservation, persistent) = if save_to_file {
        let directory = pictures_directory(uid)?;
        let (path, file) = reserve_file(&directory, &timestamp()?)?;
        (path, file, true)
    } else {
        let (path, file) = reserve_clipboard_file(uid)?;
        (path, file, false)
    };

    if let Err(error) = run_grim(&options.target, &path) {
        let _ = fs::remove_file(&path);
        return Err(error);
    }

    if copy_to_clipboard && let Err(error) = self::copy_to_clipboard(&path) {
        // The screenshot itself succeeded and remains useful even when
        // clipboard history is unavailable. Preserve the user's requested
        // editor handoff for that saved file while reporting the partial
        // failure through the normal error notification.
        clipboard_failure_actions(
            persistent,
            options.beautify,
            || {
                let _ = fs::remove_file(&path);
            },
            shutter_sound,
            || {
                let _ = open_beautify(&path, uid);
            },
        );
        let body = if persistent {
            format!("Screenshot saved to {}, but clipboard copy failed: {error}", path.display())
        } else {
            format!("Clipboard copy failed: {error}")
        };
        return Err(body.into());
    }

    shutter_sound();
    if copy_to_clipboard && persistent {
        notify("Screenshot saved and copied", &path.display().to_string());
    } else if copy_to_clipboard {
        notify("Screenshot copied to clipboard", "");
    } else {
        notify("Screenshot saved", &path.display().to_string());
    }
    if !persistent {
        let _ = fs::remove_file(&path);
    }
    if options.beautify && persistent {
        // The image capture itself has completed successfully. Beautify is an
        // optional editor handoff, so an unavailable editor does not invalidate
        // the saved image or clipboard result.
        let _ = open_beautify(&path, uid);
    }
    Ok(())
}

fn run() -> Result<()> {
    use rustix::process::{getegid, geteuid, getgid, getuid};
    let uid = getuid().as_raw();
    validate_identity(uid, geteuid().as_raw(), getgid().as_raw(), getegid().as_raw())?;
    let options = parse_args(&env::args().skip(1).collect::<Vec<_>>())?;
    capture(options, uid)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            notify("Screenshot failed", &error.to_string());
            eprintln!("ryoku-capture: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parse_accepts_each_target_without_shell_interpretation() {
        assert_eq!(
            parse_args(&args(&["--save", "both", "--all"])).unwrap(),
            Options { save: SaveMode::Both, beautify: false, target: Target::All }
        );
        assert_eq!(
            parse_args(&args(&["--save", "clipboard", "--monitor", "DP-1"])).unwrap().target,
            Target::Monitor("DP-1".into())
        );
        assert_eq!(
            parse_args(&args(&["--save", "file", "--region", "-1920,0 800x600"])).unwrap().target,
            Target::Region("-1920,0 800x600".into())
        );
    }

    #[test]
    fn parse_rejects_ambiguous_or_injected_targets() {
        assert!(parse_args(&args(&["--save", "both", "--all", "--all"])).is_err());
        assert!(parse_args(&args(&["--save", "both", "--monitor", "DP-1;touch"])).is_err());
        assert!(parse_args(&args(&["--save", "both", "--region", "0,0 100x100;id"])).is_err());
        assert!(parse_args(&args(&["--save", "clipboard", "--region", "0,0 0x10"])).is_err());
    }

    #[test]
    fn identity_must_be_an_unprivileged_matching_session() {
        assert!(validate_identity(1000, 1000, 100, 100).is_ok());
        assert!(validate_identity(0, 0, 0, 0).is_err());
        assert!(validate_identity(1000, 0, 100, 100).is_err());
        assert!(validate_identity(1000, 1000, 100, 0).is_err());
    }

    #[test]
    fn screenshot_directory_creation_rejects_symlinked_parents() {
        let unique =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root =
            env::temp_dir().join(format!("ryoku-capture-dir-{}-{unique}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let uid = rustix::process::getuid().as_raw();

        let nested = root.join("new/Pictures");
        let result = screenshot_directory(&nested, uid).unwrap();
        assert!(result.is_dir());
        assert_eq!(fs::symlink_metadata(&result).unwrap().mode() & 0o022, 0);

        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, root.join("link")).unwrap();
        assert!(screenshot_directory(&root.join("link/Pictures"), uid).is_err());

        let hostile = root.join("hostile");
        fs::create_dir(&hostile).unwrap();
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, hostile.join("Screenshots")).unwrap();
        assert!(screenshot_directory(&hostile, uid).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn output_reservation_never_overwrites_and_keeps_private_mode() {
        let unique =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory =
            env::temp_dir().join(format!("ryoku-capture-test-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let (first_path, first) = reserve_file(&directory, "2026_10_18_12_00_00").unwrap();
        drop(first);
        fs::write(&first_path, b"first image").unwrap();
        let (second_path, second) = reserve_file(&directory, "2026_10_18_12_00_00").unwrap();
        drop(second);

        assert_ne!(first_path, second_path);
        assert_eq!(fs::read(&first_path).unwrap(), b"first image");
        assert_eq!(fs::metadata(&second_path).unwrap().mode() & 0o777, 0o600);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn clipboard_failure_keeps_and_opens_saved_image_but_removes_temporary_file() {
        let events = std::cell::RefCell::new(Vec::new());
        clipboard_failure_actions(
            true,
            true,
            || events.borrow_mut().push("remove"),
            || events.borrow_mut().push("shutter"),
            || events.borrow_mut().push("editor"),
        );
        assert_eq!(*events.borrow(), ["shutter", "editor"]);

        events.borrow_mut().clear();
        clipboard_failure_actions(
            false,
            true,
            || events.borrow_mut().push("remove"),
            || events.borrow_mut().push("shutter"),
            || events.borrow_mut().push("editor"),
        );
        assert_eq!(*events.borrow(), ["remove"]);
    }
}
