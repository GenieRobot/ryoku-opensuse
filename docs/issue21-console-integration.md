# Offline console restore integration (#21)

This branch isolates the existing `ryoku-recovery` console implementation from
an accumulated integration checkout. Its dependency branch supplies the shared
recovery engine and Snapper catalog implementation. No desktop or network
service is required by the recovery CLI.

The console supports list, inspect, verify, linked-partner verification, scope
preview, manual checkpoints, configuration/system/linked preparation, root
rollback confirmation, configuration replay and system health completion.
Mutation commands use protected fixed storage, an exclusive mutation lock,
offline-console checks and affected-account writer shutdown. Scope preview is
read-only and cannot authorize destructive restore.

A persisted root rollback receipt supports retry after process exit before the
reboot request. Recovery verifies the running root against the exact target or
the retained pre-rollback checkpoint before continuing; it does not repeat an
uncertain Snapper rollback. Boot confirmation retries preserve later journal
phases. Health checks and linked retention promotion remain scope-specific.

## Acceptance still required

Issue #21 remains open. Host tests do not establish native Snapper rollback,
reboot handoff, metadata restoration on the deployed filesystem or normal
session startup exclusion. Acceptance requires a recorded offline console
exercise restoring deliberately corrupted managed content and metadata while
preserving unrelated home files, interruption coverage for implemented phases,
and failure coverage for unavailable partners, incompatible versions and bad
archives with the current-state checkpoint retained.

Historical Run04–Run13 component evidence predates this integrated CLI and must
not be presented as validation of its complete restore flow. In particular,
Run13 exercised synthetic journal storage/discovery and initializer refusal;
it did not execute the `pending` command or a complete recovery/session gate.
No new VM was created for this integration cleanup.
