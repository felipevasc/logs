use crate::{
    big_data::BigDataIndex,
    model::CodesConfig,
    query::{self, Filter},
    sources,
};
use serde_json::{json, Value};
use std::{io::Write, sync::Arc, time::Instant};

fn filter(column: &str, op: &str, value: &str) -> Filter {
    Filter {
        column: column.into(),
        op: op.into(),
        value: value.into(),
        value2: None,
    }
}

fn normalized(mut value: Value) -> Value {
    for pointer in ["/stats/levels", "/levels"] {
        if let Some(levels) = value.pointer_mut(pointer).and_then(Value::as_array_mut) {
            levels.sort_by_key(|v| v[0].as_str().unwrap_or_default().to_string());
        }
    }
    value
}

#[test]
fn big_data_preserves_filters_pagination_aggregation_and_event_identity() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("events.jsonl");
    let text = [
        json!({"timestamp":1700000000000i64,"source":"API-A","level":"error","code":"401","message":"Falha de AUTENTICAÇÃO; linha\nseguinte","user":" Alice ","bytes":"2 MB","nested":{"key":"Escaped \"phrase\""}}),
        json!({"timestamp":1700000000001i64,"source":"api-a","level":"warn","code":"400","message":"worker ready","user":"bob","bytes":"10 MB","status":"500","path":"/api/pay?client=EMPRESA"}),
        json!({"timestamp":1700000000002i64,"source":"API-B","level":"info","code":"200","message":"worker finished","user":"CAROL","bytes":"900KB","src_ip":"10.1.2.3"}),
        json!({"source":"","level":"debug","message":"prefix exact end","user":"a;b:c=d,e","bytes":"11x"}),
        json!({"timestamp":1700000000003i64,"source":"API-C","level":"error","message":"Καλημέρα ΟΣΑ Kelvin","user":"","flag":null}),
        json!({"timestamp":1700000000004i64,"source":"API-D","message":"ſtart","user":"unicode-fold"}),
        json!({"timestamp":"1970-01-01T00:00:00Z","source":"API-E","message":"epoch-zero"}),
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    std::fs::write(&path, text).unwrap();
    let mut idx = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    let codes = CodesConfig::default();
    let events: Vec<_> = (0..idx.lines.len())
        .map(|i| sources::event_at(&idx, i, &codes, &codes, &[]))
        .collect();
    let engine = Arc::new(
        BigDataIndex::open_or_build(
            &idx,
            &codes,
            &codes,
            &[],
            &folder.path().join("cache"),
            None,
        )
        .unwrap(),
    );
    let cases = vec![
        vec![],
        vec![filter("source", "equals", " API-A ")],
        vec![filter("source", "equals_exact", "API-A")],
        vec![filter("source", "in", "api-a,API-B")],
        vec![filter("source", "in_exact", "API-A\nAPI-C")],
        vec![filter("source", "not_equals", "API-A")],
        vec![filter("message", "contains", "autenticação")],
        vec![filter("message", "contains", "linha\nseguinte")],
        vec![filter("message", "contains", "Καλημέρα")],
        vec![filter("message", "contains", "Kelvin")],
        vec![filter("message", "starts_with", "prefix")],
        vec![filter("message", "ends_with", "end")],
        vec![filter("message", "regex", "(?i)worker.*(ready|finished)")],
        vec![filter("message", "regex", "^prefix.*end$")],
        vec![filter("message", "regex", "(?:worker|prefix) .*")],
        vec![filter("message", "regex", ".*")],
        vec![filter("message", "regex", "ΟΣ")],
        vec![filter("user", "empty", "")],
        vec![filter("user", "not_empty", "")],
        vec![filter("user", "equals_exact", "a;b:c=d,e")],
        vec![filter("timestamp", "gte", "1700000000001.5")],
        vec![filter("timestamp", "gte", "0")],
        vec![filter("timestamp", "empty", "")],
        vec![filter("timestamp", "not_empty", "")],
        vec![filter("_all", "query", "timestamp:>=0")],
        vec![filter("_all", "query", "timestamp:0..1")],
        vec![Filter {
            value2: Some("1700000000003".into()),
            ..filter("timestamp", "between", "1700000000001")
        }],
        vec![filter(
            "_all",
            "query",
            "worker AND (source:api-a OR source:API-B)",
        )],
        vec![filter("_all", "query", "NOT worker OR user:CAROL")],
        vec![filter(
            "_all",
            "query",
            "source:API-A OR message:/^prefix.*end$/",
        )],
        vec![filter("_all", "query", "@user=\" Alice \"")],
        vec![filter("_all", "query", "user:(bob OR CAROL)")],
        vec![filter("_all", "query", "status:500 AND path:/api/*")],
        vec![filter(
            "_all",
            "query",
            "nested.key:\"Escaped \\\"phrase\\\"\"",
        )],
        vec![filter("_all", "query", "empresa")],
        vec![filter("_all", "query", "message:/start/")],
        vec![filter("_all", "query", "message:/^START$/")],
        vec![filter("_all", "contains", "absent-marker")],
        vec![filter("_all", "contains", "worker")],
        vec![
            filter("source", "equals", "API-A"),
            filter("message", "regex", "(?i)worker"),
        ],
    ];
    for filters in &cases {
        query::clear_match_cache();
        idx.big_data = None;
        let baseline = query::indexed_matches(&idx, filters, &codes, &codes, &[]);
        idx.big_data = Some(engine.clone());
        query::clear_match_cache();
        let accelerated = query::indexed_matches(&idx, filters, &codes, &codes, &[]);
        let expected = query::filtered_indices(&events, filters);
        assert_eq!(
            accelerated,
            expected,
            "event parity: {}",
            serde_json::to_string(filters).unwrap()
        );
        assert_eq!(
            accelerated,
            baseline,
            "normal parity: {}",
            serde_json::to_string(filters).unwrap()
        );
        let mut visited = Vec::new();
        query::visit_indexed_matches(&idx, filters, &codes, &codes, &[], |i| visited.push(i));
        assert_eq!(visited, expected);
        idx.big_data = None;
        let normal_stats = query::stats_indexed(&idx, filters, &codes, &codes, &[]);
        idx.big_data = Some(engine.clone());
        let fast_stats = query::stats_indexed(&idx, filters, &codes, &codes, &[]);
        assert_eq!(
            normalized(serde_json::to_value(fast_stats).unwrap()),
            normalized(serde_json::to_value(normal_stats).unwrap()),
            "stats parity: {filters:?}"
        );
        for sort in ["timestamp", "source", "code", "bytes", ""] {
            for direction in ["asc", "desc"] {
                idx.big_data = None;
                query::clear_match_cache();
                let normal = query::explore_indexed(
                    &idx,
                    filters,
                    sort,
                    direction,
                    1,
                    2,
                    &codes,
                    &codes,
                    &[],
                );
                idx.big_data = Some(engine.clone());
                query::clear_match_cache();
                let fast = query::explore_indexed(
                    &idx,
                    filters,
                    sort,
                    direction,
                    1,
                    2,
                    &codes,
                    &codes,
                    &[],
                );
                assert_eq!(
                    normalized(serde_json::to_value(fast).unwrap()),
                    normalized(serde_json::to_value(normal).unwrap())
                );
            }
        }
    }
    for i in 0..events.len() {
        assert_eq!(
            serde_json::to_value(sources::event_at(&idx, i, &codes, &codes, &[])).unwrap(),
            serde_json::to_value(&events[i]).unwrap()
        );
    }
    let specs = vec![
        query::AggSpec {
            func: "sum".into(),
            column: "bytes".into(),
            alias: "sum".into(),
        },
        query::AggSpec {
            func: "count_distinct".into(),
            column: "user".into(),
            alias: "users".into(),
        },
    ];
    idx.big_data = None;
    let normal = query::aggregate_indexed(&idx, &[], "source", &specs, &codes, &codes, &[]);
    idx.big_data = Some(engine);
    let fast = query::aggregate_indexed(&idx, &[], "source", &specs, &codes, &codes, &[]);
    assert_eq!(
        serde_json::to_value(fast).unwrap(),
        serde_json::to_value(normal).unwrap()
    );
}

#[test]
fn big_data_explorer_agrees_across_supported_log_formats() {
    let cache = tempfile::tempdir().unwrap();
    let codes = CodesConfig::default();
    for name in [
        "exemplo.log",
        "exemplo.jsonl",
        "exemplo-apache.log",
        "exemplo-syslog.log",
        "exemplo-firewall.log",
        "exemplo-java.log",
        "exemplo-cef.log",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../exemplos")
            .join(name);
        let mut idx =
            sources::index_file(path.to_str().unwrap(), "auto", None, None, None).unwrap();
        let normal =
            query::explore_indexed(&idx, &[], "timestamp", "asc", 0, 100, &codes, &codes, &[]);
        idx.big_data = Some(Arc::new(
            BigDataIndex::open_or_build(&idx, &codes, &codes, &[], cache.path(), None).unwrap(),
        ));
        let fast =
            query::explore_indexed(&idx, &[], "timestamp", "asc", 0, 100, &codes, &codes, &[]);
        assert_eq!(
            normalized(serde_json::to_value(fast).unwrap()),
            normalized(serde_json::to_value(normal).unwrap()),
            "{name}"
        );
    }
}

#[test]
fn regex_prefilter_keeps_empty_alternatives_and_unicode_safe() {
    assert!(query::regex_index_predicate("(?:rare|x)").is_none());
    assert!(query::regex_index_predicate("rare?").is_some());
    assert!(query::regex_index_predicate("(?:rare)?").is_none());
    assert!(query::regex_index_predicate("ΟΣ").is_none());
    assert!(query::regex_index_predicate("[0-9]+$").is_none());
    assert!(query::regex_index_predicate("^rare.*marker$").is_some());
}

#[test]
fn big_data_counts_and_stats_preserve_snapshot_metadata_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("snapshot.jsonl");
    let mut event = crate::model::Event::empty();
    event.level = "CustomSeverity".into();
    event.message = "snapshot level".into();
    event.source = "service-zero".into();
    event.timestamp = Some(0);
    std::fs::write(&path, serde_json::to_string(&event).unwrap()).unwrap();
    let mut idx =
        sources::index_file(path.to_str().unwrap(), "snapshot", None, None, None).unwrap();
    let codes = CodesConfig::default();
    let engine = Arc::new(
        BigDataIndex::open_or_build(
            &idx,
            &codes,
            &codes,
            &[],
            &folder.path().join("cache"),
            None,
        )
        .unwrap(),
    );
    for column in ["*", "message"] {
        let specs = [query::AggSpec {
            func: "count".into(),
            column: column.into(),
            alias: "n".into(),
        }];
        idx.big_data = None;
        let normal = query::aggregate_indexed(&idx, &[], "level", &specs, &codes, &codes, &[]);
        idx.big_data = Some(engine.clone());
        let fast = query::aggregate_indexed(&idx, &[], "level", &specs, &codes, &codes, &[]);
        assert_eq!(
            serde_json::to_value(fast).unwrap(),
            serde_json::to_value(normal).unwrap()
        );
    }
    for filters in [
        vec![],
        vec![filter("level", "equals_exact", "CustomSeverity")],
        vec![filter("source", "equals_exact", "service-zero")],
        vec![filter("_all", "contains", "snapshot")],
        vec![filter("timestamp", "gte", "0")],
    ] {
        idx.big_data = None;
        let normal = query::stats_indexed(&idx, &filters, &codes, &codes, &[]);
        idx.big_data = Some(engine.clone());
        let fast = query::stats_indexed(&idx, &filters, &codes, &codes, &[]);
        assert_eq!(
            serde_json::to_value(fast).unwrap(),
            serde_json::to_value(normal).unwrap(),
            "{filters:?}"
        );
    }
}

fn timed<T>(mut run: impl FnMut() -> T) -> (T, f64, f64) {
    let start = Instant::now();
    let mut result = run();
    let mut times = vec![start.elapsed().as_secs_f64() * 1000.0];
    for _ in 0..4 {
        let start = Instant::now();
        result = run();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    (result, times[2], times[4])
}

#[test]
fn big_data_selective_candidates_preserve_whitespace_metadata_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("whitespace.jsonl");
    let records = [
        json!({"timestamp":1700000000000i64,"source":"service","code":"200","message":"marker"}),
        json!({"timestamp":1700000000001i64,"source":"service","code":" ","message":"marker"}),
        json!({"timestamp":1700000000002i64,"source":"service","code":"","message":" marker "}),
        json!({"timestamp":1700000000003i64,"source":"service","code":" 200 ","message":"other"}),
        json!({"timestamp":1700000000004i64,"source":"service","code":"200","message":"escaped\\marker"}),
    ];
    std::fs::write(&path, records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    let mut idx = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    let codes = CodesConfig::default();
    let engine = Arc::new(BigDataIndex::open_or_build(&idx, &codes, &codes, &[], &folder.path().join("cache"), None).unwrap());
    let mut filters_to_check = vec![filter("code", "empty", ""), filter("code", "not_empty", "")];
    for column in ["code", "message", "_all"] {
        for operation in ["contains", "not_contains"] {
            for value in [" 200 ", " marker ", "\tmarker\t", " "] {
                filters_to_check.push(filter(column, operation, value));
            }
        }
    }
    for candidate_filter in filters_to_check {
        for with_time in [false, true] {
            let mut filters = vec![candidate_filter.clone()];
            if with_time { filters.push(filter("timestamp", "gte", "1")); }
            idx.big_data = None;
            query::clear_match_cache();
            let normal = query::explore_indexed(&idx, &filters, "source", "desc", 0, 100, &codes, &codes, &[]);
            let normal_stats = query::stats_indexed(&idx, &filters, &codes, &codes, &[]);
            idx.big_data = Some(engine.clone());
            query::clear_match_cache();
            let fast = query::explore_indexed(&idx, &filters, "source", "desc", 0, 100, &codes, &codes, &[]);
            let fast_stats = query::stats_indexed(&idx, &filters, &codes, &codes, &[]);
            assert_eq!(normalized(serde_json::to_value(fast).unwrap()), normalized(serde_json::to_value(normal).unwrap()), "{filters:?}");
            assert_eq!(normalized(serde_json::to_value(fast_stats).unwrap()), normalized(serde_json::to_value(normal_stats).unwrap()), "stats {filters:?}");
        }
    }
}

#[test]
fn big_data_broad_jsonl_filters_retain_raw_metadata_checks() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("metadata.jsonl");
    let mut output = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    for i in 0..6_144 {
        // The common message guarantees >4,096 n-gram candidates even for the
        // space-padded code filter, exercising the broad rather than exact path.
        writeln!(output, "{}", json!({"source":"service", "code":(["200", " ", "", " 200 "][i % 4]),
            "level":"info", "message":"marker 200 marker"})).unwrap();
    }
    output.flush().unwrap();
    drop(output);
    let mut idx = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
    let codes = CodesConfig::default();
    let engine = Arc::new(BigDataIndex::open_or_build(&idx, &codes, &codes, &[], &folder.path().join("cache"), None).unwrap());
    for code_filter in [
        filter("code", "contains", " 200 "),
        filter("code", "not_contains", " 200 "),
        filter("code", "empty", ""),
        filter("code", "not_empty", ""),
        filter("level", "equals", "Informação"),
    ] {
        for with_source in [false, true] {
            let mut filters = vec![code_filter.clone()];
            if with_source { filters.push(filter("source", "not_equals_exact", "missing")); }
            idx.big_data = None;
            query::clear_match_cache();
            let normal = query::indexed_matches(&idx, &filters, &codes, &codes, &[]);
            idx.big_data = Some(engine.clone());
            query::clear_match_cache();
            assert_eq!(query::indexed_matches(&idx, &filters, &codes, &codes, &[]), normal, "{filters:?}");
        }
    }
}

#[test]
fn big_data_broad_projection_filters_and_sorted_pages_preserve_values_and_ties() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("categorical.jsonl");
    // More than 4,096 matches exercises the nonselective path. Snapshot keeps
    // the original whitespace, Unicode and numeric-looking categorical values.
    let values = ["2", "10", "11x", " 2 ", "2 MB", "900KB", "", " ",
        "ÁPI", "ápi", "ΟΣ", "οσ", "Zulu", "zulu", "NaN", "inf"];
    let mut output = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    for i in 0..6_144 {
        let mut event = crate::model::Event::empty();
        event.id = i;
        event.event_ref = format!("categorical:{i}");
        event.source = values[i % values.len()].into();
        event.code = values[(i / values.len()) % values.len()].into();
        event.level = if i % 8 == 0 { "Erro" } else { "Informação" }.into();
        event.message = format!("original {i}");
        writeln!(output, "{}", serde_json::to_string(&event).unwrap()).unwrap();
    }
    output.flush().unwrap();
    drop(output);
    let mut idx = sources::index_file(path.to_str().unwrap(), "snapshot", None, None, None).unwrap();
    let codes = CodesConfig::default();
    let engine = Arc::new(BigDataIndex::open_or_build(&idx, &codes, &codes, &[], &folder.path().join("cache"), None).unwrap());
    for filters in [
        vec![],
        vec![filter("source", "not_equals_exact", "missing")],
        vec![filter("source", "not_contains", "absent")],
        vec![filter("source", "not_in", "zulu,2")],
        vec![filter("source", "regex", ".*")],
        vec![filter("code", "not_empty", "")],
        vec![filter("level", "equals_exact", "Informação")],
        vec![filter("source", "not_equals_exact", "absent"), filter("code", "not_equals_exact", "absent")],
        vec![filter("source", "equals_exact", "ÁPI")],
    ] {
        idx.big_data = None;
        query::clear_match_cache();
        let normal = query::indexed_matches(&idx, &filters, &codes, &codes, &[]);
        idx.big_data = Some(engine.clone());
        query::clear_match_cache();
        assert_eq!(query::indexed_matches(&idx, &filters, &codes, &codes, &[]), normal, "{filters:?}");
        for column in ["source", "code"] {
            for direction in ["asc", "desc"] {
                for (offset, limit) in [(0, 23), (2_500, 23), (6_140, 23), (usize::MAX, 23), (0, 0), (0, usize::MAX)] {
                    if limit == usize::MAX && !filters.is_empty() { continue; }
                    idx.big_data = None;
                    query::clear_match_cache();
                    let normal = query::query_indexed(&idx, &filters, column, direction, offset, limit, &codes, &codes, &[]);
                    idx.big_data = Some(engine.clone());
                    query::clear_match_cache();
                    let fast = query::query_indexed(&idx, &filters, column, direction, offset, limit, &codes, &codes, &[]);
                    assert_eq!(serde_json::to_value(fast).unwrap(), serde_json::to_value(normal).unwrap(),
                        "{filters:?}, {column}, {direction}, {offset}, {limit}");
                }
            }
        }
    }
    query::clear_match_cache();
    for filters in [vec![], vec![filter("source", "not_contains", "absent")]] {
        let cancelled = crate::operations::run(crate::operations::generation(), || {
            crate::operations::cancel();
            query::query_indexed(&idx, &filters, "source", "asc", 0, 23, &codes, &codes, &[])
        });
        assert!(cancelled.is_err());
    }
}

#[test]
#[ignore = "Deterministic Big Data parity/performance benchmark; BIG_DATA_BENCH_EVENTS=100000,1000000"]
fn benchmark_big_data() {
    let counts = std::env::var("BIG_DATA_BENCH_EVENTS").unwrap_or_else(|_| "100000,1000000".into());
    for count in counts.split(',').map(|s| s.parse::<usize>().unwrap()) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("benchmark.jsonl");
        let mut output = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for i in 0..count {
            writeln!(output, "{}", json!({"timestamp":1700000000000i64+i as i64,"source":format!("service-{}", i%1000),"level":if i%100==0 {"error"} else {"info"},"code":if i%100==0 {"500"} else {"200"},"message":if i%10000==0 {"rare-marker-7429 authentication failed"} else {"worker completed normal request"},"user":format!("user-{}",i%5000),"request_id":format!("req-{i:09}"),"bytes":i%10000})).unwrap();
        }
        output.flush().unwrap();
        drop(output);
        let codes = CodesConfig::default();
        let started = Instant::now();
        let mut idx =
            sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let line_index_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let engine = Arc::new(
            BigDataIndex::open_or_build(
                &idx,
                &codes,
                &codes,
                &[],
                &folder.path().join("cache"),
                None,
            )
            .unwrap(),
        );
        println!(
            "BIG_DATA_BUILD {}",
            json!({"events":count,"source_bytes":std::fs::metadata(&path).unwrap().len(),"line_index_ms":line_index_ms,"build_ms":started.elapsed().as_secs_f64()*1000.0,"index":engine.info()})
        );
        for (name, filters) in [
            (
                "rare_substring",
                vec![filter("_all", "contains", "rare-marker-7429")],
            ),
            (
                "absent_substring",
                vec![filter("_all", "contains", "absent-marker-963")],
            ),
            (
                "exact_field",
                vec![filter("source", "equals_exact", "service-742")],
            ),
            (
                "query_language",
                vec![filter("_all", "query", "user:user-742 AND worker")],
            ),
            (
                "literal_regex",
                vec![filter("message", "regex", "rare-marker-7429.*failed")],
            ),
            (
                "time_window",
                vec![Filter {
                    value2: Some((1700000000000i64 + count as i64 / 2 + 100).to_string()),
                    ..filter(
                        "timestamp",
                        "between",
                        &(1700000000000i64 + count as i64 / 2).to_string(),
                    )
                }],
            ),
            (
                "broad_substring",
                vec![filter("_all", "contains", "worker")],
            ),
        ] {
            idx.big_data = None;
            let (normal, normal_ms, normal_p95) = timed(|| {
                query::clear_match_cache();
                query::indexed_matches(&idx, &filters, &codes, &codes, &[])
            });
            idx.big_data = Some(engine.clone());
            let (fast, fast_ms, fast_p95) = timed(|| {
                query::clear_match_cache();
                query::indexed_matches(&idx, &filters, &codes, &codes, &[])
            });
            assert_eq!(fast, normal, "{name}");
            println!(
                "BIG_DATA_QUERY {}",
                json!({"events":count,"query":name,"matches":fast.len(),"normal_median_ms":normal_ms,"big_data_median_ms":fast_ms,"normal_p95_ms":normal_p95,"big_data_p95_ms":fast_p95,"speedup":normal_ms/fast_ms,"parity":true,"cache":"OS warm; selection cache cleared every trial"})
            );
        }
        idx.big_data = None;
        let (normal, normal_ms, _) = timed(|| {
            query::explore_indexed(&idx, &[], "timestamp", "asc", 0, 100, &codes, &codes, &[])
        });
        idx.big_data = Some(engine.clone());
        let (fast, fast_ms, _) = timed(|| {
            query::explore_indexed(&idx, &[], "timestamp", "asc", 0, 100, &codes, &codes, &[])
        });
        assert_eq!(
            normalized(serde_json::to_value(fast).unwrap()),
            normalized(serde_json::to_value(normal).unwrap())
        );
        println!(
            "BIG_DATA_QUERY {}",
            json!({"events":count,"query":"unfiltered_explorer_facets","normal_median_ms":normal_ms,"big_data_median_ms":fast_ms,"speedup":normal_ms/fast_ms,"parity":true})
        );
        if std::env::var_os("BIG_DATA_REVIEW_CASES").is_some() {
            let segments: std::collections::HashSet<_> = (0..count)
                .filter_map(|row| engine.projection_key(row, [true, false, false]).map(|key| key.0)).collect();
            for (name, filters) in [
                ("broad_source_prefix", vec![filter("source", "starts_with", "service-")]),
                ("broad_code_exact", vec![filter("code", "equals_exact", "200")]),
                ("negative_source", vec![filter("source", "not_equals_exact", "service-742")]),
            ] {
                idx.big_data = None;
                let (normal, normal_ms, normal_p95) = timed(|| {
                    query::clear_match_cache();
                    query::indexed_matches(&idx, &filters, &codes, &codes, &[])
                });
                idx.big_data = Some(engine.clone());
                let (fast, fast_ms, fast_p95) = timed(|| {
                    query::clear_match_cache();
                    query::indexed_matches(&idx, &filters, &codes, &codes, &[])
                });
                assert_eq!(fast, normal, "{name}");
                println!("BIG_DATA_REVIEW {}", json!({"events":count,"query":name,"matches":fast.len(),"segments":segments.len(),"normal_median_ms":normal_ms,"big_data_median_ms":fast_ms,"normal_p95_ms":normal_p95,"big_data_p95_ms":fast_p95,"parity":true}));
            }
            for (name, column, direction, offset) in [
                ("sort_source_first_page", "source", "asc", 0),
                ("sort_code_first_page", "code", "desc", 0),
                ("sort_source_deep_page", "source", "desc", count / 2),
            ] {
                idx.big_data = None;
                let normal = query::query_indexed(&idx, &[], column, direction, offset, 100, &codes, &codes, &[]);
                idx.big_data = Some(engine.clone());
                let (fast, fast_ms, fast_p95) = timed(|| query::query_indexed(&idx, &[], column, direction, offset, 100, &codes, &codes, &[]));
                assert_eq!(serde_json::to_value(fast).unwrap(), serde_json::to_value(normal).unwrap(), "{name}");
                println!("BIG_DATA_REVIEW {}", json!({"events":count,"query":name,"segments":segments.len(),"offset":offset,"limit":100,"big_data_median_ms":fast_ms,"big_data_p95_ms":fast_p95,"parity":true}));
            }
        }
        let started = Instant::now();
        let reopened = BigDataIndex::open_or_build(
            &idx,
            &codes,
            &codes,
            &[],
            &folder.path().join("cache"),
            None,
        )
        .unwrap();
        assert!(reopened.info().reused);
        println!(
            "BIG_DATA_REOPEN {}",
            json!({"events":count,"reopen_ms":started.elapsed().as_secs_f64()*1000.0,"reused":true})
        );
        if std::env::var("BIG_DATA_BENCH_KEEP").is_ok_and(|value| value == "1") {
            let retained = folder.keep();
            println!("BIG_DATA_RETAINED {}", json!({"events":count,"path":retained}));
        }
    }
}

#[test]
#[ignore = "Warm selection cache/page benchmark; QUERY_CACHE_BENCH_EVENTS=100000,1000000; QUERY_CACHE_REFERENCE=1"]
fn benchmark_query_cached_pages() {
    use sha2::{Digest, Sha256};
    let counts = std::env::var("QUERY_CACHE_BENCH_EVENTS").unwrap_or_else(|_| "100000,1000000".into());
    let reference = std::env::var("QUERY_CACHE_REFERENCE").ok().as_deref() == Some("1");
    for count in counts.split(',').map(|value| value.parse::<usize>().unwrap().max(1)) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("cached-pages.jsonl");
        let mut output = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for i in 0..count {
            writeln!(output, "{}", json!({"timestamp":1700000000000i64+i as i64,"level":"info","source":"worker","code":"200","message":"completed request"})).unwrap();
        }
        output.flush().unwrap();
        drop(output);
        let idx = sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        let filters = [filter("level", "equals", "Informação")];
        query::clear_match_cache();
        for (scenario, offset, limit) in [("count_only", 0, 0), ("first_page", 0, 25), ("deep_page", count - 1, 25)] {
            let run = || if reference {
                query::query_indexed_owned_reference(&idx, &filters, "", "asc", offset, limit, &codes, &codes, &[])
            } else {
                query::query_indexed(&idx, &filters, "", "asc", offset, limit, &codes, &codes, &[])
            };
            let expected = serde_json::to_value(query::query_indexed_owned_reference(
                &idx, &filters, "", "asc", offset, limit, &codes, &codes, &[])).unwrap();
            assert_eq!(serde_json::to_value(query::query_indexed(
                &idx, &filters, "", "asc", offset, limit, &codes, &codes, &[])).unwrap(), expected);
            assert_eq!(expected["total"], count);
            let mut samples = Vec::new();
            for _ in 0..31 {
                let started = Instant::now();
                let actual = std::hint::black_box(run());
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(serde_json::to_value(actual).unwrap(), expected);
            }
            samples.sort_by(f64::total_cmp);
            println!("QUERY_CACHE_REVIEW {}", json!({"events":count,"scenario":scenario,"owned_reference":reference,
                "median_ms":samples[15],"p95_ms":samples[29],"samples":samples.len(),
                "page_rows":expected["rows"].as_array().unwrap().len(),"parity":true,
                "copied_selection_bytes_per_reference_call":count * std::mem::size_of::<usize>(),
                "reference":"same-binary reproduction of the former warm-cache Vec clone, with identical page processing",
                "result_sha256":format!("{:x}", Sha256::digest(serde_json::to_vec(&expected).unwrap()))}));
        }
        query::clear_match_cache();
    }
}
