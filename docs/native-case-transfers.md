# Native Case transfer boundary

This source module remains separate from command/UI activation. Native export
resolves issued references from SQLite and immutable evidence files. No record
array received over IPC is an export source. Native import parses record arrays
as borrowed checked JSON spans, seals immutable files, and publishes ownership
only in the native Case transaction.

## Wire contracts

Tauri commands are `case_import_native` and `case_export_native`, with
`{request: {...}, operationId?: string}`. Their wrappers use `offload_operation`
and return the retained receipt types directly.

- Export request: `{store, documentJson, path, mask, requestId}`. `documentJson`
  is a native management document; it may select a subset of its Cases. Pending
  evidence must be saved first. An unchanged server-issued unavailable stub is
  accepted and resolves to its checked original native body.
- Export result: `{kind: "native_case_export", path, format, cases, records,
  masked}`. `records` is null if a preserved opaque Case cannot be counted
  safely; it is never falsely reported as zero.
- Import request: `{store, path, requestId}`. Selection comes from the hashed
  file. Existing Case metadata comes from `load_view`, unchanged.
- Import result: `{importedCaseId, importedCaseIds, caseViews, receipt}`. The
  receipt is the native import/save receipt, including committed/current store
  stamps, replay and reconciliation state.

Import hashes the selected bytes and checks replay before generating local
Case/analysis identities. A retry after subsequent edits returns the original
imported IDs and acknowledged views and marks reconciliation when appropriate.
A changed file cannot reuse the same request identity. Import is append-only.

## Versioned file envelope

Native JSON has no root-level `cases` field:

```json
{
  "format": "loginsight.native-case",
  "schemaVersion": 3,
  "document": {"active": "foreign-id", "cases": []},
  "nativeEvidenceMap": {"schemaVersion": 1, "cases": []}
}
```

The map is outside authored Case JSON. A Case may retain an unrelated authored
property named `nativeEvidenceMap`. Case maps are in ordinal order and carry
frozen blocked aliases plus sparse container and occurrence correspondence:

```text
PortableCaseMap { caseOrdinal, anchors, containers, members }
PortableContainerMap { containerId, location }
PortableMemberMap { containerId, occurrenceId, recordOrdinal }
```

`recordOrdinal` is the exported current-array position. It is not an occurrence's
possibly sparse original position. Only authored native anchors and typed
Trail/hypothesis container references require mapping. Correspondence uses
verified container/occurrence identity, never equality of record contents.
Missing targets remain explicitly unresolved, including on later transfers.
Each Case map admits at most 10,000 combined entries and 4 MiB; all maps have an
8 MiB aggregate cap. The checked document is bounded at 64 MiB.

Native `.licase` uses binary header version 3, transport manifest version 3,
metadata schema version 3 and the same nested document. Its dedicated reader
also accepts legacy binary version 1 and legacy metadata versions 1/2. The
ordinary legacy reader continues to reject native version 3. The v0.9 command
path at commit `363b11e` checks JSON schema version and root `cases` before
frontend normalization; native JSON fails both guards, and native binary data
fails its JSON parser.

## Assets and preservation

JSON refuses exclusion history and reference declarations, directing the user
to complete `.licase` export. `.licase` verifies owned source bytes, images,
exclusion batches/masks and reference descriptors before Case publication.
Immutable files are prepared before SQL; Case metadata, native ownership,
contexts, exclusion history, receipt and active selection commit together.
An aborted transaction can leave unreferenced immutable files, never partially
published Case ownership or a success receipt.

All untargeted record fragments retain their original bytes, including unknown
members, large integers, fractional values, exponent spellings and `-0.0`.
Masking changes targeted sensitive values only. Full archives with original
reference bytes or exclusion provenance refuse masking. Parseable numeric
metadata that cannot safely enter JavaScript stays behind an unavailable stub;
its native body and mapped identities remain exportable. Unsupported opaque
layouts keep their original body and frozen unresolved decisions.

Reference interpretation is explicit. Omitted/version 1 retains legacy numeric
behavior. Only an explicit version 2 descriptor selects native exact decoding;
no global serde feature or existing Dataset interpretation is changed. Recovery
can read verified owned source bytes from an alternate cache only when the
preferred cache is absent; a corrupt preferred cache never falls back.

## Verification boundary

The isolated coupled harness uses the actual native evidence, Case publication,
portable archive, image, reference, exclusion and recovery modules. Unrelated
application IPC, resource accounting entry points and event-source adapters are
minimal harness adapters. Focused tests cover duplicate raw-record identity,
version rejection, full asset publication, file/request replay, stale authority,
masking and numeric reference interpretation. Full application, cross-platform
and activation checks remain separate gates.

On 2026-10-01, the final isolated gate passed 23 focused checks and all 252
combined checks, using storage checkpoint `6d0ac78` and native-save checkpoint
`d87c731`. The gate includes a valid 1.25 MiB context whose metadata admission
exceeds the unrelated 4 MiB record-value tree ceiling, plus a forced failure
inside exclusion insertion proving rollback of native Case ownership and import
receipt before a successful retry.

Legacy Case export endpoints now refuse protected native owners inside their
context snapshot and again immediately before destination replacement. This
also blocks an old export when adoption commits during serialization. For a
selected canonical `case-profiles-v1/<uuid>` root, native export destinations
protect the original base namespace and sibling profiles. The final follow-on
passed all 267 coupled checks against bootstrap checkpoint `a8a116c`, including
three regressions for those boundaries. Actual Tauri handler validation remains
part of the integrated application gate.
