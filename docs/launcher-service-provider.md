# Read-only Rust service provider in the unchanged Ryoku launcher

`ryoku-launcher-status` connects the pinned launcher to the actual typed Rust
service without changing upstream QML/JavaScript or implementing the Go daemon
protocol. Native KDE validation now shows actual typed success and stopped-service
error rows in the unchanged launcher, with matching request IDs in native journal
events. This is an informational extension prototype. It does not complete issue
#14 or select the final transport in
[ADR 0001](adr/0001-ui-and-service-integration.md).

The preserved source is Ryoku revision
[`98a0368253d9628be72b853985878a5db90e1547`](https://github.com/Ryoku-dev/ryoku/commit/98a0368253d9628be72b853985878a5db90e1547).
Its unchanged
[`Script.qml`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/shell/quickshell/shell/modules/launcher/shared/providers/script/Script.qml)
reads `~/.config/ryoku/launcher-scripts.json`, executes an argv array directly,
and renders the rofi-script result rows in the existing launcher. The name
“script” does not require a shell script: the extension accepts a native Rust
executable. The upstream source and its applicable notices remain unchanged.

The extension sends the query through `ROFI_INFO`, with `ROFI_RETV=0` for the
list pass. Only the literal `status` and `refresh` queries with no command-line
arguments are accepted. Other queries, missing/invalid variables, custom input
passes, and row activation (`ROFI_RETV=1`, which also appends argv) return no
rows and send no request. Other environment variables cannot select an action
or endpoint. The executable invokes no subprocess or privileged helper.

Each accepted query generates an internal random request ID, sends the existing
`RefreshStatus` action through `dispatch` and `UnixServiceBackend`, and therefore
issues only the typed version-1 `Discover` request. The socket is fixed at
`/run/user/<kernel-effective-UID>/ryoku-service.sock`; client ownership, socket
mode and kernel peer-UID checks remain in force. The exchange validates protocol
version and response correlation, bounds the response to 64 KiB and applies an
absolute five-second write/read deadline. A six-second worker wait also bounds
a blocked local connect; returning from this short-lived executable ends its
worker. This does not impose a global deadline on the older GTK caller's connect.

The row distinguishes `Discover succeeded: RecoveryUnavailable` from
`Discover failed: Unavailable` and preserves every other typed client error.
It includes the actual correlation ID. Sample update counts are rejected, and
no package/update/recovery capability is invented. Process exit success means
the presentation row was delivered, including a typed error row; it does not
turn a failed service request into success. Bounded diagnostics contain only
request ID, fixed action and typed result, without query text, peer/account/session
details or backend error text. The unchanged provider discards child stderr, so
the executable also sends one best-effort nonblocking datagram per event to the
fixed native journal socket with identifier `ryoku-launcher-status`, following
systemd's [native journal protocol](https://systemd.io/JOURNAL_NATIVE_PROTOCOL/).
The request event precedes typed dispatch; the result follows the actual client
result, including validation errors or timeout. Journald failures never affect
the result row. Collect the actual ordinary user's matching identifier/UID
records; unrelated shell startup markers cannot satisfy these request gates.

The pinned launcher caches its last keyword/query, including across reopening.
Use **ryoku status**, then **ryoku refresh** for a fresh request; alternate those
queries for further reads. Repeating the exact last query can show cached data.
No manual request-ID entry is needed. Upstream exposes a generic **Select**
action on every result row; selecting this informational row sends no request
and performs no operation. An ideal refresh button and stale-state indicator
would require a separately reviewed upstream/UI change.

## Build and disposable native fixture

Build from the same reviewed source revision as the service. GTK is optional
and is not built or needed for this provider:

```console
CARGO_TARGET_DIR=/home/jin/src/ryoku-opensuse/target/issue14-build CARGO_BUILD_JOBS=2 cargo build --offline --locked --release -p ryoku-ui --no-default-features --bin ryoku-launcher-status
```

The owning VM runner verifies hashes and native `ldd` dependencies, then stages
the binary at the new root-owned mode-0755 path
`/usr/libexec/ryoku/ryoku-launcher-status`. It needs no `rofi` package or shell
helper. The service must be built from the reviewed service revision and started
as the ordinary desktop user through the runner's separately owned unit. The
provider never starts, stops or alters a daemon.

For this explicitly disposable fixture only, the runner may stage
[`packaging/examples/launcher-scripts.json`](../packaging/examples/launcher-scripts.json)
in that desktop user's `~/.config/ryoku/launcher-scripts.json` after checking
the destination is absent. Existing configuration must not be overwritten. The
complete metadata is:

```json
[
  {
    "keyword": "ryoku",
    "name": "Ryoku service discovery",
    "exec": ["/usr/libexec/ryoku/ryoku-launcher-status"]
  }
]
```

The unchanged FileView watches this file. Open the existing launcher through
the reviewed Rust session launcher, type **ryoku status** using ordinary native
keyboard input, and capture/review the actual `RecoveryUnavailable` result row
and matching diagnostics. This is a successful discovery that truthfully says
the recovery boundary is unavailable. Then the owning runner stops/reaps only
its exact owned service, types **ryoku refresh**, and captures/reviews the actual
typed `Unavailable` row with a new correlation ID. A missing/failed service
cannot establish the successful request gate; keep that gate pending until the
real daemon starts. Preserve process identity, binary hashes and both screenshot
hashes, and remove only the fixture's own config/binary during cleanup.

The [native evidence record](../infra/tumbleweed-lab/evidence/issue14-native-launcher-service.toml)
records both independently reviewed screenshots, binary/source hashes, actual
service identity, the owned stop and matching request/result journal pairs.
`ryoku status` returned the live service's `RecoveryUnavailable`; after the
owned daemon stopped, `ryoku refresh` returned actual `Unavailable`. The service
uses a separate private state child while preserving the upstream-created
mode-0755 state directory. The captures establish this read boundary inside a
KWin/Plasma-hosted shell; they do not establish a Ryoku Hyprland login.

Local checks cover literal-query/argv validation, malicious ROFI input without
dispatch, typed success/error translation, wrong correlation/protocol, rejection
of sample counts, bounded directive-free rows and slow-trickle deadline
enforcement. Neither those tests nor the native read proof establish accessibility,
theme/scale, Hyprland login, RPM installation/removal, full Hub/store integration,
or acceptance of the remaining exposed upstream controls. This extension proves
only a narrow existing-surface read boundary. Overall acceptance still requires
architecture review and the remaining issue #14 checks, including a public PR.
