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

## Local verification and pending acceptance

Formatting, strict workspace Clippy, offline cargo-deny and the complete workspace
suite passed: 519 top-level test/doctest passes plus 17 isolated subprocess passes,
zero failures and seven ignored entries (including the restore crash-test child
that its parent executes explicitly). The recovery crate contributes 210 library,
26 console and two initializer tests. Tests cover staged publication failure,
configuration replay/metadata/absence and unrelated files, durable phase
interruptions, rollback receipt/boot verification, resume without phase rewind,
and admission refusal at every incomplete linked phase and for corrupt journals.
Native process/ACL tests require the unsandboxed host boundary; a sandboxed run
failed those environmental checks before the authorized native rerun passed.

`rpmspec --parse` passes for the updated tools spec. Host-only systemd verification
cannot complete because the enrolled mount unit and `/usr/bin/ryoku-recovery` are
not installed on the development host; this is not recorded as installed-unit
acceptance. Packaged installation and native admission remain to be exercised.

Issue #21 and PR #155 remain open/draft. The exact current CLI still needs a
recorded offline Tumbleweed exercise covering corrupted content/metadata,
unrelated-home preservation, real interruption/resume, system/linked reboot
handoff, unavailable partners, incompatible versions and bad archives with the
checkpoint retained. Historical Run04–Run13 component receipts do not validate
this candidate. The first fresh signed KVM run reached the offline CLI but refused
its first manual checkpoint because fixture provisioning omitted `ryoku-desktop`.
No restore journal or generation was created. Read-only extraction confirmed the
failure; the owned VM and private artifacts were removed. The corrected fixture
installs and verifies the exact seven signed release5 runtime RPMs and resolves
their official dependencies before restricting networking. Fixed serial operation
and failure markers permit bounded host diagnosis while rescue refuses SSH.
See the [native receipt](../infra/tumbleweed-lab/evidence/issue21-native-20261021.toml).

There is no complete signed alternate desktop/runtime/defaults package set in the
existing lab assets. Native incompatible-package refusal is explicitly unclaimed;
focused compatibility tests cover refusal and current-checkpoint retention. The
fixture installs the exact candidate console/admission unit files after the
existing signed runtime packages; it does not prove installation of a newly built
candidate RPM or an actual desktop login. Its normal-session marker exercises the
display-manager dependency and admission service only.
