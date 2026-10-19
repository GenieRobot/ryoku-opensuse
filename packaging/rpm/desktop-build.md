# Experimental desktop RPM build

These definitions implement the desktop part of issue #23. They are build
inputs, not an installer or an acceptance claim. Issue #14 remains open; the
real KWin-hosted shell and launcher proof does not establish SDDM login,
reboot, accessibility, safe integrated writes, or clean RPM installation.

## Source closure

All source archives are local inputs to an offline RPM build. The source RPM
must retain all four archives and these unpatched specifications.

| Input | Identity |
| --- | --- |
| `ryoku-opensuse-0.1.0.tar.gz` | Native `git archive` of the exact committed port revision, prefix `ryoku-opensuse-0.1.0/` |
| `ryoku-opensuse-vendor-0.1.0.tar.xz` | `cargo vendor --locked` of that revision's `Cargo.lock`, top-level `vendor/` |
| `issue14-runtime-payload.tar.gz` | Ryoku `98a0368253d9628be72b853985878a5db90e1547` exports plus the pinned fonts in `upstream/ryoku-runtime.toml`; SHA-256 `81151cef1a613b01ad8e6187df2e93cbfed70e1621de0ee2e7455d2904fc3e74` |
| `issue14-quickshell-0.3.2.tar.gz` | Official Quickshell `4f508be500dea6e5732cc3d50382a0048b17e7b1`, tree `382743e55b82f3b5837eca36b8158e79c61c319a`; SHA-256 `5e8a7b7a1a1e40de641ee6cfd24f40aa612e13c27353e5746fc79b2861ca9fd0` |

The source/vendor archive recipe is in [README.md](README.md#reproducible-source-inputs).
Use a clean committed checkout that includes the RPM runtime path in the Rust
launcher; that fixed path must be `/usr/libexec/ryoku/quickshell/bin/qs`.
Record the commit, lockfile digest, source/vendor archive digests and normalized
archive timestamps. Existing verified issue #14 archives may be copied into
`SOURCES/` without repacking. Both specs check their fixed external archive
hash before extraction; the desktop spec also checks every original payload
file using its `SHA256SUMS` before compiling native code. No upstream QML,
JavaScript, C++, auxiliary script or font file is patched.

The payload archive's historical `RUNTIME.md` and `RUNTIME-MANIFEST.toml`
describe the initial rejected Noctalia candidate. They stay in the source
archive for provenance but are not installed. The runtime RPM ships the
current repository runtime documentation and manifest. Its documented
`SHA256SUMS` describes the original source input, including source-only files;
installed native Blobs artifacts are recorded by the RPM file digests.

## Ownership

| Package | Owned runtime paths |
| --- | --- |
| `ryoku-quickshell` | `/usr/libexec/ryoku/quickshell/` only, including upstream `bin/quickshell`, `bin/qs` symlink and private desktop/icon data |
| `ryoku-desktop` | `/usr/bin/ryoku-session`, `/usr/bin/ryoku-app`, `/usr/share/wayland-sessions/ryoku-opensuse.desktop`, two `ryoku-desktop-*` user units, `/usr/share/ryoku-opensuse/session/` |
| `ryoku-desktop-runtime` | `/usr/share/ryoku-opensuse/upstream/`: unchanged shell/Hub/QML, translation catalog and native Blobs outputs |
| `ryoku-desktop-fonts` | `/usr/share/fonts/truetype/ryoku-opensuse/`, retaining font licenses beside unchanged font bytes |

All package files are root:root. Shared parent directories have matching
ownership. The session package explicitly requires the separate `ryoku-helper`
package for its user service; it does not duplicate that package's binary or
unit. `ryoku-profile-defaults` remains an independent data package. Mutable
configuration, profile overrides, recovery identity, subvolumes, journals and
home directories are absent from these payloads. No package enables a unit,
sets the default login, modifies KDE/SDDM or installs a hook. Existing
distribution Fontconfig triggers discover the dedicated font directory.

The private RPM prefix intentionally differs from the initial manual
`/usr/local/ryoku-quickshell` experiment: `/usr/local` is reserved for local
administration. The RPM leaves distribution `/usr/bin/qs` untouched. Upstream
Quickshell's own unmodified CMake install creates its private `qs` symlink and
private desktop/icon files. Its Quickshell modules are statically linked by
the upstream default build; no global Qt module installation is requested.
Blobs retains upstream's `$ORIGIN` lookup for its adjacent backing library.
Its installed native module consists explicitly of `libryoku-blobs.so`,
`libryoku-blobsplugin.so`, `qmldir` and `ryoku-blobs.qmltypes`. The first frozen
`f596630` RPM also copied CMake's generated `ryoku-blobs_qml_module_dir_map.qrc`,
whose XML contains the absolute build-directory path. That file is a build
input, with no role in loading the installed compiled module. The later
explicit four-file spec preserves the runtime bytes and omits this unwanted
metadata. That source change has not been rebuilt or installed natively;
the original RPM and its recorded digest remain unchanged evidence.

Retained Ryoku scripts remain upstream code with their notices and modes;
the packages never execute their installers. The unsupported upstream
settings/plugin/power/authentication/provider actions retain the limitations
in `docs/session-port.md`; packaging does not route them through recovery.

## Native build and review

The specs target the already reviewed Tumbleweed `20261013` repository snapshot
and Qt `6.11.2`. Development dependencies use the actual package split names
recorded in `docs/real-ryoku-runtime.md`; Qt development/import requirements
are exact so a changed private SDK fails dependency resolution instead of
silently reusing prior evidence. Native ELF requirements are left to RPM.
The session also requires native `hyprland-guiutils` (snapshot `0.2.1-1.6`),
which owns `/usr/bin/hyprland-dialog`, Hyprland's actual GUI-utility probe.
This retains the compositor's normal utility behavior without suppressing
its startup diagnostics.
The main desktop package requires the independently owned KDE/SDDM fallback
even with `--no-recommends`: `plasma6-session`, `sddm`, `xorg-x11-server`,
`xf86-input-libinput` and `breeze6-wallpapers`. The snapshot versions are
`6.7.5-1.1`, `0.21.0+git57-1.1`, `21.1.24-2.1`, `1.5.0-1.7` and
`6.7.5-1.1`, respectively. Plasma's session package hard-requires its desktop,
workspace, native Wayland compositor dependencies, PipeWire and KDE polkit
agent, but neither it nor the workspace requires the Breeze wallpaper or
SDDM. The X server and input driver match the existing native SDDM fixture.
The missing `Next` wallpaper package left the actual fallback desktop black
with panel creation deferred; installing the distribution package restored
the visible wallpaper, icons and panel in the coordinator's retained guest.
These dependencies supply the upstream fallback files. Ryoku does not own
them, select the display manager, enable SDDM or write a fallback profile.
Quickshell keeps upstream defaults, uses packaged cpptrace with its native
signal-safe unwind check, explicitly disables vendoring and disconnects CMake
FetchContent. Cargo uses only the locked local vendor directory.

As an ordinary build user, after placing the sources above in a dedicated
absolute RPM top directory, invoke native RPM tools:

```console
rpmspec --parse packaging/rpm/ryoku-quickshell.spec
rpmspec --parse packaging/rpm/ryoku-desktop.spec
rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' packaging/rpm/ryoku-quickshell.spec
rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' packaging/rpm/ryoku-desktop.spec
```

Use the chosen guest's already installed native BuildRequires; do not use
`--nodeps` for a claimed build. Build `ryoku-helper` and the independent defaults
from the same port source as appropriate. Retain full build logs, RPM/SRPM
digests, `rpm -qp --requires`, `rpm -qplv`, `rpm -qp --scripts`, and
`rpm -qp --triggers` for each result. A build alone establishes neither package
dependency resolution nor an installed result.

Before any install, the coordinator must separately review the existing
manual prototype files and their ownership, the complete native transaction
preview and file conflicts. Do not make RPM force-replace unowned or
administrator-owned prototype paths. Keep the KDE fallback and user state.
Then collect the authorized disposable guest's clean native resolution,
installation/removal, font discovery and actual session results. Issue #23
remains open until its required evidence and prerequisite gates are accepted.

## Complete native package build sequence

Keep each experiment's source revision and archive digests explicit. The
initial native build input freeze is `f596630cf3bc626bc46466a20b81f73a9e6f898a`;
the later fallback dependency metadata is a separate source revision and must
not be silently substituted into that frozen build. A future build using the
new spec needs its own Source0 digest. The locked vendor contents are unchanged.

Use an ordinary builder and an absolute top directory on the guest's disk.
Create its `SOURCES`, `SPECS`, `BUILD`, `BUILDROOT`, `RPMS` and `SRPMS`
directories. Place the four checksum-verified source archives in `SOURCES`.
Extract the five specs from that exact Source0 into `SPECS`; do not substitute
specifications from a later checkout. With the complete native SDK installed,
check these macro expansions once:

```console
rpm --eval '%{_bindir} %{_sbindir} %{_libexecdir} %{_datadir} %{_unitdir} %{_userunitdir} %{_sysusersdir}'
```

The reviewed native values are `/usr/bin`, `/usr/sbin`, `/usr/libexec`,
`/usr/share`, `/usr/lib/systemd/system`, `/usr/lib/systemd/user` and
`/usr/lib/sysusers.d`. The last three depend on installed native
`systemd-rpm-macros`. `Release: 1%{?dist}` is valid when `dist` is undefined.
Run the following builds serially and retain each complete log:

```console
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' --define '_smp_build_ncpus 2' /absolute/issue23-rpmbuild/SPECS/ryoku-profile-defaults.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' --define '_smp_build_ncpus 2' /absolute/issue23-rpmbuild/SPECS/ryoku-helper.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' --define '_smp_build_ncpus 2' /absolute/issue23-rpmbuild/SPECS/ryoku-quickshell.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' --define '_smp_build_ncpus 2' /absolute/issue23-rpmbuild/SPECS/ryoku-desktop.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' --define '_smp_build_ncpus 2' /absolute/issue23-rpmbuild/SPECS/ryoku-tools.spec
```

No Ryoku output RPM needs installation between builds: their inter-package
dependencies are runtime `Requires`, while their `BuildRequires` are native
SDK packages. The Cargo and CMake parallelism limits apply to the two-vCPU
guest. Do not override dependency checks or replace its RPM toolchain with
rustup. Inspect the resulting runtime package dependency closure and install
transaction separately after all builds succeed.
