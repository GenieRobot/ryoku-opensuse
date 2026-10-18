%global payload_root %{_datadir}/ryoku
%global qml_root %{_libdir}/qt6/qml/Ryoku
%global font_root %{_datadir}/fonts/truetype/ryoku-opensuse
# The already published sibling packages remain at release 3.
%global dependency_release 3%{?dist}

Name:           ryoku-desktop
Version:        0.1.0
Release:        4%{?dist}
Summary:        Pinned upstream Ryoku desktop package set
License:        GPL-3.0-only
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-opensuse-%{version}.tar.gz
Source1:        ryoku-opensuse-vendor-%{version}.tar.xz
Source2:        issue14-runtime-payload.tar.gz
Source3:        ryoku-writers-98a0368.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  go >= 1.26.5
BuildRequires:  patch
BuildRequires:  cmake >= 3.28
BuildRequires:  ninja
BuildRequires:  gcc-c++
BuildRequires:  qt6-base-devel = 6.11.2
BuildRequires:  qt6-declarative-devel = 6.11.2
BuildRequires:  qt6-shadertools-devel = 6.11.2
BuildRequires:  qt6-multimedia-devel = 6.11.2
Requires:       ryoku-desktop-runtime = %{version}-%{release}
Requires:       ryoku-helper = %{version}-%{dependency_release}
Requires:       ryoku-profile-defaults = %{version}-%{dependency_release}
Requires:       ryoku-tools = %{version}-%{dependency_release}

%description
Selects the pinned upstream runtime and the implemented Rust service, recovery
and immutable defaults packages. It does not install a login session, activate
services or invoke upstream controls. Session adapters and login configuration
are implemented separately by issue 25, which depends on this package set.

%package runtime
Summary:        Pinned Ryoku surfaces and coordinated native writers
License:        GPL-3.0-only AND MIT AND BSD-3-Clause
Requires:       ryoku-quickshell = 0.3.2
Requires:       ryoku-desktop-fonts = %{version}-%{release}
Requires:       qt6-declarative-imports = 6.11.2
Requires:       qt6-multimedia-imports = 6.11.2
Requires:       qt6-qt5compat-imports = 6.11.2
Requires:       kf6-syntax-highlighting
Requires:       google-noto-sans-cjk-fonts
Requires:       systemd
Requires:       util-linux
Requires:       dbus-1-tools
Requires:       hyprlock >= 0.9.6
Requires:       hypridle >= 0.1.8
Requires:       wl-clipboard
Requires:       grim
Requires:       libnotify-tools
Requires:       pipewire
Requires:       pipewire-tools
Requires:       procps
Requires:       coreutils

%description runtime
Preserves the shell, Hub, Ryoku.Ui, FrameBars, PluginKit, Hyprland QML bridges
and translations at
Ryoku revision 98a0368253d9628be72b853985878a5db90e1547. The native Blobs
plugin is built from unchanged source against native Qt. The shell, Hyprland
provider, Hub and Ryogami Go executables retain their pinned source and vendor
closure with the reviewed issue 18 launch patch and issue 25 clipboard,
equalizer and capture surface patches. Public aliases enter the Rust writer
coordinator; native payloads are private. The clipboard watcher and screenshot
capture use Rust adapters. The units are installed without being enabled, and
no Arch installer, package actuator or privileged Go helper is added.
Installing these files does not authorize unsupported upstream actions.

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
cd ..
printf 'bf6790f07f643dc7b3f282b0ce3b918b7f0efdb5c343a7572c5dd5ab9477feed  %%s\n' '%{SOURCE3}' | sha256sum --check --status
mkdir upstream-writers
tar -xJf %{SOURCE3} -C upstream-writers
patch --batch --forward -p1 -d upstream-writers < upstream/patches/issue18-writer-launch.patch
printf 'fc821d6c8fbc0453b1922ca6f41e9151b3a48a89ee65997e288adc0929589fe1  %%s\n' 'upstream/patches/0002-rust-screenshot-adapter.patch' | sha256sum --check --status
patch --batch --forward -p1 -d upstream-writers/ryoku < upstream/patches/0002-rust-screenshot-adapter.patch
printf '36bde7eab1b966d5c39b991cb921539f7a5724686d9194a6a311cbb107dd41d4  %%s\n' 'upstream/patches/0003-rust-equalizer-adapter.patch' | sha256sum --check --status
patch --batch --forward -p1 -d upstream-writers/ryoku < upstream/patches/0003-rust-equalizer-adapter.patch
printf '9004032b14def1ba2a32536d51262f1f5a36699e78b6d0cae2c91ecfd32a8970  %%s\n' 'packaging/rpm/patches/0001-clipboard-operation-errors.patch' | sha256sum --check --status
patch --batch --forward -p1 -d upstream-writers/ryoku < packaging/rpm/patches/0001-clipboard-operation-errors.patch
printf '16837b3f08edd4e5a42c418e477441e13f3ec9e71a20d17e262248d4bebcbb59  %%s\n' 'packaging/rpm/patches/0004-disable-upstream-clipboard-history.patch' | sha256sum --check --status
patch --batch --forward -p1 -d upstream-writers < packaging/rpm/patches/0004-disable-upstream-clipboard-history.patch

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-recovery --bin ryoku-writer
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p ryoku-session --bins
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
cp -a upstream-writers/ryoku/shell/ryogami/wall-ui/. %{buildroot}%{_datadir}/ryogami/
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-writer" %{buildroot}%{_libexecdir}/ryoku/ryoku-writer
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-capture" %{buildroot}%{_bindir}/ryoku-capture
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-eq" %{buildroot}%{_bindir}/ryoku-eq
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/ryoku-clipboard" %{buildroot}%{_libexecdir}/ryoku/ryoku-clipboard
install -D -m 0644 packaging/rpm/systemd/user/ryoku-clipboard.service %{buildroot}%{_userunitdir}/ryoku-clipboard.service
install -D -m 0644 packaging/rpm/systemd/user/ryoku-eq.service %{buildroot}%{_userunitdir}/ryoku-eq.service
install -D -m 0644 packaging/rpm/systemd/user/ryoku-hypridle.service %{buildroot}%{_userunitdir}/ryoku-hypridle.service
install -D -m 0644 packaging/rpm/session/hypridle.conf %{buildroot}%{_datadir}/ryoku-opensuse/session/hypridle.conf
install -D -m 0644 packaging/rpm/session/hyprlock.conf %{buildroot}%{_datadir}/ryoku-opensuse/session/hyprlock.conf
install -D -m 0755 upstream-writers/ryoku-shell %{buildroot}%{_libexecdir}/ryoku/upstream/ryoku-shell
install -D -m 0755 upstream-writers/ryoku-wm-hyprland %{buildroot}%{_libexecdir}/ryoku/upstream/ryoku-wm-hyprland
install -D -m 0755 upstream-writers/ryoku-hub %{buildroot}%{_libexecdir}/ryoku/upstream/ryoku-hub
install -D -m 0755 upstream-writers/ryogami %{buildroot}%{_libexecdir}/ryoku/upstream/ryogami
install -d %{buildroot}%{_bindir} %{buildroot}%{_libexecdir}/ryoku/quickshell
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryoku-shell
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryoku-wm-hyprland
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryoku-hub
ln -s %{_libexecdir}/ryoku/ryoku-writer %{buildroot}%{_bindir}/ryogami
ln -s bin/qs %{buildroot}%{_libexecdir}/ryoku/quickshell/qs
install -D -m 0755 blobs-build/qml/Ryoku/Blobs/libryoku-blobs.so \
    %{buildroot}%{qml_root}/Blobs/libryoku-blobs.so
install -D -m 0755 blobs-build/qml/Ryoku/Blobs/libryoku-blobsplugin.so \
    %{buildroot}%{qml_root}/Blobs/libryoku-blobsplugin.so
install -D -m 0644 blobs-build/qml/Ryoku/Blobs/qmldir \
    %{buildroot}%{qml_root}/Blobs/qmldir
install -D -m 0644 blobs-build/qml/Ryoku/Blobs/ryoku-blobs.qmltypes \
    %{buildroot}%{qml_root}/Blobs/ryoku-blobs.qmltypes
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
%{_datadir}/ryogami/
%{qml_root}/
%dir %{_libexecdir}/ryoku
%{_libexecdir}/ryoku/ryoku-writer
%{_libexecdir}/ryoku/ryoku-clipboard
%{_bindir}/ryoku-capture
%{_bindir}/ryoku-eq
%{_userunitdir}/ryoku-clipboard.service
%{_userunitdir}/ryoku-eq.service
%{_userunitdir}/ryoku-hypridle.service
%dir %{_datadir}/ryoku-opensuse
%{_datadir}/ryoku-opensuse/session/
%{_libexecdir}/ryoku/upstream/
%dir %{_libexecdir}/ryoku/quickshell
%{_libexecdir}/ryoku/quickshell/qs
%{_bindir}/ryoku-shell
%{_bindir}/ryoku-wm-hyprland
%{_bindir}/ryoku-hub
%{_bindir}/ryogami
%license runtime/LICENSE runtime/NOTICE upstream-writers/licenses/
%doc packaging/rpm/desktop-build.md upstream/ryoku-runtime.toml runtime/SHA256SUMS

%files fonts
%defattr(0644,root,root,0755)
%{font_root}/
%doc runtime/FONT-FAMILIES.txt upstream/ryoku-runtime.toml
