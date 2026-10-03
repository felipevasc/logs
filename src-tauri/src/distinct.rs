//! Exact distinct count with a bounded in-memory set and a temporary disk spill.
use std::collections::HashSet;

#[cfg(test)]
thread_local! { static SPILL_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(crate) fn with_spill_failure<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset { fn drop(&mut self) { SPILL_FAILURE.with(|flag| flag.set(self.0)); } }
    let _reset = Reset(SPILL_FAILURE.with(|flag| flag.replace(true)));
    f()
}
fn check_spill_failure() -> Result<(), String> {
    #[cfg(test)]
    if SPILL_FAILURE.with(std::cell::Cell::get) { return Err("Falha de disco temporário injetada no teste.".into()); }
    Ok(())
}

/// Exact frequency ranking. Spill before either the key budget or estimated
/// string storage exceeds the in-memory budget; SQLite's page cache is 2 MiB.
#[derive(Default)]
pub struct Terms {
    values: std::collections::HashMap<String, usize>,
    bytes: usize,
    disk: Option<rusqlite::Connection>,
}
impl Terms {
    const KEYS: usize = 25_000;
    const BYTES: usize = 8 * 1024 * 1024;

    pub fn insert(&mut self, value: String) { self.insert_impl(value, false).expect("Falha ao contar ranking no disco"); }
    pub(crate) fn try_insert(&mut self, value: String) -> Result<usize, String> { self.insert_impl(value, true) }
    fn insert_impl(&mut self, value: String, bounded: bool) -> Result<usize, String> {
        if bounded { check_spill_failure()?; }
        let bytes = value.len().saturating_add(64);
        if self.disk.is_none() {
            if let Some(count) = self.values.get_mut(&value) { *count += 1; return Ok(0); }
            if self.values.len() < Self::KEYS && self.bytes.saturating_add(bytes) <= Self::BYTES {
                self.bytes += bytes;
                self.values.insert(value, 1);
                return Ok(bytes);
            }
            let db = rusqlite::Connection::open("").map_err(|e| e.to_string())?;
            db.progress_handler(10_000, Some(crate::operations::cancelled));
            let cap = if bounded { format!("PRAGMA max_page_count={};", (crate::resources::analytics_bytes() / 4096).max(16)) } else { String::new() };
            db.execute_batch(&format!("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-2048; PRAGMA temp_store=FILE; {cap} CREATE TABLE terms(v TEXT PRIMARY KEY,n INTEGER NOT NULL) WITHOUT ROWID; BEGIN")).map_err(|e| e.to_string())?;
            {
                let mut insert = db.prepare("INSERT INTO terms(v,n) VALUES(?1,?2)").map_err(|e| e.to_string())?;
                for (value, count) in self.values.drain() { insert.execute(rusqlite::params![value, count as i64]).map_err(|e| e.to_string())?; }
            }
            self.values.shrink_to_fit();
            self.bytes = 0;
            self.disk = Some(db);
        }
        let n: i64 = self.disk.as_ref().unwrap().prepare_cached(
            "INSERT INTO terms(v,n) VALUES(?1,1) ON CONFLICT(v) DO UPDATE SET n=n+1 RETURNING n",
        ).map_err(|e| e.to_string())?.query_row([value], |row| row.get(0)).map_err(|e| e.to_string())?;
        Ok(if n == 1 { bytes } else { 0 })
    }
    pub fn top(self, limit: usize) -> Vec<(String, usize)> { self.try_top(limit).expect("Falha ao ler ranking") }
    pub(crate) fn try_top(self, limit: usize) -> Result<Vec<(String, usize)>, String> {
        if let Some(db) = self.disk {
            let mut select = db.prepare("SELECT v,n FROM terms ORDER BY n DESC,v ASC LIMIT ?1").map_err(|e| e.to_string())?;
            let values = select.query_map([limit.min(i64::MAX as usize) as i64], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as usize)))
                .map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
            return Ok(values);
        }
        let mut values: Vec<_> = self.values.into_iter().collect();
        values.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        values.truncate(limit);
        Ok(values)
    }

}

#[cfg(test)]
mod terms_tests {
    use super::Terms;
    #[test]
    fn exact_ranking_spills_on_key_budget_and_keeps_late_winners() {
        let mut terms = Terms::default();
        for i in 0..30_000 {
            terms.insert(format!("value-{i:05}"));
        }
        assert!(terms.disk.is_some());
        assert!(terms.values.is_empty());
        for _ in 0..8 {
            terms.insert("value-29999".into());
        }
        for _ in 0..4 {
            terms.insert("late value".into());
        }
        let top = terms.top(3);
        assert_eq!(
            top,
            vec![
                ("value-29999".into(), 9),
                ("late value".into(), 4),
                ("value-00000".into(), 1)
            ]
        );
    }
    #[test]
    fn long_values_spill_before_key_budget_and_preserve_unicode() {
        let mut terms = Terms::default();
        let value = "日".repeat(Terms::BYTES / 3 + 1);
        terms.insert(value.clone());
        assert!(terms.disk.is_some());
        assert!(terms.values.is_empty());
        terms.insert(value.clone());
        assert_eq!(terms.top(1), vec![(value, 2)]);
    }
}

pub struct Counter {
    values: HashSet<String>,
    bytes: usize,
    disk: Option<rusqlite::Connection>,
    count: usize,
}
impl Default for Counter {
    fn default() -> Self {
        Self {
            values: HashSet::new(),
            bytes: 0,
            disk: None,
            count: 0,
        }
    }
}
impl Counter {
    const BYTES: usize = 2 * 1024 * 1024;
    pub fn insert(&mut self, value: String) {
        self.insert_impl(value, false).expect("Falha ao contar valores distintos no disco");
    }
    /// Returns newly retained key payload, for a shared aggregate budget.
    /// Callers that need recoverable resource errors must use this API.
    pub(crate) fn try_insert(&mut self, value: String) -> Result<usize, String> {
        self.insert_impl(value, true)
    }
    fn insert_impl(&mut self, value: String, bounded: bool) -> Result<usize, String> {
        if bounded { check_spill_failure()?; }
        let bytes = value.len().saturating_add(64);
        if let Some(db) = &self.disk {
            let added = db.prepare_cached("INSERT OR IGNORE INTO vals(v) VALUES(?1)")
                .and_then(|mut statement| statement.execute([value]))
                .map_err(|e| e.to_string())?;
            self.count += added;
            return Ok(if added > 0 { bytes } else { 0 });
        }
        if self.values.contains(&value) { return Ok(0); }
        self.bytes = self.bytes.saturating_add(bytes);
        self.values.insert(value);
        self.count = self.values.len();
        if self.values.len() >= 25_000 || self.bytes >= Self::BYTES {
            let mut db = rusqlite::Connection::open("").map_err(|e| e.to_string())?;
            db.progress_handler(10_000, Some(crate::operations::cancelled));
            let pages = (crate::resources::analytics_bytes() / 4096).max(16);
            let cap = if bounded { format!("PRAGMA max_page_count={pages};") } else { String::new() };
            db.execute_batch(&format!("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-512; PRAGMA temp_store=FILE; {cap} CREATE TABLE vals(v TEXT PRIMARY KEY) WITHOUT ROWID;")).map_err(|e| e.to_string())?;
            {
                let tx = db.transaction().map_err(|e| e.to_string())?;
                {
                    let mut stmt = tx.prepare("INSERT INTO vals(v) VALUES(?1)").map_err(|e| e.to_string())?;
                    for v in self.values.drain() { stmt.execute([v]).map_err(|e| e.to_string())?; }
                }
                tx.commit().map_err(|e| e.to_string())?;
            }
            db.execute_batch("BEGIN").map_err(|e| e.to_string())?;
            self.disk = Some(db);
            self.values.shrink_to_fit();
            self.bytes = 0;
        }
        Ok(bytes)
    }
    pub fn len(&self) -> usize { self.count }
}

#[cfg(test)]
mod counter_tests {
    use super::Counter;

    #[test]
    fn cached_spill_statement_preserves_fallible_counts_and_new_payload_budget() {
        let mut counter = Counter::default();
        let first = "日".repeat(Counter::BYTES / 3 + 1);
        assert_eq!(counter.try_insert(first.clone()).unwrap(), first.len() + 64);
        assert!(counter.disk.is_some());
        assert_eq!(counter.try_insert(first).unwrap(), 0);
        for n in 0..1000 {
            let value = format!("東京-{n}");
            assert_eq!(counter.try_insert(value.clone()).unwrap(), value.len() + 64);
            assert_eq!(counter.try_insert(value).unwrap(), 0);
        }
        assert_eq!(counter.len(), 1001);
    }

    #[test]
    fn distinct_spills_by_bytes_before_key_count() {
        let mut counter = Counter::default();
        let value = "日".repeat(Counter::BYTES / 3 + 1);
        counter.insert(value.clone());
        assert!(counter.disk.is_some());
        assert!(counter.values.is_empty());
        counter.insert(value);
        counter.insert("another".into());
        assert_eq!(counter.len(), 2);
    }
}
