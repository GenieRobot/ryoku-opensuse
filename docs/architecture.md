# Architecture boundaries

The workspace has Rust crates for the UI, distribution adapters, profile data,
recovery state, integration tests, custom automation, and the typed service
contract. The service protocol supports discovery, durable operation lookup,
and read-only linked-checkpoint verification. Its per-user daemon journals
operation state; the socket-activated root helper authenticates operators and
rejects unsupported mutating requests without changing system state. The UI
client can reconnect to read an operation's durable status. No provider
mutation is currently accepted.

The recovery crate implements sealed generation capture and verification,
configuration staging and replay, a restore journal outside root rollback
scope, and preparation/confirmation paths for system-only and linked restore.
The recovery CLI coordinates current-state checkpoints, account-writer
shutdown, native rollback receipts, booted-target verification, configuration
read-back, and health-check phases. A separate read-only gate prevents the
Ryoku session from starting when recovery is pending or cannot be verified.
These code paths have fixture and subprocess coverage, but their destructive
offline-console workflow and systemd activation have not passed the required
Tumbleweed VM acceptance. The RPM declares system and user units; enabling and
running them remains an installation/acceptance task. See
[service-contract.md](service-contract.md) and
[recovery-state.md](recovery-state.md) for exact boundaries and evidence.

## Trust and write path

The implemented product must keep the UI and ordinary client code
unprivileged. A single typed backend and recovery contract must connect them to
an unprivileged Rust service and a narrow, authenticated helper. An accepted
operation must continue if its client closes. Authorization and verified
pre-change capture must gate every supported write. The captured checkpoint
must link matching Snapper system state with all declared Ryoku, configuration,
and profile state; any capture or verification failure must prevent mutation.

Root rollback alone does not constitute Ryoku recovery. The recovery design
keeps the catalog, configuration generations, and journal on the protected
state volume, and configuration replay is limited to the declared inventory so
unrelated home files remain untouched. Fixture tests cover content, metadata,
absences, hard links, interruption replay, and preservation of unrelated data.
Offline recovery commands and paired restore orchestration are implemented,
but a destructive Tumbleweed console exercise is still required to establish
that the generic path works without network or private companion data.

## Typed contract guidance

- Use domain types and enums at Rust crate boundaries. Represent invalid or
  unavailable states explicitly and return typed errors; do not encode states
  as magic strings, integers, or unrelated booleans.
- Validate untrusted input where it enters a boundary. Avoid panics and
  unchecked conversions for system, profile, storage, or IPC data.
- Keep capabilities explicit: callers receive only the operation handles and
  data they need. UI code does not invoke privileged commands or providers
  directly.
- Preserve forward-compatible, versioned serialization for persisted and IPC
  data. Do not silently treat an unknown required value as success.
- Exact shared types and provider choices are decided by the M0 feasibility
  tasks (#14–#16) before dependent implementations begin.

## Unsafe Rust and FFI

Safe Rust is the default. Unsafe code or FFI requires a focused module-level
allow with a written `SAFETY` explanation describing the invariant, ownership,
aliasing, and lifetime assumptions. Keep unsafe blocks as small as possible and
provide tests for the boundary. A separate reviewer must examine the unsafe
code and its calling convention before merge. Do not expose unchecked pointers
or foreign handles across general workspace APIs.
