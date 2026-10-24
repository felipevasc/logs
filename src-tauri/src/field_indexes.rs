//! Optional, immutable field-value indexes. Ownership, interpretation and source
//! order are in the key; only complete indexes can narrow canonical queries.
//!
//! Three kinds exist, each a preference the user asks for, never a requirement:
//! * `exact`: equality, exact or case-insensitive ASCII;
//! * `range`: numeric and date bounds (`gt`, `gte`, `lt`, `lte`, `between`);
//! * `terms`: whole words (`term`).
//! An index only supplies a superset of candidate rows. The canonical predicate still runs on
//! them, so removing or never building an index changes speed and nothing else.
use crate::{sources::FileIndex, query::PreparedFilter};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Info { pub field: String, pub kind: String, pub bytes: u64 }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    /// Fields with a complete exact-value index (kept for the existing interface).
    pub fields: Vec<String>,
    pub rows: usize,
    /// Every complete index, whatever its kind, with its size on disk.
    pub indexes: Vec<Info>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind { Exact, Range, Terms }
impl Kind {
    pub(crate) fn parse(text: Option<&str>) -> Result<Self, String> {
        match text.unwrap_or("exact") {
            "exact" => Ok(Self::Exact),
            "range" => Ok(Self::Range),
            "terms" => Ok(Self::Terms),
            other => Err(format!("Tipo de índice desconhecido: {other}.")),
        }
    }
    fn name(self) -> &'static str { match self { Self::Exact => "exact", Self::Range => "range", Self::Terms => "terms" } }
}

/// Whole words of a text: runs of letters and digits, lowercased. The `term` filter and the
/// terms index share this one function, so an index can never disagree with a scan. Words
/// longer than 128 bytes are not words of either.
pub(crate) fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && w.len() <= 128).map(str::to_lowercase)
}

/// Candidate lists longer than this are not returned: denser results keep the vectorized
/// engine path rather than materializing a huge id list.
const EXACT_CAP: usize = 4096;
const WIDE_CAP: usize = 50_000;

fn directory(idx: &FileIndex) -> Result<PathBuf, String> {
    let mut hash = Sha256::new();
    hash.update(b"field-values-v1");
    hash.update(crate::analysis_runtime::fact_namespace());
    hash.update(crate::analysis_runtime::fact_interpretation_signature());
    hash.update(idx.lines.len().to_le_bytes());
    for part in &idx.parts {
        crate::sources::validate_source(part)?;
        hash.update(format!("{:?}|{:?}|{}|{}|{:?}|{:?}|{:?}", part.identity,
            part.physical_file_id, part.base, part.format, part.header, part.calendar,
            part.ts_config.as_ref().map(|c| c.signature())));
        hash.update(format!("{:?}", part.custom));
    }
    Ok(crate::config_dir().join("field-indexes-v1").join(format!("{:x}", hash.finalize())))
}
fn path(idx: &FileIndex, column: &str, kind: Kind) -> Result<PathBuf, String> {
    // Exact indexes keep their original file name, so existing ones stay valid.
    let key = if kind == Kind::Exact { column.to_owned() } else { format!("{}\u{0}{column}", kind.name()) };
    Ok(directory(idx)?.join(format!("{:x}.sqlite3", Sha256::digest(key.as_bytes()))))
}
fn reader(path: &std::path::Path) -> Result<Connection, String> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())
}
fn complete(db: &Connection, column: &str, rows: usize) -> bool {
    db.query_row("SELECT field, rows FROM complete", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?)))
        .is_ok_and(|(field, count)| field == column && count == rows)
}
pub(crate) fn status(idx: &FileIndex) -> Result<Status, String> {
    let mut fields = Vec::new();
    let mut indexes = Vec::new();
    let dir = directory(idx)?;
    if dir.exists() {
        for file in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            crate::operations::check()?;
            let file = file.map_err(|e| e.to_string())?;
            if file.path().extension().is_none_or(|e| e != "sqlite3") { continue; }
            let Ok(db) = reader(&file.path()) else { continue };
            if let Ok(field) = db.query_row("SELECT field FROM complete WHERE rows=?1", [idx.lines.len()], |r| r.get::<_, String>(0)) {
                let kind = db.query_row("SELECT kind FROM complete", [], |r| r.get::<_, String>(0)).unwrap_or_else(|_| "exact".into());
                let bytes = file.metadata().map_or(0, |m| m.len());
                if kind == "exact" { fields.push(field.clone()); }
                indexes.push(Info { field, kind, bytes });
            }
        }
    }
    fields.sort();
    indexes.sort_by(|a, b| (&a.field, &a.kind).cmp(&(&b.field, &b.kind)));
    Ok(Status { fields, rows: idx.lines.len(), indexes })
}
/// Removes one index. Nothing depends on it: queries fall back to the canonical scan.
pub(crate) fn remove(idx: &FileIndex, column: &str, kind: Kind) -> Result<Status, String> {
    let target = path(idx, column, kind)?;
    match std::fs::remove_file(&target) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.to_string()),
    }
    status(idx)
}
pub(crate) fn build(idx: &FileIndex, column: &str, kind: Kind, state: &crate::AppState, app: Option<&tauri::AppHandle>) -> Result<Status, String> {
    if column.is_empty() || column.len() > 1024 || column.chars().any(char::is_control) || column == "_all" {
        return Err("Escolha um campo válido para indexar.".into());
    }
    if kind == Kind::Range && matches!(column, "timestamp" | "id") {
        return Err("Este campo já tem busca por intervalo nativa; não precisa de índice.".into());
    }
    let target = path(idx, column, kind)?;
    if target.exists() && reader(&target).is_ok_and(|db| complete(&db, column, idx.lines.len())) { return status(idx); }
    let dir = target.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    let mut db = Connection::open(temporary.path()).map_err(|e| e.to_string())?;
    let token = crate::operations::current_token();
    db.progress_handler(2000, Some(move || token.cancelled()));
    db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;
        CREATE TABLE complete(field TEXT NOT NULL, rows INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'exact');").map_err(|e| e.to_string())?;
    db.execute_batch(match kind {
        Kind::Exact => "CREATE TABLE entries(value TEXT NOT NULL, folded TEXT NOT NULL, row INTEGER NOT NULL);",
        Kind::Range => "CREATE TABLE entries(num REAL, ms REAL, row INTEGER NOT NULL);",
        Kind::Terms => "CREATE TABLE entries(term TEXT NOT NULL, row INTEGER NOT NULL);",
    }).map_err(|e| e.to_string())?;
    let codes = crate::analysis_runtime::codes(state);
    let system = crate::analysis_runtime::system_codes(state);
    let derived = crate::analysis_runtime::derived(state);
    let disk_limit = crate::resources::spill_bytes(true, dir)?;
    let budget = "O índice excede o orçamento de recursos. Escolha um campo menor ou ajuste os recursos.";
    let mut written = 0u64;
    let transaction = db.transaction().map_err(|e| e.to_string())?;
    {
        let mut insert = transaction.prepare(match kind {
            Kind::Exact | Kind::Range => "INSERT INTO entries VALUES(?1,?2,?3)",
            Kind::Terms => "INSERT INTO entries VALUES(?1,?2)",
        }).map_err(|e| e.to_string())?;
        for row in 0..idx.lines.len() {
            crate::operations::check()?;
            let event = crate::sources::event_at(idx, row, &codes, &system, &derived);
            match kind {
                Kind::Exact => if let Some(value) = event.col_ref(column) {
                    written = written.saturating_add((value.len() as u64).saturating_mul(4).saturating_add(128));
                    if value.len() > crate::resources::batch_bytes() || written > disk_limit {
                        return Err(budget.into());
                    }
                    insert.execute(params![value.as_ref(), value.to_ascii_lowercase(), row]).map_err(|e| e.to_string())?;
                },
                Kind::Range => {
                    // The same two readings the canonical filter uses: the column's number, and
                    // the column's text as an instant.
                    let num = event.col_num(column).filter(|n| !n.is_nan());
                    let ms = event.col_str(column).and_then(|t| crate::sources::text_to_ms(&t)).map(|v| v as f64);
                    if num.is_some() || ms.is_some() {
                        written = written.saturating_add(48);
                        if written > disk_limit { return Err(budget.into()); }
                        insert.execute(params![num, ms, row]).map_err(|e| e.to_string())?;
                    }
                }
                Kind::Terms => if let Some(value) = event.col_ref(column) {
                    if value.len() > crate::resources::batch_bytes() {
                        return Err(budget.into());
                    }
                    let mut seen = std::collections::BTreeSet::new();
                    for word in tokens(&value) {
                        if seen.contains(&word) { continue; }
                        written = written.saturating_add(word.len() as u64 * 3 + 48);
                        if written > disk_limit { return Err(budget.into()); }
                        insert.execute(params![word, row]).map_err(|e| e.to_string())?;
                        seen.insert(word);
                    }
                },
            }
            if row % 2048 == 0 {
                crate::emit_progress(app, "índice", "Indexando valores do campo", row, idx.lines.len(), "registros", true);
            }
        }
    }
    transaction.execute_batch(match kind {
        Kind::Exact => "CREATE INDEX exact_values ON entries(value,row); CREATE INDEX folded_values ON entries(folded,row);",
        Kind::Range => "CREATE INDEX num_values ON entries(num,row); CREATE INDEX ms_values ON entries(ms,row);",
        Kind::Terms => "CREATE INDEX term_values ON entries(term,row);",
    }).map_err(|e| e.to_string())?;
    crate::operations::check()?;
    crate::analysis_runtime::validate_result_owner()?;
    transaction.execute("INSERT INTO complete VALUES(?1,?2,?3)", params![column, idx.lines.len(), kind.name()]).map_err(|e| e.to_string())?;
    transaction.commit().map_err(|e| e.to_string())?;
    db.close().map_err(|(_, e)| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    crate::operations::check()?;
    crate::analysis_runtime::validate_result_owner()?;
    // Concurrent builders for the same immutable identity may keep the first.
    match temporary.persist_noclobber(&target) {
        Ok(_) => (),
        Err(error) if target.exists() && reader(&target).is_ok_and(|db| complete(&db, column, idx.lines.len())) => drop(error),
        Err(error) => return Err(error.error.to_string()),
    }
    crate::emit_progress(app, "índice", "Índice criado", idx.lines.len(), idx.lines.len(), "registros", false);
    status(idx)
}
fn rows_of(db: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql], cap: usize) -> Result<Option<Vec<usize>>, String> {
    let mut statement = db.prepare(sql).map_err(|e| e.to_string())?;
    let mut rows = statement.query(args).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        found.push(row.get::<_, usize>(0).map_err(|e| e.to_string())?);
        if found.len() > cap { return Ok(None); }
    }
    Ok(Some(found))
}
fn open(idx: &FileIndex, column: &str, kind: Kind) -> Result<Option<Connection>, String> {
    let path = path(idx, column, kind)?;
    if !path.exists() { return Ok(None); }
    let Ok(db) = reader(&path) else { return Ok(None) };
    Ok(complete(&db, column, idx.lines.len()).then_some(db))
}
/// A superset of the rows the canonical predicate accepts, or `None` when no complete index
/// applies or the answer would be too dense to be worth materializing.
pub(crate) fn candidates(idx: &FileIndex, filter: &PreparedFilter) -> Result<Option<Vec<usize>>, String> {
    let op = filter.f.op.as_str();
    let column = &filter.f.column;
    if matches!(op, "equals_exact" | "equals") {
        let (key, value) = if op == "equals_exact" { ("value", filter.f.value.clone()) } else { ("folded", filter.f.value.trim().to_ascii_lowercase()) };
        let Some(db) = open(idx, column, Kind::Exact)? else { return Ok(None) };
        return rows_of(&db, &format!("SELECT row FROM entries WHERE {key}=?1 ORDER BY row LIMIT {}", EXACT_CAP + 1), &[&value], EXACT_CAP);
    }
    if crate::query::is_numeric_op(op) {
        // Bounds typed as dates compare instants; otherwise numbers. An index answers only
        // when the filter's bounds were understood, exactly as the scan would.
        let (lo, hi) = filter.numeric_bounds();
        let key = if filter.is_time() { "ms" } else { "num" };
        let (clause, args): (String, Vec<f64>) = match (op, lo, hi) {
            ("gt", Some(b), _) => (format!("{key} > ?1"), vec![b]),
            ("gte", Some(b), _) => (format!("{key} >= ?1"), vec![b]),
            ("lt", Some(b), _) => (format!("{key} < ?1"), vec![b]),
            ("lte", Some(b), _) => (format!("{key} <= ?1"), vec![b]),
            ("between", Some(a), Some(b)) => (format!("{key} >= ?1 AND {key} <= ?2"), vec![a, b]),
            _ => return Ok(None),
        };
        let Some(db) = open(idx, column, Kind::Range)? else { return Ok(None) };
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a as &dyn rusqlite::ToSql).collect();
        return rows_of(&db, &format!("SELECT row FROM entries WHERE {clause} ORDER BY row LIMIT {}", WIDE_CAP + 1), &params, WIDE_CAP);
    }
    if op == "term" {
        if filter.terms().is_empty() { return Ok(None); }
        let Some(db) = open(idx, column, Kind::Terms)? else { return Ok(None) };
        // Every word is required: intersect the lists that are small enough to read. Skipping
        // a dense word only widens the superset.
        let mut result: Option<Vec<usize>> = None;
        for term in filter.terms() {
            let Some(rows) = rows_of(&db, &format!("SELECT row FROM entries WHERE term=?1 ORDER BY row LIMIT {}", WIDE_CAP + 1), &[term], WIDE_CAP)? else { continue };
            result = Some(match result {
                None => rows,
                Some(previous) => {
                    let keep: std::collections::HashSet<usize> = rows.into_iter().collect();
                    previous.into_iter().filter(|r| keep.contains(r)).collect()
                }
            });
        }
        return Ok(result);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Environment(Option<std::ffi::OsString>);
    impl Drop for Environment { fn drop(&mut self) { if let Some(value) = &self.0 { std::env::set_var("LOGINSIGHT_DATA_DIR", value); } else { std::env::remove_var("LOGINSIGHT_DATA_DIR"); } } }

    #[test]
    fn words_are_lowercase_alphanumeric_runs() {
        assert_eq!(tokens("Falha-de LOGIN: usuário_ana, ação=2").collect::<Vec<_>>(), ["falha", "de", "login", "usuário", "ana", "ação", "2"]);
        assert_eq!(tokens(&"x".repeat(129)).count(), 0);
        assert_eq!(tokens("").count(), 0);
    }

    #[test]
    fn indexes_are_complete_case_owned_and_match_exact_and_ascii_equality() {
        let dir = tempfile::tempdir().unwrap();
        let _environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path());
        crate::case_store::save(serde_json::json!({"cases":[{"id":"index-a"},{"id":"index-b"}],"active":null})).unwrap();
        let file = dir.path().join("original.jsonl");
        std::fs::write(&file, "{\"token\":\"Alpha\"}\n{\"token\":\"alpha\"}\n{\"token\":\"É\"}\n{}\n").unwrap();
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        for (case, create, expected) in [("index-a", true, 1), ("index-b", false, 0), ("index-a", false, 1)] {
            let identity = crate::analysis_context::snapshot(case).unwrap().identity();
            let admitted = crate::analysis_runtime::capture(&state, Some(identity.clone()), None, crate::analysis_runtime::Mode::Publish).unwrap();
            admitted.prepare_visibility(None).unwrap();
            crate::analysis_runtime::with(Some(admitted), || crate::load_files_impl(&state, &[file.to_string_lossy().into_owned()], "jsonl", Some(false), None)).unwrap();
            let admitted = crate::analysis_runtime::capture(&state, Some(identity), None, crate::analysis_runtime::Mode::Dataset).unwrap();
            admitted.prepare_visibility(None).unwrap();
            crate::analysis_runtime::with(Some(admitted), || {
                let source = crate::analysis_runtime::source(&state);
                let crate::SourceData::Indexed(idx) = &*source else { panic!("source required") };
                if create { assert_eq!(build(idx, "token", Kind::Exact, &state, None).unwrap().fields, vec!["token"]); }
                assert_eq!(status(idx).unwrap().fields.len(), expected, "{case}, creating={create}");
                if expected == 0 { return; }
                for (op, value, ids) in [("equals_exact", "Alpha", vec![0]), ("equals", " ALPHA ", vec![0, 1]), ("equals", "é", vec![])] {
                    let filters = crate::query::prepare(&[crate::query::Filter { column: "token".into(), op: op.into(), value: value.into(), value2: None }]);
                    assert_eq!(candidates(idx, &filters[0]).unwrap(), Some(ids));
                }
            });
        }
    }

    /// For every filter, the selected rows are the same with and without the index; with the
    /// index the candidate set is a superset that the canonical predicate then narrows.
    #[test]
    fn range_and_terms_indexes_never_change_which_rows_a_query_selects() {
        let dir = tempfile::tempdir().unwrap();
        let _environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path());
        crate::case_store::save(serde_json::json!({"cases":[{"id":"index-r"}],"active":null})).unwrap();
        let file = dir.path().join("original.jsonl");
        let mut text = String::new();
        for i in 0..300 {
            let words = ["falha de login", "Acesso NEGADO ao arquivo", "conexão aberta", "tempo esgotado"][i % 4];
            text.push_str(&format!("{{\"bytes\":{},\"ratio\":{}.5,\"when\":\"2026-10-{:02}T12:00:00Z\",\"note\":\"{words} {i}\"}}\n", i * 7 % 101, i % 13, 1 + i % 28));
        }
        text.push_str("{\"bytes\":\"abc\",\"note\":\"\"}\n{}\n");
        std::fs::write(&file, text).unwrap();
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        let identity = crate::analysis_context::snapshot("index-r").unwrap().identity();
        let admitted = crate::analysis_runtime::capture(&state, Some(identity.clone()), None, crate::analysis_runtime::Mode::Publish).unwrap();
        admitted.prepare_visibility(None).unwrap();
        crate::analysis_runtime::with(Some(admitted), || crate::load_files_impl(&state, &[file.to_string_lossy().into_owned()], "jsonl", Some(false), None)).unwrap();
        let admitted = crate::analysis_runtime::capture(&state, Some(identity), None, crate::analysis_runtime::Mode::Dataset).unwrap();
        admitted.prepare_visibility(None).unwrap();
        crate::analysis_runtime::with(Some(admitted), || {
            let filter = |column: &str, op: &str, value: &str, value2: Option<&str>| crate::query::Filter { column: column.into(), op: op.into(), value: value.into(), value2: value2.map(str::to_owned) };
            let cases = vec![
                filter("bytes", "gt", "90", None), filter("bytes", "gte", "50", None), filter("bytes", "lt", "3", None),
                filter("bytes", "lte", "0", None), filter("bytes", "between", "10", Some("12")), filter("ratio", "gt", "11.4", None),
                filter("when", "gte", "2026-10-27", None), filter("when", "between", "2026-10-02", Some("2026-10-03T23:59:59Z")),
                filter("note", "term", "negado", None), filter("note", "term", "ACESSO arquivo", None), filter("note", "term", "login 4", None),
                filter("note", "term", "inexistente", None), filter("note", "term", "falha", None),
            ];
            let ids = |f: &crate::query::Filter| -> Vec<usize> {
                crate::workspace::with_selection(&state, std::slice::from_ref(f), |s| s.iter().map(|e| e.id).collect::<Vec<_>>()).unwrap()
            };
            let before: Vec<Vec<usize>> = cases.iter().map(ids).collect();
            assert!(before[8].len() == 75 && before[4].iter().all(|id| *id < 300), "term and range results exist before indexing: {before:?}");
            let source = crate::analysis_runtime::source(&state);
            let crate::SourceData::Indexed(idx) = &*source else { panic!("source required") };
            for (column, kind) in [("bytes", Kind::Range), ("ratio", Kind::Range), ("when", Kind::Range), ("note", Kind::Terms)] {
                build(idx, column, kind, &state, None).unwrap();
            }
            let status = status(idx).unwrap();
            assert_eq!(status.indexes.len(), 4);
            assert!(status.indexes.iter().all(|i| i.bytes > 0));
            assert!(status.fields.is_empty(), "the exact list stays exact-only");
            let mut used = 0;
            for (case, expected) in cases.iter().zip(&before) {
                let prepared = crate::query::prepare(std::slice::from_ref(case));
                if let Some(candidates) = candidates(idx, &prepared[0]).unwrap() {
                    used += 1;
                    assert!(expected.iter().all(|id| candidates.contains(id)), "candidates are a superset for {case:?}");
                }
                assert_eq!(&ids(case), expected, "{case:?} selects the same rows with the index");
            }
            assert!(used >= 8, "the indexes served the filters they cover ({used})");
            // Removing an index returns to the scan with identical answers.
            remove(idx, "note", Kind::Terms).unwrap();
            assert_eq!(status_len(idx), 3);
            assert_eq!(&ids(&cases[8]), &before[8]);
        });
    }
    fn status_len(idx: &FileIndex) -> usize { status(idx).unwrap().indexes.len() }
}
