# Portable mapped metadata prototype (0.10)

Status: prototype checkpoint, application-wide validation pending. No 50M rerun yet.

## Choice and compatibility

Keep the existing checksummed `LIDJ0001` journal and parser/calendar/source key.
Each 27-byte record is decoded by value using `from_le_bytes`; no Rust struct
cast, packed-field reference, alignment dependency or native-endian storage is
introduced. Completed journals map their verified committed prefix with a shared
lease on the existing stable writer/pruner lock. Incomplete or invalid journals
never become a published index. Multiple readers coexist; an active reader blocks
cooperating repair and pruning. External mutation of cache files remains outside
the supported immutability contract.

`LineStore` supplies by-value access, iterators, range/partition views and lazy
multi-source offset relocation. Segment views retain shared storage and leases.
Offsets and explicit/fallback event identities are unchanged. Timestamp overlays
are immutable 8-byte little-endian maps; recalculation writes a bounded private
spool on the application data filesystem. Rollback retains the old Arc instead
of copying all timestamps. No implicit mutable-vector fallback exists.

The first checkpoint maps complete/warm journals and releases cold decoded rows
after journal publication. Its cold/partial restore still has the legacy Vec
peak. The second checkpoint replaces that builder with a temporary 27-byte spool,
retaining only the mutable multiline tail and bounded parser/checkpoint batches.
The latter adds up to 27*N temporary disk bytes while writing a durable journal;
steady-state storage stays one journal (1,350,000,072 bytes for 50M rows).
If the durable cache is unavailable or a competing writer prevents the final
handoff, the private spool remains alive with the active source until close.
Storage exhaustion fails explicitly; it never falls back to an all-row heap.
Mapped pages are OS-reclaimable file-backed pages, not a zero-RSS guarantee. Full
checksum validation touches the entire payload. Page-table and source/text index
memory, timestamp ordering vectors and analytic allocations are separate costs.

## Tiny access comparison

Standalone optimized Rust, 250,000 rows, 7 repetitions of 2,000,000 accesses per
case. Packed and native representations produced identical checksums. Raw output
and harness: `output/metadata-prototype/access.jsonl` and `access.rs`.

| Resident-buffer access | Native 32-byte row | Decoded 27-byte row |
| --- | ---: | ---: |
| Sequential, all fields | 3.49 ns | 3.92 ns |
| Deterministic random, all fields | 7.46 ns | 7.61 ns |
| Deterministic random, timestamp only | 5.66 ns | 8.32 ns |

These are small cache-hot CPU measurements, not end-to-end latency, cold-I/O,
RSS, cgroup, Windows or 50M benchmarks. They support feasibility, not a release
performance claim. Unlike a native-layout 32-byte sidecar, direct journal mapping
does not add a permanent 1.6 GB payload at 50M.

## Validation gates

- Exact cold/warm/resume event, offsets, code slices, timestamp and saved-ID parity
- Existing process-kill, corrupt/truncated/uncommitted-tail and parser fixtures
- Shared readers, blocked writer/pruner and lease retention through segment views
- Timestamp cancellation/rollback, parser/calendar/zone identity and multi-source append
- Record/payload overflow, unaligned fields, all integer boundaries and endian codec
- Low-memory process measurements: private dirty/anonymous versus file RSS, faults,
  constrained cgroup warm open/scroll/filter, disk-pressure failure without heap fallback
- Warm checksum cost and random/sequential decode costs on representative workloads

## Primary references

- [memmap2 safety and mapping lifetime](https://docs.rs/memmap2/latest/memmap2/struct.Mmap.html#safety): backing-file mutation requires explicit precautions
- [Rust undefined behavior](https://doc.rust-lang.org/reference/behavior-considered-undefined.html): invalid or misaligned references are prohibited
- [Rust unaligned reads](https://doc.rust-lang.org/std/ptr/fn.read_unaligned.html): even temporary references to packed fields can be invalid; this design uses byte arrays instead
- [fs2 file-lock contract](https://docs.rs/fs2/latest/fs2/trait.FileExt.html#notes-on-file-locks): locks are advisory and duplicated handles require care
- [Linux mmap](https://man7.org/linux/man-pages/man2/mmap.2.html): mappings are file-backed and faults/truncation behavior matter
- [Linux fsync](https://man7.org/linux/man-pages/man2/fsync.2.html): directory persistence needs a directory sync in addition to the file
- [Windows file mapping](https://learn.microsoft.com/en-us/windows/win32/memory/creating-a-file-mapping-object): the view/file lifetime is separate from physical residency
