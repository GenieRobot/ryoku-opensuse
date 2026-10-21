# Typed service and helper contract

Status: versioned Rust protocols, a runnable unprivileged per-user daemon, and a systemd socket-activated root helper for [issue #17](https://github.com/GenieRobot/ryoku-opensuse/issues/17), plus a fail-closed sequencing contract for [issue #20](https://github.com/GenieRobot/ryoku-opensuse/issues/20). The per-user service and helper units are packaged, but installed Tumbleweed activation, provider mutation, and recovery transactions are not yet proven or enabled.

## Protocol

`ryoku-service` defines protocol version 1 in Rust. Each envelope carries a validated bounded request ID and one typed request: capability discovery, durable operation lookup, or a named managed action. The decoder bounds input to 64 KiB, rejects unknown required fields and versions, and validates package, profile, checkpoint, request, and operation identifiers during deserialization. Responses distinguish acceptance, durable operation state, and structured errors. Operation phases are explicit, and journal transitions are restricted to `accepted → capturing → applying → verifying → succeeded`, with failure exits at each in-progress phase.

There is no generic command request. The helper protocol has a versioned, bounded envelope and four typed request variants: non-mutating action availability, linked-checkpoint verification, package transaction, and checkpoint restore. The latter three carry validated transaction/manifest references and typed package/checkpoint IDs. Unknown fields, unsupported versions, malformed IDs, empty/oversized/duplicate package lists, and frames over 64 KiB are rejected before policy dispatch. There are no paths, executable names, argv arrays, shell strings, environment variables, or arbitrary provider names. The helper currently returns only structured rejections: availability and provider requests return `unsupported` for an authorized operator; callers outside `ryoku-operators` and UID 0 are denied. It does not write an operation journal or invoke a provider.

Both helper socket handlers obtain kernel peer credentials and apply their UID/group policy before reading request bytes. An unauthorized peer receives a typed `unauthorized` response with the fixed correlation ID `invalid`; its request is neither parsed nor reflected. Authorized peers are subject to bounded framing and a five-second absolute deadline covering socket reads and writes. A peer cannot extend this deadline by trickling bytes. Helper connection establishment and response framing are also bounded, and helper responses require the matching protocol version and request ID. The systemd socket is fixed at `/run/ryoku/helper.sock`, owned by root and group `ryoku-operators`, mode `0660`; administrators opt users into this group manually. The systemd-activated root helper verifies the inherited descriptor's pid/count and pathname before accepting connections. It never creates an operation record or invokes a provider. Invalid complete frames from authorized peers receive a generic malformed/unsupported rejection with a fixed correlation ID; truncated/closed peers are dropped without attempting to write to a disconnected socket.

## Authentication, runtime, and persistence

On Linux, `PeerIdentity::from_unix_socket` reads UID, primary GID, and supplementary GIDs from the connected socket using `SO_PEERCRED` through rustix and `SO_PEERGROUPS` through a bounded getsockopt call. Helper policy compares those kernel credentials with the installed `ryoku-operators` GID parsed from `/etc/group`; it rejects root and any non-member even if the helper is reached through another local socket. The socket's filesystem mode independently gates access to the opt-in group. The caller UID remains available for future per-user scoping. The user daemon forwards only a typed `CheckAvailability` probe over the fixed socket. The helper accepts no user paths and does not open user configuration. Group membership is an explicit administrator decision and is not granted by package installation.

`OperationJournal` replays bounded, versioned newline JSON records from an already validated descriptor. Each legal state transition is synchronized before its in-memory state changes. Active phases can persist monotonic progress; a known total cannot change or disappear, and incomplete verification cannot become success. Invalid states are rejected before mutation. An append or sync failure blocks subsequent appends until replay establishes durable state. Before answering the next authenticated request, the runtime replays an uncertain journal, truncating only a torn final record; complete corrupt records produce `RecoveryUnavailable`. A reconnect returns that durable state without resubmitting work.

The executable derives `/run/user/<UID>/ryoku-service.sock` from its kernel identity and `.local/state/ryoku/service/operations.log` from its NSS home. Environment variables cannot redirect either path. The socket, daemon lock and journal are private `0600` files; the service state directory is `0700`. Safe shared ancestors remain unchanged. Descriptor-relative, no-follow opens reject symlinks, unexpected mounts, foreign ownership, aliases and unsafe permissions. A daemon-lifetime lock excludes duplicate startup before journal replay or stale-socket cleanup. Revalidation checks pinned directory and file identities before parsing and again before dispatch, so a substituted path returns a structured error before journal lookup. A legacy `ryoku/operations.log` migrates by a synchronized no-replace rename retaining its inode/history. Conflicting legacy/current journals fail closed.

Account lookup has a fixed buffer, a two-second response deadline and one process-wide worker. A stalled NSS module retains that worker slot until completion; late results cannot be reused. The helper account-preflight API checks operator authorization before resolving only that kernel UID's existing `.config/ryoku` directory. It accepts no account/path fields and grants no protected-generation access or mutation authority.

Discovery and durable status lookup are available. `Start` requests probe the helper and return its structured rejection, or `RecoveryUnavailable` if it cannot be reached; no action is accepted. Client socket lifetime does not own the operation journal. Focused Rust tests disconnect a status client, persist further progress, reopen the journal and reconnect to the same identity without restarting it. This establishes the journal/runtime boundary, not an enabled production executor. The user unit and privileged socket are installed but not enabled by the package. A live Tumbleweed VM install/start/reconnect test remains outstanding.

The per-user daemon also checks the connecting peer's kernel UID before parsing a request. A UID mismatch receives `unauthorized` with the fixed correlation ID `invalid`, without consulting the journal or reflecting attacker-controlled request fields. The caller must ensure the journal file is on durable state storage and protected by one exclusive writer. The production mutation coordinator acquires `MutationLock`, refreshes the shared journal and refuses unresolved prior work before acceptance. It synchronizes a detailed plan/checkpoint context alongside every phase before its corresponding hook; native capture and authorization remain trusted integration boundaries. If the lock, plan or journal cannot be validated, no new operation is accepted. The reduced and unjournaled gate helpers are compiled only for unit tests. `MutationLock` is opened from a trusted service-owned private state-directory descriptor, validates a fixed `mutation.lock` file, and opens that same inode afresh for every operation. Its exclusive `flock` is released when the operation ends or the process exits. All cooperating Ryoku writers must use the same protected journal and trusted lock directory; the advisory lock cannot constrain uncooperative tools or provider-internal writers. The detailed [mutation state machine](mutation-coordination.md) documents durable schema 2, exact associations, cancellation, failure classifications and restart refusal.

The native coordinator validates `/var/lib/ryoku-state` against the independent machine identity and retains `NativeCaptureSession`'s machine lock and registered account writer lease across pre/post native capture and mutation accounting. It binds the reviewed plan to the complete installed inventory, reopens original sealed/native partners and the retention catalog, and repeats authorization and plan/path checks before entering a trusted typed provider. State/space checks include a bounded unnamed allocation probe; no reservation or multi-subvolume atomicity is claimed. The authenticated helper API and root-only `ryoku-mutation-history` console share the fixed protected `mutations.log` and machine lock. Helper startup validates that same history. A crash preserves the last synchronized stage and blocks new work. Explicit root console reconciliation can seal only unfinished stages before Applying; potentially mutating work remains blocked for authorized recovery. Provider-specific authorization and internal writer coordination remain trusted provider obligations; the installed dispatcher advertises no provider actions and this component executes no restore. See the [native scope and reproduction contract](mutation-coordination.md).

Discovery advertises no mutation actions in this build. Known action names remain typed so the UI can present structured unsupported errors without gaining provider authority. A future executable service must report only capabilities it can actually perform after checking recovery storage and helper availability.

## Acceptance gaps

- Prove the packaged per-user service and root socket/service activation in a Tumbleweed VM, including process restart/reconnect. Descriptor-based path validation is covered by the focused Rust suite.
- Add helper request replay protection and independently review the installed privilege boundary. This implementation uses the opt-in operator group with per-user kernel UID scoping, not a dedicated service UID.
- Test authorization with multiple real UIDs/group membership, root, malformed/stale/unsupported requests, and verify denials leave system state unchanged.
- Integrate a provider and #16's verified linked capture/publication so an accepted operation survives client disconnect and can resume/query actual durable progress.
- Before enabling any provider, certify its typed plan, authorization and internal writer/transaction boundary against the native coordinator's linked checkpoint, cancellation and accounting contract. The #20 fixture actuator is not a certified provider.
- Exercise process restart, corruption, locking, interrupted journal writes, and user configuration separation in focused tests and the disposable Tumbleweed VM.

Until those gaps pass, #17 remains open and all managed writes remain disabled.

## Focused boundary evidence (18 October 2026)

The PR #147 review fixes require a successful `sync_all` after journal replay,
even when all records are complete. A failed append sync leaves the previous
durable view and write refusal intact until a later sync succeeds; process
restart applies the same rule. Regression tests inject repeated sync failures
after writing a complete record and verify that progress cannot be exposed or
extended until durability is established.

The user service passes its original absolute deadline into the helper probe,
reserving 250 ms for the correlated response. A stalled helper therefore produces
a typed `RecoveryUnavailable` within the service budget. If the response reserve
is already reached, the service does not connect to the helper.

The packaged root helper retains only `CAP_DAC_READ_SEARCH` so account preflight
can read and traverse an authorized account's private `0700` directories. It
has no DAC write override, ownership-changing or identity-switch capabilities;
`ProtectHome=read-only`, `ProtectSystem=strict`, empty ambient capabilities and
the existing kernel UID/group gates remain enforced by the unit/policy. The
package test pins the single capability assignment, and account tests preserve
private directory modes through binding/revalidation. Actual root execution
with this capability set remains unverified: the local runner requires a sudo
password. No native packaged-activation claim follows from these tests.

The [host evidence record](../infra/tumbleweed-lab/evidence/issue17-service-boundary-focused.toml)
records native socket and filesystem checks on the openSUSE Tumbleweed WSL runner,
using Rust/Cargo 1.99.0. Socket tests run outside the restricted execution sandbox
because the sandbox rejects Unix socket I/O and peer-credential queries. Tests
fail normally when those facilities are unavailable; none silently skip them.
The separately invoked native runtime test observes both native mount boundaries
and verifies the fixed namespace and ownership policy. No live VM, package
installation, privileged accepted transaction or provider mutation is claimed.


## Native installed boundary acceptance

The [two-account native fixture](native-service-fixture.md) exercises installed
systemd user services and the socket-activated helper under enforcing SELinux.
The user unit preserves native root identities by disabling private user/mount
namespaces; its runtime parents remain subject to strict ownership checks.
The helper retains only `CAP_DAC_READ_SEARCH`, read-only homes and a writable
`/var/lib/ryoku-state` exception needed by protected-state startup.

The fixture also tests replay of a nonempty durable journal after a fresh
service process and reconnecting clients, plus malformed, unsupported and stale
namespace rejection without journal mutation. A separate sandboxed account API
probe is explicitly distinguished from the production helper's disabled
provider dispatcher. Fixture results do not imply RPM installation, interactive
login, or real provider transaction acceptance.
