# Experimental native session (#25)

The optional `ryoku-session` RPM adds an SDDM entry named **Ryoku (openSUSE
experimental)**. It depends on the accepted issue #23 release-5 package set;
native desktop acceptance remains open. Select KDE
normally while those gates are incomplete. Installation changes no SDDM
default, enables no units, writes no home files and replaces no KDE entry.

## Startup and shutdown

SDDM authenticates the ordinary user and invokes `ryoku-session compositor`.
The Rust parent starts the native `/usr/bin/Hyprland` with the immutable
`/usr/share/ryoku-opensuse/session/hyprland.conf`, leaving user compositor files
untouched. Hyprland's `exec-once` calls the Rust `activate` command through
`systemd-cat`, retaining failures in the `ryoku-session` journal identifier.

Activation requires `WAYLAND_DISPLAY` and `HYPRLAND_INSTANCE_SIGNATURE`, clears
previous graphical keys, and imports only the declared graphical environment
into D-Bus and the user manager. It starts `ryoku-desktop-session.target`,
which requires the shell unit and wants the existing Rust service. The latter
continues to use the authenticated typed service contract and fixed socket;
no new privileged service or authorization policy is introduced. A drop-in
associates it with session shutdown. No user unit is globally enabled.
The session target binds to the native `graphical-session.target`, allowing
native portal activation; its stop-when-unneeded lifecycle releases that target
after this single graphical session stops. The explicit Hyprland portal package
and this target binding still need the next native validation pass.

The shell unit invokes `ryoku-session shell`, which runs
`setpriv --no-new-privs /usr/libexec/ryoku/ryoku-writer daemon daemon` with fixed
packaged paths. The Rust writer coordinator from issue #18 owns the unchanged
pinned daemon and its surface descendants. The private Quickshell runtime,
`/usr/share/ryoku` payload, native `/usr/lib64/qt6/qml` modules and original Hub
are used. The daemon selects the packaged Hyprland provider through its existing
Rust wrapper alias. No fake capability or success frame is added. The separate
clipboard daemon from the earlier #25 prototype is not started: the packaged
upstream daemon already owns that socket and clipboard topic.

`Super+Space`, `Super+Escape`, `Super+V` and `Super+N` navigate the original
launcher, quick settings, clipboard and notifications through the existing
shell IPC. `Super+,` asks the wrapped daemon to open the original Hub. The
terminal-only `ryoku-app` compatibility command uses native Konsole with literal
argv. Other application roles return an error. The optional launcher Discover
provider remains the existing `ryoku-tools-launcher-status` package; this RPM
does not rewrite the user's launcher configuration.
`Print` enters the existing coordinated Ryoshot writer through the Rust session
command; its native capture/error/cancel behavior remains pending.

Normal compositor exit stops the target, reaps its shell cgroup, stops the Rust
service and clears the declared graphical keys from the user manager. D-Bus
activation keys are emptied because that API cannot remove keys. KDE's target,
units and session entry are not modified. Shell restart is rate limited, and
shutdown has a 15-second bound. `Super+Shift+Escape` exits to SDDM so KDE can be
selected. The supported experimental fixture has one graphical login per user;
concurrent logins share a user manager and are not accepted.

## Recovery and authentication limits

Writer registration coordinates capture of the daemon's writers. It does not
create a linked checkpoint, authorize an action or certify upstream mutation
controls. Issue #1 still requires verified linked pre-change capture for every
supported integrated write, with capture failure preventing mutation. This
experimental session adds no supported protected writes and makes no atomic
recovery claim. Native configuration/Hub controls, appearance materialization,
package/service changes, PAM edits and locking are not certified by startup.
The declared inventory and existing capture rules remain unchanged.

`NoNewPrivileges` prevents executable set-ID or file-capability elevation. It
does not sandbox D-Bus, home writes, PipeWire or Wayland. Retained upstream
controls can still perform ordinary user writes outside the accepted protected
operation path; use this candidate only in the disposable fixture. The inherited
`RYOKU_POLKIT_AGENT` flag is forced off, so this candidate does not register the
unvalidated upstream polkit/PAM agent. No lock or suspend readiness is asserted.
Do not use an unported lock command as a security boundary.

Direct Hyprland startup retains the native warning about its watchdog. The
wrapper's safe-mode restart can select a different recovery configuration and
locked state. Adopting it requires per-instance cleanup and native normal-exit,
initialization-failure, crash and locked-crash evidence. This change neither
suppresses the warning nor claims watchdog recovery. Hyprlang configuration is
supported but deprecated in the inspected Hyprland 0.56.2; migration remains open.
The pinned compositor's secondary first-frame environment importer also limits
exclusive environment ownership claims despite `HYPRLAND_NO_SD_VARS=1`.

## Evidence and remaining acceptance

The preserved [first SDDM receipt](../infra/tumbleweed-lab/evidence/issue25-native-sddm-first-pass.toml)
and [reboot/KDE receipt](../infra/tumbleweed-lab/evidence/issue25-native-reboot-kde-fallback.toml)
record historical manually staged source and artifact hashes. They establish
that the older prototype rendered after native password login and reboot, and
that KDE worked after missing wallpaper dependencies were installed. They do
not validate this packaged daemon-based candidate, exact current binaries,
full workflows or failure recovery. The earlier black KDE desktop remains
recorded as a failed visual result.

Run the [functional checklist](native-desktop-workflows.md) against one recorded
supported fixture before closing #25. Required session evidence is:

1. Build/install this spec with native dependencies enabled and record the RPM,
   source, lockfile and installed executable/unit hashes.
2. Run native `systemd-analyze --user verify` on the target and services. Verify
   the independent Plasma entry remains RPM-owned and selectable.
3. Cold-login through SDDM with no prior manual target/shell activation. Record
   `journalctl --user -b -t ryoku-session`, target/shell/service state, daemon
   and surface identities, connected Wayland socket, and actual bar, launcher,
   Hub and launched-application frames. Review/sanitize logs before publishing.
4. Log out; verify target and units stop, descendants are reaped and graphical
   environment keys are cleared. Log into KDE and test its panel and launcher.
5. Reboot the same fixture and repeat both logins and the functional checklist.
6. Exercise a controlled Ryoku startup failure and demonstrate an ordinary KDE
   login. Test compositor/crash cleanup separately; normal logout is not proof
   of locked-crash handling.

A real `rpmbuild -ba` attempt with the committed source and unchanged accepted
vendor closure stopped before `%prep`: the host RPM database cannot satisfy
`cargo >= 1.99` and `rust >= 1.99`. The available Rustup 1.99 toolchain does
not satisfy RPM package dependencies. Dependency checks were not bypassed and
no native binary RPM was produced. The [build log](../infra/tumbleweed-lab/evidence/issue25-session-rpm-host-build.log)
records the exact failure. A native builder satisfying these BuildRequires is
still required.

A separate, explicitly non-acceptance host mechanics run verified that the
already installed Rust/Cargo 1.99.0 binaries meet those version minima, then
used `rpmbuild -ba --nodeps` in an isolated temporary topdir. It reused the exact
same source/vendor archives and passed prep, offline vendored compilation,
install and manifest checks. The unsigned RPM contains only the two executables,
units/drop-in, immutable config/session entry, documentation and license, with
no scriptlets or home/state paths. The [mechanics log](../infra/tumbleweed-lab/evidence/issue25-session-rpm-mechanics.log)
and [manifest](../infra/tumbleweed-lab/evidence/issue25-session-rpm-mechanics-files.tsv)
are separate from the preserved original failure. Bypassing RPMDB dependency
resolution for this inspection does not satisfy the native BuildRequires,
Tumbleweed ABI, signed-install or desktop acceptance gates.

No new QEMU was launched during this consolidation. Native acceptance could not
be repeated because there was no live authorized local guest. Local Rust,
spec parsing and unit checks cannot replace the required desktop evidence.
The [consolidation receipt](../infra/tumbleweed-lab/evidence/issue25-session-consolidation-20261021.toml) records exact host commands, results and private-log digests.
Keep issue #25 open.

## Native continuation, 22 October 2026

The [new native receipt](../infra/tumbleweed-lab/evidence/issue25-native-20261022.toml)
resolves the earlier host RPMDB blocker: the inherited native SDK snapshot has
RPM-installed Rust/Cargo 1.99, and ordinary-user BuildRequires-enabled session
builds passed with offline vendored compilation. A fresh issue25 COW overlay
installed separate signed fixture copies of the accepted package set and session
through native Zypper dependency/signature/file checks. It is an inherited Btrfs
desktop fixture, not a clean-install target.

The first password SDDM login reached Hyprland but failed to start the old packaged
writer across the native runtime mount. Rebuilding the unchanged current desktop
spec with main's existing correction produced a working writer. A second native
password login rendered the genuine shell/bar/calendar with coordinated daemon,
provider and clipboard descendants. The [reviewed frame](../infra/tumbleweed-lab/evidence/issue25-native-20261022-shell.png)
proves that visible surface only. The old helper package still makes the separate
Rust service fail its namespace check, and portal activation reported an inactive
graphical target. Current helper rebuilding, portal/screenshot metadata changes,
all workflows and reboot/KDE validation remain pending.

The Rust activation command now requests compositor exit after failed startup
cleanup, returning users to SDDM instead of an empty compositor. This committed
binary was built and installed natively; its intentional-failure path still needs
native exercise. The tested native session source is `422c322`, with exact RPM
and installed hashes in the receipt; subsequent portal/screenshot changes have
host checks only. The owned guest was orderly powered off to release the shared
KVM slot for #22. Its overlay and private logs are preserved; the original
backing disk and firmware digests are unchanged. Continue native validation on
this owned overlay after the slot becomes available. Issue #25 remains open.

## Resumed native checkpoint and pending host candidate

The [resumed #25 receipt](../infra/tumbleweed-lab/evidence/issue25-native-resume-20261022.toml)
records actual current-helper startup and password Ryoku re-entry after reboot
at `7b0d2ef`, with graphical and portal units active. Native Konsole launch,
Hub, a synthetic notification and a monitor-capture file were observed. Normal
KDE password fallback rendered its panel. Full workflow acceptance remains open.

Direct compositor exit exposed a native Hyprland portal SIGSEGV. The new logout
route first stops the desktop and graphical targets while Wayland is available,
then exits the compositor. Required Rust service readiness is checked alongside
the shell. Both corrections require another native regression.

The private `ryoku-shell` executable alias forwards original native argv through
the existing daemon writer, except the exact `lock` action routes to a fixed
distro hyprlock user unit. The lock unit uses the distro PAM policy unchanged;
it supplies no fabricated secure-lock acknowledgement or suspend handoff.

The signed fixture repositories lack `wl-clip-persist`. Its separately pinned
Rust private RPM, parser/I/O dependency port and locked closure are documented
in [clipboard-build.md](../packaging/rpm/clipboard-build.md). The session bounds
regular selections to 10 MiB, disables timestamp manipulation, and limits logs
to warnings. Original daemon history remains the only history owner. These
new lock and clipboard paths are host candidates until actual native acceptance.
