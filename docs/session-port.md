# Experimental native session (#25)

The optional `ryoku-session` RPM adds an SDDM entry named **Ryoku (openSUSE
experimental)**. The current session release 3 requires at least the issue #173
`ryoku-desktop-0.1.0-8` candidate for upstream Ryoku Amaterasu 1.0.0, pinned at
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. The openSUSE RPM version is independent
of upstream's version. Native desktop acceptance remains open. Select KDE
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
which requires the shell unit and the existing Rust service. The latter
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
are used. Startup checks the required Ryoku Ui, FrameBars, PluginKit, Blobs,
Spatial3D and Hyprland QML module metadata before launching a surface. The
activation command performs the same check before starting the target, so a
missing module enters its failure cleanup and greeter exit immediately. The
daemon selects the packaged Hyprland provider through a private Rust session
alias; ordinary provider argv still enter the existing writer coordinator. The separate
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
selected. The same ordered logout handles the original shell's
`session.logout` → Hyprland `act session.exit` path. A fixed
`ryoku-session-logout.service` runs outside the desktop target and shell cgroup;
it stops the desktop and graphical targets while Wayland is available, then
exits the compositor. Both the requester and worker require the inherited
Wayland display and Hyprland instance signature so `hyprctl` cannot select
another login after environment cleanup. Native UI/keyboard logout regression
remains pending. Short lifecycle clients now have fixed deadlines: target stop
20 seconds, target start 20 seconds, and D-Bus/environment/health/exit clients
3 seconds each. Timeout kills the newly owned client process group and allows
one second for reaping before continuing cleanup. The logout path reserves a
33-second worst-case client/reap budget within its 45-second unit limit, so a
stalled stop still reaches both environment cleanups and the compositor-exit
attempt. The compositor and lock retain their full running lifetimes.

Activation checks each required shell/service unit independently after the target
start. A grouped `systemctl is-active` call succeeds when any listed unit is
active, so it cannot prove both partners started. A failed or timed-out check
retains the startup failure and follows the same stop → environment cleanup →
compositor-exit path. The [offline partner-readiness receipt](../infra/tumbleweed-lab/evidence/issue25-session-partners-20261025.toml)
records the one-active/one-failed regressions. These checks establish unit-state
control flow, not shell rendering, socket readiness, ordinary login or reboot.

The [cross-file source contract receipt](../infra/tumbleweed-lab/evidence/issue25-session-contract-20261025.toml)
binds the packaged DM entrypoint, exact required target services, independent
Rust readiness commands and lifecycle ownership. It also checks that optional
providers cannot add ordered boot work or automatic enablement and that the
session package preserves KDE's independent desktop entry. The current full
workspace/all-features and strict Clippy checks pass. This parses trusted
metadata; it does not start a service manager or accept native login/reboot/KDE.

The guarded lab now has separate read-only installed-session inspections:
`vm-lab guest-ryoku-login-inspect` checks the ordinary fixture user's native
SDDM/PAM Hyprland login, RPM-owned session metadata, independently ready target
partners, exact process images/starts/cgroups, imported environment and the
Hyprland-owned Wayland socket. It rechecks observations before publishing static
results. The shell daemon's inherited environment is inspected because Hyprland
creates its display variables after exec; its initial `/proc` environment need
not contain them. The Quickshell alias and actual executable are checked against
their separate runtime and Quickshell RPM owners.

`vm-lab guest-ryoku-fallback-inspect` checks return to the native greeter, no
remaining fixture login or Ryoku process (including stopped players), stopped
target partners and cleared imported graphical environment. If logind has
already terminated the user manager, it instead verifies the exact inactive
manager and absent runtime endpoints without starting it. The existing
`guest-login-inspect` remains the separate Plasma runtime inspection. A selectable
KDE entry alone does not establish KDE usability, and a greeter inspection alone
does not establish which preceding failure/logout caused it.
Fallback checks also include the independent lock/logout units without adding
either to the login readiness gate. A reported inactive/failed unit must still
use its installed fragment, and any reported cgroup must be absent or report
`populated 0` through a bounded no-follow read of `cgroup.events`; an empty
direct `cgroup.procs` file is not enough to rule out descendant tasks. These
checks repeat after the unit snapshot is revalidated. The
[lifecycle/cgroup receipt](../infra/tumbleweed-lab/evidence/issue25-lifecycle-cgroup-20261025.toml)
records synthetic filesystem tests; no actual service or kernel cgroup was
changed, and portal/device/native fallback acceptance remains separate.
The process-image scan also recognizes procfs's exact ` (deleted)` suffix:
unlinking a still-running or stopped player's executable cannot turn it into
completed cleanup. The [deleted-image regression receipt](../infra/tumbleweed-lab/evidence/issue25-deleted-actor-inspection-20261025.toml)
records an owned disposable Rust-test-image unlink/stop/kill/reap test and unrelated
image exclusions. This is an inspector test, not installed-player or native
logout acceptance.

These commands neither install packages nor start/stop services, change PAM,
enter credentials, use QMP or reboot. They require the existing explicitly
seeded QEMU fixture, enforcing SELinux, fixture root, recorded ordinary account,
RPM-intact native SDDM/PAM and no autologin. Their unit/parser tests and public
evidence restrictions are recorded in the
[inspection receipt](../infra/tumbleweed-lab/evidence/issue25-ryoku-inspection-20261025.toml).
No native inspection was executed for that receipt. Before use, the coordinator
must bind the complete current #173 candidate and approve the exact lab guard,
storage and install plan. Visible frames, real C-player display, login after
reboot, KDE application use, device workflows and #176 theme accounting remain
separate acceptance gates.

The [offline deadline receipt](../infra/tumbleweed-lab/evidence/issue25-session-deadlines-20261025.toml)
records Rust subprocess tests for each failure/timeout, missing payload,
owned-descendant termination and unrelated-process isolation. These are host
control-flow tests. Native acceptance must inspect the user manager's target
jobs, portal shutdown, actual return to SDDM and subsequent KDE login after a
client deadline; killing the client group is not evidence those services stopped.
The supported experimental fixture has one graphical login per user;
concurrent logins share a user manager and are not accepted.

The four shortcut navigation requests (launcher, quick settings, clipboard and
notifications) also use the existing three-second owned-client deadline. A
stalled Quickshell IPC client and its descendants are terminated/reaped without
signalling the already running shell server. Success and nonzero exit status
still propagate; fixed positional IPC arguments and payload validation remain.
The compositor, lock/unlock, interactive screenshot, shell/Hub daemon and
clipboard-keeper lifetimes are unchanged. The
[navigation receipt](../infra/tumbleweed-lab/evidence/issue25-navigation-deadlines-20261025.toml)
records all four success/failure/stall cases and actual three-second client-tree
cleanup with an independent server stand-in. These subprocess tests are not
native Quickshell frames, cold rendering or session acceptance.

## Recovery and authentication limits

The native lock adapter now rejects missing or empty inherited Wayland/Hyprland
identities before invoking the same `systemctl --user start --wait` request.
Its unit also asserts both manager identities are present and nonempty. An
unmet assertion fails the queued start job; the former condition could skip it
without that failure. Assertions do not necessarily mark the unit failed.
See [systemd's condition/assert contract](https://www.freedesktop.org/software/systemd/man/257/systemd.unit.html#Conditions%20and%20Asserts).
The [lock precondition receipt](../infra/tumbleweed-lab/evidence/issue25-lock-preconditions-20261025.toml)
records client/result regressions and read-only native expression evaluation.
The distro locker, PAM files and complete lock/unlock lifetime remain unchanged;
these checks provide no visible-lock, password, suspend or crash-security result.

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

The current combined candidate also needs two #25-owned native package builds:
`ryoku-session` (declared NEVR `ryoku-session-0.1.0-3`) and its exact private
`ryoku-session-clipboard-keeper-0.5.0-1` dependency. The keeper is pinned upstream
Rust `wl-clip-persist` 0.5.0 with the recorded parser/I/O port; its independent
version does not change the Ryoku 1.0 source identity. Both source/vendor
archives are already cached and hash-verified in this worktree's ignored
`target/issue25-native-packet/SOURCES`, so no network fetch is needed.
The [native package prerequisite receipt](../infra/tumbleweed-lab/evidence/issue25-native-package-prerequisites-20261025.toml)
records exact spec/archive/lock/patch hashes and declared NEVRs. No current
keeper native RPM exists and no build/install is claimed. After #173 releases
the shared builder and the coordinator rechecks storage, build the keeper
(including its `%check`) and session sequentially, offline with BuildRequires
enabled, using the exact combined Source0/vendor/lock for the session. Record
actual native RPM/SRPM identities/signatures rather than relabeling historical
session 0.1.0-1 artifacts. Install remains a separate approved combined-target
phase; preserve the retained shared guest while #173 collects its evidence.

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

## Ryoku Amaterasu 1.0 session review

The pinned source's `ryoku/shell/quickshell/shell/services/SessionActions.qml`
sends logout through `call session.logout`. Its
`ryoku/shell/ipc/session.go` handler asks the selected WM provider for
`session.exit`; `ryoku/wm/hyprland/act.go` directly dispatches compositor exit.
That route would bypass the port's portal shutdown ordering. The session's
private `ryoku-wm-hyprland` alias now intercepts exactly `act session.exit` and
queues the independent logout unit. Extra arguments are refused; every other
literal provider argument, including non-UTF-8 values, stays on the existing
Rust writer path. This session-only adapter leaves the pinned source and the
#173 export/patch unchanged.

The upstream greeter, greeter guard, `sddm/setup`, Wayland wrapper, qylock and
Bluetooth agent/link are not session dependencies. No optional Voxtype ensure
command is added to the port's shell unit. Distro SDDM/PAM/password/autologin
policy, the independent KDE entry and native hyprlock remain the acceptance
targets. The [functional checklist](native-desktop-workflows.md) requires fresh
1.0 password login/reboot, UI and keyboard logout, controlled startup failure,
KDE fallback and current lock/unlock evidence. Existing 0.75.5 receipts remain
historical. Suspend/resume continuity, empty/stalled PAM and locked-crash claims
remain unproven.

The [offline 1.0 receipt](../infra/tumbleweed-lab/evidence/issue25-v1-session-offline-20261024.toml)
records the reviewed source blobs, session dependency, 14 passing focused Rust
tests, strict Clippy, formatting and RPM spec parsing. No VM was used and no
native session build/install or desktop result is claimed by that receipt.
