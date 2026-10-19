# Durable mutation coordination

This is component evidence for [issue #20](https://github.com/GenieRobot/ryoku-opensuse/issues/20).
`PersonalProfileCoordinator::execute` now composes the actual scoped native
checkpoint and personal-file writer with durable history under the existing
`MutationLock`. It is a root-authorized building block with no installed
executor. No supported mutation is enabled. Helper requests still return
structured rejections and the recovery console remains read-only. This does
not establish an installed account-writer handoff, full capture completeness,
or multi-subvolume atomicity. The older `mutation_gate::coordinate_mutation`
remains a separate generic trusted-integration interface.

## Concrete personal-profile call chain

The caller obtains the coordinator from a kernel-authenticated operator peer,
acquires its writer lock once, and prepares/reviews one private
`PersonalProfilePlan` under that guard. `execute` borrows the same guard; it
does not recursively acquire the lock or accept checkpoint booleans. Only its
actual native capture path can construct `VerifiedPersonalProfileCheckpoint`.

The exact original file record, account principal, immutable default bundle
identity and desired canonical bytes determine the reviewed plan fingerprint.
The declared personal inventory and source metadata determine the affected-state
fingerprint. History contains those digests, not the personal settings. Native
transaction and generation identities are deterministically assigned from the
operation ID; the durable duplicate-ID gate prevents repeating an action.

The machine journal is fixed at `/var/lib/ryoku-state/mutation-journal.jsonl`.
It is a root:root, single-link, `0600` regular file in the validated `0700`
state mount. Opening and torn-tail replay borrow the held writer guard. The
lock directory must match that state mount. Before every replay or append,
the journal rechecks the independent machine record, live state mount,
directory ownership/mode, and the journal and held lock's ownership/mode/link
count and named inode. It pins the existing lock description without a second flock.
A replacement or unavailable mount fails without appending through a detached
descriptor. This is the shared namespace future helper/console writers must
use, separate from the per-user status journal.

Preflight repeats the actual source/account/RPM checks and checks available
blocks on protected state, root and the account filesystem. Protected state
and root each require a 4 MiB floor plus 64 allocation blocks; account staging
requires the desired byte length plus eight blocks. These conservative estimates
are not reservations and cannot guarantee subsequent Btrfs metadata allocation.
A detected shortage returns `InsufficientSpace` before capture or apply; the
durable row is Preflight/FailedBeforeChange with `recovery_unavailable`.

After native preimage capture and exact revalidation, Applying is synchronized
before entering the private writer. Health checking compares its actual returned
file record with the desired digest/size, account ownership, required mode and
preserved ACL/SELinux metadata. First creation additionally repeats the native
final-path SELinux policy check. Health checks also reopen the preimage's native
receipt and immutable bundle and recheck the account and actual RPM defaults.

PostChangeCapture uses the exact health-checked file record as its source plan.
The newly sealed manifest names the preimage generation as its parent. Both
native partners and the live postimage are reopened before Complete, with fresh
transaction/generation/digest/snapshot identities and the same config/filesystem.
The result retains both opaque scoped proofs. Cancellation after Applying never
skips health or this post-change capture. Any failure at either stage preserves
the existing conservative recovery-required classification.

This API coordinates cooperating Ryoku writers only. External editors, RPM and
Snapper/config writers are not excluded; repeated observations reject detected
drift but do not freeze them. No dispatcher calls this API, and the installed
helper's `ProtectHome=read-only` plus empty capability set remains unchanged.
It cannot read/write private account state as this executor requires. Native
execution needs a separately reviewed account-writer handoff or scoped worker.

## State machine and exact association

The coordinator validates a bounded plan before acceptance, acquires the existing
interprocess lock, and reloads the shared operation journal while holding that
lock. An unresolved operation prevents acceptance of new work. A duplicate
operation ID cannot restart an action. The caller must supply the same protected
journal and lock directory for every cooperating entry point, and must open or
repair shared journal storage only while holding the coordinator lock. A per-user
status journal is not the protected coordinator journal.

Each row is synchronized before its integration hook starts. The socket retains
its protocol-v1 coarse status; journal schema 2 adds the detailed context.

| Detailed stage | Socket phase | Work after durable record |
| --- | --- | --- |
| Preflight | accepted | Recheck reviewed plan authorization, persistent state and space |
| Capturing | capturing | Capture, publish and independently read back every required component |
| Revalidating | capturing | Recheck the exact plan, covered paths and current native checkpoint |
| Applying | applying | Enter the provider mutation |
| HealthChecking | verifying | Check the resulting system/provider state |
| PostChangeCapture | verifying | Capture and verify the required post-change state |
| Complete | succeeded | Persist the verified post-change checkpoint reference and return the result |

Each integration hook may report bounded progress while it runs. A report is
appended and synchronized with the unchanged plan/checkpoint context before the
report call returns; the service can then return that exact durable progress to
a reconnecting client. Progress starts fresh for each detailed stage, may only
advance, and cannot clear a known total. A hook must propagate a progress-journal
error and stop its work; it cannot continue on status it could not persist.

Every record binds the same operation ID, action, bounded operation key and two
canonical SHA-256 fingerprints: the reviewed plan and affected-state inventory.
Before revalidation, the coordinator also persists the exact transaction ID,
generation ID, manifest SHA-256 and Snapper config/snapshot/filesystem link.
`LinkedGeneration::from_verified_generation` copies that reference from a
persistently verified generation. A sealed manifest digest also binds any native
snapshot receipt it contains. The integration must reopen and match that exact
generation/digest and independently revalidate its native receipt before apply.
An ID or saved successful phase is not proof that a snapshot remains available.

The generic post-change hook returns `VerifiedPostChangeGeneration`, explicitly bound
to that exact preimage. Its reference must have fresh transaction/generation IDs,
manifest digest and native snapshot number for the same Snapper configuration
and filesystem. Reusing any preimage identity, missing required verification or
binding the result to another preimage fails in the PostChangeCapture stage.
The Complete record requires this post-change reference alongside the retained
pre-change reference; replay rejects a missing/reused postimage or subsequent
terminal rewrites. Both verification wrappers remain trusted integration
attestations rather than independent native proof.

The plan, operation/action association and checkpoint reference cannot change
across later phases or replay. Replay rejects missing checkpoint context after
capture, changed associations, skipped stage transitions, invalid fingerprint or
snapshot shapes, terminal transitions and inconsistent failure classifications.
Within an active stage, it accepts only valid monotonic progress records; these
do not count as repeated stage transitions.
Every line is read and written through a 16 KiB bound; operation keys are at most
256 printable ASCII bytes and snapshot numbers are canonical nonzero `u64`s.

`MutationIntegration` requires explicit preflight, capture, revalidation, apply,
health and post-change hooks. The caller authenticates and authorizes the peer
before entering this coordinator. It must freeze every affected writer across
capture, revalidation, apply and accounting. The advisory lock coordinates
cooperating processes; it cannot stop independent editors or provider writers.
The older reduced callback wrapper is available only in unit-test builds.

## Failure, cancellation and restart

Failures preserve the detailed stage where work stopped and one of four outcomes:

| Outcome | Meaning and restart policy |
| --- | --- |
| FailedBeforeChange | Preflight/space, capture or plan revalidation failed; apply was never entered |
| CancelledBeforeChange | Cancellation was observed before Applying; apply was never entered |
| MayHaveMutated | Applying or health checks failed; explicit recovery/reconciliation is required |
| PostChangeCaptureFailed | Provider change and health checks ran, but required post-change capture failed; explicit recovery/reconciliation is required |

Pre-change recovery failures keep the socket code `recovery_unavailable`; stale
plans use `stale_request`. Cancellation and failures after apply use the existing
generic `failed` code. The detailed persisted outcome distinguishes them without
adding socket fields old clients cannot deserialize.

Cancellation is checked before preflight, after preflight, after capture and after
revalidation. A cancellation request observed after Applying does not skip health
or post-change capture and does not claim that the provider was cancelled. Provider
failure after entry remains potentially mutating even when the provider calls it
cancellation. There is no automatic rollback or unsafe provider interruption.

`OperationJournal::recovery_required` reports every unfinished operation and
terminal failures that may have changed state. The coordinator reloads and checks
this list under the common lock before accepting another operation. A process
death, callback panic or failed journal append leaves the last synchronized phase;
an Applying record conservatively means the callback may have run. No status
record authorizes automatic resume, replay or another apply. This component does
not yet provide an authorized reconciliation operation to clear that gate.

The new reader preserves legacy schema-1 status records with no invented context.
Unfinished legacy operations and legacy failures require reconciliation; a legacy
success is historical status and supplies no missing checkpoint proof. Schema-2
records require mutation context and schema-1 records cannot acquire it midway
through an existing operation. Older binaries fail closed on the new schema;
the socket protocol stays at version 1. Binary rollback/migration policy still
needs deployment validation before enabling mutations.

## Evidence and remaining integration

The Rust `mutation_coordination_tests` fixture independently reopens the on-disk
journal before each hook. It checks the exact association before a synthetic
provider write, injects low-space/capture/stale-plan/apply/health/post-capture
failures, and exercises cancellation before and after Applying. Independent lock
and initially stale journal handles serialize complete operations without phase
interleaving. Child processes exit without destructors from each of the six hooks;
restart preserves that phase and blocks new work without appending or reapplying.
Replay tests reject altered plan/checkpoint/action associations and malformed or
oversized records, missing/reused postimages and terminal reference rewrites.
Post-capture tests reject each missing verification dimension and reused
transaction/generation/digest/snapshot identity, and a result bound to the wrong
preimage. These are local synthetic orchestration tests, not native
Snapper, complete configuration capture, reboot or console restore evidence.

Focused concrete-path tests exercise the actual private file writer's returned
record and metadata checks, exact plan commitments, protected journal namespace
replacement/unsafe-file rejection, torn-tail replay and unresolved-work/duplicate
gates. They do not construct an opaque native checkpoint or claim execution of
the complete native call chain. Native cancellation/interruption, low-space,
post-capture failure and restart evidence remain required.

The existing generic `CheckpointEvidence` booleans remain trusted integration
attestations. Their validation checks reference shape; it does not prove native
capture, installed inventory completeness or writer exclusion. Enabling an action
still requires the independently verified native receipt and audited installed
capture boundary from #18/#19, an authenticated executor, one protected shared
journal/lock used by helper and console, affected-writer coordination, and native
interruption/recovery evidence. Issue #20 remains open.
