//! Verify a selected upstream export against its immutable Git tree.
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
    process::{Command, Stdio},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone)]
struct Entry {
    path: String,
    mode: String,
    sha: String,
    kind: String,
}

fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && !value.bytes().any(|byte| byte < 32 || byte == 127)
        && Path::new(value).components().all(|part| matches!(part, Component::Normal(_)))
        && !value.split('/').any(|part| part.is_empty() || matches!(part, "." | ".."))
}

fn selected(path: &str, scope: &[String]) -> bool {
    scope.iter().any(|prefix| {
        path == prefix || path.strip_prefix(prefix).is_some_and(|tail| tail.starts_with('/'))
    })
}

fn hex(value: &str) -> Result<Vec<u8>> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid Git object id".into());
    }
    (0..40).step_by(2).map(|index| Ok(u8::from_str_radix(&value[index..index + 2], 16)?)).collect()
}

fn hash(kind: &str, bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("git")
        // Hash raw Git bytes without looking for referenced objects in this
        // unrelated checkout's object database. No objects are written.
        .args(["hash-object", "--literally", "-t", kind, "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child.stdin.take().ok_or("missing hash input")?.write_all(bytes)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("Git object hashing failed".into());
    }
    let result = String::from_utf8(output.stdout)?.trim().to_owned();
    hex(&result)?;
    Ok(result)
}

fn tree_bytes(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut entries = entries.to_vec();
    entries.sort_by_key(|entry| {
        let name = entry.path.rsplit('/').next().unwrap_or("");
        format!("{name}{}", if entry.kind == "tree" { "/" } else { "" })
    });
    let mut result = Vec::new();
    for entry in entries {
        let name = entry.path.rsplit('/').next().ok_or("missing tree entry name")?;
        result.extend_from_slice(entry.mode.trim_start_matches('0').as_bytes());
        result.push(b' ');
        result.extend_from_slice(name.as_bytes());
        result.push(0);
        result.extend_from_slice(&hex(&entry.sha)?);
    }
    Ok(result)
}

fn walk(root: &Path, relative: &str, files: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| "non-UTF8 export path")?;
        let path = if relative.is_empty() { name } else { format!("{relative}/{name}") };
        if !safe_path(&path) {
            return Err("unsafe export path".into());
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(root, &path, files)?;
        } else if kind.is_file() {
            files.insert(path);
        } else {
            return Err("source export contains a symlink or special file".into());
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err(
            "usage: upstream-source-audit TREE_JSON EXPECTED_TREE EXPORT_ROOT SCOPE_FILE".into()
        );
    }
    let manifest: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let expected = &args[2];
    hex(expected)?;
    if manifest["sha"].as_str() != Some(expected) || manifest["truncated"].as_bool() != Some(false)
    {
        return Err("wrong or truncated Git tree".into());
    }
    let scope: Vec<_> = fs::read_to_string(&args[4])?.lines().map(str::to_owned).collect();
    if scope.is_empty() || !scope.iter().all(|path| safe_path(path)) {
        return Err("invalid source scope".into());
    }
    let mut entries = BTreeMap::new();
    let mut directories: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    directories.insert(String::new(), Vec::new());
    for value in manifest["tree"].as_array().ok_or("missing Git tree entries")? {
        let entry = Entry {
            path: value["path"].as_str().ok_or("missing path")?.to_owned(),
            mode: value["mode"].as_str().ok_or("missing mode")?.to_owned(),
            sha: value["sha"].as_str().ok_or("missing sha")?.to_owned(),
            kind: value["type"].as_str().ok_or("missing type")?.to_owned(),
        };
        if !safe_path(&entry.path)
            || !matches!(
                (entry.mode.as_str(), entry.kind.as_str()),
                ("040000", "tree")
                    | ("100644" | "100755" | "120000", "blob")
                    | ("160000", "commit")
            )
        {
            return Err("unsafe or unsupported Git entry".into());
        }
        hex(&entry.sha)?;
        if entry.kind == "commit" && selected(&entry.path, &scope) {
            return Err("selected source contains an unexported submodule".into());
        }
        if entries.insert(entry.path.clone(), entry.clone()).is_some() {
            return Err("duplicate Git path".into());
        }
        let parent = entry.path.rsplit_once('/').map_or("", |(parent, _)| parent);
        directories.entry(parent.to_owned()).or_default().push(entry.clone());
        if entry.kind == "tree" {
            directories.entry(entry.path.clone()).or_default();
        }
    }
    for (directory, children) in &directories {
        let expected = if directory.is_empty() {
            expected
        } else {
            let entry = entries.get(directory).ok_or("missing parent tree")?;
            if entry.kind != "tree" {
                return Err("parent entry is not a tree".into());
            }
            &entry.sha
        };
        if hash("tree", &tree_bytes(children)?)? != *expected {
            return Err(format!("Git tree identity mismatch: {directory}").into());
        }
    }
    let expected_files: BTreeSet<_> = entries
        .values()
        .filter(|entry| entry.kind == "blob" && selected(&entry.path, &scope))
        .map(|entry| entry.path.clone())
        .collect();
    if expected_files.is_empty() || scope.iter().any(|prefix| !entries.contains_key(prefix)) {
        return Err("source scope missing from pinned tree".into());
    }
    let root = Path::new(&args[3]);
    let mut actual = BTreeSet::new();
    walk(root, "", &mut actual)?;
    if actual != expected_files {
        return Err("export has missing or unexpected source files".into());
    }
    for path in &actual {
        let entry = entries.get(path).ok_or("source path missing from tree")?;
        let metadata = fs::symlink_metadata(root.join(path))?;
        let mode = if entry.mode == "100755" { 0o755 } else { 0o644 };
        if !metadata.is_file() || metadata.permissions().mode() & 0o7777 != mode {
            return Err(format!("source mode mismatch: {path}").into());
        }
        if hash("blob", &fs::read(root.join(path))?)? != entry.sha {
            return Err(format!("source blob identity mismatch: {path}").into());
        }
    }
    println!(
        "git_tree={expected}\nverified_tree_objects={}\nverified_source_files={}\nsource_identity=passed",
        directories.len(),
        actual.len()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("source audit failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_does_not_match_a_sibling_prefix() {
        let scope = vec!["ryoku/hub".to_owned()];
        assert!(selected("ryoku/hub/backend/main.go", &scope));
        assert!(!selected("ryoku/hub-other/main.go", &scope));
    }

    #[test]
    fn rejects_escaping_ambiguous_and_control_paths() {
        for path in ["", "/absolute", "a/../b", "a/./b", "a//b", "a\nb", "a\0b"] {
            assert!(!safe_path(path), "{path:?}");
        }
    }

    #[test]
    fn tree_order_uses_directory_slash_and_binary_object_ids() {
        let entries = vec![
            Entry {
                path: "foo".into(),
                mode: "040000".into(),
                sha: "22".repeat(20),
                kind: "tree".into(),
            },
            Entry {
                path: "foo.c".into(),
                mode: "100644".into(),
                sha: "11".repeat(20),
                kind: "blob".into(),
            },
        ];
        let result = tree_bytes(&entries).unwrap();
        assert!(result.starts_with(b"100644 foo.c\0"));
        assert_eq!(&result[b"100644 foo.c\0".len()..b"100644 foo.c\0".len() + 20], &[0x11; 20]);
        assert!(result.windows(b"40000 foo\0".len()).any(|part| part == b"40000 foo\0"));
        assert!(hex("invalid").is_err());
    }
}
