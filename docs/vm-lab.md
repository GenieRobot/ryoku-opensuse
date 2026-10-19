# Disposable Tumbleweed cloud lab (#13)

## Authorized openSUSE WSL runner

The user authorized this existing openSUSE WSL VM for development and tests on
2026-10-15. Build `vm-lab` with `--features wsl-lab` to select the separate
[`wsl-fixture.toml`](../infra/tumbleweed-lab/wsl-fixture.toml) input set.
The default build retains the original cloud fixture and proxy requirements.
An all-features build also selects WSL; do not use an all-features binary for
the original cloud run. Each host copies its own executable into the guest,
so guest repository, package, and snapshot assertions use the same input set.

This runner uses installed `/usr/bin` tools: QEMU 11.1.2, `qemu-img`, and
`mkisofs`, plus native openSUSE OVMF firmware. Its code and variable-template
hashes are checked before boot. It does not load the old extracted Debian
tools or libraries. The ordinary-user host must identify as Tumbleweed under
WSL and explicitly set `RYOKU_LAB_AUTHORIZED=existing-wsl-vm`. Fresh run
directories must be under the invoking workspace or temporary directory and
outside the asset directory. The same guest fixture marker, QEMU identity,
blank second-disk checks, signed repository checks, login checks, offline
network restriction, and reset requirements remain in force. WSL itself is
never passed to the destructive guest operations.

The WSL image is the exact signed Cloud appliance
`openSUSE-Tumbleweed-Minimal-VM.x86_64-1.0.0-Cloud-Snapshot20261016.qcow2`,
SHA-256 `ec8fd95a4c8a1315b3f2cdf609eb226e2578ac3faae18c1fc4242a911041a231`.
Its checksum signature verifies with the isolated openSUSE project public key,
fingerprint `AD485664E901B867051AB15F35A2F86E29B700A4`. The official SBOM lists
kernel `7.2.10-1-default`, `kernel-default-base-7.2.10-1.1.36.11`, and GRUB2
`2.16-2.1`. The first boot in the current disposable run reached the guest
login prompt and reported the expected Tumbleweed snapshot and kernel. The
current run's native Btrfs migration, recovery, KDE and reset results are
recorded separately in its evidence file; no older snapshot is a test target.

Prepare an asset directory containing the exact image named by the WSL fixture,
`image.sha256`, `image.sha256.asc`, and an isolated `gnupg` directory with the
installed openSUSE public keys. Preserve the signed checksum's original image
filename. When the rolling appliance advances, review and pin its signed
snapshot, checksum, package baseline and repository metadata together before a
new run; do not use a moving URL as the run's identity. The WSL profile uses
direct guest networking and imports no host custom certificate authorities;
direct mode is rejected by a default cloud build. Proxy environment variables
are explicitly cleared for fixture Zypper operations before applying any
selected cloud proxy. Repository signatures and exact metadata digests remain
mandatory for either network mode.

```console
cargo build --locked -p ryoku-automation --features wsl-lab --bin vm-lab
RYOKU_LAB_AUTHORIZED=existing-wsl-vm target/debug/vm-lab run \
  "$ASSET_DIRECTORY" "$PWD/target/wsl-lab-FRESH-NAME" \
  --capture-ui "$PWD/target/debug/ryoku-ui-gtk"
```

Build the optional GTK binary first using the instructions in
[`ui-integration.md`](ui-integration.md), or omit `--capture-ui`. Inspect the
fresh run's sanitized `evidence.txt` and retain private logs locally. The full
WSL acceptance sequence is still being exercised; the inspection boot and
unit tests are not a passing release or linked-recovery gate.

Before launching, use `vm-lab preflight ASSET_DIRECTORY FRESH_RUN_DIRECTORY`
with the same options, build feature, authorization environment and working
directory intended for `run`. It reports all independently checkable host
prerequisite failures together and creates no run directory, VM, disk or key.
Both commands share the runner identity/path guards, signed image and firmware
checks, source binary checks and QEMU version check. A successful preflight is
an observation; `run` repeats the checks before creating its private directory.

The optional `--profile-bundle-probe PROBE_BINARY DEFAULTS_RPM RPM_SHA256`
stage requires `--console-only --state-initializer INITIALIZER_BINARY`. It
runs after state initialization and before `--linked-checkpoint-probe`, when
that independent stage is selected. Build the probe with
`CARGO_BUILD_JOBS=2 cargo build --locked --offline -p ryoku-recovery --example native_profile_bundle_probe`.
Pass the same three arguments to `preflight` and `run`. The RPM checksum is an
explicit lowercase SHA-256 of the reviewed local artifact; a valid rebuild can
have a different RPM header timestamp and artifact checksum. Source/archive,
toolchain and build-result evidence remain separate review inputs.

This stage adds host requirements for native `/usr/bin/rpm` and its shared
libraries. Host preflight bounds the RPM size, checks the supplied artifact
hash and native RPM digests, and requires the exact reviewed
`ryoku-profile-defaults-0:0.1.0-1.noarch` package, seven file records and their
metadata/digests, with no scripts or package/file triggers. It installs nothing
on the host. The probe and RPM are copied into the private run directory before
boot, sealed against ordinary writes, hashed for evidence, and checked against
those frozen hashes before guest transfer. Local unsigned-artifact checks are
not a package-signature trust claim; no digest or signature bypass is used.

The guest must already have the initialized machine record and matching native
Btrfs state mount, the disposable QEMU seed, native `rpm` and `sha256sum`, and
runtime libraries compatible with the built probe. It must have no existing
defaults package, package payload directories or profile store. The lab excludes
external package/state writers. Before installation the Rust harness checks
these prerequisites, copies uploaded inputs into an exclusively created
root-private directory, compares actual hashes, rechecks the complete RPM
contract and probe runtime dependencies, and runs native `rpm --install --test`.
Only then does plain `rpm --install` run in the disposable guest, followed by
installed RPM verification, fixed-path default-byte/ownership verification and
the guarded inactive-bundle probe. Exact proof markers are required; failures
and raw diagnostics remain in private logs. The package and private input copies
remain on the disposable disk until normal discard. The current probe holds
the production `mutation.lock` exclusively and refuses incomplete restore
journals. The earlier Run16 guest trial predates that change and used a
probe-only lock, so its result must not be credited as production writer
exclusion; rerun the updated probe in the pinned guest.

These results cover installed generic defaults, immutable inactive bundles,
offline layer resolution and refusal checks. They do not establish active
profile selection, audited writer enrollment, complete configuration capture,
protected mutation composition or restore activation. The optional harness
wiring and host preflight alone are not native execution evidence.

The host checks require executable native commands, readable executable x86_64
ELF inputs and resolvable native shared libraries. Dependency inspection uses
the known native loader's `--list` mode, never an input's main or an arbitrary
ELF interpreter. Static x86_64 inputs are supported. Memory headroom is the
configured guest RAM plus 1 GiB from `MemAvailable`; available disk space on
the run filesystem must be at least 12 GiB plus staged input sizes. These
checks reserve no resources and do not verify guest packages, guest library
compatibility, networking, boot, or recovery behavior. The output explicitly
keeps guest prerequisites unchecked and VM runtime unproven.

The WSL build also accepts explicit `--accelerator kvm` on both commands;
omitting it retains TCG. KVM preflight requires ordinary-user access to the
native `/dev/kvm`, API version 12 and IRQCHIP/HLT/USER_MEMORY/SYNC_MMU support.
It queries capabilities without creating a VM. KVM selects `-cpu host`; TCG
retains `-cpu max`. Neither selection falls back to the other. The cloud build
rejects KVM. Guest resources remain 6 GiB and two CPUs. Every actual boot,
including reboot/reset, verifies the selected accelerator through its owned
QMP connection and records `accelerator_runtime=verified` only after that
check. Host capability queries alone establish no guest boot or recovery proof.

The first automated WSL run used port commit
`59245afcb4e00597cb91290a62439a9a76a2d2f7` and passed signed-input checks,
XFS-to-Btrfs migration, sole-disk UEFI boot, SELinux/Snapper inspection, and
reboot. KDE/SDDM provisioning completed with Plasma `6.7.5-1.1` and SDDM
`0.21.0+git57-1.1`, but the ordinary-user login gate failed. Native Xorg logs
reported `No input driver specified, ignoring this device` for the emulated
keyboard and mouse; neither `xf86-input-libinput` nor `xf86-input-evdev` was
installed. The minimal `--no-recommends` fixture therefore rendered a greeter
without accepting its keyboard input. The fixture now explicitly installs
and inventories `xf86-input-libinput` for the Xorg SDDM greeter. This change
requires a fresh run; the failed login is not relabeled successful and no
autologin, PAM bypass, or reduced session check is used.

That run terminated with exit 1 and `lab_result=blocked` because login and UI
capture did not pass. Its independent lifecycle result passed: boot identity
changed on reboot; restricted-network recovery detected controlled corruption
and Snapper restored the fixture bytes; a durable sentinel matched before
discard, then was absent on a fresh XFS baseline together with the marker,
desktop package, and Snapper configuration. The source image hash was unchanged
and the harness removed its VM disks and temporary SSH keys. This proves the
lab lifecycle and bounded fixture restore, not a linked system/Ryoku restore.

The second WSL run, built from `4836a7cffa7e4b4f9da701f613868ed5de43698c`,
installed `xf86-input-libinput-1.5.0-1.7` and passed native SDDM password login
both before and after reboot. The assertions confirmed an ordinary, local,
active Wayland session with Plasma and KWin and a connected compositor socket.
Restricted-network controlled corruption and the bounded Snapper restore also
passed. The final fresh XFS baseline had no fixture marker, desktop package,
Snapper configuration, or sentinel. The image hash was unchanged, all run
disks and SSH keys were removed, and the process exited 0 with
`lifecycle_result=passed` and `lab_result=passed`. The independent optional
result was `ui_visual_capture=blocked`; it is not credited as visual acceptance.
This establishes the WSL lab lifecycle and KDE fallback, not the complete
linked restore engine or the original cloud-only acceptance condition.

New runs remove the disposable XFS migration overlay immediately after the
standalone Btrfs disk boots and passes its base checks. The signed source image
remains necessary for the final fresh-baseline reset. Optional GTK failures
are retained in `ui-failure-private.log` before later SSH probes overwrite the
rolling guest log. The guest launcher records GTK output in root-owned mode
0600 logs, and a fixed diagnostic command retrieves at most 128 KiB per
scenario into that private host log before disposal. These raw logs must not
be published as sanitized acceptance evidence.

Run04's retained diagnostic identified the optional capture failure: the GTK
success process was running, but a second QMP connection timed out while the
input connection was still alive. The monitor serves one client at a time.
The harness now sends input and requests the framebuffer on the same QMP
connection. Both lab profiles pass 57 tests, including an input/capture sequence
with distinct reply IDs on one socket, plus all-feature automation Clippy.
Run04 uses the earlier executable and retains `ui_visual_capture=blocked`;
a fresh run with the corrected harness is required for automated visual
acceptance. Its reboot/offline/reset lifecycle was still running when this
diagnosis was recorded. Native checkpoint and 20 Btrfs/SELinux staging tests
were exercised separately in that guest and do not establish full restore.

Run04 later terminated with signal 15 before its restricted-network/reset
sequence completed. No task-owned VM process survived; its logs were retained
and its disposable disks and keys were removed. Its reboot/login and native
component results remain useful, but the full run is interrupted. Guest binary
copies now use the running process's `/proc/<pid>/exe` rather than a build
output pathname, so a concurrent Cargo rebuild cannot replace their source.

Run05 completed standalone Btrfs boot, ordinary-user native SDDM/Wayland login
before and after reboot, restricted-network fixture restoration and fresh XFS
baseline reset. Its disks and keys were removed and the signed base was
unchanged. The [Run05 evidence](../infra/tumbleweed-lab/evidence/issue13-wsl-run05.toml)
separates this passing lifecycle from visual acceptance: the historical
`ui_visual_capture=passed` flag only established that files were captured.
Inspection of the scenario images did not establish a visible GTK proof window.

New captures wait up to 120 seconds for a marker emitted after a mapped,
nonzero-size GTK window completes a paint frame. Process existence and renderer
diagnostics do not satisfy readiness. Automated output now uses
`ui_framebuffer_capture`, and visual acceptance still requires reviewing the
images. Runtime logs are retained even when transport succeeds. Both lab
profiles pass 58 tests; the updated GTK proof emitted its paint marker under
openSUSE WSLg with the existing Mesa warnings. That local result is not native
KDE visual acceptance.

The fresh [Run06 exercise](../infra/tumbleweed-lab/evidence/issue13-wsl-run06.toml)
passed the full WSL lifecycle and both GTK demonstration scenarios. Both
processes reported a painted first frame before capture; manual review confirmed
a visible proof window with successful-refresh text and a visible unavailable
error. The [success](../infra/tumbleweed-lab/evidence/issue13-wsl-run06-success.png)
and [error](../infra/tumbleweed-lab/evidence/issue13-wsl-run06-error.png) images
contain only the disposable test desktop and controlled demo data. All run disks
and keys were removed after the fresh-baseline reset. This establishes KDE demo
rendering; it does not establish live provider status, keyboard dispatch apart
from programmatic refresh, accessibility or complete linked recovery.

## Original cloud runner

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

Package sources are pinned separately: OSS and non-OSS use the official
`/history/20261011/tumbleweed/repo/` paths. Each enabled repository's exact
`repomd.xml` digest is recorded in `fixture.toml`; native forced refresh must
verify signatures, then every cache digest must match before installation.
Unknown enabled repositories, disabled signatures, indirect sources and changed
metadata fail. The official codec/update sources have no verified historical
path (tested update history returned 404), so their original enabled sources
remain with explicit signed metadata digest pins. They are never silently
disabled. Digests were independently verified with release signer
`AD485664E901B867051AB15F35A2F86E29B700A4`.
The native cloud repo files omit their signature flags and inherit defaults.
Initialization writes explicit `gpgcheck=1`, `repo_gpgcheck=1` and
`pkg_gpgcheck=1` for every recognized enabled source. Existing false or invalid
overrides are rejected; omission is not treated as disabled verification.

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
mode is 0700. SSH password authentication and root login are disabled.
The desktop test uses a separate ordinary nonroot account and an ephemeral
password delivered only through stdin and virtual keyboard events to SDDM/PAM.
It has no sudo or wheel access. Fixture-only administrative sudo access uses
the disposable SSH key and must never be
installed on a supported user system.

QEMU itself binds a loopback SSH host-forward using port zero. The host reads
that process's private QMP `info usernet` reply, requiring one exact TCP mapping
from loopback/nonzero port to the default guest's port22 before connecting.
No temporary port reservation is released for another process to bind. The
bounded QMP parser rejects malformed/ambiguous/error/unknown-ID responses and
enforces message, event, nesting and absolute timeout limits. A diskless native
probe verified the assigned port was already owned (`AddrInUse` for a competing
bind) and QEMU stopped successfully. Run13 subsequently exercised actual guest
SSH through these QEMU-owned endpoints in the complete base lifecycle.

GitHub-hosted Rust checks were observed working for PR #54 in
[run 38316734108](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/38316734108)
on `ubuntu-24.04`; exact revision, timestamps and results are recorded below.
This observation covers that Rust run. Runner inventory returned `Forbidden`;
no self-hosted VM runner registration was established by this work and no hosted
VM job ran. `vm-lab.yml` remains an explicit manual contract for an existing
authorized runner carrying `ryoku-disposable-vm`, and creates no resources.
The cloud command above supplies the actual VM validation evidence.

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
required KDE/SDDM provisioning. An active display manager and installed RPMs
are prerequisites; acceptance also requires an actual authenticated nonroot
SDDM login, active local seat0 Wayland session and live package-owned Plasma/KWin
processes. A
reboot must change the guest boot identity and pass inspection again.
The current minimal image's SBOM reports running kernel `7.2.10-1-default`,
owned by `kernel-default-base` RPM `7.2.10-1.1.36.11`; the guest inspection
checks the kernel image's actual RPM owner, rather than assuming the full
`kernel-default` package. The fixture pins GRUB2 `2.16-2.1`; the main and EFI
GRUB2 RPMs must match that version during every base inspection. Other package
versions are reported from the guest instead of carried forward from the older
fixture.

The offline phase restarts the same guest with QEMU user networking restricted;
loopback SSH remains available. A TCP connection to the existing external proxy
must fail before recovery is accepted. The guest creates a real root Snapper
snapshot, corrupts a lab-owned file, restores that file from the snapshot and
requires matching bytes. It then removes the marker and the snapshot. This
proves the lab can exercise controlled corruption and offline Snapper recovery.
Full root rollback, boot-menu snapshot selection, production linked Ryoku/home
state recovery are later issue acceptance gates; this fixture test does not
establish those behaviors. Run11 did not establish desktop login; the follow-up
adds a fail-closed mandatory check. Run17 installed KDE/SDDM but did not pass
the actual ordinary-user Wayland login/session check.

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

Package-manager and SSH readiness commands have bounded execution and output.
Each guest zypper operation used by migration or optional GTK proof has a
20-minute deadline and retains at most 1 MiB per output stream. KDE provisioning
uses a dedicated 34-minute zypper deadline, based on Run22 reaching package
479 of 745 before the prior 20-minute deadline. The complete remote KDE setup
is additionally bounded by 36 minutes in the guest and 38 minutes on the host.
Optional GTK installation has a 25-minute absolute SSH deadline to leave room
for the guest's 20-minute zypper deadline and post-install checks. Migration has
a 60-minute absolute host deadline for its
two package-manager stages (up to 40 minutes combined) and slower TCG disk
preparation. Generic
SSH commands have a 10-minute cap; guest readiness retries share one absolute
10-minute deadline, so every SSH attempt consumes that same budget. Timeout
handling drains both output streams, kills
the child process group, and reaps the direct child. Binary transfers also have
a 10-minute deadline and bounded output. Captured diagnostics remain in the
private run directory, while callers and public evidence receive stable failure
categories such as `SSH_READINESS_TIMEOUT` and `KDE_PROVISIONING_TIMEOUT`. KDE
provisioning output is saved once to `kde-provision-private.log` with mode 0600,
separate from the rolling SSH log, so later stop/recovery output cannot replace
the package-manager diagnostics.

## Historical KDE download blocker (Runs 11–13)

Earlier full recommended closure and a retry with native `--no-recommends` both
selected 750 packages and failed with Zypper exit 4 on
`libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`, HTTP 403. Hard dependencies,
signatures and TLS were preserved. The retry did not remove the codec requirement.

In Runs 11–13, the allowed official URL
`https://codecs.opensuse.org/openh264/openSUSE_Tumbleweed/x86_64/libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`
returns HTTP 302 to
`https://ciscobinary.openh264.org/libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`.
A HEAD request on 12 October 2026 verified that redirect without following it.
`ciscobinary.openh264.org` is absent from the enforced cloud allowlist. The minimum
resource action is authorization of that host through the supported environment
configuration workflow, or an accessible, supported Tumbleweed KDE fixture.
No direct-route/proxy bypass, fake codec, signature override or solver override
was attempted in those runs. Run17 supersedes the OpenH264 access result; see
the later recorded run for current installation and graphical-login evidence.

The [signed dependency trace](../infra/tumbleweed-lab/evidence/kde-dependency-trace.md)
records the hard KDE→Baloo→file metadata→FFmpeg→OpenH264 chain, full native
session/X11/pattern dry-runs and the build-only FFmpeg provider rejection.

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

The manual workflow constructs a fresh `/workspace` path using trusted workflow
run ID/attempt values, rather than accepting an artifact path from a caller.
Its `always()` upload after an attempted lab step selects only that exact
`evidence.txt`, so blocked/failed evidence is retained without publishing raw
diagnostics, disks, identities or SSH material. No manual VM workflow has run;
this workflow correction is verified locally as declarative configuration.

Normal completion leaves sanitized evidence and private diagnostics. Failure
returns nonzero and terminates only the QEMU child owned by that process; it
retains the disposable directory for diagnosis. If the process is forcibly
terminated, stop its exact QEMU instance before deleting its run directory.
Never run a wildcard process kill or delete the shared asset directory. Re-run
using a new directory after repairs. No incomplete evidence is a passing run.

## Recorded run

Run11 completed on 12 October 2026 in the existing managed cloud workspace.
[Sanitized VM evidence](../infra/tumbleweed-lab/evidence/issue13-run11.txt) contains
the checked fixture identifiers and lifecycle results. PR54 head
`18648cae24ddf501e5f5b17882dc9102ae1f820c` and its fixture produced
the tested Run11 debug executable SHA256
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

Recorded Run11 local CI-style validation used the same toolchain prefix above:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Exit 0 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Exit 0 |
| `cargo test --workspace --all-features --locked` | Exit 0; 4 lab tests + 1 workspace boundary test passed, 0 failed |
| `cargo deny check` | Exit 0; advisories, bans, licenses and sources OK |

Two actual refusal checks also returned 1: invoking the command with
`env -u RYOKU_LAB_AUTHORIZED` refused unauthorized capacity before creation;
invoking it with an existing run directory returned `File exists (os error 17)`
without resuming or deleting that directory. The issue must remain open until
its real KDE gate and subsequent review/integration verification pass.

## Follow-up Run12 failure and signature-default repair

[Run12's actual sanitized prefix](../infra/tumbleweed-lab/evidence/issue13-run12.txt)
records signed-image verification, fresh creation and successful source boot/SSH
through the QEMU-owned port. On local commit
`4323be6a1ada8d25445dac7a4fb304cba9df2607`, executable SHA256
`2940c395036fd71e7f1e483a1db727088445859cd8b9056e0618eb713dcf0414`, the command
returned **1** before package installation or Btrfs formatting: the verifier
reported `native repository signatures must be enabled`.

The actual signed fixture's six repo sections contain no explicit signature
flags. A separate read-only-backed diagnostic overlay preserved the failed disk
and established that missing flags inherit native defaults. The verifier's
explicit-field assumption was incorrect. The repair canonicalizes all three
native checks to `1` for the four recognized enabled sources, while rejecting
any explicit false/invalid value. Regression tests cover the actual omitted-field
shape, valid native true values, false/invalid overrides and idempotency.

Run12 is incomplete evidence: no migration, recovery, reset or GUI success is
credited. Its private logs/disks remain retained; only its actual sanitized prefix
is public. This verifier defect is separate from the codec runtime-policy blocker.

## Follow-up Run13: verified base lifecycle, blocked KDE

Run13 completed on 12 October 2026 at 23:34 UTC using exact local source commit
`40355fd0c31eeee9d5b18ae82252f3eee8e7ccab`, debug executable SHA256
`b2d162c36b4734df5001ce6d8fb3a6ba93bf33c515de4c34aa56f17ba787f0e9`:

```console
RYOKU_LAB_AUTHORIZED=existing-cloud-workspace ./target/debug/vm-lab run /workspace/opensuse-vm /workspace/ryoku-lab-issue13-run13
```

**Process exit 1 / `lab_result=blocked`.** The [actual sanitized evidence](../infra/tumbleweed-lab/evidence/issue13-run13.txt)
records the fresh cloud source/derived boots, mandatory native signature checking
and exact metadata pins, Btrfs/ESP/GRUB/initramfs and ACL/context parity, exact
kernel owner/GRUB/Snapper versions, enforcing SELinux and ordinary D-Bus Snapper.
Actual reboot changed the boot identity and repeated inspection passed. Offline
egress was unreachable; real corruption and native undo restored exact bytes.

The sentinel was present on the derived Btrfs/Snapper baseline, exact 29 bytes
matched and file/parent fsync passed. After whole-disk discard the fresh original
XFS guest required the sentinel, recovery marker and root Snapper config absent
via only-`NotFound` checks, plus exact KDE RPM absence. Every normal QEMU stop
required exit status 0. Disks, firmware, seed and SSH keys were removed; the
original image SHA256 was unchanged. The mode-0700 run directory contains only
the sanitized evidence and private diagnostic logs; no owned QEMU remains.

KDE's complete native Plasma/SDDM/Xorg `--no-recommends` closure selected **745**
new packages and failed preloading the original OpenH264 RPM with **HTTP 403 /
Zypper exit 4**. No RPM installation phase was shown. No ordinary-user graphical
login, post-reboot desktop proof or hosted VM artifact upload is credited. The
full #13 acceptance remains incomplete until the already-approved Cisco origin
is actually applied to runtime policy and the real desktop checks pass.

Final local checks use the recorded toolchain prefix: format, all-target/all-feature
Clippy, workspace tests (**21 lab + 1 boundary = 22 passed, 0 failed, 0 ignored**),
cargo-deny and the binary build passed. Separate independent reviews validated
the immutable implementation and signature-default repair; passing review and
base evidence do not complete the blocked KDE gate.

## Follow-up Run17: KDE packages installed, graphical login blocked

Run17 completed on 13 October 2026 in the existing managed cloud workspace with
6144 MiB guest memory, QEMU 10.0.13 and TCG. The
[sanitized evidence](../infra/tumbleweed-lab/evidence/issue13-run17.txt) records
the complete run. Its runner executable SHA256 was
`a0b3597150378af14f6bf90db0eb5f0e51efa3ea68280de0cc0cba7616660e97`.

The official OpenH264 redirect worked through the existing network path during
this run; no proxy or direct-route bypass was used. The pinned transaction
installed Plasma 6 and SDDM (`6.7.5-1.1` and `0.21.0+git57-1.1`), and the SDDM
display manager became active. The mandatory ordinary-user Plasma Wayland
login/session inspection still failed with `no active ordinary-user Plasma
Wayland SDDM/PAM login on seat0`. Raising guest memory did not resolve this.
The evidence records `desktop_login=blocked`; package installation alone is
not KDE fallback acceptance. The precise login failure cause remains unproven.
The runner reports the lifecycle/recovery/reset gates separately as
`lifecycle_result=passed` once those checks complete. `lab_result` remains
blocked until the required ordinary-user desktop login gate passes, so this
separate lifecycle status does not change issue #13 acceptance.

All independent base lifecycle gates passed: derived Btrfs/EFI/GRUB/Snapper
boot, repeated inspection after reboot, unreachable external egress, controlled
corruption, offline Snapper restore with matching bytes, marker cleanup, and
sentinel durability before discard. Reset booted fresh XFS with the marker,
KDE package, Snapper config and sentinel absent. The source image was unchanged;
disks and keys were removed. Normal teardown left no lab QEMU process.

Only sanitized evidence is public; serial, guest and desktop diagnostics remain
private. Run17 supersedes the old OpenH264 access blocker but does not satisfy
the login gate or close issue #13. It also does not establish the separate GUI
application, provider-hook or profile-state acceptance gates.

For the next diagnostic attempt, the host records fixed-name QMP framebuffer
captures immediately before sending SDDM password keystrokes and when the
ordinary-user session check times out: sddm-before-input.ppm and
sddm-login-timeout.ppm. QEMU runs with its working directory set to the
private mode-0700 run directory; each screenshot is set to mode 0600. These
files are not part of public evidence and may show account or desktop details.
Inspect them locally and sanitize them before any sharing. A capture failure is
recorded in a separate private diagnostic and does not prevent the password
input or replace the original login-timeout result.

After the first failed ordinary-user session probe, the host also attempts a
fixed, read-only journal capture for `sddm.service` and
`systemd-logind.service`. `journalctl` is bounded to 15 seconds, the last five
minutes, at most 1000 lines, and at most 1 MiB each for retained stdout and
stderr. Excess bytes are drained but discarded; the private file marks any
truncated stream. Its output is written to `sddm-logind-journal-private.log`
with mode 0600 under the private mode-0700
run directory. This file is private diagnostic material and is never copied
into public evidence. If journal capture fails, the original session-probe
result remains the login gate result.

If `--ui-binary` was requested but the ordinary-user session was not verified,
the runner records `ui_visual_capture=blocked`. A requested UI capture failure
also remains an optional blocked UI result and does not change the lifecycle
result; the top-level `lab_result` still follows the required login gate.

## Follow-up Run18: lifecycle and recovery pass, desktop login blocked

Run18 used fixture snapshot `20261011`, QEMU `10.0.13`, verified image
signature, and 6144 MiB with TCG. Its sanitized key/value evidence records
`derived_root=btrfs`, `boot=UEFI-GRUB2`, `derived_grub2=installed`,
`derived_initramfs=generated`, `derived_btrfs_boot=passed`, and the installed
Plasma/SDDM versions `6.7.5-1.1` and `0.21.0+git57-1.1`. It records
`desktop_login=blocked`, while `reboot=passed`,
`offline_snapper_restore=matched`, `reset_fresh_boot=passed`,
`base_unchanged=passed`, and `reset_disk_and_keys=removed`.

The updated runner reports completed lifecycle/recovery/reset checks
independently as `lifecycle_result=passed`; Run18 itself predates this field.
The ordinary-user SDDM/Plasma login remains the required #13 closure gate, so
the run's `lab_result=blocked` is retained until that login is verified.
Run18 predates the framebuffer capture patch; it has
no login screenshots and does not establish why authentication/session
handoff failed. Only the sanitized key/value facts above are reproduced here;
private serial, guest/session logs and raw images are excluded.

## Follow-up Run19: SDDM graphical readiness race observed

Run19's sanitized evidence still records `desktop_login=blocked`; it does not
pass issue #13's login gate. Private QMP captures showed an all-black
framebuffer immediately before password input. The timeout frame showed the
SDDM greeter with the ordinary test account selected and an empty password
field. The bounded private journal recorded the greeter starting, but no user
PAM authentication attempt or ordinary-user session launch during the input
attempt. This points to a graphical readiness race: the guest had an active
greeter session/process before the greeter UI was ready to receive input. It
does not establish a PAM rejection.

The runner now polls QMP framebuffer captures, bounded to 60 seconds, until
the greeter framebuffer is non-black. It then records the private pre-input
capture and sends the password. Temporary readiness frames stay inside the
mode-0700 run directory and are removed after inspection. This is a targeted
fix for the observed blank-frame/input race; a fresh disposable run must still
prove password authentication and the ordinary-user Plasma Wayland session
before closing #13. Raw Run19 frames and journal remain private.

## Follow-up Run22: KDE install timed out; offline recovery passed

[Sanitized Run22 evidence](../infra/tumbleweed-lab/evidence/issue13-run22.txt)
records the exact fixture, image signer, Tumbleweed package versions, derived
Btrfs boot, and lifecycle outcomes. KDE provisioning reached package 479 of
745 before its bounded zypper deadline expired under two-vCPU TCG emulation.
Plasma installation did not complete, so `desktop_login=blocked` means the
ordinary-user Wayland login probe was not reached; it is not an observed
authentication or session failure. The bounded private KDE diagnostic log
confirmed the package transaction was still progressing and remains outside
the repository.

The independent base and recovery checks passed: the VM rebooted, egress was
unreachable, controlled corruption was observed, offline Snapper restore
matched, and the reset sentinel was verified before discard. A fresh XFS reset
boot confirmed the fixture marker, package, Snapper config and sentinel were
absent. The base image was unchanged and all run disk/key artifacts were
removed. The run therefore records `lifecycle_result=passed` and
`lab_result=blocked`; it does not satisfy #13's desktop login gate.

## Observed hosted Rust checks

[GitHub run 38316734108](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/38316734108)
completed successfully for PR head `18648cae24ddf501e5f5b17882dc9102ae1f820c`.
Job [115035038160](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/38316734108/job/115035038160)
used GitHub-hosted `ubuntu-24.04`, from 12 October 2026 22:27:11 through 22:29:10 UTC.
Checkout logs prove synthetic merge `f997dbc48c3c433e4e1d2498ecf1764eea81175d`,
combining that head with base `a942ed2caf4e7087c6d3de04e11638158d528688`.

Formatting, Clippy, workspace tests (5 passed, 0 failed) and cargo-deny all ran
and passed. [Sanitized hosted evidence](../infra/tumbleweed-lab/evidence/hosted-rust-run38316734108.toml)
records the exact commands and revisions. That run contained one Rust job and
no VM lab job; it establishes no KDE/graphical or hosted VM acceptance. New PR
heads need their own check results. The manual VM workflow and separate cloud
Run11 evidence retain their stated limits.

The automatic integrated-main [run 38318265080](https://github.com/GenieRobot/ryoku-opensuse/actions/runs/38318265080)
also passed on exact merged commit `5cfa5259f3a69d7b2316d75453c28033228a2e1c`
(same reviewed tree `6c0118f2a2a01c79169e3c47202581fee07e6b54`). Ubuntu24.04
job 115039994093 ran from 22:43:27 to 22:45:35 UTC; checkout/command logs confirm
format, Clippy, 5 tests and cargo-deny passed. [Sanitized integrated evidence](../infra/tumbleweed-lab/evidence/hosted-rust-run38318265080.toml)
records this existing run. Only Rust executed; no VM job ran or was dispatched.
Both hosted observations concern PR54's earlier source, rather than this
follow-up revision. Local checks and actual Run13 evidence cover this revision.

## Optional M0-06 libzypp hook experiment

The normal `vm-lab run ASSET_DIRECTORY FRESH_RUN_DIRECTORY` lifecycle is
unchanged. A one-off disposable provider experiment can be explicitly enabled
with `--zypp-probe PROBE_BINARY PACKAGE`, for example:

```text
RYOKU_LAB_AUTHORIZED=existing-cloud-workspace \
  ./target/release/vm-lab run /workspace/opensuse-vm \
  /workspace/ryoku-lab-issue15-run19 \
  --zypp-probe ./target/release/zypp-hook-probe hello
```

Build both Rust binaries from the same reviewed checkout with
`cargo build --release --locked -p ryoku-automation --bin vm-lab --bin zypp-hook-probe`.
The probe runs after the normal guest setup/reboot checks and before offline
recovery. It first gathers the read-only provider/package ownership inventory,
then runs deny, malformed-response, bounded-timeout and missing-plugin modes in
that order. The selected package must be absent from the guest RPM database and
available from the pinned repository metadata; choose a small harmless package.
The final missing-plugin mode may intentionally install it. Any unsafe or
inconclusive result is kept in the private logs and the derived overlay is
discarded without reuse. The optional experiment skips the offline recovery
phase because its deliberate missing-plugin bypass can change guest state;
the normal reset-from-base marker check still runs. Its evidence reports
`lifecycle_result=partial` and `lab_result=partial`; do not interpret that run
as a passing #13 recovery run or as a protected provider write.

The raw inventory/probe output, SSH diagnostics and QEMU logs stay in the
mode-0700 run directory; probe-specific logs are mode 0600. Public evidence
contains only allowlisted package/version facts, normalized allowlisted paths,
file kind, owner/mode facts, symlink targets only for discovered symlinks,
ACL/xattr availability statuses and hashes, and outcome flags. Raw metadata
values and guest output remain private; hashes cover ACLs and the allowlisted
SELinux/capability xattrs. The inventory reads RPM package records,
plugin/config paths and file metadata. It does not install more provider
packages. The
experiment is Rust-only; a discovered commit plugin and direct zypper are not
treated as integrated Ryoku authorization. PackageKit/Cockpit, Myrlyn and YaST
write paths remain unsupported pending separate transaction-boundary proof.

For storage and offline console component work, `--console-only` requires a
configured `--state-probe`, `--state-initializer` or `--zypp-probe` and rejects GTK capture. It skips
KDE package installation and login, records `skipped_console_probe`, and always
reports `lifecycle_result=partial` and `lab_result=partial`. It retains signed
input checks, standalone Btrfs/UEFI boot, enforcing SELinux, reboot, offline
fixture recovery (except the destructive zypper experiment), fresh-base reset
and disposable-disk/key cleanup. This mode does not satisfy desktop acceptance.

`--state-probe /absolute/path/native_state_provision_probe` copies the compiled
Rust fixture probe, verifies its exact component results, retains private raw
output, installs/enables the state mount unit, and verifies its identity and
store reopening after reboot. The current probe also exercises process death
after a completed creation receipt. These checks are specific component proof,
not full linked restore or complete interruption coverage. Run one VM at a time
on the authorized WSL host, and discard the owned disposable disk after evidence
capture rather than preserving another installed desktop image.

`--state-initializer /absolute/path/ryoku-state-init --console-only` is a
separate fresh-store test. It cannot be combined with the creation probe or
zypper experiment. The harness runs the real initializer, requires its fresh
success result, parses its bounded bootstrap transaction/snapshot receipt and
invokes the explicit mounted-receipt resume command, then invokes initialization
again to verify the recorded store remains available,
and repeats that check after reboot. First, repeat and post-reboot output are
retained privately. It still reports partial desktop/lifecycle acceptance.

Supplied GTK/probe/initializer executables are copied from pinned open file
descriptors into fresh mode-0700 files in the private run directory before
booting. The harness records their staged SHA256 values and its own running
executable hash. A later Cargo rebuild cannot replace the guest input through
the original path. Staging opens the resolved source without following a
replacement symlink and refuses existing destinations, empty or
oversized inputs, and detected in-place changes during copying. Completed-run
cleanup also removes these staged executables; failure artifacts stay private
for inspection. This staging gate was added after Run09 started and is not
credited to that earlier binary.

Pair `--state-prepare-probe /absolute/path/native_state_init_failure_probe` with
`--state-initializer` and `--console-only` to exercise unmounted receipt recovery.
The fixture creates a verified checkpoint and complete creation receipt without
the target mount. It requires a fresh retry to refuse without another initializer
checkpoint, a poisoned UUID to remain refused and unrepaired, and foreign target
data and a foreign mount unit to remain unchanged. It then removes only its own
synthetic obstructions and invokes the real CLI resume, followed by ordinary
repeat/reboot/offline/reset checks. Both executables are staged and hashed;
raw failure/resume output remains private. This is not arbitrary-volume adoption
or complete process-interruption coverage.

## Native linked-partner result

[Run15](../infra/tumbleweed-lab/evidence/issue19-run15-linked-partners.toml)
used explicit KVM acceleration after the shared preflight passed. Its evidence
window spans approximately three minutes. Signed-input checks, sole-disk
Btrfs/UEFI boot under enforcing SELinux, initializer checks, exact linked-partner
capture/read-back, four controlled partner refusals, actual recovery CLI queries,
normal-reboot persistence, owning-journal cleanup, bounded offline fixture
restoration and fresh-baseline reset/cleanup passed. Parent and independent
worker audits confirmed no surviving VM, disk or key and an unchanged signed
base. Desktop/login remained skipped and both overall result flags remain
`partial`. This run does not prove changed-root rollback or complete Ryoku restore.

The Rust `--exercise-failures` probe also emits individually validated markers
for corrupt and symlink manifests, traversal, corrupt payloads, artifact
metadata, and capture failure before publication. It now also prepares a fresh
linked current-state checkpoint, restores the verified configuration source
into an isolated `/run` target through the recovery library, and verifies the
restored bytes/ownership/mode, declared absence, and unchanged managed-state
and `/home/tester` sentinel inode/metadata. Its
`native_linked_configuration_restore=passed` marker is separate from
`native_linked_restore_execution=unproven`: the probe does not exercise the
complete-linked root rollback. The host's strict evidence allowlist preserves
these specific markers for the issue #22 report while continuing to reject
guest paths, free-form text and unknown keys. It also publishes the fixed
`native_linked_changed_root_historical_readback=passed` and
`native_linked_boot_chain_observed=recorded` results; raw boot-chain
observations and the fixture's exact snapshot number remain private.

## Retained WSL development desktop

While the retained guest is ready, run the WSL build as the same ordinary owner:
`RYOKU_LAB_AUTHORIZED=existing-wsl-vm vm-lab development-capture RUN_DIRECTORY`.
This reads the private readiness record, verifies the live owned QEMU/supervisor,
QMP socket peer and loopback SSH forwarding, and saves only the fixed mode-0600
`development-current.ppm` in that run directory. Each call refreshes that frame.
It validates the PPM structure without treating frame contents as visual product
acceptance; inspect or convert it with the existing native image tools.

The same owner and authorization can send bounded keyboard input with
`vm-lab development-input RUN_DIRECTORY --text 'firefox'` or
`vm-lab development-input RUN_DIRECTORY --key enter`. Text accepts 1–64 lowercase
ASCII letters, digits or spaces. Named keys are `enter`, `escape`, `tab`,
`backspace`, `space`, `super`, `left`, `right`, `up`, `down`, `home` and `end`.
The command repeats the capture command's VM ownership and endpoint checks,
validates the entire input before sending, and reports a static result without
echoing input. Key presses use QEMU's bounded `send-key` interface and are spaced
150 ms apart, after its default 100 ms release delay
([QEMU schema](https://raw.githubusercontent.com/qemu/qemu/v10.0.0/qapi/ui.json)).
Capture and inspect `development-current.ppm` after typing a search, then send
`--key enter` and capture again to verify the actual launched application.

`development-input RUN_DIRECTORY --chord super+n` also accepts `super+space`
and `super+escape`. It sends explicit modifier/key press and release events.
`development-input RUN_DIRECTORY --click X,Y` accepts unsigned pixel coordinates,
checks the owned QEMU's read-only mouse inventory, captures a fresh framebuffer
for its current bounds, then moves and presses/releases the left button using
[QMP input events](https://raw.githubusercontent.com/qemu/qemu/v10.0.0/qapi/ui.json).
A fixture without a current absolute pointing device returns an unsupported
error before any input event; this helper never changes the device topology.

The WSL build also accepts `--development-desktop` for a retained development
guest. Use the existing runner authorization, signed assets, preflight and
accelerator selection. The option excludes `--console-only` and every mutation
probe; UI capture remains optional and defaults to none. After KDE provisioning,
ordinary-user login and a native framebuffer capture, the run pauses for up to
24 hours before any lifecycle reboot or reset. This is session/transport
readiness, not visual KDE or Ryoku acceptance, and reports
`development_lifecycle=not_run`.

The mode-0700 fresh run directory contains `development-desktop.ppm` and a
mode-0600 `development-desktop-private.txt` with the loopback SSH port, relative
private key/known-hosts paths, QMP socket and owned process IDs. Use those paths
relative to that run directory. The record exists only during the hold; it is
removed before shutdown. Request release by creating an owned mode-0600 regular
file named `development-release` in the run directory, containing exactly
`release` followed by one newline. Symlinks, hard links, other permissions and
other contents are refused. Write a separate private temporary file and rename
it into place so polling cannot observe partial contents. Release, expiry, or an invalid token requests guest
sync and poweroff, waits for the owned QEMU child, and retains the migrated Btrfs
disk, firmware variables, seed and private SSH key. The signed base hash is
checked. A failed graceful shutdown retains diagnostics and terminates only the
owned child. Keep the supervisor running during development.

To boot that same stopped development guest again, build the WSL binary and run
the following from the workspace containing its run directory (use absolute
paths). Resume requires an explicit accelerator and repeats its own read-only
preflight before launching:

```console
RYOKU_LAB_AUTHORIZED=existing-wsl-vm /absolute/path/vm-lab development-resume-preflight \
  /absolute/path/assets /absolute/path/workspace/target/ryoku-development --accelerator kvm
RYOKU_LAB_AUTHORIZED=existing-wsl-vm /absolute/path/vm-lab development-resume \
  /absolute/path/assets /absolute/path/workspace/target/ryoku-development --accelerator kvm
```

Use `tcg` explicitly if KVM is unavailable. Resume checks the ordinary-user WSL
authorization, installed pinned QEMU/VGA BIOS/immutable UEFI code and native
libraries, available guest memory, a private owned mode-0700 run directory,
single-link owner-controlled disk/firmware/seed inputs, and mode-0600 SSH identity
and known-hosts files. The disk must already be standalone qcow2 with the fixture
virtual size and no backing image, and `evidence.txt` must end in the stopped
development receipt. No signed base image, ISO builder, package staging,
downloads or provisioning commands are needed to resume.

An existing readiness record, QMP socket, release token, extra migration disk,
live QEMU referring to the run/disk, or concurrent resumed supervisor causes a
refusal. Stale artifacts require operator inspection; resume never deletes them
or adopts a process from a recorded PID. A failed/interrupted resume leaves its
starting receipt and diagnostics for inspection, so it cannot be blindly retried.
The new supervisor verifies its newly spawned child's exact executable, argv,
parent, working directory, QMP peer and accelerator. Its fresh loopback SSH port
inherits the single existing ed25519 host key with strict host-key checking;
historical known-hosts entries are preserved. The private readiness record also
binds the supervisor and QEMU process generations. Prior serial, QEMU, guest logs
and initial screenshot remain intact; new boot logs/initial frame use unique
`resume-...` filenames. After transport readiness, the same 24-hour hold and
release/poweroff semantics apply, with no setup, lifecycle tests or disk reset.
Transport readiness still requires separate native login and visual acceptance.

## Optional two-account native service fixture

`--service-fixture FIXTURE SERVICE HELPER` selects the guarded service example
and the matching `ryoku-service` and `ryoku-helper` binaries. It requires
`--console-only --state-initializer INITIALIZER`; when combined with the profile
and linked probes, it runs after profile validation and before linked capture.
The host's shared preflight checks all three ELFs/native loader dependencies and
counts their bytes in disk headroom. It freezes mode-0500 copies and hashes
before the first boot, then rechecks them before upload. Guest staging verifies
and privately freezes the same bytes before installing the fixed example path.

The [standalone fixture contract](native-service-fixture.md) specifies the
actual tool/unit RPM inventory, root-private input paths, native logind/user
managers, exact unit payloads, read-only requests and assisted policy-test scope.
The harness invokes `--check` before `--run`, keeps separate mode-0600 staging,
prerequisite and fixture diagnostics, and publishes only the exact pass/scope
markers and declared native RPM inventory. Missing prerequisites fail before
fixture account/service setup; this option installs no guessed packages. A
failure retains the disposable guest for diagnostics until normal owned-VM
teardown. It does not authorize host accounts/services, RPM-install claims,
interactive-login claims or accepted provider transactions. No successful
native activation run is established by adding this optional wiring.
