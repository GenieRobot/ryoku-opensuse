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
filesystem UUID, subvolume ID/parent relationship, mount source/options, and
ownership/mode with the installed machine record on every startup and before
each write. Any mismatch blocks mutation and remains visible as a typed
recovery-unavailable result. This is a design requirement; no implementation
or Tumbleweed proof exists yet.

The intended on-volume layout is:

```text
/var/lib/ryoku-state/          root:root 0700, dedicated Btrfs subvolume
  machine.json                 validated filesystem/subvolume identity
  catalog/                     rebuildable index of verified manifest IDs
  generations/<generation-id>/ immutable manifest + declared payload blobs
  journal/operations.log       append-only, synchronized phase records
  staging/<transaction-id>/    private incomplete publication, never a restore source
  writer.lock                  machine-wide writer coordination
```

A transaction holds the writer lock from inventory freeze through publication.
It validates that every requested record belongs to an approved path set,
captures declared entries without following symlinks, creates the Snapper
pre-change snapshot, then writes the manifest and content into a private
staging generation. Every payload and directory is synchronized, read back,
and checked against the manifest digest and snapshot/filesystem link. The
journal is appended and synchronized before each irreversible transition.
Only then is staging atomically renamed to its immutable generation ID and the
catalog link published atomically; provider mutation begins after both are
verifiable. A crash before publication leaves staging ignored. A crash after
publication but before provider mutation leaves a recoverable, unused linked
generation. Reusing a transaction ID is idempotent; a collision with different
content is a hard error.

After root rollback, recovery reads the catalog and journal from the separately
mounted state subvolume, matches the observed root filesystem and Snapper
snapshot ID to one exact generation link, verifies all declared content and
metadata, and restores only those paths. It never guesses from timestamps,
chooses the newest generation, follows an unlisted path, or deletes a path
without a matching verified absent record. If the mount, catalog, link, or
payload fails validation, recovery refuses restore and preserves the evidence.
A configuration-only restore is a distinct labeled action and is allowed only
when its generation is compatible and fully verified; it does not claim system
rollback. This sequence still requires end-to-end implementation and VM proof.

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

### v1 record inventory and metadata rules

The manifest root is selected by the trusted caller; a record path is always
relative to that root. This table defines the v1 enrollment boundary. Entries
called *candidate* are not enabled for a Tumbleweed writer until that writer
and its on-disk result are audited. A record is never permission to discover
siblings or recurse into a directory.

| Manifest root | Candidate record path | Owner and writer contract | Entry and mode policy | Metadata policy |
|---|---|---|---|---|
| `user-ryoku-config` | `profile.json` | The target ordinary account (resolved UID and primary GID); written by a user session. The pinned upstream profile writer requests mode `0644`; this is source behavior, not measured openSUSE state. | Regular file only in v1 enrollment. Preserve and verify source mode; reject unexpected ownership or special bits. | Record size, SHA-256, UID/GID, mode, mtime, ACL digest, and present allowlisted xattr digests. |
| `user-ryoku-config` | `theme.json`, `shell.json`, `launcher.json` | Same user ownership and session writer boundary; source-backed candidate names, not proof these files are installed or used by the openSUSE UI. | Regular files only; each path must be explicitly enrolled by the feature that writes it. | Same as `profile.json`; no xattr values or file contents are copied into public inventory documentation. |
| `user-ryoku-profile` | No v1 record is approved. The upstream `profile/hero.*` path needs explicit format and size rules before enrollment. | Same resolved target account; files in this class remain user-owned. | No wildcard or directory enrollment. | Same metadata requirements if an exact file is later approved. |
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

The lower-level publisher still accepts a caller-supplied exact
digest-to-bytes map. `capture_publish_verify` builds that map only from the
records returned by the exact-file collector, reopens regular files beneath
their supplied descriptors without following symlinks or mount crossings,
checks their size/digest, repeats the capture and compares the complete
records, then invokes the publisher and verifies the transaction from disk.
Any capture, metadata, payload, or read-back failure returns an error; no
verified manifest is returned. A crash during storage publication leaves
unindexed staging or an unindexed generation, which store readers ignore.
Capture continues to fail closed on symlink entries pending race-free symlink
metadata capture. Protected-content classification, the audited multi-layer
inventory compiler, catalog reconstruction, journal/writer coordination,
mount identity validation, and crash/reboot recovery evidence remain
incomplete. Issue #18 stays open until customized multi-layer, corrupt-input,
interrupted-capture, and linked restore acceptance fixtures pass.
The synthetic end-to-end unit fixture injects a test-only no-xattr reader
because its temporary filesystem does not expose the required ACL xattrs;
production capture still uses the kernel reader and rejects unsupported
metadata.

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
symlink leaves, incomplete hard-link sets, and filesystems that cannot supply
required metadata. The `generation_capture` module accepts multiple distinct
fixed root selectors and explicit path lists, reads each regular payload
beneath its matching descriptor with no-follow/no-cross-mount resolution,
confirms the complete capture records are unchanged, then seals, publishes,
and verifies the exact generation. The caller must freeze writers and
attest an externally verified snapshot link; neither is established by this
module. Symlink target capture, a reviewed multi-root enrollment compiler,
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
`profile.json` entry beneath a caller-supplied root directory descriptor. The
loader requires matching expected UID/GID on the root and file, root mode
`0700` or `0755`, file mode `0600` or `0644`, a single-link regular file, and
at most 64 KiB. It opens with no-symlink, beneath-root, and no-cross-mount
resolution, and rejects files modified while being read. The caller remains
responsible for securely obtaining the trusted root descriptor; this API does
not discover config paths. Companion retrieval and credential handling are
outside this crate.

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
- Add race-injection and supported-filesystem ACL/SELinux capture tests; implement safe raw-target symlink capture or keep symlinks out of v1 enrolled paths. Immutable publication has focused unit tests, but a writer lock, operation journal, coordinated interrupted-transaction handling, and schema migration remain unimplemented. The current unit tests exercise capture against synthetic temporary trees; they do not prove live Tumbleweed metadata capture.
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
| Inventory Ryoku config, app/theme/shell state, provider state and profile layers | Pinned upstream paths, an explicit v1 exact-file enrollment boundary, ownership/mode policy, symlink/xattr rules and declared exclusions above; typed local profile layers | Exact Tumbleweed ownership/mode/metadata audit for integrated Ryoku paths and final package/session inventory after login support succeeds. |
| Define manifest IDs, reason/time, checksums, metadata, package/upstream/profile revisions and verification | Typed v1 manifest, canonical JSON/SHA-256 seal/verify, explicit package/provider revisions, profile revisions, metadata fields, bounded parser, and exact-file Linux capture for regular files/directories/absence | Supported-filesystem ACL/SELinux integration proof, safe symlink target capture, schema migration implementation/tests, and live Tumbleweed metadata capture. |
| Protect catalog/generations outside root rollback; validate subvolume, writer coordination and privilege split | Storage ADR specifies the dedicated subvolume, validation and user/helper responsibilities | Setup/startup validation, locking/journal/publication implementation, and linked root rollback/reboot proof on Tumbleweed. |
| Define profile precedence, absent entries and compatible migrations | Rust local resolver, strict schema-v1 local document parser/serializer, synthetic doctest, absent-file records, and explicit reject-until-migrated policy | Persistent profile/catalog storage, any future schema migration implementation/tests, and offline linked restore proof. |
| Document snapshot/boot combinations, fallback and exclusions | Scenario matrix and explicit firmware, partitioning, arbitrary-root and undeclared-home exclusions | Exercise manifest-linked rollback, corruption handling and configuration-only fallback offline. |
| ADR retains catalog, journal and matching generation across root rollback | Contract describes storage location, link identity and refusal to guess a generation | Actual dedicated mount plus Snapper rollback/reboot evidence. |
| Inventory ownership, modes, symlinks, extended metadata and external paths without traversal | Typed metadata schema; exact v1 candidate inventory; no-follow regular/directory capture, ACL/SELinux digests, absent records, hard-link checks, and symlink fail-closed behavior | Exact runtime ownership/mode/ACL/xattr evidence for integrated Ryoku state, an enforced audited allowlist, race-injection tests, and raw-target symlink capture are needed. Run19 provider paths do not constitute a Ryoku-owned path set. |
| Profile interface works locally without remotes/credentials; public examples synthetic | Pure Rust resolver, bounded v1 parser, and fixed-name no-follow loader from a supplied descriptor; synthetic four-layer doctest and temporary-file validation tests | Caller integration for securely obtaining the intended config-root descriptor and persistence/writer coordination; companion persistence/sync remains out of scope. |

## Issue #21 restore journal implementation status

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

This is a planning and journal slice only. It does not create the current-state
checkpoint, stage content, stop writers, invoke Snapper, restore targets or
metadata, coordinate the root rollback, run health checks, or gate normal session
startup. No offline console restore or interruption exercise has been recorded
in a Tumbleweed VM. Issue #21 must remain open until those destructive restore
steps and the required evidence are implemented and reviewed.
