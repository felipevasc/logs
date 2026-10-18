use crate::local_diagnostics;
use log_learning::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

struct Restore(Option<std::ffi::OsString>);
impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(value) = &self.0 {
            std::env::set_var("LOGINSIGHT_DATA_DIR", value)
        } else {
            std::env::remove_var("LOGINSIGHT_DATA_DIR")
        }
    }
}
fn isolated(run: impl FnOnce()) {
    let directory = tempfile::tempdir().unwrap();
    let _restore = Restore(std::env::var_os("LOGINSIGHT_DATA_DIR"));
    std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
    run();
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE investigation_facts(ref TEXT PRIMARY KEY,eid INTEGER,ns TEXT,ts INTEGER,source TEXT,host TEXT,actor TEXT,src TEXT,payload TEXT);").unwrap();
    db
}
fn insert(db: &Connection, id: i64, ns: &str, ts: i64, version: &str, record: DiagnosticRecord) {
    let facts = extract_diagnostic(&record);
    let payload = json!({"parse_ok":true,"outcome":if record.success{"success"}else if record.failure{"failure"}else{"unknown"},"action":"checkout","template":record.message,"metadata":{"product":"application","service":"private-checkout","time":{"ambiguity":Value::Null},"diagnostics":facts,"learning":{"operation":"checkout","dependency":"private-db","version":version,"trace":format!("private-trace-{id}")}}});
    db.execute("INSERT INTO investigation_facts VALUES(?1,?2,?3,?4,'private-app.log','private-host','private-user','192.0.2.3',?5)",params![format!("{ns}:{id}"),id,ns,ts,payload.to_string()]).unwrap();
}
fn pool() -> DiagnosticRecord {
    DiagnosticRecord {
        message: "timeout acquiring connection".into(),
        failure: true,
        metrics: std::collections::BTreeMap::from([
            (DiagnosticMetric::PoolActive, 10.0),
            (DiagnosticMetric::PoolLimit, 10.0),
            (DiagnosticMetric::PoolPending, 3.0),
            (DiagnosticMetric::AcquireMs, 1000.0),
        ]),
        ..Default::default()
    }
}

#[test]
fn mapped_diagnostic_quantities_require_units_and_preserve_error_semantics() {
    let mut event = crate::model::Event::empty();
    event.source = "custom.log".into();
    event.level = "Erro".into();
    event.fields = json!({"elapsed":1.5,"ceiling":0.5,"vendor_code":"40P01"})
        .as_object()
        .unwrap()
        .clone();
    let mut mapping:crate::security_normalize::SourceMapping = serde_json::from_value(json!({"source":"custom.log","fields":{"query_ms":"elapsed","query_limit_ms":"ceiling","sqlstate":"vendor_code"},"units":{"query_ms":"s","query_limit_ms":"s"}})).unwrap();
    crate::security_normalize::validate_mappings(&[mapping.clone()]).unwrap();
    let (_, normalized) = crate::security_normalize::normalize(&event, &[mapping.clone()]);
    let facts: DiagnosticEventFacts =
        serde_json::from_value(local_diagnostics::event_facts(&event, &normalized)).unwrap();
    facts.validate().unwrap();
    assert_eq!(facts.metrics[&DiagnosticMetric::QueryMs], 1500.0);
    assert_eq!(facts.sources["query_ms"], "elapsed");
    assert_ne!(facts.mask & DiagnosticFeature::QuerySlow.bit(), 0);
    assert_ne!(facts.mask & DiagnosticFeature::Deadlock.bit(), 0);
    mapping.units.clear();
    let (_, normalized) = crate::security_normalize::normalize(&event, &[mapping.clone()]);
    let facts: DiagnosticEventFacts =
        serde_json::from_value(local_diagnostics::event_facts(&event, &normalized)).unwrap();
    assert!(facts.metrics.is_empty());
    assert!(!normalized.limitations.is_empty());
    mapping.units.insert("query_ms".into(), "count".into());
    assert!(crate::security_normalize::validate_mappings(&[mapping]).is_err());
}

#[test]
fn nearby_measurements_preserve_counterevidence_without_transitive_recruitment() {
    isolated(|| {
        let db = database();
        let start = 1_700_000_000_000;
        let error = || DiagnosticRecord {
            message: "timeout acquiring connection".into(),
            failure: true,
            ..Default::default()
        };
        insert(&db, 0, "a", start, "A", error());
        insert(&db, 30, "a", start + 30_000, "A", error());
        for (id, offset, active) in [(1, 4_000, 2.0), (2, 8_000, 10.0)] {
            insert(
                &db,
                id,
                "a",
                start + offset,
                "A",
                DiagnosticRecord {
                    metrics: std::collections::BTreeMap::from([
                        (DiagnosticMetric::PoolActive, active),
                        (DiagnosticMetric::PoolLimit, 10.0),
                    ]),
                    ..Default::default()
                },
            );
        }
        db.execute("UPDATE investigation_facts SET payload=json_set(payload,'$.metadata.learning.trace',NULL)",[]).unwrap();
        local_diagnostics::analyze(&db).unwrap();
        let page = local_diagnostics::page(&db, "diagnostics", None, None, None, 0, 20).unwrap();
        let episode = &page["items"][0];
        let id = episode["id"].as_str().unwrap();
        let members =
            local_diagnostics::page(&db, "diagnostic_members", None, None, Some(id), 0, 100)
                .unwrap();
        let refs = members["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["event_ref"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(refs.contains(&"a:1"));
        assert!(
            !refs.contains(&"a:2"),
            "a snapshot cannot recruit another snapshot"
        );
        let encoded = episode["assessment"].to_string();
        assert!(encoded.contains("pool_has_capacity"), "{encoded}");
    });
}

#[test]
fn knowledge_import_is_validated_before_replacement_and_can_recover_corruption() {
    isolated(|| {
        let base = DiagnosticKnowledge::baseline().unwrap();
        let value = serde_json::to_value(&base).unwrap();
        crate::diagnostic_knowledge::store(value.clone()).unwrap();
        let path = crate::config_dir().join("diagnostic-knowledge.json");
        let original = std::fs::read(&path).unwrap();
        let mut invalid = value.clone();
        invalid["evaluation"]["representative"] = json!(true);
        assert!(crate::diagnostic_knowledge::store(invalid).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        crate::diagnostic_knowledge::store(value.clone()).unwrap();
        std::fs::write(&path, b"corrupt file").unwrap();
        assert!(crate::diagnostic_knowledge::load().is_err());
        assert_eq!(
            crate::diagnostic_knowledge::store(value).unwrap()["previous_invalid"],
            true
        );
        assert_eq!(
            crate::diagnostic_knowledge::rollback().unwrap()["revision"],
            base.revision()
        );
    });
}

#[test]
fn diagnostics_keep_exact_scope_members_and_executable_next_queries() {
    isolated(|| {
        let db = database();
        for i in 0..30 {
            insert(&db, i, "a", 1_700_000_000_000 + i * 1000, "B", pool());
            insert(&db, i, "b", 1_700_000_000_000 + i * 1000, "B", pool());
        }
        let report = local_diagnostics::analyze(&db).unwrap();
        assert_eq!(report["episodes"], 2);
        let page =
            local_diagnostics::page(&db, "diagnostics", Some("a"), None, None, 0, 20).unwrap();
        assert_eq!(page["total"], 1);
        let episode = &page["items"][0];
        assert_eq!(
            episode["assessment"]["hypotheses"][0]["diagnosis"],
            "pool_exhaustion"
        );
        assert_eq!(episode["evidence_level"], 0);
        let id = episode["id"].as_str().unwrap();
        let members =
            local_diagnostics::page(&db, "diagnostic_members", Some("a"), None, Some(id), 0, 7)
                .unwrap();
        assert_eq!(members["total"], 30);
        assert_eq!(members["next_offset"], 7);
        assert!(members["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["event_ref"].as_str().unwrap().starts_with("a:")));
        let query = episode["queries"][0]["id"].as_str().unwrap();
        let results = local_diagnostics::page(
            &db,
            "diagnostic_query",
            Some("a"),
            None,
            Some(query),
            0,
            100,
        )
        .unwrap();
        assert_eq!(results["total"], 30);
        assert_eq!(results["overview"][1]["known_outcomes"], 30);
        assert!(local_diagnostics::page(
            &db,
            "diagnostic_query",
            Some("b"),
            None,
            Some(query),
            0,
            20
        )
        .is_err());
        assert!(local_diagnostics::page(
            &db,
            "diagnostic_query",
            None,
            None,
            Some("SELECT private data"),
            0,
            20
        )
        .is_err());
        assert_eq!(report["shared_evaluation"]["representative"], false);
    });
}
#[test]
fn frequent_failures_are_diagnosed_without_an_anomaly_reference_and_gaps_split_episodes() {
    isolated(|| {
        let db = database();
        for i in 0..20 {
            insert(&db, i, "a", 1_700_000_000_000 + i * 1000, "A", pool());
        }
        for i in 20..40 {
            insert(&db, i, "a", 1_700_000_300_000 + i * 1000, "A", pool());
        }
        let report = local_diagnostics::analyze(&db).unwrap();
        assert_eq!(report["episodes"], 2);
        assert_eq!(report["abstentions"], 0);
        let page = local_diagnostics::page(&db, "diagnostics", None, None, None, 0, 100).unwrap();
        assert!(page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["supporting_events"] == 20));
    });
}
#[test]
fn unknown_time_keeps_qualitative_evidence_without_inventing_a_chronology() {
    isolated(|| {
        let db = database();
        insert(&db, 1, "a", 0, "A", pool());
        db.execute("UPDATE investigation_facts SET ts=NULL", [])
            .unwrap();
        let report = local_diagnostics::analyze(&db).unwrap();
        assert_eq!(report["untimed"], 1);
        assert_eq!(report["episodes"], 1);
        let page = local_diagnostics::page(&db, "diagnostics", None, None, None, 0, 20).unwrap();
        let episode = &page["items"][0];
        assert!(episode["first"].is_null());
        assert_eq!(episode["time_known"], false);
        assert!(episode["comparisons"].as_array().unwrap().is_empty());
        assert_eq!(
            episode["assessment"]["hypotheses"][0]["diagnosis"],
            "pool_exhaustion"
        );
        let query = episode["queries"][0]["id"].as_str().unwrap();
        let result =
            local_diagnostics::page(&db, "diagnostic_query", None, None, Some(query), 0, 20)
                .unwrap();
        assert_eq!(result["items"][0]["period"], "time_unknown");
        assert!(result["items"][0]["timestamp"].is_null());
        assert_eq!(result["overview"].as_array().unwrap().len(), 1);
        let id = episode["id"].as_str().unwrap();
        crate::diagnostic_memory::record(&db,id,json!({"verdict":"confirmed","diagnosis":"pool_exhaustion","rationale":"The resource inspection confirmed exhaustion"})).unwrap();
        assert_eq!(
            crate::diagnostic_memory::status().unwrap()["ranking"]["training_episodes"],
            0
        );
    });
}
#[test]
fn version_regression_requires_known_concurrent_controls_and_a_prior_deploy() {
    isolated(|| {
        let db = database();
        insert(
            &db,
            0,
            "a",
            1_700_000_000_000,
            "B",
            DiagnosticRecord {
                deployment: true,
                success: true,
                ..Default::default()
            },
        );
        for i in 1..=120 {
            insert(
                &db,
                i,
                "a",
                1_700_000_060_000 + i * 1000,
                "B",
                DiagnosticRecord {
                    failure: true,
                    ..Default::default()
                },
            );
            insert(
                &db,
                i + 200,
                "a",
                1_700_000_060_000 + i * 1000,
                "A",
                DiagnosticRecord {
                    success: true,
                    ..Default::default()
                },
            );
        }
        local_diagnostics::analyze(&db).unwrap();
        let page =
            local_diagnostics::page(&db, "diagnostics", Some("a"), None, None, 0, 100).unwrap();
        assert!(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["assessment"]["hypotheses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|h| h["diagnosis"] == "deployment_regression")),
            "{page}"
        );
        let db = database();
        for i in 0..80 {
            insert(
                &db,
                i,
                "a",
                1_700_000_000_000 + i * 2000,
                if i < 40 { "" } else { "B" },
                DiagnosticRecord {
                    failure: i >= 40,
                    success: i < 40,
                    ..Default::default()
                },
            );
        }
        local_diagnostics::analyze(&db).unwrap();
        let page = local_diagnostics::page(&db, "diagnostics", None, None, None, 0, 100).unwrap();
        assert!(page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["assessment"]["hypotheses"]
                .as_array()
                .unwrap()
                .iter()
                .all(|h| h["diagnosis"] != "deployment_regression")));
    });
}
#[test]
fn resolution_memory_is_idempotent_editable_and_exports_no_private_values() {
    isolated(|| {
        let db = database();
        insert(&db, 1, "a", 1_700_000_000_000, "A", pool());
        local_diagnostics::analyze(&db).unwrap();
        let page = local_diagnostics::page(&db, "diagnostics", None, None, None, 0, 20).unwrap();
        let id = page["items"][0]["id"].as_str().unwrap();
        let document = json!({"verdict":"confirmed","diagnosis":"pool_exhaustion","rationale":"private-resolution: acquisition stalled while pool was full"});
        let result = crate::diagnostic_memory::record(&db, id, document.clone()).unwrap();
        assert_eq!(result["saved"], true);
        assert_eq!(result["ranking_active"], false);
        let repeated = crate::diagnostic_memory::record(&db, id, document).unwrap();
        assert_eq!(repeated["unchanged"], true);
        assert_eq!(
            crate::diagnostic_memory::status().unwrap()["resolutions"],
            1
        );
        let a = crate::diagnostic_memory::contributions().unwrap();
        let b = crate::diagnostic_memory::contributions().unwrap();
        assert_eq!(a.contributions[0].id, b.contributions[0].id);
        let text = serde_json::to_string(&a).unwrap();
        for secret in [
            "private-resolution",
            "private-checkout",
            "private-host",
            "private-user",
            "192.0.2.3",
            "1700000000000",
            "a:1",
            "timeout acquiring",
        ] {
            assert!(!text.contains(secret), "{secret}");
        }
        crate::diagnostic_memory::record(&db,id,json!({"verdict":"refuted","diagnosis":"pool_exhaustion","rationale":"The independent check disproved the initial diagnosis"})).unwrap();
        let updated = crate::diagnostic_memory::contributions().unwrap();
        assert_ne!(a.contributions[0].id, updated.contributions[0].id);
        assert!(!updated.contributions[0].labels[0].confirmed);
        let consolidated = DiagnosticKnowledge::consolidate(vec![
            a.contributions[0].clone(),
            updated.contributions[0].clone(),
        ])
        .unwrap();
        assert_eq!(consolidated.episodes.len(), 1);
        assert!(!consolidated.episodes.values().next().unwrap().labels[0].confirmed);
        let snapshot = crate::diagnostic_memory::snapshot().unwrap();
        let context = page["items"][0]["scope"]["context"].as_str().unwrap();
        let mask = extract_diagnostic(&pool()).mask;
        assert_eq!(
            crate::diagnostic_memory::precedents(&snapshot, context, mask, "different")
                .unwrap()
                .len(),
            1
        );
        assert!(crate::diagnostic_memory::precedents(
            &snapshot,
            "different-context",
            mask,
            "different"
        )
        .unwrap()
        .is_empty());
        assert!(
            crate::diagnostic_memory::precedents(&snapshot, context, mask, id)
                .unwrap()
                .is_empty()
        );
        assert!(crate::diagnostic_memory::record(&db,"foreign-episode",json!({"verdict":"confirmed","diagnosis":"pool_exhaustion","rationale":"verified elsewhere"})).is_err());
        crate::diagnostic_memory::record(&db,id,json!({"verdict":"inconclusive","rationale":"The external confirmation could not be reproduced"})).unwrap();
        let retracted = crate::diagnostic_memory::contributions().unwrap();
        assert!(retracted.contributions[0].labels.is_empty());
        assert_eq!(retracted.contributions[0].features, 0);
        assert_eq!(
            crate::diagnostic_memory::status().unwrap()["ranking"]["training_episodes"],
            0
        );
        let merged = DiagnosticKnowledge::consolidate(vec![
            a.contributions[0].clone(),
            updated.contributions[0].clone(),
            retracted.contributions[0].clone(),
        ])
        .unwrap();
        assert!(merged.episodes.values().next().unwrap().labels.is_empty());
        assert_eq!(
            crate::diagnostic_memory::status().unwrap()["resolutions"],
            1
        );
    });
}
#[test]
fn native_pipeline_persists_diagnoses_and_model_revision_in_the_analysis() {
    isolated(|| {
        let mut events = Vec::new();
        for i in 0..12 {
            let mut e = crate::model::Event::empty();
            e.id = i;
            e.event_ref = format!("original:{i}");
            e.source = "native-app.log".into();
            e.timestamp = Some(1_700_000_000_000 + i as i64 * 1000);
            e.level = "Erro".into();
            e.message = "timeout acquiring connection".into();
            e.fields=json!({"service.name":"checkout","operation":"checkout","tenantId":"a","event.outcome":"failure","db.pool.active":10,"db.pool.max":10,"db.pool.pending":3,"db.pool.acquire_ms":1000}).as_object().unwrap().clone();
            events.push(e);
        }
        let rules = crate::detections::builtin_ruleset().unwrap();
        let settings = crate::detections::Settings {
            threats: false,
            ..Default::default()
        };
        let result = crate::detections::run_stored(
            &crate::detections::Inputs {
                rules: &rules,
                settings: &settings,
                catalog: None,
            },
            &crate::detections::Source::Events(events.iter().collect()),
        )
        .unwrap();
        assert_eq!(result.metadata["complete"], true);
        assert_eq!(
            result.metadata["investigation"]["diagnostics"]["available"], true,
            "{}",
            result.metadata
        );
        let page = result
            .investigation_page("diagnostics", None, None, None, 0, 100)
            .unwrap();
        assert_eq!(page["total"], 1);
        assert_eq!(
            page["items"][0]["assessment"]["hypotheses"][0]["diagnosis"],
            "pool_exhaustion"
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("analysis.sqlite");
        result.save(&path).unwrap();
        let stored = crate::security_results::Results::open(&path).unwrap();
        let restored = stored
            .investigation_page("diagnostics", None, None, None, 0, 100)
            .unwrap();
        assert_eq!(restored["items"], page["items"]);
    });
}

#[test]
#[ignore = "Benchmark opt-in de 1M fatos normalizados; não executa na suíte essencial"]
fn diagnostics_million_rows_resource_profile() {
    isolated(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("population.sqlite");
        let mut db = Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA cache_size=-2048;PRAGMA mmap_size=0;PRAGMA temp_store=FILE;
        CREATE TABLE investigation_facts(ref TEXT PRIMARY KEY,eid INTEGER,ns TEXT,ts INTEGER,source TEXT,host TEXT,actor TEXT,src TEXT,payload TEXT);").unwrap();
        let count = std::env::var("DIAGNOSTIC_BENCH_ROWS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1_000_000)
            .clamp(10_000, 1_000_000);
        let payloads=(0..128).map(|service|{
        [false,true].map(|failure|{
            let record=if failure{pool()}else{DiagnosticRecord{success:true,..Default::default()}};
            json!({"parse_ok":true,"outcome":if failure{"failure"}else{"success"},"action":"request","metadata":{"product":"application","service":format!("service-{service}"),"time":{"ambiguity":Value::Null},"diagnostics":extract_diagnostic(&record),"learning":{"operation":"checkout","dependency":"database","version":"A"}}}).to_string()
        })
    }).collect::<Vec<_>>();
        {
            let tx = db.transaction().unwrap();
            {
                let mut statement=tx.prepare("INSERT INTO investigation_facts VALUES(?1,?2,'bench',?3,'app.log','host','','',?4)").unwrap();
                for i in 0..count {
                    statement
                        .execute(params![
                            format!("row:{i:010}"),
                            i,
                            1_700_000_000_000i64 + i as i64,
                            payloads[(i % 128) as usize][usize::from(i % 997 == 0)]
                        ])
                        .unwrap();
                }
            }
            tx.commit().unwrap();
        }
        let memory = || {
            let text = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            let get = |name: &str| {
                text.lines()
                    .find_map(|line| {
                        line.strip_prefix(name)
                            .and_then(|s| s.split_whitespace().next())
                            .and_then(|n| n.parse::<u64>().ok())
                    })
                    .unwrap_or(0)
                    * 1024
            };
            (get("VmRSS:"), get("VmHWM:"))
        };
        let (rss_before, peak_before) = memory();
        db.execute_batch("BEGIN").unwrap();
        let start = std::time::Instant::now();
        let report = local_diagnostics::analyze(&db).unwrap();
        db.execute_batch("COMMIT").unwrap();
        let elapsed = start.elapsed().as_millis();
        let (rss_after, peak_after) = memory();
        let added_peak = peak_after.saturating_sub(peak_before);
        assert_eq!(report["valid"], count);
        assert!(report["episodes"].as_u64().unwrap() > 32);
        assert_eq!(report["limited_events"], 0);
        assert!(
            added_peak <= 256 * 1024 * 1024,
            "working peak added {added_peak} bytes"
        );
        let evidence = json!({"rows":count,"contexts":128,"diagnostic_ms":elapsed,"rss_before_bytes":rss_before,"rss_after_bytes":rss_after,"process_peak_bytes":peak_after,"additional_peak_bytes":added_peak,"database_bytes":std::fs::metadata(path).unwrap().len(),"report":report,"scope":"diagnostic layer over synthetic normalized facts, including SQLite population/result work; excludes original import, GUI startup and the rest of triage","build":"native debug, application debuginfo disabled"});
        println!("DIAGNOSTIC_BENCH {evidence}");
        if let Ok(path) = std::env::var("DIAGNOSTIC_BENCH_OUTPUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
    });
}
