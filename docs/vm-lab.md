# Disposable Tumbleweed cloud lab (#13)

The selected resource is the existing managed cloud development workspace. No
paid infrastructure, provider identity, private companion repository, or local
user disk is needed. The cloud status API reports connected/running with an
enforced HTTP allowlist, no advertised VM capabilities, no credentials and no
outbound identities. Actual QEMU execution must establish VM support separately.
`/dev/kvm` is absent; the lab uses TCG software emulation.

The existing `/workspace/opensuse-vm` resource supplies QEMU 10.0.13, OVMF
2025.02, the official signed Tumbleweed 20261011 x86_64 XFS cloud base and its retained
signed checksum/release keyring. `infra/tumbleweed-lab/fixture.toml` records exact
image and firmware identifiers; `runner.toml` records the resource inventory.
The orchestrator checks the image and firmware checksums and exact release signer before
creation. The moving download alias is provenance only: a different image must
fail, even when its signature is valid. Preserve the recorded base to reproduce
this run; never relabel a later snapshot with this checksum.

## Runner and execution

Use the existing authorized cloud workspace with x86_64 Linux, at least 6 GiB
available RAM, 12 GiB free disk, Rust 1.99.0, SSH tools, GPG, sha256sum and the
extracted QEMU/genisoimage assets. The run directory must be a fresh absolute
path beneath `/workspace`, separate from the asset directory. The authorization
environment variable is an operator declaration; it cannot prove cloud location
by itself. The recorded run uses the actual managed cloud executor.

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
cargo deny check
RYOKU_LAB_AUTHORIZED=existing-cloud-workspace cargo run --locked --bin vm-lab -- run /workspace/opensuse-vm /workspace/ryoku-lab-FRESH-NAME
```

Custom orchestration and assertions are Rust under `automation/src/lab/`.
No shell script or embedded custom command implementation is generated.
NoCloud metadata contains a fresh public SSH key, public CA trust, and native
upstream service commands. The private key stays inside the run directory, whose
mode is 0700. Host SSH binds to loopback; guest passwords and root login are
disabled. Fixture-only sudo access uses its disposable key and must never be
installed on a supported user system.

GitHub hosted CI is unavailable due to the funding limitation reported by the
user. Runner inventory returned `Forbidden`; no registered self-hosted runner
has been established. `vm-lab.yml` is an explicit manual contract for an existing
authorized runner carrying `ryoku-disposable-vm`, and creates no resources. Its
presence is not evidence that a GitHub run occurred. Use the cloud command above
for validation. Registering a funded runner is a separate resource action.

## Acceptance sequence and limits

The Rust command verifies the signed XFS base, creates a fresh overlay, a second
blank migration disk and firmware variables, generates a fresh key/NoCloud ISO,
and boots the source guest. It installs required native Btrfs/Snapper/EFI tools,
creates GPT/ESP and Btrfs on the blank second disk, copies the source root with
native rsync archive/hardlink/ACL/xattr preservation, regenerates its initramfs
and installs GRUB2 in that disk's ESP. Root inode and mount-directory metadata
are included; source/target ownership, mode, numeric ACL and security context
comparisons must match before boot. It
then cleanly stops the source guest and boots the derived disk as the sole disk,
with fresh firmware variables. Root uses default subvolume `@`; distinct nested
home and var subvolumes preserve unrelated home files and persistent journal
storage across root snapshots. Root fstab follows the default subvolume, while
home and var explicitly mount their persistent subvolumes. The signed source
base stays unchanged. Every
guest mutation requires an explicit lab marker and QEMU DMI identity.
It inspects x86_64, exact guest snapshot, Btrfs, GRUB2 and Snapper, then attempts
required KDE/SDDM provisioning and checks an active display manager, recording
exact installed versions. A
reboot must change the guest boot identity and pass inspection again.
The minimal image's running kernel is `7.2.9-1-default`, owned by
`kernel-default-base` RPM `7.2.9-1.1.36.10`; the kernel image's actual RPM owner
is checked against the declarative fixture, rather than assuming the full
`kernel-default` package. GRUB2 and its Snapper plugin are `2.14-20.1`, and
Snapper is `0.13.1-3.4` in the observed fixture.
The fixture pins GRUB2's package version; the main, EFI and Snapper-plugin RPMs
must each match `2.14-20.1` during every base inspection.

The offline phase restarts the same guest with QEMU user networking restricted;
loopback SSH remains available. A TCP connection to the existing external proxy
must fail before recovery is accepted. The guest creates a real root Snapper
snapshot, corrupts a lab-owned file, restores that file from the snapshot and
requires matching bytes. It then removes the marker and the snapshot. This
proves the lab can exercise controlled corruption and offline Snapper recovery.
Full root rollback, boot-menu snapshot selection, production linked Ryoku/home
state recovery and desktop login are later issue acceptance gates; this fixture
test does not establish those behaviors.

Reset stops the guest cleanly, discards its disk and firmware variables, boots
another fresh base overlay and requires both marker absence and absence of the
installed KDE package. It then stops again and deletes disks, keys and seed
metadata; a final image checksum confirms the retained base was unchanged.
Deleting only a test file is insufficient reset evidence. Package absence requires
RPM's exact exit status 1 and not-installed response over a successful SSH
connection. Clean stop requires QEMU's successful exit status; an abnormal
hypervisor exit fails the run.
Recovery verifies the derived Btrfs root and retained Snapper configuration
immediately before disk discard. Reset must return to the signed source's XFS
root with that Snapper configuration absent. These state changes remain required
when KDE provisioning was blocked; marker/KDE absence alone is insufficient.
A distinct `/etc/ryoku-lab-reset-sentinel` is exclusively created after recovery
with exactly `retained fixture reset state\n` (29 bytes), then the file and parent
`/etc` are fsynced and the bytes read back. The sentinel remains until disk
discard. Fresh-source reset requires its absence. All reset absence checks use
native `symlink_metadata`, accepting only `NotFound`; existing entries including
dangling symlinks and metadata errors fail. Sanitized evidence records sentinel
presence, byte match, length and sync success before discard, then absence after
fresh XFS boot.

If KDE provisioning fails, the command preserves its private diagnostics and
continues the independently available base VM checks. It still returns nonzero
and writes `lab_result=blocked` after reset. This provides partial recovery and
runner evidence and never satisfies KDE fallback or closes #13.

## Current KDE download blocker

The full recommended closure and a retry with native `--no-recommends` both
selected 750 packages and failed with Zypper exit 4 on
`libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`, HTTP 403. Hard dependencies,
signatures and TLS were preserved. The retry did not remove the codec requirement.

The allowed official URL
`https://codecs.opensuse.org/openh264/openSUSE_Tumbleweed/x86_64/libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`
returns HTTP 302 to
`https://ciscobinary.openh264.org/libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`.
A HEAD request on 12 October 2026 verified that redirect without following it.
`ciscobinary.openh264.org` is absent from the enforced cloud allowlist. The minimum
resource action is authorization of that host through the supported environment
configuration workflow, or an accessible, supported Tumbleweed KDE fixture.
No direct-route/proxy bypass, fake codec, signature override or solver override
was attempted. KDE fallback acceptance remains incomplete until its real package
closure and display-manager check pass.

The first derived boot (run5) exposed a copy defect: copying only the root's
children omitted its inode security metadata. Both roots had UID/GID 0 and mode
0755; source SELinux context was `system_u:object_r:root_t:s0`, while the target
was `unlabeled_t`. The library and parent-directory contexts matched. Source
SELinux enforcement was 1. The failed serial log and nonzero result were retained
privately. Native rsync now copies root/mount-directory metadata and checks
parity. SELinux and service checks remain enabled.

Run6 then booted the sole derived Btrfs disk and accepted SSH, but normal
D-Bus Snapper configuration failed writing `/etc/sysconfig/snapper`. Native
`matchpathcon -V` identified type `etc_t` where installed policy required
`snapperd_conf_t`; ownership/mode were root:root 0644. A read-only source
comparison showed the same wrong label had been created during package
provisioning, and rsync had preserved it. Targeted native `restorecon` corrected
the label; normal D-Bus create-config/list then passed with SELinux enforcement
still 1. No AVC was available from the retained journal/audit query, so that
query alone does not establish the denial's source. The lab now normalizes only
this configuration file to installed policy before copy, compares its metadata,
and requires policy verification and enforcing SELinux after boot. Snapper
operations continue through their ordinary D-Bus interface; no `--no-dbus`
workaround or permission relaxation is used.

## Artifacts and interrupted runs

Only `evidence.txt` may be published. It contains a strict ASCII key/value
allowlist, fixture checksums, package versions and checked results. Never upload
raw serial/QEMU/SSH logs, cloud-init metadata, host keys, guest disks, firmware
variables or the whole run directory. Private diagnostic files stay under 0700
and may contain machine identifiers or network details.

Normal completion leaves sanitized evidence and private diagnostics. Failure
returns nonzero and terminates only the QEMU child owned by that process; it
retains the disposable directory for diagnosis. If the process is forcibly
terminated, stop its exact QEMU instance before deleting its run directory.
Never run a wildcard process kill or delete the shared asset directory. Re-run
using a new directory after repairs. No incomplete evidence is a passing run.

## Recorded run

Run11 completed on 12 October 2026 in the existing managed cloud workspace.
[Sanitized VM evidence](../infra/tumbleweed-lab/evidence/issue13-run11.txt) contains
the checked fixture identifiers and lifecycle results. The Rust sources and
fixture in this change produced the tested debug executable SHA256
`8476ea6f2ead253e9d1c25a416e4791df87a5c6aa3832e05d201e45e48d8475b`.

The actual commands used the existing cloud toolchain:

```console
PATH=/workspace/.cargo/bin:$PATH RUSTUP_HOME=/workspace/.rustup CARGO_HOME=/workspace/.cargo cargo build --locked --bin vm-lab
RYOKU_LAB_AUTHORIZED=existing-cloud-workspace ./target/debug/vm-lab run /workspace/opensuse-vm /workspace/ryoku-lab-issue13-run11
```

The build returned 0. The VM command returned **1**, with `lab_result=blocked`:
KDE's native `--no-recommends` transaction selected 743 new packages and failed
during preloading with HTTP 403 on the OpenH264 RPM and Zypper exit 4. No RPM
installation phase was shown in that transaction's log. Required KDE fallback
and full #13 completion remain blocked by the origin access described above.

| Independently verified gate | Run11 result |
| --- | --- |
| Signed immutable image and pinned firmware/QEMU | Passed |
| Fresh cloud source creation/boot and Btrfs derivation | Passed |
| Sole Btrfs disk UEFI/GRUB2 boot and exact RPM-owner inspection | Passed |
| Enforcing SELinux, native labels and normal D-Bus Snapper listing | Passed |
| Changed boot identity and repeated strict inspection after reboot | Passed |
| Restricted egress, actual corruption and offline Snapper byte restoration | Passed |
| Derived Btrfs/Snapper state replaced with fresh XFS/no root config | Passed |
| Exclusive sentinel present with matched 29 bytes and file/parent fsync before discard | Passed |
| Dedicated sentinel absent after fresh XFS boot, only `NotFound` accepted | Passed |
| Successful QEMU exit on every normal stop | Passed; exit status 0 required |
| Disposable disks, firmware, seeds and SSH keys removed | Passed |
| Original signed image checksum unchanged | Passed |
| KDE fallback | Blocked; HTTP 403/Zypper 4 |

The completed run directory retains only the sanitized evidence and private
diagnostic logs, with mode 0700. No live lab QEMU process remained. Failed prior
runs and diagnostic overlays remain private for investigation; they are not
published or counted as a passing full run.
The retained [historical Run10 evidence](../infra/tumbleweed-lab/evidence/issue13-run10.txt)
predates the dedicated sentinel and exact GRUB-version criterion. It does not
establish the current reset acceptance criterion; Run11 supersedes that proof.

Local CI-style validation uses the same toolchain prefix above:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Exit 0 |
| `cargo test --workspace --all-features --locked` | Exit 0; 4 lab tests + 1 workspace boundary test passed, 0 failed |
| `cargo deny check` | Exit 0; advisories, bans, licenses and sources OK |

Two actual refusal checks also returned 1: invoking the command with
`env -u RYOKU_LAB_AUTHORIZED` refused unauthorized capacity before creation;
invoking it with an existing run directory returned `File exists (os error 17)`
without resuming or deleting that directory. No hosted GitHub CI result is
claimed. The issue must remain open until its real KDE gate and subsequent
review/integration verification pass.
