# Project scope and provenance

This repository implements the approved v1.2 plan in
[roadmap issue #1](https://github.com/GenieRobot/ryoku-opensuse/issues/1): an
RPM-based Ryoku desktop for openSUSE Tumbleweed x86_64, initially using
Hyprland with KDE fallback. The port preserves the upstream experience while
isolating distribution-specific behavior in small, documented adapters.

## Upstream attribution

The primary upstream project is [Ryoku](https://github.com/Ryoku-dev/ryoku),
an Arch Linux distribution. Its project documentation identifies GPL version 3
as its license. This repository preserves the upstream GPL license text and
requires preservation of applicable upstream file notices and licenses when
components are imported. The baseline revision, provenance, component
boundaries, and any port patches are tracked in [M0-02, issue
#11](https://github.com/GenieRobot/ryoku-opensuse/issues/11); no floating
upstream branch is treated as a reproducible source pin.

## Supported scope

- Tumbleweed x86_64, with Hyprland as the initial desktop and KDE as fallback.
- Native Ryoku controls by default; Myrlyn is optional. Cockpit and individually
  compatible YaST modules are independent optional administration choices.
- Every supported integrated change requires a verified linked pre-change
  restore point containing matching Snapper system state and complete declared
  Ryoku, configuration, and profile state. Capture failure prevents mutation.
- Root rollback must preserve the catalog, configuration generations, and
  recovery journal. Recovery preserves unrelated files in home directories.
- UI, service, helper, providers, tests, and custom automation authored here are
  Rust. Upstream components retain their existing languages. RPM and CI files
  contain declarative metadata and commands only.
- A supported full Tumbleweed update uses a reviewed compatible system
  transaction; no partial Ryoku-only update is exposed.
- Boot recovery and an independent authenticated login recovery session are
  release requirements. Lock-screen recovery is an authenticated handoff.
- A generic port must work without a private companion or network during
  recovery. Private companion data is not stored in this public repository.

## Out of scope

Ruby bindings, niri, additional applications, broader administration, an ISO,
and physical-machine deployment are deferred as recorded in issue #1. Work is
developed and tested in cloud resources; desktop, boot, and recovery claims
require disposable Tumbleweed VMs. A container or local build does not prove
those release gates.
