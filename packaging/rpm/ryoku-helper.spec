Name:           ryoku-helper
Version:        0.1.0
Release:        9%{?dist}
Summary:        Typed protected helper boundary for Ryoku
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  systemd-rpm-macros
Requires:       systemd
Requires:       rpm >= 4.19
Requires:       /usr/bin/bash
Requires:       libgio-2.0.so.0()(64bit)
Requires:       gsettings-desktop-schemas

# Tumbleweed systemd processes this group-only sysusers declaration through
# its distribution-owned transaction trigger. All payload files are root-owned;
# no project scriptlet or pre-unpack group ownership is required.

%description
Installs Ryoku's socket-activated privileged helper boundary. Reviewed wallpaper
and bounded configured-hook operations require authenticated operator access,
verified linked checkpoints, exact typed policies and durable completion.
Unsupported operation families remain unavailable. Access to its socket is
limited to explicitly selected members of ryoku-operators.

This build includes the user service, protected helper and explicit ryoku-hook
inspection/authorization tool.
The recovery executables are packaged separately in ryoku-tools; installing
this package does not initialize state or enable the socket or either service.

%prep
%autosetup -n ryoku-opensuse-%{version} -a 1

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release \
    -p ryoku-service --bin ryoku-helper --bin ryoku-service --bin ryoku-hook --bin ryoku-theme-actor

%install
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-helper" \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-helper
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-service" \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-service
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-hook" \
    %{buildroot}%{_bindir}/ryoku-hook
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-theme-actor" \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-theme-actor
install -D -m 0644 packaging/rpm/systemd/ryoku-helper.service \
    %{buildroot}%{_unitdir}/ryoku-helper.service
install -D -m 0644 packaging/rpm/systemd/ryoku-helper.socket \
    %{buildroot}%{_unitdir}/ryoku-helper.socket
install -D -m 0644 packaging/rpm/systemd/user/ryoku-service.service \
    %{buildroot}%{_userunitdir}/ryoku-service.service
install -D -m 0644 packaging/rpm/sysusers.d/ryoku.conf \
    %{buildroot}%{_sysusersdir}/ryoku.conf

%files
%dir %attr(0755,root,root) %{_libexecdir}/ryoku
%attr(0755,root,root) %{_libexecdir}/ryoku/ryoku-helper
%attr(0755,root,root) %{_libexecdir}/ryoku/ryoku-service
%attr(0755,root,root) %{_bindir}/ryoku-hook
%attr(0755,root,root) %{_libexecdir}/ryoku/ryoku-theme-actor
%attr(0644,root,root) %{_unitdir}/ryoku-helper.service
%attr(0644,root,root) %{_unitdir}/ryoku-helper.socket
%attr(0644,root,root) %{_userunitdir}/ryoku-service.service
%attr(0644,root,root) %{_sysusersdir}/ryoku.conf

%license LICENSE
%doc packaging/rpm/README.md

