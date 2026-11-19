//! Exact scoped diagnostics. Row indices never leave the captured view without a
//! stable reference. Temporary SQLite tables keep high-cardinality IDs off heap.
use crate::{entities, journeys, model::Event, query::Filter, AppState};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::Manager;

#[derive(Serialize, Deserialize, Default)]
pub struct Report {
    total: usize,
    undated: usize,
    #[serde(default)]
    limited: bool,
    signals: Vec<Signal>,
    reviews: std::collections::BTreeMap<String, Value>,
}
#[derive(Serialize, Deserialize)]
struct Signal {
    kind: String,
    title: String,
    source: String,
    count: usize,
    population: usize,
    detail: String,
    observed: Vec<String>,
    reference: Vec<String>,
    measures: Vec<(String, f64)>,
    examples: Vec<Proof>,
    bins: Vec<(usize, usize)>,
    priority: u8,
    selector: Option<String>,
    cases: Vec<(String, Vec<Proof>)>,
    tests: Vec<Comparison>,
}
#[derive(Serialize, Deserialize)]
struct Comparison {
    field: String,
    value: String,
    hit: usize,
    group: usize,
    other_hit: usize,
    other: usize,
    unknown: usize,
    contrary: Option<Proof>,
}
#[derive(Serialize, Deserialize)]
struct Proof {
    event_ref: String,
    source: String,
    file: String,
    path: String,
    timestamp: Option<i64>,
    role: String,
    raw: String,
    clipped: bool,
    code: String,
    level: String,
    outcome: Option<String>,
}
fn new_signal(
    kind: &str,
    title: String,
    source: String,
    count: usize,
    population: usize,
    detail: String,
) -> Signal {
    let priority = match kind {
        "bridge" => 75,
        "steps" => 65,
        "mix" => 45,
        _ => 35,
    };
    Signal {
        kind: kind.into(),
        title,
        source,
        count,
        population,
        detail,
        observed: vec![],
        reference: vec![],
        measures: vec![],
        examples: vec![],
        bins: vec![],
        priority,
        selector: None,
        cases: vec![],
        tests: vec![],
    }
}
fn proof(view: &journeys::View<'_>, index: usize, role: &str) -> Result<Proof, String> {
    let ev = view.fetch(index)?;
    let raw = ev.raw.chars().take(750).collect::<String>();
    Ok(Proof {
        event_ref: crate::security_normalize::event_ref(&ev),
        file: ev.col_str("arquivo").unwrap_or_default(),
        path: ev.col_str("log.record.path").unwrap_or_default(),
        timestamp: ev.timestamp,
        role: role.into(),
        clipped: raw.len() < ev.raw.len(),
        raw,
        outcome: entities::action_outcome(&ev).1.map(str::to_owned),
        code: ev.code,
        level: ev.level,
        source: ev.source,
    })
}
fn text(ev: &Event, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| {
            ev.col_ref(k)
                .filter(|v| !v.is_empty() && v.len() <= 192)
                .map(|v| v.into_owned())
        })
        .unwrap_or_default()
}
fn identifier(key: &str) -> bool {
    let k = key.to_ascii_lowercase().replace(['.', '_', '-'], "");
    matches!(
        k.as_str(),
        "copy"
            | "copia"
            | "backup"
            | "job"
            | "tarefa"
            | "pedido"
            | "order"
            | "batch"
            | "lote"
            | "run"
            | "copyid"
            | "copiaid"
            | "backupid"
            | "taskid"
            | "jobid"
            | "orderid"
            | "pedidoid"
            | "batchid"
            | "loteid"
            | "runid"
            | "traceid"
            | "requestid"
            | "operationid"
            | "correlationid"
            | "transactionid"
            | "workflowid"
            | "executionid"
    )
}
fn projected_outcome(ev: &Event) -> Option<bool> {
    match entities::action_outcome(ev).1 {
        Some("success") => Some(false),
        Some("blocked" | "failure") => Some(true),
        _ => None,
    }
}
fn failure(e: rusqlite::Error) -> String {
    e.to_string()
}
fn analysis(view: &journeys::View<'_>) -> Result<Report, String> {
    scoped_analysis(view, None, 0)
}
fn scoped_analysis(
    view: &journeys::View<'_>,
    selector: Option<&str>,
    offset: usize,
) -> Result<Report, String> {
    let db = journeys::db()?;
    db.execute_batch("CREATE TABLE f(i INTEGER PRIMARY KEY,src TEXT,ns TEXT,host TEXT,ts INTEGER,stage TEXT,kind TEXT,failed INTEGER,family TEXT,critical INTEGER);
      CREATE TABLE ids(i INTEGER,k TEXT,v TEXT); CREATE TABLE attrs(i INTEGER,k TEXT,v TEXT);").map_err(failure)?;
    let mut report = Report::default();
    {
        let mut add = db
            .prepare("INSERT INTO f VALUES(?,?,?,?,?,?,?,?,?,?)")
            .map_err(failure)?;
        let mut add_id = db
            .prepare("INSERT INTO ids VALUES(?,?,?)")
            .map_err(failure)?;
        let mut attr = db
            .prepare("INSERT INTO attrs VALUES(?,?,?)")
            .map_err(failure)?;
        view.scan(|i, e| {
            report.total += 1;
            report.undated += usize::from(e.timestamp.is_none());
            if let (Some(choices), Some(path)) = (
                e.fields.get("log.batch.choices").and_then(Value::as_object),
                e.col_str("caminho"),
            ) {
                if report.reviews.contains_key(&path) || report.reviews.len() < 32 {
                    let merged = report
                        .reviews
                        .entry(path)
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(options) = merged.as_object_mut() {
                        for (k, v) in choices {
                            options.entry(k).or_insert_with(|| v.clone());
                        }
                    }
                } else {
                    report.limited = true
                }
            }
            let stage = text(
                e,
                &[
                    "event.action",
                    "stage",
                    "phase",
                    "step",
                    "etapa",
                    "event",
                    "state",
                    "estado",
                    "action",
                    "status",
                    "resultado",
                ],
            );
            let kind = text(
                e,
                &[
                    "operation.name",
                    "operation",
                    "event.dataset",
                    "http.route",
                    "type",
                    "tipo",
                    "category",
                    "kind",
                ],
            );
            let host = text(
                e,
                &["host.name", "host", "Computer", "computer", "hostname"],
            );
            let scope = [
                text(e, &["tenant.id", "tenant"]),
                text(e, &["service.namespace", "namespace"]),
                text(
                    e,
                    &[
                        "deployment.environment.name",
                        "deployment.environment",
                        "environment",
                    ],
                ),
            ];
            let ns = if scope.iter().all(String::is_empty) {
                String::new()
            } else {
                serde_json::to_string(&scope).map_err(|e| e.to_string())?
            };
            let source = if e.source.is_empty() {
                text(e, &["arquivo"])
            } else {
                e.source.clone()
            };
            let family = if !stage.is_empty() {
                stage.clone()
            } else if !e.message.trim().is_empty() {
                log_learning::tokens(&e.message).join(" ")
            } else {
                e.code.clone()
            };
            add.execute(params![
                i,
                source,
                ns,
                host,
                e.timestamp,
                stage,
                kind,
                projected_outcome(e),
                family,
                e.level == "Crítico"
            ])
            .map_err(failure)?;
            for (key, value) in e.fields.iter().filter(|(k, _)| identifier(k)) {
                let v = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                if v.is_empty() || v.len() > 192 {
                    continue;
                }
                add_id.execute(params![i, key, v]).map_err(failure)?;
            }
            for key in [
                "reenvio",
                "reenviado",
                "retry",
                "retried",
                "version",
                "service.version",
                "dependency",
                "dependency.name",
                "region",
            ] {
                if let Some(v) = e
                    .col_ref(key)
                    .filter(|v| !v.trim().is_empty() && v.len() <= 128)
                {
                    attr.execute(params![i, key, v.as_ref()]).map_err(failure)?;
                }
            }
            Ok(())
        })?;
    }
    if report.total == 0 {
        return Ok(report);
    }
    db.execute_batch("CREATE INDEX f_group ON f(ns,src,host,kind,stage); CREATE INDEX f_family ON f(ns,src,family);
        CREATE INDEX ids_key ON ids(k,v,i); CREATE INDEX ids_event ON ids(i,k); CREATE INDEX attr_row ON attrs(i,k);
        CREATE INDEX f_result ON f(failed,ts);
        CREATE TEMP TABLE scale AS SELECT min(ts) a,max(1.0,(max(ts)*1.0-min(ts)+1)/32) width FROM f;
        CREATE TEMP VIEW timed AS SELECT f.*,CAST((ts*1.0-a)/width AS INTEGER) bin FROM f,scale WHERE ts IS NOT NULL;").map_err(failure)?;
    variants(&db, view, &mut report, selector, offset)?;
    if selector.is_some() {
        return Ok(report);
    }
    durations(&db, view, &mut report)?;
    bridges(&db, view, &mut report)?;
    rare(&db, view, &mut report)?;
    mixture(&db, view, &mut report)?;
    report.signals.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(b.count.cmp(&a.count))
            .then(a.title.cmp(&b.title))
    });
    Ok(report)
}
fn durations(
    db: &Connection,
    view: &journeys::View<'_>,
    report: &mut Report,
) -> Result<(), String> {
    let mut q=db.prepare("WITH ordered AS(SELECT *,b-a elapsed,row_number() OVER(PARTITION BY ns,src,host,kind,k,seq ORDER BY b-a,v) rank,count(*) OVER(PARTITION BY ns,src,host,kind,k,seq) n FROM runs WHERE instr(seq,char(31))>0),
        ref AS(SELECT * FROM ordered WHERE n>=12 AND rank=(n+1)/2)
        SELECT o.src,o.k,o.v,o.seq,o.elapsed,r.elapsed,r.n,o.sample,r.sample FROM ordered o JOIN ref r USING(ns,src,host,kind,k,seq)
        JOIN ordered p ON p.ns=r.ns AND p.src=r.src AND p.host=r.host AND p.kind=r.kind AND p.k=r.k AND p.seq=r.seq AND p.rank=max(1,r.n*9/10)
        WHERE o.elapsed>=max(1,3*r.elapsed) AND o.elapsed>p.elapsed*1.5 ORDER BY o.elapsed DESC LIMIT 8").map_err(failure)?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, usize>(6)?,
                r.get::<_, usize>(7)?,
                r.get::<_, usize>(8)?,
            ))
        })
        .map_err(failure)?;
    for row in rows {
        let (src, k, v, seq, ms, med, n, i, reference) = row.map_err(failure)?;
        let steps: Vec<_> = seq.split(char::from(31)).collect();
        if steps.iter().skip(1).any(|s| s == &steps[0]) {
            continue;
        }
        let mut s=new_signal("duration",format!("Operação mais longa · {k}={v}"),src,1,n,"Comparação entre operações com a mesma sequência, origem, tipo e host. A duração supera três vezes a mediana e 1,5 vez o percentil 90.".into());
        s.priority = 62;
        s.measures = vec![("Observada (ms)".into(), ms), ("Mediana (ms)".into(), med)];
        for (idx, role) in [(i, "Operação"), (reference, "Referência")] {
            let mut ends=db.prepare("SELECT i FROM f WHERE i IN (
                SELECT f.i FROM ids x JOIN ids d ON x.k=d.k AND x.v=d.v JOIN f anchor ON anchor.i=x.i JOIN f ON f.i=d.i AND f.src=anchor.src AND f.ns=anchor.ns AND f.host=anchor.host AND f.kind=anchor.kind WHERE x.i=?1 AND x.k=?2 AND f.stage<>'') ORDER BY ts").map_err(failure)?;
            let ids = ends
                .query_map(params![idx, k], |r| r.get::<_, usize>(0))
                .map_err(failure)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failure)?;
            for j in ids.first().into_iter().chain(ids.last()) {
                s.examples.push(proof(view, *j, role)?)
            }
        }
        report.signals.push(s);
    }
    Ok(())
}
fn bins(
    db: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<(usize, usize)>, String> {
    let mut result = vec![(0, 0); 32];
    let mut stmt = db.prepare(sql).map_err(failure)?;
    for row in stmt
        .query_map(args, |r| Ok((r.get::<_, usize>(0)?, r.get(1)?, r.get(2)?)))
        .map_err(failure)?
    {
        let (i, n, v) = row.map_err(failure)?;
        if let Some(bin) = result.get_mut(i) {
            *bin = (n, v)
        }
    }
    Ok(result)
}
fn rare(db: &Connection, view: &journeys::View<'_>, report: &mut Report) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE source_bins AS SELECT ns,src,bin,count(*) n FROM timed WHERE family<>'' GROUP BY ns,src,bin;
        CREATE UNIQUE INDEX source_bin ON source_bins(ns,src,bin);").map_err(failure)?;
    let mut stmt=db.prepare("WITH groupset AS (
        SELECT ns,src,family,count(*) n,max(critical) critical,max(coalesce(failed,0)) bad,min(i) example
        FROM f WHERE family<>'' GROUP BY ns,src,family),
        ranked AS (SELECT *,sum(n) OVER w total,count(*) OVER w variants,max(n) OVER w common FROM groupset WINDOW w AS(PARTITION BY ns,src))
        SELECT src,family,n,total,example,critical,bad,ns FROM ranked WHERE n<=3 AND total>=20 AND (critical OR bad OR common>=total*.5) AND n*20<=total
        ORDER BY critical DESC,bad DESC,1.0*n/total,src,family LIMIT 17").map_err(failure)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, usize>(2)?,
                r.get::<_, usize>(3)?,
                r.get::<_, bool>(5)?,
                r.get::<_, bool>(6)?,
                r.get::<_, String>(7)?,
            ))
        })
        .map_err(failure)?;
    for (j, row) in rows.enumerate() {
        if j == 16 {
            report.limited = true;
            break;
        }
        let (src, value, n, total, critical, bad, ns) = row.map_err(failure)?;
        let mut s = new_signal(
            "rare",
            format!("Ocorrência rara · {value}"),
            src,
            n,
            total,
            "Contagem no recorte inteiro entre os registros dessa origem com família identificada."
                .into(),
        );
        if critical {
            s.priority = 95
        } else if bad {
            s.priority = 70
        }
        s.bins=bins(db,"WITH hits AS(SELECT bin,count(*) h FROM timed WHERE ns=?1 AND src=?2 AND family=?3 GROUP BY bin)
          SELECT s.bin,s.n,coalesce(h,0) FROM source_bins s LEFT JOIN hits USING(bin) WHERE s.ns=?1 AND s.src=?2",params![ns,s.source,value])?;
        let mut examples = db
            .prepare("SELECT i FROM f WHERE ns=?1 AND src=?2 AND family=?3 ORDER BY i LIMIT 3")
            .map_err(failure)?;
        for i in examples
            .query_map(params![ns, s.source, value], |r| r.get::<_, usize>(0))
            .map_err(failure)?
        {
            s.examples
                .push(proof(view, i.map_err(failure)?, "Ocorrência")?)
        }
        report.signals.push(s);
    }
    Ok(())
}
fn variants(
    db: &Connection,
    view: &journeys::View<'_>,
    report: &mut Report,
    selector: Option<&str>,
    offset: usize,
) -> Result<(), String> {
    // Tied clocks, repeated opening stages, and groups cut by either edge do
    // not establish a sequential reference. Retain repetitions in the middle.
    db.execute_batch("CREATE TEMP TABLE runs AS WITH steps AS (
      SELECT f.*,d.k,d.v,count(*) OVER w len,count(ts) OVER w timed
      FROM f JOIN ids d USING(i) WHERE stage<>'' AND instr(stage,char(31))=0
      WINDOW w AS(PARTITION BY ns,src,host,kind,k,v))
      SELECT ns,src,host,kind,k,v,group_concat(stage,char(31) ORDER BY ts,i) seq,min(ts) a,max(ts) b,min(i) sample
      FROM steps WHERE len BETWEEN 1 AND 32 AND len=timed GROUP BY ns,src,host,kind,k,v HAVING count(DISTINCT ts)=count(*);
      CREATE INDEX run_shape ON runs(ns,src,host,kind,k,seq);").map_err(failure)?;
    let mut stmt=db.prepare("WITH shapes AS (
      SELECT ns,src,host,kind,k,seq,count(*) n,max(b-a) dur,min(sample) example FROM runs GROUP BY ns,src,host,kind,k,seq),
      ranks AS (SELECT *,row_number() OVER(PARTITION BY ns,src,host,kind,k ORDER BY n DESC,seq) rank,
        sum(n) OVER(PARTITION BY ns,src,host,kind,k) total FROM shapes),
      bounds AS(SELECT ns,src,host,kind,min(ts) first,max(ts) last FROM f GROUP BY ns,src,host,kind)
      SELECT r.src,r.k,r.seq,base.seq,count(*),base.total,min(r.sample),base.example,base.n,r.ns,r.host,r.kind,base.dur,b.first,b.last
      FROM runs r JOIN ranks base USING(ns,src,host,kind,k) JOIN bounds b USING(ns,src,host,kind)
      WHERE rank=1 AND base.n>=5 AND base.n>=base.total*.7 AND r.seq<>base.seq AND r.a>b.first AND b.last-r.b>=base.dur
      AND instr(base.seq,char(31))>0
      GROUP BY r.ns,r.src,r.host,r.kind,r.k,r.seq,base.seq ORDER BY count(*) DESC,r.src,r.seq LIMIT 24").map_err(failure)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, usize>(4)?,
                r.get::<_, usize>(5)?,
                r.get::<_, usize>(7)?,
                r.get::<_, usize>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, String>(11)?,
                r.get::<_, i64>(12)?,
                r.get::<_, i64>(13)?,
                r.get::<_, i64>(14)?,
            ))
        })
        .map_err(failure)?;
    for row in rows {
        let (
            src,
            key,
            actual,
            base,
            n,
            total,
            ref_idx,
            support,
            ns,
            host,
            kind,
            duration,
            first,
            last,
        ) = row.map_err(failure)?;
        let signature = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&[&ns, &src, &host, &kind, &key, &actual, &base])
                    .map_err(|e| e.to_string())?
            )
        );
        if selector.is_some_and(|wanted| wanted != signature) {
            continue;
        }
        let observed: Vec<_> = actual.split(char::from(31)).map(str::to_owned).collect();
        let reference: Vec<_> = base.split(char::from(31)).map(str::to_owned).collect();
        if observed.first() != reference.first()
            || observed.iter().skip(1).any(|x| Some(x) == observed.first())
            || reference
                .iter()
                .skip(1)
                .any(|x| Some(x) == reference.first())
        {
            continue;
        }
        let absent = reference.iter().find(|step| !observed.contains(step));
        let title = absent.map_or_else(
            || "Sequência de etapas diferente".into(),
            |step| format!("Etapa sem registro · {step}"),
        );
        let mut s=new_signal("steps",title,src,n,total,format!("Operações agrupadas por {key}. A sequência de referência ocorreu em {support} de {total} execuções com horários distintos. A diferença se refere às etapas registradas."));
        s.reference = reference;
        s.observed = observed;
        s.selector = Some(signature);
        s.bins=bins(db,"SELECT cast((r.b-scale.a)*1.0/scale.width AS INTEGER) bucket,count(*),sum(seq=?6 AND r.a>?7 AND ?8-r.b>=?9) FROM runs r,scale WHERE ns=?1 AND src=?2 AND host=?3 AND kind=?4 AND k=?5 GROUP BY bucket",params![ns,s.source,host,kind,key,actual,first,last,duration])?;
        let originals = |example: usize, role: &str| -> Result<Vec<Proof>, String> {
            let mut proofs = Vec::new();
            let mut records=db.prepare("SELECT f.i FROM f JOIN ids d USING(i) JOIN f anchor ON anchor.i=?1 AND f.src=anchor.src AND f.ns=anchor.ns AND f.host=anchor.host AND f.kind=anchor.kind JOIN ids x ON x.i=anchor.i AND d.k=x.k AND d.v=x.v WHERE d.k=?2 AND f.stage<>'' ORDER BY f.ts,f.i LIMIT 32").map_err(failure)?;
            for i in records
                .query_map(params![example, key], |r| r.get::<_, usize>(0))
                .map_err(failure)?
            {
                proofs.push(proof(view, i.map_err(failure)?, role)?)
            }
            Ok(proofs)
        };
        s.examples = originals(ref_idx, "Referência")?;
        let mut samples=db.prepare("SELECT v,sample FROM runs WHERE ns=?1 AND src=?2 AND host=?3 AND kind=?4 AND k=?5 AND seq=?6 AND a>?7 AND ?8-b>=?9 ORDER BY a,v LIMIT 12 OFFSET ?10").map_err(failure)?;
        for row in samples
            .query_map(
                params![ns, s.source, host, kind, key, actual, first, last, duration, offset],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, usize>(1)?)),
            )
            .map_err(failure)?
        {
            let (value, i) = row.map_err(failure)?;
            s.cases
                .push((format!("{key}={value}"), originals(i, "Operação")?));
        }
        let mut comparisons=db.prepare("WITH ops AS(SELECT r.v id,r.sample,r.seq=?6 bad,attr.k field,min(attr.v) val
          FROM runs r JOIN ids d ON d.k=r.k AND d.v=r.v JOIN f ON f.i=d.i AND f.ns=r.ns AND f.src=r.src AND f.host=r.host AND f.kind=r.kind
          JOIN attrs attr ON attr.i=f.i WHERE r.ns=?1 AND r.src=?2 AND r.host=?3 AND r.kind=?4 AND r.k=?5 AND r.a>?7 AND ?8-r.b>=?9
          GROUP BY r.v,attr.k HAVING min(attr.v)=max(attr.v)),
          groupset AS(SELECT field,val,count(*) n,sum(bad) hit,min(sample) FILTER(WHERE NOT bad) contrary FROM ops GROUP BY field,val),
          rates AS(SELECT *,sum(n) OVER(PARTITION BY field) known,sum(hit) OVER(PARTITION BY field) hits FROM groupset)
          SELECT field,val,n,hit,known-n,hits-hit,known,contrary FROM rates WHERE hit>0 AND n>=3 AND known-n>=3
          ORDER BY 1.0*hit/n-1.0*(hits-hit)/(known-n) DESC,field,val LIMIT 3").map_err(failure)?;
        let eligible:usize=db.query_row("SELECT count(*) FROM runs WHERE ns=?1 AND src=?2 AND host=?3 AND kind=?4 AND k=?5 AND a>?6 AND ?7-b>=?8",params![ns,s.source,host,kind,key,first,last,duration],|r|r.get(0)).map_err(failure)?;
        for row in comparisons
            .query_map(
                params![ns, s.source, host, kind, key, actual, first, last, duration],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get::<_, usize>(6)?,
                        r.get::<_, Option<usize>>(7)?,
                    ))
                },
            )
            .map_err(failure)?
        {
            let (field, value, group, hit, other, other_hit, known, idx) = row.map_err(failure)?;
            s.tests.push(Comparison {
                field,
                value,
                group,
                hit,
                other,
                other_hit,
                unknown: eligible - known,
                contrary: idx.map(|i| proof(view, i, "Contraexemplo")).transpose()?,
            });
        }
        report.signals.push(s);
    }
    Ok(())
}
fn bridges(db: &Connection, view: &journeys::View<'_>, report: &mut Report) -> Result<(), String> {
    // Common identifiers (a shared batch, for example) cannot safely join
    // arbitrary sources by themselves. Bound the number of candidate edges.
    db.execute_batch("CREATE TEMP TABLE linking AS WITH keys AS (
      SELECT d.*,f.src,f.ns,f.ts,count(*) OVER(PARTITION BY f.ns,d.k,d.v) used FROM ids d JOIN f USING(i))
      SELECT * FROM keys WHERE used<=64 AND ts IS NOT NULL;
      CREATE INDEX lk_value ON linking(ns,k,v,ts,src); CREATE INDEX lk_event ON linking(i,k);").map_err(failure)?;
    let mut stmt=db.prepare("WITH transfer AS(
       SELECT b.i bridge,e.i last,a.k k1,a.v v1,b.k k2,b.v v2,e.src,
       (SELECT x.i FROM linking x WHERE x.k=a.k AND x.v=a.v AND x.ns=a.ns AND x.ts BETWEEN a.ts-3600000 AND a.ts AND x.src<>a.src ORDER BY x.ts DESC,x.i LIMIT 1) first,
       (SELECT count(*) FROM linking x WHERE x.k=a.k AND x.v=a.v AND x.ns=a.ns AND x.ts BETWEEN a.ts-3600000 AND a.ts AND x.src<>a.src) upstream
       FROM f e JOIN linking z ON z.i=e.i JOIN linking b ON b.ns=z.ns AND b.k=z.k AND b.v=z.v AND b.src<>z.src AND b.ts BETWEEN e.ts-3600000 AND e.ts
       JOIN linking a ON a.i=b.i AND a.k<>b.k WHERE e.failed=1),
       paths AS(SELECT *,sum(upstream) OVER(PARTITION BY last,k2,v2) candidates,row_number() OVER(PARTITION BY last,k2,v2 ORDER BY bridge,first,k1) rank FROM transfer WHERE first IS NOT NULL)
       SELECT first,bridge,last,k1,v1,k2,v2,src,candidates FROM paths WHERE rank=1 ORDER BY last,bridge LIMIT 17").map_err(failure)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, usize>(0)?,
                r.get::<_, usize>(1)?,
                r.get::<_, usize>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, usize>(8)?,
            ))
        })
        .map_err(failure)?;
    for (j, row) in rows.enumerate() {
        if j == 16 {
            report.limited = true;
            break;
        }
        let (first, bridge, last, k1, v1, k2, v2, src, candidates) = row.map_err(failure)?;
        let mut s=new_signal("bridge",format!("Falha vinculada a {k2}={v2}"),src,1,1,format!("O registro de passagem contém {k1}={v1} e {k2}={v2}. Os extremos compartilham identificadores na mesma janela de uma hora. Reutilização ou compartilhamento de IDs admite outros caminhos. O resultado pertence ao último registro."));
        if candidates > 1 {
            s.title = format!("Vínculo ambíguo · {k2}={v2}");
            s.detail.push_str(&format!(
                " Há {candidates} caminhos candidatos; este é um exemplo."
            ));
            s.priority = 48;
        }
        for (idx, role) in [
            (first, "Origem possível"),
            (bridge, "Passagem observada"),
            (last, "Falha observada"),
        ] {
            s.examples.push(proof(view, idx, role)?)
        }
        s.observed = vec![
            format!("{k1}={v1}"),
            "Passagem".into(),
            format!("{k2}={v2}"),
        ];
        report.signals.push(s);
    }
    Ok(())
}
fn mixture(db: &Connection, view: &journeys::View<'_>, report: &mut Report) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE load_bins AS WITH periods AS(
        SELECT ns,src,min(ts) origin,max(1,cast((max(ts)*1.0-min(ts)+12)/12 as integer)) width FROM f WHERE ts IS NOT NULL GROUP BY ns,src)
        SELECT ns,src,kind,cast((ts-origin)/width as integer) bin,origin,width,count(*) total,count(failed) known,
        sum(coalesce(failed,0)) bad,min(i) FILTER(WHERE failed) example FROM f JOIN periods USING(ns,src) WHERE ts IS NOT NULL GROUP BY ns,src,kind,bin;
        CREATE INDEX load_series ON load_bins(ns,src,kind,bin);").map_err(failure)?;
    let mut peaks=db.prepare("WITH comparable AS(SELECT c.*,sum(p.known) historical,sum(p.bad) old
        FROM load_bins c JOIN load_bins p ON p.ns=c.ns AND p.src=c.src AND p.kind=c.kind AND p.bin<c.bin
        WHERE c.bin>=3 AND c.kind<>'' GROUP BY c.ns,c.src,c.kind,c.bin HAVING sum(p.known)>=10),
        agg AS (SELECT ns,src,bin,origin,width,sum(known) n,sum(bad) y,sum(known*1.0*old/historical) expected,
        min(example) sample FROM comparable GROUP BY ns,src,bin HAVING count(*)>=2 AND n>=20 AND y>=3),
        ranked AS(SELECT a.*,(SELECT sum(total) FROM load_bins x WHERE x.src=a.src AND x.ns=a.ns AND x.bin=a.bin) population,
        row_number() OVER(PARTITION BY ns,src ORDER BY y DESC,bin) rank FROM agg a
        WHERE y>=2.0*(SELECT sum(bad)*1.0/a.bin FROM load_bins x WHERE x.src=a.src AND x.ns=a.ns AND x.bin<a.bin))
        SELECT ns,src,n,y,expected,sample,population,origin+bin*width,origin+(bin+1)*width FROM ranked WHERE rank=1 AND n>=population*.8 LIMIT 16").map_err(failure)?;
    let mut covered = std::collections::HashSet::new();
    for r in peaks
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, usize>(2)?,
                r.get::<_, usize>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, usize>(5)?,
                r.get::<_, usize>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })
        .map_err(failure)?
    {
        let (ns, src, n, y, expected, i, pop, a, b) = r.map_err(failure)?;
        covered.insert((ns.clone(), src.clone()));
        let mut s=new_signal("mix","Pico de falhas".into(),src,y,n,format!("Taxas anteriores por tipo de operação aplicadas à carga do intervalo {} até {} (fim exclusivo). Cobertura: {n} de {pop} registros. O restante é a diferença entre a contagem e a estimativa.",crate::model::ts_to_iso(a),crate::model::ts_to_iso(b)));
        s.priority = 60;
        s.measures = vec![
            ("Observadas".into(), y as f64),
            ("Esperadas pela carga".into(), expected),
            ("Diferença".into(), y as f64 - expected),
        ];
        s.bins=bins(db,"SELECT bin,count(*),sum(coalesce(failed,0)) FROM timed WHERE ns=?1 AND src=?2 GROUP BY bin",params![ns,s.source])?;
        s.examples.push(proof(view, i, "Falha durante o pico")?);
        report.signals.push(s);
    }
    let mut stmt=db.prepare("WITH bounds AS(SELECT ns,src,min(ts)+(max(ts)-min(ts))/2 mid FROM f WHERE ts IS NOT NULL GROUP BY ns,src),
       rates AS(SELECT ns,src,kind,count(*) FILTER(WHERE ts<mid AND failed IS NOT NULL) n0,
       sum(coalesce(failed,0)) FILTER(WHERE ts<mid) y0,count(*) FILTER(WHERE ts>=mid AND failed IS NOT NULL) n1,
       sum(coalesce(failed,0)) FILTER(WHERE ts>=mid) y1,count(*) FILTER(WHERE ts>=mid) all1,
       min(i) FILTER(WHERE ts>=mid AND failed=1) example,min(ts) FILTER(WHERE ts>=mid) start
       FROM f JOIN bounds USING(ns,src) WHERE ts IS NOT NULL GROUP BY ns,src,kind),
       usable AS(SELECT *,kind<>'' AND n0>=10 AND n1>0 ok FROM rates)
       SELECT src,sum(y1) FILTER(WHERE ok),sum(n1) FILTER(WHERE ok),sum(y0*1.0*n1/n0) FILTER(WHERE ok),
       ns,sum(all1),min(example) FILTER(WHERE ok),min(start)
       FROM usable GROUP BY ns,src HAVING sum(ok)>=2 AND sum(n1) FILTER(WHERE ok)>=20 AND sum(n1) FILTER(WHERE ok)>=.8*sum(all1)
       AND sum(y1) FILTER(WHERE ok)>=3 AND 1.0*sum(y1) FILTER(WHERE ok)/sum(n1) FILTER(WHERE ok)>1.3*sum(y0) FILTER(WHERE ok)/sum(n0) FILTER(WHERE ok)
       ORDER BY sum(y1) FILTER(WHERE ok) DESC LIMIT 16").map_err(failure)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, usize>(1)?,
                r.get::<_, usize>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, usize>(5)?,
                r.get::<_, usize>(6)?,
            ))
        })
        .map_err(failure)?;
    for row in rows {
        let (src, y, n, expected, ns, all, i) = row.map_err(failure)?;
        if covered.contains(&(ns.clone(), src.clone())) {
            continue;
        }
        let mut s=new_signal("mix","Alta na taxa de falhas".into(),src,y,n,format!("Duas metades do período da fonte. Taxas da primeira metade aplicadas aos volumes por operação da segunda. Cobertura: {n} de {all} registros recentes; somente grupos com ao menos dez resultados de referência."));
        s.measures = vec![
            ("Observadas".into(), y as f64),
            ("Esperadas pela carga".into(), expected),
            ("Diferença".into(), y as f64 - expected),
        ];
        s.bins=bins(db,"SELECT bin,count(*),sum(coalesce(failed,0)) FROM timed WHERE ns=?1 AND src=?2 GROUP BY bin",params![ns,s.source])?;
        s.examples.push(proof(view, i, "Falha no período recente")?);
        report.signals.push(s);
    }
    Ok(())
}
#[tauri::command]
pub async fn operational_report(
    filters: Vec<Filter>,
    selector: Option<String>,
    offset: Option<usize>,
    case_events: Option<Vec<Event>>,
    case_key: Option<String>,
    analysis_context: Option<crate::analysis_context::Identity>,
    source_generation: Option<u64>,
    app: tauri::AppHandle,
    operation_id: Option<String>,
) -> Result<Report, String> {
    let offset = offset.unwrap_or(0);
    if selector
        .as_ref()
        .is_some_and(|k| k.len() != 64 || !k.bytes().all(|c| c.is_ascii_hexdigit()))
        || (selector.is_none() && offset != 0)
        || offset > i64::MAX as usize
    {
        return Err("Seleção de operações inválida.".into());
    }
    let (admitted, events) = crate::analysis_runtime::capture_case_async(
        app.clone(),
        analysis_context,
        source_generation,
        case_events,
        case_key,
        operation_id.clone(),
        crate::global_scheduler::Priority::Normal,
    )
    .await?;
    crate::offload_case(operation_id, app.clone(), admitted, events, move |case| {
        let state = app.state::<AppState>();
        let events = case.as_deref();
        crate::workspace::validate(&filters)?;
        let key = format!(
            "operational-v2-{}-{offset}",
            selector.as_deref().unwrap_or("report")
        );
        crate::computed_cache::scope(state.inner(), &filters, events, &key, || {
            journeys::with_view(state.inner(), &filters, events, |v| {
                if let Some(k) = selector.as_deref() {
                    scoped_analysis(v, Some(k), offset)
                } else {
                    analysis(v)
                }
            })
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(id: usize, source: &str, ts: Option<i64>, fields: Value) -> Event {
        let mut e = Event::empty();
        e.id = id;
        e.event_ref = format!("ref:{id}");
        e.source = source.into();
        e.timestamp = ts;
        e.raw = fields.to_string();
        e.fields = fields.as_object().unwrap().clone();
        e
    }
    #[test]
    fn complete_scope_sequences_links_missing_times_and_mix() {
        let mut events = Vec::new();
        for run in 0..24 {
            for (pos, step) in ["start", "finish", "verify"].iter().enumerate() {
                if run == 7 && pos == 2 {
                    continue;
                }
                events.push(event(
                    events.len(),
                    "backup",
                    Some(1000 * run + pos as i64),
                    json!({"copy_id":run,"stage":step}),
                ));
            }
        }
        for i in 0..250 {
            events.push(event(
                events.len(),
                "estado",
                None,
                json!({"estado":if i==113{"divergente"}else{"normal"}}),
            ));
        }
        events.push(event(events.len(), "A", Some(600), json!({"order":"842"})));
        events.push(event(
            events.len(),
            "B",
            Some(601),
            json!({"order":"842","batch":"L93"}),
        ));
        events.push(event(
            events.len(),
            "C",
            Some(602),
            json!({"batch":"L93","state":"rejected"}),
        ));
        for i in 0..200 {
            let batch = if i < 100 { i < 10 } else { i < 190 };
            events.push(event(events.len(),"import",Some(i),json!({"type":if batch{"batch"}else{"lookup"},"result":if batch&&i%2==0{"failure"}else{"success"}})));
        }
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        let report = journeys::with_view(&state, &[], Some(&events), analysis).unwrap();
        let steps = report.signals.iter().find(|s| s.kind == "steps").unwrap();
        assert_eq!(
            (
                steps.count,
                steps.population,
                steps.reference.len(),
                steps.observed.len()
            ),
            (1, 24, 3, 2)
        );
        assert_eq!(steps.examples.len() + steps.cases[0].1.len(), 5);
        assert!(report.signals.iter().any(|s| s.kind == "rare"
            && s.count == 1
            && s.population == 250
            && s.examples[0].timestamp.is_none()));
        assert!(report
            .signals
            .iter()
            .any(|s| s.kind == "bridge" && s.examples.len() == 3));
        assert!(report
            .signals
            .iter()
            .any(|s| s.kind == "mix" && (s.measures[1].1 - s.measures[0].1).abs() < 2.0));
    }
    #[test]
    fn single_undated_outlier_in_eighty_thousand_records() {
        let events: Vec<_> = (0..80_000)
            .map(|i| {
                event(
                    i,
                    "replica",
                    None,
                    json!({"estado":if i==77_013{"divergente"}else{"sincronizado"}}),
                )
            })
            .collect();
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        let report = journeys::with_view(&state, &[], Some(&events), analysis).unwrap();
        let rare = report.signals.iter().find(|s| s.kind == "rare").unwrap();
        assert_eq!((rare.count, rare.population), (1, 80_000));
        assert_eq!(rare.examples[0].event_ref, "ref:77013");
    }
    #[test]
    fn pages_keep_distinct_operations_and_exclude_trailing_incomplete_run() {
        let mut events = Vec::new();
        for run in 0..81 {
            for (pos, stage) in ["start", "verify", "finish"].into_iter().enumerate() {
                if (run > 1 && run < 16 || run == 80) && pos == 1 {
                    continue;
                }
                events.push(event(
                    events.len(),
                    "backup",
                    Some(10 * run + pos as i64),
                    json!({"copy_id":run,"stage":stage}),
                ));
            }
        }
        let state = crate::regression_tests::state_for(crate::SourceData::None);
        let report = journeys::with_view(&state, &[], Some(&events), analysis).unwrap();
        let first = report.signals.iter().find(|s| s.kind == "steps").unwrap();
        assert_eq!((first.count, first.cases.len()), (14, 12));
        let second = journeys::with_view(&state, &[], Some(&events), |v| {
            scoped_analysis(v, first.selector.as_deref(), 12)
        })
        .unwrap();
        assert_eq!(second.signals.len(), 1);
        assert_eq!(
            second.signals[0]
                .cases
                .iter()
                .map(|c| c.0.as_str())
                .collect::<Vec<_>>(),
            ["copy_id=14", "copy_id=15"]
        );
    }
    #[test]
    fn native_codes_are_scoped_and_message_categories_survive() {
        let mut e = event(1, "Microsoft-Windows-Security-Auditing", None, json!({}));
        e.code = "4624".into();
        assert_eq!(projected_outcome(&e), Some(false));
        e.code = "4625".into();
        assert_eq!(projected_outcome(&e), Some(true));
        e.source = "Bateria".into();
        assert_eq!(projected_outcome(&e), None);
        assert_ne!(
            log_learning::tokens("id=111 estado=sincronizado"),
            log_learning::tokens("id=222 estado=divergente")
        );
        assert_eq!(
            log_learning::tokens("id=111 estado=ok"),
            log_learning::tokens("id=222 estado=ok")
        );
    }
}
