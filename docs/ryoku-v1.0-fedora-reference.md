# Pinned Fedora 44/45 RPM reference comparison

This #173 source/spec comparison reads the experimental Fedora edition at
Ryoku `a59d051f0e8080052216f6c7b9eb7e5928935ce8`. Its README targets existing
mutable Fedora 44/45 systems. The full Ryoku 1.0 target, including functional
core Rashin, remains required on Tumbleweed; Fedora mechanisms are reference
material for the owned native implementation, not feature exclusions.

[`fedora-rpm-reference.tsv`](../upstream/evidence/ryoku-v1.0/fedora-rpm-reference.tsv)
records all 54 current specs: exact path/blob/mode, package/source kind, raw
version/license/source/dependency fields, scriptlet names and existing feature
owners. [`fedora-rpm-reference.json`](../upstream/evidence/ryoku-v1.0/fedora-rpm-reference.json)
pins their complete header line/conditional records and scriptlet-region hashes,
plus 37 README/preparation/staging/payload identities. Those include 12 literal
canonical release-payload references followed from the Fedora wrappers. Every
blob was available in the existing cache; lazy fetching was disabled. No
spec/macro, preparation script, payload or package operation was executed.

## Version and source preparation

Twenty-three specs use the shared `ryoku-%{version}.tar.gz`; other specs use
separate/prepared upstream sources. Raw macros and conditionals are retained
without evaluation. A URL or checked-in spec version alone is not evidence of
download verification or compatibility on Tumbleweed.

The shared-tree specs' checked-in `Version: 0.1` is a **preparation placeholder**.
Pinned README/`prepare-srpms.sh` replace Version using `RYOKU_PKGVER` or the
release-version helper, and set Release from the RPM revision. It would be
incorrect to infer the published Fedora version from that placeholder. This
port's independent `0.1.0-8` policy comes from #173; its source remains upstream
1.0.0. No major source/API change is discounted by that package version.

The staging dispatcher sources `.rpm-release` and a package-specific payload,
calls build/package functions and adjusts staged Qt libdir links. Source
preparation can fetch upstream sources and validates its own source manifests;
none of that shell fetching/staging/publishing becomes project-owned port code.
Actual Tumbleweed inputs must retain their own immutable identity, licenses,
hash/signature verification and owned Rust/declarative package path.

## Split/dependency comparison and retained owners

| Fedora boundary | Concrete reference fact / Tumbleweed requirement |
|---|---|
| `ryoku-blobs`, shell, Hub | Native Blobs/Spatial3D, Go IPC/Hub and QML are distinct packages upstream. The port currently groups selected native/QML/private binaries in its runtime, using native Qt6 Concurrent/Quick3D/Multimedia and complete generated module metadata. The signed-snapshot provider/import evidence and actual ELF/native gates remain authoritative. |
| `ryogami` | Declares Qt base/private GUI/declarative/shader/multimedia, Vulkan/Wayland/FFmpeg build boundaries and ImageMagick/video dependencies. Fedora's ffmpeg-free/openh264 workaround and skwd-paper-bin are not guessed SUSE equivalents. Keep daemon/editor/livewall/tool behavior under #25/#175/#176 and native codec tests. |
| Hyprland/Niri variants | Build their Go WM adapters and stage compositor config/portal/helper paths. The canonical Hyprland recipe installs the helper scripts the audited provider expects; current port helper gaps need #25/#180 typed implementations. Full Niri #46 and Wayfire #185 targets remain retained. |
| Core `ryoku-rashin` / Prowl | Declares Go, GTK3/WebKitGTK, UV/Node/Python/Prowl requirements. Canonical payload builds the Go backend, generates repository context with repo-index, builds the C++ GTK window and installs backend/alias, context, skills/wiki, desktop/icon, four collectors and four units. Every capability remains required under #183; this is not a reduced chat-only target. Native/frontend/agent/privacy/recovery acceptance remains open. |
| Host lifecycle, guard/greeter and Hub controls | Source package splits do not authorize edition host helpers, system settings or login policy. Use #25/#177/#180/#181 and #27–29/#31/#32 with the existing typed authorization/linked recovery contracts. Preserve ordinary login, autologin policy and KDE fallback. |
| Plugins/content/apps/fonts/cursors | Raw versions, source and license labels are recorded per spec. #182/#47 and #180 own retained discovery/install/update/removal/appearance, using #27/#28 verified sources and package plans. A Fedora License field does not resolve the eleven current shell-font redistribution blockers. |
| Hardware, DKMS, firmware and service packages | Preserve corresponding device features through #181/#27/#28/#31/#32; actual Tumbleweed package/kernel/driver policy and native hardware behavior must be proven. The reference does not select Fedora packages, COPRs or root scripts. |

The core Rashin CMake requires `pkgconfig(gtk+-3.0)` and
`pkgconfig(webkit2gtk-4.1)`. The existing authenticated Tumbleweed 20261016 index
provides `gtk3-devel` 3.24.52+git59.b30343717d-1.1 and `webkitgtk3-devel`
2.54.1-3.1, both x86_64. Exact package checksums, locations, provided
capabilities/dependencies, signed-primary provenance and pinned CMake lines are
in [`rashin-native-provider-records.json`](../upstream/evidence/ryoku-v1.0/rashin-native-provider-records.json).
These are metadata providers, not downloaded/installed packages or proof of
native app linking, browser/agent behavior, solver compatibility or functional
Rashin. No Rashin build/runtime requirement is added by this reference slice.

## Five explicit scriptlet boundaries

The spec regions are hashed/pinned; only their source effects are classified.

| Spec | Source effects / existing Tumbleweed owner |
|---|---|
| `ryoku-desktop` | Sleep-inhibiting package cutover, migration marker, service reconciliation, Bluetooth/regdom tuning, GRUB/snapshot menu, Plymouth/dracut work, polkit reload and logind cleanup. These are privileged host/boot/lifecycle mechanisms requiring #27/#28/#31/#32/#177 and recovery owners; no scriptlet is imported. |
| `ryoku-rashin` | Host service reconciliation after install and global user-unit/timer disable on removal. #183 must preserve startup/collector/privacy behavior through its reviewed account/service contract; global enable/disable is not per-user authorization. |
| `asusctl` | systemd service lifecycle/restart macros and udev reload/trigger. #181/#31 and native package/hardware policy. |
| `ryotunes` | User socket/service lifecycle/restart macros. #182/#47 with explicit startup/data ownership. |
| `xpadneo-dkms` | Old-version DKMS removal, module add/build/install and HID udev actions. #27/#28/#31/#32 must own native kernel/package/security policy and recovery. |

This source reference unit covers all current spec identities/headers, package
split/dependency boundaries and the five direct scriptlet-bearing specs.
Deterministic checks passed all 54 spec and 37 supporting blob/mode/digest
records, header/conditional lines, scriptlet hashes, 12 canonical references,
two exact signed-cache CMake providers and local document links. Input digests
and scope are in
[`fedora-reference-validation.json`](../upstream/evidence/ryoku-v1.0/fedora-reference-validation.json).
It does not certify every transitive payload helper, external download,
license, provider operation or native package. Complete #173 semantic/native
and all owning functional acceptance gates remain open; #173/PR #174 remain
open/draft. No network source retrieval, VM, package install or build was used.
