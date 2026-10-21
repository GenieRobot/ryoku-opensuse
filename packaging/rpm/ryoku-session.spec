Name:           ryoku-session
Version:        0.1.0
Release:        1%{?dist}
Summary:        Experimental Ryoku display-manager session
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  systemd-rpm-macros
Requires:       ryoku-desktop = %{version}-5
Requires:       hyprland >= 0.56.2
Requires:       hyprland-guiutils >= 0.2.1
Requires:       plasma6-session >= 6.7.5
Requires:       breeze6-wallpapers >= 6.7.5
Requires:       sddm
Requires:       xorg-x11-server
Requires:       xf86-input-libinput
Requires:       konsole
Requires:       dolphin
Requires:       util-linux
Requires:       systemd
Requires:       dbus-1-tools
Requires:       wl-clipboard

%description
Adds a separately selectable experimental Hyprland login, Rust lifecycle and
terminal adapters, immutable compositor defaults and native user units. The
pinned upstream daemon and its surfaces enter the existing Rust writer
coordinator. Installation enables no units, changes no display-manager default,
and replaces no KDE entry or user configuration. Native acceptance of this
packaged session remains pending; upstream actions are not certified protected
writes. See docs/session-port.md for limitations and the functional checklist.

%prep
%autosetup -n ryoku-opensuse-%{version} -a 1

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-session --bins

%install
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-session" \
    %{buildroot}%{_bindir}/ryoku-session
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-app" \
    %{buildroot}%{_bindir}/ryoku-app
install -D -m 0644 packaging/rpm/wayland-sessions/ryoku-opensuse.desktop \
    %{buildroot}%{_datadir}/wayland-sessions/ryoku-opensuse.desktop
install -D -m 0644 packaging/rpm/session/hyprland.conf \
    %{buildroot}%{_datadir}/ryoku-opensuse/session/hyprland.conf
install -D -m 0644 packaging/rpm/systemd/user/ryoku-desktop-session.target \
    %{buildroot}%{_userunitdir}/ryoku-desktop-session.target
install -D -m 0644 packaging/rpm/systemd/user/ryoku-desktop-shell.service \
    %{buildroot}%{_userunitdir}/ryoku-desktop-shell.service
install -D -m 0644 packaging/rpm/systemd/user/ryoku-service.service.d/25-session.conf \
    %{buildroot}%{_userunitdir}/ryoku-service.service.d/25-session.conf

%files
%defattr(-,root,root)
%{_bindir}/ryoku-session
%{_bindir}/ryoku-app
%{_datadir}/wayland-sessions/ryoku-opensuse.desktop
%dir %{_datadir}/ryoku-opensuse
%{_datadir}/ryoku-opensuse/session/
%{_userunitdir}/ryoku-desktop-session.target
%{_userunitdir}/ryoku-desktop-shell.service
%dir %{_userunitdir}/ryoku-service.service.d
%{_userunitdir}/ryoku-service.service.d/25-session.conf
%license LICENSE
%doc docs/session-port.md docs/native-desktop-workflows.md
