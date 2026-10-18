# Issue 22 recovery engine integration

This branch isolates the existing recovery library and native probe closure from
the dirty integration checkout at a351f64. Its base is issue 19 dependency
d2981d7, which supplies the catalog, pair and rollback primitives.

The recovery library and examples are copied from the preserved integration
checkout. Only the two personal-profile capture constants are taken from the
profiles API changes; profile migration implementation is excluded. The existing
packaged defaults JSON is included because the library fixture reads its bytes.
The console CLI, initializer, installer, service/session gate wiring and mixed
VM runner changes are outside this dependency slice. Original dirty worktrees
and the retained VM are preserved.

Host checks on 2026-10-18: recovery library tests passed (231 passed, 0 failed,
9 ignored); all recovery examples compile; workspace formatting and diff
whitespace checks pass. These checks will be repeated on the committed tree.

Issue 22 remains open. The destructive VM matrix, complete managed-state and
unrelated-home comparisons, backup-failure mutation refusal, paired root/Ryoku
restore and catalog survival across rollback are not established by these host
checks. The preserved guest previously stopped at CatalogEntryMissing; the
existing probe correction has not been executed in that guest. No VM was
launched or modified for this isolation.
