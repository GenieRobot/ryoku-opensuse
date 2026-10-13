Name:           ryoku-helper
Version:        0.1.0
Release:        1%{?dist}
Summary:        Deny-only privileged helper boundary for Ryoku
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  systemd-rpm-macros
Requires:       systemd
Requires:       rpm >= 4.19

# Tumbleweed's RPM >= 4.19 applies packaged sysusers declarations before
# unpacking the payload. No compatibility scriptlet is needed because this
# package has no files owned by the group it declares.

%description
Installs Ryoku's socket-activated privileged helper boundary. The current
helper rejects all valid operations and performs no system mutation. Access
to its socket is limited to explicitly selected members of ryoku-operators.

This build includes only the implemented user service and deny-only helper.
The recovery crate currently has no executable target and is not a standalone
recovery command.

%prep
%autosetup -n ryoku-opensuse-%{version} -a 1

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release \
    -p ryoku-service --bin ryoku-helper --bin ryoku-service

%install
install -D -m 0755 target/release/ryoku-helper \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-helper
install -D -m 0755 target/release/ryoku-service \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-service
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
%attr(0644,root,root) %{_unitdir}/ryoku-helper.service
%attr(0644,root,root) %{_unitdir}/ryoku-helper.socket
%attr(0644,root,root) %{_userunitdir}/ryoku-service.service
%attr(0644,root,root) %{_sysusersdir}/ryoku.conf

%license LICENSE
%doc packaging/rpm/README.md

