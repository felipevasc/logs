# Bounded local derived-field transform prototype

`field_transform.rs` is a pure module, not yet connected to the derived-field editor or query configuration. It adds no dependency. The existing base64 0.22, roxmltree 0.20 and serde_json libraries perform local parsing only.

The pipeline accepts explicit Base64/Base64url encode/decode, URL component encode/decode, form decode, hex/text conversion, JSON/XML/query parsing and JWT payload decoding. URL component decode preserves `+`; form/query decode converts it to a space. Invalid byte sequences fail instead of being silently replaced. Duplicate query/XML elements remain arrays; XML attributes are prefixed with `@`, mixed text with `#text`, and namespace URIs preserve distinct element names. XML is a field projection, not a byte-for-byte round-trip serializer; the original field remains authoritative.

JWT payload decoding returns the mandatory `jwt_signature_not_verified` notice. It does not authenticate claims, verify a signature or decrypt JWE. XML parsing disables DTD support and uses the parser's node limit; there is no external-resource loader. Failure messages contain no input values or decoded secrets.

Default per-record limits: 256 KiB input, 512 KiB output, 4,096 nodes, depth 16 and eight steps. Structured serialization uses a capped writer; XML namespace expansion is charged before allocating repeated owned keys. Typed descendants can be exposed under the existing dotted-column convention while retaining the typed parent; a separate field count and combined payload budget make expansion atomic. Explicit dotted keys retain the JSON importer's existing precedence over nested aliases.

Eight focused standalone Rust tests pass against the production module and existing dependency builds. Cases include Unicode round trips, URL/form differences, duplicate keys, zero/false/null/empty values, namespaces, rejected DTD/external declarations, malformed data, invalid UTF-8, JWT warnings, deep/wide structures, amplification and atomic child-field budgets. No app-level configuration, saved-case isolation or query visibility claim is made by this prototype.

Integration must preserve legacy regex definitions, apply transforms as case-owned overlays, expose parent/children consistently in field discovery/filter/group/pivot/chart routes, and record bounded per-record diagnostics without aborting an otherwise valid source import. Those are subsequent changes, not implied by these unit tests.

## Derived evaluator integration

The evaluator now supports optional `steps` after legacy regex extraction or directly from a source field. Fixed Event columns take precedence over raw aliases, while custom fields keep their JSON types. Per-record diagnostics are capped and never include source values. Typed parent/descendant insertion is atomic and refuses collisions with original fields or reserved metadata targets.

Request-local recomputation restores tagged missing/present original values before applying the next configuration, including an empty configuration. This distinguishes original null from absence and leaves unmarked historical fields alone. The captured evidence object is never mutated by that overlay. Regex expansion now follows the library’s replacement syntax with a bound checked before each append, rather than a pessimistic source-length multiplier. Legacy regex outputs and provenance share a 2 MiB payload allowance per record; allocator/container/parser overhead is additional, so this is not a process heap limit.

Baked derived-store signatures use versioned, structured serialization. Existing base stores remain reusable; older derived variants are rebuilt when needed because evaluator semantics changed. Seventeen focused native checks passed for the transform helper, evaluator/provenance and derived-store identity. Two final defensive guards (reserved pipeline targets and invalid legacy regex-plus-steps) were added after that run and await the next coordinated native check.
