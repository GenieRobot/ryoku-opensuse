# Snapper coordination primitives

This is a partial implementation for [issue #19](https://github.com/GenieRobot/ryoku-opensuse/issues/19). The `ryoku-snapper` crate provides pure layout classification, validated identifiers, a bounded read-only Snapper CLI backend, separately exposed pre/post pair and root rollback capabilities, exact transaction-ID and `pre-number` pair correlation, and catalog validity/retention-pin primitives. Every native mutation capability requires a trusted coordinator to authorize it, hold the global mutation lock, freeze affected writers, and persist results before proceeding; none is dispatched by the helper.

## Behavior implemented

- `FullSystem` classifies only root and boot snapshot coverage: it requires a Btrfs root subvolume, a ready root Snapper config, and boot content that is either inside root or independently known to be snapshotted. It does not assert that a Ryoku generation exists or is linked. A separate unsnapshotted `/boot` cannot be reported as full-system protected.
- Configuration-only capability is reported when the Ryoku configuration target itself is confirmed as Btrfs with a ready Snapper config. Unsupported or unknown observations never produce full-system capability.
- Package-hook pairs match only on exact config and transaction ID. One matching pre snapshot is reported as incomplete; duplicate matching pre or post records are ambiguous. Description text and timestamps are not correlation keys.
- Pure Rust pair reconciliation reuses an exact existing package-hook pair, permits creation only when no exact pair exists, and requires explicit reconciliation for incomplete or ambiguous matches. `reconcile_pair` classifies supplied observations without querying or mutating Snapper.
- `query_pair` is a generic read-only runtime boundary: it asks a `SnapperBackend` for one config-scoped inventory, rejects foreign-config rows and duplicate snapshot IDs, then applies exact transaction correlation. It never calls either creation method.
- `SnapperCliBackend` now provides a concrete read-only list implementation. It invokes only fixed `/usr/bin/snapper` with `--jsonout --config <validated-name> list`, has no root/path override, clears the inherited environment, applies a 15-second deadline and bounded output capture, and returns sanitized error categories. It parses only the selected top-level config array and typed snapshot IDs/types. Pair correlation uses only explicit `userdata["ryoku-transaction-id"]`; descriptions are ignored. The adapter's create operations fail closed without invoking Snapper.
- `state_mount::detect_host_layout` and the read-only `ryoku-recovery layout` command inspect the live root and boot mount IDs. Root coverage is ready only when the current root's Btrfs filesystem UUID matches the validated Snapper root mapping and the native snapshot namespace reopens. Boot content is considered covered only when it shares the root mount; a distinct mount at or below `/boot` (including `/boot/efi`) is conservatively reported uncovered. The result explicitly leaves Ryoku generation linkage and root rollback untested. Configuration-only fallback and independent boot snapshot recognition remain unsupported.
- `SnapperRollbackBackend` provides a separate fixed-argument root rollback capability for an authorized recovery coordinator. It supports Snapper's classic root layout only: the selected source must be a unique read-only single snapshot, while the unique current default must be read-write. This prevents invoking Snapper's transactional rollback behavior, which changes a different snapshot and does not create the expected pair. Since the current-state checkpoint already uses the restore ID as its transaction tag, root rollback derives the separate `<restore-id>:rollback` tag. The backend rejects a reused rollback tag, calls Snapper's native `rollback --print-number` with the selected source, then checks the returned target and both newly created snapshots against a fresh inventory, including `default`, `active`, and `read-only` status. The receipt is not proof of boot compatibility or a completed boot; those observations must be independently verified before the restore journal advances. If Snapper times out or post-command verification fails, the coordinator must reconcile the rollback tag and cannot blindly retry.
- After a process exit with a durable rollback receipt, `verify_reboot_handoff` rereads Snapper inventory before the recovery CLI retries the reboot request. It requires exactly two snapshots with the rollback transaction tag: the writable default target must not yet be active, and the retained backup must be read-only and neither active nor default. This check is distinct from `verify_running_target`, which requires the target to be active after reboot. Missing, retagged, duplicated or role-changed snapshots stop the handoff.
- Catalog completeness separately requires the exact `root` Snapper config, root/boot capability, a verified system snapshot partner, and a caller-confirmed present and verified Ryoku generation partner. Duplicate transaction IDs are rejected. A `known_working` retention pin requires full-system completeness; a manually requested checkpoint may record retention intent when a transaction-bound observation verifies both partners, even if boot coverage remains unknown. That flag does not promote the entry to full-system coverage or prove that an external cleanup actor honors the pin.
- After `restore-configuration` completes a complete-linked restore, the recovery CLI reopens the exact source generation, confirms the booted rollback target, and reobserves both catalog partners before setting `known_working`. The catalog write is read back and retried on a repeated command if the journal is already complete. Configuration-only, system-only and non-full-system rows are not promoted by this path.
- `CatalogStore` persists a versioned, bounded catalog relative to an already-open directory descriptor. It requires the expected owner and exact private directory/file modes, uses a fixed filename, flock plus a process mutex, no-follow descriptor-relative reads, a SHA-256 payload checksum, and temp-file `fsync` + atomic rename + directory `fsync`. It rejects unknown schema fields, invalid identifiers, tampered/truncated documents, oversized files, and duplicate transaction IDs. The checksum detects corruption but is not a keyed authenticator; the store cannot prove the directory is outside rollback scope, and the caller must provide and protect the correct persistent location.
- `CatalogIndex::plan_retention` is a non-mutating policy planner. It requires fresh one-to-one observations for every row and keeps manual and known-working rows pinned. It also sends pinned rows to reconciliation when a partner is missing, snapshot identity is duplicated, or a `known_working` row no longer meets full-system completeness. It retains the newest requested count of eligible full-system pairs and sends other unsupported, stale, incomplete, or duplicate-snapshot rows to reconciliation. It does not delete Snapper snapshots or generations, and no native cleanup actor is wired to honor its plan yet.
- `recovery::snapper_retention::observe_retention_partners` derives those observations from a fresh Snapper inventory per catalog config, the read-only verified generation store, and the kernel-verified historical Btrfs checkpoint receipt sealed in that generation. It rejects foreign-config rows and duplicate snapshot IDs, accepts only a unique single snapshot with the exact catalog transaction tag, matching receipt and native subvolume identity as the root partner, and compares transaction, generation, config and snapshot identities. Missing or mismatched partners are reported false and cannot enter the prune set. Callers must hold the common mutation lock across catalog read, observation, planning and any later write; the returned evidence grants no cleanup authority.
- `sudo ryoku-recovery retention-plan KEEP_LATEST` exposes the observer and planner as an advisory CLI report. It holds the common mutation lock in shared mode from the catalog read through partner observation and planning, so integrated exclusive mutations cannot change those rows mid-report. It performs no cleanup; external Snapper activity does not participate in this lock, and the output is never authorization for a subsequent delete operation.
- Snapper values are typed and bounded. `SnapperBackend` describes typed list/pre/post operations; the CLI backend implements list and explicitly rejects pre/post creation.

## Remaining acceptance work

This crate does **not** complete issue #19. The following work and evidence remain:

1. Exercise the new read-only detector on supported Tumbleweed fixtures. It currently recognizes only a verified root Snapper mapping with `/boot` inside the root mount; separate snapshotted boot layouts and configuration-only fallback remain unknown/unsupported.
2. Verify the read-only CLI adapter and its output against the supported guest's Snapper version, and integrate an authorized mutation helper only after its identity and authorization are established. The adapter currently requires explicit `ryoku-transaction-id` userdata; stock zypp hook records do not prove that this key is present. `query_pair` validates the backend inventory boundary, but there is not yet an authorized create implementation.
3. Integrate `CatalogStore` with the privileged service and establish the directory's persistent mount, owner, and exclusion from every root rollback scope. Add authenticated integrity protection if the threat model requires detection of a same-owner writer who can recompute the checksum. Prove catalog and journal survival across root rollback and reject tampered or truncated state on the supported guest.
4. Recheck catalog partner existence after cleanup and rollback, and verify that the protected catalog and journal survive native rollback.
5. Integrate the rollback capability with the authenticated offline recovery coordinator, retain its receipt durably before attempting the native operation, independently verify the Btrfs default and next-boot root, and exercise timeout/interruption reconciliation in the disposable VM.
6. Integrate the pure retention plan with an authorized cleanup coordinator and verify that both partners are removed in a serialized catalog transaction; manual checkpoints and the known-working generation must stay protected from both coordinator cleanup and external Snapper cleanup.
7. Represent and demonstrate all required root/additional snapshot IDs without duplicate pairs, invalidation when either partner is missing, and configuration-only/unsupported behavior on actual layouts. This primitive catalog currently records one root snapshot ID per entry.

No VM, Snapper command, system mutation, protected-mount placement, or rollback claim was used to validate this primitive slice. Unit tests exercise descriptor-relative durable replacement in a temporary directory. Keep #19 open until its listed acceptance evidence is attached.

The recovery CLI's `manual-checkpoint` command (also available as the legacy
`checkpoint-session` alias) now marks its verified
configuration/root pair as a manual checkpoint. It still reports system
coverage as unverified. Snapper receives a single read-only snapshot without a
cleanup algorithm, and the catalog retains the explicit manual intent; this
does not prevent an administrator from deleting the snapshot or prove that a
separately configured cleanup service respects the catalog. Live cleanup
coordination remains an issue #19 acceptance item.

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
   the original receipt, and a `Planned` journal on protected storage. Its
   declared inventory includes one root-owned mode-`0600` file and one absent
   optional path; readback checks both records. It reopens both partners and
   refuses duplicate checkpoint creation. An
   interrupted or repeated capture requires explicit reconciliation; the
   probe never replaces an existing source or silently retries creation.
2. `--verify` reopens the recorded catalog/generation/native snapshot and exact
   planned journal in a fresh process. Run this again after a reboot if desired.
3. For actual root rollback evidence, the harness may perform its reviewed
   disposable-guest rollback and reboot, then run `--verify-after-root-change`.
   This requires historical lookup to pass while fresh-root ancestry
   verification fails specifically because the running root changed. The
   probe itself never rolls back root or asserts that boot files are covered.
4. `--exercise-failures` corrupts the exact generation manifest, temporarily
   makes only its recorded snapshot writable, moves it aside to test absence,
   substitutes a second probe-owned native snapshot at the recorded path, and
temporarily removes and restores the transaction-to-generation catalog link,
then moves the exact generation. Each lookup must report its typed unavailable
state. Catalog links and generations are reopened after restoration before
the probe continues. The manifest is backed up durably on the protected
   state mount at
   `/var/lib/ryoku-state/staging/native-linked-manifest-backup` before
   corruption and restored byte-for-byte before the probe continues. If that
   backup already exists, the probe refuses to proceed; an interrupted run
   requires explicit backup reconciliation: verify the root-owned backup, copy
   its bytes back to the recorded generation's `manifest.json`, compare them
   byte-for-byte, restore mode `0400`, then remove the backup and sync the
   staging directory. The manifest and backup are opened relative to pinned
   protected-state descriptors with no-symlink resolution. Do not retry the
   probe blindly. It restores the paths and
   read-only flag before final read-back and leaves both snapshots for
   inspection. This tests corrupted/missing paths, not Snapper's cleanup
   policy or deletion ownership.

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
