# Rashin local usage collection

This is implemented #183 runtime work, with remaining full Ryoku 1.0 gates in
[the runtime acceptance map](rashin-runtime.md). No live OAuth/RPC, external
provider, model download or credential is required for these collectors.

Use the owner-only `enroll_usage_source` action with a `tool` (`claude`, `codex`,
`opencode`) and absolute `path` to that tool's private data root. For example:

```json
{"action":"enroll_usage_source","tool":"claude","path":"/absolute/private/claude-data"}
```

The root must belong to the daemon's account, have mode `0700`, have no symlink
ancestors, and retain its enrolled device/inode identity. Enrollment does not
repair permissions. Custom roots are supported through this explicit bounded
enrollment. `collect_usage` with `tool` collects immediately.
`disable_usage_source` removes enrollment/cache without deleting tool files.
The restricted Ask client cannot enroll, collect, disable or read usage.

| Tool | Read-only paths under the enrolled root | Local source semantics |
| --- | --- | --- |
| Claude | `projects/**/*.jsonl` | Assistant input/output plus cache-read/cache-creation tokens; five-hour/seven-day/recent-hour activity and UTC days |
| Codex | `sessions/**/*.jsonl`, `archived_sessions/**/*.jsonl` | Cumulative-token deltas, duplicate-total suppression, counter resets and last-turn fallback; system-local days |
| opencode | `storage/message/*/*.json`, `storage/session/message/*/*.json` | Assistant input/output/reasoning/cache-write tokens; cache reads shown separately and excluded from caps; reported costs and model/provider breakdown; system-local days |

The exact retained upstream implementations are
`ryoku/rashin/usage/{claude-usage,codex-usage,opencode-usage,ryoku-ai-usage}` at
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`; their blob/mode identities remain in
the #183 source ledger. This Rust slice retains the local counter/window
semantics while bounding source reads. It does not invoke those scripts.
One-shot CLI variants `usage-claude`, `usage-codex`, `usage-opencode` accept the
private data root as the second argument and emit the same report.

`serve` and `web` run a five-minute collector only for explicitly enrolled
sources. Last-attempt timestamps persist: restart catches up an overdue source
once; a failed attempt prevents a tight retry loop. A collection lease prevents
overlapping manual/scheduled reads. Source changes during collection cannot
publish into a different enrollment. Disabling a source never implicitly
reenrolls it. Clock rollback makes the source due again.

The single Rust daemon owns this schedule, guarded by its exclusive state lease
and once-only scheduler start. This slice installs/enables no systemd unit or
timer and starts no upstream collector process. The retained separate
`ryoku-ai-usage` systemd timer/cache bridge still needs deliberate integration
with this scheduling owner so packaged login does not create duplicate collectors.

Reports distinguish `fresh`, `partial`, `stale` and `unavailable`. Failure keeps
a labeled stale last-good report with a sanitized error class. A failed first
collection has no `updated_at`, rather than claiming an authoritative zero.
Malformed or unsupported SQLite data is unavailable. The
[read-only SQLite fallback](rashin-sqlite.md) now handles both retained table
families and committed WAL snapshots. Malformed JSON and file/scan limits label
partial results. Oversized Codex tails use last-turn counts rather than assigning
an unknown historical prefix to the current window. Missing costs are labeled
unavailable rather than identifying activity as free.

Live Claude OAuth quota, Codex account-rate-limit RPC and session-limit fallback
are not inferred from local activity. Quota stays unavailable here. Opencode's
default local caps are 1,000,000 tokens/five hours and 20,000,000/seven days.
Typed opencode enrollment can set `soft_caps` fields `five_hour_limit` and
`seven_day_limit`. Caps are explicitly local estimates, never provider quota.
Live quota, complete model/session rows, the separate timer and
the original shell/cache UI remain required #183 acceptance work.

Readers pin directory descriptors, reject symlinks/hardlinks and unsafe
owners/write modes, revalidate enrolled identity, and never read root credential
files. Limits are two seconds/scan, 4096 selected files, 512 directories, depth
eight, 4 MiB/file, 64 MiB total, 100,000 activity events and 128 model rows.
Transcript/credential fields are not retained in reports, assistant context,
state cache or diagnostics. Root enrollments and private aggregate caches join
the pending #183 linked recovery and writer-coordination contract.

RFC3339 and local-calendar/DST handling use cached pinned Jiff 0.2.24 with no
embedded timezone download. The runtime requires Tumbleweed's `timezone` package.
Host RPM ownership of `/usr/share/zoneinfo/UTC` is exactly
`timezone-2026c-1.1.x86_64`; the collector receipt includes the public data-file
SHA-256. This is host provider evidence, not clean-target solving or VM acceptance.
The new dependency source/vendor closure must be refreshed with packaging.

The native GTK/WebKit app handoff remains separately immutable:
[#173's provider records](https://github.com/GenieRobot/ryoku-opensuse/blob/d2853fb5901e345b061a49cf4eb764d73b1f0891/upstream/evidence/ryoku-v1.0/rashin-native-provider-records.json)
bind `gtk+-3.0` to `gtk3-devel` and `webkit2gtk-4.1` to `webkitgtk3-devel`, with
signed Snapshot20261016 provenance, package checksums, locations and dependencies.
Metadata presence does not prove the app build, native ELF closure or installed
behavior. No such completion is claimed here.

Private deterministic fixtures exercise source formats, cache/cost semantics,
enroll/disable, persisted cadence and failure, real daemon restart catch-up,
isolated-process time zones, malformed/oversized files, unsafe/replaced roots,
stale/partial/unavailable handling and absence of transcript/key leakage. The
largest fixture is a sparse file just over 4 MiB; no remote source is fetched.
