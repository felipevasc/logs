//! Actual compressed import, not just parser matching. Only synthetic records.
use flate2::{write::GzEncoder, Compression};
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::json;
use std::io::Write;

#[test]
fn gzip_json_envelopes_support_multiple_full_record_formats_and_engine_queries() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
    std::env::set_var("LOGINSIGHT_ENGINE_DIR", dir.path().join("engine"));
    let records = [
        json!({"labels":{},"timestamp":"2026-09-16T20:55:04.550942594-03:00","line":"192.0.2.80 - - [16/Sep/2026:20:55:04 -0300] \"OPTIONS /api/example/create-session HTTP/1.1\" 204 0 \"https://example.org/\" \"synthetic-client/1.0\""}),
        json!({"collector":{"payload":"192.0.2.81 - alice [16/Sep/2026:20:55:05 -0300] \"GET /apache?page=1&isAssigned=false&tags=one%2C+two HTTP/1.1\" 200 42"},"enabled":false}),
        json!({"opaque":"<34>1 2026-09-16T23:55:06Z host service 123 ID47 - connection accepted","count":9007199254740993u64}),
        json!({"message":"2026/09/16 23:55:07 [error] 123#124: *9 connection failed, client: 192.0.2.82, server: example.org"}),
        json!({"data":json!({"line":"time=2026-09-16T23:55:08Z level=warn msg=failed request_id=0042"}).to_string()}),
        json!({"line":"ordinary prose mentions /example?page=1 HTTP/1.1\" 200 0"}),
    ].into_iter().map(|record| record.to_string()).collect::<Vec<_>>();
    let path = dir.path().join("collector.json.gz");
    let mut writer = GzEncoder::new(std::fs::File::create(&path).unwrap(), Compression::fast());
    writer.write_all(records.join("\n").as_bytes()).unwrap(); writer.finish().unwrap();
    let compressed = std::fs::read(&path).unwrap();
    let (format, events) = testkit::load(path.to_str().unwrap()).unwrap();
    assert_eq!(format, "jsonl"); assert_eq!(events.len(), records.len());
    for (event, raw) in events.iter().zip(&records) { assert_eq!(&event.raw, raw); }
    assert_eq!(events[0].timestamp, Some(1789602904550));
    assert_eq!(events[0].fields["timestamp"], "2026-09-16T20:55:04.550942594-03:00");
    assert_eq!(events[1].fields["path.tags"], "one, two"); assert_eq!(events[1].fields["enabled"], false);
    assert_eq!(events[2].fields["count"], 9007199254740993u64);
    assert!(!events[5].fields.contains_key("method")); assert!(!events[5].fields.contains_key("line.page"));
    let source = Source::open(&[path.to_str().unwrap()], "{}", "[]").unwrap(); source.prepare().unwrap();
    for (field, value, ids) in [
        ("method","OPTIONS",vec![0]),("path.page","1",vec![1]),("path.isAssigned","false",vec![1]),
        ("path.tags","one, two",vec![1]),("collector.payload.user","alice",vec![1]),
        ("opaque.app","service",vec![2]),("pid","123",vec![2,3]),("request_id","0042",vec![4]),
    ] {
        let filter = json!([{"column":field,"op":"equals_exact","value":value}]).to_string();
        assert_eq!(source.matches(Engine::Lines, &filter), ids, "line {field}");
        assert_eq!(source.matches(Engine::Columnar, &filter), ids, "columnar {field}");
        assert_eq!(source.query(Engine::Lines, &filter, "timestamp", "asc", 0, 20),
            source.query(Engine::Columnar, &filter, "timestamp", "asc", 0, 20), "query {field}");
    }
    let first = serde_json::to_value(source.events_at(&(0..records.len()).collect::<Vec<_>>())).unwrap();
    drop(source);
    let reopened = Source::open(&[path.to_str().unwrap()], "{}", "[]").unwrap(); reopened.prepare().unwrap();
    assert_eq!(serde_json::to_value(reopened.events_at(&(0..records.len()).collect::<Vec<_>>())).unwrap(), first);
    assert_eq!(std::fs::read(&path).unwrap(), compressed);
}
