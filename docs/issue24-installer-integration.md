# Issue #24 integration candidate

The canonical branch is `codex/issue24-installer-integration`, resumed on main
`6117339febe0dce613cecbf304607a3ba075bcca`. The first executable candidate is
`0f2ff6f43871b5abbb5c44b9e843e79d0c0ecce9`; its exact normalized source archive
and immutable upstream/vendor inputs are staged for the dedicated native VM.
PR [#157](https://github.com/GenieRobot/ryoku-opensuse/pull/157) remains draft
and issue [#24](https://github.com/GenieRobot/ryoku-opensuse/issues/24) remains
open until native lifecycle acceptance is linked.

Local validation used Rust/Cargo 1.99.0. Formatting, strict workspace Clippy
with all targets/features, the complete workspace all-targets/all-features test
suite, offline cargo-deny and native RPM spec parsing passed. Existing ACL and
ptrace tests required execution outside the filesystem/process sandbox; the
sandbox refusals were not treated as implementation acceptance. Focused
installer tests additionally cover unsafe package payload ownership, stable
local staging, migration conflicts/symlinks/repetition and bound recovery
journal receipts. The dedicated VM runner and guest probe tests passed, and
the release runner, guest probe and initializer build successfully.

The installer source, operational commands and compatibility constraints are
in [installer-preflight.md](installer-preflight.md). Standard #21/#22 capture
and restore fixtures remain unchanged. Installer-only absent defaults require
the exact lifecycle marker plus a closed inventory with explicit absence;
ordinary checkpoints still require their existing installed package set.

The native run uses a fresh signed Snapshot20261016 derivation with KDE,
distro dependencies and the explicit recovery bootstrap. The guest builds the
coordinated release7 RPM closure with native BuildRequires checks enabled;
Quickshell remains the unchanged accepted 0.3.2-2 input. Disposable signing is
fixture trust and is not a release signature. Hyprland login/provider actions
and the combined first-sprint gate are separate acceptance work.

A first launch invocation was rejected before creating the run directory,
disks or QEMU because its cwd was the issue worktree while its requested run
path was under the parent workspace. The unchanged candidate was invoked
from the approved parent workspace. That path-policy refusal is preflight
non-acceptance and contributes no VM lifecycle evidence.
