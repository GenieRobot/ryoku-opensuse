%global payload_root %{_datadir}/ryoku-opensuse/upstream
%global font_root %{_datadir}/fonts/truetype/ryoku-opensuse

Name:           ryoku-desktop
Version:        0.1.0
Release:        2%{?dist}
Summary:        Experimental native Rust Ryoku desktop session
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
Source2:        issue14-runtime-payload.tar.gz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  cmake >= 3.28
BuildRequires:  ninja
BuildRequires:  gcc-c++
BuildRequires:  systemd-rpm-macros
BuildRequires:  qt6-base-devel = 6.11.2
BuildRequires:  qt6-declarative-devel = 6.11.2
BuildRequires:  qt6-shadertools-devel = 6.11.2
BuildRequires:  qt6-multimedia-devel = 6.11.2
Requires:       ryoku-desktop-runtime = %{version}-%{release}
Requires:       ryoku-helper = %{version}
Requires:       hyprland >= 0.56.2
Requires:       hyprland-guiutils >= 0.2.1
# Keep the independent KDE/SDDM fallback complete with --no-recommends.
Requires:       plasma6-session >= 6.7.5
Requires:       breeze6-wallpapers >= 6.7.5
Requires:       sddm
Requires:       xorg-x11-server
Requires:       xf86-input-libinput
Requires:       konsole
Requires:       dolphin
Requires:       util-linux
Requires:       coreutils
Requires:       systemd
Requires:       dbus-1-tools

%description
Installs the Rust ryoku-session and terminal-only ryoku-app adapters with a
separately selectable Hyprland session, two user units and isolated immutable
compositor defaults. No units are enabled and no existing session, user
configuration, recovery identity or mutable state is installed or replaced.
The preserved upstream UI has unsupported actions; this is a disposable-VM
prototype and does not certify those actions or complete issues 14 or 23.

%package runtime
Summary:        Unchanged pinned Ryoku surfaces and native Blobs module
Requires:       ryoku-quickshell = 0.3.2
Requires:       ryoku-desktop-fonts = %{version}-%{release}
Requires:       qt6-declarative-imports = 6.11.2
Requires:       qt6-multimedia-imports = 6.11.2
Requires:       qt6-qt5compat-imports = 6.11.2
Requires:       kf6-syntax-highlighting
Requires:       google-noto-sans-cjk-fonts

%description runtime
Preserves the shell, Hub, Ryoku.Ui, FrameBars, PluginKit and translations at
Ryoku revision 98a0368253d9628be72b853985878a5db90e1547. The native Blobs
plugin is built from unchanged source against native Qt. Upstream QML,
JavaScript, auxiliary scripts and assets retain their original bytes and
notices. No Go daemon, Ryostore, Arch installer or privileged helper is added.
Retained upstream auxiliary scripts are not RPM install or removal hooks.

%package fonts
Summary:        Pinned fonts for the preserved Ryoku desktop
License:        Apache-2.0 AND OFL-1.1
BuildArch:      noarch
Requires:       fontconfig

%description fonts
Unchanged Material Symbols Rounded, Space Grotesk, Space Mono, SpaceMono Nerd
Font and Fraunces files and licenses from the immutable sources and SHA-256
digests in upstream/ryoku-runtime.toml. Files use a dedicated system font
directory. Fontconfig's distribution-owned triggers manage font discovery;
this package has no custom cache script or home-directory installation.

%prep
printf '81151cef1a613b01ad8e6187df2e93cbfed70e1621de0ee2e7455d2904fc3e74  %%s\n' '%{SOURCE2}' | sha256sum --check --status
%autosetup -n ryoku-opensuse-%{version} -a 1
mkdir runtime
tar -xzf %{SOURCE2} -C runtime
cd runtime
sha256sum --check --status SHA256SUMS

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-session --bins
cmake -S runtime/native/ryoku-blobs -B blobs-build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DFETCHCONTENT_FULLY_DISCONNECTED=ON
cmake --build blobs-build --parallel %{_smp_build_ncpus}

%install
install -D -m 0755 target/release/ryoku-session %{buildroot}%{_bindir}/ryoku-session
install -D -m 0755 target/release/ryoku-app %{buildroot}%{_bindir}/ryoku-app
install -D -m 0644 packaging/rpm/wayland-sessions/ryoku-opensuse.desktop \
    %{buildroot}%{_datadir}/wayland-sessions/ryoku-opensuse.desktop
install -D -m 0644 packaging/rpm/systemd/user/ryoku-desktop-session.target \
    %{buildroot}%{_userunitdir}/ryoku-desktop-session.target
install -D -m 0644 packaging/rpm/systemd/user/ryoku-desktop-shell.service \
    %{buildroot}%{_userunitdir}/ryoku-desktop-shell.service
install -D -m 0644 packaging/rpm/session/hyprland.conf \
    %{buildroot}%{_datadir}/ryoku-opensuse/session/hyprland.conf
install -D -m 0644 packaging/rpm/session/hyprland-nested.conf \
    %{buildroot}%{_datadir}/ryoku-opensuse/session/hyprland-nested.conf
install -d %{buildroot}%{payload_root}/i18n
cp -a runtime/shell runtime/hub runtime/qml %{buildroot}%{payload_root}/
cp -a runtime/i18n/catalog %{buildroot}%{payload_root}/i18n/
install -D -m 0755 blobs-build/qml/Ryoku/Blobs/libryoku-blobs.so \
    %{buildroot}%{payload_root}/qml/Ryoku/Blobs/libryoku-blobs.so
install -D -m 0755 blobs-build/qml/Ryoku/Blobs/libryoku-blobsplugin.so \
    %{buildroot}%{payload_root}/qml/Ryoku/Blobs/libryoku-blobsplugin.so
install -D -m 0644 blobs-build/qml/Ryoku/Blobs/qmldir \
    %{buildroot}%{payload_root}/qml/Ryoku/Blobs/qmldir
install -D -m 0644 blobs-build/qml/Ryoku/Blobs/ryoku-blobs.qmltypes \
    %{buildroot}%{payload_root}/qml/Ryoku/Blobs/ryoku-blobs.qmltypes
install -d %{buildroot}%{font_root}
cp -a runtime/fonts/. %{buildroot}%{font_root}/

%files
%defattr(-,root,root)
%attr(0755,root,root) %{_bindir}/ryoku-session
%attr(0755,root,root) %{_bindir}/ryoku-app
%attr(0644,root,root) %{_datadir}/wayland-sessions/ryoku-opensuse.desktop
%attr(0644,root,root) %{_userunitdir}/ryoku-desktop-session.target
%attr(0644,root,root) %{_userunitdir}/ryoku-desktop-shell.service
%dir %{_datadir}/ryoku-opensuse
%{_datadir}/ryoku-opensuse/session/
%license LICENSE
%doc docs/session-port.md docs/terminal-adapter.md packaging/rpm/desktop-build.md

%files runtime
%defattr(-,root,root)
%dir %{_datadir}/ryoku-opensuse
%{payload_root}/
%license runtime/LICENSE runtime/NOTICE
%doc docs/real-ryoku-runtime.md upstream/ryoku-runtime.toml runtime/SHA256SUMS

%files fonts
%defattr(0644,root,root,0755)
%{font_root}/
%doc runtime/FONT-FAMILIES.txt upstream/ryoku-runtime.toml
