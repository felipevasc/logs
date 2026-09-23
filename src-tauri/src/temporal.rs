//! Explainable changes inside time windows, using only the bounded discovery sample.
use super::{near_duplicate_values, Discovery, Item};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Serialize)]
pub struct Shift {
    pub kind: String,
    pub context: Vec<Item>,
    pub outcome_field: String,
    pub outcome_op: String,
    pub expected: String,
    pub observed: String,
    pub baseline_count: usize,
    pub baseline_expected: usize,
    pub baseline_observed: usize,
    pub window_count: usize,
    pub window_expected: usize,
    pub window_observed: usize,
    pub expected_share: f64,
    pub observed_share: f64,
    pub baseline_observed_share: f64,
    pub delta: f64,
    pub start: i64,
    pub end: i64,
    pub score: f64,
    pub event_id: usize,
    pub event_ref: String,
}
pub struct Record {
    pub timestamp: Option<i64>,
    pub level: String,
    pub code: String,
    pub pattern: String,
    pub event_id: usize,
    pub event_ref: String,
}
struct Column {
    field: String,
    labels: Vec<String>,
    rows: Vec<Option<usize>>,
    present: Vec<bool>,
    truncated: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Context {
    fields: [usize; 3],
    values: [usize; 3],
    len: usize,
}
fn encode(field: String, values: impl Iterator<Item = String>, max: usize) -> Column {
    let raw: Vec<_> = values.collect();
    let normalized = |v: &str| v.to_string();
    let mut frequencies = BTreeMap::<String, usize>::new();
    let mut displays = BTreeMap::new();
    for v in raw.iter().filter(|v| !v.trim().is_empty()) {
        let key = normalized(v);
        displays.entry(key.clone()).or_insert_with(|| v.clone());
        *frequencies.entry(key).or_default() += 1;
    }
    let mut ranked: Vec<_> = frequencies.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let truncated = ranked.len() > max;
    ranked.truncate(max);
    let labels: Vec<String> = ranked.iter().map(|(v, _)| displays[v].clone()).collect();
    let lookup: HashMap<_, _> = ranked
        .iter()
        .enumerate()
        .map(|(i, (v, _))| (v.as_str(), i))
        .collect();
    let rows = raw
        .iter()
        .map(|v| lookup.get(normalized(v).as_str()).copied())
        .collect();
    Column {
        field,
        labels,
        rows,
        present: raw.iter().map(|value| !value.trim().is_empty()).collect(),
        truncated,
    }
}
fn context_field(field: &str) -> bool {
    let f = field.to_ascii_lowercase();
    !matches!(
        f.as_str(),
        "code" | "level" | "name" | "id" | "message" | "raw"
    ) && !f.ends_with(".code")
        && !f.ends_with(".level")
        && !f.contains("severity")
        && !f.ends_with("_id")
        && !f.ends_with(".id")
        && !f.contains("traceid")
        && !f.contains("requestid")
}
fn subset(a: &[Item], b: &[Item]) -> bool {
    a.iter().all(|item| {
        b.iter()
            .any(|other| item.field == other.field && item.value == other.value)
    })
}
fn finish(mut shifts: Vec<Shift>) -> Vec<Shift> {
    let specificity = |field: &str| match field {
        "message" => 0,
        "code" => 1,
        _ => 2,
    };
    shifts.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.context.len().cmp(&b.context.len()))
            .then(a.start.cmp(&b.start))
            .then(specificity(&a.outcome_field).cmp(&specificity(&b.outcome_field)))
            .then(a.outcome_field.cmp(&b.outcome_field))
            .then(a.observed.cmp(&b.observed))
    });
    let mut selected: Vec<Shift> = Vec::new();
    for shift in shifts {
        if selected.iter().any(|old| {
            ((old.outcome_field == shift.outcome_field && old.observed == shift.observed)
                || (!old.event_ref.is_empty()
                    && old.event_ref == shift.event_ref
                    && old.window_observed == shift.window_observed
                    && old.baseline_observed == shift.baseline_observed))
                && old.start <= shift.end.saturating_add(1)
                && shift.start <= old.end.saturating_add(1)
                && (subset(&old.context, &shift.context) || subset(&shift.context, &old.context))
        }) {
            continue;
        }
        selected.push(shift);
        if selected.len() == 8 {
            break;
        }
    }
    selected
}

pub fn analyze(result: &mut Discovery, values: &[Vec<String>], records: &[Record]) {
    result.timed_sample_count = records.iter().filter(|r| r.timestamp.is_some()).count();
    if result.timed_sample_count < 24 {
        return;
    }
    let min = records.iter().filter_map(|r| r.timestamp).min().unwrap();
    let max = records.iter().filter_map(|r| r.timestamp).max().unwrap();
    let span = max.saturating_sub(min).saturating_add(1);
    let width = (span / 12 + i64::from(span % 12 != 0)).max(1);
    let bins = ((span - 1) / width + 1) as usize;
    result.time_bins = bins;
    if bins < 2 {
        return;
    }
    let time_rows: Vec<_> = records
        .iter()
        .map(|r| {
            r.timestamp
                .map(|t| (t.saturating_sub(min) / width) as usize)
        })
        .collect();
    let outcomes = [
        encode("level".into(), records.iter().map(|r| r.level.clone()), 64),
        encode(
            "code".into(),
            records.iter().map(|r| {
                if r.code.trim() == r.code {
                    r.code.clone()
                } else {
                    String::new()
                }
            }),
            64,
        ),
        encode(
            "message".into(),
            records.iter().map(|r| r.pattern.clone()),
            64,
        ),
    ];
    result.temporal_limited |= outcomes.iter().any(|outcome| outcome.truncated);
    let mut context_columns = Vec::<Column>::new();
    let mut context_indices = Vec::<usize>::new();
    for (i, field) in result.fields_considered.iter().enumerate() {
        if !context_field(field)
            || context_indices
                .iter()
                .any(|&other| near_duplicate_values(&values[i], &values[other]))
        {
            continue;
        }
        let mut counts = BTreeMap::<String, usize>::new();
        for value in values[i]
            .iter()
            .filter(|v| !v.is_empty() && v.trim() == v.as_str())
        {
            *counts.entry(value.clone()).or_default() += 1;
        }
        if counts.len() < 2 || counts.len() > 12 || counts.values().sum::<usize>() < 40 {
            continue;
        }
        let numeric: usize = counts
            .iter()
            .filter(|(v, _)| crate::analysis::parse_num_unit(v).is_some())
            .map(|(_, n)| *n)
            .sum();
        if !super::categorical_name(field) && numeric * 5 >= counts.values().sum::<usize>() * 4 {
            continue;
        }
        if context_columns.len() == 6 {
            result.temporal_limited = true;
            break;
        }
        context_columns.push(encode(
            field.clone(),
            values[i].iter().map(|v| {
                if v.trim() == v {
                    v.clone()
                } else {
                    String::new()
                }
            }),
            12,
        ));
        context_indices.push(i);
    }
    let mut support = HashMap::<Context, usize>::new();
    for row in 0..records.len() {
        if time_rows[row].is_none() {
            continue;
        }
        for a in 0..context_columns.len() {
            let Some(av) = context_columns[a].rows[row] else {
                continue;
            };
            for b in a..context_columns.len() {
                let Some(bv) = context_columns[b].rows[row] else {
                    continue;
                };
                for c in b..context_columns.len() {
                    if a == b && b != c {
                        continue;
                    }
                    let Some(cv) = context_columns[c].rows[row] else {
                        continue;
                    };
                    let len = if a == b {
                        1
                    } else if b == c {
                        2
                    } else {
                        3
                    };
                    let key = Context {
                        fields: [a, b, c],
                        values: [av, bv, cv],
                        len,
                    };
                    if support.len() < 32_768 || support.contains_key(&key) {
                        *support.entry(key).or_default() += 1;
                    } else {
                        result.temporal_limited = true;
                    }
                }
            }
        }
    }
    let mut contexts: Vec<_> = support.into_iter().filter(|(_, n)| *n >= 24).collect();
    contexts.sort_by(|(a, an), (b, bn)| bn.cmp(an).then(a.cmp(b)));
    if contexts.len() > 256 {
        result.temporal_limited = true;
        contexts.truncate(256);
    }
    let mut conditional = Vec::new();
    let mut global = Vec::new();
    for context in std::iter::once(None).chain(contexts.into_iter().map(|(c, _)| Some(c))) {
        if crate::operations::cancelled() {
            break;
        }
        let rows: Vec<usize> = (0..records.len())
            .filter(|&row| {
                time_rows[row].is_some()
                    && context.is_none_or(|c| {
                        (0..c.len)
                            .all(|j| context_columns[c.fields[j]].rows[row] == Some(c.values[j]))
                    })
            })
            .collect();
        let labels: Vec<Item> = context
            .map(|c| {
                (0..c.len)
                    .map(|j| Item {
                        field: context_columns[c.fields[j]].field.clone(),
                        value: context_columns[c.fields[j]].labels[c.values[j]].clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        for outcome in &outcomes {
            if outcome.labels.len() < 2 {
                continue;
            }
            let mut all = vec![0usize; outcome.labels.len()];
            let mut windows = vec![vec![0usize; outcome.labels.len()]; bins];
            let mut window_totals = vec![0usize; bins];
            for &row in &rows {
                // Outcomes outside the top labels still belong in denominators;
                // otherwise truncation could invent an 80%-dominant baseline.
                if outcome.present[row] {
                    window_totals[time_rows[row].unwrap()] += 1;
                }
                if let Some(value) = outcome.rows[row] {
                    all[value] += 1;
                    windows[time_rows[row].unwrap()][value] += 1;
                }
            }
            let total: usize = window_totals.iter().sum();
            for (bin, window) in windows.iter().enumerate() {
                let window_count = window_totals[bin];
                let baseline_count = total - window_count;
                if baseline_count < 20 || window_count < 3 {
                    continue;
                }
                let expected = (0..all.len())
                    .max_by_key(|&i| (all[i] - window[i], std::cmp::Reverse(i)))
                    .unwrap();
                let baseline_expected = all[expected] - window[expected];
                let expected_share = baseline_expected as f64 / baseline_count as f64;
                if context.is_some() && expected_share < 0.8 {
                    continue;
                }
                for observed in 0..all.len() {
                    if observed == expected || window[observed] < 3 {
                        continue;
                    }
                    let baseline_observed = all[observed] - window[observed];
                    let observed_share = window[observed] as f64 / window_count as f64;
                    let baseline_observed_share = baseline_observed as f64 / baseline_count as f64;
                    let delta = observed_share - baseline_observed_share;
                    let new_pattern = context.is_none()
                        && outcome.field == "message"
                        && baseline_observed == 0
                        && observed_share >= 0.15;
                    if delta < 0.35 && !new_pattern {
                        continue;
                    }
                    let example = rows
                        .iter()
                        .copied()
                        .find(|&r| time_rows[r] == Some(bin) && outcome.rows[r] == Some(observed))
                        .unwrap();
                    let shift = Shift {
                        kind: if context.is_some() {
                            "behavior_shift"
                        } else if new_pattern {
                            "new_pattern"
                        } else {
                            "distribution_shift"
                        }
                        .into(),
                        context: labels.clone(),
                        outcome_field: outcome.field.clone(),
                        outcome_op: if outcome.field == "message" {
                            "pattern"
                        } else {
                            "equals_exact"
                        }
                        .into(),
                        expected: outcome.labels[expected].clone(),
                        observed: outcome.labels[observed].clone(),
                        baseline_count,
                        baseline_expected,
                        baseline_observed,
                        window_count,
                        window_expected: window[expected],
                        window_observed: window[observed],
                        expected_share,
                        observed_share,
                        baseline_observed_share,
                        delta,
                        start: min.saturating_add((bin as i64).saturating_mul(width)),
                        end: min
                            .saturating_add(((bin + 1) as i64).saturating_mul(width))
                            .saturating_sub(1)
                            .min(max),
                        score: delta
                            * (window[observed] as f64).sqrt()
                            * if context.is_some() {
                                expected_share
                            } else {
                                1.0
                            },
                        event_id: records[example].event_id,
                        event_ref: records[example].event_ref.clone(),
                    };
                    if context.is_some() {
                        conditional.push(shift);
                    } else {
                        global.push(shift);
                    }
                }
            }
        }
    }
    result.behavior_shifts = finish(conditional);
    result.changes = finish(global);
}
