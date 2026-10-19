# Preserved Ryoku desktop runtime

The disposable runtime payload uses the actual shell and Settings QML at the
immutable revision in `upstream/ryoku.source.toml`. It includes the complete
`shell/quickshell` tree, Hub QML, `Ryoku.Ui`, `Ryoku.FrameBars`,
`Ryoku.PluginKit`, native `Ryoku.Blobs` source, translations, and upstream
LICENSE/NOTICE. Export mappings and immutable font sources, licenses and hashes
are recorded in [`upstream/ryoku-runtime.toml`](../upstream/ryoku-runtime.toml).
The original runtime archive retains the pinned bytes. The current desktop RPM
ships the pinned QML except for the reviewed Capture.qml routing patch and the
matching unchanged upstream Go daemon from the separately pinned,
vendored source archive in [`upstream/ryoku-shell-ipc.toml`](../upstream/ryoku-shell-ipc.toml).
The default RPM applies the routing-only Capture.qml patch recorded in
[`upstream/patches/0002-rust-screenshot-adapter.toml`](../upstream/patches/0002-rust-screenshot-adapter.toml);
capture execution and clipboard result handling are provided by the Rust
`ryoku-capture` adapter.
The pinned upstream `ryoku-wm-hyprland` provider is now a separate RPM build
input. The RPM spec is wired to build and install its unchanged binary on the
fixed session-service `PATH`, and to include the provider's original
`Ryoku.Wm.Hyprland` QML bridge module. The existing upstream daemon can then
publish its `wm` state topic to the already packaged `Ryoku.Ui/Singletons/Wm.qml`.
The daemon dispatches only neutral `wm.act` requests; it does not call the
provider's separate config-writing `apply` or `outputs` verbs. RPM build and
native package-owned workspace/overview acceptance remain pending.
The optional Rust clipboard-provider patch is not applied by the default RPM;
the original QML clipboard surface stays connected to its upstream provider.

The initial manual native proof used `/usr/local/ryoku-quickshell`. The
experimental RPMs use `/usr/libexec/ryoku/quickshell`, leaving `/usr/local`
for local administration and distribution `/usr/bin/qs` independent. The
build and ownership definitions are in
[`packaging/rpm/desktop-build.md`](../packaging/rpm/desktop-build.md).
The runtime build commands below use the current fixed packaged prefix, also
used by the later manual service-provider proof. The font commands retain the
original disposable user's fixture procedure; RPM builds install fonts in a
dedicated system directory. Rust tool packages are described in
[`packaging/rpm/tools-build.md`](../packaging/rpm/tools-build.md).

This is a desktop port experiment. ADR 0001's existing prohibition on importing
Ryostore and its unresolved visible mutation controls still apply. The payload
does not include Ryostore, an Arch package backend, or upstream privileged
helpers.
The real shell has default bar/layout settings and enumerates screens through
Quickshell, so loading it does not require inventing daemon responses.

## Native dependencies

The following names were checked in the exact signed-fixture OSS metadata for
snapshot `20261013`; the metadata digests are in the manifest. Install them in
the disposable guest using its existing pinned repositories:

```console
sudo zypper --non-interactive --no-refresh install --no-recommends \
  cmake ninja gcc-c++ qt6-base-devel qt6-declarative-devel \
  qt6-shadertools-devel qt6-multimedia-devel qt6-declarative-imports \
  qt6-multimedia-imports qt6-qt5compat-imports kf6-syntax-highlighting \
  google-noto-sans-cjk-fonts
```

The initial native launch **rejected `noctalia-qs 0.0.12-2.5`**: it exits at the
preserved shell's `DefaultEnv QSG_RENDER_LOOP=threaded` pragma. Package provides
and module presence did not establish API compatibility. The pinned Ryoku
installer also classifies Noctalia as a rival stack; its package recipe requires
generic Quickshell without recording a compatible version.

Use unchanged official upstream Quickshell **0.3.2**, revision
[`4f508be500dea6e5732cc3d50382a0048b17e7b1`](https://github.com/quickshell-mirror/quickshell/tree/4f508be500dea6e5732cc3d50382a0048b17e7b1).
The repository README identifies the GitHub mirror and canonical
`git.outfoxxed.me` repository. Its
[`launch.cpp`](https://github.com/quickshell-mirror/quickshell/blob/4f508be500dea6e5732cc3d50382a0048b17e7b1/src/launch/launch.cpp)
handles all retained `DefaultEnv` forms, `UseQApplication`, and
`QS_DROP_EXPENSIVE_FONTS`; it applies environment values before constructing Qt.
The latter rejects WOFF/WOFF2 fonts, while the retained fonts use TTF.

All eleven imported Quickshell modules exist with their default features
enabled. Critical checked APIs include WindowManager windowsets, ToplevelManager
toplevels, DesktopEntries lookup and application models, ProcessContext detached
launch with environment/working directory, Networking, Bluetooth, Pipewire,
Mpris, UPower, WlrLayershell properties, FileView adapters/atomic writes and
Socket parsing/flush. This source audit does not substitute for native loading.

The Qt imports include QtQuick Controls, Dialogs, Effects, Layouts and Shapes,
QtQml Models, QtMultimedia, Qt5Compat GraphicalEffects, folderlistmodel and
`org.kde.syntaxhighlighting`. Most runtime packages are already KDE hard
dependencies. ShaderTools development metadata supplies `cmake(Qt6ShaderTools)`
and requires the native `qsb` tool. The unchanged plugin needs public Quick,
ShaderTools and Multimedia development APIs; it does not request private Qt
headers.

Build the replacement runtime against the guest's exact Qt 6.11.2. Additional
package providers were checked in the same `20261013` metadata; the private SDK
uses the following actual split names:

```console
sudo zypper --non-interactive --no-refresh install --no-recommends \
  pkgconf cli11-devel cpptrace-devel jemalloc-devel libdrm-devel libgbm-devel \
  Mesa-libEGL-devel vulkan-headers wayland-devel wayland-protocols-devel \
  libxcb-devel pipewire-devel pam-devel polkit-devel glib2-devel \
  qt6-quick-private-devel qt6-waylandclient-private-devel qt6-gui-private-devel \
  spirv-tools-devel
```

The native `cpptrace 1.0.4` recipe explicitly enables libunwind, required by the
upstream crash handler's signal-safe-unwind configure check. Keep that check and
the default UI features enabled. No network FetchContent build is required.

The source archive retains upstream LICENSE and source unchanged. Its SHA-256 is
`5e8a7b7a1a1e40de641ee6cfd24f40aa612e13c27353e5746fc79b2861ca9fd0`.
After verifying and extracting it under `/tmp`:

```console
cmake -S /tmp/quickshell-4f508be500dea6e5732cc3d50382a0048b17e7b1 \
  -B /tmp/ryoku-quickshell-build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DGIT_REVISION=4f508be500dea6e5732cc3d50382a0048b17e7b1 \
  -DDISTRIBUTOR=Ryoku-openSUSE-disposable \
  -DCMAKE_INSTALL_PREFIX=/usr/libexec/ryoku/quickshell
cmake --build /tmp/ryoku-quickshell-build --parallel 2
sudo cmake --install /tmp/ryoku-quickshell-build
ldd /usr/libexec/ryoku/quickshell/bin/quickshell
/usr/libexec/ryoku/quickshell/bin/qs --version
```

The build installs `bin/quickshell` and the upstream `bin/qs` symlink beneath
that private prefix. The Rust session launcher must use the fixed
`/usr/libexec/ryoku/quickshell/bin/qs` path. The first native shell proof used
`/usr/local/ryoku-quickshell`; the later native service-provider proof rebuilt
the same source with the intended packaged prefix shown here. Distro-owned `/usr/bin/qs` can remain
installed and is not selected by the port launcher.

## Payload and native build

Use `/usr/share/ryoku-opensuse/upstream` as the root-owned payload directory.
The archive has relative paths and a `SHA256SUMS` inventory. Verify that inventory
after extraction, before executing a source build. The initial archive contains
plugin source; `qml/Ryoku/Blobs` is supplied by this native guest build:

```console
cd /usr/share/ryoku-opensuse/upstream
sha256sum --check SHA256SUMS
cmake -S native/ryoku-blobs -B /tmp/ryoku-blobs-build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build /tmp/ryoku-blobs-build --parallel 2
sudo cp -a /tmp/ryoku-blobs-build/qml/Ryoku/Blobs qml/Ryoku/
sudo chown -R root:root qml/Ryoku/Blobs
ldd qml/Ryoku/Blobs/libryoku-blobs.so
ldd qml/Ryoku/Blobs/libryoku-blobsplugin.so
```

No native source modifications or project-authored FFI are needed. The
[native runtime evidence](../infra/tumbleweed-lab/evidence/issue14-real-ryoku-runtime.toml)
records the actual completed plugin/runtime builds, resolved loader dependencies,
font checks and visible shell/application launch. These commands remain a
procedure rather than a substitute for collecting evidence on a new guest.

As the ordinary desktop user, retain the font directories with their licenses
under the standard user font location and refresh that user's cache:

```console
mkdir -p "$HOME/.local/share/fonts/ryoku-upstream"
cp -a /usr/share/ryoku-opensuse/upstream/fonts/. "$HOME/.local/share/fonts/ryoku-upstream/"
fc-cache -f "$HOME/.local/share/fonts/ryoku-upstream"
fc-match -f '%{family}\n' 'Material Symbols Rounded'
fc-match -f '%{family}\n' 'Space Grotesk'
fc-match -f '%{family}\n' 'SpaceMono Nerd Font'
fc-match -f '%{family}\n' 'Fraunces'
```

The payload retains the complete variable Material Symbols Rounded font, Space
Grotesk variable weights, both Fraunces variable upright/italic fonts, and all
four Space Mono and SpaceMono Nerd Font styles. Google/fonts and official
Material Design Icons sources are pinned; the official Nerd Fonts v3.4.0 source
supplies the exact `SpaceMono Nerd Font` family requested by the untouched UI.
Native `fc-scan` verified those family names before packaging. Guest cache and
rendering checks remain necessary.

## Real surface launch and bounded scope

From an authenticated ordinary-user Wayland session, the Rust `ryoku-session`
launcher sets the two QML import paths to `qml` and `shell/quickshell`,
`RYOKU_SHELL_DIR` to `shell`, and `RYOKU_I18N_DIR` to `i18n/catalog` under the
payload root. It launches the private-prefix native `qs -p shell/quickshell/shell` entrypoint
through `setpriv --no-new-privs`; Hub uses `qs -p hub/quickshell`.

```console
ryoku-session shell
ryoku-session open launcher
ryoku-session hub
```

The `open` command uses the original shell's `IpcHandler` and its existing
`openSurface` method. This can show the preserved launcher even when the absent
daemon has not advertised global shortcut capabilities. No capability or
backend result is fabricated.

The ordinary launcher app provider enumerates Quickshell's native
`DesktopEntries`, performs local fuzzy matching, and starts a GUI entry's actual
argv through the original `AppLaunch`/`Spawn` code. Searching for and launching
an installed ordinary GUI application is therefore a useful native interaction
check. `Terminal=true` applications still require the unported `ryoku-app`
wrapper. The icon index, wallpaper topic, package/update information, power,
lock, compositor state and many settings actions need the absent daemon or
unported helpers; their controls are not accepted port functionality.

Do not run upstream `ryoku-shell.service`, `ryoku-bootstrap.service`, deployment
scripts, or the Arch materializer to compensate for missing integration. The
Rust session work owns native launch/session configuration. NoNewPrivileges
blocks set-ID/file-capability escalation; it does not sandbox home writes or
ordinary desktop D-Bus APIs.

Wait for KDE's splash to exit and inspect actual pixels before claiming visible
success. Mapped/painted markers alone have previously coincided with an
obscuring KSplash window. Capture the real Ryoku bar, launcher and a launched GUI
application, with native process identities and private logs. Visible KDE shell,
application launch and the [typed service-provider read path](launcher-service-provider.md)
now have reviewed native evidence. Nested or native Hyprland behavior, full
helper compatibility, RPM installation,
accessibility and issue #14 acceptance remain separate checks.

The original 25.5 MB preserved-source/font archive is unchanged and its hash
inventory remains valid. Its bundled `RUNTIME.md` described the initial runtime
candidate; this repository document and manifest supersede that Noctalia choice.
