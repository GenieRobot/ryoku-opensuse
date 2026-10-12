# Tumbleweed dependency and version map

Inspected 2026-10-12. This is a source inventory, not a claim that Ryoku is
buildable or installable on Tumbleweed. Package availability below comes from
the official Tumbleweed `oss` repository metadata for snapshot `20261011`;
compatibility still requires the build and VM gates at the end of this document.

## Evidence and method

- Official repository: [Tumbleweed OSS](https://download.opensuse.org/tumbleweed/repo/oss/)
- Inspected `repomd.xml` on 2026-10-12. It identifies `openSUSE Tumbleweed
  20261011`, revision `1791791893`. The downloaded `repomd.xml.asc` signature verified successfully against the
  `repomd.xml.key` published beside it; its primary-key fingerprint is
  `AD485664E901B867051AB15F35A2F86E29B700A4`. The primary package list was
  the SHA-512-addressed
  `repodata/9c6719513d5c386b484e36c839e96c8411da417a450fbfa98600aa4e30d2c5410f3033637539d4f6d7013ef24009d73421454d7c588d174d1372b09e082317bb-primary.xml.zst`.
- Package presence and exact `name`, `arch`, `version-release`, summary,
  description, provides, and binary requirements were read from that primary
  metadata. The versioned entries below comprise 36 x86_64 and 2 noarch
  packages. These are the versions in this snapshot, not timeless minimums or
  version constraints.
- Upstream source is the immutable Ryoku revision recorded in
  [`upstream/ryoku.source.toml`](../upstream/ryoku.source.toml):
  [`98a0368253d9628be72b853985878a5db90e1547`](https://github.com/Ryoku-dev/ryoku/commit/98a0368253d9628be72b853985878a5db90e1547),
  inspected 2026-10-12. Relevant source paths include
  [`ryoku/apps/ryostore/quickshell/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell),
  [`ryoku/hyprland/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/ryoku/hyprland),
  [`docs/hyprland-plugins.md`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/docs/hyprland-plugins.md),
  and the upstream `release/packages/ryoku-desktop-hyprland/PKGBUILD` and
  `release/packages/ryoku-hypr-plugins/PKGBUILD`.

## x86_64 package and compatibility matrix

“Verified present” means the binary package appears in the stated official
snapshot metadata. It does not prove that an upstream config works with that
binary. Names and roles are tied to the pinned source, package metadata, or
upstream package recipe as cited above.

| Role | Official Tumbleweed package(s), snapshot version-release | Status and disposition |
|---|---|---|
| Hyprland compositor | `hyprland` 0.56.2-1.2; `hyprland-devel` 0.56.2-1.2 | **Present, compatibility unresolved.** Candidate initial compositor. Record ABI inputs and run the plugin checks below before enabling any plugin. |
| Hyprland plugins | `hyprland-plugin-hyprbars`, `hyprland-plugin-hyprfocus`, `hyprland-plugin-borders-plus-plus` each 0.56.0-1.1 | **Present, ABI unresolved.** Exact package names are confirmed in repo metadata. The upstream Ryoku recipe builds `hyprbars` and `hyprfocus`; do not assume the packaged .so ABI matches `hyprland` 0.56.2-1.2 merely because package versions look close. Validate Hyprland ABI hash with each plugin or rebuild from reviewed sources against the exact compositor build. A mismatched plugin stays disabled. |
| Other Hyprland runtime components | `hyprpolkitagent` 0.2.0-1.1; `hypridle` 0.1.8-1.1; `hyprlock` 0.9.6-1.1; `hyprpaper` 0.8.4-1.2; `hyprpicker` 0.4.7-1.2; `hyprsunset` 0.4.0-1.3; `hyprland-qtutils` 0.1.5-2.13; `hyprland-qt-support` 0.1.0-2.3 | **Present.** Roles are compositor policy agent, idle/lock/wallpaper/picker/night-light helpers and Qt integration by upstream project naming. Exact Ryoku config integration is unverified; install only helpers required by reviewed config. |
| Quickshell runtime candidate | `noctalia-qs` 0.0.12-2.5 x86_64; package metadata provides `quickshell = 0.0.12`; package description identifies `qs` as a drop-in replacement for upstream `quickshell` | **Official package present; Ryoku compatibility unresolved.** This is the Noctalia-maintained Quickshell fork in official `oss`, not the upstream Quickshell build. Its metadata describes `qs` as a drop-in replacement for the upstream `quickshell` binary and lists `quickshell = 0.0.12` as a provide. The RPM requires Qt 6.11.2 private API symbols for Gui, Qml, Quick, and WaylandClient. Ryostore QML imports `QtQuick`, `Quickshell`, `Quickshell.Io`, and `Ryoku.Ui.Singletons`; no runtime test establishes the fork supports all Ryoku shell APIs. Use only as a candidate from signed official Tumbleweed; any Qt ABI update requires a matching rebuilt package or a tested compatible binary. The `Ryoku.Ui` module remains a Ryoku-owned payload. |
| Qt runtime layer | `libQt6Core6` 6.11.2-2.1; `libQt6Qml6` 6.11.2-2.1; `libQt6Quick6` 6.11.2-2.1; `qt6-wayland` 6.11.2-2.1; `qt6-shadertools` 6.11.2-1.1; `libQt6Svg6` 6.11.2-1.1; `libQt6QuickControls2-6` 6.11.2-2.1 | **Present.** Candidate Qt 6.11.2 runtime for QtQuick/Wayland. Verify exact QML module package split and `qmlimportscanner` results against Quickshell's supported Qt version before building; not a Quickshell compatibility assertion. |
| Portal / Wayland session integration | `xdg-desktop-portal` 1.22.1-2.1; `xdg-desktop-portal-gtk` 1.15.3-1.6; `xdg-desktop-portal-hyprland` 1.4.1-1.2; `libwayland-client0` 1.26.0-1.2 | **Present.** Ryoku's upstream Hyprland recipe requires the Hyprland portal and installs its portal routing config and preview picker integration. Confirm interface and picker executable names at runtime. |
| Shared graphics | `Mesa` 26.2.4-1.1; `Mesa-dri` 26.2.4-1.1; `Mesa-libEGL1` 26.2.4-1.1; `Mesa-libGL1` 26.2.4-1.1 | **Present.** Hardware-specific GPU drivers and Vulkan/VAAPI support remain host-dependent; do not prescribe a single GPU vendor package. Test on the supported VM graphics path and document physical GPU coverage separately. |
| Ryoku shell, UI, and session payload | No matching Ryoku package is published in Tumbleweed; pinned upstream source paths are identified in [the #11 inventory](upstream-inventory.md). These comprise Quickshell QML surfaces, `Ryoku.Ui` modules, app backends, and systemd/SDDM session integration. | **Port/package work unresolved.** Build only reviewed source at the recorded upstream pin, retaining upstream language where permitted and writing new project-owned executables in Rust. A Ryoku RPM may own only declared Ryoku data, QML modules, binaries, and uniquely named session units. Do not overwrite distro-owned compositor, SDDM, Qt, KDE, or systemd files. Full shell and session behavior remains blocked on compatibility and VM evidence. |
| Session services | `sddm` 0.21.0+git57-1.1; `polkit` 127-7.2; `pipewire` 1.6.9-2.1; `wireplumber` 0.5.18-2.1; `systemd` 261.3-1.1 | **Present.** Existing Tumbleweed session/service packages are candidates. Preserve the native KDE login session as recovery/fallback; no Ryoku-owned unit may overwrite files owned by these packages. |
| KDE fallback | `plasma6-session` 6.7.5-1.1; `plasma6-workspace` 6.7.5-1.1; `kwin6` 6.7.5-1.1; `xdg-desktop-portal-kde6` 6.7.5-1.1 | **Present.** The KDE portal package is named `xdg-desktop-portal-kde6` in this snapshot. KDE is the fallback session and must be installed/bootable independently of Ryoku; test a full login/logout switch before certifying the path. |
| Upstream Ryoku Hyprland add-ons (not in official OSS) | Exact-name search of signed metadata found none of: `hypr-dynamic-cursors`, `hyprglass`, `imgborders`, `ryoku-keysounds`, `hyprland-preview-share-picker` | **Unavailable from the inspected official repo; block those features pending an explicit provider.** Do not make installable Hyprland depend on those five features by default. Candidate plan: put these in a separate, opt-in OBS project under a project-owned namespace (no `home:` personal project; this OBS project does not yet exist) with project signing key, signed repository metadata, immutable sources, source and license review, reproducible source RPM/specs, exact `hyprland-devel` build dependency and tested RPM ownership. `hypr-dynamic-cursors` (GPL-3.0-or-later, VirtCode upstream) and `hyprglass` (GPL-3.0-or-later, hyprnux upstream) must resolve an immutable source commit compatible with the selected Hyprland ABI; upstream's Arch recipes permit a moving-HEAD fallback, which this project must reject. `imgborders` is GPL-3.0-or-later and the pinned upstream recipe carries a git bundle with SHA-256 `48187e72ca62fc0b8d12809ca02082b8d27016516d84527a19abb3ff007d6a7d`; verify this bundle and review its pin before using. `ryoku-keysounds` carries GPL-3.0-or-later plugin source and MIT-licensed Mechvibes samples at upstream commit `326252a13e7bef4f1c35d08ef0189b5af6f8ba02`; retain both notices and treat sample/profile payload as separately owned `/usr/share/ryoku/keysounds/**`. The preview picker is MIT-licensed Rust software at upstream commit `e2f30ff85486e557018523da45ccbc846e3a499c`; also pin and audit its Hyprland protocols submodule and Cargo.lock. All five produce Hyprland plugins or compositor integration and remain disabled/deferred until ABI and runtime checks pass. |

### Source compatibility assessment

Hyprland 0.56.2-1.2 is a candidate, not yet a validated desktop version. The
pinned upstream plugin document states that compositor plugins depend on
Hyprland's ABI hash, incorporating its commit and major/minor versions of
Aquamarine, hyprutils, hyprgraphics, hyprcursor, and hyprlang. This is an ABI
boundary, not something inferred from equal package names. Upstream's own
`ryoku-hypr-plugins` recipe reads the installed Hyprland version header to form
that hash and builds `hyprbars`/`hyprfocus` from `hyprwm/hyprland-plugins`.
The Tumbleweed binary plugin packages are therefore only candidates; compare
`hyprctl version -j` ABI data with each package's `.abi` receipt where shipped,
or rebuild pinned reviewed sources using the exact `hyprland-devel` headers and
verify loading in a disposable session. A failure blocks that plugin, not the KDE fallback.

For Quickshell, the signed snapshot provides the `noctalia-qs` 0.0.12-2.5
candidate and its metadata explicitly describes it as a Quickshell fork/drop-in.
Its Qt 6.11.2 private API requirements make compatibility snapshot-specific.
The metadata proves package dependencies and virtual provide, not API behavior
for Ryoku: source behavior, `qs` launch flags, imported modules, `Ryoku.Ui`,
and full shell/session behavior still need source inspection and a disposable
VM test. If those checks fail, do not substitute an arbitrary OBS package;
use a controlled signed build from pinned upstream Quickshell sources or defer
the affected UI. An eventual Qt bump requires re-evaluation/rebuild of the candidate as part of the same tested update set.

The upstream `ryoku-desktop-hyprland` recipe has hard dependencies on the five
add-ons grouped in the matrix row. None was found by exact name in this
snapshot. The official `hyprland-plugin-hyprbars` and `hyprland-plugin-hyprfocus`
packages do not replace those five packages. Keep the baseline compositor
usable without optional extras; do not publish a Ryoku Hyprland metapackage
that promises unavailable features. The controlled OBS plan above records the
minimum provenance, license, ownership, and ABI gates for each source. The
current project has no established OBS project, key, or validated package
recipes, so these add-ons are unresolved.

The pinned Ryoku revision includes session units, launchers and QML plus native
upstream app backends; #11 records their Arch-specific assumptions and calls
for Rust adapter/replacement of package-facing Go backends. Retain upstream QML
only after protocol/compatibility review, and retain other upstream code only
under the roadmap's language/licensing rules; new project-owned executables
remain Rust. RPM packaging must enumerate
those exact paths and avoid distro-owned files. No such Ryoku RPM or session
payload has yet passed this build or VM validation route.

## Package source, signature, and ownership rules

1. Production packages come from the official Tumbleweed repositories, or from
   an explicitly reviewed controlled OBS project for packages unavailable
   there. Use only the exact repository URLs recorded in the build recipe and
   signed repository metadata; validate RPM signatures against configured
   openSUSE or project signing keys. Reject unsigned RPMs and signature
   failures. Never set `gpgcheck=0`, `repo_gpgcheck=0`, `--no-gpg-checks`, or
   equivalent bypasses.
2. The `noctalia-qs` candidate is consumed from the signed official Tumbleweed
   repository; this does not authorize adding a new repo. No arbitrary OBS
   project/repository, personal home project, AUR package, `yay`/`paru`, or Arch
   package is an allowed fallback. An OBS addition needs a
   named maintainer/project, source URL and immutable source revision, license
   review, reproducible build recipe, dependency closure, key fingerprint
   distribution/rotation/revocation plan, signed repo metadata, and a clean
   disposable-VM install test. A controlled project-owned OBS namespace/project does not yet exist; creating,
   reviewing, and signing it remains a blocker before those packages can be
   consumed.
3. RPM ownership is exclusive by path. Official distro RPMs own compositor,
   Qt, Mesa, portal, display manager, and system service files. Ryoku RPMs own
   only `/usr/share/ryoku/**`, Ryoku executables from Rust sources, uniquely
   named Ryoku systemd/session files, and declared Ryoku QML/plugin payloads.
   The official `noctalia-qs` RPM owns its own runtime paths. A separate Ryoku UI
   RPM owns `Ryoku.Ui` module files.
   No Ryoku package may overwrite `/usr/bin/{Hyprland,kwin_wayland,sddm}`, Qt
   libraries, vendor portal files, generic systemd units, or KDE-owned session
   entries. RPM file lists and transaction conflict checks must prove this.
4. Any Ryoku Hyprland `.so` plugin is built from reviewed, pinned source
   against the exact shipped Hyprland-devel package and belongs to a versioned
   Ryoku plugin RPM tied to that compositor build. It must carry a recorded ABI
   receipt. No network-at-login plugin fetch/rebuild and no plugin is enabled
   when its ABI differs.

## Build and validation route; blockers

1. Freeze a Tumbleweed snapshot URL and its verified signing key for each build;
   include the RPM-MD repo checksum/revision in build evidence. Build in a
   controlled OBS project or reproducible local `osc build` against that
   snapshot. Inspect spec dependency closure with `rpmspec`, perform clean
   package builds, then `rpm --checksig` every artifact. Recheck repo metadata
   signatures after publication.
2. Keep official distro dependencies as exact package dependencies where the
   API/ABI must match (notably Hyprland plugins). Generate and review RPM file
   lists, verify no path collisions using RPM transaction checks, and scan
   installed QML imports (`qmlimportscanner`) for undeclared modules.
3. In a disposable x86_64 Tumbleweed VM, install only signed packages, log into
   Hyprland, exercise compositor config, portal screen share, audio, lock,
   sleep/resume, and log out. Verify plugin ABI receipts/runtime loading. Then
   log into KDE and back to Hyprland. Repeat after a full snapshot update; test unresolved add-on ABIs fail closed and verify the Quickshell fork
   against Ryoku surfaces. A clean build is not a substitute for desktop, reboot, or recovery acceptance.
4. Current gates: `noctalia-qs` is a signed official package candidate, but its
   Qt 6.11.2 private API coupling and Ryoku API/runtime compatibility are not
   verified. The five upstream hard-dependency add-ons are absent from official
   OSS; a controlled OBS project/key/source review and ABI-matched builds are
   not established. KDE/Hyprland VM session switching remains unverified.
   Hosted CI is unavailable per project coordination notes. Until these are
   resolved, no complete Ryoku desktop package set is certified.
