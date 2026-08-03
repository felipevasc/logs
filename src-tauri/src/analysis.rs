//! Motor analítico: perfil de campos, séries para gráficos e pivô OLAP.
use crate::model::{CodesConfig, Event};
use rayon::prelude::*;
use crate::query::{AggSpec, Filter};
use crate::sources::{CompiledDerived, FileIndex};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ------------------------------------------------------------------ unidades

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
pub enum UnitKind {
    Number,
    Bytes,   // KB/MB/GB (x1024)
    Bits,    // kbps/mbps (x1000, em bits)
    DurationMs,
    None,
}

impl Default for UnitKind {
    fn default() -> Self {
        UnitKind::None
    }
}

/// Interpreta "10MB", "5.2 Gb", "100 mbps", "820", "12ms", "3s" como número.
/// Devolve (valor_normalizado, unidade).
pub fn parse_num_unit(s: &str) -> Option<(f64, UnitKind)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_lowercase();
    let (num_part, unit_part) = split_num_unit(&lower);
    let n: f64 = num_part.replace(',', ".").parse().ok()?;
    Some(match unit_part {
        "" => (n, UnitKind::Number),
        "b" | "byte" | "bytes" => (n, UnitKind::Bytes),
        "kb" => (n * 1024.0, UnitKind::Bytes),
        "mb" => (n * 1024.0 * 1024.0, UnitKind::Bytes),
        "gb" => (n * 1024.0 * 1024.0 * 1024.0, UnitKind::Bytes),
        "tb" => (n * 1024.0 * 1024.0 * 1024.0 * 1024.0, UnitKind::Bytes),
        "bps" => (n, UnitKind::Bits),
        "kbps" => (n * 1e3, UnitKind::Bits),
        "mbps" => (n * 1e6, UnitKind::Bits),
        "gbps" => (n * 1e9, UnitKind::Bits),
        "ms" => (n, UnitKind::DurationMs),
        "s" => (n * 1000.0, UnitKind::DurationMs),
        "min" => (n * 60_000.0, UnitKind::DurationMs),
        "h" => (n * 3_600_000.0, UnitKind::DurationMs),
        _ => return None,
    })
}

fn split_num_unit(s: &str) -> (&str, &str) {
    let idx = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',' || c == '-'))
        .unwrap_or(s.len());
    (s[..idx].trim(), s[idx..].trim())
}

// ------------------------------------------------------------------ perfil

#[derive(Serialize)]
pub struct FieldProfile {
    name: String,
    kind: String, // time | number | bytes | bits | duration | category | text
    cardinality: usize,
    /// quantidade de valores vazios na amostra (vira a opção "(vazio)" na árvore)
    empty: usize,
    numeric_ratio: f64,
    min: Option<f64>,
    max: Option<f64>,
    top: Vec<(String, i64)>,
}

const PROFILE_SAMPLE: usize = 3_000;

fn is_ip(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u8>().is_ok())
        || (s.contains(':') && s.len() >= 3 && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':'))
}

fn is_bool(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "true" | "false" | "0" | "1" | "sim" | "não" | "nao" | "yes" | "no"
    )
}

fn profile_column(values: Vec<String>) -> FieldProfileBuilder {
    let mut p = FieldProfileBuilder::default();
    for v in values.iter().take(PROFILE_SAMPLE) {
        let v = v.trim();
        if v.is_empty() {
            p.empty += 1;
            continue;
        }
        p.total += 1;
        *p.counts.entry(v.to_string()).or_default() += 1;
        if is_bool(v) {
            p.bools += 1;
        }
        if is_ip(v) {
            p.ips += 1;
        }
        let num_str = v.strip_suffix('%').unwrap_or(v);
        if let Some((n, unit)) = parse_num_unit(num_str) {
            p.numeric += 1;
            if v.ends_with('%') {
                p.percents += 1;
            }
            *p.unit_votes.entry(unit as u8).or_insert(0) += 1;
            p.min = Some(p.min.map(|m: f64| m.min(n)).unwrap_or(n));
            p.max = Some(p.max.map(|m: f64| m.max(n)).unwrap_or(n));
        }
    }
    p
}

#[derive(Default)]
struct FieldProfileBuilder {
    total: usize,
    empty: usize,
    numeric: usize,
    bools: usize,
    ips: usize,
    percents: usize,
    counts: HashMap<String, i64>,
    unit_votes: HashMap<u8, usize>,
    min: Option<f64>,
    max: Option<f64>,
}

impl FieldProfileBuilder {
    fn finish(self, name: &str) -> FieldProfile {
        let ratio = if self.total == 0 {
            0.0
        } else {
            self.numeric as f64 / self.total as f64
        };
        let cardinality = self.counts.len();
        let unit = self
            .unit_votes
            .iter()
            .max_by_key(|(_, c)| *c)
            .map(|(u, _)| *u)
            .unwrap_or(0);
        let kind = if name == "timestamp" {
            "time"
        } else if self.total > 0 && self.bools as f64 / self.total.max(1) as f64 >= 0.9 {
            "bool"
        } else if self.total > 0 && self.ips as f64 / self.total.max(1) as f64 >= 0.85 {
            "ip"
        } else if ratio >= 0.85 && self.percents as f64 / self.numeric.max(1) as f64 >= 0.85 {
            "percent"
        } else if ratio >= 0.85 {
            match unit {
                u if u == UnitKind::Bytes as u8 => "bytes",
                u if u == UnitKind::Bits as u8 => "bits",
                u if u == UnitKind::DurationMs as u8 => "duration",
                _ => "number",
            }
        } else if cardinality <= 25 {
            "category"
        } else if self.total > 0 && cardinality as f64 / self.total as f64 >= 0.95 {
            "id"
        } else {
            "text"
        };
        let mut top: Vec<(String, i64)> = self.counts.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(10);
        FieldProfile {
            name: name.to_string(),
            kind: kind.to_string(),
            cardinality,
            empty: self.empty,
            numeric_ratio: (ratio * 100.0).round() / 100.0,
            min: self.min,
            max: self.max,
            top,
        }
    }
}

fn collect_values(events: &[Event], columns: &[String]) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for col in columns {
        if col != "raw" {
            map.insert(col.clone(), Vec::new());
        }
    }
    // Amostra: primeiros PROFILE_SAMPLE eventos; extrai todas as colunas de uma vez.
    for ev in events.iter().take(PROFILE_SAMPLE) {
        for (col, vals) in map.iter_mut() {
            if let Some(s) = ev.col_str(col) {
                if !s.is_empty() {
                    vals.push(s);
                }
            }
        }
    }
    map
}

fn profiles_from_values(
    values: HashMap<String, Vec<String>>,
    columns: &[String],
) -> Vec<FieldProfile> {
    columns
        .iter()
        .filter(|c| *c != "raw")
        .map(|c| {
            let vals = values.get(c).cloned().unwrap_or_default();
            profile_column(vals).finish(c)
        })
        .collect()
}

pub fn profile_fields(events: &[Event], columns: &[String]) -> Vec<FieldProfile> {
    profiles_from_values(collect_values(events, columns), columns)
}

// ------------------------------------------------------------------ séries

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct SeriesSpec {
    pub chart: String, // "time" | "terms"
    pub metric: String, // count | sum | avg | min | max | distinct
    #[serde(default)]
    pub field: Option<String>, // campo numérico da métrica (None = count)
    #[serde(default)]
    pub interval_ms: Option<i64>, // None = auto
    #[serde(default)]
    pub split: Option<String>, // campo para quebrar em séries
    #[serde(default)]
    pub limit: Option<usize>, // top N (terms/split)
    #[serde(default)]
    pub unit: Option<String>, // "auto" → normaliza por unidade dominante
}

#[derive(Serialize)]
pub struct SeriesResult {
    kind: String,
    unit: String, // number | bytes | bits | duration
    interval_ms: i64,
    /// "time": epoch ms do bucket; "terms": rótulo
    x: Vec<Value>,
    series: Vec<SeriesData>,
}

#[derive(Serialize)]
pub struct SeriesData {
    name: String,
    points: Vec<f64>,
}

struct MetricAcc {
    metric: String,
    sum: f64,
    n: u64,
    min: Option<f64>,
    max: Option<f64>,
    distinct: std::collections::HashSet<String>,
}

impl MetricAcc {
    fn new(metric: &str) -> Self {
        MetricAcc {
            metric: metric.to_string(),
            sum: 0.0,
            n: 0,
            min: None,
            max: None,
            distinct: Default::default(),
        }
    }
    fn push(&mut self, ev: &Event, field: Option<&str>) {
        match self.metric.as_str() {
            "count" => self.n += 1,
            "distinct" => {
                if let Some(f) = field {
                    if let Some(v) = ev.col_str(f) {
                        if !v.is_empty() {
                            self.distinct.insert(v);
                        }
                    }
                }
            }
            _ => {
                if let Some(f) = field {
                    if let Some((n, _)) = ev
                        .col_str(f)
                        .and_then(|s| parse_num_unit(&s))
                    {
                        self.sum += n;
                        self.n += 1;
                        self.min = Some(self.min.map(|m: f64| m.min(n)).unwrap_or(n));
                        self.max = Some(self.max.map(|m: f64| m.max(n)).unwrap_or(n));
                    }
                }
            }
        }
    }
    fn value(&self) -> f64 {
        match self.metric.as_str() {
            "count" => self.n as f64,
            "distinct" => self.distinct.len() as f64,
            "sum" => self.sum,
            "avg" => {
                if self.n == 0 {
                    0.0
                } else {
                    self.sum / self.n as f64
                }
            }
            "min" => self.min.unwrap_or(0.0),
            "max" => self.max.unwrap_or(0.0),
            _ => 0.0,
        }
    }
}

fn dominant_unit(events: &[Event], field: &str, sample: usize) -> UnitKind {
    let mut votes: HashMap<u8, usize> = HashMap::new();
    for ev in events.iter().take(sample) {
        if let Some((_, u)) = ev.col_str(field).and_then(|s| parse_num_unit(&s)) {
            *votes.entry(u as u8).or_default() += 1;
        }
    }
    votes
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(u, _)| match u {
            u if u == UnitKind::Bytes as u8 => UnitKind::Bytes,
            u if u == UnitKind::Bits as u8 => UnitKind::Bits,
            u if u == UnitKind::DurationMs as u8 => UnitKind::DurationMs,
            _ => UnitKind::Number,
        })
        .unwrap_or(UnitKind::Number)
}

fn unit_name(u: UnitKind) -> String {
    match u {
        UnitKind::Bytes => "bytes",
        UnitKind::Bits => "bits",
        UnitKind::DurationMs => "duration",
        _ => "number",
    }
    .to_string()
}

pub fn compute_series(events: &[Event], spec: &SeriesSpec) -> SeriesResult {
    let limit = spec.limit.unwrap_or(10);
    let field = spec.field.as_deref();
    let unit = match (spec.unit.as_deref(), field) {
        (Some(u), _) if u != "auto" => u.to_string(),
        (Some(_), None) | (None, None) => "number".to_string(),
        (_, Some(f)) => unit_name(dominant_unit(events, f, 500)),
    };

    // splits: top N valores do campo de split
    let splits: Vec<String> = match &spec.split {
        Some(col) => {
            let mut counts: HashMap<String, i64> = HashMap::new();
            for ev in events {
                if let Some(v) = ev.col_str(col) {
                    if !v.is_empty() {
                        *counts.entry(v).or_default() += 1;
                    }
                }
            }
            let mut v: Vec<_> = counts.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            v.into_iter().take(6).map(|(k, _)| k).collect()
        }
        None => vec![],
    };
    let split_names: Vec<String> = if splits.is_empty() {
        vec![spec
            .field
            .clone()
            .unwrap_or_else(|| "eventos".into())]
    } else {
        splits.clone()
    };

    if spec.chart == "terms" {
        // ranking de valores de `field_key` (ou da métrica se count)
        let key_field = spec
            .field
            .clone()
            .unwrap_or_else(|| "level".into());
        let metric_field = if spec.metric == "count" || spec.metric == "distinct" {
            spec.field.as_deref()
        } else {
            field
        };
        let mut accs: HashMap<String, MetricAcc> = HashMap::new();
        for ev in events {
            let key = ev
                .col_str(&key_field)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "(vazio)".into());
            accs.entry(key)
                .or_insert_with(|| MetricAcc::new(&spec.metric))
                .push(ev, metric_field.map(|_| field.unwrap_or(&key_field)));
        }
        let mut items: Vec<(String, f64)> = accs
            .iter()
            .map(|(k, a)| (k.clone(), a.value()))
            .collect();
        items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        items.truncate(limit);
        return SeriesResult {
            kind: "terms".into(),
            unit,
            interval_ms: 0,
            x: items.iter().map(|(k, _)| Value::from(k.clone())).collect(),
            series: vec![SeriesData {
                name: split_names[0].clone(),
                points: items.into_iter().map(|(_, v)| v).collect(),
            }],
        };
    }

    // série temporal
    let tss: Vec<i64> = events.iter().filter_map(|ev| ev.timestamp).collect();
    if tss.is_empty() {
        return SeriesResult {
            kind: "time".into(),
            unit,
            interval_ms: 0,
            x: vec![],
            series: vec![],
        };
    }
    let (tmin, tmax) = (*tss.iter().min().unwrap(), *tss.iter().max().unwrap());
    let interval = spec.interval_ms.unwrap_or_else(|| {
        let span = (tmax - tmin).max(1);
        // ~60 buckets; escolhe intervalo "redondo"
        let target = span / 60;
        for nice in [
            1_000i64, 5_000, 15_000, 60_000, 300_000, 900_000, 1_800_000, 3_600_000,
            21_600_000, 43_200_000, 86_400_000, 604_800_000, 2_592_000_000,
        ] {
            if target <= nice {
                return nice;
            }
        }
        2_592_000_000
    });
    let n_buckets = ((tmax - tmin) / interval + 2) as usize;

    let mut accs: Vec<HashMap<String, MetricAcc>> = (0..n_buckets)
        .map(|_| HashMap::new())
        .collect();
    for ev in events {
        let Some(t) = ev.timestamp else { continue };
        let b = ((t - tmin) / interval) as usize;
        if b >= n_buckets {
            continue;
        }
        let name = match &spec.split {
            Some(col) => {
                let v = ev.col_str(col).unwrap_or_default();
                if splits.contains(&v) {
                    v
                } else {
                    continue;
                }
            }
            None => split_names[0].clone(),
        };
        accs[b]
            .entry(name)
            .or_insert_with(|| MetricAcc::new(&spec.metric))
            .push(ev, field);
    }

    SeriesResult {
        kind: "time".into(),
        unit,
        interval_ms: interval,
        x: (0..n_buckets)
            .map(|b| Value::from(tmin + b as i64 * interval))
            .collect(),
        series: split_names
            .iter()
            .map(|name| SeriesData {
                name: name.clone(),
                points: accs
                    .iter()
                    .map(|m| m.get(name).map(|a| a.value()).unwrap_or(0.0))
                    .collect(),
            })
            .collect(),
    }
}

// ------------------------------------------------------------------ pivô OLAP

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
pub struct PivotSpec {
    pub rows: Vec<String>,
    pub cols: Vec<String>,
    pub values: Vec<AggSpec>,
    #[serde(default = "default_row_limit")]
    pub limit_rows: usize,
}

fn default_row_limit() -> usize {
    2_000
}

#[derive(Serialize)]
pub struct PivotResult {
    value_names: Vec<String>,
    col_keys: Vec<String>,
    /// caminho de cada linha (nível = tamanho do caminho)
    row_paths: Vec<Vec<String>>,
    /// cells[i][j][v] = valor da linha i, coluna j, medida v
    cells: Vec<Vec<Vec<Value>>>,
    /// totais[i][v]
    totals: Vec<Vec<Value>>,
    truncated: bool,
}

pub fn pivot(events: &[Event], spec: &PivotSpec) -> PivotResult {
    let value_names: Vec<String> = spec
        .values
        .iter()
        .map(|a| {
            if a.alias.is_empty() {
                format!("{}({})", a.func, a.column)
            } else {
                a.alias.clone()
            }
        })
        .collect();

    enum Acc {
        Count(u64),
        CountDistinct(std::collections::HashSet<String>),
        Num(f64, u64, f64, f64), // sum, n, min, max
        Str(Vec<String>),
    }
    impl Acc {
        fn new(func: &str) -> Self {
            match func {
                "count" => Acc::Count(0),
                "count_distinct" => Acc::CountDistinct(Default::default()),
                "string_agg" => Acc::Str(vec![]),
                _ => Acc::Num(0.0, 0, f64::MAX, f64::MIN),
            }
        }
        fn push(&mut self, v: Option<f64>, s: Option<String>) {
            match self {
                Acc::Count(n) => *n += 1,
                Acc::CountDistinct(set) => {
                    if let Some(s) = s {
                        if set.len() < 50_000 {
                            set.insert(s);
                        }
                    }
                }
                Acc::Num(sum, n, min, max) => {
                    if let Some(v) = v {
                        *sum += v;
                        *n += 1;
                        *min = min.min(v);
                        *max = max.max(v);
                    }
                }
                Acc::Str(items) => {
                    if items.len() < 50 {
                        if let Some(s) = s {
                            if !s.is_empty() {
                                items.push(s);
                            }
                        }
                    }
                }
            }
        }
        fn finish(&self, func: &str) -> Value {
            match (self, func) {
                (Acc::Count(n), _) => Value::from(*n),
                (Acc::CountDistinct(s), _) => Value::from(s.len() as u64),
                (Acc::Num(sum, _, _, _), "sum") => Value::from((sum * 100.0).round() / 100.0),
                (Acc::Num(sum, n, _, _), "avg") => {
                    if *n == 0 {
                        Value::Null
                    } else {
                        Value::from((sum / *n as f64 * 100.0).round() / 100.0)
                    }
                }
                (Acc::Num(_, _, min, _), "min") => {
                    if *min == f64::MAX {
                        Value::Null
                    } else {
                        Value::from(*min)
                    }
                }
                (Acc::Num(_, _, _, max), "max") => {
                    if *max == f64::MIN {
                        Value::Null
                    } else {
                        Value::from(*max)
                    }
                }
                (Acc::Str(items), _) => Value::from(items.join(", ")),
                _ => Value::Null,
            }
        }
    }

    let mut col_keys: Vec<String> = Vec::new();
    let mut col_index: HashMap<String, usize> = HashMap::new();
    // key: (path_string, col_idx) → acumuladores
    let mut cells: HashMap<(String, usize), Vec<Acc>> = HashMap::new();
    let mut totals: HashMap<usize, Vec<Acc>> = HashMap::new();
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut path_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let n_vals = spec.values.len().max(1);

    let acc_cell = |cells: &mut HashMap<(String, usize), Vec<Acc>>,
                        path: String,
                        col: usize,
                        ev: &Event| {
        let accs = cells
            .entry((path, col))
            .or_insert_with(|| spec.values.iter().map(|a| Acc::new(&a.func)).collect());
        for (i, a) in spec.values.iter().enumerate() {
            let num = ev
                .col_str(&a.column)
                .and_then(|s| parse_num_unit(&s).map(|(n, _)| n));
            let s = ev.col_str(&a.column);
            accs[i].push(num, s);
        }
        let _ = n_vals;
    };

    for ev in events {
        // chave de coluna
        let col_key = if spec.cols.is_empty() {
            "(total)".to_string()
        } else {
            spec.cols
                .iter()
                .map(|c| {
                    let v = ev.col_str(c).unwrap_or_default();
                    if v.is_empty() {
                        "(vazio)".into()
                    } else {
                        v
                    }
                })
                .collect::<Vec<_>>()
                .join(" → ")
        };
        let ci = *col_index.entry(col_key.clone()).or_insert_with(|| {
            col_keys.push(col_key);
            col_keys.len() - 1
        });

        // caminhos (todos os prefixos para a árvore de drill)
        let full: Vec<String> = spec
            .rows
            .iter()
            .map(|c| {
                let v = ev.col_str(c).unwrap_or_default();
                if v.is_empty() {
                    "(vazio)".into()
                } else {
                    v
                }
            })
            .collect();
        if full.is_empty() {
            acc_cell(&mut cells, "(total)".into(), ci, ev);
            continue;
        }
        for depth in 1..=full.len() {
            let path = full[..depth].join("\u{1f}");
            if path_set.insert(path.clone()) {
                paths.push(full[..depth].to_vec());
            }
            acc_cell(&mut cells, path, ci, ev);
        }
        // totais por coluna
        let taccs = totals
            .entry(ci)
            .or_insert_with(|| spec.values.iter().map(|a| Acc::new(&a.func)).collect());
        for (i, a) in spec.values.iter().enumerate() {
            let num = ev
                .col_str(&a.column)
                .and_then(|s| parse_num_unit(&s).map(|(n, _)| n));
            let s = ev.col_str(&a.column);
            taccs[i].push(num, s);
        }
    }

    // ordena caminhos em ordem de árvore (prefixo antes de filho)
    paths.sort_by(|a, b| {
        let ja = a.join("\u{1f}").to_lowercase();
        let jb = b.join("\u{1f}").to_lowercase();
        ja.cmp(&jb)
    });
    let truncated = paths.len() > spec.limit_rows;
    paths.truncate(spec.limit_rows);

    let cells_out: Vec<Vec<Vec<Value>>> = paths
        .iter()
        .map(|p| {
            let pkey = p.join("\u{1f}");
            (0..col_keys.len())
                .map(|ci| {
                    spec.values
                        .iter()
                        .enumerate()
                        .map(|(vi, a)| {
                            cells
                                .get(&(pkey.clone(), ci))
                                .map(|accs| accs[vi].finish(&a.func))
                                .unwrap_or(Value::Null)
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let totals_out: Vec<Vec<Value>> = (0..col_keys.len())
        .map(|ci| {
            spec.values
                .iter()
                .enumerate()
                .map(|(vi, a)| {
                    totals
                        .get(&ci)
                        .map(|accs| accs[vi].finish(&a.func))
                        .unwrap_or(Value::Null)
                })
                .collect()
        })
        .collect();

    PivotResult {
        value_names,
        col_keys,
        row_paths: paths,
        cells: cells_out,
        totals: totals_out,
        truncated,
    }
}

// ------------------------------------------------------------------ entrada

/// Resolve os eventos de trabalho (memória ou índice) para os comandos.
pub fn materialize_memory(events: &[Event], filters: &[Filter]) -> Vec<Event> {
    let idx = crate::query::filtered_indices(events, filters);
    idx.into_iter().map(|i| events[i].clone()).collect()
}

pub fn materialize_indexed(
    idx: &FileIndex,
    filters: &[Filter],
    codes: &CodesConfig,
    system: &CodesConfig,
    derived: &[CompiledDerived],
    cap: usize,
) -> Vec<Event> {
    let matched = crate::query::indexed_matches(idx, filters, codes, system, derived);
    matched
        .into_par_iter()
        .take(cap)
        .map(|i| crate::sources::event_at(idx, i, codes, system, derived))
        .collect()
}
