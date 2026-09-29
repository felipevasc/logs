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

pub fn stored_analysis(
    state: &AppState,
    case_events: Option<&[Event]>,
    force: bool,
) -> Result<Arc<crate::security_results::Results>, String> {
    // One analysis worker: all rule stores share the application's analysis budget.
    static ANALYSIS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
    let _working = ANALYSIS.lock();
    crate::operations::check()?;
    let rules = detections::ruleset()?;
    let settings = detections::load_settings();
    let catalog = if settings.threats { Some(crate::threats::load_active()?) } else { None };
    let expired: Vec<_> = settings
        .suppress
        .iter()
        .map(|s| s.expires.is_some_and(|t| t <= chrono::Utc::now().timestamp_millis()))
        .collect();
    let key = format!(
        "{}|{:p}|{:p}|{}|{}|{:?}",
        case_events.map(case_key).unwrap_or_else(|| source_key(state)),
        Arc::as_ptr(&rules),
        catalog.as_ref().map(Arc::as_ptr).unwrap_or(std::ptr::null()),
        serde_json::to_string(&settings).unwrap_or_default(),
        state.derived.read().len(),
        expired,
    );
    if !force {
        if let Some(hit) = detections::cached(&key) {
            return Ok(hit);
        }
    }
    // Analyses of indexed files are kept on disk across sessions.
    let saved = case_events
        .is_none()
        .then(|| saved_path(state, &settings, catalog.as_deref(), &expired))
        .flatten();
    if let (Some(path), false) = (&saved, force) {
        if path.exists() {
            if let Ok(hit) = crate::security_results::Results::open(path) {
                detections::remember(key, hit.clone());
                return Ok(hit);
            }
        }
    }
    let inputs = detections::Inputs { rules: &rules, catalog: catalog.as_deref(), settings: &settings };
    let result = match case_events {
        Some(events) => detections::run_stored(&inputs, &Source::Events(events.iter().collect()))?,
        None => workspace::with_selection(state, &[], |selection| {
            detections::run_stored(&inputs, &Source::Selection(&selection))
        })?,
    };
    crate::operations::check()?;
    if result.metadata.get("complete").and_then(|v| v.as_bool()).unwrap_or(false) {
        detections::remember(key, result.clone());
        if let Some(path) = saved {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
                prune_saved(dir);
            }
            if let Err(error) = result.save(&path) {
                eprintln!("[triage] análise não salva: {error}");
            }
        }
    }
    Ok(result)
}

/// File of the saved analysis of the indexed source with the current rules,
/// catalog, settings and derived fields; `None` for sources held in memory.
fn saved_path(
    state: &AppState,
    settings: &Settings,
    catalog: Option<&crate::threats::CompiledCatalog>,
    expired: &[bool],
) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let source = source_key(state);
    if !source.starts_with("idx:") {
        return None;
    }
    let derived: Vec<String> = state
        .derived
        .read()
        .iter()
        .map(|d| {
            let rules: Vec<String> = d
                .rules
                .iter()
                .map(|r| format!("{}|{:?}|{}", r.re.as_str(), r.template, serde_json::to_string(&r.filter).unwrap_or_default()))
                .collect();
            format!("{}|{}|{rules:?}", d.name, d.source)
        })
        .collect();
    let mut hash = Sha256::new();
    hash.update(env!("CARGO_PKG_VERSION"));
    hash.update(source);
    hash.update(detections::fingerprint().to_le_bytes());
    hash.update(serde_json::to_string(settings).unwrap_or_default());
    hash.update(catalog.map(|c| c.signature()).unwrap_or_default());
    hash.update(format!("{derived:?}{expired:?}"));
    Some(crate::config_dir().join("triage-v1").join(format!("{:x}.sqlite", hash.finalize())))
}

/// Keeps the 8 most recently saved analyses.
fn prune_saved(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut saved: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "sqlite"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    saved.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in saved.into_iter().skip(7) {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
    }
}

#[cfg(test)]
pub fn analysis_impl(
    state: &AppState,
    case_events: Option<&[Event]>,
    force: bool,
) -> Result<Arc<serde_json::Value>, String> {
    Ok(Arc::new(stored_analysis(state, case_events, force)?.page(1, 0, 500, None, None)?))
}

/// Filtering selects related findings after the full universe has been correlated.
/// Context members keep their original level and cannot become independent high-level findings.
pub fn triage_page(
    state: &AppState,
    filters: Vec<Filter>,
    case: Option<&[Event]>,
    force: bool,
    minimum: u8,
    offset: usize,
    limit: usize,
    tactic: Option<&str>,
) -> Result<serde_json::Value, String> {
    if !(1..=5).contains(&minimum) {
        return Err("minimum_evidence must be between 1 and 5".into());
    }
    workspace::validate(&filters)?;
    let full = stored_analysis(state, case, force)?;
    if filters.is_empty() {
        return full.page(minimum, offset, limit, None, tactic);
    }
    match case {
        Some(events) => {
            let prepared = query::prepare(&filters);
            let mut ids = events.iter().filter(|e| prepared.iter().all(|f| query::matches(e, f))).map(|e| e.id);
            full.page(minimum, offset, limit, Some(&mut ids), tactic)
        }
        None => workspace::with_selection(state, &filters, |selection| {
            let mut ids = selection.iter().map(|e| e.id);
            full.page(minimum, offset, limit, Some(&mut ids), tactic)
        }),
    }
}

pub fn timeline_impl(
    state: &AppState,
    filters: Vec<Filter>,
    case: Option<&[Event]>,
    minimum: u8,
    start: i64,
    end: i64,
) -> Result<serde_json::Value, String> {
    workspace::validate(&filters)?;
    let full = stored_analysis(state, case, false)?;
    if filters.is_empty() {
        return full.timeline(minimum, start, end, None);
    }
    match case {
        Some(events) => {
            let prepared = query::prepare(&filters);
            let mut ids = events.iter().filter(|e| prepared.iter().all(|f| query::matches(e, f))).map(|e| e.id);
            full.timeline(minimum, start, end, Some(&mut ids))
        }
        None => workspace::with_selection(state, &filters, |selection| {
            let mut ids = selection.iter().map(|e| e.id);
            full.timeline(minimum, start, end, Some(&mut ids))
        }),
    }
}

#[tauri::command]
pub async fn triage_timeline(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    minimum_evidence: Option<u8>,
    start: i64,
    end: i64,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    crate::offload(move || {
        let case = crate::case_cache::resolve(case_events, case_key)?;
        timeline_impl(
            app.state::<AppState>().inner(),
            filters,
            case.as_deref().map(|v| v.as_slice()),
            minimum_evidence.unwrap_or(5),
            start,
            end,
        )
    })
    .await?
}

pub fn project(
    full: &serde_json::Value,
    minimum: u8,
    related: Option<&std::collections::HashSet<usize>>,
) -> serde_json::Value {
    use serde_json::{json, Value};
    let mut result = full.clone();
    let all = full["detections"].as_array().cloned().unwrap_or_default();
    let related_indices: std::collections::HashSet<_> = all
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            related.is_none_or(|ids| {
                d["event_ids"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|id| ids.contains(&(id.as_u64().unwrap_or(u64::MAX) as usize))))
            })
        })
        .map(|(i, _)| i)
        .collect();
    let visible: std::collections::HashSet<_> = related_indices
        .iter()
        .copied()
        .filter(|&i| all[i]["evidence_level"].as_u64().unwrap_or(0) >= minimum as u64)
        .collect();
    let counts: Vec<_> = (1..=5)
        .map(|level| related_indices.iter().filter(|&&i| all[i]["evidence_level"].as_u64() == Some(level)).count())
        .collect();
    let episodes: Vec<_> = full["episodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| {
            e["detections"]
                .as_array()
                .is_some_and(|a| a.iter().any(|i| visible.contains(&(i.as_u64().unwrap_or(u64::MAX) as usize))))
        })
        .cloned()
        .collect();
    let mut included = visible.clone();
    for e in &episodes {
        for i in e["detections"].as_array().into_iter().flatten() {
            included.insert(i.as_u64().unwrap_or(u64::MAX) as usize);
        }
    }
    let mut remap = std::collections::HashMap::new();
    let mut rows = Vec::new();
    for (i, d) in all.iter().enumerate() {
        if included.contains(&i) {
            remap.insert(i, rows.len());
            let mut row = d.clone();
            row["context_only"] = json!(!visible.contains(&i));
            rows.push(row);
        }
    }
    let episodes: Vec<_> = episodes
        .into_iter()
        .map(|mut e| {
            e["detections"] = json!(e["detections"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|i| remap.get(&(i.as_u64()? as usize)))
                .copied()
                .collect::<Vec<_>>());
            e
        })
        .collect();
    result["detections"] = json!(rows);
    result["episodes"] = json!(episodes);
    result["counts_by_level"] = json!(counts);
    result["minimum_evidence"] = json!(minimum);
    result["available_detections"] = json!(related_indices.len());
    result["visible_detections"] = json!(visible.len());
    result["analysis_scope"] = json!("full_universe");
    result["display_filtered"] = json!(related.is_some());
    let entities = full["entities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| {
            e["evidence_level"].as_u64().unwrap_or(0) >= minimum as u64
                && visible.iter().any(|&i| {
                    all[i]["namespace"] == e["namespace"]
                        && all[i]["entities"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|x| x["column"] == e["column"] && x["value"] == e["value"])
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    result["entities"] = json!(entities);
    if let Some(tactics) = result["tactics"].as_array_mut() {
        for t in tactics {
            let matches: Vec<_> = visible
                .iter()
                .map(|&i| &all[i])
                .filter(|d| d["tactics"].as_array().is_some_and(|a| a.contains(&t["key"])))
                .collect();
            t["count"] = json!(matches.len());
            let mut techniques = std::collections::BTreeMap::<String, Value>::new();
            for d in matches {
                for a in d["attack"].as_array().into_iter().flatten() {
                    let id = a["id"].as_str().unwrap_or_default().to_string();
                    let entry = techniques.entry(id.clone()).or_insert(json!({"id":id,"name":a["name"],"count":0}));
                    entry["count"] = json!(entry["count"].as_u64().unwrap_or(0) + 1);
                }
            }
            t["techniques"] = json!(techniques.into_values().collect::<Vec<_>>());
        }
    }
    result
}

/// Paginate episodes after correlation and classification, keeping every context member.
pub fn paginate(mut result: serde_json::Value, offset: usize, limit: usize) -> Result<serde_json::Value, String> {
    use serde_json::json;
    if !(1..=500).contains(&limit) {
        return Err("episode_limit deve estar entre 1 e 500".into());
    }
    let all = result["detections"].as_array().cloned().unwrap_or_default();
    let total = result["episodes"].as_array().map_or(0, Vec::len);
    let mut episodes: Vec<_> =
        result["episodes"].as_array().into_iter().flatten().skip(offset).take(limit).cloned().collect();
    let included: std::collections::HashSet<_> = episodes
        .iter()
        .flat_map(|e| e["detections"].as_array().into_iter().flatten())
        .filter_map(|i| i.as_u64().map(|i| i as usize))
        .collect();
    let mut remap = std::collections::HashMap::new();
    let mut rows = Vec::new();
    for (i, d) in all.into_iter().enumerate() {
        if included.contains(&i) {
            remap.insert(i, rows.len());
            rows.push(d);
        }
    }
    for e in &mut episodes {
        e["detections"] = json!(e["detections"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| remap.get(&(i.as_u64()? as usize)))
            .copied()
            .collect::<Vec<_>>());
    }
    result["page"] = json!({"episode_offset":offset,"episode_limit":limit,"total_episodes":total,"returned_episodes":episodes.len(),"next_offset":if offset.saturating_add(limit)<total {Some(offset+limit)}else{None}});
    result["returned_detections"] = json!(rows.iter().filter(|d| d["context_only"] != true).count());
    result["episodes"] = json!(episodes);
    result["detections"] = json!(rows);
    Ok(result)
}

#[tauri::command]
pub async fn triage(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    force: Option<bool>,
    minimum_evidence: Option<u8>,
    episode_offset: Option<usize>,
    episode_limit: Option<usize>,
    tactic: Option<String>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    crate::offload(move || {
        let state = app.state::<AppState>();
        let events = crate::case_cache::resolve(case_events, case_key)?;
        triage_page(
            state.inner(),
            filters,
            events.as_deref().map(|v| v.as_slice()),
            force.unwrap_or(false),
            minimum_evidence.unwrap_or(5),
            episode_offset.unwrap_or(0),
            episode_limit.unwrap_or(20),
            tactic.as_deref(),
        )
    })
    .await?
}

#[tauri::command]
pub async fn triage_episode(
    analysis_id: String,
    episode_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    crate::offload(move || {
        detections::cached_analysis(&analysis_id).ok_or("Análise expirada; recarregue a triagem")?.episode_members(
            &episode_id,
            offset.unwrap_or(0),
            limit.unwrap_or(100),
        )
    })
    .await?
}

pub fn evidence_event_impl(
    state: &AppState,
    analysis_id: &str,
    reference: &str,
    id: usize,
    case: Option<&[Event]>,
) -> Result<Event, String> {
    let full = stored_analysis(state, case, false)?;
    if full.metadata["analysis_id"].as_str() != Some(analysis_id) {
        return Err("O conjunto ou a análise mudou; recarregue Comprometimentos".into());
    }
    if !full.contains_member(reference, id)? {
        return Err("O evento não pertence às evidências desta análise".into());
    }
    let event = match case {
        Some(events) => events.iter().find(|e| e.id == id).cloned(),
        None => crate::event_detail_raw(state, id),
    }
    .ok_or("Evento original indisponível")?;
    if crate::security_normalize::event_ref(&event) != reference {
        return Err("A identidade do evento mudou; recarregue a análise".into());
    }
    Ok(event)
}

#[tauri::command]
pub async fn triage_evidence_event(
    analysis_id: String,
    event_ref: String,
    event_id: usize,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    app: AppHandle,
) -> Result<Event, String> {
    crate::offload(move || {
        let case = crate::case_cache::resolve(case_events, case_key)?;
        evidence_event_impl(
            app.state::<AppState>().inner(),
            &analysis_id,
            &event_ref,
            event_id,
            case.as_deref().map(|v| v.as_slice()),
        )
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
    pub provenance: crate::threats::Segment,
    pub attack: Vec<crate::attack::AttackRef>,
}

#[derive(Serialize)]
pub struct RuleMatch {
    pub id: String,
    pub name: String,
    pub severity: String,
    pub kind: String,
    pub attack: Vec<crate::attack::AttackRef>,
    #[serde(flatten)]
    pub evidence: crate::evidence::Evidence,
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
    pub normalization: crate::security_normalize::Normalized,
    pub related_findings: Vec<serde_json::Value>,
    pub related_findings_total: usize,
}

pub fn insights_impl(event: &Event) -> Result<EventInsights, String> {
    let settings = detections::load_settings();
    let (_, normalization) = crate::security_normalize::normalize(event, &settings.mappings);
    let action = normalization.get("action");
    let outcome = normalization.get("outcome");
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
                    provenance: hit.provenance,
                    attack: crate::threats::attack_for(rule).iter().map(|t| crate::attack::reference(t, &[])).collect(),
                });
            }
        }
    }
    let set = detections::ruleset()?;
    let input = detections::Inputs { rules: &set, catalog: None, settings: &settings };
    let assessed = detections::run(&input, &Source::Events(vec![event]))?;
    let rules = assessed
        .detections
        .into_iter()
        .map(|d| RuleMatch {
            id: d.rule,
            name: d.name,
            severity: d.severity,
            kind: d.origin,
            attack: d.attack,
            evidence: d.evidence,
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
        normalization,
        related_findings: Vec::new(),
        related_findings_total: 0,
    })
}

#[tauri::command]
pub async fn event_insights(
    event: Event,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    app: AppHandle,
) -> Result<EventInsights, String> {
    crate::offload(move || {
        let case = crate::case_cache::resolve(case_events, case_key)?;
        insights_in_context(app.state::<AppState>().inner(), &event, case.as_deref().map(|v| v.as_slice()))
    })
    .await?
}

pub fn insights_in_context(state: &AppState, event: &Event, case: Option<&[Event]>) -> Result<EventInsights, String> {
    let mut result = insights_impl(event)?;
    let full = stored_analysis(state, case, false)?;
    let reference = crate::security_normalize::event_ref(event);
    (result.related_findings, result.related_findings_total) = full.related_page(&reference)?;
    Ok(result)
}

#[tauri::command]
pub async fn normalization_preview(
    event: Event,
    mappings: Vec<crate::security_normalize::SourceMapping>,
) -> Result<serde_json::Value, String> {
    crate::security_normalize::validate_mappings(&mappings)?;
    let (_, normalized) = crate::security_normalize::normalize(&event, &mappings);
    let mut value = serde_json::to_value(normalized).map_err(|e| e.to_string())?;
    crate::workspace::redact_value(&mut value);
    Ok(value)
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
    pub retired: bool,
    pub evidence_level: u8,
    pub evidence_label: String,
    pub attack: Vec<crate::attack::AttackRef>,
    pub evidence: crate::evidence::Policy,
    pub provenance: serde_json::Value,
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
            retired: !r.def.enabled,
            evidence_level: r.def.evidence.assessed_level(),
            evidence_label: crate::evidence::label(r.def.evidence.assessed_level()).into(),
            attack: r.def.attack.iter().map(|t| crate::attack::reference(t, &r.def.tactics)).collect(),
            evidence: r.def.evidence.clone(),
            provenance: r.def.provenance.clone(),
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
    let mut prepared = Vec::new();
    let mut unique = std::collections::HashSet::new();
    let mut texts = Vec::new();
    for file in crate::sigma::rule_files(&target) {
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        if unique.insert(text.clone()) {
            texts.push(text);
        }
    }
    let previous = crate::sigma::load_dir(&target).0.len();
    for file in files {
        crate::operations::check()?;
        let name = file.file_name().ok_or("Arquivo sem nome")?.to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{name}: {e}"))?;
        if unique.insert(text.clone()) {
            texts.push(text.clone());
            prepared.push((name, text));
        }
    }
    if prepared.is_empty() {
        return Ok(SigmaImport { imported: 0, rules: 0, failed: vec![] });
    }
    // Resolve dependencies across the entire batch before publishing any file.
    let found = crate::sigma::convert_texts(texts.iter().map(String::as_str))?;
    let mut published = Vec::new();
    let publish = (|| -> Result<(), String> {
        for (name, text) in &prepared {
            crate::operations::check()?;
            let mut destination = target.join(name);
            let mut n = 1;
            while destination.exists() {
                destination =
                    target.join(format!("{}-{n}.yml", name.trim_end_matches(".yml").trim_end_matches(".yaml")));
                n += 1;
            }
            let pending = destination.with_extension("pending");
            std::fs::write(&pending, text).map_err(|e| e.to_string())?;
            if let Err(e) = std::fs::rename(&pending, &destination) {
                let _ = std::fs::remove_file(&pending);
                return Err(e.to_string());
            }
            published.push(destination);
        }
        Ok(())
    })();
    if let Err(error) = publish {
        for path in published {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    let imported = prepared.len();
    let rules = found.len().saturating_sub(previous);
    let failed = Vec::new();
    detections::invalidate();
    detections::clear_cache();
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
