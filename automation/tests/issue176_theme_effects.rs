//! Immutable source contract for #176's still-required real theme integration.
//! This verifies inventory completeness; it grants no execution permission.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const LEDGER: &str = include_str!("../../upstream/evidence/ryoku-v1.0/issue176-theme-effects.json");
const CORE: &str = include_str!("fixtures/issue176/config.toml");
const APPS: &str = include_str!("fixtures/issue176/apps.toml");

#[test]
fn theme_ledger_remains_bound_to_the_active_pin_and_selected_source_blobs() {
    let ledger: Value = serde_json::from_str(LEDGER).unwrap();
    let source: toml::Value =
        toml::from_str(include_str!("../../upstream/ryoku.source.toml")).unwrap();
    assert_eq!(ledger["upstream"].as_str(), source["revision"].as_str());
    let selected = include_str!("../../upstream/evidence/ryoku-v1.0/writer-source-manifest.tsv");
    let mut checked = 0;
    for (path, blob) in ledger["source_blobs"].as_object().unwrap() {
        let entries = selected
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut columns = line.split('\t');
                (columns.next()? == path).then(|| columns.next()).flatten()
            })
            .collect::<Vec<_>>();
        if let [actual] = entries.as_slice() {
            assert_eq!(Some(*actual), blob.as_str(), "{path}");
            checked += 1;
        } else {
            assert!(entries.is_empty(), "ambiguous selected source {path}");
        }
    }
    assert_eq!(
        checked, 6,
        "all six selected worker/bridge source identities remain pinned; helper scripts and template fixtures retain separate raw-source provenance"
    );
}

#[test]
fn theme_ledger_exactly_covers_all_pinned_template_inputs_outputs_and_post_hooks() {
    let ledger: Value = serde_json::from_str(LEDGER).unwrap();
    assert_eq!(ledger["upstream"], "a59d051f0e8080052216f6c7b9eb7e5928935ce8");
    let rows = ledger["templates"].as_array().unwrap();
    let mut expected = BTreeSet::new();
    for (config, source) in [("core", CORE), ("apps", APPS)] {
        assert_eq!(
            format!("{:x}", Sha256::digest(source.as_bytes())),
            ledger["fixture_sha256"][config]
        );
        let document: toml::Value = toml::from_str(source).unwrap();
        for (key, template) in document["templates"].as_table().unwrap() {
            let matched = rows
                .iter()
                .filter(|row| row["config"] == config && row["key"] == key.as_str())
                .collect::<Vec<_>>();
            let [row] = matched.as_slice() else {
                panic!("template must have exactly one source disposition: {config}/{key}");
            };
            let input = template["input_path"]
                .as_str()
                .unwrap()
                .strip_prefix("~/.config/matugen/templates/")
                .unwrap();
            let output = template["output_path"].as_str().unwrap().strip_prefix("~/").unwrap();
            assert_eq!(row["input"], input);
            assert_eq!(row["output"], output);
            assert_eq!(
                row.get("post_hook").and_then(Value::as_str),
                template.get("post_hook").and_then(toml::Value::as_str)
            );
            let group = match key.as_str() {
                "gtk3" | "gtk4" => "gtk",
                "vesktop" | "equibop" => "discord",
                "qt6ct" | "kde" => "qt",
                "hypr" => "hyprland",
                other => other,
            };
            assert_eq!(row["group"], group);
            assert!(expected.insert((config.to_string(), key.to_string())));
        }
    }
    let actual = rows
        .iter()
        .map(|row| {
            (row["config"].as_str().unwrap().to_string(), row["key"].as_str().unwrap().to_string())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, actual);
    assert_eq!(rows.len(), 30);
}

#[test]
fn theme_ledger_keeps_indirect_generation_and_logical_reload_effects_in_scope() {
    let ledger: Value = serde_json::from_str(LEDGER).unwrap();
    let ids = |key: &str| {
        ledger[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        ids("direct_effects"),
        [
            "tone",
            "palette",
            "carrier",
            "gtk-off",
            "kde-merge",
            "video-stills",
            "video-poster",
            "obsidian-snippets",
            "preview",
            "configured-integrations"
        ]
        .map(String::from)
        .into()
    );
    assert_eq!(
        ids("indirect_effects"),
        ["folder-icons", "cursor", "nomarchy", "starship", "papirus"].map(String::from).into()
    );
    assert_eq!(
        ids("reload_effects"),
        ["gsettings", "kde-notify", "kitty", "shell-palette", "wm", "lighting"]
            .map(String::from)
            .into()
    );
    let keys = ledger["reload_effects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "gsettings")
        .unwrap()["keys"]
        .as_array()
        .unwrap();
    let expected_keys =
        serde_json::json!(["color-scheme", "accent-color", "gtk-theme", "icon-theme"]);
    assert_eq!(keys, expected_keys.as_array().unwrap());
    assert_eq!(
        ledger["adjacent_not_wallpaper_trigger"]["source"],
        "settings.go -> applyFont; not paintWorker"
    );
    assert!(
        ledger["path_policy"]["logical_settings"]
            .as_str()
            .unwrap()
            .contains("never restore entire dconf")
    );
    assert!(
        ledger["path_policy"]["outputs"].as_str().unwrap().contains("unrelated siblings preserved")
    );
    assert!(
        ledger["path_policy"]["symlinks"]
            .as_str()
            .unwrap()
            .contains("regular asset restore must keep refusing")
    );
}
