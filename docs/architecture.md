# Architecture boundaries

The workspace establishes crate boundaries for user interface, distribution
adapters, profile data, recovery state, integration tests, custom automation,
and a versioned Rust service contract. The service crate provides validated
request/response types, a per-user Unix-socket daemon and journal, plus a
socket-activated root helper that rejects requests fail-closed. The RPM
declares system and user units but does not enable them. Provider mutations,
linked recovery execution, and installed Tumbleweed runtime proof are still
missing; see [service-contract.md](service-contract.md).

## Trust and write path

The implemented product must keep the UI and ordinary client code
unprivileged. A single typed backend and recovery contract must connect them to
an unprivileged Rust service and a narrow, authenticated helper. An accepted
operation must continue if its client closes. Authorization and verified
pre-change capture must gate every supported write. The captured checkpoint
must link matching Snapper system state with all declared Ryoku, configuration,
and profile state; any capture or verification failure must prevent mutation.

Root rollback alone will not constitute Ryoku recovery. The implementation
must keep the catalog, configuration generations, and recovery journal
available after root rollback, and restore must preserve unrelated home files.
A generic recovery path must work offline and without machine-specific private
data. None of these recovery behaviors is implemented or verified by this
workspace foundation.

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
