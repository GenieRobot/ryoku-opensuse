//! Narrow compatibility with the pinned Ryoku AppLaunch terminal argv contract.
//! This does not implement upstream role preferences or system administration.
use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, ExitCode},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CRASH_VARIABLES: &[&str] = &[
    "__QUICKSHELL_CRASH_INFO_FD",
    "__QUICKSHELL_CRASH_DUMP_PID",
    "__QUICKSHELL_CRASH_DUMP_FD",
    "__QUICKSHELL_CRASH_LOG_FD",
    "__QUICKSHELL_CRASH_SIGNAL",
];

#[derive(Debug, PartialEq)]
enum Terminal {
    DefaultShell,
    Program(Vec<OsString>),
}

fn parse(args: &[OsString]) -> Result<Terminal> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("terminal")) {
        return Err("only the terminal role is supported on this openSUSE prototype".into());
    }
    match &args[1..] {
        [] => Ok(Terminal::DefaultShell),
        [separator, program, ..] if separator == "--" && !program.is_empty() => {
            // GNU env still recognizes NAME=VALUE and the lone '-' operand
            // after --. Neither is an executable form this adapter supports.
            if program.as_encoded_bytes().contains(&b'=') || program == "-" {
                return Err("terminal program must not contain '=' or be '-'".into());
            }
            Ok(Terminal::Program(args[2..].to_vec()))
        }
        _ => Err("usage: ryoku-app terminal [-- <program> [arg...]]".into()),
    }
}

fn validate_identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<()> {
    if uid == 0 || uid != euid || gid != egid {
        return Err(
            "terminal launch requires a non-root user with matching real/effective IDs".into()
        );
    }
    Ok(())
}

fn terminal_command(terminal: Terminal, working_directory: &Path) -> Result<Command> {
    if !working_directory.is_absolute() {
        return Err("terminal working directory must be absolute".into());
    }
    let mut command = Command::new("/usr/bin/setpriv");
    command
        .args(["--no-new-privs", "/usr/bin/konsole", "--separate", "--workdir"])
        .arg(working_directory)
        .current_dir(working_directory);
    for name in CRASH_VARIABLES {
        command.env_remove(name);
    }
    if let Terminal::Program(argv) = terminal {
        // Konsole reparses one missing executable argument as a shell command
        // line. The fixed env executable plus -- removes that ambiguity while
        // preserving all supplied program arguments, including empty arguments.
        command.args(["-e", "/usr/bin/env", "--"]).args(argv);
    }
    Ok(command)
}

fn run() -> Result<()> {
    use rustix::process::{getegid, geteuid, getgid, getuid};
    validate_identity(
        getuid().as_raw(),
        geteuid().as_raw(),
        getgid().as_raw(),
        getegid().as_raw(),
    )?;
    let terminal = parse(&env::args_os().skip(1).collect::<Vec<_>>())?;
    let mut command = terminal_command(terminal, &env::current_dir()?)?;
    Err(command.exec().into())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ryoku-app: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn rejects_roles_and_malformed_forms_without_claiming_support() {
        for input in [
            args(&[]),
            args(&["files"]),
            args(&["browser", "--", "firefox"]),
            args(&["terminal", "--"]),
            args(&["terminal", "--", ""]),
            args(&["terminal", "--", "NAME=value", "/usr/bin/true"]),
            args(&["terminal", "--", "-"]),
            args(&["terminal", "-e", "top"]),
            args(&["terminal", "top"]),
        ] {
            assert!(parse(&input).is_err(), "unexpectedly accepted {input:?}");
        }
        assert_eq!(parse(&args(&["terminal"])).unwrap(), Terminal::DefaultShell);
    }

    #[test]
    fn terminal_preserves_literal_program_arguments_and_working_directory() {
        let supplied = args(&[
            "terminal",
            "--",
            "a program",
            "one argument",
            "",
            "$(touch /tmp/wrong)",
            "; echo wrong",
            "--profile",
            "a'b\"c",
            "ordinary=argument",
            "--",
        ]);
        let directory = Path::new("/tmp/a directory; $(literal)");
        let command = terminal_command(parse(&supplied).unwrap(), directory).unwrap();
        assert_eq!(command.get_program(), "/usr/bin/setpriv");
        let mut expected = args(&[
            "--no-new-privs",
            "/usr/bin/konsole",
            "--separate",
            "--workdir",
            "/tmp/a directory; $(literal)",
            "-e",
            "/usr/bin/env",
            "--",
        ]);
        expected.extend_from_slice(&supplied[2..]);
        assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        assert_eq!(command.get_current_dir(), Some(directory));
        let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
        for name in CRASH_VARIABLES {
            assert_eq!(environment[OsStr::new(name)], None);
        }
    }

    #[test]
    fn plain_terminal_uses_konsole_default_shell_without_a_shell_command_string() {
        let command = terminal_command(Terminal::DefaultShell, Path::new("/tmp")).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            args(&["--no-new-privs", "/usr/bin/konsole", "--separate", "--workdir", "/tmp"])
        );
        assert!(terminal_command(Terminal::DefaultShell, Path::new("relative")).is_err());
    }

    #[test]
    fn refuses_root_and_mismatched_real_effective_ids() {
        for ids in
            [(0, 0, 0, 0), (1000, 0, 1000, 1000), (1000, 1001, 1000, 1000), (1000, 1000, 1000, 0)]
        {
            assert!(validate_identity(ids.0, ids.1, ids.2, ids.3).is_err());
        }
        assert!(validate_identity(1000, 1000, 100, 100).is_ok());
    }
}
