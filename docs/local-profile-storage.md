# Local installed profile storage contract

This implements the next persistence dependency for #18, following ADR 0002.
The Rust bundle writer/loader and generation binding exist. Native capture now composes this store with authenticated account/home inputs,
RPM-verified defaults and supported writer leases. Profile activation and the
public mutation transport remain owned by #20/#39; no ApplyProfile operation
is enabled here. The authenticated user service handles ordinary local preference
reads, writes, and exact legacy migration without invoking the helper.

## Exact paths and ownership

| Location | Writer and identity | Schema, mode and absence |
|---|---|---|
| `/usr/share/ryoku/config/profile.json` | Future default-profile RPM; root:root. The Rust loader reads this fixed path, without following links. Native RPM inspection must supply the exact package name, NEVRA and upstream revision. | Required schema-1 `ProfileDocument`, single-link regular file, exactly `0644`, parent `0755`, maximum 64 KiB. Missing, malformed or unsafe defaults fail. No fallback or remote lookup. This file is never a configuration restore target. |
| `/var/lib/ryoku-state/profiles/` and `.staging/` | Authenticated helper storage on the machine-record-validated dedicated state mount; root:root. Initial creation belongs to explicit protected state provisioning. | Existing directories must be `0700`. Opening/reading does not create, adopt or repair missing directories. |
| `/var/lib/ryoku-state/profiles/<sha256>/bundle.json` | Rust `ProfileBundleStore::publish_candidate`; root:root. Immutable candidate import, with no active pointer. | Bundle schema 1, canonical JSON, maximum 512 KiB; file `0400`, directory `0500`. Name is SHA-256 of the exact canonical bundle bytes. Missing/corrupt/unsealed objects remain unavailable. No implicit latest bundle. |
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

The profile crate exposes a bounded classifier for the old Rust prototype's
exact canonical schema-1 `generic-desktop` bytes. It returns the parsed
document and source SHA-256 only when the entire input matches the old writer's
canonical encoding. The unprivileged user service uses this classifier before
calling the atomic writer; the classifier itself does not write or publish
files. Upstream Hub documents, other profile identities, reformatted JSON, and
oversized inputs are not candidates.

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

## Capture boundary and evidence

`NativeCaptureSession` validates the dedicated machine state mount and holds its
root-owned `mutation.lock` before explicitly provisioning `profiles/` and
`.staging/`. Ordinary readers never provision. Native packaged-default loading
verifies the exact fixed path with RPM ownership, digest, metadata and NEVRA;
callers cannot supply package facts. Import publishes an immutable candidate,
never an active selector. Capture binds that candidate, the authenticated account
principal and the collected personal payload into one verified generation.

The compiled exact-file inventory, bounded unknown-managed-state refusal and
account writer protocol are described in
[writer coordination](issue18-writer-coordination.md). All supported launch
routes must use that protocol. Hand editing through an external editor is not a
supported integrated writer; concurrent source changes are independently caught
by double capture/read-back, and are never normalized into a new profile.

Focused tests cover original bytes versus semantic revisions, optional layers,
fixed RPM loading, immutable publication, stale stages, metadata/path rejection,
account/bundle binding, writer loss before publication and offline reproduction
with live personal state removed. Package installation and full desktop/boot
acceptance are separate #23/#25 gates. This storage primitive neither implements
#20's durable transaction state machine nor grants a client bundle selection or
mutation authority.
