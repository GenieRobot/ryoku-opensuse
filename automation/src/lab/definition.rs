use crate::support::Result;

// This fixture deliberately uses unique scalar keys. Refuse ambiguous or
// malformed values rather than silently using defaults or a moving URL.
pub fn scalar(key: &str) -> Result<&'static str> {
    let source = if cfg!(feature = "issue24-lab") {
        include_str!("../../../infra/tumbleweed-lab/issue24-fixture.toml")
    } else if cfg!(feature = "issue21-lab") {
        include_str!("../../../infra/tumbleweed-lab/issue21-fixture.toml")
    } else if cfg!(feature = "issue20-lab") {
        include_str!("../../../infra/tumbleweed-lab/issue20-fixture.toml")
    } else if cfg!(feature = "issue19-lab") {
        include_str!("../../../infra/tumbleweed-lab/issue19-fixture.toml")
    } else if cfg!(feature = "wsl-lab") {
        include_str!("../../../infra/tumbleweed-lab/wsl-fixture.toml")
    } else {
        include_str!("../../../infra/tumbleweed-lab/fixture.toml")
    };
    let mut values = source.lines().filter_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then_some(value.trim())
    });
    let value = values.next().ok_or_else(|| format!("missing fixture key {key}"))?;
    if values.next().is_some() {
        return Err(format!("ambiguous fixture key {key}").into());
    }
    if value.starts_with('"') {
        value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| format!("malformed fixture key {key}").into())
    } else if value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() {
        Ok(value)
    } else {
        Err(format!("unsupported fixture scalar {key}").into())
    }
}

#[cfg(all(test, feature = "issue24-lab"))]
mod tests {
    use super::*;

    #[test]
    fn installer_fixture_changes_only_the_authenticated_update_metadata_pin() {
        let recovery = include_str!("../../../infra/tumbleweed-lab/issue21-fixture.toml");
        let installer = include_str!("../../../infra/tumbleweed-lab/issue24-fixture.toml");
        let unchanged = |source: &'static str| {
            source
                .lines()
                .map(str::trim)
                .filter(|line| {
                    !line.is_empty()
                        && !line.starts_with('#')
                        && !line.starts_with("repo_update_repomd_sha256 =")
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(unchanged(recovery), unchanged(installer));
        assert_eq!(
            scalar("repo_update_repomd_sha256").unwrap(),
            "6d50646263e414aa5077840e66ca1767243aa7e2527bbe13c1e0c63353df0de5"
        );
        let metadata = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../infra/tumbleweed-lab/evidence/issue24-update-repomd-20261024.xml");
        let hash =
            crate::support::output(std::process::Command::new("sha256sum").arg(metadata)).unwrap();
        assert_eq!(
            hash.split_whitespace().next(),
            Some(scalar("repo_update_repomd_sha256").unwrap())
        );
    }
}
