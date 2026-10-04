//! Opt-in frozen environmental reference. Training replaces one reviewed
//! snapshot atomically; observing an anomaly never teaches it to the reference.
use rusqlite::{params, Connection};
use serde_json::{json, Value};
pub const VERSION: &str = "environment-history-1";
fn error(e: impl std::fmt::Display) -> String {
    format!("Perfil histórico: {e}")
}
fn interpretation() -> String {
    crate::evidence::stable_id(
        "history-interpretation-1",
        [
            crate::analysis_runtime::fact_interpretation_signature(),
            serde_json::to_string(&crate::detections::load_settings().mappings).unwrap_or_default(),
        ],
    )
}
fn path(settings: &crate::investigation::Settings) -> Result<std::path::PathBuf, String> {
    let name = settings
        .profile
        .as_ref()
        .ok_or("Selecione um nome de perfil no Caso")?;
    let key = profile_key(&crate::analysis_runtime::fact_namespace(), name);
    let dir = crate::config_dir().join("environment-history");
    std::fs::create_dir_all(&dir).map_err(error)?;
    Ok(dir.join(format!("{key}.sqlite")))
}
pub(crate) fn profile_key(owner: &str, name: &str) -> String {
    crate::evidence::stable_id(
        VERSION,
        [
            owner.to_string(),
            name.to_string(),
            crate::evidence::NORMALIZATION_VERSION.into(),
        ],
    )
}
pub fn train(
    source: &Connection,
    settings: &crate::investigation::Settings,
    metadata: &Value,
) -> Result<Value, String> {
    let base = path(settings)?;
    let revision = crate::evidence::stable_id(
        "snapshot",
        [
            metadata["analysis_id"].to_string(),
            metadata["dataset_fingerprint"].to_string(),
            interpretation(),
        ],
    );
    let destination = base.with_file_name(format!(
        "{}-{revision}.sqlite",
        base.file_stem().unwrap().to_string_lossy()
    ));
    let mut manifest = train_at(&destination, source, metadata)?;
    manifest["revision"] = json!(revision);
    Ok(manifest)
}
fn train_at(
    path: &std::path::Path,
    source: &Connection,
    metadata: &Value,
) -> Result<Value, String> {
    if metadata["complete"] != true || metadata["investigation"]["complete"] != true {
        return Err("Somente uma análise completa pode fornecer uma referência".into());
    }
    if path.exists() {
        let existing =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(error)?;
        let payload: String = existing
            .query_row("SELECT payload FROM manifest", [], |r| r.get(0))
            .map_err(error)?;
        let manifest: Value = serde_json::from_str(&payload).map_err(error)?;
        if manifest["version"] == VERSION
            && manifest["interpretation"] == interpretation()
            && manifest["analysis_id"] == metadata["analysis_id"]
            && manifest["dataset_fingerprint"] == metadata["dataset_fingerprint"]
        {
            return Ok(manifest);
        }
        return Err("Revisão histórica imutável já existe com outro conteúdo".into());
    }
    let directory = path.parent().ok_or("Perfil sem diretório")?;
    let tmp = tempfile::Builder::new()
        .prefix("history-")
        .suffix(".sqlite")
        .tempfile_in(directory)
        .map_err(error)?
        .into_temp_path();
    let _disk = crate::security_budget::DiskLease::register(&tmp, false);
    let db = Connection::open(&tmp).map_err(error)?;
    let budget = crate::resources::spill_bytes(false, directory)?.min(1024 * 1024 * 1024);
    db.execute_batch(&format!("PRAGMA cache_size=-2048;PRAGMA temp_store=FILE;PRAGMA max_page_count={};CREATE TABLE manifest(payload TEXT);CREATE TABLE pairs(ns TEXT,entity TEXT,kind TEXT,target TEXT,n INTEGER,first_ts INTEGER,last_ts INTEGER,PRIMARY KEY(ns,entity,kind,target));CREATE TABLE hours(ns TEXT,entity TEXT,hour INTEGER,slot INTEGER,bytes REAL,events INTEGER,errors INTEGER,peer_group TEXT,PRIMARY KEY(ns,entity,hour));BEGIN;",budget/4096)).map_err(error)?;
    // Both reviewed evidence and higher-priority hypotheses are withheld from
    // training. This does not assert that either is a confirmed attack.
    let clean="ts IS NOT NULL AND json_extract(payload,'$.metadata.time.ambiguity') IS NULL AND NOT EXISTS(SELECT 1 FROM members m JOIN findings f USING(n) WHERE m.ref=investigation_facts.ref AND f.level>=3) AND NOT EXISTS(SELECT 1 FROM investigation_members m JOIN investigation_signals s ON m.id=s.id WHERE m.ref=investigation_facts.ref AND s.priority>=35)";
    let eligible: i64 = source
        .query_row(
            &format!("SELECT count(*) FROM investigation_facts WHERE {clean}"),
            [],
            |r| r.get(0),
        )
        .map_err(error)?;
    let total: i64 = source
        .query_row("SELECT count(*) FROM investigation_facts", [], |r| r.get(0))
        .map_err(error)?;
    let bounds: (Option<i64>, Option<i64>) = source
        .query_row(
            &format!("SELECT min(ts),max(ts) FROM investigation_facts WHERE {clean}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(error)?;
    let sql=format!("WITH clean AS(SELECT * FROM investigation_facts WHERE {clean}), relations AS(SELECT ns,actor AS entity,'user_host' AS kind,host AS target,ts FROM clean WHERE actor<>'' AND host<>'' UNION ALL SELECT ns,actor,'user_resource',json_extract(payload,'$.resource'),ts FROM clean WHERE actor<>'' AND json_extract(payload,'$.resource')<>'' UNION ALL SELECT ns,process,'process_destination',dst,ts FROM clean WHERE process<>'' AND dst<>'') SELECT ns,entity,kind,target,count(*),min(ts),max(ts) FROM relations GROUP BY ns,entity,kind,target ORDER BY ns,entity,kind,target");
    let mut stmt = source.prepare(&sql).map_err(error)?;
    let mut rows = stmt.query([]).map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        crate::operations::check()?;
        db.execute(
            "INSERT INTO pairs VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                row.get::<_, String>(0).map_err(error)?,
                row.get::<_, String>(1).map_err(error)?,
                row.get::<_, String>(2).map_err(error)?,
                row.get::<_, String>(3).map_err(error)?,
                row.get::<_, i64>(4).map_err(error)?,
                row.get::<_, i64>(5).map_err(error)?,
                row.get::<_, i64>(6).map_err(error)?
            ],
        )
        .map_err(error)?;
    }
    let sql=format!("SELECT ns,actor,(ts/3600000)*3600000 AS hour,cast(strftime('%w',ts/1000,'unixepoch') AS INTEGER)*24+cast(strftime('%H',ts/1000,'unixepoch') AS INTEGER),sum(json_extract(payload,'$.bytes')),count(*),sum(CASE WHEN json_extract(payload,'$.outcome')='failure' THEN 1 ELSE 0 END),CASE WHEN count(DISTINCT json_extract(payload,'$.metadata.peer_group'))=1 THEN max(json_extract(payload,'$.metadata.peer_group')) ELSE NULL END FROM investigation_facts WHERE {clean} AND actor<>'' GROUP BY ns,actor,hour ORDER BY ns,actor,hour");
    let mut stmt = source.prepare(&sql).map_err(error)?;
    let mut rows = stmt.query([]).map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        crate::operations::check()?;
        db.execute(
            "INSERT INTO hours VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                row.get::<_, String>(0).map_err(error)?,
                row.get::<_, String>(1).map_err(error)?,
                row.get::<_, i64>(2).map_err(error)?,
                row.get::<_, i64>(3).map_err(error)?,
                row.get::<_, Option<f64>>(4).map_err(error)?,
                row.get::<_, i64>(5).map_err(error)?,
                row.get::<_, i64>(6).map_err(error)?,
                row.get::<_, Option<String>>(7).map_err(error)?
            ],
        )
        .map_err(error)?;
    }
    let manifest = json!({"version":VERSION,"normalization_version":crate::evidence::NORMALIZATION_VERSION,"interpretation":interpretation(),"analysis_id":metadata["analysis_id"],"dataset_fingerprint":metadata["dataset_fingerprint"],"population":total,"eligible":eligible,"withheld":total-eligible,"first":bounds.0,"last":bounds.1,"frozen":true,"clock":"UTC","training":"explicit_snapshot_replacement","contamination_policy":"withhold E3+ members and priority>=35 hypotheses; never update automatically"});
    db.execute("INSERT INTO manifest VALUES(?1)", [manifest.to_string()])
        .map_err(error)?;
    db.execute_batch("COMMIT;").map_err(error)?;
    drop(db);
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tmp)
        .map_err(error)?
        .sync_all()
        .map_err(error)?;
    crate::operations::check()?;
    tmp.persist(path).map_err(error)?;
    Ok(manifest)
}
pub fn load(
    target: &duckdb::Connection,
    settings: &crate::investigation::Settings,
) -> Result<Value, String> {
    target.execute_batch("CREATE TABLE historical_pairs(ns VARCHAR,entity VARCHAR,kind VARCHAR,target VARCHAR,n BIGINT,first_ts BIGINT,last_ts BIGINT);CREATE TABLE historical_hours(ns VARCHAR,entity VARCHAR,hour BIGINT,slot INTEGER,bytes DOUBLE,events BIGINT,errors BIGINT,peer_group VARCHAR);").map_err(error)?;
    if settings.profile.is_none() {
        return Ok(json!({"status":"disabled"}));
    }
    let base = path(settings)?;
    let Some(revision) = &settings.profile_revision else {
        return Ok(
            json!({"status":"cold_start","name":settings.profile,"reason":"Nenhuma revisão de referência selecionada explicitamente neste Caso"}),
        );
    };
    let path = base.with_file_name(format!(
        "{}-{revision}.sqlite",
        base.file_stem().unwrap().to_string_lossy()
    ));
    if !path.exists() {
        return Ok(
            json!({"status":"cold_start","name":settings.profile,"reason":"Nenhuma referência treinada explicitamente neste Caso"}),
        );
    }
    load_at(target, &path)
}
fn load_at(target: &duckdb::Connection, path: &std::path::Path) -> Result<Value, String> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(error)?;
    let manifest: String = db
        .query_row("SELECT payload FROM manifest", [], |r| r.get(0))
        .map_err(error)?;
    let mut manifest: Value = serde_json::from_str(&manifest).map_err(error)?;
    if manifest["version"] != VERSION
        || manifest["normalization_version"] != crate::evidence::NORMALIZATION_VERSION
        || manifest["interpretation"] != interpretation()
    {
        return Err(
            "Referência de outra interpretação; selecione ou treine um snapshot compatível".into(),
        );
    }
    let mut stmt=db.prepare("SELECT ns,entity,kind,target,n,first_ts,last_ts FROM pairs ORDER BY ns,entity,kind,target").map_err(error)?;
    let mut rows = stmt.query([]).map_err(error)?;
    let mut appender = target.appender("historical_pairs").map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        crate::operations::check()?;
        appender
            .append_row(duckdb::params![
                row.get::<_, String>(0).map_err(error)?,
                row.get::<_, String>(1).map_err(error)?,
                row.get::<_, String>(2).map_err(error)?,
                row.get::<_, String>(3).map_err(error)?,
                row.get::<_, i64>(4).map_err(error)?,
                row.get::<_, Option<i64>>(5).map_err(error)?,
                row.get::<_, Option<i64>>(6).map_err(error)?
            ])
            .map_err(error)?;
    }
    appender.flush().map_err(error)?;
    drop(appender);
    let mut stmt=db.prepare("SELECT ns,entity,hour,slot,bytes,events,errors,peer_group FROM hours ORDER BY ns,entity,hour").map_err(error)?;
    let mut rows = stmt.query([]).map_err(error)?;
    let mut appender = target.appender("historical_hours").map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        crate::operations::check()?;
        appender
            .append_row(duckdb::params![
                row.get::<_, String>(0).map_err(error)?,
                row.get::<_, String>(1).map_err(error)?,
                row.get::<_, i64>(2).map_err(error)?,
                row.get::<_, i64>(3).map_err(error)?,
                row.get::<_, Option<f64>>(4).map_err(error)?,
                row.get::<_, i64>(5).map_err(error)?,
                row.get::<_, i64>(6).map_err(error)?,
                row.get::<_, Option<String>>(7).map_err(error)?
            ])
            .map_err(error)?;
    }
    appender.flush().map_err(error)?;
    manifest["status"] = json!("frozen_reference");
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_history_withholds_suspicious_unknown_and_ambiguous_facts_and_never_mutates_a_revision()
    {
        let source = Connection::open_in_memory().unwrap();
        source.execute_batch("CREATE TABLE investigation_facts(ref TEXT,ns TEXT,actor TEXT,host TEXT,process TEXT,dst TEXT,ts INTEGER,payload TEXT);CREATE TABLE findings(n INTEGER,level INTEGER);CREATE TABLE members(n INTEGER,ref TEXT);CREATE TABLE investigation_signals(id TEXT,priority INTEGER);CREATE TABLE investigation_members(id TEXT,ref TEXT);").unwrap();
        for (id, ns, ts, ambiguous) in [
            ("clean-a", "a", Some(3600000), false),
            ("clean-b", "b", Some(3600000), false),
            ("e3", "a", Some(3600000), false),
            ("hypothesis", "a", Some(3600000), false),
            ("unknown", "a", None, false),
            ("ambiguous", "a", Some(3600000), true),
        ] {
            source.execute("INSERT INTO investigation_facts VALUES(?1,?2,'ana','host','process','dst',?3,?4)",params![id,ns,ts,json!({"bytes":1000,"resource":"resource","outcome":"success","metadata":{"time":{"ambiguity":if ambiguous {Some("timezone")}else{None}},"peer_group":"explicit-team"}}).to_string()]).unwrap();
        }
        source.execute_batch("INSERT INTO findings VALUES(0,3);INSERT INTO members VALUES(0,'e3');INSERT INTO investigation_signals VALUES('s',35);INSERT INTO investigation_members VALUES('s','hypothesis');").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("revision.sqlite");
        let metadata = json!({"complete":true,"investigation":{"complete":true},"analysis_id":"a","dataset_fingerprint":"b"});
        let result = train_at(&path, &source, &metadata).unwrap();
        assert_eq!(result["eligible"], 2);
        assert_eq!(result["withheld"], 4);
        let snapshot =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            snapshot
                .query_row::<i64, _, _>("SELECT count(*) FROM pairs", [], |r| r.get(0))
                .unwrap(),
            6
        );
        assert_eq!(
            snapshot
                .query_row::<i64, _, _>("SELECT count(DISTINCT ns) FROM hours", [], |r| r.get(0))
                .unwrap(),
            2
        );
        assert_eq!(train_at(&path, &source, &metadata).unwrap(), result);
        assert!(train_at(&path,&source,&json!({"complete":true,"investigation":{"complete":true},"analysis_id":"changed","dataset_fingerprint":"b"})).is_err());
        assert!(train_at(
            &dir.path().join("incomplete.sqlite"),
            &source,
            &json!({"complete":false})
        )
        .is_err());
        let target = duckdb::Connection::open_in_memory().unwrap();
        load(&target, &crate::investigation::Settings::default()).unwrap();
        assert_eq!(
            load_at(&target, &path).unwrap()["status"],
            "frozen_reference"
        );
        assert_eq!(
            target
                .query_row::<i64, _, _>("SELECT count(*) FROM historical_pairs", [], |r| r.get(0))
                .unwrap(),
            6
        );
    }
}
