//! Test-only reference frozen before temporal preflight fusion.
//! Extracted from output/cautious-baseline/analysis.rs. Keep the complete body
//! so paired benchmarks compare the prior generic implementation, not a
//! specialized count shortcut with a different workload or optimizer context.
use super::*;
pub(super) fn compute_series_stream<F, I>(events: F, spec: &SeriesSpec) -> SeriesResult
where
    F: Fn() -> I,
    I: Iterator<Item = Event>,
{
    let limit = spec.limit.unwrap_or(10).min(500);
    let field = spec.field.as_deref();
    if spec.chart == "terms" && spec.metric == "count" {
        let key = field.unwrap_or("level");
        let mut counts = crate::distinct::Terms::default();
        for ev in events() {
            if crate::operations::cancelled() {
                break;
            }
            counts.insert(
                serde_json::to_string(&ev.col_str(key).filter(|s| !s.trim().is_empty())).unwrap(),
            );
        }
        let top: Vec<(Option<String>, usize)> = counts
            .top(limit)
            .into_iter()
            .map(|(key, count)| (serde_json::from_str(&key).unwrap(), count))
            .collect();
        return SeriesResult {
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
        };
    }
    let unit = match (spec.unit.as_deref(), field) {
        (Some(u), _) if u != "auto" => u.to_string(),
        (Some(_), None) | (None, None) => "number".to_string(),
        (_, Some(f)) => unit_name(dominant_unit(events(), f, 500)),
    };
    let expected_unit = if matches!(spec.metric.as_str(), "count" | "distinct") {
        None
    } else {
        match unit.as_str() {
            "number" => Some(UnitKind::Number),
            "bytes" => Some(UnitKind::Bytes),
            "bits" => Some(UnitKind::Bits),
            "duration" => Some(UnitKind::DurationMs),
            _ => None,
        }
    };
    let mut incompatible_units = 0;

    // splits: top N valores do campo de split
    let splits: Vec<String> = match &spec.split {
        Some(col) => {
            let mut counts = crate::distinct::Terms::default();
            for ev in events() {
                if crate::operations::cancelled() {
                    break;
                }
                if let Some(v) = ev.col_str(col) {
                    if !v.is_empty() {
                        counts.insert(v);
                    }
                }
            }
            counts.top(6).into_iter().map(|(k, _)| k).collect()
        }
        None => vec![],
    };
    let split_names: Vec<String> = if splits.is_empty() {
        vec![spec.field.clone().unwrap_or_else(|| "eventos".into())]
    } else {
        splits.clone()
    };

    if spec.chart == "terms" {
        // ranking de valores de `field_key` (ou da métrica se count)
        let key_field = spec.field.clone().unwrap_or_else(|| "level".into());
        let metric_field = if spec.metric == "count" || spec.metric == "distinct" {
            spec.field.as_deref()
        } else {
            field
        };
        let mut accs: HashMap<Option<String>, MetricAcc> = HashMap::new();
        for ev in events() {
            if crate::operations::cancelled() {
                break;
            }
            let key = ev.col_str(&key_field).filter(|s| !s.trim().is_empty());
            incompatible_units += usize::from(
                accs.entry(key)
                    .or_insert_with(|| MetricAcc::new(&spec.metric))
                    .push_checked(
                        &ev,
                        metric_field.map(|_| field.unwrap_or(&key_field)),
                        expected_unit,
                    ),
            );
        }
        let mut items: Vec<(Option<String>, f64, usize)> = accs
            .iter()
            .map(|(k, a)| (k.clone(), a.value(), a.n as usize))
            .collect();
        items.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items.truncate(limit);
        return SeriesResult {
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
        };
    }

    // série temporal
    let bounds = events()
        .take_while(|_| !crate::operations::cancelled())
        .filter_map(|ev| ev.timestamp)
        .fold(None, |acc: Option<(i64, i64)>, t| {
            Some(acc.map(|(a, b)| (a.min(t), b.max(t))).unwrap_or((t, t)))
        });
    if bounds.is_none() {
        return SeriesResult {
            kind: "time".into(),
            unit,
            interval_ms: 0,
            x: vec![],
            x_values: vec![],
            series: vec![],
            incompatible_units: 0,
        };
    }
    let (tmin, tmax) = bounds.unwrap();
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

    let mut accs: Vec<HashMap<String, MetricAcc>> =
        (0..n_buckets).map(|_| HashMap::new()).collect();
    for ev in events() {
        if crate::operations::cancelled() {
            break;
        }
        let Some(t) = ev.timestamp else { continue };
        let b = (t.saturating_sub(tmin) / interval) as usize;
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
        incompatible_units += usize::from(
            accs[b]
                .entry(name)
                .or_insert_with(|| MetricAcc::new(&spec.metric))
                .push_checked(&ev, field, expected_unit),
        );
    }

    SeriesResult {
        kind: "time".into(),
        unit,
        interval_ms: interval,
        x: (0..n_buckets)
            .map(|b| Value::from(tmin.saturating_add((b as i64).saturating_mul(interval))))
            .collect(),
        x_values: vec![],
        series: split_names
            .iter()
            .map(|name| SeriesData {
                name: name.clone(),
                samples: accs
                    .iter()
                    .map(|m| m.get(name).map(|a| a.n as usize).unwrap_or(0))
                    .collect(),
                points: accs
                    .iter()
                    .map(|m| m.get(name).map(|a| a.value()).unwrap_or(0.0))
                    .collect(),
            })
            .collect(),
        incompatible_units,
    }
}

