# Bounded Case reference runtime

This first usable runtime tier prepares small reference projections once and
then evaluates them entirely in memory. It does not add SQLite or filesystem
access to event evaluation, source scans, or prepared columnar builds. Larger
reference files may remain in the immutable disk store, but binding projections
beyond the explicit runtime limits returns a preparation error.

## Binding contract

`reference_lookup::Definition` uses camelCase JSON:

```json
{
  "schemaVersion": 1,
  "referenceId": "inventory",
  "keys": [
    { "referenceColumn": "tenant", "sourceField": "tenant_id" },
    { "referenceColumn": "host", "sourceField": "request.host" }
  ],
  "valueColumn": "owner"
}
```

The enclosing derived definition is `{id,name,lookup}`. For compatibility,
`source` may equal the first declared binding's `sourceField`, and `rules` and
`steps` may be empty arrays. Other source aliases, nonempty rules/transforms,
legacy extraction properties, and unknown lookup properties are rejected.
Each reference key column must appear exactly once. Preparation normalizes
bindings into the descriptor's key order. The config validator includes all
key sources in dependency ordering and cycle detection and resolves reference
IDs exclusively within that same Config.

`Compiled::new(definition)` performs no I/O. The admission worker calls
`prepare_fields(root, owner, captured_references, compiled_fields, cancelled)`
before evaluation or scheduling a materialized derived variant. It returns a
new vector with immutable prepared bindings and never mutates its arguments.
`retained_bytes(fields)` counts shared projection storage once. Every prepared
binding exposes its exact Case/analysis/content/schema identity through
`Compiled::version()`.

`prepare_fields_detailed` has the same arguments and returns typed
`PreparationError::{Unavailable { reference_id }, RetainedBytes, RetainedRows,
Cancelled, Invalid(String)}`. It expects compiled definitions and performs the
same real preparation. Portable transport can preserve missing/over-budget
assets with precise diagnostics without interpreting error text; invalid or
corrupt assets remain failures. The ordinary helper is its Display wrapper.

Admission must return a preparation failure when a required reference is
missing, corrupt, cancelled, or over budget. It must not reinterpret that
failure as a successful no-match or fall back to global active-Case state.
The Case reference manager inspects and imports JSONL files up to 8 MiB and
100,000 rows, with explicit ordered key columns. It reports current storage
availability and keeps lookup editing separate from regex/transform editing.
Configuration mutations prepare required projections before CAS, so unavailable
or over-budget lookups do not replace a previously usable configuration.

Admission prepares projections in cancellable workers and retains at most two
cached snapshots under a conservative 64 MiB budget including regex and metadata
costs. Portable `.licase` files carry verified reference bytes alongside exclusion
history. Missing or oversized imported lookups and their dependents remain saved
with explicit indexed diagnostics; healthy fields and raw analysis remain usable.

## Representation and resource limits

Each unique `(reference ID, selected value column)` projection is streamed from
the verified SQLite store in canonical BLOB-key order. The runtime retains one
byte arena containing exact encoded keys and JSON values, plus fixed 16-byte
offset/length entries. It uses binary search, without a per-row hash map or a
database connection. Identical projections used by multiple output definitions
share one `Arc`.

Across one prepared Case configuration, retained key/value/index payload is
limited to **8 MiB** and retained projection rows to **100,000**. Multiple
selected columns can retain separate projections and consume those same shared
limits. The loader checks both aggregate limits; it never returns partial
bindings. These are retained-payload bounds, not a process RSS claim: temporary
one-record JSON parsing, vector growth/shrinking, allocator/library overhead,
and concurrent admitted snapshots are additional. Admission's global cache
must account for `retained_bytes` as well as compiled regex memory.

Prepared tables hold a file-only shared `PortableSource` lease transferred
from the verified reader. SQLite closes before a table enters shared state;
`Compiled` is compile-time checked as `Send + Sync`. Evaluation uses only the
copied bytes, so a previously verified snapshot remains exact after backing
storage is externally removed. A cache miss/new process must verify storage
again. Current storage availability must therefore be reported separately from
the usability of an already admitted immutable snapshot.

## Event behavior and materialization

Dynamic source fields retain their JSON scalar type. Canonical metadata fields
use the same text representation as existing transformations, including ISO
timestamp strings; raw same-named dynamic fields cannot shadow them. Missing
inputs omit the output. Null/object/array key inputs produce a bounded record
diagnostic. A missing reference match omits the output; a matching null cell
creates an explicit null field.

The lookup branch runs inside the existing `apply_derived` lifecycle, between
ordinary ordered stages. Structured results use `expanded_fields`, share the
existing per-event output budget, reject target/child collisions, and record
provenance for overlay restoration. This allows transform → lookup → transform
dependencies without repeatedly restoring intermediate outputs.

Every existing consumer that evaluates `CompiledDerived` receives the same
typed Event fields. The existing raw-byte-bounded engine batches, column
sampling, text/entity extraction, and materialized variants use this same
pipeline. Lookup variant signatures include the full immutable reference
Version, normalized lookup definition, and evaluator version. Base stores and
existing regex/transform-only variant signatures remain unchanged.

This establishes a bounded first tier, not a claim that 50-million-row lookup
materialization meets a particular latency. Spillable/batched analytical joins
remain a future tier for larger references; they must preserve this same key,
null, row-count, ordering, and provenance contract.

## Focused validation

The isolated harness passed five runtime tests, eleven real storage tests,
one config/dependency test, and one signature compatibility check. Coverage includes typed integer/float/string/bool
keys, null versus missing, aggregate budgets, shared projections, Case owner
isolation, cancellation, dependency order/cycles, transform/lookup chaining,
structured output, target conflicts, repeated evidence overlays, restoration,
and evaluation after synthetic backing removal. Portable leases are tested for
Send/Sync, actual source bytes, exclusive-lock exclusion, and changed stamps.

The harness compiles actual model, store, transform, evaluator, and validation
code from the worktree; unrelated entity/filter helpers are non-exercised
stubs. It does not replace native integration, command/UI tests, archive
round-trips, or Windows execution. No full application build or scale benchmark
was run for this checkpoint.
