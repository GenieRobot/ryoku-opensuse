# ADR 0001: Rust UI and Ryostore integration boundary

- **Status:** Proposed for review; native KDE launcher read proof recorded,
  with architecture, desktop and packaging acceptance still pending.
- **Date:** 2026-10-13
- **Evidence revision:** 2026-10-16
- **Issue:** [M0-05, #14](https://github.com/GenieRobot/ryoku-opensuse/issues/14)
- **Source baseline:** Ryoku commit
  [`98a0368253d9628be72b853985878a5db90e1547`](https://github.com/Ryoku-dev/ryoku/commit/98a0368253d9628be72b853985878a5db90e1547),
  recorded in [`upstream/ryoku.source.toml`](../../upstream/ryoku.source.toml).

## Proposed decision

Retain the pinned upstream shell/launcher unchanged as the existing-surface
frontend, with supported reads connected through narrow project-owned Rust
executables to the versioned, authenticated Rust service contract. Keep GTK as
an optional Rust contract/diagnostic surface. The unchanged launcher has now
rendered a live service discovery result and the real error after its owned
service stopped. This supplies a representative read boundary, not acceptance
of the complete shell, Hub, store or write controls.

The tested prototype uses the existing authenticated per-user Unix socket.
D-Bus remains an alternative requiring separate design and native validation;
this proof does not select the final product transport. The UI and adapter stay
unprivileged, and only the service owns provider capabilities. Accepting this
proposal requires explicit review and the remaining issue #14 desktop gates.

Do not import or modify the upstream Ryostore surface as part of M0-05. The
reuse finding is: **a read-only slice is technically reusable without
project-owned non-Rust glue, but complete product reuse is not currently
supportable.** The pinned surface's backend boundary is a `ryostore`
child-process command protocol, so a project-owned Rust compatibility
executable could preserve the existing QML/JavaScript files. That path is not
an implemented integration. Full reuse would require a Rust adapter to safely
implement every exposed operation and preserve the old data/command semantics.
The existing store exposes installs, removals, settings, cache warming, and
catalog refresh across content types whose Arch-specific or arbitrary
installer behavior is outside the approved openSUSE write path. A stub or
failing adapter would leave misleading live controls; changing those controls
would itself change upstream executable QML. Therefore the complete surface is
not currently reusable within the approved scope. A future issue may assess a
read-only, clearly scoped reuse path or a full adapter after typed service and
provider contracts exist.

The GTK binary in `suse/ui` remains a proof, not a finished Ryostore
replacement. Its refresh action and sample backend establish a minimal
request/result/error presentation boundary; they do not establish store
compatibility, real service behavior, or completion of #14.

## Pinned integration evidence

The source pin is immutable and the relevant files are not vendored in this
repository. The conclusions below come from direct inspection of that pinned
tree, not from the moving upstream branch.

- [`Store.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/Singletons/Store.qml#L1-L31)
  imports `Quickshell.Io` and exposes catalog, item, loading, offline, error,
  and operation fields as QML `property var`, booleans, and strings. Its
  `refresh()` runs `ryostore catalog [--refresh]`; `install()` and `remove()`
  pass category and item identifiers as argv strings; `openSettings()` starts
  another CLI command.
- [`Store.qml` process handlers](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/Singletons/Store.qml#L160-L239)
  parse catalogue stdout with `JSON.parse`, read operation errors from stderr,
  and treat process exit codes as success/failure. `check` separately parses
  JSON and malformed values fail closed for the update indicator.
- [`backend/main.go`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/backend/main.go#L1-L59)
  identifies the Go process as the store data plane and dispatches the `catalog`,
  `install`, `remove`, `warm`, `check`, `open`, `settings`, and `internal`
  commands. The project cannot retain this backend as newly authored
  openSUSE executable code; any replacement must be Rust.
- [`shell.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/shell.qml#L24-L28)
  has a separate Quickshell `IpcHandler` for local navigation (`open` and
  `section`). It is not the store data/action protocol and does not provide a
  typed Rust service connection.
- [`ryostore.desktop`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/ryostore.desktop#L1-L10)
  launches `flock -n -o /tmp/ryostore.lock qs -c ryostore`, which adds launcher,
  lock-file, executable-path, and Quickshell runtime assumptions to packaging.
- The application composes the custom [`Ryoku.Ui` QML module](https://github.com/Ryoku-dev/ryoku/tree/98a0368253d9628be72b853985878a5db90e1547/ryoku/ui),
  `Ryoku.Ui.Singletons`, store-local QML, and `store.js`; retaining only the
  visible `App.qml` would not retain a complete loadable surface.
- The upstream surface has a substantial interaction model: keyboard selection
  and navigation are explicit in [`App.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/App.qml#L339-L355)
  and [`ProductGrid.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/ProductGrid.qml#L76-L99).
  It also assigns accessible roles/names to many controls, including the
  product graphic and detail actions in
  [`ProductCover.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/ProductCover.qml#L30-L42)
  and [`ProductDetail.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/apps/ryostore/quickshell/ProductDetail.qml#L698-L745).
  Source annotations are useful evidence of intent, not an Orca/AT-SPI test.
- Store colors, typography, motion, and scaling come from [`Tokens.qml` role
  resolution](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/ui/Singletons/Tokens.qml#L20-L46)
  and its [`motion and display scaling`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/ui/Singletons/Tokens.qml#L152-L188).
  A native GTK surface will not inherit those values automatically.

The workspace pins this source but vendors no upstream files. Its inventory
already records the Go/Arch provider assumptions and openSUSE Rust replacement
direction in [`docs/upstream-inventory.md`](../upstream-inventory.md#component-and-interface-inventory).

## Typed service contract requirements

Before connecting either UI, define a versioned service protocol with domain
types rather than copying the existing JSON fields directly into QML or GTK.
At minimum:

- Use a versioned interface and typed request/response enums. Requests should
  carry an opaque request ID and a domain action; responses should return the
  matching ID and a typed success value or typed error. Unknown required
  versions, variants, and fields must fail closed.
- Model loading, stale data, offline state, unsupported operations, and backend
  errors as distinct states. Do not turn an absent field or unknown operation
  into success. Text for presentation belongs in the UI, not in provider
  protocol fields.
- Keep status/catalog reads distinct from mutations. Each mutation must be
  allowlisted and pass the authorization and verified linked pre-change capture
  gates in #1. A UI disconnect must not cancel an accepted operation; a long
  operation needs a durable operation ID and queryable typed progress/result.
- Authenticate the client as the logged-in user and keep the UI unprivileged.
  The service must not hand provider commands, arbitrary executable paths, or
  generic root authority to the UI. A narrow helper remains a separate reviewed
  trust boundary.
- Bound request size, catalogue count, string lengths, concurrency, and response
  time. Preserve a structured error code and safe display detail; never use
  stderr text as the only machine-readable error channel.

A Rust `zbus` client/service is a candidate implementation of this typed IPC
edge, not a dependency selected by this ADR. It must be reviewed for session
bus peer identity, service activation, cancellation, and signal ordering before
adoption. Same-user bus credentials identify the process user; they do not
authorize a privileged write or replace explicit operation policy. GTK and
GLib objects stay inside `ryoku-ui`; QML and QuickShell handles do not cross the
Rust workspace API boundary.

For a Quickshell compatibility route, the Rust adapter may translate the
typed service contract to the pinned CLI's argv/stdout/stderr behavior. This
is an explicitly lossy boundary: the Rust side must validate every argument
and JSON value, map nonzero exits to typed errors, include a version marker for
its own schema where the untouched client tolerates it, and reject unsupported
commands/categories. The old QML contract itself remains dynamically typed.
This route is a compatibility option, not the target architecture for new
project-owned screens.

## UI, accessibility, theme, FFI, and packaging constraints

### GTK Rust surface

- GTK4 and gtk-rs are the current native proof candidate and fit a project-owned
  Rust surface. Use GTK accessible widget roles/names, keyboard activation,
  visible focus, scalable text/layout, high-contrast colors, and reduced-motion
  behavior. Verify through AT-SPI/Orca on the actual desktop; source-level
  labels are not sufficient.
- GTK uses desktop GTK settings and may not match Ryoku's custom live wallpaper
  palette. If preserving that palette is a product requirement, define a typed
  palette/state input and contrast rules instead of hardcoding current colors
  into widgets. Test light/dark, high contrast, scale, and theme changes under
  both Hyprland and KDE.
- Use safe gtk-rs wrappers at the UI edge. Keep GTK/GLib values out of shared
  domain/service crates. No project-owned `unsafe` or FFI is needed for the
  current proof. Any future C/Qt bridge requires a narrow module, a written
  safety/ownership contract, boundary tests, and independent review.
- Package the app, desktop entry, icons, and selected GTK runtime via
  declarative RPM metadata with unique Ryoku-owned paths. Verify dynamic
  dependencies against Tumbleweed, install/remove ownership, and that no file
  replaces GTK, Qt, Quickshell, KDE, SDDM, or systemd files.

### Unchanged Quickshell surface

- If a future scope chooses this route, import only the exact pinned files and
  required `Ryoku.Ui`, `Ryoku.Ui.Singletons`, translations, fonts/assets, and
  Quickshell QML module dependencies. Preserve GPL and per-file notices. No
  project-owned QML, JavaScript, or plugin glue may be added under the current
  language rule; upstream files must remain unchanged and clearly attributed.
- The current surface has explicit keyboard and accessibility annotations,
  but the palette and scale depend on Ryoku singleton files. Preserve the whole
  visual/a11y dependency closure or replace it with a newly authored Rust
  surface; do not ship a partial broken QML module.
- Actual native launch rejected `noctalia-qs 0.0.12` at the unchanged shell's
  `DefaultEnv` pragma. Unchanged upstream Quickshell 0.3.2, revision
  `4f508be500dea6e5732cc3d50382a0048b17e7b1`, built against native Qt 6.11.2
  and loaded the shell. The [runtime evidence](../../infra/tumbleweed-lab/evidence/issue14-real-ryoku-runtime.toml)
  records source, dependency, plugin, font and accepted visible-result hashes.
  This proves neither complete Hub/control compatibility nor RPM acceptance.
- A Rust CLI adapter can avoid project-authored non-Rust glue, but it must not
  let legacy UI actions escape the accepted allowlist or recovery gates. Until
  it safely covers the visible action set, do not present unchanged Ryostore as
  a supported openSUSE store.

## Consequences and verification plan

This keeps the new integration boundary Rust-owned and leaves the upstream
surface attributable and untouched. It does not preserve the complete Ryoku
store experience yet. The current GTK sample remains backend-independent and
cannot prove the selected service transport.

A [Rust executable provider](../launcher-service-provider.md) now uses the
pinned shell's existing argv/rofi-script extension to render only typed
`Discover` outcomes in the unchanged launcher. In the native KWin/Plasma guest,
ordinary keyboard input produced actual `RecoveryUnavailable` from the active
ordinary-user Rust service, then `Unavailable` after that exact owned service
was stopped and reaped. Both reviewed visible rows match the actual provider's
journal request/result IDs. No Go protocol listener or sample response supplied
these results. The [native evidence](../../infra/tumbleweed-lab/evidence/issue14-native-launcher-service.toml)
records the accepted captures and exact source/binary/log hashes.

The service fixes from issue #17 at `9f2908c45e97ae443d72a5af2b3f56d0e15a812d`
and session launcher from issue #25 at `2057a0fcada1630e24a8fde53970cbb73b6671f1`
are separate tested-native prerequisites, not implementation changes in this
focused provider publication. The provider's cached-query and informational
selection limits remain explicit. This route implements a representative
existing-surface read, while leaving Ryostore compatibility and this ADR's
overall acceptance open.

M0-05 cannot be accepted until a disposable Tumbleweed GUI run demonstrates the
remaining keyboard/focus and screen-reader accessibility, theme/scale behavior,
Hyprland and KDE fallback acceptance, and package install/removal. The native
KDE read-only request/result/error component passed; architecture/public PR
review, the remaining controls and full M0-05 acceptance remain pending.
