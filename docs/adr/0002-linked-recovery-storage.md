# ADR 0002: Linked recovery storage and profile state

- **Status:** Accepted design; runtime implementation and Tumbleweed proof are
  incomplete.
- **Issue:** [#16 (M0-07)](https://github.com/GenieRobot/ryoku-opensuse/issues/16)
- **Date:** 2026-10-14

## Context

A Snapper root rollback can restore system files without restoring Ryoku's
user-owned preferences or profile selection. Keeping the only copy of the
manifest catalog or generation under the Snapper root would also make the
link needed for recovery disappear with that rollback. An ordinary user's
home directory cannot be recursively captured or treated as privileged
state. Recovery must work without a Git remote, credentials, or private
companion data.

## Decision

Keep the catalog, append-only operation journal, immutable configuration
generations, and installed profile layers under a dedicated Btrfs subvolume
mounted at `/var/lib/ryoku-state`. It must be a direct child of the Btrfs
top-level subvolume, outside the Snapper-managed root subvolume, and excluded
from root snapshot content. Store the expected filesystem UUID and subvolume
identity in root-owned `/etc/ryoku/state-machine.json` so a root rollback
restores the expected identity record while leaving the state subvolume and
its catalog available.

At installation, startup, and before each managed write, validate the mount
descriptor and observed filesystem UUID, Btrfs subvolume ID and parent,
mount source and options, ownership, and mode against the installed record.
An absent, substituted, read-only, or incorrectly nested mount disables
managed writes and reports recovery unavailable. Never create a fallback
state directory on the root filesystem. The expected-identity record is
provisioned only from trusted subvolume-creation facts; ordinary service and
user processes cannot rewrite it.

The root-owned authenticated helper is the only writer of protected state.
The unprivileged Rust service validates requests and reads typed catalog
results. A single per-machine writer lock spans declared-inventory freeze,
Snapper pre-snapshot linkage, generation publication, and provider mutation.
The helper accepts validated transaction and manifest references, never
caller-selected paths or commands. Journal phase records are synchronized
before irreversible transitions. Published generations are immutable and
readers see only committed catalog links. This design does not claim that
the lock coordinates unrelated system tools or provider-internal writers;
those boundaries must be proven before a provider is enabled.

Each transaction link binds one transaction ID to one generation ID and
sealed manifest digest. The manifest binds the exact declared file set to a
Snapper configuration/snapshot ID and filesystem UUID. Recovery verifies
both partners and all declared payloads before restoring. A missing or
corrupt link is an error; recovery never chooses a generation by timestamp,
newest-first ordering, or a similar-looking snapshot. A configuration-only
restore is separately labeled and permitted only when the exact generation
and its compatibility evidence verify. It must not claim a system rollback.

User profiles resolve in fixed precedence: defaults, system overrides,
optional local companion settings, then personal overrides. The generic
Rust resolver consumes typed local values and never fetches remote content.
Absent keys inherit lower layers; an absent companion is valid. The local
schema rejects unknown versions and fields until an explicit local migration
exists. Private companion content and identifiers are not stored in public
examples or source.

The [local profile contract](../local-profile-contract.md) defines storage
ownership, the distinct port/upstream profile filenames, exact local input
retention, absence semantics and migration publication. All supported integrated
profile writes retain the verified linked pre-change requirement from issue #1.
The machine lock must remain held through provider mutation and post-change
accounting, including coordination with every writer of the enrolled user files.
An advisory lock without that coordination does not establish a frozen inventory.

## Supported scope and compatibility assumptions

| Observed system state | Recovery scope | Contract |
|---|---|---|
| Supported Btrfs root, ready Snapper root configuration, validated separate state subvolume, exact matching snapshot and generation | Complete linked recovery | Restore only after verifying the root snapshot link, generation seal, payloads, and compatibility. This is the target configuration; live Tumbleweed rollback/reboot proof is pending. |
| Validated state subvolume and compatible generation, but no usable matching system snapshot | Configuration-only recovery | May restore the exact declared configuration generation with a clear label; does not restore system state. Runtime proof is pending. |
| Supported root snapshot but missing, corrupt, or mismatched state subvolume/catalog/generation | No automatic restore | Fail closed, preserve evidence, and report recovery unavailable or inconsistent. |
| Non-Btrfs root, unknown or unsnapshotted boot layout, mismatched filesystem identity, or read-only/substituted state mount | No integrated mutation; manual recovery | Do not infer protection or silently fall back to another storage location. |
| Firmware/EFI variables, partition tables, arbitrary root edits, unrelated home files, external media, plugin payloads, caches, or unreviewed provider files | Excluded | Never enumerate or modify them as part of a Ryoku generation. |

### Snapshot and boot combinations

No complete boot-recovery combination is release-validated by this ADR. The
initial target is the pinned x86_64 Tumbleweed Btrfs/Snapper root with GRUB2 UEFI;
the [WSL QEMU fixture](../../infra/tumbleweed-lab/wsl-fixture.toml) records exact
image, kernel, bootloader and subvolume inputs. Its `/home` and `/var` exclusions
explain why a root snapshot alone is insufficient. Separate state initialization
and availability after reboot are evidenced by
[Run11](../../infra/tumbleweed-lab/evidence/issue21-run11-unmounted-failure-resume.toml);
that run does not prove linked Ryoku root rollback or independent boot recovery.

| Root / boot layout | Eligibility and required proof |
|---|---|
| Btrfs root, Snapper `root` mapping, GRUB2 UEFI, kernel/initramfs and required `/boot` files inside the captured root | Initial linked-recovery target. Verify actual filesystem/subvolume identity, boot artifacts, exact snapshot/generation link, independent authenticated recovery entry and reboot into the selected root. A successful snapshot command or GRUB entry alone is insufficient. |
| Same root with a separate ESP, firmware variables or bootloader installation outside the snapshot | Those components remain outside linked coverage. Only operations whose declared write set leaves them unchanged can be eligible. Changes to ESP/firmware/bootloader installation require a separately reviewed recovery mechanism and are unavailable under this contract. |
| Separate unsnapshotted `/boot`, systemd-boot/UKI, Limine, legacy BIOS, encrypted or multi-device layout not in the tested matrix | No integrated system mutation or advertised complete boot rollback until that exact layout has a reviewed coverage map and VM evidence. Do not extrapolate from GRUB2 UEFI or Btrfs presence. |
| Read-only boot of a historical Snapper snapshot | Inspection/recovery environment only. Establish the exact selected snapshot and compatible local recovery reader; do not treat mounting it or observing the same filesystem UUID as completed rollback. Supported promotion to a writable root and subsequent health checks need separate proof. |
| Compatible generation but missing matching system snapshot | Explicit configuration-only recovery may restore only declared configuration after compatibility and payload checks. It must obtain the required verified current-state checkpoint before a destructive write; if that cannot be obtained, offer inspection/manual recovery only. Never downgrade a requested linked restore automatically. |
| Root snapshot predates state-machine record or has incompatible recovery tools | The state subvolume may still exist, but ordinary startup must refuse adoption. An independently authenticated compatible recovery environment must validate original provisioning evidence; it cannot recreate or trust identity from the observed mount alone. |

Kernel/initramfs compatibility, package and provider revisions, the actual
selected boot root and profile schema are separate checks. Restoring root files
does not restore running processes or the currently loaded kernel. Completion
requires the prescribed reboot and health checks with the matching user
generation restored before normal Ryoku writers resume. Snapper retention must
not delete either partner of a retained linked point, and profile retention must
keep its referenced local inputs. Reconciliation reports missing partners; it
does not substitute another snapshot or generation.

Compatibility means that the saved profile schema is supported and the
required package, upstream, provider, and profile revisions are locally
available and explicitly accepted by the recovery operation. Labels in a
manifest alone are not compatibility evidence. Unknown schema versions are
rejected; migration must be an explicit local conversion that publishes a new
generation and retains the old generation until the new one is verified.

## Consequences

- Root rollback retains the catalog, journal, and generation only if the
  separate mount is correctly provisioned and excluded from root snapshots.
- A verified catalog link is necessary but not sufficient for a restore; the
  target system and every exact payload must also verify.
- Per-user files remain owned by the target user and are restored only from
  explicit manifest records. Unrelated home content is outside the operation.
- Offline recovery uses local protected state and installed tools. Companion
  settings are optional and cannot block recovery.
- Disk loss, repartitioning, filesystem replacement, malicious root access,
  and external backups are outside this decision.
- This is a design decision, not proof that subvolume setup, mount validation,
  journaling, restore, or root rollback currently works on Tumbleweed.

## Required evidence before claiming support

Provision and validate the dedicated subvolume on the exact supported
Tumbleweed image; show that root rollback and reboot retain the catalog and
matching generation; exercise unavailable/corrupt linkage, configuration-only
restore, offline behavior, and preservation of an unrelated home sentinel.
Record the exact image, kernel, Btrfs/Snapper versions, boot layout, and
resulting package/profile compatibility matrix. Until then, recovery is not
enabled for managed mutations.

See [the recovery-state contract](../recovery-state.md) for the versioned
manifest, exact-file inventory, metadata rules, and current implementation
status.
