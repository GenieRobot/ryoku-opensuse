# Contribution and evidence conventions

## Pull requests

- Use one focused pull request per issue. Include the issue link and a short
  summary of behavior and affected workspace boundaries.
- State the tested toolchain and exact commands. Link successful CI checks and
  attach sanitized logs, fixtures, or VM evidence required by the issue.
- Describe recovery, authorization, and failure-path effects for any supported
  write. Desktop, reboot, or recovery claims need disposable Tumbleweed VM
  evidence; a container check or screenshot alone is insufficient.
- Retain upstream attribution and component license files. Do not add private
  companion data, machine-specific configuration, secrets, or unsanitized
  machine logs.
- Do not mark checklist items complete based on plans, stubs, mocks, or
  unverified assertions. A task closes only after its acceptance criteria and
  required evidence are reviewed on the integrated revision.

## Rust review

Use the pinned toolchain and retain `Cargo.lock`. Run formatting, Clippy with
warnings denied, locked workspace tests, and `cargo deny` dependency checks.
Prefer safe Rust; unsafe and FFI changes need a local `SAFETY` explanation and a
separate reviewer, as described in [architecture](architecture.md#unsafe-rust-and-ffi).

## RPM and workflow metadata

Keep RPM specs and CI workflow configuration declarative. Any project-owned
automation behavior belongs in a Rust workspace crate; do not hide custom
implementations in shell, workflow snippets, package scriptlets, or another
language.
