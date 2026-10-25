# Ryoku 1.0 appearance transport: issue #25 candidate

This is the first protected appearance transport slice, not desktop acceptance.
The full pinned Ryogami daemon, picker, editor and livewall source/package/UI
remain present. This slice never grants general daemon authority or disables UI
features to satisfy acceptance. A retained route without its complete contract
returns an explicit guarded-unavailable response until its owning issue lands.

## Call and recovery contract

The session's Rust `ryoku-writer` owns `/run/user/UID/ryogami.sock` (0600), checks
kernel peer credentials and parses every bounded frame, including frames after
subscription. It retains all anonymous query descriptors, forwards only the
query allowlist and bounds replies; a UI never receives a native descriptor.
The native daemon starts only with the Rust parent's inherited control fd and
has no named listener/fallback. SCM_RIGHTS transfers one R or A connection at a
time. R also checks the native query allowlist; A accepts exactly one canonical
`wall.apply`, `wall.set_audio`, `wall.pause` or root-prepared `broker.workspace` frame/permit
and EOF, returns one reply and closes. Labels alone
confer no public authority.

Public static/video apply, wallpaper audio and manual workspace assignment go to
the fixed root helper with a typed action and
RPC id. Kernel identity plus the existing operator-group policy authorize the
account. The root provider resolves the unambiguous latest successful Complete
post-checkpoint for that account, verifies its generation and live historical
Snapper receipt, and revalidates the bundle against installed package revisions.
Missing, stale or timestamp-tied provenance fails closed. No client bundle choice,
plan digest or actor identity is accepted as authorization.

`review_operator_plan` and `run_operator_mutation` acquire the existing machine
mutation lock and account writer leases. The latter holds that lock, journal and
leases across authorization, linked pre-capture/proof revalidation, durable
Applying, one exact actor call, full refreeze, health/post-capture and Complete.
Only the broker-owned actor TGID and verified descendants are continued; every
other captured writer remains stopped. PID/start time, root-owned pinned image
metadata and `/proc/PID/exe` device/inode are independently checked. An actor
error still refreezes before response. Timeout, exec mismatch, failed refreeze or
poisoned lease cannot report success. The root helper returns `wallpaper_applied`
only after rereading Succeeded/Complete with a post-checkpoint and no error.

This is evidence for the pinned actor's ordinary fork/clone/exec behavior, not
universal hostile-process containment. `CLONE_UNTRACED`, namespace manipulation
and equivalent malicious native-code paths are not admitted by the reviewed
actor implementation and are not proven contained by these ptrace tests.

The approved user apply persists the existing outputs/legacy wallpaper state and
catalogue count. Exact persistent inventory includes
`.cache/ryogami/outputs.json`, `.local/state/ryoku-wallpaper`, and the newly enrolled
`.cache/ryogami/wallpaper/index.json` and `state.json`, alongside the existing
installed Ryoku inventory. These persistent files use no-follow regular-file policy
and the existing 16 MiB per-file capture cap. Referenced media and derived
thumbnail/animation/transcode caches remain outside this persistent inventory;
no recursive cache-directory enrollment is introduced. Static input is capped
at 64 MiB and video at 8 GiB before hashing, with a ten-second hash deadline.
Oversize state/input fails before mutation with an actionable error.

## Protected wallpaper audio controls

`wall.set_audio` has its own typed action, operation key, helper outcome and
single-use permit. It accepts mute and/or integer volume 0–100 for at most sixteen
unique output names. A lone `*` becomes the upstream all-output request. No apply,
automatic, shell, file-path or provider action can share this permit.

The actor preserves upstream active-workspace precedence and shared-output
fallback, persists only selected outputs and active assignment audio, then
republishes the live wallpaper frame. `.cache/ryogami/workspaces.json` is enrolled
as an exact regular, single-link persistent file with the existing 16 MiB cap;
absence, bytes and modes are captured and restored. No playlist/media directory
is recursively enrolled. The root verifies the bounded, unique output/workspace
receipt against captured post-state before a `wallpaper_audio_set` success.
Native replies retain `result.ok=true` alongside that receipt.

Every target and destination is checked before the first write. External/WE
player effects require #175's provider route; the UI and original implementation
remain present. Empty, oversized or unsupported target sets return a specific
failure. Audio does not inherit #176 automatic restore/watch/theme permission.

The existing state-save primitive now uses an exclusive random temporary file
in the destination directory, syncs it and renames it atomically. It preserves
an existing regular file's mode and refuses symlink/hardlink destinations;
legacy wallpaper state uses the same primitive. Predictable `.tmp` aliases are
never opened. Owned temporary files are removed on errors. The protected audio
path propagates persistence errors through actor refreeze and failed journal
handling, so a partial I/O failure cannot report success.

A two-file audio update snapshots exact prior bytes/absence before replacing
either destination. Failure compensates every attempted destination in reverse
order, including the failing one. The actor updates in-memory workspace state
and publishes the live frame only after both replacements succeed. Compensated
failure still returns an error. If compensation fails, its error explicitly
requires linked recovery. Fault injection covers failure after the first real
replacement, successful compensation, prior absence and failed compensation.
A reopened root-journal test proves audio failure remains
`Applying/MayHaveMutated` with its verified pre-checkpoint, no success
post-checkpoint and no Succeeded status; further operations require explicit
reconciliation. No successful post-state is invented for a failed action.

Default-parallel testing also exposed a kernel event ordering boundary: `/proc`
can reveal a stopped newborn before the tracer consumes its parent's queued
FORK/CLONE event. Reconciliation waits only for a same-tracer kernel-stopped
task, with a fixed deadline; scanning/retrying cannot assign actor ownership.
Only the actual parent event can enroll the child and inherit the parent's actor
marker. Wrong tracers, running unknown children, stale identities or running
unrelated siblings still refuse. Missing events time out without granting actor resume.
The test also reaps its prior owned fork-writer before starting the next phase.
The [retained full red/corrected logs and five outer repetitions](../infra/tumbleweed-lab/evidence/issue25-audio-failure-regression-20261025.log)
record the original failures and the exact final default-parallel success.

Direct socket tests cover typed audio routing/denial without a query-channel
write. Real native receiver tests exercise output plus active-workspace
persistence, named-output precedence, live frame publication, query-channel
refusal and single-use consumption. Both outputs and workspaces have symlink
and hardlink no-write/sentinel checks followed by successful valid operations;
an exclusive-temp sentinel/mode check covers the atomic primitive. These are
local protocol/state proofs, not installed audio-device or full VM acceptance.
See the [audio receipt](../infra/tumbleweed-lab/evidence/issue25-wallpaper-audio-20261025.toml).

## Manual native and in-shell playback pause/resume

`wall.pause` retains its original boolean/default-pause and output selection
contract. Its strict public/helper language accepts no PID, player list, command
or resume token. A separate root `PauseWallpaper` action uses the same
machine-lock, authorization, Applying, linked pre/post and durable completion
path. The frozen writer lease supplies the exact installed native-player
PID/start set; that set joins the revalidated plan and private permit. The
writer checks it again against the authenticated actor's exact installed image,
account and descendant identities before dispatch and after refreeze.

Only already-stopped `/usr/bin/ryogami-live` descendants of that pinned actor can
receive a private preserved-stop registration. It binds the actor and player
start times, UID, installed image descriptor and pidfd. Unknown, unrelated or
stale stopped processes retain the existing refusal. Ordinary apply/audio/manual
assignment grants no resume: registered players remain ptrace-held during the
actor call, then receive a real pidfd STOP before detach. A subsequent capture
recognizes the same stop without losing the user's playback choice.
Successful release now also requires observing every thread in that exact
untraced player group in real `T` state (or pidfd-proven process death).
Queued STOP and transient running states are not completion. Identity ambiguity
returns a bounded failure; only successfully admitted pins and unsuccessful
detach entries remain owned by the dedicated retry guardian. Rejected stale
registrations never enter that handoff or block construction cleanup. Only the
existing exact authorized player set can receive CONT.
The [group-stop receipt](../infra/tumbleweed-lab/evidence/issue25-detach-group-stop-20261025.toml)
preserves the hosted immediate-state failure, deterministic pending-state test
and real capture/detach/re-capture regressions; no native-player acceptance is
inferred.

The native manual handler validates its owned `os.Process` identities and start
set before signalling; it never uses raw negative/reusable process-group PIDs.
It preserves the upstream manual-output/global effective-pause behavior, changes
only transient manual/frame/player state and publishes no theme/hook request.
An explicit root-authorized `paused=false` plus a validated effective-unpaused
receipt can release only that lease's exact player pins. A SIGCONT while held
cannot bypass refreeze/post-capture; resume happens at authorized lease release.
A still-effective pause caused by another manual selection stays stopped.

Playback intent is transient, as in upstream; no new persistent state file or
package actuator is invented. Linked checkpoints still cover the existing
account inventory, not restoration of process memory or playback position. A
failure after dispatch remains Applying/MayHaveMutated with its pre-checkpoint
and no invented successful post-checkpoint. Fresh wallpaper frames now inherit
the effective pause bit so an ordinary new selection does not silently clear
in-shell pause. External `paper.pause` retains its source/UI and requires #175's
provider contract; automatic pause/theme/hook routes remain #176.

The real ptrace/pidfd regression covers two capture→ordinary actor call→detach→
re-capture cycles, stale identity, unauthorized resume, pause intent refusing
resume, actual SIGCONT while held, and explicit post-refreeze release. Native
inherited-channel fixtures cover stale player rejection, query-channel denied
pause/resume, owned child stop/resume, audio and another-output apply while
paused, and no-native-player in-shell controls. They use temporary owned sleep
children to test signal semantics; installed C-player display/device acceptance
and replacement/start behavior on the full 1.0 candidate remain native gates.
See the [pause receipt](../infra/tumbleweed-lab/evidence/issue25-wallpaper-pause-20261025.toml)
and [failed/corrected evidence](../infra/tumbleweed-lab/evidence/issue25-pause-regression-20261025.log).

## New native players inherit a prior manual pause

A verified manual pause receipt now records its explicit manual choice in the
Rust broker, separately from automatic effective pause. The frozen root lease
supplies that actor-bound choice to user apply/assignment planning. It joins the
final revalidated fingerprint/private permit; public and helper inputs cannot
supply it. The native receiver checks its actual manual map again before any
wallpaper write, so a stale choice cannot silently clear or invent a pause.

When a user-driven apply starts/replaces the native player under an inherited
pause, only a real actor-owned fork/clone plus matching fixed-player EXEC stop
can enroll it. The tracer pins that process and its installed image before the
first continuation and keeps it stopped through final refreeze, detach and later
ordinary capture. A later independently authorized resume uses the existing
exact-player release path. Decoder helpers and unrelated writers do not acquire
this policy. Background automatic respawn/restore still needs #176's own permit;
no user apply permission is lent to that route.

The [player-start receipt](../infra/tumbleweed-lab/evidence/issue25-player-start-20261025.toml)
and [logs](../infra/tumbleweed-lab/evidence/issue25-player-start-20261025.log) record
a real fork/exec first-marker fixture, five exact selected library repetitions,
private-flag forgery checks and stale-choice no-write native cases. The marker
fixture uses a synthetic installed image; real C-player READY/decode/display,
installed package integration and the refreshed complete 1.0 VM/session/device
checklist remain open. No VM/download was used for these source/control proofs.

## Manual workspace assignment and unassignment

`wall.assign` and `wall.unassign`, including the existing plain `--workspace` and
`--screen` aliases, now have independent typed user intents. They cannot supply
a prepared plan, actor frame or source-state digest. The root helper resolves
the authenticated actor's Hyprland session using a fixed read-only `j/monitors all`
query. The compositor connection checks socket/account ownership, kernel peer
credentials, pinned installed image and process start; frames/replies and the
whole read have explicit bounds. It never dispatches a compositor mutation or
resumes the writer-wrapped WM proxy. Empty selection uses the focused screen;
explicit screen and future workspace selection preserve the upstream name-first
workspace identity. Missing or ambiguous session evidence refuses the request.

The provider resolves current assignment, named/shared output fallback or the
first supported static library entry from captured state. It preserves configured
mute/volume defaults and explicit source/audio choices, binds exact outputs and
workspace bytes/absence into the revalidated plan, and captures the existing
wallpaper state plus `.cache/ryogami/workspaces.json`. Only that resolved plan
becomes a single-use private `broker.workspace` frame after authorization and the
linked pre-checkpoint. The root repeats the complete plan comparison immediately before emitting the
actor frame. Focus, captured-state or source-identity changes refuse dispatch; a
second live query cannot silently replace the reviewed target. The native receiver
rechecks both state commitments before writing. It preserves unrelated assignments, performs exact legacy-name migration,
and repaints only an active target. Future workspace assignment persists its row
without repainting a different active workspace. Unassignment preserves the
upstream output/static fallback. A current manual assignment updates the native
active workspace identity so subsequent audio controls reach that assignment.

The native state replacement is atomic. Repaint failure compensates prior
workspace bytes or absence and restores active/provider state; compensation
failure reports that linked recovery is required. Root post-state validation
checks target/provider/source/audio and exact migrated-key removal before a
`workspace_wallpaper_changed` success. Direct socket/helper tests reject forged
prepared frames, preserve selector aliases and prove that intents reach the typed
helper rather than the read-only native channel. Native inherited-channel tests
cover assign/repaint/audio/unassign/fallback, inactive targets, unrelated metadata,
stale state and successful/failed compensation. Automatic workspace playback and
theme/hook effects retain their separate #176 handoff; this manual permit cannot
authorize them.

The writer regression also injects a real second-thread resume error after one
actor thread was continued. Failed begin/tick/refreeze retains its actor context.
Every admitted actor process group is pinned with a pidfd before first resume,
including descendants enrolled by real fork/clone events. Poisoned cleanup signals
only those pins and consumes terminal waits even for previously stopped threads.
Neither a transient missing `/proc` entry during exec de-threading nor a sent
SIGKILL proves death: pidfd readiness plus full stopped-target verification must
precede detach. If bounded cleanup cannot establish both, the dedicated tracer
retains the stops and retries after returning failure. Unrelated writers are
never continued by that cleanup. The regression preserves no-marker/no-surviving
actor checks and sibling-stop checks through cleanup and release.

See the [manual workspace receipt](../infra/tumbleweed-lab/evidence/issue25-workspace-manual-20261025.toml) and
[retained failed/corrected logs](../infra/tumbleweed-lab/evidence/issue25-workspace-regression-20261025.log)
for exact offline commands, counts and remaining acceptance gates.

## Loaded wallpaper roots and custom-path parity

The root inventory already validates the current on-disk `paths.cache` setting.
The native A receiver also checks its already-loaded cache directory, store
root, state root and configuration root before creating an execution permit.
The root helper derives an account-home commitment from AccountStateRoot's
NSS-bound home descriptor, not service environment. The authenticated writer
lease checks this commitment against its capture context, and the native actor
requires its HOME to match it before checking any HOME-relative roots. The
commitment is an equality check, never authorization by itself.
This covers a daemon that still holds an earlier cache/store after the file on
disk returned to the enrolled layout. A mismatch returns a specific, correlated
refusal before any wallpaper output, catalogue, selection or legacy-state write.
The helper retains its failed-operation history; the diagnostic is not success
and does not bypass actor refreeze or the recovery journal.

This is user-state integrity and restore-point accounting. It does not classify
ordinary wallpaper changes as system-level exposure. Existing custom settings,
UI and data remain intact. The current fixed inventory's alternate-location
refusal is temporary support coverage: #175/#176 must finish actual configured
root enrollment, immutable layout binding and restoration before full custom-path
parity. `FileRecord` currently rejects unlisted UserHome paths, so silently
accepting arbitrary cache roots would weaken the existing manifest/restore
contract. This #25 slice adds no such broad enrollment.

Five native receiver regressions cover loaded-cache, loaded-store, state-root,
config-root and HOME mismatch. In the HOME case every loaded root matches the
alternate HOME, while the permit remains bound to the authenticated account
root; both the custom and enrolled trees remain unchanged on refusal. They preserve sentinel bytes in existing custom
outputs/catalogue/selection files, create no enrolled wallpaper state on refusal,
and apply successfully after the fixture's loaded roots are reset to the
enrolled configuration. Root/proxy tests distinguish this refusal from success,
wrong-id/ambiguous responses and ordinary authorization failure. The [scoped
receipt](../infra/tumbleweed-lab/evidence/issue25-wallpaper-loaded-roots-20261025.toml)
records current checks and remaining native/dependency gates.

## Retained dependency routes

`broker.automatic` is a strict typed **handoff**, never a permit. Theme/stage frames
carry bounded wallpaper-frame data. Restore/workspace/reload handoffs carry the
exact action, reason, record flag and a typed stored-state source. Issue #176
must independently resolve that source, authorize its exact purpose, capture its
complete writes and run it under its own held mutation/actor contract. User
apply permission cannot authorize restore, automatic workspace playback, playlist,
Wallpaper Engine, automatic hooks or arbitrary `sh -c` commands.

The native startup/watch/post-apply registrations and shell
`applyRyogamiFrame` → `scheduleTheme`/stage integration remain in source. They
request their own handoff; this candidate reports them unavailable. It does not
claim the real theme worker or its template/reload effects protected or accepted.
#176 must supply and test that implementation before wallpaper acceptance.

| Route family | Current boundary | Owner before full acceptance |
| --- | --- | --- |
| Listed queries, `subscribe wallpaper`, `wallpaper ui` | Authenticated bounded R proxy; no reusable public native fd | #25 |
| `wall.apply` static/video; `wallpaper set` | Root-authenticated linked-checkpoint A call | #25; real theme handoff also needs #176 |
| `wall.apply` WE; source/workshop/model/download/delete; library/favorites/effect/grade/upscale writes | Retained, explicit unavailable; no query-name heuristic or privileged arbitrary destination | #175 |
| `state.set`, picker/library view state and retained provider settings | Retained; needs exact bounded state/credential contract | #175 |
| Restore/workspace playback/reload, rotation, playlist playback, startup/watchers | Typed handoff or explicit unavailable; stored selection/assignment must be authoritative | #176 (library/playlist metadata also #175) |
| Settings actions, post-process, theme/palette/stage/depth and shell hooks | Retained, own typed write/executable envelope; no automatic arbitrary shell | #176 |
| User wallpaper mute/volume (`wall.set_audio`) | Root-authenticated single-use audio action with outputs/workspace capture and post-state receipt | #25; WE audio needs #175 |
| Manual workspace assign/unassign | Root-resolved single-use action; captured-state binding, repaint/compensation and post-state checks | #25; automatic playback/theme effects remain #176 |
| User native/in-shell pause/resume | Root-authorized exact player-set permit; preserved stops and explicit resume after verified refreeze | #25; external paper #175 and automatic effects #176 |
| Lighting hardware state/control | Retained, unavailable pending the typed preference/provider contract | #180 after #25/#173/#175/#176; preserve the full lighting affordance |

The method-by-method ledger below inventories the pinned dispatchers. An
unavailable query can be admitted only after its complete read/process/cache
behavior is reviewed; read-looking spelling is not sufficient.

## Verification and precise native prerequisites

Rust focused recovery/service suites, strict Clippy and formatting pass locally.
Direct socket tests bypass UI/CLI: query followed by a malicious write, connection
reuse, separate static/video helper calls, and the independent theme handoff.
Actor tests cover unrelated sibling isolation, a late fork/allowed descendant
exec followed by refreeze, actor exec replacement and poisoned-refreeze refusal.
An intermittent EXEC/refreeze regression found that signal injection at an EXEC
stop did not reliably terminate a rejected image. The correction retains exec
validation through emergency/final refreeze and uses pidfd-bound SIGKILL plus
bounded observed death before cleanup. Five repeated outer test runs pass; each
also proves no rejected-image write marker and no surviving actor after detach.
The [retained failed and corrected run logs](../infra/tumbleweed-lab/evidence/issue25-actor-refreeze-20261025.log)
show the failure and correction explicitly. Linux's [ptrace exec/event-stop
contract](https://man7.org/linux/man-pages/man2/ptrace.2.html) explains the distinct
exec-stop and signal-delivery semantics.

Helper tests preserve Rejected wire shape and verify success fields/caller denial.
Bundle tests reject missing/ambiguous/stale/account-mismatched provenance.

The native daemon suite is **360 passing test/subtest cases, 7 declared missing
ImageMagick skips, 9 exact cross-issue exclusions**, with no blanket E2E filter.
Four initially excluded #25 fixtures were rewritten to anonymous native query/A
channels and now pass:

- `TestE2EWallpaperTopicPublishesFrameOnSet`
- `TestE2ESubscribeToUnknownTopicErrors`
- `TestE2EWallApplyPublishesTopicFrame`
- `TestE2EWallApplyWritesLegacyState`

Their fixture refuses standalone `runDaemon`, asserts no named native socket,
uses real receiver/apply handlers and exact single-use permit frames, checks
query-channel write refusal, and verifies frame/legacy state/per-output audio.
Its synthetic private permit tests native framing/algorithms only; root authority
and linked proof come from the independent Rust tests, not its dummy digest.

The remaining nine tests retain their upstream source and await these contracts:

| Exact test | Owner | Required behavior |
| --- | --- | --- |
| `TestE2EDepthSetAndClearFoldIntoFrame` | [#176](https://github.com/GenieRobot/ryoku-opensuse/issues/176) | depth/stage writer and theme handoff |
| `TestE2EJSONRequestsAndEventsShareOneConnection` | [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175) | state.set plus favorite/catalogue writes |
| `TestE2EWallpaperNextAdvances` | [#176](https://github.com/GenieRobot/ryoku-opensuse/issues/176) | automatic rotation with stored-selection binding |
| `TestE2EFavouriteSurvivesRescan` | [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175) | favorite persistence and rescan |
| `TestE2ESetFavouriteBroadcastsCached` | [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175) | favorite/catalogue mutation |
| `TestE2EWallDeleteRequiresSubfolderCatalogKey` | [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175) | recoverable local media deletion |
| `TestEventsFlowWhileRequestRuns` | [#176](https://github.com/GenieRobot/ryoku-opensuse/issues/176) | settings watcher authorization and effects |
| `TestSlowSourceSearchDoesNotBlockStatusOnSameConnection` | [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175) | source search/download-cache provider |
| `TestClosingConnectionClearsSearchRegistry` | [#175](https://github.com/GenieRobot/ryoku-opensuse/issues/175) | source provider cancellation/connection lifecycle |

The original 13 exclusions are exactly these nine plus the four rewritten cases
above. No additional test is hidden by the final filter. Reproduce from the
pinned native daemon module after the RPM prep patches, using the existing Go
1.26 SDK/cache and no network:

```sh
GOFLAGS=-buildvcs=false GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off \
  go test -mod=vendor -skip '^(TestE2EDepthSetAndClearFoldIntoFrame|TestE2EJSONRequestsAndEventsShareOneConnection|TestE2EWallpaperNextAdvances|TestE2EFavouriteSurvivesRescan|TestE2ESetFavouriteBroadcastsCached|TestE2EWallDeleteRequiresSubfolderCatalogKey|TestEventsFlowWhileRequestRuns|TestSlowSourceSearchDoesNotBlockStatusOnSameConnection|TestClosingConnectionClearsSearchRegistry)$' -count=1 -json ./...
```

The seven executed-but-skipped tool cases and the nine excluded cross-issue
prerequisites are distinct sets with no overlap. The receipt records every exact
name in both sets.

The seven tool-dependent skips are the existing grade, optimize conversion,
optimize cancellation, auto-optimize and thumbnail-generation/freshness tests,
plus `TestScanMergeMtimeChangeReprocesses`, whose real ImageMagick dependency is
now declared consistently with its neighboring thumbnail tests. No tool was
downloaded or substituted with a stub.

Exact pinned Source2/Source3 archive hashes, runtime SHA256SUMS, zero-fuzz launch
patch + appearance patch composition and byte-for-byte reproduction pass. Native
daemon and shell IPC modules compile with vendored/offline dependencies. This
checks RPM prep mechanics, not BuildRequires-enabled native RPM/install acceptance.

Full SDDM login/reboot/KDE fallback, complete core/device workflows, real
wallpaper→theme accounting, native package/ABI/portal behavior, and the combined
#26 recovery gate remain open. No VM or download was run for this slice.
