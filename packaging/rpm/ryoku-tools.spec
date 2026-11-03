Name:           ryoku-tools
Version:        0.1.0
Release:        8%{?dist}
Summary:        Native offline Ryoku recovery console
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config
BuildRequires:  systemd-rpm-macros

%description
Installs the Rust offline recovery console. Inspection and scope previews are
read-only. Restore commands require root, verified rollback-independent state,
an offline rescue/emergency console and the installed graphical admission gate
from ryoku-tools-state-init. Required checks fail before destructive restore.
No protected state or user files are provisioned by package installation.

%package state-init
Summary:        Explicit native Ryoku state provisioning command
Requires:       snapper
Requires:       systemd
Requires:       util-linux
Requires:       coreutils
Requires:       policycoreutils
Requires:       zypper
Requires:       rpm >= 4.19
Requires:       ryoku-tools = %{version}-%{release}

%description state-init
Installs the existing Rust ryoku-state-init administrator command. When
explicitly invoked as root it verifies the supported Btrfs/Snapper layout,
creates and records dedicated state, and supports its typed resume protocol.
Also installs the explicit root console installer and the fail-closed graphical
login admission service and display
manager dependency. Graphical login requires independently provisioned state
and a complete recovery journal. Installation does not invoke initialization,
create a subvolume, write a machine identity or enable a mount. No set-ID mode
or file capability is assigned. The command's runtime checks remain mandatory.

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
    build --offline --locked --release --no-default-features \
    -p ryoku-recovery -p ryoku-ui -p ryoku-service \
    --bin ryoku-recovery --bin ryoku-state-init --bin ryoku-install --bin ryoku-launcher-status

%install
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-recovery" \
    %{buildroot}%{_bindir}/ryoku-recovery
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-state-init" \
    %{buildroot}%{_sbindir}/ryoku-state-init
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-install" \
    %{buildroot}%{_sbindir}/ryoku-install
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-launcher-status" \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-launcher-status
install -D -m 0644 packaging/rpm/systemd/ryoku-recovery-admission.service \
    %{buildroot}%{_unitdir}/ryoku-recovery-admission.service
install -D -m 0644 packaging/rpm/systemd/display-manager.service.d/ryoku-recovery.conf \
    %{buildroot}%{_unitdir}/display-manager.service.d/ryoku-recovery.conf

%files
%defattr(-,root,root)
%attr(0755,root,root) %{_bindir}/ryoku-recovery
%license LICENSE
%doc docs/recovery-state.md docs/installer-preflight.md packaging/rpm/tools-build.md

%files state-init
%defattr(-,root,root)
%attr(0755,root,root) %{_sbindir}/ryoku-state-init
%attr(0755,root,root) %{_sbindir}/ryoku-install
%attr(0644,root,root) %{_unitdir}/ryoku-recovery-admission.service
%dir %{_unitdir}/display-manager.service.d
%attr(0644,root,root) %{_unitdir}/display-manager.service.d/ryoku-recovery.conf
%license LICENSE
%doc docs/recovery-state.md docs/installer-preflight.md packaging/rpm/tools-build.md

%files launcher-status
%defattr(-,root,root)
%dir %{_libexecdir}/ryoku
%attr(0755,root,root) %{_libexecdir}/ryoku/ryoku-launcher-status
%license LICENSE
%doc docs/ui-integration.md packaging/examples/launcher-scripts.json packaging/rpm/tools-build.md
