# Issue 175: retained Ryogami provider implementation

This branch is stacked on the committed #25 interface at
`f5309eefa1954b492d613a172b2a583231ca61e4`. Source behavior is reviewed against
Ryoku `a59d051f0e8080052216f6c7b9eb7e5928935ce8` and the #173
[Ryogami/Rashin source review](ryoku-v1.0-ryogami-rashin-review.md).
The [source identities](../upstream/evidence/ryoku-v1.0/issue175-provider-source-identities.tsv)
retain 57 exact selected path/blob/mode rows, including upstream test siblings.
They are a dependency index, not a claim of complete transitive write coverage.

## Current implementation and limits

`ryogami_protocol` implements a closed typed intent schema for all ten retained
source identities, provider-specific search filters, source acquisition and
preview, cancellation, collections, model import/reference/removal, media
trash/restore, and library maintenance. No URL, credential, executable,
destination directory, checkpoint claim or generic argument map is accepted.
Search results must eventually resolve an account-scoped selection receipt;
a client's digest spelling is not authority. Import paths identify read-only
candidates, not permission to remove files. The converter enums match the
pinned picker (`light/balanced/quality`, `1080p/2k/4k`).

`ryogami_staging` implements real descriptor-relative local storage operations:
read-only plans; exclusive private staging files; streaming SHA-256 identity
verification; total/per-transfer byte quotas; cancellation and nonblocking
stream deadlines; no-replace publication; read-back verification; and local
trash/restore with write-ahead, checkpoint-linked receipts. Original inode,
mode and ownership survive trash/restore. An interrupted partial file remains
counted against capacity; it is not silently declared complete or discarded.
An immutable receipt digest must be retained in the protected operation
result: an account-owned receipt alone is not trusted recovery evidence.

**These modules are not yet dispatched by the installed helper or picker.**
The storage API is a trusted implementation primitive, not authorization. Its
caller must independently authenticate the peer, resolve the fixed account
root, hold the native writer lease, verify complete linked capture coverage,
revalidate the reviewed plan, publish the durable result and verify post-state.
The type-level linked proof used by the trash tests is a synthetic fixture;
those tests do not create native Snapper checkpoints.

The historical preference inventory retains its 16 MiB/file and 64 MiB total
limits and byte-identical schema-1/2 representation. The new asset inventory
adds schema 3 only when exact ordinary-account-owned entries and an original
native snapshot receipt are present. It enrolls fixed provider/model/preview/
thumbnail/Workshop namespaces, exact absence and metadata, with 4,096 entries,
8 GiB/file and 32 GiB total bounds. Capture, publication, payload verification
and restore staging stream content in bounded chunks. No parent traversal,
symlink or hardlink enrollment is accepted. `NativeCaptureSession` can capture
this set with the whole preference inventory into one sealed generation;
its inventory readback also checks all enrolled assets.

Restore staging preserves content, uid/gid, mode, mtime and supported ACL/label
metadata on a caller-owned private inode. The exact-file restore engine now
publishes regular/absent destinations under existing owned parents using an
immutable write-ahead record. It binds distinct source/current linked
generations, parent inode/owner/mode, exact target and generated staging
absence. Restart inspection distinguishes original, partially staged and
restored state. Interrupted staging is retained; cleanup and retry require
separate fresh captured operations. Local restored bytes never count as a
native operation result without complete post-capture.

The native mutation runner accepts an optional server-owned `AssetScope`,
freezes its paths/bounds through post-capture, and binds them with preference
and asset state into the reviewed fingerprint. Existing #25 providers retain
`None` and their unchanged preference fingerprint/capture. Scope changes are
rejected before native snapshot creation. Pair capacity is checked before
capture, and source metadata allocation remains capped at 64 MiB even when
asset content limits are larger.

The old preference-only planner and stage explicitly refuse asset-bearing
manifests instead of silently skipping media. Server-side image/video roots now resolve from the fixed owned settings file
and the helper-inspected actor Pictures location, preserving localized/English
fallback and livewall selection. Explicit library asset bindings preserve the
chosen owned root through capture, staging and restore, reject protected
credential namespaces and physical aliases, and leave configuration unchanged.
General directory creation and metadata recovery, complete provider write sets
and installed routing remain open. The fixed #25 inventory still refuses a
relocated `paths.cache`; that guard is unchanged until the #175 capture contract
can enroll the complete relocated catalog/state. Configured writable libraries
outside the authenticated home require separate owned-root enrollment. No current helper request has
been changed to forward an unprotected operation. No upstream component,
picker control, daemon feature, package path or language has been removed or
rewritten by this checkpoint.

## Operation and side-effect matrix

Paths below are upstream expressions to resolve and enroll, never client
permission to select a privileged write root. `<cache>` means the reviewed
Ryogami cache setting; `<images>`/`<videos>` mean configured library roots.
Every configured path still needs exact owned-path resolution in the provider.
`D` is `ryoku/shell/ryogami/daemon/`; `P` is its sibling `picker/`.

| Retained action / entrypoint | Exact native boundary and inputs | Existing effects and integration requirement | Implementation/test coverage now |
| --- | --- | --- | --- |
| Wallhaven search/collections/download | `P/src/core/remoteresults.cpp`; `D/rpc_sources.go` → `source_wallhaven.go::{wallhavenSearch,wallhavenCollections,wallhavenDownload}` | `wallhaven.cc/api/v1`; configured API key; optional collection username; image `.part` then rename into `<images>`; remote thumbnails/previews also write `<cache>` | Typed provider/filter/item/collection intents; shared stream/quota/cancel/trash primitives. HTTP/UI dispatch and complete capture remain open. |
| Unsplash search/download | `source_unsplash.go::unsplashSearch`; `source_download.go::imageDownload` | `api.unsplash.com/search/photos`, private `sources.unsplash.accessKey`; tracker request before image acquisition; library/thumbnail writes | Typed intent; tracker must have a separate remote-effect receipt because local restore cannot undo it. Production resolver and credentials remain open. |
| Pexels search/download | `source_pexels.go::pexelsSearch`; `source_download.go::imageDownload` | `api.pexels.com/v1/{search,curated}`, private `sources.pexels.apiKey`; returned image sources; bounded image stage and final library write | Typed closed filters and item; storage fixtures. Provider/credential integration open. |
| YouTube search/clip/download | `source_youtube.go::{youtubeSearch,youtubeDownload}` | `yt-dlp`, `ryoku-host pkg advice yt-dlp`; YouTube identity and thumbnails; `.part`/fragment outputs into `<videos>`; configured maximum height and clip bounds | Typed eleven-character item and finite bounded clip. Actual pinned process/group/environment/output containment and resume accounting open. |
| Bing daily acquisition | `source_bing.go::bingSearch` and image download path | `www.bing.com`; market parameter; image/thumb/cache writes | Typed bounded market and item; storage fixtures. Native resolver open. |
| MoeWalls search/download | `source_ryowalls.go::{ryowallsSearch,ryowallsDownload}` | `moewalls.com` HTML/download-token identity; original post and selected media URL; video destination and cache | Closed source identity; URLs excluded from privileged intent. Trusted resolution/redirect policy open. |
| MotionBGs search/download | same dispatch, MotionBGs parser | `motionbgs.com`; HTML IDs and media paths; video destination/cache | Closed source identity; typed selection receipt reference. Trusted resolver open. |
| Ryostore source search/download | same dispatch, `ryostoreSearch` | pinned upstream endpoint declaration `raw.githubusercontent.com/ryoku-dev/ryostore/main`; registry-relative media | Typed source identity; immutable result/manifest binding remains open. |
| Repository source search/download | `source_ryowalls.go::reposSearch` | `api.github.com/repos/<owner>/<repo>` and Git tree; raw/media GitHub content, LFS handling; optional private `GITHUB_TOKEN`; default-branch resolution | Closed repository/branch/path filters, never arbitrary URL/root. Token isolation and source/tree/blob binding remain open. |
| Source preview, thumbnail population and cancellation | `source_preview.go`, `source_thumbs.go`, `source_download.go::cancel`; picker `preview`/`cancelDownload` | `<cache>/remote-preview/<source>/<id>`, `<cache>/remote/<source>/<id>`; download cancellation maps; queued/terminal events | Typed preview/cancel; actual nonblocking timeout/cancel and cleanup fixtures. Account operation routing, UI progress and previews remain open. |
| Steam search/download | `rpc_workshop.go::{workshopSearch,workshopDownload}`; `workshop_steamworks.go`; `workshop_steamcmd.go::steamcmdDownload` | native Steam helper or `steamcmd +login <configured-user> +workshop_download_item 431960 <id> +quit`; Steam account/cache/library and manifest writes; process-group cleanup, original six-hour/stall policy | Strict numeric Steam identity/filter intent; shared storage fixtures. No Steam process/network was run. Exact helper/account/staging/manifest and resumable protocol remain open. |
| Model import/install | `settings.action` → `lens_models.go::actionImportSemanticModel` | `skwd-lens --install-pack <source> --models-dir <data>/ryogami/lens/models --runtime <runtime>`; settings model list and selected manifest; runtime/model manifest reads | Typed import candidate path, no executable/runtime/destination client fields. Pack validator, exact extraction/import writes and settings transaction open. |
| Add/forget model reference | `actionAddSemanticModel`, `actionRemoveSemanticModel` | read supplied manifest ID; update `semantic.models`; reconcile selected model in `ryogami.json` | Typed bounded preparation and private actor persistence/restore-stage fixtures now pass. Installed helper/UI dispatch remains open; selected-removal callback seam is #176. |
| Delete indexed/active model | `actionDeleteSemanticModel`, `actionDeleteActiveSemanticModel`, `deleteModelFiles` | managed `--remove-pack` with component or unmanaged `--remove-manifest`; settings selection/enablement writes | Typed active/index selection; recoverable content trash tested. Exact manifest file-set validation and complete model/config restore remain open; never pass the unmanaged arbitrary delete path through. |
| Delete wallpaper | `rpc.go::wall.delete` → `apply.go::deleteWallpaper` | resolves store entry; deletes source, catalog record, thumbnails and animated clip; Steam entries explicitly managed by Workshop | Typed catalog key; real owned-content trash/restore fixtures. Catalog-to-exact-path binding, all affected entries and UI event integration remain open. |
| Library cache rebuild / clear | `wall.cache_rebuild`, `library_actions.go::actionClearCache`, `library_videocache.go::actionClearVideoCache`, `daemon.go::resetCache` | async rescan; cache removal/recreation; catalog and thumbnail writes; converters may launch | Closed maintenance intents. Exact complete job set, native process containment and linked results remain open. |
| Recompute colors / Workshop thumbnails | `actionRecomputeColors`, `actionCaptureWeThumbnails` | catalog color fields; `skwd-wall-still`, `ffmpeg`, `magick`; temporary first frame/RGBA/WebP files; small thumbnail and catalog updates | Closed maintenance intents. Native pin/arguments/output and async completion accounting remain open. |
| Optimize images / convert videos | `library_optimize.go::actionOptimizeImages`, `library_actions.go::actionConvertVideos` | optimizer jobs replace image/video outputs; originals via `library_trash.go::trashOriginal`; manifest at `<data>/ryogami/trash/manifest.json`; ffmpeg/ImageMagick dependencies | Closed action and exact converter options; storage trash is tested separately. Full transforms, progress, job cancellation and rollback mapping remain open. |

Core apply/selection/favorite/playlist/settings/effects ownership remains #25;
automatic scan/watch/after-scan/retention/startup hooks remain #176. Explicit
library operations need coordinated invocation of those owners, not a second
unowned direct daemon path. This matrix does not transfer automatic retention
or the core appearance provider into #175.

## Local fixture evidence

The initial primitives checkpoint ran 26 focused service tests. Small deterministic bytes and local anonymous Unix sockets
exercise storage behavior. No wallpaper/model fetch, provider/network request,
Steam/yt-dlp/lens/converter process, VM, package installation or native UI test
is part of this evidence.

| Boundary | Evidence |
| --- | --- |
| All ten source identities; strict envelopes | `all_retained_providers_have_typed_downloads_and_roundtrip`, `client_cannot_inject_url_command_destination_or_checkpoint`, malformed/duplicate-field and source-mismatch tests |
| Input/privacy bounds | model-path, clip/Steam ID, repository and private-error tests |
| Successful atomic local transfer | `successful_transfer_is_private_atomic_verified_and_never_replaces`, `stream_success_is_bounded_without_large_media_or_network` |
| Oversized/truncated/wrong identity/cancel | `oversized_truncated_wrong_hash_and_cancelled_leave_no_payload` |
| Actual stalled-stream timeout/cancel | `nonblocking_stream_has_real_timeout_and_cancellation`; blocking streams refuse before staging |
| Stale plans/quotas/unsafe ownership | changed/full-directory, unsafe-entry, bad-root/quota and interrupted-partial tests |
| Trash/restore, linked receipt and metadata | `trash_and_restore_preserve_payload_and_receipt_with_distinct_checkpoint` |
| Delete/restore failure preservation | cancelled/stale delete, missing/tampered/symlink receipt, destination collision and interrupted-move observation tests |
| Missing checkpoint shape | invalid checkpoint evidence cannot construct the proof required by trash; this is not native snapshot verification |

The initial sandbox run refused a synthetic Unix-socket write with EPERM.
The separately approved offline local-socket rerun passed; that environmental
refusal is not relabelled a product failure or silently skipped.

The recovery extension adds 15 deterministic local tests (seven asset inventory,
six streamed generation/restore-stage, and two installed-capture integration
tests). A sparse zero-byte-content fixture of 16 MiB + 1 byte verifies that
large assets never require a payload-sized `Vec`. Shape-only native receipt
fixtures do not prove native Btrfs/Snapper operation.

| Recovery boundary | Evidence |
| --- | --- |
| Exact namespaces/account/absence/privacy | `ryogami_assets` tests cover fixed namespaces, wrong owner/account, traversal, duplicate paths, symlink/hardlink refusal, quotas and unrelated sentinels |
| One linked manifest | `installed_capture_publishes_preferences_and_exact_assets_in_one_native_linked_manifest` retains preference metadata, asset bytes, absence, principal and original snapshot receipt |
| Stale capture/interruption | Changed asset after capture rejects/retracts publication; interrupted streaming leaves no published transaction |
| Large verified payload | `schema3_streams_large_assets_and_verifies_both_payload_families` verifies preference and asset content while the old buffered accessor refuses oversized bytes |
| Restore staging | `streamed_restore_preserves_asset_bytes_and_metadata_without_legacy_stage` checks exact restored bytes/uid/gid/mode/mtime and preference-only staging refusal |
| Restore refusal/cancel | Wrong account, unreferenced path and nonempty target refuse; cancellation never publishes a target and interrupted staging cannot be reused |
| Legacy compatibility | Existing schema-1/2 workspace tests pass; schema-2 canonical bytes and old capture/capacity bounds are unchanged |

The first installed-capture integration fixture used an invalid all-zero native
filesystem UUID; correcting the synthetic receipt made the intended integration
pass. Strict lint found a complex inode fingerprint tuple; replacing it with
named fields and fixing its size access passed the subsequent full check.

The exact-file restore/scope checkpoint adds eight tests (two reviewed-scope
fingerprint/bounds tests and six journaled destination tests). The focused
`cargo test --offline --locked -p ryoku-recovery asset_restore` selection passes
7/7 because it also includes the prior streamed restore refusal test.

| Exact destination boundary | Evidence |
| --- | --- |
| Atomic restoration and persisted identity | `exact_restore_journals_before_replace_preserves_source_and_reopens_after_restart` verifies restored bytes, stage removal, protected-record reopen, original linked partner and unrelated sentinel |
| Interrupted stage cleanup and retry | `partial_stage_is_truthful_and_cleanup_and_restore_require_fresh_captured_operations` retains the partial, observes it after reopen, refuses blind replay, captures a fresh cleanup operation and then another fresh restore operation |
| Pre-write refusal | Missing target/stage coverage, wrong account, changed parent identity/mode, stale target and oversized/symlink stage injection leave the journal and destinations unchanged |
| Crash windows | `journal_only_and_post_rename_interruptions_have_distinct_restart_observations` reports Original versus Restored without falsely marking native completion |
| Record corruption | Missing/tampered record or exchanged source/checkpoint cannot reopen a valid restore plan |
| Scope identity | Reviewed paths, bounds, preference fingerprint, asset bytes and absence all participate; duplicate/malformed scopes and oversized current state refuse |

All fixtures remain local synthetic data. Native provider authorization,
Snapper execution, general tree recovery and installed picker actions have not
been exercised by these storage tests.

The configured-root checkpoint adds seven local tests: four resolver/privacy
cases, two recovery binding/legacy-encoding cases, and one configured-library
live restore/stage/reopen case. The resolver does not create directories, scan
file contents, contact providers, or return credentials. Root construction is
server-side; provider requests still cannot supply a privileged destination.
Exact item/catalog membership validation remains the provider's responsibility.

| Configured-root boundary | Evidence |
| --- | --- |
| Upstream location behavior | English default, localized Pictures, English fallback, `livewalls`, configured image/video folders and custom hidden media folders resolve without writes |
| Ownership/privacy | Foreign account, group/world-write path, symlink, external/traversal/credential root and malformed settings refuse; config/credential/unrelated sentinels remain unchanged |
| Exact capture and restore | Configured root survives generated staging derivation, linked capture, restored bytes and protected-record restart inspection |
| Historical schema-3 encoding | Fixed namespaces still serialize without a new root field; root injection and two logical names for one physical path refuse |
| Cache prerequisite | Nonstandard cache is identified but receives no exception to #25's committed inventory validator |

## Model reference actor checkpoint

The closed add/forget preparation preserves existing typed model metadata and
selection. The actor accepts a helper-prepared list only on its existing private
one-use channel; ordinary settings actions and prepared frames on query channels
remain refused. It synchronously checks the original configuration digest,
writes an exact exclusive account-owned stage, preserves mode/ACL/SELinux
metadata and unrelated JSON values (including large integers), fsyncs, renames,
fsyncs both parents and reads the result back. The fixed #25 inventory and loaded
cache checks remain unchanged. Missing configuration/parents and pending settings
refuse before mutation; initial file/directory creation remains an acceptance gate.

The only additional asset namespace admits `.ryoku-model-<sha256>.part` directly
under the authenticated home. It admits no general home path. Linked capture
records the exact stage's absence or contents, and the journaled restore fixture
captures a fresh operation before cleaning an interrupted stage. This does not
yet provide combined preference-plus-asset live restore or public operation dispatch.

The native shim preserves `semantic.models` event then callback ordering, after
synchronous persistence. It never edits `semantic.manifest` or `semantic.enabled`.
Upstream `reconcileSelectedModel` clears selection when no remaining list entry
matches it; that includes an already-orphaned selection even when forgetting an
inactive row. Those removals refuse during preparation with
`SelectionCallbackRequired`. Duplicate selected references remain supportable.
The exact #176 dependency is an authenticated, linked transaction seam for
`semantic.manifest` changes: retain stop-service and reindex callback ordering,
enroll every model/index/state side effect, await its durable outcome before
postcapture, and preserve failure/restart evidence. No callback owner files were
modified and no missing callback effect is represented as success.

| Model boundary | Local evidence |
| --- | --- |
| Strict preparation | Three Rust tests cover blank/named add, inactive/duplicate forget, selected/orphan callback refusal, stale index, capacity, malformed legacy metadata and credential omission |
| Private transport | Three Rust protocol tests bind frame/home/actor and reject public commits, player pins and manual-pause authority; eight existing wallpaper protocol tests still pass |
| Native persistence | Six pinned-source Go tests cover atomic readback, large-number/credential/mode preservation, stale/unsafe/colliding/oversized refusal, event/callback order, pending settings, private actor/query separation and exact ACL readback |
| Interrupted stage | Go failure retains the stage and leaves target bytes intact; Rust captures absence, restores cleanup under a fresh linked journal and reopens the receipt without an unrelated home scan |
| Wallpaper regression | Two existing pinned Go broker-audio tests pass, including existing external/link refusal cases |
| Reproducible integration | Minimal daemon shim patch applies after #18/#25 with zero fuzz and reproduces tested Go files exactly; RPM preparation includes the patch |

The full Rust workspace passes (recovery 275/6 existing ignored, service 173/1
existing ignored, VM 90), formatting/strict lints/cached dependency checks pass.
An initial broad upstream Go baseline run hit the sandbox's local HTTP fixture
listen restriction (and unavailable ImageMagick); it is not an acceptance result.
The focused Go fixture run uses only local files and anonymous Unix sockets with
vendored dependencies and passes eight top-level tests. No provider endpoint,
wallpaper/model download, Steam/lens process, RPM installation or native desktop
acceptance was exercised.

## Combined preference/asset restore checkpoint

The private combined plan now binds every fixed preference and exact regular or
absent asset to one immutable source and a distinct current-state native-linked
generation. Its asset scope includes every generated restore stage before that
current-state capture. Asset/preference coverage, account identity, snapshot
identity, compatibility, parent identities and original target state are checked
before preparation. All three legacy restore scopes still refuse asset-bearing
manifests. The bounded preference subset is never exposed as a standalone stage.

The coordinator persists a combined record containing the source plan,
current-state generation/digest, principal and each exact asset write-ahead
receipt. It retains the existing writer-stop/root-rollback phase gates. After
streamed asset restoration it applies the fixed preferences with their final
journal transition deferred; only complete readback of both families advances
`ConfigurationRestored`. This phase is not native health, startup admission or
operation success. Reopen reads the protected combined and per-asset records,
revalidates their immutable generation bindings and verifies completed items.
An unchanged original target with no stage may resume its exact recorded plan;
a partial stage requires a separately authorized fresh-checkpoint cleanup first.

| Combined boundary | Deterministic local evidence |
| --- | --- |
| Preparation | Complete fixed preference plus all target/stage coverage; missing preference or stage, wrong account, same/missing partner and stale live asset refuse without writes |
| Real two-family application | Actual `ryogami.json` and model bytes restore, private configuration values and unrelated sentinel survive, and protected restart readback succeeds |
| Either-family failure | Changed model stops before preference writes; unsafe preference parent after asset restoration leaves `WritersStopped`, and correcting it permits verified-item reuse and complete preference restore |
| Partial asset | Interruption retains the stage and unchanged preferences; reopen cannot blindly replay it; a fresh captured cleanup removes only that stage before the original exact plan resumes |
| Post-asset interruption | Restored model plus unchanged configuration remains `WritersStopped`; reopen verifies the completed asset and finishes preferences; corrupted combined record cannot reopen |
| Legacy behavior | Existing configuration apply/stage/restore tests run in the full workspace; old APIs still reject schema-3 asset manifests rather than omit assets |

The five combined fixtures and full offline workspace pass: recovery 280/6
existing ignored, service 173/1 existing ignored, VM 90. Formatting, strict
all-target/all-feature Clippy and cached dependency review pass. Initial test-only
fixtures were corrected to use a mutation-ready checkpoint and an existing fixed
preference path; a private test result alias was corrected without widening the
API. This is local storage/coordinator evidence, including configuration-only
application; no native root rollback, installed recovery-command dispatch,
provider helper, UI, directory-tree restore or selected-model callback acceptance
is claimed. Recovery of incomplete preparation before a combined record exists
still needs its installed coordinator path before actions can be enabled.

Configured user-space wallpaper locations are legitimate retained functionality.
The current unexposed resolver's path refusals describe incomplete exact-root
capture/recovery enrollment, not a security finding against wallpaper paths or
a decision to remove support. Final parity includes configured caches, external
account-owned libraries and valid custom media locations. Their implementation
must bind exact owned media/catalog entries without recursively capturing mixed
configuration credentials, accepting arbitrary delete targets or forwarding
reload commands. Any migration must preserve the retained feature and its data.

## Combined console dispatch and interrupted cleanup checkpoint

Asset-bearing generations now route through the installed offline recovery
console's existing root authorization, verified state mount, exclusive mutation
lock, account resolution, writer stop and native-partner checks. Preparation
captures fixed preferences plus the exact asset/restore-stage union using the
installed profile bundle. Its checkpoint transaction must equal the restore ID.
The legacy schema-1/2 console path and #25 fixed inventory remain unchanged.

`prepare-configuration` / `prepare-linked` durably resume each preparation
boundary. Complete private stages and records are verified and reused; partial
artifacts require explicit `discard-incomplete-stage`. Cleanup refuses unknown
entries, links, unrelated bytes, complete records and later phases. It can remove
only a known incomplete preference stage or an exact canonical prefix of the
expected private combined record; live settings and assets remain intact.

After interrupted asset application, that same discard command derives a typed
cleanup intent from protected plans and observed partial stages. It captures a
fresh native-linked generation containing those exact stages and the derived
cleanup-stage absences, catalogs it and marks it as an operator-retained
checkpoint before deletion. A protected cleanup record binds both operation IDs,
source/current digests, account and exact receipts. Cleanup does not advance the
original restore phase; `restore-configuration` reopens the original protected
plans and verifies completed assets before finishing the preferences. A failure
retains the partials and/or the pinned checkpoint; it never reports full restore
success. Each retry derives a new cleanup operation for remaining partials.

`rollback-linked` uses the existing journaled native handoff; its receipt alone
cannot authorize configuration or asset application. Independent boot proof is
still required. `restore-configuration` withholds `ConfigurationRestored` until
both families verify, then retains the existing health and known-working gates.
No source/model helper or picker action is enabled by this checkpoint.

| Console/recovery boundary | Deterministic local evidence |
| --- | --- |
| Interrupted preparation | Five durable boundaries resume without changing live preferences/model bytes |
| Private incomplete cleanup | Known incomplete stage/prefix cleans explicitly; unknown files/bytes, complete records and post-stage phases refuse |
| Typed live cleanup | Exact partial-stage scope excludes the model target; wrong source/checkpoint and cancellation refuse; protected receipt omits private configuration values; resumed main restore completes |
| Native handoff | Fake backend verifies exact root/source/rollback tag; writer failure makes no call; native failure remains pending; success persists receipt/guard but cannot apply before boot proof or repeat rollback |
| Console regression | All 26 console tests pass, including six malformed-ID/path-override refusal fixtures |

The eight combined fixtures and full offline all-feature workspace pass:
recovery 283/6 existing ignored, service 173/1 existing ignored, VM 90.
Formatting, strict all-target/all-feature Clippy, whitespace and cached offline
dependency review pass. One needless-borrow lint was corrected before the full
successful run. The sanitized receipt is
[issue175-local-console-cleanup-20261025.toml](../upstream/evidence/ryoku-v1.0/issue175-local-console-cleanup-20261025.toml).
These tests use local synthetic bytes and a fake native backend. The installed
console branches compile, but actual Snapper/Btrfs rollback, RPM integration and
native desktop acceptance still require the combined native candidate.

## Remaining acceptance gates

1. Install asset-aware complete linked pre/post capture and offline restore,
   preserving fixed-inventory #25 checks and unrelated home files. The durable
   protected history must bind each receipt, exact paths, identities and remote
   requests. Recover and report all crash windows, including fsync uncertainty.
2. Implement authenticated helper dispatch, account-root construction,
   authoritative search/credential/source resolution, bounded staging and
   pinned process providers. No request may choose a privileged executable,
   URL, output root or arbitrary delete target. Resume needs its own reviewed
   partial identity/offset and quota policy; a `.part` filename is not resume
   authorization.
3. Patch only retained upstream integration boundaries to use this provider;
   preserve picker filters, progress, cancel, model/library operations and
   daemon/CLI paths. The storage primitives above do not make any control work.
4. Exercise every operation via the installed broker/helper with deterministic
   local fixtures, including denied authorization, capture/planning failure,
   stale plans, absent/corrupt linked partners, interrupted/oversized work,
   unrelated-file and credential sentinels, progress and terminal results.
5. Run exact-head hosted formatting/strict lint/workspace/dependency checks.
   Record fresh native provider/RPM/UI evidence under #26 using the combined
   #173/#25/#175/#176 candidate. No native acceptance or #175 closure is claimed.
