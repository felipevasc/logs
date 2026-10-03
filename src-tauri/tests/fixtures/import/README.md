# HTTP log import fixtures

Synthetic data only (RFC 5737 / RFC 3849 example addresses). `http-records.json`
is consumed by native tests in `sources/web_logs.rs`; `http-mixed.log` also
exercises the actual file-loading, metadata, and query paths.

References:
- [NGINX access format, escaping, missing values](https://nginx.org/en/docs/http/ngx_http_log_module.html#log_format)
- [Apache common/combined](https://httpd.apache.org/docs/2.4/logs.html#common)
- [NGINX error prefix implementation](https://github.com/nginx/nginx/blob/master/src/core/ngx_log.c)
- [NGINX HTTP error context](https://github.com/nginx/nginx/blob/master/src/http/ngx_http_request.c)

Extraction preserves the exact source spelling of quoted escapes (default,
JSON, or literal) instead of guessing the producer's `escape=` setting. Raw
records are always retained. Only literal named suffixes become named fields;
unknown positional suffixes stay in `access.extra`. A custom positional format
requires an explicit schema. JSON HTTP aliases are vendor-neutral and do not
replace original fields, explicit standard columns, or explicit severity.

Run when the native toolchain is available:

```sh
cargo test --manifest-path src-tauri/Cargo.toml sources::web_logs --lib -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --test http_import -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --test nginx_acceptance -- --test-threads=1
```

These focused tests do not replace the required complete native Windows/Linux
and browser checks. The initial format label uses the first 60 physical lines. An inconclusive
automatic detection keeps conservative per-record recognition enabled, even
after a long plain-text preamble. Column discovery still samples at most 4,000
stratified events. Java
multiline framing, explicit plain text, custom schemas and snapshots are not
reinterpreted by HTTP fallback. JSON collector envelopes are enriched only
when a whole string field is a recognized self-describing record: Nginx/Apache
access, Nginx error, JSON, syslog, Java headers, CEF/LEEF or strict whole-record
logfmt. Known message keys receive priority, but arbitrary/nested names qualify.
Embedded RFC5424 recognition conservatively requires an explicit PRI, version 1,
a valid RFC3339 timestamp or `-`, and `-` structured data. Structured-data blocks
are retained as raw text rather than guessed by this automatic path; the
existing explicitly selected syslog parser is unchanged.
Enrichment is bounded to 3 levels, 32 inspected values, 64 KiB per value,
256 KiB inspected per record and 512 generated fields. Embedded source/inner
field names are capped at 512 bytes. Query expansion separately limits each
value to 64 KiB and 256 parameters, and each event to 64 inspected sources,
256 KiB and 512 outputs. Header-dependent CSV/W3C/Zeek and prose are not guessed.

The JSON raw record and outer values/types remain intact. Inner fields are
available canonically (without replacing outer fields) and under their
containing namespace, for example `line.*` or `container.payload.*`. If sibling
fields contain separate logs, only namespaced fields are added; no arbitrary
sibling becomes the canonical event. An outer timestamp wins, retains its exact
source precision in fields, and is indexed in milliseconds. Query expansion
reads complete URIs/request targets, never the rest of an access line, and is
idempotent across normalization passes.

Parser revision `web-logs-v2` and global query generation `indexes-v7` refresh
old raw-artifact metadata, columnar and timestamp caches on reopen without
changing source/event identity. Saved Case evidence and explicit Event
snapshots stay captured evidence and are not silently reparsed.
`nginx_acceptance` covers actual JSONL gzip loading and cold/warm reopening in
separate processes; `wrapped_log_import` covers a compressed mixed-envelope
pipeline with line/columnar query parity.

To inspect corrected raw data in an existing Case, use **Estrutura → Arquivos →
Recarregar fontes**, then **Análise → Explorar**. This clears current search and
filters; saved evidence and item filters are preserved. **Reabrir recorte** can
restore a saved item's filters after reloading. **Atualizar** alone does not
reload the source. Captured Case evidence stays as it was: recapturing identical
event references into that same Case can retain the first capture during
aggregation. A separate **Novo Caso** with the same raw sources provides a clean
corrected evidence collection while preserving the old one.
