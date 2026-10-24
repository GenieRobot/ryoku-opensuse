# Rashin 1.0: retained capabilities and implementation gates

This is the preparatory inventory for [#183](https://github.com/GenieRobot/ryoku-opensuse/issues/183).
The full Rashin assistant and usage experience remains part of the Tumbleweed
target. Runtime implementation and acceptance remain pending the explicit
[#25](https://github.com/GenieRobot/ryoku-opensuse/issues/25) and
[#173](https://github.com/GenieRobot/ryoku-opensuse/issues/173) prerequisites.
This document enables no operation and claims no assistant, packaging, provider,
recovery or VM acceptance.

## Immutable source and separate target disposition

The source is Ryoku Amaterasu 1.0.0 commit
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`, full tree
`06b2962f82f43118c3a98372c2a4b435b689b725`.
The [source ledger](../upstream/evidence/rashin-v1.0/source-components.tsv)
contains exactly every path in `ryoku/rashin/`, including tests, vendored code,
committed web bundles, fonts, notices and symlinks. Every row records its Git blob
and mode, current Source2/Source3 selection, a separate
`target_disposition=retain-entire-component`, `owner_issue=#183`, and a capability
group. Component counts are:

| Component | Paths | Retained purpose |
| --- | ---: | --- |
| app | 5 | Native window, launcher entry, icon link, build and app documentation |
| backend | 156 | Assistant/HTTP/WebSocket/ACP/terminal implementation, tests, embedded web distribution and dependencies |
| skills | 8 | Ryoku skill and build, WM, plugin, UI, troubleshooting and feature guidance |
| systemd | 4 | Rashin, Prowl, usage collector and persistent collector timer |
| usage | 4 | Collector guard plus Claude, Codex and opencode usage collectors |
| web | 153 | Svelte console, protocol/API clients, all sheets, tests, assets and build inputs |
| wiki | 8 | Offline Ryoku and Linux guides |
| **Total** | **338** | **Entire component retained as the target** |

`current-not-selected`, Source2 `-`, and Source3 `no` describe the reviewed
existing export/RPM gap. They are not an approved feature exclusion. The ledger
copies those facts without changing the #173 disposition ledger, export bits,
archives, source hashes, package spec or installed behavior. The authoritative
#173 review is linked from its [issue](https://github.com/GenieRobot/ryoku-opensuse/issues/173).
Git mode `120000` records the link object itself; it is not permission to follow
an arbitrary installed link. Compatible upstream UI may retain its language and
notices. Newly authored project-owned executable code must be Rust.

## Entry points and bridges

The [entry-point ledger](../upstream/evidence/rashin-v1.0/entrypoints.tsv)
records the 34 literal HTTP/WebSocket registrations in `backend/server.go`, the
static console, 89 individual Prowl client calls, daemon and alias CLI dispatch,
and the nine inbound chat actions. Source lines refer to the pinned upstream
revision. Prowl rows record client call sites and parameterized path expressions;
they do not certify an external Prowl server's contract. Query parameters and
request/response schemas still need typed operation review. HTTP method alone
does not determine side effects: a probe, GET or lifecycle check can perform
network I/O or start a process.

The [bridge reference ledger](../upstream/evidence/rashin-v1.0/bridge-references.tsv)
adds 200 exact external path/blob/mode references discovered from Rashin/usage
names, including the usage service itself. These are deliberately labeled
`reference-discovery`: a mention or an upstream test is not proof of an executable
entry point, an accepted dependency, or shared-component ownership. The concrete
integration surfaces to retain are:

| Surface | Pinned source paths | Contract and adjacent owner |
| --- | --- | --- |
| App/browser | `ryoku/rashin/app/{main.cpp,rashin-app.desktop,CMakeLists.txt}`, `backend/{server,webserve}.go`, `web/src/lib/app/router.svelte.ts` | Single-instance app, ping/offline state, `#/agent`, external links, notification and window state; #183, dependency closure #173 |
| Console sheets | `web/src/pages/*Sheet.svelte` | Agent, Ask, Chat, Wiki, Overview, System, Vault, Memory, Skills, Prowl and About; #183 |
| Ask bar/launcher | `ryoku/shell/quickshell/shell/modules/ask/`, `modules/launcher/shared/AskPanel.qml`, `services/{AskSession,Needle}.qml` | Ask/cancel/history, shared live Agent state and handoff; #183 assistant contract, #25 shell/session integration |
| Shared chat state | `ryoku/shell/quickshell/shell/services/lib/chatstate.js`, `web/src/lib/chat/` | Protocol-v3 transcript/replay, tools, permissions, model/session/usage and notification state; #183 |
| Usage UI | `ryoku/shell/quickshell/shell/services/AiUsage.qml`, qsbar AI widget/panel, dock AI configuration/panel/tile | Private local caches, optional collector start and visible availability; #183, #25 session boundary |
| Hub | `ryoku/hub/backend/rashin.go`, `quickshell/pages/RashinPage.qml`, `quickshell/pages/UpdatesPage.qml` | Availability, on/off, setup/progress and Fix with AI; #183-specific actions, #180/#181 other Hub actions |
| Terminal shells | `ryoku/apps/bash/rashin.bash`, `fish/conf.d/rashin.fish`, `zsh/rashin.zsh` | Proposed command/confirmation buffer, history, continuation and recipes; #183 |
| Desktop binds | `ryoku/wm/binds.go`, compositor adapters and tests | Ask/window shortcuts, presence checks and focus; #25 Hyprland/session, #46/#185 phased Niri/Wayfire; #183 assistant affordances |
| Doctor/update | `ryoku/cli/internal/doctor/{rashin_hint,reconcile_rashin_daemon}.go`, `internal/updater/{update,pieces}.go`, manifests | Fix hint, opt-out-aware service/wire/index refresh and optional package retention; #183 assistant contract, #25 session, #27–29 package transactions |
| Delivery/install | `release/packages/ryoku-rashin/`, `release/packages/prowl/`, installer plan/choices/deploy and service manifest | Optional delivery, no implicit reinstallation after removal; #183 Rashin requirements, #173 source/package closure, #24 installer |
| Edition references | Fedora RPM, Void package/init, Nix package and upstream packaging tests | Packaging comparison only; retain capabilities while replacing edition-specific actuators, #173/#183 |

Every path in the first table is relative to upstream `ryoku/rashin/`; external
paths in the bridge table are relative to the pinned upstream root. Neither
ledger imports or executes those files.

## Capability and side-effect matrix

The [machine-readable matrix](../upstream/evidence/rashin-v1.0/capabilities.tsv)
joins source and entry-point groups to retained behavior, side effects,
Tumbleweed provider boundaries, privacy/recovery obligations and required tests.
All rows are `pending-implementation-and-acceptance`; this is a review map.

The following distinctions must survive the adaptation:

- Ask is a separate, read-only lane with streaming/cancel/recent history and
  explicit handoff. Ask history is shared across bar, launcher, console and
  terminal; it does not silently enter the Agent transcript. Ask can record its
  own history while model tools remain read-only.
- Agent and plain Chat are separate persistent ACP sessions with live replay,
  images, thinking, tool output/diffs, approvals, cancel, session new/load/history,
  usage and terminal resume. Hermes, Codex, Claude Code, Oh My Pi, opencode and
  Gemini are retained harness choices; available ACP adapters determine readiness.
- One saved model pick per harness applies across Agent, Chat, loaded/new and
  private Ask sessions. An unavailable saved model remains visible without
  silently overwriting the preference. Ask route selection is independent.
- Connect a harness, choose its default, select Rashin's harness, select its
  model and select an Ask route are distinct actions. Background refresh updates
  only already-connected harnesses and must preserve the user's other choices.
- Terminal asks propose validated commands and read/write/system/danger tiers;
  command-buffer confirmation, continuation, resume, run feedback and recipes
  remain available. The assistant must not turn a proposal into an unguarded
  system mutation.
- Fix with AI supports doctor findings, tips and app incidents, gathers relevant
  logs/ownership context, opens the user's suitable harness and survives daemon
  restart in its own transient user unit. Repair mutations still need the typed
  per-action authorization/recovery path; a harness approval is not that path.
- Indexing retains system, desktop, packages, source/repository, user and habits
  maps, vault/memory/journal, offline skills/wiki, bounded source mirror and
  registered projects. User-configurable vault/source paths need explicit
  bounded enrollment and revalidation, preserving that configurability.
- All provider/key/sign-in, model/catalog/profile/fallback, cache/log/usage,
  project/harness/skill and network workflows visible in the Prowl console remain
  targets. The upstream broad `/api/prowl/{rest...}` proxy becomes individually
  typed, authenticated operations; these labels do not remove any action.
- Rashin stays optional at login. Enable-now, login autostart, user-selected
  boot lingering, ensure/restart, disable and removal are distinct lifecycle
  choices. The upstream default-on doctor/ensure behavior must respect the
  Tumbleweed user's choice. Prowl follows Rashin's lifecycle; usage collectors
  run only for configured/detected tools, on the persistent five-minute schedule.
- Usage preserves source semantics: Claude OAuth quota plus local activity,
  Codex app-server rate-limit buckets plus local activity, opencode local
  JSON/SQLite activity/cost and soft caps. Estimates must remain visibly
  distinguished from provider quota; stale/expired/offline data must be labeled.

## Authorization, privacy, integrity and recovery

Upstream loopback binding, Origin checks and `X-Rashin-Client: console` are
browser-origin controls, not authenticated user identity. The port needs an
authenticated desktop/browser/CLI-to-Rust boundary with typed, per-action
authorization and no public arbitrary-command or generic privileged-proxy escape.
The exact #25 transport must be reviewed before implementation stacks on it.
Prompt/model/provider responses, images, credentials, history, logs and usage
must remain private to the owning account. Public evidence uses synthetic data.
Prowl's management token stays server-side; user-requested credential reveal
remains a separately authorized private action. Network routing/probes/sign-ins
must respect the user's configured providers and network choices.

The durable state inventory must include all declared changes: Rashin config,
model picks, private Ask/session/history/recipe/habits state, vault/source/index
state, instruction marker blocks, skill links, connected harness configs,
provider credentials and routing, project registration, unit/autostart/linger
state and usage caches where applicable. Current `InstalledInventory` includes
`.config/ryoku/rashin.json` only; it does not establish coverage for the other
Rashin effects. Sensitive recovery material must retain private ownership/modes
and must not be exposed in public manifests, diagnostics or evidence. Rebuildable
indexes/caches need explicit recovery semantics rather than silent omission.

Before a covered mutation, verify the linked Snapper/configuration generation,
revalidate affected paths/identities and coordinate every affected writer. A
required capture/verification failure prevents mutation. Preserve unrelated home
files and marker-block contents; removing only Rashin-owned wiring must not
delete an agent's own configuration. Record post-change state and accurate
failure/interruption history. Offline recovery must include the required local
inputs and instructions and require neither private companion nor network.
Do not infer that upstream startup/reindex/timer/doctor healing writes are exempt.

Model/source/executable installation must use pinned, verified inputs and reviewed
Tumbleweed package/process boundaries. Setup's upstream remote Hermes installer,
host-package advice, Prowl self-update and other arbitrary downloader/command
paths need protected equivalents. Preserve provider/model/setup behavior; replace
the mechanism, and label external-provider interactions requiring credentials.
App GTK3/WebKitGTK, web/font notices, Prowl source/license and provider contracts,
ACP harness dependencies and source-to-built-bundle provenance need #173 and
#183 review; no package name or dependency closure is inferred here.

## Prerequisites and remaining acceptance

At the 25 October 2026 read-only GitHub audit, #20 was closed after PR #152 and
its linked identical-input normal/interrupted native pair. That establishes the
existing mutation coordinator contract, not certification of Rashin providers.
#25 and #173 were open: #25 lacked complete protected transport/session/native
acceptance, and #173 lacked complete semantic/RPM/import/ELF/license and refreshed
1.0 native acceptance. No dependency implementation is included here.

After those gates, #183 needs actual packaged Rust-backed web/API and assistant
tests with a deterministic local provider: ask/cancel/history, index, fix and
agent wire/unwire, ACP conversation/model/route operations, credentials and
authorization, source/model integrity, service failure/restart and linked offline
recovery. Fixtures must exercise real handlers and flows without large downloads.
Authentication alone does not prove recovery coverage.

The Tumbleweed VM gate must demonstrate install, login, an actual assistant
interaction through the packaged UI/service, usage reporting, restart, removal
and KDE fallback, using a test or configured provider. External behavior needing
credentials is labeled. Exact-head hosted CI and issue-linked evidence remain
required. This inventory milestone closes neither #183 nor any prerequisite.
