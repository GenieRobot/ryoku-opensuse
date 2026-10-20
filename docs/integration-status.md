# Integration status (20 October 2026)

This status refresh supersedes the cleanup snapshot below. The older dated
repository counts and next steps are retained as history, not current facts.
The [roadmap and Luna handoff](https://github.com/GenieRobot/ryoku-opensuse/issues/1)
remain authoritative for fixed decisions and task dependencies. The
[full Luna handoff history](luna-handoff-history.md) retains earlier dated
coordinator records.

## Current coordinator state (20 October 2026)

- `main` and `origin/main` are at
  [`3ac9c7fa0815e995e6b9f2d06a71397c1caca663`](https://github.com/GenieRobot/ryoku-opensuse/commit/3ac9c7fa0815e995e6b9f2d06a71397c1caca663).
- #15 is closed as completed M0 feasibility after merged PRs
  [#162](https://github.com/GenieRobot/ryoku-opensuse/pull/162) and
  [#163](https://github.com/GenieRobot/ryoku-opensuse/pull/163). The initial
  provider surface proposes zero protected writes. Writes without proven
  provider hooks remain unavailable; #15 does not certify a protected write
  or pass the linked-recovery/first-sprint gates.
- #18 is closed after PR
  [#161](https://github.com/GenieRobot/ryoku-opensuse/pull/161). #16 and #17
  are closed. Since #15, #16 and #17 are #19's prerequisites, #19 is the
  earliest ready open implementation task in this dependency path.
- #19 remains open and incomplete. Draft PR
  [#151](https://github.com/GenieRobot/ryoku-opensuse/pull/151) contains a
  one-commit partial based on old base `6c10d9f`; its dedicated worker is
  continuing that work. Keep the PR draft until #19 acceptance evidence passes.
  Its remaining gates are supported Btrfs/Snapper and boot-layout capability
  discovery; typed Snapper operations and protected durable catalog; package
  hook correlation by transaction ID without redundant nested pairs; partner
  presence/verification and manual/known-working retention coordination; and
  evidence that catalog/journal survive root rollback and missing partners
  invalidate complete status.
- Preserve the clean #25 worktree at
  `target/issue25-native-workflows`. No branch or PR totals are asserted here.

The first-sprint deliverable in #1 remains unchanged: a clean Tumbleweed VM
must install Ryoku, support login and reboot, return to KDE, preserve overrides
on reinstall, and demonstrate a verified linked checkpoint plus offline
console restore. #19 is one prerequisite task and does not complete that gate.
All fixed decisions in #1, including Rust for new project-owned executable
code, the verified linked pre-change recovery contract, and preservation of
unrelated home files, remain in force.

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
