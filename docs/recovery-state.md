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
| Ryoku system configuration | `/etc/ryoku/` and individually declared provider paths below `/etc` | Root-owned system configuration; exact per-file ownership/mode/metadata captured. No general `/etc` traversal. |
| User Ryoku configuration | `$XDG_CONFIG_HOME/ryoku/` (or the resolved per-user config root) | User-owned; include only declared Ryoku files, with UID/GID and mode. Do not include sibling or unrelated home files. |
| Local profile selection and layers | `/var/lib/ryoku-state/profiles/` plus per-user Ryoku profile selection under the Ryoku config root | User selection stays user-owned; installed profile defaults/system overlays are immutable generations. Companion and personal material is local-only and excluded from public examples. |
| Provider / package state | Exact provider setting files and package/upstream revision records named by the operation manifest | Capture only reviewed, known provider state needed to reverse the supported operation. Package database and arbitrary provider-owned files are not recursively copied. |

The exact provider paths remain an inventory obligation for each accepted
provider operation. Any external path must include its declared root,
expected filesystem, ownership policy, and rollback method in the operation
manifest. A path outside the allowlist is not managed merely because a tool
can access it.

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

The persisted format has a required `schema_version`; unsupported major
versions or unknown required fields fail closed. Compatible readers may ignore
unknown optional fields, while writers preserve them when round-tripping.
Migrations are deterministic, local, and version-to-version; they write a new
generation and retain the prior one until verification and catalog publication
succeed. No migration fetches code or data from a Git remote.

Every transaction manifest records:

- transaction, generation, and parent-generation IDs;
- Snapper configuration and pre-change snapshot IDs, plus the filesystem UUID;
- operation reason and UTC creation time;
- package/provider identity and before/after revisions, including upstream
  source revision where applicable;
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

Synthetic shape only (all identifiers and paths below are examples; no user
or companion information is present):

```toml
schema_version = 1
transaction_id = "txn-example-0001"
generation_id = "gen-example-0007"
parent_generation_id = "gen-example-0006"
snapper_config = "root"
snapper_snapshot_id = "42"
filesystem_uuid = "00000000-0000-0000-0000-000000000000"
reason = "profile-apply"
created_utc = "2026-01-01T00:00:00Z"
package_revision = "example-rpm-nevra"
upstream_revision = "0000000000000000000000000000000000000000"
profile_id = "desktop-default"
profile_revision = "profile-v1"
verification = "verified"
manifest_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[files]]
root = "etc-ryoku"
path = "appearance.toml"
kind = "regular"
mode = 416
uid = 0
gid = 0
size = 34
sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
acl_sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"

[[files]]
root = "user-ryoku-config"
path = "local-overrides.toml"
kind = "absent"
```

The Rust `ryoku-recovery` crate now provides dependency-free validated IDs,
fixed root selectors, root-relative paths, entry-kind records, duplicate-path
rejection, and Snapper/filesystem linkage primitives. It does not read files,
serialize manifests, calculate digests, publish generations, or authorize
mutations.

The example is illustrative, not a parser-ready schema or proof of a live
capture. Canonical encoding, parser limits, checksum coverage, and migration
compatibility need implementation tests before persisted data is accepted.

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
- Add parser, canonical digest, path traversal, symlink, ACL/xattr, absent-file,
  atomic publication, writer-lock, interrupted-transaction, and schema
  migration tests. The current crate tests only its typed metadata validation.
- Exercise successful and deliberately corrupted linked generations offline,
  including configuration-only fallback and preservation of an unrelated home
  sentinel.
- Replace illustrative inventory roots with an audited list from the selected
  installed desktop, every integrated provider, and packaged Ryoku components.
- Link the live evidence and reviewed implementation commit to issue #16
  before marking it complete.
