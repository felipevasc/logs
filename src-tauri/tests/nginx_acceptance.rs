//! Small, reproducible Nginx acceptance fixtures. These do not describe the
//! user's custom log_format and are not the generic 50M JSONL benchmark.
use flate2::{write::GzEncoder, Compression};
use loginsight_lib::testkit::{self, Engine, Source};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ROWS: usize = 24;
const BASE: i64 = 1_790_683_200_000; // explicit UTC instant, independent of local timezone/year

fn ip(id: usize) -> &'static str {
    if id % 2 == 0 {
        "203.0.113.10"
    } else {
        "2001:db8::2"
    }
}
fn code(id: usize) -> u16 {
    [200, 404, 503][id % 3]
}
fn uri(id: usize) -> &'static str {
    if id % 4 == 0 {
        "/api/items?limit=10"
    } else {
        "/health"
    }
}
fn timestamp(id: usize) -> i64 {
    BASE + (id / 3) as i64 * 1000
}

fn write_fixture(root: &Path, kind: &str) -> PathBuf {
    let mut contents = String::new();
    for id in 0..ROWS {
        let time = chrono::DateTime::from_timestamp_millis(timestamp(id)).unwrap();
        if kind == "json" {
            // A supported custom JSON mapping: code/source/time are explicit
            // standard columns; original Nginx fields remain filterable.
            contents.push_str(
                &json!({
                    "time": time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "source": "nginx-edge", "code": code(id),
                    "remote_addr": ip(id), "status": code(id), "request_uri": uri(id),
                    "request_method": "GET", "body_bytes_sent": 100 + id,
                    "message": format!("GET {} {}", uri(id), code(id)),
                })
                .to_string(),
            );
            contents.push('\n');
        } else {
            contents.push_str(&format!(
                "{} - - [{}] \"GET {} HTTP/1.1\" {} {} \"-\" \"nginx-acceptance/1.0\"\n",
                ip(id),
                time.format("%d/%b/%Y:%H:%M:%S %z"),
                uri(id),
                code(id),
                100 + id,
            ));
        }
    }
    let path = root.join(match kind {
        "json" => "nginx.jsonl",
        "gzip" => "access.log.gz",
        _ => "access.log",
    });
    if kind == "gzip" {
        let mut writer = GzEncoder::new(std::fs::File::create(&path).unwrap(), Compression::fast());
        writer.write_all(contents.as_bytes()).unwrap();
        writer.finish().unwrap();
    } else {
        std::fs::write(&path, contents).unwrap();
    }
    path
}

fn filter(column: &str, op: &str, value: impl ToString) -> Value {
    json!({"column":column,"op":op,"value":value.to_string()})
}

fn exercise(source: &Source, kind: &str) -> Value {
    let path_field = if kind == "json" {
        "request_uri"
    } else {
        "path"
    };
    let client_field = if kind == "json" {
        "client_ip"
    } else {
        "source"
    };
    let mut cases = vec![
        ("all", vec![], (0..ROWS).collect::<Vec<_>>()),
        (
            "client",
            vec![filter(client_field, "equals_exact", ip(0))],
            (0..ROWS).filter(|id| id % 2 == 0).collect(),
        ),
        (
            "code",
            vec![filter("code", "equals_exact", "503")],
            (0..ROWS).filter(|id| code(*id) == 503).collect(),
        ),
        (
            "path",
            vec![filter(path_field, "equals_exact", uri(0))],
            (0..ROWS).filter(|id| id % 4 == 0).collect(),
        ),
        (
            "time",
            vec![
                json!({"column":"timestamp","op":"between","value":(BASE+2000).to_string(),"value2":(BASE+4000).to_string()}),
            ],
            (6..15).collect(),
        ),
        (
            "combined",
            vec![
                filter(client_field, "equals_exact", ip(0)),
                filter("code", "equals_exact", "503"),
                filter(path_field, "equals_exact", uri(0)),
            ],
            vec![8, 20],
        ),
    ];
    if kind == "json" {
        cases.push((
            "original_status",
            vec![filter("status", "equals_exact", "503")],
            (0..ROWS).filter(|id| code(*id) == 503).collect(),
        ));
        cases.push((
            "source",
            vec![filter("source", "equals_exact", "nginx-edge")],
            (0..ROWS).collect(),
        ));
    }
    let mut results = BTreeMap::new();
    for (name, filters, expected_ids) in cases {
        let filters = serde_json::to_string(&filters).unwrap();
        assert_eq!(
            source.matches(Engine::Lines, &filters),
            expected_ids,
            "{kind}/{name}: known fixture IDs"
        );
        assert_eq!(
            source.matches(Engine::Columnar, &filters),
            expected_ids,
            "{kind}/{name}: indexed IDs"
        );
        assert_eq!(source.count(Engine::Columnar, &filters), expected_ids.len());
        for direction in ["asc", "desc"] {
            let expected =
                source.query(Engine::Lines, &filters, "timestamp", direction, 0, ROWS + 1);
            let mut rows = Vec::new();
            let mut cursor: Option<String> = None;
            for _ in 0..=ROWS {
                let page = source.page(&filters, "timestamp", direction, 0, 5, cursor.as_deref());
                assert_eq!(
                    page["engine"], "columnar",
                    "{kind}/{name}: silent recovery is not acceptable"
                );
                assert!(page["warning"].is_null());
                rows.extend(page["rows"].as_array().unwrap().iter().cloned());
                if !page["hasMore"].as_bool().unwrap() {
                    break;
                }
                let next = page["nextCursor"]
                    .as_str()
                    .expect("nonterminal page must have a cursor")
                    .to_string();
                assert_ne!(cursor.as_deref(), Some(next.as_str()));
                cursor = Some(next);
            }
            assert_eq!(
                rows,
                *expected["rows"].as_array().unwrap(),
                "{kind}/{name}/{direction}: cursor order including timestamp ties"
            );
            results.insert(format!("{name}/{direction}"), rows);
        }
    }
    serde_json::to_value(results).unwrap()
}

/// Only the parent acceptance test supplies these variables. Each invocation
/// is a new OS process, so no in-memory index/engine cache survives the reopen.
#[test]
fn nginx_process_worker() {
    let Ok(root) = std::env::var("LOGINSIGHT_NGINX_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let kind = std::env::var("LOGINSIGHT_NGINX_TEST_KIND").unwrap();
    let mode = std::env::var("LOGINSIGHT_NGINX_TEST_MODE").unwrap();
    let input = root.join(match kind.as_str() {
        "json" => "nginx.jsonl",
        "gzip" => "access.log.gz",
        _ => "access.log",
    });
    let (format, events) = testkit::load(input.to_str().unwrap()).unwrap();
    assert_eq!(format, if kind == "json" { "jsonl" } else { "apache" });
    assert_eq!(events.len(), ROWS);
    for (id, event) in events.iter().enumerate() {
        assert_eq!(event.timestamp, Some(timestamp(id)));
        assert_eq!(event.code, code(id).to_string());
        if kind == "json" {
            assert_eq!(event.source, "nginx-edge");
            assert_eq!(event.fields["client_ip"], ip(id));
            assert_eq!(event.fields["request_uri"], uri(id));
            assert_eq!(event.fields["status"], code(id));
        } else {
            assert_eq!(event.source, ip(id));
            assert_eq!(event.fields["path"], uri(id));
        }
    }
    let source = Source::open(&[input.to_str().unwrap()], "{}", "[]").unwrap();
    let before = source.engine_status();
    if mode == "reopen" {
        assert_eq!(before["baseReady"], true);
        assert_eq!(before["derivedReady"], true);
        assert_eq!(before["completedSegments"], before["totalSegments"]);
        assert!(before["totalSegments"].as_u64().unwrap() > 0);
    }
    source.prepare().unwrap();
    let queries = exercise(&source, &kind);
    std::fs::write(
        root.join(format!("{mode}.json")),
        json!({"pid":std::process::id(),"format":format,"queries":queries}).to_string(),
    )
    .unwrap();
}

fn run(root: &Path, kind: &str, mode: &str) -> Value {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "nginx_process_worker",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LOGINSIGHT_NGINX_TEST_ROOT", root)
        .env("LOGINSIGHT_NGINX_TEST_KIND", kind)
        .env("LOGINSIGHT_NGINX_TEST_MODE", mode)
        .env("LOGINSIGHT_DATA_DIR", root.join("data"))
        .env("LOGINSIGHT_ENGINE_DIR", root.join("engine"))
        .env("LOGINSIGHT_ENGINE", "1")
        .env("TZ", "UTC")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .unwrap();
    assert!(status.success(), "{kind}/{mode} acceptance process failed");
    serde_json::from_slice(&std::fs::read(root.join(format!("{mode}.json"))).unwrap()).unwrap()
}

fn files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        // Locks and the separate LRU access marker are intentionally mutable;
        // every persisted metadata/index payload and completion marker below
        // must remain byte-for-byte unchanged across the process reopen.
        if path.file_name().unwrap() == "tmp"
            || path.extension().is_some_and(|ext| ext == "lock" || ext == "used")
        {
            continue;
        }
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn snapshot(root: &Path) -> BTreeMap<String, (u64, u128, String)> {
    let mut artifacts = Vec::new();
    files(&root.join("data/indexes-v6/metadata-v1"), &mut artifacts);
    files(&root.join("engine"), &mut artifacts);
    assert!(artifacts
        .iter()
        .any(|p| p.extension().is_some_and(|ext| ext == "lines")));
    assert!(artifacts
        .iter()
        .any(|p| p.extension().is_some_and(|ext| ext == "state")));
    assert!(artifacts
        .iter()
        .any(|p| p.to_string_lossy().ends_with(".complete.json")));
    artifacts
        .into_iter()
        .map(|path| {
            let metadata = std::fs::metadata(&path).unwrap();
            let mut hash = Sha256::new();
            let mut file = std::fs::File::open(&path).unwrap();
            let mut buffer = [0; 65536];
            loop {
                let n = file.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            (
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                (
                    metadata.len(),
                    metadata
                        .modified()
                        .unwrap()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    format!("{:x}", hash.finalize()),
                ),
            )
        })
        .collect()
}

#[test]
fn nginx_combined_json_and_gzip_reopen_without_rebuilding_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    for kind in ["combined", "json", "gzip"] {
        let root = temp.path().join(kind);
        std::fs::create_dir_all(&root).unwrap();
        write_fixture(&root, kind);
        let first = run(&root, kind, "initial");
        let original = snapshot(&root);
        // Pin a recent timestamp before the second process. Even a filesystem
        // with coarse timestamp resolution must reveal an identical rewrite.
        let stamp = SystemTime::now() - Duration::from_secs(2);
        for path in original.keys() {
            std::fs::OpenOptions::new()
                .write(true)
                .open(root.join(path))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(stamp))
                .unwrap();
        }
        let before = snapshot(&root);
        let reopened = run(&root, kind, "reopen");
        assert_ne!(first["pid"], reopened["pid"]);
        assert_eq!(first["queries"], reopened["queries"]);
        assert_eq!(
            before,
            snapshot(&root),
            "{kind}: persisted metadata/engine artifacts were rebuilt, added, removed, or modified"
        );
    }
}
