//! Backend of the "Extração e normalização" workbench: representative sampling, local
//! explainable suggestions, sample evaluation with real ranges, whole-set verification and
//! the effect of an edit. Everything runs in the native process, in bounded batches, over
//! the admitted source; nothing here modifies an original file or sends data anywhere.
use crate::{
    enrichment_maps::{self, Compiled, Trace},
    query::Filter,
    AppState, SourceData,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

const SAMPLE_TEXT_LIMIT: usize = 16 << 10;
const POOL: usize = 3000;
const MAX_SAMPLES: usize = 200;

fn cut(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}
fn utf16(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or(0, |s| s.encode_utf16().count())
}
/// A coarse "shape" of a record: letters and digits collapse so that records written by the
/// same template share one shape and atypical records stand apart.
fn shape(text: &str) -> String {
    let mut out = String::new();
    let mut last = ' ';
    for c in text.chars().take(160) {
        let class = if c.is_ascii_digit() { '0' } else if c.is_alphabetic() { 'a' } else if c.is_whitespace() { ' ' } else { c };
        if class != last || !matches!(class, '0' | 'a' | ' ') {
            out.push(class);
        }
        last = class;
        if out.len() >= 64 {
            break;
        }
    }
    out
}
fn subject(event: &crate::model::Event, field: &str) -> Option<String> {
    match field {
        "message" => Some(event.message.clone()),
        "raw" => Some(if event.raw.is_empty() { event.message.clone() } else { event.raw.clone() }),
        other => event.col_ref(other).map(|v| v.into_owned()),
    }
}
fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ------------------------------------------------------------------ sampling
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SampleRequest {
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub seed: Option<u64>,
}
struct Candidate {
    id: usize,
    part: usize,
    event_ref: String,
    text: String,
    shape: String,
    offset: u64,
}
pub(crate) fn sample(state: &AppState, request: SampleRequest) -> Result<Value, String> {
    let count = request.count.unwrap_or(40).clamp(1, MAX_SAMPLES);
    let field = request.field.unwrap_or_else(|| "message".into());
    if field.is_empty() || field.len() > 512 {
        return Err("Escolha o campo de origem das amostras.".into());
    }
    let seed = request.seed.unwrap_or(1);
    let source = crate::analysis_runtime::source(state);
    let (ranges, names, total) = match &*source {
        SourceData::Indexed(idx) => {
            let n = idx.lines.len();
            // Rows are in source order, so each file owns one contiguous range of ids.
            let starts: Vec<usize> = idx
                .parts
                .iter()
                .map(|p| {
                    let (mut lo, mut hi) = (0usize, n);
                    while lo < hi {
                        let mid = (lo + hi) / 2;
                        if idx.lines.at(mid).offset < p.base { lo = mid + 1 } else { hi = mid }
                    }
                    lo
                })
                .collect();
            let ranges: Vec<(usize, usize)> = (0..idx.parts.len()).map(|k| (starts[k], starts.get(k + 1).copied().unwrap_or(n))).collect();
            (ranges, idx.parts.iter().map(|p| (p.path.clone(), p.file_name.clone())).collect::<Vec<_>>(), n)
        }
        SourceData::Memory(events) => (vec![(0, events.len())], vec![("Registros do Caso".to_owned(), "Registros do Caso".to_owned())], events.len()),
        SourceData::None => return Err("Abra fontes na análise para obter amostras reais. Os mapas são salvos no Caso ativo.".into()),
    };
    let chosen: Vec<usize> = (0..ranges.len())
        .filter(|k| ranges[*k].1 > ranges[*k].0 && request.sources.as_ref().is_none_or(|s| s.contains(&names[*k].0)))
        .collect();
    if chosen.is_empty() {
        return Err("Nenhuma das fontes escolhidas tem registros.".into());
    }
    let per_part = (POOL / chosen.len()).max(8);
    let mut pool: Vec<Candidate> = Vec::new();
    let mut without_subject = 0usize;
    let mut candidate_ids = 0usize;
    crate::workspace::with_selection(state, &[], |s| {
        for &part in &chosen {
            let (lo, hi) = ranges[part];
            let span = hi - lo;
            let take = per_part.min(span);
            for j in 0..take {
                crate::operations::check()?;
                // Evenly spaced positions with a deterministic jitter inside each stride.
                let stride = (span / take).max(1);
                let id = lo + (j * span / take + (splitmix(seed ^ ((part as u64) << 32) ^ j as u64) as usize % stride)).min(span - 1);
                candidate_ids += 1;
                let Some(event) = s.event(id) else { continue };
                let Some(text) = subject(&event, &field) else {
                    without_subject += 1;
                    continue;
                };
                let offset = match &*source {
                    SourceData::Indexed(idx) => idx.lines.at(id).offset - idx.part_at(id).base,
                    _ => 0,
                };
                pool.push(Candidate { id, part, event_ref: event.event_ref.clone(), shape: shape(&text), text, offset });
            }
        }
        Ok::<_, String>(())
    })??;
    if pool.is_empty() {
        return Err(format!("O campo “{field}” não existe nos registros sorteados."));
    }
    // Shapes by frequency: most samples follow the predominant formats, a share is reserved
    // for the rarest ones so that exceptions are not hidden.
    let mut by_shape: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, c) in pool.iter().enumerate() {
        by_shape.entry(c.shape.as_str()).or_default().push(i);
    }
    let mut shapes: Vec<(&str, &Vec<usize>)> = by_shape.iter().map(|(k, v)| (*k, v)).collect();
    shapes.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let rare_quota = (count * 3).div_ceil(10).min(count);
    let mut picked: Vec<usize> = Vec::new();
    let mut taken = vec![false; pool.len()];
    fn take(taken: &mut [bool], i: usize, picked: &mut Vec<usize>, count: usize) {
        if !taken[i] && picked.len() < count {
            taken[i] = true;
            picked.push(i);
        }
    }
    // Rare first (shapes seen in at most 2% of the pool, rarest first, spread across files).
    let rare_limit = (pool.len() / 50).max(2);
    for (_, members) in shapes.iter().rev().filter(|(_, m)| m.len() <= rare_limit) {
        if picked.len() >= rare_quota {
            break;
        }
        take(&mut taken, members[0], &mut picked, count);
    }
    // Then round-robin over the predominant shapes, alternating files.
    let mut round = 0usize;
    while picked.len() < count && round < pool.len() {
        let mut progressed = false;
        for (_, members) in &shapes {
            if let Some(&i) = members.get(round) {
                if !taken[i] {
                    take(&mut taken, i, &mut picked, count);
                    progressed = true;
                }
            }
            if picked.len() >= count {
                break;
            }
        }
        if !progressed && round > 0 && picked.len() >= pool.len() {
            break;
        }
        round += 1;
    }
    picked.sort_by_key(|&i| (pool[i].part, pool[i].id));
    let mut selected_by_part: HashMap<usize, usize> = HashMap::new();
    let samples: Vec<Value> = picked
        .iter()
        .map(|&i| {
            let c = &pool[i];
            *selected_by_part.entry(c.part).or_default() += 1;
            let (text, truncated) = cut(&c.text, SAMPLE_TEXT_LIMIT);
            let rank = shapes.iter().position(|(k, _)| *k == c.shape).unwrap_or(0);
            json!({"id":c.id,"eventRef":c.event_ref,"file":names[c.part].0,"fileName":names[c.part].1,"offset":c.offset.to_string(),"text":text,"truncated":truncated,"bytes":c.text.len(),"shape":c.shape,"shapeRank":rank,"shapeCount":shapes[rank].1.len()})
        })
        .collect();
    let distribution: Vec<Value> = chosen
        .iter()
        .map(|&k| json!({"file":names[k].0,"fileName":names[k].1,"records":ranges[k].1 - ranges[k].0,"selected":selected_by_part.get(&k).copied().unwrap_or(0)}))
        .collect();
    let top_shapes: Vec<Value> = shapes.iter().take(8).map(|(k, v)| json!({"shape":k,"count":v.len()})).collect();
    Ok(json!({
        "isSample": true,
        "population": total,
        "candidates": candidate_ids,
        "examined": pool.len(),
        "withoutSubject": without_subject,
        "field": field,
        "seed": seed,
        "strategy": format!("Amostra estratificada e repetível: {} registros examinados, sorteados em posições espaçadas de {} arquivo(s) entre {} registros. Cerca de 70% dos exemplos seguem os formatos predominantes; cerca de 30% vêm dos formatos mais raros.", pool.len(), chosen.len(), total),
        "samples": samples,
        "distribution": distribution,
        "shapes": top_shapes,
    }))
}

// ------------------------------------------------------------------ evaluation
fn parse_map(map_json: &str) -> Result<enrichment_maps::Definition, String> {
    if map_json.len() > 256 << 10 {
        return Err("O mapa excede 256 KiB.".into());
    }
    serde_json::from_str(map_json).map_err(|e| format!("Definição inválida: {e}"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SampleRef {
    pub id: usize,
    pub event_ref: String,
}
/// The draft applied to the sample records: per record, how every rule and output behaved,
/// with the real position of each extracted value.
pub(crate) fn evaluate_samples(state: &AppState, map_json: &str, ids: Vec<SampleRef>) -> Result<Value, String> {
    if ids.len() > 200 {
        return Err("Avalie até 200 amostras por vez.".into());
    }
    let map = parse_map(map_json)?;
    let diagnostics = enrichment_maps::extraction_diagnostics(&map);
    let compiled = match Compiled::new(map) {
        Ok(c) => c,
        Err(error) => return Ok(json!({"ok":false,"error":error,"diagnostics":diagnostics,"scope":"sample"})),
    };
    let mut results = Vec::new();
    crate::workspace::with_selection(state, &[], |s| {
        for reference in &ids {
            crate::operations::check()?;
            let Some(event) = s.event(reference.id) else {
                results.push(json!({"id":reference.id,"error":"Registro ausente ou excluído."}));
                continue;
            };
            if event.event_ref != reference.event_ref {
                results.push(json!({"id":reference.id,"error":"A referência do registro mudou."}));
                continue;
            }
            let mut one = run_one(&compiled, &event);
            one["id"] = json!(reference.id);
            results.push(one);
        }
        Ok::<_, String>(())
    })??;
    Ok(summarize(results, diagnostics))
}
/// Pasted or template lines evaluated as if they were records (message and raw text), so a
/// definition can be tried without any loaded source. Nothing is stored.
pub(crate) fn evaluate_texts(map_json: &str, texts: &[String]) -> Result<Value, String> {
    if texts.is_empty() || texts.len() > 200 || texts.iter().map(String::len).sum::<usize>() > 2 << 20 {
        return Err("Informe de 1 a 200 linhas (até 2 MiB no total).".into());
    }
    let map = parse_map(map_json)?;
    let diagnostics = enrichment_maps::extraction_diagnostics(&map);
    let compiled = match Compiled::new(map) {
        Ok(c) => c,
        Err(error) => return Ok(json!({"ok":false,"error":error,"diagnostics":diagnostics,"scope":"text"})),
    };
    let results = texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            crate::operations::check()?;
            let mut event = crate::model::Event::empty();
            event.message = text.clone();
            event.raw = text.clone();
            let mut one = run_one(&compiled, &event);
            one["id"] = json!(i);
            Ok(one)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut summary = summarize(results, diagnostics);
    summary["scope"] = json!("text");
    Ok(summary)
}
fn run_one(compiled: &Compiled, event: &crate::model::Event) -> Value {
    let mut trace = Trace { detail: true, ..Default::default() };
    let outcome = compiled.evaluate(event, Some(&mut trace));
    let (fields, error) = match outcome {
        Ok(Some(Value::Object(mut o))) => {
            o.remove("_provenance");
            (Value::Object(o), None)
        }
        Ok(_) => (json!({}), None),
        Err(e) => (json!({}), Some(e)),
    };
    json!({"eventRef":event.event_ref,"fields":fields,"error":error,"trace":trace})
}
fn summarize(results: Vec<Value>, diagnostics: Vec<Value>) -> Value {
    let mut summary: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    let mut rules: BTreeMap<String, [usize; 4]> = BTreeMap::new();
    for r in &results {
        for o in r["trace"]["outputs"].as_array().into_iter().flatten() {
            let key = format!("{}\u{1f}{}", o["rule"].as_str().unwrap_or_default(), o["field"].as_str().unwrap_or_default());
            let status = o["status"].as_str().unwrap_or("ok");
            let status: &'static str = ["ok", "missing", "empty", "invalid", "default", "kept_text", "overridden", "rejected"].into_iter().find(|s| *s == status).unwrap_or("ok");
            *summary.entry(key).or_default().entry(status).or_default() += 1;
        }
        for t in r["trace"]["rules"].as_array().into_iter().flatten() {
            let e = rules.entry(t["rule"].as_str().unwrap_or_default().to_owned()).or_default();
            if t["eligible"] == true {
                e[0] += 1;
                match t["extraction"].as_str() {
                    Some("recognized") => e[1] += 1,
                    Some("not_recognized") => e[2] += 1,
                    Some("error") => e[3] += 1,
                    _ => {}
                }
            }
        }
    }
    json!({
        "ok": true,
        "scope": "sample",
        "inspected": results.len(),
        "diagnostics": diagnostics,
        "results": results,
        "rules": rules.into_iter().map(|(rule, c)| json!({"rule":rule,"eligible":c[0],"recognized":c[1],"notRecognized":c[2],"errors":c[3]})).collect::<Vec<_>>(),
        "outputs": summary.into_iter().map(|(k, v)| { let (rule, field) = k.split_once('\u{1f}').unwrap_or(("", "")); json!({"rule":rule,"field":field,"states":v}) }).collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------------------ whole-set verification
#[derive(Default)]
struct RuleStats {
    eligible: u64,
    recognized: u64,
    not_recognized: u64,
    errors: u64,
    first_error: Option<String>,
    by_alternative: BTreeMap<usize, u64>,
}
#[derive(Default)]
struct FileStats {
    inspected: u64,
    affected: u64,
    recognized: u64,
    not_recognized: u64,
}
/// A complete, cancellable pass over the chosen cut (filters) of the loaded records.
pub(crate) fn scan(state: &AppState, filters: &[Filter], map_json: &str) -> Result<Value, String> {
    let map = parse_map(map_json)?;
    let diagnostics = enrichment_maps::extraction_diagnostics(&map);
    let compiled = Compiled::new(map)?;
    let source = crate::analysis_runtime::source(state);
    let total = match &*source {
        SourceData::Indexed(idx) => idx.lines.len(),
        SourceData::Memory(events) => events.len(),
        SourceData::None => 0,
    };
    let (mut inspected, mut affected, mut conflicts) = (0u64, 0u64, 0u64);
    let mut rules: BTreeMap<String, RuleStats> = BTreeMap::new();
    let mut outputs: BTreeMap<(String, String), BTreeMap<String, u64>> = BTreeMap::new();
    let mut files: BTreeMap<String, FileStats> = BTreeMap::new();
    let mut successes: Vec<Value> = Vec::new();
    let mut failures: Vec<Value> = Vec::new();
    let started = std::time::Instant::now();
    crate::workspace::with_selection(state, filters, |s| {
        for event in s.iter() {
            crate::operations::check()?;
            inspected += 1;
            let file = match &*source {
                SourceData::Indexed(idx) if event.id < idx.lines.len() => idx.part_at(event.id).path.clone(),
                _ => event.source.clone(),
            };
            let mut trace = Trace::default();
            let outcome = compiled.evaluate(&event, Some(&mut trace));
            let file_stats = files.entry(file.clone()).or_default();
            file_stats.inspected += 1;
            let mut failed: Option<String> = None;
            for t in &trace.rules {
                if !t.eligible {
                    continue;
                }
                let r = rules.entry(t.rule.clone()).or_default();
                r.eligible += 1;
                match t.extraction {
                    "recognized" => {
                        r.recognized += 1;
                        file_stats.recognized += 1;
                        if let Some(a) = t.alternative {
                            *r.by_alternative.entry(a).or_default() += 1;
                        }
                    }
                    "not_recognized" => {
                        r.not_recognized += 1;
                        file_stats.not_recognized += 1;
                        failed.get_or_insert(format!("A regra {} não reconheceu o texto.", t.rule));
                    }
                    "error" => {
                        r.errors += 1;
                        r.first_error.get_or_insert_with(|| t.error.clone().unwrap_or_default());
                        failed.get_or_insert(t.error.clone().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            for o in &trace.outputs {
                *outputs.entry((o.rule.clone(), o.field.clone())).or_default().entry(o.status.to_owned()).or_default() += 1;
                if o.status == "invalid" {
                    failed.get_or_insert(format!("{}: {}", o.field, o.reason.clone().unwrap_or_default()));
                }
            }
            conflicts += trace.conflicts.len() as u64;
            match outcome {
                Ok(Some(_)) => {
                    affected += 1;
                    file_stats.affected += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    failed.get_or_insert(e);
                }
            }
            let offset = match &*source {
                SourceData::Indexed(idx) if event.id < idx.lines.len() => Some((idx.lines.at(event.id).offset - idx.part_at(event.id).base).to_string()),
                _ => None,
            };
            let excerpt = |e: &crate::model::Event| cut(if e.message.is_empty() { &e.raw } else { &e.message }, 300).0.to_owned();
            if let Some(reason) = failed {
                if failures.len() < 40 {
                    failures.push(json!({"id":event.id,"eventRef":event.event_ref,"file":file,"offset":offset,"reason":reason,"text":excerpt(&event)}));
                }
            } else if successes.len() < 40 && trace.outputs.iter().any(|o| o.status == "ok") {
                successes.push(json!({"id":event.id,"eventRef":event.event_ref,"file":file,"offset":offset,"text":excerpt(&event)}));
            }
            if inspected % 2048 == 0 {
                crate::operations::progress("map-scan", "Conferindo o conjunto", inspected as usize, total, affected as usize);
            }
        }
        Ok::<_, String>(())
    })??;
    crate::operations::progress("map-scan", "Conferência concluída", inspected as usize, total, affected as usize);
    Ok(json!({
        "ok": true,
        "scope": "complete",
        "countsComplete": true,
        "inspected": inspected,
        "affected": affected,
        "conflicts": conflicts,
        "filterCount": filters.len(),
        "elapsedMs": started.elapsed().as_millis() as u64,
        "diagnostics": diagnostics,
        "rules": rules.into_iter().map(|(rule, r)| json!({"rule":rule,"eligible":r.eligible,"recognized":r.recognized,"notRecognized":r.not_recognized,"errors":r.errors,"firstError":r.first_error,"byAlternative":r.by_alternative})).collect::<Vec<_>>(),
        "outputs": outputs.into_iter().map(|((rule, field), states)| json!({"rule":rule,"field":field,"states":states})).collect::<Vec<_>>(),
        "files": files.into_iter().map(|(file, f)| json!({"file":file,"inspected":f.inspected,"affected":f.affected,"recognized":f.recognized,"notRecognized":f.not_recognized})).collect::<Vec<_>>(),
        "successes": successes,
        "failures": failures,
    }))
}

// ------------------------------------------------------------------ effect of an edit
fn rule_view(rule: &enrichment_maps::Rule) -> Value {
    let mut v = serde_json::to_value(rule).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("description");
    }
    v
}
/// What changes between the installed definition and the draft, without running either.
/// `indexed` lists the output fields (with the map's prefix) that currently have an index.
pub(crate) fn diff(current: Option<&str>, next: &str, prefix: &str, indexed: &[String]) -> Result<Value, String> {
    let next = parse_map(next)?;
    let current = current.map(parse_map).transpose()?;
    let a: BTreeMap<String, Vec<enrichment_maps::OutputInfo>> = current.as_ref().map_or_else(BTreeMap::new, |m| group(enrichment_maps::outputs(m)));
    let b = group(enrichment_maps::outputs(&next));
    let (mut added, mut removed, mut changed, mut unchanged) = (vec![], vec![], vec![], vec![]);
    for (field, infos) in &b {
        match a.get(field) {
            None => added.push(field.clone()),
            Some(old) if old.iter().map(|i| &i.signature).collect::<Vec<_>>() == infos.iter().map(|i| &i.signature).collect::<Vec<_>>() => unchanged.push(field.clone()),
            Some(_) => changed.push(field.clone()),
        }
    }
    for field in a.keys() {
        if !b.contains_key(field) {
            removed.push(field.clone());
        }
    }
    let old_rules: BTreeMap<String, Value> = current.as_ref().map_or_else(BTreeMap::new, |m| m.rules.iter().map(|r| (r.id.clone(), rule_view(r))).collect());
    let new_rules: BTreeMap<String, Value> = next.rules.iter().map(|r| (r.id.clone(), rule_view(r))).collect();
    let rules_added: Vec<&String> = new_rules.keys().filter(|k| !old_rules.contains_key(*k)).collect();
    let rules_removed: Vec<&String> = old_rules.keys().filter(|k| !new_rules.contains_key(*k)).collect();
    let rules_changed: Vec<&String> = new_rules.iter().filter(|(k, v)| old_rules.get(*k).is_some_and(|o| o != *v)).map(|(k, _)| k).collect();
    let recompute = current.as_ref().is_none_or(|c| enrichment_maps::semantic_signature(c) != enrichment_maps::semantic_signature(&next));
    // A change to enabling or to the shared patterns touches every output; otherwise only the
    // outputs whose computation changed lose their index.
    let global = current.as_ref().is_some_and(|c| c.enabled != next.enabled || c.grok_patterns != next.grok_patterns || c.id != next.id || c.version != next.version);
    let touched: BTreeSet<String> = added.iter().chain(&removed).chain(&changed).map(|f| format!("{prefix}.{f}")).collect();
    let rebuild: Vec<&String> = indexed.iter().filter(|f| recompute && (global || touched.contains(*f))).collect();
    Ok(json!({
        "recompute": recompute,
        "global": global,
        "added": added, "removed": removed, "changed": changed, "unchanged": unchanged,
        "rulesAdded": rules_added, "rulesRemoved": rules_removed, "rulesChanged": rules_changed,
        "indexesToRebuild": rebuild,
        "metadataOnly": !recompute && current.is_some(),
    }))
}
fn group(outputs: Vec<enrichment_maps::OutputInfo>) -> BTreeMap<String, Vec<enrichment_maps::OutputInfo>> {
    let mut map: BTreeMap<String, Vec<enrichment_maps::OutputInfo>> = BTreeMap::new();
    for o in outputs {
        map.entry(o.field.clone()).or_default().push(o);
    }
    map
}

// ------------------------------------------------------------------ suggestions
struct Recognizer {
    id: &'static str,
    label: &'static str,
    why: &'static str,
    /// A catalog pattern, or a composition of catalog patterns.
    expression: &'static str,
    field: &'static str,
    regex: &'static LazyLock<Regex>,
    validate: fn(&str) -> bool,
}
macro_rules! rx {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($re).expect("static regex"));
    };
}
rx!(IPV4, r"\b(?:\d{1,3}\.){3}\d{1,3}\b");
rx!(IPV6, r"\b(?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f]{1,4}\b");
rx!(ISO, r"\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?");
rx!(HTTPD, r"\b\d{2}/[A-Z][a-z]{2}/\d{4}:\d{2}:\d{2}:\d{2} [+-]\d{4}");
rx!(SYSLOG, r"\b[A-Z][a-z]{2} +\d{1,2} \d{2}:\d{2}:\d{2}\b");
rx!(SLASHDATE, r"\b\d{2}/\d{2}/\d{4} \d{2}:\d{2}:\d{2}\b");
rx!(UUID, r"\b[0-9A-Fa-f]{8}-(?:[0-9A-Fa-f]{4}-){3}[0-9A-Fa-f]{12}\b");
rx!(MAC, r"\b(?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}\b");
rx!(EMAIL, r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}");
rx!(URL, r#"\b[A-Za-z][A-Za-z0-9+.-]*://[^\s"'<>]+"#);
rx!(WINP, r#"\b[A-Za-z]:\\[^\s"'<>|]+"#);
rx!(UNIXP, r"(?:/[A-Za-z0-9._-]+){2,}");
rx!(HEXN, r"\b0[xX][0-9A-Fa-f]+\b");
rx!(DECIMAL, r"-?\b\d+\.\d+\b");
rx!(QUOTED, r#""[^"\n]{1,200}""#);
rx!(KV, r#"\b([A-Za-z_][\w.-]{0,63})=("[^"]*"|[^\s,;]+)"#);
static RECOGNIZERS: &[Recognizer] = &[
    Recognizer { id: "ip", label: "Endereço IP", why: "Parece um endereço IPv4 ou IPv6.", expression: "IP", field: "ip", regex: &IPV4, validate: |t| t.parse::<std::net::Ipv4Addr>().is_ok() },
    Recognizer { id: "ipv6", label: "Endereço IPv6", why: "Parece um endereço IPv6.", expression: "IP", field: "ip", regex: &IPV6, validate: |t| t.parse::<std::net::Ipv6Addr>().is_ok() },
    Recognizer { id: "iso8601", label: "Data e hora ISO 8601", why: "Data no formato 2026-10-23T17:05:09.", expression: "TIMESTAMP_ISO8601", field: "timestamp", regex: &ISO, validate: |_| true },
    Recognizer { id: "httpdate", label: "Data de log HTTP", why: "Data no formato 23/Oct/2026:17:05:09 +0000.", expression: "HTTPDATE", field: "timestamp", regex: &HTTPD, validate: |_| true },
    Recognizer { id: "syslogdate", label: "Data de syslog", why: "Data no formato Oct 23 17:05:09 (sem ano).", expression: "SYSLOGTIMESTAMP", field: "timestamp", regex: &SYSLOG, validate: |_| true },
    Recognizer { id: "slashdate", label: "Data e hora com barras", why: "Data no formato 23/10/2026 17:05:09 (dia/mês ou mês/dia: confirme na conversão).", expression: "%{MONTHDAY}/%{MONTHNUM}/%{YEAR} %{TIME}", field: "timestamp", regex: &SLASHDATE, validate: |_| true },
    Recognizer { id: "uuid", label: "Identificador UUID", why: "Identificador no formato 8-4-4-4-12.", expression: "UUID", field: "id", regex: &UUID, validate: |_| true },
    Recognizer { id: "mac", label: "Endereço MAC", why: "Endereço físico de interface de rede.", expression: "MAC", field: "mac", regex: &MAC, validate: |_| true },
    Recognizer { id: "email", label: "Endereço de e-mail", why: "Texto no formato nome@domínio.", expression: "EMAILADDRESS", field: "email", regex: &EMAIL, validate: |_| true },
    Recognizer { id: "url", label: "URL", why: "Endereço com esquema (http://, https://…).", expression: "URI", field: "url", regex: &URL, validate: |_| true },
    Recognizer { id: "winpath", label: "Caminho do Windows", why: "Caminho começando por uma unidade (C:\\…).", expression: "WINPATH", field: "caminho", regex: &WINP, validate: |_| true },
    Recognizer { id: "unixpath", label: "Caminho Unix", why: "Caminho com duas ou mais pastas (/a/b).", expression: "UNIXPATH", field: "caminho", regex: &UNIXP, validate: |_| true },
    Recognizer { id: "hex", label: "Número hexadecimal", why: "Número com prefixo 0x (por exemplo, um PID).", expression: "HEX", field: "valor_hex", regex: &HEXN, validate: |_| true },
    Recognizer { id: "decimal", label: "Número decimal", why: "Número com parte fracionária.", expression: "NUMBER", field: "valor", regex: &DECIMAL, validate: |_| true },
    Recognizer { id: "quoted", label: "Texto entre aspas", why: "Texto delimitado por aspas duplas.", expression: "QUOTEDSTRING", field: "texto", regex: &QUOTED, validate: |_| true },
];

fn examples(values: &BTreeSet<String>) -> Vec<&String> {
    values.iter().take(3).collect()
}
fn flatten(value: &Value, prefix: &str, depth: usize, out: &mut BTreeMap<String, String>) {
    if out.len() >= 400 {
        return;
    }
    match value {
        Value::Object(o) if depth < 5 => {
            for (k, v) in o {
                flatten(v, &if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, depth + 1, out);
            }
        }
        Value::Array(a) if depth < 5 => {
            for (i, v) in a.iter().take(3).enumerate() {
                flatten(v, &format!("{prefix}[{i}]"), depth + 1, out);
            }
        }
        Value::Null => {}
        other => {
            out.insert(prefix.to_owned(), match other {
                Value::String(s) => s.chars().take(80).collect(),
                v => v.to_string().chars().take(80).collect(),
            });
        }
    }
}
fn output_name(path: &str) -> String {
    let last = path.rsplit('.').next().unwrap_or(path);
    let clean: String = last.trim_end_matches(|c: char| c == ']' || c.is_ascii_digit() || c == '[').chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    if clean.is_empty() { "campo".into() } else { clean }
}
/// Local, explainable proposals. Nothing is applied: each suggestion says what was found,
/// where, and in how many of the examples.
pub(crate) fn suggest(texts: &[String]) -> Result<Value, String> {
    if texts.is_empty() || texts.len() > 200 || texts.iter().map(String::len).sum::<usize>() > 2 << 20 {
        return Err("Informe de 1 a 200 amostras (até 2 MiB no total).".into());
    }
    let total = texts.len();
    let mut suggestions = Vec::new();
    for r in RECOGNIZERS {
        let mut found = 0usize;
        let mut samples: BTreeSet<String> = BTreeSet::new();
        let mut positions: Vec<Value> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let mut first = true;
            for m in r.regex.find_iter(text).filter(|m| (r.validate)(m.as_str())).take(6) {
                if first {
                    found += 1;
                    first = false;
                }
                samples.insert(m.as_str().to_owned());
                if positions.len() < 600 {
                    positions.push(json!({"sample":i,"start":utf16(text, m.start()),"end":utf16(text, m.end())}));
                }
            }
        }
        if found > 0 {
            suggestions.push(json!({"id":r.id,"kind":"value","label":r.label,"why":r.why,"expression":r.expression,"field":r.field,"found":found,"total":total,"examples":examples(&samples),"positions":positions,
                "message": format!("Este trecho parece {}. Encontrado em {found} de {total} amostras.", r.label.to_lowercase())}));
        }
    }
    // key=value pairs: one proposal per key, plus a ready extraction.
    let mut keys: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for text in texts {
        let mut seen = BTreeSet::new();
        for c in KV.captures_iter(text) {
            let key = c[1].to_owned();
            let value = c[2].trim_matches('"').chars().take(60).collect::<String>();
            let entry = keys.entry(key.clone()).or_default();
            if seen.insert(key) {
                entry.0 += 1;
            }
            entry.1.insert(value);
        }
    }
    keys.retain(|_, (n, _)| *n * 4 >= total);
    let mut extractions = Vec::new();
    if !keys.is_empty() {
        for (key, (n, values)) in &keys {
            suggestions.push(json!({"id":format!("kv:{key}"),"kind":"key_value","label":format!("Valor de {key}="),"why":"Par chave=valor recorrente.","key":key,"field":output_name(key),"found":n,"total":total,"examples":examples(values),
                "message": format!("A chave {key}= aparece em {n} de {total} amostras.")}));
        }
        extractions.push(json!({"id":"key_value","label":"Pares chave=valor","explanation":"Lê as chaves reconhecidas respeitando aspas.","found":keys.values().map(|k| k.0).max().unwrap_or(0),"total":total,
            "extraction":{"kind":"key_value","pairDelimiter":"","separator":"=","quote":"\"","keys":keys.keys().map(|k| (output_name(k), k.clone())).collect::<BTreeMap<_,_>>(),"source":"message"}}));
    }
    // JSON and XML documents (possibly after a prefix).
    let (mut json_paths, mut json_docs): (BTreeMap<String, (usize, BTreeSet<String>)>, usize) = (BTreeMap::new(), 0);
    let (mut xml_paths, mut xml_docs): (BTreeMap<String, (usize, BTreeSet<String>)>, usize) = (BTreeMap::new(), 0);
    for text in texts {
        if let Some(start) = text.find(['{', '[']) {
            if let Some(Ok(value)) = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>().next() {
                if value.is_object() || value.is_array() {
                    json_docs += 1;
                    let mut flat = BTreeMap::new();
                    flatten(&value, "", 0, &mut flat);
                    for (path, sample) in flat {
                        let e = json_paths.entry(path).or_default();
                        e.0 += 1;
                        e.1.insert(sample);
                    }
                }
            }
        }
        if let Some(start) = text.find('<') {
            if let Ok(doc) = roxmltree::Document::parse(&text[start..]) {
                xml_docs += 1;
                let mut seen = BTreeSet::new();
                for node in doc.descendants().filter(|n| n.is_element()).take(300) {
                    let mut chain: Vec<String> = node.ancestors().filter(|n| n.is_element()).map(|n| match n.attribute("Name") {
                        Some(name) if n.tag_name().name() == "Data" => format!("{}[@Name='{name}']", n.tag_name().name()),
                        _ => n.tag_name().name().to_owned(),
                    }).collect();
                    chain.reverse();
                    let path = chain.join("/");
                    if let Some(t) = node.text().map(str::trim).filter(|t| !t.is_empty()) {
                        if seen.insert(path.clone()) {
                            let e = xml_paths.entry(path.clone()).or_default();
                            e.0 += 1;
                            e.1.insert(t.chars().take(80).collect());
                        }
                    }
                    for a in node.attributes().take(8) {
                        let p = format!("{path}@{}", a.name());
                        if seen.insert(p.clone()) {
                            let e = xml_paths.entry(p).or_default();
                            e.0 += 1;
                            e.1.insert(a.value().chars().take(80).collect());
                        }
                    }
                }
            }
        }
    }
    let structured = |kind: &'static str, label: &str, docs: usize, paths: &BTreeMap<String, (usize, BTreeSet<String>)>, suggestions: &mut Vec<Value>, extractions: &mut Vec<Value>| {
        if docs * 2 < total {
            return;
        }
        let top: Vec<(&String, &(usize, BTreeSet<String>))> = paths.iter().filter(|(_, (n, _))| *n * 2 >= docs).take(40).collect();
        for (path, (n, values)) in &top {
            suggestions.push(json!({"id":format!("{kind}:{path}"),"kind":kind,"label":format!("Propriedade {path}"),"why":"Propriedade presente na maioria dos documentos.","path":path,"field":output_name(path),"found":n,"total":total,"examples":examples(values),
                "message": format!("A propriedade {path} aparece em {n} de {docs} documentos {label}.")}));
        }
        extractions.push(json!({"id":kind,"label":format!("Documentos {label}"),"explanation":format!("Lê os documentos {label} com um analisador estruturado, sem expressões regulares."),"found":docs,"total":total,
            "extraction":{"kind":kind,"paths":top.iter().map(|(p, _)| (output_name(p), (*p).clone())).collect::<BTreeMap<_,_>>(),"source":"message"}}));
    };
    structured("json", "JSON", json_docs, &json_paths, &mut suggestions, &mut extractions);
    structured("xml", "XML", xml_docs, &xml_paths, &mut suggestions, &mut extractions);
    // Delimited columns: a delimiter that yields the same number of fields in most samples.
    for (delimiter, label) in [(",", "vírgula"), ("\t", "tabulação"), ("|", "barra vertical"), (";", "ponto e vírgula")] {
        let counts: Vec<usize> = texts.iter().map(|t| t.split(delimiter).count()).collect();
        let mut tally: BTreeMap<usize, usize> = BTreeMap::new();
        for c in &counts {
            *tally.entry(*c).or_default() += 1;
        }
        if let Some((&columns, &n)) = tally.iter().filter(|(c, _)| **c >= 3).max_by_key(|(_, n)| **n) {
            if n * 10 >= total * 8 && total >= 2 {
                extractions.push(json!({"id":format!("delimited:{delimiter}"),"label":format!("Colunas separadas por {label}"),"explanation":format!("{n} de {total} amostras têm {columns} colunas; aspas são respeitadas."),"found":n,"total":total,
                    "extraction":{"kind":"delimited","delimiter":delimiter,"quote":"\"","columns":(1..=columns).map(|i| format!("coluna_{i}")).collect::<Vec<_>>(),"source":"message"}}));
            }
        }
    }
    let mut kinds = Map::new();
    kinds.insert("total".into(), json!(total));
    Ok(json!({"scope":"sample","total":total,"suggestions":suggestions,"extractions":extractions,"summary":kinds}))
}

pub(crate) fn catalog() -> Value {
    json!({"patterns": crate::grok::LIBRARY, "engine": "pcre2", "compatibility": "grok-subset"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suggestions_explain_what_was_found_and_in_how_many_examples() {
        let texts: Vec<String> = (0..10).map(|i| format!("2026-10-23 17:0{i}:00 login user=ana{i} ip=10.0.0.{i} ok 0x2a")).chain(["linha sem nada".to_owned()]).collect();
        let r = suggest(&texts).unwrap();
        let s = r["suggestions"].as_array().unwrap();
        let ip = s.iter().find(|x| x["id"] == "ip").unwrap();
        assert_eq!(ip["found"], 10);
        assert_eq!(ip["total"], 11);
        assert!(ip["message"].as_str().unwrap().contains("10 de 11"));
        assert_eq!(ip["positions"][0]["start"], texts[0].find("10.0.0.0").unwrap());
        assert!(s.iter().any(|x| x["id"] == "iso8601"));
        assert!(s.iter().any(|x| x["id"] == "hex"));
        assert!(s.iter().any(|x| x["id"] == "kv:user"));
        let ex = r["extractions"].as_array().unwrap();
        assert!(ex.iter().any(|x| x["extraction"]["kind"] == "key_value"));
        // 999.1.1.1 is not an address.
        let none = suggest(&["x 999.1.1.1 y".to_owned()]).unwrap();
        assert!(none["suggestions"].as_array().unwrap().iter().all(|x| x["id"] != "ip"));
    }
    #[test]
    fn structured_data_gets_structured_proposals() {
        let json_lines: Vec<String> = (0..6).map(|i| format!("{{\"user\":{{\"name\":\"u{i}\"}},\"n\":{i}}}")).collect();
        let r = suggest(&json_lines).unwrap();
        let e = r["extractions"].as_array().unwrap().iter().find(|x| x["id"] == "json").unwrap();
        assert_eq!(e["extraction"]["paths"]["name"], "user.name");
        let csv: Vec<String> = (0..6).map(|i| format!("a{i},b,\"c,d\",{i}")).collect();
        assert!(!suggest(&csv).unwrap()["extractions"].as_array().unwrap().is_empty());
        let xml = vec!["<E><Data Name='Image'>cmd.exe</Data></E>".to_owned(); 4];
        let x = suggest(&xml).unwrap();
        assert!(x["suggestions"].as_array().unwrap().iter().any(|s| s["path"] == "E/Data[@Name='Image']"));
    }
    #[test]
    fn diff_separates_value_changes_from_notes_and_names_the_indexes_to_rebuild() {
        let mut a = enrichment_maps::built_in();
        a.schema = 2;
        let mut b = a.clone();
        b.title = "outro título".into();
        b.ui = Some(json!({"colors":{}}));
        b.rules[0].description = Some("nota".into());
        let indexed = vec!["norm.process.pid".to_owned(), "norm.host".to_owned()];
        let d = diff(Some(&serde_json::to_string(&a).unwrap()), &serde_json::to_string(&b).unwrap(), "norm", &indexed).unwrap();
        assert_eq!(d["recompute"], false);
        assert_eq!(d["metadataOnly"], true);
        assert!(d["indexesToRebuild"].as_array().unwrap().is_empty());
        b.rules[0].fields.get_mut("process.pid").unwrap().format = "text".into();
        b.rules[0].fields.remove("host");
        let d = diff(Some(&serde_json::to_string(&a).unwrap()), &serde_json::to_string(&b).unwrap(), "norm", &indexed).unwrap();
        assert_eq!(d["recompute"], true);
        assert_eq!(d["removed"], json!([]).as_array().map(|_| d["removed"].clone()).unwrap());
        assert!(d["changed"].as_array().unwrap().iter().any(|f| f == "process.pid"));
        let rebuild = d["indexesToRebuild"].as_array().unwrap();
        assert!(rebuild.iter().any(|f| f == "norm.process.pid"));
    }
    #[test]
    fn sampling_and_the_complete_scan_work_on_real_loaded_files() {
        struct Environment(Option<std::ffi::OsString>);
        impl Drop for Environment { fn drop(&mut self) { if let Some(v) = &self.0 { std::env::set_var("LOGINSIGHT_DATA_DIR", v); } else { std::env::remove_var("LOGINSIGHT_DATA_DIR"); } } }
        let dir = tempfile::tempdir().unwrap();
        let _environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path());
        crate::case_store::save(json!({"cases":[{"id":"lab-a"}],"active":null})).unwrap();
        let (a, b) = (dir.path().join("a.log"), dir.path().join("b.log"));
        let mut first = String::new();
        for i in 0..400 {
            first.push_str(&format!("falha 10.0.0.{} usuario=u{} acao=login duracao={}.5ms\n", i % 250, i % 7, i % 90));
        }
        first.push_str("linha atípica sem formato\n");
        let mut second = String::new();
        for i in 0..100 {
            second.push_str(&format!("ok 192.168.1.{} usuario=v{} acao=logout duracao={}ms\n", i % 250, i % 3, i));
        }
        std::fs::write(&a, first).unwrap();
        std::fs::write(&b, second).unwrap();
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        let identity = crate::analysis_context::snapshot("lab-a").unwrap().identity();
        let admitted = crate::analysis_runtime::capture(&state, Some(identity.clone()), None, crate::analysis_runtime::Mode::Publish).unwrap();
        admitted.prepare_visibility(None).unwrap();
        let paths = [a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
        crate::analysis_runtime::with(Some(admitted), || crate::load_files_impl(&state, &paths, "text", Some(false), None)).unwrap();
        let admitted = crate::analysis_runtime::capture(&state, Some(identity), None, crate::analysis_runtime::Mode::Dataset).unwrap();
        admitted.prepare_visibility(None).unwrap();
        crate::analysis_runtime::with(Some(admitted), || {
            let drawn = sample(&state, SampleRequest { sources: None, count: Some(30), field: None, seed: Some(7) }).unwrap();
            let again = sample(&state, SampleRequest { sources: None, count: Some(30), field: None, seed: Some(7) }).unwrap();
            assert_eq!(drawn["samples"], again["samples"], "the same seed repeats the sample");
            assert_eq!(drawn["population"], 501);
            let files: Vec<u64> = drawn["distribution"].as_array().unwrap().iter().map(|d| d["selected"].as_u64().unwrap()).collect();
            assert_eq!(files.len(), 2);
            assert!(files.iter().all(|n| *n > 0), "every file contributes: {files:?}");
            let texts: Vec<&str> = drawn["samples"].as_array().unwrap().iter().map(|s| s["text"].as_str().unwrap()).collect();
            assert!(texts.iter().any(|t| t.contains("atípica")), "the rare format is reserved a place");
            let map = crate::map_templates::all().into_iter().find(|t| t.id == "ip-usuario-acao-duracao").unwrap().map;
            let json = serde_json::to_string(&map).unwrap();
            let refs: Vec<SampleRef> = drawn["samples"].as_array().unwrap().iter().map(|s| SampleRef { id: s["id"].as_u64().unwrap() as usize, event_ref: s["eventRef"].as_str().unwrap().to_owned() }).collect();
            let quick = evaluate_samples(&state, &json, refs).unwrap();
            assert_eq!(quick["inspected"], 30);
            let done = scan(&state, &[], &json).unwrap();
            assert_eq!(done["inspected"], 501);
            assert_eq!(done["countsComplete"], true);
            // 400 lines fit the pattern; the "ok" lines have no "resultado" word list match for ms+ and the atypical one fails.
            let rule = &done["rules"][0];
            assert_eq!(rule["eligible"], 501);
            assert_eq!(rule["recognized"].as_u64().unwrap() + rule["notRecognized"].as_u64().unwrap(), 501);
            assert!(rule["notRecognized"].as_u64().unwrap() >= 1);
            assert_eq!(done["files"].as_array().unwrap().len(), 2);
            assert!(!done["failures"].as_array().unwrap().is_empty());
            assert!(done["failures"][0]["text"].is_string());
            // A source filter narrows the sample to one file.
            let only_b = sample(&state, SampleRequest { sources: Some(vec![paths[1].clone()]), count: Some(10), field: None, seed: None }).unwrap();
            assert!(only_b["samples"].as_array().unwrap().iter().all(|s| s["file"] == paths[1].as_str()));
        });
    }
    /// Reproducible scan benchmark: `LOGINSIGHT_BENCH_LINES=1000000 cargo test --release --lib
    /// scan_benchmark -- --ignored --nocapture`. Prints file size, records, build mode and rates.
    #[test]
    #[ignore]
    fn scan_benchmark() {
        struct Environment(Option<std::ffi::OsString>);
        impl Drop for Environment { fn drop(&mut self) { if let Some(v) = &self.0 { std::env::set_var("LOGINSIGHT_DATA_DIR", v); } else { std::env::remove_var("LOGINSIGHT_DATA_DIR"); } } }
        let lines: usize = std::env::var("LOGINSIGHT_BENCH_LINES").ok().and_then(|v| v.parse().ok()).unwrap_or(300_000);
        let dir = tempfile::tempdir().unwrap();
        let _environment = Environment(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", dir.path());
        crate::case_store::save(json!({"cases":[{"id":"bench"}],"active":null})).unwrap();
        let file = dir.path().join("bench.log");
        {
            use std::io::Write;
            let mut out = std::io::BufWriter::new(std::fs::File::create(&file).unwrap());
            for i in 0..lines {
                if i % 50 == 49 { writeln!(out, "linha fora do formato {i}").unwrap(); }
                else { writeln!(out, "falha 10.{}.{}.{} usuario=u{} acao=login duracao={}.5ms", i % 250, i % 200, i % 100, i % 997, i % 900).unwrap(); }
            }
        }
        let bytes = std::fs::metadata(&file).unwrap().len();
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        let identity = crate::analysis_context::snapshot("bench").unwrap().identity();
        let admitted = crate::analysis_runtime::capture(&state, Some(identity.clone()), None, crate::analysis_runtime::Mode::Publish).unwrap();
        admitted.prepare_visibility(None).unwrap();
        let paths = [file.to_string_lossy().into_owned()];
        let load = std::time::Instant::now();
        crate::analysis_runtime::with(Some(admitted), || crate::load_files_impl(&state, &paths, "text", Some(false), None)).unwrap();
        let load = load.elapsed();
        let admitted = crate::analysis_runtime::capture(&state, Some(identity), None, crate::analysis_runtime::Mode::Dataset).unwrap();
        admitted.prepare_visibility(None).unwrap();
        crate::analysis_runtime::with(Some(admitted), || {
            let map = serde_json::to_string(&crate::map_templates::all().into_iter().find(|t| t.id == "ip-usuario-acao-duracao").unwrap().map).unwrap();
            let started = std::time::Instant::now();
            let done = scan(&state, &[], &map).unwrap();
            let secs = started.elapsed().as_secs_f64();
            println!("BENCH build={} file={:.1} MiB records={} load={:.2}s scan={:.2}s rate={:.0} records/s {:.1} MiB/s recognized={} notRecognized={}",
                if cfg!(debug_assertions) { "debug" } else { "release" }, bytes as f64 / 1048576.0, done["inspected"], load.as_secs_f64(), secs,
                lines as f64 / secs, bytes as f64 / 1048576.0 / secs, done["rules"][0]["recognized"], done["rules"][0]["notRecognized"]);
            assert_eq!(done["inspected"].as_u64().unwrap(), lines as u64);
            // Baseline: the same pass with a definition that does nothing, i.e. the cost of reading records.
            let empty = serde_json::to_string(&enrichment_maps::Definition { id: "vazio".into(), version: "1".into(), title: "Vazio".into(), rules: vec![], schema: 2, description: None, enabled: true, grok_patterns: Default::default(), ui: None }).unwrap();
            let started = std::time::Instant::now();
            scan(&state, &[], &empty).unwrap();
            let base = started.elapsed().as_secs_f64();
            println!("BENCH baseline (reading records only) = {base:.2}s, {:.0} records/s; extraction share = {:.0}%", lines as f64 / base, 100.0 * (secs - base).max(0.0) / secs);
        });
    }
    #[test]
    fn pasted_lines_are_evaluated_with_real_ranges_and_per_output_states() {
        let map = crate::map_templates::all().into_iter().find(|t| t.id == "ip-usuario-acao-duracao").unwrap().map;
        let lines = vec!["falha 10.0.0.5 usuario=Ana acao=login duracao=12.5ms".to_owned(), "nada a ver".to_owned()];
        let r = evaluate_texts(&serde_json::to_string(&map).unwrap(), &lines).unwrap();
        assert_eq!(r["scope"], "text");
        assert_eq!(r["results"][0]["fields"]["usuario"], "ana");
        let extracted = r["results"][0]["trace"]["rules"][0]["extracted"].as_array().unwrap();
        let ip = extracted.iter().find(|e| e[0] == "ip_origem").unwrap();
        assert_eq!(ip[2], json!([6, 14]));
        assert_eq!(r["results"][1]["trace"]["rules"][0]["extraction"], "not_recognized");
        assert_eq!(r["rules"][0], json!({"rule":"acesso","eligible":2,"recognized":1,"notRecognized":1,"errors":0}));
        assert!(evaluate_texts("{}", &lines).is_err());
        assert!(evaluate_texts(&serde_json::to_string(&map).unwrap(), &[]).is_err());
    }
    #[test]
    fn shapes_collapse_digits_letters_and_spaces() {
        assert_eq!(shape("2026-10-23 abc  def 12"), shape("1999-01-01 zz y 7"));
        assert_ne!(shape("a=1"), shape("a:1"));
    }
    #[test]
    fn utf16_positions_count_emoji_as_two_units() {
        assert_eq!(utf16("😀 ip", 5), 3);
    }
}
