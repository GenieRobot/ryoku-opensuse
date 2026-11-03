## Layout

| Path | Files | Purpose |
|---|---|---|
| `ryoku/` | 7886 | the desktop: apps, hyprland (Lua), shell UI, hub, rashin, assets |
| `tests/` | 154 | standalone CI check scripts |
| `system/` | 113 | machine definition: boot chain, hardware policy, package sets |
| `installation/` | 107 | how a machine is built: TUI, backend installer, ISO |
| `release/` | 101 | packaging: PKGBUILDs, the [ryoku] repo, keyring |
| `docs/` | 46 | the guides |
| `ryoku-shell-installer/` | 36 | no-ISO converter for existing Arch installs |
| `bin/` | 33 | repo tooling and CI checks |
| `.githooks/` | 4 | commit and push gates |

## Key entry points

| Concern | Path |
|---|---|
| Hyprland config entry (Lua) | `ryoku/hyprland/hyprland.lua` |
| Shell daemon (Go) | `ryoku/shell/ipc/` |
| Settings app backend | `ryoku/hub/backend/` |
| Settings app UI (QML) | `ryoku/hub/quickshell/` |
| User CLI | `ryoku/cli/` |
| Rashin (this system) | `ryoku/rashin/` |
| Dev deploy loop | `ryoku/shell/deploy.sh` |
| Installer backend | `installation/backend/` |
| Package builds | `release/packages/` |

## Docs

- `docs/adding-a-window-manager.md` Adding a window manager
- `docs/bar.md` Frame bars
- `docs/barstyles.md` Bar styles
- `docs/cli.md` The `ryoku` command
- `docs/compositors.md` Compositors
- `docs/config-import.md` Config import
- `docs/conventions.md` Conventions
- `docs/development.md` Development
- `docs/dictation.md` Dictation
- `docs/frame.md` The frame
- `docs/hyprland-plugins.md` Hyprland compositor plugins
- `docs/i18n.md` Translation
- `docs/install-without-iso.md` Install Ryoku without its ISO
- `docs/installation-hardware.md` Installing on real hardware
- `docs/kernels.md` Kernels and CachyOS gaming
- `docs/launcher.md` The launcher
- `docs/nomarchy.md` Nomarchy
- `docs/performance.md` Performance
- `docs/plugins.md` Writing a Ryoku shell plugin
- `docs/power.md` Power, heat, and battery
- `docs/preinstalled.md` What ships preinstalled
- `docs/profile.md` The Profile plate
- `docs/rashin-terminal.md` Rashin in the terminal
- `docs/rashin.md` Ryoku Rashin
- `docs/roadmap.md` Ryoku Roadmap
- `docs/ryogami.md` Ryogami
- `docs/ryoku.md` What Ryoku is
- `docs/sidebars.md` Controls, ryoshot, and Ask
- `docs/stage.md` Ryostage (舞台)
- `docs/store.md` The store
- `docs/structure.md` Repository structure
- `docs/theming.md` Theming: how colour reaches an app
- `docs/ui-ux.md` UI and UX
- `docs/updates.md` Updates and delivery
