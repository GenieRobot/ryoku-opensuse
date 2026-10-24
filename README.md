# Ryoku for openSUSE

This project is building a reusable RPM-based Ryoku desktop for openSUSE
Tumbleweed x86_64. The approved v1.2 target starts with Hyprland and retains
KDE as a fallback. The port is intended to adapt the upstream Ryoku experience
to openSUSE while keeping distribution-specific integration in a small,
reviewed port layer; those product capabilities are not implemented or verified
as a complete release yet.

The implementation roadmap and current Luna coordinator handoff live in
[issue #1](https://github.com/GenieRobot/ryoku-opensuse/issues/1). See the
[integration status](docs/integration-status.md) before picking up an issue.
The primary upstream project is [Ryoku](https://github.com/Ryoku-dev/ryoku), distributed
under GPL version 3. Its pinned source revision and component boundaries are
recorded in [issue #11](https://github.com/GenieRobot/ryoku-opensuse/issues/11)
and the [upstream inventory](docs/upstream-inventory.md). Preserve upstream
notices and licenses when importing a component.
The 24 October Ryoku 1.0 source delta and openSUSE scope are tracked in
[`docs/ryoku-v1.0-delta.md`](docs/ryoku-v1.0-delta.md), with a complete path
ledger in [`upstream/ryoku-v1.0-path-delta.tsv`](upstream/ryoku-v1.0-path-delta.tsv).

The selected upstream shell includes modified iNiR-derived components:
Based on iNiR by snowarch (https://github.com/snowarch/inir).
Their original copyright, GPL version 3 license and section 7 attribution
terms remain in the payload. The openSUSE port is a modified derivative.

## Repository map

- `suse/adapters`: typed boundaries to openSUSE and desktop providers.
- `suse/recovery`: recovery state and backend contract.
- `suse/ui`: unprivileged native user interface.
- `suse/profiles`: validated generic profile model and serialization.
- `suse/tests`: contract and integration test harnesses.
- `automation`: project-owned automation implemented in Rust.
- `packaging/rpm`: declarative RPM metadata; no embedded custom shell or other
  language implementation.
- `docs`: scope, architecture boundaries, and contributor conventions.

Machine-specific configuration belongs in a separately managed private
companion and must never be committed here. Public examples and fixtures must
contain only synthetic data. Ruby bindings and the deferred expansion listed
in issue #1 are outside this repository's current implementation scope.

## Build and checks

The workspace and toolchain are pinned. From a clean checkout, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
```

The matching pull request CI workflow runs these checks. See
[`docs/contributing.md`](docs/contributing.md) for review and evidence rules.
