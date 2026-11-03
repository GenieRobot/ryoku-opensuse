# Issue #183 preparatory inventory

These data-only artifacts map the complete pinned Rashin source and retained
assistant/usage target. Start with [the capability review](../../../docs/rashin-capabilities.md).
They enable no provider, import no runtime payload and satisfy no behavioral,
package or VM acceptance gate.

- `source-components.tsv`: exactly 338 paths under `ryoku/rashin/`, exact Git
  blobs/modes and existing Source2/3 nonselection copied from #173's reviewed
  ledger, separate `retain-entire-component` target and #183 owner, component
  and capability group.
- `entrypoints.tsv`: 34 literal server registrations, static web serving,
  89 concrete Prowl web-client calls, daemon/alias CLI dispatch and nine inbound
  WebSocket actions. Paths preserve upstream parameter expressions; Prowl query
  arguments/schemas and its external server contract still require review.
- `bridge-references.tsv`: 200 exact external path/blob/mode reference candidates
  across shell, Hub, terminal apps, CLI, WM, install, packages, docs and tests.
  These are discovery evidence, not 200 claimed executable entry points. Vendor
  and historical changelog matches are omitted; `services/AiUsage.qml` is included
  explicitly because the collector consumer need not name its own component.
- `capabilities.tsv`: retained behavior, concrete side effects, Tumbleweed owners,
  authorization/privacy/integrity/recovery requirements and remaining tests.
  Group identifiers join the source and entry-point ledgers. Shared source can
  serve multiple groups; the group is a navigation aid, not proof of an exhaustive
  per-symbol or per-write audit.
- `verification.txt`: source-set/blob/mode/object checks and explicit evidence
  limits. Temporary Rust verification used only cached Git objects; it created
  no runtime implementation, dependencies, VM or downloads.

Source revision: `a59d051f0e8080052216f6c7b9eb7e5928935ce8`, tree
`06b2962f82f43118c3a98372c2a4b435b689b725`. Existing selection evidence is
`upstream/evidence/ryoku-v1.0/ryogami-rashin-disposition.tsv` on #173's
`codex/ryoku-v1-sync` branch at inventory-input commit
`c8a4d844e09fa7075583798cc5e01234d65076ef`; exact source identities, modes and selection bits
were preserved. No compiled archives/specs or that authoritative ledger changed.

To independently obtain the source identity set from a verified upstream clone:

```text
git ls-tree -r a59d051f0e8080052216f6c7b9eb7e5928935ce8 -- ryoku/rashin
git show a59d051f0e8080052216f6c7b9eb7e5928935ce8:ryoku/rashin/backend/server.go
git show a59d051f0e8080052216f6c7b9eb7e5928935ce8:ryoku/rashin/web/src/lib/api/prowl.ts
git show a59d051f0e8080052216f6c7b9eb7e5928935ce8:ryoku/rashin/backend/main.go
git show a59d051f0e8080052216f6c7b9eb7e5928935ce8:ryoku/rashin/backend/ws.go
```

All source paths and line numbers refer to that immutable upstream revision,
not to installed files. Git symlink mode records the original blob, without
dereferencing it. Every core row's target is retained regardless of current
nonselection. Runtime implementation remains gated by open prerequisites #25 and
#173; #20's completed coordinator contract does not certify a Rashin action.
No issue checklist is marked complete by this inventory.
