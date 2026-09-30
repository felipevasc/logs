//! Manual end-to-end backend workload; never creates a large input in CI.
//! See docs/desempenho-grandes-volumes.md for process/cache controls.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::time::Instant;

fn record(operation: &str, started: Instant, detail: Value) {
    println!(
        "BENCH {}",
        json!({"operation": operation, "elapsedMs": started.elapsed().as_secs_f64() * 1000.0, "detail": detail})
    );
}

#[test]
#[ignore = "requires explicit LOGINSIGHT_BENCH_FILE and LOGINSIGHT_BENCH_DIR"]
fn interactive_workload() {
    let file = std::env::var("LOGINSIGHT_BENCH_FILE").expect("set LOGINSIGHT_BENCH_FILE");
    let dir = std::env::var("LOGINSIGHT_BENCH_DIR").expect("set an isolated LOGINSIGHT_BENCH_DIR");
    let repeats: usize = std::env::var("LOGINSIGHT_BENCH_REPEATS")
        .unwrap_or_else(|_| "10".into())
        .parse()
        .expect("integer repeats");
    assert!((1..=100).contains(&repeats), "repeats must be 1..100");
    let sort_dir = std::env::var("LOGINSIGHT_BENCH_SORT_DIR").unwrap_or_else(|_| "asc".into());
    assert!(matches!(sort_dir.as_str(), "asc" | "desc"), "sort direction must be asc or desc");
    std::env::set_var("LOGINSIGHT_DATA_DIR", format!("{dir}/data"));
    testkit::set_engine_dir(&format!("{dir}/engine"));
    std::env::set_var("LOGINSIGHT_ENGINE_TRACE", "1");
    testkit::init_resources();
    let start = Instant::now();
    let source = Source::open(&[&file], "{}", "[]").expect("open fixture");
    record(
        "metadata_open",
        start,
        json!({"rows": source.len(), "sourceBytes": std::fs::metadata(&file).unwrap().len(), "sortColumn":"timestamp", "sortDir":sort_dir, "cacheState": std::env::var("LOGINSIGHT_BENCH_CACHE_STATE").unwrap_or_else(|_| "unspecified".into())}),
    );
    if let Ok(expected) = std::env::var("LOGINSIGHT_BENCH_ROWS") {
        assert_eq!(
            source.len(),
            expected.parse::<usize>().expect("integer expected rows")
        );
    }
    let start = Instant::now();
    source
        .prepare()
        .expect("complete columnar/text preparation");
    record("prepare", start, json!({"rows": source.len()}));

    let fixture: Value = std::fs::read_to_string(format!("{file}.manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let point_id = std::env::var("LOGINSIGHT_BENCH_ID")
        .ok()
        .or_else(|| fixture["pointLookupId"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{:032x}", source.len() / 2));
    println!(
        "BENCH {}",
        json!({"operation":"fixture_identity", "generatorVersion":fixture["generatorVersion"],"idOrder":fixture["idOrder"],"pointLookupId":point_id})
    );
    let workloads = [
        ("all", "[]".to_string()),
        ("rare_exact_id", json!([{"column":"trace_id","op":"equals_exact","value":point_id}]).to_string()),
        ("service_and_level", json!([{"column":"source","op":"equals_exact","value":"service-1"},{"column":"level","op":"equals_exact","value":"Erro"}]).to_string()),
        ("rare_substring", json!([{"column":"_all","op":"query","value":"rareneedle"}]).to_string()),
        ("rare_hex_free", json!([{"column":"_all","op":"query","value":point_id}]).to_string()),
    ];
    for (name, filters) in workloads {
        let mut timings = Vec::with_capacity(repeats);
        for iteration in 0..repeats {
            let start = Instant::now();
            let result = source.page(&filters, "timestamp", &sort_dir, 0, 100, None);
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            timings.push(elapsed_ms);
            record(
                "first_rows",
                start,
                json!({"workload":name,"iteration":iteration,"returned":result["rows"].as_array().unwrap().len(),"hasMore":result["hasMore"],"engine":result["engine"]}),
            );
            if let Some(cursor) = result["nextCursor"].as_str() {
                let start = Instant::now();
                let next = source.page(&filters, "timestamp", &sort_dir, 100, 100, Some(cursor));
                record(
                    "cursor_next_page",
                    start,
                    json!({"workload":name,"iteration":iteration,"returned":next["rows"].as_array().unwrap().len()}),
                );
            }
        }
        timings.sort_by(f64::total_cmp);
        println!(
            "BENCH {}",
            json!({"operation":"first_rows_distribution","workload":name,"samples":repeats,"p50Ms":timings[(repeats - 1) / 2],"p95Ms":timings[((repeats as f64 * 0.95).ceil() as usize - 1).min(repeats - 1)]})
        );
        if std::env::var_os("LOGINSIGHT_BENCH_EXPLAIN").is_some() {
            let plan = source
                .explain_page(&filters, "timestamp", &sort_dir, 100, true)
                .expect("explain interactive plan");
            println!(
                "BENCH {}",
                json!({"operation":"explain_analyze","workload":name,"plan":plan})
            );
        }
        let start = Instant::now();
        let count = source.count(Engine::Columnar, &filters);
        record("exact_count", start, json!({"workload":name,"count":count}));
        let start = Instant::now();
        let stats = source.stats(Engine::Columnar, &filters);
        record("statistics", start, json!({"workload":name,"result":stats}));
    }
}
