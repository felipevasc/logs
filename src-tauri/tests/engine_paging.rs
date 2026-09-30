//! Native cursor ranges must preserve the exact line-engine order, including
//! missing timestamps (zero), pre-epoch times, ties, and deep-page resumes.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::fmt::Write as _;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn native_cursor_ranges_match_lines_across_timestamp_ties_and_deep_pages() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
    testkit::set_engine_dir(&dir.path().join("engine").to_string_lossy());
    let mut paths = Vec::new();
    let mut total = 0;
    for part in 0..2 {
        let mut contents = String::new();
        for local in 0..701 {
            let i = part * 701 + local;
            let message = if matches!(i, 700 | 1_400) { format!("raw-only-needle record {i}") } else { format!("record {i}") };
            let mut record = json!({"source":"api", "message":message});
            let timestamp = match (i * 37) % 11 {
                0 | 1 => Some(-3_600_000),
                2 => Some(-1),
                3 | 4 => Some(0),
                5 => Some(1),
                6 | 7 => Some(3_600_000),
                _ => None,
            };
            if let Some(timestamp) = timestamp {
                record["timestamp"] = json!(chrono::DateTime::from_timestamp_millis(timestamp).unwrap().to_rfc3339());
            } else if i % 2 == 0 {
                record["timestamp"] = Value::Null;
            }
            writeln!(contents, "{record}").unwrap();
            total += 1;
        }
        let path = dir.path().join(format!("part{part}.jsonl"));
        std::fs::write(&path, contents).unwrap();
        paths.push(path.to_string_lossy().into_owned());
    }
    let source = Source::open(&paths.iter().map(String::as_str).collect::<Vec<_>>(), "{}", "[]").unwrap();
    source.prepare().unwrap();
    let filters = r#"[{"column":"source","op":"equals_exact","value":"api"}]"#;
    for sort in ["id", "timestamp"] {
        for direction in ["asc", "desc"] {
            let reference = source.query(Engine::Lines, filters, sort, direction, 0, total);
            let expected = reference["rows"].as_array().unwrap();
            assert_eq!(expected.len(), total);
            assert!(expected.iter().any(|row| row["timestamp"].is_null()));
            assert!(expected.iter().any(|row| row["timestamp"].as_i64() == Some(0)));
            assert!(expected.iter().any(|row| row["timestamp"].as_i64().is_some_and(|ts| ts < 0)));
            assert!(expected.iter().any(|row| row["timestamp"].as_i64().is_some_and(|ts| ts > 0)));
            // Start both at the beginning and far beyond the first page. The
            // first request uses OFFSET; subsequent requests must use the cursor.
            for start in [0, 1_207] {
                let mut cursor = None;
                let mut offset = start;
                while offset < total {
                    let page = source.page(filters, sort, direction, offset, 37, cursor.as_deref());
                    assert_eq!(page["engine"], "columnar");
                    let rows = page["rows"].as_array().unwrap();
                    let end = (offset + 37).min(total);
                    assert_eq!(rows.as_slice(), &expected[offset..end], "{sort} {direction} offset={offset}");
                    assert_eq!(page["hasMore"], json!(end < total));
                    if end < total {
                        assert!(page["total"].is_null());
                        cursor = Some(page["nextCursor"].as_str().unwrap().to_owned());
                    } else {
                        assert_eq!(page["total"], json!(total));
                        assert!(page["nextCursor"].is_null());
                    }
                    offset = end;
                }
            }
        }
    }
    // This raw-only predicate requires verification outside SQL. Its sparse
    // matches force query_page to advance an internal 1,024-row candidate
    // batch as well as a public cursor; both seek paths must keep the proof.
    let raw = r#"[{"column":"_all","op":"query","value":"bruto:raw-only-needle"}]"#;
    for direction in ["asc", "desc"] {
        let reference = source.query(Engine::Lines, raw, "timestamp", direction, 0, 10);
        let expected = reference["rows"].as_array().unwrap();
        assert_eq!(expected.len(), 2);
        let first = source.page(raw, "timestamp", direction, 0, 1, None);
        assert_eq!(first["rows"].as_array().unwrap().as_slice(), &expected[..1]);
        assert_eq!(first["hasMore"], true);
        assert!(first["warning"].is_string());
        let last = source.page(raw, "timestamp", direction, 1, 1, first["nextCursor"].as_str());
        assert_eq!(last["rows"].as_array().unwrap().as_slice(), &expected[1..]);
        assert_eq!(last["hasMore"], false);
        assert_eq!(last["total"], 2);
    }

}

#[test]
fn session_timestamp_proof_uses_native_order_only_for_null_free_parts() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for (case, null_free) in [("null-free", true), ("one-null", false), ("all-null", false)] {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
        testkit::set_engine_dir(&dir.path().join("engine").to_string_lossy());
        let mut paths = Vec::new();
        for part in 0..2 {
            let mut records = String::new();
            for local in 0..48 {
                let id = part * 48 + local;
                let mut record = json!({"source":"api", "message":format!("record {id}")});
                if case != "all-null" && !(case == "one-null" && id == 95) {
                    let ts = [-1_000, 0, 1_000, 0][id % 4];
                    record["timestamp"] = json!(chrono::DateTime::from_timestamp_millis(ts).unwrap().to_rfc3339());
                }
                writeln!(records, "{record}").unwrap();
            }
            let path = dir.path().join(format!("{case}-{part}.jsonl"));
            std::fs::write(&path, records).unwrap();
            paths.push(path.to_string_lossy().into_owned());
        }
        let source = Source::open(&paths.iter().map(String::as_str).collect::<Vec<_>>(), "{}", "[]").unwrap();
        source.prepare().unwrap();
        for direction in ["asc", "desc"] {
            let plan = source.explain_page("[]", "timestamp", direction, 7, false).unwrap();
            let sql = plan["sql"].as_str().unwrap();
            let key = if null_free { "ts" } else { "COALESCE(ts, 0)" };
            assert!(sql.contains(&format!("ORDER BY {key} {}, id ASC", direction.to_uppercase())), "{case}: {sql}");
            // Projection/cursor shape stays unchanged despite native ordering.
            assert!(sql.starts_with("SELECT id, COALESCE(ts, 0), id "));
            for filters in ["[]", r#"[{"column":"_all","op":"query","value":"bruto:record"}]"#] {
                let reference = source.query(Engine::Lines, filters, "timestamp", direction, 0, 100);
                let expected = reference["rows"].as_array().unwrap();
                assert_eq!(expected.len(), 96);
                assert_eq!(expected.iter().all(|row| !row["timestamp"].is_null()), null_free);
                let mut cursor = None;
                let mut offset = 0;
                while offset < expected.len() {
                    let page = source.page(filters, "timestamp", direction, offset, 7, cursor.as_deref());
                    let end = (offset + 7).min(expected.len());
                    assert_eq!(page["rows"].as_array().unwrap().as_slice(), &expected[offset..end], "{case} {direction} {filters} offset={offset}");
                    assert_eq!(page["hasMore"], json!(end < expected.len()));
                    cursor = page["nextCursor"].as_str().map(str::to_owned);
                    if let Some(cursor) = &cursor {
                        let value: Value = serde_json::from_str(cursor).unwrap();
                        assert_eq!(value["version"], 1);
                        assert_eq!(value["keys"].as_array().unwrap().len(), 2);
                        if offset == 0 {
                            let changed = if direction == "asc" { "desc" } else { "asc" };
                            assert!(source.try_page(filters, "timestamp", changed, end, 7, Some(cursor)).is_err());
                        }
                    }
                    offset = end;
                }
            }
        }
    }
}
