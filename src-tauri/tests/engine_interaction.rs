//! Manual end-to-end backend workload; never creates a large input in CI.
//! See docs/desempenho-grandes-volumes.md for process/cache controls.
use loginsight_lib::testkit::{self, Engine, Source};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::time::Instant;

const IDENTITY_VERSION: u8 = 3;
const RESIDUAL_WARNING: &str = "Este filtro exige confirmação nos registros; consultas amplas podem demorar mais.";

fn page_diagnostics(page: &Value) -> Value {
    let object = page.as_object().expect("page object");
    let fields = ["rows", "total", "hasMore", "nextCursor", "engine", "warning"];
    assert!(object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field)),
        "unexpected page field: never ignore truncation/incomplete/error/cancellation/limit diagnostics");
    assert_eq!(page["engine"], "columnar", "benchmark requires columnar engine");
    assert!(page["warning"].is_null() || page["warning"].as_str() == Some(RESIDUAL_WARNING),
        "unknown page warning: {:?}", page["warning"]);
    json!({"engine":page["engine"],"warning":page["warning"]})
}

fn assert_path_diagnostics(diagnostics: &Value, plan: &Value) {
    assert_eq!(plan["analyzed"], false, "path audit must not run ANALYZE");
    let exact = plan["exactPredicate"].as_bool().expect("path audit exactPredicate boolean");
    if let Some(mode) = plan.get("mode") {
        assert_eq!(mode, "verified_singleton", "known path audit mode");
        assert!(exact, "verified singleton must be exact");
    }
    let expected = if exact { Value::Null } else { json!(RESIDUAL_WARNING) };
    assert_eq!(diagnostics["warning"], expected, "warning must match this page's exact/residual path");
    assert_eq!(diagnostics["engine"], "columnar", "path audit requires columnar engine");
}

fn audit_page_paths(source: &Source, filters: &str, sort_dir: &str, pages: &[Value]) -> Value {
    let mut audits = Vec::with_capacity(pages.len());
    for index in 0..pages.len() {
        let offset = index * 100;
        let cursor = if index == 0 { None } else { pages[index - 1]["nextCursor"].as_str() };
        assert!(index == 0 || cursor.is_some(), "subsequent page audit requires input cursor");
        let plan = source.explain_page_at(filters, "timestamp", sort_dir, offset, 100, cursor, false)
            .expect("explain the measured page path outside timers");
        audits.push(json!({"pageIndex":index,"offset":offset,"inputCursor":cursor,"plan":plan}));
    }
    json!(audits)
}

// Borrow the fingerprint from its original JSON bytes. Replacing just this
// span preserves every other byte (including unknown fields and typed keys),
// rather than deserializing/reserializing an opaque cursor with lossy defaults.
#[derive(Deserialize)]
struct IdentityCursor<'a> {
    version: u8,
    #[serde(borrow)]
    fingerprint: &'a str,
    position: usize,
    keys: Vec<IdentityCursorKey>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
enum IdentityCursorKey {
    Integer(i64),
    Text(String),
}

fn semantic_cursor(raw: &str) -> String {
    let cursor: IdentityCursor<'_> = serde_json::from_str(raw).expect("valid cursor JSON shape");
    assert!(cursor.version > 0, "positive cursor version");
    assert!(!cursor.keys.is_empty(), "nonempty typed cursor keys");
    assert!(cursor.fingerprint.len() == 64 && cursor.fingerprint.bytes().all(|b| b.is_ascii_hexdigit()),
        "cursor fingerprint must be exactly 64 unescaped hex bytes");
    let _position = cursor.position; // Deserialization requires a nonnegative usize.
    let start = cursor.fingerprint.as_ptr() as usize - raw.as_ptr() as usize;
    assert_eq!(&raw[start..start + 64], cursor.fingerprint, "borrow original fingerprint bytes");
    let mut semantic = raw.to_string();
    semantic.replace_range(start..start + 64, &"0".repeat(64));
    semantic
}

fn page_identity(pages: &[Value]) -> Value {
    assert!(!pages.is_empty(), "at least the first page");
    let diagnostics: Vec<_> = pages.iter().map(page_diagnostics).collect();
    let response_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(pages).unwrap()));
    // Restore the original benchmark's data contract. Operational metadata is
    // checked separately against the actual page path and remains in records.
    let data: Vec<_> = pages.iter().map(|page| json!({"rows":page["rows"],"total":page["total"],
        "hasMore":page["hasMore"],"nextCursor":page["nextCursor"]})).collect();
    let full_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&data).unwrap()));
    let mut semantic = data;
    for page in &mut semantic {
        let page = page.as_object_mut().expect("page object");
        assert!(page.get("rows").is_some_and(Value::is_array), "page rows array");
        assert!(page.get("total").is_some_and(|v| v.is_null() || v.as_u64().is_some()), "page total null or nonnegative integer");
        let has_more = page.get("hasMore").and_then(Value::as_bool).expect("page hasMore boolean");
        let cursor = page.get_mut("nextCursor").expect("page nextCursor present");
        match cursor {
            Value::Null => assert!(!has_more, "indexed page with hasMore requires cursor"),
            Value::String(raw) => {
                assert!(has_more, "terminal indexed page cannot have cursor");
                *raw = semantic_cursor(raw);
            }
            _ => panic!("page nextCursor must be null or string"),
        }
    }
    json!({"identityVersion":IDENTITY_VERSION,
        "sha256":format!("{:x}", Sha256::digest(serde_json::to_vec(&semantic).unwrap())),
        "fullSha256":full_sha256,"responseSha256":response_sha256,"diagnostics":diagnostics})
}

fn dump_pages(pages: &[Value], identity: &Value, workload: &str, phase: &str) {
    let Ok(selected) = std::env::var("LOGINSIGHT_BENCH_DUMP_PAGES") else { return; };
    if selected != "*" && !selected.split(',').any(|name| name == workload) { return; }
    let selected_phase = std::env::var("LOGINSIGHT_BENCH_DUMP_PHASE").unwrap_or_else(|_| "before_exact_summary".into());
    if selected_phase != "both" && selected_phase != phase { return; }
    let dir = std::env::var("LOGINSIGHT_BENCH_DIR").expect("isolated dump directory");
    let cache_state = std::env::var("LOGINSIGHT_BENCH_CACHE_STATE").unwrap_or_else(|_| "unspecified".into());
    assert!(matches!(cache_state.as_str(), "initial_preparation" | "new_process_reopen" | "unspecified"));
    let path = std::path::Path::new(&dir).join(format!("{cache_state}.{workload}.{phase}.pages.json"));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).expect("new page dump");
    serde_json::to_writer(&mut file, &json!({"identityVersion":IDENTITY_VERSION,"workload":workload,"phase":phase,
        "iteration":0,"identity":identity,"pages":pages})).expect("write original pages");
    file.write_all(b"\n").unwrap();
    println!("BENCH {}", json!({"operation":"page_dump","workload":workload,"phase":phase,"path":path}));
}

fn assert_foreign_cursor_rejected(source: &Source, cursor: &str, sort_dir: &str) {
    let original: Value = serde_json::from_str(cursor).expect("original cursor JSON");
    for field in ["fingerprint", "version"] {
        let mut altered = original.clone();
        altered[field] = if field == "version" { json!(255) } else {
            json!(if original["fingerprint"] == "0".repeat(64) { "1".repeat(64) } else { "0".repeat(64) })
        };
        let error = source.explain_page_at("[]", "timestamp", sort_dir, 100, 100, Some(&altered.to_string()), false)
            .expect_err("foreign fingerprint/unsupported cursor version must fail");
        assert!(error.contains("PAGINATION_RESET_REQUIRED"), "unexpected cursor rejection: {error}");
        println!("BENCH {}", json!({"operation":"cursor_rejection","changedField":field,"result":"PAGINATION_RESET_REQUIRED"}));
    }
}


fn record(operation: &str, started: Instant, detail: Value) {
    println!(
        "BENCH {}",
        json!({"operation": operation, "elapsedMs": started.elapsed().as_secs_f64() * 1000.0, "detail": detail})
    );
}

fn page_sample(source: &Source, filters: &str, sort_dir: &str, workload: &str, phase: &str, iteration: usize, measured: bool) -> Value {
    let operation_id = format!("bench/{workload}/{phase}/{}/{iteration}", if measured { "sample" } else { "warmup" });
    let start = Instant::now();
    let result = testkit::with_operation(&operation_id, |_| {}, || source.page(filters, "timestamp", sort_dir, 0, 100, None)).expect("page operation");
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    if measured {
        record("first_rows", start, json!({"operationId":operation_id,"workload":workload,"phase":phase,"iteration":iteration,
            "returned":result["rows"].as_array().unwrap().len(),"hasMore":result["hasMore"],"engine":result["engine"]}));
        // Source::page includes conversion to serde_json::Value. Encoding below is
        // measured separately; it does not include Tauri IPC or WebView work.
        let start = Instant::now();
        let encoded = serde_json::to_vec(&result).expect("encode full page");
        record("json_encode", start, json!({"operationId":operation_id,"workload":workload,"phase":phase,"iteration":iteration,"bytes":encoded.len()}));
    }
    let mut pages = vec![result.clone()];
    if let Some(cursor) = result["nextCursor"].as_str() {
        let start = Instant::now();
        let next = testkit::with_operation(&format!("{operation_id}/next"), |_| {}, || source.page(filters, "timestamp", sort_dir, 100, 100, Some(cursor))).expect("next page operation");
        if measured {
            record("cursor_next_page", start, json!({"operationId":format!("{operation_id}/next"),"workload":workload,"phase":phase,"iteration":iteration,
                "returned":next["rows"].as_array().unwrap().len(),"hasMore":next["hasMore"]}));
        }
        pages.push(next);
    }
    // Identity, optional diagnostic I/O and rejection checks are outside all
    // operation timers. The original page payloads/cursors remain available.
    let mut identity = page_identity(&pages);
    identity["firstRowsMs"] = json!(elapsed_ms);
    identity["pathAudit"] = if measured && iteration == 0 {
        audit_page_paths(source, filters, sort_dir, &pages)
    } else { Value::Null };
    if measured {
        println!("BENCH {}", json!({"operation":"page_identity","operationId":operation_id,"workload":workload,"phase":phase,"iteration":iteration,
            "identityVersion":IDENTITY_VERSION,"sha256":identity["sha256"],"fullSha256":identity["fullSha256"],
            "responseSha256":identity["responseSha256"],"diagnostics":identity["diagnostics"],"pathAudit":identity["pathAudit"],"firstRowsMs":elapsed_ms}));
        if iteration == 0 {
            dump_pages(&pages, &identity, workload, phase);
            // Preserve the actual response metadata and complete path proof in
            // stdout/dumps before rejecting a mismatch.
            for (diagnostics, audit) in identity["diagnostics"].as_array().unwrap().iter()
                .zip(identity["pathAudit"].as_array().unwrap()) {
                assert_path_diagnostics(diagnostics, &audit["plan"]);
            }
            if workload == "all" && phase == "before_exact_summary" {
                if let Some(cursor) = pages[0]["nextCursor"].as_str() {
                    assert_foreign_cursor_rejected(source, cursor, sort_dir);
                } else {
                    println!("BENCH {}", json!({"operation":"cursor_rejection","result":"not_applicable_no_cursor"}));
                }
            }
        }
    }
    identity
}

#[test]
#[ignore = "requires explicit LOGINSIGHT_BENCH_FILE and LOGINSIGHT_BENCH_DIR"]
fn interactive_workload() {
    let file = std::env::var("LOGINSIGHT_BENCH_FILE").expect("set LOGINSIGHT_BENCH_FILE");
    let dir = std::env::var("LOGINSIGHT_BENCH_DIR").expect("set an isolated LOGINSIGHT_BENCH_DIR");
    let repeats: usize = std::env::var("LOGINSIGHT_BENCH_REPEATS")
        .unwrap_or_else(|_| "30".into())
        .parse()
        .expect("integer repeats");
    assert!((1..=100).contains(&repeats), "repeats must be 1..100");
    let warmup: usize = std::env::var("LOGINSIGHT_BENCH_WARMUP")
        .unwrap_or_else(|_| "3".into()).parse().expect("integer warmup");
    assert!(warmup <= 20, "warmup must be 0..20");
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
    if let Ok(selected) = std::env::var("LOGINSIGHT_BENCH_DUMP_PAGES") {
        // Only explicitly generated synthetic fixtures may be copied to dumps.
        assert_eq!(fixture["generatorVersion"], 2, "page dumps require generator-v2 synthetic fixture manifest");
        assert_eq!(fixture["rows"].as_u64(), Some(source.len() as u64));
        assert_eq!(fixture["bytes"].as_u64(), Some(std::fs::metadata(&file).unwrap().len()));
        let mut input = std::fs::File::open(&file).unwrap();
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let bytes = input.read(&mut buffer).expect("read synthetic dump fixture");
            if bytes == 0 { break; }
            hash.update(&buffer[..bytes]);
        }
        let actual_sha256 = format!("{:x}", hash.finalize());
        assert_eq!(fixture["sha256"].as_str(), Some(actual_sha256.as_str()), "dump fixture manifest hash");
        assert!(!selected.is_empty(), "explicit dump workload selection required");
        let phase = std::env::var("LOGINSIGHT_BENCH_DUMP_PHASE").unwrap_or_else(|_| "before_exact_summary".into());
        assert!(matches!(phase.as_str(), "before_exact_summary" | "after_exact_summary" | "both"), "valid dump phase");
    }
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
        ("common_text", json!([{"column":"_all","op":"query","value":"completed"}]).to_string()),
        ("missing_text", json!([{"column":"_all","op":"query","value":"__missing_bench_needle__"}]).to_string()),
        ("residual_raw_regex", json!([{"column":"_all","op":"regex","value":r"route (7|8)\b"}]).to_string()),
        ("cached_raw_regex", json!([{"column":"raw","op":"regex","value":r"route (7|8)\b"}]).to_string()),
        ("cached_message_contains", json!([{"column":"message","op":"contains","value":"request completed"}]).to_string()),
        ("rare_hex_free", json!([{"column":"_all","op":"query","value":point_id}]).to_string()),
    ];
    if let Ok(selected) = std::env::var("LOGINSIGHT_BENCH_DUMP_PAGES") {
        assert!(selected == "*" || selected.split(',').all(|name| workloads.iter().any(|(known, _)| *known == name)), "known dump workload names");
    }
    for (name, filters) in workloads {
        let mut expected = None;
        for phase in ["before_exact_summary", "after_exact_summary"] {
            if phase == "after_exact_summary" {
                let start = Instant::now();
                let count = source.count(Engine::Columnar, &filters);
                record("exact_count", start, json!({"workload":name,"count":count}));
                let start = Instant::now();
                let stats = source.stats(Engine::Columnar, &filters);
                record("statistics", start, json!({"workload":name,"result":stats}));
            }
            println!("BENCH {}", json!({"operation":"selection_cache","workload":name,"phase":phase,"snapshot":source.selection_cache()}));
            for iteration in 0..warmup {
                page_sample(&source, &filters, &sort_dir, name, phase, iteration, false);
            }
            let mut timings = Vec::with_capacity(repeats);
            let mut expected_diagnostics = None;
            for iteration in 0..repeats {
                let sample = page_sample(&source, &filters, &sort_dir, name, phase, iteration, true);
                timings.push(sample["firstRowsMs"].as_f64().unwrap());
                if let Some(ref diagnostics) = expected_diagnostics {
                    assert_eq!(&sample["diagnostics"], diagnostics,
                        "page operational path changed within {name}/{phase}; rerun path audit rather than ignore it");
                } else { expected_diagnostics = Some(sample["diagnostics"].clone()); }
                if let Some(ref expected) = expected {
                    assert_eq!(&sample["fullSha256"], expected, "original page rows/order/full cursors/total changed in {name}/{phase}/{iteration}");
                } else { expected = Some(sample["fullSha256"].clone()); }
            }
            timings.sort_by(f64::total_cmp);
            println!("BENCH {}", json!({"operation":"first_rows_distribution","workload":name,"phase":phase,"samples":repeats,"warmup":warmup,
                "p50Ms":timings[(repeats - 1) / 2],"p95Ms":(repeats >= 30).then(|| timings[((repeats as f64 * 0.95).ceil() as usize - 1).min(repeats - 1)])}));
        }
        if std::env::var_os("LOGINSIGHT_BENCH_EXPLAIN").is_some() {
            let plan = source
                .explain_page(&filters, "timestamp", &sort_dir, 100, true)
                .expect("explain interactive plan");
            println!(
                "BENCH {}",
                json!({"operation":"explain_analyze","workload":name,"plan":plan})
            );
        }
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn cursor(fingerprint: &str) -> String {
        format!(r#"{{"version":1,"fingerprint":"{fingerprint}","position":100,"keys":[{{"Integer":1735689600000}},{{"Integer":99}}],"future":{{"keep":true}}}}"#)
    }

    fn pages(fingerprint: &str) -> Vec<Value> {
        vec![json!({"rows":[{"id":1,"message":"first"},{"id":2,"message":"second"}],
            "total":300,"hasMore":true,"nextCursor":cursor(fingerprint),"engine":"columnar","warning":null})]
    }

    #[test]
    fn page_identity_v3_masks_only_scope_fingerprint_bytes() {
        let original = pages(&"a".repeat(64));
        let foreign = pages(&"b".repeat(64));
        let a = page_identity(&original);
        let b = page_identity(&foreign);
        assert_eq!(a["identityVersion"], 3);
        assert_eq!(a["sha256"], b["sha256"]);
        assert_ne!(a["fullSha256"], b["fullSha256"]);
        assert_eq!(semantic_cursor(original[0]["nextCursor"].as_str().unwrap()), cursor(&"0".repeat(64)));
        // Identical text elsewhere, unknown fields, formatting, and full i64
        // values must survive. Only the borrowed fingerprint span is masked.
        let raw = cursor(&"a".repeat(64)).replace("true", &format!(r#""{}""#, "a".repeat(64)))
            .replace("1735689600000", "9223372036854775807").replace(r#""position":100"#, r#""position" : 100"#);
        assert_eq!(semantic_cursor(&raw), raw.replacen(&format!(r#""fingerprint":"{}""#, "a".repeat(64)),
            &format!(r#""fingerprint":"{}""#, "0".repeat(64)), 1));
    }

    #[test]
    fn page_identity_v3_preserves_rows_order_bounds_and_all_cursor_fields() {
        let original = pages(&"a".repeat(64));
        let expected = page_identity(&original)["sha256"].clone();
        for field in ["row", "order", "total", "hasMore", "version", "position", "key_value", "key_type", "unknown", "formatting"] {
            let mut changed = original.clone();
            match field {
                "row" => changed[0]["rows"][0]["message"] = json!("changed"),
                "order" => changed[0]["rows"].as_array_mut().unwrap().reverse(),
                "total" => changed[0]["total"] = json!(301),
                "hasMore" => { changed[0]["hasMore"] = json!(false); changed[0]["nextCursor"] = Value::Null; }
                _ => {
                    let raw = changed[0]["nextCursor"].as_str().unwrap();
                    changed[0]["nextCursor"] = json!(match field {
                        "version" => raw.replace(r#""version":1"#, r#""version":2"#),
                        "position" => raw.replace(r#""position":100"#, r#""position":101"#),
                        "key_value" => raw.replace(r#""Integer":99"#, r#""Integer":98"#),
                        "key_type" => raw.replace(r#""Integer":99"#, r#""Text":"99""#),
                        "unknown" => raw.replace("true", "false"),
                        "formatting" => raw.replace(r#""position":100"#, r#""position" : 100"#),
                        _ => unreachable!(),
                    });
                }
            }
            assert_ne!(page_identity(&changed)["sha256"], expected, "must detect {field}");
        }
    }

    #[test]
    fn page_identity_v3_rejects_malformed_or_ambiguous_cursor_shapes() {
        let valid = cursor(&"a".repeat(64));
        let malformed = [
            "null".into(), "[]".into(), "not-json".into(), "{}".into(),
            valid.replace(&"a".repeat(64), &"z".repeat(64)),
            valid.replace(&"a".repeat(64), &"a".repeat(63)),
            valid.replace(r#""version":1"#, r#""version":"1""#),
            valid.replace(r#""position":100"#, r#""position":-1"#),
            valid.replace(r#""position":100"#, r#""position":100.5"#),
            valid.replace(r#"{"Integer":99}"#, r#"{"Integer":"99"}"#),
            valid.replace(r#"{"Integer":99}"#, r#"{"Float":99}"#),
            valid.replace(r#""position":100"#, r#""position":100,"position":101"#),
            valid.replace(r#""fingerprint":""#, &format!(r#""fingerprint":"{}","fingerprint":""#, "b".repeat(64))),
        ];
        for raw in malformed {
            assert!(std::panic::catch_unwind(|| semantic_cursor(&raw)).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn page_identity_v3_audits_operational_metadata_without_changing_data_contract() {
        let exact = pages(&"a".repeat(64));
        let mut residual = exact.clone();
        residual[0]["warning"] = json!(RESIDUAL_WARNING);
        let a = page_identity(&exact);
        let b = page_identity(&residual);
        assert_eq!(a["sha256"], b["sha256"]);
        assert_eq!(a["fullSha256"], b["fullSha256"]);
        assert_ne!(a["responseSha256"], b["responseSha256"]);
        assert_ne!(a["diagnostics"], b["diagnostics"]);
        let exact_plan = json!({"analyzed":false,"exactPredicate":true});
        let residual_plan = json!({"analyzed":false,"exactPredicate":false});
        assert_path_diagnostics(&a["diagnostics"][0], &exact_plan);
        assert_path_diagnostics(&b["diagnostics"][0], &residual_plan);
        assert_path_diagnostics(&a["diagnostics"][0], &json!({"analyzed":false,"exactPredicate":true,"mode":"verified_singleton"}));
        assert!(std::panic::catch_unwind(|| assert_path_diagnostics(&a["diagnostics"][0], &residual_plan)).is_err());
        assert!(std::panic::catch_unwind(|| assert_path_diagnostics(&b["diagnostics"][0], &exact_plan)).is_err());
        assert!(std::panic::catch_unwind(|| assert_path_diagnostics(&a["diagnostics"][0], &json!({"analyzed":false}))).is_err());
        assert!(std::panic::catch_unwind(|| assert_path_diagnostics(&b["diagnostics"][0], &json!({"analyzed":false,"exactPredicate":false,"mode":"verified_singleton"}))).is_err());
        for (field, value) in [("warning", json!("truncated")), ("warning", json!("cancelled")),
            ("warning", json!("incomplete")), ("warning", json!("resource limit")), ("warning", json!("error")),
            ("engine", json!("lines")), ("truncated", json!(true)), ("incomplete", json!(true)),
            ("error", json!("failure")), ("cancelled", json!(true)), ("limitExceeded", json!(true))] {
            let mut invalid = exact.clone();
            invalid[0][field] = value;
            assert!(std::panic::catch_unwind(|| page_identity(&invalid)).is_err(), "accepted unsafe diagnostic {field}");
        }
    }
}
