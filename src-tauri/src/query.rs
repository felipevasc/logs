use crate::model::{label_class, Event, LineMeta, LV_OTHER};
use crate::sources::{event_at, line_bytes, CompiledDerived, FileIndex};
use crate::model::CodesConfig;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use rayon::prelude::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Filter {
    pub column: String,
    pub op: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub value2: Option<String>,
}

/// Filtro com regex pré-compilada (op "regex"). Regex inválida cai para "contém".
/// `needle_lower`, `num` e `num2` são pré-computados uma vez por consulta
/// em vez de por evento/linha.
pub struct PreparedFilter {
    pub f: Filter,
    regex: Option<regex::Regex>,
    bregex: Option<regex::bytes::Regex>,
    needle_lower: String,
    num: Option<f64>,
    num2: Option<f64>,
}

pub fn prepare(filters: &[Filter]) -> Vec<PreparedFilter> {
    filters
        .iter()
        .map(|f| {
            let is_re = f.op == "regex";
            PreparedFilter {
                regex: is_re.then(|| regex::Regex::new(&f.value).ok()).flatten(),
                bregex: is_re.then(|| regex::bytes::Regex::new(&f.value).ok()).flatten(),
                needle_lower: f.value.to_lowercase(),
                num: value_as_num(&f.column, &f.value),
                num2: value_as_num(&f.column, f.value2.as_deref().unwrap_or("")),
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

#[derive(Serialize)]
pub struct AggResult {
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
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

#[derive(Clone, Debug, Deserialize)]
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

fn matches(ev: &Event, pf: &PreparedFilter) -> bool {
    let f = &pf.f;
    let col = f.column.as_str();
    let needle = pf.needle_lower.as_str();

    if f.op == "regex" {
        let hay: Cow<'_, str> = if col == "_all" {
            Cow::Owned(format!("{}\n{}", ev.message, ev.raw))
        } else {
            ev.col_ref(col).unwrap_or(Cow::Borrowed(""))
        };
        return match &pf.regex {
            Some(re) => re.is_match(&hay),
            // regex inválida → contém (case-insensitive ASCII, como ci_contains_bytes)
            None => ci_contains_bytes(hay.as_bytes(), needle.as_bytes()),
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
        _ => true,
    }
}

/// Verifica um único filtro contra um evento (condições de campos derivados).
pub fn matches_filter(ev: &Event, filter: &Filter) -> bool {
    let prepared = prepare(std::slice::from_ref(filter));
    matches(ev, &prepared[0])
}

pub fn filtered_indices(events: &[Event], filters: &[Filter]) -> Vec<usize> {
    let pfs = prepare(filters);
    events
        .iter()
        .enumerate()
        .filter(|(_, ev)| pfs.iter().all(|pf| matches(ev, pf)))
        .map(|(i, _)| i)
        .collect()
}

pub fn sort_indices(events: &[Event], indices: &mut [usize], column: &str, desc: bool) {
    // Pré-computa a chave (número opcional + texto em minúsculas) uma vez por
    // linha; o comparador anterior realocava as strings a cada comparação.
    let mut keyed: Vec<((Option<f64>, String), usize)> = indices
        .iter()
        .map(|&i| {
            let ev = &events[i];
            let num = ev.col_num(column);
            let text = ev.col_ref(column).unwrap_or_default().to_lowercase();
            ((num, text), i)
        })
        .collect();
    keyed.sort_by(|((na, sa), _), ((nb, sb), _)| {
        let ord = match (na, nb) {
            (Some(x), Some(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
            _ => sa.cmp(sb),
        };
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
    let mut groups: HashMap<String, Vec<Acc>> = HashMap::new();
    let mut order = Vec::new();
    for &i in matched {
        let event = &events[i];
        push_group(&mut groups, &mut order, event, event.col_str(group_column), specs);
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
        (event.timestamp.unwrap_or(0), event.level.as_str())
    }));
    let count = [AggSpec { func: "count".into(), column: "*".into(), alias: "n".into() }];
    let sources = aggregate_from_memory_matches(events, &matched, "source", &count);
    let codes = aggregate_from_memory_matches(events, &matched, "code", &count);
    let query = query_from_memory_matches(events, matched, sort_column, sort_dir, offset, limit);
    ExplorerSnapshot { query, stats, sources, codes }
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

fn ci_contains_bytes(hay: &[u8], needle_lower: &[u8]) -> bool {
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

fn meta_check(pf: &PreparedFilter, meta: &LineMeta, line: &[u8]) -> Tri {
    let f = &pf.f;
    let op = f.op.as_str();
    let v = f.value.trim();
    // equivalente a v.to_lowercase() (trim e lowercase comutam para whitespace)
    let needle = pf.needle_lower.trim();
    match f.column.as_str() {
        "timestamp" => match op {
            "gt" | "gte" | "lt" | "lte" | "between" => {
                if meta.ts == 0 {
                    return Tri::Fail;
                }
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
            "empty" => {
                if meta.ts == 0 {
                    Tri::Pass
                } else {
                    Tri::Fail
                }
            }
            "not_empty" => {
                if meta.ts != 0 {
                    Tri::Pass
                } else {
                    Tri::Fail
                }
            }
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
            "regex" => match &pf.bregex {
                Some(re) => {
                    if re.is_match(line) {
                        Tri::NeedEvent
                    } else {
                        Tri::Fail
                    }
                }
                None => Tri::NeedEvent,
            },
            _ => Tri::NeedEvent,
        },
        "_all" => match op {
            "regex" => match &pf.bregex {
                Some(re) => {
                    if re.is_match(line) {
                        Tri::Pass
                    } else {
                        Tri::Fail
                    }
                }
                None => Tri::NeedEvent,
            },
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

/// Filtra as linhas do índice; materializa apenas quando necessário.
pub fn indexed_matches(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> Vec<usize> {
    let pfs = prepare(filters);
    let mut matched = Vec::new();
    for (i, meta) in idx.lines.iter().enumerate() {
        let line = line_bytes(idx, i);
        let mut need = false;
        let mut ok = true;
        for pf in &pfs {
            match meta_check(pf, meta, line) {
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
            let ev = event_at(idx, i, codes, system, derived);
            if pfs.iter().all(|pf| matches(&ev, pf)) {
                matched.push(i);
            }
        } else {
            matched.push(i);
        }
    }
    matched
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
    let mut matched = indexed_matches(idx, filters, codes, system, derived);
    let desc = sort_dir == "desc";
    match sort_column {
        "" => {}
        "timestamp" => matched.sort_by(|&a, &b| {
            let ord = idx.lines[a].ts.cmp(&idx.lines[b].ts);
            if desc { ord.reverse() } else { ord }
        }),
        "level" => matched.sort_by(|&a, &b| {
            let ord = idx.lines[a].level.cmp(&idx.lines[b].level);
            if desc { ord.reverse() } else { ord }
        }),
        "code" => matched.sort_by(|&a, &b| {
            let ord = idx.lines[a]
                .code(line_bytes(idx, a))
                .cmp(idx.lines[b].code(line_bytes(idx, b)));
            if desc { ord.reverse() } else { ord }
        }),
        col => {
            // pré-computa a chave de ordenação materializando uma vez por linha
            let keys: HashMap<usize, String> = matched
                .iter()
                .map(|&i| (i, event_at(idx, i, codes, system, derived).col_str(col).unwrap_or_default()))
                .collect();
            matched.sort_by(|a, b| {
                let ord = keys[a].to_lowercase().cmp(&keys[b].to_lowercase());
                if desc { ord.reverse() } else { ord }
            });
        }
    }
    let total = matched.len();
    let rows = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut e = event_at(idx, i, codes, system, derived);
            e.raw = String::new();
            e
        })
        .collect();
    QueryResult { total, rows }
}

// ==========================================================================
// Agregações
// ==========================================================================

enum Acc {
    Count(u64),
    CountDistinct(HashSet<String>),
    Sum(f64),
    Avg(f64, u64),
    Min(Option<f64>),
    Max(Option<f64>),
    StrAgg(Vec<String>),
}

impl Acc {
    fn new(func: &str) -> Self {
        match func {
            "count" => Acc::Count(0),
            "count_distinct" => Acc::CountDistinct(HashSet::new()),
            "sum" => Acc::Sum(0.0),
            "avg" => Acc::Avg(0.0, 0),
            "min" => Acc::Min(None),
            "max" => Acc::Max(None),
            _ => Acc::StrAgg(Vec::new()),
        }
    }

    fn push(&mut self, ev: &Event, column: &str) {
        match self {
            Acc::Count(n) => *n += 1,
            Acc::CountDistinct(set) => {
                if let Some(s) = ev.col_str(column) {
                    if set.len() < 100_000 {
                        set.insert(s);
                    }
                }
            }
            Acc::Sum(n) => {
                if let Some(v) = ev.col_num(column) {
                    *n += v;
                }
            }
            Acc::Avg(sum, n) => {
                if let Some(v) = ev.col_num(column) {
                    *sum += v;
                    *n += 1;
                }
            }
            Acc::Min(cur) => {
                if let Some(v) = ev.col_num(column) {
                    *cur = Some(cur.map(|c: f64| c.min(v)).unwrap_or(v));
                }
            }
            Acc::Max(cur) => {
                if let Some(v) = ev.col_num(column) {
                    *cur = Some(cur.map(|c: f64| c.max(v)).unwrap_or(v));
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

    fn finish(&self, ev_col: &str) -> Value {
        match self {
            Acc::Count(n) => Value::from(*n),
            Acc::CountDistinct(set) => Value::from(set.len() as u64),
            Acc::Sum(n) => round2(*n).into(),
            Acc::Avg(sum, n) => {
                if *n == 0 {
                    Value::Null
                } else {
                    round2(*sum / *n as f64).into()
                }
            }
            Acc::Min(cur) | Acc::Max(cur) => match cur {
                Some(v) if ev_col == "timestamp" => {
                    Value::from(crate::model::ts_to_iso(*v as i64))
                }
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
    columns
        .par_iter()
        .map(|col| {
            let fs: Vec<Filter> = filters.iter().filter(|f| f.column != *col).cloned().collect();
            let agg = aggregate(
                events,
                &fs,
                col,
                &[AggSpec { func: "count".into(), column: "*".into(), alias: "n".into() }],
            );
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
    columns
        .par_iter()
        .map(|col| {
            let fs: Vec<Filter> = filters.iter().filter(|f| f.column != *col).cloned().collect();
            let agg = aggregate_indexed(
                idx,
                &fs,
                col,
                &[AggSpec { func: "count".into(), column: "*".into(), alias: "n".into() }],
                codes,
                system,
                derived,
            );
            (col.clone(), agg)
        })
        .collect()
}

fn build_agg_result(
    groups: HashMap<String, Vec<Acc>>,
    mut order: Vec<String>,
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    order.sort_by(|a, b| match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
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

    let rows = order
        .into_iter()
        .map(|key| {
            let accs = &groups[&key];
            let mut row = Map::new();
            row.insert(group_column.to_string(), Value::from(key));
            for (acc, spec) in accs.iter().zip(specs.iter()) {
                row.insert(alias(spec), acc.finish(&spec.column));
            }
            row
        })
        .collect();

    AggResult { columns, rows }
}

pub fn aggregate(
    events: &[Event],
    filters: &[Filter],
    group_column: &str,
    specs: &[AggSpec],
) -> AggResult {
    let idx = filtered_indices(events, filters);
    let mut groups: HashMap<String, Vec<Acc>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for i in idx {
        let ev = &events[i];
        push_group(&mut groups, &mut order, ev, ev.col_str(group_column), specs);
    }
    build_agg_result(groups, order, group_column, specs)
}

fn push_group(
    groups: &mut HashMap<String, Vec<Acc>>,
    order: &mut Vec<String>,
    ev: &Event,
    key: Option<String>,
    specs: &[AggSpec],
) {
    let key = key.unwrap_or_default();
    let key = if key.is_empty() { "(vazio)".to_string() } else { key };
    // Caminho quente é o grupo já existente: get_mut evita clonar a chave
    // a cada linha; só clona ao criar um grupo novo.
    if let Some(accs) = groups.get_mut(key.as_str()) {
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
    matches!(col, "*" | "timestamp" | "level" | "code")
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
    let matched = indexed_matches(idx, filters, codes, system, derived);
    let mut groups: HashMap<String, Vec<Acc>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    // Caminho rápido: grupo e agregações só sobre colunas de metadados
    // (timestamp, level, code) — zero parse por linha.
    let meta_only = is_meta_column(group_column)
        && specs.iter().all(|s| is_meta_column(&s.column));

    for i in matched {
        if meta_only {
            let ev = meta_event(idx, i);
            push_group(&mut groups, &mut order, &ev, ev.col_str(group_column), specs);
        } else {
            let ev = event_at(idx, i, codes, system, derived);
            push_group(&mut groups, &mut order, &ev, ev.col_str(group_column), specs);
        }
    }
    build_agg_result(groups, order, group_column, specs)
}

// ==========================================================================
// Estatísticas (histograma + níveis)
// ==========================================================================

fn query_from_indexed_matches(
    idx: &FileIndex,
    mut matched: Vec<usize>,
    sort_column: &str,
    sort_dir: &str,
    offset: usize,
    limit: usize,
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
) -> QueryResult {
    let desc = sort_dir == "desc";
    match sort_column {
        "" => {}
        "timestamp" => matched.sort_by(|&a, &b| {
            let ord = idx.lines[a].ts.cmp(&idx.lines[b].ts);
            if desc { ord.reverse() } else { ord }
        }),
        "level" => matched.sort_by(|&a, &b| {
            let ord = idx.lines[a].level.cmp(&idx.lines[b].level);
            if desc { ord.reverse() } else { ord }
        }),
        "code" => matched.sort_by(|&a, &b| {
            let ord = idx.lines[a]
                .code(line_bytes(idx, a))
                .cmp(idx.lines[b].code(line_bytes(idx, b)));
            if desc { ord.reverse() } else { ord }
        }),
        col => {
            let keys: HashMap<usize, String> = matched
                .iter()
                .map(|&i| (i, event_at(idx, i, codes, system, derived).col_str(col).unwrap_or_default()))
                .collect();
            matched.sort_by(|a, b| {
                let ord = keys[a].to_lowercase().cmp(&keys[b].to_lowercase());
                if desc { ord.reverse() } else { ord }
            });
        }
    }
    let total = matched.len();
    let rows = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut event = event_at(idx, i, codes, system, derived);
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
    let mut groups: HashMap<String, Vec<Acc>> = HashMap::new();
    let mut order = Vec::new();
    let meta_only = is_meta_column(group_column)
        && specs.iter().all(|spec| is_meta_column(&spec.column));
    for &i in matched {
        if meta_only {
            let event = meta_event(idx, i);
            push_group(&mut groups, &mut order, &event, event.col_str(group_column), specs);
        } else {
            let event = event_at(idx, i, codes, system, derived);
            push_group(&mut groups, &mut order, &event, event.col_str(group_column), specs);
        }
    }
    build_agg_result(groups, order, group_column, specs)
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
    let matched = indexed_matches(idx, filters, codes, system, derived);
    let stats = stats_from(matched.iter().map(|&i| {
        (idx.lines[i].ts, crate::model::class_label(idx.lines[i].level))
    }));
    let count = [AggSpec { func: "count".into(), column: "*".into(), alias: "n".into() }];
    let sources = aggregate_from_indexed_matches(idx, &matched, "source", &count, codes, system, derived);
    let codes_agg = aggregate_from_indexed_matches(idx, &matched, "code", &count, codes, system, derived);
    let query = query_from_indexed_matches(idx, matched, sort_column, sort_dir, offset, limit, codes, system, derived);
    ExplorerSnapshot { query, stats, sources, codes: codes_agg }
}

fn build_stats(buckets: Vec<(i64, i64)>, bucket_ms: i64, levels: Vec<(String, i64)>) -> Stats {
    Stats {
        buckets,
        bucket_ms,
        levels,
    }
}

const N_BUCKETS: usize = 60;

fn stats_from<'a>(
    iter: impl Iterator<Item = (i64, &'a str)>,
) -> Stats {
    // Chaveia por &str emprestada (zero alocação por linha); converte para
    // String apenas no Vec final.
    let mut levels: HashMap<&'a str, i64> = HashMap::new();
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    let mut items = Vec::new();
    for (ts, level) in iter {
        *levels.entry(level).or_default() += 1;
        if ts != 0 {
            min_ts = min_ts.min(ts);
            max_ts = max_ts.max(ts);
        }
        items.push(ts);
    }

    let mut buckets = Vec::new();
    let mut bucket_ms = 0i64;
    if min_ts <= max_ts {
        let span = (max_ts - min_ts).max(1);
        bucket_ms = (span / N_BUCKETS as i64).max(1);
        let mut counts = vec![0i64; N_BUCKETS + 1];
        for t in items.into_iter().filter(|&t| t != 0) {
            let b = ((t - min_ts) / bucket_ms) as usize;
            counts[b.min(N_BUCKETS)] += 1;
        }
        buckets = counts
            .into_iter()
            .enumerate()
            .map(|(b, c)| (min_ts + b as i64 * bucket_ms, c))
            .collect();
    }

    let mut levels: Vec<(String, i64)> =
        levels.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    levels.sort_by(|a, b| b.1.cmp(&a.1));
    build_stats(buckets, bucket_ms, levels)
}

pub fn stats(events: &[Event], filters: &[Filter]) -> Stats {
    let idx = filtered_indices(events, filters);
    stats_from(idx.into_iter().map(|i| {
        let ev = &events[i];
        (ev.timestamp.unwrap_or(0), ev.level.as_str())
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
    let mut matched: Vec<(i64, Cow<'static, str>)> = Vec::new();
    for (i, meta) in idx.lines.iter().enumerate() {
        let line = line_bytes(idx, i);
        let mut need = false;
        let mut ok = true;
        for pf in &pfs {
            match meta_check(pf, meta, line) {
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
            let ev = event_at(idx, i, codes, system, derived);
            if pfs.iter().all(|pf| matches(&ev, pf)) {
                matched.push((ev.timestamp.unwrap_or(0), Cow::Owned(ev.level)));
            }
        } else {
            // rótulo fixo da classe: sem String por linha
            matched.push((meta.ts, Cow::Borrowed(crate::model::class_label(meta.level))));
        }
    }
    stats_from(matched.iter().map(|(t, l)| (*t, l.as_ref())))
}
