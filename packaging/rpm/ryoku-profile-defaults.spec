Name:           ryoku-profile-defaults
Version:        0.1.0
Release:        8%{?dist}
Summary:        Generic declarative defaults for the Ryoku openSUSE port
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
BuildArch:      noarch

# New port-owned data, not an upstream Ryoku configuration import.
# SHA-256 digests are part of the installed-default verification contract.
%global _binary_filedigest_algorithm 8

%description
Provides the generic native profile defaults for the Ryoku openSUSE port:
Hyprland, the Ryoku dark theme, and comfortable density. This declarative
payload requires no companion profile, remote repository, or credentials.
It does not activate a desktop or profile and does not modify home files.

%prep
%autosetup -n ryoku-opensuse-%{version}

%build

%install
install -D -m 0644 packaging/rpm/defaults/profile.json \
    %{buildroot}%{_datadir}/ryoku/config/profile.json

%files
%dir %attr(0755,root,root) %{_datadir}/ryoku
%dir %attr(0755,root,root) %{_datadir}/ryoku/config
%attr(0644,root,root) %{_datadir}/ryoku/config/profile.json

%license LICENSE
%doc packaging/rpm/defaults/README.md
