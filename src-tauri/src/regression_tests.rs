use crate::{
    analysis, insights,
    model::{CodesConfig, Event},
    query::{self, Filter},
    sources, workspace,
};
use std::{io::Write, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new(text: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "loginsight-test-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, text).unwrap();
        Self(path)
    }
    fn index(&self, format: &str) -> sources::FileIndex {
        sources::index_file(self.0.to_str().unwrap(), format, None, None, None).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn filter(column: &str, op: &str, value: &str) -> Filter {
    Filter {
        column: column.into(),
        op: op.into(),
        value: value.into(),
        value2: None,
    }
}
fn materialize(idx: &sources::FileIndex) -> Vec<Event> {
    let empty = CodesConfig::default();
    (0..idx.lines.len())
        .map(|i| sources::event_at(idx, i, &empty, &empty, &[]))
        .collect()
}

#[test]
fn custom_identifier_uses_saved_parser() {
    let f = Fixture::new("2026-01-01 ERROR|payment|failed\n2026-01-02 INFO|worker|ok\n");
    let parser = sources::CustomParse::Regex(
        regex::Regex::new(r"(?P<level>ERROR|INFO)\|(?P<source>\w+)\|(?P<message>.*)").unwrap(),
    );
    let idx = sources::index_file(
        f.0.to_str().unwrap(),
        "custom:payments",
        Some(parser),
        None,
        None,
    )
    .unwrap();
    assert_eq!(idx.format, "custom");
    let events = materialize(&idx);
    assert_eq!(events[0].source, "payment");
    assert_eq!(events[0].level, "Erro");
}

#[test]
fn composite_index_preserves_origins_and_local_offsets() {
    let text = "{\"timestamp\":1706745600000,\"level\":\"error\",\"message\":\"same\"}\n";
    let a = Fixture::new(text);
    let b = Fixture::new(text);
    let mut idx = a.index("jsonl");
    let first_ref = materialize(&idx)[0].event_ref.clone();
    idx.append(b.index("jsonl"));
    assert_eq!(idx.parts.len(), 2);
    assert_eq!(idx.lines.len(), 2);
    let events = materialize(&idx);
    assert_eq!(events[0].event_ref, first_ref);
    assert_ne!(events[0].event_ref, events[1].event_ref);
    assert_ne!(events[0].fields["caminho"], events[1].fields["caminho"]);
    assert_eq!(events[0].raw, events[1].raw);
    assert_eq!(events[1].id, 1);
}

#[test]
fn memory_and_index_queries_have_equal_counts() {
    let f=Fixture::new("{\"timestamp\":1706745600000,\"log\":{\"level\":\"ERROR\"},\"event\":{\"code\":\"42\"},\"message\":\"failed\",\"latency\":20}\n{\"timestamp\":1706745602000,\"level\":\"INFO\",\"message\":\"ok\",\"latency\":3}\n");
    let idx = f.index("jsonl");
    let events = materialize(&idx);
    let empty = CodesConfig::default();
    for filters in [
        vec![],
        vec![filter("level", "equals", "Erro")],
        vec![filter("code", "equals", "42")],
        vec![filter("_all", "contains", "failed")],
        vec![filter("latency", "gt", "10")],
    ] {
        let a = query::query(&events, &filters, "timestamp", "asc", 0, 100);
        let b = query::query_indexed(
            &idx,
            &filters,
            "timestamp",
            "asc",
            0,
            100,
            &empty,
            &empty,
            &[],
        );
        assert_eq!(a.total, b.total);
        assert_eq!(
            a.rows.iter().map(|e| e.id).collect::<Vec<_>>(),
            b.rows.iter().map(|e| e.id).collect::<Vec<_>>()
        );
    }
}

#[test]
fn non_json_code_filter_and_aggregation_are_not_empty() {
    let f = Fixture::new("CEF:0|Vendor|Product|1|42|Failure|8|src=10.0.0.1 msg=failed\n");
    let idx = f.index("cef");
    let cfg = CodesConfig::default();
    assert_eq!(
        query::query_indexed(
            &idx,
            &[filter("code", "equals", "42")],
            "code",
            "asc",
            0,
            10,
            &cfg,
            &cfg,
            &[]
        )
        .total,
        1
    );
    let spec = query::AggSpec {
        func: "count".into(),
        column: "*".into(),
        alias: "n".into(),
    };
    let agg = query::aggregate_indexed(&idx, &[], "code", &[spec], &cfg, &cfg, &[]);
    assert_eq!(agg.rows[0]["code"], "42");
}

#[test]
fn full_series_includes_incident_after_fifty_thousand_events() {
    let f = Fixture::new("");
    {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&f.0).unwrap());
        for i in 0..60_010 {
            writeln!(
                file,
                "{{\"timestamp\":{},\"level\":\"{}\",\"message\":\"event {}\"}}",
                1706745600000i64 + i * 1000,
                if i >= 60_000 { "error" } else { "info" },
                i
            )
            .unwrap();
        }
    }
    let idx = f.index("jsonl");
    let cfg = CodesConfig::default();
    let spec = analysis::SeriesSpec {
        chart: "time".into(),
        metric: "count".into(),
        field: None,
        interval_ms: Some(1000000),
        split: None,
        limit: None,
        unit: None,
    };
    let result = analysis::compute_series_stream(
        || (0..idx.lines.len()).map(|i| sources::event_at(&idx, i, &cfg, &cfg, &[])),
        &spec,
    );
    let json = serde_json::to_value(result).unwrap();
    let sum: f64 = json["series"][0]["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .sum();
    assert_eq!(sum, 60010.0);
    let result = insights::overview(|| {
        (0..idx.lines.len()).map(|i| sources::event_at(&idx, i, &cfg, &cfg, &[]))
    });
    assert_eq!(result.total, 60010);
    assert_eq!(result.errors, 10);
    assert!(result.complete);
}

#[test]
fn trail_uses_event_ids_in_sparse_subsets() {
    let mut a = Event::empty();
    a.id = 400;
    a.timestamp = Some(1);
    let mut b = a.clone();
    b.id = 900;
    b.timestamp = Some(2);
    let trail = crate::trail_from_events(&[a, b], &[], 400, 0, 0);
    assert_eq!(trail.events[0].id, 400);
    assert!(crate::trail_from_events(&[], &[], 400, 2, 2)
        .events
        .is_empty());
}

#[test]
fn invalid_filters_fail_with_useful_errors() {
    assert!(workspace::validate(&[filter("message", "regex", "(")]).is_err());
    assert!(workspace::validate(&[filter("message", "unknown", "")]).is_err());
    assert!(workspace::validate(&[filter("latency", "gt", "abc")]).is_err());
    let mut f = filter("timestamp", "between", "20");
    f.value2 = Some("10".into());
    assert!(workspace::validate(&[f]).is_err());
}

#[test]
fn explicit_timezone_and_clock_adjustment_are_applied() {
    let config = sources::TsConfig {
        sources: vec!["message".into()],
        format: "%Y-%m-%d %H:%M:%S".into(),
        timezone_offset_minutes: Some(-180),
        clock_adjustment_ms: 5000,
        ..Default::default()
    };
    let mut event = Event::empty();
    event.message = "2024-02-01 00:00:00".into();
    sources::apply_ts_config_event(&mut event, &config.compile().unwrap());
    assert_eq!(event.timestamp, Some(1706756405000));
}

#[test]
fn distinct_counter_spills_without_silently_capping() {
    let mut counter = crate::distinct::Counter::default();
    for i in 0..60010 {
        counter.insert(format!("id-{i}"));
    }
    counter.insert("id-1".into());
    assert_eq!(counter.len(), 60010);
}

#[test]
fn comparison_normalizes_by_volume_and_preserves_new_patterns() {
    let mut events = Vec::new();
    for i in 0..30 {
        let mut ev = Event::empty();
        ev.id = i;
        ev.timestamp = Some(if i < 10 { 10 } else { 110 });
        ev.message = if i < 10 { "ok".into() } else { "failed".into() };
        events.push(ev);
    }
    let result = insights::compare(
        events.into_iter(),
        &insights::Period { start: 0, end: 99 },
        &insights::Period {
            start: 100,
            end: 199,
        },
    );
    assert_eq!(result.before_total, 10);
    assert_eq!(result.after_total, 20);
    assert_eq!(result.changes.len(), 2);
    assert!(result
        .changes
        .iter()
        .any(|c| c.pattern == "failed" && c.before == 0 && c.after == 20 && c.after_rate == 1.0));
}

#[test]
fn parser_quality_marks_unrecognized_records() {
    let ev = sources::parse_line(b"broken JSON", "jsonl", None, &[]);
    assert_eq!(ev.parse_status, "unparsed");
    assert_eq!(ev.raw, "broken JSON");
}

#[test]
fn escaped_unicode_and_anchored_regex_match_in_both_storage_modes() {
    let file=Fixture::new("{\"message\":\"FALHA \\u00c1GUA\",\"timestamp\":1706745600000}\n{\"message\":\"normal\",\"timestamp\":1706745600000}\n");
    let idx = file.index("jsonl");
    let rows = materialize(&idx);
    let empty = CodesConfig::default();
    for f in [
        filter("message", "contains", "água"),
        filter("message", "regex", "^FALHA"),
        filter("_all", "regex", "^FALHA"),
        filter("_all", "contains", "água"),
    ] {
        let a = query::query(&rows, &[f.clone()], "timestamp", "desc", 0, 10);
        let b = query::query_indexed(&idx, &[f], "timestamp", "desc", 0, 10, &empty, &empty, &[]);
        assert_eq!(a.total, 1);
        assert_eq!(a.total, b.total);
        assert_eq!(a.rows[0].id, b.rows[0].id);
    }
}
#[test]
fn redaction_preserves_json_with_escaped_values() {
    let mut value = serde_json::json!({"fields":{"password":"a\"b\\c","authorization":"Bearer private value","ordinary":"unchanged"},"raw":"password=abc, next=ok"});
    workspace::redact_value(&mut value);
    let text = serde_json::to_string(&value).unwrap();
    let restored: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(restored["fields"]["password"], "[oculto]");
    assert_eq!(restored["fields"]["authorization"], "[oculto]");
    assert_eq!(restored["fields"]["ordinary"], "unchanged");
}
#[test]
fn cancelled_work_never_starts_and_scope_is_reset() {
    let mut executed = false;
    assert!(
        crate::operations::run(crate::operations::generation().wrapping_sub(1), || {
            executed = true
        })
        .is_err()
    );
    assert!(!executed);
    assert!(!crate::operations::cancelled());
}
#[test]
fn cached_index_reopens_and_invalidates_when_source_changes() {
    let f = Fixture::new("{\"message\":\"first\"}\n");
    let a = crate::index_cache::open(f.0.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    let id = a.identity.clone();
    drop(a);
    let b = crate::index_cache::open(f.0.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    assert_eq!(b.lines.len(), 1);
    assert_eq!(b.identity, id);
    drop(b);
    std::fs::write(&f.0, "{\"message\":\"first\"}\n{\"message\":\"second\"}\n").unwrap();
    let c = crate::index_cache::open(f.0.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    assert_eq!(c.lines.len(), 2);
    assert_ne!(c.identity, id);
}
#[test]
#[ignore = "Generates a 1 GiB workload; run explicitly in release mode."]
fn benchmark_large_index() {
    let mb = std::env::var("BENCH_MB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1024);
    let file = Fixture::new("");
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&file.0).unwrap());
    let line=format!("{{\"timestamp\":1706745600000,\"level\":\"info\",\"source\":\"worker\",\"message\":\"processed {}\"}}\n","x".repeat(900));
    let count = mb * 1024 * 1024 / line.len();
    for _ in 0..count {
        writer.write_all(line.as_bytes()).unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    let start = std::time::Instant::now();
    let idx =
        crate::index_cache::open(file.0.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    let cold = start.elapsed().as_secs_f64();
    assert_eq!(idx.lines.len(), count);
    let empty = CodesConfig::default();
    let start = std::time::Instant::now();
    let first = query::query_indexed(&idx, &[], "timestamp", "desc", 0, 100, &empty, &empty, &[]);
    let first_seconds = start.elapsed().as_secs_f64();
    assert_eq!(first.total, count);
    assert_eq!(first.rows.len(), 100);
    let start = std::time::Instant::now();
    let next = query::query_indexed(
        &idx,
        &[],
        "timestamp",
        "desc",
        100,
        100,
        &empty,
        &empty,
        &[],
    );
    let page_seconds = start.elapsed().as_secs_f64();
    assert_ne!(first.rows[0].id, next.rows[0].id);
    let start = std::time::Instant::now();
    let summary = insights::overview(|| {
        (0..idx.lines.len()).map(|i| sources::event_at(&idx, i, &empty, &empty, &[]))
    });
    let overview_seconds = start.elapsed().as_secs_f64();
    assert_eq!(summary.total, count);
    drop(idx);
    let start = std::time::Instant::now();
    let reopened =
        crate::index_cache::open(file.0.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    let warm = start.elapsed().as_secs_f64();
    assert_eq!(reopened.lines.len(), count);
    let result = serde_json::json!({"bytes":count*line.len(),"events":count,"cold_index_seconds":cold,"warm_index_seconds":warm,"first_page_seconds":first_seconds,"next_page_seconds":page_seconds,"overview_seconds":overview_seconds});
    println!("BENCHMARK {result}");
    if let Ok(path) = std::env::var("BENCH_RESULT") {
        std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
}
