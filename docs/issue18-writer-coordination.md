# Issue #18 writer and capture integration

## Pinned-source audit

Base: `Ryoku-dev/ryoku@98a0368253d9628be72b853985878a5db90e1547`, tree
`49cf710dea4a8bc2c5aa2c4b524be46c5c9a2cff`. The audit checked 1,660 non-vendor
Go/QML/JS/Rust/Lua/service/shell files against their Git blob IDs, enumerated
write/create/rename/remove/FileView/atomic-write sites and followed the retained
shell, wallpaper, Hub and Hyprland launch graphs. Machine-readable source IDs,
scan coverage and launch-patch input hashes are under
`packaging/writer-coordination/`. The scan includes read/write helpers and
transient paths; its site count is not a claim that every hit is a preference.

| Typed role | Retained payload / QML | Actual incoming route |
|---|---|---|
| `daemon` | `/usr/libexec/ryoku/upstream/ryoku-shell` | Public `ryoku-shell` alias; patched shell user unit |
| `provider` | `/usr/libexec/ryoku/upstream/ryoku-wm-hyprland` | Public `ryoku-wm-hyprland` alias used by Hub and compositor autostart |
| `hub-backend` | `/usr/libexec/ryoku/upstream/ryoku-hub` | Public `ryoku-hub` alias used by Hub QML |
| `ryogami` | `/usr/libexec/ryoku/upstream/ryogami` | Public `ryogami` alias; patched wallpaper user unit |
| `ryoshot` | `/usr/share/ryoku/shell/quickshell/ryoshot` | Patched compositor binds/ryoshot module, bar/launcher/capture actions and Ryopin action |
| `welcome` | `/usr/share/ryoku/shell/quickshell/welcome` | Patched compositor autostart first-login launch |

The three supervised Qt trees are shell (`/usr/share/ryoku/shell/quickshell/shell`),
Hub (`/usr/share/ryoku/hub/quickshell`) and wallpaper picker
(`/usr/share/ryogami/shell.qml`). They remain native children of their coordinated
Go daemon so upstream Process.Kill keeps its expected target PID; no unused
wrapper roles or nested-wrapper launch replacements are retained.
The two directly launched QML roles execute fixed `/usr/libexec/ryoku/quickshell/qs -p <fixed path>`. Widgets are part of the
consolidated shell at this pin; no `quickshell/widgets` route exists. IPC-only
`qs ... ipc call` clients target already coordinated instances and remain
children of their caller; they do not instantiate a separate preference writer.
Nested wrappers are excluded from the ancestor tracer and acquire their own
lease. Descendant fork/clone/exec and orphaned grandchildren remain covered.

The source patch changes only launch commands in retained native-language
files. It does not add Go/QML application logic. Exact input blob checks,
`git apply --check`, clean application and expected output commands are tested.
The shell-unit fixture retains the pinned upstream LICENSE and NOTICE beside it.
A release carrying the upstream desktop must retain all component notices.

## Configuration coverage and exclusions

`installed_inventory.rs` is the exact code-owned restore enrollment. It covers
port profile data; Hub/shell/theme/desktop/launcher/widget/visualizer/decor/brand
preferences; wallpaper and picker preferences/selection; capture/recording,
power/stage/notification/preferences; reviewed Hyprland seed and generated
files; fixed Kitty/Fish user overrides; supported profile hero formats and
reviewed sparse overrides. Every absent path has an explicit manifest record.
The port's typed personal layer is `opensuse-profile.json`; upstream
`profile.json` remains opaque source data. Profile input bundles preserve actual
package defaults, optional system/companion inputs and their exact revisions.

The first-sprint capture deliberately refuses unknown files in the managed
`~/.config/ryoku` / `~/.config/ryogami-wall` namespaces. This includes unreviewed
rice assets/output-profile collections and sparse overrides rather than
silently calling a partial generation complete. Only the exact `.desktop.lock`
coordination inode is ignored. An alternate XDG/config/state/cache root, custom
wallpaper cache location, external link, unsupported type/metadata or oversized
file also refuses. No unsupported state is erased or normalized. Empty unrelated
home directories and unrelated home files remain untouched.

The audit classified the remaining source hits as follows:

- Arch CLI materialize/deploy/update/recovery/doctor/plugin writers: existing
  upstream disposition is Rust replacement; these executables are not supported
  writers and are not installed/enabled by this patch.
- Niri provider: outside the accepted Hyprland provider scope.
- Software-store/app payload, plugin/rice installation, external application
  settings (including credential-bearing Voxtype), Rashin and package actuators:
  require their own accepted provider/review; this patch grants no operation.
- Runtime sockets/locks/previews, generated logs, operational caches/indexes,
  clipboard/music/calendar/notes/screentime/history and sysfs/hardware actions:
  runtime/application data rather than configuration-generation restore targets.
  The exact wallpaper outputs/state files are included despite `.cache` naming.
- Wallpaper/media/plugin references are retained as preference bytes; capture
  never follows them into arbitrary user files or executable payloads.

Adding an integrated writer or managed preference requires extending this
reviewed matrix/inventory and its tests before enabling that operation. Ordinary
external editors are not integrated writers; double capture/read-back still
rejects detectable source changes. This is not a claim to freeze arbitrary
user processes.

## Protocol and capability boundary

The account-owned wrapper creates/validates `/run/user/<uid>/ryoku-writers`
(`0700`) and `writers.lock` (`0600`, single link, stable inode). It holds a shared
flock before child launch. Each registered socket is account-owned `0666` inside
the private directory; this permits the root helper's `CAP_DAC_READ_SEARCH`
policy to connect without adding DAC-write or ptrace capabilities. Peer
credentials and a canonical-home digest authenticate the native conversation.
The root helper opens the lock read-only; Linux permits flock on that descriptor.

Capture connects every registered wrapper. Each same-account parent stops its
own descendant tasks via PTRACE_SEIZE/INTERRUPT on a dedicated tracer thread,
with fork/clone/exec events and no EXITKILL. Once every subtree is stopped, the
root reader acquires the exclusive lock and revalidates registry/inodes/leases.
New writer starts wait at their shared lock before creating a process. Checks
run again immediately before immutable generation publication and at completion.
The helper never ptraces another account and #17's service policy is unchanged.

EOF/helper SIGKILL cancels leases and resumes writers after exclusive lock
release. Wrapper SIGKILL causes kernel detach/resume and breaks capture; its
stale socket blocks future capture until explicit reconciliation. SIGTERM/INT/HUP
are forwarded to owned descendants; orderly exit removes only its own endpoint.
A pre-existing SIGSTOP is preserved and capture refused, avoiding the documented
ptrace group-stop restart exception. Tests also preserve SIGSTOP arriving during
a lease and show SIGCONT cannot bypass capture. No permanent cgroup freeze exists.
See Linux [ptrace(2)](https://man7.org/linux/man-pages/man2/ptrace.2.html).

## RPM interface for #23

Package changes remain in #23. Install the Rust `ryoku-writer` executable at
`/usr/libexec/ryoku/ryoku-writer` (root:root `0755`); relocate the four upstream
Go executables to the `upstream/` paths in the table (root:root `0755`); make
`/usr/bin/{ryoku-shell,ryoku-wm-hyprland,ryoku-hub,ryogami}` symlink to that Rust
wrapper. An optional `ryoku-wm` compatibility alias maps to `provider` too.
Do not retain a second unwrapped public copy. Install QML at the exact fixed
paths above, applying the pinned launch patch before build/staging.

The port shell unit/drop-in runs `ryoku-writer daemon daemon`; the wallpaper
unit runs `ryoku-writer ryogami`. All inherited processes must retain standard
HOME/XDG roots. A shell drop-in is supplied as declarative integration input;
#23/#25 choose the actual packaged unit ownership without importing their issue
branches here. Source patch application and wrapper tests establish #18's
contract; RPM ownership, installed Qt dependencies, headed desktop/session and
boot acceptance are evidenced by their owning issues after integration.

## Acceptance evidence

Native tests run actual multithreaded writer processes and child/grandchild
subtrees through lease acquisition, concurrent starts, nested wrappers, helper
SIGKILL/disconnect, wrapper SIGKILL, stale endpoints, termination forwarding and
SIGSTOP/SIGCONT. The exact pinned upstream Go daemon was built from verified
sources and exercised through real IPC: a pending `barstyle` write cannot alter
`shell.json` during capture, then succeeds after release. QML launch paths are
verified against exact source commands and the descendant protocol; this host
has no `qs` and this evidence does not claim a running Qt UI.

The host fixture suite covers complete compiled inventory, customization,
multilayer offline reproduction, exact metadata and safe links, absent entries,
unsafe path/archive/metadata inputs, double-capture change detection and capture
cancellation before publication. Evidence files record commands/results/hashes.
No new VM, helper capability expansion or #20 mutation state machine is included.

## Publication review follow-up (20 October 2026)

The three P1 findings on merged PR #150 are covered by focused capture changes.
The generation store checks the live writer lease before publication, after
staging immediately before the generation rename, before the transaction-index
rename, and after read-back. If a boundary check fails, it unseals the generation
(which makes verification fail), moves it back out of the generation namespace,
and removes only that attempt's exact index and staged files. Successful cleanup
permits a fresh capture with the same transaction and generation IDs. An already
verified publication from an earlier successful call is preserved on a rejected
idempotent retry.

Installed capture validates the two managed namespaces and the supported
wallpaper state location at every boundary check, including after both live
captures and around publication. This also applies to callers using
`InstalledInventory::current()` directly. Unknown entries created by an
uncoordinated same-account editor therefore reject the capture when observed at
these boundaries. Arbitrary user processes are not frozen by the writer lease;
these checks do not claim a filesystem snapshot of the home directory.

Every `UserHome` record must name a path in the compiled preference inventory.
Record construction, manifest validation, sealing and parsing share this rule,
including absent records that would otherwise request deletion during restore.
The generic generation collector additionally rejects undeclared home paths
before it reads them. Verified-generation readers therefore cannot supply
unrelated home records to configuration staging.

Regression coverage includes real wrapper SIGKILL after staging and after index
publication, deterministic lease loss at each publication boundary, successful
retry after retraction, unknown files inserted into both managed namespaces,
and a correctly hashed manifest naming unrelated home data. The follow-up
evidence is recorded in
[`issue18-20261020-publication-regressions.toml`](../infra/tumbleweed-lab/evidence/issue18-20261020-publication-regressions.toml).
These are host Rust and native-process fixtures; they add no provider transaction,
durable mutation, metadata restoration or VM acceptance claim.

## Ryoku 1.0 source refresh

The active source base is now v1.0.0 commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`; the earlier 0.75.5 write-site
audit and its `write-audit.json` remain historical evidence and do not cover
the new tree. The launch patch is rebased to the v1.0 helpers: Ryoshot now
routes through `ryoku-cmd-ryoshot`, welcome and the keys hint route through
`ryoku-session-intro`, and both helpers use fixed Rust writer roles. IPC and
Ryogami child launches use the package-private Quickshell path. The selected
source scope now includes both helpers plus the new Ryogami picker and livewall
build sources; `pinned-route-sources.json` records the new source blob IDs.

`keys-hint` is an explicit writer route to the pinned Quickshell surface. The
Ryogami picker talks to the wrapped daemon; its direct UI launch is not treated
as a second daemon writer. Re-audit 1.0's changed Go/QML/JS write sites and the
picker-to-daemon protocol before claiming the previous write-site coverage.
The native, package and VM regressions still require their owning issue
acceptance; rebasing the source patch alone does not complete #18 or #25 for
the new runtime.
