//! Evidence checks only. These do not certify runtime query purity.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

const EVIDENCE: &str = include_str!("../../upstream/evidence/ryoku-v1.0/plugin-query-effects.json");
const MANIFEST: &str =
    include_str!("../../upstream/evidence/ryoku-v1.0/writer-source-manifest.tsv");

#[test]
fn plugin_effect_evidence_matches_selected_source_and_unchanged_dispatch() {
    let evidence: Value = serde_json::from_str(EVIDENCE).unwrap();
    let manifest: BTreeMap<_, _> = MANIFEST
        .lines()
        .skip(1)
        .map(|line| {
            let fields: Vec<_> = line.split('\t').collect();
            (fields[0], (fields[1], fields[2]))
        })
        .collect();
    let patch = include_str!("../../upstream/patches/issue18-writer-launch.patch");
    for source in evidence["sources"].as_array().unwrap() {
        let path = source["source"].as_str().unwrap();
        let (blob, mode) = manifest[path];
        assert_eq!(blob, source["git_blob"].as_str().unwrap(), "{path}");
        assert_eq!(format!("100{mode}"), source["git_mode"].as_str().unwrap());
        assert!(!patch.contains(&format!("diff --git a/{path} ")), "{path}");
    }
    let policy = include_str!("../../suse/recovery/src/writer_command_policy.rs");
    assert!(policy.contains("&[\"desktop\", \"plugins\", \"list\"]"));
    assert_eq!(evidence["effects"].as_array().unwrap().len(), 4);
    assert_eq!(evidence["owners"].as_object().unwrap().len(), 3);
    assert_eq!(evidence["route"], serde_json::json!(["desktop", "plugins", "list"]));
    assert_eq!(evidence["classification"], "accepted-by-current-Hub-argv-table-but-not-query-pure");
}

#[test]
#[ignore = "requires existing offline pinned Git object store, not network"]
fn plugin_effect_anchors_match_exact_pinned_blobs() {
    let repository = PathBuf::from(std::env::var_os("RYOKU_PINNED_GIT").unwrap());
    let evidence: Value = serde_json::from_str(EVIDENCE).unwrap();
    let pin = evidence["upstream_commit"].as_str().unwrap();
    let mut contents = BTreeMap::new();
    for source in evidence["sources"].as_array().unwrap() {
        let path = source["source"].as_str().unwrap();
        let result = Command::new("git")
            .env("GIT_NO_LAZY_FETCH", "1")
            .arg("-C")
            .arg(&repository)
            .args(["show", &format!("{pin}:{path}")])
            .output()
            .unwrap();
        assert!(result.status.success(), "{path}");
        assert_eq!(format!("{:x}", Sha256::digest(&result.stdout)), source["sha256"]);
        contents.insert(path.to_owned(), String::from_utf8(result.stdout).unwrap());
    }
    let effects = evidence["effects"].as_array().unwrap();
    let anchors = evidence["entrypoints"]
        .as_array()
        .unwrap()
        .iter()
        .chain(effects.iter().flat_map(|effect| effect["anchors"].as_array().unwrap().iter()));
    for anchor in anchors {
        let path = anchor["source"].as_str().unwrap();
        let lines: Vec<_> = contents[path].lines().collect();
        for line in anchor["lines"].as_array().unwrap() {
            let index = usize::try_from(line.as_u64().unwrap()).unwrap() - 1;
            assert!(lines[index].contains(anchor["text"].as_str().unwrap()), "{path}:{line}");
        }
    }
}
