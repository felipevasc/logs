//! Read-only portable capture. IPC supplies authored metadata and issued native
//! references; immutable record envelopes and exact unavailable bodies come
//! exclusively from the admitted SQLite snapshot and native files.
use super::*;
use crate::{
    analysis_context::Snapshot, case_evidence_anchors::AnchorMap, case_work_budget::Lease,
};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
};
const INVALID: &str =
    "CASE_EVIDENCE_EXPORT_CHANGED: O Caso mudou; reabra a investigação antes de exportar.";
const LIMIT: &str =
    "CASE_EVIDENCE_EXPORT_LIMIT: A exportação excede o orçamento; o destino foi preservado.";

enum ExportBody {
    Native(VerifiedCase),
    Preserved(OriginalBody),
}
pub(crate) struct ExportCase {
    owner: EvidenceOwner,
    body: ExportBody,
    anchors: AnchorMap,
    preserved_provenance: Option<VerifiedCase>,
    _credit: Vec<Lease>,
}
impl ExportCase {
    pub(crate) fn owner(&self) -> &EvidenceOwner {
        &self.owner
    }
    pub(crate) fn native(&self) -> Option<&VerifiedCase> {
        match &self.body {
            ExportBody::Native(case) => Some(case),
            _ => self.preserved_provenance.as_ref(),
        }
    }
    pub(crate) fn original(&self) -> Option<&str> {
        match &self.body {
            ExportBody::Preserved(body) => Some(body.text()),
            _ => None,
        }
    }
    pub(crate) fn anchors(&self) -> &AnchorMap {
        &self.anchors
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(case) = self.native() {
            case.validate()?;
        }
        Ok(())
    }
    pub(crate) fn write_to_with_parts(
        &self,
        writer: &mut dyn Write,
        snapshot: &Snapshot,
        envelope: impl FnMut(&str, &mut dyn Write) -> Result<(), String>,
        mut metadata: impl FnMut(&str, &Value, &mut dyn Write) -> Result<(), String>,
        mut original_field: impl FnMut(&str, &str, &mut dyn Write) -> Result<(), String>,
    ) -> Result<(), String> {
        match &self.body {
            ExportBody::Native(case) => case.write_to_with_parts(writer, false, envelope, metadata),
            ExportBody::Preserved(body) => {
                writer.write_all(b"{").map_err(|e| e.to_string())?;
                let mut fields = checked_document(body.text())?.members()?;
                let mut first = true;
                while let Some(field) = fields.next()? {
                    crate::operations::check()?;
                    if field.key.len() <= 128
                        && matches!(
                            field.key()?.as_str(),
                            "analysisContext" | "portableImportToken"
                        )
                    {
                        continue;
                    }
                    if !first {
                        writer.write_all(b",").map_err(|e| e.to_string())?;
                    }
                    first = false;
                    writer
                        .write_all(field.key.as_bytes())
                        .and_then(|_| writer.write_all(b":"))
                        .map_err(|e| e.to_string())?;
                    original_field(field.key, field.value.get(), writer)?;
                }
                if !first {
                    writer.write_all(b",").map_err(|e| e.to_string())?;
                }
                writer
                    .write_all(b"\"analysisContext\":")
                    .map_err(|e| e.to_string())?;
                // Snapshot serialization is admitted at capture time. No record
                // tree is materialized to attach its authoritative context.
                let value = serde_json::to_value(snapshot).map_err(|e| e.to_string())?;
                metadata("analysisContext", &value, writer)?;
                writer.write_all(b"}").map_err(|e| e.to_string())
            }
        }
    }
}
pub(crate) struct ExportCapture<T> {
    root: PathBuf,
    root_guard: crate::case_recovery::RootLease,
    stamp: StoreStamp,
    cases: Vec<ExportCase>,
    snapshots: Vec<Snapshot>,
    sidecars: T,
    _credit: Lease,
}
impl<T> ExportCapture<T> {
    pub(crate) fn cases(&self) -> &[ExportCase] {
        &self.cases
    }
    pub(crate) fn snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }
    pub(crate) fn sidecars(&self) -> &T {
        &self.sidecars
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.root_guard.validate()?;
        let conn = authority::connect_readonly(&self.root)?;
        conn.execute_batch("BEGIN DEFERRED")
            .map_err(|e| e.to_string())?;
        db::require_stamp(&conn, &self.stamp)?;
        for (case, snapshot) in self.cases.iter().zip(&self.snapshots) {
            authority::require_owner(&conn, &case.owner)?;
            if crate::analysis_context::read(&conn, &case.owner.case_id)? != *snapshot {
                return Err(INVALID.into());
            }
            case.validate()?;
        }
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        self.root_guard.validate()
    }
}
fn context(conn: &Connection, id: &str, credit: &mut Lease) -> Result<Snapshot, String> {
    let bytes: usize = conn
        .query_row(
            "SELECT octet_length(body) FROM case_analysis WHERE case_id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if bytes > 4 << 20 {
        return Err(LIMIT.into());
    }
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        bytes.checked_mul(3).ok_or(LIMIT)?,
    )?)?;
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(body)=?2 THEN body END FROM case_analysis WHERE case_id=?1",params![id,bytes],|r|r.get(0)).map_err(|e|e.to_string())?;
    let raw = checked_document(&text)?;
    let plan = preflight_value(raw)?;
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        plan.materialization_credit.checked_mul(2).ok_or(LIMIT)?,
    )?)?;
    let snapshot: Snapshot =
        native_snapshot_from_value(materialize_value(raw, plan)?).map_err(|e| e.to_string())?;
    if snapshot.case_id != id {
        return Err(INVALID.into());
    }
    crate::analysis_context::validate_snapshot_size(&snapshot)?;
    Ok(snapshot)
}
/// A JS-unavailable Case can still have fully interpretable native bindings.
/// Reopen those bindings using its checked exact mirror solely for identity
/// mapping; the export body itself continues to be emitted from OriginalBody.
fn preserved_native_provenance(
    conn: &Connection,
    root: &Path,
    owner: &EvidenceOwner,
    body: &str,
    credits: &mut Vec<Lease>,
) -> Result<Option<VerifiedCase>, String> {
    let bytes:usize=conn.query_row("SELECT octet_length(bindings) FROM native_evidence_cases WHERE case_id=?1 AND analysis_id=?2",params![owner.case_id,owner.analysis_id],|r|r.get(0)).map_err(|e|e.to_string())?;
    if bytes > MANIFEST_BYTES as usize {
        return Err(LIMIT.into());
    }
    credits.push(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        bytes.checked_mul(3).ok_or(LIMIT)?,
    )?);
    let text:String=conn.query_row("SELECT CASE WHEN octet_length(bindings)=?3 THEN bindings END FROM native_evidence_cases WHERE case_id=?1 AND analysis_id=?2",params![owner.case_id,owner.analysis_id,bytes],|r|r.get(0)).map_err(|e|e.to_string())?;
    let plan = preflight_value(checked_document(&text)?)?;
    credits.push(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        plan.materialization_credit.checked_mul(2).ok_or(LIMIT)?,
    )?);
    let bindings: Vec<authority::Binding> =
        serde_json::from_value(materialize_value(checked_document(&text)?, plan)?)
            .map_err(|e| e.to_string())?;
    if bindings.len() > 10_000 {
        return Err(LIMIT.into());
    }
    let estimate = match document::preflight_case(body) {
        Ok(bytes) => bytes,
        Err(_) if bindings.is_empty() => return Ok(None),
        Err(error) => return Err(error),
    };
    credits.push(crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        estimate,
    )?);
    let extracted = match extract_case(body) {
        Ok(value) => value,
        Err(_) if bindings.is_empty() => return Ok(None),
        Err(error) => return Err(error),
    };
    if extracted.containers.len() != bindings.len() {
        return Err(INVALID.into());
    }
    let mut containers = Vec::new();
    let mut budget = crate::case_evidence_budget::StorageBudget::begin(0, 0)?;
    for binding in bindings {
        if binding.reference.owner != *owner
            || !extracted.containers.iter().any(|c| {
                c.location == binding.location
                    && c.records.len() == binding.reference.member_count as usize
            })
        {
            return Err(INVALID.into());
        }
        containers.push((
            binding.location,
            authority::container(conn, root, &binding.reference, &mut budget)?,
        ));
    }
    credits.push(budget.finish());
    Ok(Some(VerifiedCase::from_parts(
        extracted.metadata,
        owner.clone(),
        containers,
    )?))
}
pub(crate) fn capture_export<T>(
    root: &Path,
    request: &SaveViewRequest,
    sidecars: impl FnOnce(&Connection, &[Snapshot]) -> Result<T, String>,
) -> Result<ExportCapture<T>, String> {
    let mut credit = commit::request_credit(request)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let root_guard = crate::case_recovery::RootLease::shared(&root, &prepare::root_work())?;
    let conn = authority::connect_readonly(&root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    db::require_stamp(&conn, &request.expected_store)?;
    let document = request.parse_document()?;
    if document.evidence_view_version != SCHEMA_VERSION
        || document.cases.is_empty()
        || document.cases.len() > 1000
    {
        return Err(INVALID.into());
    }
    let mut ids = HashSet::new();
    let mut cases = Vec::new();
    let mut snapshots = Vec::new();
    for value in document.cases {
        crate::operations::check()?;
        let id = commit::case_id(&value)?.to_owned();
        if !ids.insert(id.clone()) {
            return Err(INVALID.into());
        }
        let snapshot = context(&conn, &id, &mut credit)?;
        let owner = EvidenceOwner {
            store_id: request.expected_store.store_id.clone(),
            case_id: id,
            analysis_id: snapshot.analysis_id.clone(),
        };
        authority::require_owner(&conn, &owner)?;
        let (mut anchors, anchor_credit) = anchor_store::read(&conn, &owner)?;
        let mut credits = vec![anchor_credit];
        let mut preserved_provenance = None;
        let body = if value.get("kind").and_then(Value::as_str)
            == Some("preserved_case_unavailable")
        {
            let (current, state, view_credit) = view::case_view(&conn, &root, &owner)?;
            if current != value || !matches!(state, CaseEvidenceState::Unavailable(_)) {
                return Err(INVALID.into());
            }
            credits.push(view_credit);
            let body = read_case_body(&conn, &owner.case_id)?;
            let hash:String=conn.query_row("SELECT CASE WHEN octet_length(body_sha256)=64 THEN body_sha256 END FROM native_evidence_cases WHERE case_id=?1 AND analysis_id=?2",params![owner.case_id,owner.analysis_id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if format!("{:x}", Sha256::digest(body.text().as_bytes())) != hash {
                return Err(INVALID.into());
            }
            checked_document(body.text())?.members()?;
            preserved_provenance =
                preserved_native_provenance(&conn, &root, &owner, body.text(), &mut credits)?;
            ExportBody::Preserved(body)
        } else {
            let submitted = commit::submitted_case(value)?;
            let (current, current_credit) =
                authority::read_case(&conn, &root, &owner, authority::OpenScope::All)?;
            credits.push(current_credit);
            let mut metadata = submitted.metadata;
            let mut containers = Vec::new();
            for (location, reference) in submitted.references {
                let EvidenceReference::Committed(reference) = reference else {
                    return Err("CASE_EVIDENCE_EXPORT_PENDING: Salve o Caso antes de exportar evidências preparadas.".into());
                };
                if reference.owner != owner {
                    return Err(INVALID.into());
                }
                let (_, container) = current
                    .containers()
                    .find(|(_, c)| c.reference() == &reference)
                    .ok_or(INVALID)?;
                containers.push((location, std::sync::Arc::clone(container)));
            }
            let bindings = containers
                .iter()
                .filter_map(|(location, container)| match location {
                    ContainerLocation::ItemRows { index } => {
                        Some(crate::case_evidence_anchors::ItemBinding {
                            item_index: *index,
                            container_id: container.reference().container_id.clone(),
                            member_count: container.reference().member_count,
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let frozen = crate::case_evidence_anchors::freeze(
                &mut metadata,
                &bindings,
                &anchors,
                |visit| {
                    for (location, container) in &containers {
                        if let ContainerLocation::ItemRows { index } = location {
                            container.visit_envelopes_with_origin(
                                0..container.reference().member_count as usize,
                                |member, origin, text| {
                                    visit(*index, origin, member, text)?;
                                    Ok(Visit::Continue)
                                },
                            )?;
                        }
                    }
                    Ok(())
                },
            )?;
            anchors = frozen.anchors;
            credits.push(frozen.credit);
            metadata.as_object_mut().ok_or(INVALID)?.insert(
                "analysisContext".into(),
                serde_json::to_value(&snapshot).map_err(|e| e.to_string())?,
            );
            ExportBody::Native(VerifiedCase::from_parts(
                metadata,
                owner.clone(),
                containers,
            )?)
        };
        cases.push(ExportCase {
            owner,
            body,
            anchors,
            preserved_provenance,
            _credit: credits,
        });
        snapshots.push(snapshot);
    }
    let sidecars = sidecars(&conn, &snapshots)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    let captured = ExportCapture {
        root,
        root_guard,
        stamp: request.expected_store.clone(),
        cases,
        snapshots,
        sidecars,
        _credit: credit,
    };
    captured.validate()?;
    Ok(captured)
}

/// Build a sparse transfer map by manifest identity. Record contents never
/// participate in correspondence, including when two records are byte-equal.
pub(crate) fn capture_transfer_map(
    case_ordinal: u32,
    case: &VerifiedCase,
    anchors: &AnchorMap,
) -> Result<(PortableCaseMap, Lease), String> {
    let targets = crate::case_evidence_anchors::requested_native_targets(case.metadata(), anchors)?;
    let mut credit = targets.credit;
    let bytes = view::json_size(anchors, 1 << 20)?;
    credit.merge(crate::case_cache::reserve_work(
        credit.pool(),
        bytes.checked_mul(4).ok_or(LIMIT)?,
    )?)?;
    let mut map = PortableCaseMap {
        case_ordinal,
        anchors: anchors.clone(),
        containers: Vec::new(),
        members: Vec::new(),
    };
    let mut requested = std::collections::BTreeMap::<&str, std::collections::BTreeSet<&str>>::new();
    for (container, member) in &targets.members {
        requested
            .entry(container.as_str())
            .or_default()
            .insert(member.as_str());
    }
    for (location, container) in case.containers() {
        crate::operations::check()?;
        let id = &container.reference().container_id;
        let mut used = targets.containers.contains(id);
        for (ordinal, member) in container.manifest().members.iter().enumerate() {
            if requested
                .get(id.as_str())
                .is_some_and(|members| members.contains(member.occurrence_id.as_str()))
            {
                credit.merge(crate::case_cache::reserve_work(credit.pool(), 512)?)?;
                map.members.push(PortableMemberMap {
                    container_id: id.clone(),
                    occurrence_id: member.occurrence_id.clone(),
                    record_ordinal: u32::try_from(ordinal).map_err(|_| LIMIT)?,
                });
                used = true;
            }
        }
        if used {
            credit.merge(crate::case_cache::reserve_work(credit.pool(), 512)?)?;
            map.containers.push(PortableContainerMap {
                container_id: id.clone(),
                location: location.clone(),
            });
        }
    }
    map.validate()?;
    Ok((map, credit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case_portable_native::{self as portable, ExportRequest, ImportRequest};
    use serde_json::json;
    fn imported_fixture() -> (tempfile::TempDir, String, String) {
        let (root, _, _) = authority::tests::fixture();
        let external = tempfile::tempdir().unwrap();
        let source = external.path().join("source.json");
        let envelope = r#"{ "id":9,"fields":{"large":18446744073709551615,"near":18446744073709551614,"one":1.00,"negative":-0.0,"tiny":2.547114365375239e-8},"future":{"x":1.000} }"#;
        let source_text = format!(
            r#"{{"schemaVersion":2,"active":"foreign","cases":[{{"id":"foreign","name":"Exact Case","items":[{{"id":"item","rows":[{envelope},{envelope}]}}],"timeline":{{"annotations":[{{"id":"note","anchor":"e:item:9","text":"ambiguous stays frozen"}}]}}}}]}}"#
        );
        std::fs::write(&source, source_text).unwrap();
        let store = load_view(root.path()).unwrap().document().store.clone();
        let imported = portable::import_at(
            root.path(),
            ImportRequest {
                store,
                path: source.to_string_lossy().into(),
                request_id: "fixture-import".into(),
            },
        )
        .unwrap();
        (root, imported.imported_case_id.unwrap(), envelope.into())
    }
    fn export_request(root: &Path, id: &str, path: &Path) -> ExportRequest {
        let loaded = load_view(root).unwrap();
        let mut value = serde_json::to_value(&loaded).unwrap();
        value["cases"]
            .as_array_mut()
            .unwrap()
            .retain(|c| c["id"] == id);
        value["active"] = json!(id);
        ExportRequest {
            store: loaded.document().store.clone(),
            document_json: value.to_string(),
            path: path.to_string_lossy().into(),
            mask: false,
            request_id: uuid::Uuid::new_v4().to_string(),
        }
    }
    fn author_native_anchors(root: &Path, id: &str) -> (String, String) {
        let loaded = load_view(root).unwrap();
        let mut value = serde_json::to_value(&loaded).unwrap();
        let case = value["cases"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|c| c["id"] == id)
            .unwrap();
        let reference: EvidenceRef =
            serde_json::from_value(case["items"][0]["rows"]["reference"].clone()).unwrap();
        let conn = authority::connect_readonly(root).unwrap();
        let (native, _credit) =
            authority::read_case(&conn, root, &reference.owner, authority::OpenScope::All).unwrap();
        let (_, container) = native.containers().next().unwrap();
        let alias = format!(
            "n:{}:{}",
            reference.container_id,
            container.manifest().members[1].occurrence_id
        );
        case["timeline"]["edits"] = json!({alias.clone():{"title":"Second identical occurrence"}});
        case["caseTrails"] = json!([{"id":"trail","itemRefs":[{"kind":"container","containerId":reference.container_id}]}]);
        case["intel"] = json!({"hypotheses":[{"id":"hypothesis","itemRefs":[{"kind":"container","containerId":reference.container_id}]}]});
        save_view(
            root,
            &SaveViewRequest {
                request_id: "fixture-anchors".into(),
                expected_store: loaded.document().store.clone(),
                document_json: value.to_string(),
            },
        )
        .unwrap();
        (alias, reference.container_id)
    }
    #[test]
    fn native_json_and_licase_roundtrip_exact_duplicates_and_authored_identity() {
        let (source, id, raw) = imported_fixture();
        let (old_alias, old_container) = author_native_anchors(source.path(), &id);
        let external = tempfile::tempdir().unwrap();
        for extension in ["json", "licase"] {
            let path = external.path().join(format!("case.{extension}"));
            let exported =
                portable::export_at(source.path(), export_request(source.path(), &id, &path))
                    .unwrap();
            assert_eq!(exported.records, Some(2));
            assert!(
                crate::case_images::import_at(source.path(), &path).is_err(),
                "legacy reader must reject native transfers"
            );
            let (target, _, _) = authority::tests::fixture();
            let store = load_view(target.path()).unwrap().document().store.clone();
            let request = ImportRequest {
                store,
                path: path.to_string_lossy().into(),
                request_id: format!("native-roundtrip-{extension}"),
            };
            let imported = portable::import_at(target.path(), request.clone()).unwrap();
            let new_id = imported.imported_case_id.as_ref().unwrap();
            assert_ne!(new_id, &id);
            let view = &imported.case_views[0];
            let reference: EvidenceRef =
                serde_json::from_value(view["items"][0]["rows"]["reference"].clone()).unwrap();
            assert_ne!(reference.container_id, old_container);
            let conn = authority::connect_readonly(target.path()).unwrap();
            let (case, _credit) = authority::read_case(
                &conn,
                target.path(),
                &reference.owner,
                authority::OpenScope::All,
            )
            .unwrap();
            let (_, container) = case.containers().next().unwrap();
            let alias = format!(
                "n:{}:{}",
                reference.container_id,
                container.manifest().members[1].occurrence_id
            );
            assert_ne!(alias, old_alias);
            assert_eq!(
                view["timeline"]["edits"][&alias]["title"],
                "Second identical occurrence"
            );
            assert_eq!(view["timeline"]["annotations"][0]["anchor"], "e:item:9");
            assert_eq!(
                view["caseTrails"][0]["itemRefs"][0]["containerId"],
                reference.container_id
            );
            assert_eq!(
                view["intel"]["hypotheses"][0]["itemRefs"][0]["containerId"],
                reference.container_id
            );
            container
                .visit_envelopes(0..2, |_, envelope| {
                    assert_eq!(envelope, raw);
                    Ok(Visit::Continue)
                })
                .unwrap();
            let loaded = load_view(target.path()).unwrap();
            let mut later = serde_json::to_value(&loaded).unwrap();
            later["cases"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|case| case["id"] == *new_id)
                .unwrap()["name"] = json!("Later edit must not replace acknowledged import view");
            save_view(
                target.path(),
                &SaveViewRequest {
                    request_id: format!("later-{extension}"),
                    expected_store: loaded.document().store.clone(),
                    document_json: later.to_string(),
                },
            )
            .unwrap();
            let replay = portable::import_at(target.path(), request.clone()).unwrap();
            assert!(replay.receipt.replayed);
            assert!(replay.receipt.reconcile_required);
            assert_eq!(replay.imported_case_ids, imported.imported_case_ids);
            assert_eq!(replay.case_views, imported.case_views);
            let conn = authority::connect_readonly(target.path()).unwrap();
            assert_eq!(
                conn.query_row::<usize, _, _>("SELECT count(*) FROM cases", [], |r| r.get(0))
                    .unwrap(),
                2
            );
            drop(conn);
            std::fs::write(&path, b"{\"cases\":[{\"id\":\"changed\"}]}").unwrap();
            assert!(portable::import_at(target.path(), request).is_err());
        }
    }
    #[test]
    fn unavailable_numeric_metadata_roundtrips_its_native_anchors_and_exact_records() {
        let (source, _, _) = authority::tests::fixture();
        let external = tempfile::tempdir().unwrap();
        let path = external.path().join("wide.json");
        let old_container = uuid::Uuid::new_v4().to_string();
        let old_member = uuid::Uuid::new_v4().to_string();
        let initial_anchor = format!("n:{old_container}:{old_member}");
        let envelope =
            r#"{"id":1,"future":18446744073709551615,"fraction":2.547114365375239e-8,"one":1.00}"#;
        let input = format!(
            r#"{{"format":"loginsight.native-case","schemaVersion":3,"document":{{"active":"wide","cases":[{{"id":"wide","unsafeMetadata":18446744073709551615,"password":"private-root","futureMetadata":{{"token":"private-nested"}},"items":[{{"id":"item","rows":[{envelope}]}}],"timeline":{{"annotations":[{{"id":"note","anchor":"{initial_anchor}"}}]}}}}]}},"nativeEvidenceMap":{{"schemaVersion":1,"cases":[{{"caseOrdinal":0,"anchors":{{"schemaVersion":1,"eventAliases":[],"itemAliases":[]}},"containers":[{{"containerId":"{old_container}","location":{{"kind":"item_rows","index":0}}}}],"members":[{{"containerId":"{old_container}","occurrenceId":"{old_member}","recordOrdinal":0}}]}}]}}}}"#
        );
        std::fs::write(&path, input).unwrap();
        let store = load_view(source.path()).unwrap().document().store.clone();
        let first = portable::import_at(
            source.path(),
            ImportRequest {
                store,
                path: path.to_string_lossy().into(),
                request_id: "opaque-first".into(),
            },
        )
        .unwrap();
        assert_eq!(first.case_views[0]["kind"], "preserved_case_unavailable");
        assert!(first
            .receipt
            .analysis_contexts
            .iter()
            .all(|s| Some(&s.case_id) != first.imported_case_id.as_ref()));
        let id = first.imported_case_id.as_deref().unwrap();
        let exported = external.path().join("wide-export.json");
        let receipt =
            portable::export_at(source.path(), export_request(source.path(), id, &exported))
                .unwrap();
        assert_eq!(receipt.records, Some(1));
        let text = std::fs::read_to_string(&exported).unwrap();
        assert!(text.contains(envelope));
        assert!(text.contains("18446744073709551615"));
        assert!(!text.contains(&initial_anchor));
        let masked = external.path().join("wide-masked.json");
        let mut masked_request = export_request(source.path(), id, &masked);
        masked_request.mask = true;
        portable::export_at(source.path(), masked_request).unwrap();
        let masked_text = std::fs::read_to_string(masked).unwrap();
        assert!(!masked_text.contains("private-root"));
        assert!(!masked_text.contains("private-nested"));
        assert!(masked_text.contains("18446744073709551615"));
        assert!(masked_text.contains("1.00"));
        let (target, _, _) = authority::tests::fixture();
        let store = load_view(target.path()).unwrap().document().store.clone();
        let second = portable::import_at(
            target.path(),
            ImportRequest {
                store,
                path: exported.to_string_lossy().into(),
                request_id: "opaque-second".into(),
            },
        )
        .unwrap();
        assert_eq!(second.case_views[0]["kind"], "preserved_case_unavailable");
        let second_export = external.path().join("wide-again.json");
        portable::export_at(
            target.path(),
            export_request(
                target.path(),
                second.imported_case_id.as_deref().unwrap(),
                &second_export,
            ),
        )
        .unwrap();
        let second_text = std::fs::read_to_string(second_export).unwrap();
        assert!(second_text.contains(envelope));
        let first_value: Value = serde_json::from_str(&text).unwrap();
        let second_value: Value = serde_json::from_str(&second_text).unwrap();
        assert_ne!(
            first_value["document"]["cases"][0]["timeline"]["annotations"][0]["anchor"],
            second_value["document"]["cases"][0]["timeline"]["annotations"][0]["anchor"]
        );
    }
    #[test]
    fn native_licase_publishes_images_references_and_exclusions_with_the_case() {
        use crate::{case_archive::work, exclusion_store as ledger, reference_store as references};
        let (source, id, _) = imported_fixture();
        let conn = authority::connect_readonly(source.path()).unwrap();
        let mut snapshot = crate::analysis_context::read(&conn, &id).unwrap();
        drop(conn);
        let reference_bytes = b"{\"key\":2.547114365375239e-8,\"value\":18446744073709551615}\n";
        let descriptor = crate::analysis_context::ReferenceDescriptor {
            schema_version: 1,
            interpretation_version: 2,
            id: "exact-reference".into(),
            name: "Exact reference".into(),
            content_sha256: format!("{:x}", Sha256::digest(reference_bytes)),
            format: "jsonl".into(),
            columns: vec!["key".into(), "value".into()],
            key_columns: vec!["key".into()],
            duplicate_policy: "reject".into(),
        };
        references::prepare_jsonl(
            source.path(),
            &references::Owner {
                case_id: id.clone(),
                analysis_id: snapshot.analysis_id.clone(),
            },
            &descriptor,
            &reference_bytes[..],
            references::Limits::default(),
            &|| false,
        )
        .unwrap();
        snapshot.config.references.push(descriptor);
        snapshot.config_revision += 1;
        let conn = Connection::open(source.path().join("investigations.sqlite3")).unwrap();
        conn.execute(
            "UPDATE case_analysis SET body=?1 WHERE case_id=?2",
            params![serde_json::to_string(&snapshot).unwrap(), id],
        )
        .unwrap();
        drop(conn);
        let source_descriptor = ledger::SourceDescriptor {
            version: "file-version".into(),
            record_space: "file-records".into(),
            label: "Original source".into(),
            event_ref_prefix: Some("file-prefix".into()),
        };
        let staged = ledger::stage(
            source.path(),
            ledger::Admission {
                analysis: snapshot.identity(),
                source_receipt: json!({}),
            },
            ledger::Purpose::Exclude,
            &[source_descriptor],
            vec![Ok(ledger::InputMember {
                source_index: 0,
                locator: ledger::Locator::ByteOffset(7),
                event_ref: "file-prefix:7".into(),
            })],
            &ledger::Budget::default(),
            &work(),
        )
        .unwrap();
        ledger::publish(
            source.path(),
            staged,
            "Excluded record",
            "fixture",
            json!({}),
            &ledger::Budget::default(),
            &work(),
        )
        .unwrap();
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let image = image.into_inner();
        let image_id = format!("{:x}", Sha256::digest(&image));
        std::fs::create_dir_all(source.path().join("case-images")).unwrap();
        std::fs::write(source.path().join("case-images").join(&image_id), &image).unwrap();
        let loaded = load_view(source.path()).unwrap();
        let mut view = serde_json::to_value(&loaded).unwrap();
        view["cases"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|c| c["id"] == id)
            .unwrap()["items"][0]["attachments"] = json!([{"id":image_id,"name":"Proof.png"}]);
        save_view(
            source.path(),
            &SaveViewRequest {
                request_id: "add-image".into(),
                expected_store: loaded.document().store.clone(),
                document_json: view.to_string(),
            },
        )
        .unwrap();
        let files = tempfile::tempdir().unwrap();
        let path = files.path().join("whole.licase");
        let json_path = files.path().join("plain.json");
        std::fs::write(&json_path, b"unchanged").unwrap();
        assert!(portable::export_at(
            source.path(),
            export_request(source.path(), &id, &json_path)
        )
        .is_err());
        assert_eq!(std::fs::read(&json_path).unwrap(), b"unchanged");
        portable::export_at(source.path(), export_request(source.path(), &id, &path)).unwrap();
        let (target, _, _) = authority::tests::fixture();
        let stamp = load_view(target.path()).unwrap().document().store.clone();
        let request = ImportRequest {
            store: stamp,
            path: path.to_string_lossy().into(),
            request_id: "whole-roundtrip".into(),
        };
        let before = serde_json::to_value(load_view(target.path()).unwrap()).unwrap();
        let conn = Connection::open(target.path().join("investigations.sqlite3")).unwrap();
        let existing = crate::analysis_context::read(&conn, "c").unwrap();
        ledger::list(target.path(), &existing.identity(), None, 100).unwrap();
        conn.execute_batch("CREATE TRIGGER fixture_reject_import BEFORE INSERT ON exclusion_batches BEGIN SELECT RAISE(ABORT,'fixture sidecar rollback'); END;").unwrap();
        let error = portable::import_at(target.path(), request.clone())
            .err()
            .unwrap();
        assert!(error.contains("fixture sidecar rollback"), "{error}");
        assert_eq!(
            serde_json::to_value(load_view(target.path()).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            conn.query_row::<usize, _, _>(
                "SELECT count(*) FROM native_evidence_receipts WHERE request_id='whole-roundtrip'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            0
        );
        conn.execute_batch("DROP TRIGGER fixture_reject_import")
            .unwrap();
        drop(conn);
        let imported = portable::import_at(target.path(), request).unwrap();
        let new_id = imported.imported_case_id.unwrap();
        let conn = authority::connect_readonly(target.path()).unwrap();
        let target_snapshot = crate::analysis_context::read(&conn, &new_id).unwrap();
        drop(conn);
        assert_eq!(target_snapshot.visibility_revision, 1);
        assert_eq!(
            ledger::list(target.path(), &target_snapshot.identity(), None, 100)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            std::fs::read(target.path().join("case-images").join(&image_id)).unwrap(),
            image
        );
        let descriptor = &target_snapshot.config.references[0];
        assert_eq!(descriptor.interpretation_version, 2);
        let source = references::portable_source(
            target.path(),
            &references::Owner {
                case_id: new_id,
                analysis_id: target_snapshot.analysis_id,
            },
            descriptor,
            &|| false,
        )
        .unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut source.source_reader().unwrap(), &mut bytes).unwrap();
        assert_eq!(bytes, reference_bytes);
    }
    #[test]
    fn native_licase_literal_scope_and_receipts_survive_reopen_and_reexport() {
        use crate::{case_archive::work, exclusion_store as ledger};
        let variants = [
            json!({"$serde_json::private::RawValue":"123"}),
            json!({"$serde_json::private::RawValue":"not JSON", "sibling":"retained"}),
            json!({"nested":[{"$serde_json::private::RawValue":"{\"changed\":true}","sibling":[1,2]}]}),
        ];
        let (source, id, _) = imported_fixture();
        for (index, scope) in variants.iter().enumerate() {
            let conn = authority::connect_readonly(source.path()).unwrap();
            let snapshot = crate::analysis_context::read(&conn, &id).unwrap();
            drop(conn);
            let staged = ledger::stage(
                source.path(),
                ledger::Admission {
                    analysis: snapshot.identity(),
                    source_receipt: variants[(index + 1) % variants.len()].clone(),
                },
                ledger::Purpose::Exclude,
                &[ledger::SourceDescriptor {
                    version: "literal-source".into(),
                    record_space: "literal-records".into(),
                    label: "Source".into(),
                    event_ref_prefix: Some("literal".into()),
                }],
                vec![Ok(ledger::InputMember {
                    source_index: 0,
                    locator: ledger::Locator::ByteOffset(index as u64),
                    event_ref: format!("literal:{index}"),
                })],
                &ledger::Budget::default(),
                &work(),
            )
            .unwrap();
            ledger::publish(
                source.path(),
                staged,
                &format!("literal-{index}"),
                "fixture",
                scope.clone(),
                &ledger::Budget::default(),
                &work(),
            )
            .unwrap();
        }
        let assert_reopened = |root: &Path, id: &str| {
            let conn = authority::connect_readonly(root).unwrap();
            let snapshot = crate::analysis_context::read(&conn, id).unwrap();
            drop(conn);
            let batches = ledger::list(root, &snapshot.identity(), None, 100).unwrap();
            assert_eq!(batches.len(), variants.len());
            for batch in batches {
                let index: usize = batch
                    .label
                    .strip_prefix("literal-")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(batch.scope, variants[index]);
                assert_eq!(batch.source_receipt, variants[(index + 1) % variants.len()]);
            }
        };
        assert_reopened(source.path(), &id);
        let files = tempfile::tempdir().unwrap();
        let first = files.path().join("literal-first.licase");
        portable::export_at(source.path(), export_request(source.path(), &id, &first)).unwrap();
        let (target, _, _) = authority::tests::fixture();
        let imported = portable::import_at(
            target.path(),
            ImportRequest {
                store: load_view(target.path()).unwrap().document().store.clone(),
                path: first.to_string_lossy().into(),
                request_id: "literal-import".into(),
            },
        )
        .unwrap();
        let imported_id = imported.imported_case_id.unwrap();
        assert_reopened(target.path(), &imported_id);
        let second = files.path().join("literal-second.licase");
        portable::export_at(
            target.path(),
            export_request(target.path(), &imported_id, &second),
        )
        .unwrap();
        let (reopened, _, _) = authority::tests::fixture();
        let imported = portable::import_at(
            reopened.path(),
            ImportRequest {
                store: load_view(reopened.path()).unwrap().document().store.clone(),
                path: second.to_string_lossy().into(),
                request_id: "literal-reimport".into(),
            },
        )
        .unwrap();
        assert_reopened(reopened.path(), imported.imported_case_id.as_ref().unwrap());
    }
    #[test]
    fn valid_large_context_uses_metadata_admission_through_native_json_roundtrip() {
        let (source, id, _) = imported_fixture();
        let conn = Connection::open(source.path().join("investigations.sqlite3")).unwrap();
        let mut snapshot = crate::analysis_context::read(&conn, &id).unwrap();
        let future = "x".repeat((5 << 20) / 4);
        snapshot.config.derived_fields.push(json!({"name":"largeFutureMetadata","source":"message","pattern":"(.*)","futureMetadata":future}));
        snapshot.config_revision += 1;
        crate::analysis_context::validate_snapshot_size(&snapshot).unwrap();
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(encoded.len() > 1 << 20 && encoded.len() < 4 << 20);
        assert!(
            parse_exact_value(&encoded).is_err(),
            "record-value budget is deliberately too small for this valid context"
        );
        conn.execute(
            "UPDATE case_analysis SET body=?1 WHERE case_id=?2",
            params![encoded, id],
        )
        .unwrap();
        drop(conn);
        let external = tempfile::tempdir().unwrap();
        let path = external.path().join("large-context.json");
        portable::export_at(source.path(), export_request(source.path(), &id, &path)).unwrap();
        let (target, _, _) = authority::tests::fixture();
        let store = load_view(target.path()).unwrap().document().store.clone();
        let imported = portable::import_at(
            target.path(),
            ImportRequest {
                store,
                path: path.to_string_lossy().into(),
                request_id: "large-context-roundtrip".into(),
            },
        )
        .unwrap();
        let conn = authority::connect_readonly(target.path()).unwrap();
        let reopened =
            crate::analysis_context::read(&conn, imported.imported_case_id.as_deref().unwrap())
                .unwrap();
        assert_eq!(reopened.config, snapshot.config);
    }
    #[test]
    fn changed_store_and_fabricated_native_references_preserve_destination() {
        let (source, id, _) = imported_fixture();
        let external = tempfile::tempdir().unwrap();
        let path = external.path().join("existing.json");
        std::fs::write(&path, b"untouched destination").unwrap();
        let mut request = export_request(source.path(), &id, &path);
        request.store.epoch = uuid::Uuid::new_v4().to_string();
        assert!(portable::export_at(source.path(), request).is_err());
        let mut request = export_request(source.path(), &id, &path);
        let mut view: Value = serde_json::from_str(&request.document_json).unwrap();
        view["cases"][0]["items"][0]["rows"]["reference"]["manifestId"] =
            json!(uuid::Uuid::new_v4().to_string());
        request.document_json = view.to_string();
        assert!(portable::export_at(source.path(), request).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"untouched destination");
    }
}
