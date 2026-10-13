Name:           ryoku-helper
Version:        0.1.0
Release:        1%{?dist}
Summary:        Deny-only privileged helper boundary for Ryoku
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  systemd-rpm-macros
Requires:       systemd

# Tumbleweed's RPM >= 4.19 applies packaged sysusers declarations before
# unpacking the payload. No compatibility scriptlet is needed because this
# package has no files owned by the group it declares.

%description
Installs Ryoku's socket-activated privileged helper boundary. The current
helper rejects all valid operations and performs no system mutation. Access
to its socket is limited to explicitly selected members of ryoku-operators.

%prep
%autosetup -n ryoku-opensuse-%{version}

%build
cargo build --locked --release -p ryoku-service --bin ryoku-helper

%install
install -D -m 0755 target/release/ryoku-helper \
    %{buildroot}%{_libexecdir}/ryoku/ryoku-helper
install -D -m 0644 packaging/rpm/systemd/ryoku-helper.service \
    %{buildroot}%{_unitdir}/ryoku-helper.service
install -D -m 0644 packaging/rpm/systemd/ryoku-helper.socket \
    %{buildroot}%{_unitdir}/ryoku-helper.socket
install -D -m 0644 packaging/rpm/sysusers.d/ryoku.conf \
    %{buildroot}%{_sysusersdir}/ryoku.conf

%files
%{_libexecdir}/ryoku/ryoku-helper
%{_unitdir}/ryoku-helper.service
%{_unitdir}/ryoku-helper.socket
%{_sysusersdir}/ryoku.conf

%license LICENSE
%doc packaging/rpm/README.md

