# Ryoku 1.0 installer, release and documentation source review

This bounded #173 source unit compares upstream 1.0.0
`a59d051f0e8080052216f6c7b9eb7e5928935ce8` with 0.75.5
`98a0368253d9628be72b853985878a5db90e1547`. The full installation, application,
assistant, update and recovery experience remains the target. Edition-specific
package, disk, trust and publisher mechanisms require their owning native
contracts, not silent feature removal. No upstream program, installer,
workflow, publisher, dry-run or network operation was executed here. No source,
export manifest, RPM recipe or runtime code changes in this unit.

## Exact bounded inventory

[The path ledger](../upstream/evidence/ryoku-v1.0/installer-release-disposition.tsv)
records 295 current/prior path identities: 282 current and 213 prior, with
104 modified, 82 added, 96 unchanged and 13 old paths absent. Its scope is
explicit in [the API evidence](../upstream/evidence/ryoku-v1.0/installer-release-boundary.json):
complete `installation/`, `ryoku-shell-installer/` and `release/` roots plus
15 publication/install workflows, 13 named release/maintainer tools and eight
installation/doc/root-license/version inputs. These bounded references do not
cover every upstream workflow, document or transitive helper.

| Exact source area | Current / union paths | Current selection and owner |
|---|---:|---|
| installation | 109 / 109 | 27 backend, six shared plan, 41 ISO, ten tests, 23 TUI and two root documents. Native RPM installation stays #24 with #27–29 package/plan/progress contracts, #25 session integration; ISO/image implementation remains the separate #49 scope. |
| standalone installer | 36 / 36 | Includes sources/tests, bootstrap, checked-in executable and checksum sidecar. Preserve complete feature intent via #24/#27–29/#25, not this edition's non-Zypper actuator. |
| release | 101 / 114 | Packages/payloads, offline Gradle inputs, repository build/import, retention and docs. #173 source/recipe/notice review; #42 signed staging/provenance, #43 regression matrix, #44 tested guides/promotion, #45 maintenance cycle; #182/#183 and other existing feature owners remain required. |
| selected workflows/tools/docs | 36 / 36 | Source/reference declarations only. #42–45 own authority, staging/gates/promotion/retention; #24/#27–29/#44 own install/upgrade/remove/recovery instructions. |

Only root LICENSE/NOTICE in this ledger are selected by Source2/Source3;
none of these installer or publisher mechanisms is newly imported. Selection
is an export fact, not a permanent exclusion or accepted native behavior.
The binary/JAR/asset license/build-provenance and broader archive closure
questions remain open.

The earlier 62 installer changes concern the named implementation subtrees;
adding two root documents gives 64 current `installation/` changes. All 29
standalone-installer changes are included. The global release delta has 78
rows touching `release/`: 77 addition/modification/deletion rows plus an R100
logo move **out** to `nix/assets/ryomotion-logo.png`. The ledger's 13 absent
old paths comprise that move, ten old Gradle 1.1.3 inputs and two package
recipes. The logo's old/new mode/blob identity is separately proved. These
are source moves/replacements, not approval to remove user-facing apps or
licensing obligations. The two package recipes are prowl-agent and ryomotion;
full gateway/app behavior remains under #183/#47/#182.

## Shared install choices, authentication and persistent effects

The exact shared tables preserve 21 choice/policy rows (13 choices, eight
policies) and 32 component rows, including Rashin, app/library/model/dictation,
container/VM, gaming and developer-tool selections. `plan.go` embeds
choices.tsv; `components.go` embeds components.tsv. Their parsing, edition
availability and package/drop/hidden-desktop calculations are UI/package-plan
semantics, not an authenticated transaction or a Tumbleweed package mapping.
Keep browser, login-shell, file-manager, all compositor and component choices;
#24/#27/#29/#182 must map them to verified native providers. Core Rashin and
every #183 capability remain required despite an upstream selectable component.

| Exact route | Source behavior and required boundary |
|---|---|
| install.sh → same-ref executable + SHA sidecar → chmod/execute | Default ref is `main`, configurable through RYOKU_SHELL_REF. Downloads both files from that ref, verifies checksum then runs the binary as the invoking user (root bootstrap is refused). The cached 42,173,394-byte ELF blob matches its sidecar SHA-256 `2accb9948c01088a65008620cd9c8e9dd8bee5f655b70e7387271ae1592a6e35`. This proves these pinned bytes agree, **not** independent signing authority, source-to-binary build provenance or safe mutable-ref execution. Preserve convenient installation through #24/#27/#28 and #42's verified source/signature path; do not transplant curl-to-executable bootstrapping. |
| standalone main/edition/distro → engine steps | Supports Arch/Debian/Void/Fedora mechanisms, with edition/component checks and Fedora publication gating. There is no established Tumbleweed/Zypper actuator in this reviewed source. Preserve intent through #24/#27–29, not fallback package-manager detection or an assertion that RPM implies Fedora. |
| engine.cmd / sudo / sudoSh / sudoWrite | Runs subprocesses, optionally `sudo -n`, shell snippets and root writes. Its pacman guard compatibility can set OMARCHY_ALLOW_DIRECT_PACMAN=1. Noninteractive privilege acquisition, dry-run and an upstream bypass for retiring a foreign hook do not satisfy the port's authorization/linked-capture contract. #20/#24/#27/#28 must preserve bounds and reject recovery/authorization bypasses. |
| stepPayload / stepBackup / stepConfigs / preferences | Fetches/checks out the chosen Git ref or accepts a local checkout, manages a payload cache, backs up/copies user config, writes defaults/roles/markers and provisions a ledger. This has user-account data access and persistent effects as well as package changes. #24/#25/#18/#20/#21 require complete managed-path inventory and linked pre/post recovery, including overwrite/interruption cases. No broad home restore is authorized by a source backup function. |
| stepRepo / stepVoidRepo / stepFedoraRepo / stepPackages / stepAUR | Installs trust/repository configuration, refreshes/installs/removes package sets and can build third-party AUR content. Fedora repo config declares gpgcheck/repo_gpgcheck; this is not proof of retrieved key authority or signed Tumbleweed artifacts. #27 plans/trust and #28 protected Zypper/RPM execution must bind exact inputs and rollback/accounting; #182 owns retained third-party content/build capability and #29 presentation. |
| stepSession / stepLoginShell / doctor / verify | Changes session/DM/service/group/shell/default integration and runs reconciliation/probes. Preserve workflow and KDE fallback via #24/#25/#177/#31/#180/#181 with #20 protected operations. A successful probe or process is not complete native login/lock/reboot acceptance. |
| lifecycle resume/uninstall | Tracks completed steps, removes packages and optional repository/key state, and restores the backup chain. #24/#27–29/#21 require exact native package/config ownership and linked recovery; a completed-step marker does not prove that live state still matches or that retry is safe. |
| disk backend → disk/resize/LUKS/filesystem/accounts/pacstrap/boot helpers | Parses environment/TUI answers and can wipe/partition, encrypt, format, mount/chroot, create users, install base packages and bootloader/snapshot layout. Whole-disk and alongside paths include source checks but remain destructive. Preserve the native install/recovery experience through existing delivery contracts; #49 owns separate future ISO/media design. #173 neither invokes those helpers nor broadens #24 into ISO implementation. |

All six ISO Git links are retained literally in the evidence, including
absolute system-unit/timezone links and a /dev/null service mask. They were
read as Git blobs without dereferencing host paths. They are edition image
semantics, not the two accepted confined relative Source3 links. Preserve
feature/policy intent through #49's native image design rather than weakening
archive confinement or importing Arch autologin/PAM/boot configuration.

## Release identity, gates and retention

| Exact source chain | Authority/write surface and owned port requirement |
|---|---|
| stable-release workflow → release-bump/version → Git VERSION/tag/dispatch | Declares contents/actions write and main-only workflow logic. Upstream semantic 1.0.0 is provenance, not this port's forced RPM Version. #42/#44/#45 must promote only the exact independently tested port artifact; this review makes no workflow dispatch or tag. |
| repo build → makepkg/repo-add/gpg + shared payload/import | Builds/signs pacman artifacts, mirrors package data and creates release/control manifests through the CLI. Publisher workflows add Fedora/XBPS paths and upload/channel operations. They are reference evidence, not OBS/RPM definitions, authorized port signing keys or trusted repository endpoints. #42 owns signed pinned staging and #27/#28 verified consumption. |
| testing gate / checked-pointer / publisher workflows | Carry testing checks and last-passed pointer, gate upload/promotion on declared smoke/install outcomes and distinguish frozen release from mutable channel artifacts. Source declarations do not prove the workflow ran, its checks suffice or its artifacts match this candidate. #43 must establish the exact fresh native/recovery matrix; #44 promotes only that identity, #45 maintains drift gates. |
| release-ledger / ISO manifest | Enumerate remote release/manifest metadata and write index/pointer/JSON/JS/checksum outputs. ISO manifest hashes both ISO and signature and requires a signature file; existence/hash equality is not signature verification under a trusted key. #42/#44 need explicit signature/identity binding and tested guides; #49 owns future ISO identity requirements. |
| retention planner → runRetention → BUCKET.delete / ledger edits | Uses gathered release/pointer/manifest facts to compute object deletion and ledger rewrites, with dry-run and operation-budget branches. This can delete rollback inputs; source dry-run/defaults are not authorization or retention proof. #42/#44/#45 must preserve previously tested recovery builds and controlled bounded deletion, not inherit a provider worker or cloud credentials. |
| update/track and installation docs | Upstream branch/repository/package instructions reflect its edition mechanisms. Preserve updates, rollback, recovery and user choices through #24/#27–29/#44 and existing recovery owners. Documentation/source tests are not native operational evidence; do not direct Tumbleweed users at foreign mutable curl, pacman, DNF or XBPS commands. |

The pinned root LICENSE/NOTICE, package license declarations, offline Gradle
artifacts and test/source identities remain review inputs. Their presence
alone does not settle transitive binary/asset rights or installed notices.
This unit closes no entire #173 checklist row: complete per-operation and
transitive semantic review, source/archive/license/dependency closure, protected
provider implementation, native RPM/plugin/runtime and fresh session/recovery
acceptance remain open. #42–45/#49 ownership does not imply their tasks have
shipped or authorize publisher/installer execution under #173.
