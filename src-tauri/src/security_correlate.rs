//! External ordered hit storage. No entity or window is materialised in RAM.
//! The slice algorithms in detections remain an independent semantic oracle.
use crate::detections::Hit;
use crate::security_budget::TrackedConnection as Connection;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

fn error(e: impl std::fmt::Display) -> String {
    format!("Correlação externa: {e}")
}
pub struct Store {
    db: Connection,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Selection {
    pub set: i64,
}
impl Store {
    pub fn new() -> Result<Self, String> {
        let db = Connection::open("").map_err(error)?;
        db.progress_handler(
            10000,
            Some(|| crate::operations::cancelled() || crate::security_budget::check().is_err()),
        );
        db.execute_batch("PRAGMA cache_size=-2048;PRAGMA temp_store=FILE;
          CREATE TABLE hits(rule INTEGER,k TEXT,ts INTEGER,ref TEXT,step INTEGER,extra TEXT,id INTEGER,payload TEXT,binding TEXT);
          CREATE TABLE selected(s INTEGER,id INTEGER,step INTEGER,PRIMARY KEY(s,id));
          CREATE TABLE active(n INTEGER PRIMARY KEY,ts INTEGER,step INTEGER,ref TEXT,extra TEXT);
          CREATE INDEX active_step ON active(step,n);CREATE INDEX active_ref ON active(ref);
          CREATE TABLE frequency(value TEXT PRIMARY KEY,n INTEGER);
          CREATE TABLE gaps(value INTEGER);
          CREATE TABLE extrema(n INTEGER PRIMARY KEY,value REAL);CREATE INDEX extrema_value ON extrema(value);
          CREATE TABLE identity_refs(ref TEXT PRIMARY KEY);
          CREATE TABLE ordered(n INTEGER PRIMARY KEY,ts INTEGER,ref TEXT,step INTEGER,extra TEXT,id INTEGER,payload TEXT,binding TEXT);
          CREATE INDEX ordered_binding ON ordered(binding,n);
          CREATE INDEX ordered_member ON ordered(id,step,n);
          BEGIN;").map_err(error)?;
        Ok(Self { db })
    }
    pub fn import(&self, groups: crate::security_store::Groups<Hit>) -> Result<(), String> {
        for entry in groups.into_records()?.into_iter()? {
            let (rule, k, hit) = entry?;
            self.db
                .prepare_cached("INSERT INTO hits VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)")
                .map_err(error)?
                .execute(params![
                    rule,
                    k,
                    hit.ts,
                    hit.reference,
                    hit.step,
                    hit.extra,
                    hit.id as i64,
                    serde_json::to_string(&hit).map_err(error)?,
                    hit.binding
                ])
                .map_err(error)?;
        }
        self.db
            .execute_batch("CREATE INDEX hit_order ON hits(rule,k,ts,ref,step);COMMIT;BEGIN;")
            .map_err(error)?;
        Ok(())
    }
    pub fn keys(
        &self,
        mut visit: impl FnMut(u32, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut stmt = self
            .db
            .prepare("SELECT rule,k FROM hits GROUP BY rule,k ORDER BY rule,k")
            .map_err(error)?;
        let mut rows = stmt.query([]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            visit(
                row.get(0).map_err(error)?,
                &row.get::<_, String>(1).map_err(error)?,
            )?;
        }
        Ok(())
    }
    pub fn group(&self, rule: u32, key: &str) -> Result<Group<'_>, String> {
        self.db.execute("DELETE FROM ordered", []).map_err(error)?;
        // The original stable sort and adjacent dedup are reproduced. Equal
        // references at different timestamps are deliberately not coalesced.
        let mut stmt=self.db.prepare("SELECT ts,ref,step,extra,id,payload,binding FROM hits WHERE rule=?1 AND k=?2 ORDER BY ts,ref,step,rowid").map_err(error)?;
        let mut rows = stmt.query(params![rule, key]).map_err(error)?;
        let mut previous: Option<(i64, String, u8, Option<String>)> = None;
        let mut n = 0i64;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let identity = (
                row.get::<_, i64>(0).map_err(error)?,
                row.get::<_, String>(1).map_err(error)?,
                row.get::<_, u8>(2).map_err(error)?,
                row.get::<_, Option<String>>(3).map_err(error)?,
            );
            if previous.as_ref() == Some(&identity) {
                continue;
            }
            self.db
                .prepare_cached("INSERT INTO ordered VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")
                .map_err(error)?
                .execute(params![
                    n,
                    identity.0,
                    identity.1,
                    identity.2,
                    identity.3,
                    row.get::<_, i64>(4).map_err(error)?,
                    row.get::<_, String>(5).map_err(error)?,
                    row.get::<_, Option<String>>(6).map_err(error)?
                ])
                .map_err(error)?;
            previous = Some(identity);
            n += 1;
        }
        Ok(Group {
            store: self,
            len: n as usize,
        })
    }
    pub fn selection(&self) -> Result<Selection, String> {
        let n: i64 = self
            .db
            .query_row("SELECT coalesce(max(s),-1)+1 FROM selected", [], |r| {
                r.get(0)
            })
            .map_err(error)?;
        Ok(Selection { set: n })
    }
    pub fn add(&self, selection: &Selection, id: usize, step: usize) -> Result<(), String> {
        self.db
            .prepare_cached("INSERT OR IGNORE INTO selected VALUES(?1,?2,?3)")
            .map_err(error)?
            .execute(params![selection.set, id as i64, step as i64])
            .map_err(error)?;
        Ok(())
    }
    pub fn members(
        &self,
        s: &Selection,
        mut visit: impl FnMut(usize, usize) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut stmt = self
            .db
            .prepare("SELECT id,step FROM selected WHERE s=?1 ORDER BY id")
            .map_err(error)?;
        let mut rows = stmt.query([s.set]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            visit(
                row.get::<_, i64>(0).map_err(error)? as usize,
                row.get::<_, i64>(1).map_err(error)? as usize,
            )?;
        }
        Ok(())
    }
    pub fn clear_identity(&self) -> Result<(), String> {
        self.db
            .execute("DELETE FROM identity_refs", [])
            .map_err(error)?;
        Ok(())
    }
    pub fn reference(&self, reference: &str) -> Result<(), String> {
        self.db
            .prepare_cached("INSERT OR IGNORE INTO identity_refs VALUES(?1)")
            .map_err(error)?
            .execute([reference])
            .map_err(error)?;
        Ok(())
    }
    pub fn identity(&self, prefix: &str) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut stmt = self
            .db
            .prepare("SELECT ref FROM identity_refs ORDER BY ref")
            .map_err(error)?;
        let mut rows = stmt.query([]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            let value: String = row.get(0).map_err(error)?;
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        Ok(format!("{prefix}-{:x}", hash.finalize()))
    }
}
pub struct Group<'a> {
    store: &'a Store,
    pub len: usize,
}
impl Group<'_> {
    pub fn signature(&self, rule: &str, key: &str, context: &str) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        for part in [rule, key, context, crate::evidence::NORMALIZATION_VERSION] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        for n in 0..self.len {
            let h = self.get(n)?;
            let bytes = serde_json::to_vec(&(
                h.ts,
                h.reference,
                h.step,
                h.extra,
                h.source,
                h.uncertain_time,
                h.binding,
            ))
            .map_err(error)?;
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        Ok(format!("{:x}", hash.finalize()))
    }
    pub fn ranges(&self, s: &Selection) -> Result<Option<Vec<(usize, usize)>>, String> {
        let mut stmt=self.store.db.prepare("SELECT o.n FROM selected s JOIN ordered o ON o.id=s.id AND o.step=s.step WHERE s.s=?1 ORDER BY o.n").map_err(error)?;
        let mut rows = stmt.query([s.set]).map_err(error)?;
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        while let Some(row) = rows.next().map_err(error)? {
            let n = row.get::<_, i64>(0).map_err(error)? as usize;
            match ranges.last_mut() {
                Some(last) if n <= last.1.saturating_add(1) => last.1 = n,
                _ => {
                    if ranges.len() >= 32768 {
                        return Ok(None);
                    }
                    ranges.push((n, n));
                }
            }
        }
        Ok(Some(ranges))
    }
    pub fn restore(&self, ranges: &Value) -> Result<Selection, String> {
        let ranges: Vec<(usize, usize)> = serde_json::from_value(ranges.clone()).map_err(error)?;
        let s = self.store.selection()?;
        for (left, right) in ranges {
            if left > right || right >= self.len {
                return Err("Checkpoint fora do grupo canônico; resultado não reutilizado".into());
            }
            self.store.db.execute("INSERT OR IGNORE INTO selected SELECT ?1,id,step FROM ordered WHERE n BETWEEN ?2 AND ?3 ORDER BY n",params![s.set,left as i64,right as i64]).map_err(error)?;
        }
        Ok(s)
    }
    pub fn get(&self, n: usize) -> Result<Hit, String> {
        let payload: String = self
            .store
            .db
            .prepare_cached("SELECT payload FROM ordered WHERE n=?1")
            .map_err(error)?
            .query_row([n as i64], |r| r.get(0))
            .map_err(error)?;
        serde_json::from_str(&payload).map_err(error)
    }
    pub fn range(&self, left: usize, right: usize) -> Result<Selection, String> {
        let s = self.store.selection()?;
        self.store.db.execute("INSERT OR IGNORE INTO selected SELECT ?1,id,step FROM ordered WHERE n BETWEEN ?2 AND ?3 ORDER BY n",params![s.set,left as i64,right as i64]).map_err(error)?;
        Ok(s)
    }
    pub fn one(&self, n: usize) -> Result<Selection, String> {
        self.range(n, n)
    }
    pub fn threshold(
        &self,
        window: i64,
        count: usize,
        distinct: bool,
        mut emit: impl FnMut(usize, usize, usize) -> Result<(), String>,
    ) -> Result<(), String> {
        self.store
            .db
            .execute("DELETE FROM frequency", [])
            .map_err(error)?;
        let mut left = 0;
        let mut cardinality = 0usize;
        let mut pending: Option<(usize, usize, usize)> = None;
        for right in 0..self.len {
            crate::operations::check()?;
            let hit = self.get(right)?;
            if distinct {
                cardinality += usize::from(self.adjust(hit.extra.as_deref().unwrap_or(""), 1)?);
            }
            while hit.ts.saturating_sub(self.get(left)?.ts) > window {
                if distinct {
                    cardinality -= usize::from(
                        self.adjust(self.get(left)?.extra.as_deref().unwrap_or(""), -1)?,
                    );
                }
                left += 1;
            }
            let n = if distinct {
                cardinality
            } else {
                right + 1 - left
            };
            if n >= count {
                match pending.as_mut() {
                    Some(last) if left <= last.1 + 1 => {
                        last.1 = right;
                        last.2 = last.2.max(if distinct { n } else { 0 });
                    }
                    _ => {
                        if let Some(last) = pending.take() {
                            emit(last.0, last.1, last.2)?;
                        }
                        pending = Some((left, right, if distinct { n } else { 0 }));
                    }
                }
            }
        }
        if let Some(last) = pending {
            emit(last.0, last.1, last.2)?;
        }
        Ok(())
    }
    fn adjust(&self, value: &str, delta: i64) -> Result<bool, String> {
        let old: i64 = self
            .store
            .db
            .prepare_cached("SELECT n FROM frequency WHERE value=?1")
            .map_err(error)?
            .query_row([value], |r| r.get(0))
            .optional()
            .map_err(error)?
            .unwrap_or(0);
        if old + delta == 0 {
            self.store
                .db
                .prepare_cached("DELETE FROM frequency WHERE value=?1")
                .map_err(error)?
                .execute([value])
                .map_err(error)?;
        } else {
            self.store.db.prepare_cached("INSERT INTO frequency VALUES(?1,?2) ON CONFLICT(value) DO UPDATE SET n=excluded.n").map_err(error)?.execute(params![value,old+delta]).map_err(error)?;
        }
        Ok(if delta > 0 { old == 0 } else { old == 1 })
    }
    pub fn beacon(&self, count: usize) -> Result<Option<i64>, String> {
        self.store
            .db
            .execute("DELETE FROM gaps", [])
            .map_err(error)?;
        if self.len < count.max(6) {
            return Ok(None);
        }
        for n in 1..self.len {
            let gap = self.get(n)?.ts.saturating_sub(self.get(n - 1)?.ts);
            if gap > 0 {
                self.store
                    .db
                    .prepare_cached("INSERT INTO gaps VALUES(?1)")
                    .map_err(error)?
                    .execute([gap])
                    .map_err(error)?;
            }
        }
        let n: i64 = self
            .store
            .db
            .query_row("SELECT count(*) FROM gaps", [], |r| r.get(0))
            .map_err(error)?;
        if n + 1 < count.max(6) as i64 {
            return Ok(None);
        }
        let median: i64 = self
            .store
            .db
            .query_row(
                "SELECT value FROM gaps ORDER BY value LIMIT 1 OFFSET ?1",
                [n / 2],
                |r| r.get(0),
            )
            .map_err(error)?;
        if median < 10_000 {
            return Ok(None);
        }
        let tolerance = (median as f64 * 0.2).max(2000.0);
        let regular: i64 = self
            .store
            .db
            .query_row(
                "SELECT count(*) FROM gaps WHERE abs(value-?1)<=?2",
                params![median, tolerance],
                |r| r.get(0),
            )
            .map_err(error)?;
        Ok((regular as f64 / n as f64 >= 0.8).then_some(median))
    }
    pub fn measured(
        &self,
        rule: &crate::detections::Compiled,
        partition: Option<i64>,
        mut emit: impl FnMut(usize, usize, Value) -> Result<(), String>,
    ) -> Result<(), String> {
        self.store
            .db
            .execute("DELETE FROM extrema", [])
            .map_err(error)?;
        let mut left = 0;
        let mut sum = 0.0;
        let mut was = false;
        let value = |n: usize| -> Result<f64, String> {
            Ok(self
                .get(n)?
                .extra
                .as_deref()
                .unwrap_or("0")
                .parse::<f64>()
                .unwrap_or(0.0))
        };
        for right in 0..self.len {
            let hit = self.get(right)?;
            while left < right && hit.ts.saturating_sub(self.get(left)?.ts) > rule.window {
                sum -= value(left)?;
                left += 1;
            }
            let current = value(right)?;
            sum += current;
            if let Some(a) = &rule.def.aggregate {
                if matches!(a.operation.as_str(), "min" | "max") {
                    self.store
                        .db
                        .execute(
                            if a.operation == "min" {
                                "DELETE FROM extrema WHERE value>=?1"
                            } else {
                                "DELETE FROM extrema WHERE value<=?1"
                            },
                            [current],
                        )
                        .map_err(error)?;
                    self.store
                        .db
                        .execute("DELETE FROM extrema WHERE n<?1", [left as i64])
                        .map_err(error)?;
                    self.store
                        .db
                        .execute(
                            "INSERT INTO extrema VALUES(?1,?2)",
                            params![right as i64, current],
                        )
                        .map_err(error)?;
                }
            }
            if partition.is_some_and(|b| hit.ts.div_euclid(rule.window) != b.saturating_add(1))
                || right + 1 < self.len && self.get(right + 1)?.ts == hit.ts
            {
                continue;
            }
            let count = right + 1 - left;
            if count < rule.counts[0] {
                was = false;
                continue;
            }
            let (matched, metrics) = if let Some(r) = &rule.def.ratio {
                let ratio = sum / count as f64;
                (
                    ratio >= r.gte,
                    json!({"numerator":sum as usize,"denominator":count,"ratio":ratio,"gte":r.gte}),
                )
            } else {
                let a = rule.def.aggregate.as_ref().unwrap();
                let result = match a.operation.as_str() {
                    "sum" => sum,
                    "avg" => sum / count as f64,
                    _ => self
                        .store
                        .db
                        .query_row("SELECT value FROM extrema ORDER BY n LIMIT 1", [], |r| {
                            r.get::<_, f64>(0)
                        })
                        .map_err(error)?,
                };
                (
                    result.is_finite()
                        && a.gte.is_none_or(|v| result >= v)
                        && a.lte.is_none_or(|v| result <= v),
                    json!({"operation":a.operation,"field":a.field,"value":result,"samples":count,"gte":a.gte,"lte":a.lte}),
                )
            };
            if matched && !was {
                emit(left, right, metrics)?;
            }
            was = matched;
        }
        Ok(())
    }
    pub fn sequence(
        &self,
        window: i64,
        counts: &[usize],
        ordered: bool,
        binding: Option<&str>,
        mut emit: impl FnMut(Vec<usize>) -> Result<(), String>,
    ) -> Result<(), String> {
        self.store
            .db
            .execute("DELETE FROM active", [])
            .map_err(error)?;
        for n in 0..self.len {
            crate::operations::check()?;
            let hit = self.get(n)?;
            if hit.binding.as_deref().is_some_and(|v| Some(v) != binding) {
                continue;
            }
            self.store
                .db
                .prepare_cached("DELETE FROM active WHERE ts<?1")
                .map_err(error)?
                .execute([hit.ts.saturating_sub(window)])
                .map_err(error)?;
            self.store
                .db
                .prepare_cached("INSERT INTO active VALUES(?1,?2,?3,?4,?5)")
                .map_err(error)?
                .execute(params![
                    n as i64,
                    hit.ts,
                    hit.step,
                    hit.reference,
                    hit.extra
                ])
                .map_err(error)?;
            let mut enough = true;
            for (step, count) in counts.iter().enumerate() {
                let available: i64 = self
                    .store
                    .db
                    .query_row(
                        "SELECT count(*) FROM active WHERE step=?1",
                        [step as i64],
                        |r| r.get(0),
                    )
                    .map_err(error)?;
                if available < *count as i64 {
                    enough = false;
                    break;
                }
            }
            if !enough {
                continue;
            }
            let mut chosen = Vec::new();
            if ordered {
                let mut used = std::collections::HashSet::new();
                let mut previous = i64::MIN;
                for (step, count) in counts.iter().enumerate() {
                    let mut stmt = self
                        .store
                        .db
                        .prepare("SELECT n,ts,ref FROM active WHERE step=?1 AND ts>?2 ORDER BY n")
                        .map_err(error)?;
                    let mut rows = stmt.query(params![step as i64, previous]).map_err(error)?;
                    let mut stage = 0;
                    let mut last = previous;
                    while let Some(row) = rows.next().map_err(error)? {
                        let reference: String = row.get(2).map_err(error)?;
                        if used.insert(reference) {
                            chosen.push(row.get::<_, i64>(0).map_err(error)? as usize);
                            last = row.get(1).map_err(error)?;
                            stage += 1;
                            if stage == *count {
                                break;
                            }
                        }
                    }
                    if stage != *count {
                        enough = false;
                        break;
                    }
                    previous = last;
                }
            } else {
                let stages = counts
                    .iter()
                    .enumerate()
                    .flat_map(|(s, c)| std::iter::repeat_n(s, *c))
                    .collect::<Vec<_>>();
                let mut owners = std::collections::HashMap::new();
                for slot in 0..stages.len() {
                    if !self.assign(
                        slot,
                        &stages,
                        &mut owners,
                        &mut std::collections::HashSet::new(),
                    )? {
                        enough = false;
                        break;
                    }
                }
                chosen.extend(owners.into_values().map(|(_, n)| n));
            }
            if enough {
                chosen.sort_unstable();
                emit(chosen)?;
                self.store
                    .db
                    .execute("DELETE FROM active", [])
                    .map_err(error)?;
            }
        }
        Ok(())
    }
    fn assign(
        &self,
        slot: usize,
        stages: &[usize],
        owners: &mut std::collections::HashMap<String, (usize, usize)>,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<bool, String> {
        let mut stmt = self
            .store
            .db
            .prepare("SELECT n,ref FROM active WHERE step=?1 ORDER BY n")
            .map_err(error)?;
        let mut rows = stmt.query([stages[slot] as i64]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            let n = row.get::<_, i64>(0).map_err(error)? as usize;
            let reference: String = row.get(1).map_err(error)?;
            if !visited.insert(reference.clone()) {
                continue;
            }
            let previous = owners.get(&reference).copied();
            if previous.is_none() || self.assign(previous.unwrap().0, stages, owners, visited)? {
                owners.insert(reference, (slot, n));
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn bindings(
        &self,
        mut visit: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut stmt = self
            .store
            .db
            .prepare(
                "SELECT DISTINCT binding FROM ordered WHERE binding IS NOT NULL ORDER BY binding",
            )
            .map_err(error)?;
        let mut rows = stmt.query([]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            visit(&row.get::<_, String>(0).map_err(error)?)?;
        }
        Ok(())
    }
}
