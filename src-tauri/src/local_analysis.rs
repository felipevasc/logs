//! Desktop local exploration commands, all admitted through the existing Case
//! context/cancellation boundary. Original evidence never travels as a SQL row.
use crate::{
    analysis_context::Identity,
    analysis_runtime,
    model::Event,
    query::Filter,
    search_regex::{Matcher, Pattern, Span},
    AppState, SourceData,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Text {
        request: crate::text_reader::Request,
    },
    Schema,
    Query {
        sql: String,
    },
    QueryPage {
        result: String,
        offset: u64,
        limit: usize,
    },
    QueryExport {
        result: String,
        path: String,
        format: String,
    },
    Regex {
        pattern: Pattern,
        text: String,
    },
    /// Sample test with group ranges and positioned diagnostics.
    RegexPreview {
        pattern: Pattern,
        text: String,
        /// `line` tests each physical line on its own; anything else tests the whole text.
        scope: Option<String>,
    },
    /// Whole text of one field of a visible event, bounded, to seed a sample.
    SampleText {
        id: usize,
        event_ref: String,
        field: String,
    },
    Highlights {
        rules: Vec<crate::search_highlights::Rule>,
        texts: Vec<String>,
    },
    SearchStart {
        pattern: Pattern,
        scope: String,
    },
    SearchStep {
        search: String,
    },
    SearchPage {
        search: String,
        offset: u64,
    },
    SaveField {
        name: String,
        source: String,
        definition: crate::advanced_fields::Definition,
    },
    MapTemplate,
    MapPreview {
        map_json: String,
        limit: Option<usize>,
    },
    SaveMap {
        name: String,
        map_json: String,
    },
    /// Grok library, built-in extraction templates and the engine's limits.
    MapCatalog,
    /// Live diagnostics of every extraction in a draft (no source needed).
    MapCheck {
        map_json: String,
    },
    /// Stratified, repeatable sample of real records.
    MapSample {
        sources: Option<Vec<String>>,
        count: Option<usize>,
        field: Option<String>,
        seed: Option<u64>,
    },
    /// Records grouped by how their lines look, with the ready-made format each fits.
    MapGroups {
        sources: Option<Vec<String>>,
        field: Option<String>,
        seed: Option<u64>,
    },
    MapSuggest {
        texts: Vec<String>,
    },
    /// First records of a file as framed by the reader, or by a user-defined record start.
    MapFrame {
        path: String,
        pattern: Option<String>,
        count: Option<usize>,
    },
    /// Candidate patterns checked by the same PCRE2 engine that extracts the columns.
    MapVerify {
        texts: Vec<String>,
        patterns: Vec<String>,
    },
    /// The draft applied to sample records, with ranges and per-output states.
    MapEvaluate {
        map_json: String,
        samples: Vec<crate::map_lab::SampleRef>,
    },
    /// Pasted or template lines evaluated as records, without any loaded source.
    MapTry {
        map_json: String,
        texts: Vec<String>,
    },
    /// Complete pass over the loaded records of the current cut.
    MapScan {
        map_json: String,
    },
    MapDiff {
        map_json: String,
        current_json: Option<String>,
        prefix: String,
        #[serde(default)]
        indexed: Vec<String>,
    },
    Processes,
    ProcessPage {
        graph: String,
        parent: Option<String>,
        search: Option<String>,
        offset: Option<u64>,
        selected: Option<String>,
    },
    Context {
        id: usize,
        event_ref: String,
        mode: String,
        radius: Option<usize>,
        seconds: Option<u64>,
        field: Option<String>,
        pattern: Option<Pattern>,
        #[serde(rename = "subjectScope")]
        subject_scope: Option<String>,
    },
    ResolveText {
        path: String,
        offset: String,
    },
}
fn value(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}
fn save(
    name: &str,
    source: &str,
    definition: crate::advanced_fields::Definition,
) -> Result<Value, String> {
    let expected = crate::analysis_commands::expected_identity(None)?;
    let snapshot = crate::analysis_context::snapshot(&expected.case_id)?;
    if snapshot.identity() != expected {
        return Err(analysis_runtime::STALE.into());
    }
    let mut config = snapshot.config;
    let raw = json!({"name":name,"source":source,"advanced":definition});
    if let Some(at) = config
        .derived_fields
        .iter()
        .position(|d| d.get("name").and_then(Value::as_str) == Some(name))
    {
        config.derived_fields[at] = raw;
    } else {
        config.derived_fields.push(raw);
    }
    crate::analysis_commands::validate_config_for(&expected, &config)?;
    let _prepared = crate::analysis_commands::prepare_config(&expected, &config)?;
    crate::operations::check()?;
    let snapshot = crate::analysis_context::update(&expected, config)?;
    crate::operations::commit();
    value(crate::analysis_commands::MutationReceipt {
        analysis_context: snapshot,
    })
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextRow {
    id: usize,
    event_ref: String,
    timestamp: Option<i64>,
    level: String,
    source: String,
    text: String,
    spans: Vec<Span>,
    truncated_before: bool,
    truncated_after: bool,
    path: Option<String>,
    byte_offset: Option<String>,
    matches_complete: bool,
}
fn snippet(
    event: &Event,
    pattern: Option<&Matcher>,
    state: &AppState,
    subject_scope: &str,
) -> Result<ContextRow, String> {
    let full = if subject_scope == "message" || event.raw.is_empty() {
        &event.message
    } else {
        &event.raw
    };
    let found = pattern
        .map(|p| {
            if subject_scope == "line" {
                p.find_lines(full)
            } else {
                p.find_spans(full)
            }
        })
        .transpose()?;
    let matches_complete = found.as_ref().is_none_or(|f| f.complete);
    let at = found
        .as_ref()
        .and_then(|f| f.spans.first())
        .map_or(0, |s| s.byte_start);
    let mut start = at.saturating_sub(400);
    while !full.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + 12_000).min(full.len());
    while !full.is_char_boundary(end) {
        end -= 1;
    }
    let base = full[..start].encode_utf16().count();
    let limit = full[..end].encode_utf16().count();
    let spans = found
        .into_iter()
        .flat_map(|f| f.spans)
        .filter(|s| s.end >= base && s.start <= limit)
        .map(|mut s| {
            s.start = s.start.max(base) - base;
            s.end = s.end.min(limit) - base;
            s.captures.clear();
            s
        })
        .collect();
    let source = analysis_runtime::source(state);
    let (path, offset) = if let SourceData::Indexed(index) = &*source {
        if event.id < index.lines.len() {
            let part = index.part_at(event.id);
            (
                Some(part.path.clone()),
                Some((index.lines.at(event.id).offset - part.base).to_string()),
            )
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    Ok(ContextRow {
        id: event.id,
        event_ref: event.event_ref.clone(),
        timestamp: event.timestamp,
        source: event.source.clone(),
        level: event.level.clone(),
        text: full[start..end].into(),
        spans,
        truncated_before: start > 0,
        truncated_after: end < full.len(),
        path,
        byte_offset: offset,
        matches_complete,
    })
}
fn context(
    state: &AppState,
    id: usize,
    event_ref: &str,
    mode: &str,
    radius: usize,
    seconds: u64,
    field: Option<&str>,
    pattern: Option<&Pattern>,
    subject_scope: &str,
) -> Result<Value, String> {
    if !["event", "message", "line"].contains(&subject_scope) {
        return Err("Escopo de contexto inválido.".into());
    }
    let source = analysis_runtime::source(state);
    if let SourceData::Indexed(index) = &*source {
        if id < index.lines.len()
            && index.lines.at(id).len as usize > crate::search_regex::SUBJECT_LIMIT
        {
            return Err("O registro excede 8 MiB. Consulte o arquivo em Texto direto para navegar por janelas.".into());
        }
    }
    let center = crate::workspace::with_selection(state, &[], |s| s.event(id))?
        .ok_or("Evento ausente ou excluído.")?;
    if center.event_ref != event_ref {
        return Err("A referência do evento mudou.".into());
    }
    let matcher = pattern.map(Matcher::new).transpose()?;
    let radius = radius.clamp(1, 60);
    let mut rows = Vec::new();
    let mut more = false;
    let source = analysis_runtime::source(state);
    if mode == "physical" {
        let path = match &*source {
            SourceData::Indexed(index) if id < index.lines.len() => {
                Some(index.part_at(id).identity.clone())
            }
            _ => None,
        };
        crate::workspace::with_selection(state, &[], |s| {
            for row in id.saturating_sub(radius)..=id.saturating_add(radius) {
                if let (Some(identity), SourceData::Indexed(index)) = (&path, &*source) {
                    if row >= index.lines.len() || index.part_at(row).identity != *identity {
                        continue;
                    }
                }
                if let Some(event) = s.event(row) {
                    rows.push(snippet(&event, matcher.as_ref(), state, subject_scope)?);
                }
            }
            Ok::<_, String>(())
        })??;
    } else {
        let filters = if mode == "temporal" {
            let at = center
                .timestamp
                .ok_or("O evento não possui horário reconhecido.")?;
            let window = seconds.clamp(1, 86400) as i64 * 1000;
            vec![Filter {
                column: "timestamp".into(),
                op: "between".into(),
                value: at.saturating_sub(window).to_string(),
                value2: Some(at.saturating_add(window).to_string()),
            }]
        } else if mode == "identifier" {
            let field = field
                .filter(|s| !s.is_empty() && s.len() <= 512)
                .ok_or("Informe o campo identificador.")?;
            let v = center.col_str(field).unwrap_or_default();
            if v.is_empty() || v.len() > 64 << 10 {
                return Err("Identificador ausente ou extenso demais.".into());
            }
            vec![Filter {
                column: field.into(),
                op: "equals_exact".into(),
                value: v,
                value2: None,
            }]
        } else {
            return Err("Use contexto físico, temporal ou por identificador.".into());
        };
        crate::workspace::validate(&filters)?;
        crate::workspace::with_selection(state, &filters, |s| {
            // Keep the closest records, bounded independently of total matches.
            let mut nearest = std::collections::BinaryHeap::new();
            let indexed = if let SourceData::Indexed(index) = &*source {
                s.visit_exact_ids(|row| {
                    let distance = if mode == "temporal" {
                        index.lines.at(row).ts.abs_diff(center.timestamp.unwrap())
                    } else {
                        row.abs_diff(id) as u64
                    };
                    nearest.push((distance, row));
                    if nearest.len() > radius * 2 + 1 {
                        nearest.pop();
                        more = true;
                    }
                    Ok(true)
                })?
            } else {
                None
            };
            if indexed.is_none() {
                for event in s.iter() {
                    crate::operations::check()?;
                    let distance = if mode == "temporal" {
                        event
                            .timestamp
                            .unwrap_or(i64::MIN)
                            .abs_diff(center.timestamp.unwrap())
                    } else {
                        event.id.abs_diff(id) as u64
                    };
                    nearest.push((distance, event.id));
                    if nearest.len() > radius * 2 + 1 {
                        nearest.pop();
                        more = true;
                    }
                }
            }
            let mut ids = nearest.into_vec();
            ids.sort_by_key(|(_, id)| *id);
            for (_, id) in ids {
                if let Some(event) = s.event(id) {
                    rows.push(snippet(&event, matcher.as_ref(), state, subject_scope)?);
                }
            }
            Ok::<_, String>(())
        })??;
        rows.sort_by_key(|row| (row.timestamp, row.id));
    }
    Ok(
        json!({"center":id,"mode":mode,"subjectScope":subject_scope,"rows":rows,"more":more,"presentationFiltersIgnored":true,"visibilityRespected":true,"physicalUnit":"indexed_record"}),
    )
}
fn execute(state: &AppState, request: Request, filters: &[Filter]) -> Result<Value, String> {
    crate::workspace::validate(filters)?;
    match request {
        Request::Text { request } => {
            if analysis_runtime::current().is_some_and(|a| a.case_key.is_some()) {
                return Err("No Caso, abra o contexto dos eventos preservados. Texto direto usa o arquivo na Análise.".into());
            }
            if let Some(identity) = analysis_runtime::current().and_then(|a| a.identity.clone()) {
                let token = crate::operations::current_token();
                let visibility = crate::exclusion_store::visibility(
                    &crate::config_dir(),
                    &identity,
                    &Default::default(),
                    &crate::exclusion_store::Work {
                        cancelled: &|| token.cancelled(),
                        progress: &|_, _, _| {},
                    },
                )?;
                if visibility.has_active_batches() {
                    return Err("Existem exclusões neste Caso. O texto direto não pode contorná-las; use os eventos visíveis e seu contexto.".into());
                }
            }
            let response = crate::text_reader::execute(request)?;
            analysis_runtime::validate_result_owner()?;
            value(response)
        }
        Request::Schema => value(crate::sql_console::schema(state, filters)?),
        Request::Query { sql } => value(crate::sql_console::execute(state, filters, &sql)?),
        Request::QueryPage {
            result,
            offset,
            limit,
        } => value(crate::sql_console::page(&result, offset, limit)?),
        Request::QueryExport {
            result,
            path,
            format,
        } => value(crate::sql_console::export(&result, &path, &format)?),
        Request::Regex { pattern, text } => {
            if text.len() > crate::search_regex::SUBJECT_LIMIT {
                return Err("A amostra excede 8 MiB.".into());
            }
            value(Matcher::new(&pattern)?.find(&text)?)
        }
        Request::RegexPreview {
            pattern,
            text,
            scope,
        } => crate::regex_lab::preview(pattern, text, scope.as_deref() == Some("line")),
        Request::SampleText {
            id,
            event_ref,
            field,
        } => crate::regex_lab::sample_text(state, id, &event_ref, &field),
        Request::Highlights { rules, texts } => {
            if texts.len() > 160 || texts.iter().map(String::len).sum::<usize>() > 2 << 20 {
                return Err("A página visual excede o limite.".into());
            }
            let rules = crate::search_highlights::Compiled::new(&rules)?;
            value(
                texts
                    .iter()
                    .map(|text| rules.apply(text))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        Request::SearchStart { pattern, scope } => {
            value(crate::event_search::start(state, filters, pattern, scope)?)
        }
        Request::SearchStep { search } => value(crate::event_search::step(state, &search)?),
        Request::SearchPage { search, offset } => {
            value(crate::event_search::page(&search, offset)?)
        }
        Request::SaveField {
            name,
            source,
            definition,
        } => save(&name, &source, definition),
        Request::MapTemplate => value(crate::enrichment_maps::built_in()),
        Request::SaveMap { name, map_json } => {
            if map_json.len() > 256 << 10 {
                return Err("O mapa excede 256 KiB.".into());
            }
            let map = serde_json::from_str(&map_json).map_err(|e| e.to_string())?;
            save(
                &name,
                "raw",
                crate::advanced_fields::Definition::Map { map },
            )
        }
        Request::MapPreview { map_json, limit } => {
            if map_json.len() > 256 << 10 {
                return Err("O mapa excede 256 KiB.".into());
            }
            let map = serde_json::from_str(&map_json).map_err(|e| e.to_string())?;
            let compiled = crate::enrichment_maps::Compiled::new(map)?;
            let mut inspected = 0u64;
            let mut affected = 0u64;
            let mut conflicts = 0u64;
            let mut samples = Vec::new();
            let mut sample_bytes = 0usize;
            crate::workspace::with_selection(state, filters, |s| {
                for event in s.iter() {
                    crate::operations::check()?;
                    inspected += 1;
                    if let Some(enrichment) = compiled.evaluate(&event, None)? {
                        affected += 1;
                        conflicts += enrichment["_provenance"]["conflicts"]
                            .as_array()
                            .map_or(0, |a| a.len() as u64);
                        if samples.len() < limit.unwrap_or(12).min(30) {
                            let exact = serde_json::to_string_pretty(&enrichment)
                                .map_err(|e| e.to_string())?;
                            if sample_bytes + exact.len() <= 1 << 20 {
                                sample_bytes += exact.len();
                                samples.push(json!({"id":event.id,"event_ref":event.event_ref,"enrichmentJson":exact}));
                            }
                        }
                    }
                }
                Ok::<_, String>(())
            })??;
            Ok(
                json!({"inspected":inspected,"affected":affected,"conflicts":conflicts,"samples":samples,"sampleOnly":true,"countsComplete":true}),
            )
        }
        Request::MapCatalog => Ok(json!({
            "grok": crate::map_lab::catalog(),
            "templates": crate::map_templates::all(),
            "limits": {"mapBytes": 256 << 10, "rules": 128, "outputsPerRule": 64, "samples": 200},
        })),
        Request::MapCheck { map_json } => {
            if map_json.len() > 256 << 10 {
                return Err("O mapa excede 256 KiB.".into());
            }
            let map: crate::enrichment_maps::Definition =
                serde_json::from_str(&map_json).map_err(|e| format!("Definição inválida: {e}"))?;
            let diagnostics = crate::enrichment_maps::extraction_diagnostics(&map);
            let outputs = crate::enrichment_maps::outputs(&map);
            let error = crate::enrichment_maps::validate(&map).err();
            Ok(json!({"valid":error.is_none(),"error":error,"diagnostics":diagnostics,"outputs":outputs}))
        }
        Request::MapSample {
            sources,
            count,
            field,
            seed,
        } => crate::map_lab::sample(
            state,
            crate::map_lab::SampleRequest {
                sources,
                count,
                field,
                seed,
            },
        ),
        Request::MapGroups { sources, field, seed } => crate::map_lab::groups(
            state,
            crate::map_lab::SampleRequest {
                sources,
                count: None,
                field,
                seed,
            },
        ),
        Request::MapSuggest { texts } => crate::map_lab::suggest(&texts),
        Request::MapFrame { path, pattern, count } => {
            // Only files of the active source are read, and from the copy the reader actually opened.
            let requested = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
            let physical = {
                let source = analysis_runtime::source(state);
                match &*source {
                    SourceData::Indexed(index) => index
                        .parts
                        .iter()
                        .find(|p| std::fs::canonicalize(&p.path).ok().as_ref() == Some(&requested))
                        .map(|p| p.physical_path.clone()),
                    _ => None,
                }
            };
            let physical = physical.ok_or("O arquivo não pertence à fonte ativa.")?;
            crate::map_columns::frame(&physical, pattern.as_deref(), count.unwrap_or(60))
        }
        Request::MapVerify { texts, patterns } => crate::map_columns::verify(&texts, &patterns),
        Request::MapEvaluate { map_json, samples } => {
            crate::map_lab::evaluate_samples(state, &map_json, samples)
        }
        Request::MapTry { map_json, texts } => crate::map_lab::evaluate_texts(&map_json, &texts),
        Request::MapScan { map_json } => crate::map_lab::scan(state, filters, &map_json),
        Request::MapDiff {
            map_json,
            current_json,
            prefix,
            indexed,
        } => crate::map_lab::diff(current_json.as_deref(), &map_json, &prefix, &indexed),
        Request::Processes => value(crate::process_tree::build(state, filters)?),
        Request::ProcessPage {
            graph,
            parent,
            search,
            offset,
            selected,
        } => value(crate::process_tree::page(
            &graph,
            parent.as_deref(),
            search.as_deref().unwrap_or(""),
            offset.unwrap_or(0),
            selected.as_deref(),
        )?),
        Request::Context {
            id,
            event_ref,
            mode,
            radius,
            seconds,
            field,
            pattern,
            subject_scope,
        } => context(
            state,
            id,
            &event_ref,
            &mode,
            radius.unwrap_or(12),
            seconds.unwrap_or(120),
            field.as_deref(),
            pattern.as_ref(),
            subject_scope.as_deref().unwrap_or("event"),
        ),
        Request::ResolveText { path, offset } => {
            let offset = offset.parse::<u64>().map_err(|_| "Offset inválido.")?;
            let source = analysis_runtime::source(state);
            let SourceData::Indexed(index) = &*source else {
                return Err("Prepare os eventos deste arquivo primeiro.".into());
            };
            let requested = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
            let part = index
                .parts
                .iter()
                .find(|p| std::fs::canonicalize(&p.path).ok().as_ref() == Some(&requested))
                .ok_or("O arquivo não pertence à fonte ativa.")?;
            let offset = part.base.checked_add(offset).ok_or("Offset inválido.")?;
            let mut lo = 0;
            let mut hi = index.lines.len();
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if index.lines.at(mid).offset <= offset {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let id = lo.checked_sub(1).ok_or("Não há evento nesta posição.")?;
            let meta = index.lines.at(id);
            if index.part_at(id).identity != part.identity
                || offset >= meta.offset + meta.len as u64
                || !analysis_runtime::row_visible(index, id)?
            {
                return Err("Não há evento visível nesta posição.".into());
            }
            let codes = analysis_runtime::codes(state);
            let system = analysis_runtime::system_codes(state);
            let derived = analysis_runtime::derived(state);
            let event = crate::sources::event_at(index, id, &codes, &system, &derived);
            Ok(json!({"id":event.id,"event_ref":event.event_ref}))
        }
    }
}
#[tauri::command]
pub(crate) async fn local_analysis(
    request: Request,
    filters: Option<Vec<Filter>>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<Identity>,
    source_generation: Option<u64>,
    operation_id: Option<String>,
    app: AppHandle,
) -> Result<Value, String> {
    // These actions neither query nor publish the loaded dataset. In particular
    // direct text must work before the first structured source is published.
    // EditSource retains the source-generation pin without borrowing its rows.
    if matches!(
        &request,
        Request::Text { .. }
            | Request::Regex { .. }
            | Request::RegexPreview { .. }
            | Request::Highlights { .. }
            | Request::MapTemplate
            | Request::MapCatalog
            | Request::MapCheck { .. }
            | Request::MapSuggest { .. }
            | Request::MapFrame { .. }
            | Request::MapVerify { .. }
            | Request::MapTry { .. }
            | Request::MapDiff { .. }
            | Request::SaveField { .. }
            | Request::SaveMap { .. }
    ) {
        if matches!(&request, Request::Text { .. }) && (case_events.is_some() || case_key.is_some())
        {
            return Err("Abra o arquivo direto na Análise; nos Achados, use o contexto dos eventos preservados.".into());
        }
        let admitted = analysis_runtime::capture_async(
            app.clone(),
            analysis_context,
            source_generation,
            analysis_runtime::Mode::EditSource,
            operation_id.clone(),
            crate::global_scheduler::Priority::Interactive,
        )
        .await?;
        return crate::offload_admitted(operation_id, app.clone(), admitted, move || {
            execute(
                app.state::<AppState>().inner(),
                request,
                &filters.unwrap_or_default(),
            )
        })
        .await?;
    }
    let (admitted, events) = analysis_runtime::capture_case_async(
        app.clone(),
        analysis_context,
        source_generation,
        case_events,
        case_key,
        operation_id.clone(),
        crate::global_scheduler::Priority::Interactive,
    )
    .await?;
    crate::offload_case_interactive(operation_id, app.clone(), admitted, events, move |events| {
        analysis_runtime::with_visible_case(events, || {
            execute(
                app.state::<AppState>().inner(),
                request,
                &filters.unwrap_or_default(),
            )
        })
    })
    .await?
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn visible<T>(events: Vec<Event>, f: impl FnOnce(&AppState) -> T) -> T {
        struct Environment(Option<std::ffi::OsString>);
        impl Drop for Environment {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("LOGINSIGHT_DATA_DIR", value),
                    None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let _environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", directory.path());
        let state = crate::regression_tests::state_for(SourceData::Memory(events));
        crate::operations::run_with_token(crate::operations::token(None).unwrap(), || {
            let admitted =
                analysis_runtime::capture(&state, None, Some(0), analysis_runtime::Mode::Dataset)
                    .unwrap();
            analysis_runtime::with(Some(admitted.clone()), || {
                admitted.prepare_visibility(None).unwrap();
                f(&state)
            })
        })
        .unwrap()
    }
    pub(crate) fn event(id: usize, message: &str) -> Event {
        let mut event = Event::empty();
        event.id = id;
        event.event_ref = format!("fixture:{id}");
        event.timestamp = Some(1700000000000 + id as i64);
        event.message = message.into();
        event.raw = message.into();
        event
    }
    #[test]
    fn first_direct_open_uses_case_metadata_without_a_published_dataset() {
        visible(vec![], |state| {
            let mut document = crate::case_store::load().unwrap();
            document["cases"] = json!([{"id":"first-direct"}]);
            document["active"] = Value::Null;
            crate::case_store::save(document).unwrap();
            let identity = crate::analysis_context::snapshot("first-direct")
                .unwrap()
                .identity();
            assert!(analysis_runtime::capture(
                state,
                Some(identity.clone()),
                Some(0),
                analysis_runtime::Mode::Dataset
            )
            .is_err());
            let admitted = analysis_runtime::capture(
                state,
                Some(identity),
                Some(0),
                analysis_runtime::Mode::EditSource,
            )
            .unwrap();
            analysis_runtime::with(Some(admitted.clone()), || {
                admitted.prepare_visibility(None).unwrap();
                let directory = tempfile::tempdir().unwrap();
                let path = directory.path().join("first.log");
                std::fs::write(&path, "ready before parsing\n").unwrap();
                let response = execute(
                    state,
                    Request::Text {
                        request: crate::text_reader::Request {
                            action: "open".into(),
                            session: None,
                            path: Some(path.to_string_lossy().into_owned()),
                            pattern: None,
                            line: None,
                            offset: None,
                            limit: None,
                            matches_only: None,
                            byte_in_line: None,
                            rules: None,
                        },
                    },
                    &[],
                )
                .unwrap();
                assert_eq!(response["lines"][0]["text"], "ready before parsing");
                let session = response["status"]["session"].as_str().unwrap().to_owned();
                execute(
                    state,
                    Request::Text {
                        request: crate::text_reader::Request {
                            action: "close".into(),
                            session: Some(session),
                            path: None,
                            pattern: None,
                            line: None,
                            offset: None,
                            limit: None,
                            matches_only: None,
                            byte_in_line: None,
                            rules: None,
                        },
                    },
                    &[],
                )
                .unwrap();
            });
        });
    }
    #[test]
    fn sql_snapshot_pagination_exact_values_and_full_atomic_export() {
        let events = (0..241).map(|i| event(i, "log")).collect();
        visible(events, |state| {
            let first=crate::sql_console::execute(state,&[],"SELECT id, event_ref, 9007199254740993::BIGINT AS exact_integer, 1.230000000000000001::DECIMAL(30,18) AS exact_decimal FROM events ORDER BY id").unwrap();
            assert_eq!(first.total, "241");
            assert_eq!(first.rows.len(), 80);
            assert_eq!(first.rows[0][2].as_deref(), Some("9007199254740993"));
            assert_eq!(first.rows[0][3].as_deref(), Some("1.230000000000000001"));
            let last = crate::sql_console::page(&first.result, 240, 80).unwrap();
            assert_eq!(last.rows.len(), 1);
            assert_eq!(last.rows[0][0].as_deref(), Some("240"));
            let directory = tempfile::tempdir().unwrap();
            let output = directory.path().join("all.jsonl");
            assert_eq!(
                crate::sql_console::export(&first.result, output.to_str().unwrap(), "jsonl")
                    .unwrap(),
                241
            );
            let data = std::fs::read_to_string(output).unwrap();
            assert_eq!(data.lines().count(), 241);
            assert!(data.contains("9007199254740993"));
            let aggregate=crate::sql_console::execute(state,&[],"WITH counts AS (SELECT count(*) AS n FROM events) SELECT n, row_number() OVER() AS ord FROM counts").unwrap();
            assert_eq!(aggregate.rows[0][0].as_deref(), Some("241"));
        });
    }
    #[test]
    fn direct_text_opens_before_complete_scan_searches_late_match_and_pages_context() {
        visible(vec![], |_| {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("large.log");
            let mut file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
            use std::io::Write;
            for i in 0..200_000 {
                writeln!(
                    file,
                    "{i:06} {} {}",
                    "padding".repeat(8),
                    if i == 199_995 {
                        "needle ação"
                    } else {
                        "nothing"
                    }
                )
                .unwrap();
            }
            file.flush().unwrap();
            drop(file);
            let request =
                |action: &str,
                 session: Option<String>,
                 pattern: Option<Pattern>,
                 line: Option<u64>,
                 matches_only: bool| crate::text_reader::Request {
                    action: action.into(),
                    session,
                    path: Some(path.to_string_lossy().into_owned()),
                    pattern,
                    line,
                    offset: None,
                    limit: Some(30),
                    matches_only: Some(matches_only),
                    byte_in_line: None,
                    rules: None,
                };
            let opened =
                crate::text_reader::execute(request("open", None, None, None, false)).unwrap();
            let status = opened.status.unwrap();
            assert!(!status.index_complete);
            assert_eq!(opened.lines[0].line, 1);
            let id = status.session;
            let p = Pattern {
                engine: "pcre2".into(),
                text: "(?<=needle )ação".into(),
                case_sensitive: true,
                multiline: false,
                dot_all: false,
            };
            crate::text_reader::execute(request("search", Some(id.clone()), Some(p), None, true))
                .unwrap();
            let mut finished = false;
            for _ in 0..100 {
                let result = crate::text_reader::execute(request(
                    "step",
                    Some(id.clone()),
                    None,
                    None,
                    true,
                ))
                .unwrap();
                if result.status.as_ref().unwrap().search_complete {
                    assert_eq!(result.status.unwrap().matches, 1);
                    assert!(result.lines[0].text.contains("ação"));
                    assert!(!result.lines[0].spans.is_empty());
                    finished = true;
                    break;
                }
            }
            assert!(finished);
            let context = crate::text_reader::execute(request(
                "page",
                Some(id.clone()),
                None,
                Some(199_990),
                false,
            ))
            .unwrap();
            assert_eq!(context.lines[0].line, 199_990);
            crate::text_reader::execute(request("close", Some(id), None, None, false)).unwrap();
        });
    }
    #[test]
    fn incremental_event_search_preserves_sparse_ids_and_scope() {
        let mut first = event(900, "user=alice\nother");
        first.raw = "user=alice\nother".into();
        let second = event(2, "nothing");
        visible(vec![first, second], |state| {
            let p = Pattern {
                engine: "pcre2".into(),
                text: "(?<=user=)(?<user>\\w+)".into(),
                case_sensitive: true,
                multiline: false,
                dot_all: false,
            };
            let mut result = crate::event_search::start(state, &[], p, "message".into()).unwrap();
            while !result.complete {
                result = crate::event_search::step(state, &result.search).unwrap();
            }
            assert_eq!(result.matches, 1);
            assert_eq!(result.rows[0]["id"], 900);
        });
    }
    #[test]
    fn enormous_first_line_opens_in_bounded_wave_and_search_resumes_at_next_line() {
        visible(vec![], |_| {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("single.log");
            use std::io::Write;
            let mut file = std::fs::File::create(&path).unwrap();
            for _ in 0..3 {
                file.write_all(&vec![b'x'; 8 << 20]).unwrap();
            }
            file.write_all(b"\nneedle\n").unwrap();
            drop(file);
            let run = |request: Value| {
                crate::text_reader::execute(serde_json::from_value(request).unwrap()).unwrap()
            };
            let opened = run(json!({"action":"open","path":path}));
            let status = opened.status.unwrap();
            assert!(!status.index_complete);
            assert_eq!(status.indexed_bytes, (8 << 20).to_string());
            assert_eq!(opened.lines[0].line, 1);
            assert!(opened.lines[0].truncated_after);
            assert!(!opened.lines[0].matches_complete);
            let id = status.session;
            run(
                json!({"action":"search","session":id,"pattern":{"engine":"rust","text":"^needle$","caseSensitive":true}}),
            );
            let mut complete = false;
            for _ in 0..8 {
                let page = run(json!({"action":"step","session":id,"matchesOnly":true}));
                let status = page.status.unwrap();
                if status.search_complete {
                    assert_eq!(status.skipped_records, 1);
                    assert_eq!(status.matches, 1);
                    assert_eq!(page.lines[0].line, 2);
                    assert_eq!(page.lines[0].text, "needle");
                    complete = true;
                    break;
                }
            }
            assert!(complete);
            run(json!({"action":"close","session":id}));
        });
    }
    #[test]
    fn advanced_map_and_capture_fields_are_reversible_and_queryable() {
        let mut e = event(0, "user=alice");
        e.fields=serde_json::from_value(json!({"Channel":"Microsoft-Windows-Sysmon/Operational","ProcessId":"0x2a","Computer":"A"})).unwrap();
        e.code = "1".into();
        e.source = "Microsoft-Windows-Sysmon".into();
        let original = e.fields.clone();
        let map = crate::advanced_fields::Compiled::new(crate::advanced_fields::Definition::Map {
            map: crate::enrichment_maps::built_in(),
        })
        .unwrap();
        let capture =
            crate::advanced_fields::Compiled::new(crate::advanced_fields::Definition::Captures {
                pattern: Pattern {
                    engine: "pcre2".into(),
                    text: "(?<=user=)(?<user>\\w+)".into(),
                    case_sensitive: true,
                    multiline: false,
                    dot_all: false,
                },
                capture: "user".into(),
            })
            .unwrap();
        let derived = vec![
            crate::sources::CompiledDerived {
                name: "norm".into(),
                source: "raw".into(),
                rules: vec![],
                steps: vec![],
                lookup: None,
                advanced: Some(map),
            },
            crate::sources::CompiledDerived {
                name: "extracted".into(),
                source: "message".into(),
                rules: vec![],
                steps: vec![],
                lookup: None,
                advanced: Some(capture),
            },
        ];
        crate::sources::apply_derived(&mut e, &derived);
        assert_eq!(e.fields["norm.process.pid"], 42);
        assert_eq!(e.fields["extracted"], "alice");
        assert_eq!(e.fields["ProcessId"], original["ProcessId"]);
        crate::sources::apply_derived(&mut e, &derived);
        assert!(e.derived_diagnostics.is_empty());
        crate::sources::apply_derived(&mut e, &[]);
        assert_eq!(e.fields, original);
    }
    #[test]
    fn admitted_case_payload_drives_sql_and_context_then_restores_dataset() {
        visible(vec![event(0, "dataset"), event(1, "dataset")], |state| {
            analysis_runtime::with_visible_case(Some(vec![event(900, "case-only")]), || {
                let page = crate::sql_console::execute(state, &[], "SELECT id,message FROM events")
                    .unwrap();
                assert_eq!(page.total, "1");
                assert_eq!(page.rows[0][0].as_deref(), Some("900"));
                assert_eq!(page.rows[0][1].as_deref(), Some("case-only"));
                let rows = context(
                    state,
                    900,
                    "fixture:900",
                    "temporal",
                    2,
                    120,
                    None,
                    None,
                    "message",
                )
                .unwrap();
                assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
                assert_eq!(rows["rows"][0]["id"], 900);
            });
            let page = crate::sql_console::execute(state, &[], "SELECT count(*) AS n FROM events")
                .unwrap();
            assert_eq!(page.rows[0][0].as_deref(), Some("2"));
        });
    }

    #[test]
    fn expression_filter_scopes_sql_and_extracted_fields_stay_filterable() {
        let messages = ["user=ana ok", "user=joao ok", "nothing here", "user=ana again"];
        let mut events: Vec<Event> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| event(i, m))
            .collect();
        let capture =
            crate::advanced_fields::Compiled::new(crate::advanced_fields::Definition::Captures {
                pattern: Pattern {
                    engine: "pcre2".into(),
                    text: r"user=(?<usuario>\w+)".into(),
                    case_sensitive: true,
                    multiline: false,
                    dot_all: false,
                },
                capture: "usuario".into(),
            })
            .unwrap();
        let derived = vec![crate::sources::CompiledDerived {
            name: "usuario".into(),
            source: "message".into(),
            rules: vec![],
            steps: vec![],
            lookup: None,
            advanced: Some(capture),
        }];
        for e in &mut events {
            crate::sources::apply_derived(e, &derived);
        }
        // The original text stays; the extracted field exists only where the pattern matched.
        assert_eq!(events[0].message, "user=ana ok");
        assert_eq!(events[0].fields["usuario"], "ana");
        assert!(events[2].fields.get("usuario").is_none());
        let search = Filter {
            column: "message".into(),
            op: "search".into(),
            value: r"(?<=user=)ana".into(),
            value2: Some(r#"{"engine":"pcre2","caseSensitive":true}"#.into()),
        };
        crate::workspace::validate(std::slice::from_ref(&search)).unwrap();
        let invalid = Filter {
            value: r"(?<=a+)b".into(),
            ..search.clone()
        };
        assert!(crate::workspace::validate(&[invalid]).is_err());
        // The saved capture is an ordinary field for filters (SQL over an indexed source is
        // checked natively by scripts/preview/test-local-native.mjs).
        let by_field = Filter {
            column: "usuario".into(),
            op: "equals_exact".into(),
            value: "joao".into(),
            value2: None,
        };
        assert!(crate::query::matches_filter(&events[1], &by_field));
        assert!(!crate::query::matches_filter(&events[0], &by_field));
        visible(events, |state| {
            let count = |filters: &[Filter], sql: &str| {
                crate::sql_console::execute(state, filters, sql).unwrap().rows[0][0].clone()
            };
            // "Apply current filters" scopes SQL to the same rows a page of Eventos shows.
            assert_eq!(count(&[], "SELECT count(*) FROM events").as_deref(), Some("4"));
            assert_eq!(
                count(std::slice::from_ref(&search), "SELECT count(*) FROM events").as_deref(),
                Some("2")
            );
            // A raw-field expression selects by its own subject.
            let raw = Filter {
                column: "raw".into(),
                op: "search".into(),
                value: r"^nothing".into(),
                value2: Some(r#"{"engine":"rust","caseSensitive":true}"#.into()),
            };
            assert_eq!(
                count(&[raw], "SELECT count(*) FROM events").as_deref(),
                Some("1")
            );
        });
    }
}
