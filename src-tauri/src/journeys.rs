//! Exact identifier grouping. Temporary SQLite tables keep cardinality and
//! paging independent of RAM; equal identifiers describe a link, not causality.
use crate::{
    insights,
    model::{CodesConfig, Event},
    operations,
    query::{self, Filter, PreparedFilter},
    sources::{self, CompiledDerived, FileIndex},
    AppState, SourceData,
};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use tauri::Manager;

const FIELD_CAP: usize = 128;
const DISTINCT_CAP: usize = 512;
const PROFILE_BYTES: usize = 8 * 1024 * 1024;
const KEY_BYTES: usize = 4096;

#[derive(Serialize)]
pub struct JourneyField {
    pub field: String,
    pub label: String,
    pub kind: String,
    pub suggested: bool,
    pub coverage: f64,
    pub distinct: Option<usize>,
    pub distinct_limited: bool,
    pub sampled: bool,
    pub fields_limited: bool,
}
#[derive(Serialize)]
pub struct JourneyGroup {
    pub value: String,
    pub count: usize,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub duration_ms: Option<i64>,
    pub sources: Vec<String>,
    pub sources_limited: bool,
    pub errors: usize,
    pub warnings: usize,
    pub missing_time: usize,
}
#[derive(Serialize)]
pub struct JourneyIndex {
    pub total: usize,
    pub groups: Vec<JourneyGroup>,
    pub complete: bool,
    pub field: String,
    pub missing_key: usize,
    pub missing_time: usize,
    pub skipped_keys: usize,
}
#[derive(Serialize)]
pub struct JourneyEvents {
    pub total: usize,
    pub rows: Vec<Event>,
    pub complete: bool,
    pub field: String,
    pub value: String,
    pub missing_time: usize,
    pub rows_clipped: usize,
}

fn normalized(field: &str) -> String {
    field
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn kind(field: &str) -> &'static str {
    let f = normalized(field);
    if f.ends_with("traceid") {
        "trace"
    } else if f.ends_with("requestid") || f == "reqid" || f == "xrequestid" {
        "request"
    } else if [
        "correlationid",
        "transactionid",
        "activityid",
        "operationid",
    ]
    .iter()
    .any(|s| f.ends_with(s))
    {
        "correlation"
    } else if f.ends_with("sessionid") || matches!(f.as_str(), "idsessao" | "sessaoid") {
        "session"
    } else if [
        "username",
        "userid",
        "userprincipalname",
        "accountname",
        "principalname",
    ]
    .iter()
    .any(|s| f.ends_with(s))
        || matches!(f.as_str(), "user" | "usuario" | "nomeusuario" | "idusuario")
    {
        "user"
    } else if [
        "sourceip",
        "destinationip",
        "clientip",
        "remoteip",
        "srcip",
        "dstip",
        "remoteaddr",
        "sourceaddress",
        "destinationaddress",
    ]
    .iter()
    .any(|s| f.ends_with(s))
        || matches!(
            f.as_str(),
            "ip" | "ipcliente" | "iporigem" | "ipdestino" | "enderecoip"
        )
    {
        "ip"
    } else {
        "custom"
    }
}
fn valid_field(field: &str) -> Result<(), String> {
    let f = normalized(field);
    if field.is_empty()
        || field.len() > 256
        || field.chars().any(char::is_control)
        || matches!(
            f.as_str(),
            "id" | "eventref" | "timestamp" | "raw" | "message" | "description"
        )
        || f.ends_with("eventid")
        || f.ends_with("recordid")
    {
        return Err("Escolha um campo de correlação; o identificador interno do registro e campos de texto/data não definem uma jornada.".into());
    }
    Ok(())
}
fn key(event: &Event, field: &str) -> Option<String> {
    if event
        .fields
        .get(field)
        .is_some_and(|v| v.is_null() || v.is_array() || v.is_object())
    {
        return None;
    }
    event.col_str(field).filter(|v| !v.trim().is_empty())
}

fn window_filters(
    field: &str,
    mut filters: Vec<Filter>,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<Vec<Filter>, String> {
    valid_field(field)?;
    if matches!(kind(field), "user" | "ip") && (from.is_none() || to.is_none()) {
        return Err("Investigações por usuário ou IP precisam de início e fim explícitos; use uma janela próxima ao evento.".into());
    }
    if from.zip(to).is_some_and(|(a, b)| a > b) {
        return Err("O início deve ser anterior ao fim.".into());
    }
    for (value, op) in [(from, "gte"), (to, "lte")] {
        if let Some(value) = value {
            filters.push(Filter {
                column: "timestamp".into(),
                op: op.into(),
                value: value.to_string(),
                value2: None,
            });
        }
    }
    Ok(filters)
}

enum Records<'a> {
    Memory(&'a [Event]),
    Indexed(&'a FileIndex),
    Empty,
}
struct View<'a> {
    records: Records<'a>,
    prepared: Vec<PreparedFilter>,
    codes: &'a CodesConfig,
    system: &'a CodesConfig,
    derived: &'a [CompiledDerived],
}
impl View<'_> {
    fn fetch(&self, index: usize) -> Event {
        match self.records {
            Records::Memory(events) => events[index].clone(),
            Records::Indexed(file) => {
                sources::event_at(file, index, self.codes, self.system, self.derived)
            }
            Records::Empty => unreachable!(),
        }
    }
    fn scan(
        &self,
        mut visit: impl FnMut(usize, &Event) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut failure = None;
        match self.records {
            Records::Memory(events) => {
                for (i, event) in events.iter().enumerate() {
                    if i % 2048 == 0 {
                        operations::check()?;
                    }
                    if self.prepared.iter().all(|f| query::matches(event, f)) {
                        visit(i, event)?;
                    }
                }
            }
            Records::Indexed(file) => query::visit_indexed_prepared(
                file,
                &self.prepared,
                self.codes,
                self.system,
                self.derived,
                |i| {
                    if failure.is_none() {
                        if let Err(error) = visit(i, &self.fetch(i)) {
                            failure = Some(error);
                        }
                    }
                },
            ),
            Records::Empty => {}
        }
        if let Some(error) = failure {
            return Err(error);
        }
        operations::check()
    }
}
fn with_view<T>(
    state: &AppState,
    filters: &[Filter],
    case: Option<&[Event]>,
    run: impl FnOnce(&View<'_>) -> Result<T, String>,
) -> Result<T, String> {
    crate::workspace::validate(filters)?;
    let source = state.source.read();
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    let records = if let Some(events) = case {
        Records::Memory(events)
    } else {
        match &*source {
            SourceData::Memory(events) => Records::Memory(events),
            SourceData::Indexed(index) => Records::Indexed(index),
            SourceData::None => Records::Empty,
        }
    };
    run(&View {
        records,
        prepared: query::prepare(filters),
        codes: &codes,
        system: &system,
        derived: &derived,
    })
}
fn db() -> Result<Connection, String> {
    let db = Connection::open("").map_err(|e| {
        format!("Não foi possível abrir armazenamento temporário das jornadas: {e}")
    })?;
    db.progress_handler(10_000, Some(operations::cancelled));
    db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-2048; PRAGMA temp_store=FILE; BEGIN;").map_err(|e| e.to_string())?;
    Ok(db)
}

#[derive(Default)]
struct FieldAcc {
    present: usize,
    values: HashSet<String>,
    limited: bool,
}
pub(crate) fn fields_impl(
    state: &AppState,
    filters: &[Filter],
    case: Option<&[Event]>,
) -> Result<Vec<JourneyField>, String> {
    with_view(state, filters, case, |view| {
        let mut fields = BTreeMap::<String, FieldAcc>::new();
        let (mut total, mut bytes, mut limited) = (0, 0usize, false);
        view.scan(|_, event| {
            total += 1;
            for (name, value) in &event.fields {
                if valid_field(name).is_err() || !value.is_string() && !value.is_number() {
                    continue;
                }
                let Some(value) = key(event, name) else {
                    continue;
                };
                if !fields.contains_key(name) && fields.len() >= FIELD_CAP {
                    limited = true;
                    continue;
                }
                let acc = fields.entry(name.clone()).or_default();
                acc.present += 1;
                if !acc.values.contains(&value) {
                    if acc.values.len() >= DISTINCT_CAP
                        || value.len() > 256
                        || bytes.saturating_add(value.len() + 64) > PROFILE_BYTES
                    {
                        acc.limited = true;
                    } else {
                        bytes += value.len() + 64;
                        acc.values.insert(value);
                    }
                }
            }
            Ok(())
        })?;
        let mut result: Vec<_> = fields
            .into_iter()
            .map(|(field, acc)| {
                let kind = kind(&field);
                JourneyField {
                    label: field.clone(),
                    field,
                    kind: kind.into(),
                    suggested: matches!(kind, "trace" | "request" | "correlation" | "session"),
                    coverage: acc.present as f64 / total.max(1) as f64,
                    distinct: (!acc.limited).then_some(acc.values.len()),
                    distinct_limited: acc.limited,
                    sampled: false,
                    fields_limited: limited,
                }
            })
            .collect();
        result.sort_by(|a, b| {
            b.suggested
                .cmp(&a.suggested)
                .then(b.coverage.total_cmp(&a.coverage))
                .then(a.field.cmp(&b.field))
        });
        Ok(result)
    })
}

pub(crate) fn index_impl(
    state: &AppState,
    filters: &[Filter],
    case: Option<&[Event]>,
    field: &str,
    offset: usize,
    limit: usize,
    sort: &str,
    singles: bool,
) -> Result<JourneyIndex, String> {
    valid_field(field)?;
    let order = match sort {
        "recent" => "e DESC,v",
        "duration" => "(e-b) DESC,n DESC,v",
        "count" => "n DESC,e DESC,v",
        "errors" => "errors DESC,n DESC,v",
        _ => return Err("Ordenação de jornadas inválida.".into()),
    };
    with_view(state, filters, case, |view| {
        let db = db()?;
        db.execute_batch("CREATE TABLE journeys(v TEXT PRIMARY KEY,n INTEGER,b INTEGER,e INTEGER,errors INTEGER,warnings INTEGER,missing INTEGER) WITHOUT ROWID; CREATE TABLE origins(v TEXT,s TEXT,n INTEGER,PRIMARY KEY(v,s)) WITHOUT ROWID;").map_err(|e|e.to_string())?;
        let (mut missing_key, mut missing_time, mut skipped_keys) = (0, 0, 0);
        view.scan(|_, event| {
            let Some(value) = key(event, field) else { missing_key += 1; return Ok(()); };
            if value.len() > KEY_BYTES { skipped_keys += 1; return Ok(()); }
            missing_time += usize::from(event.timestamp.is_none());
            db.prepare_cached("INSERT INTO journeys VALUES(?1,1,?2,?2,?3,?4,?5) ON CONFLICT(v) DO UPDATE SET n=n+1,b=CASE WHEN b IS NULL THEN excluded.b WHEN excluded.b IS NULL THEN b ELSE min(b,excluded.b) END,e=CASE WHEN e IS NULL THEN excluded.e WHEN excluded.e IS NULL THEN e ELSE max(e,excluded.e) END,errors=errors+excluded.errors,warnings=warnings+excluded.warnings,missing=missing+excluded.missing")
                .map_err(|e|e.to_string())?.execute(params![value,event.timestamp,insights::is_error(event) as i64,(event.level=="Aviso") as i64,event.timestamp.is_none() as i64]).map_err(|e|e.to_string())?;
            db.prepare_cached("INSERT INTO origins VALUES(?1,?2,1) ON CONFLICT(v,s) DO UPDATE SET n=n+1").map_err(|e|e.to_string())?.execute(params![value,event.source]).map_err(|e|e.to_string())?;
            Ok(())
        })?;
        let minimum = if singles { 1 } else { 2 };
        let total: usize = db
            .query_row(
                "SELECT count(*) FROM journeys WHERE n>=?1",
                [minimum],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())? as usize;
        let sql = format!("SELECT v,n,b,e,errors,warnings,missing FROM journeys WHERE n>=?1 ORDER BY {order} LIMIT ?2 OFFSET ?3");
        let mut select = db.prepare(&sql).map_err(|e| e.to_string())?;
        let mut groups = select
            .query_map(
                params![
                    minimum,
                    limit.clamp(1, 200) as i64,
                    offset.min(i64::MAX as usize) as i64
                ],
                |row| {
                    let start: Option<i64> = row.get(2)?;
                    let end: Option<i64> = row.get(3)?;
                    Ok(JourneyGroup {
                        value: row.get(0)?,
                        count: row.get::<_, i64>(1)? as usize,
                        start,
                        end,
                        duration_ms: start.zip(end).map(|(s, e)| e.saturating_sub(s)),
                        sources: vec![],
                        sources_limited: false,
                        errors: row.get::<_, i64>(4)? as usize,
                        warnings: row.get::<_, i64>(5)? as usize,
                        missing_time: row.get::<_, i64>(6)? as usize,
                    })
                },
            )
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for group in &mut groups {
            operations::check()?;
            let mut sources = db
                .prepare_cached("SELECT s FROM origins WHERE v=?1 ORDER BY n DESC,s LIMIT 13")
                .map_err(|e| e.to_string())?;
            group.sources = sources
                .query_map([&group.value], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            group.sources_limited = group.sources.len() > 12;
            group.sources.truncate(12);
        }
        operations::check()?;
        Ok(JourneyIndex {
            total,
            groups,
            complete: skipped_keys == 0,
            field: field.into(),
            missing_key,
            missing_time,
            skipped_keys,
        })
    })
}

pub(crate) fn events_impl(
    state: &AppState,
    filters: &[Filter],
    case: Option<&[Event]>,
    field: &str,
    value: &str,
    from: Option<i64>,
    to: Option<i64>,
    offset: usize,
    limit: usize,
) -> Result<JourneyEvents, String> {
    valid_field(field)?;
    if value.trim().is_empty() || value.len() > KEY_BYTES {
        return Err("Informe um identificador não vazio de até 4096 bytes.".into());
    }
    if matches!(kind(field), "user" | "ip") && (from.is_none() || to.is_none()) {
        return Err("Investigações por usuário ou IP precisam de início e fim explícitos; use uma janela próxima ao evento.".into());
    }
    if from.zip(to).is_some_and(|(a, b)| a > b) {
        return Err("O início deve ser anterior ao fim.".into());
    }
    with_view(state, filters, case, |view| {
        let db = db()?;
        db.execute_batch("CREATE TABLE records(i INTEGER PRIMARY KEY,t INTEGER); CREATE INDEX records_time ON records(t,i);").map_err(|e|e.to_string())?;
        let (mut total, mut missing_time) = (0, 0);
        view.scan(|i, event| {
            if key(event, field).as_deref() != Some(value) {
                return Ok(());
            }
            if from.is_some() || to.is_some() {
                let Some(t) = event.timestamp else {
                    return Ok(());
                };
                if from.is_some_and(|x| t < x) || to.is_some_and(|x| t > x) {
                    return Ok(());
                }
            }
            total += 1;
            missing_time += usize::from(event.timestamp.is_none());
            db.prepare_cached("INSERT INTO records VALUES(?1,?2)")
                .map_err(|e| e.to_string())?
                .execute(params![i as i64, event.timestamp])
                .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        let mut select = db
            .prepare("SELECT i FROM records ORDER BY t IS NULL,t,i LIMIT ?1 OFFSET ?2")
            .map_err(|e| e.to_string())?;
        let ids = select
            .query_map(
                params![
                    limit.clamp(1, 500) as i64,
                    offset.min(i64::MAX as usize) as i64
                ],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut rows_clipped = 0;
        let rows = ids
            .into_iter()
            .map(|i| {
                let (row, clipped) = crate::event_preview::preview(&view.fetch(i as usize));
                rows_clipped += usize::from(clipped);
                row
            })
            .collect();
        operations::check()?;
        Ok(JourneyEvents {
            total,
            rows,
            complete: true,
            field: field.into(),
            value: value.into(),
            missing_time,
            rows_clipped,
        })
    })
}

#[tauri::command]
pub async fn journey_fields(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    app: tauri::AppHandle,
) -> Result<Vec<JourneyField>, String> {
    let case_events = crate::case_cache::take(case_events, case_key)?;
    crate::offload(move || {
        fields_impl(
            app.state::<AppState>().inner(),
            &filters,
            case_events.as_deref(),
        )
    })
    .await?
}
#[tauri::command]
pub async fn journey_index(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    field: String,
    offset: Option<usize>,
    limit: Option<usize>,
    sort: Option<String>,
    include_singles: Option<bool>,
    from: Option<i64>,
    to: Option<i64>,
    app: tauri::AppHandle,
) -> Result<JourneyIndex, String> {
    let case_events = crate::case_cache::take(case_events, case_key)?;
    crate::offload(move || {
        let filters = window_filters(&field, filters, from, to)?;
        index_impl(
            app.state::<AppState>().inner(),
            &filters,
            case_events.as_deref(),
            &field,
            offset.unwrap_or(0),
            limit.unwrap_or(50),
            sort.as_deref().unwrap_or("recent"),
            include_singles.unwrap_or(false),
        )
    })
    .await?
}
#[tauri::command]
pub async fn journey_events(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    field: String,
    value: String,
    from: Option<i64>,
    to: Option<i64>,
    offset: Option<usize>,
    limit: Option<usize>,
    app: tauri::AppHandle,
) -> Result<JourneyEvents, String> {
    let case_events = crate::case_cache::take(case_events, case_key)?;
    crate::offload(move || {
        events_impl(
            app.state::<AppState>().inner(),
            &filters,
            case_events.as_deref(),
            &field,
            &value,
            from,
            to,
            offset.unwrap_or(0),
            limit.unwrap_or(100),
        )
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> AppState {
        AppState {
            source: parking_lot::RwLock::new(SourceData::None),
            source_names: parking_lot::RwLock::new(vec![]),
            derived: parking_lot::RwLock::new(vec![]),
            codes: parking_lot::RwLock::new(Default::default()),
            system_codes: parking_lot::RwLock::new(Default::default()),
            case_store_lock: parking_lot::Mutex::new(()),
            codes_path: Default::default(),
            system_codes_path: Default::default(),
        }
    }
    fn event(i: usize, value: &str, t: Option<i64>) -> Event {
        let mut e = Event::empty();
        e.id = i;
        e.timestamp = t;
        e.source = format!("source{}", i % 3);
        e.fields.insert("trace.id".into(), value.into());
        e
    }
    #[test]
    fn exact_keys_time_order_counts_and_undated() {
        let state = state();
        let mut rows = vec![
            event(0, "A", Some(20)),
            event(1, "A", None),
            event(2, "A", Some(0)),
            event(3, "a", Some(5)),
            event(4, " A", Some(9)),
            event(5, "B", Some(8)),
        ];
        rows[0].level = "Erro".into();
        let idx = index_impl(
            &state,
            &[],
            Some(&rows),
            "trace.id",
            0,
            50,
            "duration",
            false,
        )
        .unwrap();
        assert_eq!(idx.total, 1);
        let g = &idx.groups[0];
        assert_eq!(
            (
                g.value.as_str(),
                g.count,
                g.start,
                g.end,
                g.duration_ms,
                g.missing_time,
                g.errors
            ),
            ("A", 3, Some(0), Some(20), Some(20), 1, 1)
        );
        assert_eq!(g.sources.len(), 3);
        let evs = events_impl(
            &state,
            &[],
            Some(&rows),
            "trace.id",
            "A",
            None,
            None,
            0,
            100,
        )
        .unwrap();
        assert_eq!(
            evs.rows.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![2, 0, 1]
        );
        assert_eq!(evs.missing_time, 1);
        let page = events_impl(
            &state,
            &[],
            Some(&rows),
            "trace.id",
            "A",
            Some(0),
            Some(20),
            1,
            1,
        )
        .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows[0].id, 0);
    }
    #[test]
    fn cardinality_pages_filters_and_entity_window() {
        let state = state();
        let rows: Vec<_> = (0..26000)
            .map(|i| event(i, &format!("request-{i:05}"), Some(i as i64)))
            .collect();
        let page = index_impl(
            &state,
            &[],
            Some(&rows),
            "trace.id",
            25999,
            100,
            "recent",
            true,
        )
        .unwrap();
        assert_eq!(page.total, 26000);
        assert_eq!(page.groups[0].value, "request-00000");
        let fields = fields_impl(&state, &[], Some(&rows)).unwrap();
        assert!(fields[0].suggested);
        assert_eq!(fields[0].kind, "trace");
        assert_eq!(fields[0].coverage, 1.0);
        assert!(fields[0].distinct.is_none());
        assert!(fields[0].distinct_limited);
        assert!(events_impl(
            &state,
            &[],
            Some(&rows),
            "user.name",
            "demo",
            None,
            None,
            0,
            100
        )
        .is_err());
        assert!(index_impl(&state, &[], Some(&rows), "event_id", 0, 50, "recent", true).is_err());
        assert_eq!(kind("event.id"), "custom");
        assert_eq!(kind("http.request.id"), "request");
        assert_eq!(kind("metadata.correlation_id"), "correlation");
        let filters = vec![Filter {
            column: "trace.id".into(),
            op: "equals_exact".into(),
            value: "request-00007".into(),
            value2: None,
        }];
        let idx = index_impl(
            &state,
            &filters,
            Some(&rows),
            "trace.id",
            0,
            10,
            "count",
            true,
        )
        .unwrap();
        assert_eq!(idx.total, 1);
        assert_eq!(idx.groups[0].value, "request-00007");
    }

    #[test]
    fn entity_index_window_and_bounded_previews() {
        let state = state();
        let mut rows = vec![
            event(0, "flow", Some(0)),
            event(1, "flow", Some(10)),
            event(2, "flow", None),
        ];
        for row in &mut rows {
            row.fields.insert("user.name".into(), "demo".into());
        }
        rows[0].message = "\u{0}".repeat(100_000);
        assert!(window_filters("user.name", vec![], None, None).is_err());
        assert!(window_filters("user.name", vec![], Some(10), Some(0)).is_err());
        let filters = window_filters("user.name", vec![], Some(0), Some(0)).unwrap();
        let index = index_impl(
            &state,
            &filters,
            Some(&rows),
            "user.name",
            0,
            50,
            "recent",
            true,
        )
        .unwrap();
        assert_eq!(index.total, 1);
        assert_eq!(index.groups[0].count, 1);
        let events = events_impl(
            &state,
            &[],
            Some(&rows),
            "trace.id",
            "flow",
            None,
            None,
            0,
            100,
        )
        .unwrap();
        assert_eq!(events.total, 3);
        assert_eq!(events.rows_clipped, 1);
        assert!(
            serde_json::to_vec(&events.rows[0]).unwrap().len()
                <= crate::event_preview::PREVIEW_BYTES
        );
        assert_eq!(rows[0].message.len(), 100_000);
    }

    #[test]
    fn indexed_and_case_scopes_keep_exact_field_and_key() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for value in [
            serde_json::json!({"timestamp":"2026-09-22T10:00:02Z","source":"api","trace":{"id":"Flow"},"level":"error"}),
            serde_json::json!({"timestamp":"2026-09-22T10:00:01Z","source":"db","trace":{"id":"Flow"}}),
            serde_json::json!({"timestamp":"2026-09-22T10:00:03Z","source":"api","trace":{"id":"flow"}}),
            serde_json::json!({"timestamp":"2026-09-22T10:00:04Z","source":"api","trace_id":"Flow"}),
        ] {
            writeln!(file, "{value}").unwrap();
        }
        file.flush().unwrap();
        let index =
            sources::index_file(file.path().to_str().unwrap(), "auto", None, None, None).unwrap();
        let state = state();
        *state.source.write() = SourceData::Indexed(index);
        let groups = index_impl(&state, &[], None, "trace.id", 0, 50, "count", false).unwrap();
        assert_eq!(groups.total, 1);
        assert_eq!(groups.groups[0].count, 2);
        assert_eq!(groups.groups[0].sources.len(), 2);
        assert_eq!(groups.missing_key, 1);
        let events =
            events_impl(&state, &[], None, "trace.id", "Flow", None, None, 0, 100).unwrap();
        assert_eq!(events.total, 2);
        assert_eq!(events.rows[0].source, "db");
        assert_eq!(events.rows[1].source, "api");
        let case = events.rows[..1].to_vec();
        let subset =
            index_impl(&state, &[], Some(&case), "trace.id", 0, 50, "count", true).unwrap();
        assert_eq!(subset.total, 1);
        assert_eq!(subset.groups[0].count, 1);
        assert_eq!(subset.groups[0].sources, vec!["db"]);
        let empty =
            events_impl(&state, &[], None, "trace.id", " Flow", None, None, 0, 100).unwrap();
        assert_eq!(empty.total, 0);
    }
}
