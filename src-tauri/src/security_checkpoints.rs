//! Atomic per-group result checkpoints. Membership is encoded as positions in
//! a canonically sorted, content-hashed hit group, then remapped to current IDs.
//! Late arrivals change the group hash and force complete reconciliation.
use crate::security_store::Spool;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
pub const VERSION: &str = "correlation-checkpoints-1";
pub struct Cache {
    db: crate::security_budget::TrackedConnection,
    pending: bool,
    cacheable: bool,
    pub reused: usize,
    pub computed: usize,
    pub skipped: usize,
}
fn error(e: impl std::fmt::Display) -> String {
    format!("Checkpoint de correlação: {e}")
}
impl Cache {
    pub fn new() -> Result<Self, String> {
        #[cfg(test)]
        let path = std::path::PathBuf::from("");
        #[cfg(not(test))]
        let path = {
            let dir = crate::config_dir().join(VERSION);
            std::fs::create_dir_all(&dir).map_err(error)?;
            dir.join(format!(
                "{}.sqlite",
                crate::evidence::stable_id(VERSION, [crate::analysis_runtime::fact_namespace()])
            ))
        };
        Self::open(&path)
    }
    pub(crate) fn open(path: &std::path::Path) -> Result<Self, String> {
        let db = crate::security_budget::TrackedConnection::open(path).map_err(error)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        db.execute_batch("PRAGMA cache_size=-2048;PRAGMA temp_store=FILE;CREATE TABLE IF NOT EXISTS groups(k TEXT PRIMARY KEY,complete INTEGER,created INTEGER);CREATE TABLE IF NOT EXISTS raws(g TEXT,n INTEGER,payload TEXT,PRIMARY KEY(g,n));").map_err(error)?;
        Ok(Self {
            db,
            pending: false,
            cacheable: true,
            reused: 0,
            computed: 0,
            skipped: 0,
        })
    }
    pub fn load(&mut self, key: &str) -> Result<Option<Spool<Value>>, String> {
        let exists: bool = self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM groups WHERE k=?1 AND complete=1)",
                [key],
                |r| r.get(0),
            )
            .map_err(error)?;
        if !exists {
            return Ok(None);
        }
        let mut result = Spool::new()?;
        let mut stmt = self
            .db
            .prepare("SELECT payload FROM raws WHERE g=?1 ORDER BY n")
            .map_err(error)?;
        let mut rows = stmt.query([key]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            let payload: String = row.get(0).map_err(error)?;
            result.push(
                crate::case_evidence::parse_exact_value(&payload)?
                    .into_parts()
                    .0,
            )?;
        }
        self.reused += 1;
        Ok(Some(result))
    }
    pub fn begin(&mut self, key: &str) -> Result<(), String> {
        // These are disposable accelerators; all evidence remains in Results.
        let bytes:u64=self.db.query_row("SELECT (page_count-freelist_count)*page_size FROM pragma_page_count(),pragma_freelist_count(),pragma_page_size()",[],|r|r.get(0)).map_err(error)?;
        if bytes > 256 * 1024 * 1024 {
            self.db
                .execute_batch("BEGIN;DELETE FROM raws;DELETE FROM groups;COMMIT;")
                .map_err(error)?;
        }
        self.db.execute_batch("BEGIN").map_err(error)?;
        self.pending = true;
        self.cacheable = true;
        self.db
            .execute(
                "INSERT OR REPLACE INTO groups VALUES(?1,0,?2)",
                params![key, chrono::Utc::now().timestamp_millis()],
            )
            .map_err(error)?;
        self.db
            .execute("DELETE FROM raws WHERE g=?1", [key])
            .map_err(error)?;
        self.computed += 1;
        Ok(())
    }
    pub fn push(
        &mut self,
        key: &str,
        raw: &Value,
        ranges: Option<Vec<(usize, usize)>>,
    ) -> Result<(), String> {
        if !self.cacheable {
            return Ok(());
        }
        let Some(ranges) = ranges else {
            self.cacheable = false;
            return Ok(());
        };
        let mut payload = raw.clone();
        payload["external"] = Value::Null;
        payload["members"] = json!([]);
        payload["checkpoint_ranges"] = json!(ranges);
        let payload = payload.to_string();
        if payload.len() > 1024 * 1024 {
            self.cacheable = false;
            return Ok(());
        }
        let n: i64 = self
            .db
            .query_row(
                "SELECT coalesce(max(n),-1)+1 FROM raws WHERE g=?1",
                [key],
                |r| r.get(0),
            )
            .map_err(error)?;
        self.db
            .execute(
                "INSERT INTO raws VALUES(?1,?2,?3)",
                params![key, n, payload],
            )
            .map_err(error)?;
        Ok(())
    }
    pub fn finish(&mut self, key: &str) -> Result<(), String> {
        crate::operations::check()?;
        if self.cacheable {
            self.db
                .execute("UPDATE groups SET complete=1 WHERE k=?1", [key])
                .map_err(error)?;
            self.db.execute_batch("COMMIT").map_err(error)?;
        } else {
            self.db.execute_batch("ROLLBACK").map_err(error)?;
            self.skipped += 1;
        }
        self.pending = false;
        Ok(())
    }
    pub fn describe(&self) -> Value {
        json!({"version":VERSION,"reused_groups":self.reused,"reconciled_groups":self.computed,"uncached_large_outputs":self.skipped,"membership":"exact sorted-hit ranges with current-ID remapping","late_events":"invalidate every changed group; temporal reconciliation includes bucket overlap","resume":"only committed complete groups; interrupted transactions roll back","scope":"native correlations; statistical population signals reconciled exactly"})
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        if self.pending {
            let _ = self.db.execute_batch("ROLLBACK");
        }
    }
}
pub fn publish_revision(db: &Connection, metadata: &Value) -> Result<Value, String> {
    #[cfg(test)]
    let temporary = tempfile::NamedTempFile::new().map_err(error)?;
    #[cfg(test)]
    let path = temporary.path().to_path_buf();
    #[cfg(not(test))]
    let path = {
        let directory = crate::config_dir().join(VERSION);
        std::fs::create_dir_all(&directory).map_err(error)?;
        directory.join(format!(
            "{}.sqlite",
            crate::evidence::stable_id(VERSION, [crate::analysis_runtime::fact_namespace()])
        ))
    };
    publish_at(db, metadata, &path)
}
fn publish_at(db: &Connection, metadata: &Value, path: &std::path::Path) -> Result<Value, String> {
    let _disk = crate::security_budget::DiskLease::register(path, false);
    crate::analysis_runtime::validate_result_owner()?;
    if metadata["complete"] != true {
        return Err("Revisão incompleta não pode substituir a análise anterior".into());
    }
    db.execute(
        "ATTACH DATABASE ?1 AS checkpoint_journal",
        [path.to_string_lossy().as_ref()],
    )
    .map_err(error)?;
    let result = (|| -> Result<Value, String> {
        db.execute_batch("CREATE TABLE IF NOT EXISTS checkpoint_journal.latest(id TEXT PRIMARY KEY,ns TEXT,classification TEXT);
          CREATE TABLE IF NOT EXISTS checkpoint_journal.publication(k TEXT PRIMARY KEY,payload TEXT);
          BEGIN IMMEDIATE;CREATE TEMP TABLE current_classification(id TEXT PRIMARY KEY,ns TEXT,classification TEXT);
          INSERT INTO current_classification SELECT id,namespace,json_object('rule',json_extract(payload,'$.rule'),'evidence_level',level,'claim',json_extract(payload,'$.claim'),'outcome',json_extract(payload,'$.outcome'),'reasons',json_extract(payload,'$.evidence_reasons'),'missing',json_extract(payload,'$.missing_evidence'),'rule_version',json_extract(payload,'$.rule_version'),'policy_version',json_extract(payload,'$.policy_version'),'normalization_version',json_extract(payload,'$.normalization_version')) FROM findings;
          INSERT INTO revision_changes SELECT 'added',c.id,c.ns,c.classification FROM current_classification c WHERE NOT EXISTS(SELECT 1 FROM checkpoint_journal.latest p WHERE p.id=c.id);
          INSERT INTO revision_changes SELECT 'retracted',p.id,p.ns,p.classification FROM checkpoint_journal.latest p WHERE NOT EXISTS(SELECT 1 FROM current_classification c WHERE c.id=p.id);
          INSERT INTO revision_changes SELECT 'revised_classification',c.id,c.ns,c.classification FROM current_classification c JOIN checkpoint_journal.latest p ON c.id=p.id WHERE c.classification<>p.classification;").map_err(error)?;
        let previous: Option<String> = db
            .query_row(
                "SELECT payload FROM checkpoint_journal.publication WHERE k='current'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        let previous: Value = previous
            .map(|s| serde_json::from_str(&s).map_err(error))
            .transpose()?
            .unwrap_or(Value::Null);
        let count = |kind: &str| {
            db.query_row(
                "SELECT count(*) FROM revision_changes WHERE kind=?1",
                [kind],
                |r| r.get::<_, i64>(0),
            )
            .map_err(error)
        };
        let revision = json!({"version":VERSION,"status":"complete","number":previous["number"].as_u64().unwrap_or(0)+1,"previous_analysis_id":previous["analysis_id"],"analysis_id":metadata["analysis_id"],"added":count("added")?,"retracted":count("retracted")?,"revised_classifications":count("revised_classification")?,"change_page":"revision","meaning":"Exact finding identity and classification changes relative to the last complete calculation in this Case; a retraction is not proof that an activity was benign."});
        db.execute_batch("DELETE FROM checkpoint_journal.latest;INSERT INTO checkpoint_journal.latest SELECT id,ns,classification FROM current_classification;").map_err(error)?;
        db.execute(
            "INSERT OR REPLACE INTO checkpoint_journal.publication VALUES('current',?1)",
            [revision.to_string()],
        )
        .map_err(error)?;
        crate::analysis_runtime::validate_result_owner()?;
        crate::security_budget::check_now()?;
        db.execute_batch("DROP TABLE current_classification;COMMIT")
            .map_err(error)?;
        Ok(revision)
    })();
    if result.is_err() {
        let _ = db.execute_batch("ROLLBACK;DROP TABLE IF EXISTS current_classification;");
    }
    db.execute_batch("DETACH DATABASE checkpoint_journal")
        .map_err(error)?;
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_revisions_retract_removed_findings_revise_grades_and_never_publish_incomplete() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.sqlite");
        let db = Connection::open("").unwrap();
        db.execute_batch("CREATE TABLE findings(id TEXT,namespace TEXT,level INTEGER,payload TEXT);CREATE TABLE revision_changes(kind TEXT,id TEXT,ns TEXT,payload TEXT);").unwrap();
        db.execute(
            "INSERT INTO findings VALUES('a','tenant',1,'{\"rule\":\"r\",\"claim\":\"activity\"}')",
            [],
        )
        .unwrap();
        let first =
            publish_at(&db, &json!({"complete":true,"analysis_id":"first"}), &path).unwrap();
        assert_eq!(first["added"], 1);
        db.execute("DELETE FROM revision_changes", []).unwrap();
        db.execute("UPDATE findings SET level=3", []).unwrap();
        let next =
            publish_at(&db, &json!({"complete":true,"analysis_id":"second"}), &path).unwrap();
        assert_eq!(next["added"], 0);
        assert_eq!(next["revised_classifications"], 1);
        assert_eq!(next["previous_analysis_id"], "first");
        db.execute("DELETE FROM revision_changes", []).unwrap();
        db.execute("DELETE FROM findings", []).unwrap();
        assert!(publish_at(&db, &json!({"complete":false,"analysis_id":"bad"}), &path).is_err());
        let final_revision =
            publish_at(&db, &json!({"complete":true,"analysis_id":"third"}), &path).unwrap();
        assert_eq!(final_revision["retracted"], 1);
        assert_eq!(final_revision["number"], 3);
        assert_eq!(final_revision["previous_analysis_id"], "second");
    }
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_result_checkpoint_is_atomic_resumable_and_empty_results_are_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoint.sqlite");
        {
            let mut cache = Cache::open(&path).unwrap();
            cache.begin("complete").unwrap();
            cache
                .push("complete", &json!({"rule":0}), Some(vec![(0, 1000)]))
                .unwrap();
            cache.finish("complete").unwrap();
            cache.begin("empty").unwrap();
            cache.finish("empty").unwrap();
            cache.begin("interrupted").unwrap();
            cache
                .push("interrupted", &json!({"rule":0}), Some(vec![(2, 3)]))
                .unwrap();
        }
        let mut cache = Cache::open(&path).unwrap();
        assert!(cache.load("interrupted").unwrap().is_none());
        assert!(cache.load("changed-content").unwrap().is_none());
        assert_eq!(
            cache
                .load("empty")
                .unwrap()
                .unwrap()
                .into_iter()
                .unwrap()
                .count(),
            0
        );
        let raw = cache
            .load("complete")
            .unwrap()
            .unwrap()
            .into_iter()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(raw["checkpoint_ranges"], json!([[0, 1000]]));
    }
}
