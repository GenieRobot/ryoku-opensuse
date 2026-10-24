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
directory, and enter that token in the login form. It is rotated on each web
daemon start. Index a document, then ask a question containing terms from that
document. Refresh the console to see the answer and usage. The default
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
kernel peer UID before parsing. HTTP requires a private bearer token, or the
HttpOnly SameSite=Strict cookie plus the exact configured Origin for POST.
Origin, Host and `X-Rashin-Client` are not identity credentials. Cross-origin
requests, duplicate HTTP headers, chunked bodies, oversized frames and arbitrary
commands are refused. Browser responses are private, uncached, escaped, and have
a restrictive CSP; the console requires no JavaScript.

| Action family | JSON actions | Effects and authorization |
| --- | --- | --- |
| Inspect | `status`, `providers`, `get_index`, `recent`, `sessions`, `session`, `job`, `usage`, `logs` | Account-private reads; provider inventory never returns keys; logs run only fixed bounded native readers |
| Ask | `ask`, `cancel` | Read-only provider lane, private independent history, bounded asynchronous execution and cancellation |
| Conversation | `new_session`, `prompt`, `forget_job`, `delete_session` | Persistent independent Agent/Chat transcripts; deletion removes associated jobs and usage records |
| Credentials | `put_provider`, `remove_provider` | Owner configures/removes an explicit provider; keys remain only in private state and provider stdin |
| Route/model | `set_route` | Owner selects a provider and one of its configured model IDs for an explicit lane |
| Index | `index` | Owner replaces a bounded set of explicitly uploaded UTF-8 documents; every SHA-256 is verified before publication |
| Fix | `fix` | Owner supplies a typed doctor/tip/app finding; collects bounded user-unit/journal/RPM observations and creates an Agent investigation request with Tumbleweed ownership/recovery instructions |
| Local agent instructions | `wire`, `unwire` | Owner installs/removes instructions for the built-in Agent lane; Ask/Chat are unaffected |

`authorization::Access::AskClient` is a separately tested restricted library
policy: it cannot configure credentials/routes/index/wiring, start Fix/Agent/Chat,
or inspect/cancel an Agent job. Neither HTTP nor Unix requests accept a claimed
role. Current installed transports are not extended by this branch; the
standalone owner transports deliberately grant the authenticated account owner
its runtime actions. A future shell/model transport must assign its role on the
trusted side instead of forwarding an owner credential to a model.

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
native state changes. The runtime package requires `systemd` and `rpm >= 4.19`.

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

Select it with `set_route` fields `lane`, `provider`, `model`. Ask, Agent and Chat
have explicit independent routes in this slice. Removing or updating a provider
does not silently overwrite an unavailable saved model/route. A new request then
reports unavailable until the owner explicitly chooses an available route.
There is no automatic fallback to a different external provider.
Choosing an external route sends that lane's prompt, indexed evidence and its
conversation context, including Fix's native evidence excerpts, to the
explicitly configured endpoint. Provider keys are
not inserted into assistant context.

Usage records preserve provider/model, input/output tokens, timestamp and
whether token counts are estimates or fixture results. Provider-reported counts
come from the completion response. The runtime's activity is explicitly **not
provider quota**; quota is reported unavailable. Upstream Claude OAuth, Codex
rate-limit and opencode collectors remain separate required work.

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
size limits refuse the request without publishing it. Jobs, sessions, documents,
model catalogs and in-flight provider workers have explicit bounds. Forget/delete
actions make the retained-history bounds usable for ongoing operation.

Private state is not exported as diagnostics or committed evidence. The offline
tests contain synthetic text/keys only. **Atomic private state publication and
these failure semantics are not linked Snapper/configuration recovery.**

## Full Ryoku 1.0 acceptance map

The 338-path source ledger, 34 upstream HTTP/WebSocket registrations, 89 Prowl
client calls, bridge references and capability matrix remain retained targets.
No row in the original inventory is removed by this slice.

| Retained upstream capability | Implemented here | Required for #183 acceptance |
| --- | --- | --- |
| Ask/recent/cancel/handoff | Real asynchronous Ask, cancellation, independent history and explicit Agent sessions | Upstream streaming protocol, shell/launcher shared history and explicit UI handoff |
| Agent/plain Chat | Independent persistent turns, sessions, load/read/delete and saved lane routes | ACP initialize/session/prompt/notifications, v3 replay/WebSocket, tools, approvals, images, thinking, terminal resume and all retained harnesses |
| Fix doctor/tip/app | Typed investigation using configured Agent route, bounded native user-unit/journal/RPM context; evidence is data | Complete app logs/coredumps/package-change context, upstream transient fix unit/terminal harness, protected repair providers |
| Agent wire/unwire | Effective built-in Agent instructions | Exact external marker/skill/MCP wiring, preservation of unrelated config, harness readiness/default/model operations |
| Index/vault/memory/wiki/skills | SHA-256-verified uploaded local document retrieval | System/package/desktop/source/user/habits maps, bounded source mirrors, registered projects, vault/memory/journal enrollment and complete offline skills/wiki |
| Provider/model/network | Local/fixture and configured HTTPS completion, private keys, catalog selection, unavailable-pick retention | Every individual Prowl provider/sign-in/model/profile/fallback/cache/network/project/harness/skill operation, adapter/source integrity and full settings UI |
| Usage | Real completion activity, labeled estimates and fixture data | Claude OAuth quota/activity, Codex app-server buckets/activity, opencode JSON/SQLite costs/soft caps, collector schedule and upstream usage UI |
| Web/app/API | Tested authenticated Rust HTTP/Unix boundary and basic browser console | Retained upstream Svelte console/native app and all source routes/sheets, theme/vitals/system/vault content and shell/Hub bridges |
| Terminal | Typed JSON CLI, no arbitrary command execution | Upstream proposed-command tiers, command buffer confirmation, continuation/run feedback/recipes and shell integrations |
| Lifecycle | Explicit optional standalone process, restart and stale socket recovery | Optional RPM install/remove, enable-now/login/linger/disable choices, Prowl lifecycle, no implicit reinstallation and registered writer coordination |
| Protected mutations/recovery | No system/service/package/external-harness mutation exposed | #25's authenticated typed dispatch to #20 native coordinator; complete state enrollment, writer leasing, verified pre/post linked generations, authorization/revalidation and offline restore proof |

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
