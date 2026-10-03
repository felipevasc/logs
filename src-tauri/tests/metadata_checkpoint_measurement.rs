//! Opt-in observation of explicitly selected checkpoint policies in one binary.
//! Native compilation/execution is a prerequisite; see the matching run protocol.
use loginsight_lib::testkit;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::Instant;

fn digest_file(path: &Path) -> String {
    let mut file = BufReader::new(File::open(path).unwrap());
    let mut hash = Sha256::new();
    let mut buffer = [0; 65_536];
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    format!("{:x}", hash.finalize())
}

fn semantic_digest(result: &Value) -> String {
    // Include every materialized Event, offset, metadata field, header and column.
    // A fixed field order avoids dependence on JSON object insertion order.
    let mut hash = Sha256::new();
    for field in ["format", "header", "columns", "metadata", "events", "rows"] {
        hash.update(serde_json::to_vec(&result[field]).unwrap());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}

fn input_rows(path: &Path, small_waves: bool) -> usize {
    let maximum_bytes = if small_waves { 1 << 20 } else { 64 << 20 };
    let bytes = std::fs::metadata(path).unwrap();
    assert!(
        bytes.is_file() && bytes.len() > 0 && bytes.len() <= maximum_bytes,
        "use a regular JSONL fixture no larger than 64 MiB (1 MiB for recovery)"
    );
    let mut reader = BufReader::new(File::open(path).unwrap());
    let mut line = Vec::new();
    let mut count = 0;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).unwrap() == 0 {
            break;
        }
        assert!(
            line.len() <= 1 << 20,
            "individual fixture rows must fit in 1 MiB"
        );
        let row: Value = serde_json::from_slice(&line).expect("one JSON object per physical line");
        assert!(row.is_object(), "only object-per-line JSONL is admitted");
        count += 1;
        assert!(
            count <= if small_waves { 1_000 } else { 100_000 },
            "metadata_probe materializes all events; keep this workload bounded"
        );
    }
    count
}

#[test]
#[ignore = "manual checkpoint measurement; requires explicit fixture, isolated directory and mode"]
fn checkpoint_measurement_workload() {
    let file = std::fs::canonicalize(
        std::env::var("LOGINSIGHT_CHECKPOINT_FILE").expect("set LOGINSIGHT_CHECKPOINT_FILE"),
    )
    .unwrap();
    let root = std::path::PathBuf::from(
        std::env::var("LOGINSIGHT_CHECKPOINT_DIR")
            .expect("set LOGINSIGHT_CHECKPOINT_DIR to a NEW directory for cold/interrupt"),
    );
    let mode =
        std::env::var("LOGINSIGHT_CHECKPOINT_MODE").expect("cold, warm, interrupt or resume");
    assert!(matches!(
        mode.as_str(),
        "cold" | "warm" | "interrupt" | "resume"
    ));
    let small_waves = matches!(mode.as_str(), "interrupt" | "resume");
    let rows = input_rows(&file, small_waves);
    let policy_name = std::env::var("LOGINSIGHT_CHECKPOINT_POLICY").expect("set LOGINSIGHT_CHECKPOINT_POLICY to wave or batched");
    assert!(matches!(policy_name.as_str(), "wave" | "batched"));
    let mut probe_options = json!({"smallWaves":small_waves, "checkpointPolicy":policy_name});
    if small_waves && policy_name == "batched" {
        // Deterministic recovery fixture; never compare this with production timing.
        probe_options["checkpointRows"] = json!(16);
        probe_options["checkpointSourceBytes"] = json!(4096);
        probe_options["checkpointElapsedMs"] = json!(60000);
    }
    let effective_policy = testkit::metadata_checkpoint_policy(&probe_options.to_string()).unwrap();
    let source_sha256 = digest_file(&file); // Outside timing; warms the OS file cache.
    let binary_sha256 = digest_file(&std::env::current_exe().unwrap());
    let workers = std::env::var("LOGINSIGHT_CHECKPOINT_WORKERS").unwrap_or_else(|_| "default".into());
    assert!(matches!(workers.as_str(), "1" | "default"), "workers factor must be 1 or default");
    let fresh = matches!(mode.as_str(), "cold" | "interrupt");
    if fresh { std::fs::create_dir(&root).expect("directory must not exist; its parent must exist"); }
    let data = root.join("data");
    let settings = data.join("resource-settings.json");
    let one_worker = json!({"schemaVersion":1,"mode":"automatic","memoryLimitMib":null,"parallelismLimit":1});
    if fresh {
        std::fs::create_dir(&data).unwrap();
        if workers == "1" {
            let output = OpenOptions::new().write(true).create_new(true).open(&settings).unwrap();
            serde_json::to_writer_pretty(output, &one_worker).unwrap();
        }
    }
    if workers == "1" {
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&settings).unwrap()).unwrap(), one_worker);
    } else { assert!(!settings.exists(), "default scenario must not inherit saved resource settings"); }
    std::env::set_var("LOGINSIGHT_DATA_DIR", &data);
    testkit::init_resources();
    let resources = testkit::resource_snapshot();
    if workers == "1" { assert_eq!(resources["globalParallelism"], 1); assert_eq!(resources["parserThreads"], 1); }
    let context = json!({
        "sourcePath": file, "sourceSha256": source_sha256,
        "sourceBytes": std::fs::metadata(&file).unwrap().len(), "sourceRows": rows,
        "binarySha256": binary_sha256, "smallWaves": small_waves,
        "checkpointPolicy":policy_name, "checkpointThresholds":effective_policy,
        "workersScenario":workers, "resources":resources,
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "availableParallelism": std::thread::available_parallelism().unwrap().get(),
        "memoryLimitMb": std::env::var("LOGINSIGHT_MEMORY_LIMIT_MB").ok(),
        "timezoneEnv": std::env::var("TZ").ok(),
    });
    let predecessor = if fresh {
        None
    } else {
        let previous_mode = if mode == "warm" { "cold" } else { "interrupt" };
        let previous: Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("{previous_mode}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(previous["schemaVersion"], 1);
        assert_eq!(
            previous["context"], context,
            "source, binary and resources must match"
        );
        Some(previous)
    };
    // Refuse overwrite before any probe. Failed runs leave their isolated directory.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(format!("{mode}.json")))
        .unwrap();
    let operation_id = format!("checkpoint-observation-{}", uuid::Uuid::new_v4());
    let observations = RefCell::new(Vec::<Value>::new());
    let interrupted = Cell::new(false);
    let started = Instant::now();
    probe_options["operationId"] = json!(operation_id);
    let result = testkit::metadata_probe(
        file.to_str().unwrap(),
        "jsonl",
        root.join("cache").to_str().unwrap(),
        &probe_options.to_string(),
        &|mut progress| {
            progress["atUs"] = json!(started.elapsed().as_micros() as u64);
            let cancel_here = mode == "interrupt"
                && progress["phaseId"] == "metadata-checkpoint-publish"
                && progress["checkpointRows"].as_u64().unwrap() > 0
                && progress["parsedRows"].as_u64().unwrap()
                    > progress["checkpointRows"].as_u64().unwrap()
                && !interrupted.get();
            observations.borrow_mut().push(progress);
            if cancel_here {
                interrupted.set(true);
                assert!(testkit::cancel_metadata_probe(&operation_id));
            }
        },
    );
    let probe_elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let progress = observations.into_inner();
    let mut report = json!({
        "schemaVersion":1, "mode":mode, "context":context,
        "probeElapsedMs":probe_elapsed_ms, "progress":progress,
        "scope":"Reporter intervals; includes observer overhead. Probe total also materializes all Events. No pure fsync, process-RSS, IPC or UI latency claim.",
    });
    if mode == "interrupt" {
        assert!(
            interrupted.get(),
            "fixture must reach a second nonempty checkpoint"
        );
        report["expectedCancellationError"] = json!(result
            .err()
            .expect("cancelled probe must not return a partial index"));
    } else {
        let result = result.expect("metadata probe must succeed");
        assert_eq!(result["rows"], rows);
        assert_eq!(
            result["metadataResidentRows"], 0,
            "persisted index must be file-backed"
        );
        report["result"] = json!({
            "semanticSha256":semantic_digest(&result), "rows":result["rows"],
            "parsedRows":result["parsedRows"], "resumedRows":result["resumedRows"],
            "checkpointRows":result["checkpointRows"],
        });
        drop(result);
        if mode == "warm" {
            let previous = predecessor.as_ref().unwrap();
            assert_eq!(
                report["result"]["semanticSha256"],
                previous["result"]["semanticSha256"]
            );
            assert_eq!(report["result"]["parsedRows"], 0);
            assert_eq!(report["result"]["resumedRows"], rows);
            assert!(!report["progress"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["phaseId"] == "metadata-checkpoint-write"));
        } else {
            // Oracle work happens after the measurement and is not included in it.
            probe_options.as_object_mut().unwrap().remove("operationId");
            probe_options["uncached"] = json!(true);
            let oracle = testkit::metadata_probe(
                file.to_str().unwrap(),
                "jsonl",
                root.join("cache").to_str().unwrap(),
                &probe_options.to_string(),
                &|_| {},
            )
            .unwrap();
            assert_eq!(report["result"]["semanticSha256"], semantic_digest(&oracle));
            report["uncachedParityVerified"] = json!(true);
        }
        if mode == "resume" {
            let previous = predecessor.as_ref().unwrap();
            let before = previous["progress"].as_array().unwrap();
            let committed = before
                .iter()
                .rev()
                .find(|p| p["phaseId"] == "metadata-checkpoint-committed")
                .unwrap();
            let interrupted = before.last().unwrap();
            let scanned = before
                .iter()
                .rev()
                .find(|p| p["phaseId"] == "metadata-scan")
                .unwrap();
            let durable = committed["checkpointRows"].as_u64().unwrap();
            assert_eq!(report["result"]["resumedRows"], durable);
            assert_eq!(
                report["result"]["parsedRows"].as_u64().unwrap() + durable,
                rows as u64
            );
            report["recovery"] = json!({
                "kind":"cooperative cancellation after data sync, before manifest publication",
                "durableRowsBeforeInterrupt":durable,
                "metadataRowsParsedButNotCommitted":interrupted["parsedRows"].as_u64().unwrap() - durable,
                "sourceBytesScannedButNotCommitted":scanned["completed"].as_u64().unwrap() - committed["completed"].as_u64().unwrap(),
                "resumedRows":report["result"]["resumedRows"],
                "parsedRowsAfterResume":report["result"]["parsedRows"],
                "committedPrefixNotReparsed":true,
            });
        }
    }
    serde_json::to_writer_pretty(&mut output, &report).unwrap();
    output.write_all(b"\n").unwrap();
    println!(
        "CHECKPOINT_REPORT {}",
        root.join(format!("{mode}.json")).display()
    );
}
