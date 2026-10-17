# Configuration application after offline preparation

`configuration_apply::apply_configuration` realizes the entire enrolled
configuration subset from a verified generation. It handles regular files,
directories, hard-link groups, symlinks and explicit absences, then verifies
content and recorded inode metadata before advancing `ConfigurationRestored`.
The console/helper integration, writer shutdown, native root rollback, health
checks and normal-session gate remain unfinished. This API is not exposed by
the installed read-only CLI and does not complete issue #21.

The privileged coordinator supplies authenticated, approved root descriptors,
validates the persistent mount and retained current-state checkpoint, derives
compatibility, and holds exclusive ownership with all relevant writers stopped.
The writer requires the exact persisted digest-bound plan in
`RootRollbackComplete`. For configuration-only recovery, the coordinator records
the root phase as a no-op after writer shutdown; complete-linked recovery needs
the independently verified native rollback first. Journal phases alone are not
proof that native actions occurred. System-only recovery does not use this API.

Before target writes, reopen and verify the persistent disk stage, match the
journal plan, check root coverage, reject conflicting ancestor declarations and
unrepresentable/reserved path components, and validate existing parent traversal
without symlinks. A declared directory can replace an old symlink or file;
unrecorded missing parents are never implicitly created for present entries.
Descendant absences beneath an absent ancestor are supported.

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
The two explicitly ignored native tests require privileged ACL/SELinux support
and separate source/destination filesystems. Run them in the seeded disposable
Tumbleweed guest with a private Btrfs `TMPDIR` under `/var/tmp` and
`configuration_apply_tests --include-ignored --test-threads=1`. These fixtures use
synthetic snapshot observations; their native filesystem results do not establish
a complete linked console restore or real process-death/boot interruption gates.

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
unfinished attempt. This changes the initialization layout and still needs
native validation with the new binaries; Run12 predates it. Existing partial
development stores without this directory require explicit reconciliation.
The helper/session startup gate and full restore executor remain unfinished.

Run13 now proves the new journal directory/lock initialization, fixed-path
discovery, missing-store refusal without recreation, retained partial-file
refusal and discovery of a synthetic unfinished plan on native Btrfs. Healthy
repeat, enforcing reboot, bounded offline fixture restore, fresh-baseline reset
and disposable-resource cleanup passed. The [artifact](../infra/tumbleweed-lab/evidence/issue21-run13-journal-discovery.toml)
records exact inputs and evidence hashes. It does not prove restore execution
or session gating, and did not invoke the `ryoku-recovery pending` executable.
The later journal owner/group/hard-link checks are covered by local tests and
are not attributed to the Run13 binaries.
