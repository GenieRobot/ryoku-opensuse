# Testing prerequisites checked on openSUSE WSL

Checked against the actual host on 16 October 2026. No additional installation
is needed for the existing console VM experiment. This inventory describes the
implemented test lanes, not a completed Ryoku desktop installation.

## Host packages

All packages below are installed. Use the host for builds and ordinary tests;
filesystem formatting, root rollback and test-account creation belong inside
the disposable guest.

| Purpose | Installed packages / tools |
| --- | --- |
| Rust | `rustup`; repository toolchain **1.99.0**, Cargo, rustfmt, Clippy; `cargo-deny` 0.20.2 |
| Native linking | `gcc`, `gcc-c++`, `binutils`, `glibc-devel`, `make`, `pkgconf-pkg-config` |
| Optional GTK build | `gtk4-devel` 4.22.5; `libopenssl-devel` 3.5.3 is also installed |
| Virtualization | `qemu-x86`, `qemu-tools` **11.1.2-1.1**; `qemu-ovmf-x86_64` **202608-3.1**; `qemu-vgabios` |
| Seed ISO | **`mkisofs`** (the WSL runner selects this executable) |
| Transport and verification | `openssh-clients`, `openssh-common`, `gpg2`, `coreutils`, `git-core`, `rsync` |
| RPM build | `rpm-build` **4.20.1-11.1**, `systemd-rpm-macros` |

The generic defaults RPM built successfully using these tools, without host
installation. A helper RPM additionally requires its source and vendor archives
and satisfaction of the spec's versioned Rust/Cargo BuildRequires; an executable
provided by rustup alone is not evidence that RPM BuildRequires are satisfied.

## Existing console guest

The signed image and repository metadata are pinned in
[`wsl-fixture.toml`](../infra/tumbleweed-lab/wsl-fixture.toml). Migration installs
`btrfsprogs`, `dosfstools`, `parted`, `rsync`, `acl`, `snapper`,
`grub2-x86_64-efi` and `grub2-snapper-plugin`, with their native dependencies.
The image supplies SSH/sudo transport, RPM/Zypper, systemd, util-linux, coreutils,
dracut, udev and SELinux utilities. Run14 exercised the existing initializer,
Snapper capture, protected-store reads and actual recovery CLI under enforcing
SELinux. New binaries still require architecture and dynamic-library checks.

These guest packages do not need installation on the WSL host. Native
`findmnt`, `btrfs`, `snapper`, mount and bootloader operations run in the guest.
The separate profile-bundle probe also needs the built `ryoku-profile-defaults`
RPM installed there and successfully initialized protected state storage.

The future service-isolation exercise needs two ordinary NSS accounts,
logind-managed `/run/user/<UID>` directories and genuine user-manager/session
state. Those are guest fixture setup requirements. The helper additionally
needs the initializer's verified machine record/state mount and systemd socket
activation. No host accounts or system services should be changed for it.

## Launch conditions

- Build the runner with **`--features wsl-lab`**. The default is the old cloud
  runner and correctly rejects WSL authorization.
- Run as the ordinary host user with
  `RYOKU_LAB_AUTHORIZED=existing-wsl-vm`.
- Launch from `/home/jin/src/ryoku-opensuse` when the run directory is beneath
  that workspace. A source checkout under `/tmp` does not authorize a run
  directory elsewhere. Use absolute binary paths to separate build location
  from invocation location.
- Use a fresh absolute run directory outside `/tmp/ryoku-wsl-assets`; retain
  the signed base and matching signature/key material.
- Verify the fixture's image, release signer, firmware and VGA hashes; never
  silently substitute newer inputs.
- Permit only one VM at a time, using 2 vCPUs and 6 GiB RAM. The audited host
  had approximately 9.8 GiB available RAM and 943 GiB disk space. These are
  observations, so check them again immediately before launch. Keep VM disks
  on the workspace filesystem, not the 7.8 GiB `/tmp` tmpfs.
- Use `CARGO_BUILD_JOBS=2`. Use a short temporary root such as `/tmp/rt.XXXXXX`
  for Unix-socket tests to stay within Linux pathname limits.
- Stage and hash exact executable inputs before boot. A passing build does not
  establish guest runtime compatibility, snapshot substitution, reboot or
  restore behavior.

## Current limits

No missing host package explains the two Run15 pre-boot refusals: one invocation
used the cloud build and another used the wrong working directory. Neither
started a VM. Run14's substitution failure was a fixture error involving a
cross-parent rename of a read-only Btrfs subvolume. The corrected sibling-wrapper
exchange still needs native confirmation.

Changed-root rollback is a separate unproven experiment. Its observed GRUB root
pin, snapshot namespace mount, actual EFI boot chain and embedded initramfs
configuration need analysis before changes. Installing more tools does not
establish those contracts. Full installed writers, connected mutation/restore
workflows and desktop/session integration also remain incomplete.
