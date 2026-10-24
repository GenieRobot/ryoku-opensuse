%global payload_root %{_datadir}/ryoku
%global qml_root %{_libdir}/qt6/qml/Ryoku
%global font_root %{_datadir}/fonts/truetype/ryoku-opensuse
# The SDK can contain Qt5 and Qt6; the native generator must scan this Qt6
# payload explicitly rather than silently failing its Qt-version guess.
%global __qml_requires %{_rpmconfigdir}/qml.req --qtver 6

Name:           ryoku-desktop
Version:        0.1.0
Release:        8%{?dist}
Summary:        Pinned upstream Ryoku desktop package set
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
Source2:        ryoku-runtime-a59d051.tar.gz
Source3:        ryoku-writers-a59d051.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  go >= 1.26.5
BuildRequires:  patch
BuildRequires:  cmake >= 3.28
BuildRequires:  ninja
BuildRequires:  gcc-c++
BuildRequires:  qt6-base-devel = 6.11.2
BuildRequires:  qt6-concurrent-devel = 6.11.2
BuildRequires:  qt6-declarative-devel = 6.11.2
BuildRequires:  qt6-shadertools-devel = 6.11.2
BuildRequires:  qt6-multimedia-devel = 6.11.2
BuildRequires:  qt6-quick3d-devel = 6.11.2
BuildRequires:  qt6-gui-private-devel = 6.11.2
BuildRequires:  vulkan-headers
BuildRequires:  wayland-devel
BuildRequires:  wayland-protocols-devel
BuildRequires:  pkgconf-pkg-config
BuildRequires:  pkgconfig(libavformat)
BuildRequires:  pkgconfig(libavcodec)
BuildRequires:  pkgconfig(libavutil)
BuildRequires:  pkgconfig(libswscale)
BuildRequires:  qml-autoreqprov
BuildRequires:  qmlpluginexports-qt6
Requires:       ryoku-desktop-runtime = %{version}-%{release}
Requires:       ryoku-helper = %{version}-%{release}
Requires:       ryoku-profile-defaults = %{version}-%{release}
Requires:       ryoku-tools = %{version}-%{release}

%description
Selects the pinned upstream runtime and the implemented Rust service, recovery
and immutable defaults packages. It does not install a login session, activate
services or invoke upstream controls. Session adapters and login configuration
are implemented separately by issue 25, which depends on this package set.

%package runtime
Summary:        Pinned Ryoku surfaces and coordinated native writers
License:        GPL-3.0-only AND GPL-3.0-or-later AND GPL-2.0-or-later AND AGPL-3.0-or-later AND Apache-2.0 AND MIT AND BSD-3-Clause AND OFL-1.1 AND CC-BY-3.0 AND LicenseRef-sound-theme-freedesktop-0.8-CC-BY-SA
Requires:       ryoku-quickshell = 0.3.2-3
Requires:       ryoku-desktop-fonts = %{version}-%{release}
Requires:       qt6-declarative-imports = 6.11.2
Requires:       qt6-multimedia-imports = 6.11.2
Requires:       qt6-quick3d-imports = 6.11.2
Requires:       qt6-qt5compat-imports = 6.11.2
Requires:       qt6-imageformats
Requires:       ImageMagick
Requires:       ffmpeg
# WaveCapture starts these commands with QProcess; ELF scanning cannot find them.
Requires:       pulseaudio-utils
Requires:       pipewire-tools
Requires:       kf6-syntax-highlighting-imports = 6.31.0
Requires:       google-noto-sans-cjk-fonts
Requires:       systemd
Requires:       util-linux
Requires:       dbus-1-tools
# These qmldir modules are private to the shell's import root under /usr/share.
# Tumbleweed's generator recognizes provides only below /qt6/qml/, while its
# requires scanner also sees these private module imports. Keep all scanning
# enabled and declare exactly the two modules shipped in this package.
Provides:       qt6qmlimport(shell.services)
Provides:       qt6qmlimport(shell.barkit)
# Upstream ships directory imports with singleton exports but no module line.
Provides:       qt6qmlimport(Ryoku.Ui.Singletons)
Provides:       qt6qmlimport(Ryoku.PluginKit.Singletons)
Provides:       qt6qmlimport(Ryoku.Ryogami)

%description runtime
Preserves selected Ryoku 1.0.0 shell, Hub, Ryoku.Ui, FrameBars, PluginKit,
Hyprland QML bridges, Ryogami picker, helper scripts and translations at
revision a59d051f0e8080052216f6c7b9eb7e5928935ce8. Blobs and Spatial3D native
modules are built from that source against native Qt. The shell, Hyprland
provider, Hub and Ryogami Go executables retain their pinned source and vendor
closure with the reviewed v1.0 issue 18 launch patch. Public aliases enter the
Rust writer coordinator; native payloads are private. No upstream custom
greeter, PAM setup, display-manager mutation, Arch installer, package actuator
or privileged Go helper is installed. Installing these files does not
authorize unsupported upstream actions.

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
printf '8aeff20645c14af76f3d341e5d4e7c31923fd249f6671caa704c8b23a6c84e65  %%s\n' '%{SOURCE2}' | sha256sum --check --status
%autosetup -n ryoku-opensuse-%{version} -a 1
mkdir runtime
tar -xzf %{SOURCE2} -C runtime
cd runtime
sha256sum --check --status SHA256SUMS
cd ..
printf '7b68eb2e1a81fe3d6b28edaa89708b2c6663082e72f74bdce4087eb02799c9be  %%s\n' '%{SOURCE3}' | sha256sum --check --status
mkdir upstream-writers
tar -xJf %{SOURCE3} -C upstream-writers
patch --batch --forward --fuzz=0 -p1 -d upstream-writers < upstream/patches/issue18-writer-launch.patch

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-recovery --bin ryoku-writer
cd upstream-writers/ryoku/shell/ipc
GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0 \
    go build -trimpath -mod=vendor -buildvcs=false -o ../../../ryoku-shell .
cd ../../..
cd ryoku/wm/hyprland
GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0 \
    go build -trimpath -mod=vendor -buildvcs=false -o ../../../ryoku-wm-hyprland .
cd ../../..
cd ryoku/hub/backend
GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0 \
    go build -trimpath -mod=vendor -buildvcs=false -o ../../../ryoku-hub .
cd ../../..
cd ryoku/shell/ryogami/daemon
GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0 \
    go build -trimpath -mod=vendor -buildvcs=false -o ../../../../ryogami .
cd ../../../..
mkdir -p licenses/go
cp -a %{_docdir}/go/"$(basename "$(go env GOROOT)")"/LICENSE licenses/go/LICENSE
find ryoku -path '*/vendor/*' -type f \
    \( -iname '*license*' -o -iname '*copying*' -o -iname '*notice*' \) \
    -exec cp --parents -t licenses {} +
cd ..
cmake -S runtime/native/ryoku-blobs -B blobs-build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DFETCHCONTENT_FULLY_DISCONNECTED=ON
cmake --build blobs-build --parallel %{_smp_build_ncpus}
cmake -S upstream-writers/ryoku/shell/ryogami/picker -B picker-build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release
cmake --build picker-build --parallel %{_smp_build_ncpus}
upstream-writers/ryoku/shell/livewall/build.sh ryogami-live

%install
install -d %{buildroot}%{payload_root}/shell %{buildroot}%{payload_root}/hub %{buildroot}%{payload_root}/i18n
cp -a upstream-writers/ryoku/shell/quickshell %{buildroot}%{payload_root}/shell/
cp -a upstream-writers/ryoku/hub/quickshell %{buildroot}%{payload_root}/hub/
cp -a runtime/i18n/catalog %{buildroot}%{payload_root}/i18n/
install -d %{buildroot}%{_libdir}/qt6/qml
cp -a runtime/qml/Ryoku %{buildroot}%{_libdir}/qt6/qml/
install -d %{buildroot}%{qml_root}/Wm
cp -a upstream-writers/ryoku/wm/hyprland/qml %{buildroot}%{qml_root}/Wm/Hyprland
install -d %{buildroot}%{_datadir}/ryogami
install -D -m 0644 upstream-writers/ryoku/shell/ryogami/picker/shell.qml \
    %{buildroot}%{_datadir}/ryogami/shell.qml
install -D -m 0644 upstream-writers/ryoku/shell/ryogami/picker/data/ryogami.desktop \
    %{buildroot}%{_datadir}/applications/ryogami.desktop
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-writer" %{buildroot}%{_libexecdir}/ryoku/ryoku-writer
install -D -m 0755 upstream-writers/ryoku-shell %{buildroot}%{_libexecdir}/ryoku/upstream/ryoku-shell
install -D -m 0755 upstream-writers/ryoku-wm-hyprland %{buildroot}%{_libexecdir}/ryoku/upstream/ryoku-wm-hyprland
install -D -m 0755 upstream-writers/ryoku-hub %{buildroot}%{_libexecdir}/ryoku/upstream/ryoku-hub
install -D -m 0755 upstream-writers/ryogami %{buildroot}%{_libexecdir}/ryoku/upstream/ryogami
install -D -m 0755 upstream-writers/ryoku/shell/scripts/ryoku-cmd-ryoshot \
    %{buildroot}%{_bindir}/ryoku-cmd-ryoshot
install -D -m 0755 upstream-writers/ryoku/shell/scripts/ryoku-session-intro \
    %{buildroot}%{_bindir}/ryoku-session-intro
install -d %{buildroot}%{_bindir} %{buildroot}%{_libexecdir}/ryoku/quickshell
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryoku-shell
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryoku-wm-hyprland
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryoku-hub
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryogami
ln -s bin/qs %{buildroot}%{_libexecdir}/ryoku/quickshell/qs
install -d %{buildroot}%{qml_root}
cp -a blobs-build/qml/Ryoku/Blobs %{buildroot}%{qml_root}/
cp -a blobs-build/qml/Ryoku/Spatial3D %{buildroot}%{qml_root}/
cp -a picker-build/qml/Ryoku/Ryogami %{buildroot}%{qml_root}/
find %{buildroot}%{qml_root} -type f -name '*.qrc' -delete
install -D -m 0755 ryogami-live %{buildroot}%{_bindir}/ryogami-live
install -d %{buildroot}%{font_root}
cp -a runtime/fonts/. %{buildroot}%{font_root}/

%files
%defattr(-,root,root)
%license LICENSE
%doc packaging/rpm/desktop-build.md

%files runtime
%defattr(-,root,root)
%dir %{payload_root}
%{payload_root}/shell/
%{payload_root}/hub/
%{payload_root}/i18n/
%{_datadir}/ryogami/shell.qml
%{_datadir}/applications/ryogami.desktop
%{qml_root}/
%dir %{_libexecdir}/ryoku
%{_libexecdir}/ryoku/ryoku-writer
%{_libexecdir}/ryoku/upstream/
%dir %{_libexecdir}/ryoku/quickshell
%{_libexecdir}/ryoku/quickshell/qs
%{_bindir}/ryoku-shell
%{_bindir}/ryoku-wm-hyprland
%{_bindir}/ryoku-hub
%{_bindir}/ryogami
%{_bindir}/ryogami-live
%{_bindir}/ryoku-cmd-ryoshot
%{_bindir}/ryoku-session-intro
%license runtime/LICENSE runtime/NOTICE upstream-writers/licenses/
%license packaging/licenses/ryoku-v1.0/
%doc packaging/rpm/desktop-build.md upstream/ryoku-runtime.toml runtime/SHA256SUMS

%files fonts
%defattr(0644,root,root,0755)
%{font_root}/
%doc runtime/FONT-FAMILIES.txt upstream/ryoku-runtime.toml
