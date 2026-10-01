//! Case records held by the backend per content version. The interface sends
//! them once (`case_sync`) and then refers to them by key, instead of
//! serializing every record on each query.
use crate::model::Event;
use parking_lot::Mutex;
use std::sync::Arc;

pub const MISS: &str = "CASE_CACHE_MISS";
pub(crate) const CHANGED: &str = "CASE_CACHE_CHANGED: As evidências do Caso mudaram; recarregue a consulta.";

type Owner = Option<(String, String)>;
fn owner(identity: Option<&crate::analysis_context::Identity>) -> Owner {
    identity.map(|identity| (identity.case_id.clone(), identity.analysis_id.clone()))
}

struct Entry {
    owner: Owner,
    key: String,
    publication: String,
    events: Arc<Vec<Event>>,
}
static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

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
    cache.retain(|entry| entry.owner != owner || entry.key != key);
    if cache.len() >= 3 {
        cache.remove(0);
    }
    cache.push(Entry { owner, key, publication: uuid::Uuid::new_v4().to_string(), events: Arc::new(events) });
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
    resolve_for_with_token(events, key, identity).map(|(events, _)| events)
}

/// Capture the payload and publication token under one cache read. A reused
/// client key never identifies replacement content as the same publication.
fn resolve_for_with_token(
    events: Option<Vec<Event>>, key: Option<String>, identity: Option<&crate::analysis_context::Identity>,
) -> Result<(Option<Arc<Vec<Event>>>, Option<String>), String> {
    if let Some(events) = events {
        // Legacy explicit-input commands remain supported. Each capture is its
        // own immutable publication, even if a caller reuses a textual key.
        return Ok((Some(Arc::new(events)), Some(uuid::Uuid::new_v4().to_string())));
    }
    let Some(key) = key else { return Ok((None, None)) };
    let owner = owner(identity);
    CACHE.lock().iter().find(|entry| entry.owner == owner && entry.key == key)
        .map(|entry| (Some(Arc::clone(&entry.events)), Some(entry.publication.clone())))
        .ok_or_else(|| MISS.to_string())
}

/// A synchronized capture linearizes at this short final publication check.
/// Expired entries cannot be replaced by a resupplied payload behind a receipt.
pub(crate) fn validate_token(key: &str, expected: &str, identity: Option<&crate::analysis_context::Identity>) -> Result<(), String> {
    let owner = owner(identity);
    let cache = CACHE.lock();
    let entry = cache.iter().find(|entry| entry.owner == owner && entry.key == key).ok_or(MISS)?;
    if entry.publication != expected { return Err(CHANGED.into()); }
    Ok(())
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
    take_for_with_token(events, key, identity).map(|(events, _)| events)
}

pub(crate) fn take_for_with_token(
    events: Option<Vec<Event>>, key: Option<String>, identity: Option<&crate::analysis_context::Identity>,
) -> Result<(Option<Vec<Event>>, Option<String>), String> {
    let (events, publication) = resolve_for_with_token(events, key, identity)?;
    Ok((events.map(|shared| Arc::try_unwrap(shared).unwrap_or_else(|arc| (*arc).clone())), publication))
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
    fn same_key_replacement_changes_publication_without_rebinding_old_payloads() {
        let identity = crate::analysis_context::Identity { case_id: "token-case".into(), analysis_id: uuid::Uuid::new_v4().to_string(), config_revision: 0, visibility_revision: 0 };
        let key = format!("same-key-{}", uuid::Uuid::new_v4());
        let mut original = Event::empty(); original.id = 7; original.event_ref = "same:7".into(); original.message = "original".into();
        store_for(key.clone(), vec![original.clone()], Some(&identity));
        let (old, token) = take_for_with_token(None, Some(key.clone()), Some(&identity)).unwrap();
        let (_, repeated) = take_for_with_token(None, Some(key.clone()), Some(&identity)).unwrap();
        assert_eq!(token, repeated);
        let mut replacement = original.clone(); replacement.message = "replacement".into();
        store_for(key.clone(), vec![replacement], Some(&identity));
        let (new, changed) = take_for_with_token(None, Some(key.clone()), Some(&identity)).unwrap();
        assert_ne!(token, changed);
        assert_eq!(validate_token(&key, token.as_deref().unwrap(), Some(&identity)).unwrap_err(), CHANGED);
        assert!(validate_token(&key, changed.as_deref().unwrap(), Some(&identity)).is_ok());
        assert_eq!(old.unwrap()[0].message, "original");
        assert_eq!(new.unwrap()[0].message, "replacement");
        let (_, explicit_a) = take_for_with_token(Some(vec![original.clone()]), Some(key.clone()), Some(&identity)).unwrap();
        let (_, explicit_b) = take_for_with_token(Some(vec![original]), Some(key.clone()), Some(&identity)).unwrap();
        assert_ne!(explicit_a, explicit_b);
        for suffix in ["a", "b", "c"] { store_for(format!("{key}-{suffix}"), Vec::new(), Some(&identity)); }
        assert_eq!(take_for_with_token(None, Some(key), Some(&identity)).unwrap_err(), MISS);
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
