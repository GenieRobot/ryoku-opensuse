# Snapper coordination primitives

This is a partial implementation for [issue #19](https://github.com/GenieRobot/ryoku-opensuse/issues/19). The `ryoku-snapper` crate provides pure layout classification, validated identifiers, a bounded read-only Snapper CLI backend, exact transaction-ID pre/post pair correlation, and in-memory catalog validity and retention-pin primitives.

## Behavior implemented

- `FullSystem` classifies only root and boot snapshot coverage: it requires a Btrfs root subvolume, a ready root Snapper config, and boot content that is either inside root or independently known to be snapshotted. It does not assert that a Ryoku generation exists or is linked. A separate unsnapshotted `/boot` cannot be reported as full-system protected.
- Configuration-only capability is reported when the Ryoku configuration target itself is confirmed as Btrfs with a ready Snapper config. Unsupported or unknown observations never produce full-system capability.
- Package-hook pairs match only on exact config and transaction ID. One matching pre snapshot is reported as incomplete; duplicate matching pre or post records are ambiguous. Description text and timestamps are not correlation keys.
- Pure Rust pair reconciliation reuses an exact existing package-hook pair, permits creation only when no exact pair exists, and requires explicit reconciliation for incomplete or ambiguous matches. `reconcile_pair` classifies supplied observations without querying or mutating Snapper.
- `query_pair` is a generic read-only runtime boundary: it asks a `SnapperBackend` for one config-scoped inventory, rejects foreign-config rows and duplicate snapshot IDs, then applies exact transaction correlation. It never calls either creation method.
- `SnapperCliBackend` now provides a concrete read-only list implementation. It invokes only fixed `/usr/bin/snapper` with `--jsonout --config <validated-name> list`, has no root/path override, clears the inherited environment, applies a 15-second deadline and bounded output capture, and returns sanitized error categories. It parses only the selected top-level config array and typed snapshot IDs/types. Pair correlation uses only explicit `userdata["ryoku-transaction-id"]`; descriptions are ignored. The adapter's create operations fail closed without invoking Snapper.
- Catalog completeness separately requires root/boot capability, a verified system snapshot partner, and a caller-confirmed present and verified Ryoku generation partner. Duplicate transaction IDs are rejected. Manual and known-working retention pins can only be set on complete entries.
- `CatalogStore` persists a versioned, bounded catalog relative to an already-open directory descriptor. It requires the expected owner and exact private directory/file modes, uses a fixed filename, flock plus a process mutex, no-follow descriptor-relative reads, a SHA-256 payload checksum, and temp-file `fsync` + atomic rename + directory `fsync`. It rejects unknown schema fields, invalid identifiers, tampered/truncated documents, oversized files, and duplicate transaction IDs. The checksum detects corruption but is not a keyed authenticator; the store cannot prove the directory is outside rollback scope, and the caller must provide and protect the correct persistent location.
- Snapper values are typed and bounded. `SnapperBackend` describes typed list/pre/post operations; the CLI backend implements list and explicitly rejects pre/post creation.

## Remaining acceptance work

This crate does **not** complete issue #19. The following work and evidence remain:

1. Implement and review a read-only detector for supported openSUSE Btrfs, Snapper config, mount, and boot layouts; confirm every observation against real supported Tumbleweed fixtures. This crate currently classifies supplied observations only.
2. Verify the read-only CLI adapter and its output against the supported guest's Snapper version, and integrate an authorized mutation helper only after its identity and authorization are established. The adapter currently requires explicit `ryoku-transaction-id` userdata; stock zypp hook records do not prove that this key is present. `query_pair` validates the backend inventory boundary, but there is not yet an authorized create implementation.
3. Integrate `CatalogStore` with the privileged service and establish the directory's persistent mount, owner, and exclusion from every root rollback scope. Add authenticated integrity protection if the threat model requires detection of a same-owner writer who can recompute the checksum. Prove catalog and journal survival across root rollback and reject tampered or truncated state on the supported guest.
4. Connect catalog entries to #18's generation store and verify both partners before claiming complete status; recheck partner existence after cleanup and rollback.
5. Implement and test retention ownership so manual checkpoints and the known-working generation are protected and Snapper cleanup cannot orphan catalog links.
6. Represent and demonstrate all required root/additional snapshot IDs without duplicate pairs, invalidation when either partner is missing, and configuration-only/unsupported behavior on actual layouts. This primitive catalog currently records one root snapshot ID per entry.

No VM, Snapper command, system mutation, protected-mount placement, or rollback claim was used to validate this primitive slice. Unit tests exercise descriptor-relative durable replacement in a temporary directory. Keep #19 open until its listed acceptance evidence is attached.

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
   probe itself never rolls back root or asserts that boot files are covered.
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
rollback remains unproven. A returned historical handle is
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
This path has no root-switch command or boot configuration changes.

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
