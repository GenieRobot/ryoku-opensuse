# Integration status (21 October 2026)

This status refresh supersedes the cleanup snapshot below. The older dated
repository counts and next steps are retained as history, not current facts.
The [roadmap and Luna handoff](https://github.com/GenieRobot/ryoku-opensuse/issues/1)
remain authoritative for fixed decisions and task dependencies. The
[full Luna handoff history](luna-handoff-history.md) retains earlier dated
coordinator records.

## Current coordinator state (21 October 2026)

- `main` and `origin/main` are at
  [`b8c68ba675f57dab04d4bc465bb437f5574044e5`](https://github.com/GenieRobot/ryoku-opensuse/commit/b8c68ba675f57dab04d4bc465bb437f5574044e5), merged PR #151.
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
- #17 remains open for installed helper capability execution and Tumbleweed
  activation; managed provider writes remain disabled. Its closure is not a
  prerequisite for #20: #20 explicitly depends on #18 and #19, both closed.
  Therefore #20 is the earliest ready open implementation task.
- Draft [PR #152](https://github.com/GenieRobot/ryoku-opensuse/pull/152) is
  partial #20 work and remains draft. Continue the issue's gates for durable
  preflight/capture/verification/mutation/post-capture phases, serialized
  supported mutations and affected-writer coordination, plan/path
  revalidation before mutation, safe cancellation and accurate failure/history
  states. Demonstrate that every required checkpoint failure blocks mutation
  and concurrent operations cannot mismatch root/config generations; make no
  multi-subvolume atomicity claim.
- Preserve the clean #25 worktree at
  `target/issue25-native-workflows`. No branch or PR totals are asserted here.

The first-sprint deliverable in #1 remains unchanged: a clean Tumbleweed VM
must install Ryoku, support login and reboot, return to KDE, preserve overrides
on reinstall, and demonstrate a verified linked checkpoint plus offline
console restore. Closing #19 does not complete that gate. All fixed decisions
in #1 remain in force: new project-owned executable code is Rust; supported
changes require a verified linked pre-change restore point with mutation
blocked on required capture/verification failure; unrelated home files must be
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
