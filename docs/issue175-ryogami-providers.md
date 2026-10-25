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

The inherited inventory explicitly omits referenced media/model payloads and
limits generation files to 16 MiB, 64 MiB total. It cannot be relabelled as
complete coverage for videos, Steam trees or model packs. Before activation,
#175 needs a separately verified asset receipt/content capture and restore
extension, exact source-to-destination ownership, and account-private remote
request accounting. No current helper request has been changed to forward an
unprotected operation. No upstream component, picker control, daemon feature,
package path or language has been removed or rewritten by this checkpoint.

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
| Add/forget model reference | `actionAddSemanticModel`, `actionRemoveSemanticModel` | read supplied manifest ID; update `semantic.models`; reconcile selected model in `ryogami.json` | Typed optional read-only path and bounded index; persistent settings plan remains open. |
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

Focused `cargo test --offline --locked -p ryoku-service ryogami_` currently
contains 26 tests. Small deterministic bytes and local anonymous Unix sockets
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
