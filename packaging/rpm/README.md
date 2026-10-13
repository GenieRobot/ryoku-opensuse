# RPM metadata

RPM spec files and related package metadata belong here. Keep them declarative:
use native RPM macros and package metadata, not embedded shell, Python, Ruby, or
other custom implementations. Build behavior must come from the Rust workspace
or retained upstream components. Package names, dependencies, scriptlets, and
service policy are introduced with the task that implements each capability.

The deny-only helper boundary installs `systemd/ryoku-helper.socket`,
`systemd/ryoku-helper.service`, and `sysusers.d/ryoku.conf`. The socket is
root-owned and group-readable/writable by the opt-in `ryoku-operators` group;
the package installs a sysusers declaration that provisions the group but
adds no users. Administrators must select users explicitly. The socket is
installed but not enabled by the package. The per-user daemon connects to this
fixed socket with a typed `CheckAvailability` request. The helper identifies
the caller by kernel peer UID, returns `unsupported` for every valid request,
and does not read user files, persist journal records, or mutate the system.
The `ryoku-helper.spec` builds and installs the binary, units, and sysusers
file; RPM build validation still requires an openSUSE RPM build environment.

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
