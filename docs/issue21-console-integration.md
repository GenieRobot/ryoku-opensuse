# Offline console restore (#21)

The console and required shared restore implementation now build on main after
#20, independently of the old #22 branch. The port retains main's complete
compiled `UserHome` preference inventory, RPM-backed profile bundles and
`ProtectedSnapperCatalog`; it does not import the #22 failure-gate harness.

`ryoku-recovery` provides list, inspect, verify, verify-linked, scope preview,
manual-checkpoint, configuration/system/linked preparation, rollback handoff,
boot confirmation, configuration replay and health completion. Mutations require
root, protected fixed storage, the common exclusive mutation lock and an offline
rescue/emergency target with graphical.target inactive. The affected account's
sessions and user manager must be stopped and rechecked. The current-state
checkpoint captures the complete installed inventory and exact local default
profile bundle, is verified and retained before application, and is never deleted
by failed restore or staging repair.

System and linked restore support the verified single-root Snapper layout.
Additional Snapper configurations refuse; external ESP and general boot
compatibility remain outside the proven scope. Source package revisions must
match the snapshot's RPM inventory. The source snapshot must contain the exact
recovery admission units, so rolling back cannot remove the gate while a linked
restore still needs configuration replay. A durable native rollback receipt
prevents uncertain rollback replay. Boot confirmation requires the exact writable
Btrfs/Snapper target before configuration restoration or completion.

The existing `ryoku-tools-state-init` subpackage now owns the admission service
and display-manager dependency, and requires the matching console package. No
additional subpackage was introduced. Installing this administrator integration
makes graphical login depend on independently provisioned state and a complete,
valid recovery journal. `admit-session` reads that journal under the common shared
mutation lock; pending, missing, corrupt or busy state refuses admission. The
service's failure requests emergency.target. Restore preflight checks exact unit
bytes/metadata and the display manager's loaded Requires/After dependencies.
Package installation does not initialize state, enable a mount or activate the
service. Operators must reload systemd after installation; the preflight refuses
stale loaded dependencies. The gate covers the supported display-manager login
path, not manually launched compositors from arbitrary root-controlled consoles.

## Verification

Formatting, strict workspace Clippy, offline cargo-deny and the complete workspace
suite passed: 523 top-level test/doctest passes plus 17 isolated subprocess passes,
zero failures and seven ignored entries (including the restore crash-test child
that its parent executes explicitly). The recovery crate contributes 211 library,
26 console and two initializer tests. Tests cover staged publication failure,
configuration replay/metadata/absence and unrelated files, durable phase
interruptions, rollback receipt/boot verification, resume without phase rewind,
and admission refusal at every incomplete linked phase and for corrupt journals.
The same-source configuration-to-linked regression proves that different receipt
inputs cannot reuse a stage. Compatibility refusal tests verify a current-state
checkpoint remains available without staging or application. Native process/ACL
tests passed at the authorized host boundary. `rpmspec --parse` also passed.

Fresh Run05 completed with exit 0 on the exact executable candidate
`aeed2e71c6b1fdc7d6ac148ebc4a3e4653d1eb9b`; its hosted
[CI run 39673083227](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39673083227)
passed. The existing authorized WSL/KVM runner verified the signed 20261016 image,
pinned repository metadata and seven signed release5 runtime RPMs, then restricted
guest networking before offline recovery. SELinux remained enforcing. Guest clock
preflight measured post-set skew 2s within its 5s bound before repository writes.

The native CLI passed configuration restoration with exact bytes, metadata and
absence; unrelated-home preservation; actual SIGKILL at WritersStopped and replay;
linked and system-only root rollback/reboot/confirmation; retained current-state
checkpoints; bad-archive and unavailable-partner refusals. Refusals required exact
exit 2, successful operations exit 0. The actual admission service refused pending
journals and admitted a fixture display-manager marker only after completion.
All three journals and checkpoints were checked again after another reboot.

The runner then proved ordinary offline Snapper corruption/restore, discarded the
owned derived disks and booted a fresh signed XFS reset. Fixture markers, installed
package, Snapper configuration and reset sentinel were absent. Owned disks,
firmware, seed and SSH keys were removed; private logs were removed after recording
the allowlisted report. Independent SHA256 verification confirmed the source image
unchanged and the QEMU process scan was empty. Overall `lab_result=partial` is the
expected console-only result because KDE provisioning and actual desktop login
were skipped; all claimed #21 results passed. The
[native receipt](../infra/tumbleweed-lab/evidence/issue21-native-20261021.toml) records
hashes, prior failed runs and corrections; the
[Run05 report](../infra/tumbleweed-lab/evidence/issue21-native-20261021/run05.txt)
contains the sanitized lifecycle results.

There is no complete signed alternate desktop/runtime/defaults package set in the
existing lab assets. Native incompatible-package refusal is explicitly unclaimed;
focused compatibility tests cover refusal and current-checkpoint retention. The
fixture installs exact candidate console/admission unit files after the existing
signed runtime packages; it does not prove installation of a newly built candidate
RPM or an actual desktop login. Its normal-session marker exercises the
display-manager dependency and admission service only. Provider actions were not
exercised. Issue #21 and PR #155 remain open/draft for coordinator review.
