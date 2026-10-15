//! Separations end to end: files on disk, every mode and the in-parts path.
use super::*;
use std::io::Write;

fn grant_for(input: &Path) -> String {
    let token = uuid::Uuid::new_v4().simple().to_string();
    GRANTS.lock().unwrap().insert(token.clone(), Grant { input: input.to_path_buf(), matched: None, rest: None });
    token
}

fn spec(mode: &str, conditions: serde_json::Value) -> SplitSpec {
    serde_json::from_value(serde_json::json!({"framing":{"kind":"auto"},"mode":mode,"rule":{"conditions":conditions}})).unwrap()
}

fn java_log(items: usize) -> String {
    let mut text = String::new();
    for i in 0..items {
        let secs = i as u64;
        text.push_str(&format!("2026-10-12 {:02}:{:02}:{:02},123 {} [main] c.e.Service - item {i}\n", secs / 3600 % 24, secs / 60 % 60, secs % 60, if i % 5 == 0 { "ERROR" } else { "INFO " }));
        if i % 5 == 0 {
            text.push_str("java.lang.IllegalStateException: falha\n\tat c.e.Service.run(Service.java:10)\n\t... 12 more\n");
        }
    }
    text
}

fn expected(text: &str) -> (String, String) {
    let (mut matched, mut rest) = (String::new(), String::new());
    let mut current = String::new();
    let mut flush = |item: &mut String| {
        if item.contains(" ERROR ") { matched.push_str(item) } else { rest.push_str(item) }
        item.clear();
    };
    for line in text.split_inclusive('\n') {
        if line.starts_with("2026-") && !current.is_empty() {
            flush(&mut current);
        }
        current.push_str(line);
    }
    flush(&mut current);
    (matched, rest)
}

#[test]
fn every_mode_writes_the_expected_files() {
    let dir = tempfile::tempdir().unwrap();
    let jdir = dir.path().join("journal");
    let text = java_log(3_000);
    let (want_matched, want_rest) = expected(&text);
    let errors = serde_json::json!([{"kind":"text","values":["ERROR"],"scope":"firstLine"}]);
    for mode in ["matched", "both", "move"] {
        let input = dir.path().join(format!("{mode}.log"));
        fs::write(&input, &text).unwrap();
        let token = grant_for(&input);
        let result = run_impl(&token, &spec(mode, errors.clone()), &jdir, false).unwrap();
        assert_eq!(result.strategy, "stream");
        assert_eq!(fs::read_to_string(&result.matched.path).unwrap(), want_matched, "{mode}");
        assert!(result.matched.path.ends_with(&format!("{mode}.filtrado.log")));
        match mode {
            "matched" => assert!(result.rest.is_none() && input.exists()),
            "both" => {
                assert_eq!(fs::read_to_string(&result.rest.as_ref().unwrap().path).unwrap(), want_rest);
                assert!(input.exists());
            }
            _ => {
                assert_eq!(fs::read_to_string(&result.rest.as_ref().unwrap().path).unwrap(), want_rest);
                assert!(result.original_removed && !input.exists());
            }
        }
        assert_eq!(result.stats.items, 3_000);
        assert_eq!(result.stats.matched_items, 600);
    }
    // A second run never overwrites the first outputs.
    let input = dir.path().join("both.log");
    let token = grant_for(&input);
    let again = run_impl(&token, &spec("matched", errors), &jdir, false).unwrap();
    assert!(again.matched.path.ends_with("both.filtrado-2.log"));
}

#[test]
fn parts_mode_matches_stream_mode_and_consumes_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let jdir = dir.path().join("journal");
    lowspace::TEST_CHUNK.with(|c| c.set(Some(64 * 1024 + 3)));
    let mut text = String::from("\u{feff}");
    text.push_str(&java_log(20_000));
    let input = dir.path().join("big.log");
    fs::write(&input, &text).unwrap();
    let (want_matched, want_rest) = expected(&text[3..]);
    let token = grant_for(&input);
    let result = run_impl(&token, &spec("move", serde_json::json!([{"kind":"text","values":["ERROR"],"scope":"firstLine"}])), &jdir, true).unwrap();
    assert_eq!(result.strategy, "parts");
    assert!(!input.exists());
    let bom = "\u{feff}";
    assert_eq!(fs::read_to_string(&result.matched.path).unwrap(), format!("{bom}{want_matched}"));
    assert_eq!(fs::read_to_string(&result.rest.unwrap().path).unwrap(), format!("{bom}{want_rest}"));
    assert!(fs::read_dir(dir.path()).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".loginsight")));
    assert!(pending_impl(&jdir).is_empty());
}

#[test]
fn sorted_cut_keeps_the_head_in_place_and_cancel_restores() {
    let dir = tempfile::tempdir().unwrap();
    let jdir = dir.path().join("journal");
    lowspace::TEST_CHUNK.with(|c| c.set(Some(100_000)));
    let text = java_log(30_000);
    let input = dir.path().join("app.log");
    fs::write(&input, &text).unwrap();
    let cut = crate::sources::naive_to_ms(chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap().and_hms_opt(5, 0, 0).unwrap());
    let rule: SplitSpec = serde_json::from_value(serde_json::json!({"framing":{"kind":"auto"},"mode":"move","rule":{"sorted":true,"conditions":[{"kind":"date","to":cut}]}})).unwrap();
    let token = grant_for(&input);
    let result = run_impl(&token, &rule, &jdir, true).unwrap();
    let matched = fs::read_to_string(&result.matched.path).unwrap();
    let rest = fs::read_to_string(&result.rest.unwrap().path).unwrap();
    assert_eq!(format!("{matched}{rest}"), text);
    assert!(matched.ends_with("item 17999\n") && rest.starts_with("2026-10-12 05:00:00,123"));
    assert_eq!(result.stats.items, 0, "decided by binary search");

    // An interrupted carve is listed and can be undone byte for byte.
    let input = dir.path().join("again.log");
    fs::write(&input, &text).unwrap();
    let prep = stream::prepare(&input, open_original(&input, true).unwrap(), &rule).unwrap();
    let mut journal = Journal {
        version: 1, id: "abc123".into(), original: input.clone(), matched: dir.path().join("m.log"), rest: dir.path().join("r.log"),
        work: dir.path().join(".loginsight-separacao-abc"), bom: Vec::new(), original_len: prep.len, phase: Phase::Carve, cut: prep.len,
        head_end: prep.len, head_matched: false, chunks: Vec::new(), renamed: false, assembled: Default::default(), matched_bytes: 0,
        rest_bytes: 0, stats: Stats::default(), framing: String::new(), started_ms: 1,
    };
    fs::create_dir_all(&journal.work).unwrap();
    let mut sink = MapSink::create(&journal.map_path()).unwrap();
    let mut meter = Meter::new("t", "t", 0, prep.len);
    stream::run(&prep, &mut sink, &mut meter).unwrap();
    sink.finish().unwrap();
    lowspace::locate_head(&mut journal).unwrap();
    journal.save(&jdir).unwrap();
    let Prepared { mut file, .. } = prep;
    // Two chunks, then a "crash".
    let head = journal.head_end;
    let stop_at = journal.original_len - 150_000;
    journal.head_end = stop_at;
    assert!(lowspace::carve(&mut journal, &jdir, &mut file).is_ok());
    drop(file);
    journal.head_end = head;
    journal.save(&jdir).unwrap();
    assert!(fs::metadata(&input).unwrap().len() < text.len() as u64);
    let pending = pending_impl(&jdir);
    assert_eq!(pending.len(), 1);
    assert!(pending[0].can_undo && pending[0].carved > 0);
    assert!(recover_impl(&jdir, "abc123", true).unwrap().is_none());
    assert_eq!(fs::read_to_string(&input).unwrap(), text);
    assert!(pending_impl(&jdir).is_empty());
    assert!(recover_impl(&jdir, "../x", true).is_err());
}

#[test]
fn utf16_logs_keep_their_encoding_and_line_numbers_work() {
    let dir = tempfile::tempdir().unwrap();
    let jdir = dir.path().join("journal");
    let text = "linha 1\nlinha 2\nlinha 3\nlinha 4\n";
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let input = dir.path().join("ps.txt");
    fs::File::create(&input).unwrap().write_all(&bytes).unwrap();
    let token = grant_for(&input);
    let spec: SplitSpec = serde_json::from_value(serde_json::json!({"framing":{"kind":"line"},"mode":"both","rule":{"conditions":[{"kind":"ordinal","unit":"line","from":2,"to":3}]}})).unwrap();
    let result = run_impl(&token, &spec, &jdir, false).unwrap();
    let decode = |path: &str| {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xfe]);
        String::from_utf16(&bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()).unwrap()
    };
    assert_eq!(decode(&result.matched.path), "linha 2\nlinha 3\n");
    assert_eq!(decode(&result.rest.unwrap().path), "linha 1\nlinha 4\n");
}

#[test]
fn outputs_never_replace_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("a.log");
    fs::write(&input, "x\n").unwrap();
    assert!(check_outputs(&input, &input, None).is_err());
    assert!(check_outputs(&input, &dir.path().join("b.log"), Some(&dir.path().join("b.log"))).is_err());
    assert_eq!(default_output(Path::new("C:/logs/server.log.2026-10-12"), "filtrado"), Path::new("C:/logs/server.log.2026-10-12.filtrado.log"));
    assert_eq!(default_output(Path::new("C:/logs/app.json"), "restante"), Path::new("C:/logs/app.restante.json"));
}

/// Throughput on a generated Java log: `cargo test --release --lib log_split::tests::bench_large_file -- --ignored --nocapture`
/// (size in MiB from LOGINSIGHT_SPLIT_BENCH_MB, folder from LOGINSIGHT_SPLIT_BENCH_DIR).
#[test]
#[ignore]
fn bench_large_file() {
    let mib: u64 = std::env::var("LOGINSIGHT_SPLIT_BENCH_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(1024);
    let base = std::env::var_os("LOGINSIGHT_SPLIT_BENCH_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = tempfile::tempdir_in(base).unwrap();
    let jdir = dir.path().join("journal");
    let source = dir.path().join("source.log");
    let started = Instant::now();
    let first = chrono::NaiveDate::from_ymd_opt(2026, 10, 12).unwrap().and_hms_opt(0, 0, 0).unwrap();
    let mut i = 0u64;
    {
        let mut out = std::io::BufWriter::with_capacity(8 << 20, File::create(&source).unwrap());
        let mut written = 0u64;
        while written < mib << 20 {
            // 50 items per second, chronological across days.
            let at = first + chrono::Duration::seconds((i / 50) as i64);
            let line = format!("{},{:03} {} [http-nio-8080-exec-{}] c.e.service.OrderService - pedido {i} processado para o cliente {} em {} ms\n", at.format("%Y-%m-%d %H:%M:%S"), i % 1000, if i % 50 == 0 { "ERROR" } else { "INFO " }, i % 200, i * 7 % 10_000, i % 900);
            written += line.len() as u64;
            out.write_all(line.as_bytes()).unwrap();
            if i % 50 == 0 {
                let trace = "java.lang.IllegalStateException: pedido inválido\n\tat c.e.service.OrderService.process(OrderService.java:120)\n\tat c.e.web.OrderController.post(OrderController.java:45)\n\t... 87 more\n";
                written += trace.len() as u64;
                out.write_all(trace.as_bytes()).unwrap();
            }
            i += 1;
        }
    }
    println!("generated {mib} MiB in {:.1}s", started.elapsed().as_secs_f64());
    // The middle of the generated period.
    let to = crate::sources::naive_to_ms(first + chrono::Duration::seconds((i / 100) as i64));
    let cases: [(&str, serde_json::Value, bool); 5] = [
        ("both, ERROR in first line", serde_json::json!({"framing":{"kind":"auto"},"mode":"both","rule":{"conditions":[{"kind":"text","values":["ERROR"],"scope":"firstLine"}]}}), false),
        ("both, regex over items", serde_json::json!({"framing":{"kind":"auto"},"mode":"both","rule":{"conditions":[{"kind":"regex","pattern":"cliente 1[0-9]{2}\\b.*em [0-9]{3} ms"}]}}), false),
        ("both, date before the middle (every item)", serde_json::json!({"framing":{"kind":"auto"},"mode":"both","rule":{"conditions":[{"kind":"date","to":to}]}}), false),
        ("both, date before the middle (chronological)", serde_json::json!({"framing":{"kind":"auto"},"mode":"both","rule":{"sorted":true,"conditions":[{"kind":"date","to":to}]}}), false),
        ("move in parts, date before the middle (chronological)", serde_json::json!({"framing":{"kind":"auto"},"mode":"move","rule":{"sorted":true,"conditions":[{"kind":"date","to":to}]}}), true),
    ];
    for (label, spec, parts) in cases {
        let input = dir.path().join("input.log");
        fs::copy(&source, &input).unwrap();
        let token = grant_for(&input);
        let spec: SplitSpec = serde_json::from_value(spec).unwrap();
        let started = Instant::now();
        let result = crate::operations::run_with_token(crate::operations::token(None).unwrap(), || run_impl(&token, &spec, &jdir, parts)).unwrap().unwrap();
        let seconds = started.elapsed().as_secs_f64();
        println!("{label}: {seconds:.1}s, {:.0} MiB/s, strategy {}, matched {} MiB, items {}", mib as f64 / seconds, result.strategy, result.matched.bytes >> 20, result.stats.items);
        for path in [Some(result.matched.path), result.rest.map(|r| r.path)].into_iter().flatten() {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_file(&input);
    }
}
