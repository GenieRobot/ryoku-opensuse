# Issue #25 functional checklist

Current candidate: the optional `ryoku-session` RPM layered on the accepted
issue #23 package set. The session contract and historical login/reboot/KDE
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
SIGSEGV. The new ordered logout, private Rust clipboard keeper and distro
hyprlock adapter require new native validation; host tests do not accept them.

| Workflow | Implementation boundary | Required native evidence | Current result / device gap |
| --- | --- | --- | --- |
| Notifications | Pinned Quickshell NotificationServer; `Super+N` opens `quick-settings#notifications` | Receive a real application notification, review popup/history, open and dismiss; verify service loss/recovery without fabricated events | Pending; no physical device required |
| Clipboard | Packaged pinned daemon's real clipboard topic and wl-clipboard; no competing Rust socket owner | Copy/paste text and image, retain selection after source exit, inspect history/star/clear and reconnect/error behavior using synthetic data | Native pending; existing daemon owns history; private pinned Rust selection keeper candidate builds separately, not yet installed |
| Audio | Original PipeWire UI and native user server | Actual default sink/source, mute/volume, playback and recorded loopback; failure and reconnection | Pending; simulated VM sink/loopback must be labeled; no speaker/microphone proof |
| Network | Original native NetworkManager paths and existing provider | Compare real device/link state with native tooling, exercise ordinary permitted connections and actual failures; keep systemwide writes unsupported | Pending; virtual Ethernet does not prove Wi-Fi, signal or hotspot behavior |
| Bluetooth | Original BlueZ reads/ordinary controls; privileged service actions unsupported | Real adapter discovery/pair/connect/disconnect and failure presentation, with adapter provenance | Pending; absent VM adapter means unavailable/untested, never simulated success |
| Screenshots | Retained capture surface and existing coordinated writer route | Capture actual output/region, save/copy and error/cancel paths, check resulting image | Pending; capture helpers/dependencies and preferences need native verification |
| Screen sharing | Native portal/PipeWire/compositor interaction | Real application requests a screen; inspect chooser, grant/revoke/cancel and stream termination | Pending; no portal, sharing or recording acceptance from a framebuffer capture |
| Authentication | SDDM login remains native; typed Rust service/helper boundary unchanged; upstream polkit agent disabled | Ordinary password login plus separate reviewed agent/PAM authorization tests before enabling protected actions | Current native SDDM password login and reboot re-entry observed; privileged prompts remain unavailable |
| Locking | Native distro hyprlock through fixed user unit and Rust alias; no idle/suspend integration accepted | Real lock/unlock, wrong password, cancellation, suspend/resume and locked compositor crash without unlock exposure | Native pending; wrong-password, unlock, suspend/resume and crash behavior unproven |
| Shell/Hub/launcher/appearance | Pinned surfaces under existing Rust writer coordinator; original launch/result path | Actual visible bar/Hub/launcher, keyboard navigation, app launch, theme/scale and honest errors; authorized recovery gates for supported persistent writes | Native bar/Hub/launcher and Konsole launch observed; appearance writes and full error paths pending |
| KDE fallback | Independent Plasma session with explicit wallpaper/session dependencies | Cold login, panel/launcher/app, reboot roundtrip and recovery after intentional Ryoku startup failure | Current native ordinary KDE fallback and panel observed; app interaction and intentional-failure gate pending |

For every native test record source/RPM versions, fixture/image revision,
installed hashes, environment, sanitized startup logs and actual screenshots
where relevant. Identify virtual devices, loopback simulation, absent devices
and physical tests separately. Use synthetic notification and clipboard data;
raw private session logs and clipboard contents do not belong in this repository.

The issue #1 recovery and authorization contract remains in force. Registering
writers permits coordinated capture; it is not capture or per-action approval.
No row enables protected writes or substitutes NNP for authorization. Keep
privileged package, service, PAM and systemwide operations unavailable until
their reviewed Rust operation verifies the linked checkpoint and required
capture failures are proven to prevent mutation.

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
