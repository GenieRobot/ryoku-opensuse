# Ryoku 1.0 login, lock and Bluetooth source boundaries

This #173 source/API unit compares pinned upstream 1.0.0
`a59d051f0e8080052216f6c7b9eb7e5928935ce8` with 0.75.5
`98a0368253d9628be72b853985878a5db90e1547`. It retains the complete login,
lockscreen, theme, fingerprint/password, Bluetooth pairing and device-control
feature target. It changes no source, archive, recipe, UI or runtime policy.
Current selection gaps below are implementation facts, not product exclusions.

## Exact inventory and package boundary

[The path ledger](../upstream/evidence/ryoku-v1.0/lock-bluetooth-disposition.tsv)
pins current/prior Git modes and blobs for 122 paths: 66 complete current
`ryoku/lockscreen` entries, 45 shell/Hub QML files containing a direct
`Quickshell.Bluetooth` import or `ryoku-bluetooth-link` reference, and 11
integration/helper/test inputs. There are 84 prior paths, 38 additions,
28 modifications and 56 unchanged identities; no paths disappear in this
bounded union. These counts do not claim complete transitive UI coverage.

All 45 QML candidates are selected by both Source2 and Source3. Two other
integration inputs are mapped by Source2 and three by Source3. The 66
lockscreen entries are currently outside both exports; this is a concrete
source/archive and native-implementation gap for #173/#177, not permission to
omit login/lock functionality. The original upstream Git identities remain
pinned. The upstream desktop payload recipe installs lock launchers, greeter,
session wrapper, guard system unit and Bluetooth agent user unit. The current
Tumbleweed desktop recipe does not install those upstream greeter/PAM/DM
mechanisms as-is. Neither the upstream recipe nor raw source selection proves
that a native operation is packaged, authorized or accepted.

[The API evidence](../upstream/evidence/ryoku-v1.0/lock-bluetooth-api-boundary.json)
pins 27 entrypoint/provenance files with byte digests and exact source-line
anchors, plus six license/asset identities and the literal absolute Git link.
It distinguishes original source from existing port patches. The current
launch patch routes several direct Bluetooth launches/API mutations through
the existing Rust writer boundary; those guards are not a completed typed
Bluetooth provider or a reason to drop the corresponding controls. All retained
operations still require #178's explicit confirmation and recovery contracts.

## Login and lock authentication is separate from persistent write ownership

| Pinned source chain | Conditional effect and owner requirement |
|---|---|
| `sddm/setup` → `sudo -n`/root helpers | Enables SDDM and graphical.target; alternatively changes runit DM links. Edits `/etc/pam.d/sddm` with Arch `system-login` assumptions, adds keyring PAM lines or strips them when autologin is detected, and creates a missing Wayland session entry. #177 owns native Tumbleweed DM/PAM/keyring/autologin policy and explicit protected writes; #25 owns session integration, with #18/#20 capture/authorization and #31 service operations. Do not copy the Arch stack into the port. |
| `install-qylock` → user/root write helpers → `ryoku-qylock-activate` | Writes SDDM configuration/theme files and user lock/theme/generation trees, seeds `~/.config/qylock/theme` only when absent, migrates legacy themes, stages/promotes generations and can remove/replace trees. Generation/mutation flock guards and restore_previous are upstream coordination, not the port's authorization or linked checkpoint proof. #177/#25/#18/#20/#21 must bind exact file ownership, rollback and interrupted activation. Optional themes remain part of the full target under #182. |
| Hub `LockscreenPage.lockLauncher` / IPC `actions.go` → `ryoku-qylock-lock` → `lock.sh` | PATH-resolved launcher holds launch/generation guards before starting the QML lock entry. `lock.sh` validates theme/runtime conditions, tracks the login1 session and starts Quickshell. PATH lookup or a process existing does not establish compositor-secure lock. #177 must test the exact native Quickshell/session-lock protocol and failure behavior; #25 integrates suspend/logout/session lifecycle. |
| `lock_shell.qml` → `proof.sh` → `SddmShim.qml` | The source models secure-lock proof and foreground login1 activity before arming two `Quickshell.Services.Pam` conversations. Password and fingerprint race; first guarded `PamResult.Success` aborts the other and signals loginSucceeded. Session changes cancel pending reveal/authentication. These are source guards, not proof that the Tumbleweed compositor, PAM modules or all races enforce the intended invariant. |
| `assets/pam/ryoku-lock[-pw]` | Local `PamContext.configDirectory` loads bundled services: fingerprint uses pam_fprintd timeout=-1, pam_deny and pam_unix account/session; password uses pam_unix **nullok**, account/session checks and password-change denial. #177 must decide native account/empty-password policy explicitly and verify failure, lockout, fingerprint cancellation, logout and foreground changes without credential logging. Bundled PAM configuration does not avoid distro-policy review. |
| loginSucceeded → `unlock.sh` → `ryoku-qylock-unlock-prepare` → `loginctl unlock-session` | Attempts hard sleep-block restoration, generation-proof invalidation and login1/session checks before reveal. The prepare helper uses `ryoku-host`, transient inhibitor/service operations, busctl and the shell unlock-prepare route. Authentication success alone does not authorize arbitrary configuration/service writes; #177/#25 require the exact protected state transition and interruption/relock checks. |
| `ryoku-greeter` / `ryoku-wayland-session` | Greeter selects DRM outputs and Weston/kiosk behavior, reads keyboard and cross-account greeter-primary/numlock handoffs. Session wrapper imports allowlisted user-systemd environment assignments with `eval`, may run `ryoku materialize` (persistent user configuration), waits boundedly for Weston and starts the chosen session/default SDDM wrapper. It maps failure to exit 2 for upstream SDDM's greeter restart behavior. #177/#25 must validate native paths/version semantics, environment provenance, configuration capture and KDE fallback. |
| `ryoku-greeter-guard.service` → guard → `ryoku-host svc --system restart sddm` | System service follows display-manager startup, watches process/login1 state and writes `/run/ryoku/greeter-guard` counters/timestamps. Defaults permit three restarts after a grace period. A missing-helper heuristic may affect the login manager, so #177/#31 must establish correct authority, bounds, active-session protection and recoverability on Tumbleweed. No restart or authentication was executed in this review. |

The precise import link
`qylock/quickshell-lockscreen/imports/QtGraphicalEffects/private` is a Git
120000 blob whose literal target is
`/usr/lib/qt6/qml/Qt5Compat/GraphicalEffects/private`. It is an absolute directory
link, not either of the two accepted relative regular-blob Rashin links.
This review reads its Git bytes without dereferencing the host path. #177 must
resolve the real native Qt5Compat/module layout while preserving effects and
skins; do not relax Source3's confinement policy or pretend the source link is
a validated Tumbleweed dependency. QtMultimedia/SddmComponents compatibility
shims and `Quickshell.Services.Pam` likewise need native import/plugin checks.

## Bluetooth bond, confirmation and durable state boundaries

| Pinned source chain | Conditional effect and retained Tumbleweed contract |
|---|---|
| Hub `ConnectionsPage.activateDevice/pairDevice` and shell `BtLink.linkCommand` | Connected Hub devices invoke `d.disconnect()` directly. Pairing stops discovery, launches the shared helper with the address and tracks UI process state. `BluetoothDiscovery.setDiscovering/stopAll` changes `adapter.discovering`, with shared owner tracking. Keep discovery, presentation, connect/disconnect and all UI affordances; #178 must separate status reads from state changes across all reachable Quickshell BlueZ paths, including direct API calls. |
| `ryoku-bluetooth-link` → `btc` | Validates MAC syntax, then pipes **`yes`** into `bluetoothctl --agent KeyboardDisplay --timeout ...`; it stops scanning and can power the adapter on, pair, trust and connect. The script's subprocess timeout and address regex are not user consent, authenticated device identity or a capture contract. #178 owns typed BlueZ operations and **explicit confirmation**, including cancel/timeout and partial-success reporting; do not silently port the auto-accept behavior. |
| Failed connect on a previously paired device → remove → pair/trust/connect again | The exact script records `was_paired`, attempts the normal link, then executes **`bluetoothctl remove "$address"`** before retrying pairing when that attempt fails. This can replace a durable bond, not merely reconnect. #178/#20 require a separately authorized repair intent, linked pre/post state and realistic recovery limits for secret/bond material. An automatic destructive retry must not be treated as read-only diagnosis or harmless connection repair. Preserve intentional repair functionality. |
| `ryoku-bt-agent.service` → `/bin/sh -c 'tail -f /dev/null | bluetoothctl --agent KeyboardDisplay'` | User service follows bluetooth.service, keeps stdin open and describes **auto-confirming comparison prompts** in its source. This is the agent implementation: there is no separate `ryoku-bt-agent` executable in this pinned tree. #178 must preserve agent/pairing functionality with typed explicit consent, reject/cancel lifecycle and no credential history; do not silently inherit this auto-confirm default. User-service execution is distinct from root execution and from BlueZ/polkit authorization. |
| `ryoku-bluetooth-power` → save_state → apply_state | Writes `${XDG_STATE_HOME:-$HOME/.local/state}/ryoku/bluetooth-power` (environment override supported) via umask077/temp/move **before** applying bluetoothctl power and rfkill block/unblock. Supports on/off/toggle/restore/state/is-on. Saved intent may therefore differ from hardware state after failure. #178/#25 require bounded paths, explicit mutation authorization, linked capture/post-state and accurate UI; #181 owns host/service integration where needed. |

The 45 QML candidates include multiple bar/sidebar/launcher and iNiR/stage
variants. Import discovery is not proof of every loader or direct BlueZ action;
the exact-argv writer guard alone cannot protect an unmediated QML property
setter. Full typed routes, confirmation UI, checkpoint scope, interruption and
native BlueZ/device tests remain #178 acceptance. Existing upstream test files
are pinned source evidence only; their mocks were not executed or presented as
physical-device acceptance here.

## Provenance and unresolved gates

Vendored `qylock/README.ryoku.md` declares Darkkal44/qylock commit
`cde4d11e9e3d385620becdc877a0521e40a55e47`, a trimmed clockwork/lockscreen
bundle and GPL version 3 license text. The exact license, README, theme
metadata and Outfit-Black font bytes are pinned alongside root LICENSE/NOTICE.
That declaration does not independently prove every font/preview/compatibility
shim's license, its modification notice, or redistribution permission. Those
per-asset and native installed-notice checks remain open under #173/#177;
no license/version is inferred from a directory or font name.

This unit leaves the full #173 shell/native, semantic and fresh acceptance
checklist open. #177 owns DM/PAM/guard/lock implementation and native secure-lock
claims; #178 owns all typed Bluetooth/agent/device operations; #25 owns initial
session/lifecycle integration. #18/#20/#21/#31 and #181/#182 supply the shared
protected-write, service and theme contracts. Nothing here completes those
features, changes their target scope or substitutes inventory for acceptance.
