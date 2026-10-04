//! Reopen the real import/query path in fresh OS processes. No in-process
//! engine memo can conceal repeated work. Optional rows env supports profiling.
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

#[test]
fn startup_process_worker() {
    let Ok(root) = std::env::var("LOGINSIGHT_STARTUP_TEST_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    std::env::set_var("LOGINSIGHT_DATA_DIR", root.join("data"));
    testkit::set_engine_dir(root.join("engine").to_str().unwrap());
    let paths = [root.join("a.jsonl"), root.join("b.jsonl")];
    let started = Instant::now();
    let source = Source::open(
        &paths
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect::<Vec<_>>(),
        "{}",
        "[]",
    )
    .unwrap();
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    source.prepare().unwrap();
    let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let count = source.count(Engine::Columnar, "[]");
    let page = source.query(Engine::Columnar, "[]", "id", "asc", 0, 5);
    let query_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let analysis = source.startup_analysis().unwrap();
    let analysis_ms = started.elapsed().as_secs_f64() * 1000.0;
    let stores = std::fs::read_dir(root.join("engine"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "duckdb"))
        .map(|entry| {
            let metadata = entry.metadata().unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                json!([
                    metadata.len(),
                    metadata
                        .modified()
                        .unwrap()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                        .to_string()
                ]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let computed = std::fs::read_dir(root.join("data/computed-v1"))
        .unwrap()
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                json!(entry
                    .metadata()
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
                    .to_string()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    std::fs::write(root.join("result.json"), json!({"openMs":open_ms,"prepareMs":prepare_ms,"queryMs":query_ms,"analysisMs":analysis_ms,"analysis":analysis,"computed":computed,"rows":count,"page":page,"stores":stores}).to_string()).unwrap();
}

fn reopen(root: &Path) -> Value {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "startup_process_worker",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("LOGINSIGHT_STARTUP_TEST_ROOT", root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "startup child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&std::fs::read(root.join("result.json")).unwrap()).unwrap()
}
fn remove_receipts(root: &Path) {
    if !root.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(root).unwrap().flatten() {
        if entry.file_type().unwrap().is_dir() {
            remove_receipts(&entry.path());
        } else if entry
            .file_name()
            .to_string_lossy()
            .ends_with(".verified.json")
        {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
}

#[test]
fn unchanged_sources_reopen_without_rebuilding_and_only_changed_sources_are_processed() {
    let root = tempfile::tempdir().unwrap();
    let rows = std::env::var("LOGINSIGHT_STARTUP_BENCH_ROWS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1000);
    for name in ["a.jsonl", "b.jsonl"] {
        let mut output =
            std::io::BufWriter::new(std::fs::File::create(root.path().join(name)).unwrap());
        for row in 0..rows {
            writeln!(output, "{}", json!({"timestamp":1700000000000i64 + row as i64,"level":"info","source":name,"message":format!("persisted event {row}"),"user":format!("user-{}",row%100)})).unwrap();
        }
    }
    // The fixture starts with stable source files, like a previously collected
    // dataset. Recent native change times are deliberately ineligible for reuse.
    #[cfg(any(unix, windows))]
    std::thread::sleep(std::time::Duration::from_millis(2100));
    let cold = reopen(root.path());
    #[cfg(any(unix, windows))]
    std::thread::sleep(std::time::Duration::from_millis(2100));
    remove_receipts(root.path()); // migration/fallback: force a full verification once
    let verified = reopen(root.path());
    let warm = reopen(root.path());
    assert_eq!(cold["rows"], rows * 2);
    assert_eq!(warm["rows"], cold["rows"]);
    assert_eq!(warm["page"], cold["page"]);
    assert_eq!(warm["analysis"], cold["analysis"]);
    assert!(
        cold["computed"].as_object().unwrap().len() >= 4,
        "overview, profiles and both source hashes must be durable"
    );
    assert_eq!(
        warm["computed"], cold["computed"],
        "startup must reuse saved analyses without rewriting/recomputing them"
    );
    assert_eq!(
        warm["stores"], cold["stores"],
        "warm startup must preserve the original checkpoint files"
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(root.path().join("b.jsonl"))
        .unwrap()
        .write_all(b"{\"message\":\"new event\",\"timestamp\":1800000000000}\n")
        .unwrap();
    let changed = reopen(root.path());
    assert_eq!(changed["rows"], rows * 2 + 1);
    assert_eq!(
        changed["analysis"]["overview"]["total"],
        rows * 2 + 1,
        "changed data must not reuse the previous overview"
    );
    assert_eq!(
        changed["analysis"]["hashes"][0], cold["analysis"]["hashes"][0],
        "A's hash remains valid after B changes"
    );
    assert_ne!(
        changed["analysis"]["hashes"][1]["sha256"], cold["analysis"]["hashes"][1]["sha256"],
        "B's new content requires a new hash"
    );
    assert_eq!(changed["page"]["rows"], cold["page"]["rows"]);
    for (name, stamp) in cold["stores"].as_object().unwrap() {
        assert_eq!(
            &changed["stores"][name], stamp,
            "adding data to B must preserve completed checkpoints, including A"
        );
    }
    for (name, stamp) in cold["computed"].as_object().unwrap() {
        assert_eq!(
            &changed["computed"][name], stamp,
            "unchanged source hashes and completed results remain available"
        );
    }
    assert!(
        changed["stores"].as_object().unwrap().len() > cold["stores"].as_object().unwrap().len()
    );
    println!(
        "STARTUP_REUSE {}",
        json!({"rows":rows*2,"coldMs":cold["openMs"].as_f64().unwrap()+cold["prepareMs"].as_f64().unwrap(),"fullVerificationMs":verified["openMs"].as_f64().unwrap()+verified["prepareMs"].as_f64().unwrap(),"reuseMs":warm["openMs"].as_f64().unwrap()+warm["prepareMs"].as_f64().unwrap(),"coldAnalysisMs":cold["analysisMs"],"reuseAnalysisMs":warm["analysisMs"],"queryMs":warm["queryMs"]})
    );
}
