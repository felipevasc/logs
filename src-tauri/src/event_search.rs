//! Incremental regex over parsed fields. Hits live in SQLite; each step is
//! bounded and resumes by source position, including Cases with sparse IDs.
use crate::{
    model::Event,
    query::Filter,
    search_regex::{Matcher, Pattern},
    text_reader::Owner,
    AppState, SourceData,
};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::Instant,
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
struct Data {
    conn: Connection,
    _file: tempfile::NamedTempFile,
    cursor: usize,
    total: usize,
    matches: u64,
    skipped: u64,
    limited: u64,
    buckets: Vec<u64>,
}
struct Search {
    owner: Owner,
    pattern: Pattern,
    scope: String,
    filters: Vec<Filter>,
    data: Mutex<Data>,
    touched: Mutex<Instant>,
}
static SEARCHES: LazyLock<Mutex<HashMap<String, Arc<Search>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub search: String,
    pub rows: Vec<Value>,
    pub matches: u64,
    pub scanned: usize,
    pub total: usize,
    pub complete: bool,
    pub skipped: u64,
    pub limited: u64,
    pub offset: u64,
    pub buckets: Vec<u64>,
    pub filter_count: usize,
    pub subject_scope: String,
}
pub(crate) fn start(
    state: &AppState,
    filters: &[Filter],
    pattern: Pattern,
    scope: String,
) -> Result<Page, String> {
    Matcher::new(&pattern)?;
    crate::workspace::validate(filters)?;
    if !["line", "message", "event"].contains(&scope.as_str()) {
        return Err("Use linha física, mensagem ou evento multilinha.".into());
    }
    let source = crate::analysis_runtime::source(state);
    let total = match &*source {
        SourceData::Indexed(index) => index.lines.len(),
        SourceData::Memory(events) => events.len(),
        SourceData::None => 0,
    };
    let file = tempfile::NamedTempFile::new().map_err(err)?;
    let conn = Connection::open(file.path()).map_err(err)?;
    conn.execute_batch("PRAGMA journal_mode=MEMORY;PRAGMA cache_size=-4096;PRAGMA max_page_count=1048576;CREATE TABLE hits(pos INTEGER PRIMARY KEY,body TEXT NOT NULL)").map_err(err)?;
    let id = uuid::Uuid::new_v4().to_string();
    let search = Arc::new(Search {
        owner: Owner::current()?,
        pattern,
        scope,
        filters: filters.to_vec(),
        data: Mutex::new(Data {
            conn,
            _file: file,
            cursor: 0,
            total,
            matches: 0,
            skipped: 0,
            limited: 0,
            buckets: vec![0; 128],
        }),
        touched: Mutex::new(Instant::now()),
    });
    {
        let mut searches = SEARCHES.lock();
        searches.retain(|_, s| s.touched.lock().elapsed().as_secs() < 1800);
        if searches.len() >= 4 {
            if let Some(old) = searches
                .iter()
                .min_by_key(|(_, s)| *s.touched.lock())
                .map(|(id, _)| id.clone())
            {
                searches.remove(&old);
            }
        }
        searches.insert(id.clone(), search);
    }
    step(state, &id)
}
fn get(id: &str) -> Result<Arc<Search>, String> {
    let search = SEARCHES
        .lock()
        .get(id)
        .cloned()
        .ok_or("Busca expirada. Execute novamente.")?;
    search.owner.validate()?;
    *search.touched.lock() = Instant::now();
    Ok(search)
}
fn matched(event: &Event, scope: &str, matcher: &Matcher) -> Result<Option<Value>, String> {
    let subject = if scope == "message" {
        event.message.as_str()
    } else if event.raw.is_empty() {
        event.message.as_str()
    } else {
        event.raw.as_str()
    };
    if subject.len() > crate::search_regex::SUBJECT_LIMIT {
        return Err("Registro acima do limite de busca.".into());
    }
    let text = subject;
    let found = if scope == "line" {
        matcher.find_lines(text)?
    } else {
        matcher.find_spans(text)?
    };
    if let Some(first) = found.spans.first() {
        let first_byte = first.byte_start;
        let mut start = first_byte.saturating_sub(320);
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (start + 12_000).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let base = text[..start].encode_utf16().count();
        let last = text[..end].encode_utf16().count();
        let spans: Vec<_> = found
            .spans
            .into_iter()
            .filter(|s| s.end >= base && s.start <= last)
            .map(|mut s| {
                s.start = s.start.max(base) - base;
                s.end = s.end.min(last) - base;
                s.captures.clear();
                s
            })
            .collect();
        return Ok(Some(
            json!({"id":event.id,"event_ref":event.event_ref,"timestamp":event.timestamp,"source":event.source.chars().take(300).collect::<String>(),"level":event.level,"text":&text[start..end],"spans":spans,"truncatedBefore":start>0,"truncatedAfter":end<text.len(),"matchByteOffset":first_byte.to_string(),"matchesComplete":found.complete}),
        ));
    }
    Ok(None)
}
pub(crate) fn step(state: &AppState, id: &str) -> Result<Page, String> {
    let search = get(id)?;
    let mut data = search.data.lock();
    let matcher = Matcher::new(&search.pattern)?;
    let filters = crate::query::prepare(&search.filters);
    let source = crate::analysis_runtime::source(state);
    let begin = Instant::now();
    let mut cursor = data.cursor;
    let mut bytes = 0;
    let mut matches = 0;
    let mut skipped = 0;
    let mut limited = 0;
    let total = data.total;
    let mut buckets = vec![0u64; 128];
    let tx = data.conn.transaction().map_err(err)?;
    let mut insert = tx.prepare("INSERT INTO hits VALUES(?1,?2)").map_err(err)?;
    crate::workspace::with_selection(state, &[], |selection| {
        while cursor
            < match &*source {
                SourceData::Indexed(index) => index.lines.len(),
                SourceData::Memory(events) => events.len(),
                SourceData::None => 0,
            }
            && bytes < 8 << 20
            && begin.elapsed().as_millis() < 180
        {
            crate::operations::check()?;
            if let SourceData::Indexed(index) = &*source {
                if index.lines.at(cursor).len as usize > crate::search_regex::SUBJECT_LIMIT {
                    cursor += 1;
                    skipped += 1;
                    continue;
                }
            }
            let event = match &*source {
                SourceData::Memory(events) => events.get(cursor).cloned(),
                _ => selection.event(cursor),
            };
            let position = cursor;
            cursor += 1;
            let Some(event) = event else {
                continue;
            };
            bytes += event.raw.len() + event.message.len();
            if !filters
                .iter()
                .all(|filter| crate::query::matches(&event, filter))
            {
                continue;
            }
            match matched(&event, &search.scope, &matcher) {
                Ok(Some(hit)) => {
                    if hit["matchesComplete"] == false {
                        limited += 1;
                    }
                    let body = serde_json::to_string(&hit).map_err(err)?;
                    insert.execute(params![position, body]).map_err(err)?;
                    matches += 1;
                    buckets[(position.saturating_mul(128) / total.max(1)).min(127)] += 1;
                }
                Ok(None) => {}
                Err(error) => {
                    crate::operations::check()?;
                    let _ = error;
                    skipped += 1;
                }
            }
        }
        Ok::<_, String>(())
    })??;
    search.owner.validate()?;
    drop(insert);
    tx.commit().map_err(err)?;
    data.cursor = cursor;
    data.matches += matches;
    data.skipped += skipped;
    data.limited += limited;
    for (i, n) in buckets.into_iter().enumerate() {
        data.buckets[i] += n;
    }
    drop(data);
    page(id, 0)
}
pub(crate) fn page(id: &str, offset: u64) -> Result<Page, String> {
    let search = get(id)?;
    let data = search.data.lock();
    let mut stmt = data
        .conn
        .prepare("SELECT body FROM hits ORDER BY pos LIMIT 80 OFFSET ?1")
        .map_err(err)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    for body in stmt
        .query_map([offset], |r| r.get::<_, String>(0))
        .map_err(err)?
    {
        let body = body.map_err(err)?;
        bytes += body.len();
        if bytes > 2 << 20 {
            return Err("A página de resultados excede 2 MiB. Reduza o contexto.".into());
        }
        rows.push(serde_json::from_str::<Value>(&body).map_err(err)?);
    }
    Ok(Page {
        search: id.into(),
        rows,
        matches: data.matches,
        scanned: data.cursor,
        total: data.total,
        complete: data.cursor == data.total,
        skipped: data.skipped,
        limited: data.limited,
        offset,
        buckets: data.buckets.clone(),
        filter_count: search.filters.len(),
        subject_scope: search.scope.clone(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_matches_center_preview_and_physical_scope_does_not_cross_lines() {
        let p = Pattern {
            engine: "pcre2".into(),
            text: "(?<=key=)secret".into(),
            case_sensitive: true,
            multiline: false,
            dot_all: false,
        };
        let m = Matcher::new(&p).unwrap();
        let mut e = Event::empty();
        e.raw = format!("{}key=secret", "x".repeat(60_000));
        let hit = matched(&e, "event", &m).unwrap().unwrap();
        assert!(hit["text"].as_str().unwrap().contains("secret"));
        assert_eq!(hit["truncatedBefore"], true);
        let m = Matcher::new(&Pattern {
            text: "a\\s+b".into(),
            ..p
        })
        .unwrap();
        e.raw = "a\nb".into();
        assert!(matched(&e, "line", &m).unwrap().is_none());
        assert!(matched(&e, "event", &m).unwrap().is_some());
    }
    #[test]
    fn message_and_raw_context_preserve_search_subject_and_line_anchors() {
        let pattern = Pattern {
            engine: "rust".into(),
            text: "^user=ana$".into(),
            case_sensitive: true,
            multiline: false,
            dot_all: false,
        };
        let matcher = Matcher::new(&pattern).unwrap();
        let mut event = Event::empty();
        event.message = "user=ana".into();
        event.raw = "{\"message\":\"user=ana\"}".into();
        assert!(matched(&event, "message", &matcher).unwrap().is_some());
        assert!(matched(&event, "event", &matcher).unwrap().is_none());
        event.raw = "header\r\nuser=ana\r\nuser=ana\r\n".into();
        let result = matched(&event, "line", &matcher).unwrap().unwrap();
        assert_eq!(result["spans"].as_array().unwrap().len(), 2);
        assert_eq!(result["matchByteOffset"], "8");
    }
}
