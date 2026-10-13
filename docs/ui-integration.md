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
workspace now contains an optional GTK 4 Rust proof binary (`gtk-ui` feature).
It reuses the typed read-only refresh contract and renders status, request
result, and service-unavailable error with GTK labels and a named,
keyboard-activatable button. The sample backend uses fixed demonstration data;
it is not a system service client. GTK remains the selected M0 proof candidate,
while its Tumbleweed package/runtime dependencies, Wayland behavior,
accessibility, theme integration, and lifecycle handling still need guest
validation. The executor currently lacks GTK/GLib development metadata, so a
feature build has not yet reached compilation of the proof binary. KDE fallback
must be included in the graphical test.

GTK's Rust bindings wrap a C toolkit internally, so keep the wrapper API at
the presentation edge, do not expose GTK/GLib objects through workspace
contracts, and do not add project-owned unsafe FFI. The service boundary must
remain usable by a retained upstream surface only if that surface can consume
the same authenticated protocol without project-owned QML/JavaScript glue.

## Remaining validation

After Run17 completed its helper-copy/migration phase, the workspace built and
tested successfully; the terminal proof rendered both its sample success and
service-unavailable paths. The GTK proof source and lockfile are now present,
but the feature build requires system GTK4/GLib development packages and has
not yet compiled locally. Run17 installed KDE/SDDM, but its ordinary-user
Wayland login check remained blocked. On a disposable Tumbleweed graphical VM
with a working login, build the native surface and demonstrate all
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
