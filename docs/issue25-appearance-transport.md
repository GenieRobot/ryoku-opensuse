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
`wall.apply` or `wall.set_audio` frame/permit and EOF, returns one reply and closes. Labels alone
confer no public authority.

Public static/video apply and wallpaper audio control go to the fixed root helper with a typed action and
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

Direct socket tests cover typed audio routing/denial without a query-channel
write. Real native receiver tests exercise output plus active-workspace
persistence, named-output precedence, live frame publication, query-channel
refusal and single-use consumption. Both outputs and workspaces have symlink
and hardlink no-write/sentinel checks followed by successful valid operations;
an exclusive-temp sentinel/mode check covers the atomic primitive. These are
local protocol/state proofs, not installed audio-device or full VM acceptance.
See the [audio receipt](../infra/tumbleweed-lab/evidence/issue25-wallpaper-audio-20261025.toml).

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
| User wallpaper mute/volume (`wall.set_audio`) | Root-authenticated single-use audio action with outputs/workspace capture and post-state receipt | #25; WE audio needs #175 |
| User pause/workspace assignment controls | Retained, unavailable pending exact core appearance plan/state contract | #25 with #176 for automatic effects |
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

The native daemon suite is **336 passing test/subtest cases, 7 declared missing
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
