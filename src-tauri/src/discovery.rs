//! Bounded, deterministic exploratory statistics. Counts describe the sample;
//! associations are co-occurrences and never claims about causality.
use crate::{
    analysis::{parse_num_unit, UnitKind},
    insights,
    model::Event,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
#[path = "temporal.rs"]
mod temporal;

pub const SAMPLE_CAP: usize = 6_000;
const FIELD_CAP: usize = 24;
const VALUE_BYTES: usize = 200;
type CandidateField = (usize, Vec<(String, usize)>, HashMap<String, String>);

#[derive(Default, Serialize)]
pub struct Discovery {
    pub total: usize,
    pub sample_count: usize,
    pub limited: bool,
    pub complete: bool,
    pub fields_limited: bool,
    pub errors: usize,
    pub warnings: usize,
    pub missing_time: usize,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub fields_considered: Vec<String>,
    pub categories: Vec<Category>,
    pub associations: Vec<Association>,
    pub outliers: Vec<Outlier>,
    pub templates: Vec<Template>,
    pub behavior_shifts: Vec<temporal::Shift>,
    pub changes: Vec<temporal::Shift>,
    pub timed_sample_count: usize,
    pub time_bins: usize,
    pub temporal_limited: bool,
}
#[derive(Serialize)]
pub struct ValueCount {
    pub value: String,
    pub count: usize,
    pub share: f64,
}
#[derive(Serialize)]
pub struct Category {
    pub field: String,
    pub present: usize,
    pub distinct: usize,
    pub dominant: ValueCount,
    pub rare: Vec<ValueCount>,
}
#[derive(Clone, Serialize)]
pub struct Item {
    pub field: String,
    pub value: String,
}
#[derive(Serialize)]
pub struct Association {
    pub left: Item,
    pub right: Item,
    pub count: usize,
    pub support: f64,
    pub confidence: f64,
    pub lift: f64,
}
#[derive(Serialize)]
pub struct Example {
    pub event_id: usize,
    pub event_ref: String,
    pub value: f64,
}
#[derive(Serialize)]
pub struct Outlier {
    pub field: String,
    pub unit: String,
    pub count: usize,
    pub median: f64,
    pub mad: f64,
    pub lower: f64,
    pub upper: f64,
    pub outlier_count: usize,
    pub low_count: usize,
    pub high_count: usize,
    pub min: f64,
    pub max: f64,
    pub examples: Vec<Example>,
}
#[derive(Serialize)]
pub struct Template {
    pub pattern: String,
    pub count: usize,
    pub share: f64,
    pub errors: usize,
    pub event_id: usize,
    pub event_ref: String,
}

/// Algorithm R with a fixed seed: reproducible, spread through the full input,
/// and independent of event IDs (case snapshots may reuse IDs).
pub struct Sampler {
    pub ids: Vec<usize>,
    seen: usize,
    random: u64,
}
impl Sampler {
    pub fn new() -> Self {
        Self {
            ids: Vec::with_capacity(SAMPLE_CAP),
            seen: 0,
            random: 0x9e3779b97f4a7c15,
        }
    }
    pub fn push(&mut self, id: usize) {
        self.seen += 1;
        if self.ids.len() < SAMPLE_CAP {
            self.ids.push(id);
            return;
        }
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        let slot = (self.random % self.seen as u64) as usize;
        if slot < SAMPLE_CAP {
            self.ids[slot] = id;
        }
    }
}

impl Discovery {
    pub fn observe(&mut self, timestamp: Option<i64>, error: bool, warning: bool) {
        self.total += 1;
        self.errors += usize::from(error);
        self.warnings += usize::from(warning);
        if let Some(t) = timestamp {
            self.start = Some(self.start.map_or(t, |old| old.min(t)));
            self.end = Some(self.end.map_or(t, |old| old.max(t)));
        } else {
            self.missing_time += 1;
        }
    }
}

fn eligible(field: &str) -> bool {
    !matches!(
        field,
        "id" | "timestamp" | "raw" | "message" | "description" | "arquivo" | "caminho"
    )
}
fn categorical_name(field: &str) -> bool {
    let f = field.to_lowercase();
    matches!(
        f.as_str(),
        "source" | "level" | "code" | "status" | "port" | "pid"
    ) || f.ends_with("_id")
        || f.ends_with(".id")
        || f.ends_with("code")
        || f.ends_with("status")
        || f.ends_with("port")
}
fn quantile(sorted: &[f64], fraction: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

fn near_duplicate_values(left: &[String], right: &[String]) -> bool {
    let mut overlap = 0usize;
    let mut identical = 0usize;
    for (a, b) in left.iter().zip(right) {
        if a.trim().is_empty() || b.trim().is_empty() {
            continue;
        }
        overlap += 1;
        identical += usize::from(a.eq_ignore_ascii_case(b));
    }
    overlap > 0 && (identical == overlap || (overlap >= 20 && identical * 100 >= overlap * 95))
}

pub fn analyze<F, I>(mut result: Discovery, events: F) -> Discovery
where
    F: Fn() -> I,
    I: Iterator<Item = Event>,
{
    let mut field_counts = BTreeMap::<String, usize>::new();
    let mut templates = BTreeMap::<String, Template>::new();
    for ev in events().take(SAMPLE_CAP) {
        if crate::operations::cancelled() {
            break;
        }
        result.sample_count += 1;
        for field in ["source", "level", "code", "name"]
            .into_iter()
            .chain(ev.fields.keys().map(String::as_str))
        {
            if !eligible(field) {
                continue;
            }
            if field.len() > 160 || (!field_counts.contains_key(field) && field_counts.len() >= 128)
            {
                result.fields_limited = true;
                continue;
            }
            if ev
                .col_ref(field)
                .is_some_and(|v| !v.trim().is_empty() && v.len() <= VALUE_BYTES)
            {
                *field_counts.entry(field.to_owned()).or_default() += 1;
            }
        }
        if !ev.message.trim().is_empty() {
            let pattern = insights::pattern_of(&ev.message);
            let entry = templates.entry(pattern.clone()).or_insert(Template {
                pattern,
                count: 0,
                share: 0.0,
                errors: 0,
                event_id: ev.id,
                event_ref: ev.event_ref.clone(),
            });
            entry.count += 1;
            entry.errors += usize::from(insights::is_error(&ev));
        }
    }
    let mut fields: Vec<_> = field_counts.into_iter().collect();
    fields.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    result.fields_limited |= fields.len() > FIELD_CAP;
    fields.truncate(FIELD_CAP);
    result.fields_considered = fields.into_iter().map(|(field, _)| field).collect();
    let mut values =
        vec![Vec::<String>::with_capacity(result.sample_count); result.fields_considered.len()];
    let mut identities = Vec::with_capacity(result.sample_count);
    let mut temporal_records = Vec::with_capacity(result.sample_count);
    for ev in events().take(result.sample_count) {
        if crate::operations::cancelled() {
            break;
        }
        for (i, field) in result.fields_considered.iter().enumerate() {
            let v = ev
                .col_ref(field)
                .filter(|v| v.len() <= VALUE_BYTES)
                .map(|v| v.into_owned())
                .unwrap_or_default();
            values[i].push(v);
        }
        temporal_records.push(temporal::Record {
            timestamp: ev.timestamp,
            level: ev.level.clone(),
            code: ev.code.clone(),
            pattern: insights::pattern_of(&ev.message),
            event_id: ev.id,
            event_ref: ev.event_ref.clone(),
        });
        identities.push((ev.id, ev.event_ref));
    }
    temporal::analyze(&mut result, &values, &temporal_records);
    let mut association_fields: Vec<CandidateField> = Vec::new();
    for (column, field) in result.fields_considered.iter().enumerate() {
        let vals = &values[column];
        let mut counts = BTreeMap::<String, usize>::new();
        let mut numeric = Vec::new();
        let mut units = [0usize; 5];
        for (i, val) in vals
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.trim().is_empty())
        {
            // Same case semantics as an "equals" filter. Keep original label.
            *counts.entry(val.to_ascii_lowercase()).or_default() += 1;
            if let Some((n, unit)) = parse_num_unit(val).filter(|(n, _)| n.is_finite()) {
                numeric.push((i, n, unit));
                units[unit as usize] += 1;
            }
        }
        let present: usize = counts.values().sum();
        if present < 5 {
            continue;
        }
        let (unit, unit_count) = units
            .iter()
            .enumerate()
            .max_by_key(|(i, c)| (**c, std::cmp::Reverse(*i)))
            .unwrap();
        let is_numeric = !categorical_name(field)
            && *unit_count >= 20
            && *unit_count == numeric.len()
            && *unit_count as f64 / present as f64 >= 0.9;
        if is_numeric {
            numeric.retain(|(_, _, u)| *u as usize == unit);
            let mut sorted: Vec<f64> = numeric.iter().map(|(_, n, _)| *n).collect();
            sorted.sort_by(f64::total_cmp);
            let median = quantile(&sorted, 0.5);
            let mut distances: Vec<f64> = sorted.iter().map(|n| (n - median).abs()).collect();
            distances.sort_by(f64::total_cmp);
            let mad = quantile(&distances, 0.5);
            // MAD is robust to rare extremes; IQR avoids flagging a second broad mode.
            let radius =
                (mad * 1.4826 * 3.5).max((quantile(&sorted, 0.75) - quantile(&sorted, 0.25)) * 3.0);
            let lower = median - radius;
            let upper = median + radius;
            if !lower.is_finite() || !upper.is_finite() {
                continue;
            }
            let low_count = sorted.iter().filter(|n| **n < lower).count();
            let high_count = sorted.iter().filter(|n| **n > upper).count();
            let outlier_count = low_count + high_count;
            if outlier_count > 0 && outlier_count * 5 <= sorted.len() {
                let mut unusual: Vec<_> = numeric
                    .iter()
                    .filter(|(_, n, _)| *n < lower || *n > upper)
                    .collect();
                unusual.sort_by(|a, b| {
                    (b.1 - median)
                        .abs()
                        .total_cmp(&(a.1 - median).abs())
                        .then(a.0.cmp(&b.0))
                });
                result.outliers.push(Outlier {
                    field: field.clone(),
                    unit: match unit {
                        x if x == UnitKind::Bytes as usize => "bytes",
                        x if x == UnitKind::Bits as usize => "bits/s",
                        x if x == UnitKind::DurationMs as usize => "ms",
                        _ => "número",
                    }
                    .into(),
                    count: sorted.len(),
                    median,
                    mad,
                    lower,
                    upper,
                    outlier_count,
                    low_count,
                    high_count,
                    min: sorted[0],
                    max: *sorted.last().unwrap(),
                    examples: unusual
                        .into_iter()
                        .take(3)
                        .map(|(i, n, _)| Example {
                            event_id: identities[*i].0,
                            event_ref: identities[*i].1.clone(),
                            value: *n,
                        })
                        .collect(),
                });
            }
            continue;
        }
        if counts.len() < 2 || counts.len() > 64 || counts.len() * 2 > present {
            continue;
        }
        let labels: HashMap<_, _> = vals
            .iter()
            .map(|v| (v.to_ascii_lowercase(), v.clone()))
            .collect();
        let mut ranked: Vec<_> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let make = |(v, n): &(String, usize)| ValueCount {
            value: labels[v].clone(),
            count: *n,
            share: *n as f64 / present as f64,
        };
        let rare = ranked
            .iter()
            .rev()
            .filter(|(_, n)| *n <= (present / 50).max(2))
            .take(5)
            .map(make)
            .collect();
        result.categories.push(Category {
            field: field.clone(),
            present,
            distinct: ranked.len(),
            dominant: make(&ranked[0]),
            rare,
        });
        if ranked.len() <= 32
            && association_fields.len() < 12
            && !association_fields
                .iter()
                .any(|(other, _, _)| near_duplicate_values(&values[column], &values[*other]))
        {
            association_fields.push((column, ranked, labels));
        }
    }
    let denominator = result.sample_count.max(1) as f64;
    for a in 0..association_fields.len() {
        for b in a + 1..association_fields.len() {
            let (ac, a_counts, a_labels) = &association_fields[a];
            let (bc, b_counts, b_labels) = &association_fields[b];
            // Normalized aliases are useful for browsing but not discoveries.
            let a_name = &result.fields_considered[*ac];
            let b_name = &result.fields_considered[*bc];
            if a_name.starts_with(&format!("{b_name}."))
                || b_name.starts_with(&format!("{a_name}."))
            {
                continue;
            }
            let mut pairs = BTreeMap::<(String, String), usize>::new();
            for (av, bv) in values[*ac].iter().zip(&values[*bc]) {
                if av.trim().is_empty() || bv.trim().is_empty() {
                    continue;
                }
                *pairs
                    .entry((av.to_ascii_lowercase(), bv.to_ascii_lowercase()))
                    .or_default() += 1;
            }
            // Suppress near-duplicate fields too: >=95% identical among at
            // least 20 co-present rows. A few missing/different values must not
            // turn a normalized alias into a high-lift "discovery".
            if near_duplicate_values(&values[*ac], &values[*bc]) {
                continue;
            }
            for ((av, bv), count) in pairs {
                if count < 5 || count as f64 / denominator < 0.005 {
                    continue;
                }
                let an = a_counts.iter().find(|(v, _)| v == &av).unwrap().1;
                let bn = b_counts.iter().find(|(v, _)| v == &bv).unwrap().1;
                let lift = count as f64 * denominator / (an * bn) as f64;
                // Use the more specific side as the premise; one card per pair.
                let confidence = count as f64 / an.min(bn) as f64;
                if lift < 1.5 || confidence < 0.6 {
                    continue;
                }
                let left = Item {
                    field: a_name.clone(),
                    value: a_labels[&av].clone(),
                };
                let right = Item {
                    field: b_name.clone(),
                    value: b_labels[&bv].clone(),
                };
                let (left, right) = if an <= bn {
                    (left, right)
                } else {
                    (right, left)
                };
                result.associations.push(Association {
                    left,
                    right,
                    count,
                    support: count as f64 / denominator,
                    confidence,
                    lift,
                });
            }
        }
    }
    result.associations.sort_by(|a, b| {
        (b.lift * (b.count as f64).sqrt())
            .total_cmp(&(a.lift * (a.count as f64).sqrt()))
            .then(a.left.field.cmp(&b.left.field))
            .then(a.left.value.cmp(&b.left.value))
            .then(a.right.field.cmp(&b.right.field))
            .then(a.right.value.cmp(&b.right.value))
    });
    // Keep the ranking useful when one pair of fields has dozens of values.
    let mut per_pair = HashMap::<(String, String), usize>::new();
    result.associations.retain(|association| {
        let mut pair = [
            association.left.field.clone(),
            association.right.field.clone(),
        ];
        pair.sort();
        let count = per_pair
            .entry((pair[0].clone(), pair[1].clone()))
            .or_default();
        *count += 1;
        *count <= 2
    });
    result.associations.truncate(12);
    result.categories.sort_by(|a, b| {
        b.rare
            .len()
            .cmp(&a.rare.len())
            .then(b.dominant.share.total_cmp(&a.dominant.share))
            .then(a.field.cmp(&b.field))
    });
    result.outliers.sort_by(|a, b| {
        b.outlier_count
            .cmp(&a.outlier_count)
            .then(a.field.cmp(&b.field))
    });
    result.outliers.truncate(8);
    result.templates = templates.into_values().filter(|t| t.count >= 2).collect();
    result
        .templates
        .sort_by(|a, b| b.count.cmp(&a.count).then(a.pattern.cmp(&b.pattern)));
    result.templates.truncate(20);
    for t in &mut result.templates {
        t.share = t.count as f64 / denominator;
    }
    result.limited = result.total > result.sample_count;
    result.complete = !crate::operations::cancelled();
    result
}
