%global upstream_revision 4f508be500dea6e5732cc3d50382a0048b17e7b1
%global runtime_prefix %{_libexecdir}/ryoku/quickshell

Name:           ryoku-quickshell
Version:        0.3.2
Release:        3%{?dist}
Summary:        Pinned private Quickshell runtime for the experimental Ryoku desktop
License:        LGPL-3.0-only
URL:            https://git.outfoxxed.me/quickshell/quickshell
Source0:        issue14-quickshell-0.3.2.tar.gz
ExclusiveArch:  x86_64
BuildRequires:  cmake >= 3.28
BuildRequires:  ninja
BuildRequires:  gcc-c++
BuildRequires:  pkgconf
BuildRequires:  cli11-devel
BuildRequires:  cpptrace-devel
BuildRequires:  jemalloc-devel
BuildRequires:  libdrm-devel
BuildRequires:  libgbm-devel
BuildRequires:  Mesa-libEGL-devel
BuildRequires:  vulkan-headers
BuildRequires:  wayland-devel
BuildRequires:  wayland-protocols-devel >= 1.41
BuildRequires:  libxcb-devel
BuildRequires:  pipewire-devel
BuildRequires:  pam-devel
BuildRequires:  polkit-devel
BuildRequires:  glib2-devel
BuildRequires:  spirv-tools-devel
BuildRequires:  qt6-base-devel = 6.11.2
BuildRequires:  qt6-declarative-devel = 6.11.2
BuildRequires:  qt6-shadertools-devel = 6.11.2
BuildRequires:  qt6-quick-private-devel = 6.11.2
BuildRequires:  qt6-waylandclient-private-devel = 6.11.2
BuildRequires:  qt6-gui-private-devel = 6.11.2

# Upstream links these modules into qs. They have no dynamically installed
# qmldir/plugin pair for the distribution's automatic provides generator.
# These are the public modules imported by the pinned Ryoku payload.
Provides:       qt6qmlimport(Quickshell)
Provides:       qt6qmlimport(Quickshell.Bluetooth)
Provides:       qt6qmlimport(Quickshell.Hyprland)
Provides:       qt6qmlimport(Quickshell.Io)
Provides:       qt6qmlimport(Quickshell.Networking)
Provides:       qt6qmlimport(Quickshell.Services.Mpris)
Provides:       qt6qmlimport(Quickshell.Services.Notifications)
Provides:       qt6qmlimport(Quickshell.Services.Pipewire)
Provides:       qt6qmlimport(Quickshell.Services.SystemTray)
Provides:       qt6qmlimport(Quickshell.Services.UPower)
Provides:       qt6qmlimport(Quickshell.Wayland)
Provides:       qt6qmlimport(Quickshell.Widgets)
Provides:       qt6qmlimport(Quickshell.WindowManager)

%description
Builds unchanged official Quickshell 0.3.2 at the immutable upstream revision
4f508be500dea6e5732cc3d50382a0048b17e7b1 against the reviewed native Qt SDK.
The default upstream features and cpptrace signal-safe unwind check remain
enabled. No build-time dependency downloads or source patches are used.
Only the Ryoku-private prefix is owned; the distribution's qs is independent.
This package is an experimental dependency, not desktop acceptance evidence.

%prep
printf '5e8a7b7a1a1e40de641ee6cfd24f40aa612e13c27353e5746fc79b2861ca9fd0  %%s\n' '%{SOURCE0}' | sha256sum --check --status
%autosetup -n quickshell-%{upstream_revision}

%build
cmake -S . -B build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=%{runtime_prefix} \
    -DGIT_REVISION=%{upstream_revision} \
    -DDISTRIBUTOR=Ryoku-openSUSE-experimental \
    -DVENDOR_CPPTRACE=OFF \
    -DFETCHCONTENT_FULLY_DISCONNECTED=ON
cmake --build build --parallel %{_smp_build_ncpus}

%install
DESTDIR=%{buildroot} cmake --install build

%files
%defattr(-,root,root)
%dir %{_libexecdir}/ryoku
%{runtime_prefix}/
%license LICENSE LICENSE-GPL
%doc README.md BUILD.md
