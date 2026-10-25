# Ryoku 1.0 Qt import and native provider evidence

This bounded #173 unit records existing signed Tumbleweed OSS20261016 RPMs,
ELF headers and pinned source contracts. It changes no Requires, source, build
selection or feature. No package/source download, extraction, plugin/command
execution, rebuild or VM operation occurred. Full Ryoku functionality remains
the target, including Spatial3D, video/depth/palette behavior and core Rashin.
These source/provider facts are not native or complete transitive acceptance.

[`qt-native-provider-proof.json`](../upstream/evidence/ryoku-v1.0/qt-native-provider-proof.json)
records seven cached RPM SHA-512s/signature results, six module plugin file
SHA-256s matched to signed RPM header digests, their actual readelf dynamic
sections and ten unique direct SONAME providers. It also binds generated
Spatial3D/Concurrent and static Quickshell UPower source contracts.
[`qt-native-primary-records.json`](../upstream/evidence/ryoku-v1.0/qt-native-primary-records.json)
retains 49 exact signed-primary records, literal package XML fragments/digests,
strong and weak relations, and the authenticated repository/primary identities.
All 52,976 primary records and compressed/open hashes were checked locally.

## Payload ownership is separate from QML capability ownership

| Module URI | Signed native payload / plugin | Signed QML capability provider |
|---|---|---|
| QtQuick3D | qt6-quick3d-imports 6.11.2-1.1; libqquick3dplugin.so | qt6-quick3d-imports |
| QtMultimedia | qt6-multimedia-imports 6.11.2-1.2; libquickmultimediaplugin.so | qt6-multimedia-imports |
| QtQuick.Dialogs | qt6-declarative-imports 6.11.2-2.1; libqtquickdialogsplugin.so | qtdeclarative-imports-provides-qt6 1.0-1.55 |
| Qt.labs.platform | same declarative payload; liblabsplatformplugin.so | same capability helper |
| QtQuick.Controls.Material | same declarative payload; libqtquickcontrols2materialstyleplugin.so | same capability helper |
| QtQuick.Window | same declarative payload; libquickwindowplugin.so | same capability helper |

The helper's signed metadata Requires **qt6-declarative-imports = 6.11.2**.
The declarative payload owns ELF/file entries while the helper declares these
qt6qmlimport capabilities, including legacy/versioned import aliases. Do not
attribute all capability Provides to the payload or fabricate replacements.
The helper record is authenticated repository metadata; its RPM was not newly
fetched or installed. Existing Qt6 automatic Requires remains enabled in the
port spec. The resulting built-RPM Requires and native solver must still prove
that the helper is pulled in for the actual scanned imports.

The six plugin ELF headers directly need ten distinct libraries across Qt
Core/Qml/LabsPlatform/MultimediaQuick/Quick3D/QuickControls2/QuickControls2Material/QuickDialogs2, libstdc++ and
libc. Exact per-plugin sets and each signed provider's name/EVR are in JSON;
these are direct dependencies, not a complete recursive or loaded ABI proof.
The two additional signed payloads are ImageMagick
7.1.2.33-1.1 and ffmpeg 9.0.2-4.1. They own magick/identify/convert and
ffmpeg/ffprobe entrypoints respectively; ImageMagick's two compatibility names
are relative links to magick. No native tool or codec suite ran here.

## Generated Spatial3D and static UPower are different provider contracts

Pinned `ryoku/shell/plugin/CMakeLists.txt` is retained in Source2/native. It
creates **Ryoku.Spatial3D 1.0**, depends on QtQuick3D and links
Qt6::Concurrent, Qt6::Quick and Qt6::Quick3D. The Stage consumer imports both
QtQuick3D and Ryoku.Spatial3D. Signed cmake(Qt6Concurrent) and
cmake(Qt6Quick3D) providers are qt6-concurrent-devel 6.11.2-2.1 and
qt6-quick3d-devel 6.11.2-1.1; the existing BuildRequires name those packages.
Spatial3D is built by this port, not supplied by a guessed distribution QML
package. Its backing/plugin pair, $ORIGIN path, generated qmldir/qmltypes and
automatic Provides still need actual build/install/import/ELF validation.

Quickshell's pinned archive proves default-on Quickshell.Services.UPower 0.1,
Qt::DBus linkage and static plugin linkage into qs. The current
ryoku-quickshell spec has its real explicit QML Provide. Its Qt DBus build
capability is supplied by qt6-dbus-devel 6.11.2-2.1 through the signed
qt6-base-devel requirement chain. The UPower daemon is a separate provider;
the [cava/upower slice](ryoku-v1.0-runtime-providers.md) adds its runtime
requirement without claiming static module load or service activation success.
PowerProfiles implementation choice and protected direct/socket/startup wiring
remain owned by #181/#25/#176/#20. No service selection changes here.

## Keep the two FFmpeg ABI contracts distinct

| Consumer | Exact signed source/provider contract |
|---|---|
| Qt Multimedia 6.11.2-1.2 backend | Requires libavcodec.so.62, libavformat.so.62 and libavutil.so.60; signed standalone providers are FFmpeg 8.1.3-3.1 libraries. |
| ffmpeg CLI 9.0.2-4.1 | Requires ABI 63/63/61, with raw rich alternatives for matching FFmpeg 9 standalone or mini library packages preserved. It is not the Qt backend's ABI 62/60 provider. |
| retained livewall build.sh | Uses generic pkg-config libavformat/libavcodec/libavutil/libswscale and Wayland. The cached SDK packet selects FFmpeg 9 devel packages; source does not force that choice for every build. Actual rebuilt livewall DT_NEEDED has not been measured. |

Separate SONAMEs identify separate contracts. This is not proof of loaded
coexistence, all codecs, hardware acceleration, a successful livewall build or
native source/API compatibility. Do not substitute ABI 63/61 providers for
Qt's 62/60 requirements or infer livewall's actual linked ABI solely from the
CLI package version. Preserve exact SDK resolution, rich dependencies and
both library families when the native solver/build gate resumes.

All seven RPMs and six existing extracted plugins were verified read-only;
only these bounded direct providers are established. Complete generated
requires/recursive solver and native ELF loading, Qt/Quickshell/Blobs/Spatial3D
builds, Ryogami image/video/tool tests, notices/license closure and fresh
session/recovery acceptance remain open. #25/#175/#176 and other feature owners
retain the complete experience; current package selection is not product scope
reduction. No entire #173 checklist row is checked complete by this evidence.

## Enforced candidate metadata gate

The Rust `rpm-package-audit` now requires the generated runtime Requires to
contain exact capabilities for QtQuick, QtMultimedia, Quickshell, QtQuick3D,
Qt.labs.platform, QtQuick.Controls.Material, QtQuick.Window, QtQuick.Dialogs,
Quickshell.Services.SystemTray, Quickshell.Services.UPower, Ryoku.Blobs and
Ryoku.Spatial3D. The previous three-import check could accept metadata missing
the 1.0 native/service imports. Every internal import must also have its exact
provider in the candidate package set. The audit completes validation before
publishing its evidence directory.

Focused omission tests reject each missing required capability, SONAME and
lookalike substitutes, missing internal providers, a wrong Spatial3D module
name and unknown private imports. The expected names are checked against the
pinned scanner output and selected import-name inventory. All ten ordinary
audit tests and the explicit native spec-parser test pass using cached locked
Rust dependencies; this does not rebuild an RPM or run a module.

This tightens the acceptance gate without changing runtime features, payloads,
archives. The native candidate correction below preserves Spatial3D major 1
in the generator capability rather than assuming a bare URI. Generated v1.0 RPM
metadata, versioned capability constraints, recursive signed-snapshot solver
closure, plugin loading and native session/recovery tests remain required; a
passing synthetic metadata test does not satisfy them.

The subsequent [native module install contract](../upstream/evidence/ryoku-v1.0/native-module-install-contract.json)
also makes the RPM audit require eight regular files: each Blobs/Spatial3D
module's `qmldir`, target `.qmltypes`, backing library and plugin library under
`/usr/lib64/qt6/qml/Ryoku`. Their target/output-directory and full-directory
copy contracts are pinned to source and recipe; historical native Blobs output
confirms its four names. Spatial3D uses the same Qt target pattern, with the
refreshed generated output still an explicit native gate. Missing, symlink,
directory, duplicate and lookalike cases fail. Historical Blobs-only payload
cannot pass the 1.0 gate.

This is file/type integrity, not proof of ELF contents, RPATH, ABI, loadability
or correct QML type registration. Blobs shader sources are embedded by
`qt_add_shaders`; the inventory gate assumes no standalone installed `.qsb`
files. Their resource contents and execution remain native acceptance work.
All eleven ordinary audit tests and explicit native spec parsing pass using
cached locked dependencies; runtime sources, specs and archives stay unchanged.

## Native candidate QML metadata correction

The first native runtime RPM built from port `8985d22698e3f73d77a9ee2e9305d438c5f9f6b0`
(SHA-256 `252f4adcc3d233888ef199e654336ebbb541a37f2e26f5283144832c242b1697`)
exposed two separate problems. Its explicit Spatial3D 1.0 consumer generates
`qt6qmlimport(Ryoku.Spatial3D.1)`, satisfied by the same RPM's
`qt6qmlimport(Ryoku.Spatial3D.1) = 0`. The audit now matches that exact major-bearing
capability name, rather than demanding a bare URI or literal equality with the
versioned Provide line. Wrong-major, missing, malformed and lookalike providers
remain failures; constrained internal requirements require separate native solver
validation and fail this gate.

The runtime also ships `shell.modules.phone` and `shell.modules.stage.frame`
qmldirs under its private shell tree, outside the distribution's `/qt6/qml`
provider scan roots. Their exact selected Source3 blobs, modes, module declarations
and original RPM file digests are recorded in
[private-qml-providers.json](../upstream/evidence/ryoku-v1.0/private-qml-providers.json).
The runtime spec declares these two real private capabilities, retaining all
scanning and payloads. The unchanged original native Requires/aggregate Provides
fixtures reproduce the missing-provider failure; tests bind each added Provide
to its selected qmldir and runtime subpackage.

This is source and recipe evidence. A corrected native RPM must be rebuilt and
hashed before rerunning the production audit. The original failing RPM/audit
remain evidence; source regressions do not establish native solver, ABI loading,
phone, editor or full Ryoku acceptance. No feature is removed by this correction.

## Static Blobs shader proof from the first native build

[blobs-shader-native-static-proof.json](../upstream/evidence/ryoku-v1.0/blobs-shader-native-static-proof.json)
binds the original runtime RPM, its packaged Blobs backing ELF, generated RCC
source, all four QSBs and five pinned Source2 source blobs. Streamed extraction
of only that library matches the build hash. All three complete RCC arrays
(data, names and structure) occur in the packaged ELF. The 32,033-byte data
array contains exactly four length-delimited shader payloads; every generated
QSB name, length and SHA-256 matches, with no trailing resource bytes. RCC order
is checked by its own names, independently of filesystem enumeration order.

The initial diagnostic collector mistook the `.qsb` directory for a shader
file. Its failure is retained separately; corrected regular-file selection and
this independent resource proof establish exactly four generated shaders.
This validates static embedding for the original `8985d22` candidate only.
The corrected RPM still needs rebuilding and auditing, and shader loading,
rendering, native ABI/solver and full desktop acceptance remain open. No
candidate executable or plugin was launched by this check.
