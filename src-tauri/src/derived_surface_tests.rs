//! Cross-surface proof for case-owned Base64 -> ParseJson fields. These tests
//! exercise the native implementations used by both Tauri and MCP Dataset
//! commands; they do not claim MCP supports saved-Case evidence or that the UI's
//! sampled field catalog discovers every child in a large source.
use crate::{
    analysis_context, analysis_runtime, model::Event, query, sources, AppState, SourceData,
};
use base64::Engine as _;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use std::{collections::BTreeMap, ffi::OsString, sync::Arc};

const START: i64 = 1_700_000_000_000;

struct Fixture {
    state: AppState,
    original: Vec<Event>,
    identity: analysis_context::Identity,
    _directory: tempfile::TempDir,
    environment: Vec<(&'static str, Option<OsString>)>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let environment = [
            "LOGINSIGHT_DATA_DIR",
            "LOGINSIGHT_ENGINE_DIR",
            "LOGINSIGHT_ENGINE",
            "LOGINSIGHT_TIME_PRECOMPUTE",
        ]
        .into_iter()
        .map(|name| (name, std::env::var_os(name)))
        .collect();
        std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
        std::env::set_var("LOGINSIGHT_ENGINE_DIR", directory.path().join("engine"));
        std::env::set_var("LOGINSIGHT_ENGINE", "1");
        std::env::set_var("LOGINSIGHT_TIME_PRECOMPUTE", "0");
        crate::engine::set_enabled(false);
        crate::case_store::save(json!({"cases":[{"id":"decoded-case"}],"active":null})).unwrap();
        let initial = analysis_context::snapshot("decoded-case").unwrap();
        let config = serde_json::from_value(json!({"derivedFields":[{
            "name":"decoded", "source":"encoded", "rules":[],
            "steps":["base64_decode","parse_json"]
        }]}))
        .unwrap();
        let identity = analysis_context::update(&initial.identity(), config)
            .unwrap()
            .identity();
        let statuses = [503, 200, 503, 404, 201, 500];
        let mut records = Vec::new();
        for id in 0..9 {
            let mut record = json!({
                "timestamp":START + id as i64 * 1000,
                "source":"nginx-json-fixture", "message":format!("request {id}"),
                "level":"INFO", "path":format!("/request/{id}")
            });
            if id < statuses.len() || id == 8 {
                let body = json!({
                    "status":statuses[id.min(5)], "allowed":id % 2 == 1,
                    "user":{"id":if id % 2 == 0 {"alpha"} else {"beta"}},
                    "nil":null, "empty":"", "tags":["api",id]
                });
                record["encoded"] = Value::String(
                    base64::engine::general_purpose::STANDARD.encode(body.to_string()),
                );
            } else if id == 6 {
                record["encoded"] = json!("not valid base64!");
            }
            if id == 8 {
                // Atomic collision handling must preserve the original child,
                // rather than publish a partial transformed object.
                record["decoded.status"] = json!(999);
            }
            records.push(record.to_string() + "\n");
        }
        let first = directory.path().join("first.jsonl");
        let second = directory.path().join("second.jsonl");
        std::fs::write(&first, records[..5].concat()).unwrap();
        std::fs::write(&second, records[5..].concat()).unwrap();
        let mut index =
            sources::index_file(first.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        index
            .append(
                sources::index_file(second.to_str().unwrap(), "jsonl", None, None, None).unwrap(),
            )
            .unwrap();
        let original = (0..9)
            .map(|id| sources::event_at(&index, id, &Default::default(), &Default::default(), &[]))
            .collect();
        let state = AppState {
            source: RwLock::new(SourceData::None),
            source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(Vec::new()),
            codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()),
            derived: RwLock::new(Vec::new()),
            case_store_lock: Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        };
        let publishing = analysis_runtime::capture(
            &state,
            Some(identity.clone()),
            Some(0),
            analysis_runtime::Mode::Publish,
        )
        .unwrap();
        analysis_runtime::with(Some(publishing), || {
            crate::source_publication::publish(
                &state,
                index,
                vec!["first.jsonl".into(), "second.jsonl".into()],
                vec![],
                false,
            )
        })
        .unwrap();
        Self {
            state,
            original,
            identity,
            _directory: directory,
            environment,
        }
    }

    fn dataset(&self) -> Arc<analysis_runtime::Admitted> {
        let admitted = analysis_runtime::capture(
            &self.state,
            Some(self.identity.clone()),
            Some(crate::source_publication::receipt_locked(&self.state).generation),
            analysis_runtime::Mode::Dataset,
        )
        .unwrap();
        admitted.prepare_visibility(None).unwrap();
        admitted.validate(&self.state).unwrap();
        admitted
    }

    fn synchronized_case(&self) -> (Arc<analysis_runtime::Admitted>, Vec<Event>) {
        let key = format!("derived-child-matrix-{}", uuid::Uuid::new_v4());
        let receipt = tauri::async_runtime::block_on(crate::case_cache::case_sync(
            key.clone(),
            self.original.clone(),
            Some(self.identity.clone()),
        ))
        .unwrap();
        let (admitted, events) = analysis_runtime::capture_case(
            &self.state,
            Some(self.identity.clone()),
            None,
            None,
            Some(key),
        )
        .unwrap();
        assert_eq!(
            admitted.case_content_token.as_deref(),
            Some(receipt.case_content_token.as_str())
        );
        let events = admitted.prepare_visibility(events).unwrap().unwrap();
        admitted.validate(&self.state).unwrap();
        (admitted, events)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        crate::engine::set_enabled(false);
        for (name, value) in self.environment.drain(..) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn filter(column: &str, op: &str, value: &str) -> query::Filter {
    query::Filter {
        column: column.into(),
        op: op.into(),
        value: value.into(),
        value2: None,
    }
}

fn ids(result: &query::QueryResult) -> Vec<usize> {
    result.rows.iter().map(|event| event.id).collect()
}

fn assert_preserved(rows: &[Event], originals: &[Event], full_raw: bool) {
    assert_eq!(rows.len(), 9);
    for row in rows {
        let original = &originals[row.id];
        assert_eq!(row.event_ref, original.event_ref);
        assert_eq!(row.fields.get("encoded"), original.fields.get("encoded"));
        assert_eq!(row.message, original.message);
        if full_raw {
            assert_eq!(row.raw, original.raw);
        }
        if row.id < 6 {
            assert!(row.fields["decoded"].is_object());
            assert!(row.fields["decoded.status"].is_number());
            assert!(row.fields["decoded.allowed"].is_boolean());
            assert_eq!(row.fields["decoded.nil"], Value::Null);
            assert_eq!(row.fields["decoded.empty"], "");
            assert_eq!(row.fields["decoded.tags.1"], row.id);
            assert!(row.derived_originals.contains_key("decoded.status"));
        } else {
            assert!(!row.fields.contains_key("decoded"));
            assert!(!row.fields.contains_key("decoded.user.id"));
            if row.id == 6 || row.id == 8 {
                assert!(!row.derived_diagnostics.is_empty());
            }
            if row.id == 8 {
                assert_eq!(row.fields["decoded.status"], 999);
            }
        }
    }
}

/// The exact same native analytic implementations receive Dataset admission or
/// the synchronized Case's request-local transformed evidence. No JS/mock engine
/// or manually constructed decoded children stand in for the transform pipeline.
fn surfaces(fixture: &Fixture, evidence: Option<&[Event]>) -> Value {
    let state = &fixture.state;
    let page = crate::query_events_scope_impl(state, vec![], "id", "asc", 0, 20, evidence).unwrap();
    assert_eq!(page.total, 9);
    assert_preserved(&page.rows, &fixture.original, false);
    let cases = [
        (filter("decoded.status", "gte", "500"), vec![0, 2, 5, 8]),
        (
            filter("decoded.allowed", "equals_exact", "false"),
            vec![0, 2, 4],
        ),
        (
            filter("decoded.nil", "equals_exact", "null"),
            vec![0, 1, 2, 3, 4, 5],
        ),
        (filter("decoded.status", "empty", ""), vec![6, 7]),
        (
            filter("decoded.tags.0", "equals_exact", "api"),
            vec![0, 1, 2, 3, 4, 5],
        ),
        (
            filter(
                "_all",
                "query",
                "decoded.allowed:false AND decoded.status>=500",
            ),
            vec![0, 2],
        ),
    ];
    for (test, expected) in cases {
        let result =
            crate::query_events_scope_impl(state, vec![test.clone()], "id", "asc", 0, 20, evidence)
                .unwrap();
        assert_eq!(
            ids(&result),
            expected,
            "{} {} {}",
            test.column,
            test.op,
            test.value
        );
        assert_eq!(result.total, expected.len());
        assert_eq!(
            crate::count_filtered_impl(state, vec![test], evidence.map(<[Event]>::to_vec)).unwrap(),
            expected.len()
        );
    }
    let valid = vec![filter("decoded.user.id", "not_empty", "")];
    let ascending = crate::query_events_scope_impl(
        state,
        valid.clone(),
        "decoded.status",
        "asc",
        0,
        20,
        evidence,
    )
    .unwrap();
    assert_eq!(
        ascending
            .rows
            .iter()
            .map(|event| event.fields["decoded.status"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [200, 201, 404, 500, 503, 503]
    );
    let descending = crate::query_events_scope_impl(
        state,
        valid.clone(),
        "decoded.status",
        "desc",
        0,
        20,
        evidence,
    )
    .unwrap();
    assert_eq!(
        descending
            .rows
            .iter()
            .map(|event| event.fields["decoded.status"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [503, 503, 500, 404, 201, 200]
    );
    let measures: Vec<query::AggSpec> = serde_json::from_value(json!([
        {"func":"count","column":"*","alias":"n"},
        {"func":"sum","column":"decoded.status","alias":"status_sum"}
    ]))
    .unwrap();
    let grouped = crate::aggregate_events_impl(
        state,
        "decoded.user.id",
        measures.clone(),
        valid.clone(),
        evidence.map(<[Event]>::to_vec),
    );
    assert!(grouped.error.is_none(), "{:?}", grouped.error);
    let groups: BTreeMap<_, _> = grouped
        .rows
        .iter()
        .map(|row| {
            (
                row["decoded.user.id"].as_str().unwrap(),
                (
                    row["n"].as_u64().unwrap(),
                    row["status_sum"].as_f64().unwrap(),
                ),
            )
        })
        .collect();
    assert_eq!(
        groups,
        BTreeMap::from([("alpha", (3, 1207.0)), ("beta", (3, 1104.0))])
    );
    let spec = crate::grouped_timeline::Spec::new(
        "decoded.user.id".into(),
        crate::grouped_timeline::Grid {
            start: START,
            bucket_ms: 3000,
            bucket_count: 3,
        },
        Some(12),
        crate::grouped_timeline::Context::default(),
    )
    .unwrap();
    let timeline = crate::workspace::grouped_timeline_impl(state, &[], evidence, &spec).unwrap();
    assert_eq!(timeline.total.buckets, [3, 3, 3]);
    assert_eq!(timeline.missing.buckets, [0, 0, 3]);
    let timelines: BTreeMap<_, _> = timeline
        .series
        .iter()
        .map(|series| (series.key.as_str(), series.buckets.clone()))
        .collect();
    assert_eq!(
        timelines,
        BTreeMap::from([("alpha", vec![2, 1, 0]), ("beta", vec![1, 2, 0])])
    );
    let chart_spec = serde_json::from_value(json!({"chart":"time","metric":"sum","field":"decoded.status","split":"decoded.allowed","interval_ms":1000,"limit":12})).unwrap();
    let chart = crate::compute_series_impl(
        state,
        valid.clone(),
        evidence.map(<[Event]>::to_vec),
        chart_spec,
    )
    .unwrap();
    assert_eq!(
        chart
            .series
            .iter()
            .flat_map(|series| &series.points)
            .sum::<f64>(),
        2311.0
    );
    assert_eq!(
        chart
            .series
            .iter()
            .map(|series| series.samples.iter().sum::<usize>())
            .sum::<usize>(),
        6
    );
    let pivot_spec = serde_json::from_value(json!({"rows":["decoded.user.id"],"cols":["decoded.allowed"],"values":[{"func":"count","column":"*","alias":"n"},{"func":"sum","column":"decoded.status","alias":"status_sum"}],"limit_rows":20})).unwrap();
    let pivot = serde_json::to_value(
        crate::pivot_impl(state, valid, evidence.map(<[Event]>::to_vec), pivot_spec).unwrap(),
    )
    .unwrap();
    assert_eq!(pivot["processed_events"], 6);
    assert_eq!(pivot["complete"], true);
    assert_eq!(pivot["truncated"], false);
    let mut jsonl = Vec::new();
    let mut csv = Vec::new();
    for (format, output) in [("jsonl", &mut jsonl), ("csv", &mut csv)] {
        let count = if let Some(events) = evidence {
            crate::workspace::write_events_export(output, format, false, || events.iter().cloned())
                .unwrap()
        } else {
            crate::workspace::with_selection(state, &[], |selection| {
                crate::workspace::write_events_export(output, format, false, || selection.iter())
            })
            .unwrap()
            .unwrap()
        };
        assert_eq!(count, 9);
    }
    let exported: Vec<Event> = String::from_utf8(jsonl)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_preserved(&exported, &fixture.original, true);
    let csv = String::from_utf8(csv).unwrap();
    assert!(csv.lines().next().unwrap().contains("decoded.status"));
    assert!(csv.lines().next().unwrap().contains("decoded.allowed"));
    json!({"asc":ids(&ascending),"desc":ids(&descending),"group":grouped,"timeline":timeline,"chart":chart,"pivot":pivot,
        "exportFields":exported.iter().map(|event| (&event.event_ref, &event.fields)).collect::<Vec<_>>()})
}

#[test]
fn base64_json_children_match_dataset_lines_columnar_and_synchronized_case_surfaces() {
    let fixture = Fixture::new();
    let admitted = fixture.dataset();
    let expected = analysis_runtime::with(Some(admitted.clone()), || surfaces(&fixture, None));
    crate::engine::set_enabled(true);
    analysis_runtime::with(Some(admitted.clone()), || {
        let source = analysis_runtime::source(&fixture.state);
        let SourceData::Indexed(index) = &*source else {
            panic!("indexed fixture")
        };
        let derived = analysis_runtime::derived(&fixture.state);
        crate::engine::prepare(
            index,
            &Default::default(),
            &Default::default(),
            &derived,
            &|_, _| {},
        )
        .unwrap();
        assert!(crate::engine::session_checked(
            index,
            &Default::default(),
            &Default::default(),
            &derived
        )
        .unwrap()
        .is_some());
        assert_eq!(surfaces(&fixture, None), expected);
        let valid = [filter("decoded.user.id", "not_empty", "")];
        let mut cursor = None;
        let mut collected = Vec::new();
        loop {
            let page = query::query_page_indexed(
                index,
                &valid,
                "decoded.status",
                "asc",
                0,
                2,
                cursor.as_deref(),
                &Default::default(),
                &Default::default(),
                &derived,
            )
            .unwrap();
            assert_eq!(page.engine, "columnar");
            collected.extend(page.rows.iter().map(|event| event.id));
            if !page.has_more {
                break;
            }
            cursor = Some(page.next_cursor.expect("indexed next page"));
            assert!(collected.len() < 9, "cursor must advance");
        }
        assert_eq!(
            collected,
            expected["asc"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_u64().unwrap() as usize)
                .collect::<Vec<_>>()
        );
    });
    let (case_admitted, case_events) = fixture.synchronized_case();
    assert_preserved(&case_events, &fixture.original, true);
    analysis_runtime::with(Some(case_admitted.clone()), || {
        assert_eq!(surfaces(&fixture, Some(&case_events)), expected);
        case_admitted.validate_visibility().unwrap();
    });
    // Case synchronization and request-local transforms never rewrite evidence.
    let reloaded = crate::case_cache::take_for(
        None,
        case_admitted.case_key.clone(),
        Some(&fixture.identity),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(reloaded.unwrap()).unwrap(),
        serde_json::to_value(&fixture.original).unwrap()
    );
    assert!(!fixture.original[0].raw.is_empty());
    assert!(!fixture.original[0].fields.contains_key("decoded"));
    assert_eq!(fixture.original[8].fields["decoded.status"], 999);
}
