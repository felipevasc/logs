//! Case records held by the backend per content version. The interface sends
//! them once (`case_sync`) and then refers to them by key, instead of
//! serializing every record on each query.
use crate::model::Event;
use parking_lot::Mutex;
use std::sync::Arc;

pub const MISS: &str = "CASE_CACHE_MISS";

type Owner = Option<(String, String)>;
fn owner(identity: Option<&crate::analysis_context::Identity>) -> Owner {
    identity.map(|identity| (identity.case_id.clone(), identity.analysis_id.clone()))
}

static CACHE: Mutex<Vec<(Owner, String, Arc<Vec<Event>>)>> = Mutex::new(Vec::new());

pub fn store(key: String, events: Vec<Event>) {
    store_for(key, events, None);
}

fn store_for(
    key: String,
    events: Vec<Event>,
    identity: Option<&crate::analysis_context::Identity>,
) {
    let owner = owner(identity);
    let mut cache = CACHE.lock();
    cache.retain(|(o, k, _)| *o != owner || *k != key);
    if cache.len() >= 3 {
        cache.remove(0);
    }
    cache.push((owner, key, Arc::new(events)));
}

/// Explicit records win; a key must have been synchronized before.
pub fn resolve(
    events: Option<Vec<Event>>,
    key: Option<String>,
) -> Result<Option<Arc<Vec<Event>>>, String> {
    resolve_for(
        events,
        key,
        crate::analysis_runtime::current()
            .as_ref()
            .and_then(|admitted| admitted.identity.as_ref()),
    )
}

fn resolve_for(
    events: Option<Vec<Event>>,
    key: Option<String>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<Option<Arc<Vec<Event>>>, String> {
    if let Some(events) = events {
        return Ok(Some(Arc::new(events)));
    }
    let Some(key) = key else { return Ok(None) };
    let owner = owner(identity);
    CACHE
        .lock()
        .iter()
        .find(|(o, k, _)| *o == owner && *k == key)
        .map(|(_, _, events)| Some(events.clone()))
        .ok_or_else(|| MISS.to_string())
}

/// Owned records for commands whose implementation consumes them.
pub fn take(events: Option<Vec<Event>>, key: Option<String>) -> Result<Option<Vec<Event>>, String> {
    take_for(
        events,
        key,
        crate::analysis_runtime::current()
            .as_ref()
            .and_then(|admitted| admitted.identity.as_ref()),
    )
}

pub(crate) fn take_for(
    events: Option<Vec<Event>>,
    key: Option<String>,
    identity: Option<&crate::analysis_context::Identity>,
) -> Result<Option<Vec<Event>>, String> {
    Ok(match resolve_for(events, key, identity)? {
        Some(shared) => Some(Arc::try_unwrap(shared).unwrap_or_else(|arc| (*arc).clone())),
        None => None,
    })
}

#[tauri::command]
pub async fn case_sync(
    key: String,
    events: Vec<Event>,
    analysis_context: Option<crate::analysis_context::Identity>,
) -> Result<(), String> {
    if key.is_empty() || key.len() > 512 {
        return Err("Chave de caso inválida.".into());
    }
    if let Some(identity) = &analysis_context {
        crate::analysis_runtime::validate_identity(identity)?;
    }
    store_for(key, events, analysis_context.as_ref());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_keys_never_cross_case_or_recreated_analysis_owners() {
        let identity = |case_id: &str, analysis_id: &str| crate::analysis_context::Identity {
            case_id: case_id.into(),
            analysis_id: analysis_id.into(),
            config_revision: 0,
            visibility_revision: 0,
        };
        let a = identity("a", "analysis-a");
        let b = identity("b", "analysis-b");
        let recreated = identity("a", "replacement-a");
        let key = format!("shared-key-{}", uuid::Uuid::new_v4());
        store_for(key.clone(), vec![Event::empty()], Some(&a));
        assert!(resolve_for(None, Some(key.clone()), Some(&b)).is_err());
        assert!(resolve_for(None, Some(key.clone()), Some(&recreated)).is_err());
        assert!(resolve_for(None, Some(key.clone()), None).is_err());
        let mut revised = a.clone();
        revised.config_revision = 10;
        assert_eq!(
            resolve_for(None, Some(key.clone()), Some(&revised))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        store_for(key.clone(), vec![], Some(&b));
        assert_eq!(
            resolve_for(None, Some(key.clone()), Some(&a))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        assert!(resolve_for(None, Some(key), Some(&b))
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn keys_resolve_until_evicted() {
        store("a".into(), vec![Event::empty()]);
        assert_eq!(resolve(None, Some("a".into())).unwrap().unwrap().len(), 1);
        assert!(resolve(None, None).unwrap().is_none());
        for k in ["b", "c", "d"] {
            store(k.into(), vec![]);
        }
        assert_eq!(resolve(None, Some("a".into())).unwrap_err(), MISS);
        assert_eq!(
            resolve(Some(vec![]), Some("zzz".into()))
                .unwrap()
                .unwrap()
                .len(),
            0
        );
    }
}
