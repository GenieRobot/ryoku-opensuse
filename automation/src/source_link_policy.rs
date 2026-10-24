//! Preserve source-only Git links without following host filesystem paths.
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

pub fn resolve_source_link(
    source: &str,
    bytes: &[u8],
    selected_modes: &BTreeMap<String, u32>,
) -> Result<String, &'static str> {
    if source.is_empty()
        || source.bytes().any(|byte| byte < 32 || byte == 127 || matches!(byte, b'\\' | b':'))
        || source.split('/').any(|part| part.is_empty() || matches!(part, "." | ".."))
        || !Path::new(source).components().all(|part| matches!(part, Component::Normal(_)))
        || selected_modes.get(source) != Some(&0o120000)
    {
        return Err("source link must be a selected immutable Git link");
    }
    let target = std::str::from_utf8(bytes).map_err(|_| "non-UTF8 source link target")?;
    if target.is_empty()
        || target.len() > 4096
        || target.bytes().any(|byte| byte < 32 || byte == 127 || matches!(byte, b'\\' | b':'))
        || Path::new(target).is_absolute()
        || target.split('/').any(str::is_empty)
    {
        return Err("unsafe source link target");
    }
    let mut parts: Vec<_> = source.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop().ok_or("source link escapes archive root")?;
            }
            part => parts.push(part),
        }
    }
    let resolved = parts.join("/");
    if !matches!(selected_modes.get(&resolved), Some(0o644 | 0o755)) {
        return Err("source link target must be a selected regular Git blob");
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected() -> BTreeMap<String, u32> {
        [
            ("ryoku/rashin/app/logo.svg", 0o120000),
            ("ryoku/rashin/web/public/favicon.svg", 0o120000),
            ("ryoku/assets/brand/rashin-mark.svg", 0o644),
            ("ryoku/assets/brand/rashin-mark-small.svg", 0o644),
        ]
        .into_iter()
        .map(|(path, mode)| (path.to_owned(), mode))
        .collect()
    }

    #[test]
    fn resolves_both_pinned_icon_links_from_source_paths_only() {
        for (source, target, expected) in [
            (
                "ryoku/rashin/app/logo.svg",
                "../../assets/brand/rashin-mark.svg",
                "ryoku/assets/brand/rashin-mark.svg",
            ),
            (
                "ryoku/rashin/web/public/favicon.svg",
                "../../../assets/brand/rashin-mark-small.svg",
                "ryoku/assets/brand/rashin-mark-small.svg",
            ),
        ] {
            assert_eq!(
                resolve_source_link(source, target.as_bytes(), &selected()),
                Ok(expected.into())
            );
        }
    }

    #[test]
    fn rejects_escaping_absolute_malformed_and_unselected_targets() {
        let source = "ryoku/rashin/app/logo.svg";
        for target in [
            "",
            "/etc/passwd",
            "../../../../outside",
            "../../../outside",
            "../../assets/brand/missing.svg",
            "../../assets/brand/",
            "../../assets//brand/rashin-mark.svg",
            "../../assets/brand/rashin-mark.svg\n",
            "../../assets/brand/rashin-mark.svg\0",
            "C:/outside",
            "..\\outside",
        ] {
            assert!(
                resolve_source_link(source, target.as_bytes(), &selected()).is_err(),
                "{target:?}"
            );
        }
        assert!(resolve_source_link(source, &[0xff], &selected()).is_err());
        assert!(resolve_source_link(source, &vec![b'x'; 4097], &selected()).is_err());
        for bad_source in ["/absolute", "ryoku/../logo", "ryoku//logo", "unknown"] {
            assert!(resolve_source_link(bad_source, b"../target", &selected()).is_err());
        }
    }

    #[test]
    fn refuses_link_chains_cycles_submodules_and_non_link_sources() {
        let source = "ryoku/rashin/app/logo.svg";
        let target = b"../../assets/brand/rashin-mark.svg";
        for mode in [0, 0o120000, 0o160000] {
            let mut modes = selected();
            modes.insert("ryoku/assets/brand/rashin-mark.svg".into(), mode);
            assert!(resolve_source_link(source, target, &modes).is_err());
        }
        assert!(resolve_source_link(source, b"logo.svg", &selected()).is_err());
        assert!(
            resolve_source_link("ryoku/assets/brand/rashin-mark.svg", b"target", &selected())
                .is_err()
        );
    }
}
