# Preserved Ryoku terminal launch adapter

This focused part of [issue 25](https://github.com/GenieRobot/ryoku-opensuse/issues/25)
adds the Rust `ryoku-app` binary in the existing session crate. It supports only:

```text
ryoku-app terminal
ryoku-app terminal -- <program> [arg...]
```

At the pinned Ryoku revision, `shell/services/AppLaunch.qml` constructs the
second form for XDG entries with `Terminal=true`. The preserved upstream
`ryoku/shell/scripts/ryoku-app` documents both forms. Other roles, terminal
preference strings and configuration writes are not implemented; unsupported
forms print an error and return failure. Konsole is the prototype terminal,
provided by the exact 20261013 snapshot's `konsole` 26.08.2-1.1 package at
`/usr/bin/konsole`.

Every invocation checks that the caller is non-root and has matching real and
effective user/group IDs. It then executes native `setpriv --no-new-privs`
before Konsole. `--separate` keeps the new terminal in that process lineage
instead of delegating to a pre-existing Konsole over D-Bus. `--workdir` and
the process working directory both receive the caller's actual directory as
literal arguments. The same five Quickshell crash variables removed by the
upstream `Spawn.qml` are also removed here.

For a program, the native argv is `konsole --separate --workdir <directory>
-e /usr/bin/env -- <program> [arg...]`. This deliberately avoids a behavior in
[Konsole 26.08.2 Application.cpp](https://github.com/KDE/konsole/blob/v26.08.2/src/Application.cpp):
when `-e` has a single argument that is not an executable, Konsole reparses it
as a shell command line. The fixed `env` executable and separator preserve the
literal argument vector, including spaces, empty arguments and metacharacters.
The program name itself cannot contain `=` or equal `-`: GNU `env` treats those
as assignment/environment-control operands even after `--`, so the adapter
rejects them rather than silently changing execution semantics. Later arguments
may contain `=` normally.
No shell command string is generated or evaluated by this adapter. An explicitly
requested shell executable remains an ordinary terminal program.

There is an upstream limitation: the pinned `AppLaunch.qml` supplies the XDG
entry's `workingDirectory` to `Spawn.run` for ordinary GUI apps, but omits it
for the terminal branch. The adapter preserves the actual inherited directory;
it cannot reconstruct a discarded desktop entry's `Path=`. No QML is changed
and this limitation is not presented as fixed.

Build with `cargo build --locked -p ryoku-session --bin ryoku-app`. The
coordinator may install the reviewed binary as `/usr/bin/ryoku-app` in the
approved disposable guest; do not install the upstream Bash script in that
location. No host deployment or VM action is performed by this change.
Tests check malformed/unsupported forms, literal arguments, process lineage
options, crash environment removal and identity rejection. Actual terminal
and TUI rendering require the retained desktop VM and remain separate evidence.
