# Ryoku 1.0 application integrations and host source boundaries

This partial #173 review reads pinned cached source at upstream 1.0.0
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`, compared with 0.75.5
`98a0368253d9628be72b853985878a5db90e1547`. It preserves every application,
palette integration, dictation/model, VM/remote-console and hardware workflow
as a full Tumbleweed target. It changes no source, payload, archive, spec or
runtime policy, and executes no application, endpoint, helper, installer,
model download/build, SSH, VM, device or privilege operation.

## Exact source and current package boundary

[The path ledger](../upstream/evidence/ryoku-v1.0/apps-system-disposition.tsv)
pins 370 current/prior identities: 369 current, 298 prior; 124 modified,
72 added, 173 unchanged and one old path absent. The complete bounded roots are
207 app paths, 26 palette-bridge paths, 113 current system paths and six Redux
asset inputs, plus 17 named Hub/backend/QML interfaces. The evidence includes
both prior/current modes and blobs and actual Source2/Source3 mappings.

| Area | Concrete scope and retained Tumbleweed owner |
|---|---|
| apps | 99 Store, 56 RyoVM/Ryoport and 52 application seed/helper/doc inputs. #182 owns Store/content/install/build, #47 full app inventory, #180 user appearance/settings and #181 host/admin operations. |
| palette bridge | All 26 daemon/watch/preferences, integration installer/remover, browser/music-client assets, native service and test paths. #25 core appearance, #180 user settings and #176 configured hooks; #182 content/integration ownership. |
| system | 62 hardware, 21 boot, 11 package, five policy, nine extras, two container, one service manifest and two root docs. #181/#32/#31 host/security/services, #178 Bluetooth, #177/#25 sleep/lock/session, #27/#28 packages, #49 separate media scope and #24 native install integration. |
| explicit Hub/Redux interfaces | Voxtype/Redux assets/backend/UI, palette routes, GPU VM/domain/hook routes and host control pages. #179 dictation/models, #47/#181/#182 VM integration, #180/#25 user appearance and existing #18/#20/#21 protection. |

Only the five named Hub pages are selected by Source2; those plus the 18
backend/Redux inputs are selected by Source3. The complete app, palette-bridge
and system roots in this unit are currently outside both exports. These are
current selection facts and unresolved source/package integration work, not
permanent exclusions. Raw backend selection does not implement a typed native
provider or authorize its embedded side effects.

The app and palette deltas contain 97 and 18 changed paths. System has 61
addition/modification rows plus one R100 image move out: the apparent absent
`system/boot/limine/ryoku-wall.png` is byte/mode-identical at
`fedora/system/boot/grub/theme/ryoku-wall.png`. The [API evidence](../upstream/evidence/ryoku-v1.0/apps-system-boundary.json)
pins that relocation, 45 source/anchor files, all system `.rules`/`.policy`
bytes, 38 raw service-manifest rows and five license/provenance identities.
None of these counts establishes complete loader, transitive API or native
acceptance coverage.

## App data, code execution and remote/VM state

| Exact source entry/call chain | Source effects and required guardrails |
|---|---|
| `ryoku-fastfetch` → config `// ryoku:renderer` → data-home executable | Lexically rejects empty/absolute/`..`/non-whitelisted renderer text, then checks `-x`/`-f` and executes `$XDG_DATA_HOME/ryoku/fastfetch/$renderer`. These tests follow filesystem links; this block does not resolve and confine symlink targets. Installed product/config can therefore choose user-account code execution, not just display data. Preserve custom renderers through #182/#180 trust/activation and bounded path/identity policy; authentication alone is not sandboxing. This helper is not currently selected/installed by this recipe. |
| fastfetch fallback → `ryoku-hub fastfetch effective <cache>` | Can render/write an effective config cache according to wallpaper/fixed preference. A display/query-like call may have persistent cache effects; #180/#25 must distinguish owned cache from user source configuration and preserve custom layouts/palettes without broad recovery scope. |
| Nautilus extension → stash install/compress helpers | Runs shared stash scripts for selected paths and exposes install/compress/send workflows. File-manager activation is not package trust or write authorization. #182/#27/#28 retain install/build/content choices with typed targets and #20/#21 linked mutation/recovery; #47 owns app integration. |
| RyoVM QML `Vm` → `ryovm` | Downloads images, creates/configures/launches/stops/deletes VM storage, grows disks and creates/restores/deletes qemu-img snapshots; deletion has quickemu and rm fallbacks. The source validates some names/paths, but those checks are not the port's complete managed-path, destructive-operation or disk-consistency proof. Preserve VM controls under #47/#181/#182 with typed intent, explicit destructive consent, bounded storage and realistic checkpoint limits. VM disk rollback is not a general home restore. |
| Ryoport → `ryovm lg` → Hub gpu vm | Friendly alias delegates libvirt/Looking Glass passthrough operations separately from quickemu; Hub GPU domain/hook helpers can write host/libvirt/VFIO configuration. Keep GPU passthrough/console capabilities under #47/#181/#20/#32, with privilege-specific capture/authorization and device identity; do not treat a user frontend as proof of user-only effects. |
| QML `Remotes` → `ryossh` | Manages SSH config/includes, remote metadata, keys/known-host state, tunnels and serial/telnet/RDP/VNC console records; saved passwords go through keyring operations. Probe options use `StrictHostKeyChecking=accept-new`, so a query-like SSH probe can update trust-on-first-use host-key state depending on effective SSH settings. #47/#181/#32 must separate discovery from trust/credential mutation, explicit host identity/consent and secret-safe recovery/logging. Guest provisioning/guest sudo is distinct from host privilege. |
| `ryovm-fetch` | HTTP HEAD/GET/Range downloader writes destination/partial data and removes partial output on failure/cancel. Cancellation and HTTP status checks are not content signature, path scope or VM image publisher trust. Preserve download capability with #47/#182 verified input/space/storage policy; no image was acquired here. |

The Store's 99 paths remain bound to the existing #182 catalogue/install/build
review and #27/#28 prerequisites. No app, renderer, remote, console or VM
capability is removed to avoid its guardrail work. Direct user commands,
Quickshell processes and properties need their own reviewed boundaries; an
exact-argv Rust alias table does not mediate every original upstream route.

## Palette display versus integration mutation

The bridge daemon reads cached colors.json and ryogami.json preferences,
validates values, watches files and serves palette/preferences GET/SSE routes
(default `127.0.0.1:47616`, with a listen override). These read-oriented HTTP
handlers are separate from the setup/removal side effects. Loopback by default
is not an accepted policy for arbitrary bind overrides or complete privacy
proof; no server was started.

Hub `runPaletteBridge` selects installation/removal scripts. `install.sh` can
build/install a checkout binary and user unit and disable legacy services.
`install-integrations.sh`/`remove-integrations.sh` modify Matugen configuration,
user data/templates, ownership TSV, Spicetify extension configuration,
Vesktop/Flatpak quick CSS/settings and Zen profile/extension files. Receipt or
owned-files tracking is useful source behavior, not independently authorized
write scope or complete linked checkpoint proof. Preserve live palette and all
Spotify/Vesktop/Zen integration choices under #25/#180/#182, with #176 hooks,
#31 lifecycle and #18/#20/#21 capture/restore. Do not copy browser-profile or
configured-script mutation into an unprotected provider.

## Voxtype/Redux native build, model and user-service state

Hub embeds Redux setup.py/adapter.py/LICENSE/UPSTREAM.md and stages them before
executing Python. Setup fetches pinned parakeet.cpp/ggml, builds a native
runtime, downloads a packed GGUF, publishes adapter/model/service/receipt files
and supports checked removal. Go coordinates cancellation for descendants,
selected preset changes, user-service drop-ins, daemon-reload and enable/stop.
These are retained dictation features with user data/model/service writes;
process-group cancellation and a receipt do not substitute for #179/#20
bounded authorization, linked pre/post state and interrupted-install recovery.
New project-owned provider logic remains Rust; upstream language provenance is
retained rather than silently imported as a port actuator.

Pinned UPSTREAM declares adapter revision
`072c62470215fc40dd060db0c0cdab1b9c825ae4` (MIT), native revision
`5c9b0fbbd8e611d88e47d7e1ebae9bca9b447e8f` and model revision
`68f808a5903a0bf12657bca3a2a669d573abfb58`, SHA-256
`574614b9a4d9f72ab202877a7ad6a1f4bf819dd1d27b9d42dfe8cd429fcebdd5`,
with a CC-BY-4.0 model declaration. Those pinned declarations and MIT bytes are
provenance inputs, not independent remote model/native-source verification or
redistribution-rights conclusions. No model was downloaded, built or licensed
by inference; #173/#179 retain dependency/notice/input verification gates.

## Host policy, hardware and persistent/live effects

| Pinned host boundary | Native owner requirement |
|---|---|
| network-kill on/off/boot → service state + marker + nft + nmcli | Enabling services and durable marker precedes firewall/network-off changes; off removes marker, disables units, deletes the nft table and restores networking best-effort. #181/#32/#31/#20 must preserve security and explicit recovery intent with full config/live state accounting and interruption tests; marker/status output is not firewall proof. |
| DNS / Wi-Fi backend / powersave | DNS writes a NetworkManager global-dns drop-in and can pkexec itself; backend switch changes config/markers and restarts NetworkManager; power-save uses iw and per-device runtime saved state. #181/#178 where device-related, #31/#32/#20 own typed exact targets, service/connectivity impact, privilege and linked capture/post-state. Preserve all controls; do not port edition-specific policy defaults as accepted Tumbleweed semantics. |
| GPU mux / drivers / audio / power | Firmware/sysfs knobs can re-exec through pkexec/sudo; driver paths use foreign package/initramfs mechanisms; audio recovery restarts/kills user services and may reset USB devices. Battery charge ceiling/ASPM apply reads stored user intent then writes root-owned hardware state. #181/#180/#27/#28/#31/#32 must distinguish user preference from privileged device/live state, supported hardware and realistic recovery; range checks alone are not authorization. |
| power cutover / package hooks / service manifest | Cutover coordinates session generations, sleep inhibitors, lock clients and package replacement; hooks invoke it during package operations. Manifest has 38 system/user activation rows and persistent condition/disable markers. #25/#177/#31/#27/#28/#20 require native service/lock ordering and complete managed inventory; do not infer secure suspend from a helper or source test. |
| timedate/users polkit policy | Timedate grants timezone changes to active wheel users without a password; users action binds ryoku-host/users and retained admin authentication. Native Tumbleweed group/action/helper identity, per-verb data bounds and #20 capture remain #181/#32 work. Authentication alone does not bound arbitrary root writes. |

A concrete executable-identity gap is pinned in
`system/hardware/network/50-ryoku-dns.rules`: it grants polkit YES to an active
local wheel user when the exec program matches the **basename** ryoku-dns,
including a developer-checkout path by its comment. This is not a binding to a
reviewed immutable root executable. The exact original rule and context are
source evidence and **are not selected or installed by this package recipe**.
Preserve the privileged DNS workflow through #181/#32/#20 typed-program
identity, explicit authorization and complete capture; do not carry that broad
basename grant into the native provider. No feature removal is recommended.
All other system rule/policy bytes are recorded for per-action native review,
not endorsed wholesale.

Root LICENSE/NOTICE, palette MIT and Redux MIT/UPSTREAM identities are pinned;
they do not settle every app/font/logo/guest-image/plugin/model dependency's
rights or installed notices. Full loader/source/archive, per-operation and
native package/device/session/recovery acceptance remains open. This source
unit implements no feature provider and checks no entire #173 acceptance row
complete; every original target and owner remains required.
