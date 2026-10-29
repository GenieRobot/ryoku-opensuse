# Ryogami automatic hook capability and effect matrix

Issue #176 is open. This branch is based on the exact #25 transport head
`899d97f9234b4dab1f947921b25c301d012deb2d`; source facts below refer to upstream
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. Paths beginning `daemon/` are under
`ryoku/shell/ryogami/`; `ipc/` is under `ryoku/shell/`.

Registration does not confer authority. Each automatic invocation needs its
own account/content/executable/write-set policy, independently verified linked
pre-state, bounded execution, writer isolation and verified post-state. The
original source, editor, settings and UI remain retained.

| Registration / trigger | Original executable, arguments and environment | Persistent writes / children / external effects | Contract and current implementation scope |
| --- | --- | --- | --- |
| `daemon/postapply.go` `afterApply`, excluding restore-like reasons | Arbitrary configured command after quoted `%type%`, `%name%`, `%path%`, `%thumb%` replacement; `sh -c` → `nohup setsid sh -c`, inherited environment | Arbitrary filesystem/network/process effects; detached children outlive reply | Exact configured-command policy; this issue's first operational slice supports bounded built-in commands and existing enrolled regular-file outputs. No inherited environment, detached child or implicit authority. |
| `daemon/postapply.go` `onStart`, `postProcessOnRestore=true` | Same command list; event reconstructed from saved outputs | Same effects during login/restore; selection of `*` or first sorted output | Same exact policy, with separately approved restore permission and authoritative saved-output match. Recovery does not implicitly approve a command. |
| `daemon/schedule.go` onStart and `schedule` setting | Timer to next schedule transition; `ryoku-hub desktop matugen set {mode:…}`; wallpaper/random/playlist calls | Wallpaper selection, palette setting, child Hub and its writers; scheduled external WE effects | Separate schedule/selection/palette envelopes; retained #176 gate. |
| `daemon/overview_backdrop.go` onStart and `overviewBackdrop.*` | In-process resolve/copy/blur helper pipeline | Overview backdrop directory/cache, generated image and settings-derived output | Exact source/cache inventory and transform identities needed; retained gate. |
| `daemon/library_thumbjobs.go` onStart and `performance.maxThumbJobs` | In-process atomic job limit 1–32; later scan worker goroutines | Registration changes memory only; actual scan invokes image/video tools and generates thumbnail/catalogue files | Keep registration distinct from scan effects; library generation provider is #175, trigger accounting #176. |
| `daemon/lens.go` onStart and `semantic.*` | Lens service and configured semantic model/runtime helpers | Semantic library/index/cache/model processes and potential provider installation | #175 model/provider authorization plus #176 lifetime/trigger policy; retained gate. |
| `daemon/pause_rules.go` onStart, `playback.*`, `paper.idlePauseSeconds`, afterApply | Window/process/audio/idle polling, two-second timer; native/external player pause | Playback runtime effects and signals; process inspection; no implicit user resume grant | Independent exact-player policy; #25 manual pause grants no automatic permission. |
| `daemon/library_videocache.go` onStart, daily loop | In-process prune; explicit clear stops/restores player | Deletes transcoded media/cache; player stop/restore and any restore hooks | #175 deletion/retention receipts plus #176 timer and restore authorization. |
| `daemon/rotation_settings.go` onStart and `general.random*` | In-process random pool, interval minimum ten seconds | Repeated wallpaper apply, playlist/catalogue state and downstream hooks | Stored-selection binding and independent apply plus hook envelopes; retained gate. |
| `daemon/power.go` onStart | Power subscription with fallback ticker | Effective performance/pause/render changes and consequent player work | Read subscription separated from each authorized effect; retained gate. |
| `daemon/display_fill.go` onStart and `display.fillModes` | In-process fit map and surface republish | Live wallpaper frame/publication; no persistent write in registration | Typed publication policy needed; no command-policy grant borrowed. |
| `daemon/library_trash.go` onStart, daily loop | In-process trash retention | Deletes original media at stored paths, rewrites `.local/share/ryogami/trash/manifest.json`; copy/rename/remove branches | #175 exact recoverable-media ownership, #176 timer/trigger; retained gate. |
| `daemon/audio_duck.go` onStart and `playback.muteOnOtherAudio` | Resolved `pactl list sink-inputs`, long-lived `pactl subscribe`, retry goroutine | Audio-server reads followed by effective mute/playback effects | Exact executable/server identities, subscription lifetime and player policy needed. |
| `daemon/notify.go` afterApply except restore-like reasons | Resolved `notify-send -a Ryogami [-i thumbnail/source] "Wallpaper changed" name`; child and wait goroutine | Desktop notification/D-Bus side effect; referenced icon reads | Separate notification/runtime executable/server policy; retained gate. |
| `daemon/matugen_outputs.go` afterApply | Configured `externalMatugenCommand`, split into argv with `%config%`, `%path%`, `%scheme%`, `%mode%`, `%index%`; video thumbnail fallback | Writes `<cache>/matugen-config.toml`; configured template inputs/outputs and arbitrary configured `post_hook` reloads; renderer children | Complete template/content/output/reload policy separate from configured postProcessing; retained gate. |
| `daemon/theme_policy.go` afterApply and `theme.policy`, `theme.staticTheme`, `theme.customColors` | Shell/Hub palette bridge; custom seed theme construction; wallpaper-follow reassertion | Theme seed/settings/palette writes and downstream renderer/reload work | Own exact palette/template/reload envelope; retained gate. |
| `daemon/palette_frame.go` `matugen.videoFrame` | Reload config and restore outputs | Re-extracts video still, republish/apply and downstream theme effects | Independent restore/source/transform policy; retained gate. |
| `daemon/wallpaper_mute.go` `wallpaperMute` | In-process default and `setAudio` | Output/workspace audio persistence, player/audio effects | #25 manual audio action supplies no watcher permission; retained gate. |
| `daemon/library_paths.go` `paths.wallpaper`, `paths.videoWallpaper` | Reload config, library rewatch, asynchronous rescan | New configured roots, catalogue/cache/transforms and child processes | #175 root/scan policy plus #176 watcher purpose; retained gate. |
| `daemon/workshop_library.go` `paths.steam`, `paths.steamWorkshop` | Workshop rewatch request | Subsequent workshop catalogue/provider work on changed roots | #175 configured-root/provider policy plus #176 callback envelope. |
| `daemon/library_watch_status.go` `library.pollingFallback` | Rewatch channel; inotify or polling loop | Subsequent library scans and their cache/catalogue/tool effects | #175 scan provider, #176 polling lifetime/event authorization. |
| `daemon/resource_tier.go` `resource_tier` | In-memory decode cap, restore while video plays | Player replacement/decode/cache plus downstream restore effects | Exact restore/player policy; retained gate. |
| `daemon/theme_preview.go` `shell.hoverPreview` | End hover preview when disabled | Restores prior palette/theme/preview state and any bridge effects | Exact preview/restore/reload policy; retained gate. |
| `daemon/process.go` picker release setting | `kickPark` and idle grace supervision | Picker stop/restart/runtime lifetime; fixed QML child | Preserve picker path; independent runtime lifetime policy; no shell-command authority. |
| `daemon/daemon.go` startup migration, heal, store initialization, restore/rescan, playlist resume | In-process migration/scan plus image/video/WE helpers | Config/cache/index/state migration, rewritten/removed animated WebP, persisted library and selection, players | Each root/write/provider decision remains individually gated; #25 core user apply is not startup authority. |
| `daemon/daemon.go` settings/library/WM/workshop watchers and `restore_watch.go` | Filesystem/WM subscriptions and fallback loops | Settings watcher callbacks, rescans, hotplug/workspace/restore apply and spawned helpers | Registration alone cannot authorize callbacks; exact purpose/source binding needed on every event. |
| `daemon/daemon.go::warmAvailability` → registered availability callbacks | `videoOptimize` calls `ffmpeg -hide_banner -encoders` and `-decoders`; `weather` uses DNS plus two-second TCP connection to `api.open-meteo.com:443`; other callbacks inspect battery sysfs, executable paths, model/runtime manifests and palette scripts | Child FFmpeg probes and external network contact occur outside the grouped start callbacks; discovery-only callbacks read local state | **Still incomplete in this slice:** group start gates do not protect these probes. Exact process/read/network availability policies and per-probe tests are required within #176 before acceptance. |
| `ipc/ryogami.go` wallpaper frame change → `scheduleTheme()` → `ipc/theming.go::paintWorker` | Shell daemon; dynamic/static matugen branch, ffmpeg video sample, WM adapter, Hub lighting worker | See complete downstream effect list below | #25 preserves the typed `shell.wallpaper.theme` handoff. Theme remains separately gated pending full path/template/reload enrollment; the actual worker remains retained. |
| `ipc/ryogami.go` source/depth change → stage worker | Shell stage/cutout/helper pipeline | Generated depth/cache, frame depth publication, potentially model/helper work | Separate stage source/model/write/recovery policy; retained gate. |

The wallpaper theme handoff is not equivalent to writing `colors.json`.
The issue's `applyRyogamiFrame` entrypoint description corresponds to
`consumeRyogamiFrames` at this exact v1.0 source pin: changed frame sources wake
`scheduleTheme`, while source/depth changes separately wake stage. The #25
patch preserves these as typed requests to their independent Rust owner.
`paintWorker` first writes wallpaper luminance/tone even for a named theme. Its
static branch calls `matugenApplyStatic`; the dynamic branch checks Match
wallpaper, samples video as needed and calls `matugenApply`. `ipc/matugen.go`
uses the configured template map, its generated active config and template
`post_hook` commands; it writes palette/tones, GTK CSS, KDE globals,
application templates, optional Obsidian links and Steam Material theme paths.
`liveFrame` generates a per-video PNG with ffmpeg, atomically repoints the
stable poster symlink and prunes old frames. Success reloads compositor config,
reapplies border colours, restores a live Hub preview via a temporary JSON file
(and may delete a stale preview marker), then wakes `ledsWorker` to invoke
`ryoku-hub lighting accent`. These paths, custom templates and hardware effects
are not all enrolled by #25's fixed wallpaper inventory. No theme execution
permission follows from a wallpaper user operation or configured shell hook.
The deeper source/effect ledger additionally traces folder/cursor icon-tree
generation, Nomarchy publication and the exact GSettings keys. Font configuration
writes come from the adjacent `settings.go -> applyFont` callback, not from
`paintWorker`; their separate trigger must not inflate a wallpaper plan.

The machine-readable contract is
[`issue176-theme-effects.json`](../upstream/evidence/ryoku-v1.0/issue176-theme-effects.json).
Rust automation checks every shipped core/apps template input, output,
post-hook and roster group against byte-exact pinned source fixtures. Its
declaration requirements were sent to #175 for the shared exact-output capture
and restore seam; hook/template/reload selection and execution remain #176.
Logical GSettings keys need their own capture/restore actor: replacing the entire
`.config/dconf/user` database would overwrite unrelated preferences. Exact
symlink, absent-target and new-parent ownership declarations remain required.

## Logical GSettings capture and recovery enrollment

The theme contract now represents only the four retained desktop-interface keys:
`color-scheme`, `accent-color`, `gtk-theme` and `icon-theme`. A capsule preserves
the original user override separately from its effective and default values;
restoring an unset override means reset, even if the default equals an explicitly
stored value. The GTK nudge preserves the original placeholder/target sequence.
Values use typed GIO strings without shell or GVariant text parsing. The
actor loads only the exact pinned `/usr/share/glib-2.0/schemas` bundle with no
process-default or parent schema search chain. Missing bundle data refuses.
The whole desired batch is range-checked before any write, and a missing acknowledgement,
changed guard or unexpected read-back cannot attest success. These primitives
are tested against real installed GIO with a private memory backend and schema;
they do not activate native theme writes.

Native construction additionally checks the actual backend object is
`DConfSettingsBackend`; `GSETTINGS_BACKEND=dconf` alone is insufficient because
GIO documents a dummy memory fallback when the backend cannot load. A memory or
alternate backend is refused before any key capture. The selected owned backend
reference is passed explicitly to the settings object, and runtime/session
guards are rechecked after it loads. Tests separately prove real DConf type
identification in a private temporary environment and memory-fallback refusal;
this backend test does not claim an authenticated desktop capture.

The separately versioned capsule binds the exact account, worker, schema,
session-service, approved policy, desired changes, plan and native pre-checkpoint.
Outer `RecoveryManifest` schema 3 enrolls that capsule only when its account and
native receipt exactly match the manifest. Legacy schemas 1 and 2 retain their
original bytes and digests. Immutable generation read-back retains the complete
logical image. Existing configuration and complete-linked restore planning
refuses schema-3 key state before creating a journal or changing targets until
the guarded key restore actor joins the full restore barrier. A file-only
runner therefore cannot silently report a complete logical restore.

The packaged `/usr/libexec/ryoku/ryoku-theme-actor` entrypoint currently exposes
one capture-only operation over an inherited connection from a real root peer.
It rejects duplicate keys, other accounts, extra/replayed packets, unknown
operations and disconnects. The launcher uses the fixed root-owned executable,
clean environment, a bounded deadline, parent-death protection and observed
child reaping. Landlock denies filesystem writes and seccomp denies later
exec/fork and non-Unix networking while permitting required GIO threads.
The live scope pins declared root-owned runtime/schema files, checks executable
mappings and the exact session socket/process, and checks the held pidfd for
process death on each guard check. The receipt is structural linkage; the root
coordinator must independently verify native checkpoint provenance/availability,
retain all writer holds and durably enroll pre/post state before any settings
write can be enabled.

`NativeCaptureSession::capture_with_theme_settings` now joins this read-only
capture to generation publication. The root coordinator creates a separate
one-shot socket and passes its descriptor over the existing authenticated writer
lease. The account wrapper keeps the original lease and native writer stops,
launches the fixed worker, and acknowledges only after observed worker death
and renewed writer/lock checks. The separate descriptor preserves real root
peer credentials without adding a helper capability or changing account
identity. The root side validates the nonce, account, policy, plan, desired keys
and exact native receipt, then waits for the wrapper's completion status.

The logical capture runs after the file image has passed its exact read-back
checks and before that same generation is sealed/published. A failed worker,
foreign account/receipt, extra reply, lost writer hold or failed publication
aborts the whole generation. Pure transaction, transport and process ownership
code live in recovery; GIO/session attestation remains in service. The helper RPM
declares the dynamically loaded GIO ABI and desktop schema dependencies.

Actual-binary tests prove private invocation and nonroot-peer refusal. The
hosted root-only transport harness separately verifies a root-created descriptor
crossing into an ordinary-account capture worker, missing-descriptor refusal
and exact native receipt return using a read-only injected backend. It is not
evidence of native DConf/session success.
Native root/session capture, logical restore execution and combined
file/key/reload completion remain required before activating theme execution.

The first slice's deliberately narrow execution policy is an initial supported
policy class, not permanent removal of commands, children, custom roots or
networked hooks. Extending it requires a new reviewed policy class with complete
recovery and effect accounting. Native Tumbleweed login/restore/theme lifecycle
acceptance remains required under #26. No VM or media/model download is allowed
while the host C: storage gate remains active.

## Implemented configured-command path

The retained daemon sends `hook.postprocess` with the filtered command index,
exact configured command SHA-256, saved wallpaper path/type and restore flag.
It sends no executable or expanded shell command to the helper and never
executes a returned command. Matching configured commands run independently of
the broader start/after-apply registration groups, exactly once. The startup
route independently reads saved output selection and requires both the UI's
`postProcessOnRestore` choice and the exact policy's restore permission.

`ryoku-hook inspect INDEX` prints the current command, type and digest.
After reviewing those bytes, an operator explicitly invokes:

```text
ryoku-hook authorize INDEX COMMAND_SHA256 SECONDS apply|restore [kitty_user] [fish_user] [staged] [template:SHA256:RELATIVE_PATH ...]
```

`kitty_user` means the existing `.config/kitty/user.conf`; `fish_user` means the
existing `.config/fish/user.fish`. Both are already exact capture/restore
inventory entries. No arbitrary output path or recursive directory is admitted.
Commands, settings, custom paths and UI affordances remain retained. This first
supported policy class runs Bash built-ins, with literal upstream-compatible
placeholder quoting, clean HOME/PATH/LANG, account home as working directory,
1–30 second wall deadline, two-second CPU limit, 128 MiB address limit and
16 MiB output limit. Output inodes are write-only; reads are restricted to
pinned Bash/interpreter/library/cache inodes. A written output cannot become
a `dlopen`/ELF-loader input. Bash and its reviewed runtime
content jointly identify the executable policy. Landlock ABI 3 constrains file
reads, writes, creation, truncation, rename and executable access; a native
x86_64 seccomp allowlist denies networking, child creation, process control,
namespace/mount/ptrace/io_uring APIs and device ioctls. The ELF loader cannot read
an unapproved executable. Child descriptors are closed on exec; no daemon or
helper socket is inherited. Existing output links, multiple links, unsafe modes,
wrong owner, alternate roots, modified command/config/runtime or unknown policy
refuse before launching.
The helper's captured home and output device/inode identities join the exact
plan and private call. The wrapper checks them before launch and uses the pinned
home descriptor as cwd; a substituted HOME directory or output inode cannot
redirect writes into an uncaptured tree.

Approval itself uses the authenticated operator helper and existing protected
mutation coordinator. The root-owned `0600` policy record lives in verified
rollback-independent state and references its approval operation. A pending,
failed or interrupted approval is unusable: each run requires Succeeded,
Complete and a linked post-checkpoint for that referenced approval. Every run
independently checks current account, complete configuration bytes, command
content/index/type, saved output selection and exact execution/write policy.
It then acquires the existing machine lock and all account writer freezes,
requires verified linked pre-capture, revalidates the exact plan and durably
enters Applying before the one-use authenticated writer-lease `H` message.
The root helper's capabilities and read-only home protection are unchanged.

The Rust account wrapper spawns the confined command while every native writer
tree stays stopped. The policy denies all fork/clone forms, so this initial
class has no permitted grandchildren to lose during refreeze. Timeout,
helper disconnect or wrapper shutdown kills the exact owned child; parent
death also sends SIGKILL. The wrapper observes child death and rechecks the
held writer freeze before acknowledging any result. A kernel-stalled child
retains that lease while the helper times out; no completion is invented.
Root health checks, full installed post-capture and durable Complete precede
the correlated success response. Failed execution/refreeze/post-capture keeps
the existing recoverable Applying/MayHaveMutated or post-capture-failure history.
The declared outputs and their exact post-state are in the linked generation;
the policy digest in the receipt identifies that write set. Policy records are
operational authorization metadata, not user-editable configuration.

The optional `staged` policy is separately bound into approval and the plan;
existing direct approvals cannot acquire it implicitly. It preserves literal
HOME, cwd, UID, EUID and primary GID while computing against a private HOME
mounted at that same absolute path. Only exact declared output preimages and
separately approved template inputs seed the private home; unrelated home content
and inherited environment are not exposed.
The rest of the mount namespace is recursively read-only, networking has a
private namespace, and the noexec tmpfs is limited to 64 MiB and 256 inodes.
The same root-pinned Bash/runtime and seccomp policy apply. A second exec of
the approved installed Bash succeeds with numeric identity intact. An ELF
stored in the staging output cannot run through the approved ELF loader.

After observed child success, the wrapper accepts one private tmpfs descriptor
and inspects its complete membership. Undeclared entries, links, oversized
outputs, changed real preimages or unsafe destination parents refuse publication.
It prepares every replacement inode with captured mode/UID/GID/ACL/SELinux
metadata before publishing a name. A failed publication compensates attempted
paths from the preimages and leaves the existing linked recovery operation
failed; it never reports success after compensation. Timeout, cancellation and
quota failure publish nothing. Native writers remain held throughout computation,
commit and refreeze verification. Tests cover a real second-rename failure and
exact content/metadata compensation. Namespace denial is actionable failure,
never fallback to an unconstrained launcher.

`ryoku-hook inspect-template .config/matugen/templates/NAME` prints the exact
bounded regular input's digest. Up to 32 unique `template:SHA256:RELATIVE_PATH`
arguments may be added to a `staged` approval. Each input is at most 64 KiB,
account-owned, singly linked, privately writable and resolved without symlinks or
mount traversal. The helper verifies the read set during policy review; the
wrapper copies precisely those bytes at the same path with mode 0400. Real source
identity/content and private input content are checked before output publication.
A changed template requires explicit authorization of its new digest. Input
declarations convey no write, restore or parent-tree authority. Tests prove exact
template consumption, sibling isolation and changed/linked/oversized/write refusals.

This staging class currently supports the two existing captured output files,
the explicit bounded template read set and pinned Bash only. It does not yet
admit matugen, dynamic template outputs, input trees, optional parent creation,
symlinks, logical GSettings changes or
reload actors. The local environment has no installed/cached matugen binary;
real package execution and native lifecycle proof remain open. Secondary group
semantics and broader environment/input needs require their own reviewed policy.
The retained wallpaper → scheduleTheme → paintWorker path remains in scope,
with its complete effect ledger; this staging checkpoint does not prove it.

Template output enrollment must also include `matugenEnsureDirs`: it creates 23
directory leaves before roster/app-suite filtering, including disabled apps,
plus a conditional Steam parent. These exact leaves and any absent ancestors
need independent creation/restore records; selected template output ancestors
alone are incomplete. Existing directory metadata and unrelated siblings remain
protected, and relocated XDG roots require exact resolved declarations.

Unsupported external executables, children, file creation/replacement, custom
roots and network effects receive real policy/refusal errors. They retain their
UI/source and require further reviewed policy classes. This is usable bounded
command execution, not acceptance of the remaining matrix rows. Native daemon
trigger→helper approval→Snapper/generation→actual desktop execution, interrupted
approval restoration, and all full Tumbleweed lifecycle tests remain unproven.

## Startup broker readiness follow-up

Native startup now waits, before configuration/store initialization and hook
registration execution, for a bootstrap byte on its authenticated inherited
parent channel. Rust sends it after binding and validating the public broker
listener and before starting proxy threads that can transfer R/A descriptors.
An early connection therefore cannot race a valid startup hook into a missing
broker, nor can a query descriptor overtake bootstrap. Missing/invalid readiness
fails after five seconds without beginning native startup. The signal grants
no hook execution authority: the exact typed policy and linked operation remain
mandatory. A real Unix descriptor test checks byte/SCM_RIGHTS ordering and the
unchanged query frame; the complete retained Rust/daemon suites pass.

The original configured-command receipt is historical at commit
`9a77472971ae17f19df403eac4bcba0b752927c6` (hosted CI `40209142753` green).
The startup follow-up has its own source/log hashes in
`issue176-startup-readiness-20261025.toml`. Neither receipt accepts the remaining
per-hook/theme or native lifecycle requirements.
