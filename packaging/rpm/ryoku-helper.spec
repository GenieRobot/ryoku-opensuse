Name:           ryoku-helper
Version:        0.1.0
Release:        2%{?dist}
Summary:        Read-only recovery gate and privileged boundary for Ryoku
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
Installs Ryoku's socket-activated privileged helper and read-only recovery
session gate. The helper verifies exact linked checkpoints and rejects
mutation requests. Access to its socket is limited to explicitly selected
members of ryoku-operators.

This build includes the user service, read-only linked-checkpoint helper, and
session recovery gate. Offline recovery commands are packaged separately.

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
install -D -m 0644 packaging/rpm/systemd/ryoku-recovery-gate.service \
    %{buildroot}%{_unitdir}/ryoku-recovery-gate.service
install -D -m 0644 packaging/rpm/systemd/ryoku-recovery-gate.socket \
    %{buildroot}%{_unitdir}/ryoku-recovery-gate.socket
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
%attr(0644,root,root) %{_unitdir}/ryoku-recovery-gate.service
%attr(0644,root,root) %{_unitdir}/ryoku-recovery-gate.socket
%attr(0644,root,root) %{_userunitdir}/ryoku-service.service
%attr(0644,root,root) %{_sysusersdir}/ryoku.conf

%license LICENSE
%doc packaging/rpm/README.md

