# Issue #15 feasibility acceptance audit

Audited on 20 October 2026 against the live
[#15 body](https://github.com/GenieRobot/ryoku-opensuse/issues/15) and
[#1 fixed decisions](https://github.com/GenieRobot/ryoku-opensuse/issues/1).
The provider experiments are integrated at
[`650e186`](https://github.com/GenieRobot/ryoku-opensuse/commit/650e1866b5a000e6757068428c682aba09821323)
through [PR #162](https://github.com/GenieRobot/ryoku-opensuse/pull/162).

**Recommendation: close #15 after coordinator review of this mapping as an
M0 provider feasibility task.** No provider write is certified, enabled or
selected as a protected first-release operation. The GitHub issue is still
open at this audit; only the coordinator changes its state.

## Scope of the decision

The initial selected surface is read-only: validated product/profile/recovery
status and job history, plus individually proven package/repository/Snapper
queries. A query whose data source or non-mutation behavior is unverified
remains unavailable/awaiting proof. This is the minimum provider capability
set, not a declaration that the overall product or first sprint is complete.

The set of **proposed protected writes for this feasibility outcome is empty**.
The package transaction in the matrix remains a future Rust-owned candidate,
not a selected protected operation. Repository/trust edits, generic system
settings, general Snapper administration and external Myrlyn/Cockpit/YaST
writes are excluded. The existing Rust capability table and
`is_verified_protected_write` predicate continue to reject all initial writes.
No executable behavior is changed by this audit.

Issue #1 still requires authorization, verified matching system and Ryoku
capture, failure-stop, durable execution and recoverability before any
supported integrated write is enabled. A later implementation must supply
that evidence; completion of #15 does not supply it. The negative experiments
cannot be used to waive that rule.

## Literal checklist mapping

| #15 requirement | Evidence and result | Audit disposition |
|---|---|---|
| Inspect maintained supported service, IPC and CLI boundaries for Zypper/libzypp, Myrlyn, Cockpit and available YaST modules. | The [matrix](provider-capabilities.md#interface-coverage-and-integration-choice) names fixed CLI, libzypp notifications, PackageKit D-Bus, Cockpit bridge/helper, Myrlyn Qt/libzypp and module-specific YaST boundaries. The [inspected references](provider-experiments-20261020.md#interface-references-inspected) pin source revisions and distinguish installed packages from merely available packages. | Satisfied as interface inspection. No generic API or unmeasured installed provider is assumed. |
| Map operation, privilege, error/cancellation behavior, integration level and Rust implementation path for every initial candidate. | The [operation matrix](provider-capabilities.md#capability-and-integration-matrix) records these dimensions for each initial candidate. It separates documented errors/cancellation from runtime unknowns. `Cancel`/`AllowCancel`, Myrlyn callback cancellation, operation-specific privilege, client-loss requirements and unsupported broad YaST writes are explicit. | Satisfied as a capability map. Unknown behavior is an explicit activation blocker, not an assumed capability. |
| Prove transaction/configuration hook placement before and after package changes, repository edits and selected administrative writes. | The [real guest experiments](provider-experiments-20261020.md#measured-operations) reached `COMMITBEGIN` and `COMMITEND` around a successful package transaction; the pinned call-site source identifies their placement. A separate denial still allowed a write. Repository add/priority/enable/disable/remove and the selected hostname change produced real before/after state changes with a byte-identical registered plugin and zero frames. | Satisfied as feasibility characterization: the package notifications exist but cannot veto, and this hook is absent from the measured configuration paths. Those paths are rejected for protection. No functioning recovery hook is claimed. |
| Check Cockpit local-desktop/category navigation and any embedding constraints; keep remote listening a separate opt-in. | The [installed-boundary inspection](provider-experiments-20261020.md#installed-cockpit-and-packagekit-boundaries) records the real `cockpit-desktop` helper, its page-path mapping, Packages under Tools, same-origin/documented-component embedding constraints, and the wildcard system socket. The experiment kept that socket masked/inactive and observed no Cockpit listener. | Satisfied as the requested navigation/constraint check. A graphical navigation demonstration and default-install listener behavior were not tested; the local interface remains awaiting proof and is not enabled. |
| Classify operations as verified protected, read-only, unsupported or awaiting proof; record a minimal first-release set. | The [matrix](provider-capabilities.md) defines the four classifications; its [minimal set](provider-capabilities.md#minimal-first-release-operation-set) selects a read-only surface and excludes every unproven write. Typed Rust capabilities preserve that exclusion. | Satisfied. There are zero verified protected writes. |

An infeasible hook is a valid feasibility result when the affected operation
is explicitly excluded. Recording no hook for repository/hostname writes
does not assert that a hook protects them. Unlike Run19, the integrated
experiment actually reached package commit notifications and measured writes.
This is why it resolves the earlier evidence gap without reclassifying
Run19's failed preconditions as successful tests.

## Literal acceptance criteria

| #15 acceptance criterion | Evidence and disposition |
|---|---|
| The matrix clearly distinguishes native controls, embedded panels and externally launched interfaces. | Satisfied by the explicit presentation classes and separate embedded-candidate table. No embedded implementation is implied. |
| Each proposed protected write has an evidenced pre-mutation hook and affected-state inventory; an app-launch checkpoint is insufficient. | Satisfied for the selected set: **no protected write is proposed**. Failed/absent hooks exclude the corresponding candidates. The [affected-state inventory](provider-experiments-20261020.md#affected-state-inventory-and-limits) distinguishes measured state from broader unproven recovery scope. Neither an app launch nor a plugin ACK authorizes mutation. Any future proposed protected write must independently satisfy this criterion and #1 before activation. |
| No new Ruby glue or assumed generic Cockpit REST API is required; infeasible provider paths are explicit blockers. | Satisfied. Project-owned probe code is Rust, upstream components remain unchanged, and Cockpit uses documented interfaces. Fail-open libzypp notifications and bypassing external/configuration paths are explicit blockers to protected integration. |

## Completion evidence and retained limitations

The requested matrix, documented interface references and minimal real hook
experiments are published in the integrated
[matrix](https://github.com/GenieRobot/ryoku-opensuse/blob/650e1866b5a000e6757068428c682aba09821323/docs/provider-capabilities.md),
[report](https://github.com/GenieRobot/ryoku-opensuse/blob/650e1866b5a000e6757068428c682aba09821323/docs/provider-experiments-20261020.md),
and [sanitized evidence](https://github.com/GenieRobot/ryoku-opensuse/blob/650e1866b5a000e6757068428c682aba09821323/infra/tumbleweed-lab/evidence/issue15-provider-boundaries-20261020.toml).
The coordinator should link this audit when updating the issue checklist.

The run's `issue_complete = false` preserves its original completion
assessment. This audit revises the closure recommendation by separating
literal feasibility criteria from future activation requirements; it does
not revise any measured result or claim historical protection success.
The new acceptance-audit section records that separate recommendation.

The existing limits remain: XFS characterization only; no linked capture,
complete recovery inventory, post-state recovery verification or Ryoku
journal proof; no PackageKit/Myrlyn/YaST transaction demonstration; no
trust-key mutation, client-disconnect/restart or runtime cancellation proof;
and no graphical Cockpit launch or embedding demonstration. These block
enabling the affected capabilities. #15's body does not require implementing
those excluded capabilities or successfully protecting at least one write.
The remaining product and first-sprint gates in #1 are unchanged.

## Audit patch validation

This patch changes documentation and the audit metadata only. It starts from
the integrated evidence commit above and runs no new guest experiment.

Passed on 20 October 2026:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked --offline --target-dir /tmp/ryoku-issue15-cargo-target -- -D warnings`
- `cargo test --workspace --all-features --locked --offline --target-dir /tmp/ryoku-issue15-cargo-target`
- `cargo deny --offline check`
- `git diff --check`
- Rust TOML parsing and assertions retaining the failed denial result
  (provider exit 0, probe exit 1, package installed) and successful guest exit.

The workspace test command leaves five existing environment-dependent tests
ignored: native ACL/SELinux metadata, cross-filesystem staging, exact upstream
source/daemon fixtures (two tests), and native `/run/user/UID` ownership.
This audit does not claim those environment-specific tests ran.
