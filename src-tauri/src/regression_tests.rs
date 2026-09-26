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

fn state_for(source: crate::SourceData) -> crate::AppState {
    crate::AppState {
        source: parking_lot::RwLock::new(source),
        source_names: parking_lot::RwLock::new(vec![]),
        codes: parking_lot::RwLock::new(CodesConfig::default()),
        system_codes: parking_lot::RwLock::new(CodesConfig::default()),
        derived: parking_lot::RwLock::new(vec![]),
        case_store_lock: parking_lot::Mutex::new(()),
        codes_path: PathBuf::new(),
        system_codes_path: PathBuf::new(),
    }
}

#[test]
fn mixed_numeric_sort_is_transitive_and_matches_indexed_units() {
    let values = ["11x", "10", "2", "1s", "900ms", "NaN", "-3", "z"];
    let text = values
        .iter()
        .map(|value| serde_json::json!({"mixed":value}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let fixture = Fixture::new(&text);
    let index = fixture.index("jsonl");
    let codes = CodesConfig::default();
    let events: Vec<_> = (0..index.lines.len())
        .map(|i| sources::event_at(&index, i, &codes, &codes, &[]))
        .collect();
    for direction in ["asc", "desc"] {
        let memory = query::query(&events, &[], "mixed", direction, 0, 100);
        let indexed =
            query::query_indexed(&index, &[], "mixed", direction, 0, 100, &codes, &codes, &[]);
        let values = |rows: &[Event]| {
            rows.iter()
                .map(|e| e.col_str("mixed").unwrap())
                .collect::<Vec<_>>()
        };
        let mut expected = vec!["-3", "2", "10", "900ms", "1s", "11x", "NaN", "z"];
        if direction == "desc" {
            expected.reverse();
        }
        assert_eq!(values(&memory.rows), expected);
        assert_eq!(values(&indexed.rows), expected);
    }
    assert!(analysis::parse_num_unit(&format!("{}TB", "9".repeat(300))).is_none());
    let mut invalid = Event::empty();
    invalid.fields.insert("number".into(), "NaN".into());
    assert!(invalid.col_num("number").is_none());
}

#[test]
fn categories_keep_literal_empty_label_separate_from_missing() {
    let mut events = Vec::new();
    for value in [
        None,
        Some(""),
        Some(" "),
        Some("(vazio)"),
        Some("API"),
        Some("api"),
        Some(" API "),
    ] {
        let mut event = Event::empty();
        if let Some(value) = value {
            event.fields.insert("category".into(), value.into());
        }
        events.push(event);
    }
    let specs = vec![query::AggSpec {
        func: "count".into(),
        column: "*".into(),
        alias: "n".into(),
    }];
    let grouped = query::aggregate(&events, &[], "category", &specs);
    assert_eq!(grouped.rows.len(), 5);
    for (value, row) in grouped.group_values.iter().zip(&grouped.rows) {
        let selected = query::filtered_indices(
            &events,
            &[filter(
                "category",
                if value.is_some() {
                    "equals_exact"
                } else {
                    "empty"
                },
                value.as_deref().unwrap_or(""),
            )],
        );
        assert_eq!(row["n"], serde_json::json!(selected.len()));
    }
    let series = serde_json::to_value(analysis::compute_series(
        &events,
        &serde_json::from_value(
            serde_json::json!({"chart":"terms","metric":"count","field":"category","limit":10}),
        )
        .unwrap(),
    ))
    .unwrap();
    assert_eq!(series["x_values"].as_array().unwrap().len(), 5);
    assert!(series["x_values"]
        .as_array()
        .unwrap()
        .contains(&serde_json::Value::Null));
    assert!(series["x_values"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("(vazio)")));
}

#[test]
fn pivot_uses_exact_tuples_and_rejects_mixed_numeric_units() {
    let mut events = Vec::new();
    for (r1, r2, c1, c2, latency) in [
        ("a\u{1f}b", "c", "x → y", "z", "1s"),
        ("a", "b\u{1f}c", "x", "y → z", "10MB"),
    ] {
        let mut event = Event::empty();
        event.timestamp = Some(100);
        for (field, value) in [
            ("r1", r1),
            ("r2", r2),
            ("c1", c1),
            ("c2", c2),
            ("latency", latency),
        ] {
            event.fields.insert(field.into(), value.into());
        }
        events.push(event);
    }
    let spec=serde_json::from_value(serde_json::json!({"rows":["r1","r2"],"cols":["c1","c2"],"values":[{"func":"count","column":"*","alias":"n"},{"func":"avg","column":"latency","alias":"avg"},{"func":"min","column":"timestamp","alias":"start"}]})).unwrap();
    let result = serde_json::to_value(analysis::pivot(&events, &spec)).unwrap();
    assert_eq!(result["row_values"].as_array().unwrap().len(), 4);
    assert_eq!(result["col_values"].as_array().unwrap().len(), 2);
    assert_ne!(result["col_values"][0], result["col_values"][1]);
    assert_eq!(result["incompatible_units"], serde_json::json!([0, 1, 0]));
    for total in result["totals"].as_array().unwrap() {
        assert_eq!(total[0], 1);
        assert!(total[1].is_null());
        assert_eq!(total[2], 100.0);
    }
    let leaf_count = result["row_values"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, p)| p.as_array().unwrap().len() == 2)
        .map(|(i, _)| {
            result["cells"][i]
                .as_array()
                .unwrap()
                .iter()
                .map(|cell| cell[0].as_u64().unwrap_or(0))
                .sum::<u64>()
        })
        .sum::<u64>();
    assert_eq!(leaf_count, 2);
}

#[test]
fn pivot_exactly_two_hundred_columns_does_not_drop_later_records() {
    let mut events = Vec::new();
    for i in 0..400 {
        let mut event = Event::empty();
        event
            .fields
            .insert("column".into(), format!("c{}", i % 200).into());
        events.push(event);
    }
    let spec=serde_json::from_value(serde_json::json!({"rows":[],"cols":["column"],"values":[{"func":"count","column":"*","alias":"n"}]})).unwrap();
    let result = serde_json::to_value(analysis::pivot(&events, &spec)).unwrap();
    assert_eq!(result["complete"], true);
    assert_eq!(result["processed_events"], 400);
    assert!(result["totals"]
        .as_array()
        .unwrap()
        .iter()
        .all(|total| total[0] == 2));
}

#[test]
fn parallel_facets_inherit_cancellation_and_nested_operations_restore_token() {
    let generation = crate::operations::generation();
    let result = crate::operations::run(generation, || {
        crate::operations::run(generation, || {}).unwrap();
        assert_eq!(crate::operations::current_generation(), Some(generation));
        crate::operations::cancel();
        let events = vec![Event::empty(); 10];
        let columns = vec!["level".to_string(); 8];
        assert!(query::multi_count(&events, &[], &columns)
            .iter()
            .all(|(_, result)| result.rows.is_empty()));
    });
    assert!(result.is_err());
    assert!(crate::operations::current_generation().is_none());
}

#[test]
fn discovery_category_counts_round_trip_with_exact_filters() {
    let mut events = Vec::new();
    for value in ["API", "api", " API "] {
        for _ in 0..30 {
            let mut event = Event::empty();
            event.fields.insert("component".into(), value.into());
            events.push(event);
        }
    }
    let state = state_for(crate::SourceData::Memory(events.clone()));
    let result = serde_json::to_value(crate::discover_patterns_impl(&state, vec![], None)).unwrap();
    let category = result["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|category| category["field"] == "component")
        .unwrap();
    assert_eq!(category["distinct"], 3);
    let dominant = category["dominant"]["value"].as_str().unwrap();
    assert_eq!(
        query::filtered_indices(&events, &[filter("component", "equals_exact", dominant)]).len(),
        category["dominant"]["count"].as_u64().unwrap() as usize
    );
}

#[test]
fn grouped_numeric_measures_do_not_compare_incompatible_units() {
    let fixture=Fixture::new("{\"group\":\"first\",\"size\":\"1s\"}\n{\"group\":\"first\",\"size\":\"500ms\"}\n{\"group\":\"second\",\"size\":\"1MB\"}\n");
    let index = fixture.index("jsonl");
    let codes = CodesConfig::default();
    let events: Vec<_> = (0..index.lines.len())
        .map(|i| sources::event_at(&index, i, &codes, &codes, &[]))
        .collect();
    let specs = vec![
        query::AggSpec {
            func: "count".into(),
            column: "*".into(),
            alias: "n".into(),
        },
        query::AggSpec {
            func: "avg".into(),
            column: "size".into(),
            alias: "mean".into(),
        },
    ];
    let memory = query::aggregate(&events, &[], "group", &specs);
    let indexed = query::aggregate_indexed(&index, &[], "group", &specs, &codes, &codes, &[]);
    assert_eq!(memory.incompatible_units, vec![0, 1]);
    assert_eq!(memory.value_units, vec!["", "duration"]);
    assert!(memory.rows.iter().all(|row| row["mean"].is_null()));
    assert_eq!(
        serde_json::to_value(memory).unwrap(),
        serde_json::to_value(indexed).unwrap()
    );
    let compatible = query::aggregate(
        &events,
        &[filter("group", "equals_exact", "first")],
        "group",
        &specs,
    );
    assert_eq!(compatible.incompatible_units, vec![0, 0]);
    assert_eq!(compatible.rows[0]["mean"], 750.0);
}

#[test]
fn case_scope_queries_overview_comparison_timeline_and_export_are_isolated() {
    let mut dataset = Event::empty();
    dataset.id = 9;
    dataset.timestamp = Some(50);
    dataset.source = "Dataset".into();
    dataset.message = "dataset only".into();
    let state = state_for(crate::SourceData::Memory(vec![dataset]));
    let mut case = Vec::new();
    for (id, timestamp, level) in [
        (31, Some(0), "Erro"),
        (72, Some(100), "Aviso"),
        (95, None, "Informação"),
    ] {
        let mut event = Event::empty();
        event.id = id;
        event.timestamp = timestamp;
        event.source = "Caso".into();
        event.level = level.into();
        event.event_ref = format!("saved:{id}");
        event.message = format!("saved event {id}");
        event.raw = "preserved raw".into();
        event
            .fields
            .insert("password".into(), "secret-example".into());
        case.push(event);
    }
    let filters = vec![filter("source", "equals_exact", "Caso")];
    let overview = workspace::overview_scope_impl(&state, filters.clone(), Some(&case)).unwrap();
    assert_eq!(
        (
            overview.total,
            overview.errors,
            overview.warnings,
            overview.undated
        ),
        (3, 1, 1, 1)
    );
    assert_eq!((overview.start, overview.end), (Some(0), Some(100)));
    assert_eq!(
        workspace::overview_scope_impl(&state, vec![], Some(&[]))
            .unwrap()
            .total,
        0
    );
    assert_eq!(workspace::overview_impl(&state, vec![]).unwrap().total, 1);
    let page =
        crate::query_events_scope_impl(&state, filters.clone(), "id", "desc", 1, 1, Some(&case));
    assert_eq!(page.total, 3);
    assert_eq!(page.rows[0].id, 72);
    assert_eq!(page.rows[0].event_ref, "saved:72");
    assert!(
        crate::query_events_scope_impl(&state, vec![], "id", "asc", 0, 100, Some(&[]))
            .rows
            .is_empty()
    );
    let explore = crate::explore_snapshot_scope_impl(
        &state,
        filters.clone(),
        "id",
        "asc",
        0,
        10,
        None,
        Some(&case),
    );
    assert_eq!(explore.query.total, 3);
    assert_eq!(explore.query.rows[0].id, 31);
    assert_eq!(
        explore
            .stats
            .buckets
            .iter()
            .map(|(_, count)| count)
            .sum::<i64>(),
        2
    );
    assert_eq!(explore.stats.buckets[0].0, 0);
    assert!(explore
        .stats
        .buckets
        .iter()
        .all(|(timestamp, _)| *timestamp <= 100));
    assert_eq!(
        explore
            .stats
            .levels
            .iter()
            .map(|(_, count)| count)
            .sum::<i64>(),
        3
    );
    let range = serde_json::to_value(
        workspace::timeline_range_scope_impl(&state, filters.clone(), 0, 100, 240, Some(&case))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(range["total"], 2);
    assert_eq!(range["errors"], 1);
    assert_eq!(range["warnings"], 1);
    assert!(range["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|bucket| bucket["timestamp"].as_i64().unwrap() <= 100));
    let comparison = workspace::compare_scope_impl(
        &state,
        filters.clone(),
        insights::Period { start: 0, end: 49 },
        insights::Period {
            start: 50,
            end: 150,
        },
        Some(&case),
    )
    .unwrap();
    assert_eq!(
        (
            comparison.before_total,
            comparison.after_total,
            comparison.before_errors
        ),
        (1, 1, 1)
    );
    let prepared = query::prepare(&filters);
    let mut jsonl = Vec::new();
    assert_eq!(
        workspace::write_events_export(&mut jsonl, "jsonl", true, || case
            .iter()
            .filter(|event| prepared.iter().all(|filter| query::matches(event, filter)))
            .cloned())
        .unwrap(),
        3
    );
    let exported: Vec<serde_json::Value> = std::str::from_utf8(&jsonl)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(exported[0]["event_ref"], "saved:31");
    assert_eq!(exported[0]["raw"], "preserved raw");
    assert_eq!(exported[0]["fields"]["password"], "[oculto]");
    assert_eq!(case[0].fields["password"], "secret-example");
    assert_eq!(
        crate::query_events_impl(&state, vec![], "id", "asc", 0, 10).rows[0].source,
        "Dataset"
    );
}

#[test]
fn json_arrays_preserve_nested_fields_strings_and_epoch_units() {
    let file = Fixture::new("\u{feff}[\n {\"Timestamp\":\"1706745600000000\",\"service\":{\"name\":\"payments\"},\"log\":{\"level\":\"ERROR\"},\"http\":{\"response\":{\"status_code\":503}},\"message\":\"braces } ] and \\\"quotes\\\"\",\"tags\":[\"a\",\"b\"]},\n {\"timestamp\":1706745600000000000,\"message\":\"ok\"}\n]\n");
    let index = file.index("auto");
    assert_eq!(index.lines.len(), 2);
    let events = materialize(&index);
    assert_eq!(events[0].source, "payments");
    assert_eq!(events[0].level, "Erro");
    assert_eq!(events[0].timestamp, Some(1706745600000));
    assert_eq!(events[1].timestamp, events[0].timestamp);
    assert_eq!(events[0].fields["http.response.status_code"], 503);
    assert_eq!(events[0].fields["service"], "payments");
    assert_eq!(events[0].fields["tags"], serde_json::json!(["a", "b"]));
    assert_eq!(events[0].message, "braces } ] and \"quotes\"");
    assert!(events.iter().all(|e| e.parse_status == "parsed"));
    assert!(index.columns.contains(&"http.response.status_code".into()));
    assert_ne!(events[0].event_ref, events[1].event_ref);
}

#[test]
fn json_array_rejects_incomplete_input_and_preserves_literal_dotted_fields() {
    for text in [
        "[{\"message\":\"ok\"}",
        "[{\"message\":\"ok\"},]",
        "[{\"message\":\"ok\"}] garbage",
    ] {
        let file = Fixture::new(text);
        assert!(sources::index_file(file.0.to_str().unwrap(), "auto", None, None, None).is_err());
    }
    let ev = sources::parse_line(br#"{"service":{"name":"nested"},"service.name":"explicit","level":null,"severity":"WARN","message":"hello"}"#, "jsonl", None, &[]);
    assert_eq!(ev.source, "explicit");
    assert_eq!(ev.level, "Aviso");
    assert_eq!(ev.fields["service.name"], "explicit");
}

#[test]
fn discovery_respects_case_filters_and_matches_indexed_storage() {
    let mut text = String::new();
    for i in 0..100 {
        let region = if i < 30 { "south" } else { "north" };
        let status = if i < 25 { "failed" } else { "ok" };
        text.push_str(&format!("{{\"source\":\"api\",\"region\":\"{region}\",\"status\":\"{status}\",\"latency\":\"{}ms\",\"message\":\"request {} finished\"}}\n", if i == 99 {900} else {10}, i));
    }
    let file = Fixture::new(&text);
    let index = file.index("auto");
    let events = materialize(&index);
    let state = state_for(crate::SourceData::Indexed(index));
    let indexed = crate::discover_patterns_impl(&state, vec![], None);
    assert_eq!(indexed.total, 100);
    assert_eq!(indexed.sample_count, 100);
    assert!(!indexed.limited);
    assert!(indexed.complete);
    assert_eq!(indexed.templates[0].count, 100);
    assert_eq!(indexed.outliers[0].outlier_count, 1);
    assert_eq!(indexed.outliers[0].examples[0].event_id, 99);
    assert_eq!(indexed.outliers[0].unit, "ms");
    let association = indexed
        .associations
        .iter()
        .find(|a| a.left.field == "status" && a.left.value == "failed")
        .unwrap();
    assert_eq!(association.right.value, "south");
    assert!((association.confidence - 1.0).abs() < 1e-9);
    assert!((association.lift - 100.0 / 30.0).abs() < 1e-9);
    let memory = crate::discover_patterns_impl(
        &state_for(crate::SourceData::Memory(events.clone())),
        vec![],
        None,
    );
    assert_eq!(
        serde_json::to_value(indexed).unwrap(),
        serde_json::to_value(memory).unwrap()
    );
    let case = crate::discover_patterns_impl(
        &state,
        vec![filter("region", "equals", "south")],
        Some(events),
    );
    assert_eq!(case.total, 30);
    assert_eq!(case.sample_count, 30);
    assert!(case.outliers.is_empty());
}

#[test]
fn discovery_sample_is_bounded_repeatable_and_reaches_file_tail() {
    let mut first = crate::discovery::Sampler::new();
    let mut second = crate::discovery::Sampler::new();
    for i in 0..120_000 {
        first.push(i);
        second.push(i);
    }
    assert_eq!(first.ids, second.ids);
    assert_eq!(first.ids.len(), 6000);
    for tenth in 0..10 {
        let count = first.ids.iter().filter(|&&i| i / 12_000 == tenth).count();
        assert!(
            (400..800).contains(&count),
            "unrepresentative decile {tenth}: {count}"
        );
    }
}

#[test]
fn discoveries_suppress_near_aliases_and_diversify_field_pairs() {
    let events = (0..400)
        .map(|i| {
            let mut ev = Event::empty();
            ev.code = format!("c{}", i % 8);
            ev.fields.insert(
                "status".into(),
                if i == 0 {
                    "different".into()
                } else {
                    ev.code.clone().into()
                },
            );
            ev.fields
                .insert("region".into(), format!("r{}", i % 4).into());
            ev.fields
                .insert("phase".into(), format!("p{}", i % 2).into());
            ev
        })
        .collect();
    let result =
        crate::discover_patterns_impl(&state_for(crate::SourceData::Memory(events)), vec![], None);
    assert!(!result.associations.is_empty());
    let mut pairs = std::collections::BTreeMap::<_, usize>::new();
    for association in result.associations {
        let mut pair = [association.left.field, association.right.field];
        pair.sort();
        assert_ne!(pair, ["code", "status"]);
        assert!(
            !pair.iter().any(|field| field == "status"),
            "Near-alias must not generate duplicate relations with other fields"
        );
        *pairs.entry(pair).or_default() += 1;
    }
    assert!(pairs.len() >= 3);
    assert!(pairs.values().all(|count| *count <= 2));
}

#[test]
fn time_series_ends_at_last_observed_bucket_without_false_zero() {
    let mut events = vec![Event::empty(); 3];
    for (event, timestamp) in events.iter_mut().zip([1000, 1999, 2000]) {
        event.timestamp = Some(timestamp);
    }
    let spec = analysis::SeriesSpec {
        chart: "time".into(),
        metric: "count".into(),
        field: None,
        interval_ms: Some(1000),
        split: None,
        limit: None,
        unit: None,
    };
    let result = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(result["x"], serde_json::json!([1000, 2000]));
    assert_eq!(result["series"][0]["points"], serde_json::json!([2.0, 1.0]));
    let result = serde_json::to_value(analysis::compute_series(&events[..1], &spec)).unwrap();
    assert_eq!(result["x"], serde_json::json!([1000]));
    assert_eq!(result["series"][0]["points"], serde_json::json!([1.0]));
}

#[test]
fn terms_count_is_exact_beyond_memory_budget_and_counts_missing_values() {
    let spec = analysis::SeriesSpec {
        chart: "terms".into(),
        metric: "count".into(),
        field: Some("message".into()),
        interval_ms: None,
        split: None,
        limit: Some(3),
        unit: Some("auto".into()),
    };
    let result = analysis::compute_series_stream(
        || {
            (0..30_006).map(|i| {
                let mut ev = Event::empty();
                ev.message = if i >= 30_004 {
                    String::new()
                } else if i >= 30_000 {
                    "late winner".into()
                } else {
                    format!("event-{i:05}")
                };
                ev
            })
        },
        &spec,
    );
    let result = serde_json::to_value(result).unwrap();
    assert_eq!(
        result["x"],
        serde_json::json!(["late winner", "(vazio)", "event-00000"])
    );
    assert_eq!(
        result["series"][0]["points"],
        serde_json::json!([4.0, 2.0, 1.0])
    );
    assert_eq!(result["unit"], "number");
}

fn contextual_fixture(incident: bool, timestamp: bool) -> Vec<Event> {
    let mut events = Vec::new();
    for bin in 0..12 {
        for combination in 0..8 {
            for repetition in 0..6 {
                let mut ev = Event::empty();
                ev.id = events.len();
                ev.event_ref = format!("context-test:{}", ev.id);
                ev.timestamp = timestamp.then_some(1_706_745_600_000 + bin * 1000 + repetition);
                ev.code = if incident && bin == 5 && combination == 7 {
                    "unexpected".into()
                } else {
                    format!("normal-{combination}")
                };
                ev.message = "request finished".into();
                for (field, bit) in [("a", 0), ("b", 1), ("c", 2)] {
                    ev.fields.insert(
                        field.into(),
                        format!("v{}", (combination >> bit) & 1).into(),
                    );
                }
                ev.fields.insert("a_alias".into(), ev.fields["a"].clone());
                ev.fields.insert("constant".into(), "same".into());
                ev.fields
                    .insert("trace_id".into(), format!("trace-{}", ev.id).into());
                events.push(ev);
            }
        }
    }
    events
}

#[test]
fn contextual_change_finds_three_field_incident_in_middle_of_period() {
    let events = contextual_fixture(true, true);
    let result =
        crate::discover_patterns_impl(&state_for(crate::SourceData::Memory(events)), vec![], None);
    let shift = result
        .behavior_shifts
        .iter()
        .find(|s| s.observed == "unexpected")
        .expect("three-field incident");
    assert_eq!(shift.context.len(), 3);
    assert_eq!(shift.expected, "normal-7");
    assert_eq!(shift.outcome_op, "equals_exact");
    assert_eq!(shift.baseline_count, 66);
    assert_eq!(shift.window_observed, 6);
    assert_eq!(shift.expected_share, 1.0);
    assert_eq!(shift.observed_share, 1.0);
    assert_eq!(shift.baseline_observed_share, 0.0);
    assert!(shift.start <= 1_706_745_605_000 && shift.end >= 1_706_745_605_005);
    assert!(shift
        .context
        .iter()
        .all(|item| ["a", "b", "c"].contains(&item.field.as_str())));
    assert!(result.behavior_shifts.len() <= 8);
    assert!(result.changes.is_empty());
}

#[test]
fn stable_contexts_and_undated_data_do_not_flood_temporal_findings() {
    for (incident, timestamp) in [(false, true), (true, false)] {
        let result = crate::discover_patterns_impl(
            &state_for(crate::SourceData::Memory(contextual_fixture(
                incident, timestamp,
            ))),
            vec![],
            None,
        );
        assert!(result.behavior_shifts.is_empty());
        assert!(result.changes.is_empty());
        if !timestamp {
            assert_eq!(result.timed_sample_count, 0);
            assert_eq!(result.time_bins, 0);
        }
    }
}

#[test]
fn truncated_outcomes_do_not_invent_a_dominant_context_baseline() {
    let mut events = Vec::new();
    let mut add = |source: &str, code: String, bin: i64| {
        let mut ev = Event::empty();
        ev.id = events.len();
        ev.event_ref = format!("truncated:{}", ev.id);
        ev.source = source.into();
        ev.code = code;
        ev.timestamp = Some(1_706_745_600_000 + bin * 1000);
        events.push(ev);
    };
    for value in 0..63 {
        for i in 0..5 {
            add("background", format!("frequent-{value}"), (value + i) % 12);
        }
    }
    for i in 0..6 {
        add("background", "incident".into(), i);
    }
    for i in 0..20 {
        add("target", "expected".into(), i % 4);
    }
    for i in 0..27 {
        add("target", format!("unique-{i}"), i % 4);
    }
    for _ in 0..3 {
        add("target", "incident".into(), 5);
    }
    let result =
        crate::discover_patterns_impl(&state_for(crate::SourceData::Memory(events)), vec![], None);
    assert!(result.temporal_limited);
    assert!(!result
        .behavior_shifts
        .iter()
        .any(|s| s.observed == "incident" && s.context.iter().any(|item| item.value == "target")));
}

#[test]
fn changes_expose_new_message_pattern_in_middle_window() {
    let events = (0..120)
        .map(|i| {
            let mut ev = Event::empty();
            ev.id = i;
            ev.event_ref = format!("new-message:{i}");
            ev.timestamp = Some(1_706_745_600_000 + (i / 10) as i64 * 1000 + (i % 10) as i64);
            ev.message = if (50..60).contains(&i) {
                "queue stalled"
            } else {
                "request completed"
            }
            .into();
            ev
        })
        .collect();
    let result =
        crate::discover_patterns_impl(&state_for(crate::SourceData::Memory(events)), vec![], None);
    let change = result
        .changes
        .iter()
        .find(|s| s.observed == "queue stalled")
        .unwrap();
    assert_eq!(change.kind, "new_pattern");
    assert_eq!(change.outcome_op, "pattern");
    assert_eq!(change.baseline_observed, 0);
    assert_eq!(change.window_observed, 10);
    assert!(change.context.is_empty());
}

#[test]
fn series_skips_incompatible_units_and_reports_their_count() {
    let events = ["10ms", "20ms", "1GB"]
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let mut ev = Event::empty();
            ev.timestamp = Some(1000 + i as i64);
            ev.fields.insert("metric".into(), v.into());
            ev
        })
        .collect::<Vec<_>>();
    let spec = analysis::SeriesSpec {
        chart: "time".into(),
        metric: "avg".into(),
        field: Some("metric".into()),
        interval_ms: Some(1000),
        split: None,
        limit: None,
        unit: Some("auto".into()),
    };
    let result = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(result["unit"], "duration");
    assert_eq!(result["incompatible_units"], 1);
    assert_eq!(result["series"][0]["points"], serde_json::json!([15.0]));
    assert_eq!(result["series"][0]["samples"], serde_json::json!([2]));
}

#[test]
fn sparse_numeric_units_sample_values_instead_of_prefix_records() {
    let mut events = (0..501)
        .map(|i| {
            let mut event = Event::empty();
            event.timestamp = Some(1000 + i);
            event
        })
        .collect::<Vec<_>>();
    events[500].fields.insert("latency".into(), "5ms".into());
    let mut spec = analysis::SeriesSpec {
        chart: "time".into(),
        metric: "avg".into(),
        field: Some("latency".into()),
        interval_ms: Some(1000),
        split: None,
        limit: None,
        unit: Some("auto".into()),
    };
    let result = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(result["unit"], "duration");
    assert_eq!(result["incompatible_units"], 0);
    assert_eq!(result["series"][0]["points"], serde_json::json!([5.0]));
    assert_eq!(result["series"][0]["samples"], serde_json::json!([1]));
    spec.metric = "count".into();
    spec.field = None;
    let count = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(count["unit"], "number");
    assert_eq!(count["series"][0]["points"], serde_json::json!([501.0]));

    // Ties must resolve consistently between separately computed metrics.
    events[499].fields.insert("latency".into(), "5".into());
    spec.field = Some("latency".into());
    for metric in ["min", "max", "avg", "sum"] {
        spec.metric = metric.into();
        let result = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
        assert_eq!(result["unit"], "number");
        assert_eq!(result["incompatible_units"], 1);
    }
}

#[test]
fn exact_value_filters_preserve_case_whitespace_and_empty_literals() {
    let values = ["API", "api", " API ", "(vazio)", "", "ÁPI"];
    let mut text = values
        .iter()
        .map(|value| serde_json::json!({"category":value,"message":value,"code":value}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    text.push_str("\n{\"message\":\"missing\"}\n");
    // Same decoded string as row 0, through escaped JSON text.
    text.push_str(
        "{\"category\":\"\\u0041PI\",\"message\":\"\\u0041PI\",\"code\":\"\\u0041PI\"}\n",
    );
    let fixture = Fixture::new(&text);
    let index = fixture.index("jsonl");
    let events = materialize(&index);
    let empty = CodesConfig::default();
    for (value, expected) in [
        ("API", vec![0, 7]),
        ("api", vec![1]),
        (" API ", vec![2]),
        ("(vazio)", vec![3]),
        ("", vec![4]),
        ("ÁPI", vec![5]),
        ("ápi", vec![]),
    ] {
        let exact = vec![filter("category", "equals_exact", value)];
        assert!(workspace::validate(&exact).is_ok());
        assert_eq!(
            query::filtered_indices(&events, &exact),
            expected,
            "memory {value:?}"
        );
        assert_eq!(
            query::indexed_matches(&index, &exact, &empty, &empty, &[]),
            expected,
            "index {value:?}"
        );
        let not_exact = vec![filter("category", "not_equals_exact", value)];
        assert!(workspace::validate(&not_exact).is_ok());
        let complement = (0..events.len())
            .filter(|i| !expected.contains(i))
            .collect::<Vec<_>>();
        assert_eq!(query::filtered_indices(&events, &not_exact), complement);
        assert_eq!(
            query::indexed_matches(&index, &not_exact, &empty, &empty, &[]),
            complement
        );
    }
    for column in ["code", "message"] {
        let exact = vec![filter(column, "equals_exact", "API")];
        assert_eq!(query::filtered_indices(&events, &exact), vec![0, 7]);
        assert_eq!(
            query::indexed_matches(&index, &exact, &empty, &empty, &[]),
            vec![0, 7]
        );
    }
    // Existing manual equals keeps its prior case-insensitive/trimmed semantics.
    let legacy = vec![filter("category", "equals", " API ")];
    assert_eq!(query::filtered_indices(&events, &legacy), vec![0, 1, 7]);
    assert_eq!(
        query::indexed_matches(&index, &legacy, &empty, &empty, &[]),
        vec![0, 1, 7]
    );
}

#[test]
fn numeric_series_samples_distinguish_gaps_from_real_zero() {
    let events = [
        (0, Some("-10ms")),
        (2, Some("-5ms")),
        (3, Some("0ms")),
        (4, Some("invalid")),
        (5, Some("1GB")),
        (6, None),
        (7, Some("4ms")),
        (7, Some("6ms")),
    ]
    .into_iter()
    .map(|(second, value)| {
        let mut event = Event::empty();
        event.timestamp = Some(1000 + second * 1000);
        if let Some(value) = value {
            event.fields.insert("metric".into(), value.into());
        }
        event
    })
    .collect::<Vec<_>>();
    let mut spec = analysis::SeriesSpec {
        chart: "time".into(),
        metric: "max".into(),
        field: Some("metric".into()),
        interval_ms: Some(1000),
        split: None,
        limit: None,
        unit: Some("duration".into()),
    };
    for metric in ["min", "max", "sum", "avg"] {
        spec.metric = metric.into();
        let result = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
        assert_eq!(
            result["series"][0]["samples"],
            serde_json::json!([1, 0, 1, 1, 0, 0, 0, 2])
        );
        assert_eq!(result["series"][0]["points"][0], -10.0);
        assert_eq!(result["series"][0]["points"][1], 0.0); // Empty, compatible legacy points.
        assert_eq!(result["series"][0]["points"][3], 0.0); // A real observed zero.
        assert_eq!(result["incompatible_units"], 1);
    }
    spec.metric = "count".into();
    let count = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(
        count["series"][0]["samples"],
        serde_json::json!([1, 0, 1, 1, 1, 1, 1, 2])
    );
    spec.metric = "distinct".into();
    let distinct = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(
        distinct["series"][0]["samples"],
        serde_json::json!([1, 0, 1, 1, 1, 1, 0, 2])
    );

    // Terms use the same validity count, including the optimized count path.
    spec.chart = "terms".into();
    spec.metric = "avg".into();
    let terms = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    for (label, samples) in terms["x"]
        .as_array()
        .unwrap()
        .iter()
        .zip(terms["series"][0]["samples"].as_array().unwrap())
    {
        let missing = matches!(label.as_str().unwrap(), "invalid" | "1GB" | "(vazio)");
        assert_eq!(samples.as_u64().unwrap(), u64::from(!missing));
    }
    spec.metric = "count".into();
    let terms = serde_json::to_value(analysis::compute_series(&events, &spec)).unwrap();
    assert_eq!(
        terms["series"][0]["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .sum::<u64>(),
        8
    );
}

#[test]
fn timeline_buckets_never_extend_past_requested_end() {
    let state = state_for(crate::SourceData::None);
    for (end, requested, expected) in [(100, 80, 51), (0, 100, 1), (2, 100, 3)] {
        let result = workspace::timeline_range_impl(&state, vec![], 0, end, requested).unwrap();
        let json = serde_json::to_value(result).unwrap();
        let buckets = json["buckets"].as_array().unwrap();
        assert_eq!(buckets.len(), expected);
        assert!(buckets
            .iter()
            .all(|b| b["timestamp"].as_i64().unwrap() <= end));
    }
}

#[test]
fn generic_text_with_non_ascii_prefix_never_panics() {
    for message in [
        "错误详情：服务器不可用",
        "⚠ falha de conexão",
        "日本語ログの行",
        "2026-01-01 10:00:00 错误详情",
    ] {
        let ev = sources::parse_line(message.as_bytes(), "text", None, &[]);
        assert_eq!(ev.message, message);
    }
}

#[test]
fn discovery_does_not_mix_incompatible_units_or_claim_independent_pairs() {
    let mut events = Vec::new();
    for i in 0..100 {
        let mut ev = Event::empty();
        ev.fields
            .insert("region".into(), if i % 2 == 0 { "a" } else { "b" }.into());
        ev.fields
            .insert("kind".into(), if i % 4 < 2 { "x" } else { "y" }.into());
        ev.fields.insert(
            "metric".into(),
            if i % 2 == 0 { "10ms" } else { "10GB" }.into(),
        );
        ev.fields.insert(
            "mostly_duration".into(),
            match i {
                0 => "1GB",
                1 => "900ms",
                _ => "10ms",
            }
            .into(),
        );
        events.push(ev);
    }
    let state = state_for(crate::SourceData::Memory(events));
    let result = crate::discover_patterns_impl(&state, vec![], None);
    assert!(result.outliers.is_empty());
    assert!(result
        .associations
        .iter()
        .all(|a| a.left.field != "kind" && a.right.field != "kind"));
    assert_eq!(analysis::parse_num_unit("1e3").unwrap().0, 1000.0);
    assert!(analysis::parse_num_unit("NaN").is_none());
    assert!(analysis::parse_num_unit("Infinity").is_none());
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
fn timeline_keeps_exact_counts_and_gaps_across_storage_modes() {
    let start = 1_706_745_600_000i64;
    let file = Fixture::new(&format!(
        "{{\"timestamp\":{start},\"level\":\"error\",\"source\":\"api\",\"message\":\"failed\"}}\n{{\"timestamp\":{},\"level\":\"warn\",\"source\":\"api\",\"message\":\"slow\"}}\n{{\"timestamp\":{},\"level\":\"info\",\"source\":\"api\",\"message\":\"ok\"}}\n{{\"level\":\"error\",\"source\":\"api\",\"message\":\"undated\"}}\n",
        start + 5_000,
        start + 10_000,
    ));
    let index = file.index("jsonl");
    let events = materialize(&index);
    let make_state = |source| crate::AppState {
        source: parking_lot::RwLock::new(source),
        source_names: parking_lot::RwLock::new(vec![]),
        codes: parking_lot::RwLock::new(CodesConfig::default()),
        system_codes: parking_lot::RwLock::new(CodesConfig::default()),
        derived: parking_lot::RwLock::new(vec![]),
        case_store_lock: parking_lot::Mutex::new(()),
        codes_path: PathBuf::new(),
        system_codes_path: PathBuf::new(),
    };
    let memory = make_state(crate::SourceData::Memory(events));
    let indexed = make_state(crate::SourceData::Indexed(index));
    for state in [&memory, &indexed] {
        let result =
            workspace::timeline_range_impl(state, vec![], start, start + 10_000, 5).unwrap();
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["total"], 3);
        assert_eq!(json["errors"], 1);
        assert_eq!(json["warnings"], 1);
        let buckets = json["buckets"].as_array().unwrap();
        assert_eq!(buckets.len(), 5);
        assert_eq!(
            buckets
                .iter()
                .map(|b| b["count"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1, 0, 1, 0, 1]
        );
        assert_eq!(buckets[0]["errors"], 1);
        assert_eq!(buckets[2]["warnings"], 1);
        let filtered = workspace::timeline_range_impl(
            state,
            vec![filter("level", "equals", "Erro")],
            start,
            start + 10_000,
            5,
        )
        .unwrap();
        assert_eq!(serde_json::to_value(filtered).unwrap()["total"], 1);
    }
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
#[ignore = "Generates a workload (BENCH_MB, default 1 GiB); run explicitly."]
fn benchmark_large_index() {
    let mb = std::env::var("BENCH_MB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1024);
    let file = Fixture::new("");
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&file.0).unwrap());
    let array = std::env::var("BENCH_FORMAT").as_deref() == Ok("array");
    let variants: Vec<_> = (0..32).map(|i| format!(
        "{{\"timestamp\":{},\"log\":{{\"level\":\"{}\"}},\"service\":{{\"name\":\"{}\"}},\"region\":\"{}\",\"duration_ms\":{},\"message\":\"processed job {} {}\"}}",
        1706745600000i64+i*1000, if i<8 {"error"} else {"info"}, if i<10 {"api"} else {"worker"}, if i<12 {"south"} else {"north"}, if i==31 {900} else {20+i%5}, i, "x".repeat(900)
    )).collect();
    let average = variants.iter().map(|line| line.len() + 1).sum::<usize>() / variants.len();
    let count = mb * 1024 * 1024 / average;
    if array {
        writer.write_all(b"[\n").unwrap();
    }
    for i in 0..count {
        if array && i > 0 {
            writer.write_all(b",\n").unwrap();
        }
        let unique = variants[i % variants.len()].replacen(
            &format!("processed job {} ", i % variants.len()),
            &format!("processed job {i} "),
            1,
        );
        writer.write_all(unique.as_bytes()).unwrap();
        if !array {
            writer.write_all(b"\n").unwrap();
        }
    }
    if array {
        writer.write_all(b"\n]\n").unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    let start = std::time::Instant::now();
    let idx = crate::index_cache::open(file.0.to_str().unwrap(), "auto", None, None, None).unwrap();
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
    let start = std::time::Instant::now();
    let top10 = analysis::compute_series_stream(
        || (0..idx.lines.len()).map(|i| sources::event_at(&idx, i, &empty, &empty, &[])),
        &analysis::SeriesSpec {
            chart: "terms".into(),
            metric: "count".into(),
            field: Some("message".into()),
            interval_ms: None,
            split: None,
            limit: Some(10),
            unit: None,
        },
    );
    let top10_count_seconds = start.elapsed().as_secs_f64();
    assert_eq!(
        serde_json::to_value(top10).unwrap()["x"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    let state = state_for(crate::SourceData::Indexed(idx));
    let start = std::time::Instant::now();
    let discovery = crate::discover_patterns_impl(&state, vec![], None);
    let discovery_seconds = start.elapsed().as_secs_f64();
    assert_eq!(discovery.total, count);
    assert_eq!(
        discovery.sample_count,
        count.min(crate::discovery::SAMPLE_CAP)
    );
    assert!(discovery.complete);
    assert!(!discovery.outliers.is_empty());
    assert!(!discovery.associations.is_empty());
    drop(state);
    let start = std::time::Instant::now();
    let reopened =
        crate::index_cache::open(file.0.to_str().unwrap(), "auto", None, None, None).unwrap();
    let warm = start.elapsed().as_secs_f64();
    assert_eq!(reopened.lines.len(), count);
    let result = serde_json::json!({"format":if array {"json_array"} else {"jsonl"},"profile":if cfg!(debug_assertions) {"debug"} else {"release"},"bytes":std::fs::metadata(&file.0).unwrap().len(),"events":count,"cold_index_seconds":cold,"warm_index_seconds":warm,"first_page_seconds":first_seconds,"next_page_seconds":page_seconds,"overview_seconds":overview_seconds,"top10_unique_messages_seconds":top10_count_seconds,"discovery_seconds":discovery_seconds,"discovery_sample_count":discovery.sample_count,"discovery_associations":discovery.associations.len(),"discovery_outliers":discovery.outliers.len(),"discovery_behavior_shifts":discovery.behavior_shifts.len(),"discovery_changes":discovery.changes.len()});
    println!("BENCHMARK {result}");
    if let Ok(path) = std::env::var("BENCH_RESULT") {
        std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
}

#[test]
fn query_parameters_are_automatically_expanded_into_subfields() {
    // 1. JSON com campo aninhado virando request.query_parameter
    let raw = br#"{"request":{"query_parameter":"chave1=valor1&chave2=valor2"},"url":"/search?action=find&limit=10","msg":"User logged in & status=ok","plain":"key=val"}"#;
    let ev = sources::parse_line(raw, "jsonl", None, &[]);

    // Campo original preservado
    assert_eq!(
        ev.fields.get("request.query_parameter").unwrap(),
        "chave1=valor1&chave2=valor2"
    );
    // Subcampos criados
    assert_eq!(
        ev.fields.get("request.query_parameter.chave1").unwrap(),
        "valor1"
    );
    assert_eq!(
        ev.fields.get("request.query_parameter.chave2").unwrap(),
        "valor2"
    );

    // URL com '?' também gera subcampos sob url.*
    assert_eq!(
        ev.fields.get("url").unwrap(),
        "/search?action=find&limit=10"
    );
    assert_eq!(ev.fields.get("url.action").unwrap(), "find");
    assert_eq!(ev.fields.get("url.limit").unwrap(), "10");

    // Campos normais de texto NÃO são falsamente expandidos
    assert!(ev.fields.get("msg.status").is_none());
    assert!(ev.fields.get("plain.key").is_none());

    // 2. Percent-decoding e '+'
    let raw_encoded = br#"{"query":"nome=Jo%C3%A3o&cidade=S%C3%A3o+Paulo"}"#;
    let ev_encoded = sources::parse_line(raw_encoded, "jsonl", None, &[]);
    assert_eq!(ev_encoded.fields.get("query.nome").unwrap(), "João");
    assert_eq!(ev_encoded.fields.get("query.cidade").unwrap(), "São Paulo");
}

#[test]
fn query_parameters_subfields_are_indexed_and_filterable() {
    let mut text = String::new();
    for i in 0..20 {
        let chave1 = if i % 2 == 0 { "alpha" } else { "beta" };
        let chave2 = format!("val_{i}");
        text.push_str(&format!(
            "{{\"source\":\"web\",\"request\":{{\"query_parameter\":\"chave1={chave1}&chave2={chave2}\"}},\"message\":\"req {i}\"}}\n"
        ));
    }
    let file = Fixture::new(&text);
    let index = file.index("auto");

    // Descoberta de colunas inclui os subcampos
    assert!(index.columns.contains(&"request.query_parameter".to_string()));
    assert!(index.columns.contains(&"request.query_parameter.chave1".to_string()));
    assert!(index.columns.contains(&"request.query_parameter.chave2".to_string()));

    // Filtro pelo subcampo funciona perfeitamente
    let filter = crate::query::Filter {
        column: "request.query_parameter.chave1".into(),
        op: "equals_exact".into(),
        value: "alpha".into(),
        value2: None,
    };
    let codes = crate::model::CodesConfig::default();
    let derived = vec![];
    let matched = crate::query::indexed_matches(&index, &[filter], &codes, &codes, &derived);
    assert_eq!(matched.len(), 10);
}


// ------------------------------------------------------------ triage scenarios

fn triage_events(events: Vec<Event>, threats: bool) -> crate::detections::Triage {
    let rules = crate::detections::builtin_ruleset().unwrap();
    let catalog = crate::threats::builtin_catalog();
    let settings = crate::detections::Settings { threats, ..Default::default() };
    let inputs = crate::detections::Inputs {
        rules: &rules,
        catalog: threats.then_some(&*catalog),
        settings: &settings,
    };
    let refs: Vec<&Event> = events.iter().collect();
    crate::detections::run(&inputs, &crate::detections::Source::Events(refs)).unwrap()
}

fn win(id: usize, t: i64, provider: &str, code: &str, fields: serde_json::Value) -> Event {
    let mut e = Event::empty();
    e.id = id;
    e.timestamp = Some(t);
    e.source = provider.into();
    e.code = code.into();
    if let serde_json::Value::Object(map) = fields {
        e.fields = map;
    }
    e.message = format!("{provider} {code}");
    e.raw = e.message.clone();
    e
}

fn rules_of(t: &crate::detections::Triage) -> Vec<String> {
    let mut ids: Vec<String> = t.detections.iter().map(|d| d.rule.clone()).collect();
    ids.sort();
    ids.dedup();
    ids
}

#[test]
fn triage_ssh_bruteforce_success_is_one_episode_with_risky_source() {
    let mut lines = Vec::new();
    for i in 0..12 {
        lines.push(format!("Jan 31 08:0{}:{:02} srv-app-01 sshd[2211]: Failed password for root from 45.90.12.3 port {} ssh2", i / 6, (i % 6) * 9, 50000 + i));
    }
    lines.push("Jan 31 08:03:10 srv-app-01 sshd[2211]: Accepted password for root from 45.90.12.3 port 51122 ssh2".into());
    lines.push("Jan 31 08:05:00 srv-app-01 systemd[1]: Started Daily apt download activities.".into());
    let fixture = Fixture::new(&lines.join("\n"));
    let index = fixture.index("auto");
    let state = state_for(crate::SourceData::Indexed(index));
    let value = crate::triage::triage_impl(&state, vec![], None, true).unwrap();
    let t: serde_json::Value = (*value).clone();
    let rules: Vec<&str> = t["detections"].as_array().unwrap().iter().map(|d| d["rule"].as_str().unwrap()).collect();
    assert!(rules.contains(&"auth.bruteforce.source"), "{rules:?}");
    assert!(rules.contains(&"auth.bruteforce.success"), "{rules:?}");
    assert!(rules.contains(&"auth.root.public"), "{rules:?}");
    let episodes = t["episodes"].as_array().unwrap();
    assert_eq!(episodes.len(), 1, "{episodes:?}");
    assert_eq!(episodes[0]["severity"], "high");
    let top = &t["entities"][0];
    assert_eq!(top["value"], "45.90.12.3");
    assert_eq!(top["scope"], "público");
    assert!(top["failures"].as_u64().unwrap() >= 12);
    // Evidence filters reproduce the supporting records.
    let detection = t["detections"].as_array().unwrap().iter().find(|d| d["rule"] == "auth.bruteforce.source").unwrap();
    let filters: Vec<Filter> = serde_json::from_value(detection["filters"].clone()).unwrap();
    let count = crate::count_filtered_impl(&state, filters, None);
    assert_eq!(count, 12);
    assert!(detection["summary"].as_str().unwrap().contains("45.90.12.3"));
    assert!(t["tactics"].as_array().unwrap().iter().any(|x| x["key"] == "credential-access" && x["count"].as_u64().unwrap() > 0));
}

#[test]
fn triage_windows_spraying_webshell_encoded_powershell_and_log_clear() {
    let base = 1_700_000_000_000i64;
    let security = "Microsoft-Windows-Security-Auditing";
    let sysmon = "Microsoft-Windows-Sysmon";
    let mut events = Vec::new();
    for i in 0..10 {
        events.push(win(i, base + i as i64 * 20_000, security, "4625", serde_json::json!({"TargetUserName": format!("user{i}"), "IpAddress": "10.9.9.9", "LogonType": "3", "Computer": "DC01"})));
    }
    events.push(win(20, base + 600_000, sysmon, "1", serde_json::json!({"ParentImage": "C:\\Windows\\System32\\inetsrv\\w3wp.exe", "Image": "C:\\Windows\\System32\\cmd.exe", "CommandLine": "cmd.exe /c whoami", "Computer": "WEB01"})));
    events.push(win(21, base + 610_000, sysmon, "1", serde_json::json!({"ParentImage": "C:\\Windows\\System32\\cmd.exe", "Image": "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", "CommandLine": "powershell.exe -nop -w hidden -enc SQBFAFgA", "Computer": "WEB01"})));
    events.push(win(22, base + 900_000, security, "1102", serde_json::json!({"SubjectUserName": "admin", "Computer": "WEB01"})));
    let t = triage_events(events, false);
    let rules = rules_of(&t);
    for expected in ["auth.spraying", "exec.webshell-child", "exec.encoded-powershell", "evasion.log-clear"] {
        assert!(rules.contains(&expected.to_string()), "missing {expected}: {rules:?}");
    }
    let spray = t.detections.iter().find(|d| d.rule == "auth.spraying").unwrap();
    assert_eq!(spray.distinct, 10);
    assert!(spray.summary.contains("10.9.9.9") && spray.summary.contains("10"), "{}", spray.summary);
    // WEB01 activity forms a multi-tactic episode.
    let web = t.episodes.iter().find(|e| e.entities.iter().any(|x| x.value == "WEB01")).unwrap();
    assert!(web.tactics.len() >= 3, "{:?}", web.tactics);
    assert!(web.title.starts_with("Possível cadeia de ataque"), "{}", web.title);
    assert!(t.entities.iter().any(|e| e.value == "WEB01" && e.score >= 70));
}

#[test]
fn triage_web_scan_tool_success_port_scan_and_beacon() {
    let mut lines = Vec::new();
    for i in 0..45 {
        lines.push(format!("203.0.113.50 - - [31/Jan/2024:08:{:02}:{:02} -0300] \"GET /admin{i}.php HTTP/1.1\" 404 12 \"-\" \"sqlmap/1.7\"", 1 + i / 30, (i * 2) % 60));
    }
    lines.push("203.0.113.50 - - [31/Jan/2024:08:04:00 -0300] \"GET /uploads/shell.php?cmd=id HTTP/1.1\" 200 88 \"-\" \"sqlmap/1.7\"".into());
    lines.push("192.168.1.10 - - [31/Jan/2024:08:05:00 -0300] \"GET /api/users?id=1 UNION SELECT password FROM users HTTP/1.1\" 500 12 \"-\" \"Mozilla/5.0\"".into());
    let web = Fixture::new(&lines.join("\n"));
    let web_state = state_for(crate::SourceData::Indexed(web.index("auto")));
    let t: serde_json::Value = (*crate::triage::triage_impl(&web_state, vec![], None, true).unwrap()).clone();
    let rules: Vec<&str> = t["detections"].as_array().unwrap().iter().map(|d| d["rule"].as_str().unwrap()).collect();
    for expected in ["web.scan", "tool.scanner", "web.success-after-scan"] {
        assert!(rules.contains(&expected), "missing {expected}: {rules:?}");
    }
    let mut fw = Vec::new();
    for port in 0..25 {
        fw.push(format!("Jan 31 08:00:{:02} fw-01 kernel: [9.1] iptables DROP IN=eth0 OUT= SRC=198.51.100.7 DST=10.0.0.1 PROTO=TCP SPT=40000 DPT={} SYN", port, 1000 + port));
    }
    for n in 0..12 {
        let t = 600 + n * 60 + (n % 2);
        fw.push(format!("Jan 31 08:{:02}:{:02} fw-01 kernel: [9.2] iptables ACCEPT IN=eth0 OUT= SRC=10.0.0.23 DST=185.100.87.3 PROTO=TCP SPT=40001 DPT=443", t / 60, t % 60));
    }
    let firewall = Fixture::new(&fw.join("\n"));
    let fw_state = state_for(crate::SourceData::Indexed(firewall.index("auto")));
    let t: serde_json::Value = (*crate::triage::triage_impl(&fw_state, vec![], None, true).unwrap()).clone();
    let detections = t["detections"].as_array().unwrap();
    let rules: Vec<&str> = detections.iter().map(|d| d["rule"].as_str().unwrap()).collect();
    assert!(rules.contains(&"net.portscan"), "{rules:?}");
    let beacon = detections.iter().find(|d| d["rule"] == "net.beacon").expect("beacon");
    assert!((59_000..=61_000).contains(&beacon["period_ms"].as_i64().unwrap()), "{}", beacon["period_ms"]);
    assert!(beacon["summary"].as_str().unwrap().contains("1 min"), "{}", beacon["summary"]);
}

#[test]
fn triage_threat_signals_rarity_and_suppression() {
    let base = 1_700_000_000_000i64;
    let mut events = Vec::new();
    for i in 0..80 {
        let process = if i == 40 { "C:\\Users\\Public\\x.exe" } else { ["C:\\Windows\\explorer.exe", "C:\\Windows\\System32\\svchost.exe", "C:\\Program Files\\App\\app.exe", "C:\\Windows\\System32\\cmd.exe", "C:\\Windows\\System32\\conhost.exe", "C:\\Windows\\System32\\taskhostw.exe"][i % 6] };
        events.push(win(i, base + i as i64 * 1000, "Microsoft-Windows-Sysmon", "1", serde_json::json!({"Image": process, "Computer": "WS1"})));
    }
    let mut sqli = Event::empty();
    sqli.id = 500;
    sqli.timestamp = Some(base + 5000);
    sqli.source = "203.0.113.9".into();
    sqli.message = "GET /p?id=1' UNION SELECT username,password FROM users--".into();
    sqli.raw = sqli.message.clone();
    events.push(sqli);
    let t = triage_events(events.clone(), true);
    assert!(t.rare.iter().any(|r| r.value == "x.exe"), "{:?}", t.rare.iter().map(|r| &r.value).collect::<Vec<_>>());
    let signal = t.detections.iter().find(|d| d.origin == "threats").expect("threat signal");
    assert_eq!(signal.name, "Injeção SQL");
    assert!(signal.entities.iter().any(|e| e.value == "203.0.113.9"));
    assert!(signal.attack.iter().any(|a| a.id == "T1190"));
    // Suppressing the signal for that address hides it and counts it.
    let rules = crate::detections::builtin_ruleset().unwrap();
    let catalog = crate::threats::builtin_catalog();
    let settings = crate::detections::Settings {
        suppress: vec![crate::detections::Suppression { rule: signal.rule.clone(), column: None, value: Some("203.0.113.9".into()), ..Default::default() }],
        ..Default::default()
    };
    let inputs = crate::detections::Inputs { rules: &rules, catalog: Some(&catalog), settings: &settings };
    let refs: Vec<&Event> = events.iter().collect();
    let hidden = crate::detections::run(&inputs, &crate::detections::Source::Events(refs)).unwrap();
    assert!(hidden.detections.iter().all(|d| d.origin != "threats"));
    assert!(hidden.suppressed >= 1);
}

#[test]
fn search_language_filters_indexed_and_memory_sources_alike() {
    let text = [
        r#"{"timestamp":"2024-01-31T08:00:00Z","level":"error","message":"login failed","user":"alice","src_ip":"10.1.2.3","status":401}"#,
        r#"{"timestamp":"2024-01-31T08:00:01Z","level":"info","message":"login ok","user":"bob","src_ip":"8.8.8.8","status":200}"#,
        r#"{"timestamp":"2024-01-31T08:00:02Z","level":"warn","message":"slow request","user":"carol","src_ip":"10.200.0.1","status":200}"#,
    ]
    .join("\n");
    let fixture = Fixture::new(&text);
    let index = fixture.index("jsonl");
    let codes = CodesConfig::default();
    let events: Vec<Event> = (0..index.lines.len()).map(|i| sources::event_at(&index, i, &codes, &codes, &[])).collect();
    for (query, expected) in [
        ("login", 2),
        ("login failed", 1),
        ("user:(alice OR carol)", 2),
        ("ip:10.0.0.0/8", 2),
        ("-level:erro status>=200", 2),
        ("@src_scope:público", 1),
        ("status:4* OR user:carol", 2),
        ("NOT (user:alice OR user:bob)", 1),
    ] {
        let filters = vec![filter("_all", "query", query)];
        assert_eq!(query::filtered_indices(&events, &filters).len(), expected, "memory {query}");
        assert_eq!(query::indexed_matches(&index, &filters, &codes, &codes, &[]).len(), expected, "indexed {query}");
    }
    let list = vec![filter("user", "in", "alice\nBOB")];
    assert_eq!(query::filtered_indices(&events, &list).len(), 2);
    let nets = vec![filter("src_ip", "cidr", "10.0.0.0/8, 192.168.0.0/16")];
    assert_eq!(query::indexed_matches(&index, &nets, &codes, &codes, &[]).len(), 2);
    assert!(workspace::validate(&[filter("_all", "query", "user:(alice")]).is_err());
    assert!(workspace::validate(&[filter("src_ip", "cidr", "10.0.0.0/99")]).is_err());
}

// ------------------------------------------------------------ ingestion

fn events_of(index: &sources::FileIndex) -> Vec<Event> {
    let codes = CodesConfig::default();
    (0..index.lines.len()).map(|i| sources::event_at(index, i, &codes, &codes, &[])).collect()
}

#[test]
fn json_envelopes_are_split_into_records() {
    let cloudtrail = r#"{"Records":[{"eventVersion":"1.08","eventTime":"2024-01-31T10:00:00Z","eventSource":"signin.amazonaws.com","eventName":"ConsoleLogin","eventID":"a1b2","sourceIPAddress":"203.0.113.5","userIdentity":{"type":"Root","arn":"arn:aws:iam::1:root"},"responseElements":{"ConsoleLogin":"Success"},"additionalEventData":{"MFAUsed":"No"}},{"eventTime":"2024-01-31T10:05:00Z","eventSource":"iam.amazonaws.com","eventName":"CreateAccessKey","eventID":"c3d4","sourceIPAddress":"203.0.113.5","userIdentity":{"type":"IAMUser","userName":"ops"},"errorCode":"AccessDenied"}]}"#;
    let fixture = Fixture::new(cloudtrail);
    let index = fixture.index("auto");
    assert_eq!(index.lines.len(), 2);
    let events = events_of(&index);
    assert_eq!(events[0].code, "ConsoleLogin");
    assert_eq!(events[0].fields["eventID"], "a1b2");
    assert!(events[0].message.contains("ConsoleLogin"));
    assert_eq!(events[1].level, "Aviso");
    assert_eq!(crate::entities::value(&events[0], crate::entities::Role::SrcIp).as_deref(), Some("203.0.113.5"));
    let t = triage_events(events, false);
    let rules = rules_of(&t);
    assert!(rules.contains(&"cloud.root".to_string()) && rules.contains(&"cloud.console-no-mfa".to_string()), "{rules:?}");

    let pretty = "{\n  \"meta\": {\"count\": 2},\n  \"value\": [\n    {\"time\": \"2024-01-31T10:00:00Z\", \"message\": \"a\"},\n    {\"time\": \"2024-01-31T10:00:01Z\", \"message\": \"b\"}\n  ],\n  \"next\": null\n}\n";
    let fixture = Fixture::new(pretty);
    let index = fixture.index("auto");
    assert_eq!(events_of(&index).iter().map(|e| e.message.clone()).collect::<Vec<_>>(), ["a", "b"]);
    let elastic = r#"{"took":3,"hits":{"total":{"value":1},"hits":[{"_source":{"@timestamp":"2024-01-31T10:00:00Z","message":"x"}}]}}"#;
    let fixture = Fixture::new(elastic);
    assert_eq!(fixture.index("auto").lines.len(), 1);
    // JSONL with array fields stays line-oriented.
    let jsonl = "{\"message\":\"a\",\"items\":[{\"x\":1}]}\n{\"message\":\"b\",\"items\":[]}\n";
    let fixture = Fixture::new(jsonl);
    assert_eq!(fixture.index("auto").lines.len(), 2);
}

#[test]
fn zeek_auditd_and_suricata_are_readable() {
    let zeek = "#separator \\x09\n#set_separator\t,\n#path\tconn\n#fields\tts\tuid\tid.orig_h\tid.orig_p\tid.resp_h\tid.resp_p\tproto\tservice\n#types\ttime\tstring\taddr\tport\taddr\tport\tenum\tstring\n1706695200.123456\tC1\t10.0.0.5\t51000\t93.184.216.34\t443\ttcp\tssl\n1706695201.5\tC2\t10.0.0.5\t51001\t8.8.8.8\t53\tudp\t-\n";
    let fixture = Fixture::new(zeek);
    let index = fixture.index("auto");
    assert_eq!(index.format, "zeek");
    let events = events_of(&index);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].timestamp, Some(1_706_695_200_123));
    assert!(events[0].message.contains("10.0.0.5 → 93.184.216.34:443"), "{}", events[0].message);
    assert!(!events[1].fields.contains_key("service"));
    assert_eq!(crate::entities::value(&events[0], crate::entities::Role::DstIp).as_deref(), Some("93.184.216.34"));

    let audit = "type=USER_AUTH msg=audit(1706695200.500:101): pid=1 uid=0 auid=4294967295 ses=4294967295 msg='op=PAM:authentication grantors=? acct=\"root\" exe=\"/usr/sbin/sshd\" hostname=45.90.12.3 addr=45.90.12.3 terminal=ssh res=failed'\ntype=EXECVE msg=audit(1706695201.000:102): argc=2 a0=\"cat\" a1=\"/etc/shadow\"\ntype=PROCTITLE msg=audit(1706695201.000:102): proctitle=636174002F6574632F736861646F77\n";
    let fixture = Fixture::new(audit);
    let index = fixture.index("auto");
    assert_eq!(index.format, "auditd");
    let events = events_of(&index);
    assert_eq!(events[0].timestamp, Some(1_706_695_200_500));
    assert_eq!(events[0].level, "Aviso");
    assert_eq!(crate::entities::action_outcome(&events[0]), (Some("logon"), Some("failure")));
    assert_eq!(crate::entities::value(&events[0], crate::entities::Role::User).as_deref(), Some("root"));
    assert_eq!(crate::entities::value(&events[0], crate::entities::Role::SrcIp).as_deref(), Some("45.90.12.3"));
    assert_eq!(events[2].fields["cmdline"], "cat /etc/shadow");
    assert_eq!(events[1].fields["audit_serial"], events[2].fields["audit_serial"]);

    let eve = r#"{"timestamp":"2024-01-31T10:00:00.000000+0000","event_type":"alert","src_ip":"45.90.12.3","dest_ip":"10.0.0.1","dest_port":22,"alert":{"signature":"ET SCAN SSH BruteForce","signature_id":2001219,"severity":2}}"#;
    let fixture = Fixture::new(eve);
    let events = events_of(&fixture.index("auto"));
    assert_eq!(events[0].message, "ET SCAN SSH BruteForce");
    assert_eq!(events[0].code, "2001219");
    assert_eq!(events[0].level, "Aviso");
    assert_eq!(crate::entities::action_outcome(&events[0]).0, Some("ids_alert"));
}

#[test]
fn zip_and_tar_members_become_sources() {
    let dir = std::env::temp_dir().join(format!("loginsight-archive-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("pack.zip");
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("logs/app.log", options).unwrap();
        zip.write_all(b"2024-01-31 10:00:00 ERROR falha\n2024-01-31 10:00:01 INFO ok\n").unwrap();
        zip.start_file("../evil.log", options).unwrap();
        zip.write_all(b"x\n").unwrap();
        zip.start_file("image.png", options).unwrap();
        zip.write_all(b"\x89PNG").unwrap();
        zip.finish().unwrap();
    }
    let members = workspace::archive_members(&zip_path).unwrap();
    assert_eq!(members.len(), 1, "{members:?}");
    assert!(members[0].ends_with("pack.zip!/logs/app.log"));
    let index = crate::index_source_file(&members[0], "auto", None).unwrap();
    assert_eq!(index.lines.len(), 2);
    assert_eq!(index.parts[0].path, members[0]);
    assert_eq!(index.parts[0].file_name, "app.log");

    let tgz_path = dir.join("pack.tar.gz");
    {
        let file = std::fs::File::create(&tgz_path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let data = b"{\"message\":\"a\"}\n{\"message\":\"b\"}\n{\"message\":\"c\"}\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "srv/events.jsonl", &data[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let members = workspace::archive_members(&tgz_path).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(crate::index_source_file(&members[0], "auto", None).unwrap().lines.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Real EVTX samples (Windows event exports). Run with
/// LOGINSIGHT_EVTX_SAMPLES=<dir> cargo test --lib evtx_samples -- --ignored --nocapture
#[test]
#[ignore]
fn evtx_samples_triage() {
    let dir = std::env::var("LOGINSIGHT_EVTX_SAMPLES").expect("LOGINSIGHT_EVTX_SAMPLES");
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "evtx") || std::fs::metadata(&path).unwrap().len() < 1000 {
            continue;
        }
        let mut events = Vec::new();
        let count = sources::visit_evtx_file(path.to_str().unwrap(), usize::MAX, |mut e| {
            e.id = events.len();
            events.push(e);
            Ok(())
        })
        .unwrap();
        assert!(count > 0);
        if std::env::var("LOGINSIGHT_EVTX_DUMP").is_ok() {
            for e in events.iter().take(8) {
                println!("   {} {} {:?}", e.source, e.code, e.fields.iter().filter(|(k, _)| !["arquivo", "caminho"].contains(&k.as_str())).map(|(k, v)| format!("{k}={}", v.to_string().chars().take(80).collect::<String>())).collect::<Vec<_>>());
            }
        }
        let t = triage_events(events, true);
        println!("\n== {} ({} eventos)", path.file_name().unwrap().to_string_lossy(), count);
        for d in &t.detections {
            println!("  [{}] {} · {} · {}", d.severity, d.name, d.count, d.summary);
        }
        for e in &t.episodes {
            println!("  episódio: {} ({}) {:?}", e.title, e.severity, e.tactics);
        }
    }
}
