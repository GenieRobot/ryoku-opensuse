# RPM metadata

RPM spec files and related package metadata belong here. Keep them declarative:
use native RPM macros and package metadata, not embedded shell, Python, Ruby, or
other custom implementations. Build behavior must come from the Rust workspace
or retained upstream components. Package names, dependencies, scriptlets, and
service policy are introduced with the task that implements each capability.

The experimental native desktop, unchanged runtime/font payload and private
official Quickshell build are specified in `ryoku-desktop.spec` and
`ryoku-quickshell.spec`. Their source closure, ownership and native build
commands are in [desktop-build.md](desktop-build.md). These definitions do not
claim installed-session acceptance or close issue #23.

The distinct Release 2 installation candidate and four-spec native build
sequence are documented in [install-phase2.md](install-phase2.md). It reuses
the unchanged, independently verified Quickshell Release 1 artifact and keeps
the first frozen source/RPM evidence separate.

`ryoku-tools.spec` separately packages the read-only recovery inspector,
explicit administrator initializer, optional GTK proof and read-only launcher
provider. [tools-build.md](tools-build.md) records their ownership, native
prerequisites and build commands. Installing these packages performs no state
provisioning or service activation.

The deny-only service boundary installs the helper's
`systemd/ryoku-helper.socket` and `systemd/ryoku-helper.service`, plus the
user-scoped `systemd/user/ryoku-service.service` and `sysusers.d/ryoku.conf`.
The socket is root-owned and group-readable/writable by the opt-in
`ryoku-operators` group. The package installs a sysusers declaration that
provisions the group but adds no users. Administrators must select users
explicitly. The socket is installed but not enabled by the package. The
per-user daemon connects to this fixed socket with a typed
`CheckAvailability` request. The helper checks the kernel peer UID, primary
GID, and supplementary group IDs against the local `ryoku-operators` GID, and
independently denies root. It returns `unsupported` for every valid request
and does not read user files, persist journal records, or mutate the system.
The `ryoku-helper.spec` builds and installs the service and deny-only helper,
units, and sysusers file. Both executables and package metadata are explicitly
root-owned; the socket's runtime group is `ryoku-operators`, while user
configuration and journals remain owned by each user. The recovery crate also
provides `ryoku-state-init` and `ryoku-recovery` executables; this helper RPM
does not build or install them. This definition does
not claim an installed Tumbleweed result or bit-for-bit reproducible binaries.
The first ordinary-user native helper/defaults builds passed with the guest's
installed RPM-provided Rust/Cargo SDK; their artifact inspection is recorded in
[the issue #23 evidence](../../infra/tumbleweed-lab/evidence/issue23-first-native-rpms.toml).
Rustup executables alone do not satisfy the declared native RPM BuildRequires.
Clean installation and group creation remain unvalidated.
No unit is enabled by the package. To opt in, an administrator adds selected
users to `ryoku-operators`, waits for them to start a new login session, and
enables `ryoku-helper.socket`; each user separately enables
`ryoku-service.service` in their user manager.

### Sysusers install order

The spec installs `ryoku.conf` in `%{_sysusersdir}` and has no Ryoku sysusers
scriptlet or trigger. The actual native helper RPM has an empty `SYSUSERS`
header. In the reviewed guest, the distribution's `systemd` package owns the
`transfiletriggerin` for `/usr/lib/sysusers.d`: after package transactions it
invokes `systemd-sysusers` when `/run/systemd/system` exists. This is the
native integration used here; a minimum RPM version alone does not establish
before-unpack group creation. The recorded guest trigger output confirms the
mechanism without installing the Ryoku package.

The native trigger skips offline updates. Applying sysusers declarations at
boot is a separate systemd boot-service path, subject to that service's native
conditions; offline installation and boot provisioning have not been proven
by this artifact inspection. No `%pre` is necessary for payload ownership:
every packaged file is root-owned. The sole declaration is
`g ryoku-operators - -`; it declares one group and no users or memberships.
Administrators must verify the group exists before opting users in and
activating the socket. The earlier package-inspection evidence assumed that
the retained fixture already had the group; that assumption was not verified
by a native NSS query. The later native pre-install lookup successfully found
`root` but returned exit 2 for `ryoku-operators`, proving the target group is
currently absent. This permits a first-creation installation check without
removing any group. The original artifact identities and phase-one evidence
remain unchanged; [the corrective preflight record](../../infra/tumbleweed-lab/evidence/issue23-phase2-source-preflight.toml)
supersedes the earlier assumption. Package-triggered creation itself has not
yet been observed.

The helper/defaults RPMs contain no activation scriptlets or triggers of their
own. Distribution-owned systemd transaction triggers still apply to packaged
unit/sysusers paths. Their observed code, package identities, hashes and
remaining installation gates are recorded in the issue #23 evidence above.

### Reproducible source inputs

The spec takes a source archive for the exact Git commit and a separate Cargo
vendor archive derived from that commit's `Cargo.lock`. The checked-in
spec redirects crates.io to `vendor/` with Cargo command-line configuration,
and the build uses `--locked --offline`; it cannot resolve newer crates or
access the network. Build the two source files from a clean, committed checkout
using the same version as `Version:` in the spec:

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

Record the Git commit, `Cargo.lock` digest, both source archive digests, RPM
toolchain versions, and resulting RPM digest with each build. The archive
commands normalize source ordering, timestamps, ownership, and gzip headers;
the locked vendored dependency set lets the RPM build run offline. Explicit
GNU format avoids access/change-time PAX headers from builders whose default
tar format is POSIX; those headers otherwise change after reading the inputs.
These steps
make the inputs repeatable and auditable, but do not claim byte-identical RPM
outputs across builders or replace the required clean-VM install evidence.

### Generic profile defaults

`ryoku-profile-defaults.spec` is a separate noarch data package using the same
`Source0` Git archive convention. It needs no Cargo vendor archive or executable
build. Its sole runtime payload is the newly authored port default
`/usr/share/ryoku/config/profile.json`, with root:root ownership, mode `0644`,
and a required SHA-256 RPM file digest. The generic profile selects Hyprland,
Ryoku dark theme and comfortable density; companion settings are optional.
The package has no scriptlets, activation hooks or home-directory files.

`recovery::rpm_defaults` checks installed ownership using the fixed native
`/usr/bin/rpm --query --file` selection and a fixed queryformat. It requires one
owning `ryoku-profile-defaults`/noarch header and one exact matching file-array
row. It compares the recorded digest, type/mode, root owner/group and size with
a bounded no-follow read, then requeries package facts and rereads the file.
The loader accepts no package/path/executable/database override. Both output
pipes are drained nonblocking with separate limits and a five-second deadline;
failure kills the child process group and returns only sanitized categories.
There are no package scripts, command strings or network operations in this
query path. RPM installation trust and the shared package-writer lock remain
separate prerequisites.

The parser follows RPM 4.20's documented [paired query arrays](https://rpm.org/docs/4.20.x/manual/queryformat.html)
and [file/header tags](https://rpm.org/docs/4.20.x/manual/tags.html), including
`EPOCHNUM`, `FILEDIGESTALGO`, `FILENAMES`, `FILEDIGESTS`, `FILEMODES`, owner/group
names and `LONGFILESIZES`. It selects the exact default path rather than using
the package's first file. Missing/ambiguous ownership, unsupported digest
algorithms, changed package facts or modified default bytes block import.

Rust fixtures exercise these decisions, output overflow on both pipes,
nonzero child exit and timeout without invoking the host RPM database. Native
RPM build/query/install validation remains pending; this metadata and these
fixtures are not activation or VM evidence.
