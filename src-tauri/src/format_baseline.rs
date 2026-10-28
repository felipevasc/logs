//! Safety net for changes to the readers: how every file of the corpus (`exemplos/*` and
//! `testdata/corpus/*`) is detected and read is recorded in `testdata/format-baseline.json`, and
//! this test fails on any difference. A reader change may only add: when a difference is the
//! expected one, review it in the diff of the baseline and regenerate it with
//! `UPDATE_FORMAT_BASELINE=1 cargo test --lib format_baseline`. See docs/formatos-de-log.md.
use crate::model::CodesConfig;
use crate::sources;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BASELINE: &str = "testdata/format-baseline.json";
/// Absolute paths and identities differ between machines; everything else is part of the digest.
const MACHINE_FIELDS: [&str; 2] = ["arquivo", "caminho"];

fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in [root.join("../exemplos"), root.join("testdata/corpus")] {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().is_some_and(|n| n != "grande.jsonl") {
                files.push(path);
            }
        }
    }
    files.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
    files
}

/// What the app builds for one file: format, records, dates (relative to the first one, so the
/// machine's time zone does not matter) and a digest of every record's content.
pub(crate) fn describe(path: &Path, scratch: &Path) -> Value {
    // Line endings differ between checkouts (Windows converts); the readers must not.
    let bytes = std::fs::read(path).unwrap();
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    let copy = scratch.join(path.file_name().unwrap());
    std::fs::write(&copy, text).unwrap();
    let index = sources::index_file(copy.to_str().unwrap(), "auto", None, None, None).unwrap();
    let codes = CodesConfig::default();
    let mut digest = Sha256::new();
    let mut keys: BTreeMap<String, usize> = BTreeMap::new();
    let (mut stamped, mut first) = (0usize, None::<i64>);
    for i in 0..index.lines.len() {
        let event = sources::event_at(&index, i, &codes, &codes, &[]);
        let relative = event.timestamp.map(|t| t - *first.get_or_insert(t));
        stamped += usize::from(event.timestamp.is_some());
        let mut fields = Map::new();
        for (key, value) in &event.fields {
            if !MACHINE_FIELDS.contains(&key.as_str()) {
                *keys.entry(key.clone()).or_default() += 1;
                fields.insert(key.clone(), value.clone());
            }
        }
        digest.update(json!([relative, event.level, event.source, event.code, event.name, event.message, event.raw, fields]).to_string());
    }
    let digest = format!("{:x}", digest.finalize());
    json!({ "format": index.format, "records": index.lines.len(), "stamped": stamped, "digest": &digest[..16], "fields": keys })
}

pub(crate) fn snapshot() -> Value {
    let scratch = std::env::temp_dir().join(format!("loginsight-baseline-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut files = Map::new();
    for path in corpus() {
        files.insert(path.file_name().unwrap().to_string_lossy().into_owned(), describe(&path, &scratch));
    }
    std::fs::remove_dir_all(&scratch).ok();
    Value::Object(files)
}

#[test]
fn format_baseline() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(BASELINE);
    let now = snapshot();
    if std::env::var_os("UPDATE_FORMAT_BASELINE").is_some() {
        std::fs::write(&file, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
        return;
    }
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).expect("testdata/format-baseline.json")).unwrap();
    let (saved, now) = (saved.as_object().unwrap(), now.as_object().unwrap());
    let mut differences = Vec::new();
    for (name, value) in now {
        match saved.get(name) {
            None => differences.push(format!("{name}: not in the baseline (add it with UPDATE_FORMAT_BASELINE=1)")),
            Some(before) if before != value => differences.push(format!("{name}:\n   before {before}\n   now    {value}")),
            _ => {}
        }
    }
    for name in saved.keys().filter(|n| !now.contains_key(*n)) {
        differences.push(format!("{name}: in the baseline but missing from the corpus"));
    }
    assert!(differences.is_empty(), "the readers changed how corpus files are read:\n{}", differences.join("\n"));
}
