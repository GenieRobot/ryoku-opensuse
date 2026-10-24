# Issue #25 functional checklist

Current candidate: `ryoku-session-0.1.0-3` layered on the issue #173
`ryoku-desktop-0.1.0-8` Ryoku Amaterasu 1.0.0 candidate, upstream pin
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. The session contract and historical login/reboot/KDE
receipts are in [session-port.md](session-port.md). Each row below remains
pending for this exact candidate. Source inspection and host tests are not a
native desktop pass. No physical device behavior is claimed.

The [first native checkpoint](../infra/tumbleweed-lab/evidence/issue25-native-20261022.toml)
retains the initial runtime/service failures. The [resumed native receipt](../infra/tumbleweed-lab/evidence/issue25-native-resume-20261022.toml)
records current helper startup, ordinary password Ryoku re-entry after reboot,
all six required session/portal units, actual launcher-to-Konsole and Hub,
synthetic notification popup, monitor-capture file creation and normal KDE
fallback at exact source `7b0d2ef`. These are partial observations; the full
rows below remain open. Direct compositor logout caused a native portal
SIGSEGV. The ordered logout worker, private Rust clipboard keeper and distro
hyprlock adapter require new native validation. The worker now covers the
original shell's provider session-exit route as well as the keyboard route;
host tests do not establish the native result. All earlier receipts use the
0.75.5 source and cannot establish 1.0 acceptance.

| Workflow | Implementation boundary | Required native evidence | Current result / device gap |
| --- | --- | --- | --- |
| Notifications | Pinned Quickshell NotificationServer; `Super+N` opens `quick-settings#notifications` | Receive a real application notification, review popup/history, open and dismiss; verify service loss/recovery without fabricated events | Pending; no physical device required |
| Clipboard | Packaged pinned daemon's real clipboard topic and wl-clipboard; no competing Rust socket owner | Copy/paste text and image, retain selection after source exit, inspect history/star/clear and reconnect/error behavior using synthetic data | Native pending; existing daemon owns history; private pinned Rust selection keeper candidate builds separately, not yet installed |
| Audio | Original PipeWire UI and native user server | Actual default sink/source, mute/volume, playback and recorded loopback; failure and reconnection | Pending; simulated VM sink/loopback must be labeled; no speaker/microphone proof |
| Network | Original native NetworkManager paths and existing provider | Compare real device/link state with native tooling, exercise ordinary permitted connections and actual failures; keep systemwide writes unsupported | Pending; virtual Ethernet does not prove Wi-Fi, signal or hotspot behavior |
| Bluetooth | Reviewed BlueZ status/read paths; only explicitly implemented ordinary controls; agent/link and protected pairing/bond removal remain unavailable | Compare actual adapter/device state and unavailable/error presentation with native tooling; exercise only enabled controls | Pending; absent VM adapter means unavailable/untested; pairing/bond changes require their own authorized Rust provider/recovery work |
| Screenshots | Retained capture surface and existing coordinated writer route | Capture actual output/region, save/copy and error/cancel paths, check resulting image | Pending; capture helpers/dependencies and preferences need native verification |
| Screen sharing | Native portal/PipeWire/compositor interaction | Real application requests a screen; inspect chooser, grant/revoke/cancel and stream termination | Pending; no portal, sharing or recording acceptance from a framebuffer capture |
| Authentication | SDDM login remains native; typed Rust service/helper boundary unchanged; upstream polkit agent disabled | Ordinary password login plus separate reviewed agent/PAM authorization tests before enabling protected actions | Current native SDDM password login and reboot re-entry observed; privileged prompts remain unavailable |
| Locking | Native distro hyprlock through fixed user unit and Rust alias; no idle/suspend integration accepted | Current VM lock/unlock, wrong-password rejection and cancellation; preserve distro PAM stack/password policy | Native pending; empty/stalled PAM, suspend/resume continuity and locked-crash guarantees remain open and unclaimed |
| Shell/Hub/launcher/appearance | Pinned surfaces under existing Rust writer coordinator; original launch/result path | Actual visible bar/Hub/launcher, keyboard navigation, app launch, theme/scale and honest errors; authorized recovery gates for supported persistent writes | Native bar/Hub/launcher and Konsole launch observed; appearance writes and full error paths pending |
| KDE fallback | Independent Plasma session with explicit wallpaper/session dependencies | Cold login, panel/launcher/app, reboot roundtrip and recovery after intentional Ryoku startup failure | Current native ordinary KDE fallback and panel observed; app interaction and intentional-failure gate pending |
| Logout/failure | Original shell provider session-exit and keyboard request use the independent fixed logout unit; required startup failure exits to SDDM | Exercise both logout routes while portals are active; verify no portal crash, target/descendant shutdown and cleared environment; then password-login to KDE. Repeat after controlled startup failure, a timed-out lifecycle client and reboot | Offline subprocess failure/deadline ordering passed; native service/greeter behavior pending for 1.0; earlier direct compositor exit crashed the Hyprland portal |

For every native test record source/RPM versions, fixture/image revision,
installed hashes, environment, sanitized startup logs and actual screenshots
where relevant. Identify virtual devices, loopback simulation, absent devices
and physical tests separately. Use synthetic notification and clipboard data;
raw private session logs and clipboard contents do not belong in this repository.
Record SDDM/PAM and password/autologin configuration hashes before install and
after both session logins; the port does not install the upstream greeter,
guard, `sddm/setup`, Wayland wrapper or qylock. Keep optional Voxtype absent
during at least one successful cold login so it cannot become an accidental
startup prerequisite.

The issue #1 recovery and authorization contract remains in force. Registering
writers permits coordinated capture; it is not capture or per-action approval.
No row enables protected writes or substitutes NNP for authorization. Keep
privileged package, service, PAM and systemwide operations unavailable until
their reviewed Rust operation verifies the linked checkpoint and required
capture failures are proven to prevent mutation.

## Provider write reports remain an independent coverage gate

The pinned Hyprland `ApplyReport.Written` lists only three live files:
`~/.config/hypr/settings.lua`, `settings_window.lua` and `rebinds.lua`.
`writeOverlayLua` also writes each file under
`~/.config/ryoku/user_edits/hypr`, yielding six persistent user files. The same
apply then calls `PublishGreeterNumlock`, whose handoff destination is selected
by `RYOKU_GREETER_NUMLOCK_FILE`, `RYOKU_GREETER_STATE_DIR` or the global default.
The three-path report is not a complete write envelope or #18/#20 capture proof.
This remains separate from the wallpaper operation covered here. The typed
Hyprland settings path must account for every actual user write; #177 owns the
cross-account greeter handoff, with #180 owning its user-setting integration.
Keep the full controls in the product target and this acceptance gate open.

## Consolidation of earlier #25 work

The former canonical head was `3d90c3c2ff014ab718c6dfcecbd4cd5c9ef2d670`.
Its complete reachable history is preserved locally in the verified
`target/integration-archive/issue25-native-workflows/pre-consolidation-3d90c3c.bundle`
(SHA-256 `ba785e45b69b0470acf37aa2df6b8f6ad52f0dcda1f8b1a086ce750ee456bc92`).
The current branch is reconstructed on merged #23 main `e025c7b` so it does
not regress unrelated recovery/service fixes from older mixed history.

The useful original Rust lifecycle/terminal adapter, declarative session
configuration and #25 native receipts are retained and adapted to current
package paths. Direct unregistered shell launches and the standalone clipboard
socket owner were replaced by the packaged daemon's existing Rust writer route.
No unrelated issue branch was merged.

Closed historical [PR #156](https://github.com/GenieRobot/ryoku-opensuse/pull/156), head
`c76c3140b1b419eed351664ad55ae75f2ce714d2`, remains an older partial source.
Its Rust equalizer, clipboard reply-presentation patch, clipboard disable patch,
and hyprlock/hypridle candidates are not represented or accepted by this change.
They require individual protocol/recovery/native review before adoption; the
coordinator closed the older PR as superseded while retaining its source branch
and explicitly unaccepted unique candidates. There is one active #25
implementation lane, [PR #169](https://github.com/GenieRobot/ryoku-opensuse/pull/169).
