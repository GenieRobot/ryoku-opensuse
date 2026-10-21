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
| FIFO at an enrolled preference | Actual manual capture and required pre-restore backup fail; no partial transaction, restore journal or mutation |
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
root administrators, arbitrary user xattrs, absolute/traversal-bearing symlink targets and unregistered external writers are unsupported. A
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

## Passing acceptance (22 October 2026)

Fresh Run05 exited **0** on executable candidate
`c0fd43499dd90b7cfd0d32e115286d206c755351`. Every required #22 result and the
reused #21 console lifecycle passed. The
[receipt](../infra/tumbleweed-lab/evidence/issue22-native-20261022.toml) and
[sanitized report](../infra/tumbleweed-lab/evidence/issue22-native-20261022/run05.txt)
record exact inputs, comparisons, interrupted phases, unsupported cases, prior
failed fixtures and their corrections. The candidate's
[hosted checks](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39682720008)
passed formatting, strict lints, tests and dependency review.

The tested image was signed Tumbleweed Cloud Snapshot20261016 x86_64, with kernel
`7.2.10-1-default` / package `7.2.10-1.1.36.11`, GRUB2 `2.16-2.1`, Snapper
`0.13.1-3.4`, enforcing SELinux and QEMU `11.1.2` with KVM. The derived single-root
Btrfs/UEFI layout used numbered Snapper roots and dedicated top-level state
storage. Exact signed image, firmware and repository metadata checks preceded
package writes; recovery ran with guest egress restricted.

All 72 compiled managed-home records matched after recovery, including regular
content, declared metadata, a safe sibling symlink and absence. Two unrelated
home sentinels survived; the #22 sentinel comparison includes POSIX ACL and
SELinux/capability metadata through a separately pinned Documents descriptor.
The source generation's retained profile inputs, catalog and current-state
checkpoints remained verified through actual root rollback and an extra reboot.

Storage reservation left **3,092,480 bytes**, below the mandatory 4 MiB payload.
The real checkpoint then refused with `InsufficientSpace` requiring 5,370,221
bytes while 438,272 were available. The fixture asserts the measured pressure
before expecting refusal. Sealed corrupt manifests/payloads, path traversal,
FIFO manual and pre-restore capture, missing partners and concurrent supported
console writers also produced exact refusal exit 2 without the associated
mutation. Actual capture and every selected restore phase survived the required
process deaths with retained checkpoints and correct restart behavior.

The actual sandboxed admission service refused incomplete state and admitted its
display-manager marker after completion. Fresh XFS reset proved the derived
marker, package, Snapper config and reset sentinel absent. The runner removed
owned disks, firmware, seed, staged inputs and SSH keys; independent checks found
the original image hash unchanged and no QEMU process. Private logs were removed
after recording only the sanitized report and diagnostic hashes. `lab_result`
remains `partial` because this console gate skips real desktop login.

Local Rust/Cargo 1.99 checks passed: strict workspace Clippy, formatting,
523 top-level/doctest plus 17 subprocess workspace passes, nine probe/observer
regressions, both default and #22 lab suites (78 each), offline dependency policy
and diff checks. Seven existing ignored entries remain; native VM assertions are
established by Run05 rather than those ignored tests. Issue integration and
closure remain with the coordinator.
