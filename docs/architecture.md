# Architecture boundaries

The workspace separates user interface, distribution adapters, profile data,
recovery state, integration tests, and custom automation. These are Rust crate
boundaries so later issues can introduce typed contracts without coupling
privileged operations to the UI.

## Trust and write path

The UI and ordinary client code are unprivileged. A single typed backend and
recovery contract connects them to an unprivileged Rust service and a narrow,
authenticated helper. Closing the client does not cancel an accepted operation.
Authorization and verified pre-change capture are prerequisites for every
supported write. The captured checkpoint must link matching Snapper system
state with all declared Ryoku, configuration, and profile state; any capture or
verification failure prevents mutation.

Root rollback alone is not Ryoku recovery. The catalog, configuration
generations, and recovery journal remain available after root rollback, and
restore must preserve unrelated home files. A generic recovery path must work
offline and without machine-specific private data.

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
