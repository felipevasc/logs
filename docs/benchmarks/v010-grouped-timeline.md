# Exact grouped Explore Timeline backend

This slice defines aggregation over an explicitly admitted, filtered and transformed
analysis snapshot. It does not add a Tauri command or look up the mutable active
Case. The admission layer owns source/config/visibility revision checks and must
reject missing/stale identity for an active case-owned analysis.

## Wire result

- `field`: original requested field reference
- `grid`: `{start,bucketMs,bucketCount}`
- `total`: `{count,buckets}`
- `series`: `[{key,count,buckets}]`, exact busiest values
- `other`, `missing`: each `{count,buckets}`, always explicit
- `untimed`, `outsideGrid`: selected records outside the timed chart
- `limit`: effective limit, default 12 and clamped to 1–24
- `selection`: `"top"`
- `context`: `{analysis,sourceGeneration,caseKey}`; analysis is
  `{caseId,analysisId,configRevision,visibilityRevision}` or null for an admitted
  legacy/no-Case view. Source generation is process-local, not durable identity.

Every bucket vector has `bucketCount` elements. Every count equals its bucket sum.
For each bucket, total equals all named series plus Other plus Missing. Top ranking
uses in-grid timed counts descending, then full key ascending for deterministic
ties. Untimed and outsideGrid apply to the complete filtered selection, before
bucketing or group ranking. Timestamp zero follows the existing absent sentinel.

The grid is half-open `[start, start + bucketMs * bucketCount)`, evaluated with i128
arithmetic. Positive grids require positive width and at most 240 buckets. Empty
stats grids use count=0 and width=0. Values beyond the grid are reported separately,
not silently clamped into its last bucket.

Missing means canonical `Ctx::get(field_ref(field))` returned None. Empty strings,
whitespace-only strings, serialized JSON null and full long values remain distinct
keys. Any UI display shortening must retain the full original key for actions.

## Execution and budgets

`engine::grouped_timeline` requires the ready immutable Session and a proven exact
canonical key expression. It computes exact top keys with DuckDB's bounded/spillable
aggregation, then streams at most `(limit+2)*bucketCount+2` histogram groups into the
result. It returns None only for unavailable/unproven capability. Cancellation,
spill, memory or result-budget failures remain errors and never trigger a second
raw scan.

The event fallback accepts only already admitted/visible/transformed filtered
records. It preserves every full key within `LOGINSIGHT_ANALYTICS_LIMIT_MB`, returns
an explicit resource error when that budget is exceeded, and poisons its accumulator
so an ignored row failure cannot be finalized as a partial chart. It does not claim
unbounded high-cardinality fallback support. Callers can narrow filters or increase
the explicit budget; indexed supported fields retain the spill-capable SQL route.

## Verification state

- Four isolated accumulator/grid/budget/wire tests passed using a fake field source
- The actual SQL-producing function was tested against real DuckDB and the exact
  accumulator on a tiny fixture: blank/missing/quoted values, exact top/Other,
  zero/untimed records, empty grids and endpoints requiring i128 arithmetic passed
- Canonical alias/case/nested/long-key tests and engine SQL/fallback parity,
  budget and cancellation tests are authored for the coordinated native run
- App admission, per-Case transforms/exclusions, UI integration and stale revision
  rejection are separate integration tests, not claimed by the isolated drivers
- No full suite, 50M fixture or large-scale grouped query was run in this slice
