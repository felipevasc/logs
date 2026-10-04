//! Content-addressed interpretations. A rule change never changes a fact.
//! Only the derived overlay is stored; originals and current row IDs stay with the source.
use crate::security_budget::TrackedConnection as Connection;
use crate::{
    model::Event,
    security_normalize::{Normalized, SourceMapping},
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const VERSION: &str = "security-facts-2";

#[derive(Serialize, Deserialize)]
struct Fact {
    fields: Map<String, Value>,
    normalized: Normalized,
    timestamp: Option<i64>,
}

pub struct Facts {
    db: Connection,
    signature: String,
    pending: usize,
    current_key: String,
    pub hits: usize,
    pub computed: usize,
    pub predicate_reuses: usize,
    serial: i64,
    quota: u64,
    pub evicted: usize,
}

fn error(e: impl std::fmt::Display) -> String {
    format!("Fatos normalizados: {e}")
}

impl Facts {
    pub fn new(mappings: &[SourceMapping]) -> Result<Self, String> {
        let signature = crate::evidence::stable_id(
            VERSION,
            [
                crate::evidence::NORMALIZATION_VERSION.to_string(),
                serde_json::to_string(mappings).map_err(error)?,
                crate::analysis_runtime::fact_namespace(),
            ],
        );
        #[cfg(test)]
        let path = std::path::PathBuf::from("");
        #[cfg(not(test))]
        let path = {
            let dir = crate::config_dir().join(VERSION);
            std::fs::create_dir_all(&dir).map_err(error)?;
            dir.join(format!("{signature}.sqlite"))
        };
        #[cfg(not(test))]
        prune_directory(path.parent().unwrap(), &path, 4 * 1024 * 1024 * 1024)?;
        Self::open(&path, signature)
    }

    fn open(path: &Path, signature: String) -> Result<Self, String> {
        let db = Connection::open(path).map_err(error)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        db.execute_batch("PRAGMA cache_size=-2048; PRAGMA temp_store=FILE;
            CREATE TABLE IF NOT EXISTS facts(k TEXT PRIMARY KEY,payload TEXT NOT NULL,touched INTEGER NOT NULL) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS fact_age ON facts(touched);
            CREATE TABLE IF NOT EXISTS predicates(fact TEXT,plan TEXT,payload TEXT NOT NULL,PRIMARY KEY(fact,plan)) WITHOUT ROWID;
            BEGIN;").map_err(error)?;
        let serial = db
            .query_row("SELECT coalesce(max(touched),0) FROM facts", [], |r| {
                r.get(0)
            })
            .map_err(error)?;
        Ok(Self {
            db,
            signature,
            pending: 0,
            current_key: String::new(),
            hits: 0,
            computed: 0,
            predicate_reuses: 0,
            serial,
            quota: 512 * 1024 * 1024,
            evicted: 0,
        })
    }

    pub fn normalize(
        &mut self,
        event: &Event,
        mappings: &[SourceMapping],
    ) -> Result<(Event, Normalized), String> {
        crate::operations::check()?;
        // Every input read by normalization is included. Presentation enrichments
        // are excluded because the normalizer explicitly clears them.
        let mut hash = Sha256::new();
        hash.update(self.signature.as_bytes());
        hash.update(
            serde_json::to_vec(&(
                crate::security_normalize::event_ref(event),
                event.timestamp,
                &event.source,
                &event.code,
                &event.message,
                &event.raw,
                &event.fields,
                &event.parse_status,
            ))
            .map_err(error)?,
        );
        let key = format!("{:x}", hash.finalize());
        self.current_key = key.clone();
        let payload: Option<String> = self
            .db
            .prepare_cached("SELECT payload FROM facts WHERE k=?1")
            .map_err(error)?
            .query_row([&key], |r| r.get(0))
            .optional()
            .map_err(error)?;
        if let Some(payload) = payload {
            let (value, _credit) = crate::case_evidence::parse_exact_value(&payload)?.into_parts();
            let fact: Fact = serde_json::from_value(value).map_err(error)?;
            let mut derived = event.clone();
            derived.fields.retain(|key, _| !key.starts_with("_sec."));
            derived.fields.extend(fact.fields);
            derived.timestamp = fact.timestamp;
            derived.event_ref = crate::security_normalize::event_ref(event);
            derived.name.clear();
            derived.description.clear();
            self.hits += 1;
            return Ok((derived, fact.normalized));
        }
        let (derived, normalized) = crate::security_normalize::normalize(event, mappings);
        let fact = Fact {
            fields: derived
                .fields
                .iter()
                .filter(|(key, _)| key.starts_with("_sec."))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            timestamp: derived.timestamp,
            normalized: normalized.clone(),
        };
        let payload = serde_json::to_string(&fact).map_err(error)?;
        self.maintain(payload.len() as u64)?;
        self.serial += 1;
        self.db
            .prepare_cached("INSERT OR REPLACE INTO facts VALUES(?1,?2,?3)")
            .map_err(error)?
            .execute(params![key, payload, self.serial])
            .map_err(error)?;
        self.computed += 1;
        self.pending += 1;
        if self.pending >= 1024 {
            self.checkpoint()?;
        }
        Ok((derived, normalized))
    }

    pub fn checkpoint(&mut self) -> Result<(), String> {
        self.db.execute_batch("COMMIT; BEGIN;").map_err(error)?;
        self.pending = 0;
        Ok(())
    }
    pub fn content_key(&self) -> &str {
        &self.current_key
    }
    pub fn select_key(&mut self, key: String) {
        self.current_key = key;
    }
    pub fn restore_predicates(
        &mut self,
        plan: &str,
        memo: &mut [Option<bool>],
    ) -> Result<(), String> {
        memo.fill(None);
        let payload: Option<String> = self
            .db
            .prepare_cached("SELECT payload FROM predicates WHERE fact=?1 AND plan=?2")
            .map_err(error)?
            .query_row(params![self.current_key, plan], |r| r.get(0))
            .optional()
            .map_err(error)?;
        if let Some(payload) = payload {
            let values: Vec<(usize, bool)> = serde_json::from_str(&payload).map_err(error)?;
            for (id, value) in values {
                if let Some(slot) = memo.get_mut(id) {
                    *slot = Some(value);
                } else {
                    return Err("Checkpoint de predicados incompatível com o plano".into());
                }
            }
            self.predicate_reuses += 1;
        }
        Ok(())
    }
    pub fn save_predicates(&mut self, plan: &str, memo: &[Option<bool>]) -> Result<(), String> {
        let values = memo
            .iter()
            .enumerate()
            .filter_map(|(i, value)| value.map(|value| (i, value)))
            .collect::<Vec<_>>();
        self.db
            .prepare_cached("INSERT OR REPLACE INTO predicates VALUES(?1,?2,?3)")
            .map_err(error)?
            .execute(params![
                self.current_key,
                plan,
                serde_json::to_string(&values).map_err(error)?
            ])
            .map_err(error)?;
        self.pending += 1;
        if self.pending >= 1024 {
            self.checkpoint()?;
        }
        Ok(())
    }
    fn maintain(&mut self, incoming: u64) -> Result<(), String> {
        if self.pending % 32 != 0 && incoming < 1024 * 1024 {
            return Ok(());
        }
        let size:u64=self.db.query_row("SELECT (page_count-freelist_count)*page_size FROM pragma_page_count(),pragma_freelist_count(),pragma_page_size()",[],|r|r.get(0)).map_err(error)?;
        if size.saturating_add(incoming) < self.quota * 4 / 5 {
            return Ok(());
        }
        loop {
            let count: i64 = self
                .db
                .query_row("SELECT count(*) FROM facts", [], |r| r.get(0))
                .map_err(error)?;
            if count == 0 {
                break;
            }
            let batch = (count / 5).clamp(1, 10000);
            self.db.execute("DELETE FROM predicates WHERE fact IN (SELECT k FROM facts ORDER BY touched LIMIT ?1)",[batch]).map_err(error)?;
            self.evicted += self
                .db
                .execute(
                    "DELETE FROM facts WHERE k IN (SELECT k FROM facts ORDER BY touched LIMIT ?1)",
                    [batch],
                )
                .map_err(error)?;
            let size:u64=self.db.query_row("SELECT (page_count-freelist_count)*page_size FROM pragma_page_count(),pragma_freelist_count(),pragma_page_size()",[],|r|r.get(0)).map_err(error)?;
            if size.saturating_add(incoming) < self.quota * 3 / 5 {
                break;
            }
        }
        Ok(())
    }
}

/// Only disposable interpretations are evicted. Original source data and
/// explicitly frozen historical profiles are never removed by this cache.
#[cfg(not(test))]
fn prune_directory(directory: &Path, current: &Path, quota: u64) -> Result<(), String> {
    let mut files = std::fs::read_dir(directory)
        .map_err(error)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|v| v == "sqlite"))
        .filter_map(|entry| {
            let m = entry.metadata().ok()?;
            Some((m.modified().ok()?, entry.path(), m.len()))
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|entry| entry.0);
    let mut total = files.iter().map(|entry| entry.2).sum::<u64>();
    for (_, path, size) in files {
        if total <= quota - 512 * 1024 * 1024 {
            break;
        }
        if path != current {
            std::fs::remove_file(&path).map_err(error)?;
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

impl Drop for Facts {
    fn drop(&mut self) {
        let _ = self.db.execute_batch("ROLLBACK");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_disposable_cache_eviction_recomputes_without_changing_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Facts::open(&dir.path().join("facts.sqlite"), "small".into()).unwrap();
        cache.quota = 64 * 1024;
        let mut event = Event::empty();
        event.event_ref = "first".into();
        event
            .fields
            .insert("CommandLine".into(), Value::from("whoami"));
        let expected = cache.normalize(&event, &[]).unwrap().0.fields;
        for i in 0..1000 {
            let mut e = event.clone();
            e.event_ref = format!("many:{i}");
            cache.normalize(&e, &[]).unwrap();
        }
        assert!(cache.evicted > 0);
        let got = cache.normalize(&event, &[]).unwrap();
        assert_eq!(got.0.fields, expected);
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn cache_reuses_interpretation_but_preserves_current_identity_and_rejects_changed_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.sqlite");
        let mut event = Event::empty();
        event.event_ref = "source:v1:record:1".into();
        event.id = 7;
        event
            .fields
            .insert("CommandLine".into(), Value::from("whoami"));
        let mut first = Facts::open(&path, "same-version".into()).unwrap();
        let original = first.normalize(&event, &[]).unwrap();
        first.checkpoint().unwrap();
        drop(first);
        let mut second = Facts::open(&path, "same-version".into()).unwrap();
        event.id = 23;
        let cached = second.normalize(&event, &[]).unwrap();
        assert_eq!(cached.0.id, 23);
        assert_eq!(cached.0.fields, original.0.fields);
        assert_eq!(second.hits, 1);
        event
            .fields
            .insert("CommandLine".into(), Value::from("hostname"));
        assert_ne!(
            second.normalize(&event, &[]).unwrap().0.fields,
            cached.0.fields
        );
        assert_eq!(second.computed, 1);
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn cancelled_uncommitted_facts_are_recomputed_and_version_isolation_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("facts.sqlite");
        let event = Event::empty();
        {
            let mut store = Facts::open(&path, "v1".into()).unwrap();
            store.normalize(&event, &[]).unwrap();
        }
        let mut store = Facts::open(&path, "v1".into()).unwrap();
        store.normalize(&event, &[]).unwrap();
        assert_eq!(store.computed, 1);
        store.checkpoint().unwrap();
        drop(store);
        let mut store = Facts::open(&path, "v2".into()).unwrap();
        store.normalize(&event, &[]).unwrap();
        assert_eq!(store.hits, 0);
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_predicate_checkpoint_reuses_only_the_same_content_and_plan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let mut event = Event::empty();
        event.event_ref = "test:1".into();
        let mut cache = Facts::open(&path, "version".into()).unwrap();
        cache.normalize(&event, &[]).unwrap();
        cache
            .save_predicates("plan-a", &[Some(true), None, Some(false)])
            .unwrap();
        cache.checkpoint().unwrap();
        drop(cache);
        let mut cache = Facts::open(&path, "version".into()).unwrap();
        cache.normalize(&event, &[]).unwrap();
        let mut memo = [None; 3];
        cache.restore_predicates("plan-a", &mut memo).unwrap();
        assert_eq!(memo, [Some(true), None, Some(false)]);
        cache.restore_predicates("plan-b", &mut memo).unwrap();
        assert_eq!(memo, [None; 3]);
        event.message = "changed".into();
        cache.normalize(&event, &[]).unwrap();
        cache.restore_predicates("plan-a", &mut memo).unwrap();
        assert_eq!(memo, [None; 3]);
    }
}
