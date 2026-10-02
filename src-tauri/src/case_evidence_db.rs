//! Short authority transactions. Schema installation and adoption are gated on
//! verified recovery by the higher-level Case store; importing this module is
//! read-only. Main-schema triggers constrain the known legacy writer.
use super::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
const INVALID: &str = "CASE_EVIDENCE_AUTHORITY: A autoridade da investigação mudou; reabra-a.";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthoritySnapshot {
    pub store: Option<StoreStamp>,
    pub body_revision: String,
    pub analyses: Vec<crate::analysis_context::Identity>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeDependency {
    pub kind: String,
    pub id: String,
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ManifestMetadata {
    pub reference: EvidenceRef,
    pub bytes: u64,
    #[serde(deserialize_with = "batch_ids")]
    pub batches: Vec<String>,
}
fn batch_ids<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    struct Ids;
    impl<'de> serde::de::Visitor<'de> for Ids {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most1024 unique batch UUIDs")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut values: A,
        ) -> Result<Self::Value, A::Error> {
            let mut ids = Vec::new();
            let mut seen = std::collections::HashSet::new();
            while ids.len() < 1024 {
                let Some(id) = values.next_element::<String>()? else {
                    return Ok(ids);
                };
                if id.len() != 36 || uuid::Uuid::parse_str(&id).is_err() || !seen.insert(id.clone())
                {
                    return Err(serde::de::Error::custom("invalid batch identity"));
                }
                ids.push(id);
            }
            if values.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("too many batches"));
            }
            Ok(ids)
        }
    }
    deserializer.deserialize_seq(Ids)
}
pub(super) fn exists(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
fn revision(conn: &Connection) -> Result<String, String> {
    if !exists(conn, "metadata")? {
        return Ok("0".into());
    }
    let value: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key='revision'", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let value = value.unwrap_or_else(|| "0".into());
    if value.parse::<u64>().map_err(|_| INVALID)?.to_string() != value {
        return Err(INVALID.into());
    }
    Ok(value)
}
/// Works on an existing read-only connection; never calls the migrating
/// case_store connector. Caller supplies a consistent SQLite read transaction.
pub(crate) fn snapshot_authority(conn: &Connection) -> Result<AuthoritySnapshot, String> {
    crate::operations::check()?;
    let body_revision = revision(conn)?;
    let store = if exists(conn, "native_evidence_store")? {
        let pair: Option<(String, String)> = conn
            .query_row(
                "SELECT store_id,epoch FROM native_evidence_store WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        pair.map(|(store_id, epoch)| StoreStamp {
            store_id,
            epoch,
            revision: body_revision.clone(),
        })
    } else {
        None
    };
    let mut analyses = Vec::new();
    let mut retained_identity_bytes = 0usize;
    if exists(conn, "case_analysis")? && exists(conn, "cases")? {
        let mut stmt=conn.prepare("SELECT a.case_id,octet_length(a.body),CASE WHEN octet_length(a.body)<=4194304 THEN a.body END FROM case_analysis a JOIN cases c ON c.id=a.case_id ORDER BY a.case_id").map_err(|e|e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            crate::operations::check()?;
            if analyses.len() >= 1000 || row.get::<_, u64>(1).map_err(|e| e.to_string())? > 4 << 20
            {
                return Err("CASE_EVIDENCE_RECOVERY_LIMIT".into());
            }
            let id: String = row.get(0).map_err(|e| e.to_string())?;
            let text: String = row.get(2).map_err(|e| e.to_string())?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct IdentityOnly<'a> {
                #[serde(borrow)]
                case_id: std::borrow::Cow<'a, str>,
                #[serde(borrow)]
                analysis_id: std::borrow::Cow<'a, str>,
                config_revision: u64,
                visibility_revision: u64,
            }
            // Unknown/config values are syntax-checked then skipped, never
            // materialized as a full Value tree for this identity comparison.
            let view: IdentityOnly<'_> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if view.case_id != id
                || view.case_id.is_empty()
                || view.case_id.len() > 4096
                || view.analysis_id.len() > 36
            {
                return Err(INVALID.into());
            }
            uuid::Uuid::parse_str(&view.analysis_id).map_err(|_| INVALID)?;
            retained_identity_bytes = retained_identity_bytes
                .checked_add(view.case_id.len())
                .and_then(|n| n.checked_add(view.analysis_id.len() + 128))
                .filter(|n| *n <= 8 << 20)
                .ok_or("CASE_EVIDENCE_RECOVERY_LIMIT")?;
            analyses.push(crate::analysis_context::Identity {
                case_id: view.case_id.into_owned(),
                analysis_id: view.analysis_id.into_owned(),
                config_revision: view.config_revision,
                visibility_revision: view.visibility_revision,
            });
        }
    }
    Ok(AuthoritySnapshot {
        store,
        body_revision,
        analyses,
    })
}
/// Includes all retained native history, even if its Case was deleted. No
/// payload is opened here; recovery applies its aggregate bytes/reader budgets.
pub(crate) fn native_dependencies(
    conn: &Connection,
    mut visitor: impl FnMut(NativeDependency) -> Result<(), String>,
) -> Result<(), String> {
    let mut count = 0usize;
    for dependency in bootstrap::dependencies(conn)? {
        visitor(dependency)?;
        count += 1;
    }
    for (table, kind, suffix) in [
        ("native_evidence_batches", "batch", "batch"),
        ("native_evidence_manifests", "manifest", "manifest"),
    ] {
        if !exists(conn, table)? {
            continue;
        }
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {},octet_length(body),CASE WHEN octet_length(body)<=65536 THEN body END FROM {table}",
                if kind == "batch" {
                    "batch_id"
                } else {
                    "manifest_id"
                }
            ))
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            crate::operations::check()?;
            count += 1;
            if count > 16_384 || row.get::<_, u64>(1).map_err(|e| e.to_string())? > 64 << 10 {
                return Err("CASE_EVIDENCE_RECOVERY_LIMIT".into());
            }
            let stored_id: String = row.get(0).map_err(|e| e.to_string())?;
            let text: String = row.get(2).map_err(|e| e.to_string())?;
            let (id, bytes, sha256) = if kind == "batch" {
                let value: BatchRef = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                (value.batch_id, value.bytes, value.sha256)
            } else {
                let value: ManifestMetadata =
                    serde_json::from_str(&text).map_err(|e| e.to_string())?;
                (
                    value.reference.manifest_id,
                    value.bytes,
                    value.reference.manifest_sha256,
                )
            };
            if stored_id != id {
                return Err(INVALID.into());
            }
            uuid::Uuid::parse_str(&id).map_err(|_| INVALID)?;
            if sha256.len() != 64
                || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || bytes > CAPTURE_BYTES
            {
                return Err(INVALID.into());
            }
            visitor(NativeDependency {
                kind: kind.into(),
                relative_path: format!("evidence-v1/{id}.{suffix}"),
                id,
                bytes,
                sha256,
            })?;
        }
    }
    Ok(())
}
/// Applied to the verified staging database before SQLite restores it into the
/// live profile. An older snapshot without native references has no epoch.
pub(crate) fn restore_epoch(
    tx: &Transaction<'_>,
    epoch: &str,
) -> Result<Option<StoreIdentity>, String> {
    uuid::Uuid::parse_str(epoch).map_err(|_| INVALID)?;
    if !exists(tx, "native_evidence_store")? {
        return Ok(None);
    }
    let id: Option<String> = tx
        .query_row(
            "SELECT store_id FROM native_evidence_store WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(store_id) = id else { return Ok(None) };
    tx.execute(
        "UPDATE native_evidence_store SET epoch=?1 WHERE singleton=1",
        [epoch],
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(StoreIdentity {
        store_id,
        epoch: epoch.into(),
    }))
}

pub(super) fn schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(r#"
CREATE TABLE IF NOT EXISTS native_evidence_store(singleton INTEGER PRIMARY KEY CHECK(singleton=1),store_id TEXT NOT NULL,epoch TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_evidence_batches(batch_id TEXT PRIMARY KEY,body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_evidence_manifests(manifest_id TEXT PRIMARY KEY,body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_evidence_cases(case_id TEXT PRIMARY KEY,analysis_id TEXT NOT NULL,metadata TEXT NOT NULL,bindings TEXT NOT NULL,evidence_signature TEXT NOT NULL,body_sha256 TEXT NOT NULL,recovery_id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_evidence_anchors(case_id TEXT NOT NULL,analysis_id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(case_id,analysis_id));
CREATE TABLE IF NOT EXISTS native_evidence_protected(case_id TEXT PRIMARY KEY,store_id TEXT NOT NULL,analysis_id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_evidence_permits(case_id TEXT PRIMARY KEY,nonce TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS native_evidence_receipts(request_id TEXT PRIMARY KEY,request_sha256 TEXT NOT NULL,receipt TEXT NOT NULL,created_ms INTEGER NOT NULL);
CREATE TRIGGER IF NOT EXISTS native_evidence_cases_insert BEFORE INSERT ON cases
WHEN EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=NEW.id)
 AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=NEW.id)
 AND NOT EXISTS(SELECT 1 FROM cases c WHERE c.id=NEW.id AND c.body IS NEW.body AND c.position IS NEW.position)
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS native_evidence_cases_update BEFORE UPDATE ON cases
WHEN (NEW.id IS NOT OLD.id OR NEW.body IS NOT OLD.body OR NEW.position IS NOT OLD.position)
 AND ((EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=OLD.id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=OLD.id))
 OR (EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=NEW.id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=NEW.id)))
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS native_evidence_cases_delete BEFORE DELETE ON cases
WHEN EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=OLD.id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS native_evidence_context_insert BEFORE INSERT ON case_analysis
WHEN EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=NEW.case_id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=NEW.case_id)
 AND (json_valid(NEW.body)=0 OR json_type(NEW.body,'$.caseId') IS NOT 'text' OR json_extract(NEW.body,'$.caseId') IS NOT NEW.case_id
 OR json_type(NEW.body,'$.analysisId') IS NOT 'text' OR json_extract(NEW.body,'$.analysisId') IS NOT (SELECT analysis_id FROM native_evidence_protected WHERE case_id=NEW.case_id))
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS native_evidence_context_update BEFORE UPDATE ON case_analysis
WHEN ((EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=OLD.case_id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=OLD.case_id))
 OR (EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=NEW.case_id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=NEW.case_id)))
 AND (NEW.case_id IS NOT OLD.case_id OR json_valid(NEW.body)=0 OR json_type(NEW.body,'$.caseId') IS NOT 'text' OR json_extract(NEW.body,'$.caseId') IS NOT NEW.case_id
 OR json_type(NEW.body,'$.analysisId') IS NOT 'text' OR json_extract(NEW.body,'$.analysisId') IS NOT (SELECT analysis_id FROM native_evidence_protected WHERE case_id=NEW.case_id))
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
CREATE TRIGGER IF NOT EXISTS native_evidence_context_delete BEFORE DELETE ON case_analysis
WHEN EXISTS(SELECT 1 FROM native_evidence_protected p WHERE p.case_id=OLD.case_id) AND NOT EXISTS(SELECT 1 FROM native_evidence_permits a WHERE a.case_id=OLD.case_id)
BEGIN SELECT RAISE(ABORT,'CASE_NATIVE_EVIDENCE_REQUIRED'); END;
"#).map_err(|e|e.to_string())
}
pub(super) fn initialize_identity(
    tx: &Transaction<'_>,
    identity: &StoreIdentity,
) -> Result<(), String> {
    uuid::Uuid::parse_str(&identity.store_id).map_err(|_| INVALID)?;
    uuid::Uuid::parse_str(&identity.epoch).map_err(|_| INVALID)?;
    let old: Option<(String, String)> = tx
        .query_row(
            "SELECT store_id,epoch FROM native_evidence_store WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match old {
        Some((id, epoch)) if id == identity.store_id && epoch == identity.epoch => Ok(()),
        Some(_) => Err(INVALID.into()),
        None => {
            tx.execute(
                "INSERT INTO native_evidence_store VALUES(1,?1,?2)",
                params![identity.store_id, identity.epoch],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        }
    }
}
pub(super) fn stamp(conn: &Connection) -> Result<StoreStamp, String> {
    let (store_id, epoch) = conn
        .query_row(
            "SELECT store_id,epoch FROM native_evidence_store WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    Ok(StoreStamp {
        store_id,
        epoch,
        revision: revision(conn)?,
    })
}
pub(super) fn require_stamp(conn: &Connection, expected: &StoreStamp) -> Result<(), String> {
    if stamp(conn)? != *expected {
        Err(INVALID.into())
    } else {
        Ok(())
    }
}
pub(super) fn identity_for(
    conn: &Connection,
    case_id: &str,
) -> Result<crate::analysis_context::Identity, String> {
    if case_id.is_empty() || case_id.len() > 4096 {
        return Err(INVALID.into());
    }
    let bytes:usize=conn.query_row("SELECT octet_length(a.body) FROM case_analysis a JOIN cases c ON c.id=a.case_id WHERE a.case_id=?1",[case_id],|row|row.get(0)).map_err(|e|e.to_string())?;
    if bytes > 4 << 20 {
        return Err(INVALID.into());
    }
    // SQLite TEXT, its owned String and the JSON deserializer's escaped-key
    // scratch may coexist. Reserve them before fetching the body.
    let _scratch = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes
            .checked_mul(3)
            .and_then(|n| n.checked_add(16 << 10))
            .ok_or(INVALID)?,
    )?;
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(body)=?2 THEN body END FROM case_analysis WHERE case_id=?1",params![case_id,bytes],|row|row.get(0)).map_err(|e|e.to_string())?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct View<'a> {
        #[serde(borrow)]
        case_id: std::borrow::Cow<'a, str>,
        #[serde(borrow)]
        analysis_id: std::borrow::Cow<'a, str>,
        config_revision: u64,
        visibility_revision: u64,
    }
    let view: View<'_> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if view.case_id != case_id || view.analysis_id.len() > 36 {
        return Err(INVALID.into());
    }
    uuid::Uuid::parse_str(&view.analysis_id).map_err(|_| INVALID)?;
    Ok(crate::analysis_context::Identity {
        case_id: case_id.into(),
        analysis_id: view.analysis_id.into_owned(),
        config_revision: view.config_revision,
        visibility_revision: view.visibility_revision,
    })
}
pub(super) fn protect(tx: &Transaction<'_>, owner: &EvidenceOwner) -> Result<(), String> {
    if stamp(tx)?.store_id != owner.store_id {
        return Err(INVALID.into());
    }
    let actual: String = tx
        .query_row(
            "SELECT json_extract(body,'$.analysisId') FROM case_analysis WHERE case_id=?1",
            [&owner.case_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if actual != owner.analysis_id {
        return Err(INVALID.into());
    }
    tx.execute("INSERT INTO native_evidence_protected(case_id,store_id,analysis_id) VALUES(?1,?2,?3) ON CONFLICT(case_id) DO UPDATE SET store_id=excluded.store_id,analysis_id=excluded.analysis_id",params![owner.case_id,owner.store_id,owner.analysis_id]).map_err(|e|e.to_string())?;
    Ok(())
}
/// COMMIT is owned here. Permit removal is fallible and mandatory before it.
pub(super) fn transaction<T>(
    conn: &mut Connection,
    case_ids: &[String],
    validate: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
    work: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    transaction_inner(conn, case_ids, false, validate, work)
}
/// Initial schema and legacy-writer barriers are installed in the same commit
/// as verified adoption, after source authority comparison and before permits.
pub(super) fn transaction_initializing<T>(
    conn: &mut Connection,
    case_ids: &[String],
    validate: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
    work: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    transaction_inner(conn, case_ids, true, validate, work)
}
fn transaction_inner<T>(
    conn: &mut Connection,
    case_ids: &[String],
    initialize: bool,
    validate: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
    work: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    crate::operations::check()?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if exists(&tx, "native_evidence_permits")?
        && tx
            .query_row::<u64, _, _>("SELECT count(*) FROM native_evidence_permits", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?
            != 0
    {
        return Err(
            "CASE_EVIDENCE_RECOVERY_REQUIRED: Permissão persistida inesperadamente.".into(),
        );
    }
    // Source/recovery/store/owner checks run in the same immediate transaction
    // before any compatibility writer permit exists.
    validate(&tx)?;
    if initialize {
        crate::analysis_context::schema(&tx)?;
        schema(&tx)?;
    }
    if case_ids.len() > 1000 {
        return Err("CASE_EVIDENCE_LIMIT".into());
    }
    let nonce = uuid::Uuid::new_v4().to_string();
    for id in case_ids {
        if id.is_empty() || id.len() > 4096 {
            return Err(INVALID.into());
        }
        tx.execute(
            "INSERT INTO native_evidence_permits VALUES(?1,?2)",
            params![id, nonce],
        )
        .map_err(|e| e.to_string())?;
    }
    let result = work(&tx)?;
    crate::operations::check()?;
    let removed = tx
        .execute(
            "DELETE FROM native_evidence_permits WHERE nonce=?1",
            [&nonce],
        )
        .map_err(|e| e.to_string())?;
    if removed != case_ids.len()
        || tx
            .query_row::<u64, _, _>("SELECT count(*) FROM native_evidence_permits", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?
            != 0
    {
        return Err("CASE_EVIDENCE_RECOVERY_REQUIRED: Permissão não encerrada.".into());
    }
    tx.commit().map_err(|e| e.to_string())?;
    crate::operations::commit();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Connection, EvidenceOwner) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE cases(id TEXT PRIMARY KEY,body TEXT NOT NULL,position INTEGER NOT NULL);CREATE TABLE case_analysis(case_id TEXT PRIMARY KEY,body TEXT NOT NULL);CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);INSERT INTO metadata VALUES('revision','1');INSERT INTO cases VALUES('c','{\"id\":\"c\"}',0);INSERT INTO case_analysis VALUES('c','{\"caseId\":\"c\",\"analysisId\":\"00000000-0000-4000-8000-000000000001\",\"configRevision\":0,\"visibilityRevision\":0}');").unwrap();
        schema(&conn).unwrap();
        let identity = StoreIdentity {
            store_id: uuid::Uuid::new_v4().to_string(),
            epoch: uuid::Uuid::new_v4().to_string(),
        };
        let tx = conn.transaction().unwrap();
        initialize_identity(&tx, &identity).unwrap();
        let owner = EvidenceOwner {
            store_id: identity.store_id,
            case_id: "c".into(),
            analysis_id: "00000000-0000-4000-8000-000000000001".into(),
        };
        protect(&tx, &owner).unwrap();
        tx.commit().unwrap();
        (conn, owner)
    }
    #[test]
    fn old_unchanged_upsert_allowed_effective_mutation_and_null_owner_denied() {
        let (conn, _) = setup();
        conn.execute("INSERT INTO cases VALUES('c','{\"id\":\"c\"}',0) ON CONFLICT(id) DO UPDATE SET body=excluded.body,position=excluded.position WHERE cases.body<>excluded.body OR cases.position<>excluded.position",[]).unwrap();
        for sql in["UPDATE cases SET body='{}' WHERE id='c'","UPDATE cases SET position=1 WHERE id='c'","DELETE FROM case_analysis WHERE case_id='c'","DELETE FROM cases WHERE id='c'","UPDATE case_analysis SET body='{}' WHERE case_id='c'","UPDATE case_analysis SET body='{\"caseId\":\"c\",\"analysisId\":null}' WHERE case_id='c'"]{assert!(conn.execute(sql,[]).is_err(),"{sql}");}
        conn.execute("UPDATE case_analysis SET body='{\"caseId\":\"c\",\"analysisId\":\"00000000-0000-4000-8000-000000000001\",\"configRevision\":1,\"visibilityRevision\":2}' WHERE case_id='c'",[]).unwrap();
    }
    #[test]
    fn permits_cannot_survive_success_or_failed_publication() {
        let (mut conn, _) = setup();
        transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| {
                tx.execute(
                    "UPDATE cases SET body='{\"id\":\"c\",\"note\":\"ok\"}' WHERE id='c'",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            },
        )
        .unwrap();
        let before: String = conn
            .query_row("SELECT body FROM cases WHERE id='c'", [], |r| r.get(0))
            .unwrap();
        assert!(transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| {
                tx.execute("DELETE FROM cases WHERE id='c'", [])
                    .map_err(|e| e.to_string())?;
                Err::<(), _>("injected failure".into())
            }
        )
        .is_err());
        assert_eq!(
            conn.query_row::<String, _, _>("SELECT body FROM cases WHERE id='c'", [], |r| r.get(0))
                .unwrap(),
            before
        );
        assert_eq!(
            conn.query_row::<u64, _, _>("SELECT count(*) FROM native_evidence_permits", [], |r| r
                .get(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn tombstone_survives_native_delete_and_blocks_old_resurrection() {
        let (mut conn, _) = setup();
        transaction(
            &mut conn,
            &["c".into()],
            |_| Ok(()),
            |tx| {
                tx.execute("DELETE FROM case_analysis WHERE case_id='c'", [])
                    .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM cases WHERE id='c'", [])
                    .map_err(|e| e.to_string())?;
                Ok(())
            },
        )
        .unwrap();
        assert!(conn
            .execute("INSERT INTO cases VALUES('c','{}',0)", [])
            .is_err());
        assert_eq!(
            conn.query_row::<u64, _, _>(
                "SELECT count(*) FROM native_evidence_protected",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn context_only_updates_change_the_recovery_snapshot_without_body_revision() {
        let (conn, _) = setup();
        let before = snapshot_authority(&conn).unwrap();
        conn.execute("UPDATE case_analysis SET body='{\"caseId\":\"c\",\"analysisId\":\"00000000-0000-4000-8000-000000000001\",\"configRevision\":1,\"visibilityRevision\":0}' WHERE case_id='c'",[]).unwrap();
        let after = snapshot_authority(&conn).unwrap();
        assert_eq!(before.body_revision, after.body_revision);
        assert_ne!(before, after);
    }
}
