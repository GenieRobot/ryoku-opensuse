//! Native desktop defaults and the pinned AppLaunch terminal argv contract.
//! Custom Hub role preference selection remains owned by issue #180.
use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, ExitCode, Stdio},
    time::Duration,
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

fn parse_terminal(args: &[OsString]) -> Result<Terminal> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("terminal")) {
        return Err("expected the terminal role".into());
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum DesktopRole {
    Browser,
    Files,
}

impl DesktopRole {
    fn mime(self) -> &'static str {
        match self {
            Self::Browser => "x-scheme-handler/http",
            Self::Files => "inode/directory",
        }
    }
}

#[derive(Debug, PartialEq)]
enum Launch {
    Terminal(Terminal),
    Desktop(DesktopRole, Vec<OsString>),
}

fn parse(args: &[OsString]) -> Result<Launch> {
    let role = match args.first().and_then(|arg| arg.to_str()) {
        Some("terminal") => return parse_terminal(args).map(Launch::Terminal),
        Some("browser") => DesktopRole::Browser,
        Some("files") => DesktopRole::Files,
        _ => return Err("usage: ryoku-app terminal [-- PROGRAM [ARG...]] | browser|files [-- TARGET...] (additional custom roles are not available yet)".into()),
    };
    let operands = if args.get(1).is_some_and(|arg| arg == "--") { &args[2..] } else { &args[1..] };
    if operands.iter().any(|arg| arg.is_empty()) {
        return Err("browser/file targets must not be empty".into());
    }
    Ok(Launch::Desktop(role, operands.to_vec()))
}

fn desktop_id(output: &[u8]) -> Result<&str> {
    let invalid = "No valid desktop handler: select a native default browser or file manager in KDE System Settings > Default Applications";
    let value = std::str::from_utf8(output).map_err(|_| invalid)?;
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value.len() > 4095
        || !value.ends_with(".desktop")
        || value == ".desktop"
        || value.starts_with('-')
        || value
            .chars()
            .any(|c| c.is_control() || (c.is_whitespace() && c != ' ') || c == '/' || c == '\\')
    {
        return Err(invalid.into());
    }
    Ok(value)
}

fn native_command(program: &str, directory: &Path) -> Result<Command> {
    if !directory.is_absolute() {
        return Err("application working directory must be absolute".into());
    }
    let mut command = Command::new("/usr/bin/setpriv");
    command.args(["--no-new-privs", program]).current_dir(directory).stdin(Stdio::null());
    for name in CRASH_VARIABLES {
        command.env_remove(name);
    }
    Ok(command)
}

fn desktop_using(
    role: DesktopRole,
    operands: &[OsString],
    directory: &Path,
    query: &mut impl FnMut(Command, Duration) -> Result<Vec<u8>>,
    launch: &mut impl FnMut(Command, Duration) -> Result<()>,
) -> Result<()> {
    let mut command = native_command("/usr/bin/xdg-mime", directory)?;
    command.args(["query", "default", role.mime()]);
    let output = query(command, Duration::from_secs(3))?;
    let id = desktop_id(&output)?;
    let mut command = native_command("/usr/bin/gtk-launch", directory)?;
    // GLib owns desktop-file/DBus dispatch and GFile filename/URI conversion.
    // -- ends GTK's option parser before all user-controlled operands.
    command.arg("--").arg(id).args(operands);
    // gtk-launch can await a D-Bus startup ping; the bound covers the short
    // launcher handshake, never the successfully launched GUI lifetime.
    launch(command, Duration::from_secs(30))
}

fn validate_identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<()> {
    if uid == 0 || uid != euid || gid != egid {
        return Err(
            "application launch requires a non-root user with matching real/effective IDs".into()
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
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let directory = env::current_dir()?;
    match parse(&args)? {
        Launch::Terminal(terminal) => Err(terminal_command(terminal, &directory)?.exec().into()),
        Launch::Desktop(role, operands) => desktop_using(
            role,
            &operands,
            &directory,
            &mut ryoku_session::client::capture_control,
            &mut ryoku_session::client::checked_control,
        ),
    }
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
            assert!(parse_terminal(&input).is_err(), "unexpectedly accepted {input:?}");
        }
        assert_eq!(parse_terminal(&args(&["terminal"])).unwrap(), Terminal::DefaultShell);
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
        let command = terminal_command(parse_terminal(&supplied).unwrap(), directory).unwrap();
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
    fn desktop_roles_use_native_defaults_and_literal_targets_without_exec_parsing() {
        use std::os::unix::ffi::OsStringExt;
        let mut operands = args(&[
            "https://example.invalid/a%20b?q=$(literal)&x='quote'",
            "./a file#name%",
            "--help",
        ]);
        operands.push(OsString::from_vec(b"./non-utf8-\xff".to_vec()));
        for role in [DesktopRole::Browser, DesktopRole::Files] {
            let selected = if role == DesktopRole::Browser {
                "Example Browser.desktop"
            } else {
                "org.example.Native.desktop"
            };
            let mut queries = 0;
            let mut launches = 0;
            desktop_using(
                role,
                &operands,
                Path::new("/tmp"),
                &mut |command, timeout| {
                    queries += 1;
                    assert_eq!(timeout, Duration::from_secs(3));
                    assert_eq!(command.get_program(), "/usr/bin/setpriv");
                    assert_eq!(
                        command.get_args().collect::<Vec<_>>(),
                        args(&[
                            "--no-new-privs",
                            "/usr/bin/xdg-mime",
                            "query",
                            "default",
                            role.mime()
                        ])
                    );
                    Ok(format!("{selected}\n").into_bytes())
                },
                &mut |command, timeout| {
                    launches += 1;
                    assert_eq!(timeout, Duration::from_secs(30));
                    assert_eq!(command.get_program(), "/usr/bin/setpriv");
                    let mut expected =
                        args(&["--no-new-privs", "/usr/bin/gtk-launch", "--", selected]);
                    expected.extend_from_slice(&operands);
                    assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
                    assert_eq!(command.get_current_dir(), Some(Path::new("/tmp")));
                    let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
                    for name in CRASH_VARIABLES {
                        assert_eq!(environment[OsStr::new(name)], None);
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!((queries, launches), (1, 1));
        }
        for role in ["browser", "files"] {
            let expected_role =
                if role == "browser" { DesktopRole::Browser } else { DesktopRole::Files };
            assert_eq!(parse(&args(&[role])).unwrap(), Launch::Desktop(expected_role, vec![]));
            assert_eq!(
                parse(&args(&[role, "--", "a path"])).unwrap(),
                Launch::Desktop(expected_role, args(&["a path"]))
            );
            assert_eq!(
                parse(&args(&[role, "a path"])).unwrap(),
                Launch::Desktop(expected_role, args(&["a path"]))
            );
            assert!(parse(&args(&[role, ""])).is_err());
        }
        assert!(parse(&args(&["editor"])).unwrap_err().to_string().contains("custom roles"));
    }

    #[test]
    fn invalid_missing_and_failed_handlers_never_dispatch_and_launch_errors_propagate() {
        let mut invalid = vec![
            b"".to_vec(),
            b".desktop".to_vec(),
            b"/tmp/app.desktop".to_vec(),
            b"a\\b.desktop".to_vec(),
            b"a.desktop\nb.desktop\n".to_vec(),
            b"-option.desktop".to_vec(),
            b"a\tb.desktop".to_vec(),
            b"\xff.desktop".to_vec(),
        ];
        invalid.push([vec![b'a'; 4096], b".desktop".to_vec()].concat());
        for output in invalid {
            let result = desktop_using(
                DesktopRole::Browser,
                &[],
                Path::new("/tmp"),
                &mut |_, _| Ok(output.clone()),
                &mut |_, _| panic!("invalid handler launched"),
            );
            assert!(result.unwrap_err().to_string().contains("Default Applications"));
        }
        assert!(
            desktop_using(
                DesktopRole::Files,
                &[],
                Path::new("/tmp"),
                &mut |_, _| Err("query failed".into()),
                &mut |_, _| panic!("failed query launched")
            )
            .is_err()
        );
        for error in ["missing desktop handler", "launch failed", "launch deadline"] {
            let result = desktop_using(
                DesktopRole::Files,
                &[],
                Path::new("/tmp"),
                &mut |_, _| Ok(b"org.example.Native.desktop".to_vec()),
                &mut |_, _| Err(error.into()),
            );
            assert_eq!(result.unwrap_err().to_string(), error);
        }
        let id = format!("{}.desktop", "a".repeat(4095 - 8));
        assert_eq!(desktop_id(id.as_bytes()).unwrap(), id);
        let spec = include_str!("../../../../packaging/rpm/ryoku-session.spec");
        for dependency in ["/usr/bin/xdg-mime", "/usr/bin/gtk-launch"] {
            assert!(spec.lines().any(|line| {
                line.strip_prefix("Requires:").is_some_and(|value| value.trim() == dependency)
            }));
        }
    }

    #[test]
    fn packaged_terminal_and_files_shortcuts_enter_the_supported_role_contract() {
        let config = include_str!("../../../../packaging/rpm/session/hyprland.conf");
        for (key, role) in [("Return", "terminal"), ("E", "files")] {
            let prefix = format!("bind = SUPER, {key}, exec, ");
            let bindings =
                config.lines().filter_map(|line| line.strip_prefix(&prefix)).collect::<Vec<_>>();
            assert_eq!(bindings, [format!("/usr/bin/ryoku-app {role}")]);
            match parse(&args(&[role])).unwrap() {
                Launch::Terminal(Terminal::DefaultShell) if role == "terminal" => {}
                Launch::Desktop(DesktopRole::Files, operands)
                    if role == "files" && operands.is_empty() => {}
                other => panic!(
                    "packaged shortcut does not enter its supported no-argument role: {other:?}"
                ),
            }
        }
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
