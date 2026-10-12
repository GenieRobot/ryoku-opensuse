# Ryoku for openSUSE

Ryoku for openSUSE is a reusable RPM-based desktop for openSUSE Tumbleweed
x86_64. The approved v1.2 plan starts with Hyprland and retains KDE as a
fallback. It adapts the upstream Ryoku experience to openSUSE while keeping
distribution-specific integration in a small, reviewed port layer.

The implementation roadmap and fixed decisions live in
[issue #1](https://github.com/GenieRobot/ryoku-opensuse/issues/1). The primary
upstream project is [Ryoku](https://github.com/Ryoku-dev/ryoku), distributed
under GPL version 3. We do not vendor upstream code until its exact source
revision and component boundaries are recorded in [issue
#11](https://github.com/GenieRobot/ryoku-opensuse/issues/11); preserve the
upstream notices and licenses when importing any component.

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
