use crate::model::Event;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Serialize)]
pub struct Pattern {
    pub pattern: String,
    pub count: usize,
    pub errors: usize,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub example: Event,
}
#[derive(Clone, Serialize)]
pub struct Bucket {
    pub timestamp: i64,
    pub count: usize,
    pub errors: usize,
}
#[derive(Clone, Serialize)]
pub struct Finding {
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub event_id: Option<usize>,
}
#[derive(Clone, Serialize)]
pub struct Overview {
    pub total: usize,
    pub errors: usize,
    pub warnings: usize,
    pub undated: usize,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub buckets: Vec<Bucket>,
    pub levels: BTreeMap<String, usize>,
    pub sources: Vec<(String, usize)>,
    pub sources_other: usize,
    pub patterns: Vec<Pattern>,
    pub patterns_limited: bool,
    pub findings: Vec<Finding>,
    pub complete: bool,
    pub latency: Option<Latency>,
}
#[derive(Clone, Serialize)]
pub struct Latency {
    pub field: String,
    pub unit: String,
    pub incompatible: usize,
    pub count: usize,
    pub sampled: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

pub fn pattern_of(message: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re=RE.get_or_init(|| regex::Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f-]{27,}\b|\b(?:\d{1,3}\.){3}\d{1,3}\b|\b0x[0-9a-f]+\b|\b\d+(?:[.,]\d+)?\b").unwrap());
    let first = message.lines().next().unwrap_or(message);
    re.replace_all(first, "‹…›").chars().take(400).collect()
}
pub fn is_error(ev: &Event) -> bool {
    matches!(ev.level.as_str(), "Erro" | "Crítico")
}

pub fn overview<F, I>(events: F) -> Overview
where
    F: Fn() -> I,
    I: Iterator<Item = Event>,
{
    let mut out = Overview {
        total: 0,
        errors: 0,
        warnings: 0,
        undated: 0,
        start: None,
        end: None,
        buckets: vec![],
        levels: BTreeMap::new(),
        sources: vec![],
        sources_other: 0,
        patterns: vec![],
        patterns_limited: false,
        findings: vec![],
        complete: true,
        latency: None,
    };
    let mut patterns: HashMap<String, Pattern> = HashMap::new();
    let mut sources: HashMap<String, usize> = HashMap::new();
    let mut latency = Vec::<f64>::new();
    let mut latency_count = 0;
    let mut latency_field = String::new();
    let mut latency_unit = String::new();
    let mut incompatible = 0;
    let mut random = 0x9e3779b97f4a7c15u64;
    for mut ev in events() {
        if crate::operations::cancelled() {
            out.complete = false;
            break;
        }
        out.total += 1;
        let error = is_error(&ev);
        out.errors += usize::from(error);
        out.warnings += usize::from(ev.level == "Aviso");
        *out.levels.entry(ev.level.clone()).or_default() += 1;
        let src = if ev.source.is_empty() {
            "Sem origem"
        } else {
            &ev.source
        };
        if sources.len() < 10000 || sources.contains_key(src) {
            *sources.entry(src.into()).or_default() += 1;
        }
        if let Some(t) = ev.timestamp {
            out.start = Some(out.start.map(|v| v.min(t)).unwrap_or(t));
            out.end = Some(out.end.map(|v| v.max(t)).unwrap_or(t));
        } else {
            out.undated += 1;
        }
        let pattern = pattern_of(&ev.message);
        if let Some(p) = patterns.get_mut(&pattern) {
            p.count += 1;
            p.errors += usize::from(error);
            if let Some(t) = ev.timestamp {
                p.first = Some(p.first.map(|v| v.min(t)).unwrap_or(t));
                p.last = Some(p.last.map(|v| v.max(t)).unwrap_or(t));
            }
        } else if patterns.len() < 20_000 {
            ev.raw.clear();
            let mut example = ev.clone();
            example.fields.clear();
            example.message = example.message.chars().take(500).collect();
            patterns.insert(
                pattern.clone(),
                Pattern {
                    pattern,
                    count: 1,
                    errors: usize::from(error),
                    first: ev.timestamp,
                    last: ev.timestamp,
                    example,
                },
            );
        } else {
            out.patterns_limited = true;
        }
        // Field-specific units: only combine values from one selected latency field.
        let candidate = if latency_field.is_empty() {
            [
                "latency_ms",
                "duration_ms",
                "latencia",
                "latency",
                "duration",
            ]
            .into_iter()
            .find(|k| ev.col_num(k).is_some())
        } else {
            Some(latency_field.as_str())
        };
        if let Some(field) = candidate {
            if let Some(n) = ev.col_num(field).filter(|n| n.is_finite() && *n >= 0.0) {
                let parsed_unit = ev
                    .col_str(field)
                    .and_then(|v| crate::analysis::parse_num_unit(&v))
                    .map(|(_, unit)| unit);
                let unit = if field.ends_with("_ms")
                    || parsed_unit == Some(crate::analysis::UnitKind::DurationMs)
                {
                    "ms"
                } else {
                    "unidade da fonte"
                };
                if latency_field.is_empty() {
                    latency_field = field.to_string();
                    latency_unit = unit.into();
                }
                if unit != latency_unit {
                    incompatible += 1;
                    continue;
                }
                latency_count += 1;
                if latency.len() < 10000 {
                    latency.push(n);
                } else {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    let j = (random % latency_count as u64) as usize;
                    if j < latency.len() {
                        latency[j] = n;
                    }
                }
            }
        }
    }
    if let (Some(start), Some(end)) = (out.start, out.end) {
        let width = ((end.saturating_sub(start)) / 90 + 1).max(1000);
        out.buckets = (0..=((end - start) / width))
            .map(|i| Bucket {
                timestamp: start + i * width,
                count: 0,
                errors: 0,
            })
            .collect();
        for ev in events() {
            if crate::operations::cancelled() {
                out.complete = false;
                break;
            }
            if let Some(t) = ev.timestamp {
                if let Some(b) = out.buckets.get_mut(((t - start) / width) as usize) {
                    b.count += 1;
                    b.errors += usize::from(is_error(&ev));
                }
            }
        }
        let baseline = out.errors as f64 / out.total.max(1) as f64;
        if let Some(b) = out
            .buckets
            .iter()
            .filter(|b| b.count >= 20 && b.errors >= 5)
            .max_by(|a, b| {
                (a.errors as f64 / a.count as f64).total_cmp(&(b.errors as f64 / b.count as f64))
            })
        {
            let rate = b.errors as f64 / b.count as f64;
            if rate > baseline * 2.0 && rate - baseline > 0.05 {
                out.findings.push(Finding {
                    kind: "spike".into(),
                    title: "Concentração de erros".into(),
                    detail: format!(
                        "{:.1}% neste intervalo; {:.1}% no período selecionado. {} de {} eventos.",
                        rate * 100.0,
                        baseline * 100.0,
                        b.errors,
                        b.count
                    ),
                    start: Some(b.timestamp),
                    end: Some(b.timestamp + width - 1),
                    event_id: None,
                });
            }
        }
        if let Some(b) = out
            .buckets
            .iter()
            .skip(1)
            .take(out.buckets.len().saturating_sub(2))
            .find(|b| b.count == 0)
        {
            out.findings.push(Finding{kind:"gap".into(),title:"Intervalo sem registros".into(),detail:"Confira a cobertura e a rotação dos arquivos antes de concluir que houve indisponibilidade.".into(),start:Some(b.timestamp),end:Some(b.timestamp+width-1),event_id:None});
        }
    }
    out.sources = sources.into_iter().collect();
    out.sources
        .sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.sources.truncate(20);
    out.sources_other = out.total - out.sources.iter().map(|(_, count)| count).sum::<usize>();
    out.patterns = patterns.into_values().collect();
    out.patterns
        .sort_by(|a, b| b.count.cmp(&a.count).then(a.pattern.cmp(&b.pattern)));
    if let Some(p) = out
        .patterns
        .iter()
        .filter(|p| p.errors > 0)
        .max_by_key(|p| p.errors)
    {
        out.findings.push(Finding {
            kind: "pattern".into(),
            title: "Falha recorrente".into(),
            detail: format!("{} ocorrências · {}", p.errors, p.pattern),
            start: p.first,
            end: p.last,
            event_id: Some(p.example.id),
        });
    }
    out.patterns.truncate(80);
    if out.undated > 0 {
        out.findings.push(Finding {
            kind: "quality".into(),
            title: "Eventos sem horário".into(),
            detail: format!(
                "{} registros não entram na linha do tempo. Configure a data e o fuso da fonte.",
                out.undated
            ),
            start: None,
            end: None,
            event_id: None,
        });
    }
    if !latency.is_empty() {
        latency.sort_by(f64::total_cmp);
        let pct = |p: f64| latency[((latency.len() - 1) as f64 * p).round() as usize];
        out.latency = Some(Latency {
            field: latency_field,
            unit: latency_unit,
            incompatible,
            count: latency_count,
            sampled: latency.len(),
            p50: pct(0.5),
            p95: pct(0.95),
            p99: pct(0.99),
        });
    }
    out
}

#[derive(Clone, Deserialize, schemars::JsonSchema)]
pub struct Period {
    pub start: i64,
    pub end: i64,
}
#[derive(Serialize)]
pub struct Change {
    pub pattern: String,
    pub before: usize,
    pub after: usize,
    pub before_rate: f64,
    pub after_rate: f64,
    pub delta: f64,
    pub example: Event,
}
#[derive(Serialize)]
pub struct Comparison {
    pub before_total: usize,
    pub after_total: usize,
    pub before_errors: usize,
    pub after_errors: usize,
    pub changes: Vec<Change>,
    pub limited: bool,
}
pub fn compare(events: impl Iterator<Item = Event>, before: &Period, after: &Period) -> Comparison {
    let mut out = Comparison {
        before_total: 0,
        after_total: 0,
        before_errors: 0,
        after_errors: 0,
        changes: vec![],
        limited: false,
    };
    let mut groups: HashMap<String, (usize, usize, Event)> = HashMap::new();
    for mut ev in events {
        if crate::operations::cancelled() {
            break;
        }
        let Some(t) = ev.timestamp else { continue };
        let a = t >= before.start && t <= before.end;
        let b = t >= after.start && t <= after.end;
        if !a && !b {
            continue;
        }
        out.before_total += usize::from(a);
        out.after_total += usize::from(b);
        out.before_errors += usize::from(a && is_error(&ev));
        out.after_errors += usize::from(b && is_error(&ev));
        let key = pattern_of(&ev.message);
        if !groups.contains_key(&key) && groups.len() >= 20_000 {
            out.limited = true;
            continue;
        }
        ev.raw.clear();
        ev.fields.clear();
        ev.message = ev.message.chars().take(500).collect();
        let entry = groups.entry(key).or_insert((0, 0, ev));
        entry.0 += usize::from(a);
        entry.1 += usize::from(b);
    }
    out.changes = groups
        .into_iter()
        .map(|(pattern, (before, after, example))| {
            let a = before as f64 / out.before_total.max(1) as f64;
            let b = after as f64 / out.after_total.max(1) as f64;
            Change {
                pattern,
                before,
                after,
                before_rate: a,
                after_rate: b,
                delta: b - a,
                example,
            }
        })
        .collect();
    out.changes.sort_by(|a, b| {
        b.delta
            .abs()
            .total_cmp(&a.delta.abs())
            .then(b.after.cmp(&a.after))
    });
    out.changes.truncate(100);
    out
}
