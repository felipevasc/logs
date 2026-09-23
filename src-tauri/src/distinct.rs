//! Exact distinct count with a bounded in-memory set and a temporary disk spill.
use std::collections::HashSet;

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

    pub fn insert(&mut self, value: String) {
        if self.disk.is_none() {
            if let Some(count) = self.values.get_mut(&value) {
                *count += 1;
                return;
            }
            let bytes = value.len().saturating_add(64);
            if self.values.len() < Self::KEYS && self.bytes.saturating_add(bytes) <= Self::BYTES {
                self.bytes += bytes;
                self.values.insert(value, 1);
                return;
            }
            let db = rusqlite::Connection::open("")
                .expect("Não foi possível criar arquivo temporário para o ranking");
            db.progress_handler(10_000, Some(crate::operations::cancelled));
            db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-2048; PRAGMA temp_store=FILE; CREATE TABLE terms(v TEXT PRIMARY KEY,n INTEGER NOT NULL) WITHOUT ROWID; BEGIN").expect("Falha ao preparar ranking no disco");
            {
                let mut insert = db
                    .prepare("INSERT INTO terms(v,n) VALUES(?1,?2)")
                    .expect("Falha ao preparar ranking");
                for (value, count) in self.values.drain() {
                    insert
                        .execute(rusqlite::params![value, count as i64])
                        .expect("Falha ao transferir ranking para o disco");
                }
            }
            self.values.shrink_to_fit();
            self.bytes = 0;
            self.disk = Some(db);
        }
        self.disk
            .as_ref()
            .unwrap()
            .prepare_cached(
                "INSERT INTO terms(v,n) VALUES(?1,1) ON CONFLICT(v) DO UPDATE SET n=n+1",
            )
            .expect("Falha ao preparar contagem do ranking")
            .execute([value])
            .expect("Falha ao contar ranking no disco");
    }

    pub fn top(self, limit: usize) -> Vec<(String, usize)> {
        if let Some(db) = self.disk {
            let mut select = db
                .prepare("SELECT v,n FROM terms ORDER BY n DESC,v ASC LIMIT ?1")
                .expect("Falha ao ordenar ranking no disco");
            return select
                .query_map([limit.min(i64::MAX as usize) as i64], |row| {
                    Ok((row.get(0)?, row.get::<_, i64>(1)? as usize))
                })
                .expect("Falha ao consultar ranking")
                .collect::<Result<Vec<_>, _>>()
                .expect("Falha ao ler ranking");
        }
        let mut values: Vec<_> = self.values.into_iter().collect();
        values.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        values.truncate(limit);
        values
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
        if let Some(db) = &self.disk {
            self.count += db
                .execute("INSERT OR IGNORE INTO vals(v) VALUES(?1)", [value])
                .expect("Falha ao contar valores distintos no disco");
            return;
        }
        if self.values.contains(&value) {
            return;
        }
        self.bytes = self.bytes.saturating_add(value.len().saturating_add(64));
        self.values.insert(value);
        self.count = self.values.len();
        if self.values.len() >= 25_000 || self.bytes >= Self::BYTES {
            let mut db = rusqlite::Connection::open("")
                .expect("Não foi possível criar arquivo temporário para a contagem");
            db.progress_handler(10_000, Some(crate::operations::cancelled));
            db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA cache_size=-512; PRAGMA temp_store=FILE; CREATE TABLE vals(v TEXT PRIMARY KEY) WITHOUT ROWID;").expect("Falha ao preparar contagem");
            {
                let tx = db.transaction().expect("Falha na contagem");
                {
                    let mut stmt = tx
                        .prepare("INSERT INTO vals(v) VALUES(?1)")
                        .expect("Falha na contagem");
                    for v in self.values.drain() {
                        stmt.execute([v]).expect("Falha na contagem");
                    }
                }
                tx.commit().expect("Falha na contagem");
            }
            db.execute_batch("BEGIN").expect("Falha na contagem");
            self.disk = Some(db);
            self.values.shrink_to_fit();
            self.bytes = 0;
        }
    }
    pub fn len(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod counter_tests {
    use super::Counter;

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
