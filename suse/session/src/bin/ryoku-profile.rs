//! User-facing entry points for typed personal profile operations.

use std::{env, process::ExitCode};

use ryoku_ui::{BackendError, RequestId, service_client::UnixServiceBackend};

#[derive(Debug, Eq, PartialEq)]
enum Action {
    ShowLocal,
    SetLocal(ryoku_profiles::SettingsLayer),
    MigrateLegacy,
}

fn parse(args: &[String]) -> Result<Action, &'static str> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["show"] => Ok(Action::ShowLocal),
        ["set", json] if json.len() <= ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES => {
            serde_json::from_str(json)
                .map(Action::SetLocal)
                .map_err(|_| "Expected one bounded JSON settings object with only desktop, theme, and density fields.")
        }
        ["migrate-legacy"] => Ok(Action::MigrateLegacy),
        _ => Err("Usage: ryoku-profile show | set '{\"desktop\":null,\"theme\":\"system\",\"density\":null}' | migrate-legacy"),
    }
}

fn validate_identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<(), &'static str> {
    if uid == 0 || uid != euid || gid != egid {
        return Err("ryoku-profile requires a non-root user with matching real/effective IDs");
    }
    Ok(())
}

fn failure_message(error: BackendError) -> &'static str {
    match error {
        BackendError::NotFound => "No legacy profile was found; no files were changed.",
        BackendError::UnsupportedOperation => {
            "The existing profile is not a recognized legacy preferences file; no files were changed."
        }
        BackendError::StaleRequest => {
            "The profile request became stale; inspect local profile state before retrying."
        }
        BackendError::RecoveryUnavailable => {
            "The profile or user service could not be verified; inspect local profile state before retrying."
        }
        BackendError::Unavailable => "The Ryoku user service is unavailable.",
        BackendError::Unauthorized => "The Ryoku user service rejected this user.",
        BackendError::Busy => "The Ryoku user service is busy; try again later.",
        BackendError::Failed => {
            "Profile migration did not finish cleanly; inspect local profile state before retrying."
        }
        _ => "Profile migration failed; inspect local profile state before retrying.",
    }
}

fn show_failure_message(error: BackendError) -> &'static str {
    match error {
        BackendError::Unauthorized => "The Ryoku user service rejected this user.",
        BackendError::Busy => "The Ryoku user service is busy; try again later.",
        BackendError::Unavailable => "The local Ryoku user service is unavailable.",
        BackendError::RecoveryUnavailable => {
            "The local profile could not be verified; inspect profile state before retrying."
        }
        _ => "The local profile could not be read.",
    }
}

fn set_failure_message(error: BackendError) -> &'static str {
    match error {
        BackendError::Unauthorized => "The Ryoku user service rejected this user.",
        BackendError::Busy => "The Ryoku user service is busy; try again later.",
        BackendError::Unavailable => "The local Ryoku user service is unavailable.",
        BackendError::UnsupportedOperation => {
            "This local profile cannot be edited by the generic profile writer."
        }
        BackendError::StaleRequest => {
            "Profile state changed while saving; read it again before retrying."
        }
        BackendError::Failed => {
            "The profile save may have completed; read the profile before retrying."
        }
        BackendError::RecoveryUnavailable => {
            "The local profile could not be verified; inspect profile state before retrying."
        }
        _ => "The local profile could not be saved.",
    }
}

fn run() -> Result<(), String> {
    use rustix::process::{getegid, geteuid, getgid, getuid};
    validate_identity(getuid().as_raw(), geteuid().as_raw(), getgid().as_raw(), getegid().as_raw())
        .map_err(str::to_owned)?;
    let action = parse(&env::args().skip(1).collect::<Vec<_>>()).map_err(str::to_owned)?;
    let service = UnixServiceBackend::from_environment().map_err(|error| {
        match action {
            Action::ShowLocal => show_failure_message(error),
            Action::SetLocal(_) => set_failure_message(error),
            Action::MigrateLegacy => failure_message(error),
        }
        .to_owned()
    })?;
    match action {
        Action::ShowLocal => {
            let profile = service
                .get_local_profile(RequestId(std::process::id() as u64))
                .map_err(|error| show_failure_message(error).to_owned())?;
            let serialized = serde_json::to_string_pretty(&profile)
                .map_err(|_| "The local profile could not be formatted.".to_owned())?;
            println!("{serialized}");
        }
        Action::SetLocal(settings) => {
            let profile = service
                .set_local_profile(RequestId(std::process::id() as u64), settings)
                .map_err(|error| set_failure_message(error).to_owned())?;
            let serialized = serde_json::to_string_pretty(&profile)
                .map_err(|_| "The local profile could not be formatted.".to_owned())?;
            println!("{serialized}");
        }
        Action::MigrateLegacy => {
            match service.migrate_legacy_local_profile(RequestId(std::process::id() as u64)) {
                Ok(_) => println!(
                    "Legacy Ryoku preferences were copied. The original profile remains unchanged."
                ),
                Err(error) => return Err(failure_message(error).into()),
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ryoku-profile: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn accepts_show_set_and_explicit_legacy_migration() {
        assert_eq!(parse(&args(&["show"])), Ok(Action::ShowLocal));
        assert_eq!(
            parse(&args(&["set", r#"{"desktop":"kde","theme":"system","density":null}"#])),
            Ok(Action::SetLocal(ryoku_profiles::SettingsLayer {
                desktop: Some(ryoku_profiles::Desktop::Kde),
                theme: Some(ryoku_profiles::Theme::System),
                density: None,
            }))
        );
        assert_eq!(parse(&args(&["migrate-legacy"])), Ok(Action::MigrateLegacy));
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["set", "{}", "extra"])).is_err());
        assert!(parse(&args(&["set", r#"{"desktop":"../../outside"}"#])).is_err());
        assert!(parse(&args(&["set", r#"{"commands":["sh"]}"#])).is_err());
        assert!(
            parse(&args(&["set", &" ".repeat(ryoku_profiles::MAX_PROFILE_DOCUMENT_BYTES + 1)]))
                .is_err()
        );
        assert!(parse(&args(&["migrate-legacy", "--force"])).is_err());
        assert!(parse(&args(&["import", "/tmp/profile.json"])).is_err());
    }

    #[test]
    fn read_errors_do_not_claim_profile_contents_or_remote_access() {
        assert_eq!(
            show_failure_message(BackendError::Unavailable),
            "The local Ryoku user service is unavailable."
        );
        assert_eq!(
            show_failure_message(BackendError::RecoveryUnavailable),
            "The local profile could not be verified; inspect profile state before retrying."
        );
    }

    #[test]
    fn refuses_root_or_mismatched_identity() {
        assert!(validate_identity(0, 0, 0, 0).is_err());
        assert!(validate_identity(1000, 0, 1000, 1000).is_err());
        assert!(validate_identity(1000, 1000, 1000, 1001).is_err());
        assert!(validate_identity(1000, 1000, 1000, 1000).is_ok());
    }

    #[test]
    fn uncertain_migration_outcomes_do_not_claim_that_nothing_was_written() {
        for error in [BackendError::Failed, BackendError::StaleRequest] {
            let message = failure_message(error);
            assert!(message.contains("inspect local profile state"));
            assert!(!message.contains("no files were changed"));
        }
    }

    #[test]
    fn unsupported_migration_reports_that_the_source_was_not_recognized() {
        let message = failure_message(BackendError::UnsupportedOperation);
        assert!(message.contains("not a recognized legacy preferences file"));
        assert!(message.contains("no files were changed"));
    }
}
