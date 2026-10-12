# Project scope and provenance

This repository is being developed toward the approved v1.2 plan in
[roadmap issue #1](https://github.com/GenieRobot/ryoku-opensuse/issues/1): an
RPM-based Ryoku desktop for openSUSE Tumbleweed x86_64, initially using
Hyprland with KDE fallback. The port preserves the upstream experience while
isolating distribution-specific behavior in small, documented adapters.

This issue establishes repository conventions and empty crate boundaries. The
product capabilities below are target requirements; they are not implemented
or verified by this foundation commit.

## Upstream attribution

The primary upstream project is [Ryoku](https://github.com/Ryoku-dev/ryoku),
an Arch Linux distribution. Its project documentation identifies GPL version 3
as its license. This repository preserves the upstream GPL license text and
requires preservation of applicable upstream file notices and licenses when
components are imported. The baseline revision, provenance, component
boundaries, and any port patches are tracked in [M0-02, issue
#11](https://github.com/GenieRobot/ryoku-opensuse/issues/11); no floating
upstream branch is treated as a reproducible source pin.

## Approved target scope and release requirements

- Target Tumbleweed x86_64, with Hyprland as the initial desktop and KDE as
  fallback.
- The product should provide native Ryoku controls by default; Myrlyn is
  optional. Cockpit and individually compatible YaST modules are independent
  optional administration choices.
- Before every supported integrated change, implementation must verify a
  linked pre-change restore point containing matching Snapper system state and
  complete declared Ryoku, configuration, and profile state. Capture failure
  must prevent mutation.
- Recovery must keep the catalog, configuration generations, and recovery
  journal available after root rollback, and preserve unrelated home files.
- All new project-owned executable code must be Rust. Retained upstream
  components keep their existing languages. RPM and CI files must contain
  declarative metadata and commands only.
- A supported full Tumbleweed update must use a reviewed compatible system
  transaction; implementation must not expose partial Ryoku-only updates.
- Boot recovery and an independent authenticated login recovery session are
  release requirements. Lock-screen recovery must use an authenticated handoff.
- The generic port must work without a private companion or network during
  recovery. Private companion data must not be stored in this public repository.

## Out of scope

Ruby bindings, niri, additional applications, broader administration, an ISO,
and physical-machine deployment are deferred as recorded in issue #1. Work is
developed and tested in cloud resources; desktop, boot, and recovery claims
require disposable Tumbleweed VMs. A container or local build does not prove
those release gates.
