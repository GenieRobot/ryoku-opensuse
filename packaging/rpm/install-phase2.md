# Phase-two native installation candidate

This is a distinct source cut based on integrated commit
`1e138c968adb92a5020134b7b0d67873d9775a77`. The four port specs use
`0.1.0-2`; the independently pinned and unchanged Quickshell spec remains
`0.3.2-1`. The phase-one source archive, native RPM/SRPM outputs and evidence
remain unchanged. This document defines preparation and build inputs; it is
not a native build or installation result.

## Exact source and reused artifact identity

The source commit for this cut is the clean commit containing these Release
bumps and this document. Its full Git SHA, commit epoch, `Cargo.lock` digest,
Source0 archive digest and tool versions are written into the external
`target/issue23-install-prep/checksum-manifest.txt` after committing. This
avoids a self-referencing source-commit field inside the Git archive.
Source0 uses native `git archive --format=tar` with the prefix
`ryoku-opensuse-0.1.0/`, followed by `gzip -n`.

The following unchanged inputs are copied byte-for-byte into the new
disk-backed `target/issue23-install-prep/SOURCES` directory. No phase-one
filename is replaced in its original directory.

| Input | SHA-256 |
| --- | --- |
| `ryoku-opensuse-vendor-0.1.0.tar.xz` | `37e07449436648ebfe54d9da92bb785c52b636691966bda3690f652af31dd7ec` |
| `issue14-runtime-payload.tar.gz` | `81151cef1a613b01ad8e6187df2e93cbfed70e1621de0ee2e7455d2904fc3e74` |
| `issue14-quickshell-0.3.2.tar.gz` | `5e8a7b7a1a1e40de641ee6cfd24f40aa612e13c27353e5746fc79b2861ca9fd0` |

The unchanged lockfile SHA-256 is
`62d167a989cd85865ea38dc31e6fcdf982600d55150f0c1d4be8cdaf85545061`.
The vendor archive contains the same 81 locked crates, with all 4,751 vendor
files previously verified against Cargo inventories. It retains its original
normalized epoch `1792109014` and explicit GNU tar format; it is deliberately
reused without recompression or retimestamping. The new Source0 uses its new
commit epoch, so these two provenance timestamps differ explicitly.

Reuse the actual independently reviewed runtime RPM
`ryoku-quickshell-0.3.2-1.x86_64.rpm`, SHA-256
`626c3f6928d886a79c3a774e5b9f70aaded67888cbf9e3399a795648c0e18537`.
The unchanged Quickshell spec SHA-256 is
`afae5a37f97e82eaaddf9e2838aaf818a7ee1039869a6a85ccc6851dc3aa8362`.
It uses only its pinned external source archive and identical native build
definition, so it is not rebuilt or relabeled for this port-only source cut.
Its generated strong requirements for Gui/Qml/Quick/WaylandClient's exact
`Qt_6.11.2_PRIVATE_API` remain part of transaction validation. Keep its original
SRPM and debug artifacts with the phase-one evidence.

## Changes requiring distinct candidate outputs

The new desktop spec includes the integrated hard dependencies for the
independent KDE/SDDM fallback, including `breeze6-wallpapers`; those were absent
from the first frozen RPM. It also installs exactly four native Blobs files:
`libryoku-blobs.so`, `libryoku-blobsplugin.so`, `qmldir` and
`ryoku-blobs.qmltypes`. The unwanted CMake resource-map XML with an absolute
builder path is excluded. Neither change modifies upstream executable source
or font bytes. Sysusers documentation now describes the actual distribution
post-transaction trigger; no Ryoku scriptlet is introduced.

The Release bump distinguishes every port-owned native output from the
phase-one packages, including the unchanged default data and the separately
packaged service/UI/recovery tools. Runtime/font dependencies use
`0.1.0-2` where the spec already pins `%{version}-%{release}`. All nine new
port runtime RPMs plus the unchanged Quickshell RPM form the ten-package
candidate set. Source-only preparation does not validate these outputs.

## Separate native build directory

The coordinator stages the four verified archives and their `SHA256SUMS` in
`/home/tester/ryoku-rpmbuild-install/SOURCES`, separate from the original
`/home/tester/ryoku-rpmbuild`. As the ordinary native builder, create its
`SPECS`, `BUILD`, `BUILDROOT`, `RPMS` and `SRPMS` directories. Check all four
archive hashes from the new `SOURCES` directory, then extract only these
four specs from that exact Source0:

```console
tar -xzf /home/tester/ryoku-rpmbuild-install/SOURCES/ryoku-opensuse-0.1.0.tar.gz --strip-components=3 -C /home/tester/ryoku-rpmbuild-install/SPECS ryoku-opensuse-0.1.0/packaging/rpm/ryoku-profile-defaults.spec ryoku-opensuse-0.1.0/packaging/rpm/ryoku-helper.spec ryoku-opensuse-0.1.0/packaging/rpm/ryoku-desktop.spec ryoku-opensuse-0.1.0/packaging/rpm/ryoku-tools.spec
```

The native BuildRequires and macro providers are the same reviewed SDK used
for phase one. Preserve dependency checks and the two-job limits. Build only
these four specs, serially, retaining a separate complete log for each:

```console
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /home/tester/ryoku-rpmbuild-install' --define '_smp_build_ncpus 2' /home/tester/ryoku-rpmbuild-install/SPECS/ryoku-profile-defaults.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /home/tester/ryoku-rpmbuild-install' --define '_smp_build_ncpus 2' /home/tester/ryoku-rpmbuild-install/SPECS/ryoku-helper.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /home/tester/ryoku-rpmbuild-install' --define '_smp_build_ncpus 2' /home/tester/ryoku-rpmbuild-install/SPECS/ryoku-desktop.spec
CARGO_BUILD_JOBS=2 rpmbuild -ba --define '_topdir /home/tester/ryoku-rpmbuild-install' --define '_smp_build_ncpus 2' /home/tester/ryoku-rpmbuild-install/SPECS/ryoku-tools.spec
```

These commands build packages only. Native review must confirm all nine
port runtime outputs have Release 2, the reused Quickshell retains Release 1,
the fallback dependencies are present, Blobs has only its four runtime files,
and upstream/file-mode/scriptlet contracts still hold. Keep debug artifacts
separate from the explicit ten-package runtime selection.

The coordinator owns the subsequent dependency and installed-file review,
prototype backup, authorized installation and recovery plan. The first dry
run excluded all ten packages from conflict checking. A subsequent native
pre-install `getent` check found `root` but not `ryoku-operators` (target lookup
exit 2), correcting the earlier unverified assumption that the fixture already
had the group. This allows a first group-creation check without deleting or
altering any group. It does not prove package-triggered creation before an
installation is observed. No package install, signature trust, group creation, removability,
or package-owned GUI/recovery result is claimed by this source cut. Issue #23
remains open pending the required native evidence and prerequisite acceptance.
