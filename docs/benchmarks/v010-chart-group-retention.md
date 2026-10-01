# Chart group retention checkpoint

The legacy chart SQL paths collected every term/split group into a Rust vector,
then sorted and discarded almost everything. A request for ten terms could keep
millions of labels after DuckDB had already completed its own aggregation.

`engine/ops.rs` now consumes these results through the existing fallible Arrow
stream and keeps a worst-first heap of at most the requested terms (capped at
500), or six split names. Work is O(G log K), plus the final O(K log K) ordering.
Live label allocation capacities, count-term JSON sort keys, fixed heap storage
and one candidate share the configured analytics value limit. Evicted/discarded
keys do not accumulate charges. A candidate that exceeds the live budget fails
explicitly even if its rank would have been discarded.

This bounds Rust group selection, not total process RSS: DuckDB grouping/spill,
Arrow chunks and the final chart buffers are separate. Existing unit sampling,
metric arithmetic and split-selection semantics are unchanged. No Other series
was added to legacy charts.

## Exact behavior retained

- Count terms: descending counts, then the JSON serialization of the optional key
- Other terms: descending `f64::total_cmp`, then `Option<String>` ordering
- Split names: descending counts, then the raw string
- Numeric term warnings: incompatible-unit totals include every streamed group,
  even discarded groups and a zero requested term limit
- Fetch, conversion, budget and cancellation errors propagate without publishing
  a partial chart; already bounded time-bucket results also use fallible streaming

## Focused verification

Ten regression tests were added to `series_retention_tests` and
`bounded_analytics_tests::series_*`. A standalone harness compiled the actual
changed production functions, Arrow decoding, numeric/blank UDFs and operation
cancellation module against the existing DuckDB dependency build. It used a tiny
in-memory session/schema adapter; it did not compile the complete application.
All ten tests passed, including:

- 4,096 distinct SQL keys with Top10 under an 8 KiB selection-value budget
- 10,000 candidate replacements with at most six retained keys under 4 KiB
- JSON/raw/Unicode/optional-key ties, NaN payloads, signed zero, and the 500 cap
- Incompatible units below Top N, six selected splits, budget/fetch errors and
  cancellation followed by a successful retry

An additional temporary baseline-comparison test passed 52 complete serialized
response pairs using the pre-change and current production `series_of` functions.
These cover both chart kinds, all six metrics, explicit/automatic units,
split/no-split, missing/escaped/Unicode keys, zero and positive limits, and
negative/zero/missing timestamp grouping keys. Source parsing and `git diff
--check` also passed. The canonical integrated native tests remain queued for the
coordinated validation run. No full build, 50M run, cold-scale run or latency/RSS
claim is part of this checkpoint.

## Metric-specific projection checkpoint

The previous `metric_columns` projection requested all eight aggregates for every
metric with a field. An isolated `EXPLAIN` against the existing DuckDB build
confirmed that `count(DISTINCT ...)` and the file-ordered `sum(... ORDER BY id)`
remained in `HASH_GROUP_BY`, even when the caller only used min or max.

The following slice reduces the transfer to three columns: the selected metric,
admitted samples and applicable unit warnings. Physical-plan tests confirm that
DISTINCT occurs only for distinct, ordered sum only for sum/avg, and min/max only
for their respective metrics. Count/distinct also bind and execute without the
numeric UDFs registered. No-field numeric metrics retain zero values/samples;
unknown metric names retain their earlier zero value and numeric sample/warning
counts. File order is preserved for sum/avg because
[floating-point sum depends on ordering](https://duckdb.org/docs/current/sql/functions/aggregates#sumarg).
This matters for memory as well as CPU: DuckDB documents that
[sorting aggregates require all inputs and some states cannot spill](https://duckdb.org/docs/current/guides/performance/how_to_tune_workloads#limitations).

The standalone production-function harness passes all 13 focused tests, including
52 old/new complete response pairs. An additional fixture inserts numeric values
in reverse file order and checks sum=3 and avg=0.75, exact count/distinct/numeric
samples, incompatible units, missing fields, and buckets with no valid numbers.
Full native compilation and large-scale performance remain deferred.

## Indexed time-bucket checkpoint

The legacy SQL time chart previously owned each selected split label again in
every occupied bucket's `HashMap<String, Metric>`. Six 64 KiB labels over 2,001
occupied buckets imply about 750 MiB of repeated key bytes. The selected labels
themselves are only 384 KiB. This is a source-based allocation calculation, not
an RSS measurement.

Selected names now move once into the final series response. The histogram query
maps them to fixed numeric series IDs, groups by those IDs and returns numeric
rows. Rust writes points and sample counts directly into the final indexed
vectors. It has no intermediate bucket maps or per-bucket label allocations.
For the example above, selected labels plus both numeric vectors occupy about
572 KiB, excluding metadata, SQL literals, DuckDB state, Arrow buffers and other
response data. Sparse buckets remain zero; existing selected-split order,
warning totals and missing/empty split behavior are preserved.

The standalone harness passes 15 tests and 58 complete old/new response pairs,
including six added long-label pairs. A structural test uses six 64 KiB labels
and 2,001 buckets, verifies that their allocation pointers/capacities are moved
unchanged into the final buffers, and calculates the old/new storage shapes
without allocating the old repeated keys. SQL tests cover long quoted/Unicode
labels, sparse buckets, a discarded seventh group, and empty split selection.
These checks establish behavior and allocation structure, not measured latency
or process-memory improvement. Full native/50M validation remains deferred.

The corresponding fallback label-retention issue was taken up in the following
checkpoint, preserving its existing distinct-counter behavior and budgets.

## Fallback bucket-index checkpoint

`analysis::compute_series_budgeted` now uses sparse bucket maps keyed by the
selected series index. It borrows split values for lookup and moves selected
names into the final response, so neither occupied buckets nor response assembly
clone those labels. Metric accumulators remain lazy; numeric input order,
distinct insertion/duplicate charging, spill errors and shared output budgets
are unchanged. Accounting no longer charges removed label copies.

A standalone harness compiled the actual analysis, Event, distinct-counter,
resource and cancellation code, with minimal unused identity/role adapters and
the actual analytics-budget methods. All 14 focused recovery-budget tests pass.
Two new cases cover typed/missing/empty split values and six 8 KiB labels across
41 occupied buckets under a 256 KiB shared analytics allowance for count, sum and
distinct. A 4 KiB allowance still rejects the labels. A separate before/after
check confirms that the old count path exhausts the 256 KiB allowance while the
indexed path returns the complete chart.

A temporary comparison harness also passes 286 complete old/new response pairs
across terms/time, six supported metrics plus unknown names, explicit/automatic
units, absent metric fields, string/dynamic/id/timestamp splits, negative/zero/
missing timestamps, and long labels. Existing injected spill failure and
cancellation tests remain green. Source parsing and diff checks pass; these are
focused checks, with canonical native and scale evaluation still deferred.
