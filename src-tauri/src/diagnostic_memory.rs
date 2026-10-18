//! Private incident resolutions, scoped to the captured Case and interpretation.
//! Feedback is explicit; editing it rebuilds a bounded challenger from current
//! labels so an old mistaken review is not trained repeatedly or kept forever.
use log_learning::{Diagnosis, DiagnosticRanker, DiagnosticRankingExample, DiagnosticSummary};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_RESOLUTIONS: u64 = 10_000;
fn error(e: impl std::fmt::Display) -> String {
    format!("Memória diagnóstica: {e}")
}
pub fn path() -> std::path::PathBuf {
    let mut mappings =
        serde_json::to_value(crate::detections::load_settings().mappings).unwrap_or(Value::Null);
    mappings.sort_all_objects();
    let owner = json!([
        log_learning::DIAGNOSTIC_VERSION,
        crate::evidence::NORMALIZATION_VERSION,
        crate::analysis_runtime::fact_namespace(),
        crate::analysis_runtime::fact_interpretation_signature(),
        mappings
    ]);
    crate::config_dir().join("diagnostic-memory").join(format!(
        "{}.sqlite",
        log_learning::digest(owner.to_string().as_bytes())
    ))
}
fn open() -> Result<Connection, String> {
    let path = path();
    std::fs::create_dir_all(path.parent().ok_or("Diretório da memória ausente")?).map_err(error)?;
    let db = Connection::open(path).map_err(error)?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(error)?;
    db.execute_batch("PRAGMA journal_mode=DELETE;PRAGMA cache_size=-1024;PRAGMA mmap_size=0;PRAGMA max_page_count=16384;
        CREATE TABLE IF NOT EXISTS resolutions(episode TEXT,diagnosis TEXT,context TEXT,at INTEGER,verdict TEXT,rationale TEXT,features INTEGER,base_score INTEGER,eligible INTEGER,evidence TEXT,updated INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(episode,diagnosis));
        CREATE INDEX IF NOT EXISTS resolution_context ON resolutions(context,at DESC);
        CREATE TABLE IF NOT EXISTS memory_state(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER,model TEXT);
        CREATE TABLE IF NOT EXISTS sharing_origin(id INTEGER PRIMARY KEY CHECK(id=1),receipt TEXT);
        CREATE TABLE IF NOT EXISTS sharing_episodes(episode TEXT PRIMARY KEY,receipt TEXT UNIQUE);").map_err(error)?;
    let updated = db
        .prepare("PRAGMA table_info(resolutions)")
        .map_err(error)?
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?
        .iter()
        .any(|s| s == "updated");
    if !updated {
        db.execute_batch("ALTER TABLE resolutions ADD COLUMN updated INTEGER NOT NULL DEFAULT 0")
            .map_err(error)?;
    }
    Ok(db)
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Confirmed,
    Refuted,
    Expected,
    Inconclusive,
}
impl Verdict {
    fn label(&self) -> &'static str {
        match self {
            Self::Confirmed => "Causa confirmada por revisão",
            Self::Refuted => "Hipótese descartada por revisão",
            Self::Expected => "Mudança esperada",
            Self::Inconclusive => "Investigação inconclusiva",
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feedback {
    pub verdict: Verdict,
    #[serde(default)]
    pub diagnosis: Option<Diagnosis>,
    pub rationale: String,
}
pub struct Snapshot {
    pub revision: u64,
    pub model: DiagnosticRanker,
    db: Connection,
}
pub fn snapshot() -> Result<Snapshot, String> {
    let db = open()?;
    let entry: Option<(u64, String)> = db
        .query_row(
            "SELECT revision,model FROM memory_state WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(error)?;
    match entry {
        None => Ok(Snapshot {
            revision: 0,
            model: DiagnosticRanker::default(),
            db,
        }),
        Some((revision, text)) => {
            if text.len() > 128 * 1024 {
                return Err("Ranking excedeu o orçamento".into());
            }
            let model: DiagnosticRanker = serde_json::from_str(&text).map_err(error)?;
            model.validate()?;
            Ok(Snapshot {
                revision,
                model,
                db,
            })
        }
    }
}
pub fn status() -> Result<Value, String> {
    let snapshot = snapshot()?;
    let count: u64 = snapshot
        .db
        .query_row("SELECT count(*) FROM resolutions", [], |r| r.get(0))
        .map_err(error)?;
    Ok(
        json!({"revision":snapshot.revision,"resolutions":count,"limit":MAX_RESOLUTIONS,"ranking":{"active":snapshot.model.active,"training_episodes":snapshot.model.training_episodes,"validation_episodes":snapshot.model.validation_episodes,"validation_pairs":snapshot.model.validation_pairs,"baseline_correct":snapshot.model.baseline_correct,"candidate_correct":snapshot.model.candidate_correct,"meaning":"Ranking pequeno, limitado a hipóteses já sustentadas. A promoção exige ganho em pares confirmados/refutados de episódios posteriores, sem regressão por família. Não é calibração de probabilidades."},"scope":"Case and captured interpretation; local only"}),
    )
}
pub fn precedents(
    snapshot: &Snapshot,
    context: &str,
    mask: u64,
    current_episode: &str,
) -> Result<Vec<Value>, String> {
    if snapshot.revision == 0 {
        return Ok(Vec::new());
    }
    let mut statement=snapshot.db.prepare("SELECT episode,diagnosis,at,verdict,rationale,features FROM resolutions WHERE context=?1 AND episode<>?2 ORDER BY at DESC,episode,diagnosis LIMIT 128").map_err(error)?;
    let mut rows = statement
        .query(params![context, current_episode])
        .map_err(error)?;
    let mut matches = Vec::new();
    // Coverage markers are not an incident signature. Only recognized symptoms
    // participate; the same version/trace alone must never retrieve a cause.
    let contextual = log_learning::DiagnosticFeature::VersionMeasured.bit()
        | log_learning::DiagnosticFeature::TraceMeasured.bit()
        | log_learning::DiagnosticFeature::QueryMeasured.bit()
        | log_learning::DiagnosticFeature::PoolMeasured.bit()
        | log_learning::DiagnosticFeature::DependencyMeasured.bit()
        | log_learning::DiagnosticFeature::AcquisitionMeasured.bit()
        | log_learning::DiagnosticFeature::PoolAtLimit.bit()
        | log_learning::DiagnosticFeature::PoolQueued.bit()
        | log_learning::DiagnosticFeature::PoolHasCapacity.bit()
        | log_learning::DiagnosticFeature::LearnedDeviation.bit();
    let mask = mask & !contextual;
    if mask == 0 {
        return Ok(matches);
    }
    while let Some(row) = rows.next().map_err(error)? {
        let past = row.get::<_, i64>(5).map_err(error)? as u64 & !contextual;
        let common = (past & mask).count_ones();
        let union = (past | mask).count_ones();
        if common == 0 || common * 2 < union {
            continue;
        }
        let diagnosis: Option<Diagnosis> =
            serde_json::from_str(&row.get::<_, String>(1).map_err(error)?).map_err(error)?;
        let verdict: Verdict =
            serde_json::from_str(&row.get::<_, String>(3).map_err(error)?).map_err(error)?;
        matches.push(json!({"episode":row.get::<_,String>(0).map_err(error)?,"diagnosis":diagnosis,"title":diagnosis.map(|d|d.title()).unwrap_or("Revisão do episódio"),"at":row.get::<_,Option<i64>>(2).map_err(error)?,"verdict":verdict,"verdict_label":verdict.label(),"rationale":row.get::<_,String>(4).map_err(error)?,"similarity":common as f64/union as f64,"is_current_evidence":false}));
    }
    matches.sort_by(|a, b| {
        b["similarity"]
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&a["similarity"].as_f64().unwrap_or(0.0))
            .then(b["at"].as_i64().cmp(&a["at"].as_i64()))
    });
    matches.truncate(5);
    Ok(matches)
}
pub fn record(
    analysis_db: &Connection,
    episode_id: &str,
    document: Value,
) -> Result<Value, String> {
    let feedback: Feedback = serde_json::from_value(document).map_err(error)?;
    if feedback.rationale.trim().chars().count() < 8 || feedback.rationale.len() > 2000 {
        return Err("Descreva a verificação e o resultado (8–2000 caracteres/bytes)".into());
    }
    if matches!(feedback.verdict, Verdict::Confirmed | Verdict::Refuted)
        && feedback.diagnosis.is_none()
    {
        return Err("Escolha a hipótese confirmada ou descartada".into());
    }
    let episode = crate::local_diagnostics::episode(analysis_db, episode_id)?;
    let summary: DiagnosticSummary =
        serde_json::from_value(episode["summary"].clone()).map_err(error)?;
    let context = episode["scope"]["context"]
        .as_str()
        .ok_or("Episódio sem contexto")?;
    let diagnosis = if matches!(feedback.verdict, Verdict::Expected | Verdict::Inconclusive) {
        None
    } else {
        feedback.diagnosis
    };
    let diagnosis_key = serde_json::to_string(&diagnosis).map_err(error)?;
    let verdict_key = serde_json::to_string(&feedback.verdict).map_err(error)?;
    let hypothesis = episode["assessment"]["hypotheses"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|h| {
                serde_json::from_value::<Diagnosis>(h["diagnosis"].clone()).ok() == diagnosis
            })
        });
    let base = hypothesis.and_then(|h| h["score"].as_u64()).unwrap_or(0);
    let eligible =
        hypothesis.is_some() && matches!(feedback.verdict, Verdict::Confirmed | Verdict::Refuted);
    let evidence = summary
        .features
        .values()
        .flat_map(|s| s.event_refs.iter().map(|r| r.event_ref.clone()))
        .take(32)
        .collect::<Vec<_>>();
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(error)?;
    let previous: Option<(String, String)> = tx
        .query_row(
            "SELECT verdict,rationale FROM resolutions WHERE episode=?1 AND diagnosis=?2",
            params![episode_id, diagnosis_key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(error)?;
    if previous.as_ref().is_some_and(|(verdict, rationale)| {
        verdict == &verdict_key && rationale == feedback.rationale.trim()
    }) {
        return Ok(
            json!({"saved":true,"unchanged":true,"classification_updated":false,"raw_logs_sent":false}),
        );
    }
    // Episode-level expected/inconclusive replaces prior causal judgments.
    // Returning to a causal review removes that episode-level placeholder.
    tx.execute(
        "DELETE FROM resolutions WHERE episode=?1 AND (?2 OR diagnosis='null')",
        params![episode_id, diagnosis.is_none()],
    )
    .map_err(error)?;
    let count: u64 = tx
        .query_row("SELECT count(*) FROM resolutions", [], |r| r.get(0))
        .map_err(error)?;
    if previous.is_none() && count >= MAX_RESOLUTIONS {
        return Err("A memória atingiu 10000 resoluções; o histórico foi preservado e a nova revisão não foi gravada".into());
    }
    let revision: u64 = tx
        .query_row(
            "SELECT coalesce((SELECT revision FROM memory_state WHERE id=1),0)+1",
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    tx.execute(
        "INSERT OR REPLACE INTO resolutions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            episode_id,
            diagnosis_key,
            context,
            episode["first"].as_i64(),
            verdict_key,
            feedback.rationale.trim(),
            summary.mask() as i64,
            base,
            eligible,
            serde_json::to_string(&evidence).map_err(error)?,
            revision
        ],
    )
    .map_err(error)?;
    let mut examples = Vec::new();
    {
        let mut statement=tx.prepare("SELECT episode,diagnosis,at,verdict,features,base_score FROM resolutions WHERE eligible ORDER BY at DESC,episode,diagnosis LIMIT 2048").map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            crate::operations::check()?;
            let Some(at) = row.get::<_, Option<i64>>(2).map_err(error)? else {
                continue;
            };
            examples.push(DiagnosticRankingExample {
                episode: row.get(0).map_err(error)?,
                diagnosis: serde_json::from_str(&row.get::<_, String>(1).map_err(error)?)
                    .map_err(error)?,
                at,
                confirmed: row.get::<_, String>(3).map_err(error)? == "\"confirmed\"",
                features: row.get::<_, i64>(4).map_err(error)? as u64,
                base_score: row.get(5).map_err(error)?,
            });
        }
    }
    examples.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then(a.episode.cmp(&b.episode))
            .then(a.diagnosis.cmp(&b.diagnosis))
    });
    let model = log_learning::train_diagnostic_ranker(&examples)?;
    tx.execute(
        "INSERT OR REPLACE INTO memory_state VALUES(1,?1,?2)",
        params![revision, serde_json::to_string(&model).map_err(error)?],
    )
    .map_err(error)?;
    crate::operations::check()?;
    crate::operations::commit();
    tx.commit().map_err(error)?;
    Ok(
        json!({"saved":true,"revision":revision,"ranking_active":model.active,"classification_updated":false,"raw_logs_sent":false,"meaning":"Revisão declarada pelo usuário; não equivale a validação representativa. Precedentes são locais e só ordenam hipóteses que têm suporte no episódio atual."}),
    )
}
pub fn contributions() -> Result<log_learning::DiagnosticContributionBundle, String> {
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(error)?;
    tx.execute(
        "INSERT OR IGNORE INTO sharing_origin VALUES(1,?1)",
        [uuid::Uuid::new_v4().simple().to_string()],
    )
    .map_err(error)?;
    let origin: String = tx
        .query_row("SELECT receipt FROM sharing_origin WHERE id=1", [], |r| {
            r.get(0)
        })
        .map_err(error)?;
    let episodes = {
        let mut statement=tx.prepare("SELECT episode,max(updated) FROM resolutions r WHERE eligible OR EXISTS(SELECT 1 FROM sharing_episodes s WHERE s.episode=r.episode) GROUP BY episode ORDER BY max(updated) DESC,episode LIMIT 256").map_err(error)?;
        let values = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        values
    };
    let mut contributions = Vec::new();
    for (episode, revision) in episodes {
        crate::operations::check()?;
        tx.execute(
            "INSERT OR IGNORE INTO sharing_episodes VALUES(?1,?2)",
            params![episode, uuid::Uuid::new_v4().simple().to_string()],
        )
        .map_err(error)?;
        let receipt: String = tx
            .query_row(
                "SELECT receipt FROM sharing_episodes WHERE episode=?1",
                [&episode],
                |r| r.get(0),
            )
            .map_err(error)?;
        let mask: u64 = tx
            .query_row(
                "SELECT features FROM resolutions WHERE episode=?1 ORDER BY updated DESC LIMIT 1",
                [&episode],
                |r| r.get(0),
            )
            .map_err(error)?;
        let mut scores = std::collections::BTreeMap::new();
        let mut labels = Vec::new();
        let mut statement=tx.prepare("SELECT diagnosis,verdict,base_score FROM resolutions WHERE episode=?1 AND features=?2 AND eligible ORDER BY diagnosis").map_err(error)?;
        let mut rows = statement.query(params![episode, mask]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            let diagnosis: Diagnosis =
                serde_json::from_str(&row.get::<_, String>(0).map_err(error)?).map_err(error)?;
            scores.insert(diagnosis, row.get(2).map_err(error)?);
            labels.push(log_learning::DiagnosticLabel {
                diagnosis,
                confirmed: row.get::<_, String>(1).map_err(error)? == "\"confirmed\"",
            });
        }
        let mut contribution = log_learning::DiagnosticContribution {
            schema_version: log_learning::DIAGNOSTIC_SCHEMA,
            id: String::new(),
            origin: origin.clone(),
            episode: receipt,
            revision,
            features: if labels.is_empty() { 0 } else { mask },
            scores,
            labels,
        };
        contribution.seal();
        contribution.validate()?;
        contributions.push(contribution);
    }
    tx.commit().map_err(error)?;
    let bundle = log_learning::DiagnosticContributionBundle {
        schema_version: log_learning::DIAGNOSTIC_SCHEMA,
        contributions,
    };
    bundle.validate()?;
    Ok(bundle)
}
