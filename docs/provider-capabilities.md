# Provider capabilities and recovery hooks

Status: feasibility model for M0-06, updated 2026-10-20. This document records the initial candidate set and the evidence still needed. It does **not** certify any provider as protected or mark issue #15 complete.

The [20 October disposable-guest experiments](provider-experiments-20261020.md)
now reach real `COMMITBEGIN`/`COMMITEND` notifications and demonstrate a package
write despite the plugin's `ERROR` response. Repository edits and a static
hostname change also bypass that plugin. These are negative feasibility results;
the [evidence record](../infra/tumbleweed-lab/evidence/issue15-provider-boundaries-20261020.toml)
contains no protected-operation claim.

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

At this revision there are **no verified protected provider writes**. The
20 October experiment confirms that the libzypp plugin is not a failure-stop
gate: the denying plugin was invoked and zypper still installed the requested
package. The native Rust service-owned operation remains awaiting proof.
Historical Run19 never reached `COMMITBEGIN`; its unchanged RPM hash remains
inconclusive and is not retroactively upgraded by the new run.

The Rust `ryoku-adapters::capabilities` module carries this initial operation
allowlist in typed form. Its protected-write predicate requires all three
operation-specific proof states: pre-mutation checkpoint, complete affected
state inventory, and post-state verification. The initial table intentionally
sets no write to `verified-protected`; it does not claim runtime provider
detection or replace the evidence below.

The same module now declares `required_recovery_scope(operation)` as a typed
minimum coverage contract. Its `AffectedState` entries make recovery scope
reviewable and testable at the operation boundary:

| Rust operation | Required state coverage |
|---|---|
| `PackageTransaction`, `CockpitPackageWrite` | Linked system snapshot; package database; installed RPM payload/configuration; package script and service effects; boot, kernel and initramfs artifacts; repository definitions/policy and metadata cache; Ryoku catalog/journal and configuration/profiles. |
| `RepositoryOrTrustMutation` | Linked system snapshot; repository definitions/policy; trust keys/imported signing state; package metadata cache; Ryoku catalog/journal and configuration/profiles. |
| `GeneralSnapperAdministration` | Linked system snapshot and Snapper configuration, plus Ryoku catalog/journal and configuration/profiles. These operations remain unsupported as general administration. |
| `MyrlynWrite`, `YastModuleWrite` | `UnboundedAdministrativeState`. The current app/module write surface is too broad to declare a complete inventory; these stay unsupported. A future candidate must first narrow to a typed action. |
| Read-only query operations | Empty mutation scope; this does not yet prove provider read APIs are non-mutating on the pinned guest. |

These scopes define what evidence a future checkpoint integration must cover;
they are not themselves capture hooks or proof that all listed state is
currently restorable. The service's journaled mutation gate supplies the
ordering contract: acquire the shared writer lock, durably record acceptance,
capture and verify linked state, revalidate the exact plan, durably record
`Applying`, call the provider, verify post-state, then durably record success.
Capture, verification, revalidation, apply, or post-state failure never yields
a successful operation; the provider callback is unreachable until checkpoint
and plan validation have both passed. The gate unit tests exercise this
ordering with fakes. They do not establish that a real zypper, PackageKit,
Cockpit, Myrlyn, or YaST transaction is intercepted at its mutation boundary.

The presentation classes are separate from the protection status: **native**
means a Ryoku-owned control and service request; **embedded panel** means an
upstream panel rendered inside a Ryoku-owned view; **external application**
means a separately launched desktop program; and **external browser panel**
means a separately launched browser administration page. The embedded class
is represented in the Rust type for explicit classification, but this
repository has no implemented embedded provider panel. No evidence currently
supports embedding Cockpit, Myrlyn, or YaST as a protected control.

| Operation/candidate | Interface class | Current status | Privilege, errors and cancellation | Rust path and hook assessment |
|---|---|---|---|---|
| Show product release, current profile, recovery availability and completed job history | Native Ryoku controls | Awaiting proof (read-only contract) | Unprivileged read from validated, versioned service data; missing/stale/corrupt data must be shown as unavailable/error, never guessed. | Typed Rust backend query. No provider write hook. Data-source and privacy review remains. |
| Search package metadata and inspect installed package/repository state | Native Ryoku controls; no external panel by default | Awaiting proof; candidate read-only | Read-only metadata queries should not obtain root privileges. Report stale metadata and provider errors. | Rust provider adapter over a narrowly defined zypper query or PackageKit read API. Pin query options and prove they do not refresh metadata or mutate state on the target snapshot. |
| Install/remove/update packages, including a full Tumbleweed update | Native Ryoku controls; direct zypper, Myrlyn and PackageKit/Cockpit are outside this control surface | Awaiting proof for a Rust-owned operation; commit-plugin protection is unsupported | Privileged, operation-specific authorization. Validate exact requested package IDs and policy before authorization. Provider dependency/conflict/signature failures are failures; do not continue partially. Pre-commit cancellation may abort; post-commit cancellation is not rollback. Preserve durable job outcome across client exit. | Fixed zypper arguments or PackageKit roles remain candidate Rust service interfaces. The 20 October guest observed both commit notifications in a normal transaction, then installed another package despite plugin ERROR. The plugin cannot provide capture failure-stop; no linked capture or journal path was exercised. Do not run a shell command from UI or authorize arbitrary zypper arguments. |
| Add/change/enable/disable repositories, alter priorities, import/remove trust keys | Native Ryoku controls | Unsupported for first release pending separate proof | Root/polkit privilege; repository and trust failures must be explicit. Do not offer arbitrary URLs or trust-on-first-use. | Rust service only after exact scope and hook is proven. Repository writes change future package trust and resolution, and must checkpoint before each mutation; capturing only before a later install does not protect them. External YaST/zypper/Cockpit editors bypass the path. |
| Snapper list/read-only status | Native Ryoku controls | Awaiting proof (read-only path) | Read through the least-privileged supported interface; distinguish empty, unavailable and denied. | Rust query adapter; verify D-Bus/CLI semantics in the VM. |
| Create/delete/modify Snapper snapshots or configuration | Native Ryoku controls | Unsupported as general administration; Ryoku recovery operations are separate | Privileged and constrained to authenticated recovery workflow. Destructive errors must preserve the last usable recovery point. | Narrow Rust recovery service operation only. Arbitrary snapshot deletion/config edits can invalidate linked recovery and are not an ordinary provider action. |
| Change selected system settings (initial candidate: a small, enumerated set of display/session preferences) | Native Ryoku controls | Unsupported until each setting has an affected-state inventory and restore proof | Per-operation authorization; validate values and report validation/service-reload failures. Do not accept arbitrary file paths or shell fragments. | Rust typed operation and declared config ownership. No generic “settings provider” hook. |
| Browse/manage software through Myrlyn | Externally launched Qt desktop application; embedding is not established | Unsupported for writes in first release; read-only use is a candidate, not yet target-verified | Upstream describes package and repository changes through its libzypp backend and a non-root read-only mode. Its privilege prompts, read-only enforcement and cancellation behavior must be checked in the pinned guest. | No Rust service API or per-operation Ryoku hook is evidenced. A launch-time checkpoint is invalid. Keep writes out of the supported Ryoku flow unless every commit is mediated before mutation; do not add Ruby glue. |
| Cockpit package panel (`cockpit-packages`) | External local-desktop helper/browser module; embedding needs a documented stable component contract | Awaiting proof for local read-only access; protected writes unsupported | The 20 October guest has PackageKit 1.4.0 with backend `zypp`; the module calls its D-Bus transaction interface. Polkit, errors, cancellation and graphical behavior still need operation-specific runtime proof. | No PackageKit or Cockpit package transaction entered Ryoku's capture path. Native D-Bus introspection and the installed Tools manifest are inventory evidence, not a recovery hook. Protected writes remain unsupported. |
| Cockpit overview / service and host panels | External local-desktop helper or browser; embed only through the documented Cockpit contract | Awaiting proof for local read-only navigation; unsupported for protected writes | Cockpit has its own privilege and service boundaries. UI errors or cancellation do not provide Ryoku recovery. Keep remote access a separate opt-in. | The installed Cockpit 365 helper and Packages Tools menu were inspected on 20 October. The helper isolates its browser/server in a network namespace by source, while the system socket ships wildcard port 9090. Graphical operation and runtime namespace isolation are unverified. No generic REST API or protected embedded panel is assumed. |
| YaST software, repository or system-configuration modules | Externally launched optional interface; individual modules may be excluded | Unsupported for protected changes in first release; read-only query candidates require module-by-module proof | YaST modules can perform privileged writes. Do not assume module-level privilege or cancellation prompts satisfy product authorization/recovery. | YaST's established module ecosystem is Ruby-based. New project-owned Ruby glue/bindings are deferred. External modules cannot be wrapped by a UI launch hook; omit write-capable modules from the supported Ryoku path until their writes can be mediated per operation. |

## Interface coverage and integration choice

This matrix names an upstream boundary that can be investigated; it does not
claim that the pinned guest has that package installed or that Ryoku has
integrated it. UI placement and write protection are separate decisions.

No embedded panel is selected or implemented in the first-release control
surface. This is an explicit coverage result, not an assumption that upstream
pages can be embedded transparently.

| Embedded candidate | Ryoku implementation | Operation coverage and decision | Evidence required before reconsideration |
|---|---|---|---|
| Cockpit Packages | None. Cockpit's documented embedding/navigation contract is only a candidate for a Ryoku-owned browser view; no embedding code or guest integration has been demonstrated. | Package reads await target proof. Package writes are unsupported because PackageKit requests do not enter the Rust checkpoint/job/journal path by evidence available here. | Demonstrate the selected Cockpit page's embedding and navigation contract in the local Plasma session, identify its exact PackageKit request path and privilege behavior, then prove every write is mediated before the provider call and survives client loss. |
| Cockpit host/service pages | None. Local browser launch remains the external-panel candidate. | Local read-only navigation awaits guest proof; write-capable host/service controls remain outside the protected Ryoku workflow. | Demonstrate page/category navigation and privilege scope in the target session; verify the actual local listener and prove per-operation capture/authorization before supporting any write. |
| Myrlyn or YaST modules | None; each remains a separately launched upstream application if selected for optional administration. | Myrlyn and YaST writes are unsupported. No embedded Rust adapter or common upstream module API is evidenced. | Establish a stable embedding/API contract and operation-level interception without adding project-owned Ruby glue, then prove the full recovery boundary for each specific operation. |

The Rust `IntegrationSurface::EmbeddedPanel` variant records this possible
presentation class. It is descriptive only and grants no operation or
protection capability. The initial operation allowlist contains no embedded
panel operation.

| Provider surface | Documented boundary to inspect | Ryoku presentation decision | Protection level supported by current evidence |
|---|---|---|---|
| Zypper/libzypp package transaction | Fixed zypper CLI operations; libzypp `ZYpp::commit` and its commit-plugin protocol | Native Rust controls would call one typed service operation. Do not expose arbitrary CLI arguments or invoke a UI shell. | Awaiting proof for a complete protected transaction. Pinned source and the 20 October guest confirm that plugin ERROR closes the plugin while the transaction continues. Use it for telemetry only. |
| PackageKit and Cockpit Packages | `org.freedesktop.PackageKit` system-bus transaction methods and signals; `cockpit-packages` uses PackageKit D-Bus | Native Rust service may call a fixed PackageKit role after capture. Cockpit Packages remains externally launched or separately evaluated for embedding. | Awaiting proof for read APIs; unsupported for protected writes until the same pre-commit and journal boundary is measured through PackageKit and Cockpit. |
| Myrlyn | Upstream Qt application backed by libzypp; upstream README describes package/repository management and a read-only mode | External application only. No embedded panel or Rust API is evidenced. | Read-only candidate awaiting target proof; all writes unsupported in Ryoku. |
| Cockpit host/service views | Documented Cockpit browser, bridge, embedding and privilege interfaces; installed `cockpit-desktop` helper | External helper is a local launch candidate. Packages registers under Tools. Actual graphical navigation and native Ryoku category integration are unverified; embedding requires a documented API and matching origin. | Awaiting proof for local read-only navigation; no protected writes. The experiment kept the system socket masked/inactive; this does not establish unmodified install defaults. |
| YaST modules | Module-specific YaST UI/service and package bindings; inspect each module and its package on the guest | External launch only for explicitly unsupported administration. No project-owned Ruby adapter or generic module wrapper. | Unsupported for protected writes; any read-only module remains awaiting module-specific proof. |
| Snapper | Snapper CLI and D-Bus/service surface | Native Rust read controls; constrained recovery operations remain in the recovery service. | Read-only query awaiting proof; general snapshot/config administration unsupported. |

### Cockpit local desktop and remote access

Cockpit is a browser-oriented administration interface. On 20 October the
guest's Cockpit 365 RPM supplied `/usr/libexec/cockpit-desktop`: a concrete
upstream helper for external local launch without enabling `cockpit.socket`.
Its inspected implementation starts the bridge in the user's session and
isolates its web server/browser in a separate network namespace. This needs
runtime graphical validation before Ryoku exposes it. The installed Packages
manifest registers a Tools entry; it does not establish native Ryoku navigation
or a stable embedding API. Cockpit's documented embedding contract requires
the same origin and explicitly documented components. Local navigation remains
an external Cockpit session and does not create a protected Ryoku operation.

Remote access is a separate opt-in. Installing or opening a local Cockpit view
must not enable a listener reachable from other machines. Verify the default
with `systemctl is-enabled cockpit.socket`, `systemctl is-active cockpit.socket`
and `ss -ltnp`; if a local-only endpoint is used, verify its actual loopback
binding. Only a distinct user-requested remote setup may enable a non-loopback
listener, and that setup remains outside the first-release recovery contract.
The inspected 365 socket unit has `ListenStream=9090` without a loopback address;
enabling it is not local-only configuration. The disposable experiment masked
the socket before installing Cockpit, then observed masked/inactive state and
no Cockpit listener. It did not test default package enablement.
Cockpit's embedding guide documents integration constraints; it is not a generic
REST API and does not mediate provider writes.

The read-only `zypp-hook-probe --inventory` diagnostic now records the
`cockpit.socket` enablement and activity states, the socket unit's configured
listen scope, and a separate host-wide TCP listener scope. Listener addresses
are reduced to `none`, `loopback-only`, `non-loopback-present` or `unavailable`;
the raw addresses are not emitted. The host-wide result is not attributed to
Cockpit. These observations describe the guest at probe time only: inactive or
disabled does not prove Cockpit cannot be started, configured socket addresses
do not prove what a later activation binds, and no result proves native desktop
category navigation, embedding, or write protection. Those UI checks still
require the graphical guest and the selected Cockpit module.

## Candidate libzypp commit gate (awaiting Tumbleweed proof)

### Sanitized Run19 results

These are the allowlisted findings from the disposable guest on fixture
snapshot `20261011`. Package versions are exact to that guest; normalized path
keys below are sanitized names, not raw command output or host paths.

| Fact | Sanitized result |
|---|---|
| Relevant RPM-owned packages | `libzypp 17.38.16-1.1.x86_64`; `zypper 1.14.101-1.1.x86_64`; `zypper-needs-restarting 1.14.101-1.1.noarch` (3 packages) |
| Commit-plugin directory | One RPM-owned directory was discovered: normalized key `usr_lib_zypp_plugins_commit`, kind `directory`, package owner `libzypp`, mode `0755`, uid/gid `0/0` |
| Sanitized path inventory | Five normalized entries are listed below. ACLs were present on all five, and no symlinks were found. ACL values are omitted; only SHA-256 digests are recorded below. |

| Entry key (normalized) | Kind | RPM owner | Mode | UID/GID | ACL SHA-256 |
|---|---|---|---:|---:|---|
| `etc_zypp_needreboot` | regular file | `libzypp` | `0644` | `0/0` | `82423033d591a8c3228661facec8c2cb4e95625a13269e802dcd08ac98e7d9cb` |
| `etc_zypp_systemCheck` | regular file | `libzypp` | `0644` | `0/0` | `82423033d591a8c3228661facec8c2cb4e95625a13269e802dcd08ac98e7d9cb` |
| `etc_zypp_zypper_conf` | regular file | `zypper` | `0644` | `0/0` | `82423033d591a8c3228661facec8c2cb4e95625a13269e802dcd08ac98e7d9cb` |
| `usr_lib_zypp_plugins_commit` | directory | `libzypp` | `0755` | `0/0` | `0557359434fae41db3ea2f9006e77672f2a3d33e4c2bd8276dd504e42662bcf3` |
| `var_log_zypper_log` | regular file | `zypper` | `0640` | `0/0` | `e957040aa926e7592861c49de6d536cb72c7c2c12ac78679d945d6e2abd39653` |

The guest could not provide SELinux-label or file-capability xattr query
results; their status is **unavailable**, not absent. The five-entry inventory
is not a complete affected-state inventory for a package transaction.

| Probe mode | Sanitized result | What it establishes |
|---|---|---|
| Deny response | Probe registered, but no `COMMITBEGIN` was observed; deny decision was not invoked. The zypper child exited 4 and the RPM-state hash was unchanged. | No live response behavior was exercised. Pinned source review shows ERROR closes the plugin but does not veto a transaction; the run itself did not reach that code. |
| Malformed response | Probe registered, but no `COMMITBEGIN` was observed; malformed reply was not sent. The zypper child exited 4 and the RPM-state hash was unchanged. | Pre-hook failure cause is unclassified; no evidence about libzypp's handling of malformed plugin replies. |
| Bounded timeout | Probe registered, but no `COMMITBEGIN` was observed; timeout mode did not begin. The zypper child exited 4 and the RPM-state hash was unchanged. | No live timeout behavior was exercised. In the matching source, a receive timeout is caught and closes the plugin; it is not propagated as a transaction veto. |
| Missing plugin | The zypper child exited 4 before a package transaction began; RPM-state hash was unchanged. | This run did not test a transaction. Pinned source review shows plugin loading is optional and no plugin is required before `TargetImpl` proceeds to commit. |

The runner records only a fixed `zypp_*_error_category` label derived from
provider stderr: `authorization`, `package_manager_lock`, `package_resolution`,
`network_or_download`, `repository_configuration`, `repository_or_metadata`,
`plugin`, `no_stderr` or `unclassified`. It never copies stderr text into public
evidence; raw command output stays in the permission-restricted private run log.
These labels help target a repeat experiment and do not establish hook placement
or whether a provider write was blocked.

No package commit, PackageKit/Cockpit operation, checkpoint ID, post-change
verification or recovery-journal entry was observed. Run19 therefore does not
prove a blocking hook or protected provider write. Keep issue #15 open and keep
package writes awaiting proof; do not describe the unchanged hash as a passed
denial test.

Run19's only behavioral conclusion is that the target snapshot contains a
libzypp commit-plugin directory and the probe harness did not reach a commit
frame in these attempts. Runtime provider claims still require an actual
`COMMITBEGIN`, provider exit/result, meaningful before/after RPM state, and
linked recovery-journal evidence for the same transaction.

### Pinned libzypp 17.38.16 source review

Run19 recorded guest `libzypp 17.38.16-1.1.x86_64`. I downloaded the exact
NEVRA binary from the fixture's pinned official Tumbleweed snapshot and inspected
its RPM headers: `SOURCERPM` is `libzypp-17.38.16-1.1.src.rpm`, and its
`SOURCEPKGID` equals the source RPM header's `SIGMD5`
(`1e7d2d5304e19675bc85b0d98d0ab046`). The binary RPM SHA-256 is
`3d3a33e9118b748d1ca8d9d0d65e443fce98d91cb35b2f2b82bce296e5adf715`; the
matching source RPM SHA-256 is
`a101827f533bb47623a60ce28d0e12612db28086b7923a59409b8052be2d69c4`; its
package identity matches the binary header's source-package identifier. Its spec
declares `Version: 17.38.16`, `Release: 1.1`, and uses the
`libzypp-17.38.16.tar.bz2` source archive, SHA-256
`6b5a20972643fbab4ebbed93551baa2b9568c6704559f86255fe58ffaa640112`.
The [binary artifact](https://download.opensuse.org/history/20261011/tumbleweed/repo/oss/x86_64/libzypp-17.38.16-1.1.x86_64.rpm)
and [source RPM](https://download.opensuse.org/source/tumbleweed/repo/oss/src/libzypp-17.38.16-1.1.src.rpm)
are the inspected files. The source files `PluginExecutor.cc`, `TargetImpl.cc`,
and `PluginScript.cc` match the upstream `17.38.16` tag at commit
[`a8aac2497414ef42a03ff5cbc174ab10ddbf8231`](https://github.com/openSUSE/libzypp/commit/a8aac2497414ef42a03ff5cbc174ab10ddbf8231)
byte-for-byte (SHA-256 values respectively
`e93debc76cbfbedf8b8be815add1e6934031cc19f822eaba0e641bb0bab77306`,
`ea6c140dda466cae05feaf6fbcf71c5ca1a2e1f62195ec8c1acd06d6465e71c7`, and
`92c7c2cd81ce01d46171bf78d3d756f198609763598240ab34d4fc9e6f44bb21`). This
binds the official snapshot artifact with the Run19 guest's recorded NEVRA to
the tagged upstream source. The original Run19 record did not retain an
installed RPM checksum, so its per-file identity cannot be independently
rechecked against the downloaded package.

The pinned [`TargetImpl.cc`](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/zypp/target/TargetImpl.cc)
loads commit plugins and sends `COMMITBEGIN`, then proceeds to store non-package
state and call the package commit without checking a plugin decision. The
pinned [`PluginExecutor.cc`](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/zypp/PluginExecutor.cc)
has `send()` return `void`; `doSend()` accepts ACK/ENOMETHOD and closes the
plugin on ERROR, an unexpected response, or a caught protocol exception. The
executor erases closed plugins and continues. The matching
[`Plugin-Commit.doc`](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/doc/autoinclude/Plugin-Commit.doc)
calls `COMMITBEGIN` a pre-install notification, but does not give the plugin a
transaction-veto result. The docs say an ERROR cancels the plugin execution; the implementation confirms
this is not cancellation of `ZYpp::commit`. This establishes source-level
placement before the package actions and fail-open behavior after plugin
rejection; Run19 did not reach the hook at runtime.

Conclusion: a libzypp commit plugin cannot provide the required
pre-mutation failure-stop gate for this release. Treat such a plugin as
telemetry only; do not use ERROR, malformed output, plugin disappearance,
missing plugin, timeout, or a negative checkpoint callback to authorize a
zypper write. This does not establish that the whole package transaction operation is protected or
unsupported through every possible Ryoku-owned execution path. That operation
remains awaiting proof until a complete mediated transaction is demonstrated;
Run19 still supplies no guest `COMMITBEGIN`, provider result, or journal entry.

A libzypp commit plugin may provide transaction telemetry, but it is not an
authorization or recovery boundary. A broker called only from this plugin cannot
make zypper fail closed: the pinned implementation closes the plugin after a
negative or missing reply and continues the transaction. To protect the native
Ryoku operation, any viable design must place the checkpoint and authorization
in the Rust-owned service before it launches the fixed provider operation, then
keep a durable job through provider completion and verify/journal the result.
This does not prevent out-of-band system administration; those interfaces must
remain outside the protected Ryoku workflow unless separately mediated.

Run19 did not reach `COMMITBEGIN`, so it still provides no runtime transaction
result or recovery-journal evidence. The pinned source review settles the
commit-plugin veto question for the matching upstream release, but it does not
measure PackageKit, its zypp backend, Cockpit, Myrlyn, or YaST. The 20 October
run subsequently recorded installed Cockpit/PackageKit versions, native
interfaces and repository/hostname bypasses; it did not run transactions
through those optional interfaces or test their cancellation behavior.
A future protected-operation experiment should validate the fixture binary's source
provenance, then test the Rust service-owned transaction path: inject
checkpoint failure and prove the provider is never invoked; complete one
harmless transaction; and match its capture ID, provider outcome, resulting RPM
state and journal entry. Separately inspect PackageKit and Cockpit to determine
whether either is called by that path or bypasses it. Treat direct RPM,
repository-definition and trust-key writes as outside the protected workflow
until separate pre-mutation controls are demonstrated.

References for source analysis are pinned to libzypp tag `17.38.16` above.
The [20 October report](provider-experiments-20261020.md#interface-references-inspected)
adds release-tagged PackageKit 1.4.0, Cockpit 365, Cockpit Packages 5 and Myrlyn
1.0.0 interface references. Tagged upstream source is distinguished from
installed downstream RPM byte identity and actual transaction evidence.

“Embedded” is an integration choice, not a protection property. A panel that can submit writes remains a provider bypass unless those exact writes are mediated. Where the upstream integration cannot meet this boundary, use an external launch only for unsupported/admin use and clearly mark it outside the protected controls.

## Provider boundary decision

### Minimal first-release operation set

The initial supported set is intentionally small: display validated release,
profile, recovery and completed-job data; search packages and inspect installed
package/repository state through proven read-only calls; and expose an
allowlisted package transaction only if the provider's pre-mutation checkpoint,
failure-stop, post-state verification and journal behavior pass on the pinned
guest. Until that evidence exists, package writes remain unavailable. Repository
and trust edits, general Snapper administration, generic system settings, and
write-capable Myrlyn, Cockpit or YaST surfaces are excluded. Local Cockpit
read-only navigation is optional external administration, not a core Ryoku
control.

The first implementation candidate is a small native Rust control surface backed by one Rust service. It should use a typed, allowlisted operation API, a durable job identifier and typed states such as Queued, Authorizing, Capturing, Running, Verifying, Succeeded, Failed and OutcomeUnknown. These are design names, not implemented types. Cancellation should be typed separately as NotStarted, Requested, Accepted, TooLate or Unsupported; never map a canceled client connection to a canceled package transaction.

A reviewed adapter may invoke a fixed zypper command or a supported D-Bus provider method after the checkpoint. The exact interface remains **awaiting proof**. The adapter must not interpolate arbitrary user text into a command, accept arbitrary arguments, expose a shell, or silently retry a write. PackageKit, where installed, is a possible standardized package-management boundary, not proof that every repository/config operation participates in the Ryoku checkpoint. libzypp is implemented outside this Rust workspace; adding a direct FFI bridge is not approved by this feasibility document.

Myrlyn, Cockpit and YaST are not alternate owners of the recovery contract. If future proof shows their write path cannot be intercepted before each mutation, they remain external optional administration interfaces and their mutations are outside the protected first-release operation set.

## Minimal live hook experiments required

Run these against the supported disposable Tumbleweed guest and record exact package/provider versions, command/API, relevant service state, redacted logs and before/after state. Do not use the production host. The existing lab run is infrastructure evidence, not a provider-hook experiment.

1. **Rust-owned transaction boundary:** invoke the candidate native Rust service operation for one harmless package already available in the frozen repository snapshot (or a no-op plan followed by a disposable transaction). Record authorization, linked pre-change capture ID, provider transaction/job ID, commit start/completion, final RPM state and post-state verification. Inject checkpoint failure and show that the provider is never called. The libzypp commit plugin is not a candidate failure-stop hook; source review proves its error/timeout path continues into the commit.
2. **Bypass classification:** verify which interfaces can reach PackageKit/libzypp operations independently of the Ryoku service. Direct zypper, PackageKit, Myrlyn, Cockpit software and YaST software remain outside the protected Ryoku workflow unless each write is mediated before provider invocation. Record versions and exact request paths; do not spend a live transaction testing whether the libzypp plugin vetoes, since the pinned source shows no veto result is consumed.
3. **Cancellation and client loss:** close the client before provider start, request cancellation before commit, then disconnect after commit begins. Verify no write before checkpoint, accurate cancellation reporting, persistent job status and recovery after service restart. Never claim that killing the client or provider process reverses a committed transaction.
4. **Repository/trust operation:** use a disposable local repository fixture. Prove the pre-change checkpoint precedes each repo definition, priority, enablement and trust-key mutation; verify failure blocks the write and recovery restores the affected definition plus trust state.
5. **Selected config operation:** choose one explicitly bounded setting. Capture its complete affected-state inventory; inject checkpoint failure; perform the write; verify post-state, service reload/error behavior, restore, and preservation of unrelated home files.
6. **UI integration:** for each optional panel, verify navigation and authorization in a local graphical session; inspect the actual write request path. Confirm Cockpit is not remotely listening unless the user separately opts in. Test whether the interface can be constrained to read-only; do not infer this from its visual presentation.

A candidate moves to **verified protected** only after the matching experiments pass and evidence is linked. A demonstrated bypass or inability to stop a real attempted write moves that operation to **unsupported**. A failed attempt that never reaches the hook, like Run19, leaves it **awaiting proof**.

## Interface references

These references describe upstream interfaces; they are not evidence that this port has integrated or validated them.

- [zypper manual (openSUSE documentation)](https://en.opensuse.org/SDB:Zypper_manual) — CLI operations and options. Consult the pinned guest's `zypper --help` and man page before selecting flags; the moving manual does not prove the guest's exact behavior.
- [libzypp project](https://github.com/openSUSE/libzypp) — native package-management library and source. Its existence does not prove stable Rust bindings.
- [libzypp 17.38.16 commit-plugin protocol](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/doc/autoinclude/Plugin-Commit.doc), [plugin protocol overview](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/doc/autoinclude/Plugins.doc), [plugin executor](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/zypp/PluginExecutor.cc), [plugin script timeout](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/zypp/PluginScript.cc) and [commit call site](https://github.com/openSUSE/libzypp/blob/a8aac2497414ef42a03ff5cbc174ab10ddbf8231/zypp/zypp/target/TargetImpl.cc) — pinned source review of the snapshot's matching libzypp NEVRA; runtime transaction behavior remains unexercised.
- [PackageKit API](https://www.freedesktop.org/software/PackageKit/gtk-doc/), [PackageKit D-Bus API](https://www.freedesktop.org/software/PackageKit/docs/introduction.html), [transaction interface XML](https://github.com/PackageKit/PackageKit/blob/main/src/org.freedesktop.packagekit1.Transaction.xml) and [polkit actions](https://github.com/PackageKit/PackageKit/blob/main/data/policy/org.freedesktop.packagekit.policy.in) — standardized transaction methods, signals and authorization actions. Authorization and completion signals are not a pre-commit recovery hook. Confirm backend/version and behavior in the guest.
- [Myrlyn source and README](https://github.com/shundhammer/myrlyn) and [openSUSE news on its origin](https://news.opensuse.org/2024/12/20/new-pkg-mgmt-tool-debuts/) — upstream describes a Qt UI backed by libzypp, package/repository management and read-only use. The installed Tumbleweed build, permission behavior and exact UI remain to be inspected.
- [Cockpit embedding guide](https://cockpit-project.org/guide/latest/embedding.html) — documented embedding/navigation constraints.
- [Cockpit packages guide](https://cockpit-project.org/guide/latest/packages.html) — Cockpit package/frontend integration model.
- [Cockpit privilege guide](https://cockpit-project.org/guide/latest/privileges.html) — privilege handling at Cockpit's interfaces; it does not mediate the Ryoku checkpoint.
- [Cockpit startup guide](https://cockpit-project.org/guide/latest/startup.html) and [`cockpit-ws` manual](https://cockpit-project.org/guide/latest/cockpit-ws.8.html) — browser/server startup and socket configuration references. They do not establish the pinned guest's service state or listener addresses.
- [Cockpit project](https://github.com/cockpit-project/cockpit) — frontend/backend project and source. Do not invent a generic REST endpoint.
- [openSUSE cockpit-packages README](https://github.com/openSUSE/cockpit-packages/blob/main/README.md) and [source](https://github.com/openSUSE/cockpit-packages) — identifies the module as a Cockpit package UI using PackageKit D-Bus; its Tumbleweed package version and operation hooks remain unverified here.
- [YaST framework](https://github.com/yast/yast-yast2) and [YaST packager](https://github.com/yast/yast-packager) — project/module sources, not a single common transaction API. Inspect each shipped module's code, dependencies and mutations separately.
- [Snapper project](https://github.com/openSUSE/snapper) — CLI and service implementation reference. The guest's supported read-only D-Bus/CLI calls still require direct verification.

All links and interface/version claims need revalidation against the target's pinned/current Tumbleweed snapshot before they are treated as implementation evidence.


The probe harness also exposes `missing-plugin` (runs without registering any probe executable) and `timeout-commit` (the probe deliberately withholds its COMMITBEGIN response). Every zypper transaction is launched through GNU coreutils `timeout` with a 45 second deadline and 5 second TERM-to-KILL grace period; unsupported timeout implementations are rejected before plugin registration. The timeout mode records the plugin PID and verifies it is gone after the bounded run. Each mode reports RPM state before and after. Denial and malformed-response outcomes require the probe to record `COMMITBEGIN`, zypper to fail before timeout, and RPM state to remain unchanged. Timeout mode requires a recorded `COMMITBEGIN`, the bounded timeout status, a terminated plugin PID, and unchanged RPM state. The missing-plugin case records whether the operation proceeded; if zypper succeeds without installing the requested package or fails before a real transaction, the result is marked inconclusive. Any changed RPM state is a failed safety result and requires discarding the disposable overlay. These remain characterization runs, not proof that the provider path is protected.

The host-side timeout regression test launches a Rust test process and Rust grandchild under GNU coreutils `timeout`, then checks that both PIDs disappear after the deadline. It validates process-tree signal behavior on the Linux CI host only. It does not validate the Tumbleweed guest's coreutils build, libzypp plugin discovery, or whether zypper prevents RPM changes; those still require the disposable guest experiments above.

### Existing Run19 result and diagnostic gap

The private Run19 logs identify Tumbleweed `zypper 1.14.101-1.1` and
`libzypp 17.38.16-1.1`, with no installed PackageKit, PackageKit zypp backend,
Cockpit Packages, Myrlyn or YaST2 package. RPM state was unchanged in the deny,
malformed, timeout and missing-plugin modes, and the probe plugin was not
invoked in any mode. Each zypper invocation exited with status 4 before the
transaction reached the commit hook. The 45-second timeout mode also did not
exercise its timeout path. These are failed preconditions, not evidence that
the hook blocks or permits a provider commit.

Run19's probe discarded captured zypper stdout and stderr, so the private logs
do not reveal why resolution stopped before the plugin. The Rust probe now
records at most 16 KiB from each stream as escaped text in the existing
mode-0600 private run log. The public evidence writer continues to select only
allowlisted outcome fields; raw provider text is not public evidence.
The [20 October run](provider-experiments-20261020.md) used a working frozen
repository/package fixture, reached the actual commit notifications and
confirmed a write despite plugin denial. This closes the diagnostic gap but
does not supply a protected-operation path.
