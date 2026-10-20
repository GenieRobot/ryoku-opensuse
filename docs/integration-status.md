# Integration status (20 October 2026)

This is a handoff for the completed Git cleanup, not acceptance evidence for
the open implementation issues. The [roadmap and Luna handoff](https://github.com/GenieRobot/ryoku-opensuse/issues/1)
remain the source of scope and task dependencies.

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
