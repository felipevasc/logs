//! Exact distinct count with a bounded in-memory set and a temporary disk spill.
use std::collections::HashSet;
pub struct Counter {
    values: HashSet<String>,
    disk: Option<rusqlite::Connection>,
    count: usize,
}
impl Default for Counter {
    fn default() -> Self {
        Self {
            values: HashSet::new(),
            disk: None,
            count: 0,
        }
    }
}
impl Counter {
    pub fn insert(&mut self, value: String) {
        if let Some(db) = &self.disk {
            self.count += db
                .execute("INSERT OR IGNORE INTO vals(v) VALUES(?1)", [value])
                .expect("Falha ao contar valores distintos no disco");
            return;
        }
        self.values.insert(value);
        self.count = self.values.len();
        if self.values.len() >= 25_000 {
            let mut db = rusqlite::Connection::open("")
                .expect("Não foi possível criar arquivo temporário para a contagem");
            db.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE vals(v TEXT PRIMARY KEY) WITHOUT ROWID;").expect("Falha ao preparar contagem");
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
        }
    }
    pub fn len(&self) -> usize {
        self.count
    }
}
