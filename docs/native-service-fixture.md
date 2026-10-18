# Disposable native service fixture

`native_service_fixture` is validation tooling for issue #17. It requires the
seeded disposable QEMU guest, Tumbleweed, PID 1 systemd, enforcing SELinux, active
logind, and the independently initialized protected Btrfs state/machine record.
Run it after state initialization and boot validation. The parent harness owns
VM launch, inputs, private logs and disposal. It does not run on the host.

Build these three inputs from the same committed source, with two build jobs:

```console
CARGO_BUILD_JOBS=2 cargo build --locked -p ryoku-service --bin ryoku-service --bin ryoku-helper --example native_service_fixture
```

Stage the example as `/usr/local/sbin/ryoku-native-service-fixture`, root:root
mode `0755`, with its normal installed SELinux label. Stage the two matching
executables as `ryoku-service` and `ryoku-helper` inside
`/run/ryoku-service-fixture-input`, a root:root mode `0700` directory. Each input
must be a root:root, single-linked regular file of mode `0755`. No path override
is accepted. The fixture embeds the existing user/helper/socket/sysusers
payloads, including the reviewed fixed protected-state namespace exception.

Run the fixed example path as root with `--check` first, then `--run`. The check
is read-only. It inventories actual tool and native-unit RPM owners, verifies
the two inputs through the guest's native loader, checks the protected store,
and refuses existing target files, fixture NSS identities, homes, runtimes,
linger records, overrides or operator group. It requires the helper RPM to be
absent; this is direct payload installation and unit activation, not an RPM
build/install proof. A partial run is never adopted or retried in place.

Required native tools are `rpm`, `systemctl`, `loginctl`, `journalctl`,
`systemd-sysusers`, `getent`, `groupadd`, `useradd`, `usermod`, `runuser`,
`restorecon`, `matchpathcon`, and `bash`. Their actual RPM providers are emitted
as `native_service_tool_<name>_rpm`. Native `user@.service`,
`user-runtime-dir@.service` and user `dbus.socket` providers are also inventoried.
The probe accepts whichever native D-Bus implementation supplies the working
user bus. It reports missing prerequisites before setup and installs no guessed
packages. The loader must exist at `/lib64/ld-linux-x86-64.so.2`; ordinary system
payload directories must be root-owned and unwritable by group/other.
Tool inspection resolves the canonical executable for metadata and RPM-owner
checks, while execution retains the selected safe absolute alias. This preserves
multicall dispatch such as `restorecon` linking to `setfiles`.

The run exclusively creates `ryoku-lab-operator` (UID/primary GID 23171) and
`ryoku-lab-observer` (23172), with locked passwords and separate homes. Only the
operator joins the natively provisioned `ryoku-operators` group. Native
`loginctl enable-linger`, `user-runtime-dir@UID` and `user@UID` create their
runtime directories and managers. The probe verifies runtime ownership/mode,
logind's selected path, the working user bus and the actual unit fragment path.
Bus readiness checks the native socket's active state and queries the user
manager under one bounded deadline; it never manually starts `dbus.socket`,
which native providers may configure for dependency-only activation.
This proves lingered user-manager behavior when it passes, not interactive PAM
login. `Type=simple` startup/restart readiness uses actual discovery and missing
operation exchanges under one absolute 30-second deadline. Restart PID checks
follow that readiness check and require a positive, fresh main PID.

Real child processes run under each NSS UID. Read-only checks cover typed
discovery, a fixed `NotFound` operation lookup, helper availability rejection,
malformed operator frames, root rejection, duplicate-daemon refusal and fresh
process restart. Client frames are capped at 64 KiB, each exchange has a
five-second deadline, and server kernel UIDs and correlations are checked.
The initial phase keeps both journals empty while checking lock and socket
inodes. An extended phase stops the operator daemon, uses `OperationJournal`
to persist an Accepted then Failed fixture operation, and starts the daemon.
New clients before and after another process restart must receive that exact
terminal record; the journal bytes and inode must remain unchanged. No real
checkpoint or provider execution is claimed. A `Start(CreateCheckpoint)` is
sent only to check its structured unsupported rejection. Malformed JSON,
an unknown arbitrary-command request, an unsupported version and a temporarily
renamed journal directory must also produce structured errors without changing
history. The stale directory returns `RecoveryUnavailable`, the current native
runtime contract.

Permission evidence is reported separately. First the original private runtime
and socket modes must produce actual cross-user/nonmember DAC denials. A scoped
fixture-only mode change on held, verified inodes then lets those denied UIDs
reach the independent kernel UID/group gate. Denied peers must receive a typed
`unauthorized` response before any request bytes. The original `0700`, `0600`
and `0660` modes are restored on normal/error paths and revalidated; authorized
read-only exchanges repeat afterward. These assisted policy checks are never
reported as production DAC evidence.

On success the fixture stops its native user/helper units and disables the two
linger records. Accounts, payloads and receipt remain for disposable guest
teardown. On failure they are retained for diagnosis and VM disposal; scoped
permission restoration still runs. Commands have bounded output/deadlines and
bounded native unit logs are captured on failure. These diagnostics belong only
in the harness's private guest logs, not public evidence. The harness must
require every fixed pass marker and retain only the declared package inventory
and pass/not-tested values. The source/tests do not themselves prove activation.


The actual helper process must have exactly `CAP_DAC_READ_SEARCH` in its
permitted, effective and bounding sets, and `NoNewPrivs=1`. Its unit retains
read-only homes and permits writes only to the protected store required by
startup. The user unit disables private mount/user namespaces so strict root
ownership checks see the native identities; it retains its other restrictions.

A separate transient `ryoku-account-fixture` unit executes the existing
`prepare_operator_account` API with the same privilege restrictions. Its only
additional write exception is `/run/ryoku`, for a fixed test socket. Two real
accounts own distinct private `0700` configuration roots and `0600` markers.
The operator resolves and reads only its own marker; the nonmember is rejected
before account resolution. Original modes and markers are checked afterward.
This verifies the account-binding API and capability policy; the production
helper dispatcher still rejects provider requests and does not call that API.

Extended pass markers are emitted on stderr into the private fixture log,
keeping compatibility with the existing runner's strict public marker schema.
Publish only their fixed names/results and log hashes. Do not publish arbitrary
guest diagnostics. The empty-journal marker describes the initial phase only.
