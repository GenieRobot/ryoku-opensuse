# Read-only opencode SQLite activity

The #183 local collector now falls back from current/legacy filesystem JSON to
opencode's `session_message` and `message` SQLite tables, following the retained
`opencode-usage` source blob `7282aef9c35772cb896eddc51ac5c91ca588d40f` at
`a59d051f0e8080052216f6c7b9eb7e5928935ce8`. V2 blob/role/time column aliases,
nested model/provider fields, SQL timestamp fallback and older flat rows feed
the same private token/cost/cache parser as filesystem messages. Only explicitly
enrolled private roots are eligible. Live OAuth/RPC and unrelated Rashin
capabilities remain separate required work.

Source reads stay in safe Rust. The collector opens exact DB/WAL/journal names
relative to the pinned enrollment, refuses unsafe objects and changed identities,
and acquires Linux OFD shared locks over SQLite's reader range. An exclusive
writer is refused without waiting. Main DB and WAL are copied only within the
existing 64 MiB and two-second budget; sizes/identities/mtime/ctime and sidecar
presence are revalidated. Hot or unrecognized rollback journals are refused;
the collector performs no recovery or checkpoint on application files.

The safe image module validates SQLite page sizes and WAL format 3007000,
header/frame checksums in either byte order, salts, commit boundaries and image
size. Only committed pages are overlaid. Uncommitted frames remain invisible.
A stale older-generation suffix after a valid current commit is handled as in
SQLite's [documented WAL reader/reset algorithm](https://sqlite.org/fileformat2.html#walformat).
An unknown format, damaged current checksum, truncated frame, uncertain snapshot
or source race produces unavailable/stale data, not a partial physical-database
claim. No source page, journal, WAL or SHM byte is written.

The system library receives only the private in-memory image, never a source
path or descriptor. The owned buffer has 64 extra zero bytes, remains stable
through close, and uses the read-only deserialize flag without free/resize
flags. Only this private image's WAL header is normalized for
[SQLite deserialization](https://sqlite.org/c3ref/deserialize.html). No temporary
database copy or source transcript is left on disk.

The single `#[allow(unsafe_code)]` module documents opaque-handle ownership,
statement borrowing, buffer lifetime and callbacks. Handles are non-Send;
statements finalize before the owner closes; UTF-8 bytes are copied only at a
valid result row before SQLite may invalidate them. The authorizer denies all
actions except bounded reads of the two table families/schema and their column
metadata. DDL/DML, ATTACH/DETACH, functions, extensions and dangerous PRAGMAs
are denied. Views/triggers/extensions/trusted schema are disabled, the backing
image is read-only, query-only mode is set, temporary storage is in memory and
virtual-table preparation is prohibited. Only compiled table/column candidate
lists generate SQL; no public request accepts SQL. See SQLite's
[column lifetime](https://sqlite.org/c3ref/column_blob.html) and
[authorizer contract](https://sqlite.org/c3ref/set_authorizer.html).

An isolated system-library probe established actual serialize/deserialize
operation and refusal of UPDATE with `SQLITE_READONLY=8`. The historical
`ENABLE_DESERIALIZE` flag reports zero, `OMIT_DESERIALIZE` reports zero, and the
exported API works. `sqlite3_db_readonly` also reports zero for the memory
connection, so it is deliberately not used as an attestation. The actual
read-only image, query-only setting and authorizer are independent controls.

The installed Tumbleweed provider is exactly `libsqlite3-0-3.53.2-3.1.x86_64`,
owning soname `libsqlite3.so.0` and the resolved library recorded in the receipt.
Rust links that exact soname without a devel symlink, header, SQLite CLI,
downloaded crate or bundled C source. The runtime spec explicitly requires
`libsqlite3-0`; no package was installed or downloaded. The supported native
library remains trusted C code. Separate review of this FFI boundary, hosted
portability checks, complete RPM closure and packaged Tumbleweed VM acceptance
remain required; no memory-safety, native install or linked-recovery claim is
inferred from host tests alone.

Tests create synthetic databases through the installed library, cover both
schemas and all blob/role/time aliases, and exercise the real collector/API.
Real subprocesses hold live WAL/uncommitted/reset states and exclusive locks.
Tests also cover both checksum byte orders, malformed header/frame/salt/CRC,
truncation, interior page corruption, a commit during copy, source/sidecar byte
equality, query/attachment/file-authority denials, limits and last-good stale
reports. Test-only writers are excluded from production builds. Evidence tracks
source/check hashes and public package facts, not raw user data or host logs.
