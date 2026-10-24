# Reproducing the source and dependency evidence

The source commit/tree and reproducible archive identities are in `export.json`.
`runtime-source-manifest.tsv` and `writer-source-manifest.tsv` record every
exported source mapping/blob, byte digest and executable mode. `source-sites.json`
records the selected non-vendor native source text scan, including candidate
QML import lines. It is a text index, so imports inside embedded scripts and
quoted paths are candidates rather than validated module identities.
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
