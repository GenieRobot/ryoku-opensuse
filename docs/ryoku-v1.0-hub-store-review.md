# Hub and store: bounded 1.0 source disposition

Issue #173 review against upstream commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. Hub tree:
`ab3da2d33a1dcfb0655caa18b4e088b5a6464c4d`; Ryostore tree:
`1de749ea35a0f57fb0f187f00e720907a4740a1c`.
These are current-tree inventories, not the changed-path counts in the
[delta matrix](ryoku-v1.0-delta.md).

## Exact selection and installation

[`hub-store-disposition.tsv`](../upstream/evidence/ryoku-v1.0/hub-store-disposition.tsv)
records all 728 current files, original Git blob/mode, Source2 destinations,
Source3 selection and disposition. Selection was checked against
`runtime-source-manifest.tsv` and `writer-source-manifest.tsv` in that same
evidence directory, then against `packaging/rpm/ryoku-desktop.spec`.

| Current source files | Count | Result and reason |
|---|---:|---|
| `ryoku/hub/quickshell/**` | 132 | Both exports select them; the RPM copies the patched Source3 tree to its Hub payload. Source2's duplicate Hub QML is not the installed copy. |
| `ryoku/hub/backend/**` | 494 | Source3 build inputs, including tests, embedded assets and Go vendor sources. Build the private `ryoku-hub` binary; do not install the source tree. Dependency notices are collected separately. |
| `ryoku/hub/{README.md,CHANGELOG.md,ryoku-hub.desktop}` | 3 | Excluded from both exports and install recipe. No new standalone Hub desktop entry is introduced. |
| `ryoku/apps/ryostore/**` | 99 | Entire application excluded: no backend, QML, executable, desktop entry or service is exported/built/installed. No store action is enabled by this sync. |

This file selection does not grant permission to execute every embedded Hub
function. Public `ryoku-hub` resolves to Rust `ryoku-writer`, whose exact-arity
table in `suse/recovery/src/writer_command_policy.rs` admits reviewed queries
and refuses other routes before child/setup activity. Existing patch/negative
tests remain the evidence for that boundary; no builds were rerun for this
documentation-only review.

## API and write-route findings

Hub `backend/apps.go` returns an array of application roles and candidates
(`role`, `label`, `fallback`, `combo`, `candidates` with `label`, `cmd`,
`installed`). It reads binds and probes binaries with `exec.LookPath`; it is
not Ryostore's catalog or an install provider. `backend/main.go` dispatches
the Hub families already enumerated in the source review; no Ryostore
install/remove dispatcher is present there.

Ryostore `backend/model.go` instead defines a catalog with `generatedAt`,
`revision`, `windowManager`, `offline`, `categories` and `items`. Items include
manifest/hash/version/compatibility, installed/active/enabled flags, bundle
counts, required package managers and unavailable reasons. In
`backend/progress.go`, progress is textual
`RYOSTORE-PROGRESS <download|extract|verify|apply> <done> <total>`, throttled
at 100 ms. It has no request or durable operation identity. The port service
uses versioned request/response envelopes, request IDs, operation IDs and
journaled operation status (`suse/service/src/lib.rs`, `runtime.rs`). These
contracts cannot be substituted for each other; there is no reviewed store
adapter or supported install/remove bridge in this candidate.

| Excluded entrypoint | Concrete side effects / dependencies |
|---|---|
| `backend/main.go` `catalog` / `warm` | Catalog can build live providers, write `catalog.json` and the seen revision; warm fetches previews/screenshots and rewrites cached asset URLs. Even catalog is not a pure read query. |
| `backend/paths.go`, `cache.go`, `extras_assets.go` | Default source is `https://raw.githubusercontent.com/ryoku-dev/ryostore/main`; `RYOSTORE_BASE`, legacy `RYOKU_EXTRAS_BASE` and `$XDG_CONFIG_HOME/ryoku/ryostore-base` override it. Network registries/assets and local `file://` trees feed `$XDG_CACHE_HOME/ryoku/ryostore`, with hashed per-source subdirectories for overrides. This mutable remote catalog is not a pinned package input. |
| `backend/product_transaction.go`, `receipt.go` | HTTP product fetches, staging/backup/removal directories beside product destinations, file writes/renames and transaction locks/journals. Receipts live at `$XDG_STATE_HOME/ryoku/store/<category>/<id>.json`, plus `revision.json`, `transactions/` and `transactions.lock`. Receipt hashes/file ownership do not constitute the port's recovery authorization. |
| `backend/provider_plugins.go` / `host_package_manager.go` | Reads `$XDG_CONFIG_HOME/ryoku/plugins.json`, product manifests and `ryoku-host pkgmgr`; upstream product placement/package-manager compatibility needs a separately reviewed openSUSE provider. |
| `backend/main.go` `internal` | Includes installer and guest install/remove entrypoints. Excluding the entire app avoids exposing these through a catalog-only assumption. |

Retained Hub `Singletons/Host.qml` probes `ryoku-host capabilities` and defaults
capabilities to unsupported. This package does not install that upstream host
helper; its JSON (`packageManager`, `init`, `snapshots`, `aur`, `users`) is not
the port's capability protocol. Direct Hub host/package/service launch sites
are covered by the existing refusal patch. The retained `backend/rashin.go`
query checks only binary availability; its on/off paths would install/remove
`ryoku-rashin`, call enable/disable, re-author desktop configuration and append
`$XDG_STATE_HOME/ryoku/provisioned`. Only `rashin status` is admitted by Rust.

## Provenance, dependencies and remaining gates

Root upstream GPL/NOTICE provenance, the exact iNiR attribution and retained
asset exceptions are recorded in `packaging/licenses/ryoku-v1.0/README.md`.
Hub's embedded Redux `assets/redux/{adapter.py,setup.py,LICENSE,UPSTREAM.md}`
remains immutable MIT-attributed source; its setup/model mutation routes are
refused. Backend vendor notices are collected by the desktop spec. Excluded
Ryostore source and remotely described products do not add an installed RPM
license claim; their per-product licenses must be checked if a future provider
ever imports them.

Retained Hub requires its compiled Go backend, coordinated Quickshell and
the selected QML import closure, including UPower. The global import inventory
and signed RPM evidence remain authoritative. Ryostore-only QtMultimedia and
Controls imports do not justify installing the excluded application. Native
module/import/ELF checks, UI behavior and complete license closure are pending;
the current storage gate permits neither builds nor guest access. The full
#173 delta, #25 session checks and #26 recovery acceptance remain open. This
review completes only the file disposition and the contract distinction for
Hub/store, not an overall #173 acceptance item.
