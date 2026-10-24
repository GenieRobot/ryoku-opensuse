# Issue #24 integration evidence

Run10 completed native issue #24 acceptance from committed source
`ca2e736e0dee72cfdc2c1a0dac917c3dcdd9cb0d`, on base main
`6117339febe0dce613cecbf304607a3ba075bcca`. Its
[exact-source CI](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40119300801)
passed before launch. The subsequent integration update changes documentation
and sanitized evidence only; it does not replace the tested executable source.
The coordinator owns PR [#157](https://github.com/GenieRobot/ryoku-opensuse/pull/157)
integration, issue closure, branch cleanup and worktree completion.

The [Run10 receipt](../infra/tumbleweed-lab/evidence/issue24-native-20261022/run10.txt)
and [native runner output](../infra/tumbleweed-lab/evidence/issue24-native-20261022/run10-native.txt)
record clean/repeat installation, canonical versioned migration and repetition,
removal/repetition/reinstall, verified linked checkpoints, real pre/post-Applying
SIGKILL cases, exact linked recovery/reconciliation and recovered reinstall.
The completed report survived another reboot. The common offline corruption
and Snapper recovery checks also passed, followed by fresh signed-source reset
with no Ryoku package, marker, configuration or retained sentinel.

| Acceptance requirement | Direct evidence |
| --- | --- |
| Clean/repeat installation, migration and removal | All ten issue24 scenario results passed in the exact-source native report. |
| Verified checkpoint before package/configuration apply | The native probe verifies linked pre-change evidence, retained manual partners and distinct post-change partners for completed operations. Unsafe-catalog cases refuse before apply. |
| Preserve user overrides and unrelated files | Conflict checks preserve the complete package inventory, 72 managed paths, unrelated sentinel metadata/bytes and journal. Removal and reinstall retain the migrated user state. |
| Recover interrupted installation | Both actual SIGKILL phases activate emergency; new work and wrong receipts refuse; the exact source is rolled back, booted, confirmed and restored before reconciliation and a new install. |
| Compare managed state before/after | [18 comparison records](../infra/tumbleweed-lab/evidence/issue24-native-20261022/run10-managed.tsv), each with 72 entries; the declared migration changes state and subsequent repeat/removal/reinstall/recovery preserves it. |
| Identify the tested package closure | [Eight signed native RPM hashes](../infra/tumbleweed-lab/evidence/issue24-native-20261022/run10-rpms.sha256), exact source/probe/runner hashes and signature/pin receipts. |
| Reboot, reset and disposal | Completed evidence after reboot, fresh XFS-source package/marker/config/sentinel absence, unchanged signed image, removed disks/keys and an independent zero-QEMU scan. |

The signed Snapshot20261016 Minimal-VM source begins with an unnumbered direct
`@` root. The installer explicitly refuses that unsupported recovery layout
without journal/checkpoint/package changes. The disposable fixture separately
prepares the established numbered writable Snapper default, mounted snapshot
tree and GRUB snapshot booting before installer scenarios. This administrator
fixture provisioning is distinct from product installation; the installer does
not rewrite boot layout. A real bounded system-bus roundtrip handles socket
activation before native Snapper inventory after reboot. Broker dependency-only
start policy and product recovery gates remain intact.

Local Rust/Cargo 1.99 checks passed: complete workspace all-targets/all-features
tests, strict Clippy, formatting, offline cargo-deny and diff validation.
Focused coverage includes package staging/ownership, canonical migration,
exact persisted recovery receipts, pre-gate rejection classification, supported
numbered-root admission with 11 refusal variants, and fixture activation with
missing owners, invalid acknowledgement and timeout refusal. ACL/ptrace/QMP
checks ran outside the restricted process/filesystem sandbox where required.

The [chronological evidence index](../infra/tumbleweed-lab/evidence/issue24-native-20261022.toml)
retains Run01–09 failures, host interruption, authenticated metadata drift and
separate diagnostic limits. The isolated issue24 fixture changes only its
independently authenticated update metadata digest; shared #21/#22 pins remain
unchanged. No diagnostic disk, binary or service override was reused as normal
Run10 acceptance. Native runner cleanup left only local logs and receipts;
all guest disks, firmware, seeds, SSH credentials, staged inputs and sockets
were removed, and the signed source hash remained exact.

This proves the pinned historical upstream
`98a0368253d9628be72b853985878a5db90e1547` / 0.75.5-beta.20 candidate and openSUSE
port RPMs 0.1.0-7, with accepted Quickshell 0.3.2-2. Disposable local signing is
fixture trust, not a release signature. Upstream Amaterasu 1.0 is a separate
major product transition owned by #173; the port RPM release counter and
migration schema are independent. Refreshed #173/#25/#26 acceptance must test
the approved 1.0 source and closed inventory. The generic console-only runner
labels `lifecycle_result=partial` and `lab_result=partial` exclude graphical
login/UI; every #24-specific field and reset invariant passed. Hyprland login,
provider actions and the combined first-sprint gate remain separate work.

Operator commands and supported limits are in
[installer-preflight.md](installer-preflight.md).
