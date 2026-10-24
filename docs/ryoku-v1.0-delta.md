# Ryoku 1.0 upstream delta and openSUSE disposition

Compared the last reviewed/package baseline, `98a0368253d9628be72b853985878a5db90e1547`
(0.75.5-beta.20), with v1.0.0 `a59d051f0e8080052216f6c7b9eb7e5928935ce8`
(tree `06b2962f82f43118c3a98372c2a4b435b689b725`). The comparison contains
6,220 changed paths: 4,930 added, 1,055 modified, 227 deleted, and 8 renamed.
The complete path/status ledger is [`upstream/ryoku-v1.0-path-delta.tsv`](../upstream/ryoku-v1.0-path-delta.tsv).
Upstream's ISO filenames for Arch and CachyOS both carry `a59d051`; they are
release provenance, not alternate openSUSE baselines.

This is a tree-level change inventory, not a claim that every upstream feature
belongs in the initial Tumbleweed port. Disposition records whether the area
must be refreshed now, reviewed for the initial desktop, deferred, or excluded.

## Change map and required work

| Changed area | Tree-level changes | What changed in 1.0 | openSUSE action and owner |
|---|---:|---|---|
| Shell and Quickshell | 4,354 paths (3,841 added, 346 modified, 163 deleted, 4 renamed), including 3,470 under `ryoku/shell/quickshell` | Extensive shell/QML and service growth; Stage adds `QtQuick3D` and `Ryoku.Spatial3D`; new spatial stage scene, settings, widgets and animation behavior; major startup, service, applet and appearance changes | **Refresh required.** Re-export selected QML; rebuild expanded `Ryoku.Blobs` and new `Ryoku.Spatial3D` native modules; install both complete QML module trees including generated shaders, plugins and metadata; scan every new import and system service call; preserve notices and prove imports. Record omitted upstream paths and rationale. #173, then #25 VM gate. |
| Ryogami / Rashin | 315 paths under `ryoku/rashin`; 521 under `ryoku/shell/ryogami` | Rashin web/backend and terminal, usage collectors, systemd integration; expanded Ryogami settings, palette integrations and management interfaces | **Review retained payload.** Identify which apps are in the existing RPM payload, update source/manifests and dependencies for those; defer separate Rashin/companion capabilities unless required by #25. No unreviewed network/model or host-control actions. #173, #27–30, #47. |
| CLI and system lifecycle | 292 paths under `ryoku/cli` (159 added, 128 modified, 5 deleted) | Release channels, tracking/rollback, update and doctor behavior, system/WM adapters, plugin compatibility/import, distro-specific package and service operations | **Adapter contract refresh.** Keep project-owned implementation Rust; update upstream command/API inventory. Explicitly exclude pacman, AUR, DNF and XBPS actuators from this port; route only accepted operations through existing openSUSE provider/recovery contracts. #173, #27–30, #1. |
| Hub and store | 163 Hub paths; 97 app/store paths | Store catalog and bundle model changes; install progress/product types and backend API changes; Hub surfaces and controls expanded. New QML imports include `Quickshell.Services.UPower`; Ryostore adds `QtMultimedia` and `QtQuick.Controls`; `ryoku/ui` adds `QtQuick.Dialogs`. | **Protocol review required.** Reconcile shipped UI against the port's Rust provider before any supported install/remove action. Preserve non-package catalog presentation only when it cannot trigger unsupported actions. Confirm these framework modules are in the selected RPM closure through generated QML and ELF dependency metadata. #173, #27–30. |
| Window managers and integrations | 117 `ryoku/wm` paths; 18 `ryoku/hyprland`; new Niri (32 paths) and Wayfire (42 paths) trees; new Wayfire data | Shared WM adapter grew; Niri support substantially expanded; Wayfire support added; compositor-specific settings/keybinds and plugins changed | **Initial release stays Hyprland.** Re-audit Hyprland config/provider references used by #25. Track Niri and Wayfire as deferred compatibility work under #46; do not silently broaden release support. |
| Lock, greeter, session and sleep security | 14 `ryoku/lockscreen` paths, plus shell systemd/session changes and new SDDM guard files/tests; 49 changed files in the selected lock/session/script/unit surface | Greeter now selects DRM card and renderer conditions, records stderr, supervises Weston, and changes XKB/numlock/display settling. New root `ryoku-greeter-guard` watches SDDM helpers/login1 events and restarts SDDM after a 20-second no-helper window (up to three restarts per boot). Session imports systemd user environment, materializes user state, waits for DRM, supervises compositor and reports failure to SDDM. `sddm/setup` changes PAM keyring/autologin handling and host session state. qylock fixes PAM conversation, empty/hung auth and watchdog/power paths. Shell unit adds optional Voxtype ensure, removes AI usage timer and adds Bluetooth agent unit. | **Security review required before retaining any part.** Exclude upstream install-time `/etc/pam.d/sddm`, SDDM, display-target and service mutations from RPM scriptlets. Review exact Rust-owned equivalents and imported QML against Tumbleweed SDDM/PAM, authenticated lock/recovery handoff and KDE fallback. Test guard restart caps/races, PAM preservation/autologin, XKB, multiple outputs, compositor death/logout, and suspend/resume fail-closed behavior. #173, #25, #36–41. |
| Additional applications and bridge integrations | 97 app paths; 18 palette-bridge paths; new keysounds and app data | Ryotunes, Ryomanager, Ryostore/Ryovm and other app packaging; palette bridge integrations for Vesktop/Zen/Spotify; optional app and key-sound assets | **Inventory payload closure.** Refresh manifests/notices for shipped files. Defer standalone apps and their package/config writers to #47 or the owning M3 task; no new home-file mutation without scoped ownership/recovery rules. |
| Fedora edition | 141 paths across `fedora/packages` and `fedora/system`, plus workflow/translation entries | Experimental Fedora 44/45 RPM edition, specs, source staging, package translations, host adapter and release publisher | **Reference only.** Use the specs to discover package splits and runtime dependencies. Do not import Fedora scriptlets, DNF/system mutation, GRUB/Plymouth hooks, or shell staging as project code. Map only verified Tumbleweed equivalents in this project's declarative RPMs. #173. |
| Void edition and host providers | 151 paths across `void/packages`, `void/init`, and `void/iso` | Experimental XBPS/runit/Turnstile edition, package templates, services and ISO installer | **Out of scope.** Do not port XBPS/runit/installer code. Use only to identify shared UI assets or protocol behavior where useful. |
| Nix packaging | 35 paths across `nix/`, the flake and assets | Initial shared application packaging extraction; explicitly not a NixOS module or validated release | **Out of scope for RPM implementation.** Use as a cross-check for app boundaries only. |
| Installer and ISO | 62 paths under `installation/{backend,tui,iso,installerplan,tests}`; 29 paths in `ryoku-shell-installer` | Fedora/Void-aware install plans and edition-specific installer paths/tests | **Unsupported for this project.** Continue requiring users to install openSUSE independently; do not import destructive upstream installer paths or claim ISO support (#49 is deferred). |
| System integration and hardware | 61 paths under `system/` (hardware, packages, boot, policy, services, extras, containers) | Expanded hardware/platform package maps, boot integration, service and distribution policy | **Review only the Hyprland session dependencies actually shipped.** Do not import another distribution's bootloader, init, hardware mutation or package policy. |
| Release, repository and CI | 77 paths under `release/`; 33 GitHub workflow paths; 17 `bin/` tools | Multi-edition stable/testing releases, signed repositories, manifests, provenance, ISO publication, publisher and maintainer tools, new CI gates | **Provenance reference only.** Do not reuse signing keys, repository URLs, or publisher scripts. Update this project's source pin, build inputs, package metadata and release docs independently. #173, #42–45. |
| Docs, tests, policies and licensing | 42 docs paths, 85 test paths, root policy/tooling metadata; NOTICE modified | New user, developer, release and edition guidance; test coverage; policy and notice changes | **Review licensing and relevant contracts.** Update the source inventory, dependency map, RPM build guide and acceptance docs; retain required upstream notices for every imported file. Upstream tests are evidence of changed behavior, not substitutes for this port's tests/VM acceptance. #173. |

Counts are based on the two exact Git trees and recorded file paths; rename
detection is enabled. The full TSV is authoritative for path coverage. For
security and payload scope, issue #173 must additionally record each retained
path set and each excluded set, not just directory totals.

## Packaging consequences

- `StageSpatialScene.qml` is a new `QtQuick3D` import. Candidate openSUSE RPM
  dependencies are `qt6-quick3d-devel` and `qt6-quick3d-imports`; native
  `ryoku/shell/plugin/CMakeLists.txt` also adds Qt `Concurrent` and builds the
  `Ryoku.Spatial3D` module linked to Concurrent, Quick and Quick3D. The v1.0
  `build.sh` copies both `Ryoku/Blobs` and `Ryoku/Spatial3D`; the current RPM
  installs only four Blobs files. #173 must update the Source2 CMake tree and
  spec install closure together, include generated Blobs shaders, and resolve
  exact build/runtime packages from the signed snapshot. Confirm generated
  QML scanner and ELF dependencies, including Concurrent's runtime split.
- The reviewed changed QML imports also include `Qt.labs.platform`,
  `QtQuick.Controls.Material`, `QtQuick.Window`, and
  `Quickshell.Services.SystemTray`. Existing declarative-import and Quickshell
  dependencies may cover these; re-run dependency scanning on the refreshed
  payload rather than assuming coverage.
- Fedora 44/45 specs provide a broader package split and helper inventory, but
  Fedora's `Version:` fields and package names do not determine the openSUSE
  product or RPM version. Compare each actual runtime dependency and map it to
  a Tumbleweed package before adding it.
- The current `Source2`, `Source3`, Quickshell source and writer patch are still
  pinned to 0.75.5. The new source pin in `upstream/ryoku.source.toml` records
  reviewed release provenance; `upstream/ryoku-runtime.toml` remains the
  current artifact manifest until #173 refreshes the payload and evidence.
- `ryoku-desktop` RPM `Version: 0.1.0` is this port's package version, not the
  upstream desktop version. Do not change it to `1.0.0` solely because the
  upstream release is 1.0.0.

## Completion state

The source-tree comparison is complete at directory and path level. Payload
closure, per-file retention/exclusion review, exact RPM dependency resolution,
patch regeneration, security review, and VM acceptance are still open in
[#173](https://github.com/GenieRobot/ryoku-opensuse/issues/173). Until those
gates pass, #23 remains historical evidence for its original artifact and
#25/#26 remain open.
