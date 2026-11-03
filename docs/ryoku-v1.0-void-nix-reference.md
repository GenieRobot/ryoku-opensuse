# Ryoku 1.0 Void and Nix source reference

This partial #173 distribution review reads only cached Git objects at Ryoku
1.0.0 `a59d051f0e8080052216f6c7b9eb7e5928935ce8`, compared with 0.75.5
`98a0368253d9628be72b853985878a5db90e1547`. It does not execute or import
XBPS templates, runit scripts, Nix expressions, installers, containers, fetchers
or signing tools. All original feature and asset targets remain retained;
Tumbleweed replaces edition-specific platform mechanisms through existing
Rust/provider/declarative packaging contracts.

## Complete bounded identities and current selection

[The disposition ledger](../upstream/evidence/ryoku-v1.0/void-nix-disposition.tsv)
pins all 189 current path/blob/mode identities, absent in the prior tree:

| Area | Current paths | Source reference and Tumbleweed responsibility |
|---|---:|---|
| Void init | 61 | runit system/user services, environment and session startup, translation manifest; #25/#31, with #177 lock, #178 Bluetooth, #179 dictation, #183 Rashin and #181/#32 network/security owners. |
| Void packages | 66 | 45 XBPS templates, shared package translations, install/remove hooks, repository signing/build inputs; #173 source/spec review, #27 plans/trust, #28 transactions and #182/#47 content/app target owners. |
| Void ISO | 24 | live image, offline repository, partition/chroot/deploy/boot/snapshot adapters; #49 media scope and #24/#27/#28 native install/package contracts. No new ISO implementation is introduced. |
| Void README | 1 | edition declarations and upstream limitations; not Tumbleweed feature exclusions. |
| Nix packages | 27 | derivations plus default composition; compare source/data/native dependency and wrapper boundaries, not package names or evaluated closure. #25/#175/#176 appearance, #180/#182 content/UI, #183 Rashin and #47 other app ownership remain distinct. |
| Nix patches / asset / README | 6 / 1 / 1 | exact patch and logo identities retained; upstream says initial extraction needs validation. |
| Root flake.nix / flake.lock | 2 | dependency declarations and locked revisions/narHash values; no Nix evaluation or source fetch. |

The earlier 151 Void figure covers packages/init/ISO without README. There
are 35 paths under `nix/` **plus** two root flakes, hence 37 in this review;
all are additions, not 35 including flakes. None of these 189 reference files
is currently in Source2 or Source3. That does not remove their feature intent
or source provenance, and does not resolve the remaining source/archive
coverage question. They are reference inputs, not newly installed port code.
No archive, selection manifest, spec, installer or runtime behavior changed.

[The reference JSON](../upstream/evidence/ryoku-v1.0/void-nix-reference.json)
records all 72 template/derivation identities and lexical declaration/edition
interface sites (45 Void templates and 27 Nix files), 30 anchored API/recipe
sources, complete raw init translation rows, package-mapping counts, the full
parsed flake lock and eight supporting shared-source identities. Lexical sites
are not evaluated multiline dependency expressions or transitive closure.
Pinned checksum/narHash declarations are not independent remote content or
publisher-trust validation. The cached Git blobs establish what was declared.

## Void package, init and installer behavior

`void/README.md` declares an experimental glibc x86_64 edition using runit,
Turnstile, elogind, SDDM and Niri, reached through `ryoku-host`. Its statement
that Hyprland is not shipped reflects that edition's compiler constraints;
Tumbleweed retains Hyprland under #25, Niri under #46 and Wayfire under #185.
The package table has 300 rows, including 37 `void=-` mappings with notes.
Those may denote a bundled equivalent or missing mechanism, not 37 features
that the Tumbleweed target can drop. A table row is neither a signed Tumbleweed
provider record nor a successful package solve.

| Source boundary | Concrete effects and owned replacement |
|---|---|
| `ryoku-desktop/template` → shared `release/packages/ryoku-desktop/payload.sh` | Sources generated release metadata, builds the shared payload and installs it in runit mode. Shared application source is not a separate Void desktop. Preserve payload/features while #173 verifies native recipe/dependency/license facts and #25/#182/#183 own runtime integration. |
| Desktop `INSTALL` / `REMOVE` | Conditional power cutover, cache updates, polkit reload, system service reconciliation, Bluetooth daemon restart, Wi-Fi regdom/backlight tuning; removal can delete runit links and `/var/lib/ryoku` markers. #27/#28/#31 and #18/#20/#21 require typed transaction/service authority and complete linked write/capture accounting; #178/#181 own relevant device/network operations. Package hooks are not imported as Rust actuators. |
| `init/session/session-start` | Publishes session environment into Turnstile envdir under umask077, updates D-Bus activation environment, ensures/reconciles user services and restarts the roster. `environment-d` rejects several shell-syntax forms before an eval-based export. #25/#31 must preserve correct session environment/lifecycle using native systemd contracts and validated data; this review does not certify the parser or run it. |
| `init/translations.tsv` | 27 rows: 24 translated, three explicitly `accepted-loss`. The latter concern portal drop-in, systemd polkit action and Arch live units. Translated-row notes also record StartLimit, TimeoutStopSec, persistent timer, NoNewPrivileges and ordering differences. These upstream losses are evidence, **not accepted Tumbleweed losses**. Native sandbox/order/timer/authorization semantics require #25/#31 and per-feature owners. |
| Rashin/Prowl run/finish | Rashin `serve --if-enabled` brings up Prowl and its finish brings it down. Prowl waits for Rashin, clears a stale listener and serves its gateway on a configurable port. AI-usage timer becomes a loop. Preserve the complete core assistant/usage/gateway capability set under #183 with protected settings/lifecycle/privacy; do not interpret an upstream runit choice as Tumbleweed acceptance. |
| Network kill guard / boot guard | runit cannot order the firewall guard before NetworkManager; source records a residual startup race. Boot guard delegates update fallback. #181/#32/#31 must verify fail-closed native ordering/security; #27–29/#21 own protected update/recovery behavior. A source comment does not establish firewall or boot safety. |
| `packages/repo/{build-repo.sh,container-runner.sh,new-signing-key}` | Non-root xbps-src builds, source archives/checksums, a container with read-only private-key mount, RSA key/plist creation and xbps-rindex indexing/signing. #27/#28 require the port's reviewed repository/trust/transaction contracts. No keys, containers or repositories are created, mounted, signed or contacted here. |
| `iso/backend/ryoku-install` → shared disk/LUKS/filesystem libraries plus Void adapters | Imports destructive shared install machinery, writes a chroot target, installs XBPS closure, configures users/services/locales, seeds Limine/firmware boot paths and snapper layout. ISO builder installs tools, creates/mounts staging and assembles offline repositories. These are platform installer operations, not desktop helpers. Preserve native install/recovery experience under #24/#27/#28 and separate media scope #49; do not import disk or boot scripts into #173. |

The init manifest's raw translations and notes are preserved verbatim in the
JSON, including absolute/package-owned/wildcard source rows that are not
literal current-tree files. Translation target tree-entry counts are recorded
rather than falsely treating each row as one source file or native unit.
The eight shared identities include package resolver/service manifest, payload
recipe and destructive disk/LUKS/filesystem libraries; they anchor delegation,
not a complete transitive installer/security review.

## Nix extraction, wrappers and complete feature intent

`nix/README.md` explicitly packages applications/data rather than configuring a
NixOS machine. It delegates module/installer/login/hardware/update policy to
Ryoku-on-NixOS and calls this an initial, unvalidated extraction. In particular,
a comment mentioning `nix/modules/ryoku.nix` in desktop-data describes external
module ownership; that path is not present in this pinned tree. Do not claim an
in-repository NixOS service implementation from that comment.

The flake source filter removes packaging/flake inputs from its application
source. The exact lock retains eight non-root dependency nodes, including two
nixpkgs universes, their revisions and NAR hashes. Some original URL refs move,
but the lock pins resolved revisions. It also declares a pinned Quickshell
screencopy patch. None of this proves the port's pinned Quickshell ABI, cache
availability, publisher trust, license closure or native plugin/import loading.

| Exact derivation boundary | Source fact and Tumbleweed ownership |
|---|---|
| `ryoku-desktop-data.nix` / `ryoku-helpers.nix` | Copies configs, themes, QML and helpers, rewrites FHS executable/import paths and wraps lifecycle commands. It avoids materializing user units that override NixOS-owned services. #25/#31 must preserve desktop/session behavior through the port's owned paths, protected materialization and native units; #177/#178/#180/#182 own their feature boundaries. Nix shell snippets are not new port actuators. |
| `ryoku-shell.nix` | Tests for the polkit helper and replaces its FHS location with Nix's polkit output path. Path relocation is not authentication/recovery acceptance and must not weaken #18/#20/#25 authorization or native polkit validation. |
| `ryoku-cli.nix` | Wraps host/update platform as nixos/nix, sets immutable configuration/source/version paths and patches a doctor reconciler. Preserve doctor/update/track/rollback capabilities through #25/#27–29/#31/#32 and existing recovery owners, replacing only the platform mechanism. |
| `ryoku-rashin.nix` | Builds vendored backend, generates repository index and copies wiki/skills; wraps Nix package/backend flags, config/data and runtime PATH, and provides the `rashin` alias. This is not the entire assistant native-app/HTTP/WebSocket/ACP integration. #183 retains every capability and the separate exact GTK/WebKit provider handoff; native runtime remains open. |
| `ryoku-ryogami.nix` / `ryoku-livewall.nix` | Builds wallpaper/editor/livewall components, depends on FFmpeg/ImageMagick/Matugen/curl/inotify/Quickshell/model tooling, rewrites model/palette paths and installs picker license notices. Preserve full behavior under #25/#175/#176 with bounded writes, checkpoint ownership and hook policy; immutable paths are not protection of user-state mutations. |
| `ryoku-ryostore.nix` | Builds/install catalogue backend from the shared app tree. Its command has no explicit `-mod=vendor` flag, unlike the Rashin command; this textual difference does not prove a network access occurred or that module closure is complete. #182 retains catalogue/install/build capability with #27/#28 package and trust prerequisites. |
| Other app/native/content derivations and patches | Preserve exact package/patch/asset identities, including Ryotunes authentication patches, Niri output cleanup, models, cursor/font and VM helpers. #47/#182 plus #46/#25/#175 retain full target behavior. Dependency, authentication/privacy, external-source verification, modification notices and per-asset rights need separate native/owner proof; do not copy broad derivation license declarations onto every asset. |

Root LICENSE/NOTICE and the raw template/Nix license fields remain provenance
inputs; no new SPDX assertion or redistribution-rights conclusion is made.
Exact six Nix patch identities and the logo remain in the ledger; external
flake sources are declarations, not newly acquired source archives. Full
source/archive closure, transitive helper/API/notice review, native dependency
solver results, lifecycle/security behavior and fresh desktop/recovery
acceptance stay open. This unit does not check the distribution checklist row
complete or replace feature-owner acceptance.
