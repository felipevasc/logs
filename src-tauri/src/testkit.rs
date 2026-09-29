//! Entry points for the integration tests in `tests/`, which exercise the same
//! loading and query paths as the app. Not part of the application's behavior.

pub use crate::model::Event;
use crate::model::CodesConfig;
use crate::query::{self, AggSpec, Filter};
use crate::sources::{event_at, CompiledDerived, FileIndex};
use serde_json::Value;

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
        let mut idx: Option<FileIndex> = None;
        for path in paths {
            let part = crate::index_source_file(path, "auto", None)?;
            match &mut idx {
                Some(all) => all.append(part),
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

    pub fn is_empty(&self) -> bool {
        self.idx.lines.is_empty()
    }

    /// Builds the columnar stores now (as a file load does).
    pub fn prepare(&self) -> Result<(), String> {
        crate::engine::set_enabled(true);
        crate::engine::prepare(&self.idx, &self.codes, &self.system, &self.derived, &|_, _| {})
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
                crate::engine::matches(&self.engine(), &query::prepare(&filters)),
            ),
            Engine::Lines => self.lines(|| {
                query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived)
            }),
        }
    }

    pub fn count(&self, engine: Engine, filters: &str) -> usize {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => Self::columnar("count", crate::engine::count(&self.engine(), &filters)),
            Engine::Lines => self.lines(|| {
                query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived).len()
            }),
        }
    }

    pub fn query(&self, engine: Engine, filters: &str, sort: &str, dir: &str, offset: usize, limit: usize) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "query",
                crate::engine::query(&self.engine(), &query::prepare(&filters), sort, dir, offset, limit),
            )),
            Engine::Lines => self.lines(|| {
                json(query::query_indexed(
                    &self.idx, &filters, sort, dir, offset, limit, &self.codes, &self.system, &self.derived,
                ))
            }),
        }
    }

    pub fn explore(&self, engine: Engine, filters: &str, sort: &str, dir: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "explore",
                crate::engine::explore(&self.engine(), &query::prepare(&filters), sort, dir, 0, 25),
            )),
            Engine::Lines => self.lines(|| {
                json(query::explore_indexed(
                    &self.idx, &filters, sort, dir, 0, 25, &self.codes, &self.system, &self.derived,
                ))
            }),
        }
    }

    pub fn stats(&self, engine: Engine, filters: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "stats",
                crate::engine::stats(&self.engine(), &query::prepare(&filters)),
            )),
            Engine::Lines => self.lines(|| {
                json(query::stats_indexed(&self.idx, &filters, &self.codes, &self.system, &self.derived))
            }),
        }
    }

    pub fn aggregate(&self, engine: Engine, filters: &str, group: &str, specs: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        let specs: Vec<AggSpec> = parse(specs);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "aggregate",
                crate::engine::aggregate(&self.engine(), &query::prepare(&filters), group, &specs),
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
                crate::engine::multi_count(&self.engine(), &filters, &columns),
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
                crate::engine::series(&self.engine(), &query::prepare(&filters), &spec),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived);
                json(crate::analysis::compute_series_stream(|| self.events(&ids), &spec))
            }),
        }
    }

    pub fn overview(&self, engine: Engine, filters: &str) -> Value {
        let filters: Vec<Filter> = parse(filters);
        match engine {
            Engine::Columnar => json(Self::columnar(
                "overview",
                crate::engine::overview(&self.engine(), &query::prepare(&filters)),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived);
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
                crate::engine::compare(&self.engine(), &query::prepare(&filters), &before, &after),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived);
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
                crate::engine::pivot(&self.engine(), &query::prepare(&filters), &spec),
            )),
            Engine::Lines => self.lines(|| {
                let ids = query::indexed_matches(&self.idx, &filters, &self.codes, &self.system, &self.derived);
                json(crate::analysis::pivot_stream(self.events(&ids), &spec))
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
