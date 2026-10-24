# Private session clipboard keeper

Issue #25 needs the original daemon's selection retention process. The signed
20261016 fixture repositories do not supply `wl-clip-persist`. This separate
private RPM builds the pinned upstream Rust implementation; the existing daemon
continues to own clipboard history. It creates no competing socket provider.

## Reproduce sources

Use the exact revision and hashes in `upstream/session-dependencies.toml`.
Export the upstream Git revision into a temporary build directory. Copy the
registry crate `wayrs-proto-parser` 3.0.1 (first verify its registry checksum)
into `port/wayrs-proto-parser`, then apply
`upstream/patches/issue25-clipboard-port.patch` with `patch -p1`. Replace the
upstream lock with `packaging/rpm/session/clipboard/Cargo.lock`. Run
`cargo vendor --locked vendor` with normal verified registry transport. The
local parser remains a source path dependency, not a registry replacement.

Create the source archive from only `Cargo.toml`, `Cargo.lock`, `LICENSE`,
`README.md`, `src/` and the parser's `Cargo.toml`, `LICENSE`, `README.md`, `src/`
under the root `wl-clip-persist-0.5.0`. Create the vendor archive with root
`vendor`. For each archive use GNU tar `--sort=name --mtime=@1792617110
--owner=0 --group=0 --numeric-owner -cJf`. Verify both exact SHA-256 values
before handing them to rpmbuild. Cache files, parser registry metadata, nested
lockfiles, `.git` and build output are excluded from the source archive.

## Checks and native build

Run `cargo test --offline --locked --workspace` and strict all-target workspace
Clippy on the derived source, and `cargo deny --offline check` with the project
policy. No vulnerability ignore or duplicate-version exception is added.
The port's tests cover XML entities, malformed XML entities, and actual async
owned-descriptor write/EOF behavior. Native Wayland behavior remains a distinct
acceptance requirement.

Place the verified source and vendor archives in the native RPM SOURCES and
build `packaging/rpm/ryoku-session-clipboard-keeper.spec` with ordinary tester
credentials and BuildRequires enabled. The spec builds offline with the locked
vendor closure and executes the port tests in `%check`. Build and install the
separate `ryoku-session` RPM against that exact keeper RPM. Retain normal native
signature, dependency, conflict and file verification. Fixture signing is not
production signing. Do not infer clipboard correctness from a build or install.

The session uses a 10 MiB regular-selection limit and warning-only logs, with
no timestamp manipulation. Synthetic text/image selection retention after
source exit, history/star/clear, reconnect/error handling and actual native
lock behavior remain pending in `docs/native-desktop-workflows.md`.
