# CLI/Rashin source archive retention gate

The complete Ryoku 1.0 target retains every CLI and Rashin component. This
#173 tranche extends the **next Source3 export recipe**, preserving exact
immutable source and mode facts while leaving Source2 mappings and RPM
build/install unchanged. It does not substitute source archival for working
protected features under #183 and the existing CLI/provider owners.

At pinned `a59d051f0e8080052216f6c7b9eb7e5928935ce8`, the declared additional
roots are all `ryoku/cli/**` (769 paths, 9,936,015 blob bytes) and all
`ryoku/rashin/**` (338 paths, 3,339,325 blob bytes). Rashin has 332 regular
0644 files, four 0755 files and two Git 120000 icon links. CLI has 769 regular
0644 files. No source path/capability is dropped. Exact group counts, selected
scope, byte sizing and unchanged recipe hashes are in
[`source-retention-plan.json`](../upstream/evidence/ryoku-v1.0/source-retention-plan.json).

The **currently recorded Source3 artifact** still contains 5,861 entries and
has SHA-256 `7b68eb2e1a81fe3d6b28edaa89708b2c6663082e72f74bdce4087eb02799c9be`.
It has not been regenerated here, so the current disposition ledgers correctly
retain the old export-selection facts. The next recipe declares 6,968 source
entries / 108,218,278 blob bytes, including both links. Regeneration must update
its archive hash, manifests, selection ledgers and owner references together;
this tranche does not claim that artifact closure is complete. Rashin's full
338-path target and #183 capability matrix remain mandatory throughout.

## Source-only link boundary

`automation/src/source_link_policy.rs` resolves literal Git link bytes against
selected immutable source paths without touching the host filesystem:

- A link source must itself be a selected Git 120000 entry. Its UTF-8 target
  must be relative, bounded, free of control/NUL/ambiguous platform separators
  and resolve lexically within the archive root.
- The resolved target must be a selected regular Git 0644/0755 blob.
  Unselected/dangling targets, absolute paths, archive escapes, link chains,
  cycles, directory links and submodules are refused.
- The exporter permits these links only in raw Source3; any Source2 runtime
  mapping of a link is still rejected. It writes the literal link text,
  records mode 120000 plus the link Git blob in the manifest, and records the
  link object's SHA-256 and resolved Git blob in `writer-source-links.json`.
  GNU tar preserves the link entry; no dereference option is used.

The two pinned Rashin links resolve to brand assets already selected by
Source3, as recorded in
[`rashin-source-links.json`](../upstream/evidence/ryoku-v1.0/rashin-source-links.json).
Preserving those raw source links does not authorize installed arbitrary links,
file access or a runtime write. The new raw roots are not new Go build targets,
QML runtime exports, service activation or package-installed Rashin entrypoints.
Those implementation/acceptance paths remain owned by #183, #25, #27–29 and
the previously mapped provider issues, preserving their full feature scope.

## Validation and generation gate

The isolated Rust policy's three tests pass locally without dependencies;
workspace formatting and diff checks pass. Exporter integration tests cover
both pinned link texts/target blobs, literal tar type/text, repeat archive
hashes, dirty host links ignored, unchanged runtime selection and refusal before
publishing an archive. Their compilation/full execution is delegated to hosted
exact-head CI because the local exporter dependency cache is unavailable; no
missing dependencies are rebuilt or downloaded here.

No full archive/staging output was generated. Existing runtime/writer staging
measures 134,596 / 108,756 KiB. Added raw source is exactly 13,275,340 bytes;
the predicted next GNU Source3 tar is 114,032,640 bytes before XZ compression.
The plan reserves **1,342,177,280 bytes (1.25 GiB)** for two complete fresh
reproduction runs, retaining both stage trees/archives with one transient tar
at a time and filesystem/metadata margin. Compressed output size cannot be
known until generated; the reserve is a budget, not a measured new artifact.

Before any full regeneration, the coordinator must remeasure available space
against the existing storage guard and authorize that bounded output. Then
generate twice from pinned Git objects, compare archive hashes, verify every
regular blob/mode and both tar link entries, update actual manifests and source
facts, and preserve existing Source2 bytes. Native package/build, licensing,
solver, #183 behavior and #25/#26 acceptance remain separate open gates.
