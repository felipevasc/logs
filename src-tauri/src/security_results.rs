//! Disk-backed complete findings. Only a bounded page crosses the UI/MCP boundary.
use crate::detections::{Detection, Triage};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn err(e: impl std::fmt::Display) -> String {
    format!("Resultados da análise em disco: {e}")
}
fn execute<P: rusqlite::Params>(db: &Connection, sql: &str, params: P) -> rusqlite::Result<usize> {
    db.prepare_cached(sql)?.execute(params)
}
fn row<T, P: rusqlite::Params, F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>>(
    db: &Connection,
    sql: &str,
    params: P,
    f: F,
) -> rusqlite::Result<T> {
    db.prepare_cached(sql)?.query_row(params, f)
}
pub struct Results {
    db: Mutex<Connection>,
    pub metadata: Value,
}
pub struct Writer {
    db: Connection,
    next: i64,
}
impl Writer {
    pub fn new() -> Result<Self, String> {
        let db = Connection::open("").map_err(err)?;
        db.execute_batch("PRAGMA cache_size=-2048; PRAGMA temp_store=FILE; PRAGMA mmap_size=0;
            CREATE TABLE findings(n INTEGER PRIMARY KEY, id TEXT UNIQUE, root INTEGER, level INTEGER, impact INTEGER, start INTEGER, end INTEGER, namespace TEXT, payload TEXT, pattern TEXT, display_root INTEGER, participants TEXT);
            CREATE INDEX finding_root ON findings(root,level);
            CREATE INDEX finding_display ON findings(display_root,level);
            CREATE TABLE parents(n INTEGER PRIMARY KEY,p INTEGER,size INTEGER);
            CREATE TABLE members(n INTEGER,ref TEXT,event INTEGER,namespace TEXT, PRIMARY KEY(n,ref));
            CREATE INDEX member_ref ON members(namespace,ref); CREATE INDEX member_event ON members(event,n);
            CREATE TABLE owners(namespace TEXT,ref TEXT,n INTEGER,PRIMARY KEY(namespace,ref));
            CREATE TABLE tactics(n INTEGER,t TEXT,PRIMARY KEY(n,t)); CREATE INDEX tactic_t ON tactics(t,n);
            CREATE TABLE techniques(n INTEGER,id TEXT,name TEXT,t TEXT,PRIMARY KEY(n,id)); CREATE INDEX technique_t ON techniques(t,n);
            CREATE TABLE entities(n INTEGER,namespace TEXT,col TEXT,value TEXT,label TEXT,level INTEGER,PRIMARY KEY(n,namespace,col,value));
            CREATE INDEX entity_key ON entities(namespace,col,value,level);
            CREATE TABLE entity_stats(namespace TEXT,col TEXT,value TEXT,events INTEGER,failures INTEGER,PRIMARY KEY(namespace,col,value));
            CREATE TABLE episodes(root INTEGER PRIMARY KEY,id TEXT,level INTEGER,impact INTEGER,start INTEGER,end INTEGER,count INTEGER,payload TEXT,pattern TEXT);
            CREATE INDEX episode_pattern ON episodes(pattern,root);
            CREATE TABLE cards(root INTEGER PRIMARY KEY,id TEXT,level INTEGER,impact INTEGER,start INTEGER,end INTEGER,count INTEGER,payload TEXT);
            BEGIN;").map_err(err)?;
        Ok(Self { db, next: 0 })
    }
    fn root(&self, n: i64) -> Result<(i64, i64), String> {
        let mut cur = n;
        loop {
            let (parent, size): (i64, i64) =
                row(&self.db, "SELECT p,size FROM parents WHERE n=?1", [cur], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(err)?;
            if parent == cur {
                if cur != n {
                    execute(&self.db, "UPDATE parents SET p=?1 WHERE n=?2", params![cur, n]).map_err(err)?;
                }
                return Ok((cur, size));
            }
            cur = parent;
        }
    }
    pub fn push(&mut self, d: &Detection) -> Result<(), String> {
        crate::security_budget::check()?;
        let impact = match d.severity.as_str() {
            "critical" => 4,
            "high" => 3,
            "medium" => 2,
            "low" => 1,
            _ => 0,
        };
        let n = self.next;
        self.next += 1;
        let inserted = execute(
            &self.db,
            "INSERT OR IGNORE INTO findings VALUES(?1,?2,?1,?3,?4,?5,?6,?7,?8,?9,?1,?10)",
            params![
                n,
                d.id,
                d.evidence.evidence_level,
                impact,
                d.start,
                d.end,
                d.namespace,
                serde_json::to_string(d).map_err(err)?,
                crate::security_grouping::pattern(d),
                serde_json::to_string(&d.participants).map_err(err)?
            ],
        )
        .map_err(err)?;
        if inserted == 0 {
            return Ok(());
        }
        execute(&self.db, "INSERT INTO parents VALUES(?1,?1,1)", [n]).map_err(err)?;
        for member in &d.evidence.evidence_members {
            execute(
                &self.db,
                "INSERT OR IGNORE INTO members VALUES(?1,?2,?3,?4)",
                params![n, member.event_ref, member.event_id as i64, d.namespace],
            )
            .map_err(err)?;
            if d.evidence.evidence_level == 0 {
                continue;
            }
            let owner: Option<i64> = row(
                &self.db,
                "SELECT n FROM owners WHERE namespace=?1 AND ref=?2",
                params![d.namespace, member.event_ref],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
            if let Some(owner) = owner {
                let (mut a, mut sa) = self.root(n)?;
                let (mut b, mut sb) = self.root(owner)?;
                if a != b {
                    if sa < sb {
                        std::mem::swap(&mut a, &mut b);
                        std::mem::swap(&mut sa, &mut sb);
                    }
                    execute(&self.db, "UPDATE parents SET p=?1 WHERE n=?2", params![a, b]).map_err(err)?;
                    execute(&self.db, "UPDATE parents SET size=?1 WHERE n=?2", params![sa + sb, a]).map_err(err)?;
                }
            } else {
                execute(&self.db, "INSERT INTO owners VALUES(?1,?2,?3)", params![d.namespace, member.event_ref, n])
                    .map_err(err)?;
            }
        }
        for t in &d.tactics {
            execute(&self.db, "INSERT OR IGNORE INTO tactics VALUES(?1,?2)", params![n, t]).map_err(err)?;
        }
        for a in &d.attack {
            execute(
                &self.db,
                "INSERT OR IGNORE INTO techniques VALUES(?1,?2,?3,?4)",
                params![n, a.id, a.name, a.tactics.first()],
            )
            .map_err(err)?;
        }
        for e in &d.entities {
            execute(
                &self.db,
                "INSERT OR IGNORE INTO entities VALUES(?1,?2,?3,?4,?5,?6)",
                params![n, d.namespace, e.column, e.value, e.label, d.evidence.evidence_level],
            )
            .map_err(err)?;
        }
        Ok(())
    }
    pub fn entity_stat(
        &self,
        namespace: &str,
        column: &str,
        value: &str,
        events: usize,
        failures: usize,
    ) -> Result<(), String> {
        execute(
            &self.db,
            "INSERT OR REPLACE INTO entity_stats VALUES(?1,?2,?3,?4,?5)",
            params![namespace, column, value, events as i64, failures as i64],
        )
        .map_err(err)?;
        Ok(())
    }
    pub fn finish(self, triage: Triage) -> Result<Arc<Results>, String> {
        let mut previous = -1;
        loop {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let n: Option<i64> =
                row(&self.db, "SELECT n FROM findings WHERE n>?1 ORDER BY n LIMIT 1", [previous], |r| r.get(0))
                    .optional()
                    .map_err(err)?;
            let Some(n) = n else { break };
            previous = n;
            let (root, _) = self.root(n)?;
            execute(&self.db, "UPDATE findings SET root=?1,display_root=?1 WHERE n=?2", params![root, n]).map_err(err)?;
        }
        let mut previous = -1;
        loop {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let root: Option<i64> =
                row(&self.db, "SELECT root FROM findings WHERE root>?1 ORDER BY root LIMIT 1", [previous], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(err)?;
            let Some(root) = root else { break };
            previous = root;
            let (level, impact, start, end, count): (i64, i64, Option<i64>, Option<i64>, usize) = row(
                &self.db,
                "SELECT max(level),max(impact),min(start),max(end),count(*) FROM findings WHERE root=?1",
                [root],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .map_err(err)?;
            let lead: String = row(
                &self.db,
                "SELECT payload FROM findings WHERE root=?1 ORDER BY level DESC,impact DESC,id LIMIT 1",
                [root],
                |r| r.get(0),
            )
            .map_err(err)?;
            let lead: Value = serde_json::from_str(&lead).map_err(err)?;
            let mut hash = Sha256::new();
            if level == 0 {
                let finding = lead["id"].as_str().unwrap_or_default();
                hash.update((finding.len() as u64).to_le_bytes());
                hash.update(finding.as_bytes());
            }
            let mut records = 0;
            let mut statement = self
                .db
                .prepare("SELECT DISTINCT m.ref FROM members m JOIN findings f USING(n) WHERE f.root=?1 ORDER BY m.ref")
                .map_err(err)?;
            for reference in statement.query_map([root], |r| r.get::<_, String>(0)).map_err(err)? {
                let reference = reference.map_err(err)?;
                hash.update((reference.len() as u64).to_le_bytes());
                hash.update(reference.as_bytes());
                records += 1;
            }
            let id = format!("{}-{:x}", if level == 0 { "episode-unassessed" } else { "episode" }, hash.finalize());
            let mut statement = self
                .db
                .prepare("SELECT DISTINCT t FROM tactics JOIN findings USING(n) WHERE root=?1 ORDER BY t")
                .map_err(err)?;
            let tactics: Vec<String> =
                statement.query_map([root], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
            let payload = json!({"id":id,"title":lead["name"],"summary":lead["summary"],"severity":lead["severity"],"score":level*20,"evidence_level":level,"start":start,"end":end,"detections":[],"event_refs":[],"record_count":records,"detection_count":count,"tactics":tactics,"entities":lead["entities"].as_array().map(|v|v.iter().take(6).cloned().collect::<Vec<_>>()).unwrap_or_default()});
            let pattern = if records == 1 {
                let mut stmt = self.db.prepare("SELECT pattern FROM findings WHERE root=?1 ORDER BY pattern").map_err(err)?;
                let keys = stmt.query_map([root], |r| r.get::<_, Option<String>>(0)).map_err(err)?
                    .collect::<Result<Vec<_>, _>>().map_err(err)?;
                crate::security_grouping::episode_pattern(keys)
            } else { None };
            execute(
                &self.db,
                "INSERT INTO episodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![root, id, level, impact, start, end, count, payload.to_string(), pattern],
            )
            .map_err(err)?;
        }
        self.group_patterns()?;
        self.add_participants()?;
        self.db.execute_batch("COMMIT; CREATE INDEX card_order ON cards(level DESC,impact DESC,id); CREATE INDEX card_id ON cards(id); CREATE INDEX episode_id ON episodes(id); CREATE TEMP TABLE selected(event INTEGER PRIMARY KEY);").map_err(err)?;
        let mut metadata = serde_json::to_value(triage).map_err(err)?;
        let mut all_levels = [0usize; 5];
        let mut levels = self
            .db
            .prepare("SELECT level,count(*) FROM findings WHERE level>0 GROUP BY level")
            .map_err(err)?;
        for result in levels
            .query_map([], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, usize>(1)?)))
            .map_err(err)?
        {
            let (level, count) = result.map_err(err)?;
            all_levels[level - 1] = count;
        }
        drop(levels);
        metadata["universe_counts_by_level"] = json!(all_levels);
        metadata["storage"] = json!({"kind":"sqlite","complete":true,"working_memory_budget_mib":256});
        metadata["memory"] = crate::security_budget::snapshot();
        Ok(Arc::new(Results { db: Mutex::new(self.db), metadata }))
    }

    fn add_participants(&self) -> Result<(), String> {
        let mut stmt=self.db.prepare("SELECT root,payload FROM cards ORDER BY root").map_err(err)?;
        let mut cards=stmt.query([]).map_err(err)?;
        while let Some(card)=cards.next().map_err(err)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let root:i64=card.get(0).map_err(err)?;
            let payload:String=card.get(1).map_err(err)?;
            let mut value:Value=serde_json::from_str(&payload).map_err(err)?;
            let mut profile=crate::security_participants::Participants::default();
            let mut members=self.db.prepare("SELECT participants FROM findings WHERE display_root=?1 ORDER BY id").map_err(err)?;
            for p in members.query_map([root],|r|r.get::<_,String>(0)).map_err(err)? {
                crate::operations::check()?;
                crate::security_budget::check()?;
                profile.merge(&serde_json::from_str(&p.map_err(err)?).map_err(err)?);
            }
            value["participants"]=serde_json::to_value(profile).map_err(err)?;
            execute(&self.db,"UPDATE cards SET payload=?1 WHERE root=?2",params![value.to_string(),root]).map_err(err)?;
        }
        Ok(())
    }
    fn group_patterns(&self) -> Result<(), String> {
        self.db.execute_batch("INSERT INTO cards SELECT root,id,level,impact,start,end,count,payload FROM episodes;").map_err(err)?;
        // Work in SQLite before paging. Original episodes and findings remain intact.
        let mut stmt = self.db.prepare("SELECT pattern,min(root),count(*),sum(count),min(start),max(end) FROM episodes WHERE pattern IS NOT NULL GROUP BY pattern HAVING count(*)>1 ORDER BY pattern").map_err(err)?;
        let mut groups = stmt.query([]).map_err(err)?;
        while let Some(group) = groups.next().map_err(err)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let key: String = group.get(0).map_err(err)?;
            let root: i64 = group.get(1).map_err(err)?;
            let occurrences: usize = group.get(2).map_err(err)?;
            let count: usize = group.get(3).map_err(err)?;
            let start: Option<i64> = group.get(4).map_err(err)?;
            let end: Option<i64> = group.get(5).map_err(err)?;
            let lead: String = row(&self.db, "SELECT payload FROM episodes WHERE pattern=?1 ORDER BY id LIMIT 1", [&key], |r| r.get(0)).map_err(err)?;
            let mut payload: Value = serde_json::from_str(&lead).map_err(err)?;
            payload["id"] = json!(key);
            payload["start"] = json!(start);
            payload["end"] = json!(end);
            payload["record_count"] = json!(occurrences);
            payload["detection_count"] = json!(count);
            payload["grouping"] = json!({"kind":"pattern","occurrence_count":occurrences,"version":crate::security_grouping::VERSION});
            execute(&self.db, "DELETE FROM cards WHERE root IN(SELECT root FROM episodes WHERE pattern=?1)", [&key]).map_err(err)?;
            execute(&self.db, "INSERT INTO cards VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![root, key, payload["evidence_level"].as_i64(),
                    row(&self.db, "SELECT impact FROM episodes WHERE root=?1", [root], |r| r.get::<_, i64>(0)).map_err(err)?,
                    start, end, count, payload.to_string()]).map_err(err)?;
            execute(&self.db, "UPDATE findings SET display_root=?1 WHERE root IN(SELECT root FROM episodes WHERE pattern=?2)", params![root, key]).map_err(err)?;
        }
        Ok(())
    }
}

impl Results {
    pub fn contains_member(&self, reference: &str, event: usize) -> Result<bool, String> {
        row(
            &self.db.lock(),
            "SELECT EXISTS(SELECT 1 FROM members WHERE event=?1 AND ref=?2)",
            params![event as i64, reference],
            |r| r.get(0),
        )
        .map_err(err)
    }
    /// Compact timeline over the complete result, independent of episode pages.
    pub fn timeline(
        &self,
        minimum: u8,
        start: i64,
        end: i64,
        related: Option<&mut dyn Iterator<Item = usize>>,
    ) -> Result<Value, String> {
        let _working = crate::security_store::working_lane();
        if !(1..=5).contains(&minimum) || end <= start || end.checked_sub(start).is_none() {
            return Err("Nível ou intervalo inválido".into());
        }
        let db = self.db.lock();
        execute(&db, "DELETE FROM selected", []).map_err(err)?;
        let filtered = related.is_some();
        if let Some(ids) = related {
            for id in ids {
                crate::operations::check()?;
                crate::security_budget::check()?;
                execute(&db, "INSERT OR IGNORE INTO selected VALUES(?1)", [id as i64]).map_err(err)?;
            }
        }
        let mut stmt=db.prepare("SELECT CASE WHEN f.start<=?1 THEN 0 ELSE min(200,CAST((CAST(f.start AS REAL)-?1)*200/(?2-?1) AS INTEGER)) END AS slot, f.level,count(*),min(f.start),max(f.end) FROM findings f WHERE f.level>=?3 AND f.start<=?2 AND f.end>=?1 AND (?4=0 OR EXISTS(SELECT 1 FROM members m JOIN selected s ON s.event=m.event WHERE m.n=f.n)) GROUP BY slot,f.level ORDER BY slot,f.level DESC").map_err(err)?;
        let bins=stmt.query_map(params![start,end,minimum,filtered],|r|Ok(json!({"slot":r.get::<_,usize>(0)?,"evidence_level":r.get::<_,u8>(1)?,"count":r.get::<_,usize>(2)?,"start":r.get::<_,i64>(3)?,"end":r.get::<_,i64>(4)?}))).map_err(err)?.collect::<Result<Vec<_>,_>>().map_err(err)?;
        Ok(
            json!({"analysis_id":self.metadata["analysis_id"],"minimum_evidence":minimum,"start":start,"end":end,"complete":true,"bins":bins,"count":bins.iter().map(|b|b["count"].as_u64().unwrap_or(0)).sum::<u64>()}),
        )
    }
    pub fn page(
        &self,
        minimum: u8,
        offset: usize,
        limit: usize,
        related: Option<&mut dyn Iterator<Item = usize>>,
        tactic: Option<&str>,
    ) -> Result<Value, String> {
        let _working = crate::security_store::working_lane();
        if !(1..=5).contains(&minimum) || !(1..=500).contains(&limit) {
            return Err("Nível/página inválidos".into());
        }
        let db = self.db.lock();
        execute(&db, "DELETE FROM selected", []).map_err(err)?;
        let filtered = related.is_some();
        if let Some(ids) = related {
            for id in ids {
                crate::operations::check()?;
                crate::security_budget::check()?;
                execute(&db, "INSERT OR IGNORE INTO selected VALUES(?1)", [id as i64]).map_err(err)?;
            }
        }
        let condition="(?1=0 OR EXISTS(SELECT 1 FROM members m JOIN selected s ON s.event=m.event WHERE m.n=f.n)) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM tactics t WHERE t.n=f.n AND t.t=?2))";
        let mut counts = [0usize; 5];
        let mut stmt = db
            .prepare(&format!(
                "SELECT f.level,count(*) FROM findings f WHERE {condition} AND f.level>0 GROUP BY f.level"
            ))
            .map_err(err)?;
        for row in stmt
            .query_map(params![filtered, tactic], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, usize>(1)?)))
            .map_err(err)?
        {
            let (l, n) = row.map_err(err)?;
            counts[l - 1] = n;
        }
        let available: usize = counts.iter().sum();
        let visible: usize = counts[(minimum - 1) as usize..].iter().sum();
        let eligible = format!("EXISTS(SELECT 1 FROM findings f WHERE f.display_root=e.root AND f.level>=?3 AND {condition})");
        let total: usize = row(
            &db,
            &format!("SELECT count(*) FROM cards e WHERE {eligible}"),
            params![filtered, tactic, minimum],
            |r| r.get(0),
        )
        .map_err(err)?;
        let mut stmt=db.prepare(&format!("SELECT root,payload FROM cards e WHERE {eligible} ORDER BY level DESC,impact DESC,id LIMIT ?4 OFFSET ?5")).map_err(err)?;
        let mut detections = Vec::new();
        let mut episodes = Vec::new();
        let mut bytes = 0usize;
        for row in stmt
            .query_map(params![filtered, tactic, minimum, limit as i64, offset as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?
        {
            let (root, payload) = row.map_err(err)?;
            let mut episode: Value = serde_json::from_str(&payload).map_err(err)?;
            if bytes > 8 * 1024 * 1024 && !episodes.is_empty() {
                break;
            }
            // Bound transport even for an episode containing millions of findings.
            let mut members=db.prepare(&format!("SELECT f.payload, (f.level>=?3 AND {condition}) AS visible, e.id FROM findings f JOIN episodes e ON e.root=f.root WHERE f.display_root=?4 ORDER BY visible DESC,f.level DESC,f.impact DESC,f.start,f.id LIMIT 100")).map_err(err)?;
            let mut indices = Vec::new();
            let mut refs = std::collections::BTreeSet::new();
            let mut member_bytes = 0usize;
            for row in members
                .query_map(params![filtered, tactic, minimum, root], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?))
                })
                .map_err(err)?
            {
                let (payload, visible, source_episode) = row.map_err(err)?;
                if member_bytes.saturating_add(payload.len()) > 8 * 1024 * 1024 && !indices.is_empty() {
                    break;
                }
                if payload.len() > 16 * 1024 * 1024 {
                    return Err(
                        "Um achado individual excede 16 MiB de transporte; dados completos continuam em disco".into()
                    );
                }
                crate::operations::check()?;
                crate::security_budget::check()?;
                member_bytes += payload.len();
                bytes += payload.len();
                let mut d: Value = serde_json::from_str(&payload).map_err(err)?;
                d["context_only"] = json!(!visible);
                d["source_episode_id"] = json!(source_episode);
                for r in d["event_refs"].as_array().into_iter().flatten() {
                    if let Some(r) = r.as_str() {
                        refs.insert(r.to_string());
                    }
                }
                indices.push(detections.len());
                detections.push(d);
            }
            episode["members_complete"] =
                json!(episode["detection_count"].as_u64().unwrap_or(0) <= indices.len() as u64);
            episode["detections"] = json!(indices);
            episode["event_refs"] = json!(refs);
            episodes.push(episode);
        }
        let mut result = self.metadata.clone();
        result["detections"] = json!(detections);
        result["episodes"] = json!(episodes);
        if let Some(tactics) = result["tactics"].as_array_mut() {
            for t in tactics {
                let key = t["key"].as_str().unwrap_or_default();
                let mut levels = [0usize; 5];
                let mut stmt=db.prepare("SELECT f.level,count(*) FROM tactics t JOIN findings f USING(n) WHERE t.t=?1 AND f.level>0 AND (?2=0 OR EXISTS(SELECT 1 FROM members m JOIN selected s ON s.event=m.event WHERE m.n=f.n)) GROUP BY f.level").map_err(err)?;
                for row in stmt
                    .query_map(params![key, filtered], |r| Ok((r.get::<_, usize>(0)?, r.get::<_, usize>(1)?)))
                    .map_err(err)?
                {
                    let (l, n) = row.map_err(err)?;
                    levels[l - 1] = n;
                }
                t["counts_by_level"] = json!(levels);
                t["count"] = json!(levels[(minimum - 1) as usize..].iter().sum::<usize>());
                let mut stmt=db.prepare("SELECT a.id,a.name,count(*) FROM techniques a JOIN findings f USING(n) WHERE a.t=?1 AND f.level>=?2 AND (?3=0 OR EXISTS(SELECT 1 FROM members m JOIN selected s ON s.event=m.event WHERE m.n=f.n)) GROUP BY a.id ORDER BY count(*) DESC,a.id").map_err(err)?;
                t["techniques"] = json!(stmt
                    .query_map(params![t["key"].as_str(), minimum, filtered], |r| Ok(
                        json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"count":r.get::<_,usize>(2)?})
                    ))
                    .map_err(err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(err)?);
            }
        }
        let mut stmt=db.prepare(&format!("SELECT e.namespace,e.col,e.value,e.label,max(f.level),count(DISTINCT f.n),min(f.start),max(f.end) FROM entities e JOIN findings f USING(n) WHERE f.level>=?3 AND {condition} GROUP BY e.namespace,e.col,e.value ORDER BY max(f.level) DESC,e.namespace,e.col,e.value LIMIT 100")).map_err(err)?;
        let mut entities=stmt.query_map(params![filtered,tactic,minimum],|r|Ok(json!({"namespace":r.get::<_,String>(0)?,"column":r.get::<_,String>(1)?,"value":r.get::<_,String>(2)?,"label":r.get::<_,String>(3)?,"evidence_level":r.get::<_,u8>(4)?,"score":r.get::<_,u8>(4)? as u32*20,"detections":r.get::<_,usize>(5)?,"events":Value::Null,"failures":Value::Null,"first":r.get::<_,Option<i64>>(6)?,"last":r.get::<_,Option<i64>>(7)?,"tactics":[]}))).map_err(err)?.collect::<Result<Vec<_>,_>>().map_err(err)?;
        for entity in &mut entities {
            let value = entity["value"].as_str().unwrap_or("");
            let column = entity["column"].as_str().unwrap_or("");
            let scope = if ["@src_ip", "@dst_ip"].contains(&column) {
                crate::entities::parse_ip(value).map(crate::entities::ip_scope)
            } else {
                None
            };
            let stats: Option<(usize, usize)> = row(
                &db,
                "SELECT events,failures FROM entity_stats WHERE namespace=?1 AND col=?2 AND value=?3",
                params![entity["namespace"].as_str(), column, value],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(err)?;
            entity["scope"] = json!(scope);
            if let Some((events, failures)) = stats {
                entity["events"] = json!(events);
                entity["failures"] = json!(failures);
            }
            entity["level"] = json!(if entity["evidence_level"].as_u64().unwrap_or(0) >= 4 {
                "alto"
            } else if entity["evidence_level"].as_u64().unwrap_or(0) >= 2 {
                "médio"
            } else {
                "baixo"
            });
        }
        result["entities"] = json!(entities);
        result["counts_by_level"] = json!(counts);
        result["minimum_evidence"] = json!(minimum);
        result["available_detections"] = json!(available);
        result["visible_detections"] = json!(visible);
        result["analysis_scope"] = json!("full_universe");
        result["display_filtered"] = json!(filtered);
        result["page"] = json!({"episode_offset":offset,"episode_limit":limit,"total_episodes":total,"returned_episodes":episodes.len(),"next_offset":if offset.saturating_add(episodes.len())<total {Some(offset+episodes.len())} else{None}});
        result["returned_detections"] = json!(detections.iter().filter(|d| d["context_only"] != true).count());
        result["entities_page_limit"] = json!(100);
        Ok(result)
    }
    pub fn episode_members(&self, id: &str, offset: usize, limit: usize) -> Result<Value, String> {
        let _working = crate::security_store::working_lane();
        if !(1..=500).contains(&limit) {
            return Err("Limite de membros inválido".into());
        }
        let db = self.db.lock();
        let (root, total, grouped): (i64, usize, bool) =
            row(&db, "SELECT root,count,1 FROM cards WHERE id=?1 UNION ALL SELECT root,count,0 FROM episodes WHERE id=?1 LIMIT 1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(err)?;
        let mut stmt = db
            .prepare(&format!("SELECT f.payload,e.id FROM findings f JOIN episodes e ON e.root=f.root WHERE f.{}=?1 ORDER BY f.level DESC,f.impact DESC,f.start,f.id LIMIT ?2 OFFSET ?3", if grouped { "display_root" } else { "root" }))
            .map_err(err)?;
        let mut members = Vec::new();
        let mut bytes = 0;
        for payload in
            stmt.query_map(params![root, limit as i64, offset as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(err)?
        {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let (payload, source_episode) = payload.map_err(err)?;
            if bytes + payload.len() > 8 * 1024 * 1024 && !members.is_empty() {
                break;
            }
            if payload.len() > 16 * 1024 * 1024 {
                return Err("Achado individual excede o limite de transporte".into());
            }
            bytes += payload.len();
            let mut d: Value = serde_json::from_str(&payload).map_err(err)?;
            d["source_episode_id"] = json!(source_episode);
            members.push(d);
        }
        Ok(
            json!({"analysis_id":self.metadata["analysis_id"],"episode_id":id,"total":total,"detections":members,"next_offset":if offset.saturating_add(members.len())<total {Some(offset+members.len())}else{None}}),
        )
    }
    #[cfg(test)]
    pub fn related(&self, reference: &str) -> Result<Vec<Value>, String> {
        Ok(self.related_page(reference)?.0)
    }
    pub fn related_page(&self, reference: &str) -> Result<(Vec<Value>, usize), String> {
        let _working = crate::security_store::working_lane();
        let db = self.db.lock();
        let mut stmt=db.prepare("SELECT DISTINCT f.payload FROM findings f JOIN members m USING(n) WHERE m.ref=?1 ORDER BY f.level DESC,f.id LIMIT 500").map_err(err)?;
        let total =
            row(&db, "SELECT count(DISTINCT n) FROM members WHERE ref=?1", [reference], |r| r.get(0)).map_err(err)?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for payload in stmt.query_map([reference], |r| r.get::<_, String>(0)).map_err(err)? {
            crate::operations::check()?;
            crate::security_budget::check()?;
            let payload = payload.map_err(err)?;
            if !result.is_empty() && bytes + payload.len() > 8 * 1024 * 1024 {
                break;
            }
            if payload.len() > 16 * 1024 * 1024 {
                return Err("Achado individual excede o limite de transporte".into());
            }
            bytes += payload.len();
            result.push(serde_json::from_str(&payload).map_err(err)?);
        }
        Ok((result, total))
    }
}
