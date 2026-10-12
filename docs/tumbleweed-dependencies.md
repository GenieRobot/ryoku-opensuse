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
  `AD485664E901B867051AB15F35A2F86E29B700A4`. The primary package list was the SHA-512-addressed
  `repodata/9c6719513d5c386b484e36c839e96c8411da417a450fbfa98600aa4e30d2c5410f3033637539d4f6d7013ef24009d73421454d7c588d174d1372b09e082317bb-primary.xml.zst`.
- Package presence and exact `name`, `arch`, `version-release` were read from
  that primary metadata; only `x86_64` rows are listed. These are the versions
  in the inspected snapshot, not timeless minimums or version constraints.
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
| Quickshell runtime | No `quickshell` binary package in the inspected official `oss` metadata | **Not present; blocking.** Ryostore's pinned UI imports `QtQuick`, `Quickshell`, `Quickshell.Io`, and `Ryoku.Ui.Singletons`; standard Qt packages alone do not provide Quickshell APIs or the `Ryoku.Ui` module. No OBS source/package was verified in this inspection. Do not add an arbitrary OBS repo. Either package reviewed Quickshell source in a controlled, signed project and package the Ryoku QML module under explicit RPM ownership, or defer Ryostore UI integration. |
| Qt runtime layer | `libQt6Core6` 6.11.2-2.1; `libQt6Qml6` 6.11.2-2.1; `libQt6Quick6` 6.11.2-2.1; `qt6-wayland` 6.11.2-2.1; `qt6-shadertools` 6.11.2-1.1; `libQt6Svg6` 6.11.2-1.1; `libQt6QuickControls2-6` 6.11.2-2.1 | **Present.** Candidate Qt 6.11.2 runtime for QtQuick/Wayland. Verify exact QML module package split and `qmlimportscanner` results against Quickshell's supported Qt version before building; not a Quickshell compatibility assertion. |
| Portal / Wayland session integration | `xdg-desktop-portal` 1.22.1-2.1; `xdg-desktop-portal-gtk` 1.15.3-1.6; `xdg-desktop-portal-hyprland` 1.4.1-1.2; `libwayland-client0` 1.26.0-1.2 | **Present.** Ryoku's upstream Hyprland recipe requires the Hyprland portal and installs its portal routing config and preview picker integration. Confirm interface and picker executable names at runtime. |
| Shared graphics | `Mesa` 26.2.4-1.1; `Mesa-dri` 26.2.4-1.1; `Mesa-libEGL1` 26.2.4-1.1; `Mesa-libGL1` 26.2.4-1.1 | **Present.** Hardware-specific GPU drivers and Vulkan/VAAPI support remain host-dependent; do not prescribe a single GPU vendor package. Test on the supported VM graphics path and document physical GPU coverage separately. |
| Session services | `sddm` 0.21.0+git57-1.1; `polkit` 127-7.2; `pipewire` 1.6.9-2.1; `wireplumber` 0.5.18-2.1; `systemd` 261.3-1.1 | **Present.** Existing Tumbleweed session/service packages are candidates. Preserve the native KDE login session as recovery/fallback; no Ryoku-owned unit may overwrite files owned by these packages. |
| KDE fallback | `plasma6-session` 6.7.5-1.1; `plasma6-workspace` 6.7.5-1.1; `kwin6` 6.7.5-1.1; `kwin6-devel` 6.7.5-1.1; `xdg-desktop-portal-kde6` 6.7.5-1.1 | **Present.** The KDE portal package is named `xdg-desktop-portal-kde6` in this snapshot. KDE is the fallback session and must be installed/bootable independently of Ryoku; test a full login/logout switch before certifying the path. |

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
verify loading in a disposable session. A failure blocks that plugin, not the
KDE fallback.

For Quickshell, the pinned Ryostore QML itself proves the API imports above but
does not establish a supported Quickshell/Qt version range. Upstream's package
recipe in this pinned revision does not declare a Quickshell version constraint
for this UI. Quickshell compatibility is thus **unresolved** until its upstream
release documentation/source at a pinned revision is checked, the Qt imports
are scanned, and the built Ryostore QML is run against that exact Qt runtime.

## Package source, signature, and ownership rules

1. Production packages come from the official Tumbleweed repositories, or from
   an explicitly reviewed controlled OBS project for packages unavailable
   there. Use only the exact repository URLs recorded in the build recipe and
   signed repository metadata; validate RPM signatures against configured
   openSUSE or project signing keys. Reject unsigned RPMs and signature
   failures. Never set `gpgcheck=0`, `repo_gpgcheck=0`, `--no-gpg-checks`, or
   equivalent bypasses.
2. No arbitrary OBS project/repository, personal home project, AUR package,
   `yay`/`paru`, or Arch package is an allowed fallback. An OBS addition needs a
   named maintainer/project, source URL and immutable source revision, license
   review, reproducible build recipe, dependency closure, key fingerprint
   distribution/rotation/revocation plan, signed repo metadata, and a clean
   disposable-VM install test. No such Quickshell OBS source was verified as
   part of this task; creating or trusting one remains a blocker.
3. RPM ownership is exclusive by path. Official distro RPMs own compositor,
   Qt, Mesa, portal, display manager, and system service files. Ryoku RPMs own
   only `/usr/share/ryoku/**`, Ryoku executables from Rust sources, uniquely
   named Ryoku systemd/session files, and declared Ryoku QML/plugin payloads.
   A controlled Quickshell RPM owns Quickshell runtime files under its upstream
   supported prefix; a separate Ryoku UI RPM owns `Ryoku.Ui` module files.
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
   log into KDE and back to Hyprland. Repeat after a full snapshot update; test
   the unsupported plugin/Quickshell cases fail closed. A clean build is not a
   substitute for desktop, reboot, or recovery acceptance.
4. Current gates: Quickshell has no official OSS package in the inspected
   snapshot and no approved controlled OBS package/source was verified; exact
   Quickshell-to-Qt compatibility remains unknown; plugin package ABI matching
   has not been proven; KDE VM session switching remains unverified. Hosted CI is unavailable per project coordination notes.
   Until these are resolved, no complete Ryoku desktop package set is certified.
