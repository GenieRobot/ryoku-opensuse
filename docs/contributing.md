# Contribution and evidence conventions

## Upstream guidance and port requirements

Follow [Ryoku's contribution guide](https://github.com/Ryoku-dev/ryoku/blob/main/CONTRIBUTING.md)
and its linked conventions where applicable. Keep changes small and cohesive,
one concern per component, and reuse existing boundaries. Source flows from the
repository into the installed system; preserve user configuration and never
copy live tweaks back as source. UI state and decisions belong behind the
backend IPC boundary, and compositor-specific behavior belongs in its provider;
consumers ask actual capabilities rather than test compositor names.

Use `[area] scope: imperative summary` commit subjects of at most 72 characters
(`global` for shell changes), without a trailing period, em-dashes or attribution
trailers. Never bypass hooks. Update the affected area's `CHANGELOG.md`; include
a plain-language `Note: New: ...`, `Note: Fixed: ...` or `Note: Removed: ...`
trailer for a user-visible change. Keep historical commits intact.

Run applicable checks, including `qmllint` when available and real native
behavior tests for UI changes. Parsing or host tests alone do not establish
desktop acceptance. Use Prowl if available; otherwise use normal source tools
without an installation detour.

This port's Rust, declarative RPM, Tumbleweed and recovery requirements take
precedence over upstream Arch/pacman/Go deployment instructions. The approved
exception permits small, attributed QML routing/binding/result patches. Newly
authored project backend logic stays Rust; retained upstream components keep
their existing languages. Do not replace the pinned runtime or migrate its
compositor architecture merely to match newer upstream documentation. The
current ordinary-settings/protected-system distinction is recorded in
[native desktop workflows](native-desktop-workflows.md#user-authorized-recovery-scope-adjustment).

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
