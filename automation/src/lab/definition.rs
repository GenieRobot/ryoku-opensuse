use crate::support::Result;

// This fixture deliberately uses unique scalar keys. Refuse ambiguous or
// malformed values rather than silently using defaults or a moving URL.
pub fn scalar(key: &str) -> Result<&'static str> {
    let source = if cfg!(feature = "issue19-lab") {
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
