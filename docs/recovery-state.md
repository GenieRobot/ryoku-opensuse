# Managed state and profile recovery contract

Status: design contract for issue [#16](https://github.com/GenieRobot/ryoku-opensuse/issues/16),
with partial recovery implementation and native evidence below. Dedicated-state
initialization and reboot persistence have been exercised on Tumbleweed; complete
linked restore and an independent boot recovery path remain unfinished. Progress
notes below retain the scope of each historical build rather than attributing
later changes to earlier VM results.

The storage and profile decisions are recorded in
[ADR 0002](adr/0002-linked-recovery-storage.md). The repository now includes
state-mount validation, generation/catalog storage, a native initializer path,
and recovery CLI components. Installed startup integration and live linked
rollback proof remain outstanding.

## Decision: linked generations outside root rollback

The durable catalog, operation journal, and immutable Ryoku generation store
belong on a dedicated Btrfs subvolume mounted at `/var/lib/ryoku-state`. The
subvolume must be a direct child of the Tumbleweed filesystem's top-level
subvolume and must not be nested beneath the Snapper-managed root subvolume.
The root Snapper configuration must exclude this mount from its snapshot
content. Installation and every startup must verify filesystem UUID, Btrfs
subvolume identity, kernel-reported source, required mount security options,
ownership, mode, and that the mount is active before permitting a managed write. Missing, substituted,
read-only, or incorrectly nested state storage is a hard stop for mutations;
software must never silently fall back to a root directory or another disk.

Each managed transaction first freezes the declared file set, records a
generation, and asks Snapper to create the system pre-change snapshot. The
catalog entry binds the transaction ID, generation ID, Snapper configuration
and snapshot ID, and content digest. Only after the complete file inventory
and digest have been read back and the linkage is durable may the operation
mutate the system. Root rollback can then restore `/etc` and system files while
the catalog and immutable generation remain addressable. Recovery resolves
the catalog link and either restores that exact generation or reports a typed
unavailable/inconsistent state. It must not infer a generation from timestamps
or select the newest one as a fallback.

The unprivileged service owns typed request validation and per-user durable
operation status. The installed helper is a narrow authenticated, deny-only
boundary: it supports availability and linked-checkpoint verification while
provider mutations remain disabled. A separate root-only offline recovery CLI
uses fixed paths, the protected state mount, and the shared mutation lock for
the currently implemented checkpoint/preparation/restore lanes. It accepts
validated identifiers rather than caller-supplied paths or shell text. These
components do not yet provide a single installed helper coordinator for
capture, provider mutation, and restore; managed provider writes remain
disabled.

This design preserves Ryoku state across a root rollback; it does not
promise to preserve the state subvolume across disk loss, repartitioning,
filesystem replacement, or a malicious root administrator. Backups to another
device are outside this contract. Native Btrfs/Snapper restore compatibility,
early mount ordering, and failure behavior must be demonstrated in the
disposable VM before implementation can claim recovery support.

### Storage layout and transaction ordering

The proposed state subvolume is mounted at `/var/lib/ryoku-state`, owned by
`root:root`, mode `0700`. Its direct parent is the Btrfs top-level subvolume;
it is not a child of the Snapper root subvolume. The root Snapper configuration
must exclude the mounted state path. The implementation must resolve the
mountpoint without following a substituted path, then compare the observed
filesystem UUID and subvolume ID/parent relationship with the installed
machine record on every startup and before each write. The Btrfs UUID is read
from the pinned mounted descriptor, binding the reported filesystem source to
the recorded identity without depending on an unstable device-node spelling.
The collector also requires the packaged mount policy's `nodev`, `nosuid`,
and `noexec` flags and exactly one `subvolid` matching the recorded subvolume;
contradictory `dev`, `suid`, or `exec` flags fail closed. Any mismatch blocks mutation and remains visible as a typed
recovery-unavailable result.

The Linux recovery crate now provides a fail-closed live collector in
`state_mount`. It opens the fixed relative path beneath a trusted `/`
descriptor without following symlinks, pins the directory, checks root:root
mode `0700`, and matches its kernel mount ID to the exact `/proc/self/mountinfo`
entry. Mountinfo parsing is bounded to 4 MiB and checks the effective
mountpoint, filesystem type, and read-only option, and decodes the mount root.
For Btrfs, the mountinfo root may be the subvolume's filesystem-relative path,
so requiring that field to equal `/` would reject normal subvolume mounts. The
collector instead requires the pinned directory's Btrfs root inode number
`256`, which rejects a bind mount of an ordinary directory inside the
subvolume while allowing the subvolume root itself. Btrfs filesystem UUID and
subvolume ID/parent ID come from the read-only `BTRFS_IOC_FS_INFO` and
`BTRFS_IOC_GET_SUBVOL_INFO` calls on that pinned descriptor. The subvolume
parent ioctl works through the separate mountpoint, so that mount does not
hide parent identity. Any missing entry, unsupported ioctl, malformed or
oversized mount table, wrong parent, wrong identity, or unsafe descriptor is a
typed hard stop. The collector is used by the recovery CLI, explicit state
initialization, and the privileged helper's read-only generation reader; it is
not wired into package installation or enabled provider mutations.
`GenerationStore::open_state_mount` calls the collector before
opening/creating store directories, so an arbitrary directory descriptor
cannot initialize the production store. The recovery crate now has
a strict schema-v1 reader and exclusive provisioner for
`/etc/ryoku/state-machine.json`, stored on the root filesystem outside the
state mount. Provisioning accepts facts from the trusted subvolume-creation
transaction; startup reads that independent record and passes its identity to
`GenerationStore::open_state_mount`. The reader rejects noncanonical UUIDs,
unsupported/unknown/duplicate fields, non-top-level parents, oversized or
noncanonical JSON, symlinked paths, and files without root:root mode `0600`.
The privileged helper now opens `GenerationReader::open_state_mount` before
entering its request loop. A missing or corrupt record, absent or mismatched
mount, or unsafe store prevents the helper from serving requests; the validated
read-only generation descriptor stays pinned for the helper lifetime. Mutating
helper actions remain rejected; the root-only recovery CLI separately uses the
validated writable state store for offline recovery commands.
The helper's read-only linked-checkpoint request reopens a selected generation
and its original Snapper partner; it does not make either available for restore.
Package-install provisioning is not wired; the explicit root-only
`ryoku-state-init` command creates the machine record after verifying the
newly created subvolume and mount identity. During that initial transaction it
also creates the empty immutable-profile bundle store inside the verified
state subvolume, before publishing the machine record. The state-creation lock
and bootstrap root checkpoint are revalidated around this operation. An
interrupted resume accepts only the exact empty `profiles/.staging` layout;
partial or populated storage remains a reconciliation case. The `initialize`
fast path does not upgrade older initialized stores.
The collector now validates the observed source is present, binds its live
filesystem UUID through the Btrfs ioctl to the machine record, and checks the
required per-mount security flags plus the exact `subvolid` before opening the
store. It intentionally does not persist a raw device-node path, which may
change across boots. The exact mountinfo representation and systemd-mounted
behavior still need Tumbleweed VM proof.

The intended on-volume layout is:

```text
/var/lib/ryoku-state/          root:root 0700, dedicated Btrfs subvolume
  catalog/                     rebuildable index of verified manifest IDs
  generations/<generation-id>/ immutable manifest + declared payload blobs
  restore-journals/<id>.json   per-attempt synchronized recovery phases
  restore-journals/.restore-journal.lock
  mutation.lock                shared Ryoku session/recovery serialization
  snapper-catalog/catalog.json retained Snapper transaction/retention index
  staging/<transaction-id>/    private incomplete publication, never a restore source
  profiles/<id>/               root-owned profile bundles and private staging

/etc/ryoku/state-machine.json  root:root 0600, expected identity installed
                               by explicit state initialization after verified
                               subvolume creation; package install does not run it
~/.local/state/ryoku/service/  per-user durable service operation history
  operations.log               survives daemon/client restarts; separate from
                               the protected machine recovery journal
```

A protected mutation holds `mutation.lock` from inventory freeze through
publication and apply.
The required provider transaction protocol validates that every requested
record belongs to an approved path set, captures declared entries without
following symlinks, creates the Snapper pre-change snapshot, then writes the
manifest and content into a private staging generation. Every payload and
directory is synchronized, read back, and checked against the manifest digest
and snapshot/filesystem link. The per-user operation journal records durable
progress, while the protected restore journal atomically persists each recovery
phase. Only after the generation and catalog link are verifiable may a provider
mutation begin. A crash before publication leaves staging ignored. A crash
after publication but before provider mutation leaves a recoverable, unused
linked generation. Reusing a transaction ID is idempotent; a collision with
different content is a hard error. This provider transaction path remains
disabled pending the authenticated helper and provider integration gates.

After root rollback, recovery reads the catalog and journal from the separately
mounted state subvolume, matches the observed root filesystem and Snapper
snapshot ID to one exact generation link, verifies all declared content and
metadata, and restores only those paths. It never guesses from timestamps,
chooses the newest generation, follows an unlisted path, or deletes a path
without a matching verified absent record. If the mount, catalog, link, or
payload fails validation, recovery refuses restore and preserves the evidence.
A configuration-only restore is a distinct labeled action and is allowed only
when its generation is compatible and fully verified; it does not claim system
rollback. Full provider-transaction coordination and manifest-backed linked
console recovery still require end-to-end VM proof.

## Managed state inventory

Only paths explicitly present in a versioned manifest may be read or changed.
No recursive walk of `/`, `/etc`, or `$HOME` is permitted. Each path is
relative to a fixed declared root; reject absolute paths, `..`, empty path
components, duplicate normalized paths, device nodes, sockets, FIFOs, and
symlink traversal. A symlink may be recorded as a symlink only when its raw
target is captured as metadata; do not follow it during inventory, hashing,
copy, or restore. Extended attributes are allowlisted by namespace and name;
security labels and ACLs are recorded where supported, and unsupported
metadata makes capture incomplete rather than silently dropping it.

| Class | Declared roots / examples | Owner and backup rule |
|---|---|---|
| Catalog and recovery journals | `/var/lib/ryoku-state/catalog/`, `/var/lib/ryoku-state/snapper-catalog/`, `/var/lib/ryoku-state/restore-journals/`, `/var/lib/ryoku-state/mutation.lock` | Root-owned writes; outside root Snapper rollback; include in a separate durable state backup policy. The generation link catalog, Snapper retention catalog, per-attempt restore journals, and common mutation lock are distinct filesets. The user service's `operations.log` is separately rooted under that user's private state directory. |
| Immutable configuration generations | `/var/lib/ryoku-state/generations/<id>/` | Root-owned, content-addressed, read-only after publish; includes only paths explicitly declared by a generation manifest. |
| Proposed Ryoku system configuration | `/etc/ryoku/` is an openSUSE namespace proposal; provider files below `/etc` are not yet enumerated | No Tumbleweed provider path is currently authorized for recovery. Do not interpret this proposed namespace as a discovered or audited installed path. |
| User Ryoku configuration | `$XDG_CONFIG_HOME/ryoku/` (or the resolved per-user config root) | User-owned; include only declared Ryoku files, with UID/GID and mode. Do not include sibling or unrelated home files. |
| Persistent session preferences | `$XDG_CONFIG_HOME/ryoku/{shell,matugen,theme,brand,launcher,widgets,performance,recording,visualizer,stage,camera,ryoshot,ryoshot-beautify,equalizer,power,power-profile,keypresses}.json` plus exact `$XDG_STATE_HOME/ryoku/{flags,capture,record}.json`, `nvibrant-value`, `backlight`, `touchpad.disabled`, and `stage-walls.json` paths and the sibling `ryoku-nightlight`, `ryoku-nightlight-enabled`, and `ryoku-nightlight-schedule.json` files, each anchored under the resolved account roots | The pinned shell and Hub QML writers persist appearance, theme source, brand, launcher, widget, performance, recording, visualizer, Stage, camera, Ryoshot drawing/editor, equalizer, keypress-overlay, and power-profile preferences in these exact JSON files; `ryoshot.json` contains brush styles and save settings, while the keypress file stores only theme, display mode and optional placement, not raw key events or typed text. `Capture.qml` persists screenshot delay/save/beautify choices in `capture.json`; `Recorder.qml` persists desktop-audio, microphone and edit-after-recording choices in `record.json`; the ordinary settings backend also writes `shell.json` and `matugen.json`; upstream `Flags.qml` writes DND, keep-awake, and game-mode flags; `Devices.qml` stores screen vibrance as a scalar in `nvibrant-value`; the pinned shell stores the raw user-selected backlight level, touchpad-disabled intent, night-light enabled/temperature/schedule state, and per-wall Stage effect/layer arrangement in seven exact state files. These preferences are restored as data and do not grant a new privileged device-control path. The Stage registry captures only the exact settings file; generated Stage images, progress records, and referenced source assets are not traversed. The pinned power-profile provider stores the selected profile in `power-profile.json` and user CPU/battery profile definitions and apply delay in `power.json`. These are bounded opaque user-owned preferences; capture does not authorize applying hardware settings. Capture is limited to these records, the profile selection, and the three `.local` parent-directory metadata records required for state preferences. Screenshot/recording outputs, desktop note text (`$XDG_STATE_HOME/ryoku/desktop-notes.txt`), generated palette/cache data, launcher-frequency history (`$XDG_STATE_HOME/ryoku/launcher-usage.json`), notification history/cache (`$XDG_STATE_HOME/ryoku/notifications.json` and `~/.cache/qs-rise-notifications.json`), clipboard history, calendar events, location, the separately rooted `$XDG_CONFIG_HOME/ryogami-wall/config.json` document (the pinned wallpaper module reads its `overviewBackdrop` section; the whole file also holds API keys, user paths, custom commands, and generated-output destinations, so it remains excluded pending a narrow preference migration), plugin-defined settings, arbitrary launcher scripts, API secrets (`$XDG_CONFIG_HOME/ryoku/rashin.env`), user assets referenced by paths, and other XDG state remain excluded pending separate schema and privacy review. |
| Local profile selection and layers | `/var/lib/ryoku-state/profiles/` plus per-user Ryoku profile selection under the Ryoku config root | User selection stays user-owned; installed profile defaults/system overlays are immutable generations. Companion and personal material is local-only and excluded from public examples. |
| Provider / package state | No exact Tumbleweed provider setting path is currently approved; package/upstream revisions may be recorded as manifest metadata | Provider writes remain unsupported pending #15 hook proof. Package databases and provider-owned files are not recovery inputs. |

Any future provider operation must first name and audit each affected path,
including package owner, writer, filesystem, ownership/mode/metadata, and
rollback behavior. A path outside a reviewed operation allowlist is not
managed merely because a tool can access it.

### v1 record inventory and metadata rules

The manifest root is selected by the trusted caller; a record path is always
relative to that root. This table defines the v1 enrollment boundary. Entries
called *candidate* are not enabled for a Tumbleweed writer until that writer
and its on-disk result are audited. A record is never permission to discover
siblings or recurse into a directory.

A disabled production scope now covers the port-owned Rust personal profile
writer: `InstalledInventory::personal_profile_writer()` enrolls only
`user-ryoku-config/opensuse-profile.json` with authenticated account UID/primary GID,
regular-file or explicit absence, and modes `0600`/`0644`. A second exact scope,
`InstalledInventory::session_preferences()`, captures that selection, the
upstream `shell.json`, `matugen.json`, `theme.json`, `brand.json`,
`launcher.json`, `widgets.json`, `performance.json`, `recording.json`,
`visualizer.json`, `stage.json`, `camera.json`, `ryoshot.json`,
`ryoshot-beautify.json`,
`equalizer.json`, `power.json`, `power-profile.json`, `profile.json`, and `keypresses.json` preference files, the `Flags.qml`, `Capture.qml`, `Recorder.qml`, and `Devices.qml` state, and exact backlight, touchpad, night-light, and Stage wall-registry preference files. These state records share the existing three parent-directory metadata entries; no directory walk is permitted. First profile creation
uses `0600`; updates preserve supported metadata. The seventeen configuration
preference files other than `keypresses.json` are regular files up to 1 MiB
with owner UID/primary GID
and mode `0600`, `0640`, or `0644`; capture fails above this explicit bound.
The keypress preferences file has a 64 KiB bound.
The flags, capture, and recording settings files are regular files up to 4 KiB
with the same ownership and those modes; `nvibrant-value` and night-light
temperature are regular scalars up to 16 bytes, backlight up to 32 bytes, and
the touchpad/night-light enable markers up to one byte. The night-light schedule
JSON is bounded to 4 KiB; the opaque Stage wall registry is bounded to 1 MiB. All use the same ownership and modes; the three state directories allow `0700`, `0750`, or `0755`. Missing
entries are recorded explicitly, symlinked path components fail capture, and
clipboard files are never enrolled.
The session inventory requires a verified local profile bundle to resolve the
selected profile; capture without that bundle fails before publication. It is
not the full Ryoku state set: user-referenced assets, plugin-defined settings,
launcher scripts, operational histories and other feature data are excluded
until their schemas, privacy and exact path policies are reviewed. The full
`InstalledInventory::current()` remains unavailable. The recovery fixture stages and restores pinned `flags.json` and
`stage-walls.json` records through a separate home-root descriptor, verifies the
Stage bytes and mode, and checks that an unrelated home sentinel survives. The coordinator still does not invoke this
session inventory for installed mutations. A recovery fixture now stages the
exact pinned `shell.json` and `matugen.json` settings alongside the flags file through separate
configuration and home-root descriptors, while preserving an unrelated home
sentinel. Its disabled personal-profile
coordinator now records a verified generation/Snapper pair in the protected
`snapper-catalog/catalog.json` after reopening both partners. It marks that
scope `configuration-only` because boot coverage is not established; it cannot
claim a full-system checkpoint. No installed linked-generation/console restore
is claimed. See the
[personal writer and remaining composition](local-profile-storage.md#disabled-personal-profile-production-scope).
This does not enroll the upstream UI/profile format described below or enable
any installed service mutation dispatch.

The recovery CLI also has a root-only `manual-checkpoint` action (with a
legacy `checkpoint-session` alias) for the
fixed `session_preferences` inventory. It creates and verifies a current-root
Snapper snapshot, captures the exact user settings/profile-selection paths,
seals the account binding, and records a linked entry with `unsupported`
capability because boot coverage is not assessed. It is not a native cleanup
pin. State initialization provisions that catalog; ordinary recovery opens it
without creating missing files. The CLI can prepare a configuration-only
restore from an exact session checkpoint with
`sudo ryoku-recovery prepare-configuration RESTORE_ID SOURCE_TRANSACTION_ID`.
It checks the recorded Ryoku package revisions and settings schemas before
creating the fresh current-state checkpoint and staged journal. Catalog
capability remains unsupported, so this does not establish cleanup-safe
retention or prove the complete offline console workflow. Checkpoint capture,
preparation and replay require `rescue.target` or `emergency.target` while
`graphical.target` is inactive; a root TTY in a live graphical session is
refused.

The read-only `sudo ryoku-recovery retention-plan KEEP_LATEST` command loads
the protected Snapper catalog, verifies generation links and their sealed
historical Btrfs receipts, and queries fresh Snapper inventories before
printing `pinned`, `keep`, `prune_candidates`, and `reconcile` transaction IDs.
It holds the common mutation lock in shared mode while assembling the report,
so an integrated exclusive mutation cannot change linked rows during that
read. The command is advisory: it does not delete a
snapshot, generation, or catalog row, and its output is not authorization for a
later cleanup operation. A future cleanup coordinator must reacquire the
common mutation lock exclusively and reobserve every partner before acting;
external Snapper activity does not participate in Ryoku's lock.

The offline CLI now exposes the library's two system rollback paths. Use
`prepare-system RESTORE_ID SOURCE_TRANSACTION_ID` to verify the source's
cataloged root snapshot and package revisions, stop the bound user's session
writers, and capture all paths declared by the source into a fresh linked
current-state checkpoint. `rollback-system RESTORE_ID` reopens that exact
checkpoint and verifies the running writable Btrfs root still has the origin
UUID recorded at capture. It then persists the writer-stopped phase, requests
one tagged Snapper rollback, and asks systemd to reboot while retaining the
writer guard through the handoff. Re-enter rescue/emergency mode after boot,
then `confirm-rollback` verifies the actual Btrfs/Snapper target;
`complete-system` runs required service health checks before opening the
recovery gate. Reboot after completion to resume normal sessions.

For paired recovery, `prepare-linked RESTORE_ID SOURCE_TRANSACTION_ID` stages
the selected checkpoint and captures its matching current state;
`rollback-linked RESTORE_ID` performs the journaled root rollback and reboot.
Re-enter rescue/emergency mode after boot, run `confirm-rollback RESTORE_ID`,
then `restore-configuration RESTORE_ID` to replay the staged user settings and
complete the pair; reboot once more to resume normal sessions after the journal
is complete. Configuration replay validates restored JSON/profile data and
requires both a healthy systemd manager and the active recovery-gate socket
before the journal can reach `Complete`. These new CLI routes compile and their malformed-ID
rejection is covered locally; neither route has yet passed the required native
Tumbleweed offline restore, process-interruption, or unrelated-home sentinel
comparison. Treat every interrupted or failed phase as pending recovery and
reconcile it explicitly; commands refuse to retry a rollback automatically.
After a complete-linked restore reaches `Complete`, the recovery CLI rechecks
the exact source generation, active rollback target and live Snapper partner.
It then marks the source catalog row `known_working` only when its capability is
full-system and verifies the catalog write by reading it back. Repeating the
command after the journal is complete retries this catalog step. This catalog
update has local compile and CLI-test coverage only; it has not been exercised
in the Tumbleweed offline restore gate.

| Manifest root | Candidate record path | Owner and writer contract | Entry and mode policy | Metadata policy |
|---|---|---|---|---|
| `user-ryoku-config` | `opensuse-profile.json` | The target ordinary account (resolved UID and primary GID); no production writer is enabled. Future writes must use the port-owned typed profile writer through the linked checkpoint coordinator. This name is deliberately distinct from the preserved upstream Hub file. | Regular file or explicit absence; `0600`/`0644`; schema-1 `ProfileDocument`; maximum 64 KiB. Preserve and verify source mode. | Record size, SHA-256, UID/GID, mode, mtime, ACL digest, and present allowlisted xattr digests. |
| `user-ryoku-config` | `profile.json` and, only for the currently selected custom hero, `profile/` plus `profile/hero.<ext>` | The target ordinary account; written by the pinned upstream Ryoku Hub profile UI. The upstream writer requests mode `0644`; this is source behavior, not measured openSUSE state. | Preserve `profile.json` opaquely (regular file or explicit absence, up to 1 MiB; `0600`/`0640`/`0644`). For a valid custom selection, enroll only its safe `hero.<lowercase-extension>` basename, a regular file up to 16 MiB, and its directory metadata. No directory enumeration; unsupported or changing selections fail capture. | Preserve raw bytes and observed UID/GID, mode, mtime, ACL digest and supported xattr digests for every enrolled record. |
| `user-ryoku-config` | `theme.json`, `shell.json`, `launcher.json` | Same user ownership and session writer boundary; source-backed candidate names, not proof these files are installed or used by the openSUSE UI. | Regular files only; each path must be explicitly enrolled by the feature that writes it. | Record source metadata; do not copy xattr values or file contents into public inventory documentation. |
| `user-ryoku-config` | `profile/` and the exact `profile/hero.<ext>` selected by a valid custom hero in `profile.json` | Same resolved target account; written by the pinned upstream profile UI. | Directory metadata plus one regular file up to 16 MiB; only safe `hero.<lowercase-extension>` basenames are accepted. The selected file and directory must exist. No wildcard or directory walk. | Same metadata requirements as other enrolled user configuration records. |
| `etc-ryoku` | No v1 record is approved yet. `appearance.toml` is a synthetic schema example only. | Would require an audited root-owned writer and exact packaged path ownership first. | No system path is admitted based on the proposed `/etc/ryoku` namespace alone. | Capture must fail closed on any unavailable required metadata. |
| `state-generation` | No transaction file record. | Root-owned immutable generation storage is internal protected state, not an input root for arbitrary manifest paths. | Generation publication is controlled by the storage protocol below. | Manifest files do not recursively describe the catalog, journal, or their containing directories. |

For an enrolled user regular file, the caller resolves the account once and
checks the observed UID and primary GID against that identity. It preserves
the observed permission bits rather than changing ownership or normalizing
modes; mode changes must be an explicit operation. `0644` above is the pinned
upstream writer's requested mode for selected preference files, not a promise
about umask effects or live Tumbleweed metadata. Root-owned system entries, if
later approved, must declare the exact package owner, writer, UID/GID and
permitted mode before capture is enabled. Catalog and generation directories
are root-owned with mode `0700`; their internal files are root-owned and are
not restored as user configuration.

The regular-file policy is the default. A symlink record is permitted only
when the specific enrolled path declares that symlinks are allowed. Capture
stores the raw target bytes and link metadata using `lstat`-style semantics;
validation, digesting, backup, and restore never resolve the target. A target
may point outside the declared root without authorizing access to that target.
Directory records describe that directory alone and never grant recursive
traversal. Device nodes, sockets, FIFOs, hard-link surprises, and unlisted
entry types fail capture. An absent record is valid only for a declared exact
path; it cannot be synthesized by scanning a parent directory.

The Linux collector captures exact symlink leaves with `lstat`-style metadata
and raw target text. It reads the final link with `readlinkat` and captures
symlink xattrs through `lgetxattr` anchored at the already-open parent
descriptor; it never opens or follows the link itself. Absolute targets,
targets containing a `..` component, invalid UTF-8 targets, and targets over
4096 bytes fail closed because the current generation policy only admits
relative in-scope target text. Intermediate symlink components remain
rejected. Symlink capture has focused fixture coverage, but no live managed
configuration path is yet approved for enrollment. Root selectors are also
not filesystem paths supplied by a manifest: a trusted caller must resolve
`user-ryoku-config` to the selected account's config directory descriptor,
`user-ryoku-profile` to the selected account's profile area, and `etc-ryoku`
to the reserved `/etc/ryoku` namespace descriptor. No system file is currently
approved under that namespace. `state-generation` is protected store-internal
space, not a caller-selected source root. The collector enforces entries it
receives; it does not yet compile the inventory or establish those trusted
descriptors.

| Metadata field | V1 meaning | Missing / unsupported behavior |
|---|---|---|
| `mode`, `uid`, `gid`, `mtime_unix_ns` | Observed permission/special bits, owner, group, and nanosecond mtime for the exact entry. | These are required observations. Capture errors rather than substituting defaults. |
| `acl_sha256` | Digest of both supported POSIX access and default ACL xattrs, with names and presence markers included. `null` means both reads succeeded and neither ACL xattr exists. | An unsupported ACL read is an error, not `null`. |
| `extended.values` | Digests of present allowlisted xattrs; the current allowlist contains only `security.selinux`. An empty map means that label read succeeded and no label was present. | Unsupported xattr reads and oversized values fail capture. No raw xattr values enter the manifest. |
| Regular-file `size`, `sha256`, `hard_link_group` | Bounded byte count and content digest; hard-link identity is represented only when the exact declared set supports it. | I/O failure, file mutation during capture, and undeclared hard-link relationships fail capture. |
| `absent` | The declared relative path was missing at capture time. It is an affirmative record, not an inventory scan result. | Only the exact declared path can later be removed by a verified restore. |

The root selector mapping above is an interface contract, not implemented path
discovery. Until a reviewed inventory compiler and descriptor resolver are
integrated, these records prove capture primitives only and do not authorize a
production recovery transaction.

Record POSIX ACL data in `acl_sha256` and SELinux labels by digest in the
allowlisted xattr map. The only v1 xattr name is `security.selinux`; do not
collect `user.*`, capability, or arbitrary security namespaces. The ACL digest
represents the supported access/default ACL metadata for that entry; do not
silently omit an ACL because the runtime or filesystem cannot read it. The
collector must report whether each applicable metadata source is supported;
unsupported required metadata makes the capture incomplete. Metadata values
are not included in public examples, only synthetic digests.

The following are explicitly external to transaction file inventories:
`/usr/share/ryoku/config/` (package-owned immutable defaults, recovered by
reinstalling the exact RPM), `$XDG_STATE_HOME/ryoku/` (upstream CLI operational
state), `$XDG_DATA_HOME/ryoku/plugins/`, `$XDG_CACHE_HOME/ryoku/`, unrelated
home files, and all unreviewed provider paths below `/etc`, `/var`, or `/usr`.
`/var/lib/ryoku-state/catalog`, `journal`, `generations`, and `profiles` are
protected storage internals rather than manifest entries. Their location does
not authorize following symlinks or reading arbitrary content there.

### Pinned upstream findings and first-sprint scope

The upstream pin in `upstream/ryoku.source.toml` was inspected for the
first-sprint user-state contract. It defines the CLI state root as
`$XDG_STATE_HOME/ryoku` (default `~/.local/state/ryoku`) in
[`ryoku/cli/internal/sys/paths.go`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/internal/sys/paths.go),
and the packaged config source and user materialization rules in
[`ryoku/cli/internal/updater/materialize.go`](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/internal/updater/materialize.go).
These references identify upstream ownership concepts; the Go implementation
is not shipped by this port. The first-sprint inventory is:

| Exact path or pattern | Owner / writer | First-sprint recovery treatment |
|---|---|---|
| `/usr/share/ryoku/config/` | Immutable package-owned defaults, installed by the exact Ryoku RPM revision. | Do not copy into a user generation. Record the package revision; reinstall the same package to recover defaults. |
| `$XDG_CONFIG_HOME/ryoku/` (default `~/.config/ryoku/`), only individually declared preference and profile files | Ordinary-user owned; written by the Ryoku UI/config writer. Upstream examples include `hypr.json`, `shell.json`, `launcher.json`, `hub.toml`, `profile.json`, `profile/hero.*`, and sparse `user_edits/` files. | Capture only allowlisted paths. Preserve owner, mode, symlink target and supported ACL/xattrs. Unknown files and directories are not recursively included. For a valid custom selection, capture only its exact safe hero basename with the reviewed type/size rules; sparse overlays remain excluded. |
| `$XDG_STATE_HOME/ryoku/` (default `~/.local/state/ryoku/`) | User-owned upstream CLI operational data; examples include `manifest.json`, `materialized`, `release-cache/`, `provisioned/`, `migrations/`, and `repo`. | Excluded from first-sprint configuration generations. It can contain cache or provision receipts; no blanket copy. Add individual records only if a supported operation needs them and defines format and privacy. |
| `$XDG_STATE_HOME/ryoku/clipboard/` (default `~/.local/state/ryoku/clipboard/`) | User-owned clipboard history index and content blobs written by the selected shell provider; it may contain credentials or other sensitive copied data. | Explicitly excluded from linked generations and restore. Do not recursively capture the XDG state root. Clipboard history has no backup or restore guarantee; configuration and profile recovery must leave this operational data out of its declared write set. |
| `$XDG_STATE_HOME/ryoku/{events.json,screentime.json,weather-loc.json,weather-cache.json,record-session,discord-record}` | Pinned shell writers hold calendar entries, recent app-usage history, private weather location/cache, recording-session timing, and an integration flag. | Excluded: these are personal history, location, cache, or live-session state rather than the declared first-sprint preference set. No directory walk or content import is performed. |
| `$XDG_STATE_HOME/{ryoku-wallpaper,ryoku-ryowalls.json,ryoku-live-frames/}` and legacy `$XDG_STATE_HOME/ryoku-live-frame.png` | The pinned daemon reads the wallpaper selector and per-wall tune from the XDG state root; the separate wallpaper producer is not in this shell archive or the current desktop RPM. The daemon creates live video-frame cache entries under `ryoku-live-frames/` and removes the legacy single-frame cache. | Excluded: the selector and tune belong to an unbundled wallpaper integration with no reviewed writer contract in this package; generated stills are rebuildable cache, and source media is not copied. The legacy single-frame cache is not enrolled. |
| `$XDG_STATE_HOME/ryoku/store/{barstyles.json,revision.json,barstyle-views/}` | The shell's bar-style provider reads downloaded views and an index/revision record from the upstream store. Views can include executable QML. | Excluded with the store payload. A generation must not restore downloaded executable UI or its index without a separately reviewed artifact, provenance, and compatibility contract. |
| `$XDG_CONFIG_HOME/ryoku/{desktop,hypr,ryogami,decor}.json`, `hub.toml`, `monitors/<name>.json`, and `stash.json` | Pinned Hub/providers store window-manager/system controls, hardware-specific display profiles, and the Cobalt container-engine toggle. | Excluded until each provider/write boundary and dynamic path contract is reviewed. In particular, restoring the Cobalt toggle can start external services, and monitor filenames are user/hardware-selected. |
| `$XDG_CONFIG_HOME/ryoku/{plugins.json,launcher-snippets.json,launcher-quicklinks.json}` and user plugin/config payloads | Upstream plugin settings can contain arbitrary plugin-defined values; launcher snippets and quick links can contain user-authored commands or URLs. | Excluded because these records can carry executable behavior, external references, or private user content. They require a distinct schema, privacy, and authorization review. |
| `$XDG_DATA_HOME/ryoku/plugins/` (default `~/.local/share/ryoku/plugins/`) | User-owned plugin payloads; upstream tracks plugin receipts/state separately. | Excluded from first-sprint recovery. Plugin payloads, secrets and executable content need separate review. |
| `$XDG_CACHE_HOME/ryoku/{colors.json,hypr-perf.json}` (default `~/.cache/ryoku/`) | The shell derives wallpaper palette data and performance hints at runtime. | Excluded and rebuildable; never use as a recovery source. Other Ryoku cache entries are likewise not traversed. |
| `$XDG_CONFIG_HOME/hypr/user.lua`, `$XDG_CONFIG_HOME/kitty/user.conf`, `$XDG_CONFIG_HOME/fish/user.fish` | User-owned raw overrides named by upstream `docs/config-import.md`; not necessarily managed by the openSUSE port. | Preserve as user data unless a specific import feature is implemented and the file is explicitly enrolled. Package reinstall must not replace them. |
| `/var/lib/ryoku-state/catalog/`, `snapper-catalog/`, `restore-journals/`, `mutation.lock`, `generations/`, `staging/`, and `profiles/` | Root-owned openSUSE storage internals on the separately validated state subvolume; recovery commands and explicit provisioning paths are their only current writers. | These are protected store internals, not user/system file records in a transaction manifest. Their integrity/publication rules are defined above. |
| `/etc/ryoku/` and openSUSE provider files | Root-owned; exact contents depend on the selected Rust adapter and supported operation. | Not yet audited against an installed openSUSE package. No broad `/etc` traversal or provider path is authorized. Add exact files, owner, writer and restore behavior before enabling each operation. |

This inventory is intentionally scoped to supported Ryoku preferences, profile
selection and local package identity. It does not claim to inventory every
upstream Quickshell surface, plugin, or user edit. Upstream `docs/profile.md`
describes a user-facing profile page and hero image, not the openSUSE four-layer
precedence above; that precedence is a new local contract. Package intent and
upstream revisions may be recorded, but pacman databases and generic provider
state are not restore targets.

### Exact paths evidenced by the pinned upstream source

The following are source-backed path contracts at the immutable revision above.
They describe upstream Ryoku ownership; they are not a claim that these files
exist on the Tumbleweed guest or that every upstream feature is in this port.
Variable roots are resolved for one target user and never authorize walking the
home directory.

| Path or exact file set | Source-backed writer/ownership | Recovery scope |
|---|---|---|
| `$XDG_CONFIG_HOME/ryoku/{desktop,hypr,shell,theme,launcher,ryogami,widgets,visualizer,decor,brand,profile}.json` and `hub.toml` | Upstream Hub/provider preference stores; written by the user session. `rice.go` names its store set and Hub `config.go`/`profile.go` define their locations. The default root is `~/.config/ryoku`. | Per-file opt-in records only for settings the openSUSE UI actually supports. Do not treat the directory or JSON suffix as a recursive allowlist. |
| `$XDG_CONFIG_HOME/ryoku/profile.json` and selected `profile/hero.<ext>` | Upstream Profile UI writes JSON and copies a selected custom hero into `profile/`; both are user-owned. | The `session_preferences` capture preserves the opaque selection document and, only for a valid custom selection, the exact safe basename with a 16 MiB bound and its directory metadata. It does not walk `profile/`, include unselected assets, or conflate this document with `opensuse-profile.json`. |
| `$XDG_CONFIG_HOME/hypr/{monitors.lua,gpu.lua,keyboard.lua,user.lua}` and optional `monitors_user.lua`; generated `settings.lua`/`rebinds.lua`; mirrored generated overlay files `ryoku/user_edits/hypr/{settings.lua,rebinds.lua}`; user-editable `modules/` | The pinned provider interface (`ryoku/cli/vendor/ryoku-wm/detect.go`) distinguishes generated seeds, user-owned files, generated outputs and hand-edit escape hatches. | Not a current Tumbleweed provider set. If a compatible provider is later supported, list only specific files in that operation. |
| `$XDG_CONFIG_HOME/niri/{monitors.kdl,gpu.kdl,keyboard.kdl,user.kdl,monitors_user.kdl}`; generated `settings.kdl`/`rebinds.kdl`; mirrored generated overlay files `ryoku/user_edits/niri/{settings.kdl,rebinds.kdl}` | Same pinned provider interface; niri's `monitors_user.kdl` is a required seeded include. | Not a current Tumbleweed provider set. No arbitrary `niri/` capture. |
| `$XDG_CONFIG_HOME/ryoku/user_edits/<relative-path>` | Sparse user-authored forks. Upstream enumerates regular files, skipping symlinks, `.md` files, and nested `ryoku/user_edits/`; the overlay mirrors paths into `~/.config`. | Preserve only a future explicitly enrolled file. Upstream's recursive overlay behavior is not adopted as this project's capture policy. |
| `$XDG_STATE_HOME/ryoku/{repo,deployed,materialized,doctor-report.txt}` | CLI operational state at default `~/.local/state/ryoku`; upstream CLI reference identifies checkout, deploy revision, materialization hash manifest, and latest report. Other features have independent state beneath this root. | Excluded from configuration generations. Do not copy this directory or rely on these Arch-updater records for restore. The separately declared `clipboard/` subtree is also excluded because it may contain sensitive copied content. |
| `$XDG_DATA_HOME/ryoku/plugins/`, `$XDG_STATE_HOME/ryoku/plugins/<id>`, `$XDG_CACHE_HOME/ryoku/plugins/<id>` | Plugin payload, plugin state and cache have separate upstream roots. Plugin payloads can contain executable code. | Excluded. Plugin sync/recovery requires a separate reviewed contract. |
| `/usr/share/ryoku/config/` | Package-provided base config. The pinned `materialize.go` copies regular files into the user's config root and stores a path-to-content-hash manifest at `$XDG_STATE_HOME/ryoku/materialized`. | Package source is regenerated from the exact package revision, not copied into a user generation. The hash manifest does not represent full file metadata. |

Pinned implementation references: [Hub store paths and rice store set](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/hub/backend/rice.go), [Hub config path](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/hub/backend/config.go), [profile file and hero directory](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/hub/backend/profile.go), [preference write mode and atomic writer](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/hub/backend/desktop.go), [Hyprland/Niri seed and generated path contract](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/vendor/ryoku-wm/detect.go), [sparse overlay enumeration](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/internal/sys/useredits.go), and [CLI state file descriptions](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/docs/cli.md).

`materialize.go` and `sys/fsops.go` prove that the upstream materializer tracks
relative paths and SHA-256 values and copies file permission bits into a new
user-owned file. It does not capture or restore source UID/GID, mtime, ACLs,
xattrs, hard-link identity or symlink targets. Its behavior therefore cannot
serve as evidence that a recovery collector already meets the metadata
contract. The openSUSE manifest must capture metadata independently and fail
closed when a required supported attribute cannot be read.

For the cited user-session writers, source calls request `0755` directories and
`0644` preference files (`atomicWrite` explicitly chmods files); resulting
ownership is the account running the UI/materializer. `CopyFile` requests the
source permission bits for its temporary copy but does not chown or restore
timestamps. These are writer semantics, not observed Tumbleweed metadata and
not proof that all upstream files use identical modes.

The pinned upstream tree also contains system/installer paths such as
`/etc/ryoku-release`, `/etc/ryoku/default-kernel`,
`/etc/ryoku/snapshots-disabled`, `/etc/ryoku/limine-existing-esp`,
`/etc/systemd/logind.conf.d/10-ryoku-lid.conf`,
`/etc/sddm.conf.d/99-ryoku.conf`, and `/var/lib/ryoku/*`. Their writers and
ownership rules are specific to the Arch installer/packages or upstream greeter
implementation. They are **not** evidence for Tumbleweed package ownership,
and none is admitted to this manifest by name or by directory.
The source paths for these examples are the pinned [desktop package
recipe](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/release/packages/ryoku-desktop/PKGBUILD), [installer
bootloader](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/installation/backend/lib/bootloader.sh), [installer
snapshot choice](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/installation/backend/lib/snapshots.sh), [greeter
writer](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/hub/backend/lock.go), and [tmpfiles
ownership declaration](https://github.com/Ryoku-dev/ryoku/blob/98a0368253d9628be72b853985878a5db90e1547/ryoku/cli/systemd/ryoku.tmpfiles.conf).

### Tumbleweed evidence boundary

The committed Tumbleweed fixture pins the `snapshot20261011` repository
metadata digests and requests `plasma6-session`, `sddm`, and
`xorg-x11-server`. Sanitized Run17 evidence records installed versions
`plasma6-session 6.7.5-1.1` and `sddm 0.21.0+git57-1.1`, with SDDM active and
the ordinary-user KDE login blocked. It does not record `rpm -qf` results,
package file lists, post-install `/etc` contents, user config paths, UID/GID,
modes, ACLs, or xattrs. The run therefore proves package presence/version and
service state only.

There is currently no supported Tumbleweed provider write set in this project:
package, repository, Snapper, Plasma/SDDM and arbitrary system-setting writes
remain outside the recovery allowlist until their #15 pre-mutation boundary and
affected-state inventory are proven. The log does not justify declaring
`/etc/sddm.conf.d`, `/etc/zypp`, `/etc/PackageKit`, `/etc/snapper`, or any other
provider path Ryoku-owned. Exact package ownership and file metadata require a
sanitized guest inspection after installation; this audit did not connect to
or modify the running VM.

For every declared regular file record: normalized relative path, entry type,
size, SHA-256, mode, UID/GID, modification time, hard-link group where
relevant, ACL digest, and allowlisted extended-attribute names and digests.
For symlinks record mode, owner, and raw target without following it. For
directories record mode, owner, and relevant metadata. Record an explicit
`absent` entry for every declared destination that did not exist before the
transaction; restore removes a transaction-created file only when that
absence record is linked to the same verified generation. Do not store file
contents for secrets or undeclared data in the public catalog.

## Manifest schema and publication rules

An optional `account_principal` field records the original per-user capture
association in schemas 1 and 2. It contains a versioned SHA-256 commitment over
the trusted account resolver's UID, primary GID, account name and canonical home.
It is covered by the manifest seal and never confers authorization on its own.
Omission preserves legacy canonical bytes and means the original account is
unproven. A per-user helper access path must refuse that omission even when all
user records are `absent`; file metadata cannot establish the missing identity.
Later NSS results must never retroactively bind an existing generation.
Authorized use repeats account/root resolution and compares the original
commitment before preparation, staging and apply. Raw account names/home paths
and commitments do not belong in public diagnostics or evidence. An identical
recreated OS identity is indistinguishable under these four fields.

The schema has a required `schema_version`; the current typed writer accepts
version 1 only and rejects any other version before serialization. The v1
compatibility rule is strict: the persisted v1 reader rejects unknown fields
rather than silently discarding data. Optionality applies only to known v1 fields, where
an absent value means `None`. Any field-set or semantic change requires an
explicit local version-to-version migration. A migration writes a new
generation and retains the prior one until verification and catalog
publication succeed; it never fetches code or data from a Git remote. The reader parses and verifies sealed v1 JSON locally; a version-to-version
migration executor is not implemented yet.

Version 1 canonical encoding is compact UTF-8 JSON. JSON object keys are sorted
lexically, file records are sorted by fixed root name and relative path, and
the SHA-256 digest covers this canonical document with `manifest_sha256`
omitted. The sealed document includes that digest. The Rust recovery crate
validates v1 schema bounds before producing this canonical byte representation,
then seals and verifies the digest when callers construct the typed value. The
persisted reader accepts at most 1 MiB, validates parsed values, and verifies
the digest before returning a manifest. The Linux `generation_capture` layer
now composes exact-file capture with bounded no-follow payload reads, a second
metadata/content capture, manifest sealing, immutable publication, and
transaction read-back verification. The input remains an explicit caller
inventory and an externally verified snapshot-link attestation; this code
does not validate the dedicated subvolume/mount identity, freeze writers,
append the operation journal, derive an audited inventory, contact Snapper,
restore paths, or authorize a provider mutation.

The exact signed `mtime_unix_ns` value is encoded as a canonical decimal JSON string so the entire supported i128 range survives serde_json's canonical value representation without rounding or overflow.

Generation payload filenames are their lowercase SHA-256 values. A generation contains only `manifest.json` and `payloads/<sha256>.blob`; duplicate file content is stored once, and any unlisted entry invalidates verification. The store enforces 256 manifest entries, 16 MiB per file, 64 MiB total payload data, and the existing 1 MiB manifest limit. The transaction catalog entry is compact canonical JSON containing schema version, transaction ID, generation ID, and manifest digest. The storage root and its `staging`, `generations`, and `catalog` directories must be root-owned mode `0700`; published generation directories are mode `0500`, and their files are mode `0400`. Publication writes only below directory descriptors, refuses symlinked store subdirectories, synchronizes files and directories, verifies every referenced blob against its size and digest, renames the complete staging directory into `generations/<generation-id>` without replacement, then publishes `catalog/<transaction-id>.json` without replacement. `verify_transaction` requires a canonical catalog link and revalidates the manifest and every referenced blob. Incomplete staging is never considered by readers. A verified generation means its supplied manifest and content agree; it does not by itself authorize a restore or mutation.

The protected payload map also retains raw metadata needed by a future restore.
Each present `acl_sha256` requires the exact canonical `ryoku-posix-acl-v1`
bundle containing access/default names, presence bits, lengths, and raw values.
This is the existing ACL digest preimage, so manifest hashes and schema-v1
encoding stay unchanged. Metadata-bearing generations use catalog schema v2,
which older readers reject explicitly. Content-only generations retain catalog
schema v1, and the new reader checks that the catalog version matches its
generation's metadata requirements. Each `extended.values` digest requires the
raw value of its allowlisted xattr. Metadata for directories and symlinks is included,
even without regular-file content. Raw values remain in mode-`0400` protected
blobs; public manifests still contain only digests. The reader rejects malformed
ACL encodings, missing/extra/corrupt blobs, and values over 64 KiB. The store
allows at most 768 distinct blobs for 256 declared entries (file content, ACL
bundle, and SELinux label per entry); total payload data remains capped at
64 MiB. The initial space estimate includes metadata blobs and their allocation
overhead. Source-byte limits include actual metadata bytes across every root;
ACL serialization overhead counts toward stored bytes rather than source bytes.

Previously published generations that declare metadata hashes without storing
their raw blobs now fail verification. Their missing values cannot be inferred
from current files; retain those artifacts as incomplete evidence and capture
a new generation from an approved live inventory. No old metadata generation
is silently relabeled complete. Metadata capture/persistence tests use synthetic
ACL/label values; applying these values, preserving live Tumbleweed metadata
through restore, and interruption/reboot recovery remain unimplemented or
unverified. This change does not enable a restore command or provider write.

The lower-level publisher still accepts a caller-supplied exact
digest-to-bytes map. `capture_publish_verify` builds that map only from the
records returned by the exact-file collector, reopens regular files beneath
their supplied descriptors without following symlinks or mount crossings,
checks their size/digest, repeats the capture and compares the complete
records, then invokes the publisher and verifies the transaction from disk.
Any capture, metadata, payload, or read-back failure returns an error; no
verified manifest is returned. A crash during storage publication leaves
unindexed staging or an unindexed generation, which store readers ignore.
Before reopening payloads, the capture path checks available blocks through
the open staging-directory descriptor. It bounds requested payload bytes from
the initial inventory and includes the maximum manifest size and per-file
allocation overhead. This preflight is advisory rather than a reservation;
any later write or synchronization failure still aborts capture, so callers
must not authorize mutation unless publication and read-back verification both
succeed. This check covers the generation store filesystem only; it does not
estimate Snapper snapshot or provider package transaction space.
Capture preserves safe relative symlink targets as text without following
them and rejects escaping targets. The repository now has generation/catalog
reconstruction, mount identity validation, mutation locking and durable
recovery journals. Protected-content classification, the audited multi-layer
inventory compiler, provider writer integration, supported-filesystem metadata
proof and complete crash/reboot recovery evidence remain incomplete. Issue #18
stays open until customized multi-layer, corrupt-input, interrupted-capture,
and linked restore acceptance fixtures pass.
The synthetic end-to-end unit fixture injects a test-only no-xattr reader
because its temporary filesystem does not expose the required ACL xattrs;
production capture still uses the kernel reader and rejects unsupported
metadata. Additional injected-metadata fixtures verify preservation of access
and default ACL bytes and SELinux labels for files, directories, and symlinks,
metadata deduplication, cross-root byte limits, and a complete 256-entry
inventory with more than 256 metadata blobs. A filesystem-backed configuration
application fixture also captures a real base-only default ACL before staging,
restores it to the destination directory, and compares the exact kernel
readback bytes. Named-user ACL restoration still requires a native fixture
with distinct mapped UIDs.

Every transaction manifest records:

- transaction, generation, and parent-generation IDs;
- Snapper configuration and pre-change snapshot IDs, plus the filesystem UUID;
- operation reason and UTC creation time;
- each changed package's explicit package name, before/after NEVRA, and
  upstream source revision where applicable, plus provider identity and
  before/after provider revision;
- selected profile ID, profile revision, and the resolved profile-layer
  revision set;
- declared file records (including `absent` records), type, ownership, mode,
  symlink target, allowed metadata digests, size, and content digest;
- verification result per phase (`pending`, `verified`, or `failed`) and a
  final manifest digest over canonical encoding.

IDs and paths are validated before lookup. The manifest is written to a
private temporary entry in the dedicated state volume, synchronized, read
back and checked, then atomically published. A failed capture, metadata read,
digest check, or Snapper link prevents mutation. Catalog indexes are
rebuildable from verified immutable manifests; the journal is append-only and
is not reconstructed from mutable root state.

Synthetic canonical JSON shape only (identifiers and paths are examples; no
user or companion information is present):

```json
{
  "capture_status": "verified",
  "created_utc": "2026-01-01T00:00:00Z",
  "files": [
    {
      "hard_link_group": null,
      "kind": "regular-file",
      "metadata": {
        "acl_sha256": null,
        "extended": { "values": {} },
        "gid": 0,
        "mode": 416,
        "mtime_unix_ns": "0",
        "uid": 0
      },
      "path": "appearance.toml",
      "root": "etc-ryoku",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "size": 34
    },
    { "kind": "absent", "path": "local-overrides.toml", "root": "user-ryoku-config" },
    {
      "kind": "symlink",
      "metadata": {
        "acl_sha256": null,
        "extended": { "values": { "selinux-label": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" } },
        "gid": 1000,
        "mode": 511,
        "mtime_unix_ns": "0",
        "uid": 1000
      },
      "path": "theme-current",
      "root": "user-ryoku-config",
      "target": "themes/neutral.toml"
    }
  ],
  "filesystem_uuid": "00000000-0000-0000-0000-000000000000",
  "generation_id": "gen-example-0007",
  "linkage_status": "verified",
  "manifest_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "package_revisions": [],
  "parent_generation_id": "gen-example-0006",
  "profile_revision": null,
  "provider_revision": null,
  "reason": "profile-apply",
  "schema_version": 1,
  "snapper_config": "root",
  "snapper_snapshot_id": "42",
  "transaction_id": "txn-example-0001",
  "verification_status": "verified"
}
```

The Rust `ryoku-recovery` crate provides validated IDs, fixed root selectors,
root-relative paths, entry-kind records, duplicate-path rejection,
Snapper/filesystem linkage primitives, canonical JSON serialization,
manifest digest verification, and bounded offline parsing of sealed v1 JSON.
Its Linux capture module accepts an already-open approved root descriptor and
an exact caller-declared path list. It opens parent components with
`openat2` resolution that rejects symlinks, magic links, and mount crossings;
it never recurses, records absent entries, hashes bounded regular files, and
captures owner, group, mode, nanosecond mtime, POSIX ACL digests, and
SELinux-label digests through the opened file descriptor. It detects
identity/content/metadata changes during capture and rejects special files,
escaping symlinks, incomplete hard-link sets, and filesystems that cannot supply
required metadata. Safe relative symlinks are captured separately without
following their targets. The `generation_capture` module accepts multiple distinct
fixed root selectors and explicit path lists, reads each regular payload
beneath its matching descriptor with no-follow/no-cross-mount resolution,
confirms the complete capture records are unchanged, then seals, publishes,
and verifies the exact generation. The caller must freeze writers and
attest an externally verified snapshot link; neither is established by this
module. A reviewed multi-root enrollment compiler,
authorization, restore, and Tumbleweed runtime proof remain unimplemented.
Neither collector nor generation capture is a privileged helper or can
authorize a mutation by itself.

The typed contract also names operation reason/time, per-package identity and
before/after NEVRA, provider identity and before/after revision, upstream source
revision provenance, all four resolved profile layers, per-phase verification,
the canonical manifest digest slot, hard-link groups, ACL/xattr digests, and
explicit capture exclusions. ACLs use the dedicated ACL digest field; the
allowlisted xattr enum contains only SELinux labels, represented by digest.
The fixed file roots are `/etc/ryoku`, resolved per-user Ryoku config/profile
roots, and the generation namespace. Catalog, journal, plugin payloads,
package databases, caches, and arbitrary home/provider files are not file
records. The collector only enforces the exact paths passed to it; a reviewed
inventory compiler and trusted root-descriptor resolver must supply that list.
Unsupported metadata fails capture rather than being recorded as absent.

The v1 JSON encoding stores `mtime_unix_ns` as a canonical decimal string so nanosecond values retain their exact `i128` range. The example is illustrative and synthetic; it is not proof of a live capture. Tests cover v1 schema bounds, canonical encoding, digest sealing/verification, stable file/package ordering, bounded strict parsing, unknown-field rejection, wrapper validation, tampering, and verified round trips. The parser accepts v1 only; a local migration executor and compatibility tests across schema versions remain future work.

## Profile resolution and private data

The merge order is fixed and per-key:

1. profile defaults;
2. local system overrides;
3. optional local companion settings;
4. personal user overrides.

The highest present value wins; an absent key inherits the previous value.
An absent companion layer is normal. The resolver needs only locally supplied
typed data and performs no Git or network access. The current
`ryoku-profiles` Rust model captures this merge order for the initial typed
settings surface. It has no filesystem, command, or network capability.
Its Rust module documentation includes a synthetic local example that resolves
all four layers without a Git remote, credentials, or companion payload.

The local `ProfileDocument` JSON contract is schema version 1, bounded to 64
KiB, and contains only a validated profile ID plus typed settings. Unknown
fields and noncanonical profile IDs are rejected; setting values are closed
enums, so documents cannot introduce paths, commands, or scripts. Canonical
serialization has fixed struct-field order and no maps. The parser accepts
only v1. There are no legacy inputs to migrate yet: an older schema remains
unsupported until a deliberate version-to-version migration and fixtures are
added, and a future version is rejected rather than guessed. This API parses
caller-supplied bytes, and the local loader can open only the fixed
`opensuse-profile.json` entry beneath a caller-supplied root directory descriptor. The
loader requires matching expected UID/GID on the root and file, root mode
`0700` or `0755`, file mode `0600` or `0644`, a single-link regular file, and
at most 64 KiB. It opens with no-symlink, beneath-root, and no-cross-mount
resolution, and rejects files modified while being read. The caller remains
responsible for securely obtaining the trusted root descriptor; this API does
not discover config paths. `save_local_profile` accepts the same already-open
root descriptor and authenticated owner identity, validates the document,
creates a same-directory exclusive temporary file, syncs it, atomically renames
it to the fixed leaf, and syncs the directory. New files use `0600`; supported
existing modes plus POSIX access ACL and SELinux label xattrs are preserved.
Unknown xattrs are rejected before replacement rather than silently discarded.
This user-owned preference write needs no Git,
companion service, credential, per-save checkpoint, or privileged helper. It
does not coordinate concurrent writers outside this crate; callers must
serialize updates if more than one writer can publish this document.
Companion retrieval and credential handling are outside this crate.

For the v1 recovery inventory, that document maps to the exact
`user-ryoku-config/opensuse-profile.json` candidate in the inventory table. It stores a
local profile identity and typed settings, but does not label itself as one of
the four resolution layers. The caller must explicitly map its values to the
intended layer; it cannot silently treat this file as defaults, system
overrides, companion, or personal overrides. The profile resolver accepts
those four typed values from its caller, while the recovery manifest's
`profile_revision` binds their provenance. A
missing optional layer is encoded as `null` in its revision slot (and as no
layer value to the resolver); it does not cause a remote lookup. Layer
revision labels identify the inputs used, but do not embed their contents or
establish that they have been persisted or made durable. In particular, there
is no public companion file path, payload, revision value, URL, credential, or
identity in this contract. System override persistence and the caller that
assembles all four layers remain integration work.

`ProfileLayers::content_revisions` now supplies a deterministic revision for
each present typed layer, hashing its canonical, map-free JSON with SHA-256.
Defaults, system overrides, and personal overrides always have revisions;
the optional companion revision stays absent when no companion input was
loaded. A present but empty companion is distinct from an absent companion.
Callers may place these opaque `sha256-…` values in the matching recovery
manifest revision slots. These hashes identify the layer values used during
resolution; they do not prove that those inputs were installed from a trusted
source, saved durably, or coordinated against concurrent writers. The public
API still accepts caller-supplied layer values and does not capture a real
installed profile set.

Synthetic manifest profile provenance (all values below are examples):

```json
{
  "profile_revision": {
    "profile_id": "demo-desktop",
    "profile_revision": "profile-r1",
    "defaults_revision": "defaults-r1",
    "system_override_revision": "system-r1",
    "companion_revision": null,
    "personal_override_revision": "personal-r1"
  },
  "files": [
    { "kind": "absent", "path": "shell.json", "root": "user-ryoku-config" }
  ]
}
```

The absent `shell.json` is meaningful only if the transaction explicitly
declared that exact candidate path; this example does not enable it for
capture. The synthetic revision labels describe no private or real profile.

Synthetic schema-v1 example:

```json
{"schema_version":1,"id":"demo-desktop","settings":{"desktop":"kde","theme":"ryoku_dark","density":null}}
```

Public profiles and examples must be synthetic. Never commit private companion
payloads, remote URLs, access tokens, user identifiers, private profile names,
or real personal overrides. Future sync/import code must remain outside the
generic local resolver; unavailable companion data must not block boot,
recovery, package updates, or use of public defaults. A profile may select
declared settings only; it cannot contain executable hooks, arbitrary paths,
package-manager commands, or provider authorization.

## Snapshot, boot, and exclusion coverage

| Scenario | Required behavior | Current evidence |
|---|---|---|
| Root Snapper rollback with valid linked generation | Keep catalog, journal, and matching immutable generation available; restore exact declared state. | CLI and engine support preparation, target confirmation, and configuration replay after the root rollback; full linked rollback/reboot remains unverified on Tumbleweed. |
| Root rollback with missing/corrupt generation or catalog link | Refuse silent fallback; report recovery unavailable/inconsistent and preserve evidence. | Local journal, generation, stage, and partner refusal tests; native offline end-to-end failure exercise remains outstanding. |
| Offline boot and recovery | Use local catalog, generation, and supported system tools; do not require a Git host, credentials, or companion. | Native runs cover state initialization, journal discovery, and bounded Snapper fixture recovery; manifest-backed console restore is not yet proven offline. |
| Configuration-only recovery where compatible Snapper state is absent | Offer only a clearly labeled configuration restore when its generation and file metadata verify; never claim system rollback. | Root-only CLI captures a current-state checkpoint, stages, stops account writers, replays and health-checks; fixture tests pass, native console acceptance is pending. |
| Btrfs root plus Snapper snapshots | Supported target when root and state subvolumes are separately validated and the restore order is implemented. | Native state initialization and checkpoint components exist; linked state retention across root rollback and reboot remains unverified. |
| Non-Btrfs root, unknown boot layout, firmware, partitioning, or arbitrary root edits | Out of supported restore scope; block integrated system mutation or require manual recovery. | Explicit exclusion. |
| Unrelated home content, secrets, external media, and undeclared provider files | Never traverse, capture, overwrite, or delete. | Descriptor-based fixture tests preserve undeclared peers; full native restore sentinel evidence remains outstanding. |

This contract does not expand Snapper coverage to firmware/EFI variables,
partition tables, bootloader installation, arbitrary root-owned state,
unrelated home files, or external devices. Boot recovery must be independently
authenticated and must validate that the linked generation is compatible
with the observed root snapshot before restore.

## Remaining proof before acceptance

- Install the dedicated state subvolume initializer and mount integration, then
  prove the catalog, journal, and generations survive an actual Snapper root
  rollback and reboot on the disposable Tumbleweed VM.
- Add capture race-injection coverage and run the ACL/SELinux round-trip tests
  on a supported native filesystem. The capture engine preserves safe relative
  symlink targets without following them; enrolled session preferences remain
  limited to their explicit regular-file/directory allowlist. Local capture
  tests do not prove live Tumbleweed ownership or metadata behavior.
- Exercise successful and deliberately corrupted manifest-backed linked
  restores offline, including configuration-only recovery, interruption
  boundaries, and preservation of an unrelated home sentinel. Existing native
  runs prove storage initialization, journal discovery, and bounded Snapper
  fixture restoration, not this complete restore path.
- Replace illustrative provider roots with an audited inventory from the
  selected installed desktop, integrated providers, and packaged Ryoku
  components.
- Link the live evidence and reviewed implementation commit to issue #16
  before marking it complete.

## Issue #16 checklist reconciliation

This matrix reports repository evidence without treating an ADR or synthetic
example as a live recovery result. Every issue checkbox remains open until the
corresponding implementation and disposable-guest evidence are linked.

| Issue #16 item | Current repository evidence | Still needed |
|---|---|---|
| Inventory Ryoku config, app/theme/shell state, provider state and profile layers | Pinned upstream paths, an explicit v1 exact-file enrollment boundary, ownership/mode policy, symlink/xattr rules and declared exclusions above; typed local profile layers | Exact Tumbleweed ownership/mode/metadata audit for integrated Ryoku paths and final package/session inventory after login support succeeds. |
| Define manifest IDs, reason/time, checksums, metadata, package/upstream/profile revisions and verification | Typed v1 manifest, canonical JSON/SHA-256 seal/verify, explicit package/provider revisions, profile revisions, metadata fields, bounded parser, and exact-file Linux capture for regular files/directories/absence and safe relative symlinks | Supported-filesystem ACL/SELinux integration proof, schema migration implementation/tests, and live Tumbleweed metadata capture. |
| Protect catalog/generations outside root rollback; validate subvolume, writer coordination and privilege split | Storage ADR, state-mount identity validation, generation/catalog stores, mutation lock, journal and native state initialization components | Installed startup/mount integration, linked root rollback/reboot proof on Tumbleweed, and end-to-end coordination with supported writers. |
| Define profile precedence, absent entries and compatible migrations | Rust local resolver, strict schema-v1 local document parser/serializer, synthetic doctest, absent-file records, and explicit reject-until-migrated policy | Integration with persistent generation capture/restore, any future schema migration implementation/tests, and offline linked restore proof. |
| Document snapshot/boot combinations, fallback and exclusions | Scenario matrix and explicit firmware, partitioning, arbitrary-root and undeclared-home exclusions | Exercise manifest-linked rollback, corruption handling and configuration-only fallback offline. |
| ADR retains catalog, journal and matching generation across root rollback | [ADR 0002](adr/0002-linked-recovery-storage.md) specifies the separate subvolume, identity validation, transaction link, profile contract, fallback behavior, and exclusions | Actual dedicated mount plus Snapper rollback/reboot evidence. |
| Inventory ownership, modes, symlinks, extended metadata and external paths without traversal | Typed metadata schema; explicit session-preference allowlist; no-follow capture, ACL/SELinux digests, absent records, hard-link checks, and safe relative symlink capture in the generic engine | Exact runtime ownership/mode/ACL/xattr evidence for integrated Ryoku state, a complete audited installed-path inventory, and race-injection tests. Run19 provider paths do not constitute a Ryoku-owned path set. |
| Profile interface works locally without remotes/credentials; public examples synthetic | Pure Rust resolver, bounded v1 parser, and fixed-name no-follow loader from a supplied descriptor; synthetic four-layer doctest and temporary-file validation tests | Caller integration for securely obtaining the intended config-root descriptor and persistence/writer coordination; companion persistence/sync remains out of scope. |

## Issue #21 restore journal implementation status

The `ryoku-recovery` Rust binary now has bounded, offline `list`, `inspect`,
and `verify` commands for the installed machine's generation store. `list`
verifies each cataloged generation before reporting its generation ID,
Snapper reference, and whether an original native receipt was recorded; a
generation that cannot be verified is marked unverified and unavailable
instead of being presented as a restore candidate. The command explicitly says
that this local integrity check does not verify the current Snapper partner,
which remains the job of `verify-linked`. It reads
the root-owned `/etc/ryoku/state-machine.json` record, validates the fixed
`/var/lib/ryoku-state` mount against that identity, then opens existing store
directories read-only and checks owner/mode. It enumerates committed catalog
links and verifies the canonical link, sealed manifest, and payload hashes.
`inspect` prints manifest summary fields and `verify` emits a compact
integrity result. Caller-supplied store paths are not accepted. Inspection
does not create state, write a journal, or authorize restore. Separate
`prepare-configuration`, `prepare-system`, `prepare-linked`, `rollback-system`,
`rollback-linked`, `confirm-rollback`, `restore-configuration`, and
`complete-system` commands now expose the available recovery lanes; they
use fixed protected paths, a verified current-state checkpoint, an exclusive
mutation lock, offline-console checks, and account writer quiescence. Their
availability does not establish a successful full linked restore or replace
the pending Tumbleweed console/interruption acceptance exercise.

The `plan` command accepts a transaction and one of the three supported
scopes. It verifies the saved generation and reports the number of captured
configuration entries. For configuration scopes, it lists each declared root
and relative path with the intended action, without printing file contents or
symlink targets. System and complete-linked previews also check whether the
cataloged original Snapper partner is currently available and compare the
three recorded Ryoku RPM revisions with the package database inside that exact
snapshot. Configuration-only previews compare those revisions with the
installed packages. This is only a read-only preview: it does not establish
full operating-system or boot compatibility, create a current-state
checkpoint, quiesce writers, obtain restore authorization, or make a restore
ready. A successful preview must not be treated as permission to execute it.

The Rust recovery crate now provides typed preflight plans for configuration-only,
system-only, and complete linked scopes, plus an atomically replaced, fsync'd
phase journal stored relative to a trusted private directory descriptor. Journal
updates are locked across processes, reject stale or skipped phase transitions,
and can be reloaded after process interruption. All scopes require a verified
current-state checkpoint. Configuration-only and complete-linked scopes also
require a verified generation/archive and known-compatible configuration
revisions; system-only requires a supported available snapshot and
known-compatible system revisions, without depending on the configuration
partner. Complete-linked additionally requires exact Snapper/filesystem
linkage. The privileged caller remains responsible for deriving the typed
compatibility evidence from installed package/profile state. Reload validates
the persisted snapshot ID, filesystem UUID, and scope invariants.

`GenerationStore` and the read-only `GenerationReader` can now open a
`VerifiedGeneration` handle after validating the complete catalog linkage,
manifest, and payload set. The handle pins its payload directory by descriptor,
rejects unreferenced digests, and rechecks bytes against their digest and shape
on each read. Replacing a generation's directory name cannot redirect a handle
to another directory; changing a referenced blob makes its next read fail.

The `configuration_stage` module binds a restore plan to that handle's exact
manifest digest, transaction, generation, Snapper config/snapshot, filesystem UUID, and declared
configuration count. It prepares file bytes, decoded access/default ACLs,
allowlisted raw labels, directory metadata, symlink targets, hard-link group
records, and explicit absences in one immutable in-memory bundle. Repeated
content and metadata digests share allocations; raw payload loading is bounded
to 64 MiB, with decoded ACL storage bounded by its corresponding raw payloads.
System-only plans and empty configuration scopes are rejected. Immutable
`state-generation` profile records remain in the durable source and are not
exposed as writable configuration targets. A changed source or mismatched plan
returns no bundle. Preparation performs no target writes or journal advances.

The plan persists the originally selected source manifest digest, separately
from the current-state checkpoint digest. Journal schema 3 requires this field
and a separate fresh `restore_id`; its filename binds that restore attempt,
so the same source checkpoint can be restored in more than one attempt.
Legacy schema-1/schema-2 journals and plans without those identities are rejected rather than
silently selecting a replacement source. Retain the current-state checkpoint
when handling an unsupported old journal. A coherently rebuilt and verified
generation with the same IDs and entry count still cannot satisfy the original
plan if its manifest has changed. Manifest validation also requires every
hard-link group to contain at least two regular records with identical content,
size, metadata, and fixed root. Invalid groups cannot be sealed or verified,
including groups linking system and user roots or singleton aliases.

This preparation does not establish authorization, compatibility, or a current
checkpoint, and an in-memory bundle is not durable `ContentStaged` evidence.
Process restart must reopen the exact durable generation and repeat preparation
against the recorded plan and manifest digest. Destructive restore must not
infer missing stage contents or advance on an unchecked caller assertion.

The `inode_metadata` module can now prepare exact metadata on caller-owned,
exclusively held regular-file and directory staging descriptors. It checks
file kind and bytes before changing metadata and refuses regular files with
existing hard-link aliases. Ownership is set before mode, mode before ACLs,
and timestamps last. Supported attributes absent from the source are removed;
unrelated user xattrs are left alone. File/directory metadata and supported
raw ACL/label values are read back after sync, so a filesystem normalization
or an incompatible ACL/mode pair cannot be reported as an exact restore.
Modification times use Euclidean division to preserve pre-epoch nanoseconds;
the existing access time is not set by this operation. All pathname resolution
and creation of protected staging inodes remain the caller's responsibility.

These primitives accept only immutable staged entries; they perform no live
target publication or journal transition. Symlink preparation uses a pinned
private parent descriptor, one fixed leaf, no-follow ownership/timestamp
operations, and Linux `l*xattr` operations. It verifies the link identity,
raw target, ownership, time, mode and supported metadata afterward; the link
target is never followed. Linux's unsupported POSIX ACL lookup on symlinks
means ACL absence, while unsupported SELinux capture still fails.
A failed inode preparation may have partially changed its
private staging inode, which must not be published. Linux tests on the WSL
host prove content/mode/time checks, pinned-descriptor behavior, access/default
ACL round trips and removal, preservation of unrelated user attributes, and
rejection of mismatched content and existing hard links. Named-user ACL tests
need the host's UID mappings rather than the restricted sandbox mapping.
Cross-owner changes and real SELinux label restoration still require privileged
Tumbleweed guest evidence; synthetic label payload verification does not prove
kernel label application.

`DiskStageStore` now materializes the immutable configuration bundle beneath a
caller-validated persistent root-owned mode-0700 directory. Fixed flat object
slots prevent manifest paths or raw symlink targets from participating in
stage path resolution. Regular files and directories receive exact metadata;
declared file aliases share one inode, symlinks retain their raw target, and
explicitly absent entries have no object. The realization uses the capture
limits: 256 paths, 16 MiB per file and 64 MiB of expanded file/target/metadata
bytes, counting reused blobs per declared path before creating a directory.

Before completion, staging syncs objects, directory entries and filesystem
metadata, then recaptures and compares their content, metadata, absences and
hard-link equivalence. Only then does it atomically publish and sync a mode-0600
receipt containing the exact plan and records. Reopening after restart requires
the same bundle reconstructed from its verified generation and persisted plan;
it checks the receipt, complete object inventory and every object again.
Missing completion, stale plans, changed bytes/metadata, broken links, extra
objects, symlink substitutions and populated absent slots fail verification.
An incomplete existing stage is never overwritten or promoted to complete;
the coordinator must remain in recovery and explicitly discard/rebuild it.

These are disk-backed stages, not live target changes. The recovery coordinator
validates the persistent mount, retains the checkpoint and source, holds the
exclusive mutation lock, and matches the verified stage plan before advancing
`ContentStaged`. The store itself does not enforce that coordination or
advance a journal. Stage files may live on a different filesystem/subvolume
from final targets; the configuration writer prepares destination-local inodes
and verifies them before publication rather than assuming cross-filesystem
renames.
The Linux tests exercise actual generation capture/publication, stage creation,
generation-based reopen and tamper rejection. A subprocess exits after each of
the five synchronized generation-publication boundaries; reopening refuses an
unindexed transaction and explicit publication reconciliation completes it. A
second subprocess exits at each of the four disk-stage creation boundaries;
only the stage with its synchronized completion receipt reopens, and create-once
refuses to overwrite incomplete stages. These model userspace process exit only.
At the coordinator level, a populated stage without a completion receipt fails
closed after retaining its verified checkpoint and journal; explicit
coordinator cleanup and retry reach `ContentStaged` with the same checkpoint
digest. The discard path refuses unknown entries, symlink substitutions,
external hard links, changed modes and populated directories, and refuses to
remove a complete stage. No SIGKILL, kernel crash, power-cut, or linked
Tumbleweed root rollback exercise is claimed by those local tests.

The `preparation` coordinator now composes the current configuration capture,
verified generation publication, journal and disk stage. It requires a fresh
independently preverified Snapper link and the caller's complete approved
current-state inventory/provenance. Every affected configuration path must be
covered before capture starts. The source is reopened and verified locally;
the actual current bytes and metadata are captured, published and verified
before their generation identity/digest enter the journal. The source system
and configuration compatibility checks remain scope-specific. If a subsequent
check fails, the new current-state generation is retained rather than deleted.

The coordinator advances only `Planned` → `CheckpointVerified` → `ContentStaged`,
after verifying their corresponding artifacts. On restart it reopens the exact
source and current-state generations, validates their recorded identities and
freshness relationship, requires the native current-state snapshot partner,
and recaptures the approved live inventory. Changed content/metadata/alias
relationships or a different inventory block readiness without recapturing
over the saved checkpoint. It checks live state again after verifying the disk
stage. An incomplete stage can be removed only through the root-only
offline-console command `sudo ryoku-recovery discard-incomplete-stage
RESTORE_ID`. The command holds the exclusive recovery mutation lock, reopens
the journal and exact verified source, and permits cleanup only while the
journal is `Planned` or `CheckpointVerified`. It removes only stage objects
matching that source and refuses completed receipts, unknown entries,
metadata/type substitutions, external hard links, or populated staged
directories. The verified current-state checkpoint and journal are retained;
rerun `prepare-configuration RESTORE_ID SOURCE_TRANSACTION_ID` (or
`prepare-linked` for the linked scope) to resume against that same checkpoint.
The command rechecks the account, source, current files, saved generation and
Snapper partner, and reconciles the catalog row only after verifying the saved
native receipt. A stage already recorded complete is reopened and verified
rather than recreated. For configuration-only attempts,
resume also reopens the stage after `WritersStopped` and later phases without
requiring the live targets to still match the pre-restore checkpoint; the
writer applies are idempotently replayed from the verified source. Initial
journal creation rejects invalid plans/schemas and every phase except `Planned`,
matching the preflight inventory bound; low-level phase persistence is not
evidence that the caller performed the corresponding native action.
The journal tests also start a child process, exit immediately after each
durable phase transition for configuration-only, system-only, and complete-
linked scopes, reopen the store in the parent, verify restart discovery, and
advance the journal to `Complete`. The root-rollback phase uses only a
synthetic test receipt. This proves durable journal resumption, not that an
interrupted native rollback or its associated system action is safe to replay.

`execute_configuration_only` connects the durable writer-stop phase to the
existing destination-local configuration writer. It requires the caller to
provide a guard that remains alive during file application and health checks,
records `ConfigurationRestored`, `HealthChecked`, and `Complete` only after
their corresponding callbacks succeed, and leaves the journal pending on any
failure. A restarted attempt can replay partially published files and retry a
failed health check. The API does not authenticate the caller or implement the
native service shutdown; those are still required before an installed helper
may expose this path.

The same preparation coordinator now has a system-only lane. It validates the
source snapshot and system compatibility before capturing current Ryoku state;
the source-only result has a distinct type that cannot be stored in a restore
journal. It then publishes and reopens the captured state generation, and only
then creates a normal restore plan that requires the verified current-state
checkpoint. The plan and checkpoint identity are recorded in a
`CheckpointVerified` journal. Resume revalidates the source plan, retained
generation, current snapshot identity, and exact live inventory. It deliberately
does not create a configuration stage or advance to `WritersStopped`; native
writer control and Snapper rollback remain responsibilities of the privileged
recovery coordinator. The current-state checkpoint must use the same
Snapper root configuration and filesystem as the selected source, with a
distinct snapshot ID.

The privileged caller must authenticate the operator, validate persistent
mounts and complete root inventory, derive native snapshot/compatibility facts,
and freeze writers from native snapshot creation through handoff to recovery.
The coordinator does not create the native Snapper checkpoint or prove the
caller observations. Local tests use actual configuration capture, generation,
stage and journal operations with synthetic preverified system-snapshot facts;
they do not prove native system snapshot creation or writer quiescence.

The configuration-only writer now restores live targets in fixture-backed
tests. The preparation API now accepts a writer-stop guard and a Snapper
rollback backend, persists `WritersStopped` before invoking rollback, records
the exact native receipt, and returns the guard with the handoff. After reboot,
it reopens the journal and source generation, checks the running Btrfs root,
target and backup identities, verifies Snapper's active/default row, and only
then records `RootRollbackComplete`. System-only attempts can run a health
check before completing. Production writer control and health checks remain
caller contracts. The CLI now creates a native current-state Snapper checkpoint
and staging journal for configuration-only attempts. Before checkpoint capture
or configuration replay, it terminates the selected account's logind sessions,
stops `user@UID.service`, and verifies that neither remains active. The common
exclusive mutation lock stays held across each operation, preventing the gated
Ryoku session from restarting before recovery state is complete. The fixed
`loginctl` and `systemctl` calls have deadlines and bounded, continuously
drained output; truncation or timeout blocks the operation. Persistent retention
enforcement and full system-rollback orchestration remain outside this path.
The CLI exposes
`sudo ryoku-recovery confirm-rollback RESTORE_ID` and
`sudo ryoku-recovery restore-configuration RESTORE_ID`. The latter verifies
the source and retained checkpoint partners, resolves the sealed local account
from the fixed account database, rechecks the booted linked target, restores
through descriptor-relative writes, validates JSON/profile data and systemd
recovery-gate health, and only then
completes the journal. A separate Rust recovery-gate endpoint now makes the Ryoku
compositor launcher refuse startup while recovery is pending or unavailable;
it does not block KDE or other sessions and still needs native socket and
pending-journal validation. No offline console restore or interruption
exercise has been recorded in a Tumbleweed VM. Issue #21 must remain open until
the destructive restore steps and required evidence are implemented and
reviewed.

`ryoku_snapper::SnapperCheckpointBackend` supplies the native single-checkpoint
operation used by root-only configuration preparation. It invokes
the fixed `/usr/bin/snapper` binary with `create --type single --read-only`,
no cleanup algorithm, and an exact transaction userdata tag. It rejects an
already tagged transaction before creation and confirms the returned nonzero
snapshot number, single type, and unique tag in a fresh inventory. Invalid
inventories, ambiguous output and command failures block success. A timeout
may leave a snapshot behind; callers must reconcile instead of automatically
retrying or deleting it. The caller must hold the mutation lock throughout.

The fixed native CLI supervisor applies its 15-second deadline until both
output pipes close and the direct child exits. It reads each pipe without
blocking, keeps at most 2 MiB stdout and 16 KiB stderr, and continues draining
excess bytes without retaining them. Stdout overflow remains an error;
stderr remains private and discarded, including overflow. No pipe-reader
threads or blocking reader joins remain. On failure it kills the process group
created for that command and reaps the direct child. Exit is observed with
`waitid(WNOWAIT)` until cleanup, preventing reuse of the group leader PID.

Rust subprocess tests cover a parent that exits while its descendant retains
stdout or stderr, and verify that the deadline returns and the owned descendant
is killed. Group cleanup covers descendants that remain in that group; it is
not containment of a descendant that deliberately calls `setsid`/changes group,
or of Snapper's separate daemon. Dropping the read pipes still avoids waiting
for such an escaped process's EOF. Spawn/kill/reap rely on normal kernel
scheduling and signal delivery, not a hard real-time guarantee for a task stuck
in uninterruptible kernel I/O. A successful command ends when the direct child
has exited and both pipes close; it does not prove that detached work finished.

This operation verifies Snapper inventory identity, not the underlying Btrfs
UUID/read-only flag, root/boot coverage, or configuration-generation linkage.
The configuration-preparation CLI performs the Btrfs checkpoint verification
and captures a linked current-state generation under the mutation lock. This
single-checkpoint path does not implement paired pre/post provider hooks or
coordinate root rollback. The read-only `SnapperCliBackend` retains disabled
pre/post mutation methods. Tests simulate command responses; they do not claim
a native checkpoint was created on this WSL host.

`native_checkpoint::verify_new_root_checkpoint` independently opens the fixed
default-root `.snapshots/<number>/snapshot` beneath a descriptor verified as
the process root, refusing symlinks. Btrfs ioctls must show matching filesystem
UUIDs, distinct nonzero root/snapshot subvolume UUIDs and IDs, the read-only
snapshot flag, and the running root UUID as snapshot parent UUID. The result
retains the snapshot descriptor and supplies a verified generation-capture
link. This check applies to a newly created checkpoint of the current root,
not historical source availability after rollback. Snapper config mapping,
transaction correlation, boot coverage and writer freezing remain independent
requirements. The checkpoint backend and verifier are wired into the root-only
configuration-preparation CLI for its current-state checkpoint; they do not
complete historical-root rollback or the full linked recovery coordinator.
Local tests cover rejected identity/flag
observations and rejection of a directory masquerading as the process root;
successful native creation and Btrfs ioctl verification are now recorded in
the [Run04 component evidence](../infra/tumbleweed-lab/evidence/issue21-run04-native-checkpoint.toml).
The first VM invocation exposed a flag-namespace bug: `GET_SUBVOL_INFO` returns
root-item flags, requiring `BTRFS_ROOT_SUBVOL_RDONLY`, rather than the similarly
named snapshot-creation ioctl flag. Correcting that check allowed verification
of the existing snapshot without creating a duplicate. A real read-write toggle
of only that disposable snapshot was rejected; restoring read-only made the
verification pass again. This component evidence does not prove full recovery.

Build the fixture-only experiment with
`cargo build --locked -p ryoku-recovery --example native_checkpoint_probe`.
Copy `target/debug/examples/native_checkpoint_probe` into the existing seeded
Tumbleweed QEMU guest after its standalone Btrfs boot and root Snapper setup,
then invoke it with guest sudo. The probe checks the lab marker and QEMU DMI
before any mutation, creates a native checkpoint tagged with the guest boot
ID, verifies its kernel identity, and exercises duplicate-transaction and
wrong-filesystem rejection. It retains the snapshot for inspection until the
disposable disk is discarded. Repeating it on the same boot fails rather than
duplicating the checkpoint. Use explicit `--verify-existing` to reconcile the
unique transaction-tagged snapshot without creating one, including after a
failed initial verification. This is a native component experiment, not a full
linked restore or checkpoint acceptance gate.

The new [configuration application path](configuration-application.md) now
publishes enrolled configuration inode types and absences through private
destination-local stages, verifies read-back, and advances only to
`ConfigurationRestored`. Durable-boundary replay, unrelated-file preservation,
metadata and cross-filesystem tests cover the writer. The recovery CLI now
coordinates its configuration-only and complete-linked lanes with offline
console checks, account writer shutdown, journal transitions and health checks.
The configuration apply writer also has a subprocess SIGKILL regression that
terminates after each of its six durable boundaries, reopens the journal, and
replays the same checkpoint; the focused test passed locally. This verifies
userspace process interruption only and does not replace a Tumbleweed restore,
reboot, kernel-crash, or power-loss exercise.
The fixture-only native linked-checkpoint probe now also applies a synthetic
configuration restore, deliberately returns a health-check failure after the
durable `ConfigurationRestored` phase, drops the prepared in-memory state,
reopens preparation from the journal, and resumes to `Complete`. The same
fixture checks a named POSIX ACL principal (UID 65534) through capture and
restore. Its VM-lab validator requires the restart and exact ACL markers. The
probe is compiled and its marker validator passes locally, but it has not run
on the pinned Tumbleweed guest; this callback interruption/reopen path does
not claim a process kill or native root rollback.
The host test
`configuration_restored_journal_reopens_after_health_failure_and_completes`
also exercises the `PreparationStores` flow directly: it injects a health
check failure after `ConfigurationRestored`, drops the prepared object,
reopens from the journal, and completes the idempotent replay. The recovery
library suite passes; this does not replace the native ACL/console probe or a
process-kill, reboot, or root-rollback exercise.
Issue #21 still needs the destructive Tumbleweed restore and interruption
exercise. The separate session gate holds the shared mutation lock only while
reading the journal and releases it before returning `clear`. This allows
supported exclusive operations while the desktop is open; those operations
must still stop affected account writers and retain the exclusive lock through
checkpoint, mutation, verification and journaling. The gate denies startup
when an operation already owns the lock or an unfinished journal exists. Root
rollback remains restricted to the offline recovery console. Native socket
activation and pending-journal runtime validation remain open; the gate is not
yet evidence for those paths. The fixed `mutation.lock` inode is provisioned
by explicit state initialization; the gate opens it read-only so its systemd
read-only state mount policy remains effective.

Restore journal transitions now depend on the selected scope. Configuration-only
recovery applies after `WritersStopped` and cannot record `RootRollbackComplete`;
complete-linked recovery still requires that phase before configuration apply.
System-only recovery skips configuration staging. The Rust apply tests use the
configuration-only phase path, and the recovery CLI now invokes the writer for
configuration-only and already-verified linked attempts. Installed-helper
authorization and native Tumbleweed execution remain unproven.

The installation integration now includes a creation-side subvolume primitive
(`state_provision`): it requires the running root descriptor and a matching
Btrfs top-level descriptor, creates only `@ryoku-state`, and derives the
expected identity from kernel read-back of that newly created volume. Existing
names are never adopted. Its generated systemd mount unit pins the numeric
subvolume ID. This is not yet an installed initializer: durable creation intent,
bootstrap checkpoint coordination, mount/unit installation and interrupted
provisioning reconciliation still need integration and native verification.

Root-side machine identity publication uses a private pending file, file sync,
no-replace rename and directory sync. Repeating publication can resume a
complete matching pending record. Incomplete, unsafe or conflicting pending
records are retained and refused; they never become the trusted startup
identity. The provisioning caller must serialize publication. This closes the
direct-to-final-file partial-write window without claiming that subvolume
creation itself is yet resumable.

`cargo build --locked -p ryoku-recovery --example native_state_provision_probe`
builds a component experiment restricted to root in the seeded QEMU fixture.
Before invoking it, the lab harness must mount the running root's Btrfs ID 5
at `/run/ryoku-lab-top`. The probe creates and verifies a tagged Snapper
checkpoint, creates the fixed state volume, checks duplicate refusal, mounts
it by its created ID, checks expected and wrong identities, initializes the
generation store, then publishes and reopens the independent machine record.
It retains all resources for inspection until the disposable disk is discarded.
The probe is built and host refusal is checked; native execution is pending.
Its receipt is experimental and does not implement durable pre-creation intent.
Boot mounting and interruption resume are explicitly reported as unproven.

The creation proof can also install its generated mount unit through
`install_mount_unit`. This checks that the supplied running root still has
the created filesystem UUID and opens each fixed systemd directory without
following symlinks. Publication checks owner, mode, link count and exact bytes,
syncs a pending file, and renames without replacing existing content. A complete
matching pending file is resumable; conflicting or partial files are retained.
This writer does not relabel, enable or start the unit, and the current native
probe does not exercise it yet. Boot persistence remains unverified.

Run07's native creation probe completed in the SELinux-enforcing Btrfs guest.
An evidence-filter omission dropped its success fields, so a separate
`--inspect-existing` invocation read the protected machine record, verified its
actual mounted partner, rejected a wrong expected identity and reopened the
generation reader without mutation. The [read-back artifact](../infra/tumbleweed-lab/evidence/issue21-run07-state-readback.toml)
records probe/log identities and these limits. Neither experiment establishes
boot persistence, interruption resume or complete offline restore.

The fixture probe also accepts `--install-boot-unit`: it verifies a new tagged
root checkpoint, saves the origin boot ID, publishes the recorded-identity mount
unit twice to check idempotence, relabels it and enables it through systemd.
This completed in Run07. `--verify-boot` refuses the original boot and, only
after a different boot ID, requires the independently recorded mount identity,
enabled and active systemd unit, and readable generation store. The post-reboot
check passed in Run07: a different boot ID, enabled/active mount unit, matching
independent identity and reopened generation reader were verified. These are
component experiments; production provisioning
intent, locking and interruption reconciliation still need integration.

The next creation probe uses `StateCreationTransaction`: a root-only guard
revalidates the native bootstrap checkpoint, holds a protected provisioning
lock, and syncs a creation intent before invoking the kernel creation operation.
It then syncs the created filesystem/subvolume UUID and ID as a separate result.
Any prior intent, complete or partial, refuses a fresh attempt and preserves
evidence for reconciliation. The guard remains alive through mount and machine
record publication. A crash between creation and result publication therefore
cannot silently become a fresh attempt or authorize adopting an unknown volume.
The local exclusive-publication regression passes; this new transaction has
not yet been executed in a VM. Automated reconciliation and coordination with
the global mutation lock remain unfinished.

`resume_created` now supports the complete-result case: it locks provisioning,
revalidates the supplied native bootstrap checkpoint, requires the exact saved
intent and transaction, and parses bounded root-owned canonical result bytes.
It compares the recorded filesystem/subvolume identity with the mounted partner
and additionally checks the kernel subvolume UUID. Only that match returns a
creation handle; no ownership or mount metadata is repaired on failure. Missing
or partial results still require explicit reconciliation. Receipt regressions
cover cross-transaction/filesystem mismatches and malformed encodings. Native
resume and process-interruption coverage remain pending.

The next creation probe includes a bounded process-death experiment after
mounting the created state volume. It releases the original provisioning guard,
starts a child that reopens the protected receipts and holds the lock, waits
for its private readiness marker, kills only that owned child, then resumes in
another fresh process. The parent also reopens the guard before publishing the
machine identity. Child readiness is bounded to 30 seconds. This experiment
builds and passes Clippy but has not run natively; Run07 predates the required
creation-intent records. Even a successful run covers only this completed
creation boundary, not every restore or provisioning interruption phase.

Run08 executed that completed-creation process-death experiment successfully;
the [component evidence](../infra/tumbleweed-lab/evidence/issue21-run08-creation-resume.toml)
records exact binaries and retained runtime-log hashes. Desktop acceptance is
intentionally skipped in this console fixture. Full lifecycle completion is
recorded only after its remaining reset and cleanup finish.

The packaged `ryoku-state-init initialize` command connects the fresh storage
path: root-only kernel preflight, an initializer-wide lock before
Snapper creation, protected config-to-root mapping checks, verified bootstrap
checkpoint, durable creation intent/result, dedicated subvolume and mount unit,
SELinux relabeling, enabled/mounted state, generation-store setup and independent
machine identity published last. It removes only its own transient top mount;
failures retain persistent resources. Healthy repeated invocation reopens the
existing recorded store without changing its identity. Incomplete attempts are
refused; automatic CLI reconciliation remains unfinished. It builds, its mapping
regression and recovery tests pass, Clippy passes, and unprivileged host execution
is refused. Run09 passed fresh initialization, healthy repeat, post-reboot
availability, and final reset/cleanup in the enforcing Tumbleweed guest;
[evidence](../infra/tumbleweed-lab/evidence/issue21-run09-initializer.toml)
records exact binary/log hashes. Package installation does not invoke the
initializer or automatically bootstrap state. The initializer does not implement
manual recovery checkpoints, restore or the global protected-mutation protocol.

Machine identity reads also use nonblocking opens so unexpected special files
can be rejected by inode-type checks without waiting for a FIFO writer. A
bounded regression verifies that FIFO refusal alongside the existing identity
publication and metadata checks. That hardening postdates the Run09 binary.

The initializer now also accepts `resume TRANSACTION_ID SNAPSHOT_ID` for a
complete creation receipt whose state volume is already mounted. It validates
the supplied snapshot against the current root, matches protected intent/result
records, reopens the matching kernel subvolume UUID under the provisioning
lock, then completes unit/store/machine-record setup. It creates no replacement
volume or checkpoint. Missing mounts, partial receipts and mismatches remain
reconciliation failures. A fresh `initialize` detects prior intent/result/lock
or pending-identity entries before creating another checkpoint, including
partial files and dangling links. Argument and pending-entry regressions pass;
the new CLI resume path is not yet exercised natively. Unmounted and ambiguous
creation windows still need explicit reconciliation rather than automatic adoption.

The next initializer build extends the complete-result case to a missing state
mount. It opens a fresh private temporary ID-5 view, compares the fixed volume
with the protected receipt's filesystem UUID, subvolume ID and UUID, and only
then creates an absent target mountpoint and continues setup. An existing
ordinary directory, foreign mount, missing result or mismatch still fails for
reconciliation. The temporary directory uses an exclusive random name, so it
does not reuse or unmount a killed attempt's old path. The lookup itself does
not change volume metadata. Recovery regressions and Clippy pass; this new
unmounted CLI branch remains unproven natively. The next creation probe also
tests receipt reopening before mounting and rejects a foreign transaction.

Run10 has passed the real initializer's fresh creation, explicit mounted-receipt
resume in a fresh CLI process, healthy repeat and availability after reboot.
The [artifact](../infra/tumbleweed-lab/evidence/issue21-run10-initializer-resume.toml)
records the staged immutable binary and private-log hashes. Its resume followed
healthy creation; it did not kill the initializer before identity publication.
The later unmounted fallback and random temporary-mount changes are not credited
to that binary. Run10 finished offline restore, fresh-baseline reset, unchanged
signed-base verification, and removal of disposable disks, keys and staged input.

`native_state_init_failure_probe` is a new fixture-only controller for the real
unmounted CLI branch. It creates complete protected intent/result records and
then requires: fresh retry refusal without another initializer checkpoint,
wrong-UUID refusal without rewriting the poisoned receipt, unchanged foreign
target data and mount-unit content, and successful CLI resume after explicit
cleanup of those known synthetic obstructions. It reopens the independent
machine record, kernel mount and generation reader afterward. The controller
builds and passes Clippy and refuses unprivileged host execution; native results
are pending. Its intentional receipt corruption belongs only to the seeded
disposable QEMU fixture and is never production reconciliation behavior.

Privileged helper startup now opens only an already-initialized generation
store. Missing staging/generation/catalog directories fail before the request
loop; startup does not create or synchronize replacement directories. The
explicit initializer retains the creation-capable constructor for its verified
transaction. A regression removes a catalog directory and confirms startup
refuses without recreating it; protected-state identity forwarding checks pass.
This storage-layout gate does not replace the unfinished restore-journal and
normal-session health gates.

Run11 passed all six controlled refusal/resume checks with the real initializer,
then reopened the expected machine identity, kernel mount and generation reader.
The [artifact](../infra/tumbleweed-lab/evidence/issue21-run11-unmounted-failure-resume.toml)
records exact staged executables and log hashes; healthy repeat also passed.
Reboot availability, bounded offline fixture restore, fresh-baseline reset and
disposable disk/key cleanup also passed. The overall console result remains
partial because desktop validation was skipped. The later helper
startup constructor change is not attributed to the Run11 binaries.

Healthy repeated initialization now also verifies the exact recorded mount-unit
bytes and its owned `local-fs.target.wants` enablement link. A live mount alone
cannot establish persistent boot setup. Missing, changed or unsafe unit/link
state is refused without recreating or repairing it. Read-only checks cover
absent files, foreign content, relative/absolute fixed link targets and writable
enablement directories; recovery tests and Clippy pass. This additional check
postdates Run11 and does not claim detection of every possible systemd override
or replace native reboot validation.

### Issue #18: code-owned enrollment and captured profile validation

`recovery::installed_capture` now compiles an immutable exact-file plan from
code-owned writer enrollments. Its public request accepts transaction facts and
fixed user/system descriptor slots; it accepts no path list, profile bytes,
owner policy, or profile revisions. The service must supply the user descriptor
from its authenticated account resolver and preserve the native snapshot-link
wrapper. An enrollment includes its writer, root, path, owner class, allowed
entry type and modes, size bound, and explicit absence policy. Compilation
rejects duplicate/escaping paths, missing writer coverage, protected generation
storage as a capture root, unsupported owner/root combinations, and ambiguous
profile layer mappings. Every enrolled entry must appear exactly once in the
captured record set, including entries that are absent.

Before publication, after payload collection and the second live capture, the
coordinator validates every record against that plan. It preserves source
UID/GID, mode, mtime, ACLs and SELinux labels; it never normalizes metadata.
Regular-only enrollments reject symlinks and hard-link groups. Explicit directory
and safe symlink enrollments describe only the exact entry, without traversal.
The four profile documents are parsed from the actual content-addressed payloads
being sealed, so a separate read cannot race with the captured profile. Unknown
schemas/fields, conflicting profile identities, missing defaults, bad digests,
and missing payloads fail before publication. Defaults → system → optional
companion → personal precedence and content-derived revisions use the existing
Rust profile parser/resolver. Missing optional documents retain absent records
and null revisions, distinguishable from present empty documents. The raw input
bytes remain in the generation, including their original formatting. No capture
step fetches a repository, consults credentials, or uses a network client.

The installed catalogue intentionally returns `NoAuditedWriters` today. This is
a remaining implementation prerequisite, not a completed #18 acceptance gate:

- `profile.json` is an opaque bounded entry in the exact session preference
  capture inventory; a valid custom selection enrolls only the referenced safe
  hero basename and its directory metadata, with no directory walk. The
  pinned upstream `theme.json`, `shell.json`, and `launcher.json` writers still
  need final metadata audit and writer quiescence integrated across native
  checkpoint/capture/mutation before that inventory can be used by installed
  operations.
- Installed defaults and system/companion/personal layer persistence still need
  reviewed local locations, privilege separation, identity/owner rules and
  immutable storage integration. No fixture path establishes that contract.
- The service must compose authenticated account-root revalidation, the complete
  enrolled writer set, validated rollback-independent store and native snapshot
  receipt. Account resolution or a generic mutation lock alone is insufficient.
- Tumbleweed runtime metadata and interruption evidence remain required. These
  temporary-fixture tests are not VM, boot, or installed-writer evidence.

The private Rust test catalogue uses synthetic `defaults.json`, `system.json`,
`companion.json`, `personal.json`, and an absent setting under a temporary root.
Those names are not production enrollments. Focused tests cover four-layer
resolution and raw-byte/metadata read-back, missing versus empty optional layers,
invalid schemas and profile identities, exact inventory omissions/substitutions,
UID/GID/mode failures, symlink policy, unavailable metadata, and mutation between
the two captures. Rejected inputs leave no catalog or generation publication.
Run `cargo test -p ryoku-recovery installed_capture --offline` for this evidence.

The next #18 persistence increment is specified in
[Local installed profile storage](local-profile-storage.md): a fixed local RPM
default loader, immutable protected system/companion bundles, raw-byte versus
semantic revisions, exact bundle IDs in sealed generation provenance, and
complete offline profile resolution. It adds no active selection, CLI/provider
mutation, or production enrollment. Package installation/audit, explicit profile
store provisioning, per-account authorization and the protected personal writer
remain required before enabling complete capture for an installed profile.
