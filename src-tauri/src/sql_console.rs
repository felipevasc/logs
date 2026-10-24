use crate::{query::Filter, text_reader::Owner, AppState};
use duckdb::Connection;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sqlparser::{
    ast::{Expr, ObjectName, Query, Select, Statement, TableFactor, Visit, Visitor},
    dialect::DuckDbDialect,
    parser::Parser,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io::Write,
    ops::ControlFlow,
    sync::{Arc, LazyLock},
    time::Instant,
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
const FUNCTIONS:&str="count sum avg min max first last any_value median mode quantile quantile_cont quantile_disc stddev stddev_pop stddev_samp variance var_pop var_samp approx_count_distinct string_agg list array_agg bool_and bool_or arg_min arg_max histogram count_if row_number rank dense_rank percent_rank cume_dist ntile lag lead first_value last_value nth_value abs ceil ceiling floor round trunc power sqrt ln log log10 exp sign greatest least coalesce if ifnull nullif nullifzero zeroifnull lower upper length char_length octet_length bit_length trim ltrim rtrim substring substr left right replace concat concat_ws starts_with ends_with contains position strpos split_part string_split str_split regexp_matches regexp_full_match regexp_extract regexp_extract_all regexp_replace regexp_split_to_array levenshtein md5 sha256 hash typeof json_valid json_extract json_extract_string json_value json_type json_array_length json_keys to_json to_timestamp epoch epoch_ms epoch_us make_timestamp date_trunc date_part date_diff datediff strftime strptime try_strptime day month year hour minute second cast try_cast count_star";
struct Guard {
    ctes: HashSet<String>,
    forbidden: HashSet<String>,
    functions: HashSet<&'static str>,
    nodes: usize,
}
impl Visitor for Guard {
    type Break = String;
    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<String> {
        if let Some(with) = &q.with {
            for cte in &with.cte_tables {
                let name = cte.alias.name.value.to_lowercase();
                if self.forbidden.contains(&name) {
                    return ControlFlow::Break(format!(
                        "O nome de CTE {name} é reservado. Escolha outro alias."
                    ));
                }
                self.ctes.insert(name);
            }
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_statement(&mut self, s: &Statement) -> ControlFlow<String> {
        if matches!(s, Statement::Query(_)) {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(
                "A console aceita somente uma consulta SELECT, inclusive WITH.".into(),
            )
        }
    }
    fn pre_visit_select(&mut self, s: &Select) -> ControlFlow<String> {
        if s.into.is_some() {
            ControlFlow::Break("SELECT INTO não é permitido.".into())
        } else {
            ControlFlow::Continue(())
        }
    }
    fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<String> {
        let name = name.to_string().trim_matches('"').to_lowercase();
        if name == "events" || self.ctes.contains(&name) {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(format!("A consulta só pode ler events e suas CTEs: {name}"))
        }
    }
    fn pre_visit_table_factor(&mut self, t: &TableFactor) -> ControlFlow<String> {
        match t {
            TableFactor::Table {
                args: None,
                version: None,
                with_hints,
                json_path: None,
                ..
            } if with_hints.is_empty() => ControlFlow::Continue(()),
            TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            _ => ControlFlow::Break(
                "Funções de tabela, arquivos externos e tabelas de sistema não são permitidos."
                    .into(),
            ),
        }
    }
    fn pre_visit_expr(&mut self, e: &Expr) -> ControlFlow<String> {
        self.nodes += 1;
        if self.nodes > 20_000 {
            return ControlFlow::Break("A consulta excede o limite de complexidade.".into());
        }
        if let Expr::Function(f) = e {
            let name = f.name.to_string().to_lowercase();
            if !self.functions.contains(name.as_str()) {
                return ControlFlow::Break(format!(
                    "Função fora da lista de consulta local: {name}"
                ));
            }
        }
        ControlFlow::Continue(())
    }
}
pub(crate) fn validate(sql: &str, forbidden: HashSet<String>) -> Result<String, String> {
    if sql.len() > 128 << 10 {
        return Err("A consulta excede 128 KiB.".into());
    }
    let mut statements = Parser::parse_sql(&DuckDbDialect {}, sql).map_err(err)?;
    if statements.len() != 1 || !matches!(statements[0], Statement::Query(_)) {
        return Err("Execute uma única consulta SELECT ou WITH.".into());
    }
    let statement = statements.remove(0);
    let mut guard = Guard {
        ctes: HashSet::new(),
        forbidden,
        functions: FUNCTIONS.split_whitespace().collect(),
        nodes: 0,
    };
    if let ControlFlow::Break(error) = statement.visit(&mut guard) {
        return Err(error);
    }
    Ok(statement.to_string())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Column {
    pub name: String,
    pub data_type: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    pub result: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total: String,
    pub offset: u64,
    pub complete: bool,
    pub filter_count: usize,
}
struct Snapshot {
    owner: Owner,
    conn: Mutex<Connection>,
    columns: Vec<Column>,
    total: u64,
    filter_count: usize,
    touched: Mutex<Instant>,
    _engine: Option<Arc<crate::engine::Session>>,
    _scratch: Option<tempfile::TempDir>,
}
static RESULTS: LazyLock<Mutex<HashMap<String, Arc<Snapshot>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
fn columns(conn: &Connection, table: &str) -> Result<Vec<Column>, String> {
    let mut stmt = conn
        .prepare(&format!("DESCRIBE SELECT * FROM {}", ident(table)))
        .map_err(err)?;
    stmt.query_map([], |row| {
        Ok(Column {
            name: row.get(0)?,
            data_type: row.get(1)?,
        })
    })
    .map_err(err)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(err)
}
fn fallback(
    state: &AppState,
    filters: &[Filter],
) -> Result<(Connection, tempfile::TempDir), String> {
    let scratch = tempfile::tempdir().map_err(err)?;
    let conn = Connection::open_in_memory().map_err(err)?;
    conn.execute_batch(&format!("SET temp_directory={}; SET memory_limit='{}MB'; SET max_temp_directory_size='4GB'; SET threads={};",crate::engine::sql::lit(&scratch.path().to_string_lossy()),crate::resources::duckdb_memory_mb().min(64),crate::resources::query_threads().min(2))).map_err(err)?;
    let fields = crate::workspace::with_selection(state, filters, |selection| {
        let mut fields = BTreeSet::new();
        for event in selection.iter() {
            crate::operations::check()?;
            fields.extend(event.fields.keys().cloned());
            if fields.len() > 2048 {
                return Err("Mais de 2048 campos: reduza o recorte antes de consultar.".to_string());
            }
        }
        Ok::<_, String>(fields)
    })??;
    let mut names = vec![
        "id".to_owned(),
        "event_ref".into(),
        "timestamp".into(),
        "source".into(),
        "level".into(),
        "code".into(),
        "name".into(),
        "description".into(),
        "message".into(),
        "raw".into(),
    ];
    for field in fields {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&field)) {
            names.push(field);
        }
    }
    conn.execute_batch(&format!(
        "CREATE TABLE events({})",
        names
            .iter()
            .enumerate()
            .map(|(i, n)| format!(
                "{} {}",
                ident(n),
                if i == 0 || i == 2 {
                    "BIGINT"
                } else {
                    "VARCHAR"
                }
            ))
            .collect::<Vec<_>>()
            .join(",")
    ))
    .map_err(err)?;
    let mut appender = conn.appender("events").map_err(err)?;
    let mut bytes = 0usize;
    crate::workspace::with_selection(state, filters, |selection| {
        for event in selection.iter() {
            crate::operations::check()?;
            let values: Vec<duckdb::types::Value> = names
                .iter()
                .enumerate()
                .map(|(i, name)| match i {
                    0 => duckdb::types::Value::BigInt(event.id as i64),
                    2 => event
                        .timestamp
                        .map(duckdb::types::Value::BigInt)
                        .unwrap_or(duckdb::types::Value::Null),
                    _ => event
                        .col_ref(name)
                        .map(|v| duckdb::types::Value::Text(v.into_owned()))
                        .unwrap_or(duckdb::types::Value::Null),
                })
                .collect();
            bytes += values
                .iter()
                .map(|v| match v {
                    duckdb::types::Value::Text(s) => s.len(),
                    _ => 8,
                })
                .sum::<usize>();
            if bytes > 4usize << 30 {
                return Err("O snapshot SQL excede 4 GiB; reduza o recorte.".into());
            }
            appender
                .append_row(duckdb::appender_params_from_iter(values.iter()))
                .map_err(err)?;
        }
        Ok::<_, String>(())
    })??;
    appender.flush().map_err(err)?;
    drop(appender);
    Ok((conn, scratch))
}
pub(crate) fn schema(state: &AppState, filters: &[Filter]) -> Result<Vec<Column>, String> {
    if let Some(prepared) = crate::engine::console::prepare(state, filters)? {
        columns(&prepared.conn, "events")
    } else {
        let (conn, _scratch) = fallback(state, filters)?;
        columns(&conn, "events")
    }
}
pub(crate) fn execute(state: &AppState, filters: &[Filter], sql: &str) -> Result<Page, String> {
    crate::workspace::validate(filters)?;
    let owner = Owner::current()?;
    let prepared = crate::engine::console::prepare(state, filters)?;
    let (conn, engine, scratch) = if let Some(prepared) = prepared {
        let sql = validate(sql, crate::engine::console::forbidden(&prepared.conn)?)?;
        crate::engine::console::materialize(&prepared, &sql)?;
        // TEMP objects are connection-local; retain and use the original connection.
        let crate::engine::console::Prepared {
            conn: original,
            session,
            raw,
        } = prepared;
        drop(raw);
        (original, Some(session), None)
    } else {
        let (conn, scratch) = fallback(state, filters)?;
        let sql = validate(
            sql,
            HashSet::from_iter(["events", "console_result"].map(str::to_owned)),
        )?;
        let _watch = crate::engine::interruptible(&conn)?;
        conn.execute_batch(&format!("CREATE TEMP TABLE console_result AS {sql}"))
            .map_err(err)?;
        drop(_watch);
        (conn, None, Some(scratch))
    };
    let columns = columns(&conn, "console_result")?;
    if columns.len() > 256 {
        return Err("O resultado excede 256 colunas. Selecione os campos necessários.".into());
    }
    let total = conn
        .query_row("SELECT count(*) FROM console_result", [], |r| {
            r.get::<_, u64>(0)
        })
        .map_err(err)?;
    owner.validate()?;
    let id = uuid::Uuid::new_v4().to_string();
    let snapshot = Arc::new(Snapshot {
        owner,
        conn: Mutex::new(conn),
        columns,
        total,
        filter_count: filters.len(),
        touched: Mutex::new(Instant::now()),
        _engine: engine,
        _scratch: scratch,
    });
    {
        let mut results = RESULTS.lock();
        results.retain(|_, s| s.touched.lock().elapsed().as_secs() < 1800);
        if results.len() >= 6 {
            if let Some(old) = results
                .iter()
                .min_by_key(|(_, s)| *s.touched.lock())
                .map(|(id, _)| id.clone())
            {
                results.remove(&old);
            }
        }
        results.insert(id.clone(), snapshot);
    }
    page(&id, 0, 80)
}
fn get(id: &str) -> Result<Arc<Snapshot>, String> {
    let result = RESULTS
        .lock()
        .get(id)
        .cloned()
        .ok_or("Resultado SQL expirado. Execute novamente.")?;
    result.owner.validate()?;
    *result.touched.lock() = Instant::now();
    Ok(result)
}
fn select(columns: &[Column]) -> String {
    columns
        .iter()
        .map(|c| format!("CAST({} AS VARCHAR)", ident(&c.name)))
        .collect::<Vec<_>>()
        .join(",")
}
pub(crate) fn page(id: &str, offset: u64, limit: usize) -> Result<Page, String> {
    let result = get(id)?;
    let conn = result.conn.lock();
    let _watch = crate::engine::interruptible(&conn)?;
    let sql = format!(
        "SELECT {} FROM console_result ORDER BY rowid LIMIT {} OFFSET {}",
        select(&result.columns),
        limit.clamp(1, 200),
        offset
    );
    let mut statement = conn.prepare(&sql).map_err(err)?;
    let mut rows = statement.query([]).map_err(err)?;
    let mut output = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next().map_err(err)? {
        crate::operations::check()?;
        let mut cells = Vec::new();
        for i in 0..result.columns.len() {
            let value: Option<String> = row.get(i).map_err(err)?;
            bytes += value.as_ref().map_or(4, String::len);
            if bytes > 2 << 20 {
                return Err("A página SQL excede 2 MiB. Use substring nos campos extensos.".into());
            }
            cells.push(value);
        }
        output.push(cells);
    }
    result.owner.validate()?;
    Ok(Page {
        result: id.into(),
        columns: result.columns.clone(),
        rows: output,
        total: result.total.to_string(),
        offset,
        complete: true,
        filter_count: result.filter_count,
    })
}
pub(crate) fn export(id: &str, path: &str, format: &str) -> Result<u64, String> {
    if !["csv", "jsonl"].contains(&format) {
        return Err("Use CSV ou JSONL.".into());
    }
    let result = get(id)?;
    let destination = std::path::Path::new(path);
    let parent = destination.parent().ok_or("Destino inválido.")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(err)?;
    let conn = result.conn.lock();
    let _watch = crate::engine::interruptible(&conn)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM console_result ORDER BY rowid",
            select(&result.columns)
        ))
        .map_err(err)?;
    let mut rows = stmt.query([]).map_err(err)?;
    fn csv(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
    if format == "csv" {
        writeln!(
            file,
            "{}",
            result
                .columns
                .iter()
                .map(|c| csv(&c.name))
                .collect::<Vec<_>>()
                .join(",")
        )
        .map_err(err)?;
    }
    let mut n = 0;
    while let Some(row) = rows.next().map_err(err)? {
        crate::operations::check()?;
        let cells = (0..result.columns.len())
            .map(|i| row.get::<_, Option<String>>(i))
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        if format == "csv" {
            writeln!(
                file,
                "{}",
                cells
                    .iter()
                    .map(|s| s.as_ref().map(|s| csv(s)).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .map_err(err)?;
        } else {
            let values: serde_json::Map<String, serde_json::Value> = result
                .columns
                .iter()
                .zip(cells)
                .map(|(c, value)| {
                    (
                        c.name.clone(),
                        value
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            serde_json::to_writer(
                &mut file,
                &serde_json::json!({"values":values,"types":result.columns}),
            )
            .map_err(err)?;
            file.write_all(b"\n").map_err(err)?;
        }
        n += 1;
    }
    result.owner.validate()?;
    crate::operations::check()?;
    file.as_file().sync_all().map_err(err)?;
    file.persist(destination).map_err(err)?;
    crate::operations::commit();
    Ok(n)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn check(s: &str) -> Result<String, String> {
        validate(
            s,
            HashSet::from_iter(["evn", "ev", "events"].map(str::to_owned)),
        )
    }
    #[test]
    fn accepts_ctes_aggregates_windows_and_quoted_fields() {
        assert!(check("WITH counts AS (SELECT level, count(*) AS n FROM events GROUP BY level) SELECT *, rank() OVER (ORDER BY n DESC) FROM counts").is_ok());
        assert!(check("SELECT try_cast(\"process.pid\" AS BIGINT), event_ref FROM events").is_ok());
    }
    #[test]
    fn rejects_external_access_including_nested_and_cte_shadowing() {
        for sql in [
            "SELECT * FROM read_csv('secret')",
            "SELECT read_text('secret')",
            "SELECT li_raw(id,1) FROM events",
            "COPY events TO 'secret'",
            "SELECT * FROM evn",
            "WITH evn AS(SELECT * FROM evn) SELECT * FROM evn",
            "SELECT * FROM events; DELETE FROM events",
            "SELECT (SELECT * FROM duckdb_settings()) FROM events",
            "SELECT query('SELECT * FROM evn')",
            "INSTALL httpfs",
            "SELECT * INTO other FROM events",
        ] {
            assert!(check(sql).is_err(), "{sql}");
        }
    }
}
