//! Commands of the security triage: detections, entity insights and rule management.
use crate::{
    detections::{self, Settings, Source},
    entities,
    model::Event,
    query::{self, Filter},
    workspace, AppState, SourceData,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

fn source_key(state: &AppState) -> String {
    match &*state.source.read() {
        SourceData::Indexed(idx) => format!(
            "idx:{}:{}",
            idx.lines.len(),
            idx.parts.iter().map(|p| p.identity.as_str()).collect::<Vec<_>>().join(",")
        ),
        SourceData::Memory(events) => format!("mem:{}:{:p}", events.len(), events.as_ptr()),
        SourceData::None => "none".into(),
    }
}

fn case_key(events: &[Event]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for e in events {
        e.id.hash(&mut hasher);
        e.event_ref.hash(&mut hasher);
        e.timestamp.hash(&mut hasher);
    }
    format!("case:{}:{:x}", events.len(), hasher.finish())
}

pub fn triage_impl(
    state: &AppState,
    filters: Vec<Filter>,
    case_events: Option<&[Event]>,
    force: bool,
) -> Result<Arc<serde_json::Value>, String> {
    workspace::validate(&filters)?;
    let rules = detections::ruleset()?;
    let settings = detections::load_settings();
    let catalog = if settings.threats { crate::threats::load_active().ok() } else { None };
    let key = format!(
        "{}|{}|{:p}|{:p}|{}|{}",
        case_events.map(case_key).unwrap_or_else(|| source_key(state)),
        serde_json::to_string(&filters).unwrap_or_default(),
        Arc::as_ptr(&rules),
        catalog.as_ref().map(Arc::as_ptr).unwrap_or(std::ptr::null()),
        serde_json::to_string(&settings).unwrap_or_default(),
        state.derived.read().len(),
    );
    if !force {
        if let Some(hit) = detections::cached(&key) {
            return Ok(hit);
        }
    }
    let inputs = detections::Inputs { rules: &rules, catalog: catalog.as_deref(), settings: &settings };
    let result = match case_events {
        Some(events) => {
            let prepared = query::prepare(&filters);
            let selected: Vec<&Event> = events
                .iter()
                .filter(|e| prepared.iter().all(|f| query::matches(e, f)))
                .collect();
            detections::run(&inputs, &Source::Events(selected))?
        }
        None => workspace::with_selection(state, &filters, |selection| {
            detections::run(&inputs, &Source::Selection(&selection))
        })?,
    };
    crate::operations::check()?;
    let value = Arc::new(serde_json::to_value(result).map_err(|e| e.to_string())?);
    if value.get("complete").and_then(|v| v.as_bool()).unwrap_or(false) {
        detections::remember(key, value.clone());
    }
    Ok(value)
}

#[tauri::command]
pub async fn triage(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    force: Option<bool>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    crate::offload(move || {
        let state = app.state::<AppState>();
        let events = crate::case_cache::resolve(case_events, case_key)?;
        triage_impl(state.inner(), filters, events.as_deref().map(|v| v.as_slice()), force.unwrap_or(false))
            .map(|v| (*v).clone())
    })
    .await?
}

#[derive(Serialize)]
pub struct ThreatExplanation {
    pub id: String,
    pub name: String,
    pub category: String,
    pub severity: String,
    pub kind: String,
    pub snippet: String,
    pub normalized: bool,
    pub attack: Vec<crate::attack::AttackRef>,
}

#[derive(Serialize)]
pub struct RuleMatch {
    pub id: String,
    pub name: String,
    pub severity: String,
    pub kind: String,
    pub attack: Vec<crate::attack::AttackRef>,
}

#[derive(Serialize)]
pub struct EventInsights {
    pub entities: Vec<entities::EntityValue>,
    pub action: Option<String>,
    pub action_label: Option<String>,
    pub outcome: Option<String>,
    pub decoded: Vec<entities::Decoded>,
    pub threats: Vec<ThreatExplanation>,
    pub rules: Vec<RuleMatch>,
}

pub fn insights_impl(event: &Event) -> Result<EventInsights, String> {
    let (action, outcome) = entities::action_outcome(event);
    let mut threats = Vec::new();
    if detections::load_settings().threats {
        if let Ok(catalog) = crate::threats::load_active() {
            for hit in catalog.explain(event) {
                let rule = catalog.rule(hit.rule);
                threats.push(ThreatExplanation {
                    id: rule.id.clone(),
                    name: rule.name.clone(),
                    category: rule.category.clone(),
                    severity: rule.severity.clone(),
                    kind: rule.kind.clone(),
                    snippet: hit.snippet,
                    normalized: hit.normalized,
                    attack: crate::threats::attack_for(rule)
                        .iter()
                        .map(|t| crate::attack::reference(t, &[]))
                        .collect(),
                });
            }
        }
    }
    let rules = detections::ruleset()?
        .rules
        .iter()
        .filter(|r| r.def.kind == "single" && r.matches(event))
        .take(20)
        .map(|r| RuleMatch {
            id: r.def.id.clone(),
            name: r.def.name.clone(),
            severity: r.def.severity.clone(),
            kind: r.origin.to_string(),
            attack: r.def.attack.iter().map(|t| crate::attack::reference(t, &r.def.tactics)).collect(),
        })
        .collect();
    Ok(EventInsights {
        entities: entities::entity_values(event),
        action: action.map(str::to_string),
        action_label: action.map(|a| entities::action_label(a).to_string()),
        outcome: outcome.map(str::to_string),
        decoded: entities::decode_payloads(event),
        threats,
        rules,
    })
}

#[tauri::command]
pub async fn event_insights(event: Event) -> Result<EventInsights, String> {
    crate::offload(move || insights_impl(&event)).await?
}

#[derive(Serialize)]
pub struct RuleInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: String,
    pub kind: String,
    pub origin: String,
    pub enabled: bool,
    pub attack: Vec<crate::attack::AttackRef>,
}

#[derive(Serialize)]
pub struct RulesOverview {
    pub rules: Vec<RuleInfo>,
    pub sigma_errors: Vec<String>,
    pub sigma_dir: String,
    pub settings: Settings,
}

pub fn rules_impl() -> Result<RulesOverview, String> {
    let settings = detections::load_settings();
    let set = detections::ruleset()?;
    let mut rules: Vec<RuleInfo> = set
        .rules
        .iter()
        .map(|r| RuleInfo {
            id: r.def.id.clone(),
            name: r.def.name.clone(),
            description: r.def.description.clone(),
            severity: r.def.severity.clone(),
            kind: r.def.kind.clone(),
            origin: r.origin.to_string(),
            enabled: r.enabled,
            attack: r.def.attack.iter().map(|t| crate::attack::reference(t, &r.def.tactics)).collect(),
        })
        .collect();
    rules.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.name.cmp(&b.name)));
    Ok(RulesOverview {
        rules,
        sigma_errors: set.sigma_errors.clone(),
        sigma_dir: detections::sigma_dir().to_string_lossy().into_owned(),
        settings,
    })
}

#[tauri::command]
pub async fn detection_rules() -> Result<RulesOverview, String> {
    crate::offload(rules_impl).await?
}

pub fn save_settings_impl(settings: Settings) -> Result<(), String> {
    detections::save_settings(&settings)?;
    detections::invalidate();
    detections::clear_cache();
    Ok(())
}

#[tauri::command]
pub async fn detection_settings_save(settings: Settings) -> Result<(), String> {
    crate::offload(move || save_settings_impl(settings)).await?
}

#[derive(Serialize)]
pub struct SigmaImport {
    pub imported: usize,
    pub rules: usize,
    pub failed: Vec<String>,
}

pub fn sigma_import_impl(paths: Vec<String>) -> Result<SigmaImport, String> {
    let target = detections::sigma_dir();
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for path in paths {
        let path = std::path::PathBuf::from(path);
        if path.is_dir() {
            files.extend(crate::sigma::rule_files(&path));
        } else if path.is_file() {
            files.push(path);
        }
    }
    let (mut imported, mut rules, mut failed) = (0, 0, Vec::new());
    for file in files {
        crate::operations::check()?;
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: {e}"));
                continue;
            }
        };
        match crate::sigma::convert_text(&text) {
            Ok(found) => {
                let mut destination = target.join(&name);
                let mut n = 1;
                while destination.exists() && std::fs::read_to_string(&destination).ok().as_deref() != Some(text.as_str()) {
                    destination = target.join(format!("{}-{n}.yml", name.trim_end_matches(".yml").trim_end_matches(".yaml")));
                    n += 1;
                }
                std::fs::write(&destination, &text).map_err(|e| e.to_string())?;
                imported += 1;
                rules += found.len();
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    detections::invalidate();
    detections::clear_cache();
    failed.truncate(200);
    Ok(SigmaImport { imported, rules, failed })
}

#[tauri::command]
pub async fn sigma_import(paths: Vec<String>) -> Result<SigmaImport, String> {
    crate::offload(move || sigma_import_impl(paths)).await?
}

pub fn sigma_clear_impl() -> Result<(), String> {
    let dir = detections::sigma_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    detections::invalidate();
    detections::clear_cache();
    Ok(())
}

#[tauri::command]
pub async fn sigma_clear() -> Result<(), String> {
    crate::offload(sigma_clear_impl).await?
}
