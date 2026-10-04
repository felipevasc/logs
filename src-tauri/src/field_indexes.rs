//! Optional, immutable field-value indexes. Ownership, interpretation and source
//! order are in the key; only complete indexes can narrow canonical queries.
use crate::{sources::FileIndex, query::PreparedFilter};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status { pub fields: Vec<String>, pub rows: usize }

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
fn path(idx: &FileIndex, column: &str) -> Result<PathBuf, String> {
    Ok(directory(idx)?.join(format!("{:x}.sqlite3", Sha256::digest(column.as_bytes()))))
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
    let dir = directory(idx)?;
    if dir.exists() {
        for file in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            crate::operations::check()?;
            let file = file.map_err(|e| e.to_string())?;
            if file.path().extension().is_none_or(|e| e != "sqlite3") { continue; }
            let Ok(db) = reader(&file.path()) else { continue };
            if let Ok(field) = db.query_row("SELECT field FROM complete WHERE rows=?1", [idx.lines.len()], |r| r.get::<_, String>(0)) {
                fields.push(field);
            }
        }
    }
    fields.sort();
    Ok(Status { fields, rows: idx.lines.len() })
}
pub(crate) fn build(idx: &FileIndex, column: &str, state: &crate::AppState, app: Option<&tauri::AppHandle>) -> Result<Status, String> {
    if column.is_empty() || column.len() > 1024 || column.chars().any(char::is_control) || column == "_all" {
        return Err("Escolha um campo válido para indexar.".into());
    }
    let target = path(idx, column)?;
    if target.exists() && reader(&target).is_ok_and(|db| complete(&db, column, idx.lines.len())) { return status(idx); }
    let dir = target.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let temporary = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    let mut db = Connection::open(temporary.path()).map_err(|e| e.to_string())?;
    let token = crate::operations::current_token();
    db.progress_handler(2000, Some(move || token.cancelled()));
    db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;
        CREATE TABLE entries(value TEXT NOT NULL, folded TEXT NOT NULL, row INTEGER NOT NULL);
        CREATE TABLE complete(field TEXT NOT NULL, rows INTEGER NOT NULL);").map_err(|e| e.to_string())?;
    let codes = crate::analysis_runtime::codes(state);
    let system = crate::analysis_runtime::system_codes(state);
    let derived = crate::analysis_runtime::derived(state);
    let disk_limit = crate::resources::spill_bytes(true, dir)?;
    let mut written = 0u64;
    let transaction = db.transaction().map_err(|e| e.to_string())?;
    {
        let mut insert = transaction.prepare("INSERT INTO entries VALUES(?1,?2,?3)").map_err(|e| e.to_string())?;
        for row in 0..idx.lines.len() {
            crate::operations::check()?;
            let event = crate::sources::event_at(idx, row, &codes, &system, &derived);
            if let Some(value) = event.col_ref(column) {
                written = written.saturating_add((value.len() as u64).saturating_mul(4).saturating_add(128));
                if value.len() > crate::resources::batch_bytes() || written > disk_limit {
                    return Err("O índice excede o orçamento de recursos. Escolha um campo menor ou ajuste os recursos.".into());
                }
                insert.execute(params![value.as_ref(), value.to_ascii_lowercase(), row]).map_err(|e| e.to_string())?;
            }
            if row % 2048 == 0 {
                crate::emit_progress(app, "índice", "Indexando valores do campo", row, idx.lines.len(), "registros", true);
            }
        }
    }
    transaction.execute_batch("CREATE INDEX exact_values ON entries(value,row); CREATE INDEX folded_values ON entries(folded,row);").map_err(|e| e.to_string())?;
    crate::operations::check()?;
    crate::analysis_runtime::validate_result_owner()?;
    transaction.execute("INSERT INTO complete VALUES(?1,?2)", params![column, idx.lines.len()]).map_err(|e| e.to_string())?;
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
pub(crate) fn candidates(idx: &FileIndex, filter: &PreparedFilter) -> Result<Option<Vec<usize>>, String> {
    let (key, value) = match filter.f.op.as_str() {
        "equals_exact" => ("value", filter.f.value.clone()),
        "equals" => ("folded", filter.f.value.trim().to_ascii_lowercase()),
        _ => return Ok(None),
    };
    let path = path(idx, &filter.f.column)?;
    if !path.exists() { return Ok(None); }
    let Ok(db) = reader(&path) else { return Ok(None) };
    if !complete(&db, &filter.f.column, idx.lines.len()) { return Ok(None); }
    let mut statement = db.prepare(&format!("SELECT row FROM entries WHERE {key}=?1 ORDER BY row LIMIT 4097")).map_err(|e| e.to_string())?;
    let mut rows = statement.query([value]).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        found.push(row.get(0).map_err(|e| e.to_string())?);
    }
    // Dense values keep the vectorized engine path, never truncate a result.
    Ok((found.len() <= 4096).then_some(found))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexes_are_complete_case_owned_and_match_exact_and_ascii_equality() {
        struct Environment(Option<std::ffi::OsString>);
        impl Drop for Environment { fn drop(&mut self) { if let Some(value) = &self.0 { std::env::set_var("LOGINSIGHT_DATA_DIR", value); } else { std::env::remove_var("LOGINSIGHT_DATA_DIR"); } } }
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
                if create { assert_eq!(build(idx, "token", &state, None).unwrap().fields, vec!["token"]); }
                assert_eq!(status(idx).unwrap().fields.len(), expected, "{case}, creating={create}");
                if expected == 0 { return; }
                for (op, value, ids) in [("equals_exact", "Alpha", vec![0]), ("equals", " ALPHA ", vec![0, 1]), ("equals", "é", vec![])] {
                    let filters = crate::query::prepare(&[crate::query::Filter { column: "token".into(), op: op.into(), value: value.into(), value2: None }]);
                    assert_eq!(candidates(idx, &filters[0]).unwrap(), Some(ids));
                }
            });
        }
    }
}
