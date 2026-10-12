//! Pin every enabled native repository before package solving or installation.
use crate::{
    definition::scalar,
    guest::fixture_only,
    support::{Result, output, require},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

const REPOSITORIES: [(&str, &str, &str, &str); 4] = [
    ("repo-oss", "repo_oss_url", "repo_oss_repomd_sha256", "tumbleweed/repo/oss"),
    ("repo-non-oss", "repo_non_oss_url", "repo_non_oss_repomd_sha256", "tumbleweed/repo/non-oss"),
    ("repo-openh264", "repo_codec_url", "repo_codec_repomd_sha256", "openh264/openSUSE_Tumbleweed"),
    ("repo-update", "repo_update_url", "repo_update_repomd_sha256", "update/tumbleweed"),
];
const SIGNATURE_SETTINGS: [&str; 3] = ["gpgcheck", "repo_gpgcheck", "pkg_gpgcheck"];

fn boolean(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err("invalid native repository boolean".into()),
    }
}

fn pinned_section(lines: &[&str]) -> Result<(String, Option<String>)> {
    let alias = lines[0]
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or("malformed native repository section")?;
    let mut settings = BTreeMap::new();
    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        let (key, value) = trimmed.split_once('=').ok_or("malformed native repository setting")?;
        require(
            settings.insert(key.trim(), value.trim()).is_none(),
            "duplicate native repository setting",
        )?;
    }
    if !settings.get("enabled").map_or(Ok(true), |value| boolean(value))? {
        return Ok((format!("{}\n", lines.join("\n")), None));
    }
    let (_, url_key, _, original_path) = REPOSITORIES
        .iter()
        .find(|repo| repo.0 == alias)
        .ok_or("unknown enabled repository; refusing unpinned package source")?;
    // Missing fields inherit native defaults; the signed cloud fixture omits
    // all three. Canonicalize them to explicit mandatory checks below rather
    // than mistaking inheritance for disabled verification. An explicit false
    // or invalid override remains a failed input, never an accepted policy.
    for key in SIGNATURE_SETTINGS {
        if let Some(value) = settings.get(key) {
            require(boolean(value)?, "native repository signature override disabled")?;
        }
    }
    require(
        !settings.contains_key("mirrorlist") && !settings.contains_key("metalink"),
        "repository indirection is not a pinned source",
    )?;
    let old =
        settings.get("baseurl").ok_or("native repository baseurl missing")?.trim_end_matches('/');
    let pinned = scalar(url_key)?;
    let accepted = if alias == "repo-openh264" {
        ["http://codecs.opensuse.org/", "https://codecs.opensuse.org/"]
            .iter()
            .any(|prefix| old == format!("{prefix}{original_path}"))
    } else {
        [
            "http://download.opensuse.org/",
            "https://download.opensuse.org/",
            "https://downloadcontent2.opensuse.org/",
        ]
        .iter()
        .any(|prefix| old == format!("{prefix}{original_path}"))
    };
    require(
        accepted || old == pinned.trim_end_matches('/'),
        "unexpected original repository origin or path",
    )?;
    let mut result = String::new();
    for line in lines {
        let key = line.split_once('=').map(|(key, _)| key.trim());
        if key == Some("baseurl") {
            result.push_str(&format!("baseurl={pinned}\n"));
        } else if key.is_some_and(|key| SIGNATURE_SETTINGS.contains(&key)) {
            // Write each verified setting once in canonical form below.
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    for key in SIGNATURE_SETTINGS {
        result.push_str(&format!("{key}=1\n"));
    }
    Ok((result, Some(alias.to_owned())))
}

fn pinned_file(source: &str) -> Result<(String, Vec<String>)> {
    let lines: Vec<_> = source.lines().collect();
    let mut result = String::new();
    let mut aliases = Vec::new();
    let mut start = 0;
    while start < lines.len() && !lines[start].trim().starts_with('[') {
        require(
            lines[start].trim().is_empty() || lines[start].trim().starts_with(['#', ';']),
            "setting outside native repository section",
        )?;
        result.push_str(lines[start]);
        result.push('\n');
        start += 1;
    }
    while start < lines.len() {
        let end = (start + 1..lines.len())
            .find(|index| lines[*index].trim().starts_with('['))
            .unwrap_or(lines.len());
        let (section, alias) = pinned_section(&lines[start..end])?;
        result.push_str(&section);
        if let Some(alias) = alias {
            aliases.push(alias);
        }
        start = end;
    }
    Ok((result, aliases))
}

pub fn pin() -> Result<()> {
    fixture_only()?;
    let mut changes = Vec::new();
    let mut aliases = BTreeSet::new();
    for entry in fs::read_dir("/etc/zypp/repos.d")? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "repo") {
            let metadata = fs::symlink_metadata(&path)?;
            require(
                metadata.is_file(),
                "native repository file must not be a symlink or special file",
            )?;
            let (pinned, found) = pinned_file(&fs::read_to_string(&path)?)?;
            for alias in found {
                require(aliases.insert(alias), "duplicate enabled repository alias")?;
            }
            changes.push((path, pinned));
        }
    }
    require(
        REPOSITORIES.iter().all(|repo| aliases.contains(repo.0)),
        "required native repository missing or disabled",
    )?;
    // Validate the whole source set before changing any file; never silently
    // disable an enabled update/codec repository to obtain an easier solver.
    for (path, contents) in changes {
        fs::write(path, contents)?;
    }
    Ok(())
}

pub fn verify_cache() -> Result<()> {
    fixture_only()?;
    for (alias, _, hash_key, _) in REPOSITORIES {
        let metadata = Path::new("/var/cache/zypp/raw").join(alias).join("repodata/repomd.xml");
        require(
            fs::symlink_metadata(&metadata)?.is_file(),
            "native signed repository metadata cache missing or indirect",
        )?;
        let digest = output(Command::new("sha256sum").arg(&metadata))?;
        require(
            digest.split_whitespace().next() == Some(scalar(hash_key)?),
            "native repository metadata differs from signed fixture pin",
        )?;
    }
    println!("repository_metadata=signature-checked-and-pinned");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_missing_signature_fields_become_explicit_mandatory_checks() {
        let source = "[repo-oss]\nname=openSUSE-Tumbleweed-Oss\nenabled=1\nautorefresh=1\nbaseurl=https://downloadcontent2.opensuse.org/tumbleweed/repo/oss/\n";
        let (pinned, aliases) = pinned_file(source).unwrap();
        assert_eq!(aliases, ["repo-oss"]);
        for key in SIGNATURE_SETTINGS {
            assert_eq!(pinned.lines().filter(|line| *line == format!("{key}=1")).count(), 1);
        }
        assert!(pinned.contains("/history/20261011/"));
        assert!(pinned.contains("enabled=1"));
        assert_eq!(pinned_file(&pinned).unwrap().0, pinned);
    }

    #[test]
    fn native_true_values_canonicalize_and_false_or_invalid_overrides_fail() {
        let source =
            "[repo-oss]\nenabled=1\nbaseurl=https://download.opensuse.org/tumbleweed/repo/oss/\n";
        for key in SIGNATURE_SETTINGS {
            for value in ["1", "true", "yes", "on", "TRUE", "Yes"] {
                let (pinned, _) = pinned_file(&format!("{source}{key}={value}\n")).unwrap();
                assert!(pinned.contains(&format!("{key}=1\n")));
            }
            for value in ["0", "false", "no", "off", "", "maybe", "yes;skip"] {
                assert!(pinned_file(&format!("{source}{key}={value}\n")).is_err());
            }
        }
    }

    #[test]
    fn core_repository_uses_snapshot_history_and_preserves_signatures() {
        let source = "[repo-oss]\nenabled=1\ngpgcheck=1\nbaseurl=https://downloadcontent2.opensuse.org/tumbleweed/repo/oss/\n";
        let (pinned, aliases) = pinned_file(source).unwrap();
        assert!(pinned.contains("/history/20261011/tumbleweed/repo/oss/"));
        assert!(pinned.contains("gpgcheck=1"));
        assert_eq!(aliases, ["repo-oss"]);
        assert_eq!(pinned_file(&pinned).unwrap().0, pinned);
    }

    #[test]
    fn enabled_unknown_unsigned_duplicate_or_redirected_sources_fail_closed() {
        let source = "[repo-oss]\nenabled=1\ngpgcheck=1\nbaseurl=https://download.opensuse.org/tumbleweed/repo/oss/\n";
        for invalid in [
            source.replace("repo-oss", "third-party"),
            source.replace("gpgcheck=1", "gpgcheck=0"),
            source.replace("download.opensuse.org", "example.org"),
            format!("{source}repo_gpgcheck=0\n"),
            format!("{source}pkg_gpgcheck=0\n"),
            format!("{source}mirrorlist=https://example.org\n"),
            format!("{source}gpgcheck=1\n"),
        ] {
            assert!(pinned_file(&invalid).is_err());
        }
        let disabled = "[repo-debug]\nenabled=0\nbaseurl=https://download.opensuse.org/debug/tumbleweed/repo/oss/\n";
        assert_eq!(pinned_file(disabled).unwrap(), (disabled.to_owned(), Vec::new()));
    }
}
