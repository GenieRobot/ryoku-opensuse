//! Reproduce runtime inputs from immutable Git objects and pinned font bytes.
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

#[path = "../source_link_policy.rs"]
mod source_link_policy;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Debug)]
struct Entry {
    path: String,
    mode: u32,
    oid: String,
}

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.bytes().any(|byte| byte < 32 || byte == 127)
        && !path.split('/').any(|part| part.is_empty() || matches!(part, "." | ".."))
        && Path::new(path).components().all(|part| matches!(part, Component::Normal(_)))
}

fn oid(value: &str) -> bool {
    value.len() == 40
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn text<'a>(table: &'a toml::Value, key: &str) -> Result<&'a str> {
    table.get(key).and_then(toml::Value::as_str).ok_or_else(|| format!("missing {key}").into())
}

fn output(command: &mut Command) -> Result<Vec<u8>> {
    let result = command.output()?;
    if !result.status.success() {
        return Err(format!("command failed: {}", String::from_utf8_lossy(&result.stderr)).into());
    }
    Ok(result.stdout)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    output(Command::new("git").arg("-C").arg(root).args(args))
}

fn entries(root: &Path, revision: &str) -> Result<Vec<Entry>> {
    let bytes = git(root, &["ls-tree", "-r", "-z", revision])?;
    let mut result = Vec::new();
    for record in bytes.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
        let record = std::str::from_utf8(record)?;
        let (header, path) = record.split_once('\t').ok_or("invalid Git tree record")?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 3 || !safe_path(path) || !oid(fields[2]) {
            return Err("invalid Git tree entry".into());
        }
        let mode = match (fields[0], fields[1]) {
            ("100644", "blob") => 0o644,
            ("100755", "blob") => 0o755,
            ("120000", "blob") => 0o120000,
            // Keep unsupported entries so selected submodules fail explicitly.
            _ => 0,
        };
        result.push(Entry { path: path.into(), mode, oid: fields[2].into() });
    }
    Ok(result)
}

struct Blobs {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl Blobs {
    fn new(root: &Path) -> Result<Self> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        Ok(Self {
            input: Some(child.stdin.take().ok_or("missing Git stdin")?),
            output: BufReader::new(child.stdout.take().ok_or("missing Git stdout")?),
            child,
        })
    }
    fn read(&mut self, expected: &str) -> Result<Vec<u8>> {
        let input = self.input.as_mut().ok_or("closed Git stdin")?;
        writeln!(input, "{expected}")?;
        input.flush()?;
        let mut header = String::new();
        self.output.read_line(&mut header)?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 || fields[0] != expected || fields[1] != "blob" {
            return Err("Git returned a different source object".into());
        }
        let size: usize = fields[2].parse()?;
        if size > 256 * 1024 * 1024 {
            return Err("source object exceeds export bound".into());
        }
        let mut bytes = vec![0; size];
        self.output.read_exact(&mut bytes)?;
        let mut newline = [0];
        self.output.read_exact(&mut newline)?;
        if newline != *b"\n" {
            return Err("invalid Git object terminator".into());
        }
        Ok(bytes)
    }
    fn finish(mut self) -> Result<()> {
        self.input.take();
        if !self.child.wait()?.success() {
            return Err("Git object reader failed".into());
        }
        Ok(())
    }
}
impl Drop for Blobs {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn selected(path: &str, prefix: &str) -> Option<String> {
    if path == prefix {
        Some(String::new())
    } else {
        path.strip_prefix(prefix).and_then(|tail| tail.strip_prefix('/')).map(str::to_owned)
    }
}

fn export_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if !safe_path(relative) {
        return Err("unsafe export destination".into());
    }
    let path = root.join(relative);
    fs::create_dir_all(path.parent().ok_or("missing export parent")?)?;
    let mut parent = path.parent();
    while let Some(directory) = parent {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755))?;
        if directory == root {
            break;
        }
        parent = directory.parent();
    }
    Ok(path)
}

fn put(root: &Path, relative: &str, bytes: &[u8], mode: u32) -> Result<()> {
    if !matches!(mode, 0o644 | 0o755) {
        return Err("unsafe source mode".into());
    }
    let path = export_path(root, relative)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn files(root: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, at: &Path, result: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(at)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(root, &entry.path(), result)?;
            } else if kind.is_file() {
                let path =
                    entry.path().strip_prefix(root)?.to_str().ok_or("non-UTF8 path")?.to_owned();
                if !safe_path(&path) {
                    return Err("unsafe source path".into());
                }
                result.push(path);
            } else {
                return Err("export contains non-regular source".into());
            }
        }
        Ok(())
    }
    let mut result = Vec::new();
    walk(root, root, &mut result)?;
    result.sort();
    Ok(result)
}

fn review(
    entry: &Entry,
    bytes: &[u8],
    targets: &[String],
    writer_source: bool,
) -> Result<Option<serde_json::Value>> {
    if entry.path.contains("/vendor/")
        || entry.path.ends_with("_test.go")
        || entry.path.contains("/tests/")
    {
        return Ok(None);
    }
    let extension = Path::new(&entry.path).extension().and_then(|value| value.to_str());
    if !matches!(
        extension,
        Some(
            "go" | "qml"
                | "js"
                | "lua"
                | "c"
                | "cpp"
                | "h"
                | "hpp"
                | "sh"
                | "py"
                | "rs"
                | "service"
        )
    ) && !bytes.starts_with(b"#!")
    {
        return Ok(None);
    }
    let contents = std::str::from_utf8(bytes)?;
    let mut sites = Vec::new();
    let mut imports = BTreeSet::new();
    for (index, line) in contents.lines().enumerate() {
        if extension == Some("qml")
            && let Some(import) = line.trim().strip_prefix("import ")
        {
            imports.insert(import.split_whitespace().next().ok_or("empty QML import")?.to_owned());
        }
        let write = [
            "os.WriteFile(",
            "os.Create(",
            "os.CreateTemp(",
            "os.Mkdir",
            "os.Rename(",
            "os.Remove",
            "os.OpenFile(",
            "atomicWrite(",
            "atomicJSON(",
            "saveRaw(",
            "FileView",
            ".setText(",
            "writeFile(",
            "writeAtomic(",
            "QFile",
            "QSaveFile",
            "std::ofstream",
            "write_text(",
            "write_bytes(",
            "shutil.",
        ]
        .iter()
        .any(|pattern| line.contains(pattern));
        let process = [
            "exec.Command(",
            "exec.CommandContext(",
            "Process {",
            "Quickshell.execDetached(",
            "startDetached(",
            "DBus",
            "login1",
            "pkexec",
            "sudo",
            "systemctl",
            "pacman",
            "dnf",
            "xbps",
            "/dev/",
            "/sys/",
            "subprocess.",
        ]
        .iter()
        .any(|pattern| line.contains(pattern));
        if write || process {
            sites.push(serde_json::json!({"line": index + 1, "write_candidate": write, "process_or_host_candidate": process, "text": line}));
        }
    }
    Ok(Some(
        serde_json::json!({"source": entry.path, "git_blob": entry.oid, "sha256": format!("{:x}", Sha256::digest(bytes)), "runtime_exports": targets, "writer_source": writer_source, "qml_imports": imports, "sites": sites}),
    ))
}

fn archive(root: &Path, target: &Path, epoch: u64, compression: &str) -> Result<()> {
    let tar = target.with_extension("tar");
    let mut command = Command::new("tar");
    command
        .env_remove("TAR_OPTIONS")
        .args(["--sort=name", "--format=gnu", "--owner=0", "--group=0", "--numeric-owner"])
        .arg(format!("--mtime=@{epoch}"))
        .arg("-cf")
        .arg(&tar)
        .arg("-C")
        .arg(root)
        .arg(".");
    output(&mut command)?;
    let destination = OpenOptions::new().write(true).create_new(true).open(target)?;
    let mut command = Command::new(compression);
    command.env_remove("GZIP").env_remove("XZ_OPT").env_remove("XZ_DEFAULTS");
    command.args(if compression == "gzip" { &["-n", "-9", "-c"][..] } else { &["-6", "-c"][..] });
    command.arg(&tar).stdout(Stdio::from(destination));
    output(&mut command)?;
    fs::remove_file(tar)?;
    Ok(())
}

fn run(
    source: &Path,
    manifest_path: &Path,
    scope_path: &Path,
    fonts: &Path,
    target: &Path,
) -> Result<()> {
    let manifest = toml::Value::Table(fs::read_to_string(manifest_path)?.parse()?);
    let revision = text(&manifest, "source_revision")?;
    let tree = text(&manifest, "source_tree")?;
    if !oid(revision) || !oid(tree) {
        return Err("source commit/tree must be complete lowercase object ids".into());
    }
    let actual_tree =
        String::from_utf8(git(source, &["rev-parse", &format!("{revision}^{{tree}}")])?)?;
    if actual_tree.trim() != tree {
        return Err("source commit/tree mismatch".into());
    }
    let scopes: Vec<_> = fs::read_to_string(scope_path)?.lines().map(str::to_owned).collect();
    if scopes.is_empty() || scopes.iter().any(|path| !safe_path(path)) {
        return Err("invalid writer source scope".into());
    }
    let exports =
        manifest.get("source_exports").and_then(toml::Value::as_array).ok_or("missing exports")?;
    let mut mappings = Vec::new();
    for mapping in exports {
        let origin = text(mapping, "source")?;
        let payload = text(mapping, "payload")?;
        if !safe_path(origin) || !safe_path(payload) {
            return Err("unsafe source mapping".into());
        }
        mappings.push((origin.to_owned(), payload.to_owned()));
    }
    let entries = entries(source, revision)?;
    let selected_modes: BTreeMap<_, _> = entries
        .iter()
        .filter(|entry| scopes.iter().any(|scope| selected(&entry.path, scope).is_some()))
        .map(|entry| (entry.path.clone(), entry.mode))
        .collect();
    if scopes
        .iter()
        .any(|scope| !entries.iter().any(|entry| selected(&entry.path, scope).is_some()))
        || mappings
            .iter()
            .any(|(origin, _)| !entries.iter().any(|entry| selected(&entry.path, origin).is_some()))
    {
        return Err("declared scope missing from source tree".into());
    }
    fs::create_dir(target)?;
    fs::set_permissions(target, fs::Permissions::from_mode(0o755))?;
    let runtime = target.join("runtime");
    let writers = target.join("writers");
    fs::create_dir(&runtime)?;
    fs::create_dir(&writers)?;
    let mut blobs = Blobs::new(source)?;
    let mut payload_paths = BTreeSet::new();
    let mut provenance = String::from("payload\tsource\tgit_blob\tmode\n");
    let mut writer_provenance = String::from("source\tgit_blob\tmode\n");
    let mut source_count = 0;
    let mut review_records = Vec::new();
    let mut source_links = Vec::new();
    for entry in &entries {
        let write = scopes.iter().any(|scope| selected(&entry.path, scope).is_some());
        let targets: Vec<_> = mappings
            .iter()
            .filter_map(|(origin, payload)| {
                selected(&entry.path, origin).map(|tail| {
                    if tail.is_empty() { payload.clone() } else { format!("{payload}/{tail}") }
                })
            })
            .collect();
        if !write && targets.is_empty() {
            continue;
        }
        if entry.mode == 0 {
            return Err(format!("selected source is a symlink/submodule: {}", entry.path).into());
        }
        let bytes = blobs.read(&entry.oid)?;
        if entry.mode == 0o120000 {
            if !targets.is_empty() {
                return Err(format!(
                    "selected runtime source is a symlink/submodule: {}",
                    entry.path
                )
                .into());
            }
            let resolved =
                source_link_policy::resolve_source_link(&entry.path, &bytes, &selected_modes)?;
            let target =
                entries.iter().find(|entry| entry.path == resolved).ok_or("missing link target")?;
            let literal = std::str::from_utf8(&bytes)?;
            std::os::unix::fs::symlink(literal, export_path(&writers, &entry.path)?)?;
            writer_provenance.push_str(&format!("{}\t{}\t120000\n", entry.path, entry.oid));
            source_links.push(serde_json::json!({
                "source": entry.path, "git_blob": entry.oid, "git_mode": "120000",
                "link_text": literal, "link_object_sha256": format!("{:x}", Sha256::digest(&bytes)),
                "resolved_source": resolved, "resolved_git_blob": target.oid,
                "resolved_mode": format!("{:o}", target.mode),
            }));
            source_count += 1;
            continue;
        }
        if let Some(record) = review(entry, &bytes, &targets, write)? {
            review_records.push(record);
        }
        if write {
            put(&writers, &entry.path, &bytes, entry.mode)?;
            writer_provenance
                .push_str(&format!("{}\t{}\t{:o}\n", entry.path, entry.oid, entry.mode));
            source_count += 1;
        }
        for path in targets {
            if !payload_paths.insert(path.clone()) {
                return Err(format!("duplicate payload destination: {path}").into());
            }
            put(&runtime, &path, &bytes, entry.mode)?;
            provenance
                .push_str(&format!("{path}\t{}\t{}\t{:o}\n", entry.path, entry.oid, entry.mode));
        }
    }
    blobs.finish()?;
    let assets =
        manifest.get("font_assets").and_then(toml::Value::as_array).ok_or("missing fonts")?;
    for font in assets {
        let path = text(font, "payload")?;
        let expected = text(font, "sha256")?;
        if !safe_path(path) || !path.starts_with("fonts/") || !payload_paths.insert(path.into()) {
            return Err("unsafe or duplicate font path".into());
        }
        let original = fonts.join(path);
        if !fs::symlink_metadata(&original)?.is_file() || sha256(&original)? != expected {
            return Err(format!("font identity mismatch: {path}").into());
        }
        put(&runtime, path, &fs::read(&original)?, 0o644)?;
        provenance.push_str(&format!("{path}\t{}\tsha256:{expected}\t644\n", text(font, "url")?));
    }
    let mut embedded = toml::Table::new();
    for key in [
        "source_repository",
        "source_revision",
        "source_tree",
        "source_release",
        "source_exports",
        "font_assets",
    ] {
        embedded
            .insert(key.into(), manifest.get(key).ok_or_else(|| format!("missing {key}"))?.clone());
    }
    put(&runtime, "RUNTIME-MANIFEST.toml", toml::to_string(&embedded)?.as_bytes(), 0o644)?;
    put(&runtime, "RUNTIME.md", b"Pinned Ryoku runtime source. RUNTIME-MANIFEST.toml records the immutable upstream tree and source mappings. Font bytes are independently SHA-256 pinned. SHA256SUMS covers every regular file in this input; this archive is source provenance, not runtime acceptance.\n", 0o644)?;
    let families =
        assets.iter().map(|font| text(font, "payload")).collect::<Result<Vec<_>>>()?.join("\n")
            + "\n";
    put(&runtime, "FONT-FAMILIES.txt", families.as_bytes(), 0o644)?;
    let mut sums = String::new();
    for path in files(&runtime)? {
        sums.push_str(&format!("{}  ./{}\n", sha256(&runtime.join(&path))?, path));
    }
    put(&runtime, "SHA256SUMS", sums.as_bytes(), 0o644)?;
    let epoch = manifest
        .get("coordinated_writers")
        .and_then(|value| value.get("source_commit_epoch"))
        .and_then(toml::Value::as_integer)
        .ok_or("missing source epoch")?;
    let epoch = u64::try_from(epoch)?;
    let runtime_archive = target.join(text(&manifest, "source_archive")?);
    let writers_archive = target.join(text(
        manifest.get("coordinated_writers").ok_or("missing writers")?,
        "source_archive",
    )?);
    for path in [&runtime_archive, &writers_archive] {
        if path.parent() != Some(target) {
            return Err("unsafe archive name".into());
        }
    }
    archive(&runtime, &runtime_archive, 0, "gzip")?;
    archive(&writers, &writers_archive, epoch, "xz")?;
    fs::write(target.join("runtime-source-manifest.tsv"), provenance)?;
    fs::write(target.join("writer-source-manifest.tsv"), writer_provenance)?;
    fs::write(
        target.join("writer-source-links.json"),
        serde_json::to_string_pretty(&source_links)? + "\n",
    )?;
    fs::write(
        target.join("source-review.json"),
        serde_json::to_string_pretty(&review_records)? + "\n",
    )?;
    let report = serde_json::json!({
        "source_revision": revision,
        "source_tree": tree,
        "runtime_archive": runtime_archive.file_name().and_then(|name| name.to_str()).ok_or("invalid archive name")?,
        "runtime_sha256": sha256(&runtime_archive)?,
        "runtime_bytes": fs::metadata(&runtime_archive)?.len(),
        "runtime_files": files(&runtime)?.len(),
        "writer_archive": writers_archive.file_name().and_then(|name| name.to_str()).ok_or("invalid archive name")?,
        "writer_sha256": sha256(&writers_archive)?,
        "writer_bytes": fs::metadata(&writers_archive)?.len(),
        "writer_source_files": source_count,
        "writer_source_links": source_links.len(),
        "runtime_tar_epoch": 0,
        "writer_tar_epoch": epoch,
        "gzip": "-n -9",
        "xz": "-6",
        "reviewed_source_files": review_records.len(),
        "review_note": "Complete selected non-vendor native source text scan; candidate write/process sites require the documented semantic disposition, and are not all configuration writes.",
    });
    let report = serde_json::to_string_pretty(&report)? + "\n";
    fs::write(target.join("export.json"), &report)?;
    print!("{report}");
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let result = match args.as_slice() {
        [source, manifest, scope, fonts, target] => run(source, manifest, scope, fonts, target),
        _ => Err("usage: upstream-runtime-export UPSTREAM_GIT RUNTIME_TOML WRITER_SCOPE FONT_ROOT FRESH_OUTPUT".into()),
    };
    if let Err(error) = result {
        eprintln!("runtime export failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let root = std::env::temp_dir()
                .join(format!("ryoku-runtime-export-{}-{unique}", std::process::id()));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("source")).unwrap();
            fs::create_dir(root.join("fonts")).unwrap();
            fs::create_dir(root.join("fonts/fonts")).unwrap();
            fs::write(root.join("fonts/fonts/license.txt"), b"font license\n").unwrap();
            let source = root.join("source");
            output(Command::new("git").arg("init").arg("-q").arg(&source)).unwrap();
            fs::create_dir(source.join("src")).unwrap();
            fs::write(source.join("src/main.qml"), b"import QtQuick\nItem {}\n").unwrap();
            fs::write(source.join("src/helper"), b"upstream executable\n").unwrap();
            fs::set_permissions(source.join("src/helper"), fs::Permissions::from_mode(0o755))
                .unwrap();
            fs::write(source.join("unselected"), b"must not be exported\n").unwrap();
            fs::write(root.join("scope"), "src\n").unwrap();
            Self(root)
        }
        fn commit(&self) {
            let source = self.0.join("source");
            git(&source, &["add", "."]).unwrap();
            git(
                &source,
                &[
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-qm",
                    "fixture",
                ],
            )
            .unwrap();
            let revision =
                String::from_utf8(git(&source, &["rev-parse", "HEAD"]).unwrap()).unwrap();
            let tree =
                String::from_utf8(git(&source, &["rev-parse", "HEAD^{tree}"]).unwrap()).unwrap();
            let font = sha256(&self.0.join("fonts/fonts/license.txt")).unwrap();
            fs::write(self.0.join("manifest"), format!(
                "source_repository = \"fixture\"\nsource_revision = \"{}\"\nsource_tree = \"{}\"\nsource_release = \"fixture\"\nsource_archive = \"runtime.tar.gz\"\n[[source_exports]]\nsource = \"src\"\npayload = \"shell\"\n[[font_assets]]\npayload = \"fonts/license.txt\"\nurl = \"https://example.invalid/font\"\nsha256 = \"{font}\"\n[coordinated_writers]\nsource_archive = \"writers.tar.xz\"\nsource_commit_epoch = 123\n",
                revision.trim(), tree.trim()
            )).unwrap();
        }
        fn export(&self, output: &str) -> Result<()> {
            run(
                &self.0.join("source"),
                &self.0.join("manifest"),
                &self.0.join("scope"),
                &self.0.join("fonts"),
                &self.0.join(output),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exports_exact_objects_modes_and_reproducible_archives_without_worktree_bytes() {
        let fixture = Fixture::new();
        fixture.commit();
        // A dirty checkout must not influence an immutable object export.
        fs::write(fixture.0.join("source/src/main.qml"), b"changed after commit\n").unwrap();
        fixture.export("first").unwrap();
        fixture.export("second").unwrap();
        for archive in ["runtime.tar.gz", "writers.tar.xz"] {
            assert_eq!(
                sha256(&fixture.0.join("first").join(archive)).unwrap(),
                sha256(&fixture.0.join("second").join(archive)).unwrap()
            );
        }
        assert_eq!(
            fs::read(fixture.0.join("first/runtime/shell/main.qml")).unwrap(),
            b"import QtQuick\nItem {}\n"
        );
        assert_eq!(
            fs::metadata(fixture.0.join("first/runtime/shell/helper"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(!fixture.0.join("first/writers/unselected").exists());
        let verification = Command::new("sha256sum")
            .args(["--check", "--status", "SHA256SUMS"])
            .current_dir(fixture.0.join("first/runtime"))
            .status()
            .unwrap();
        assert!(verification.success());
        assert!(fixture.export("first").is_err());
    }

    #[test]
    fn refuses_selected_links_and_changed_font_bytes() {
        let fixture = Fixture::new();
        std::os::unix::fs::symlink("../unselected", fixture.0.join("source/src/link")).unwrap();
        fixture.commit();
        assert!(fixture.export("link").unwrap_err().to_string().contains("symlink/submodule"));
        fs::remove_file(fixture.0.join("source/src/link")).unwrap();
        fixture.commit();
        fs::write(fixture.0.join("fonts/fonts/license.txt"), b"different font bytes").unwrap();
        assert!(fixture.export("font").unwrap_err().to_string().contains("font identity mismatch"));
    }

    #[test]
    fn source_only_icon_links_preserve_literal_tar_metadata_and_repeat_hashes() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let links = [
            ("ryoku/rashin/app/logo.svg", "../../assets/brand/rashin-mark.svg"),
            ("ryoku/rashin/web/public/favicon.svg", "../../../assets/brand/rashin-mark-small.svg"),
        ];
        fs::create_dir_all(source.join("ryoku/assets/brand")).unwrap();
        for name in ["rashin-mark.svg", "rashin-mark-small.svg"] {
            fs::write(source.join("ryoku/assets/brand").join(name), b"<svg/>\n").unwrap();
        }
        for (path, target) in links {
            fs::create_dir_all(source.join(path).parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, source.join(path)).unwrap();
        }
        fs::write(fixture.0.join("scope"), "src\nryoku/rashin\nryoku/assets/brand\n").unwrap();
        fixture.commit();
        // The committed link object, never a dirty host link, supplies the bytes.
        for (path, _) in links {
            fs::remove_file(source.join(path)).unwrap();
            std::os::unix::fs::symlink("/host-path-must-not-be-read", source.join(path)).unwrap();
        }
        fixture.export("first").unwrap();
        fixture.export("second").unwrap();
        for archive in ["runtime.tar.gz", "writers.tar.xz"] {
            assert_eq!(
                sha256(&fixture.0.join("first").join(archive)).unwrap(),
                sha256(&fixture.0.join("second").join(archive)).unwrap()
            );
        }
        let listing = String::from_utf8(
            output(Command::new("tar").args(["-tvf"]).arg(fixture.0.join("first/writers.tar.xz")))
                .unwrap(),
        )
        .unwrap();
        let manifest =
            fs::read_to_string(fixture.0.join("first/writer-source-manifest.tsv")).unwrap();
        let evidence: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(fixture.0.join("first/writer-source-links.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(evidence.as_array().unwrap().len(), 2);
        for (path, target) in links {
            let exported = fixture.0.join("first/writers").join(path);
            assert!(fs::symlink_metadata(&exported).unwrap().file_type().is_symlink());
            assert_eq!(fs::read_link(exported).unwrap(), Path::new(target));
            assert!(
                manifest.lines().any(
                    |line| line.starts_with(&format!("{path}\t")) && line.ends_with("\t120000")
                )
            );
            assert!(listing.lines().any(
                |line| line.starts_with('l') && line.contains(&format!("./{path} -> {target}"))
            ));
        }
        assert!(!fixture.0.join("first/runtime/ryoku/rashin").exists());
    }

    #[test]
    fn refuses_source_only_links_to_unselected_escaping_and_link_targets() {
        for target in ["../unselected", "../../outside", "/etc/passwd", "other"] {
            let fixture = Fixture::new();
            let retained = fixture.0.join("source/retained");
            fs::create_dir(&retained).unwrap();
            std::os::unix::fs::symlink(target, retained.join("icon")).unwrap();
            std::os::unix::fs::symlink("../src/main.qml", retained.join("other")).unwrap();
            fs::write(fixture.0.join("scope"), "src\nretained\n").unwrap();
            fixture.commit();
            assert!(fixture.export("rejected").is_err(), "{target}");
            assert!(!fixture.0.join("rejected/writers.tar.xz").exists());
        }
    }

    #[test]
    fn pinned_rashin_link_objects_resolve_to_current_selected_brand_blobs() {
        let evidence: serde_json::Value = serde_json::from_str(include_str!(
            "../../../upstream/evidence/ryoku-v1.0/rashin-source-links.json"
        ))
        .unwrap();
        let rows: BTreeMap<_, _> =
            include_str!("../../../upstream/evidence/ryoku-v1.0/writer-source-manifest.tsv")
                .lines()
                .skip(1)
                .map(|line| {
                    let fields: Vec<_> = line.split('\t').collect();
                    (fields[0].to_owned(), (fields[1], u32::from_str_radix(fields[2], 8).unwrap()))
                })
                .collect();
        let mut selected_modes: BTreeMap<_, _> =
            rows.iter().map(|(path, (_, mode))| (path.clone(), *mode)).collect();
        let links = evidence["links"].as_array().unwrap();
        assert_eq!(links.len(), 2);
        for link in links {
            let source = link["source"].as_str().unwrap();
            selected_modes.insert(source.into(), 0o120000);
        }
        for link in links {
            let source = link["source"].as_str().unwrap();
            let resolved = source_link_policy::resolve_source_link(
                source,
                link["link_text"].as_str().unwrap().as_bytes(),
                &selected_modes,
            )
            .unwrap();
            assert_eq!(resolved, link["resolved_source"].as_str().unwrap());
            assert_eq!(rows[&resolved].0, link["resolved_git_blob"].as_str().unwrap());
        }
        let scopes = include_str!("../../../packaging/rpm/upstream-writer-paths.txt");
        for root in ["ryoku/cli", "ryoku/rashin"] {
            assert!(scopes.lines().any(|line| line == root));
        }
    }
}
