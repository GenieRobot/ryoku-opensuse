//! Rust implementation of the pinned Ryoku ten-band PipeWire equalizer CLI.
//!
//! Persistent preferences stay in the ordinary user's XDG config directory.
//! The smart filter is a per-user PipeWire client and never calls a privileged
//! helper or changes system audio policy.

use std::{
    env,
    error::Error,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const FREQUENCIES: [u32; 10] = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
const NODE_IN: &str = "ryoku_equalizer";
const UNIT: &str = "ryoku-eq.service";
const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_COMMAND_OUTPUT: usize = 4 * 1024 * 1024;
const NODE_START_ATTEMPTS: usize = 40;
const NODE_START_INTERVAL: Duration = Duration::from_millis(50);
const PRESETS: &[(&str, [i32; 10])] = &[
    ("Flat", [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    ("Bass", [5, 7, 5, 2, 1, 0, 0, 0, 1, 2]),
    ("Treble", [-2, -1, 0, 1, 2, 3, 4, 5, 6, 6]),
    ("Vocal", [-2, -1, 1, 3, 5, 5, 4, 2, 1, 0]),
    ("Pop", [2, 4, 2, 0, 1, 2, 4, 2, 1, 2]),
    ("Rock", [5, 4, 2, -1, -2, -1, 2, 4, 5, 6]),
    ("Jazz", [3, 3, 1, 1, 1, 1, 2, 1, 2, 3]),
    ("Classic", [0, 1, 2, 2, 2, 2, 1, 2, 3, 4]),
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    gains: [i32; 10],
    preset: String,
    on: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { gains: [0; 10], preset: "Flat".into(), on: false }
    }
}

impl State {
    fn from_json(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or("equalizer state must be a JSON object")?;
        let mut state = Self::default();
        for (index, gain) in state.gains.iter_mut().enumerate() {
            let key = format!("b{}", index + 1);
            if let Some(value) = object.get(&key) {
                let value = value.as_i64().ok_or("equalizer gain must be an integer")?;
                if !(-12..=12).contains(&value) {
                    return Err("equalizer gain is outside -12..12 dB".into());
                }
                *gain = value as i32;
            }
        }
        if let Some(value) = object.get("preset") {
            let value = value.as_str().ok_or("equalizer preset must be a string")?;
            if value != "Custom" && !PRESETS.iter().any(|(name, _)| *name == value) {
                return Err("equalizer preset is unknown".into());
            }
            state.preset = value.to_owned();
        }
        if let Some(value) = object.get("on") {
            state.on = value.as_bool().ok_or("equalizer on value must be boolean")?;
        }
        Ok(state)
    }

    fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        for (index, gain) in self.gains.iter().enumerate() {
            object.insert(format!("b{}", index + 1), json!(gain));
        }
        object.insert("preset".into(), json!(self.preset));
        object.insert("on".into(), json!(self.on));
        Value::Object(object)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Action {
    Get,
    SetBand { index: usize, gain: i32 },
    Preset(String),
    On(bool),
    Apply,
    Retarget,
}

fn parse_args(args: &[String]) -> Result<Action> {
    match args {
        [] => Ok(Action::Get),
        [arg] if arg == "get" => Ok(Action::Get),
        [verb, index, gain] if verb == "set-band" => {
            let index = index.parse::<usize>().map_err(|_| "band index must be 1-10")?;
            if !(1..=10).contains(&index) {
                return Err("band index must be 1-10".into());
            }
            let gain = gain.parse::<i32>().map_err(|_| "gain must be a whole number of dB")?;
            Ok(Action::SetBand { index: index - 1, gain: gain.clamp(-12, 12) })
        }
        [verb, preset] if verb == "preset" => {
            if !PRESETS.iter().any(|(name, _)| *name == preset) {
                return Err("unknown equalizer preset".into());
            }
            Ok(Action::Preset(preset.clone()))
        }
        [verb] if verb == "on" => Ok(Action::On(true)),
        [verb] if verb == "off" => Ok(Action::On(false)),
        [verb] if verb == "apply" => Ok(Action::Apply),
        [verb] if verb == "retarget" => Ok(Action::Retarget),
        _ => Err("usage: ryoku-eq get | set-band 1-10 -12..12 | preset NAME | on | off | apply | retarget".into()),
    }
}

fn validate_identity() -> Result<u32> {
    let uid = rustix::process::getuid().as_raw();
    let euid = rustix::process::geteuid().as_raw();
    let gid = rustix::process::getgid().as_raw();
    let egid = rustix::process::getegid().as_raw();
    if uid == 0 || uid != euid || gid != egid {
        return Err("ryoku-eq requires the matching non-root desktop identity".into());
    }
    Ok(uid)
}

fn check_dir(path: &Path, uid: u32, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let mode = metadata.mode() & 0o7777;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || (metadata.uid() != uid && metadata.uid() != 0)
        || mode & 0o022 != 0
        || (private && (metadata.uid() != uid || mode & 0o077 != 0))
    {
        return Err("equalizer directory has unsafe type, owner or permissions".into());
    }
    Ok(())
}

fn check_user_dir(path: &Path, uid: u32, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let mode = metadata.mode() & 0o7777;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || mode & 0o022 != 0
        || (private && mode & 0o077 != 0)
    {
        return Err("equalizer directory has unsafe type, owner or permissions".into());
    }
    Ok(())
}

fn ensure_dirs(path: &Path, uid: u32, final_mode: u32) -> Result<()> {
    if !path.is_absolute() || path.components().any(|part| matches!(part, Component::ParentDir)) {
        return Err("equalizer path must be absolute and traversal-free".into());
    }
    let mut current = PathBuf::from("/");
    for component in path.components() {
        let Component::Normal(name) = component else { continue };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(_) => check_dir(&current, uid, false)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                fs::set_permissions(
                    &current,
                    fs::Permissions::from_mode(if current == path { final_mode } else { 0o755 }),
                )?;
                check_dir(&current, uid, false)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    check_dir(path, uid, false)
}

fn config_directory(uid: u32) -> Result<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config"));
    if base.as_os_str().is_empty() || !base.is_absolute() {
        return Err("XDG config directory must be absolute".into());
    }
    ensure_dirs(&base, uid, 0o755)?;
    check_user_dir(&base, uid, false)?;
    let ryoku = base.join("ryoku");
    if matches!(fs::symlink_metadata(&ryoku), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        fs::create_dir(&ryoku)?;
        fs::set_permissions(&ryoku, fs::Permissions::from_mode(0o700))?;
    }
    check_user_dir(&ryoku, uid, false)?;
    Ok(ryoku)
}

fn state_path(uid: u32) -> Result<PathBuf> {
    Ok(config_directory(uid)?.join("equalizer.json"))
}

fn read_state(path: &Path, uid: u32) -> Result<State> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.len() > MAX_STATE_BYTES
        || metadata.mode() & 0o022 != 0
    {
        return Err("equalizer state file has unsafe type, owner or permissions".into());
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 != metadata.len() {
        return Err("equalizer state changed during read".into());
    }
    State::from_json(&serde_json::from_slice(&bytes)?)
}

fn write_state(path: &Path, state: &State, uid: u32) -> Result<()> {
    let parent = path.parent().ok_or("equalizer state parent is missing")?;
    check_user_dir(parent, uid, false)?;
    let mode = match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == uid
                && metadata.nlink() == 1
                && matches!(metadata.mode() & 0o777, 0o600 | 0o640 | 0o644) =>
        {
            metadata.mode() & 0o777
        }
        Ok(_) => return Err("equalizer state file has unsafe metadata".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0o600,
        Err(error) => return Err(error.into()),
    };
    let temporary = parent.join(format!(".equalizer-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(mode).open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(&serde_json::to_vec(&state.to_json())?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn config_paths(uid: u32) -> Result<(PathBuf, PathBuf)> {
    let runtime =
        PathBuf::from(env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is unavailable")?);
    if !runtime.is_absolute() {
        return Err("XDG_RUNTIME_DIR must be absolute".into());
    }
    check_dir(&runtime, uid, true)?;
    let ryoku_runtime = runtime.join("ryoku");
    let eq_runtime = ryoku_runtime.join("eq");
    for (directory, mode) in [(&ryoku_runtime, 0o700), (&eq_runtime, 0o700)] {
        if matches!(fs::symlink_metadata(directory), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(mode))?;
        }
        check_dir(directory, uid, true)?;
    }
    Ok((state_path(uid)?, eq_runtime.join("filter-chain.conf")))
}

fn bounded_command(program: &str, args: &[&str], timeout: &str) -> Result<Vec<u8>> {
    let mut command = Command::new("/usr/bin/timeout");
    command.args(["--signal=KILL", timeout, program]).args(args);
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "{} failed",
            Path::new(program).file_name().unwrap_or_default().to_string_lossy()
        )
        .into());
    }
    if output.stdout.len() > MAX_COMMAND_OUTPUT || output.stderr.len() > MAX_COMMAND_OUTPUT {
        return Err("equalizer command output exceeded its limit".into());
    }
    Ok(output.stdout)
}

fn default_sink() -> Option<String> {
    let bytes = bounded_command("/usr/bin/pactl", &["get-default-sink"], "3s").ok()?;
    let output = std::str::from_utf8(&bytes).ok()?.trim();
    (!output.is_empty() && output.bytes().all(|byte| byte.is_ascii_graphic()))
        .then(|| output.to_owned())
}

fn object_id(value: &Value, selector: impl Fn(&Value) -> bool) -> Option<u64> {
    value.as_array()?.iter().find(|item| selector(item))?.get("id")?.as_u64()
}

fn node_id() -> Result<Option<u64>> {
    let bytes = bounded_command("/usr/bin/pw-dump", &["Node"], "3s")?;
    let value: Value = serde_json::from_slice(&bytes)?;
    Ok(object_id(&value, |item| {
        item.get("type").and_then(Value::as_str).is_some_and(|kind| kind.ends_with(":Node"))
            && item.pointer("/info/props/node.name").and_then(Value::as_str) == Some(NODE_IN)
    }))
}

fn running() -> bool {
    node_id().is_ok_and(|node| node.is_some())
}

fn pipewire_string(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 255
        || !value.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
    {
        return Err("audio sink name contains unsupported characters".into());
    }
    Ok(value.to_owned())
}

fn render_config(state: &State, target: &str) -> Result<String> {
    let target = pipewire_string(target)?;
    let mut nodes = String::new();
    let mut links = String::new();
    for (index, (frequency, gain)) in FREQUENCIES.iter().zip(state.gains).enumerate() {
        nodes.push_str(&format!(
            "                    {{ type = builtin name = eq_band_{} label = bq_peaking control = {{ \"Freq\" = {frequency}.0 \"Q\" = 1.41 \"Gain\" = {gain}.0 }} }}\n",
            index + 1
        ));
        if index > 0 {
            links.push_str(&format!(
                "                    {{ output = \"eq_band_{index}:Out\" input = \"eq_band_{}:In\" }}\n",
                index + 1
            ));
        }
    }
    Ok(format!(
        "# Generated by ryoku-eq. Edited here, it lasts until the next band change.\n\
         context.properties = {{ log.level = 0 }}\n\
         context.spa-libs = {{\n    audio.convert.* = audioconvert/libspa-audioconvert\n    support.* = support/libspa-support\n}}\n\
         context.modules = [\n    {{ name = libpipewire-module-rt\n        args = {{ nice.level = -11 }}\n        flags = [ ifexists nofail ]\n    }}\n    {{ name = libpipewire-module-protocol-native }}\n    {{ name = libpipewire-module-client-node }}\n    {{ name = libpipewire-module-adapter }}\n    {{ name = libpipewire-module-filter-chain\n        args = {{\n            node.description = \"Ryoku Equalizer\"\n            media.name = \"Ryoku Equalizer\"\n            filter.graph = {{\n                nodes = [\n{nodes}                ]\n                links = [\n{links}                ]\n            }}\n            audio.channels = 2\n            audio.position = [ FL FR ]\n            capture.props = {{\n                node.name = \"{NODE_IN}\"\n                node.description = \"Ryoku Equalizer\"\n                media.class = Audio/Sink\n                node.virtual = true\n                audio.position = [ FL FR ]\n                filter.smart = true\n                filter.smart.name = \"ryoku-eq\"\n                filter.smart.target = {{ node.name = \"{target}\" }}\n            }}\n            playback.props = {{\n                node.name = \"ryoku_equalizer_out\"\n                node.description = \"Ryoku Equalizer\"\n                node.passive = true\n                node.virtual = true\n                audio.position = [ FL FR ]\n                stream.dont-remix = true\n            }}\n        }}\n    }}\n]\n"
    ))
}

fn write_filter_config(path: &Path, content: &str, uid: u32) -> Result<()> {
    let parent = path.parent().ok_or("equalizer runtime parent is missing")?;
    check_dir(parent, uid, true)?;
    let temporary = parent.join(format!(".filter-chain-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn push_gains(id: u64, state: &State) -> Result<()> {
    let params = state
        .gains
        .iter()
        .enumerate()
        .map(|(index, gain)| format!("\"eq_band_{}:Gain\" {gain}.0", index + 1))
        .collect::<Vec<_>>()
        .join(" ");
    bounded_command(
        "/usr/bin/pw-cli",
        &["s", &id.to_string(), "Props", &format!("{{ params = [{params}] }}")],
        "3s",
    )?;
    Ok(())
}

fn retarget(id: u64) -> Result<()> {
    let target = default_sink().ok_or("no default audio sink")?;
    bounded_command(
        "/usr/bin/pw-metadata",
        &[
            "-n",
            "filters",
            &id.to_string(),
            "filter.smart.target",
            &format!("{{ \"node.name\": \"{target}\" }}"),
            "Spa:String:JSON",
        ],
        "3s",
    )?;
    Ok(())
}

fn carrying(id: u64) -> Result<bool> {
    let bytes = bounded_command("/usr/bin/pw-dump", &["Link"], "3s")?;
    let value = serde_json::from_slice::<Value>(&bytes)?;
    Ok(value.as_array().is_some_and(|links| {
        links.iter().any(|link| {
            link.get("type").and_then(Value::as_str).is_some_and(|kind| kind.ends_with(":Link"))
                && link.pointer("/info/input-node-id").and_then(Value::as_u64) == Some(id)
        })
    }))
}

fn wait_for_node(
    mut discover: impl FnMut() -> Result<Option<u64>>,
    attempts: usize,
    interval: Duration,
) -> Result<u64> {
    for attempt in 0..attempts {
        if let Some(id) = discover()? {
            return Ok(id);
        }
        if attempt + 1 < attempts {
            thread::sleep(interval);
        }
    }
    Err("PipeWire equalizer filter did not become available".into())
}

fn wait_for_node_absent(
    mut discover: impl FnMut() -> Result<Option<u64>>,
    attempts: usize,
    interval: Duration,
) -> Result<()> {
    for attempt in 0..attempts {
        match discover() {
            Ok(None) => return Ok(()),
            Ok(Some(_)) => {}
            Err(error) if attempt + 1 == attempts => {
                return Err(format!("could not verify stopped equalizer filter: {error}").into());
            }
            Err(_) => {}
        }
        if attempt + 1 < attempts {
            thread::sleep(interval);
        }
    }
    Err("PipeWire equalizer filter remained active after stop".into())
}

fn start_filter(state: &State, config: &Path, uid: u32) -> Result<()> {
    let target = default_sink().ok_or("no default audio sink")?;
    write_filter_config(config, &render_config(state, &target)?, uid)?;
    let id = match node_id()? {
        Some(id) => id,
        None => {
            bounded_command("/usr/bin/systemctl", &["--user", "restart", UNIT], "8s")?;
            wait_for_node(node_id, NODE_START_ATTEMPTS, NODE_START_INTERVAL)?
        }
    };
    bounded_command(
        "/usr/bin/pw-metadata",
        &["-n", "filters", &id.to_string(), "filter.smart.disabled", "false", "Spa:String:JSON"],
        "3s",
    )?;
    retarget(id)?;
    push_gains(id, state)
}

fn stop_filter() -> Result<()> {
    if let Ok(Some(id)) = node_id() {
        let _ = bounded_command(
            "/usr/bin/pw-metadata",
            &["-n", "filters", &id.to_string(), "filter.smart.disabled", "true", "Spa:String:JSON"],
            "3s",
        );
        for _ in 0..40 {
            if !carrying(id).unwrap_or(true) {
                break;
            }
            thread::sleep(NODE_START_INTERVAL);
        }
    }
    bounded_command("/usr/bin/systemctl", &["--user", "stop", UNIT], "8s")?;
    wait_for_node_absent(node_id, NODE_START_ATTEMPTS, NODE_START_INTERVAL)
}

fn commit(path: &Path, config: &Path, state: &State, uid: u32) -> Result<()> {
    write_state(path, state, uid)?;
    if state.on { start_filter(state, config, uid) } else { stop_filter() }
}

fn output_state(state: &State) -> Result<()> {
    let running = running();
    let target = default_sink().unwrap_or_default();
    let mut output = state.to_json();
    let object = output.as_object_mut().ok_or("invalid equalizer state")?;
    object.insert("running".into(), json!(running));
    object.insert("target".into(), json!(target));
    println!("{}", output);
    Ok(())
}

fn execute(args: &[String]) -> Result<()> {
    let uid = validate_identity()?;
    let action = parse_args(args)?;
    let path = state_path(uid)?;
    let state = read_state(&path, uid)?;
    if action == Action::Get {
        return output_state(&state);
    }
    let (state_path, config_path) = config_paths(uid)?;
    let mut next = state;
    match action {
        Action::Get => unreachable!(),
        Action::SetBand { index, gain } => {
            next.gains[index] = gain;
            next.preset = "Custom".into();
            next.on = true;
        }
        Action::Preset(name) => {
            let (_, gains) = PRESETS
                .iter()
                .find(|(preset, _)| *preset == name)
                .ok_or("unknown equalizer preset")?;
            next.gains = *gains;
            next.preset = name;
            next.on = next.preset != "Flat";
        }
        Action::On(enabled) => next.on = enabled,
        Action::Apply => {}
        Action::Retarget if !next.on => return Ok(()),
        Action::Retarget => {
            if let Some(id) = node_id()? {
                retarget(id)?;
                return Ok(());
            }
        }
    }
    commit(&state_path, &config_path, &next, uid)
}

fn main() -> ExitCode {
    match execute(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ryoku-eq: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_start_wait_succeeds_only_after_the_pipewire_node_appears() {
        let mut probes = 0;
        let node = wait_for_node(
            || {
                probes += 1;
                Ok((probes == 3).then_some(42))
            },
            4,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(node, 42);
        assert_eq!(probes, 3);
    }

    #[test]
    fn filter_stop_waits_for_node_removal_and_reports_unverifiable_state() {
        let mut probes = 0;
        wait_for_node_absent(
            || {
                probes += 1;
                Ok((probes < 3).then_some(42))
            },
            4,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(probes, 3);

        let error = wait_for_node_absent(|| Ok(Some(42)), 2, Duration::ZERO).unwrap_err();
        assert_eq!(error.to_string(), "PipeWire equalizer filter remained active after stop");
        let error =
            wait_for_node_absent(|| Err("pw-dump failed".into()), 1, Duration::ZERO).unwrap_err();
        assert!(error.to_string().contains("could not verify stopped equalizer filter"));
    }

    #[test]
    fn filter_start_reports_missing_pipewire_node_instead_of_success() {
        let error = wait_for_node(|| Ok(None), 3, Duration::ZERO).unwrap_err();
        assert_eq!(error.to_string(), "PipeWire equalizer filter did not become available");
    }
}
