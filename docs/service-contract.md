# Typed service and helper contract

Status: Rust protocol and authorization primitives plus a runnable unprivileged per-user daemon for [issue #17](https://github.com/GenieRobot/ryoku-opensuse/issues/17), and a fail-closed sequencing contract for [issue #20](https://github.com/GenieRobot/ryoku-opensuse/issues/20). This does not claim that a privileged helper, provider mutation, or recovery transaction is operational.

## Protocol

`ryoku-service` defines protocol version 1 in Rust. Each envelope carries a validated bounded request ID and one typed request: capability discovery, durable operation lookup, or a named managed action. The decoder bounds input to 64 KiB, rejects unknown required fields and versions, and validates package, profile, checkpoint, request, and operation identifiers during deserialization. Responses distinguish acceptance, durable operation state, and structured errors. Operation phases are explicit, and journal transitions are restricted to `accepted → capturing → applying → verifying → succeeded`, with failure exits at each in-progress phase.

There is no generic command request. The helper protocol has its own versioned, bounded envelope and only three typed request variants: linked-checkpoint verification, package transaction, and checkpoint restore. They carry validated transaction/manifest references and typed package/checkpoint IDs. Unknown fields, unsupported versions, malformed IDs, empty/oversized/duplicate package lists, and frames over 64 KiB are rejected before policy dispatch. There are no paths, executable names, argv arrays, shell strings, environment variables, or arbitrary provider names. The current response schema has only rejected outcomes: even a valid request from the configured service UID returns `unsupported` until the full linked pre-change capture and provider path are implemented. This protocol adds no helper listener and executes no mutations.

## Authentication, runtime, and persistence

On Linux, `PeerIdentity::from_unix_socket` reads credentials from the connected socket using `SO_PEERCRED` through safe rustix APIs. The helper authorization rule accepts only the non-root service UID configured out of band by a root-owned installation. A deployment must reserve that UID exclusively for the unprivileged Ryoku service; same-UID authentication is not authorization for any write. This crate does not create/listen on sockets, install policy, or set up the dedicated account.

`OperationJournal` replays bounded, versioned newline JSON records from a caller-supplied already-validated file descriptor. Each legal state transition is synchronized before its in-memory state changes, so a reconnect can query the last durable state without resubmitting the action. The `ryoku-service` executable now binds `$XDG_RUNTIME_DIR/ryoku-service.sock` (mode `0600`) and stores its journal at `$XDG_STATE_HOME/ryoku/operations.log` (mode `0600` in a private per-user directory). It refuses root startup, checks directory/file ownership and permissions, rejects a live duplicate socket, applies bounded request framing and socket read/write deadlines, and authenticates clients using kernel peer credentials. It serves one request per connection. Discovery and durable status lookup are available; all `Start` requests receive a typed `unsupported` error and are never journaled as accepted. Run it in the foreground as the logged-in user with both standard XDG variables set; no system-wide installation or service unit is included yet.

The caller must ensure the journal file is on durable state storage and protected by one exclusive writer. `mutation_gate` provides a blocking Linux `flock` guard and requires a typed verified linked-generation result plus plan revalidation before it invokes a caller-supplied apply callback. The lock descriptor must refer to a service-owned file in a non-user-writable directory, and every participating writer must lock the same inode. `flock` is advisory; it does not coordinate uncooperative external tools.

The coordinator does not create or validate `/var/lib/ryoku-state`, implement checkpoint creation/read-back verification, guarantee durable operation-history transitions around the apply callback, coordinate provider-internal writers, or implement rollback. Its traits and test fakes prove only local callback ordering and same-inode process serialization. The `CheckpointVerifier` implementation remains a trusted integration boundary; a caller-provided boolean is not independent evidence. No provider action is run by this module.

Discovery advertises no mutation actions in this build. Known action names remain typed so the UI can present structured unsupported errors without gaining provider authority. A future executable service must report only capabilities it can actually perform after checking recovery storage and helper availability.

## Acceptance gaps

- Add a managed user-session service unit/lifecycle, live end-to-end process restart/reconnect proof, and stricter race-free descriptor-based validation for runtime/state paths.
- Add a privileged helper executable/service unit with a root-owned socket policy, dedicated service identity, request replay protection, and independently reviewed installation permissions; connect its bounded listener to this typed protocol.
- Connect only the `ryoku-service` identity to the helper; test forged peer identities, other users, root, malformed/stale/unsupported requests, and ensure all denials leave system state unchanged.
- Integrate a provider and #16's verified linked capture/publication so an accepted operation survives client disconnect and can resume/query actual durable progress.
- Connect the coordinator to durable operation phases, authorization, complete #16 capture/publication, low-space checks, cancellation boundaries, provider-level writer coordination, and post-change capture; test interruptions and recovery.
- Exercise process restart, corruption, locking, interrupted journal writes, and user configuration separation in focused tests and the disposable Tumbleweed VM.

Until those gaps pass, #17 remains open and all managed writes remain disabled.
