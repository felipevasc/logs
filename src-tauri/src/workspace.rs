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
    operation_id: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
) -> Result<crate::LoadSummary, String> {
    let admitted = crate::analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, crate::analysis_runtime::Mode::Publish, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    crate::offload_admitted(operation_id, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        load_bundle_impl(state.inner(), members, Some(&app))
    }).await?
}

pub(crate) fn load_bundle_impl(
    state: &AppState,
    members: Vec<ImportSource>,
    app: Option<&AppHandle>,
) -> Result<crate::LoadSummary, String> {
    let mut combined: Option<sources::FileIndex> = None;
    let mut names = Vec::new();
    let mut published_inputs = Vec::new();
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
                crate::index_source_file(&path, &format, app)?
            };
            if let Some(all) = &mut combined {
                all.append(idx)?;
            } else {
                combined = Some(idx);
            }
            published_inputs.push(match max_events {
                Some(max_events) => crate::source_publication::Input::Eventlog { channel: path.clone(), max_events },
                None => crate::source_publication::Input::File { paths: vec![path.clone()], format },
            });
            names.push(path);
        }
    }
    let idx = combined.ok_or("Selecione ao menos uma fonte.")?;
    crate::operations::check()?;
    crate::prepare_engine(state, &idx, app)?;
    crate::emit_progress(app, "carregamento", "Ativando fonte carregada", 0, 0, "registros", true);
    crate::source_publication::publish(state, idx, names, published_inputs, false)
}

pub struct Selection<'a> {
    source: &'a SourceData,
    prepared: std::sync::Arc<Vec<query::PreparedFilter>>,
    codes: &'a CodesConfig,
    system: &'a CodesConfig,
    derived: &'a [CompiledDerived],
    failure: std::sync::Arc<parking_lot::Mutex<Option<String>>>,
    hydrated: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

/// Exactly one queued batch, one batch held by the consumer, and one being
/// filled by the producer. A record larger than the payload budget travels
/// alone. Early drop disconnects the channel before joining its producer.
struct EventBatches {
    receiver: Option<std::sync::mpsc::Receiver<Vec<Event>>>,
    worker: Option<std::thread::JoinHandle<()>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    failure: std::sync::Arc<parking_lot::Mutex<Option<String>>>,
}
impl Iterator for EventBatches {
    type Item = Vec<Event>;
    fn next(&mut self) -> Option<Self::Item> { crate::global_scheduler::blocking(|| self.receiver.as_ref()?.recv().ok()) }
}
impl Drop for EventBatches {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.receiver.take();
        if self.worker.take().is_some_and(|worker| crate::global_scheduler::blocking(|| worker.join()).is_err()) {
            *self.failure.lock() = Some("Falha na leitura em fluxo dos eventos.".into());
        }
    }
}
struct OrderedEvents { batches: EventBatches, current: std::vec::IntoIter<Event> }
impl Iterator for OrderedEvents {
    type Item = Event;
    fn next(&mut self) -> Option<Event> {
        loop {
            if let Some(event) = self.current.next() { return Some(event); }
            self.current = self.batches.next()?.into_iter();
        }
    }
}

impl Selection<'_> {
    /// A staging/preview consumer can stream exact row IDs from its admitted
    /// indexed scope, retaining the caller's existing source/visibility guards.
    pub(crate) fn visit_exact_ids(&self, visit: impl FnMut(usize) -> Result<bool, String>) -> Result<Option<()>, String> {
        let SourceData::Indexed(index) = self.source else { return Ok(None); };
        crate::engine::visit_exact_matches(&crate::engine::Source { idx: index, codes: self.codes, system: self.system, derived: self.derived }, &self.prepared, visit)
    }

    fn batches(&self, idx: &sources::FileIndex) -> EventBatches {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        // The 50M line metadata and mmaps are shared; do not copy time_order.
        let idx = sources::FileIndex { parts: idx.parts.clone(), lines: Arc::clone(&idx.lines), columns: idx.columns.clone(), time_order: Arc::clone(&idx.time_order) };
        let (codes, system, derived) = ((*self.codes).clone(), (*self.system).clone(), self.derived.to_vec());
        let prepared = Arc::clone(&self.prepared);
        let failure = Arc::clone(&self.failure);
        let report_failure = Arc::clone(&failure);
        let hydrated = Arc::clone(&self.hydrated);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let token = crate::operations::current_token().with_stop(Arc::clone(&stop));
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<Event>>(1);
        let worker = std::thread::Builder::new().name("loginsight-event-stream".into()).spawn(move || {
            let result = crate::operations::run_with_token(token, || {
                let mut batch = Vec::new();
                let mut bytes = 0usize;
                let mut disconnected = false;
                query::visit_indexed_prepared_control(&idx, &prepared, &codes, &system, &derived, |id| {
                    if worker_stop.load(Ordering::Relaxed) { return Ok(false); }
                    let event = sources::event_at(&idx, id, &codes, &system, &derived);
                    hydrated.fetch_add(1, Ordering::Relaxed);
                    let size = query::event_payload_bytes(&event);
                    if !batch.is_empty() && (batch.len() >= 8192 || bytes.saturating_add(size) > crate::resources::batch_bytes()) {
                        if crate::global_scheduler::blocking(|| sender.send(std::mem::take(&mut batch))).is_err() { disconnected = true; return Ok(false); }
                        bytes = 0;
                    }
                    bytes = bytes.saturating_add(size);
                    batch.push(event);
                    Ok(true)
                })?;
                if !disconnected && !batch.is_empty() { let _ = crate::global_scheduler::blocking(|| sender.send(batch)); }
                Ok::<(), String>(())
            }).and_then(|value| value);
            if let Err(error) = result {
                if !worker_stop.load(Ordering::Relaxed) { *report_failure.lock() = Some(error); }
            }
        });
        match worker {
            Ok(worker) => EventBatches { receiver: Some(receiver), worker: Some(worker), stop, failure },
            Err(error) => {
                *failure.lock() = Some(error.to_string());
                EventBatches { receiver: None, worker: None, stop, failure }
            }
        }
    }
    /// Each bounded wave folds in parallel; no complete-ID vector is required.
    pub fn par_fold<A: Send>(&self, init: impl Fn() -> A + Sync + Send, step: impl Fn(&mut A, &Event) + Sync + Send, merge: impl Fn(A, A) -> A + Sync + Send) -> A {
        let token = crate::operations::current_token();
        match self.source {
            SourceData::Indexed(idx) => {
                let mut result = init();
                for batch in self.batches(idx) {
                    let next = crate::global_scheduler::map(batch.chunks(batch.len().div_ceil(crate::resources::workers()).max(1)), |events| {
                        let mut value = init();
                        for event in events { if !token.cancelled() { step(&mut value, event); } }
                        value
                    }).into_iter().fold(init(), &merge);
                    result = merge(result, next);
                    if token.cancelled() { break; }
                }
                result
            }
            SourceData::Memory(events) => {
                let mut result = init();
                // Retain at most one bounded wave's accumulators, never one
                // accumulator per chunk across the entire in-memory source.
                for wave in events.chunks(8192) {
                    let next = crate::global_scheduler::map(wave.chunks(wave.len().div_ceil(crate::resources::workers()).max(1)), |events| {
                        let mut value = init();
                        for event in events {
                            if !token.cancelled() && self.prepared.iter().all(|pf| query::matches(event, pf)) { step(&mut value, event); }
                        }
                        value
                    }).into_iter().fold(init(), &merge);
                    result = merge(result, next);
                    if token.cancelled() { break; }
                }
                result
            }
            SourceData::None => init(),
        }
    }
    pub fn event(&self, id: usize) -> Option<Event> {
        match self.source {
            SourceData::Indexed(idx) => match crate::analysis_runtime::row_visible(idx, id) {
                Ok(true) => Some(sources::event_at(idx, id, self.codes, self.system, self.derived)),
                Ok(false) => None,
                Err(error) => { crate::analysis_runtime::record_failure(error); None }
            },
            SourceData::Memory(events) => events.get(id).filter(|event| event.id == id).or_else(|| events.iter().find(|event| event.id == id)).cloned(),
            SourceData::None => None,
        }
    }
    pub fn iter(&self) -> Box<dyn Iterator<Item = Event> + '_> {
        match self.source {
            SourceData::Indexed(idx) => Box::new(OrderedEvents { batches: self.batches(idx), current: Vec::new().into_iter() }),
            SourceData::Memory(events) => Box::new(events.iter().take_while(|_| !crate::operations::cancelled()).filter(|event| self.prepared.iter().all(|pf| query::matches(event, pf))).cloned()),
            SourceData::None => Box::new(std::iter::empty()),
        }
    }
    #[cfg(test)]
    pub(crate) fn hydrated_count(&self) -> usize { self.hydrated.load(std::sync::atomic::Ordering::Relaxed) }
}
/// Runs a query-engine operation on an indexed source; `None` means the
/// caller answers with the line engine.
pub(crate) fn with_engine<T>(
    state: &AppState,
    f: impl FnOnce(&crate::engine::Source<'_>) -> Result<Option<T>, String>,
) -> Result<Option<T>, String> {
    let source = crate::analysis_runtime::source(&state);
    let SourceData::Indexed(idx) = &*source else { return Ok(None) };
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    f(&crate::engine::Source { idx, codes: &codes, system: &system, derived: &derived })
}

pub fn with_selection<T>(state: &AppState, filters: &[Filter], f: impl FnOnce(Selection<'_>) -> T) -> Result<T, String> {
    let source = crate::analysis_runtime::source(&state);
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    if let SourceData::Indexed(idx) = &*source {
        for part in &idx.parts { sources::validate_source(part)?; }
    }
    let failure = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let result = f(Selection {
        source: &source, prepared: std::sync::Arc::new(query::prepare(filters)),
        codes: &codes, system: &system, derived: &derived,
        failure: std::sync::Arc::clone(&failure), hydrated: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    crate::operations::check()?;
    if let Some(error) = failure.lock().take() { return Err(error); }
    Ok(result)
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
            "query",
            "in",
            "in_exact",
            "not_in",
            "cidr",
            "not_cidr",
            "detection",
        ]
        .contains(&f.op.as_str())
        {
            return Err(format!("Operador inválido: {}", f.op));
        }
        if f.op == "query" {
            crate::querylang::compile(&f.value)?;
        }
        if f.op == "detection" && crate::detections::ruleset()?.find(&f.value).is_none() {
            return Err(format!("Regra de detecção não encontrada: {}.", f.value));
        }
        if matches!(f.op.as_str(), "in" | "not_in") && query::list_values(&f.value).next().is_none() {
            return Err("Informe ao menos um valor da lista.".into());
        }
        if matches!(f.op.as_str(), "cidr" | "not_cidr") {
            let mut any = false;
            for value in query::list_values(&f.value).flat_map(|v| v.split_whitespace()) {
                crate::querylang::IpNet::parse(value)
                    .ok_or_else(|| format!("Rede inválida: {value}. Use o formato 10.0.0.0/8."))?;
                any = true;
            }
            if !any {
                return Err("Informe ao menos uma rede.".into());
            }
        }
        if f.op == "regex" {
            crate::operations::check()?;
            crate::query_regex::compile(&f.value, crate::query_regex::ORDINARY).map_err(|e| format!("Expressão inválida: {e}"))?;
            crate::operations::check()?;
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
pub async fn validate_filters(filters: Vec<Filter>, app: AppHandle, analysis_context: Option<crate::analysis_context::Identity>) -> Result<(), String> {
    let admitted = crate::case_editor_admission_async(app.clone(), analysis_context).await?;
    crate::offload_admitted(None, app, admitted, move || validate(&filters)).await?
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
    if let Some(result) = with_engine(state, |src| crate::engine::overview(src, &query::prepare(&filters)))? {
        return Ok(result);
    }
    with_selection(state, &filters, |selection| {
        insights::overview(|| selection.iter())
    })
}
#[tauri::command]
pub async fn dataset_overview(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<insights::Overview, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
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
    let source = crate::analysis_runtime::source(&state);
    match &*source {
        SourceData::Indexed(idx) => {
            let gate = crate::analysis_runtime::indexed_gate(idx)?;
            if scoped_filters.len() == 1 {
                let codes = crate::analysis_runtime::codes(&state);
                let system = crate::analysis_runtime::system_codes(&state);
                let derived = crate::analysis_runtime::derived(&state);
                let input = crate::engine::Source { idx, codes: &codes, system: &system, derived: &derived };
                if let Some(histogram) = crate::engine::timeline_histogram(&input, start, end, width, bucket_count)? {
                    return Ok(TimelineRange {
                        start, end, bucket_ms: width, total: histogram.total, errors: histogram.errors, warnings: histogram.warnings,
                        buckets: histogram.buckets.into_iter().enumerate().map(|(i, b)| TimelineBucket {
                            timestamp: start.saturating_add((i as i64).saturating_mul(width)),
                            count: b.count, errors: b.errors, warnings: b.warnings,
                        }).collect(),
                    });
                }
                // The normal timeline reads only the compact index metadata.
                // It never allocates an ID for every matching log line.
                for (id, meta) in idx.lines.iter().enumerate() {
                    if id % 2048 == 0 {
                        crate::operations::check()?;
                    }
                    if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(id)) { continue; }
                    if meta.ts != 0 {
                        add(meta.ts, crate::model::class_label(meta.level));
                    }
                }
            } else {
                let codes = crate::analysis_runtime::codes(&state);
                let system = crate::analysis_runtime::system_codes(&state);
                let derived = crate::analysis_runtime::derived(&state);
                query::visit_indexed_matches(idx, &scoped_filters, &codes, &system, &derived, |id| {
                    let meta = &idx.lines.at(id);
                    if meta.ts != 0 { add(meta.ts, crate::model::class_label(meta.level)); }
                })?;
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
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<TimelineRange, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
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
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<insights::Comparison, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
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
    if let Some(result) =
        with_engine(state, |src| crate::engine::compare(src, &query::prepare(&filters), &before, &after))?
    {
        return Ok(result);
    }
    with_selection(state, &filters, |s| {
        insights::compare(s.iter(), &before, &after)
    })
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
pub fn sources_impl(state: &AppState) -> Result<Vec<SourceInfo>, String> {
    let source = crate::analysis_runtime::source(&state);
    Ok(match &*source {
        SourceData::Indexed(idx) => {
            let gate = crate::analysis_runtime::indexed_gate(idx)?;
            idx
            .parts
            .iter()
            .map(|p| {
                let start = idx.lines.partition_point(|m| m.offset < p.base);
                let end = idx
                    .lines
                    .partition_point(|m| m.offset < p.base + p.mmap.len() as u64);
                let mut count = 0usize;
                let mut undated = 0usize;
                let (mut min, mut max) = (None, None);
                let mut sample = Vec::new();
                for id in start..end {
                    if id % 2048 == 0 { crate::operations::check()?; }
                    if gate.as_ref().is_some_and(|gate| !gate.allows_known_row(id)) { continue; }
                    let meta = idx.lines.at(id);
                    count += 1;
                    if meta.ts == 0 { undated += 1; } else {
                        min = Some(min.map_or(meta.ts, |value: i64| value.min(meta.ts)));
                        max = Some(max.map_or(meta.ts, |value: i64| value.max(meta.ts)));
                    }
                    // A fixed deterministic reservoir remains bounded even if
                    // the visible source spans tens of millions of records.
                    if sample.len() < 200 { sample.push(id); }
                    else {
                        let position = (id as u64).wrapping_mul(0x9e3779b97f4a7c15).rotate_left(17) % count as u64;
                        if position < 200 { sample[position as usize] = id; }
                    }
                }
                let unparsed = sample.iter().filter(|&&id| sources::parse_part_line(p, sources::line_bytes(idx, id)).parse_status == "unparsed").count();
                Ok(SourceInfo {
                    id: p.identity.clone(),
                    path: p.path.clone(),
                    name: p.file_name.clone(),
                    format: p.format.clone(),
                    bytes: p.mmap.len() as u64,
                    count,
                    undated,
                    start: min,
                    end: max,
                    sampled: sample.len(),
                    unparsed,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
        },
        SourceData::Memory(events) => vec![SourceInfo {
            id: "eventlog".into(),
            path: String::new(),
            name: crate::analysis_runtime::source_names(state).join(" + "),
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
    })
}
#[tauri::command]
pub async fn list_sources(app: AppHandle, analysis_context: Option<crate::analysis_context::Identity>, source_generation: Option<u64>) -> Result<Vec<SourceInfo>, String> {
    let admitted = crate::analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, crate::analysis_runtime::Mode::Dataset, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_admitted(None, app.clone(), admitted, move || sources_impl(app.state::<AppState>().inner())).await?
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
    static HASH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static PEM: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let value = HASH
        .get_or_init(|| regex::Regex::new(r"\$(?:[156y]|2[aby])\$[./A-Za-z0-9$=,-]{20,}").unwrap())
        .replace_all(value, "[hash protegido]");
    let value=PEM.get_or_init(||regex::Regex::new(r"(?s)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.*?(?:-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|$)").unwrap()).replace_all(&value,"[chave privada oculta]");
    RE.get_or_init(||regex::Regex::new(r#"(?i)(password|passwd|token|secret|AWS_SECRET_ACCESS_KEY|authorization|api[_-]?key)(["']?\s*[:=]\s*)(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?:Bearer|Basic)\s+[^\s",;]+|[^\s",;]+)"#).unwrap()).replace_all(&value,"$1$2\"[oculto]\"").into_owned()
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
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<usize, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        validate(&filters)?;
        if !["csv", "jsonl"].contains(&format.as_str()) {
            return Err("Formato de exportação inválido.".into());
        }
        let state = app.state::<AppState>();
        let requested = PathBuf::from(&path);
        let canonical = requested.canonicalize().unwrap_or(requested.clone());
        if let SourceData::Indexed(idx) = &*crate::analysis_runtime::source(&state) {
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
                })?
            }?;
            crate::operations::check()?;
            file.flush().map_err(|e| e.to_string())?;
            file.get_ref().sync_all().map_err(|e| e.to_string())?;
            drop(file);
            crate::operations::check()?;
            if let Some(admitted) = crate::analysis_runtime::current() { admitted.validate_visibility()?; }
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
                let text = path.to_string_lossy().into_owned();
                if archive_kind(&text).is_some() {
                    let members = archive_members(&path)?;
                    if members.is_empty() && explicit {
                        return Err(format!("Nenhum arquivo de log em {}.", path.display()));
                    }
                    files.extend(members);
                } else if explicit
                    || [
                        "log", "txt", "jsonl", "ndjson", "json", "csv", "tsv", "evtx", "gz", "audit", "xlsx", "xlsm",
                        "xlsb", "xls", "ods",
                    ]
                    .contains(&ext.as_str())
                    || ext.parse::<u32>().is_ok()
                {
                    files.push(text);
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
// Canonical conversions/extractions are immutable, completed generations. Their
// conversion itself restarts after interruption; metadata can resume only after
// this stage has published its checksummed completion marker.
pub(crate) use canonical::Input as CanonicalInput;

pub(crate) fn canonical_source_lease(path: &Path) -> Result<Option<std::sync::Arc<std::fs::File>>, String> {
    canonical::reader(path).map(|entry| entry.map(|(lease, _, _)| lease))
}
pub(crate) fn canonical_event_identity(path: &Path) -> Result<Option<String>, String> {
    canonical::reader(path).map(|entry| entry.map(|(_, artifact, _)| artifact.logical_identity))
}
/// Verify the ultimate input even when disposable intermediate conversions
/// were pruned. Physical payload validation remains a separate source guard.
pub(crate) fn validate_canonical_origin(path: &Path) -> Result<(), String> {
    canonical::original(path).map(|_| ())
}
pub(crate) fn canonical_original(path: &Path) -> Result<Option<(PathBuf, String)>, String> {
    canonical::original(path)
}
pub(crate) fn prune_canonical_sources() {
    canonical::prune(&canonical::root());
    canonical::prune_identity_pending();
}

mod canonical {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::{HashMap, VecDeque};
    use std::fs::{File, OpenOptions};
    use std::io::{Seek, SeekFrom};
    use std::sync::{Arc, LazyLock};
    use std::time::{Duration, Instant, SystemTime};

    const VERSION: u32 = 1;
    const MARKER: &str = ".complete.json";
    const MAX_MARKER: u64 = 32 << 20;
    const MEMO_BYTES: usize = 16 << 20;
    const LOCK_WAIT: Duration = Duration::from_secs(5);

    pub(super) fn progress(phase_id: &'static str, phase: &'static str, completed: usize, total: usize, unit: &'static str) {
        crate::operations::report_progress("carregamento", phase_id, phase, completed, total, unit, 0);
    }
    pub(super) struct ConversionWriter<'a> {
        pub(super) inner: &'a mut dyn Write,
        pub(super) bytes: usize,
        pub(super) last: Instant,
        pub(super) report: bool,
    }
    impl Write for ConversionWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let count = self.inner.write(bytes)?;
            self.bytes = self.bytes.saturating_add(count);
            if self.report && self.last.elapsed() >= Duration::from_millis(150) {
                progress("canonical-convert", "Convertendo fonte; interrupção reinicia esta etapa", self.bytes, 0, "bytes");
                self.last = Instant::now();
            }
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
    }

    pub(super) fn root() -> PathBuf { absolute(&crate::config_dir().join("expanded-v2")) }
    fn absolute(path: &Path) -> PathBuf {
        if path.is_absolute() { path.to_path_buf() } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    }
    fn identity_root() -> PathBuf { absolute(&crate::config_dir().join("source-identities-v1")) }
    fn valid_key(key: &str) -> bool { key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()) }
    fn relative_name(path: &Path) -> Result<String, String> {
        let text = path.to_str().ok_or("Nome de arquivo convertido não é UTF-8.")?;
        if text.is_empty() || text.len() > 4096 || path.is_absolute() || path.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err("Caminho inválido no cache de conversão.".into());
        }
        Ok(text.replace('\\', "/"))
    }
    fn regular(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
    }
    fn artifact_regular(dir: &Path, relative: &Path) -> bool {
        if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir() && !m.file_type().is_symlink()) { return false; }
        let mut current = dir.to_path_buf();
        for component in relative.components() {
            current.push(component.as_os_str());
            let Ok(metadata) = std::fs::symlink_metadata(&current) else { return false };
            if metadata.file_type().is_symlink() { return false; }
        }
        regular(&current)
    }
    fn sync_dir(path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        File::open(path).and_then(|file| file.sync_all()).map_err(|e| e.to_string())?;
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }
    fn lock_file(root: &Path, key: &str) -> Result<File, String> {
        let dir = root.join(".locks");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        OpenOptions::new().create(true).read(true).write(true).open(dir.join(format!("{key}.lock"))).map_err(|e| e.to_string())
    }
    fn lock(root: &Path, key: &str, exclusive: bool) -> Result<File, String> {
        let file = lock_file(root, key)?;
        let started = Instant::now();
        let mut reported = false;
        loop {
            crate::operations::check()?;
            let result = if exclusive { fs2::FileExt::try_lock_exclusive(&file) } else { fs2::FileExt::try_lock_shared(&file) };
            match result {
                Ok(()) => return Ok(file),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                    if !reported {
                        progress("canonical-lock", "Aguardando acesso à conversão", 0, 0, "bytes");
                        reported = true;
                    }
                    if started.elapsed() >= LOCK_WAIT {
                        return Err("Conversão em uso por outra leitura ou preparação; tente novamente após ela terminar.".into());
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct Stamp {
        bytes: u64,
        modified: Option<(u64, u32)>,
        native: Option<(u64, u64)>,
    }
    impl Stamp {
        fn of(file: &File) -> Result<Self, String> {
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if !metadata.is_file() { return Err("A fonte precisa ser um arquivo regular.".into()); }
            let modified = metadata.modified().ok().map(|t| {
                let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
                (d.as_secs(), d.subsec_nanos())
            });
            Ok(Self { bytes: metadata.len(), modified, native: sources::file_identity(file) })
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub(super) struct Origin {
        requested_path: String,
        canonical_path: String,
        stamp: Stamp,
        fingerprint: String,
    }
    fn capture(path: &Path, file: &File) -> Result<Origin, String> {
        let stamp = Stamp::of(file)?;
        let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| absolute(path)).to_string_lossy().into_owned();
        // Match the legacy source fingerprint, but obtain its size, time and
        // sampled bytes from the SAME opened file, without mapping mutable input.
        let mut hash = Sha256::new();
        hash.update(canonical_path.as_bytes());
        hash.update(stamp.bytes.to_le_bytes());
        if let Some((seconds, nanos)) = stamp.modified {
            hash.update((u128::from(seconds) * 1_000_000_000 + u128::from(nanos)).to_le_bytes());
        }
        let mut reader = file.try_clone().map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 65536];
        for offset in [0, stamp.bytes / 2, stamp.bytes.saturating_sub(65536)] {
            crate::operations::check()?;
            reader.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            let count = (stamp.bytes - offset).min(buffer.len() as u64) as usize;
            reader.read_exact(&mut buffer[..count]).map_err(|e| format!("Fonte alterada durante a leitura: {e}"))?;
            hash.update(&buffer[..count]);
        }
        if Stamp::of(file)? != stamp { return Err("A fonte mudou durante a leitura; reabra o arquivo.".into()); }
        Ok(Origin { requested_path: absolute(path).to_string_lossy().into_owned(), canonical_path, stamp, fingerprint: format!("{:x}", hash.finalize()) })
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub(super) struct LegacyAlias {
        pub(super) path: PathBuf,
        pub(super) identity: String,
    }
    pub(crate) struct Input {
        pub(crate) path: PathBuf,
        pub(crate) logical_identity: String,
        pub(super) legacy: Option<LegacyAlias>,
        file: File,
        origin: Origin,
        original: Origin,
        _lease: Option<Arc<File>>,
    }
    impl Input {
        pub(crate) fn open(path: &Path) -> Result<Self, String> {
            crate::operations::check()?;
            let canonical = reader(path)?;
            let file = crate::case_archive_format::open_regular(path).map_err(|e| format!("Não foi possível abrir {}: {e}", path.display()))?;
            let origin = capture(path, &file)?;
            let (logical_identity, legacy, original, lease) = match canonical {
                Some((lease, artifact, original)) => (artifact.logical_identity, artifact.legacy, original, Some(lease)),
                None => (origin.fingerprint.clone(), Some(LegacyAlias { path: path.to_path_buf(), identity: origin.fingerprint.clone() }), origin.clone(), None),
            };
            let input = Self { path: path.to_path_buf(), logical_identity, legacy, file, origin, original, _lease: lease };
            input.validate()?;
            Ok(input)
        }
        pub(crate) fn reader(&self) -> Result<File, String> {
            self.validate()?;
            let mut file = self.file.try_clone().map_err(|e| e.to_string())?;
            file.rewind().map_err(|e| e.to_string())?;
            Ok(file)
        }
        pub(crate) fn legacy_source(&self) -> Option<(PathBuf, String)> {
            self.legacy.as_ref().map(|alias| (alias.path.clone(), alias.identity.clone()))
        }
        fn validate(&self) -> Result<(), String> {
            crate::operations::check()?;
            let current = crate::case_archive_format::open_regular(&self.path).map_err(|e| format!("Fonte indisponível: {e}"))?;
            if capture(&self.path, &current)? != self.origin || Stamp::of(&self.file)? != self.origin.stamp {
                return Err("A fonte foi alterada durante a conversão; reabra uma cópia estável. A conversão incompleta será reiniciada.".into());
            }
            if self.original != self.origin { validate_original(&self.original)?; }
            Ok(())
        }
    }
    fn validate_original(original: &Origin) -> Result<(), String> {
        crate::operations::check()?;
        let path = Path::new(&original.requested_path);
        let file = crate::case_archive_format::open_regular(path).map_err(|_| "A fonte original da conversão não está disponível; reabra a fonte original.".to_string())?;
        if capture(path, &file)? != *original {
            return Err("A fonte original da conversão mudou; reabra a fonte antes de consultar ou calcular hashes.".into());
        }
        Ok(())
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub(super) struct Artifact {
        pub(super) path: String,
        bytes: u64,
        sha256: String,
        pub(super) logical_identity: String,
        legacy: Option<LegacyAlias>,
    }
    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    pub(super) struct Manifest {
        version: u32,
        key: String,
        converter: String,
        origin: Origin,
        original: Origin,
        pub(super) artifacts: Vec<Artifact>,
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Checked<T> {
        value: T,
        sha256: String,
    }
    fn checked_bytes<T: serde::Serialize>(value: T) -> Result<Vec<u8>, String> {
        let encoded = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        serde_json::to_vec(&Checked { value, sha256: format!("{:x}", Sha256::digest(encoded)) }).map_err(|e| e.to_string())
    }
    fn checked_read<T: serde::Serialize + serde::de::DeserializeOwned>(reader: impl Read) -> Option<T> {
        let checked: Checked<T> = serde_json::from_reader(reader).ok()?;
        let bytes = serde_json::to_vec(&checked.value).ok()?;
        (checked.sha256 == format!("{:x}", Sha256::digest(bytes))).then_some(checked.value)
    }
    struct Memo {
        path: PathBuf,
        marker_stamp: Stamp,
        encoded_bytes: usize,
        manifest: Arc<Manifest>,
        files: HashMap<String, Stamp>,
    }
    static MEMO: LazyLock<parking_lot::Mutex<VecDeque<Memo>>> = LazyLock::new(|| parking_lot::Mutex::new(VecDeque::new()));

    fn hash_file(path: &Path) -> Result<(Stamp, String), String> {
        if !regular(path) { return Err("Artefato convertido ausente ou não regular.".into()); }
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let before = Stamp::of(&file)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut completed = 0usize;
        let total = usize::try_from(before.bytes).unwrap_or(usize::MAX);
        let mut reported = Instant::now();
        progress("canonical-verify", "Verificando integridade da conversão", 0, total, "bytes");
        loop {
            crate::operations::check()?;
            let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 { break; }
            hash.update(&buffer[..read]);
            completed = completed.saturating_add(read);
            if reported.elapsed() >= Duration::from_millis(150) {
                progress("canonical-verify", "Verificando integridade da conversão", completed, total, "bytes");
                reported = Instant::now();
            }
        }
        progress("canonical-verify", "Verificando integridade da conversão", completed, total, "bytes");
        let current = File::open(path).map_err(|e| e.to_string())?;
        if Stamp::of(&file)? != before || Stamp::of(&current)? != before {
            return Err("Artefato convertido mudou durante a validação.".into());
        }
        Ok((before, format!("{:x}", hash.finalize())))
    }
    fn manifest(dir: &Path) -> Result<Option<Arc<Manifest>>, String> {
        let marker = dir.join(MARKER);
        if !artifact_regular(dir, Path::new(MARKER)) { return Ok(None); }
        let file = File::open(&marker).map_err(|e| e.to_string())?;
        let stamp = Stamp::of(&file)?;
        if stamp.bytes > MAX_MARKER { return Ok(None); }
        if let Some(hit) = MEMO.lock().iter().find(|entry| entry.path == dir && entry.marker_stamp == stamp) {
            return Ok(Some(Arc::clone(&hit.manifest)));
        }
        let Some(value) = checked_read::<Manifest>(std::io::BufReader::new(file)) else { return Ok(None) };
        let encoded = serde_json::to_vec(&(VERSION, &value.converter, &value.origin, &value.original)).map_err(|e| e.to_string())?;
        if value.key != format!("{:x}", Sha256::digest(encoded)) { return Ok(None); }
        if value.version != VERSION || dir.file_name().and_then(|n| n.to_str()) != Some(value.key.as_str())
            || !valid_key(&value.key) || value.artifacts.len() > ARCHIVE_MEMBERS {
            return Ok(None);
        }
        let mut names = std::collections::HashSet::new();
        for artifact in &value.artifacts {
            if relative_name(Path::new(&artifact.path)).ok().as_deref() != Some(artifact.path.as_str())
                || !valid_key(&artifact.sha256) || !valid_key(&artifact.logical_identity)
                || artifact.legacy.as_ref().is_some_and(|alias| alias.identity != artifact.logical_identity || alias.path.to_string_lossy().len() > 4096)
                || !names.insert(&artifact.path) { return Ok(None); }
        }
        let manifest = Arc::new(value);
        let mut memo = MEMO.lock();
        memo.retain(|entry| entry.path != dir);
        if stamp.bytes as usize <= MEMO_BYTES {
            while memo.iter().map(|entry| entry.encoded_bytes).sum::<usize>() + stamp.bytes as usize > MEMO_BYTES {
                memo.pop_front();
            }
            memo.push_back(Memo { path: dir.to_path_buf(), marker_stamp: stamp.clone(), encoded_bytes: stamp.bytes as usize, manifest: Arc::clone(&manifest), files: HashMap::new() });
        }
        Ok(Some(manifest))
    }
    fn artifact_valid(dir: &Path, artifact: &Artifact) -> Result<bool, String> {
        let path = dir.join(&artifact.path);
        if !artifact_regular(dir, Path::new(&artifact.path)) { return Ok(false); }
        let file = File::open(&path).map_err(|e| e.to_string())?;
        let stamp = Stamp::of(&file)?;
        if stamp.bytes != artifact.bytes { return Ok(false); }
        if MEMO.lock().iter().find(|entry| entry.path == dir).and_then(|entry| entry.files.get(&artifact.path)) == Some(&stamp) {
            return Ok(true);
        }
        let (actual, sha256) = hash_file(&path)?;
        if actual.bytes != artifact.bytes || sha256 != artifact.sha256 { return Ok(false); }
        if let Some(entry) = MEMO.lock().iter_mut().find(|entry| entry.path == dir) {
            entry.files.insert(artifact.path.clone(), actual);
        }
        Ok(true)
    }
    fn location(path: &Path) -> Result<Option<(PathBuf, String)>, String> {
        let root = root();
        let lexical = (absolute(path), root.clone());
        let resolved = std::fs::canonicalize(path).ok().zip(std::fs::canonicalize(&root).ok());
        for (path, root) in std::iter::once(lexical).chain(resolved) {
            let Ok(relative) = path.strip_prefix(&root) else { continue };
            let mut parts = relative.components();
            let key = parts.next().and_then(|part| part.as_os_str().to_str()).ok_or("Caminho de conversão inválido.")?;
            if !valid_key(key) { return Err("Conversão ainda não publicada; reabra a fonte original.".into()); }
            return Ok(Some((root.join(key), relative_name(parts.as_path())?)));
        }
        Ok(None)
    }
    pub(super) fn reader(path: &Path) -> Result<Option<(Arc<File>, Artifact, Origin)>, String> {
        let Some((dir, relative)) = location(path)? else { return Ok(None) };
        let key = dir.file_name().and_then(|n| n.to_str()).ok_or("Cache inválido.")?;
        let lease = Arc::new(lock(dir.parent().ok_or("Cache inválido.")?, key, false)?);
        let manifest = manifest(&dir)?.ok_or("Conversão incompleta; reabra a fonte para regenerá-la.")?;
        let artifact = manifest.artifacts.iter().find(|artifact| artifact.path == relative).ok_or("Arquivo não pertence à conversão concluída.")?;
        if !artifact_valid(&dir, artifact)? { return Err("Conversão corrompida; reabra a fonte para regenerá-la.".into()); }
        touch(&dir);
        Ok(Some((lease, artifact.clone(), manifest.original.clone())))
    }
    pub(super) fn original(path: &Path) -> Result<Option<(PathBuf, String)>, String> {
        let Some((_lease, _artifact, original)) = reader(path)? else { return Ok(None) };
        validate_original(&original)?;
        let generation = format!("{:x}", Sha256::digest(serde_json::to_vec(&original).map_err(|e| e.to_string())?));
        Ok(Some((PathBuf::from(original.requested_path), generation)))
    }
    fn touch(dir: &Path) {
        let _ = OpenOptions::new().create(true).append(true).open(dir.join(".used"))
            .and_then(|file| file.set_modified(SystemTime::now()));
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct IdentityRecord {
        version: u32,
        key: String,
        sha256: String,
        identity: String,
        legacy: Option<LegacyAlias>,
    }
    fn logical_identity(input: &Input, converter: &str, name: &str, sha256: &str, bytes: u64, legacy: Option<&Path>) -> Result<(String, Option<LegacyAlias>), String> {
        let key = format!("{:x}", Sha256::digest(format!("logical-v1|{}|{}|{converter}|{name}|{sha256}", input.logical_identity, input.original.requested_path).as_bytes()));
        let root = identity_root();
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let _lock = lock(&root, &key, true)?;
        let target = root.join(format!("{key}.json"));
        if regular(&target) {
            let file = File::open(&target).map_err(|e| e.to_string())?;
            if file.metadata().map_err(|e| e.to_string())?.len() <= 16384 {
                if let Some(record) = checked_read::<IdentityRecord>(file) {
                    if record.version == VERSION && record.key == key && record.sha256 == sha256 && valid_key(&record.identity)
                        && record.legacy.as_ref().is_none_or(|alias| alias.identity == record.identity && alias.path.to_string_lossy().len() <= 4096) {
                        return Ok((record.identity, record.legacy));
                    }
                }
            }
            // This mapping preserves saved references and is not a disposable
            // cache. Never silently replace a corrupt durable identity record.
            return Err("Registro de identidade da fonte convertido está inválido; preserve-o para recuperação antes de tentar novamente.".into());
        }
        let alias = if let Some(legacy_path) = legacy.filter(|path| regular(path)) {
            match hash_file(legacy_path) {
                Ok((stamp, digest)) if stamp.bytes == bytes && digest == sha256 => {
                    let file = File::open(legacy_path).map_err(|e| e.to_string())?;
                    let old = capture(legacy_path, &file)?;
                    if old.stamp == stamp { Some(LegacyAlias { path: legacy_path.to_path_buf(), identity: old.fingerprint }) } else { None }
                }
                Err(error) if crate::operations::cancelled() => return Err(error),
                _ => None,
            }
        } else { None };
        let identity = alias.as_ref().map(|alias| alias.identity.clone()).unwrap_or_else(|| key.clone());
        let record = IdentityRecord { version: VERSION, key: key.clone(), sha256: sha256.into(), identity: identity.clone(), legacy: alias.clone() };
        let bytes = checked_bytes(record)?;
        if bytes.len() > 16384 { return Err("Registro de identidade excedeu o limite.".into()); }
        let mut pending = tempfile::Builder::new().prefix(&format!("{key}.pending-")).tempfile_in(&root).map_err(|e| e.to_string())?;
        pending.write_all(&bytes).and_then(|_| pending.as_file().sync_all()).map_err(|e| e.to_string())?;
        crate::operations::check()?;
        pending.persist_noclobber(&target).map_err(|e| e.error.to_string())?;
        sync_dir(&root)?;
        Ok((identity, alias))
    }

    pub(super) struct Store {
        pub(super) input: Input,
        pub(super) converter: String,
        pub(super) root: PathBuf,
        pub(super) dir: PathBuf,
        key: String,
    }
    impl Store {
        pub(super) fn new(input: Input, converter: &str) -> Result<Self, String> {
            let encoded = serde_json::to_vec(&(VERSION, converter, &input.origin, &input.original)).map_err(|e| e.to_string())?;
            let key = format!("{:x}", Sha256::digest(encoded));
            let root = root();
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let dir = root.join(&key);
            Ok(Self { input, converter: converter.into(), root, dir, key })
        }
        pub(super) fn lock(&self, exclusive: bool) -> Result<File, String> { lock(&self.root, &self.key, exclusive) }
        pub(super) fn ready(&self, only: Option<&str>) -> Result<Option<Arc<Manifest>>, String> {
            self.input.validate()?;
            let Some(manifest) = manifest(&self.dir)? else { return Ok(None) };
            if manifest.origin != self.input.origin || manifest.original != self.input.original || manifest.converter != self.converter { return Ok(None); }
            let requested: Vec<&Artifact> = match only {
                Some(name) => match manifest.artifacts.iter().find(|artifact| artifact.path == name) { Some(artifact) => vec![artifact], None => return Ok(None) },
                None => manifest.artifacts.iter().collect(),
            };
            for artifact in requested { if !artifact_valid(&self.dir, artifact)? { return Ok(None); } }
            self.input.validate()?;
            touch(&self.dir);
            Ok(Some(manifest))
        }
        pub(super) fn staging(&self) -> Result<tempfile::TempDir, String> {
            tempfile::Builder::new().prefix(&format!("{}.pending-", self.key)).tempdir_in(&self.root).map_err(|e| e.to_string())
        }
        pub(super) fn artifact(&self, staging: &Path, name: &Path, legacy: Option<&Path>) -> Result<Artifact, String> {
            let name = relative_name(name)?;
            self.input.validate()?;
            let (stamp, sha256) = hash_file(&staging.join(&name))?;
            self.input.validate()?;
            let (logical_identity, legacy) = logical_identity(&self.input, &self.converter, &name, &sha256, stamp.bytes, legacy)?;
            Ok(Artifact { path: name, bytes: stamp.bytes, sha256, logical_identity, legacy })
        }
        pub(super) fn publish(&self, staging: tempfile::TempDir, artifacts: Vec<Artifact>) -> Result<(), String> {
            progress("canonical-publish", "Publicando conversão validada", 0, 0, "bytes");
            self.input.validate()?;
            crate::operations::check()?;
            let manifest = Manifest { version: VERSION, key: self.key.clone(), converter: self.converter.clone(), origin: self.input.origin.clone(), original: self.input.original.clone(), artifacts };
            // Directory entries must be durable before the completion marker.
            let mut directories = std::collections::BTreeSet::new();
            for artifact in &manifest.artifacts {
                let path = staging.path().join(&artifact.path);
                let mut parent = path.parent();
                while let Some(dir) = parent.filter(|dir| dir.starts_with(staging.path())) {
                    directories.insert(dir.to_path_buf());
                    parent = dir.parent();
                }
            }
            for dir in directories.iter().rev() { sync_dir(dir)?; }
            let encoded = checked_bytes(manifest)?;
            if encoded.len() as u64 > MAX_MARKER { return Err("Manifesto da conversão excedeu o limite.".into()); }
            let mut file = OpenOptions::new().write(true).create_new(true).open(staging.path().join(MARKER)).map_err(|e| e.to_string())?;
            file.write_all(&encoded).and_then(|_| file.sync_all()).map_err(|e| format!("Falha ao gravar o manifesto da conversão: {e}"))?;
            // Windows cannot rename a directory while a child file is open.
            // The marker is durable; close its handle before publication.
            drop(file);
            sync_dir(staging.path())?;
            self.input.validate()?;
            crate::operations::check()?;
            // Caller owns the exclusive lease, so no mapped reader or other
            // process can observe an invalid generation being replaced.
            if self.dir.exists() { std::fs::remove_dir_all(&self.dir).map_err(|e| format!("Falha ao remover a conversão inválida: {e}"))?; }
            MEMO.lock().retain(|entry| entry.path != self.dir);
            std::fs::rename(staging.path(), &self.dir).map_err(|e| format!("Falha ao publicar a conversão validada: {e}"))?;
            sync_dir(&self.root)?;
            touch(&self.dir);
            Ok(())
        }
    }
    pub(super) fn prune_identity_pending() {
        let root = identity_root();
        let Ok(entries) = std::fs::read_dir(&root) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((key, _)) = name.split_once(".pending-") else { continue };
            if !valid_key(key) || !entry.file_type().is_ok_and(|kind| kind.is_file()) { continue; }
            let Ok(file) = lock_file(&root, key) else { continue };
            if fs2::FileExt::try_lock_exclusive(&file).is_ok() { let _ = std::fs::remove_file(entry.path()); }
        }
    }
    pub(super) fn prune(root: &Path) { prune_with(root, &|_| {}); }
    fn stale(path: &Path, cutoff: SystemTime) -> bool {
        std::fs::metadata(path.join(".used")).or_else(|_| std::fs::metadata(path)).ok()
            .and_then(|metadata| metadata.modified().ok()).is_some_and(|time| time < cutoff)
    }
    fn prune_with(root: &Path, before_lock: &dyn Fn(&Path)) {
        let Ok(entries) = std::fs::read_dir(root) else { return };
        let cutoff = SystemTime::now().checked_sub(Duration::from_secs(30 * 24 * 3600)).unwrap_or(SystemTime::UNIX_EPOCH);
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let key = name.split_once(".pending-").map(|(key, _)| key).unwrap_or(&name);
            if !valid_key(key) || !entry.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()) { continue; }
            let path = entry.path();
            let pending = name.contains(".pending-");
            if !pending && !stale(&path, cutoff) { continue; }
            before_lock(&path);
            let Ok(file) = lock_file(root, key) else { continue };
            if fs2::FileExt::try_lock_exclusive(&file).is_err() { continue; }
            if !pending && !stale(&path, cutoff) { continue; }
            let _ = std::fs::remove_dir_all(&path);
            MEMO.lock().retain(|entry| entry.path != path);
        }
        // source-identities-v1 is intentionally durable. Its bounded records
        // are read/updated individually and never evicted with the payloads.
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static ENV: parking_lot::Mutex<()> = parking_lot::const_mutex(());
        struct Fixture {
            dir: tempfile::TempDir,
            previous: Option<std::ffi::OsString>,
            _guard: parking_lot::MutexGuard<'static, ()>,
        }
        impl Fixture {
            fn new() -> Self {
                let guard = ENV.lock();
                let dir = tempfile::tempdir().unwrap();
                let previous = std::env::var_os("LOGINSIGHT_DATA_DIR");
                std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path().join("data"));
                Self { dir, previous, _guard: guard }
            }
            fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
                let path = self.dir.path().join(name);
                std::fs::write(&path, bytes).unwrap();
                path
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                MEMO.lock().clear();
                match &self.previous {
                    Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                    None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
                }
            }
        }
        fn convert(path: &Path, legacy: Option<PathBuf>, calls: &AtomicUsize) -> Result<PathBuf, String> {
            super::super::canonical_file(Input::open(path)?, "fixture-v1", legacy, |input, out| {
                calls.fetch_add(1, Ordering::SeqCst);
                std::io::copy(&mut input.reader()?, out).map_err(|e| e.to_string())?;
                Ok(())
            })
        }
        fn legacy_file(fixture: &Fixture, name: &str, bytes: &[u8]) -> PathBuf {
            let root = crate::config_dir().join("expanded");
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join(name);
            std::fs::write(&path, bytes).unwrap();
            assert!(path.starts_with(fixture.dir.path()));
            path
        }
        fn old_identity(path: &Path) -> String {
            crate::index_cache::identity(path.to_str().unwrap(), &std::fs::read(path).unwrap())
        }
        fn canonical_directories() -> Vec<PathBuf> {
            std::fs::read_dir(root()).unwrap().flatten().filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                valid_key(&name) || name.contains(".pending-")
            }).map(|entry| entry.path()).collect()
        }

        #[test]
        fn opened_source_fingerprint_matches_legacy_and_replacement_changes_physical_key() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\ntwo\n");
            let input = Input::open(&path).unwrap();
            assert_eq!(input.logical_identity, old_identity(&path));
            let first = Store::new(input, "fixture-v1").unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            let replacement = fixture.file("replacement.log", b"one\ntwo\n");
            OpenOptions::new().read(true).write(true).open(&replacement).unwrap().set_modified(modified).unwrap();
            // On Windows the held original file can prevent replacement, so
            // test the generation comparison directly with the second handle.
            #[cfg(unix)]
            {
                std::fs::rename(&replacement, &path).unwrap();
                assert!(first.input.validate().is_err());
                let second = Store::new(Input::open(&path).unwrap(), "fixture-v1").unwrap();
                assert_ne!(first.key, second.key);
                assert_eq!(first.input.logical_identity, second.input.logical_identity);
            }
            #[cfg(not(unix))]
            assert_ne!(first.input.origin.stamp.native, Stamp::of(&File::open(&replacement).unwrap()).unwrap().native);
        }

        #[test]
        fn complete_conversion_reuses_and_corrupt_payload_or_marker_is_rebuilt() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\ntwo\n");
            let calls = AtomicUsize::new(0);
            let target = convert(&path, None, &calls).unwrap();
            assert_eq!(convert(&path, None, &calls).unwrap(), target);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            std::fs::write(&target, b"bad\nbad\n").unwrap();
            assert_eq!(convert(&path, None, &calls).unwrap(), target);
            assert_eq!(std::fs::read(&target).unwrap(), b"one\ntwo\n");
            std::fs::write(target.parent().unwrap().join(MARKER), b"{partial").unwrap();
            convert(&path, None, &calls).unwrap();
            std::fs::remove_file(&target).unwrap();
            convert(&path, None, &calls).unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        }

        #[test]
        fn source_mutation_and_cancellation_never_publish_partial_conversions() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\n");
            let result = super::super::canonical_file(Input::open(&path).unwrap(), "fixture-v1", None, |_, out| {
                out.write_all(b"one\n").unwrap();
                std::fs::write(&path, b"different source\n").unwrap();
                Ok(())
            });
            assert!(result.is_err());
            assert!(canonical_directories().is_empty());
            let id = format!("canonical-cancel-{}", uuid::Uuid::new_v4());
            let token = crate::operations::token(Some(id.clone())).unwrap();
            let result = crate::operations::run_with_token(token, || {
                super::super::canonical_file(Input::open(&path).unwrap(), "fixture-v1", None, |_, out| {
                    out.write_all(b"incomplete").unwrap();
                    assert!(crate::operations::cancel_id(&id));
                    Ok(())
                })
            });
            assert!(result.is_err());
            assert!(canonical_directories().is_empty());
            let calls = AtomicUsize::new(0);
            let target = convert(&path, None, &calls).unwrap();
            assert_eq!(std::fs::read(target).unwrap(), b"different source\n");
            assert_eq!(calls.load(Ordering::SeqCst), 1, "conversion restarts, rather than seeking into partial output");
        }

        #[test]
        fn migrated_identity_survives_prune_and_legacy_payload_removal() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\ntwo\n");
            let legacy = legacy_file(&fixture, "old-expanded.log", b"one\ntwo\n");
            let expected = old_identity(&legacy);
            let calls = AtomicUsize::new(0);
            let target = convert(&path, Some(legacy.clone()), &calls).unwrap();
            assert_eq!(super::super::canonical_event_identity(&target).unwrap(), Some(expected.clone()));
            let lease = super::super::canonical_source_lease(&target).unwrap().unwrap();
            let used = target.parent().unwrap().join(".used");
            OpenOptions::new().read(true).write(true).open(&used).unwrap().set_modified(SystemTime::UNIX_EPOCH).unwrap();
            prune(&root());
            assert!(target.exists(), "a mapped reader lease blocks pruning");
            drop(lease);
            prune(&root());
            assert!(!target.exists());
            std::fs::remove_file(&legacy).unwrap();
            let rebuilt = convert(&path, Some(legacy), &calls).unwrap();
            assert_eq!(super::super::canonical_event_identity(&rebuilt).unwrap(), Some(expected));
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert!(std::fs::read_dir(identity_root()).unwrap().flatten().any(|entry| entry.path().extension().is_some_and(|ext| ext == "json")));
        }

        #[test]
        fn pruning_rechecks_recent_reuse_after_acquiring_its_exclusive_lease() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\n");
            let target = convert(&path, None, &AtomicUsize::new(0)).unwrap();
            OpenOptions::new().read(true).write(true).open(target.parent().unwrap().join(".used")).unwrap().set_modified(SystemTime::UNIX_EPOCH).unwrap();
            let store = Store::new(Input::open(&path).unwrap(), "fixture-v1").unwrap();
            let reused = AtomicUsize::new(0);
            prune_with(&root(), &|dir| {
                if dir == store.dir {
                    let _lease = store.lock(false).unwrap();
                    assert!(store.ready(Some("payload")).unwrap().is_some());
                    reused.fetch_add(1, Ordering::SeqCst);
                }
            });
            assert_eq!(reused.load(Ordering::SeqCst), 1);
            assert!(target.exists(), "the fresh warm-load touch supersedes prune's earlier stale observation");
        }

        #[test]
        fn corrupt_legacy_artifact_is_never_adopted_and_identity_record_is_not_overwritten() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\ntwo\n");
            let legacy = legacy_file(&fixture, "old-expanded.log", b"bad\nbad\n");
            let wrong = old_identity(&legacy);
            let calls = AtomicUsize::new(0);
            let target = convert(&path, Some(legacy.clone()), &calls).unwrap();
            assert_ne!(super::super::canonical_event_identity(&target).unwrap(), Some(wrong));
            let mapping = std::fs::read_dir(identity_root()).unwrap().flatten().map(|entry| entry.path()).find(|path| path.extension().is_some_and(|ext| ext == "json")).unwrap();
            std::fs::write(&mapping, b"{damaged identity}").unwrap();
            std::fs::remove_dir_all(target.parent().unwrap()).unwrap();
            let error = convert(&path, Some(legacy), &calls).unwrap_err();
            assert!(error.contains("identidade"));
            assert_eq!(std::fs::read(mapping).unwrap(), b"{damaged identity}");
        }

        #[test]
        fn concurrent_publishers_build_once_and_pending_cleanup_respects_locks() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\ntwo\n");
            let calls = AtomicUsize::new(0);
            std::thread::scope(|scope| {
                let jobs: Vec<_> = (0..4).map(|_| scope.spawn(|| convert(&path, None, &calls).unwrap())).collect();
                let paths: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
                assert!(paths.windows(2).all(|pair| pair[0] == pair[1]));
            });
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let store = Store::new(Input::open(&path).unwrap(), "unfinished-v1").unwrap();
            let writer = store.lock(true).unwrap();
            let pending = store.staging().unwrap();
            prune(&root());
            assert!(pending.path().exists());
            drop(writer);
            prune(&root());
            assert!(!pending.path().exists());
        }

        #[test]
        fn durable_identity_updates_are_serialized_and_pending_records_are_pruned_safely() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\n");
            let input = Input::open(&path).unwrap();
            let legacy = legacy_file(&fixture, "old-expanded.log", b"one\n");
            let expected = old_identity(&legacy);
            let hash = format!("{:x}", Sha256::digest(b"one\n"));
            std::thread::scope(|scope| {
                let jobs: Vec<_> = (0..4).map(|_| scope.spawn(|| logical_identity(&input, "fixture-v1", "payload", &hash, 4, Some(&legacy)).unwrap())).collect();
                for job in jobs { assert_eq!(job.join().unwrap().0, expected); }
            });
            let records: Vec<_> = std::fs::read_dir(identity_root()).unwrap().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == "json")).collect();
            assert_eq!(records.len(), 1);
            assert!(std::fs::metadata(&records[0]).unwrap().len() <= 16384);
            let key = records[0].file_stem().unwrap().to_str().unwrap();
            let active = lock(&identity_root(), key, true).unwrap();
            let pending = tempfile::Builder::new().prefix(&format!("{key}.pending-")).tempfile_in(identity_root()).unwrap();
            prune_identity_pending();
            assert!(pending.path().exists());
            drop(active);
            prune_identity_pending();
            assert!(!pending.path().exists());
            assert!(records[0].exists(), "logical mappings are never ordinary cache-pruned");
        }

        #[test]
        fn ordinary_source_needs_no_canonical_directory_or_lease() {
            let fixture = Fixture::new();
            let path = fixture.file("plain.log", b"one\n");
            assert!(!root().exists());
            assert!(super::super::canonical_source_lease(&path).unwrap().is_none());
            assert!(super::super::canonical_event_identity(&path).unwrap().is_none());
            assert!(!root().exists());
        }

        #[test]
        #[cfg(unix)]
        fn cached_symlink_is_rejected_without_touching_its_external_target() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\n");
            let private = fixture.file("private.txt", b"do not replace");
            let calls = AtomicUsize::new(0);
            let target = convert(&path, None, &calls).unwrap();
            std::fs::remove_file(&target).unwrap();
            std::os::unix::fs::symlink(&private, &target).unwrap();
            assert!(super::super::canonical_source_lease(&target).is_err());
            convert(&path, None, &calls).unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), b"one\n");
            assert_eq!(std::fs::read(&private).unwrap(), b"do not replace");
        }

        #[test]
        fn cross_process_reader_lock_child() {
            let Some(path) = std::env::var_os("LOGINSIGHT_CANONICAL_LOCK_TEST") else { return };
            let file = OpenOptions::new().read(true).write(true).open(path).unwrap();
            assert!(fs2::FileExt::try_lock_exclusive(&file).is_err());
        }

        #[test]
        fn reader_lease_blocks_another_process() {
            let fixture = Fixture::new();
            let path = fixture.file("source.log", b"one\n");
            let target = convert(&path, None, &AtomicUsize::new(0)).unwrap();
            let lease = super::super::canonical_source_lease(&target).unwrap().unwrap();
            let key = target.parent().unwrap().file_name().unwrap();
            let lock_path = root().join(".locks").join(format!("{}.lock", key.to_string_lossy()));
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "workspace::canonical::tests::cross_process_reader_lock_child", "--test-threads=1"])
                .env("LOGINSIGHT_CANONICAL_LOCK_TEST", lock_path).status().unwrap();
            assert!(status.success());
            drop(lease);
        }

        #[test]
        fn gzip_encoding_chain_keeps_validated_legacy_reference_and_display_identity() {
            let fixture = Fixture::new();
            let text = "2026-09-30 12:00:00 INFO autenticação\n";
            let utf16: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&utf16).unwrap();
            let path = fixture.file("events.log.gz", &encoder.finish().unwrap());
            let raw_id = old_identity(&path);
            let old_gzip = legacy_file(&fixture, &format!("{}-events.log", &raw_id[..16]), &utf16);
            let gzip_id = old_identity(&old_gzip);
            let old_utf8 = legacy_file(&fixture, &format!("{}-utf8-{}", &gzip_id[..16], old_gzip.file_name().unwrap().to_string_lossy()), text.as_bytes());
            let expected = old_identity(&old_utf8);
            let idx = crate::index_source_file(path.to_str().unwrap(), "auto", None).unwrap();
            let codes = CodesConfig::default();
            let event = sources::event_at(&idx, 0, &codes, &codes, &[]);
            assert_eq!(event.event_ref, format!("{expected}:0"));
            assert_eq!(event.fields["arquivo"], "events.log.gz");
            assert_eq!(event.fields["caminho"], path.to_string_lossy().as_ref());
        }

        #[test]
        fn utf32_signatures_do_not_publish_a_misdecoded_utf16_artifact() {
            let fixture = Fixture::new();
            for (name, bytes) in [
                ("UTF-32LE", vec![0xff, 0xfe, 0, 0, b'A', 0, 0, 0, b'\n', 0, 0, 0]),
                ("UTF-32BE", vec![0, 0, 0xfe, 0xff, 0, 0, 0, b'A', 0, 0, 0, b'\n']),
            ] {
                let path = fixture.file("events.log", &bytes);
                assert!(super::super::sniff_encoding(&bytes).is_none(), "unsupported signatures cannot be reported as UTF-16");
                let error = super::super::expand_encoding(&path).unwrap_err();
                assert!(error.contains(name));
                assert!(error.contains("UTF-8 ou UTF-16"));
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert!(!root().exists(), "reject before publishing any canonical output");
            }
            let ambiguous = [0xff, 0xfe, 0, 0, b'A', 0, b'\n', 0];
            let path = fixture.file("utf16-leading-null.log", &ambiguous);
            let error = super::super::expand_encoding(&path).unwrap_err();
            assert!(error.contains("UTF-16LE com NUL inicial"));
            assert_eq!(std::fs::read(path).unwrap(), ambiguous);
            assert!(!root().exists());
            // The same path can be corrected and retried without a stale output.
            let text = "Falha de autenticação\r\n";
            let utf16: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
            let path = fixture.file("events.log", &utf16);
            let converted = super::super::expand_encoding(&path).unwrap().unwrap();
            assert_eq!(std::fs::read_to_string(converted).unwrap(), text);
            assert_eq!(std::fs::read(path).unwrap(), utf16);
        }

        #[test]
        fn supported_encoding_signatures_keep_the_existing_detection() {
            assert_eq!(super::super::sniff_encoding(&[0xff, 0xfe, b'A', 0]).map(|encoding| encoding.name()), Some("UTF-16LE"));
            assert_eq!(super::super::sniff_encoding(&[0xfe, 0xff, 0, b'A']).map(|encoding| encoding.name()), Some("UTF-16BE"));
            assert!(super::super::sniff_encoding("\u{feff}ação\n".as_bytes()).is_none());
            assert_eq!(super::super::sniff_encoding(b"a\xe7\xe3o\n").map(|encoding| encoding.name()), Some("windows-1252"));
            assert_eq!(super::super::unsupported_encoding_bom(&[0xff, 0xfe]), None);
        }

        #[cfg(unix)]
        #[test]
        fn source_inputs_and_replacements_decline_non_regular_files() {
            use std::os::unix::{ffi::OsStrExt, fs::symlink};
            const INPUT: &str = "LOGINSIGHT_TEST_SOURCE_NONREGULAR_ROOT";
            if let Some(path) = std::env::var_os(INPUT) {
                let directory = PathBuf::from(path);
                std::env::set_var("LOGINSIGHT_DATA_DIR", directory.join("data"));
                let fifo = |path: &Path| {
                    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                };
                let input_path = directory.join("input.jsonl");
                fifo(&input_path);
                assert!(Input::open(&input_path).err().unwrap().contains("regular"));
                assert!(super::super::expand_encoding(&input_path).unwrap_err().contains("regular"));
                assert!(crate::sources::prepare_index(input_path.to_str().unwrap(), "jsonl", None, None).err().unwrap().contains("regular"));

                let path = directory.join("source.jsonl");
                std::fs::write(&path, b"{\"message\":\"original\"}\n").unwrap();
                let input = Input::open(&path).unwrap();
                let original = input.original.clone();
                let prepared = crate::sources::prepare_index(path.to_str().unwrap(), "jsonl", None, None).unwrap();
                let moved = directory.join("source-kept.jsonl");
                std::fs::rename(&path, &moved).unwrap();
                fifo(&path);
                assert!(input.validate().unwrap_err().contains("regular"));
                assert!(validate_original(&original).is_err());
                assert!(prepared.validate().unwrap_err().contains("regular"));
                assert!(crate::sources::validate_source(&prepared.part).unwrap_err().contains("regular"));
                let link = directory.join("regular-link.jsonl");
                symlink(&moved, &link).unwrap();
                let index = crate::index_source_file(link.to_str().unwrap(), "jsonl", None).unwrap();
                assert_eq!(index.lines.len(), 1, "regular symlink targets remain supported");
                assert_eq!(std::fs::read(&moved).unwrap(), b"{\"message\":\"original\"}\n");

                let id = format!("source-open-cancel-{}", uuid::Uuid::new_v4());
                let token = crate::operations::token(Some(id.clone())).unwrap();
                assert!(crate::operations::run_with_token(token, || {
                    assert!(crate::operations::cancel_id(&id));
                    assert!(Input::open(&directory.join("missing")).err().unwrap().contains("cancelada"));
                }).is_err());
                return;
            }
            let directory = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "workspace::canonical::tests::source_inputs_and_replacements_decline_non_regular_files", "--test-threads=1"])
                .env(INPUT, directory.path()).stdout(std::process::Stdio::null()).spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() { assert!(status.success()); break; }
                if Instant::now() >= deadline {
                    let _ = child.kill(); let _ = child.wait();
                    panic!("source open or revalidation blocked on a non-regular input");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn archive_rejects_missing_or_corrupt_members_and_preserves_virtual_paths() {
            let fixture = Fixture::new();
            let archive = fixture.dir.path().join("logs.zip");
            let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
            zip.start_file("folder/events.log", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"one\ntwo\n").unwrap();
            zip.finish().unwrap();
            let members = super::super::archive_members(&archive).unwrap();
            assert_eq!(members, vec![format!("{}!/folder/events.log", archive.display())]);
            let target = super::super::resolve_member(&members[0]).unwrap().unwrap();
            std::fs::write(&target, b"bad\nbad\n").unwrap();
            let recovered = super::super::resolve_member(&members[0]).unwrap().unwrap();
            assert_eq!(std::fs::read(&recovered).unwrap(), b"one\ntwo\n");
            std::fs::remove_file(&recovered).unwrap();
            assert_eq!(super::super::archive_members(&archive).unwrap(), members);
            assert_eq!(std::fs::read(super::super::resolve_member(&members[0]).unwrap().unwrap()).unwrap(), b"one\ntwo\n");
        }

        #[test]
        #[cfg(unix)]
        fn workbook_timezone_child() {
            if let Some(path) = std::env::var_os("LOGINSIGHT_CANONICAL_TZ_CHANGE_INPUT") {
                let rejected = crate::spreadsheet::expand(Path::new(&path), &|_| std::env::set_var("TZ", "Pacific/Honolulu")).is_err();
                std::fs::write(std::env::var_os("LOGINSIGHT_CANONICAL_TZ_OUTPUT").unwrap(), serde_json::to_vec(&rejected).unwrap()).unwrap();
                return;
            }
            let Some(path) = std::env::var_os("LOGINSIGHT_CANONICAL_TZ_INPUT") else { return };
            let output = crate::spreadsheet::expand(Path::new(&path), &|_| {}).unwrap().unwrap();
            let text = std::fs::read_to_string(&output).unwrap();
            let events: Vec<Event> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            let result = serde_json::json!({"path":output,"offset":chrono::Local::now().offset().to_string(),"times":events.iter().map(|event| event.timestamp).collect::<Vec<_>>()});
            std::fs::write(std::env::var_os("LOGINSIGHT_CANONICAL_TZ_OUTPUT").unwrap(), serde_json::to_vec(&result).unwrap()).unwrap();
        }

        #[test]
        #[cfg(unix)]
        fn workbook_cache_distinguishes_same_current_offset_with_different_historical_zones() {
            let fixture = Fixture::new();
            let path = fixture.dir.path().join("historical.xlsx");
            let mut book = rust_xlsxwriter::Workbook::new();
            let sheet = book.add_worksheet();
            sheet.write_string(0, 0, "Timestamp").unwrap();
            sheet.write_string(0, 1, "Message").unwrap();
            let format = rust_xlsxwriter::Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
            for (row, month) in [(1, 1), (2, 7)] {
                let date = rust_xlsxwriter::ExcelDateTime::from_ymd(2020, month, 15).unwrap().and_hms(12, 0, 0).unwrap();
                sheet.write_datetime_with_format(row, 0, &date, &format).unwrap();
                sheet.write_string(row, 1, "historical event").unwrap();
            }
            book.save(&path).unwrap();
            let run = |zone: &str, label: &str| -> serde_json::Value {
                let output = fixture.dir.path().join(format!("{label}.json"));
                let status = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "workspace::canonical::tests::workbook_timezone_child", "--test-threads=1"])
                    .env("TZ", zone).env("LOGINSIGHT_CANONICAL_TZ_INPUT", &path).env("LOGINSIGHT_CANONICAL_TZ_OUTPUT", &output).status().unwrap();
                assert!(status.success());
                serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap()
            };
            let seasonal = run("Europe/Berlin", "berlin");
            let fixed_zone = if seasonal["offset"] == "+02:00" { "Africa/Johannesburg" } else { "Africa/Lagos" };
            let fixed = run(fixed_zone, "fixed");
            assert_eq!(seasonal["offset"], fixed["offset"]);
            assert_ne!(seasonal["path"], fixed["path"]);
            assert_ne!(seasonal["times"], fixed["times"]);
        }

        #[test]
        #[cfg(unix)]
        fn workbook_timezone_change_mid_conversion_never_publishes_mixed_snapshots() {
            let fixture = Fixture::new();
            let path = fixture.dir.path().join("changing-zone.xlsx");
            let mut book = rust_xlsxwriter::Workbook::new();
            let sheet = book.add_worksheet();
            sheet.write_string(0, 0, "Timestamp").unwrap();
            sheet.write_string(0, 1, "Message").unwrap();
            for row in 1..=2050 {
                sheet.write_string(row, 0, "2020-01-15 12:00:00").unwrap();
                sheet.write_string(row, 1, "event").unwrap();
            }
            book.save(&path).unwrap();
            let output = fixture.dir.path().join("zone-change.json");
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "workspace::canonical::tests::workbook_timezone_child", "--test-threads=1"])
                .env("TZ", "UTC").env("LOGINSIGHT_CANONICAL_TZ_CHANGE_INPUT", &path).env("LOGINSIGHT_CANONICAL_TZ_OUTPUT", &output).status().unwrap();
            assert!(status.success());
            assert!(serde_json::from_slice::<bool>(&std::fs::read(output).unwrap()).unwrap());
            assert!(canonical_directories().is_empty());
        }

        fn indexed_state(index: sources::FileIndex) -> AppState {
            AppState {
                source: parking_lot::RwLock::new(SourceData::Indexed(index)),
                source_publication: parking_lot::RwLock::new(Default::default()),
                source_names: parking_lot::RwLock::new(Vec::new()),
                codes: parking_lot::RwLock::new(Default::default()),
                system_codes: parking_lot::RwLock::new(Default::default()),
                derived: parking_lot::RwLock::new(Vec::new()),
                case_store_lock: parking_lot::Mutex::new(()),
                codes_path: Default::default(),
                system_codes_path: Default::default(),
            }
        }
        fn write_zip(path: &Path, member: &str, bytes: &[u8]) {
            let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
            zip.start_file(member, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            zip.write_all(bytes).unwrap();
            zip.finish().unwrap();
        }
        fn write_gzip(path: &Path, bytes: &[u8]) {
            let mut encoder = flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::default());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap();
        }
        fn write_workbook(path: &Path, message: &str) {
            let mut book = rust_xlsxwriter::Workbook::new();
            let sheet = book.add_worksheet();
            sheet.write_string(0, 0, "Mensagem").unwrap();
            sheet.write_string(0, 1, "Nível").unwrap();
            sheet.write_string(1, 0, message).unwrap();
            sheet.write_string(1, 1, "info").unwrap();
            book.save(path).unwrap();
        }

        #[test]
        fn changed_zip_gzip_and_workbook_reject_custody_with_empty_or_warm_hash_cache() {
            for warmed in [false, true] {
                for kind in ["zip", "gz", "xlsx"] {
                    let fixture = Fixture::new();
                    let original = fixture.dir.path().join(format!("source.{kind}"));
                    let text = b"2026-09-30 12:00:00 INFO alpha\n";
                    let source_path = match kind {
                        "zip" => {
                            write_zip(&original, "events.log", text);
                            super::super::archive_members(&original).unwrap().remove(0)
                        }
                        "gz" => { write_gzip(&original, text); original.to_string_lossy().into_owned() }
                        _ => { write_workbook(&original, "alpha event"); original.to_string_lossy().into_owned() }
                    };
                    let index = crate::index_source_file(&source_path, "auto", None).unwrap();
                    let physical = PathBuf::from(&index.parts[0].physical_path);
                    let state = indexed_state(index);
                    if warmed {
                        let hashes = crate::pivots::hashes_impl(&state).unwrap();
                        assert!(!hashes.is_empty());
                        assert!(hashes.iter().any(|hash| hash.origin == "original" && hash.path == original.to_string_lossy().as_ref()));
                    }
                    let replacement = fixture.dir.path().join(format!("replacement.{kind}"));
                    match kind {
                        "zip" => write_zip(&replacement, "events.log", b"2026-09-30 12:00:00 INFO omega\n"),
                        "gz" => write_gzip(&replacement, b"2026-09-30 12:00:00 INFO omega\n"),
                        _ => write_workbook(&replacement, "omega event"),
                    }
                    std::fs::rename(&replacement, &original).unwrap();
                    assert!(super::super::validate_canonical_origin(&physical).is_err());
                    assert!(crate::pivots::hashes_impl(&state).is_err(), "{kind}, warmed={warmed}");
                }
            }
        }

        #[test]
        fn nested_provenance_and_event_identity_survive_intermediate_pruning() {
            let fixture = Fixture::new();
            let archive = fixture.dir.path().join("nested.zip");
            let text = "2026-09-30 12:00:00 INFO original event\n";
            let encoded: Vec<u8> = [0xff, 0xfe].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
            write_zip(&archive, "events.log", &encoded);
            let virtual_path = super::super::archive_members(&archive).unwrap().remove(0);
            let index = crate::index_source_file(&virtual_path, "auto", None).unwrap();
            let codes = CodesConfig::default();
            let reference = sources::event_at(&index, 0, &codes, &codes, &[]).event_ref;
            let final_file = PathBuf::from(&index.parts[0].physical_path);
            let intermediate = super::super::resolve_member(&virtual_path).unwrap().unwrap();
            let parent_dir = intermediate.ancestors().find(|path| path.parent() == Some(root().as_path())).unwrap().to_path_buf();
            OpenOptions::new().read(true).write(true).open(parent_dir.join(".used")).unwrap().set_modified(SystemTime::UNIX_EPOCH).unwrap();
            prune(&root());
            assert!(!intermediate.exists());
            assert!(final_file.exists());
            super::super::validate_canonical_origin(&final_file).unwrap();
            let state = indexed_state(index);
            let hashes = crate::pivots::hashes_impl(&state).unwrap();
            assert!(hashes.iter().any(|hash| hash.path == virtual_path && hash.origin == "extraído"));
            let reopened = crate::index_source_file(&virtual_path, "auto", None).unwrap();
            assert_eq!(sources::event_at(&reopened, 0, &codes, &codes, &[]).event_ref, reference);
            std::fs::write(&archive, b"changed original archive").unwrap();
            assert!(crate::pivots::hashes_impl(&state).is_err());
        }

        #[test]
        #[cfg(unix)]
        fn raw_hash_cache_includes_native_generation_beyond_sampled_public_identity() {
            let fixture = Fixture::new();
            let mut bytes = vec![b'x'; 400_000];
            *bytes.last_mut().unwrap() = b'\n';
            let path = fixture.file("large.log", &bytes);
            let first = sources::index_file(path.to_str().unwrap(), "auto", None, None, None).unwrap();
            let public_identity = first.parts[0].identity.clone();
            let first_state = indexed_state(first);
            let old = crate::pivots::hashes_impl(&first_state).unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            bytes[100_000] = b'y'; // Outside first/middle/last64KiB samples.
            let replacement = fixture.file("replacement.log", &bytes);
            OpenOptions::new().read(true).write(true).open(&replacement).unwrap().set_modified(modified).unwrap();
            std::fs::rename(&replacement, &path).unwrap();
            let second = sources::index_file(path.to_str().unwrap(), "auto", None, None, None).unwrap();
            assert_eq!(second.parts[0].identity, public_identity);
            let new = crate::pivots::hashes_impl(&indexed_state(second)).unwrap();
            assert_eq!(old[0].id, new[0].id, "public ID compatibility is distinct from cache generation");
            assert_ne!(old[0].sha256, new[0].sha256);
            assert_eq!(new[0].sha256, format!("{:x}", Sha256::digest(&bytes)));
        }

        #[test]
        fn workbook_explicit_references_remain_based_on_original_workbook() {
            let fixture = Fixture::new();
            let path = fixture.dir.path().join("events.xlsx");
            let mut book = rust_xlsxwriter::Workbook::new();
            let sheet = book.add_worksheet();
            sheet.write_string(0, 0, "Mensagem").unwrap();
            sheet.write_string(0, 1, "Nível").unwrap();
            sheet.write_string(1, 0, "original workbook event").unwrap();
            sheet.write_string(1, 1, "info").unwrap();
            book.save(&path).unwrap();
            let expected = format!("planilha:{}:0:2", &old_identity(&path)[..16]);
            let output = crate::spreadsheet::expand(&path, &|_| {}).unwrap().unwrap();
            let first = std::fs::read_to_string(&output).unwrap();
            let event: Event = serde_json::from_str(first.lines().next().unwrap()).unwrap();
            assert_eq!(event.event_ref, expected);
            let identity = super::super::canonical_event_identity(&output).unwrap();
            OpenOptions::new().read(true).write(true).open(output.parent().unwrap().join(".used")).unwrap().set_modified(SystemTime::UNIX_EPOCH).unwrap();
            prune(&root());
            let rebuilt = crate::spreadsheet::expand(&path, &|_| {}).unwrap().unwrap();
            assert_eq!(std::fs::read_to_string(&rebuilt).unwrap(), first);
            assert_eq!(super::super::canonical_event_identity(&rebuilt).unwrap(), identity);
        }
    }
}

pub(crate) fn canonical_file(
    input: CanonicalInput,
    converter: &str,
    legacy: Option<PathBuf>,
    write: impl FnOnce(&CanonicalInput, &mut dyn Write) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let store = canonical::Store::new(input, converter)?;
    let name = Path::new("payload");
    {
        let _read = store.lock(false)?;
        if store.ready(Some("payload"))?.is_some() { return Ok(store.dir.join(name)); }
    }
    let _write = store.lock(true)?;
    if store.ready(Some("payload"))?.is_some() { return Ok(store.dir.join(name)); }
    let staging = store.staging()?;
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(staging.path().join(name)).map_err(|e| e.to_string())?;
    let mut out = BufWriter::new(file);
    canonical::progress("canonical-convert", "Convertendo fonte; interrupção reinicia esta etapa", 0, 0, "bytes");
    {
        let mut tracked = canonical::ConversionWriter { inner: &mut out, bytes: 0, last: std::time::Instant::now(), report: !converter.starts_with("workbook-") };
        write(&store.input, &mut tracked)?;
    }
    out.flush().and_then(|_| out.get_ref().sync_all()).map_err(|e| e.to_string())?;
    drop(out);
    let artifact = store.artifact(staging.path(), name, legacy.as_deref())?;
    store.publish(staging, vec![artifact])?;
    Ok(store.dir.join(name))
}

// ------------------------------------------------------------------ archives

const ARCHIVE_SEPARATOR: &str = "!/";
const ARCHIVE_MEMBERS: usize = 10_000;
const ARCHIVE_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;

pub fn archive_kind(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".zip") {
        Some("zip")
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some("tgz")
    } else if lower.ends_with(".tar") {
        Some("tar")
    } else {
        None
    }
}

/// Member paths that look like logs (text, compressed logs, EVTX, spreadsheets).
fn loggable_member(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if file.is_empty() || file.starts_with('.') || lower.contains("__macosx/") {
        return false;
    }
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    ["log", "txt", "jsonl", "ndjson", "json", "csv", "tsv", "evtx", "gz", "out", "err", "audit", "xlsx", "xlsm", "xlsb", "xls", "ods"]
        .contains(&ext)
        || ext.is_empty()
        || ext.parse::<u32>().is_ok()
}

/// Relative path without traversal, absolute roots or drive letters.
fn safe_member_path(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.replace('\\', "/").split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Validates a completed immutable extraction or restarts the extraction.
/// Only the requested member is rechecked on direct lookup; listing validates
/// every member. Checksums are memoized only while immutable file stamps match.
fn extract_archive(archive: &Path, requested: Option<&Path>) -> Result<(PathBuf, Vec<String>), String> {
    let input = CanonicalInput::open(archive)?;
    let legacy = input.legacy_source().map(|(_, id)| crate::config_dir().join("expanded").join(format!("archive-{}", &id[..16])));
    let kind = archive_kind(&archive.to_string_lossy()).ok_or("Formato de pacote não suportado.")?;
    let store = canonical::Store::new(input, &format!("archive-v1-{kind}"))?;
    let requested_name = requested.map(|name| format!("members/{}", name.to_string_lossy().replace('\\', "/")));
    let names_of = |manifest: &canonical::Manifest| -> Vec<String> {
        match requested {
            Some(path) => vec![path.to_string_lossy().replace('\\', "/")],
            None => manifest.artifacts.iter().filter_map(|artifact| artifact.path.strip_prefix("members/").map(str::to_string)).collect(),
        }
    };
    {
        let _read = store.lock(false)?;
        if let Some(manifest) = store.ready(requested_name.as_deref())? {
            return Ok((store.dir.join("members"), names_of(&manifest)));
        }
    }
    let _write = store.lock(true)?;
    if let Some(manifest) = store.ready(requested_name.as_deref())? {
        return Ok((store.dir.join("members"), names_of(&manifest)));
    }
    let staging = store.staging()?;
    let mut names = Vec::new();
    let mut unique = std::collections::HashSet::new();
    let mut artifacts = Vec::new();
    let mut total = 0u64;
    let mut reported = std::time::Instant::now();
    canonical::progress("canonical-extract", "Extraindo pacote; interrupção reinicia esta etapa", 0, 0, "bytes");
    let mut write_member = |name: &str, reader: &mut dyn Read, declared: u64| -> Result<(), String> {
        crate::operations::check()?;
        if !loggable_member(name) { return Ok(()); }
        let Some(relative) = safe_member_path(name) else { return Ok(()) };
        if names.len() >= ARCHIVE_MEMBERS { return Err("O pacote tem mais de 10.000 arquivos de log.".into()); }
        if !unique.insert(relative.clone()) { return Err("O pacote contém nomes de arquivos duplicados; extraia-os separadamente.".into()); }
        let member = PathBuf::from("members").join(&relative);
        let target = staging.path().join(&member);
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| e.to_string())?;
        let mut out = BufWriter::new(file);
        let mut buf = [0u8; 65536];
        let mut written = 0u64;
        loop {
            crate::operations::check()?;
            let n = reader.read(&mut buf).map_err(|e| format!("Falha ao extrair {name}: {e}"))?;
            if n == 0 { break; }
            written += n as u64;
            total += n as u64;
            if reported.elapsed() >= std::time::Duration::from_millis(150) {
                canonical::progress("canonical-extract", "Extraindo pacote; interrupção reinicia esta etapa", usize::try_from(total).unwrap_or(usize::MAX), 0, "bytes");
                reported = std::time::Instant::now();
            }
            if total > ARCHIVE_TOTAL_BYTES || (declared > 0 && written > declared.saturating_mul(2) + 1024 * 1024) {
                return Err("O pacote excede o limite de extração (64 GB) ou declara tamanhos inconsistentes.".into());
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        out.flush().and_then(|_| out.get_ref().sync_all()).map_err(|e| e.to_string())?;
        drop(out);
        let old = legacy.as_ref().map(|dir| dir.join(&relative));
        artifacts.push(store.artifact(staging.path(), &member, old.as_deref())?);
        names.push(relative.to_string_lossy().replace('\\', "/"));
        Ok(())
    };
    match kind {
        "zip" => {
            let file = store.input.reader()?;
            let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| format!("ZIP inválido: {e}"))?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).map_err(|e| format!("ZIP inválido: {e}"))?;
                if entry.is_dir() || entry.encrypted() { continue; }
                let name = entry.name().to_string();
                let size = entry.size();
                write_member(&name, &mut entry, size)?;
            }
        }
        _ => {
            let file = store.input.reader()?;
            let reader: Box<dyn Read> = if kind == "tgz" {
                Box::new(flate2::read::MultiGzDecoder::new(std::io::BufReader::new(file)))
            } else { Box::new(std::io::BufReader::new(file)) };
            let mut tar = tar::Archive::new(reader);
            for entry in tar.entries().map_err(|e| format!("TAR inválido: {e}"))? {
                let mut entry = entry.map_err(|e| format!("TAR inválido: {e}"))?;
                if !entry.header().entry_type().is_file() { continue; }
                let name = entry.path().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
                let size = entry.header().size().unwrap_or(0);
                write_member(&name, &mut entry, size)?;
            }
        }
    }
    drop(write_member);
    store.publish(staging, artifacts)?;
    Ok((store.dir.join("members"), names))
}

/// Virtual paths (`pacote.zip!/pasta/app.log`) for the members of an archive.
pub fn archive_members(archive: &Path) -> Result<Vec<String>, String> {
    let (_, names) = extract_archive(archive, None)?;
    Ok(names
        .into_iter()
        .map(|name| format!("{}{ARCHIVE_SEPARATOR}{name}", archive.to_string_lossy()))
        .collect())
}

/// Resolves a virtual member path to its extracted file.
pub fn resolve_member(path: &str) -> Result<Option<PathBuf>, String> {
    let Some((archive, member)) = path.split_once(ARCHIVE_SEPARATOR) else { return Ok(None) };
    if archive_kind(archive).is_none() {
        return Ok(None);
    }
    let relative = safe_member_path(member).ok_or("Caminho inválido dentro do pacote.")?;
    let (dir, _) = extract_archive(Path::new(archive), Some(&relative))?;
    let target = dir.join(relative);
    if !target.is_file() {
        return Err(format!("{member} não foi encontrado em {archive}."));
    }
    Ok(Some(target))
}

pub fn expand_gzip(path: &Path) -> Result<PathBuf, String> {
    let input = CanonicalInput::open(path)?;
    let legacy = input.legacy_source().map(|(path, id)| crate::config_dir().join("expanded").join(format!(
        "{}-{}", &id[..16], path.file_stem().unwrap_or_default().to_string_lossy()
    )));
    canonical_file(input, "gzip-v1", legacy, |input, out| {
        let mut input = flate2::read::MultiGzDecoder::new(input.reader()?);
        let mut buffer = [0u8; 65536];
        loop {
            crate::operations::check()?;
            let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 { break; }
            out.write_all(&buffer[..count]).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

fn unsupported_encoding_bom(sample: &[u8]) -> Option<&'static str> {
    // Check the longest signatures first: UTF-32LE shares UTF-16LE's prefix.
    // UTF-16LE followed by an initial NUL is ambiguous and is declined too.
    if sample.starts_with(&[0xFF, 0xFE, 0, 0]) { Some("UTF-32LE") }
    else if sample.starts_with(&[0, 0, 0xFE, 0xFF]) { Some("UTF-32BE") }
    else { None }
}

/// Text logs written by Windows tools in UTF-16 (PowerShell, Event Viewer
/// exports) or in a legacy code page, recognized from a sample; UTF-8 and
/// binary files return None. Import separately rejects unsupported BOMs.
pub fn sniff_encoding(sample: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    if unsupported_encoding_bom(sample).is_some() { return None; }
    if sample.starts_with(&[0xFF, 0xFE]) {
        return Some(encoding_rs::UTF_16LE);
    }
    if sample.starts_with(&[0xFE, 0xFF]) {
        return Some(encoding_rs::UTF_16BE);
    }
    if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return None;
    }
    // UTF-16 without a byte order mark: ASCII text has a zero in every other byte.
    let pairs = sample.len() / 2;
    if pairs >= 16 {
        let (mut even, mut odd) = (0usize, 0usize);
        for pair in sample.chunks_exact(2) {
            even += usize::from(pair[0] == 0);
            odd += usize::from(pair[1] == 0);
        }
        if odd * 10 >= pairs * 4 && even * 10 < pairs {
            return Some(encoding_rs::UTF_16LE);
        }
        if even * 10 >= pairs * 4 && odd * 10 < pairs {
            return Some(encoding_rs::UTF_16BE);
        }
    }
    if sample.contains(&0) {
        return None;
    }
    // A legacy code page shows invalid UTF-8 and no valid multi-byte character.
    let mut rest = sample;
    let mut invalid = false;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                if !text.is_ascii() {
                    return None;
                }
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                if !valid.is_ascii() {
                    return None;
                }
                match error.error_len() {
                    Some(length) => {
                        invalid = true;
                        rest = &after[length..];
                    }
                    // A character cut at the end of the sample.
                    None => break,
                }
            }
        }
    }
    invalid.then_some(encoding_rs::WINDOWS_1252)
}

/// Converts a UTF-16 or legacy code page text file to UTF-8 once; `None` when
/// the file is read as it is.
pub fn expand_encoding(path: &Path) -> Result<Option<PathBuf>, String> {
    let input = CanonicalInput::open(path)?;
    let mut sample = Vec::new();
    input.reader()?.take(1 << 20).read_to_end(&mut sample).map_err(|e| e.to_string())?;
    if let Some(encoding) = unsupported_encoding_bom(&sample) {
        let ambiguity = if encoding == "UTF-32LE" { " Também pode ser UTF-16LE com NUL inicial; a leitura automática é ambígua." } else { "" };
        return Err(format!("A assinatura do arquivo é compatível com {encoding}, que ainda não é suportado.{ambiguity} Converta uma cópia para UTF-8 ou UTF-16 e tente novamente. O arquivo original não foi alterado."));
    }
    let Some(encoding) = sniff_encoding(&sample) else { return Ok(None) };
    let legacy = input.legacy_source().map(|(path, id)| crate::config_dir().join("expanded").join(format!(
        "{}-utf8-{}", &id[..16], path.file_name().unwrap_or_default().to_string_lossy()
    )));
    canonical_file(input, &format!("utf8-v1-{}", encoding.name()), legacy, |input, out| {
        let mut reader = input.reader()?;
        let mut decoder = encoding.new_decoder_with_bom_removal();
        let mut buffer = [0u8; 65536];
        let mut text = String::with_capacity(1 << 18);
        loop {
            crate::operations::check()?;
            let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
            let last = count == 0;
            let mut remaining = &buffer[..count];
            loop {
                let (result, read, _) = decoder.decode_to_string(remaining, &mut text, last);
                remaining = &remaining[read..];
                out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
                text.clear();
                if result == encoding_rs::CoderResult::InputEmpty { break; }
            }
            if last { break; }
        }
        Ok(())
    }).map(Some)
}

/// Exact grouped timeline over the same admitted scope as rows/count/facets.
#[tauri::command]
pub(crate) async fn grouped_timeline(
    filters: Vec<Filter>,
    field: String,
    grid: crate::grouped_timeline::Grid,
    limit: Option<usize>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<crate::grouped_timeline::Response, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    let context = crate::grouped_timeline::Context {
        analysis: admitted.identity.clone(), source_generation: admitted.source_generation, case_key: admitted.case_key.clone(),
    };
    let spec = crate::grouped_timeline::Spec::new(field, grid, limit, context)?;
    crate::offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| grouped_timeline_impl(app.state::<AppState>().inner(), &filters, case_events.as_deref(), &spec)).await?
}

pub(crate) fn grouped_timeline_impl(
    state: &AppState, filters: &[Filter], case_events: Option<&[Event]>, spec: &crate::grouped_timeline::Spec,
) -> Result<crate::grouped_timeline::Response, String> {
    validate(filters)?;
    spec.validate()?;
    let prepared = query::prepare(filters);
    let mut accumulator = crate::grouped_timeline::Accumulator::new(spec)?;
    if let Some(events) = case_events {
        for event in events {
            crate::operations::check()?;
            if prepared.iter().all(|filter| query::matches(event, filter)) { accumulator.add_event(event)?; }
        }
    } else {
        if let Some(result) = with_engine(state, |source| crate::engine::grouped_timeline(source, &prepared, spec))? { return Ok(result); }
        with_selection(state, filters, |selection| {
            let indexed = matches!(selection.source, SourceData::Indexed(_));
            for event in selection.iter() {
                if indexed { accumulator.add_indexed_event(&event)?; }
                else { accumulator.add_event(&event)?; }
            }
            Ok::<(), String>(())
        })??;
    }
    accumulator.finish()
}

#[cfg(test)]
mod grouped_epoch_domain_tests {
    use super::*;
    #[test]
    fn grouped_case_and_memory_keep_epoch_distinct_from_absent_time() {
        let state = AppState {
            source_publication: Default::default(), source: parking_lot::RwLock::new(SourceData::None),
            source_names: Default::default(), codes: Default::default(), system_codes: Default::default(),
            derived: Default::default(), case_store_lock: Default::default(),
            codes_path: Default::default(), system_codes_path: Default::default(),
        };
        let mut epoch = Event::empty(); epoch.timestamp = Some(0); epoch.source = "epoch".into();
        let mut absent = Event::empty(); absent.id = 1; absent.source = "absent".into();
        let events = vec![epoch, absent];
        let spec = crate::grouped_timeline::Spec::new("source".into(),
            crate::grouped_timeline::Grid { start: 0, bucket_ms: 1, bucket_count: 1 },
            None, Default::default()).unwrap();
        let case = grouped_timeline_impl(&state, &[], Some(&events), &spec).unwrap();
        *state.source.write() = SourceData::Memory(events);
        let memory = grouped_timeline_impl(&state, &[], None, &spec).unwrap();
        assert_eq!(case, memory);
        assert_eq!((case.total.count, case.untimed), (1, 1));
        assert_eq!(case.total.buckets, [1]);
        assert_eq!(case.series[0].key, "epoch");
    }
}
