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
reinterpreted by HTTP fallback. Container envelopes are not unwrapped here.
