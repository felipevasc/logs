//! Native import contract: exact records/identity, metadata, columns, warm
//! reopen, line/columnar parity. Fixtures never require installed NGINX.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

const MIXED: &str = include_str!("fixtures/import/http-mixed.log");

fn probe(path: &Path, format: &str, cache: &Path, uncached: bool) -> Value {
    testkit::metadata_probe(
        path.to_str().unwrap(),
        format,
        cache.to_str().unwrap(),
        &json!({"uncached": uncached}).to_string(),
        &|_| {},
    )
    .unwrap()
}

#[test]
fn http_mixed_metadata_matches_events_and_reopens_without_reparsing() {
    assert_mixed_import(MIXED);
}

#[test]
fn http_mixed_offsets_preserve_lf_crlf_and_unterminated_last_records() {
    let lines = MIXED.lines().collect::<Vec<_>>();
    for newline in ["\n", "\r\n"] {
        let content = lines.join(newline);
        assert_mixed_import(&content);
        assert_mixed_import(&(content + newline));
    }
}

fn assert_mixed_import(content: &str) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("mixed.log");
    let cache = root.path().join("cache");
    std::fs::write(&path, content).unwrap();
    let first = probe(&path, "auto", &cache, false);
    assert_eq!(first["format"], "mixed");
    assert_eq!(first["rows"], 7);
    let events = first["events"].as_array().unwrap();
    let mut offset = 0usize;
    // Identity uses offsets into the original source bytes. `.lines()` drops
    // CRLF, so counting `line.len() + 1` only works for LF checkouts.
    for (id, record) in content.split_inclusive('\n').enumerate() {
        let line = record.strip_suffix('\n').unwrap_or(record);
        let line = line.strip_suffix('\r').unwrap_or(line);
        assert_eq!(events[id]["raw"], line);
        assert_eq!(events[id]["id"], id);
        assert!(events[id]["event_ref"]
            .as_str()
            .unwrap()
            .ends_with(&format!(":{offset}")));
        assert_eq!(first["metadata"][id][0], offset);
        assert_eq!(
            first["metadata"][id][2].as_i64().unwrap(),
            events[id]["timestamp"].as_i64().unwrap_or(0)
        );
        offset += record.len();
    }
    assert_eq!(events[0]["fields"]["client_ip"], "203.0.113.10");
    assert_eq!(events[1]["fields"]["pid"], "77");
    assert_eq!(events[2]["fields"]["body_bytes_sent"], 9007199254740993u64);
    assert_eq!(events[3]["fields"]["request_time"], "0.1234567890123456789");
    assert_eq!(events[4]["parse_status"], "unparsed");
    assert_eq!(events[5]["fields"]["request"], "-");
    let columns = first["columns"].as_array().unwrap();
    for name in [
        "client_ip",
        "pid",
        "request_time",
        "path",
        "request",
        "status",
    ] {
        assert!(columns.iter().any(|column| column == name));
    }
    assert_eq!(
        columns
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        columns.len()
    );
    let warm = probe(&path, "auto", &cache, false);
    let oracle = probe(&path, "auto", &cache, true);
    for field in ["key", "format", "columns", "metadata", "events", "rows"] {
        assert_eq!(warm[field], first[field], "warm {field}");
        assert_eq!(oracle[field], first[field], "oracle {field}");
    }
    assert_eq!(warm["parsedRows"], 0);
    assert_eq!(warm["resumedRows"], 7);
}

#[test]
fn logfmt_reserved_columns_are_unique_and_original_values_remain_available() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("named.log");
    std::fs::write(
        &path,
        "timestamp=2026-10-02T13:20:30Z level=error code=0042 source=host msg=failed User=alice\n",
    )
    .unwrap();
    let result = probe(&path, "auto", &root.path().join("cache"), false);
    assert_eq!(result["format"], "logfmt");
    let columns = result["columns"].as_array().unwrap();
    assert_eq!(
        columns
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        columns.len()
    );
    let event = &result["events"][0];
    assert_eq!(event["code"], "0042");
    assert_eq!(event["fields"]["code"], "0042");
    assert_eq!(event["fields"]["level"], "error");
    assert_eq!(event["fields"]["User"], "alice");
    assert_eq!(event["fields"]["user"], "alice");
}

#[test]
fn late_structured_rows_are_recovered_only_for_automatic_import() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("late.log");
    let mut text = (0..65)
        .map(|n| format!("plain preamble line {n}\n"))
        .collect::<String>();
    text.push_str(MIXED.lines().next().unwrap());
    text.push('\n');
    std::fs::write(&path, &text).unwrap();
    assert_eq!(testkit::detect_format(text.as_bytes()), "text");
    let auto = probe(&path, "auto", &root.path().join("cache"), false);
    let explicit = probe(&path, "text", &root.path().join("cache"), false);
    assert_eq!(auto["format"], "mixed");
    assert_eq!(auto["events"][65]["parse_status"], "parsed");
    assert_eq!(auto["events"][65]["fields"]["method"], "GET");
    assert_eq!(explicit["format"], "text");
    assert_eq!(explicit["events"][65]["parse_status"], "text");
    assert!(explicit["events"][65]["fields"].get("method").is_none());
    assert_eq!(
        auto["events"][65]["event_ref"],
        explicit["events"][65]["event_ref"]
    );
    assert_eq!(auto["events"][65]["raw"], explicit["events"][65]["raw"]);
}

#[test]
fn structured_http_fields_have_exact_line_and_columnar_query_parity() {
    let root = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", root.path().join("data"));
    std::env::set_var("LOGINSIGHT_ENGINE_DIR", root.path().join("engine"));
    let path = root.path().join("mixed.log");
    std::fs::write(&path, MIXED).unwrap();
    let source = Source::open(&[path.to_str().unwrap()], "{}", "[]").unwrap();
    source.prepare().unwrap();
    for (field, value, expected) in [
        ("pid", "77", vec![1]),
        ("client_ip", "203.0.113.10", vec![0, 5, 6]),
        ("code", "503", vec![2]),
        ("request_time", "0.1234567890123456789", vec![3]),
        ("request_id", "000012345678901234567890", vec![6]),
        ("path", "/health", vec![2, 3]),
    ] {
        let filter = json!([{"column":field,"op":"equals_exact","value":value}]).to_string();
        assert_eq!(source.matches(Engine::Lines, &filter), expected, "{field}");
        assert_eq!(
            source.matches(Engine::Columnar, &filter),
            expected,
            "{field}"
        );
        assert_eq!(
            source.query(Engine::Columnar, &filter, "timestamp", "asc", 0, 20),
            source.query(Engine::Lines, &filter, "timestamp", "asc", 0, 20),
            "{field}"
        );
    }
    let first = source.events_at(&(0..7).collect::<Vec<_>>());
    drop(source);
    let reopened = Source::open(&[path.to_str().unwrap()], "{}", "[]").unwrap();
    reopened.prepare().unwrap();
    let next = reopened.events_at(&(0..7).collect::<Vec<_>>());
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(next).unwrap()
    );
}

#[test]
fn wrapped_http_metadata_uses_outer_nanoseconds_and_bounded_query_fields() {
    let content = [
        json!({"labels":{},"timestamp":"2026-09-16T20:55:04.550942594-03:00",
            "line":"192.0.2.80 - - [16/Sep/2026:20:55:04 -0300] \"OPTIONS /api/example/create-session HTTP/1.1\" 204 0 \"https://example.org/\" \"synthetic-client/1.0\""}),
        json!({"labels":{"job":"synthetic"},
            "line":"192.0.2.80 - - [16/Sep/2026:20:55:05 -0300] \"GET /api/items?page=1&isAssigned=false&tags=one%2C+two HTTP/1.1\" 200 1 \"-\" \"synthetic-client/1.0\""}),
    ].into_iter().map(|value| value.to_string()).collect::<Vec<_>>().join("\n");
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("wrapped.jsonl"); let cache = dir.path().join("cache");
    std::fs::write(&path, &content).unwrap();
    let cold = probe(&path, "auto", &cache, false);
    assert_eq!(cold["rows"], 2); assert_eq!(cold["parsedRows"], 2);
    assert_eq!(cold["events"][0]["timestamp"], 1789602904550i64); assert_eq!(cold["metadata"][0][2], 1789602904550i64);
    assert_eq!(cold["events"][0]["fields"]["method"], "OPTIONS");
    assert_eq!(cold["events"][1]["fields"]["request.page"], "1");
    assert_eq!(cold["events"][1]["fields"]["path.isAssigned"], "false");
    assert_eq!(cold["events"][1]["fields"]["path.tags"], "one, two");
    for column in ["method","client_ip","line.method","request.page","path.tags"] {
        assert!(cold["columns"].as_array().unwrap().iter().any(|value| value == column));
    }
    let warm = probe(&path, "auto", &cache, false); let oracle = probe(&path, "auto", &cache, true);
    assert_eq!(warm["parsedRows"], 0); assert_eq!(warm["resumedRows"], 2);
    for key in ["key","columns","events","metadata"] {
        assert_eq!(cold[key], warm[key], "warm {key}"); assert_eq!(cold[key], oracle[key], "oracle {key}");
    }
    for (record, event) in content.lines().zip(cold["events"].as_array().unwrap()) { assert_eq!(event["raw"], record); }
}
