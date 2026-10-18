# Local profile contract

This is the issue [#16](https://github.com/GenieRobot/ryoku-opensuse/issues/16)
design companion to [ADR 0002](adr/0002-linked-recovery-storage.md) and the
[managed-state inventory](recovery-state.md#managed-state-inventory). It defines
the interface and required persistence behavior; it does not enable profile
imports, application, or provider writes. Implementation claims below refer to
main commit `3e1bba8`, before this documentation change.

## Layer identity and local storage

Each setting resolves independently in this order. A missing key or JSON
`null` inherits the preceding value; it does not delete a file or reset a
lower layer. An unresolved value remains unset for the consuming capability
to validate. Profile values alone never authorize an operation.

| Order | Layer | Storage and authority contract |
|---|---|---|
| 1 | Defaults | Package-owned `/usr/share/ryoku/config/profile.json`, root:root, regular file `0644`; record exact RPM identity and revision. An installed importer must verify ownership and bytes before accepting it. Preserve the local typed input needed for offline resolution in protected profile storage. Restore of `/usr/share` belongs to the matching system/package partner. |
| 2 | System overrides | Validated local input retained in an immutable object under `/var/lib/ryoku-state/profiles/`. Publication requires the authenticated helper and machine writer lock. No arbitrary `/etc` path is admitted and no mutable external checkout is a recovery source. |
| 3 | Optional companion | Optional validated local input in the same protected store. Missing companion means no companion layer. A present empty document is distinct from absence. Import, if implemented, copies and validates bytes locally; the resolver never clones, fetches, executes hooks, or reads credentials. |
| 4 | Personal overrides | Target-account-owned `opensuse-profile.json` beneath that account's securely resolved Ryoku config root. This production filename is reserved for the port's typed format; upstream Hub's `profile.json` remains opaque upstream state. A future writer must preserve existing validated metadata, use `0600` for a newly created file, and enroll this exact path in recovery. |

The production names above are design decisions, not an assertion that those
files or packages exist on current main. The existing Rust loader opens only
`profile.json` beneath a caller-supplied directory descriptor. It must not be
pointed at the upstream Hub directory as a typed personal-profile loader.
Adapting that leaf name and wiring the trusted account-root resolver are
downstream integration work. No migration may reinterpret arbitrary Hub JSON
as the port's schema.

The helper owns the protected store root and writable directories at
`root:root 0700`. Published profile objects must be immutable, digest-addressed,
root-owned, and inaccessible to other ordinary accounts; the generation
storage policy uses `0500` directories and `0400` payload files. A reader opens
only existing committed objects. It must not create a missing store, adopt a
directory on the root filesystem, or repair permissions while reading.

A saved generation must retain the exact defaults, system and companion inputs
it used, together with the captured personal file or affirmative absence record.
Its profile revision must select that immutable local input set, and layer
revisions must match its resolved values. Semantic settings digests and raw
source digests serve different purposes: semantic digests identify typed
values, while raw digests verify retained bytes. A revision label alone is
insufficient. Retention must keep every profile object referenced by a retained
generation; an absent remote or live companion checkout is irrelevant.
Missing referenced local bytes are corruption and cannot be replaced by
current defaults or by silently dropping a previously present layer.

The service authenticates the requesting account and resolves its root using
trusted account information. Requests cannot supply an arbitrary home, UID,
root descriptor, profile-store path, or another account's selection. The
unprivileged service receives only authorized typed results; `0700` protected
storage is read by the helper and independent authenticated recovery tools.
User file metadata remains the target user's metadata when captured/restored;
protected payload ownership is not copied onto the user file.

## Local schema and examples

The current `ProfileDocument` parser accepts schema 1, at most 64 KiB, a bounded
lowercase ASCII/hyphen profile ID, and closed typed `desktop`, `theme`, and
`density` settings. Unknown fields, versions and enum values fail. Documents
cannot contain paths, scripts, commands or remote references. The existing
loader checks root/file UID and GID, root mode `0700` or `0755`, file mode
`0600` or `0644`, regular-file type and a single hard link. It rejects symlink
traversal, mount crossings and mutation during the read. Its caller must
already have securely obtained the intended root descriptor.

These are synthetic standalone schema-1 documents, one per input layer:

```json
{"schema_version":1,"id":"demo-desktop","settings":{"desktop":"hyprland","theme":"ryoku_dark","density":"comfortable"}}
```

```json
{"schema_version":1,"id":"demo-desktop","settings":{"desktop":"kde","theme":null,"density":null}}
```

```json
{"schema_version":1,"id":"demo-desktop","settings":{"desktop":null,"theme":"system","density":null}}
```

```json
{"schema_version":1,"id":"demo-desktop","settings":{"desktop":null,"theme":null,"density":"compact"}}
```

The resolved result is KDE, system theme, compact density. Omitting the third
document resolves to KDE, Ryoku dark, compact. The interface works with these
local values and no Git repository, network, credentials or private companion.
The [Rust resolver doctest](../suse/profiles/src/lib.rs) exercises both the
typed interface and the four-layer example; unit tests cover companion absence.
Public documents must never include real companion contents, names, URLs,
credentials or machine identifiers.

## Absence, capture and writer coordination

There are three distinct states: an absent optional layer, a present layer with
unset keys, and an absent declared filesystem entry. The current resolver
represents absent system/personal input as an empty typed layer and hashes that
layer; only the companion has an explicit optional revision. Consequently,
layer hashes cannot prove whether a personal file existed. Preserve that fact
in its exact-file manifest record:

```json
{"kind":"absent","path":"opensuse-profile.json","root":"user-ryoku-config"}
```

This is a proposed production inventory entry, not permission to add it to
current capture inputs. Absence requires a successful no-follow lookup showing
the enrolled entry is missing; unreadable metadata, unsafe parents, and I/O
errors are failures. A verified absent record authorizes removal only of its
exact enrolled target during the matching restore. It never authorizes recursive
directory deletion or removal of unrelated home data. A read of missing optional
profile state must not create a file merely to record defaults.

For every supported integrated Ryoku/profile write, the machine writer lock
must cover inventory selection, user/provider writer quiescence, verified
linked pre-change capture, publication, mutation and post-change accounting.
Freeze all writers of enrolled files, including upstream session writers;
holding an advisory machine lock alone does not stop a text editor or a process
that does not participate. Recheck content/metadata before publication and
before mutation. If the operation cannot establish its declared write boundary,
it must stay unavailable. The future personal writer remains unprivileged;
authorization and privileged capture happen through the narrow helper contract.

Profile activation must verify the local input set and package/schema
compatibility before obtaining mutation permission. A failed capture, changed
input, missing referenced layer or unsupported metadata prevents application.
Client closure does not cancel journal ownership or release coordination while
a protected operation is still running. This defines the requirement for
issues #17–#22; this document adds no writer or activation implementation.

## Schema migration and rollback compatibility

Manifest, catalog and profile versions are separate. Current main reads
manifest/profile schema 1; catalog schema 2 additionally binds raw metadata
payloads, while content-only generations use catalog schema 1. An older reader
must reject a version it cannot faithfully interpret. A root rollback can
therefore expose an older recovery binary to newer protected state: failure to
read it must block mutation and normal recovery rather than rewrite that state.
Keep an independently available compatible recovery environment in the tested
boot configuration.

A future migration must recognize one exact source format, validate source
bytes and metadata, and run offline under the same authorized writer boundary.
It publishes a new versioned object/generation with fresh digests and an explicit
source-to-result relationship; it never edits a sealed generation in place.
Verify complete payloads, resolved settings and the new catalog link before
changing selection. Retain the old object and every retained snapshot's
required inputs. An interrupted conversion leaves the old selection usable and
incomplete staging unselectable; conflicting results fail rather than overwrite.

Unknown, newer or ambiguous formats remain unsupported. Current main has no
version-to-version migration executor or legacy personal-file converter. A new
converter requires fixtures for supported source/target pairs, invalid and
unknown versions, interrupted publication, repeated invocation and recovery
after root rollback. Missing raw ACL/xattr payloads cannot be reconstructed from
current files or repaired by migration. Filename migration from the prototype
must recognize only its exact typed format, preserve the source and refuse a
conflicting destination; upstream Hub documents are never migration inputs.
