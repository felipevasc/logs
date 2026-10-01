//! Motor analítico: perfil de campos, séries para gráficos e pivô OLAP.
use crate::model::Event;
use crate::query::{AggSpec, AnalyticsBudget};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ------------------------------------------------------------------ unidades

#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
pub enum UnitKind {
    Number,
    Bytes, // KB/MB/GB (x1024)
    Bits,  // kbps/mbps (x1000, em bits)
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
    if let Ok(n) = s.parse::<f64>() {
        return n.is_finite().then_some((n, UnitKind::Number));
    }
    let lower = s.to_lowercase();
    let (num_part, unit_part) = split_num_unit(&lower);
    let n: f64 = num_part.replace(',', ".").parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    let parsed = match unit_part {
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
    };
    parsed.0.is_finite().then_some(parsed)
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
    pub sampled_events: usize,
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
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u8>().is_ok()
        })
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
            sampled_events: self.total + self.empty,
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
    for sample in 0..events.len().min(PROFILE_SAMPLE) {
        let ev = &events[sample * events.len() / events.len().min(PROFILE_SAMPLE)];
        for (col, vals) in map.iter_mut() {
            vals.push(ev.col_str(col).unwrap_or_default());
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
    pub chart: String,  // "time" | "terms"
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
    pub(crate) kind: String,
    pub(crate) unit: String, // number | bytes | bits | duration
    pub(crate) interval_ms: i64,
    /// "time": epoch ms do bucket; "terms": rótulo
    pub(crate) x: Vec<Value>,
    /// Exact category values for terms charts; None means missing/empty.
    pub(crate) x_values: Vec<Option<String>>,
    pub(crate) series: Vec<SeriesData>,
    pub(crate) incompatible_units: usize,
}

#[derive(Serialize)]
pub struct SeriesData {
    pub(crate) name: String,
    pub(crate) points: Vec<f64>,
    /// Values admitted to each accumulator; zero identifies an empty numeric bucket.
    /// Count includes all records; distinct includes records with a nonempty value.
    pub(crate) samples: Vec<usize>,
}

struct MetricAcc {
    metric: String,
    sum: f64,
    n: u64,
    min: Option<f64>,
    max: Option<f64>,
    distinct: crate::distinct::Counter,
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
    fn push_checked(
        &mut self,
        ev: &Event,
        field: Option<&str>,
        expected: Option<UnitKind>,
        budget: &mut AnalyticsBudget,
    ) -> Result<bool, String> {
        match self.metric.as_str() {
            "count" => self.n += 1,
            "distinct" => {
                if let Some(f) = field {
                    if let Some(v) = ev.col_str(f) {
                        if !v.is_empty() {
                            budget.charge(self.distinct.try_insert(v)?)?;
                            self.n += 1;
                        }
                    }
                }
            }
            _ => {
                if let Some(f) = field {
                    if let Some((n, unit)) = ev
                        .col_str(f)
                        .and_then(|s| parse_num_unit(&s))
                        .filter(|(n, _)| n.is_finite())
                    {
                        if expected.is_some_and(|e| e != unit) {
                            return Ok(true);
                        }
                        self.sum += n;
                        self.n += 1;
                        self.min = Some(self.min.map(|m: f64| m.min(n)).unwrap_or(n));
                        self.max = Some(self.max.map(|m: f64| m.max(n)).unwrap_or(n));
                    }
                }
            }
        }
        Ok(false)
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

fn dominant_unit(
    events: impl Iterator<Item = Event>,
    field: &str,
    sample: usize,
) -> Result<UnitKind, String> {
    let mut votes = [0usize; 5];
    let mut valid = 0;
    for ev in events {
        crate::operations::check()?;
        if valid >= sample {
            break;
        }
        if let Some((_, unit)) = ev
            .col_str(field)
            .and_then(|s| parse_num_unit(&s))
            .filter(|(n, _)| n.is_finite())
        {
            votes[unit as usize] += 1;
            valid += 1;
        }
    }
    crate::operations::check()?;
    Ok(dominant_of(votes))
}

/// Most frequent unit among sampled values (ties favor the lower kind).
pub(crate) fn dominant_of(votes: [usize; 5]) -> UnitKind {
    votes
        .into_iter()
        .enumerate()
        .max_by_key(|(unit, count)| (*count, std::cmp::Reverse(*unit)))
        .map(|(u, _)| match u {
            u if u == UnitKind::Bytes as usize => UnitKind::Bytes,
            u if u == UnitKind::Bits as usize => UnitKind::Bits,
            u if u == UnitKind::DurationMs as usize => UnitKind::DurationMs,
            _ => UnitKind::Number,
        })
        .unwrap_or(UnitKind::Number)
}

pub(crate) fn unit_name(u: UnitKind) -> String {
    match u {
        UnitKind::Bytes => "bytes",
        UnitKind::Bits => "bits",
        UnitKind::DurationMs => "duration",
        _ => "number",
    }
    .to_string()
}

pub fn compute_series(events: &[Event], spec: &SeriesSpec) -> Result<SeriesResult, String> {
    compute_series_stream(|| events.iter().cloned(), spec)
}

pub fn compute_series_stream<F, I>(events: F, spec: &SeriesSpec) -> Result<SeriesResult, String>
where
    F: Fn() -> I,
    I: Iterator<Item = Event>,
{
    let result = compute_series_budgeted(events, spec, &mut AnalyticsBudget::new())?;
    crate::operations::check()?;
    Ok(result)
}

/// One budget covers every pass and all accumulators, including disk-backed
/// keys and the final response. Spilling does not reset the payload allowance.
fn compute_series_budgeted<F, I>(
    events: F,
    spec: &SeriesSpec,
    budget: &mut AnalyticsBudget,
) -> Result<SeriesResult, String>
where
    F: Fn() -> I,
    I: Iterator<Item = Event>,
{
    crate::operations::check()?;
    let limit = spec.limit.unwrap_or(10).min(500);
    let field = spec.field.as_deref();
    if spec.chart == "terms" && spec.metric == "count" {
        let key = field.unwrap_or("level");
        let mut counts = crate::distinct::Terms::default();
        for ev in events() {
            crate::operations::check()?;
            let value = serde_json::to_string(&ev.col_str(key).filter(|s| !s.trim().is_empty()))
                .map_err(|e| e.to_string())?;
            budget.charge(counts.try_insert(value)?)?;
        }
        crate::operations::check()?;
        let top: Vec<(Option<String>, usize)> = counts
            .try_top(limit)?
            .into_iter()
            .map(|(key, count)| {
                serde_json::from_str(&key)
                    .map(|key| (key, count))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<_, _>>()?;
        charge_terms_output(budget, top.iter().map(|(key, _)| key), key)?;
        crate::operations::check()?;
        return Ok(SeriesResult {
            kind: "terms".into(),
            unit: "number".into(),
            interval_ms: 0,
            x: top
                .iter()
                .map(|(value, _)| Value::from(value.clone().unwrap_or_else(|| "(vazio)".into())))
                .collect(),
            x_values: top.iter().map(|(value, _)| value.clone()).collect(),
            series: vec![SeriesData {
                name: key.into(),
                samples: top.iter().map(|(_, n)| *n).collect(),
                points: top.into_iter().map(|(_, n)| n as f64).collect(),
            }],
            incompatible_units: 0,
        });
    }
    let dominant = match (spec.unit.as_deref(), field) {
        (Some(u), _) if u != "auto" => UnitKind::Number,
        (_, Some(f)) => dominant_unit(events(), f, 500)?,
        _ => UnitKind::Number,
    };
    let unit = series_unit(spec, |_| dominant);
    budget.charge(unit.len())?;
    let expected_unit = expected_unit(spec, &unit);
    let mut incompatible_units = 0;

    // Splits preserve the legacy exact top six and its deterministic ordering.
    let splits: Vec<String> = match &spec.split {
        Some(col) => {
            let mut counts = crate::distinct::Terms::default();
            for ev in events() {
                crate::operations::check()?;
                if let Some(v) = ev.col_str(col) {
                    if !v.is_empty() {
                        budget.charge(counts.try_insert(v)?)?;
                    }
                }
            }
            crate::operations::check()?;
            counts.try_top(6)?.into_iter().map(|(k, _)| k).collect()
        }
        None => vec![],
    };
    let has_splits = !splits.is_empty();
    let split_names: Vec<String> = if !has_splits {
        let name = spec.field.as_deref().unwrap_or("eventos");
        budget.charge(name.len().saturating_add(std::mem::size_of::<String>()))?;
        vec![name.into()]
    } else {
        // The ranking already charged each key. Move selected names instead
        // of retaining another copy for lookup and another for the response.
        budget.charge(splits.len().saturating_mul(std::mem::size_of::<String>()))?;
        splits
    };

    if spec.chart == "terms" {
        let key_field = field.unwrap_or("level");
        let mut accs: HashMap<Option<String>, MetricAcc> = HashMap::new();
        for ev in events() {
            crate::operations::check()?;
            let key = ev.col_str(key_field).filter(|s| !s.trim().is_empty());
            let acc = match accs.entry(key) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    charge_metric_group(budget, entry.key(), &spec.metric)?;
                    entry.insert(MetricAcc::new(&spec.metric))
                }
            };
            incompatible_units +=
                usize::from(acc.push_checked(&ev, field, expected_unit, budget)?);
        }
        crate::operations::check()?;
        // Move the keys instead of duplicating every group before selecting top N.
        budget.charge(accs.len().saturating_mul(std::mem::size_of::<(
            Option<String>,
            f64,
            usize,
        )>()))?;
        let mut items: Vec<(Option<String>, f64, usize)> = accs
            .into_iter()
            .map(|(key, acc)| (key, acc.value(), acc.n as usize))
            .collect();
        items.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items.truncate(limit);
        charge_terms_output(budget, items.iter().map(|(key, _, _)| key), &split_names[0])?;
        crate::operations::check()?;
        return Ok(SeriesResult {
            kind: "terms".into(),
            unit,
            interval_ms: 0,
            x: items
                .iter()
                .map(|(k, _, _)| Value::from(k.clone().unwrap_or_else(|| "(vazio)".into())))
                .collect(),
            x_values: items.iter().map(|(key, _, _)| key.clone()).collect(),
            series: vec![SeriesData {
                name: split_names[0].clone(),
                samples: items.iter().map(|(_, _, n)| *n).collect(),
                points: items.into_iter().map(|(_, v, _)| v).collect(),
            }],
            incompatible_units,
        });
    }

    let mut bounds: Option<(i64, i64)> = None;
    for ev in events() {
        crate::operations::check()?;
        if let Some(t) = ev.timestamp {
            bounds = Some(bounds.map(|(a, b)| (a.min(t), b.max(t))).unwrap_or((t, t)));
        }
    }
    crate::operations::check()?;
    let Some((tmin, tmax)) = bounds else {
        return Ok(SeriesResult {
            kind: "time".into(),
            unit,
            interval_ms: 0,
            x: vec![],
            x_values: vec![],
            series: vec![],
            incompatible_units: 0,
        });
    };
    let (interval, n_buckets) = series_interval(spec, tmin, tmax);
    budget.charge(n_buckets.saturating_mul(std::mem::size_of::<HashMap<usize, MetricAcc>>()))?;
    let mut accs: Vec<HashMap<usize, MetricAcc>> =
        (0..n_buckets).map(|_| HashMap::new()).collect();
    for ev in events() {
        crate::operations::check()?;
        let Some(t) = ev.timestamp else { continue };
        let b = (t.saturating_sub(tmin) / interval) as usize;
        if b >= n_buckets {
            continue;
        }
        let series_index = match &spec.split {
            Some(col) => {
                if !has_splits { continue; }
                let Some(value) = ev.col_ref(col) else { continue; };
                let Some(index) = split_names.iter().position(|name| name == value.as_ref()) else { continue; };
                index
            }
            None => 0,
        };
        let acc = match accs[b].entry(series_index) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                // Sparse cells keep their existing metric/distinct budgets,
                // but reference one selected label by index instead of owning it.
                budget.charge(
                    128usize.saturating_add(std::mem::size_of::<MetricAcc>())
                        .saturating_add(spec.metric.len()),
                )?;
                entry.insert(MetricAcc::new(&spec.metric))
            }
        };
        incompatible_units += usize::from(acc.push_checked(&ev, field, expected_unit, budget)?);
    }
    budget.charge(n_buckets.saturating_mul(std::mem::size_of::<Value>()))?;
    for _ in &split_names {
        budget.charge(
            std::mem::size_of::<SeriesData>().saturating_add(
                n_buckets.saturating_mul(std::mem::size_of::<f64>() + std::mem::size_of::<usize>()),
            ),
        )?;
    }
    crate::operations::check()?;
    let result = SeriesResult {
        kind: "time".into(),
        unit,
        interval_ms: interval,
        x: (0..n_buckets)
            .map(|b| Value::from(tmin.saturating_add((b as i64).saturating_mul(interval))))
            .collect(),
        x_values: vec![],
        series: split_names
            .into_iter()
            .enumerate()
            .map(|(index, name)| SeriesData {
                name,
                samples: accs
                    .iter()
                    .map(|m| m.get(&index).map(|a| a.n as usize).unwrap_or(0))
                    .collect(),
                points: accs
                    .iter()
                    .map(|m| m.get(&index).map(|a| a.value()).unwrap_or(0.0))
                    .collect(),
            })
            .collect(),
        incompatible_units,
    };
    crate::operations::check()?;
    Ok(result)
}

fn charge_metric_group(
    budget: &mut AnalyticsBudget,
    key: &Option<String>,
    metric: &str,
) -> Result<(), String> {
    budget.group(key, 0)?;
    budget.charge(std::mem::size_of::<MetricAcc>().saturating_add(metric.len()))
}

fn charge_terms_output<'a>(
    budget: &mut AnalyticsBudget,
    keys: impl Iterator<Item = &'a Option<String>>,
    name: &str,
) -> Result<(), String> {
    budget.charge(name.len().saturating_add(std::mem::size_of::<SeriesData>()))?;
    for key in keys {
        crate::operations::check()?;
        budget.charge(
            key.as_ref()
                .map_or("(vazio)".len(), |s| s.len().saturating_mul(2))
                .saturating_add(
                    std::mem::size_of::<Value>()
                        + std::mem::size_of::<Option<String>>()
                        + std::mem::size_of::<f64>()
                        + std::mem::size_of::<usize>(),
                ),
        )?;
    }
    Ok(())
}

/// Unit of a series: explicit, or the dominant unit of the metric field.
pub(crate) fn series_unit(spec: &SeriesSpec, dominant: impl FnOnce(&str) -> UnitKind) -> String {
    match (spec.unit.as_deref(), spec.field.as_deref()) {
        (Some(u), _) if u != "auto" => u.to_string(),
        (Some(_), None) | (None, None) => "number".to_string(),
        (_, Some(f)) => unit_name(dominant(f)),
    }
}

/// Unit values must have to enter numeric metrics (none for counts).
pub(crate) fn expected_unit(spec: &SeriesSpec, unit: &str) -> Option<UnitKind> {
    if matches!(spec.metric.as_str(), "count" | "distinct") {
        None
    } else {
        match unit {
            "number" => Some(UnitKind::Number),
            "bytes" => Some(UnitKind::Bytes),
            "bits" => Some(UnitKind::Bits),
            "duration" => Some(UnitKind::DurationMs),
            _ => None,
        }
    }
}

/// Bucket width and count of a time series spanning `[tmin, tmax]`.
pub(crate) fn series_interval(spec: &SeriesSpec, tmin: i64, tmax: i64) -> (i64, usize) {
    let interval = spec.interval_ms.filter(|n| *n > 0).unwrap_or_else(|| {
        let span = tmax.saturating_sub(tmin).max(1);
        // ~60 buckets; escolhe intervalo "redondo"
        let target = span / 60;
        for nice in [
            1_000i64,
            5_000,
            15_000,
            60_000,
            300_000,
            900_000,
            1_800_000,
            3_600_000,
            21_600_000,
            43_200_000,
            86_400_000,
            604_800_000,
            2_592_000_000,
        ] {
            if target <= nice {
                return nice;
            }
        }
        2_592_000_000
    });
    let interval = interval.max((tmax.saturating_sub(tmin) / 2000).max(1));
    let n_buckets = (tmax.saturating_sub(tmin) / interval + 1) as usize;
    (interval, n_buckets)
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
    row_values: Vec<Vec<Option<String>>>,
    col_values: Vec<Vec<Option<String>>>,
    incompatible_units: Vec<usize>,
    value_units: Vec<String>,
    /// cells[i][j][v] = valor da linha i, coluna j, medida v
    cells: Vec<Vec<Vec<Value>>>,
    /// totais[i][v]
    totals: Vec<Vec<Value>>,
    truncated: bool,
    complete: bool,
    processed_events: usize,
}

pub fn pivot(events: &[Event], spec: &PivotSpec) -> Result<PivotResult, String> {
    pivot_stream(events.iter().cloned(), spec)
}

pub fn pivot_stream(
    events: impl Iterator<Item = Event>,
    spec: &PivotSpec,
) -> Result<PivotResult, String> {
    let result = pivot_budgeted(events, spec, &mut AnalyticsBudget::new())?;
    crate::operations::check()?;
    Ok(result)
}

fn pivot_budgeted(
    events: impl Iterator<Item = Event>,
    spec: &PivotSpec,
    budget: &mut AnalyticsBudget,
) -> Result<PivotResult, String> {
    crate::operations::check()?;
    let mut value_names = Vec::new();
    for a in &spec.values {
        crate::operations::check()?;
        let bytes = if a.alias.is_empty() {
            a.func
                .len()
                .saturating_add(a.column.len())
                .saturating_add(2)
        } else {
            a.alias.len()
        };
        // Names, unit metadata, and the transient per-event values vector.
        budget.charge(bytes.saturating_add(128))?;
        value_names.push(if a.alias.is_empty() {
            format!("{}({})", a.func, a.column)
        } else {
            a.alias.clone()
        });
    }

    enum Acc {
        Count(u64),
        CountDistinct(crate::distinct::Counter),
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
        fn push(
            &mut self,
            v: Option<f64>,
            s: Option<&str>,
            budget: &mut AnalyticsBudget,
        ) -> Result<(), String> {
            match self {
                Acc::Count(n) => *n += 1,
                Acc::CountDistinct(set) => {
                    if let Some(s) = s {
                        budget.charge(set.try_insert(s.to_string())?)?;
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
                        if let Some(s) = s.filter(|s| !s.is_empty()) {
                            budget.charge(s.len().saturating_add(std::mem::size_of::<String>()))?;
                            items.push(s.to_string());
                        }
                    }
                }
            }
            Ok(())
        }
        fn finish(&self, func: &str, budget: &mut AnalyticsBudget) -> Result<Value, String> {
            crate::operations::check()?;
            Ok(match (self, func) {
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
                (Acc::Str(items), _) => {
                    let bytes = items
                        .iter()
                        .fold(0usize, |n, s| n.saturating_add(s.len()))
                        .saturating_add(items.len().saturating_sub(1).saturating_mul(2));
                    budget.charge(bytes)?;
                    Value::from(items.join(", "))
                }
                _ => Value::Null,
            })
        }
    }

    type Path = Vec<Option<String>>;
    fn path_bytes(path: &Path) -> usize {
        path.iter().fold(std::mem::size_of::<Path>(), |n, value| {
            n.saturating_add(std::mem::size_of::<Option<String>>())
                .saturating_add(value.as_ref().map_or(0, String::len))
        })
    }
    fn path_label(path: &Path, budget: &mut AnalyticsBudget) -> Result<String, String> {
        let bytes = if path.is_empty() {
            "(total)".len()
        } else {
            path.iter()
                .fold(0usize, |n, value| {
                    n.saturating_add(value.as_deref().unwrap_or("(vazio)").len())
                })
                .saturating_add(path.len().saturating_sub(1).saturating_mul(" → ".len()))
        };
        budget.charge(bytes.saturating_add(std::mem::size_of::<String>()))?;
        let mut label = String::with_capacity(bytes);
        if path.is_empty() {
            label.push_str("(total)");
        }
        for (i, value) in path.iter().enumerate() {
            if i > 0 {
                label.push_str(" → ");
            }
            label.push_str(value.as_deref().unwrap_or("(vazio)"));
        }
        Ok(label)
    }
    let mut col_keys: Vec<String> = Vec::new();
    let mut col_values: Vec<Path> = Vec::new();
    let mut col_index: HashMap<Path, usize> = HashMap::new();
    // Intern typed paths once; cells retain numeric IDs, not a copy of every
    // potentially wide row label for every column and aggregation.
    let mut paths: Vec<Path> = Vec::new();
    let mut path_index: HashMap<Path, usize> = HashMap::new();
    let mut cells: HashMap<(usize, usize), Vec<Acc>> = HashMap::new();
    let mut totals: HashMap<usize, Vec<Acc>> = HashMap::new();
    let n_vals = spec.values.len().max(1);
    let mut value_units = vec![None; spec.values.len()];
    let mut incompatible_units = vec![0usize; spec.values.len()];
    let acc_bytes = spec
        .values
        .len()
        .saturating_mul(std::mem::size_of::<Acc>())
        .saturating_add(128);
    let mut processed_events = 0;
    for ev in events {
        crate::operations::check()?;
        let col_key: Path = spec
            .cols
            .iter()
            .map(|c| ev.col_str(c).filter(|v| !v.trim().is_empty()))
            .collect();
        let ci = if let Some(&ci) = col_index.get(&col_key) {
            ci
        } else {
            if col_keys.len() >= 200 {
                return Err("O pivô excedeu o limite de 200 colunas. Restrinja os filtros ou reduza os agrupamentos.".into());
            }
            budget.charge(path_bytes(&col_key).saturating_mul(2).saturating_add(64))?;
            let label = path_label(&col_key, budget)?;
            let ci = col_keys.len();
            col_index.insert(col_key.clone(), ci);
            col_values.push(col_key);
            col_keys.push(label);
            ci
        };
        let values: Vec<_> = spec
            .values
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let text = ev.col_str(&a.column);
                if matches!(a.func.as_str(), "sum" | "avg" | "min" | "max") {
                    let parsed = if a.column == "timestamp" {
                        ev.timestamp.map(|t| (t as f64, UnitKind::Number))
                    } else {
                        text.as_deref().and_then(parse_num_unit)
                    };
                    let num = parsed.and_then(|(num, unit)| {
                        if !num.is_finite() {
                            return None;
                        }
                        let expected = value_units[i].get_or_insert(unit);
                        if *expected != unit {
                            incompatible_units[i] += 1;
                            None
                        } else {
                            Some(num)
                        }
                    });
                    (num, None)
                } else if a.func == "count" {
                    (None, None)
                } else {
                    (None, text)
                }
            })
            .collect();
        let full: Path = spec
            .rows
            .iter()
            .map(|c| ev.col_str(c).filter(|v| !v.trim().is_empty()))
            .collect();
        // Empty row dimensions still have one total row, as before.
        for depth in usize::from(!full.is_empty())..=full.len() {
            crate::operations::check()?;
            let prefix = &full[..depth];
            let pi = if let Some(&pi) = path_index.get(prefix) {
                pi
            } else {
                let bytes = prefix.iter().fold(std::mem::size_of::<Path>(), |n, value| {
                    n.saturating_add(std::mem::size_of::<Option<String>>())
                        .saturating_add(value.as_ref().map_or(0, String::len))
                });
                budget.charge(bytes.saturating_mul(2).saturating_add(64))?;
                let pi = paths.len();
                let path = prefix.to_vec();
                path_index.insert(path.clone(), pi);
                paths.push(path);
                pi
            };
            let cell_count = cells.len();
            let accs = match cells.entry((pi, ci)) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    if cell_count >= 100_000 / n_vals {
                        return Err("O pivô excedeu o orçamento de células. Restrinja os filtros ou reduza os agrupamentos.".into());
                    }
                    budget.charge(acc_bytes)?;
                    entry.insert(spec.values.iter().map(|a| Acc::new(&a.func)).collect())
                }
            };
            for (acc, (num, text)) in accs.iter_mut().zip(&values) {
                crate::operations::check()?;
                acc.push(*num, text.as_deref(), budget)?;
            }
        }
        let taccs = match totals.entry(ci) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                budget.charge(acc_bytes)?;
                entry.insert(spec.values.iter().map(|a| Acc::new(&a.func)).collect())
            }
        };
        for (acc, (num, text)) in taccs.iter_mut().zip(&values) {
            crate::operations::check()?;
            acc.push(*num, text.as_deref(), budget)?;
        }
        processed_events += 1;
    }
    crate::operations::check()?;

    // Sort IDs by the same typed tree ordering (prefix before child).
    budget.charge(paths.len().saturating_mul(std::mem::size_of::<usize>()))?;
    let mut ordered_paths: Vec<usize> = (0..paths.len()).collect();
    ordered_paths.sort_by(|&a, &b| paths[a].cmp(&paths[b]));
    let output_rows = spec
        .limit_rows
        .min(2_000)
        .min(100_000 / col_keys.len().max(1) / n_vals);
    let truncated = paths.len() > output_rows;
    ordered_paths.truncate(output_rows);

    let output_cell_bytes = spec
        .values
        .len()
        .saturating_mul(std::mem::size_of::<Value>())
        .saturating_add(std::mem::size_of::<Vec<Value>>());
    budget.charge(
        ordered_paths
            .len()
            .saturating_mul(
                col_keys
                    .len()
                    .saturating_mul(output_cell_bytes)
                    .saturating_add(std::mem::size_of::<Vec<Vec<Value>>>()),
            )
            .saturating_add(col_keys.len().saturating_mul(output_cell_bytes)),
    )?;
    let mut cells_out = Vec::new();
    let mut row_paths = Vec::new();
    let mut row_values = Vec::new();
    for pi in ordered_paths {
        crate::operations::check()?;
        let path = &paths[pi];
        budget.charge(path_bytes(path).saturating_add(std::mem::size_of::<Vec<String>>()))?;
        let labels = if path.is_empty() {
            budget.charge(
                "(total)"
                    .len()
                    .saturating_add(std::mem::size_of::<String>()),
            )?;
            vec!["(total)".into()]
        } else {
            let mut labels = Vec::new();
            for value in path {
                let value = value.as_deref().unwrap_or("(vazio)");
                budget.charge(value.len().saturating_add(std::mem::size_of::<String>()))?;
                labels.push(value.to_string());
            }
            labels
        };
        row_paths.push(labels);
        row_values.push(path.clone());
        let mut row = Vec::new();
        for ci in 0..col_keys.len() {
            let mut cell = Vec::new();
            for (vi, a) in spec.values.iter().enumerate() {
                crate::operations::check()?;
                let value = if incompatible_units[vi] > 0 {
                    Value::Null
                } else if let Some(accs) = cells.get(&(pi, ci)) {
                    accs[vi].finish(&a.func, budget)?
                } else {
                    Value::Null
                };
                cell.push(value);
            }
            row.push(cell);
        }
        cells_out.push(row);
    }
    let mut totals_out = Vec::new();
    for ci in 0..col_keys.len() {
        let mut total = Vec::new();
        for (vi, a) in spec.values.iter().enumerate() {
            crate::operations::check()?;
            total.push(if incompatible_units[vi] > 0 {
                Value::Null
            } else if let Some(accs) = totals.get(&ci) {
                accs[vi].finish(&a.func, budget)?
            } else {
                Value::Null
            });
        }
        totals_out.push(total);
    }
    crate::operations::check()?;
    Ok(PivotResult {
        value_names,
        col_keys,
        row_paths,
        row_values,
        col_values,
        incompatible_units,
        value_units: value_units
            .into_iter()
            .map(|unit| unit.map(unit_name).unwrap_or_default())
            .collect(),
        cells: cells_out,
        totals: totals_out,
        truncated,
        complete: true,
        processed_events,
    })
}

#[cfg(test)]
mod recovery_budget_tests {
    use super::*;
    use serde_json::json;

    fn event(source: &str, message: &str, timestamp: i64) -> Event {
        let mut event = Event::empty();
        event.source = source.into();
        event.message = message.into();
        event.timestamp = Some(timestamp);
        event
    }

    fn series_spec(chart: &str, metric: &str) -> SeriesSpec {
        SeriesSpec {
            chart: chart.into(),
            metric: metric.into(),
            field: Some("message".into()),
            interval_ms: Some(1000),
            split: None,
            limit: Some(10),
            unit: Some("auto".into()),
        }
    }

    fn pivot_spec(func: &str) -> PivotSpec {
        PivotSpec {
            rows: vec!["source".into()],
            cols: vec![],
            values: vec![AggSpec {
                func: func.into(),
                column: "message".into(),
                alias: String::new(),
            }],
            limit_rows: 2000,
        }
    }

    #[test]
    fn time_split_labels_do_not_repeat_in_the_shared_bucket_budget() {
        let labels: Vec<_> = (0..6).map(|i| format!("{i}{}", "x".repeat(8 << 10))).collect();
        let events: Vec<_> = (0..41).flat_map(|bucket| labels.iter().flat_map(move |label| {
            [event(label, "1KB", bucket * 1000), event(label, "1KB", bucket * 1000)]
        })).collect();
        for (metric, point) in [("count", 2.0), ("sum", 2048.0), ("distinct", 1.0)] {
            let mut spec = series_spec("time", metric);
            spec.split = Some("source".into());
            spec.unit = Some("bytes".into());
            let result = crate::resources::with_analytics_limit(256 << 10, || compute_series(&events, &spec)).unwrap();
            assert_eq!(result.series.iter().map(|s| &s.name).collect::<Vec<_>>(), labels.iter().collect::<Vec<_>>());
            assert!(result.series.iter().all(|s| s.points == vec![point; 41] && s.samples == vec![2; 41]));
            assert_eq!(result.incompatible_units, 0);
            assert!(crate::resources::with_analytics_limit(4096, || compute_series(&events, &spec)).is_err(),
                "selected labels and distinct values still share the finite budget");
        }
    }

    #[test]
    fn time_split_indices_preserve_typed_missing_and_empty_values() {
        let mut events = Vec::new();
        for value in [None, Some(json!("")), Some(json!(" ")), Some(json!(7)), Some(json!(false)), Some(Value::Null), Some(json!("null")), Some(json!({"a":1}))] {
            let mut row = event("source", "1KB", 0);
            if let Some(value) = value { row.fields.insert("group".into(), value); }
            events.push(row);
        }
        let mut spec = series_spec("time", "count");
        spec.split = Some("group".into());
        let result = compute_series(&events, &spec).unwrap();
        assert_eq!(result.series.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["null", " ", "7", "false", "{\"a\":1}"]);
        assert_eq!(result.series.iter().map(|s| s.samples[0]).collect::<Vec<_>>(), vec![2,1,1,1,1]);
        for row in &mut events { row.fields.remove("group"); }
        let empty = compute_series(&events, &spec).unwrap();
        assert_eq!(empty.series.len(), 1);
        assert_eq!(empty.series[0].name, "message");
        assert_eq!(empty.series[0].samples, vec![0]);
        assert_eq!(empty.series[0].points, vec![0.0]);
    }

    #[test]
    fn series_preserves_exact_ranking_empty_keys_units_and_samples() {
        let events = vec![
            event("a", "b", 0),
            event("a", "a", 0),
            event("a", "b", 0),
            event("a", "", 0),
            event("a", "(vazio)", 0),
        ];
        let terms = compute_series(&events, &series_spec("terms", "count")).unwrap();
        assert_eq!(
            terms.x,
            vec![json!("b"), json!("(vazio)"), json!("a"), json!("(vazio)")]
        );
        assert_eq!(
            terms.x_values,
            vec![
                Some("b".into()),
                Some("(vazio)".into()),
                Some("a".into()),
                None
            ]
        );
        assert_eq!(terms.series[0].points, vec![2.0, 1.0, 1.0, 1.0]);
        assert_eq!(terms.series[0].samples, vec![2, 1, 1, 1]);

        let events = vec![
            event("a", "1KB", 0),
            event("a", "3KB", 0),
            event("a", "5ms", 1000),
        ];
        let spec = series_spec("time", "avg");
        let result = compute_series(&events, &spec).unwrap();
        assert_eq!(result.unit, "bytes");
        assert_eq!(result.x, vec![json!(0), json!(1000)]);
        assert_eq!(result.series[0].points, vec![2048.0, 0.0]);
        assert_eq!(result.series[0].samples, vec![2, 0]);
        assert_eq!(result.incompatible_units, 1);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(compute_series_stream(|| events.iter().cloned(), &spec).unwrap())
                .unwrap()
        );
    }

    #[test]
    fn series_splits_keep_exact_top_six_and_distinct_bucket_counts() {
        let mut events: Vec<_> = (0..7)
            .map(|i| event(&format!("group-{i}"), "same", 0))
            .collect();
        events.push(event("group-6", "same", 0));
        events.push(event("group-6", "another", 0));
        events.push(event("group-6", "same", 1000));
        let mut spec = series_spec("time", "distinct");
        spec.split = Some("source".into());
        let result = compute_series(&events, &spec).unwrap();
        assert_eq!(
            result
                .series
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["group-6", "group-0", "group-1", "group-2", "group-3", "group-4"]
        );
        assert_eq!(result.series[0].points, vec![2.0, 1.0]);
        assert_eq!(result.series[0].samples, vec![3, 1]);
        assert_eq!(result.series[1].points, vec![1.0, 0.0]);
    }

    #[test]
    fn distinct_accumulators_share_budget_without_charging_duplicates() {
        crate::resources::with_analytics_limit(140, || {
            let mut budget = AnalyticsBudget::new();
            let mut first = MetricAcc::new("distinct");
            let mut second = MetricAcc::new("distinct");
            let same = event("a", "same", 0);
            first
                .push_checked(&same, Some("message"), None, &mut budget)
                .unwrap();
            first
                .push_checked(&same, Some("message"), None, &mut budget)
                .unwrap();
            second
                .push_checked(&same, Some("message"), None, &mut budget)
                .unwrap();
            assert_eq!(first.value(), 1.0);
            assert_eq!(first.n, 2);
            assert_eq!(second.value(), 1.0);
            assert!(second
                .push_checked(&event("a", "new", 0), Some("message"), None, &mut budget)
                .is_err());
        });
    }

    #[test]
    fn recovery_series_limits_labels_buckets_and_distinct_payload_together() {
        let spec = series_spec("terms", "count");
        crate::resources::with_analytics_limit(4096, || {
            let repeats = vec![event("a", "same", 0); 100];
            assert_eq!(
                compute_series(&repeats, &spec).unwrap().series[0].points,
                vec![100.0]
            );
            let wide = vec![event("a", &"日".repeat(1500), 0)];
            assert!(compute_series(&wide, &spec)
                .err()
                .unwrap()
                .contains("orçamento"));
            assert!(compute_series(&wide, &series_spec("terms", "distinct")).is_err());
            let buckets: Vec<_> = (0..20)
                .map(|i| event("a", &"x".repeat(256), i * 1000))
                .collect();
            assert!(compute_series(&buckets, &series_spec("time", "distinct")).is_err());
        });
    }

    #[test]
    fn recovery_pivot_preserves_typed_tree_order_and_full_totals() {
        let mut events = vec![
            event("B", "2KB", 0),
            event("A", "4KB", 0),
            event("A", "6KB", 0),
        ];
        events[0].level = "leaf".into();
        events[1].level = "leaf".into();
        events[2].level.clear();
        events[0].code = "x".into();
        events[1].code = "x".into();
        events[2].code = "y".into();
        let mut spec = pivot_spec("sum");
        spec.rows.push("level".into());
        spec.cols.push("code".into());
        let result = pivot(&events, &spec).unwrap();
        assert_eq!(
            result.row_values,
            vec![
                vec![Some("A".into())],
                vec![Some("A".into()), None],
                vec![Some("A".into()), Some("leaf".into())],
                vec![Some("B".into())],
                vec![Some("B".into()), Some("leaf".into())],
            ]
        );
        assert_eq!(result.col_keys, vec!["x", "y"]);
        assert_eq!(
            result.cells[0],
            vec![vec![json!(4096.0)], vec![json!(6144.0)]]
        );
        assert_eq!(
            result.totals,
            vec![vec![json!(6144.0)], vec![json!(6144.0)]]
        );
        assert_eq!(result.value_units, vec!["bytes"]);
        assert!(result.complete);
        assert!(!result.truncated);
        assert_eq!(result.processed_events, 3);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(pivot_stream(events.iter().cloned(), &spec).unwrap()).unwrap()
        );
        spec.limit_rows = 1;
        let limited = pivot(&events, &spec).unwrap();
        assert_eq!(limited.row_values, vec![vec![Some("A".into())]]);
        assert_eq!(
            limited.totals,
            vec![vec![json!(6144.0)], vec![json!(6144.0)]]
        );
        assert!(limited.complete && limited.truncated);
        assert_eq!(limited.processed_events, 3);
    }

    #[test]
    fn pivot_interning_keeps_separator_and_missing_labels_distinct() {
        let mut events = vec![event("", "left → right", 0), event("(vazio)", "left", 0)];
        events[0].code = "tail".into();
        events[1].code = "right → tail".into();
        let mut spec = pivot_spec("count");
        spec.cols = vec!["message".into(), "code".into()];
        let result = pivot(&events, &spec).unwrap();
        assert_eq!(result.row_paths, vec![vec!["(vazio)"], vec!["(vazio)"]]);
        assert_eq!(
            result.row_values,
            vec![vec![None], vec![Some("(vazio)".into())]]
        );
        assert_eq!(
            result.col_keys,
            vec!["left → right → tail", "left → right → tail"]
        );
        assert_ne!(result.col_values[0], result.col_values[1]);
        assert_eq!(
            result.cells,
            vec![
                vec![vec![json!(1)], vec![Value::Null]],
                vec![vec![Value::Null], vec![json!(1)]],
            ]
        );
        assert_eq!(result.totals, vec![vec![json!(1)], vec![json!(1)]]);
    }

    #[test]
    fn pivot_retains_first_fifty_strings_and_exact_distinct_counts() {
        let mut events: Vec<_> = (0..60)
            .map(|i| event("group", &format!("v{i:02}"), 0))
            .collect();
        events.insert(0, event("group", "", 0));
        events.push(events[1].clone());
        let expected = (0..50)
            .map(|i| format!("v{i:02}"))
            .collect::<Vec<_>>()
            .join(", ");
        let strings = pivot(&events, &pivot_spec("string_agg")).unwrap();
        assert_eq!(strings.cells[0][0][0], json!(expected));
        assert_eq!(strings.totals[0][0], json!(expected));
        let distinct = pivot(&events, &pivot_spec("count_distinct")).unwrap();
        assert_eq!(distinct.cells[0][0][0], json!(61));
        assert_eq!(distinct.totals[0][0], json!(61));
    }

    #[test]
    fn pivot_rejects_shared_label_string_and_final_output_budget_exhaustion() {
        let wide_label = vec![event(&"日".repeat(500), "x", 0)];
        crate::resources::with_analytics_limit(1024, || {
            assert!(pivot(&wide_label, &pivot_spec("count")).is_err());
        });
        let grouped: Vec<_> = (0..8)
            .map(|i| event(&format!("group-{i}"), &"x".repeat(512), 0))
            .collect();
        crate::resources::with_analytics_limit(4096, || {
            assert!(pivot(&grouped, &pivot_spec("string_agg")).is_err());
            assert!(pivot(&grouped, &pivot_spec("count_distinct")).is_err());
        });
        let mut spec = pivot_spec("string_agg");
        spec.rows.clear();
        let event = vec![event("a", &"x".repeat(512), 0)];
        // Both the cell and total retain their own first-N strings and final joins.
        crate::resources::with_analytics_limit(2500, || {
            assert!(pivot(&event, &spec).is_err());
        });
        crate::resources::with_analytics_limit(4096, || {
            assert_eq!(
                pivot(&event, &spec).unwrap().cells[0][0][0],
                json!("x".repeat(512))
            );
        });
    }

    #[test]
    fn pivot_unit_conflicts_and_column_caps_never_claim_partial_success() {
        let mixed = vec![event("a", "1KB", 0), event("a", "2ms", 0)];
        let result = pivot(&mixed, &pivot_spec("sum")).unwrap();
        assert_eq!(result.incompatible_units, vec![1]);
        assert_eq!(result.cells[0][0][0], Value::Null);
        assert_eq!(result.totals[0][0], Value::Null);
        assert!(result.complete);
        let columns: Vec<_> = (0..201)
            .map(|i| event(&format!("column-{i}"), "x", 0))
            .collect();
        let mut spec = pivot_spec("count");
        spec.cols = vec!["source".into()];
        spec.rows.clear();
        assert!(pivot(&columns, &spec)
            .err()
            .unwrap()
            .contains("200 colunas"));
    }

    #[test]
    fn recovery_rankings_and_distinct_aggregates_propagate_spill_errors() {
        let events = vec![event("a", "1", 0)];
        crate::distinct::with_spill_failure(|| {
            assert!(compute_series(&events, &series_spec("terms", "count")).is_err());
            assert!(compute_series(&events, &series_spec("terms", "distinct")).is_err());
            assert!(compute_series(&events, &series_spec("time", "distinct")).is_err());
            let mut spec = series_spec("time", "count");
            spec.split = Some("source".into());
            assert!(compute_series(&events, &spec).is_err());
            assert!(pivot(&events, &pivot_spec("count_distinct")).is_err());
        });
    }

    #[test]
    fn every_series_pass_and_pivot_propagate_cancellation() {
        let events = vec![event("a", "1KB", 0), event("a", "2KB", 1000)];
        let mut spec = series_spec("time", "distinct");
        spec.split = Some("source".into());
        // Unit discovery, split ranking, bounds, and final bucket accumulation.
        for cancelled_pass in 0..4 {
            let id = format!("recovery-series-{}", uuid::Uuid::new_v4());
            let token = crate::operations::token(Some(id.clone())).unwrap();
            let pass = std::cell::Cell::new(0);
            let outer = crate::operations::run_with_token(token, || {
                let result = compute_series_stream(
                    || {
                        let current = pass.get();
                        pass.set(current + 1);
                        let id = &id;
                        events.iter().cloned().enumerate().map(move |(i, event)| {
                            if current == cancelled_pass && i == 1 {
                                assert!(crate::operations::cancel_id(id));
                            }
                            event
                        })
                    },
                    &spec,
                );
                assert_eq!(result.err().as_deref(), Some("Operação cancelada."));
            });
            assert!(outer.is_err());
        }
        let id = format!("recovery-pivot-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let outer = crate::operations::run_with_token(token, || {
            let result = pivot_stream(
                events.iter().cloned().enumerate().map(|(i, event)| {
                    if i == 1 {
                        assert!(crate::operations::cancel_id(&id));
                    }
                    event
                }),
                &pivot_spec("count"),
            );
            assert_eq!(result.err().as_deref(), Some("Operação cancelada."));
        });
        assert!(outer.is_err());
    }

    #[test]
    fn cancelled_empty_streams_do_not_return_successful_empty_results() {
        for chart in ["terms", "time"] {
            let id = format!("recovery-empty-{}", uuid::Uuid::new_v4());
            let token = crate::operations::token(Some(id.clone())).unwrap();
            let outer = crate::operations::run_with_token(token, || {
                let mut spec = series_spec(chart, "count");
                spec.unit = Some("number".into());
                let result = compute_series_stream(
                    || {
                        std::iter::from_fn(|| {
                            assert!(crate::operations::cancel_id(&id));
                            None
                        })
                    },
                    &spec,
                );
                assert_eq!(result.err().as_deref(), Some("Operação cancelada."));
            });
            assert!(outer.is_err());
        }
        let id = format!("recovery-empty-pivot-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let outer = crate::operations::run_with_token(token, || {
            let result = pivot_stream(
                std::iter::from_fn(|| {
                    assert!(crate::operations::cancel_id(&id));
                    None
                }),
                &pivot_spec("count"),
            );
            assert_eq!(result.err().as_deref(), Some("Operação cancelada."));
        });
        assert!(outer.is_err());
    }
}
