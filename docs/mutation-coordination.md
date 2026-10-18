# Durable mutation coordination

This is component evidence for [issue #20](https://github.com/GenieRobot/ryoku-opensuse/issues/20).
`mutation_gate::coordinate_mutation` orchestrates a trusted integration under the
existing `MutationLock`; no integration is installed and no supported mutation
is enabled. Helper requests still return structured rejections. The recovery
console remains read-only. This change does not establish helper/console writer
coordination, native capture completeness, or multi-subvolume atomicity.

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

Every record binds the same operation ID, action, bounded operation key and two
canonical SHA-256 fingerprints: the reviewed plan and affected-state inventory.
Before revalidation, the coordinator also persists the exact transaction ID,
generation ID, manifest SHA-256 and Snapper config/snapshot/filesystem link.
`LinkedGeneration::from_verified_generation` copies that reference from a
persistently verified generation. A sealed manifest digest also binds any native
snapshot receipt it contains. The integration must reopen and match that exact
generation/digest and independently revalidate its native receipt before apply.
An ID or saved successful phase is not proof that a snapshot remains available.

The post-change hook returns `VerifiedPostChangeGeneration`, explicitly bound
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
capture, changed associations, skipped/repeated phases, invalid fingerprint or
snapshot shapes, terminal transitions and inconsistent failure classifications.
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

The existing `CheckpointEvidence` booleans remain trusted integration
attestations. Their validation checks reference shape; it does not prove native
capture, installed inventory completeness or writer exclusion. Enabling an action
still requires the independently verified native receipt and audited installed
capture boundary from #18/#19, an authenticated executor, one protected shared
journal/lock used by helper and console, affected-writer coordination, and native
interruption/recovery evidence. Issue #20 remains open.
