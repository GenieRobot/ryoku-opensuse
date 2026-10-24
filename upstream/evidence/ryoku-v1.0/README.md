# Reproducing the source and dependency evidence

The source commit/tree and reproducible archive identities are in `export.json`.
`runtime-source-manifest.tsv` and `writer-source-manifest.tsv` record every
exported source mapping/blob, byte digest and executable mode. `source-sites.json`
records the selected non-vendor native source text scan, including candidate
QML import lines. It is a text index, so imports inside embedded scripts and
quoted paths are candidates rather than validated module identities.

The area disposition ledgers distinguish current source selection from feature
support: `hub-store-disposition.tsv` records 728 current files;
`ryogami-rashin-disposition.tsv` records 769 current files and bridge inputs;
`cli-lifecycle-disposition.tsv` records 774 current/prior paths (769 current
CLI files plus five upstream deletions). Each records exact Git identities and
selection/install dispositions. `cli-lifecycle-sites.tsv` records 835 original
write/process/probe candidate lines in 138 non-vendor production Go files.
It is a lexical candidate index, not complete behavioral or recovery proof.
The verified upstream Git tree preserves the complete CLI; selected
Source2/Source3 exports currently do not include that root, so complete CLI
source-only archive closure remains pending. These review records remove no
source/package roots and do not reduce the full Ryogami target.

`current-not-selected` describes only the existing export/RPM state. It is not
a permanent feature exclusion. Earlier ledger revisions called these rows
`excluded`; exact source identities and selection bits are unchanged. Retained
target owners are #177 greeter/session guard/lock, #178 Bluetooth link/agent,
#179 Voxtype/Redux, #180 Hub user/appearance, #181 Hub host/network/security,
#182 Store/content/apps, #183 Rashin/usage and #184 phone; #47 remains the app
inventory parent. #25/#175/#176 retain core appearance/provider/hook ownership.
Niri #46 and phased Tumbleweed Wayfire #185 are separate retained future targets.
Only edition-specific platform mechanisms are replaced, with full behavior
and protected-operation acceptance still required under those owners.

The Ryogami/Rashin ledger has explicit `target_disposition` and `owner_issue`
columns separate from its current source/export/install facts. All 338 Rashin
source rows and three selected bridges say `retain-entire-component` / `#183`.
The original source/blob/mode/export fields are unchanged. Current nonselection
must not be interpreted as permission to omit a retained assistant capability;
#183 owns the complete capability/entrypoint/provider matrix after prerequisites,
and #173 retains source/archive/package closure responsibility.

`rashin-source-links.json` pins the two Rashin icon link objects and resolves
their literal relative targets against the immutable Git tree. Both targets
are regular brand assets already selected in Source3. It records no host-link
dereference and does not certify an installed link. The next source-only retention recipe validates
selected internal regular targets and preserves literal Git link metadata;
runtime mappings still reject links. `source-retention-plan.json` separates
the new declared CLI/Rashin roots from unchanged current archive/ledger facts
and records the bounded regeneration budget. Full retained archive coverage
remains open until regenerated and verified.

`hyprland-plugin-boundary.json` pins seven selected upstream blobs and exact
line anchors plus the project policy/recipe inputs for the partial compositor
semantic review. It distinguishes roster writes/code loading, repository
fetch/build and direct provider routes from the Hub query/refusal policy.
It certifies source facts only; #180/#182 and their prerequisites retain the
capabilities and own protected implementation/acceptance.

`shell-font-rights.json` records eleven selected shell font binary identities,
decoded sfnt rights metadata and all active loader references. It establishes
an unresolved redistribution evidence gate, not a legal conclusion or removal
of font choices. `inir-font-license-provenance.json` separately pins all 22
selected iNiR fonts and three adjacent OFL1.1 texts with uniquely named package
copies; focused source/audit checks do not certify native delivery or resolve
the other eleven fonts. `font-license-validation.json` records the focused
nine-test audit/native-spec result, exact input hashes and cached-toolchain
limits separately from pending native package and redistribution acceptance.

`wavecapture-providers.json` and three literal `.xml.fragment` files preserve
the verified primary package-file evidence for `pactl` / `pw-record` plus the
PipeWire PulseAudio capability provider. They retain primary conflicts and
obsoletes omitted by the initial candidate index. Exact RPM Requires/source
contracts are tested; native payload, installed-system solver and audio runtime
acceptance remain open. No package or metadata download was needed.
`wavecapture-validation.json` records exact input hashes, signed-primary field
checks and ten focused audit/native-spec tests separately from those native
acceptance gates.

`picker-license-provenance.json` records the exact selected CMake source,
two embedded TTF resources and their license texts plus picker LICENSE/NOTICE.
Four byte-identical copies have unique package basenames under
`packaging/licenses/ryoku-v1.0/`; this avoids root/picker notice-name collisions
and delivers the two font texts omitted by CMake's TTF-only resource glob.
Rust source checks verify copy hashes and Source3 blob/mode selection; the RPM
audit checks regular license files and the generated runtime license identity.
Native RPM extraction/text comparison and broader asset-license closure remain
required; no native build or redistribution acceptance is implied by the JSON.

`picker-license-validation.json` records the exact audit/spec/lock/provenance
source hashes and focused Rust 1.99 test result: seven passed, including the
native spec-parser case and the new license absence/type/location/duplicate
and byte-provenance checks. The harness reused existing dependency artifacts;
their versions/dep-info matched Cargo.lock and their metadata was accepted by
rustc. It does not claim fresh cache-source verification or replace hosted
workspace/Clippy/dependency checks. The fixture inventories are not a built
RPM; actual installed license bytes and generated module resources remain a
native acceptance gate.

`qml-import-names.txt` is the scanner's complete unique named-module graph,
including transitive modules and writer-only Hyprland/picker QML. The native
scanner parses QML and is authoritative for that named-module inventory.

Official repository root:
`https://downloadcontent2.opensuse.org/history/20261016/tumbleweed/repo/oss/`.
`repomd.xml` SHA-256:
`1d891e33c8abd582b71e579118e6b4c94e5e57ecdc1b77a2a7fc1abf7cc930a0`.
The detached signature verifies with the openSUSE Project key fingerprint
`AD485664E901B867051AB15F35A2F86E29B700A4`.

The signed primary metadata path and compressed SHA-512 are the same identity:
`repodata/00c8e15be77a2a6e1f49e58f3c1c66142e2987fda576cceec54c9db0e9a7f789e43233f12f463d0d5313f941f665928aa45e79cba73de6699b847d560cc454ad-primary.xml.zst`.
Its uncompressed SHA-512 is
`b78e3942f0585d06a9c1d10115e78d23f357b31cc23a95d550b4de46be090a670782c0d23fa10e21bbcfe7f485b1f85250700f6a77b203021c728e22913605df`.
Run Rust `rpm-md-index PRIMARY_ZST COMPRESSED_SHA512 OPEN_SHA512 FRESH_JSONL`,
then its `select` or `provides` mode with the desired package/capability names.
The index is accepted only after both checksums and the declared 52,976-record
XML closure pass.

The package JSON files retain exact names, epochs, versions, releases, arches,
checksums, relative repository locations and dependency/provide relations.
`real-import-packages.json` identifies the five actual Qt/KDE import payloads.
Their downloaded RPM SHA-512s each matched these records, and the verbose
`rpmkeys --checksig` result is retained in `import-rpm-signatures.txt`.
Only an isolated task SDK was extracted; no host RPM installation or KVM was
used. The SDK scanner is the signed `qt6-declarative-tools` binary at
`SDK_ROOT/usr/libexec/qt6/qmlimportscanner` (Qt 6.11.2).

The retained scanner JSON is produced with these arguments:

```text
qmlimportscanner -rootPath PATCHED_WRITER_ROOT/ryoku -importPath SDK_ROOT/usr/lib64/qt6/qml -importPath RUNTIME_ROOT/qml -importPath PATCHED_WRITER_ROOT/ryoku/shell/quickshell -importPath PATCHED_WRITER_ROOT/ryoku/shell/quickshell/shell
```

`PATCHED_WRITER_ROOT` is Source3 after the exact-blob reviewed patch;
`RUNTIME_ROOT` is Source2, verified before any build. Those three placeholders
replace task-local absolute prefixes in the committed JSON. Scanner stderr
is empty. Qt/KDE module paths resolve to actual extracted RPM files. Missing
generated native modules and Quickshell runtime `qs.*` aliases remain explicit;
the source scanner does not execute the engine's URL interceptor or validate
loaded native plugins/ELF dependencies.

`quickshell-systemtray-source.json` records exact file blobs/digests from the
independently hash-checked pinned Quickshell archive: the default-on feature,
module URI/version, static library/plugin link and singleton API. This is the
source basis for Release 3's one additional real QML Provide. The generated
spec metadata was checked using the ignored native `rpm-package-audit` test
`native_spec_parser_preserves_systemtray_provider_and_matching_release`;
the ordinary test compares the complete parsed Quickshell module inventory
against static Provides. Neither test substitutes for building/loading the
refreshed native Quickshell RPM.
