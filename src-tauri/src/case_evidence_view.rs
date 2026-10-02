//! Bounded management views. Native record envelopes never enter this DTO.
use super::*;
use crate::case_work_budget::Lease;
use rusqlite::{Connection, OptionalExtension};
use std::{io::Write, path::Path};
const INVALID: &str = "CASE_EVIDENCE_AUTHORITY: Reabra a investigação antes de continuar.";
const LIMIT: &str =
    "CASE_EVIDENCE_VIEW_LIMIT: A visualização excede o orçamento; os originais foram preservados.";

/// Keep the admitted management allocations charged until IPC serialization
/// finishes. The wire is exactly CaseViewDocument, with no allocator metadata.
pub(crate) struct LoadedView {
    document: CaseViewDocument,
    _credits: Vec<Lease>,
}
impl LoadedView {
    pub(crate) fn document(&self) -> &CaseViewDocument {
        &self.document
    }
}
impl Serialize for LoadedView {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.document.serialize(serializer)
    }
}
pub(super) fn json_size(value: &impl Serialize, limit: usize) -> Result<usize, String> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(input.len())
                .filter(|bytes| *bytes <= self.limit)
                .ok_or_else(|| std::io::Error::other(LIMIT))?;
            Ok(input.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|e| e.to_string())?;
    Ok(counter.bytes)
}
fn add_credit(lease: &mut Lease, bytes: usize) -> Result<(), String> {
    let extra = crate::case_cache::reserve_work(lease.pool(), bytes)?;
    lease.merge(extra)
}
fn context(conn: &Connection, case: &mut Value, lease: &mut Lease) -> Result<(), String> {
    let id = case.get("id").and_then(Value::as_str).ok_or(INVALID)?;
    let bytes: usize = conn
        .query_row(
            "SELECT octet_length(body) FROM case_analysis WHERE case_id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if bytes > 4 << 20 {
        return Err(LIMIT.into());
    }
    let _scratch = crate::case_cache::reserve_work(lease.pool(), bytes.checked_mul(3).and_then(|n| n.checked_add(64 << 10)).ok_or(LIMIT)?)?;
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(body)=?2 THEN body END FROM case_analysis WHERE case_id=?1",rusqlite::params![id,bytes],|row|row.get(0)).map_err(|e|e.to_string())?;
    let raw = RawJson::checked_with_limit(&text, 4 << 20)?;
    let mut projected = String::from("{");
    let mut first = true;
    let mut interpretation_seen = false;
    let mut members = raw.members()?;
    while let Some(field) = members.next()? {
        if field.key.len() <= 128 && field.key()? == "interpretation" {
            if interpretation_seen { return Err("CASE_EVIDENCE_DUPLICATE_KEY".into()); }
            interpretation_seen = true;
            continue;
        }
        if !first { projected.push(','); }
        first = false;
        projected.push_str(field.key); projected.push(':'); projected.push_str(field.value.get());
    }
    projected.push('}');
    let raw = RawJson::checked_with_limit(&projected, 4 << 20)?;
    add_credit(lease, projected.len().checked_mul(2).ok_or(LIMIT)?)?;
    add_credit(lease, decode::preflight_metadata(raw)?.materialization_credit)?;
    let value = decode::MetadataDecoder::new().value(raw)?;
    if value.get("caseId").and_then(Value::as_str) != Some(id) {
        return Err(INVALID.into());
    }
    case.as_object_mut()
        .ok_or(INVALID)?
        .insert("analysisContext".into(), value);
    Ok(())
}
fn unavailable(
    owner: &EvidenceOwner,
    code: &str,
    count: Option<u32>,
) -> Result<(Value, CaseEvidenceState), String> {
    if code.is_empty() || code.len() > 256 {
        return Err(INVALID.into());
    }
    let stub = UnavailableCaseStub {
        kind: UnavailableCaseKind::PreservedCaseUnavailable,
        id: owner.case_id.clone(),
        code: code.into(),
    };
    let diagnostic=PreservedCaseDiagnostic{case_id:owner.case_id.clone(),owner:Some(owner.clone()),code:code.into(),message:"Os registros originais estão preservados. Este Caso não pode ser interpretado com os limites ou formato atuais.".into(),preserved_count:count,readiness:CaseReadiness::PreservedOnly};
    Ok((
        serde_json::to_value(stub).map_err(|e| e.to_string())?,
        CaseEvidenceState::Unavailable(diagnostic),
    ))
}
pub(super) fn case_view(
    conn: &Connection,
    root: &Path,
    owner: &EvidenceOwner,
) -> Result<(Value, CaseEvidenceState, Lease), String> {
    // A stored native-issued unavailable marker contains no record data. A
    // fabricated/extended marker is rejected rather than becoming a new Case.
    let small:Option<String>=conn.query_row("SELECT CASE WHEN octet_length(metadata)<=8192 THEN metadata END FROM native_evidence_cases WHERE case_id=?1 AND analysis_id=?2",rusqlite::params![owner.case_id,owner.analysis_id],|row|row.get(0)).map_err(|e|e.to_string())?;
    if let Some(text) = small {
        if let Ok(stub) = serde_json::from_str::<UnavailableCaseStub>(&text) {
            authority::require_owner(conn, owner)?;
            if stub.id != owner.case_id {
                return Err(INVALID.into());
            }
            let (view, state) = unavailable(owner, &stub.code, None)?;
            let lease =
                crate::case_cache::reserve_work(crate::case_work_budget::global(), 32 << 10)?;
            return Ok((view, state, lease));
        }
    }
    let (verified, mut credit) =
        authority::read_case(conn, root, owner, authority::OpenScope::Metadata)?;
    let mut state = verified.evidence_state()?;
    credit.merge(anchor_store::decorate(conn, owner, &mut state)?)?;
    let mut view = verified.view_metadata()?;
    context(conn, &mut view, &mut credit)?;
    if let Err(error) = metadata_transport_safe(&view) {
        if error != "CASE_EVIDENCE_METADATA_NUMBER" {
            return Err(error);
        }
        let count = match &state {
            CaseEvidenceState::Ready(summary) => Some(summary.preserved_count),
            _ => None,
        };
        let (view, state) = unavailable(owner, "metadata_number_unavailable", count)?;
        return Ok((view, state, credit));
    }
    Ok((view, state, credit))
}

/// Exact serialized envelope admission on an existing publication transaction.
/// Only one decoded Case is retained at a time; no all-Case record view exists.
pub(super) fn validate_size(conn: &Connection, root: &Path, limit: usize) -> Result<(), String> {
    let store = db::stamp(conn)?;
    let active:Option<Option<String>>=conn.query_row("SELECT CASE WHEN octet_length(value)<=8192 THEN value END FROM metadata WHERE key='active'",[],|row|row.get(0)).optional().map_err(|e|e.to_string())?;
    let active = match active {
        Some(Some(value)) => serde_json::from_str::<Option<String>>(&value).map_err(|_| INVALID)?,
        Some(None) => return Err(LIMIT.into()),
        None => None,
    };
    let envelope = CaseViewDocument {
        evidence_view_version: SCHEMA_VERSION,
        store: store.clone(),
        active,
        cases: Vec::new(),
        case_evidence: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut total = json_size(&envelope, limit)?;
    let mut count = 0usize;
    let mut diagnostics = 0usize;
    let mut stmt=conn.prepare("SELECT CASE WHEN octet_length(c.id)<=4096 THEN c.id END,CASE WHEN octet_length(n.analysis_id)=36 THEN n.analysis_id END FROM cases c LEFT JOIN native_evidence_cases n ON n.case_id=c.id ORDER BY c.position,c.id").map_err(|e|e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        if count >= 1000 {
            return Err(LIMIT.into());
        }
        let owner = EvidenceOwner {
            store_id: store.store_id.clone(),
            case_id: row.get(0).map_err(|e| e.to_string())?,
            analysis_id: row.get(1).map_err(|e| e.to_string())?,
        };
        let (case, state, _credit) = case_view(conn, root, &owner)?;
        let mut add = json_size(&case, limit)?
            .checked_add(json_size(&state, limit)?)
            .ok_or(LIMIT)?;
        if count > 0 {
            add = add.checked_add(2).ok_or(LIMIT)?;
        }
        if let CaseEvidenceState::Unavailable(diagnostic) = &state {
            add = add
                .checked_add(json_size(diagnostic, limit)?)
                .ok_or(LIMIT)?;
            if diagnostics > 0 {
                add = add.checked_add(1).ok_or(LIMIT)?;
            }
            diagnostics += 1;
        }
        count += 1;
        total = total
            .checked_add(add)
            .filter(|n| *n <= limit)
            .ok_or(LIMIT)?;
    }
    Ok(())
}

pub(crate) fn load_view(root: &Path) -> Result<LoadedView, String> {
    bootstrap(root)?;
    crate::operations::check()?;
    let root_guard = crate::case_recovery::RootLease::shared(root, &prepare::root_work())?;
    {
        let mut migration = authority::connect_readwrite(root)?;
        crate::case_interpretation::initialize_native(&mut migration, root)?;
    }
    let conn = authority::connect_readonly(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    if !db::exists(&conn, "native_evidence_store")? {
        return Err("CASE_EVIDENCE_MIGRATION_REQUIRED: Prepare e verifique a recuperação antes de abrir a visualização nativa.".into());
    }
    let store = db::stamp(&conn)?;
    let active:Option<(usize,Option<String>)>=conn.query_row("SELECT octet_length(value),CASE WHEN octet_length(value)<=8192 THEN value END FROM metadata WHERE key='active'",[],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|e.to_string())?;
    let active = match active {
        Some((bytes, Some(value))) if bytes <= 8192 => {
            serde_json::from_str::<Option<String>>(&value).map_err(|_| INVALID)?
        }
        Some(_) => return Err(INVALID.into()),
        None => None,
    };
    if active.as_ref().is_some_and(|id| id.len() > 4096) {
        return Err(INVALID.into());
    }
    let mut document = CaseViewDocument {
        evidence_view_version: SCHEMA_VERSION,
        store: store.clone(),
        active,
        cases: Vec::new(),
        case_evidence: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut credits = Vec::new();
    let mut bytes = 0usize;
    let mut stmt=conn.prepare("SELECT CASE WHEN octet_length(c.id)<=4096 THEN c.id END,CASE WHEN octet_length(n.analysis_id)<=36 THEN n.analysis_id END FROM cases c LEFT JOIN native_evidence_cases n ON n.case_id=c.id ORDER BY c.position,c.id").map_err(|e|e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        crate::operations::check()?;
        if document.cases.len() >= 1000 {
            return Err(LIMIT.into());
        }
        let case_id: String = row.get(0).map_err(|e| e.to_string())?;
        let analysis_id: Option<String> = row.get(1).map_err(|e| e.to_string())?;
        let analysis_id = analysis_id.ok_or("CASE_EVIDENCE_MIGRATION_REQUIRED")?;
        let owner = EvidenceOwner {
            store_id: store.store_id.clone(),
            case_id,
            analysis_id,
        };
        let (case, state, credit) = case_view(&conn, root, &owner)?;
        bytes = bytes
            .checked_add(json_size(&case, VIEW_DOCUMENT_BYTES)?)
            .ok_or(LIMIT)?;
        if bytes > VIEW_DOCUMENT_BYTES {
            return Err(LIMIT.into());
        }
        if let CaseEvidenceState::Unavailable(diagnostic) = &state {
            document.diagnostics.push(diagnostic.clone());
        }
        document.cases.push(case);
        document.case_evidence.push(state);
        credits.push(credit);
    }
    drop(rows);
    drop(stmt);
    // Count the complete envelope too, so every successfully loaded document
    // can be sent unchanged within the public documentJson byte ceiling.
    json_size(&document, VIEW_DOCUMENT_BYTES)?;
    if document.active.as_ref().is_some_and(|active| {
        !document
            .cases
            .iter()
            .any(|case| case.get("id").and_then(Value::as_str) == Some(active))
    }) {
        return Err(INVALID.into());
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    root_guard.validate()?;
    crate::operations::check()?;
    Ok(LoadedView {
        document,
        _credits: credits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_publication_size_matches_the_complete_management_wire() {
        let (root, _, _) = authority::tests::fixture();
        let view = load_view(root.path()).unwrap();
        let exact = serde_json::to_vec(&view).unwrap().len();
        let conn = authority::connect_readonly(root.path()).unwrap();
        validate_size(&conn, root.path(), exact).unwrap();
        assert!(validate_size(&conn, root.path(), exact - 1).is_err());
    }
    #[test]
    fn management_view_contains_references_and_current_context_with_retained_credit() {
        let (root, request, _) = authority::tests::fixture();
        let view = load_view(root.path()).unwrap();
        assert_eq!(view.document.store.identity(), request.store);
        assert_eq!(view.document.cases.len(), 1);
        let case = &view.document.cases[0];
        assert_eq!(
            case["items"][0]["rows"]["kind"],
            "native_evidence_container"
        );
        assert_eq!(case["items"][0]["rows"]["preservedCount"], 1);
        assert!(case["items"][0]["rows"].get("message").is_none());
        assert_eq!(case["analysisContext"]["visibilityRevision"], 3);
        assert!(view.document.diagnostics.is_empty());
        assert!(view._credits[0].bytes() > 0);
        let serialized = serde_json::to_value(&view).unwrap();
        assert!(serialized.get("_credits").is_none());
        assert_eq!(serialized["cases"][0]["id"], "c");
    }
    #[test]
    fn unsafe_metadata_emits_a_preserved_stub_without_changing_native_rows() {
        let (root, _, _) = authority::tests::fixture();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let before: String = conn
            .query_row("SELECT body FROM cases WHERE id='c'", [], |r| r.get(0))
            .unwrap();
        let text: String = conn
            .query_row(
                "SELECT metadata FROM native_evidence_cases WHERE case_id='c'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut metadata: Value = serde_json::from_str(&text).unwrap();
        metadata["opaqueMetadata"] = serde_json::json!(18446744073709551615u64);
        conn.execute(
            "UPDATE native_evidence_cases SET metadata=?1 WHERE case_id='c'",
            [metadata.to_string()],
        )
        .unwrap();
        let view = load_view(root.path()).unwrap();
        let stub: UnavailableCaseStub =
            serde_json::from_value(view.document.cases[0].clone()).unwrap();
        assert_eq!(stub.code, "metadata_number_unavailable");
        assert_eq!(stub.id, "c");
        assert_eq!(view.document.diagnostics.len(), 1);
        assert!(matches!(
            &view.document.case_evidence[0],
            CaseEvidenceState::Unavailable(_)
        ));
        let after: String = conn
            .query_row("SELECT body FROM cases WHERE id='c'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
        let stored: String = conn
            .query_row(
                "SELECT metadata FROM native_evidence_cases WHERE case_id='c'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored.contains("18446744073709551615"));
    }
    #[test]
    fn management_fraction_is_exact_and_full_envelope_size_is_counted() {
        let (root, _, _) = authority::tests::fixture();
        let conn = Connection::open(root.path().join("investigations.sqlite3")).unwrap();
        let text: String = conn
            .query_row(
                "SELECT metadata FROM native_evidence_cases WHERE case_id='c'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut metadata: Value = serde_json::from_str(&text).unwrap();
        metadata["fraction"] = Value::Number(
            serde_json::Number::from_f64(f64::from_bits(0x3e5b597464455d8a)).unwrap(),
        );
        conn.execute(
            "UPDATE native_evidence_cases SET metadata=?1 WHERE case_id='c'",
            [metadata.to_string()],
        )
        .unwrap();
        let view = load_view(root.path()).unwrap();
        assert_eq!(
            view.document.cases[0]["fraction"]
                .as_f64()
                .unwrap()
                .to_bits(),
            0x3e5b597464455d8a
        );
        let actual = serde_json::to_vec(&view).unwrap().len();
        assert_eq!(json_size(&view, actual).unwrap(), actual);
        assert!(json_size(&view, actual - 1).is_err());
    }
    #[test]
    fn six_large_case_settings_do_not_expand_management_or_metadata_save_payloads() {
        let (root, _, _) = authority::tests::fixture();
        let loaded = load_view(root.path()).unwrap();
        let mut document = serde_json::to_value(&loaded).unwrap();
        for n in 0..6 { document["cases"].as_array_mut().unwrap().push(serde_json::json!({"id":format!("large-{n}"),"name":"Large settings","items":[]})); }
        save_view(root.path(), &SaveViewRequest { request_id: "large-settings-create".into(), expected_store: loaded.document().store.clone(), document_json: document.to_string() }).unwrap();
        drop(loaded);
        for n in 0..6 {
            let id = format!("large-{n}");
            let conn = crate::case_store::context_connection(root.path()).unwrap();
            let snapshot = crate::analysis_context::read(&conn, &id).unwrap(); drop(conn);
            crate::case_interpretation::update_at(root.path(), &snapshot.identity(), |settings| {
                settings.codes = serde_json::from_value(serde_json::json!({"application":{"1":{"name":format!("owner-{n}"),"description":"x".repeat(2_800_000)}}})).unwrap(); Ok(())
            }).unwrap();
        }
        let loaded = load_view(root.path()).unwrap();
        let mut document = serde_json::to_value(&loaded).unwrap();
        assert!(document["cases"].as_array().unwrap().iter().all(|case| case["analysisContext"].get("interpretation").is_none()));
        assert!(document.to_string().len() < 128 << 10, "the all-Case DTO excludes catalog payloads");
        document["cases"][1]["name"] = serde_json::json!("Changed authored metadata");
        save_view(root.path(), &SaveViewRequest { request_id: "large-settings-metadata".into(), expected_store: loaded.document().store.clone(), document_json: document.to_string() }).unwrap();
        for n in 0..6 {
            let conn = crate::case_store::context_connection(root.path()).unwrap();
            let snapshot = crate::analysis_context::read(&conn, &format!("large-{n}")).unwrap();
            let settings = snapshot.interpretation.unwrap();
            assert_eq!(settings.codes.sources["application"]["1"].description.len(), 2_800_000);
            assert_eq!(settings.codes.sources["application"]["1"].name, format!("owner-{n}"));
        }
    }

}
