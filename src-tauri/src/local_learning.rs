//! Learning is local, bounded and separate from evidence classification.
//! A frozen model scores the full population; only a validated candidate can
//! be committed after successful publication of the complete analysis.
use log_learning::{
    digest, Assessment, Contribution, Domain, Limits, Model, Observation, PublicState, SeenFilter,
    SharedModel, MAX_DOCUMENT_BYTES,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub const VERSION: &str = "local-learning-1";
const TRAINING_CAP: usize = 10_000;
const RECEIPT_CAP: u64 = 200_000;
const VALIDATION_CAP: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub auto_update: bool,
    pub limits: Limits,
    pub github_repository: Option<String>,
    pub github_branch: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_update: true,
            limits: Limits::default(),
            github_repository: None,
            github_branch: "learning-models".into(),
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()?;
        if let Some(repo) = &self.github_repository {
            let parts = repo.split('/').collect::<Vec<_>>();
            if parts.len() != 2
                || parts.iter().any(|p| {
                    p.is_empty()
                        || p.len() > 100
                        || !p
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                        || matches!(*p, "." | "..")
                })
            {
                return Err("Repositório deve ter formato proprietário/projeto".into());
            }
        }
        if self.github_branch.is_empty()
            || self.github_branch.len() > 100
            || !self
                .github_branch
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(
                "Branch do aprendizado deve usar letras, números, hífen ou sublinhado".into(),
            );
        }
        Ok(())
    }
}
fn error(e: impl std::fmt::Display) -> String {
    format!("Aprendizado local: {e}")
}
fn check() -> Result<(), String> {
    crate::operations::check()?;
    crate::security_budget::check()
}
fn model_key(context: &str) -> String {
    let values = serde_json::from_str::<Vec<String>>(context).unwrap_or_default();
    if values.len() == 4 && !values[2].is_empty() {
        let namespace = if values[0] == format!("source:{}", values[1]) {
            "case-service"
        } else {
            &values[0]
        };
        digest(
            json!([namespace, values[2], values[3]])
                .to_string()
                .as_bytes(),
        )
    } else {
        digest(context.as_bytes())
    }
}
fn sampled(receipt: &str, observations: u64, groups: usize) -> bool {
    let budget = (TRAINING_CAP / groups.max(1)).max(1) as u64;
    let stride = observations.div_ceil(budget).max(1);
    u64::from_str_radix(&receipt[..16], 16).unwrap_or(0) % stride == 0
}
pub fn event_context(event: &crate::model::Event) -> Value {
    let first = |fields: &[&str]| {
        fields.iter().find_map(|field| {
            event
                .col_str(field)
                .filter(|v| !v.trim().is_empty() && v.chars().count() <= 256)
        })
    };
    json!({"operation":first(&["http.route","operation.name","operation"]),"version":first(&["service.version","app.version","deployment.version","version"]),"dependency":first(&["dependency.name","destination.service.name","db.system","server.address"]),"trace":first(&["trace.id","trace_id","traceId","request.id","request_id","requestId","correlation_id"]),"failure":crate::insights::is_error(event)})
}
pub fn profile_path() -> PathBuf {
    let mut mappings =
        serde_json::to_value(crate::detections::load_settings().mappings).unwrap_or(Value::Null);
    mappings.sort_all_objects();
    let identity = json!([
        VERSION,
        crate::evidence::NORMALIZATION_VERSION,
        crate::analysis_runtime::fact_namespace(),
        crate::analysis_runtime::fact_interpretation_signature(),
        mappings
    ]);
    crate::config_dir().join("local-learning").join(format!(
        "{}.sqlite",
        digest(identity.to_string().as_bytes())
    ))
}
fn open_profile(path: &std::path::Path) -> Result<Connection, String> {
    std::fs::create_dir_all(path.parent().ok_or("Diretório do perfil ausente")?).map_err(error)?;
    let db = Connection::open(path).map_err(error)?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(error)?;
    db.execute_batch("PRAGMA journal_mode=DELETE;PRAGMA cache_size=-1024;PRAGMA mmap_size=0;PRAGMA max_page_count=32768;
        CREATE TABLE IF NOT EXISTS model(id INTEGER PRIMARY KEY CHECK(id=1),revision TEXT NOT NULL,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS receipts(event TEXT PRIMARY KEY,generation INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS revisions(generation INTEGER PRIMARY KEY,revision TEXT,payload TEXT);
        CREATE TABLE IF NOT EXISTS sentinels(id TEXT PRIMARY KEY,payload TEXT);
        CREATE TABLE IF NOT EXISTS compacted_receipts(id INTEGER PRIMARY KEY CHECK(id=1),bits BLOB NOT NULL);").map_err(error)?;
    Ok(db)
}
fn load_seen(db: &Connection) -> Result<SeenFilter, String> {
    match db
        .query_row("SELECT bits FROM compacted_receipts WHERE id=1", [], |r| {
            r.get::<_, Vec<u8>>(0)
        })
        .optional()
        .map_err(error)?
    {
        Some(bits) => SeenFilter::from_bytes(bits),
        None => Ok(SeenFilter::default()),
    }
}
fn compact_receipts(db: &Connection, generation: u64) -> Result<(), String> {
    let count: u64 = db
        .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
        .map_err(error)?;
    if count <= 100_000 {
        return Ok(());
    }
    let mut seen = load_seen(db)?;
    let mut statement = db
        .prepare(
            "SELECT event FROM receipts WHERE generation<?1 ORDER BY generation,event LIMIT 50000",
        )
        .map_err(error)?;
    let old = statement
        .query_map([generation.saturating_sub(3)], |r| r.get::<_, String>(0))
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    for receipt in &old {
        seen.insert(receipt)?;
        db.execute("DELETE FROM receipts WHERE event=?1", [receipt])
            .map_err(error)?;
    }
    db.execute(
        "INSERT OR REPLACE INTO compacted_receipts VALUES(1,?1)",
        [seen.bytes()],
    )
    .map_err(error)?;
    Ok(())
}
fn load_model(db: &Connection, limits: &Limits) -> Result<(Model, String), String> {
    let row = db
        .query_row("SELECT revision,payload FROM model WHERE id=1", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .optional()
        .map_err(error)?;
    if let Some((revision, text)) = row {
        if text.len() > MAX_DOCUMENT_BYTES || digest(text.as_bytes()) != revision {
            return Err("Perfil excedeu o orçamento ou divergiu de sua revisão".into());
        }
        let model: Model = serde_json::from_str(&text).map_err(error)?;
        model.validate()?;
        if &model.limits != limits {
            return Err("Os limites do perfil mudaram. Reinicie o aprendizado local antes de aplicar os novos limites.".into());
        }
        Ok((model, revision))
    } else {
        let model = Model {
            limits: limits.clone(),
            ..Default::default()
        };
        let revision = digest(serde_json::to_string(&model).map_err(error)?.as_bytes());
        Ok((model, revision))
    }
}
pub fn shared_model() -> Result<SharedModel, String> {
    let path = crate::config_dir().join("learning-shared.json");
    let text = if path.exists() {
        if std::fs::metadata(&path).map_err(error)?.len() > MAX_DOCUMENT_BYTES as u64 {
            return Err("Base compartilhada acima do orçamento".into());
        }
        std::fs::read_to_string(path).map_err(error)?
    } else {
        include_str!("../../models/shared-model.json").into()
    };
    let model: SharedModel = serde_json::from_str(&text).map_err(error)?;
    model.validate()?;
    Ok(model)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sentinel {
    key: String,
    receipt: String,
    message: String,
    failure: bool,
    latency_ms: Option<f64>,
    bytes: Option<f64>,
}
impl Sentinel {
    fn observation(&self) -> Observation<'_> {
        Observation {
            message: &self.message,
            failure: self.failure,
            latency_ms: self.latency_ms,
            bytes: self.bytes,
        }
    }
}
pub struct Pending {
    path: PathBuf,
    base: String,
    model: Model,
    receipts: Vec<String>,
    sentinels: Vec<Sentinel>,
    reviewed_receipts: Vec<String>,
}
impl Pending {
    /// CAS and one SQLite transaction couple counts, receipts and revision.
    /// Called after successful analysis publication, never on cancellation.
    pub fn commit(self) -> Result<Value, String> {
        let mut db = open_profile(&self.path)?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(error)?;
        let (_, current) = load_model(&tx, &self.model.limits)?;
        if current != self.base {
            return Ok(json!({"status":"concurrent_update_deferred","applied":false}));
        }
        let text = serde_json::to_string(&self.model).map_err(error)?;
        if text.len() > MAX_DOCUMENT_BYTES {
            return Err("Modelo excedeu 16 MiB; candidato não aplicado".into());
        }
        self.model.validate()?;
        let revision = digest(text.as_bytes());
        tx.execute("INSERT OR REPLACE INTO revisions SELECT json_extract(payload,'$.generation'),revision,payload FROM model WHERE id=1",[]).map_err(error)?;
        tx.execute("DELETE FROM revisions WHERE generation NOT IN(SELECT generation FROM revisions ORDER BY generation DESC LIMIT 3)",[]).map_err(error)?;
        tx.execute(
            "INSERT OR REPLACE INTO model VALUES(1,?1,?2)",
            params![revision, text],
        )
        .map_err(error)?;
        for key in &self.receipts {
            tx.execute(
                "INSERT OR IGNORE INTO receipts VALUES(?1,?2)",
                params![key, self.model.generation],
            )
            .map_err(error)?;
        }
        compact_receipts(&tx, self.model.generation)?;
        for receipt in &self.reviewed_receipts {
            tx.execute(
                "DELETE FROM sentinels WHERE json_extract(payload,'$.receipt')=?1",
                [receipt],
            )
            .map_err(error)?;
        }
        for sample in self.sentinels.iter().take(VALIDATION_CAP) {
            let text = serde_json::to_string(sample).map_err(error)?;
            tx.execute("INSERT OR IGNORE INTO sentinels SELECT ?1,?2 WHERE (SELECT count(*) FROM sentinels)<256",params![digest(text.as_bytes()),text]).map_err(error)?;
        }
        tx.commit().map_err(error)?;
        Ok(
            json!({"status":"accepted","applied":true,"revision":revision,"generation":self.model.generation,"new_observations":self.receipts.len()}),
        )
    }
}

struct Row {
    reference: String,
    id: i64,
    ts: i64,
    message: String,
    failure: bool,
    latency: Option<f64>,
    bytes: Option<f64>,
    stream: String,
    protected: bool,
    product: String,
    action: String,
    outcome: String,
}
impl Row {
    fn get(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            reference: r.get(0)?,
            id: r.get(1)?,
            ts: r.get(2)?,
            message: r.get(3)?,
            failure: r.get(4)?,
            latency: r.get(5)?,
            bytes: r.get(6)?,
            stream: r.get(7)?,
            protected: r.get(8)?,
            product: r.get(9)?,
            action: r.get(10)?,
            outcome: r.get(11)?,
        })
    }
    fn observation(&self) -> Observation<'_> {
        Observation {
            message: &self.message,
            failure: self.failure,
            latency_ms: self.latency,
            bytes: self.bytes,
        }
    }
    fn sentinel(&self, key: &str) -> Sentinel {
        Sentinel {
            key: key.into(),
            receipt: self.receipt(),
            message: self.message.clone(),
            failure: self.failure,
            latency_ms: self.latency,
            bytes: self.bytes,
        }
    }
    fn receipt(&self) -> String {
        digest(
            json!([
                self.reference,
                self.ts,
                self.message,
                self.failure,
                self.latency,
                self.bytes,
                self.stream,
                self.product,
                self.action,
                self.outcome
            ])
            .to_string()
            .as_bytes(),
        )
    }
}
const SELECT_ROWS:&str="SELECT ref,eid,ts,message,failure,latency,bytes,stream,protected,product,action,outcome FROM learning_input WHERE context=?1 AND valid ORDER BY stream,ts,ref";
#[derive(Default)]
struct Previous {
    stream: String,
    ts: Option<i64>,
    cluster: Option<u32>,
    public: Option<PublicState>,
    ambiguous: bool,
}
impl Previous {
    fn cluster(&self, row: &Row) -> Option<u32> {
        if !row.stream.is_empty()
            && !self.ambiguous
            && self.stream == row.stream
            && self
                .ts
                .is_some_and(|t| row.ts > t && row.ts.saturating_sub(t) <= 1_800_000)
        {
            self.cluster
        } else {
            None
        }
    }
    fn set(&mut self, row: &Row, cluster: Option<u32>, public: Option<PublicState>) {
        self.ambiguous = self.stream == row.stream && self.ts == Some(row.ts);
        self.stream = row.stream.clone();
        self.ts = Some(row.ts);
        self.cluster = cluster;
        self.public = public;
    }
}

pub fn analyze(db: &Connection, settings: &Settings) -> Result<(Value, Option<Pending>), String> {
    analyze_mode(db, settings, None)
}
pub fn learn_expected(db: &Connection, settings: &Settings, signal: &str) -> Result<Value, String> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM learning_incidents WHERE id=?1)",
            [signal],
            |r| r.get(0),
        )
        .map_err(error)?;
    if !exists {
        return Err("Diagnóstico aprendido não pertence a esta análise".into());
    }
    let mut settings = settings.clone();
    settings.auto_update = true;
    let (report, pending) = analyze_mode(db, &settings, Some(signal))?;
    if let Some(pending) = pending {
        crate::operations::check()?;
        crate::operations::commit();
        let mut result = pending.commit()?;
        result["review"] = json!("explicit_expected_behavior");
        result["classification_updated"] = json!(false);
        Ok(result)
    } else {
        Ok(
            json!({"status":"withheld","applied":false,"validation":report["validation"],"meaning":"A revisão exige observações elegíveis e controles independentes suficientes. Membros E3+ e sinais prioritários continuam excluídos."}),
        )
    }
}
fn analyze_mode(
    db: &Connection,
    settings: &Settings,
    expected: Option<&str>,
) -> Result<(Value, Option<Pending>), String> {
    if !settings.enabled {
        return Ok((json!({"enabled":false}), None));
    }
    settings.validate()?;
    check()?;
    let trusted = if let Some(signal) = expected {
        let mut statement = db
            .prepare(
                "SELECT ref FROM investigation_members WHERE id=?1 ORDER BY ts,ref LIMIT 10001",
            )
            .map_err(error)?;
        let refs = statement
            .query_map([signal], |r| r.get::<_, String>(0))
            .map_err(error)?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(error)?;
        if refs.len() > TRAINING_CAP {
            return Err("A revisão excede 10000 observações; use um recorte menor".into());
        }
        refs
    } else {
        BTreeSet::new()
    };
    db.execute_batch(
        "DROP TABLE IF EXISTS temp.learning_input;DROP TABLE IF EXISTS temp.learning_flags;",
    )
    .map_err(error)?;
    crate::operations::report_progress(
        "comprometimentos",
        "learning",
        "Aprendendo e comparando comportamentos locais",
        0,
        0,
        "registros",
        0,
    );
    db.execute_batch("CREATE TEMP TABLE learning_input AS
        SELECT f.ref,f.eid,f.ns,f.source,f.ts,
          substr(coalesce(json_extract(f.payload,'$.template'),''),1,2400) AS message,
          coalesce(json_extract(f.payload,'$.outcome'),'unknown') AS outcome,
          coalesce(json_extract(f.payload,'$.action'),'') AS action,
          coalesce(json_extract(f.payload,'$.outcome')='failure',0) OR coalesce(json_extract(f.payload,'$.metadata.learning.failure'),0) OR (json_extract(f.payload,'$.metadata.product')='web' AND CAST(json_extract(f.payload,'$.code') AS INTEGER) BETWEEN 500 AND 599) AS failure,
          json_extract(f.payload,'$.latency_ms') AS latency,json_extract(f.payload,'$.bytes') AS bytes,
          substr(coalesce(json_extract(f.payload,'$.metadata.product'),'unknown'),1,64) AS product,
          CASE WHEN length(json_extract(f.payload,'$.metadata.learning.trace')) BETWEEN 1 AND 256 THEN json_array('trace',json_extract(f.payload,'$.metadata.learning.trace'))
            WHEN length(f.session) BETWEEN 1 AND 256 AND length(f.host) BETWEEN 1 AND 256 THEN json_array('session',f.host,f.session) ELSE '' END AS stream,
          CASE WHEN length(f.host)<=256 THEN f.host ELSE '' END AS host,
          substr(coalesce(json_extract(f.payload,'$.metadata.learning.version'),''),1,256) AS version,
          substr(coalesce(json_extract(f.payload,'$.metadata.learning.dependency'),''),1,256) AS dependency,
          json_array(f.ns,f.source,substr(coalesce(json_extract(f.payload,'$.metadata.service'),''),1,256),substr(coalesce(json_extract(f.payload,'$.metadata.learning.operation'),json_extract(f.payload,'$.action'),''),1,256)) AS context,
          coalesce(json_extract(f.payload,'$.parse_ok'),0) AND NOT coalesce(json_extract(f.payload,'$.metadata.content_clipped'),0) AND f.ts IS NOT NULL AND json_extract(f.payload,'$.metadata.time.ambiguity') IS NULL AS valid,
          EXISTS(SELECT 1 FROM members m JOIN findings v USING(n) WHERE m.ref=f.ref AND v.level>=3)
            OR EXISTS(SELECT 1 FROM investigation_members m JOIN investigation_signals s ON m.id=s.id WHERE m.ref=f.ref AND s.priority>=35) AS protected
        FROM investigation_facts f;
        CREATE INDEX learning_input_context ON learning_input(context,stream,ts,ref);
        CREATE INDEX learning_input_time ON learning_input(context,ts);
        CREATE TEMP TABLE learning_flags(ref TEXT PRIMARY KEY,eid INTEGER,ts INTEGER,context TEXT,ns TEXT,source TEXT,kinds TEXT,surprise REAL);
        CREATE INDEX learning_flags_context ON learning_flags(context,ts);
        CREATE TABLE IF NOT EXISTS learning_exports(domain TEXT PRIMARY KEY,payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS learning_incidents(id TEXT PRIMARY KEY,kind TEXT,ns TEXT,col TEXT,entity TEXT,priority INTEGER,first INTEGER,last INTEGER,payload TEXT);
        CREATE INDEX IF NOT EXISTS learning_incident_page ON learning_incidents(ns,entity,priority DESC,id);").map_err(error)?;
    let (total, valid): (u64, u64) = db
        .query_row(
            "SELECT count(*),coalesce(sum(valid),0) FROM learning_input",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(error)?;
    let path = profile_path();
    let profile = open_profile(&path)?;
    let (mut reference, base) = load_model(&profile, &settings.limits)?;
    let seen = load_seen(&profile)?;
    let dedup_false_positive_rate = seen.estimated_false_positive_rate();
    let mut anchors = {
        let mut statement = profile
            .prepare("SELECT payload FROM sentinels ORDER BY id LIMIT 256")
            .map_err(error)?;
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(error)?;
        rows.map(|r| serde_json::from_str::<Sentinel>(&r.map_err(error)?).map_err(error))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut reviewed_receipts = BTreeSet::new();
    let anchor_receipts = anchors
        .iter()
        .map(|a| a.receipt.clone())
        .collect::<BTreeSet<_>>();
    let shared = shared_model()?;
    let receipts_count: u64 = profile
        .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
        .map_err(error)?;
    let can_train = settings.auto_update
        && valid >= settings.limits.minimum_support * 2
        && valid.saturating_mul(10) >= total.saturating_mul(9)
        && receipts_count < RECEIPT_CAP;
    let can_train = can_train && dedup_false_positive_rate < 0.02;
    let persisted_keys = reference
        .contexts
        .iter()
        .filter(|(_, c)| c.count >= settings.limits.minimum_support)
        .map(|(k, _)| k.clone())
        .collect::<BTreeSet<_>>();
    let mut groups=db.prepare("SELECT context,ns,source,min(ts),max(ts),count(*) FROM learning_input WHERE valid GROUP BY context ORDER BY count(*) DESC,context LIMIT ?1").map_err(error)?;
    let mut groups = groups
        .query_map([settings.limits.max_contexts as i64 + 1], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, u64>(5)?,
            ))
        })
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    let mut contexts_limited = groups.len() > settings.limits.max_contexts;
    let admitted_groups = groups.len().min(settings.limits.max_contexts);
    let mut bounds = BTreeMap::<String, (i64, i64)>::new();
    for (context, _, _, first, last, _) in groups.iter().take(admitted_groups) {
        let entry = bounds.entry(model_key(context)).or_insert((*first, *last));
        entry.0 = entry.0.min(*first);
        entry.1 = entry.1.max(*last);
    }
    for (context, _, _, first, last, _) in groups.iter_mut().take(admitted_groups) {
        if let Some((min, max)) = bounds.get(&model_key(context)) {
            *first = *min;
            *last = *max;
        }
    }
    let mut training_keys = BTreeSet::new();
    let mut holdout = Vec::new();
    let mut scored = 0u64;
    let mut cold_start = 0u64;
    let mut shared_support = 0u64;
    let mut bootstrap = 0usize;
    // Each context fits its initial reference strictly before the evaluated tail.
    for (context, _, _, first, last, observations) in
        groups.iter().take(settings.limits.max_contexts)
    {
        let key = model_key(context);
        if !reference.contexts.contains_key(&key)
            && reference.contexts.len() >= settings.limits.max_contexts
        {
            contexts_limited = true;
            continue;
        }
        if reference
            .contexts
            .get(&key)
            .is_some_and(|c| c.count >= settings.limits.minimum_support)
        {
            continue;
        }
        let cutoff = first.saturating_add(last.saturating_sub(*first).saturating_mul(3) / 5);
        let mut statement = db.prepare(SELECT_ROWS).map_err(error)?;
        let mut rows = statement.query([context]).map_err(error)?;
        let mut previous = Previous::default();
        while let Some(row) = rows.next().map_err(error)? {
            check()?;
            let row = Row::get(row).map_err(error)?;
            if row.ts > cutoff || row.protected || training_keys.len() >= TRAINING_CAP {
                previous = Previous::default();
                continue;
            }
            let receipt = row.receipt();
            if !sampled(&receipt, *observations, admitted_groups) {
                previous.set(
                    &row,
                    reference.score(&key, &row.observation(), None).cluster,
                    None,
                );
                continue;
            }
            if training_keys.contains(&receipt) {
                continue;
            }
            let already: bool = profile
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM receipts WHERE event=?1)",
                    [&receipt],
                    |r| r.get(0),
                )
                .map_err(error)?;
            if already || seen.contains(&receipt) || anchor_receipts.contains(&receipt) {
                previous = Previous::default();
                continue;
            }
            let id = reference.train(&key, &row.observation(), previous.cluster(&row));
            previous.set(&row, id, None);
            training_keys.insert(receipt);
            bootstrap += 1;
        }
    }
    let frozen = reference.clone();
    detect_changes(
        db,
        &frozen,
        &groups,
        &persisted_keys,
        settings.limits.max_contexts,
    )?;
    let mut candidate = reference;
    let mut contributions = BTreeMap::<Domain, Contribution>::new();
    // Bootstrap contributions must pass the same publication/validation gate.
    for (context, ns, source, first, last, observations) in
        groups.iter().take(settings.limits.max_contexts)
    {
        let key = model_key(context);
        let cutoff = first.saturating_add(last.saturating_sub(*first).saturating_mul(3) / 5);
        let training_end = first.saturating_add(last.saturating_sub(*first).saturating_mul(4) / 5);
        let persisted = persisted_keys.contains(&key);
        let mut statement = db.prepare(SELECT_ROWS).map_err(error)?;
        let mut rows = statement.query([context]).map_err(error)?;
        let mut previous = Previous::default();
        let mut trained_previous = Previous::default();
        let mut context_holdout = 0usize;
        while let Some(row) = rows.next().map_err(error)? {
            check()?;
            let row = Row::get(row).map_err(error)?;
            let obs = row.observation();
            let receipt = row.receipt();
            let reviewed = trusted.contains(&row.reference) && !row.protected;
            if reviewed {
                reviewed_receipts.insert(receipt.clone());
            }
            let public = PublicState::new(
                &row.action,
                if row.outcome == "unknown" && row.failure {
                    "failure"
                } else {
                    &row.outcome
                },
                row.latency,
                row.bytes,
                row.message.split_whitespace().count(),
            );
            let mut score = if persisted || row.ts > cutoff {
                frozen.score(&key, &obs, previous.cluster(&row))
            } else {
                Assessment::default()
            };
            let domain = Domain::from_product(&row.product);
            let (support, shared_total) = shared.support(domain, public);
            if !score.ready
                && row.ts > cutoff
                && shared_total >= 200
                && public.outcome > 0
                && (public.latency > 0 || public.bytes > 0)
                && support.saturating_mul(200) < shared_total
            {
                score.ready = true;
                score.deviations.push(log_learning::Deviation{kind:"shared_context_candidate".into(),surprise:-((support as f64+0.5)/(shared_total as f64+1.0)).log2(),support:shared_total,detail:"Combinação estrutural pouco observada na base compartilhada deste domínio. Candidato de baixa cobertura local; não define a normalidade deste ambiente.".into()});
            }
            previous.set(&row, score.cluster, None);
            let changed: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM learning_flags WHERE ref=?1)",
                    [&row.reference],
                    |r| r.get(0),
                )
                .map_err(error)?;
            if changed {
                score.ready = true;
                score.deviations.push(log_learning::Deviation{kind:"temporal_change".into(),surprise:4.0,support:frozen.contexts.get(&key).map_or(0,|c|c.count),detail:"Mudança temporal persistente ou aumento da taxa de falhas em relação à referência aprendida.".into()});
            }
            if score.ready {
                scored += 1;
            } else {
                cold_start += 1;
            }
            if shared_total > 0 && support > 0 {
                shared_support += 1;
            }
            if score.unusual() {
                let kinds = serde_json::to_string(&score.deviations).map_err(error)?;
                let surprise = score
                    .deviations
                    .iter()
                    .map(|d| d.surprise)
                    .fold(0.0f64, f64::max);
                db.execute(
                    "INSERT OR IGNORE INTO learning_flags VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        row.reference,
                        row.id,
                        row.ts,
                        context,
                        ns,
                        source,
                        kinds,
                        surprise
                    ],
                )
                .map_err(error)?;
            }
            if !reviewed
                && row.ts > training_end
                && !row.protected
                && holdout.len() < VALIDATION_CAP
                && context_holdout
                    < (VALIDATION_CAP / groups.len().min(settings.limits.max_contexts).max(1))
                        .max(1)
            {
                holdout.push(row.sentinel(&key));
                context_holdout += 1;
            }
            let already = profile
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM receipts WHERE event=?1)",
                    [&receipt],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(error)?;
            let is_bootstrap = training_keys.contains(&receipt);
            let eligible = can_train
                && (candidate.contexts.contains_key(&key)
                    || candidate.contexts.len() < settings.limits.max_contexts)
                && (reviewed || sampled(&receipt, *observations, admitted_groups))
                && !already
                && !seen.contains(&receipt)
                && (reviewed || !anchor_receipts.contains(&receipt))
                && !row.protected
                && (reviewed || !score.unusual())
                && (reviewed || row.ts <= training_end)
                && (training_keys.len() < TRAINING_CAP || is_bootstrap)
                && receipts_count + (training_keys.len() as u64) < RECEIPT_CAP;
            if eligible {
                let prior = trained_previous.cluster(&row);
                let prior_public = prior.and(trained_previous.public);
                let id = if is_bootstrap {
                    candidate.score(&key, &obs, None).cluster
                } else {
                    training_keys.insert(receipt);
                    candidate.train(&key, &obs, prior)
                };
                let domain = Domain::from_product(&row.product);
                contributions
                    .entry(domain)
                    .or_insert_with(|| Contribution::new(domain))
                    .observe(public, prior_public);
                trained_previous.set(&row, id, Some(public));
            } else if !row.protected && !score.unusual() && row.ts <= training_end {
                trained_previous.set(&row, candidate.score(&key, &obs, None).cluster, None);
            } else {
                trained_previous = Previous::default();
            }
        }
    }
    anchors.retain(|a| !reviewed_receipts.contains(&a.receipt));
    let mut before = 0u64;
    let mut after = 0u64;
    let mut established = 0u64;
    let mut bootstrap_alerts = 0u64;
    for sample in anchors.iter().chain(&holdout) {
        let a = frozen.score(&sample.key, &sample.observation(), None);
        let b = candidate.score(&sample.key, &sample.observation(), None);
        if a.ready {
            before += u64::from(a.unusual());
            after += u64::from(b.unusual());
            established += 1;
        }
        if b.unusual() {
            bootstrap_alerts += 1;
        }
    }
    let gate = can_train
        && !training_keys.is_empty()
        && holdout.len() >= 20
        && after <= before
        && (established >= 20
            || bootstrap_alerts.saturating_mul(5) < (anchors.len() + holdout.len()) as u64);
    let incidents = if expected.is_none() {
        build_incidents(db)?
    } else {
        0
    };
    let unusual: u64 = db
        .query_row("SELECT count(*) FROM learning_flags", [], |r| r.get(0))
        .map_err(error)?;
    let contributions = contributions
        .into_values()
        .filter(|c| c.total() > 0)
        .map(|mut c| {
            c.seal();
            c
        })
        .collect::<Vec<_>>();
    if gate && expected.is_none() {
        for contribution in &contributions {
            contribution.validate()?;
            db.execute(
                "INSERT INTO learning_exports VALUES(?1,?2)",
                params![
                    serde_json::to_string(&contribution.domain).map_err(error)?,
                    serde_json::to_string(contribution).map_err(error)?
                ],
            )
            .map_err(error)?;
        }
    }
    let used_revision = digest(&serde_json::to_vec(&frozen).map_err(error)?);
    let report = json!({"enabled":true,"version":VERSION,"model_revision":used_revision,"parent_revision":base,"model_generation":frozen.generation,"shared_revision":shared.revision(),"shared_matching_observations":shared_support,
        "repository":settings.github_repository,"branch":settings.github_branch,"total":total,"valid":valid,"scored":scored,"cold_start":cold_start,"unusual":unusual,"incidents":incidents,"contexts_limited":contexts_limited,"bootstrap_observations":bootstrap,"training_limit":TRAINING_CAP,"receipt_limit":RECEIPT_CAP,"compacted_receipts_bytes":seen.bytes().len(),"dedup_estimated_false_positive_rate":dedup_false_positive_rate,"model_contexts":frozen.contexts.len(),"dictionary_overflow":frozen.contexts.values().map(|c|c.overflow).sum::<u64>(),
        "validation":{"holdout":holdout.len(),"historical_anchors":anchors.len(),"before_alerts":before,"after_alerts":after,"accepted":gate,"labels_required":false,"scope":"Temporally held-out observations and frozen historical anchors; this is a consistency check, not measured incident detection accuracy"},
        "update":{"status":if gate{"validated_pending_publication"}else if !settings.auto_update{"disabled"}else if receipts_count>=RECEIPT_CAP{"receipt_budget_reached"}else{"withheld"},"applied":false},
        "meaning":"Surpresa estatística e associações observadas; não atribuem causalidade ou nível de evidência.","privacy":"Mensagens e perfis ficam neste computador. A sincronização só publica contagens de estados de um vocabulário fechado e depende de uma ação explícita."});
    let pending = if gate {
        candidate.generation += 1;
        Some(Pending {
            path,
            base,
            model: candidate,
            receipts: training_keys.into_iter().collect(),
            sentinels: holdout,
            reviewed_receipts: reviewed_receipts.into_iter().collect(),
        })
    } else {
        None
    };
    Ok((report, pending))
}

fn detect_changes(
    db: &Connection,
    model: &Model,
    groups: &[(String, String, String, i64, i64, u64)],
    persisted: &BTreeSet<String>,
    maximum: usize,
) -> Result<(), String> {
    for (context, ns, source, first, last, _) in groups.iter().take(maximum) {
        let key = model_key(context);
        let Some(reference) = model
            .contexts
            .get(&key)
            .filter(|c| c.count >= model.limits.minimum_support)
        else {
            continue;
        };
        let cutoff = if persisted.contains(&key) {
            first.saturating_sub(1)
        } else {
            first.saturating_add(last.saturating_sub(*first).saturating_mul(3) / 5)
        };
        let mut change = log_learning::DurationChange::new(&reference.latency);
        let mut previous = None::<i64>;
        let mut statement=db.prepare("SELECT (ts/60000-CASE WHEN ts<0 AND ts%60000<>0 THEN 1 ELSE 0 END)*60000,count(*),sum(failure),avg(latency) FROM learning_input WHERE context=?1 AND valid AND ts>?2 GROUP BY (ts/60000-CASE WHEN ts<0 AND ts%60000<>0 THEN 1 ELSE 0 END) ORDER BY 1").map_err(error)?;
        let windows = statement
            .query_map(params![context, cutoff], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, Option<f64>>(3)?,
                ))
            })
            .map_err(error)?;
        for window in windows {
            check()?;
            let (start, n, failures, latency) = window.map_err(error)?;
            let end = start.saturating_add(59999);
            if previous.is_some_and(|p| start.saturating_sub(p) > 900000) {
                if let Some(detector) = change.as_mut() {
                    detector.reset();
                }
            }
            previous = Some(start);
            let duration = latency
                .zip(change.as_mut())
                .is_some_and(|(value, detector)| detector.observe(value, n));
            let failure_shift = n >= 20
                && failures >= 3
                && log_learning::rate_interval(failures, n).0
                    > log_learning::rate_interval(reference.failures, reference.count).1;
            for (kind, active, condition, threshold) in [
                (
                    "duration_change",
                    duration,
                    "latency>?8",
                    reference.latency.upper_quantile(0.95) * 1.25,
                ),
                (
                    "failure_rate_change",
                    failure_shift,
                    "failure AND ?8>=0",
                    0.0,
                ),
            ] {
                if !active {
                    continue;
                }
                let detail=json!([{"kind":kind,"surprise":4.0,"support":reference.count,"detail":"Mudança observada em janelas cronológicas, comparada à referência fixa; confirmar carga, versão, dependências e coleta."}]).to_string();
                db.execute(&format!("INSERT OR IGNORE INTO learning_flags SELECT ref,eid,ts,context,?1,?2,?3,4.0 FROM learning_input WHERE context=?4 AND valid AND ts BETWEEN ?5 AND ?6 AND ts>?7 AND {condition}"),params![ns,source,detail,context,start,end,cutoff,threshold]).map_err(error)?;
            }
        }
    }
    Ok(())
}
fn build_incidents(db: &Connection) -> Result<u64, String> {
    let mut statement=db.prepare("SELECT context,ns,source,(ts/300000-CASE WHEN ts<0 AND ts%300000<>0 THEN 1 ELSE 0 END)*300000,count(*),max(surprise) FROM learning_flags GROUP BY context,(ts/300000-CASE WHEN ts<0 AND ts%300000<>0 THEN 1 ELSE 0 END) ORDER BY context,4").map_err(error)?;
    let groups = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, u64>(4)?,
                r.get::<_, f64>(5)?,
            ))
        })
        .map_err(error)?;
    let mut total = 0;
    for group in groups {
        check()?;
        let (context, ns, source, start, count, surprise) = group.map_err(error)?;
        let end = start.saturating_add(299999);
        let id = format!(
            "learned:{}",
            digest(json!([VERSION, context, start]).to_string().as_bytes())
        );
        let mut deviations=db.prepare("SELECT kinds FROM learning_flags WHERE context=?1 AND ts BETWEEN ?2 AND ?3 ORDER BY ref LIMIT 8").map_err(error)?;
        let deviations = deviations
            .query_map(params![context, start, end], |r| r.get::<_, String>(0))
            .map_err(error)?
            .map(|v| serde_json::from_str::<Value>(&v.map_err(error)?).map_err(error))
            .collect::<Result<Vec<_>, _>>()?;
        let contrasts = contrasts(db, &context, start, end)?;
        let explanation=format!("Registros que desviam da referência aprendida neste contexto e intervalo: {count}. As comparações mostram onde o desvio se concentra; confira as alternativas e os originais.");
        let payload = json!({"id":id,"kind":"local_learning","namespace":ns,"column":"source","value":source,"priority":35,"first":start,"last":end,"explanation":explanation,"measurements":{"model_version":VERSION,"supporting_events":count,"maximum_surprise_bits":surprise,"deviations":deviations,"comparisons":contrasts},"evidence_level":0,"benign_alternatives":["Mudança de versão ou configuração","Variação legítima de carga ou operação","Mudança de cobertura da coleta"],"missing_evidence":["Corroboração da causa e resultado da operação"]});
        let mut payload = payload;
        let fields: Vec<String> = serde_json::from_str(&context).map_err(error)?;
        payload["measurements"]["scope"] = json!({"namespace":ns,"source":source,"service":fields.get(2),"operation":fields.get(3)});
        let priority = if count >= 3 { 35 } else { 20 };
        payload["priority"] = json!(priority);
        if count < 3 {
            payload["missing_evidence"]
                .as_array_mut()
                .unwrap()
                .push(json!(
                    "Ocorrência isolada: falta suporte adicional para distinguir a hipótese."
                ));
        }
        db.execute(
            "INSERT INTO learning_incidents VALUES(?1,'local_learning',?2,'source',?3,?7,?4,?5,?6)",
            params![id, ns, source, start, end, payload.to_string(), priority],
        )
        .map_err(error)?;
        db.execute("INSERT OR IGNORE INTO investigation_members SELECT ?1,ref,eid,ts FROM learning_flags WHERE context=?2 AND ts BETWEEN ?3 AND ?4",params![id,context,start,end]).map_err(error)?;
        total += 1;
    }
    Ok(total)
}
fn contrasts(db: &Connection, context: &str, start: i64, end: i64) -> Result<Vec<Value>, String> {
    let (population,hits):(u64,u64)=db.query_row("SELECT count(*),sum(EXISTS(SELECT 1 FROM learning_flags f WHERE f.ref=i.ref)) FROM learning_input i WHERE context=?1 AND valid AND ts BETWEEN ?2 AND ?3",params![context,start,end],|r|Ok((r.get(0)?,r.get(1)?))).map_err(error)?;
    let mut result = Vec::new();
    // Field names are constants. Both cohorts share namespace, source, service,
    // operation and time, preventing unrelated populations from becoming controls.
    for field in ["host", "version", "dependency"] {
        let sql=format!("SELECT {field},count(*),sum(EXISTS(SELECT 1 FROM learning_flags f WHERE f.ref=i.ref)) FROM learning_input i WHERE context=?1 AND valid AND ts BETWEEN ?2 AND ?3 AND {field}<>'' GROUP BY {field} ORDER BY count(*) DESC,{field} LIMIT 32");
        let mut statement = db.prepare(&sql).map_err(error)?;
        for row in statement
            .query_map(params![context, start, end], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, u64>(2)?,
                ))
            })
            .map_err(error)?
        {
            let (value, n, k) = row.map_err(error)?;
            let rest = population - n;
            let rest_hits = hits - k;
            if n < 5 || k < 3 {
                continue;
            }
            let rate = k as f64 / n as f64;
            let other = if rest == 0 {
                None
            } else {
                Some(rest_hits as f64 / rest as f64)
            };
            let lower = log_learning::rate_interval(k, n).0;
            let upper = log_learning::rate_interval(rest_hits, rest).1;
            let discriminates = rest >= 5 && lower > upper;
            result.push(json!({"field":field,"value":value,"observations":n,"unusual":k,"rate":rate,"comparison_observations":rest,"comparison_unusual":rest_hits,"comparison_rate":other,"status":if discriminates{"supports_association"}else if rest<5{"insufficient_controls"}else{"inconclusive"},"hypothesis":if discriminates{format!("Investigar a diferença associada a {field}={value}; o desvio se concentra nesse grupo.")}else{"A comparação disponível não distingue esta hipótese.".into()},"limitations":["Associação observacional; não demonstra causa.","Comparações exploratórias exigem confirmação independente."]}));
        }
    }
    result.sort_by(|a, b| {
        b["status"]
            .as_str()
            .eq(&Some("supports_association"))
            .cmp(&a["status"].as_str().eq(&Some("supports_association")))
            .then_with(|| {
                b["rate"]
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&a["rate"].as_f64().unwrap_or(0.0))
            })
            .then_with(|| a["value"].as_str().cmp(&b["value"].as_str()))
    });
    result.truncate(6);
    Ok(result)
}

pub fn reset() -> Result<Value, String> {
    let path = profile_path();
    let mut db = open_profile(&path)?;
    db.backup(
        rusqlite::DatabaseName::Main,
        path.with_extension("reset-backup.sqlite"),
        None,
    )
    .map_err(error)?;
    let tx = db.transaction().map_err(error)?;
    tx.execute_batch(
        "DELETE FROM model;DELETE FROM receipts;DELETE FROM revisions;DELETE FROM sentinels;DELETE FROM compacted_receipts;",
    )
    .map_err(error)?;
    tx.commit().map_err(error)?;
    Ok(json!({"status":"reset","scope":"current_case_and_interpretation","logs_modified":false}))
}
pub fn rollback() -> Result<Value, String> {
    let path = profile_path();
    let mut db = open_profile(&path)?;
    let revisions: u64 = db
        .query_row("SELECT count(*) FROM revisions", [], |r| r.get(0))
        .map_err(error)?;
    let backup = path.with_extension("reset-backup.sqlite");
    if revisions == 0 && backup.exists() {
        db.restore(
            rusqlite::DatabaseName::Main,
            &backup,
            None::<fn(rusqlite::backup::Progress)>,
        )
        .map_err(error)?;
        std::fs::remove_file(backup).map_err(error)?;
        return Ok(json!({"status":"rolled_back_reset","logs_modified":false}));
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(error)?;
    let (generation, revision, text): (u64, String, String) = tx
        .query_row(
            "SELECT generation,revision,payload FROM revisions ORDER BY generation DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "Não há uma revisão anterior do aprendizado")?;
    tx.execute(
        "INSERT OR REPLACE INTO model VALUES(1,?1,?2)",
        params![revision, text],
    )
    .map_err(error)?;
    tx.execute("DELETE FROM receipts WHERE generation>?1", [generation])
        .map_err(error)?;
    tx.execute("DELETE FROM revisions WHERE generation>=?1", [generation])
        .map_err(error)?;
    tx.commit().map_err(error)?;
    Ok(json!({"status":"rolled_back","revision":revision,"generation":generation}))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("LOGINSIGHT_DATA_DIR", v),
                None => std::env::remove_var("LOGINSIGHT_DATA_DIR"),
            }
        }
    }
    fn isolated(run: impl FnOnce()) {
        let root = tempfile::tempdir().unwrap();
        let _restore = Restore(std::env::var_os("LOGINSIGHT_DATA_DIR"));
        std::env::set_var("LOGINSIGHT_DATA_DIR", root.path());
        run();
    }
    fn fixture(prefix: &str, incident: bool, invalid: bool) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE investigation_facts(ref TEXT PRIMARY KEY,eid INTEGER,ns TEXT,ts INTEGER,source TEXT,host TEXT,session TEXT,payload TEXT);
            CREATE TABLE findings(n INTEGER PRIMARY KEY,level INTEGER);CREATE TABLE members(n INTEGER,ref TEXT);
            CREATE TABLE investigation_signals(id TEXT PRIMARY KEY,kind TEXT,ns TEXT,col TEXT,entity TEXT,priority INTEGER,first INTEGER,last INTEGER,payload TEXT);
            CREATE TABLE investigation_members(id TEXT,ref TEXT,eid INTEGER,ts INTEGER,PRIMARY KEY(id,ref));").unwrap();
        for i in 0..400 {
            let failure = incident && (250..280).contains(&i);
            let payload = json!({"template":if failure{"database connection pool exhausted"}else{"request completed successfully"},"action":"request","outcome":if failure{"failure"}else{"success"},"code":if failure{"500"}else{"200"},"parse_ok":!invalid,"latency_ms":if failure{8000.0}else{12.0},"bytes":100,"metadata":{"product":"application","service":"checkout-api","time":{"ambiguity":Value::Null},"learning":{"operation":"checkout","version":if failure{"B"}else{"A"},"dependency":"database","failure":failure}}});
            db.execute("INSERT INTO investigation_facts VALUES(?1,?2,'tenant-a',?3,'application.log',?4,'',?5)",params![format!("{prefix}:{i}"),i,1_700_000_000_000i64+i*1000,if failure{"host-b"}else{"host-a"},payload.to_string()]).unwrap();
        }
        db
    }
    #[test]
    fn learned_incidents_have_exact_members_controls_and_no_automatic_evidence_promotion() {
        isolated(|| {
            let db = fixture("batch-a", true, false);
            let (report, pending) = analyze(&db, &Settings::default()).unwrap();
            assert!(report["incidents"].as_u64().unwrap() > 0);
            assert_eq!(report["validation"]["labels_required"], false);
            let payload: String = db
                .query_row(
                    "SELECT payload FROM learning_incidents WHERE kind='local_learning' LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let signal: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(signal["evidence_level"], 0);
            assert!(signal["measurements"]["comparisons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["status"] == "supports_association" && c["field"] == "version"));
            let members: u64 = db
                .query_row("SELECT count(*) FROM investigation_members", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(members, 30);
            let update = pending
                .expect("clean holdout validates the normal portion")
                .commit()
                .unwrap();
            assert_eq!(update["applied"], true);
            let profile = open_profile(&profile_path()).unwrap();
            let withheld_row=db.query_row("SELECT ref,eid,ts,message,failure,latency,bytes,stream,protected,product,action,outcome FROM learning_input WHERE ref='batch-a:260'",[],Row::get).unwrap();
            let withheld: bool = profile
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM receipts WHERE event=?1)",
                    [withheld_row.receipt()],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!withheld);
        });
    }
    #[test]
    fn repeated_analysis_is_idempotent_and_incremental_updates_support_rollback() {
        isolated(|| {
            let (_, pending) =
                analyze(&fixture("batch-a", false, false), &Settings::default()).unwrap();
            let first = pending.unwrap().commit().unwrap();
            let (_, pending) =
                analyze(&fixture("batch-a", false, false), &Settings::default()).unwrap();
            assert!(
                pending.is_none(),
                "same events cannot increment counts again"
            );
            let (_, pending) =
                analyze(&fixture("batch-b", false, false), &Settings::default()).unwrap();
            let second = pending.unwrap().commit().unwrap();
            assert!(second["generation"].as_u64() > first["generation"].as_u64());
            let restored = rollback().unwrap();
            assert_eq!(restored["revision"], first["revision"]);
        });
    }
    #[test]
    fn quality_gate_and_disabled_updates_never_write_a_candidate() {
        isolated(|| {
            let (report, pending) =
                analyze(&fixture("invalid", false, true), &Settings::default()).unwrap();
            assert_eq!(report["valid"], 0);
            assert!(pending.is_none());
            let settings = Settings {
                auto_update: false,
                ..Default::default()
            };
            let (report, pending) = analyze(&fixture("normal", false, false), &settings).unwrap();
            assert!(pending.is_none());
            assert_eq!(report["update"]["status"], "disabled");
        });
    }
    #[test]
    fn uncommitted_analysis_cannot_train_and_competing_candidates_use_cas() {
        isolated(|| {
            let (_, discarded) =
                analyze(&fixture("discarded", false, false), &Settings::default()).unwrap();
            drop(discarded);
            let db = open_profile(&profile_path()).unwrap();
            assert_eq!(
                db.query_row("SELECT count(*) FROM model", [], |r| r.get::<_, u64>(0))
                    .unwrap(),
                0
            );
            drop(db);
            let (_, a) = analyze(&fixture("a", false, false), &Settings::default()).unwrap();
            let (_, b) = analyze(&fixture("b", false, false), &Settings::default()).unwrap();
            a.unwrap().commit().unwrap();
            assert_eq!(
                b.unwrap().commit().unwrap()["status"],
                "concurrent_update_deferred"
            );
        });
    }
    #[test]
    fn stable_service_continues_across_rotated_files_without_crossing_namespaces() {
        let a = json!(["source:one.log", "one.log", "service-a", "checkout"]).to_string();
        let b = json!(["source:two.log", "two.log", "service-a", "checkout"]).to_string();
        assert_eq!(model_key(&a), model_key(&b));
        assert_ne!(
            model_key(&json!(["tenant-a", "one.log", "service-a", "checkout"]).to_string()),
            model_key(&json!(["tenant-b", "one.log", "service-a", "checkout"]).to_string())
        );
    }
    #[test]
    fn resetting_the_profile_preserves_a_recoverable_backup() {
        isolated(|| {
            let (_, pending) =
                analyze(&fixture("original", false, false), &Settings::default()).unwrap();
            let original = pending.unwrap().commit().unwrap();
            reset().unwrap();
            assert_eq!(rollback().unwrap()["status"], "rolled_back_reset");
            let profile = open_profile(&profile_path()).unwrap();
            let (_, revision) = load_model(&profile, &Limits::default()).unwrap();
            assert_eq!(revision, original["revision"].as_str().unwrap());
        });
    }
    #[test]
    fn assessed_evidence_and_priority_signals_are_withheld_from_training_and_export_is_textless() {
        isolated(|| {
            let db = fixture("protected", false, false);
            db.execute_batch("INSERT INTO findings VALUES(1,3);INSERT INTO members VALUES(1,'protected:100');INSERT INTO investigation_signals VALUES('priority','known','tenant-a','source','application.log',35,0,0,'{}');INSERT INTO investigation_members VALUES('priority','protected:101',101,0);").unwrap();
            let (_, pending) = analyze(&db, &Settings::default()).unwrap();
            pending.unwrap().commit().unwrap();
            let profile = open_profile(&profile_path()).unwrap();
            for reference in ["protected:100", "protected:101"] {
                let row=db.query_row("SELECT ref,eid,ts,message,failure,latency,bytes,stream,protected,product,action,outcome FROM learning_input WHERE ref=?1",[reference],Row::get).unwrap();
                let stored: bool = profile
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM receipts WHERE event=?1)",
                        [row.receipt()],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert!(!stored);
            }
            let text: String = db
                .query_row("SELECT payload FROM learning_exports LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            for secret in [
                "application.log",
                "tenant-a",
                "host-a",
                "checkout-api",
                "request completed",
            ] {
                assert!(!text.contains(secret));
            }
        });
    }
    #[test]
    fn rotated_sources_share_a_temporal_boundary_without_learning_from_the_evaluated_future() {
        isolated(|| {
            let db = fixture("old", false, false);
            db.execute_batch("INSERT INTO investigation_facts SELECT replace(ref,'old:','new:'),eid+400,ns,ts+3600000,'new.log',host,session,json_set(payload,'$.template','database connection pool exhausted','$.outcome','failure','$.code','500','$.latency_ms',8000,'$.metadata.learning.failure',1) FROM investigation_facts;").unwrap();
            let (report, _) = analyze(&db, &Settings::default()).unwrap();
            assert_eq!(report["scored"], 400);
            assert_eq!(report["unusual"], 400);
        });
    }
    #[test]
    fn optional_review_adds_expected_patterns_without_erasing_the_original_diagnosis() {
        isolated(|| {
            let db = fixture("review", true, false);
            let (_, pending) = analyze(&db, &Settings::default()).unwrap();
            pending.unwrap().commit().unwrap();
            let id: String = db
                .query_row(
                    "SELECT id FROM learning_incidents ORDER BY id LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let result = learn_expected(&db, &Settings::default(), &id).unwrap();
            assert_eq!(result["applied"], true);
            assert_eq!(result["classification_updated"], false);
            let profile = open_profile(&profile_path()).unwrap();
            let (model, _) = load_model(&profile, &Limits::default()).unwrap();
            let context =
                json!(["tenant-a", "application.log", "checkout-api", "checkout"]).to_string();
            let obs = Observation {
                message: "database connection pool exhausted",
                failure: true,
                latency_ms: Some(8000.0),
                bytes: Some(100.0),
            };
            assert!(!model
                .score(&model_key(&context), &obs, None)
                .deviations
                .iter()
                .any(|d| d.kind == "message_novelty"));
            assert!(db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM learning_incidents WHERE id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap());
            assert_eq!(
                learn_expected(&db, &Settings::default(), &id).unwrap()["applied"],
                false
            );
        });
    }
    #[test]
    fn a_single_rare_event_keeps_its_original_evidence_and_declares_low_support() {
        for epoch_offset in [0i64, -1_700_000_500_000] {
            isolated(|| {
                let db = fixture("rare", false, false);
                db.execute("UPDATE investigation_facts SET ts=ts+?1", [epoch_offset])
                    .unwrap();
                db.execute("UPDATE investigation_facts SET payload=json_set(payload,'$.template','previously unseen message','$.latency_ms',8000) WHERE ref='rare:350'",[]).unwrap();
                let (report, _) = analyze(&db, &Settings::default()).unwrap();
                assert_eq!(report["unusual"], 1);
                assert_eq!(report["incidents"], 1);
                let priority: u64 = db
                    .query_row("SELECT priority FROM learning_incidents", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(priority, 20);
                let reference: String = db
                    .query_row("SELECT ref FROM investigation_members", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(reference, "rare:350");
            });
        }
    }
    #[test]
    fn native_event_codes_are_not_misread_as_http_failures() {
        isolated(|| {
            let db = fixture("windows", false, false);
            db.execute_batch("UPDATE investigation_facts SET payload=json_set(payload,'$.code','500','$.outcome','unknown','$.metadata.product','windows-security','$.metadata.learning.failure',0);").unwrap();
            let (_, pending) = analyze(&db, &Settings::default()).unwrap();
            pending.unwrap().commit().unwrap();
            let profile = open_profile(&profile_path()).unwrap();
            let (model, _) = load_model(&profile, &Limits::default()).unwrap();
            assert!(model.contexts.values().all(|c| c.failures == 0));
        });
    }
}
