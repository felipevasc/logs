//! Exact analytics over streamed residual selections, independent of paging.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::io::Write;

#[test]
fn compact_catalog_relation_preserves_lookup_precedence_and_free_text() {
    let dir = tempfile::tempdir().unwrap();
    testkit::set_engine_dir(dir.path().join("engine").to_str().unwrap());
    let path = dir.path().join("catalog.jsonl");
    let mut file = std::fs::File::create(&path).unwrap();
    for source in ["api", "auth", "*"] {
        for code in ["42", "43", "44", "45", ""] {
            writeln!(file, "{}", json!({"source":source,"code":code,"message":"ordinary","name":"intrinsic"})).unwrap();
        }
    }
    let user = json!({"api":{"42":{"name":"user exact","description":"ue"}},"*":{"42":{"name":"user wildcard","description":"uw"},"43":{"name":"","description":"blank override"}}}).to_string();
    let system = json!({"api":{"42":{"name":"system exact hidden"},"43":{"name":"system exact hidden too"},"44":{"name":"system exact"}},"*":{"42":{"name":"system wildcard hidden"},"44":{"name":"system wildcard"},"45":{"name":"last fallback"}}}).to_string();
    let mut source = Source::open(&[path.to_str().unwrap()], &user, "[]").unwrap();
    source.set_system_catalog(&system);
    source.prepare().unwrap();
    assert_eq!(source.query(Engine::Columnar, "[]", "id", "asc", 0, 100), source.query(Engine::Lines, "[]", "id", "asc", 0, 100));
    for word in ["user exact", "user wildcard", "blank override", "system exact", "last fallback", "intrinsic"] {
        let filters = json!([{"column":"_all","op":"query","value":word}]).to_string();
        assert_eq!(source.matches(Engine::Columnar, &filters), source.matches(Engine::Lines, &filters), "{word}");
    }
}

#[test]
fn residual_analytics_share_complete_selections_and_cancel_without_partial_cache() {
    let dir = tempfile::tempdir().unwrap();
    testkit::set_engine_dir(dir.path().join("engine").to_str().unwrap());
    let paths: Vec<_> = (0..2).map(|part| {
        let path = dir.path().join(format!("part-{part}.jsonl"));
        let mut file = std::fs::File::create(&path).unwrap();
        for n in 0..3000 {
            let id = part * 3000 + n;
            writeln!(file, "{}", json!({
                "timestamp": 1700000000000i64 + id * 1000,
                "service": if id % 2 == 0 { "api" } else { "auth" },
                "level": if id % 3 == 0 { "error" } else { "info" },
                "message": format!("Ação {} {}", id, if id % 5 == 0 { "needle" } else { "ordinary" }),
                "number": format!("{} ms", id % 13), "user": format!("user-{}", id % 23),
                "raw_marker": "field only in raw", "nested": { "value": if id % 7 == 0 { "rare" } else { "common" } },
            })).unwrap();
        }
        path
    }).collect();
    let path_strings: Vec<_> = paths.iter().map(|p| p.to_str().unwrap()).collect();
    let source = Source::open(&path_strings, "{}", "[]").unwrap();
    source.prepare().unwrap();
    let filters = json!([
        {"column":"_all", "op":"contains", "value":"raw_marker"},
        {"column":"message", "op":"regex", "value":"Ação.*(needle|ordinary)"},
        {"column":"number", "op":"gte", "value":"3 ms"},
    ]).to_string();
    assert_eq!(source.try_count(&filters).unwrap(), source.count(Engine::Lines, &filters));
    let cached = source.selection_cache();
    assert!(cached["entries"].as_u64().unwrap() > 0);
    assert_eq!(source.stats(Engine::Columnar, &filters), source.stats(Engine::Lines, &filters));
    assert_eq!(source.selection_cache(), cached, "stats should reuse the completed residual table");
    let specs = json!([
        {"func":"count", "column":"*", "alias":"n"},
        {"func":"sum", "column":"number", "alias":"sum"},
        {"func":"avg", "column":"number", "alias":"average"},
        {"func":"count_distinct", "column":"user", "alias":"users"},
        {"func":"concat", "column":"message", "alias":"first_messages"},
    ]).to_string();
    assert_eq!(source.aggregate(Engine::Columnar, &filters, "service", &specs), source.aggregate(Engine::Lines, &filters, "service", &specs));
    assert_eq!(source.selection_cache(), cached);
    assert_eq!(source.multi_count(Engine::Columnar, &filters, &["level", "service"]), source.multi_count(Engine::Lines, &filters, &["level", "service"]));
    assert_eq!(source.selection_cache(), cached);

    let cancelled_filters = json!([{"column":"_all", "op":"regex", "value":"raw_marker.*field only"}]).to_string();
    let before_cancel = source.selection_cache();
    let result = testkit::with_operation("streamed-selection-cancel", |event: Value| {
        if event["phase"] == "Verificação concluída; publicando seleção" {
            testkit::cancel_named("streamed-selection-cancel");
        }
    }, || source.try_count(&cancelled_filters));
    assert!(result.is_err());
    assert_eq!(source.selection_cache(), before_cancel);
    assert_eq!(source.try_count(&cancelled_filters).unwrap(), source.count(Engine::Lines, &cancelled_filters));
}
