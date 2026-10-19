# Rust UI integration proof

The source-backed architecture decision and Quickshell reuse assessment are in
[`ADR 0001`](adr/0001-ui-and-service-integration.md).

The real pinned Ryoku launcher now has a narrow Rust executable provider for
typed read-only service discovery through its existing extension API. See
[`the provider contract and native fixture`](launcher-service-provider.md).
It rendered actual `RecoveryUnavailable` and stopped-service `Unavailable`
results inside the unchanged launcher in the native KDE guest. The
[evidence record](../infra/tumbleweed-lab/evidence/issue14-native-launcher-service.toml)
contains accepted screenshot and correlated journal hashes. This passes the
narrow existing-surface request/result/error component gate. Architecture/public
PR review, accessibility, theme/scale, Hyprland and packaging acceptance remain
open; the provider does not implement the Ryostore backend.

The original clipboard surface now has a separate Rust user-service candidate.
Its only QML changes are documented in
[`the upstream patch registry`](../upstream/patches/README.md): a dedicated
socket path and visible provider/operation errors. Capture, state, validation
and operations are Rust. The clipboard provider has not yet passed native UI
acceptance.

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
JavaScript. Preserve their notices and exact pin. The approved exception allows
small attributed routing, binding and result-display patches to existing
upstream surfaces; backend behavior remains in Rust. Any new Ryoku surface
therefore belongs in Rust.

Keep the Rust presentation layer separate from backend and provider code. The
workspace contains an optional GTK 4 Rust proof binary (`gtk-ui` feature). Its
existing “Refresh status” button is a user-triggered, non-destructive request.
The default demonstration backend renders fixed sample update counts and a
service-unavailable error for visual capture. An opt-in `--connect-service`
mode sends the existing typed `Discover` request to the fixed
`$XDG_RUNTIME_DIR/ryoku-service.sock` endpoint after checking the runtime
directory and socket ownership/modes and the connected peer UID. It maps the
service's `Ready` and `RecoveryUnavailable` states into distinct UI status
messages and renders authentication failures as typed errors. `Discover` now
checks helper reachability and protected-state initialization through the
read-only helper availability operation; a ready state still advertises no
managed writes and does not claim that a particular checkpoint is available.
Responses are limited to 64 KiB and each request has one five-second deadline
covering nonblocking socket connection, request writes and response reads. No
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

GTK remains an optional M0 proof candidate. The revised ADR proposes the
unchanged launcher plus Rust read adapter as the existing-surface path; that
proposal still needs review. GTK's Tumbleweed package and
runtime dependencies, Wayland behavior, accessibility, theme integration, and
lifecycle handling still need guest validation. GitHub Rust checks install
`libgtk-4-dev` so all-feature CI can compile and test the GTK target. KDE
fallback must be included in the graphical test.

Refresh executes the socket exchange on a Rust worker thread. GTK remains
responsive during the bounded service timeout, disables repeat refreshes while
one request is pending, and applies the typed result on the GTK main loop.

GTK's Rust bindings wrap a C toolkit internally, so keep the wrapper API at
the presentation edge, do not expose GTK/GLib objects through workspace
contracts, and do not add project-owned unsafe FFI. The service boundary must
remain usable by a retained upstream surface only if that surface can consume
the same authenticated protocol without project-owned QML/JavaScript glue.

## Remaining validation

The GTK proof and opt-in service-discovery adapter are present. The
`--connect-service` path has local protocol tests, but it has not yet been
visually accepted against the running daemon in a Tumbleweed desktop. Run20
passed automated request/result/paint markers but visual review found KSplash
obscuring the view, so it establishes no visible GTK-window success. The lab
now verifies that the exact RPM-owned `ksplashqml` process belongs to the
authenticated Plasma session and waits for it to exit before launching GTK;
it rechecks both splash absence and session identity immediately before spawn,
then records `ui_capture_splash=absent`. This avoids the observed overlay
race, but does not replace review of the captured framebuffer. The real
unchanged launcher subsequently passed the native typed success/error gate
described above; GTK demonstration captures are not used for that conclusion.
The recorded issue13 WSL run06 rendered the demonstration windows after a native KDE login;
those fixed responses did not exercise the daemon. On a
disposable Tumbleweed graphical VM with a working login and the service
available, build the native surface and demonstrate all of the following with
sanitized evidence:

1. Launch and render typed backend status in a real desktop window under
   Hyprland, including keyboard navigation, screen-reader labels, and current
   desktop theme behavior.
2. Preserve/review the completed KDE launcher read-only success and typed
   error evidence; repeat this actual request/result path for the other accepted
   session and packaged lifecycle.
3. Repeat launch, theme, accessibility, and action checks under the KDE
   fallback, including Wayland/XWayland behavior where relevant.
4. Package and install the UI and required runtime dependencies through the
   declarative RPM metadata, verify clean removal, and confirm it does not
   replace distro-owned desktop files.
5. Verify the service client cannot invoke writes or bypass recovery gates;
   package and daemon activation must also be tested against the final
   transport decision in ADR 0001.

Until the remaining graphical gates and architecture review pass, M0-05 remains
incomplete and dependent graphical UI work remains blocked at the integration
decision. The terminal
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

### Real daemon capture

The same lab has a separate opt-in real daemon lane. Build the GTK binary,
`ryoku-service`, and `vm-lab` from the same reviewed source revision, then add
`--capture-user-service` to the graphical run:

```console
RYOKU_LAB_AUTHORIZED=existing-wsl-vm cargo run --locked --features wsl-lab --bin vm-lab -- \
  run /path/to/verified-assets /path/to/fresh-run --accelerator kvm \
  --capture-ui /path/to/ryoku-ui-gtk \
  --capture-user-service /path/to/ryoku-service
```

The daemon option requires `--capture-ui` and rejects console-only runs. The
host checks both executable inputs, freezes them in the private run directory,
and records their hashes. The guest verifies the daemon's hash and native
dependencies before installing it at `/usr/libexec/ryoku/ryoku-service`; an
existing destination is refused. This fixture installation does not test RPM
ownership, install/remove behavior, or the production user service unit.

The daemon runs as the existing authenticated ordinary desktop user in that
user's normal runtime directory. A separately named transient user unit owns
and reaps the process. The fixture refuses a pre-existing unit/socket, records
the PID and process start identity, and checks its UID and executable before
stopping it. A five-minute native lifetime bound also applies. No root helper
is installed or started by this lane.

The first window waits for **Enter** supplied through the existing QMP input
path; it does not click Refresh programmatically. It sends the real typed
`Discover` request and must render `RecoveryUnavailable`, the daemon's actual
read-only discovery result. This is a successful request, not a claim that
recovery is available. The fixture then stops/reaps its daemon, opens a fresh
window, and requires a real connection failure rendered as typed `Unavailable`.
Each scenario must emit exactly one fresh request, the matching typed result,
and an after-paint marker for that request before the framebuffer is captured.
The bounded public evidence distinguishes this lane from demonstration mode;
runtime details and screenshots remain private until reviewed and sanitized.

These checks establish neither assistive-technology announcements nor complete
keyboard navigation, theme/scale behavior, Hyprland support, or packaging.
They also do not resolve the existing-surface conflict: the pinned Ryostore's
visible mutation controls cannot currently be supported by the approved Rust
adapter, and the GTK status proof is not its representative replacement.
ADR 0001 stays proposed and issue #14 stays open pending that review and the
remaining desktop acceptance evidence.
