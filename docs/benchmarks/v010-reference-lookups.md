# Case-owned reference lookup: first storage checkpoint

This checkpoint implements and tests the local storage/probe primitive in
`src-tauri/src/reference_store.rs`. It does **not** yet enable reference lookup
in the application, change a Case configuration, or claim end-to-end support in
filters, charts, exports, MCP, or large-source scans. Existing regex and
transformation evaluation is unchanged. JSONL is the only prepared format in
this checkpoint; existing CSV/TSV/JSON descriptors remain metadata.

## API and ownership

- `Owner { case_id, analysis_id }` identifies a Case analysis; both components
  are required. A cloned/imported Case must prepare its own owned reference
- `Version { store_version, owner, reference_id, content_sha256, schema_sha256 }`
  identifies immutable bytes and interpretation. The schema digest includes
  format, schema/store versions, ordered columns, ordered key columns, and the
  duplicate policy. The display name is intentionally excluded
- `prepare_jsonl(root, owner, descriptor, input: impl Read, limits, cancelled)`
  returns `PreparedReference`. It does not activate the reference or change a
  Case/source. The caller must separately compare-and-swap the admitted Case
  config revision after preparation
- `open(root, owner, descriptor, cancelled)` returns a `ReferenceReader` that
  owns a read lease and a read-only SQLite connection. Keep this reader for the
  entire prepared operation; opening it again verifies both complete files
- `reader.prepared()` returns immutable version/size/row-count metadata
- `lookup(keys: &[Value], column)` returns `Result<Option<Value>, Error>`.
  `None` is an unmatched key; `Some(Value::Null)` is a matching null cell
- `lookup_many(keys: &[&[Value]], column, cancelled)` is a bounded interactive
  batch with one reused prepared statement. It preserves input order and has no
  cross-product or source-row expansion

Config/visibility revisions remain part of the runtime request identity, not
the file identity. Reusing a reference across config revisions is safe only
after the runtime verifies that the exact immutable descriptor is still in the
admitted Case. This module cannot authorize that admission.

No descriptor or manifest supplies a filesystem path. The caller selects the
application storage root; hashed owner/version components choose paths beneath
it. Store directories and members reject symlinks. Advisory leases protect
cooperating application operations, not arbitrary external filesystem mutation.

## Defined semantics

Each nonempty physical line must contain one JSON object, with exactly the
declared literal column names. CRLF and a final line without a newline work;
blank lines, missing/extra columns, duplicate top-level JSON names, and invalid
JSON fail preparation. Column names are case-sensitive, so `Key` and `key` are
distinct. Duplicate descriptor columns or key-column declarations are invalid.

Composite keys use the ordered array of parsed JSON scalar values encoded by
`serde_json`, not separator concatenation. Strings are exact decoded strings:
no trimming, case folding, numeric coercion, or Unicode normalization. Null,
missing, array, and object key components are rejected. A duplicate composite
key rejects the entire preparation, even if its projected value is identical.

Numbers use the application's `serde_json::Value` representation. Integer `1`,
floating-point `1.0`, string `"1"`, and boolean `true` are distinct keys;
equivalent floating-point spellings such as `1e0` and `1.0` are the same parsed
key. Floating-point keys therefore use the parser's binary64 precision, not
arbitrary-precision arithmetic or original token spelling. Future UI/import
mapping must expose this type contract, especially for identifier columns.
Changing normalization requires a store version bump.

Lookup returns one selected cell as an ordinary typed `Value`, including
structured values. The eventual evaluator must apply the existing derived
output budget, expansion/conflict checks, original-field restoration, and
per-record diagnostics before inserting that value into an event. A store
error is never interpreted as a successful no-match.

## Resource and recovery contract

Default hard ceilings are 256 MiB source bytes, 512 KiB per physical record,
2 million reference records, and 512 MiB for the SQLite database. Callers may
lower these limits. Source reading uses a 64 KiB buffer; JSON parsing and
serialization hold one bounded record at a time. No all-row vector, key hash
map, or in-memory deduplication pass is created.

Keys are limited to 64 KiB encoded. A probe batch has at most 128 keys, 2 MiB
total encoded key bytes, and 2 MiB total serialized output. Oversized input,
output, or cancellation returns an error without a partial result. SQLite uses
a suggested 2 MiB page cache, disabled mmap, a disk-backed B-tree, and a page
count ceiling. These are payload/engine controls, not a total process RSS
claim. JSON trees, allocator overhead, SQLite's other allocations, and OS cache
are additional.

The source copy and database are separate disk costs. Temporary SQLite
journals, staging, simultaneous preparations, and retained versions add to
those costs. There is no aggregate per-Case quota or cleanup implementation in
this checkpoint; runtime admission must account for concurrency and total disk
before making it user-facing.

Preparation creates a private sibling staging directory. It streams exact
source bytes while inserting unique keys into a SQLite transaction. Source
SHA-256 is compared against the descriptor and against the completed retained
copy; the database is closed and hashed completely. A checksummed ready
manifest is flushed and renamed inside staging, then the complete directory
is renamed into the immutable version location. The manifest includes the
exact version, byte sizes, row count, database hash, and record ceiling.

Publication never replaces an existing complete version. A competing complete
publisher is reusable only after normal owner/schema/content/hash validation.
Each open obtains a shared lease first, validates manifest identity/checksum
and limits, and verifies the complete source and database hashes before
opening SQLite read-only. A future cleanup path must obtain the same lease
exclusively before removing a version and must not unlink/recreate a live
lease to bypass a reader.

Cancellation, duplicate keys, malformed input, mismatched hashes, and bounded
resource failures discard staging and leave any existing version usable.
Cancellation is checked again after creating the ready manifest and before
the directory commit. Once publication succeeds, an immutable object may
remain even if later Case admission loses its CAS race; it must never activate
the reference in the wrong Case. A crash can leave private `.prepare-*`
orphans, which readers never consider ready. Recovery cleanup is deferred.

File flushes and Unix directory flushes are used. Windows installed execution,
filesystem/power-loss behavior, and crash-orphan cleanup have not been
validated; no stronger durability guarantee is claimed for those platforms.

## Large-source integration boundary

Reusing a prepared statement is **not** a vectorized join. Probing SQLite for
every event across 50 million source rows must not become an implicit evaluator
fallback. The initial API is suitable for bounded previews and testing semantics.

The next integration should build a prepared derived source variant under the
existing cancellable resource scheduler:

1. Admit the exact Case/config identity and acquire immutable reference leases
2. Export canonical typed key bytes and selected cells to a bounded relation;
   use the same key encoder on source values, avoiding SQL type inference or
   coercions that collapse integer/float/string distinctions
3. Compare bounded repeated-key batching with a DuckDB left equality join on
   canonical key bytes. The reference uniqueness proof must hold before the
   join. Preserve source event IDs/order and distinguish missing matches from
   matched null values
4. Materialize through the existing bounded derived-variant path. Include
   reference versions and lookup semantics in its cache identity, and retain
   the original source/Case until the complete variant is ready and admitted
5. Verify row/columnar parity and all derived consumers before exposing the
   field in filters, groups, charts, exports, evidence, and MCP

This is an integration proposal, not a measured result. DuckDB's join plan,
spill requirements, per-Case quotas, activation UI, CSV/TSV parsing, portable
reference transport, diagnostics, and full application wiring remain open.

## Focused verification

Ten production-module tests passed on Linux using existing native dependency
artifacts: typed/composite keys, missing versus null, duplicate rejection,
case-sensitive columns, owner/schema/hash isolation, corruption of actual
SQLite payload bytes, input/row/database/batch bounds, shared/exclusive leases,
cancellation immediately before publication, reuse, and a fresh-process reopen.

The standalone Rust harness includes the real `reference_store.rs` and extracts
the exact `ReferenceDescriptor` declaration from `analysis_context.rs`; no
store, parser, SQL, hash, or filesystem behavior is stubbed. It does not compile
the application runtime, commands, UI, or engine. Full native integration and
Windows tests remain separate gates. There were no large fixtures or benchmarks.

## Primary sources

- [SQLite CREATE TABLE](https://www.sqlite.org/lang_createtable.html): explicit
  `NOT NULL` and a BLOB primary key on a `WITHOUT ROWID` table avoid nullable-key
  uniqueness surprises. The module relies on SQLite duplicate rejection
- [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html): transaction
  durability depends on filesystem flush/locking behavior. Application-ready
  publication is separate from the database transaction
- [SQLite pragmas](https://www.sqlite.org/pragma.html): `cache_size` is a cache
  suggestion, while `max_page_count` bounds database growth; neither is an
  application-wide memory/disk quota
- [DuckDB workload tuning](https://duckdb.org/docs/lts/guides/performance/how_to_tune_workloads)
  and [memory management](https://www.duckdb.org/2024/07/09/memory-management):
  joins can require substantial state and spill; streaming input alone does
  not remove the preparation resource problem
- [DuckDB indexing](https://www.duckdb.org/docs/current/guides/performance/indexing):
  an index is not a general acceleration strategy for joins and aggregations
