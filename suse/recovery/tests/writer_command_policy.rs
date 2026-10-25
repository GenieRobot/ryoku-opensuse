//! Exercise both public Hub aliases and the explicit Rust role entry.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hub_alias_and_role_refuse_actuators_before_native_launch() {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let fixture = Fixture(
        std::env::temp_dir().join(format!("ryoku-hub-policy-{}-{unique}", std::process::id())),
    );
    fs::create_dir(&fixture.0).unwrap();
    let alias = fixture.0.join("ryoku-hub");
    let binary = env!("CARGO_BIN_EXE_ryoku-writer");
    std::os::unix::fs::symlink(binary, &alias).unwrap();
    for arguments in [
        vec!["voxtype", "download", "parakeet-redux"],
        vec!["phone", "pair", "device"],
        vec!["phone", "status"],
        vec!["phone", "devices"],
        vec!["connectivity", "set"],
        vec!["connectivity", "connect"],
        vec!["network", "hotspot-apply"],
        vec!["battery", "charge-limit"],
        vec!["package", "install"],
        vec!["voxtype", "restart"],
        vec!["nomarchy", "nomarchy-activate", "/tmp/unreviewed/nomarchy-activate"],
        vec!["nomarchy", "nomarchy-deactivate", "/usr/share/ryoku/nomarchy/nomarchy-deactivate"],
        vec!["import", "apply", "decisions.json"],
        vec!["rice", "apply", "look"],
        vec!["shell", "loaders", "bash"],
    ] {
        for typed_role in [false, true] {
            let mut command =
                Command::new(if typed_role { std::path::Path::new(binary) } else { &alias });
            if typed_role {
                command.arg("hub-backend");
            }
            let output = command.args(&arguments).output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert_eq!(output.stderr, b"ryoku-writer: Unsupported\n", "{arguments:?}");
            assert!(output.stdout.is_empty());
        }
    }
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
}
