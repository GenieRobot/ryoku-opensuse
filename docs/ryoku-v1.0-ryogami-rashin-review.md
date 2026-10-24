# Ryogami and Rashin: source disposition and protected integration gates

The product target is the full Ryoku experience on Tumbleweed. Ryogami,
its editor, livewall, wallpaper actions and related modules remain selected
and packaged. The temporarily proposed exclusion was cancelled; none of its
build/install/route removals was committed. The source findings below identify
required protected integration, not permission to reduce the product target
or to activate an unprotected writer. This review does not certify the current
reachable write paths as safe.

Review pin: `a59d051f0e8080052216f6c7b9eb7e5928935ce8`.
Ryogami tree `3e57f5c00ddb10be66ef13ebf3c26a69bf493f1d` has 424 current files:
175 daemon, 248 picker and one `.gitignore`. Rashin tree
`4decda201797e274e834342813e53bdba325d37b` has 338 current files. The delta
matrix's 521 Ryogami / 315 Rashin entries count changes from 0.75.5, including
deletions and renames; they are not current-tree sizes. Current Ryogami has
neither a `hyprland/` nor a `bin/` subtree. The removed `wall-ui` is not replaced
by copying an old source path.

## Export and installed boundary

[`ryogami-rashin-disposition.tsv`](../upstream/evidence/ryoku-v1.0/ryogami-rashin-disposition.tsv)
records 769 exact blobs/modes, selection in both export manifests, and install
disposition. It includes the two trees, three livewall sources, one selected
unit and three bridges outside Rashin. Every selected row was checked against
`runtime-source-manifest.tsv` / `writer-source-manifest.tsv`; install claims
come from `packaging/rpm/ryoku-desktop.spec`.

| Source group | Disposition |
|---|---|
| `ryoku/shell/ryogami/daemon/**` (175) | Source3 private Go binary build inputs, including vendor/tests. Install the compiled daemon under `/usr/libexec/ryoku/upstream/ryogami`; public `ryogami` aliases Rust writer. |
| `ryoku/shell/ryogami/picker/**` (248) | Source3 CMake inputs. Install generated `Ryoku.Ryogami` QML/native module, `shell.qml`, `data/ryogami.desktop`, LICENSE and NOTICE. Do not copy all source files into `/usr/share/ryogami`. |
| `ryoku/shell/livewall/**` (3) | Source3 C/Wayland protocol/build inputs; install compiled `ryogami-live`. This helper is outside the Ryogami tree. |
| `ryoku/shell/systemd/user/ryogami.service` | Selected Source3 patch/fixture input, but no unit installation in this desktop spec. Selection is not activation. |
| `ryoku/rashin/**` (338) | Entire tree excluded from both exports and install recipe: app 5, backend 156, skills 8, systemd 4, usage 4, web 153, wiki 8. No agent executable, web app, desktop entry, collector or unit installed. |
| Hub `backend/rashin.go`, `quickshell/pages/RashinPage.qml`; shell `quickshell/shell/services/AiUsage.qml` | Retained bridges outside excluded Rashin tree. Hub mutation routes refused; AiUsage service-start launch patched to Rust refusal. Source exclusion alone would not prove these bridges harmless. |

## Reachability: acceptance blocker, not a completed safety audit

Picker `src/core/daemon.cpp` opens a `QLocalSocket`; its `send` writes JSON
method/params requests to the same `$XDG_RUNTIME_DIR/ryogami.sock` used by the
Go daemon (`proto.go`, `/tmp` fallback). `src/core/remoteresults.cpp` directly
calls `source.search`, `source.download`, `workshop.search` and
`workshop.download`. The daemon dispatches JSON-RPC and plain wallpaper/depth
verbs (`daemon.go`, `rpc.go`, `verbs.go`) and registered extension actions.
Thus the generated installed module reaches the private installed daemon;
these are not merely unused source files.

At this checkpoint `suse/recovery/src/writer_protocol.rs` checks the exact
query policy only for `HubBackend`; `Ryogami` is coordinated but has no
equivalent action allowlist. Namespace/recovery coordination does not prove
that all new v1 methods are authorized. The following reachable writes and
automatic startup hooks require protected Rust integration and negative
socket/CLI tests before this candidate can be accepted. This documentation
does not resolve that blocker or approve the existing reachable writes.

| Selected entrypoints | Write/runtime consequences |
|---|---|
| `rpc.go`: `state.set`, `wall.delete`, `wall.cache_rebuild`, playlist mutation, effects/grade commit, upscale start | Cache/state, source wallpaper deletion, transforms and process creation. Query-looking preview methods can generate files; classify by implementation, not method spelling. |
| `source_download.go`, `source_{wallhaven,pexels,unsplash,youtube}.go` | HTTP searches/downloads, temporary files/renames into configured wallpaper/video directories; Wallhaven API, Pexels API, Unsplash API, YouTube/watch and thumbnail endpoints. YouTube launches `yt-dlp`; its advice probe uses `ryoku-host`. No remote provider is accepted by this review. |
| `lens_models.go`, `lens.go` | Registers Import/Add/Remove/DeleteSemanticModel and DeleteActiveSemanticModel. Locates `skwd-lens`, runtime/model manifests, writes model library under XDG data and launches install/remove helpers. Existing availability probing must not turn a locally installed helper into authorization. |
| `workshop_steamcmd.go`, workshop sources | Finds/launches `steamcmd`, writes Steam workshop content and uses network/account integration. No Steam/Wallpaper Engine capability imported. |
| `settings_store.go`, `config.go` | Writes `$XDG_CONFIG_HOME/ryoku/{ryogami,shell,matugen}.json`; legacy migration reads `$XDG_CONFIG_HOME/ryogami-wall`. Matugen setting delegation calls `ryoku-hub desktop matugen set`, already refused, but local writes also exist. |
| `apply.go`, `store.go` | `$XDG_CACHE_HOME/ryogami` (or configured cache), `outputs.json`, state/library/playlist files, and `$XDG_STATE_HOME/ryoku-wallpaper` plus legacy JSON. Configured picture/video paths and deletes require exact ownership review beyond the old fixed inventory. |
| `postapply.go` | `afterApply` and `onStart` can call `runPostProcessing`; `postProcessOnRestore` permits hooks during restore/start. `runDetachedShell` launches `sh -c` wrapping `nohup setsid sh -c`. A CLI-only refusal would leave this automatic path executable. |
| `palette_bridge.go`, `matugen_outputs.go`, `upscale*.go` | Delegated Hub mutations are refused, but generated matugen config/output writes, transform tools and self-spawned `upscale-worker` need their own boundary. Absence of optional packages is not a refusal policy. |

The retained core wallpaper apply/restore/UI contract must be integrated with
the existing protected writer/checkpoint path before writes are supported.
The current routes remain reachable, which is a blocker rather than an
implemented refusal. This review does not infer approval from #25's need for
a functioning shell.

### Core dependency and the limit of a launch-only fix

The selected `ryoku/shell/ipc/ryogami.go` subscribes to the wallpaper topic;
`ipc/theming.go` explicitly leaves apply, topic publication, transitions and
depth to Ryogami. Removing the whole daemon therefore removes core wallpaper
behavior, rather than merely an optional editor. Removing the picker alone
does not stop automatic daemon effects.

The source exposes a concrete checkpoint mismatch: `daemon/store.go`
`openStore` now uses `<cache>/wallpaper/{index,state}.json` and creates that
directory. `applyWallpaperReasonRecord` in `apply.go` calls `store.mutate`
(persisting the new index) in addition to the known `outputs.json` and
`$XDG_STATE_HOME/ryoku-wallpaper`. The port's current
`suse/recovery/src/installed_inventory.rs` still pins upstream 0.75.5 and
lists `.cache/ryogami/{outputs,state}.json`, not these new nested files.
Coordination of the daemon's process tree is not proof that the new files
are part of the checkpoint. This review does not expand recovery ownership.

`daemon.go` also starts `healAnimatedWebp`, legacy migration, rescan, playlist
resume and workshop/library/settings/WM watchers. Its `runStartHooks` includes
registrations in `postapply.go`, `schedule.go`, `overview_backdrop.go`,
`library_thumbjobs.go`, `lens.go`, `pause_rules.go`, `library_videocache.go`,
`rotation_settings.go`, `power.go`, `display_fill.go`, `library_trash.go` and
`audio_duck.go`. Some write cache, delete retained media or spawn helpers
without an incoming socket action. Denying only download/model RPC verbs
would leave those automatic paths live.

The existing seams are Rust `run_writer` argument validation, native
`dispatchRequest` / `dispatchVerb`, and exact startup/child launch sites in
the source patch. A future focused patch must test each of those seams and
refuse unreviewed actions before their write/launch body. Native direct
socket tests must exercise the patched daemon, not merely a Rust alias.
No new policy proxy, automatic model/provider capability or expanded recovery
manifest has been introduced. A complete startup/core-write disposition and
protected adapter are required while retaining the complete wallpaper
experience. Until then the candidate is **not ready for installation/appearance
acceptance**. Removing the wallpaper bridge would also require changes to
`shell/shell.qml` reload readiness and visualizer gating; that exclusion is
not being pursued, and those bindings remain intact.

### Ownership and retained full-feature integration plan

The issue scopes below were read on 24 October 2026, including the new M2
owners #175 and #176 under tracker #4. They are ownership boundaries, not a
claim that a provider has been implemented or certified. Both new tasks depend
on #173's source-path review and #25's protected core appearance integration;
they retain the complete UI and behavior.

| Work | Existing owner / contract | Required next implementation and evidence |
|---|---|---|
| Core wallpaper display/apply/restore, editor integration, transitions, dynamic palette, video and depth | [#25: core desktop and appearance boundaries](https://github.com/GenieRobot/ryoku-opensuse/issues/25), using the existing #18 writer/capture and #20 durable mutation contracts | Retain all source/UI/package nodes. Route every persistent apply, assignment, favorite, playlist, settings and effect change through a typed Rust plan with explicit affected paths, authorization, linked pre-checkpoint verification, revalidation, health and post-change accounting. Include daemon startup, timers and direct IPC; wrapping only the public CLI is insufficient. Native appearance/session acceptance remains #25, combined recovery #26. |
| Newly nested Ryogami index/state and configuration ownership | #25 core appearance integration and #175 retained library/provider operations, consuming #18 capture and #20 coordination | Reconcile `.cache/ryogami/wallpaper/{index,state}.json`, configured library/cache roots, absent files, ownership and bounds with the complete capture/restore model. Classify persistent preferences/receipts versus regenerable thumbnails/model indexes. Neither completed foundation issue certifies new v1 paths; no broad home-directory capture or inventory expansion is made by this source review. |
| RPM dependency installation/removal needed by the retained features | [#27: package discovery/plans](https://github.com/GenieRobot/ryoku-opensuse/issues/27), [#28: protected package/repository/full-system transactions](https://github.com/GenieRobot/ryoku-opensuse/issues/28), [#29: native clients/progress](https://github.com/GenieRobot/ryoku-opensuse/issues/29) | Reuse the shared package plan and protected execution when a feature needs a distro package. Preserve full Tumbleweed transaction policy. These issues do not automatically authorize image/video/model downloads or file deletion. #30 is optional Myrlyn integration, not a Ryogami provider owner. |
| Image/video source downloads, Steam workshop, model pack install/remove, library management and source-media deletion | [#175: protect retained Ryogami library and provider actions](https://github.com/GenieRobot/ryoku-opensuse/issues/175), integrated with #25's protected core transport and #18/#20 contracts | Implement bounded Rust providers behind the authenticated helper: source/manifest/trust identities, exact account-owned destinations, size/time quotas, safe interruption/resume and trash/restore receipts. Revalidate reviewed plans and linked pre/post state before local writes/deletes/process launches. Keep credentials private and record remote requests separately because local rollback cannot undo external effects. Preserve every picker/CLI/daemon action; RPM acquisition remains #27–29. |
| Startup/restore/postapply hooks, timers/watchers and configured commands | [#176: constrain retained Ryogami startup and post-apply hooks](https://github.com/GenieRobot/ryoku-opensuse/issues/176), integrated with #25 and #18/#20 | Implement Rust-owned typed execution policy bound to authenticated account, exact hook identity/content, permitted arguments/environment/directory, time and write/network scope. Enabling/changing policy requires explicit authorization; every automatic run revalidates an exact approved policy and complete linked capture/accounting before launch. Arbitrary shell strings cannot supply trusted plans. Preserve hook configuration and approved behavior; prove child containment/refreeze and no silent unapproved restore execution. No generic privileged exec API is granted. |
| User-service changes | [#31: narrowly typed service controls](https://github.com/GenieRobot/ryoku-opensuse/issues/31), with #25 responsible for ordinary session lifecycle | Separate runtime start/stop from persistent enablement/config changes and document restore limits. This does not authorize arbitrary systemctl commands or require putting service control in #173. |
| Separate Rashin/other app adoption | [#47: remaining application inventory and scoped application issues](https://github.com/GenieRobot/ryoku-opensuse/issues/47) | Current export exclusion is an inventory fact, not a decision to delete those applications from the eventual full product. Each future activation needs its explicit app/state/provider acceptance. This is distinct from already retained core Ryogami. |

Reuse `NativeMutationProvider::plan/apply/health_check` and
`run_operator_mutation` in `suse/service/src/native_mutation.rs`, the verified
generation and revalidation types in `mutation_gate.rs`, the authenticated
operator/account binding, writer leases, durable operation history and linked
pre/post accounting described in [mutation coordination](mutation-coordination.md).
The current service `ManagedAction` enum has package/profile/checkpoint actions,
but no wallpaper, source-download, model-pack, source-delete or hook action.
Those typed contracts and an installed provider dispatcher remain missing;
#25 owns core appearance transport, #175 the retained library/providers and
#176 automatic hook execution policy. Creating these owners does not implement
their contracts or certify the currently reachable writes;
the documented zero-certified-provider-write result must not be relabelled
as support for them. Existing daemon JSON-RPC IDs and settings-action names
are source entrypoints, not authorization or a replacement for durable
service operation IDs. New project-owned implementation and tests remain Rust.

#173 owns the immutable semantic/path/API audit, preserved source/package
selection, dependency/native build and license closure, launch integration
patch provenance, and exact refreshed acceptance evidence. Its current scope
does not authorize a new generic writer, a new recovery design, or the missing
network/model/hook providers. Preserve Source3 daemon (175), picker (248),
livewall (3) and selected unit (1); preserve the private Go daemon/public Rust
alias, generated `Ryoku.Ryogami` module/Provide, editor shell/desktop entry,
`ryogami-live`, QML actions and shell readiness bindings. The selected unit is
still not installed by the current spec; #25 must integrate its ordinary
session lifecycle rather than confuse source selection with activation.
Preserve Blobs/Spatial3D and shared shell/Hub payload as well.

Tests for the implementing owner must exercise CLI, direct socket requests,
startup/restore hooks and automatic watchers; stale/denied/missing-checkpoint
requests must not write or spawn the actuator. Prove the nested-state pre/post
receipts and recovery after interrupted operations, then verify the full
editor/display/palette/video/depth experience in the recorded Tumbleweed guest.
Hosted Rust CI can validate code while local storage is constrained; it cannot
replace native socket/tool/plugin/appearance acceptance. #173, #25 and #26
remain open, and no native acceptance follows from this plan.

The concrete source/dependency sequence for #173 is:

1. Keep the current immutable exports and all Ryogami build/install/alias/UI
   paths intact; preserve the original 0.75.5 acceptance labels. Complete
   path-level semantic review of the remaining delta areas without importing
   another distribution's package managers or privileged host scripts.
2. Complete installed picker font-license delivery and other notice/provider
   ownership evidence. Keep the two selected TTF license texts with the shipped
   module notices; no dependency or license is guessed from a capability name.
3. Reuse the verified signed Snapshot20261016 SDK/import inputs and 68-package
   closure when storage is restored. Retain native Qt 6.11.2 GUI-private,
   Multimedia/ShaderTools, Vulkan headers, Wayland/protocols and FFmpeg library
   inputs; retain ImageMagick/FFmpeg runtime tools and Quickshell 0.3.2-3.
   Resolve WaveCapture's still-unverified `pactl`/`pw-record` file owners from
   exact RPM payload evidence before adding those dependencies.
4. Build/test the exact candidate and inspect generated picker/Blobs/Spatial3D
   module metadata, shaders, imports and ELF closure. Keep source compilation
   and tool-suite evidence separate from guest graphics/codec results.
5. Coordinate #25's protected full appearance adapter with #175's retained
   library/providers and #176's startup/restore/postapply policy, using the
   reviewed pushed #25 interface and issue-specific branches as their scopes
   require. Then verify native session/provider/hook behavior with the refreshed
   package under #26's combined recovery gate. Preserve full functionality and
   keep prerequisite ordering explicit; no feature removal or protection
   bypass substitutes for those implementation and acceptance results.

## Excluded Rashin runtime

Rashin is an agent/terminal service, not a wallpaper dependency. Its
`backend/server.go` binds HTTP to `127.0.0.1:<configured port>` and exposes,
among others, POST `/api/fix`, `/api/index`, `/api/agents/wire`, `/api/ask`,
`/api/term`, `/api/chat/terminal` and WebSocket `/ws/chat`. `acp.go` starts a
configured backend command; `setup.go` references the Hermes remote installer
and host package advice. `paths.go` defines the knowledge vault at
`$XDG_DATA_HOME/ryoku/rashin` (or `RYOKU_RASHIN_VAULT`), config at
`$XDG_CONFIG_HOME/ryoku/rashin.json`, and pid/setup runtime state beneath
`$XDG_RUNTIME_DIR/ryoku-rashin` (temporary-directory fallback).

Excluded units are `ryoku-rashin.service` (serve --if-enabled),
`ryoku-prowl.service` (gateway down/serve, default port 8788),
`ryoku-ai-usage.service` and its persistent five-minute timer. No agent/model,
terminal, gateway, usage collection or credential provider is activated.
Future standalone app work belongs to #47 and its scoped application issues;
package planning/execution remains #27–29. The retained Hub on/off routes and collector start are refused
as described above; status remains an availability probe only.

## Build and licensing evidence still required

Picker `CMakeLists.txt` requires Qt Quick, ShaderTools, Multimedia and
GuiPrivate (minimum 6.6), C++20 and Vulkan headers. It resolves Vulkan at
runtime, builds the native module with `$ORIGIN` plugin rpath, embeds QML/JS,
shader resources, brand art and two TTF fonts. `livewall/build.sh` uses
wayland-scanner/protocol XML and links wayland-client, libavformat, libavcodec,
libavutil and libswscale. These require native import/plugin/ELF validation;
no source inspection substitutes for it. Go tests involving ImageMagick and
FFmpeg remain pending the actual tool suite.

Picker NOTICE attributes skwd-wall v2 (GPL-3.0-or-later), identifies its shader
port and separate skwd-paper playback, and describes Duel's Serpantinum
design reference. It also identifies Roboto Condensed Bold UI as Apache-2.0
and Symbols Nerd Font as MIT. The exact font texts are selected in
`picker/qml/theme/fonts/*.LICENSE-*.txt`, but CMake embeds only `*.ttf` and the
spec explicitly installs only picker LICENSE/NOTICE. **The font license texts
need explicit installed license closure**; selection in Source3 does not
prove they ship beside the generated module. Root LICENSE/NOTICE and vendored
Go notices are separately retained. No license identity is inferred for
excluded Rashin dependencies or optional downloaded models.

The guest remains paused under the storage gate. This review adds immutable
source evidence only; full delta coverage, protected integration/tests,
native builds, tool/runtime checks, license closure and fresh #25/#26
acceptance are still pending. PR #174 remains draft/open.
