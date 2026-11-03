%global upstream_revision a59d051f0e8080052216f6c7b9eb7e5928935ce8
%global upstream_version 1.0.0
%global private_root %{_libexecdir}/ryoku/upstream

Name:           ryoku-rashin
Version:        0.1.0
Release:        8%{?dist}
Summary:        Pinned native Rashin backend, companion and account data
License:        GPL-3.0-only AND BSD-3-Clause AND ISC AND OFL-1.1 AND LicenseRef-Rashin-Embedded-Web-Review-Pending
URL:            https://github.com/GenieRobot/ryoku-opensuse
Source0:        ryoku-writers-a59d051.tar.xz
Source1:        ryoku-rashin-context-a59d051.md
Source2:        ryoku-rashin-source-a59d051.json
ExclusiveArch:  x86_64
BuildRequires:  go >= 1.26.5
BuildRequires:  cmake >= 3.20
BuildRequires:  ninja
BuildRequires:  gcc-c++
BuildRequires:  pkgconf-pkg-config
BuildRequires:  pkgconfig(gtk+-3.0)
BuildRequires:  pkgconfig(webkit2gtk-4.1)
Requires:       ryoku-desktop-runtime = %{version}-%{release}
Requires:       prowl >= 0.17.2
Requires:       uv
Requires:       nodejs24
Requires:       gcc
Requires:       python3

%description
Builds the retained Go backend, embedded Svelte console and C++ GTK/WebKit
companion from upstream Ryoku %{upstream_version} at %{upstream_revision}.
The RPM version follows the co-versioned Tumbleweed port candidate policy.
The package preserves native source behavior and installs private executables,
skills, wiki, usage collectors and the full pinned repository context.

This candidate is not selected by the desktop metapackage. Public CLI aliases,
desktop launchers and active user units require issue 183's tested authenticated
transport and writer/recovery integration. Bundled JavaScript notices/source
closure, Prowl closure and native package/session acceptance remain open.

%prep
printf '387d829860ba844ad32ecdd5982678707665f3251e41bbb43c6fd6f87dc7eee0  %%s\n' '%{SOURCE0}' | sha256sum --check --status
printf '7441655d8f9d384f4e1b292d6e44de3a2c4fbccf097108e09578927602ec97d8  %%s\n' '%{SOURCE1}' | sha256sum --check --status
printf '71c0668104187108fe013ac58b383f301fad128b519a4e2f41b1ba761988b58c  %%s\n' '%{SOURCE2}' | sha256sum --check --status
%setup -q -c -n %{name}-%{version}

%build
export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0
mkdir -p build licenses/go licenses/websocket licenses/fonts
go build -C ryoku/rashin/backend -trimpath -mod=vendor -buildvcs=false \
    -o "$PWD/build/ryoku-rashin" .
cmake -S ryoku/rashin/app -B build/app -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DFETCHCONTENT_FULLY_DISCONNECTED=ON
cmake --build build/app --parallel %{_smp_build_ncpus}
cp -a %{_defaultdocdir}/go/"$(basename "$(go env GOROOT)")"/LICENSE licenses/go/LICENSE
cp -a ryoku/rashin/backend/vendor/github.com/coder/websocket/LICENSE.txt licenses/websocket/LICENSE.txt
cp -a ryoku/rashin/web/src/fonts/OFL.txt licenses/fonts/OFL.txt

%check
GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOWORK=off CGO_ENABLED=0 \
    go test -C ryoku/rashin/backend -mod=vendor \
    -run '^TestRepoIndexDoc(LayoutAndDocs|RejectsMissingRoot)$' .

%install
install -D -m 0755 build/ryoku-rashin %{buildroot}%{private_root}/ryoku-rashin
install -D -m 0755 build/app/rashin-app %{buildroot}%{private_root}/rashin-app
install -D -m 0644 %{SOURCE1} %{buildroot}%{_datadir}/ryoku/rashin/ryoku-repo.md
install -D -m 0644 %{SOURCE2} %{buildroot}%{_datadir}/ryoku/rashin/source.json
install -d %{buildroot}%{_datadir}/ryoku/rashin/wiki %{buildroot}%{_datadir}/ryoku/skills/ryoku
install -m 0644 ryoku/rashin/wiki/*.md %{buildroot}%{_datadir}/ryoku/rashin/wiki/
install -m 0644 ryoku/rashin/skills/ryoku/*.md %{buildroot}%{_datadir}/ryoku/skills/ryoku/
install -d %{buildroot}%{private_root}/rashin-usage
install -m 0755 ryoku/rashin/usage/ryoku-ai-usage ryoku/rashin/usage/claude-usage \
    ryoku/rashin/usage/codex-usage ryoku/rashin/usage/opencode-usage \
    %{buildroot}%{private_root}/rashin-usage/

%files
%defattr(-,root,root)
%dir %{_libexecdir}/ryoku
%dir %{private_root}
%{private_root}/ryoku-rashin
%{private_root}/rashin-app
%{private_root}/rashin-usage/
%dir %{_datadir}/ryoku
%{_datadir}/ryoku/rashin/
%dir %{_datadir}/ryoku/skills
%{_datadir}/ryoku/skills/ryoku/
%license LICENSE NOTICE licenses/
%doc ryoku/rashin/web/README.md ryoku/rashin/systemd/ ryoku/rashin/app/rashin-app.desktop
