# Ryoku 1.0 upstream delta and openSUSE disposition

Compared the last reviewed/package baseline, `98a0368253d9628be72b853985878a5db90e1547`
(0.75.5-beta.20), with v1.0.0 `a59d051f0e8080052216f6c7b9eb7e5928935ce8`
(tree `06b2962f82f43118c3a98372c2a4b435b689b725`). The comparison contains
6,220 changed paths: 4,930 added, 1,055 modified, 227 deleted, and 8 renamed.
The complete path/status ledger is [`upstream/ryoku-v1.0-path-delta.tsv`](../upstream/ryoku-v1.0-path-delta.tsv).
Upstream's ISO filenames for Arch and CachyOS both carry `a59d051`; they are
release provenance, not alternate openSUSE baselines.

This is a tree-level change inventory for the full Ryoku 1.0 target on
Tumbleweed. Disposition separates current source/package selection from
retained feature implementation and acceptance. Current nonselection/refusal
is a gap to close under its owning task, not a permanent product exclusion;
edition-specific platform mechanisms need reviewed Tumbleweed replacements.

## Change map and required work

| Changed area | Tree-level changes | What changed in 1.0 | openSUSE action and owner |
|---|---:|---|---|
| Shell and Quickshell | 4,354 paths (3,841 added, 346 modified, 163 deleted, 4 renamed), including 3,470 under `ryoku/shell/quickshell` | Extensive shell/QML and service growth; Stage adds `QtQuick3D` and `Ryoku.Spatial3D`; new spatial stage scene, settings, widgets and animation behavior; major startup, service, applet and appearance changes | **Refresh required.** Re-export selected QML; rebuild expanded `Ryoku.Blobs` and new `Ryoku.Spatial3D` native modules; install both complete QML module trees including generated shaders, plugins and metadata; scan every new import and system service call; preserve notices and prove imports. Record omitted upstream paths and rationale. #173, then #25 VM gate. |
| Ryogami / Rashin | 315 changed paths under `ryoku/rashin`; 521 under `ryoku/shell/ryogami` | Rashin web/backend and terminal, usage collectors, systemd integration; expanded Ryogami settings, palette integrations and management interfaces | [Exact current inventory and ownership](ryoku-v1.0-ryogami-rashin-review.md): preserve Ryogami daemon 175 + picker 248 and separate livewall, all package/UI nodes and readiness bindings. Core appearance is #25; retained source/workshop/model/library/delete providers are #175; startup/restore/postapply policy is #176, using #18/#20. All retain full functionality; implementation/native acceptance remains open. All 338 core Rashin paths are verified in Source3 as raw source, with three retained bridges; Source2/runtime installation is still pending; retain Rashin/usage under #183, with #47 the app inventory parent. Native/tool/license gates remain open. |
| CLI and system lifecycle | 292 paths under `ryoku/cli` (159 added, 128 modified, 5 deleted) | Release channels, tracking/rollback, update and doctor behavior, system/WM adapters, plugin compatibility/import, distro-specific package and service operations | [Exact CLI source/adapter review](ryoku-v1.0-cli-lifecycle-review.md): 769 current files and five deleted paths, exact blobs/modes and complete Source3 raw retention without Source2/RPM CLI installation; 835 process/write/probe candidates. Preserve full lifecycle features via typed Rust Tumbleweed equivalents, replacing edition-specific package/root-script operations. Core/status/settings #25; package plans/execution/client #27–29; service/security #31/#32; linked recovery #18–21/#26. Two-run source-only archive verification passes; per-operation implementation/acceptance remain open. |
| Hub and store | 163 Hub paths; 97 app/store paths | Store catalog and bundle model changes; install progress/product types and backend API changes; expanded Hub controls/imports | [Exact current disposition/contract review](ryoku-v1.0-hub-store-review.md): 132 installed Hub QML files/private backend build inputs; 99 Ryostore files not currently selected. Retain the full workflows: #180 user/appearance, #181 host/network/security, #182 Store/content/apps, with #27–29 packages. Catalog/warm writes require typed providers. Implementation/import/ELF/license/native gates remain open. |
| Window managers and integrations | 138 unique changed paths: 117 `ryoku/wm` (including 32 Niri and 42 Wayfire adapter changes), 18 Hyprland config, one standalone Niri config and two standalone Wayfire config | Shared WM adapter grew; Niri support substantially expanded; Wayfire support added; compositor-specific settings/keybinds and plugins changed | [Exact 221-path/API review](ryoku-v1.0-compositor-review.md) separates current source/export/build facts and full targets: #25 Hyprland, #46 Niri, [#185](https://github.com/GenieRobot/ryoku-opensuse/issues/185) Wayfire. Shared client/vendor identity, live preview versus discovery, apply write inventory and helper/package boundaries are recorded; complete per-operation/native acceptance remains open. |
| Lock, greeter, session and sleep security | 14 lockscreen changes plus shell/session/systemd and 49 selected security-surface changes | Greeter renderer/DRM/Weston supervision, SDDM guard, session wrapper, PAM/numlock settling, qylock authentication/watchdog/power, Bluetooth agent and dictation service changes | Retained login/guard/lock target #177 alongside #25 ordinary session work; Bluetooth link/agent #178 and dictation #179. Adapt to Tumbleweed-owned SDDM/PAM/systemd, preserve password/autologin/KDE fallback and test failure/resume/crash guarantees. Current export gaps/refusals are temporary safety facts; only edition-specific platform mechanisms are replaced. |
| Additional applications and bridge integrations | 97 app paths; 18 palette-bridge paths; keysounds/app data | Ryotunes/Ryomanager/Ryostore/Ryovm and palette integrations | Retained full-system workflows under #182 Store/content/apps and #180 configuration, using #27–29 package providers. Keep Ryogami picker/livewall and complete notices/dependencies. Current unselected apps are implementation gaps, not removal from the release target. #24 owns protected native install experience; #183 Rashin/usage and #184 phone retain their explicit owner scopes, with #47 the application inventory parent. |
| Fedora edition | 141 paths across `fedora/packages` and `fedora/system`, plus workflow/translation entries | Experimental Fedora 44/45 RPM edition, specs, source staging, package translations, host adapter and release publisher | **Reference only.** Use the specs to discover package splits and runtime dependencies. Do not import Fedora scriptlets, DNF/system mutation, GRUB/Plymouth hooks, or shell staging as project code. Map only verified Tumbleweed equivalents in this project's declarative RPMs. #173. |
| Void edition and host providers | 151 paths across `void/packages`, `void/init`, and `void/iso`, plus README (152 total) | Experimental XBPS/runit/Turnstile edition, package templates, services and ISO installer | Preserve shared feature/API intent; replace XBPS/runit/Turnstile and edition installer mechanisms with Tumbleweed package/service/installer contracts. Use this edition as source evidence, not a runtime actuator. |
| Nix packaging | 35 paths under `nix/`, plus two root flakes (37 total) | Initial shared application packaging extraction; explicitly not a NixOS module or validated release | Cross-check source/app/package boundaries; retain their feature intent through this port's declarative RPM/provider design rather than importing Nix host mechanisms. |
| Installer and ISO | 62 paths under `installation/{backend,tui,iso,installerplan,tests}`; 29 paths in `ryoku-shell-installer` | Fedora/Void-aware install plans and edition-specific installer paths/tests | Retain the install experience through #24 native protected installation, #182 content/apps and #27–29 package contracts. Replace edition-specific destructive disk/config/package mechanisms with reviewed Tumbleweed equivalents. This source review does not add an ISO implementation or change separate #49 media scope. |
| System integration and hardware | 61 paths under `system/` (hardware, packages, boot, policy, services, extras, containers) | Expanded hardware/platform package maps, boot integration, service and distribution policy | **Review only the Hyprland session dependencies actually shipped.** Do not import another distribution's bootloader, init, hardware mutation or package policy. |
| Release, repository and CI | 77 paths under `release/`; 33 GitHub workflow paths; 17 `bin/` tools | Multi-edition stable/testing releases, signed repositories, manifests, provenance, ISO publication, publisher and maintainer tools, new CI gates | **Provenance reference only.** Do not reuse signing keys, repository URLs, or publisher scripts. Update this project's source pin, build inputs, package metadata and release docs independently. #173, #42–45. |
| Docs, tests, policies and licensing | 42 docs paths, 85 test paths, root policy/tooling metadata; NOTICE modified | New user, developer, release and edition guidance; test coverage; policy and notice changes | **Review licensing and relevant contracts.** Update the source inventory, dependency map, RPM build guide and acceptance docs; retain required upstream notices for every imported file. Upstream tests are evidence of changed behavior, not substitutes for this port's tests/VM acceptance. #173. |

Counts are based on the two exact Git trees and recorded file paths; rename
detection is enabled. The full TSV is authoritative for path coverage. For
security and payload scope, issue #173 must additionally record each retained
path set and each currently unselected set with its retained target/owner, not
just directory totals. Platform mechanism replacement does not remove a feature.

## Packaging consequences

- `StageSpatialScene.qml` is a new `QtQuick3D` import. Candidate openSUSE RPM
  dependencies are `qt6-quick3d-devel` and `qt6-quick3d-imports`; native
  `ryoku/shell/plugin/CMakeLists.txt` also adds Qt `Concurrent` and builds the
  `Ryoku.Spatial3D` module linked to Concurrent, Quick and Quick3D. The v1.0
  `build.sh` copies both `Ryoku/Blobs` and `Ryoku/Spatial3D`; the refreshed RPM
  spec now copies both module trees and generated shaders. #173 must resolve
  exact build/runtime packages from the signed snapshot. Confirm generated
  QML scanner and ELF dependencies, including Concurrent's runtime split.
- The old `ryoku/shell/ryogami/wall-ui` tree is deleted. The desktop spec now
  builds and installs the v1.0 `ryoku/shell/ryogami/picker` QML/C++ module,
  `ryogami-live`, picker desktop entry and license files. Its build adds
  private Qt GUI, Vulkan headers and FFmpeg/Wayland development requirements;
  resolve their exact Tumbleweed providers. The installed daemon also invokes
  `magick`, `ffmpeg` and `ffprobe`, so runtime dependencies and unavailable
  codec/provider behavior need package evidence.
- The reviewed changed QML imports also include `Qt.labs.platform`,
  `QtQuick.Controls.Material`, `QtQuick.Window`, and
  `Quickshell.Services.SystemTray`. Existing declarative-import and Quickshell
  dependencies may cover these; re-run dependency scanning on the refreshed
  payload rather than assuming coverage.
- Fedora 44/45 specs provide a broader package split and helper inventory, but
  Fedora's `Version:` fields and package names do not determine the openSUSE
  product or RPM version. Compare each actual runtime dependency and map it to
  a Tumbleweed package before adding it.
- Source2/Source3 candidate archives, font checksums, writer path selection and
  launch patch have been regenerated against the v1.0.0 tree; their exact
  hashes are in `upstream/ryoku-runtime.toml` and
  `packaging/rpm/desktop-build.md`. Native build, full write-site audit,
  Tumbleweed dependency resolution and package/VM acceptance remain open.
- `ryoku-desktop` RPM `Version: 0.1.0` is this port's package version, not the
  upstream desktop version. Do not change it to `1.0.0` solely because the
  upstream release is 1.0.0.

## Completion state

The CLI source-review head `0d70b432600284f8bc50cf9c174a1b92ce49c875`
passed hosted Rust [run 40129176975](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40129176975).
Documentation head `b7a51b32bf98ae1a64181e09929edc227b2808ec` also passed
[run 40129795018](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40129795018).
Storage-checkpoint head `ca033724bd91f51575aa13386cd9aadaa187678d` passed
[run 40130495735](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40130495735).
Provider/hook ownership head `75eae133bfd22c6d24e9a3f761161b5175c5d66b` passed
[run 40132250850](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40132250850).
All Ryogami source/package/UI nodes remain intact; no new export root or
feature was removed. CLI/Rashin raw archival coverage now passes the two-run proof; protected behavioral/native
acceptance remains open, as detailed in the source review. This CI result
does not complete an area checklist or the combined #173/#25/#26 gate.

The coordinator now reports 12.0 GiB free on host C: and repository `target/`
still at 41 GiB. The VM remains paused pending coordinator storage
reassessment; the prior 10 GiB guard has not been relaxed.

The reproducible candidate exports, immutable file/blob/site evidence,
reviewed command refusals, signed snapshot dependency selections and licensing
dispositions are detailed in [the source review](ryoku-v1.0-source-review.md).

The source-tree path comparison and initial candidate exports are complete.
Per-file semantic/write-site review, exact RPM dependency resolution, native
build/load validation, security boundary review, and VM acceptance are still open in
[#173](https://github.com/GenieRobot/ryoku-opensuse/issues/173). Until those
gates pass, #23 remains historical evidence for its original artifact and
#25/#26 remain open.

The partial [login/lock/Bluetooth source unit](ryoku-v1.0-lock-bluetooth-review.md)
pins 122 paths and 27 API/provenance inputs, with #177/#178/#25 ownership for
PAM/session writes, explicit pairing consent and bond-repair recovery. It
retains the full target and leaves complete semantic/native acceptance open.

The partial [Void/Nix source reference](ryoku-v1.0-void-nix-reference.md) pins
all 189 identities, 72 template/derivation inputs, raw init translation losses
and package mappings. Platform mechanisms remain reference-only; full feature
ownership and semantic/native distribution acceptance remain open.
