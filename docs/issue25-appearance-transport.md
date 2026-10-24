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
`wall.apply` frame/permit and EOF, returns one reply and closes. Labels alone
confer no public authority.

Public static/video apply goes to the fixed root helper with a typed action and
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
installed Ryoku inventory. The two new files use no-follow regular-file policy
and the existing 16 MiB per-file capture cap. Referenced media and derived
thumbnail/animation/transcode caches remain outside this persistent inventory;
no recursive cache-directory enrollment is introduced. Static input is capped
at 64 MiB and video at 8 GiB before hashing, with a ten-second hash deadline.
Oversize state/input fails before mutation with an actionable error.

## Retained dependency routes

`broker.automatic` is a strict typed **handoff**, never a permit. Theme/stage frames
carry bounded wallpaper-frame data. Restore/workspace/reload handoffs carry the
exact action, reason, record flag and a typed stored-state source. Issue #176
must independently resolve that source, authorize its exact purpose, capture its
complete writes and run it under its own held mutation/actor contract. User
apply permission cannot authorize restore, workspace assignment, playlist,
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
| User pause/audio/workspace assignment controls | Retained, unavailable pending exact core appearance plan/state contract | #25 with #176 for automatic effects |
| Lighting hardware state/control | Retained, unavailable; explicit hardware-provider ownership still required | Coordinator follow-up; not silently assigned to #25 |

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

The native daemon suite is **323 passing test/subtest cases, 7 declared missing
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
