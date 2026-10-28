//! Native side of the "Colunas" flow of the extraction page: how a file's records are framed
//! (what the reader detected, and what a user-defined record start would produce) and the
//! exact verification of candidate patterns with the same PCRE2 engine that extracts the columns.
use crate::search_regex::{Matcher, Pattern};
use serde_json::{json, Value};

const FRAME_BYTES: usize = 4 << 20;
const RECORD_TEXT_LIMIT: usize = 16 << 10;
const MAX_PATTERNS: usize = 120;
const MAX_TEXTS: usize = 200;
const MAX_TEXT_BYTES: usize = 2 << 20;

fn utf16(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or(0, |s| s.encode_utf16().count())
}

/// First records of a file framed by `pattern` (a user record start, in the syntax of the Rust
/// `regex` crate) or, without one, by whatever the reader detects for the file.
pub(crate) fn frame(path: &str, pattern: Option<&str>, count: usize) -> Result<Value, String> {
    use std::io::Read;
    let count = count.clamp(1, 400);
    let file = crate::case_archive_format::open_regular(std::path::Path::new(path)).map_err(|e| format!("Não foi possível abrir '{path}': {e}"))?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let mut bytes = Vec::new();
    file.take(FRAME_BYTES as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let truncated_file = total > bytes.len() as u64;
    if truncated_file {
        // Never cut a line in half: the last (partial) line is dropped.
        if let Some(at) = bytes.iter().rposition(|&b| b == b'\n') {
            bytes.truncate(at + 1);
        }
    }
    let (format, detected) = crate::sources::detected_framing(&bytes)?;
    let user = match pattern.map(|p| p.trim_matches(['\r', '\n'])).filter(|p| !p.is_empty()) {
        Some(p) => {
            if p.len() > 2048 {
                return Err("O início de registro excede 2 KiB.".into());
            }
            Some((p, regex::RegexBuilder::new(p).size_limit(8 << 20).build().map_err(|e| format!("Início de registro inválido: {e}"))?))
        }
        None => None,
    };
    let auto = match (&user, &detected) {
        (None, Some(p)) => Some(regex::Regex::new(p).map_err(|e| e.to_string())?),
        _ => None,
    };
    let start = user.as_ref().map(|(_, re)| re).or(auto.as_ref());
    let mut records: Vec<(usize, String)> = Vec::new();
    let mut offset = 0usize;
    let mut cut_short = false;
    for raw in bytes.split(|&b| b == b'\n') {
        let line_at = offset;
        offset += raw.len() + 1;
        let line = crate::sources::decode_text(raw.strip_suffix(b"\r").unwrap_or(raw));
        if line.trim().is_empty() {
            continue;
        }
        let begins = start.is_none_or(|re| re.is_match(&line));
        match records.last_mut() {
            Some(last) if !begins => {
                if last.1.len() < RECORD_TEXT_LIMIT {
                    last.1.push('\n');
                    last.1.push_str(&line);
                }
            }
            _ => {
                if records.len() >= count {
                    cut_short = true;
                    break;
                }
                records.push((line_at, line.into_owned()));
            }
        }
    }
    let records: Vec<Value> = records
        .into_iter()
        .map(|(offset, text)| {
            let lines = text.matches('\n').count() + 1;
            json!({"offset": offset.to_string(), "text": text, "lines": lines})
        })
        .collect();
    Ok(json!({
        "format": format,
        "detected": detected,
        "pattern": user.as_ref().map(|(p, _)| *p),
        "records": records,
        "more": cut_short || truncated_file,
    }))
}

/// Every candidate against every text: the range of its first capture group in the first match
/// (UTF-16 units, like the page), or `null`. A pattern the engine rejects or abandons says so.
pub(crate) fn verify(texts: &[String], patterns: &[String], counts: bool) -> Result<Value, String> {
    if texts.is_empty() || texts.len() > MAX_TEXTS || texts.iter().map(String::len).sum::<usize>() > MAX_TEXT_BYTES {
        return Err("Informe de 1 a 200 registros (até 2 MiB no total).".into());
    }
    if patterns.is_empty() || patterns.len() > MAX_PATTERNS || patterns.iter().any(|p| p.len() > 4096) {
        return Err("Informe de 1 a 120 padrões de até 4 KiB.".into());
    }
    let mut out = Vec::with_capacity(patterns.len());
    for text in patterns {
        crate::operations::check()?;
        let matcher = match Matcher::new(&Pattern { engine: "pcre2".into(), text: text.clone(), case_sensitive: true, multiline: false, dot_all: false }) {
            Ok(m) => m,
            Err(error) => {
                out.push(json!({"error": error, "ranges": []}));
                continue;
            }
        };
        let mut ranges: Vec<Value> = Vec::with_capacity(texts.len());
        let mut occurrences: Vec<Value> = Vec::new();
        let mut failure: Option<String> = None;
        for subject in texts {
            if failure.is_some() {
                ranges.push(Value::Null);
                occurrences.push(json!(0));
                continue;
            }
            let found = if counts { matcher.all_ranges(subject, 50) } else { matcher.first_ranges(subject).map(|first| first.into_iter().collect()) };
            match found {
                Ok(all) => {
                    ranges.push(match all.first().and_then(|groups| groups.get(1).copied().flatten()) {
                        Some((a, b)) => json!([utf16(subject, a), utf16(subject, b)]),
                        None => Value::Null,
                    });
                    occurrences.push(json!(all.iter().filter(|groups| groups.get(1).copied().flatten().is_some()).count()));
                }
                Err(error) => {
                    if crate::operations::check().is_err() {
                        return Err(error);
                    }
                    failure = Some(error);
                    ranges.push(Value::Null);
                    occurrences.push(json!(0));
                }
            }
        }
        out.push(if counts { json!({"error": failure, "ranges": ranges, "counts": occurrences}) } else { json!({"error": failure, "ranges": ranges}) });
    }
    Ok(json!({"results": out}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_reports_group_one_in_utf16_units() {
        let texts = vec!["ação (sku 42) fim".to_owned(), "nada".to_owned()];
        let patterns = vec![r"\((sku \d+)\)".to_owned(), r"(".to_owned(), r"(?<!\w)(\d+)(?!\w)".to_owned()];
        let r = verify(&texts, &patterns, false).unwrap();
        let results = r["results"].as_array().unwrap();
        assert_eq!(results[0]["ranges"], json!([[6, 12], null]));
        assert!(results[1]["error"].is_string());
        assert_eq!(results[2]["ranges"], json!([[10, 12], null]));
        // with counts: how many times each pattern's group 1 occurs in a text
        let counted = verify(&["80/open, 443/open, 22/closed".to_owned()], &[r"(\d+)/open".to_owned(), r"(\d+)/udp".to_owned()], true).unwrap();
        assert_eq!(counted["results"][0]["counts"], json!([2]));
        assert_eq!(counted["results"][1]["counts"], json!([0]));
        assert_eq!(counted["results"][0]["ranges"], json!([[0, 2]]));
    }

    #[test]
    fn a_user_record_start_changes_how_the_reader_frames_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("regs.log");
        std::fs::write(&path, "REG;1;a\ncontinua 1\nREG;2;b\ncontinua 2\ncontinua 3\n").unwrap();
        let path = path.to_str().unwrap();
        let plain = crate::sources::index_file(path, "auto", None, None, None).unwrap();
        assert_eq!(plain.lines.len(), 5, "without a record start every line is a record");
        let config = crate::sources::TsConfig { record_start: Some("^REG;".into()), ..Default::default() };
        let framed = crate::sources::index_file(path, "auto", None, Some(config.compile().unwrap()), None).unwrap();
        assert_eq!(framed.lines.len(), 2);
        assert_eq!(String::from_utf8_lossy(crate::sources::line_bytes(&framed, 1)), "REG;2;b\ncontinua 2\ncontinua 3");
        // A different record start is a different framing: it never reuses the other's cached metadata.
        let other = crate::sources::TsConfig { record_start: Some("^continua 2".into()), ..Default::default() };
        let again = crate::sources::index_file(path, "auto", None, Some(other.compile().unwrap()), None).unwrap();
        assert_ne!(crate::sources::line_bytes(&again, 0), crate::sources::line_bytes(&framed, 0));
        assert!(crate::sources::TsConfig { record_start: Some("(".into()), ..Default::default() }.compile().is_err(), "an invalid expression is refused when saved");
    }

    #[test]
    fn frame_follows_a_user_record_start_and_the_detected_one() {
        let dir = std::env::temp_dir().join(format!("loginsight-frame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        let mut text = String::new();
        for n in 0..8 {
            text.push_str(&format!("2026-10-23 17:05:{:02} ERRO falha {n}\n\tat a.b.C\n\tat d.e.F\n", n));
        }
        text.push_str("REG;1;x\n");
        std::fs::write(&path, text).unwrap();
        let path = path.to_string_lossy().into_owned();
        let auto = frame(&path, None, 50).unwrap();
        assert!(auto["detected"].is_string(), "a date-led multi-line log is framed by the reader: {auto}");
        assert_eq!(auto["records"][0]["lines"], 3);
        let mine = frame(&path, Some("^REG;"), 50).unwrap();
        assert_eq!(mine["pattern"], "^REG;");
        let texts: Vec<_> = mine["records"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap().to_owned()).collect();
        assert_eq!(texts.len(), 2, "everything before the first REG line is one record: {texts:?}");
        assert!(texts[0].starts_with("2026-10-23 17:05:00") && texts[0].contains("falha 7"));
        assert!(frame(&path, Some("("), 50).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
