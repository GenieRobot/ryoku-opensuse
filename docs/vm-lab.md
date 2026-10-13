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
Each guest zypper operation used by KDE setup, migration, or optional GTK proof
has a 20-minute deadline and retains at most 1 MiB per output stream. The
complete remote KDE setup is additionally bounded by 29 minutes in the guest
and 30 minutes on the host. Optional GTK installation has a 25-minute absolute
SSH deadline to leave room for the guest's 20-minute zypper deadline and
post-install checks. Migration has a 60-minute absolute host deadline for its
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
