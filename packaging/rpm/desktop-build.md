# Pinned desktop RPM inputs

## Ryoku 1.0 source refresh status

The current source review pin is Ryoku v1.0.0 at
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. Source2 and Source3 now export
that exact tree with verified pinned font assets, updated hashes, and an
issue 18 launch patch rebased to the new helper paths. Native package build,
exact Tumbleweed dependency resolution, actual SDDM session acceptance and VM
recovery remain open in [#173](https://github.com/GenieRobot/ryoku-opensuse/issues/173).
The old #23 result remains evidence for its original 0.75.5 artifacts only.

Issue #23 packages the pinned upstream desktop and existing Rust components.
The `ryoku-desktop` metapackage selects the runtime, immutable profile defaults,
service/helper and recovery inspector. Its runtime subpackage preserves the
shell, Hub, shared QML, translations, Blobs/Spatial3D native modules, and the
Ryogami wallpaper picker; the fonts
subpackage owns only its dedicated font directory. Quickshell is built from
official unchanged source in the private `/usr/libexec/ryoku/quickshell`
prefix, leaving the distribution's `/usr/bin/qs` independent.

The login session, Rust session adapters, Hyprland defaults, KDE fallback
selection and session units belong to issue #25. They are absent from this
package set because #25 depends on #23. Building these RPMs requires no
`suse/session` crate. Packaging upstream controls does not make their writes
supported or authorized; supported integrated writes still require the common
authorization and verified pre-change capture contract.

## Source closure

All RPM builds use local archives, exact source identities and disconnected
Cargo/Go/CMake dependency resolution. The source RPMs retain their corresponding
inputs and spec files. The source archive convention is documented in
[README.md](README.md#reproducible-source-inputs).

| Input | Source identity |
| --- | --- |
| `ryoku-opensuse-0.1.0.tar.gz` | Exact committed port revision, normalized `git archive` with prefix `ryoku-opensuse-0.1.0/` |
| `ryoku-opensuse-vendor-0.1.0.tar.xz` | Vendor archive from that revision's exact `Cargo.lock` |
| `ryoku-runtime-a59d051.tar.gz` | Selected Ryoku 1.0.0 tree exports plus 18 individually SHA-256-pinned font assets; archive SHA-256 `8aeff20645c14af76f3d341e5d4e7c31923fd249f6671caa704c8b23a6c84e65`; 54,073,125 bytes; 4,472 files including generated provenance/checksums |
| `ryoku-writers-a59d051.tar.xz` | Exact 5,861-file export from tree `06b2962f82f43118c3a98372c2a4b435b689b725`, including Go vendor closures, session helper scripts, Ryogami picker and live wallpaper source; archive SHA-256 `7b68eb2e1a81fe3d6b28edaa89708b2c6663082e72f74bdce4087eb02799c9be` |
| `issue14-quickshell-0.3.2.tar.gz` | Official Quickshell `4f508be500dea6e5732cc3d50382a0048b17e7b1`; SHA-256 `5e8a7b7a1a1e40de641ee6cfd24f40aa612e13c27353e5746fc79b2861ca9fd0` |

Both desktop specs check their external archive hashes before extraction. The
desktop spec also verifies Source2's complete `SHA256SUMS` before building the
expanded Blobs module and the Spatial3D module. It installs both full generated
module trees, including shaders and plugin metadata, and the Ryogami picker
module built from pinned upstream C++/QML sources. Generated CMake build trees
and source maps are excluded.

Source3 is selected by `upstream-writer-paths.txt` from the official pinned
upstream tree. The complete recursive GitHub tree manifest for v1.0.0 has SHA-256
`5b5fbc92aae8082ba29135a350de5887078e13fd994b74b59c895b190aa9c774` and root
tree `06b2962f82f43118c3a98372c2a4b435b689b725`. The selected 5,861 files were
checked for exact path set, Git blob identity and executable modes. The archive
is sorted, owner/group 0, and timestamped at upstream commit epoch `1792838837`.
Retain the API manifest, audit output and source digests alongside build logs.

The desktop spec applies the rebased issue #18
`upstream/patches/issue18-writer-launch.patch` to Source3. It routes the v1.0
Ryoshot and session-intro helpers through fixed Rust writer roles and pins IPC
and Ryogami launches to the private Quickshell path. The shell IPC, Hyprland
provider, Hub backend and Ryogami daemon build with Go 1.26.5 or newer using
their committed vendor directories:
`GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0`,
`-trimpath -mod=vendor -buildvcs=false`. No module resolution downloads or
upstream installer run during the build. The binary RPM preserves upstream
notices, vendored dependency license/notice files and the native Go standard
library license. Source RPMs retain the full selected source/license closure.

The patched shell/Hub QML, Ryogami picker, Ryoshot helper and session-intro
helper accompany these exact binaries. Source2 supplies the 1.0 shared QML,
translations, Blobs/Spatial3D sources and fonts. Upstream host-setup and custom
SDDM scripts remain excluded and are never invoked by package installation or
removal.

The installed provenance is `upstream/ryoku-runtime.toml`. Its previous
20261013 repository observation is historical only; exact current Tumbleweed
dependency resolution and acceptance are still required.

## Ownership and native builds

All payload files are root:root. Defaults are immutable under `/usr/share`;
personal overrides, profiles, journals, machine identities and user homes are
absent from the file lists. Shared `/usr/libexec/ryoku` directory ownership
uses mode 0755 in every package. No package enables a unit, changes the login
session, installs a custom transaction hook or invokes state provisioning.

### Coordinated launch ownership

`ryoku-desktop-runtime` builds and owns the root-owned Rust coordinator
`/usr/libexec/ryoku/ryoku-writer`. Its public `/usr/bin/ryoku-shell`,
`ryoku-wm-hyprland`, `ryoku-hub` and `ryogami` aliases enter this wrapper; the
native upstream binaries exist only below `/usr/libexec/ryoku/upstream/`.
These paths implement the exact issue #18 payload map. Recovery inspection
and explicit initialization remain owned by `ryoku-tools`; they do not own
or build the writer binary a second time.

The fixed QML paths are `/usr/share/ryoku/shell/quickshell`,
`/usr/share/ryoku/hub/quickshell` and `/usr/share/ryogami/shell.qml`. Shared
project modules use Qt's standard `/usr/lib64/qt6/qml/Ryoku` namespace so direct
coordinated launches can resolve them without issue #25 session environment.
The dynamically loaded `Ryoku.Wm.Hyprland` focus/shortcut bridges are also
installed from the verified Source3 WM tree. No distribution Qt module is replaced. Translations remain below
`/usr/share/ryoku/i18n`.

The unchanged Quickshell RPM owns `/usr/libexec/ryoku/quickshell/bin/qs`.
Desktop-runtime owns the relative `qs -> bin/qs` link required by the issue
#18 fixed launch contract. Both own their shared parent as mode 0755. This
keeps original Quickshell build inputs independently auditable.

Tumbleweed's QML requires scanner remains enabled for the complete payload.
The spec explicitly selects its native `--qtver 6` option: an SDK containing
both Qt5 and Qt6 otherwise makes the generator's version guess fail while
rpmbuild can still return success. The Rust inventory audit rejects missing
Qt6 scanner output and unresolved internal module imports.
Its provides generator recognizes `qmldir` files only below `/qt6/qml/`, so
the runtime explicitly provides its two private `shell.services` and
`shell.barkit` modules under `/usr/share`. Quickshell's twelve public modules
used by this payload are statically embedded in its private `qs` executable;
the Quickshell spec declares their unversioned QML provides. Shared `Ryoku.*`
modules retain automatic provides from their standard Qt paths, except the two
shipped `Ryoku.Ui.Singletons` and `Ryoku.PluginKit.Singletons` directory imports:
their upstream qmldir files omit a module line, so their provides are explicit.
The desktop
build requires the native QML generators so missing tooling cannot silently
remove the import requirements. These declarations do not make Quickshell's
private modules available to an unrelated Qt executable.

Quickshell requires Qt private APIs. Its SDK and runtime imports are pinned to
Qt 6.11.2 and RPM generates the native ELF dependency requirements. A rolling
Tumbleweed update with a changed private ABI requires rebuilding and validating
these packages in the compatible full-system transaction. Old RPM evidence
does not establish compatibility with a newer image.

Build the changed specs with native installed BuildRequires as an ordinary user,
serially with bounded parallelism, keeping an independent complete build log
for each. For example:

```console
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/rpmbuild' --define '_smp_build_ncpus 2' packaging/rpm/ryoku-profile-defaults.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/rpmbuild' --define '_smp_build_ncpus 2' packaging/rpm/ryoku-helper.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/rpmbuild' --define '_smp_build_ncpus 2' packaging/rpm/ryoku-tools.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/rpmbuild' --define '_smp_build_ncpus 2' packaging/rpm/ryoku-ui-proof.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/rpmbuild' --define '_smp_build_ncpus 2' packaging/rpm/ryoku-quickshell.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/rpmbuild' --define '_smp_build_ncpus 2' packaging/rpm/ryoku-desktop.spec
```

An unchanged Quickshell artifact may be reused only when its original build
log, SRPM, source/spec hashes and native inputs match this spec, and the chosen
current guest passes normal dependency/ABI resolution, install and reinstall.
Record its original unsigned digest separately from any fixture-signed copy;
do not describe it as rebuilt by the current PR. Release 2 changes its QML
provides and requires a new native build; the old Release 1 artifact cannot
substitute for it.

The helper, tools, UI proof and desktop-runtime specs honor Cargo's standard
`CARGO_TARGET_DIR` for both compilation and installation. An ordinary-user,
native build-root cache outside rpmbuild's package build directories can be
shared between those builds; Cargo still checks source fingerprints and the
locked vendor closure. Record cache origin and complete logs. This does not
reuse a host-built binary or bypass native BuildRequires.

The explicit runtime set has ten packages: defaults, helper, UI proof,
Quickshell, desktop, desktop-runtime, desktop-fonts, tools, tools-state-init
and tools-launcher-status. Debug outputs are recorded separately. Collect
RPM/SRPM hashes, `rpm -qp --requires`, `rpm -qplv`, scripts, triggers and file
capabilities. Compare every non-directory path across packages and against
installed ownership before native installation. Never force-replace local
prototype files. Retain native dependency-resolution, install and reinstall
results with exact guest snapshot and repository/package identities.

Run the read-only Rust inventory command against a directory containing exactly
the ten runtime RPMs, with a fresh output directory:

```console
cargo run --locked --offline -p ryoku-automation --bin rpm-package-audit -- RUNTIME_RPMS FRESH_MANIFEST_DIRECTORY
```

It checks root ownership, immutable `/usr` paths, modes, capabilities, all
scriptlet/trigger classes and cross-package file conflicts, and writes native
RPM identity/file/requires/provides inventories. It permits shared directories
only with identical modes. Native Zypper must additionally check installed
ownership and resolve dependencies in the selected clean target before install.
