Name:           ryoku-ui-proof
Version:        0.1.0
Release:        1%{?dist}
Summary:        GTK proof for Ryoku's typed read-only UI boundary
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  desktop-file-utils
Requires:       libgtk-4-1

%description
Installs the experimental GTK desktop proof for Ryoku's typed, read-only
status action. The default backend displays fixed demonstration data; it is
not a package manager or a production Ryoku desktop application.

%prep
%autosetup -n ryoku-opensuse-%{version} -a 1

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release \
    -p ryoku-ui --features gtk-ui --bin ryoku-ui-gtk

%check
desktop-file-validate packaging/rpm/applications/org.ryoku.OpenSUSE.UiProof.desktop

%install
install -D -m 0755 target/release/ryoku-ui-gtk \
    %{buildroot}%{_bindir}/ryoku-ui-gtk
install -D -m 0644 packaging/rpm/applications/org.ryoku.OpenSUSE.UiProof.desktop \
    %{buildroot}%{_datadir}/applications/org.ryoku.OpenSUSE.UiProof.desktop
install -D -m 0644 packaging/rpm/icons/org.ryoku.OpenSUSE.UiProof.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/org.ryoku.OpenSUSE.UiProof.svg

%files
%attr(0755,root,root) %{_bindir}/ryoku-ui-gtk
%attr(0644,root,root) %{_datadir}/applications/org.ryoku.OpenSUSE.UiProof.desktop
%attr(0644,root,root) %{_datadir}/icons/hicolor/scalable/apps/org.ryoku.OpenSUSE.UiProof.svg
%license LICENSE
