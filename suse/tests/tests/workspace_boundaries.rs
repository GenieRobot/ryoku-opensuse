use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const MEMBERS: [&str; 6] =
    ["suse/adapters", "suse/recovery", "suse/ui", "suse/profiles", "suse/tests", "automation"];

#[test]
fn workspace_members_have_rust_only_source_boundaries() -> Result<(), Box<dyn Error>> {
    let tests_package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = tests_package
        .parent()
        .and_then(Path::parent)
        .ok_or("test package is not under the repository workspace")?;
    let workspace_manifest = fs::read_to_string(root.join("Cargo.toml"))?;

    for member in MEMBERS {
        assert!(
            workspace_manifest.contains(&format!("\"{member}\"")),
            "workspace does not declare member {member}"
        );
        let package_root = root.join(member);
        assert!(
            package_root.join("Cargo.toml").is_file(),
            "workspace member {member} has no Cargo manifest"
        );
        assert_rust_sources(&package_root.join("src"))?;
    }

    Ok(())
}

fn assert_rust_sources(directory: &Path) -> Result<(), Box<dyn Error>> {
    assert!(directory.is_dir(), "missing Rust source directory: {directory:?}");
    let mut pending = vec![PathBuf::from(directory)];
    let mut source_count = 0;

    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(current)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }

            assert_eq!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("rs"),
                "non-Rust source under workspace crate boundary: {path:?}"
            );
            source_count += 1;
        }
    }

    assert!(source_count > 0, "workspace member has no Rust source: {directory:?}");
    Ok(())
}
