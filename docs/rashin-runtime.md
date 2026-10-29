# Rashin Rust runtime: development operation and acceptance

This is the implemented runtime slice for [#183](https://github.com/GenieRobot/ryoku-opensuse/issues/183).
It implements real private assistant requests, rather than treating the
[retained source inventory](rashin-capabilities.md) as runtime acceptance.
**Issue #183 remains incomplete.** This binary is not yet installed by an RPM,
connected to the upstream Svelte console, or certified for linked recovery.
The standalone console below is a development client, not a replacement or a
parity claim for the retained Ryoku 1.0 interface.

## Running offline

Build with `cargo build --locked --offline -p ryoku-rashin`. Create an absolute
directory owned by your account, mode `0700`, with no symlink ancestors. Run:

```text
ryoku-rashin web /absolute/private/rashin-directory
```

The daemon prints its loopback URL. Open it, read `browser.token` in the private
directory, then open the workspace link. In the browser authentication dialog,
use username `owner` and the token as the password. The token rotates on each web
daemon start. Close the browser session to clear cached authentication. Index a
document, then ask a question containing terms from that document. The workspace
shows recent answers, separate Agent/Chat conversations, explicit model picks,
provider editing and asynchronous model refresh. Job pages refresh while running
and show ephemeral streamed progress, cancellation and terminal results. The default
`local-retrieval-v1` provider returns matching indexed evidence with source names
and hashes. It performs no network I/O and is not a general language model.

`serve` starts only the Unix socket. Both modes create `rashin.sock` in the
private directory. A second daemon is refused. A stopped daemon's socket may be
replaced; an unrelated file at that path is never unlinked. There is no service
enablement, login autostart, lingering, default-on healing, package installation,
or implicit network download.

`ryoku-rashin logs COMPONENT` performs a one-shot bounded native inspection.
The `logs` API action returns the same result over the authenticated boundary.
It reads the selected user unit state/journal, RPM capability and package owner
of its fixed `/usr/bin` or `/usr/lib/systemd/user` path. No caller-selected path,
option, command, system-unit action or privileged journal is accepted. Results
label unavailable, timed-out and truncated observations instead of fabricating
context when the user bus, journal or RPM lookup is unavailable.

The CLI reads a typed JSON action from stdin, keeping credentials out of process
arguments:

```text
ryoku-rashin request /absolute/private/rashin-directory/rashin.sock
```

For example, provide `{"action":"ask","id":"question-1","prompt":"How does Tumbleweed manage packages?"}`
on stdin, then `{"action":"job","id":"question-1"}` to poll. Each invocation
has one request. Reusing an ID with the same lane/session/prompt returns the
existing job; using a different request with that ID fails. Forget a completed
job to release its bounded history slot before reusing that ID. IDs use letters,
digits, hyphen and underscore, up to 64 bytes.

## Implemented actions and authority

All HTTP actions use `POST /api/action` with `Content-Type: application/json`.
The same `Request` enum is used by the CLI, HTTP handler, console and tests.
Unknown actions and fields are refused. The Unix socket authenticates the
kernel peer UID before parsing. HTTP requires a private bearer token, or browser
HTTP Basic authentication (`owner` and the private token) plus the exact configured
Origin for POST. Cookies are never issued or accepted as authority.
Origin, Host and `X-Rashin-Client` are not identity credentials. Cross-origin
requests, duplicate HTTP headers, chunked bodies, oversized frames and arbitrary
commands are refused. Browser responses are private, uncached, escaped, and have
a restrictive CSP; the console requires no JavaScript.

| Action family | JSON actions | Effects and authorization |
| --- | --- | --- |
| Inspect | `status`, `providers`, `get_index`, `wiki_pages`, `read_wiki`, `recent`, `sessions`, `session`, `job`, `jobs`, `usage`, `logs` | Account-private reads; provider inventory never returns keys; logs run only fixed bounded native readers |
| Ask | `ask`, `cancel` | Read-only provider lane, private independent history, bounded asynchronous execution and cancellation |
| Conversation | `new_session`, `prompt`, `forget_job`, `delete_session` | Persistent independent Agent/Chat transcripts; deletion removes associated jobs and usage records |
| Credentials | `put_provider`, `update_provider`, `refresh_models`, `remove_provider`, `connect_prowl_provider`, `issue_ask_client`, `ask_clients`, `revoke_ask_client` | Owner configures/removes an explicit provider; keys remain only in private state and provider stdin |
| Route/model | `set_route` | Owner selects a provider and one of its configured model IDs for an explicit lane |
| Index | `index`, `index_wiki` | Owner replaces a bounded set of explicitly uploaded UTF-8 documents; every SHA-256 is verified before publication |
| Fix | `fix_preview`, `fix` | Owner supplies a typed doctor/tip/app finding; external native-evidence egress requires a reviewed destination-bound preview; creates an Agent investigation request with Tumbleweed ownership/recovery instructions |
| Ask handoff | `handoff`, `manual_handoff`, `clear_handoff` | Owner stages recorded/partial Ask or explicit manual context, or runs it in Agent while cancelling Ask; plain Chat is separate |
| Local agent instructions | `wire`, `unwire` | Owner installs/removes instructions for the built-in Agent lane; Ask/Chat are unaffected |
| Local usage sources | `enroll_usage_source`, `collect_usage`, `disable_usage_source` | Owner explicitly enrolls a private tool data root, collects local activity or removes enrollment/cache; restricted Ask clients cannot use these actions |

`authorization::Access::AskClient` is enforced by the live HTTP API when the
owner explicitly delegates an Ask credential. Its allowed actions are `ask`,
`cancel` (Ask jobs only), `status` (Ask route/activity only), `get_index`, `recent`
and `job` (Ask jobs only). It shares the account's Ask history/index context;
it cannot configure credentials/routes/index/wiring, start Agent/Chat/Fix or
handoff, inspect Agent jobs/transcripts, collect native logs or use local usage
sources. JSON never selects the caller's role.

Use the owner-only `{"action":"issue_ask_client","lifetime_seconds":900}` to
obtain one private bearer token plus its revocation ID. The default lifetime is
15 minutes, bounded from 60 seconds to one hour, with at most eight live grants.
Pass the token only in `Authorization: Bearer TOKEN` to `/api/action`. An explicit
scoped or invalid bearer cannot inherit stronger authority from browser
authentication. Host/loopback/Origin controls continue to apply. `ask_clients` lists IDs
and expiry metadata without tokens/hashes; `revoke_ask_client` with `id` revokes
subsequent requests. Accepted jobs remain controlled by the owner; use `cancel`
to stop their model work. Only token hashes live in bounded memory, expiry uses
monotonic elapsed time, and restart discards all grants. Tokens never enter
prompts, URLs, persisted state or diagnostics.

The Unix transport and full browser console authenticate the account owner.
Capabilities limit HTTP authority. A client with owner filesystem/socket access
retains that account's authority. Same UID is not a separate client identity.
The shell/ACP integration still requires isolated credential delivery/process
access and typed native per-action authorization with linked recovery. This live
HTTP capability path does not establish those remaining native bridge gates.

`fix` performs investigation only: findings never become a shell command or a
package/service mutation. `wire` currently affects the built-in Agent's real
system context. It does **not** install marker blocks or skill links into Hermes,
Codex, Claude Code, Oh My Pi, opencode or Gemini configuration. Those external
effects require their reviewed typed providers and linked capture.

Native collectors use fixed `/usr/bin/systemctl`, `/usr/bin/journalctl` and
`/usr/bin/rpm` commands, a cleared environment with a kernel-UID-derived runtime
directory, a 500 ms deadline and 32 KiB output bound per probe. Stalled/oversized
children are killed and reaped. Fix includes UTF-8-safe, explicitly labeled
512-byte excerpts from each observation; the explicit logs action supplies the
full bounded evidence. Findings retain request idempotency even if recollected
native state changes. For an external Agent route, first call
`{"action":"fix_preview","kind":"app","component":"example.service","evidence":"The reviewed finding"}`. Review its exact
`kind`, `evidence`, `native_evidence` and destination (provider ID, endpoint and model); the disclosure
also identifies the supplied finding, indexed context and configured instructions
sent with the investigation. To approve, send the returned
`native_evidence_consent` token in the `fix` request alongside `id`, `kind`,
`component` and `evidence`. Missing/expired/unknown confirmation returns
`evidence_consent_required` before any provider dispatch. The token is owner-only,
in-memory, valid for five minutes, bound to that kind/finding/component and provider/route
configuration revision, and is not persisted or inserted into the prompt. Fix
sends the reviewed native summary without recollecting it. Changing any provider
credentials/configuration or route invalidates previews, and the revision is
checked again atomically when the provider is captured. The same transaction
consumes the token once: another request ID cannot reuse it. A failed storage
publication consumes it without dispatching, requiring a fresh preview. Restart
discards previews.
Successful request retries remain idempotent without sending evidence again.
Local/fixture Fix requires no external-evidence confirmation.
The runtime package requires `systemd` and `rpm >= 4.19`.

## Explicit Ask handoff

`{"action":"handoff","ask_id":"selected-ask"}` stages the recorded question
and completed answer, or a generation-bound snapshot of the current partial Ask
answer. It accepts no replacement text. Partial snapshots are labeled
`in_flight_ask`; completed records are `recorded_ask`. Staging leaves Ask work
running. Pending context is visible through owner `sessions`, survives restart
and Agent session changes, and is consumed once by the next accepted Agent
`prompt`. Ask and Chat do not consume it. Serialized untrusted conversation data
keeps delimiter-looking answer text as data. Later prompts do not prepend it again.

`manual_handoff` accepts a required `question`, optional `answer` (default empty)
and optional `run`. It trims supplied context and labels it `manual`, with no
recorded Ask ID. It cannot claim recorded provenance through a caller-selected
source flag. Manual context supports question-only continuation and remains
separate from canonical Ask history. The browser offers these manual controls
and shows provenance beside pending context.

For explicit immediate Run, supply `"run":{"id":"handoff-job","session":"agent-session"}`
in either action. Accepted Run atomically captures the source/partial snapshot,
chosen Agent session, provider and prompt, publishes the new job, consumes pending
context and marks active Ask jobs cancelled. Cancellation flags/progress change
only after successful state publication. Ask cannot later publish a completion or
usage for that cancelled generation. Chat work stays separate. This preserves
the upstream single Ask-lane cancellation semantics without adding an unbounded
provider worker: the existing four-worker admission limit still applies before
acceptance. Busy/invalid/unavailable/quota/storage rejection preserves source work
and pending context; a failed state write performs no provider launch.

An accepted Run ID returns its recorded status on replay without restaging,
resending or cancelling later Ask work, including after restart. Different
source/session/manual-context reuse conflicts. Cancelled, failed or non-Ask jobs
cannot masquerade as canonical sources. Explicit manual context remains available.
Accepted job failure/cancellation retains its source metadata without incomplete
conversation turns. Prompt/context bounds remain enforced. `clear_handoff` removes
pending context; forgetting a referenced Ask removes its staged snapshot. Manual
context has no false source-job identity. Legacy recorded handoff state decodes
with default recorded provenance; older canonical Run fingerprints are retained.

These owner-only actions remain unavailable to scoped Ask clients. The pinned
Ryoku1.0 `backend/ws.go` blob `d237e2d732630f86619ab44f4a9beca8589264ab`
(lines563–624) selects canonical history when available, accepts manual/empty-answer
context, stages without cancellation and cancels Ask on Run. The pinned
`backend/handoff_test.go` blob `673d416ddf4895f2d2cb2d61b40258435f4fbf60`
(lines65–120) explicitly tests unfinished/manual Run cancellation and prompt data;
both are at commit `a59d051f0e8080052216f6c7b9eb7e5928935ce8` under
`ryoku/rashin/`. The Rust API/browser and deterministic local fixture path implement
those semantics. Actual shell/launcher/ACP/WebSocket v3 shared state, cancellation
and replay integration remain required full-parity gates.

## Provider and model configuration

The local provider is always available as provider `local` and cannot be
overwritten or removed. `put_provider` also supports:

- `open_ai_compatible`: an explicit HTTPS endpoint (normally ending in `/v1`),
  a private `api_key`, and a bounded `models` catalog. Completions use the distro
  `/usr/bin/curl` with TLS validation, redirects and ambient proxies disabled,
  `.curlrc` disabled, a cleared child environment (only `LANG=C`), time/response
  limits and cancellable output polling. The
  key and request body travel over stdin rather than command arguments. Curl
  diagnostics and response errors are not echoed. External interaction needs
  the user's endpoint, credentials and an installed curl; the desktop runtime
  spec explicitly requires the Tumbleweed `curl` package. No such request runs
  in routine tests.
- `fixture`: explicit `delay_millis` (0–1000) and outcome
  `{"type":"answer","text":"synthetic response"}` or `{"type":"failure"}`.
  It is labeled `test_only`, uses `fixture-v1`, performs no network I/O and
  marks usage `fixture: true`. It tests real pending/cancel/failure paths.

Example provider configuration must be supplied as private stdin JSON, not a
shell command containing a real key:

```json
{"action":"put_provider","id":"example","provider":{"kind":"open_ai_compatible","endpoint":"https://provider.example/v1","api_key":"USER_SUPPLIED_PRIVATE_KEY","models":["model-1"]}}
```

`update_provider` accepts `id` and a typed `changes` object with optional
`endpoint`, `api_key`, `models` and `streaming`. Omitted fields retain their
existing private values. The owner can update models/streaming/endpoint without
revealing or re-entering the stored key. A supplied key rotates it privately;
responses and inventory remain redacted. Empty or invalid patches publish no
state change. The protected local provider cannot be edited, and scoped Ask
credentials cannot perform edits. Any successful edit invalidates native Fix
previews through the same configuration revision as full replacement.

```json
{"action":"update_provider","id":"example","changes":{"models":["provider/模型@profile#fast"],"streaming":true}}
```

Model IDs are bounded opaque UTF-8 strings: Unicode and punctuation stay exact,
control characters are refused, and requests must use an ID in the configured
catalog. IDs are JSON values; they never become command arguments, executable
syntax, URL paths or filesystem paths. This retains the pinned ACP model
contract (`SetModel` passes `modelID`/config-option values directly) and avoids
turning catalog identifiers into an edition-specific alphabet.

`refresh_models` takes an owner-chosen request `id` and configured `provider` ID
and starts an asynchronous `catalog_refresh` job. Query/cancel/forget it through
the existing typed job actions. The configured OpenAI-compatible provider uses
fixed authenticated HTTPS `GET /models`, following the
[model-list response contract](https://developers.openai.com/api/reference/resources/models/methods/list).
Only bounded `data[].id` metadata is accepted: at most 128 rows, opaque ID
validation, exact deduplication and the existing 1 MiB/45-second curl limits.
No model files are downloaded. The operation sends no prompt/index/history data.
Keys stay in the private stdin configuration and never enter job responses.
Local/explicit catalog fixtures exercise the same publication path offline.

A provider may be configured before models are offered; an empty catalog is a
valid connected-but-unavailable selection state. Refresh success atomically
updates offered IDs plus job status/count, preserving credentials, other settings
and every saved lane pick. It publishes no conversation turn or token-usage
record. Per-provider configuration generations reject a response after newer
settings/key/endpoint/catalog changes. Independent providers refresh concurrently;
newer route choices are retained. Cancellation/failure preserves the existing
catalog. Restart marks unfinished work interrupted and never silently reissues a
network request. No automatic refresh or provider connection is enabled here.

Select it with `set_route` fields `lane`, `provider`, `model`. Ask, Agent and Chat
have explicit independent routes in this slice. Removing or updating a provider
does not silently overwrite an unavailable saved model/route. A new request then
reports unavailable until the owner explicitly chooses an available route, except
for Ask's separately selected ACP fallback described below.
Choosing an external route sends that lane's prompt, indexed evidence and its
conversation context to the explicitly configured endpoint. Fix's automatic
native excerpts additionally require the preview confirmation described above.
Provider keys are
not inserted into assistant context.

Configured OpenAI-compatible providers may explicitly set `"streaming":true`;
the default remains false for existing provider configurations. This requests
OpenAI SSE and usage events from the same configured HTTPS endpoint. The fixed
curl client now disables stdout buffering. Keys and request JSON remain stdin-only,
and the existing no-shell/environment/TLS/redirect/proxy/time/size limits apply.
The decoder handles fragmented UTF-8, LF/CRLF, comments and multiline data, and
requires a terminal `[DONE]`. Malformed/error/tool-call frames, extra data after
the terminal marker, 64 KiB frame limits, 128 KiB text limits and the total 1 MiB
response bound fail the request. This completion provider still executes no tools;
the retained typed ACP/tool approval path remains required. Stop reason and
provider-reported actual model are recorded separately from the saved model pick,
which is never silently changed. Missing usage remains explicitly estimated.

`job` returns a top-level `progress` string while a streaming job is Running.
It is private bounded memory for at most four workers, with at most 128 KiB per
worker, and is never fsynced on each delta or inserted into durable session turns.
Cancellation, failure, completion and restart discard it. Successful completion
alone publishes answer/session/usage through the existing atomic state boundary.
The explicitly labeled fixture provider also supports a bounded streamed outcome
for deterministic workflow tests; it contacts no endpoint. Live provider/network
acceptance and upstream v3 WebSocket/ACP/UI replay remain required.

Active cancellation/progress registration occurs under the state publication
lock before Running becomes visible. A per-process monotonic worker generation
binds callbacks, completion and cleanup to that job instance. Cancelling/forgetting
and reusing an ID cannot let an old worker publish into or clear its replacement.
Generation counters are checked for exhaustion; restart never resumes old workers.

Usage records preserve provider/model, input/output tokens, provider-reported actual model when available, timestamp and
whether token counts are estimates or fixture results. Provider-reported counts
come from the completion response. The runtime's activity is explicitly **not
provider quota**; quota is reported unavailable. Upstream Claude OAuth, Codex
live rate-limit RPC remain required work. The
[enrolled local activity collectors](rashin-usage.md), persistent schedule,
private fixture tests and dependency handoff are implemented separately.
The [opencode SQLite fallback](rashin-sqlite.md) includes committed WAL snapshots
and a documented system-library boundary requiring separate review before merge.

## Privacy, persistence and failure

`state.json` contains credentials, prompts, transcripts, jobs, index, instructions,
routes and usage; it is owner-only `0600`, bounded to 8 MiB. Socket and browser
token are `0600`; the state directory is `0700`. Descriptor-relative openat2
rejects symlinks, unsafe owners/modes, nonregular data and hardlinks. An exclusive
process lease serializes daemons. State publication writes a new private inode,
fsyncs it, renames, then fsyncs the directory. An abandoned stage is discarded
under the lease and never promoted after a crash.

Provider work runs outside the state mutex. A cancellation prevents answer,
transcript and usage publication even if a provider finishes concurrently.
Failed/cancelled requests do not insert incomplete conversation turns. Pending
jobs become `interrupted` after daemon restart and are never silently reissued
to a network provider. A state publication I/O failure marks the runtime
unhealthy; it stops reporting subsequent state as healthy until restart. Logical
size limits refuse the request without publishing it. Each Running job reserves
128 bytes under the private state cap so concurrent owner configuration cannot
consume its terminal-error allowance. If a completion's answer/history/usage
would exceed the cap, that clone is discarded and the worker records
`failed`/`limit`, preserving prior turns and usage. The owner can free history
and retry with a new request ID; the daemon stays usable. A failure to publish
even that terminal record makes the runtime unavailable until restart. Jobs, sessions, documents,
model catalogs and in-flight provider workers have explicit bounds. Forget/delete
actions make the retained-history bounds usable for ongoing operation.

Private state is not exported as diagnostics or committed evidence. The offline
tests contain synthetic text/keys only. **Atomic private state publication and
these failure semantics are not linked Snapper/configuration recovery.**

## ACP process and authorization boundary

The retained `backend/acp.go` speaks newline-delimited JSON-RPC over adapter
stdio: initialize, capability negotiation, session new/load/prompt/cancel and
model/config-option selection. `ws.go` supplies protocol-v3 replay and
`acptools.go` carries tool/diff/permission state. This runtime does not yet spawn
those harnesses. A same-UID generic child can access its account's files, so an
ACP process by itself is not a restricted identity or a protected mutation
provider. The live bridge must receive an authenticated restricted capability
without owner credentials, and tools that change covered state must use typed
per-action authorization plus the #25 state/writer/linked-recovery seam.
Adapter lifecycle, transport and native execution need a concrete isolation
contract before exposing that production path. All retained harnesses, UI and
terminal behavior remain required; this records an integration boundary without
removing those features.

## Full Ryoku 1.0 acceptance map

The 338-path source ledger, 34 upstream HTTP/WebSocket registrations, 89 Prowl
client calls, bridge references and capability matrix remain retained targets.
No row in the original inventory is removed by this slice.

| Retained upstream capability | Implemented and host-tested in this runtime | Still required for full #183 acceptance |
| --- | --- | --- |
| Ask/recent/cancel/handoff | Asynchronous local/fixture/HTTPS/Prowl Ask, bounded live SSE progress, actual model/usage, cancellation/restart, canonical recorded/manual/unfinished handoff, typed NEEDS_AGENT reasons, owner-selected isolated ACP fallback and atomic Agent Run cancelling Ask | Original launcher/shell/Hub entrypoints and shared history, complete WebSocket-v3 replay |
| Agent/plain Chat | Independent persistent sessions/turns, load/read/delete, saved routes and explicit model picks; both JSON/SSE gateway completions | Persistent/shared ACP session and replay integration, all retained harnesses and config modes, approvals/diffs/images/thinking/terminal |
| Fix doctor/tip/app | Typed investigation, bounded native user-unit/journal/RPM evidence, external destination/finding preview, expiring single-use exact-action consent, replay safety; tools disabled to preserve reviewed evidence | Complete upstream app/coredump/package-change context and native entrypoints, transient unit/terminal harness, actual authorized repair providers with linked recovery |
| Agent wire/unwire | Effective owner-controlled built-in Agent instructions, separate Ask/Chat, private persistence and scoped denial | Exact external marker/skill/MCP wiring, unrelated config preservation, native harness readiness/default/model operations and protected writer enrollment |
| Index/vault/memory/wiki/skills | Verified uploaded document retrieval, all8 exact retained Wiki sources plus Tumbleweed views, browser/API navigation and confirmed per-document merge preserving uploads | Native machine/package/desktop/user/habits maps, registered projects, vault/memory/journal/skills enrollment and source mirror/index build/report lifecycle |
| Fast-lane read-only tools | All5 execute through real model/tool-result rounds: system_query, read_file, list_dir, search_code, fetch_url; four rounds then final no-tools, explicit owner grants, path/Store/DNS guards, cancellation, bounded actual native/network fixtures and live/saved tool status in job API/browser | Complete retained activity details/diff/approval UI, escalation semantics, native default root discovery and full ACP tool/permission/process integration |
| Provider/model/network | Private key CRUD/retention, opaque IDs, explicit saved/unavailable lane picks, async generation-bound catalog refresh, HTTPS and real authenticated Prowl Ask/Agent/Chat; four typed management reads (profiles/active/profile-models/presets) | Remaining retained Prowl provider/profile CRUD and selection, sign-in/OAuth/device flows, fallback/cache/network/project/harness/skill operations, all adapter model/config options and full native settings UI; actual packaged endpoint/provider acceptance |
| Usage | Completion activity plus enrolled Claude/Codex/opencode JSON/SQLite activity, committed WAL snapshots, costs/cache/reset/soft caps, opencode model/session-component rows, local/UTC windows, one five-minute daemon schedule and labeled source failures | Real OAuth/RPC/session-limit quota, remaining provider/model/session detail, original separate timer/cache/shell UI and service integration |
| ACP/process/harness | Configured captured ELF adapter→real isolated stdio lifecycle, initialize/new/load/model/prompt/text notifications/cancel and bounded replay contract tested; actual API/browser jobs, private bounded thinking, descriptor-granted file read broker and permission denial/approved jailed kernel probe | Full native/shared sessions/load/replay and model/config options, all retained harness/interpreter/runtime and sign-in support, tools/approvals/images/full terminal/UI integration; no full ACP parity claim |
| Web/native app/API | Owner Unix/HTTP API, scoped Ask bridge, cookie-free Basic/exact-Origin browser, real Ask/Agent/Chat/provider/model/job/handoff/Fix/usage/Wiki/root/tool permission forms | Retained Svelte/native app routes/sheets, theme/vitals/system/vault content, launcher/shell/Hub bridges, isolated credential delivery and real WebKit/native browser behavior |
| Terminal | Typed request/logs CLI and bounded fixed native read probes | Upstream proposed-command tiers, command-buffer confirmation, continuation/run feedback/recipes, terminal sessions and shell integrations |
| Service/lifecycle | Optional explicitly started development daemon/web process, private state lease/atomic persistence, interrupted restart handling, cancellation and source/provider failure states | Packaged install/remove and native launch, enable-now/login/linger/disable choices, real Prowl start/stop/health/recovery/version inventory, service failure/recovery/removal/KDE tests and registered writer coordination |
| Protected state/mutations/recovery | Private atomic Store, rollback/quota/restart failure semantics, descriptor-bound grants; no external covered mutation is exposed | #25 authenticated NSS/pidfd/native coordinator seam, exact InstalledInventory enrollment of all Rashin state/effects, writer leases, action authorization/revalidation, verified linked pre/post generations and offline restore proof |
| Source/package/VM closure | Retained source/license inventory, exact runtime executable provider receipts and offline dependency checks; no large downloads | Complete vendor/export/RPM/native dependency closure and packaged Tumbleweed install/login/interaction/usage/restart/removal/KDE acceptance after #25/#173 integration |

Before production integration the coordinator must enroll the exact durable
Rashin state and every external effect in installed capture/restore policy, add
the real process to the writer lease protocol, and route each covered action
through the trusted native mutation coordinator. The present standalone custom
directory is not in `InstalledInventory`; a state-file hash or an authenticated
UID must never be accepted as a substitute for native linked checkpoint evidence.
The unprivileged API cannot manufacture a `VerifiedLinkedGeneration`.

After #25/#173 integration, complete the full behavioral map and demonstrate the
packaged UI/service in the existing Tumbleweed VM: install, login, configured or
fixture interaction, usage, restart, removal and KDE fallback. Native acceptance
and exact-head hosted CI remain separate gates; no issue closure is requested.

## Validation

`suse/rashin/tests/workflows.rs` exercises the actual shared handlers and launches
the built Rust executable for real loopback HTTP and Unix CLI requests. It covers
index/source integrity, Ask/Agent/Chat separation, effective local wiring,
Fix investigation, unavailable model preservation, private credentials, typed
action authorization, HTTP authentication/Origin/CSP/XSS/smuggling, bounds,
cancel/failure/recovery, restart interruption, deletion and unsafe filesystem
objects. Tests require local sockets, so the execution environment must allow
socketpair and bind. No real provider key, remote request, model download or VM
is required. See the issue183 evidence TOML for exact commands and scope.


## Browser credential isolation

The former login response put the raw owner bearer in a host cookie. A browser
could send that cookie to another service on the same loopback host at a different
port; HttpOnly, SameSite and request Origin checks did not prevent disclosure to
that service. Cookies have no port isolation ([RFC6265 §8.5](https://www.rfc-editor.org/rfc/rfc6265.html#section-8.5)).
The gateway now issues no authentication cookie and ignores all cookie credentials,
including a legacy valid token with a forged exact Origin. Browser authentication
uses the HTTP protection space, which includes scheme, host and port
([RFC9110 §11.5](https://www.rfc-editor.org/rfc/rfc9110.html#section-11.5)).
Only the canonical Basic credential for username `owner` is accepted. Explicit
owner and delegated Ask bearer APIs retain their existing typed authority.

The threat is a different local web service receiving ambient browser credentials.
A standards-conforming client does not forward this authentication to another
origin, even when that service challenges with the same realm. The exact Host,
loopback peer, Origin checks, no-store responses, restrictive CSP, escaped content,
relative-only redirects and absence of credential URLs remain required. Every
browser mutation requires the exact Origin; an unauthenticated same-origin form
can challenge and retry without executing the action first. Provider forms keep
keys private and a blank edit retains the saved key. Saved unavailable model picks
remain visible until explicitly changed. No external provider request is needed
to render or edit these forms.

This boundary assumes the user does not manually give another service the token,
and that the browser implements HTTP origin/protection-space rules. A process
with owner filesystem/socket access already has account authority. Loopback HTTP
does not defend against privileged traffic inspection or a compromised browser.
A previously leaked bearer remains usable until web restart rotates it; restart
is required when upgrading from the cookie implementation. Automated Rust handler
regressions and an independent standard-client two-port proof exercise the current
boundary. An actual browser engine is unavailable on this host; native browser,
WebKit, packaged UI and Tumbleweed VM acceptance remain open alongside the full
retained ACP/Prowl/provider/profile/sign-in/terminal/recovery capabilities.


## Explicit local Prowl gateway connection and profiles

`configure_prowl` accepts a nonzero `port` and a private management `token`.
The destination is always `127.0.0.1`; it accepts no URL, host, path, process,
credential filename or generic proxy request. The token is bounded printable
ASCII (16–512 bytes). Omit `token` to retain a saved credential. `prowl_status`
returns only configured/port/credential-present metadata. `disable_prowl` removes
the connection and credential from current private state; it does not stop or
remove Prowl. Historical state/recovery material still needs private retention.
The owner browser's Prowl gateway page provides the same configuration/read
workflows; blank token edits preserve the secret. No token is rendered back.

Owner-only `read_prowl` takes a typed `query`:

| Query operation | Fixed upstream endpoint | Returned data |
| --- | --- | --- |
| `profiles` | `GET /api/profiles` | Retained profile fields and model counts |
| `active_profile` | `GET /api/profiles/active` | Explicit profile ID or null |
| `profile_models` with positive safe-integer `id` | `GET /api/profiles/ID/models` | Membership priority/enabled/provider/model/ranks/limits/budget |
| `profile_presets` | `GET /api/profiles/presets` | Preset metadata and requirements |

Example request: `{"action":"read_prowl","query":{"operation":"profiles"}}`.
The original Prowl management bearer stays server-side. Reads never rewrite saved
Rashin lane/model picks, create conversation turns or count model usage. A new
connection generation rejects a response captured before reconfiguration or
removal. No automatic gateway spawn, ensure, autostart, model download, OAuth
request or external endpoint occurs. A missing/stopped/unprovable gateway returns
unavailable; HTTP unauthorized/not-found/malformed responses remain errors, with
no fabricated empty profile data. Typed response fields discard unknown fields
instead of forwarding arbitrary upstream bodies, and a raw token echo is refused.

The transport connects once, then verifies the established server-side reverse
IPv4 four-tuple, nonzero socket inode and current owner UID from the fixed
`/proc/self/net/tcp` file on procfs before sending **any** request byte. Linux's
[documented TCP table format](https://docs.kernel.org/networking/proc_net_tcp.html)
provides those fields. A listening-port lookup alone is insufficient. Missing,
ambiguous, wrong-UID, malformed or unavailable ownership evidence never permits
credential send. A pending accept or ambiguous procfs snapshot is polled within
the same deadline until a fresh unique owned nonzero inode appears. A duplicate
zero-inode snapshot is never accepted as proof; persistent ambiguity fails and
a foreign UID is rejected immediately.

Rebinding a listening port cannot replace an already-established stream. The
verified stream is held through the request, with no reconnect or redirect; a
closed stream fails. Passing a socket/credential deliberately from an owning
process remains that account's authority. A different UID's listening socket is
refused before Host, Authorization or any other request data is sent. This proves
account ownership of the peer, not a separate identity among same-UID processes.
Privileged kernel/procfs compromise remains outside this boundary.

The Rust transport bounds connect to one second, total request I/O to three
seconds, HTTP headers/trailers to 16KiB each, decoded JSON body to 1MiB, and framing
plus body consumption to 1MiB+32KiB (with at most an 8KiB buffered read-ahead).
It accepts fixed-length, chunked and EOF-framed identity responses, refusing
ambiguous lengths, compression, malformed/truncated data and redirects. Procfs
inspection is capped at 4MiB and validated as procfs. This introduces no new
crate/package and leaves configured HTTPS completion/catalog transport unchanged.

The retained contract is pinned Ryoku 1.0 commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`:
`backend/prowlgw.go` blob `0fbe44ee981735e3095080f57052ae37ff735de9`
(default loopback8788, management bearer, no redirect), `web/src/lib/api/prowl.ts`
blob `1bf8a0327b47b72c43c94bba2357264e7d0091f2` (lines192–202 profile reads),
and `web/src/lib/pages/prowl/types.ts` blob
`a78f45721bda91d23c20e569917a2a1c783149b9` (lines555–633 response fields),
all relative to upstream `ryoku/rashin/`.

Same-UID real TCP fixtures cover configured reads/forms, private credential
retention, chunking, bounds, timeout, stale generations and failures. A separately
executed subordinate-ID user-namespace test proves the foreign UID1 listener
receives EOF with zero request bytes and the owner UID0 receives Unauthorized.
It uses existing mappings, creates no host account and requires no root/password
or download. The explicit test is ignored in default portable CI because
subordinate mappings are an environment prerequisite; its host result is recorded
separately from default counts. The packaged Tumbleweed kernel/procfs proof remains
required. Profile create/edit/activate/reorder/delete/presets, every sign-in and
other retained Prowl action, protected state/writer/recovery and full native/UI/VM
acceptance remain required subsequent functionality; these reads certify none of
those mutation or packaging gates.

## Browser handoff, local usage and Fix review

A completed Ask job offers two explicit continuations: stage its recorded question
and answer for the next Agent message, or run that canonical context in a chosen
existing Agent conversation. Chat conversations are not offered as Agent targets.
The workspace displays pending context and can clear it. Copied question/answer
form fields cannot replace the recorded source; accepted-ID replay remains
idempotent. Chat and other independent activity do not consume staged context.

The local usage page enrolls an explicit private Claude/Codex/opencode directory,
collects through the existing descriptor-bound readers, and disables enrollment
with cache removal. Optional opencode soft caps require both windows; invalid
edits preserve existing enrollment. Reports distinguish not-yet-collected,
fresh/partial/stale/unavailable activity and unavailable provider quota. Failed
collection retains a labeled last-good result. Transcript/credential content is
excluded from reports. The existing single-daemon persistent five-minute schedule
owns periodic collection; these controls add no service/timer, startup or linger
setting and perform no OAuth/RPC/provider request.

Built-in Agent instruction forms call the existing `wire`/`unwire` actions; a
replacement applies to Agent messages, with Ask and Chat separate. They do not
claim native/external harness configuration enrollment or linked recovery.

The Fix investigation page accepts a doctor/tip/app finding and first performs
`fix_preview`. It displays the redacted Agent destination, exact supplied finding,
bounded native observations, expiry and disclosure. Only an explicit checked
confirmation submits the bound kind/component/finding/consent token to `fix`.
Consent and credentials never enter a URL. Missing confirmation executes no job;
an externally routed mismatched/stale/replayed approval remains rejected by the
existing backend. Local Fix keeps its existing backend behavior. Accepted retry
returns the same work record without additional egress. The review POST returns
a private no-store page; execution redirects to the job GET, which shows progress
and terminal status. Proposed repairs still require the separately authorized
typed native provider and linked recovery; no shell command is executed here.

All new pages/forms retain owner authentication, exact-Origin browser mutations,
no cookie authority, escaped output and scoped Ask denial. Deterministic private
fixtures exercise actual handlers and runtime actions without external provider
calls. These browser paths do not establish retained native app/shell/ACP/harness
protocol/tool/approval/terminal or packaged Tumbleweed browser/VM acceptance.


## Offline Wiki and verified guide indexing

The owner-only `wiki_pages` and `read_wiki` actions expose all eight retained
Ryoku1.0 Wiki pages: overview, desktop, Go tools, Hyprland Lua, Linux basics,
Niri KDL, Quickshell QML and Rashin. Fixed IDs choose embedded assets; no path,
URL, subprocess or download is accepted. The originals total25,481bytes and
remain byte-exact, including their source Git blobs/SHA-256/byte counts. Original
GPLv3 and attribution are embedded and visible through the Wiki license page.
`upstream/evidence/rashin-v1.0/wiki-assets.json` records source and adaptation
provenance. Source/export/RPM closure remains the separate acceptance gate.

The default overview/Linux basics are Tumbleweed adaptations. The package
information uses official openSUSE [Zypper](https://manpages.opensuse.org/Tumbleweed/zypper/zypper.8.en.html)
and [Tumbleweed update](https://doc.opensuse.org/documentation/tumbleweed/updating_upgrading_reverting/)
references. Other guides carry explicit upstream-reference context; the Go guide
is retained for upstream sources while this port's owned executable code is Rust.
The exact original edition is separately readable with `original:true`; the
browser does not offer indexing that Arch-oriented reference by mistake.
Both the pinned upstream digest and current view digest are shown distinctly.

`index_wiki` takes a fixed page `id` and its current `sha256`. It verifies the
compiled source/view, then merges one `wiki/ID.md` document into the existing
bounded index, preserving unrelated uploaded documents. Identical indexing is
idempotent. A stale page digest or an existing different document returns Conflict;
replacement requires its exact `replace_sha256`. Browser replacement additionally
requires a checked confirmation naming the document. Stale confirmation does not
overwrite a newer document. Existing index size/content validation and atomic
private storage still apply. No reading/indexing operation executes Wiki commands.

Actual owner browser routes provide navigation, source/reference/license views
and index forms. Source HTML stays text, fenced code is escaped and only known
Wiki paths or explicit HTTPS links become anchors. Images/scripts are not fetched.
The existing owner/Origin/no-cookie policy and scoped Ask restrictions apply.
Private upload content is never included in public provenance or Wiki listings.
Deterministic tests index the guide, obtain real local Ask output with its source
and digest, and verify persistence after restart, replacement conflicts, safe
rendering and preservation of an existing uploaded document. Full machine/source
maps, vault/memory/journal/project enrollment, native Svelte/app/shell/ACP and every
retained provider/tool/terminal/recovery/Tumbleweed acceptance gate remain required.


## Prowl-backed assistant completions

After configuring the private local Prowl connection, use the browser's
**Connect Prowl provider** form or owner `connect_prowl_provider` with a provider
`id`, explicit route/model `models` and `streaming` choice. Connection adds a
provider choice; it does not rewrite Ask/Agent/Chat picks. Choose a lane model
explicitly afterward. The pinned upstream quick path uses `auto:fast` with
`POST /v1/chat/completions` and the existing gateway management bearer
(`backend/quick.go` lines475–487 and `quick_test.go` lines47–94 at the retained
Ryoku1.0 revision). The Rust provider uses that fixed endpoint; model refresh uses
the OpenAI-compatible fixed `GET /v1/models` metadata endpoint. Actual packaged
Prowl endpoint/version acceptance remains required.

The provider stores a reference to the single private Prowl connection, not a
second copy of its token. Worker port/token capture occurs with the accepted
job's provider/model state. A resolved credential context is excluded from JSON
serialization/deserialization and cannot be installed through `put_provider`.
Provider inventory and forms return only redacted connection metadata. Model and
streaming edits keep the credential reference; change the gateway port/token in
its connection form, where omitted/blank token preserves the existing value.
Connection edits/disconnect advance connected provider generations, invalidating
stale catalog results and old native Fix previews without rewriting saved picks.
Disconnected picks remain saved and visibly unavailable. Already accepted work
keeps its captured destination until terminal state or explicit cancellation;
disconnecting the configuration starts no automatic replacement or replay.

The model transport uses one direct Rust IPv4 stream to the configured loopback
port, proves the established reverse tuple/nonzero inode/current UID before any
request byte, and never reconnects, redirects or consults proxy environment.
Only fixed completion/model paths are available; model IDs remain opaque JSON
values. Prompt JSON is bounded to256KiB, HTTP body to1MiB, framing overhead to32KiB,
with the existing 8KiB buffered read-ahead. Connect is bounded to1s, peer proof to3s,
and total model request to45s. Model I/O checks cancellation at50ms intervals;
proof checks cancellation before any write. Foreign-peer proof covers both GET
and POST, including a prompt that must never reach the foreign account.

Both JSON and incremental SSE responses use the existing bounded completion
validation. Live deltas stay in ephemeral job memory; missing terminal, cancelled
or failed work publishes no partial turn/usage. Prowl's actual model and usage
are recorded separately from the saved route/model pick. Ask, Agent and Chat
keep their histories independent. A Prowl gateway can forward to external
providers; native Fix therefore requires the same reviewed egress consent as a
configured external completion provider. Fix destination classification resolves
the stored Prowl credential reference first: a configured gateway requires
reviewed consent, and a disconnected gateway is unavailable rather than local.
Native tool activity/escalation, ACP/harness
operations, Prowl server mutations/sign-ins/native lifecycle/protected recovery
and complete packaged acceptance remain subsequent required functionality.

Real local HTTP fixtures exercise owner browser connect/edit, streamed Ask/Agent,
non-streamed Chat, model refresh and usage, saved-pick retention, one-copy private
credential storage, unavailable disconnect state, stale credential/catalog races,
and cancellation closing the actual stream without partial transcript or usage.
No external provider account/request or model download is needed for these tests.

## Explicit read-only system tools

A Prowl provider's **Enable read-only system tools** browser permission, or owner
`set_read_only_tools {provider,enabled}`, enables the retained fast lane's
`system_query` function. It starts disabled and survives restart only after an
explicit owner change. The form explains that native query results go to the
selected gateway and may reach its configured external provider. Scoped Ask
clients cannot change this permission. Accepted jobs capture the permission with
their provider; changing it affects new jobs. Cancel an accepted job to stop it.
Native Fix always disables tool calls, preserving its reviewed bounded evidence.

The eight retained topics remain available with Tumbleweed commands: installed
packages/count through RPM; cached updates through `zypper --no-refresh`; user
service state through `systemctl --user show`; process names and CPU/memory through
`ps`; filesystem capacity through `df`; kernel release through `uname`; display
controllers through `lspci`; and interface addresses through `ip -brief addr`.
Executable paths and options are fixed. Model arguments select only an explicitly
validated installed package or user service name. There is no shell, command
substitution, arbitrary executable, repository refresh, installation or gateway
spawn. Process/network summaries retain the 16/12-line limits. Results label
truncation and failures; an unavailable native command never becomes fabricated
success. Runtime RPM requirements include the exact Tumbleweed provider names
`zypper`, `procps`, `coreutils`, `pciutils` and `iproute2`.

The bounded gateway loop follows pinned `backend/quick.go` lines389–429 and
`quicktools.go` lines27–135: at most four tool rounds, then one final answer round
without tool schemas. A tool call in that final round fails instead of executing.
Unknown tools and invalid arguments return bounded error results to the model;
they select no executable. Each round allows at most eight calls with 16KiB
arguments each, 128-byte IDs and 64-byte function names. Fragmented UTF-8/SSE
assembly preserves call IDs and arguments while enforcing existing frame/body
bounds. Missing terminal, duplicate IDs, malformed frames and excessive calls fail.
One 45-second deadline covers the entire loop; each native probe gets at most
eight seconds, checks cancellation and kills/reaps its child on every outcome.
No extra worker or background service is created. Token accounting accumulates
all rounds; intermediate tool messages/results stay ephemeral, while only the
successful final answer and aggregate usage enter private conversation state.

This is the first executable read-only tool slice. The complete retained
`read_file`/`list_dir` vault/config roots and secret/path boundaries, source
`search_code`, public-web `fetch_url` DNS/redirect restrictions, tool activity UI,
TOOLS_REQUIRED escalation and isolated ACP/harness operations remain required
follow-ups. Protected mutations and full native/package/VM acceptance stay open.

## Enrolled file and directory tools

Each Prowl provider can now receive separate owner grants for ordinary file
reads. In its browser section, enter a root ID and absolute directory, then check
the disclosure that this gateway/provider may receive its text. The owner API
uses `enroll_read_root {provider,id,path}`, `read_roots {provider}` and
`disable_read_root {provider,id}`. Scoped clients cannot enroll, inspect or disable
these roots. Up to eight roots per provider are stored; provider removal or
replacement by a different provider kind removes its grants. File enrollment
adds `read_file`/`list_dir` only; it does not enable `system_query`. Native Fix
keeps every tool disabled even when file/system permissions are enabled.

A grant binds the directory's device/inode and accepted work holds its verified
file descriptor. Tools accept an absolute path under that root or
`ROOT_ID:/relative/path`. A replaced enrollment is visibly unavailable and rejects
new work until explicit owner re-enrollment; accepted work keeps its captured
root descriptor. Overlapping absolute grants are ambiguous and fail, while a
root-ID path explicitly chooses its grant. Disabling a root affects new work;
cancel accepted work to stop its captured reads. The owner chooses paths
explicitly; no client-supplied HOME value establishes an account root.

Pinned `backend/quicktools.go` lines137–245 and `readonly.go` lines57–108 define
the retained vault/config/data reads and secret markers. Markers are checked on
both supplied and full physical names, so root-ID aliases cannot hide
`.config/prowl-agent`, keys, tokens, credentials or the other retained protected
names. This Rust port stores provider secrets in its private Store rather than
the upstream port-only configuration. Its held Store device/inode is therefore
excluded at enrollment and every walked component, including renamed or bind
aliases. Model-selected filenames cannot authorize Store access.

Descriptor-relative `openat2` refuses traversal, symlinks, magic links and mount
crossings. Regular files require one link and safe ownership/modes. `O_PATH`
metadata validation proves a regular file/directory before data-open through that
held kernel descriptor, preventing a path race from opening a device/FIFO.
Only the trusted numeric descriptor is reopened through verified procfs; no
model-selected proc path is followed. File identity, size, timestamp, mode and
link count are rechecked around reads. Directory changes fail instead of
returning a mixed listing. Secret, unsafe and protected listing entries are
omitted with an explicit omitted count.

Text reads return at most32KiB with truncation labeled; NUL-bearing binary data
is refused. Directory scans cap at4096 entries and return at most200 sorted
names with truncation/omission labels. Cancellation/deadline checks surround the
bounded file read and each directory iteration; these are cooperative checks
around kernel filesystem operations. Intermediate read results remain ephemeral;
only a successful final assistant answer and aggregate usage are persisted.
No write, filesystem repair, permission change, enrollment download or external
process is performed by these file tools.

Deterministic actual gateway tests exercise allowed list/read results, private
Store and renamed aliases, secret names, symlinks/hardlinks, replaced enrollment,
owner/browser disclosure, scoped denial, restart/disable, independent system-tool
permission, Fix isolation and cancellation. A separate explicitly executed
user/mount-namespace proof binds the Store below an enrolled root: direct alias
enrollment and nested mount crossing both fail. Portable CI keeps that
capability-dependent test distinct from its default suite.

This explicit enrollment path restores functional read/list workflows. Native
NSS/default vault/config/data discovery and complete protected enrollment/writer
recovery still depend on the stable shared native contract; no #25 files are
changed here. Source search, public-web fetch, tool activity/escalation and full
ACP/harness/native/package/VM acceptance remain required follow-ups.

## Indexed source search through Prowl

After enrolling an ordinary source root, separately select **Use as indexed
source** in its provider section and confirm the snippet disclosure. The owner
API is `set_code_search {provider,root_id}`; `root_id:null` disables source search.
`read_roots` exposes the saved selection and root availability. It survives
restart, while disabling/re-enrolling that root clears the selection. This does
not start a service, build/download an index or enable system queries. Select
an existing Prowl index of the enrolled source. Scoped clients cannot change it.

Pinned `backend/prowl.go` lines215–252 and `prowl_test.go` lines99–147 supply the
retained contract: authenticated `GET /api/code/search`, fixed `q`, `repo` and
`limit=20`, then at most12 tool hits with 160-byte snippets. The repo comes only
from the captured owner-selected root. UTF-8 query/path bytes are percent-encoded
into the fixed loopback endpoint; they cannot select a URL, header or option.
Queries are bounded to1024bytes and encoded paths to8KiB. The existing verified
same-owner connected stream and bounded response handling apply to every search.
Index I/O checks cancellation under a20-second bound within the whole tool loop's
45-second budget. No gateway management bearer enters a URL, tool argument,
returned hit or client log.

Only typed file/start_line/snippet fields survive decoding. More than20 hits,
malformed data, excessive fields or credential echoes fail without relaying the
raw response. Decoded metadata strings and snippet/file fields are checked before
truncation, so JSON Unicode escaping cannot hide the management credential. Every result path must identify an allowed ordinary file through
the held root; outside/secret/Store/link/missing targets are omitted with a count.
At most12 remaining hits and UTF-8-safe160-byte snippets reach the model. These
are labeled Prowl-index results, whose content freshness follows the configured
index. The selected root's device/inode is revalidated before and after the index
request; replacement during a held response returns Integrity and discloses no
snippets. A later re-enrollment requires an explicit new source selection.

The actual authenticated gateway fixtures exercise completion→index search→tool
result→completion, exact encoded query/repo/limit, owner/browser selection and
restart/disable, unsafe hit omission, unknown field removal, hit/snippet limits,
malformed/credential echo responses, cancellation during index I/O and a barrier
controlled source replacement. Index report/build/mirror lifecycle and the full
Prowl/native package acceptance remain required alongside public-web fetch,
tool activity/escalation, ACP/harness/process/terminal and protected native
state/writer/recovery integration. No retained feature is removed from the target.

## Owner-enabled public web text

Enable **public web reads** separately for a Prowl provider in the browser or use
owner `set_public_web {provider,enabled}`. It starts disabled, survives restart,
and does not grant system queries, file roots or source search. Scoped clients
cannot change it. The disclosure explains that model-selected URL requests reach
those public sites and fetched text reaches the configured provider. Accepted
work captures the permission; disable affects new work and Cancel stops accepted
work. Native Fix always suppresses every tool, including public web fetch.

The retained `fetch_url` contract is pinned `backend/quicktools.go` lines274–389:
public HTTP/S GET, at most three redirects, bounded readable text and no local or
private-network access. The cached Rust `url`2.5.8 parser normalizes IDNA and
unusual IP spellings before validation. URL userinfo, unsupported protocols,
controls, local/private/shared/link-local/unspecified/multicast/reserved targets
and non-global special-use prefixes are refused. The IANA
[IPv4](https://www.iana.org/assignments/iana-ipv4-special-registry) and
[IPv6](https://www.iana.org/assignments/iana-ipv6-special-registry) registry audit
includes the non-global `2001:2::/48` benchmarking range, which is already inside
the `2001::/23` exception whitelist. Mapped IPv4 is checked as its actual IPv4
destination; NAT64/6to4/Teredo forms also validate embedded IPv4 addresses.

Fixed `/usr/bin/getent -A ahosts -- HOST` resolves domains. All returned addresses
must pass, with no accepting a public address from a mixed private answer. Each
hop pins that validated set into curl; literal IPs remain literal. Curl never
follows redirects itself. Every relative/absolute Location is parsed and checked,
then DNS is revalidated and repinned for the next request. There is no generic
URL proxy, shell, fallback client or inherited proxy/credential environment.
Fixed curl argv contains no URL, query or credential; escaped URL/pin config goes
on stdin. Requests send no account Authorization/Cookie/netrc/client certificate,
and fresh per-hop processes carry no cookies across redirects. TLS verification
stays enabled; HTTP and HTTPS remain available.

One12-second fetch deadline sits inside the whole45-second tool loop. Resolver,
client and charset conversion are cancellable child processes with bounded pipe
pumps and unconditional kill/wait. Curl receives512MiB address-space,8CPU-second
and zero-core limits before stdin config can start a request, preserving stricter
inherited limits. Headers cap at16KiB; response/decompression consumption caps at
40KiB before a24KiB body prefix is retained. Plain output caps at8KiB, with
truncation labeled. Curl decodes supported content codings. UTF-8 has replacement
explicitly labeled; declared other charsets use fixed glibc iconv with bounded
stdin/output, no model-selected options. Partial conversion is marked lossy and
truncated. HTML tags and script/style bodies are removed; referenced images,
scripts and stylesheets are never fetched or executed. No response file or model
artifact is created. Only the final successful assistant answer and aggregate
model usage persist; intermediate page text remains ephemeral.

The exact installed Tumbleweed providers are curl8.21.0-1.1 and
glibc2.44-3.1 for getent/iconv. RPM requirements name curl/glibc and the three
absolute executable file capabilities. Missing tools fail unavailable without
fallback. The URL/IDNA/ICU dependency closure was already cached and resolved with
`--offline`; existing locked package versions stay unchanged. Vendor/export and
packaged acceptance remain required.

Portable units cover URL/IP/IANA, real local-host resolver rejection, mixed DNS,
exact curl environment/argv/pins, missing tools, headers/HTML/charset and bounds.
Explicitly executed fresh user/network namespaces assign public-classified
fixture addresses only to isolated loopback, with no external route or traffic.
Real curl fixtures prove public redirects, changed per-hop DNS pins, private
rebinding/redirect refusal, redirect caps, gzip/Latin-1 text and cancellation.
Actual authenticated gateway fixtures prove Ask→fetch→redirect→text→model, no
credentials at the web server, independent owner/browser permission, scoped
denial, restart/disable/Fix boundaries and cancellation without partial usage.
These namespace proofs are distinct from portable CI prerequisites. Tool activity,
escalation, ACP/harness/process/terminal, Prowl/index/native/writer/recovery,
complete UI/package closure and packaged Tumbleweed acceptance stay required.


## Tool activity in jobs

The pinned `ryoku/rashin/backend/quick.go:419–425` publishes tool start/finish
around real execution; `quick_test.go:270`, `ws_test.go:48–64`,
`acptools.go:15–39` and `web/src/lib/pages/chat/ToolItem.svelte:20–75` retain the
status, replay and UI contract. The Rust fast lane now reports real execution
through `job.tool_activity` and the authenticated `/job/ID` browser page.

Each generation has at most32 ordered records (four rounds of eight calls).
Records contain `ordinal`, a SHA-256 `call_hash`, one of the five fixed tool
names, `acp_tool` (or `unknown_tool`), `phase` and an optional typed `error`. Raw model call
IDs, unknown names, arguments, URLs, filesystem paths and tool input/output are
never copied to this status metadata. Original wire IDs/results still reach the
model in the existing bounded request; status logging does not change tools or
grants. Every update checks the accepted job generation, so an old worker cannot
append to a reused request ID.

Live `running` rows stay in bounded worker memory. Terminal `completed`,
`failed` or `cancelled` rows are saved with the job result in the same private
atomic state transaction; finished rows remain finished if a later tool is
cancelled. The no-JavaScript page refreshes while work runs and renders all
statuses and typed errors with escaped text. Older jobs without these optional
fields still load. Restart labels any retained unfinished row `interrupted`;
an abrupt crash before terminal publication can lose ephemeral activity, and
this status view does not claim a durable event journal or full WebSocket-v3
replay. Pending ACP tool detail/output/diffs and approvals remain required.

Activity retention is optional under the existing8MiB state quota. If rows would
displace the final answer, usage or a cancellation, the rows are omitted and
`tool_activity_truncated:true` labels the omission in API/browser. The reserved
terminal-error allowance and cancellation/generation checks remain in force.
No activity callback writes files, emits logs, invokes a shell or widens access.
Actual gateway fixtures hold a native search request while API/browser show
`running`, then prove success/failure/cancellation, privacy and saved restart
history. Separate tests cover interrupted restart, stale generation, all32 rows
and answer/cancellation retention at the state limit.


## Ask requests that need Agent

The retained `backend/ask.go:29` and `backend/askserve.go:362–382` represent a
requested machine change with `NEEDS_AGENT:` and a bounded one-line reason;
`backend/askserve_test.go:359–367` asserts that the marker is stripped. All
references are under `ryoku/rashin/` at the pinned1.0 commit above. The Rust Ask
lane now requests and recognizes this response, normalizes whitespace, clips at
safe UTF-8 boundaries to500bytes and stores `needs_agent:true` with
`needs_agent_reason` alongside the cleaned answer. An empty marker reason keeps
the upstream `(no answer)` fallback and typed requirement. Ordinary text and
non-leading/case-different marker mentions retain their meaning.

Ask's live progress holds possible leading marker fragments across delta
boundaries and suppresses the control response until its final cleaned result
is atomically published. The filter resets between actual tool rounds, so a
later answer cannot leak a marker after an earlier tool preamble. Its memory and
input stay within the existing128KiB progress bound. Cancellation/failure saves
no partial reason, answer or handoff; a completed first call remains in usage if
the fallback is later cancelled. Stale generations retain their existing guard.
Agent and plain Chat keep their own text and histories.

The authenticated job page labels **Agent handoff required**, escapes the
reason, and offers the actual existing Stage/Run controls. A completed flagged
Ask grants no new capability and starts no Agent by itself. Owner confirmation
through these controls captures the canonical question/cleaned answer, runs the
selected Agent session once, and preserves Chat separation and retry/restart
semantics. Restricted Ask clients cannot invoke owner handoff actions.

Actual JSON/SSE gateway fixtures exercise typed reasons, a tool preamble before
the marker, escaped browser display, explicit Agent execution, idempotent retry,
restart and separate Chat. Deterministic streamed fixtures prove split marker
suppression and cancellation/failure without incomplete records. The owner can
select a configured ACP model as Ask fallback. Leading `TOOLS_REQUIRED` and
unavailable or failed fast-lane providers trigger that captured route; the
marker is withheld from progress and answers. Without an available fallback a
tool request fails explicitly. The route, model and enrolled read grants are
captured when the generation starts, so subsequent configuration changes affect
only later jobs. The fallback uses the same isolated ACP Ask boundary and its
existing mutation denials. Completed primary and fallback calls receive separate
usage records, and the job shows the effective fallback and reason. Cancellation
continues to apply to the whole generation. Agent and Chat remain independent.
The real namespace fixture covers split markers, route/grant changes, denied
reads, unavailable fast route, cancellation, accounting and lane separation.
Complete ACP/approvals/diffs/replay/native UI and all other acceptance-map rows
remain required.


## Configured ACP adapter process

The owner can enroll an installed ACP ELF executable from the browser or with
`enroll_acp_provider` (`id`, absolute `path`, explicit `models`, one fixed
`arguments` mode: `stdio`, `acp`, or `experimental_acp`, and optional `network`).
Enrollment captures device/inode/size/SHA256 after a no-symlink bounded read;
each launch revalidates the artifact. Re-enrollment clears prior file/tool grants.
The provider inventory reports its fingerprint and network choice and keeps
saved model picks explicit. Owner model edits retain the artifact; real async
catalog refresh negotiates an adapter session and preserves unavailable saved
picks. Standard existing lane selectors route Ask, Agent and separate Chat to
this provider. The deterministic `ryoku-rashin-acp-fixture` executable exercises
this actual production path without model downloads or a model service.

The installed runtime requires exact executable capabilities for
`/usr/bin/unshare`, `/usr/bin/mount`, `/usr/bin/setpriv`, `/usr/bin/chroot` and
`/usr/bin/prlimit`, plus the `--map-auto` mapping helpers
`/usr/bin/newuidmap` and `/usr/bin/newgidmap`. The observed Tumbleweed providers are
`util-linux-2.42.1-1.2.x86_64` (four helpers) and
`coreutils-9.11-4.2.x86_64` (chroot) and
`shadow-pw-mgmt-4.19.4-2.2.x86_64` (mapping helpers); immutable file digests are in this slice's
receipt. This is host package ownership evidence; complete native/RPM closure
and packaged VM acceptance remain separate gates. Missing helpers fail
unavailable; there is no shell fallback or automatic installation.

Before executing the adapter, the Rust bootstrap creates fresh user, mount, PID,
IPC and UTS namespaces, plus a fresh network namespace by default. An explicit
owner network grant shares the configured host network. Only nonrecursive,
read-only `/usr/bin` and `/usr/lib64` runtime binds and exact null/random devices
enter a private256MiB tmpfs root. It does not recursively bind `/usr`, which can
contain locked WSL/C submounts. Private/home/state paths stay absent. The adapter
runs as a distinct mapped UID/GID65534 with no effective/permitted capabilities
and no-new-privs. Root/runtime writes, mount/signal/proc-parent/symlink escapes
and inherited descriptor leaks are refused; only stdio is inherited. Its own
home/tmp writes are ephemeral and bounded. Resource caps cover address space,
CPU, processes, descriptors and file size. The parent kills/waits the manager;
PID namespace death terminates descendants. Capability or subordinate-UID setup
failure is an explicit unavailable state, never a weakened launch.

Bootstrap configuration travels over stdin with exact-length raw reads, so it
cannot consume the queued ACP initialize frame. Bootstrap writes are nonblocking
with a three-second bound. The ACP1 NDJSON client uses nonblocking pipes, bounded
frames/total input/event count/deadlines and typed errors without raw provider
error bodies. It negotiates initialize capabilities, new/load sessions, explicit
model selection, prompt text, session-bound notifications and cancellation.
Wrong-session/wrong-ID/malformed/oversized responses fail. Current-model updates
record the actual model separately from the saved choice. Counts are estimated
when the protocol response provides no turn token counts.

The runtime's file broker uses the existing owner-enrolled held roots and Store,
secret/link/mount guards. Whole-file reads preserve exact UTF-8/LF/CRLF/trailing-newline bytes up to32KiB.
Explicit validated ranges preserve original line endings, cap output at32KiB and
scan at most1MiB; requests beyond the scan/output bound fail explicitly. Invalid
UTF-8 is refused rather than silently replaced. Permissions are selected from the adapter's
actual options: enrolled read access or the explicitly enabled fixed kernel
probe can be allowed once; broader operations remain denied until their native
writer/approval integration exists. The adapter can run its permitted internal
probe inside the jail. Host file writes and the separate ACP client-terminal
capability are not advertised. This slice does not claim full terminal,
approval, diff or protected native mutation implementation. Ask denials produce
a typed Agent-needed reason while preserving the answer; Agent/Chat stay separate.
Bounded `acp_tool` status uses fingerprints and typed errors, with no raw paths,
URLs, inputs or outputs in activity metadata.

Text thinking notifications are displayed separately from the answer in the
authenticated job page. Live thinking is held in memory, limited to 32 KiB and
discarded on cancellation or failure. Completed thinking is stored in private
job state and survives restart; it is omitted if necessary to preserve the
answer and usage at the state size limit. Browser output is escaped. The real
ACP fixture verifies live visibility, cancellation, bounds, scoped access,
completed persistence and restart. This does not claim complete native replay
or all upstream thinking controls.

ACP `usage_update` reports the provider's current context used and size as
separate values in the live and completed job page. These values describe the
context window, not billed tokens or provider quota. Invalid or negative values
fail the turn, cancellation discards the live update, and completed updates
survive restart. A state-size limit may omit the update to preserve the answer.
The real ACP fixture covers live reporting, cancellation, malformed values,
browser display and restart.

ACP session title and available-command notifications are bounded and shown in
the authenticated job view. Titles from completed Agent or Chat turns also
label their local conversation and survive restart; cancelled or failed turns
do not change the saved title. Commands carry only the name, description and
optional input hint, with unknown provider fields discarded and output escaped.
Titles remove the retained upstream identity and Ask handoff preambles before
display or persistence.
At the state limit, optional details can be omitted to preserve the answer. This
does not establish persistent native ACP session replay.

ACP adapters may advertise models either through the legacy model list or a
`model` select configuration option. Catalog refresh and explicit model picks
use the corresponding protocol method, and configuration updates update the
actual model without rewriting the saved lane pick. The namespace fixture
exercises catalog, selection and live updates through the real engine.

Prowl connections retry transient connection timeouts for at most three seconds
before sending any request or credential. Each attempt is limited to one second
so cancellation remains observable. The established connection still requires
the same peer UID verification before any request bytes leave the process.

Network-enabled ACP Fix uses the existing exact finding/configuration-bound,
expiring, single-use review contract. Tools are disabled for Fix. An independent
loopback observer proves zero adapter calls before consent, one authorized call,
and no extra traffic on token replay or identical accepted-ID retry; no external
network is used in this proof. Actual browser enrollment/confirmation, reads and
secret denial, independent Agent/Chat jobs, model updates/catalog, cancellation,
re-enrollment grant clearing and restart are tested through the real engine.

The pinned source contract remains `backend/acp.go:610–707,834–883,995–1106,
1188–1242,1246–1290` and its `acp_test.go`, `acp_tail_test.go`, `acptools_test.go`
at the retained1.0 commit. Runtime completion currently opens a fresh isolated
session per job and supplies the explicit conversation context. The real client
load/replay contract is tested, but runtime shared/persistent native ACP session
resume still requires integration. All original harnesses, interpreter/runtime
closure, authentication/config options, image/thinking/plan/tool details, full
terminal and approval/diff/replay UI remain required.
NSS/default state, installed inventory/writer leases and linked recovery for
covered effects, remaining Prowl/provider/usage workflows, native app/shell/Hub,
full package closure and Tumbleweed acceptance remain in the full issue scope.
