//! Time admission only for the explicitly selected disposable #21 fixture.
use crate::{
    guest::fixture_only,
    process,
    support::{Result, require},
};
use serde_json::{Value, json};
use std::{
    fs,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// Latest detached signature on the exact pinned repository metadata:
// 2026-10-21T16:04:49Z. A later run must refresh the fixture after 30 days.
const MIN_EPOCH: u64 = 1_792_598_689;
const MAX_EPOCH: u64 = MIN_EPOCH + 30 * 24 * 60 * 60;
const MAX_SKEW: u64 = 5;

pub fn validate_epoch(value: u64) -> Result<u64> {
    require(
        (MIN_EPOCH..=MAX_EPOCH).contains(&value),
        "host time is outside signed fixture validity window",
    )?;
    Ok(value)
}

pub fn host_epoch() -> Result<u64> {
    validate_epoch(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

pub fn utc() -> Result<String> {
    Ok(command("/usr/bin/date", &["--utc", "+%Y-%m-%dT%H:%M:%SZ"])?.trim().to_owned())
}

fn command(program: &str, args: &[&str]) -> Result<String> {
    let output = process::run(Command::new(program).args(args), Duration::from_secs(15), 16_384)?;
    require(
        !output.timed_out
            && output.status.success()
            && !output.stdout_truncated
            && !output.stderr_truncated,
        "fixture clock command failed",
    )?;
    Ok(String::from_utf8(output.stdout)?)
}

fn sample() -> Result<Value> {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| i128::from(v.as_secs()))
        .unwrap_or_else(|error| -i128::from(error.duration().as_secs()));
    let uptime = fs::read_to_string("/proc/uptime")?;
    let uptime: f64 = uptime.split_whitespace().next().ok_or("missing monotonic clock")?.parse()?;
    require(uptime.is_finite() && uptime >= 0.0, "invalid monotonic clock")?;
    Ok(
        json!({"epoch":epoch, "utc":command("/usr/bin/date", &["--utc", "+%Y-%m-%dT%H:%M:%SZ"])?.trim(),
        "monotonic_seconds":uptime, "timedatectl":command("/usr/bin/timedatectl", &["show", "--property=NTP", "--property=NTPSynchronized", "--property=TimeUSec", "--property=RTCTimeUSec"])?}),
    )
}

pub fn prepare(epoch: &str) -> Result<()> {
    require(cfg!(feature = "issue21-lab"), "clock changes require isolated issue21 fixture")?;
    fixture_only()?;
    require(rustix::process::geteuid().as_raw() == 0, "fixture clock changes require root")?;
    let parsed = validate_epoch(epoch.parse()?)?;
    require(parsed.to_string() == epoch, "noncanonical host clock value")?;
    let before = sample()?;
    eprintln!("native_issue21_clock_before={before}");
    // Keep external NTP from undoing the authorized host-derived fixture time.
    command("/usr/bin/timedatectl", &["set-ntp", "false"])?;
    command("/usr/bin/date", &["--utc", "--set", &format!("@{parsed}")])?;
    let after = sample()?;
    eprintln!("native_issue21_clock_after={after}");
    println!("{}", json!({"requested_epoch":parsed, "before":before, "after":after}));
    Ok(())
}

pub fn verify(reply: &str, requested: u64, current_host: u64) -> Result<()> {
    validate_epoch(requested)?;
    validate_epoch(current_host)?;
    let reply: Value = serde_json::from_str(reply)?;
    require(
        reply["requested_epoch"].as_u64() == Some(requested),
        "guest acknowledged a different host clock",
    )?;
    let actual = reply["after"]["epoch"].as_u64().ok_or("guest clock result absent")?;
    require(
        actual.abs_diff(current_host) <= MAX_SKEW,
        "guest clock exceeds five-second drift bound",
    )?;
    for stage in ["before", "after"] {
        require(
            reply[stage]["epoch"].is_i64()
                && reply[stage]["utc"].as_str().is_some_and(|v| v.ends_with('Z'))
                && reply[stage]["monotonic_seconds"]
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && v >= 0.0)
                && reply[stage]["timedatectl"].is_string(),
            "incomplete clock observation",
        )?;
    }
    require(
        reply["after"]["timedatectl"]
            .as_str()
            .is_some_and(|value| value.lines().any(|line| line == "NTP=no")),
        "guest NTP remains enabled",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_clock_requires_signed_fixture_window() {
        assert!(validate_epoch(MIN_EPOCH - 1).is_err());
        assert!(validate_epoch(MIN_EPOCH).is_ok());
        assert!(validate_epoch(MAX_EPOCH).is_ok());
        assert!(validate_epoch(MAX_EPOCH + 1).is_err());
    }
    #[test]
    fn drift_bound_and_complete_observations_fail_closed() {
        let sample = json!({"epoch":MIN_EPOCH, "utc":"2026-10-21T16:04:49Z", "monotonic_seconds":10.0, "timedatectl":"NTP=no\n"});
        let valid = json!({"requested_epoch":MIN_EPOCH, "before":sample, "after":sample});
        verify(&valid.to_string(), MIN_EPOCH, MIN_EPOCH + 5).unwrap();
        assert!(verify(&valid.to_string(), MIN_EPOCH, MIN_EPOCH + 6).is_err());
        let mut invalid = valid.clone();
        invalid["after"]["epoch"] = json!(0);
        assert!(verify(&invalid.to_string(), MIN_EPOCH, MIN_EPOCH).is_err());
        let mut invalid = valid.clone();
        invalid["before"]["monotonic_seconds"] = Value::Null;
        assert!(verify(&invalid.to_string(), MIN_EPOCH, MIN_EPOCH).is_err());
        let mut invalid = valid.clone();
        invalid["after"]["timedatectl"] = json!("NTP=yes\n");
        assert!(verify(&invalid.to_string(), MIN_EPOCH, MIN_EPOCH).is_err());
        assert!(verify(&valid.to_string(), MIN_EPOCH + 1, MIN_EPOCH).is_err());
    }
}
