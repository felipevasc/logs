//! Evidence-linked local diagnoses. Population work is streamed through SQLite;
//! only bounded hypotheses, references and query pages reach the UI/MCP.
use log_learning::{
    DiagnosticEventFacts, DiagnosticFeature as F, DiagnosticMetric as M, DiagnosticQuery,
    DiagnosticRecord, DiagnosticReference, DiagnosticSummary, DIAGNOSTIC_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const EPISODE_GAP_MS: i64 = 120_000;
const MAX_EPISODE_MS: i64 = 21_600_000;
const MAX_EPISODES: u64 = 10_000;
fn error(e: impl std::fmt::Display) -> String {
    format!("Diagnóstico local: {e}")
}
fn check() -> Result<(), String> {
    crate::operations::check()?;
    crate::security_budget::check()
}

/// Only explicitly named quantities with known units enter the diagnostic
/// vocabulary. Ambiguous generic durations are left for field interpretation.
pub fn event_facts(
    event: &crate::model::Event,
    n: &crate::security_normalize::Normalized,
) -> Value {
    let first = |fields: &[&str]| {
        fields.iter().find_map(|&field| {
            event
                .col_str(field)
                .filter(|v| !v.trim().is_empty() && v.chars().count() <= 256)
                .map(|v| (field.to_string(), v))
        })
    };
    let mapped = |role: &str| {
        n.values
            .get(role)
            .filter(|p| !p.value.trim().is_empty() && p.field.len() <= 256 && p.value.len() <= 256)
            .map(|p| (p.field.clone(), p.value.clone()))
    };
    let code = first(&["error.code", "sqlstate", "db.sqlstate", "SQLSTATE", "errno"])
        .or_else(|| mapped("sqlstate"))
        .or_else(|| mapped("error_code"));
    let exception = first(&["exception.type", "error.type", "exception.class"])
        .or_else(|| mapped("exception_type"));
    let auth = first(&[
        "authentication.failure_reason",
        "auth.failure_reason",
        "auth.reason",
        "winlog.event_data.Status",
        "Status",
    ])
    .or_else(|| mapped("auth_reason"));
    let mut record = DiagnosticRecord {
        message: event.message.chars().take(8192).collect(),
        code: code
            .as_ref()
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| event.code.clone()),
        exception: exception
            .as_ref()
            .map(|(_, v)| v.clone())
            .unwrap_or_default(),
        auth_reason: auth.as_ref().map(|(_, v)| v.clone()).unwrap_or_default(),
        product: n.product.clone(),
        failure: crate::insights::is_error(event)
            || matches!(n.get("outcome"), Some("failure" | "blocked")),
        success: n.get("outcome") == Some("success"),
        documentation: crate::security_normalize::field_text(event, "event.kind").as_deref()
            == Some("documentation")
            || n.content_clipped,
        deployment: matches!(
            n.get("action"),
            Some("deployment" | "deploy" | "deployment_complete")
        ) || first(&["event.action"]).is_some_and(|(_, v)| {
            matches!(v.as_str(), "deployment" | "deploy" | "deployment_complete")
        }),
        ..Default::default()
    };
    if n.get("sqlstate").is_some()
        || code
            .as_ref()
            .is_some_and(|(field, _)| field.to_ascii_lowercase().contains("sqlstate"))
    {
        record.product = "postgresql".into();
    }
    if let Some((_, system)) = first(&["db.system"]) {
        if matches!(system.as_str(), "postgresql" | "mysql") {
            record.product = system;
        }
    }
    // Many native adapters do not know an application's outcome. An explicit
    // error level remains a report of failure; an explicit success wins on a
    // contradiction, which is already visible in normalization provenance.
    let quantities: &[(M, &[&str], bool)] = &[
        (
            M::PoolActive,
            &["db.pool.active", "pool.active", "pool_active"],
            false,
        ),
        (
            M::PoolLimit,
            &["db.pool.max", "pool.max", "pool_max"],
            false,
        ),
        (
            M::PoolPending,
            &["db.pool.pending", "pool.pending", "pool_pending"],
            false,
        ),
        (
            M::AcquireMs,
            &[
                "db.pool.acquire_ms",
                "pool.acquire_ms",
                "connection.acquire_ms",
            ],
            true,
        ),
        (
            M::QueryMs,
            &["db.query.duration_ms", "db.query_ms", "query.duration_ms"],
            true,
        ),
        (
            M::QueryLimitMs,
            &["db.query.limit_ms", "query.limit_ms"],
            true,
        ),
        (
            M::DependencyMs,
            &["dependency.duration_ms", "upstream.duration_ms"],
            true,
        ),
        (
            M::DependencyLimitMs,
            &["dependency.timeout_ms", "upstream.timeout_ms"],
            true,
        ),
        (M::LockWaitMs, &["db.lock.wait_ms", "lock.wait_ms"], true),
        (
            M::RequestRate,
            &["request.rate_per_second", "requests_per_second"],
            false,
        ),
        (
            M::RequestRateBaseline,
            &[
                "request.baseline_per_second",
                "requests_baseline_per_second",
            ],
            false,
        ),
        (M::RetryCount, &["retry.count", "retry_count"], false),
        (M::RetryLimit, &["retry.limit", "retry_limit"], false),
    ];
    let mut sources = BTreeMap::new();
    for &(metric, fields, duration) in quantities {
        if let Some(number) = n.numbers.get(metric.role()).filter(|number| {
            number.unit == metric.unit()
                && number.value.is_finite()
                && (0.0..=1e15).contains(&number.value)
        }) {
            record.metrics.insert(metric, number.value);
            sources.insert(metric.role().into(), number.field.clone());
            continue;
        }
        // An explicit mapping with a missing/incompatible unit must stay unknown;
        // do not silently fall back to a similarly named quantity.
        if n.values.contains_key(metric.role()) {
            continue;
        }
        for &field in fields {
            let Some(text) = event.col_str(field) else {
                continue;
            };
            let Some((value, unit)) = crate::analysis::parse_num_unit(&text) else {
                continue;
            };
            let compatible = if duration {
                matches!(
                    unit,
                    crate::analysis::UnitKind::Number | crate::analysis::UnitKind::DurationMs
                )
            } else {
                unit == crate::analysis::UnitKind::Number
            };
            if compatible && value >= 0.0 && value <= 1e15 {
                record.metrics.insert(metric, value);
                sources.insert(
                    serde_json::to_value(metric)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string(),
                    field.to_string(),
                );
                break;
            }
        }
    }
    for (name, field) in [
        ("code", code.as_ref().map(|v| v.0.as_str())),
        ("exception", exception.as_ref().map(|v| v.0.as_str())),
        ("auth_reason", auth.as_ref().map(|v| v.0.as_str())),
    ] {
        if let Some(field) = field {
            sources.insert(name.into(), field.into());
        }
    }
    let mut facts = log_learning::extract_diagnostic(&record);
    if !record.documentation && record.failure && !record.success {
        if let Some((field, status)) = first(&["dependency.status_code", "upstream.status_code"]) {
            if status
                .parse::<u16>()
                .is_ok_and(|s| (500..=599).contains(&s))
            {
                facts.add(F::DependencyFailure, true);
                sources.insert("dependency_status".into(), field);
            }
        }
    }
    facts.sources = sources;
    serde_json::to_value(facts).unwrap_or(Value::Null)
}

#[derive(Default)]
struct Episode {
    id: String,
    context: String,
    namespace: String,
    source: String,
    service: String,
    operation: String,
    dependency: String,
    actor: String,
    first: i64,
    last: i64,
    summary: DiagnosticSummary,
    time_known: bool,
    untimed_signature: u64,
}
pub fn analyze(db: &Connection) -> Result<Value, String> {
    let population: u64 = db
        .query_row("SELECT count(*) FROM investigation_facts", [], |r| r.get(0))
        .map_err(error)?;
    crate::operations::report_progress(
        "comprometimentos",
        "diagnostics",
        "Comparando sintomas e explicações",
        0,
        population as usize,
        "registros",
        0,
    );
    db.execute_batch("CREATE TABLE IF NOT EXISTS diagnostic_rows(
        ref TEXT PRIMARY KEY,eid INTEGER,ns TEXT,ts INTEGER,source TEXT,context TEXT,service TEXT,operation TEXT,
        dependency TEXT,host TEXT,version TEXT,actor TEXT,src TEXT,trace TEXT,failure INTEGER,mask INTEGER,typed INTEGER,metrics TEXT,latency REAL,outcome INTEGER);
        CREATE TABLE IF NOT EXISTS diagnostic_episodes(id TEXT PRIMARY KEY,ns TEXT,context TEXT,source TEXT,first INTEGER,last INTEGER,priority INTEGER,payload TEXT);
        CREATE INDEX IF NOT EXISTS diagnostic_episode_page ON diagnostic_episodes(ns,priority DESC,id);
        CREATE TABLE IF NOT EXISTS diagnostic_members(id TEXT,ref TEXT,eid INTEGER,ts INTEGER,PRIMARY KEY(id,ref));
        CREATE TABLE IF NOT EXISTS diagnostic_queries(id TEXT PRIMARY KEY,episode TEXT,kind TEXT,payload TEXT);").map_err(error)?;
    let mut statement=db.prepare("SELECT ref,eid,ns,ts,source,host,actor,src,payload FROM investigation_facts ORDER BY ref").map_err(error)?;
    let mut rows = statement.query([]).map_err(error)?;
    let mut total = 0u64;
    let mut valid = 0u64;
    let mut timed = 0u64;
    let mut insert = db.prepare("INSERT INTO diagnostic_rows VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)").map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        check()?;
        total += 1;
        if total % 4096 == 0 {
            crate::operations::report_progress(
                "comprometimentos",
                "diagnostics",
                "Comparando sintomas e explicações",
                total as usize,
                population as usize,
                "registros",
                0,
            );
        }
        let payload: Value =
            serde_json::from_str(&row.get::<_, String>(8).map_err(error)?).map_err(error)?;
        if payload["parse_ok"] != true || payload["metadata"]["content_clipped"] == true {
            continue;
        }
        valid += 1;
        let bounded = |v: Option<&str>| {
            v.filter(|s| !s.is_empty() && s.len() <= 256)
                .unwrap_or("")
                .to_string()
        };
        let namespace: String = row.get(2).map_err(error)?;
        let source: String = row.get(4).map_err(error)?;
        let actor: String = row.get(6).map_err(error)?;
        let service = bounded(payload["metadata"]["service"].as_str());
        let operation = bounded(
            payload["metadata"]["learning"]["operation"]
                .as_str()
                .or(payload["action"].as_str()),
        );
        let dependency = bounded(payload["metadata"]["learning"]["dependency"].as_str());
        let version = bounded(payload["metadata"]["learning"]["version"].as_str());
        let trace = bounded(payload["metadata"]["learning"]["trace"].as_str());
        let mut facts: DiagnosticEventFacts = match payload["metadata"].get("diagnostics") {
            Some(value) => serde_json::from_value(value.clone()).map_err(error)?,
            None => DiagnosticEventFacts {
                failure: payload["outcome"] == "failure"
                    || payload["metadata"]["learning"]["failure"] == true,
                ..Default::default()
            },
        };
        facts.validate()?;
        if !version.is_empty() {
            facts.add(F::VersionMeasured, true);
        }
        if !trace.is_empty() {
            facts.add(F::TraceMeasured, true);
        }
        let authentication = facts.mask
            & (F::AuthenticationFailure.bit()
                | F::AuthenticationSuccess.bit()
                | F::CredentialExpired.bit()
                | F::CredentialInvalid.bit()
                | F::AccessDenied.bit())
            != 0;
        let context = json!([
            namespace,
            if service.is_empty() {
                &source
            } else {
                &service
            },
            operation,
            dependency,
            if authentication { actor.as_str() } else { "" }
        ])
        .to_string();
        let timestamp = if payload["metadata"]["time"]["ambiguity"].is_null() {
            row.get::<_, Option<i64>>(3).map_err(error)?
        } else {
            None
        };
        timed += u64::from(timestamp.is_some());
        insert
            .execute(params![
                row.get::<_, String>(0).map_err(error)?,
                row.get::<_, i64>(1).map_err(error)?,
                namespace,
                timestamp,
                source,
                context,
                service,
                operation,
                dependency,
                bounded(Some(row.get::<_, String>(5).map_err(error)?.as_str())),
                version,
                actor,
                row.get::<_, String>(7).map_err(error)?,
                trace,
                facts.failure,
                facts.mask as i64,
                facts.typed as i64,
                serde_json::to_string(&facts.metrics).map_err(error)?,
                payload["latency_ms"].as_f64(),
                match payload["outcome"].as_str() {
                    Some("success") => 1,
                    Some("failure") => 2,
                    _ => 0,
                }
            ])
            .map_err(error)?;
    }
    drop(rows);
    drop(statement);
    drop(insert);
    db.execute_batch(
        "CREATE INDEX IF NOT EXISTS diagnostic_context_time ON diagnostic_rows(context,ts,ref);
        CREATE INDEX IF NOT EXISTS diagnostic_service_time ON diagnostic_rows(ns,service,ts);
        CREATE INDEX IF NOT EXISTS diagnostic_member_ref ON diagnostic_members(ref,id);",
    )
    .map_err(error)?;
    let has_learning: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='learning_flags')",
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    if has_learning {
        db.execute(&format!("UPDATE diagnostic_rows SET mask=mask|{} WHERE ref IN(SELECT ref FROM learning_flags)",F::LearnedDeviation.bit()),[]).map_err(error)?;
        db.execute(&format!("UPDATE diagnostic_rows SET mask=mask|{} WHERE ref IN(SELECT ref FROM learning_flags WHERE kinds LIKE '%duration_change%' OR kinds LIKE '%temporal_change%')",F::LatencyChange.bit()),[]).map_err(error)?;
        db.execute(&format!("UPDATE diagnostic_rows SET mask=mask|{} WHERE ref IN(SELECT ref FROM learning_flags WHERE kinds LIKE '%failure_rate_change%')",F::FailureChange.bit()),[]).map_err(error)?;
    }
    let report = build_episodes(db, total, valid, timed)?;
    let has_incidents: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='learning_incidents')",
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    if has_incidents {
        db.execute("UPDATE learning_incidents SET payload=json_set(payload,'$.measurements.diagnostic_episode_ids',json('[]'))",[]).map_err(error)?;
        let mut statement=db.prepare("SELECT DISTINCT l.id,d.id FROM learning_incidents l JOIN investigation_members m ON m.id=l.id JOIN diagnostic_members dm ON dm.ref=m.ref JOIN diagnostic_episodes d ON d.id=dm.id ORDER BY l.id,d.priority DESC,d.id").map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let mut current = String::new();
        let mut linked = 0;
        while let Some(row) = rows.next().map_err(error)? {
            check()?;
            let id: String = row.get(0).map_err(error)?;
            if current != id {
                current = id.clone();
                linked = 0;
            }
            if linked >= 3 {
                continue;
            }
            let diagnostic: String = row.get(1).map_err(error)?;
            db.execute("UPDATE learning_incidents SET payload=json_insert(payload,'$.measurements.diagnostic_episode_ids[#]',?1) WHERE id=?2",params![diagnostic,id]).map_err(error)?;
            linked += 1;
        }
    }
    Ok(report)
}

fn build_episodes(db: &Connection, total: u64, valid: u64, timed: u64) -> Result<Value, String> {
    let knowledge = crate::diagnostic_knowledge::load();
    let knowledge_error = knowledge.as_ref().err().cloned();
    let knowledge = knowledge.or_else(|_| log_learning::DiagnosticKnowledge::baseline())?;
    let memory = crate::diagnostic_memory::snapshot();
    let memory_error = memory.as_ref().err().cloned();
    let memory = memory.ok();
    let mut statement=db.prepare("SELECT ref,eid,ns,ts,source,context,service,operation,dependency,actor,failure,mask,typed FROM diagnostic_rows ORDER BY context,ts IS NULL,CASE WHEN ts IS NULL THEN mask ELSE 0 END,ts,ref").map_err(error)?;
    let mut rows = statement.query([]).map_err(error)?;
    let mut active = Episode::default();
    let mut episodes = 0;
    let mut limited = 0;
    let mut abstentions = 0;
    let contextual = F::VersionMeasured.bit()
        | F::TraceMeasured.bit()
        | F::PoolMeasured.bit()
        | F::PoolAtLimit.bit()
        | F::PoolQueued.bit()
        | F::PoolHasCapacity.bit()
        | F::QueryMeasured.bit()
        | F::AcquisitionMeasured.bit()
        | F::DependencyMeasured.bit()
        | F::DeployObserved.bit()
        | F::AuthenticationSuccess.bit();
    while let Some(row) = rows.next().map_err(error)? {
        check()?;
        let time: Option<i64> = row.get(3).map_err(error)?;
        let ts = time.unwrap_or(0);
        let context: String = row.get(5).map_err(error)?;
        let mask = row.get::<_, i64>(11).map_err(error)? as u64;
        let failure: bool = row.get(10).map_err(error)?;
        let trigger = mask & !contextual != 0 || failure;
        if !trigger {
            continue;
        }
        if active.summary.events > 0
            && (active.context != context
                || active.time_known != time.is_some()
                || if time.is_some() {
                    ts.saturating_sub(active.last) > EPISODE_GAP_MS
                        || ts.saturating_sub(active.first) > MAX_EPISODE_MS
                } else {
                    active.untimed_signature != mask
                })
        {
            abstentions += u64::from(finish_episode(
                db,
                &mut active,
                memory.as_ref(),
                &knowledge,
            )?);
            episodes += 1;
            active = Episode::default();
        }
        if active.summary.events == 0 {
            if episodes >= MAX_EPISODES {
                limited += 1;
                continue;
            }
            let reference: String = row.get(0).map_err(error)?;
            active = Episode {
                id: format!(
                    "diagnostic:{}",
                    log_learning::digest(
                        json!([DIAGNOSTIC_VERSION, context, reference, time])
                            .to_string()
                            .as_bytes()
                    )
                ),
                context,
                namespace: row.get(2).map_err(error)?,
                source: row.get(4).map_err(error)?,
                service: row.get(6).map_err(error)?,
                operation: row.get(7).map_err(error)?,
                dependency: row.get(8).map_err(error)?,
                actor: row.get(9).map_err(error)?,
                first: ts,
                last: ts,
                summary: DiagnosticSummary::default(),
                time_known: time.is_some(),
                untimed_signature: mask,
            };
        }
        active.last = ts;
        let reference = DiagnosticReference {
            event_ref: row.get(0).map_err(error)?,
            event_id: row.get(1).map_err(error)?,
            timestamp: time,
        };
        active.summary.observe(
            reference.clone(),
            mask,
            row.get::<_, i64>(12).map_err(error)? as u64,
            failure,
        );
        db.execute(
            "INSERT INTO diagnostic_members VALUES(?1,?2,?3,?4)",
            params![active.id, reference.event_ref, reference.event_id, time],
        )
        .map_err(error)?;
    }
    if active.summary.events > 0 {
        abstentions += u64::from(finish_episode(
            db,
            &mut active,
            memory.as_ref(),
            &knowledge,
        )?);
        episodes += 1;
    }
    Ok(
        json!({"enabled":true,"available":true,"version":DIAGNOSTIC_VERSION,"total":total,"valid":valid,"timed":timed,"untimed":valid-timed,"invalid_records":total-valid,"episodes":episodes,"abstentions":abstentions,"limited_events":limited,"coverage":if limited>0{"partial"}else{"valid_records; unknown time never establishes chronology"},"families":["database","dependency","dns_tls","deployment","authentication"],"meaning":"Hipóteses verificáveis e pontuação ordinal de suporte; não são probabilidade de causa ou evidência de comprometimento.","accuracy_measured":false,"memory_revision":memory.as_ref().map(|m|m.revision),"memory_error":memory_error,"learned_ranking_active":memory.as_ref().is_some_and(|m|m.model.active),"knowledge_revision":knowledge.revision(),"knowledge_error":knowledge_error,"shared_ranking_active":knowledge.ranker.active,"shared_evaluation":knowledge.evaluation}),
    )
}

fn finish_episode(
    db: &Connection,
    episode: &mut Episode,
    memory: Option<&crate::diagnostic_memory::Snapshot>,
    knowledge: &log_learning::DiagnosticKnowledge,
) -> Result<bool, String> {
    // Resource snapshots often live in ordinary metric records rather than in
    // error messages. Admit bounded contextual measurements on the same known
    // host, within five seconds of a member; a trace, when present on both,
    // must agree. This is contextual corroboration, never a causal edge.
    let mut contextual_samples_limited = false;
    if episode.time_known {
        let metric_mask = F::PoolMeasured.bit()
            | F::PoolAtLimit.bit()
            | F::PoolQueued.bit()
            | F::PoolHasCapacity.bit()
            | F::AcquisitionMeasured.bit()
            | F::QueryMeasured.bit()
            | F::DependencyMeasured.bit()
            | F::RequestRateIncrease.bit()
            | F::RetryIncrease.bit();
        let mut statement=db.prepare("SELECT r.ref,r.eid,r.ts,r.mask,r.typed FROM diagnostic_rows r WHERE r.context=?1 AND r.ts BETWEEN ?2 AND ?3 AND (r.mask & ?4)<>0 AND NOT EXISTS(SELECT 1 FROM diagnostic_members m WHERE m.id=?5 AND m.ref=r.ref) AND EXISTS(SELECT 1 FROM diagnostic_members m JOIN diagnostic_rows f ON f.ref=m.ref WHERE m.id=?5 AND f.host<>'' AND f.host=r.host AND abs(r.ts-f.ts)<=5000 AND (r.trace='' OR f.trace='' OR r.trace=f.trace)) ORDER BY r.ts,r.ref LIMIT 257").map_err(error)?;
        let mut rows = statement
            .query(params![
                episode.context,
                episode.first,
                episode.last,
                metric_mask as i64,
                episode.id
            ])
            .map_err(error)?;
        let mut samples = Vec::new();
        while let Some(row) = rows.next().map_err(error)? {
            check()?;
            if samples.len() == 256 {
                contextual_samples_limited = true;
                break;
            }
            let reference = DiagnosticReference {
                event_ref: row.get(0).map_err(error)?,
                event_id: row.get(1).map_err(error)?,
                timestamp: row.get(2).map_err(error)?,
            };
            samples.push((
                reference,
                row.get::<_, i64>(3).map_err(error)? as u64 & metric_mask,
                row.get::<_, i64>(4).map_err(error)? as u64 & metric_mask,
            ));
        }
        // Freeze the sample set before inserting it: a metric snapshot must not
        // recruit another snapshot five seconds away into a transitive chain.
        drop(rows);
        drop(statement);
        for (reference, mask, typed) in samples {
            episode
                .summary
                .observe(reference.clone(), mask, typed, false);
            db.execute(
                "INSERT INTO diagnostic_members VALUES(?1,?2,?3,?4)",
                params![
                    episode.id,
                    reference.event_ref,
                    reference.event_id,
                    reference.timestamp
                ],
            )
            .map_err(error)?;
        }
    }
    let comparisons = version_comparisons(db, episode)?;
    if comparisons
        .iter()
        .any(|v| v["status"] == "supports_association")
    {
        let refs = episode
            .summary
            .features
            .values()
            .flat_map(|s| s.event_refs.iter())
            .take(1)
            .cloned()
            .collect::<Vec<_>>();
        episode.summary.features.insert(
            F::VersionAssociation,
            log_learning::DiagnosticSupport {
                observations: 1,
                typed_observations: 1,
                event_refs: refs,
            },
        );
        if !episode.service.is_empty() {
            let mut deploy=db.prepare("SELECT ref,eid,ts FROM diagnostic_rows WHERE ns=?1 AND service=?2 AND ts BETWEEN ?3 AND ?4 AND (mask & ?5)<>0 ORDER BY ts DESC,ref LIMIT 8").map_err(error)?;
            let refs = deploy
                .query_map(
                    params![
                        episode.namespace,
                        episode.service,
                        episode.first.saturating_sub(1_800_000),
                        episode.first,
                        F::DeployObserved.bit() as i64
                    ],
                    |r| {
                        Ok(DiagnosticReference {
                            event_ref: r.get(0)?,
                            event_id: r.get(1)?,
                            timestamp: r.get(2)?,
                        })
                    },
                )
                .map_err(error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(error)?;
            if !refs.is_empty() {
                episode.summary.features.insert(
                    F::DeployObserved,
                    log_learning::DiagnosticSupport {
                        observations: refs.len() as u64,
                        typed_observations: refs.len() as u64,
                        event_refs: refs,
                    },
                );
            }
        }
    }
    let mut assessment = log_learning::assess_diagnostic(&episode.summary, &knowledge.rules);
    if let Some(memory) = memory.filter(|m| m.model.active) {
        memory.model.apply(&episode.summary, &mut assessment);
    } else {
        knowledge.ranker.apply(&episode.summary, &mut assessment);
    }
    let precedents = match memory {
        Some(memory) => crate::diagnostic_memory::precedents(
            memory,
            &episode.context,
            episode.summary.mask(),
            &episode.id,
        )?,
        None => Vec::new(),
    };
    let (records,known_outcomes,failures,known_traces,failed_traces):(u64,u64,u64,u64,u64)=db.query_row("SELECT count(*),coalesce(sum(outcome IN(1,2)),0),coalesce(sum(outcome=2),0),count(DISTINCT nullif(trace,'')),count(DISTINCT CASE WHEN outcome=2 THEN nullif(trace,'') END) FROM diagnostic_rows WHERE context=?1 AND ((?2 IS NULL AND ref IN(SELECT ref FROM diagnostic_members WHERE id=?4)) OR ts BETWEEN ?2 AND ?3)",params![episode.context,episode.time_known.then_some(episode.first),episode.time_known.then_some(episode.last),episode.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(error)?;
    let priority = (assessment
        .hypotheses
        .iter()
        .map(|h| h.score)
        .max()
        .unwrap_or(20)
        + if failed_traces >= 5 {
            ((failed_traces as f64).log2() as u16 * 2).min(10)
        } else {
            0
        })
    .min(85);
    let impact = json!({"records":records,"known_outcomes":known_outcomes,"unknown_outcomes":records-known_outcomes,"failure_records":failures,"failure_record_rate":(known_outcomes>0).then(||failures as f64/known_outcomes as f64),"known_traces":known_traces,"failed_traces":failed_traces,"priority_policy":"Maximum ordinal support plus up to 10 points for distinct explicitly linked failed traces; no boost from repeated log lines alone","meaning":"Contagens de registros e IDs observados; não estimam usuários, operações ou impacto não coberto pela coleta."});
    let mut queries = Vec::new();
    for kind in &assessment.next_queries {
        let kind_text = serde_json::to_string(kind).map_err(error)?;
        let id = format!(
            "diagnostic-query:{}",
            log_learning::digest(json!([episode.id, kind]).to_string().as_bytes())
        );
        let query = json!({"id":id,"kind":kind,"title":kind.title(),"interpretation":kind.interpretation(),"section":"diagnostic_query","signal_id":id,"read_only":true});
        db.execute(
            "INSERT INTO diagnostic_queries VALUES(?1,?2,?3,?4)",
            params![id, episode.id, kind_text, query.to_string()],
        )
        .map_err(error)?;
        queries.push(query);
    }
    let payload = json!({"id":episode.id,"kind":"diagnostic","namespace":episode.namespace,"value":if episode.service.is_empty(){&episode.source}else{&episode.service},"source":episode.source,"first":episode.time_known.then_some(episode.first),"last":episode.time_known.then_some(episode.last),"time_known":episode.time_known,"priority":priority,"evidence_level":0,
        "scope":{"context":episode.context,"service":episode.service,"operation":episode.operation,"dependency":episode.dependency,"actor":episode.actor},
        "supporting_events":episode.summary.events,"failure_events":episode.summary.failure_events,"assessment":assessment,"summary":episode.summary,"comparisons":comparisons,"queries":queries,"precedents":precedents,"impact":impact,"contextual_measurements":{"limited":contextual_samples_limited,"meaning":"Snapshots do mesmo host a até cinco segundos de um evento de apoio; trace deve concordar quando conhecido. Proximidade não demonstra causa e capacidade livre pode ser intermitente."}});
    db.execute(
        "INSERT INTO diagnostic_episodes VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            episode.id,
            episode.namespace,
            episode.context,
            episode.source,
            episode.time_known.then_some(episode.first),
            episode.time_known.then_some(episode.last),
            priority,
            payload.to_string()
        ],
    )
    .map_err(error)?;
    Ok(assessment.abstention.is_some())
}

fn version_comparisons(db: &Connection, episode: &Episode) -> Result<Vec<Value>, String> {
    if !episode.time_known {
        return Ok(Vec::new());
    }
    // Require simultaneous known versions on known common hosts. A version
    // seen only before/after another version is confounded with time.
    let sql="WITH comparable_strata AS (SELECT host,(ts/60000-CASE WHEN ts<0 AND ts%60000<>0 THEN 1 ELSE 0 END) minute FROM diagnostic_rows WHERE context=?1 AND ts BETWEEN ?2 AND ?3 AND host<>'' AND version<>'' AND outcome IN(1,2) GROUP BY host,minute HAVING count(DISTINCT version)>=2)
      SELECT version,count(*),sum(outcome=2),count(DISTINCT (ts/60000-CASE WHEN ts<0 AND ts%60000<>0 THEN 1 ELSE 0 END)) FROM diagnostic_rows r WHERE context=?1 AND ts BETWEEN ?2 AND ?3 AND version<>'' AND outcome IN(1,2) AND EXISTS(SELECT 1 FROM comparable_strata s WHERE s.host=r.host AND s.minute=(r.ts/60000-CASE WHEN r.ts<0 AND r.ts%60000<>0 THEN 1 ELSE 0 END)) GROUP BY version ORDER BY count(*) DESC,version LIMIT 16";
    let mut statement = db.prepare(sql).map_err(error)?;
    let groups = statement
        .query_map(params![episode.context, episode.first, episode.last], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, u64>(3)?,
            ))
        })
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    let total = groups.iter().map(|g| g.1).sum::<u64>();
    let failures = groups.iter().map(|g| g.2).sum::<u64>();
    let mut result = Vec::new();
    for (version, n, k, minutes) in &groups {
        let rest = total - n;
        let rest_failures = failures - k;
        let a = log_learning::rate_interval_with_z(*k, *n, 3.65);
        let b = log_learning::rate_interval_with_z(rest_failures, rest, 3.65);
        let supported = *n >= 20 && *k >= 3 && rest >= 20 && *minutes >= 2 && a.0 > b.1;
        result.push(json!({"version":version,"observations":n,"failures":k,"interval":a,"comparison_observations":rest,"comparison_failures":rest_failures,"comparison_interval":b,"minute_windows":minutes,"status":if supported{"supports_association"}else{"inconclusive"},"meaning":"Associação exploratória entre versões conhecidas em hosts comuns. Tempo, operação e dependência delimitados; mudanças de carga e distribuição ainda exigem confirmação."}));
    }
    Ok(result)
}

fn scoped_episode(
    db: &Connection,
    id: &str,
    namespace: Option<&str>,
    entity: Option<&str>,
) -> Result<Value, String> {
    let text:Option<String>=db.query_row("SELECT payload FROM diagnostic_episodes WHERE id=?1 AND (?2 IS NULL OR ns=?2) AND (?3 IS NULL OR source=?3 OR json_extract(payload,'$.value')=?3 OR json_extract(payload,'$.scope.actor')=?3)",params![id,namespace,entity],|r|r.get(0)).optional().map_err(error)?;
    serde_json::from_str(&text.ok_or("Episódio diagnóstico não pertence a este recorte")?)
        .map_err(error)
}
pub fn episode(db: &Connection, id: &str) -> Result<Value, String> {
    scoped_episode(db, id, None, None)
}

pub fn page(
    db: &Connection,
    section: &str,
    namespace: Option<&str>,
    entity: Option<&str>,
    signal: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<Value, String> {
    if !(1..=100).contains(&limit) || offset > i64::MAX as usize {
        return Err("Página diagnóstica fora do orçamento".into());
    }
    let available: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='diagnostic_episodes')",
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    if !available {
        return Ok(
            json!({"items":[],"total":0,"offset":offset,"next_offset":Value::Null,"available":false,"meaning":"Recalcule a análise para gerar diagnósticos nesta versão."}),
        );
    }
    if section == "diagnostics" {
        let condition="(?1 IS NULL OR ns=?1) AND (?2 IS NULL OR source=?2 OR json_extract(payload,'$.value')=?2 OR json_extract(payload,'$.scope.actor')=?2) AND (?3 IS NULL OR id=?3)";
        let total: u64 = db
            .query_row(
                &format!("SELECT count(*) FROM diagnostic_episodes WHERE {condition}"),
                params![namespace, entity, signal],
                |r| r.get(0),
            )
            .map_err(error)?;
        let mut statement=db.prepare(&format!("SELECT payload FROM diagnostic_episodes WHERE {condition} ORDER BY priority DESC,first,id LIMIT ?4 OFFSET ?5")).map_err(error)?;
        let items = statement
            .query_map(
                params![namespace, entity, signal, limit as i64, offset as i64],
                |r| r.get::<_, String>(0),
            )
            .map_err(error)?
            .map(|r| serde_json::from_str::<Value>(&r.map_err(error)?).map_err(error))
            .collect::<Result<Vec<_>, _>>()?;
        let next = offset.saturating_add(items.len());
        return Ok(
            json!({"items":items,"total":total,"offset":offset,"next_offset":((next as u64)<total).then_some(next)}),
        );
    }
    if section == "diagnostic_members" {
        let id = signal.ok_or("Escolha o episódio diagnóstico")?;
        let episode = scoped_episode(db, id, namespace, entity)?;
        let total: u64 = db
            .query_row(
                "SELECT count(*) FROM diagnostic_members WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .map_err(error)?;
        let mut statement=db.prepare("SELECT ref,eid,ts FROM diagnostic_members WHERE id=?1 ORDER BY ts,ref LIMIT ?2 OFFSET ?3").map_err(error)?;
        let items=statement.query_map(params![id,limit as i64,offset as i64],|r|Ok(json!({"event_ref":r.get::<_,String>(0)?,"event_id":r.get::<_,i64>(1)?,"timestamp":r.get::<_,Option<i64>>(2)?}))).map_err(error)?.collect::<Result<Vec<_>,_>>().map_err(error)?;
        let next = offset.saturating_add(items.len());
        return Ok(
            json!({"items":items,"total":total,"offset":offset,"next_offset":((next as u64)<total).then_some(next),"episode":episode["id"]}),
        );
    }
    if section != "diagnostic_query" {
        return Err("Seção diagnóstica desconhecida".into());
    }
    let id = signal.ok_or("Escolha uma consulta do diagnóstico")?;
    let (episode_id, kind, text): (String, String, String) = db
        .query_row(
            "SELECT episode,kind,payload FROM diagnostic_queries WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "Consulta diagnóstica não pertence a esta análise")?;
    let episode = scoped_episode(db, &episode_id, namespace, entity)?;
    let kind: DiagnosticQuery = serde_json::from_str(&kind).map_err(error)?;
    let query: Value = serde_json::from_str(&text).map_err(error)?;
    let context = episode["scope"]["context"]
        .as_str()
        .ok_or("Episódio sem contexto")?;
    let first = episode["first"].as_i64();
    let last = episode["last"].as_i64();
    let before = first.zip(last).map(|(first, last)| {
        first.saturating_sub(last.saturating_sub(first).max(60_000).min(1_800_000))
    });
    let time_condition = "((?2 IS NULL AND ts IS NULL) OR ts BETWEEN ?2 AND ?3)";
    let filter = match kind {
        DiagnosticQuery::Pool => format!(
            "(mask & {})<>0",
            F::PoolMeasured.bit()
                | F::PoolQueued.bit()
                | F::AcquireTimeout.bit()
                | F::PoolExhaustedReported.bit()
                | F::AcquisitionMeasured.bit()
                | F::QueryMeasured.bit()
        ),
        DiagnosticQuery::Database => format!(
            "(mask & {})<>0",
            F::QueryMeasured.bit()
                | F::QuerySlow.bit()
                | F::Deadlock.bit()
                | F::LockWait.bit()
                | F::ConnectionLimit.bit()
        ),
        DiagnosticQuery::Dependency => format!(
            "(mask & {})<>0 OR dependency<>''",
            F::ConnectTimeout.bit()
                | F::ConnectionFailure.bit()
                | F::ReadTimeout.bit()
                | F::ConnectionRefused.bit()
                | F::DependencyMeasured.bit()
                | F::DependencyFailure.bit()
        ),
        DiagnosticQuery::DnsTls => format!(
            "(mask & {})<>0",
            F::DnsFailure.bit()
                | F::TlsExpired.bit()
                | F::TlsHostname.bit()
                | F::TlsUntrusted.bit()
        ),
        DiagnosticQuery::Versions | DiagnosticQuery::Authentication | DiagnosticQuery::Overview => {
            "1=1".into()
        }
    };
    let total:u64=db.query_row(&format!("SELECT count(*) FROM diagnostic_rows WHERE context=?1 AND {time_condition} AND ({filter})"),params![context,before,last],|r|r.get(0)).map_err(error)?;
    let mut statement=db.prepare(&format!("SELECT ref,eid,ts,source,host,version,dependency,actor,src,trace,failure,mask,metrics,outcome FROM diagnostic_rows WHERE context=?1 AND {time_condition} AND ({filter}) ORDER BY ts,ref LIMIT ?4 OFFSET ?5")).map_err(error)?;
    let mut rows = statement
        .query(params![context, before, last, limit as i64, offset as i64])
        .map_err(error)?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(error)? {
        check()?;
        let ts: Option<i64> = row.get(2).map_err(error)?;
        let mask = row.get::<_, i64>(11).map_err(error)? as u64;
        let metrics: Value =
            serde_json::from_str(&row.get::<_, String>(12).map_err(error)?).map_err(error)?;
        let features = log_learning::DIAGNOSTIC_FEATURES
            .iter()
            .filter(|f| mask & f.bit() != 0)
            .map(|f| f.label())
            .collect::<Vec<_>>();
        items.push(json!({"event_ref":row.get::<_,String>(0).map_err(error)?,"event_id":row.get::<_,i64>(1).map_err(error)?,"timestamp":ts,"period":if ts.is_none(){"time_unknown"}else if ts<first{"before"}else{"during"},"source":row.get::<_,String>(3).map_err(error)?,"host":row.get::<_,String>(4).map_err(error)?,"version":row.get::<_,String>(5).map_err(error)?,"dependency":row.get::<_,String>(6).map_err(error)?,"actor":row.get::<_,String>(7).map_err(error)?,"origin":row.get::<_,String>(8).map_err(error)?,"trace":row.get::<_,String>(9).map_err(error)?,"reported_failure":row.get::<_,bool>(10).map_err(error)?,"outcome":match row.get::<_,i64>(13).map_err(error)?{1=>"success",2=>"failure",_=>"unknown"},"observations":features,"measurements":metrics}));
    }
    let mut overview = Vec::new();
    let periods = if first.is_some() {
        vec![
            ("before", before, first.map(|v| v.saturating_sub(1))),
            ("during", first, last),
        ]
    } else {
        vec![("time_unknown", None, None)]
    };
    for (label, start, end) in periods {
        let (n,known,failures,missing_version,missing_host,with_metrics):(u64,u64,u64,u64,u64,u64)=db.query_row(&format!("SELECT count(*),coalesce(sum(outcome IN(1,2)),0),coalesce(sum(outcome=2),0),coalesce(sum(version=''),0),coalesce(sum(host=''),0),coalesce(sum(metrics<>'{{}}'),0) FROM diagnostic_rows WHERE context=?1 AND {time_condition}"),params![context,start,end],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(error)?;
        overview.push(json!({"period":label,"first":start,"last":end,"records":n,"known_outcomes":known,"unknown_outcomes":n-known,"failures":failures,"failure_rate":(known>0).then(||failures as f64/known as f64),"unknown_versions":missing_version,"unknown_hosts":missing_host,"records_with_measurements":with_metrics}));
    }
    let next = offset.saturating_add(items.len());
    Ok(
        json!({"items":items,"total":total,"offset":offset,"next_offset":((next as u64)<total).then_some(next),"query":query,"overview":overview,"comparisons":episode["comparisons"],"scope":{"namespace":episode["namespace"],"context":episode["scope"],"first":before,"last":last},"unknown_policy":"Ausência nos logs não refuta uma hipótese sem cobertura explícita. As taxas usam somente resultados conhecidos; períodos e distribuições podem ser diferentes."}),
    )
}
