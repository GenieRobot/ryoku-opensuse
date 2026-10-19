# Experimental Rust tools RPMs

`ryoku-tools.spec` completes the build definitions for existing project Rust
UI/recovery executables alongside the desktop, helper and profile defaults
specifications. It shares their exact committed `Source0` and locked vendor
`Source1` inputs and offline Cargo policy. Source0 must include the native
launcher provider introduced by `fc50d0b599e787dc7266328e1a8068ba7d3a4a65`
(or its integrated equivalent). It must also include these spec/docs files.

| Package | Executable | Role |
| --- | --- | --- |
| `ryoku-tools` | `/usr/bin/ryoku-recovery` | Read-only inspection of existing protected recovery state |
| `ryoku-tools-state-init` | `/usr/sbin/ryoku-state-init` | Explicit root-only initializer/resume command |
| `ryoku-tools-ui` | `/usr/bin/ryoku-ui-gtk` | Optional GTK status proof, with explicit real-service mode |
| `ryoku-tools-launcher-status` | `/usr/libexec/ryoku/ryoku-launcher-status` | Read-only typed Discover provider for unchanged Ryoku launcher |

All executable files are mode 0755, root:root, without set-ID bits or assigned
capabilities. The packages own no mutable system or user state and have no
scriptlets, triggers, units, autostarts or privileged activation. The initializer
is invoked only through its existing explicit administrator workflow; its
package does not initialize `/var/lib/ryoku-state`, `/etc/ryoku`, a mount unit,
Snapper or a profile. The UI/provider depend on the independent helper/service
package and do not duplicate its units or binaries. The opt-in launcher JSON
example is installed only as documentation, with no default profile change.
The native launcher provider is built without the GTK feature before building
the separate GTK target, preserving its lightweight ELF dependency closure.

## Native prerequisites verified from the pinned metadata

The existing signed Tumbleweed snapshot is `20261013`. The locally retained
`repomd.xml` SHA-256 is
`c64c293f246a439f0b5e1881fe8adfc980ca44420a84935f31e223949810daa4`;
its primary metadata SHA-512 is
`55cd766fd1a01ac7cd7070158e06c9e9caf391e6b6fd594e5fd500978c41e2379aefcde492f93945243fcdaf14a9abdeb1bd3871711d8bb4bc321174233dc11d`.
These match the checked-in fixture and runtime manifest. The native package
records provide:

| Package | Snapshot version | Relevant facts |
| --- | --- | --- |
| `rust` | `1.99.0-1.1` | Native RPM provider `rust`; requires `rust1.99` |
| `cargo` | `1.99.0-1.1` | Native RPM provider `cargo`; requires `cargo1.99` and `rust = 1.99.0` |
| `rust1.99` | `1.99.0-1.1` | Compiler and `rust-std`; hard dependencies include clang, gcc and glibc-devel |
| `cargo1.99` | `1.99.0-1.1` | Actual `/usr/bin/cargo`; requires matching `rust-std` |
| `gtk4-devel` | `4.22.5-1.1` | Native GTK proof development SDK |
| `pkgconf-pkg-config` | `2.5.1-1.5` | Native pkg-config command for Cargo's GTK system probes |
| `rpm-build` | `4.20.1-11.1` | Already installed by the coordinator |
| `systemd-rpm-macros` | `26-1.3` | Already installed by the coordinator |

An executable from rustup alone cannot satisfy the declared RPM BuildRequires.
The coordinator can review one native transaction containing `rust cargo
gtk4-devel pkgconf-pkg-config gcc` using the existing pinned repositories and
the already installed Qt/Quickshell SDK. Let Zypper resolve all native hard
dependencies and show the complete preview before the authorized guest
installation. The compiler package alone expands to 1,268,213,440 installed
bytes, so the coordinator must include its dependency closure in the disk
preview. This document records metadata availability, not a guest install or
solver result. It authorizes no host package changes.

Runtime provisioning tools use existing native providers: `snapper`,
`systemd`, `util-linux` (mount/umount), `coreutils` (timeout) and
`policycoreutils` (restorecon). Kernel Btrfs operations remain in the existing
Rust implementation. RPM generates native ELF requirements for each package;
the GTK proof must not make the launcher provider depend on GTK.

## Native build and retained evidence

After preparing the same Source0/vendor archives as documented in
[README.md](README.md#reproducible-source-inputs), run as an ordinary builder:

```console
rpmspec --parse packaging/rpm/ryoku-tools.spec
rpmbuild -ba --define '_topdir /absolute/issue23-rpmbuild' packaging/rpm/ryoku-tools.spec
```

Retain the source revision and lockfile/archive hashes, full native build log,
RPM and SRPM hashes, per-package file/ELF dependency inventories and the empty
scriptlet/trigger queries. Do not claim a native build using `--nodeps`.
Installation/removal, dependency resolution, path conflicts, graphical behavior
and recovery acceptance remain separate guest evidence gates under issue #23.
