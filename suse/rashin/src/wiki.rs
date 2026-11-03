//! Embedded offline Wiki with exact retained-source provenance and separate
//! Tumbleweed views. IDs select fixed compiled assets, never filesystem paths.
use crate::{Error, Result, api::Document, digest};
use serde::Serialize;
const REVISION: &str = "a59d051f0e8080052216f6c7b9eb7e5928935ce8";
struct Entry {
    id: &'static str,
    title: &'static str,
    file: &'static str,
    blob: &'static str,
    sha256: &'static str,
    text: &'static str,
}
const ENTRIES: &[Entry] = &[
    Entry {
        id: "overview",
        title: "Ryoku Wiki",
        file: "README.md",
        blob: "2741c7bf98f5dec67bf79d874b6c2687fd50c5e2",
        sha256: "30954be823edee4edde1f825e2c95b802133aa896e990785d6089b30a2d4e3fe",
        text: include_str!("../wiki/upstream/README.md"),
    },
    Entry {
        id: "desktop",
        title: "Desktop tour",
        file: "desktop.md",
        blob: "58db5a7a526182e2462cb443cb33c9b9c0939913",
        sha256: "b50af0c931b831cb5a005edde1d9c8f3941d9cfd9569268ba67eab2dcd36c4de",
        text: include_str!("../wiki/upstream/desktop.md"),
    },
    Entry {
        id: "go-tools",
        title: "Upstream Go tools",
        file: "go-tools.md",
        blob: "c84296c8708720cdf5cba94c6583faface2e6212",
        sha256: "e8c5d1e28e08a1294b7c53fd851d7f18312ad1b9a02f055740e1887ddf26502d",
        text: include_str!("../wiki/upstream/go-tools.md"),
    },
    Entry {
        id: "hyprland-lua",
        title: "Hyprland Lua",
        file: "hyprland-lua.md",
        blob: "906af9e91269ba54a1fe63748e56a23ec4a59596",
        sha256: "4ed9d87479929e632f4c13e5cb7a4151d60f8f9e1a142a19743e79c86dee22b2",
        text: include_str!("../wiki/upstream/hyprland-lua.md"),
    },
    Entry {
        id: "linux-basics",
        title: "Linux basics on Tumbleweed",
        file: "linux-basics.md",
        blob: "fcd8ee8fee7d230b66dfcc46534d55c10daf56dd",
        sha256: "c7ffe29210b3fecba24cecdd399d5d37aabf4441f846ef709cd182bb1c3f12f1",
        text: include_str!("../wiki/upstream/linux-basics.md"),
    },
    Entry {
        id: "niri-kdl",
        title: "Niri KDL",
        file: "niri-kdl.md",
        blob: "1d136e4abb9923b9d4ed3544c271326581d62cb1",
        sha256: "75518052b592a5e8e20fa8e8af4648009f0af0de12606294dbe095b742c7d1e3",
        text: include_str!("../wiki/upstream/niri-kdl.md"),
    },
    Entry {
        id: "quickshell-qml",
        title: "Quickshell QML",
        file: "quickshell-qml.md",
        blob: "f97fa516d5e2e287c83596d2102c54e5e016f256",
        sha256: "a78dbea07cdacc15d0cdf810123ab43ccf5cf97ea074fb841e1541a10142bd54",
        text: include_str!("../wiki/upstream/quickshell-qml.md"),
    },
    Entry {
        id: "rashin",
        title: "Rashin",
        file: "rashin.md",
        blob: "2cff66bff19f95ca1f6bbcf375c898b6993345d5",
        sha256: "7c9aa23afb3e32832827973e38e9f6dc4a3e6dac15f9e3a9659cec294839e3cd",
        text: include_str!("../wiki/upstream/rashin.md"),
    },
];
#[derive(Serialize)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub content: String,
    pub sha256: String,
    pub upstream_commit: &'static str,
    pub upstream_path: String,
    pub upstream_blob: &'static str,
    pub upstream_sha256: &'static str,
    pub original: bool,
}
fn entry(id: &str) -> Result<&'static Entry> {
    ENTRIES.iter().find(|entry| entry.id == id).ok_or(Error::NotFound)
}
pub fn page(id: &str, original: bool) -> Result<Page> {
    let entry = entry(id)?;
    if digest(entry.text.as_bytes()) != entry.sha256 {
        return Err(Error::Integrity);
    }
    let content = if original {
        entry.text.to_owned()
    } else {
        match id {
            "overview" => include_str!("../wiki/tumbleweed/overview.md").into(),
            "linux-basics" => include_str!("../wiki/tumbleweed/linux-basics.md").into(),
            "go-tools" => format!(
                "# Upstream Go tooling reference\n\nThis guide describes the retained upstream Go sources. Project-owned executable code in this Tumbleweed port is Rust. The original examples remain reference material.\n\n{}",
                entry.text
            ),
            _ => format!(
                "# Retained upstream reference\n\nThis Ryoku 1.0 guide describes the upstream component. Match desktop shortcuts, configuration paths and harness availability to your installed Tumbleweed providers. See the [Tumbleweed overview](README.md) for this assistant workspace.\n\n{}",
                entry.text
            ),
        }
    };
    Ok(Page {
        id: id.into(),
        title: entry.title.into(),
        sha256: digest(content.as_bytes()),
        content,
        upstream_commit: REVISION,
        upstream_path: format!("ryoku/rashin/wiki/{}", entry.file),
        upstream_blob: entry.blob,
        upstream_sha256: entry.sha256,
        original,
    })
}
pub fn pages() -> Result<Vec<Page>> {
    ENTRIES.iter().map(|entry| page(entry.id, false)).collect()
}
pub(crate) fn document(id: &str, sha256: &str) -> Result<Document> {
    let page = page(id, false)?;
    if page.sha256 != sha256 {
        return Err(Error::Conflict);
    }
    Ok(Document { name: format!("wiki/{id}.md"), content: page.content, sha256: page.sha256 })
}
pub(crate) fn link(file: &str) -> Option<String> {
    ENTRIES.iter().find(|entry| entry.file == file).map(|entry| format!("/wiki/{}", entry.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_retained_wiki_page_has_exact_source_provenance_and_separate_tumbleweed_content() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../upstream/evidence/rashin-v1.0/wiki-assets.json"
        ))
        .unwrap();
        assert_eq!(manifest["upstream_commit"], REVISION);
        assert_eq!(ENTRIES.len(), 8);
        assert_eq!(manifest["pages"].as_array().unwrap().len(), 8);
        let mut ids = std::collections::BTreeSet::new();
        let mut total = 0;
        for entry in ENTRIES {
            assert!(ids.insert(entry.id));
            let row = manifest["pages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == entry.id)
                .unwrap();
            let original = page(entry.id, true).unwrap();
            assert_eq!(original.content, entry.text);
            assert_eq!(original.sha256, entry.sha256);
            assert_eq!(row["git_blob"], entry.blob);
            assert_eq!(row["sha256"], entry.sha256);
            assert_eq!(row["bytes"], entry.text.len());
            total += entry.text.len();
            let current = page(entry.id, false).unwrap();
            assert_ne!(current.sha256, original.sha256);
            assert_eq!(document(entry.id, &current.sha256).unwrap().sha256, current.sha256);
            assert_eq!(document(entry.id, &original.sha256).err(), Some(Error::Conflict));
        }
        assert_eq!(total, 25481);
        for (path, bytes) in [
            (
                "suse/rashin/wiki/tumbleweed/overview.md",
                include_bytes!("../wiki/tumbleweed/overview.md").as_slice(),
            ),
            (
                "suse/rashin/wiki/tumbleweed/linux-basics.md",
                include_bytes!("../wiki/tumbleweed/linux-basics.md").as_slice(),
            ),
        ] {
            let row = manifest["tumbleweed_adaptations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["local_path"] == path)
                .unwrap();
            assert_eq!(row["sha256"], digest(bytes));
            assert_eq!(row["bytes"], bytes.len());
        }
        assert_eq!(digest(include_bytes!("../wiki/COPYING")), manifest["license_source"]["sha256"]);
        let linux = page("linux-basics", false).unwrap();
        assert!(linux.content.contains("zypper dup"));
        assert!(!linux.content.contains("pacman"));
        assert!(page("linux-basics", true).unwrap().content.contains("pacman"));
        assert!(page("overview", false).unwrap().content.contains("openSUSE Tumbleweed"));
        for id in ["../private", "/etc/passwd", "linux-basics.md", "license"] {
            assert_eq!(page(id, false).err(), Some(Error::NotFound));
        }
    }
}
