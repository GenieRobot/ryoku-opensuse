# Typed service and helper contract

Status: versioned Rust protocols, a runnable unprivileged per-user daemon, and a systemd socket-activated root helper for [issue #17](https://github.com/GenieRobot/ryoku-opensuse/issues/17), plus a fail-closed sequencing contract for [issue #20](https://github.com/GenieRobot/ryoku-opensuse/issues/20). The per-user service and helper units are packaged, but installed Tumbleweed activation, provider mutation, and recovery transactions are not yet proven or enabled.

## Account-bound configuration roots

The service now has an account-bound default configuration-root component,
`AccountStateRoot`. It takes a kernel-authenticated peer identity, resolves its
UID through system NSS into a bounded buffer, and pins the existing account
home and `.config/ryoku` directories. Client `HOME`, XDG variables and manifest
paths cannot select the root. The account's UID and primary GID must own each
user directory; symlinks, writable-by-group/other directories and mount crossings
below the home directory are refused. Revalidation repeats account resolution
and compares pinned home, `.config` and `.config/ryoku` directory identities
before reuse. Replacing `.config` while retaining the original Ryoku directory
also invalidates the binding. Root UID is unsupported.
No files are created or enrolled, and this component does not authorize a write.
Nondefault XDG roots need a future explicit trusted mapping.

Both initial lookup and revalidation run NSS and root opening in a process-wide
worker with a two-second response deadline, one in-flight task, and no queued
work. The deadline bounds how long the caller waits; it does **not** cancel
libc/NSS or interrupt filesystem calls. A timed-out task retains the sole permit
until it finishes, so a permanently stalled NSS module leaves account lookup
unavailable until the helper restarts. Subsequent calls fail immediately with
`LookupBusy`; they do not accumulate threads. Each task has a fresh private
response channel. Late results are discarded and cannot become another peer's
binding. Permits are released on completion, task error, panic and thread-start
failure. An expired invocation cannot return a usable binding even if its result
was queued while the caller was descheduled.

`helper_protocol::prepare_operator_account` authenticates the opt-in operator
before any lookup, then selects only that exact peer's UID. It maps saturation
to structured `busy` and unavailable/changed/unsafe account state to
`recovery_unavailable`, without exposing NSS records or filesystem details.
This is a preparation boundary for the forthcoming capture executor; installed
mutating helper requests still return rejections. It does not perform account
lookup or authorize linked-checkpoint verification.
It provides no enrollment, writer exclusion, operation acceptance or mutation
authority on its own. Callers must still retain the coordinator lock and freeze
the declared writers through capture, publication and plan revalidation.

Focused Rust evidence in `account_state::tests` covers a synthetic stalled task,
timeout plus repeated busy refusal, late-result isolation, eventual recovery,
error/panic permit release, expired result refusal, native system NSS lookup,
and directory replacement/owner/mode/symlink fixtures. The helper preflight test
proves denied/root peers never reach the resolver, successful preflight uses the
exact authenticated identity, and errors have the expected structured mapping.
These local tests do not prove a live NSS outage or Tumbleweed deployment.

Per-user generation access now has an additional original-capture association.
`AccountPrincipal` commits to the resolved UID, primary GID, account name and
canonical home in a versioned, domain-separated SHA-256 value. The protected
manifest seals this optional association alongside its records. NSS names and
home strings are not copied into the association, and diagnostics redact its
commitment. This is an equality check rather than an authorization token or a
privacy guarantee against guessing account identities. Account recreation with
all four identical identity fields cannot be detected by this association.
`AccountStateRoot::capture_account` repeats account/root validation before
providing installed capture with the coherent UID/GID/principal tuple. Installed
capture seals that original principal with its exact captured records, including
absences; the generic trusted low-level capture API supplies no association.

`prepare_operator_generation` first applies the operator policy, resolves only
the kernel-authenticated peer, and returns `AccountGenerationAccess` only when
the protected verified generation carries the matching original association.
All per-user roots require it, including generations consisting entirely of
absent files. Legacy generations without the field remain inspectable and
retain their original canonical bytes; they are not enrolled by later account
observations. System-only generations require a separate privileged policy.
The guard repeats NSS/directory and manifest-seal checks before staging or a
trusted preparation/apply handoff. Replaced home/configuration directories or
changed NSS identity invalidate the pinned account. Errors expose neither NSS
fields nor the commitment.

This guard does not enable installed helper requests or replace operation
authorization, protected state-mount validation, writer freezing, checkpoint
verification, or recovery scope policy for system records in a mixed generation.
The forthcoming executor must use the guard at each preparation/stage/apply
boundary while retaining its coordinator lock and writer exclusion. Existing
low-level recovery functions remain trusted primitives. Focused tests cover
UID/GID/name/home mismatches, both absent-only per-user roots, legacy refusal,
changed seals, stale-account refusal before callbacks, and directory replacement.

## Protocol

`ryoku-service` defines protocol version 3 in Rust. Each envelope carries a validated bounded request ID and one typed request: capability discovery, durable operation lookup, local-profile read/write, read-only linked-checkpoint verification, or a named managed action. The decoder bounds input to 64 KiB, rejects unknown required fields and versions, and validates recovery, package, profile, checkpoint, request, and operation identifiers during decoding. Responses include the typed local profile, verified linked-checkpoint identities, durable operation state, or structured errors. Operation phases follow `accepted → capturing → applying → verifying → succeeded`, with failure exits from each active phase; active phases can persist monotonic progress updates before advancing. `OperationStatus::is_valid` is the shared check for phase/error/progress consistency; a succeeded operation with a known total must complete all reported steps, and the journal validates this across the final phase transition. Reconnecting clients reject invalid statuses as well as wrong operation IDs. The operation journal keeps its independent v1 status and v2 mutation-context record formats.

Local profile reads and ordinary personal-setting writes stay in the
authenticated unprivileged user daemon. An absent file reads as built-in
defaults without creating a file. `SetLocalProfile` atomically writes only the
fixed typed personal settings document beneath the authenticated account root;
it uses no root helper, machine checkpoint, or operation journal entry. The
document remains in the declared recovery inventory for later linked
recovery operations. `MigrateLegacyLocalProfile` accepts only the exact
canonical schema-1 document emitted by the old Rust prototype, atomically
creates the new personal settings file without replacing an existing one, and
preserves the source. A retry is successful only when the destination already
matches that exact imported document; conflicting or unsafe destinations are
refused. This same-user settings migration uses no privileged helper or machine
checkpoint. System/companion profile imports and protected profile activation
remain unavailable. The root helper remains an independent narrower protocol.

There is no generic command request. Helper protocol version 2 has a bounded envelope and four typed request variants: non-mutating action availability, linked-checkpoint verification, package transaction, and checkpoint restore. These requests carry validated transaction/generation references and typed package/checkpoint IDs. Unknown fields, unsupported versions, malformed IDs, empty/oversized/duplicate package lists, and request or response frames over 64 KiB are rejected before policy dispatch or service use. There are no paths, executable names, argv arrays, shell strings, environment variables, or arbitrary provider names. An authorized operator can request read-only verification of the exact generation, its protected Snapper catalog row, and the still-present single snapshot carrying the matching transaction tag. A missing, stale, duplicated, or mismatched partner is unavailable; the response includes verified generation and snapshot identities only after all three checks pass. The per-user service exposes that verifier without journaling it as a mutation. Availability, package, and restore requests still return `unsupported`; callers outside `ryoku-operators` and UID 0 are denied for helper requests. The service reports `recovery_unavailable` when the helper is missing or its response cannot be verified. It does not invoke a provider.

Both helper socket handlers obtain kernel peer credentials and apply their UID/group policy before reading request bytes. An unauthorized peer receives a typed `unauthorized` response with the fixed correlation ID `invalid`; its request is neither parsed nor reflected. Authorized peers are subject to bounded framing and one absolute five-second deadline. Helper clients use the same deadline from nonblocking connect through request and response framing, and the server uses it for request and response I/O. A slow peer cannot extend the deadline by sending or receiving one byte at a time. The systemd socket is fixed at `/run/ryoku/helper.sock`, owned by root and group `ryoku-operators`, mode `0660`; administrators opt users into this group manually. The systemd-activated root helper verifies the inherited descriptor's pid/count and pathname before accepting connections. It never creates an operation record or invokes a provider. Invalid complete frames from authorized peers receive a generic malformed/unsupported rejection with a fixed correlation ID; truncated/closed peers are dropped without attempting to write to a disconnected socket.

The separate `/run/ryoku/recovery-gate.sock` is a fixed, read-only session
check. It accepts only the versioned literal `RYOKU-RECOVERY-GATE/1` request
from a non-root local peer and returns `clear`, `recovery-required`, or
`unavailable`; it exposes no attempt IDs, paths, or user configuration. The
root service reopens the machine identity and state mount and rescans the
durable restore journal on every query. It first takes the common mutation lock
shared and scans without the journal's exclusive flock; contention returns
`recovery-required` instead of holding up session startup. The Ryoku session
launcher verifies that the socket peer is root and requires `clear` before
starting Hyprland;
the shell user unit repeats the check before starting the upstream daemon.
Missing state, malformed responses, or pending attempts refuse the Ryoku
session; KDE remains selectable. The socket is world-connectable solely for
this low-information query and is independent from the operator helper socket.
An administrator must initialize recovery state and enable
`ryoku-recovery-gate.socket`. Native systemd activation and failure behavior
still require Tumbleweed validation. Client connect, request framing, and
response reads share one five-second deadline; the server uses nonblocking
poll-based request parsing and response writes under one deadline as well.
Native socket transport and systemd activation remain unverified in this
continuation.

The shared lock is held only for the journal check and is released before the
one-shot response. A long-lived session lock would deadlock supported online
operations that need the same lock exclusively. Those operations must stop
their affected writers and keep the exclusive guard for the complete
checkpoint/mutation/verification/journal sequence. Root rollback remains an
offline-console operation. The gate still does not establish a complete online
restore coordinator; native systemd activation and lock behavior require
Tumbleweed validation.

## Authentication, runtime, and persistence

The root helper and recovery-gate units use `ProtectSystem=strict` without a
writable state-store exception, empty capability sets,
`NoNewPrivileges=yes`, `PrivateDevices=yes`, read-only kernel tunables/modules
and control groups, and `RestrictSUIDSGID=yes`. Their current operations only
read protected recovery state. Future capture and restore operations require
a separately reviewed unit change with write access limited to the protected
state mount. These unit restrictions have not been validated on the pinned
Tumbleweed guest; helper RPM installation remains untested.

The native collector uses its own namespace's descriptor mount ID to select
the matching mountinfo row; it does not compare against a persisted host mount
ID. A bind/remount of the exact subvolume root at the fixed state path must
still satisfy root ownership/mode, inode 256, a writable Btrfs mount, the
machine-record filesystem UUID, subvolume ID and direct-top-level parent ID.
The exception changes none of those checks, cannot make an underlying
read-only filesystem writable, and does not admit a missing or substituted
volume. `ProtectHome=read-only` and all other helper hardening remain unchanged;
future account capture integration must separately prove its namespace access.

On Linux, `PeerIdentity::from_unix_socket` reads UID, primary GID, and supplementary GIDs from the connected socket using `SO_PEERCRED` through rustix and `SO_PEERGROUPS` through a bounded getsockopt call. Helper policy compares those kernel credentials with the installed `ryoku-operators` GID parsed from `/etc/group`; it rejects root and any non-member even if the helper is reached through another local socket. The socket's filesystem mode independently gates access to the opt-in group. The caller UID remains available for future per-user scoping. The user daemon forwards only a typed `CheckAvailability` probe over the fixed socket. The helper accepts no user paths and does not open user configuration. Group membership is an explicit administrator decision and is not granted by package installation.

`OperationJournal` replays bounded, versioned newline JSON records from a caller-supplied already-validated file descriptor. Each legal state transition is synchronized before its in-memory state changes, so a reconnect can query the last durable state without resubmitting the action. Capturing, Applying and Verifying may each persist multiple progress updates; a phase update cannot reduce completed work, erase progress, or change a known total. An unknown total may become known. Writes and replay both reject impossible progress (`completed > total`) and invalid phase/error combinations; invalid caller-supplied status is rejected before it reaches disk.

Before forwarding any new mutating `Start` request, the user service checks the
replayed operation journal for unresolved work. Accepted/in-progress operations
and failures that may have changed state block new starts with
`recovery_unavailable` without contacting the helper. A failure durably
classified as pre-change does not block future work; read-only linked-checkpoint
verification remains available for diagnosis. This prevents an unreviewed
second operation from starting beside work that requires explicit
reconciliation. It does not enable any mutating action or provide automatic
resume.

The executable user daemon selects `/run/user/<kernel-effective-UID>` and the
NSS account's default `.local/state/ryoku/service/operations.log`. It ignores `HOME`,
`XDG_RUNTIME_DIR` and `XDG_STATE_HOME` for these selections. Root and mismatched
real/effective UID or GID are refused. Nondefault XDG locations require a future
trusted mapping. NSS lookup uses the same bounded worker as account-bound
configuration roots, independently of whether `.config/ryoku` exists.

Runtime and state directories are opened with `openat2` and pinned descriptors;
symlinks, magic links, unsafe ownership/modes and mount crossings below the
selected roots are refused. `/run` and `/run/user` must be root-owned, and the
UID's runtime directory must be mode `0700`. The fixed transitions into `/run`,
`/run/user`, and `/run/user/<UID>` permit independent native runtime mounts;
all descendant socket/lock opens retain the same-mount restriction. Home,
`.local`, `.local/state` and `.local/state/ryoku` permit owner-only writes with
modes `0700`, `0750` or `0755`. Upstream QML legitimately creates the shared
`ryoku` directory with `0755`; the native daemon's dedicated
`.local/state/ryoku/service` directory requires `0700`.
Existing safe shared directories and upstream data retain their modes
during opening and revalidation. Missing state descendants are created with
`mkdirat(0700)` and parent directories are synchronized. Existing unsafe entries
are not repaired.
The journal opens relative to the pinned state directory with no-follow and
nonblocking flags. `fstat` must prove a single-linked, same-UID regular file of
mode `0600` before replay. Symlinks, hard links, FIFOs and device/directory
substitutions cannot redirect or block journal replay. The containing directory
is synchronized before use.

Under the existing daemon lifetime lock, startup migrates a sole legacy
`.local/state/ryoku/operations.log` into the private `service` child. It first
validates and synchronizes the legacy descriptor, then uses an anchored atomic
`renameat2(RENAME_NOREPLACE)` and verifies the destination against that pinned
inode. Both source and destination directories are synchronized before replay.
The rename preserves the file's inode and durable history; no copying, chmod,
truncation or history selection is used for migration. Both journal leaves
existing is a startup error even if either is empty. Invalid legacy or current
entries, including dangling symlinks, fail closed. A fresh journal is created
exclusively only when both leaves are absent. Restart before the rename sees
the legacy leaf; restart after the rename sees the current leaf and synchronizes
both directories again. Corrupt history remains a replay error and never causes
a fresh-journal fallback. Revalidation checks all pinned ancestor identities,
the private service directory and journal metadata, and absence of a new legacy
leaf before further requests. UID and GID need not be equal; ownership checks
continue to use the kernel UID.

The ordinary user unit explicitly sets `PrivateTmp=no` and `PrivateUsers=no`
and requests no filesystem namespace isolation. In the retained Run18 diagnostic
overlay, an equivalent `PrivateTmp=yes` transient user unit had only the
account's own UID/GID mapped. It observed the native root-owned `/run` and
`/run/user` as `65534:65534`, while its own runtime directory retained the
account UID/GID and mode `0700`. This matches the
[systemd user-namespace contract](https://github.com/systemd/systemd/blob/v261/man/systemd.exec.xml):
an unprivileged manager implicitly needs a user namespace for private mounts.
The daemon therefore rejected startup at its strict root-owner check; overflow
IDs cannot establish root ownership. Run19 subsequently passed native activation
of the corrected user unit with two ordinary accounts.

The production daemon path uses no shared temporary files or temporary-directory
API: its socket, lifetime lock and journal use the fixed, validated private
roots above. It launches no child program or provider. The unit retains
`NoNewPrivileges=yes`, `UMask=0077` and `RestrictAddressFamilies=AF_UNIX`;
all descriptor, ownership and peer-credential checks remain in force. Other
processes with the same UID retain their existing access to shared temporary
directories; the unit does not claim to isolate them. The privileged helper's
filesystem and user-namespace hardening is unchanged.

The daemon holds an exclusive, nonblocking lock on the private same-UID regular
`/run/user/<UID>/ryoku-service.lock` for its entire lifetime. This prevents
duplicate cooperating daemons before journal replay or socket cleanup; it is
separate from the protected shared mutation lock. Socket bind and stale probes
use the held runtime descriptor through Linux `/proc/self/fd`, and permissions
are set on a pinned socket inode, so a substituted leaf cannot redirect chmod.
Only an owned single-linked socket whose nonblocking probe returns
`ECONNREFUSED` can be removed; its identity is checked again in the same pinned
namespace immediately before unlink. Other probe failures preserve the entry.
The published socket must be mode `0600`.

Before and after journal replay, and after kernel peer authentication both
before parsing and immediately before dispatch, the daemon re-resolves NSS and
reopens the published runtime/home/state paths. It compares every pinned inode,
including all state ancestors, the lock, journal and socket, and rechecks their
metadata. Replacing an ancestor while retaining its original descendants does
not preserve the binding. An observed namespace change returns a fixed
`recovery_unavailable` error without exposing paths/account data or consulting
the journal/helper. The checks do not isolate mutually hostile processes with
the same UID, which already share authority over their own files.

The daemon serves one bounded request per connection. Its nonblocking frame
read and response write share one five-second deadline, so a slow client cannot
extend repeated per-read timeouts. Discovery, durable status lookup, and
read-only linked-checkpoint verification are available. `Start` requests
probe helper availability and still receive structured rejections without
being journaled as accepted. A
focused reconnect test reopens a synced journal and queries that replayed
status through the request handler. The Rust UI service client also exposes a
read-only `get_operation_status` call that sends `GetOperation`, validates the
response version, request correlation, and returned operation identity. It
preserves typed service failures such as malformed, stale, busy, unavailable,
and missing (`NotFound`) through fixed UI error variants without copying
server-provided detail; it never resubmits or restarts an action. This is a
client API, not a graphical reconnect control or an accepted mutation flow. Native
temporary fixtures cover unsafe entries, expected-UID mismatches, ancestor/file
replacement, pinned-inode chmod, live/stale sockets and daemon-lock contention.
Run19 separately passed native direct-payload activation and access checks with
two real accounts. The packaged user unit and privileged socket remain disabled
by default; helper RPM build/install and interactive login remain unproven.

The explicitly ignored read-only test
`native_runtime_mount_boundaries_preserve_strict_ownership_and_fixed_namespace`
passed on the authorized native WSL namespace with both
`native_runtime_run_to_user_mount_boundary=true` and
`native_runtime_user_to_uid_mount_boundary=true`, plus fixed-namespace and
owner/mode checks passing. It reopens the actual fixed runtime directory and
confirms the expected-UID mismatch is refused without creating a lock, socket
or journal. This proves acceptance of those legitimate mount boundaries; it
does not establish a successful daemon startup or multi-user isolation.

`cargo build --locked -p ryoku-service --example native_user_service_probe`
builds a read-only native client for an operator-started daemon. Run
`target/debug/examples/native_user_service_probe --run` as the ordinary user.
The probe accepts no endpoint override, connects only to
`/run/user/<kernel-UID>/ryoku-service.sock`, checks the server's kernel UID and
uses separate bounded, five-second exchanges for typed `Discover` and
`GetOperation` with a fixed missing operation ID. It requires the current empty
capability list, `recovery_unavailable`, exact response correlations and
`not_found`. It neither starts a daemon nor sends a managed action, touches
accounts, calls the helper or creates journal records. Running it again after
an operator-controlled daemon restart checks a fresh connection without
inventing an accepted provider transaction. Its fixed pass/fail output contains
no account, path or operation contents. This client is validation tooling; it
does not by itself establish a successful native run or multi-user isolation.

The [actual native WSL service evidence](../infra/tumbleweed-lab/evidence/issue17-wsl-native-service.toml),
recorded in commit `4423640`, proves direct executable startup with decoy XDG
values ignored, authenticated typed discovery and `NotFound` responses,
duplicate-daemon refusal while the original still answered, and both client
exchanges after a fresh-process restart. The empty journal's digest remained
unchanged, and both task-owned daemon processes were stopped. RPM installation,
systemd activation, multi-user isolation, helper execution and reconnecting to
an accepted transaction remain unproven by this evidence.

The guarded [disposable native service fixture](native-service-fixture.md)
passed in [fresh console Run19](../infra/tumbleweed-lab/evidence/issue17-run19-native-services.toml)
from frozen source `6b1d195`, under enforcing
SELinux with two real NSS accounts and native systemd/logind user managers.
Actual typed discovery/`NotFound`, opt-in helper rejection, malformed operator
frames, root rejection, original-mode DAC denial and separately assisted kernel
UID/group denials passed. Modes were restored before authorized rechecks;
duplicate refusal and fresh-process restart preserved empty journal and lock
inodes while replacing socket inodes. Task units were stopped and linger
disabled. This proves direct fixture payload activation, not helper RPM
installation, interactive login or accepted-operation replay.

The per-user daemon also checks the connecting peer's kernel UID before parsing a request. A UID mismatch receives `unauthorized` with the fixed correlation ID `invalid`, without consulting the journal or reflecting attacker-controlled request fields. The caller must ensure the journal file is on durable state storage and protected by one exclusive writer. The production mutation coordinator acquires `MutationLock`, refreshes the shared journal and refuses unresolved prior work before acceptance. It synchronizes a detailed plan/checkpoint context alongside every phase before its corresponding hook; native capture and authorization remain trusted integration boundaries. If the lock, plan or journal cannot be validated, no new operation is accepted. The reduced and unjournaled gate helpers are compiled only for unit tests. `MutationLock` is opened from a trusted service-owned private state-directory descriptor, validates a fixed `mutation.lock` file, and opens that same inode afresh for every operation. Its exclusive `flock` is released when the operation ends or the process exits. All cooperating Ryoku writers must use the same protected journal and trusted lock directory; the advisory lock cannot constrain uncooperative tools or provider-internal writers. The detailed [mutation state machine](mutation-coordination.md) documents durable schema 2, exact associations, cancellation, failure classifications and restart refusal.

The coordinator does not create or validate `/var/lib/ryoku-state`, implement checkpoint creation/read-back verification, coordinate provider-internal writers, or implement rollback. A crash preserves the last synchronized phase and requires explicit reconciliation before new work; it never automatically repeats apply. Instrumented tests cover process exit at every hook, independent stale journal views under the common lock, and pre-change versus potentially-mutating/post-capture failures. They do not establish installed helper/console coordination or external-writer exclusion. `MutationIntegration` remains a trusted integration boundary; caller-provided booleans are not independent evidence. No installed provider action is enabled by this module.

Discovery advertises no mutation actions in this build. Known action names remain typed so the UI can present structured unsupported errors without gaining provider authority. The service now probes the root helper with a typed availability request during `Discover`: `Unsupported` means the helper and protected-state initialization are healthy while writes remain disabled; transport, authorization, or recovery-state failures map to `recovery_unavailable`. A future executable service must report only capabilities it can actually perform after checking recovery storage and helper availability.

## Acceptance gaps

- Prove helper RPM build/install and packaged activation on Tumbleweed; Run19 passed direct fixture payload activation, two-account access checks and process restart/reconnect.
- Add helper request replay protection and independently review the installed privilege boundary. This implementation uses the opt-in operator group with per-user kernel UID scoping, not a dedicated service UID.
- Extend native authorization checks to stale/replayed references and enabled capture/provider operations. Run19 passed multiple real UIDs, group membership, root and malformed/unsupported read-only requests with empty operation journals.
- Integrate a provider and #16's verified linked capture/publication so an accepted operation survives client disconnect and can resume/query actual durable progress.
- Connect the coordinator to durable operation phases, authorization, complete #16 capture/publication, low-space checks, cancellation boundaries, provider-level writer coordination, and post-change capture; test interruptions and recovery.
- Exercise process restart, corruption, locking, interrupted journal writes, and user configuration separation in focused tests and the disposable Tumbleweed VM.

Until those gaps pass, #17 remains open and all managed writes remain disabled.
