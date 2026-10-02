# v0.10: one-million-record acceptance

On 2026-10-01, a Linux library-test executable passed three separate processes:
new-cache preparation, warm reopening with a 512 MiB engine budget, and warm
reopening with a 256 MiB budget. Each process passed its one selected test and
exited with code 0; the external observer reported no aborts or errors.

## Exact inputs

- Native source checkpoint:
  [`4139457ce197c8ab832e7b41e946550d5237dd45`](https://github.com/felipevasc/logs/commit/4139457ce197c8ab832e7b41e946550d5237dd45).
  Its `src-tauri` subtree matched the build source, Git tree
  `cca227059257488a6eb222793c6952eaeea8a3f5`.
- Executable: `loginsight_lib-0844d2591bd3aa9e`, SHA-256
  `3cf77fc60565e4a925677bd7ce9ad2af01a08ba4c2c791038a7ba15c7c8c199e`.
- [Fixture generator](../../scripts/bench/generate-logs.mjs): version 2,
  1,000,000 rows, seed 42, padding 0, permuted IDs; 278,148,091 bytes.
  Corpus SHA-256:
  `e5f6331c2a4fcb53f150a1dd2a1f33adb04dc3eab9ba317a7b647fc7724d8f79`.
- Point lookup: `0000000000000000000000006585cfa1`, at row ID 500,000.
  Timestamps tie in groups of ten; `rareneedle` occurs every 100,000 rows.
- A new isolated cache produced two complete engine stores, format version 5,
  plus the `indexes-v6/metadata-v1` journal, schema version 1. The measured cache
  key was `632112b8517218eacf69e27fc08000ba652f4e8092f7302a9ffbad5b3fd8541b`.

## Observed timings and memory

Times below are individual observations, in milliseconds. Preparation builds
new metadata and engine stores; the warm processes validate and reuse them.

| Operation | Preparation, 512 MiB | Warm, 512 MiB | Warm, 256 MiB |
|---|---:|---:|---:|
| Metadata preparation / open | 2,490.00 | 35.87 | 32.36 |
| Engine preparation / ready-cache validation | 17,052.00 | 259.92 | 118.86 |
| Count all rows | — | 22.39 | 26.54 |
| Count exact trace ID | — | 6.60 | 6.10 |
| Count rare substring | — | 191.85 | 85.57 |
| First 100 rows, timestamp ascending | — | 25.46 | 9.89 |
| Next 100 rows, timestamp ascending | — | 8.34 | 7.09 |
| First 100 rows, timestamp descending | — | 13.05 | 14.01 |
| Next 100 rows, timestamp descending | — | 26.75 | 14.83 |

Memory is in MiB (1,048,576 bytes). The metadata-completion snapshot is
synchronous; process peaks are sampled every 250 ms. Separately sampled maxima
need not occur together and should not be added.

| Memory measurement | Preparation | Warm, 512 MiB | Warm, 256 MiB |
|---|---:|---:|---:|
| Anonymous at metadata completion | 67.52 | 32.54 | 32.54 |
| Peak sampled Anonymous | 539.62 | 94.38 | 36.05 |
| Peak sampled file-backed RSS | 361.54 | 79.66 | 71.02 |
| Peak sampled total RSS | 899.68 | 173.72 | 107.07 |

Preparation took 19.55 seconds in the test and 19.76 seconds in the observer;
peak new-cache size was 113,972,106 bytes. All three metadata snapshots reported
1,000,000 mapped rows, zero resident rows and zero resident payload bytes. This
diagnostic describes metadata storage, not every allocation in the process.

## Correctness and preservation

Both warm processes returned exactly 1,000,000 / 1 / 10 for the all-row,
exact-ID and rare-substring counts. First and continuation pages matched all
expected row IDs in both timestamp directions, with ascending row-ID tie breaks.
The point lookup returned row 500,000. Metadata samples at rows 0, 500,000 and
999,999 retained their event references. Both stores were complete and ready;
the warm preflight verified parser, calendar, source and schema identity before
opening metadata, without admitting a rebuild.

The full corpus hash matched before and after, and its size, modification time,
device and inode stayed unchanged. Both warm runs preserved the immutable cache
artifacts; 36 manifest/payload hashes were verified after the timed 256 MiB work.
Previously retained caches and baselines were untouched. The minimum observed
free disk space was 2,355,703,808 bytes, above the 2 GiB safety floor.

## Reproduction inputs and limits

Use the two ignored tests in
[`mapped_metadata_workload.rs`](../../src-tauri/src/mapped_metadata_workload.rs):
`mapped_metadata_workload::prepare_one_million_cache`, followed by
`mapped_metadata_workload::mapped_warm_workload` in a fresh process for each
budget. Select tests with `--exact --ignored --nocapture --test-threads=1`.
Supply the generated JSONL and its adjacent `.manifest.json`; preparation
requires an explicitly created, empty cache directory outside the source path.

| Environment input | Value |
|---|---|
| `LOGINSIGHT_BENCH_FILE` | Absolute path to the verified 1M JSONL |
| `LOGINSIGHT_BENCH_DIR` | Absolute path to the isolated cache |
| `LOGINSIGHT_BENCH_ROWS` | `1000000` |
| `LOGINSIGHT_BENCH_PREPARE_NEW` | `1` for preparation only; unset for warm runs |
| `LOGINSIGHT_MEMORY_LIMIT_MB` | `512`, then `256` for the second warm run |
| `LOGINSIGHT_QUERY_SPILL_MB` / `LOGINSIGHT_BUILD_SPILL_MB` | `64` / `64` |
| `LOGINSIGHT_TEMP_LIMIT_MB` | `192` |
| `LOGINSIGHT_TIME_PRECOMPUTE` | `0` |
| `LOGINSIGHT_METADATA_ONLY` / `LOGINSIGHT_BENCH_ENABLE_50M` | Unset |
| Locale and calendar | `LC_ALL=C`; measured system offset −03:00, `TZ`/`TZDIR` unset |

Keep calendar settings consistent between preparation and reopening. The
external observer reserved a 2 GiB disk floor, plus 64 MiB for warm work and
another 192 MiB before preparation; it also enforced a 512 MiB new-cache cap and
stopped on host available memory below 512 MiB for three seconds. These external
guards are separate from the ignored tests and must be supplied when reproducing
the guarded run. Engine budgets and spill limits constrain engine work, **not
process RSS**; the measured preparation RSS exceeded 512 MiB.

There was one observation per stage, with uncontrolled OS page cache; the corpus
had already been read for checksum verification. Sampled peaks are not guaranteed
maxima. No baseline percentage, p95, desktop responsiveness or one-million-row
process-kill/resume result is claimed. No 50-million-row workload ran in this
acceptance; that scale remains disabled by default. This result does not replace
platform CI or installed-updater validation.
