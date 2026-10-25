# Retained shell font source and rights evidence

This #173 source/license review preserves the full typography and font-selection
target. It records an unresolved redistribution blocker, not a legal conclusion
or permission to distribute. No font, loader, UI choice or package path is
removed or replaced by this review.

[`shell-font-rights.json`](../upstream/evidence/ryoku-v1.0/shell-font-rights.json)
records all eleven exact path/blob/mode/SHA-256 identities in pinned upstream
`ryoku/shell/quickshell/shell/assets/fonts/` at
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. The blobs total 366,768 bytes.
Every file is selected by Source3; the ledger also records any Source2 export
paths. The current RPM copies the shell QML tree, including this directory.
The selected `shell/modules/desktop/Singletons/Fonts.qml` explicitly loads
all eleven files and exposes their font names as choices; its exact blob and
FontLoader line anchors are recorded. This is shipped/reachable material,
independent of which font a user initially selects.

The evidence decodes sfnt name-table fields directly from immutable Git blob
bytes without installing or executing a font. Identical values are deduplicated
per name ID. IDs 0/1 identify copyright/family, 8/9 manufacturer/designer,
11/12 their URLs, and 13/14 license text/URL. Those strings are source facts;
they do not establish that an upstream tree's general GPL notice licenses
third-party assets or that the required permission texts have been supplied.

| Exact font | Pinned embedded rights evidence / unresolved question |
|---|---|
| `unrealised.ttf` (`6f98cb96fe23e10e71d4217fdd900753fa371793`) | Name ID 13 states personal, non-profit use; commercial use requires a license, referring to included readme/PDF files. The pinned font directory contains no supporting text files. Required redistribution permission/license evidence is unresolved. |
| `vip-rawy-regular.otf` (`8bc9a23a76c4edea77746db95b6d7e109960d423`) | IDs 13/14 contain a copyright/“All Rights Reserved” statement rather than a license grant or usable license URL. Required permission/license evidence is unresolved. |
| `daydream-demo.otf` | License fields contain literal empty quotation marks. The demo filename and those fields establish no permission; supporting terms remain unresolved. |
| `abberancy.ttf`, `jf-flat-regular.otf` | Embedded author/manufacturer references are recorded. Attribution alone establishes no redistribution grant; complete applicable terms remain unresolved. |
| `overhead.ttf`, `reckoner.ttf`, `reckoner-bold.ttf`, `sabana-regular.ttf`, `vexa-light.ttf`, `xenophobia-nominal.ttf` | No license field 13/14 was found. Copyright/family metadata is preserved where present. Missing fields do not prove either permission or prohibition; authoritative terms remain unresolved. |

The pinned directory consists only of these eleven font binaries. The source
review does not fetch current websites or infer a license from a family name,
an author link, a general root notice or a font's availability online. This is
separate from the resolved pinned Ryogami picker font-text recipe fix, and
separate from the OFL texts accompanying the selected iNiR font families.

For those 22 selected iNiR Inter/Montserrat/Rubik TTFs, the three adjacent
license texts explicitly identify OFL 1.1. Their exact blobs, bytes, hashes and
complete font coverage are recorded in
[`inir-font-license-provenance.json`](../upstream/evidence/ryoku-v1.0/inir-font-license-provenance.json).
The runtime RPM copies that QML/font tree, so its License expression now also
includes `OFL-1.1`; the separate external-font subpackage's expression did not
describe these runtime-owned fonts. Three distinctly named byte-identical OFL
copies are added to the existing `%license` directory. The focused audit checks
their pinned hashes/coverage and rejects missing or ambiguous installed license
texts; parsed runtime metadata must include the OFL and picker identities.
These recipe/source checks do not resolve the eleven fonts above or prove a
built RPM's native license delivery.

The focused cached-rustc audit harness passed all nine tests, including the
native `rpmspec` query and positive/negative component-license checks; workspace
formatting and diff checks passed. Exact input hashes, toolchain/dependency
identities and limits are in
[`font-license-validation.json`](../upstream/evidence/ryoku-v1.0/font-license-validation.json).
This reused compatible cached dependency artifacts rather than rebuilding
dependencies; hosted exact-head checks remain a separate validation gate.

#173 owns source/provenance and candidate package license closure; #25 and #180
own the retained desktop typography/settings experience. Payload resolution
requires accepted applicable license/permission evidence for exact binaries or
feature-equivalent licensed assets that preserve the font-selection capability,
with source/payload/license manifests and tests updated under the owning work.
This slice makes neither choice silently. Until that resolution and native
license delivery are verified, candidate redistribution/license acceptance
remains open. Do not treat the current broad RPM License expression or green
Rust CI as closing this asset-specific gate.
