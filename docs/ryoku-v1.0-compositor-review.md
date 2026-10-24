# Ryoku 1.0 compositor semantic review

This is a partial #173 semantic review of the retained compositor surface at
upstream `a59d051f0e8080052216f6c7b9eb7e5928935ce8`. It does not complete the
compositor delta row, certify a protected mutation or replace native acceptance.
Hyprland remains the initial session under #25; Niri #46 and Wayfire #185 are
distinct retained targets. No component, plugin capability or source root is
removed by this review.

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
