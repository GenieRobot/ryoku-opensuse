# Provider boundary experiments, 20 October 2026

Issue: [#15](https://github.com/GenieRobot/ryoku-opensuse/issues/15).
Status: **negative feasibility evidence; issue remains open**.
The [machine-readable record](../infra/tumbleweed-lab/evidence/issue15-provider-boundaries-20261020.toml)
contains exact versions, source/binary hashes, measured state and private-log
digests. The [capability matrix](provider-capabilities.md) remains the operation
allowlist. No operation became verified protected.

## Fixture and trust

This was a new QEMU TCG guest with two vCPUs and 3072 MiB RAM, an independent
qcow2 overlay, fresh UEFI variables, fresh SSH keys, and fresh cloud-init state.
Only the verified official Snapshot20261016 cloud image was used as a read-only
backing file. Its SHA-256 was checked against `image.sha256`, whose detached
signature validated under the pinned openSUSE Project key
`AD485664E901B867051AB15F35A2F86E29B700A4`. The image hash is
`ec8fd95a4c8a1315b3f2cdf609eb226e2578ac3faae18c1fc4242a911041a231`.
The shared WSL fixture and all other issue guests were untouched.

The guest booted kernel `7.2.10-1-default` with its original **XFS** root.
This is provider characterization, not the Btrfs/Snapper recovery fixture.
There was no linked checkpoint, Ryoku journal integration, restore proof,
graphical session, or first-sprint lifecycle claim.

All default repositories were disabled in this disposable overlay. The sole
enabled source was the official archived
`https://downloadcontent2.opensuse.org/history/20261016/tumbleweed/repo/oss/`,
with metadata and package signature checks enabled. Its cached `repomd.xml`
matched the separately checked hash
`1d891e33c8abd582b71e579118e6b4c94e5e57ecdc1b77a2a7fc1abf7cc930a0`.
No unsigned source, additional trust key, or signature bypass was used.
The absent `hello` and `tree` packages were downloaded before the experiments
so that the probe's 45-second transaction deadline measured cached transactions.

The existing Rust probe's root/QEMU/disposable-fixture guard remained active.
The cloud-init image carried the existing `disposable-issue13` guard value
solely for compatibility with that probe, plus a distinct issue15 marker.
That marker does not attribute this run to issue #13.

## Measured operations

| Operation | Actual observation | Classification |
|---|---|---|
| `zypp-hook-probe hello observe-commit` | Installed `hello-2.12.3-1.4.x86_64`; zypper exited 0 in 6920 ms. The plugin recorded `PLUGINBEGIN`, `COMMITBEGIN`, `COMMITEND`, `PLUGINEND`. Installed RPM count changed from 372 to 373. | A real notification boundary was reached. ACK supplies telemetry, not authorization or capture. |
| `zypper addrepo --disable --no-refresh dir:///var/empty ryoku-issue15-probe` | Created the exact new repo file while the denying plugin was installed. | Not intercepted by the commit plugin. |
| `zypper modifyrepo --priority 71 --enable ryoku-issue15-probe`, then `--disable` | Each operation changed that file's digest. | Repository policy/enablement bypass the commit plugin. |
| `zypper removerepo ryoku-issue15-probe` | Removed the file. The complete repository experiment produced zero plugin-log bytes; the plugin hash matched before and after. | This candidate hook cannot protect repository edits. |
| `hostnamectl --static set-hostname ryoku-issue15-admin-probe` | Changed both `/etc/hostname` and `/proc/sys/kernel/hostname`; no plugin frame. Reversing to the fixture's original hostname restored both measured hashes. | Hostname administration bypasses the commit plugin. This reversal is not linked recovery. |
| `zypp-hook-probe tree deny-commit` | The plugin reached `COMMITBEGIN` and replied `ERROR`. Zypper nevertheless installed `tree-2.3.2-1.3.x86_64`, exited 0 in 7909 ms, and changed RPM count from 399 to 400. The closed plugin received no `COMMITEND`. The probe exited 1 because mutation occurred. | **Confirmed fail-open plugin boundary; unsupported for write protection.** |

The two RPM baselines differ because the intervening disposable-fixture setup
installed 26 signed Cockpit/PackageKit packages. No comparison crosses that
setup transaction. Both measured package operations independently required
the requested package to be absent before running.

The normal observation and final denial used the same Rust executable and
libzypp `17.38.16-1.1`/zypper `1.14.101-1.1`. Thus this run closes Run19's
specific “never reached COMMITBEGIN” diagnostic gap. It confirms the previously
reviewed libzypp source behavior; it does not turn the plugin into a recovery
gate. Malformed-response, timeout and missing-plugin modes were not rerun.

The final denying transaction was the last operation that changed provider
state. After read-only result inspection, the guest was powered off. QEMU
exited 0; its overlay, firmware variables, seed, keys and cloud-init files were
deleted. The backing image checksum still matched. No failed-probe overlay
was reused, and no fresh-baseline reboot/reset lifecycle is claimed.

## Affected-state inventory and limits

Package observations measured the complete sorted installed-name/NEVRA list,
requested package presence, actual provider status/frames and executable
payload hashes. `rpm -q --scripts hello tree` returned no package scriptlets.
These are deliberately small packages; absence of their scriptlets does not
bound arbitrary package transactions. The complete inventory for a proposed
protected package operation still includes RPM database and payload/config,
all script/trigger/service effects, boot/initramfs state, repository/cache/trust
state, and matching Ryoku configuration/profile/catalog/journal state.
No claim is made that those components were captured or restored here.

The repository test measured the exact
`/etc/zypp/repos.d/ryoku-issue15-probe.repo` lifecycle: absent, root-owned `0644`
file, changed priority/enablement, absent again. It used a local empty directory,
never refreshed it and imported no trust keys. Repository operations can also
change zypp history/cache/service state. The experiment does **not** certify a
complete repository/trust restoration inventory, or test key import/removal.

The selected administrative test measured `/etc/hostname`, its `0644` root
ownership, the kernel hostname and the native
`org.freedesktop.hostname1.SetStaticHostname` interface. In this fixture,
changing the static hostname also changed the kernel hostname. A future
protected setting must account for both and for dependent services, rather
than assume that replaying one file is sufficient.

Each configuration experiment used an independently empty frame log and
checked the installed denying plugin's SHA-256 before and after its writes.
Only the sequential, bracketed experiments are public evidence; earlier
unbracketed diagnostic attempts are excluded.

## Installed Cockpit and PackageKit boundaries

The guest installed official `cockpit-ws`, `cockpit-bridge` and
`cockpit-system` **365-2.1**, `cockpit-packages` **5-1.1**, and
PackageKit plus its zypp backend **1.4.0-2.1**.
Native D-Bus introspection identified backend `zypp` and `CreateTransaction`.
No PackageKit package transaction was executed, and introspection alone does
not establish a recovery hook or client-loss behavior.

The shipped `/usr/share/cockpit/packages/manifest.json` registers the
**Packages** page in the **Tools** group with minimum Cockpit version 137.
The installed package path is `packages`, not `cockpit-packages`.
The reviewed package release source calls PackageKit `InstallPackages` and
`RemovePackages` through Cockpit's cancellable-transaction helper. Neither
method enters Ryoku's capture/journal path by evidence from this run.

`cockpit-ws` ships `/usr/libexec/cockpit-desktop`. Its inspected implementation
accepts a page path, starts `cockpit-bridge` in the ordinary session, then
starts its browser/web server inside a separate network namespace. It expands
`packages` to `/cockpit/@localhost/packages/index.html`. This is a concrete
upstream local-desktop launch candidate that does not require enabling
`cockpit.socket`. The installed helper needs a suitable browser and graphical
session; this console experiment did not launch it or validate its namespace,
focus, authentication or page interaction at runtime. No native Ryoku
category or embedded panel was implemented.

The shipped system socket specifies `ListenStream=9090` without a loopback
address. It must not be enabled as a supposed local-only endpoint.
This run masked `cockpit.socket` **before** package installation, verified it
remained masked/inactive afterward, and observed no Cockpit TCP listener.
That controlled result is not a claim about an unmodified installation's
default enablement. SSH remained available solely through the experiment's
loopback host forward. No remote Cockpit access was enabled.

Cockpit 365's embedding contract requires the same origin and permits reliance
only on components explicitly documented as API; arbitrary page URLs are not
stable embedding contracts. The Packages manifest establishes menu placement,
not a stable public embedding API. Privilege escalation uses the user's
session/polkit/sudo boundaries and is separate from Ryoku recovery.

## Interface references inspected

These are source/API references, distinct from installed-byte observations.
The release tags below match available package versions; a release tag alone
does not prove the downstream RPM has no patches.

- [Cockpit 365 embedding contract](https://github.com/cockpit-project/cockpit/blob/365/doc/modules/guide/pages/embedding.adoc),
  [privileges](https://docs.cockpit-project.org/cockpit-guide/365/guide/privileges.html)
  and [startup](https://docs.cockpit-project.org/cockpit-guide/365/guide/startup.html):
  framing/origin, authentication, and independent socket activation.
- [Cockpit Packages 5 provider calls](https://github.com/openSUSE/cockpit-packages/blob/ead74e01f946996ef1c50ac5591db54cd36bfa50/src/backend/packagekit.ts):
  `InstallPackages` and `RemovePackages`; errors propagate from the provider.
- [PackageKit 1.4.0 transaction interface](https://github.com/PackageKit/PackageKit/blob/16c3c84ab4c498cefe1668314c9efc04e0c172ab/src/org.freedesktop.PackageKit.Transaction.xml):
  `Cancel`, `AllowCancel`, `ErrorCode`, `Finished`, and role-specific methods.
  Cancellation availability is not rollback or proof of completed recovery.
- [Myrlyn 1.0.0 README](https://github.com/shundhammer/myrlyn/blob/58a8c143ec896a5eacf83f447c8940bc32b6c24b/README.md)
  and [commit/cancel implementation](https://github.com/shundhammer/myrlyn/blob/58a8c143ec896a5eacf83f447c8940bc32b6c24b/src/PkgCommitPage.cc):
  a Qt/libzypp external application with non-root read-only mode; the commit
  page calls `ZYpp::commit` and requests cancellation through its callbacks.
  The frozen repository offers `myrlyn-1.0.0-1.6`, but it was not installed or
  graphically tested.
- [YaST repository client at inspected commit](https://github.com/yast/yast-packager/blob/a8986d4929d173e4f8b1a7e9b0a2ba78f908d6f1/src/lib/packager/clients/repositories.rb):
  separate `Pkg.SourceSetEnabled`, `SourceSetPriority`, `SourceEditSet`,
  `SourceDelete`, `KeyManager.Write`, and `SourceSaveAll` calls demonstrate
  why a package-commit notification is not a generic repository/trust gate.
  This source revision is not asserted to match the guest repository's
  available `yast2-packager-5.0.13-1.1` package. YaST was not installed.
  No new Ruby integration is proposed.

## Remaining issue acceptance

The matrix, concrete interfaces, package notification experiment and observed
bypasses are evidence-backed. A native Rust service-owned package operation
remains **awaiting proof**; the plugin and external writes remain
**unsupported** as protected controls. Minimal release exposure is validated
read-only product information plus individually proven read queries, with
provider mutation controls unavailable.

Issue #15 stays open: there is still no operation with genuine linked
pre-mutation capture, complete affected-state coverage, failure-stop,
post-state verification and matching Ryoku journal evidence. Trust-key writes,
PackageKit/Myrlyn/YaST runtime paths, cancellation/client disconnect/restart,
and actual local Cockpit navigation remain unverified. The unsafe denying
transaction is successful *negative characterization*, never a green safety
test or completion of those acceptance requirements.
