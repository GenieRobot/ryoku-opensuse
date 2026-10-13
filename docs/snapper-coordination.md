# Snapper coordination primitives

This is a partial implementation for [issue #19](https://github.com/GenieRobot/ryoku-opensuse/issues/19). The `ryoku-snapper` crate provides pure layout classification, validated identifiers, a bounded read-only Snapper CLI backend, exact transaction-ID pre/post pair correlation, and in-memory catalog validity and retention-pin primitives.

## Behavior implemented

- `FullSystem` classifies only root and boot snapshot coverage: it requires a Btrfs root subvolume, a ready root Snapper config, and boot content that is either inside root or independently known to be snapshotted. It does not assert that a Ryoku generation exists or is linked. A separate unsnapshotted `/boot` cannot be reported as full-system protected.
- Configuration-only capability is reported when the Ryoku configuration target itself is confirmed as Btrfs with a ready Snapper config. Unsupported or unknown observations never produce full-system capability.
- Package-hook pairs match only on exact config and transaction ID. One matching pre snapshot is reported as incomplete; duplicate matching pre or post records are ambiguous. Description text and timestamps are not correlation keys.
- Pure Rust pair reconciliation reuses an exact existing package-hook pair, permits creation only when no exact pair exists, and requires explicit reconciliation for incomplete or ambiguous matches. `reconcile_pair` classifies supplied observations without querying or mutating Snapper.
- `query_pair` is a generic read-only runtime boundary: it asks a `SnapperBackend` for one config-scoped inventory, rejects foreign-config rows and duplicate snapshot IDs, then applies exact transaction correlation. It never calls either creation method.
- `SnapperCliBackend` now provides a concrete read-only list implementation. It invokes only fixed `/usr/bin/snapper` with `--jsonout --config <validated-name> list`, has no root/path override, clears the inherited environment, applies a 15-second deadline and bounded output capture, and returns sanitized error categories. It parses only the selected top-level config array and typed snapshot IDs/types. Pair correlation uses only explicit `userdata["ryoku-transaction-id"]`; descriptions are ignored. The adapter's create operations fail closed without invoking Snapper.
- Catalog completeness separately requires root/boot capability, a verified system snapshot partner, and a caller-confirmed present and verified Ryoku generation partner. Duplicate transaction IDs are rejected. Manual and known-working retention pins can only be set on complete entries.
- `CatalogStore` persists a versioned, bounded catalog relative to an already-open directory descriptor. It requires the expected owner and exact private directory/file modes, uses a fixed filename, flock plus a process mutex, no-follow descriptor-relative reads, a SHA-256 payload checksum, and temp-file `fsync` + atomic rename + directory `fsync`. It rejects unknown schema fields, invalid identifiers, tampered/truncated documents, oversized files, and duplicate transaction IDs. The checksum detects corruption but is not a keyed authenticator; the store cannot prove the directory is outside rollback scope, and the caller must provide and protect the correct persistent location.
- Snapper values are typed and bounded. `SnapperBackend` describes typed list/pre/post operations; the CLI backend implements list and explicitly rejects pre/post creation.

## Remaining acceptance work

This crate does **not** complete issue #19. The following work and evidence remain:

1. Implement and review a read-only detector for supported openSUSE Btrfs, Snapper config, mount, and boot layouts; confirm every observation against real supported Tumbleweed fixtures. This crate currently classifies supplied observations only.
2. Verify the read-only CLI adapter and its output against the supported guest's Snapper version, and integrate an authorized mutation helper only after its identity and authorization are established. The adapter currently requires explicit `ryoku-transaction-id` userdata; stock zypp hook records do not prove that this key is present. `query_pair` validates the backend inventory boundary, but there is not yet an authorized create implementation.
3. Integrate `CatalogStore` with the privileged service and establish the directory's persistent mount, owner, and exclusion from every root rollback scope. Add authenticated integrity protection if the threat model requires detection of a same-owner writer who can recompute the checksum. Prove catalog and journal survival across root rollback and reject tampered or truncated state on the supported guest.
4. Connect catalog entries to #18's generation store and verify both partners before claiming complete status; recheck partner existence after cleanup and rollback.
5. Implement and test retention ownership so manual checkpoints and the known-working generation are protected and Snapper cleanup cannot orphan catalog links.
6. Represent and demonstrate all required root/additional snapshot IDs without duplicate pairs, invalidation when either partner is missing, and configuration-only/unsupported behavior on actual layouts. This primitive catalog currently records one root snapshot ID per entry.

No VM, Snapper command, system mutation, protected-mount placement, or rollback claim was used to validate this primitive slice. Unit tests exercise descriptor-relative durable replacement in a temporary directory. Keep #19 open until its listed acceptance evidence is attached.
