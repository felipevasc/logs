//! Real-process recovery plus exact metadata/materialized-event parity. Tiny
//! waves and explicit tiny durability thresholds exercise the production policy
//! without generating large fixtures. smallWaves alone does not change policy.
use loginsight_lib::testkit;
use serde_json::{json, Value};
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn probe(
    path: &Path,
    format: &str,
    cache: &Path,
    options: &Value,
    progress: &dyn Fn(Value),
) -> Result<Value, String> {
    let mut options = options.clone();
    if options["smallWaves"].as_bool().unwrap_or(true) {
        for (key, value) in [("checkpointRows", 8), ("checkpointSourceBytes", 1024), ("checkpointElapsedMs", 60000)] {
            if options.get(key).is_none() { options[key] = value.into(); }
        }
    }
    testkit::metadata_probe(
        path.to_str().unwrap(),
        format,
        cache.to_str().unwrap(),
        &options.to_string(),
        progress,
    )
}
fn equivalent(actual: &Value, expected: &Value) {
    for field in ["format", "header", "columns", "metadata", "events", "rows"] {
        assert_eq!(actual[field], expected[field], "{field}");
    }
}
fn oracle(path: &Path, format: &str, cache: &Path, mut options: Value) -> Value {
    options["uncached"] = true.into();
    options.as_object_mut().unwrap().remove("operationId");
    probe(path, format, cache, &options, &|_| {}).unwrap()
}
fn files(cache: &Path, extension: &str) -> Vec<PathBuf> {
    std::fs::read_dir(cache)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == extension))
        .collect()
}
fn jsonl(rows: usize) -> String {
    (0..rows).map(|i| format!("{{\"timestamp\":\"2026-09-30T12:00:00Z\",\"code\":\"C{i}\",\"message\":\"event {i}\",\"nested\":{{\"position\":{i}}}}}\n")).collect()
}
fn array(rows: usize) -> String {
    format!("\u{feff}[\n{}\n]", (0..rows).map(|i| json!({"timestamp":"2026-09-30T12:00:00Z", "message":format!("event {i} quoted \\\" ] }}"), "nested":[{"position":i}]}).to_string()).collect::<Vec<_>>().join(",\n"))
}

#[test]
fn every_canonical_parser_resumes_with_exact_offsets_ids_headers_and_columns() {
    let root = tempfile::tempdir().unwrap();
    let mut multiline =
        "\tat orphan.first(Start.java:1)\r\n\tat orphan.second(Start.java:2)\r\n".to_string();
    for i in 0..44 {
        multiline.push_str(&format!(
            "2026-09-30 12:00:{:02},001 ERROR [main] a.Service - failure {i}\r\n",
            i % 60
        ));
        for frame in 0..i % 9 {
            multiline.push_str(&format!("\tat a.Service.call(Service.java:{frame})\r\n"));
        }
    }
    multiline.push_str("\tat final.unterminated(Service.java:9)");
    let mut snapshots = String::new();
    for i in 0..44 {
        let mut event = testkit::Event::empty();
        event.message = format!("snapshot {i}");
        event.raw = format!("original {i}");
        event.event_ref = format!("explicit:{i}");
        snapshots.push_str(&serde_json::to_string(&event).unwrap());
        snapshots.push('\n');
    }
    let cases = vec![
        ("jsonl", "jsonl", jsonl(73), json!({})),
        (
            "text",
            "text",
            format!(
                "\n{}final no newline",
                (0..73)
                    .map(|i| format!("2026-09-30 12:00:00 INFO message {i}\r\n\r\n"))
                    .collect::<String>()
            ),
            json!({}),
        ),
        (
            "csv",
            "csv",
            format!(
                "\u{feff}Data;Nível;Mensagem\r\n{}",
                (0..73)
                    .map(|i| format!("30/09/2026 12:00:00;Erro;\"message; {i}\"\r\n"))
                    .collect::<String>()
            ),
            json!({}),
        ),
        (
            "tsv",
            "tsv",
            format!(
                "timestamp\tlevel\tmessage\n{}",
                (0..73)
                    .map(|i| format!("2026-09-30T12:00:00Z\tERROR\tmessage {i}\n"))
                    .collect::<String>()
            ),
            json!({}),
        ),
        (
            "w3c",
            "w3c",
            format!(
                "#Software: test\n#Fields: date time cs-method cs-uri-stem sc-status\n{}",
                (0..73)
                    .map(|i| format!("# comment {i}\n2026-09-30 12:00:00 GET /{i} 200\n"))
                    .collect::<String>()
            ),
            json!({}),
        ),
        (
            "zeek",
            "zeek",
            format!(
                "#separator \\x09\n#fields\tts\tuid\tid.orig_h\n{}",
                (0..73)
                    .map(|i| format!("# comment\n1790769600\tuid{i}\t10.0.0.1\n"))
                    .collect::<String>()
            ),
            json!({}),
        ),
        (
            "regex",
            "custom",
            (0..73)
                .map(|i| format!("2026-09-30T12:00:00Z ERROR {i} message {i}\n"))
                .collect(),
            json!({"regex":r"^(?P<timestamp>\S+) (?P<level>\S+) (?P<code>\d+) (?P<message>.*)$"}),
        ),
        (
            "delimiter",
            "custom",
            (0..73)
                .map(|i| format!("2026-09-30T12:00:00Z|ERROR|{i}|message {i}\n"))
                .collect(),
            json!({"delimiter":"|","fields":["timestamp","level","code","message"]}),
        ),
        ("multiline", "log4j", multiline, json!({})),
        ("array", "jsonl", array(73), json!({})),
        (
            "envelope",
            "jsonl",
            format!(
                "{{\"before\":true,\"Records\":{},\"after\":true}}",
                array(73).trim_start_matches('\u{feff}')
            ),
            json!({}),
        ),
        ("snapshot", "snapshot", snapshots, json!({})),
        (
            "syslog",
            "syslog3164",
            (0..73)
                .map(|i| format!("Sep 30 12:00:00 host app: INFO message {i}\n"))
                .collect(),
            json!({"year":2032}),
        ),
        (
            "firewall",
            "firewall",
            (0..73)
                .map(|i| format!("Sep 30 12:00:00 host kernel: SRC=10.0.0.1 DST=10.0.0.2 ID={i}\n"))
                .collect(),
            json!({"year":2032}),
        ),
    ];
    for (name, format, contents, options) in cases {
        let path = root.path().join(format!("{name}.log"));
        std::fs::write(&path, contents).unwrap();
        let cache = root.path().join(format!("{name}-cache"));
        let expected = oracle(&path, format, &cache, options.clone());
        let id = format!("metadata-{name}-{}", uuid::Uuid::new_v4());
        let mut interrupted = options.clone();
        interrupted["operationId"] = id.clone().into();
        let killed = Cell::new(false);
        let result = probe(&path, format, &cache, &interrupted, &|p| {
            if p["phaseId"] == "metadata-checkpoint-committed"
                && p["completed"].as_u64().unwrap() < p["total"].as_u64().unwrap()
                && !killed.replace(true)
            {
                testkit::cancel_metadata_probe(&id);
            }
        });
        assert!(killed.get(), "{name} needs a real intermediate checkpoint");
        assert!(result.is_err(), "{name} must not publish a partial index");
        let resumed = probe(&path, format, &cache, &options, &|_| {}).unwrap();
        equivalent(&resumed, &expected);
        assert_eq!(resumed["metadataResidentRows"], 0, "{name} resumed metadata must be file-backed");
        assert!(
            resumed["parsedRows"].as_u64().unwrap() < resumed["rows"].as_u64().unwrap(),
            "{name} reparsed a prefix"
        );
        let warm = probe(&path, format, &cache, &options, &|_| {}).unwrap();
        equivalent(&warm, &expected);
        assert_eq!(warm["parsedRows"], 0, "{name} warm metadata parse");
        assert_eq!(warm["metadataResidentRows"], 0, "{name} warm metadata must be file-backed");
        assert_eq!(warm["resumedRows"], warm["rows"]);
    }
}

#[test]
fn multiline_tail_can_resume_with_no_sealed_record_yet() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("one-long-event.log");
    let text = format!("2026-09-30 12:00:00,001 ERROR [main] a.Service - first\n{}2026-09-30 12:00:01,001 INFO [main] a.Service - second\n", (0..70).map(|i| format!("\tat a.Service.call(Service.java:{i})\n")).collect::<String>());
    std::fs::write(&path, text).unwrap();
    let cache = root.path().join("cache");
    let expected = oracle(&path, "log4j", &cache, json!({}));
    let id = format!("tail-{}", uuid::Uuid::new_v4());
    let cancelled = Cell::new(false);
    assert!(
        probe(&path, "log4j", &cache, &json!({"operationId":id}), &|p| {
            if p["phaseId"] == "metadata-checkpoint-committed" && !cancelled.replace(true) {
                assert_eq!(p["checkpointRows"], 0);
                testkit::cancel_metadata_probe(&id);
            }
        })
        .is_err()
    );
    let resumed = probe(&path, "log4j", &cache, &json!({}), &|_| {}).unwrap();
    equivalent(&resumed, &expected);
    assert_eq!(
        resumed["resumedRows"], 0,
        "unsealed tail must not be reported as a complete record"
    );
    assert_eq!(
        resumed["parsedRows"], 1,
        "persisted first-line metadata must not be reparsed"
    );
}

#[test]
fn cancellation_at_each_commit_gate_restores_exactly_the_manifest_prefix() {
    for phase in ["metadata-checkpoint-write", "metadata-checkpoint-sync", "metadata-checkpoint-publish",
                  "metadata-checkpoint-manifest-ready", "metadata-checkpoint-committed"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.jsonl");
        std::fs::write(&path, jsonl(73)).unwrap();
        let cache = root.path().join("cache");
        let expected = oracle(&path, "jsonl", &cache, json!({}));
        let id = format!("commit-gate-{}", uuid::Uuid::new_v4());
        let durable = Cell::new(0u64);
        let cancelled = Cell::new(false);
        let result = probe(&path, "jsonl", &cache, &json!({"operationId":id}), &|p| {
            let target = p["phaseId"] == phase && durable.get() > 0 && !cancelled.get();
            if p["phaseId"] == "metadata-checkpoint-committed" {
                durable.set(p["checkpointRows"].as_u64().unwrap());
            }
            if target {
                cancelled.set(true);
                assert!(testkit::cancel_metadata_probe(&id));
            }
        });
        assert!(cancelled.get() && result.is_err(), "{phase} did not cancel at the second checkpoint");
        let state: Value = serde_json::from_slice(&std::fs::read(files(&cache, "state").pop().unwrap()).unwrap()).unwrap();
        assert_eq!(state["state"]["sealed_rows"], durable.get(), "{phase} published an unexpected prefix");
        let resumed = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
        equivalent(&resumed, &expected);
        assert_eq!(resumed["resumedRows"], durable.get());
        assert_eq!(resumed["parsedRows"].as_u64().unwrap() + durable.get(), 73);
        assert_eq!(std::fs::metadata(files(&cache, "lines").pop().unwrap()).unwrap().len(), 72 + 27 * 73);
        let warm = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
        equivalent(&warm, &expected);
        assert_eq!(warm["parsedRows"], 0);
    }
}

#[test]
fn deferred_small_waves_and_rollback_preserve_identical_results() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.jsonl");
    std::fs::write(&path, jsonl(73)).unwrap();
    let mut results = Vec::new();
    let mut counts = Vec::new();
    for (name, options) in [
        ("wave", json!({"checkpointPolicy":"wave", "checkpointRows":1, "checkpointSourceBytes":1})),
        ("batched", json!({"checkpointPolicy":"batched", "checkpointRows":16, "checkpointSourceBytes":4096})),
    ] {
        let cache = root.path().join(name);
        let waves = Cell::new(0);
        let commits = Cell::new(0);
        let result = probe(&path, "jsonl", &cache, &options, &|p| {
            if p["phaseId"] == "metadata-scan" && p["parsedRows"].as_u64().unwrap() > 0 { waves.set(waves.get() + 1); }
            if p["phaseId"] == "metadata-checkpoint-committed" { commits.set(commits.get() + 1); }
        }).unwrap();
        assert!(commits.get() > 2, "fixture must cross multiple data checkpoints");
        counts.push((waves.get(), commits.get()));
        results.push(result);
    }
    equivalent(&results[0], &results[1]);
    assert_eq!(counts[0].0, counts[1].0, "policy must not change parser waves");
    assert!(counts[1].1 < counts[0].1, "batched policy must defer small waves");
}

#[test]
fn raw_eof_checkpoint_survives_cancelled_column_discovery_and_restore() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.jsonl");
    std::fs::write(&path, jsonl(73)).unwrap();
    let cache = root.path().join("cache");
    let expected = oracle(&path, "jsonl", &cache, json!({}));
    let id = format!("columns-{}", uuid::Uuid::new_v4());
    assert!(
        probe(&path, "jsonl", &cache, &json!({"operationId":id}), &|p| {
            if p["phaseId"] == "metadata-columns" {
                testkit::cancel_metadata_probe(&id);
            }
        })
        .is_err()
    );
    let restored = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    equivalent(&restored, &expected);
    assert_eq!(restored["parsedRows"], 0);
    let id = format!("restore-{}", uuid::Uuid::new_v4());
    assert!(
        probe(&path, "jsonl", &cache, &json!({"operationId":id}), &|p| {
            if p["phaseId"] == "metadata-restore" || p["phaseId"] == "metadata-map-validate" || p["phaseId"] == "metadata-map-reuse" {
                testkit::cancel_metadata_probe(&id);
            }
        })
        .is_err()
    );
    equivalent(
        &probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap(),
        &expected,
    );
}

#[test]
fn corrupt_committed_payload_and_manifest_are_rejected_but_trailing_bytes_are_ignored() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.jsonl");
    std::fs::write(&path, jsonl(73)).unwrap();
    let cache = root.path().join("cache");
    let expected = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    let payload = files(&cache, "lines").pop().unwrap();
    let state = files(&cache, "state").pop().unwrap();
    let size = std::fs::metadata(&payload).unwrap().len();
    let tail = b"uncommitted torn record";
    std::fs::OpenOptions::new()
        .append(true)
        .open(&payload)
        .unwrap()
        .write_all(tail)
        .unwrap();
    let warm = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    equivalent(&warm, &expected);
    assert_eq!(warm["parsedRows"], 0);
    // Shared mapped admission reads only the committed prefix. It cannot
    // truncate a file that another reader may currently have mapped.
    assert_eq!(std::fs::metadata(&payload).unwrap().len(), size + tail.len() as u64);
    let mut bytes = std::fs::read(&payload).unwrap();
    bytes[72 + 12] ^= 0x40;
    std::fs::write(&payload, bytes).unwrap();
    let rebuilt = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    equivalent(&rebuilt, &expected);
    assert_eq!(rebuilt["resumedRows"], 0);
    assert_eq!(rebuilt["parsedRows"], rebuilt["rows"]);
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    manifest["state"]["cursor"] = 1.into();
    std::fs::write(&state, manifest.to_string()).unwrap();
    let rebuilt = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    equivalent(&rebuilt, &expected);
    assert_eq!(rebuilt["parsedRows"], rebuilt["rows"]);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&payload)
        .unwrap();
    file.set_len(size - 1).unwrap();
    drop(file);
    let rebuilt = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    equivalent(&rebuilt, &expected);
    assert_eq!(rebuilt["resumedRows"], 0);
}

#[test]
fn source_generation_parser_configuration_and_calendar_anchor_separate_variants() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("syslog.log");
    let contents = (0..40)
        .map(|i| format!("Sep 30 12:00:00 host app: message {i}\n"))
        .collect::<String>();
    std::fs::write(&path, &contents).unwrap();
    let cache = root.path().join("cache");
    let first = probe(&path, "syslog3164", &cache, &json!({"year":2031}), &|_| {}).unwrap();
    let next = probe(&path, "syslog3164", &cache, &json!({"year":2032}), &|_| {}).unwrap();
    assert_ne!(first["key"], next["key"]);
    assert_eq!(next["resumedRows"], 0);
    assert_ne!(first["metadata"][0][2], next["metadata"][0][2]);
    assert_eq!(next["metadata"][0][2], next["events"][0]["timestamp"]);
    let changed = probe(&path, "text", &cache, &json!({"year":2032}), &|_| {}).unwrap();
    assert_ne!(changed["key"], next["key"]);
    assert_eq!(changed["resumedRows"], 0);
    let re_a = probe(
        &path,
        "custom",
        &cache,
        &json!({"regex":"(?P<message>.*)"}),
        &|_| {},
    )
    .unwrap();
    let re_b = probe(
        &path,
        "custom",
        &cache,
        &json!({"regex":"(?P<source>.*)"}),
        &|_| {},
    )
    .unwrap();
    assert_ne!(re_a["key"], re_b["key"]);
    assert_eq!(re_b["resumedRows"], 0);
    let old_time = std::fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = root.path().join("replacement");
    std::fs::write(&replacement, &contents).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old_time))
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    let replaced = probe(&path, "syslog3164", &cache, &json!({"year":2032}), &|_| {}).unwrap();
    assert_ne!(replaced["key"], next["key"]);
    assert_eq!(replaced["resumedRows"], 0);
}

#[test]
fn malformed_array_suffix_never_publishes_a_complete_index() {
    let root = tempfile::tempdir().unwrap();
    for (n, suffix) in [",]", ",", ",42]", "] extra"].iter().enumerate() {
        let path = root.path().join(format!("malformed-{n}.json"));
        let cache = root.path().join(format!("cache-{n}"));
        let mut contents = array(30);
        contents.pop();
        contents.push_str(suffix);
        std::fs::write(&path, contents).unwrap();
        assert!(probe(&path, "jsonl", &cache, &json!({}), &|_| {}).is_err());
        let state: Value =
            serde_json::from_slice(&std::fs::read(files(&cache, "state").pop().unwrap()).unwrap())
                .unwrap();
        assert_eq!(state["state"]["scan_complete"], false);
        assert!(state["state"]["columns"].is_null());
        assert!(probe(&path, "jsonl", &cache, &json!({}), &|_| {}).is_err());
    }
}

fn child(root: &Path, mode: &str) -> Child {
    child_format(root, mode, "jsonl")
}
fn child_format(root: &Path, mode: &str, format: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "metadata_process_worker",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LOGINSIGHT_METADATA_TEST_ROOT", root)
        .env("LOGINSIGHT_METADATA_TEST_MODE", mode)
        .env("LOGINSIGHT_METADATA_TEST_FORMAT", format)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}
fn wait_marker(child: &mut Child, path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("worker exited before checkpoint: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("timed out waiting for durable checkpoint");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Invoked as a separate OS process; the parent kills it with no destructor or
/// graceful cancellation path. A normal test invocation has nothing to do.
#[test]
fn metadata_process_worker() {
    let Ok(root) = std::env::var("LOGINSIGHT_METADATA_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let mode = std::env::var("LOGINSIGHT_METADATA_TEST_MODE").unwrap();
    let format = std::env::var("LOGINSIGHT_METADATA_TEST_FORMAT").unwrap_or_else(|_| "jsonl".into());
    let committed = Cell::new(false);
    let phases = std::cell::RefCell::new(Vec::new());
    let result = probe(
        &root.join("source.jsonl"),
        &format,
        &root.join("cache"),
        &json!({}),
        &|p| {
            phases.borrow_mut().push(p["phaseId"].clone());
            let gate = match mode.as_str() {
                "before-data-write" => "metadata-checkpoint-write",
                "before-data-sync" => "metadata-checkpoint-sync",
                "after-data-sync" => "metadata-checkpoint-publish",
                "after-manifest-sync" => "metadata-checkpoint-manifest-ready",
                "after-commit" => "metadata-checkpoint-committed",
                _ => "",
            };
            let hold = committed.get() && p["phaseId"] == gate;
            if p["phaseId"] == "metadata-checkpoint-committed" {
                committed.set(true);
            }
            if hold {
                std::fs::write(root.join("ready"), p.to_string()).unwrap();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        },
    );
    std::fs::write(root.join("phases.json"), serde_json::to_vec(&*phases.borrow()).unwrap()).unwrap();
    std::fs::write(
        root.join("result.json"),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
    assert!(result.is_ok(), "child recovery failed: {result:?}");
}

#[test]
fn killed_process_resumes_without_reparsing_committed_rows_and_lock_protects_pruning() {
    for mode in ["before-data-write", "before-data-sync", "after-data-sync", "after-manifest-sync", "after-commit"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.jsonl");
        std::fs::write(&path, jsonl(73)).unwrap();
        let cache = root.path().join("cache");
        let expected = oracle(&path, "jsonl", &cache, json!({}));
        let mut writer = child(root.path(), mode);
        wait_marker(&mut writer, &root.path().join("ready"));
        let marker: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("ready")).unwrap()).unwrap();
        assert!(marker["checkpointRows"].as_u64().unwrap() > 0);
        let state = files(&cache, "state").pop().unwrap();
        let old_state = std::fs::read(&state).unwrap();
        let durable = serde_json::from_slice::<Value>(&old_state).unwrap()["state"]["sealed_rows"].as_u64().unwrap();
        testkit::prune_metadata_probe(cache.to_str().unwrap(), 0);
        assert_eq!(
            std::fs::read(&state).unwrap(),
            old_state,
            "active writer state was pruned"
        );
        let id = format!("lock-wait-{}", uuid::Uuid::new_v4());
        let waiting = probe(&path, "jsonl", &cache, &json!({"operationId":id}), &|p| {
            if p["phaseId"] == "metadata-lock" {
                testkit::cancel_metadata_probe(&id);
            }
        });
        assert!(waiting.is_err());
        assert_eq!(std::fs::read(&state).unwrap(), old_state);
        writer.kill().unwrap();
        assert!(!writer.wait().unwrap().success());
        let mut restarted = child(root.path(), "finish");
        assert!(restarted.wait().unwrap().success());
        let result: Result<Value, String> =
            serde_json::from_slice(&std::fs::read(root.path().join("result.json")).unwrap())
                .unwrap();
        let resumed = result.unwrap();
        equivalent(&resumed, &expected);
        assert_eq!(resumed["resumedRows"], durable, "{mode} reused a different prefix");
        assert_eq!(
            resumed["parsedRows"].as_u64().unwrap() + resumed["resumedRows"].as_u64().unwrap(),
            resumed["rows"].as_u64().unwrap()
        );
        let payload = files(&cache, "lines").pop().unwrap();
        assert_eq!(std::fs::metadata(payload).unwrap().len(), 72 + 27 * 73);
        #[cfg(any(unix, windows))]
        {
            std::thread::sleep(std::time::Duration::from_millis(2100));
            // The newly completed checkpoint gets its proof only after its
            // inode clock has settled; the following process must reuse it.
            let mut verify = child(root.path(), "finish");
            assert!(verify.wait().unwrap().success());
        }
        let mut warm = child(root.path(), "finish");
        assert!(warm.wait().unwrap().success());
        let result: Result<Value, String> =
            serde_json::from_slice(&std::fs::read(root.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(result.unwrap()["parsedRows"], 0);
        let phases: Vec<Value> = serde_json::from_slice(&std::fs::read(root.path().join("phases.json")).unwrap()).unwrap();
        assert_eq!(phases, [json!("metadata-map-reuse")], "a new process must reuse the validation, not scan the completed metadata");
    }
}

#[test]
fn killed_process_preserves_mutable_multiline_tail_at_each_commit_gate() {
    for mode in ["before-data-write", "before-data-sync", "after-data-sync", "after-manifest-sync", "after-commit"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.jsonl");
        let text = format!("2026-09-30 12:00:00,001 ERROR [main] a.Service - first\n{}2026-09-30 12:00:01,001 INFO [main] a.Service - second\n",
            (0..170).map(|i| format!("\tat a.Service.call(Service.java:{i})\n")).collect::<String>());
        std::fs::write(&path, text).unwrap();
        let cache = root.path().join("cache");
        let expected = oracle(&path, "log4j", &cache, json!({}));
        let mut writer = child_format(root.path(), mode, "log4j");
        wait_marker(&mut writer, &root.path().join("ready"));
        let state: Value = serde_json::from_slice(&std::fs::read(files(&cache, "state").pop().unwrap()).unwrap()).unwrap();
        assert_eq!(state["state"]["sealed_rows"], 0, "first long record must still be mutable");
        assert!(state["state"]["tail"].is_object());
        writer.kill().unwrap();
        assert!(!writer.wait().unwrap().success());
        let mut restarted = child_format(root.path(), "finish", "log4j");
        assert!(restarted.wait().unwrap().success());
        let result: Result<Value, String> = serde_json::from_slice(&std::fs::read(root.path().join("result.json")).unwrap()).unwrap();
        let resumed = result.unwrap();
        equivalent(&resumed, &expected);
        assert_eq!(resumed["resumedRows"], 0);
        assert_eq!(resumed["parsedRows"], 1, "{mode} reparsed or skipped the preserved multiline tail");
        assert_eq!(resumed["rows"], 2);
    }
}

#[test]
fn unavailable_cache_reports_degradation_without_claiming_durable_rows() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.jsonl");
    std::fs::write(&path, jsonl(40)).unwrap();
    let cache = root.path().join("not-a-directory");
    std::fs::write(&cache, b"occupied").unwrap();
    let warning = Cell::new(false);
    let actual = probe(&path, "jsonl", &cache, &json!({}), &|p| {
        if p["phaseId"] == "metadata-unavailable" {
            warning.set(true);
        }
        assert_eq!(p["checkpointRows"], 0);
    })
    .unwrap();
    assert!(warning.get());
    assert_eq!(actual["rows"], 40);
    assert_eq!(actual["resumedRows"], 0);
    equivalent(&actual, &oracle(&path, "jsonl", &cache, json!({})));
}

#[test]
fn deferred_wave_keeps_source_validation_and_cancellation_checks() {
    for change_source in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.jsonl");
        std::fs::write(&path, jsonl(40)).unwrap();
        let cache = root.path().join("cache");
        let id = format!("deferred-check-{}", uuid::Uuid::new_v4());
        let changed = Cell::new(false);
        let options = json!({"operationId":id,"checkpointRows":1000,"checkpointSourceBytes":1048576});
        let result = probe(&path, "jsonl", &cache, &options, &|p| {
            if p["phaseId"] == "metadata-scan" && p["parsedRows"].as_u64().unwrap() > 0 && !changed.replace(true) {
                if change_source {
                    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(jsonl(1).as_bytes()).unwrap();
                } else { assert!(testkit::cancel_metadata_probe(&id)); }
            }
        });
        assert!(changed.get() && result.is_err());
        assert!(files(&cache, "state").is_empty(), "deferred wave must not publish after invalidation/cancel");
        let fresh = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
        assert_eq!(fresh["rows"], if change_source { 41 } else { 40 });
        assert_eq!(fresh["resumedRows"], 0);
        equivalent(&fresh, &oracle(&path, "jsonl", &cache, json!({})));
    }
}

#[test]
fn generation_change_during_scan_does_not_publish_a_mixed_source() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.jsonl");
    std::fs::write(&path, jsonl(40)).unwrap();
    let cache = root.path().join("cache");
    let changed = Cell::new(false);
    let result = probe(&path, "jsonl", &cache, &json!({}), &|p| {
        if p["phaseId"] == "metadata-checkpoint-committed" && !changed.replace(true) {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(jsonl(1).as_bytes())
                .unwrap();
        }
    });
    assert!(changed.get());
    assert!(result.is_err());
    let fresh = probe(&path, "jsonl", &cache, &json!({}), &|_| {}).unwrap();
    assert_eq!(fresh["rows"], 41);
    assert_eq!(fresh["resumedRows"], 0);
}

#[test]
fn final_column_sample_keeps_the_original_stratified_selection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sample.jsonl");
    let mut contents = String::new();
    for i in 0..8000 {
        let mut row = json!({"message":format!("row {i}")});
        if i == 1 {
            row["not_in_stratified_sample"] = true.into();
        }
        if i == 7998 {
            row["late_sampled_column"] = true.into();
        }
        contents.push_str(&row.to_string());
        contents.push('\n');
    }
    std::fs::write(&path, contents).unwrap();
    let cache = root.path().join("cache");
    let result = probe(
        &path,
        "jsonl",
        &cache,
        &json!({"smallWaves":false}),
        &|_| {},
    )
    .unwrap();
    let columns = result["columns"].as_array().unwrap();
    assert!(columns.contains(&json!("late_sampled_column")));
    assert!(!columns.contains(&json!("not_in_stratified_sample")));
    let warm = probe(
        &path,
        "jsonl",
        &cache,
        &json!({"smallWaves":false}),
        &|_| {},
    )
    .unwrap();
    assert_eq!(warm["columns"], result["columns"]);
    assert_eq!(warm["parsedRows"], 0);
}

#[cfg(unix)]
#[test]
fn historical_timezone_rules_separate_raw_metadata_across_real_processes() {
    if !Path::new("/usr/share/zoneinfo/Europe/Berlin").exists()
        || !Path::new("/usr/share/zoneinfo/Africa/Johannesburg").exists()
    {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("source.jsonl"),
        "{\"timestamp\":\"2026-01-15 12:00:00\",\"message\":\"naive winter timestamp\"}\n",
    )
    .unwrap();
    let run = |zone: &str| {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "metadata_process_worker",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("LOGINSIGHT_METADATA_TEST_ROOT", root.path())
            .env("LOGINSIGHT_METADATA_TEST_MODE", "finish")
            .env("TZ", zone)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .unwrap();
        assert!(status.success());
        let result: Result<Value, String> =
            serde_json::from_slice(&std::fs::read(root.path().join("result.json")).unwrap())
                .unwrap();
        result.unwrap()
    };
    let berlin = run("Europe/Berlin");
    let johannesburg = run("Africa/Johannesburg");
    assert_ne!(berlin["timezone"], johannesburg["timezone"]);
    assert_ne!(berlin["key"], johannesburg["key"]);
    assert_eq!(
        berlin["metadata"][0][2].as_i64().unwrap()
            - johannesburg["metadata"][0][2].as_i64().unwrap(),
        3_600_000
    );
    assert_eq!(johannesburg["resumedRows"], 0);
    // These zones have the same present offset during European summer. The
    // historical January timestamp must stay distinct even in that case.
    if berlin["currentOffset"] == johannesburg["currentOffset"] {
        assert_ne!(
            berlin["events"][0]["timestamp"],
            johannesburg["events"][0]["timestamp"]
        );
    }
    let warm = run("Europe/Berlin");
    assert_eq!(warm["parsedRows"], 0);
    equivalent(&warm, &berlin);
}
