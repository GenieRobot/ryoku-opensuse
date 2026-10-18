use super::*;
use ryoku_profiles::{Density, Desktop, ProfileDocument, ProfileLayers, Theme};
use std::io::Write;
const DEFAULTS: &[u8] = include_bytes!("../../../packaging/rpm/defaults/profile.json");
fn inventory() -> Vec<u8> {
    format!("PKG\tryoku-profile-defaults\t0\t0.1.0\t1.1\tnoarch\t8\nFILE\t/usr/share/ryoku\t\t16877\troot\troot\t0\t0\nFILE\t{DEFAULTS_PATH}\t{}\t33188\troot\troot\t{}\t0\nFILE\t/usr/share/licenses/ryoku-profile-defaults/LICENSE\t{}\t33188\troot\troot\t35000\t128\n", digest(DEFAULTS).as_str(), DEFAULTS.len(), "a".repeat(64)).into_bytes()
}
#[test]
fn packaged_payload_is_port_owned_complete_generic_data_without_companion() {
    let document = ProfileDocument::parse(DEFAULTS).unwrap();
    assert_eq!(document.id.as_str(), "generic-desktop");
    let settings = ProfileLayers { defaults: document.settings, ..Default::default() }.resolve();
    assert_eq!(
        (settings.desktop, settings.theme, settings.density),
        (Some(Desktop::Hyprland), Some(Theme::RyokuDark), Some(Density::Comfortable))
    );
    let proof = load_with(|| Ok(inventory()), || Ok(DEFAULTS.to_vec())).unwrap();
    assert_eq!(proof.package().name.as_str(), DEFAULTS_PACKAGE);
    assert_eq!(proof.package().nevra.as_str(), "ryoku-profile-defaults-0:0.1.0-1.1.noarch");
    assert_eq!(proof.package().upstream_revision, None);
    assert_eq!(proof.bytes(), DEFAULTS);
}
#[test]
fn exact_file_array_row_is_selected_and_ambiguous_owners_are_rejected() {
    let original = String::from_utf8(inventory()).unwrap();
    let selected = parse_inventory(original.as_bytes()).unwrap();
    assert_eq!(selected.digest, digest(DEFAULTS));
    assert_eq!(selected.size, DEFAULTS.len() as u64);
    for bad in [
        format!("{original}{original}"),
        format!(
            "{original}{}\n",
            original.lines().find(|line| line.contains(DEFAULTS_PATH)).unwrap()
        ),
        original.replace(DEFAULTS_PATH, "/usr/share/ryoku/config/profile.json.extra"),
        original.replace("PKG\tryoku-profile-defaults", "PKG\tforeign-package"),
        original.replace("\tnoarch\t8", "\tx86_64\t8"),
        original.replace("\tnoarch\t8", "\tnoarch\t1"),
        original.replace("\t33188\troot\troot", "\t33206\troot\troot"),
        original.replace("\t33188\troot\troot", "\t33188\tuser\troot"),
        original.replace("\t33188\troot\troot", "\t33188\troot\tuser"),
        original.replace("\t0\t0.1.0", "\t00\t0.1.0"),
        original.trim_end().into(),
        format!("{original}\0"),
    ] {
        assert!(parse_inventory(bad.as_bytes()).is_err());
    }
    assert!(matches!(
        parse_inventory(&vec![b'x'; MAX_STDOUT + 1]),
        Err(RpmDefaultsError::OutputTooLarge)
    ));
}
#[test]
fn modified_bytes_failed_query_and_package_changes_block_loader() {
    assert!(matches!(
        load_with(|| Ok(inventory()), || Ok(b"tampered".to_vec())),
        Err(RpmDefaultsError::ContentMismatch)
    ));
    assert!(matches!(
        load_with(|| Err(RpmDefaultsError::CommandFailed), || panic!("read must not occur")),
        Err(RpmDefaultsError::CommandFailed)
    ));
    let mut calls = 0;
    assert!(matches!(
        load_with(
            || {
                calls += 1;
                let bytes = inventory();
                Ok(if calls == 2 {
                    String::from_utf8(bytes).unwrap().replace("\t1.1\t", "\t2.1\t").into_bytes()
                } else {
                    bytes
                })
            },
            || Ok(DEFAULTS.to_vec())
        ),
        Err(RpmDefaultsError::ConcurrentChange)
    ));
    let mut reads = 0;
    assert!(matches!(
        load_with(
            || Ok(inventory()),
            || {
                reads += 1;
                Ok(if reads == 2 { b"changed after query".to_vec() } else { DEFAULTS.to_vec() })
            }
        ),
        Err(RpmDefaultsError::ConcurrentChange)
    ));
}
#[test]
fn native_command_has_only_fixed_readonly_arguments_and_clean_environment() {
    let command = native_command();
    assert_eq!(command.get_program(), RPM);
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(args, ["--noplugins", "--query", "--file", DEFAULTS_PATH, "--queryformat", FORMAT]);
    assert_eq!(command.get_envs().count(), 3);
    assert!(!FORMAT.contains("lua") && !FORMAT.contains("%(") && !FORMAT.contains("SCRIPTS"));
}
fn child(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear().env("RYOKU_TEST_RPM_CHILD", mode).args([
        "--exact",
        "rpm_defaults::tests::bounded_child_fixture",
        "--nocapture",
        "--quiet",
    ]);
    command
}
#[test]
fn bounded_child_fixture() {
    let Ok(mode) = std::env::var("RYOKU_TEST_RPM_CHILD") else { return };
    match mode.as_str() {
        "stdout" => {
            let _ = std::io::stdout().write_all(&vec![b'x'; MAX_STDOUT * 2]);
        }
        "stderr" => {
            let _ = std::io::stderr().write_all(&vec![b'e'; MAX_STDERR * 2]);
        }
        "timeout" => thread::sleep(Duration::from_secs(5)),
        "failure" => std::process::exit(7),
        "success" => {
            println!("bounded-success-marker");
        }
        _ => panic!("unexpected private fixture mode"),
    }
}
#[test]
fn subprocess_output_lifetime_and_failures_are_bounded_without_shells() {
    for mode in ["stdout", "stderr"] {
        assert!(matches!(
            run_bounded(child(mode), Duration::from_secs(2)),
            Err(RpmDefaultsError::OutputTooLarge)
        ));
    }
    let started = Instant::now();
    assert!(matches!(
        run_bounded(child("timeout"), Duration::from_millis(100)),
        Err(RpmDefaultsError::TimedOut)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(
        run_bounded(child("failure"), Duration::from_secs(2)),
        Err(RpmDefaultsError::CommandFailed)
    ));
    let bytes = run_bounded(child("success"), Duration::from_secs(2)).unwrap();
    assert!(std::str::from_utf8(&bytes).unwrap().contains("bounded-success-marker"));
}
