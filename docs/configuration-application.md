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
