# Issue #13 bounded KDE dependency investigation

Observed 2026-10-12 in an authorized existing cloud QEMU TCG guest. This is
dependency/blocker evidence, not desktop acceptance. The investigation used
PR54 head `18648cae24ddf501e5f5b17882dc9102ae1f820c`; it performed no source push
or Actions dispatch/re-run. The subsequent follow-up changes and Run13 have
separate revision and acceptance evidence.

The original GPG-enabled native repositories refreshed successfully. All closure
checks used `sudo zypper --no-refresh --non-interactive install --dry-run
--no-recommends` and preserved hard requirements and original providers.

| Requested package set | Native exit | Solver result |
| --- | ---: | --- |
| `plasma6-session sddm` | 0 | 743 new packages, including OpenH264 |
| `plasma6-session-x11 sddm` | 0 | 753 new packages, including OpenH264 |
| `patterns-kde-kde_plasma sddm` | 0 | 762 new packages, including OpenH264 |
| `plasma6-session sddm ffmpeg-8-mini-libs` | 4 | Unsatisfied build-only requirement |
| `patterns-kde-kde_plasma sddm ffmpeg-8-mini-libs` | 4 | Same native rejection |

Signed native metadata yields the hard `Requires`/exact SONAME `Provides` chain:

```text
plasma6-session 6.7.5-1.1
 -> plasma6-workspace 6.7.5-1.1
 -> libKF6Baloo.so.6()(64bit) [libKF6Baloo6 6.31.0-1.1]
 -> libKF6FileMetaData.so.3()(64bit) [libKF6FileMetaData3 6.31.0-1.1]
 -> kf6-kfilemetadata >= 6.31.0
 -> libavcodec.so.62()(64bit) [libavcodec62 8.1.3-2.1]
 -> libopenh264.so.8()(64bit) [libopenh264-8 2.6.0-2.suse1699.10]
```

The X11 session hard-requires `plasma6-session = 6.7.5`; the official KDE pattern
also hard-requires `plasma6-session`. Mozilla's exact OpenH264 provider hard-requires
`libopenh264-8(x86-64) = 2.6.0-2.suse1699.10`, retaining the denied payload.

`ffmpeg-8-mini-libs 8.1.3-2.1` provides the FFmpeg SONAMEs, but its official
description is a "cut-down version for building other packages" and warns about
missing expected features. It requires `this-is-only-for-build-envs`, conflicts
with normal FFmpeg libraries, and has no Obsoletes. Both complete KDE requests
fail natively with:

```text
nothing provides 'this-is-only-for-build-envs' needed by the to be installed
ffmpeg-8-mini-libs-8.1.3-2.1.x86_64
```

The solver offers omission or dependency breaking; neither was accepted.

Native cached package metadata identifies the original codecs.opensuse.org RPM
URL. An original HTTP-origin probe returned 302 to the original Cisco host;
following that redirect through the existing enforced proxy returned 403. Run11
separately recorded the HTTPS package install failure for the same package:

`https://codecs.opensuse.org/openh264/openSUSE_Tumbleweed/x86_64/libopenh264-8-2.6.0-2.suse1699.10.x86_64.rpm`

Original payload host: `ciscobinary.openh264.org`. No alternate-origin download,
proxy bypass, signature weakening, dependency exclusion or policy relaxation was
used. No supported avoiding KDE/SDDM fixture was established in the enabled signed
repositories. The user has approved this exact public origin. The current
enforced runtime allowlist still omits it and exposes no configuration mutation
control; the approval must be applied through supported configuration to a new
chat/environment. That is the minimum necessary next resource action; full native installation and actual graphical
fallback/lifecycle validation are still required afterward. Authorization alone
does not establish acceptance or rule out later failures.

The investigation guest was shut down and reaped with QEMU exit 0; original base
checksum remains unchanged. Private mode-0700 logs remain under
`/workspace/ryoku-lab-issue13-kde-investigation`; no keys/disks/raw logs are public.

A narrow future-success reporting issue remains: the evidence alphabet omits `+`,
so `sddm 0.21.0+git57-1.1` would be dropped from a successful version inventory.
Committed Run11 never emitted this field because KDE provisioning failed, so its
blocked outcome and recorded assertions are unaffected. No sanitizer patch made.
