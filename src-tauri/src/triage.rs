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
    match &*crate::analysis_runtime::source(&state) {
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
    crate::computed_cache::case_signature(events).unwrap_or_default()
}

// All rule stores share one analysis budget, but waiting remains cancellable.
static ANALYSIS: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
fn analysis_lock(cache_only: bool) -> Result<parking_lot::MutexGuard<'static, ()>, String> {
    if cache_only {
        return ANALYSIS.try_lock().ok_or_else(|| "TRIAGE_NOT_CALCULATED: O cálculo está em andamento; o resultado anterior ainda não está disponível.".to_string());
    }
    loop {
        crate::operations::check()?;
        if let Some(guard) = ANALYSIS.try_lock_for(std::time::Duration::from_millis(25)) {
            return Ok(guard);
        }
    }
}

pub fn stored_analysis(
    state: &AppState,
    case_events: Option<&[Event]>,
    force: bool,
) -> Result<Arc<crate::security_results::Results>, String> {
    stored_analysis_mode(state, case_events, force, false)
}

// Cache-only reads must never turn pagination or navigation into a new scan.
pub(crate) fn stored_analysis_mode(
    state: &AppState,
    case_events: Option<&[Event]>,
    force: bool,
    cache_only: bool,
) -> Result<Arc<crate::security_results::Results>, String> {
    let _working = if cache_only { None } else { Some(analysis_lock(false)?) };
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
        "{}|{}|{:p}|{:p}|{}|{}|{:?}",
        crate::analysis_runtime::cache_namespace(),
        case_events.map(case_key).unwrap_or_else(|| source_key(state)),
        Arc::as_ptr(&rules),
        catalog.as_ref().map(Arc::as_ptr).unwrap_or(std::ptr::null()),
        serde_json::to_string(&settings).unwrap_or_default(),
        crate::analysis_runtime::derived(&state).len(),
        expired,
    );
    if !force {
        if let Some(hit) = detections::cached(&key) {
            return Ok(hit);
        }
    }
    let saved = saved_path(state, case_events, &settings, catalog.as_deref(), &expired);
    if let (Some(path), false) = (&saved, force) {
        if path.exists() {
            if let Ok(hit) = crate::security_results::Results::open(path) {
                detections::remember(key, hit.clone());
                return Ok(hit);
            }
        }
    }
    if cache_only {
        return Err("TRIAGE_NOT_CALCULATED: A análise está ausente ou desatualizada. Clique em Calcular comprometimentos.".into());
    }
    crate::operations::report_progress("comprometimentos", "calculate", "Calculando comprometimentos", 0, 0, "registros", 0);
    let inputs = detections::Inputs { rules: &rules, catalog: catalog.as_deref(), settings: &settings };
    let result = match case_events {
        Some(events) => detections::run_stored(&inputs, &Source::Events(events.iter().collect()))?,
        None => workspace::with_selection(state, &[], |selection| {
            detections::run_stored(&inputs, &Source::Selection(&selection))
        })??,
    };
    crate::operations::check()?;
    if result.metadata.get("complete").and_then(|v| v.as_bool()).unwrap_or(false) {
        detections::remember(key, result.clone());
        if let Some(path) = saved {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(error) = result.save(&path) {
                eprintln!("[triage] análise não salva: {error}");
            }
        }
    }
    Ok(result)
}

/// Complete results survive restart and Case switches. Actual content and
/// interpretation changes, rather than screen/session counters, invalidate them.
fn saved_path(
    state: &AppState,
    case_events: Option<&[Event]>,
    settings: &Settings,
    catalog: Option<&crate::threats::CompiledCatalog>,
    expired: &[bool],
) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    // Session publication counters are deliberately absent here. Persistent
    // ownership, visibility and actual interpretation survive an app restart;
    // an identical source opened after another file must use the same result.
    let interpretation = match case_events {
        Some(events) => crate::computed_cache::case_signature(events),
        None => crate::computed_cache::source_signature(state).or_else(|| {
            let source = crate::analysis_runtime::source(state);
            match &*source { SourceData::Memory(events) => crate::computed_cache::case_signature(events), _ => None }
        }),
    }?;
    let mut hash = Sha256::new();
    hash.update(b"triage-persistence-v2");
    hash.update(crate::computed_cache::SEMANTICS);
    hash.update(interpretation);
    hash.update(detections::fingerprint());
    hash.update(serde_json::to_string(settings).unwrap_or_default());
    hash.update(catalog.map(|c| c.signature()).unwrap_or_default());
    hash.update(format!("{expired:?}"));
    Some(crate::config_dir().join("triage-v1").join(format!("{:x}.sqlite", hash.finalize())))
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
    triage_page_mode(state, filters, case, force, minimum, offset, limit, tactic, false)
}

fn triage_page_mode(
    state: &AppState,
    filters: Vec<Filter>,
    case: Option<&[Event]>,
    force: bool,
    minimum: u8,
    offset: usize,
    limit: usize,
    tactic: Option<&str>,
    cache_only: bool,
) -> Result<serde_json::Value, String> {
    if !(1..=5).contains(&minimum) {
        return Err("minimum_evidence must be between 1 and 5".into());
    }
    workspace::validate(&filters)?;
    let full = stored_analysis_mode(state, case, force, cache_only)?;
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
        })?,
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
    timeline_mode(state, filters, case, minimum, start, end, false)
}

pub(crate) fn timeline_mode(
    state: &AppState,
    filters: Vec<Filter>,
    case: Option<&[Event]>,
    minimum: u8,
    start: i64,
    end: i64,
    cache_only: bool,
) -> Result<serde_json::Value, String> {
    workspace::validate(&filters)?;
    let full = stored_analysis_mode(state, case, false, cache_only)?;
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
        })?,
    }
}

#[tauri::command]
pub async fn triage_timeline(
    filters: Vec<Filter>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    minimum_evidence: Option<u8>,
    start: i64,
    end: i64,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        let case = case_events;
        timeline_mode(
            app.state::<AppState>().inner(),
            filters,
            case.as_deref(),
            minimum_evidence.unwrap_or(5),
            start,
            end,
            true,
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
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    force: Option<bool>,
    cache_only: Option<bool>,
    operation_id: Option<String>,
    minimum_evidence: Option<u8>,
    episode_offset: Option<usize>,
    episode_limit: Option<usize>,
    tactic: Option<String>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, operation_id.clone(), crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id, app.clone(), admitted, case_events, move |case_events| {
        let state = app.state::<AppState>();
        let events = case_events;
        triage_page_mode(
            state.inner(),
            filters,
            events.as_deref(),
            force.unwrap_or(false),
            minimum_evidence.unwrap_or(5),
            episode_offset.unwrap_or(0),
            episode_limit.unwrap_or(20),
            tactic.as_deref(),
            cache_only.unwrap_or(false),
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
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        let full = stored_analysis_mode(app.state::<AppState>().inner(), case_events.as_deref(), false, true)?;
        if full.metadata["analysis_id"].as_str() != Some(&analysis_id) { return Err("A análise mudou; reabra suas evidências".into()); }
        full.episode_members(
            &episode_id,
            offset.unwrap_or(0),
            limit.unwrap_or(100),
        )
    })
    .await?
}

pub fn investigation_page_impl(state: &AppState, analysis_id: &str, section: &str, namespace: Option<&str>, entity: Option<&str>, signal: Option<&str>, offset: usize, limit: usize, case: Option<&[Event]>) -> Result<serde_json::Value,String> {
    let full = stored_analysis_mode(state, case, false, true)?;
    if full.metadata["analysis_id"].as_str()!=Some(analysis_id) { return Err("A análise mudou; recalcule antes de investigar.".into()); }
    full.investigation_page(section, namespace, entity, signal, offset, limit)
}

#[tauri::command]
pub async fn investigation_page(analysis_id: String, section: String, namespace: Option<String>, entity: Option<String>, signal_id: Option<String>, offset: Option<usize>, limit: Option<usize>, case_events: Option<Vec<Event>>, case_key: Option<String>, analysis_context: Option<crate::analysis_context::Identity>, source_generation: Option<u64>, operation_id:Option<String>, app: AppHandle) -> Result<serde_json::Value,String> {
    let (admitted, records)=crate::analysis_runtime::capture_case_async(app.clone(),analysis_context,source_generation,case_events,case_key,operation_id.clone(),crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id,app.clone(),admitted,records,move |records| {
        investigation_page_impl(app.state::<AppState>().inner(),&analysis_id,&section,namespace.as_deref(),entity.as_deref(),signal_id.as_deref(),offset.unwrap_or(0),limit.unwrap_or(20),records.as_deref())
    }).await?
}
pub fn investigation_profile_train_impl(state:&AppState,analysis_id:&str,case:Option<&[Event]>)->Result<serde_json::Value,String>{
    let _lane=crate::security_store::working_lane()?;
    let full=stored_analysis_mode(state,case,false,true)?;
    if full.metadata["analysis_id"].as_str()!=Some(analysis_id){return Err("A análise mudou; a referência não foi substituída".into());}
    let mut settings=detections::load_settings();let mut manifest=full.train_profile(&settings.investigation)?;
    settings.investigation.profile_revision=manifest["revision"].as_str().map(str::to_string);
    let receipt=serde_json::to_value(save_settings_impl(settings,None)?).map_err(|e|e.to_string())?;manifest["analysisContext"]=receipt["analysisContext"].clone();Ok(manifest)
}
pub fn investigation_proposal_evaluate_impl(state:&AppState,analysis_id:&str,signal_id:&str,controls:crate::security_proposals::Controls,case:Option<&[Event]>)->Result<serde_json::Value,String>{
    controls.validate()?;let original=stored_analysis_mode(state,case,false,true)?;
    if original.metadata["analysis_id"].as_str()!=Some(analysis_id){return Err("Análise desatualizada; proposta não avaliada".into());}
    let proposal=original.proposal(signal_id)?;let rule:detections::RuleDef=serde_json::from_value(proposal["rule"].clone()).map_err(|_|"Proposta exige desenho manual; não há seletor seguro compilável")?;
    let rules=detections::proposal_ruleset(rule)?;let mut settings=detections::load_settings();settings.threats=false;settings.investigation.enabled=false;
    let inputs=detections::Inputs{rules:&rules,catalog:None,settings:&settings};
    let evaluated=match case {Some(events)=>detections::run_stored_evaluation(&inputs,&Source::Events(events.iter().collect()))?,None=>workspace::with_selection(state,&[],|selection|detections::run_stored_evaluation(&inputs,&Source::Selection(&selection)))??};
    let evaluation=evaluated.evaluate_controls(&proposal,&controls,&original)?;original.record_proposal_evaluation(signal_id,&evaluation)?;Ok(evaluation)
}
pub fn investigation_proposal_accept_impl(state:&AppState,analysis_id:&str,signal_id:&str,rationale:&str,case:Option<&[Event]>)->Result<crate::analysis_commands::MutationReceipt,String>{
    let original=stored_analysis_mode(state,case,false,true)?;if original.metadata["analysis_id"].as_str()!=Some(analysis_id){return Err("Análise desatualizada; proposta não ativada".into());}
    let proposal=original.proposal(signal_id)?;let evaluation=original.proposal_evaluation(signal_id)?;let accepted=crate::security_proposals::accepted(&proposal,&evaluation,rationale)?;
    let text=crate::case_security::with(|snapshot|snapshot.custom_rules_json.clone());
    let mut document=match text {Some(text)=>serde_json::from_str::<serde_json::Value>(&text).map_err(|e|e.to_string())?,None=>serde_json::json!({"version":1,"rules":[]})};
    let rules=document["rules"].as_array_mut().ok_or("Regras do Caso fora do formato version/rules")?;
    if rules.iter().any(|rule|rule["id"].as_str()==Some(&accepted.id)){return Err("Proposta já existe nas regras do Caso; revise a definição existente".into());}
    rules.push(serde_json::to_value(accepted).map_err(|e|e.to_string())?);save_settings_impl(detections::load_settings(),Some(document.to_string()))
}
#[tauri::command]
pub async fn investigation_proposal_evaluate(analysis_id:String,signal_id:String,controls:crate::security_proposals::Controls,case_events:Option<Vec<Event>>,case_key:Option<String>,analysis_context:Option<crate::analysis_context::Identity>,source_generation:Option<u64>,operation_id:Option<String>,app:AppHandle)->Result<serde_json::Value,String>{
    let (admitted,records)=crate::analysis_runtime::capture_case_async(app.clone(),analysis_context,source_generation,case_events,case_key,operation_id.clone(),crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id,app.clone(),admitted,records,move|records|investigation_proposal_evaluate_impl(app.state::<AppState>().inner(),&analysis_id,&signal_id,controls,records.as_deref())).await?
}
#[tauri::command]
pub async fn investigation_proposal_accept(analysis_id:String,signal_id:String,rationale:String,case_events:Option<Vec<Event>>,case_key:Option<String>,analysis_context:Option<crate::analysis_context::Identity>,source_generation:Option<u64>,operation_id:Option<String>,app:AppHandle)->Result<crate::analysis_commands::MutationReceipt,String>{
    let (admitted,records)=crate::analysis_runtime::capture_case_async(app.clone(),analysis_context,source_generation,case_events,case_key,operation_id.clone(),crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id,app.clone(),admitted,records,move|records|investigation_proposal_accept_impl(app.state::<AppState>().inner(),&analysis_id,&signal_id,&rationale,records.as_deref())).await?
}
#[tauri::command]
pub async fn investigation_profile_train(analysis_id:String,case_events:Option<Vec<Event>>,case_key:Option<String>,analysis_context:Option<crate::analysis_context::Identity>,source_generation:Option<u64>,operation_id:Option<String>,app:AppHandle)->Result<serde_json::Value,String>{
    let (admitted,records)=crate::analysis_runtime::capture_case_async(app.clone(),analysis_context,source_generation,case_events,case_key,operation_id.clone(),crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id,app.clone(),admitted,records,move|records|investigation_profile_train_impl(app.state::<AppState>().inner(),&analysis_id,records.as_deref())).await?
}

pub fn evidence_event_impl(
    state: &AppState,
    analysis_id: &str,
    reference: &str,
    id: usize,
    case: Option<&[Event]>,
) -> Result<Event, String> {
    let full = stored_analysis_mode(state, case, false, true)?;
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
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<Event, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        let case = case_events;
        evidence_event_impl(
            app.state::<AppState>().inner(),
            &analysis_id,
            &event_ref,
            event_id,
            case.as_deref(),
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
    pub related_findings_calculated: bool,
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
        related_findings_calculated: false,
    })
}

#[tauri::command]
pub async fn event_insights(
    event: Event,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: AppHandle,
) -> Result<EventInsights, String> {
    let (admitted, case_events) = crate::analysis_runtime::capture_case_async(app.clone(), analysis_context, source_generation, case_events, case_key, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(None, app.clone(), admitted, case_events, move |case_events| {
        let case = case_events;
        if crate::analysis_runtime::visibility_restricted() {
            let visible = match case.as_deref() {
                Some(events) => events.iter().any(|candidate| candidate.event_ref == event.event_ref && candidate.id == event.id),
                None => crate::event_detail_raw(app.state::<AppState>().inner(), event.id).is_some_and(|candidate| candidate.event_ref == event.event_ref),
            };
            if !visible { return Err("O evento não pertence à análise visível atual.".into()); }
        }
        insights_in_context(app.state::<AppState>().inner(), &event, case.as_deref())
    })
    .await?
}

pub fn insights_in_context(state: &AppState, event: &Event, case: Option<&[Event]>) -> Result<EventInsights, String> {
    let mut result = insights_impl(event)?;
    // Opening one record must not trigger full-universe correlation. Its local
    // normalization/rules stay available even before Comprometimentos is asked for.
    match stored_analysis_mode(state, case, false, true) {
        Ok(full) => {
            let reference = crate::security_normalize::event_ref(event);
            (result.related_findings, result.related_findings_total) = full.related_page(&reference)?;
            result.related_findings_calculated = true;
        }
        Err(error) if error.starts_with("TRIAGE_NOT_CALCULATED:") => {}
        Err(error) => return Err(error),
    }
    Ok(result)
}

#[tauri::command]
pub async fn normalization_preview(
    event: Event,
    mappings: Vec<crate::security_normalize::SourceMapping>,
) -> Result<serde_json::Value, String> {
    crate::offload(move || {
        crate::security_normalize::validate_mappings(&mappings)?;
        let (_, normalized) = crate::security_normalize::normalize(&event, &mappings);
        // Shown as recorded, like every other record view.
        serde_json::to_value(normalized).map_err(|e| e.to_string())
    }).await?
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
    pub sigma_compatibility:serde_json::Value,
    pub retirement_reviews:serde_json::Value,
    pub sigma_errors: Vec<String>,
    pub sigma_dir: String,
    pub settings: Settings,
    pub custom_rules_json: Option<String>,
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
            attack: r
                .def
                .attack
                .iter()
                .map(|t| crate::attack::reference(t, &r.def.tactics))
                .collect(),
            evidence: r.def.evidence.clone(),
            provenance: r.def.provenance.clone(),
        })
        .collect();
    rules.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.name.cmp(&b.name)));
    Ok(RulesOverview {
        rules,
        sigma_compatibility:crate::sigma::compatibility(),
        retirement_reviews:serde_json::from_str(include_str!("../resources/detection-retirements.json")).map_err(|e|e.to_string())?,
        sigma_errors: set.sigma_errors.clone(),
        sigma_dir: "Fontes Sigma preservadas neste Caso".into(),
        settings,
        custom_rules_json: crate::case_security::with(|snapshot| {
            snapshot.custom_rules_json.clone()
        }),
    })
}

#[tauri::command]
pub async fn detection_rules(
    analysis_context: Option<crate::analysis_context::Identity>,
    app: AppHandle,
) -> Result<RulesOverview, String> {
    let identity = crate::analysis_commands::expected_identity(analysis_context)?;
    let admitted = crate::analysis_runtime::capture_async(app.clone(),
        Some(identity),
        None,
        crate::analysis_runtime::Mode::Metadata, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_admitted(None, app, admitted, rules_impl).await?
}

pub fn save_settings_impl(
    settings: Settings,
    custom_rules_json: Option<String>,
) -> Result<crate::analysis_commands::MutationReceipt, String> {
    let expected = crate::analysis_commands::expected_identity(None)?;
    let domain = if custom_rules_json.is_some() { "security_rules" } else { "security_settings" };
    let snapshot = crate::case_interpretation::update_domain(&expected, domain, |interpretation| {
        detections::validate_settings(&settings, &interpretation.security.detection_settings()?)?;
        interpretation.security.detection_settings_json =
            serde_json::to_string(&settings).map_err(|e| e.to_string())?;
        if let Some(text) = custom_rules_json {
            interpretation.security.custom_rules_json = (!text.trim().is_empty()).then_some(text);
        }
        Ok(())
    })?;
    Ok(crate::analysis_commands::MutationReceipt {
        analysis_context: snapshot,
    })
}
#[tauri::command]
pub async fn detection_settings_save(
    settings: Settings,
    custom_rules_json: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    app: AppHandle,
) -> Result<crate::analysis_commands::MutationReceipt, String> {
    let identity = crate::analysis_commands::expected_identity(analysis_context)?;
    let admitted = crate::analysis_runtime::capture_async(app.clone(),
        Some(identity),
        None,
        crate::analysis_runtime::Mode::Metadata, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_admitted(None, app, admitted, move || {
        save_settings_impl(settings, custom_rules_json)
    })
    .await?
}
#[derive(Serialize)]
pub struct SigmaImport {
    pub imported: usize,
    pub rules: usize,
    pub failed: Vec<String>,
    #[serde(rename = "analysisContext")]
    #[serde(serialize_with = "crate::analysis_context::serialize_management_snapshot")]
    pub analysis_context: crate::analysis_context::Snapshot,
}

pub fn sigma_import_impl(paths: Vec<String>) -> Result<SigmaImport, String> {
    let expected = crate::analysis_commands::expected_identity(None)?;
    if paths.len() > crate::case_security::MAX_SIGMA_FILES {
        return Err("Selecione até 512 arquivos Sigma.".into());
    }
    let mut files = Vec::new();
    for path in paths {
        crate::operations::check()?;
        let path = std::path::PathBuf::from(path);
        if path.is_dir() {
            files.extend(crate::sigma::rule_files(&path));
        } else if path.is_file() {
            files.push(path);
        }
        if files.len() > crate::case_security::MAX_SIGMA_FILES {
            return Err("Importação Sigma excede 512 arquivos.".into());
        }
    }
    // Read all selected bytes before the transaction. No global files are changed.
    let mut prepared = Vec::new();
    let mut bytes = 0usize;
    for file in files {
        crate::operations::check()?;
        let name = file
            .file_name()
            .ok_or("Arquivo sem nome")?
            .to_string_lossy()
            .into_owned();
        let text = crate::case_security::read_bounded(&file)?;
        bytes = bytes.saturating_add(text.len());
        if bytes > 2 << 20 {
            return Err("Importação Sigma excede 2 MiB.".into());
        }
        prepared.push(crate::case_security::SigmaSource { name, text });
    }
    let (mut imported, mut rules) = (0, 0);
    let snapshot = crate::case_interpretation::update_domain(&expected, "security_sigma", |interpretation| {
        let sources = &mut interpretation.security.sigma_sources;
        let previous = if sources.is_empty() {
            0
        } else {
            crate::sigma::convert_texts(sources.iter().map(|s| s.text.as_str()))?.len()
        };
        let mut unique: std::collections::HashSet<_> =
            sources.iter().map(|s| s.text.clone()).collect();
        for source in prepared {
            if unique.insert(source.text.clone()) {
                sources.push(source);
                imported += 1;
            }
        }
        if !sources.is_empty() {
            rules = crate::sigma::convert_texts(sources.iter().map(|s| s.text.as_str()))?
                .len()
                .saturating_sub(previous);
        }
        Ok(())
    })?;
    Ok(SigmaImport {
        imported,
        rules,
        failed: vec![],
        analysis_context: snapshot,
    })
}
#[tauri::command]
pub async fn sigma_import(
    paths: Vec<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    app: AppHandle,
) -> Result<SigmaImport, String> {
    let identity = crate::analysis_commands::expected_identity(analysis_context)?;
    let admitted = crate::analysis_runtime::capture_async(app.clone(),
        Some(identity),
        None,
        crate::analysis_runtime::Mode::Metadata, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_admitted(None, app, admitted, move || sigma_import_impl(paths)).await?
}
pub fn sigma_clear_impl() -> Result<crate::analysis_commands::MutationReceipt, String> {
    let expected = crate::analysis_commands::expected_identity(None)?;
    let snapshot = crate::case_interpretation::update_domain(&expected, "security_sigma", |interpretation| {
        interpretation.security.sigma_sources.clear();
        Ok(())
    })?;
    Ok(crate::analysis_commands::MutationReceipt {
        analysis_context: snapshot,
    })
}
#[tauri::command]
pub async fn sigma_clear(
    analysis_context: Option<crate::analysis_context::Identity>,
    app: AppHandle,
) -> Result<crate::analysis_commands::MutationReceipt, String> {
    let identity = crate::analysis_commands::expected_identity(analysis_context)?;
    let admitted = crate::analysis_runtime::capture_async(app.clone(),
        Some(identity),
        None,
        crate::analysis_runtime::Mode::Metadata, None, crate::global_scheduler::Priority::Normal).await?;
    crate::offload_admitted(None, app, admitted, sigma_clear_impl).await?
}

#[cfg(test)]
mod on_demand_tests {
    use super::*;

    #[test]
    fn saved_analysis_reopens_after_source_generation_changes_and_invalidates_changed_context() {
        use parking_lot::{Mutex, RwLock};
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                detections::clear_cache();
                match &self.0 {
                    Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                    None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
                }
            }
        }
        let root = tempfile::tempdir().unwrap();
        let _restore = Restore(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", root.path());
        crate::case_store::save(serde_json::json!({"cases":[{"id":"reopen"}]})).unwrap();
        let path = root.path().join("source.jsonl");
        std::fs::write(&path, "{\"message\":\"persisted event\",\"timestamp\":\"2026-10-04T12:00:00Z\"}\n").unwrap();
        let index = crate::sources::index_file(path.to_str().unwrap(), "jsonl", None, None, None).unwrap();
        let state = AppState {
            source: RwLock::new(SourceData::None), source_publication: RwLock::new(Default::default()),
            source_names: RwLock::new(vec![path.to_string_lossy().into_owned()]), codes: RwLock::new(Default::default()),
            system_codes: RwLock::new(Default::default()), derived: RwLock::new(vec![]),
            case_store_lock: Mutex::new(()), codes_path: Default::default(), system_codes_path: Default::default(),
        };
        let identity = crate::analysis_context::snapshot("reopen").unwrap().identity();
        let publisher = crate::analysis_runtime::capture(&state, Some(identity.clone()), None, crate::analysis_runtime::Mode::Publish).unwrap();
        crate::analysis_runtime::with(Some(publisher), || crate::source_publication::publish(&state, index, vec![path.to_string_lossy().into_owned()], vec![], false)).unwrap();
        let mut admitted = crate::analysis_runtime::capture(&state, Some(identity), None, crate::analysis_runtime::Mode::Dataset).unwrap();
        let settings = Settings::default();
        let first_key = crate::analysis_runtime::with(Some(admitted.clone()), || saved_path(&state, None, &settings, None, &[])).unwrap();
        // Use a tiny completed fixture result. The cache-only production path
        // must load this exact disk result without invoking any analysis.
        let event = Event::empty();
        let rules = detections::test_ruleset(vec![], vec![]).unwrap();
        let fixture_settings = Settings { threats: false, investigation: crate::investigation::Settings { enabled: false, ..Default::default() }, ..Default::default() };
        let result = detections::run_stored(&detections::Inputs { rules: &rules, catalog: None, settings: &fixture_settings }, &Source::Events(vec![&event])).unwrap();
        let catalog = crate::analysis_runtime::with(Some(admitted.clone()), || crate::threats::load_active()).unwrap();
        let disk = crate::analysis_runtime::with(Some(admitted.clone()), || saved_path(&state, None, &settings, Some(&catalog), &[])).unwrap();
        std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
        result.save(&disk).unwrap();
        detections::clear_cache();
        Arc::get_mut(&mut admitted).unwrap().source_generation = Some(73);
        assert_eq!(crate::analysis_runtime::with(Some(admitted.clone()), || saved_path(&state, None, &settings, None, &[])).unwrap(), first_key);
        let restored = crate::analysis_runtime::with(Some(admitted.clone()), || stored_analysis_mode(&state, None, false, true)).unwrap();
        assert_eq!(restored.metadata["analysis_id"], result.metadata["analysis_id"]);
        drop(restored);
        // Case records used to have no durable snapshot, and a global eight-file
        // cleanup used to discard other Cases. Save more than eight universes.
        let mut first_events = Vec::new();
        for number in 0..12 {
            let mut event = Event::empty();
            event.event_ref = format!("preserved:{number}"); event.message = format!("original {number}");
            let events = vec![event];
            let disk = crate::analysis_runtime::with(Some(admitted.clone()), || saved_path(&state, Some(&events), &settings, Some(&catalog), &[])).unwrap();
            result.save(&disk).unwrap();
            if number == 0 { first_events = events; }
        }
        detections::clear_cache();
        let restored = crate::analysis_runtime::with(Some(admitted.clone()), || stored_analysis_mode(&state, Some(&first_events), false, true)).unwrap();
        assert_eq!(restored.metadata["analysis_id"], result.metadata["analysis_id"]);
        drop(restored); detections::clear_cache();
        // Same IDs/timestamps must not conceal changes in the actual evidence.
        first_events[0].message.push_str(" changed");
        assert!(crate::analysis_runtime::with(Some(admitted.clone()), || stored_analysis_mode(&state, Some(&first_events), false, true)).err().unwrap().starts_with("TRIAGE_NOT_CALCULATED:"));
        Arc::get_mut(&mut admitted).unwrap().identity.as_mut().unwrap().visibility_revision += 1;
        assert_ne!(crate::analysis_runtime::with(Some(admitted.clone()), || saved_path(&state, None, &settings, None, &[])).unwrap(), first_key);
        assert!(crate::analysis_runtime::with(Some(admitted.clone()), || stored_analysis_mode(&state, None, false, true)).err().unwrap().starts_with("TRIAGE_NOT_CALCULATED:"));
        Arc::get_mut(&mut admitted).unwrap().identity.as_mut().unwrap().config_revision += 1;
        assert_ne!(crate::analysis_runtime::with(Some(admitted.clone()), || saved_path(&state, None, &settings, None, &[])).unwrap(), first_key);
    }

    #[test]
    fn cancelled_triage_does_not_wait_for_another_analysis_to_finish() {
        let working = ANALYSIS.lock();
        assert!(analysis_lock(true).err().unwrap().starts_with("TRIAGE_NOT_CALCULATED:"));
        let id = format!("triage-wait-{}", uuid::Uuid::new_v4());
        let token = crate::operations::token(Some(id.clone())).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let result = crate::operations::run_with_token(token, || analysis_lock(false).map(drop));
            done_tx.send(matches!(result, Err(_) | Ok(Err(_)))).unwrap();
        });
        ready_rx.recv().unwrap();
        assert!(crate::operations::cancel_id(&id));
        let cancelled = done_rx.recv_timeout(std::time::Duration::from_secs(2));
        // Always unlock/join before asserting, so a regression cannot hang the suite.
        drop(working);
        waiter.join().unwrap();
        assert_eq!(cancelled.unwrap(), true, "cancel must settle while the other analysis still holds the lock");
    }
}

pub fn investigation_narrative_review_impl(state:&AppState,analysis_id:&str,draft:crate::security_narrative::Draft,namespace:Option<&str>,entity:Option<&str>,case:Option<&[Event]>)->Result<serde_json::Value,String>{
    let full=stored_analysis_mode(state,case,false,true)?;if full.metadata["analysis_id"].as_str()!=Some(analysis_id){return Err("A análise mudou; revise a síntese no contexto corrente".into());}full.review_narrative(&draft,namespace,entity)
}
#[tauri::command]
pub async fn investigation_narrative_review(analysis_id:String,draft:crate::security_narrative::Draft,namespace:Option<String>,entity:Option<String>,case_events:Option<Vec<Event>>,case_key:Option<String>,analysis_context:Option<crate::analysis_context::Identity>,source_generation:Option<u64>,operation_id:Option<String>,app:AppHandle)->Result<serde_json::Value,String>{
    let(admitted,records)=crate::analysis_runtime::capture_case_async(app.clone(),analysis_context,source_generation,case_events,case_key,operation_id.clone(),crate::global_scheduler::Priority::Normal).await?;
    crate::offload_case(operation_id,app.clone(),admitted,records,move|records|investigation_narrative_review_impl(app.state::<AppState>().inner(),&analysis_id,draft,namespace.as_deref(),entity.as_deref(),records.as_deref())).await?
}
