# Rust installer package preflight

`ryoku-install preview` is the read-only package preflight for issue #24. It
accepts the seven required local RPMs and up to two explicitly selected
optional packages. The required set is `ryoku-desktop`,
`ryoku-desktop-runtime`, `ryoku-desktop-fonts`, `ryoku-helper`, `ryoku-tools`,
`ryoku-profile-defaults`, and `ryoku-quickshell`. The optional packages are
`ryoku-tools-ui` and `ryoku-tools-launcher-status`.

The command is shipped with `ryoku-tools-state-init` so it is available after
the separate bootstrap package is installed. Initialize the dedicated state
mount first, then run:

```console
sudo ryoku-install preview \
  ./ryoku-desktop-0.1.0-*.x86_64.rpm \
  ./ryoku-desktop-runtime-0.1.0-*.x86_64.rpm \
  ./ryoku-desktop-fonts-0.1.0-*.noarch.rpm \
  ./ryoku-helper-0.1.0-*.x86_64.rpm \
  ./ryoku-tools-0.1.0-*.x86_64.rpm \
  ./ryoku-profile-defaults-0.1.0-*.noarch.rpm \
  ./ryoku-quickshell-0.3.2-*.x86_64.rpm
```

Use one file for each required pattern; add the GTK proof or launcher-status
RPM only when explicitly selected.

For a removal preview of the recognized installed Ryoku package set, run:

```console
sudo ryoku-install remove-preview
```

This enumerates installed Ryoku payload/configuration paths and confirms the
installed RPMs have no package scriptlets or triggers before asking Zypper for
a removal dry run. The command does not request clean-deps, so unrelated
dependencies are retained in the proposal. Review the complete solver output
for any dependent package removals before a future authorized removal flow.
For each installed payload path, the preview verifies that RPM still
attributes the path to the package that declares it. It marks paths with
remaining non-Ryoku owners as `retained-shared-owner`, package-owned files as
`removed-with-package`, and package-owned directories as
`removed-if-empty-directory`. The installed owner inventory is queried again
after the solver preview and any change invalidates the result. This is RPM
ownership evidence, not a live-file integrity check or an authorized removal
operation.

To inspect one explicitly selected ordinary account for the exact legacy Rust
profile format, run:

```console
sudo ryoku-install migration-preview 1000
```

The UID must resolve to exactly one NSS account with an ordinary home. The
command opens the home and Ryoku configuration directories without following
symlinks, checks ownership and metadata, reads only that account's fixed
`profile.json`, verifies the source did not change during inspection, and
requires `opensuse-profile.json` to be absent before reporting a recognized
candidate. Output includes only the source digest and mode. Missing or
unrecognized documents are left untouched. This installer command remains a
read-only preview. A future migration writer must import only the same exact
canonical legacy format through the account's unprivileged service. It must
create the new personal settings file atomically and preserve the legacy
source. No migration writer is provided by this preflight command.

It requires one unambiguous `/etc/os-release` `ID=opensuse-tumbleweed`, x86_64,
full root/boot Snapper coverage, a
valid initialized Ryoku state mount and generation store, and no unfinished
restore attempt. It inventories installed `ryoku-*` RPMs and reports current
and target NEVRAs for repeat/update review. An unknown Ryoku package name,
unsupported architecture, or duplicate installed revision stops preflight for
explicit migration review. Before asking Zypper, the command checks each
selected RPM's non-directory payload files against a bounded RPM database
ownership inventory and the live filesystem. The preview reports
`direct_file_payload_ownership_conflicts=none` only after this check passes.
It also checks that existing direct directory payload paths are real
directories, without following symlinks; their metadata and RPM co-owners are
bound into a separate `payload_directory_snapshot_sha256`. The preview reports
`direct_directory_type_conflicts=none` and
`direct_directory_metadata_conflicts=none` only after checking existing
directory mode, UID, and GID against the RPM payload. The selected RPMs must
request root-owned directories, and duplicate direct directory paths must
agree on metadata. Transitive dependency
payload conflicts are reported only through Zypper's solver preview, not these
direct-payload checks. Existing file paths are accepted only
when owned solely by the same package being upgraded; unowned files, files
owned by another package, and duplicate selected payload paths stop the
preview. The filesystem check is descriptor-relative and does not follow
symlinked parents or the final entry. Zypper runs with `--dry-run`,
`--no-refresh`, and `--no-recommends`; GPG checks remain enabled. The solver's
detailed output is shown as the resolved transaction preview. Dependency file
conflicts and clean-machine installation still require review and native
acceptance.

The preflight queries each RPM's exact name, NEVRA, architecture, complete
direct payload path list, file-versus-directory modes, and RPM `%config` path
list. It rejects package scriptlets, triggers and file triggers, and requires the fixed package
allowlist. It also rejects direct package payload paths outside `/usr`,
including home directories, `/etc/ryoku`, and the protected state mount. This
preserves user-owned files from direct Ryoku RPM payload writes. The
package-manager solver output must still be reviewed for its complete
dependency transaction and configuration effects. RPM and Zypper stdout and
stderr have separate limits, and both tools run under fixed timeouts.

Before invoking RPM or Zypper, the command opens each operator-selected input
without following a final symlink, checks its type and size, reads a bounded
stable byte snapshot, and writes that snapshot to a random root-private staging
directory under `/var/tmp`; each input is limited to 256 MiB and the whole set
to 1 GiB. All RPM queries and the solver preview use those
staged copies, so replacing or changing the original pathname after validation
does not change the reviewed input. Each staged file is read back and checked
against the captured bytes before it is passed to RPM. The staging directory
is removed on both success and error.

The output includes each staged package's SHA-256 and an
`install_plan_sha256`. The plan digest binds the installed Ryoku NEVRA baseline,
selected package names and NEVRAs, source digests, direct payload paths and
file paths, `%config` paths, the payload ownership/existence snapshot (also
printed as `payload_ownership_snapshot_sha256`), and the exact Zypper solver
preview bytes. Repeating a preview with the same inputs and
solver output produces the same digest. Treat it only as a compact identifier for the reviewed preview: it is not a signature, approval,
authorization token, checkpoint receipt, or permission to apply a transaction.
Any future writer must independently repeat validation under the protected
mutation lock and verify a fresh linked checkpoint.

Both commands are previews: neither applies a proposed transaction, creates a checkpoint,
authorizes a mutation, or advances an operation journal. The later installer
must use the shared protected mutation path, capture and verify a fresh linked
pre-change checkpoint, then revalidate the package and configuration plan
under the mutation lock before applying it. Issue #22 recovery acceptance and
issue #23 clean-install evidence remain prerequisites; this preview does not
close issue #24 or establish installation acceptance.
