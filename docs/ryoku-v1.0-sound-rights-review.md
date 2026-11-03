# Selected sound provenance and rights limits

This #173 notices review preserves every selected sound asset and the full
Ryoku sound/typing/notification choices. It records exact source evidence and
remaining redistribution questions, not a legal determination or a grant
inferred from a package header. No audio is played, decoded or replaced.

[`sound-asset-disposition.tsv`](../upstream/evidence/ryoku-v1.0/sound-asset-disposition.tsv)
and [`sound-asset-provenance.json`](../upstream/evidence/ryoku-v1.0/sound-asset-provenance.json)
pin all 73 source audio paths at
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`: Git blobs/modes, byte counts,
SHA-256s, selections, declarations and metadata limits. The earlier 63-path
count covered direct UI files; ten additional `.oga` resources are embedded
by the selected IPC `sounds.go`. Source2 maps the 63 UI files; Source3 retains
all 73. Neither archive, spec nor runtime behavior changes in this unit.

| Group | Exact coverage / source declaration |
|---|---|
| Typing | 32 WAV hashes match the selected sounds-manifest. It declares Monkeytype `91bd24bb8513785c7364cbea29296ff7adafac41`, `frontend/static/sounds`, GPL-3.0-only; root NOTICE preserves attribution. This is recorded source licensing/byte provenance, without an external refetch or a new grant inferred from artist tags. |
| Python bar style | 31 files: 27 WAV and four MPEG audio files. Component NOTICE attributes Serpantinum `cfdd810` after 2.2.12 and declares AGPL-3.0-or-later for ported portions; LICENSE is retained. No separate per-sample origin/license mapping was found. Independent third-party media terms remain unresolved rather than inferred from a component label or artist name. |
| IPC desktop | Ten Ogg/Vorbis `.oga` files, each byte-identical to its original member in the cached official freedesktop 0.8 archive. Member identities, per-sound CREDITS regions and the package-copy digest are recorded. |

The cached archive is 478,237 bytes, SHA-256
`cb518b20eef05ec2e82dda1fa89a292c1760dc023aba91b8aa69bafac85e8a14`, matching
the recorded official source identity. Its CREDITS is byte-for-byte identical
to the packaged `sound-theme-freedesktop-CREDITS`. The archive's **COPYING is
empty (zero bytes)**; README directs licenses/origins to CREDITS. An empty
member supplies no grant or useful license text, and none is added to the RPM.

| IPC name | Byte-verified original / verbatim CREDITS license |
|---|---|
| battery-full | complete / CC-BY Attribution 3.0 Unported |
| battery-low, notification-critical | dialog-warning / CC-BY-SA |
| device-added, power-plug | device-added / CC-BY-SA |
| device-removed, power-unplug | device-removed / CC-BY-SA |
| notification | message-new-instant / GPLv2+ |
| shutter | camera-shutter / CC-BY-SA (relicensed with permission from author) |
| volume-change | audio-volume-change / CC-BY-SA (relicensed with permission from author) |

CREDITS supplies author/source references but no CC-BY-SA version for these
entries. Inspected Vorbis comments supply no rights field resolving it.
`LicenseRef-sound-theme-freedesktop-0.8-CC-BY-SA` preserves that limitation; it
is not silently promoted to an invented SPDX version or accepted redistribution.

Only container metadata was inspected: RIFF LIST/INFO rights fields, Vorbis
comment rights fields, and MPEG headers with absence of leading ID3v2/trailing
ID3v1. This is bounded metadata inspection, not every possible format extension
or an audio decoder. Artist labels include Anton Porsche/W0LFB0MB on typing
samples and Brukowskij on four bar-style WAVs; attribution is not a separate
grant. Source declarations and independently established per-asset rights are
recorded as distinct facts.

Native notice delivery and accepted redistribution terms remain open alongside
the eleven font blockers. Any separately reviewed licensed-equivalent change
must preserve user-visible sound choices through the owning #25/#180 work.
This source review makes no replacement, omission, network request, VM/package
operation or functionality/acceptance claim.

Deterministic validation checks all 73 identities/modes/digests and source
selections, 32 manifest hashes, ten cached member/CREDITS regions, six exact
notice/runtime sources and published metadata boundaries. Full Vorbis comment
key lists contain no hidden rights/version key; RIFF rights lists were
independently re-read. Input hashes and limitations are in
[`sound-rights-validation.json`](../upstream/evidence/ryoku-v1.0/sound-rights-validation.json).
