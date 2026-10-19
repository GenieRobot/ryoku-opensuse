# Issue 22 linked evidence follow-up

Recovered the unpublished linked-checkpoint public evidence allowlist and shared-reader regression against published helper commit c0617b3b0bd11d4b45645bbb8b73e2c5da085820. The originally recorded tree 19e2619e28ca3158f4f0153c156e615cdd9d47bd was unavailable; this corrected candidate includes the ACL, mutation contention and restore restart markers already emitted by the parent probe and required by its validator.

The filter accepts only bounded public result values for these marker names. Tests reject private path values. The shared-reader regression proves independent shared leases coexist and each blocks exclusive mutation until both are dropped.

Validation on 2026-10-19: locked offline full workspace tests passed outside the socket-restricting sandbox; strict Clippy for the entire workspace and all targets, formatting and diff checks passed. Initial sandbox tests failed because local Unix sockets were denied; no tests were disabled. Logs, patch and Git bundle are retained under target/integration-archive in the coordinator workspace.

No native VM acceptance is claimed. The preserved issue22 guest has partial prior fixture state; this parent runner only supports fresh provisioning, so no guest was launched or changed. Destructive linked recovery, root switch, exact boot compatibility, and the remaining issue22 acceptance matrix remain open. No PR, merge or issue closure is included.
