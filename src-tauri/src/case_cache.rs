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

/// Publication captured by a successful synchronization. Exact field actions
/// bind this token so a reused client key cannot silently select new evidence.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReceipt {
    pub case_content_token: String,
}

pub fn store(key: String, events: Vec<Event>) {
    store_for(key, events, None);
}

fn store_for(
    key: String,
    events: Vec<Event>,
    identity: Option<&crate::analysis_context::Identity>,
) -> String {
    let owner = owner(identity);
    let publication = uuid::Uuid::new_v4().to_string();
    let shared = Arc::new(events);
    let mut retired = Vec::with_capacity(3);
    let mut cache = CACHE.lock();
    for position in (0..cache.len()).rev() {
        if cache[position].owner == owner && cache[position].key == key {
            retired.push(cache.remove(position));
        }
    }
    if cache.len() >= 3 {
        retired.push(cache.remove(0));
    }
    cache.push(Entry { owner, key, publication: publication.clone(), events: shared });
    drop(cache);
    // The last Arc can own a large nested Event graph. Destruction must not
    // serialize other Cases' captures/token checks behind the global mutex.
    drop(retired);
    publication
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
pub(crate) fn resolve_for_with_token(
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

/// Find the original vector position without cloning any event payload. Legacy
/// ID-only detail preserves its first match; exact refs reject duplicates.
pub(crate) fn record_position(events: &[Event], id: usize, event_ref: Option<&str>) -> Result<Option<usize>, String> {
    crate::operations::check()?;
    let mut first_id = None;
    let mut found = None;
    for (position, event) in events.iter().enumerate() {
        if position % 256 == 0 { crate::operations::check()?; }
        if event.id != id { continue; }
        first_id.get_or_insert(position);
        if event_ref.is_none_or(|expected| event.event_ref == expected) {
            if found.is_some() { return Err("A referência exata do registro é ambígua neste Caso.".into()); }
            found = Some(position);
            if event_ref.is_none() { break; }
        }
    }
    crate::operations::check()?;
    // Retain an ID-only mismatch for the exact consumer's identity error.
    Ok(found.or(first_id))
}

#[cfg(test)]
fn select_record(events: Option<&[Event]>, id: usize, event_ref: Option<&str>) -> Result<Option<Vec<Event>>, String> {
    let Some(events) = events else { crate::operations::check()?; return Ok(None); };
    let position = record_position(events, id, event_ref)?;
    let selected = position.map(|position| events[position].clone()).into_iter().collect();
    crate::operations::check()?;
    Ok(Some(selected))
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
) -> Result<SyncReceipt, String> {
    if key.is_empty() || key.len() > 512 {
        return Err("Chave de caso inválida.".into());
    }
    if let Some(identity) = &analysis_context {
        crate::analysis_runtime::validate_identity(identity)?;
    }
    let case_content_token = store_for(key, events, analysis_context.as_ref());
    Ok(SyncReceipt { case_content_token })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_single_record_capture_retains_one_copy_and_distinguishes_absent_case_from_dataset() {
        let key = format!("single-record-{}", uuid::Uuid::new_v4());
        let events: Vec<_> = (0..32).map(|id| {
            let mut event = Event::empty(); event.id = id; event.event_ref = format!("single:{id}");
            event.raw = "x".repeat(16_384); event
        }).collect();
        store_for(key.clone(), events, None);
        let (first, first_token) = resolve_for_with_token(None, Some(key.clone()), None).unwrap();
        let (second, second_token) = resolve_for_with_token(None, Some(key), None).unwrap();
        let first = first.unwrap(); let second = second.unwrap();
        assert!(Arc::ptr_eq(&first, &second), "admission shares the immutable payload");
        assert_eq!(first_token, second_token);
        let mut chosen = select_record(Some(first.as_slice()), 17, Some("single:17")).unwrap().unwrap();
        assert_eq!(chosen.len(), 1); assert_eq!(chosen[0].id, 17);
        assert_eq!(chosen.iter().map(|event| event.raw.len()).sum::<usize>(), 16_384);
        assert_eq!(first.iter().map(|event| event.raw.len()).sum::<usize>(), 32 * 16_384);
        chosen[0].raw.push_str("changed copy");
        assert_eq!(first[17].raw.len(), 16_384, "transforming the selected copy cannot mutate evidence");
        assert!(select_record(Some(first.as_slice()), 100, None).unwrap().unwrap().is_empty());
        assert!(select_record(None, 100, None).unwrap().is_none());
        let mut sparse = Event::empty(); sparse.id = 500;
        assert_eq!(select_record(Some(&[sparse]), 500, None).unwrap().unwrap()[0].id, 500);
        let token = crate::operations::token(Some("single-record-cancel".into())).unwrap();
        assert!(crate::operations::run_with_token(token, || {
            crate::operations::cancel_id("single-record-cancel");
            select_record(Some(first.as_slice()), 17, Some("single:17"))
        }).is_err());
    }
    #[test]
    fn single_record_selection_preserves_legacy_order_and_rejects_duplicate_exact_handles() {
        let mut first = Event::empty(); first.id = 1; first.event_ref = "A".into(); first.raw = "first".into();
        let mut second = first.clone(); second.event_ref = "B".into(); second.raw = "second".into();
        let records = vec![first.clone(), second];
        assert_eq!(select_record(Some(&records), 1, None).unwrap().unwrap()[0].raw, "first");
        assert_eq!(select_record(Some(&records), 1, Some("B")).unwrap().unwrap()[0].raw, "second");
        let mut duplicate = first.clone(); duplicate.raw = "different body".into();
        assert!(select_record(Some(&[first, duplicate]), 1, Some("A")).unwrap_err().contains("ambígua"));
    }
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
