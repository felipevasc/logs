use crate::model::CodesConfig;
use crate::model::{label_class, Event, LineMeta, LV_OTHER};
use crate::sources::{event_at, line_bytes, CompiledDerived, FileIndex};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use parking_lot::Mutex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Filter {
    pub column: String,
    pub op: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub value2: Option<String>,
}

/// Filtro com regex pré-compilada; entradas inválidas são rejeitadas na fronteira.
/// `needle_lower`, `num` e `num2` são pré-computados uma vez por consulta
/// em vez de por evento/linha.
pub struct PreparedFilter {
    pub f: Filter,
    regex: Option<regex::Regex>,
    needle_lower: String,
    num: Option<f64>,
    num2: Option<f64>,
    threat: Option<crate::threats::RuleMatcher>,
    /// Compiled search expression (`op = "query"`).
    expr: Option<crate::querylang::Expr>,
    /// Lowercased values for `in` / `not_in`.
    set: Option<std::collections::HashSet<String>>,
    /// Networks for `cidr` / `not_cidr`.
    nets: Vec<crate::querylang::IpNet>,
    /// Detection rule reproduced as evidence filter (`op = "detection"`).
    detection: Option<(std::sync::Arc<crate::detections::RuleSet>, usize)>,
}

/// Values of an `in` filter: one per line (commas also separate).
pub fn list_values(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(['\n', '\r', ','])
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

pub fn prepare(filters: &[Filter]) -> Vec<PreparedFilter> {
    prepare_with_threat_catalog(filters, None)
}

pub(crate) fn prepare_with_threat_catalog(
    filters: &[Filter],
    catalog: Option<&std::sync::Arc<crate::threats::CompiledCatalog>>,
) -> Vec<PreparedFilter> {
    filters
        .iter()
        .map(|f| {
            let is_re = f.op == "regex";
            let is_query = f.op == "query";
            PreparedFilter {
                regex: is_re.then(|| regex::Regex::new(&f.value).ok()).flatten(),
                needle_lower: if is_query { String::new() } else { f.value.to_lowercase() },
                num: value_as_num(&f.column, &f.value),
                num2: value_as_num(&f.column, f.value2.as_deref().unwrap_or("")),
                threat: if f.op == "threat_rule" {
                    crate::threats::matcher(&f.value, catalog.cloned()).ok()
                } else {
                    None
                },
                expr: is_query
                    .then(|| {
                        crate::querylang::compile_with(
                            &f.value,
                            &crate::querylang::Options { threats: catalog, detections: true },
                        )
                        .ok()
                    })
                    .flatten(),
                set: if f.op == "in_exact" { Some(f.value.lines().filter(|v| !v.is_empty()).map(str::to_string).collect()) }
                    else { matches!(f.op.as_str(), "in" | "not_in").then(|| list_values(&f.value).map(str::to_lowercase).collect()) },
                nets: if matches!(f.op.as_str(), "cidr" | "not_cidr") {
                    list_values(&f.value)
                        .flat_map(|v| v.split_whitespace())
                        .filter_map(crate::querylang::IpNet::parse)
                        .collect()
                } else {
                    Vec::new()
                },
                detection: (f.op == "detection")
                    .then(|| {
                        let set = crate::detections::ruleset().ok()?;
                        let index = set.rules.iter().position(|r| r.def.id == f.value)?;
                        Some((set, index))
                    })
                    .flatten(),
                f: f.clone(),
            }
        })
        .collect()
}

#[derive(Serialize)]
pub struct QueryResult {
    pub total: usize,
    pub rows: Vec<Event>,
}

#[derive(Serialize, Default)]
pub struct AggResult {
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
    /// Exact grouping values, parallel to rows; None is an empty/missing value.
    pub group_values: Vec<Option<String>>,
    pub incompatible_units: Vec<usize>,
    pub value_units: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub buckets: Vec<(i64, i64)>,
    pub bucket_ms: i64,
    pub levels: Vec<(String, i64)>,
}

/// Recorte completo do Explorador. A seleção de linhas é calculada uma vez e
/// reutilizada para tabela, histograma e facetas no mesmo ciclo de leitura.
#[derive(Serialize)]
pub struct ExplorerSnapshot {
    pub query: QueryResult,
    pub stats: Stats,
    pub sources: AggResult,
    pub codes: AggResult,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct AggSpec {
    pub func: String,
    pub column: String,
    pub alias: String,
}

/// Interpreta o valor digitado no filtro como número. Para a coluna
/// `timestamp`, aceita epoch em ms ou texto ISO ("2024-01-01 10:30").
fn value_as_num(column: &str, s: &str) -> Option<f64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<f64>() {
        return Some(n);
    }
    if column == "timestamp" {
        return crate::sources::parse_timestamp(s).map(|ms| ms as f64);
    }
    // aceita valores com unidade ("100 MB", "2s") nos filtros >, < e entre
    crate::analysis::parse_num_unit(s).map(|(n, _)| n)
}

pub fn matches(ev: &Event, pf: &PreparedFilter) -> bool {
    let f = &pf.f;
    if f.op == "threat_rule" {
        return f.column == "_all"
            && pf
                .threat
                .as_ref()
                .is_some_and(|matcher| matcher.matches(ev));
    }
    if f.op == "query" {
        // An invalid expression was rejected by validation; never match silently.
        return pf.expr.as_ref().is_some_and(|expr| expr.matches(ev));
    }
    if f.op == "detection" {
        return pf
            .detection
            .as_ref()
            .is_some_and(|(set, index)| set.rules[*index].matches(ev));
    }
    let col = f.column.as_str();
    let needle = pf.needle_lower.as_str();
    if let Some(set) = &pf.set {
        if f.op == "in_exact" { return ev.col_ref(col).is_some_and(|v| set.contains(v.as_ref())); }
        let hit = ev
            .col_ref(col)
            .is_some_and(|v| set.contains(&v.trim().to_lowercase()));
        return (f.op == "in") == hit;
    }
    if matches!(f.op.as_str(), "cidr" | "not_cidr") {
        let hit = ev
            .col_ref(col)
            .and_then(|v| crate::entities::parse_ip(&v))
            .is_some_and(|ip| pf.nets.iter().any(|n| n.contains(ip)));
        return (f.op == "cidr") == hit;
    }
    if f.op == "pattern" {
        return crate::insights::pattern_of(&ev.message) == f.value;
    }

    if f.op == "regex" {
        let hay: Cow<'_, str> = if col == "_all" {
            Cow::Owned(format!("{}\n{}", ev.message, ev.raw))
        } else {
            ev.col_ref(col).unwrap_or(Cow::Borrowed(""))
        };
        return match &pf.regex {
            Some(re) => re.is_match(&hay),
            // regex inválida → contém (case-insensitive ASCII, como ci_contains_bytes)
            None => false,
        };
    }

    if col == "_all" {
        let text = format!("{}\n{}", ev.message, ev.raw);
        return match f.op.as_str() {
            "contains" => ci_contains_bytes(text.as_bytes(), needle.as_bytes()),
            "not_contains" => !ci_contains_bytes(text.as_bytes(), needle.as_bytes()),
            _ => false,
        };
    }
    match f.op.as_str() {
        "contains" => ev
            .col_ref(col)
            .map(|s| ci_contains_bytes(s.as_bytes(), needle.as_bytes()))
            .unwrap_or(false),
        "not_contains" => ev
            .col_ref(col)
            .map(|s| !ci_contains_bytes(s.as_bytes(), needle.as_bytes()))
            .unwrap_or(true),
        "equals" => ev
            .col_ref(col)
            .map(|s| s.eq_ignore_ascii_case(f.value.trim()))
            .unwrap_or(false),
        "not_equals" => ev
            .col_ref(col)
            .map(|s| !s.eq_ignore_ascii_case(f.value.trim()))
            .unwrap_or(true),
        "equals_exact" => ev
            .col_ref(col)
            .map(|s| s.as_ref() == f.value)
            .unwrap_or(false),
        "not_equals_exact" => ev
            .col_ref(col)
            .map(|s| s.as_ref() != f.value)
            .unwrap_or(true),
        "starts_with" => ev
            .col_ref(col)
            .map(|s| {
                let (hay, ndl) = (s.as_bytes(), needle.as_bytes());
                hay.len() >= ndl.len() && hay[..ndl.len()].eq_ignore_ascii_case(ndl)
            })
            .unwrap_or(false),
        "empty" => ev.col_ref(col).map(|s| s.trim().is_empty()).unwrap_or(true),
        "not_empty" => ev
            .col_ref(col)
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false),
        "gt" | "gte" | "lt" | "lte" => {
            let (Some(a), Some(b)) = (ev.col_num(col), pf.num) else {
                return false;
            };
            match f.op.as_str() {
                "gt" => a > b,
                "gte" => a >= b,
                "lt" => a < b,
                _ => a <= b,
            }
        }
        "between" => {
            let (Some(a), Some(lo), Some(hi)) = (ev.col_num(col), pf.num, pf.num2) else {
                return false;
            };
            a >= lo && a <= hi
        }
        _ => false,
    }
}

/// Verifica um único filtro contra um evento (condições de campos derivados).
pub fn matches_filter(ev: &Event, filter: &Filter) -> bool {
    let prepared = prepare(std::slice::from_ref(filter));
    matches(ev, &prepared[0])
}

pub fn filtered_indices(events: &[Event], filters: &[Filter]) -> Vec<usize> {
    if filters.is_empty() {
        return (0..events.len()).collect();
    }
    let pfs = prepare(filters);
    if let (Some(engine), Some(predicate)) = (crate::case_cache::engine_for(events), index_predicate(&pfs)) {
        if let Ok(Some(candidates)) = engine.candidates_if_selective(&predicate, events.len()) {
            return candidates.into_iter()
                .take_while(|_| !crate::operations::cancelled())
                .filter(|&i| events.get(i).is_some_and(|ev| pfs.iter().all(|pf| matches(ev, pf))))
                .collect();
        }
    }
    events
        .iter()
        .enumerate()
        .take_while(|_| !crate::operations::cancelled())
        .filter(|(_, ev)| pfs.iter().all(|pf| matches(ev, pf)))
        .map(|(i, _)| i)
        .collect()
}

pub fn sort_indices(events: &[Event], indices: &mut [usize], column: &str, desc: bool) {
    // Pré-computa a chave (número opcional + texto em minúsculas) uma vez por
    // linha; o comparador anterior realocava as strings a cada comparação.
    let mut keyed: Vec<((Option<f64>, String), usize)> = indices
        .iter()
        .take_while(|_| !crate::operations::cancelled())
        .map(|&i| {
            let ev = &events[i];
            let num = ev.col_num(column);
            let text = ev.col_ref(column).unwrap_or_default().to_lowercase();
            ((num, text), i)
        })
        .collect();
    if keyed.len() != indices.len() {
        return;
    }
    keyed.sort_by(|((na, sa), _), ((nb, sb), _)| {
        let ord = compare_sort_keys(*na, sa, *nb, sb);
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
    for (slot, (_, i)) in indices.iter_mut().zip(keyed) {
        *slot = i;
    }
}

// A mixed numeric/text comparator must keep the two kinds in a fixed order.
// Falling back to text only for mixed pairs creates cycles (2 < 10 < 11x < 2).
fn compare_sort_keys(a: Option<f64>, sa: &str, b: Option<f64>, sb: &str) -> std::cmp::Ordering {
    match (a.filter(|n| n.is_finite()), b.filter(|n| n.is_finite())) {
        (Some(a), Some(b)) => a.total_cmp(&b).then_with(|| sa.cmp(sb)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => sa.cmp(sb),
    }
}

pub fn query(
    events: &[Event],
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> QueryResult {
    let mut idx = filtered_indices(events, filters);
    if !sort_column.is_empty() {
        sort_indices(events, &mut idx, sort_column, sort_dir == "desc");
    }
    let total = idx.len();
    let rows = idx
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut e = events[i].clone();
            crate::entities::annotate(&mut e);
            e.raw = String::new();
            e
        })
        .collect();
    QueryResult { total, rows }
}

fn query_from_memory_matches(
    events: &[Event],
    mut matched: Vec<usize>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> QueryResult {
    if !sort_column.is_empty() {
        sort_indices(events, &mut matched, sort_column, sort_dir == "desc");
    }
    let total = matched.len();
    let rows = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut event = events[i].clone();
            crate::entities::annotate(&mut event);
            event.raw.clear();
            event
        })
        .collect();
    QueryResult { total, rows }
}

fn aggregate_from_memory_matches(
    events: &[Event],
    matched: &[usize],
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order = Vec::new();
    for &i in matched {
        if crate::operations::cancelled() {
            break;
        }
        let event = &events[i];
        push_group(
            &mut groups,
            &mut order,
            event,
            event.col_str(group_column),
            specs,
        );
    }
    build_agg_result(groups, order, group_column, specs)
}

pub fn explore(
    events: &[Event],
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
) -> ExplorerSnapshot {
    let matched = filtered_indices(events, filters);
    let stats = stats_from(matched.iter().map(|&i| {
        let event = &events[i];
        (event.timestamp, event.level.as_str())
    }));
    let count = [AggSpec {
        func: "count".into(),
        column: "*".into(),
        alias: "n".into(),
    }];
    let sources = aggregate_from_memory_matches(events, &matched, "source", &count);
    let codes = aggregate_from_memory_matches(events, &matched, "code", &count);
    let query = query_from_memory_matches(events, matched, sort_column, sort_dir, offset, limit);
    ExplorerSnapshot {
        query,
        stats,
        sources,
        codes,
    }
}

// ==========================================================================
// Caminho indexado (arquivos grandes): filtra por metadados e só
// materializa o necessário.
// ==========================================================================

enum Tri {
    Pass,
    Fail,
    NeedEvent,
}

/// Index predicates are necessary conditions only. Every candidate is checked
/// by the original matcher; unsupported/negative conditions stay in that stage.
pub(crate) fn index_predicate(filters: &[PreparedFilter]) -> Option<crate::big_data::Predicate> {
    use crate::big_data::{ExactMode, Predicate};
    let parts: Vec<_> = filters.iter().filter_map(|pf| {
        let f = &pf.f;
        let scoped = (f.column != "_all").then(|| f.column.clone());
        let equals = |value: String, mode| Predicate::Equals { column: scoped.clone(), value, mode };
        match f.op.as_str() {
            "query" => pf.expr.as_ref()?.index_predicate(),
            "regex" => regex_index_predicate(pf.regex.as_ref()?.as_str()),
            "equals" if scoped.is_some() => Some(equals(f.value.trim().into(), ExactMode::AsciiInsensitive)),
            "equals_exact" if scoped.is_some() => Some(equals(f.value.clone(), ExactMode::Sensitive)),
            "in" if scoped.is_some() => Some(Predicate::Or(list_values(&f.value)
                .map(|v| equals(v.into(), ExactMode::TrimLower)).collect())),
            "in_exact" if scoped.is_some() => Some(Predicate::Or(f.value.lines().filter(|v| !v.is_empty())
                .map(|v| equals(v.into(), ExactMode::Sensitive)).collect())),
            "contains" | "starts_with" | "ends_with" if pf.needle_lower.chars().count() >= 3 => {
                Some(Predicate::Contains(pf.needle_lower.clone()))
            }
            "gt" | "gte" | "lt" | "lte" | "between" if f.column == "timestamp" => {
                timestamp_predicate(&f.op, pf.num?, pf.num2)
            }
            _ => None,
        }
    }).collect();
    (!parts.is_empty()).then_some(Predicate::And(parts))
}

/// The regex parser proves a prefix/suffix is necessary for every alternative.
/// Empty, infinite, short or non-ASCII literal sets keep the streaming path.
pub(crate) fn regex_index_predicate(pattern: &str) -> Option<crate::big_data::Predicate> {
    use regex_syntax::hir::literal::{ExtractKind, Extractor};
    use crate::big_data::Predicate;
    let hir = regex_syntax::parse(pattern).ok()?;
    let plans: Vec<_> = [ExtractKind::Prefix, ExtractKind::Suffix].into_iter().filter_map(|kind| {
        let sequence = Extractor::new().kind(kind).extract(&hir);
        let mut values = Vec::new();
        for literal in sequence.literals()? {
            let text = std::str::from_utf8(literal.as_bytes()).ok()?;
            // Unicode lowercasing can depend on surrounding characters. ASCII
            // literals avoid that ambiguity and are necessary in the folded pool.
            if !text.is_ascii() || text.len() < 3 { return None; }
            values.push(text.to_ascii_lowercase());
        }
        values.sort_unstable();
        values.dedup();
        (!values.is_empty()).then(|| Predicate::Or(values.into_iter().map(Predicate::Contains).collect()))
    }).collect();
    (!plans.is_empty()).then_some(Predicate::And(plans))
}

/// Round outwards so floating point thresholds cannot exclude a matching
/// integer timestamp. The event matcher retains inclusive/exclusive semantics.
pub(crate) fn timestamp_predicate(op: &str, a: f64, b: Option<f64>) -> Option<crate::big_data::Predicate> {
    let bound = |n: f64, lower: bool| {
        (n.is_finite() && n.abs() < 9_007_199_254_740_992.0)
            .then(|| (if lower { n.floor() } else { n.ceil() } as i64, true))
    };
    let (lower, upper) = match op {
        "gt" | "gte" => (Some(bound(a, true)?), None),
        "lt" | "lte" => (None, Some(bound(a, false)?)),
        "between" => (Some(bound(a, true)?), Some(bound(b?, false)?)),
        _ => return None,
    };
    Some(crate::big_data::Predicate::Timestamp { lower, upper })
}

// These predicates depend only on standard string columns. Evaluate the
// original matcher over their FAST projection, avoiding stored-event block
// decompression for scattered matches. Other predicates need the full event.
fn fast_filter_columns(filters: &[PreparedFilter]) -> Option<[bool; 3]> {
    let mut columns = [false; 3];
    for pf in filters {
        if !matches!(pf.f.op.as_str(), "contains" | "not_contains" | "equals" | "not_equals" |
            "equals_exact" | "not_equals_exact" | "in" | "in_exact" | "not_in" | "regex" |
            "starts_with" | "ends_with" | "empty" | "not_empty" | "gt" | "gte" | "lt" | "lte" | "between") {
            return None;
        }
        let slot = match pf.f.column.as_str() { "source" => 0, "code" => 1, "level" => 2, _ => return None };
        columns[slot] = true;
    }
    Some(columns)
}

fn fast_candidate_matches(engine: &crate::big_data::BigDataIndex, row: usize, filters: &[PreparedFilter], columns: [bool; 3]) -> bool {
    let mut event = Event::empty();
    if columns[0] { event.source = engine.col_str(row, "source").unwrap_or_default(); }
    if columns[1] { event.code = engine.col_str(row, "code").unwrap_or_default(); }
    if columns[2] { event.level = engine.col_str(row, "level").unwrap_or_default(); }
    filters.iter().all(|pf| matches(&event, pf))
}

type FastProjectionKey = (u32, [Option<u64>; 3]);

fn cached_fast_candidate(engine: &crate::big_data::BigDataIndex, row: usize, filters: &[PreparedFilter], columns: [bool; 3], cache: &mut HashMap<FastProjectionKey, bool>) -> bool {
    let key = engine.projection_key(row, columns);
    if let Some(result) = key.and_then(|key| cache.get(&key)) { return *result; }
    let result = fast_candidate_matches(engine, row, filters, columns);
    // The original predicate is constant for an identical projection within
    // this query. Bound reuse when a categorical field has high cardinality.
    if cache.len() < 4096 { if let Some(key) = key { cache.insert(key, result); } }
    result
}

pub(crate) fn ci_contains_bytes(hay: &[u8], needle_lower: &[u8]) -> bool {
    if !hay.is_ascii() || !needle_lower.is_ascii() {
        return String::from_utf8_lossy(hay)
            .to_lowercase()
            .contains(String::from_utf8_lossy(needle_lower).as_ref());
    }
    let Some((&first, rest)) = needle_lower.split_first() else {
        return true;
    };
    let mut from = 0usize;
    // memchr2 no primeiro byte (duas caixas) e confirma o restante — ordens de
    // grandeza mais rápido que varrer byte a byte.
    while let Some(p) = if first.is_ascii_alphabetic() {
        memchr::memchr2(first, first.to_ascii_uppercase(), &hay[from..])
    } else {
        memchr::memchr(first, &hay[from..])
    } {
        let i = from + p;
        let end = i + needle_lower.len();
        if end <= hay.len() && hay[i + 1..end].eq_ignore_ascii_case(rest) {
            return true;
        }
        from = i + 1;
        if from >= hay.len() {
            break;
        }
    }
    false
}

fn meta_check(pf: &PreparedFilter, meta: &LineMeta, line: &[u8], enriched: bool) -> Tri {
    let f = &pf.f;
    let op = f.op.as_str();
    if op == "query" {
        let Some(expr) = &pf.expr else { return Tri::Fail };
        // Catalog names/descriptions and derived fields are absent from the
        // raw line; free text that may match them needs the parsed event.
        if enriched {
            return Tri::NeedEvent;
        }
        let escaped = memchr::memchr(b'\\', line).is_some();
        return match expr.line_check(meta, line, escaped) {
            Some(true) => Tri::Pass,
            Some(false) => Tri::Fail,
            None => Tri::NeedEvent,
        };
    }
    // Exact selection uses decoded field values, including whitespace and case.
    // Raw spans can contain JSON escapes or represent a normalized standard field.
    if matches!(op, "equals_exact" | "not_equals_exact" | "threat_rule") {
        return Tri::NeedEvent;
    }
    let v = f.value.trim();
    // equivalente a v.to_lowercase() (trim e lowercase comutam para whitespace)
    let needle = pf.needle_lower.trim();
    if matches!(f.column.as_str(), "message" | "_all") && memchr::memchr(b'\\', line).is_some() {
        return Tri::NeedEvent;
    }
    match f.column.as_str() {
        // Zero is both the metadata sentinel and the valid Unix epoch. Only
        // the event can distinguish an absent date from Some(0).
        "timestamp" if meta.ts == 0 => Tri::NeedEvent,
        "timestamp" => match op {
            "gt" | "gte" | "lt" | "lte" | "between" => {
                let a = meta.ts as f64;
                // limites pré-computados em prepare (pf.num/pf.num2)
                let pass = match op {
                    "gt" => pf.num.map(|b| a > b),
                    "gte" => pf.num.map(|b| a >= b),
                    "lt" => pf.num.map(|b| a < b),
                    "lte" => pf.num.map(|b| a <= b),
                    _ => match (pf.num, pf.num2) {
                        (Some(lo), Some(hi)) => Some(a >= lo && a <= hi),
                        _ => None,
                    },
                };
                match pass {
                    Some(true) => Tri::Pass,
                    Some(false) => Tri::Fail,
                    None => Tri::NeedEvent,
                }
            }
            "empty" => Tri::Fail,
            "not_empty" => Tri::Pass,
            _ => Tri::NeedEvent,
        },
        "level" => match (op, label_class(v)) {
            ("equals", Some(c)) => {
                if meta.level == c {
                    Tri::Pass
                } else if meta.level == LV_OTHER {
                    Tri::NeedEvent
                } else {
                    Tri::Fail
                }
            }
            ("not_equals", Some(c)) => {
                if meta.level == c {
                    Tri::Fail
                } else if meta.level == LV_OTHER {
                    Tri::NeedEvent
                } else {
                    Tri::Pass
                }
            }
            _ => Tri::NeedEvent,
        },
        "code" => {
            if meta.code_len == 0 {
                return Tri::NeedEvent;
            }
            let code = meta.code(line);
            match op {
                "equals" | "not_equals" => {
                    let eq = code.eq_ignore_ascii_case(v.as_bytes());
                    if (op == "equals") == eq {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                "contains" | "not_contains" => {
                    if v.is_empty() {
                        return Tri::NeedEvent;
                    }
                    let has = !code.is_empty() && ci_contains_bytes(code, needle.as_bytes());
                    if (op == "contains") == has {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                "empty" => {
                    if code.is_empty() {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                "not_empty" => {
                    if !code.is_empty() {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                _ => Tri::NeedEvent,
            }
        }
        "message" => match op {
            "contains" | "not_contains" => {
                if v.is_empty() {
                    return Tri::NeedEvent;
                }
                let has = ci_contains_bytes(line, needle.as_bytes());
                if !has {
                    // certeza sem parsear
                    if op == "contains" {
                        Tri::Fail
                    } else {
                        Tri::Pass
                    }
                } else {
                    Tri::NeedEvent // confirmar no evento parseado
                }
            }
            // Anchors refer to the parsed message, not the surrounding JSON.
            "regex" => Tri::NeedEvent,
            _ => Tri::NeedEvent,
        },
        "_all" => match op {
            "regex" => Tri::NeedEvent,
            "contains" | "not_contains" => {
                if v.is_empty() {
                    return Tri::NeedEvent;
                }
                let has = ci_contains_bytes(line, needle.as_bytes());
                if (op == "contains") == has {
                    Tri::Pass
                } else {
                    Tri::Fail
                }
            }
            _ => Tri::NeedEvent,
        },
        _ => Tri::NeedEvent,
    }
}

struct MatchCacheEntry {
    idx_id: usize,
    idx_identity: String,
    lines_count: usize,
    filters_key: String,
    derived_count: usize,
    codes_count: usize,
    matches: Arc<Vec<usize>>,
}

static MATCH_CACHE: Mutex<Vec<MatchCacheEntry>> = Mutex::new(Vec::new());
const MATCH_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// Capacity accounting over at most 16 cache entries, without cloning matches.
pub(crate) fn resource_cache_metrics() -> Option<(usize, u64)> {
    let cache = MATCH_CACHE.try_lock()?;
    let mut bytes = (cache.capacity() as u64).saturating_mul(std::mem::size_of::<MatchCacheEntry>() as u64);
    let mut seen = std::collections::HashSet::new();
    for entry in cache.iter() {
        bytes = bytes.saturating_add(entry.idx_identity.capacity() as u64).saturating_add(entry.filters_key.capacity() as u64);
        if seen.insert(Arc::as_ptr(&entry.matches) as usize) {
            bytes = bytes.saturating_add((entry.matches.capacity() as u64).saturating_mul(std::mem::size_of::<usize>() as u64))
                .saturating_add(std::mem::size_of::<Vec<usize>>() as u64 + 2 * std::mem::size_of::<usize>() as u64);
        }
    }
    Some((cache.len(), bytes))
}

fn filters_cache_key(filters: &[Filter]) -> String {
    // JSON escaping makes arbitrary field names/values unambiguous.
    serde_json::to_string(filters).expect("serializable filters")
}

pub fn clear_match_cache() {
    MATCH_CACHE.lock().clear();
}

/// Filtra as linhas do índice; materializa apenas quando necessário.
pub fn indexed_matches(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Vec<usize> {
    Arc::unwrap_or_clone(indexed_matches_shared(idx, filters, codes, system, derived))
}

/// Read-only consumers share the cached selection. A full copy is needed only
/// by the compatibility API above or when a caller actually reorders IDs.
pub(crate) fn indexed_matches_shared(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Arc<Vec<usize>> {
    if filters.is_empty() {
        return Arc::new((0..idx.lines.len()).collect());
    }
    let idx_id = idx.lines.as_ptr() as usize;
    let lines_count = idx.lines.len();
    let idx_identity = idx.parts.iter().map(|p| p.identity.as_str()).collect::<Vec<_>>().join(",");
    let fkey = filters_cache_key(filters);
    let derived_count = derived.len();
    let codes_count = codes.sources.len() + system.sources.len();
    let same_entry = |e: &MatchCacheEntry| {
        e.idx_id == idx_id
            && e.lines_count == lines_count
            && e.derived_count == derived_count
            && e.codes_count == codes_count
            && e.idx_identity == idx_identity
            && e.filters_key == fkey
    };

    {
        let cache = MATCH_CACHE.lock();
        if let Some(entry) = cache.iter().find(|e| same_entry(e)) {
            return Arc::clone(&entry.matches);
        }
    }

    let mut matched = Vec::new();
    visit_indexed_matches(idx, filters, codes, system, derived, |i| matched.push(i));
    let matched = Arc::new(matched);

    // Vec capacity is the actual retained allocation, which can exceed len.
    let bytes = matched.capacity().saturating_mul(std::mem::size_of::<usize>());
    if !crate::operations::cancelled() && bytes <= MATCH_CACHE_BYTES / 4 {
        let mut cache = MATCH_CACHE.lock();
        // Concurrent misses may already have published this same selection.
        if let Some(entry) = cache.iter().find(|e| same_entry(e)) {
            return Arc::clone(&entry.matches);
        }
        while !cache.is_empty() && (cache.len() >= 16 ||
            cache.iter().map(|e| e.matches.capacity() * std::mem::size_of::<usize>()).sum::<usize>() + bytes > MATCH_CACHE_BYTES) {
            cache.remove(0);
        }
        cache.push(MatchCacheEntry {
            idx_id,
            idx_identity,
            lines_count,
            filters_key: fkey,
            derived_count,
            codes_count,
            matches: Arc::clone(&matched),
        });
    }

    matched
}

/// A few legacy raw-line shortcuts normalize whitespace differently from the
/// decoded event matcher. Preserve that tri-state path for those queries: a
/// selective index plan could otherwise remove rows before metadata is checked.
/// In-memory Cases have no raw metadata shortcuts and keep their normal plan.
fn needs_metadata_filter_path(filters: &[PreparedFilter]) -> bool {
    filters.iter().any(|pf| {
        (pf.f.column == "code" && matches!(pf.f.op.as_str(), "empty" | "not_empty"))
            || (matches!(pf.f.column.as_str(), "code" | "message" | "_all")
                && matches!(pf.f.op.as_str(), "contains" | "not_contains")
                && pf.f.value != pf.f.value.trim())
    })
}

/// Visit matching positions without allocating a vector proportional to the file.
pub fn visit_indexed_matches(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    mut visit: impl FnMut(usize),
) {
    let pfs = prepare(filters);
    if let (Some(engine), Some(predicate)) = (&idx.big_data, (!needs_metadata_filter_path(&pfs)).then(|| index_predicate(&pfs)).flatten()) {
        if let Ok(Some(candidates)) = engine.candidates_if_selective(&predicate, idx.lines.len()) {
            let fast_columns = fast_filter_columns(&pfs);
            let mut verification = HashMap::new();
            for i in candidates {
                if crate::operations::cancelled() { break; }
                if let Some(columns) = fast_columns {
                    if cached_fast_candidate(engine, i, &pfs, columns, &mut verification) { visit(i); }
                    continue;
                }
                let ev = event_at(idx, i, codes, system, derived);
                if pfs.iter().all(|pf| matches(&ev, pf)) { visit(i); }
            }
            return;
        }
    }
    visit_indexed_prepared(idx, &pfs, codes, system, derived, visit);
}

/// Lines per parallel work unit. Batches keep memory bounded and results in order.
const SCAN_CHUNK: usize = 8192;

/// Scans the index in parallel chunks, calling `map` for each matching line
/// (with the parsed event when one was needed) and `visit` in file order.
/// Statistics request the legacy metadata/event projection; selection-only
/// callers can verify FAST columns without materializing a full event.
pub(crate) fn scan_indexed<T: Send>(
    idx: &FileIndex,
    pfs: &[PreparedFilter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    preserve_event_projection: bool,
    map: impl Fn(usize, &LineMeta, Option<Event>) -> T + Sync,
    mut visit: impl FnMut(T),
) {
    if let (Some(engine), Some(predicate)) = (&idx.big_data, (!needs_metadata_filter_path(pfs)).then(|| index_predicate(pfs)).flatten()) {
        if let Ok(Some(candidates)) = engine.candidates_if_selective(&predicate, idx.lines.len()) {
            // Bounded ordered batches share parsed-event blocks between workers.
            let generation = crate::operations::current_generation();
            let fast_columns = fast_filter_columns(pfs);
            let mut verification = HashMap::new();
            let enriched: Vec<_> = pfs.iter()
                .map(|pf| query_needs_enrichment(pf, codes, system, derived)).collect();
            // Preserve statistics' existing distinction between metadata-only
            // filters and filters that require the original decoded event.
            let needs_event = |i: usize| preserve_event_projection && pfs.iter().zip(&enriched)
                .any(|(pf, enriched)| matches!(meta_check(pf, &idx.lines[i], line_bytes(idx, i), *enriched), Tri::NeedEvent));
            for batch in candidates.chunks(SCAN_CHUNK) {
                if crate::operations::cancelled() { break; }
                if let Some(columns) = fast_columns {
                    let selected: Vec<_> = batch.iter().copied().filter(|&i|
                        !crate::operations::cancelled_for(generation) && cached_fast_candidate(engine, i, pfs, columns, &mut verification)).collect();
                    let items: Vec<_> = selected.par_iter().take_any_while(|_| !crate::operations::cancelled_for(generation))
                        .map(|&i| {
                            let ev = needs_event(i).then(|| event_at(idx, i, codes, system, derived));
                            map(i, &idx.lines[i], ev)
                        }).collect();
                    for item in items { visit(item); }
                    continue;
                }
                let items: Vec<_> = batch.par_iter().filter_map(|&i| {
                    if crate::operations::cancelled_for(generation) { return None; }
                    let ev = event_at(idx, i, codes, system, derived);
                    pfs.iter().all(|pf| matches(&ev, pf)).then(|| {
                        let projected = (!preserve_event_projection || needs_event(i)).then_some(ev);
                        map(i, &idx.lines[i], projected)
                    })
                }).collect();
                for item in items { visit(item); }
            }
            return;
        }
    }
    let enriched: Vec<bool> = pfs
        .iter()
        .map(|pf| query_needs_enrichment(pf, codes, system, derived))
        .collect();
    let generation = crate::operations::current_generation();
    // Broad or negative categorical filters may have no selective postings.
    // Their original matcher still needs only these persisted string columns.
    // Keep metadata tri-state checks first: their raw-span normalization can
    // differ from decoded values. FAST replaces only an otherwise needed event.
    // Statistics retain their metadata/event projection policy below.
    let broad_projection = (!preserve_event_projection && !pfs.is_empty())
        .then(|| idx.big_data.as_ref().zip(fast_filter_columns(pfs)))
        .flatten();
    let chunk = |from: usize, to: usize| -> Vec<T> {
        let mut out = Vec::new();
        let mut verification = HashMap::new();
        for i in from..to {
            if (i - from) % 2048 == 0 && crate::operations::cancelled_for(generation) {
                break;
            }
            let meta = &idx.lines[i];
            let line = line_bytes(idx, i);
            let mut need = false;
            let mut ok = true;
            for (pf, enriched) in pfs.iter().zip(&enriched) {
                match meta_check(pf, meta, line, *enriched) {
                    Tri::Fail => {
                        ok = false;
                        break;
                    }
                    Tri::NeedEvent => need = true,
                    Tri::Pass => {}
                }
            }
            if !ok {
                continue;
            }
            if need {
                if let Some((engine, columns)) = broad_projection {
                    if cached_fast_candidate(engine, i, pfs, columns, &mut verification) {
                        out.push(map(i, meta, None));
                    }
                    continue;
                }
                let ev = event_at(idx, i, codes, system, derived);
                if pfs.iter().all(|pf| matches(&ev, pf)) {
                    out.push(map(i, meta, Some(ev)));
                }
            } else {
                out.push(map(i, meta, None));
            }
        }
        out
    };
    let total = idx.lines.len();
    if total <= SCAN_CHUNK * 2 || rayon::current_num_threads() <= 1 {
        for item in chunk(0, total) {
            visit(item);
        }
        return;
    }
    let batch = SCAN_CHUNK * rayon::current_num_threads() * 2;
    let mut start = 0;
    while start < total {
        if crate::operations::cancelled() {
            break;
        }
        let end = (start + batch).min(total);
        let parts: Vec<Vec<T>> = (start..end)
            .step_by(SCAN_CHUNK)
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|from| chunk(from, (from + SCAN_CHUNK).min(end)))
            .collect();
        for part in parts {
            for item in part {
                visit(item);
            }
        }
        start = end;
    }
}

pub(crate) fn visit_indexed_prepared(
    idx: &FileIndex,
    pfs: &[PreparedFilter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    visit: impl FnMut(usize),
) {
    scan_indexed(idx, pfs, codes, system, derived, false, |i, _, _| i, visit);
}

/// Free text in a search expression may match code names/descriptions from
/// the catalogs or derived fields, which are not part of the raw line.
fn query_needs_enrichment(
    pf: &PreparedFilter,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> bool {
    let Some(expr) = &pf.expr else { return false };
    let mut needles = Vec::new();
    expr.free_text_needles(&mut needles);
    if needles.is_empty() {
        return false;
    }
    if !derived.is_empty() {
        return true;
    }
    [codes, system].iter().any(|catalog| {
        catalog.sources.values().flat_map(|m| m.values()).any(|info| {
            let name = info.name.to_lowercase();
            let description = info.description.to_lowercase();
            needles.iter().any(|n| name.contains(n.as_str()) || description.contains(n.as_str()))
        })
    })
}

pub fn query_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> QueryResult {
    let matched = indexed_matches_shared(idx, filters, codes, system, derived);
    query_from_indexed_matches(
        idx,
        matched,
        sort_column,
        sort_dir,
        offset,
        limit,
        codes,
        system,
        derived,
    )
}

/// Same-binary benchmark reference: explicitly recover an owned selection,
/// reproducing the previous warm-cache Vec clone before identical page work.
/// This is not the historical binary or a second production implementation.
#[cfg(test)]
pub(crate) fn query_indexed_owned_reference(
    idx: &FileIndex,
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> QueryResult {
    let matched = Arc::new(indexed_matches(idx, filters, codes, system, derived));
    query_from_indexed_matches(idx, matched, sort_column, sort_dir, offset, limit, codes, system, derived)
}

fn sort_time(idx: &FileIndex, matched: &mut Vec<usize>, desc: bool) {
    if matched.len() > idx.lines.len() / 4 {
        if matched.len() == idx.lines.len() {
            *matched = idx.ordered().to_vec();
        } else {
            let mut selected = vec![false; idx.lines.len()];
            for &i in matched.iter() {
                selected[i] = true;
            }
            *matched = idx
                .ordered()
                .iter()
                .copied()
                .filter(|&i| selected[i])
                .collect();
        }
        if desc {
            matched.reverse();
            let mut start = 0;
            while start < matched.len() {
                let mut end = start + 1;
                while end < matched.len()
                    && idx.lines[matched[end]].ts == idx.lines[matched[start]].ts
                {
                    end += 1;
                }
                matched[start..end].reverse();
                start = end;
            }
        }
    } else {
        matched.sort_by(|&a, &b| {
            let ord = idx.lines[a].ts.cmp(&idx.lines[b].ts);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }
}

// ==========================================================================
// Agregações
// ==========================================================================

enum Acc {
    Count(u64),
    CountDistinct(crate::distinct::Counter),
    Sum(f64, [usize; 5]),
    Avg(f64, u64, [usize; 5]),
    Min(Option<f64>, [usize; 5]),
    Max(Option<f64>, [usize; 5]),
    StrAgg(Vec<String>),
}

impl Acc {
    fn new(func: &str) -> Self {
        match func {
            "count" => Acc::Count(0),
            "count_distinct" => Acc::CountDistinct(Default::default()),
            "sum" => Acc::Sum(0.0, [0; 5]),
            "avg" => Acc::Avg(0.0, 0, [0; 5]),
            "min" => Acc::Min(None, [0; 5]),
            "max" => Acc::Max(None, [0; 5]),
            _ => Acc::StrAgg(Vec::new()),
        }
    }

    fn push(&mut self, ev: &Event, column: &str) {
        let numeric = || {
            if matches!(column, "timestamp" | "id") {
                ev.col_num(column)
                    .map(|value| (value, crate::analysis::UnitKind::Number))
            } else {
                ev.col_ref(column)
                    .and_then(|value| crate::analysis::parse_num_unit(&value))
            }
        };
        match self {
            Acc::Count(n) => *n += 1,
            Acc::CountDistinct(set) => {
                if let Some(s) = ev.col_str(column) {
                    set.insert(s);
                }
            }
            Acc::Sum(n, units) => {
                if let Some((v, unit)) = numeric() {
                    *n += v;
                    units[unit as usize] += 1;
                }
            }
            Acc::Avg(sum, n, units) => {
                if let Some((v, unit)) = numeric() {
                    *sum += v;
                    *n += 1;
                    units[unit as usize] += 1;
                }
            }
            Acc::Min(cur, units) => {
                if let Some((v, unit)) = numeric() {
                    *cur = Some(cur.map(|c: f64| c.min(v)).unwrap_or(v));
                    units[unit as usize] += 1;
                }
            }
            Acc::Max(cur, units) => {
                if let Some((v, unit)) = numeric() {
                    *cur = Some(cur.map(|c: f64| c.max(v)).unwrap_or(v));
                    units[unit as usize] += 1;
                }
            }
            Acc::StrAgg(items) => {
                if items.len() < 100 {
                    if let Some(s) = ev.col_str(column) {
                        if !s.is_empty() {
                            items.push(s);
                        }
                    }
                }
            }
        }
    }

    fn units(&self) -> &[usize; 5] {
        match self {
            Acc::Sum(_, units)
            | Acc::Avg(_, _, units)
            | Acc::Min(_, units)
            | Acc::Max(_, units) => units,
            _ => &[0; 5],
        }
    }

    fn finish(&self, ev_col: &str) -> Value {
        match self {
            Acc::Count(n) => Value::from(*n),
            Acc::CountDistinct(set) => Value::from(set.len() as u64),
            Acc::Sum(n, _) => round2(*n).into(),
            Acc::Avg(sum, n, _) => {
                if *n == 0 {
                    Value::Null
                } else {
                    round2(*sum / *n as f64).into()
                }
            }
            Acc::Min(cur, _) | Acc::Max(cur, _) => match cur {
                Some(v) if ev_col == "timestamp" => Value::from(crate::model::ts_to_iso(*v as i64)),
                Some(v) => round2(*v).into(),
                None => Value::Null,
            },
            Acc::StrAgg(items) => {
                let mut s = items.join(", ");
                if items.len() >= 100 {
                    s.push_str(", …");
                }
                Value::from(s)
            }
        }
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ------------------------------------------------------------------ multi-agregação

/// Contagens por valor para várias colunas de uma vez (árvore de exploração).
/// Cada coluna é agregada com os filtros MENOS o filtro da própria coluna,
/// garantindo que toda opção exibida exista no recorte complementar.
/// As colunas são processadas em paralelo (rayon).
pub fn multi_count(
    events: &[Event],
    filters: &[Filter],
    columns: &[String],
) -> Vec<(String, AggResult)> {
    let generation = crate::operations::current_generation();
    columns
        .par_iter()
        .map(|col| {
            let fs: Vec<Filter> = filters
                .iter()
                .filter(|f| f.column != *col)
                .cloned()
                .collect();
            let run = || {
                aggregate(
                    events,
                    &fs,
                    col,
                    &[AggSpec {
                        func: "count".into(),
                        column: "*".into(),
                        alias: "n".into(),
                    }],
                )
            };
            let agg = match generation {
                Some(g) => crate::operations::run(g, run).unwrap_or_default(),
                None => run(),
            };
            (col.clone(), agg)
        })
        .collect()
}

/// Versão indexada (mmap) da multi-agregação.
pub fn multi_count_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    columns: &[String],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Vec<(String, AggResult)> {
    let generation = crate::operations::current_generation();
    columns
        .par_iter()
        .map(|col| {
            let fs: Vec<Filter> = filters
                .iter()
                .filter(|f| f.column != *col)
                .cloned()
                .collect();
            let run = || {
                aggregate_indexed(
                    idx,
                    &fs,
                    col,
                    &[AggSpec {
                        func: "count".into(),
                        column: "*".into(),
                        alias: "n".into(),
                    }],
                    codes,
                    system,
                    derived,
                )
            };
            let agg = match generation {
                Some(g) => crate::operations::run(g, run).unwrap_or_default(),
                None => run(),
            };
            (col.clone(), agg)
        })
        .collect()
}

fn build_agg_result(
    groups: HashMap<Option<String>, Vec<Acc>>,
    mut order: Vec<Option<String>>,
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    order.sort_by(|a, b| {
        let (sa, sb) = (a.as_deref().unwrap_or(""), b.as_deref().unwrap_or(""));
        compare_sort_keys(
            sa.parse().ok(),
            &sa.to_lowercase(),
            sb.parse().ok(),
            &sb.to_lowercase(),
        )
        .then_with(|| a.cmp(b))
    });

    let alias = |s: &AggSpec| {
        if s.alias.is_empty() {
            format!("{}({})", s.func, s.column)
        } else {
            s.alias.clone()
        }
    };
    let mut columns = vec![group_column.to_string()];
    columns.extend(specs.iter().map(&alias));
    let mut incompatible_units = vec![0; specs.len()];
    let mut value_units = vec![String::new(); specs.len()];
    for i in 0..specs.len() {
        let mut counts = [0usize; 5];
        for accs in groups.values() {
            for (total, count) in counts.iter_mut().zip(accs[i].units()) {
                *total += count;
            }
        }
        if let Some((unit, count)) = counts
            .iter()
            .enumerate()
            .max_by_key(|(unit, count)| (**count, std::cmp::Reverse(*unit)))
            .filter(|(_, count)| **count > 0)
        {
            incompatible_units[i] = counts.iter().sum::<usize>() - count;
            value_units[i] = ["number", "bytes", "bits", "duration", "number"][unit].into();
        }
    }

    let group_values = order.clone();
    let rows = order
        .into_iter()
        .map(|key| {
            let accs = &groups[&key];
            let mut row = Map::new();
            row.insert(
                group_column.to_string(),
                Value::from(key.unwrap_or_else(|| "(vazio)".into())),
            );
            for (i, (acc, spec)) in accs.iter().zip(specs.iter()).enumerate() {
                row.insert(
                    alias(spec),
                    if incompatible_units[i] > 0 {
                        Value::Null
                    } else {
                        acc.finish(&spec.column)
                    },
                );
            }
            row
        })
        .collect();

    AggResult {
        columns,
        rows,
        group_values,
        incompatible_units,
        value_units,
    }
}

pub fn aggregate(
    events: &[Event],
    filters: &[Filter],
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    let idx = filtered_indices(events, filters);
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order: Vec<Option<String>> = Vec::new();

    for i in idx {
        if crate::operations::cancelled() {
            break;
        }
        let ev = &events[i];
        push_group(&mut groups, &mut order, ev, ev.col_str(group_column), specs);
    }
    build_agg_result(groups, order, group_column, specs)
}

fn push_group(
    groups: &mut HashMap<Option<String>, Vec<Acc>>,
    order: &mut Vec<Option<String>>,
    ev: &Event,
    key: Option<String>,
    specs: &[AggSpec],
) {
    let key = key.filter(|value| !value.trim().is_empty());
    // Caminho quente é o grupo já existente: get_mut evita clonar a chave
    // a cada linha; só clona ao criar um grupo novo.
    if let Some(accs) = groups.get_mut(&key) {
        for (acc, spec) in accs.iter_mut().zip(specs.iter()) {
            acc.push(ev, &spec.column);
        }
        return;
    }
    order.push(key.clone());
    let mut accs: Vec<Acc> = specs.iter().map(|s| Acc::new(&s.func)).collect();
    for (acc, spec) in accs.iter_mut().zip(specs.iter()) {
        acc.push(ev, &spec.column);
    }
    groups.insert(key, accs);
}

/// Colunas disponíveis direto dos metadados (sem parse da linha).
fn is_meta_column(col: &str) -> bool {
    matches!(col, "*" | "timestamp" | "level")
}

/// Evento parcial montado só com os campos dos metadados.
fn meta_event(idx: &FileIndex, i: usize) -> Event {
    let m = &idx.lines[i];
    let mut ev = Event::empty();
    ev.id = i;
    if m.ts != 0 {
        ev.timestamp = Some(m.ts);
    }
    ev.level = crate::model::class_label(m.level).to_string();
    let code = m.code(line_bytes(idx, i));
    if !code.is_empty() {
        ev.code = String::from_utf8_lossy(code).into_owned();
    }
    ev
}

pub fn aggregate_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    group_column: &str,
    specs: &[AggSpec],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> AggResult {
    let matched = indexed_matches_shared(idx, filters, codes, system, derived);
    if let Some(result) = fast_count_aggregation(idx, &matched, group_column, specs) { return result; }
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order: Vec<Option<String>> = Vec::new();

    // Caminho rápido: grupo e agregações só sobre colunas de metadados
    // (timestamp, level, code) — zero parse por linha.
    let meta_only = is_meta_column(group_column) && specs.iter().all(|s| is_meta_column(&s.column));
    for &i in matched.iter() {
        if crate::operations::cancelled() {
            break;
        }
        if meta_only {
            let ev = meta_event(idx, i);
            push_group(
                &mut groups,
                &mut order,
                &ev,
                ev.col_str(group_column),
                specs,
            );
        } else {
            let ev = event_at(idx, i, codes, system, derived);
            push_group(
                &mut groups,
                &mut order,
                &ev,
                ev.col_str(group_column),
                specs,
            );
        }
    }
    build_agg_result(groups, order, group_column, specs)
}

// ==========================================================================
// Estatísticas (histograma + níveis)
// ==========================================================================

/// Rank the distinct categorical sort keys, then select the requested range
/// within each rank. Deep pages do not require sorting or decoding the prefix.
/// The key comparator and position tie-break exactly match the stable legacy
/// sort, including numeric strings, units, Unicode folding and segment overlap.
fn categorical_page(
    engine: &crate::big_data::BigDataIndex,
    matched: &[usize],
    column: &str,
    desc: bool,
    offset: usize,
    limit: usize,
) -> Option<Vec<usize>> {
    let slot = match column { "source" => 0, "code" => 1, _ => return None };
    let mut columns = [false; 3];
    columns[slot] = true;
    let generation = crate::operations::current_generation();
    let mut dictionary = HashMap::new();
    let mut keys: Vec<(Option<f64>, String, usize)> = Vec::new();
    let mut row_keys = Vec::with_capacity(matched.len());
    for (position, &row) in matched.iter().enumerate() {
        if position % 2048 == 0 && crate::operations::cancelled_for(generation) {
            return None;
        }
        let (segment, ordinals) = engine.projection_key(row, columns)?;
        let ordinal = ordinals[slot];
        let key = match dictionary.entry((segment, ordinal)) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                // A missing ordinal represents the empty string. A dictionary
                // decoding error requests the original stored-event path.
                let value = match ordinal {
                    Some(_) => engine.col_str(row, column)?,
                    None => String::new(),
                };
                let mut event = Event::empty();
                if slot == 0 { event.source = value; } else { event.code = value; }
                let key = keys.len();
                keys.push((event.col_num(column), event.col_ref(column)?.to_lowercase(), 0));
                entry.insert(key);
                key
            }
        };
        keys[key].2 += 1;
        row_keys.push(key);
    }
    drop(dictionary);
    let compare = |a: usize, b: usize| {
        let order = compare_sort_keys(keys[a].0, &keys[a].1, keys[b].0, &keys[b].1);
        if desc { order.reverse() } else { order }
    };
    let mut ordered: Vec<_> = (0..keys.len()).collect();
    ordered.sort_unstable_by(|&a, &b| compare(a, b));
    let mut ranks = vec![0; keys.len()];
    let mut counts: Vec<usize> = Vec::new();
    let mut previous = None;
    for key in ordered {
        if !previous.is_some_and(|prev| compare(prev, key).is_eq()) {
            counts.push(0);
        }
        let rank = counts.len() - 1;
        ranks[key] = rank;
        counts[rank] += keys[key].2;
        previous = Some(key);
    }
    // Equal comparison keys from different segments share a rank, preserving
    // the original row order across all segments rather than segment order.
    let page_end = offset.saturating_add(limit).min(matched.len());
    let mut base = 0usize;
    let ranges: Vec<_> = counts.iter().map(|&count| {
        let range = (offset.saturating_sub(base).min(count), page_end.saturating_sub(base).min(count));
        base += count;
        range
    }).collect();
    let mut seen = vec![0usize; counts.len()];
    let mut page = Vec::with_capacity(page_end.saturating_sub(offset));
    for (position, (&row, key)) in matched.iter().zip(row_keys).enumerate() {
        if position % 2048 == 0 && crate::operations::cancelled_for(generation) {
            return None;
        }
        let rank = ranks[key];
        let within = seen[rank];
        seen[rank] += 1;
        let (from, to) = ranges[rank];
        if within >= from && within < to { page.push((rank, row)); }
    }
    if crate::operations::cancelled_for(generation) { return None; }
    page.sort_unstable();
    Some(page.into_iter().map(|(_, row)| row).collect())
}

fn query_from_indexed_matches(
    idx: &FileIndex,
    mut matched: Arc<Vec<usize>>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> QueryResult {
    let desc = sort_dir == "desc";
    let total = matched.len();
    if limit == 0 || offset >= total {
        return QueryResult { total, rows: Vec::new() };
    }
    let page_end = offset.saturating_add(limit).min(total);
    let mut page_offset = offset;
    let categorical = idx.big_data.as_ref()
        .and_then(|engine| categorical_page(engine, &matched, sort_column, desc, offset, limit));
    if let Some(page) = categorical {
        matched = Arc::new(page);
        page_offset = 0;
    } else if idx.big_data.is_some() && matches!(sort_column, "timestamp" | "level") && page_end < total {
        let matched = Arc::make_mut(&mut matched);
        // Only the requested prefix needs ordering. Include the source position
        // in ties to reproduce the stable ordering of the full legacy sort.
        let compare = |&a: &usize, &b: &usize| {
            let order = if sort_column == "timestamp" { idx.lines[a].ts.cmp(&idx.lines[b].ts) }
                else { idx.lines[a].level.cmp(&idx.lines[b].level) };
            (if desc { order.reverse() } else { order }).then_with(|| a.cmp(&b))
        };
        matched.select_nth_unstable_by(page_end, compare);
        matched.truncate(page_end);
        matched.sort_unstable_by(compare);
    } else {
    match sort_column {
        "" => {}
        "timestamp" => sort_time(idx, Arc::make_mut(&mut matched), desc),
        "level" => Arc::make_mut(&mut matched).sort_by(|&a, &b| {
            let ord = idx.lines[a].level.cmp(&idx.lines[b].level);
            if desc {
                ord.reverse()
            } else {
                ord
            }
        }),
        col => {
            let keys: HashMap<usize, (Option<f64>, String)> = matched
                .iter()
                .take_while(|_| !crate::operations::cancelled())
                .map(|&i| {
                    let event = event_at(idx, i, codes, system, derived);
                    (
                        i,
                        (
                            event.col_num(col),
                            event.col_ref(col).unwrap_or_default().to_lowercase(),
                        ),
                    )
                })
                .collect();
            if keys.len() != matched.len() {
                return QueryResult {
                    total: 0,
                    rows: vec![],
                };
            }
            Arc::make_mut(&mut matched).sort_by(|a, b| {
                let ord = compare_sort_keys(keys[a].0, &keys[a].1, keys[b].0, &keys[b].1);
                if desc {
                    ord.reverse()
                } else {
                    ord
                }
            });
        }
    }
    }
    let rows = matched
        .iter()
        .copied()
        .skip(page_offset)
        .take(limit)
        .map(|i| {
            let mut event = event_at(idx, i, codes, system, derived);
            crate::entities::annotate(&mut event);
            event.raw.clear();
            event
        })
        .collect();
    QueryResult { total, rows }
}

fn aggregate_from_indexed_matches(
    idx: &FileIndex,
    matched: &[usize],
    group_column: &str,
    specs: &[AggSpec],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> AggResult {
    if let Some(result) = fast_count_aggregation(idx, matched, group_column, specs) { return result; }
    let mut groups: HashMap<Option<String>, Vec<Acc>> = HashMap::new();
    let mut order = Vec::new();
    let meta_only =
        is_meta_column(group_column) && specs.iter().all(|spec| is_meta_column(&spec.column));
    for &i in matched {
        if crate::operations::cancelled() {
            break;
        }
        if meta_only {
            let event = meta_event(idx, i);
            push_group(
                &mut groups,
                &mut order,
                &event,
                event.col_str(group_column),
                specs,
            );
        } else {
            let event = event_at(idx, i, codes, system, derived);
            push_group(
                &mut groups,
                &mut order,
                &event,
                event.col_str(group_column),
                specs,
            );
        }
    }
    build_agg_result(groups, order, group_column, specs)
}

fn fast_count_aggregation(idx: &FileIndex, matched: &[usize], group_column: &str, specs: &[AggSpec]) -> Option<AggResult> {
    if !specs.iter().all(|spec| spec.func == "count") { return None; }
    // Preserve the existing metadata classification of levels, including
    // unknown labels in snapshot events. Other count projections use text.
    if is_meta_column(group_column) && specs.iter().all(|spec| is_meta_column(&spec.column)) { return None; }
    let terms = idx.big_data.as_ref()?.count_terms(matched, group_column)?;
    let mut groups = HashMap::new();
    let mut order = Vec::with_capacity(terms.len());
    for (key, count) in terms {
        order.push(key.clone());
        groups.insert(key, specs.iter().map(|_| Acc::Count(count)).collect());
    }
    Some(build_agg_result(groups, order, group_column, specs))
}

pub fn explore_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> ExplorerSnapshot {
    let matched = indexed_matches_shared(idx, filters, codes, system, derived);
    let stats = stats_from(matched.iter().map(|&i| {
        (
            (idx.lines[i].ts != 0).then_some(idx.lines[i].ts),
            crate::model::class_label(idx.lines[i].level),
        )
    }));
    let count = [AggSpec {
        func: "count".into(),
        column: "*".into(),
        alias: "n".into(),
    }];
    let sources =
        aggregate_from_indexed_matches(idx, &matched, "source", &count, codes, system, derived);
    let codes_agg =
        aggregate_from_indexed_matches(idx, &matched, "code", &count, codes, system, derived);
    let query = query_from_indexed_matches(
        idx,
        matched,
        sort_column,
        sort_dir,
        offset,
        limit,
        codes,
        system,
        derived,
    );
    ExplorerSnapshot {
        query,
        stats,
        sources,
        codes: codes_agg,
    }
}

fn build_stats(buckets: Vec<(i64, i64)>, bucket_ms: i64, levels: Vec<(String, i64)>) -> Stats {
    Stats {
        buckets,
        bucket_ms,
        levels,
    }
}

const N_BUCKETS: usize = 60;

fn histogram_from_bounds(
    iter: impl Iterator<Item = Option<i64>>,
    min_ts: i64,
    max_ts: i64,
    generation: Option<u64>,
) -> Option<(Vec<(i64, i64)>, i64)> {
    if min_ts > max_ts { return Some((Vec::new(), 0)); }
    let span = max_ts.saturating_sub(min_ts).saturating_add(1);
    let bucket_ms = (span / N_BUCKETS as i64)
        .saturating_add(i64::from(span % N_BUCKETS as i64 != 0))
        .max(1);
    let count = (((span - 1) / bucket_ms + 1) as usize).min(N_BUCKETS);
    let mut counts = vec![0i64; count];
    for (position, ts) in iter.enumerate() {
        if position % 2048 == 0 && crate::operations::cancelled_for(generation) { return None; }
        let Some(t) = ts else { continue };
        let b = (t.saturating_sub(min_ts) / bucket_ms) as usize;
        counts[b.min(count - 1)] += 1;
    }
    let buckets = counts.into_iter().enumerate().map(|(b, c)| {
        (min_ts.saturating_add((b as i64).saturating_mul(bucket_ms)), c)
    }).collect();
    Some((buckets, bucket_ms))
}

fn stats_from<'a>(iter: impl Iterator<Item = (Option<i64>, &'a str)> + Clone) -> Stats {
    // Callers replay borrowed projections over existing selections. Keep only
    // bounds and level counts; do not allocate another timestamp per event.
    let generation = crate::operations::current_generation();
    let mut levels: HashMap<&'a str, i64> = HashMap::new();
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    for (position, (ts, level)) in iter.clone().enumerate() {
        if position % 2048 == 0 && crate::operations::cancelled_for(generation) {
            return build_stats(Vec::new(), 0, Vec::new());
        }
        *levels.entry(level).or_default() += 1;
        if let Some(ts) = ts {
            min_ts = min_ts.min(ts);
            max_ts = max_ts.max(ts);
        }
    }

    let Some((buckets, bucket_ms)) = histogram_from_bounds(iter.map(|(ts, _)| ts), min_ts, max_ts, generation)
        else { return build_stats(Vec::new(), 0, Vec::new()); };

    let mut levels: Vec<(String, i64)> = levels
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    levels.sort_by(|a, b| b.1.cmp(&a.1));
    build_stats(buckets, bucket_ms, levels)
}

pub fn stats(events: &[Event], filters: &[Filter]) -> Stats {
    let idx = filtered_indices(events, filters);
    stats_from(idx.iter().map(|&i| {
        let ev = &events[i];
        (ev.timestamp, ev.level.as_str())
    }))
}

pub fn stats_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Stats {
    let pfs = prepare(filters);
    let generation = crate::operations::current_generation();
    let mut timestamps = Vec::new();
    let mut levels: HashMap<Cow<'static, str>, i64> = HashMap::new();
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    scan_indexed(
        idx,
        &pfs,
        codes,
        system,
        derived,
        true,
        |_, meta, ev| match ev {
            Some(ev) => (ev.timestamp, Cow::Owned(ev.level)),
            // rótulo fixo da classe: sem String por linha
            None => (
                (meta.ts != 0).then_some(meta.ts),
                Cow::Borrowed(crate::model::class_label(meta.level)),
            ),
        },
        |(timestamp, level)| {
            *levels.entry(level).or_default() += 1;
            if let Some(timestamp) = timestamp {
                min_ts = min_ts.min(timestamp);
                max_ts = max_ts.max(timestamp);
                timestamps.push(timestamp);
            }
        },
    );
    // Preserve the exact metadata/event projection without retaining its level
    // String and Option discriminant per row or decoding/filtering a second time.
    if crate::operations::cancelled_for(generation) { return build_stats(Vec::new(), 0, Vec::new()); }
    let Some((buckets, bucket_ms)) = histogram_from_bounds(timestamps.iter().copied().map(Some), min_ts, max_ts, generation)
        else { return build_stats(Vec::new(), 0, Vec::new()); };
    let mut levels: Vec<_> = levels.into_iter().map(|(level, count)| (level.into_owned(), count)).collect();
    levels.sort_by(|a, b| b.1.cmp(&a.1));
    build_stats(buckets, bucket_ms, levels)
}

#[cfg(test)]
mod shared_selection_tests {
    use super::*;

    #[test]
    fn shared_selection_survives_sorting_owned_callers_and_cache_clear() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("shared-selection.jsonl");
        let records = [
            serde_json::json!({"timestamp":1700000003000i64,"level":"info","source":"c","code":"300","message":"z"}),
            serde_json::json!({"timestamp":1700000001000i64,"level":"info","source":"a","code":"100","message":"x"}),
            serde_json::json!({"timestamp":1700000002000i64,"level":"info","source":"b","code":"200","message":"y"}),
        ];
        std::fs::write(&path, records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let idx = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        // Numeric timestamps below 1e8 are intentionally not recognized as
        // epochs by the parser. Verify this fixture really exercises sorting.
        assert_eq!(idx.lines.iter().map(|meta| meta.ts).collect::<Vec<_>>(),
            [1700000003000i64, 1700000001000, 1700000002000]);
        let codes = CodesConfig::default();
        let filters = [Filter { column:"level".into(), op:"equals".into(), value:"Informação".into(), value2:None }];
        let selection = indexed_matches_shared(&idx, &filters, &codes, &codes, &[]);
        assert_eq!(*selection, [0, 1, 2]);
        for (column, expected) in [("", [0, 1, 2]), ("timestamp", [1, 2, 0]),
            ("level", [0, 1, 2]), ("source", [1, 2, 0]), ("code", [1, 2, 0]), ("message", [1, 2, 0])] {
            let result = query_from_indexed_matches(&idx, Arc::clone(&selection), column, "asc", 0, 10, &codes, &codes, &[]);
            assert_eq!(result.total, 3);
            assert_eq!(result.rows.iter().map(|event| event.id).collect::<Vec<_>>(), expected, "{column}");
            assert_eq!(*selection, [0, 1, 2], "sorting {column} modified a shared selection");
        }
        let mut owned = indexed_matches(&idx, &filters, &codes, &codes, &[]);
        owned.reverse();
        assert_eq!(owned, [2, 1, 0]);
        assert_eq!(*indexed_matches_shared(&idx, &filters, &codes, &codes, &[]), [0, 1, 2]);
        clear_match_cache();
        assert_eq!(*selection, [0, 1, 2]);
        let result = query_from_indexed_matches(&idx, selection, "", "asc", 1, 1, &codes, &codes, &[]);
        assert_eq!(result.rows[0].id, 1);
    }
}

#[cfg(test)]
mod histogram_tests {
    use super::*;

    // Frozen projection-retention path for parity and isolated RSS comparison.
    fn retained_stats_reference(idx: &FileIndex, filters: &[Filter], codes: &CodesConfig) -> Stats {
        let pfs = prepare(filters);
        let mut matched: Vec<(Option<i64>, Cow<'static, str>)> = Vec::new();
        scan_indexed(idx, &pfs, codes, codes, &[], true,
            |_, meta, event| match event {
                Some(event) => (event.timestamp, Cow::Owned(event.level)),
                None => ((meta.ts != 0).then_some(meta.ts), Cow::Borrowed(crate::model::class_label(meta.level))),
            }, |item| matched.push(item));
        stats_from(matched.iter().map(|(timestamp, level)| (*timestamp, level.as_ref())))
    }

    fn normalized_stats(mut stats: Stats) -> Value {
        // Equal-frequency levels have no defined order in either HashMap path.
        stats.levels.sort();
        serde_json::to_value(stats).unwrap()
    }

    #[test]
    fn indexed_histogram_compaction_preserves_metadata_and_decoded_projections() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("histogram-projections.jsonl");
        let records = [
            serde_json::json!({"timestamp":"1970-01-01T00:00:00Z","source":"a","level":"custom","message":"first"}),
            serde_json::json!({"timestamp":1700000000000i64,"source":"a","level":"info","message":"second"}),
            serde_json::json!({"source":"b","level":"warn","message":"missing date"}),
            serde_json::json!({"timestamp":1700000000060i64,"source":"b","level":"error","message":"fourth"}),
            serde_json::json!({"source":"b","level":"","message":"missing level"}),
            serde_json::json!({"timestamp":1700000000119i64,"source":"a","level":"custom","message":"sixth"}),
        ];
        std::fs::write(&path, records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let idx = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        assert_eq!(idx.lines[0].ts, 0);
        assert_eq!(crate::sources::event_at(&idx, 0, &codes, &codes, &[]).timestamp, Some(0));
        for filters in [vec![],
            vec![Filter { column:"source".into(), op:"regex".into(), value:".*".into(), value2:None }],
            vec![Filter { column:"source".into(), op:"equals".into(), value:"b".into(), value2:None }],
            vec![Filter { column:"message".into(), op:"contains".into(), value:"missing".into(), value2:None }],
            vec![Filter { column:"level".into(), op:"equals".into(), value:"absent".into(), value2:None }]] {
            let expected = normalized_stats(retained_stats_reference(&idx, &filters, &codes));
            let actual = normalized_stats(stats_indexed(&idx, &filters, &codes, &codes, &[]));
            assert_eq!(actual, expected, "{filters:?}");
        }
    }

    #[test]
    fn histogram_bucket_pass_remains_cancellable() {
        let reads = std::cell::Cell::new(0usize);
        let result = crate::operations::run(crate::operations::generation(), || {
            let generation = crate::operations::current_generation();
            histogram_from_bounds((0..100_000).map(|timestamp| {
                reads.set(reads.get() + 1);
                if reads.get() == 10 { crate::operations::cancel(); }
                Some(timestamp)
            }), 0, 99_999, generation)
        });
        assert!(result.is_err());
        assert!(reads.get() <= 2049);
    }

    #[test]
    #[ignore = "Indexed histogram retention timing/RSS; STATS_INDEXED_BENCH_EVENTS, STATS_INDEXED_REFERENCE=1"]
    fn benchmark_indexed_histogram_retention() {
        use std::io::Write;
        let count = std::env::var("STATS_INDEXED_BENCH_EVENTS").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(1_000_000).max(1);
        let reference = std::env::var("STATS_INDEXED_REFERENCE").ok().as_deref() == Some("1");
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("histogram-memory.jsonl");
        let mut output = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for i in 0..count {
            writeln!(output, "{}", serde_json::json!({"timestamp":1700000000000i64+i as i64,"level":"info","source":"worker","message":"completed"})).unwrap();
        }
        output.flush().unwrap(); drop(output);
        let idx = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let codes = CodesConfig::default();
        let expected = normalized_stats(stats_from(idx.lines.iter().map(|meta|
            ((meta.ts != 0).then_some(meta.ts), crate::model::class_label(meta.level)))));
        let mut samples = Vec::new();
        for _ in 0..7 {
            let started = std::time::Instant::now();
            let actual = if reference { retained_stats_reference(&idx, &[], &codes) }
                else { stats_indexed(&idx, &[], &codes, &codes, &[]) };
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(normalized_stats(actual), expected);
        }
        samples.sort_by(f64::total_cmp);
        println!("INDEXED_HISTOGRAM_REVIEW {}", serde_json::json!({"events":count,"reference":reference,
            "median_ms":samples[3],"p95_ms":samples[6],"samples":samples.len(),"parity":true,
            "retained_item_bytes":if reference { std::mem::size_of::<(Option<i64>, Cow<'static, str>)>() } else { std::mem::size_of::<i64>() }}));
    }

    #[test]
    fn histogram_preserves_boundaries_missing_dates_and_levels() {
        let items = [(None, "custom"), (None, "custom"), (Some(0), "Info"),
            (Some(59), "Error"), (Some(60), "Error"), (Some(119), "Info")];
        let result = stats_from(items.into_iter());
        assert_eq!(result.bucket_ms, 2);
        assert_eq!(result.buckets.len(), 60);
        for (position, &(start, count)) in result.buckets.iter().enumerate() {
            assert_eq!(start, position as i64 * 2);
            assert_eq!(count, i64::from([0, 29, 30, 59].contains(&position)));
        }
        let mut levels = result.levels;
        levels.sort();
        assert_eq!(levels, vec![("Error".into(), 2), ("Info".into(), 2), ("custom".into(), 2)]);
        let empty = stats_from(std::iter::empty());
        assert!(empty.buckets.is_empty());
        assert!(empty.levels.is_empty());
        assert_eq!(empty.bucket_ms, 0);
        let missing = stats_from([(None, "unknown")].into_iter());
        assert!(missing.buckets.is_empty());
        assert_eq!(missing.levels, vec![("unknown".into(), 1)]);
        let equal = stats_from([(Some(-42), "Info"); 3].into_iter());
        assert_eq!(equal.buckets, vec![(-42, 3)]);
        assert_eq!(equal.bucket_ms, 1);
        let extremes = stats_from([i64::MIN, 0, i64::MAX].into_iter().map(|t| (Some(t), "Info")));
        assert_eq!(extremes.buckets.len(), 60);
        assert_eq!(extremes.buckets.iter().map(|(_, n)| n).sum::<i64>(), 3);
        assert_eq!(extremes.buckets.first(), Some(&(i64::MIN, 1)));
        assert_eq!(extremes.buckets.last().unwrap().1, 2);
    }

    #[test]
    fn histogram_stops_reading_after_cancellation() {
        let reads = std::cell::Cell::new(0usize);
        let result = crate::operations::run(crate::operations::generation(), || {
            stats_from((0..100_000).map(|i| {
                reads.set(reads.get() + 1);
                if reads.get() == 100 { crate::operations::cancel(); }
                (Some(i), "Info")
            }))
        });
        assert!(result.is_err());
        assert!(reads.get() <= 2049, "read {} events after cancellation", reads.get());
    }

    #[test]
    #[ignore = "manual histogram timing/RSS benchmark; HISTOGRAM_BENCH_EVENTS controls size"]
    fn benchmark_histogram_memory() {
        let count = std::env::var("HISTOGRAM_BENCH_EVENTS").ok()
            .and_then(|n| n.parse::<usize>().ok()).unwrap_or(1_000_000).max(1);
        // A compact existing timestamp column; fixture allocation is outside timing.
        let timestamps: Vec<_> = (0..count).map(|i| 1_700_000_000_000 + i as i64).collect();
        let mut samples = Vec::new();
        let mut expected = None;
        for _ in 0..7 {
            let started = std::time::Instant::now();
            let result = stats_from(timestamps.iter().enumerate().map(|(i, &t)|
                (Some(t), if i % 10 == 0 { "Error" } else { "Info" })));
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(result.buckets.iter().map(|(_, n)| n).sum::<i64>(), count as i64);
            assert_eq!(result.levels.iter().map(|(_, n)| n).sum::<i64>(), count as i64);
            let result = serde_json::to_value(result).unwrap();
            if let Some(expected) = &expected { assert_eq!(&result, expected); } else { expected = Some(result); }
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        println!("HISTOGRAM_BENCH {}", serde_json::json!({
            "events": count, "samplesMs": samples, "medianMs": sorted[3], "p95Ms": sorted[6],
            "fixtureBytes": timestamps.capacity() * std::mem::size_of::<i64>()
        }));
    }
}
