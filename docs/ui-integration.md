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
workspace contains an optional GTK 4 Rust proof binary (`gtk-ui` feature). Its
existing “Refresh status” button is a user-triggered, non-destructive request.
The default demonstration backend renders fixed sample update counts and a
service-unavailable error for visual capture. An opt-in `--connect-service`
mode sends the existing typed `Discover` request to the fixed
`$XDG_RUNTIME_DIR/ryoku-service.sock` endpoint after checking the runtime
directory and socket ownership/modes and the connected peer UID. It maps the
service's `Ready` and `RecoveryUnavailable` states into distinct UI status
messages and renders authentication failures as typed errors. Responses are
limited to 64 KiB and socket operations time out after five seconds. No
managed action is sent, and these states do not claim package-update data.

This Unix-socket adapter exercises the current service runtime as an
integration prototype; it does not settle the transport decision in ADR 0001.
Start the service as the ordinary desktop user in one terminal:

```console
cargo run --locked -p ryoku-service --bin ryoku-service
```

Then launch the GTK client in that user's desktop session in another terminal;
click **Refresh status** to issue `Discover`:

```console
cargo run --locked -p ryoku-ui --features gtk-ui --bin ryoku-ui-gtk -- --connect-service
```

GTK remains the selected M0 proof candidate, while its Tumbleweed package and
runtime dependencies, Wayland behavior, accessibility, theme integration, and
lifecycle handling still need guest validation. GitHub Rust checks install
`libgtk-4-dev` so all-feature CI can compile and test the GTK target. KDE
fallback must be included in the graphical test.

GTK's Rust bindings wrap a C toolkit internally, so keep the wrapper API at
the presentation edge, do not expose GTK/GLib objects through workspace
contracts, and do not add project-owned unsafe FFI. The service boundary must
remain usable by a retained upstream surface only if that surface can consume
the same authenticated protocol without project-owned QML/JavaScript glue.

## Remaining validation

On 2026-10-15, the GTK proof was built and launched locally in openSUSE
Tumbleweed WSL2 through WSLg, using Rust 1.99.0 and GTK 4.22.5. The ordinary
user service ran with `XDG_STATE_HOME=$HOME/.local/state`; the GTK client ran
with `--connect-service --capture-on-launch`. The operator confirmed that the
window was visible and that clicking **Refresh status** displayed recovery
unavailable. The service's private socket was present with mode `0600`.
All-feature workspace Clippy and locked tests passed. GTK logged unavailable
accessibility-bus and Mesa/EGL driver warnings; this run does not establish
accessibility or accelerated rendering support. It is a WSL client/service
smoke test, without Hyprland/KDE login, packaging, reboot, or rollback evidence.

The GTK proof and opt-in service-discovery adapter are present. The
`--connect-service` path has local protocol tests, but it has not yet been
verified against the running daemon in a Tumbleweed desktop. Run18 installed
KDE/SDDM, but its ordinary-user Wayland login check remained blocked. On a
disposable Tumbleweed graphical VM with a working login and the service
available, build the native surface and demonstrate all of the following with
sanitized evidence:

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
5. Verify the service client cannot invoke writes or bypass recovery gates;
   package and daemon activation must also be tested against the final
   transport decision in ADR 0001.

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
