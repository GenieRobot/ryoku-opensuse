# Managed state and profile recovery contract

Status: M0 design contract for issue [#16](https://github.com/GenieRobot/ryoku-opensuse/issues/16).
This is an implementation target, not evidence that dedicated storage or a
boot recovery path has been exercised on Tumbleweed.

## Decision: linked generations outside root rollback

The durable catalog, operation journal, and immutable Ryoku generation store
belong on a dedicated Btrfs subvolume mounted at `/var/lib/ryoku-state`. The
subvolume must be a direct child of the Tumbleweed filesystem's top-level
subvolume and must not be nested beneath the Snapper-managed root subvolume.
The root Snapper configuration must exclude this mount from its snapshot
content. Installation and every startup must verify filesystem UUID, Btrfs
subvolume identity, mount source and options, ownership, mode, and that the
mount is active before permitting a managed write. Missing, substituted,
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

The unprivileged service owns request validation and catalog reads. A narrow
authenticated privileged helper performs the bounded capture/restore and
provider writes. The helper accepts a transaction ID and validated manifest
reference, not caller-supplied paths or shell text. A per-machine lock and
generation state machine serialize writers; repeated transaction IDs are
idempotent, and an interrupted operation remains journaled for explicit
recovery. Readers may inspect committed generations while a writer is active,
but cannot observe partially published generations. The journal is appended
and synchronized before each irreversible phase transition.

This design preserves Ryoku state across a root rollback; it does not promise
to preserve the state subvolume across disk loss, repartitioning, filesystem
replacement, or a malicious root administrator. Backups to another device are
outside this contract. Native Btrfs/Snapper restore compatibility, early mount
ordering, and failure behavior must be demonstrated in the disposable VM
before implementation can claim recovery support.

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
| Catalog and operation journal | `/var/lib/ryoku-state/catalog/`, `journal/` | Root-owned helper writes; outside root Snapper rollback; include in a separate durable state backup policy. |
| Immutable configuration generations | `/var/lib/ryoku-state/generations/<id>/` | Root-owned, content-addressed, read-only after publish; includes only paths explicitly declared by a generation manifest. |
| Proposed Ryoku system configuration | `/etc/ryoku/` is an openSUSE namespace proposal; provider files below `/etc` are not yet enumerated | No Tumbleweed provider path is currently authorized for recovery. Do not interpret this proposed namespace as a discovered or audited installed path. |
| User Ryoku configuration | `$XDG_CONFIG_HOME/ryoku/` (or the resolved per-user config root) | User-owned; include only declared Ryoku files, with UID/GID and mode. Do not include sibling or unrelated home files. |
| Local profile selection and layers | `/var/lib/ryoku-state/profiles/` plus per-user Ryoku profile selection under the Ryoku config root | User selection stays user-owned; installed profile defaults/system overlays are immutable generations. Companion and personal material is local-only and excluded from public examples. |
| Provider / package state | No exact Tumbleweed provider setting path is currently approved; package/upstream revisions may be recorded as manifest metadata | Provider writes remain unsupported pending #15 hook proof. Package databases and provider-owned files are not recovery inputs. |

Any future provider operation must first name and audit each affected path,
including package owner, writer, filesystem, ownership/mode/metadata, and
rollback behavior. A path outside a reviewed operation allowlist is not
managed merely because a tool can access it.

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
| `$XDG_CONFIG_HOME/ryoku/` (default `~/.config/ryoku/`), only individually declared preference and profile files | Ordinary-user owned; written by the Ryoku UI/config writer. Upstream examples include `hypr.json`, `shell.json`, `launcher.json`, `hub.toml`, `profile.json`, `profile/hero.*`, and sparse `user_edits/` files. | Capture only allowlisted paths. Preserve owner, mode, symlink target and supported ACL/xattrs. Unknown files and directories are not recursively included. The `hero.*` and sparse overlay paths require explicit type/size rules before enabling them. |
| `$XDG_STATE_HOME/ryoku/` (default `~/.local/state/ryoku/`) | User-owned upstream CLI operational data; examples include `manifest.json`, `materialized`, `release-cache/`, `provisioned/`, `migrations/`, and `repo`. | Excluded from first-sprint configuration generations. It can contain cache or provision receipts; no blanket copy. Add individual records only if a supported operation needs them and defines format and privacy. |
| `$XDG_DATA_HOME/ryoku/plugins/` (default `~/.local/share/ryoku/plugins/`) | User-owned plugin payloads; upstream tracks plugin receipts/state separately. | Excluded from first-sprint recovery. Plugin payloads, secrets and executable content need separate review. |
| `$XDG_CACHE_HOME/ryoku/` (default `~/.cache/ryoku/`) | User cache, when an integrated component creates it. | Excluded and rebuildable; never use as a recovery source. |
| `$XDG_CONFIG_HOME/hypr/user.lua`, `$XDG_CONFIG_HOME/kitty/user.conf`, `$XDG_CONFIG_HOME/fish/user.fish` | User-owned raw overrides named by upstream `docs/config-import.md`; not necessarily managed by the openSUSE port. | Preserve as user data unless a specific import feature is implemented and the file is explicitly enrolled. Package reinstall must not replace them. |
| `/var/lib/ryoku-state/catalog/`, `journal/`, `generations/`, and `profiles/` | Root-owned openSUSE storage internals on the separately validated state subvolume; written by the authenticated helper. | These are protected store internals, not user/system file records in a transaction manifest. Their integrity/publication rules are defined above. |
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
| `$XDG_CONFIG_HOME/ryoku/profile.json` and `profile/hero.<ext>` | Profile UI writes JSON and copies a selected custom hero into `profile/`; both are user-owned. The hero extension and size are not fixed by the upstream contract. | The metadata file is a candidate declared file. A hero image needs explicit format/size rules before capture; no directory walk. |
| `$XDG_CONFIG_HOME/hypr/{monitors.lua,gpu.lua,keyboard.lua,user.lua}` and optional `monitors_user.lua`; generated `settings.lua`/`rebinds.lua`; mirrored generated overlay files `ryoku/user_edits/hypr/{settings.lua,rebinds.lua}`; user-editable `modules/` | The pinned provider interface (`ryoku/cli/vendor/ryoku-wm/detect.go`) distinguishes generated seeds, user-owned files, generated outputs and hand-edit escape hatches. | Not a current Tumbleweed provider set. If a compatible provider is later supported, list only specific files in that operation. |
| `$XDG_CONFIG_HOME/niri/{monitors.kdl,gpu.kdl,keyboard.kdl,user.kdl,monitors_user.kdl}`; generated `settings.kdl`/`rebinds.kdl`; mirrored generated overlay files `ryoku/user_edits/niri/{settings.kdl,rebinds.kdl}` | Same pinned provider interface; niri's `monitors_user.kdl` is a required seeded include. | Not a current Tumbleweed provider set. No arbitrary `niri/` capture. |
| `$XDG_CONFIG_HOME/ryoku/user_edits/<relative-path>` | Sparse user-authored forks. Upstream enumerates regular files, skipping symlinks, `.md` files, and nested `ryoku/user_edits/`; the overlay mirrors paths into `~/.config`. | Preserve only a future explicitly enrolled file. Upstream's recursive overlay behavior is not adopted as this project's capture policy. |
| `$XDG_STATE_HOME/ryoku/{repo,deployed,materialized,doctor-report.txt}` | CLI operational state at default `~/.local/state/ryoku`; upstream CLI reference identifies checkout, deploy revision, materialization hash manifest, and latest report. Other features have independent state beneath this root. | Excluded from configuration generations. Do not copy this directory or rely on these Arch-updater records for restore. |
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

The schema has a required `schema_version`; the current typed writer accepts
version 1 only and rejects any other version before serialization. The v1
compatibility rule is strict: a future reader must reject unknown fields rather
than silently discard data. Optionality applies only to known v1 fields, where
an absent value means `None`. Any field-set or semantic change requires an
explicit local version-to-version migration. A migration writes a new
generation and retains the prior one until verification and catalog
publication succeed; it never fetches code or data from a Git remote. The
reader/parser and migration executor are not implemented yet.

Version 1 canonical encoding is compact UTF-8 JSON. JSON object keys are sorted
lexically, file records are sorted by fixed root name and relative path, and
the SHA-256 digest covers this canonical document with `manifest_sha256`
omitted. The sealed document includes that digest. The Rust recovery crate
validates v1 schema bounds before producing this canonical byte representation,
then seals and verifies the digest when callers construct the typed value. It
still has no parser,
filesystem collector, durable writer, or generation publisher, so this is not
yet a persistent-storage or capture guarantee.

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
        "mtime_unix_ns": 0,
        "uid": 0
      },
      "path": "appearance.toml",
      "root": "etc-ryoku",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "size": 34
    },
    { "kind": "absent", "path": "local-overrides.toml", "root": "user-ryoku-config" }
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
Snapper/filesystem linkage primitives, canonical JSON serialization and
manifest digest verification. It does not read files, acquire metadata,
publish generations, or authorize mutations.

The typed contract also names operation reason/time, per-package identity and
before/after NEVRA, provider identity and before/after revision, upstream source
revision provenance, all four resolved profile layers, per-phase verification,
the canonical manifest digest slot, hard-link groups, ACL/xattr digests, and
explicit capture exclusions. Its initial xattr enum is limited to SELinux
labels and POSIX access/default ACLs; these values are represented by digests.
The fixed file roots are `/etc/ryoku`, resolved per-user Ryoku config/profile
roots, and the generation namespace. Catalog, journal, plugin payloads,
package databases, caches, and arbitrary home/provider files are not file
records. This code is a validated data model only: it does not acquire metadata
or guarantee that a future collector enforces every declared rule. Unsupported
metadata must still fail capture at that boundary.

The example is illustrative, not a parser-ready schema or proof of a live
capture. Tests cover v1 schema bounds, canonical encoding, digest
sealing/verification, and stable file/package ordering. Parser limits and
version migration still need implementation tests before persisted data is
accepted.

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
| Root Snapper rollback with valid linked generation | Keep catalog, journal, and matching immutable generation available; restore exact declared state. | Design only; dedicated state mount and linked restore remain unverified. |
| Root rollback with missing/corrupt generation or catalog link | Refuse silent fallback; report recovery unavailable/inconsistent and preserve evidence. | Design only. |
| Offline boot and recovery | Use local catalog, generation, and supported system tools; do not require a Git host, credentials, or companion. | Existing VM lab evidence covers offline recovery fixture mechanics; not this manifest-backed profile store. |
| Configuration-only recovery where compatible Snapper state is absent | Offer only a clearly labeled configuration restore when its generation and file metadata verify; never claim system rollback. | Design only. |
| Btrfs root plus Snapper snapshots | Supported target when root and state subvolumes are separately validated and the restore order is implemented. | Tumbleweed subvolume exploration exists; Ryoku state subvolume linkage not yet validated. |
| Non-Btrfs root, unknown boot layout, firmware, partitioning, or arbitrary root edits | Out of supported restore scope; block integrated system mutation or require manual recovery. | Explicit exclusion. |
| Unrelated home content, secrets, external media, and undeclared provider files | Never traverse, capture, overwrite, or delete. | Contract; needs adversarial path and metadata tests. |

This contract does not expand Snapper coverage to firmware/EFI variables,
partition tables, bootloader installation, arbitrary root-owned state,
unrelated home files, or external devices. Boot recovery must be independently
authenticated and must validate that the linked generation is compatible
with the observed root snapshot before restore.

## Remaining proof before acceptance

- Implement the dedicated subvolume setup/validation and prove it survives
  actual Snapper root rollback and reboot on the disposable Tumbleweed VM.
- Add parser, path traversal, symlink, ACL/xattr, absent-file, atomic
  publication, writer-lock, interrupted-transaction, and schema migration
  tests. The current crate has no parser or filesystem collector, and its
  metadata tests do not exercise live filesystem capture.
- Exercise successful and deliberately corrupted linked generations offline,
  including configuration-only fallback and preservation of an unrelated home
  sentinel.
- Replace illustrative inventory roots with an audited list from the selected
  installed desktop, every integrated provider, and packaged Ryoku components.
- Link the live evidence and reviewed implementation commit to issue #16
  before marking it complete.

## Issue #16 checklist reconciliation

This matrix reports repository evidence without treating an ADR or synthetic
example as a live recovery result. Every issue checkbox remains open until the
corresponding implementation and disposable-guest evidence are linked.

| Issue #16 item | Current repository evidence | Still needed |
|---|---|---|
| Inventory Ryoku config, app/theme/shell state, provider state and profile layers | Pinned upstream path tables and exclusions above; typed local profile layers | Run19's exact Tumbleweed package/path ownership and metadata findings; final integrated Ryoku package/session inventory after login support succeeds. |
| Define manifest IDs, reason/time, checksums, metadata, package/upstream/profile revisions and verification | Typed v1 manifest, canonical JSON/SHA-256 seal/verify, explicit package/provider revisions, profile revisions, metadata fields and validation tests | Parser and schema migration implementation; metadata acquisition and filesystem capture tests. |
| Protect catalog/generations outside root rollback; validate subvolume, writer coordination and privilege split | Storage ADR specifies the dedicated subvolume, validation and user/helper responsibilities | Setup/startup validation, locking/journal/publication implementation, and linked root rollback/reboot proof on Tumbleweed. |
| Define profile precedence, absent entries and compatible migrations | Rust local resolver plus synthetic doctest; absent-file records; strict v1 compatibility/migration rules | Persistent profile/catalog storage and migration implementation; offline linked restore proof. |
| Document snapshot/boot combinations, fallback and exclusions | Scenario matrix and explicit firmware, partitioning, arbitrary-root and undeclared-home exclusions | Exercise manifest-linked rollback, corruption handling and configuration-only fallback offline. |
| ADR retains catalog, journal and matching generation across root rollback | Contract describes storage location, link identity and refusal to guess a generation | Actual dedicated mount plus Snapper rollback/reboot evidence. |
| Inventory ownership, modes, symlinks, extended metadata and external paths without traversal | Typed metadata schema, pinned Ryoku source inventory and no-recursive-traversal rules | Exact Tumbleweed runtime ownership/mode/ACL/xattr evidence from Run19; audited allowlist and collector behavior tests. |
| Profile interface works locally without remotes/credentials; public examples synthetic | Pure Rust resolver has no filesystem/network capability; synthetic four-layer doctest | None for the resolver contract; companion persistence/sync remains out of scope. |
