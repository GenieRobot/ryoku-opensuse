# Fresh Ryoku 1.0 desktop and first-sprint acceptance

This is #173's acceptance-note contract for #25, #26 and the #1/M2 roadmap.
It completes the documentation-gating task only. It reports no new native
package, desktop, service, device, recovery or release acceptance. The full
Ryoku 1.0 target remains required; open feature owners are implementation and
acceptance work, not permission to remove capabilities.

## Bind evidence to the combined candidate

Build and test one recorded combined port candidate containing the reviewed
#173 inputs and applicable #25/runtime integration. Record the actual port
commit and integrated feature commits. This is a joint candidate gate, not a
requirement to manufacture separate, inconsistent passing runs or to treat
issue state as evidence. Source/hosted CI can pass while #173/#25/#26 remain
open for the same candidate's native results.

| Identity dimension | Required record and current reviewed source facts |
|---|---|
| Upstream major source | Ryoku Amaterasu **1.0.0**, commit `a59d051f0e8080052216f6c7b9eb7e5928935ce8`, tree `06b2962f82f43118c3a98372c2a4b435b689b725`. A package called 0.1.0 does not identify this source or establish API compatibility. |
| Port source | Exact clean port commit, integrated issue commits, Source0 and locked Source1/Cargo.lock hashes and source generation commands. Produce/verify these for the combined candidate; an older input packet or copied executable does not bind a later port commit. |
| Upstream Source2 | `ryoku-runtime-a59d051.tar.gz`, SHA-256 `8aeff20645c14af76f3d341e5d4e7c31923fd249f6671caa704c8b23a6c84e65`, 54,073,125 bytes, 4,472 files. Bind its export manifest and applied patch. |
| Upstream Source3 | `ryoku-writers-a59d051.tar.xz`, SHA-256 `387d829860ba844ad32ecdd5982678707665f3251e41bbb43c6fd6f87dc7eee0`, 34,625,504 bytes, 6,968 source entries excluding directories, including two literal internal links. Raw CLI/Rashin retention is not functional acceptance. |
| Package definitions and artifacts | All six exact spec hashes, source/patch/provider/ownership inputs, parsed metadata, actual RPM/SRPM names/EVRs/architectures, SHA-256s and signatures. Current base specs declare port 0.1.0-8 and Quickshell 0.3.2-3; measure the combined candidate rather than reuse these labels as binary proof. Upstream 1.0.0 and the port RPM version remain independent. |
| Build environment | Exact native Tumbleweed snapshot, toolchain, Qt/Quickshell/compositor versions, enabled repositories and authenticated metadata. BuildRequires stays enabled; offline/vendor/source settings and all dependency resolution must be evidenced. A --nodeps mechanics build is not native build acceptance. |
| Fresh target | Exact signed image and firmware hashes, kernel, filesystem/subvolumes/encryption, SELinux, baseline package/configuration inventory and repository/signature policy. A reused SDK/build guest is not the clean acceptance target. Recorded owned fixture changes must be distinguished from baseline image state. |

The authoritative source/export fields are in
[`ryoku.source.toml`](../upstream/ryoku.source.toml),
[`ryoku-runtime.toml`](../upstream/ryoku-runtime.toml),
[`export.json`](../upstream/evidence/ryoku-v1.0/export.json) and the
[two-run reproduction proof](../upstream/evidence/ryoku-v1.0/source-retention-reproduction.json).
These are source inputs, not a fresh binary/VM receipt. If any candidate input
changes after testing, preserve the old result under its original tuple and
rerun the affected gates before applying the result to the new candidate.

## Required fresh evidence and its owner

| Gate | Evidence required for this candidate |
|---|---|
| #173 package/native closure | Build complete current RPMs/SRPMs; run package ownership/dependency/scriptlet/license audits and normal native dependency installation. Retain generated QML Provides/Requires, actual plugin/shader/qmldir files, ELF dependencies and loaded imports. Source scans and cached vendor ELF headers cannot replace the built port modules. |
| #173/#25 runtime dependencies | Verify cava/upower and audio command providers, the declarative payload/capability-helper split, static Quickshell modules, generated Blobs/Spatial3D and Concurrent/Quick3D/Multimedia. Resolve Qt's FFmpeg8 backend separately from the candidate FFmpeg9 CLI/livewall build and run the image/video/tool scenarios. A package name/Provide alone does not prove endpoint, codec or rendering behavior. |
| #25 ordinary session | Use the native display-manager path with ordinary password authentication, correct user/service environment and installed packages; verify startup, logout, failure and reboot without an interactive development shell. Keep KDE selectable and demonstrate fallback after a controlled Ryoku failure. Do not substitute autologin or a PAM bypass for the required login evidence. |
| #25 workflows and owned providers | Record actual launcher/Hub/appearance, notifications, clipboard, audio, network/Bluetooth, screenshot/screen-share/portal, authentication and locking results with the exact provider/owner and test method. Distinguish native API behavior, simulation and physical-device proof. An untested feature stays an open target; hiding it or marking its owner complete is not acceptance. |
| Protected appearance and automatic effects | Before claiming wallpaper/apply/restore, prove the typed authorized route and complete linked pre/post capture, including `applyRyogamiFrame` → `scheduleTheme()` → `paintWorker` and startup/watcher/post-apply writes under #25/#176. Library/download/model/delete providers stay #175; other protected user/host controls stay #180/#181. Authentication, an app-launch checkpoint or a successful socket response alone is insufficient. |
| #26 fresh/repeat installation | Exercise the native #24 installation path on the recorded clean target and repeat it with controlled Ryoku overrides and unrelated home sentinels. Retain exact package/configuration inventories and preservation comparisons. Historical installer success does not prove the refreshed source/package set. |
| #26 linked offline recovery | Customize synthetic supported state, create the matching linked checkpoint, cause controlled damage and restore through the console with source/companion repositories unavailable. Record matching system/Ryoku/package identities, terminal operation/checkpoint IDs, restored managed state and unchanged unrelated sentinels. Keep interruption/failure outcomes and realistic filesystem/recovery limits explicit. |
| Full retained target and notices | Keep core Rashin and every feature owner's required capabilities in the full target. Source presence, a screenshot or a reduced provider does not demonstrate their functionality. Unresolved font/sound/model notices/rights and unverified writes remain blockers to the corresponding acceptance/release claim; this contract grants no redistribution permission. |

The detailed [source delta/owner map](ryoku-v1.0-delta.md),
[runtime provider contracts](ryoku-v1.0-runtime-providers.md) and
[Qt/native proof](ryoku-v1.0-qt-native-providers.md) identify input facts and
open boundaries. They do not waive native tests. Fresh results must label
passed, failed, interrupted and unverified scopes separately. Preserve failed
attempts and sanitize published evidence; do not publish credentials, raw
private logs or machine/companion data.

## Historical receipts remain historical

| Existing receipt | Exact original boundary; what it cannot satisfy |
|---|---|
| [#23 native package receipt](../infra/tumbleweed-lab/evidence/issue23-native-acceptance-20261021.toml) | Port 0.1.0-5, Quickshell 0.3.2-2; recorded runtime SHA-256 `81151cef1a613b01ad8e6187df2e93cbfed70e1621de0ee2e7455d2904fc3e74`, writer SHA-256 `bf6790f07f643dc7b3f282b0ce3b918b7f0efdb5c343a7572c5dd5ab9477feed`, audited tree `49cf710dea4a8bc2c5aa2c4b524be46c5c9a2cff`. It proves only its recorded artifact/package checks, not current 1.0 package, desktop or recovery acceptance. |
| [#24 native Run10](../infra/tumbleweed-lab/evidence/issue24-native-20261022/run10.txt) | Its explicit version scope is upstream `98a0368253d9628be72b853985878a5db90e1547` / 0.75.5-beta.20, port 0.1.0-7 and Quickshell 0.3.2-2. Preserve that label and result; no relabeling as a59d051/1.0 or current package acceptance. |
| Hosted Rust checks, source audits and cached RPM/ELF evidence | They validate their exact source/check scope. They do not establish a fresh display-manager login, loaded port plugin, device operation, protected mutation or linked console restore. |

Do not close #25 or #26, mark M2/#4 complete, or use #23/#24 as the refreshed
#173 acceptance result without the required current-candidate evidence above.
The coordinator may link one consistent combined native run across the issues;
each issue still needs its own scoped evidence. #173's acceptance-note/roadmap
checkbox may be completed when these notes and links are updated; the native,
semantic, provider and rights checkboxes remain independent and open.
