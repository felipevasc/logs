//! Bounded working storage. SQLite temporary databases are removed on close.
//! Storage failures fail the analysis; they never silently discard evidence.
use crate::security_budget::TrackedConnection as Connection;
use rusqlite::{params, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Seek, Write};

/// Sequential spill file: intermediate findings never accumulate in a Vec.
pub struct Spool<T> {
    file: BufWriter<std::fs::File>,
    tracked: crate::security_budget::TrackedFile,
    marker: std::marker::PhantomData<T>,
}
impl<T: Serialize + DeserializeOwned> Spool<T> {
    pub fn new() -> Result<Self, String> {
        let (file, tracked) = crate::security_budget::TrackedFile::new()?;
        Ok(Self {
            file: BufWriter::with_capacity(65536, file),
            tracked,
            marker: Default::default(),
        })
    }
    pub fn push(&mut self, value: T) -> Result<(), String> {
        let bytes = serde_json::to_vec(&value).map_err(db_error)?;
        self.file
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .map_err(db_error)?;
        self.file.write_all(&bytes).map_err(db_error)?;
        crate::security_budget::check()
    }
    pub fn into_iter(mut self) -> Result<SpoolIter<T>, String> {
        self.file.flush().map_err(db_error)?;
        let mut file = self.file.into_inner().map_err(db_error)?;
        file.rewind().map_err(db_error)?;
        Ok(SpoolIter {
            file: BufReader::with_capacity(65536, file),
            _tracked: self.tracked,
            marker: Default::default(),
        })
    }
}
pub struct SpoolIter<T> {
    file: BufReader<std::fs::File>,
    _tracked: crate::security_budget::TrackedFile,
    marker: std::marker::PhantomData<T>,
}
pub struct Records<T> {
    db: Connection,
    marker: std::marker::PhantomData<T>,
}
impl<T: Serialize + DeserializeOwned> Records<T> {
    pub fn new() -> Result<Self, String> {
        let db = Connection::open("").map_err(db_error)?;
        db.execute_batch("PRAGMA cache_size=-1024; PRAGMA temp_store=FILE; CREATE TABLE records(id INTEGER PRIMARY KEY,payload TEXT); BEGIN;").map_err(db_error)?;
        Ok(Self {
            db,
            marker: Default::default(),
        })
    }
    pub fn insert(&self, id: usize, value: &T) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO records VALUES(?1,?2)",
                params![id as i64, serde_json::to_string(value).map_err(db_error)?],
            )
            .map_err(db_error)?;
        Ok(())
    }
    pub fn get(&self, id: usize) -> Result<Option<T>, String> {
        let value: Option<String> = self
            .db
            .query_row(
                "SELECT payload FROM records WHERE id=?1",
                [id as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        value
            .map(|s| serde_json::from_str(&s).map_err(db_error))
            .transpose()
    }
}
impl<T: DeserializeOwned> Iterator for SpoolIter<T> {
    type Item = Result<T, String>;
    fn next(&mut self) -> Option<Self::Item> {
        let mut size = [0u8; 8];
        match self.file.read(&mut size[..1]) {
            Ok(0) => return None,
            Err(e) => return Some(Err(db_error(e))),
            _ => {}
        }
        let mut read = || -> Result<T, String> {
            self.file.read_exact(&mut size[1..]).map_err(db_error)?;
            let len = u64::from_le_bytes(size) as usize;
            if len > 64 * 1024 * 1024 {
                return Err("Um achado individual excedeu 64 MiB; evidência não foi descartada silenciosamente".into());
            }
            let mut bytes = vec![0; len];
            self.file.read_exact(&mut bytes).map_err(db_error)?;
            serde_json::from_slice(&bytes).map_err(db_error)
        };
        Some(read())
    }
}

const MEMORY_BYTES: usize = 24 * 1024 * 1024;
const GROUP_BYTES: usize = 32 * 1024 * 1024;
/// All production analysis/page workers share this lane. Disk caches, working
/// groups and transport buffers have separate bounded allocations within it.
pub fn working_lane() -> Result<crate::security_budget::Guard, String> {
    crate::security_budget::enter()
}

pub struct Groups<T> {
    memory: BTreeMap<(u32, String), Vec<T>>,
    bytes: usize,
    disk: Option<Connection>,
}
impl<T> Default for Groups<T> {
    fn default() -> Self {
        Self {
            memory: BTreeMap::new(),
            bytes: 0,
            disk: None,
        }
    }
}
fn db_error(error: impl std::fmt::Display) -> String {
    format!("Armazenamento temporário da análise: {error}")
}
impl<T: Serialize + DeserializeOwned> Groups<T> {
    /// Transfer individual records without materialising any hot key.
    pub fn into_records(self) -> Result<Spool<(u32, String, T)>, String> {
        let mut output = Spool::new()?;
        if let Some(conn) = self.disk {
            conn.execute_batch("COMMIT").map_err(db_error)?;
            let mut stmt = conn
                .prepare("SELECT rule,k,payload FROM hits ORDER BY rule,k,rowid")
                .map_err(db_error)?;
            let mut rows = stmt.query([]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                crate::operations::check()?;
                let json: String = row.get(2).map_err(db_error)?;
                output.push((
                    row.get(0).map_err(db_error)?,
                    row.get(1).map_err(db_error)?,
                    serde_json::from_str(&json).map_err(db_error)?,
                ))?;
            }
        } else {
            for ((rule, key), values) in self.memory {
                for value in values {
                    output.push((rule, key.clone(), value))?;
                }
            }
        }
        Ok(output)
    }
    pub fn push(&mut self, rule: u32, key: &str, value: T) -> Result<(), String> {
        crate::security_budget::check()?;
        let json = serde_json::to_string(&value).map_err(db_error)?;
        if self.disk.is_none()
            && self.bytes.saturating_add(json.len() + key.len() + 128) > MEMORY_BYTES
        {
            let conn = Connection::open("").map_err(db_error)?;
            conn.execute_batch("PRAGMA cache_size=-4096; PRAGMA temp_store=FILE; CREATE TABLE hits(rule INTEGER, k TEXT, payload TEXT); CREATE INDEX hits_group ON hits(rule,k); BEGIN;").map_err(db_error)?;
            for ((r, k), values) in std::mem::take(&mut self.memory) {
                for v in values {
                    conn.execute(
                        "INSERT INTO hits VALUES(?1,?2,?3)",
                        params![r, k, serde_json::to_string(&v).map_err(db_error)?],
                    )
                    .map_err(db_error)?;
                }
            }
            self.disk = Some(conn);
            self.bytes = 0;
        }
        if let Some(conn) = &self.disk {
            conn.prepare_cached("INSERT INTO hits VALUES(?1,?2,?3)")
                .map_err(db_error)?
                .execute(params![rule, key, json])
                .map_err(db_error)?;
        } else {
            self.bytes += json.len() + key.len() + 128;
            self.memory
                .entry((rule, key.into()))
                .or_default()
                .push(value);
        }
        Ok(())
    }
    pub fn into_iter(self) -> Result<GroupIter<T>, String> {
        if let Some(conn) = self.disk {
            conn.execute_batch("COMMIT").map_err(db_error)?;
            Ok(GroupIter::Disk {
                conn,
                last: None,
                marker: std::marker::PhantomData,
            })
        } else {
            Ok(GroupIter::Memory(self.memory.into_iter()))
        }
    }
}
pub enum GroupIter<T> {
    Memory(std::collections::btree_map::IntoIter<(u32, String), Vec<T>>),
    Disk {
        conn: Connection,
        last: Option<(u32, String)>,
        marker: std::marker::PhantomData<T>,
    },
}
impl<T: DeserializeOwned> Iterator for GroupIter<T> {
    type Item = Result<((u32, String), Vec<T>), String>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Memory(iter) => iter.next().map(Ok),
            Self::Disk { conn, last, .. } => {
                let key: Result<Option<(u32, String)>, _> = match last.as_ref() {
                    Some((rule, key)) => conn
                        .query_row(
                            "SELECT rule,k FROM hits WHERE (rule,k) > (?1,?2) ORDER BY rule,k LIMIT 1",
                            params![rule, key],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional(),
                    None => conn
                        .query_row("SELECT rule,k FROM hits ORDER BY rule,k LIMIT 1", [], |r| {
                            Ok((r.get(0)?, r.get(1)?))
                        })
                        .optional(),
                };
                let key = match key {
                    Ok(Some(k)) => k,
                    Ok(None) => return None,
                    Err(e) => return Some(Err(db_error(e))),
                };
                *last = Some(key.clone());
                let read = || -> Result<Vec<T>, String> {
                    let mut stmt = conn
                        .prepare("SELECT payload FROM hits WHERE rule=?1 AND k=?2")
                        .map_err(db_error)?;
                    let rows = stmt
                        .query_map(params![key.0, key.1], |r| r.get::<_, String>(0))
                        .map_err(db_error)?;
                    let mut values = Vec::new();
                    let mut bytes = 0;
                    for row in rows {
                        crate::operations::check()?;
                        crate::security_budget::check()?;
                        let json = row.map_err(db_error)?;
                        bytes += json.len() + 128;
                        if bytes > GROUP_BYTES {
                            return Err("Uma chave de correlação excedeu 32 MiB. A análise foi interrompida sem descartar evidências; refine o Caso ou os campos de vínculo da regra.".into());
                        }
                        values.push(serde_json::from_str(&json).map_err(db_error)?);
                    }
                    Ok(values)
                };
                Some(read().map(|v| (key, v)))
            }
        }
    }
}

#[derive(Default)]
pub struct Seen {
    memory: HashSet<String>,
    bytes: usize,
    disk: Option<Connection>,
}
impl Seen {
    pub fn insert(&mut self, key: String) -> Result<bool, String> {
        if self.disk.is_none() && self.bytes + key.len() + 64 > 4 * 1024 * 1024 {
            let conn = Connection::open("").map_err(db_error)?;
            conn.execute_batch("PRAGMA cache_size=-1024; CREATE TABLE seen(k TEXT PRIMARY KEY) WITHOUT ROWID; BEGIN;")
                .map_err(db_error)?;
            for k in self.memory.drain() {
                conn.execute("INSERT INTO seen VALUES(?1)", [k])
                    .map_err(db_error)?;
            }
            self.disk = Some(conn);
            self.bytes = 0;
        }
        if let Some(conn) = &self.disk {
            Ok(conn
                .prepare_cached("INSERT OR IGNORE INTO seen VALUES(?1)")
                .map_err(db_error)?
                .execute([key])
                .map_err(db_error)?
                == 1)
        } else {
            if self.memory.contains(&key) {
                return Ok(false);
            }
            self.bytes += key.len() + 64;
            Ok(self.memory.insert(key))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlite_spill_preserves_every_member_and_order() {
        let mut groups = Groups::default();
        let mut expected = BTreeMap::<(u32, String), Vec<String>>::new();
        for i in 0..1200 {
            let key = (i % 3, format!("key-{}", i % 12));
            let value = format!("{i}:{}", "x".repeat(32768));
            expected.entry(key.clone()).or_default().push(value.clone());
            groups.push(key.0, &key.1, value).unwrap();
        }
        assert!(groups.disk.is_some());
        let actual: BTreeMap<_, _> = groups
            .into_iter()
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(actual, expected);
        let mut seen = Seen::default();
        for i in 0..70000 {
            assert!(seen.insert(format!("identity-{i}")).unwrap());
        }
        assert!(seen.disk.is_some());
        for i in 0..70000 {
            assert!(!seen.insert(format!("identity-{i}")).unwrap());
        }
    }
}
