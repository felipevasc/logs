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
    assert_eq!(shift.outcome_op, "equals");
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
