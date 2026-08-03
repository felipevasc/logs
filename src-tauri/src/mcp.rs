//! Servidor MCP (Model Context Protocol) embutido no app.
//!
//! Transporte: streamable HTTP em `http://127.0.0.1:{port}/mcp` (apenas
//! loopback). Cada tool reutiliza a função `*_impl` correspondente de
//! `lib.rs`, então o comportamento é idêntico ao dos comandos Tauri da UI.
//!
//! Convenções:
//! - Tools operam sempre sobre a fonte global carregada no app; o parâmetro
//!   `case_events` dos comandos Tauri não é exposto (não há conjuntos de Caso
//!   via MCP).
//! - Tools que MUTAM estado emitem o evento `mcp-state-changed` com payload
//!   `{"kind": "source" | "codes" | "derived" | "ts_config" | "formats" | "cases"}`
//!   para o frontend fazer live-refresh.
//! - Resultados são JSON serializado (pretty) em um content de texto.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::analysis::{PivotSpec, SeriesSpec};
use crate::query::{AggSpec, Filter};
use crate::sources::{DerivedRule, TsConfig};
use crate::AppState;

pub const DEFAULT_PORT: u16 = 39_117;

// ------------------------------------------------------------- configuração

/// Estado gerenciado do servidor MCP (para o comando `mcp_status`).
pub struct McpState {
    pub enabled: bool,
    pub port: u16,
    pub running: AtomicBool,
    pub config_path: PathBuf,
}

impl McpState {
    pub fn new(enabled: bool, port: u16, config_path: PathBuf) -> Self {
        McpState {
            enabled,
            port,
            running: AtomicBool::new(false),
            config_path,
        }
    }
}

#[derive(Deserialize)]
struct McpFileConfig {
    enabled: Option<bool>,
    port: Option<u16>,
}

/// Lê `mcp.json` do diretório de configuração. Arquivo ausente ou inválido
/// cai nos defaults (habilitado, porta 39117); quando ausente, o default é
/// gravado para o usuário descobrir onde configurar (falha é ignorável).
pub fn load_config() -> (bool, u16, PathBuf) {
    let path = crate::config_dir().join("mcp.json");
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, "{\n  \"enabled\": true,\n  \"port\": 39117\n}\n");
    }
    let parsed: Option<McpFileConfig> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let enabled = parsed.as_ref().and_then(|c| c.enabled).unwrap_or(true);
    let port = parsed
        .as_ref()
        .and_then(|c| c.port)
        .filter(|p| *p > 0)
        .unwrap_or(DEFAULT_PORT);
    (enabled, port, path)
}

#[derive(Serialize)]
pub struct ToolDesc {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Serialize)]
pub struct McpStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub url: String,
    pub config_path: String,
    pub tools: Vec<ToolDesc>,
}

pub fn status(mcp: &McpState) -> McpStatus {
    McpStatus {
        enabled: mcp.enabled,
        running: mcp.running.load(Ordering::Relaxed),
        port: mcp.port,
        url: format!("http://127.0.0.1:{}/mcp", mcp.port),
        config_path: mcp.config_path.display().to_string(),
        tools: tool_catalog()
            .into_iter()
            .map(|(name, description)| ToolDesc { name, description })
            .collect(),
    }
}

/// Catálogo das tools com descrições curtas em pt-BR (para a UI de settings).
pub fn tool_catalog() -> Vec<(&'static str, &'static str)> {
    vec![
        ("load_file", "Carrega um arquivo de log como fonte atual (muta estado)"),
        ("load_files", "Carrega vários arquivos unidos em uma única fonte (muta estado)"),
        ("load_event_log", "Carrega eventos de um canal do Event Log do Windows (muta estado)"),
        ("list_channels", "Lista os canais disponíveis do Event Log do Windows"),
        ("clear_events", "Descarta a fonte de eventos carregada (muta estado)"),
        ("source_summary", "Resumo da fonte carregada: contagem, colunas e descrição"),
        ("query_events", "Consulta eventos com filtros, ordenação e paginação"),
        ("event_detail", "Retorna um evento completo pelo id (inclui a linha bruta)"),
        ("explore_snapshot", "Recorte do explorador: linhas, histograma e facetas em uma chamada"),
        ("aggregate_events", "Agrega eventos por coluna (count, sum, avg, min, max, ...)"),
        ("trail_events", "Trilha temporal em torno de um evento (N antes, N depois)"),
        ("count_filtered", "Conta quantos eventos passam nos filtros"),
        ("tree_aggs", "Contagens por valor de várias colunas (árvore de exploração)"),
        ("stats_events", "Histograma temporal e distribuição por nível"),
        ("profile_fields", "Perfil estatístico dos campos do recorte filtrado"),
        ("compute_series", "Séries para gráficos: temporal ou ranking de termos"),
        ("pivot", "Tabela dinâmica (pivô OLAP) sobre o recorte filtrado"),
        ("list_formats", "Lista os formatos de log disponíveis (ids para load_file)"),
        ("save_custom_format", "Cria/atualiza um formato customizado (muta estado)"),
        ("test_parse", "Testa um formato customizado contra linhas de exemplo"),
        ("get_ts_config", "Retorna a config de data/hora salva para um arquivo"),
        ("set_ts_config", "Grava/remove config de data/hora e reaplica na fonte (muta estado)"),
        ("test_ts_config", "Testa uma config de data/hora nos primeiros eventos"),
        ("list_derived_fields", "Lista os campos derivados configurados"),
        ("save_derived_field", "Cria/atualiza um campo derivado por regex (muta estado)"),
        ("delete_derived_field", "Remove um campo derivado (muta estado)"),
        ("get_codes", "Retorna o catálogo de códigos do usuário"),
        ("get_codes_path", "Caminho do arquivo codes.json em disco"),
        ("save_codes", "Substitui o catálogo de códigos e re-enriquece eventos (muta estado)"),
        ("harvest_codes", "Reextrai o catálogo de eventos do sistema operacional (muta estado)"),
        ("system_codes_count", "Quantidade de códigos no catálogo extraído do sistema"),
        ("cases_load", "Carrega os casos de análise persistidos"),
        ("cases_save", "Persiste os casos de análise (muta estado)"),
    ]
}

// ------------------------------------------------------------------ startup

/// Sobe o servidor MCP em loopback. Retorna quando o servidor encerra
/// (erro de bind ou falha de I/O); roda para sempre no caso normal.
pub async fn serve(app: AppHandle, port: u16) {
    let listener = match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[mcp] não foi possível abrir 127.0.0.1:{port}: {e}");
            return;
        }
    };
    if let Some(mcp) = app.try_state::<McpState>() {
        mcp.running.store(true, Ordering::Relaxed);
    }
    eprintln!("[mcp] servidor ouvindo em http://127.0.0.1:{port}/mcp");

    let app_factory = app.clone();
    let service = StreamableHttpService::new(
        move || Ok(LogInsightMcp::new(app_factory.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("[mcp] erro no servidor HTTP: {e}");
    }
    if let Some(mcp) = app.try_state::<McpState>() {
        mcp.running.store(false, Ordering::Relaxed);
    }
}

// ------------------------------------------------------------------ helpers

fn ok_json<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value).map_err(|e| {
        McpError::internal_error(format!("falha ao serializar resultado: {e}"), None)
    })?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Erros de domínio (String) viram CallToolResult com is_error — visíveis ao
/// chamador — em vez de erro de protocolo JSON-RPC.
fn from_domain<T: Serialize>(result: Result<T, String>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(v) => ok_json(&v),
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
    }
}

fn join_err(e: tokio::task::JoinError) -> McpError {
    McpError::internal_error(format!("task da tool falhou: {e}"), None)
}

fn notify_state_changed(app: &AppHandle, kind: &str) {
    let _ = app.emit("mcp-state-changed", serde_json::json!({ "kind": kind }));
}

fn succeeded(result: &CallToolResult) -> bool {
    result.is_error != Some(true)
}

// ------------------------------------------------------------- parâmetros

const FORMAT_IDS_DOC: &str = "Format id: auto, jsonl, syslog3164, syslog5424, apache, firewall, cef, leef, log4j, logfmt, csv, w3c, text, wildfly or custom:<name>. Use list_formats to see the available ids.";
const FILTERS_DOC: &str = "Filters to apply (AND semantics). Each filter: {column, op, value, value2?}. Ops: contains, not_contains, equals, not_equals, starts_with, regex, gt, gte, lt, lte, between (uses value2 as upper bound), empty, not_empty. Special column \"_all\" matches the whole raw line. For the timestamp column, gt/gte/lt/lte/between accept epoch ms or ISO text.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LoadFileParams {
    /// Absolute path of the log file to load.
    pub path: String,
    #[schemars(description = "Format id: auto, jsonl, syslog3164, syslog5424, apache, firewall, cef, leef, log4j, logfmt, csv, w3c, text, wildfly or custom:<name>. Use list_formats to see the available ids.")]
    pub format: String,
    /// If true, merge with the currently loaded source instead of replacing it.
    #[serde(default)]
    pub merge: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LoadFilesParams {
    /// Absolute paths of the log files to load (joined into a single in-memory source).
    pub paths: Vec<String>,
    #[schemars(description = FORMAT_IDS_DOC)]
    pub format: String,
    /// If true, merge with the currently loaded source instead of replacing it.
    #[serde(default)]
    pub merge: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LoadEventLogParams {
    /// Windows Event Log channel name (e.g. "Application", "System"). Use list_channels to enumerate.
    pub channel: String,
    /// Maximum number of events to read (newest first), clamped to 1..=100000.
    #[serde(default = "default_max_events")]
    pub max_events: usize,
    /// If true, merge with the currently loaded source instead of replacing it.
    #[serde(default)]
    pub merge: Option<bool>,
}

fn default_max_events() -> usize {
    10_000
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Column to sort by (empty = natural order of the source).
    #[serde(default)]
    pub sort_column: String,
    /// Sort direction: "asc" or "desc".
    #[serde(default)]
    pub sort_dir: String,
    /// Rows to skip (pagination).
    #[serde(default)]
    pub offset: usize,
    /// Max rows to return (clamped to 1..=2000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EventDetailParams {
    /// Event id (line index of the current source, as returned by query_events).
    pub id: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AggregateParams {
    /// Column to group by (e.g. "source", "level", "code" or any dynamic field).
    pub group_column: String,
    /// Aggregations to compute per group. Each: {func, column, alias}. Funcs: count, count_distinct, sum, avg, min, max, string_agg. Use column "*" with func "count".
    pub aggs: Vec<AggSpec>,
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrailParams {
    /// Id of the center event.
    pub center_id: usize,
    /// How many events before the center to include.
    #[serde(default = "default_trail_span")]
    pub before: usize,
    /// How many events after the center to include.
    #[serde(default = "default_trail_span")]
    pub after: usize,
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
}

fn default_trail_span() -> usize {
    20
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FiltersParams {
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TreeAggsParams {
    /// Columns to aggregate (one AggResult per column, each computed with all filters except its own).
    pub columns: Vec<String>,
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ComputeSeriesParams {
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Series specification: {chart: "time" | "terms", metric: count | sum | avg | min | max | distinct, field?, interval_ms?, split?, limit?, unit?}.
    pub spec: SeriesSpec,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PivotParams {
    #[schemars(description = FILTERS_DOC)]
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Pivot specification: {rows: [column...], cols: [column...], values: [AggSpec...], limit_rows?}.
    pub spec: PivotSpec,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveCustomFormatParams {
    /// Format name (referenced later as format id "custom:<name>").
    pub name: String,
    /// Parser kind: "regex" (pattern with named groups) or "delimited" (separator + field names).
    pub kind: String,
    /// Regex with named capture groups, e.g. (?<message>.*) (only for kind "regex").
    #[serde(default)]
    pub pattern: String,
    /// Column separator, e.g. ";" or "\t" (only for kind "delimited").
    #[serde(default)]
    pub separator: String,
    /// Field names in column order (only for kind "delimited").
    #[serde(default)]
    pub fields: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TestParseParams {
    /// Parser kind: "regex" or "delimited".
    pub kind: String,
    /// Regex with named capture groups (kind "regex").
    #[serde(default)]
    pub pattern: String,
    /// Column separator (kind "delimited").
    #[serde(default)]
    pub separator: String,
    /// Field names in column order (kind "delimited").
    #[serde(default)]
    pub fields: Vec<String>,
    /// Sample log lines to parse (up to 5 non-empty lines are parsed).
    pub sample: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTsConfigParams {
    /// Absolute path of the log file the timestamp config applies to.
    pub path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetTsConfigParams {
    /// Absolute path of the log file the timestamp config applies to.
    pub path: String,
    /// Timestamp config to save and apply; null removes the saved config for this path.
    pub config: Option<TsConfig>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TestTsConfigParams {
    /// Timestamp config to test against the first 5 events of the current source. Returns (input, parsed result) pairs.
    pub config: TsConfig,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveDerivedFieldParams {
    /// Name of the derived field (created/updated; appears as a column).
    pub name: String,
    /// Source column the regex rules run against (e.g. "message").
    pub source: String,
    /// Rules tried in order (OR): {pattern: regex with at least one group, template?: "$1 - $2", filter?: Filter}. First non-empty extraction wins.
    pub rules: Vec<DerivedRule>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteDerivedFieldParams {
    /// Name of the derived field to delete.
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveCodesParams {
    /// Full codes catalog: {source ("*" = any): {code: {name, description}}}. Replaces the current catalog and re-enriches loaded events.
    pub codes: serde_json::Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CasesSaveParams {
    /// Cases document as managed by the app ({active, cases: [...]}); stored as opaque JSON.
    pub data: serde_json::Value,
}

// ------------------------------------------------------------------ servidor

#[derive(Clone)]
pub struct LogInsightMcp {
    app: AppHandle,
    // lido pelo código gerado de #[tool_handler] (roteamento das tools)
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl LogInsightMcp {
    pub fn new(app: AppHandle) -> Self {
        LogInsightMcp {
            app,
            tool_router: Self::tool_router(),
        }
    }

    /// Roda `f` (leitura do AppState) numa thread blocking e devolve JSON.
    async fn run<T, F>(&self, f: F) -> Result<CallToolResult, McpError>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&AppState) -> T + Send + 'static,
    {
        let app = self.app.clone();
        let value = tokio::task::spawn_blocking(move || {
            let state = app.state::<AppState>();
            f(state.inner())
        })
        .await
        .map_err(join_err)?;
        ok_json(&value)
    }

    /// Igual a `run`, mas para operações com erro de domínio (Result<_, String>).
    async fn run_domain<T, F>(&self, f: F) -> Result<CallToolResult, McpError>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&AppState) -> Result<T, String> + Send + 'static,
    {
        let app = self.app.clone();
        let result = tokio::task::spawn_blocking(move || {
            let state = app.state::<AppState>();
            f(state.inner())
        })
        .await
        .map_err(join_err)?;
        from_domain(result)
    }

    /// Igual a `run_domain`, mas passa também o AppHandle (progresso para a UI).
    async fn run_domain_app<T, F>(&self, f: F) -> Result<CallToolResult, McpError>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&AppState, &AppHandle) -> Result<T, String> + Send + 'static,
    {
        let app = self.app.clone();
        let result = tokio::task::spawn_blocking(move || {
            let state = app.state::<AppState>();
            f(state.inner(), &app)
        })
        .await
        .map_err(join_err)?;
        from_domain(result)
    }

    // ------------------------------------------------------------ carregamento

    #[tool(description = "Load a log file as the current source (replaces it unless merge=true). MUTATES app state: emits 'mcp-state-changed' {kind: 'source'}. Returns {count, columns, source_desc}. Java formats (log4j, wildfly) group multi-line stacktraces into a single event: the raw line holds the whole block and the event gains 'stacktrace' (array of 'at ...' frames) and 'exception' fields.")]
    async fn load_file(&self, Parameters(p): Parameters<LoadFileParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain_app(move |state, app| crate::load_file_impl(state, &p.path, &p.format, p.merge, Some(app)))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "source");
        }
        Ok(result)
    }

    #[tool(description = "Load several log files joined into a single in-memory source. MUTATES app state: emits 'mcp-state-changed' {kind: 'source'}. Returns {count, columns, source_desc}. Java formats (log4j, wildfly) group multi-line stacktraces into single events ('stacktrace'/'exception' fields).")]
    async fn load_files(&self, Parameters(p): Parameters<LoadFilesParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain_app(move |state, app| crate::load_files_impl(state, &p.paths, &p.format, p.merge, Some(app)))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "source");
        }
        Ok(result)
    }

    #[tool(description = "Load events from a Windows Event Log channel as the current source. MUTATES app state: emits 'mcp-state-changed' {kind: 'source'}. Returns {count, columns, source_desc}.")]
    async fn load_event_log(&self, Parameters(p): Parameters<LoadEventLogParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain_app(move |state, app| crate::load_event_log_impl(state, &p.channel, p.max_events, p.merge, Some(app)))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "source");
        }
        Ok(result)
    }

    #[tool(description = "List the available Windows Event Log channels.", annotations(read_only_hint = true))]
    async fn list_channels(&self) -> Result<CallToolResult, McpError> {
        self.run_domain(|_| crate::sources::list_channels()).await
    }

    #[tool(description = "Clear the currently loaded event source. MUTATES app state: emits 'mcp-state-changed' {kind: 'source'}.", annotations(read_only_hint = false, destructive_hint = true))]
    async fn clear_events(&self) -> Result<CallToolResult, McpError> {
        self.run(|state| crate::clear_events_impl(state)).await?;
        notify_state_changed(&self.app, "source");
        ok_json(&"source cleared")
    }

    #[tool(description = "Summary of the currently loaded source: event count, discovered columns, description and the list of source names (more than one when sources are merged). Empty values when nothing is loaded.", annotations(read_only_hint = true))]
    async fn source_summary(&self) -> Result<CallToolResult, McpError> {
        self.run(|state| crate::source_summary_impl(state)).await
    }

    // ------------------------------------------------------------ consulta

    #[tool(description = "Query events of the current source with filters, sorting and pagination. Returns {total, rows} (rows omit the raw line; use event_detail for the full event). Operates on the global loaded source.", annotations(read_only_hint = true))]
    async fn query_events(&self, Parameters(p): Parameters<QueryParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| {
            crate::query_events_impl(state, p.filters, &p.sort_column, &p.sort_dir, p.offset, p.limit)
        })
        .await
    }

    #[tool(description = "Get one full event by id (includes the raw line and all dynamic fields). Operates on the global loaded source.", annotations(read_only_hint = true))]
    async fn event_detail(&self, Parameters(p): Parameters<EventDetailParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::event_detail_impl(state, p.id)).await
    }

    #[tool(description = "Explorer snapshot in one call: paginated rows + time histogram + source/code facets for the filtered slice. Operates on the global loaded source.", annotations(read_only_hint = true))]
    async fn explore_snapshot(&self, Parameters(p): Parameters<QueryParams>) -> Result<CallToolResult, McpError> {
        let app = self.app.clone();
        let snapshot = tokio::task::spawn_blocking(move || {
            let state = app.state::<AppState>();
            crate::explore_snapshot_impl(state.inner(), p.filters, &p.sort_column, &p.sort_dir, p.offset, p.limit, Some(&app))
        })
        .await
        .map_err(join_err)?;
        ok_json(&snapshot)
    }

    #[tool(description = "Aggregate events by a column with aggregation functions. Operates on the global loaded source.", annotations(read_only_hint = true))]
    async fn aggregate_events(&self, Parameters(p): Parameters<AggregateParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::aggregate_events_impl(state, &p.group_column, p.aggs, p.filters, None))
            .await
    }

    #[tool(description = "Time trail around an event: N events before, the event, N after (by timestamp, within the filtered slice). Operates on the global loaded source.", annotations(read_only_hint = true))]
    async fn trail_events(&self, Parameters(p): Parameters<TrailParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::trail_events_impl(state, p.center_id, p.before, p.after, p.filters, None))
            .await
    }

    #[tool(description = "Count how many events of the current source pass the filters.", annotations(read_only_hint = true))]
    async fn count_filtered(&self, Parameters(p): Parameters<FiltersParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::count_filtered_impl(state, p.filters, None)).await
    }

    #[tool(description = "Value counts for several columns at once (exploration tree); each column is aggregated with all filters except its own.", annotations(read_only_hint = true))]
    async fn tree_aggs(&self, Parameters(p): Parameters<TreeAggsParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::tree_aggs_impl(state, p.columns, p.filters, None)).await
    }

    #[tool(description = "Time histogram (buckets) and level distribution for the filtered slice.", annotations(read_only_hint = true))]
    async fn stats_events(&self, Parameters(p): Parameters<FiltersParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::stats_events_impl(state, p.filters)).await
    }

    // ------------------------------------------------------------ análise

    #[tool(description = "Statistical profile of each field of the filtered slice (kind, cardinality, min/max, top values).", annotations(read_only_hint = true))]
    async fn profile_fields(&self, Parameters(p): Parameters<FiltersParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::profile_fields_impl(state, p.filters, None)).await
    }

    #[tool(description = "Compute chart series over the filtered slice: time-bucketed or top-terms, with metrics (count/sum/avg/min/max/distinct) and optional split.", annotations(read_only_hint = true))]
    async fn compute_series(&self, Parameters(p): Parameters<ComputeSeriesParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::compute_series_impl(state, p.filters, None, p.spec)).await
    }

    #[tool(description = "OLAP pivot table over the filtered slice: row dimensions, column dimensions and value aggregations.", annotations(read_only_hint = true))]
    async fn pivot(&self, Parameters(p): Parameters<PivotParams>) -> Result<CallToolResult, McpError> {
        self.run(move |state| crate::pivot_impl(state, p.filters, None, p.spec)).await
    }

    // ------------------------------------------------------------ formatos

    #[tool(description = "List available log formats (id + display name), including saved custom formats. Use the ids in load_file/load_files.", annotations(read_only_hint = true))]
    async fn list_formats(&self) -> Result<CallToolResult, McpError> {
        self.run(|_| crate::list_formats_impl()).await
    }

    #[tool(description = "Create or update a custom log format (regex with named groups or delimited). MUTATES app state: emits 'mcp-state-changed' {kind: 'formats'}.", annotations(read_only_hint = false, idempotent_hint = true))]
    async fn save_custom_format(&self, Parameters(p): Parameters<SaveCustomFormatParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain(move |_| crate::save_custom_format_impl(&p.name, &p.kind, &p.pattern, &p.separator, p.fields))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "formats");
        }
        Ok(result)
    }

    #[tool(description = "Test a custom format definition (regex or delimited) against sample log lines; returns the parsed events.", annotations(read_only_hint = true))]
    async fn test_parse(&self, Parameters(p): Parameters<TestParseParams>) -> Result<CallToolResult, McpError> {
        self.run_domain(move |_| crate::test_parse_impl(&p.kind, &p.pattern, &p.separator, &p.fields, &p.sample))
            .await
    }

    // ------------------------------------------------------------ data/hora

    #[tool(description = "Get the saved timestamp config for a log file path (null if none).", annotations(read_only_hint = true))]
    async fn get_ts_config(&self, Parameters(p): Parameters<GetTsConfigParams>) -> Result<CallToolResult, McpError> {
        self.run(move |_| crate::load_ts_config(&p.path)).await
    }

    #[tool(description = "Save (or remove, with config=null) the timestamp config of a file and re-apply it to the current source if it is that file. MUTATES app state: emits 'mcp-state-changed' {kind: 'ts_config'}.", annotations(read_only_hint = false, idempotent_hint = true))]
    async fn set_ts_config(&self, Parameters(p): Parameters<SetTsConfigParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain_app(move |state, app| crate::set_ts_config_impl(state, &p.path, p.config, Some(app)))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "ts_config");
        }
        Ok(result)
    }

    #[tool(description = "Test a timestamp config against the first 5 events of the current source; returns (input, result) pairs.", annotations(read_only_hint = true))]
    async fn test_ts_config(&self, Parameters(p): Parameters<TestTsConfigParams>) -> Result<CallToolResult, McpError> {
        self.run_domain(move |state| crate::test_ts_config_impl(state, p.config)).await
    }

    // ------------------------------------------------------------ derivados

    #[tool(description = "List the configured derived fields (regex-extracted columns).", annotations(read_only_hint = true))]
    async fn list_derived_fields(&self) -> Result<CallToolResult, McpError> {
        self.run(|state| crate::list_derived_fields_impl(state)).await
    }

    #[tool(description = "Create or update a derived field extracted by regex rules. MUTATES app state: emits 'mcp-state-changed' {kind: 'derived'}.", annotations(read_only_hint = false, idempotent_hint = true))]
    async fn save_derived_field(&self, Parameters(p): Parameters<SaveDerivedFieldParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain(move |state| crate::save_derived_field_impl(state, &p.name, &p.source, p.rules))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "derived");
        }
        Ok(result)
    }

    #[tool(description = "Delete a derived field. MUTATES app state: emits 'mcp-state-changed' {kind: 'derived'}.", annotations(read_only_hint = false, destructive_hint = true))]
    async fn delete_derived_field(&self, Parameters(p): Parameters<DeleteDerivedFieldParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain(move |state| crate::delete_derived_field_impl(state, &p.name))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "derived");
        }
        Ok(result)
    }

    // ------------------------------------------------------------ códigos

    #[tool(description = "Get the user codes catalog (source -> code -> name/description) as JSON.", annotations(read_only_hint = true))]
    async fn get_codes(&self) -> Result<CallToolResult, McpError> {
        self.run(|state| {
            serde_json::from_str::<serde_json::Value>(&crate::get_codes_impl(state))
                .unwrap_or(serde_json::Value::Null)
        })
        .await
    }

    #[tool(description = "Replace the user codes catalog and re-enrich loaded events. MUTATES app state: emits 'mcp-state-changed' {kind: 'codes'}.", annotations(read_only_hint = false, idempotent_hint = true))]
    async fn save_codes(&self, Parameters(p): Parameters<SaveCodesParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain(move |state| {
                let text = serde_json::to_string_pretty(&p.codes).map_err(|e| e.to_string())?;
                crate::save_codes_impl(state, &text)
            })
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "codes");
        }
        Ok(result)
    }

    #[tool(description = "Get the on-disk path of the user codes catalog file (codes.json).", annotations(read_only_hint = true))]
    async fn get_codes_path(&self) -> Result<CallToolResult, McpError> {
        self.run(|state| state.codes_path.display().to_string()).await
    }

    #[tool(description = "Re-extract the operating system's event code catalog (slow, ~20s) and re-enrich loaded events. MUTATES app state: emits 'mcp-state-changed' {kind: 'codes'}.", annotations(read_only_hint = false))]
    async fn harvest_codes(&self) -> Result<CallToolResult, McpError> {
        let result = self.run_domain(|state| crate::harvest_codes_impl(state)).await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "codes");
        }
        Ok(result)
    }

    #[tool(description = "Number of codes in the catalog extracted from the operating system.", annotations(read_only_hint = true))]
    async fn system_codes_count(&self) -> Result<CallToolResult, McpError> {
        self.run(|state| crate::system_codes_count_impl(state)).await
    }

    // ------------------------------------------------------------ casos

    #[tool(description = "Load the persisted analysis cases document ({active, cases: [...]}).", annotations(read_only_hint = true))]
    async fn cases_load(&self) -> Result<CallToolResult, McpError> {
        self.run(|_| crate::cases_load_impl()).await
    }

    #[tool(description = "Persist the analysis cases document (opaque JSON managed by the app). MUTATES app state: emits 'mcp-state-changed' {kind: 'cases'}.", annotations(read_only_hint = false, idempotent_hint = true))]
    async fn cases_save(&self, Parameters(p): Parameters<CasesSaveParams>) -> Result<CallToolResult, McpError> {
        let result = self
            .run_domain(move |state| crate::cases_save_impl(state, p.data))
            .await?;
        if succeeded(&result) {
            notify_state_changed(&self.app, "cases");
        }
        Ok(result)
    }
}

#[tool_handler]
impl ServerHandler for LogInsightMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("loginsight", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "LogInsight MCP server: load, query and analyze the log data of the LogInsight desktop app.\n\
                 \n\
                 STATE: all state lives in the app process and is shared with the user's UI. Tools always \
                 operate on the globally loaded source (case-specific event sets are not supported via MCP).\n\
                 \n\
                 TYPICAL FLOW: (1) list_formats to pick a format id; (2) load_file(path, format) to load a \
                 source; (3) source_summary to confirm what is loaded (count, columns); (4) query_events to \
                 browse rows and event_detail to see a full event including the raw line; (5) aggregate_events, \
                 compute_series, pivot, profile_fields and stats_events for analysis; (6) trail_events for the \
                 time context around a given event.\n\
                 \n\
                 FILTERS: tools accept a filters array of {column, op, value, value2?} with AND semantics. \
                 Ops: contains, not_contains, equals, not_equals, starts_with, regex, gt, gte, lt, lte, \
                 between (uses value2 as upper bound), empty, not_empty. The special column \"_all\" matches \
                 the whole raw line. For the timestamp column, gt/gte/lt/lte/between accept epoch \
                 milliseconds or ISO date-time text.\n\
                 \n\
                 FORMAT IDS: auto, jsonl, syslog3164, syslog5424, apache, firewall, cef, leef, log4j, \
                 wildfly, logfmt, csv, w3c, text, or custom:<name> for user-defined formats.\n\
                 \n\
                 JAVA STACKTRACES: the log4j and wildfly formats group multi-line stacktraces into a single \
                 event (a line matching the format header starts an event; following lines that do not match \
                 belong to it). Such events expose fields.stacktrace (JSON array of \"at ...\" frames) and \
                 fields.exception, and their raw line contains the whole block — so use the special \"_all\" \
                 column (or the stacktrace field) to search text that only appears inside a stacktrace.\n\
                 \n\
                 MUTATIONS: tools marked as mutating (load_*, clear_events, save_*, set_ts_config, \
                 delete_derived_field, harvest_codes, cases_save) change the app state and live-refresh the \
                 user's UI via the 'mcp-state-changed' event. Always tell the user before changing the loaded \
                 data source or any saved configuration on their behalf."
                    .to_string(),
            )
    }
}
