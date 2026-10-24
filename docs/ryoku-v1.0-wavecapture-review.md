# WaveCapture runtime command providers

The native `WaveCapture` at upstream
`a59d051f0e8080052216f6c7b9eb7e5928935ce8` starts external commands through
QProcess. ELF dependency scanning cannot discover them. The selected
`ryoku/shell/plugin/wavecapture.cpp` is blob
`8dc1c62ec266f5262f1636e8383e2ead9d882aec`, exported into Source2 as
`native/ryoku-blobs/wavecapture.cpp`.

- Lines 97/120 start `pactl` to read the default sink and short source list.
- Line 161 starts `pw-record` with the resolved monitor target, rate, mono
  channel, signed 16-bit/raw format and `-` stdout. The audio stream is read
  by the module; this path does not request a persistent recording file.

[`wavecapture-providers.json`](../upstream/evidence/ryoku-v1.0/wavecapture-providers.json)
pins source identity/anchors, repository provenance and three exact RPM
package records. The cached Tumbleweed OSS 20261016 `repomd.xml` signature was
reverified locally with openSUSE signer
`AD485664E901B867051AB15F35A2F86E29B700A4`. Its SHA-256 remains
`1d891e33c8abd582b71e579118e6b4c94e5e57ecdc1b77a2a7fc1abf7cc930a0`.
Both compressed and open primary SHA-512/size values match that signed index;
all 52,976 package records were parsed without downloading metadata or RPMs.

Unlike the earlier JSON index, the original primary XML includes relevant
executable `<file>` entries. These are signed repository package-file facts:

| Command | Exact x86_64 package | Primary file entry |
|---|---|---|
| `pactl` | `pulseaudio-utils` `0:17.0-7.6` | `/usr/bin/pactl` |
| `pw-record` | `pipewire-tools` `0:1.6.9-2.1` | `/usr/bin/pw-record` |

The evidence retains literal original-byte package fragments, their byte
counts/SHA-256s, RPM SHA-512/location, versions, file entries and complete
primary provides/requires/conflicts/obsoletes. The fragments inherit the
original root's namespaces and are not standalone XML documents. Their source
package records contain 11 and 24 file entries respectively; this is not a
claim that primary XML is a complete RPM file list. The old
`wavecapture-provider-candidates.json` remains the earlier incomplete JSON-index
checkpoint, superseded for this file-ownership question by the verified raw
primary evidence.

`pulseaudio-utils` requires the virtual `pulseaudio-daemon` capability.
The retained `pipewire-pulseaudio` `0:1.6.9-2.1` record provides that capability
and itself requires `pulseaudio-utils`. Its raw conflicts are also preserved:
`pipewire-modules < 1.0.0`, `pulseaudio`, and `pulseaudio-daemon`; it has no
primary obsoletes entries. The client requirement therefore does not name or
require selecting the PulseAudio server package. This provider/capability
fact is **not a successful installed-system Zypper solve**; self-provides and
conflicts must be handled by the native solver, and current audio services
must be preserved by the owning session/package contracts.

The runtime spec now explicitly requires `pulseaudio-utils` and
`pipewire-tools`; it adds no service activation, provider replacement,
installer logic or guessed virtual Provide. The package audit rejects a
built runtime missing either requirement. Focused tests cover missing or
similarly named packages, exact executable/provider/source-manifest contracts,
literal primary fragment hashes and parsed native spec output. All ten focused
audit/spec tests passed with compatible cached dependencies; this is separate
from hosted exact-head checks. Exact input hashes and validation limits are in
[`wavecapture-validation.json`](../upstream/evidence/ryoku-v1.0/wavecapture-validation.json).

Native RPM payload/executable-mode and executable startup checks, installed
system Zypper transaction compatibility, actual monitor capture/retry behavior,
and #25/#26 audio/session/recovery acceptance remain open. No package was
downloaded or installed and no service, VM or live audio state was accessed
for this source/recipe fix. The eleven unrelated shell-font redistribution
blockers and all other #173 acceptance limits remain unchanged.
