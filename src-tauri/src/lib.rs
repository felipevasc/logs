mod analysis;
mod mcp;
mod model;
mod query;
mod sources;

use model::{CodesConfig, Event, STANDARD_COLUMNS};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::collections::HashSet;
use std::io::Write;
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
    count: usize,
    columns: Vec<String>,
    source_desc: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationProgress {
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
    let _ = app.emit("operation-progress", OperationProgress {
        operation: operation.into(),
        phase: phase.into(),
        completed,
        total,
        unit: unit.into(),
        cancellable,
    });
}

pub(crate) fn config_dir() -> PathBuf {
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
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

const CATALOG_VERSION: u32 = 3;

fn load_codes(path: &PathBuf) -> CodesConfig {
    let mut cfg = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<CodesConfig>(&text).ok())
        .unwrap_or_default();

    // Catálogo versionado: mescla entradas novas do padrão embutido sem
    // sobrescrever o que o usuário personalizou.
    let version_path = path.with_file_name("codes.version");
    let version = std::fs::read_to_string(&version_path)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if version < CATALOG_VERSION {
        let default: CodesConfig =
            serde_json::from_str(include_str!("default_codes.json")).unwrap_or_default();
        for (source, codes) in default.sources {
            let entry = cfg.sources.entry(source).or_default();
            for (code, info) in codes {
                entry.entry(code).or_insert(info);
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(path, text);
        }
        let _ = std::fs::write(&version_path, CATALOG_VERSION.to_string());
    }
    cfg
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

/// Carrega o catálogo extraído do sistema, se existir. A extração inicial
/// acontece em background na primeira execução (ver `run`).
fn load_system_codes(path: &PathBuf) -> CodesConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<CodesConfig>(&text).ok())
        .unwrap_or_default()
}

/// Materializa todos os eventos da fonte atual (para união com outra fonte).
fn materialize_current(state: &AppState) -> Vec<Event> {
    let source = state.source.read();
    match &*source {
        SourceData::Memory(evs) => evs.clone(),
        SourceData::Indexed(idx) => analysis::materialize_indexed(
            idx,
            &[],
            &CodesConfig::default(),
            &CodesConfig::default(),
            &[],
            usize::MAX,
        ),
        SourceData::None => vec![],
    }
}

fn store_events(state: &AppState, mut evs: Vec<Event>, source_desc: String) -> LoadSummary {
    let codes = state.codes.read().clone();
    let system = state.system_codes.read().clone();
    let derived = state.derived.read();
    for (i, ev) in evs.iter_mut().enumerate() {
        ev.id = i;
        ev.enrich(&codes, &system);
        sources::apply_derived(ev, &derived);
    }
    let columns = all_columns(&evs);
    let count = evs.len();
    *state.source.write() = SourceData::Memory(evs);
    *state.source_names.write() = vec![source_desc.clone()];
    LoadSummary {
        count,
        columns,
        source_desc,
    }
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
) -> Result<LoadSummary, String> {
    offload(move || {
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
    emit_progress(app, "carregamento", "Lendo Event Log", 0, max_events, "eventos", false);
    let evs = sources::read_channel(channel, max_events)?;
    emit_progress(app, "carregamento", "Enriquecendo eventos", evs.len(), max_events, "eventos", false);
    let desc = format!("Event Log: {channel}");
    let summary = if merge.unwrap_or(false) && !matches!(&*state.source.read(), SourceData::None) {
        let mut all = materialize_current(state);
        all.extend(evs);
        let mut names = state.source_names.read().clone();
        names.push(desc);
        let desc = names.join(" + ");
        *state.source_names.write() = names;
        store_events_merged(state, all, desc)
    } else {
        store_events(state, evs, desc)
    };
    emit_progress(app, "carregamento", "Concluído", summary.count, summary.count, "eventos", false);
    Ok(summary)
}

/// igual a store_events, mas preserva a lista de fontes unidas (já atualizada pelo chamador)
fn store_events_merged(state: &AppState, evs: Vec<Event>, source_desc: String) -> LoadSummary {
    let names = state.source_names.read().clone();
    let summary = store_events(state, evs, source_desc);
    *state.source_names.write() = names;
    summary
}

/// Indexa um arquivo aplicando formato customizado e config de data/hora salva.
fn index_source_file(path: &str, format: &str, app: Option<&AppHandle>) -> Result<sources::FileIndex, String> {
    let custom = if let Some(name) = format.strip_prefix("custom:") {
        let f = custom_formats()
            .into_iter()
            .find(|f| f.name == name)
            .ok_or_else(|| format!("Formato customizado '{name}' não encontrado."))?;
        Some(f.to_parse()?)
    } else {
        None
    };
    let saved_ts = load_ts_config(path)
        .map(|c| c.compile())
        .transpose()?;
    let file_label = path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
    let mut idx = sources::index_file(path, format, custom, saved_ts, Some(&|done, total| {
        emit_progress(app, "carregamento", &format!("Indexando {file_label}"), done, total, "linhas", false);
    }))?;
    if idx.ts_config.is_some() {
        sources::retimestamp_index(&mut idx, None);
    }
    Ok(idx)
}

#[tauri::command]
async fn load_file(
    path: String,
    format: String,
    merge: Option<bool>,
    app: AppHandle,
) -> Result<LoadSummary, String> {
    offload(move || {
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
    emit_progress(app, "carregamento", "Preparando arquivo", 0, 0, "linhas", false);
    let idx = index_source_file(path, format, app)?;
    emit_progress(app, "carregamento", "Indexando linhas", idx.lines.len(), idx.lines.len(), "linhas", false);
    if merge.unwrap_or(false) && !matches!(&*state.source.read(), SourceData::None) {
        // união: materializa a fonte atual + o novo arquivo em uma única fonte em memória
        let mut all = materialize_current(state);
        all.extend(analysis::materialize_indexed(
            &idx,
            &[],
            &CodesConfig::default(),
            &CodesConfig::default(),
            &[],
            usize::MAX,
        ));
        let mut names = state.source_names.read().clone();
        names.push(format!("Arquivo: {path}"));
        let desc = names.join(" + ");
        *state.source_names.write() = names;
        let summary = store_events_merged(state, all, desc);
        emit_progress(app, "carregamento", "Concluído", summary.count, summary.count, "eventos", false);
        return Ok(summary);
    }
    let summary = LoadSummary {
        count: idx.lines.len(),
        columns: idx.columns.clone(),
        source_desc: format!("Arquivo: {path}"),
    };
    *state.source.write() = SourceData::Indexed(idx);
    *state.source_names.write() = vec![summary.source_desc.clone()];
    emit_progress(app, "carregamento", "Concluído", summary.count, summary.count, "eventos", false);
    Ok(summary)
}

#[tauri::command]
async fn load_files(
    paths: Vec<String>,
    format: String,
    merge: Option<bool>,
    app: AppHandle,
) -> Result<LoadSummary, String> {
    offload(move || {
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
    if paths.is_empty() {
        return Err("Nenhum arquivo selecionado.".into());
    }
    let merge = merge.unwrap_or(false) && !matches!(&*state.source.read(), SourceData::None);
    let mut all = if merge { materialize_current(state) } else { vec![] };
    let mut names = if merge { state.source_names.read().clone() } else { vec![] };
    for path in paths {
        emit_progress(app, "carregamento", "Preparando arquivo", 0, 0, "linhas", false);
        let idx = index_source_file(path, format, app)?;
        emit_progress(
            app,
            "carregamento",
            "Indexando linhas",
            idx.lines.len(),
            idx.lines.len(),
            "linhas",
            false,
        );
        all.extend(analysis::materialize_indexed(
            &idx,
            &[],
            &CodesConfig::default(),
            &CodesConfig::default(),
            &[],
            usize::MAX,
        ));
        names.push(format!("Arquivo: {path}"));
    }
    let desc = names.join(" + ");
    *state.source_names.write() = names;
    let summary = store_events_merged(state, all, desc);
    emit_progress(app, "carregamento", "Concluído", summary.count, summary.count, "eventos", false);
    Ok(summary)
}

// ------------------------------------------------------- data/hora (TsConfig)

fn ts_configs_path() -> PathBuf {
    config_dir().join("ts_configs.json")
}

pub(crate) fn load_ts_config(path: &str) -> Option<sources::TsConfig> {
    let map: std::collections::HashMap<String, sources::TsConfig> =
        std::fs::read_to_string(ts_configs_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())?;
    map.get(path).cloned()
}

#[tauri::command]
fn get_ts_config(path: String) -> Option<sources::TsConfig> {
    load_ts_config(&path)
}

#[tauri::command]
async fn set_ts_config(
    path: String,
    config: Option<sources::TsConfig>,
    app: AppHandle,
) -> Result<(), String> {
    offload(move || {
        let state = app.state::<AppState>();
        set_ts_config_impl(state.inner(), &path, config, Some(&app))
    })
    .await?
}

pub(crate) fn set_ts_config_impl(
    state: &AppState,
    path: &str,
    config: Option<sources::TsConfig>,
    app: Option<&AppHandle>,
) -> Result<(), String> {
    // persiste por caminho de arquivo
    let mut map: std::collections::HashMap<String, sources::TsConfig> =
        std::fs::read_to_string(ts_configs_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    match &config {
        Some(c) => {
            map.insert(path.to_string(), c.clone());
        }
        None => {
            map.remove(path);
        }
    }
    let text = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
    std::fs::write(ts_configs_path(), text).map_err(|e| e.to_string())?;

    // aplica na fonte atual se for o mesmo arquivo
    let compiled = config.as_ref().map(|c| c.compile()).transpose()?;
    match &mut *state.source.write() {
        SourceData::Indexed(idx) => {
            if idx.path == path {
                idx.ts_config = compiled;
                sources::retimestamp_index(idx, Some(&|done, total| {
                    emit_progress(app, "data/hora", "Recalculando timestamps", done, total, "linhas", false);
                }));
            }
        }
        // fontes unidas/em memória: aplica aos eventos cujo arquivo de origem é este
        SourceData::Memory(evs) => {
            let single = state.source_names.read().len() <= 1;
            let total = evs.len();
            for (i, ev) in evs.iter_mut().enumerate() {
                let ev_path = ev.col_str("caminho").unwrap_or_default();
                if ev_path == path || (ev_path.is_empty() && single) {
                    if let Some(cc) = &compiled {
                        sources::apply_ts_config_event(ev, cc);
                    }
                }
                if i % 4096 == 0 {
                    emit_progress(app, "data/hora", "Recalculando timestamps", i, total, "eventos", false);
                }
            }
            emit_progress(app, "data/hora", "Data/hora aplicada", total, total, "eventos", false);
        }
        SourceData::None => {}
    }
    Ok(())
}

#[tauri::command]
async fn test_ts_config(
    config: sources::TsConfig,
    app: AppHandle,
) -> Result<Vec<(String, String)>, String> {
    offload(move || {
        let state = app.state::<AppState>();
        test_ts_config_impl(state.inner(), config)
    })
    .await?
}

pub(crate) fn test_ts_config_impl(
    state: &AppState,
    config: sources::TsConfig,
) -> Result<Vec<(String, String)>, String> {
    let source = state.source.read();
    let cc = config.compile()?;
    let joined_of = |ev: &Event, line: &str, idx: Option<&sources::FileIndex>| {
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
            for i in 0..idx.lines.len().min(5) {
                let mut ev = sources::parse_line(
                    sources::line_bytes(idx, i),
                    &idx.format,
                    idx.custom.as_ref(),
                    &idx.header,
                );
                let line = String::from_utf8_lossy(sources::line_bytes(idx, i)).into_owned();
                sources::apply_ts_config(&mut ev, &cc, idx, &line);
                let entrada = joined_of(&ev, &line, Some(idx));
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
                    regex::Regex::new(&r.pattern).ok().map(|re| sources::CompiledRule {
                        re,
                        template: r.template.clone(),
                        filter: r.filter.clone(),
                    })
                })
                .collect();
            if rules.is_empty() {
                None
            } else {
                Some(sources::CompiledDerived { name: d.name, source: d.source, rules })
            }
        })
        .collect()
}

#[tauri::command]
fn list_derived_fields(state: State<AppState>) -> Vec<sources::DerivedField> {
    list_derived_fields_impl(state.inner())
}

pub(crate) fn list_derived_fields_impl(state: &AppState) -> Vec<sources::DerivedField> {
    let defs: Vec<sources::DerivedFieldCompat> = std::fs::read_to_string(derived_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if !defs.is_empty() {
        return defs.into_iter().map(|d| d.normalize()).collect();
    }
    state
        .derived
        .read()
        .iter()
        .map(|d| sources::DerivedField {
            name: d.name.clone(),
            source: d.source.clone(),
            rules: d
                .rules
                .iter()
                .map(|r| sources::DerivedRule {
                    pattern: r.re.as_str().to_string(),
                    template: r.template.clone(),
                    filter: r.filter.clone(),
                })
                .collect(),
        })
        .collect()
}

#[tauri::command]
async fn save_derived_field(
    name: String,
    source: String,
    rules: Vec<sources::DerivedRule>,
    app: AppHandle,
) -> Result<(), String> {
    offload(move || {
        let state = app.state::<AppState>();
        save_derived_field_impl(state.inner(), &name, &source, rules)
    })
    .await?
}

pub(crate) fn save_derived_field_impl(
    state: &AppState,
    name: &str,
    source: &str,
    rules: Vec<sources::DerivedRule>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Informe um nome para o campo.".into());
    }
    if rules.is_empty() {
        return Err("Informe ao menos uma regra (regex).".into());
    }
    let compiled: Vec<sources::CompiledRule> = rules
        .iter()
        .map(|r| {
            regex::Regex::new(&r.pattern)
                .map(|re| sources::CompiledRule {
                    re,
                    template: r.template.clone(),
                    filter: r.filter.clone(),
                })
                .map_err(|e| format!("Regex inválida em regra ({}): {e}", r.pattern))
        })
        .collect::<Result<_, _>>()?;
    let mut defs: Vec<sources::DerivedFieldCompat> = std::fs::read_to_string(derived_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut defs: Vec<sources::DerivedField> =
        defs.into_iter().map(|d| d.normalize()).collect();
    defs.retain(|d| d.name != name);
    defs.push(sources::DerivedField {
        name: name.to_string(),
        source: source.to_string(),
        rules,
    });
    let text = serde_json::to_string_pretty(&defs).map_err(|e| e.to_string())?;
    std::fs::write(derived_path(), text).map_err(|e| e.to_string())?;
    state.derived.write().retain(|d| d.name != name);
    state.derived.write().push(sources::CompiledDerived {
        name: name.to_string(),
        source: source.to_string(),
        rules: compiled,
    });
    // fonte em memória (união/Event Log): aplica já; indexada aplica por linha sob demanda
    if let SourceData::Memory(evs) = &mut *state.source.write() {
        let derived = state.derived.read();
        for ev in evs.iter_mut() {
            sources::apply_derived(ev, &derived);
        }
    }
    Ok(())
}

#[tauri::command]
async fn delete_derived_field(name: String, app: AppHandle) -> Result<(), String> {
    offload(move || {
        let state = app.state::<AppState>();
        delete_derived_field_impl(state.inner(), &name)
    })
    .await?
}

pub(crate) fn delete_derived_field_impl(state: &AppState, name: &str) -> Result<(), String> {
    let defs: Vec<sources::DerivedFieldCompat> = std::fs::read_to_string(derived_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut defs: Vec<sources::DerivedField> =
        defs.into_iter().map(|d| d.normalize()).collect();
    defs.retain(|d| d.name != name);
    let text = serde_json::to_string_pretty(&defs).map_err(|e| e.to_string())?;
    std::fs::write(derived_path(), text).map_err(|e| e.to_string())?;
    state.derived.write().retain(|d| d.name != name);
    // remove o campo dos eventos em memória imediatamente
    if let SourceData::Memory(evs) = &mut *state.source.write() {
        for ev in evs.iter_mut() {
            ev.fields.remove(name);
        }
    }
    Ok(())
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

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CustomFormat {
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
    std::fs::read_to_string(config_dir().join("formats.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

#[derive(serde::Serialize)]
pub(crate) struct FormatInfo {
    id: String,
    name: String,
}

#[tauri::command]
fn list_formats() -> Vec<FormatInfo> {
    list_formats_impl()
}

pub(crate) fn list_formats_impl() -> Vec<FormatInfo> {
    let mut v: Vec<FormatInfo> = [
        ("auto", "Automático (inferir)"),
        ("jsonl", "JSON / Elastic (ECS)"),
        ("syslog3164", "Syslog (RFC 3164)"),
        ("syslog5424", "Syslog (RFC 5424)"),
        ("apache", "Apache / Nginx (combined)"),
        ("firewall", "Firewall (iptables / netfilter)"),
        ("cef", "CEF (ArcSight)"),
        ("leef", "LEEF (QRadar)"),
        ("log4j", "Log4j / Logback"),
        ("wildfly", "WildFly / JBoss"),
        ("logfmt", "Logfmt (key=value)"),
        ("csv", "CSV (com cabeçalho)"),
        ("w3c", "IIS / W3C"),
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
    name: String,
    kind: String,
    pattern: String,
    separator: String,
    fields: Vec<String>,
) -> Result<(), String> {
    offload(move || save_custom_format_impl(&name, &kind, &pattern, &separator, fields)).await?
}

pub(crate) fn save_custom_format_impl(
    name: &str,
    kind: &str,
    pattern: &str,
    separator: &str,
    fields: Vec<String>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Informe um nome para o formato.".into());
    }
    build_custom_parse(kind, pattern, separator, &fields)?; // valida
    let mut fmts = custom_formats();
    fmts.retain(|f| f.name != name);
    fmts.push(CustomFormat {
        name: name.to_string(),
        kind: kind.to_string(),
        pattern: pattern.to_string(),
        separator: separator.to_string(),
        fields,
    });
    let text = serde_json::to_string_pretty(&fmts).map_err(|e| e.to_string())?;
    std::fs::write(config_dir().join("formats.json"), text).map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_events(state: State<AppState>) {
    clear_events_impl(state.inner())
}

pub(crate) fn clear_events_impl(state: &AppState) {
    *state.source.write() = SourceData::None;
    state.source_names.write().clear();
}

/// Resumo da fonte carregada no momento (para MCP/UI saberem o que há no app).
#[derive(Serialize)]
pub(crate) struct SourceSummary {
    count: usize,
    columns: Vec<String>,
    source_desc: String,
    source_names: Vec<String>,
}

#[tauri::command]
async fn source_summary(app: AppHandle) -> Result<SourceSummary, String> {
    // Memory: a descoberta de colunas varre até 20k eventos — offload também.
    offload(move || {
        let state = app.state::<AppState>();
        source_summary_impl(state.inner())
    })
    .await
}

pub(crate) fn source_summary_impl(state: &AppState) -> SourceSummary {
    let source_names = state.source_names.read().clone();
    let source = state.source.read();
    let (count, columns, source_desc) = match &*source {
        SourceData::None => (0, vec![], String::new()),
        SourceData::Memory(evs) => (evs.len(), all_columns(evs), source_names.join(" + ")),
        SourceData::Indexed(idx) => (
            idx.lines.len(),
            idx.columns.clone(),
            format!("Arquivo: {}", idx.path),
        ),
    };
    SourceSummary {
        count,
        columns,
        source_desc,
        source_names,
    }
}

#[tauri::command]
async fn query_events(
    filters: Vec<query::Filter>,
    sort_column: String,
    sort_dir: String,
    offset: usize,
    limit: usize,
    app: AppHandle,
) -> Result<query::QueryResult, String> {
    offload(move || {
        let state = app.state::<AppState>();
        query_events_impl(state.inner(), filters, &sort_column, &sort_dir, offset, limit)
    })
    .await
}

pub(crate) fn query_events_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> query::QueryResult {
    let source = state.source.read();
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    let limit = limit.clamp(1, 2_000);
    match &*source {
        SourceData::Memory(events) => {
            query::query(events, &filters, sort_column, sort_dir, offset, limit)
        }
        SourceData::Indexed(idx) => query::query_indexed(
            idx, &filters, sort_column, sort_dir, offset, limit, &codes, &system, &derived,
        ),
        SourceData::None => query::QueryResult {
            total: 0,
            rows: vec![],
        },
    }
}

#[tauri::command]
async fn explore_snapshot(
    filters: Vec<query::Filter>,
    sort_column: String,
    sort_dir: String,
    offset: usize,
    limit: usize,
    app: AppHandle,
) -> Result<query::ExplorerSnapshot, String> {
    offload(move || {
        let state = app.state::<AppState>();
        explore_snapshot_impl(state.inner(), filters, &sort_column, &sort_dir, offset, limit, Some(&app))
    })
    .await
}

pub(crate) fn explore_snapshot_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    app: Option<&AppHandle>,
) -> query::ExplorerSnapshot {
    emit_progress(app, "exploração", "Aplicando filtros", 0, 0, "eventos", false);
    let source = state.source.read();
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    let limit = limit.clamp(1, 2_000);
    let snapshot = match &*source {
        SourceData::Memory(events) => query::explore(
            events, &filters, sort_column, sort_dir, offset, limit,
        ),
        SourceData::Indexed(idx) => query::explore_indexed(
            idx, &filters, sort_column, sort_dir, offset, limit, &codes, &system, &derived,
        ),
        SourceData::None => query::ExplorerSnapshot {
            query: query::QueryResult { total: 0, rows: vec![] },
            stats: query::Stats { buckets: vec![], bucket_ms: 0, levels: vec![] },
            sources: query::AggResult { columns: vec![], rows: vec![] },
            codes: query::AggResult { columns: vec![], rows: vec![] },
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
    snapshot
}

#[tauri::command]
async fn aggregate_events(
    group_column: String,
    aggs: Vec<query::AggSpec>,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<query::AggResult, String> {
    offload(move || {
        let state = app.state::<AppState>();
        aggregate_events_impl(state.inner(), &group_column, aggs, filters, case_events)
    })
    .await
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
        let indices = query::filtered_indices(&events, &filters);
        let filtered: Vec<Event> = indices.into_iter().map(|i| events[i].clone()).collect();
        return query::aggregate(&filtered, &[], group_column, &aggs);
    }
    let source = state.source.read();
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    match &*source {
        SourceData::Memory(events) => query::aggregate(events, &filters, group_column, &aggs),
        SourceData::Indexed(idx) => {
            query::aggregate_indexed(idx, &filters, group_column, &aggs, &codes, &system, &derived)
        }
        SourceData::None => query::AggResult {
            columns: vec![],
            rows: vec![],
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
fn trail_from_events(events: &[Event], filters: &[query::Filter], center_id: usize, before: usize, after: usize) -> TrailResult {
    let idxs = query::filtered_indices(events, filters);
    let mut rows: Vec<(i64, usize)> = idxs
        .iter()
        .map(|&i| (events[i].timestamp.unwrap_or(i64::MIN), i))
        .collect();
    rows.sort();
    let pos = rows
        .iter()
        .position(|&(_, i)| i == center_id)
        .unwrap_or_else(|| rows.len().saturating_sub(1));
    let start = pos.saturating_sub(before);
    let end = (pos + after + 1).min(rows.len());
    TrailResult {
        events: rows[start..end].iter().map(|&(_, i)| events[i].clone()).collect(),
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
    app: AppHandle,
) -> Result<TrailResult, String> {
    offload(move || {
        let state = app.state::<AppState>();
        trail_events_impl(state.inner(), center_id, before, after, filters, case_events)
    })
    .await
}

pub(crate) fn trail_events_impl(
    state: &AppState,
    center_id: usize,
    before: usize,
    after: usize,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> TrailResult {
    if let Some(events) = case_events {
        return trail_from_events(&events, &filters, center_id, before, after);
    }
    let source = state.source.read();
    match &*source {
        SourceData::Memory(events) => trail_from_events(events, &filters, center_id, before, after),
        SourceData::Indexed(idx) => {
            let matched = query::indexed_matches(
                idx,
                &filters,
                &state.codes.read(),
                &state.system_codes.read(),
                &state.derived.read(),
            );
            let mut rows: Vec<(i64, usize)> = matched.into_iter().map(|i| (idx.lines[i].ts, i)).collect();
            rows.sort();
            let pos = rows
                .iter()
                .position(|&(_, i)| i == center_id)
                .unwrap_or_else(|| rows.len().saturating_sub(1));
            let start = pos.saturating_sub(before);
            let end = (pos + after + 1).min(rows.len());
            let codes = state.codes.read();
            let system = state.system_codes.read();
            let derived = state.derived.read();
            TrailResult {
                events: rows[start..end]
                    .iter()
                    .map(|&(_, i)| crate::sources::event_at(idx, i, &codes, &system, &derived))
                    .collect(),
                before_available: start,
                after_available: rows.len() - end,
            }
        }
        SourceData::None => TrailResult { events: vec![], before_available: 0, after_available: 0 },
    }
}

/// Contagem simples de eventos que passam nos filtros (abas de filtros salvos).
#[tauri::command]
async fn count_filtered(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<usize, String> {
    offload(move || {
        let state = app.state::<AppState>();
        count_filtered_impl(state.inner(), filters, case_events)
    })
    .await
}

pub(crate) fn count_filtered_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> usize {
    if let Some(events) = case_events {
        return query::filtered_indices(&events, &filters).len();
    }
    let source = state.source.read();
    match &*source {
        SourceData::Memory(events) => query::filtered_indices(events, &filters).len(),
        SourceData::Indexed(idx) => query::indexed_matches(
            idx,
            &filters,
            &state.codes.read(),
            &state.system_codes.read(),
            &state.derived.read(),
        )
        .len(),
        SourceData::None => 0,
    }
}

#[tauri::command]
async fn tree_aggs(
    columns: Vec<String>,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    app: AppHandle,
) -> Result<Vec<(String, query::AggResult)>, String> {
    offload(move || {
        let state = app.state::<AppState>();
        tree_aggs_impl(state.inner(), columns, filters, case_events)
    })
    .await
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
    let source = state.source.read();
    match &*source {
        SourceData::Memory(events) => query::multi_count(events, &filters, &columns),
        SourceData::Indexed(idx) => query::multi_count_indexed(
            idx,
            &filters,
            &columns,
            &state.codes.read(),
            &state.system_codes.read(),
            &state.derived.read(),
        ),
        SourceData::None => vec![],
    }
}

#[tauri::command]
async fn stats_events(filters: Vec<query::Filter>, app: AppHandle) -> Result<query::Stats, String> {
    offload(move || {
        let state = app.state::<AppState>();
        stats_events_impl(state.inner(), filters)
    })
    .await
}

pub(crate) fn stats_events_impl(state: &AppState, filters: Vec<query::Filter>) -> query::Stats {
    let source = state.source.read();
    let codes = state.codes.read();
    let system = state.system_codes.read();
    let derived = state.derived.read();
    match &*source {
        SourceData::Memory(events) => query::stats(events, &filters),
        SourceData::Indexed(idx) => query::stats_indexed(idx, &filters, &codes, &system, &derived),
        SourceData::None => query::Stats {
            buckets: vec![],
            bucket_ms: 0,
            levels: vec![],
        },
    }
}

#[tauri::command]
async fn event_detail(id: usize, app: AppHandle) -> Result<Option<Event>, String> {
    offload(move || {
        let state = app.state::<AppState>();
        event_detail_impl(state.inner(), id)
    })
    .await
}

pub(crate) fn event_detail_impl(state: &AppState, id: usize) -> Option<Event> {
    let source = state.source.read();
    match &*source {
        SourceData::Memory(events) => events.get(id).cloned(),
        SourceData::Indexed(idx) => {
            if id < idx.lines.len() {
                Some(sources::event_at(
                    idx,
                    id,
                    &state.codes.read(),
                    &state.system_codes.read(),
                    &state.derived.read(),
                ))
            } else {
                None
            }
        }
        SourceData::None => None,
    }
}

// ------------------------------------------------------------------ análise

const ANALYSIS_CAP: usize = 50_000;

fn work_events(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Vec<Event> {
    if let Some(events) = case_events {
        // eventos do Caso também respeitam os filtros ativos (realidade filtrada)
        let indices = query::filtered_indices(&events, &filters);
        return indices
            .into_iter()
            .take(ANALYSIS_CAP)
            .map(|i| events[i].clone())
            .collect();
    }
    let source = state.source.read();
    match &*source {
        SourceData::Memory(events) => analysis::materialize_memory(events, &filters),
        SourceData::Indexed(idx) => analysis::materialize_indexed(
            idx,
            &filters,
            &state.codes.read(),
            &state.system_codes.read(),
            &state.derived.read(),
            ANALYSIS_CAP,
        ),
        SourceData::None => vec![],
    }
}

fn work_columns(evs: &[Event]) -> Vec<String> {
    let mut cols: Vec<String> = model::STANDARD_COLUMNS.iter().map(|s| s.to_string()).collect();
    let mut extra = std::collections::HashSet::new();
    for ev in evs.iter().take(500) {
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
    app: AppHandle,
) -> Result<Vec<analysis::FieldProfile>, String> {
    offload(move || {
        let state = app.state::<AppState>();
        profile_fields_impl(state.inner(), filters, case_events)
    })
    .await
}

pub(crate) fn profile_fields_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
) -> Vec<analysis::FieldProfile> {
    let evs = work_events(state, filters, case_events);
    let columns = work_columns(&evs);
    analysis::profile_fields(&evs, &columns)
}

#[tauri::command]
async fn compute_series(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    spec: analysis::SeriesSpec,
    app: AppHandle,
) -> Result<analysis::SeriesResult, String> {
    offload(move || {
        let state = app.state::<AppState>();
        compute_series_impl(state.inner(), filters, case_events, spec)
    })
    .await
}

pub(crate) fn compute_series_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    spec: analysis::SeriesSpec,
) -> analysis::SeriesResult {
    let evs = work_events(state, filters, case_events);
    analysis::compute_series(&evs, &spec)
}

#[tauri::command]
async fn pivot(
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    spec: analysis::PivotSpec,
    app: AppHandle,
) -> Result<analysis::PivotResult, String> {
    offload(move || {
        let state = app.state::<AppState>();
        pivot_impl(state.inner(), filters, case_events, spec)
    })
    .await
}

pub(crate) fn pivot_impl(
    state: &AppState,
    filters: Vec<query::Filter>,
    case_events: Option<Vec<Event>>,
    spec: analysis::PivotSpec,
) -> analysis::PivotResult {
    let evs = work_events(state, filters, case_events);
    analysis::pivot(&evs, &spec)
}

#[tauri::command]
async fn get_codes(app: AppHandle) -> Result<String, String> {
    offload(move || {
        let state = app.state::<AppState>();
        get_codes_impl(state.inner())
    })
    .await
}

pub(crate) fn get_codes_impl(state: &AppState) -> String {
    serde_json::to_string_pretty(&*state.codes.read()).unwrap_or_default()
}

#[tauri::command]
async fn save_codes(text: String, app: AppHandle) -> Result<(), String> {
    offload(move || {
        let state = app.state::<AppState>();
        save_codes_impl(state.inner(), &text)
    })
    .await?
}

pub(crate) fn save_codes_impl(state: &AppState, text: &str) -> Result<(), String> {
    let cfg: CodesConfig =
        serde_json::from_str(text).map_err(|e| format!("JSON inválido: {e}"))?;
    std::fs::write(&state.codes_path, text).map_err(|e| format!("Falha ao gravar: {e}"))?;
    *state.codes.write() = cfg;
    // Re-enriquece eventos em memória (fontes indexadas enriquecem sob demanda).
    let codes = state.codes.read().clone();
    let system = state.system_codes.read().clone();
    if let SourceData::Memory(events) = &mut *state.source.write() {
        for ev in events.iter_mut() {
            ev.name.clear();
            ev.description.clear();
            ev.enrich(&codes, &system);
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
pub(crate) struct HarvestSummary {
    count: usize,
    sources: usize,
}

/// Extrai (ou reextrai) o catálogo completo de eventos do sistema operacional.
#[tauri::command]
async fn harvest_codes(app: AppHandle) -> Result<HarvestSummary, String> {
    offload(move || {
        let state = app.state::<AppState>();
        harvest_codes_impl(state.inner())
    })
    .await?
}

pub(crate) fn harvest_codes_impl(state: &AppState) -> Result<HarvestSummary, String> {
    let (cfg, count) = sources::harvest_system_codes()?;
    let sources = cfg.sources.len();
    if let Ok(text) = serde_json::to_string(&cfg) {
        let _ = std::fs::write(&state.system_codes_path, text);
    }
    *state.system_codes.write() = cfg;
    // Re-enriquece eventos em memória (fontes indexadas enriquecem sob demanda).
    let codes = state.codes.read().clone();
    let system = state.system_codes.read().clone();
    if let SourceData::Memory(events) = &mut *state.source.write() {
        for ev in events.iter_mut() {
            ev.name.clear();
            ev.description.clear();
            ev.enrich(&codes, &system);
        }
    }
    Ok(HarvestSummary { count, sources })
}

#[tauri::command]
fn system_codes_count(state: State<AppState>) -> usize {
    system_codes_count_impl(state.inner())
}

pub(crate) fn system_codes_count_impl(state: &AppState) -> usize {
    state
        .system_codes
        .read()
        .sources
        .values()
        .map(|m| m.len())
        .sum()
}

// Os casos de análise são JSON opaco gerenciado pelo frontend; o backend
// apenas persiste em disco (sobrevive a reloads e fechamento do app).
fn cases_path() -> PathBuf {
    config_dir().join("cases.json")
}

fn cases_backup_path() -> PathBuf {
    config_dir().join("cases.backup.json")
}

fn read_cases_file(path: &PathBuf) -> Option<serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

#[tauri::command]
fn cases_load() -> serde_json::Value {
    cases_load_impl()
}

pub(crate) fn cases_load_impl() -> serde_json::Value {
    read_cases_file(&cases_path())
        .or_else(|| read_cases_file(&cases_backup_path()))
        .unwrap_or_else(|| serde_json::json!({ "active": null, "cases": [] }))
}

#[tauri::command]
fn cases_save(data: serde_json::Value, state: State<AppState>) -> Result<(), String> {
    cases_save_impl(state.inner(), data)
}

pub(crate) fn cases_save_impl(state: &AppState, data: serde_json::Value) -> Result<(), String> {
    let _store_guard = state.case_store_lock.lock();
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let path = cases_path();
    let backup = cases_backup_path();
    let temp = dir.join("cases.pending.json");
    let text = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;

    {
        let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }

    if read_cases_file(&path).is_some() {
        std::fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    }

    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temp, &path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_codes_path(state: State<AppState>) -> String {
    state.codes_path.display().to_string()
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
    let codes_path = config_dir().join("codes.json");
    let codes = load_codes(&codes_path);
    let system_codes_path = config_dir().join("system_codes.json");
    let system_codes = load_system_codes(&system_codes_path);
    let (mcp_enabled, mcp_port, mcp_config_path) = mcp::load_config();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            source: RwLock::new(SourceData::None),
            source_names: RwLock::new(vec![]),
            derived: RwLock::new(load_derived()),
            codes: RwLock::new(codes),
            system_codes: RwLock::new(system_codes),
            case_store_lock: Mutex::new(()),
            codes_path,
            system_codes_path,
        })
        .manage(mcp::McpState::new(mcp_enabled, mcp_port, mcp_config_path))
        .setup(move |app| {
            // Primeira execução: extrai o catálogo do sistema em background
            // (leva ~20s e não pode bloquear a abertura da janela).
            let state = app.state::<AppState>();
            let needs_harvest = state.system_codes.read().sources.is_empty();
            if needs_harvest {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    if let Ok((cfg, _)) = sources::harvest_system_codes() {
                        let state = handle.state::<AppState>();
                        if let Ok(text) = serde_json::to_string(&cfg) {
                            let _ = std::fs::write(&state.system_codes_path, text);
                        }
                        *state.system_codes.write() = cfg;
                    }
                });
            }
            // Servidor MCP embutido (loopback) para automação por agentes.
            if mcp_enabled {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    mcp::serve(handle, mcp_port).await;
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_channels,
            load_event_log,
            load_file,
            load_files,
            clear_events,
            source_summary,
            query_events,
            explore_snapshot,
            aggregate_events,
            trail_events,
            count_filtered,
            tree_aggs,
            stats_events,
            event_detail,
            get_codes,
            save_codes,
            get_codes_path,
            relaunch_elevated,
            harvest_codes,
            system_codes_count,
            cases_load,
            cases_save,
            list_formats,
            save_custom_format,
            get_ts_config,
            set_ts_config,
            test_ts_config,
            test_parse,
            list_derived_fields,
            save_derived_field,
            delete_derived_field,
            profile_fields,
            compute_series,
            pivot,
            mcp_status,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o LogInsight");
}
