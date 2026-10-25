# Linux basics on openSUSE Tumbleweed

openSUSE Tumbleweed manages the rolling distribution. Ryoku supplies the desktop
configuration and its own application workflows. Keep distribution packages,
Ryoku configuration and private user data distinct when planning a change.

## Package information

Tumbleweed uses Zypper and RPM. These queries inspect package metadata:

```text
zypper search PACKAGE
zypper info PACKAGE
zypper what-provides CAPABILITY
rpm -q PACKAGE
rpm -qf /absolute/installed/file
```

The official [Tumbleweed Zypper manual](https://manpages.opensuse.org/Tumbleweed/zypper/zypper.8.en.html)
describes package queries, install and removal. A Tumbleweed distribution upgrade
uses `zypper dup`, rather than the Leap update workflow; see the official
[updating and snapshots guide](https://doc.opensuse.org/documentation/tumbleweed/updating_upgrading_reverting/).
For a Ryoku operation, review its typed plan and linked recovery requirements
before authorizing a mutation. This Wiki does not run a package command.

## Recovery

A filesystem rollback alone is not complete desktop recovery. Keep the Snapper
system state paired with the verified Ryoku configuration/profile generation.
Use the recovery catalog's verified pair and retain private offline inputs.
Restoration must preserve unrelated home files. Do not treat a package-only
rollback as a kernel or complete user-state rollback.

## User services and private logs

Desktop services belong to the login account. Inspect the user manager with:

```text
systemctl --user list-units
systemctl --user status UNIT
journalctl --user -u UNIT --since today
```

Read status and journal evidence before restarting a service. A Fix investigation
shows its selected Agent destination and bounded native excerpt before explicit
confirmation. Journal output can contain private data; review the excerpt before
sending it to an external provider.

## Configuration and state

The configuration owner determines the supported edit path. Use the owning app
or command for generated settings. Keep private credentials, histories, usage
sources and recovery material restricted to the account. Explicit source
collection reads only the enrolled private tool directory and reports aggregate
activity; local token estimates and soft caps are not provider quota.

Continue with the [desktop tour](desktop.md) or [Rashin](rashin.md). The exact
Arch-oriented upstream edition of this page is retained separately as a reference.
