# Local installed profile storage contract

This implements the next persistence dependency for #18, following ADR 0002.
The Rust bundle writer/loader, generation binding, generic RPM payload and
read-only installed ownership verifier exist. Run16 validates native RPM
build/install/query and inactive bundle storage on the disposable Tumbleweed
guest; see [the native evidence](../infra/tumbleweed-lab/evidence/issue18-run16-native-bundles.toml).
The authenticated unprivileged service reads and atomically writes the fixed
personal settings document. Ordinary personal settings do not require a
machine checkpoint or elevation; the exact file remains in the recovery
inventory and is captured by later linked recovery operations. Exact canonical
legacy personal-profile migration is also available through the same-user
writer, preserving the source and requiring no machine checkpoint. System and
companion profile imports remain disabled. The checkpointed personal-profile
coordinator below is a separate protected path and is not connected to ordinary
personal setting updates. Full installed inventory, profile activation and
per-account imported bundle selection remain unfinished; this is not an
enabled ApplyProfile operation or completion of #18/#39.

## Exact paths and ownership

| Location | Writer and identity | Schema, mode and absence |
|---|---|---|
| `/usr/share/ryoku/config/profile.json` | `ryoku-profile-defaults` noarch RPM; root:root. The Rust loader reads this fixed path without following links and verifies its exact installed RPM owner/NEVRA, file digest, size and metadata. The document is port-owned, so no upstream revision is asserted. | Required schema-1 `ProfileDocument`, single-link regular file, exactly `0644`, parent `0755`, maximum 64 KiB. Missing, malformed or unsafe defaults fail. No fallback or remote lookup. This file is never a configuration restore target. |
| `/var/lib/ryoku-state/profiles/` and `.staging/` | Authenticated lifecycle storage on the machine-record-validated dedicated state subvolume; root:root. Fresh `ryoku-state-init` provisioning creates it under the verified bootstrap checkpoint and state-creation lock. Resume accepts only the exact empty layout from an interrupted bootstrap. `initialize` now validates this store before reporting an existing state mount available, but does not create or repair it. | Existing directories must be `0700`. Opening/reading does not create or repair missing directories. Populated or partial layouts require reconciliation; existing initialized state stores are not upgraded by `initialize`. |
| `/var/lib/ryoku-state/profiles/<sha256>/bundle.json` | Rust `ProfileBundleStore::publish_candidate`; root:root. Production publication requires a borrowed `StateMountProfileLifecycleGuard`; service capture and recovery checkpoint-session derive it from their already-held exclusive `mutation.lock` descriptor. Immutable candidate import, with no active pointer. | Bundle schema 1, canonical JSON, maximum 512 KiB; file `0400`, directory `0500`. Name is SHA-256 of the exact canonical bundle bytes. The guard and native state/store descriptors are revalidated around durable staging, rename, and sealing. Missing/corrupt/unsealed objects remain unavailable. No implicit latest bundle. |
| `~/.config/ryoku/opensuse-profile.json` | Read and atomically updated by the authenticated unprivileged Rust daemon using an NSS/kernel-peer-bound account home and fixed relative directory/leaf names. A personal setting update uses no helper or machine checkpoint. If `.config` or `ryoku` is absent, reads return built-in defaults without creating directories; the first explicit save creates only those missing directories at `0700`, after no-follow, same-mount and owner checks. Legacy migration accepts only the old exact canonical schema-1 Rust document, atomically creates this file without replacing an existing one, and preserves the source. This port-owned name is distinct from upstream Hub's `~/.config/ryoku/profile.json`. | Schema-1 `ProfileDocument`, `generic-desktop` identity. An absent file reads as built-in defaults without creating it; the first save uses `0600`, while updates preserve the validated existing mode and supported metadata. Persistent state remains in the recovery inventory and this exact file is the scoped restore target. Corrupt, unsafe or foreign-identity documents are preserved and refused. |

The preserved upstream Hub owns `~/.config/ryoku/profile.json` for its own
profile and hero-selection document. The earlier Rust prototype used that same
filename for a different strict schema; the port uses the distinct
`opensuse-profile.json` path and never parses or rewrites the upstream file.
`ryoku-profile show` reads the typed personal profile from the local user
service and prints its bounded JSON document. It uses only the installed
service and fixed local socket; it does not contact a Git remote or require
network credentials. The service returns built-in defaults when the personal
file is absent and does not create a file during the read. Its typed
`SetLocalProfile` request atomically replaces only the validated personal
settings document. This is a same-user preference write, not a system profile
activation, and it does not create a machine checkpoint or operation journal
entry.

`ryoku-profile set` provides a local CLI for the same fixed settings document.
It accepts one bounded JSON `SettingsLayer`; all three fields are supplied,
and `null` means that the personal layer does not override that setting:

```console
ryoku-profile set '{"desktop":"kde","theme":"system","density":null}'
```

The command sends typed values through the authenticated user-service socket.
Unknown fields, paths, scripts, commands, and unsupported enum values are
rejected before any write. The result is the saved profile document. Run
`ryoku-profile show` first when you want to preserve other personal values.

The `ryoku-profile migrate-legacy` command imports only the exact canonical
schema-1 document emitted by the old Rust prototype. It uses the same-user
atomic writer, leaves the source document untouched, and never overwrites an
existing destination. A retry succeeds only if that destination contains the
exact imported document; a conflicting or unsafe destination is refused. This
is a local personal-settings migration; it invokes no privileged helper and
creates no machine checkpoint. The upstream Hub hero/profile document remains
opaque and is never a candidate.

Run the command from the account whose preferences should be migrated, while
the Ryoku user service is available:

```console
ryoku-profile show
ryoku-profile migrate-legacy
```

The command requires matching non-root real/effective UID and GID, takes no
paths or preference values from its arguments, and prints only a result or a
bounded generic error. It does not change the running upstream Hub profile.

The exact session-preference capture inventory records the upstream
`profile.json` bytes opaquely, with an explicit absence record and a 1 MiB bound.
For a valid custom selection, capture reads only its `hero.<lowercase-extension>`
basename beneath `profile/`, enrolls that exact regular file (up to 16 MiB),
and records the directory metadata. It never enumerates the directory. Unsafe
or changing selections fail capture; default and gallery selections do not
enroll user assets. This document remains separate from the port-owned profile
resolver. Installed capture and restore still require the complete
writer-coordination path.

The profile crate exposes a bounded classifier for the old Rust prototype's
exact canonical schema-1 `generic-desktop` bytes. It returns the parsed
document and source SHA-256 only when the entire input matches the old writer's
canonical encoding. The unprivileged user service uses this classifier before
calling the atomic writer; the classifier itself does not write or publish
files. Upstream Hub documents, other profile identities, reformatted JSON, and
oversized inputs are not candidates.

The root-only personal-profile coordinator remains the intended path for
protected profile imports that change packaged defaults, system overrides or
selected recovery bundles. It is not connected to service requests. Ordinary
personal settings use the local user-owned writer above; legacy migration and
protected profile activation remain unavailable.

System and companion layers have no mutable files under `/etc` or an arbitrary
user-selected root. A local authorized importer supplies validated document
bytes and publishes a new immutable bundle containing the required defaults
and optional system/companion inputs. Each uses the same bounded
`ProfileDocument` schema and profile identity. Unknown keys, schemas, paths,
commands and remote references are rejected by the existing typed parser.
Absent system or companion data is normal; a present empty document is distinct.
Companion fetch/sync and credentials are outside this contract.

The store is protected machine state. An unprivileged requester cannot list or
select another account's bundle merely by knowing its digest. Per-account
selection authorization is a required future service gate; the store primitive
itself is not an authorization API and has no transport endpoint.

## Content, metadata and publication

A bundle retains the original JSON text and SHA-256 digest for each local layer,
plus exact default-package provenance. It also derives semantic layer revisions
from canonical typed settings. These identities serve different purposes:
whitespace changes alter a raw-source digest and the enclosing bundle ID, while
the semantic settings revision stays equal. Source bytes are private protected
payloads. Validation errors contain no source text or profile values.

The packaged default copy inside a protected bundle is provenance for offline
resolution. It neither becomes a manifest file record nor authorizes writing
`/usr/share`; complete system restore still needs the matching package/system
partner. Personal metadata belongs to the normal exact-file generation and is
preserved without normalization. Protected bundle modes are storage policy,
not replacement metadata for a user file.

Publication first validates bounded canonical content and available space. It
creates a new transaction-named staging directory exclusively, writes and
synchronizes `bundle.json`, seals the file, and verifies content and directory
membership. A no-replace rename publishes the object, then the directory is
sealed and synchronized. Readers reject the brief unsealed state and any
crash-left unsealed object. Stale staging or an existing conflicting/corrupt
object requires reconciliation; the writer never repairs or overwrites it.
An already published exact valid object can be reused idempotently.

Only fixed native machine-record/mount discovery constructs the production
store. Every operation rechecks that mount and directory association. A verified
bundle handle reopens the exact digest-named entry and checks its inode as well
as content. Holding a descriptor to a renamed/deleted bundle is insufficient.
Cleanup and transaction writers must share the machine writer lock. No garbage
collector exists; a bundle referenced by a retained generation must remain
available, including after root rollback.

## Capture and offline resolution

`capture_with_installed_profile_bundle` accepts a verified selected bundle and
a code-owned enrollment. It checks exact default-package provenance, verifies
the bundle before capture and again just before generation publication, and
resolves the captured personal payload against its local layers. The manifest's
existing opaque `profile_revision` slot binds the bundle using
`bundle-sha256-<64 lowercase hexadecimal digits>`; the other revision slots bind
the semantic layer inputs and preserve null for absent optional layers. No
manifest schema change or arbitrary path encoding is required.

`ProfileBundleStore::resolve_generation` reopens that exact bundle, checks the
saved package association, reads the personal payload from a verified
generation, resolves it offline, and compares every provenance field. Missing
bundle data fails without trying a live default, a current profile, another
bundle, or a companion checkout. Callers must compose this result with native
snapshot and generation verification before reporting complete availability;
the lower-level generation verifier alone does not verify external bundles.

## Disabled personal-profile production scope

`PersonalProfileCoordinator` composes the actual fixed-path components for one
supported writer. Its production constructor authorizes the kernel peer before
NSS lookup, pins the existing account configuration root, and opens the
machine-record-validated protected state mount. It takes no caller path, UID,
profile bundle ID or source inventory. It requires already provisioned profile
storage; neither it nor a reader creates missing account/profile directories.

The caller acquires the existing #20 `MutationLock` at the fixed protected-state
`mutation.lock` once and holds its guard through preparation and the concrete
`execute` call, including both captures and health checks. Guard checks compare the current lock inode without recursive
flock or silently recreating a missing lock. Account, mount and namespace
checks remain required alongside the advisory lock. External editors and RPM
tools do not participate in this exclusion; repeated reads reject observed
changes but do not claim to freeze those tools.

Preparation validates the existing personal schema and the three native typed
settings (`desktop`, `theme`, `density`), then verifies the actual RPM defaults.
This first production scope supports only generic packaged defaults plus the
account's optional personal layer. System and companion imports and arbitrary
bundle selection are unavailable. An absent personal file resolves against
packaged defaults without being created. A future authorized first change
creates it; missing `.config/ryoku` bootstrap remains outside this scope.

Before any candidate publication or snapshot creation, `RootCaptureMapping`
requires the secure original `/etc/snapper/configs/root` record and effective
`/usr/bin/snapper --jsonout --config root get-config` result to agree on
`SUBVOLUME="/"` and `FSTYPE="btrfs"`. The effective JSON contract is Snapper's
[flat string-valued configuration object](https://github.com/openSUSE/snapper/blob/master/client/snapper/cmd-get-config.cc).
Duplicate keys, unavailable queries, unsafe/replaced config files and changed
configuration values fail. The actual root and `/.snapshots` must be distinct
Btrfs subvolumes on the validated filesystem; their namespace, mount and kernel
identities remain pinned. A nonrecursive native inventory is bounded to 4,096
directory entries, including ignored nonnumeric names. Numeric aliases and
unsafe numeric entries fail. The returned snapshot number must have been absent
from this native baseline, independently of Snapper's config-scoped inventory.
This prevents a misdirected config's new number from selecting an older root
snapshot with the same number and current-root ancestry. Config and namespace
checks repeat before create, after create, before sealing and before the private
writer; native UUID/ancestry and transaction-tag checks remain required.

Capture publishes/reuses the exact immutable defaults bundle, creates a native
Snapper checkpoint while holding the common guard, captures every record in
`InstalledInventory::personal_profile_writer()` (exactly `opensuse-profile.json`, present
or absent), seals the authenticated account principal and bundle provenance,
and reopens the persisted generation, native receipt and bundle. RPM raw bytes
and package facts, account identity, source metadata/content and the native
receipt are checked again before generation publication. A successful result
is an opaque `VerifiedPersonalProfileCheckpoint`, specifically for this scope;
it is not complete Ryoku state or evidence for unrelated writes. It has no
conversion to #20's generic checkpoint evidence. The full
`InstalledInventory::current()` remains unavailable.

The crate-private writer binds the desired canonical bytes to that checkpoint.
It repeats the exact source, native receipt, RPM and bundle checks before
replacement, writes an exclusive transaction-named staging file, verifies and
synchronizes it, then atomically renames it and synchronizes the directory.
Existing UID/GID, mode, POSIX ACL and SELinux label are reproduced from the
verified preimage payloads; the modified file gets its new write time. Unknown
attributes, foreign ownership, hard links, symlinks and unsupported modes fail.
First creation explicitly applies `0600` even under a parent default ACL, so
inherited named entries have no effective group-class access. For first creation, a bounded fixed `/usr/sbin/matchpathcon -N -n -m file`
query resolves raw policy for the exact NSS-derived final path. The held staged
inode must already have precisely that label; missing policy/tool/label or any
mismatch fails without relabeling. Both output pipes are capped at 4 KiB and
the child process group has a two-second deadline. Account association is
revalidated around the query and again before rename. This uses the
[upstream raw-context/type query contract](https://github.com/SELinuxProject/selinux/blob/main/libselinux/utils/matchpathcon.c).
Existing-file updates replay the verified source label. First creation
uses no-replace rename. Failed staging is retained and never
adopted or truncated on retry. A failure after rename must be recorded as an
Applying failure, not as proof that nothing changed.

`PersonalProfileCoordinator::execute` now binds this writer to the exact private
plan and opaque preimage, synchronizes Applying in the fixed root-owned machine
journal before writer entry, and verifies the actual resulting file, ownership,
mode, ACL and SELinux label. It rechecks account/RPM/bundle/native facts and
captures a fresh opaque postimage whose manifest names the preimage generation
as parent. It returns only after both references are durably recorded as
Complete. Cancellation, low-space preflight and failure/restart rules are
described in [mutation coordination](mutation-coordination.md). This concrete
composition does not enable transport dispatch or solve account-writer access.

No service/helper dispatch invokes this writer or capture coordinator. The
installed helper also has `ProtectHome=read-only` and an empty capability
bounding set: this root-only building block cannot write account state inside
that unit. Future integration needs an authenticated handoff to an account
writer or a separately reviewed worker with narrowly scoped access, without
globally relaxing helper protection. Native
Snapper capture, privileged metadata replay, root rollback and post-change
capture for this slice have not been validated end to end. Temporary-fixture
tests exercise the same private write primitive and guard hooks; they do not
construct the opaque native checkpoint or substitute synthetic native proof.

## Remaining integration and evidence

1. Extend profile-store provisioning from bootstrap into authenticated
   lifecycle handling for existing initialized state stores. A concrete
   root-only `StateMountProfileLifecycleGuard` now pins and holds the common
   exclusive mutation lock and rejects incomplete restore journals before
   provisioning. Wire this guard into an explicit initialized-store lifecycle
   operation with the required checkpoint policy; this API is not yet exposed
   through the installer or helper.
2. Validate the concrete #20 call chain natively, including failure/restart,
   cancellation and fresh post-change capture. Implement an authenticated
   account-writer boundary and explicit authorized reconciliation. Retain one
   common guard throughout; installed dispatch remains disabled.
3. Add separately checkpointed account-directory bootstrap, authorized local
   imports and per-account bundle selection if supported. Enroll each actual
   additional writer and its exact state; the scoped personal inventory cannot
   be presented as the full installed inventory.
4. Compose bundle verification into recovery/availability and retention, and
   validate personal capture/write/restore on Tumbleweed. Run16 proves the
   installed RPM and inactive bundle component only, not this new writer,
   scoped checkpoint, SELinux final-path policy comparison, activation or
   full-generation acceptance. Native writer prerequisites additionally include
   `matchpathcon` (Run17 guest preflight identified its installed provider as
   `selinux-tools-3.11-2.1.x86_64`), loaded file-context policy for the
   authenticated home, and permission to query inode labels. The writer
   checker's fixed `/usr/sbin/matchpathcon` invocation still needs native
   end-to-end verification; package discovery alone does not establish it.

Focused Rust tests exercise exact bytes and semantic revisions, optional layers,
fixed-path package loading, atomic immutable publication, failed/stale stages,
symlink and foreign-owner rejection, rename/replacement of a pinned bundle,
package/provenance binding before publication, and offline profile reproduction
from a saved generation with live personal state removed. Run
`cargo test -p ryoku-recovery installed_ --offline`.

## Preconditions for a future native personal capture/write test

These are additional code/runtime preconditions, not requests to install more
host packages. The existing inactive-bundle probe does not exercise this path.

- Build the reviewed production-scope code and a noninstalled, guarded fixture
  that calls the concrete `execute` API. It must obtain a real ordinary-user peer
  from a kernel-authenticated Unix connection; a hand-built identity is not
  authentication evidence. No current service dispatch exposes this operation.
- Use only the seeded disposable guest and a dedicated opted-in
  `ryoku-operators` account with NSS identity and existing account-owned home,
  `.config`, and `.config/ryoku` directories, each `0700`, `0750` or `0755`.
  The configuration parents must share the expected mount; no symlink or
  environment-selected root is accepted. Exercise both absent and existing
  valid schema-1 personal state, with exact UID/primary GID and `0600`/`0644`.
- Initialize and validate the dedicated Btrfs state mount and generation store,
  then explicitly provision fresh root:root `0700` `profiles/` and `.staging/`
  under trusted lifecycle exclusion. Readers never repair them. Retain the
  existing common `mutation.lock` guard throughout capture and write, with no
  external account, RPM, Snapper-config or namespace writers during the test.
- Install the frozen generic-default RPM in the guest with native RPM digest
  checks; `/usr/bin/rpm` must prove its exact fixed payload and ownership.
  Require `/usr/bin/snapper`, its effective root/Btrfs config and working normal
  daemon/backend connection, plus a writable, bounded `/.snapshots` namespace
  on the expected root filesystem. The mapping checks described above and
  native snapshot receipt must pass independently.
- Require enforcing SELinux with readable policy and inode labels, and the
  tested `selinux-tools` package providing the fixed-path `matchpathcon` query.
  First creation requires the naturally created staged label to equal the
  final-path policy label. Existing-file tests require permission to replay
  exact UID/GID, POSIX ACL and SELinux metadata. Also require supported
  `openat2`, `statx`, Btrfs ioctls, xattrs, fsync and atomic rename behavior.
- The root coordinator requires effective UID/GID 0. The installed helper's
  `ProtectHome=read-only` and empty capability set cannot provide the needed
  account-write boundary. A reviewed fixture execution context may demonstrate
  the primitive, but production acceptance still requires a narrowly scoped
  account-writer handoff or worker, durable #20 Applying/failure/post-capture
  composition, and native restore evidence. Do not relax the helper unit or
  present direct fixture execution as installed activation.

## Guarded native bundle probe

The noninstalled Rust example `native_profile_bundle_probe --run` exercises
the real fixed-path RPM loader and native state-store APIs only in the existing
seeded disposable QEMU fixture. It requires effective UID/GID 0, exact
`/etc/ryoku-lab-fixture` content `disposable-issue13\n`, and QEMU DMI identity.
It refuses a pre-existing `profiles` directory, including partial previous
evidence. It does not install defaults, initialize/mount state, create snapshots,
activate a profile, enroll a writer, invoke the helper, or write a user home.

The probe publishes three inactive bundles with actual packaged defaults and
synthetic local optional layers. It closes and reopens the store by exact ID,
checks original source bytes, resolves four layers offline, distinguishes absent
from present-empty optional inputs, and verifies rejection of moved/replaced
directories and deliberately corrupted content. This is component evidence;
it does not establish complete-generation or restore/activation acceptance.

The source probe now opens and holds the production `mutation.lock` under
nonblocking exclusive `flock`; it refuses to proceed if the lock is unsafe,
replaced, or any restore journal is incomplete. This serializes it with
cooperating Ryoku writers. The prior Run16 evidence predates this change and
used a fixture-only `.profile-probe.lock`, so it does not prove production
writer exclusion; rerun the probe on the exact disposable Tumbleweed guest
before crediting this updated path. Before publication and cleanup the probe
revalidates its pinned native state and profile namespaces. Cleanup first
checks the entire expected set, metadata, exact inodes and bytes, including the
original receipt for intentionally corrupted content. It unlinks only those
files and then their empty directories, never recursively deletes a path.
Failure before cleanup leaves evidence; a cleanup failure stops immediately
and retains remaining evidence for inspection.

| Environment | Required tools and preconditions |
|---|---|
| Host, defaults RPM build | `rpm-build` (`rpmbuild`), RPM macro/build dependencies, Git and gzip for the exact source archive. This data RPM needs no Rust build or vendored Cargo archive. Host installation into the RPM database is not required. |
| Host, probe build | Workspace Rust/Cargo 1.99 or newer, native linker/C toolchain, and cached dependencies from `Cargo.lock`. Build with `CARGO_BUILD_JOBS=2 cargo build -p ryoku-recovery --example native_profile_bundle_probe --locked --offline`. The resulting ELF must match the guest architecture and runtime libraries. |
| Disposable guest, defaults | `rpm` providing `/usr/bin/rpm` and the installed `ryoku-profile-defaults` package. Exact owner, metadata, SHA-256 and size must match `/usr/share/ryoku/config/profile.json`; the root-owned directory is `0755`, the single-link regular file `0644`. Missing or modified inputs fail. |
| Disposable guest, state | Previously initialized `/etc/ryoku/state-machine.json` (root `0600`, parent root `0700`) and writable dedicated Btrfs `/var/lib/ryoku-state` (root `0700`) matching its recorded filesystem/subvolume/parent identity. Kernel `openat2`, `statx`, Btrfs read-only identity ioctls and `/proc/self/mountinfo` must be available. `profiles/` must be absent. |
| Disposable guest, execution | Seeded QEMU identity, effective UID/GID 0, no external state/RPM writers, and compatible executable/runtime. The probe itself invokes only fixed `/usr/bin/rpm`; it does not require a companion, network, `findmnt`, `btrfs` CLI or Snapper CLI. |

Fixture validation commands (do not execute the native `--run` path on a host):

```console
CARGO_BUILD_JOBS=2 cargo test -p ryoku-recovery provisioning_ --offline
CARGO_BUILD_JOBS=2 cargo test -p ryoku-recovery --example native_profile_bundle_probe --offline
```
