// Parsing allocates many small values on every worker thread; the system
// allocator serializes them, mimalloc does not.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod analysis;
mod analysis_context;
mod case_interpretation;
mod case_resources;
mod case_security;
mod analysis_runtime;
mod analysis_commands;
mod analysis_visibility;
mod attack;
mod case_cache;
mod case_evidence;
mod case_evidence_anchors;
mod case_evidence_budget;
mod case_evidence_commands;
mod case_evidence_display;
mod case_evidence_display_policy;
mod case_evidence_history;
mod case_evidence_members;
mod case_portable_native;
mod case_portable_commands;
mod case_recovery;
mod case_recovery_commands;
mod case_work_budget;
mod case_archive_format;
mod case_archive;
mod case_archive_references;
mod case_images;
mod case_store;
mod detections;
mod evidence;
mod exclusion_store;
mod exclusion_commands;
mod security_normalize;
mod security_content;
mod security_store;
mod security_results;
mod security_grouping;
mod security_participants;
#[cfg(test)]
mod security_participants_tests;
mod security_budget;
mod security_reconstruct;
mod discovery;
mod distinct;
mod engine;
mod entities;
mod event_preview;
mod field_transform;
mod grouped_timeline;
mod index_cache;
mod metadata_checkpoint;
mod metadata_store;
mod insights;
mod java_stacktrace;
mod journeys;
mod mcp;
mod model;
mod operations;
mod global_scheduler;
mod page_projection;
mod projection_commands;
mod pivots;
mod query;
mod query_regex;
mod querylang;
#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod publication_tests;
mod reference_commands;
mod reference_store;
mod reference_lookup;
mod remote;
mod resources;
mod resource_settings;
mod sigma;
mod sources;
mod source_publication;
mod spreadsheet;
#[doc(hidden)]
pub mod testkit;
mod threats;
mod timeline_export;
mod triage;
mod updates;
mod workspace;
mod detail_commands;
#[cfg(test)]
mod derived_surface_tests;
#[cfg(test)]
mod mapped_metadata_workload;

use model::{CodesConfig, Event, STANDARD_COLUMNS};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State};

pub enum SourceData {
    None,
    /// Fonte pequena/média em memória (Event Log do Windows).
    Memory(Vec<Event>),
    /// Arquivo grande indexado via mmap (materialização preguiçosa).
    Indexed(sources::FileIndex),
}

pub struct AppState {
    pub(crate) source_publication: RwLock<source_publication::Publication>,
    pub source: RwLock<SourceData>,
    /// nomes das fontes carregadas (mais de uma quando arquivos são unidos)
    pub source_names: RwLock<Vec<String>>,
    pub codes: RwLock<CodesConfig>,
    pub system_codes: RwLock<CodesConfig>,
    pub derived: RwLock<Vec<sources::CompiledDerived>>,
    pub case_store_lock: Mutex<()>,
    pub codes_path: PathBuf,
    pub system_codes_path: PathBuf,
}

#[derive(Serialize)]
pub(crate) struct LoadSummary {
    publication: source_publication::Receipt,
    count: usize,
    columns: Vec<String>,
    source_desc: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationProgress {
    operation_id: Option<String>,
    phase_id: String,
    operation: String,
    phase: String,
    completed: usize,
    total: usize,
    unit: String,
    cancellable: bool,
}

/// Emite progresso para a UI quando há uma janela (comandos Tauri e chamadas
/// MCP passam o handle; chamadas sem UI passam `None` e viram no-op).
fn emit_progress(
    app: Option<&AppHandle>,
    operation: &str,
    phase: &str,
    completed: usize,
    total: usize,
    unit: &str,
    cancellable: bool,
) {
    let Some(app) = app else { return };
    let _ = app.emit(
        "operation-progress",
        OperationProgress {
            operation_id: operations::current_id(),
            phase_id: phase.into(),
            operation: operation.into(),
            phase: phase.into(),
            completed,
            total,
            unit: unit.into(),
            cancellable: operations::current_generation().is_some()
                && (cancellable || matches!(operation, "carregamento" | "exploração" | "análise")),
        },
    );
}

pub(crate) fn config_dir() -> PathBuf {
    case_recovery::profiles::selected(&base_config_dir())
}

pub(crate) fn base_config_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("LOGINSIGHT_DATA_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(windows)]
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    #[cfg(not(windows))]
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("LogInsight")
}

/// Move trabalho pesado para uma thread blocking do runtime, mantendo os
/// comandos Tauri async (a thread principal/webview não congela). O closure
/// captura o AppHandle e resolve `app.state::<AppState>()` lá dentro, pois
/// `State<'_, T>` não pode ser movido para um closure 'static.
async fn offload<T, F>(f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce() -> T + Send + 'static {
    offload_operation(None, f).await
}

async fn offload_operation<T, F>(operation_id: Option<String>, f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce() -> T + Send + 'static {
    offload_operation_priority(operation_id, global_scheduler::Priority::Normal, f).await
}
async fn offload_operation_priority<T, F>(operation_id: Option<String>, priority: global_scheduler::Priority, f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce() -> T + Send + 'static {
    let token = operations::token(operation_id)?.with_priority(priority);
    tauri::async_runtime::spawn_blocking(move || operations::run_with_token(token, f))
        .await.map_err(|e| e.to_string())?
}

/// Cancellation may arrive while existing readers hold the source. Recheck
/// under the acquired write guard, before replacing or merging any state.
pub(crate) fn source_write_checked(
    state: &AppState,
) -> Result<parking_lot::RwLockWriteGuard<'_, SourceData>, String> {
    loop {
        operations::check()?;
        if let Some(source) = state.source.try_write_for(std::time::Duration::from_millis(50)) {
            // Cancellation could arrive at the same instant the final reader
            // released the lock. Check again before allowing any mutation.
            operations::check()?;
            if let Some(admitted) = analysis_runtime::current() {
                if let Some(identity) = &admitted.identity { analysis_runtime::validate_identity(identity)?; }
                admitted.validate_publication(state, false)?;
            }
            return Ok(source);
        }
    }
}

/// Reject a changed/truncated mapped source before an infallible legacy reader
/// can mistake a refused engine session for permission to scan stale offsets.
pub(crate) fn validate_current_source(state: &AppState) -> Result<(), String> {
    let source = crate::analysis_runtime::source(&state);
    if let SourceData::Indexed(idx) = &*source {
        for part in &idx.parts { sources::validate_source(part)?; }
    }
    Ok(())
}

/// Execute the captured snapshot with its original cancellation registration.
async fn offload_admitted<T, F>(operation_id: Option<String>, app: AppHandle, admitted: std::sync::Arc<analysis_runtime::Admitted>, f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce() -> T + Send + 'static {
    offload_case(operation_id, app, admitted, None, move |_| f()).await
}

/// Expensive ledger verification/mask compilation runs only after cancellation
/// registration. Evidence and memory sources are filtered once in this worker.
async fn offload_case<T, F>(operation_id: Option<String>, app: AppHandle, admitted: std::sync::Arc<analysis_runtime::Admitted>, case_events: Option<Vec<Event>>, f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce(Option<Vec<Event>>) -> T + Send + 'static {
    offload_case_input(operation_id, app, admitted, global_scheduler::Priority::Normal, move |admitted| admitted.prepare_visibility(case_events), f).await
}

async fn offload_case_interactive<T, F>(operation_id: Option<String>, app: AppHandle, admitted: std::sync::Arc<analysis_runtime::Admitted>, case_events: Option<Vec<Event>>, f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce(Option<Vec<Event>>) -> T + Send + 'static {
    offload_case_input(operation_id, app, admitted, global_scheduler::Priority::Interactive, move |admitted| admitted.prepare_visibility(case_events), f).await
}

/// A detail/field action prepares only its selected record. Admission still
/// binds the complete synchronized publication and validates its token at exit.
async fn offload_case_record<T, F>(operation_id: Option<String>, app: AppHandle, admitted: std::sync::Arc<analysis_runtime::Admitted>, case_events: Option<std::sync::Arc<case_cache::Records>>, id: usize, event_ref: Option<String>, f: F) -> Result<T, String>
where T: Send + 'static, F: FnOnce(Option<Vec<Event>>) -> T + Send + 'static {
    offload_case_input(operation_id, app, admitted, global_scheduler::Priority::Interactive, move |admitted| admitted.prepare_visibility_record(case_events.as_deref().map(case_cache::Records::as_slice), id, event_ref.as_deref()), f).await
}

async fn offload_case_input<T, P, F>(operation_id: Option<String>, app: AppHandle, admitted: std::sync::Arc<analysis_runtime::Admitted>, priority: global_scheduler::Priority, input: P, f: F) -> Result<T, String>
where T: Send + 'static, P: FnOnce(&analysis_runtime::Admitted) -> Result<Option<Vec<Event>>, String> + Send + 'static, F: FnOnce(Option<Vec<Event>>) -> T + Send + 'static {
    let token = admitted.take_operation(operation_id, priority)?;
    tauri::async_runtime::spawn_blocking(move || operations::run_with_token(token, || {
        admitted.validate(app.state::<AppState>().inner())?;
        let progress_app = app.clone();
        analysis_runtime::with(Some(admitted.clone()), || operations::with_reporter(std::sync::Arc::new(move |progress| {
            let _ = progress_app.emit("operation-progress", progress);
        }), || {
            let case_events = input(&admitted)?;
            admitted.validate(app.state::<AppState>().inner())?;
            operations::check()?;
            let result = f(case_events);
            admitted.validate_visibility()?;
            admitted.validate_native_case_authority()?;
            admitted.schedule_derived_variant(app.state::<AppState>().inner());
            Ok(result)
        }))
    })).await.map_err(|error| error.to_string())??
}

#[tauri::command]
fn cancel_task(operation_id: String) -> bool { operations::cancel_id(&operation_id) }

/// Legacy application templates are read-only during startup. In particular,
/// a missing version marker never authorizes rewriting malformed originals
/// before the explicit Case migration can diagnose/preserve them.
fn read_legacy_catalog(path: &PathBuf) -> Option<CodesConfig> {
    use std::io::Read;
    let file = crate::case_archive_format::open_regular(path).ok()?;
    let mut bytes = Vec::new();
    file.take((3 << 20) + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 3 << 20 { return None; }
    serde_json::from_slice(&bytes).ok()
}
fn load_codes(path: &PathBuf) -> CodesConfig {
    read_legacy_catalog(path).unwrap_or_else(|| serde_json::from_str(include_str!("default_codes.json")).unwrap_or_default())
}

fn all_columns(events: &[Event]) -> Vec<String> {
    let mut cols: Vec<String> = STANDARD_COLUMNS.iter().map(|s| s.to_string()).collect();
    let mut extra = HashSet::new();
    for ev in events.iter().take(20_000) {
        for k in ev.fields.keys() {
            extra.insert(k.clone());
        }
    }
    let mut extra: Vec<String> = extra.into_iter().collect();
    extra.sort();
    cols.extend(extra);
    cols
}

/// Read the legacy system template without writing or harvesting implicitly.
/// A Case obtains a new system catalog only through its explicit editor action.
fn load_system_codes(path: &PathBuf) -> CodesConfig {
    read_legacy_catalog(path).unwrap_or_default()
}

#[tauri::command]
async fn list_channels() -> Result<Vec<String>, String> {
    offload(sources::list_channels).await?
}

#[tauri::command]
async fn load_event_log(
    channel: String,
    max_events: usize,
    merge: Option<bool>,
    app: AppHandle,
    operation_id: Option<String>,
    analysis_context: Option<analysis_context::Identity>,
    source_generation: Option<u64>,
) -> Result<LoadSummary, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Publish, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(operation_id, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        load_event_log_impl(state.inner(), &channel, max_events, merge, Some(&app))
    })
    .await?
}

pub(crate) fn load_event_log_impl(
    state: &AppState,
    channel: &str,
    max_events: usize,
    merge: Option<bool>,
    app: Option<&AppHandle>,
) -> Result<LoadSummary, String> {
    let max_events = max_events.clamp(1, 100_000);
    emit_progress(
        app,
        "carregamento",
        "Lendo Event Log",
        0,
        max_events,
        "eventos",
        false,
    );
    let idx = workspace::index_channel(channel, max_events)?;
    operations::check()?;
    prepare_engine(state, &idx, app)?;
    emit_progress(app, "carregamento", "Ativando fonte carregada", 0, 0, "registros", true);
    let summary = source_publication::publish(
        state, idx, vec![format!("Event Log: {channel}")],
        vec![source_publication::Input::Eventlog { channel: channel.into(), max_events }],
        merge.unwrap_or(false),
    )?;
    emit_progress(
        app,
        "carregamento",
        "Concluído",
        summary.count,
        summary.count,
        "eventos",
        false,
    );
    Ok(summary)
}

/// Indexa um arquivo aplicando formato customizado e config de data/hora salva.
fn index_source_file(
    path: &str,
    format: &str,
    app: Option<&AppHandle>,
) -> Result<sources::FileIndex, String> {
    emit_progress(app, "carregamento", "Preparando entrada para indexação", 0, 0, "", true);
    // Members of ZIP/TAR packages keep a stable virtual path (pacote.zip!/app.log).
    let member = workspace::resolve_member(path)?;
    let physical = member
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let extension = std::path::Path::new(&physical)
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    if extension == "evtx" {
        let mut idx = workspace::index_channel(&physical, usize::MAX)?;
        idx.parts[0].path = path.to_string();
        return Ok(idx);
    }
    let file_label = path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
    // Workbooks become one event per row; exports named .xls that are text are read as text.
    let sheet = if spreadsheet::is_spreadsheet(std::path::Path::new(&physical)) {
        spreadsheet::expand(std::path::Path::new(&physical), &|rows| {
            emit_progress(app, "carregamento", &format!("Lendo planilha {file_label}"), rows, 0, "linhas", false);
        })?
    } else {
        None
    };
    let (expanded, format) = match sheet {
        Some(events) => (Some(events), "snapshot"),
        None => {
            let expanded = if extension == "gz" {
                Some(workspace::expand_gzip(std::path::Path::new(&physical))?)
            } else {
                member.clone()
            };
            // UTF-16 and legacy code pages are converted to UTF-8 before indexing.
            let text = expanded.clone().unwrap_or_else(|| std::path::PathBuf::from(&physical));
            (workspace::expand_encoding(&text)?.or(expanded), format)
        }
    };
    let index_path = expanded
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let custom = if let Some(name) = format.strip_prefix("custom:") {
        let f = custom_formats()
            .into_iter()
            .find(|f| f.name == name)
            .ok_or_else(|| format!("Formato customizado '{name}' não encontrado."))?;
        Some(f.to_parse()?)
    } else {
        None
    };
    let saved_ts = load_ts_config(path).map(|c| c.compile()).transpose()?;
    let mut idx = index_cache::open_with_progress(
        &index_path,
        format,
        custom,
        saved_ts,
        Some(&|progress| {
            if let Some(app) = app {
                let _ = app.emit("operation-progress", serde_json::json!({
                    "operationId": operations::current_id(),
                    "operation": "carregamento", "phaseId": progress.phase_id,
                    "phase": format!("{} · {}", progress.phase, file_label),
                    "completed": progress.completed, "total": progress.total,
                    "unit": progress.unit, "cancellable": true, "state": "indexing",
                    "elapsedMs": operations::elapsed_ms(),
                    "resumedRows": progress.resumed_rows,
                    "checkpointRows": progress.checkpoint_rows,
                }));
            }
        }),
    )?;
    idx.parts[0].path = path.to_string();
    idx.parts[0].file_name = file_label;
    if idx.ts_config.is_some() {
        emit_progress(app, "carregamento", "Aplicando configuração de data/hora", 0, idx.lines.len(), "registros", true);
        idx = index_cache::timestamps_on_load(idx, Some(&|phase, done, total| {
            emit_progress(app, "carregamento", phase, done, total, "registros", true);
        }))?;
    }
    Ok(idx)
}

fn with_ingestion_progress<T>(app: Option<&AppHandle>, f: impl FnOnce() -> T) -> T {
    if let Some(app) = app.cloned() {
        operations::with_reporter(std::sync::Arc::new(move |progress: operations::Progress| {
            let _ = app.emit("operation-progress", progress);
        }), f)
    } else { f() }
}

#[tauri::command]
async fn load_file(
    path: String,
    format: String,
    merge: Option<bool>,
    app: AppHandle,
    operation_id: Option<String>,
    analysis_context: Option<analysis_context::Identity>,
    source_generation: Option<u64>,
) -> Result<LoadSummary, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Publish, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(operation_id, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        load_file_impl(state.inner(), &path, &format, merge, Some(&app))
    })
    .await?
}

pub(crate) fn load_file_impl(
    state: &AppState,
    path: &str,
    format: &str,
    merge: Option<bool>,
    app: Option<&AppHandle>,
) -> Result<LoadSummary, String> {
    with_ingestion_progress(app, || {
    emit_progress(
        app,
        "carregamento",
        "Preparando arquivo",
        0,
        0,
        "linhas",
        false,
    );
    let idx = index_source_file(path, format, app)?;
    operations::check()?;
    emit_progress(
        app,
        "carregamento",
        "Indexando linhas",
        idx.lines.len(),
        idx.lines.len(),
        "linhas",
        false,
    );
    prepare_engine(state, &idx, app)?;
    emit_progress(app, "carregamento", "Ativando fonte carregada", 0, 0, "registros", true);
    let summary = source_publication::publish(
        state, idx, vec![format!("Arquivo: {path}")],
        vec![source_publication::Input::File { paths: vec![path.into()], format: format.into() }],
        merge.unwrap_or(false),
    )?;
    emit_progress(
        app,
        "carregamento",
        "Concluído",
        summary.count,
        summary.count,
        "eventos",
        false,
    );
    Ok(summary)
    })
}

/// Builds the query engine's stores for newly indexed files (cached per
/// file), so the first queries are already fast. Opening takes longer once.
pub(crate) fn prepare_engine(state: &AppState, idx: &sources::FileIndex, app: Option<&AppHandle>) -> Result<(), String> {
    let codes = crate::analysis_runtime::codes(&state).clone();
    let system = crate::analysis_runtime::system_codes(&state).clone();
    let derived = crate::analysis_runtime::derived(&state).clone();
    let operation_id = operations::current_id();
    let started = std::time::Instant::now();
    let phase_clock = Mutex::new((String::new(), std::time::Instant::now()));
    engine::prepare_detailed(idx, &codes, &system, &derived, &|p| {
        let Some(app) = app else { return };
        let phase_elapsed = {
            let mut phase = phase_clock.lock();
            if phase.0 != p.phase { *phase = (p.phase.clone(), std::time::Instant::now()); }
            phase.1.elapsed().as_millis() as u64
        };
        let _ = app.emit("operation-progress", serde_json::json!({
            "operationId": operation_id, "operation": "carregamento", "phaseId": p.phase,
            "phase": p.phase, "completed": p.completed, "total": p.total, "unit": "registros",
            "cancellable": p.state == "indexing", "state": p.state,
            "checkpointRows": p.checkpoint_rows, "completedSegments": p.completed_segments,
            "totalSegments": p.total_segments, "resumedRows": p.resumed_rows,
            "elapsedMs": started.elapsed().as_millis() as u64, "phaseElapsedMs": phase_elapsed,
            "error": p.error
        }));
    })
}

#[tauri::command]
async fn engine_status(app: AppHandle, analysis_context: Option<analysis_context::Identity>, source_generation: Option<u64>) -> Result<Option<engine::EngineStatus>, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Dataset, None, crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(None, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        let source = crate::analysis_runtime::source(&state);
        match &*source {
            SourceData::Indexed(idx) => Some(engine::status(idx, &crate::analysis_runtime::codes(&state), &crate::analysis_runtime::system_codes(&state), &crate::analysis_runtime::derived(&state))),
            _ => None,
        }
    }).await
}

#[tauri::command]
async fn engine_retry(app: AppHandle, operation_id: Option<String>, analysis_context: Option<analysis_context::Identity>, source_generation: Option<u64>) -> Result<Option<engine::EngineStatus>, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Dataset, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(operation_id, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        let source = crate::analysis_runtime::source(&state);
        let SourceData::Indexed(idx) = &*source else { return Ok(None) };
        engine::retry(idx, &crate::analysis_runtime::codes(&state), &crate::analysis_runtime::system_codes(&state), &crate::analysis_runtime::derived(&state));
        prepare_engine(state.inner(), idx, Some(&app))?;
        let status = engine::status(idx, &crate::analysis_runtime::codes(&state), &crate::analysis_runtime::system_codes(&state), &crate::analysis_runtime::derived(&state));
        Ok(Some(status))
    }).await?
}

#[tauri::command]
async fn load_files(
    paths: Vec<String>,
    format: String,
    merge: Option<bool>,
    app: AppHandle,
    operation_id: Option<String>,
    analysis_context: Option<analysis_context::Identity>,
    source_generation: Option<u64>,
) -> Result<LoadSummary, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Publish, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(operation_id, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        load_files_impl(state.inner(), &paths, &format, merge, Some(&app))
    })
    .await?
}

pub(crate) fn load_files_impl(
    state: &AppState,
    paths: &[String],
    format: &str,
    merge: Option<bool>,
    app: Option<&AppHandle>,
) -> Result<LoadSummary, String> {
    with_ingestion_progress(app, || {
    if paths.is_empty() {
        return Err("Nenhum arquivo selecionado.".into());
    }
    let mut indices: Option<sources::FileIndex> = None;
    let mut names = Vec::new();
    for path in paths {
        operations::check()?;
        let idx = index_source_file(path, format, app)?;
        if let Some(ref mut all) = indices {
            all.append(idx)?;
        } else {
            indices = Some(idx);
        }
        names.push(format!("Arquivo: {path}"));
    }
    operations::check()?;
    let idx = indices.unwrap();
    prepare_engine(state, &idx, app)?;
    emit_progress(app, "carregamento", "Ativando fonte carregada", 0, 0, "registros", true);
    let summary = source_publication::publish(
        state, idx, names,
        vec![source_publication::Input::File { paths: paths.to_vec(), format: format.into() }],
        merge.unwrap_or(false),
    )?;
    emit_progress(
        app,
        "carregamento",
        "Pronto",
        summary.count,
        summary.count,
        "eventos",
        false,
    );
    Ok(summary)
    })
}

// ------------------------------------------------------- data/hora (TsConfig)

pub(crate) fn load_ts_config(path: &str) -> Option<sources::TsConfig> {
    analysis_runtime::current().and_then(|admitted| admitted.interpretation.timestamps.get(path).cloned())
}

#[tauri::command]
async fn get_ts_config(path: String, app: AppHandle, analysis_context: Option<analysis_context::Identity>) -> Result<Option<sources::TsConfig>, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app, admitted, move || load_ts_config(&path)).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimestampMutation {
    publication: source_publication::Receipt,
    #[serde(serialize_with = "crate::analysis_context::serialize_management_snapshot")]
    analysis_context: analysis_context::Snapshot,
}

#[tauri::command]
async fn set_ts_config(
    path: String, config: Option<sources::TsConfig>, app: AppHandle, operation_id: Option<String>,
    analysis_context: Option<analysis_context::Identity>, source_generation: Option<u64>,
) -> Result<TimestampMutation, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::EditSource, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(operation_id, app.clone(), admitted, move || set_ts_config_impl(app.state::<AppState>().inner(), &path, config, Some(&app))).await?
}

pub(crate) fn set_ts_config_impl(state: &AppState, path: &str, config: Option<sources::TsConfig>, app: Option<&AppHandle>) -> Result<TimestampMutation, String> {
    let admitted = case_interpretation::current()?;
    let expected = admitted.identity.as_ref().ok_or(analysis_runtime::STALE)?;
    let compiled = config.as_ref().map(|value| value.compile()).transpose()?;
    let mut source = source_write_checked(state)?;
    let owner = source_publication::receipt_locked(state).analysis_context;
    if !matches!(&*source, SourceData::None) && owner.as_ref().is_none_or(|owner| owner.case_id != expected.case_id || owner.analysis_id != expected.analysis_id) {
        return Err("A fonte aberta pertence a outro Caso; reabra a fonte deste Caso antes de ajustar data/hora.".into());
    }
    let mut receipt = source_publication::prepare_touch_locked(state)?;
    // Prepare replacement metadata without mutating the published source.
    // Failure or a stale editor leaves both its source and saved settings intact.
    let replacement = match &*source {
        SourceData::Indexed(index) => {
            let mut next = sources::FileIndex { parts: index.parts.clone(), lines: index.lines.clone(), columns: index.columns.clone(), time_order: index.time_order.clone() };
            let position = next.parts.iter().position(|part| part.path == path).ok_or("O arquivo não pertence à fonte deste Caso.")?;
            next.parts[position].ts_config = compiled;
            sources::retimestamp_index(&mut next, Some(&|done, total| emit_progress(app, "data/hora", "Recalculando timestamps", done, total, "linhas", false)))?;
            SourceData::Indexed(next)
        }
        SourceData::Memory(events) => {
            let single = state.source_names.read().len() <= 1;
            let mut events = events.clone();
            for event in &mut events {
                let event_path = event.col_str("caminho").unwrap_or_default();
                if event_path == path || (event_path.is_empty() && single) {
                    if let Some(config) = &compiled { sources::apply_ts_config_event(event, config); }
                    else { return Err("Reabra o arquivo original para remover o ajuste de data/hora de uma fonte antiga em memória.".into()); }
                }
            }
            SourceData::Memory(events)
        }
        SourceData::None => SourceData::None,
    };
    operations::check()?;
    let snapshot = case_interpretation::update_domain(expected, "timestamps", |settings| {
        if let Some(config) = config { settings.timestamps.insert(path.into(), config); }
        else { settings.timestamps.remove(path); }
        Ok(())
    })?;
    receipt.analysis_context = Some(snapshot.identity());
    *source = replacement;
    let mutation = source_publication::commit_touch_locked(state, receipt);
    if let SourceData::Indexed(index) = &*source {
        engine::request_rebuild(index, &snapshot.interpretation.as_ref().unwrap().codes, &snapshot.interpretation.as_ref().unwrap().system_codes, &analysis_runtime::derived(state));
    }
    Ok(TimestampMutation { publication: mutation.publication, analysis_context: snapshot })
}

#[tauri::command]
async fn test_ts_config(
    config: sources::TsConfig,
    path: Option<String>,
    app: AppHandle, analysis_context: Option<analysis_context::Identity>, source_generation: Option<u64>) -> Result<Vec<(String, String)>, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Dataset, None, crate::global_scheduler::Priority::Normal).await?;
    offload_admitted(None, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        test_ts_config_impl(state.inner(), config, path.as_deref())
    })
    .await?
}

pub(crate) fn test_ts_config_impl(
    state: &AppState,
    config: sources::TsConfig,
    path: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let source = crate::analysis_runtime::source(&state);
    let cc = config.compile()?;
    let joined_of = |ev: &Event, line: &str, idx: Option<&sources::FilePart>| {
        config
            .sources
            .iter()
            .map(|s| match (s.as_str(), idx) {
                ("arquivo", Some(i)) => std::path::Path::new(&i.path)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| i.path.clone()),
                ("caminho", Some(i)) => i.path.clone(),
                ("linha", _) => line.to_string(),
                (other, _) => ev.col_str(other).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = Vec::new();
    match &*source {
        SourceData::Indexed(idx) => {
            let visibility = analysis_runtime::indexed_gate(idx)?;
            for i in (0..idx.lines.len())
                .filter(|&i| visibility.as_ref().is_none_or(|gate| gate.allows_known_row(i)))
                .filter(|&i| path.is_none_or(|path| idx.part_at(i).path == path))
                .take(5)
            {
                let part = idx.part_at(i);
                let mut ev = sources::parse_part_line(part, sources::line_bytes(idx, i));
                let line = String::from_utf8_lossy(sources::line_bytes(idx, i)).into_owned();
                let entrada = joined_of(&ev, &line, Some(part));
                sources::apply_ts_config(&mut ev, &cc, part, &line);
                let resultado = ev
                    .timestamp
                    .map(crate::model::ts_to_iso)
                    .unwrap_or_else(|| "(não reconhecido)".into());
                out.push((entrada, resultado));
            }
        }
        // fontes unidas/em memória: testa sobre os eventos materializados
        SourceData::Memory(evs) => {
            for ev in evs.iter().take(5) {
                let mut ev = ev.clone();
                let entrada = joined_of(&ev, &ev.raw.clone(), None);
                sources::apply_ts_config_event(&mut ev, &cc);
                let resultado = ev
                    .timestamp
                    .map(crate::model::ts_to_iso)
                    .unwrap_or_else(|| "(não reconhecido)".into());
                out.push((entrada, resultado));
            }
        }
        SourceData::None => return Err("Nenhum arquivo carregado.".into()),
    }
    Ok(out)
}

// ------------------------------------------------------- campos derivados

fn derived_path() -> PathBuf {
    config_dir().join("derived_fields.json")
}

fn load_derived() -> Vec<sources::CompiledDerived> {
    let defs: Vec<sources::DerivedFieldCompat> = std::fs::read_to_string(derived_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    defs.into_iter()
        .map(|d| d.normalize())
        .filter_map(|d| {
            let rules: Vec<sources::CompiledRule> = d
                .rules
                .iter()
                .filter_map(|r| {
                    regex::Regex::new(&r.pattern)
                        .ok()
                        .and_then(|re| sources::CompiledRule::new(re, r.template.clone(), r.filter.clone()).ok())
                })
                .collect();
            if !d.steps.is_empty() && rules.len() != d.rules.len() {
                // Invalid regex extraction cannot silently become a transform
                // of the entire source when legacy parsing skips that rule.
                return None;
            }
            if rules.is_empty() && d.steps.is_empty() && d.lookup.is_none() {
                None
            } else {
                Some(sources::CompiledDerived {
                    name: d.name,
                    source: d.source,
                    rules,
                    steps: d.steps,
                    lookup: d.lookup.map(crate::reference_lookup::Compiled::new),
                })
            }
        })
        .collect()
}

#[tauri::command]
async fn list_derived_fields(analysis_context: Option<analysis_context::Identity>) -> Result<Vec<sources::DerivedField>, String> {
    offload(move || analysis_commands::list_definitions(&analysis_commands::expected_identity(analysis_context)?)).await?
}

#[tauri::command]
async fn save_derived_field(
    name: String, source: String, rules: Vec<sources::DerivedRule>,
    steps: Option<Vec<field_transform::Step>>, analysis_context: Option<analysis_context::Identity>,
) -> Result<analysis_commands::MutationReceipt, String> {
    offload(move || analysis_commands::save_definition(&analysis_commands::expected_identity(analysis_context)?, &name, &source, rules, steps)).await?
}

#[tauri::command]
async fn delete_derived_field(name: String, analysis_context: Option<analysis_context::Identity>) -> Result<analysis_commands::MutationReceipt, String> {
    offload(move || analysis_commands::delete_definition(&analysis_commands::expected_identity(analysis_context)?, &name)).await?
}

#[tauri::command]
async fn test_parse(
    kind: String,
    pattern: String,
    separator: String,
    fields: Vec<String>,
    sample: String,
) -> Result<Vec<serde_json::Value>, String> {
    offload(move || test_parse_impl(&kind, &pattern, &separator, &fields, &sample)).await?
}

pub(crate) fn test_parse_impl(
    kind: &str,
    pattern: &str,
    separator: &str,
    fields: &[String],
    sample: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let cp = build_custom_parse(kind, pattern, separator, fields)?;
    let mut out = Vec::new();
    for line in sample.lines().filter(|l| !l.trim().is_empty()).take(5) {
        let ev = sources::parse_line(line.as_bytes(), "custom", Some(&cp), &[]);
        out.push(serde_json::to_value(&ev).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn build_custom_parse(
    kind: &str,
    pattern: &str,
    separator: &str,
    fields: &[String],
) -> Result<sources::CustomParse, String> {
    if kind == "delimited" {
        let sep = separator.chars().next().unwrap_or(',');
        if fields.is_empty() {
            return Err("Informe os campos na ordem das colunas.".into());
        }
        Ok(sources::CustomParse::Delimited {
            sep,
            fields: fields.to_vec(),
        })
    } else {
        let re = regex::Regex::new(pattern).map_err(|e| format!("Regex inválida: {e}"))?;
        if re.capture_names().flatten().count() == 0 {
            return Err("A regex precisa de grupos nomeados, ex.: (?<message>.*).".into());
        }
        Ok(sources::CustomParse::Regex(re))
    }
}

// ------------------------------------------------------- formatos de log

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomFormat {
    name: String,
    #[serde(default = "kind_regex")]
    kind: String, // "regex" | "delimited"
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    separator: String,
    #[serde(default)]
    fields: Vec<String>,
}

fn kind_regex() -> String {
    "regex".into()
}

impl CustomFormat {
    fn to_parse(&self) -> Result<sources::CustomParse, String> {
        build_custom_parse(&self.kind, &self.pattern, &self.separator, &self.fields)
    }
}

fn custom_formats() -> Vec<CustomFormat> {
    analysis_runtime::current().map(|admitted| admitted.interpretation.formats.clone()).unwrap_or_default()
}

#[derive(serde::Serialize)]
pub(crate) struct FormatInfo {
    id: String,
    name: String,
}

#[tauri::command]
async fn list_formats(app: AppHandle, analysis_context: Option<analysis_context::Identity>) -> Result<Vec<FormatInfo>, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app, admitted, list_formats_impl).await
}

pub(crate) fn list_formats_impl() -> Vec<FormatInfo> {
    let mut v: Vec<FormatInfo> = [
        ("auto", "Automático (inferir)"),
        ("jsonl", "JSON / JSONL (CloudTrail, Azure, Okta, Suricata, Elastic)"),
        ("syslog3164", "Syslog (RFC 3164)"),
        ("syslog5424", "Syslog (RFC 5424)"),
        ("apache", "Apache / Nginx (common / combined)"),
        ("nginx-error", "Nginx (error.log)"),
        ("mixed", "Misto (registros estruturados por linha)"),
        ("firewall", "Firewall (iptables / netfilter)"),
        ("cef", "CEF (ArcSight)"),
        ("leef", "LEEF (QRadar)"),
        ("log4j", "Log4j / Logback"),
        ("wildfly", "WildFly / JBoss"),
        ("logfmt", "Logfmt (key=value)"),
        ("csv", "CSV / TSV (cabeçalho; separador detectado)"),
        ("w3c", "IIS / W3C"),
        ("zeek", "Zeek (TSV)"),
        ("auditd", "Linux auditd"),
        ("text", "Texto puro"),
    ]
    .into_iter()
    .map(|(id, name)| FormatInfo {
        id: id.into(),
        name: name.into(),
    })
    .collect();
    for c in custom_formats() {
        v.push(FormatInfo {
            id: format!("custom:{}", c.name),
            name: format!("{} (custom)", c.name),
        });
    }
    v
}

#[tauri::command]
async fn save_custom_format(
    name: String, kind: String, pattern: String, separator: String, fields: Vec<String>,
    app: AppHandle, analysis_context: Option<analysis_context::Identity>,
) -> Result<analysis_commands::MutationReceipt, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app, admitted, move || save_custom_format_impl(&name, &kind, &pattern, &separator, fields)).await?
}

pub(crate) fn save_custom_format_impl(name: &str, kind: &str, pattern: &str, separator: &str, fields: Vec<String>) -> Result<analysis_commands::MutationReceipt, String> {
    let admitted = case_interpretation::current()?;
    let expected = admitted.identity.as_ref().ok_or(analysis_runtime::STALE)?;
    let format = CustomFormat { name: name.into(), kind: kind.into(), pattern: pattern.into(), separator: separator.into(), fields };
    format.to_parse()?;
    let snapshot = case_interpretation::update_domain(expected, "formats", |settings| {
        settings.formats.retain(|f| f.name != name);
        settings.formats.push(format);
        Ok(())
    })?;
    Ok(analysis_commands::MutationReceipt { analysis_context: snapshot })
}

#[tauri::command]
async fn clear_events(app: AppHandle, operation_id: Option<String>, analysis_context: Option<analysis_context::Identity>, source_generation: Option<u64>) -> Result<(), String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Publish, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    // Waiting for existing source readers and releasing native sessions can
    // take time; neither belongs on the WebView's synchronous IPC callback.
    offload_admitted(operation_id, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        clear_events_impl(state.inner())
    }).await?
}

pub(crate) fn clear_events_impl(state: &AppState) -> Result<(), String> {
    source_publication::clear(state)
}

#[tauri::command]
async fn source_snapshot(app: AppHandle) -> Result<source_publication::Snapshot, String> {
    offload(move || {
        let state = app.state::<AppState>();
        source_publication::snapshot(state.inner())
    }).await
}

/// Resumo da fonte carregada no momento (para MCP/UI saberem o que há no app).
#[derive(Serialize)]
pub(crate) struct SourceSummary {
    #[serde(rename = "analysisDiagnostics")]
    analysis_diagnostics: Vec<analysis_context::Diagnostic>,
    count: usize,
    columns: Vec<String>,
    source_desc: String,
    source_names: Vec<String>,
}

#[tauri::command]
async fn source_summary(app: AppHandle, analysis_context: Option<analysis_context::Identity>, source_generation: Option<u64>) -> Result<SourceSummary, String> {
    let admitted = analysis_runtime::capture_async(app.clone(), analysis_context, source_generation, analysis_runtime::Mode::Dataset, None, crate::global_scheduler::Priority::Normal).await?;
    // Memory: a descoberta de colunas varre até 20k eventos — offload também.
    offload_admitted(None, app.clone(), admitted, move || {
        let state = app.state::<AppState>();
        source_summary_impl(state.inner())
    })
    .await?
}

pub(crate) fn source_summary_impl(state: &AppState) -> Result<SourceSummary, String> {
    let source = crate::analysis_runtime::source(&state);
    let source_names = crate::analysis_runtime::source_names(&state);
    let (count, columns, source_desc) = match &*source {
        SourceData::None => (0, vec![], String::new()),
        SourceData::Memory(evs) => (evs.len(), all_columns(evs), source_names.join(" + ")),
        SourceData::Indexed(idx) => (
            analysis_runtime::visible_total(idx)?,
            idx.columns.clone(),
            source_names.join(" + "),
        ),
    };
    Ok(SourceSummary {
        analysis_diagnostics: analysis_runtime::current().map(|admitted| (*admitted.diagnostics).clone()).unwrap_or_default(),
        count,
        columns,
        source_desc,
        source_names,
    })
}

#[tauri::command]
async fn query_events(
    filters: Vec<query::Filter>,
    sort_column: String,
    sort_dir: String,
    offset: usize,
    limit: usize,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<query::QueryResult, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Interactive).await?;
    offload_case_interactive(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        query_events_scope_impl(
            state.inner(),
            filters,
            &sort_column,
            &sort_dir,
            offset,
            limit,
            case_events.as_deref(),
        )
    })
    .await?
}

pub(crate) fn query_events_scope_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    case_events: Option<&[Event]>,
) -> Result<query::QueryResult, String> {
    let _interactive = operations::interactive();
    match case_events {
        Some(events) => Ok(query::query(
            events,
            &filters,
            sort_column,
            sort_dir,
            offset,
            limit.clamp(1, 2_000),
        )),
        None => query_events_impl(state, filters, sort_column, sort_dir, offset, limit),
    }
}

pub(crate) fn query_events_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> Result<query::QueryResult, String> {
    let source = crate::analysis_runtime::source(&state);
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    let limit = limit.clamp(1, 2_000);
    match &*source {
        SourceData::Memory(events) => {
            Ok(query::query(events, &filters, sort_column, sort_dir, offset, limit))
        }
        SourceData::Indexed(idx) => query::query_indexed(
            idx,
            &filters,
            sort_column,
            sort_dir,
            offset,
            limit,
            &codes,
            &system,
            &derived,
        ),
        SourceData::None => Ok(query::QueryResult {
            total: 0,
            rows: vec![],
        }),
    }
}

/// Interactive rows are independent of histogram/facet work and exact totals.
#[tauri::command]
async fn query_page(
    filters: Vec<query::Filter>,
    sort_column: String,
    sort_dir: String,
    offset: usize,
    limit: usize,
    cursor: Option<String>,
    operation_id: Option<String>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<query::QueryPage, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Interactive).await?;
    offload_case_interactive(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let limit = limit.clamp(1, 2_000);
        if let Some(events) = case_events.as_deref() {
            return Ok(query::QueryPage::from_exact(
                query::query(events, &filters, &sort_column, &sort_dir, offset, limit), offset, "memory", None,
            ));
        }
        let state = app.state::<AppState>();
        let source = crate::analysis_runtime::source(&state);
        let codes = crate::analysis_runtime::codes(&state);
        let system = crate::analysis_runtime::system_codes(&state);
        let derived = crate::analysis_runtime::derived(&state);
        match &*source {
            SourceData::Indexed(idx) => query::query_page_indexed(
                idx, &filters, &sort_column, &sort_dir, offset, limit, cursor.as_deref(), &codes, &system, &derived,
            ),
            SourceData::Memory(events) => Ok(query::QueryPage::from_exact(
                query::query(events, &filters, &sort_column, &sort_dir, offset, limit), offset, "memory", None,
            )),
            SourceData::None => Ok(query::QueryPage::from_exact(
                query::QueryResult { total: 0, rows: Vec::new() }, offset, "memory", None,
            )),
        }
    }).await?
}

#[tauri::command]
async fn explore_snapshot(
    filters: Vec<query::Filter>,
    sort_column: String,
    sort_dir: String,
    offset: usize,
    limit: usize,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<query::ExplorerSnapshot, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        explore_snapshot_scope_impl(
            state.inner(),
            filters,
            &sort_column,
            &sort_dir,
            offset,
            limit,
            Some(&app),
            case_events.as_deref(),
        )
    })
    .await?
}

pub(crate) fn explore_snapshot_scope_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    app: Option<&AppHandle>,
    case_events: Option<&[Event]>,
) -> Result<query::ExplorerSnapshot, String> {
    match case_events {
        Some(events) => Ok(query::explore(
            events,
            &filters,
            sort_column,
            sort_dir,
            offset,
            limit.clamp(1, 2_000),
        )),
        None => explore_snapshot_impl(state, filters, sort_column, sort_dir, offset, limit, app),
    }
}

pub(crate) fn explore_snapshot_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    app: Option<&AppHandle>,
) -> Result<query::ExplorerSnapshot, String> {
    emit_progress(
        app,
        "exploração",
        "Aplicando filtros",
        0,
        0,
        "eventos",
        false,
    );
    let source = crate::analysis_runtime::source(&state);
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    let limit = limit.clamp(1, 2_000);
    let snapshot = match &*source {
        SourceData::Memory(events) => {
            query::explore(events, &filters, sort_column, sort_dir, offset, limit)
        }
        SourceData::Indexed(idx) => query::explore_indexed(
            idx,
            &filters,
            sort_column,
            sort_dir,
            offset,
            limit,
            &codes,
            &system,
            &derived,
        )?,
        SourceData::None => query::ExplorerSnapshot {
            query: query::QueryResult {
                total: 0,
                rows: vec![],
            },
            stats: query::Stats {
                buckets: vec![],
                bucket_ms: 0,
                levels: vec![],
            },
            sources: query::AggResult {
                columns: vec![],
                rows: vec![],
                group_values: vec![],
                ..Default::default()
            },
            codes: query::AggResult {
                columns: vec![],
                rows: vec![],
                group_values: vec![],
                ..Default::default()
            },
        },
    };
    emit_progress(
        app,
        "exploração",
        "Recorte calculado",
        snapshot.query.total,
        snapshot.query.total,
        "eventos",
        false,
    );
    Ok(snapshot)
}

#[tauri::command]
async fn aggregate_events(
    group_column: String,
    aggs: Vec<query::AggSpec>,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<query::AggResult, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        Ok::<_, String>(aggregate_events_impl(state.inner(), &group_column, aggs, filters, case_events))
    })
    .await?
}

pub(crate) fn aggregate_events_impl(
    state: &AppState,
    group_column: &str,
    aggs: Vec<query::AggSpec>,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> query::AggResult {
    // eventos do Caso: aplica os filtros recebidos e agrega sobre o recorte
    if let Some(events) = case_events {
        return query::aggregate(&events, &filters, group_column, &aggs);
    }
    let source = crate::analysis_runtime::source(&state);
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    match &*source {
        SourceData::Memory(events) => query::aggregate(events, &filters, group_column, &aggs),
        SourceData::Indexed(idx) => query::aggregate_indexed(
            idx,
            &filters,
            group_column,
            &aggs,
            &codes,
            &system,
            &derived,
        ),
        SourceData::None => query::AggResult {
            columns: vec![],
            rows: vec![],
            group_values: vec![],
            ..Default::default()
        },
    }
}

/// Agregações da árvore de exploração em uma única chamada:
/// contagens por valor de cada coluna, com o filtro da própria coluna excluído.
#[derive(serde::Serialize)]
pub(crate) struct TrailResult {
    events: Vec<Event>,
    before_available: usize,
    after_available: usize,
}

/// Trilha temporal em torno de um evento: N antes, o evento, N depois.
fn trail_from_events(
    events: &[Event],
    filters: &[query::Filter],
    center_id: usize,
    before: usize,
    after: usize,
) -> TrailResult {
    let idxs = query::filtered_indices(events, filters);
    let mut rows: Vec<(i64, usize)> = idxs
        .iter()
        .map(|&i| (events[i].timestamp.unwrap_or(i64::MIN), i))
        .collect();
    rows.sort();
    let pos = rows.iter().position(|&(_, i)| events[i].id == center_id);
    let Some(pos) = pos else {
        return TrailResult {
            events: vec![],
            before_available: 0,
            after_available: 0,
        };
    };
    let start = pos.saturating_sub(before);
    let end = (pos + after + 1).min(rows.len());
    TrailResult {
        events: rows[start..end]
            .iter()
            .map(|&(_, i)| {
                let mut event = events[i].clone();
                entities::annotate(&mut event);
                event
            })
            .collect(),
        before_available: start,
        after_available: rows.len() - end,
    }
}

/// Trilha temporal de um evento no artefato ou no conjunto do Caso.
#[tauri::command]
async fn trail_events(
    center_id: usize,
    before: usize,
    after: usize,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<TrailResult, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        trail_events_impl(
            state.inner(),
            center_id,
            before,
            after,
            filters,
            case_events,
        )
    })
    .await?
}

pub(crate) fn trail_events_impl(
    state: &AppState,
    center_id: usize,
    before: usize,
    after: usize,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Result<TrailResult, String> {
    if let Some(events) = case_events {
        return Ok(trail_from_events(&events, &filters, center_id, before, after));
    }
    query::check_collected_ids(before.saturating_add(after).saturating_add(1))?;
    let source = crate::analysis_runtime::source(&state);
    Ok(match &*source {
        SourceData::Memory(events) => trail_from_events(events, &filters, center_id, before, after),
        SourceData::Indexed(idx) => {
            let empty = || TrailResult { events: vec![], before_available: 0, after_available: 0 };
            let Some(center_meta) = idx.lines.get(center_id) else { return Ok(empty()); };
            let center = (center_meta.ts, center_id);
            let (mut previous, mut following) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
            let (mut left, mut right, mut found) = (0usize, 0usize, false);
            let codes = crate::analysis_runtime::codes(&state); let system = crate::analysis_runtime::system_codes(&state); let derived = crate::analysis_runtime::derived(&state);
            query::visit_indexed_matches(idx, &filters, &codes, &system, &derived, |id| {
                let key = (idx.lines.at(id).ts, id);
                if key < center { left += 1; previous.insert(key); if previous.len() > before { previous.pop_first(); } }
                else if key > center { right += 1; following.insert(key); if following.len() > after { following.pop_last(); } }
                else { found = true; }
            })?;
            if !found { return Ok(empty()); }
            let mut budget = query::AnalyticsBudget::new();
            let mut events = Vec::new();
            for (_, id) in previous.into_iter().chain(std::iter::once(center)).chain(following) {
                let mut event = sources::event_at(idx, id, &codes, &system, &derived);
                analysis_runtime::attach_provenance(idx, &mut event)?;
                entities::annotate(&mut event);
                budget.charge(query::event_payload_bytes(&event))?;
                events.push(event);
            }
            TrailResult { events, before_available: left.saturating_sub(before), after_available: right.saturating_sub(after) }
        }
        SourceData::None => TrailResult {
            events: vec![],
            before_available: 0,
            after_available: 0,
        },
    })
}

/// Contagem simples de eventos que passam nos filtros (abas de filtros salvos).
#[tauri::command]
async fn count_filtered(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<usize, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        count_filtered_impl(state.inner(), filters, case_events)
    })
    .await?
}

pub(crate) fn count_filtered_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Result<usize, String> {
    if let Some(events) = case_events {
        return Ok(query::count_memory(&events, &filters));
    }
    let source = crate::analysis_runtime::source(&state);
    Ok(match &*source {
        SourceData::Memory(events) => query::count_memory(events, &filters),
        SourceData::Indexed(idx) => {
            let (codes, system, derived) = (crate::analysis_runtime::codes(&state), crate::analysis_runtime::system_codes(&state), crate::analysis_runtime::derived(&state));
            let src = engine::Source { idx, codes: &codes, system: &system, derived: &derived };
            match engine::count(&src, &filters)? { Some(count) => count, None => query::count_lines(idx, &filters, &codes, &system, &derived)? }
        }
        SourceData::None => 0,
    })
}

#[tauri::command]
async fn tree_aggs(
    columns: Vec<String>,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<Vec<(String, query::AggResult)>, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        Ok::<_, String>(tree_aggs_impl(state.inner(), columns, filters, case_events))
    })
    .await?
}

pub(crate) fn tree_aggs_impl(
    state: &AppState,
    columns: Vec<String>,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Vec<(String, query::AggResult)> {
    if let Some(events) = case_events {
        return query::multi_count(&events, &filters, &columns);
    }
    let source = crate::analysis_runtime::source(&state);
    match &*source {
        SourceData::Memory(events) => query::multi_count(events, &filters, &columns),
        SourceData::Indexed(idx) => query::multi_count_indexed(
            idx,
            &filters,
            &columns,
            &crate::analysis_runtime::codes(&state),
            &crate::analysis_runtime::system_codes(&state),
            &crate::analysis_runtime::derived(&state),
        ),
        SourceData::None => vec![],
    }
}

#[tauri::command]
async fn stats_events(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<query::Stats, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        if let Some(events) = case_events {
            return Ok(query::stats(&events, &filters));
        }
        let state = app.state::<AppState>();
        stats_events_impl(state.inner(), filters)
    })
    .await?
}

pub(crate) fn stats_events_impl(state: &AppState, filters: Vec<query::Filter>) -> Result<query::Stats, String> {
    let source = crate::analysis_runtime::source(&state);
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    match &*source {
        SourceData::Memory(events) => Ok(query::stats(events, &filters)),
        SourceData::Indexed(idx) => query::stats_indexed(idx, &filters, &codes, &system, &derived),
        SourceData::None => Ok(query::Stats {
            buckets: vec![],
            bucket_ms: 0,
            levels: vec![],
        }),
    }
}

pub(crate) fn event_detail_impl(state: &AppState, id: usize) -> Option<Event> {
    let _interactive = operations::interactive();
    let mut event = event_detail_raw(state, id)?;
    entities::annotate(&mut event);
    Some(event)
}

fn event_detail_raw(state: &AppState, id: usize) -> Option<Event> {
    let source = crate::analysis_runtime::source(&state);
    match &*source {
        SourceData::Memory(events) => events.get(id).filter(|event| event.id == id).or_else(|| events.iter().find(|event| event.id == id)).cloned(),
        SourceData::Indexed(idx) => {
            if id >= idx.lines.len() { return None; }
            match analysis_runtime::row_visible(idx, id) {
                Ok(true) => (), Ok(false) => return None,
                Err(error) => { analysis_runtime::record_failure(error); return None; }
            }
            let mut event = sources::event_at(idx, id, &crate::analysis_runtime::codes(&state), &crate::analysis_runtime::system_codes(&state), &analysis_runtime::derived(state));
            if let Err(error) = analysis_runtime::attach_provenance(idx, &mut event) { analysis_runtime::record_failure(error); return None; }
            Some(event)
        }
        SourceData::None => None,
    }
}

// ------------------------------------------------------------------ análise

#[tauri::command]
async fn discover_patterns(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<discovery::Discovery, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| { workspace::validate(&filters)?; discover_patterns_impl(app.state::<AppState>().inner(), filters, case_events) })
        .await?
}

pub(crate) fn discover_patterns_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Result<discovery::Discovery, String> {
    let memory = |events: &[Event]| {
        let prepared = query::prepare(&filters);
        let mut sample = discovery::Sampler::new();
        let mut result = discovery::Discovery::default();
        for (i, ev) in events.iter().enumerate() {
            if operations::cancelled() {
                break;
            }
            if prepared.iter().all(|pf| query::matches(ev, pf)) {
                result.observe(ev.timestamp, insights::is_error(ev), ev.level == "Aviso");
                sample.push(i);
            }
        }
        sample.ids.sort_unstable();
        discovery::analyze(result, || sample.ids.iter().map(|&i| events[i].clone()))
    };
    if let Some(events) = case_events {
        return Ok(memory(&events));
    }
    let source = crate::analysis_runtime::source(&state);
    Ok(match &*source {
        SourceData::Memory(events) => memory(events),
        SourceData::Indexed(idx) => {
            let codes = crate::analysis_runtime::codes(&state);
            let system = crate::analysis_runtime::system_codes(&state);
            let derived = crate::analysis_runtime::derived(&state);
            let mut sample = discovery::Sampler::new();
            let mut result = discovery::Discovery::default();
            query::visit_indexed_matches(idx, &filters, &codes, &system, &derived, |i| {
                let meta = &idx.lines.at(i);
                result.observe(
                    (meta.ts != 0).then_some(meta.ts),
                    matches!(meta.level, model::LV_ERR | model::LV_CRIT),
                    meta.level == model::LV_WARN,
                );
                sample.push(i);
            })?;
            sample.ids.sort_unstable();
            discovery::analyze(result, || {
                sample
                    .ids
                    .iter()
                    .map(|&i| sources::event_at(idx, i, &codes, &system, &derived))
            })
        }
        SourceData::None => discovery::analyze(discovery::Discovery::default(), std::iter::empty),
    })
}

const ANALYSIS_CAP: usize = 3_000; // Distributed sample used only by field profiling.

fn work_events(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Result<Vec<Event>, String> {
    if let Some(events) = case_events {
        let matched = query::filtered_indices(&events, &filters);
        return Ok((0..matched.len().min(ANALYSIS_CAP))
            .map(|s| events[matched[s * matched.len() / matched.len().min(ANALYSIS_CAP)]].clone())
            .collect());
    }
    let source = crate::analysis_runtime::source(&state);
    let codes = crate::analysis_runtime::codes(&state);
    let system = crate::analysis_runtime::system_codes(&state);
    let derived = crate::analysis_runtime::derived(&state);
    Ok(match &*source {
        SourceData::Indexed(idx) => {
            let src = engine::Source { idx, codes: &codes, system: &system, derived: &derived };
            let total = match engine::count(&src, &filters)? { Some(count) => count, None => query::count_lines(idx, &filters, &codes, &system, &derived)? };
            let limit = total.min(ANALYSIS_CAP);
            let mut ids = Vec::with_capacity(limit);
            let mut position = 0usize;
            query::visit_indexed_matches(idx, &filters, &codes, &system, &derived, |id| {
                if ids.len() < limit && position == ids.len() * total / limit { ids.push(id); }
                position += 1;
            })?;
            ids.into_iter().map(|id| sources::event_at(idx, id, &codes, &system, &derived)).collect()
        }
        SourceData::Memory(events) => {
            let matched = query::filtered_indices(events, &filters);
            (0..matched.len().min(ANALYSIS_CAP))
                .map(|s| {
                    events[matched[s * matched.len() / matched.len().min(ANALYSIS_CAP)]].clone()
                })
                .collect()
        }
        SourceData::None => vec![],
    })
}

fn work_columns(evs: &[Event]) -> Vec<String> {
    let mut cols: Vec<String> = model::STANDARD_COLUMNS
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut extra = std::collections::HashSet::new();
    for ev in evs {
        for k in ev.fields.keys() {
            extra.insert(k.clone());
        }
    }
    let mut extra: Vec<String> = extra.into_iter().collect();
    extra.sort();
    cols.extend(extra);
    cols
}

#[tauri::command]
async fn profile_fields(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<Vec<analysis::FieldProfile>, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        profile_fields_impl(state.inner(), filters, case_events)
    })
    .await?
}

pub(crate) fn profile_fields_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Result<Vec<analysis::FieldProfile>, String> {
    let evs = work_events(state, filters, case_events)?;
    let columns = work_columns(&evs);
    Ok(analysis::profile_fields(&evs, &columns))
}

#[tauri::command]
async fn compute_series(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    spec: analysis::SeriesSpec,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<analysis::SeriesResult, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        compute_series_impl(state.inner(), filters, case_events, spec)
    })
    .await?
}

pub(crate) fn compute_series_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    spec: analysis::SeriesSpec,
) -> Result<analysis::SeriesResult, String> {
    if let Some(events) = case_events {
        let prepared = query::prepare(&filters);
        return analysis::compute_series_stream(|| events.iter().filter(|event| prepared.iter().all(|pf| query::matches(event, pf))).cloned(), &spec);
    }
    if let Some(result) = workspace::with_engine(state, |source| engine::series(source, &query::prepare(&filters), &spec))? { return Ok(result); }
    workspace::with_selection(state, &filters, |selection| analysis::compute_series_stream(|| selection.iter(), &spec))?
}

#[tauri::command]
async fn pivot(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    spec: analysis::PivotSpec,
    app: AppHandle,
    operation_id: Option<String>,
) -> Result<analysis::PivotResult, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        workspace::validate(&filters)?;
        let state = app.state::<AppState>();
        pivot_impl(state.inner(), filters, case_events, spec)
    })
    .await?
}

pub(crate) fn pivot_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    spec: analysis::PivotSpec,
) -> Result<analysis::PivotResult, String> {
    if let Some(events) = case_events {
        let prepared = query::prepare(&filters);
        return analysis::pivot_stream(events.iter().filter(|event| prepared.iter().all(|pf| query::matches(event, pf))).cloned(), &spec);
    }
    if let Some(result) = workspace::with_engine(state, |source| engine::pivot(source, &query::prepare(&filters), &spec))? { return Ok(result); }
    workspace::with_selection(state, &filters, |selection| analysis::pivot_stream(selection.iter(), &spec))?
}

pub(crate) async fn case_editor_admission_async(app: AppHandle, identity: Option<analysis_context::Identity>) -> Result<std::sync::Arc<analysis_runtime::Admitted>, String> {
    let identity = analysis_commands::expected_identity(identity)?;
    analysis_runtime::capture_async(app, Some(identity), None, analysis_runtime::Mode::Metadata, None, global_scheduler::Priority::Normal).await
}

pub(crate) fn case_editor_admission(state: &AppState, identity: Option<analysis_context::Identity>) -> Result<std::sync::Arc<analysis_runtime::Admitted>, String> {
    let identity = analysis_commands::expected_identity(identity)?;
    analysis_runtime::capture(state, Some(identity), None, analysis_runtime::Mode::Metadata)
}

#[tauri::command]
async fn get_codes(app: AppHandle, analysis_context: Option<analysis_context::Identity>) -> Result<String, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app.clone(), admitted, move || get_codes_impl(app.state::<AppState>().inner())).await
}
pub(crate) fn get_codes_impl(state: &AppState) -> String {
    serde_json::to_string_pretty(&*analysis_runtime::codes(state)).unwrap_or_default()
}

// Retained for legacy template publishers. Case-effective dictionaries are
// immutable and never share the profile's mutable dictionaries.
static CATALOG_PUBLICATION: parking_lot::RwLock<()> = parking_lot::RwLock::new(());
pub(crate) fn catalog_read_guard() -> Result<parking_lot::RwLockReadGuard<'static, ()>, String> {
    CATALOG_PUBLICATION.try_read().ok_or_else(|| "CATALOG_UPDATE_PENDING: O catálogo está sendo atualizado. Aguarde e recarregue a consulta.".into())
}
#[tauri::command]
async fn save_codes(text: String, app: AppHandle, analysis_context: Option<analysis_context::Identity>) -> Result<analysis_commands::MutationReceipt, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app.clone(), admitted, move || save_codes_impl(app.state::<AppState>().inner(), &text)).await?
}
pub(crate) fn save_codes_impl(_state: &AppState, text: &str) -> Result<analysis_commands::MutationReceipt, String> {
    if text.len() > 3 << 20 { return Err("O catálogo excede o limite de 3 MiB de configuração do Caso.".into()); }
    let cfg: CodesConfig = serde_json::from_str(text).map_err(|e| format!("JSON inválido: {e}"))?;
    let admitted = case_interpretation::current()?;
    let snapshot = case_interpretation::update_domain(admitted.identity.as_ref().ok_or(analysis_runtime::STALE)?, "codes", |settings| { settings.codes = cfg; Ok(()) })?;
    Ok(analysis_commands::MutationReceipt { analysis_context: snapshot })
}

#[derive(serde::Serialize)]
pub(crate) struct HarvestSummary {
    count: usize,
    sources: usize,
    #[serde(rename = "analysisContext")]
    #[serde(serialize_with = "crate::analysis_context::serialize_management_snapshot")]
    analysis_context: analysis_context::Snapshot,
}

/// Extrai (ou reextrai) o catálogo completo de eventos do sistema operacional.
#[tauri::command]
async fn harvest_codes(app: AppHandle, analysis_context: Option<analysis_context::Identity>) -> Result<HarvestSummary, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app.clone(), admitted, move || harvest_codes_impl(app.state::<AppState>().inner())).await?
}

pub(crate) fn harvest_codes_impl(_state: &AppState) -> Result<HarvestSummary, String> {
    let admitted = case_interpretation::current()?;
    let (cfg, count) = sources::harvest_system_codes()?;
    let sources = cfg.sources.len();
    let analysis_context = case_interpretation::update_domain(admitted.identity.as_ref().ok_or(analysis_runtime::STALE)?, "systemCodes", |settings| { settings.system_codes = cfg; Ok(()) })?;
    Ok(HarvestSummary { count, sources, analysis_context })
}

#[tauri::command]
async fn system_codes_count(app: AppHandle, analysis_context: Option<analysis_context::Identity>) -> Result<usize, String> {
    let admitted = case_editor_admission_async(app.clone(), analysis_context).await?;
    offload_admitted(None, app.clone(), admitted, move || system_codes_count_impl(app.state::<AppState>().inner())).await
}

pub(crate) fn system_codes_count_impl(state: &AppState) -> usize {
    analysis_runtime::system_codes(state)
        .sources
        .values()
        .map(|m| m.len())
        .sum()
}

#[tauri::command]
async fn cases_load() -> Result<serde_json::Value, String> {
    offload(cases_load_impl).await?
}
pub(crate) fn cases_load_impl() -> Result<serde_json::Value, String> {
    case_store::load()
}
#[tauri::command]
async fn cases_save(data: serde_json::Value, app: AppHandle) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        cases_save_impl(app.state::<AppState>().inner(), data)
    })
    .await
    .map_err(|e| e.to_string())?
}
pub(crate) fn cases_save_impl(
    state: &AppState,
    data: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let _guard = state.case_store_lock.lock();
    case_store::save(data)
}

#[tauri::command]
fn get_codes_path(state: State<AppState>) -> String {
    state.codes_path.display().to_string()
}

/// Interface scale of the window (webview zoom, like the browser's zoom).
/// Returns false where the platform cannot zoom, so the interface keeps 100%.
#[tauri::command]
fn ui_zoom(window: tauri::WebviewWindow, scale: f64) -> bool {
    scale.is_finite() && window.set_zoom(scale.clamp(0.5, 2.0)).is_ok()
}

/// Status do servidor MCP embutido (para a tela de configurações).
#[tauri::command]
fn mcp_status(mcp: State<mcp::McpState>) -> mcp::McpStatus {
    mcp::status(&mcp)
}

/// Relança o aplicativo com privilégios de administrador (UAC) e encerra
/// a instância atual. Necessário para canais restritos como o Security.
#[tauri::command]
fn relaunch_elevated() -> Result<(), String> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let ps_cmd = if cfg!(debug_assertions) {
            // Em dev o exe depende do dev server do Tauri CLI, então eleva
            // a cadeia inteira (`npx tauri dev` na raiz do projeto).
            let root = exe
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .ok_or("Não foi possível localizar a raiz do projeto.")?;
            format!(
                "Start-Process cmd.exe -ArgumentList '/c','cd /d \"{}\" && npx tauri dev' -Verb RunAs",
                root.display()
            )
        } else {
            // Em produção os assets estão embutidos: basta relançar o exe.
            format!("Start-Process -FilePath '{}' -Verb RunAs", exe.display())
        };
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps_cmd])
            .spawn()
            .map_err(|e| format!("Falha ao solicitar elevação: {e}"))?;
        std::thread::sleep(std::time::Duration::from_millis(600));
        std::process::exit(0);
    }
    #[cfg(not(windows))]
    {
        Err("Elevação só é necessária no Windows.".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(feature = "update-e2e")]
    updates::e2e_arguments();
    // Resolve a verified restart selection before any data, updater, catalog,
    // MCP or cache initialization. This process then retains one fixed root.
    case_recovery::profiles::initialize(&base_config_dir(), &case_recovery::Work {
        cancelled: &|| false,
        progress: &|_, _, _| {},
    });
    let context = tauri::generate_context!();
    // Before anything reads the data folder: a new version backs it up first.
    let updates = updates::prepare(&context.package_info().version);
    // Resolve the selected profile and preserve updater backup order before
    // freezing the restart-only budget or starting any shared parser workers.
    resource_settings::initialize(&config_dir());
    resources::init();
    let codes_path = config_dir().join("codes.json");
    let codes = load_codes(&codes_path);
    let system_codes_path = config_dir().join("system_codes.json");
    let system_codes = load_system_codes(&system_codes_path);
    let (mcp_enabled, mcp_port, mcp_config_path, mcp_token) = mcp::load_config();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates)
        .on_window_event(updates::on_window_event)
        .manage(AppState {
            source: RwLock::new(SourceData::None),
            source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(vec![]),
            derived: RwLock::new(load_derived()),
            codes: RwLock::new(codes),
            system_codes: RwLock::new(system_codes),
            case_store_lock: Mutex::new(()),
            codes_path,
            system_codes_path,
        })
        .manage(mcp::McpState::new(
            mcp_enabled,
            mcp_port,
            mcp_config_path,
            mcp_token,
        ))
        .setup(move |app| {
            // No startup catalog writes/automatic harvest. The local v0.11
            // migration must see original profile files unchanged; explicit
            // harvest saves only the selected Case after migration and CAS.
            // Old parser caches and unused indexes do not accumulate on disk.
            std::thread::spawn(index_cache::prune);
            // Servidor MCP embutido (loopback) para automação por agentes.
            if mcp_enabled {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    mcp::serve(handle, mcp_port).await;
                });
            }
            #[cfg(feature = "update-e2e")]
            updates::run_e2e(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ui_zoom,
            resource_settings::resource_settings_status,
            resource_settings::resource_settings_save,
            resource_settings::case_resource_settings_status,
            resource_settings::case_resource_settings_save,
            case_images::case_image_add,
            case_images::case_image_read,
            case_images::export_investigation,
            remote::remote_list,
            remote::remote_save,
            remote::remote_delete,
            remote::remote_test,
            remote::remote_import,
            timeline_export::export_timeline,
            threats::threat_catalog,
            threats::threat_catalog_update,
            threats::threat_scan,
            threats::threat_events,
            journeys::journey_fields,
            journeys::journey_index,
            journeys::journey_events,
            mcp::mcp_configure,
            workspace::dataset_overview,
            triage::triage,
            triage::triage_episode,
            triage::triage_evidence_event,
            triage::triage_timeline,
            triage::event_insights,
            triage::normalization_preview,
            triage::detection_rules,
            triage::detection_settings_save,
            triage::sigma_import,
            triage::sigma_clear,
            case_cache::case_sync,
            pivots::timeline_lanes,
            pivots::entity_summary,
            pivots::ioc_sightings,
            pivots::source_hashes,
            workspace::timeline_range,
            workspace::grouped_timeline,
            workspace::import_investigation,
            workspace::load_bundle,
            workspace::compare_periods,
            workspace::list_sources,
            workspace::cancel_operation,
            cancel_task,
            engine_status,
            engine_retry,
            workspace::validate_filters,
            workspace::export_events,
            workspace::export_document,
            workspace::expand_paths,
            list_channels,
            load_event_log,
            load_file,
            load_files,
            clear_events,
            source_summary,
            source_snapshot,
            query_events,
            query_page,
            explore_snapshot,
            aggregate_events,
            trail_events,
            count_filtered,
            tree_aggs,
            stats_events,
            detail_commands::event_detail,
            detail_commands::java_trace_detail,
            get_codes,
            save_codes,
            get_codes_path,
            relaunch_elevated,
            harvest_codes,
            system_codes_count,
            cases_load,
            cases_save,
            case_evidence_commands::cases_load_view,
            case_evidence_commands::cases_save_view,
            case_evidence_commands::case_evidence_prepare,
            case_evidence_commands::case_evidence_prepare_membership,
            case_evidence_commands::case_evidence_discard,
            case_evidence_commands::case_evidence_member_detail,
            case_evidence_commands::case_evidence_member_field_text,
            case_evidence_commands::case_evidence_member_java_trace,
            case_evidence_commands::case_evidence_timeline,
            case_evidence_commands::case_evidence_display_timeline,
            case_evidence_commands::case_evidence_display_members,
            case_evidence_commands::case_evidence_preview,
            case_evidence_commands::case_evidence_source_receipt,
            case_evidence_commands::case_evidence_find_members,
            case_evidence_commands::case_evidence_open,
            case_portable_commands::case_import_native,
            case_portable_commands::case_export_native,
            case_recovery_commands::case_recovery_status,
            case_recovery_commands::case_recovery_prepare_restart,
            case_recovery_commands::case_recovery_return_original,
            list_formats,
            save_custom_format,
            get_ts_config,
            set_ts_config,
            test_ts_config,
            test_parse,
            analysis_commands::analysis_context_snapshot,
            analysis_commands::analysis_context_update,
            analysis_commands::preview_field_transform,
            reference_commands::reference_inspect,
            reference_commands::reference_import,
            reference_commands::reference_list,
            reference_commands::reference_remove,
            reference_commands::reference_save_lookup,
            projection_commands::analysis_field_text,
            projection_commands::query_projected_page,
            projection_commands::hydrate_projected_rows,
            projection_commands::hydrate_projected_field,
            analysis_runtime::exclusion_visibility,
            exclusion_commands::exclusion_capabilities,
            exclusion_commands::exclusion_preview,
            exclusion_commands::exclusion_commit,
            exclusion_commands::exclusion_discard,
            exclusion_commands::exclusion_list,
            exclusion_commands::exclusion_archive_page,
            exclusion_commands::exclusion_restore_batch,
            exclusion_commands::exclusion_restore_selected,

            list_derived_fields,
            save_derived_field,
            delete_derived_field,
            profile_fields,
            discover_patterns,
            compute_series,
            pivot,
            mcp_status,
            updates::update_status,
            updates::update_startup,
            updates::update_check,
            updates::update_download,
            updates::update_cancel,
            updates::update_install,
            updates::update_install_on_close,
            updates::update_set_check_on_start,
            updates::update_skip,
            updates::update_open_page,
        ])
        .run(context)
        .expect("erro ao iniciar o LogInsight");
}

#[cfg(test)]
mod security_tests;
