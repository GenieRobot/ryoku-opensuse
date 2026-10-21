# Snapper coordination and protected catalog

Issue [#19](https://github.com/GenieRobot/ryoku-opensuse/issues/19) provides typed
Snapper operations, live layout discovery, and a durable catalog joining native
snapshots to sealed Ryoku generations. The final native acceptance matrix and
hashes are recorded in
[`issue19-coordination-20261021.toml`](../infra/tumbleweed-lab/evidence/issue19-coordination-20261021.toml).
The sections below dated before the final coordination acceptance describe
historical partial results; their remaining gates are superseded by that matrix.

## Current contract

- `snapper_layout::discover_layout` queries bounded native config mappings and
  inventories, then binds them to kernel Btrfs identities through no-follow
  descriptors. `FullSystem` means root and tracked `/boot` **file coverage**.
  The external ESP is explicitly excluded; this is not boot compatibility or
  restore readiness. A separate unsupported `/boot` yields configuration-only
  coverage when the configuration target is tracked, or unsupported otherwise.
- `SnapperCliBackend`, `SnapperPairBackend` and `SnapperRollbackBackend` separate
  read-only discovery from explicit mutation. Exact config/transaction IDs and
  pre-number linkage correlate existing package hook pairs. Ambiguous or
  incomplete pairs require reconciliation. Creation refuses an existing pair;
  no description/time heuristic or automatic mutation retry is used.
- `ProtectedSnapperCatalog` anchors `CatalogStore` at the validated independent
  state mount's `snapper-catalog` directory. Root-owned private metadata,
  no-follow descriptor access, locked updates, checksums and atomic synchronized
  replacement protect durable publication. Checksums detect corruption, not a
  malicious root writer able to recompute them. Catalog schema 2 records exact
  root/generation IDs plus config, snapshot ID and kernel identity for every
  additional partner. Root-only schema 1 remains readable.
- Every status lookup reopens the sealed historical generation and root receipt,
  verifies additional native identities and transaction tags, and checks current
  layout and cleanup ownership. Stored verification flags cannot establish
  completeness. Missing, deleted, substituted or writable partners invalidate
  complete status; corrupt catalog state is unavailable.
- Publication removes all linked snapshots from Snapper automatic cleanup and
  verifies the change before publishing. An interrupted publication can retain
  extra snapshots but cannot publish unprotected partners. Manual pins persist;
  known-working promotion additionally requires the exact writable rollback
  target actually running and no unfinished recovery journals. This records
  boot/storage health, not application health.
- Retention is deliberately conservative: every catalog-linked snapshot remains
  excluded from automatic cleanup. The pure retention planner preserves manual
  and known-working pins and requires reconciliation for missing/ambiguous
  partners. There is no automatic deletion actor; a future authorized pruning
  coordinator must revalidate partners and honor those pins.

These library operations require an authorized coordinator and existing mutation
serialization. They do not enable a provider write veto, helper restore dispatch,
complete linked restore, application health checks, or ESP rollback. A root
snapshot alone is not linked recovery. The package transaction in the native
fixture demonstrates stock hook correlation, not protected package writes.

## Acceptance matrix (21 October 2026)

| Issue requirement | Evidence |
| --- | --- |
| Supported root/boot-files, configuration-only and unsupported layouts | Live signed Tumbleweed fixture; separate tmpfs `/boot` and untracked target cases; ESP exclusion explicit |
| Typed operations and root/additional IDs | Native root receipt and additional `issue19-extra` config; exact kernel identities; bounded parser/argument regressions |
| Existing package hooks, no redundant pair | Real initially absent `hello` RPM installed with transaction userdata; stock pre/post pair reused and explicit creation refused without inventory change |
| Rollback-independent catalog and journal | Actual numbered-root rollback/reboot; sealed generation, both catalogs, exact synthetic Planned journal and additional receipt reopened |
| Missing/deleted/corrupt partners | Native missing root, generation and additional paths; writable/substituted root refusal; deleted additional snapshot; truncated/tampered protected catalog refusal |
| Manual/known-working and cleanup ownership | Durable manual pin across rollback; premature known-working refusal then exact booted-source promotion; native number cleanup at zero limits preserves all catalog partners while deleting a sacrificial snapshot |
| Lifecycle and isolation | Enforcing SELinux, offline byte restoration, fresh signed XFS reset, unchanged seed hash, owned disk/key disposal; no other issue guest used |

The journal is a synthetic `Planned` persistence fixture. Console lifecycle
coverage is intentionally `partial` because desktop acceptance is outside this
run. Full linked restore and general boot compatibility remain separate gates.

## Historical implementation and evidence

## Original native identity and linked historical lookup

The recovery crate now carries the original kernel identity from
`NativeRootCheckpoint::verified_link()` through generation capture into a
schema-2 manifest. The sealed receipt records the exact root Snapper config
and number, filesystem UUID, snapshot subvolume ID and UUID, and origin root
UUID. Manifest validation requires those link fields to match the manifest;
the manifest digest covers the receipt and transaction/generation identities.
Its immutable transaction catalog link uses schema 3, so older readers refuse
the new contract. Schema-1 manifests and catalog versions 1/2 remain readable,
but cannot establish historical snapshot provenance. They are never upgraded
from a later snapshot observation.

`GenerationStore::open_historical_transaction` and its reader counterpart
reopen the exact catalog link, verify the manifest and every payload, then
open only `.snapshots/<recorded-number>/snapshot` beneath the actual process
root without following symlinks. The native check matches the original
filesystem UUID, subvolume ID/UUID, origin UUID and current read-only flag.
It intentionally does not require the original parent UUID to equal the
running root's UUID after rollback. `HistoricalRootCheckpoint` is distinct
from a fresh `NativeRootCheckpoint` and cannot supply a fresh capture link.
Snapshot identity alone, including a deserialized receipt supplied outside
this protected association, is not historical provenance.

Every lookup rechecks both partners. Missing generation/catalog/payload,
corrupt linkage, absent original receipt, missing snapshot, substituted
identity/path, and writable snapshot yield typed unavailable results. There
is no newest-generation fallback, receipt repair, or replacement snapshot.
Generation files must also have the parent directory's owner and exactly one
hard link. Production callers use the state-mount-validated store/reader;
offline directory readers do not prove placement outside rollback scope.

The read-only `ryoku-recovery verify-linked TRANSACTION_ID` command exposes
this check through the fixed protected store. Success reports the exact
verified partners and explicitly leaves boot compatibility/restore readiness
unassessed; a missing or invalid partner exits with a typed error. Existing
`verify` checks generation integrity only. `inspect` now distinguishes a
recorded original receipt from a legacy absence without asserting availability.

Rust regressions cover receipt propagation through publication/read-back,
canonical seals and config/number/filesystem binding, legacy refusal,
catalog corruption/schema downgrade, missing generation, changed-root
identity comparison, substituted snapshot ID/UUID/origin and read-only
flags, and a missing partner on a second lookup. Kernel observations in the
local association tests are synthetic. These checks do not establish boot
coverage, supported layout discovery, cleanup/retention ownership, journal
survival, actual root rollback, or installed helper integration. Those
remaining issue #19 gates still require native fixture evidence.

### Disposable native receipt experiment

Build `cargo build --locked -p ryoku-recovery --example native_linked_checkpoint_probe`.
The executable refuses non-root execution and requires both the seeded
`/etc/ryoku-lab-fixture` marker and QEMU DMI. Run it only after the existing
state initializer has installed and verified the dedicated state mount.
The parent lab harness owns VM creation, root rollback/reboot, and cleanup.

1. `native_linked_checkpoint_probe --capture` creates a fixed synthetic source
   under `/run`, one transaction-tagged root checkpoint, its generation with
   the original receipt, and a `Planned` journal on protected storage. It
   reopens both partners and refuses duplicate checkpoint creation. An
   interrupted or repeated capture requires explicit reconciliation; the
   probe never replaces an existing source or silently retries creation.
2. `--verify` reopens the recorded catalog/generation/native snapshot and exact
   planned journal in a fresh process. Run this again after a reboot if desired.
3. For actual root rollback evidence, the harness may perform its reviewed
   disposable-guest rollback and reboot, then run `--verify-after-root-change`.
   This requires historical lookup to pass while fresh-root ancestry
   verification fails specifically because the running root changed. The
   explicit `--prepare-root-rollback` mode uses the typed backend and verifies
   its reboot handoff; it does not assert that boot files are covered.
4. `--exercise-failures` temporarily makes only its recorded snapshot writable,
   moves it aside to test absence, substitutes a second probe-owned native
   snapshot at the recorded path, and temporarily moves the exact generation.
   Each lookup must report its typed unavailable state. It restores the paths
   and read-only flag before final read-back and leaves both snapshots for
   inspection. An interrupted failure experiment preserves the fixture for
   reconciliation; do not retry it blindly. This tests missing paths, not
   Snapper's cleanup policy or deletion ownership.

The journal is a synthetic `Planned` persistence fixture, not a prepared
restore attempt or proof of a fresh current-state checkpoint. Capture uses a
fixed synthetic timestamp and payload. Local build, Clippy, and unprivileged
host refusal pass; native component evidence is linked below. Actual root
rollback evidence is recorded in the 21 October section below. A returned historical handle is
only an observation at lookup time: an eventual mutation executor must reopen
and compare both catalog/generation and native snapshot under its cleanup and
mutation lock immediately before use.

The Rust VM harness now accepts the paired options
`--linked-checkpoint-probe PROBE_BINARY --recovery-cli CLI_BINARY`, together
with `--console-only --state-initializer INITIALIZER_BINARY`. It stages and
hashes each input before starting, checks installed guest digests, and runs
capture, the actual CLI's `pending`/`verify-linked`, partner failure checks,
normal reboot, fresh-process read-back, and the same CLI commands again.
All exact fixture markers are validated before being accepted as evidence.
The ordinary feature retains normal-reboot behavior. The explicit `issue19-lab`
feature selects the separate rollback fixture described below.

`--observe-layout` records bounded read-only observations of the original
kernel command line, fstab, GRUB defaults/generated configuration, Snapper
configuration/version/help, Btrfs default/root/snapshot namespace and mounted
filesystems. Raw observations remain in private before/after logs; public
evidence says only `recorded` and leaves root switching and boot compatibility
`unproven`. Missing, oversized or unsuccessful observations are retained as
unavailable rather than promoted to compatibility facts. Review actual
rootflags and the separate snapshot namespace before planning any root switch.

The harness retains the synthetic Planned journal through normal reboot and
checks its exact linkage with the probe and the real `pending` command. Only
then does `--cleanup-journal` reopen both partners, lock the protected journal
directory, compare the fixed fixture record's type/owner/mode/link count and
entire parsed contents, unlink that one matching record, and synchronize the
directory. The harness requires `pending` to be empty before the initializer's
healthy-repeat check. Other or changed journals are never deleted; failures
preserve the disposable fixture for inspection. The local harness tests,
build/Clippy and probe host-refusal checks pass.

The [Run15 artifact](../infra/tumbleweed-lab/evidence/issue19-run15-linked-partners.toml)
records actual KVM execution against the signed Tumbleweed 20261013 fixture:
original receipt/catalog/generation verification, all four partner-refusal
cases, real CLI queries, normal-reboot persistence and exact owning-journal
cleanup passed. Offline fixture restoration and fresh-base reset/cleanup also
passed. Console mode deliberately reports partial overall coverage because
desktop provisioning and login were skipped. Before/after layout observations
retain the same running root identity and subvolume `@`; this is normal-reboot
persistence evidence. Root rollback, boot compatibility, retention ownership,
installed writer capture and helper/restore execution remain unproven.

### Current main integration and reused-guest checks (18 October 2026)

PR #139 is based on `adc6ec0602ecdaec45eeb1fd65e17b999bac885f`, after
prerequisite #17 was accepted. The broad launch-preflight and KVM changes have
been removed from this PR. Its retained Rust harness support only stages the
linked probe and read-only CLI, validates their exact results, and connects
those checks to the existing optional console fixture. It introduces no new
accelerator or shared launch policy. Run14/15 above describe historical builds;
their KVM launch infrastructure is not part of the current PR.

The current probe uses the exclusive `issue19-linked-probe` transaction,
`issue19-linked-generation`, `issue19-linked-journal`, and
`issue19-linked-substitute` snapshot tag. Its source is
`/run/ryoku-issue19-linked-source`. This avoids adopting another issue's retained
fixture when reusing the already running guest. Interrupted attempts still
require reconciliation; identifiers are not reused automatically.

The reused Tumbleweed 20261016 guest passed fresh capture, original sealed
receipt and payload/catalog/journal read-back, duplicate refusal, writable,
missing and substituted snapshot refusal, missing-generation refusal, and the
actual `verify-linked` CLI. The probe restored every temporarily moved path and
snapshot read-only flag. It removed only its exact synthetic Planned journal;
both pre-existing recovery journals retained their original hashes. No reboot
or root switch was performed during this check. Exact binary/log hashes,
commands, baseline failures and cleanup are recorded in
[`issue19-current-main-linked-partners-20261018.toml`](../infra/tumbleweed-lab/evidence/issue19-current-main-linked-partners-20261018.toml).

Focused host checks are `cargo test --locked --offline -p ryoku-recovery
linked_checkpoint`, the same command with `native_checkpoint`, and the same
command with `native_capture_receipt`. These cover 11 tests. The complete
workspace suite has one failure also reproduced on unchanged `adc6ec0`:
`verified_multiroot_capture_stages_only_the_shared_configuration_scope` supplies
the forbidden internal `StateGeneration` root and receives `ProtectedInternalRoot`.
The workspace suite with only that test excluded, all-feature Clippy with
warnings denied, formatting and offline dependency review pass. Configuration
capture/staging is unchanged by this PR beyond receipt propagation.

The guest's two retained #22 journals use schema 4. Both unchanged `adc6ec0`
and this PR's CLI reject them as invalid because their journal reader supports
schema 3; the guest's previously installed CLI understands schema 4. No journal
was migrated or rewritten. The #19 probe independently loaded and verified its
own schema-3 Planned journal before exact cleanup, and current native partner
verification succeeds for both the new #19 transaction and the retained
`native-linked-probe` transaction.

Issue #19 remains incomplete for its own acceptance gates: supported layout
discovery and additional snapshot IDs, actual package-hook pair correlation,
catalog/journal persistence across a changed-root rollback, and retention
ownership remain unproven. The native missing-partner checks establish current
availability invalidation only. This PR neither enables an integrated writer
nor proves complete linked restoration, boot compatibility or protected
known-working cleanup. The unrelated baseline test failure is recorded
separately from these issue #19 gaps.

## Isolated integration cleanup (18 October 2026)

The Snapper crate also exposes separate, explicitly selected mutation backends
for pre/post pairs and classic root rollback. They use fixed Snapper arguments,
exact transaction tags, fresh inventory checks, and no automatic retry after an
uncertain mutation. Rollback receipts distinguish the retained backup from the
writable default target; read-only checks separately validate reboot handoff
and the running target. Calling these APIs requires an authorized coordinator,
the common mutation lock, durable journaling, and writer coordination. This
slice does not dispatch them through the helper or recovery CLI.

Pair correlation validates the post snapshot's pre-number and rejects ambiguous
single/pair mixtures. Catalog readers do not create writer locks, and normal
writers can require an existing lock. Lock and catalog files require exact
private metadata, matching directory group, and one link; lock acquisition
checks path/descriptor identity before and after flock.

The pure retention planner requires complete fresh observations, preserves
manual and known-working pins, and sends missing or ambiguous partners for
reconciliation. A manual pin records retention intent without claiming full
system protection. No cleanup actor is wired to enforce this plan.

These APIs and their host fixtures are partial issue #19 work. Supported live
layout/additional-snapshot discovery, package-hook integration, catalog and
journal survival through actual root rollback, and native cleanup ownership
still require acceptance evidence. Existing native linked-partner evidence
remains a separate slice; this host validation does not establish boot safety.

## Changed-root persistence (21 October 2026)

The isolated `issue19-lab` console fixture selects signed Tumbleweed 20261016
and KVM, reuses the Rust migration/VM lifecycle, and changes only its fresh disk.
It initializes protected state on the original direct-child root, configures a
persistent Snapper namespace and snapshot-aware GRUB, then bootstraps a numbered
writable default root. Fresh state provisioning still requires a direct-child
root. Reopening an already initialized store validates its recorded mount,
generation store, mount unit and unfinished journals before returning; it no
longer applies creation-only root constraints to a healthy rollback root.

Run07 captured snapshot `root:5` from root `@/.snapshots/4/snapshot` (subvolume
264), then invoked the typed rollback backend and booted
`@/.snapshots/8/snapshot` (subvolume 268). The snapshot namespace stayed on
subvolume 259 and protected state stayed on `@ryoku-state` subvolume 261 with
`nosuid,nodev,noexec`. A new process reopened the original sealed snapshot,
transaction catalog, generation payload and exact synthetic Planned journal.
The actual recovery CLI verified the same partners before and after rollback.
The fixture removed only its verified Planned journal and the state initializer
then reopened the healthy store on the changed root.

Native Snapper 0.13.1 requires explicitly selected `read-only` list columns;
its default JSON omits that field. Rollback also needs global `--quiet` for
`--print-number` to return only the numeric target. Exact-argument regressions
cover both production adapter corrections. Failed runs remain failed evidence:
run03 refused changed Update metadata, run04 refused the omitted inventory
field, run05 refused a direct-root layout without a numbered default, and run06
refused fresh initialization after the numbered-root bootstrap. The separate
fixture pins the newly signature-verified Update metadata; shared pins are
unchanged.

Run07 also passed offline controlled-byte restoration, signed-base fresh-boot
reset, base-image hash comparison and disk/key disposal. Desktop acceptance
was not requested, so the generic lifecycle result remains `partial`.
Detailed hashes and command provenance are in
[`issue19-root-rollback-20261021.toml`](../infra/tumbleweed-lab/evidence/issue19-root-rollback-20261021.toml).
This demonstrates persistence of the linked catalog and synthetic Planned
journal. It does not demonstrate a complete linked restore or boot compatibility,
and at that stage did not close the separate layout/additional-ID, package-hook
and retention ownership gates. The final coordination matrix above covers them.
