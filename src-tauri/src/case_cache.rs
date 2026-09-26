//! Case records held by the backend per content version. The interface sends
//! them once (`case_sync`) and then refers to them by key, instead of
//! serializing every record on each query.
use crate::model::Event;
use parking_lot::Mutex;
use std::sync::Arc;

pub const MISS: &str = "CASE_CACHE_MISS";

static CACHE: Mutex<Vec<(String, Arc<Vec<Event>>)>> = Mutex::new(Vec::new());

pub fn store(key: String, events: Vec<Event>) {
    let mut cache = CACHE.lock();
    cache.retain(|(k, _)| *k != key);
    if cache.len() >= 3 {
        cache.remove(0);
    }
    cache.push((key, Arc::new(events)));
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
        .find(|(k, _)| *k == key)
        .map(|(_, events)| Some(events.clone()))
        .ok_or_else(|| MISS.to_string())
}

/// Owned records for commands whose implementation consumes them.
pub fn take(events: Option<Vec<Event>>, key: Option<String>) -> Result<Option<Vec<Event>>, String> {
    Ok(match resolve(events, key)? {
        Some(shared) => Some(Arc::try_unwrap(shared).unwrap_or_else(|arc| (*arc).clone())),
        None => None,
    })
}

#[tauri::command]
pub async fn case_sync(key: String, events: Vec<Event>) -> Result<(), String> {
    if key.is_empty() || key.len() > 512 {
        return Err("Chave de caso inválida.".into());
    }
    store(key, events);
    Ok(())
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
}
