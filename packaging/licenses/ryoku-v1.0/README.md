# Selected v1.0 third-party notices

The source revision is `a59d051f0e8080052216f6c7b9eb7e5928935ce8`.
The runtime preserves the original root NOTICE and nested licenses/notices;
the RPM also installs the notices in this directory with `%license`.

- `inir-NOTICE` is the exact retained `ryoku/shell/quickshell/inir/NOTICE`.
  It specifies GPL version 3 with section 7 attribution and identity terms.
  Based on iNiR by snowarch (https://github.com/snowarch/inir).
  These are modified components of Ryoku and this openSUSE port. The required
  attribution is retained in this project's README and the patched upstream
  Hub credits and welcome metadata. The original deep LICENSE/NOTICE files
  remain beside the code.
- `redux-LICENSE` and `redux-UPSTREAM.md` are the exact MIT notice and origin
  record for embedded `ryoku/hub/backend/assets/redux/`. The Python adapter
  and setup source remain immutable upstream embedded assets; this project
  does not implement or enable the model download/setup provider. The Rust
  Hub command policy refuses `voxtype ensure/download/set/rmmodel` and the
  direct UI package/setup/service launchers before upstream dispatch.
- The selected Python bar-style code (the directory name is upstream's)
  retains its AGPL-3.0 license text and attribution. Upstream root NOTICE
  identifies Serpantinum revision `cfdd810` as AGPL-3.0-or-later. It is not
  relicensed as the root project's GPL-only code.
- Stage and iNiR each retain
  `modules/common/widgets/shapes/LICENSE`, exact Git blob
  `261eeb9e9f8b2b4b0d119366dda99c6fd7d35c64`. Both files contain the
  Apache License version 2.0; the runtime RPM's license expression now
  includes `Apache-2.0`. The files remain beside their geometry/JS assets.
- `ryogami-picker-LICENSE` and `ryogami-picker-NOTICE` are exact pinned
  picker copies. NOTICE explicitly declares GPL-3.0-or-later for its skwd-wall
  derivation; the runtime license expression records that component alongside
  the root GPL-3.0-only identity. The distinct filenames preserve both picker
  and root LICENSE/NOTICE rather than relying on duplicate basenames in RPM's
  license-copy staging.
- `ryogami-picker-RobotoCondensed-LICENSE-Apache-2.0.txt` and
  `ryogami-picker-SymbolsNerdFont-LICENSE-MIT.txt` are exact texts beside the
  two TTF resources embedded by the picker's CMake module. They are installed
  through this directory's existing `%license` declaration. Original blobs,
  SHA-256s, byte counts and selected font identities are recorded in
  `upstream/evidence/ryoku-v1.0/picker-license-provenance.json`. The RPM audit
  requires all four uniquely named texts as regular files in the runtime
  license directory and the picker license identity in its generated header.
  Source/recipe validation is separate from the pending native RPM payload
  comparison and complete asset-license acceptance.

- `inir-inter-OFL-1.1.txt`, `inir-montserrat-OFL-1.1.txt` and
  `inir-rubik-OFL-1.1.txt` are exact pinned texts beside all 22 selected iNiR
  TTFs. The runtime package owns those fonts, so its License expression includes
  `OFL-1.1` independently of the separate external-font subpackage. The
  provenance ledger records each selected font/blob and copy hash; the RPM
  audit requires all three texts as regular files in its license directory.
  This does not resolve the eleven other desktop fonts with incomplete rights
  evidence; see `docs/ryoku-v1.0-font-rights-review.md`. Typography and font
  choices remain retained targets, with redistribution/native acceptance open.

`sound-theme-freedesktop-CREDITS` is copied verbatim from the official
`sound-theme-freedesktop-0.8.tar.bz2` source archive at
<https://people.freedesktop.org/~mccann/dist/sound-theme-freedesktop-0.8.tar.bz2>
(SHA-256 `cb518b20eef05ec2e82dda1fa89a292c1760dc023aba91b8aa69bafac85e8a14`).
Every one of the ten embedded `ryoku/shell/ipc/assets/*.oga` files is byte
identical to its counterpart in the signed Tumbleweed
`sound-theme-freedesktop-0.8-9.20.noarch` RPM from the pinned 20261016 OSS
metadata. Renamed assets have these original identities:

| Embedded name | Original sound | CREDITS license |
|---|---|---|
| battery-full | complete | CC-BY Attribution 3.0 Unported |
| battery-low, notification-critical | dialog-warning | CC-BY-SA |
| device-added, power-plug | device-added | CC-BY-SA |
| device-removed, power-unplug | device-removed | CC-BY-SA |
| notification | message-new-instant | GPLv2+ |
| shutter | camera-shutter | CC-BY-SA, relicensed with author permission |
| volume-change | audio-volume-change | CC-BY-SA, relicensed with author permission |

The exact authors and original source URLs are in CREDITS. Its CC-BY-SA
entries do not state a version. The candidate RPM records that limitation as
`LicenseRef-sound-theme-freedesktop-0.8-CC-BY-SA`; it does not invent a version
or infer a single GPL license from the RPM header. This is a provenance record;
redistribution/license-metadata closure remains a #173 acceptance item until
the native package review accepts that recorded license identity.
