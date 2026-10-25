Name:           ryoku-session-clipboard-keeper
Version:        0.5.0
Release:        1
Summary:        Private Rust clipboard selection keeper for the Ryoku session
License:        MIT AND Unicode-3.0
URL:            https://github.com/Linus789/wl-clip-persist
Source0:        ryoku-session-clipboard-keeper-%{version}.tar.xz
Source1:        ryoku-session-clipboard-vendor-%{version}.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo >= 1.99
BuildRequires:  rust >= 1.99
BuildRequires:  gcc

%description
Private build of the pinned upstream Rust Wayland clipboard keeper. The
session invokes only the regular clipboard with a bounded selection size and
without timestamp manipulation. No unit, autostart, shell command or clipboard
history service is installed. Native clipboard acceptance is tracked in #25.
The refreshed locked dependency closure and reviewed Rust parser/I/O port are
recorded in upstream/session-dependencies.toml.

%prep
printf 'e0fe47bc936c49bcc45e579225cbf2538f3dd387083e631622c744fc98cf0c21  %%s\n' '%{SOURCE0}' | sha256sum -c -
printf '72a89a10effa498bee1c51fa1f7e8da7b8b89009751bc81f8821ac8226c5ebb3  %%s\n' '%{SOURCE1}' | sha256sum -c -
%autosetup -n wl-clip-persist-%{version} -a 1

%build
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    build --offline --locked --release -p wl-clip-persist

%check
CARGO_NET_OFFLINE=true cargo \
    --config 'source.crates-io.replace-with="vendored-sources"' \
    --config 'source.vendored-sources.directory="vendor"' \
    test --offline --locked --workspace

%install
install -D -m 0755 "${CARGO_TARGET_DIR:-target}/release/wl-clip-persist" \
    %{buildroot}%{_libexecdir}/ryoku/session/upstream/wl-clip-persist
install -D -m 0644 port/wayrs-proto-parser/LICENSE licenses/wayrs-proto-parser/LICENSE
find vendor -type f \( -iname 'LICENSE*' -o -iname 'COPYING*' -o -iname 'NOTICE*' -o -iname 'UNLICENSE*' \) -print | while IFS= read -r file; do
    install -D -m 0644 "$file" "licenses/$file"
done

%files
%defattr(-,root,root)
%dir %{_libexecdir}/ryoku
%dir %{_libexecdir}/ryoku/session
%dir %{_libexecdir}/ryoku/session/upstream
%{_libexecdir}/ryoku/session/upstream/wl-clip-persist
%license LICENSE licenses/
%doc README.md
