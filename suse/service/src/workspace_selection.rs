//! Read-only Hyprland selection and captured-file resolution for manual
//! assignment. No user-supplied socket, executable, command or prepared plan.
use ryoku_recovery::wallpaper_protocol::{self as wire, ActorIdentity, WallpaperKind};
use ryoku_recovery::workspace_protocol::{
    PreparedWorkspace, WorkspaceOperation, WorkspaceRequest, WorkspaceSelector, WorkspaceWallpaper,
};
use serde::Deserialize;
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
    time::{Duration, Instant},
};

const MAX_MONITORS: usize = 32;
const MAX_REPLY: u64 = 256 * 1024;
#[derive(Deserialize)]
struct Workspace {
    id: i64,
    name: String,
}
#[derive(Deserialize)]
struct Monitor {
    name: String,
    focused: bool,
    #[serde(rename = "activeWorkspace")]
    active: Workspace,
}

fn workspace_id(value: &Workspace) -> String {
    if !value.name.is_empty() {
        value.name.clone()
    } else if value.id != 0 {
        value.id.to_string()
    } else {
        String::new()
    }
}
pub(crate) fn select_target(
    monitors: &[u8],
    selector: &WorkspaceSelector,
) -> io::Result<(WorkspaceSelector, bool)> {
    selector.validate().map_err(io::Error::other)?;
    let monitors: Vec<Monitor> = serde_json::from_slice(monitors).map_err(io::Error::other)?;
    if monitors.len() > MAX_MONITORS {
        return Err(io::Error::other("workspace monitor capacity"));
    }
    let mut names = std::collections::BTreeSet::new();
    for monitor in &monitors {
        if monitor.name.is_empty() || !names.insert(&monitor.name) {
            return Err(io::Error::other("ambiguous workspace output"));
        }
    }
    let matches: Vec<_> = monitors
        .iter()
        .filter(|monitor| {
            if selector.output.is_empty() {
                monitor.focused
            } else {
                monitor.name == selector.output
            }
        })
        .collect();
    if matches.is_empty() && !selector.id.is_empty() && !selector.output.is_empty() {
        return Ok((selector.clone(), false));
    }
    if matches.len() != 1 {
        return Err(io::Error::other("choose one active workspace output"));
    }
    let monitor = matches[0];
    let current = workspace_id(&monitor.active);
    if current.is_empty() && selector.id.is_empty() {
        return Err(io::Error::other("output has no active workspace"));
    }
    let target = WorkspaceSelector {
        id: if selector.id.is_empty() { current.clone() } else { selector.id.clone() },
        name: if selector.id.is_empty() || selector.name.is_empty() && selector.id == current {
            monitor.active.name.clone()
        } else {
            selector.name.clone()
        },
        output: monitor.name.clone(),
    };
    target.validate().map_err(io::Error::other)?;
    let active = target.id == current;
    Ok((target, active))
}

fn process_start(pid: u32) -> io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(") ")
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::other("process start identity missing"))
}

fn actor_value(actor: &ActorIdentity, uid: u32, key: &[u8]) -> io::Result<Option<String>> {
    if process_start(actor.pid)? != actor.start_time {
        return Err(io::Error::other("stale actor identity"));
    }
    let status = std::fs::read_to_string(format!("/proc/{}/status", actor.pid))?;
    let ids = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or_else(|| io::Error::other("actor account missing"))?
        .split_whitespace()
        .collect::<Vec<_>>();
    if ids.len() != 4 || ids.iter().any(|id| id.parse::<u32>() != Ok(uid)) {
        return Err(io::Error::other("actor account mismatch"));
    }
    let mut bytes = Vec::new();
    File::open(format!("/proc/{}/environ", actor.pid))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::other("actor environment capacity"));
    }
    let mut selected = None;
    for entry in bytes.split(|byte| *byte == 0) {
        if let Some(value) = entry.strip_prefix(key) {
            if selected.is_some() || value.len() > 4096 {
                return Err(io::Error::other("ambiguous session value"));
            }
            let value = String::from_utf8(value.to_vec()).map_err(io::Error::other)?;
            if value.chars().any(char::is_control) {
                return Err(io::Error::other("invalid session value"));
            }
            selected = Some(value);
        }
    }
    if process_start(actor.pid)? != actor.start_time {
        return Err(io::Error::other("actor identity changed"));
    }
    Ok(selected)
}
fn actor_signature(actor: &ActorIdentity, uid: u32) -> io::Result<String> {
    let signature = actor_value(actor, uid, b"HYPRLAND_INSTANCE_SIGNATURE=")?
        .ok_or_else(|| io::Error::other("the wallpaper actor has no bound Hyprland session"))?;
    if signature.is_empty()
        || signature.len() > 96
        || !signature.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err(io::Error::other("invalid Hyprland session identity"));
    }
    Ok(signature)
}

fn first_static(home: &File, uid: u32, directory: &Path) -> io::Result<Option<String>> {
    let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))?;
    let share = File::open("/usr/share")?;
    let (root, relative, owner) = if let Ok(relative) = directory.strip_prefix(&home_path) {
        (home, relative, uid)
    } else {
        (&share, directory.strip_prefix("/usr/share").map_err(io::Error::other)?, 0)
    };
    if relative.components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
        return Err(io::Error::other("noncanonical wallpaper directory"));
    }
    let opened = match rustix::fs::openat2(
        root,
        if relative.as_os_str().is_empty() { Path::new(".") } else { relative },
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS
            | rustix::fs::ResolveFlags::NO_XDEV,
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = opened.metadata()?;
    if metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other("wallpaper directory is not a stable account source"));
    }
    let mut selected: Option<String> = None;
    let mut bytes = 0_usize;
    for entry in std::fs::read_dir(format!("/proc/self/fd/{}", opened.as_raw_fd()))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("wallpaper name is not UTF-8"))?;
        bytes = bytes.saturating_add(name.len());
        if bytes > 16 * 1024 * 1024 {
            return Err(io::Error::other(
                "default wallpaper directory exceeds verified name capacity",
            ));
        }
        if name.starts_with('.') || entry.file_type()?.is_dir() {
            continue;
        }
        let extension = Path::new(&name)
            .extension()
            .and_then(|part| part.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !["jpg", "jpeg", "png", "webp", "bmp", "gif", "tiff", "tif", "avif"]
            .contains(&extension.as_str())
        {
            continue;
        }
        if selected.as_ref().is_none_or(|old| name < *old) {
            selected = Some(name);
        }
    }
    Ok(selected.map(|name| directory.join(name).to_string_lossy().into_owned()))
}
pub(crate) fn default_source(
    actor: &ActorIdentity,
    uid: u32,
    home: &File,
    settings: Option<&[u8]>,
) -> io::Result<Option<String>> {
    let home_path = std::fs::read_link(format!("/proc/self/fd/{}", home.as_raw_fd()))?;
    let settings: serde_json::Value = settings
        .map(serde_json::from_slice)
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(directory) =
        settings["paths"]["wallpaper"].as_str().filter(|value| !value.is_empty())
    {
        let directory = if directory == "~" {
            home_path.clone()
        } else if let Some(tail) = directory.strip_prefix("~/") {
            home_path.join(tail)
        } else {
            directory.into()
        };
        return first_static(home, uid, &directory);
    }
    let localized = actor_value(actor, uid, b"XDG_PICTURES_DIR=")?
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home_path.join("Pictures"));
    let directory = localized.join("Wallpapers");
    if directory.is_dir() {
        return first_static(home, uid, &directory);
    }
    first_static(home, uid, &home_path.join("Pictures/Wallpapers"))
}

pub(crate) fn read_target(
    actor: &ActorIdentity,
    uid: u32,
    selector: &WorkspaceSelector,
) -> io::Result<(WorkspaceSelector, bool)> {
    let signature = actor_signature(actor, uid)?;
    let root = File::open("/")?;
    let directory = File::from(rustix::fs::openat2(
        &root,
        format!("run/user/{uid}/hypr/{signature}"),
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS,
    )?);
    let metadata = directory.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other("unsafe Hyprland session directory"));
    }
    let expected = File::from(rustix::fs::openat2(
        &root,
        "usr/bin/Hyprland",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
        rustix::fs::ResolveFlags::BENEATH
            | rustix::fs::ResolveFlags::NO_SYMLINKS
            | rustix::fs::ResolveFlags::NO_MAGICLINKS,
    )?);
    let identity = expected.metadata()?;
    if !identity.is_file() || identity.uid() != 0 || identity.mode() & 0o022 != 0 {
        return Err(io::Error::other("unsafe compositor image"));
    }
    let reply =
        compositor_reply(&directory, uid, &identity, Instant::now() + Duration::from_secs(2))?;
    select_target(&reply, selector)
}
fn compositor_reply(
    directory: &File,
    uid: u32,
    identity: &std::fs::Metadata,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    let socket =
        rustix::fs::statat(directory, ".socket.sock", rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
    if rustix::fs::FileType::from_raw_mode(socket.st_mode) != rustix::fs::FileType::Socket
        || socket.st_uid != uid
        || socket.st_mode & 0o022 != 0
    {
        return Err(io::Error::other("unsafe compositor socket"));
    }
    let path = format!("/proc/self/fd/{}/.socket.sock", directory.as_raw_fd());
    let mut stream = crate::helper_protocol::connect_helper_until(Path::new(&path), deadline)?;
    // The helper connector deliberately returns O_NONBLOCK; this private
    // one-query reader uses absolute remaining SO_RCVTIMEO/SO_SNDTIMEO bounds.
    stream.set_nonblocking(false)?;
    let peer = rustix::net::sockopt::socket_peercred(&stream)?;
    if peer.uid.as_raw() != uid {
        return Err(io::Error::other("Hyprland peer account mismatch"));
    }
    let image = File::open(format!("/proc/{}/exe", peer.pid.as_raw_pid()))?.metadata()?;
    if (identity.dev(), identity.ino()) != (image.dev(), image.ino()) {
        return Err(io::Error::other("Hyprland peer image mismatch"));
    }
    let peer_start = process_start(peer.pid.as_raw_pid() as u32)?;
    stream.set_write_timeout(Some(
        deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "compositor query deadline"))?,
    ))?;
    stream.write_all(b"j/monitors all")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "compositor query deadline"))?;
        stream.set_read_timeout(Some(remaining))?;
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if reply.len().saturating_add(count) > MAX_REPLY as usize {
            return Err(io::Error::other("workspace reply capacity"));
        }
        reply.extend_from_slice(&chunk[..count]);
    }
    let after = File::open(format!("/proc/{}/exe", peer.pid.as_raw_pid()))?.metadata()?;
    if process_start(peer.pid.as_raw_pid() as u32)? != peer_start
        || (identity.dev(), identity.ino()) != (after.dev(), after.ino())
    {
        return Err(io::Error::other("compositor identity changed"));
    }

    Ok(reply)
}

pub(crate) struct StoredWorkspace<'a> {
    pub home: &'a Path,
    pub outputs: Option<&'a [u8]>,
    pub workspaces: Option<&'a [u8]>,
    pub settings: Option<&'a [u8]>,
}
pub(crate) fn prepare(
    request: &WorkspaceRequest,
    target: WorkspaceSelector,
    active: bool,
    stored: StoredWorkspace<'_>,
    default_source: impl FnOnce() -> io::Result<Option<String>>,
) -> io::Result<PreparedWorkspace> {
    let StoredWorkspace { home, outputs, workspaces, settings } = stored;
    request.validate().map_err(io::Error::other)?;
    let workspace_state: serde_json::Value = workspaces
        .map(serde_json::from_slice)
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or_else(|| serde_json::json!({"version":1,"assignments":{}}));
    let assignments = workspace_state["assignments"]
        .as_object()
        .ok_or_else(|| io::Error::other("invalid workspace assignments"))?;
    let outputs_sha256 = outputs.map(wire::digest);
    let outputs: serde_json::Value = outputs
        .map(serde_json::from_slice)
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or_else(|| serde_json::json!({}));
    let settings: serde_json::Value = settings
        .map(serde_json::from_slice)
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or_else(|| serde_json::json!({}));
    let defaults = (
        settings["wallpaperMute"].as_bool().unwrap_or(true),
        settings["wallpaperVolume"].as_f64().unwrap_or(100.0).round().clamp(0.0, 100.0) as u8,
    );
    let key = format!("hyprland:{}", target.id);
    let mut matching = assignments
        .iter()
        .filter(|(_, entry)| {
            entry["provider"] == "hyprland"
                && entry["name"] == target.name
                && !target.name.is_empty()
        })
        .collect::<Vec<_>>();
    matching.sort_by_key(|(key, entry)| (entry["output"] != target.output, (*key).clone()));
    let existing = assignments
        .get(&key)
        .map(|value| (key.clone(), value))
        .or_else(|| matching.first().map(|(key, value)| ((*key).clone(), *value)));
    let remove_keys = existing.as_ref().map(|(key, _)| vec![key.clone()]).unwrap_or_default();
    let fallback = outputs.get(&target.output).or_else(|| outputs.get("*"));
    let selected = if request.operation == WorkspaceOperation::Unassign {
        if active { fallback } else { None }
    } else {
        existing.as_ref().map(|(_, value)| *value).or(fallback)
    };
    let wallpaper = if let Some(intent) = &request.wallpaper {
        Some(WorkspaceWallpaper {
            kind: intent.kind,
            path: intent.path.clone(),
            mute: intent.mute.unwrap_or(defaults.0),
            volume: intent.volume.unwrap_or(defaults.1),
        })
    } else if let Some(entry) = selected {
        let path = entry["path"]
            .as_str()
            .ok_or_else(|| io::Error::other("stored wallpaper has no source"))?;
        let path = if let Some(tail) = path.strip_prefix("~/") {
            home.join(tail).to_string_lossy().into_owned()
        } else {
            path.to_owned()
        };
        let kind = match entry["type"].as_str() {
            Some("static") => WallpaperKind::Static,
            Some("video") => WallpaperKind::Video,
            _ => {
                return Err(io::Error::other(
                    "external wallpaper requires its provider route (#175)",
                ));
            }
        };
        Some(WorkspaceWallpaper {
            kind,
            path,
            mute: entry["mute"].as_bool().unwrap_or(defaults.0),
            volume: entry["volume"]
                .as_u64()
                .filter(|v| *v <= 100)
                .map(|v| v as u8)
                .unwrap_or(defaults.1),
        })
    } else if request.operation == WorkspaceOperation::Assign || active && !remove_keys.is_empty() {
        default_source()?.map(|path| WorkspaceWallpaper {
            kind: WallpaperKind::Static,
            path,
            mute: defaults.0,
            volume: defaults.1,
        })
    } else {
        None
    };
    let prepared = PreparedWorkspace {
        request: request.clone(),
        provider: "hyprland".into(),
        target,
        active,
        wallpaper,
        workspace_sha256: workspaces.map(wire::digest),
        outputs_sha256,
        remove_keys,
    };
    prepared.validate().map_err(io::Error::other)?;
    Ok(prepared)
}

pub(crate) fn verify_post(
    prepared: &PreparedWorkspace,
    state: &serde_json::Value,
    home: &Path,
) -> io::Result<()> {
    let assignments = state["assignments"]
        .as_object()
        .ok_or_else(|| io::Error::other("missing post-change assignments"))?;
    let key = format!("hyprland:{}", prepared.target.id);
    if prepared.request.operation == WorkspaceOperation::Unassign {
        if prepared.remove_keys.iter().any(|key| assignments.contains_key(key)) {
            return Err(io::Error::other("assignment removal not persisted"));
        }
    } else {
        let wall = prepared
            .wallpaper
            .as_ref()
            .ok_or_else(|| io::Error::other("missing prepared source"))?;
        let entry =
            assignments.get(&key).ok_or_else(|| io::Error::other("assignment not persisted"))?;
        let portable = Path::new(&wall.path)
            .strip_prefix(home)
            .ok()
            .map(|relative| format!("~/{}", relative.display()));
        let path =
            entry["path"].as_str().ok_or_else(|| io::Error::other("missing stored source"))?;
        let kind = match wall.kind {
            WallpaperKind::Static => "static",
            WallpaperKind::Video => "video",
        };
        if (path != wall.path && portable.as_deref() != Some(path))
            || entry["type"] != kind
            || entry["provider"] != "hyprland"
            || entry["id"] != prepared.target.id
            || entry["output"] != prepared.target.output
            || entry["name"].as_str().unwrap_or_default() != prepared.target.name
            || entry["mute"].as_bool() != Some(wall.mute)
            || entry["volume"].as_u64() != Some(u64::from(wall.volume))
            || prepared.remove_keys.iter().any(|old| old != &key && assignments.contains_key(old))
        {
            return Err(io::Error::other(
                "post-change assignment differs from authorized target/source/audio",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryoku_recovery::workspace_protocol::WorkspaceWallpaperIntent;
    fn prepare(
        request: &WorkspaceRequest,
        target: WorkspaceSelector,
        active: bool,
        home: &Path,
        outputs: Option<&[u8]>,
        workspaces: Option<&[u8]>,
        settings: Option<&[u8]>,
    ) -> io::Result<PreparedWorkspace> {
        super::prepare(
            request,
            target,
            active,
            StoredWorkspace { home, outputs, workspaces, settings },
            || Ok(None),
        )
    }
    struct SocketFixture {
        path: std::path::PathBuf,
        directory: File,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl SocketFixture {
        fn new(reply: Vec<u8>, delay: Duration) -> Self {
            use std::os::unix::{fs::PermissionsExt, net::UnixListener};
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "ryoku-workspace-socket-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            let listener = UnixListener::bind(path.join(".socket.sock")).unwrap();
            listener.set_nonblocking(true).unwrap();
            let worker = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_millis(500);
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                            let mut query = Vec::new();
                            let _ = stream.read_to_end(&mut query);
                            if !query.is_empty() {
                                assert_eq!(query, b"j/monitors all");
                                std::thread::sleep(delay);
                                let _ = stream.write_all(&reply);
                            }
                            return;
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("fixture accept: {error}"),
                    }
                }
            });
            Self { directory: File::open(&path).unwrap(), path, worker: Some(worker) }
        }
    }
    impl Drop for SocketFixture {
        fn drop(&mut self) {
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
            std::fs::remove_dir_all(&self.path).unwrap();
        }
    }
    #[test]
    fn post_state_matches_target_source_type_audio_and_exact_migration() {
        let request = WorkspaceRequest {
            operation: WorkspaceOperation::Assign,
            workspace: selector("DP-1"),
            wallpaper: None,
        };
        let (target, active) = select_target(&monitors(), &request.workspace).unwrap();
        let old=br#"{"version":1,"assignments":{"hyprland:old":{"provider":"hyprland","id":"old","name":"work","output":"DP-1","type":"static","path":"~/old.jpg"}}}"#;
        let prepared =
            prepare(&request, target, active, Path::new("/home/fixture"), None, Some(old), None)
                .unwrap();
        let state = serde_json::json!({"assignments":{"hyprland:work":{"provider":"hyprland","id":"work","name":"work","output":"DP-1","type":"static","path":"~/old.jpg","mute":true,"volume":100}}});
        assert!(verify_post(&prepared, &state, Path::new("/home/fixture")).is_ok());
        let mut wrong = state.clone();
        wrong["assignments"]["hyprland:work"]["type"] = "video".into();
        assert!(verify_post(&prepared, &wrong, Path::new("/home/fixture")).is_err());
        let mut wrong = state.clone();
        wrong["assignments"]["hyprland:old"] = serde_json::json!({});
        assert!(verify_post(&prepared, &wrong, Path::new("/home/fixture")).is_err());
        let mut removed = prepared;
        removed.request.operation = WorkspaceOperation::Unassign;
        removed.remove_keys = vec!["hyprland:work".into()];
        assert!(verify_post(&removed, &state, Path::new("/home/fixture")).is_err());
        assert!(
            verify_post(
                &removed,
                &serde_json::json!({"assignments":{}}),
                Path::new("/home/fixture")
            )
            .is_ok()
        );
    }
    #[test]
    fn readonly_compositor_ipc_is_peer_bound_bounded_and_never_dispatches_mutations() {
        let uid = rustix::process::geteuid().as_raw();
        let image = std::fs::metadata(std::env::current_exe().unwrap()).unwrap();
        let fixture = SocketFixture::new(monitors(), Duration::ZERO);
        let reply = compositor_reply(
            &fixture.directory,
            uid,
            &image,
            Instant::now() + Duration::from_millis(500),
        )
        .unwrap();
        assert_eq!(select_target(&reply, &selector("DP-1")).unwrap().0.id, "work");
        let fixture = SocketFixture::new(monitors(), Duration::ZERO);
        assert!(
            compositor_reply(
                &fixture.directory,
                uid + 1,
                &image,
                Instant::now() + Duration::from_millis(100)
            )
            .is_err()
        );
        let fixture = SocketFixture::new(monitors(), Duration::ZERO);
        let wrong = std::fs::metadata("/usr/bin/sleep").unwrap();
        assert!(
            compositor_reply(
                &fixture.directory,
                uid,
                &wrong,
                Instant::now() + Duration::from_millis(100)
            )
            .is_err()
        );
        let fixture = SocketFixture::new(vec![b'x'; MAX_REPLY as usize + 1], Duration::ZERO);
        assert!(
            compositor_reply(
                &fixture.directory,
                uid,
                &image,
                Instant::now() + Duration::from_millis(500)
            )
            .is_err()
        );
        let fixture = SocketFixture::new(monitors(), Duration::from_millis(80));
        let start = Instant::now();
        assert!(
            compositor_reply(&fixture.directory, uid, &image, start + Duration::from_millis(20))
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_millis(500));
    }
    #[test]
    fn compositor_peer_exec_replacement_is_not_the_prepared_session() {
        use std::os::unix::{net::UnixListener, process::CommandExt};
        const ROLE: &str = "RYOKU_WORKSPACE_PEER_ROLE";
        const DIRECTORY: &str = "RYOKU_WORKSPACE_PEER_DIR";
        if std::env::var(ROLE).as_deref() == Ok("peer") {
            let directory = std::path::PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
            let listener = UnixListener::bind(directory.join(".socket.sock")).unwrap();
            std::fs::write(directory.join("ready"), b"1").unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut query = Vec::new();
            stream.read_to_end(&mut query).unwrap();
            assert_eq!(query, b"j/monitors all");
            stream.write_all(&monitors()).unwrap();
            panic!(
                "fixture exec failed: {}",
                std::process::Command::new("/usr/bin/sleep").arg("1").exec()
            );
        }
        let directory =
            std::env::temp_dir().join(format!("ryoku-workspace-peer-exec-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","workspace_selection::tests::compositor_peer_exec_replacement_is_not_the_prepared_session","--nocapture"]).env(ROLE,"peer").env(DIRECTORY,&directory).stdout(std::process::Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !directory.join("ready").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let expected = std::fs::metadata(std::env::current_exe().unwrap()).unwrap();
        let result = compositor_reply(
            &File::open(&directory).unwrap(),
            rustix::process::geteuid().as_raw(),
            &expected,
            Instant::now() + Duration::from_secs(1),
        );
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(directory).unwrap();
        assert!(result.is_err(), "exec replacement accepted");
    }
    #[test]
    fn static_library_fallback_is_ordered_bounded_and_does_not_recurse_into_media() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("ryoku-workspace-library-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Pictures/Wallpapers/nested")).unwrap();
        for name in ["z.png", "a.jpg", ".hidden.png", "clip.mp4", "nested/first.png"] {
            std::fs::write(root.join("Pictures/Wallpapers").join(name), b"fixture").unwrap();
        }
        let home = File::open(&root).unwrap();
        let directory = root.join("Pictures/Wallpapers");
        assert_eq!(
            first_static(&home, rustix::process::geteuid().as_raw(), &directory).unwrap(),
            Some(directory.join("a.jpg").to_string_lossy().into_owned())
        );
        std::fs::write(root.join("home.png"), b"fixture").unwrap();
        let actor = ActorIdentity {
            pid: std::process::id(),
            start_time: process_start(std::process::id()).unwrap(),
        };
        assert_eq!(
            default_source(
                &actor,
                rustix::process::geteuid().as_raw(),
                &home,
                Some(br#"{"paths":{"wallpaper":"~"}}"#)
            )
            .unwrap(),
            Some(root.join("home.png").to_string_lossy().into_owned())
        );
        symlink(&directory, root.join("linked")).unwrap();
        assert!(
            first_static(&home, rustix::process::geteuid().as_raw(), &root.join("linked")).is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    fn selector(output: &str) -> WorkspaceSelector {
        WorkspaceSelector { output: output.into(), ..Default::default() }
    }
    fn monitors() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!([
            {"name":"DP-1","focused":true,"activeWorkspace":{"id":1,"name":"work"}},
            {"name":"HDMI-A-1","focused":false,"activeWorkspace":{"id":2,"name":"2"}}
        ]))
        .unwrap()
    }
    #[test]
    fn focus_and_screen_resolve_using_the_upstream_name_first_identity() {
        let (focused, active) = select_target(&monitors(), &selector("")).unwrap();
        assert!(active);
        assert_eq!(focused.id, "work");
        assert_eq!(focused.output, "DP-1");
        let (second, active) = select_target(&monitors(), &selector("HDMI-A-1")).unwrap();
        assert!(active);
        assert_eq!(second.id, "2");
        let future =
            WorkspaceSelector { id: "future".into(), output: "DP-1".into(), name: String::new() };
        let (target, active) = select_target(&monitors(), &future).unwrap();
        assert!(!active);
        assert!(target.name.is_empty());
        let future =
            WorkspaceSelector { id: "future".into(), output: "DP-9".into(), name: "Later".into() };
        assert_eq!(select_target(b"[]", &future).unwrap(), (future, false));
        assert!(select_target(b"[]", &selector("")).is_err());
        let mut ambiguous: serde_json::Value = serde_json::from_slice(&monitors()).unwrap();
        ambiguous[1]["focused"] = true.into();
        assert!(select_target(&serde_json::to_vec(&ambiguous).unwrap(), &selector("")).is_err());
    }
    #[test]
    fn captured_assignment_and_output_fallback_hashes_bind_current_screen_intent() {
        let request = WorkspaceRequest {
            operation: WorkspaceOperation::Assign,
            workspace: selector("DP-1"),
            wallpaper: None,
        };
        let (target, active) = select_target(&monitors(), &request.workspace).unwrap();
        let outputs = br#"{"*":{"type":"static","path":"~/fallback.png","mute":true,"volume":80}}"#;
        let workspaces=br#"{"version":1,"assignments":{"hyprland:old":{"provider":"hyprland","id":"old","name":"work","output":"DP-1","type":"video","path":"~/current.mp4","mute":false,"volume":25}}}"#;
        let prepared = prepare(
            &request,
            target.clone(),
            active,
            Path::new("/home/fixture"),
            Some(outputs),
            Some(workspaces),
            None,
        )
        .unwrap();
        assert_eq!(prepared.remove_keys, ["hyprland:old"]);
        assert_eq!(prepared.wallpaper.as_ref().unwrap().path, "/home/fixture/current.mp4");
        assert_eq!(prepared.wallpaper.as_ref().unwrap().volume, 25);
        assert_eq!(prepared.outputs_sha256, Some(wire::digest(outputs)));
        assert_eq!(prepared.workspace_sha256, Some(wire::digest(workspaces)));
        let unassign = WorkspaceRequest { operation: WorkspaceOperation::Unassign, ..request };
        let fallback = prepare(
            &unassign,
            target,
            active,
            Path::new("/home/fixture"),
            Some(outputs),
            Some(workspaces),
            None,
        )
        .unwrap();
        assert_eq!(fallback.wallpaper.unwrap().path, "/home/fixture/fallback.png");
    }
    #[test]
    fn explicit_source_inherits_configured_audio_and_external_state_never_becomes_a_plan() {
        let request = WorkspaceRequest {
            operation: WorkspaceOperation::Assign,
            workspace: selector("DP-1"),
            wallpaper: Some(WorkspaceWallpaperIntent {
                kind: WallpaperKind::Static,
                path: "/home/fixture/chosen.png".into(),
                mute: None,
                volume: None,
            }),
        };
        let (target, active) = select_target(&monitors(), &request.workspace).unwrap();
        let prepared = prepare(
            &request,
            target.clone(),
            active,
            Path::new("/home/fixture"),
            None,
            None,
            Some(br#"{"wallpaperMute":false,"wallpaperVolume":35}"#),
        )
        .unwrap();
        assert!(!prepared.wallpaper.as_ref().unwrap().mute);
        assert_eq!(prepared.wallpaper.as_ref().unwrap().volume, 35);
        let request = WorkspaceRequest { wallpaper: None, ..request };
        assert!(
            prepare(
                &request,
                target.clone(),
                active,
                Path::new("/home/fixture"),
                Some(br#"{"*":{"type":"we","path":"steam"}}"#),
                None,
                None
            )
            .is_err()
        );
        assert!(
            prepare(&request, target, active, Path::new("/home/fixture"), None, None, None)
                .is_err()
        );
    }
    #[test]
    fn actor_environment_rejects_stale_start_and_foreign_account_before_selection() {
        let actor = ActorIdentity {
            pid: std::process::id(),
            start_time: process_start(std::process::id()).unwrap(),
        };
        assert!(actor_signature(&actor, u32::MAX).is_err());
        let stale = ActorIdentity { start_time: actor.start_time + 1, ..actor };
        assert!(actor_signature(&stale, rustix::process::geteuid().as_raw()).is_err());
    }
}
