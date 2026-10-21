# Protected installer lifecycle (#24)

`ryoku-install` is an explicit root console command in
`ryoku-tools-state-init`. It uses the shared machine writer lease, durable
mutation journal and verified native linked checkpoints. Each execution runs
in a named systemd service and survives closure of its waiting client. Every
new operation requires a fresh preview and fresh verified pre-change capture.

The supported target is Tumbleweed x86_64, a verified single-root Snapper layout
with root-covered `/boot` files, an independently initialized state mount, and
an existing KDE fallback session. External ESP restoration and extra Snapper
roots remain unsupported. Stop graphical login and use an independent root
console. Establish an exclusive administrator maintenance window; external
root package commands do not participate in Ryoku's writer lock.

## Bootstrap and reviewed package closure

The recovery bootstrap is separate from desktop installation: install the
matching `ryoku-tools` and `ryoku-tools-state-init` RPMs and explicitly run
`ryoku-state-init`. This provides the independent console and admission units
before the first desktop checkpoint. Bootstrap does not create personal
configuration or activate a desktop. Distro dependencies and KDE must already
be installed through the distribution's normal signed package policy.

After entering the maintenance console, establish the target account's native
runtime and writer coordination namespace before a preview or capture:

```console
sudo systemctl start user-runtime-dir@UID.service
sudo setpriv --reuid=UID --regid=GID --init-groups /usr/sbin/ryoku-install account-bootstrap
```

Use the account's NSS UID and primary GID. `account-bootstrap` accepts no
arguments and initializes only its ordinary kernel caller's writer namespace
and lock in the existing native `/run/user/UID`. Root, extra arguments, a
different primary group and missing/unsafe runtime prerequisites refuse. It
does not create runtime directories, change account preferences or publish a
checkpoint. Reestablish it after rescue isolation or reboot if logind removed
the account runtime. The protected capture lease continues to refuse a missing
namespace; its existing #22 behavior is unchanged.

The candidate closure uses desktop/runtime/fonts/helper/defaults/tools release
`0.1.0-7` and unchanged Quickshell `0.3.2-2`. Desktop requires matching
runtime/helper/defaults/tools releases; runtime requires matching fonts and the
fixed Quickshell revision. The state initializer requires matching tools.
Optional package identities are `ryoku-ui-proof` and
`ryoku-tools-launcher-status`. An unrecognized installed Ryoku RPM refuses
installation until separately reviewed. Accepted versions are compiled into
`installer_state`; absence is explicit for every recognized name.

The installer stages stable bounded local RPM bytes in a private directory,
checks complete direct payload ownership and directory metadata, rejects
out-of-scope paths and package scriptlets/triggers, and requires signatures
from keys already trusted by native RPM. It applies only those exact local
RPMs with native dependency checking. It never asks a solver to add, remove or
update distro dependencies. An incomplete closure fails the native RPM test
before capture/apply. Direct Ryoku payloads are limited to `/usr`.

## Preview and execution

`LAYERS` is `none` for explicitly absent system/companion inputs or the exact
SHA-256 of a retained immutable local profile bundle. It is never a remote
repository or profile pathname. Existing system/companion source bytes are
retained exactly while packaged defaults change. Personal settings remain
account-owned; installation and removal do not edit them.

```console
sudo ryoku-install lifecycle-preview install UID LAYERS RPM [RPM ...]
sudo ryoku-install install UID OPERATION LAYERS REVIEWED_SHA256 RPM [RPM ...]
sudo ryoku-install lifecycle-preview remove UID LAYERS
sudo ryoku-install remove UID OPERATION LAYERS REVIEWED_SHA256
sudo ryoku-install lifecycle-preview migrate UID LAYERS
sudo ryoku-install migrate UID OPERATION LAYERS REVIEWED_SHA256
```

Use the printed `plan_sha256` as `REVIEWED_SHA256`. Operation IDs are fresh
bounded identifiers; they cannot be reused. The digest binds the action,
account, installed Ryoku baseline, selected package identities and bytes,
direct payload state, fixed distro policy/KDE evidence and retained layer
identity. The declared managed-state fingerprint is revalidated under the
same account writer lease. A digest is a review identifier, never an
authorization token or checkpoint receipt.

Execution is owned by `ryoku-installer-OPERATION.service`. Inspect its journal
and the protected mutation history after an error. The shared coordinator
persists each boundary before entering it and refuses new work while an
operation may have changed state. The installer reopens exact native snapshot,
generation and catalog identities before apply, checks the full managed user
inventory and distro policy afterward, then captures a distinct verified
post-change partner. A failed checkpoint prevents package/configuration apply.

The older `preview RPM...`, `remove-preview` and `migration-preview UID`
commands remain read-only diagnostics. Their Zypper previews can show
transitive proposals; they do not authorize or execute those proposals.

## Versioned migration and removal

Migration rule `installer-migrate-v1` recognizes only the byte-exact canonical
schema-1 `generic-desktop` document produced by the original Rust prototype
at `.config/ryoku/profile.json`. Upstream Hub JSON, different formatting,
unknown versions and ambiguous/conflicting destinations refuse. The legacy
source is retained, including metadata. The destination
`.config/ryoku/opensuse-profile.json` is atomically linked from a complete,
synced unnamed inode with target account ownership and mode `0600`; an
existing destination cannot be overwritten. An identical completed migration
is idempotent. Pre/post generations record source-to-result bytes and metadata.

Removal erases only the recognized desktop/runtime/defaults/helper/optional
Ryoku packages after a successful native dependency test. It retains the two
recovery bootstrap packages, all protected generations/journals/profile inputs,
all user files and unrelated dependencies. It does not request clean-deps.
Repeat removal is a protected no-op with fresh verified checkpoints. Reinstall
uses the same signed complete local closure and preserves personal overrides.

## Installer-only absent-default provenance

A clean target or removed desktop has no packaged default document. Profile
bundle schema 2 explicitly records `defaults_absent=true`, the default package
identity with revision `absent`, and a fixed neutral `generic-desktop` input.
It does not invent an installed RPM. All ten recognized package entries have
an affirmative present revision or absence. Schema 1 remains byte-compatible.

Schema 2 is accepted for capture/restore only with the exact
`installer-lifecycle-v1` source reason and closed installer package inventory.
The console uses that inventory only for a marked installer source; ordinary
console checkpoints keep their original three required installed packages.
Unknown/missing markers, unknown package names/versions, omitted absence
entries, schema mixing and inconsistent default presence refuse.

## Interrupted execution and recovery

A crash before `Applying` has no authorized package/profile mutation. From the
root console, explicitly reconcile that record before planning fresh work:

```console
sudo ryoku-install recover UID OPERATION none
```

After `Applying`, retain the original `OPERATION-pre` checkpoint and restore
that exact source through the independent console:

```console
sudo ryoku-recovery prepare-linked RESTORE_ID OPERATION-pre
sudo ryoku-recovery rollback-linked RESTORE_ID
# After reboot into the recorded target:
sudo ryoku-recovery confirm-rollback RESTORE_ID
sudo ryoku-recovery restore-configuration RESTORE_ID
sudo ryoku-install recover UID OPERATION RESTORE_ID
```

Reconciliation requires the exact completed linked restore journal, original
source manifest/generation/transaction, booted native rollback target, restored
package inventory and integrity, and matching complete account configuration
bytes/metadata/absence. Wrong, missing or ambiguous receipts keep the gate
closed. The original failure stays in history alongside its bound installer
restore receipt. Reconciliation never reapplies a transaction; subsequent
installation requires a new preview, operation ID and checkpoint.

## Evidence

Historical October 18 preview receipts cover read-only behavior only. Current
native lifecycle acceptance remains pending until the dedicated #24 VM report
is linked on its issue and PR. Package installation does not claim Hyprland
login, provider mutations or the combined first-sprint gate.
