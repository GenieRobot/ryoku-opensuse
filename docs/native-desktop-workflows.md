# Native desktop workflows and the remaining action boundary

Issue [25](https://github.com/GenieRobot/ryoku-opensuse/issues/25), following
[ADR 0001](adr/0001-ui-and-service-integration.md). This records implementation
and source inspection. A separate opt-in trial of the pinned upstream user
daemon has limited native evidence in
[`issue25-upstream-ordinary-trial-oct18.toml`](../infra/tumbleweed-lab/evidence/issue25-upstream-ordinary-trial-oct18.toml);
the current desktop RPM builds and packages this pinned backend by default.
Theme and ordinary-preference behavior still needs regression through the
installed RPM-owned binary. The session/reboot/KDE results in
[session-port.md](session-port.md) retain their existing scope.

Inspected input: Ryoku `98a0368253d9628be72b853985878a5db90e1547`, preserved
runtime archive SHA-256
`81151cef1a613b01ad8e6187df2e93cbfed70e1621de0ee2e7455d2904fc3e74`.
Paths below are relative to that archive unless linked to the upstream tree.
The original runtime archive remains unchanged. The RPM now applies one
routing-only patch to `Capture.qml`; screenshot execution and result handling
are in the Rust adapter. The Go daemon was exercised in the separately
recorded native trial and is integrated into the desktop RPM build and user
unit; workflow regression through the installed RPM-owned binary remains
pending. Its exposed controls and privileged
operations are not thereby accepted for release.

## User-authorized recovery scope adjustment

The original roadmap #1 says: “Every supported integrated system or Ryoku
change requires a verified linked pre-change restore point.” The repository's
project-scope.md and provider-capabilities.md repeat that broad requirement;
they did not already exempt ordinary personal settings. The user's subsequent
instruction explicitly distinguishes ordinary user settings from privileged
operations and says not to block all settings on Rust protection. That newer
instruction narrows the earlier per-interaction gate as follows. It does not
remove recovery coverage of persistent user configuration.

| Operation class | Working path and recovery requirement |
| --- | --- |
| Transient navigation, notifications, application events | Retain native upstream behavior; no machine checkpoint or elevation |
| Ordinary per-user/session controls, including DND, volume/mute, personal appearance/launcher preferences | Preserve working native controls and ordinary user ownership; do not require a machine checkpoint or root for each interaction |
| Persistent user configuration | Keep in the declared recovery inventory and linked restore generations, while allowing ordinary edits; generation capture and per-interaction blocking are different policies |
| Privileged packages, system services, PAM/security and systemwide configuration | Keep operation-specific authorization and verified linked pre-change capture before supported integrated mutation |
| Mixed controls such as network, Bluetooth and recording | Classify the actual operation and affected state; ordinary native interaction is not automatically a privileged system change. Separate it from service enablement, shared configuration or protected security actions |
| Missing native/provider implementation | Report unavailable honestly; missing implementation is not a reason to disable unrelated working controls |

The earlier blanket managed-write assessment below is narrowed by this
classification. Native Flags/PipeWire controls are retained. Only actions
actually within the protected class need a capture-gated Rust route.

## Implemented notification workflow

`Super+N`, or `ryoku-session open notifications`, opens the original notification
history through the existing `shell.openSurface` IPC method. The Rust route
passes the literal `quick-settings#notifications` identifier, matching the
upstream notification rail. An empty monitor uses upstream's active-output
fallback; with the missing `wm` topic this can fall back to the first screen.
The existing private Quickshell path, payload validation, ordinary-user checks
and no-new-privileges launch remain enforced. Native IPC failure returns a
nonzero exit; a zero IPC result alone does not prove visible content.

The actual receive/model/popup/history implementation is already in
[`Notifs.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/quickshell/shell/services/Notifs.qml):
it instantiates `Quickshell.Services.Notifications.NotificationServer`, tracks
real freedesktop notifications, and dismisses them through `Notification.dismiss`.
It does not subscribe to the missing shell daemon. Navigation matches
[`RailStatus.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/quickshell/shell/modules/bar/framebars/widgets/RailStatus.qml#L99)
and the `notifications` initial-page handler in `MenuQuickSettings.qml`.

Receiving an application event, presenting its in-memory history, opening a
surface and dismissing that event do not request a managed system/configuration
change. Persistent settings still belong in recovery inventory.
The adjacent DND button changes `Flags.dnd`, whose [pinned `Flags.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/quickshell/shell/services/Flags.qml)
writes `$XDG_STATE_HOME/ryoku/flags.json`. The pinned ordinary settings
backend writes appearance and shell preferences to
`$XDG_CONFIG_HOME/ryoku/shell.json` and palette-generation preferences to
`$XDG_CONFIG_HOME/ryoku/matugen.json`. The pinned shell/Hub QML also persists
theme, brand, launcher, widget, performance, recording, and visualizer settings
in their own JSON files; `Capture.qml` stores screenshot delay/save/beautify
choices in `$XDG_STATE_HOME/ryoku/capture.json`, Camera.qml stores the webcam
overlay layout in `camera.json`, Stage stores global appearance/motion
settings in `stage.json`, and Ryoshot stores editor preferences in
`ryoshot-beautify.json`; the drawing tool stores brush, zoom, copy, and save
preferences in `ryoshot.json`. The equalizer stores its user curve, preset, and
enabled preference in `equalizer.json`; the shell refreshes the live graph state
from the audio provider on startup. The recording overlay stores only its theme,
display mode, and optional placement in `keypresses.json`; raw events and typed
text remain ephemeral. `Recorder.qml` stores its desktop-audio, microphone, and
edit-after-recording preferences in `$XDG_STATE_HOME/ryoku/record.json`;
`Devices.qml` stores the screen vibrance percentage in
`$XDG_STATE_HOME/ryoku/nvibrant-value`. The pinned shell also persists user-selected backlight level, touchpad-disabled intent, and night-light enabled state, temperature, and schedule in five fixed files under `$XDG_STATE_HOME`; these are now exact bounded session-preference records. Per-wall Stage effect and layer arrangements are stored separately in the bounded `stage-walls.json` registry; generated images, progress logs, and referenced source assets are not traversed. The pinned power-profile provider stores
the selected profile in `power-profile.json` and user CPU/battery definitions
and apply delay in `power.json`. Both are captured as bounded opaque preferences;
capture does not authorize applying hardware settings. `InstalledInventory::session_preferences()` declares
these exact preference files, the local profile selection, and only the three
state-directory metadata records needed for the declared state preferences; it never captures
other XDG state, generated cache files, plugin-defined settings, scripts,
histories, or referenced user assets. Recovery fixtures stage and restore the core shell/matugen settings, flags, and
per-wall Stage registry through separate configuration and home-root descriptors
while preserving an unrelated home sentinel.
Ordinary toggles still do not require a machine snapshot. Native DND behavior
and the installed linked-generation/console workflow remain unverified.
Arbitrary application notification actions are a separate acceptance case; no
privileged action is inferred from receiving a notification.

## Rust clipboard history provider

The current desktop RPM source builds and installs the Rust provider, applies the pinned
socket/error-display patch to the existing clipboard surface, and starts the
provider with the Ryoku session target. This selects one clipboard selection
watcher: the RPM also patches upstream daemon bootstrap to skip its clipboard
history startup hook. The source and package wiring are implemented; native
installed-session behavior and clipboard round-trip acceptance remain pending.

The Rust `ryoku-clipboard` service watches Wayland selection changes with
`wl-paste`, owns a bounded history, and serves the original clipboard surface
over the dedicated `$XDG_RUNTIME_DIR/ryoku-clipboard.sock` endpoint. This is
separate from upstream's shared `ryoku-shell.sock`, so it does not implement or
intercept other shell topics. The upstream QML patch only selects that socket
and displays provider/operation errors; capture, copy-back,
delete, clear, starring, persistence, MIME validation and data integrity are
handled in Rust.

History is capped at 100 entries and 10 MiB per payload. Content and index files
are stored under `$XDG_STATE_HOME/ryoku/clipboard` (default
`~/.local/state/ryoku/clipboard`) with mode `0600` inside mode `0700`
directories; cache files use the matching XDG cache subtree. Clipboard history
can contain credentials and other sensitive content. It is private to the
desktop UID but is not encrypted. The recovery policy is to exclude clipboard
contents from linked generations: recovery captures Ryoku configuration and
profiles, not this sensitive operational history. Clipboard history therefore
has no linked backup or restore guarantee; clearing it through the UI remains
supported. Recovery must not recursively copy the XDG state tree to include
clipboard data. Image entries are size and dimension bounded, but the current
preview file is the original image data rather than a downscaled thumbnail.

The persisted index retains the pinned Go provider's `nextID`, `lastPrune`,
entry fields, numeric FNV-1a `hash`, nanosecond `ts`, and `%016x.bin` content
filenames. Rust also writes a `sha256` field for integrity checking; Go's JSON
decoder ignores that additional field. The migration test reads a Go-format
fixture and confirms the rewritten index remains in the shared format.

The implementation has unit coverage for migration of the pinned upstream
history format, MIME and payload limits, integrity checks, persistence,
copy-back payload/MIME delivery and failure ordering, star/delete/clear
behavior, content-addressed file handling, and preservation of unrelated files.
The Release-10 source selects the Rust provider for the original UI path, but
that source has not yet produced a complete RPM or passed native VM validation.
The Oct 18 upstream-daemon evidence belongs to the previous Release-2 path and
does not establish the Rust provider's behavior.

### Native receive, history and dismiss evidence

After the package-owned shell survived a native reboot and ordinary SDDM login,
the actual Quickshell process received a synthetic freedesktop notification,
showed it in the original history via `Super+N`, and dismissed the card through
its original UI. The monitor recorded `NotificationClosed` for notification ID
1 with reason 2. Sanitized screenshot hashes and the scope of this run are in
[`the Oct 18 evidence record`](../infra/tumbleweed-lab/evidence/issue25-notifications-oct18.toml).
This does not establish DND suppression/restoration, arbitrary application
actions, or notification behavior under KDE fallback.

### Native receive, history and dismiss reproduction procedure

Run only in the coordinator's retained ordinary-user Ryoku SDDM session after
the corresponding binary/configuration installation has passed the existing
pre-change capture gate. Commands here are native tools, not new automation.
Keep the original shell/source/package receipts and the journal for the run.

1. Record `systemctl --user show ryoku-desktop-shell.service -p MainPID -p ActiveState`
   and the active executable and payload path. Query the existing bus owner
   without requesting activation:

   ```console
   busctl --user --timeout=5 call -- org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus GetNameOwner s org.freedesktop.Notifications
   busctl --user status org.freedesktop.Notifications
   busctl --user --timeout=5 call -- org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications GetServerInformation
   ```

   The owner must be this shell's Quickshell process, not Plasma or a competing
   daemon. If no owner exists, or another application owns it, stop this test
   and record the actual registration/lifecycle failure. Do not start another
   notification server or stop another desktop's server to manufacture success.
2. In a separate fixture terminal, run
   `busctl --user monitor org.freedesktop.Notifications` to retain the real
   `Notify`/`NotificationClosed` traffic. Send a uniquely labelled event from
   another terminal; replace the example run token for each attempt:

   ```console
   busctl --user --timeout=5 call -- org.freedesktop.Notifications /org/freedesktop/Notifications org.freedesktop.Notifications Notify 'susssasa{sv}i' ryoku-workflow 0 '' 'Ryoku notification run-001' 'Native notification receive and history check' 0 0 -1
   ```

   Record the returned unsigned notification ID. `-1` is deliberate: the
   pinned model removes the popup after five seconds but keeps normal history;
   `0` would instead be excluded from history by this implementation. A reply
   alone proves delivery to the bus server, not rendering.
3. Capture and review the actual popup. Test ordinary DND both on and off:
   record popup suppression while on, continued history delivery, and restored
   popup delivery after turning it off. Restore the user's original setting.
   No machine checkpoint is required for these ordinary toggles. After the
   popup disappears, press the real `Super+N` shortcut. Capture the original
   notification center showing this exact summary and body. Also exercise the
   CLI route once and record its exit status. Check the shell journal for QML
   errors; verify closing/reopening the panel keeps the event.
4. Click this card's dismiss button in the original UI. The monitor must show
   `NotificationClosed` for the matching ID with reason `2` (dismissed by user).
   Capture the center after removal and reopen it to verify the event remains
   absent. Do not substitute the client's `CloseNotification` call for this UI
   action. Repeat with two distinct events to check the correct card is removed.
5. Log out normally, verify shell unit termination, then log into KDE and check
   its notification owner belongs to that desktop. Repeat the Ryoku test after
   fresh login/reboot. A shared user bus may keep other services alive; inspect
   ownership rather than assuming it changed. Stop the monitor afterwards and
   sanitize unrelated notification contents before publishing logs.

Acceptance needs paired bus traffic, owner identity, actual popup/history/
dismiss frames and lifecycle results. Host tests check route arguments,
allowlisting and inherited launch guards; they cannot prove D-Bus ownership,
display, focus, accessibility or hardware behavior. This change adds no fake
server, replacement notification UI or diagnostic app.

## Core functional checklist

“Native implementation, unverified” means real upstream code exists but the
named workflow has not passed this VM gate. “Needs bridge” identifies missing
Rust behavior. “Boundary conflict” means a backend alone cannot mediate the
unchanged UI. No row below asserts physical-device testing.

| Workflow | Actual implementation/status | Exact remaining work and evidence |
| --- | --- | --- |
| SDDM login, shell, reboot, KDE fallback | Staged native run passed in existing evidence | Revalidate installed RPM paths and cold boot; resolve watchdog locked-crash policy described in session-port.md |
| Launcher/application launch | Real shell and typed Discover/error provider passed; terminal adapter launched Konsole | Installed-RPM regression; ordinary desktop entry, Terminal=true, keyboard/focus, accessibility and scaling; other launcher providers remain individually unported |
| Notifications receive/history/dismiss | After native reboot, the package-owned Quickshell process received a real bus notification, `Super+N` showed original history, and the original card dismiss emitted `NotificationClosed` reason 2 | Repeat for packaged lifecycle only after a relevant change; DND suppression/restoration, arbitrary application actions and behavior in KDE remain unverified |
| Notification DND/keep-awake/game mode | Native `Flags.qml` user-owned JSON; exact capture scope now exists | Verify native behavior and restart persistence, then wire generation capture/restore into the coordinator; no per-toggle machine checkpoint |
| Audio graph/output/input reads | Native `Audio.qml` PipeWire nodes and trackers; unverified | Show real sink/source/stream states and hot removal in VM; classify dummy sink as simulated, no claim of audible hardware output |
| Audio volume/mute/default device/codec/EQ | Native ordinary PipeWire controls retained; Rust `ryoku-eq` now packages the pinned ten-band smart-filter workflow | Verify native volume/mute/default device controls and audible EQ in a supported session. The adapter persists ordinary user preferences and uses a per-user PipeWire unit; include preferences in recovery. The Go daemon's volume/mute verbs are retained from pinned upstream source and remain subject to installed-RPM regression. Protect only separately identified privileged/system changes |
| Clipboard history/copy/delete/clear/star | The Oct 18 ordinary-flow trial used the previous Release-2 upstream clipboard backend and displayed a captured entry. Current Release-10 RPM source routes the original QML surface to the Rust provider, installs/starts its unit, and disables the upstream watcher; this source is not yet built or runtime-accepted | Build/install the current RPM and verify multiple selections, copy-back, mutations, restart persistence and provider loss through the original UI. Clipboard history may contain sensitive data and is explicitly excluded from linked recovery. Historical upstream trial evidence does not establish the Rust provider's behavior |
| Network status/Wi-Fi/WireGuard/DNS | The desktop RPM now requires NetworkManager for the pinned daemon provider and Hub `nmcli` workflows. The opt-in daemon trial was built with its upstream NetworkManager provider, but the live connection query was not completed; no network-status behavior is credited | Read actual NetworkManager availability and Ethernet state, then test the UI and missing actions. Ordinary user connection/radio operations use native policy and permissions, with secrets out of argv/logs. Classify shared/systemwide DNS, service or security changes separately for protected recovery. Retain working ordinary Hub nmcli paths. Distinguish absent Wi-Fi from physical testing |
| Bluetooth status/discovery/pair/connect/power | The desktop RPM now requires BlueZ for the retained Quickshell provider and `bluetoothctl` pairing flow; no-adapter behavior remains unverified | Use native policy/permissions for ordinary pairing, connection, adapter and profile actions. Privileged service enablement or security/systemwide changes retain protected recovery. Test no-adapter VM and separately physical adapter/device paths |
| Screenshot | Selected RPM patch routes the original `Capture.qml` selection into Rust `ryoku-capture`; Rust opens the Pictures path component-by-component without following symlinks, creates only an owned `Screenshots` child, reserves a private unique file, invokes grim with argv, checks clipboard copy through the pinned daemon, and only reports success after requested operations finish. If clipboard copy fails after a persistent screenshot was saved, the file remains and the requested editor still opens; clipboard-only temporary files are removed | Native RPM/VM workflow still needs validation for full screen, monitor, region, file, clipboard, both, beautify, concurrent captures and provider errors. Capture preferences remain in the declared user inventory; the adapter does not add a per-capture machine checkpoint |
| Screen sharing/recording | Desktop RPM requires the portal frontend, Hyprland and GTK backends, and KDE's backend even with recommendations suppressed. The Ryoku session sets `XDG_CURRENT_DESKTOP=Hyprland`, so the installed Hyprland backend's desktop-specific `hyprland-portals.conf` supplies interface routing; original Recorder still invokes an absent helper ecosystem | Verify the active ScreenCast backend, real client chooser/grant/PipeWire stream/cancel/revoke across login; port missing recording controls and preserve ordinary recording/performance preferences in recovery inventory without per-edit machine capture; do not equate package routing configuration to a working share |
| Authentication/keyring | The desktop RPM now requires the upstream Hyprland Polkit agent and the Ryoku target wants its native user service for system authorization dialogs. The original shell `polkit` topic is still absent; Keyring IPC only renders prompts | Verify real action identity, trusted user/PAM conversation, cancellation/retry/secret handling and agent lifecycle. The system agent is not the Ryoku helper and does not authorize protected Ryoku mutations. Keep unsupported privileged controls unavailable until authenticated Rust helper dispatch, linked capture and native error UI are implemented. Credential recovery policy must be defined |
| Lock/unlock | The packaged Hyprland session offers `Super+L` and starts a bounded-restart Hypridle user service with a fixed policy that requests logind locking after five idle minutes and before sleep; Hypridle routes both to the configured Hyprlock command | Verify actual Wayland session-lock and authenticated unlock; idle timer, compositor death, restart, suspend/resume and failed-auth tests; preserve locked state, independent recovery handoff, no unported PAM edits |
| Tray | The pinned unchanged `ryoku-shell` daemon already owns the StatusNotifierWatcher, proxies StatusNotifierItem and DBusMenu, and serves the original QML `tray` topic and action calls. The desktop RPM builds this daemon from the checksummed upstream archive; no new project-owned Go code is added. Native implementation, not yet accepted in the current package-owned login | Verify the packaged daemon starts with the Hyprland session, publishes a real tray item, and exercises activate/contextMenu/scroll/aboutToShow/menuEvent, watcher registration/restart and real application interaction. Require a non-empty frame from a real item; source inspection and daemon build are not runtime evidence |
| WM/workspaces/overview/shortcuts | The preserved runtime already includes `Ryoku.Ui/Singletons/Wm.qml`; the upstream daemon already publishes the `wm` topic and reconnects its `ryoku-wm` client. The desktop RPM spec now declares build/install steps for the unchanged pinned Hyprland provider and its original `Ryoku.Wm.Hyprland` QML bridges. Build and package-owned runtime behavior remain unverified. See the [pinned-source review](../infra/tumbleweed-lab/evidence/issue25-pinned-wm-provider-source-review-oct18.toml). | Verify the provider binary resolves on the session daemon's fixed PATH, reports the live compositor version with its pinned capability manifest, feeds focused-output/window geometry/workspace joins, reconnects after compositor restart, and performs the UI's allowlisted neutral actions. Confirm QML bridge loading and real overview/workspace/window interactions. The daemon does not call the provider's separate `apply`/`outputs` config-writing verbs. |
| Hub shell settings/appearance | In the opt-in unchanged-daemon trial, the original theme command generated shared `colors.json`; the original bar and notification center rendered a matching static Tokyo Night palette | Package the ordinary backend and repeat the palette flow after RPM install; test ordinary preferences and persistence. Dynamic wallpaper/app theming remains unverified. Keep privileged system effects on the Rust authorization and linked-recovery path |
| Hub direct config/system pages | Mixed ordinary user and protected system effects | Retain ordinary config writers; route privileged service/security/systemwide operations through Rust authorization/capture. Do not disable whole pages solely because some actions are protected |
| Native software/update controls | Arch-oriented/unported, separate issues 27–30 | Typed openSUSE catalog/transaction/full-update providers and linked recovery. Do not run pacman/AUR or claim Ryostore reuse from launcher Discover |

## Existing integration boundaries inspected

| Boundary | Can retain unchanged | Cannot mediate |
| --- | --- | --- |
| Shell `IpcHandler`, `shell.qml:715` | Rust argv selects existing surfaces; keyring prompt display functions also exist | No interceptor for control actions, no authorization/capture request, no per-action failure UI |
| Launcher `shared/providers/script/Script.qml` | Rust executable returns rofi rows; activation receives literal argv plus ROFI_RETV/ROFI_INFO | Only its own provider rows; cannot intercept Hub, sliders, native setters or FileView writes |
| `ryoku-shell.sock` subscriptions/calls | A genuine Rust compatibility server could supply independently implemented topics and translate allowed actions to the typed service | Clipboard, Settings, Polkit and several other control sockets have no result parser; errors cannot be made visible by server changes alone. Supplying empty defaults is not implementing these topics |
| Existing child-process helper commands | Rust executables can implement reviewed argv/stdout/exit protocols where the caller already uses them | Embedded `bash -c`, direct `nmcli`/`pactl`, absolute helper paths, Qt property setters and file writes do not all pass through any single executable |
| `Ryoku.PluginKit` | Original visual components/tokens can be retained | PluginObjectSlot uses Qt.createComponent to instantiate a QML component; no process-wide mutation hook. Its Config singleton also seeds shell.json directly |
| Native Quickshell services | Real notifications, PipeWire/BlueZ reads and native event rendering | Writable native objects bypass the Rust service; NNP does not prevent user D-Bus/Wayland/PipeWire requests or home writes |
| Declarative settings/provider capabilities | Can select supported layouts/providers and legitimately reflect actual behavior | Settings.supports defaults missing caps to allowed; compositor caps hide certain rows only. No demonstrated global read-only/action-router setting covers all writers |

## Exact effect paths requiring classification in the pinned surfaces

These are independent paths; replacing `ryoku-shell` cannot mediate all of them.
After the user-authorized scope adjustment, ordinary user writers in this list
are retained and inventoried, not blanket-blocked. Protected system effects
still require the reviewed Rust route.

* `services/Audio.qml:134-135`: `Pipewire.preferredDefaultAudioSink/Source` setters;
  `RailStatus.qml` and audio mixer delegates directly set `.audio.muted` and
  `.audio.volume`. `Audio.qml` also launches `pactl set-card-profile`.
* `services/Flags.qml:53`: `onAdapterUpdated: writeAdapter()` for DND,
  keep-awake, game mode and associated persisted timestamps; first-run seed too.
* `services/Capture.qml:54`: direct capture preference FileView writes and
  embedded capture shell program; `Recorder.qml` has further file/process paths.
* `hub/quickshell/pages/PerformancePage.qml:163,210`, `RecordingPage.qml:186,247`,
  `LauncherPage.qml:134` and `WidgetsPage.qml:122`: direct writes of
  performance.json, recording.json, launcher.json and widgets.json, including
  first-run seeds. These do not use the Hub Settings singleton.
* `hub/quickshell/pages/ConnectionsPage.qml:1447`: direct BlueZ adapter enable;
  `:1496` rfkill, `:1504` privileged Bluetooth service enable. Network/hotspot
  paths also invoke nmcli directly; a protected `network.*` socket handler
  would leave those separate paths untouched.
* `hub/quickshell/pages/LockscreenPage.qml`: external lock/keyring/security-key
  helpers and embedded PAM-edit command. NNP blocks privilege gain through
  pkexec; it is not an implementation of the protected operation.
* `hub/quickshell/Singletons/Settings.qml`: settings.patch/settings.reset
  control socket has no result parser. `services/Clipboard.qml` similarly
  emits clipboard.copy/delete/clear/star without consuming operation results.
  A rejected recovery gate is therefore not reliably shown to the user.
* `services/Equalizer.qml`: the selected Rust adapter reports nonzero actions;
  the shell ignores exits from intentionally superseded commands, refreshes its
  state after a current action fails, and sends a rate-limited native notification.
  This replaces optimistic values with the helper-owned state; native audio graph
  acceptance remains pending.

The additional topic calls include `nightlight.toggle/set`,
`powerprofiles.setProfile`, `weather.retry/configure`, `calendar.configure`,
`music.track`, `updates.check`, `keypress.configure/settings`, and
`session.logout/reboot/shutdown`. Each needs a declared implementation and
effect classification; a generic shell-command RPC is not an acceptable port.
Addon/container/AI/lighting and other deferred controls do not become supported
because the shell renders. A release action registry must enumerate the chosen
supported page/control set and fail closed for all others.

## Selected architecture for complete write integration

There is no existing configuration, provider or plugin API that intercepts all
these writes. The revised scope does not require interception of every ordinary
user interaction. Protected system actions still need integration; working
native personal controls should remain intact. The user's direction was to make the least possible
changes to Ryoku and openSUSE and make them work together. The coordinator
selected a documented minimal upstream UI routing patch set, keeping the
original shell and Hub. This narrowly relaxes the earlier unchanged-QML rule
for binding/forwarding and result presentation; validation, providers,
authorization, capture and durable operations remain Rust-owned. This
notification change does not yet apply that larger integration patch set.

The smallest *architectural* upstream extension preserving the original visual
experience is a single backend action/state interface, but integrating it is
not a one-line patch or only a new singleton. Upstream would need to:

1. Publish versioned operation capabilities (supported/unavailable/reason),
   authoritative state revisions, typed action arguments, request IDs and
   durable operation IDs. The unprivileged Rust service remains the authority;
   the legacy JSON adapter may translate transport but cannot grant capabilities.
2. Replace selected **protected system effects** with action submission.
   Retain ordinary native personal settings and first-run user defaults;
   inventory their persistent state. Replace embedded privileged scripts with
   specific authenticated operations. Port missing providers without disabling
   unrelated working native controls.
3. Render pending, denied, capture-failed, provider-failed and completed states.
   Reconcile from authoritative state after failure; do not optimistically show
   success. Missing clipboard/settings providers need real response handling.
   Any protected system control needs defined commit/revision semantics and
   capture before its effect. Ordinary native volume sliders remain ordinary
   controls; no checkpoint-per-slider policy is introduced.
4. Preserve current upstream behavior behind an explicit upstream backend mode;
   the port selects the supported backend through native configuration. Add
   authoritative disconnected-state handling so stale state is not actionable.

The selected path is to maintain those changes as a small reviewed patch set
against exact upstream file hashes, preserving the original archive and source
pin. New QML bindings may forward actions and present authoritative state/errors;
they must not acquire backend business logic. Applying patches uses Rust
automation or the native declarative RPM patch pipeline, with clear attribution.
Upstreaming this interface remains desirable but is not assumed to have happened.

The considered alternative was Rust-owned managed controls and Hub, retaining
original components only where their boundaries meet the contract. It would
replace parts of the original experience and add theme/accessibility/keyboard
work, while still needing to remove unsafe duplicate original controls. It is
not the selected default. Adding a separate Rust settings app alongside the
unchanged unsafe controls would be insufficient in either architecture.

Both choices require the same Rust backend work: a finite domain action registry,
real native providers, effect/recovery scope, peer authentication and operation
authorization, linked verified capture **before** mutation, serialized revision
checks, durable execution surviving UI closure, post-change accounting and
failure recovery. Reuse the existing service/recovery contract rather than
starting a second privileged daemon. For each domain, test denied auth and
failed capture with proof that no provider setter/command ran, real provider
failure and rollback, restart/client disconnect, and native visible results.
Notification navigation can proceed now; implementing and validating the selected
backend and minimal UI routing changes remain necessary to finish the
working-desktop goal.
