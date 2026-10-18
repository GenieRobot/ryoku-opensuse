Name:           ryoku-tools
Version:        0.1.0
Release:        3%{?dist}
Summary:        Read-only native Ryoku recovery inspection tools
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config

%description
Installs the Rust recovery inspector for existing verified Ryoku state. Its
list, pending, inspect, verify and verify-linked commands are read-only; it
does not restore a system or provision state. No services, protected-state
directories, machine records, mutable defaults or user files are installed.
Package availability is not evidence of accepted recovery or boot readiness.

%package state-init
Summary:        Explicit native Ryoku state provisioning command
Requires:       snapper
Requires:       systemd
Requires:       util-linux
Requires:       coreutils
Requires:       policycoreutils

%description state-init
Installs the existing Rust ryoku-state-init administrator command. When
explicitly invoked as root it verifies the supported Btrfs/Snapper layout,
creates and records dedicated state, and supports its typed resume protocol.
Installation does not invoke it, create a subvolume, write a machine identity,
enable a mount, or authorize a caller. No set-ID mode or file capability is
assigned. The command's runtime checks remain mandatory.

%package launcher-status
Summary:        Rust read-only service provider for the preserved launcher
Requires:       ryoku-helper = %{version}-%{release}

%description launcher-status
Installs the native ryoku-launcher-status provider in the private Ryoku libexec
directory. It sends only typed Discover requests to the fixed ordinary-user
service socket and emits genuine result/error rows for the unchanged launcher.
It uses no shell, rofi process, privileged mutation or helper command. Selecting
the provider is a separate explicit user configuration step; the opt-in example
is documentation and is never copied into a user's launcher configuration.

%prep
%autosetup -n ryoku-opensuse-%{version} -a 1

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-recovery \
    --bin ryoku-recovery --bin ryoku-state-init
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-ui --no-default-features \
    --bin ryoku-launcher-status

%install
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-recovery" \
    %{buildroot}%{_bindir}/ryoku-recovery
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-state-init" \
    %{buildroot}%{_sbindir}/ryoku-state-init
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-launcher-status" \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-launcher-status

%files
%defattr(-,root,root)
%attr(0755,root,root) %{_bindir}/ryoku-recovery
%license LICENSE
%doc docs/recovery-state.md packaging/rpm/tools-build.md

%files state-init
%defattr(-,root,root)
%attr(0755,root,root) %{_sbindir}/ryoku-state-init
%license LICENSE
%doc docs/recovery-state.md packaging/rpm/tools-build.md

%files launcher-status
%defattr(-,root,root)
%dir %{_libexecdir}/ryoku
%attr(0755,root,root) %{_libexecdir}/ryoku/ryoku-launcher-status
%license LICENSE
%doc docs/ui-integration.md packaging/examples/launcher-scripts.json packaging/rpm/tools-build.md
