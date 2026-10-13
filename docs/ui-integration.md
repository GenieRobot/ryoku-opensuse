# Rust UI integration proof

The source-backed architecture decision and Quickshell reuse assessment are in
[`ADR 0001`](adr/0001-ui-and-service-integration.md).

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
feature build has not yet reached compilation of the proof binary. The GitHub
Rust check installs `libgtk-4-dev` so all-feature CI can compile and test the
GTK target. KDE fallback must be included in the graphical test.

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

### Disposable VM visual capture

The `Rust checks` workflow retains a release `ryoku-ui-gtk` artifact only
after a successful push to `main`. Pull request runs compile and test GTK but
never publish the binary. The artifact name includes the exact 40-character
source commit SHA and expires after 14 days. It includes
`BUILD-METADATA.txt` with that commit, workflow run, Cargo.lock hash, and
binary SHA-256, plus the build host's dynamic dependency listing.

From the successful `Rust checks` workflow run for the main commit you intend
to test, download that exact artifact:

```console
MAIN_SHA=<40-character-main-commit-sha>
RUN_ID=<successful-main-push-workflow-run-id>
ARTIFACT_DIR="/workspace/ryoku-ui-gtk-$MAIN_SHA"
gh run download "$RUN_ID" --repo GenieRobot/ryoku-opensuse \
  --name "ryoku-ui-gtk-$MAIN_SHA" --dir "$ARTIFACT_DIR"
grep -Fx "source_commit=$MAIN_SHA" "$ARTIFACT_DIR/BUILD-METADATA.txt"
EXPECTED_SHA="$(sed -n 's/^binary_sha256=//p' "$ARTIFACT_DIR/BUILD-METADATA.txt")"
ACTUAL_SHA="$(sha256sum "$ARTIFACT_DIR/ryoku-ui-gtk" | cut -d ' ' -f 1)"
test "$ACTUAL_SHA" = "$EXPECTED_SHA"
```

The main-only build checks the binary's linked libraries with `ldd` and fails
the artifact job if any are unresolved on Ubuntu 24.04. The VM guest installs
Tumbleweed's `libgtk-4-1`, then runs its own `ldd` check before installing or
launching the binary. An incompatible architecture, loader, or unresolved
library stops capture with a clear error naming the missing library in the
private guest diagnostic; no window launch is credited.

Pass the verified binary to the disposable VM lab:

```console
RYOKU_LAB_AUTHORIZED=existing-cloud-workspace cargo run --locked --bin vm-lab -- \
  run /workspace/opensuse-vm /workspace/ryoku-lab-FRESH-NAME \
  --capture-ui "$ARTIFACT_DIR/ryoku-ui-gtk"
```

The optional path launches separate success and service-unavailable windows
inside the authenticated ordinary-user Plasma Wayland session and captures the
VGA framebuffer through the VM's private QMP socket. The resulting
`ryoku-ui-success.ppm` and `ryoku-ui-error.ppm` files stay inside the
mode-0700 run directory; each file is mode 0600. Normal #13 runs retain their
original package and display behavior when the option is absent.

Review both screenshots locally before sharing; sanitize visible account or
machine details first. This capture proves GTK startup and the displayed
sample outcomes only. It does not establish accessibility, theme integration,
Hyprland behavior, packaging acceptance, or a real service client. Issue #14
remains open until its separate desktop evidence and review gates pass.
