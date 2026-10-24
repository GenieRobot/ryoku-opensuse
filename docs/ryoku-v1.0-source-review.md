# Ryoku 1.0 candidate source and command boundary

This records issue #173's candidate at upstream commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`, tree
`06b2962f82f43118c3a98372c2a4b435b689b725`. It is a major upstream source
change. The openSUSE RPM version remains the independently versioned `0.1.0`.
No old VM result is relabelled as acceptance of this source.

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

## Retained and excluded behavior

| Area | Exact selected source surface / behavior | Candidate disposition |
|---|---|---|
| Shell/core | `ryoku/shell/ipc`, selected QML, scripts and user units | Retain native shell, approved Rust writer roles and existing namespace/recovery ownership. Refresh actual v1 unit fixture; use private coordinated Quickshell launcher. Shell state/cache writes remain under the existing managed-writer boundary. |
| Native modules | `ryoku/shell/plugin` | Build both Blobs and new Spatial3D with complete QML metadata, plugins and shaders. No prebuilt upstream binaries. Native loading/ELF closure remains an acceptance gate. |
| Hub queries | `ryoku/hub/backend` dispatch | Rust exact-arity allowlist retains reviewed display/query routes. Unrecognized routes and every unapproved mutation family return `Unsupported` before UID lookup, namespace creation, coordinator setup or native child dispatch. Non-UTF-8 arguments and additional verbs do not bypass the table. |
| Hub writes | config/shell/desktop/WM/output/lock/GPU/CPU/lighting/Voxtype/Rashin/phone/rice/fastfetch/starship/profile/matugen/reload-cover/palette/share/clipboard/import | No mutation provider imported or enabled. Rust default refusal covers aliases and explicit `hub-backend`; table-driven tests cover every dispatch family. Read-only list/status/get variants are individually allowed only where reviewed; the entire phone family is refused. |
| Phone | Hub `phone.go`; shell `PhoneBridge.qml`, `PhoneState.qml`, `KdeConnectService.qml`; Hub `PhoneConnectionTab.qml` | Status/devices are refused, preserving default `installed=false`, `runtimeReady=false` and inactive UI sections even on hosts with KDE Connect installed. Pair/ADB IPC handlers refuse while unavailable. Direct accept/reject/device-call/ADB pair/connect launchers enter Rust refusal; premature accepted/declined feedback is removed. No phone provider implementation. |
| Import | `import.go`, provider import metadata and parser files | `providers` and local detection are display queries. Remote scan, git clone, decisions apply, undo and provider configuration writes are refused. No import/recovery capability is implied by shipping the immutable backend source. |
| Rice/profile | `rice.go`, `profile.go` | List/preflight are queries. Writes to `$XDG_CONFIG_HOME/ryoku/{shell,launcher,widgets,visualizer,decor,brand,ryogami,plugins}.json`, rice directories, wallpaper/config targets and profile export/import are refused. |
| Voxtype/Redux | `voxtype.go`, embedded `assets/redux/{adapter.py,setup.py}` | Get is a query; config/model writes, download/build/setup, service registration/removal and package changes are refused. Direct DictationPage shell deletion/package commands and dock setup/restart launches are replaced by Rust refusal. Embedded Python remains upstream provenance, never a new project provider. |
| Host/package/auth | Hub SettingsSheet, Dictation/Global/Input/Connections/Users/Lockscreen/Updates pages | Direct package install/remove/update, PAM/keyring/fingerprint/security-key writes, system keymap/timezone and service changes enter Rust refusal. Display probes are retained. No pacman/DNF/XBPS or privileged helper is installed. |
| Network/Bluetooth | Selected shell bar/sidebar/Stage/iNiR services and Hub Connections | Direct nmcli profile/password/hotspot mutations, Tailscale changes/operator escalation, battery sysfs/pkexec writes and deferred package actions enter Rust refusal. Direct Quickshell Networking/BlueZ connect/disconnect/forget/trust/power/discovery assignments are replaced before API dispatch. Read-only properties and status/list queries remain. Original Bluetooth link/agent scripts are excluded. |
| Ryogami | `ryoku/shell/ryogami/{daemon,picker,hyprland,bin}` | Retain daemon, picker and livewall helper for existing wallpaper contract. Deleted `wall-ui` is not packaged. Image/video transforms require actual ImageMagick/FFmpeg evidence; source compilation alone is insufficient. |
| WM/session | `ryoku/wm`, selected Hyprland bridge | Hyprland remains the initial compositor. Niri and Wayfire provider activation is deferred to #46. The port's Rust session adapter remains authoritative. |
| Greeter/lock/security | Upstream greeter, `sddm/setup`, guard, custom qylock, Bluetooth agent/link | Exact excluded path families are outside the source export/install recipes. Do not import root display-manager watchdog/restart, PAM/autologin rewriting, custom lock guarantees, or re-pairing behavior. Native hyprlock and distro SDDM/KDE fallback require fresh #25 acceptance. |
| Other distributions/apps | Fedora/Void/Nix, installer/ISO/release publishers, standalone Rashin/store/apps/palette bridge | Reference/excluded source only. No alternate package manager, boot/init/GRUB/Plymouth mutation, installer, repository signing/publisher, model capability or standalone app scope is imported. |

Nomarchy's separate `ryoku/shell/nomarchy` host payload is excluded. Its
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

#24 exclusively owns the single KVM slot. #173 has not used it. Native
RPM/Blobs/Spatial3D/picker builds, full imports/ELF loading, preserved surface
checks and fresh session/recovery acceptance are queued behind that work.
#24's 0.75.5-specific package/VM evidence keeps its original label. #23 remains
historical. #25 and #26 stay open until the refreshed 1.0 package, Hyprland
login/logout/failure return, distro SDDM/PAM/autologin preservation, hyprlock,
KDE fallback and recovery checks pass with exact new artifacts.

License provenance and the CC-BY-SA version limitation are recorded in
`packaging/licenses/ryoku-v1.0/README.md`. PR #174 stays draft/open until every
#173 acceptance item, including native/license closure, is verified.
