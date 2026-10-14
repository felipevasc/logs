//! Who accessed the data set: every client class, the families inside it and
//! the reasons why a browser claim cannot be true. Built from the `ua.*`
//! fields with the regular aggregations, so the query engine and its cache
//! answer it when available.
use crate::model::Event;
use crate::query::{AggSpec, Filter};
use crate::sources::user_agent;
use crate::AppState;
use serde_json::{json, Map, Value};

const FAMILIES_PER_CLASS: usize = 50;
const AGENTS: usize = 50;

fn measures() -> Vec<AggSpec> {
    [("count", "*", "records"), ("count_distinct", "@src_ip", "sources"), ("min", "timestamp", "first"), ("max", "timestamp", "last")]
        .into_iter()
        .map(|(func, column, alias)| AggSpec { func: func.into(), column: column.into(), alias: alias.into() })
        .collect()
}

fn filter(column: &str, op: &str, value: &str) -> Filter {
    Filter { column: column.into(), op: op.into(), value: value.into(), value2: None }
}

/// Groups of one column, largest first, with the measures under fixed names.
fn groups(state: &AppState, column: &str, filters: &[Filter], case_events: &Option<Vec<Event>>, limit: usize) -> Result<(Vec<Value>, usize), String> {
    crate::operations::check()?;
    let result = crate::aggregate_events_cached_impl(state, column, measures(), filters.to_vec(), case_events.clone())?;
    let mut rows: Vec<(String, Map<String, Value>)> = result
        .rows
        .into_iter()
        .zip(result.group_values)
        .filter_map(|(row, value)| Some((value.filter(|v| !v.trim().is_empty())?, row)))
        .collect();
    let count = |row: &Map<String, Value>| row.get("records").and_then(Value::as_u64).unwrap_or(0);
    rows.sort_by(|a, b| count(&b.1).cmp(&count(&a.1)).then_with(|| a.0.cmp(&b.0)));
    let omitted = rows.len().saturating_sub(limit) + result.omitted_groups;
    Ok((
        rows.into_iter()
            .take(limit)
            .map(|(value, row)| json!({"value": value, "records": count(&row), "sources": row.get("sources"), "first": row.get("first"), "last": row.get("last")}))
            .collect(),
        omitted,
    ))
}

pub(crate) fn inventory(state: &AppState, filters: Vec<Filter>, mut case_events: Option<Vec<Event>>) -> Result<Value, String> {
    // Captured Case events may predate the user agent profile.
    for event in case_events.iter_mut().flatten() {
        user_agent::annotate(event);
    }
    let (found, _) = groups(state, "ua.class", &filters, &case_events, user_agent::CLASSES.len() + 1)?;
    let mut classes = Vec::new();
    for class in user_agent::CLASSES {
        let Some(mut entry) = found.iter().find(|g| g["value"] == *class).cloned() else { continue };
        let mut scoped = filters.clone();
        scoped.push(filter("ua.class", "equals_exact", class));
        let (families, omitted) = groups(state, "ua.family", &scoped, &case_events, FAMILIES_PER_CLASS)?;
        entry["class"] = json!(class);
        entry["description"] = json!(user_agent::describe(class));
        entry["families"] = json!(families);
        entry["omitted_families"] = json!(omitted);
        classes.push(entry);
    }
    let total: u64 = classes.iter().filter_map(|c| c["records"].as_u64()).sum();
    let (anomalies, omitted_anomalies) = groups(state, "ua.anomaly", &filters, &case_events, 50)?;
    let mut forged = filters.clone();
    forged.push(filter("ua.anomaly", "not_empty", ""));
    let (agents, omitted_agents) = groups(state, "@user_agent", &forged, &case_events, AGENTS)?;
    Ok(json!({
        "total": total,
        "classes": classes,
        "anomalies": anomalies,
        "omitted_anomalies": omitted_anomalies,
        "forged_agents": agents,
        "omitted_forged_agents": omitted_agents,
        "method": "Classificação pelo próprio texto do user agent: gramática de cada motor, tokens que os navegadores congelam, versões existentes na data do evento e listas de ferramentas, bibliotecas, robôs e monitores conhecidos. Um user agent pode ser forjado para parecer comum; ausência de anomalia não prova que o cliente é um navegador.",
    }))
}
