# Provider capabilities and recovery hooks

Status: feasibility model for M0-06, 2026-10-13. This document records the initial candidate set and the evidence still needed. It does **not** certify any provider as protected or mark issue #15 complete.

## Release boundary

The native Ryoku UI is the primary interface. Cockpit, Myrlyn and individual YaST modules are optional and independently selectable. Cockpit remote listening is a separate opt-in; installing its local desktop shell must not enable a network listener by default. Use explicit supported operations; do not present an administrative application as a general-purpose way to make protected changes.

Every supported write must follow one Rust-owned operation path:

1. Validate the request and authorize the caller for that operation.
2. Inventory all affected system and Ryoku-owned state.
3. Capture and verify the linked pre-change checkpoint: matching Snapper system state plus complete declared Ryoku, configuration and profile state.
4. Start the provider transaction only after checkpoint verification succeeds.
5. Keep the accepted operation under the service's durable job owner if its UI/client disconnects. Report progress, cancellation capability and terminal outcome separately.
6. Verify the provider's final state, record it in the recovery journal, and publish the result.

A checkpoint when an app opens is insufficient. If an interface can commit writes without entering this path, those writes are **unsupported** for the protected product workflow. A failed, unavailable or ambiguous capture must prevent the write. Cancellation before commit may stop a request; after an irreversible commit begins, cancellation must not be described as rollback. The worker must observe provider completion and report success, failure or indeterminate outcome. On service restart, reconcile an indeterminate job before another write can start.

The affected-state inventory is operation-specific. Package transactions include package database, installed RPM payload/configuration, scripts and service effects, boot/kernel/initramfs artifacts, repository/package-manager state, and Ryoku's own catalog/generation/journal changes. Repository edits include repo definitions, trust/key material, priorities, vendor/config policy, metadata cache, and any imported signing-key state. A system configuration write includes the exact files, linked services and persistent state that the module may mutate. If the set cannot be bounded and restored, that operation is unsupported.

## Capability and integration matrix

The status labels below describe the evidence available in this repository, not upstream product quality:

- **Verified protected** — pre/post hook placement and the affected-state inventory were demonstrated on the supported Tumbleweed VM, and authorization, failure and recovery behavior were checked.
- **Read-only** — the operation cannot mutate state through the exposed interface; its data may still be unavailable.
- **Unsupported** — the product will not offer the operation in the first release.
- **Awaiting proof** — a plausible interface exists, but its precise contract, hook placement or Tumbleweed behavior has not been demonstrated.

At this revision there are **no verified protected provider writes**. Run17 confirms the base Tumbleweed migration, offline restore and reset lifecycle, while the KDE login gate remains blocked. The provider hook experiments below have not been run. Do not infer acceptance from a command's existence or a successful package installation.

| Operation/candidate | Interface class | Current status | Privilege, errors and cancellation | Rust path and hook assessment |
|---|---|---|---|---|
| Show product release, current profile, recovery availability and completed job history | Native Ryoku controls | Awaiting proof (read-only contract) | Unprivileged read from validated, versioned service data; missing/stale/corrupt data must be shown as unavailable/error, never guessed. | Typed Rust backend query. No provider write hook. Data-source and privacy review remains. |
| Search package metadata and inspect installed package/repository state | Native Ryoku controls; optionally present through a local panel | Awaiting proof; candidate read-only | Read-only metadata queries should not obtain root privileges. Report stale metadata and provider errors. | Rust provider adapter. Prove query flags do not refresh metadata or mutate state on the pinned Tumbleweed snapshot. |
| Install/remove/update packages, including a full Tumbleweed update | Native Ryoku controls | Awaiting proof; only a reviewed full-system transaction is a candidate | Privileged, operation-specific authorization. Validate exact requested package IDs and policy before authorization. Provider dependency/conflict/signature failures are failures; do not continue partially. Pre-commit cancellation may abort; post-commit cancellation is not rollback. Preserve durable job outcome across client exit. | Rust service owns the transaction and durable job; candidate interfaces are zypper CLI or PackageKit over D-Bus. Neither path has been proven to expose a transaction boundary that guarantees the linked checkpoint immediately before every commit. Do not run a shell command from UI or authorize arbitrary zypper arguments. Direct package clients bypass the Ryoku checkpoint. |
| Add/change/enable/disable repositories, alter priorities, import/remove trust keys | Native Ryoku controls | Unsupported for first release pending separate proof | Root/polkit privilege; repository and trust failures must be explicit. Do not offer arbitrary URLs or trust-on-first-use. | Rust service only after exact scope and hook is proven. Repository writes change future package trust and resolution, and must checkpoint before each mutation; capturing only before a later install does not protect them. External YaST/zypper/Cockpit editors bypass the path. |
| Snapper list/read-only status | Native Ryoku controls | Awaiting proof (read-only path) | Read through the least-privileged supported interface; distinguish empty, unavailable and denied. | Rust query adapter; verify D-Bus/CLI semantics in the VM. |
| Create/delete/modify Snapper snapshots or configuration | Native Ryoku controls | Unsupported as general administration; Ryoku recovery operations are separate | Privileged and constrained to authenticated recovery workflow. Destructive errors must preserve the last usable recovery point. | Narrow Rust recovery service operation only. Arbitrary snapshot deletion/config edits can invalidate linked recovery and are not an ordinary provider action. |
| Change selected system settings (initial candidate: a small, enumerated set of display/session preferences) | Native Ryoku controls | Unsupported until each setting has an affected-state inventory and restore proof | Per-operation authorization; validate values and report validation/service-reload failures. Do not accept arbitrary file paths or shell fragments. | Rust typed operation and declared config ownership. No generic “settings provider” hook. |
| Browse/manage software through Myrlyn | Embedded panel if a supported local embedding contract is verified; otherwise externally launched interface | Unsupported for writes in first release; read-only embedding awaiting proof | Myrlyn is a separate privileged-capable package manager UI. Its authorization prompts and cancellation lifecycle must be tested, not inherited by assumption. | Myrlyn is not a Rust service API. Do not embed it and label its writes protected. A launch-time checkpoint is invalid. A supported embedding must disable/route all commit paths through the Rust operation service or remain read-only. |
| Cockpit package panel (openSUSE cockpit-packages) | Embedded panel only if the upstream Cockpit embedding/navigation contract fits; otherwise externally launched interface | Awaiting proof; protected writes unsupported | The openSUSE module uses PackageKit D-Bus. Confirm its polkit action, transaction errors and cancellation behavior in the target snapshot. | The D-Bus transaction path is a concrete candidate to inspect, not evidence that it runs the Ryoku checkpoint. Each commit must be mediated before mutation or this remains unsupported. |
| Cockpit overview / service and host panels | Embedded panel only if the upstream Cockpit embedding/navigation contract and local-only mode fit; otherwise externally launched interface | Awaiting proof for read-only navigation; unsupported for protected writes | Cockpit may invoke privileged system services through its own authorization model. Its UI's error and cancellation behavior does not provide the Ryoku recovery guarantee. | Cockpit uses its documented frontend/backend and service boundaries; there is no assumed generic Cockpit REST API. A panel is protected only if each commit reaches the Rust service before mutation. Remote listening remains separately opt-in and off by default. |
| YaST software, repository or system-configuration modules | Externally launched optional interface; individual modules may be excluded | Unsupported for protected changes in first release; read-only query candidates require module-by-module proof | YaST modules can perform privileged writes. Do not assume module-level privilege or cancellation prompts satisfy product authorization/recovery. | YaST's established module ecosystem is Ruby-based. New project-owned Ruby glue/bindings are deferred. External modules cannot be wrapped by a UI launch hook; omit write-capable modules from the supported Ryoku path until their writes can be mediated per operation. |

## Candidate libzypp commit gate (awaiting Tumbleweed proof)

The current source-level candidate is libzypp's commit-plugin protocol, not an
application-launch notification. Its documented `PLUGINBEGIN`/`COMMITBEGIN`
exchange occurs around `ZYpp::commit`; `COMMITBEGIN` includes planned
transaction steps before installation starts. However, the inspected upstream
`PluginExecutor::send()` implementation accepts ACK/ENOMETHOD and removes a
plugin after another reply, then continues broadcasting. It is therefore not
established that returning ERROR vetoes the package commit. This is a critical
open question: a callback before the RPM actions is useful only if a timeout,
plugin failure, unavailable broker, or negative checkpoint actually prevents
the enclosing transaction. Do not treat a Rust plugin as a fail-closed gate
until a disposable guest proves that property for the exact installed libzypp.

If the native plugin cannot abort the transaction, it may still provide
transaction telemetry but is not the authorization/recovery boundary. The
product must leave that provider path unsupported until another pre-commit
mechanism reliably stops zypper, PackageKit, and Cockpit writes. A small
root-owned Rust executable could request the linked checkpoint and acknowledge
only after the service confirms capture, but this is only a hypothesis; no
plugin has been installed or tested here. The package version, configured
plugin directory, protocol version, ordering, timeout behavior, cancellation
semantics, and PackageKit backend in the pinned snapshot remain unverified.

The next disposable-guest experiment must record exact `zypper`, `libzypp`,
PackageKit, PackageKit zypp backend, Cockpit and Myrlyn versions and package
ownership; inspect the actual libzypp plugin path; then install a Rust probe that
rejects `PLUGINBEGIN` or `COMMITBEGIN`. Attempt a harmless zypper transaction
and a PackageKit transaction separately and measure whether ERROR, no ACK,
malformed reply, or timeout actually prevents commit and leaves RPM state
unchanged. Do not infer veto from the callback occurring. If libzypp continues,
record the exact bypass and mark this path unsupported. Only if native behavior
reliably blocks writes should a checkpoint broker be tested for negative
checkpoint, timeout and success; compare capture ID, transaction steps, and
installed package state. Confirm
whether Cockpit uses that same PackageKit backend in the guest. Treat direct
RPM, repository-definition, and trust-key writes as bypasses until separate
pre-mutation controls are demonstrated. A libzypp commit hook cannot be claimed
to cover operations that do not pass through that commit path.

References for experiment design: [libzypp commit plugin protocol](https://github.com/openSUSE/libzypp/blob/master/zypp/doc/autoinclude/Plugin-Commit.doc),
[libzypp plugin executor](https://github.com/openSUSE/libzypp/blob/master/zypp/PluginExecutor.cc),
[PackageKit zypp backend](https://github.com/PackageKit/PackageKit/blob/main/backends/zypp/pk-backend-zypp.cpp),
and the [PackageKit transaction API](https://github.com/PackageKit/PackageKit/blob/main/src/org.freedesktop.packagekit1.Transaction.xml).
These moving source references establish a candidate only; each must be pinned
to and checked against the actual guest packages before implementation.

“Embedded” is an integration choice, not a protection property. A panel that can submit writes remains a provider bypass unless those exact writes are mediated. Where the upstream integration cannot meet this boundary, use an external launch only for unsupported/admin use and clearly mark it outside the protected controls.

## Provider boundary decision

The first implementation candidate is a small native Rust control surface backed by one Rust service. It should use a typed, allowlisted operation API, a durable job identifier and typed states such as Queued, Authorizing, Capturing, Running, Verifying, Succeeded, Failed and OutcomeUnknown. These are design names, not implemented types. Cancellation should be typed separately as NotStarted, Requested, Accepted, TooLate or Unsupported; never map a canceled client connection to a canceled package transaction.

A reviewed adapter may invoke a fixed zypper command or a supported D-Bus provider method after the checkpoint. The exact interface remains **awaiting proof**. The adapter must not interpolate arbitrary user text into a command, accept arbitrary arguments, expose a shell, or silently retry a write. PackageKit, where installed, is a possible standardized package-management boundary, not proof that every repository/config operation participates in the Ryoku checkpoint. libzypp is implemented outside this Rust workspace; adding a direct FFI bridge is not approved by this feasibility document.

Myrlyn, Cockpit and YaST are not alternate owners of the recovery contract. If future proof shows their write path cannot be intercepted before each mutation, they remain external optional administration interfaces and their mutations are outside the protected first-release operation set.

## Minimal live hook experiments required

Run these against the supported disposable Tumbleweed guest and record exact package/provider versions, command/API, relevant service state, redacted logs and before/after state. Do not use the production host. The existing lab run is infrastructure evidence, not a provider-hook experiment.

1. **Transaction boundary:** invoke the candidate native Rust operation for one harmless package already available in the frozen repository snapshot (or a no-op plan followed by a disposable transaction). Record authorization, linked pre-change capture ID, provider transaction/job ID, commit start/completion, final RPM state and post-state verification. Inject checkpoint failure and show that the provider is never called.
2. **Bypass check:** perform the same transaction through direct zypper, PackageKit, Myrlyn, Cockpit software and YaST software interfaces one at a time. Determine whether any documented supported hook reliably prevents commit until the Ryoku checkpoint is verified. A before/after notification, app launch, package script, or desktop prompt is not a pre-commit gate.
3. **Cancellation and client loss:** close the client before provider start, request cancellation before commit, then disconnect after commit begins. Verify no write before checkpoint, accurate cancellation reporting, persistent job status and recovery after service restart. Never claim that killing the client or provider process reverses a committed transaction.
4. **Repository/trust operation:** use a disposable local repository fixture. Prove the pre-change checkpoint precedes each repo definition, priority, enablement and trust-key mutation; verify failure blocks the write and recovery restores the affected definition plus trust state.
5. **Selected config operation:** choose one explicitly bounded setting. Capture its complete affected-state inventory; inject checkpoint failure; perform the write; verify post-state, service reload/error behavior, restore, and preservation of unrelated home files.
6. **UI integration:** for each optional panel, verify navigation and authorization in a local graphical session; inspect the actual write request path. Confirm Cockpit is not remotely listening unless the user separately opts in. Test whether the interface can be constrained to read-only; do not infer this from its visual presentation.

A candidate moves to **verified protected** only after the matching experiments pass and evidence is linked. Failed or unavailable hooks move that operation to **unsupported**. Missing UI/VM availability leaves the item **awaiting proof**.

## Interface references

These references describe upstream interfaces; they are not evidence that this port has integrated or validated them.

- [zypper manual (openSUSE documentation)](https://en.opensuse.org/SDB:Zypper_manual) — CLI operations and options. openSUSE Wiki requests may be blocked in some automated environments; consult the current supported manual and guest's zypper --help/man page before implementing.
- [libzypp project](https://github.com/openSUSE/libzypp) — native package-management library and source. Its existence does not prove stable Rust bindings or the required transaction hook.
- [PackageKit API](https://www.freedesktop.org/software/PackageKit/gtk-doc/) and [PackageKit D-Bus API](https://www.freedesktop.org/software/PackageKit/docs/introduction.html) — standardized package operation interfaces. Confirm the Tumbleweed backend/version, authorization and exact transaction behavior in the guest.
- [Myrlyn source](https://github.com/shundhammer/myrlyn) and [openSUSE news on its origin](https://news.opensuse.org/2024/12/20/new-pkg-mgmt-tool-debuts/) — package-manager project reference and background. Its actual Tumbleweed build/version, write surface and embedding feasibility still require guest inspection.
- [Cockpit embedding guide](https://cockpit-project.org/guide/latest/embedding.html) — documented embedding/navigation constraints.
- [Cockpit packages guide](https://cockpit-project.org/guide/latest/packages.html) — Cockpit package/frontend integration model.
- [Cockpit privilege guide](https://cockpit-project.org/guide/latest/privileges.html) — privilege handling at Cockpit's interfaces; it does not mediate the Ryoku checkpoint.
- [Cockpit project](https://github.com/cockpit-project/cockpit) — frontend/backend project and source. Do not invent a generic REST endpoint.
- [openSUSE cockpit-packages module](https://github.com/openSUSE/cockpit-packages) — concrete package panel using PackageKit D-Bus; its operation hooks remain unverified here.
- [YaST project](https://github.com/yast/yast-yast2) — module framework/source. Each candidate module must be checked independently for implementation language, write set and hook behavior.

All links and interface/version claims need revalidation against the target's pinned/current Tumbleweed snapshot before they are treated as implementation evidence.


The probe harness also exposes `missing-plugin` (runs without registering any probe executable) and `timeout-commit` (the probe deliberately withholds its COMMITBEGIN response). Every zypper transaction is launched through GNU coreutils `timeout` with a 45 second deadline and 5 second TERM-to-KILL grace period; unsupported timeout implementations are rejected before plugin registration. The timeout mode records the plugin PID and verifies it is gone after the bounded run. Each mode reports RPM state before and after. Denial and malformed-response outcomes require the probe to record `COMMITBEGIN`, zypper to fail before timeout, and RPM state to remain unchanged. Timeout mode requires a recorded `COMMITBEGIN`, the bounded timeout status, a terminated plugin PID, and unchanged RPM state. The missing-plugin case records whether the operation proceeded; if zypper succeeds without installing the requested package or fails before a real transaction, the result is marked inconclusive. Any changed RPM state is a failed safety result and requires discarding the disposable overlay. These remain characterization runs, not proof that the provider path is protected.
