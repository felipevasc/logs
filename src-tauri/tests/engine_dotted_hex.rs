//! Exact literal dotted fields reuse the same bounded candidates as plain keys.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::fmt::Write;

const TRACE: &str = "0123456789abcdef0123456789abcdef";

fn file(dir: &std::path::Path, name: &str, records: &[Value]) -> String {
    let path = dir.join(name);
    let mut body = String::new();
    for record in records { writeln!(&mut body, "{record}").unwrap(); }
    std::fs::write(&path, body).unwrap();
    path.to_string_lossy().into_owned()
}
fn filters(column: &str, op: &str, value: &str) -> String {
    json!([{"column":column,"op":op,"value":value,"value2":null}]).to_string()
}
fn parity(source: &Source, filter: &str) {
    assert_eq!(source.matches(Engine::Columnar, filter), source.matches(Engine::Lines, filter), "{filter}");
    assert_eq!(source.count(Engine::Columnar, filter), source.count(Engine::Lines, filter), "count {filter}");
    for direction in ["asc", "desc"] {
        let page = source.page(filter, "id", direction, 0, 100, None);
        let expected = source.query(Engine::Lines, filter, "id", direction, 0, 100);
        assert_eq!(page["rows"], expected["rows"], "rows {direction}: {filter}");
        assert_eq!(page["total"], expected["total"]);
    }
}

#[test]
fn dotted_scalar_candidates_preserve_literal_key_case_cursor_and_fallback_semantics() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
    testkit::set_engine_dir(&dir.path().join("engine").to_string_lossy());
    let upper = TRACE.to_ascii_uppercase();
    let first = file(dir.path(), "one.jsonl", &[
        json!({"message":"first", "trace":{"id":TRACE}, "hash":"f".repeat(64)}),
        json!({"message":"collision", "trace":{"id":"other"}, "other":TRACE}),
        json!({"message":"uppercase", "trace.id":upper}),
        json!({"message":"literal wins", "trace":{"id":"wrong"}, "trace.id":TRACE}),
        json!({"message":"numeric", "numeric.id":18446744073709551615u64}),
    ]);
    let second = file(dir.path(), "two.jsonl", &[
        json!({"message":"second part", "trace":{"id":TRACE}}),
        json!({"message":"case-sensitive key", "Trace.ID":TRACE}),
        json!({"message":"request", "request.id":"request/DEADBEEF?x=1"}),
        json!({"message":"nested child", "trace":{"nested":{"id":TRACE}}}),
        json!({"message":"missing", "trace.id":null}),
    ]);
    let source = Source::open(&[&first, &second], "{}", "[]").unwrap();
    source.prepare().unwrap();
    for (column, value) in [
        ("trace.id", TRACE), ("trace.id", upper.as_str()), ("Trace.ID", TRACE),
        ("trace.nested.id", TRACE), ("request.id", "request/DEADBEEF?x=1"),
        ("numeric.id", "18446744073709551615"),
        ("trace.id", "fedcba9876543210fedcba9876543210"), ("trace.ID", TRACE),
    ] { parity(&source, &filters(column, "equals_exact", value)); }
    let exact = filters("trace.id", "equals_exact", TRACE);
    assert_eq!(source.matches(Engine::Columnar, &exact), vec![0, 3, 5]);
    for (column, value) in [("trace.id", upper.as_str()), ("Trace.ID", TRACE), ("numeric.id", "18446744073709551615")] {
        let plan = source.explain_page(&filters(column, "equals_exact", value), "id", "asc", 10, false).unwrap();
        assert_eq!(plan["mode"], "verified_singleton", "a dotted scalar must reach bounded canonical verification");
        assert!(plan["sql"].is_null());
    }
    for direction in ["asc", "desc"] {
        let expected = source.query(Engine::Lines, &exact, "id", direction, 0, 100);
        let mut actual = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = source.page(&exact, "id", direction, actual.len(), 1, cursor.as_deref());
            actual.extend(page["rows"].as_array().unwrap().iter().cloned());
            if page["hasMore"] == false { break; }
            cursor = Some(page["nextCursor"].as_str().expect("next cursor").to_string());
            assert!(actual.len() < 10, "cursor must advance");
        }
        assert_eq!(Value::Array(actual), expected["rows"]);
    }
    for expression in [format!("NOT trace.id:\"{TRACE}\""), format!("trace.id:\"{TRACE}\" OR message:missing")] {
        let filter = filters("_all", "query", &expression);
        parity(&source, &filter);
        assert_ne!(source.explain_page(&filter, "id", "asc", 100, false).unwrap()["mode"], "verified_singleton");
    }
    parity(&source, &filters("trace.id", "not_equals_exact", TRACE));

    let structured = file(dir.path(), "structured.jsonl", &[
        json!({"message":"array", "trace.id":[TRACE]}),
        json!({"message":"scalar", "trace.id":TRACE}),
    ]);
    let structured = Source::open(&[&structured], "{}", "[]").unwrap();
    structured.prepare().unwrap();
    parity(&structured, &exact);
    assert_ne!(structured.explain_page(&exact, "id", "asc", 100, false).unwrap()["mode"], "verified_singleton", "mixed structured field declines the scalar capability");

    // Only one field matches, but cross-field token collisions exceed the
    // existing 4,096-candidate budget. Fallback must return the exact same row.
    let dense: Vec<_> = (0..4_100).map(|id| json!({
        "message":"bounded fallback", "trace.id":if id == 0 { TRACE.to_string() } else { format!("{id:032x}") }, "other":TRACE
    })).collect();
    let dense = file(dir.path(), "dense.jsonl", &dense);
    let dense = Source::open(&[&dense], "{}", "[]").unwrap();
    dense.prepare().unwrap();
    parity(&dense, &exact);
    assert_eq!(dense.matches(Engine::Columnar, &exact), vec![0]);
    assert_ne!(dense.explain_page(&exact, "id", "asc", 100, false).unwrap()["mode"], "verified_singleton", "candidate overflow retains the normal exact SQL route");
}
