//! Views that pivot on entities: timeline lanes, entity summaries,
//! indicator sightings and source integrity hashes.
use crate::{
    model::Event,
    query::{self, Filter},
    querylang::{self, Ctx},
    workspace, AppState, SourceData,
};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use tauri::{AppHandle, Manager};

// ------------------------------------------------------------------ lanes

#[derive(Serialize)]
pub struct Lane {
    pub value: String,
    pub total: usize,
    pub errors: usize,
    pub counts: Vec<u32>,
    pub error_counts: Vec<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lanes {
    pub column: String,
    pub start: i64,
    pub end: i64,
    pub bucket_ms: i64,
    pub buckets: usize,
    pub lanes: Vec<Lane>,
    pub others: Option<Lane>,
    pub missing: usize,
    pub distinct_limited: bool,
}

const LANE_KEYS: usize = 20_000;

/// Bucket width and count with the same rounding as `timeline_range`.
pub fn layout(start: i64, end: i64, requested: usize) -> (i64, usize) {
    let requested = requested.clamp(1, 240);
    let span = end.saturating_sub(start).saturating_add(1);
    let width = (span / requested as i64).saturating_add(i64::from(span % requested as i64 != 0)).max(1);
    (width, (((span - 1) / width + 1) as usize).min(requested))
}

#[derive(Default)]
struct LaneAcc {
    lanes: HashMap<Box<str>, (usize, usize, Vec<u32>, Vec<u32>)>,
    others: (usize, usize, Vec<u32>, Vec<u32>),
    missing: usize,
    limited: bool,
}

pub fn lanes_impl(
    state: &AppState,
    filters: Vec<Filter>,
    start: i64,
    end: i64,
    bucket_count: usize,
    column: String,
    limit: usize,
    case_events: Option<&[Event]>,
) -> Result<Lanes, String> {
    workspace::validate(&filters)?;
    if start > end {
        return Err("O início deve ser anterior ao fim.".into());
    }
    if column.trim().is_empty() {
        return Err("Escolha um campo para separar a linha do tempo.".into());
    }
    let (width, buckets) = layout(start, end, bucket_count);
    let mut scoped = vec![Filter { column: "timestamp".into(), op: "between".into(), value: start.to_string(), value2: Some(end.to_string()) }];
    scoped.extend(filters);
    let field = querylang::field_ref(&column);
    let empty = || (0usize, 0usize, vec![0u32; buckets], vec![0u32; buckets]);
    let step = |acc: &mut LaneAcc, ev: &Event| {
        let Some(t) = ev.timestamp else { return };
        if t < start || t > end {
            return;
        }
        let index = (((t - start) / width) as usize).min(buckets - 1);
        let error = matches!(ev.level.as_str(), "Erro" | "Crítico");
        let ctx = Ctx::new(ev);
        let Some(value) = ctx.get(&field).filter(|v| !v.trim().is_empty()) else {
            acc.missing += 1;
            return;
        };
        let value = value.trim();
        let slot = if let Some(slot) = acc.lanes.get_mut(value) {
            slot
        } else if acc.lanes.len() < LANE_KEYS {
            acc.lanes.entry(value.chars().take(300).collect::<String>().into_boxed_str()).or_insert_with(empty)
        } else {
            acc.limited = true;
            if acc.others.2.is_empty() {
                acc.others = empty();
            }
            &mut acc.others
        };
        slot.0 += 1;
        slot.2[index] += 1;
        if error {
            slot.1 += 1;
            slot.3[index] += 1;
        }
    };
    let merge = |mut a: LaneAcc, b: LaneAcc| {
        for (key, (total, errors, counts, error_counts)) in b.lanes {
            let entry = a.lanes.entry(key).or_insert_with(empty);
            entry.0 += total;
            entry.1 += errors;
            for (x, y) in entry.2.iter_mut().zip(counts) {
                *x += y;
            }
            for (x, y) in entry.3.iter_mut().zip(error_counts) {
                *x += y;
            }
        }
        if !b.others.2.is_empty() {
            if a.others.2.is_empty() {
                a.others = empty();
            }
            a.others.0 += b.others.0;
            a.others.1 += b.others.1;
            for (x, y) in a.others.2.iter_mut().zip(b.others.2) {
                *x += y;
            }
            for (x, y) in a.others.3.iter_mut().zip(b.others.3) {
                *x += y;
            }
        }
        a.missing += b.missing;
        a.limited |= b.limited;
        a
    };
    let acc = match case_events {
        Some(events) => {
            let prepared = query::prepare(&scoped);
            let mut acc = LaneAcc::default();
            for ev in events.iter().filter(|e| prepared.iter().all(|f| query::matches(e, f))) {
                step(&mut acc, ev);
            }
            acc
        }
        None => workspace::with_selection(state, &scoped, |selection| selection.par_fold(LaneAcc::default, step, merge)),
    };
    crate::operations::check()?;
    let mut all: Vec<(Box<str>, (usize, usize, Vec<u32>, Vec<u32>))> = acc.lanes.into_iter().collect();
    all.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
    let limit = limit.clamp(1, 24);
    let mut others = if acc.others.2.is_empty() { None } else { Some(acc.others) };
    let rest: Vec<_> = all.split_off(all.len().min(limit));
    for (_, (total, errors, counts, error_counts)) in rest {
        let o = others.get_or_insert_with(empty);
        o.0 += total;
        o.1 += errors;
        for (x, y) in o.2.iter_mut().zip(counts) {
            *x += y;
        }
        for (x, y) in o.3.iter_mut().zip(error_counts) {
            *x += y;
        }
    }
    Ok(Lanes {
        column,
        start,
        end,
        bucket_ms: width,
        buckets,
        lanes: all
            .into_iter()
            .map(|(value, (total, errors, counts, error_counts))| Lane { value: value.into(), total, errors, counts, error_counts })
            .collect(),
        others: others.map(|(total, errors, counts, error_counts)| Lane { value: String::new(), total, errors, counts, error_counts }),
        missing: acc.missing,
        distinct_limited: acc.limited,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn timeline_lanes(
    filters: Vec<Filter>,
    start: i64,
    end: i64,
    bucket_count: usize,
    column: String,
    limit: Option<usize>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    app: AppHandle,
) -> Result<Lanes, String> {
    let case_events = crate::case_cache::resolve(case_events, case_key)?;
    crate::offload(move || {
        lanes_impl(
            app.state::<AppState>().inner(),
            filters,
            start,
            end,
            bucket_count,
            column,
            limit.unwrap_or(8),
            case_events.as_deref().map(|v| v.as_slice()),
        )
    })
    .await?
}

// ------------------------------------------------------------------ entities

#[derive(Serialize, Clone)]
pub struct EntityCount {
    pub value: String,
    pub count: usize,
    pub failures: usize,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct EntityGroup {
    pub column: String,
    pub label: String,
    pub distinct: usize,
    pub distinct_limited: bool,
    pub values: Vec<EntityCount>,
}

const SUMMARY_ROLES: &[&str] = &["@user", "@src_ip", "@dst_ip", "@host", "@process", "@domain", "@hash", "@url", "@file", "@cmdline"];

pub fn entity_summary_impl(
    state: &AppState,
    filters: Vec<Filter>,
    limit: usize,
    case_events: Option<&[Event]>,
) -> Result<Vec<EntityGroup>, String> {
    workspace::validate(&filters)?;
    type Acc = (Vec<HashMap<Box<str>, EntityCount>>, Vec<bool>);
    let init = || -> Acc { (vec![HashMap::new(); SUMMARY_ROLES.len()], vec![false; SUMMARY_ROLES.len()]) };
    let roles: Vec<crate::entities::Role> = SUMMARY_ROLES.iter().filter_map(|c| crate::entities::role_of_column(c)).collect();
    let step = |acc: &mut Acc, ev: &Event| {
        let ctx = Ctx::new(ev);
        let failure = ctx.role(crate::entities::Role::Outcome) == Some("failure");
        for (slot, role) in roles.iter().enumerate() {
            let Some(value) = ctx.role(*role) else { continue };
            let map = &mut acc.0[slot];
            if !map.contains_key(value) && map.len() >= 50_000 {
                acc.1[slot] = true;
                continue;
            }
            let entry = map.entry(value.chars().take(1000).collect::<String>().into_boxed_str()).or_insert_with(|| EntityCount {
                value: value.chars().take(1000).collect(),
                count: 0,
                failures: 0,
                first: None,
                last: None,
                scope: None,
            });
            entry.count += 1;
            entry.failures += usize::from(failure);
            if let Some(t) = ev.timestamp {
                entry.first = Some(entry.first.map_or(t, |f| f.min(t)));
                entry.last = Some(entry.last.map_or(t, |l| l.max(t)));
            }
        }
    };
    let merge = |mut a: Acc, b: Acc| {
        for (slot, map) in b.0.into_iter().enumerate() {
            for (key, value) in map {
                match a.0[slot].get_mut(&key) {
                    Some(entry) => {
                        entry.count += value.count;
                        entry.failures += value.failures;
                        entry.first = match (entry.first, value.first) { (Some(x), Some(y)) => Some(x.min(y)), (x, y) => x.or(y) };
                        entry.last = match (entry.last, value.last) { (Some(x), Some(y)) => Some(x.max(y)), (x, y) => x.or(y) };
                    }
                    None => {
                        a.0[slot].insert(key, value);
                    }
                }
            }
            a.1[slot] |= b.1[slot];
        }
        a
    };
    let acc = match case_events {
        Some(events) => {
            let prepared = query::prepare(&filters);
            let mut acc = init();
            for ev in events.iter().filter(|e| prepared.iter().all(|f| query::matches(e, f))) {
                step(&mut acc, ev);
            }
            acc
        }
        None => workspace::with_selection(state, &filters, |selection| selection.par_fold(init, step, merge)),
    };
    crate::operations::check()?;
    let limit = limit.clamp(1, 500);
    Ok(SUMMARY_ROLES
        .iter()
        .enumerate()
        .filter_map(|(slot, column)| {
            let map = &acc.0[slot];
            if map.is_empty() {
                return None;
            }
            let mut values: Vec<EntityCount> = map.values().cloned().collect();
            values.sort_by(|a, b| b.count.cmp(&a.count).then(a.value.cmp(&b.value)));
            values.truncate(limit);
            for v in &mut values {
                if matches!(*column, "@src_ip" | "@dst_ip") {
                    v.scope = crate::entities::parse_ip(&v.value).map(|ip| crate::entities::ip_scope(ip).to_string());
                }
            }
            let role = crate::entities::role_of_column(column)?;
            Some(EntityGroup {
                column: column.to_string(),
                label: crate::entities::info(role).label.into(),
                distinct: map.len(),
                distinct_limited: acc.1[slot],
                values,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn entity_summary(
    filters: Vec<Filter>,
    limit: Option<usize>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    app: AppHandle,
) -> Result<Vec<EntityGroup>, String> {
    let case_events = crate::case_cache::resolve(case_events, case_key)?;
    crate::offload(move || {
        entity_summary_impl(app.state::<AppState>().inner(), filters, limit.unwrap_or(50), case_events.as_deref().map(|v| v.as_slice()))
    })
    .await?
}

// ------------------------------------------------------------------ indicators

#[derive(Serialize, Clone, Default)]
pub struct Sighting {
    pub value: String,
    pub count: usize,
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub event_ids: Vec<usize>,
    pub sources: Vec<String>,
}

/// Where each indicator appears in the loaded sources (case-insensitive
/// substring of the record), in one pass with a multi-pattern automaton.
pub fn sightings_impl(state: &AppState, values: Vec<String>, filters: Vec<Filter>) -> Result<Vec<Sighting>, String> {
    workspace::validate(&filters)?;
    let mut values: Vec<String> = values.into_iter().map(|v| v.trim().to_string()).filter(|v| v.len() >= 2).collect();
    values.sort();
    values.dedup();
    if values.len() > 5000 {
        return Err("Informe até 5.000 indicadores por vez.".into());
    }
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let automaton = aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(true)
        .build(&values)
        .map_err(|e| e.to_string())?;
    type Acc = Vec<Sighting>;
    let init = || -> Acc { values.iter().map(|v| Sighting { value: v.clone(), ..Default::default() }).collect() };
    let step = |acc: &mut Acc, ev: &Event| {
        let mut seen = std::collections::HashSet::new();
        for text in [ev.raw.as_str(), ev.message.as_str()] {
            for found in automaton.find_overlapping_iter(text) {
                seen.insert(found.pattern().as_usize());
            }
        }
        for index in seen {
            let s = &mut acc[index];
            s.count += 1;
            if let Some(t) = ev.timestamp {
                s.first = Some(s.first.map_or(t, |f| f.min(t)));
                s.last = Some(s.last.map_or(t, |l| l.max(t)));
            }
            if s.event_ids.len() < 5 {
                s.event_ids.push(ev.id);
            }
            if let Some(file) = ev.fields.get("arquivo").and_then(|v| v.as_str()) {
                if s.sources.len() < 8 && !s.sources.iter().any(|x| x == file) {
                    s.sources.push(file.to_string());
                }
            }
        }
    };
    let merge = |mut a: Acc, b: Acc| {
        for (x, y) in a.iter_mut().zip(b) {
            x.count += y.count;
            x.first = match (x.first, y.first) { (Some(p), Some(q)) => Some(p.min(q)), (p, q) => p.or(q) };
            x.last = match (x.last, y.last) { (Some(p), Some(q)) => Some(p.max(q)), (p, q) => p.or(q) };
            for id in y.event_ids {
                if x.event_ids.len() < 5 {
                    x.event_ids.push(id);
                }
            }
            for source in y.sources {
                if x.sources.len() < 8 && !x.sources.contains(&source) {
                    x.sources.push(source);
                }
            }
        }
        a
    };
    let acc = workspace::with_selection(state, &filters, |selection| selection.par_fold(init, step, merge));
    crate::operations::check()?;
    Ok(acc)
}

#[tauri::command]
pub async fn ioc_sightings(values: Vec<String>, filters: Vec<Filter>, app: AppHandle) -> Result<Vec<Sighting>, String> {
    crate::offload(move || sightings_impl(app.state::<AppState>().inner(), values, filters)).await?
}

// ------------------------------------------------------------------ custody

#[derive(Serialize, Clone)]
pub struct SourceHash {
    pub id: String,
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    /// "original" when the hashed file is the one the user opened;
    /// "extraído" for package members and decompressed files.
    pub origin: String,
}

static HASHES: Mutex<Vec<(String, SourceHash)>> = Mutex::new(Vec::new());

fn sha256_file(path: &std::path::Path) -> Result<(String, u64), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buffer[..n]);
        if total % (256 << 20) < (1 << 20) {
            crate::operations::check()?;
        }
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

pub fn hashes_impl(state: &AppState) -> Result<Vec<SourceHash>, String> {
    let parts: Vec<(String, String, String)> = match &*state.source.read() {
        SourceData::Indexed(idx) => idx.parts.iter().map(|p| (p.identity.clone(), p.path.clone(), p.file_name.clone())).collect(),
        _ => Vec::new(),
    };
    // A package member is hashed as extracted, and the package itself as the
    // user opened it. Live channels and other non-file sources have no hash.
    let mut jobs: Vec<(String, String, String, std::path::PathBuf, &'static str)> = Vec::new();
    for (id, path, name) in parts {
        match workspace::resolve_member(&path)? {
            Some(member) => {
                let container = path.split("!/").next().unwrap_or(&path).to_string();
                if !jobs.iter().any(|job| job.1 == container) {
                    let file = std::path::Path::new(&container);
                    let stamp = std::fs::metadata(file).ok().map(|m| (m.len(), m.modified().ok()));
                    let label = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| container.clone());
                    jobs.push((format!("package:{container}:{stamp:?}"), container.clone(), label, file.to_path_buf(), "original"));
                }
                jobs.push((id, path, name, member, "extraído"));
            }
            None if std::path::Path::new(&path).is_file() => {
                let physical = std::path::PathBuf::from(&path);
                jobs.push((id, path, name, physical, "original"));
            }
            None => {}
        }
    }
    use rayon::prelude::*;
    let generation = crate::operations::current_generation();
    let results: Vec<Result<SourceHash, String>> = jobs
        .into_par_iter()
        .map(|(id, path, name, physical, origin)| {
            if let Some((_, hit)) = HASHES.lock().iter().find(|(k, _)| *k == id) {
                return Ok(hit.clone());
            }
            if crate::operations::cancelled_for(generation) {
                return Err("Operação cancelada.".into());
            }
            let (sha256, bytes) = sha256_file(&physical)?;
            let hash = SourceHash { id: id.clone(), path, name, bytes, sha256, origin: origin.into() };
            let mut cache = HASHES.lock();
            if cache.len() > 512 {
                cache.remove(0);
            }
            cache.push((id, hash.clone()));
            Ok(hash)
        })
        .collect();
    results.into_iter().collect()
}

#[tauri::command]
pub async fn source_hashes(app: AppHandle) -> Result<Vec<SourceHash>, String> {
    crate::offload(move || hashes_impl(app.state::<AppState>().inner())).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_layout_matches_timeline_rounding() {
        assert_eq!(layout(0, 999, 10), (100, 10));
        assert_eq!(layout(0, 1000, 10), (101, 10));
        assert_eq!(layout(5, 5, 120), (1, 1));
    }
}
