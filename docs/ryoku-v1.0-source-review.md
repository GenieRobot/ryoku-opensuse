# Ryoku 1.0 candidate source and command boundary

This records issue #173's candidate at upstream commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`, tree
`06b2962f82f43118c3a98372c2a4b435b689b725`. It is a major upstream source
change. The openSUSE RPM version remains the independently versioned `0.1.0`.
No old VM result is relabelled as acceptance of this source.

The release target retains the full Ryoku 1.0 system on Tumbleweed, including
login/guard/lock, Bluetooth link/agent, dictation, Hub controls, Store/apps and
installation workflows. Current manifest nonselection or runtime refusal is
an implementation/security gap, not permanent feature removal. New M2 owners
are #177 greeter/session guard/lock, #178 Bluetooth, #179 Voxtype/Redux,
#180 Hub user/appearance actions, #181 Hub host/network/security and #182
Store/themes/plugins/rices/apps, #183 Rashin/usage and #184 phone integration,
in addition to #25/#175/#176. #47 remains the application inventory parent. #173 owns
source/spec/provenance review only; those tasks retain the feature behavior
and implement Tumbleweed-specific guardrails. Only edition-specific platform
mechanisms are replaced. No code or package root is changed by this update.

## Reproducible source evidence

The Rust `upstream-runtime-export` tool reads exact Git objects and modes,
checks the full source commit/tree, refuses selected links/submodules, and
checks each of the 18 font files against its independently pinned digest.
It never exports mutable checkout bytes. Its tests dirty a checkout and prove
the same bytes/modes/archives still result, and reject font tampering and
selected links. An existing output directory is rejected.

Run from the repository, with the two exact upstream commits fetched into a
source cache and the manifest's pinned font files supplied in `FONT_ROOT`:

```text
cargo run --offline -p ryoku-automation --bin upstream-runtime-export -- UPSTREAM_GIT upstream/ryoku-runtime.toml packaging/rpm/upstream-writer-paths.txt FONT_ROOT FRESH_OUTPUT
```

Two independent exports reproduced these candidate inputs. Their full
path/mode/blob/SHA-256 records are committed in
[`upstream/evidence/ryoku-v1.0/`](../upstream/evidence/ryoku-v1.0/).

| Input | Files | Bytes | SHA-256 |
|---|---:|---:|---|
| `ryoku-runtime-a59d051.tar.gz` | 4,472 | 54,073,125 | `8aeff20645c14af76f3d341e5d4e7c31923fd249f6671caa704c8b23a6c84e65` |
| `ryoku-writers-a59d051.tar.xz` | 5,861 | 33,208,768 | `7b68eb2e1a81fe3d6b28edaa89708b2c6663082e72f74bdce4087eb02799c9be` |

Source2 has 4,450 Git-to-payload mappings, 18 font assets and four generated
metadata files. Some Git blobs are deliberately mapped to two import roots.
The generated manifest includes the selected source/export/font identity and
does not recursively include its own archive hash. GNU tar uses sorted paths,
numeric uid/gid 0, epoch 0 and gzip `-n -9`. Source3 uses the actual commit epoch
1792838837 and xz `-6`. These settings reproduce the recorded Source3 hash.
Source2 replaces an earlier unaccepted candidate whose staging directory was
lost; it does not claim that unrecoverable archive was reproduced.

`source-sites.json` indexes all 3,411 selected non-vendor native source text
files, including embedded Python and Rust, with Git blob, SHA-256, QML imports,
write/process candidate lines and export destinations. A candidate line is
not automatically a configuration writer: stdout, sockets, caches, fixture
code and application state need distinct dispositions. The complete export
manifests, rather than a directory count, define the selected payload.

## Retained targets and current integration boundaries

The component-level [Hub/store review](ryoku-v1.0-hub-store-review.md)
records exact file dispositions and the incompatible store API/write contract.
It covers those areas only; it does not complete the full delta matrix or native
acceptance gates.

The separate [CLI/system lifecycle review](ryoku-v1.0-cli-lifecycle-review.md)
records 774 current/prior paths, process/write candidates and exact Rust owner
boundaries. Current CLI nonselection is a package fact, not permanent feature
exclusion; complete CLI source-only archival and protected equivalents remain
pending. This source-only review changes no #25 shell/session files.

| Area | Exact selected source surface / behavior | Candidate disposition |
|---|---|---|
| Shell/core | `ryoku/shell/ipc`, selected QML, scripts and user units | Retain native shell, approved Rust writer roles and existing namespace/recovery ownership. Refresh actual v1 unit fixture; use private coordinated Quickshell launcher. Shell state/cache writes remain under the existing managed-writer boundary. |
| Native modules | `ryoku/shell/plugin` | Build both Blobs and new Spatial3D with complete QML metadata, plugins and shaders. No prebuilt upstream binaries. Native loading/ELF closure remains an acceptance gate. |
| Hub queries | `ryoku/hub/backend` dispatch | Rust exact-arity allowlist retains reviewed display/query routes. Unrecognized routes and every unapproved mutation family return `Unsupported` before UID lookup, namespace creation, coordinator setup or native child dispatch. Non-UTF-8 arguments and additional verbs do not bypass the table. |
| Hub writes | config/shell/desktop/WM/output/lock/GPU/CPU/lighting/Voxtype/Rashin/phone/rice/fastfetch/starship/profile/matugen/reload-cover/palette/share/clipboard/import | Retained target: #180 user/appearance actions, #181 host/network/security, #179 dictation, #182 Store/apps, #183 Rashin and #184 phone. Current exact-query policy and mutation refusals are temporary guardrails, not final support or feature removal. Each owner must implement typed authorized operations with linked recovery and preserve UI affordances. |
| Phone | Hub `phone.go`; shell `PhoneBridge.qml`, `PhoneState.qml`, `KdeConnectService.qml`; Hub `PhoneConnectionTab.qml` | Retained phone/KDE Connect/ADB workflows are owned by #184, with shared host controls #181 and Bluetooth bond/agent operations #178. Current status/devices/pair/ADB refusals and false capability gates are safety facts, not final feature scope. No successful pairing or completed provider is claimed. |
| Import | `import.go`, provider import metadata and parser files | Retained import/configuration workflow under #180, with installed content/provider scope #182. Current providers/local-detection queries remain; remote scan/clone/apply/undo refusals are temporary until typed ownership/recovery integration passes. |
| Rice/profile | `rice.go`, `profile.go` | Retained #180 settings/profile and #182 content workflows. Current list/preflight queries and write refusals do not implement final functionality. Exact config/rice/wallpaper/profile destinations need authorized linked pre/post recovery before writes are supported. |
| Voxtype/Redux | `voxtype.go`, embedded `assets/redux/{adapter.py,setup.py}` | Retained dictation/setup/model/service/removal target under #179. Existing query-only/refusal launch patches are temporary safety boundaries. Retain compatible upstream UI/assets/Python provenance and licenses; implement new project-owned integration in Rust with package/service/recovery owners. |
| Host/package/auth | Hub SettingsSheet, Dictation/Global/Input/Connections/Users/Lockscreen/Updates pages | Retained host workflows under #181, using #27–29 packages, #31 services and #32 security. Current direct package/auth/keymap/time/service launch refusals do not define final feature scope. Replace edition-specific actuators with typed Tumbleweed operations, not blanket UI removal. |
| Network/Bluetooth | Selected shell bar/sidebar/Stage/iNiR services, Hub Connections, upstream Bluetooth link/agent | Retained BlueZ list/connect/pair/remove/trust/power/discovery/link/agent target #178 and host/network/VPN/Tailscale/device controls #181. Current direct nmcli/pkexec/sysfs/BlueZ refusal patches and nonselection of link/agent scripts are safety/package facts only. New Rust providers need authorization, exact state ownership and linked accounting while preserving controls. |
| Ryogami | `ryoku/shell/ryogami/{daemon,picker}` plus separate `ryoku/shell/livewall` | Preserve the complete daemon/editor/livewall experience and its package/UI nodes; no exclusion is pursued. No current Ryogami `hyprland`/`bin` subtrees exist. [Exact inventory and protected-integration ownership](ryoku-v1.0-ryogami-rashin-review.md) map core appearance to #25, retained source/workshop/model/library/delete providers to #175 and startup/restore/postapply policy to #176, using #18/#20 contracts. These owners retain full functionality; selection and task creation do not certify current writes. Native/tool/license acceptance remains pending. |
| WM/session | `ryoku/wm`, selected Hyprland bridge | Hyprland remains the initial supported compositor under #25. Niri #46 and phased Tumbleweed Wayfire #185 are distinct retained future feature targets, not exclusions. Their Rust session/provider and native acceptance work stays with those owners; #173 changes no session implementation. |
| Greeter/lock/security | `ryoku/lockscreen/sddm/{ryoku-greeter,ryoku-greeter-guard,ryoku-wayland-session,setup}`, qylock | Retained greeter/session-supervision/lock target under #177 alongside #25 ordinary session integration. These paths are not currently exported/installed; this is an implementation gap. Adapt visuals and behavior to Tumbleweed SDDM/PAM/systemd, preserve passwords/autologin and KDE fallback, and prove timeout/resume/crash guarantees before claiming them. |
| Distribution mechanisms | Fedora/Void/Nix, edition installers/release publishers and package managers | Preserve source/provenance and user-facing feature intent; replace Pacman/AUR/DNF/XBPS, edition boot/init/GRUB/Plymouth/signing and installer mechanisms with reviewed Tumbleweed equivalents. Reference packaging does not authorize its scripts. |
| Apps and install experience | Rashin, Ryostore/Ryovm, Ryotunes/Ryomanager, themes/plugins/rices/palette integrations and installer workflows | Retained full-system target: #182 Store/content/standalone app workflows, #183 Rashin/usage, #47 app inventory, #24 protected native installation, #27–29 package plans/execution/client experience, #180 user configuration. Current export gaps remain explicit and are not permanent feature exclusions. |

Nomarchy's separate `ryoku/shell/nomarchy` host payload is not currently selected.
Retained Nomarchy UI/styling remains part of the full target; Omarchy host
activation is a distribution-specific mechanism requiring a reviewed
Tumbleweed content/configuration equivalent under #180/#182. Its
`nomarchy-activate` blob `fa9c3eb58de928679c7108c6506283b09a143d2d` creates
links/files in `$XDG_BIN_HOME`, `$XDG_CONFIG_HOME/omarchy` and
`$XDG_STATE_HOME/omarchy`. The retained IPC `nomarchy.go` previously resolved
a user or system helper and executed that path. The patch keeps its read-only
availability lookup but fixes the actual executable to the Rust refusal
boundary; a resolved helper is only an opaque refused argument and cannot
be the child executable. Activation/deactivation role and alias negatives
cover both user and system candidate paths. Other core launch paths retain
their existing routes.

Stage's shapes `qmldir` blob `1c21111fbe51b9e8c30294da50030874223c92b9`
declares `ShapeCanvas 1.0 ShapeCanvas.qml` on line 2, but that file is absent
from the exact Git tree. Stage actually instantiates CachedShapeCanvas; its
remaining mentions of ShapeCanvas are comments. The patch removes only the
dangling declaration and retains the module plus every geometry/JS/license
asset. The applied-blob test checks that exact resulting module declaration;
affected MaterialShape/MaterialCookie pass Qt bytecode compilation and the
patched scanner emits no missing-file diagnostics.

The applied patch's exact original blobs are in
`packaging/writer-coordination/pinned-route-sources.json`. The offline pinned
source test hashes each original, applies the full patch with `git apply
--check` and `git apply`, and checks the resulting launch/refusal sites. It
also verifies the unchanged PhoneState/PhoneConnectionTab gate blobs. Binary
tests invoke both the public `ryoku-hub` alias and explicit Rust role and prove
`Unsupported` occurs without native child/setup side effects. These tests do
not substitute for native QML loading or fresh VM acceptance.

All 45 patched QML files pass the signed Qt 6.11.2 `qmlcachegen --bare
--only-bytecode` syntax/bytecode check; the changed Model.js also passes
`node --check`. Qt's formatter aborts on the immutable original Kairos
BluetoothPage as well as its patched copy; it is not used as a passing
validation claim. Import/type resolution is still the separate native gate.

## Fedora comparison and Tumbleweed dependencies

Reviewed immutable Fedora references are `fedora/packages/rpm/ryoku-blobs.spec`,
`ryogami.spec`, `ryoku-hub.spec`, `ryoku-shell.spec`,
`ryoku-desktop-hyprland.spec` and their shared staging recipe. Fedora splits
Blobs/Spatial3D, shell, Hub and Ryogami and also uses package Version 0.1
independently of upstream. It confirms new Quick3D/Multimedia, picker private
Qt GUI/Vulkan/Wayland/FFmpeg and ImageMagick boundaries. Its `ffmpeg-free`,
openh264 workaround, `skwd-paper-bin`, COPRs, shell staging and DNF/system
scriptlets are not openSUSE package evidence and are not imported.

The official 20261016 Tumbleweed OSS `repomd.xml` and signature are retained.
SHA-256 is `1d891e33c8abd582b71e579118e6b4c94e5e57ecdc1b77a2a7fc1abf7cc930a0`;
`gpgv` verified openSUSE Project signer
`AD485664E901B867051AB15F35A2F86E29B700A4`. The Rust `rpm-md-index` verifies
the signed primary archive's compressed and open SHA-512 before indexing
52,976 packages, rejects truncated/duplicate/unsupported records, and retains
exact NEVRA/architecture/checksum/location/provides/requires for selections.

| Required boundary | Exact signed-snapshot package evidence |
|---|---|
| Qt core/declarative/Concurrent | 6.11.2-2.1; `cmake(Qt6Concurrent)` is provided by `qt6-concurrent-devel`, now explicit BuildRequires |
| Multimedia | `qt6-multimedia-imports` 6.11.2-1.2; corresponding development library split retained in package evidence |
| Quick3D | `qt6-quick3d-devel` and `qt6-quick3d-imports` 6.11.2-1.1 |
| Compatibility effects | `qt6-qt5compat-imports` 6.11.2-1.1 |
| Syntax highlighting QML | `kf6-syntax-highlighting-imports` 6.31.0-1.1, rather than assuming the non-import package provides the module |
| Core QML module capabilities | `qtdeclarative-imports-provides-qt6` 1.0-1.55 requires real `qt6-declarative-imports = 6.11.2`; virtual Provides are not proof of installed module files |
| Picker | Qt GUI private headers 6.11.2, Vulkan headers 1.4.363-1.1, Wayland 1.26.0-1.2/protocols 1.49-1.3, pkgconfig FFmpeg libraries |
| Video/images | Snapshot default FFmpeg 9.0.2-4.1; FFmpeg 8 is also present and is not silently assumed to be the selected resolver result. ImageMagick 7.1.2.33-1.1. Codec behavior requires native tests. |

`qml-import-names.txt` covers the selected named-module import graph, including
QtQuick3D, QtMultimedia, QtQuick.Dialogs/Controls.Material, Qt.labs.platform,
Quickshell UPower/SystemTray and the new internal Stage/iNiR module graph.
Signed native qmlimportscanner and Go SDK RPMs were checksum/signature-checked
and extracted into an isolated task SDK. No host RPM installation was used.
Five actual Qt/KDE QML import RPMs have now also been downloaded and each
SHA-512 checked against the signed primary metadata. `rpmkeys --verbose`
verified both header/payload digests and RSA/SHA-512 signatures with key ID
`29b700a4`; exact identities/checksums/locations and signature results are in
`real-import-packages.json` and `import-rpm-signatures.txt`. Their files were
extracted only into the isolated SDK. The patched scanner output is retained
as `qml-import-scanner.json`, with reproducible root placeholders instead of
machine-local paths. All external Qt/KDE modules resolve to real SDK module
directories; source-root Stage/iNiR modules resolve with the actual shell
import roots. No missing-file diagnostic remains after the shapes metadata fix.

The parsed selected Source2/Source3 module inventory includes 13 Quickshell
modules, including the Hyprland bridge import omitted by a Source2-only scan.
The pinned Quickshell source exposes the bridge's GlobalShortcut pressed and
released signals/appid and HyprlandFocusGrab active/windows properties.
`qs.Commons`/`qs.Ui` are runtime aliases, not RPM modules: Quickshell's pinned
`src/core/generation.cpp` adds `qs:@/`, and `qsintercept.cpp` maps `@/qs/` to
the config root. Their exact retained `shell/Commons/qmldir` and
`shell/Ui/qmldir` blobs are respectively
`1c9101c12847259e3b96d5986e5cd529f9d6364c` and
`9061ad6ab71ffcf73570ec7c6dcca08b633a2598`.

The original Quickshell RPM metadata declared twelve static module Provides
and omitted the selected `Quickshell.Services.SystemTray` import. Exact pinned
module/default/static-link evidence is retained in
`quickshell-systemtray-source.json`; the original archive hash was rechecked
before inspecting those files. Quickshell packaging Release 3 adds only that
real module Provide, and desktop-runtime requires `ryoku-quickshell = 0.3.2-3`.
Its source revision and default feature selection are unchanged. #24's older
`0.3.2-2` binary/package evidence retains its historical label. A Rust test
compares all thirteen parsed Quickshell imports with the spec's static
Provides, and the native `rpmspec` test confirms both the generated SystemTray
Provide and matching release dependency. The built-RPM inventory check also
rejects a missing SystemTray provider; native package loading remains pending.

Generated Quickshell/Blobs/Spatial3D/Ryogami module files, runtime alias
loading, final generator requirements/provides and ELF resolution still
require the native package build. Do not add invented internal Provides to
make an unresolved runtime alias or generated module disappear.

## Acceptance and remaining evidence

### Source-review checkpoint (24 October 2026)

Hosted Rust checks [run 40132250850](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40132250850)
completed successfully for exact documentation head
`75eae133bfd22c6d24e9a3f761161b5175c5d66b`. The earlier storage-checkpoint head
`ca033724bd91f51575aa13386cd9aadaa187678d` passed
[run 40130495735](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40130495735).
The earlier documentation head
`b7a51b32bf98ae1a64181e09929edc227b2808ec` passed
[run 40129795018](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40129795018).
The CLI source-review head
`0d70b432600284f8bc50cf9c174a1b92ce49c875` also passed
[run 40129176975](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40129176975).
These results include the full Ryogami preservation handoff. Subsequent
documentation-only status updates do not relabel that run as testing another
commit. The local checks for these review documents are immutable ledger/line
integrity, local links and whitespace; the removed build cache stays removed.

The full Ryogami daemon/editor/livewall/module/alias/UI/readiness payload is
unchanged. No source/package root or feature was removed by the area reviews.
Hub/store has 728 current-file dispositions, Ryogami/Rashin 769, and CLI
lifecycle 774 current/prior paths with 835 original source-site records.
These inventories do not certify the reachable mutation paths. #25 owns core
appearance/settings integration through the existing Rust authorization,
complete capture and linked pre/post mutation contracts; package operations
remain #27–29, service/security scope #31/#32. Retained source/workshop/model/
library/delete operations are owned by [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175);
automatic startup/restore/postapply hooks and timers/watchers by
[#176](https://github.com/GenieRobot/ryoku-opensuse/issues/176). Both require
typed authorization, bounded ownership and linked recovery, preserving full
functionality; their implementation and acceptance remain open.

Other retained feature owners are [#177](https://github.com/GenieRobot/ryoku-opensuse/issues/177)
for greeter/session guard/lock, [#178](https://github.com/GenieRobot/ryoku-opensuse/issues/178)
for Bluetooth link/agent, [#179](https://github.com/GenieRobot/ryoku-opensuse/issues/179)
for Voxtype/Redux, [#180](https://github.com/GenieRobot/ryoku-opensuse/issues/180)
for Hub user/appearance actions, [#181](https://github.com/GenieRobot/ryoku-opensuse/issues/181)
for Hub host/network/security, and [#182](https://github.com/GenieRobot/ryoku-opensuse/issues/182)
for Store/content/standalone apps, [#183](https://github.com/GenieRobot/ryoku-opensuse/issues/183)
for Rashin/usage and [#184](https://github.com/GenieRobot/ryoku-opensuse/issues/184)
for phone/KDE Connect/ADB; #47 remains the application inventory parent.
Niri remains the retained future target #46; phased Wayfire is separately
owned by [#185](https://github.com/GenieRobot/ryoku-opensuse/issues/185).
Their ownership does not certify existing
refused or unmediated paths; their tests and refreshed native gates remain open.

The full verified Git tree preserves CLI source/provenance, but existing
Source2/Source3 are selected exports and contain no `ryoku/cli` root.
Complete CLI source-only archive coverage, per-reconciler/host-verb semantic
proof, and the owned Rust equivalents remain open. Additional #173 gates are
the remaining delta rows, actual WaveCapture binary ownership, picker font
license delivery and complete notices, native RPM/plugin/QML/ELF/tool checks,
and fresh #25/#26 behavior/recovery acceptance. The builder remains paused;
a green hosted Rust run proves none of those native gates.

The coordinator's latest host measurement reports **12.0 GiB free on Windows
C:** and repository `target/` unchanged at **41 GiB**, improving from the prior
9.8 GiB and 4.0 GiB host readings. The existing storage guard is unchanged. The VM
stays paused until the coordinator reassesses the storage gate; this reading
does not authorize a build, install, guest access or target-cache rebuild.
No VM or package state was accessed for this documentation update.

The Rust source/export/index and command-policy checks are independent of
the KVM slot. Sandbox Unix/TCP socket failures are environment limitations,
separate from product results; socket tests must be rerun with ordinary local
process access. The pinned unmodified Go modules use Go 1.26.9, committed
vendor dependencies and networking disabled. IPC, Hyprland and Hub tests pass;
Ryogami requires the
actual image/video tools before a full pass can be claimed.
All four patched Go payload modules also build successfully with that exact
SDK, vendor-only inputs and CGO disabled. This build result is separate from
the unmodified upstream test results; upstream Nomarchy tests expect an
activation capability that this port deliberately refuses. `rpmspec -P`
parses the updated desktop spec successfully; it is not resolver or native
RPM build acceptance.

At the initial source checkpoint, #24 exclusively owned the single KVM slot
and #173 had not used it. The current paused-builder/storage status is recorded
below. Native
RPM/Blobs/Spatial3D/picker builds, full imports/ELF loading, preserved surface
checks and fresh session/recovery acceptance are queued behind that work.
#24's 0.75.5-specific package/VM evidence keeps its original label. #23 remains
historical. #25 and #26 stay open until the refreshed 1.0 package, Hyprland
login/logout/failure return, distro SDDM/PAM/autologin preservation, hyprlock,
KDE fallback and recovery checks pass with exact new artifacts.

License provenance and the CC-BY-SA version limitation are recorded in
`packaging/licenses/ryoku-v1.0/README.md`. PR #174 stays draft/open until every
#173 acceptance item, including native/license closure, is verified.

## Bounded native staging and source-only follow-up

The source candidate was rebased onto merged #24 main
`51e5911635f31b71a05a2bd640306f0f482bc88b`, preserving its installer behavior.
Release `0.1.0-8` distinguishes the refreshed desktop/helper/defaults/tools
content and exact dependencies from #24's historical `0.1.0-7` payload.
Quickshell remains upstream `0.3.2`, packaging Release 3. Rebased candidate
`925fb7129cfc4f86d162583fa62a7c71a24434e3` passed exact-head hosted CI and
the offline workspace/Clippy/source/spec checks before native staging.

The old SDK disk is only read-only backing for a private builder overlay;
its measured identity is in `builder-cache.toml`. Its SDK had the required
Rust/Cargo 1.99 and base Qt 6.11.2, but no cached RPM payloads. A no-refresh
solve against its exact signed 20261016 OSS metadata selected 68 missing
packages, 33,470,129 bytes. Rust `rpm-md-index solver-selection` matches the
full name/architecture/edition against the verified primary index and rejects
other operations/repositories, missing identities, duplicates and count
changes. Its three tests and the real 68-package selection passed before
the host cache cleanup. All 68 downloaded SHA-512s match the signed index;
all 136 RPM header/payload RSA/SHA-512 signatures verify with key ID
`29b700a4`. No repository metadata refresh or unrelated input download was
performed. The existing five Qt/KDE import RPMs remain available for reuse.

Native build/install did not begin: host Windows C: fell below the required
10 GiB free-space guard, despite sufficient guest space. The sole builder
was paused at a 176,619,520-byte overlay; its original backing/firmware and
all source inputs remain preserved. No native build, package install or
runtime result is credited. The deleted host debug cache is not rebuilt
while that guard is unsatisfied. Source-only validation uses formatting,
whitespace and native `rpmspec` metadata checks; full Cargo reruns and native
acceptance wait for storage recovery.

The new native `WaveCapture` reads sink identity using `pactl get-default-sink`
and `pactl list short`, then reads PCM from `pw-record` stdout. These process
dependencies cannot be inferred from ELF scanning. Exact signed-index package
records for `pulseaudio-utils` 17.0-7.6 and `pipewire-tools` 1.6.9-2.1 are
retained in `wavecapture-provider-candidates.json`. That index establishes
package/version/digest/location, but does not retain their complete file
lists. The payloads are not locally cached and the VM is paused, so actual
ownership of `/usr/bin/pactl` and `/usr/bin/pw-record`, solver compatibility
and resulting hard runtime requirements remain unverified. No dependency or
virtual Provide is invented from a package name. `SystemMonitor` only opens
proc/sys/hardware data read-only; Spatial3D accepts absolute local image
paths and generates geometry without persistent writes or process launches.

The retained Stage/iNiR shape license files were also checked directly at
the pinned Git blob `261eeb9e9f8b2b4b0d119366dda99c6fd7d35c64`: both are
Apache-2.0. The runtime license metadata now includes that verified license;
the broader native/license acceptance gate remains open.
