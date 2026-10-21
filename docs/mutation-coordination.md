# Durable mutation coordination

Issue [#20](https://github.com/GenieRobot/ryoku-opensuse/issues/20) connects the
durable mutation gate to native linked capture, complete installed configuration
capture, account writer leases, protected history and an independent root history
console. **The installed supported provider action set remains empty.** The
helper dispatcher continues returning typed unsupported responses; the native
fixture actuator is confined to the seeded disposable QEMU experiment. This
library does not certify package/provider hooks or enable a protected write.

## State machine

`coordinate_mutation` serializes trusted integrations with `MutationLock`.
`native_mutation::run_operator_mutation` first authenticates a kernel socket peer
against operator policy, resolves only that account through NSS and opens a
`NativeCaptureSession`. That session owns the existing fixed machine lock,
`/var/lib/ryoku-state/mutation.lock`, and stops all registered account writers.
The native coordinator enters its internal locked path without acquiring a
second flock description. The session remains alive through final accounting.

Every detailed phase is synchronized in `/var/lib/ryoku-state/mutations.log`
before its corresponding work starts. The lock is acquired before opening,
repairing or refreshing this shared journal. A duplicate ID or unresolved prior
operation prevents another accepted operation. Per-user status journals cannot
substitute for this protected coordinator journal.

| Detailed stage | Protocol-v1 phase | Work after durable record |
| --- | --- | --- |
| Preflight | accepted | Recheck exact plan authorization, NSS/path bindings and complete inventory |
| StateChecking | capturing | Validate protected mount, space, native layout and catalog availability |
| Capturing | capturing | Create native root checkpoint; capture, seal, publish and reopen full installed configuration; verify native partners and retention catalog |
| Revalidating | capturing | Reauthorize and recheck plan, all covered paths, exact sealed/native partners and catalog completeness |
| Applying | applying | Repeat final native/plan checks, then enter the trusted provider mutation |
| HealthChecking | verifying | Revalidate boundaries and inspect provider result |
| PostChangeCapture | verifying | Create and verify fresh native/configuration state and retention linkage |
| Complete | succeeded | Persist exact verified postimage alongside retained preimage and return result |

`MutationContext` binds each operation/action to its immutable operation key,
canonical SHA-256 plan fingerprint and complete affected-inventory fingerprint.
The native inventory fingerprint includes content, metadata, safe links and
explicit absent entries across the compiled first-sprint inventory. Two live
reads must agree; unknown managed files refuse the operation. Provider `plan()`
must separately describe its entire typed write set and current provider state.
The coordinator replaces its affected-state fingerprint with the independently
collected account inventory. A reviewed result supplies no authorization token:
a fresh execution must compare it under a new lease.

After capture, the same synchronized record binds the transaction ID, generation
ID, manifest digest and native Snapper config/number/filesystem identity.
Native readback opens the original sealed receipt and matches exact kernel
snapshot identity, payloads and metadata; it never manufactures provenance from
a later observation. The retained cleanup catalog must independently report
complete. The coordinator repeats these checks after authorization/capture and
again immediately before the provider call. An ID or saved successful phase
never authorizes apply.

The postimage must use fresh transaction/generation IDs, digest and native
snapshot number for the same filesystem/config, bound to the exact preimage.
Replay rejects changed associations, skipped phases, missing/reused postimages,
invalid fingerprint/reference shapes and terminal rewrites. Schema 2 adds this
context to the append-only journal; schema 1 status records remain readable with
no invented checkpoint proof. Older binaries refuse unknown schema/stages.
Deployment/binary rollback compatibility remains separate from this component.

## Native scope and writer coordination

The native integration supports one default root Snapper config and the full
compiled account preference inventory. Extra Snapper configurations refuse
rather than silently omitting their subvolumes. Layout and catalog checks retain
#19's root/boot-file coverage limits and exclude the ESP. There is **no
multi-subvolume atomicity claim**, general boot compatibility proof or automatic
rollback. The two captures are serialized observations around one provider.

Every registered writer retains its #18 lease from before root capture until
post-change capture and final history synchronization finish. New registered
writers wait on their shared coordination lock. A dead wrapper, stale endpoint,
changed namespace, lost connection or failed lease check prevents pre-change
mutation. Ordinary external editors are not frozen; double collection and later
boundary checks reject observed changes without claiming a home snapshot.
Native provider-internal writers must independently obey the machine lock before
that provider can be enabled. The installed helper's capability policy and
zero-certified-provider-write boundary are unchanged.

Native systemd may mount `/run/user/<uid>` separately. The writer reader permits
only that exact tmpfs root with private UID/GID ownership and `rw,nosuid,nodev`.
It pins and rechecks the descriptor mount ID and inode. Runtime bind substitution,
symlinks and mounts below that boundary refuse; descendant opens retain
`NO_XDEV`. The fixed root-owned runtime parent must remain safe.

State space checks conservatively require room for bounded pre/post generations
and native root changes. They are estimates, not reservations. Later allocation,
capture, metadata or synchronization errors still prevent apply. The protected
store is validated against its independent machine identity at each boundary.

## Cancellation, failures and restart

| Durable outcome | Meaning |
| --- | --- |
| FailedBeforeChange | Preflight, state/space, capture or revalidation failed; apply was never entered |
| CancelledBeforeChange | Cancellation observed before Applying; apply was never entered |
| MayHaveMutated | Applying or health failed; explicit recovery/reconciliation required |
| PostChangeCaptureFailed | Mutation and health ran, required post capture failed; explicit recovery/reconciliation required |

Cancellation is checked before preflight, after preflight, after state/space
checks, after capture and after revalidation. Once Applying is synchronized,
a cancellation flag cannot skip health or accounting or claim provider undo.
Pre-change recovery errors retain protocol-v1 `recovery_unavailable`; stale plans
use `stale_request`. The persisted outcome distinguishes cancellation and
potentially mutating failures from these coarse error codes.

Process death, panic or failed journal sync preserves the last synchronized
stage. An Applying record conservatively means the provider may have run.
`OperationJournal::recovery_required` reports unfinished operations and terminal
failures that may have changed state. The coordinator reloads and checks that
list under the machine lock before accepting work; it never automatically
replays apply or resumes a provider from a status record.

`ryoku-mutation-history status` independently opens the same protected state,
lock and journal and prints exact history plus the unresolved count. It is
root-only and accepts no path/identity override. The production helper startup
uses this same history gate. Since the console serializes replay against
mutation, it can wait until an active operation releases the lock.

`ryoku-mutation-history reconcile-before-change OPERATION_ID` is an explicit
root console action that seals an unfinished Preflight, StateChecking, Capturing
or Revalidating record as failed before mutation. It neither deletes checkpoints
nor repeats a provider. Applying, HealthChecking, PostChangeCapture, completed
and terminal records refuse reconciliation. Potentially mutating work remains
blocked for the separate authorized recovery workflow. This history console
executes no restore and does not replace the #21 offline restore gate.

## Reproducing the native fixture

Use only the existing signed Snapshot20261016 asset bundle and a fresh short
issue20 run path. Build from this checkout:

```text
cargo build --locked --offline -p ryoku-automation --features issue20-lab --bin vm-lab -p ryoku-recovery --bin ryoku-state-init -p ryoku-service --bin ryoku-mutation-history --example native_mutation_probe
```

From the authorized WSL workspace root:

```text
RYOKU_LAB_AUTHORIZED=existing-wsl-vm PATH_TO_BUILD/vm-lab run EXISTING_SIGNED_ASSETS FRESH_ISSUE20_RUN --console-only --state-initializer PATH_TO_BUILD/ryoku-state-init --mutation-probe PATH_TO_BUILD/examples/native_mutation_probe --mutation-console PATH_TO_BUILD/ryoku-mutation-history
```

Repeat with a second fresh run path and `--mutation-interrupt` for process death
inside Applying. Pin copies of all four completed executables outside Cargo's
output directory before the pair: workspace tests can rebuild ordinary binary
outputs with a different unified feature set. Both variants must report the same
staged hashes. `issue20-lab` selects only `issue20-fixture.toml`; #19 and generic
fixture inputs/defaults are unchanged. Input binaries are staged and hashed
before boot. The harness requires exact allowlisted results and normal signed
boot, reboot, offline fixture recovery, fresh-base reset and owned-disk/key
disposal. Console-only results remain partial desktop/lifecycle acceptance.

The fixture's local imported profile defaults and marker actuator are synthetic
inputs; native Snapper, Btrfs, configuration capture, writer wrapper, kernel peer
authentication, journal, console and process boundaries are real. It exercises
actual low space by allocating its owned disk, an invalid personal profile that
fails capture, changed provider plans and external enrolled-path changes after
capture, cancellation, independent concurrent processes and retained writer
leases. Before every first marker write it independently reopens the durable
Applying association and native generation/catalog, checks that the preimage
lacks the marker, then synchronizes a verification event before mutation. A
successful postimage must contain the exact marker. Process exits without Rust
destructors at Revalidating and Applying test restart refusal; post-capture
failure remains distinguishable after reboot through the independent console.

The instrumented Rust unit suite also covers failures and process death at every
phase, cancellation on both sides of Applying, independent stale journal views,
malformed/tampered replay and missing/reused/mismatched postimage proofs. Native
results and exact artifact hashes are recorded with the issue acceptance evidence.

### Current native evidence

The [final identical-input pair](../infra/tumbleweed-lab/evidence/issue20-final-pair-20261021.toml)
records normal run09 and interrupted-Applying run10, both exiting 0. They use the
same image, firmware, four staged executable hashes and signed repository pins.
Their allowlisted output differs only at the expected final scenario result.
Both prove the direct unnamed-allocation ENOSPC assertion, capture/plan/path
failure refusal, independent helper serialization, retained writer leases,
first-mutation ordering, reboot history readback, offline fixture recovery and
fresh signed-base reset/disposal. The normal variant records distinct
post-capture failure; the interrupted variant retains Applying and its exact
preimage, refuses new work and unsafe reconciliation, and invents no postimage.

The [cache diagnosis](../infra/tumbleweed-lab/evidence/issue20-cache-diagnostic-20261021.toml)
records authenticated fresh run08 identifying the update repository's changed,
nonempty metadata. Its exact digest matched a host download whose detached
signature verified against the pinned openSUSE key before updating the isolated
#20 pin. Empty metadata now fails explicitly; mismatch diagnostics retain the
alias and both digests. SSH host verification remained enforced. The
[earlier evidence](../infra/tumbleweed-lab/evidence/issue20-mutation-20261021.toml)
remains historical: run03/run04 are qualified nonidentical inputs, while run05
and run06 failed setup and their unrecorded cache digests remain unknown.

This completes the #20 native mutation-fixture pair. Provider capabilities stay
disabled; fixture inputs do not certify a provider write or a production linked
restore. Console-only results still report partial desktop/lifecycle acceptance.
Keep the draft and issue open for coordinator review of the final source and CI.
