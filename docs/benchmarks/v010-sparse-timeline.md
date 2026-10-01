# Fixed-range Timeline with sparse active exclusions

This stage accelerates only an unfiltered indexed Timeline with an admitted
active-union exclusion mask. The existing requested start/end, ceil bucket width,
inclusive bounds, saturating subtraction and final-bucket clamp remain unchanged.
Filtered Timeline, dynamic Stats, Case evidence and archive scopes are unchanged.

## Dispatch and exactness

- Sources below 100,000 rows keep the existing gated metadata scan. The focused
  comparison below supports this conservative initial floor, not an optimal one.
- Stored mask cardinalities choose the smaller of excluded and visible positional
  rows before reading bitmap words, metadata, or setting up an engine Session.
  A domain larger than 4,096 rows declines immediately. Dense masks do not scan
  the metadata to discover their density.
- Sparse excluded rows are accumulated and subtracted from the complete verified
  seven-class time-v1 sidecar histogram. The complete reader set must account for
  every source row, including untimed rows. Every subtraction is checked.
- A sparse visible remainder reads only its selected metadata rows and directly
  constructs the histogram before Session acquisition, without requiring a
  sidecar or a ready engine store. Exclusion subtraction retains the existing
  Session readiness requirement.
- Bitmap words are visited once using set-bit iteration; complement visits mask
  their final partial word. Stored cardinality, word count, and out-of-domain bits
  are checked. No row IDs or timestamp vectors are collected or cached. At most
  two 240-bucket histograms are temporarily retained, below 12 KiB on x86_64.
- Each mapped appearance is counted once, even when an event reference occurs in
  multiple appended sources. Overlapping exclusion batches remain a union. Zero
  and null timestamps stay outside dated histograms; duplicates and i64 endpoints
  preserve their current bucket semantics.

The RowGate retains the exact indexed Mask used by admission. Its existing source
generation check still binds the metadata Arc and mapped parts. Mask/source and
membership leases are checked before and after the sparse read. Cancellation is
checked during bitmap and selected-row iteration. Exclusion subtraction keeps the
complete sidecar Arc generation pinned and rechecks it after reading metadata;
stale sets are evicted using the existing generation-specific invalidation.
Unsupported capability or checked subtraction mismatch returns to the existing
gated scan. Integrity and cancellation errors never become an unrestricted result.

## Metadata/store equivalence boundary

Direct visible accumulation matches the current metadata Timeline scan. Subtraction
additionally requires metadata and stored canonical timestamp/level equivalence.
Ordinary supported parsers use the same parser and level classification. Catalog
enrichment changes name/description only; derived fields do not replace canonical
timestamp/level fields. Timestamp overlays use the same timestamp-rule evaluator.

This stage conservatively declines subtraction for unknown formats and log4j /
wildfly multiline records, whose metadata may initially parse only the first
physical line. It also declines timestamp rules reading id, event_ref, name or
description: event_at assigns identity and enriches catalogs before evaluating
those rules, while retimestamp_index does not. A native regression records this
existing difference with an id-based rule: metadata sees zero for row 1 while its
hydrated timestamp is 1. Correcting the general timestamp-rule/cache-identity
contract is a separate follow-on; this stage does not mix those domains.

## Focused evidence

The standalone harness compiled the actual sparse histogram, bitmap visitor,
mapped LineStore and time_index writer/reader/ranks. It passed 1,820 exact
comparisons over 70 rows, three segments and all seven internal classes, including
duplicates, null/zero timestamps, negative values and i64 endpoints. Maximum work
was 34 selected metadata rows and two bitmap words. It also checked partial or
missing sidecars, changed sidecar generation, simulated lease rejection, archive
decline, cardinality mismatch, unsupported timestamp rules and named cancellation.
Admission/mask construction was stubbed in this harness. Authored native tests
cover real admitted-mask access, overlapping batches, repeated mapped appearances,
membership corruption and stale-reader eviction; their application build/run is
pending the coordinated integration check.

A subsequent authored routing regression disables Session acquisition and checks that a
real compiled mask still returns its sparse visible remainder, while cancellation
and a changed source remain errors. This closes the case where an unprepared
engine previously sent even a tiny visible remainder through a full metadata scan.

The 70-row arithmetic probe confirmed fixed overhead can lose to a tiny scan:
200 one-exclusion queries initially took 2.13 ms against 0.05 ms for an in-memory
scan. That result motivated the source-size floor; it is not a comparison against
the full application path.

A second focused probe used a synthetic 100,000-row mapped metadata file
(2.7 MB), one sidecar, 30 repeated queries across three ranges, and production
source sample-hash, file-identity, timezone and membership-lease verification.
It included two existing gate validations for both paths and the additional
source/lease validations required by the optional path. Observed microseconds:

| Smaller domain | Optional p50 / p95 | Gated scan p50 / p95 | Sparse metadata reads over 30 queries |
| --- | ---: | ---: | ---: |
| 1 excluded row | 998 / 1,984 | 1,873 / 2,418 | 30 |
| 4,096 excluded rows | 1,045 / 2,732 | 2,001 / 4,310 | 122,880 |
| 32 visible rows | 1,059 / 4,164 | 1,978 / 5,059 | 960 |
| Dense decline | 1,908 / 2,863 | 2,056 / 2,828 | 0 before the existing full scan |

Each accepted query visited 1,563 bitmap words; every full scan visited 100,000
metadata rows. Retained per-row timestamp bytes were zero. The dense timing
difference is noise: both routes execute the same full scan.

These are warm, focused measurements from one run. Session lookup and canonical
archive-origin verification were outside the harness; its synthetic source was
not an archive. The 4,096-row cap does not establish a cold-page or RSS bound:
scattered metadata can fault many mapped pages. Full application validation,
50M/cold workloads, page faults, cancellation latency and memory measurements
remain deferred to the coordinated finalization run.
