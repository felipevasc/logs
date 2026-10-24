//! Console admission reuses the ready stores, but grants only a scoped events
//! view. User SQL cannot name the attached stores or invoke table functions.
use super::{Connection, Session};
use crate::{query::Filter, AppState};
use std::sync::Arc;
pub(crate) struct Prepared {
    pub conn: Connection,
    pub session: Arc<Session>,
    pub raw: super::udf::Tests,
}
fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
pub(crate) fn prepare(state: &AppState, filters: &[Filter]) -> Result<Option<Prepared>, String> {
    crate::workspace::with_engine(state, |src| {
        let Some(session) = super::session_checked(src.idx, src.codes, src.system, src.derived)?
        else {
            return Ok(None);
        };
        let conn = session.base.lock().try_clone().map_err(|e| e.to_string())?;
        let _watch = super::ops::interruptible(&conn)?;
        let restricted =
            !filters.is_empty() || !crate::analysis_runtime::visibility_unrestricted(src.idx)?;
        if restricted {
            conn.execute_batch("CREATE TEMP TABLE console_scope(id BIGINT PRIMARY KEY)")
                .map_err(|e| e.to_string())?;
            let mut append = conn.appender("console_scope").map_err(|e| e.to_string())?;
            crate::workspace::with_selection(state, filters, |selection| {
                let result = selection.visit_exact_ids(|id| {
                    crate::operations::check()?;
                    append.append_row([id as i64]).map_err(|e| e.to_string())?;
                    Ok(true)
                })?;
                if result.is_none() {
                    for event in selection.iter() {
                        crate::operations::check()?;
                        append
                            .append_row([event.id as i64])
                            .map_err(|e| e.to_string())?;
                    }
                }
                Ok::<(), String>(())
            })??;
            append.flush().map_err(|e| e.to_string())?;
            drop(append);
        }
        // Selection may itself acquire a names guard. Do not recursively hold
        // a read guard while another Case is waiting to refresh the catalog.
        let _names = session.names_guard(src.codes, src.system)?;
        let mut fields: Vec<_> = session.schema.fields.iter().collect();
        fields.sort_by_key(|(name, _)| *name);
        let mut raw = super::udf::Tests::default();
        let index = crate::analysis_runtime::clone_index(src.idx);
        let gate = crate::analysis_runtime::indexed_gate(src.idx)?;
        let token = crate::operations::current_token();
        let raw_expression=raw.raw("id",Arc::new(move|id|{
            if token.cancelled(){return Err("Operação cancelada.".into());}
            let id=usize::try_from(id).map_err(|_|"Identidade inválida no texto bruto.")?;
            if id>=index.lines.len(){return Err("Identidade fora da fonte admitida.".into());}
            if gate.as_ref().is_some_and(|gate|!gate.allows_known_row(id)){return Ok(None);}
            if index.lines.at(id).len as usize>crate::search_regex::SUBJECT_LIMIT{return Err("O registro bruto excede 8 MiB; use Texto direto para consultar suas janelas.".into());}
            let part=index.part_at(id);crate::sources::validate_source(part)?;
            Ok(Some(crate::sources::parse_part_line(part,crate::sources::line_bytes(&index,id)).raw))
        }));
        let mut projection = vec![
            "id".to_owned(),
            "event_ref".into(),
            "ts AS timestamp".into(),
            "source".into(),
            "level".into(),
            "code".into(),
            "name".into(),
            "description".into(),
            "message".into(),
            format!("{raw_expression} AS raw"),
        ];
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::from_iter(
            [
                "id",
                "event_ref",
                "timestamp",
                "source",
                "level",
                "code",
                "name",
                "description",
                "message",
                "raw",
            ]
            .map(str::to_owned),
        );
        for (name, expression) in fields {
            if used.insert(name.to_lowercase()) {
                projection.push(format!("{expression} AS {}", ident(name)));
            }
        }
        let source = if restricted {
            "evn WHERE id IN (SELECT id FROM console_scope)"
        } else {
            "evn"
        };
        conn.execute_batch(&format!(
            "CREATE TEMP VIEW events AS SELECT {} FROM {source}",
            projection.join(",")
        ))
        .map_err(|e| e.to_string())?;
        drop(_watch);
        drop(_names);
        Ok(Some(Prepared { conn, session, raw }))
    })
}
pub(crate) fn materialize(prepared: &Prepared, sql: &str) -> Result<(), String> {
    let _watch = super::ops::interruptible(&prepared.conn)?;
    // Hold the catalog revision used to resolve names until the statement has
    // completed. A concurrent Case must not swap its name/description join.
    let current = crate::analysis_runtime::current();
    let catalogs = current
        .as_ref()
        .map(|a| (&a.interpretation.codes, &a.interpretation.system_codes));
    let _names = if let Some((codes, system)) = catalogs {
        Some(prepared.session.names_guard(codes, system)?)
    } else {
        None
    };
    prepared
        .conn
        .execute_batch(&format!("CREATE TEMP TABLE console_result AS {sql}"))
        .map_err(|e| e.to_string())?;
    crate::operations::check()
}
pub(crate) fn forbidden(conn: &Connection) -> Result<std::collections::HashSet<String>, String> {
    let mut names = std::collections::HashSet::from_iter(
        [
            "events",
            "ev",
            "evn",
            "enr",
            "console_scope",
            "console_result",
        ]
        .map(str::to_owned),
    );
    let mut statement = conn
        .prepare(
            "SELECT table_name FROM duckdb_tables() UNION ALL SELECT view_name FROM duckdb_views()",
        )
        .map_err(|e| e.to_string())?;
    for name in statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
    {
        names.insert(name.map_err(|e| e.to_string())?.to_lowercase());
    }
    Ok(names)
}
