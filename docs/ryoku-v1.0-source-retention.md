# Verified CLI/Rashin source archive retention

The full Ryoku 1.0 target retains complete CLI and core Rashin functionality.
This #173 tranche closes their raw-source archive-input gate, preserving every
immutable path/blob/mode while leaving Source2 and runtime build/install
selection unchanged. Functional protected behavior remains required under #183
and the previously mapped session/lifecycle/provider owners.

At upstream `a59d051f0e8080052216f6c7b9eb7e5928935ce8`, Source3 now contains
all 769 `ryoku/cli/**` entries (9,936,015 blob bytes) and all 338
`ryoku/rashin/**` entries (3,339,325 blob bytes). Rashin has 332 regular 0644
files, four 0755 files and two Git 120000 icon links. CLI has 769 regular
0644 files. Exact original identities and target owners are unchanged; the
current ledgers now record `source-only-archived` with Source3 selected.
Source2 mappings and current RPM build/install targets remain unchanged.

Two independent offline exports produced Source3 SHA-256
`387d829860ba844ad32ecdd5982678707665f3251e41bbb43c6fd6f87dc7eee0`,
34,625,504 compressed bytes and 6,968 source entries. The independent tar/Git
verifier checked all 6,966 regular blob IDs/modes, both literal link objects,
669 directories, UID/GID 0 and epoch 1792838837, with no missing/extra source
paths. Both archives and generated manifests/link/site evidence are identical.
Source2 reproduced `8aeff20645c14af76f3d341e5d4e7c31923fd249f6671caa704c8b23a6c84e65`
and its manifest remained byte-for-byte unchanged. Exact results are in
[`source-retention-reproduction.json`](../upstream/evidence/ryoku-v1.0/source-retention-reproduction.json).
The prior 5,861-entry Source3 hash remains historical evidence in
[`source-retention-plan.json`](../upstream/evidence/ryoku-v1.0/source-retention-plan.json).

## Source-only link boundary

`automation/src/source_link_policy.rs` resolves literal Git link bytes against
selected immutable source paths without following host filesystem paths:

- A source must be a selected Git 120000 link. Its UTF-8 target is relative,
  bounded, free of control/NUL/ambiguous platform separators and resolves
  lexically within the archive root.
- The resolved target must be a selected regular Git 0644/0755 blob.
  Unselected/dangling targets, absolute paths, archive escapes, link chains,
  cycles, directory links and submodules are refused.
- Links are permitted only in raw Source3; Source2 runtime mappings still
  reject them. The manifest preserves mode 120000 and the link Git blob;
  `writer-source-links.json` records literal text, SHA-256 and resolved blob.
  GNU tar preserves link type/text without dereferencing it.

Both Rashin icon links resolve to already selected brand assets, as pinned in
[`rashin-source-links.json`](../upstream/evidence/ryoku-v1.0/rashin-source-links.json).
Raw retention grants no arbitrary installed link/file access. The complete
sources are preserved for the required CLI/Rashin implementations; archival
presence does not activate an agent, collector, service or edition actuator.
The #183 capability matrix at `a50efed8150e28098141cab6f12924844bbc6582`
remains an immutable earlier selection checkpoint; this #173 ledger advances
only its Source3 facts, preserving every path/blob/mode and retained capability.

## Validation and remaining gates

The eight exporter/policy tests cover exact pinned targets, positive tar link
metadata, repeat hashes, dirty host links ignored, invalid-target rejection
before archive publication and byte-for-byte Source2 archive/manifest invariance
when only Source3 scope changes. All eight pass offline, alongside ten focused
package/license/native-spec tests, formatting and diff checks. The exporter
implementation at `211f04004aca3ccd7c81955f42e797e1820242e4` passed hosted
[run 40138454803](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/40138454803);
refreshed-input exact-head checks remain a separate gate.

The offline build and both reproductions occupy 810,958,848 bytes before the
small focused-test binaries, within the authorized 1,342,177,280-byte cap.
No network, package install or VM access was used. Generated archives remain
under the existing ignored target area; only source/evidence is committed.

Per-operation semantic/write/recovery review, complete redistribution/license
closure (including the eleven shell-font blockers), native RPM/plugin/import/
ELF and installed-system solver checks, functional core Rashin and refreshed
#25/#26 acceptance remain open. Raw source input closure completes none of
those behavior/native gates; #173 and PR #174 remain open/draft.
