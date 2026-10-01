# Optional exact time precomputation: structural stage

This stage publishes and verifies optional per-segment timestamp summaries in the
existing background queue. Count, statistics and timeline routing are not enabled
by this checkpoint. Missing or corrupt summaries continue to use the existing SQL
route, and optional work never changes the main checkpoint's readiness or key.

## Scheduling and ownership

- All mandatory base/derived stores finish before optional work starts
- Ready warm sessions may enqueue optional work; session creation/getters only
  clone already-verified all-part reader sets, with no sidecar I/O
- Full checksum/order verification and read-only DuckDB sorting occur outside
  Registry locks, one segment at a time, using the normal build memory/spill budget
- Foreground page/detail interactions preempt optional work; status polling does
  not. Retry waits for 250 ms–2 s of quiet time on the existing worker
- Queue revision, source ownership and UpdatePause stop obsolete work. Optional
  I/O failures stay diagnostic; demand-based retries are limited to one per 30 s
- Cached handles retain shared main-checkpoint and sidecar leases. Accepted
  requests retain only current base/desired keys, while source replacement/close
  evicts unowned handles. In-flight readers keep their own Arcs until they finish
- Any absent part means no capability: partial summaries never imply zero matches

## Files and safeguards

The optional file is `<store-key>.time-v1`, bound to the SHA-256 of the existing
complete manifest. The old manifest, store version and source/config keys remain
unchanged. The file contains seven sorted little-endian timestamp arrays, excluding
the existing zero/missing sentinel, plus exact per-level totals. Space is at most
8*N + 192 bytes per segment. Timestamp and calendar semantics come from the already
validated immutable engine store, not a fresh parse of raw input.

Writers hold a shared main-checkpoint lease and an exclusive time lease; readers
hold both shared leases. Named pending data and spill directories carry the store
key, so aged cleanup uses the same main writer lock. Sidecar bytes count toward
cache storage, and deleting a main store requires safe sidecar cleanup first.
Checksum validation is cancellable in bounded chunks and releases the verification
mapping before retaining a demand-paged view. Advisory locking protects cooperating
application processes; external cache mutation remains unsupported.

## Read-only comparisons and disable setting

Set `LOGINSIGHT_TIME_PRECOMPUTE=0` to disable optional build/validation scheduling.
The default is enabled. Disabling preserves exact SQL behavior and is required for
read-only comparisons against the retained 0.9 corpus/cache, so those runs do not
silently add approximately 400 MB of summaries at 50M rows. This setting is separate
from engine memory budgets and does not impose a process RSS limit.

## Focused verification and next consumer

The producer/lease driver has exercised real DuckDB sorting, exact total count,
dual-lease lifetime, cancellation, corruption rejection/repair, unchanged reuse,
cleanup and temporary-file removal on a tiny fixture. Reference stats/query tests,
new all-or-none cache ownership tests and queue preemption tests are included in
source for the next coordinated focused native run. No 50M or cold-scale run is
part of this structural checkpoint.

Next consumers are capability-gated exact count/stats, then fixed-range timeline
histograms. The latter should rank requested bucket boundaries in the sorted
arrays instead of scanning every LineStore row on each zoom. Parity must preserve
requested bounds, ceil bucket widths, final-bucket inclusion, ties, zero/missing
and negative timestamps, error/warning classes, and all-part fallback.
