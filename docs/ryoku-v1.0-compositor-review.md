# Ryoku 1.0 compositor semantic review

This is a partial #173 semantic review of the retained compositor surface at
upstream `a59d051f0e8080052216f6c7b9eb7e5928935ce8`. It does not complete the
compositor delta row, certify a protected mutation or replace native acceptance.
Hyprland remains the initial session under #25; Niri #46 and Wayfire #185 are
distinct retained targets. No component, plugin capability or source root is
removed by this review.

## Exact component and compiled-client boundary

[`compositor-disposition.tsv`](../upstream/evidence/ryoku-v1.0/compositor-disposition.tsv)
records 221 current/prior path identities: 218 current, 155 prior, with 66
added, 69 modified, three old paths absent and 83 unchanged. Two old paths
moved to `ryoku/keysounds` (R100 profiles and R085 import helper); one cursor
tracking script was deleted. The ledger pins both relocated destinations with
current blobs/modes and #25/#180/#182 ownership. These four roots have
138 unique changed paths. The earlier 32 Niri / 42 Wayfire figures describe
adapter changes nested inside the 117 `ryoku/wm` changes; they are not extra
root-config counts. Hyprland config contributes 18, standalone Niri config one
and standalone Wayfire config two. Source path moves/deletion are upstream facts,
not removal of the corresponding full Tumbleweed feature target.

| Current area | Files | Current Source3 / runtime recipe fact and owner |
|---|---:|---|
| `wm` shared root | 21 | Raw source selected. Eleven production Go files are shared client/model code; tests/module metadata remain source inputs. #25/#177/#180 and #27–29 own operation contracts. |
| `wm/hyprland` | 34 | Raw source selected; current spec builds the private Hyprland adapter and installs its four QML bridge files. #25/#180/#182. |
| `wm/niri` | 44 | Complete raw adapter source selected; its binary is not a current spec build/install target. Retain full Niri under #46. |
| `wm/wayfire` | 42 | Complete raw adapter/decor source selected; its binary/plugin is not a current build/install target. Retain phased Wayfire under #185. |
| `wm/vendor` | 15 | Exact vendored TOML dependency and notices; preserve its MIT COPYING under the existing license-copy recipe. |
| `hyprland` config/helpers | 51 | Three modules selected as raw/patch inputs; other current source selection gaps are mapped to #25/#177/#180. No imported source script is treated as an authorized actuator. |
| `niri` / `wayfire` config | 9 / 2 | Current export gaps, retained under #46/#185 for their native implementations. |

[`compositor-api-boundary.json`](../upstream/evidence/ryoku-v1.0/compositor-api-boundary.json)
pins 14 API/helper files with exact blobs/modes/digests and source anchors.
It also checks all 44 shared-WM vendor mirrors: eleven production files each
in IPC, Hub, Ryogami and CLI `vendor/ryoku-wm`. Every blob/mode equals the
root WM source. Thus the audited shared client is present in the current
`-mod=vendor` IPC/Hub/Ryogami builds; CLI remains raw-source-only. Hyprland's
provider uses the root WM module. This avoids assuming that a root source
file necessarily controls a separately vendored binary.

## Shared client, apply and output semantics

The three provider `main.go` dispatch tables are preserved in the evidence.
Hyprland exposes caps/act/watch/apply/outputs/state/defaults/session/schema/
binds/environment/plugins. Niri has import in place of plugins; Wayfire has
pages. These are source API inventories, not accepted arbitrary argv routes.

| Exact entry/call chain | Concrete effect and required owner contract |
|---|---|
| `wm.Client.Open/OpenNamed` → `providerCommandContext` | Builds `ryoku-wm-<name>` and invokes it with argv; detection accepts `RYOKU_WM` before session hints. PATH resolution and process-group cancellation are lifecycle behavior, not per-operation authorization. #25/#46/#185 require known provider identities and typed operation contracts. |
| `DryRun` / `Preview` → `apply <store> --preview` → Hyprland `pushEval(liveLua(o))` | The path attempts a live `hyprctl eval` before returning its report. File-free preview is not pure discovery. Preserve preview and pure compatibility discovery as distinct #25/#180 capabilities. Source review executes neither. |
| Hyprland `runApply` → `writeOverlayLua` | Generates settings.lua, settings_window.lua and rebinds.lua in both `hypr/` and `ryoku/user_edits/hypr/` under the configured config home: six persistent files. `ApplyReport.Written` lists only the three live paths, so #25/#18/#20 capture must derive the complete inventory rather than trust that report. |
| `runApply` → `wm.PublishGreeterNumlock` | Also writes the numlock handoff, default `/var/lib/ryoku/greeter-numlock`, with mode 0666 requested at creation. Source environment overrides can select another path. #177 owns cross-account greeter policy and #25 must bind the exact accepted write scope; this report is not proof of permissions or root execution. |
| `runOutputs` / mirror action → `runMonitor` | Starts PATH-resolved `ryoku-monitor`; output reports monitors.lua after the helper returns. Its exact upstream script is outside current Source3 selection and no current port spec installs that command. Preserve display/layout/mirror behavior through #25/#180; actual host availability and native result remain unverified. |
| Scratchpad `runWorkspaceHelper` | Runs `scripts/ryoku-workspace` inside the user's Hyprland config. That source helper is also outside current Source3 selection. User-config script execution has user-account data access; preserve scratchpad behavior with the owned typed boundary, including #176 hook policy where applicable. |
| `runCtl` / session | Runs hyprctl and resolves live sockets under XDG_RUNTIME_DIR/hypr or /tmp/hypr. Socket/detection/provider success and session-entry text do not certify authentication, managed configuration or KDE fallback; #25/#177 native gates remain open. |
| Shared `Reclaim/VerifyRemoval` | Plans/rechecks outgoing package sets using DNF/XBPS/Pacman-specific queries and no Zypper adapter. These functions are planners, not successful removals. Preserve WM switching/reclaim via #27 plans, #28 transactions, #29 presentation and #46/#185 provider semantics. |

All these effects are conditional source paths. This unit reads pinned Git
objects and package recipes, not live sockets/config/credentials. Upstream
root LICENSE/NOTICE and the vendored TOML COPYING remain source provenance;
complete provider/plugin/asset licensing and native dependency closure remain
open. The full feature target includes live preview, display controls,
scratchpad, session switching and every compositor; current package gaps are
implementation work, not feature exclusions.

The ledger/anchor/vendor checks validate exact immutable identities, current
manifest selection, source-line context and owner classification. Per-action
arity/data bounds, all compositor live/persistent effects, complete capture/
restore accounting and actual runtime acceptance remain open. This inventory
does not check the entire compositor checklist item complete.

## Hyprland plugin discovery and code execution

[`hyprland-plugin-boundary.json`](../upstream/evidence/ryoku-v1.0/hyprland-plugin-boundary.json)
records seven exact upstream path/blob/mode/SHA-256 identities and source-line
anchors, all selected by Source3. It also pins the four project policy/recipe
inputs at `80fae51be3e4e568c9d9b19d8a18919387fdd223`. The current launch patch
does not change those seven upstream files. The RPM builds the Hyprland Go
adapter, installs its private binary, installs all Hub QML including
`pages/PluginsPage.qml`, and exposes `ryoku-wm-hyprland` through the Rust writer.

The active UI has rebuild, inspect, add and remove handlers in
`ryoku/hub/quickshell/pages/PluginsPage.qml` (blob
`4ccca8f831f8cc55fb61763a5f87cb92a4d795bb`). They issue
`ryoku-hub desktop plugins rebuild …`, `add --inspect <url>`,
`add <url> <ids…>` and `remove <id>`. Hub `backend/desktop.go`
(`4a6b873e4ae9c6cc1ae1fc4810580aab51966233`) forwards through
`pluginsDesktop` → `desktopClient().Plugins(args...)` → shared
`wm.Client.Plugins` → `ryoku-wm-<provider> plugins …`. Hyprland's `main.go`
dispatches that to `runPlugins` in `config_plugins.go`
(`30d57757f3d59c6cb0d400523635594161a569df`).

The Hub Rust exact-arity table currently admits `desktop plugins list` and
refuses the other plugin verbs before Hub child setup. **That admitted route
is not a pure read in this upstream version.** The exact chain is:

1. `runPlugins(["list"])` calls `pluginRosterNow()` in
   `config_plugins_tier.go` (`8bda9a91d12191d9121e3b254502650e36b4637d`).
2. `pluginRosterNow` calls `describePlugin` for bundled plugins, local receipts
   and discovered hyprpm copies. For keysounds, `describePlugin` calls
   `keysoundsUserDir`, which creates the user's `ryoku/keysounds` data directory
   and its README (using `XDG_DATA_HOME`, or `~/.local/share`).
3. For a loaded unbundled plugin, `describePlugin` can refresh its settings
   receipt through `writeReceipt`. For an enabled compatible plugin that is
   not loaded, it calls `hyprctlPluginLoad(c.Path)` and `pushEval`, then can
   update receipt metadata. This loads plugin code into the user's compositor;
   listing the roster is therefore also an activation route.

The branch-conditioned [query-effect ledger](../upstream/evidence/ryoku-v1.0/plugin-query-effects.json)
binds this operation to six exact source blobs and distinguishes four effects:
keysounds directory/README creation, refresh of an already-loaded extra plugin's
receipt, enabled compatible plugin activation/evaluation, and post-load receipt
refresh. The first effect occurs before installed-copy lookup, including when
keysounds is disabled or missing. The second does not require the plugin toggle
to be enabled. The activation branch excludes already-loaded and stale copies
through switch ordering; receipt persistence additionally requires an existing
receipt. Its atomic writer can create the parent directory and temporary file
before renaming the receipt.

This is reachable without an explicit activation click: `PluginsPage.qml`
starts its list process with `running: true`, refreshes after actions, and
refreshes 1.5 seconds after the Hub Save signal. Success can coexist with ignored
README/receipt write errors. Neither exact argv nor a successful list result
proves no writes. The ledger supplies concrete negative and positive acceptance
cases for #180/#182 while preserving discovery, My sounds, activation, build,
removal and status feedback. Those owners must implement the protected Rust
operations; #173 does not change this executable Go behavior. The evidence tests
bind selection, unchanged patch coverage and pinned source anchors; they do not
execute or certify any plugin operation. Other query routes and the complete
compositor acceptance row remain open.

Other plugin routes have additional effects in
`config_plugins_build.go` (`f7e3aa4ee1545b5a8b51c9f66462af7903cb1ae4`):

| Entry | Exact source behavior and boundary |
|---|---|
| `add --inspect <git-url>` | `runPluginAdd` checks the toolchain and calls `fetchRepo` before returning metadata. Inspection can clone/fetch and modify the cache; it is not offline, side-effect-free discovery. |
| `add`, `rebuild` | Fetch/read `hyprpm.toml`; `checkoutFor` uses a manifest pin where present, otherwise a moving default branch. `buildPlugins` passes repository `Build` strings to `runBuildSteps`, which executes `sh -c` at line 256 with the account environment and repository as working directory. |
| Build/install | `installSo` invokes `strip` and writes `~/.local/lib/hyprland/plugins/<id>.so` plus `<id>.abi`; receipts use `<id>.json`. Asset copying replaces the target tree. Rebuild also reauthors `settings.lua` and can unload/load plugin copies in the live compositor. |
| `sync` | Reconciles enabled plugin loads/unloads and applies generated plugin configuration to the live compositor. |
| `remove <id>` | Unloads an unbundled plugin and removes its `.so`, `.abi` and receipt; the Hub path also removes the returned desktop-store key and reapplies configuration. |

This is a concrete **user-account code-execution and persistent-state
boundary**, not evidence of root execution. A repository/configured build
string can execute with the account's data access; a loaded `.so` runs in its
compositor. Source toolchain availability, ABI checks, a fixed launcher,
authentication or writer freeze coordination alone do not bound those writes
or authorize that code. The Rust `Provider` route currently forwards its argv
to the fixed private adapter without the Hub query filter, and the private
adapter is installed too. The Hub refusal therefore does not protect all
direct entrypoints. These are source-proven paths; this review runs none of
them and makes no runtime exploit or acceptance claim.

## Required retained contracts and owners

- #180 owns the Hub user/appearance interface, including explicit separation
  of pure discovery from cache writes, activation, settings and automatic
  reconciliation. Preserve both discovery and activation capabilities; a
  query label must not silently authorize loading code or writing receipts.
- #182 owns retained plugin catalog/source/install/build/update/removal.
  Its prerequisites are #25, #173, #20, #27 and #28. Downloads need immutable
  reviewed source identities, verification and size/license policy; package
  planning/execution must use #27/#28 rather than an edition package manager.
  The supported build experience needs a bounded, reviewed execution/write
  contract instead of executing arbitrary repository shell strings with the
  account's unrestricted access. This review neither imports those strings
  into a new provider nor removes the build/install capability.
- #25 supplies protected session/appearance transport, with #18/#20 complete
  capture, typed authorization, bounded ownership and linked pre/post recovery.
  All direct CLI/private-provider and UI/automatic routes must meet the same
  contract; a Hub-only check is insufficient. #176 owns configured automatic
  hook policy where those contracts intersect.

Owner implementation must prove pure inventory performs no file/cache writes,
network fetch or compositor load; separately test authorized fetch/build,
activation/rebuild/removal, path and provenance tampering, denied requests,
linked recovery and user-data preservation. This is an implementation handoff,
not tests that have passed. Native package/plugin/ABI/session acceptance and
the remaining WM dispatch/config/output/reclaim review stay open under #173
and the owning runtime issues.
