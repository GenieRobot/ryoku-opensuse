//! OS-backed runtime/schema/session guard for the one-use capture worker.
//! A root-provided receipt is structural data here; native checkpoint
//! availability and durable enrollment are verified by the root coordinator.
use crate::theme_gio::GioThemeSettingsActor;
use ryoku_recovery::{
    Sha256,
    logical_settings::SessionWitness,
    theme_actor::{CaptureRequest, control_lifetime},
    theme_settings_actor::ThemeSettingsActor,
    wallpaper_protocol::digest,
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
};
struct Pin {
    file: File,
    identity: (u64, u64, u64, i64, i64, i64, i64),
}
fn identity(metadata: &std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    )
}
impl Pin {
    fn open(path: &Path, expected: &Sha256) -> io::Result<Self> {
        let mut file = File::from(rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
        )?);
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.mode() & 0o6022 != 0
            || metadata.len() > 64 * 1024 * 1024
        {
            return Err(io::Error::other(
                "actor runtime must be a bounded root-owned immutable file",
            ));
        }
        let mut bytes = Vec::new();
        file.by_ref().take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 64 * 1024 * 1024 || digest(&bytes) != expected.as_str() {
            return Err(io::Error::other(
                "actor runtime/schema content differs from approved bytes",
            ));
        }
        let pin = Self { file, identity: identity(&metadata) };
        pin.check(path)?;
        Ok(pin)
    }
    fn check(&self, path: &Path) -> io::Result<()> {
        if identity(&self.file.metadata()?) != self.identity
            || identity(&std::fs::symlink_metadata(path)?) != self.identity
        {
            return Err(io::Error::other("actor runtime/schema image was replaced or changed"));
        }
        Ok(())
    }
}
fn start_time(pid: u32) -> io::Result<u64> {
    let mut text = String::new();
    File::open(format!("/proc/{pid}/stat"))?.take(8193).read_to_string(&mut text)?;
    if text.len() > 8192 {
        return Err(io::Error::other("oversized process identity"));
    }
    text.rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or_else(|| io::Error::other("missing process identity"))?
        .parse()
        .map_err(io::Error::other)
}
fn allowed_runtime_path(path: &Path) -> bool {
    path.starts_with("/usr/lib")
        || path.starts_with("/lib")
        || path.starts_with("/lib64")
        || path == Path::new("/etc/ld.so.cache")
        || path == Path::new("/usr/share/glib-2.0/schemas/gschemas.compiled")
        || path == Path::new("/usr/libexec/ryoku/ryoku-theme-actor")
}
pub struct CaptureScope {
    control: UnixStream,
    pins: BTreeMap<PathBuf, Pin>,
    session: SessionWitness,
    process: File,
}
impl CaptureScope {
    pub fn open(request: &CaptureRequest, control: UnixStream) -> io::Result<Self> {
        if rustix::net::sockopt::socket_peercred(&control)?.uid.as_raw() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "capture scope peer is not root",
            ));
        }
        if request.session.uid != request.binding.uid
            || request.session.digest()? != request.binding.session_service_sha256
            || request.session.process.pid == 0
            || request.runtime_files.is_empty()
            || request.runtime_files.len() > 128
        {
            return Err(io::Error::other("capture runtime/session declaration mismatch"));
        }
        let mut pins = BTreeMap::new();
        for entry in &request.runtime_files {
            let path = PathBuf::from(&entry.path);
            if path.as_os_str().len() > 512
                || !allowed_runtime_path(&path)
                || std::fs::canonicalize(&path)? != path
                || pins.contains_key(&path)
            {
                return Err(io::Error::other("ambiguous/unsupported actor runtime declaration"));
            }
            pins.insert(path.clone(), Pin::open(&path, &entry.sha256)?);
        }
        let executable = std::fs::canonicalize("/proc/self/exe")?;
        if !pins.contains_key(&executable) {
            return Err(io::Error::other("capture worker itself is not in its approved runtime"));
        }
        Pin::open(&executable, &request.binding.executable_sha256)?;
        let schema = Path::new("/usr/share/glib-2.0/schemas/gschemas.compiled");
        if !pins.contains_key(schema) {
            return Err(io::Error::other("capture schema is not in its approved runtime"));
        }
        Pin::open(schema, &request.binding.schema_sha256)?;
        let process = File::from(rustix::process::pidfd_open(
            rustix::process::Pid::from_raw(request.session.process.pid as i32)
                .ok_or_else(|| io::Error::other("invalid session PID"))?,
            rustix::process::PidfdFlags::empty(),
        )?);
        let scope = Self { control, pins, session: request.session.clone(), process };
        scope.check()?;
        Ok(scope)
    }
    pub fn check(&self) -> io::Result<()> {
        control_lifetime(&self.control)?;
        let mut events = [rustix::event::PollFd::new(&self.process, rustix::event::PollFlags::IN)];
        if rustix::event::poll(&mut events, Some(&rustix::event::Timespec::default()))? != 0 {
            return Err(io::Error::other("authenticated session process exited"));
        }
        for (path, pin) in &self.pins {
            pin.check(path)?;
        }
        let socket = PathBuf::from(format!("/run/user/{}/bus", self.session.uid));
        let directory = std::fs::symlink_metadata(socket.parent().expect("fixed parent"))?;
        let metadata = std::fs::symlink_metadata(&socket)?;
        if !directory.is_dir()
            || directory.uid() != self.session.uid
            || directory.mode() & 0o7777 != 0o700
            || !metadata.file_type().is_socket()
            || metadata.uid() != self.session.uid
            || (metadata.dev(), metadata.ino()) != (self.session.device, self.session.inode)
        {
            return Err(io::Error::other("authenticated session endpoint changed"));
        }
        let stream = UnixStream::connect(&socket)?;
        let peer = rustix::net::sockopt::socket_peercred(&stream)?;
        if peer.uid.as_raw() != self.session.uid
            || peer.pid.as_raw_pid() as u32 != self.session.process.pid
            || start_time(self.session.process.pid)? != self.session.process.start_time
        {
            return Err(io::Error::other("authenticated session process changed"));
        }
        let executable = std::fs::canonicalize(format!("/proc/{}/exe", self.session.process.pid))?;
        Pin::open(&executable, &self.session.executable_sha256)?;
        // Verify every executable file mapping, including late GIO modules.
        let mut maps = String::new();
        File::open("/proc/self/maps")?.take(1024 * 1024 + 1).read_to_string(&mut maps)?;
        if maps.len() > 1024 * 1024 {
            return Err(io::Error::other("actor executable map exceeds bound"));
        }
        for line in maps.lines() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.get(1).is_some_and(|permissions| permissions.contains('x'))
                && let Some(path) = fields.get(5).filter(|path| path.starts_with('/'))
            {
                let path = Path::new(path);
                if !self.pins.contains_key(path) {
                    return Err(io::Error::other(
                        "unapproved executable mapping in GSettings worker",
                    ));
                }
            }
        }
        Ok(())
    }
}
pub fn native_factory(
    request: &CaptureRequest,
    control: UnixStream,
) -> io::Result<Box<dyn ThemeSettingsActor>> {
    let scope = CaptureScope::open(request, control)?;
    let expected = request.binding.clone();
    let binding = expected.clone();
    let actor = GioThemeSettingsActor::for_authenticated_worker(
        expected,
        Box::new(move || {
            scope.check()?;
            Ok(binding.clone())
        }),
    )?;
    Ok(Box::new(actor))
}
