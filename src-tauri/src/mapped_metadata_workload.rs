//! Ignored library acceptance workload for the preserved synthetic metadata cache.
//! Run only through the external observer in the coordinated resource window.
//! Uses the application testkit and never asks the engine to prepare stores.
use crate::testkit::{self, Source};
use serde_json::{json, Value};
use std::time::Instant;
fn proc_values(path: &str) -> Value {
    let mut values = serde_json::Map::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let fields: Vec<_> = value.split_whitespace().collect();
                if fields.get(1) == Some(&"kB") {
                    if let Ok(number) = fields[0].parse::<u64>() {
                        values.insert(key.into(), json!(number * 1024));
                    }
                }
            }
        }
    }
    Value::Object(values)
}
fn emit(operation: &str, started: Instant, detail: Value) {
    println!(
        "BENCH {}",
        json!({"operation":operation,"elapsedMs":started.elapsed().as_secs_f64()*1000.0,"detail":detail})
    );
}
/// Refuse parser/calendar/schema drift before Source::open can write a new
/// journal. This touches only the source's small identity samples and manifests;
/// payload checksums remain the reader/observer's measured validation work.
fn expected_stores(rows: usize) -> usize {
    if rows == 1_000_000 {
        2
    } else {
        52
    }
}

fn admit_rows(manifest: &Value) -> usize {
    let expected = manifest["rows"].as_u64().unwrap() as usize;
    let requested = std::env::var("LOGINSIGHT_BENCH_ROWS")
        .unwrap_or_else(|_| "1000000".into())
        .parse::<usize>()
        .unwrap();
    assert_eq!(
        requested, expected,
        "fixture differs from requested acceptance scale"
    );
    assert!(
        expected == 1_000_000
            || (expected == 50_000_000
                && std::env::var("LOGINSIGHT_BENCH_ENABLE_50M").as_deref() == Ok("1")),
        "default acceptance is 1M; 50M requires an additional explicit opt-in and user request"
    );
    expected
}

fn preserved_preflight(file: &str, cache: &str, manifest: &Value) {
    assert_eq!(crate::index_cache::INDEX_DIR, "indexes-v6");
    assert_eq!(crate::index_cache::METADATA_DIR, "metadata-v1");
    assert_eq!(crate::metadata_checkpoint::VERSION, 1);
    assert!(
        crate::load_ts_config(file).is_none(),
        "fixture must keep its original timestamps"
    );
    let prepared =
        crate::sources::prepare_index(file, "auto", None, None).expect("read-only parser identity");
    assert_eq!(prepared.part.format, "jsonl");
    let key = prepared.key().expect("metadata key");
    let directory = std::path::Path::new(cache).join("data/indexes-v6/metadata-v1");
    let text = std::fs::read_to_string(directory.join(format!("{key}.state"))).expect(
        "current parser/calendar must match the preserved complete journal; no rebuild permitted",
    );
    let envelope: Value = serde_json::from_str(&text).unwrap();
    let state = &envelope["state"];
    assert_eq!(state["version"], 1);
    assert_eq!(state["key"], key);
    assert_eq!(state["source_bytes"], manifest["bytes"]);
    assert_eq!(state["sealed_rows"], manifest["rows"]);
    assert_eq!(state["scan_complete"], true);
    assert_eq!(state["cursor"], manifest["bytes"]);
    assert_eq!(state["tail"], Value::Null);
    assert_eq!(
        state["payload_bytes"].as_u64().unwrap(),
        72 + manifest["rows"].as_u64().unwrap() * 27
    );
    assert_eq!(
        std::fs::metadata(directory.join(format!("{key}.lines")))
            .unwrap()
            .len(),
        state["payload_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        state["columns"],
        json!([
            "timestamp",
            "source",
            "level",
            "code",
            "name",
            "description",
            "message",
            "latency_ms",
            "mixed_sort",
            "request.method",
            "request.path",
            "trace_id",
            "user",
            "@user",
            "arquivo",
            "caminho"
        ])
    );
    if manifest["rows"] == 50_000_000 {
        assert_eq!(
            key,
            "ba8a0852a2ccb77f444461a4b6927b68bdba3f5e0a17edfcb71b5959b2d0e0cf"
        );
        assert_eq!(
            state["payload_sha256"],
            "bf16517f52f41596c5d2845f9bfa20568c6c88fcef18890fe2b013a17c5f12ed"
        );
    }
    let mut stores = 0;
    let mut rows = 0;
    for entry in std::fs::read_dir(std::path::Path::new(cache).join("engine")).unwrap() {
        let entry = entry.unwrap();
        if !entry
            .file_name()
            .to_string_lossy()
            .ends_with(".complete.json")
        {
            continue;
        }
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
        assert_eq!(value["version"], 5);
        assert_eq!(value["identity"], prepared.part.identity);
        rows += value["rows"].as_u64().unwrap();
        stores += 1;
    }
    assert_eq!(rows, manifest["rows"].as_u64().unwrap());
    assert_eq!(
        stores,
        expected_stores(manifest["rows"].as_u64().unwrap() as usize)
    );
    println!(
        "BENCH {}",
        json!({"operation":"preserved_preflight","metadataKey":key,"parser":prepared.part.format,"calendar":prepared.part.calendar,"engineStores":stores,"rows":rows})
    );
}
#[test]
#[ignore = "requires explicit preserved fixture/cache and a coordinated CPU window"]
fn mapped_warm_workload() {
    let file = std::env::var("LOGINSIGHT_BENCH_FILE").expect("fixture required");
    let cache = std::env::var("LOGINSIGHT_BENCH_DIR").expect("cache required");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{file}.manifest.json")).unwrap())
            .unwrap();
    assert_eq!(
        std::env::var("LOGINSIGHT_TIME_PRECOMPUTE").as_deref(),
        Ok("0"),
        "preserved caches may not receive optional sidecars"
    );
    assert_eq!(manifest["generatorVersion"], 2);
    assert_eq!(manifest["idOrder"], "permuted");
    assert_eq!(manifest["seed"], 42);
    assert_eq!(manifest["padding"], 0);
    assert_eq!(manifest["characteristics"]["timestampTies"], 10);
    assert_eq!(manifest["characteristics"]["rareNeedleEvery"], 100_000);
    assert_eq!(manifest["characteristics"]["uniqueHexIds"], true);
    let expected = admit_rows(&manifest);
    assert_eq!(
        std::fs::metadata(&file).unwrap().len(),
        manifest["bytes"].as_u64().unwrap()
    );
    std::env::set_var("LOGINSIGHT_DATA_DIR", format!("{cache}/data"));
    testkit::set_engine_dir(&format!("{cache}/engine"));
    testkit::init_resources();
    // Library tests disable the columnar engine by default; enable reads without
    // calling prepare() or scheduling any background cache construction.
    crate::engine::set_enabled(true);
    let metadata_only = std::env::var("LOGINSIGHT_METADATA_ONLY").as_deref() == Ok("1");
    // Metadata-only still reads the preserved warm fixture; cold/resume probes
    // use a different, explicitly approved cache and harness.
    preserved_preflight(&file, &cache, &manifest);
    let started = Instant::now();
    let source = Source::open(&[&file], "{}", "[]").expect("metadata reopen");
    let metadata_ms = started.elapsed().as_secs_f64() * 1000.0;
    // Take these snapshots synchronously before any engine status/query work.
    let rollup = proc_values("/proc/self/smaps_rollup");
    let status = proc_values("/proc/self/status");
    println!(
        "BENCH {}",
        json!({"operation":"metadata_open","elapsedMs":metadata_ms,"detail":{"rows":source.len(),"metadataStorage":source.metadata_storage(),"memoryRollupBytes":rollup,"processStatusBytes":status,"memoryLimitIsEngineBudgetOnly":true}})
    );
    assert_eq!(source.len(), expected);
    let storage = source.metadata_storage();
    assert_eq!(storage["residentRows"], 0);
    assert_eq!(storage["mappedRows"], json!(expected));
    assert_eq!(storage["residentPayloadBytes"], 0);
    let ids = [0, expected / 2, expected - 1];
    let samples = source.events_at(&ids);
    assert_eq!(
        samples.iter().map(|event| event.id).collect::<Vec<_>>(),
        ids
    );
    assert_eq!(samples[1].fields["trace_id"], manifest["pointLookupId"]);
    assert!(samples.iter().all(|event| !event.event_ref.is_empty()));
    println!(
        "BENCH {}",
        json!({"operation":"metadata_samples","ids":ids,"events":samples})
    );
    if metadata_only {
        return;
    }
    let started = Instant::now();
    let readiness = source.engine_status();
    emit("ready_cache_validation", started, readiness.clone());
    assert_eq!(readiness["baseReady"], true, "refusing any engine rebuild");
    assert_eq!(
        readiness["derivedReady"], true,
        "refusing any engine rebuild"
    );
    assert_eq!(readiness["completedRows"], json!(expected));
    assert_eq!(readiness["totalRows"], json!(expected));
    assert_eq!(
        readiness["completedSegments"],
        json!(expected_stores(expected))
    );
    assert_eq!(readiness["totalSegments"], json!(expected_stores(expected)));
    let exact =
        json!([{"column":"trace_id","op":"equals_exact","value":manifest["pointLookupId"]}])
            .to_string();
    let rare = r#"[{"column":"_all","op":"query","value":"rareneedle"}]"#;
    for (name, filters, count) in [
        ("all", "[]", expected),
        ("rare_exact_id", exact.as_str(), 1),
        ("rare_substring", rare, expected.div_ceil(100_000)),
    ] {
        let started = Instant::now();
        let actual = source
            .try_count(filters)
            .expect("exact count within configured engine budget");
        emit(
            "exact_count",
            started,
            json!({"workload":name,"count":actual}),
        );
        assert_eq!(actual, count);
    }
    // Fixture timestamps tie in groups of ten; final ID tie-break is always ascending.
    assert_eq!(expected % 10, 0);
    for direction in ["asc", "desc"] {
        let started = Instant::now();
        let first = source
            .try_page("[]", "timestamp", direction, 0, 100, None)
            .unwrap();
        assert_eq!(first["engine"], "columnar");
        let rows = first["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 100);
        let wanted: Vec<_> = if direction == "asc" {
            (0..100).collect()
        } else {
            (0..100)
                .map(|rank| (expected / 10 - 1 - rank / 10) * 10 + rank % 10)
                .collect()
        };
        assert_eq!(
            rows.iter()
                .map(|row| row["id"].as_u64().unwrap() as usize)
                .collect::<Vec<_>>(),
            wanted
        );
        emit(
            "first_rows",
            started,
            json!({"workload":"all","sortDir":direction,"returned":100}),
        );
        let cursor = first["nextCursor"].as_str().expect("continuation");
        let started = Instant::now();
        let second = source
            .try_page("[]", "timestamp", direction, 100, 100, Some(cursor))
            .unwrap();
        let rows = second["rows"].as_array().unwrap();
        let wanted: Vec<_> = if direction == "asc" {
            (100..200).collect()
        } else {
            (100..200)
                .map(|rank| (expected / 10 - 1 - rank / 10) * 10 + rank % 10)
                .collect()
        };
        assert_eq!(
            rows.iter()
                .map(|row| row["id"].as_u64().unwrap() as usize)
                .collect::<Vec<_>>(),
            wanted
        );
        emit(
            "cursor_next_page",
            started,
            json!({"workload":"all","sortDir":direction,"returned":rows.len()}),
        );
    }
    let exact_page = source
        .try_page(&exact, "timestamp", "asc", 0, 100, None)
        .unwrap();
    assert_eq!(exact_page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(exact_page["rows"][0]["id"], json!(expected / 2));
}

/// The existing 1M stores use schema4. Prepare schema5 only in an explicitly
/// new empty acceptance cache, never over either retained corpus/cache.
#[test]
#[ignore = "requires explicit new empty 1M cache, disk headroom and coordinated window"]
fn prepare_one_million_cache() {
    assert_eq!(
        std::env::var("LOGINSIGHT_BENCH_PREPARE_NEW").as_deref(),
        Ok("1")
    );
    assert_eq!(
        std::env::var("LOGINSIGHT_TIME_PRECOMPUTE").as_deref(),
        Ok("0")
    );
    let file = std::env::var("LOGINSIGHT_BENCH_FILE").expect("explicit 1M fixture required");
    let cache = std::env::var("LOGINSIGHT_BENCH_DIR").expect("explicit new empty cache required");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{file}.manifest.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["rows"], 1_000_000,
        "preparation is permitted only for the 1M gate"
    );
    assert_eq!(admit_rows(&manifest), 1_000_000);
    assert_eq!(manifest["generatorVersion"], 2);
    assert_eq!(manifest["idOrder"], "permuted");
    let directory = std::path::Path::new(&cache);
    assert!(
        directory.is_dir(),
        "create an isolated cache directory explicitly first"
    );
    assert!(
        std::fs::read_dir(directory).unwrap().next().is_none(),
        "refusing to alter an existing cache"
    );
    assert!(
        !std::path::Path::new(&file).starts_with(directory),
        "source must be outside the new cache"
    );
    std::env::set_var("LOGINSIGHT_DATA_DIR", directory.join("data"));
    testkit::set_engine_dir(&directory.join("engine").to_string_lossy());
    testkit::init_resources();
    crate::engine::set_enabled(true);
    let started = Instant::now();
    let source = Source::open(&[&file], "{}", "[]").expect("bounded 1M metadata preparation");
    emit(
        "metadata_prepare_1m",
        started,
        json!({"rows":source.len(),"metadataStorage":source.metadata_storage(),"memoryRollupBytes":proc_values("/proc/self/smaps_rollup")}),
    );
    assert_eq!(source.len(), 1_000_000);
    assert_eq!(source.metadata_storage()["residentRows"], 0);
    let started = Instant::now();
    source.prepare().expect("bounded 1M engine preparation");
    let readiness = source.engine_status();
    emit("engine_prepare_1m", started, readiness.clone());
    assert_eq!(readiness["baseReady"], true);
    assert_eq!(readiness["derivedReady"], true);
    assert_eq!(readiness["completedRows"], 1_000_000);
    assert_eq!(readiness["completedSegments"], 2);
    preserved_preflight(&file, &cache, &manifest);
}
