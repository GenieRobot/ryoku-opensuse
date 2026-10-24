//! Retained Hub queries do not grant upstream host, package or home actuators.
//! Mutation providers are accepted separately with authorization and recovery.
use std::ffi::OsString;

const QUERIES: &[&[&str]] = &[
    &["keybinds"],
    &["apps"],
    &["shell", "get"],
    &["desktop", "get"],
    &["desktop", "defaults"],
    &["desktop", "cursors"],
    &["desktop", "icons"],
    &["desktop", "layouts"],
    &["desktop", "icon-theme"],
    &["desktop", "plugins", "list"],
    &["wm", "list"],
    &["outputs"],
    &["outputs", "list"],
    &["outputs", "profiles"],
    &["outputs", "presets"],
    &["lock", "list"],
    &["gpu", "caps"],
    &["gpu", "mode"],
    &["gpu", "tune", "caps"],
    &["gpu", "tune", "get"],
    &["cpu", "caps"],
    &["cpu", "active"],
    &["lighting", "state"],
    &["voxtype", "get"],
    &["rashin", "status"],
    &["rice", "list"],
    &["rice", "preflight"],
    &["fastfetch", "get"],
    &["starship", "list"],
    &["starship", "get"],
    &["matugen"],
    &["matugen", "get"],
    &["palette-bridge", "status"],
    &["share", "status"],
    &["clipboard", "stats"],
    &["import", "providers"],
    &["import", "detect"],
];

pub(crate) fn hub_query(arguments: &[OsString]) -> bool {
    let Some(arguments): Option<Vec<_>> =
        arguments.iter().map(|argument| argument.to_str()).collect()
    else {
        return false;
    };
    if QUERIES.contains(&arguments.as_slice()) {
        return true;
    }
    match arguments.as_slice() {
        ["config", "get", key] => !key.is_empty(),
        ["desktop", "variants", layout] => !layout.is_empty(),
        ["cpu", "caps", profile] => !profile.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn queries_have_exact_arity_and_no_mutation_suffix_or_encoding_bypass() {
        for query in QUERIES {
            let mut arguments: Vec<_> = query.iter().map(OsString::from).collect();
            assert!(hub_query(&arguments), "{query:?}");
            arguments.push("set".into());
            arguments.push("value".into());
            assert!(!hub_query(&arguments), "query accepted extra verb: {query:?}");
        }
        for arguments in [
            ["config", "get", "theme"],
            ["desktop", "variants", "default"],
            ["cpu", "caps", "balanced"],
        ] {
            assert!(hub_query(&arguments.map(OsString::from)));
            let mut extra: Vec<_> = arguments.map(OsString::from).into();
            extra.push("write".into());
            assert!(!hub_query(&extra));
            extra.pop();
            extra[2] = "".into();
            assert!(!hub_query(&extra));
        }
        assert!(!hub_query(&[]));
        assert!(!hub_query(&["future-provider".into(), "status".into()]));
        assert!(!hub_query(&["phone".into(), OsString::from_vec(vec![0xff])]));
    }

    #[test]
    fn every_hub_mutation_family_refuses_before_upstream_dispatch() {
        // All command families from the immutable v1.0 main.go dispatch.
        let mutations: &[&[&str]] = &[
            &["keybinds", "set"],
            &["apps", "install"],
            &["config", "set", "key", "value"],
            &["shell", "set", "bash"],
            &["shell", "loaders", "bash"],
            &["desktop", "save", "{}"],
            &["desktop", "plugins", "add", "https://example.invalid/plugin"],
            &["desktop", "icon-theme", "apply"],
            &["wm", "preview", "niri"],
            &["outputs", "apply", "{}"],
            &["outputs", "save", "name", "{}"],
            &["lock", "set", "theme"],
            &["gpu", "apply", "hybrid"],
            &["gpu", "tune", "set", "0", "clock", "1"],
            &["cpu", "switch", "performance"],
            &["cpu", "set", "all", "governor", "performance"],
            &["lighting", "follow", "--all", "on"],
            &["lighting", "scan"],
            &["voxtype", "ensure"],
            &["voxtype", "download", "parakeet-redux"],
            &["voxtype", "rmmodel", "parakeet-redux"],
            &["voxtype", "set", "{}"],
            &["rashin", "on"],
            &["rashin", "off"],
            &["phone", "status"],
            &["phone", "devices"],
            &["phone", "pair", "device"],
            &["phone", "unpair", "device"],
            &["phone", "on"],
            &["rice", "apply", "look"],
            &["rice", "capture", "name"],
            &["rice", "import", "/tmp/look"],
            &["fastfetch", "save", "{}"],
            &["fastfetch", "effective", "/tmp/file"],
            &["starship", "set", "pill"],
            &["starship", "palette", "wallpaper"],
            &["starship", "refresh"],
            &["profile", "import", "/tmp/profile"],
            &["profile", "export", "/tmp/profile"],
            &["matugen", "set", "{}"],
            &["matugen", "render-apps"],
            &["reload-cover", "import", "/tmp/image"],
            &["palette-bridge", "install"],
            &["share", "install"],
            &["clipboard", "prune"],
            &["import", "scan", "https://example.invalid/config"],
            &["import", "apply", "decisions.json"],
            &["import", "undo"],
        ];
        for arguments in mutations {
            let arguments: Vec<_> = arguments.iter().map(OsString::from).collect();
            assert!(!hub_query(&arguments), "{arguments:?}");
            assert!(
                matches!(
                    crate::writer_protocol::run_writer(
                        crate::writer_protocol::WriterKind::HubBackend,
                        &arguments
                    ),
                    Err(crate::capture_session::CaptureSessionError::Unsupported)
                ),
                "{arguments:?}"
            );
        }
    }
}
