use crate::{
    insights,
    model::{CodesConfig, Event},
    query::{self, Filter},
    sources::{self, CompiledDerived},
    AppState, SourceData,
};
use serde::Serialize;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ImportSource {
    File {
        paths: Vec<String>,
        format: String,
    },
    Eventlog {
        channel: String,
        #[serde(rename = "maxEvents")]
        max_events: usize,
    },
}
#[tauri::command]
pub async fn load_bundle(
    members: Vec<ImportSource>,
    app: AppHandle,
) -> Result<crate::LoadSummary, String> {
    crate::offload(move || {
        let mut combined: Option<sources::FileIndex> = None;
        let mut names = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for member in members {
            let inputs = match member {
                ImportSource::File { paths, format } => paths
                    .into_iter()
                    .map(|path| (path, format.clone(), None))
                    .collect::<Vec<_>>(),
                ImportSource::Eventlog {
                    channel,
                    max_events,
                } => vec![(channel, String::new(), Some(max_events.clamp(1, 100_000)))],
            };
            for (path, format, max_events) in inputs {
                crate::operations::check()?;
                let key = std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
                if !seen.insert(key) {
                    continue;
                }
                let idx = if let Some(max) = max_events {
                    index_channel(&path, max)?
                } else {
                    crate::index_source_file(&path, &format, Some(&app))?
                };
                if let Some(all) = &mut combined {
                    all.append(idx);
                } else {
                    combined = Some(idx);
                }
                names.push(path);
            }
        }
        let idx = combined.ok_or("Selecione ao menos uma fonte.")?;
        crate::operations::check()?;
        let summary = crate::LoadSummary {
            count: idx.lines.len(),
            columns: idx.columns.clone(),
            source_desc: names.join(" + "),
        };
        let state = app.state::<AppState>();
        let mut source = state.source.write();
        crate::operations::commit();
        *source = SourceData::Indexed(idx);
        *state.source_names.write() = names;
        Ok(summary)
    })
    .await?
}

pub struct Selection<'a> {
    source: &'a SourceData,
    ids: Vec<usize>,
    codes: &'a CodesConfig,
    system: &'a CodesConfig,
    derived: &'a [CompiledDerived],
}
impl Selection<'_> {
    pub fn iter(&self) -> Box<dyn Iterator<Item = Event> + '_> {
        match self.source {
            SourceData::Indexed(idx) => Box::new(
                self.ids
                    .iter()
                    .take_while(|_| !crate::operations::cancelled())
                    .map(|&i| sources::event_at(idx, i, self.codes, self.system, self.derived)),
            ),
            SourceData::Memory(events) => Box::new(
                self.ids
                    .iter()
                    .take_while(|_| !crate::operations::cancelled())
                    .map(|&i| events[i].clone()),
            ),
            SourceData::None => Box::new(std::iter::empty()),
        }
    }
}
pub fn with_selection<T>(
    state: &AppState,
    filters: &[Filter],
    f: impl FnOnce(Selection<'_>) -> T,
) -> T {
    let source = state.source.read();
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    let ids = match &*source {
        SourceData::Indexed(idx) => query::indexed_matches(idx, filters, &codes, &system, &derived),
        SourceData::Memory(events) => query::filtered_indices(events, filters),
        SourceData::None => vec![],
    };
    f(Selection {
        source: &source,
        ids,
        codes: &codes,
        system: &system,
        derived: &derived,
    })
}

pub fn validate(filters: &[Filter]) -> Result<(), String> {
    for f in filters {
        if ![
            "contains",
            "not_contains",
            "equals",
            "not_equals",
            "equals_exact",
            "not_equals_exact",
            "starts_with",
            "regex",
            "gt",
            "gte",
            "lt",
            "lte",
            "between",
            "empty",
            "not_empty",
            "pattern",
            "threat_rule",
        ]
        .contains(&f.op.as_str())
        {
            return Err(format!("Operador inválido: {}", f.op));
        }
        if f.op == "regex" {
            regex::Regex::new(&f.value).map_err(|e| format!("Expressão inválida: {e}"))?;
        }
        if f.op == "threat_rule" {
            if f.column != "_all" {
                return Err("O filtro de ameaça deve usar a coluna _all.".into());
            }
            crate::threats::matcher(&f.value, None)?;
        }
        if ["gt", "gte", "lt", "lte", "between"].contains(&f.op.as_str()) {
            let parse = |v: &str| {
                v.parse::<f64>()
                    .ok()
                    .or_else(|| {
                        if f.column == "timestamp" {
                            sources::parse_timestamp(v).map(|n| n as f64)
                        } else {
                            crate::analysis::parse_num_unit(v).map(|(n, _)| n)
                        }
                    })
                    .filter(|v| v.is_finite())
            };
            let lo = parse(&f.value).ok_or_else(|| format!("Valor inválido para {}.", f.column))?;
            if f.op == "between" {
                let hi = parse(f.value2.as_deref().unwrap_or(""))
                    .ok_or("Informe o fim do intervalo.")?;
                if lo > hi {
                    return Err("O início deve ser anterior ao fim.".into());
                }
            }
        }
    }
    Ok(())
}
#[tauri::command]
pub fn validate_filters(filters: Vec<Filter>) -> Result<(), String> {
    validate(&filters)
}
#[tauri::command]
pub fn cancel_operation() {
    crate::operations::cancel();
}

pub fn overview_impl(state: &AppState, filters: Vec<Filter>) -> Result<insights::Overview, String> {
    overview_scope_impl(state, filters, None)
}
pub fn overview_scope_impl(
    state: &AppState,
    filters: Vec<Filter>,
    case_events: Option<&[Event]>,
) -> Result<insights::Overview, String> {
    validate(&filters)?;
    if let Some(events) = case_events {
        let prepared = query::prepare(&filters);
        let result = insights::overview(|| {
            events
                .iter()
                .filter(|event| prepared.iter().all(|filter| query::matches(event, filter)))
                .cloned()
        });
        crate::operations::check()?;
        return Ok(result);
    }
    Ok(with_selection(state, &filters, |selection| {
        insights::overview(|| selection.iter())
    }))
}
#[tauri::command]
pub async fn dataset_overview(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<insights::Overview, String> {
    crate::offload(move || {
        overview_scope_impl(
            app.state::<AppState>().inner(),
            filters,
            case_events.as_deref(),
        )
    })
    .await?
}

#[derive(Serialize)]
pub struct TimelineBucket {
    timestamp: i64,
    count: usize,
    errors: usize,
    warnings: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRange {
    start: i64,
    end: i64,
    bucket_ms: i64,
    total: usize,
    errors: usize,
    warnings: usize,
    buckets: Vec<TimelineBucket>,
}

pub fn timeline_range_impl(
    state: &AppState,
    filters: Vec<Filter>,
    start: i64,
    end: i64,
    bucket_count: usize,
) -> Result<TimelineRange, String> {
    timeline_range_scope_impl(state, filters, start, end, bucket_count, None)
}
pub fn timeline_range_scope_impl(
    state: &AppState,
    filters: Vec<Filter>,
    start: i64,
    end: i64,
    bucket_count: usize,
    case_events: Option<&[Event]>,
) -> Result<TimelineRange, String> {
    validate(&filters)?;
    if start > end {
        return Err("O início deve ser anterior ao fim.".into());
    }
    let mut scoped_filters = Vec::with_capacity(filters.len() + 1);
    scoped_filters.push(Filter {
        column: "timestamp".into(),
        op: "between".into(),
        value: start.to_string(),
        value2: Some(end.to_string()),
    });
    scoped_filters.extend(filters);
    let bucket_count = bucket_count.clamp(1, 240);
    let span = end.saturating_sub(start).saturating_add(1);
    let width = (span / bucket_count as i64)
        .saturating_add(i64::from(span % bucket_count as i64 != 0))
        .max(1);
    // Rounding the bucket width up can otherwise create empty buckets beyond
    // the requested end, especially when zoomed into a sub-second interval.
    let bucket_count = (((span - 1) / width + 1) as usize).min(bucket_count);
    let mut result = TimelineRange {
        start,
        end,
        bucket_ms: width,
        total: 0,
        errors: 0,
        warnings: 0,
        buckets: (0..bucket_count)
            .map(|i| TimelineBucket {
                timestamp: start.saturating_add((i as i64).saturating_mul(width)),
                count: 0,
                errors: 0,
                warnings: 0,
            })
            .collect(),
    };
    let mut add = |timestamp: i64, level: &str| {
        if timestamp < start || timestamp > end {
            return;
        }
        let index = (timestamp.saturating_sub(start) / width) as usize;
        let bucket = &mut result.buckets[index.min(bucket_count - 1)];
        bucket.count += 1;
        result.total += 1;
        if matches!(level, "Erro" | "Crítico") {
            bucket.errors += 1;
            result.errors += 1;
        } else if level == "Aviso" {
            bucket.warnings += 1;
            result.warnings += 1;
        }
    };
    if let Some(events) = case_events {
        let prepared = query::prepare(&scoped_filters);
        for event in events {
            crate::operations::check()?;
            if prepared.iter().all(|filter| query::matches(event, filter)) {
                if let Some(timestamp) = event.timestamp {
                    add(timestamp, &event.level);
                }
            }
        }
        return Ok(result);
    }
    let source = state.source.read();
    match &*source {
        SourceData::Indexed(idx) => {
            if scoped_filters.len() == 1 {
                // The normal timeline reads only the compact index metadata.
                // It never allocates an ID for every matching log line.
                for (id, meta) in idx.lines.iter().enumerate() {
                    if id % 2048 == 0 {
                        crate::operations::check()?;
                    }
                    if meta.ts != 0 {
                        add(meta.ts, crate::model::class_label(meta.level));
                    }
                }
            } else {
                let codes = state.codes.read();
                let system = state.system_codes.read();
                let derived = state.derived.read();
                let matched =
                    query::indexed_matches(idx, &scoped_filters, &codes, &system, &derived);
                crate::operations::check()?;
                for id in matched {
                    crate::operations::check()?;
                    let meta = &idx.lines[id];
                    if meta.ts != 0 {
                        add(meta.ts, crate::model::class_label(meta.level));
                    }
                }
            }
        }
        SourceData::Memory(events) => {
            let matched = query::filtered_indices(events, &scoped_filters);
            crate::operations::check()?;
            for id in matched {
                crate::operations::check()?;
                let ev = &events[id];
                if let Some(timestamp) = ev.timestamp {
                    add(timestamp, &ev.level);
                }
            }
        }
        SourceData::None => {}
    }
    Ok(result)
}

#[tauri::command]
pub async fn timeline_range(
    filters: Vec<Filter>,
    start: i64,
    end: i64,
    bucket_count: usize,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<TimelineRange, String> {
    crate::offload(move || {
        if case_events.is_none() {
            return timeline_range_impl(
                app.state::<AppState>().inner(),
                filters,
                start,
                end,
                bucket_count,
            );
        }
        timeline_range_scope_impl(
            app.state::<AppState>().inner(),
            filters,
            start,
            end,
            bucket_count,
            case_events.as_deref(),
        )
    })
    .await?
}
#[tauri::command]
pub async fn compare_periods(
    filters: Vec<Filter>,
    before: insights::Period,
    after: insights::Period,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<insights::Comparison, String> {
    crate::offload(move || {
        if case_events.is_none() {
            compare_impl(app.state::<AppState>().inner(), filters, before, after)
        } else {
            compare_scope_impl(
                app.state::<AppState>().inner(),
                filters,
                before,
                after,
                case_events.as_deref(),
            )
        }
    })
    .await?
}
pub fn compare_impl(
    state: &AppState,
    filters: Vec<Filter>,
    before: insights::Period,
    after: insights::Period,
) -> Result<insights::Comparison, String> {
    compare_scope_impl(state, filters, before, after, None)
}
pub fn compare_scope_impl(
    state: &AppState,
    filters: Vec<Filter>,
    before: insights::Period,
    after: insights::Period,
    case_events: Option<&[Event]>,
) -> Result<insights::Comparison, String> {
    validate(&filters)?;
    if before.start > before.end || after.start > after.end {
        return Err("Revise os intervalos de comparação.".into());
    }
    if before.start <= after.end && after.start <= before.end {
        return Err("Os períodos não podem se sobrepor.".into());
    }
    if let Some(events) = case_events {
        let prepared = query::prepare(&filters);
        let result = insights::compare(
            events
                .iter()
                .filter(|event| prepared.iter().all(|filter| query::matches(event, filter)))
                .cloned(),
            &before,
            &after,
        );
        crate::operations::check()?;
        return Ok(result);
    }
    Ok(with_selection(state, &filters, |s| {
        insights::compare(s.iter(), &before, &after)
    }))
}

#[derive(Serialize)]
pub struct SourceInfo {
    pub id: String,
    pub path: String,
    pub name: String,
    pub format: String,
    pub bytes: u64,
    pub count: usize,
    pub undated: usize,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub sampled: usize,
    pub unparsed: usize,
}
pub fn sources_impl(state: &AppState) -> Vec<SourceInfo> {
    let source = state.source.read();
    match &*source {
        SourceData::Indexed(idx) => idx
            .parts
            .iter()
            .map(|p| {
                let start = idx.lines.partition_point(|m| m.offset < p.base);
                let end = idx
                    .lines
                    .partition_point(|m| m.offset < p.base + p.mmap.len() as u64);
                let lines = &idx.lines[start..end];
                let min = lines
                    .iter()
                    .filter_map(|m| (m.ts != 0).then_some(m.ts))
                    .min();
                let max = lines
                    .iter()
                    .filter_map(|m| (m.ts != 0).then_some(m.ts))
                    .max();
                let sample = lines.len().min(200);
                let mut unparsed = 0;
                for n in 0..sample {
                    let ev = sources::parse_line(
                        sources::line_bytes(idx, start + n * lines.len() / sample),
                        &p.format,
                        p.custom.as_ref(),
                        &p.header,
                    );
                    if ev.parse_status == "unparsed" {
                        unparsed += 1;
                    }
                }
                SourceInfo {
                    id: p.identity.clone(),
                    path: p.path.clone(),
                    name: p.file_name.clone(),
                    format: p.format.clone(),
                    bytes: p.mmap.len() as u64,
                    count: lines.len(),
                    undated: lines.iter().filter(|m| m.ts == 0).count(),
                    start: min,
                    end: max,
                    sampled: sample,
                    unparsed,
                }
            })
            .collect(),
        SourceData::Memory(events) => vec![SourceInfo {
            id: "eventlog".into(),
            path: String::new(),
            name: state.source_names.read().join(" + "),
            format: "Event Log".into(),
            bytes: 0,
            count: events.len(),
            undated: events.iter().filter(|e| e.timestamp.is_none()).count(),
            start: events.iter().filter_map(|e| e.timestamp).min(),
            end: events.iter().filter_map(|e| e.timestamp).max(),
            sampled: events.len(),
            unparsed: 0,
        }],
        SourceData::None => vec![],
    }
}
#[tauri::command]
pub async fn list_sources(app: AppHandle) -> Result<Vec<SourceInfo>, String> {
    crate::offload(move || sources_impl(app.state::<AppState>().inner())).await
}

pub fn index_events(events: &[Event]) -> Result<sources::FileIndex, String> {
    let dir = crate::config_dir().join("snapshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "events-{}.jsonl",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let mut file = BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
    for ev in events {
        serde_json::to_writer(&mut file, ev).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    sources::index_file(&path.to_string_lossy(), "snapshot", None, None, None)
}

pub fn index_channel(channel: &str, max_events: usize) -> Result<sources::FileIndex, String> {
    use sha2::{Digest, Sha256};
    let dir = crate::config_dir().join("snapshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "windows-{}.jsonl",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let result = (|| {
        let mut file = BufWriter::new(std::fs::File::create(&path).map_err(|e| e.to_string())?);
        let count = sources::visit_channel(channel, max_events, |mut event| {
            let mut hash = Sha256::new();
            hash.update(channel.as_bytes());
            hash.update(event.raw.as_bytes());
            event.event_ref = format!("windows:{:x}", hash.finalize());
            serde_json::to_writer(&mut file, &event).map_err(|e| e.to_string())?;
            file.write_all(b"\n").map_err(|e| e.to_string())
        })?;
        if count == 0 {
            return Err("Nenhum evento encontrado nesta fonte.".into());
        }
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        let mut idx = sources::index_file(&path.to_string_lossy(), "snapshot", None, None, None)?;
        idx.parts[0].path = channel.to_string();
        idx.parts[0].file_name = Path::new(channel)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        Ok(idx)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    result
}

fn csv(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.into()
    };
    format!("\"{}\"", value.replace('"', "\"\""))
}
pub fn redact(value: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(||regex::Regex::new(r#"(?i)(password|passwd|token|secret|authorization|api[_-]?key)(["']?\s*[:=]\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:Bearer|Basic)\s+[^\s",;]+|[^\s",;]+)"#).unwrap()).replace_all(value,"$1$2\"[oculto]\"").into_owned()
}
pub fn redact_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase().replace(['_', '-'], "");
                if [
                    "password",
                    "passwd",
                    "token",
                    "secret",
                    "authorization",
                    "apikey",
                    "accesstoken",
                    "refreshtoken",
                ]
                .contains(&key.as_str())
                {
                    *value = serde_json::Value::String("[oculto]".into());
                } else {
                    redact_value(value);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_value(value);
            }
        }
        serde_json::Value::String(text) => *text = redact(text),
        _ => {}
    }
}
#[tauri::command]
pub async fn export_events(
    path: String,
    format: String,
    filters: Vec<Filter>,
    mask: bool,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<usize, String> {
    crate::offload(move || {
        validate(&filters)?;
        if !["csv", "jsonl"].contains(&format.as_str()) {
            return Err("Formato de exportação inválido.".into());
        }
        let state = app.state::<AppState>();
        let requested = PathBuf::from(&path);
        let canonical = requested.canonicalize().unwrap_or(requested.clone());
        if let SourceData::Indexed(idx) = &*state.source.read() {
            if idx.parts.iter().any(|p| {
                PathBuf::from(&p.path)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(&p.path))
                    == canonical
            }) {
                return Err("Escolha outro arquivo para não substituir a fonte.".into());
            }
        }
        let parent = requested
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut pending = tempfile::Builder::new()
            .prefix(".loginsight-export-")
            .suffix(".part")
            .tempfile_in(parent)
            .map_err(|e| e.to_string())?;
        {
            let mut file = BufWriter::new(pending.as_file_mut());
            let count = if let Some(events) = case_events.as_deref() {
                let prepared = query::prepare(&filters);
                write_events_export(&mut file, &format, mask, || {
                    events
                        .iter()
                        .filter(|event| prepared.iter().all(|filter| query::matches(event, filter)))
                        .cloned()
                })
            } else {
                with_selection(state.inner(), &filters, |s| {
                    write_events_export(&mut file, &format, mask, || s.iter())
                })
            }?;
            crate::operations::check()?;
            file.flush().map_err(|e| e.to_string())?;
            file.get_ref().sync_all().map_err(|e| e.to_string())?;
            drop(file);
            crate::operations::check()?;
            // Same-directory atomic replacement works for existing destinations
            // on Windows and Unix, after the save picker confirms overwriting.
            pending
                .persist(&requested)
                .map_err(|e| e.error.to_string())?;
            crate::operations::commit();
            Ok(count)
        }
    })
    .await?
}
pub(crate) fn write_events_export<F, I>(
    file: &mut impl Write,
    format: &str,
    mask: bool,
    events: F,
) -> Result<usize, String>
where
    F: Fn() -> I,
    I: Iterator<Item = Event>,
{
    let mut columns: Vec<String> = crate::model::STANDARD_COLUMNS
        .iter()
        .map(|v| v.to_string())
        .collect();
    columns.extend(["id", "event_ref", "parse_status", "raw"].map(str::to_string));
    if format == "csv" {
        let mut extra = std::collections::BTreeSet::new();
        for event in events() {
            crate::operations::check()?;
            for key in event.fields.keys() {
                extra.insert(key.clone());
                if extra.len() > 10000 {
                    return Err(
                        "Há mais de 10.000 campos. Use JSONL para preservar todos os dados.".into(),
                    );
                }
            }
        }
        columns.extend(extra.into_iter().filter(|c| {
            !crate::model::STANDARD_COLUMNS.contains(&c.as_str())
                && !["id", "event_ref", "parse_status", "raw"].contains(&c.as_str())
        }));
        writeln!(
            file,
            "{}",
            columns.iter().map(|v| csv(v)).collect::<Vec<_>>().join(",")
        )
        .map_err(|e| e.to_string())?;
    }
    let mut count = 0;
    for ev in events() {
        crate::operations::check()?;
        let mut value = serde_json::to_value(&ev).map_err(|e| e.to_string())?;
        if mask {
            redact_value(&mut value);
        }
        let text = if format == "csv" {
            columns
                .iter()
                .map(|column| {
                    let text = if column == "timestamp" {
                        ev.timestamp
                            .map(crate::model::ts_to_iso)
                            .unwrap_or_default()
                    } else {
                        value
                            .get(column)
                            .or_else(|| value.get("fields").and_then(|v| v.get(column)))
                            .filter(|v| !v.is_null())
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .unwrap_or_default()
                    };
                    csv(&text)
                })
                .collect::<Vec<_>>()
                .join(",")
        } else {
            serde_json::to_string(&value).map_err(|e| e.to_string())?
        };
        writeln!(file, "{text}").map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}
#[tauri::command]
pub async fn export_document(path: String, content: String) -> Result<(), String> {
    if content.len() > 64 * 1024 * 1024 {
        return Err("O relatório excede 64 MB.".into());
    }
    crate::offload(move || {
        let path = Path::new(&path);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temp.write_all(content.as_bytes())
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        crate::operations::check()?;
        temp.persist(path).map_err(|e| e.error.to_string())?;
        crate::operations::commit();
        Ok(())
    })
    .await?
}
#[tauri::command]
pub async fn import_investigation(path: String) -> Result<serde_json::Value, String> {
    crate::offload(move || crate::case_images::import_document(&path)).await?
}

#[tauri::command]
pub async fn expand_paths(paths: Vec<String>) -> Result<Vec<String>, String> {
    crate::offload(move || {
        let mut pending: Vec<(PathBuf, bool)> = paths
            .into_iter()
            .map(|p| (PathBuf::from(p), true))
            .collect();
        let mut files = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while let Some((path, explicit)) = pending.pop() {
            crate::operations::check()?;
            let canonical = path.canonicalize().map_err(|e| e.to_string())?;
            if !seen.insert(canonical) {
                continue;
            }
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if !entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
                        pending.push((entry.path(), false));
                    }
                }
            } else if path.is_file() {
                if files.len() >= 10000 {
                    return Err("Selecione até 10.000 arquivos por vez.".into());
                }
                let ext = path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                if explicit
                    || [
                        "log", "txt", "jsonl", "ndjson", "json", "csv", "tsv", "evtx", "gz",
                    ]
                    .contains(&ext.as_str())
                    || ext.parse::<u32>().is_ok()
                {
                    files.push(path.to_string_lossy().into_owned());
                }
            }
        }
        files.sort();
        files.dedup();
        if files.is_empty() {
            return Err("Nenhum arquivo de log encontrado.".into());
        }
        Ok(files)
    })
    .await?
}
pub fn expand_gzip(path: &Path) -> Result<PathBuf, String> {
    let dir = crate::config_dir().join("expanded");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mapped = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|e| e.to_string())?;
    let id = crate::index_cache::identity(&path.to_string_lossy(), &mapped);
    drop(mapped);
    let target = dir.join(format!(
        "{}-{}",
        &id[..16],
        path.file_stem().unwrap_or_default().to_string_lossy()
    ));
    if target.exists() {
        return Ok(target);
    }
    let temp = target.with_extension("pending");
    let result = (|| {
        let mut input = flate2::read::MultiGzDecoder::new(file);
        let mut out = BufWriter::new(std::fs::File::create(&temp).map_err(|e| e.to_string())?);
        let mut buf = [0u8; 65536];
        loop {
            crate::operations::check()?;
            let n = input.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&temp, &target).map_err(|e| e.to_string())?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
