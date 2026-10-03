//! Case records held by the backend per content version. The interface sends
//! them once (`case_sync`) and then refers to them by key, instead of
//! serializing every record on each query.
use crate::model::Event;
use parking_lot::Mutex;
use std::sync::Arc;

pub const MISS: &str = "CASE_CACHE_MISS";

struct CachedCase {
    key: String,
    events: Arc<Vec<Event>>,
    engine: Option<Arc<crate::big_data::BigDataIndex>>,
    content_key: String,
    bytes: usize,
}

static CACHE: Mutex<Vec<CachedCase>> = Mutex::new(Vec::new());
const CACHE_BYTES: usize = 256 * 1024 * 1024;

pub(crate) struct ResourceMetrics {
    pub versions: usize,
    pub event_count: u64,
    pub event_estimated_bytes: u64,
    pub overhead_bytes: u64,
    pub engines: Vec<Arc<crate::big_data::BigDataIndex>>,
}

/// Reads insertion-time estimates, never walks the preserved event corpus.
pub(crate) fn resource_metrics() -> Option<ResourceMetrics> {
    let cache = CACHE.try_lock()?;
    let mut seen = std::collections::HashSet::new();
    let mut metrics = ResourceMetrics {
        versions: cache.len(), event_count: 0, event_estimated_bytes: 0,
        overhead_bytes: (cache.capacity() as u64).saturating_mul(std::mem::size_of::<CachedCase>() as u64),
        engines: Vec::new(),
    };
    for entry in cache.iter() {
        metrics.overhead_bytes = metrics.overhead_bytes.saturating_add(entry.key.capacity() as u64)
            .saturating_add(entry.content_key.capacity() as u64);
        if seen.insert(Arc::as_ptr(&entry.events) as usize) {
            metrics.event_count = metrics.event_count.saturating_add(entry.events.len() as u64);
            let spare = entry.events.capacity().saturating_sub(entry.events.len()) as u64;
            metrics.event_estimated_bytes = metrics.event_estimated_bytes.saturating_add(entry.bytes as u64)
                .saturating_add(spare.saturating_mul(std::mem::size_of::<Event>() as u64));
        }
        if let Some(engine) = &entry.engine { metrics.engines.push(engine.clone()); }
    }
    Some(metrics)
}

fn value_bytes(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(s) => s.len(),
        serde_json::Value::Array(items) => items.iter().map(value_bytes).sum::<usize>() + items.len() * std::mem::size_of::<serde_json::Value>(),
        serde_json::Value::Object(map) => map.iter().map(|(k,v)| k.len() + value_bytes(v) + 64).sum(),
        _ => 16,
    }
}

fn metadata(events: &[Event]) -> (String, usize) {
    let bytes = events.iter().map(|e| std::mem::size_of::<Event>() +
        e.event_ref.len() + e.parse_status.len() + e.source.len() + e.level.len() +
        e.code.len() + e.name.len() + e.description.len() + e.message.len() + e.raw.len() +
        e.fields.iter().map(|(k,v)| k.len() + value_bytes(v) + 64).sum::<usize>()).sum();
    (crate::big_data::fingerprint_events(events), bytes)
}

fn insert(cache: &mut Vec<CachedCase>, entry: CachedCase) {
    cache.retain(|old| old.key != entry.key);
    // Keep the active version even when it alone exceeds the cache budget.
    // Eviction affects cache reuse, never truncates evidence or rows.
    while !cache.is_empty() && (cache.len() >= 3 ||
        cache.iter().map(|old| old.bytes).sum::<usize>().saturating_add(entry.bytes) > CACHE_BYTES) {
        cache.remove(0);
    }
    cache.push(entry);
}

#[cfg(test)]
pub fn store(key: String, events: Vec<Event>) {
    let (content_key, bytes) = metadata(&events);
    let mut cache = CACHE.lock();
    insert(&mut cache, CachedCase { key, events: Arc::new(events), engine: None, content_key, bytes });
}

/// Explicit records win; a key must have been synchronized before.
pub fn resolve(events: Option<Vec<Event>>, key: Option<String>) -> Result<Option<Arc<Vec<Event>>>, String> {
    if let Some(events) = events {
        return Ok(Some(Arc::new(events)));
    }
    let Some(key) = key else { return Ok(None) };
    CACHE
        .lock()
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| Some(entry.events.clone()))
        .ok_or_else(|| MISS.to_string())
}

/// Match only the exact immutable version, never a content-count approximation.
pub fn engine_for(events: &[Event]) -> Option<Arc<crate::big_data::BigDataIndex>> {
    CACHE.lock().iter().find(|entry| {
        entry.events.len() == events.len() && entry.events.as_ptr() == events.as_ptr()
    }).and_then(|entry| entry.engine.clone())
}

pub fn content_key_for(events: &[Event]) -> Option<String> {
    CACHE.lock().iter().find(|entry| entry.events.len() == events.len() && entry.events.as_ptr() == events.as_ptr())
        .map(|entry| entry.content_key.clone())
}

#[tauri::command]
pub async fn case_sync(key: String, events: Vec<Event>, big_data: Option<bool>, app: tauri::AppHandle) -> Result<crate::big_data_commands::ModeStatus, String> {
    if key.is_empty() || key.len() > 512 {
        return Err("Chave de caso inválida.".into());
    }
    crate::offload(move || {
        let engine = if big_data == Some(true) && !events.is_empty() {
            Some(Arc::new(crate::big_data::BigDataIndex::open_or_build_events(
                &events, &crate::config_dir().join("big-data-v1"),
                Some(&|completed, total| crate::emit_progress(Some(&app), "bigdata", "Indexando eventos do Caso", completed, total, "eventos", true)),
            )?))
        } else { None };
        crate::operations::check()?;
        let info = engine.as_ref().map(|engine| engine.info());
        let status = crate::big_data_commands::ModeStatus {
            enabled: big_data == Some(true), ready: engine.is_some(), event_count: events.len(),
            index_bytes: info.as_ref().map_or(0, |info| info.index_bytes),
            reused: info.as_ref().is_some_and(|info| info.reused),
            build_ms: info.as_ref().map_or(0, |info| info.build_ms), engine: "tantivy",
        };
        let (content_key, bytes) = metadata(&events);
        let mut cache = CACHE.lock();
        crate::operations::check()?;
        crate::operations::commit();
        insert(&mut cache, CachedCase { key, events: Arc::new(events), engine, content_key, bytes });
        Ok(status)
    }).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_resolve_until_evicted() {
        store("a".into(), vec![Event::empty()]);
        assert_eq!(resolve(None, Some("a".into())).unwrap().unwrap().len(), 1);
        assert!(resolve(None, None).unwrap().is_none());
        for k in ["b", "c", "d"] {
            store(k.into(), vec![]);
        }
        assert_eq!(resolve(None, Some("a".into())).unwrap_err(), MISS);
        assert_eq!(resolve(Some(vec![]), Some("zzz".into())).unwrap().unwrap().len(), 0);
    }

    #[test]
    fn shared_versions_retain_identity_and_use_case_index_without_copying() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Event::empty(); a.id = 42; a.event_ref = "original:42".into(); a.message = "rare preserved phrase".into();
        let mut b = Event::empty(); b.id = 900; b.event_ref = "original:900".into(); b.message = "usual record".into();
        let events = vec![a,b];
        let engine = Arc::new(crate::big_data::BigDataIndex::open_or_build_events(&events, dir.path(), None).unwrap());
        let (content_key, bytes) = metadata(&events);
        insert(&mut CACHE.lock(), CachedCase { key:"indexed-test".into(), events:Arc::new(events), engine:Some(engine), content_key, bytes });
        let first = resolve(None, Some("indexed-test".into())).unwrap().unwrap();
        let second = resolve(None, Some("indexed-test".into())).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(engine_for(&first).is_some());
        assert_eq!(content_key_for(&first), content_key_for(&second));
        let filter = crate::query::Filter { column:"message".into(), op:"contains".into(), value:"rare preserved".into(), value2:None };
        let result = crate::query::query(&first, &[filter], "id", "asc", 0, 100);
        assert_eq!(result.total,1); assert_eq!(result.rows[0].id,42); assert_eq!(result.rows[0].event_ref,"original:42");
        CACHE.lock().retain(|entry| entry.key != "indexed-test");
    }
}
