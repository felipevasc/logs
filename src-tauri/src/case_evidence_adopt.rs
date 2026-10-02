//! Verified legacy adoption. The recovery generation is immutable authority for
//! preparation; a single immediate transaction installs the native boundary.
use super::*;
use crate::{case_recovery::VerifiedRecovery, case_work_budget::Lease};
use rusqlite::{params, Connection};
use std::path::Path;
const INVALID:&str="CASE_EVIDENCE_ADOPTION_CHANGED: A investigação mudou depois da recuperação; prepare uma nova recuperação antes de adotar.";

pub(crate) struct PublishedAdoption {
    pub receipt: AdoptionReceipt,
    _credit: Lease,
}
impl Serialize for PublishedAdoption {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.receipt.serialize(serializer)
    }
}

const CONTEXT_BYTES: usize = 4 << 20;
/// The callback is deliberately before Value/Snapshot decoding. It accounts
/// for the legacy Value plus the imported Snapshot clone and bounded strings.
fn admit_legacy_span(
    raw: RawJson<'_>,
    mut admit: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), String> {
    if raw.get().len() > CONTEXT_BYTES {
        return Err("CASE_EVIDENCE_CONTEXT_LIMIT".into());
    }
    let plan = preflight_value(raw)?;
    let bytes = plan
        .materialization_credit
        .checked_mul(2)
        .and_then(|n| n.checked_add(raw.get().len().checked_mul(4)?))
        .ok_or(INVALID)?;
    admit(bytes)
}
fn add_credit(credit: &mut Lease, bytes: usize) -> Result<(), String> {
    credit.merge(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes,
    )?)
}
fn missing_context(
    case_id: &str,
    body: &str,
    assets: &Path,
    credit: &mut Lease,
) -> Result<crate::analysis_context::Snapshot, String> {
    let document = checked_document(body)?;
    let mut fields = document.members()?;
    let mut context = None;
    while let Some(field) = fields.next()? {
        crate::operations::check()?;
        // All possible escaped spellings of this fixed key fit this bound.
        if field.key.len() <= 128 && field.key()? == "analysisContext" {
            if context.is_some() {
                return Err("CASE_EVIDENCE_DUPLICATE_CONTEXT".into());
            }
            context = Some(field.value);
        }
    }
    if let Some(raw) = context.filter(|raw| raw.kind() != b'n') {
        admit_legacy_span(raw, |bytes| add_credit(credit, bytes))?;
        let value: Value = serde_json::from_str(raw.get()).map_err(|e| e.to_string())?;
        return crate::analysis_context::prepare_legacy_case(case_id, Some(&value), assets);
    }
    let path = assets.join("derived_fields.json");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return crate::analysis_context::prepare_new_case(case_id)
        }
        Err(error) => return Err(error.to_string()),
        Ok(_) => (),
    }
    use std::io::Read;
    let mut file = crate::case_archive_format::open_regular(&path)?;
    let size =
        usize::try_from(file.metadata().map_err(|e| e.to_string())?.len()).map_err(|_| INVALID)?;
    if size > CONTEXT_BYTES {
        return Err("CASE_EVIDENCE_CONTEXT_LIMIT".into());
    }
    add_credit(
        credit,
        size.checked_mul(3)
            .and_then(|n| n.checked_add(64 << 10))
            .ok_or(INVALID)?,
    )?;
    let mut bytes = vec![0; size];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let mut extra = [0u8; 1];
    if file.read(&mut extra).map_err(|e| e.to_string())? != 0 {
        return Err(INVALID.into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "CASE_EVIDENCE_CONTEXT_UTF8")?;
    let value = match RawJson::checked(&text) {
        Ok(raw) => {
            admit_legacy_span(raw, |bytes| add_credit(credit, bytes))?;
            serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text))
        }
        Err(_) => {
            crate::operations::check()?;
            Value::String(text)
        }
    };
    crate::analysis_context::prepare_legacy_value(case_id, value)
}
fn existing_context(
    conn: &Connection,
    case_id: &str,
    credit: &mut Lease,
) -> Result<crate::analysis_context::Snapshot, String> {
    let size: usize = conn
        .query_row(
            "SELECT octet_length(body) FROM case_analysis WHERE case_id=?1",
            [case_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if size > CONTEXT_BYTES {
        return Err("CASE_EVIDENCE_CONTEXT_LIMIT".into());
    }
    add_credit(
        credit,
        size.checked_mul(3)
            .and_then(|n| n.checked_add(64 << 10))
            .ok_or(INVALID)?,
    )?;
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(body)=?2 THEN body END FROM case_analysis WHERE case_id=?1",params![case_id,size],|row|row.get(0)).map_err(|e|e.to_string())?;
    admit_legacy_span(RawJson::checked(&text)?, |bytes| add_credit(credit, bytes))?;
    let snapshot: crate::analysis_context::Snapshot =
        serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if snapshot.case_id != case_id
        || snapshot.schema_version != 1
        || uuid::Uuid::parse_str(&snapshot.analysis_id).is_err()
    {
        return Err(INVALID.into());
    }
    Ok(snapshot)
}

/// No incoming `verified` boolean is trusted. Only the recovery module can
/// construct the verified generation supplied to this operation.
pub(crate) fn adopt(
    root: &Path,
    request_id: &str,
    recovery: &VerifiedRecovery,
) -> Result<PublishedAdoption, String> {
    prepare::request_id(request_id)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let _serial = prepare::serialized_guard()?;
    recovery.validate()?;
    let recovery_receipt = recovery.receipt();
    let expected_copy = root
        .join("case-recovery-v1")
        .join(&recovery_receipt.recovery_id)
        .join("investigations.sqlite3");
    if recovery.snapshot_database_path() != expected_copy {
        return Err(INVALID.into());
    }
    let fingerprint = prepare::fingerprint(
        &serde_json::json!({"kind":"native_adoption_v1","recovery":recovery_receipt}),
    )?;
    let conn = authority::connect_readonly(&root)?;
    if let Some(replay) =
        receipts::lookup(&conn, request_id, &fingerprint, receipts::Kind::Adoption)?
    {
        let (entry, credit) = replay.into_parts();
        let mut receipt: AdoptionReceipt =
            serde_json::from_value(entry.value).map_err(|e| e.to_string())?;
        receipt.current_store = db::stamp(&conn)?;
        receipt.replayed = true;
        receipt.reconcile_required = receipt.current_store != receipt.after;
        root_guard.validate()?;
        return Ok(PublishedAdoption {
            receipt,
            _credit: credit,
        });
    }
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if snapshot_authority(&conn)? != *recovery.authority() {
        return Err(INVALID.into());
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    let before = recovery.authority().clone();
    let identity = before
        .store
        .as_ref()
        .map(StoreStamp::identity)
        .unwrap_or_else(|| StoreIdentity {
            store_id: uuid::Uuid::new_v4().to_string(),
            epoch: uuid::Uuid::new_v4().to_string(),
        });
    let expected = StoreStamp {
        store_id: identity.store_id.clone(),
        epoch: identity.epoch.clone(),
        revision: before.body_revision.clone(),
    };
    let copied_path = recovery.snapshot_database_path();
    let copied_root = copied_path.parent().ok_or(INVALID)?;
    let copied = authority::connect_readonly(copied_root)?;
    copied
        .execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if snapshot_authority(&copied)? != before {
        return Err(INVALID.into());
    }
    let has_native = db::exists(&copied, "native_evidence_cases")?;
    let sql = if has_native {
        "SELECT CASE WHEN octet_length(c.id)<=4096 THEN c.id END,c.position FROM cases c WHERE NOT EXISTS(SELECT 1 FROM native_evidence_cases n WHERE n.case_id=c.id) ORDER BY c.position,c.id"
    } else {
        "SELECT CASE WHEN octet_length(id)<=4096 THEN id END,position FROM cases ORDER BY position,id"
    };
    let mut stmt = copied.prepare(sql).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut prepared = Vec::new();
    let mut mirror_bytes = 0usize;
    let mut credit = crate::case_cache::reserve_work(crate::case_work_budget::global(), 2 << 20)?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        if prepared.len() >= 1000 {
            return Err("CASE_EVIDENCE_ADOPTION_LIMIT".into());
        }
        let id: String = row.get(0).map_err(|e| e.to_string())?;
        let position: i64 = row.get(1).map_err(|e| e.to_string())?;
        let body = read_case_body(&copied, &id)?;
        let context_exists = if db::exists(&copied, "case_analysis")? {
            copied
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM case_analysis WHERE case_id=?1)",
                    [&id],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?
        } else {
            false
        };
        let mut snapshot = if context_exists {
            existing_context(&copied, &id, &mut credit)?
        } else {
            missing_context(
                &id,
                body.text(),
                &recovery.snapshot_assets_root(),
                &mut credit,
            )?
        };
        // At most two bounded messages; reserve their owned growth before
        // constructing them. The unchanged context-size gate follows below.
        add_credit(&mut credit, 8 << 10)?;
        for diagnostic in crate::analysis_context::reference_interpretation_diagnostics(
            snapshot.config.references.iter().map(|reference| reference.interpretation_version),
            true,
        ) {
            if !snapshot.migration_diagnostics.iter().any(|existing| existing.code == diagnostic.code) {
                snapshot.migration_diagnostics.push(diagnostic);
            }
        }
        let context_bytes = view::json_size(&snapshot, 4 << 20)?;
        credit.merge(crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            context_bytes.checked_mul(3).ok_or(INVALID)?,
        )?)?;
        let case =
            prepared_case::prepare_case(&root, &expected, snapshot, body.text(), &root_guard)?;
        mirror_bytes = mirror_bytes
            .checked_add(case.mirror_bytes())
            .filter(|n| *n <= 256 << 20)
            .ok_or("CASE_EVIDENCE_ADOPTION_LIMIT")?;
        prepared.push((position, case));
    }
    drop(rows);
    drop(stmt);
    copied.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(copied);
    recovery.validate()?; // All hashing/copy work precedes the writer transaction.
    root_guard.validate()?;
    for (_, case) in &prepared {
        case.validate(&root)?;
    }
    let next = before
        .body_revision
        .parse::<u64>()
        .map_err(|_| INVALID)?
        .checked_add(1)
        .ok_or(INVALID)?;
    let after = StoreStamp {
        store_id: identity.store_id.clone(),
        epoch: identity.epoch.clone(),
        revision: next.to_string(),
    };
    let receipt = AdoptionReceipt {
        request_id: request_id.into(),
        recovery: recovery_receipt,
        before: match &before.store {
            Some(store) => RecoverySource::Native {
                store: store.clone(),
            },
            None => RecoverySource::Legacy {
                body_revision: before.body_revision.clone(),
            },
        },
        after: after.clone(),
        current_store: after.clone(),
        replayed: false,
        reconcile_required: false,
        adopted: prepared
            .iter()
            .map(|(_, case)| case.owner().clone())
            .collect(),
        unavailable: prepared
            .iter()
            .filter_map(|(_, case)| match case.evidence() {
                CaseEvidenceState::Unavailable(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
    };
    let receipt_bytes = view::json_size(&receipt, VIEW_DOCUMENT_BYTES)?;
    credit.merge(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        receipt_bytes.checked_mul(8).ok_or(INVALID)?,
    )?)?;
    let value = serde_json::to_value(&receipt).map_err(|e| e.to_string())?;
    let ids = prepared
        .iter()
        .map(|(_, case)| case.owner().case_id.clone())
        .collect::<Vec<_>>();
    let mut conn = authority::connect_readwrite(&root)?;
    db::transaction_initializing(
        &mut conn,
        &ids,
        |tx| {
            if snapshot_authority(tx)? != before {
                return Err(INVALID.into());
            }
            root_guard.validate()?;
            for (_, case) in &prepared {
                case.validate(&root)?;
            }
            Ok(())
        },
        |tx| {
            crate::analysis_context::schema(tx)?;
            db::initialize_identity(tx, &identity)?;
            for (position, case) in &prepared {
                crate::operations::check()?;
                crate::analysis_context::canonicalize_native_adoption(tx, case.snapshot())?;
                case.publish_existing(tx, *position, &receipt.recovery.recovery_id)?;
            }
            tx.execute("INSERT INTO metadata(key,value) VALUES('revision',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[&after.revision]).map_err(|e|e.to_string())?;
            // Mark the prior legacy-initialization boundary complete only after all
            // missing contexts have been supplied from this verified generation.
            tx.execute(
                "INSERT OR IGNORE INTO metadata(key,value) VALUES('case-analysis-v1','1')",
                [],
            )
            .map_err(|e| e.to_string())?;
            view::validate_size(tx, &root, VIEW_DOCUMENT_BYTES)?;
            receipts::insert(
                tx,
                request_id,
                &fingerprint,
                &receipts::Entry {
                    version: 1,
                    kind: receipts::Kind::Adoption,
                    expected_store: before.store.clone(),
                    publication_store: after.identity(),
                    value,
                },
            )?;
            Ok(())
        },
    )?;
    Ok(PublishedAdoption {
        receipt,
        _credit: credit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn legacy() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let conn = crate::case_store::connect(root.path()).unwrap();
        conn.execute("INSERT INTO metadata VALUES('revision','1')", [])
            .unwrap();
        conn.execute("INSERT INTO metadata VALUES('active','\"c\"')", [])
            .unwrap();
        let mut snapshot = crate::analysis_context::prepare_new_case("c").unwrap();
        snapshot.config.derived_fields.push(serde_json::json!({"name":"x","source":"message","pattern":"(.*)","future":"2.547114365375239e-8".parse::<f64>().unwrap()}));
        conn.execute(
            "INSERT INTO case_analysis VALUES('c',?1)",
            [serde_json::to_string(&snapshot).unwrap()],
        )
        .unwrap();
        conn.execute("INSERT INTO cases VALUES('c',?1,0)",[r#"{"id":"c","name":"Case","items":[{"id":"i","rows":[{"id":8,"event_ref":"same","future":18446744073709551615},{"id":8,"event_ref":"same","future":1.0},{"future":-0.0}]}],"timeline":{"annotations":[{"id":"a","anchor":"e:i:same"}]}}"#]).unwrap();
        root
    }
    fn recovery(root: &Path) -> VerifiedRecovery {
        crate::case_recovery::prepare(root, &prepare::root_work()).unwrap()
    }
    #[test]
    fn json_only_adoption_discloses_exact_records_and_keeps_legacy_reference_policy() {
        let root = tempfile::tempdir().unwrap();
        let envelope = r#"{ "id":1,"event_ref":"witness","timestamp":null,"source":"","level":"","code":"","name":"","description":"","message":"","raw":"","fields":{"key":2.547114365375239e-8},"future":{"one":1.000} }"#;
        let mut context = crate::analysis_context::prepare_new_case("c").unwrap();
        context.config.references.push(crate::analysis_context::ReferenceDescriptor {
            schema_version: 1, interpretation_version: 1, id: "r".into(), name: "Legacy reference".into(),
            content_sha256: "a".repeat(64), format: "jsonl".into(), columns: vec!["key".into()],
            key_columns: vec!["key".into()], duplicate_policy: "reject".into(),
        });
        let descriptor = serde_json::to_string(&context.config.references[0]).unwrap();
        let config = context.config.clone();
        let document = format!("{{\"cases\":[{{\"id\":\"c\",\"items\":[{{\"id\":\"i\",\"rows\":[{envelope}]}}],\"analysisContext\":{}}}],\"active\":\"c\"}}", serde_json::to_string(&context).unwrap());
        std::fs::write(root.path().join("cases.json"), &document).unwrap();
        bootstrap(root.path()).unwrap();
        // Early context admission follows the same legacy-effective policy;
        // neither initialization nor adoption rewrites a record envelope.
        drop(crate::case_store::context_connection(root.path()).unwrap());
        let backup = recovery(root.path());
        let adopted = adopt(root.path(), "numeric-policy", &backup).unwrap();
        let conn = authority::connect_readonly(root.path()).unwrap();
        let current = crate::analysis_context::read(&conn, "c").unwrap();
        assert_eq!(current.config, config);
        assert_eq!(serde_json::to_string(&current.config.references[0]).unwrap(), descriptor);
        assert_eq!(current.migration_diagnostics.iter().filter(|d| d.code == "reference_interpretation_legacy").count(), 1);
        let loaded = load_view(root.path()).unwrap();
        let reference: EvidenceReference = serde_json::from_value(loaded.document().cases[0]["items"][0]["rows"]["reference"].clone()).unwrap();
        let lease = open_reference(root.path(), &adopted.receipt.after.identity(), &reference).unwrap();
        let actual = lease.envelope(&lease.member_at(0).unwrap()).unwrap();
        assert_eq!(actual, envelope);
        let exact = materialize_event(&actual, preflight_envelope(&actual).unwrap()).unwrap();
        assert_eq!(exact.fields["key"].as_f64().unwrap().to_bits(), 0x3e5b597464455d8a);
        let legacy: crate::model::Event = serde_json::from_str(envelope).unwrap();
        assert_eq!(legacy.fields["key"].as_f64().unwrap().to_bits(), 0x3e5b597464455d8b);
        assert_eq!(std::fs::read_to_string(root.path().join("cases.json")).unwrap(), document);
        assert!(adopt(root.path(), "numeric-policy", &backup).unwrap().receipt.replayed);
        assert_eq!(crate::analysis_context::read(&conn, "c").unwrap().migration_diagnostics, current.migration_diagnostics);
    }
    #[test]
    fn legacy_context_admission_precedes_decode_and_retains_legacy_fraction_policy() {
        let raw = RawJson::checked(r#"{"many":[null,null,null],"fraction":2.547114365375239e-8}"#)
            .unwrap();
        let mut called = false;
        let refused = admit_legacy_span(raw, |bytes| {
            called = true;
            assert!(bytes > raw.get().len());
            Err("named-credit-refusal".into())
        });
        assert!(called);
        assert_eq!(refused.unwrap_err(), "named-credit-refusal");
        let original: Value = serde_json::from_str(raw.get()).unwrap();
        let mut held =
            crate::case_cache::reserve_work(crate::case_work_budget::global(), 0).unwrap();
        admit_legacy_span(raw, |bytes| add_credit(&mut held, bytes)).unwrap();
        let admitted: Value = serde_json::from_str(raw.get()).unwrap();
        assert_eq!(
            admitted["fraction"].as_f64().unwrap().to_bits(),
            original["fraction"].as_f64().unwrap().to_bits()
        );
        assert!(held.bytes() > raw.get().len());
    }
    #[test]
    fn embedded_and_copied_global_context_use_admitted_legacy_interpretation() {
        let assets = tempfile::tempdir().unwrap();
        let mut snapshot = crate::analysis_context::prepare_new_case("foreign").unwrap();
        snapshot.config.derived_fields.push(serde_json::json!({"name":"x","source":"message","pattern":"(.*)","future":"2.547114365375239e-8".parse::<f64>().unwrap()}));
        let text = serde_json::to_string(&snapshot).unwrap();
        let expected: crate::analysis_context::Snapshot = serde_json::from_str(&text).unwrap();
        let body=format!("{{\"id\":\"local\",\"analysisContext\":{text},\"items\":[{{\"rows\":[{{\"future\":18446744073709551615}}]}}]}}");
        let mut credit =
            crate::case_cache::reserve_work(crate::case_work_budget::global(), 0).unwrap();
        let decoded = missing_context("local", &body, assets.path(), &mut credit).unwrap();
        assert_eq!(decoded.config, expected.config);
        assert!(credit.bytes() > text.len());
        std::fs::write(
            assets.path().join("derived_fields.json"),
            r#"[{"name":"x","source":"message","pattern":"(.*)","future":2.547114365375239e-8}]"#,
        )
        .unwrap();
        let mut credit =
            crate::case_cache::reserve_work(crate::case_work_budget::global(), 0).unwrap();
        let decoded =
            missing_context("local", r#"{"id":"local"}"#, assets.path(), &mut credit).unwrap();
        assert_eq!(decoded.config, expected.config);
        std::fs::write(assets.path().join("derived_fields.json"), "not-json").unwrap();
        let mut credit =
            crate::case_cache::reserve_work(crate::case_work_budget::global(), 0).unwrap();
        let malformed =
            missing_context("local", r#"{"id":"local"}"#, assets.path(), &mut credit).unwrap();
        assert_eq!(malformed.legacy_raw, Some(Value::String("not-json".into())));
        assert_eq!(
            malformed.migration_diagnostics[0].code,
            "invalid_legacy_document"
        );
    }
    #[test]
    fn verified_adoption_preserves_envelopes_freezes_ambiguity_and_replays_without_old_writer_loss()
    {
        let root = legacy();
        let backup = recovery(root.path());
        let original = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let effective = crate::analysis_context::read(&original, "c").unwrap();
        drop(original);
        let result = adopt(root.path(), "adopt-one", &backup).unwrap();
        assert_eq!(result.receipt.adopted.len(), 1);
        assert!(!result.receipt.replayed);
        let loaded = load_view(root.path()).unwrap();
        let case = &loaded.document().cases[0];
        assert_eq!(case["timeline"]["annotations"][0]["anchor"], "e:i:same");
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let (map, _credit) = anchor_store::read(&conn, &result.receipt.adopted[0]).unwrap();
        assert_eq!(
            map.blocked_event("e:i:same").unwrap().state,
            crate::case_evidence_anchors::BlockedState::Ambiguous
        );
        let body: String = conn
            .query_row("SELECT body FROM cases WHERE id='c'", [], |row| row.get(0))
            .unwrap();
        for exact in ["18446744073709551615", "\"future\":1.0", "\"future\":-0.0"] {
            assert!(body.contains(exact));
        }
        assert!(conn
            .execute("UPDATE cases SET body='{}' WHERE id='c'", [])
            .is_err());
        assert_eq!(
            crate::analysis_context::read(&conn, "c").unwrap(),
            effective
        );
        let replay = adopt(root.path(), "adopt-one", &backup).unwrap();
        assert!(replay.receipt.replayed);
        assert!(!replay.receipt.reconcile_required);
        assert_eq!(replay.receipt.after, result.receipt.after);
        backup.validate().unwrap();
    }
    #[test]
    fn context_only_divergence_refuses_adoption_without_installing_native_schema() {
        let root = legacy();
        let backup = recovery(root.path());
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        conn.execute(
            "UPDATE case_analysis SET body=json_set(body,'$.configRevision',1) WHERE case_id='c'",
            [],
        )
        .unwrap();
        assert!(adopt(root.path(), "adopt-stale", &backup)
            .err()
            .unwrap()
            .starts_with("CASE_EVIDENCE_ADOPTION_CHANGED"));
        assert!(!db::exists(&conn, "native_evidence_store").unwrap());
        assert_eq!(snapshot_authority(&conn).unwrap().body_revision, "1");
    }
    #[test]
    fn adoption_sql_failure_rolls_back_mirrors_schema_and_permits_then_retries() {
        let root = legacy();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let before: String = conn
            .query_row("SELECT body FROM cases WHERE id='c'", [], |row| row.get(0))
            .unwrap();
        conn.execute_batch("CREATE TRIGGER fixture_abort BEFORE UPDATE OF body ON cases BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        let backup = recovery(root.path());
        assert!(adopt(root.path(), "adopt-retry", &backup).is_err());
        assert!(!db::exists(&conn, "native_evidence_store").unwrap());
        assert_eq!(
            conn.query_row::<String, _, _>("SELECT body FROM cases WHERE id='c'", [], |row| row
                .get(0))
                .unwrap(),
            before
        );
        conn.execute_batch("DROP TRIGGER fixture_abort").unwrap();
        // The injected trigger is outside the observed body/context authority;
        // removing this test hook permits the identical snapshot to publish.
        let result = adopt(root.path(), "adopt-retry", &backup).unwrap();
        assert!(!result.receipt.replayed);
        assert_eq!(
            conn.query_row::<u64, _, _>(
                "SELECT count(*) FROM native_evidence_permits",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            0
        );
    }
}
