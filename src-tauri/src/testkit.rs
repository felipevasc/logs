//! Entry points for the integration tests in `tests/`, which exercise the same
//! loading and query paths as the app. Not part of the application's behavior.

pub use crate::model::Event;
use crate::model::CodesConfig;
use crate::query::{self, AggSpec, Filter};
use crate::sources::{event_at, CompiledDerived, FileIndex};
use serde_json::Value;

/// Initialize the same coordinated parser pool as the desktop before any
/// benchmark/fixture work can initialize Rayon's unrestricted default pool.
pub fn init_resources() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(crate::resources::init);
}

/// Loads a file as the app does (packages, spreadsheets, encodings, detected
/// format) and returns the detected format and every event.
pub fn load(path: &str) -> Result<(String, Vec<Event>), String> {
    let idx = crate::index_source_file(path, "auto", None)?;
    let empty = crate::model::CodesConfig::default();
    let events = (0..idx.lines.len())
        .map(|i| crate::sources::event_at(&idx, i, &empty, &empty, &[]))
        .collect();
    Ok((idx.parts[0].format.clone(), events))
}

pub fn detect_format(sample: &[u8]) -> &'static str {
    crate::sources::detect_format(sample)
}

pub fn encoding_of(sample: &[u8]) -> Option<&'static str> {
    crate::workspace::sniff_encoding(sample).map(|encoding| encoding.name())
}

/// Files loaded together (one part each), with catalogs and derived fields.
pub struct Source {
    idx: FileIndex,
    codes: CodesConfig,
    system: CodesConfig,
    derived: Vec<CompiledDerived>,
}

/// Which engine answers: the columnar engine (must answer) or the line engine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Columnar,
    Lines,
}

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("JSON de teste inválido ({e}): {json}"))
}

fn json(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("serializable result")
}

impl Source {
    /// `codes` is a catalog in the app's JSON format; `derived` a list of
    /// derived fields as saved by the app.
    pub fn open(paths: &[&str], codes: &str, derived: &str) -> Result<Source, String> {
        init_resources();
        let mut idx: Option<FileIndex> = None;
        for path in paths {
            let part = crate::index_source_file(path, "auto", None)?;
            match &mut idx {
                Some(all) => all.append(part)?,
                None => idx = Some(part),
            }
        }
        let defs: Vec<crate::sources::DerivedFieldCompat> = parse(derived);
        let derived = defs
            .into_iter()
            .map(|d| d.normalize())
            .map(|d| CompiledDerived {
                name: d.name,
                source: d.source,
                steps: d.steps,
                lookup: d.lookup.map(crate::reference_lookup::Compiled::new),
                rules: d
                    .rules
                    .iter()
                    .map(|r| crate::sources::CompiledRule {
                        re: regex::Regex::new(&r.pattern).expect("regex de teste"),
                        template: r.template.clone(),
                        filter: r.filter.clone(),
                    })
                    .collect(),
            })
            .collect();
        Ok(Source {
            idx: idx.ok_or("Nenhum arquivo.")?,
            codes: parse(codes),
            system: CodesConfig::default(),
            derived,
        })
    }

    pub fn len(&self) -> usize {
        self.idx.lines.len()
    }

    /// Storage accounting for the ignored library workload, not OS RSS/heap.
    #[cfg(test)]
    pub fn metadata_storage(&self) -> Value {
        let rows = self.idx.lines.len();
        let resident = self.idx.lines.resident_rows();
        serde_json::json!({"rows": rows, "residentRows": resident, "mappedRows": rows - resident,
            "residentPayloadBytes": resident as u64 * std::mem::size_of::<crate::model::LineMeta>() as u64})
    }

    pub fn is_empty(&self) -> bool {
        self.idx.lines.is_empty()
    }
    pub fn set_system_catalog(&mut self, catalog: &str) {
        self.system = parse(catalog);
        crate::engine::catalogs_changed();
    }

    /// Builds the columnar stores now (as a file load does).
    pub fn prepare(&self) -> Result<(), String> {
        crate::engine::set_enabled(true);
        crate::engine::prepare(&self.idx, &self.codes, &self.system, &self.derived, &|_, _| {})
    }

    /// Deterministic pending-derived fixture: only immutable base stores exist.
    pub fn prepare_base(&self) -> Result<(), String> {
        crate::engine::set_enabled(true);
        crate::engine::prepare(&self.idx, &self.codes, &self.system, &[], &|_, _| {})
    }

    fn engine(&self) -> crate::engine::Source<'_> {
        crate::engine::Source {
            idx: &self.idx,
            codes: &self.codes,
            system: &self.system,
            derived: &self.derived,
        }
    }

    fn events(&self, ids: &[usize]) -> impl Iterator<Item = Event> + '_ {
        let ids = ids.to_vec();
        ids.into_iter()
            .map(|i| event_at(&self.idx, i, &self.codes, &self.system, &self.derived))
    }

    fn lines<T>(&self, f: impl FnOnce() -> T) -> T {
        crate::engine::set_enabled(false);
        let out = f();
        crate::engine::set_enabled(true);
        out
    }

    fn columnar<T>(what: &str, value: Option<T>) -> T {
        value.unwrap_or_else(|| panic!("o motor colunar não respondeu: {what}"))
    }

    pub fn matches(&self, engine: Engine, filters: &str) -> Vec<usize> {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => Self::columnar(
                "matches",
                crate::engine::matches(&self.engine(), &query::prepare(&filters)).expect("matching query failed"),
            ),
            Engine::Lines => self.lines(|| {
                query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap()
            }),
        }
    }

    pub fn count(&self, engine: Engine, filters: &str) -> usize {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => Self::columnar("count", crate::engine::count(&self.engine(), &filters).expect("analytics query failed")),
            Engine::Lines => self.lines(|| {
                query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap().len()
            }),
        }
    }

    pub fn try_count(&self, filters: &str) -> Result<usize, String> {
        let filters: Vec<Filter> = parse(filters);
        crate::engine::count(&self.engine(), &filters)?.ok_or("Motor indisponível.".into())
    }

    pub fn selection_cache(&self) -> Value {
        crate::engine::session(&self.idx, &self.codes, &self.system, &self.derived)
            .map_or(Value::Null, |session| session.selection_snapshot())
    }

    pub fn query(&self, engine: Engine, filters: &str, sort: &str, dir: &str, offset: usize, limit: usize) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "query",
                crate::engine::query(&self.engine(), &query::prepare(&filters), sort, dir, offset, limit).unwrap(),
            )),
            Engine::Lines => self.lines(|| {
                json(query::query_indexed(
                    &self.idx, &filters, sort, dir, offset, limit, &self.codes, &self.system, &self.derived,
                ).unwrap())
            }),
        }
    }

    /// Interactive page benchmark/parity entry point. Requires an indexed
    /// answer, so benchmark runs never silently measure a line fallback.
    pub fn page(&self, filters: &str, sort: &str, dir: &str, offset: usize, limit: usize, cursor: Option<&str>) -> Value {
        self.try_page(filters, sort, dir, offset, limit, cursor).expect("interactive page")
    }

    pub fn try_page(&self, filters: &str, sort: &str, dir: &str, offset: usize, limit: usize, cursor: Option<&str>) -> Result<Value, String> {
        let filters: Vec<Filter> = parse(filters);
        Self::columnar("page", crate::engine::query_page(
            &self.engine(), &query::prepare(&filters), sort, dir, offset, limit, cursor,
        )).map(json)
    }

    pub fn recovery_page(&self, filters: &str, sort: &str, dir: &str, offset: usize, limit: usize) -> Result<Value, String> {
        let filters: Vec<Filter> = parse(filters);
        query::query_page_lines(&self.idx, &filters, sort, dir, offset, limit, &self.codes, &self.system, &self.derived).map(json)
    }

    pub fn engine_status(&self) -> Value {
        json(crate::engine::status(&self.idx, &self.codes, &self.system, &self.derived))
    }

    pub fn explain_page(&self, filters: &str, sort: &str, dir: &str, limit: usize, analyze: bool) -> Result<Value, String> {
        let filters: Vec<Filter> = parse(filters);
        Self::columnar("explain page", crate::engine::explain_page(&self.engine(), &query::prepare(&filters), sort, dir, limit, analyze))
    }

    pub fn explain_page_at(&self, filters: &str, sort: &str, dir: &str, offset: usize, limit: usize, cursor: Option<&str>, analyze: bool) -> Result<Value, String> {
        let filters: Vec<Filter> = parse(filters);
        Self::columnar("explain page", crate::engine::explain_page_at(&self.engine(), &query::prepare(&filters), sort, dir, offset, limit, cursor, analyze))
    }

    pub fn explore(&self, engine: Engine, filters: &str, sort: &str, dir: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "explore",
                crate::engine::explore(&self.engine(), &query::prepare(&filters), sort, dir, 0, 25).unwrap(),
            )),
            Engine::Lines => self.lines(|| {
                json(query::explore_indexed(
                    &self.idx, &filters, sort, dir, 0, 25, &self.codes, &self.system, &self.derived,
                ).unwrap())
            }),
        }
    }

    pub fn stats(&self, engine: Engine, filters: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "stats",
                crate::engine::stats(&self.engine(), &query::prepare(&filters)).expect("analytics query failed"),
            )),
            Engine::Lines => self.lines(|| {
                json(query::stats_indexed(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap())
            }),
        }
    }

    pub fn aggregate(&self, engine: Engine, filters: &str, group: &str, specs: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        let specs: Vec<AggSpec> = parse(specs);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "aggregate",
                crate::engine::aggregate(&self.engine(), &query::prepare(&filters), group, &specs).unwrap_or_else(|error| Some(query::AggResult::failure(error))),
            )),
            Engine::Lines => self.lines(|| {
                json(query::aggregate_indexed(
                    &self.idx, &filters, group, &specs, &self.codes, &self.system, &self.derived,
                ))
            }),
        }
    }

    pub fn multi_count(&self, engine: Engine, filters: &str, columns: &[&str]) -> Value {
        let filters: Vec<Filter> = parse(filters);
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        match engine {
            Engine::Columnar => json(Self::columnar(
                "multi_count",
                crate::engine::multi_count(&self.engine(), &filters, &columns).expect("analytics query failed"),
            )),
            Engine::Lines => self.lines(|| {
                json(query::multi_count_indexed(
                    &self.idx, &filters, &columns, &self.codes, &self.system, &self.derived,
                ))
            }),
        }
    }

    pub fn series(&self, engine: Engine, filters: &str, spec: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        let spec: crate::analysis::SeriesSpec = parse(spec);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "series",
                crate::engine::series(&self.engine(), &query::prepare(&filters), &spec).expect("analytics query failed"),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap();
                json(crate::analysis::compute_series_stream(|| self.events(&ids), &spec).unwrap())
            }),
        }
    }

    pub fn overview(&self, engine: Engine, filters: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "overview",
                crate::engine::overview(&self.engine(), &query::prepare(&filters)).expect("analytics query failed"),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap();
                json(crate::insights::overview(|| self.events(&ids)))
            }),
        }
    }

    pub fn compare(&self, engine: Engine, filters: &str, before: (i64, i64), after: (i64, i64)) -> Value {
        let filters: Vec<Filter> = parse(filters);
        let before = crate::insights::Period { start: before.0, end: before.1 };
        let after = crate::insights::Period { start: after.0, end: after.1 };
        match engine {
            Engine::Columnar => json(Self::columnar(
                "compare",
                crate::engine::compare(&self.engine(), &query::prepare(&filters), &before, &after).expect("analytics query failed"),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap();
                json(crate::insights::compare(self.events(&ids), &before, &after))
            }),
        }
    }

    pub fn pivot(&self, engine: Engine, filters: &str, spec: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        let spec: crate::analysis::PivotSpec = parse(spec);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "pivot",
                crate::engine::pivot(&self.engine(), &query::prepare(&filters), &spec).expect("analytics query failed"),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).unwrap();
                json(crate::analysis::pivot_stream(self.events(&ids), &spec).unwrap())
            }),
        }
    }

    /// Events of the given lines as the app materializes them.
    pub fn events_at(&self, ids: &[usize]) -> Vec<Event> {
        self.events(ids).collect()
    }
}

/// Where the columnar stores are kept (tests point it to a temporary folder).
pub fn set_engine_dir(path: &str) {
    std::env::set_var("LOGINSIGHT_ENGINE_DIR", path);
}

/// Drops cached line-engine selections so each engine computes its own.
pub fn clear_caches() {
    query::clear_match_cache();
}

pub fn cancel_named(id: &str) { crate::operations::cancel_id(id); }
pub fn with_operation<T>(id: &str, progress: impl Fn(Value) + Send + Sync + 'static, work: impl FnOnce() -> T) -> Result<T, String> {
    let token = crate::operations::token(Some(id.into()))?;
    crate::operations::run_with_token(token, || crate::operations::with_reporter(std::sync::Arc::new(move |event| progress(json(event))), work))
}

/// Time spent per step of reading `count` lines of a file (manual profiling).
#[doc(hidden)]
pub fn profile_reading(path: &str, count: usize) -> Vec<(&'static str, std::time::Duration)> {
    use std::time::{Duration, Instant};
    let idx = crate::index_source_file(path, "auto", None).expect("índice");
    let empty = CodesConfig::default();
    let n = count.min(idx.lines.len());
    let part = &idx.parts[0];
    let mut out: Vec<(&'static str, Duration)> = Vec::new();
    let t = Instant::now();
    for i in 0..n {
        std::hint::black_box(crate::sources::parse_line(
            crate::sources::line_bytes(&idx, i),
            &part.format,
            part.custom.as_ref(),
            &part.header,
        ));
    }
    out.push(("parse_line", t.elapsed()));
    let t = Instant::now();
    let events: Vec<Event> = (0..n).map(|i| event_at(&idx, i, &empty, &empty, &[])).collect();
    out.push(("event_at", t.elapsed()));
    let t = Instant::now();
    for ev in &events {
        std::hint::black_box(crate::entities::extract(ev).get(crate::entities::Role::User).map(str::len));
    }
    out.push(("entities::extract", t.elapsed()));
    let t = Instant::now();
    for ev in &events {
        std::hint::black_box(crate::entities::tool(ev).map(|t| t.name));
    }
    out.push(("entities::tool", t.elapsed()));
    let t = Instant::now();
    for ev in &events {
        std::hint::black_box(crate::insights::pattern_of(&ev.message));
    }
    out.push(("pattern_of", t.elapsed()));
    let t = Instant::now();
    for ev in &events {
        let mut text = ev.message.to_lowercase();
        for value in ev.fields.values() {
            if let Value::String(s) = value {
                text.push_str(&s.to_lowercase());
            }
        }
        std::hint::black_box(text);
    }
    out.push(("valores em minúsculas", t.elapsed()));
    let t = Instant::now();
    for ev in &events {
        std::hint::black_box(crate::entities::value(ev, crate::entities::Role::Tool));
    }
    out.push(("entities::value(Tool)", t.elapsed()));
    out
}

/// Exercises the real metadata cache/parser with deliberately tiny waves. The
/// options and callback are test harness controls, never production env flags.
pub fn metadata_probe(
    path: &str,
    format: &str,
    cache_dir: &str,
    options: &str,
    progress: &dyn Fn(Value),
) -> Result<Value, String> {
    use crate::metadata_checkpoint::{Progress, Resume};
    use std::cell::Cell;
    init_resources();
    let options: Value = serde_json::from_str(options).map_err(|e| e.to_string())?;
    let custom = if let Some(pattern) = options["regex"].as_str() {
        Some(crate::sources::CustomParse::Regex(regex::Regex::new(pattern).map_err(|e| e.to_string())?))
    } else if let Some(delimiter) = options["delimiter"].as_str() {
        Some(crate::sources::CustomParse::Delimited {
            sep: delimiter.chars().next().ok_or("empty delimiter")?,
            fields: options["fields"].as_array().ok_or("missing fields")?.iter().map(|v| v.as_str().unwrap_or_default().to_string()).collect(),
        })
    } else { None };
    let ts = if options["timestamp"].is_object() {
        Some(serde_json::from_value::<crate::sources::TsConfig>(options["timestamp"].clone()).map_err(|e| e.to_string())?.compile()?)
    } else { None };
    let mut prepared = crate::sources::prepare_index(path, format, custom, ts)?;
    if options["smallWaves"].as_bool().unwrap_or(true) {
        prepared.limits = crate::sources::IndexLimits { chunk_bytes: 512, chunk_lines: 7, wave_chunks: 2, json_records: 7, json_bytes: 512 };
    }
    if let Some(year) = options["year"].as_i64() { prepared.part.calendar.year = i32::try_from(year).map_err(|e| e.to_string())?; }
    let key = prepared.key()?;
    let restored_rows = Cell::new(0usize);
    let committed_rows = Cell::new(0usize);
    let on_progress = |p: &Progress| {
        restored_rows.set(restored_rows.get().max(p.resumed_rows));
        committed_rows.set(committed_rows.get().max(p.checkpoint_rows));
        progress(serde_json::json!({
            "phaseId": p.phase_id, "phase": p.phase,
            "completed": p.completed, "total": p.total, "unit": p.unit,
            "resumedRows": p.resumed_rows, "checkpointRows": p.checkpoint_rows,
            "parsedRows": p.parsed_rows,
        }));
    };
    let run = || -> Result<Value, String> {
        let idx = if options["uncached"].as_bool().unwrap_or(false) {
            let seed = Resume { cursor: prepared.initial_cursor(), ..Default::default() };
            crate::sources::index_prepared(&prepared, seed, &mut |_, _, _, _| Ok(()), Some(&on_progress))?
        } else {
            crate::index_cache::open_prepared_at(&prepared, std::path::Path::new(cache_dir), Some(&on_progress))?
        };
        let rows: Vec<Value> = idx.lines.iter().map(|m| serde_json::json!([m.offset, m.len, m.ts, m.level, m.code_off, m.code_len])).collect();
        let empty = crate::model::CodesConfig::default();
        let events: Vec<Event> = (0..idx.lines.len()).map(|i| crate::sources::event_at(&idx, i, &empty, &empty, &[])).collect();
        Ok(serde_json::json!({
            "key": key, "format": idx.format, "header": idx.header, "columns": idx.columns,
            "timezone": idx.parts[0].calendar.timezone, "currentOffset": chrono::Local::now().offset().to_string(),
            "metadata": rows, "events": events, "rows": idx.lines.len(),
            "metadataResidentRows": idx.lines.resident_rows(),
            "resumedRows": restored_rows.get(), "checkpointRows": committed_rows.get(),
            "parsedRows": prepared.parsed_rows.load(std::sync::atomic::Ordering::Relaxed),
        }))
    };
    if let Some(operation_id) = options["operationId"].as_str() {
        let token = crate::operations::token(Some(operation_id.into()))?;
        crate::operations::run_with_token(token, run)?
    } else { run() }
}

pub fn cancel_metadata_probe(operation_id: &str) -> bool {
    crate::operations::cancel_id(operation_id)
}

pub fn prune_metadata_probe(cache_dir: &str, age_seconds: u64) {
    crate::metadata_checkpoint::prune(std::path::Path::new(cache_dir), std::time::SystemTime::now() - std::time::Duration::from_secs(age_seconds));
}
