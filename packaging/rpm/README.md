# RPM package definitions

Issue #23 defines six specs for ten runtime packages. The
[desktop build](desktop-build.md) documents pinned upstream sources and private
Quickshell/Blobs builds. The [Rust package inventory](tools-build.md) documents
service, UI and recovery ownership. Specs use native declarative packaging,
Cargo and retained upstream Go/CMake; they add no executable project scriptlets
or transaction hooks.

The desktop runtime implements the integrated issue #18 writer payload map,
public wrapper aliases and fixed QML launch paths. The optional `ryoku-session.spec` adds the experimental issue #25 login,
Rust adapters and immutable Hyprland defaults. It depends on the accepted
release-5 desktop package set and retains KDE dependencies explicitly. See
[the session contract](../../docs/session-port.md); this additional spec has
not passed the native RPM/session acceptance gate.
The separate GTK proof remains an explicitly labeled demonstration application.
Installing any of these RPMs does not authorize a supported system mutation or
establish recovery readiness.

## Service and sysusers ownership

`ryoku-helper` owns the service/helper binaries, system socket/service, user
service and group-only sysusers declaration accepted under issue #17. Every
payload file is root-owned. The runtime socket grants access to selected
`ryoku-operators` members; the declaration enrolls no users. Neither service
nor socket is enabled by installation. Administrators select users explicitly
and wait for a new login session before activating the boundary.

Tumbleweed's distribution-owned systemd file trigger processes
`/usr/lib/sysusers.d` after a package transaction on a running system. The
Ryoku package adds no custom sysusers scriptlet. The RPM version alone does
not prove before-unpack group creation; no payload ownership depends on that
group. Offline provisioning at boot is a separate systemd path. Native
installation evidence must check actual group creation before socket use.

## Reproducible source inputs

Use the exact committed port revision for Source0 and its exact `Cargo.lock`
for Source1. Each Rust spec redirects crates.io to the local vendor directory
and builds with `--locked --offline`. The desktop specs additionally validate
their pinned external archives. Sources use normalized ordering, timestamps,
ownership and gzip headers; this identifies repeatable inputs without claiming
byte-identical outputs across builders.

```sh
VERSION=0.1.0
SOURCE_DATE_EPOCH=$(git show -s --format=%ct HEAD)
mkdir -p SOURCES
git archive --format=tar --prefix="ryoku-opensuse-${VERSION}/" HEAD \
  | gzip -n > "SOURCES/ryoku-opensuse-${VERSION}.tar.gz"
VENDOR_TMP=$(mktemp -d)
cargo vendor --locked --manifest-path Cargo.toml "${VENDOR_TMP}/vendor"
tar --format=gnu --sort=name --mtime="@${SOURCE_DATE_EPOCH}" --owner=0 --group=0 \
  --numeric-owner -C "${VENDOR_TMP}" -cJf \
  "SOURCES/ryoku-opensuse-vendor-${VERSION}.tar.xz" vendor
sha256sum "SOURCES/ryoku-opensuse-${VERSION}.tar.gz" \
  "SOURCES/ryoku-opensuse-vendor-${VERSION}.tar.xz"
rm -rf "${VENDOR_TMP}"
```

Retain source revision, lockfile/archive hashes, native toolchain versions,
complete build logs, RPM/SRPM hashes, file/dependency inventories, scripts and
triggers with each build. BuildRequires remain enabled. The exact current
Tumbleweed image and repositories must be recorded with native dependency
resolution and installation evidence; historic 20261013 results are provenance
for those older artifacts only. Rolling-release updates require compatible
dependency resolution, especially for Quickshell's private Qt ABI.

The [2026-10-21 native acceptance receipt](../../infra/tumbleweed-lab/evidence/issue23-native-acceptance-20261021.toml)
records the exact six-spec source closure, ten-package manifests and signed
fresh Snapshot20261016 dependency, install and reinstall checks. The linked
logs include native Qt6 scanning, first sysusers group creation and strict
override preservation. Its fixture signing key is not a release key. That receipt covers its recorded
port 0.1.0-5 / Quickshell 0.3.2-2 input tuple; it does not certify the current
Ryoku 1.0 candidate. #24 Run10 also retains its explicit 0.75.5 / port
0.1.0-7 label. Follow the [fresh #173/#25/#26 acceptance contract](../../docs/ryoku-v1.0-acceptance-gates.md)
before closing the refreshed desktop or first-sprint gates. Source/hosted CI
and reused SDK artifacts do not substitute for the required fresh native
package, session, fallback and linked offline recovery evidence.

## Defaults and mutable overrides

`ryoku-profile-defaults` owns only the generic port profile under
`/usr/share/ryoku/config/profile.json`, root:root mode 0644 with SHA-256 RPM
file digests. It selects Hyprland, Ryoku dark theme and comfortable density.
It contains no executable code, activation hooks or companion data.
Personal/system override layers and recovery state are absent from every
package payload. Reinstallation replaces package defaults while preserving
user-owned state. Native conflict and reinstall checks must prove this on the
chosen guest; metadata alone does not prove installation acceptance.

`rpm-install-fixture` is an acceptance utility, absent from every RPM payload.
It has no install, activation or command-dispatch operation. Its fixed fixture
paths can be created only as root in the marked x86_64 Snapshot20261016 QEMU
guest, with the issue-specific owned-overlay marker and
`RYOKU_RPM_TEST_AUTHORIZED=issue23-owned-overlay`. It refuses existing fixture
files and requires the operator group to be absent before preparation. The
subsequent receipt checks preserve exact file bytes, link targets, ownership,
modes, mtime, inode and hard-link counts, require an empty newly created
operator group and reject package activation links. Its protected-state
namespace sentinel is explicitly invalid recovery state; it creates no valid
profile object, machine identity or restore point. Normal Zypper transactions
are invoked separately in the authorized disposable VM.
