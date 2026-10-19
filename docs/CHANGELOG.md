# Documentation changelog

## Unreleased

- Package and start the Rust clipboard history service for the Ryoku session;
  apply the pinned upstream QML routing/error patch so the existing clipboard
  surface uses its dedicated socket and remove the upstream daemon's clipboard
  startup hook so only the Rust provider watches selections. Native
  installed-session acceptance remains pending.
- Capture the pinned Stage provider's `stage-walls.json` per-wall effect and
  layer-arrangement registry as a bounded opaque preference. Do not traverse
  generated Stage output, progress records, or source assets referenced by the
  registry.
- Extend the first-sprint session inventory with the pinned upstream backlight
  level, touchpad-disabled marker, and night-light enabled/temperature/schedule
  records. Each is an exact bounded account-owned path; the state records
  restore preference data and do not authorize a new privileged device-control
  path. Restore preflight continues to derive its allowed path and inode kind
  from the compiled inventory.
- Include the pinned power-profile provider's `power-profile.json` user choice
  and bounded `power.json` CPU/battery profile definitions in session
  checkpoints. Use the compiled capture inventory as the restore preflight's
  path-and-kind policy, preventing declared preferences such as Ryoshot and
  device vibrance from being captured successfully but refused at restore.
  Capturing these files does not authorize hardware setting writes.
- On a failed Ryoku session activation, stop the partial desktop target, clear
  the named manager and D-Bus activation environment, and request Hyprland exit
  so the display manager can recover. Refuse cleanup when the command is
  invoked without a compositor instance signature. Host tests cover the
  cleanup/exit ordering; native lifecycle validation remains open.
- Enroll the pinned Ryoshot drawing-tool preference file in the bounded
  first-sprint configuration inventory. Capture exact `ryoshot.json` bytes as
  an opaque user-owned regular file; its save-directory setting does not
  authorize traversing or capturing that directory. Also include the bounded
  scalar screen-vibrance preference written to `nvibrant-value`; keep launcher
  frequency and notification histories, note text, and API secrets excluded.
  The focused inventory fixture verifies absent and present cases, exact
  payload preservation, and observed UID/GID/mode metadata;
  host test evidence is recorded for issue #18.
- Keep user-authored desktop notes out of recovery generations: name the
  pinned `$XDG_STATE_HOME/ryoku/desktop-notes.txt` writer path explicitly in
  the exclusions and assert that session-preference capture omits its content.
  Harden both read-only root systemd services with private devices, read-only
  kernel tunables/modules and control groups, and SUID/SGID creation disabled.
  Native unit activation remains unverified on the pinned Tumbleweed guest.
- Keep typed local profile reads and ordinary settings writes in the
  authenticated unprivileged service. Connect exact canonical legacy personal
  profile import to the same-user atomic writer; the importer preserves its
  source and never invokes the privileged helper or creates a machine
  checkpoint. An identical retry succeeds only when the stored destination
  exactly matches the imported document; conflicting destinations are never
  replaced. Protected system and companion profile activation remains gated on
  the linked checkpoint coordinator. Protocol version 3 retains the typed
  request boundary.
- Add `ryoku-profile show` as a bounded, read-only view of the local typed
  profile through the authenticated user service. It needs no Git remote or
  network credentials; protected profile activation remains gated on linked
  recovery.
- Add a concrete root-only profile lifecycle guard for initialized state
  mounts. It pins the validated state directory and production mutation-lock
  inode, holds the lock exclusively, and refuses incomplete restore journals;
  public provisioning no longer accepts caller-invented exclusion guards.
  The historical Run16 probe used a private lock, so rerun on the pinned guest
  before claiming native validation. No initialized-store CLI/helper operation
  is enabled yet. `ryoku-state-init initialize` validates profile storage
  before reporting an existing state mount as available and never creates or
  repairs it implicitly.
- Require that lifecycle guard for every public inactive-bundle publication.
  Production capture and checkpoint-session paths pass a duplicate of their
  already-held exclusive mutation-lock descriptor; publication revalidates
  the guard across durable staging, rename, and sealing. Fixture-only tests use
  a crate-private path. Native probe rerun remains required.
- Resume the offline system or linked rollback reboot handoff when a validated
  Snapper rollback receipt is already durable at `WritersStopped`. This path
  requests only the reboot and never repeats the native rollback after a
  process exit between receipt persistence and reboot. It first checks whether
  the exact rollback target already booted and advances the journal instead of
  issuing another reboot. A not-yet-booted result is distinct from invalid
  target/backup metadata; only the former can continue after the current root
  independently matches the saved checkpoint. Unexpected identity leaves
  recovery pending. Add subprocess-exit coverage for both system-only and
  complete-linked scopes after durable receipt persistence, confirming the
  `WritersStopped` journal remains discoverable with the exact target ID. The
  reboot retry also rereads Snapper inventory and requires the exact tagged
  writable default target plus its retained read-only backup. Health checks
  still control completion.
- Keep native rollback target verification available through
  `ConfigurationRestored`, `HealthChecked`, and `Complete` for complete-linked
  recovery, and through system health/completion phases for system-only
  recovery. This lets idempotent restore and known-working retention retries
  revalidate the exact running Btrfs/Snapper target after later journal writes.
- Make `confirm-rollback` idempotent after a durable boot confirmation: a
  retry revalidates the source, running target, and backup, then reports the
  current restore phase instead of attempting the forbidden
  `WritersStopped` transition again.
- Add the pinned ten-band equalizer command in Rust, route the original
  `Equalizer.qml` to it, and package its per-user PipeWire smart-filter unit and
  audio tooling dependencies. State writes stay in the user's Ryoku config
  directory with atomic updates; generated runtime configuration stays private.
  PipeWire graph behavior and audible output still need acceptance in a
  supported desktop session.
- Declare `procps` as a direct desktop RPM dependency because the packaged
  Hypridle lock command calls `/usr/bin/pidof`. Bump the desktop RPM release so
  updated installations receive the dependency. The installed Tumbleweed host
  package provides that path; resolution in the pinned VM fixture and idle-lock
  runtime behavior still require validation.
- Print the SHA-256 of every staged install RPM and a deterministic install-plan
  digest binding the installed baseline, package metadata and payload/config
  paths, source bytes, and solver preview. The digest identifies review output
  only; it is not authorization or a mutation permit. Issue #24 remains
  read-only pending recovery acceptance and clean-install evidence.
- Exercise durable restore-journal process exits after every phase for
  configuration-only, system-only, and complete-linked scopes. Reopening must
  rediscover incomplete attempts and resume journal transitions to `Complete`;
  the rollback receipt is synthetic, so this does not prove native action
  replay or VM recovery safety.
- Preserve every validated issue #22 linked-checkpoint failure marker in the
  sanitized public lab report. The allowlist now admits the fixed symlink,
  payload, owner/group, directory-group, and failed-capture keys while retaining
  the key/value character filter. A marker audit also added the fixed
  changed-root historical-readback and boot-chain-observed results while
  keeping raw boot facts and snapshot IDs private; focused evidence tests pass.
- Validate the protected state mount's per-mount `nodev`, `nosuid`, and
  `noexec` flags and require its `subvolid` to match the installed machine
  identity. Contradictory device, suid, or exec options now stop recovery-state
  access before store open. The mounted Btrfs UUID still binds the live source
  to the recorded filesystem without storing an unstable device-node path;
  exact Tumbleweed mountinfo behavior remains to be verified.
- Bound root-helper socket exchanges with a single five-second deadline from
  nonblocking connect through complete request/response framing. Requests and
  responses remain limited to 64 KiB. This protects the helper transport from
  peers that stall or trickle bytes; native socket behavior remains pending
  runtime validation.
- Include the pinned Hub's exact `profile.json` in the bounded session
  preference inventory as an opaque user file or explicit absence. Capture
  preserves its bytes and metadata without treating it as the port's separate
  `opensuse-profile.json` schema or traversing referenced custom hero assets.
  The capture fixture verifies the raw payload round-trip; installed writer
  coordination and full restore integration remain open.
- Make service `Discover` probe the authenticated root helper and validated
  recovery state before reporting `Ready`. The discovery response still
  advertises no supported writes, and the Rust UI now labels readiness without
  implying that a particular linked checkpoint is available. Focused service
  and UI tests pass; installed Tumbleweed helper activation still needs its
  existing native acceptance gate.
- Require the shared XDG portal frontend and Hyprland, GTK and KDE backends
  from the desktop RPM, including with recommendations disabled. Package
  presence does not certify portal routing or screen-sharing behavior; the
  native ScreenCast and fallback acceptance cases remain open.
- Add a `Super+L` lock path to the full Hyprland session using the distro
  Hyprlock package and a fixed packaged lock surface. It uses a solid background
  and leaves PAM policy to the distribution; authentication and session-lock
  behavior still require VM validation.
- Start the distro Hypridle daemon as a bounded-restart user service tied to the
  Ryoku session target. Its packaged policy requests the same Hyprlock path
  after five minutes of idle and before logind sleep; screen lock and
  suspend/resume behavior still need VM validation. Bump the desktop RPM
  release for the added service unit.
- Require and start the distro Hyprland Polkit agent from the Ryoku session
  target so ordinary system authorization requests have a native prompt.
  This agent does not route protected Ryoku actions through the journaled Rust
  helper; those UI actions remain unsupported and the runtime prompt is
  unverified.
- Extend the linked-recovery failure probe with a traversal-manifest case and
  unsafe committed-manifest mode case. Both require typed refusal and restore
  backed-up bytes/mode before success; they remain source/compile coverage
  until exercised on the disposable Tumbleweed guest.
- Extend the linked-recovery failure probe with a committed-manifest symlink
  substitution. It retains a durable byte backup, exchanges the original file
  through a same-filesystem recovery path, requires historical lookup refusal,
  and checks restored bytes and metadata before cleanup. The probe compiles and
  its host marker validator passes; the destructive guest scenario remains open.
- Extend the linked-recovery probe with separate manifest owner-only and
  group-only corruption cases. Historical lookup must reject each changed
  identity, and the durable backup restores exact bytes and metadata before
  cleanup. The group-only case exposed a missing generation-file GID check;
  generation directory and artifact ownership now matches on both UID and GID,
  including newly created store artifacts. Native compilation and the focused
  host marker validator pass; both destructive guest cases remain unrun.
- Extend the linked-recovery failure probe to corrupt only the sealed
  generation payload-directory GID, require typed historical refusal, and
  restore its backed-up UID/GID/mode before revalidating the linked checkpoint.
  The native example compiles and the focused host marker validator passes;
  the destructive guest case remains unrun.
- Reject aliased destination-root descriptors in configuration restore. A
  regression fixture verifies the writer fails before applying files or
  creating destination-local staging when two manifest roots resolve to the
  same directory inode.
- Reject aliased source-root descriptors during generation capture. A regression
  fixture confirms distinct `StateRoot` labels cannot seal the same directory
  under separate inventories; the capture fails before reading or publishing.
- Add a pure retention planner for the protected Snapper catalog. It requires
  fresh evidence for every catalog row, pins manual and known-working
  checkpoints, retains the configured newest count of complete full-system
  pairs, and sends stale or ambiguous rows to reconciliation. It does not
  delete snapshots or generations; native cleanup ownership remains open.
- Add a read-only retention partner observer that verifies catalog rows against
  the protected generation store and fresh Snapper inventories. Foreign-config
  rows and duplicate snapshot IDs invalidate the observation; missing or
  mismatched partners cannot be selected for pruning. This does not perform
  cleanup or replace mutation-lock and native guest validation. Bump the
  recovery tools RPM release; native partner observation remains unverified on
  Tumbleweed.
- Expose that observer and the pure retention planner through the advisory
  `ryoku-recovery retention-plan` command. It reports prune candidates without
  deleting data or granting cleanup authority. Bump the recovery tools RPM
  release; native Tumbleweed output validation remains required.
- Require retention partner observations to match each generation's sealed
  root snapshot receipt against live Btrfs identity as well as a Snapper list
  row and exact transaction tag. A matching numeric row alone cannot mark a
  system partner verified. Bump the recovery tools RPM release.
- Add a read-only `ryoku-recovery layout` probe that binds root Btrfs/Snapper
  observations to the live root mount and classifies boot coverage by mount
  identity, including nested mounts such as `/boot/efi`. Any distinct mount
  under `/boot` is reported uncovered. Configuration-only fallback and
  independently snapshotted boot remain unsupported, and the command does not
  verify a linked Ryoku generation or root rollback. Native Tumbleweed layout
  evidence remains required.
- Make the recovery gate a one-shot journal check instead of holding a shared
  mutation lease for the whole desktop session. The lifetime lease prevented
  protected operations wired through the exclusive mutation coordinator from
  running while the user was logged in. Supported mutations still serialize
  under that exclusive lock and quiesce their account writers; root rollback
  remains restricted to the offline recovery console. Native Tumbleweed
  concurrency and socket activation validation remain required. Bump the
  desktop and helper RPM releases for the changed session and gate binaries.
- Add deterministic insufficient-space publication coverage that asserts no
  staging, generation or catalog entry is created, plus subprocess-exit
  reconciliation at all five generation-publication and four disk-stage
  creation boundaries, and replay at all six configuration-apply boundaries.
  These host fixtures do not replace the pending disposable-VM recovery gate.
- Add a root-only `complete-system` recovery command that reopens the linked
  source generation and native rollback target under the exclusive mutation
  lock, then runs bounded systemd readiness and recovery-gate checks before
  completing a system-only attempt. Keep `ryoku-recovery` and
  `ryoku-state-init` in the offline-capable recovery tools package. Add
  `prepare-system`/`rollback-system` for system-only recovery and
  `prepare-linked`/`rollback-linked` for paired root/configuration recovery.
  The rollback commands retain the linked checkpoint and journal one tagged
  Snapper operation before requesting reboot; post-boot confirmation and
  health/replay commands complete the journal. These paths compile, but native
  console acceptance remains outstanding. Recovery mutations now acquire the
  shared session lock nonblocking and report an active Ryoku lease instead of
  waiting indefinitely for the desktop to exit.
- Add root-only `checkpoint-session TRANSACTION_ID USERNAME` to capture the
  compiled shell, matugen, theme, brand, launcher, widget, performance,
  recording, visualizer, screenshot-capture, session-flag, equalizer curve,
  keypress-overlay and recording options, and personal-profile-selection records
  for one explicitly selected local user. It takes and verifies a tagged root
  Snapper snapshot, seals the account binding into the generation, preserves
  the raw current profile bytes, and fails on inventory or metadata mismatch.
  The command requires initialized protected state, the exclusive writer lock,
  and Snapper's preverification. It writes and reopens a
  linked entry marked unsupported until boot/layout coverage is established;
  it records manual-checkpoint retention intent but does not prove Snapper
  cleanup services honor that intent. State initialization creates the
  catalog explicitly, and normal recovery refuses to create a missing catalog
  lock. Native Snapper cleanup enforcement and full console restore remain
  incomplete.
- Add root-only `prepare-configuration RESTORE_ID SOURCE_TRANSACTION_ID` for
  exact session-preference checkpoints and `prepare-linked` for restoring the
  same declared data alongside a matching root rollback. It checks the
  supported path inventory, verifies settings JSON/profile schemas and package
  compatibility, captures a fresh current-state checkpoint, stages the replay,
  and persists the selected scope. Native cleanup retention and VM console
  restore acceptance remain open.
- Require `rescue.target` or `emergency.target` with `graphical.target` inactive
  for session checkpoint capture, configuration preparation and replay. A root
  TTY inside a live graphical boot no longer qualifies as offline recovery.
- Stop the selected account's logind sessions and user manager before creating
  a session checkpoint or capturing/restoring configuration; verify the manager
  and session list are clear while the exclusive machine mutation lock is held.
  The Ryoku session gate prevents the managed desktop from restarting during
  the operation.
- Bound recovery's `loginctl`/`systemctl` output while draining child pipes;
  oversized results fail closed without buffering unbounded output or
  blocking the child on a full pipe.
- Persist the exact Snapper root-rollback receipt in linked-restore journals,
  verify the booted writable Btrfs target and read-only backup before marking
  rollback complete, and coordinate writer-stop, one-shot rollback, reboot
  handoff guard retention, and system-only health completion. Verified
  configuration replay advances linked attempts through
  `ConfigurationRestored`. Add root-only `confirm-rollback` console command
  with fixed protected paths and the exclusive mutation lock, plus
  `restore-configuration` for configuration-only or already-verified linked
  attempts. The latter resolves only the sealed local account, verifies source
  and checkpoint partners, revalidates the booted root, restores through the
  descriptor-based writer, and requires JSON/profile health checks before it
  completes the journal. System-only health completion is implemented but
  remains unaccepted without native VM evidence. Root rollback uses the distinct
  `<restore-id>:rollback` Snapper tag so it cannot collide with the current-state
  checkpoint's `<restore-id>` tag.
- Connect configuration-only restore preparation to the idempotent destination
  writer and journal completion. Retries can reopen a partially applied attempt;
  caller-provided writer quiescence and health checks remain integration gates.
- Add system-only recovery preparation that verifies the selected source
  compatibility, captures and reopens current Ryoku state, and persists a
  resumable `CheckpointVerified` attempt without starting root rollback.
- Extend the read-only recovery scope preview to list each declared Ryoku
  configuration path and its restore/removal action without exposing saved
  contents, ACLs, or symlink targets. Restore execution remains unavailable.
- Add an opt-in root-backed recovery gate that blocks the Ryoku compositor and
  shell daemon when a restore attempt is pending or recovery state is
  unavailable; retain a shared mutation lease for the compositor lifetime, and
  keep KDE separately selectable. Provision its lock inode during explicit
  state initialization and open the lease read-only. Native gate activation and
  restore execution are still unvalidated.
- Add a Rust clipboard-history service and a narrowly patched original surface;
  native VM UI acceptance and recovery policy remain open.
- Apply upstream Ryoku contribution conventions with explicit openSUSE port
  requirements and preservation of the pinned runtime.
- Add typed service protocol v2 routing for authenticated, read-only linked
  checkpoint verification against the protected catalog and live tagged
  Snapper snapshot; keep provider mutations disabled and the helper's state
  mount read-only.
- Provision the global mutation-lock inode during explicit recovery-state
  initialization and let the session recovery gate acquire shared leases from
  a read-only descriptor under systemd's strict filesystem sandbox. Add a
  read-only state-mount identity path and a journal scan under that global
  lease, avoiding an exclusive flock on a read-only descriptor.
- Declare exact recovery records for pinned Ryoku `shell.json`, `matugen.json`,
  personal profile selection, and session flags without walking XDG state or
  including generated cache/clipboard data; verify fixture restore across
  separate configuration and home-root descriptors.
- Add an explicit native Snapper pre/post pair creator that reconciles each
  transaction tag, exact `pre-number` relationship, and returned snapshot ID
  against a fresh inventory before reporting success; the operation remains
  unavailable through service dispatch until authorization, writer
  coordination, and catalog integration are complete.
- Add a separate fixed-argument Snapper root rollback capability that checks
  the selected single-snapshot source, records a unique restore-attempt tag,
  and verifies both newly created snapshots and the returned target in a fresh
  inventory. It is not dispatched; Btrfs default selection, boot compatibility,
  durable receipt handling, and timeout reconciliation remain coordinator work.
- Add disabled personal-profile coordinator code to persist verified
  generation/Snapper pairs in a separate rollback-independent catalog, marked
  configuration-only until boot coverage is established.
- Added a filesystem-backed configuration-restore regression for exact base-only
  POSIX default ACL preservation. The test installs the ACL before generation
  capture, applies the captured configuration, and compares kernel readback.
