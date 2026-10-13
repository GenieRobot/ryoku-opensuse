# RPM metadata

RPM spec files and related package metadata belong here. Keep them declarative:
use native RPM macros and package metadata, not embedded shell, Python, Ruby, or
other custom implementations. Build behavior must come from the Rust workspace
or retained upstream components. Package names, dependencies, scriptlets, and
service policy are introduced with the task that implements each capability.

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
configuration and journals remain owned by each user. The recovery crate is a
library and has no recovery executable target to package. This definition does
not claim an installed Tumbleweed result or bit-for-bit reproducible binaries;
RPM build/install validation still requires an openSUSE RPM build environment.
No unit is enabled by the package. To opt in, an administrator adds selected
users to `ryoku-operators`, waits for them to start a new login session, and
enables `ryoku-helper.socket`; each user separately enables
`ryoku-service.service` in their user manager.

### Sysusers install order

The spec installs `ryoku.conf` in `%{_sysusersdir}` and does not call a
sysusers macro. Tumbleweed's RPM 4.20.1 has native sysusers support (introduced
in RPM 4.19): RPM processes packaged `u`/`g` declarations with
`systemd-sysusers` before unpacking that package's payload. This creates
`ryoku-operators` during package installation, before an administrator can
enable or start the socket. No `%pre` macro is needed because the package has
no files whose ownership depends on this group. The metadata declares only
the group, so it does not add any users or enable the socket.

Evidence:

- [RPM 4.20 user and group packaging documentation](https://rpm.org/docs/4.20.x/manual/users_and_groups.html) describes automatic processing of `/usr/lib/sysusers.d` files at install and says it occurs before payload unpack; RPM 4.19 introduced this behavior.
- [openSUSE Tumbleweed `sysusers.d(5)`](https://manpages.opensuse.org/Tumbleweed/systemd/sysusers.d.5.en.html) documents that systemd-sysusers creates declared groups at package installation or boot.
- Tumbleweed repository metadata checked 2026-10-13 reported [RPM 4.20.1-11.1](https://download.opensuse.org/tumbleweed/repo/oss/x86_64/rpm-4.20.1-11.1.x86_64.rpm), newer than the native-support baseline.

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
tar --sort=name --mtime="@${SOURCE_DATE_EPOCH}" --owner=0 --group=0 \
  --numeric-owner -C "${VENDOR_TMP}" -cJf \
  "SOURCES/ryoku-opensuse-vendor-${VERSION}.tar.xz" vendor
sha256sum "SOURCES/ryoku-opensuse-${VERSION}.tar.gz" \
  "SOURCES/ryoku-opensuse-vendor-${VERSION}.tar.xz"
rm -rf "${VENDOR_TMP}"
```

Record the Git commit, `Cargo.lock` digest, both source archive digests, RPM
toolchain versions, and resulting RPM digest with each build. The archive
commands normalize source ordering, timestamps, ownership, and gzip headers;
the locked vendored dependency set lets the RPM build run offline. These steps
make the inputs repeatable and auditable, but do not claim byte-identical RPM
outputs across builders or replace the required clean-VM install evidence.
