# Rust UI integration proof

M0-05 adds an executable Rust proof in `suse/ui`: a typed dashboard status,
one explicit `RefreshStatus` action, and typed success and service-error
results. The action is read-only. The small terminal surface shows the
request/result flow and can render the unavailable-service path with
`--service-unavailable`. `UiBackend` is the narrow seam for a future
unprivileged service client; it grants the surface no provider or privileged
operation handle. The included Rust tests describe successful state delivery
and error preservation.

This is a contract and rendering proof, not the required graphical desktop
proof. The demo backend supplies fixed sample values and does not connect to a
system service. Do not treat it as evidence that status is accurate or that a
Tumbleweed graphical application launches.

## Integration decision

The repository currently contains no pinned Ryoku UI source tree, only the
upstream inventory and source pin. Upstream Quickshell surfaces are QML and
JavaScript. Existing upstream files may be retained unchanged with their
upstream notices after compatibility and protocol review; project-owned
changes to those executable files are outside the approved Rust-only rule.
Any new Ryoku surface therefore belongs in Rust.

Keep the Rust presentation layer separate from backend and provider code. The
next candidate to evaluate is a GTK 4 Rust surface communicating with an
unprivileged Rust service over a versioned, typed IPC contract (D-Bus is the
first IPC candidate). This keeps package/provider actions behind the service
and avoids a UI-to-provider FFI boundary. This is a candidate, not a final
toolkit or IPC selection: accessibility, Wayland behavior, desktop theme
integration, dependency availability, and lifecycle handling remain to be
proved on Tumbleweed. KDE fallback must be included in the graphical test.

GTK's Rust bindings wrap a C toolkit internally, so keep the wrapper API at
the presentation edge, do not expose GTK/GLib objects through workspace
contracts, and do not add project-owned unsafe FFI. The service boundary must
remain usable by a retained upstream surface only if that surface can consume
the same authenticated protocol without project-owned QML/JavaScript glue.

## Remaining validation

After Run17 completed its helper-copy/migration phase, the workspace built and
tested successfully; the terminal proof rendered both its sample success and
service-unavailable paths. Run17 installed KDE/SDDM, but its ordinary-user
Wayland login check remained blocked. On a disposable Tumbleweed graphical VM
with a working login, build the selected native surface and demonstrate all
of the following with sanitized evidence:

1. Launch and render typed backend status in a real desktop window under
   Hyprland, including keyboard navigation, screen-reader labels, and current
   desktop theme behavior.
2. Dispatch the read-only refresh action and display both a successful typed
   result and a typed service error.
3. Repeat launch, theme, accessibility, and action checks under the KDE
   fallback, including Wayland/XWayland behavior where relevant.
4. Package and install the UI and required runtime dependencies through the
   declarative RPM metadata, verify clean removal, and confirm it does not
   replace distro-owned desktop files.
5. Replace the demonstration backend with the real unprivileged service client
   only after its authenticated/versioned contract and error behavior are
   defined; verify it cannot invoke writes or bypass recovery gates.

Until this graphical proof is recorded, M0-05 remains incomplete and dependent
graphical UI work remains blocked at the integration decision. The terminal
contract tests and output are not substitutes for that proof.
