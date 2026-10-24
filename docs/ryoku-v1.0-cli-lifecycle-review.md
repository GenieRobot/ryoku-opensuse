# CLI and system lifecycle: 1.0 source and adapter review

The full Ryoku experience remains the target. This review records current
source/package facts and the Tumbleweed implementation each feature needs;
it makes no new payload or feature exclusions and changes no shell/session
file owned by #25. No CLI, updater, doctor, host or recovery action is enabled
by this documentation.

## Exact source and current package disposition

The CLI tree at upstream commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8` is
`61cd98e4bf1ff6add76c4f54b94f1a9f955fb721`. It has **769 current files**,
compared with 615 at `98a0368253d9628be72b853985878a5db90e1547`.
The delta is **292 paths: 159 added, 128 modified, five deleted**, with no
renames in this area. Counts include tests and vendor files.

[`cli-lifecycle-disposition.tsv`](../upstream/evidence/ryoku-v1.0/cli-lifecycle-disposition.tsv)
records the 774-path union, current/prior exact blob and mode, delta status,
area and Source2/Source3/RPM disposition. All 769 current `ryoku/cli/**`
paths are absent from `runtime-source-manifest.tsv` and
`writer-source-manifest.tsv`. `packaging/rpm/*.spec` does not build or install
this Go CLI, `ryoku-host`, `ryoku-manifest`, its boot guard or tmpfiles file.
This is a current package gap, **not a decision to remove their intended
features from the full Tumbleweed design**. Source0's project-owned Rust
tools and similarly named upstream Go commands are distinct implementations.

| Current CLI area | Files | Current interpretation |
|---|---:|---|
| Root commands, tests, module metadata and changelog | 21 | User-facing command/API reference; no current Go CLI installation |
| `cmd/{ryoku-host,ryoku-manifest}` | 2 | Runtime host abstraction versus release-time manifest generator; neither currently built |
| `internal/updater` | 52 | Update, track, status, materialization, release and boot lifecycle reference |
| `internal/doctor` | 195 | Reconcilers and tests; a plain doctor call repairs state, not just discovery |
| `internal/host` | 52 | Edition-specific package/init/process/auth/system abstraction; no Zypper implementation |
| `internal/{keyring,securitykey,keyboard}` | 14 / 8 / 5 | Authentication, account/session and console/boot keyboard integration |
| `internal/{sys,importer,limineefi}` | 13 / 2 / 2 | Process/filesystem/repository helpers, import orchestrator, EFI remembered-entry helper |
| `internal/{ryokumanifest,ryotunesrelease}` | 6 / 5 | Release metadata generation and separate application release installation |
| `plugin_template`, `systemd` | 8 / 2 | Scaffolding and upstream boot/tmpfiles metadata |
| `vendor` | 382 | Exact Go dependencies/shared Ryoku WM/i18n source; not a new port executable |

Complete CLI bytes remain available in the verified immutable upstream Git
cache/tree. The existing Source2/Source3 archives are **selected exports, not
complete upstream archives**: they do not contain this CLI. Full CLI archival
coverage is therefore a pending #173 source-input task, not a property of
their recorded hashes. Retain the exact complete source and notices in a
verified source-only archive during the coordinated input refresh, without
silently building or activating the edition-specific host actuators. No
archive was generated or existing selection changed during this offline pass.

## Dispatch, runtime and subprocess boundaries

`main.go` dispatches update, materialize, reset, owner, rollback, boot-guard,
boot-menu, snapshots, status, version, reload, wm, deploy, recovery, track,
plugin, doctor, verify, debug, keyring, security-key, keyboard and import.
`cmd/ryoku-host/main.go` forwards to `internal/host.App.Execute`.
`internal/host/command.go` separately exposes init/pkgmgr/capabilities,
snapshots, repositories, services/environment, inhibition/transient jobs,
session maintenance, packages, power/time/keymap/initramfs, fingerprint auth,
network, phone and users. These command strings are not the Rust service's
typed/versioned managed-action protocol.

[`cli-lifecycle-sites.tsv`](../upstream/evidence/ryoku-v1.0/cli-lifecycle-sites.tsv)
records **835 process/probe or filesystem-write candidate lines in 138
non-vendor production Go files**, with original blob and line. It scans direct
`exec.Command/CommandContext/LookPath`, process/exec calls, `sys.Run/RunOut/Sudo`,
host runner calls and selected direct file-write/remove/rename/mkdir/link/mode
APIs. Comments, tests and vendor code are outside this candidate scan, but
remain in the complete disposition ledger. A candidate can be a read-only
probe, an indirect wrapper or a destructive call; this lexical inventory is
not an exhaustive call graph or per-operation certification.

| Exact source entrypoints | Platform behavior and persistent/runtime effects |
|---|---|
| `internal/host/{detect,package,dnf,repo,table,overlay}.go` | Detects only `pacman`, `xbps-install`, or `rpm` plus `dnf5`/`dnf`; `RYOKU_HOST_PKGMGR` accepts only those three. Installs/removes via Pacman, Yay/Paru AUR helpers, DNF or XBPS and supports caller-supplied package overlays. Writes/refreshes `/etc/pacman.conf`, `/etc/xbps.d`, `/etc/yum.repos.d/ryoku.repo` and associated caches. Seeing RPM on Tumbleweed does not make this a Zypper backend. |
| `internal/updater/update.go`, `ryokuset.go`, `channel.go`; root `track.go` | Default update explicitly moves only the Ryoku package set; `--system` adds system/AUR/Flatpak work. It runs materialize/reload/doctor and supports a new-binary stage2 handoff. Release channels, Git checkout deploy and package channel changes affect installed code/config/repositories. Tumbleweed needs reviewed compatible full-system transactions, not a translated partial-update shortcut. |
| `internal/updater/runstate.go`, `gui.go` | Publishes phase/PID/step/progress/prompt state in `$XDG_RUNTIME_DIR/ryoku-update.json`, plus authentication/control plumbing. Its JSON/PID and secret FIFO model is not a durable Rust operation ID, authorization token or verified checkpoint. Preserve the user progress/reconnect experience through the existing service contract. |
| `internal/updater/{materialize,reset,owner}.go` | Composes shared defaults and `~/.config/ryoku/user_edits`, writes many `~/.config` targets and converges Quickshell config. Reset drops selected overrides; file ownership and unrelated user data must be preserved through the port's installer/profile/capture contracts. No blanket ownership of `~/.config/quickshell` is granted by upstream comments. |
| `recovery.go`, `track.go` | Prefer a checkout script; otherwise recovery fetches `https://raw.githubusercontent.com/ryoku-dev/ryoku/main/bin/ryoku-recovery` with curl into a temporary file and runs Bash. Track can fetch branch-specific `bin/ryoku-track` from main/unstable-dev and execute it. A TLS connection or branch name does not pin executable identity. Their intended recovery/channel features need a pinned/verified Rust implementation, not mutable script execution. |
| `internal/updater/{bootguard,bootmenu}.go`, `internal/limineefi/remembered.go` | Boot guard uses `/var/lib/ryoku/update-pending.json`, `/var/lib/ryoku/boot`, notice state, `/boot/limine.conf`, GRUB reboot selection and EFI state. Fedora boot-menu code writes snapshot-menu/kernel copies under `/boot`, uses `/etc/kernel/cmdline` and prunes copies. These effects are not authorized by a Snapper root checkpoint alone, particularly ESP/boot/runtime boundaries. |
| `internal/host/{snapshots,service,service_manifest,environment,session,transient,turnstile}.go` | Snapper create/cleanup, systemctl/systemd-run versus runit/sv, environment propagation, service symlinks/manifests and session wrappers. Transient accepts arbitrary command/args and properties; service start is not per-command authorization. Service persistent state includes `/var/lib/ryoku/service-manifest`, system/user definitions and enablement; runtime effects require separate health/restore accounting. |
| `internal/doctor/doctor.go`, `reconcile_*.go` | Default doctor performs convergent repair across packages, release trust, boot/kernel/initramfs, services, display/WM, network, device controls and app/config migrations. `--check/--json/--explain` set a read-only flag for reconcilers; `--report` still writes a diagnostic report. `verify` and diagnostics need native read-only/provider/privacy review rather than treating every doctor mode as harmless. |
| `wm.go` | Lists/status/caps and provider config/action routes, but also WM package switch/removal and reclaim. Preserve Hyprland discovery/integration through #25; package switching/removal requires the package coordinator and later compositor scope. Do not forward arbitrary actions merely because a provider reports a capability. |
| `internal/{keyring,securitykey,keyboard}` and host `{auth,users,phone,network,power,system,initramfs}.go` | pkexec/sudo escalation, PAM target rewriting, keyring/security-key stores, user/group/password/file deletion and quotas; keyboard writes `/etc/X11/xorg.conf.d/00-keyboard.conf` and `/etc/vconsole.conf`, then may rebuild boot images. Host phone plans include package/modprobe/config operations; network helpers and sysfs power changes have additional external/runtime effects. Authentication alone does not establish their affected-state capture or authorization scope. |
| `plugin.go`, `plugin_new.go`, `plugin_share.go`, `internal/importer/importer.go` | Git clone and template writes, Ryostore install/remove/placement, plugin capability audit, GitHub/gh publishing/browser launch, and headless Hub import scan/apply/undo. Upstream plugin audit metadata is not the port's trusted mutation plan. Publishing/network credentials and remote changes are not reversible by restoring local files. |
| `cmd/ryoku-manifest/main.go`, `internal/ryokumanifest`, `internal/ryotunesrelease` | Release generator consumes edition/installer choices and writes JSON; it is publish-time tooling, not a runtime provider. Ryotunes release code queries/installs package assets and escalates package operations, requiring the same package trust/transaction contract as other applications. |

The low-level `internal/sys/sys.go` `Sudo` simply delegates to a sudo command;
`internal/host/host.go` `ExecRunner.Run` executes a provided command spec.
Neither verifies a linked configuration generation. Upstream
`host.Snapshots` recognizes its three edition families, and its create operation
calls `snapper ... create` plus cleanup. This is not the port's complete linked
pre/post recovery protocol or protected-store identity check.

`systemd/ryoku-boot-guard.service` runs `/usr/bin/ryoku boot-guard` before the
display manager when the update marker exists. `ryoku.tmpfiles.conf` creates
world-writable boot handoff state and `0666` greeter handoff files. The current
RPMs do not install either file. The port must preserve lifecycle/recovery
features through its authenticated Rust admission/recovery path and native
Tumbleweed metadata, not copy these edition-specific root consumers blindly.

## Existing owner and required Rust equivalent

| Feature to preserve | Existing owner and exact integration obligation |
|---|---|
| WM/status/capability queries, shell lifecycle and appearance state | #25's verified Rust desktop/provider path; use the existing adapter capability/result types. The full generic upstream `ryoku`/`ryoku-host` command surface is not currently implemented by those adapters. #173 records call/response semantics; it does not invent successful replacements. |
| Read-only doctor/service/system information | #31's native information/service discovery, #25 for desktop-specific health, #27 for installed package/source/lock discovery. Split findings by owner; preserve diagnostics with bounded, privacy-reviewed output. A typed read cannot execute a repair or silently refresh mutating provider state. |
| Update/track/package source selection and package rollback | #27 resolves exact package/source/signature/vendor plans; #28 executes authorized revalidated full-system/repository transactions with linked pre/post checkpoints; #29 owns UI/CLI client progress/reconnect. Preserve channel/version intent via pinned signed Tumbleweed artifacts and supported policy, never import Pacman/AUR/DNF/XBPS operations. A package downgrade also needs a reviewed compatible plan; it is distinct from complete linked checkpoint restore. |
| Complete linked recovery and boot/login admission | Existing #18–#21 foundation/console contracts and #25 session integration, with #26 combined acceptance. Reuse the packaged Rust `ryoku-recovery`/admission paths, verified immutable generation/snapshot identities, independent protected catalog/journal and authenticated execution. Implement any missing user-facing CLI routing under those owners; no curl-to-Bash fallback. Offline recovery must require neither network nor a mutable remote script. |
| Materialize/reset/defaults/profile edits | #24 owns protected installer/default composition and override preservation; #25 owns core desktop settings/appearance integration. Reuse their exact plan/capture ownership, with #20 coordinator primitives, rather than adopting an unbounded config-tree writer. Remaining general import/plugin state adapters need explicit scoped ownership under the relevant desktop/application work. |
| Start/stop/enable, environment, power/runtime lifecycle | #31 typed service operations with affected persistent state and runtime limitations; #25 ordinary session/desktop lifecycle. Arbitrary transient command strings cannot become a generic service/helper API. Missing power/network/device operations need explicit scoped provider work, not a blanket service authorization. |
| PAM, account, keyring, fingerprint, security-key and destructive user operations | #32's selected account/security adapter scope, plus #25 ordinary login/lock behavior. New operations require individual authorization/capture/restore evidence and synthetic-account tests. Never log credentials or assume a root snapshot restores erased home data. |
| Remaining standalone app, plugin publication and application-release behavior | #47 inventories/splits applications; package acquisition remains #27–29. #173 retains source/API/provenance evidence. Network publishing/import execution and per-app writers need explicit implementation ownership; #47 is not a generic write authorization. |

The current `ManagedAction` enum in `suse/service/src/lib.rs` has catalog,
package/profile and checkpoint actions, not a generic doctor, arbitrary
transient command, root script, keyring or plugin-publish action. New
project-owned implementation/tests remain Rust. Reuse #20's
`NativeMutationProvider`, plan fingerprint/revalidation, authenticated account,
writer lease, linked capture and durable operation accounting; credentials or
a launch checkpoint alone do not authorize later repair subprocesses. Shell
hooks require bounded write scope and explicit authorization, including their
external/runtime effects, before they can participate in a protected action.

Selected Hub/Ryogami callers of `ryoku-host` still exist outside this CLI
tree (for example Hub `backend/{wm,phone}.go` and Ryogami
`daemon/source_youtube.go`). They do not cause the Go CLI to be embedded or
installed. Existing refusal patches cover some mutators, while missing query
or feature bridges remain real integration gaps. #25 must connect the retained
clients to the appropriate typed Rust providers; #173 must not remove their
features or package a false-success compatibility shim.

## Provenance, validation and remaining work

Root GPL/NOTICE provenance remains authoritative for retained upstream code;
vendored exceptions must keep their own texts. Exact CLI license blobs include
godbus BSD-2-Clause `670d88fcaaf0bcbca03a7179bad8b83393607b7f` and x/sys
BSD-3-Clause `2a7cf70da6e498df9c11ab6a5eaa2ddd7af34da4`. The plugin template's
MIT text `eb28fd9e105c82cb9fd8f74b10f0385bb6ad5130` intentionally has year/author
placeholders for a newly generated plugin; it does not relicense all CLI code.
Shared vendored WM/i18n provenance remains in the complete per-file ledger.
The CLI Go module declares Go 1.23 and local WM/i18n replacements; this does
not change the selected shell/Ryogami SDK's Go 1.26 requirement.

Validation here compares both immutable Git trees, exact path/blob/mode and
delta counts, manifest nonselection and current RPM recipes; checks site line
contents against original blobs; verifies local documentation links and
`git diff --check`. No code, build, download, package/VM access or shell/session
edit is part of this pass. The current source archives and runtime selection
are unchanged.

The remaining semantic work includes per-reconciler read/write/trust/restore
proof, every nested host verb's Tumbleweed contract, full CLI source-only
archival coverage, implementing the owned Rust equivalents, native provider
tests and refreshed #25/#26 lifecycle acceptance. This area inventory is not
whole-CLI behavioral acceptance and does not complete the full #173 semantic
matrix or authorize any mutation. #173 and PR #174 remain open/draft.
