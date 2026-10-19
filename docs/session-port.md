# Experimental native Ryoku session

Tracks [issue 25](https://github.com/GenieRobot/ryoku-opensuse/issues/25).
The Rust `ryoku-session` executable starts the actual preserved upstream
Quickshell shell and Hub. This is an experimental disposable-VM integration,
not completion evidence for the desktop, reboot or recovery gates.

## Reviewed startup boundary

The source pin is `98a0368253d9628be72b853985878a5db90e1547`.
The original `ryoku/hyprland/modules/autostart.lua` invokes materialization,
keyring initialization, power/sysfs restoration, lighting, GPU persistence,
Limine snapshot recovery and the upstream shell/lock daemon. Running it would
also select upstream Arch-oriented components. None of these commands is part
of this session bootstrap.

The new native Hyprlang file is isolated at
`/usr/share/ryoku-opensuse/session/hyprland.conf`. Explicit non-Lua paths select
the legacy parser in the reviewed
[Hyprland v0.56.2 ConfigManager.cpp](https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/config/ConfigManager.cpp).
No standalone Lua or QML file is authored. The approved small QML routing/result
exception is documented in [`upstream/patches`](../upstream/patches/README.md).
The default compositor configuration, existing
home configuration and KDE session entry are not replaced. Plugins remain off.

The SDDM entry runs `ryoku-session compositor`; Hyprland's `exec-once` calls
`systemd-cat --identifier=ryoku-session /usr/bin/ryoku-session activate` after
its first render callback. The native journal wrapper preserves activation
errors that Hyprland otherwise redirects to `/dev/null`. The Rust activator
imports only the named graphical environment into the user manager and D-Bus,
then starts `ryoku-desktop-session.target`. That target starts the existing
Rust service and a separate upstream daemon unit. The daemon supervises the
original Quickshell shell and supplies ordinary settings and clipboard topics.
The target also wants the distro Hyprland Polkit agent's native user unit for
system authorization dialogs. This generic agent is separate from the Ryoku
helper, does not journal operations, and does not make unsupported Ryoku
controls safe. Compositor exit stops this Ryoku target. It does not stop KDE's
shared graphical-session target.
The shell is required and ordered before the target, with `Type=exec` so an
executable startup failure fails target activation. The independently owned
Rust backend is wanted but is not stopped on desktop logout. Its native state
and helper fixture must be prepared separately; starting this target does not
initialize or repair backend state.
Activation directly starts the target without `reset-failed`. Native testing
confirmed that resetting an unloaded shell unit returns an error in a fresh
user manager and prevented the initial shell start. The unit's existing
restart/burst limits remain enforced; activation does not clear failures to
bypass them.
If activation fails after entering the compositor hook, it stops the Ryoku
target, clears the named manager and D-Bus environment, and requests Hyprland
exit so the display manager can recover instead of leaving a blank compositor.
The command refuses to run this cleanup when invoked without a compositor
instance signature. Host fixture tests cover cleanup ordering and make sure a
cleanup failure does not suppress the exit request; guest lifecycle behavior
still needs native validation.
`HYPRLAND_NO_SD_VARS=1` disables the importer and cleanup in Hyprland's
`Compositor.cpp`. However, the pinned
[Executor.cpp](https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/config/supplementary/executor/Executor.cpp)
contains a second first-frame importer that ignores this flag whenever
Aquamarine has a session. Startup environment ownership is therefore not
exclusive; those fixed upstream imports may race the Rust activator. Do not
claim that this flag fully isolates a nested compositor if it acquires a
libseat session. This remains a native lifecycle compatibility limitation.
On normal compositor exit, Rust stops the shell, empties the fixed graphical
keys in D-Bus (whose API cannot unset keys), and unsets them in systemd. It
attempts both environment cleanups even if one fails. Forced termination of
the session wrapper still needs logout/reboot validation; this is not a
claim of cleanup after SIGKILL or a crashed user manager.
Do not run this full session launcher inside an existing graphical session:
its manager/D-Bus environment import belongs to a separate login session.

## Payload and native prerequisites

Issue 14 owns exporting the unchanged upstream runtime payload and compiling
the upstream native Blobs plugin. The agreed payload layout is:

```text
/usr/share/ryoku-opensuse/upstream/
  LICENSE
  NOTICE
  shell/quickshell/shell/shell.qml
  hub/quickshell/shell.qml
  qml/Ryoku/{Ui,FrameBars,PluginKit,Blobs}/qmldir
  i18n/
```

Original relative assets and applicable license notices must accompany these
trees. The launcher requires the entrypoint and module manifests before it
executes the fixed `/usr/libexec/ryoku/quickshell/bin/qs`; plugin loading and
runtime imports still require VM validation.
It sets both QML import path variables to the two absolute directories `qml`
and `shell/quickshell` below this root, `RYOKU_SHELL_DIR` to its `shell`, and
`RYOKU_I18N_DIR` to its `i18n/catalog`.
For a staged payload, use `ryoku-session shell --payload-root /absolute/root`
or `ryoku-session hub --payload-root /absolute/root` in the graphical session.

The retained fixture's snapshot 20261013 package inventory verifies `hyprland`
0.56.2-1.2, the initially considered `noctalia-qs` 0.0.12-2.5, `qt6-wayland`,
Mesa/EGL/DRI, `xdg-desktop-portal`, `xdg-desktop-portal-hyprland`, and
`xdg-desktop-portal-gtk`. Their exact Qt import/plugin closure is tracked by
issue 14; availability is not API compatibility. The native test rejected
Noctalia 0.0.12 because it does not recognize the preserved `DefaultEnv`
pragma. It is not the runtime used by this launcher. Keep the independently
installed SDDM/Plasma session, PipeWire and WirePlumber. The baseline shortcuts
use the existing KDE `konsole` and `dolphin` executables. The native launcher
also requires `/usr/bin/setpriv`, `/usr/bin/systemctl`, `/usr/bin/systemd-cat` and
`/usr/bin/dbus-update-activation-environment`; confirm file providers in the
guest before installing the session. No arbitrary repositories are required.

The full session binds `Super+L` to the distro Hyprlock binary and passes an
explicit packaged configuration at
`/usr/share/ryoku-opensuse/session/hyprlock.conf`. The configuration uses a
solid background rather than capturing the unlocked desktop, and Hyprlock's
default PAM authenticator remains enabled. It does not edit `/etc/pam.d` or
replace any system authentication policy. This only supplies the lock path;
successful lock acquisition, PAM unlock, compositor failure, suspend/resume,
and recovery handoff still need native validation. Its syntax follows the
[Hyprlock 0.9.6 example](https://github.com/hyprwm/hyprlock/blob/v0.9.6/assets/example.conf)
and the [Hyprlock authentication/configuration reference](https://wiki.hypr.land/Hypr-Ecosystem/hyprlock/).

The Ryoku session target also starts the distro Hypridle 0.1.8 user service
with the fixed `/usr/share/ryoku-opensuse/session/hypridle.conf`. The service
is stopped with the target and has a bounded restart policy. After five idle
minutes and before logind sleep it requests `loginctl lock-session`; Hypridle routes that
request to the same Hyprlock command used by logind. The lock command checks for
an existing Hyprlock process before starting another. After resume, Hypridle
turns the display back on. This declarative policy has not yet been exercised
in the guest, so idle detection, authentication, and suspend/resume behavior
remain unverified. The configuration follows the
[Hypridle reference](https://wiki.hypr.land/0.56.0/Hypr-Ecosystem/hypridle/).

The rechecked OSS `repomd.xml` SHA-256 is
`c64c293f246a439f0b5e1881fe8adfc980ca44420a84935f31e223949810daa4`,
matching the fixture definition. The exact primary records confirm
`libaquamarine14` 0.15.1-1.1, `util-linux` 2.42.4-1.1 with
`/usr/bin/setpriv`, and `dbus-1` 1.14.10-5.7 requiring `dbus-1-tools`.
This refreshes the older 20261011 inventory for the pending desktop experiment.

The first native SDDM frame also exposed the missing `hyprland-guiutils`
dependency. The same snapshot provides `hyprland-guiutils` 0.2.1-1.6 x86_64,
RPM SHA-512
`d1e0301c959336a07007ddb4a34feca1828779f7db4c6f3ec724d45dea8f15ff09d13e97d667c25318e04f267c65acbeaf723dc6daf5ee8348c2b83af28e71b9`.
It owns `hyprland-dialog`, the executable checked by Hyprland, and the native
run/welcome/update/donate dialogs. It requires `libhyprtoolkit.so.6` and
`libhyprutils.so.13` and conflicts with its predecessor `hyprland-qtutils`.
Use the signed snapshot solver for that closure; availability does not imply
it was installed in the first successful frame. Do not suppress the warning.

The fixture explicitly installs `hyprland-guiutils` and `breeze6-wallpapers`
even with recommendations disabled, and reports their installed RPM versions.
The latter supplies Plasma's default `Next` wallpaper. In the retained guest,
the native look-and-feel requested `Next`, but both the package and the entire
`/usr/share/wallpapers` directory were absent. Authenticated KDE login and a
rendered Konsole window therefore did not establish a usable fallback desktop:
the desktop, panel and launcher captures were black.

The captured native layout contained panel applets but none of the geometry
fields which [Plasma 6.7.5's serializer](https://github.com/KDE/plasma-workspace/blob/v6.7.5/shell/shellcorona.cpp)
emits for an existing PanelView. That source also defers panel creation until
the desktop containment is ready. The [wallpaper fallback](https://github.com/KDE/plasma-workspace/blob/v6.7.5/wallpapers/image/plugin/utils/mediaproxy.cpp)
returns immediately when its default package is invalid, while
[containment readiness](https://github.com/KDE/libplasma/blob/v6.7.5/src/plasmaquick/plasmoid/containmentitem.cpp)
waits for wallpaper loading to finish. This establishes a concrete dependency
gap consistent with the observed missing panel. A fresh ordinary KDE login
after installing the native dependencies rendered the Next wallpaper, Home
and Trash icons, and full bottom panel. The retained guest also passed a real
reboot followed by ordinary Ryoku SDDM login and a reviewed shell capture.
The [separate reboot/fallback evidence](../infra/tumbleweed-lab/evidence/issue25-native-reboot-kde-fallback.toml)
records exact local artifact hashes and preserves the earlier black-desktop
failure. No renderer override or saved-layout reset was used. This proves
the staged session gates; RPM first-install behavior, watchdog recovery and
the remaining full backend integration still require their own validation.

The replacement runtime is unchanged official Quickshell 0.3.2 at commit
[`4f508be500dea6e5732cc3d50382a0048b17e7b1`](https://github.com/quickshell-mirror/quickshell/commit/4f508be500dea6e5732cc3d50382a0048b17e7b1),
built against the guest's native Qt SDK. Its reviewed source supports
`DefaultEnv`; successful full-shell rendering remains a VM gate. Its isolated
CMake prefix is `/usr/libexec/ryoku/quickshell`: the source installs
`bin/quickshell` and the `bin/qs` symlink. The Rust launcher uses this fixed
absolute path for surfaces and IPC and prepends its `bin` directory only to
the child surface's inherited PATH for upstream launches of `qs` by name.
The RPM-owned `/usr/bin/qs` is not replaced. There is no client-supplied runtime
executable option. Issue 14 owns source provenance and build evidence.
Earlier native prototype evidence used `/usr/local/ryoku-quickshell`; that
historical evidence is unchanged. The packaged private prefix needs a fresh
launch check after installation.

The launcher has an explicit `compositor --software-rendering` diagnostic
option, setting Mesa's `LIBGL_ALWAYS_SOFTWARE=1`. It does not assert that the VM
DRM backend, renderer or native SDDM login works. Keep this override out of the
generic session until actual VM evidence identifies a need.

### Nested visual proof inside KDE

`ryoku-session nested` requires an existing `WAYLAND_DISPLAY` and launches a
separate native Hyprland window using `hyprland-nested.conf`. Its `exec-once`
directly launches the NNP-protected preserved shell. The Rust launcher and
configuration do not call the activator, start the Ryoku target, replace a
portal or register a polkit agent. The upstream Executor exception above
prevents claiming complete user-manager isolation. The parent's D-Bus address and
Wayland socket are inherited only by the child process. The nested compositor
then supplies its own socket to its shell child. Full SDDM session acceptance
is still required after this intermediate visible proof.

This path was checked against the exact snapshot's `libaquamarine14`
0.15.1-1.1: [Aquamarine v0.15.1 DRM.cpp](https://github.com/hyprwm/aquamarine/blob/v0.15.1/src/backend/drm/DRM.cpp)
filters DRM devices using the explicit `AQ_DRM_DEVICES` list. An empty list
selects none. [Hyprland v0.56.2 Compositor.cpp](https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/Compositor.cpp)
registers the Wayland backend as its fallback and honors
`HYPRLAND_NO_SD_VARS=1` for its main activation-environment updates (the
Executor exception above still applies).
The nested command sets both variables. Aquamarine still attempts session
discovery before filtering devices; no DRM scanout backend is selected.
The existing KDE session keeps ownership of its display.

`nested --software-rendering` sets `LIBGL_ALWAYS_SOFTWARE=true` in the child,
as documented by [Mesa](https://docs.mesa3d.org/envvars.html#libgl-environment-variables).
No wlroots-only renderer variables, guessed Aquamarine backend variables or
fake display capabilities are used. Actual Wayland fallback, nested renderer,
window size and shell IPC still need the retained-VM test.

### Native watchdog launch review

Hyprland 0.56.2 warns on direct launch without its watchdog. The reviewed
[native launcher](https://github.com/hyprwm/Hyprland/blob/v0.56.2/start/src/main.cpp)
accepts `--path /usr/bin/Hyprland --no-nixgl -- --config <isolated.conf>`:
arguments after `--` reach Hyprland, and the fixed path avoids PATH selection.
It inherits the Rust child's environment and performs no PAM authentication,
account creation or session selection; SDDM remains responsible for those.
`--no-nixgl` avoids its optional Nix probing/wrapper path on this native RPM
installation. This is a reviewed candidate command, not the implemented path.

The [instance implementation](https://github.com/hyprwm/Hyprland/blob/v0.56.2/start/src/core/Instance.cpp)
adds a watchdog pipe, waits for the compositor, forwards TERM/INT through a
graceful termination path, and sets a parent-death SIGKILL on its child.
After an initialized compositor crashes, the launcher automatically starts a
new instance with `--safe-mode` and, if applicable, `--locked`. The
[config selector](https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/config/supplementary/jeremy/Jeremy.cpp)
prioritizes generated `recoverycfg.lua` over the explicit Ryoku config in safe
mode. Thus substituting the watchdog command alone would leave the Rust parent
waiting while the previous shell target/environment still belongs to the dead
instance and the recovery instance lacks Ryoku's reviewed activation command.
The wrapper also returns zero for some initialization failures, so its exit
status cannot replace actual compositor/shell evidence.

Before adopting this wrapper, the Rust lifecycle must observe each compositor
instance ending, stop that instance's surfaces and clear its graphical keys,
then explicitly distinguish upstream recovery from an active Ryoku session.
Normal logout, failed initialization, crash/recovery and locked crash each
need native tests. No fake watchdog descriptor or warning suppression is
acceptable. The proven direct launch remains an experimental path with its
warning visible. Legacy `.conf` also remains supported-but-deprecated; the
Rust-only constraint rules out authoring a replacement executable Lua config.

The follow-up review found no compatible recovery override in this exact
wrapper: its supported switches are `--path`, `--no-nixgl`, `--force-nixgl`
and forwarded compositor arguments. Both `--config` and `HYPRLAND_CONFIG`
lose to the compositor's safe-mode selection. An owned `--path` Rust adapter
could provide a cleanup boundary around each child, but cannot make native
safe mode honor the explicit Ryoku config through these interfaces. Removing
`--safe-mode` would silently replace upstream's crash policy with another
normal startup. Removing `--locked` would discard a security-relevant state
and is not an acceptable adaptation. The native lock path forces a session
lock; recovery cannot assume that an unported locker will take over.

There are two concrete future contracts. An upstream-supported recovery-config
override could retain native safe-mode/lock semantics while selecting a
reviewed Ryoku recovery configuration. Alternatively, a deliberately specified
fail-closed policy could terminate the whole owned graphical session on an
unclean compositor exit and return control to SDDM. The latter is logout,
not in-session crash recovery. It needs an owned per-instance Rust supervisor,
verified propagation of termination and parent death to the actual compositor,
cleanup before any new instance, bounded failure handling, and error reporting
independent of the wrapper's zero exit status. No restart may clear a prior
locked state into an unlocked desktop. Neither contract is implemented or
validated here; merely swapping the executable would claim an unsupported
lifecycle. Keep the native wrapper adoption and locked-crash validation open
when assessing issue 25, alongside the deprecated configuration migration.

## Core surface routing and current limits

The source-backed [core workflow checklist and action-boundary decision](native-desktop-workflows.md)
separate implemented behavior, missing providers and controls whose direct
writes cannot be mediated by an external Rust executable. `Super+N` now routes
to the original notification center (`quick-settings#notifications`); receiving,
viewing and dismissing real notifications uses upstream's native notification
server. Its native acceptance procedure is recorded there and remains pending.
The adjacent DND setting remains an ordinary native user control under the
user-authorized scope adjustment in that document; its persistent state belongs
in recovery inventory without requiring a machine checkpoint per toggle.

`Super+Space`, `Super+Escape` and `Super+V` use a typed Rust selection of the
actual upstream `shell.qml` `IpcHandler` method:
`/usr/libexec/ryoku/quickshell/bin/qs -p <shell-directory> ipc call shell openSurface "" <surface>`.
The allowed surface IDs are `launcher`, `quick-settings` and `clipboard`.
Empty monitor uses upstream's active/first-screen fallback. No fake daemon
capability frame is published. Direct Hyprland `global` binds would be inert
because upstream `CustomShortcut.qml` requires daemon-provided capabilities.
The overview remains unavailable until its real compositor capability bridge
is present. `Super+,` starts the authentic Hub; `Super+Shift+Escape` exits
Hyprland and returns to the display manager.

Both surface commands execute the native
[`setpriv --no-new-privs`](https://github.com/util-linux/util-linux/blob/v2.41.2/sys-utils/setpriv.1.adoc)
before `qs`. The shell user unit also sets `NoNewPrivileges=yes`. This prevents
child execution from acquiring set-ID/file-capability privileges, including
unported `pkexec`/`sudo` privilege escalation. It is **not** a sandbox against
home writes, existing permissions or policy-authorized D-Bus actions. It also
applies to applications launched as children of the shell.
Every launcher action also rejects root or mismatched real/effective user or
group IDs. NNP alone would not remove privileges from an existing root process.

Unchanged Hub code contains operations outside the accepted Rust contract:
`LockscreenPage.qml` builds a privileged Bash PAM edit; `ConnectionsPage.qml`
invokes privileged Bluetooth service changes; update text still mentions
pacman. Shell and Hub settings also write configuration directly. There is no
reviewed upstream global switch that redirects all these operations through
the typed service. Thus Hub writes and package, power, authentication, lock,
network, Bluetooth, screenshot or screen-share workflows are not certified.
Clipboard now has a separate Rust user service and documented original-surface
route, but its native UI and recovery behavior are unverified. Do not present
any unverified control as a supported integrated operation.
Missing daemons/helpers must remain visibly failed/unavailable, not simulated
successful responses. The preserved source currently permits a genuine
surface/launcher prototype; full write integration requires a reviewed protocol
extension or upstream change under the project's Rust-only constraints.

## Installation and evidence checklist

Only the coordinator installs into the approved disposable guest. Build the
Rust binary with `cargo build --locked -p ryoku-session`. Install that binary
as `/usr/bin/ryoku-session`, the `.desktop` file under
`/usr/share/wayland-sessions/`, the desktop user units under
`/usr/lib/systemd/user/`, the pinned upstream daemon under
`/usr/libexec/ryoku/`, and the isolated compositor file at the path above.
Install the verified issue 14 payload separately. Preserve existing paths and
user overrides; production RPM ownership and signatures remain a separate gate.
Run native `systemd-analyze --user verify` on the installed units and reload the user
manager before login. Never globally enable the shell unit: only this session
target owns it.
For the nested proof also install `hyprland-nested.conf` beside the login
configuration; invoke `nested` from the real desktop user's Wayland environment.

Required evidence remains open:

- Confirm native `Hyprland --version`, parser acceptance,
  `/usr/libexec/ryoku/quickshell/bin/qs --version`, Qt
  modules and the Blobs plugin from installed signed-package receipts.
- Capture the actual visible Ryoku bar, open launcher and Hub, and retain
  runtime logs. A process, GTK test window or splash screen is insufficient.
- Demonstrate launcher invocation and an actual application window. Record
  unavailable providers without fabricated capabilities.
- Log into Ryoku through SDDM, log out to KDE, reboot and repeat both logins.
- Verify notifications, clipboard, audio, network, Bluetooth, screenshots,
  screen sharing, authentication and locking individually, identifying each
  simulated, untested or physically tested device path.
- Resolve unsupported writes through authenticated typed Rust operations with
  linked pre-change recovery before calling them supported.

Local Rust checks alone verify argument rejection and missing-payload failures;
they provide no graphical or reboot acceptance evidence.

### Native DRM preflight

The fixture uses `-vga none -device virtio-vga` without a virgl or OpenGL display
option (`automation/src/lab/host.rs`). A guest render node is necessary evidence,
not proof of working scanout. The exact
[Aquamarine 0.15.1 DRM source](https://github.com/hyprwm/aquamarine/blob/v0.15.1/src/backend/drm/DRM.cpp)
requires an active libseat session, a KMS GPU, PRIME import, CRTC vblank events,
monotonic timestamps and universal planes. The exact
[Hyprland 0.56.2 renderer](https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/render/OpenGL.cpp)
initializes EGL using a matching device or GBM and requires GLES 3.2 or its 3.0
fallback and BGRA8888 texture support. It has no explicit llvmpipe rejection;
missing DMA-BUF extensions produce a warning. Do not add a wlroots renderer
override or claim hardware acceleration from the presence of `renderD128`.

Before changing sessions, stage both configurations and units and run native
`Hyprland --verify-config --config /usr/share/ryoku-opensuse/session/hyprland.conf`
as the ordinary user. Check the private runtime's version, native file
providers, unchanged Plasma session entry, initialized backend and user-unit
verification. A parser check does not open a DRM session. The next proof must
be ordinary-password SDDM login with a connected Wayland socket, the native
Hyprland executable, an active shell unit, actual bar/launcher screenshots and
the compositor log's renderer/DRM details. Then exit, verify shell/target stop
and cleared manager keys, log into KDE, reboot, and repeat both logins. The
existing Plasma-specific lab inspection must not be reported as Hyprland
acceptance.

The cold-start regression gate requires a fresh ordinary SDDM login, with no
prior `show`, manual shell activation or manual target startup used to load
the shell unit. Record automatic `ryoku-session` journal entries, target and
shell state, actual shell process and visible frame. A manual diagnostic
activation in an already failed session cannot satisfy this gate. Local Rust
tests do not emulate systemd's unit-loading semantics; the native cold-start
test is the authoritative check for this regression.

### Retained guest login procedure

The lab provides a typed, opt-in host command, built with `--features wsl-lab`:

```text
vm-lab development-login /absolute/owned/run --session ryoku
vm-lab development-capture /absolute/owned/run
vm-lab development-login /absolute/owned/run --session kde
```

Before each login, log out normally and wait until the fixture user has no
registered sessions. This command deliberately refuses to terminate an active
login. Stage the matching new `vm-lab` binary at the guest's existing
`/tmp/ryoku-vm-lab` path first. The host requires the live retained development
record, owned QEMU peer, matching loopback SSH forwarding and pinned existing
host key. It generates a fresh password in memory, sends it to the guarded
guest preparation helper on stdin, then types it into the rendered native
SDDM greeter via the owned QMP connection. Session selection writes only
SDDM's last-session preference; it does not enable autologin. The existing
account and original Plasma fixture record remain unchanged. Unknown session
names, active logins and changed fixture identities fail closed.

The Ryoku inspector checks the ordinary user's active SDDM/Wayland/Hyprland
login, intact RPM-owned compositor, running NNP shell unit, fixed private
Quickshell executable with the preserved shell path, process ownership and
connected user Wayland socket. KDE uses the existing Plasma inspector. The
timestamped `development-login-*-private.log` contains bounded inspection
output, never the password. Login markers explicitly require separate visual
acceptance; capture and inspect the actual bar, launcher and launched app.
After recording logout cleanup and KDE fallback, the coordinator reboots the
same owned guest through native `systemctl reboot`, waits for SSH/SDDM, then
repeats both login commands and captures. This helper neither reboots nor
claims the reboot, graphics or full workflow gates by itself.
