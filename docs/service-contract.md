# Typed service and helper contract

Status: initial Rust protocol and authorization primitives for [issue #17](https://github.com/GenieRobot/ryoku-opensuse/issues/17). This does not claim that a daemon, helper executable, provider mutation, or recovery transaction is operational.

## Protocol

`ryoku-service` defines protocol version 1 in Rust. Each envelope carries a validated bounded request ID and one typed request: capability discovery, durable operation lookup, or a named managed action. The decoder bounds input to 64 KiB, rejects unknown required fields and versions, and validates package, profile, checkpoint, request, and operation identifiers during deserialization. Responses distinguish acceptance, durable operation state, and structured errors. Operation phases are explicit, and journal transitions are restricted to `accepted → capturing → applying → verifying → succeeded`, with failure exits at each in-progress phase.

There is no generic command request. The helper request enum has only linked-checkpoint verification, package transaction, and checkpoint restore variants, each carrying opaque transaction/manifest IDs and typed package/checkpoint IDs. There are no paths, executable names, argv arrays, shell strings, environment variables, or arbitrary provider names. The helper policy currently refuses all effectful actions because no implementation can yet verify the complete linked pre-change capture required by #1 and #16.

## Authentication and persistence primitives

On Linux, `PeerIdentity::from_unix_socket` reads credentials from the connected socket using `SO_PEERCRED` through safe rustix APIs. The helper authorization rule accepts only the non-root service UID configured out of band by a root-owned installation. A deployment must reserve that UID exclusively for the unprivileged Ryoku service; same-UID authentication is not authorization for any write. This crate does not create/listen on sockets, install policy, or set up the dedicated account.

`OperationJournal` replays bounded, versioned newline JSON records from a caller-supplied already-validated file descriptor. Each legal state transition is synchronized before its in-memory state changes, so a reconnect can query the last durable state without resubmitting the action. The caller must ensure the journal file is service/root owned, mode restricted, on durable state storage, and protected by one exclusive writer. The implementation does not create or validate `/var/lib/ryoku-state`, perform file locking, rotate the journal, or implement a transaction executor.

Discovery advertises no mutation actions in this build. Known action names remain typed so the UI can present structured unsupported errors without gaining provider authority. A future executable service must report only capabilities it can actually perform after checking recovery storage and helper availability.

## Acceptance gaps

- Add a real unprivileged Rust service transport and process that authenticates its clients and returns typed responses.
- Add a privileged helper executable/service unit with a root-owned socket policy, dedicated service identity, bounded framing, replay protection, and independently reviewed installation permissions.
- Connect only the `ryoku-service` identity to the helper; test forged peer identities, other users, root, malformed/stale/unsupported requests, and ensure all denials leave system state unchanged.
- Integrate a provider and #16's verified linked capture/publication so an accepted operation survives client disconnect and can resume/query actual durable progress.
- Exercise process restart, corruption, locking, interrupted journal writes, and user configuration separation in focused tests and the disposable Tumbleweed VM.

Until those gaps pass, #17 remains open and all managed writes remain disabled.
