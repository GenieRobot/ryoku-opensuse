# Configuration application after offline preparation

`configuration_apply::apply_configuration` realizes the entire enrolled
configuration subset from a verified generation. It handles regular files,
directories, hard-link groups, symlinks and explicit absences, then verifies
content and recorded inode metadata before advancing `ConfigurationRestored`.
The recovery library now coordinates writer shutdown, a single Snapper root
rollback attempt, durable receipt publication, booted-target verification and
the system-only health-check transition. The CLI exposes
`sudo ryoku-recovery confirm-rollback RESTORE_ID` and
`sudo ryoku-recovery restore-configuration RESTORE_ID`. The latter verifies
the source and retained checkpoint partners, resolves the sealed local account
through the protected account database, rechecks a complete-linked boot target,
replays staged files through fixed directory descriptors, then validates JSON
and typed profile data before completing the journal. Both commands require
the protected mount and exclusive mutation lock. The CLI performs account
writer shutdown before capture/replay and runs the configured health checks.
`prepare-system`/`rollback-system` expose system-only restore, while
`prepare-linked`/`rollback-linked` expose the paired root and configuration
path. The rollback commands request a reboot after Snapper records its
attempt-tagged receipt; immediately beforehand they compare the live root UUID
to the exact origin sealed into the retained current-state checkpoint. After
boot, the operator re-enters rescue/emergency mode, confirms the target and
either completes system health checks or replays the staged configuration.
These CLI paths compile and reject malformed IDs, but the destructive offline
VM acceptance exercise remains outstanding; issue #21 remains open.

The privileged coordinator supplies authenticated, approved root descriptors,
validates the persistent mount and retained current-state checkpoint, derives
compatibility, and holds exclusive ownership with all relevant writers stopped.
The writer requires the exact persisted digest-bound plan in `WritersStopped`
for configuration-only recovery, or `RootRollbackComplete` for complete-linked
recovery. A configuration-only journal moves directly from `WritersStopped`
to `ConfigurationRestored`; its valid phase set cannot claim a root rollback.
Complete-linked recovery requires an independently verified native rollback
first. The journal receipt records Snapper's exact previous-default, backup
and target snapshot IDs while the attempt is in `WritersStopped`. After reboot,
native verification must match the running Btrfs root identity to the writable
active/default target, confirm its source parent and read-only backup, and
confirm the Snapper attempt tag before the journal can enter
`RootRollbackComplete`. Resume then replays configuration and advances to
`ConfigurationRestored`; health checking is still required before `Complete`.
Journal phases alone are not proof that native actions occurred.
System-only recovery does not use this API.

Before target writes, reopen and verify the persistent disk stage, match the
journal plan, check root coverage, reject conflicting ancestor declarations and
unrepresentable/reserved path components, and validate existing parent traversal
without symlinks. Destination descriptors must identify distinct directory
inodes: assigning the same directory to multiple manifest roots could route
otherwise valid relative paths into the wrong configuration namespace, so the
writer rejects aliased descriptors before creating local stages or changing
targets. A declared directory can replace an old symlink or file;
unrecorded missing parents are never implicitly created for present entries.
Descendant absences beneath an absent ancestor are supported.

Generation capture applies the same namespace rule to source descriptors. Two
different `StateRoot` labels may not refer to one directory inode, or the
checkpoint could seal the same physical namespace under two independent
relative-path inventories and later route a restore ambiguously. Capture rejects
that condition before reading files or publishing a catalog entry.

Each destination parent gets a private mode-0700 staging directory with a
mode-0600 ownership receipt bound to the attempt, plan, root and parent path.
This permits publication when the persistent source stage and destination are
on different filesystems or subvolumes. Stage regular-file bytes and metadata
before renaming, and create aliases from the first published group member.
Replacing a file inode preserves the bytes and recorded metadata of unrelated
hard-link peers. Symlinks are recreated without following either old or new
targets. Directories are materialized before children and receive final owner,
mode, ACL, SELinux label and mtime after child writes and temporary-stage cleanup.
Directory removal only succeeds when empty; undeclared children are retained.

File/symlink replacement uses an atomic rename. Changing between directory and
non-directory types can require an unlink/create gap. The persisted phase remains
in recovery across that gap. Flush objects and relevant directories before each
progress callback; cancellation leaves the checkpoint reference and recovery
phase intact. Resume reopens the source/stage/journal and replays the complete
configuration idempotently. A foreign or incomplete local staging receipt is
never adopted or removed automatically. An interruption before receipt completion
therefore stays safely in recovery and needs explicit reconciliation. Keep normal
writers/session startup blocked throughout; this writer does not implement that
system-level gate itself.

Only a full read-back of bytes, metadata, absences and hard-link equivalence
advances the journal. Reopening an already `ConfigurationRestored` attempt only
verifies; it refuses new target changes rather than overwriting them silently.
The coordinator must still run health checks before completing recovery.

Tests exercise all inode kinds, retained unrelated data/metadata, nested parents,
hard links across parents, declared directory type changes, symlink confinement,
nonempty directory refusal, stage/phase failures, foreign receipts, path bounds,
completed-phase tampering and reopen/replay at every durable callback boundary.
A subprocess test also exits without running Rust destructors after each of the
six durable apply boundaries; the parent reopens the real journal and replays the
same staged checkpoint, verifying managed files and the unrelated-home sentinel.
The two explicitly ignored native tests require privileged ACL/SELinux support
and separate source/destination filesystems. Run them in the seeded disposable
Tumbleweed guest with a private Btrfs `TMPDIR` under `/var/tmp` and
`configuration_apply_tests --include-ignored --test-threads=1`. These fixtures use
synthetic snapshot observations. The process-exit test is complemented by a
local subprocess test that receives SIGKILL after each of the same six durable
apply boundaries; the parent reopens the journal, replays the checkpoint, and
checks managed state and the unrelated-home sentinel. These userspace tests do
not establish kernel crash, reboot, or the complete linked console restore gate.

Run05 passed all 13 cases, including both native-only tests, on Tumbleweed
20261013 with SELinux enforcing. The [component evidence](../infra/tumbleweed-lab/evidence/issue21-run05-configuration-apply.toml)
records the exact test-binary/log hashes and remaining acceptance limits.

Restart discovery now provides `RestoreJournalStore::incomplete_attempts`, a
bounded scan under the same journal writer lock. It validates every record and
returns every phase before `Complete`, in stable attempt-ID order, without
requiring IDs remembered by a client. Corrupt records, external links, unknown
entries, retained temporary publications and excessive directory size refuse
an all-clear result. Tests reopen the store at every durable phase and cover
those refusal cases. This API does not yet enforce a normal-session startup
gate: the coordinator must validate the persistent mount, serialize operations,
and consume the scan before session use. No issue acceptance claim is made
from these component tests alone.

The console now exposes `ryoku-recovery pending` through that scan. It reads
the independent machine record, verifies the fixed Btrfs state mount, and
opens `restore-journals` and its existing lock without creating either. Missing
or unsafe journal storage is an error rather than an empty list. Explicit
state initialization creates and synchronizes that directory/lock before
machine identity publication; healthy repeated initialization refuses any
unfinished attempt. Each pending row now includes the selected source
transaction/generation, restore scope, current-state generation, and durable
phase, so a console operator can identify which attempt needs attention. The
command explicitly states that it does not gate normal-session startup. This
changes the initialization layout and still needs
native validation with the new binaries; Run12 predates it. Existing partial
development stores without this directory require explicit reconciliation.
The Rust `ryoku-helper --recovery-gate` mode and separate read-only socket now
provide a session gate: `ryoku-session compositor` requires a verified `clear`
response before starting Hyprland, and refuses Ryoku startup on pending or
unavailable recovery state. The gate does not block KDE or other login
sessions. It is opt-in and requires an administrator to enable
`ryoku-recovery-gate.socket` after recovery storage is initialized. The native
socket activation and pending-journal refusal still need Tumbleweed evidence;
full linked restore remains unaccepted until offline execution and interruption
behavior are recorded.

Run13 now proves the new journal directory/lock initialization, fixed-path
discovery, missing-store refusal without recreation, retained partial-file
refusal and discovery of a synthetic unfinished plan on native Btrfs. Healthy
repeat, enforcing reboot, bounded offline fixture restore, fresh-baseline reset
and disposable-resource cleanup passed. The [artifact](../infra/tumbleweed-lab/evidence/issue21-run13-journal-discovery.toml)
records exact inputs and evidence hashes. It does not prove restore execution
or session gating, and did not invoke the `ryoku-recovery pending` executable.
The later journal owner/group/hard-link checks are covered by local tests and
are not attributed to the Run13 binaries.
