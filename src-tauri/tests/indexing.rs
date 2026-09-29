//! Large files are indexed in parallel chunks of a few megabytes. The entries
//! must be the ones a line-by-line reading finds, including multi-line
//! entries that cross a chunk boundary, headers, comments and CRLF endings.
use loginsight_lib::testkit::{self, Event};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn workdir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("dados"));
        std::env::set_var("LOGINSIGHT_ENGINE", "0");
        dir
    })
    .path()
}

fn load(name: &str, text: &str) -> (String, Vec<Event>) {
    let path: PathBuf = workdir().join(name);
    std::fs::write(&path, text).unwrap();
    testkit::load(&path.to_string_lossy()).unwrap()
}

#[test]
fn stack_traces_crossing_chunks_stay_whole() {
    // Continuation lines before the first entry form their own entry.
    let mut expected = vec!["\tat orphan.Start(Start.java:1)\n\tat orphan.Start(Start.java:2)".to_string()];
    let mut text = format!("{}\n", expected[0]);
    for i in 0..70_000 {
        let depth = i % 7;
        let mut entry = format!(
            "2026-09-28 10:{:02}:{:02},{:03} ERROR [main] com.acme.Service{} - Falha {i}",
            (i / 60) % 60,
            i % 60,
            i % 1000,
            i % 13
        );
        for d in 0..depth {
            let _ = write!(entry, "\n\tat com.acme.Service{}.call{d}(Service.java:{})", i % 13, 100 + d);
        }
        if depth > 0 && i % 5 == 0 {
            entry.push_str("\nCaused by: java.io.IOException: disco cheio");
        }
        text.push_str(&entry);
        text.push('\n');
        expected.push(entry);
    }
    assert!(text.len() > 12 << 20, "several chunks");
    let (format, events) = load("servico.log", &text);
    assert_eq!(format, "log4j");
    assert_eq!(events.len(), expected.len());
    for (event, entry) in events.iter().zip(&expected) {
        assert_eq!(event.raw.trim_end_matches('\n'), entry.as_str());
    }
    assert_eq!(events[1].message, "Falha 0");
    assert_eq!(events.last().unwrap().message, "Falha 69999");
}

#[test]
fn csv_rows_with_crlf_and_header_across_chunks() {
    let mut text = String::from("Data;Nível;Mensagem\r\n");
    let rows = 220_000;
    for i in 0..rows {
        let _ = write!(text, "28/09/2026 {:02}:{:02}:{:02};Erro;Mensagem {i}", i / 3600 % 24, i / 60 % 60, i % 60);
        if i + 1 < rows {
            text.push_str("\r\n");
        }
    }
    assert!(text.len() > 8 << 20);
    let (format, events) = load("planilha.csv", &text);
    assert_eq!(format, "csv-semicolon");
    assert_eq!(events.len(), rows);
    for i in [0, 1, 65_535, 131_072, rows - 1] {
        assert_eq!(events[i].message, format!("Mensagem {i}"), "row {i}");
    }
}

#[test]
fn w3c_comments_are_skipped_in_every_chunk() {
    let mut text = String::from("#Software: IIS\n#Fields: date time cs-method cs-uri-stem sc-status\n");
    let mut requests = 0;
    for i in 0..300_000 {
        if i % 1000 == 0 {
            text.push_str("#Date: 2026-09-28 10:00:00\n");
        }
        let _ = writeln!(text, "2026-09-28 10:{:02}:{:02} GET /api/item/{i} 200", i / 60 % 60, i % 60);
        requests += 1;
    }
    assert!(text.len() > 12 << 20);
    let (format, events) = load("u_ex260928.log", &text);
    assert_eq!(format, "w3c");
    assert_eq!(events.len(), requests);
    assert!(events.iter().all(|e| !e.raw.starts_with('#')));
}

#[test]
fn json_array_records_keep_their_order() {
    let mut text = String::from("[\n");
    let records = 120_000;
    for i in 0..records {
        let _ = write!(text, "  {{\n    \"timestamp\": \"2026-09-28T10:00:00Z\",\n    \"message\": \"registro {i}\"\n  }}");
        text.push_str(if i + 1 < records { ",\n" } else { "\n" });
    }
    text.push(']');
    let (_, events) = load("exportacao.json", &text);
    assert_eq!(events.len(), records);
    for i in [0, 1, 4_095, 60_000, records - 1] {
        assert_eq!(events[i].message, format!("registro {i}"));
    }
}
