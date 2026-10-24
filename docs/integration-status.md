# Integration status (21 October 2026)

## Ryoku 1.0 source refresh (24 October 2026)

Upstream Ryoku 1.0.0 is pinned at `a59d051f0e8080052216f6c7b9eb7e5928935ce8`.
The complete 0.75.5-to-1.0 path delta and openSUSE dispositions are recorded
in [the release delta inventory](ryoku-v1.0-delta.md), with every changed path
in [the TSV ledger](../upstream/ryoku-v1.0-path-delta.tsv). Follow-up
[issue #173](https://github.com/GenieRobot/ryoku-opensuse/issues/173) now tracks
payload closure, RPM dependency resolution, writer/session review and new VM
acceptance. Current RPM runtime archives and issue #23 evidence remain on the
previous pin; #25/#26 stay open pending refreshed acceptance.

This status refresh supersedes the cleanup snapshot below. The older dated
repository counts and next steps are retained as history, not current facts.
The [roadmap and Luna handoff](https://github.com/GenieRobot/ryoku-opensuse/issues/1)
remain authoritative for fixed decisions and task dependencies. The
[full Luna handoff history](luna-handoff-history.md) retains earlier dated
coordinator records.

## Current coordinator state (21 October 2026)

- GitHub `origin/main` is at
  [`2d798e194df78e49d602c8e84325364fa6267139`](https://github.com/GenieRobot/ryoku-opensuse/commit/2d798e194df78e49d602c8e84325364fa6267139), the merge of PR #152.
  The coordinator's local `main` checkout was last observed clean at
  `e025c7b8c96e55c6c0582462a1bec2f782fd1a20`, four commits behind `origin/main`.
- #15 is closed as completed M0 feasibility after merged PRs
  [#162](https://github.com/GenieRobot/ryoku-opensuse/pull/162) and
  [#163](https://github.com/GenieRobot/ryoku-opensuse/pull/163). The initial
  provider surface proposes zero protected writes. Writes without proven
  provider hooks remain unavailable; #15 does not certify a protected write
  or pass the linked-recovery/first-sprint gates.
- #18 is closed after PR [#161](https://github.com/GenieRobot/ryoku-opensuse/pull/161).
  #19 is closed by merged PR
  [#151](https://github.com/GenieRobot/ryoku-opensuse/pull/151) at `b8c68ba`.
  Exact-head hosted Rust workspace checks passed; local workspace tests, strict
  Clippy, formatting, cargo-deny and native build passed. The signed Tumbleweed
  run10 acceptance record exited 0 and covers supported layouts, stock hook
  pair reuse, root/additional snapshot identities, catalog/journal persistence
  across actual root rollback, refusal for missing/corrupt/deleted partners,
  manual/known-working pins, and zero-limit cleanup. See the
  [acceptance matrix](https://github.com/GenieRobot/ryoku-opensuse/blob/b8c68ba675f57dab04d4bc465bb437f5574044e5/infra/tumbleweed-lab/evidence/issue19-coordination-20261021.toml),
  [Snapper contract](https://github.com/GenieRobot/ryoku-opensuse/blob/b8c68ba675f57dab04d4bc465bb437f5574044e5/docs/snapper-coordination.md),
  and [hosted CI run](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39622755741/job/119300204507).
- #19 does not claim external ESP coverage or general boot compatibility:
  `FullSystem` covers root plus tracked `/boot` files. The actual-root-rollback
  persistence result uses a synthetic `Planned` journal and does not establish
  a prepared/executable linked restore. Known-working promotion verifies
  boot/storage state, not application health. Hook telemetry is not a
  protected provider write or veto. Catalog partners are excluded from
  automatic Snapper cleanup, but no deletion actor or helper restore dispatch
  is enabled. Console lifecycle remains partial without desktop provisioning
  and login.
- #17 is closed completed through PR #148 and the linked native installed
  acceptance record. Its documented limits remain: managed provider writes
  are disabled; no RPM install or interactive PAM login is claimed.
- #23 is closed after merged PR
  [#149](https://github.com/GenieRobot/ryoku-opensuse/pull/149). The exact
  tested evidence head [`5b6627e`](https://github.com/GenieRobot/ryoku-opensuse/commit/5b6627e386e51b2b5ec6b2b614334db4ccfb1426)
  passed hosted exact-head CI, local formatting, strict Clippy, locked
  all-feature workspace tests, cargo-deny and whitespace checks. All six
  native RPM spec builds passed with BuildRequires enabled. Ten runtime
  package manifests cover 1,607 paths with no conflicts, mutable paths or
  custom scriptlets/triggers; Qt6 scanning found 32 import requirements and
  internal QML imports resolve. Signed Snapshot20261016 clean-target dependency
  download, dry-run, install and same-version reinstall passed. Installed
  manifests matched artifacts, `rpm -V` was empty for all ten packages after
  both transactions, the operator group had empty membership, helper/user
  units remained inactive, and five sentinels were unchanged. See the
  [native acceptance receipt](https://github.com/GenieRobot/ryoku-opensuse/blob/5b6627e386e51b2b5ec6b2b614334db4ccfb1426/infra/tumbleweed-lab/evidence/issue23-native-acceptance-20261021.toml),
  [build and package evidence](https://github.com/GenieRobot/ryoku-opensuse/tree/5b6627e386e51b2b5ec6b2b614334db4ccfb1426/infra/tumbleweed-lab/evidence/issue23-native-20261021),
  and [hosted CI run](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39646896660).
- #23 acceptance is packaging-only. It does not establish session/login/PAM/
  provider execution, production deployment, recovery readiness, Btrfs rollback
  or first-sprint completion. Session adapters remain #25. It does not claim
  byte-identical binaries across separately provisioned SDKs or production
  release signatures.
- #25 remains open and draft. Canonical draft [PR #169](https://github.com/GenieRobot/ryoku-opensuse/pull/169)
  is at exact head [`b02f69b`](https://github.com/GenieRobot/ryoku-opensuse/commit/b02f69bc558a8b9de9931e2075f110b63198bb9e).
  The preceding hosted Rust checks
  [run 39650401347](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39650401347/job/119396914353)
  passed at `c99c08f`; hosted Rust CI for the current head is
  [run 39651972315](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39651972315)
  passed formatting, lint, tests and dependency review. Its configured
  main-only GTK proof artifact job was skipped. Local source checks were rerun:
  formatting,
  all-target/all-feature strict Clippy, 474 top-level plus 17 subprocess tests
  (0 failed, 5 existing ignored), session build and cargo-deny passed.
- BuildRequires-enabled `rpmbuild -ba` still stops before `%prep` because host
  RPMDB lacks `cargo >= 1.99` and `rust >= 1.99`; the failure is preserved.
  Separate non-acceptance mechanics using `rpmbuild -ba --nodeps`, isolated
  temporary directories, and the exact source/vendor archives passed `%prep`,
  locked offline vendored build, RPM `%install` payload staging, and manifest
  checks. `--nodeps` bypassed only missing RPMDB providers;
  it produced unsigned RPM/SRPM artifacts, with 15 root-owned entries and
  empty scriptlets. See the [BuildRequires build log](https://github.com/GenieRobot/ryoku-opensuse/blob/b02f69bc558a8b9de9931e2075f110b63198bb9e/infra/tumbleweed-lab/evidence/issue25-session-rpm-host-build.log),
  [mechanics log](https://github.com/GenieRobot/ryoku-opensuse/blob/b02f69bc558a8b9de9931e2075f110b63198bb9e/infra/tumbleweed-lab/evidence/issue25-session-rpm-mechanics.log),
  [mechanics manifest](https://github.com/GenieRobot/ryoku-opensuse/blob/b02f69bc558a8b9de9931e2075f110b63198bb9e/infra/tumbleweed-lab/evidence/issue25-session-rpm-mechanics-files.tsv),
  and [acceptance receipt](https://github.com/GenieRobot/ryoku-opensuse/blob/b02f69bc558a8b9de9931e2075f110b63198bb9e/infra/tumbleweed-lab/evidence/issue25-session-consolidation-20261021.toml).
  Native BuildRequires/Tumbleweed ABI/install, SDDM/login, reboot/KDE
  roundtrip/fallback, and all core workflows remain unaccepted; no package was
  installed into a guest and no system state changed.
- Earlier staged login/reboot/KDE receipts are historical evidence, not
  acceptance for the exact `b02f69b` candidate. Native RPM install, cold SDDM
  startup, login, reboot, KDE roundtrip and fallback, plus notifications,
  clipboard, audio, network, Bluetooth, screenshots, screen sharing,
  authentication and locking remain pending. No VM was launched for this
  candidate.
- Legacy [PR #156](https://github.com/GenieRobot/ryoku-opensuse/pull/156) is
  closed as superseded by #169. Its unique equalizer, standalone clipboard and
  lock/idle candidates remain unaccepted; its source branch is retained for
  reference. Do not treat those candidates as part of #169. Preserve the
  current #25 branch and `target/issue25-native-workflows` worktree.
- #20 is closed after merged PR
  [#152](https://github.com/GenieRobot/ryoku-opensuse/pull/152) at
  `2d798e1`. Exact-head hosted [CI run 39655902287](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/39655902287)
  and local checks passed: 480 top-level tests plus 17 subprocess checks (0
  failed, 5 existing ignored), strict Clippy, formatting, cargo-deny,
  runtime-boundary test and native fixture build. Final same-input native
  [run11 normal](https://github.com/GenieRobot/ryoku-opensuse/blob/2d798e194df78e49d602c8e84325364fa6267139/infra/tumbleweed-lab/evidence/issue20-run11-normal-rebased-20261021.txt)
  and [run12 interrupted](https://github.com/GenieRobot/ryoku-opensuse/blob/2d798e194df78e49d602c8e84325364fa6267139/infra/tumbleweed-lab/evidence/issue20-run12-interrupted-rebased-20261021.txt)
  both exited 0. Their allowlisted evidence matched except for the expected
  scenario-result row; executable hashes and signed input pins matched. The
  [pair receipt](https://github.com/GenieRobot/ryoku-opensuse/blob/2d798e194df78e49d602c8e84325364fa6267139/infra/tumbleweed-lab/evidence/issue20-final-pair-20261021.toml)
  records pre-change refusal, ordering, serialization, writer leases, durable
  interruption/post-capture-failure history, reboot readback and offline
  fixture recovery.
- #20 scope is console-only. Authenticated run08 verified changed repository
  metadata by detached signature before updating only the #20 pin; empty cache
  metadata now fails explicitly, and generic/#19 pins plus strict SSH
  verification remain unchanged. No provider mutation capability is enabled.
  The results do not establish provider certification, production linked
  restore, multi-subvolume atomicity or desktop/first-sprint acceptance.
- #21 is the next dependency-ready open implementation task: its only explicit
  prerequisite, #20, is now closed. #21's offline console restore acceptance
  remains outstanding. #22 also remains open and depends on #21; the M1 tracker
  #3 and recovery foundation gate are not complete.
No branch or PR totals are asserted here.

The first-sprint deliverable in #1 remains unchanged: a clean Tumbleweed VM
must install Ryoku, support login and reboot, return to KDE, preserve overrides
on reinstall, and demonstrate a verified linked checkpoint plus offline
console restore. Closing #19, #20, #23 or #25 does not complete that gate. All fixed
decisions in #1 remain in force: new project-owned executable code is Rust;
supported changes require a verified linked pre-change restore point, with
mutation blocked on required capture/verification failure; unrelated home files must be
preserved; and private companion data remains outside this public repository.

## Historical Git cleanup snapshot (20 October 2026)

The following details describe the earlier cleanup handoff and are superseded
by the current coordinator state above. They are retained as dated history.

## Repository state

- At the end of Git cleanup, `main` and `origin/main` matched at `6c10d9f`
  (merged #18 work), and the main checkout had no uncommitted changes.
- The only other local branch and worktree is
  `codex/issue25-native-workflows` at `3d90c3c`; its worktree is clean. It is
  unfinished #25 work, not a candidate for `main` yet.
- The remote has 26 branch refs including `main`. The remaining issue branches
  are associated with open work or draft PRs; the Dependabot branch has open
  PR #158. Obsolete merged, superseded, and mixed integration refs were
  retired after their exact contents or full histories were accounted for.
- The old mixed residual snapshot is preserved on
  `codex/integration-residual-snapshot` and in a verified local Git bundle
  under `target/integration-archive/residual-snapshot/`. It contains distinct
  UI client and profile changes as well as code represented on focused issue
  branches. Its tests fail because two referenced recovery modules are absent.
  Do not cherry-pick or merge the whole snapshot; extract and validate unique
  changes on their issue branches when that implementation work resumes.

## Issue and PR handoff

- Issues #10–#14, #16, and #17 are closed. Issue #15 was reopened because the
  required provider hook proof remains absent. Issues #18–#26 and #39 remain
  open; later milestone and tracker issues remain open by design. An issue is
  complete only when its own acceptance checklist and evidence pass.
- Draft integration PRs are [#149](https://github.com/GenieRobot/ryoku-opensuse/pull/149)
  (#23), [#151](https://github.com/GenieRobot/ryoku-opensuse/pull/151) (#19),
  [#152](https://github.com/GenieRobot/ryoku-opensuse/pull/152) (#20),
  [#153](https://github.com/GenieRobot/ryoku-opensuse/pull/153) (#22),
  [#155](https://github.com/GenieRobot/ryoku-opensuse/pull/155) (#21),
  [#156](https://github.com/GenieRobot/ryoku-opensuse/pull/156) (#25), and
  [#157](https://github.com/GenieRobot/ryoku-opensuse/pull/157) (#24).
  Green host checks on an individual branch do not satisfy the VM and linked
  recovery gates for those issues.
- Five existing #23 RPM evidence files were copied byte-for-byte onto draft
  PR #149 at `2992699`. The #25 native-workflows branch received the existing
  Rust clipboard provider commit at `3d90c3c`. Neither change closes its
  issue. The mixed source branch was retired after its unique content was
  represented or archived.
- #26 still requires the fresh-install, repeat-install, KDE fallback, and
  linked-checkpoint console-restore acceptance evidence recorded in its issue.
  Its evidence branch is partial and has no completion claim.

## Continuing work

Read the relevant issue, its prerequisites, and the draft PR before resuming.
Keep each issue's existing work on a focused branch, test the exact committed
tree, and attach its required evidence before merging or closing it. Preserve
the residual snapshot until its distinct changes have been assigned and
validated. Use the existing disposable Tumbleweed VM where its state suits the
scenario; a fresh image is required for a clean-install claim.
