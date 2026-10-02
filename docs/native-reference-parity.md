# Native reference number interpretation gate

Reference descriptors retain schema version 1 and gain optional
`interpretationVersion`. Omitted/version 1 preserves existing behavior; explicit
version 2 selects exact native interpretation. Ordinary reference import still
creates version 1. No global serde setting or existing configuration is upgraded.

The concrete decimal witness is `2.547114365375239e-8`:

| Input interpretation | Binary64 bits | Matching reference interpretation |
| --- | --- | --- |
| Existing Dataset `serde_json` parser | `0x3e5b597464455d8b` | v1 |
| Native exact raw-value parser | `0x3e5b597464455d8a` | v2 |
| Native capture of the already-decoded Dataset Number | `0x3e5b597464455d8b` | v1 |

These are different Numbers. A versioned index cannot make both equal without
coercion. The fixture verifies that cross-version lookups do not match. It also
checks separate string `"1"`, integer `1`, float `1.0`, integer zero and negative
floating zero keys, and exact returned fractional values.

`prepare_jsonl_version` and `open_exact` expose explicit v2 preparation. A v2
cache has a distinct `Version.store_version`, schema hash and owned directory;
the existing projection/derived signatures therefore include its interpretation.
The old source and index remain byte-identical. Ordinary `open`, `prepare_jsonl`
remain pinned to a descriptor's declared interpretation, including after a cache
for the other version has been built. `open_source` and `recovery_directory` can
find verified owned original bytes under the other version without rebuilding
or switching indexes during recovery. Only execution of an explicitly selected
interpretation builds its missing cache.

The interpreter runs on source records, stored row bodies and projected return
values, so a second serde parse cannot undo the exact result. Each bounded exact
decode reserves shared work credit before allocation. No global `raw_value` or
`float_roundtrip` feature is enabled.

The selected conservative policy keeps existing Dataset/JSONL/ParseJson
interpretation unchanged. Exact native Case records preserve their actual
number kinds and bits. Capturing an existing Dataset Number does not reinterpret
its source decimal. Native import diagnostics identify the reference
interpretation and explain that equality can differ across the two domains;
there is no hidden numeric coercion. A native input can explicitly declare
version 2 without changing legacy defaults or unrelated global indexes.

Five isolated checks passed against the production reference store, exact
parser, raw document extraction and string-only masking on 2026-10-01. The
harness used the unchanged exact compiler/derived slices where needed and
adapted unrelated IPC/cache-eviction plumbing. This is not the full application
admission or cross-platform activation gate.
