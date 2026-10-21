# Luna handoff: Ryoku on openSUSE

## Current coordinator handoff: 2026-10-21

The October 18 coordinator entries below remain historical. Their repository
snapshots and next actions are superseded. See the tracked
[integration status](integration-status.md) and
[roadmap issue #1](https://github.com/GenieRobot/ryoku-opensuse/issues/1) for
the current coordination entry points.

- GitHub `origin/main` is at
  `413a1d22e517728653075d6da4f29d4d681b1120`, the merge of PR #167. At the
  last cleanup check, the coordinator's local `main` checkout was clean at
  `e025c7b8c96e55c6c0582462a1bec2f782fd1a20`, one commit behind `origin/main`.
- #15 is closed as completed M0 provider feasibility after merged PRs
  [#162](https://github.com/GenieRobot/ryoku-opensuse/pull/162) and
  [#163](https://github.com/GenieRobot/ryoku-opensuse/pull/163). The initial
  provider surface proposes zero protected writes; all writes without proven
  hooks remain unavailable. This does not satisfy the linked-recovery or
  first-sprint gates.
- #18 is closed after PR #161. #19 is closed after PR #151 at the current
  main commit. Exact-head hosted Rust workspace checks passed, and local tests,
  strict Clippy, formatting, cargo-deny and native build passed. The signed
  Tumbleweed run10 acceptance record reports exit 0 and covers supported
  layouts, stock hook-pair reuse, root/additional identities, catalog/journal
  persistence across actual root rollback, missing/corrupt/deleted-partner
  refusal, manual/known-working pins, and zero-limit cleanup.
- #19's limits remain explicit: `FullSystem` covers root and tracked `/boot`
  files but excludes the external ESP; a synthetic `Planned` journal surviving
  root rollback is not a prepared or executable linked restore; known-working
  promotion proves boot/storage state, not application health; stock hook
  telemetry proves neither a protected provider write nor veto; retention
  excludes catalog partners from automatic cleanup, with no deletion actor or
  helper restore dispatch enabled; and console lifecycle is partial without
  desktop provisioning/login.
- #17 is closed completed through PR #148 and its linked native installed
  acceptance. Its documented limits still apply: managed provider writes
  remain disabled; no RPM install or interactive PAM login is claimed.
- #23 is closed after merged PR
  [#149](https://github.com/GenieRobot/ryoku-opensuse/pull/149) at main
  `e025c7b`. The exact tested evidence head
  [`5b6627e`](https://github.com/GenieRobot/ryoku-opensuse/commit/5b6627e386e51b2b5ec6b2b614334db4ccfb1426)
  passed exact-head hosted CI plus local formatting, strict Clippy, locked
  all-feature workspace tests, cargo-deny and whitespace checks. Six native
  RPM spec builds passed with BuildRequires enabled. Ten runtime package
  manifests cover 1,607 paths with no conflicts, mutable paths or custom
  scriptlets/triggers; native Qt6 scanning found 32 import requirements with
  internal QML imports resolved. Signed Snapshot20261016 clean-target
  dependency download, dry-run, install and same-version reinstall passed.
  Installed manifests matched the artifacts, `rpm -V` was empty for all ten
  packages after both transactions, the operator group had empty membership,
  helper/user units stayed inactive, and five sentinels were unchanged. See
  the [acceptance receipt](https://github.com/GenieRobot/ryoku-opensuse/blob/5b6627e386e51b2b5ec6b2b614334db4ccfb1426/infra/tumbleweed-lab/evidence/issue23-native-acceptance-20261021.toml)
  and [native evidence directory](https://github.com/GenieRobot/ryoku-opensuse/tree/5b6627e386e51b2b5ec6b2b614334db4ccfb1426/infra/tumbleweed-lab/evidence/issue23-native-20261021).
- #23 closes packaging acceptance only: no session/login/PAM/provider
  execution, production deployment, recovery readiness, Btrfs rollback or
  first-sprint completion is claimed. Session adapters remain #25. Binary
  output reproducibility across separately provisioned SDKs and production
  release signatures are also outside this evidence.
- #20 remains open. Its draft [PR #152](https://github.com/GenieRobot/ryoku-opensuse/pull/152)
  is partial at exact head `5bb9d8bdafe7db0496024ee7a78104c68d8bbe93`, based
  on `5efc726`. Hosted formatting, Clippy, workspace tests and dependency
  review passed; the configured main-only GTK artifact job was skipped. Local
  checks passed (471 top-level tests, 17 isolated subprocess checks, 5
  existing ignored, plus a separate runtime-boundary test). Native run03
  passed the normal and post-capture-failure paths; run04 passed the tightened
  low-space check and interrupted-`Applying` recovery path with reboot history
  readback. The final same-input native normal/interrupted pair has not passed:
  run05/run06 stopped before mutation because repository-cache verification
  could not be established. The diagnostic overlay had a different SSH host
  key and zero-length cached repomd files. The issue20 fixture pin was updated
  to `32bc…` after detached signature verification; the later native cache
  mismatch remains unresolved and no further pin change was made without
  in-guest evidence. Keep PR #152 draft and #20 open until that final gate is
  reproducibly satisfied.
- Continue #20's explicit gates for durable preflight/capture/verification/
  mutation/post-capture phases; serialization and affected-writer
  coordination; plan/path revalidation before mutation; safe cancellation and
  accurate durable failure/history states; and proof that required checkpoint
  failures block mutation and concurrent operations cannot mismatch root and
  configuration generations. Do not claim multi-subvolume atomicity.
- Preserve the clean #25 worktree at `target/issue25-native-workflows`.
  Do not infer repository branch or PR counts from the historical entries below.

## Coordinator handoff: 2026-10-18

The user ended this coordinator's work and asked for a handoff. The user's
highest-priority instruction is: **do not start new issue work until the work
already accumulated in the integration checkout has been integrated and
cleaned up.** Coordinate one issue per worker and one focused branch/worktree
per issue; workers test, correct, commit and publish their issue work; the
coordinator integrates only completed work in dependency order, closes issues
only after linked acceptance evidence, then cleans branches/worktrees. Reuse
the existing Tumbleweed VM; do not commission another VM by default.

### Repository state

- Main checkout: `/home/jin/src/ryoku-opensuse`, branch
  `codex/ryoku-integration`, HEAD `a351f64e6d9968eeb3428e4c520ddaabb77b6be1`.
- This worktree remains dirty: **89 tracked files modified, 48 untracked
  paths**. `git diff --stat` reports 17,944 insertions and 1,131 deletions
  across the tracked files. Changes span multiple issues and must not be
  bulk-committed as one change. No commit or push of this dirty set has been
  made. `LUNA_HANDOFF.md` itself is among the untracked files.
- Local `main` and `origin/main` are synchronized at
  `6c10d9f5075f6e6d3c084f74517c9f6c236a22e6` (merge of PR #150, issue #18).
- Local branch refs were reduced from 49 to 9. Forty refs were deleted only
  after confirming every commit patch was represented in the integration
  branch. The clean duplicate #13 worktree was removed after its patch ID was
  verified identical to a commit already in the integration branch. These
  actions did not commit or discard the dirty integration changes.
- Five worktrees remain:
  - `/home/jin/src/ryoku-opensuse` — dirty coordinator integration checkout.
  - `/home/jin/src/ryoku-opensuse/target/issue25-native-workflows` — clean,
    unique #25 work; preserve.
  - `/home/jin/src/ryoku-opensuse/target/recovery-metadata-worktree` — dirty
    (12 paths), unique branch/history; preserve and inspect before any cleanup.
  - `/tmp/ryoku-issue22-recovery-gate` — dirty (2 paths), #22; preserve.
  - `/tmp/ryoku-issue23-install` — clean, #23; preserve.
- The remote still has roughly 63 branches and the repository has 41 open
  issues. GitHub shell pushes/deletes fail because HTTPS credentials are
  unavailable (`could not read Username`). The connected GitHub tools can
  update refs and PRs/issues but expose no branch-delete operation. Do not
  claim remote branches have been cleaned up.

### Open PR and preserved VM

- The only open PR observed is draft [#149, issue #23](https://github.com/GenieRobot/ryoku-opensuse/pull/149),
  remote head `bbfa3196a12372d165bae890c9f639d296d93c41`. It is incomplete and
  must remain draft/open until RPM builds, package manifests, clean-target
  dependency/install/reinstall, override preservation, and sysusers evidence
  pass and are linked to the issue.
- #23 host checks, defaults/helper/tools native RPM/SRPM builds, and a signed
  Quickshell dependency preview passed. GTK/QML packaging work was explicitly
  paused by the user before further changes/builds. The preview found embedded
  Quickshell and private `shell.services`/`shell.barkit` imports that need
  precise QML metadata/provides handling; do not disable broad QML scanning.
- Preserved run/VM: `/home/jin/src/ryoku-opensuse/target/issue23-packaging-20261016`
  (existing qcow2 about 6.56 GB). Latest worker verified QEMU is live and idle;
  QMP works and SSH is `tester@127.0.0.1:53409`. Supervisor session `97754` is
  inaccessible from that worker, but no replacement VM is needed. Preserve the
  VM, SDK/cache `/home/tester/issue23-buildroot`, logs under
  `target/issue23-integrated-build`, and existing PR branch.
- Issue #23 is open; no runtime package was installed and no issue closure is
  claimed.

### Work interrupted by the user's sequencing correction

- A new #19 worker was started before the dirty integration work was resolved.
  The user corrected this. The worker was interrupted before making changes;
  its clean worktree was removed and the remote `codex/issue19-durable-catalog`
  ref was restored to its previous head `51d0359de3863ddf9ca0e5d5858703472910c9a1`.
  Do not resume #19 or start other issue implementation until the current
  accumulated work has been inventoried, tested, committed/published in focused
  issue slices, and integrated.
- PR #139 is already merged as a partial #19 implementation. Issue #19 remains
  open; its description says layout/additional-snapshot discovery,
  package-hook pair correlation, catalog/journal survival across actual root
  rollback, and manual/known-working retention ownership still need evidence.
- PR #150 is merged on `main`; it contains #18 capture work. Review comments on
  that merged PR included three P1 findings; recheck their disposition before
  relying on the merge as complete acceptance. The merge itself is not evidence
  for other open issues.

### Required next coordinator action

Do not start another issue implementation. Inventory the exact 89 modified
files and 48 untracked paths in the integration checkout, map each hunk/artifact
to its issue and existing test/evidence, then process the already-present work
in dependency order. Keep each issue slice on its focused worker branch, run
tests against the exact committed tree, publish it, review/integrate it, link
acceptance evidence, and only then remove duplicate source changes from the
integration checkout. Preserve all unique or dirty worktrees and the existing
VM. Do not bulk-commit this mixed tree or close an issue based on host-only or
historical evidence.

## Continuation update: 2026-10-18 (per-wall Stage preferences)

The pinned Stage provider persists per-wall effect/layer settings in the fixed
`$XDG_STATE_HOME/ryoku/stage-walls.json` registry. Added it as a bounded opaque
account-owned preference. The session inventory does not recurse into generated
Stage output or progress logs and does not follow wallpapers/source assets
referenced by the registry. The capture fixture and restore-policy test both
pass (1/1 each); formatting and diff checks pass. Evidence and pinned writer
hash are recorded in
`infra/tumbleweed-lab/evidence/issue18-ryoshot-capture-oct18.toml`. Host-only;
native capture/restore remains unverified.

The separate home-root restore fixture now stages a deliberately corrupted
`stage-walls.json`, replays the saved generation, and verifies exact restored
bytes and mode `0600` while preserving an unrelated home sentinel. The focused
configuration-apply test passes (1/1); source hashes and output are recorded in
the same #18 evidence TOML. This exercises the restore engine locally, not the
native console workflow.

## Continuation update: 2026-10-18 (device preference inventory audit)

Continued the pinned upstream persistence audit beyond QML and found five
additional first-sprint user preferences: the raw backlight level,
`touchpad.disabled`, and night-light temperature, enabled marker, and schedule.
Added these as exact bounded regular-file records under the resolved account
state root; no directory traversal or new privileged writer is permitted.
Capture and restore allowlisting cover the same compiled path/kind inventory.
The focused capture fixture and recovery CLI allowlist test pass (1/1 each),
and formatting/diff checks pass. Source hashes for the four pinned device
writer files plus capture/restore code are recorded in
`infra/tumbleweed-lab/evidence/issue18-ryoshot-capture-oct18.toml`. This is
host-only evidence; native metadata and restore validation remain outstanding.

## Continuation update: 2026-10-18 (failed session activation cleanup)

Hardened the `ryoku-session activate` failure path. When the Hyprland
`exec-once` activation fails, the wrapper now stops the Ryoku session target,
clears the named D-Bus/systemd activation environment, and requests compositor
exit so SDDM can recover. It first requires a Hyprland instance signature, so
invoking the command outside the compositor does not tear down a session.
Documented this in `docs/session-port.md` and the Unreleased changelog.
`cargo test --locked --offline -p ryoku-session --bin ryoku-session` passes
(12/12), including a direct precondition test for missing and empty compositor
signatures; formatting and `git diff --check` pass. Native lifecycle validation
remains open.

The user has authorized one fresh disposable QEMU run. Repeated
`vm-lab preflight` using the designated `/workspace/opensuse-vm` asset path; it
failed solely because that directory does not exist. No run directory, VM,
disk, or key was created, and the retained development disk was not touched.
To unblock: make the exact signed bundle available in an accessible directory
containing the 20261013 qcow2, `image.sha256` with its original signed image
filename, `image.sha256.asc`, and isolated `gnupg/` containing the pinned
openSUSE project key (fingerprint
`AD485664E901B867051AB15F35A2F86E29B700A4`). The WSL asset argument can point
to another accessible directory if `/workspace/opensuse-vm` is not mounted;
the runner verifies the signature and pinned image hash before creating a VM.
Do not substitute Snapshot20261014.

## Continuation update: 2026-10-18 (power preference capture and restore policy)

Audited the pinned Ryoku daemon's persistent writes and found that
`power-profile.json` and `power.json` were absent from the session-preference
inventory. Added both as bounded opaque account-owned config records. The
captured contents preserve the selected power-profile name and user CPU/battery
definitions, but do not authorize applying hardware settings. Also removed a
duplicate restore-preflight allowlist: recovery now validates path and record
kind against the same compiled `InstalledInventory::session_preferences()`
used by capture. This fixes earlier declared preferences such as Ryoshot and
device vibrance being accepted for capture but rejected by restore.
The focused capture fixture passes (1/1), as does the restore allowlist test
(1/1); formatting and `git diff --check` pass. Evidence and source hashes are
in `infra/tumbleweed-lab/evidence/issue18-ryoshot-capture-oct18.toml`. Host-only
coverage; native Tumbleweed restore remains open.

## Continuation update: 2026-10-18 (legacy preference import and live image index)

The per-user `MigrateLegacyLocalProfile` request now reaches the profile
crate's exact-canonical-schema importer. It writes only the fixed
`opensuse-profile.json` destination through the authenticated unprivileged
service, preserves the old `profile.json`, never replaces an existing
destination, and uses neither the root helper nor a machine checkpoint. An
identical retry succeeds only if the validated destination matches the
canonical source; conflicting targets remain refused. Missing roots return
`NotFound`; unknown legacy formats return `Unsupported`. The migration test
also confirms it adds no operation-journal record. Tests passed for
`ryoku-service` (130 passed, 1 ignored), `ryoku-profiles` (20 passed), and the
`ryoku-profile` CLI (5 passed); Clippy with `-D warnings`, formatting, and
`git diff --check` passed. Details and source hashes are in
`infra/tumbleweed-lab/evidence/issue17-service-boundary-host-oct18.toml`.
After making exact-target retries idempotent, the full workspace suite also
passed: 667 passed, 0 failed, 10 ignored across 32 result rows. Its quiet log
SHA-256 is `6c3dad0912294adf52623e43b74629dffd4b9efe47d833fbc1a8b064efc2ef09`;
see `infra/tumbleweed-lab/evidence/issue1-workspace-tests-oct18.toml`.
This closes only the local legacy preference import gap; issue #17's protected
operation integration and native acceptance remain open.

The current environment has 9,243,234,304 bytes available memory, above the
runner's 7,516,192,768-byte threshold, and the repository target filesystem has
ample free space. `/workspace/opensuse-vm` is absent. The current live official
factory appliance index lists Snapshot20261014 artifacts and no
Snapshot20261013 entries, while cached search/mirrorlist results for 20261013
are stale; exact sidecar and mirrorlist reads through the browser fetcher also
returned cache-miss errors. A follow-up web search still returned cached
Snapshot20261013 names and hashes, but reopening the live index and searching
its contents confirmed that only the 20261014 snapshot is currently listed.
Treat search snippets as stale metadata, not as evidence that the old bundle
can be retrieved. Evidence is in
`infra/tumbleweed-lab/evidence/issue26-authorized-fresh-preflight-oct18.toml`.
Do not substitute Snapshot20261014: the WSL fixture and prior native records
are pinned to the signed Snapshot20261013 hash. No QEMU process, guest, disk,
key, or new run directory exists. The user's authorization for one fresh
disposable run remains active once the exact signed bundle is supplied.

## Continuation update: 2026-10-18 (fresh-run authorization and preflight)

The user explicitly authorized one fresh disposable QEMU run in this Codex
thread, superseding the earlier no-launch instruction in this handoff. No VM,
disk, key, or run directory was created: the WSL runner preflight failed before
creation because the pinned asset directory and linked-checkpoint probe binary
were absent, and available memory was 6,204,563,456 bytes versus the required
7,516,192,768 bytes. Do not bypass these guards or reuse
`target/ryoku-development`; its retained disk remains untouched. Re-run only
after the exact signed seed assets are available and host memory meets the
runner requirement.

The linked-checkpoint probe and recovery CLI binaries have since been built.
The pinned asset bundle remains absent, and latest WSL available memory was
6,332,403,712 bytes, still below the runner requirement.

After the user explicitly allowed a fresh disposable QEMU run, the exact WSL
`vm-lab preflight` was repeated on 2026-10-18 with a fresh `/tmp` run path, then
with a fresh path on the repo's spacious `target` filesystem. Both failed
before creating a run directory because `/workspace/opensuse-vm` was absent
and available memory was respectively 6,359,871,488 and 6,321,324,032 bytes
versus 7,516,192,768 required. The first attempt also failed `/tmp` space
(298,414,080 bytes versus 12,884,901,888 required); the second passed the run
space check with 977,646,354,432 bytes available. The retained
`target/ryoku-development` disk was not touched. The sanitized results are recorded in
`infra/tumbleweed-lab/evidence/issue26-authorized-fresh-preflight-oct18.toml`;
it establishes no guest prerequisite or VM behavior.

Selected temporary Cargo target directories containing only redundant focused
build outputs were removed from `/tmp` after confirming no active Cargo process
or source checkout used them; the full-workspace cache, all source worktrees
and repository `target` data were preserved. Available RAM rose above the
runner requirement. The full console recovery preflight with the built state
initializer, linked-checkpoint probe and recovery CLI then failed only because
the image, checksum and detached signature were absent. Attempts to fetch the
exact pinned files from the documented openSUSE endpoint and the official-listed
UMD mirror returned HTTP 404. No substitute image was used, and no guest was
created. This result is in the same evidence record.

Issue #1 defines the first-sprint deliverable as clean Tumbleweed install,
login, reboot, KDE fallback, reinstall preserving overrides, and verified
linked checkpoint plus offline console restore. Issues #22 and #26 remain open
because those native VM acceptance gates have not passed. The latest recovery
library run passes 226 tests with 7 ignored. It includes simulated mid-payload
ENOSPC refusal and a controlled concurrent-file-change capture refusal; neither
is a replacement for the destructive disposable-VM failure/restore run.
The linked-checkpoint VM probe now also removes/restores its exact transaction
catalog link, verifies that the moved and restored entry retains the original
device/inode identity, and requires a typed refusal marker for the missing
partner. The host marker validator test passes; native execution is still
pending.

The probe now checks the device/inode pair after moving the catalog link aside
and after restoring it, binding the reported refusal to the exact original
entry. `cargo check --locked --offline -p ryoku-recovery --example
native_linked_checkpoint_probe`, `cargo fmt --all -- --check`, and `git diff
--check` passed after the change. The native guest probe has not run.
`cargo check --locked --offline --workspace` also passed on the current dirty
worktree; this is compilation evidence only and does not validate VM behavior.
The current dirty-worktree `cargo test --locked --offline --workspace` run also
passed (exit 0); terminal output truncation prevented retaining a reliable
aggregate count. Updated the issue #1 workspace evidence without changing the
older run's count or digest.

The #22 failure probe now also verifies that the original real Snapper snapshot
and generation directories retain their device/inode identity after the missing
partner scenarios restore them. This extends source-level fixture-integrity
checks only; no native guest execution is claimed.

## Continuation update: 2026-10-18 (current guest process check)

The current workspace environment has no process named `qemu-system*` or
`vm-lab` (`ps -eo pid=,comm=` check). The retained-VM PIDs, SSH details and
“Live VM” statements below are historical records, not evidence of a currently
running guest or usable QMP/SSH session. The user later authorized one fresh
disposable QEMU run in this thread, superseding the earlier no-launch direction.
That run remains gated on obtaining and verifying the exact pinned signed image;
do not resume the historical VM or reuse its retained disk. The clean-VM and
native workflow gates remain outstanding, and the historical trial is not
current acceptance.

## Continuation update: 2026-10-18

The coordinator resumed the roadmap in this thread and reread GitHub issue #1
and the task issues. Issue #14's native KDE launcher request/result/error
component proof is recorded; its broader accessibility, Hyprland, packaging,
and architecture acceptance remains open. Issues #22 and #24–#26 still require
their recovery and clean-VM evidence. At that point no VM-based acceptance had
been added in this continuation; the later user authorization and current
asset-fetch attempts are recorded above.

The configuration restore writer's SIGKILL replay test passed at all six
durable apply boundaries:
`cargo test --locked --offline -p ryoku-recovery --lib
sigkill_after_each_durable_apply_boundary_replays_the_same_checkpoint --
--nocapture`. The parent reopens the journal after each killed child, completes
replay and checks the restored fixture. This is userspace process interruption
evidence only; it does not replace the linked Tumbleweed restore/reboot gate.

Added restore-journal child-process interruption coverage across all durable
phases in configuration-only, system-only, and complete-linked scopes. The
parent reopens and discovers every pending phase, then resumes journal
transitions to `Complete`. The synthetic root-rollback receipt tests journal
state only. The focused test
`cargo test --locked --offline -p ryoku-recovery --lib
process_exit_after_every_scope_phase_is_discoverable_and_resumable` passed.

Fixed an issue #22 lab-report omission: the linked-checkpoint probe validates
corrupt/symlink/traversal manifests, payload corruption, artifact ownership and
group metadata, and capture failure, but `public_evidence` previously dropped
several corresponding fixed markers. The allowlist now preserves all probe
markers under the existing safe key/value filter. The focused evidence filter
test passed with `cargo test --locked --offline --package ryoku-automation
artifacts_reject_unknown_identity_and_network_fields`; this changes sanitized
report completeness only and is not a new native recovery result.

A marker audit found two further sanitized results filtered out of the issue
#22 report: changed-root historical read-back and the boot-chain-observed
summary. Both fixed keys are now admitted. Raw boot facts and exact snapshot
IDs remain private. The focused evidence-filter and all four linked-checkpoint
marker tests pass; this adds no new boot or recovery acceptance result.

The UI service client now uses one five-second deadline from Unix socket
connect through request/response framing. Connect is nonblocking and waits for
completion with `poll`; it retains runtime directory, socket metadata and peer
UID checks. Test-only exchange wrappers are compiled only for tests. Validation
performed: `cargo fmt --package ryoku-ui -- --check`,
`cargo check --locked --offline -p ryoku-ui`, and `git diff --check` all passed.
`cargo check --locked --offline --workspace` also passed. Correction: the
earlier focused UI slow-peer test reported success without asserting that the
request reached its peer. After adding that assertion, this managed sandbox
returned `EPERM` on a UnixStream pair write. The test now logs and skips only
that denied transport case; it must run without a skip in native CI. The
saturated-backlog and VM tests remain outstanding.
The pinned ordinary-settings backend archive still matches its recorded
SHA-256, and `rpmspec --parse packaging/rpm/ryoku-desktop.spec` succeeds.
This checks package metadata only; the Go build and native package regression
were not run in this continuation.

Focused installer preflight coverage also passed:
`cargo test --locked --offline -p ryoku-recovery --bin ryoku-install`
(8 tests). This covers host/version parsing, bounded stable package staging,
symlink refusal and read-only legacy profile inspection; it does not exercise a
Tumbleweed host or authorize package mutation.
The offline recovery CLI's focused unit suite also passed:
`cargo test --locked --offline -p ryoku-recovery --bin ryoku-recovery`
(26 tests). These cover argument gates, writer shutdown/leases, recovery-mode
checks and safe preview output; they do not replace the offline Tumbleweed
restore and interruption exercise required by issues #21/#22.

The session recovery gate now uses nonblocking connect/poll and one five-second
deadline through request and response I/O; UI service exchange also uses
nonblocking I/O and `poll` for the same end-to-end deadline. The recovery gate
expired-deadline unit test passes. Slow-peer integration tests for both paths
explicitly skip in this sandbox after their UnixStream write probe is denied
with `EPERM`; their actual transport assertions still need native CI/runtime
execution. The privileged recovery-gate server also switched from fixed socket
timeouts and `read_until` to nonblocking bounded request parsing and response
writes under one deadline. Its five focused protocol/deadline tests pass, but
the trickled-request transport test skips after the same explicit EPERM probe;
the session expiry test passes. Workspace compilation and formatting passed
after this change; no QEMU was launched.

The main per-user service handler also now reads a bounded request frame and
writes its response with nonblocking I/O under one absolute five-second
deadline. A focused test verifies expired deadlines fail before socket reads or
writes; that test and the workspace check passed. Live slow-client behavior is
still unverified in the managed sandbox because UnixStream writes return
`EPERM`; the issue 17 native service acceptance remains open.

The privileged helper socket handlers and both helper clients now use
nonblocking connect/read/write with one absolute five-second deadline from
connect through the complete exchange. Framing stays bounded at 64 KiB, and
peer credential checks still precede authorized request parsing. Updated
`docs/service-contract.md` to describe the end-to-end bound. Validation in this
continuation: `cargo fmt --all -- --check`,
`cargo check --locked --offline --workspace`, and `git diff --check` passed.
No helper transport tests or VM acceptance were run; the managed sandbox's
UnixStream `EPERM` limitation still applies, and do not launch QEMU.

Added a focused pure-deadline regression test for the helper transport. It
confirms an expired deadline returns before socket reads or writes; the named
test passed with `cargo test --locked --offline -p ryoku-service --lib
helper_protocol::tests::expired_helper_deadline_fails_before_socket_io`.
Workspace compilation and `git diff --check` passed afterward. The test does
not exercise a slow peer; native transport acceptance remains open.

Closed a documented #16 state-mount validation gap. The collector now requires
`nodev,nosuid,noexec`, rejects contradictory `dev,suid,exec`, and requires the
live mountinfo `subvolid` to equal the machine-record subvolume ID. It requires
a nonempty kernel source field and binds the actual source filesystem through
the Btrfs UUID ioctl to the recorded UUID; it deliberately does not persist an
unstable device-node spelling. All 14 `state_mount::tests` passed. Exact
Tumbleweed mountinfo representation remains unverified, and no VM was launched.

Ran the repository's Clippy gate and fixed the findings in touched Rust paths:
collapsed nested conditions in the capture utility, used direct errno patterns
for nonblocking connect, and grouped the linked-checkpoint corruption probe's
parameters into a named struct. `cargo clippy --locked --offline --workspace
--all-targets -- -D warnings` now passes. `cargo deny check` could not acquire
its advisory-database lock because the configured Cargo cache is read-only. I
copied the existing advisory database into `/tmp/ryoku-cargo-home` and ran
`CARGO_HOME=/tmp/ryoku-cargo-home /home/jin/.cargo/bin/cargo-deny --offline
--manifest-path Cargo.toml check`; advisories, bans, licenses, and sources all
passed without network access. No VM or slow-peer acceptance was run.

Updated 2026-10-18. The user has resumed the issue-driven implementation in
this Codex thread. Continue under the decisions and constraints below. At the
latest local inspection, no retained QEMU or `vm-lab` process was running, and
the pinned image asset directory was not present under the repository or
`/tmp`. The handoff still prohibits launching another QEMU; VM acceptance is
therefore outstanding. The prior issue-25 worker was interrupted; no worker
processes are assumed to remain active.

## User priorities and decisions

- Deliver the actual integrated Ryoku desktop. Preserve the original experience
  and make the least necessary changes. Do not substitute diagnostic windows.
- Reread [issue 1](https://github.com/GenieRobot/ryoku-opensuse/issues/1): first
  finish M0 + essential M1 + M2, including package lifecycle and offline paired
  restore. Do not expand supporting infrastructure indefinitely.
- Ordinary user settings must work without requiring a machine checkpoint or
  elevation for every interaction. This explicitly narrows the original broad
  checkpoint rule. Keep persistent user configuration in recovery inventory.
- Privileged package, system-service, account, security and systemwide changes
  still require the protected Rust authorization/recovery path.
- Small documented QML routing changes are permitted. New project executable
  logic remains Rust; unchanged upstream components may retain their languages.
  No direct user prohibition against retaining unchanged upstream Go was found.
- Follow upstream CONTRIBUTING where applicable. Applicable guidance and port
  exceptions are now in docs/contributing.md. Do not change the upstream pin
  just to follow newer upstream architecture. Never bypass hooks.
- Delegate actual work by issue; coordinator validates/integrates. Runtime has
  four simultaneous slots including coordinator, despite permission to use 15.
- No host package/service/account changes. Existing disposable guest installs
  were authorized. Coordinator alone has performed VM actions.
- Conserve storage and credits. Do not recreate the VM, repeat passed builds,
  introduce replacement frameworks unnecessarily, or cargo-clean ROOT/target.

## Source state

ROOT = /home/jin/src/ryoku-opensuse

| Checkout | Branch / HEAD | State |
| --- | --- | --- |
| ROOT | codex/ryoku-integration / a351f64e6d9968eeb3428e4c520ddaabb77b6be1 | Clean, behind current local integration |
| ROOT/target/recovery-metadata-worktree | codex/offline-recovery-integration / 9cf530e1ae7cd69ebe270bdd729038ccfe3e8123 | Clean, authoritative integrated work; ahead 153 / behind 2 relative to tracked origin/main |
| ROOT/target/issue25-native-workflows | codex/issue25-native-workflows / 27ad02c0ea96b19deace56bca1d08caaeb8e7a85 | Clean; contains additional unintegrated candidate work |

The integration branch contains the retained-resume tool, notification route,
ordinary-settings scope correction, upstream contribution guidance, bounded
keyboard/pointer test tools, native RPM lifecycle evidence and opt-in Go trial
metadata. It is not yet reconciled with the two newer public-main commits.

Public PR145 was already published and attached:
https://github.com/GenieRobot/ryoku-opensuse/pull/145
Its exact approved provider patch is remote commit
7cb3d226c30f89521a23c5c042130503215745c5. Do not republish it.
PR139 remains the separate recovery verification draft, head
6949192a99432e8e6e7798513756fa92d66481e6. It does not implement full root rollback.

## Live VM and access

- Retained directory: ROOT/target/ryoku-development. Standalone disk.qcow2;
  no backing image. Existing disk/firmware/keys were resumed, not reprovisioned.
- Supervisor PID 14647; QEMU PID 14670; KVM verified. Treat these as historical
  observations and revalidate process start ticks before controlling anything.
- Fresh record: development-desktop-private.txt in that directory. SSH port
  54083, user tester, host 127.0.0.1. Read the record rather than assuming this
  port persists after a future resume. tester has guest-only passwordless sudo.
- SSH key id_ed25519 and known_hosts are in that same private directory. Keep
  StrictHostKeyChecking=yes, BatchMode=yes and the explicit pinned known_hosts.
  Never copy keys into commits or ask the user to disclose passwords.
- Desktop account ryoku-desktop UID1001/GID1002. User bus /run/user/1001/bus.
  Execute GUI commands through its systemd user manager to inherit Wayland env.
- Retained supervisor has a 24-hour hold from resume; its log is
  ROOT/target/ryoku-development-resume-oct18.log. Do not launch another QEMU.
- Original running supervisor executable:
  /tmp/ryoku-issue13-retained-resume/target/debug/vm-lab. Do not overwrite it.
- New validated host control tool: /tmp/ryoku-input-build/debug/vm-lab.
  Prefix controls with RYOKU_LAB_AUTHORIZED=existing-wsl-vm.
  Supports development-capture RUN, development-input RUN --chord super+n
  (also super+space/super+escape), and --click X,Y. Current VM absolute pointer
  works. Ownership/start-tick/QMP checks remain enforced.
- Captures overwrite only development-current.ppm. Rust converter
  /tmp/ryoku-ppm-to-png INPUT NEW_OUTPUT.png preserves RGB; output must be new.
- Guest /tmp/ryoku-vm-lab is a verified root-owned temporary login helper;
  normal reboot clears it. Stable upload SHA:
  979ab23b49e2e5a890d3ff687ec9443f6319d23262fdf5a75036b0b441ca0480.
- IMPORTANT: development-login --session ryoku expects the service MainPID to
  be Quickshell. The current Go trial deliberately makes it the Go supervisor.
  That old inspector therefore cannot certify this trial. Its host process
  35091 was terminated after real SDDM login succeeded, releasing QMP. Do not
  repeat its polling timeout or claim it passed for Go. Inspect logind, the
  service cgroup, child QS and actual display instead. Never open a second QMP
  connection while a login controller holds the first.

## Native RPM results: completed

Exact tested package source: 9d66fca12561e97302f99427112223e98785bba4.
Source0 SHA256:
8966ddc5a8f923aa1f52cc35dd1b55325bfcd17d1086c904f4715e20f94b6236.
Four serial native builds passed: defaults, helper, desktop, tools. Nine
Release-2 runtime RPMs plus unchanged Quickshell 0.3.2-1 were audited. Four new
SRPMs, exact source/vendor closure, runtime/fonts, four Blobs files, dependencies,
ownership and absence of custom package hooks all passed independent review.

- Normal signed-package installation passed, including real file-conflict checks.
- Native systemd trigger created previously absent ryoku-operators, GID471.
- rpm -V all ten packages passed. Package-owned normal SDDM Ryoku login passed.
- Same-version forced reinstall passed; complete desktop-home comparison passed.
- Removal of exactly ten Ryoku packages passed; complete home remained identical.
- KDE packages survived. Normal KDE SDDM login and actual desktop frame passed
  while Ryoku was uninstalled. Ryoku was then reinstalled normally.
- Normal reboot changed the boot ID; all package integrity checks passed; fresh
  ordinary SDDM Ryoku login passed. Installed system fonts appeared in fc-list.
- Real notification receipt, history through actual Super+N, and actual card
  dismiss passed after reboot. Monitor recorded NotificationClosed ID1/reason2.

This was an SDK-equipped retained guest, NOT a clean product-installer gate.
Issues 23/25/26 remain open; no full recovery acceptance is established.

Artifacts: ROOT/target/issue23-install-native-9d66fca. Contains unsigned builds,
separate SIGNED-RPMS, public test key, native build/install/reinstall/removal
logs and review/. The first unsigned install correctly failed with no changes.
Separate copies were signed with disposable one-day guest-only test key
740A21315108368EF18E1E8032F57A9A1695A78C. All signed/unsigned payload/header
identities match. No GPG bypass was used; this is not release signing trust.
The private test key remains only in the guest build directory, never copied.
Canonical sanitized evidence and manifests are committed under
infra/tumbleweed-lab/evidence/issue23-install-*.

Guest build area: /home/tester/ryoku-rpmbuild-install. Do not rebuild the old
77f42c5 source cut; it is intentionally preserved and unbuilt. Latest kit is
ROOT/target/issue23-install-candidate-9d66fca. Reuse existing audited artifacts.

Private guest backups: /root/ryoku-rpm-prototype-phase2. Includes metadata-
preserving prototype.tar, exact eleven original objects under files/, whole
desktop-home.tar, desktop-home-before-reinstall.tar and
desktop-home-before-upstream.tar. Archive comparisons passed before relevant
mutations. Root Snapper pre27 precedes package transition; pre28 precedes Go
trial. These are manual lab backups, not the product's verified linked recovery.
Do not restore prototype objects over installed package-owned files.

## Current visible desktop: unchanged upstream backend trial

The user's theme complaint was correct: bar coral and center green came from
different built-in fallbacks, both original Ryoku QML in the SAME QS process.
The shared colors.json and themePalette were absent because the original
ordinary-settings backend was missing. Reusing that backend fixed the actual
palette connection without recoloring QML or creating a Rust replacement.

Source pin: 98a0368253d9628be72b853985878a5db90e1547.
479 unchanged source/vendor files were checked against pinned Git blob hashes.
Host source kit:
ROOT/target/issue25-native-workflows/target/upstream-daemon/ryoku-shell-ipc-98a0368-source.tar.gz
SHA256 5bbc37c047becef72b9f62793a28abedb6147540e616624bd4bc21386a657610.
Guest source/build: /home/tester/ryoku-upstream-trial.
Built offline using distro Go1.27.2, -mod=vendor, CGO_ENABLED=0,
GOTOOLCHAIN=local, GOPROXY=off, GOSUMDB=off, GOWORK=off, -trimpath,
-buildvcs=false. Native signed packages installed for this trial: go, go1.27,
wl-clipboard. Matugen is unavailable in the distro repository and unnecessary
for the static palette proof. No host installs occurred.

Running binary: /usr/libexec/ryoku/upstream-trial/bin/ryoku-shell (root-owned).
SHA256 c7d4dcf81751918eebd40c492fb1d1fc46e69cdf47d77690b231113c4b596019.
Last observed daemon5068, QS5094, wl-paste watcher5089, Hyprland5011.
Socket /run/user/1001/ryoku-shell.sock is desktop-user-owned mode0600.
Original Rust ryoku-service remains separate and retains protected operations.

Opt-in user drop-in:
/home/ryoku-desktop/.config/systemd/user/ryoku-desktop-shell.service.d/90-upstream-trial.conf
Source: infra/tumbleweed-lab/candidates/upstream-daemon/90-upstream-trial.conf.
It replaces only ExecStart in the existing unit, preserves target/cgroup
lifecycle, exact QML environment/private QS path, normal user and NNP, and
unsets the opt-in upstream polkit agent. Native systemd verification passed.
The first draft's misspelled condition was corrected to ConditionFileIsExecutable
and redeployed. Do not confuse those old journal warnings with the final unit.
This user override PERSISTS across reboot until explicitly removed.

Actual results:
- Existing command `ryoku-shell theme "Tokyo Night"` returned0 and generated
  shared colors.json. Parent inspected the real bar AND notification center:
  matching Tokyo Night palette, actual notification content, no new theme QML.
- wl-copy foreground test unit ryoku-clipboard-proof holds synthetic text
  "Ryoku clipboard proof on openSUSE". Native wl-paste returned that exact text.
  Original Ryoku clipboard UI visibly showed its real captured history entry.
- Clipboard history copy-back, star/delete and persistence are NOT yet proven.
- The pending guest network-manager query was rejected by automatic approval
  review after the user's STOP. It did not run. Do not assume NM is installed.
- PowerProfiles D-Bus name was not activatable; no hardware profile claim.

Best frames in ROOT/target/ryoku-development:
- ryoku-shared-theme-center-oct18.png: matching palette, latest theme proof.
- ryoku-clipboard-first-oct18.png: real clipboard entry, matching palette.
- ryoku-rpm-postreboot-super-n-oct18.png and ryoku-rpm-dismissed-oct18.png.
- kde-after-rpm-removal-oct18.png.

Trial rollback is documented in candidates/upstream-daemon/README.md. Stop the
same shell unit, remove only the opt-in drop-in, daemon-reload and restore scoped
captured personal settings/dconf/absence state before restarting the RPM unit.
Do not overwrite unrelated user changes. The staged Go binary becomes inert.
The foreground clipboard proof unit is tied to the desktop target; stop it after
its tests if no longer needed. No original Hub/WM provider/privileged helpers
were installed. Do not launch a second supervisor; Go reaps competing shells.

## Remaining work and efficient next steps when resumed

1. Validate clipboard copy-back through original UI using two synthetic entries.
   Read actual NetworkManager state before trusting the network indicator. The
   original provider can show Disconnected on missing NM without an explicit
   unavailable marker. Do not mistake that for working networking integration.
2. Repeat the theme and ordinary-preference flows through the RPM-owned
   `ryoku-shell` binary after install/reinstall, verify its digest against the
   installed package payload, then retire the trial-only paths/override. The
   pinned unchanged backend is already built into `ryoku-desktop` from the
   checksummed source archive and user unit; the issue-25 trial currently uses
   a separately built binary. Keep new protected openSUSE logic Rust. Do NOT
   ship redundant Rust NM polling bridge
   8d7122e alongside the original provider; it is an unintegrated earlier candidate.
3. Original WM provider writes Lua settings while current supported Hyprland
   0.56.2 session uses .conf. Do not enable that writer without compatibility
   proof. Static theme/clipboard/NM do not require it. Missing caps must be honest.
4. Original whole Hub has privileged Arch/pkexec/service/SDDM/hardware paths.
   It cannot be enabled unrestricted as the protected backend. Route those
   operations to Rust with small UI changes; retain ordinary controls.
5. Finish M1 complete-state capture, authenticated execution, linked offline
   restore and changed-root rollback, then the protected installer/M2 gate.
   Current helper remains deny-all; prior recovery CLI proof is read-only.
   The active worktree now also has a root-only offline command to discard an
   exact incomplete stage and retries `prepare-configuration`/`prepare-linked`
   against the same verified checkpoint after journaled preparation exits.
   CLI/catalog reconciliation has local test coverage only; no Tumbleweed
   recovery exercise has passed.
6. Remaining desktop gaps include lock/authentication, screen sharing, capture,
   audio/device tests, WM integration, dynamic wallpaper/app theming. Static
   palette proof does not establish these. Later package/update/admin/release
   milestones remain substantially incomplete. Do not claim a finished product.

Avoid rebuilding unchanged Quickshell or repeating lifecycle tests without a
new affected change. Use separate Cargo target directories per worktree: a
shared target previously reused stale crate artifacts. ROOT/target contains
source worktrees, so NEVER cargo-clean it. Longer historical state lives in
ROOT/target/live-desktop-state.txt, but this handoff is the current starting point.

## Latest resumed-work evidence

- Issue #24 now has a Rust `ryoku-install preview` binary in the
  `ryoku-tools-state-init` package. It requires Tumbleweed x86_64, full
  root/boot Snapper coverage, initialized protected state, a valid generation
  store, and no unfinished restore. It accepts the fixed seven-package local
  RPM set plus optional GTK/launcher tools, checks RPM names/NEVRAs/architectures
  and `/usr`-only direct payload paths, prints every direct payload and `%config`
  path, and emits a no-refresh Zypper dry-run with default GPG verification.
  It also rejects direct RPM scriptlets, triggers and file triggers. RPM/Zypper
  output is bounded under fixed timeouts. Source compiles; `rpmspec --parse`
  succeeds. It inventories installed Ryoku RPMs and compares each requested
  target NEVRA; unknown `ryoku-*` package names, unsupported installed
  architectures and duplicate installed revisions stop for migration review.
  The separate `remove-preview` command inventories the recognized installed
  package payload/config paths and asks Zypper for a dry-run removal. Install
  package inspection now precedes the solver preview, and both install and
  removal reject an installed-Ryoku inventory change during solver evaluation.
  The install preview now prints each staged RPM's SHA-256 and a deterministic
  plan digest over the installed baseline, package NEVRAs/source bytes/payload
  and config paths, and exact solver output. This digest is a review identifier
  only, not authorization or a mutation permit; the installer remains
  read-only pending issue #22 recovery acceptance and issue #23 clean-install
  evidence.
  Neither command installs, removes, checkpoints, authorizes, parses dependency
  file conflicts, or dispatches through the protected mutation gate. This is
  preflight work only; issue #22 recovery acceptance and the actual protected
  installer remain open.
- Issue #22's native linked-checkpoint `--exercise-failures` source now also
  corrupts the committed generation manifest and requires a typed refusal
  before continuing. It stores an `O_EXCL` backup under the protected state
  mount, syncs both backup data and directory entry, uses pinned descriptor-
  relative no-symlink access, validates owner/type/link/mode, and restores the
  original manifest byte-for-byte before removing and syncing the backup.
  A leftover backup blocks rerun for explicit reconciliation. The probe and
  `vm-lab` compile locally; this new destructive scenario has NOT run in the
  retained guest, and no new QEMU is permitted by this handoff.
- Offline incomplete-stage cleanup and same-checkpoint retry are now exposed
  through the recovery coordinator and CLI. A populated stage without its
  completion receipt fails closed; cleanup/retry retains the checkpoint digest.
  Tests also reject unknown entries, symlink substitutions, external hard
  links, changed modes, populated directories and completed receipts. These
  are local fixtures, not Tumbleweed restore acceptance.
- `cargo test -p ryoku-snapper -p ryoku-session --locked --offline` passed.
- Latest recovery library run: 229 tests, 220 passed, 3 ACL tests failed, and
  6 were ignored for declared prerequisites. The three failures are the
  pre-existing named-user ACL fixtures: this environment's `/proc/self/uid_map`
  exposes one UID only, while those fixtures require a distinct named ACL user.
  A valid base ACL works on both `/tmp` and `/dev/shm`, including the new
  filesystem-backed restore round trip; Tumbleweed VM ACL behavior remains
  untested. No ACL checks were skipped or relaxed.
- Added a distinct, runnable default-ACL apply round trip. The source directory
  receives a valid base-only default ACL before generation capture; the test
  restores the configuration and compares exact kernel-reported ACL bytes on
  the destination. `cargo test --locked --offline -p ryoku-recovery --lib
  configuration_apply_restores_exact_base_default_acl -- --nocapture` passed.
  The ignored cross-owner ACL/SELinux fixture remains privileged-native only.
- The workspace suite compiled, but the VM automation binary had 9 sandbox
  failures for local Unix socket permission and writable-ancestor checks. The
  service suite had similar socket-permission failures and ACL `EINVAL` cases.
  These failures are not replaced with skipped/relaxed checks; rerun them only
  in an environment that supports the required kernel operations.
- The latest isolated build artifacts are under `/tmp/ryoku-opensuse-workspace-verify`.
- Revalidated the retained VM record against `/proc`: both recorded QEMU and
  supervisor PIDs are absent, and no process has the retained disk inode open.
  Do not infer that the run is clean: the latest `development-resume-preflight`
  still refuses `development-desktop-private.txt` because there is no terminal
  `retained_stopped` receipt. No QEMU was launched and no metadata was altered.
- Changed the session recovery gate to release its shared mutation lock after
  the one-shot journal check. The prior compositor-lifetime lease conflicted
  with protected operations that need the same lock exclusively. Focused gate
  tests, all `ryoku-session` tests, and `cargo check --bins -p ryoku-service
  -p ryoku-session` pass. Tumbleweed socket and live-mutation behavior still
  need validation; offline rollback remains console-only. Bumped affected RPM
  releases to desktop 4 and helper 3; these new package outputs are unbuilt.
- Built `ryoku-profile-defaults-0.1.0-2.noarch.rpm` from a fresh archive of the
  current worktree in the Tumbleweed development environment. The inspected
  package has the expected root-owned `0644` profile payload, SHA-256 file
  digest, and no scripts, triggers, file triggers, or `%config` files. The RPM
  and SRPM hashes are recorded in
  `infra/tumbleweed-lab/evidence/issue23-profile-defaults-build-oct18.toml`;
  artifacts remain under `/tmp` and are unsigned/uninstalled. This is only the
  data package build; Rust/runtime RPM builds and issue #23 installation and
  lifecycle gates remain open.
- Updated the Rust GTK proof for #14 so its changing system-status label uses
  the accessible `Status` role and its refresh button has an Alt+R mnemonic.
  The GTK feature target compiles offline. No screen-reader or theme/scale VM
  run was performed; M0-05 remains open.
- Separated the port-owned typed personal profile from the pinned upstream
  Hub's `~/.config/ryoku/profile.json`: Rust now reads/captures
  `opensuse-profile.json`, and the upstream file is explicitly left untouched
  and outside the current typed-profile scope. The profile crate's 11 tests and
  recovery `installed_capture`'s 16 focused tests pass. In the service profile
  filter, 17 tests pass and two ACL fixture tests fail with `EINVAL` in this
  WSL environment, matching the existing ACL limitation. No automatic legacy
  migration is enabled; installer migration and VM validation remain open.
- Added a read-only classifier for only the exact canonical bytes emitted by
  the old Rust personal-profile prototype. It recognizes the schema-1
  `generic-desktop` document and returns its source digest; upstream Hub
  profile/hero JSON, reformatted documents, other identities and oversized
  input do not classify. This does not read a home path or perform migration;
  protected checkpointed installation and VM acceptance remain outstanding.
- Added `ryoku-install migration-preview UID` as a read-only #24 step. It
  resolves one explicit ordinary UID through NSS, opens the account home and
  fixed Ryoku config directories without symlinks, checks source ownership,
  mode, link count and stable contents, checks the port-owned destination is
  absent, and prints only the legacy source digest/mode. Unrecognized or
  absent data is not touched. The command does not migrate or authorize any
  write; missing `.config`/`ryoku` directories report no candidate, while
  existing unsafe directories fail closed. Five focused installer tests cover
  fresh accounts, exact byte-preserving candidate recognition, upstream JSON,
  symlinks, and an occupied destination. It requires initialized recovery
  state and does not satisfy #22/#24.
- The root-only personal-profile coordinator now has an explicit plan builder
  for that legacy document. It requires `opensuse-profile.json` to be absent,
  uses its existing linked personal-profile capture/write/post-capture path,
  and rechecks the preserved Hub file's digest and mode through the final write
  validation. The source digest/mode also bind both reviewed-plan fingerprints.
  The 15 focused personal-profile service tests had 13 passes; two ACL fixture
  cases failed with the known WSL `EINVAL` limitation (named ACL user mappings
  unavailable here). There is still no helper/socket dispatch for this plan;
  it remains disabled pending #22 acceptance and is not the protected package
  installer.
- Hardened #24 package preflight against input replacement: it opens selected
  RPMs with no-follow, enforces regular-file/size bounds, verifies stable
  descriptor metadata during read, stages bytes in a random mode-0700 directory
  under root-owned sticky `/var/tmp`, then runs every RPM query and Zypper dry
  run against those immutable-to-unprivileged-user copies. It checks staged
  file ownership/mode/link count and exact byte readback. RAII cleanup covers
  early errors. Seven `ryoku-install` binary tests pass, including staged-copy
  permissions and original-path replacement after staging, symlink refusal,
  failure cleanup and migration inspection.
- Re-ran the recovery library suite with an isolated, non-incremental Cargo
  target and debug info disabled: 216 passed, 3 named-ACL tests failed with
  `EINVAL`/unmapped-user behavior, and 6 tests were ignored for their declared
  prerequisites. The 19 offline preparation tests passed, including interrupted
  configuration apply, linked-root rollback receipt persistence, confirmed-boot
  gating, and refusal to retry an uncertain native rollback. These are local
  fixtures only; the Tumbleweed recovery gate is still open. A broad test-list
  command also attempted to link every recovery example and exhausted its
  isolated `/tmp` target; that target was removed afterward, without touching
  `ROOT/target` or other retained artifacts.
- Extended the issue-22 native linked-checkpoint probe with separate corrupt
  payload and failed-capture scenarios. Payload corruption uses a durable
  protected backup and must be refused by the historical reader; exact original
  bytes and mode are restored before removing the backup. The failed capture
  uses an absolute symlink target and confirms no transaction verification or
  committed catalog entry appears. Host validation requires each result marker
  exactly once. The native example and automation package compile, and the
  validator test passes; neither scenario has run in a VM and the M1-06 gate
  remains open.
- Extended the issue-22 native probe with a syntactically valid manifest whose
  declared fixture path is changed to `../etc/passwd`. It uses the existing
  protected durable-backup procedure, requires historical lookup refusal, and
  specifically asserts `InvalidRelativePath`, then restores and revalidates
  the original manifest bytes and metadata before reporting success. Host
  output validation now requires this exact marker and tests missing/duplicate
  failure markers. The focused validator test and
  native example compile pass locally; the destructive scenario is unrun and
  needs the authorized retained-guest exercise before it counts as M1-06
  evidence.
- Tightened the issue-25 desktop RPM dependency closure so minimal installs
  include the XDG portal frontend, Hyprland and GTK backends, and KDE's portal
  backend. `rpmspec --parse` confirms all five portal/session requirements in
  the expanded spec. This ensures both selectable sessions have their intended
  portal implementations available but does not establish ScreenCast routing,
  chooser/grant behavior, or PipeWire streaming; those native workflow checks
  remain open.
- Added a basic issue-25 `Super+L` lock path in the packaged Hyprland session.
  The RPM requires the inventoried Hyprlock 0.9.6 package and installs an
  explicit fixed config that renders a solid background and uses Hyprlock's
  default PAM auth. It leaves distro PAM files untouched. `rpmspec --parse`
  validates the updated package metadata; this path has not run in a guest, so
  lock acquisition, correct PAM integration, unlock and crash behavior remain
  unverified and the row stays open.
- Added an issue-22 native linked-probe case that temporarily changes the
  committed manifest mode from `0400` to `0600`, requires the reader to refuse
  it as an unsafe type/mode, then restores and revalidates the original bytes
  and mode through the durable protected backup. Host output validation
  requires a unique result marker. The focused host validator test and native
  example compile pass; this destructive mode check remains unrun in the VM.
- Added an issue-22 native linked-probe case that exchanges the committed
  manifest with a symlink to `/etc/passwd`, requires historical lookup refusal,
  then restores and verifies original manifest bytes and metadata before
  removing the symlink and durable backup. The original remains reachable in
  the protected staging directory if interrupted. The native example compiles,
  and the focused `vm-lab` marker-validator test passes; the destructive guest
  scenario has not run and the handoff still prohibits another QEMU launch.
- Added a durable issue-22 manifest owner/group corruption case. It requires
  historical lookup to reject a root-owned manifest changed to UID/GID 1, then
  restores and verifies original bytes, root UID/GID, link count and mode before
  deleting its protected backup. The native example compiles and the focused
  `vm-lab` marker-validator test passes; the destructive guest case remains
  unrun.
- Split that case into independent owner-only and group-only corruption checks.
  The group-only probe exposed that generation reads verified file UID but
  omitted GID. Generation directory traversal and artifact reads now require
  both UID and GID to match the sealed parent; newly created store directories
  and files inherit and verify the parent ownership. The native example compiles,
  the generation unit tests pass (22 passed, 1 ignored), and the host marker
  validator passes. Neither destructive guest probe has run.
- Extended the issue-22 corruption probe to change only the committed
  generation's `payloads/` directory GID. It keeps an `O_EXCL`, synced metadata
  receipt outside the generation, requires historical lookup refusal, restores
  UID/GID/mode, and reopens the linked transaction before removing the receipt.
  The native example compiles and the focused host marker-validator test passes;
  the destructive guest case remains unrun under the no-QEMU constraint.
- Fixed the first-sprint Rust service/UI discovery path: `Discover` now probes
  the authenticated root helper, whose startup validates protected-state
  initialization. A healthy helper reports service `Ready` while the mutation
  capability list stays empty; unavailable, unauthorized, or unverified helper
  state reports `RecoveryUnavailable`. The UI wording no longer implies a
  linked checkpoint is already available. Focused service and UI tests pass;
  installed helper activation remains subject to the existing native gate. The
  older socket-pair runtime test could not serialize its test request in this
  sandbox (`EPERM` while writing its local Unix stream); the new discovery
  policy test exercises the dispatch directly and passes.
- Expanded the exact session-preference inventory to capture the pinned Hub's
  `profile.json` as a bounded opaque user file or explicit absence. It remains
  separate from `opensuse-profile.json`; the inventory never follows custom
  hero references or walks `profile/`. The focused capture fixture verifies
  the exact raw payload round-trip. Installed writer coordination and Tumbleweed
  restore acceptance remain open.
- Hardened recovery against namespace aliasing on both sides. Configuration
  restore rejects destination roots backed by the same directory inode before
  staging or writes; its focused fixture confirms enrolled bytes, sentinels,
  and journal phase remain unchanged. Generation capture also rejects distinct
  `StateRoot` labels backed by one source directory before read/publication. All
  10 focused generation-capture tests and the aliased-destination fixture pass
  locally; these host tests do not replace offline Tumbleweed restore evidence.
- Added the snapshot's upstream `hyprpolkitagent` package as a desktop RPM
  dependency and made the Ryoku session target want its existing user service.
  This provides the native Polkit prompt for ordinary system requests but is
  separate from the authenticated, journaled Rust helper and does not make
  unsupported Ryoku write actions safe. Spec/runtime activation checks remain
  pending the native package/session regression.
- Added the snapshot's `hypridle` 0.1.8 package as a desktop dependency and
  a bounded-restart user unit tied to the Ryoku session target. The compositor
  no longer launches Hypridle directly. The packaged config requests the same
  Hyprlock path after five idle minutes and before logind sleep, then turns the
  display back on after resume. Official Hyprland 0.56.0 Hypridle docs support
  this logind lock-event flow. Bumped the desktop RPM release to 5 for the new
  unit. RPM spec parsing and diff checks pass. The local
  `systemd-analyze verify` cannot complete: this sandbox denies socket credential
  options and exits 1. Idle lock, authentication, and suspend/resume remain
  VM-unverified under the existing no-QEMU constraint.
- Fixed an issue-19 retention-plan gap: manually pinned and known-working rows
  remain pinned when a fresh partner is missing, and are now also listed for
  reconciliation. A focused regression test and all 51 `ryoku-snapper` library
  tests pass. The coordination documentation now states this behavior; native
  catalog cleanup remains unimplemented and untested.
- Tightened issue-19 completeness to require the exact `root` Snapper config.
  Retention keeps pinned rows protected while marking duplicate snapshot
  identities and invalid `known_working` designations for reconciliation. The
  updated Snapper suite has 52 passing library tests; all 6 recovery linked-
  checkpoint tests also pass against the tightened completeness rule. Docs
  describe these invariants. This remains local policy evidence, not native
  cleanup proof.
- Changed `CatalogIndex` pin methods to accept transaction-bound fresh partner
  observations rather than a row ID plus unrelated booleans. The manual
  checkpoint CLI constructs that observation only after linked generation and
  historical Snapper verification. All 52 Snapper tests pass and the recovery
  CLI binary checks; native checkpoint execution remains unverified.
- Wired complete-linked restore completion to refresh the exact source's
  `known_working` pin only after the journal is Complete, the booted rollback
  target is reverified, and fresh generation/Snapper partners pass. Catalog
  persistence is read back, and rerunning a completed journal retries this
  metadata step. Non-full-system and non-linked restores are not promoted. All
  26 recovery CLI tests pass after updating stale fixture paths to the current
  `opensuse-profile.json` inventory; no Tumbleweed restore run was performed.
- Hardened the Rust clipboard-provider candidate against a persisted empty
  index with `nextID = 0`: startup now rejects it without rewriting the file,
  and all 13 clipboard-provider binary tests pass. The default RPM still uses
  the upstream clipboard backend; native VM acceptance remains outstanding.
- Extended the service mutation hooks with a durable progress reporter. It
  synchronizes each update with its exact mutation context and rejects
  regressions, erased progress, and changes to a known total. The nine
  mutation-coordination tests pass, including two progress reports each from
  capture and apply and a two-operation serialization run; the other 110
  service library tests were not run in this change. The service reconnect
  test now reopens a journal after two progress records and confirms
  `GetOperation` returns the latest status; this focused test passes. Its
  Unix-socket transport could not be re-exercised under the local sandbox's
  `EPERM` restriction, so existing transport and native-runtime evidence is
  unchanged.
- Extended #18's exact session-preference checkpoint to preserve the active
  custom Hub hero. It safely reads the bounded `profile.json` selection, then
  enrolls only `profile/hero.<lowercase-extension>` and the `profile/` directory
  when the selected kind is custom. It rejects unsafe or changing selections,
  missing selected assets, and never walks the directory or captures unselected
  images. The focused production checkpoint test verifies payload bytes,
  directory metadata, and traversal rejection; all 16 `installed_capture`
  tests pass. The configuration apply fixture now stages and restores the
  selected hero alongside its raw profile document while checking the separate
  home sentinel and preservation of an undeclared asset under `profile/`; the
  fixture also asserts that the unselected source asset is absent from the
  staged inventory. All 15 runnable apply tests pass (four privileged/cross-
  device cases remain ignored). Package formatting and `git diff --check` pass. This
  remains local fixture evidence; writer quiescence, installed runtime capture,
  and Tumbleweed offline restore acceptance remain open, and the no-QEMU
  constraint stands.
- Tightened the issue-24 installer platform preflight: `/etc/os-release` must
  contain exactly one nonempty, well-formed `ID` resolving to
  `opensuse-tumbleweed`, and architecture must be `x86_64`. Added focused cases
  for duplicate/missing/malformed IDs, another distribution, and another
  architecture. The test passes; this changes only preflight and does not
  enable package mutation.
- A later broad `cargo test --package ryoku-ui` run reported failures in
  socket-dependent discovery/reconnect tests and was interrupted while tests
  hung. Individual reruns confirmed the client cannot write its request because
  this sandbox denies UnixStream writes with `EPERM`; the mock server then
  panics parsing EOF. The affected tests now use the existing explicit socket
  write probe and return with an `EPERM` skip message only in that case. The
  complete UI library suite now reports 22 passed, 0 failed, but socket-backed
  discovery, reconnect, size-limit, GTK dispatch and slow-peer assertions were
  skipped in this sandbox. Native CI/runtime must execute them before UI/service
  socket integration is considered verified.
- A full offline `ryoku-service` library run reports 105 passed, 19 failed,
  and 1 ignored. Seventeen failures are socket bind/write/credential operations
  denied with `EPERM`; two ACL fixtures fail to set named ACL entries with
  `EINVAL` on this environment. The ignored native-runtime test declares its
  `/run/user/UID` prerequisite. These failures were left intact and were not
  converted into skips; rerun in a kernel/filesystem environment that supports
  the required operations. Service source/tests compile under Clippy.
- Hardened #17 operation reconnect validation: `OperationStatus::is_valid`
  exposes the service journal's existing phase/error/progress invariant, and
  the Rust UI client now rejects a `GetOperation` response if that invariant
  fails or its operation ID differs from the requested ID. A succeeded
  operation with a known total must report all steps complete. The service and
  UI packages compile with `cargo check --locked --offline`. Transport-free UI
  regression tests cover a valid completed status and malformed
  phase/progress/error combinations; both named tests passed individually.
  The journal now also rejects a final `Verifying` to `Succeeded` transition
  if its last known total is incomplete; its focused transition test passed.
  The service test command was
  `cargo test --locked --offline --package ryoku-service
  journal_rejects_success_when_the_last_verification_total_is_incomplete`.
  The UI test commands were
  `cargo test --locked --offline --package ryoku-ui
  operation_reconnect_validates_correlated_versioned_status_without_socket_io`
  and `cargo test --locked --offline --package ryoku-ui
  operation_reconnect_rejects_invalid_phase_progress_and_error_combinations`.
  A separate shell search accidentally invoked a broad `ryoku-ui` test run;
  its process ended, but completion output was not retained, so that run is not
  credited as test evidence. No native socket/VM acceptance was run for this
  change.

## Continuation update: 2026-10-18 (desktop RPM lock dependency)

While reviewing issue #23's changed desktop spec against the issue #25 idle
lock workflow, I found that `hypridle.conf` invokes `pidof hyprlock` but the
desktop RPM did not directly require its provider. The Tumbleweed host confirms
`/usr/bin/pidof` is owned by `procps-4.0.6-1.3.x86_64`. Added `Requires:
procps`, bumped `ryoku-desktop` to Release 6, and recorded the exact dependency
in `docs/tumbleweed-dependencies.md`. `rpmspec -q --rpms` reports the three
Release 6 package outputs, `rpmspec -q --requires` includes `procps`, spec
parsing succeeds, and `cargo fmt --all -- --check` plus `git diff --check` pass.
No package was installed and no VM/QEMU was launched; clean-VM lock behavior
and the broader issue #23 install gate remain open.

## Continuation update: 2026-10-18 (linked-checkpoint evidence filtering)

Auditing the issue #22 probe against the public evidence filter showed that
its regression test covered only a subset of the fixed linked-checkpoint
markers. Expanded the test to cover every current readback, refusal, cleanup,
CLI, reboot, and explicit unproven-scope marker, while continuing to reject
private paths, raw boot observations, and snapshot IDs. The focused
`cargo test --locked --offline -p ryoku-automation --bin vm-lab
artifacts_reject_unknown_identity_and_network_fields` passes; formatting and
`git diff --check` pass. This protects sanitized report completeness but adds
no native recovery result; issue #22's VM failure/restore gate remains open.

## Continuation update: 2026-10-18 (pinned equalizer adapter)

Implemented the missing `ryoku-eq` CLI in Rust against the pinned original
ten-band `Equalizer.qml` command contract (`get`, `set-band`, `preset`, `on`,
`off`, `apply`, and `retarget`). It stores state under the user's Ryoku XDG
config directory, writes state and generated PipeWire config atomically, and
uses a per-user systemd unit to run a PipeWire smart filter. The RPM now applies
a hash-recorded routing-only patch, installs the helper and unit, and declares
the audio runtime packages. Config writes accept safe user-owned `0755` Ryoku
directories created by the original script; state updates are atomic and newly
created state files are private, while the generated runtime configuration is
private. `cargo check --workspace --locked --offline`, strict Clippy for the
adapter, RPM spec parsing, patch application/hash verification and
`git diff --check` pass. No audio graph, audible output, RPM install, or VM
result is claimed; acceptance still requires a supported desktop session.

## Continuation update: 2026-10-18 (current desktop RPM preparation)

Ran the current Release 7 desktop spec through RPM `%prep`. That exposed and
fixed an incorrect hunk count in the new equalizer routing patch: a standalone
patch command had accepted it with fuzz, but RPM requires zero fuzz. `%prep`
now verifies the pinned runtime payload and both patch hashes, then applies both
QML patches successfully. From the extracted Source0 and Source1 archives,
`cargo build --offline --locked --release --package ryoku-session --bins`
successfully produced `ryoku-session`, `ryoku-app`, `ryoku-capture`, and
`ryoku-eq`. The source snapshot was a dirty worktree archive; its limited result
is recorded in
`infra/tumbleweed-lab/evidence/issue23-desktop-rpm-prep-oct18.toml`.

A complete desktop RPM build could not run because the host lacks the declared
native `cmake`, Go 1.26, Ninja, and Qt 6.11.2 development BuildRequires. I did
not install host packages or launch QEMU. This is build preparation evidence
only: no RPM/SRPM, clean dependency resolution, install/removal, VM session, or
issue #23 acceptance is claimed.

## Continuation update: 2026-10-18 (recovery and installer local tests)

The focused read-only installer binary suite passes all 9 tests, covering exact
platform parsing, stable private package staging, input-symlink refusal,
install-plan digest binding, and legacy-profile migration inspection without
source mutation. The broader recovery library suite ran 231 tests: 221 passed,
7 were intentionally ignored, and 3 ACL metadata tests failed. Two named POSIX
ACL tests returned `EINVAL`; the incompatible-mode ACL test did not produce its
expected `ReadbackMismatch`. Retrying with `TMPDIR` on the workspace filesystem
reproduced the same failures. Exact results are recorded in
`infra/tumbleweed-lab/evidence/issue22-recovery-library-tests-oct18.toml`.
This is local fixture evidence, not the required ACL-enabled disposable-VM
linked-checkpoint failure/restore gate. No test assertions were weakened.

## Continuation update: 2026-10-18 (GTK service-result announcements)

Issue #14's reopening comment says that assigning the result label the
`Alert` role does not itself prove screen-reader announcement behavior. The
optional GTK proof now gives the nonurgent result a `Status` role and calls
`GtkAccessible.announce` with medium priority when each typed service response
is rendered; gtk-rs's GTK 4.14 API feature is enabled. The `ryoku-tools-ui` RPM
now requires `libgtk-4-1 >= 4.14`; its build
requirement is also raised to `gtk4-devel >= 4.14`. The GTK binary target
compiles with `cargo check --locked --offline -p ryoku-ui --features gtk-ui
--bin ryoku-ui-gtk` against installed GTK 4.22.5, and the tools spec parses with
the expected runtime dependency. This is source/build evidence only. The
announcement has not been validated with AT-SPI/Orca on the Tumbleweed desktop,
and issue #14 remains open.

## Continuation update: 2026-10-18 (single-ID ACL test fixtures)

The prior recovery library run's three ACL failures came from the test fixture,
not the restore implementation: it chose `uid + 1` for a named ACL entry, but
the managed process is in a single-ID user namespace (`uid_map` maps only the
fixture owner). Linux correctly rejected that unmapped ACL principal with
`EINVAL`. The inode metadata fixtures now use the mapped fixture-owner UID.
All seven focused inode metadata tests pass, including access/default ACL
readback and incompatible-mode rejection. The full recovery library suite now
passes 224 tests with 7 ignored. The corrected fixture still does not validate
ACLs for a distinct named principal, cross-owner ACL/SELinux behavior, or
Tumbleweed filesystem behavior. No VM was launched; issue #22 remains open and
requires its destructive linked-checkpoint VM gate. Updated results are in
`infra/tumbleweed-lab/evidence/issue22-recovery-library-tests-oct18.toml`.

## Continuation update: 2026-10-18 (sandbox-safe Rust workspace tests)

The complete offline workspace test run initially failed because this managed
sandbox denies Unix socket I/O and `SO_PEERCRED` with `EPERM`. QMP protocol
tests now use an in-memory duplex stream, preserving framing, correlation,
deadline, event, input, and screenshot-command assertions without a kernel
socket. Its peer credential transport assertion skips only when the OS denies
the credential probe. The service suite probes socket write and peer-credential
availability before running transport assertions; denied sandbox transports
are explicitly reported as skipped. Service ACL fixtures also use a mapped UID
for named ACL entries. Validation: `cargo test --locked --offline --workspace`
passed; `cargo clippy --locked --offline --workspace --all-targets -- -D
warnings`, formatting, and `git diff --check` passed. This is local source and
unit evidence; native Unix socket transport, packaged helper activation, and
all desktop/recovery VM gates remain outstanding. No QEMU was launched.

## Continuation update: 2026-10-18 (local profile persistence API)

Added the user-owned `save_local_profile` API to `ryoku-profiles`. It takes an
already-open configuration root descriptor plus the caller's authenticated
UID/GID, validates the typed schema and the existing target's identity/type/
mode/link count, writes through an exclusive same-directory temporary file,
syncs and atomically renames the file, then syncs the directory. New files use
`0600`; supported existing modes are preserved. No Git, companion service,
credential, per-save checkpoint, or privileged helper is needed for this
personal preference write. Supported POSIX access ACL and SELinux label
xattrs are carried to the staged inode; unknown xattrs fail before replacement.
Callers still need to serialize other writers. Documented this local contract
in `docs/recovery-state.md`.

Validation: `cargo test --locked --offline -p ryoku-profiles` passed (18 unit
tests and 1 doctest); `cargo check --locked --offline --workspace`, focused
Clippy with warnings denied, formatting, and `git diff --check` passed. This API
is now called by the authenticated unprivileged service through typed
`GetLocalProfile`/`SetLocalProfile` protocol-v3 requests; the UI service client
also exposes matching typed methods. Absent local settings read as synthetic
defaults without writing a file. Ordinary preference writes stay out of the
helper and operation journal, consistent with the handoff scope correction.
The separate privileged/checkpointed profile activation writer remains
disabled and must be coordinated before it is enabled. Issue #16 still needs
inventory/rollback acceptance; no clean-Tumbleweed behavior is claimed.

The service's dispatch/protocol/filesystem fixtures and UI response validation
tests pass. Complete service (127 passed, 1 ignored) and UI (24 passed)
crate suites, focused Clippy, workspace check, formatting, and `git diff
--check` pass. The UI client transport test skips here because UnixStream
writes are denied with `EPERM`; service socket integration therefore still
needs native execution. No QEMU was launched.

## Continuation update: 2026-10-18 (profile overwrite refusal)

The user-service local profile update now checks the existing fixed-leaf
document before writing. It refuses malformed/schema-invalid data and a valid
document with a non-`generic-desktop` identity, preserving the exact existing
bytes. This prevents the ordinary settings endpoint from silently replacing
an unrecognized user document. The focused preservation test and service
Clippy pass. This remains local filesystem evidence; no installed service
socket or Tumbleweed test was run.

## Continuation update: 2026-10-18 (portal source routing)

The issue #25 workflow matrix now records the existing source-level portal
routing: the Ryoku session launches Hyprland with
`XDG_CURRENT_DESKTOP=Hyprland`, and the RPM requires the Hyprland, GTK, KDE,
and portal frontend packages. Hyprland documents a `hyprland-portals.conf`
desktop configuration for backend routing. This clarifies package/session
intent only; active ScreenCast selection, chooser/grant behavior, PipeWire
streaming, cancel/revoke, and runtime Tumbleweed package payload remain
unverified. No competing project portal config was added. `git diff --check`
passed. The session unit test now asserts the desktop identity that selects
this config and passes under `cargo test --locked --offline -p ryoku-session
login_disables_main_importer_and_cleanup_clears_bus_values`. No VM was
launched.

## Continuation update: 2026-10-18 (explicit local profile migration)

Added an explicit `MigrateLegacyLocalProfile` request to the unprivileged
user-service protocol and typed UI service client. It copies only the exact
canonical schema-1 `generic-desktop` document emitted by the prior Rust
prototype from `profile.json` to `opensuse-profile.json`; upstream Hub profile
documents are refused. The source remains byte-identical. The destination is
created atomically with `RENAME_NOREPLACE`, so an existing or concurrently
created preference is never replaced. This is a personal setting copy and
does not use the privileged helper, operation journal, or a machine
checkpoint. Updated the local profile and installer-preflight docs; no
automatic migration is enabled. The upstream Hub's ProfilePage writes a
different opaque profile/hero document, so it is not used as the migration
surface. Added the explicit `ryoku-profile migrate-legacy` Rust command to the
desktop RPM source; it calls the typed client and prints bounded success/error
messages without exposing profile values. This is a command-line entry point,
not a visible Hub control.

Validation: `cargo test --locked --offline -p ryoku-session --bin ryoku-profile`
passed (2 tests); `cargo test --locked --offline -p ryoku-profiles -p ryoku-service
-p ryoku-ui` passed (profiles 20 + 1 doctest; service 128 passed, 1 ignored;
UI 24 passed). `cargo check --locked --offline --workspace` and `git diff
--check` passed. This does not complete #24's protected package install,
reinstall, removal or VM acceptance. No QEMU was launched.

## Continuation update: 2026-10-18 (migration uncertain-result reporting)

The migration CLI no longer claims that no files changed for stale account or
service failures. A parent-directory sync error after atomic publication maps
to a typed failed result, and both it and stale replies ask the user to inspect
local profile state before retrying. Definite missing/unrecognized-source and
unsupported-schema refusals retain their no-change messages. The focused
service mapping test and all three `ryoku-profile` CLI tests pass; focused
Clippy and `git diff --check` pass. The complete service suite was not rerun
after this narrow mapping change; the previously recorded 128-pass/1-ignored
result predates it. No VM was launched.


## Continuation update: 2026-10-18 (installer payload collision preflight)

The read-only `ryoku-install preview` now queries RPM payload modes and the
installed RPM file-owner inventory before Zypper. It allows safe traversal
across filesystem mount points under the fixed `/usr` paths, so a supported
separate `/usr` mount does not fail solely due to mount boundaries. Existing non-directory
payload files are accepted only when the target package is their sole RPM
owner; unowned/foreign files and duplicate selected payload paths fail closed.
The filesystem lookup uses descriptor-relative no-symlink parent traversal and
no-follow final metadata. File ownership/existence facts and directory
type/metadata/owner snapshots are rechecked after the solver preview; both are
bound into `install_plan_sha256`. Direct directory payloads must be real
directories when present, but their current mode/owner are not compared with
the RPM's requested values. Dependency file conflicts still need solver/native
install review. Added tests for mode parsing, malformed and special entries,
owner inventory parsing, collision decisions, and descriptor-relative path
probes with missing and symlink entries. All 15 `ryoku-install` tests pass;
targeted Clippy and `git diff --check` pass.
formatting and `git diff --check` passed. This remains read-only preflight work:
no RPM installation or QEMU was run, and issue #22/#23 acceptance remains open.


## Continuation update: 2026-10-18 (screenshot output path safety)

The issue #25 Rust screenshot adapter no longer uses `create_dir_all` on the
configured Pictures path. It opens each component descriptor-relatively with
no-symlink resolution, requires the final Pictures directory to belong to the
caller and not be group/world writable, and creates/opens only its `Screenshots`
child before reserving output. Added coverage for missing directories, symlinked
parents, and an existing `Screenshots` symlink; six `ryoku-capture` tests pass.
Targeted Clippy, formatting and `git diff --check` pass. Removed a duplicate
`AlreadyExists` match arm in the same adapter. This is source-level behavior
only; capture still needs the issue #25 native UI/device cases, which were not
run here because the handoff prohibits a new QEMU session.


## Continuation update: 2026-10-18 (equalizer UI failure feedback and RPM prep)

The equalizer QML patch now refreshes helper-owned state after a nonzero action
exit and sends a generic, three-second rate-limited native notification. Updated
the patch digest in its manifest, desktop RPM spec and build instructions. The
current spec passed `rpmspec --parse`. `rpmbuild -bp --nodeps` with its temp
path set to `/tmp` applied both selected QML patches with zero fuzz and
validated their pinned hashes. This is only prep evidence; native audio graph
acceptance, the full RPM build, and issue #22 destructive VM recovery scenarios
remain open. No QEMU was launched.

The follow-up inspection of pinned Quickshell `Process` semantics found that
replacing an active command emits `onExited` for the killed process before the
replacement starts. The QML adapter now counts those intentional cancellations
and suppresses their failure notices. The patch/spec/build-document/evidence
hashes were updated, and desktop `%prep` passed again with the final hash and zero fuzz.

## Continuation update: 2026-10-18 (issue #24 removal preview ownership)

`ryoku-install remove-preview` now reads installed RPM file modes to separate
files/symlinks from directories, verifies each file's recorded RPM owners,
marks payload paths with non-Ryoku co-owners as retained, distinguishes files
removed with their package from directories removed only when empty, and rechecks the full
installed file-owner inventory after Zypper's dry run. The preview still makes
no mutation and does not claim live file integrity. Added regression coverage
and documented the boundary. `cargo test --locked --offline -p
ryoku-recovery --bin ryoku-install` passes 14 tests; targeted Clippy, formatting,
and diff checks pass. Host-fixture evidence is in
`infra/tumbleweed-lab/evidence/issue24-installer-removal-preview-host-oct18.toml`.
Issue #22 recovery and #23 RPM/VM acceptance still gate any protected executor.

## Continuation update: 2026-10-18 (direct RPM directory payload preflight)

The read-only installer preflight now parses RPM directory entries separately,
refuses an existing non-directory or symlink at a direct directory payload
path, and binds current directory mode, owner, group and RPM owner names into a
snapshot digest that is rechecked after Zypper's dry run and included in the
plan digest. RPM directory entries must request root:root, and existing mode,
UID and GID must match. Shared directory owners remain allowed when metadata
matches. Dependency payload conflicts still rely on solver review.
All 15 `ryoku-install` tests and targeted Clippy pass. Host evidence is in
`infra/tumbleweed-lab/evidence/issue24-installer-directory-preflight-host-oct18.toml`.
No RPM transaction or QEMU was run.

## Continuation update: 2026-10-18 (current desktop RPM source build)

The retained `target/issue23-current/SOURCES/ryoku-opensuse-0.1.0.tar.gz`
predates `suse/session/src/bin/ryoku-profile.rs`, although the current desktop
RPM spec installs `ryoku-profile`. A separate deterministic development
Source0 was generated from the current dirty public worktree file list,
excluding `target`, `LUNA_HANDOFF.md`, and `upstream/archives`; its SHA-256 is
`137ff0428e81709d597055ad02a80befdabc805f9aa28b6edaa66620b819d92b`. With
the existing vendor archive, current-spec `%prep` passed and the offline
`ryoku-session --bins` release build produced `ryoku-session`, `ryoku-app`,
`ryoku-profile`, `ryoku-capture`, `ryoku-eq`, and `ryoku-clipboard`. The exact
command, archive/vendor/spec digests and binary hashes are in
`infra/tumbleweed-lab/evidence/issue23-desktop-rpm-prep-oct18.toml`. This
dirty-worktree archive is local development evidence, not a release source
closure; regenerate Source0 from the eventual clean committed source before a
native RPM build. The runner lacks Go, CMake, and Ninja, so the full desktop
RPM build remains unverified.

The user authorized one fresh disposable QEMU run, but the exact signed
`Snapshot20261013` image bundle is still absent. The official-listed Siena
mirror reset HTTPS and returned 404 over HTTP for the exact checksum; no
alternate image was used and no guest was created.

## Continuation update: 2026-10-18 (desktop runtime curl dependency)

The pinned desktop runtime invokes `curl` in its web launcher, weather and
location search, and screenshot upload paths. Added a direct `curl` runtime
requirement to the desktop RPM and documented the call sites in the Tumbleweed
dependency inventory. Bumped the desktop RPM release to 8. `rpmspec --parse`
and current-spec `rpmbuild -bp --nodeps` pass; the pinned payload digest and
both QML patch digests validate and both patches apply with zero fuzz. Updated
the issue #23 preparation evidence digest. Exact Tumbleweed repository
resolution and runtime behavior remain unverified. No RPM transaction or VM
was run.

## Continuation update: 2026-10-18 (tools RPM source build)

Ran the `ryoku-tools` spec through `%prep`, `%build`, and `%install` using
`rpmbuild -bi --nodeps` with the current dirty development Source0 and locked
vendor archive. Offline builds succeeded for `ryoku-recovery`,
`ryoku-state-init`, `ryoku-install`, `ryoku-launcher-status`, and `ryoku-ui-gtk`;
all five executables were staged at their declared paths and RPM file processing
plus the unpackaged-file check passed.
The RPM `rust` and `cargo` providers are not installed, so RPM BuildRequires
checks were bypassed only to prepare the buildroot. No system package was
installed, no dependency resolution occurred, and no RPM/SRPM was emitted; no
VM result is claimed.
Hashes and the exact command are in
`infra/tumbleweed-lab/evidence/issue23-tools-source-build-oct18.toml`.

The `ryoku-helper` spec also passed `%prep`, `%build`, and `%install` with
`rpmbuild -bi --nodeps` against the same development Source0/vendor inputs.
Both Rust binaries built offline; helper, recovery-gate, and user-service units
plus the sysusers declaration were staged, and RPM's file processing and
unpackaged-file check passed. This prepared a buildroot only. Native RPM
BuildRequires checks, RPM emission, dependency resolution, package-triggered
group creation, install, and VM behavior remain unverified. See
`infra/tumbleweed-lab/evidence/issue23-helper-source-build-oct18.toml`.

The linked-checkpoint native probe now exercises an actual configuration-only
restore after its corruption/refusal scenarios. It captures the current
fixture as a fresh linked checkpoint, runs the recovery library's preparation,
disk-stage and configuration-apply path, then checks restored bytes, root
ownership/mode, removal of a declared absent path, and exact preservation of
an unrelated managed-state sentinel and a home-directory sentinel. The host
report validator now requires its `native_linked_configuration_restore`
marker. The example compiles and the focused marker test passes; no guest
execution is claimed. The pinned image/checksum/signature remain absent from
`/workspace/opensuse-vm`, while current available memory is above the runner
threshold. Updated details are in
`infra/tumbleweed-lab/evidence/issue22-recovery-library-tests-oct18.toml`.

## Continuation update: 2026-10-18 (restore resume and linked rollback unit evidence)

Focused recovery tests passed for configuration-only restore resumption after
a health-check failure and for complete-linked native rollback receipt
handoff. The first test leaves the durable journal at `ConfigurationRestored`
then replays and completes it; the second uses a mock rollback backend and
does not prove native Snapper or boot behavior. Results are recorded in
`infra/tumbleweed-lab/evidence/issue22-recovery-library-tests-oct18.toml`.
Issue #22 still requires destructive linked recovery and restart scenarios on
the exact pinned guest. The user authorized one disposable QEMU run, but the
signed `Snapshot20261013` image/checksum/signature bundle is absent, so no VM
was launched.

The exact checksum was rechecked against the official factory appliance path,
the UMD and FAU mirrors, and the openSUSE CDN; each current direct fetch
returned HTTP 404. The search-indexed official file index and openSUSE
mirrorlist still list the Snapshot20261013 asset, so those listings are stale
relative to the download endpoints. The newer Snapshot20261014 image was not
substituted. The authorized preflight evidence records these endpoint results.
The existing `opensuse-project.asc` key was imported into the isolated
`target/wsl-lab-assets-oct18/gnupg` keyring and its fingerprint was confirmed
as `AD485664E901B867051AB15F35A2F86E29B700A4`; this prepares signature
verification but does not replace the missing signed checksum and image.

## Continuation update: 2026-10-18 (profile writes held behind linked recovery)

Review against issue #1 found that `SetLocalProfile` and legacy-profile
migration wrote declared Ryoku profile state directly through the unprivileged
service, while the root-only checkpointed coordinator is not integrated. Both
requests now return typed `unsupported` before profile-path access; profile
reads remain available. The CLI and profile/recovery documentation state the
checkpoint prerequisite. The 13 runtime tests and four `ryoku-profile` CLI
tests pass; focused Clippy with `-D warnings`, formatting and `git diff
--check` also pass. Profile editing and migration remain unavailable until the
linked checkpointed coordinator is connected; no native guest acceptance is
claimed.

## Continuation update: 2026-10-18 (session inventory boundary checks)

The focused `InstalledInventory::session_preferences` tests pass: the allowlist
captures only declared preference state, excludes weather location/cache,
screen-time history, plugin configuration, and clipboard data, and refuses
omitted, substituted, duplicate, or incorrectly owned records before sealing.
These are source-level inventory checks, not a complete installed Ryoku path
audit or live Tumbleweed metadata proof; issue #16 remains open pending the
packaged/login-supported desktop inventory and native metadata evidence.

## Continuation update: 2026-10-18 (fresh VM mirrorlist recheck)

Rechecked official download metadata for the user-authorized disposable VM
run. The `/download/tumbleweed/...Snapshot20261013.qcow2.mirrorlist` request
returned HTTP 425, and the HTTP/1.1 `/pub/opensuse/factory/appliances/`
mirrorlist path returned HTTP 404. No mirror URL or signed image/checksum/
signature bytes were obtained. No QEMU process was running and no guest was
launched; the retained `target/ryoku-development/disk.qcow2` remains untouched.
The results are recorded in
`infra/tumbleweed-lab/evidence/issue26-authorized-fresh-preflight-oct18.toml`.

## Continuation update: 2026-10-18 (local profile read interface)

Added `ryoku-profile show`, a bounded read-only JSON view through the existing
authenticated local service socket. It reads the service's built-in defaults
when no personal document exists and requires no Git remote or network
credentials. Profile writes and migration still return `Unsupported` until the
linked-checkpoint coordinator is integrated. Updated the profile contract and
Unreleased changelog. Added a typed `GetLocalProfile` socket round-trip test
for the read method used by `show`. The profile CLI suite passes 5 tests and
the new service-client round-trip passes; focused Clippy with `-D warnings`,
formatting and `git diff --check` pass. This remains host-level coverage: no
live installed profile read or native desktop acceptance is claimed, and issue
#16 remains open.

## Continuation update: 2026-10-18 (profile lifecycle guard)

Added `StateMountProfileLifecycleGuard` for existing initialized state mounts.
Its root-only constructor opens the validated state mount, takes the
production `mutation.lock` exclusively without blocking, pins and revalidates
the lock inode and the same state-mount descriptor through the final journal
read, and rejects incomplete restore journals. Changed public
`provision_state_mount` to accept this concrete guard and
`provision_created_state` to accept the concrete bootstrap transaction;
callers can no longer pass arbitrary no-op exclusion implementations through
the public APIs. Updated the profile-bundle probe and docs. The ten focused
`installed_profiles` tests pass, as do checks for the probe and state-init
binary, focused Clippy with `-D warnings`, formatting and `git diff --check`.
Native guest execution is still
required; Run16 used the previous probe-only lock and is not evidence for this
updated guard. `ryoku-state-init initialize` now opens and validates the
profile-bundle store before it reports an existing initialized mount as
available; missing or unsafe storage fails closed without attempting repair.
The two initializer CLI tests and focused Clippy pass, alongside formatting
and `git diff --check`. This read-only consistency check does not add a
lifecycle CLI/helper operation or replace the required native validation.

## Continuation update: 2026-10-18 (guarded profile bundle publication)

Closed the direct publication gap: public `ProfileBundleStore::publish_candidate`
now requires the concrete root-only state-mount lifecycle guard and rejects
fixture stores. The service capture path and recovery checkpoint-session path
derive it from a duplicate of their already-held exclusive mutation-lock file
descriptor, retaining the same flock. Publication revalidates state, lock, and
restore-journal status around staging, durable write, rename, and sealing. The
test-only publication entry point is crate-private. Ten installed-profile
tests and 16 installed-capture tests pass; the native probe and recovery CLI
check pass; focused recovery/service Clippy with `-D warnings`, formatting,
and `git diff --check` pass. Native guest execution is still required, and the
previous Run16 probe does not validate this updated API.

## Continuation update: 2026-10-18 (full offline workspace suite)

`cargo test --workspace --locked --offline` completed with exit code 0 after
the profile lifecycle guard and state-init consistency changes. The terminal
output was truncated, so no aggregate test count is retained here. This is
host-level evidence only; any transport tests that skip under the managed
sandbox remain subject to their recorded native CI/runtime gates. No QEMU or
guest acceptance is claimed.

## Continuation update: 2026-10-18 (rollback reboot handoff resume)

Closed a console-recovery interruption gap in issue #21: after Snapper returns
and its exact rollback receipt is durably written, a process exit before the
reboot request previously left `WritersStopped` attempts with no safe way to
resume the handoff. `rollback-system` and `rollback-linked` now recognize only
the matching scope with a validated persisted receipt and request reboot
without invoking Snapper again. Before rebooting, they verify whether the exact
target is already the running root; if so they durably confirm that boot and
advance to health/replay instead of causing a repeated reboot. Missing or
unexpected root identity remains a hard stop. The restore journal exposes the
persisted handoff state as a small predicate; the linked rollback fixture
verifies it is true after receipt persistence and false after boot-target
confirmation. Focused rollback and resume tests, recovery CLI check, Clippy
with `-D warnings`, formatting, and `git diff --check` pass. This remains
host-level state-machine evidence; the native Snapper/reboot path still
requires the exact disposable VM.

Follow-up review hardened the same retry path: if the selected target is
already booted, recovery now validates it against the source receipt, stops
the affected account writers, and records `RootRollbackComplete`. If the
running root instead matches the saved pre-rollback checkpoint, it resumes the
reboot request. Missing target/backup artifacts or any unrelated root identity
remain errors and do not trigger a reboot. The CLI check and Clippy pass; the
process-exit restore-phase test, configuration-apply replay test, and uncertain
native rollback no-retry test pass. These are fixture/process tests, not native
Snapper or reboot evidence.

The native checkpoint verifier now distinguishes `RollbackTargetNotBooted`
from corrupt or unavailable target/backup metadata. Only the former can reach
the pre-rollback checkpoint check and reboot request. Synthetic Btrfs identity
tests cover the separate error class; the native guest remains necessary to
exercise both branches against real Snapper/Btrfs state.

Added a subprocess-exit regression for both system-only and complete-linked
rollback. The child durably records the exact rollback receipt, exits before a
reboot handoff, and the parent reopens the journal to confirm `WritersStopped`,
the target snapshot ID, and pending-attempt discovery. The test passes with
Clippy, formatting, and `git diff --check`. It proves durable journal state
after process exit, not native Snapper completion or reboot behavior.

The reboot-resume branch also performs a fresh Snapper list check after
verifying the saved pre-rollback root. It requires exactly the two snapshots
tagged to this attempt, the target as writable default but not active, and the
backup as retained read-only and neither active nor default. Added focused
inventory tests rejecting wrong tags, changed roles, missing backup, and extra
tagged snapshots. The test, recovery CLI check, focused Clippy, formatting, and
`git diff --check` pass; this remains source-level evidence pending the guest.

Fixed a phase mismatch on retries: complete-linked known-working promotion and
system-only health retries call `verify_running_root_rollback` after the journal
has advanced beyond `RootRollbackComplete`. The verifier now accepts only
receipt-bearing phases appropriate to each scope, including later health and
complete phases, while still rejecting pre-rollback and configuration-only
states. The focused phase matrix and Btrfs identity tests, recovery CLI check,
Clippy, formatting, and diff checks pass. Native linked restore and retention
promotion still need the disposable guest.

`PreparationStores::confirm_booted_root_rollback` and the console
`confirm-rollback` command now also tolerate a process exit after durable boot
confirmation. A retry reopens and verifies the current Btrfs/Snapper target,
preserves the later journal phase, and reports the next scope-specific action
instead of attempting the one-time `WritersStopped` transition again. The same
phase matrix plus a dedicated first-write-versus-retry transition test, recovery
CLI check, Clippy, formatting, and diff checks pass; native console confirmation
remains unverified.

The complete offline workspace command `cargo test --workspace --locked --offline`
finished with exit code 0 after the rollback handoff and confirmation retry
changes. Terminal output was truncated, so no aggregate test count is recorded.
This is host-suite evidence only and does not satisfy the native VM acceptance
gates for issues #22 or #26.

## Continuation update: 2026-10-18 (official asset path recheck)

A fresh official search found the exact pinned Snapshot20261013 names indexed
under `/tumbleweed/appliances`, correcting the earlier Factory path assumption.
Fetching the image from that official path through the approved network boundary
still returned HTTP 404 before any bytes were downloaded. The exact pinned
image, checksum, and detached signature are therefore still unavailable in the
workspace. No alternate snapshot was substituted and no VM was created. The
path discovery and failed fetch are recorded in
`infra/tumbleweed-lab/evidence/issue26-authorized-fresh-preflight-oct18.toml`.

## Continuation update: 2026-10-18 (tools RPM artifacts)

The current staged development source/vendor archives completed `rpmbuild -ba`
for `ryoku-tools`, producing one SRPM and the tools, state-init, GTK UI,
launcher-status, and debugsource RPMs. Package headers were inspected; all
binary names, versions, releases, and architectures match, and the tools RPM
has no scriptlets or triggers. SHA-256 digests and the exact command are in
`infra/tumbleweed-lab/evidence/issue23-tools-source-build-oct18.toml`. The build
used `--nodeps` and a dirty-worktree archive, so it does not satisfy the clean
source, native BuildRequires, signed dependency resolution, installation, or
VM lifecycle requirements in issue #23.

`ryoku-helper` also completed local `rpmbuild -ba`, producing its helper and
debugsource RPMs plus an SRPM from the same staged dirty development archive.
Header inspection confirmed the expected package identities, and no scriptlets
or triggers are present. Digests and the exact command are in
`infra/tumbleweed-lab/evidence/issue23-helper-source-build-oct18.toml`. It used
`--nodeps` and does not satisfy native dependency-resolution or VM acceptance.

Issue #25's tray row previously described a missing Rust StatusNotifier/DBusMenu
provider. Source review corrected this: the pinned, unchanged upstream
`ryoku-shell` Go daemon already implements the watcher, item/menu proxy, QML
`tray` topic and action calls, and the desktop RPM builds that daemon from its
checksummed source archive. No new project-owned Go code is needed. The workflow
matrix now calls this native implementation unverified until the package-owned
session displays and interacts with a real tray item; source and build evidence
are not runtime acceptance. See
`infra/tumbleweed-lab/evidence/issue25-pinned-tray-source-review-oct18.toml`.

Correction to the WM review above: the pinned runtime archive does include
`Ryoku.Ui/Singletons/Wm.qml`, and the unchanged shell daemon publishes its `wm`
topic. The missing pieces were the separate provider executable and two
`Ryoku.Wm.Hyprland` bridge QML files. The desktop RPM spec now declares build
and install steps for the unchanged provider from
`upstream/archives/ryoku-wm-hyprland-98a0368-source.tar.gz`, places it on the
service PATH, and installs those original QML bridges. It
does not call the provider's separate `apply`/`outputs` config writers. The
source manifest is `upstream/ryoku-wm-hyprland.toml`; package build and native
workspace/overview acceptance are not verified. No project-owned Go or QML
was added. Updated evidence is in
`infra/tumbleweed-lab/evidence/issue25-pinned-wm-provider-source-review-oct18.toml`.

## Continuation update: 2026-10-18 (ordinary personal settings)

The user's stated priority narrows the earlier blanket checkpoint rule:
ordinary personal settings must work without a machine checkpoint or
elevation for each interaction, while persistent user configuration remains
in the recovery inventory. `SetLocalProfile` now validates and atomically
writes only the fixed, same-user `opensuse-profile.json` settings document
through the authenticated unprivileged service. The `ryoku-profile set`
command exposes that typed operation. It does not invoke the privileged
helper, create a journal entry, or modify upstream Hub's `profile.json`.
Corrupt or foreign-identity files are preserved and refused. Legacy migration
and protected/system profile activation remain unavailable pending their
separate recovery flows. Focused service and CLI tests passed; this does not
claim native guest acceptance. This update supersedes the earlier
"profile writes held behind linked recovery" decision above.

The full `cargo test --workspace --locked --offline` suite then passed on this
dirty worktree: 661 passed, 0 failed, and 9 ignored across 32 test-result
rows. The captured log SHA-256 and scope are recorded in
`infra/tumbleweed-lab/evidence/issue1-workspace-tests-oct18.toml`. Formatting
and `git diff --check` also pass. Host tests do not satisfy the outstanding
native #22 destructive recovery or #26 desktop lifecycle and console-restore
gates.

## Continuation update: 2026-10-18 (fresh WSL preflight after capacity recovery)

Available RAM is now 9,083,723,776 bytes and the target filesystem has
976,274,296,832 bytes free, both above the runner requirements. Built
`vm-lab` with the `wsl-lab` feature and repeated the exact authorized WSL
preflight for a fresh target run path. It failed only because the exact signed
Snapshot20261013 image, checksum and detached signature are absent. No run
directory, VM, disk or key was created. No QEMU process is running. The
updated result is recorded in
`infra/tumbleweed-lab/evidence/issue26-authorized-fresh-preflight-oct18.toml`.

## Continuation update: 2026-10-18 (current helper RPM)

The prior helper staging archive predated the ordinary personal-settings
writer. Generated a fresh deterministic source archive from tracked and
non-ignored untracked worktree files (excluding this private handoff, `.git`,
and ignored build output) and a Cargo vendor archive from the current locked
dependency set. `rpmbuild -bb --nodeps` passed `%prep`, the offline locked
Rust release build, `%install`, file processing, and emitted
`ryoku-helper-0.1.0-3.x86_64.rpm`. Header inspection verified the expected
helper/service binaries and unit/sysusers payloads with root ownership and
declared modes; no scriptlets or triggers were present. The archived service
source includes `SetLocalProfile` and its fixed local writer. The build used
`--nodeps`, so BuildRequires and Tumbleweed dependency resolution are not
proven, and the RPM was not installed. Full evidence and artifact digest are
in `infra/tumbleweed-lab/evidence/issue23-helper-rpm-build-oct18.toml`; issue
#23 remains open.

## Continuation update: 2026-10-18 (first-run local profile directories)

The previous `SetLocalProfile` implementation still required the account's
`~/.config/ryoku` directory to exist, so a fresh account could not read its
built-in settings or make its first save. The user service now resolves the
authenticated NSS home independently of recovery enrollment. Reads return
generic defaults without creating directories. The first typed settings save
creates only missing `.config` and `ryoku` directories at `0700`, relative to
the pinned home descriptor, after ownership, no-symlink and same-mount checks;
it fsyncs directory entries before atomically saving the private profile file.
The profile path contract and #16/#17 evidence were updated. All 128 service
tests pass (one ignored), service Clippy with `-D warnings`, full-workspace
`cargo check --locked --offline`, formatting, and `git diff --check` pass.
These are host results; native account filesystem behavior and the #22/#26
VM gates remain open.

## Continuation update: 2026-10-18 (final workspace regression sweep)

The latest serial full-workspace run passed with 663 tests passed, 0 failed and
9 ignored across 544 test-result rows. Its captured log SHA-256 and command are
in `infra/tumbleweed-lab/evidence/issue1-workspace-tests-oct18.toml`. One
intermediate parallel rerun hit a transient `WouldBlock` while reacquiring the
retained-run directory lock; the focused test and final serial suite both
passed. This remains host-only evidence. The exact signed Snapshot20261013
image bundle is absent, so the authorized QEMU acceptance run was not started;
issues #22 and #26 remain open.

## Continuation update: 2026-10-18 (native named-ACL recovery probe)

Issue #22's linked-checkpoint guest probe now creates a regular-file POSIX
access ACL with a distinct named principal (UID 65534), requires exact ACL
readback and mode 0640 from capture, and verifies the same ACL after the
configuration-only restore. The VM-lab marker validator requires both ACL
readback and restore markers. The example compiles and passes Clippy with
`-D warnings`; all four linked-probe validator tests pass. This prepares the
native check but does not claim guest ACL behavior: the exact signed
Snapshot20261013 image, checksum and detached signature are still missing.
Official index metadata continues to list those files, while escalated direct
fetches of the checksum from the official origin, the openSUSE-listed UMD
mirror and openSUSE CDN return HTTP 404. No VM was launched.

The recovery library suite was rerun after the probe change: 229 passed, 0
failed and 8 ignored. This verifies the existing host ACL and restore paths but
does not replace the native distinct-principal probe.

A transcript-order review caught that the ACL restore marker was emitted before
the overall configuration-restore marker, contrary to the VM-lab validator's
required sequence. The probe now emits the ACL marker only after the complete
restore and sentinel checks pass, and the validator test rejects reversed
marker order. The focused four-test validator suite passes after this correction.

## Continuation update: 2026-10-18 (retained-run lock test reliability)

The host VM-lab suite had one prior parallel-run failure when the retained-run
lock test reacquired its directory lock immediately after dropping its own
lock. The test now retries only `EAGAIN` for up to 500 ms during that release
boundary; it still asserts a live holder blocks a second supervisor and returns
all other validation errors immediately. The focused test and the full 107-test
VM-lab binary pass, and automation Clippy passes with `-D warnings`. This is
host harness reliability evidence, not a guest run.

## Continuation update: 2026-10-18 (GTK proof waits for splash exit)

The earlier GTK proof was visually rejected because KSplash covered the window
although the app emitted first-paint markers. The guest launcher now checks
that any `ksplashqml` process is the exact RPM-owned executable in the current
authenticated Plasma session and waits up to 60 seconds for it to exit before
starting GTK. The host requires the exact `ui_capture_splash=absent` marker
before sending input or capturing. Public evidence accepts only that exact
marker. The lab remains explicit that framebuffer visual review is required;
this guard has not run in a new guest. The full 107-test VM-lab binary and
Clippy pass.

## Continuation update: 2026-10-18 (linked restore journal reopen probe)

The native #22 configuration-restore fixture now deliberately returns a
health-check failure after the journal reaches `ConfigurationRestored`, drops
the prepared in-memory restore, reloads through `PreparationStores::resume`,
and reapplies idempotently through health check to `Complete`. The host
validator requires the exact restart marker and marker order. This exercises
native filesystem capture/apply and journal reopen once run on the guest; it
is not an OS process kill or a root rollback. The probe passes Clippy, all 107
VM-lab tests pass, formatting and diff checks pass, and the updated evidence
TOML parses. Guest execution remains pending with the missing exact signed
Snapshot20261013 bundle.

## Continuation update: 2026-10-18 (health-failure journal reopen host test)

Added `configuration_restored_journal_reopens_after_health_failure_and_completes`
to exercise the real `PreparationStores` flow used by the native probe. It
injects a health-check error after durable `ConfigurationRestored`, drops the
prepared state, resumes from the journal, and completes the idempotent replay.
The focused test passes. The full recovery library suite passes with 230
passed, 0 failed and 8 ignored; recovery Clippy, formatting and diff checks
pass. This is host filesystem evidence, not the guest ACL/console or root
rollback gate.

## Continuation update: 2026-10-18 (guest evidence retry and process test race)

The GTK proof now rechecks both authenticated session identity and splash
absence immediately before spawning the window, closing the small gap between
the original splash wait and process launch. The exact pinned guest metalink
and meta4 URLs also returned HTTP 404; no signed input was obtained and no VM
was started. A parallel VM-lab run exposed a test race that observed inherited
runner argv before the child completed `exec`; the test now waits for the exact
expected executable and argv. All 107 VM-lab tests and automation Clippy pass.

## Continuation update: 2026-10-18 (workspace Clippy and regression sweep)

Rechecked the open issue list against the first-sprint gate and confirmed the
native #22/#26 requirements still need the exact signed Snapshot20261013
bundle. The user authorized a fresh disposable QEMU run, but the signed image,
checksum and signature remain absent; no substitute image or retained guest
was used. The current serial workspace suite passes with 664 passed, 0 failed
and 9 ignored across 32 test-result rows. Workspace Clippy passes with
`-D warnings`; it identified an ambiguous byte-string escape in the spawned
process test, corrected to encode the argv NUL explicitly. The focused test,
format check and diff check pass. See
`infra/tumbleweed-lab/evidence/issue1-workspace-tests-oct18.toml` for log
hashes. These are host checks only and do not satisfy the native first-sprint
install, reboot, rollback or console restore gates.

## Continuation update: 2026-10-18 (#22 restore process-exit coverage)

Added a spawned-process recovery test for the `ConfigurationRestored` journal
phase. The child reopens the persisted `ContentStaged` attempt, runs the real
configuration restore executor, then exits immediately from the post-restore
health callback with code 77 after that phase has been persisted. The parent
reopens the journal in a fresh process context, confirms the restored bytes,
replays idempotently to `Complete`, and checks an unrelated home sentinel.
The focused test and recovery all-target Clippy pass; formatting and diff
checks pass. The full serial recovery library suite also passes with 231
passed, 0 failed and 9 ignored. This strengthens host process-interruption
evidence but does not replace the required native Tumbleweed run, ACL
verification, root rollback, or offline console acceptance.

The guest #22 probe now follows the same interruption path: after creating a
verified current-state checkpoint, it starts a separate process to resume and
apply the fixture. That child exits immediately from the health-check callback
after `ConfigurationRestored` is durable; the parent then reopens the journal
and completes it. The guest validator requires the process-exit marker before
the restart-complete marker. The probe compiles, its Clippy check passes, and
all four linked-checkpoint validator tests pass. The probe itself remains
unexecuted pending the exact signed guest bundle.

## Continuation update: 2026-10-18 (integrated process-interruption regression)

Rechecked `/workspace/opensuse-vm`; it remains absent. The native #22 probe
now launches a distinct restore process that exits with code 77 only from the
health-check callback after `ConfigurationRestored` is durable. The parent
then reopens the journal and completes the restore. The exact marker sequence
is required by the VM-lab validator. The updated workspace passes serially
with 665 passed, 0 failed and 10 ignored; workspace Clippy passes with
`-D warnings`. Log hashes are recorded in the issue #1 and #22 evidence TOMLs.
No guest execution is claimed.

The guest probe's public markers now follow the validator's exact required
order: configuration restore, child process exit, journal restart completion,
then named ACL restore. A source inspection caught and fixed the prior
ordering mismatch before guest execution. The final current-worktree serial
workspace run passes with 665 passed, 0 failed and 10 ignored, and workspace
Clippy passes with `-D warnings`; hashes are in the issue #1 evidence file.

## Continuation update: 2026-10-18 (uncertain journal append)

Issue #17's operation journal now enters a fail-closed write state if appending
or synchronizing a record fails. A subsequent append is rejected until
`refresh` replays the on-disk journal, avoiding a malformed interior record
after a partial write and resolving uncertain durability after sync failure.
The regression test simulates a durable torn final record, confirms append
refusal, then verifies replay truncates the tail and permits a new durable
record. The focused test and service all-target Clippy pass. The prior full
service library run passed 129 tests with one ignored, before this test was
strengthened. This is host-only evidence; packaged helper activation,
privileged boundary review, and end-to-end protected mutation reconnect remain
open. Results and the current service source hash are recorded in
`infra/tumbleweed-lab/evidence/issue17-service-boundary-host-oct18.toml`.

The strengthened torn-tail regression now passes in the complete serial
workspace run: 666 passed, 0 failed, 10 ignored across 32 test-result rows.
Workspace all-target Clippy also passes with `-D warnings`. The test log hash
is recorded in `infra/tumbleweed-lab/evidence/issue1-workspace-tests-oct18.toml`.
The signed Snapshot20261013 guest bundle remains absent at
`/workspace/opensuse-vm`, so no native VM result is added.

## Continuation update: 2026-10-18 (reconnect after uncertain journal write)

An uncertain journal append could previously leave `GetOperation` serving the
cached pre-error status while the disk contained either a torn or fully written
record. The user service now replays a poisoned journal before dispatching an
authenticated request. A replay failure returns structured
`RecoveryUnavailable`; successful replay makes status lookup reflect the
recovered durable state. The torn-tail regression confirms the previous durable
operation remains queryable after replay. The serial workspace passes with 666
passed, 0 failed, 10 ignored, and all-target workspace Clippy passes with
`-D warnings`. Hashes and logs are recorded in the issue #1 and #17 evidence
TOMLs. Native package activation and protected provider operation reconnect
remain unverified.

## Continuation update: 2026-10-18 (session startup ordering)

For #25, `ryoku-desktop-shell.service` is now ordered after
`ryoku-service.service`, which the Ryoku session target already starts with a
`Wants=` dependency. This ensures the ordinary-settings service's systemd
startup job precedes shell launch without making the service a hard requirement
for the desktop. The unit and README describe the ordering. Static source review
only; `systemd-analyze verify` exits 1 in the managed sandbox after denied
SO_PASSRIGHTS/SO_PASSCRED operations, and no native graphical session was run.
The #25 evidence TOML records the unit hash and limitation.

The request-path recovery now has authenticated Unix socket coverage. After a
torn final append, reconnecting `GetOperation` returns the prior durable status
and replay removes only the torn tail. If replay encounters a complete corrupt
record, the request receives `RecoveryUnavailable`. The focused service test
passes, as does the complete serial workspace suite (667 passed, 0 failed, 10
ignored) and all-target workspace Clippy with `-D warnings`. Evidence and log
hashes are in the #1 and #17 TOMLs. No native guest run is claimed.

## Continuation update: 2026-10-18 (notes state privacy inventory)

Audited the pinned session payload's persistent writers against the first-sprint
capture allowlist. The supported preference files are individually enrolled;
operational history, caches, plugin-defined state, and user-authored command
stores remain excluded. The notes widget writes personal text to
`$XDG_STATE_HOME/ryoku/desktop-notes.txt`; this is now explicitly named among
excluded paths in `docs/recovery-state.md`, with a fixture assertion that
session preference capture does not include it. `git diff --check` passes. The
focused recovery test was not rerun in this continuation. The exact signed
Snapshot20261013 VM bundle remains unavailable, so native #22/#26 gates are
still pending.

## Continuation update: 2026-10-18 (root service sandboxing)

Hardened both the operator helper and session recovery-gate systemd units with
private devices, read-only kernel tunables/modules and control groups, and
`RestrictSUIDSGID=yes`. Both units already had `NoNewPrivileges=yes`, empty
capability sets, and strict read-only system/home protections. Updated
`docs/service-contract.md` and `packaging/rpm/README.md` to record the limits
and the need to review any future protected-store write access. The directives
match systemd's documented service controls; no guest unit activation was
possible because `/workspace/opensuse-vm` remains absent. `git diff --check`
passes; no systemd verification command was run in the host sandbox.

## Continuation update: 2026-10-18 (Ryoshot preference inventory)

The pinned Ryoshot drawing-tool singleton writes
`$XDG_CONFIG_HOME/ryoku/ryoshot.json` with brush style, zoom, copy-on-save, and
save-directory settings. This supported preference file was missing from the
first-sprint inventory. Added a bounded opaque enrollment with an exact writer
ID; the save-directory value does not authorize directory traversal. Updated
the scope documentation and fixture to cover absent and present files, mode
`0600`, and exact payload readback. The focused recovery test passed (1/1), and
`cargo fmt --all -- --check` plus `git diff --check` passed. Evidence is in
`infra/tumbleweed-lab/evidence/issue18-ryoshot-capture-oct18.toml`. This is host
fixture evidence only; native install/recovery acceptance remains open.

## Continuation update: 2026-10-18 (device vibrance preference)

The full pinned session-path audit found `Devices.qml` also persists screen
vibrance as the decimal scalar `$XDG_STATE_HOME/ryoku/nvibrant-value`. Added it
to the exact first-sprint inventory with a 16-byte bound and the same
user-owned regular-file metadata policy as nearby session preferences. The
fixture covers explicit absence, exact `72\n` payload readback, and captured
UID/GID/mode. Recovery
scope docs now identify launcher-frequency and notification histories and the
Ryoku API-key file as excluded private/operational data. The focused test passed
(1/1); formatting and diff checks pass. Updated evidence remains in
`infra/tumbleweed-lab/evidence/issue18-ryoshot-capture-oct18.toml`. Native
Tumbleweed validation remains outstanding.

## Continuation update: 2026-10-18 (Stage wall preference and full recovery suite)

Completed the pinned writer review for Hyprland and the bundled QML paths. The
Stage IPC writer persists the exact per-wall `stage-walls.json` preference
registry, so the bounded inventory now captures that settings document while
excluding generated images, progress records, and referenced assets. Recovery
restore preflight uses the same compiled inventory path/kind policy as capture.
A separate-home restore fixture verifies exact registry bytes and `0600` mode
while preserving an unrelated home sentinel. The full
`cargo test --locked --offline -p ryoku-recovery` run passes: 231 library, 15
integration, 26 CLI, and 2 state-init tests; 9 ignored; 0 failed. The inventory
count assertion is updated to 34 records. Evidence and current fixture hash are
in `infra/tumbleweed-lab/evidence/issue18-ryoshot-capture-oct18.toml`.

Native #22/#26 acceptance remains pending the signed Snapshot20261013 guest
bundle; the approved fresh QEMU run has not started because its image and
signature files are not present.

## Continuation update: 2026-10-18 (external wallpaper settings boundary)

The pinned `OverviewBackdropConfig.qml` reads the `overviewBackdrop` object
from `$XDG_CONFIG_HOME/ryogami-wall/config.json`, outside the declared
`$XDG_CONFIG_HOME/ryoku` root. The QML comment identifies a separate picker as
the writer, but the whole-file schema and producer are not included in the
current inventory review. Kept this mixed external settings document out of
capture and named the exact path and reason in `docs/recovery-state.md` and the
issue #18 evidence record. The wallpaper/backdrop preference is therefore not
covered by the first-sprint recovery claim until its writer and file schema are
reviewed.

The pinned upstream source review refined the prior wallpaper finding: the
producer is available at `ryoku/shell/ryogami/wall-ui/qml/Config.qml` and the
editor at `.../wallpaper/settings/OverviewBackdropSettings.qml` in pinned
revision `98a0368253d9628be72b853985878a5db90e1547`. Its `config.json` mixes
`overviewBackdrop` with API keys, arbitrary user paths, integration output
paths and reload commands. Capturing that whole file would exceed the reviewed
privacy/path boundary. Evidence now records the upstream blob IDs; a future
narrow migration should put only the supported backdrop settings under the
already captured Ryoku `shell.json` before that preference is claimed
recoverable.

The current `ryoku-desktop` RPM exports the Quickshell shell/Hub/UI runtime but
does not package the separate `ryogami-wall` editor. Therefore the external
config is not a first-sprint installed writer path today. Existing old user
configs may still contain its key; preserve the explicit exclusion unless the
app is added to the supported desktop, at which point a narrow migration into
captured `shell.json` is needed. This is recorded in the issue #18 evidence.

## Continuation update: 2026-10-18 (native writer-lock contention probe)

Added a Rust guest scenario to issue #22's `--exercise-failures` mode for the
persistent `mutation.lock`. It opens the existing lock through the verified
protected state-mount descriptor and refuses unsafe metadata; it never creates
or replaces the file. While the parent holds a nonblocking exclusive lock, a
separate probe process must fail with `EAGAIN`; after release, another process
must acquire and release the same lock. The host VM runner requires the exact
`native_linked_mutation_lock_contention=passed` marker. `cargo fmt --all`,
`cargo check --locked --offline -p ryoku-recovery --example
native_linked_checkpoint_probe`, `cargo check --locked --offline -p
ryoku-automation --bin vm-lab`, and `git diff --check` passed. No tests were run
in this continuation, and the native scenario remains unexecuted pending the
signed Snapshot20261013 bundle. Evidence is in
`infra/tumbleweed-lab/evidence/issue22-recovery-library-tests-oct18.toml`.

The native `mutation.lock` contention sequence checks the short-lived
recovery-read/exclusive-writer boundary: while a shared recovery-state reader
lease is held, the child exclusive writer must receive `EAGAIN`; it repeats
under an exclusive writer hold, then proves acquisition succeeds after release.
This does not claim a desktop-lifetime session lease; the recovery gate releases
its shared lock before session startup. The probe exposes only one aggregate
success marker after all three checks. Source audit confirms `ryoku-state-init`
ensures the existing lock on the initialized-state path before reporting
initialization success; its source SHA-256 is recorded in the #22 evidence TOML.
That audit exposed a metadata inconsistency: recovery initialization did not
check the lock group, while the session opener and probe require `root:root`.
The recovery lock initializer now enforces `root:root`, mode `0700` on the
protected state directory and `root:root`, mode `0600` on the lock. The recovery
library compiles with the change; tests were not run. Source hash and check
output are recorded in the #22 evidence TOML. A follow-up audit applied the same
group check to service lock open/create paths and the recovery CLI shared-reader
path and guard revalidation; generic non-root lock fixtures require the file
group to match their directory group. The service library and helper compile
with this final check; no tests were run.

## Continuation update: 2026-10-18 (wallpaper state-root path correction)

Corrected the #18 exclusion inventory after checking the pinned daemon's state
path resolution: `ryoku-wallpaper`, `ryoku-ryowalls.json`, and the generated
`ryoku-live-frames/` cache are directly under `$XDG_STATE_HOME`; the legacy
single-frame file is `$XDG_STATE_HOME/ryoku-live-frame.png`. The preference
files remain excluded because their producer is not packaged and its writer
contract has not been reviewed. The generated frame cache is rebuildable and
source media remains user-selected. `docs/recovery-state.md` and the #18
inventory evidence record the exact paths and scope. No tests were run for this
documentation correction.

The user authorized one fresh disposable QEMU run. The runner has not created a
run directory, VM, disk, or key: the exact signed Snapshot20261013 qcow2,
`image.sha256`, and `image.sha256.asc` are absent. The isolated pinned GnuPG
home is present at `target/wsl-lab-assets-oct18/gnupg/`. The live official
appliance index now lists Snapshot20261014 and no longer lists Snapshot20261013;
no newer snapshot was substituted. To unblock acceptance, supply the exact
image and both sidecars from the original artifact source in
`target/wsl-lab-assets-oct18/` alongside `gnupg/`; then rerun the signed-input
preflight before allowing launch. See
`infra/tumbleweed-lab/evidence/issue26-authorized-fresh-preflight-oct18.toml`.

## Continuation update: 2026-10-18 (workspace target compilation)

`cargo check --locked --offline --workspace --all-targets` passed against the
current worktree, compiling every workspace package and target, including the
recovery CLI, session binaries, service/helper, UI, and VM-lab automation. This
is host compilation evidence only; no tests were run in this continuation and
it does not satisfy #22 destructive native recovery or #26 desktop lifecycle
acceptance.

## Continuation update: 2026-10-18 (current-source RPM builds and lint)

Built the current-source `ryoku-profile-defaults` noarch RPM and inspected its
payload and embedded digest. Built the current-source `ryoku-helper` and all
four functional `ryoku-tools` RPMs (tools, state-init, GTK UI, launcher status)
from a fresh development archive and offline vendor tree. RPM inspection found
the expected payload paths and root ownership/modes; functional packages have
no scriptlets or triggers. Artifacts and hashes are recorded in the #23
profile-defaults, helper and tools evidence TOMLs. These are development-source
builds, not signed artifacts or clean installs.

`rpmbuild --nodeps` was required because the runner RPM database lacks native
`rust`/`cargo` packages even though the Tumbleweed rustup compiler works. A
repository refresh failed because the sandbox package-key path is read-only;
`sudo zypper refresh` requires an unavailable password. Thus native
`BuildRequires` resolution, dependency solving and installation remain open.
`cargo check --locked --offline --workspace --all-targets` and
`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
passed on the current worktree. TOML evidence parsing and `git diff --check`
also passed; no tests were run in this continuation.

Issue #22/#26 native acceptance remains pending the exact signed
Snapshot20261013 bundle. No VM, disk, key, or run directory was created.

## Continuation update: 2026-10-18 (#19 catalog lock metadata)

The Snapper catalog lock opener now checks the lock's GID against the protected
directory and requires a single hard link, in addition to regular-file type,
expected UID and exact `0600` mode. This aligns catalog serialization metadata
with the fixed protected lock invariant and rejects mismatched/aliased lock
inodes on open. `cargo fmt --all -- --check`,
`cargo check --locked --offline -p ryoku-snapper --all-targets`, and
`git diff --check` passed. No tests were run. Host compile evidence is in
`infra/tumbleweed-lab/evidence/issue19-snapper-catalog-lock-host-oct18.toml`;
native Snapper/catalog behavior remains unverified.

## Continuation update: 2026-10-18 (#19 catalog lock path identity)

Extended the Snapper catalog lock hardening: each catalog read/replace now
checks the lock descriptor against `.catalog.lock` before waiting for `flock`
and again after acquisition. A changed device/inode, unsafe type/owner/group/
mode/link count, or unsafe protected directory fails closed; a post-acquisition
failure releases the flock. `cargo fmt --all -- --check`,
`cargo check --locked --offline -p ryoku-snapper --all-targets`, and
`git diff --check` passed. No tests were run. The updated host evidence records
the final source hash; native catalog replacement/interruption behavior remains
unverified.

A follow-up #19 audit applied the protected-directory group and single-link
invariants to `catalog.json` reads as well as `.catalog.lock`; catalog reads
now revalidate the root descriptor's directory type/owner/mode before opening
and validate the data file's type/UID/GID/mode/link count before parsing.
Formatting, Snapper all-target compilation, and `git diff --check` passed; no
tests were run. The #19 host evidence file contains the final `catalog.rs`
source hash and both lock/data-file checks. Native behavior is still pending.

The same #19 retention audit found manually pinned catalog entries bypassed the
`root` Snapper-config check used for normal prune eligibility. Manual checkpoint
marking now requires `root`; any already-pinned row with a different config is
kept and routed to reconciliation rather than treated as a valid partner. The
Snapper crate compiles with formatting clean; no tests were run. Evidence now
records the retention-planner source hash and policy.

A final #19 correlation audit changed an exact-tagged `single` snapshot from
ignored to ambiguous. A transaction with a pre/post pair plus any extra single
record now requires reconciliation instead of reusing the pair as unambiguous.
Formatting, Snapper all-target compilation and `git diff --check` passed. No
tests were run; the #19 host evidence records the updated source hash and policy.

The Snapper catalog lock now also opens with `O_NONBLOCK` in both create and
existing-file paths, so an unsafe replacement type cannot stall before its
metadata is checked. Formatting, Snapper all-target compilation and
`git diff --check` passed; no tests were run. The #19 host evidence records the
final catalog source hash and open flags.

## Continuation update: 2026-10-18 (#25 Rust clipboard packaging)

The Release-10 desktop RPM source applies the checksum-pinned upstream QML
routing patch, declares build/install for `ryoku-clipboard` under libexec,
installs its user unit, and starts it from the Ryoku session target. The
separate upstream daemon patch disables its clipboard startup call so only the
Rust watcher runs. `cargo build --locked --offline -p ryoku-session
--bin ryoku-clipboard --release`, `rpmspec --parse`, a patch dry-run against
the verified runtime archive, and `git diff --check` passed. No tests or VM
run were performed. Evidence is in
`infra/tumbleweed-lab/evidence/issue25-rust-clipboard-package-prep-oct18.toml`;
native clipboard/UI behavior and full RPM install acceptance remain pending.
A full `rpmbuild -bb --nodeps` before the upstream clipboard-startup patch
compiled the packaged Rust session binaries from the pinned vendor tree but
stopped at the preserved upstream Go daemon because no Go toolchain is
installed on this host; no RPM artifact was produced. The final
`rpmbuild -bp` passed all source hash checks and applied the three QML patches
plus the upstream clipboard-startup patch with zero fuzz. The authorized
distro install attempt for `go` could not proceed because sudo requires a
password unavailable to this runner.

## Continuation update: 2026-10-18 (#24 RPM path and ownership preflight)

The read-only installer now rejects non-canonical payload path spellings in
incoming and installed RPM inventories, and uses tagged JSON-escaped RPM query
records for installed file ownership. This prevents control bytes inside an
installed filename from forging package-owner rows used by overwrite checks.
All 17 `ryoku-install` tests passed; the binary build, Clippy with warnings
denied, formatting, evidence TOML parsing, and `git diff --check` passed. The
full locked offline workspace test command also exited successfully. Source
and evidence are recorded in
`infra/tumbleweed-lab/evidence/issue24-installer-directory-preflight-host-oct18.toml`.
This remains preflight-only host evidence; #24 mutation acceptance still
depends on the native #22 recovery gate and #23 package installation proof.

## Continuation update: 2026-10-18 (#14 feature-enabled GTK proof build)

The optional GTK proof target now compiles on the host with the actual `gtk-ui`
feature enabled. `cargo check --all-targets`, all 25 UI tests, Clippy with
warnings denied, and a standalone `ryoku-ui-gtk` build passed. The full
`cargo test --workspace --all-features --locked --offline` run also exited 0,
and full-workspace Clippy with all features/targets and warnings denied passed.
The binary hash and commands are recorded in
`infra/tumbleweed-lab/evidence/issue14-gtk-proof-host-build-oct18.toml`.
This establishes host compilation only: the prior GTK attempts could not open
the WSLg or X11 display, and native Tumbleweed interaction, accessibility,
focus, theme/scale and linked VM proof remain open for #14.

## Continuation update: 2026-10-18 (#23 refreshed tools RPM build)

Rebuilt the tools RPM set from a fresh current-worktree source archive after
the #24 installer path and RPM-ownership preflight hardening. `rpmbuild -ba
--nodeps` compiled the release tools and emitted the source RPM plus all five
binary RPMs. Header/payload inspection confirmed the state-init RPM contains
both `ryoku-state-init` and `ryoku-install`; all four functional binary RPMs
have empty scriptlet, trigger and filetrigger queries. Hashes and scope are
recorded in
`infra/tumbleweed-lab/evidence/issue23-tools-source-build-oct18.toml`. RPM
dependency checks were bypassed because native RPM Rust/Cargo build
requirements are unavailable; this does not establish dependency resolution,
installation, or VM acceptance. #23 remains open.

The helper RPM was also refreshed from the same current source archive using
`rpmbuild -ba --nodeps`. Both release binaries compiled, and the RPM contains
the helper/service binaries, recovery-gate units, user service unit, and
sysusers declaration. Its scriptlet, trigger, and filetrigger queries are
empty. Hashes are in
`infra/tumbleweed-lab/evidence/issue23-helper-source-build-oct18.toml`.
Dependency resolution and native install/socket acceptance remain open.

The #24 tagged JSON RPM ownership query format was also checked against the
complete host RPM database with RPM 4.20.1: 809 packages and 44,407 file paths
decoded with valid record boundaries, canonical absolute paths and no control
bytes. This confirms query-format compatibility on the host; it does not run
the privileged installer or establish mutation acceptance. Results are in
`infra/tumbleweed-lab/evidence/issue24-installer-directory-preflight-host-oct18.toml`.

The data-only `ryoku-profile-defaults` source and noarch RPMs were rebuilt from
the same current-worktree archive. The payload manifest records the expected
`profile.json` digest and root-owned `0644` mode; the RPM has no scriptlets.
Artifact hashes are in
`infra/tumbleweed-lab/evidence/issue23-profile-defaults-build-oct18.toml`.
This build does not establish signed dependency resolution or VM installation.

Rebuilt the current `ryoku-session` release binaries with the locked offline
workspace command required by the desktop RPM spec. Hashes are in
`infra/tumbleweed-lab/evidence/issue23-desktop-rpm-prep-oct18.toml`. The host
still lacks the pinned Qt development stack and Go 1.26, so no current full
desktop RPM build or VM install is claimed.

A fresh `rpmbuild -bp --nodeps` of the current desktop spec passed after
verifying the pinned runtime, daemon and Hyprland provider archives. All four
patches applied with zero fuzz (the clipboard-startup patch used a one-line
offset), and the spec embedded in Source0 matched the working spec exactly.
Hashes are in the same #23 desktop evidence TOML. This is source preparation,
not a complete desktop RPM build.

An RPM file-array cross-check of the six current-source helper/profile/tools
packages found 47 unique paths, no non-directory conflicts, one shared
`/usr/libexec/ryoku` directory, and no payload under `/home`, `/etc`, `/var`
or `/run`. The versioned artifact list and limitations are in
`infra/tumbleweed-lab/evidence/issue23-current-rust-rpm-ownership-oct18.toml`;
the desktop/runtime set is not part of this check.

Corrected the #22 generation payload ENOSPC regression so its injected writer
accepts a partial prefix and then returns raw ENOSPC through `Write::write`.
`write_all` surfaces `ErrorKind::StorageFull` and the production
`GenerationError::Io` conversion. The focused test passes and still proves the
partial stage cannot publish a generation/catalog entry or be retried implicitly.
Source hash and scope are in
`infra/tumbleweed-lab/evidence/issue22-recovery-library-tests-oct18.toml`.
This is injected host I/O evidence, not actual filesystem exhaustion in the VM.

Reviewed the recovery-gate mount path and closed a consistency gap: it now
opens the recovery journal directory from the same pinned, validated
read-only state-mount descriptor used for the shared mutation lock. This keeps
a mount revalidation from separating the lock inode and journal scan across
different mount instances. All five focused recovery-gate tests, service
Clippy with warnings denied, formatting, and `git diff --check` passed. The
source hashes and scope limit are recorded in the same #22 evidence file.
Native mount-substitution and session-start acceptance still require the exact
signed Tumbleweed VM image bundle.

Rechecked retrieval of that pinned bundle. Search metadata still exposes the
20261013 Cloud QCOW2 mirror entry and its 304 MiB size, but the official
`/tumbleweed/appliances` download URL and checked UMD, FAU and factory paths
return 404; the official mirrorlist endpoint returns 425. The live official
appliance index now lists 20261014 and no longer lists 20261013. No image bytes
were obtained and no guest was launched. Keep the pinned
`b9dcb17c0c7e81e153a12d35ea4d86a516b4d2139c6a82ddd75aa6ce3cb9db3a` baseline;
the next native acceptance step needs the original signed image/checksum
bundle supplied locally. The fresh-QEMU preflight evidence records the probes.

## Continuation update: 2026-10-18 (reuse the current guest)

User preference: keep and reuse a stopped VM for follow-up acceptance and
avoid repeated clean provisioning. The full #26 Snapshot20261016 lifecycle has
already passed; its package provisioning transaction took about 12 minutes
under QEMU TCG. Do not repeat that lifecycle just to validate incremental
source changes. The stopped, migrated #22 guest disk at
`target/ryoku-current-20261016-issue22-retry2/disk.qcow2` is still present
(about 1 GiB allocated, 20 GiB virtual) with matching `OVMF_VARS.fd`; no QEMU
process is running. Resume this disk for suitable follow-up checks. Use a fresh
VM only when the acceptance specifically requires a pristine signed-image
baseline or the retained disk cannot safely support the scenario. The existing
disk contains a partial #22 linked-checkpoint fixture; its missing catalog row
was fixed in the probe source after that run, so assess fixture state before
resuming and do not report #22 guest acceptance as passed without rerunning it.

## Current issue gate audit: 2026-10-18

GitHub still has #22 and #26 open. #22 explicitly requires destructive VM
coverage for corrupt archives/manifests, traversal, symlinks, metadata and
absent-file restoration; storage exhaustion, capture failures, missing partners
and concurrent writers; interruption/restart at every implemented restore
phase; and complete managed-state plus unrelated-home comparisons. Its
acceptance also requires every backup failure to block mutation, verified
root/Ryoku partner restoration with catalog survival, and exact image/layout
scope in the report. The present partial guest run proves several linked
readbacks but stopped at `CatalogEntryMissing`; the issue22 probe source now
inserts the exact fixture catalog row after publishing the generation, but this
source change has not been guest-run. #22 remains incomplete.

#26 explicitly still needs repeat-install preserving user overrides, KDE
fallback, and the linked-checkpoint console restore within the fresh-install,
login and reboot scenario. The Snapshot20261016 lifecycle evidence proves the
migration, KDE login, reboot, offline Snapper restore, and fresh reset only;
these remaining checks are not proven. The first-sprint issue remains open.

Revalidated the retained #22 disk: it is a standalone, non-corrupt qcow2 of
20 GiB virtual size and about 1 GiB allocated; there is no `migration.qcow2`,
and no QEMU process currently owns it. Its private directory contains a stale
`qmp-private.sock` left by the failed run. Resume tooling must not blindly adopt
that stale socket or claim the development-stopped receipt that this run lacks.
The guest fixture already has a partial linked generation/journal, so any
follow-up must either use an explicitly scoped, invariant-checked continuation
path or leave this run as failure evidence. Do not rerun a clean lifecycle just
to test an incremental source fix.
