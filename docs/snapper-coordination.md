# Snapper coordination primitives

This is a partial implementation for [issue #19](https://github.com/GenieRobot/ryoku-opensuse/issues/19). The `ryoku-snapper` crate provides pure layout classification, validated identifiers, a typed backend interface, exact transaction-ID pre/post pair correlation, and in-memory catalog validity and retention-pin primitives.

## Behavior implemented

- `FullSystem` classifies only root and boot snapshot coverage: it requires a Btrfs root subvolume, a ready root Snapper config, and boot content that is either inside root or independently known to be snapshotted. It does not assert that a Ryoku generation exists or is linked. A separate unsnapshotted `/boot` cannot be reported as full-system protected.
- Configuration-only capability is reported when the Ryoku configuration target itself is confirmed as Btrfs with a ready Snapper config. Unsupported or unknown observations never produce full-system capability.
- Package-hook pairs match only on exact config and transaction ID. One matching pre snapshot is reported as incomplete; duplicate matching pre or post records are ambiguous. Description text and timestamps are not correlation keys.
- Catalog completeness separately requires root/boot capability, a verified system snapshot partner, and a caller-confirmed present and verified Ryoku generation partner. Duplicate transaction IDs are rejected. Manual and known-working retention pins can only be set on complete entries.
- Snapper values are typed and bounded. `SnapperBackend` describes list/pre/post operations without selecting a command, D-Bus method, or concrete executor.

## Remaining acceptance work

This crate does **not** complete issue #19. The following work and evidence remain:

1. Implement and review a read-only detector for supported openSUSE Btrfs, Snapper config, mount, and boot layouts; confirm every observation against real supported Tumbleweed fixtures. This crate currently classifies supplied observations only.
2. Implement a concrete authorized Snapper adapter and verify exact pre/post metadata, transaction IDs, config names, errors, and duplicate-hook behavior on the supported guest. The current trait is only an interface.
3. Implement the catalog and journal as durable, authenticated, integrity-checked storage outside the root rollback scope. Prove their survival across root rollback and reject tampered or truncated state.
4. Connect catalog entries to #18's generation store and verify both partners before claiming complete status; recheck partner existence after cleanup and rollback.
5. Implement and test retention ownership so manual checkpoints and the known-working generation are protected and Snapper cleanup cannot orphan catalog links.
6. Represent and demonstrate all required root/additional snapshot IDs without duplicate pairs, invalidation when either partner is missing, and configuration-only/unsupported behavior on actual layouts. This primitive catalog currently records one root snapshot ID per entry.

No VM, Snapper command, system mutation, durable-write claim, or rollback claim was used to validate this primitive slice. Keep #19 open until its listed acceptance evidence is attached.
