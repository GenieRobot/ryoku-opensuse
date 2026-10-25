# Ryoku 1.0 retained command and system-bus providers

This #173 dependency slice binds two retained runtime features to the existing
signed Tumbleweed OSS20261016 metadata. It adds runtime Requires for **cava**
and **upower**. Source, UI and features remain intact; no package, service,
endpoint, audio device or VM is installed, started or tested by this review.

[`cava-upower-providers.json`](../upstream/evidence/ryoku-v1.0/cava-upower-providers.json)
contains the complete two selected primary records and literal XML fragments,
repository signature/hash identities, four selected Ryoku source consumers,
and four Quickshell source contracts verified against its exact cached archive.
The compressed and open primary SHA-512s, open size and all 52,976 package
records were checked independently; repomd's signature was reverified with
openSUSE key `AD485664E901B867051AB15F35A2F86E29B700A4`.
No metadata refresh, package/source fetch or dependency rebuild occurred.

| Retained consumer | Exact signed candidate and recipe contract |
|---|---|
| Blobs `CavaFeed` → QStandardPaths::findExecutable("cava") | `cava-1.0.0-1.3.x86_64` owns `/usr/bin/cava` in signed primary file metadata. The selected plugin CMake builds cavafeed.cpp; an absent executable leaves the requested feed without its process. Runtime Requires `cava` supplies this command; ELF analysis cannot infer it. |
| shell Battery.qml → Quickshell.Services.UPower → system-bus `org.freedesktop.UPower` | `upower-1.91.4-1.1.x86_64` is the authenticated UPower service package (primary records include its configuration and `/usr/bin/upower`). Runtime Requires `upower` makes that provider available. Exact installed D-Bus activation payload and live native response still need acceptance; a command file alone is not that proof. |

Quickshell's UPower QML module and the UPower daemon are different contracts.
The pinned Quickshell archive `5e8a7b7a1a1e40de641ee6cfd24f40aa612e13c27353e5746fc79b2861ca9fd0`
contains a default-on SERVICE_UPOWER option, module URI
`Quickshell.Services.UPower` version 0.1, Qt::DBus linkage and system-bus
service initialization. The existing `ryoku-quickshell` spec already declares
that real static QML Provide. Adding `upower` supplies the separate service
package; it does not fabricate a QML provider or certify a native module load.

The runtime RPM audit now requires these two exact package names in addition
to the existing pactl/pw-record providers. Focused tests reject missing and
similarly named devel dependencies, bind the signed records to selected source
consumers, and check the runtime subpackage's Requires rather than a dependency
on another subpackage. Native spec parsing is checked separately from a built
RPM. Package Version/Release policy and Source2/Source3 remain unchanged.

## Power-profile provider choice and protected wiring remain open

Power-profile functionality remains part of the full target. Both the core
shell IPC and Quickshell use `org.freedesktop.UPower.PowerProfiles`; this is
separate from battery status. The pinned signed candidates are
`power-profiles-daemon-0.30-1.6.x86_64` and
`tuned-ppd-2.28.0-1.1.noarch`. Both provide **and conflict with** `ppd-service`;
neither has an Obsoletes entry in the recorded primary records. Forcing one
named implementation can replace a host's existing service choice. This slice
adds neither a named daemon nor a capability Requires until exact endpoint/
activation compatibility is proved. That is an unresolved dependency/wiring
gate, not removal of power-profile status, control or restore.

The core `shell.services.PowerProfiles` path sends socket intents to
`powerprofiles.setProfile`; its Go implementation sets the system-bus profile
and persists power-profile.json. Startup also restores/seeds state and signal/
timer handling can invoke hardware apply. Some iNiR/stage variants use the
Quickshell PowerProfiles API directly. A package dependency or D-Bus/polkit
success does not protect those writes. #181/#25/#176/#20 retain typed-program
identity, explicit authorization, bounded complete capture, linked recovery
and startup/restore policy ownership. This slice implements none of those
operations, changes no service selection and claims no native acceptance.

## Remaining package and native evidence

Quick3D/Concurrent/Multimedia signed package identities, the existing scanner,
Quickshell static imports and source-generated Spatial3D remain in #173's
open closure work. Six cached Qt plugin ELF headers have been inspected without
loading them; their full provider/ABI record is a separate unfinished slice.
Source-generated Spatial3D and rebuilt Quickshell/Blobs still need actual RPM,
plugin/import/ELF loading and fresh native/session/recovery acceptance. Existing
source inventories and hosted Rust CI do not close those gates or any entire
#173 checklist row. All Ryoku features, including core Rashin, remain required.
