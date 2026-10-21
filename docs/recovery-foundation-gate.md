# Recovery foundation gate (#22)

The Rust `native_recovery_gate_probe` adds destructive failure and restart
scenarios to the accepted #21 disposable console lifecycle. Select
`issue22-lab` explicitly; a normal #21 run keeps its original result contract.
The #22 runner requires every #21 outcome followed by every fixed #22 outcome.
Missing, reordered, additional or altered results fail validation. Public output
is allowlisted; raw command output and account-specific metadata stay private.

The probe refuses outside the seeded QEMU guest and verifies the protected state
mount before every entry. The supplied production console remains unchanged at
`/usr/local/sbin/ryoku-recovery`; a Rust fixture launcher occupies its normal
command path before the numbered-root bootstrap snapshot. This fixture launcher
observes and invokes the real console. It is not installed by a project RPM.

## Scenarios

| Scenario | Required observation |
| --- | --- |
| Corrupt manifest/schema and `../` inventory path | Exact refusal exit 2; no restore attempt or transaction; complete live inventory and unrelated sentinel unchanged |
| Corrupt archived payload | Refusal before checkpoint publication/application; the exclusively owned corrupt source is restored exactly before continuing |
| FIFO at an enrolled preference | Actual capture failure; no partial transaction or mutation |
| Native Btrfs capacity reservation | Required backup refuses; the private reservation is removed and synchronized; configuration remains unchanged |
| Concurrent supported console writers | While the actual capture owner is paused with its machine lock, a second capture and restore preparation run concurrently and both refuse without publishing |
| Capture process death | SIGKILL after a successful staging fsync; same transaction tag refuses restart, incomplete artifacts remain for explicit inspection, and a fresh independent transaction succeeds |
| Restore process death | SIGKILL after each selected successful journal-directory fsync; exact journal/checkpoint readback, incomplete-session refusal and real console restart |
| Complete comparison | All 72 compiled preference records, including absent paths, regular bytes, modes, ownership, POSIX ACLs, SELinux/capability metadata and symlink values match; unrelated-home bytes and the same captured metadata match |
| Linked/system rollback | Actual Snapper rollback, reboot and native boot confirmation; matching generation/catalog/checkpoints remain available after both rollback and an extra reboot |

The phase sweep covers `Planned`, `CheckpointVerified`, `ContentStaged`,
`WritersStopped`, `ConfigurationRestored`, `HealthChecked` and `Complete` for
configuration restoration. The real linked rollback is interrupted after its
durable native handoff receipt in `WritersStopped`; restart must verify the
recorded Snapper target and request reboot without repeating rollback. The
boot-confirmation command is killed at `RootRollbackComplete` and restarted.
System completion is separately killed at `HealthChecked` and `Complete`.
Every selected boundary has a durable fixture receipt, checked again by the
post-reboot report. No journal phase is forged or rewound by the harness.

The observer uses parent/child `ptrace` syscall stops on Linux x86_64. It selects
only successful `fsync` returns and reads the exact published journal while the
child is stopped. A bounded timeout, early child exit or missing selected
boundary fails the scenario. SIGKILL is reaped; `PTRACE_O_EXITKILL` also kills the
observed child if its observer disappears. No production failpoint is added.

## Scope

This gate concerns the implemented root console and its required pre-restore
backup paths. Provider writes remain disabled. The #20 verified mutation gate
and its retained native pre-write failure evidence remain the evidence for that
separate typed mutation coordinator; this probe does not enable another provider.

System restoration supports the verified single-root Btrfs/Snapper layout with
the dedicated top-level state subvolume and standard XDG home inventory.
Additional restore roots, external ESP recovery, disk-loss backup, arbitrary
root administrators, arbitrary user xattrs and unregistered external writers are unsupported. A
rollback killed before a durable native handoff receipt still requires manual
inspection; this probe does not automatically retry that uncertain operation.
Installed package revisions must match the source; no complete alternate signed
runtime set is available for native incompatible-package proof. Candidate RPM
installation, actual desktop login and general boot compatibility are unclaimed.

The runner retains the original signed-image, repository-signature, SELinux,
offline networking, reset and owned-resource disposal requirements. A
console-only lifecycle reports `partial` for desktop coverage even when every
recovery assertion passes. Native acceptance depends on the linked receipt and
sanitized report, not the implemented harness or local process tests alone.
