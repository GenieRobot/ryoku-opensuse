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
installed but not enabled by the package. The per-user daemon connects to this fixed socket
with a typed `CheckAvailability` request. The helper identifies the caller by
kernel peer UID, returns `unsupported` for every valid request, and does not
read user files, persist journal records, or mutate the system. The
`ryoku-helper.spec` builds and installs the binary, units, and sysusers file;
RPM build validation still requires an openSUSE RPM build environment.
