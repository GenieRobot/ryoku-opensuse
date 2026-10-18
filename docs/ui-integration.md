# Rust UI integration proof

The source-backed architecture decision and Quickshell reuse assessment are in
[`ADR 0001`](adr/0001-ui-and-service-integration.md).

The existing pinned Ryoku launcher now has a narrow Rust executable provider
for typed service discovery. Its native KWin/Plasma proof rendered the live
service's actual `RecoveryUnavailable` result, then typed `Unavailable` after
that owned service stopped. Both reviewed visible rows match actual native
journal request/result IDs. The [provider contract](launcher-service-provider.md)
documents the original argv/rofi extension, source attribution, invocation/cache
limits, exact separate issue #17/#25 prerequisites and reproduction procedure.
The [evidence record](../infra/tumbleweed-lab/evidence/issue14-native-launcher-service.toml)
preserves source/binary/capture hashes. The provider was accepted in
[PR #145](https://github.com/GenieRobot/ryoku-opensuse/pull/145), integrated at
`3e1bba87a9aef57adb0a6ba53b46d2aa9da7eb6d`. The
[proof follow-up PR #143](https://github.com/GenieRobot/ryoku-opensuse/pull/143)
records the optional GTK keyboard and package metadata and its accessibility
review correction. This follow-up does not close #14.

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

GTK remains an optional proof candidate. The revised ADR proposes the unchanged
launcher plus Rust adapter for the existing-surface read boundary, pending
architecture review. GTK's Tumbleweed package and
runtime dependencies, Wayland behavior, accessibility, theme integration, and
lifecycle handling still need guest validation. The declarative
[`ryoku-ui-proof.spec`](../packaging/rpm/ryoku-ui-proof.spec) describes a
separate experimental proof package with a GTK runtime dependency and
Ryoku-owned desktop entry/icon paths. It does not establish an RPM install,
removal, or dependency result. GitHub Rust checks install
`libgtk-4-dev` so all-feature CI can compile and test the GTK target. KDE
fallback must be included in the graphical test.

The GTK control has an `_Refresh status` mnemonic (`Alt+R`) and remains
the window's default button, so GTK also provides normal Tab focus and Enter
activation. The request-result label is the only live region, with GTK's polite
`Status` role for successful and failed read requests. The separate summary is
an ordinary label: a success is announced once, and failures clear stale
summary text. The button publishes its label and mnemonic to GTK's accessibility
API. These are source-level semantics only. Run06's refresh was
dispatched programmatically, so it does not prove keyboard activation; neither
Run06 nor the current CI proves focus visibility or announcements through
AT-SPI/Orca.

## Acceptance evidence for issue #14

The native component proof uses Tumbleweed Snapshot 20261013, Plasma/KWin
Wayland, pinned Ryoku `98a0368253d9628be72b853985878a5db90e1547` and unchanged
Quickshell 0.3.2 at `4f508be500dea6e5732cc3d50382a0048b17e7b1`.
The [runtime evidence](../infra/tumbleweed-lab/evidence/issue14-real-ryoku-runtime.toml)
records that dependency closure; the
[launcher evidence](../infra/tumbleweed-lab/evidence/issue14-native-launcher-service.toml)
records ordinary keyboard dispatch, actual service success and stopped-service
error, and matching visible request IDs. On 18 October, both retained screenshots
were viewed again and their SHA-256 values and the journal digest matched the
record. This rechecks existing evidence; it is not a new VM run.

| Issue requirement | Evidence and limit |
| --- | --- |
| Inspect pinned UI boundaries and candidates | [ADR 0001](adr/0001-ui-and-service-integration.md) compares the original launcher extension, Ryostore child-process boundary and optional Rust GTK surface. |
| Typed state and a non-destructive existing-surface action | [PR #145](https://github.com/GenieRobot/ryoku-opensuse/pull/145) implements only `RefreshStatus`/`Discover`; native success is `RecoveryUnavailable`, then a genuinely stopped service produces typed `Unavailable`. |
| Rust integration and upstream attribution | The provider and client are Rust. The pinned launcher/QML/JavaScript remain unchanged with their notices; native JSON selects the existing argv extension. No Go protocol impersonation or new executable glue is introduced. |
| Accessibility, theme, packaging and FFI constraints | [ADR constraints](adr/0001-ui-and-service-integration.md#ui-accessibility-theme-ffi-and-packaging-constraints) record these limits. This PR adds optional declarative proof RPM metadata and one GTK result live region; it supplies no screen-reader or package install/removal acceptance. |
| Surface conflict and architecture decision | ADR 0001 remains proposed. Full Ryostore mutation controls lack an approved Rust provider path; dependent full-surface work remains blocked until that conflict is resolved. The launcher read path is the demonstrated scope. |

The representative existing-surface request/result/error component gate has
passed. Architecture acceptance and the issue's completion decision remain with
the coordinator; broader product accessibility, theme, Hyprland and package
lifecycle validation remain explicitly unclaimed.
The [18 October review evidence](../infra/tumbleweed-lab/evidence/issue14-pr143-review-oct18.toml)
records the GTK feature build, all 17 UI/protocol tests without skips, Clippy,
RPM spec parsing, resolved host libraries and the unavailable desktop entry
validator, alongside the original capture digest recheck.

GTK's Rust bindings wrap a C toolkit internally, so keep the wrapper API at
the presentation edge, do not expose GTK/GLib objects through workspace
contracts, and do not add project-owned unsafe FFI. The service boundary must
remain usable by a retained upstream surface only if that surface can consume
the same authenticated protocol without project-owned QML/JavaScript glue.

## Product validation beyond the read proof

The GTK proof and opt-in service-discovery adapter are present. The
`--connect-service` path has local protocol tests, but it has not yet been
verified against the running daemon in a Tumbleweed desktop. Run18 installed
KDE/SDDM, but its ordinary-user Wayland login check remained blocked. The later
launcher proof recorded above passed the native existing-surface discovery and
real stopped-service error component; it used separately recorded service and
session source, not a new capture option in this branch. On a
disposable Tumbleweed graphical VM with a working login and the service
available, build the native surface and demonstrate all of the following with
sanitized evidence:

1. Launch and render typed backend status in a real desktop window under
   Hyprland, including keyboard navigation, screen-reader labels, and current
   desktop theme behavior.
2. Preserve/review the completed KDE launcher read-only success/error evidence
   and repeat the real request path for the other accepted session and packaged
   lifecycle.
3. Repeat launch, theme, accessibility, and action checks under the KDE
   fallback, including Wayland/XWayland behavior where relevant.
4. Package and install the UI and required runtime dependencies through the
   declarative RPM metadata, verify clean removal, and confirm it does not
   replace distro-owned desktop files.
5. Verify the service client cannot invoke writes or bypass recovery gates;
   package and daemon activation must also be tested against the final
   transport decision in ADR 0001.

These checks are product acceptance work beyond the demonstrated representative
read proof. Issue #14 requires its constraints to be recorded, and its
architecture decision still needs review. Dependent full-surface mutation UI
work remains blocked by the documented Ryostore/provider conflict. The terminal
contract tests and demonstration output do not replace native proof for any
additional supported surface.

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
