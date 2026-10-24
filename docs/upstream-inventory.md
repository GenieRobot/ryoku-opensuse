# Upstream source inventory

This inventory tracks the latest reviewed upstream release and the separate
source revision currently represented by packaged runtime artifacts. The
immutable source review pin, repository identity, observed ref, and inspection date are recorded in
[`upstream/ryoku.source.toml`](../upstream/ryoku.source.toml). The inventory
links below point at that exact commit; links to upstream's moving `main`
branch are not source pins. The detailed component table below preserves the
previous 0.75.5 package inventory and immutable links; issue #173 records the
component-by-component 1.0 delta before those mappings are treated as current.

## Ryoku Amaterasu 1.0 review (24 October 2026)

The current source review pin is v1.0.0 commit
[`a59d051f0e8080052216f6c7b9eb7e5928935ce8`](https://github.com/Ryoku-dev/ryoku/commit/a59d051f0e8080052216f6c7b9eb7e5928935ce8),
tree `06b2962f82f43118c3a98372c2a4b435b689b725`. The 2026-10-24 Arch and
CachyOS ISO filenames both carry this source prefix. Those distribution images
are provenance clues only; this project continues to target openSUSE Tumbleweed.

The release adds Fedora 44/45 RPM packaging and a `QtQuick3D` import in
`ryoku/shell/quickshell/shell/modules/stage/StageSpatialScene.qml`. Fedora's
specs are useful for package split and dependency discovery, but include Fedora
scriptlets, package-manager assumptions, and privileged host workflows. They
are not port implementations. The openSUSE desktop spec now declares the Qt
Quick 3D build and QML runtime dependencies; exact package resolution and
runtime import validation remain part of issue #173.

The v1.0 release also changes the SDDM session wrapper, setup path, shell
session startup, and adds a greeter guard. These files require a fresh security
and compatibility review against this project's Rust service, authenticated
recovery, and KDE fallback requirements. The store/backend and updater changes
remain future adapter work; no upstream package actuator is imported.

**Important:** the v1.0 source review pin does not mean the shipped runtime
payload has been refreshed. `upstream/ryoku-runtime.toml`, the generated
`Source2` and `Source3` archives, and issue 18 writer patch still describe
0.75.5 at `98a0368253d9628be72b853985878a5db90e1547`. Issue #173 owns the full
payload, patch, session, and fresh Tumbleweed acceptance refresh. Preserve the
old #23 package result as historical evidence for its exact inputs.

## Previous package baseline and provenance

- Repository: [Ryoku-dev/ryoku](https://github.com/Ryoku-dev/ryoku), the
  upstream project's single-source-of-truth repository for an Arch-based
  distribution, desktop, system definition, and installer.
- Previously inspected package revision:
  [`98a0368253d9628be72b853985878a5db90e1547`](https://github.com/Ryoku-dev/ryoku/commit/98a0368253d9628be72b853985878a5db90e1547),
  the `main`/HEAD observed on 2026-10-12. This commit's parent is
  `f863d5ba52a08aef0f0af164f4cd45140f7f47ab`; its subject is
  `release: bump unstable to 0.75.5-beta.20`.
- The upstream root [README](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/README.md)
  describes Ryoku as an Arch Linux distribution and declares GPL-3.0. The
  pinned [LICENSE](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/LICENSE)
  is GNU GPL version 3. The upstream [NOTICE](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/NOTICE)
  and applicable per-file notices travel with any imported source or assets.
- No upstream source is vendored by this inventory. The project-level GPL-3.0
  license is already present; it does not replace notices or licensing files
  accompanying a future imported component.

## Component and interface inventory

“Disposition” states what this openSUSE project may carry forward. It does not
claim that a mapped feature is implemented. The current workspace foundation
contains only Rust crate boundaries; adapters remain future work.

| Area | Pinned upstream files and concrete interface | Arch assumptions observed | openSUSE disposition |
|---|---|---|---|
| Installer and live image | [`installation/tui/main.go`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/installation/tui/main.go), [`installation/backend/ryoku-install`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/installation/backend/ryoku-install), [`installation/backend/lib/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/installation/backend/lib), and [`installation/iso/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/installation/iso). The TUI gathers install options; the backend orchestrates partitioning, encryption, filesystem setup, `pacstrap`, chroot configuration, and desktop deployment. | Arch ISO/archiso, `pacstrap`, `arch-chroot`, pacman, Limine and Arch package sets; backend and TUI are Bash and Go. | **Unsupported.** This project does not build an OS installer or ISO. Users install supported openSUSE Tumbleweed through openSUSE's installer. Do not port or expose the Arch destructive installer. |
| User CLI | [`ryoku/cli/main.go`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/main.go), [`ryoku/cli/internal/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/internal), and the upstream [CLI reference](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/docs/cli.md). Public command families include update/status, deploy/materialize, recovery, doctor, and shell controls; CLI subcommands call system tools and persist local state. | Go CLI dispatches to pacman/yay, Arch-specific filesystem and service paths, and compositor tools. | **Rust adapter/replacement.** Implement only accepted openSUSE commands behind typed Rust provider boundaries. Preserve user-facing concepts where their semantics fit; do not port Go CLI code as new project-owned executable code. |
| Software store | [`ryoku/apps/ryostore/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore) contains the Quickshell UI and Go backend; [`system/extras/ryostore-install`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/system/extras/ryostore-install) is the bundle actuator. The documented command interface includes `ryostore catalog`, `ryostore install <category> <id>`, `ryostore open <discover|library|category>`, and `ryostore settings <category> [id]` ([store design](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/docs/store.md)). | Package bundles invoke pacman and an AUR helper; the UI also manages non-package content such as plugins, rices, and color schemes. | **Rust adapter/replacement for package actions; retain UI only after protocol review.** No supported UI or command may invoke pacman, yay/paru, or AUR sources. A package catalog may expose only a future allowlisted openSUSE package-provider operation; unrelated content kinds require their own scoped review. No store actuator is implemented by this inventory. |
| Updater | [`ryoku/cli/internal/updater/`](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/internal/updater), especially `update.go`, `ryokuset.go`, and `snapshots_test.go`; the public [update contract](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/docs/updates.md) documents its package, checkout, snapshot, and session handoff behavior. | The packaged path updates named Ryoku packages separately from the base OS using `pacman`; optional `--system` runs `pacman -Syu` and `yay -Sua`. Snapper snapshot commands and pacman package metadata are assumed. | **Rust adapter/replacement.** The openSUSE implementation must gate mutations on the project's linked pre-change recovery point and use a reviewed full Tumbleweed transaction. It must not expose Ryoku-only partial updates or Arch/AUR operations. This inventory supplies no update implementation. |
| Recovery | [`ryoku/cli/recovery.go`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/recovery.go), [`bin/ryoku-recovery`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/bin/ryoku-recovery), [`ryoku/cli/systemd/ryoku-boot-guard.service`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/systemd/ryoku-boot-guard.service); command contract in [`docs/cli.md`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/docs/cli.md). Upstream exposes a user recovery command and boot guard around its configuration/package model. | Recovery refers to pacman-owned files, Arch config paths, and Snapper/boot behavior; its files do not implement this project's linked catalog, generation, journal, or private-profile contract. | **Rust replacement.** Use the existing `suse/recovery` contract, authenticated recovery entry points, and complete-state restore rules from #1. Do not retain the upstream reset path as an openSUSE recovery backend. |
| Login/session | [`ryoku/shell/systemd/user/ryoku-session.target`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/systemd/user/ryoku-session.target), [`ryoku/shell/systemd/user/ryoku-shell.service`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/systemd/user/ryoku-shell.service), [`ryoku/shell/systemd/user/ryoku-bootstrap.service`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/systemd/user/ryoku-bootstrap.service), and [`ryoku/lockscreen/sddm/ryoku-wayland-session`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/lockscreen/sddm/ryoku-wayland-session). These define systemd user ordering, shell startup, and the SDDM Wayland session wrapper. | SDDM, Arch session paths, upstream shell binaries, compositor-specific environment, and helper scripts. | **Declarative replacement plus scoped retained UI/config.** Express supported openSUSE session units and RPM file ownership declaratively. Retain compatible upstream QML/config only with its native-language rules and complete notices; replace privileged/session operations through reviewed Rust adapters. Session/recovery security contracts in #1 take precedence. |

### Arch package-operation exclusion

The upstream store and updater intentionally have package paths for pacman and
AUR helpers. Those paths are not compatible with this port. No supported
command, store item, session action, or package metadata may implicitly
translate an upstream package request into an AUR operation. openSUSE package
operations must go through the reviewed provider boundary and its allowlist.
Until that Rust adapter is implemented and tested, package installation and
update are unsupported in this project.

## Patch and synchronization procedure

The source pin is metadata only; upstream files are not copied into this
repository. Keep any future adaptation small and auditable:

1. Review the candidate upstream change at its immutable commit URL and inspect
   its full-tree diff against the current pin. Verify repository identity,
   license, component paths, and any new file-level notices.
2. Change the full commit in `upstream/ryoku.source.toml` only after that
   review. Record the prior SHA, new SHA, reason, and affected components in
   the issue/PR evidence. Never use a branch name, release alias, or short SHA
   as the reproducible revision.
3. Prefer independent Rust adapters or declarative openSUSE metadata over
   patches to retained upstream files. If a retained component needs a patch,
   store only the minimal unified diff under `upstream/patches/`, name it for
   its component and purpose, record its upstream base and rationale, and prove
   both `git apply --check` and clean application to that exact pinned source.
   Do not add dormant patch files or vendor a whole upstream tree.
4. Import only files required by an accepted issue. Preserve the upstream
   copyright notices, GPL-3.0 license text, NOTICE, and any component-specific
   license/attribution files in the shipped source/package. Record imported
   paths and patches in the source manifest before release.
5. Run the owning issue's Rust, package, and VM acceptance checks after every
   sync; a source revision change alone is not evidence of a compatible port.

Current patch set: `upstream/patches/issue18-writer-launch.patch`, limited to
supported writer launch coordination and the pinned Quickshell executable path.
The exact pinned shell-unit test fixture is retained under
`suse/recovery/tests/fixtures/` with upstream LICENSE and NOTICE. No desktop
runtime source tree is vendored. See [#18 writer integration](issue18-writer-coordination.md).
