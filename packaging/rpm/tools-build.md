# Rust service, UI and recovery packages

`ryoku-helper.spec` owns the implemented service/helper binaries, system and
user units and the opt-in `ryoku-operators` sysusers declaration. It preserves
the units accepted under issue #17. Installation does not enable a service,
enroll a user, initialize state or invoke recovery.

`ryoku-ui-proof.spec` remains the sole owner of `/usr/bin/ryoku-ui-gtk` and its
proof application entry/icon. `ryoku-tools.spec` builds the recovery inspector,
explicit administrator initializer, read-only installer preflight and lightweight
launcher provider from the existing workspace. Its launcher provider uses `--no-default-features` so it
does not acquire GTK dependencies. The inspector, initializer, preflight and provider
build in one Cargo invocation so the service's additional Rustix `event`
feature is unified once, avoiding duplicate dependency/recovery compilation. The launcher configuration example is only
documentation; installation never writes a live user configuration.

All executables are root:root mode 0755, without file capabilities or set-ID
bits. The state initializer has native runtime requirements for Snapper,
systemd, util-linux, coreutils and policycoreutils; installing its executable
does not run it. Authorization and the initializer's supported-layout checks
remain mandatory at invocation. Package availability does not certify boot
recovery, restoration or any protected mutation.

The desktop-runtime package separately owns the issue #18 Rust writer wrapper
and its private native payloads. The recovery inspector/state initializer build
selects its binaries explicitly and never duplicates writer ownership.

The three Rust service/UI/tools specs use the same exact port source archive and Cargo vendor
archive, built with `--locked --offline`. Dependencies come from `Cargo.lock`;
the vendor directory preserves registry checksums and upstream licenses.
Native BuildRequires remain checked by `rpmbuild`; a rustup executable alone
does not satisfy RPM-provided `rust`/`cargo` requirements. Record source,
lockfile and vendor digests and the native toolchain NEVRAs with every build.

Mutable defaults and account state belong outside RPM payloads. These specs
own no home, `/etc`, `/var` or `/run` files and contain no project scriptlets
or triggers. The distribution's systemd sysusers/unit triggers apply to their
standard metadata directories. Group creation on installation must be checked
on the native guest; a minimum RPM version alone does not establish it.
