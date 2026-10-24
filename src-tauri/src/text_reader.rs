//! Direct text: bounded native reads, incremental on-disk line/match directories.
//! No parser, complete line count, DuckDB or inverted index on the open path.
use crate::{
    analysis_context::Identity,
    search_regex::{Matcher, Pattern, Span},
};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Instant, SystemTime},
};
const PAGE: usize = 80;
const WINDOW: usize = 16 << 10;
const STEP_BYTES: u64 = 8 << 20;
const MAX_SESSIONS: usize = 6;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    identity: Option<Identity>,
    generation: Option<u64>,
    case_key: Option<String>,
    content_token: Option<String>,
}
impl Owner {
    pub(crate) fn current() -> Result<Self, String> {
        let context =
            crate::analysis_runtime::current().ok_or("Contexto de análise não admitido.")?;
        Ok(Self {
            identity: context.identity.clone(),
            generation: context.source_generation,
            case_key: context.case_key.clone(),
            content_token: context.case_content_token.clone(),
        })
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self != &Self::current()? {
            return Err("O Caso, a configuração ou a fonte mudou. Execute novamente.".into());
        }
        crate::analysis_runtime::validate_result_owner()
    }
}
struct Directory {
    conn: Connection,
    _file: tempfile::NamedTempFile,
    indexed: u64,
    index_scan: u64,
    lines: u64,
    search_offset: u64,
    search_line: u64,
    search_scan: u64,
    search_buffer: Vec<u8>,
    pattern: Option<Pattern>,
    skipped: u64,
    limited: u64,
    matches: u64,
    buckets: Vec<u64>,
}
struct Session {
    owner: Mutex<Owner>,
    path: PathBuf,
    file: Mutex<File>,
    size: u64,
    modified: Option<SystemTime>,
    directory: Mutex<Directory>,
    touched: Mutex<Instant>,
}
static SESSIONS: LazyLock<Mutex<HashMap<String, Arc<Session>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub session: String,
    pub path: String,
    pub bytes: String,
    pub indexed_bytes: String,
    pub indexed_lines: u64,
    pub searched_bytes: String,
    pub matches: u64,
    pub index_complete: bool,
    pub search_complete: bool,
    pub skipped_records: u64,
    pub limited_records: u64,
    pub buckets: Vec<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Line {
    pub line: u64,
    pub offset: String,
    pub bytes: String,
    pub text: String,
    pub window_start: usize,
    pub truncated_before: bool,
    pub truncated_after: bool,
    pub lossy: bool,
    pub spans: Vec<Span>,
    pub matches_complete: bool,
    pub highlights: Vec<crate::search_highlights::Mark>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub action: String,
    pub session: Option<String>,
    pub path: Option<String>,
    pub pattern: Option<Pattern>,
    pub line: Option<u64>,
    pub offset: Option<u64>,
    pub limit: Option<usize>,
    pub matches_only: Option<bool>,
    pub byte_in_line: Option<usize>,
    pub rules: Option<Vec<crate::search_highlights::Rule>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Response {
    pub status: Option<Status>,
    pub lines: Vec<Line>,
}
fn regular(path: &Path) -> Result<File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = options.open(path).map_err(err)?;
    if !file.metadata().map_err(err)?.is_file() {
        return Err("Selecione um arquivo de texto regular.".into());
    }
    Ok(file)
}
impl Session {
    fn validate(&self) -> Result<(), String> {
        self.owner.lock().validate()?;
        self.validate_file()
    }
    fn validate_file(&self) -> Result<(), String> {
        if crate::analysis_runtime::visibility_restricted() {
            return Err("O texto direto não pode contornar exclusões. Use os eventos visíveis e seu contexto.".into());
        }
        let metadata = std::fs::metadata(&self.path).map_err(err)?;
        if metadata.len() != self.size || metadata.modified().ok() != self.modified {
            return Err(
                "O arquivo mudou durante a leitura. Reabra para obter uma nova referência.".into(),
            );
        }
        *self.touched.lock() = Instant::now();
        Ok(())
    }
    fn status(&self, id: &str, d: &Directory) -> Result<Status, String> {
        Ok(Status {
            session: id.into(),
            path: self.path.to_string_lossy().into_owned(),
            bytes: self.size.to_string(),
            indexed_bytes: (d.indexed + d.index_scan).to_string(),
            indexed_lines: d.lines + u64::from(d.index_scan > 0),
            searched_bytes: (d.search_offset + d.search_scan).to_string(),
            matches: d.matches,
            index_complete: d.indexed == self.size,
            search_complete: d.pattern.is_none() || d.search_offset == self.size,
            skipped_records: d.skipped,
            limited_records: d.limited,
            buckets: d.buckets.clone(),
        })
    }
}
/// Read at most one wave of a physical record. Callers checkpoint long lines,
/// so even a file with no newlines can show its first window immediately.
fn record(reader: &mut impl BufRead, budget: u64) -> Result<(Vec<u8>, u64, bool), String> {
    let mut output = Vec::new();
    let mut consumed = 0u64;
    loop {
        crate::operations::check()?;
        let bytes = reader.fill_buf().map_err(err)?;
        if bytes.is_empty() {
            break;
        }
        let available = bytes.len().min((budget - consumed) as usize);
        if available == 0 {
            return Ok((output, consumed, false));
        }
        let bytes = &bytes[..available];
        let n = memchr::memchr(b'\n', bytes).map_or(bytes.len(), |i| i + 1);
        let keep = n.min(crate::search_regex::SUBJECT_LIMIT.saturating_sub(output.len()));
        output.extend_from_slice(&bytes[..keep]);
        let end = bytes[n - 1] == b'\n';
        reader.consume(n);
        consumed += n as u64;
        if end {
            return Ok((output, consumed, true));
        }
    }
    Ok((output, consumed, true))
}
fn open(path: &str) -> Result<(String, Arc<Session>), String> {
    let path = std::fs::canonicalize(path).map_err(err)?;
    let mut file = regular(&path)?;
    let meta = file.metadata().map_err(err)?;
    let mut signature = [0u8; 4096];
    let n = file.read(&mut signature).map_err(err)?;
    if signature[..n].starts_with(&[0xff, 0xfe])
        || signature[..n].starts_with(&[0xfe, 0xff])
        || signature[..n].contains(&0)
    {
        return Err("Este arquivo requer decodificação ou parsing. Abra em Eventos para preservar seu formato.".into());
    }
    file.rewind().map_err(err)?;
    let temp = tempfile::NamedTempFile::new().map_err(err)?;
    let conn = Connection::open(temp.path()).map_err(err)?;
    conn.execute_batch("PRAGMA journal_mode=MEMORY;PRAGMA cache_size=-4096;PRAGMA max_page_count=1048576;CREATE TABLE lines(n INTEGER PRIMARY KEY,off INTEGER NOT NULL,len INTEGER NOT NULL,complete INTEGER NOT NULL);CREATE TABLE hits(n INTEGER PRIMARY KEY,off INTEGER NOT NULL,at INTEGER NOT NULL);CREATE INDEX hits_offset ON hits(off);").map_err(err)?;
    let session = Arc::new(Session {
        owner: Mutex::new(Owner::current()?),
        path,
        file: Mutex::new(file),
        size: meta.len(),
        modified: meta.modified().ok(),
        touched: Mutex::new(Instant::now()),
        directory: Mutex::new(Directory {
            conn,
            _file: temp,
            indexed: 0,
            index_scan: 0,
            lines: 0,
            search_offset: 0,
            search_line: 0,
            search_scan: 0,
            search_buffer: Vec::new(),
            pattern: None,
            skipped: 0,
            limited: 0,
            matches: 0,
            buckets: vec![0; 128],
        }),
    });
    let id = uuid::Uuid::new_v4().to_string();
    let mut sessions = SESSIONS.lock();
    sessions.retain(|_, s| s.touched.lock().elapsed().as_secs() < 1800);
    if sessions.len() >= MAX_SESSIONS {
        if let Some(old) = sessions
            .iter()
            .min_by_key(|(_, s)| *s.touched.lock())
            .map(|(id, _)| id.clone())
        {
            sessions.remove(&old);
        }
    }
    sessions.insert(id.clone(), Arc::clone(&session));
    Ok((id, session))
}
fn step(session: &Session, d: &mut Directory, search: bool) -> Result<(), String> {
    let mut at = if search { d.search_offset } else { d.indexed };
    let mut pending = if search { d.search_scan } else { d.index_scan };
    let start = at + pending;
    if start >= session.size {
        return Ok(());
    }
    let mut file = session.file.lock();
    file.seek(SeekFrom::Start(start)).map_err(err)?;
    let mut reader = BufReader::with_capacity(64 << 10, &mut *file);
    let matcher = if search {
        d.pattern.as_ref().map(Matcher::new).transpose()?
    } else {
        None
    };
    let mut line = if search { d.search_line } else { d.lines };
    let mut retained = if search {
        d.search_buffer.clone()
    } else {
        Vec::new()
    };
    let began = Instant::now();
    let tx = d.conn.transaction().map_err(err)?;
    // Prepare once per wave, rather than parsing this UPSERT for every physical
    // line. Repeated SQL preparation dominates large-file scans otherwise.
    let mut insert_line=tx.prepare("INSERT INTO lines(n,off,len,complete) VALUES(?1,?2,?3,?4) ON CONFLICT(n) DO UPDATE SET len=MAX(lines.len,excluded.len),complete=MAX(lines.complete,excluded.complete)").map_err(err)?;
    let mut insert_hit = tx
        .prepare("INSERT INTO hits(n,off,at) VALUES(?1,?2,?3)")
        .map_err(err)?;
    let mut matched = 0;
    let mut buckets = vec![0u64; 128];
    let mut skipped = 0;
    let mut limited = 0;
    while at + pending < session.size
        && (at + pending - start < STEP_BYTES && began.elapsed().as_millis() < 180
            || at + pending == start)
    {
        crate::operations::check()?;
        let (bytes, len, ended) = record(&mut reader, STEP_BYTES - (at + pending - start))?;
        if len == 0 {
            break;
        }
        pending += len;
        if search {
            let keep = bytes
                .len()
                .min(crate::search_regex::SUBJECT_LIMIT.saturating_sub(retained.len()));
            retained.extend_from_slice(&bytes[..keep]);
        }
        let complete = ended || at + pending == session.size;
        insert_line
            .execute(params![line + 1, at, pending, complete])
            .map_err(err)?;
        if !complete {
            break;
        }
        line += 1;
        if let Some(matcher) = &matcher {
            if pending > crate::search_regex::SUBJECT_LIMIT as u64 {
                skipped += 1;
            } else {
                let text = String::from_utf8_lossy(&retained);
                let found = matcher.find_spans(text.trim_end_matches(['\r', '\n']))?;
                if !found.complete {
                    limited += 1;
                }
                if let Some(first) = found.spans.first() {
                    insert_hit
                        .execute(params![line, at, first.byte_start])
                        .map_err(err)?;
                    matched += 1;
                    buckets
                        [((at as u128 * 128 / session.size.max(1) as u128) as usize).min(127)] += 1;
                }
            }
        }
        at += pending;
        pending = 0;
        retained.clear();
    }
    session.validate()?;
    crate::operations::check()?;
    drop(insert_hit);
    drop(insert_line);
    tx.commit().map_err(err)?;
    if at + pending >= d.indexed + d.index_scan {
        d.indexed = at;
        d.index_scan = pending;
        d.lines = line;
    }
    if search {
        d.search_offset = at;
        d.search_line = line;
        d.search_scan = pending;
        d.search_buffer = retained;
        d.matches += matched;
        for (i, n) in buckets.into_iter().enumerate() {
            d.buckets[i] += n;
        }
        d.skipped += skipped;
        d.limited += limited;
    }
    Ok(())
}
fn page(session: &Session, d: &Directory, r: &Request) -> Result<Vec<Line>, String> {
    let limit = r.limit.unwrap_or(PAGE).clamp(1, 160);
    let only = r.matches_only.unwrap_or(false);
    let sql = if only {
        "SELECT l.n,l.off,l.len,h.at,l.complete FROM hits h JOIN lines l USING(n) ORDER BY l.n LIMIT ?1 OFFSET ?2"
    } else {
        "SELECT n,off,len,0,complete FROM lines WHERE n>=?2 ORDER BY n LIMIT ?1"
    };
    let mut statement = d.conn.prepare(sql).map_err(err)?;
    let locations = statement
        .query_map(
            params![
                limit,
                r.offset
                    .unwrap_or(if only { 0 } else { r.line.unwrap_or(1) })
            ],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, usize>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            },
        )
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;
    let matcher = d.pattern.as_ref().map(Matcher::new).transpose()?;
    let mut result = Vec::new();
    let mut file = session.file.lock();
    let mut response_bytes = 0usize;
    let colors = crate::search_highlights::Compiled::new(r.rules.as_deref().unwrap_or_default())?;
    for (line, off, len, first, line_complete) in locations {
        crate::operations::check()?;
        let mut start = r
            .byte_in_line
            .unwrap_or(if only {
                first.saturating_sub(WINDOW / 3)
            } else {
                0
            })
            .min(len.saturating_sub(1) as usize);
        // Read a full bounded subject for authoritative regex anchors/lookarounds,
        // then crop around the actual match. Never re-evaluate a window as a record.
        file.seek(SeekFrom::Start(off)).map_err(err)?;
        let mut bytes = vec![0; len.min(crate::search_regex::SUBJECT_LIMIT as u64) as usize];
        file.read_exact(&mut bytes).map_err(err)?;
        let decoded = String::from_utf8_lossy(&bytes);
        let lossy = matches!(decoded, std::borrow::Cow::Owned(_));
        let full = decoded.trim_end_matches(['\r', '\n']);
        let found = if line_complete && len <= crate::search_regex::SUBJECT_LIMIT as u64 {
            matcher.as_ref().map(|m| m.find_spans(full)).transpose()?
        } else {
            None
        };
        if len > crate::search_regex::SUBJECT_LIMIT as u64 && start >= bytes.len() {
            file.seek(SeekFrom::Start(off + start as u64))
                .map_err(err)?;
            bytes = vec![0; (len - start as u64).min(WINDOW as u64) as usize];
            file.read_exact(&mut bytes).map_err(err)?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            result.push(Line {
                line,
                offset: off.to_string(),
                bytes: len.to_string(),
                text,
                window_start: start,
                truncated_before: start > 0,
                truncated_after: !line_complete || (start as u64 + WINDOW as u64) < len,
                lossy: true,
                spans: vec![],
                matches_complete: false,
                highlights: vec![],
            });
            continue;
        }
        start = start.min(full.len());
        while !full.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (start + WINDOW).min(full.len());
        while !full.is_char_boundary(end) {
            end -= 1;
        }
        let start16 = full[..start].encode_utf16().count();
        let end16 = full[..end].encode_utf16().count();
        let complete = found.as_ref().is_none_or(|f| f.complete)
            && line_complete
            && len <= crate::search_regex::SUBJECT_LIMIT as u64;
        let spans = found
            .into_iter()
            .flat_map(|f| f.spans)
            .filter(|s| s.end >= start16 && s.start <= end16)
            .map(|mut s| {
                s.start = s.start.max(start16) - start16;
                s.end = s.end.min(end16) - start16;
                s.captures.clear();
                s
            })
            .collect();
        let highlights = (if line_complete && len <= crate::search_regex::SUBJECT_LIMIT as u64 {
            colors.apply(full)?
        } else {
            vec![]
        })
        .into_iter()
        .filter(|m| m.end >= start16 && m.start <= end16)
        .map(|mut m| {
            m.start = m.start.max(start16) - start16;
            m.end = m.end.min(end16) - start16;
            m
        })
        .collect();
        let row = Line {
            line,
            offset: off.to_string(),
            bytes: len.to_string(),
            text: full[start..end].into(),
            window_start: start,
            truncated_before: start > 0,
            truncated_after: !line_complete || end < full.len() || len > bytes.len() as u64,
            lossy,
            spans,
            matches_complete: complete,
            highlights,
        };
        response_bytes += serde_json::to_vec(&row).map_err(err)?.len();
        if response_bytes > 2 << 20 {
            return Err("A página excede 2 MiB. Reduza as regras de destaque ou a quantidade de linhas solicitadas.".into());
        }
        result.push(row);
    }
    Ok(result)
}
pub(crate) fn execute(r: Request) -> Result<Response, String> {
    if r.action == "open" {
        if crate::analysis_runtime::visibility_restricted() {
            return Err(
                "Existem exclusões neste Caso. Explore somente os eventos visíveis.".into(),
            );
        }
        let (id, session) = open(r.path.as_deref().ok_or("Informe o arquivo.")?)?;
        session.validate()?;
        let mut d = session.directory.lock();
        step(&session, &mut d, false)?;
        return Ok(Response {
            status: Some(session.status(&id, &d)?),
            lines: page(&session, &d, &r)?,
        });
    }
    let id = r.session.as_deref().ok_or("Informe a sessão de texto.")?;
    let session = SESSIONS
        .lock()
        .get(id)
        .cloned()
        .ok_or("Sessão de texto expirada. Reabra o arquivo.")?;
    if r.action == "rebind" {
        let fresh = Owner::current()?;
        fresh.validate()?;
        let directory = session.directory.lock();
        let mut owner = session.owner.lock();
        if owner.identity != fresh.identity
            || owner.case_key != fresh.case_key
            || owner.content_token != fresh.content_token
        {
            return Err("O Caso ou sua configuração mudou. Reabra o texto.".into());
        }
        session.validate_file()?;
        *owner = fresh;
        drop(owner);
        return Ok(Response {
            status: Some(session.status(id, &directory)?),
            lines: page(&session, &directory, &r)?,
        });
    }
    session.validate()?;
    if r.action == "close" {
        SESSIONS.lock().remove(id);
        return Ok(Response {
            status: None,
            lines: vec![],
        });
    }
    let mut d = session.directory.lock();
    match r.action.as_str() {
        "search" => {
            if let Some(p) = &r.pattern {
                Matcher::new(p)?;
            }
            d.conn.execute("DELETE FROM hits", []).map_err(err)?;
            d.pattern = r.pattern.clone();
            d.search_offset = 0;
            d.search_line = 0;
            d.search_scan = 0;
            d.search_buffer.clear();
            d.matches = 0;
            d.buckets.fill(0);
            d.skipped = 0;
            d.limited = 0;
        }
        "step" => {
            if d.pattern.is_some() {
                step(&session, &mut d, true)?;
            } else {
                step(&session, &mut d, false)?;
            }
        }
        "page" | "status" => {}
        _ => return Err("Ação de texto desconhecida.".into()),
    }
    let lines = if r.action == "status" {
        vec![]
    } else {
        page(&session, &d, &r)?
    };
    session.validate()?;
    Ok(Response {
        status: Some(session.status(id, &d)?),
        lines,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_records_handle_crlf_empty_and_final_line() {
        let mut reader = std::io::Cursor::new(b"a\r\n\nlast");
        assert_eq!(
            record(&mut reader, u64::MAX).unwrap(),
            (b"a\r\n".to_vec(), 3, true)
        );
        assert_eq!(
            record(&mut reader, u64::MAX).unwrap(),
            (b"\n".to_vec(), 1, true)
        );
        assert_eq!(
            record(&mut reader, u64::MAX).unwrap(),
            (b"last".to_vec(), 4, true)
        );
        assert_eq!(record(&mut reader, u64::MAX).unwrap().1, 0);
    }
    #[test]
    fn huge_record_has_bounded_retention_and_exact_next_offset() {
        let mut input = vec![b'x'; crate::search_regex::SUBJECT_LIMIT + 100];
        input.extend_from_slice(b"\nnext\n");
        let mut reader = std::io::Cursor::new(input);
        let (bytes, len, complete) = record(&mut reader, u64::MAX).unwrap();
        assert!(complete);
        assert_eq!(bytes.len(), crate::search_regex::SUBJECT_LIMIT);
        assert_eq!(len, crate::search_regex::SUBJECT_LIMIT as u64 + 101);
        assert_eq!(record(&mut reader, u64::MAX).unwrap().0, b"next\n");
    }
    #[test]
    fn physical_record_can_checkpoint_without_consuming_the_whole_line() {
        let mut reader = std::io::Cursor::new(b"abcdefgh\nnext\n");
        assert_eq!(
            record(&mut reader, 4).unwrap(),
            (b"abcd".to_vec(), 4, false)
        );
        assert_eq!(
            record(&mut reader, 4).unwrap(),
            (b"efgh".to_vec(), 4, false)
        );
        assert_eq!(record(&mut reader, 4).unwrap(), (b"\n".to_vec(), 1, true));
        assert_eq!(
            record(&mut reader, 8).unwrap(),
            (b"next\n".to_vec(), 5, true)
        );
    }
}
